# WP2A 补充规格（回应 CODEX_WP2A_SPEC_FINAL_CLOSURE_REVIEW10141613.md 剩余3个Major+2处文字）

**性质：** 本文件是**补充规格**，不是对既有权威文件的整体替代。本轮只修订`D:\nova\docs\reviews\CODEX_WP2A_SPEC_FINAL_CLOSURE_REVIEW10141613.md`指出的3个Major（test/build evidence写入顺序、Stale与真实错误混淆、`generate_requirement_snapshot`允许状态含Draft）及2处文字澄清（`run_startup_recovery`措辞、权威文件关系）。**不重写已被该审核判定关闭或维持的设计**（历史分类、completed门禁7步核心逻辑、唯一键扩展、拒绝原因API返回、B4冻结检查主体）。
**状态：** 规格设计稿，**未实现任何代码**。方案修正不等于实现或运行验收通过。

---

## 0. 权威文件关系（回应"文字与权威关系"，不使用占位引用）

| 文件 | 承载范围 | 本轮状态 |
|---|---|---|
| `docs/release-v0.1.0/NOVA_WP2A_SELF_CHECK_SPEC10141417.md` | 历史分类（第4节）、completed门禁7步核心逻辑（第3.1-3.5节）、唯一键扩展（第3.6节相关）、`confirmed_requirement_items`DDL与拆分规则（第2.1-2.5节）、冻结检查主体与5个入口（第5节，除5.3节两处文字外）、拒绝原因API返回（已关闭） | **继续有效，不作废**，本文件只覆盖/追加下方列出的具体章节 |
| `docs/release-v0.1.0/NOVA_SINGLE_AI_MIGRATION_PLAN10141420.md` | 对上表的指针式汇总 | 同上，继续有效 |
| `docs/release-v0.1.0/NOVA_WP2A_SELF_CHECK_SPEC10141347.md` | 被`10141417.md`第3/4/6/7/8节引用为"已关闭设计来源"的历史版本 | **明确列为保留的权威附件**：`10141417.md`对它的引用不是"占位"，是"该部分设计已经稳定、本轮未改动，细节仍记录在该历史文件对应章节"——即原始完工门禁7步伪代码、历史分类组合表等**仍以`10141347.md`对应章节的文字为准**，`10141417.md`不重复抄录是为了避免反复转录导致的走样，不是说这些内容不再承载规则 |
| **本文件（`NOVA_WP2A_FINAL_AMENDMENT10141726.md`）** | 下方第1/2/3节：仅覆盖本轮3个Major对应的具体机制；第4节：2处文字修正 | 新增，与上述文件**不冲突，是追加/覆盖关系**：凡本文件明确给出新设计的点，以本文件为准；凡本文件未提及的点，以`10141417.md`（及其引用的`10141347.md`对应章节）为准 |

**不得重新引入的已作废规则（明示，防止实现者走回头路）：** `self_checks`的生成/构建唯一约束曾经考虑过"覆盖写入"语义，已被`10141347.md`/`10141417.md`否决，固定为"追加写入+取`rowid`最新一行"；`user_acceptance`曾经考虑过加`round`列，已被否决，固定为不改schema、靠`result_generation`原子递增实现失效；历史分类曾经把"开发成功+无审核痕迹"当单AI正面证据，已被否决且不得恢复。

---

## 1. test/build action的CAS与evidence写入顺序（回应原M1未关闭问题1）

### 1.1 代码依据（本轮逐一核验，确认审核意见引用的行号属实）

- `run_tests_for_round`现状：`core_loop.rs:2172-2191`（无测试适用分支）、`2194-2226`（allowlist拒绝）、`2238-2270`（取消）、`2273-2300`（实际测试结果）——**全部**先调用`db.record_test_evidence(...)`，再调用`db.finish_action(task_id, &test_key, ...)`，且现状`finish_action`不带任何attempt校验。
- `run_build_for_round`现状：`core_loop.rs:2561-2591`（allowlist拒绝）、`2603-2633`（取消）、`2636-2661`起（实际构建结果）——同构，先写`record_build_evidence`，后`finish_action`。
- 两个函数开头调用`gate_action(db, task_id, "test_execution"/"build_execution", &test_key/&build_key, false)`，`Started`分支已按本次WP2A上一轮方案携带完整`TaskAction`（含该测试/构建action**自己的**`attempt_number`）。

