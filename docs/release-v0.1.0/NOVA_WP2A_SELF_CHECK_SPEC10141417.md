# WP2A：结构化自检契约与完工门禁规格说明（第六版，权威文件，关闭剩余3个Major）

**权威声明：** 本文件与配套的 `docs/release-v0.1.0/NOVA_SINGLE_AI_MIGRATION_PLAN10141420.md` 是本轮唯一权威方案，自包含，实现者不需要回查`...10141347.md`及更早版本才能完成WP2A-0/WP2A主体。
**状态：** 规格设计稿，**未实现任何代码**。方案修正不等于代码实现或运行验收通过。
**回应对象：** `D:\nova\docs\reviews\CODEX_WP2A_SPEC_CLOSURE_REVIEW10141407.md` 剩余3个Major：原M1（执行上下文attempt绑定未真正关闭）、原M2（WP2A-0核心契约缺失）、B4衍生Major（冻结入口未覆盖确认/恢复命令在状态转换前的写入）。**原B1/B2/B3/m1已被该审核判定关闭，本轮不重新设计，原样维持。**

---

## 0. 本版关闭对照表

| 编号 | 问题 | 本版位置 |
|---|---|---|
| M1 | `Started`分支仍丢弃`start_action`返回值，完成时无CAS校验 | 第1节：`ActionGate`携带`TaskAction`，`finish_action`新增`expected_attempt`CAS |
| M2 | `confirmed_requirement_items`/`generate_requirement_snapshot`契约缺失 | 第2节：完整DDL+拆分规则+命令契约 |
| B4衍生 | `confirm_no_tests_applicable`/`confirm_no_build_applicable`等入口在状态转换前已写action/confirmation | 第5.3节：两个公共入口函数顶部新增冻结检查 |
| Minor×2 | `no_build_applicable_is_authorized`签名、预览措辞 | 第3.6/6节顺手澄清 |

---

## 1. 执行上下文：执行前绑定attempt，完成时CAS校验（回应M1）

### 1.1 代码依据（本轮逐一核验，不是转抄审核意见）

- `storage/actions.rs:67-108`的`start_action`：通过一条条件`UPDATE`把动作标记为`started`，**已经**在同一次调用里`self.find_action(...)?`读回并返回完整`TaskAction`（含启动那一刻的`attempt_number`）——这个值是"执行前"就存在的，不是"事后"才产生的，只是上层代码没有使用它。
- `storage/actions.rs:146-179`的`finish_action`：`UPDATE`条件是`WHERE task_id=?5 AND idempotency_key=?6 AND status='started'`，**不校验`attempt_number`**——这意味着如果调用者A在attempt 1时启动，输出因故晚到；期间action已经因恢复/重试流程进入attempt 2的`started`，A此刻调用`finish_action`仍会命中`status='started'`这个条件（因为attempt 2也是`started`状态），把attempt 2错误标记为成功，而实际完成的是attempt 1的旧工作。
- `core_loop.rs`里现有`gate_action`/`finish_gated_action`（第1545-1578、2777-2808行附近）在`Started`分支把`db.start_action(...)`的返回值丢弃（`Ok(_) => Ok(ActionGate::Started)`），`finish_gated_action`原样转发`db.finish_action(...)`的结果但不传入任何"预期attempt"。

### 1.2 决定：`ActionGate`携带完整`TaskAction`，`finish_action`新增CAS参数

```rust
enum ActionGate {
    Started(TaskAction),           // 携带 start_action 返回的完整记录——这就是"执行前绑定"
    AlreadySucceeded(TaskAction),  // 携带已成功那次的完整记录
    Uncertain,
}

fn gate_action(db: &Database, task_id: &str, action_type: &str, key: &str, is_read_only: bool) -> NovaResult<ActionGate> {
    let planned = db.plan_action(task_id, action_type, key)?;
    if planned.status == ActionStatus::Uncertain { return Ok(ActionGate::Uncertain); }
    if planned.status == ActionStatus::Succeeded { return Ok(ActionGate::AlreadySucceeded(planned)); }
    match db.start_action(task_id, key, is_read_only) {
        Ok(action) => Ok(ActionGate::Started(action)),      // 直接带出返回值，不丢弃
        Err(_) => match db.find_action(task_id, key)? {
            Some(action) if action.status == ActionStatus::Succeeded => Ok(ActionGate::AlreadySucceeded(action)),
            Some(action) if action.status == ActionStatus::Uncertain => Ok(ActionGate::Uncertain),
            _ => Ok(ActionGate::Uncertain),
        },
    }
}
```

