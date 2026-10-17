# WP2A：结构化自检契约与完工门禁规格说明（第四版，回应 CODEX_WP2A_SPEC_REREVIEW10141259.md）

**与上一版关系：** 本文件是 `docs/release-v0.1.0/NOVA_WP2A_SELF_CHECK_SPEC10141243.md` 的修订版，原文件保留。本版集中关闭 `D:\nova\docs\reviews\CODEX_WP2A_SPEC_REREVIEW10141259.md` 的3个Blocker（B1–B3）、4个Major（M1–M4）及相关Minor。
**状态：** 规格设计稿，**未实现任何代码**。方案修订不等于实现或运行验收通过。
**核心架构变化（一句话）：** 新增统一"结果归属"字段 `result_attempt`，在开发/修复动作成功的那一刻捕获，原样向后传递给自检、测试证据、构建证据、无测试/无构建确认四类记录；完工门禁不再是独立的"先查后写"函数，而是直接作为新增条件内联进 `Database::transition_task` 本已存在、本已唯一的那条原子 `UPDATE` 语句；门禁失败时不转换状态，任务保持 `AwaitingUserAcceptance`，只返回结构化拒绝原因。

---

## 0. 本版关闭对照表

| 编号 | 问题 | 本版位置 |
|---|---|---|
| B1 | 当前attempt自检可能配旧attempt的test/build证据 | 第2节：新增`result_attempt`统一归属字段 |
| B2 | 历史分类规则自相矛盾 | 第5节：重新定义为不重叠三分类+独立`integrity_warning` |
| B3 | 门禁失败的状态/事务语义冲突 | 第3节：固定为"拒绝转换，保持AwaitingUserAcceptance" |
| M1 | 权威入口未明确到`Database::transition_task` | 第3节：门禁内联进该函数既有的原子UPDATE |
| M2 | 旧未完成任务无需求快照 | 第1.4节：新增显式"补建快照"命令，不静默生成 |
| M3 | `self_checks`表结构不完整 | 第2.3节：完整DDL |
| M4/m1/m2 | 占位引用、形式覆盖边界、WP2A-0/主体关系 | 第6节 |

---

## 1. 需求覆盖快照（WP2A-0，补充M2）

### 1.1 新表（与上一版一致）

```sql
CREATE TABLE confirmed_requirement_items (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  field TEXT NOT NULL CHECK(field IN ('goal','in_scope','not_in_scope','done_criteria')),
  item_index INTEGER NOT NULL,
  item_text_redacted TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(task_id, field, item_index)
);
CREATE INDEX idx_confirmed_requirement_items_task ON confirmed_requirement_items(task_id);
```

### 1.2 新任务的写入时机（与上一版一致）

`transition_task`的`to == AwaitingConfirmation`分支内，解析当前`goal/in_scope/not_in_scope/done_criteria`按换行生成条目并写入，仅当该`task_id`在表中尚无任何行时才写入（避免重复）。

### 1.3 解析规则的边界声明（回应m1）

**按换行拆分只是v0.1.0的结构化最小机制，不是语义完整性证明，也不是独立审核。** 自检里某一项标`covered`，仍然只是开发AI自己的陈述，必须继续受真实证据（第2节）和用户确认（第3节）约束——`requirement_coverage`门禁只保证"形式上没有漏项、没有伪造项、没有停留在partial"，不保证AI的判断在语义上是对的。

### 1.4 旧未完成任务无快照的处理（回应M2，新增）

**问题：** `confirmed_requirement_items`表随WP2A-0的迁移一起新建，**不回填历史数据**。迁移落地前已经处于`Confirmed/Queued/Running/Verifying/SafeFixing/PreviewReady/AwaitingUserAcceptance/NeedsUserAction`等非终态的任务，在表里没有任何行。

**决定（不静默生成，不要求用户重新描述）：**