### 1.2 关键澄清：开发`result_attempt`与测试/构建action自身attempt是两个不同概念

- `result_attempt`（上一轮已定义）= 开发/修复动作（`dev-{task_id}-round-{round}`）成功那一刻的`attempt_number`，写入`self_checks`/`test_evidence`/`build_evidence`的**归属标签**，用于completed门禁判断"这份证据是否对应当前开发结果"。
- **本节新增概念：** `test_action_attempt`/`build_action_attempt` = 测试/构建action**自己**（`test-{task_id}-generation-{g}-round-{r}`/`build-...`）的`attempt_number`，用于**该action自身**的CAS完成校验——这与"这份证据对应哪个开发结果"是两件不同的事：前者管"这次测试执行本身有没有被更晚的一次测试执行取代"，后者管"这次测试执行对应的开发产出是不是当前的"。**两者都要校验，缺一不可**，不能把其中一个的CAS当成另一个的保证。

### 1.3 决定：CAS成功后才允许写入可被门禁采纳的evidence

**原则：** 对`run_tests_for_round`/`run_build_for_round`的**全部**分支（无测试/无构建适用、allowlist拒绝、取消、timeout、passed/failed），统一调整为：**先对该action调用`finish_action(expected_attempt=test_action_attempt或build_action_attempt)`，CAS成功后才调用`record_test_evidence`/`record_build_evidence`**。

**写入顺序表：**

| 分支（test侧，build侧同构） | 现状顺序 | 修订后顺序 |
|---|---|---|
| 无测试适用（已有确认） | 写evidence(no_tests_applicable) → finish_action(Succeeded) | finish_action(Succeeded, expected_attempt) → **CAS成功后** → 写evidence(no_tests_applicable) |
| allowlist拒绝 | 写evidence(insufficient_evidence) → finish_action(Failed) | finish_action(Failed, expected_attempt) → **CAS成功后** → 写evidence(insufficient_evidence) |
| 取消 | 写evidence(cancelled) → finish_action(Cancelled) | finish_action(Cancelled, expected_attempt) → **CAS成功后** → 写evidence(cancelled) |
| 实际结果（passed/failed/timed_out） | 写evidence(status) → finish_action(Succeeded/Failed) | finish_action(Succeeded/Failed, expected_attempt) → **CAS成功后** → 写evidence(status) |

**最小伪代码（以"实际测试结果"分支为代表，其余分支同构替换状态值）：**

```rust
// run_tests_for_round 开头（gate_action返回Started时）：
let test_action_attempt = match gate_action(db, task_id, "test_execution", &test_key, false)? {
    ActionGate::Uncertain => { /* 既有逻辑，不变 */ }
    ActionGate::AlreadySucceeded(action) => {
        // 既有"按result_attempt比对"逻辑不变（上一轮已定义），此分支不涉及CAS（没有新执行）
    }
    ActionGate::Started(action) => action.attempt_number,   // 执行前捕获，与dev result_attempt是不同变量
};

// ...执行测试命令，得到 outcome/status... （现状逻辑不变）

// 【本节核心改动】先CAS，后写证据：
let finish_status = if status == "passed" { Succeeded } else if ... { Failed } else { Cancelled };
match db.finish_action(task_id, &test_key, finish_status, evidence_ref, error_summary, test_action_attempt) {
    Ok(_finished_action) => {
        // CAS成功，现在才允许写入可被门禁采纳的证据：
        db.record_test_evidence(&RecordTestEvidenceRequest { ..., status, ... })?;
        // ...原有的 transition_task 等后续逻辑不变...
    }
    Err(NovaError::StaleActionAttempt(_)) => {
        // 【本节关键】陈旧完成：不写任何evidence，不transition，静默返回
        return Ok(TestRoundOutcome::Stale);   // 新增枚举值，见第2节
    }
    Err(other) => return Err(other),   // 真实错误（数据库/IO/配置等），原样上抛，不吞掉
}
```

`run_build_for_round`按同一模式改写，`BuildRoundOutcome`同样新增`Stale`变体。

### 1.4 CAS成功但evidence写入前崩溃——维持既有fail-closed规则，不新增机制