```rust
// Database::finish_action 新增 expected_attempt 参数，UPDATE 条件同时校验
pub fn finish_action(
    &self, task_id: &str, idempotency_key: &str, status: ActionStatus,
    evidence_ref: Option<&str>, error_summary: Option<&str>,
    expected_attempt: i64,                                   // 新增
) -> NovaResult<TaskAction> {
    let rows_affected = self.conn().execute(
        "UPDATE task_actions SET status=?1, finished_at=?2, evidence_ref=?3, error_summary=?4, updated_at=?2
         WHERE task_id=?5 AND idempotency_key=?6 AND status='started' AND attempt_number=?7",
        params![status.as_str(), now, evidence_ref, error_summary, task_id, idempotency_key, expected_attempt],
    )?;
    if rows_affected == 0 {
        // 不区分"早就不是started"还是"attempt已变"，统一按陈旧完成 fail-closed：
        // 不读当前行当返回值、不假装成功，直接拒绝。
        return Err(NovaError::Config(
            "动作完成校验失败：当前执行归属与启动时不一致（可能已被更新的执行取代），已拒绝写入".to_string(),
        ));
    }
    self.find_action(task_id, idempotency_key)?.ok_or_else(|| NovaError::Database("动作记录在更新后丢失".to_string()))
}
```

```rust
// core_loop.rs 的 finish_gated_action 同步新增 expected_attempt 参数，原样转发：
fn finish_gated_action(db: &Database, task_id: &str, key: &str, succeeded: bool,
                        result: &ExecutionResult, expected_attempt: i64) -> NovaResult<TaskAction> {
    let status = if succeeded { Succeeded } else if was_cancelled(result) { Cancelled } else { Failed };
    db.finish_action(task_id, key, status, evidence_ref, error_summary, expected_attempt)
}
```

### 1.3 调用方使用方式（`run_dev_and_test_round`，完整流程）

```rust
let result_attempt: i64 = match gate_action(db, task_id, "ai_development", &dev_key, false)? {
    ActionGate::Uncertain => { /* 既有：转needs_user_action，return ActionUncertain */ }
    ActionGate::AlreadySucceeded(action) => action.attempt_number,   // 执行前就确定，不调用AI
    ActionGate::Started(action) => {
        let captured_attempt = action.attempt_number;                // 【执行前绑定】在调用AI之前固定
        let result = adapter.run_development(&dev_request, cancel.clone());
        record_command_evidence(db, project_id, task_id, &result, "ai_development")?;  // 不参与门禁判定，现状不变，照常记录
        let succeeded = matches!(result.outcome, ExecutionOutcome::Success);
        match finish_gated_action(db, task_id, &dev_key, succeeded, &result, captured_attempt) {
            Ok(finished) => {
                if !succeeded { /* 既有失败/取消处理，return */ }
                finished.attempt_number   // 与 captured_attempt 必然相等（CAS已验证），仅为类型统一直接用它
            }
            Err(_) => {
                // 【完成时CAS失败】说明本次执行已被更晚的一次attempt取代，是陈旧结果。
                // 不得继续把这次(可能已经成功)的AI输出当作当前结果：
                // 不写自检、不写test/build证据、不转换任务状态——直接静默停止本次round处理。
                return Ok(DevTestOutcome::Stale);   // 新增枚举值，调用方视为等同Stopped：不前进、不报错给用户
            }
        }
    }
};
// result_attempt 之后原样传给自检写入、run_tests_for_round、run_build_for_round（见第3节）
```

### 1.4 陈旧结果允许保留哪些审计记录，不得写成什么