1. 完工门禁（第3节）查到某任务在`confirmed_requirement_items`里零行时，**不**把"零行"解释成"权威集合为空、直接放行"，也**不**自动用当前的`goal/in_scope/not_in_scope/done_criteria`现场生成一份"已确认"快照——两者都违反"不得静默生成已确认快照"的要求。统一拒绝完工，原因标记为`RequirementSnapshotMissing`，与"任务已填了需求但覆盖不全"（`RequirementCoverageIncomplete`）是两个不同的拒绝原因，不能混用。
2. **新增一个显式命令**（这是本节"至少列出真实改动"的要求）：`generate_requirement_snapshot(task_id)`——只读当前任务**已经确认过、未被修改过**的`goal/in_scope/not_in_scope/done_criteria`（这四个字段创建后无法编辑，见上一版第1.3节核验结论，内容保证没变），对其执行与WP2A-0相同的换行拆分逻辑并写入快照表；仅当该任务**非终态**且表中**尚无**该`task_id`的行时才允许调用，否则拒绝（防止重复写入或对已完成任务倒填数据）。
3. **界面层面：** 当完工因`RequirementSnapshotMissing`被拒绝时，前端展示的不是"请重新描述你的需求"，而是把该任务已有的`goal/in_scope/not_in_scope/done_criteria`原文**只读展示**给用户看一遍，附一个"确认沿用，生成需求清单"按钮，点击后调用上面的新命令——用户不需要重新打字，只需要确认一次。
4. **已完成的历史任务：** 永远不触发这条路径（完工门禁只在`AwaitingUserAcceptance → Completed`这一条边上求值，已经是`Completed`的任务不会再被评估），不回填、不改写历史完工结论。

**验收用例：** 见第7节#13。

---

## 2. 统一执行归属：`result_attempt`（回应B1，核心新增机制）

### 2.1 问题重述与代码依据

**代码依据（本轮逐一核验）：**
- `self_checks`按`dev_action_attempt_number`过滤（上一版方案）。
- `test_evidence`/`build_evidence`的实际列（`storage/evidence.rs:30-44,59-70,321-333,473-485`核验确认）里**没有**任何attempt/dev归属字段，只有`task_id/result_generation/round`。
- `test_action_key`/`build_action_key`（`core_loop.rs:2040,2368`）本身只按generation+round构造，不含dev attempt。
- `adopted_passing_generation_round()`（`core_loop/documents.rs:237-288`）只按generation+round查evidence，对evidence背后对应哪一次dev执行完全不知情。

**真实可构造的反例（Codex指出，本轮确认成立）：** 同一task、同一generation、同一round内，如果开发action因崩溃恢复等原因被真实重试（`attempt_number`递增），当前attempt产出的自检有效，但`test_evidence`/`build_evidence`仍是**重试前那次**旧attempt遗留的`passed`记录（因为`ActionGate::AlreadySucceeded`分支会直接复用旧证据，不重新跑测试/构建）——按上一版方案，self_checks命中当前attempt、evidence命中同generation/round，两者各自"看起来都对"，但其实不是同一次执行产出的结果，用户确认后可能错误进入`completed`。

### 2.2 决定：`result_attempt`——在发起执行时捕获，向后传递，不在事后重新贴标签

**核心原则（直接回应"不能在旧输出返回后读取当前attempt给它重新贴标签"）：** `result_attempt`的值**只在`run_dev_and_test_round`开头、确认本轮开发action已成功（无论是刚跑完成功，还是`AlreadySucceeded`复用）的那一刻读取一次**，赋给一个局部变量，之后作为**显式参数**依次传入`run_tests_for_round(..., result_attempt)`、`run_build_for_round(..., result_attempt)`，这两个函数把它原样写入自己产生的`test_evidence`/`build_evidence`行——**不允许**测试/构建函数自己在写证据时重新去查一次`task_actions`的"当前"`attempt_number`（那样就是"事后重新贴标签"，如果中间发生了重试，查到的会是新值，而证据本身对应的其实是旧值）。

**函数签名变化（最小改动，不新增并行的ID体系）：**

```rust
fn run_dev_and_test_round(..., round: i64, ...) -> NovaResult<DevTestOutcome> {
    // 现有dev action门禁逻辑不变（第1554-1616行）
    let result_attempt = db.find_action(task_id, &dev_key)?
        .ok_or(...)?.attempt_number;   // 开发action成功后，立即读取一次，不再变化

    // 【本规格新增步骤】自检解析：写入 self_checks，round、result_attempt 均为此刻捕获的值

    run_tests_for_round(db, project_id, task_id, project_dir, round, result_attempt, cancel)?
    run_build_for_round(db, project_id, task_id, project_dir, round, result_attempt, cancel)?
}
```

### 2.3 Schema改动——完整DDL（回应M3，不再只给查询片段）