**不新增处理逻辑。** 这个场景（action已经`Succeeded`，但对应的`test_evidence`/`build_evidence`行因崩溃未写入）**已经由现有代码覆盖**：`ActionGate::AlreadySucceeded`分支（见上一轮方案与`core_loop.rs`现状`run_tests_for_round`里"查无对应证据"的既有处理）在发现"action已成功但找不到证据"时，写入`insufficient_evidence`状态并转入`NeedsUserAction`，不假造证据、不自动重跑、不凭action成功就宣布通过。本节改动（调整写入顺序）**不改变**这条既有fail-closed路径，只是让"CAS失败"和"CAS成功但evidence未写"这两种情况都分别有明确、不冲突的既有/新增处理方式。

### 1.5 验收用例（回应"test/build attempt晚到但dev attempt未变"）

| 场景 | 预期行为 |
|---|---|
| dev `result_attempt`未变，但test action因恢复/重试已进入attempt 2，test attempt 1的命令输出晚到 | attempt 1调用`finish_action(expected_attempt=1)`，此刻行真实attempt已是2，CAS失败（`rows_affected=0`），返回`NovaError::StaleActionAttempt`，调用方匹配到该错误、返回`TestRoundOutcome::Stale`，**不写入`test_evidence`**，不覆盖attempt 2的action状态 |
| 同上，但晚到的是build attempt 1 | `BuildRoundOutcome::Stale`，同构处理，不写`build_evidence` |
| test attempt 2随后正常完成 | 自己的CAS基于`expected_attempt=2`成功，正常写入`test_evidence`，`result_attempt`标签为当前dev `result_attempt` |
| 无测试适用/allowlist拒绝/取消三类非"实际执行"分支同样发生attempt晚到 | 同样先CAS、CAS失败则不写对应evidence，验证"全部分支统一适用，不只覆盖happy path" |

---

## 2. 区分Stale与真实错误（回应原M1未关闭问题2）

### 2.1 新增错误类型

```rust
// error.rs，NovaError 新增一个变体：
pub enum NovaError {
    Config(String),
    Credential(String),
    Database(String),
    Network(String),
    Api(String),
    StaleActionAttempt(String),   // 新增：仅由 finish_action 的 CAS 未命中产生
}
```

### 2.2 `Database::finish_action`的区分写法

```rust
pub fn finish_action(&self, task_id: &str, idempotency_key: &str, status: ActionStatus,
                      evidence_ref: Option<&str>, error_summary: Option<&str>,
                      expected_attempt: i64) -> NovaResult<TaskAction> {
    let rows_affected = self.conn().execute(
        "UPDATE task_actions SET status=?1, finished_at=?2, evidence_ref=?3, error_summary=?4, updated_at=?2
         WHERE task_id=?5 AND idempotency_key=?6 AND status='started' AND attempt_number=?7",
        params![...],
    )?;   // 真实数据库错误（连接失败、SQL错误等）在这里就通过 `?` 正常传播为 NovaError::Database，不受本节改动影响
    if rows_affected == 0 {
        return Err(NovaError::StaleActionAttempt(format!(
            "action {idempotency_key} 的 attempt 已不是预期的 {expected_attempt}，视为陈旧完成"
        )));   // 只有这一种情况才返回这个专门的错误类型
    }
    self.find_action(task_id, idempotency_key)?.ok_or_else(|| NovaError::Database("动作记录在更新后丢失".to_string()))
}
```

### 2.3 调用方的明确匹配伪代码（dev/test/build统一模式，回应"不要只说明开发CAS成功后的后续流程"）

```rust
// dev（run_dev_and_test_round）：
match finish_gated_action(db, task_id, &dev_key, succeeded, &result, captured_attempt) {
    Ok(finished) => { /* 正常后续：自检解析、推进到test/build */ }
    Err(NovaError::StaleActionAttempt(_)) => return Ok(DevTestOutcome::Stale),
    Err(other) => return Err(other),   // 数据库/IO/配置错误原样上抛，不转成Stale
}

// test（run_tests_for_round）：
match db.finish_action(task_id, &test_key, finish_status, evidence_ref, error_summary, test_action_attempt) {
    Ok(_) => { db.record_test_evidence(...)?; /* 原有后续逻辑 */ }
    Err(NovaError::StaleActionAttempt(_)) => return Ok(TestRoundOutcome::Stale),
    Err(other) => return Err(other),
}

// build（run_build_for_round）：
match db.finish_action(task_id, &build_key, finish_status, evidence_ref, error_summary, build_action_attempt) {
    Ok(_) => { db.record_build_evidence(...)?; /* 原有后续逻辑 */ }
    Err(NovaError::StaleActionAttempt(_)) => return Ok(BuildRoundOutcome::Stale),
    Err(other) => return Err(other),
}
```