| 产物 | 陈旧执行是否仍会产生 | 是否可保留 | 是否可被当作"当前执行成功证据" |
|---|---|---|---|
| `command_evidence`（原始命令日志） | 会（在CAS检查**之前**就已经写入，现状`record_command_evidence`本就不参与完工门禁判定，不改变） | 可以保留，作为历史审计 | 不适用——该表从来不是门禁判定输入 |
| `task_actions`完成状态写入 | **不会**——CAS失败直接拒绝，`task_actions`行维持被更新attempt留下的状态，不被陈旧调用覆盖 | — | 不适用，没有写入 |
| 自检（`self_checks`） | **不会**——自检写入发生在`finish_gated_action`成功返回之后，CAS失败时代码直接`return Stale`，根本不会走到自检解析这一步 | — | 不适用，没有产出 |
| 测试/构建证据 | **不会**——同理，`run_tests_for_round`/`run_build_for_round`在`result_attempt`确定之后才被调用，陈旧路径在此之前已经退出 | — | 不适用，没有产出 |

**结论：** 只有不参与完工门禁判定的原始命令日志会为陈旧执行留痕（纯审计用途），自检、测试证据、构建证据这三类真正被完工门禁使用的产物，陈旧执行**从根本上不会产生**——不是"产生了但被标记为无效"，是"CAS失败后代码路径直接终止，后续步骤未发生"。

### 1.5 验收用例（回应"attempt1晚到、attempt2已started"的具体场景）

| 场景 | 预期行为 |
|---|---|
| attempt 1正常走完（启动→调用AI→finish），期间无并发 | `finish_action`的CAS条件天然满足（`attempt_number`没变过），正常成功 |
| attempt 1启动后AI调用中，恢复/重试流程把该action重置为attempt 2并`started` | attempt 1的AI输出返回后调用`finish_action(expected_attempt=1)`，此刻行的真实`attempt_number`已是2，`UPDATE`的`WHERE attempt_number=1`匹配不到任何行，`rows_affected=0`，CAS失败，拒绝写入，`task_actions`保持attempt 2的`started`状态不受扰动 |
| attempt 1晚到的输出不会被采纳为自检/测试/构建的依据 | 因为`finish_gated_action`对attempt 1返回`Err`，调用方直接`return Stale`，不执行后续任何写入 |
| attempt 2随后正常完成 | attempt 2自己调用`finish_action(expected_attempt=2)`，此刻行仍是attempt 2的`started`，CAS通过，正常成功，后续自检/证据按`result_attempt=2`写入 |

---

## 2. WP2A-0完整契约（回应M2）

### 2.1 `confirmed_requirement_items`完整DDL

```sql
CREATE TABLE confirmed_requirement_items (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  field TEXT NOT NULL CHECK(field IN ('goal','in_scope','not_in_scope','done_criteria')),
  item_index INTEGER NOT NULL CHECK(item_index >= 0),
  item_text_redacted TEXT NOT NULL CHECK(length(item_text_redacted) > 0),   -- 空行在拆分阶段已被丢弃，不会产生空字符串行
  created_at TEXT NOT NULL,
  UNIQUE(task_id, field, item_index)
);
CREATE INDEX idx_confirmed_requirement_items_task ON confirmed_requirement_items(task_id);
```

**稳定item标识：** `(field, item_index)`二元组即为稳定标识，`field`取值固定为四个枚举之一，`item_index`是该字段内部、拆分后剩余有效行的从0开始的出现顺序号——不随后续任何操作重新编号（因为这四个源字段创建后不可编辑，已在此前核验确认：全仓库搜索`commands.rs`/`storage/tasks.rs`无任何"更新任务goal/in_scope/not_in_scope/done_criteria"的命令，`create_task`是唯一写入点）。

### 2.2 四个字段的稳定拆分规则（精确定义，不留弹性空间）

对`goal`/`in_scope`/`not_in_scope`/`done_criteria`四个`Option<String>`字段，各自独立执行：

1. 若字段为`None`或字符串为空（trim后长度为0），该字段贡献**零**个需求项，跳过。
2. 否则，按`\n`分割字符串为若干行（Windows环境下`\r\n`先统一替换为`\n`，避免`\r`残留进入条目文本）。
3. 对每一行执行`trim()`去除首尾空白。
4. **丢弃**trim后长度为0的行（纯空行、只有空格的行）——不生成对应的`item_index`，不保留"空洞"编号（即剩余行紧密重排为0,1,2...，不是原始行号）。
5. 剩余的非空行，按其在**丢弃空行之后**的相对顺序，从0开始分配`item_index`，写入`item_text_redacted`（内容已做常规脱敏处理，与其余持久化文本字段一致）。
6. 顺序必须保留——`item_index`严格反映用户原始输入中各条目的先后关系，不重新排序、不去重合并相同文本的行（若用户恰好写了两行完全相同的内容，保留为两条独立item，因为它们在原始输入里就是两条）。