```sql
-- 新表，WP2A-0之后、WP2A主体迁移新增
CREATE TABLE self_checks (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  round INTEGER NOT NULL,
  result_attempt INTEGER NOT NULL,          -- 捕获自对应 dev/fix action 的 attempt_number，不可为空
  parse_status TEXT NOT NULL CHECK(parse_status IN ('valid','invalid_format')),
  self_reported_status TEXT CHECK(
    self_reported_status IS NULL
    OR self_reported_status IN ('self_reported_complete','self_reported_incomplete','self_reported_blocked')
  ),                                          -- parse_status='invalid_format'时允许为NULL（解析失败，没有这个字段可读）
  requirement_coverage_json TEXT,             -- parse_status='valid'时应非NULL；由应用层校验JSON结构，不在DDL层约束
  check_results_json TEXT,
  unfinished_items_json TEXT,                 -- parse_status='valid'时应为合法JSON数组（可空数组），应用层校验
  blocking_issues_json TEXT,                  -- 同上
  raw_block_redacted TEXT NOT NULL,           -- 原始区块文本，已脱敏，技术详情展示用，任何parse_status都必须有
  created_at TEXT NOT NULL
);
CREATE INDEX idx_self_checks_task_round_attempt ON self_checks(task_id, round, result_attempt);
-- 不设 UNIQUE：同一(task_id,round,result_attempt)允许追加多行（同attempt重复解析场景），
-- "当前判定"由下方查询函数的 ORDER BY rowid DESC LIMIT 1 决定，不依赖唯一约束。

-- 对既有表的新增列（新迁移版本，仿照既有 test_evidence_v6/build_evidence_v10 的重建模式）
CREATE TABLE test_evidence_v7 (
  -- 原 test_evidence_v6 全部列保持不变，新增一列：
  ... ,
  result_attempt INTEGER          -- 可为NULL：历史行（迁移前产生）backfill为NULL，表示"该行产生于本机制生效之前，不参与归属比对"
);
-- INSERT INTO test_evidence_v7 (..., result_attempt) SELECT ..., NULL FROM test_evidence;
-- DROP TABLE test_evidence; ALTER TABLE test_evidence_v7 RENAME TO test_evidence;

CREATE TABLE build_evidence_v11 (
  -- 原 build_evidence_v10 全部列保持不变，新增一列：
  ... ,
  result_attempt INTEGER          -- 语义同上，历史行NULL
);

CREATE TABLE no_tests_confirmations_v2 (
  -- 原 no_tests_confirmations 全部列保持不变，新增一列：
  ... ,
  result_attempt INTEGER          -- 历史行NULL；新确认写入时必须带上当前result_attempt
);
-- 注意：原有 UNIQUE(task_id, result_generation, round) 不变，不升级为包含result_attempt，
-- 理由见2.4节"旧确认授权失效"说明。

CREATE TABLE no_build_confirmations_v2 (
  -- 同上模式，新增 result_attempt 列
);
```

**"当前判定"权威查询（self_checks，维持上一版rowid排序结论，本版补全完整上下文）：**

```sql
SELECT * FROM self_checks
  WHERE task_id = ?1 AND round = ?2 AND result_attempt = ?3  -- ?3 现场重新读取 task_actions 当前 attempt_number
  ORDER BY rowid DESC LIMIT 1
```

### 2.4 `AlreadySucceeded`缓存复用逻辑的改动（回应"仅增加evidence字段不够"）

**改动位置：** `run_tests_for_round`/`run_build_for_round`的`ActionGate::AlreadySucceeded`分支（`core_loop.rs:2077-2095`附近，`run_build_for_round`同构）。

**改动内容：** 原逻辑是"查到任意一条该(generation,round)的evidence就直接复用其status"。新逻辑追加一次比对：**取到的`existing`证据行的`result_attempt`必须等于本次调用传入的`result_attempt`参数**；不相等时，**不得复用**，走向与"查无证据"完全相同的既有fail-closed分支（写入`insufficient_evidence`，原因文案改为"测试/构建证据来自更早的一次开发执行，当前执行尚未产生对应证据"，`transition_task(..., NeedsUserAction)`——这条边已经合法存在，是现有机制，不是新增边）。**不自动重新运行测试/构建**（不新增AI调用或命令执行，只是如实阻断，与PM"无法安全恢复时如实阻断,不虚构自动恢复能力"的要求一致）。

### 2.5 无测试/无构建授权的归属防护（回应"防止旧结果授权被错误用于新结果"）

**改动位置：** `no_tests_applicable_is_authorized`/`no_build_applicable_is_authorized`（`core_loop/documents.rs:304-359`）。