**明确禁止的写法（直接回应审核意见）：** 不得写`Err(_) => return Ok(DevTestOutcome::Stale)`这种吞掉一切错误类型的模式——必须显式匹配`NovaError::StaleActionAttempt`这一个变体，其余所有`NovaError`变体（`Config`/`Credential`/`Database`/`Network`/`Api`）都必须用`Err(other) => return Err(other)`原样上抛，交给上层既有的错误处理（最终体现为任务失败/错误提示），不得被静默吞掉或误判为"正常的陈旧执行"。

---

## 3. `generate_requirement_snapshot`排除Draft（回应原M2未关闭问题）

### 3.1 代码依据（本轮核验，确认审核意见引用属实）

- `apps/desktop/src/stores/coreLoop.ts:460-478`：`confirmRequirement()`连续调用两次`transition_task`——先`toState:'awaiting_confirmation'`，再`toState:'confirmed'`。这证实"转入`AwaitingConfirmation`"确实发生在用户点击确认**之后**，不是确认前的纯展示态；但这不代表后端应该把`Draft`也列入允许状态——`Draft`本身是"用户还没点确认"的状态，与`AwaitingConfirmation`（已经点了确认、正在走转换）是两个不同阶段。
- `storage/tasks.rs:31-36`（`TaskState`枚举，结合`LEGAL_TRANSITIONS`完整列表）：真实状态集合包含`Draft/AwaitingConfirmation/Confirmed/Queued/Running/Verifying/SafeFixing/PreviewReady/AwaitingUserAcceptance/NeedsUserAction/Interrupted/Completed/Failed/Cancelled`。

### 3.2 决定：后端允许状态显式排除Draft，不依赖"前端先展示"的产品流程假设

**修正上一版契约"任务必须处于非终态"（范围过宽，包含了Draft）为：**

```rust
const SNAPSHOT_ALLOWED_STATES: &[TaskState] = &[
    TaskState::AwaitingConfirmation,
    TaskState::Confirmed,
    TaskState::Queued,
    TaskState::Running,
    TaskState::Verifying,
    TaskState::SafeFixing,
    TaskState::PreviewReady,
    TaskState::AwaitingUserAcceptance,
    TaskState::NeedsUserAction,
    TaskState::Interrupted,
];
// 显式排除：Draft（未确认）、Completed/Failed/Cancelled（终态）

pub fn generate_requirement_snapshot(db: &Database, project_id: &str, task_id: &str)
    -> NovaResult<Vec<ConfirmedRequirementItem>> {
    let task = db.task(task_id)?;
    if task.project_id != project_id {
        return Err(NovaError::Config("task_id 与 project_id 不匹配，已拒绝".to_string()));
    }
    if !SNAPSHOT_ALLOWED_STATES.contains(&task.state) {
        return Err(NovaError::Config(format!(
            "当前任务状态（{}）不允许生成需求快照：Draft 任务尚未确认需求，终态任务不需要补建快照",
            task.state
        )));
    }
    if let Some(existing) = /* 已有快照 */ {
        return Ok(existing);   // 幂等返回——但这个分支本身也必须先过上面的状态校验，不能绕过
    }
    // ...拆分并写入...
}
```

**"已有快照的幂等返回也不能绕过状态校验"（直接回应审核意见）：** 状态校验在函数**最开头**执行，幂等检查（"已有则直接返回"）是**状态校验通过之后**的下一步——不存在"因为已经有快照了，所以不用检查状态"这种短路路径。即便某个`Draft`任务由于未来的bug已经意外产生了快照行（理论上不应发生，因为写入点只在`AwaitingConfirmation`转换分支里），本函数仍然会在状态校验这一步就拒绝对`Draft`任务的调用，不会读到那条意外产生的快照行就当作"幂等成功"直接返回——**拒绝发生在读快照之前**。