**示例：** `in_scope = "支持导出Excel\n\n支持导出PDF  \n支持导出Excel"` → 拆分得到3项：`(in_scope,0,"支持导出Excel")`、`(in_scope,1,"支持导出PDF")`、`(in_scope,2,"支持导出Excel")`（中间空行丢弃，重复文本保留为两条独立item）。

### 2.3 新任务的快照生成时机与用户确认的关系

**写入时机：** 在`Database::transition_task`的`to == TaskState::AwaitingConfirmation`分支内（即用户点击"我已确认需求，继续"触发的那次转换，`apps/desktop/src/stores/coreLoop.ts:468-476`对应的后端调用），在**同一个**`BEGIN IMMEDIATE`事务内，紧接着既有的状态写入逻辑之后，执行2.2节的拆分规则并批量插入。

**与用户确认的关系：** 这次转换本身**就是**"用户确认需求"的动作（`AwaitingConfirmation`这个状态名的含义是"等待确认"，转入这个状态是确认动作的结果，不是确认动作本身的前置）——所以快照的生成天然绑定在用户确认那一刻，不需要额外的二次确认步骤。

### 2.4 不允许静默覆盖已确认快照

```rust
// transition_task 的 to==AwaitingConfirmation 分支内：
if self.list_confirmed_requirement_items(task_id)?.is_empty() {
    self.insert_confirmed_requirement_items(task_id, &split_requirement_fields(&task))?;
}
// 若已有行（理论上不应该，因为这四个字段不可编辑、这个转换对同一任务只会走一次），
// 直接跳过插入——绝不删除已有行后重新生成，避免任何形式的"重新确认时覆盖旧快照"。
```

### 2.5 重复调用、失败、部分写入的事务/幂等规则

- **重复调用`transition_task(to=AwaitingConfirmation)`：** 第二次调用时任务已经离开`Draft`状态，`is_legal_transition`本身就会拒绝这次转换（现有机制，未改变），因此2.4节的"快照已存在則跳過"分支在实践中只会在`Draft→AwaitingConfirmation`这唯一一次合法转换时真正执行写入；它的存在是防御性的，不依赖转换合法性检查本身。
- **失败/部分写入：** 拆分+插入在同一个`BEGIN IMMEDIATE`事务内与状态写入一起提交或一起回滚——不存在"状态已经变成`AwaitingConfirmation`但快照只插入了一半"的中间态。
- **幂等：** 同一事务内"若已有行则跳过"的检查，使得即便这段代码在同一次事务里被意外调用两次（不应该发生，但做防御性处理），也不会产生重复行（`UNIQUE(task_id,field,item_index)`本身也是最后一道防线）。

### 2.6 `generate_requirement_snapshot`完整命令契约（回应M2明确要求）

```rust
pub fn generate_requirement_snapshot(
    db: &Database,
    project_id: &str,
    task_id: &str,
) -> NovaResult<Vec<ConfirmedRequirementItem>>
```