**改动内容：** 两个函数新增`result_attempt: i64`参数（由调用方现场传入，与上面同理）。`find_no_tests_confirmation`/`find_no_build_confirmation`查到的确认行，必须额外核对其`result_attempt`列与传入值一致才算有效授权；`no_build_applicable_is_authorized`的第二条路径（`authorization_source == "mechanical_not_applicable"`，直接记在evidence行上）天然已经靠2.3节给`build_evidence`新增的`result_attempt`列解决——因为这条路径本身就是读evidence行的字段，不是单独的确认表，前面2.4节的比对已经覆盖了它。

**不升级confirmations表的UNIQUE约束的原因：** `no_tests_confirmations`/`no_build_confirmations`的`UNIQUE(task_id, result_generation, round)`保持不变（不加`result_attempt`到唯一约束里），因为同一个(generation,round)在不同attempt下**理应**各自需要一次独立确认——如果把`result_attempt`也塞进UNIQUE约束，会允许同一(generation,round)下**多个不同attempt各自有一条"有效"确认行同时存在**，这反而削弱了约束的意义；维持原唯一约束（新attempt要确认，必须先感知到旧确认已经不适用），配合应用层的`result_attempt`匹配比对，效果等价于"旧确认不适用于新attempt"，不需要放宽唯一约束。

---

## 3. 完工门禁的权威落点与失败时的状态语义（回应B3、M1，合并处理因为两者是同一个机制的两面）

### 3.1 代码依据

- `commands.rs:536-553`的`transition_task`是通用Tauri命令，暴露给前端，可传任意合法目标状态——这是M1指出的潜在旁路。
- `core_loop.rs:562-583`的`accept_task_result`只是"先写user_acceptance，再调用`transition_task(..., Completed)`"的组合，本身不是权威门禁点。
- `storage/tasks.rs:316-332`的`Database::transition_task`在`to=='completed'`时已经把user_acceptance检查写进同一条`UPDATE...WHERE`语句的`EXISTS`子句——这**已经是**全仓库唯一一处写`tasks.state`为`completed`的SQL（本轮重新核验：`grep "SET state"`全仓库只有这一处非测试代码命中）。

### 3.2 决定：新条件直接内联进这同一条SQL，不新建平行的门禁函数

**修正上一版"`completion_gate_for_current_result()`在同一事务内完成判定"的提法**——这个说法容易让人以为还有一个独立于`transition_task`的函数在"也做一次检查"，而通用`transition_task`命令仍可能走原来的老路径绕过去。本版明确：**没有第二个权威入口**。`storage/tasks.rs`的`transition_task`函数本体，在`to == TaskState::Completed`时，`WHERE`子句的`EXISTS`部分从"只核对user_acceptance"扩展为同时核对：

```sql
UPDATE tasks
SET state = ?1, updated_at = ?2
WHERE id = ?3
  AND state = ?4
  AND (
    ?1 <> 'completed'
    OR (
      result_generation > 0
      AND EXISTS (SELECT 1 FROM user_acceptance ua
                  WHERE ua.task_id = tasks.id AND ua.result_generation = tasks.result_generation AND ua.accepted = 1)
      AND EXISTS (SELECT 1 FROM confirmed_requirement_items cri WHERE cri.task_id = tasks.id)          -- 需求快照存在
      AND NOT EXISTS (                                                                                  -- coverage必须完整覆盖，用NOT EXISTS表达"没有缺漏"
        SELECT 1 FROM confirmed_requirement_items cri
        WHERE cri.task_id = tasks.id
          AND NOT EXISTS (
            -- 此处的"当前有效self_checks判定行"需要先在应用层算出 current_round/current_result_attempt，
            -- 再作为参数绑定进来；SQLite 的 EXISTS 子查询无法独立完成"取rowid最新一行"的语义，
            -- 因此实现时由 Rust 侧先执行一次只读查询取到当前判定行的 requirement_coverage_json，
            -- 在应用层反序列化后比对完整性，而不是整段写成一条自包含SQL——这是本版对上一版
            -- "必须塞进一条UPDATE"要求的澄清：权威入口是这同一条UPDATE语句，但它的参数可以是
            -- 应用层在同一个BEGIN IMMEDIATE事务内、刚刚读出来的值，不代表SQL本身必须是自给自足的
            -- 一整段可以不依赖任何外部计算的语句。
          )
      )
      AND ?5 = 'self_check_valid_and_complete'   -- 应用层预计算好的布尔结果，作为参数绑定进来
    )
  )
```