### 3.3 Draft/AwaitingConfirmation/Confirmed的真实语义边界（回应"明确真实语义与可调用边界"）

| 状态 | 含义 | `generate_requirement_snapshot`是否可调用 |
|---|---|---|
| `Draft` | 用户正在描述需求，尚未点击"确认"按钮；`goal`等字段可能还在被前端表单编辑中（虽然后端一旦`create_task`写入即不可变，但产品语义上这个阶段"需求"还没有被用户正式确认为最终版本） | **不可调用**，直接拒绝 |
| `AwaitingConfirmation` | 用户已经点击确认，`transition_task`正在/已经把任务转入这个状态；这个转换本身会在同一事务内触发`confirmed_requirement_items`的自动快照写入（`10141417.md`第2.3节既有机制） | 可调用（幂等场景：若自动快照因某种原因缺失，可在此状态下补建） |
| `Confirmed` | 已经过`AwaitingConfirmation→Confirmed`（见`coreLoop.ts`的第二次`transition_task`调用），需求确认流程完全走完 | 可调用 |

### 3.4 验收用例（回应"增加Draft、AwaitingConfirmation、Confirmed、终态、重复调用的验收用例"）

| # | 场景 | 预期结果 |
|---|---|---|
| 1 | 对`Draft`任务直接调用`generate_requirement_snapshot` | 拒绝，不写任何item，错误信息说明"Draft任务尚未确认需求" |
| 2 | 对`AwaitingConfirmation`任务调用（正常的自动快照已存在） | 幂等返回现有行 |
| 3 | 对`AwaitingConfirmation`任务调用（自动快照因故缺失） | 补建快照，按2.2节拆分规则生成 |
| 4 | 对`Confirmed`任务调用 | 正常生成/幂等返回 |
| 5 | 对`Completed`/`Failed`/`Cancelled`任务调用 | 拒绝，终态不需要补建快照 |
| 6 | 重复调用同一非终态、非Draft任务 | 幂等返回同一结果，不重复插入、不报错 |
| 7 | 对冻结（`legacy_classification`已标记）的旧非终态任务调用 | 仍允许（`10141417.md`第2.6节已明确本命令不受冻结限制），但状态校验（本节新增）独立生效——若该冻结任务恰好处于`Draft`，同样拒绝，两条规则互不替代 |

**保持已有需求展示和明确确认流程、生成快照不解除legacy冻结：** 与`10141417.md`第1.4/2.6/5.6节一致，本节不改变这些结论。

---

## 4. 文字修正（回应"文字与权威关系"的2处）

### 4.1 `run_startup_recovery`的准确表述

**代码依据：** `storage/recovery.rs:73-75`（`mark_started_actions_uncertain()`）、`:199-208`（`interrupt_all_in_flight_tasks()`调用`transition_task(..., Interrupted, ...)`，并写入恢复checkpoint）——这些都是**真实的写操作**，不是纯读。

**修正：** 此前（`10141417.md`第5.2/5.3节）把`run_startup_recovery`描述为"纯只读/记账性质"不准确。**准确表述为："恢复记账写入——会把未完成的action标记为`uncertain`、把在途任务转入`Interrupted`并记录恢复checkpoint，但不会重新调用AI、不会重跑任何命令、不会推进任务进入下一个执行阶段"。** 这个表述上的修正不改变`10141417.md`第5节冻结检查的结论本身（恢复扫描确实不需要在这几个写操作上加`legacy_classification`检查，因为它们是"记账"而不是"继续执行"——这个结论维持，只是措辞从"只读"改为"恢复记账写入"，更准确）。

### 4.2 权威文件关系已在第0节给出，此处不重复。

---

## 5. 本文件状态声明

本文件是**补充规格**，第1-3节涉及的`NovaError::StaleActionAttempt`新变体、`finish_action`签名变化、`run_tests_for_round`/`run_build_for_round`写入顺序调整、`generate_requirement_snapshot`状态校验**均未实现**。第0节列出的既有权威文件（`10141417.md`、`10141420.md`、及其引用的`10141347.md`对应章节）继续有效，未被本文件推翻。需经 Codex 独立复审通过后，才能转入编码工作包。