| 契约项 | 定义 |
|---|---|
| **项目与任务归属** | 必须核验`task.project_id == project_id`，不匹配则拒绝（与仓库内其余命令的既有归属校验模式一致） |
| **允许状态** | 任务必须处于**非终态**（`state NOT IN ('completed','failed','cancelled')`）——终态任务永远拒绝，无论是否有快照 |
| **输入来源** | 任务当前（且唯一、不可变）的`goal`/`in_scope`/`not_in_scope`/`done_criteria`四个字段，**只读**，不接受调用方传入任何新文本 |
| **用户明确确认要求** | 本命令**不代表**"用户刚刚做出了确认"——它只是把已经确认过、从未变过的历史文本结构化落地。前端在调用它之前，必须先把这四个字段的原文**只读展示**给用户看一遍，并要求用户点击一个明确的"确认沿用，生成需求清单"按钮；命令本身不校验这个UI步骤是否发生（后端无法验证前端是否真的展示过），这是产品流程层面的约束，不是本命令的参数校验职责——**但命令的存在前提是"任务已经在更早的某次`AwaitingConfirmation`转换里被用户确认过需求"，这件事本身由任务当前处于非`Draft`状态这一事实保证**（因为`Draft`状态的任务从未经过确认，不会出现在"旧任务无快照"场景里——它们走2.3节的正常自动生成路径） |
| **输出** | 返回新插入的`ConfirmedRequirementItem`列表（与2.1节DDL对应的行） |
| **重复调用行为** | 若该`task_id`已存在快照行，**直接返回现有行，不重新生成、不追加、不报错**（幂等，与2.4节"不允许静默覆盖"一致——"不静默覆盖"不等于"报错"，重复调用是安全的只读式幂等操作） |
| **已有快照处理** | 见上一行——已有则原样返回 |
| **空需求处理** | 若四个字段按2.2节规则拆分后总条目数为0（用户四个字段全部为空/全部是空行），本命令仍可执行但不插入任何行，返回空列表；下游完工门禁遇到这种"确实没有任何需求项"的任务时，按第3节`RequirementSnapshotMissing`的同一拒绝路径处理（空快照与"从未生成快照"在门禁判定里是同一种不满足条件，不需要额外区分） |
| **失败条件** | 项目/任务不匹配；任务处于终态 |
| **是否受冻结限制** | **不受限制，允许对`legacy_classification`已冻结的任务调用**（第5节已明确：本命令只补需求确认结构化记录，不涉及继续执行，调用后冻结状态不变） |

### 2.7 旧非终态任务没有快照时的边界（重申，呼应第5节冻结规则）

旧任务不要求重新描述需求——2.6节"输入来源"已经保证只读取历史文本。展示已有内容后，用户点击确认按钮触发2.6节命令，**不得**由后端自动调用（不存在"用户打开任务页面就自动生成快照"的隐式触发）。历史已完成任务（`Completed`）不回填，不改写（2.6节"允许状态"已排除终态）。

---

## 3. 最终完工门禁（维持已关闭的B2设计，原样内联，不重新设计）

### 3.1-3.5 完整判定逻辑

与 `NOVA_WP2A_SELF_CHECK_SPEC10141347.md` 第3节的`completed_gate_checks`伪代码完全一致（该节已被Codex判定"方案层面已满足"），原样内联：7步检查——历史冻结（见第5节更新）、adopted evidence所在(generation,round)、当前开发action真实attempt、同attempt测试证据或授权、同attempt构建证据或授权、当前有效自检、需求覆盖（现在明确引用第2节`confirmed_requirement_items`）、当前用户确认；全部在`Database::transition_task`的`to==Completed`分支、同一`BEGIN IMMEDIATE`事务内完成；门禁失败时拒绝转换、保持`AwaitingUserAcceptance`、不写`NeedsUserAction`、原因只通过API返回（第6节m1结论维持）。

### 3.6 `no_build_applicable_is_authorized`明确签名（回应Minor 1）

```rust
pub(crate) fn no_build_applicable_is_authorized(
    db: &Database,
    project_id: &str,
    task_id: &str,
    result_generation: i64,
    round: i64,
    result_attempt: i64,                     // 新增，明确参数，不再用"..."省略
    authorization_source: Option<&str>,
) -> NovaResult<bool> {
    if let Some(confirmation) = db.find_no_build_confirmation(project_id, task_id, result_generation, round, result_attempt)? {
        return Ok(true);   // find_no_build_confirmation 内部已按 result_attempt 精确匹配（见上一版第2.3节）
    }
    // mechanical_not_applicable 路径：直接信任持久化在 build_evidence 行上的 authorization_source，
    // 但调用方必须已经确认这条 build_evidence 行的 result_attempt == 当前 result_attempt（第3.1节第4步的职责），
    // 本函数不重复读取 evidence 行，只负责判断传入的 authorization_source 字符串本身。
    Ok(authorization_source == Some("mechanical_not_applicable"))
}
```

`no_tests_applicable_is_authorized`同构，新增`result_attempt`参数，传给`find_no_tests_confirmation`。

---

## 4. 历史分类（维持已关闭的B1设计，原样内联）

与 `NOVA_WP2A_SELF_CHECK_SPEC10141347.md` 第4节一致（已被Codex判定关闭）：强信号（`ai_review`action/`task_reviews`行）OR判定`dual_ai_evidenced`；弱信号（磁盘文档内容）只做`integrity_warning`；其余归`unknown_insufficient`；`single_ai_evidenced`当前永不触发但枚举保留。