**说明（避免过度形式化，回应PM"不要求为了形式强行把全部逻辑塞进单条复杂SQL"）：** 真正不可绕过的保证来自**事务边界**，不是"SQL语句本身有多复杂"。具体实现：在`transition_task`已有的`BEGIN IMMEDIATE`事务内（`storage/tasks.rs:298`），当`to==Completed`时，先执行几条只读`SELECT`（查当前`self_checks`判定行、查`confirmed_requirement_items`、核对两者的coverage完整性、核对`result_attempt`与当前`task_actions`一致），把结果归约成一个布尔值，再执行上面这条`UPDATE`并把该布尔值和其余已有参数一起绑定进去——**全程不释放事务**，不存在"判定"和"写入"之间可以被其他连接插入的窗口（`BEGIN IMMEDIATE`从事务开始就持有写锁）。这与M3上一版的"单SQL自给自足"要求不同，是PM本轮明确放宽后的版本，但"只有一个权威函数、不可被旁路"的核心保证不变——因为通用`transition_task`命令和`accept_task_result`最终都会调用**这同一个**`Database::transition_task`函数，没有第二条路径能写`completed`。

### 3.3 失败时的状态语义——固定选择（回应B3）

**选定方案：** 完工门禁的任一条件不满足时，`transition_task`返回现有的`Err(NovaError::Config("无法进入completed：..."))`（扩展现有的错误分支，`storage/tasks.rs:346-350`已有的"进入completed缺少匹配用户确认记录"分支，改为依据具体缺失条件返回对应的结构化原因），**任务状态不发生任何转换，保持`AwaitingUserAcceptance`原地不动**。

**明确禁止的表述（纠正上一版矛盾）：** 不写入`NeedsUserAction`，不记录声称"已转入NeedsUserAction"的`task_state_events`行，不新增`(AwaitingUserAcceptance, NeedsUserAction)`之外的转换边使用场景（该边继续只由`accept_task_result(accepted=false)`这一既有、语义完全不同的"用户明确拒绝"场景使用，与本门禁无关）。

**用户仍可查看效果：** 因为任务停留在`AwaitingUserAcceptance`（这个状态本身就是"证据已通过、可以打开产品查看"的状态，`PreviewReady → AwaitingUserAcceptance`这条边不受本门禁影响），用户随时可以继续查看已经生成的真实效果，不受"确认被拒绝"这件事影响。

**各失败原因的"恢复"说明（回应"不默认新增AI调用,不虚构自动恢复能力"）：**

| 拒绝原因 | 恢复方式 | 是否新增AI调用 |
|---|---|---|
| `RequirementSnapshotMissing`（第1.4节） | 用户点"确认沿用，生成需求清单"，调用`generate_requirement_snapshot` | 否 |
| `SelfCheckMissing`（自检记录缺失） | 正常情况下不会出现在`AwaitingUserAcceptance`（见3.4节解释）；真出现时，这是数据异常，如实阻断，不提供自动恢复按钮，只展示技术详情供排查 | 否（没有自动恢复路径） |
| `SelfCheckInvalidFormat` | 同上，如实阻断 | 否 |
| `SelfCheckNotComplete`/`UnfinishedItemsPresent`/`BlockingIssuesPresent` | 用户点"不通过"（既有`accept_task_result(accepted=false)`路径），走向既有的`NeedsUserAction → SafeFixing`修复流程，受既有一次修复上限约束 | 否（复用既有修复机制，不新增） |
| `RequirementCoverageIncomplete` | 同上，走"不通过"→修复 | 否 |
| `UserAcceptanceMissing` | 用户还没点确认/不通过，这不是"失败"，只是尚未操作，界面照常显示"等待你的确认" | 否 |

### 3.4 为什么`SelfCheckMissing`等"数据异常"类原因在正常路径下不会出现

因为`result_attempt`机制（第2节）保证：自检写入和test/build证据写入使用同一个在round开头捕获的值，二者必然同步；只要走到`AwaitingUserAcceptance`，说明`adopted_passing_generation_round()`已经找到了通过的证据，而该证据所在round的自检也必然已经在同一次`run_dev_and_test_round`调用里写入——`SelfCheckMissing`只可能在"进程崩溃发生在自检写入和证据写入之间"这种极端恢复场景下出现，属于防御性兜底，不是正常业务分支。