---

## 5. 旧非终态任务的冻结——补齐确认/恢复命令入口（回应B4衍生Major）

### 5.1 持久化机制（维持已关闭部分，原样内联）

`tasks.legacy_classification`列，只在WP2A迁移那一刻为**当时**非终态的任务一次性计算并写入；迁移后新建任务恒为`NULL`，不受冻结检查影响（避免误伤新任务）。

### 5.2 已确认的5个入口（维持，原样内联）

`run_core_loop`启动检查、`unblock_test_execution_for_retry`、恢复扫描（纯记账，不触发执行）、通用`transition_task`到`Queued`/`Running`、最终`Completed`门禁第0步。

### 5.3 新增：确认/恢复命令入口的冻结检查（回应本轮衍生Major，核心新增）

**代码依据（本轮核验）：** `core_loop.rs:1204`的`pub fn confirm_no_tests_applicable`与`core_loop.rs:818`的`pub fn confirm_no_build_applicable`是两个真实暴露的公共入口（经`commands.rs`接到Tauri命令）。其内部调用链（`reconcile_no_tests_confirmation`→`reconcile_no_tests_confirmation_attempt`，`core_loop.rs:1293`起）会依次：① 写入`no_tests_confirmations`行（约1354行）；② 可能调用`db.reset_failed_action_for_retry(...)`重置action（约1382行）；③ 调用`transition_task(..., Queued, ...)`（约1396-1404行）。**这三步此前都没有任何冻结检查**——即使第③步最终会被第5.2节"通用`transition_task`到`Queued`"的检查拦住，第①②步已经先把`confirmation`行写入、把`action`重置，造成冻结任务的审计状态被污染（不是"只读查看"或"安全取消"，是继续执行的准备动作）。

**决定：** 在两个公共入口函数**最开头**（在调用`reconcile_...`之前）新增冻结检查，一次检查保护整条调用链，不需要在内部三个步骤分别打补丁：

```rust
pub fn confirm_no_tests_applicable(
    db: &Database, project_id: &str, task_id: &str, reason: Option<&str>,
) -> NovaResult<NoTestsConfirmation> {
    let task = db.task(task_id)?;
    if is_legacy_frozen(&task) {           // 新增：legacy_classification IN ('dual_ai_evidenced','unknown_insufficient')
        return Err(NovaError::Config("该任务的历史执行路径无法确认，已冻结，不能继续处理".to_string()));
    }
    reconcile_no_tests_confirmation(db, project_id, task_id, reason)
}
// confirm_no_build_applicable 同构，入口在 core_loop.rs:818
```

**为什么不需要事务化合并（PM给出的二选一里选择第一种）：** 因为检查发生在**调用任何写入操作之前**（函数最开头，`reconcile_...`尚未被调用），天然保证"冻结时不写任何action/confirmation变化"，不需要把检查、reset、写入、transition四步包进同一个事务再在中途回滚——根本不会执行到那四步。

**恢复扫描的中断记账与此处的区分：** `run_startup_recovery`（第5.2节已覆盖的入口）只做"把运行中的任务标记为`interrupted`"这类**只读/记账性质**的操作，不涉及`reset_failed_action_for_retry`或写`confirmation`，因此不受本节影响，也不需要加冻结检查——它和`confirm_no_tests_applicable`/`confirm_no_build_applicable`的区别正是"记账" vs "继续执行准备动作"。

### 5.4 冻结拒绝的状态/事件一致性（验收要求的"前后对比"）

| 检查项 | 冻结拒绝前 | 冻结拒绝后 |
|---|---|---|
| 任务状态 | 维持调用前的状态（如`NeedsUserAction`） | 不变（函数在写入前已返回错误） |
| action状态及attempt | 维持调用前的值 | 不变 |
| `no_tests_confirmations`/`no_build_confirmations` | 调用前没有本次尝试对应的新行 | 仍然没有——函数在到达`record_no_tests_confirmation`之前就已经返回 |
| `task_state_events` | 调用前的最后一条事件不变 | 不新增任何事件行（因为没有发生状态转换） |

### 5.5 允许的操作边界（维持，原样内联）