---

## 4. `preview_ready`与`completed`的分离（维持上一版，未变）

与 `NOVA_WP2A_SELF_CHECK_SPEC10141243.md` 第3.1/3.3节一致：`adopted_passing_generation_round()`不改动，不受本规格任何新增条件影响；本门禁只挂在`AwaitingUserAcceptance → Completed`这一条边。

---

## 5. 历史双AI任务分类——不重叠三分类（回应B2）

### 5.1 问题根源（本轮重新审视上一版矛盾）

上一版同时写了"三项证据任一命中即双AI"和"某些命中组合要归unknown"，这两条规则在覆盖同一输入时会给出两个不同答案，是真矛盾。根源在于把"文档内容"这种**不可独立核验归属的弱信号**，和"数据库行"这种**只能由代码执行产生、不可被用户直接编辑的强信号**，放在同一个OR逻辑里平等对待。

### 5.2 决定：两类强信号做OR，弱信号只做佐证/警示，不参与分类决策

**强信号（可核验且归属明确，只有这两个参与OR分类判定）：**
- `task_actions`存在`action_type='ai_review'`的行（任意status，核验依据：`core_loop.rs:1697-1698`，只有双AI路径会写这个action_type）；
- `task_reviews`表存在该任务的行。

**弱信号（不单独采信，仅用于`integrity_warning`标注）：**
- `.nova/tasks/<task-id>/FIRST_REVIEW.md`/`SECOND_REVIEW.md`磁盘文件存在非模板占位内容——这是普通文件，没有任何机制阻止用户手动编辑或放置，**不满足"来源真实性可核实"的要求**，不能单独作为分类依据。

**最终算法（三分类，互斥，覆盖B2要求的全部7种组合）：**

```
若 (ai_review action存在) OR (task_reviews行存在)：
    → dual_ai_evidenced
    若同时：文档内容与两个强信号的状态不一致（例如只有其中一个强信号命中，
            或文档内容明显缺失/与预期round数不符）：
        附加 integrity_warning，具体描述不一致之处；分类本身不变，仍是dual_ai_evidenced
否则若 (ai_development Succeeded存在) AND (两个强信号都不存在) AND (任务已到达终态或awaiting_user_acceptance)：
    → single_ai_evidenced
否则：
    → unknown_insufficient，并具体区分原因：
        - "仅文档命中，无数据库强信号支撑，来源真实性无法单独核实"
        - "历史schema版本证据源不存在"
        - "开发都没有成功记录，数据不足"
```

### 5.3 PM要求的7种组合逐一判定（回应B2通过条件）

| 组合 | 分类 | 说明 |
|---|---|---|
| 仅有`ai_review action` | `dual_ai_evidenced` | +integrity_warning（task_reviews/文档缺失，建议标注但不强制） |
| 仅有`task_reviews` | `dual_ai_evidenced` | +integrity_warning（ai_review action/文档缺失） |
| 仅有实质审核文档（无ai_review action、无task_reviews） | `unknown_insufficient` | 原因："仅文档命中，无数据库强信号支撑" |
| `ai_review action`有，`task_reviews`无 | `dual_ai_evidenced` | +integrity_warning（审核执行有记录但复审结论未落库，可能中途中断） |
| 审核文档有，`task_reviews`无（`ai_review action`也无，等同上面第3行） | `unknown_insufficient` | 同第3行；若`ai_review action`存在则落入第4行 |
| 三者全无但开发成功 | `single_ai_evidenced`（需任务已到达终态或待验收） | 若开发都没成功，连这个也不成立，落入`unknown_insufficient`（"数据不足"） |
| 历史schema版本证据源不存在 | `unknown_insufficient` | 原因："历史schema版本证据不全" |

### 5.4 冻结与保留规则（维持不变）

`unknown_insufficient`与`dual_ai_evidenced`的非终态任务同等冻结，不静默转换、不自动继续、不自动完成。已完成历史任务（任意分类）只读保留，不改写结论。`integrity_warning`只是报告里的附加标注，不改变冻结规则本身是否触发——触发条件只看最终分类是不是`dual_ai_evidenced`或`unknown_insufficient`且非终态。

---

## 6. 文档准确性修正（回应M4/m1/m2）