只读查看、取消（`transition_task(to=Cancelled)`）、`generate_requirement_snapshot`（第2.6节已明确不受冻结限制）——三者均不触发5.3节新增的检查（前两者不经过`confirm_no_tests_applicable`/`confirm_no_build_applicable`这条调用链，`generate_requirement_snapshot`是独立命令，设计上就不检查冻结）。

### 5.6 `generate_requirement_snapshot`不解除冻结（重申，原样内联）

调用该命令不修改`tasks.legacy_classification`列，冻结状态在调用前后完全一致。

### 5.7 解冻机制仍不在本轮范围（维持）

---

## 6. 预览与完工分离（维持已关闭的B2/M2周边设计，顺手澄清Minor 2）

`adopted_passing_generation_round()`不改动，`preview_ready`进入条件不受`result_attempt`约束。**措辞澄清（回应Minor 2）：** 证据总览（evidence overview）/技术文档在展示`preview_ready`阶段的证据时，若该证据的`result_attempt`与当前`task_actions`最新attempt不一致，界面文案不得使用"当前已验证"一类措辞，应如实说明"该证据来自更早的一次执行"，避免用户误以为预览内容已经代表最新一次开发/修复的真实验证结果——这只是展示文案的澄清，不改变`preview_ready`本身不依赖`result_attempt`的既有规则（第6节标题所述分离原则维持不变）。

---

## 7. 已关闭规则重申（防止后续轮次误改）

失败保持原状态（不转`NeedsUserAction`）；预览与完工分离；自检必须`self_reported_complete`且两类数组均为空；不默认新增AI调用；一次修复上限（`MAX_AUTO_FIX_CYCLES`/`current_pending_round`）现状不变；既有无测试/无构建授权规则保留，只新增`result_attempt`维度；拒绝原因只API返回不新增事件系统；唯一键扩展含`result_attempt`、依赖SQL的NULL不冲突语义；历史分类OR判定双AI、弱信号降级为`integrity_warning`。

---

## 8. 验收矩阵（新增/更新本轮相关场景，其余与上一版一致不重复抄录）

| # | 场景 | 预期结果 |
|---|---|---|
| 1 | attempt1晚到，attempt2已started（本轮核心场景） | attempt1的`finish_action`CAS失败，不覆盖attempt2，不产出自检/证据，调用方返回`DevTestOutcome::Stale`，静默停止 |
| 2 | 旧AI输出、自检、测试、构建均不能满足attempt2 | 见第1.4节：陈旧执行根本不产出自检/测试/构建证据，无需"满足"与否的判断，天然不存在 |
| 3 | 新建`confirmed_requirement_items`（含空行/重复行拆分） | 按2.2节规则精确拆分，验证示例见2.2节 |
| 4 | `generate_requirement_snapshot`对已有快照任务重复调用 | 幂等返回现有行，不报错，不重新生成 |
| 5 | `generate_requirement_snapshot`对全空需求任务调用 | 返回空列表，不插入行；下游完工门禁按`RequirementSnapshotMissing`处理 |
| 6 | 冻结任务调用`confirm_no_tests_applicable` | 函数开头即拒绝，不写`no_tests_confirmations`，不reset action，不transition |
| 7 | 冻结任务调用`confirm_no_build_applicable` | 同上 |
| 8 | 冻结任务调用`generate_requirement_snapshot` | 允许执行，冻结状态不变 |
| 9 | 冻结任务取消 | 允许，转入`Cancelled` |
| 10 | 冻结任务只读查看 | 允许 |
| 11 | 旧非终态任务无快照直接走向完工 | `RequirementSnapshotMissing`拒绝 |

其余场景（历史NULL、新attempt重新授权、直接状态转换绕过、并发变化、真实证据回归、一次修复上限回归等）与`NOVA_WP2A_SELF_CHECK_SPEC10141347.md`第8节一致，不重复抄录（该节内容已被Codex判定对应的B1/B2/B3部分关闭，本轮未改变这些场景的预期结果）。

---

## 9. 本文件状态声明

本文件是**规格设计稿**。`ActionGate`/`finish_action`签名改动、`confirmed_requirement_items`表、`generate_requirement_snapshot`命令、两个确认入口的冻结检查、`legacy_classification`列及其余全部机制**均未实现**。需经 Codex 独立复审通过后，才能转入编码工作包。