- 上一版第144行"`docs/release-v0.1.0/NOVA_SINGLE_AI_MIGRATION_PLAN<本轮时间戳>.md`"占位引用，更正为实际路径：`docs/release-v0.1.0/NOVA_SINGLE_AI_MIGRATION_PLAN10141246.md`（该文件的第5节历史分类算法已被本文件第5节取代，以本文件为准）。
- m1已在第1.3节正面说明。
- m2（WP2A-0与WP2A主体的提交/复审顺序）：不强制要求分两次提交或分两次复审，但WP2A主体（第2–3节的门禁逻辑）在编码时必须确认WP2A-0的迁移已经先行存在（无论是同一个PR内先后提交，还是分开的PR），因为门禁逻辑直接查询`confirmed_requirement_items`表。

---

## 7. 验收矩阵（全表重写，覆盖本轮新增场景）

| # | 场景 | 前置 | 预期结果 | 拒绝原因 |
|---|---|---|---|---|
| 1 | 有效完成（happy path） | self_checks/coverage/evidence的result_attempt全部一致，用户已确认 | `Completed` | — |
| 2 | 当前attempt自检有效，test/build证据来自旧attempt（B1核心反例） | 开发真实重试过，`AlreadySucceeded`分支检测到`result_attempt`不匹配 | 更早已在`Verifying`阶段被拦截为`insufficient_evidence`+`NeedsUserAction`，根本不会走到`AwaitingUserAcceptance` | 测试/构建阶段：证据来自更早的开发执行 |
| 3 | self_reported_incomplete/blocked+空数组 | — | 拒绝转入`Completed`，停留`AwaitingUserAcceptance` | `SelfCheckNotComplete` |
| 4 | complete但unfinished/blocking非空 | — | 同上 | `UnfinishedItemsPresent`/`BlockingIssuesPresent` |
| 5 | coverage漏项/重复/伪造/partial/not_covered | — | 同上 | `RequirementCoverageIncomplete` |
| 6 | 空壳/格式错误 | — | 同上 | `SelfCheckInvalidFormat` |
| 7 | 旧attempt自检（真实重试后attempt已变） | — | 同上（正常路径下不会单独出现，见3.4节） | `SelfCheckMissing` |
| 8 | 缺少用户确认 | self_checks/evidence均有效 | 停留`AwaitingUserAcceptance`，等待用户操作，不算"拒绝" | `UserAcceptanceMissing` |
| 9 | 用户确认绕过（直接调用通用`transition_task`） | — | 同样被`Database::transition_task`唯一权威入口拒绝 | 视具体缺失条件 |
| 10 | 直接状态转换绕过 | 静态核验 | 全仓库对`tasks.state`写`completed`只有`storage/tasks.rs`一处（本轮已核验） | — |
| 11 | 并发变化（判定后、写入前self_checks被覆盖） | `BEGIN IMMEDIATE`事务内完成判定+写入 | 并发连接等待锁，不存在交错窗口 | — |
| 12 | 旧未完成任务无需求快照 | 迁移前已非终态，`confirmed_requirement_items`零行 | 拒绝转入`Completed`，提示"确认沿用，生成需求清单" | `RequirementSnapshotMissing` |
| 13 | 历史已完成任务不受影响 | 任务已是`Completed` | 不重新评估，不回填快照 | — |
| 14 | 真实证据门禁回归 | self_checks有效但测试/构建未通过 | 停在既有失败/修复状态，不会先到`AwaitingUserAcceptance` | 不适用（门禁尚未求值） |
| 15 | 一次修复上限回归 | — | round上限仍为2，`MAX_AUTO_FIX_CYCLES`现状不变 | — |
| 16 | 无测试/无构建授权正常场景回归 | 真实`no_tests_confirmations`/`mechanical_not_applicable`行，`result_attempt`匹配 | 正常授权通过 | — |
| 17 | 无测试/无构建授权被旧attempt污染 | 确认行的`result_attempt`不是当前值 | 授权失效，视为未授权 | 回到既有"需要用户确认"流程 |
| 18 | 可预览不可完工 | 无自检但证据通过 | `preview_ready`达成，`completed`被拒 | `SelfCheckMissing`（防御性，不应在正常路径出现） |

---

## 8. 本文件状态声明

本文件是**规格设计稿**，`confirmed_requirement_items`、`self_checks`、新增迁移列、`Database::transition_task`的门禁扩展、`generate_requirement_snapshot`命令、全部测试**均未实现**。需经 Codex 独立复审通过后，才能转入编码工作包（WP2A-0 → WP2A主体）。
