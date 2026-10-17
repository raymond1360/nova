# WP2A：结构化自检契约与完工门禁规格说明（第五版，权威文件）

**权威声明（回应M2）：** 本文件与配套的 `docs/release-v0.1.0/NOVA_SINGLE_AI_MIGRATION_PLAN10141350.md` 是本轮**唯一权威方案**，自洽、自包含，不要求实现者回去拼凑 `NOVA_WP2A_SELF_CHECK_SPEC10141243.md`/`10141315.md`/`NOVA_SINGLE_AI_MIGRATION_PLAN10141246.md`/`10141317.md` 等旧文件的章节才能得到完整实现规则。旧文件只作为变更历史保留，不承载当前实现条件；本文件各节如涉及旧版已有内容，均直接原样内联重写，不写"与上一版一致"。
**状态：** 规格设计稿，**未实现任何代码**。方案修正不等于代码实现或运行验收通过。
**回应对象：** `D:\nova\docs\reviews\CODEX_WP2A_SPEC_TARGETED_REVIEW10141332.md` 的4个Blocker（B1–B4）、2个Major（M1–M2）、1个Minor（m1）。

---

## 0. 本版关闭对照表

| 编号 | 问题 | 本版位置 |
|---|---|---|
| B1 | "开发成功+无审核痕迹+终态"仍被当作单AI正面证据 | 第4节：删除该推断，诚实说明当前无单AI正面证据来源 |
| B2 | 最终completed门禁未重验同执行归属的真实证据/授权 | 第3节：完整伪代码，覆盖test/build/no_tests/no_build |
| B3 | UNIQUE约束与新attempt重新授权冲突 | 第2.3节：唯一键扩展含result_attempt，用NULL区分新旧 |
| B4 | 旧非终态任务冻结缺少实际入口 | 第5节：持久化`legacy_classification`列+5个入口清单 |
| M1 | result_attempt捕获仍是事后查询 | 第1.3节：改为从start/finish返回值直接捕获 |
| M2 | 文档引用新旧混用 | 本节声明+全文自包含 |
| m1 | 拒绝原因持久化语义未定 | 第3.4节：选定"仅API返回+可重新计算"，不新增事件系统 |

---

## 1. 统一执行归属：`result_attempt`（回应M1，修正捕获方式）

### 1.1 代码依据（本轮核验`ActionGate`/`finish_gated_action`真实签名）

- `ActionGate`枚举（`core_loop.rs:2732-2742`）：`Started`/`AlreadySucceeded`/`Uncertain`三个变体，**均不携带`attempt_number`**。
- `gate_action`（`core_loop.rs:2744-2775`）：`Started`分支调用`db.start_action(...)`后用`Ok(_) => Ok(ActionGate::Started)`**丢弃了返回值**；`AlreadySucceeded`分支在`planned.status == Succeeded`时直接返回，此刻`planned`变量本身已经带着正确的`attempt_number`，但函数签名没有把它带出去。
- `finish_gated_action`（`core_loop.rs:2790`起）返回`NovaResult<()>`，而它内部调用的`db.finish_action(...)`（`storage/actions.rs:146-178`）**已经返回`NovaResult<TaskAction>`**（在同一次UPDATE之后立即读回，不是分离的两次查询）——`finish_gated_action`自己把这个现成的、时序正确的返回值丢弃了。

**结论：** 上一版伪代码里"开发成功后`db.find_action(...)?.attempt_number`"确实是"事后重新查询"，即使查询发生在同一个函数调用的下一行，语义上仍是"再问一次现在是什么状态"，不是"记住刚才发生的是什么状态"——如果中间存在任何并发/恢复写入，这两者可能不同。

### 1.2 决定：让现成的返回值携带attempt_number，不新增ID体系

**签名改动（最小，只是把已经算出来的值带出来，不新增查询）：**

```rust
enum ActionGate {
    Started,
    AlreadySucceeded(i64),   // 携带 planned.attempt_number，来自 gate_action 内部已经读到的值
    Uncertain,
}

fn gate_action(...) -> NovaResult<ActionGate> {
    let planned = db.plan_action(task_id, action_type, key)?;
    if planned.status == ActionStatus::Uncertain { return Ok(ActionGate::Uncertain); }
    if planned.status == ActionStatus::Succeeded {
        return Ok(ActionGate::AlreadySucceeded(planned.attempt_number));  // 直接带出已有值
    }
    match db.start_action(task_id, key, is_read_only) {
        Ok(_) => Ok(ActionGate::Started),
        Err(_) => match db.find_action(task_id, key)? {
            Some(action) if action.status == ActionStatus::Succeeded =>
                Ok(ActionGate::AlreadySucceeded(action.attempt_number)),
            Some(action) if action.status == ActionStatus::Uncertain => Ok(ActionGate::Uncertain),
            _ => Ok(ActionGate::Uncertain),
        },
    }
}

fn finish_gated_action(...) -> NovaResult<TaskAction> {   // 原来是 NovaResult<()>
    // ...既有逻辑不变，最后一步：
    db.finish_action(task_id, key, status, evidence_ref, error_summary)   // 直接 return 这个值，不再丢弃
}
```

**调用方（`run_dev_and_test_round`）捕获方式：**

```rust
let result_attempt: i64 = match gate_action(db, task_id, "ai_development", &dev_key, false)? {
    ActionGate::Uncertain => { /* 既有逻辑：转needs_user_action，return */ }
    ActionGate::AlreadySucceeded(attempt) => attempt,               // 直接来自gate_action的返回值
    ActionGate::Started => {
        // ...既有的调用AI、record_command_evidence逻辑不变...
        let finished = finish_gated_action(db, task_id, &dev_key, succeeded, &result)?;  // 捕获返回值
        if !succeeded { /* 既有失败处理，return */ }
        finished.attempt_number                                      // 直接来自finish_gated_action的返回值
    }
};
```

**`result_attempt`之后只作为参数向下传递**（第2节），不再在`run_tests_for_round`/`run_build_for_round`内部重新查询`task_actions`。

### 1.3 影响范围确认

本改动只涉及`core_loop.rs`里`ActionGate`枚举定义、`gate_action`、`finish_gated_action`三处签名/实现，以及所有调用它们的地方改为使用返回值而非丢弃——这些调用点同样存在于`run_review_round`（休眠双AI路径），该路径的调用方式需要同步改（否则编译不过），但休眠路径的`result_attempt`不参与本规格任何门禁逻辑，改动只是类型兼容，不改变其行为。

---

## 2. Schema与写入规则（整合B1/B3所需的`result_attempt`落地）

### 2.1 `self_checks`完整DDL（与上一版内容一致，此处原样内联，不要求回查旧文件）

```sql
CREATE TABLE self_checks (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  round INTEGER NOT NULL,
  result_attempt INTEGER NOT NULL,            -- 来自第1.2节捕获方式，不可为空
  parse_status TEXT NOT NULL CHECK(parse_status IN ('valid','invalid_format')),
  self_reported_status TEXT CHECK(
    self_reported_status IS NULL
    OR self_reported_status IN ('self_reported_complete','self_reported_incomplete','self_reported_blocked')
  ),
  requirement_coverage_json TEXT,
  check_results_json TEXT,
  unfinished_items_json TEXT,
  blocking_issues_json TEXT,
  raw_block_redacted TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_self_checks_task_round_attempt ON self_checks(task_id, round, result_attempt);
-- 当前判定查询："WHERE task_id=? AND round=? AND result_attempt=? ORDER BY rowid DESC LIMIT 1"
```

### 2.2 `test_evidence`/`build_evidence`新增列（结构性迁移，仿照既有`_v*`重建模式）

```sql
-- test_evidence_v7（在既有 test_evidence_v6 全部列基础上新增一列）
CREATE TABLE test_evidence_v7 ( ... 既有列不变 ..., result_attempt INTEGER );
INSERT INTO test_evidence_v7 (..., result_attempt) SELECT ..., NULL FROM test_evidence;
DROP TABLE test_evidence; ALTER TABLE test_evidence_v7 RENAME TO test_evidence;
-- build_evidence_v11 同构，result_attempt 历史行同样 backfill 为 NULL
```

**写入时机：** `run_tests_for_round`/`run_build_for_round`新增`result_attempt: i64`参数（由调用方第1.2节捕获的值传入，不在函数内部重新查询），`record_test_evidence`/`record_build_evidence`写入时带上这个值。

**`AlreadySucceeded`缓存复用分支改动：**

```rust
// run_tests_for_round 内，ActionGate::AlreadySucceeded 分支：
let existing = db.list_test_evidence_for_round(project_id, task_id, result_generation, round)?;
let reusable = existing.into_iter().next_back()
    .filter(|e| e.result_attempt == Some(result_attempt));   // 新增比对，不匹配则视为不存在
match reusable {
    Some(evidence) => return Ok(test_round_outcome_from_status(&evidence)),
    None => {
        // 走既有的"insufficient_evidence + NeedsUserAction"fail-closed分支
        // 原因文案新增一种："测试证据来自更早的一次开发执行，当前执行尚未产生对应证据"
    }
}
```

`run_build_for_round`同构处理。

### 2.3 `no_tests_confirmations`/`no_build_confirmations`——唯一键扩展（回应B3，选定方案）

**最终确定方案（二选一已选定，不保留候选）：** 唯一键扩展为包含`result_attempt`，利用SQL标准语义"UNIQUE约束中的NULL永远不等于另一个NULL，不构成冲突"，让历史行（NULL）与任意新行、以及不同attempt的新行之间天然互不冲突。

```sql
CREATE TABLE no_tests_confirmations_v2 (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  task_id TEXT NOT NULL,
  result_generation INTEGER NOT NULL,
  round INTEGER NOT NULL,
  result_attempt INTEGER,                     -- 历史行迁移时 backfill 为 NULL；新行必须写入真实值
  confirmed_by TEXT NOT NULL CHECK(confirmed_by = 'user'),
  reason TEXT,
  confirmed_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(task_id, result_generation, round, result_attempt),   -- 扩展后的唯一键
  FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id) ON DELETE CASCADE
);
INSERT INTO no_tests_confirmations_v2 (id, project_id, task_id, result_generation, round, result_attempt, confirmed_by, reason, confirmed_at, created_at)
  SELECT id, project_id, task_id, result_generation, round, NULL, confirmed_by, reason, confirmed_at, created_at FROM no_tests_confirmations;
DROP TABLE no_tests_confirmations; ALTER TABLE no_tests_confirmations_v2 RENAME TO no_tests_confirmations;
CREATE INDEX idx_no_tests_confirmations_task ON no_tests_confirmations(task_id);
-- no_build_confirmations_v2 同构
```

**写入语义：**

```rust
pub fn record_no_tests_confirmation(&self, ..., result_attempt: i64, ...) -> NovaResult<NoTestsConfirmation> {
    self.conn().execute(
        "INSERT INTO no_tests_confirmations (id, project_id, task_id, result_generation, round, result_attempt, confirmed_by, reason, confirmed_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'user', ?7, ?8, ?8)
         ON CONFLICT(task_id, result_generation, round, result_attempt) DO NOTHING",
        params![id, project_id, task_id, result_generation, round, result_attempt, reason, now],
    )?;
    // 查询时也带上 result_attempt
    self.conn().query_row(
        "SELECT ... FROM no_tests_confirmations WHERE task_id=?1 AND result_generation=?2 AND round=?3 AND result_attempt=?4",
        params![task_id, result_generation, round, result_attempt], read_confirmation,
    )
}
```

**三个具体问题的回答（回应B3通过条件）：**
1. **新attempt重新确认可写入：** attempt 1确认后唯一键是`(task_id,gen,round,1)`；attempt 2需要确认时唯一键是`(task_id,gen,round,2)`——两者不是同一个元组，`INSERT`不会被attempt 1的行阻挡，天然可以独立写入一条新行。
2. **旧attempt授权不可被误用：** `find_no_tests_confirmation`的查询条件里`result_attempt=?`精确匹配当前值，attempt 1的确认行永远不会被当成attempt 2的授权返回。
3. **历史确认可审计：** 迁移只新增列、backfill为NULL，不删除、不改写任何历史行内容；`DROP TABLE`+`RENAME`是结构性重建（与仓库里`test_evidence_v6`等既有迁移手法完全一致），不是数据丢弃。

**`no_tests_applicable_is_authorized`/`no_build_applicable_is_authorized`同步改动：** 新增`result_attempt: i64`参数，传给`find_no_tests_confirmation`/`find_no_build_confirmation`；`no_build_applicable_is_authorized`的`mechanical_not_applicable`路径天然通过`build_evidence.result_attempt`的比对覆盖（第2.2节已处理），不需要额外改动。

---

## 3. 最终完工门禁——完整伪代码（回应B2，核心新增内容）

### 3.1 代码依据

- `storage/tasks.rs:316-332`：现有`transition_task`在`to=='completed'`时，`WHERE`子句的`EXISTS`只检查`user_acceptance`。
- `core_loop.rs:562-583`：`accept_task_result`只是"先写user_acceptance，再调用`transition_task(...,Completed)`"，自身不是权威判定点。
- 全仓库核验：写`tasks.state`的SQL只有`storage/tasks.rs`这一处（`grep "SET state"`确认），`transition_task`是唯一权威入口，无论通过通用Tauri命令、`accept_task_result`还是恢复路径调用，最终都落到这一个函数。

### 3.2 完整判定伪代码（这是"最终门禁"本身，不是预览/辅助函数）

```rust
// 位于 Database::transition_task 内部，当 to == TaskState::Completed 时触发
// 全程在已经开启的 BEGIN IMMEDIATE 事务内，与现有事务边界完全一致，不新开事务

fn completed_gate_checks(&self, task: &Task) -> NovaResult<Result<(), RejectReason>> {
    // 0. 旧任务冻结检查（见第5节），放在最前面：
    if let Some(classification) = &task.legacy_classification {
        if classification == "dual_ai_evidenced" || classification == "unknown_insufficient" {
            return Ok(Err(RejectReason::LegacyTaskFrozen));
        }
    }

    // 1. 当前采纳的证据所在 (generation, round)：
    //    adopted_passing_generation_round() 本身不改动，仍只按 generation+round 查“是否有通过证据”
    let Some((adopted_generation, adopted_round)) =
        adopted_passing_generation_round(self, &task.project_id, &task.id, task)? else {
        return Ok(Err(RejectReason::NoAdoptedEvidence));   // 防御性：正常路径不会在AwaitingUserAcceptance触发
    };

    // 2. 当前开发/修复 action 的真实 attempt（从 task_actions 现读，这是“当前执行归属”的唯一权威来源）：
    let dev_key = format!("dev-{}-round-{}", task.id, adopted_round);
    let Some(dev_action) = self.find_action(&task.id, &dev_key)? else {
        return Ok(Err(RejectReason::DevActionNotSucceeded));
    };
    if dev_action.status != ActionStatus::Succeeded {
        return Ok(Err(RejectReason::DevActionNotSucceeded));
    }
    let current_attempt = dev_action.attempt_number;

    // 3. 同一 result_attempt 的真实测试证据，或同 attempt 的有效无测试授权：
    let test_ok = self.list_test_evidence_for_round(&task.project_id, &task.id, adopted_generation, adopted_round)?
        .into_iter()
        .any(|e| e.result_attempt == Some(current_attempt) && e.status == "passed")
        || no_tests_applicable_is_authorized(self, &task.project_id, &task.id, adopted_generation, adopted_round, current_attempt)?;
    if !test_ok {
        return Ok(Err(RejectReason::EvidenceAttemptMismatch { kind: "test" }));
    }

    // 4. 同一 result_attempt 的真实构建证据，或同 attempt 的有效无构建授权（含 mechanical_not_applicable 路径）：
    let build_ok = self.list_build_evidence_for_round(&task.project_id, &task.id, adopted_generation, adopted_round)?
        .into_iter()
        .any(|e| e.result_attempt == Some(current_attempt) && e.status == "passed")
        || /* no_build_applicable_is_authorized 同构，含 result_attempt 比对 */ ...;
    if !build_ok {
        return Ok(Err(RejectReason::EvidenceAttemptMismatch { kind: "build" }));
    }

    // 5. 当前有效自检（round + result_attempt 精确匹配，取 rowid 最新一行）：
    let Some(self_check) = self.current_self_check(&task.id, adopted_round, current_attempt)? else {
        return Ok(Err(RejectReason::SelfCheckMissing));
    };
    if self_check.parse_status != "valid" { return Ok(Err(RejectReason::SelfCheckInvalidFormat)); }
    if self_check.self_reported_status != Some("self_reported_complete".into()) {
        return Ok(Err(RejectReason::SelfCheckNotComplete));
    }
    if !self_check.unfinished_items.is_empty() { return Ok(Err(RejectReason::UnfinishedItemsPresent)); }
    if !self_check.blocking_issues.is_empty() { return Ok(Err(RejectReason::BlockingIssuesPresent)); }

    // 6. 需求覆盖完整性：
    let authoritative_items = self.list_confirmed_requirement_items(&task.id)?;
    if authoritative_items.is_empty() {
        return Ok(Err(RejectReason::RequirementSnapshotMissing));
    }
    if !coverage_matches_exactly(&self_check.requirement_coverage, &authoritative_items) {
        return Ok(Err(RejectReason::RequirementCoverageIncomplete));
    }

    // 7. 当前用户确认（既有逻辑，result_generation 而非 round，原样保留）：
    if !self.has_current_user_acceptance(&task.id, task.result_generation)? {
        return Ok(Err(RejectReason::UserAcceptanceMissing));
    }

    Ok(Ok(()))
}
```

**接入点：** `transition_task`在`to == Completed`分支，调用上面的`completed_gate_checks`，结果为`Err(reason)`时，`UPDATE`语句的`WHERE`附加条件视为不满足（`rows_affected == 0`的等价路径），函数返回`Err(NovaError::Config(reason.to_message()))`，**不执行任何状态写入**（见第3.3节如何与现有`UPDATE...WHERE`结构结合）。

**覆盖的入口（回应"必须覆盖任务已经处于AwaitingUserAcceptance,但当前自检+旧证据/授权的直接完成尝试"）：** 因为这段逻辑就在`Database::transition_task`本体内，无论调用方是通用`transition_task`命令、`accept_task_result`、还是任何未来的恢复/重入代码，只要最终都是调用这一个函数，都会经过第3步、第4步对"当前`result_attempt`"的重新核验——**不依赖Verifying阶段"曾经检查过"这个事实**，每一次尝试进入`completed`都重新查一遍当前的真实证据/授权是否对应当前attempt。

### 3.3 与现有`UPDATE...WHERE`原子语句的结合方式

现有`storage/tasks.rs:317-333`的`UPDATE`语句本身保持"状态合法性+目标状态"的`WHERE`结构不变；`completed_gate_checks`在**进入这条UPDATE之前**、同一个`BEGIN IMMEDIATE`事务内执行只读查询并计算出`Ok(())`或`Err(reason)`，若为`Err`，函数直接提前返回错误、回滚事务，不执行UPDATE；若为`Ok`，再执行UPDATE（此刻UPDATE本身仍保留原有的`EXISTS(user_acceptance...)`条件作为双重保险，不删除现有逐行防护）。全程不释放`BEGIN IMMEDIATE`持有的写锁，不存在"判定"与"写入"之间可以被其他连接插入的窗口。

### 3.4 拒绝原因的持久化语义（回应m1，选定方案）

**决定：拒绝原因只通过API错误返回，不持久化专门的审计/事件记录，不新增事件系统。** 理由：`RejectReason`的每一种取值，都是对已经持久化的数据（`self_checks`/`confirmed_requirement_items`/`test_evidence`/`build_evidence`/`user_acceptance`/`legacy_classification`）做一次只读推导的结果，不是需要单独记住的新事实——用户重新打开任务时，前端调用一个**只读预检命令**（复用`completed_gate_checks`的只读部分，不触发`UPDATE`），用当前数据重新算一遍，得到的原因与之前完全一致（因为输入数据没变），**不需要专门的事件表来"记住"曾经为什么被拒绝过**。`task_state_events`表不会收到任何"completed被拒绝"的记录——因为没有发生状态转换，这与第3.3节的B3结论一致。

---

## 4. 历史分类——删除错误的单AI正面证据推断（回应B1）

### 4.1 问题确认

上一版算法：`(ai_development Succeeded存在) AND (两个强信号都不存在) AND (任务已到达终态或待验收)` → `single_ai_evidenced`。**本轮确认这个推断不成立：** 开发步骤（`run_dev_and_test_round`）是单AI/双AI路径**完全共用**的同一函数（`core_loop.rs`核验确认），"开发成功"只能证明"开发这一步执行过"，不能证明"任务没有进入或不需要进入双AI审核路径"——这是把"缺少证据"当成了"证据缺失本身=单AI"的逆向推断，逻辑上不成立。

### 4.2 决定：删除该分支，诚实承认当前无可用的单AI正面证据来源

```
若 (ai_review action存在) OR (task_reviews行存在)：
    → dual_ai_evidenced（+ integrity_warning，若两强信号状态不一致）
否则：
    → unknown_insufficient
```

**不再有`single_ai_evidenced`在当前可达代码路径下被真实触发的场景——这是本轮核验后的诚实结论，不是回避设计。** 原因：`run_core_loop_dual_ai_v0_2`从未被任何Tauri命令注册、不可达，意味着*所有*任务事实上都只能通过单AI生产入口`run_core_loop`运行；但这件事本身无法从任务的执行痕迹里**独立证明**——因为开发/测试/构建的Action Ledger记录在两条代码路径下长得一模一样，缺少"哪个顶层函数启动了这次运行"的任何持久化标记。`不为识别历史任务增加不必要的新系统`（PM原话）意味着本规格不会为了填补这个空白去新增一个"启动入口标记"字段——**这不是遗漏，是明确的范围选择**：当前代码对"这个任务是从哪个顶层入口启动的"这件事本来就没有留痕，WP2A不为了让历史分类更"好看"而额外发明一套记录体系。

**`single_ai_evidenced`这个分类枚举值在算法定义里保留**（为未来版本留出空间，例如将来真的加了启动入口标记），但在当前schema下，它永远不会被本算法实际赋予给任何任务——所有任务只会落入`dual_ai_evidenced`或`unknown_insufficient`两类之一。

### 4.3 组合判定表（回应B1通过条件，更新后与算法完全一致）

| 组合 | 分类 |
|---|---|
| 仅有`ai_review action` | `dual_ai_evidenced`（+integrity_warning） |
| 仅有`task_reviews` | `dual_ai_evidenced`（+integrity_warning） |
| `ai_review action`有，`task_reviews`无 | `dual_ai_evidenced`（+integrity_warning） |
| 仅有实质审核文档（无数据库强信号） | `unknown_insufficient`（"仅文档命中，无数据库强信号支撑"） |
| 两强信号都无，但开发成功、任务已到达终态/待验收 | `unknown_insufficient`（"无法区分是否曾进入双AI审核路径"，**不再是single_ai_evidenced**） |
| 开发都未成功 | `unknown_insufficient`（"数据不足"） |
| 历史schema证据源不存在 | `unknown_insufficient`（"历史schema版本证据不全"） |

### 4.4 保留规则

已完成历史任务（任意分类）只读保留，不改写结论——这条不受本节修改影响。

---

## 5. 旧非终态任务的冻结——持久化分类+实际入口约束（回应B4）

### 5.1 为什么需要持久化（而不是每次都重新计算）

**关键澄清（避免"冻结检查误伤新任务"）：** 第4节的分类算法如果对**所有**任务（包括WP2A上线后新建的任务）实时生效，会把"刚完成开发、还没来得及产生任何审核痕迹"的全新单AI任务也判成`unknown_insufficient`并冻结——这是不可接受的。**冻结机制的真实目标只是处理"WP2A迁移落地那一刻，数据库里已经存在的非终态历史任务"，不是对未来任务的常态检查。** 新任务的正确性由第3节的完工门禁（重验当前`result_attempt`的真实证据/自检/确认）独立保证，不依赖历史分类。

### 5.2 决定：迁移时一次性计算并持久化，不是每次动态重算

```sql
ALTER TABLE tasks ADD COLUMN legacy_classification TEXT
  CHECK(legacy_classification IS NULL
        OR legacy_classification IN ('dual_ai_evidenced','single_ai_evidenced','unknown_insufficient'));
```

**迁移脚本行为：** 在WP2A迁移执行的那一刻，对**当时**处于非终态（`state NOT IN ('completed','failed','cancelled')`）的每一个任务，按第4节算法计算一次分类并写入这一列；**终态任务不计算，保持NULL**（它们已经终态，永远不会再触发第3节门禁，分类与否没有实际意义）；**迁移之后新建的任务`legacy_classification`恒为NULL**（因为新任务创建时就是NULL，没有任何代码会在之后为新任务填这一列）。

**冻结判定：** `task.legacy_classification IN ('dual_ai_evidenced', 'unknown_insufficient')` → 冻结；为`NULL`或`'single_ai_evidenced'`（当前不会真实出现，见4.2节，但枚举定义允许）→ 不冻结。

### 5.3 接入的实际入口（逐一列出，回应B4"必须接入哪些入口"）

| # | 入口 | 代码位置 | 冻结时行为 |
|---|---|---|---|
| 1 | `run_core_loop`启动检查 | `core_loop.rs:101-155`（现有只检查`state IN (Confirmed,Queued)`） | 新增：若`legacy_classification`冻结，返回`Err`，不进入`Running`，不调用任何AI |
| 2 | `unblock_test_execution_for_retry` | 对应函数（现有可把`NeedsUserAction`重新排队） | 新增同样检查，冻结则拒绝排队 |
| 3 | 恢复扫描/重入（`storage/recovery.rs`） | 现有`run_startup_recovery`只把运行中状态标记`interrupted`（纯只读记账，不恢复执行） | 本身不需要改动——它不触发执行，只是记账；但恢复**之后**任何试图继续执行的调用都会走入口1/2，天然被拦截 |
| 4 | 用户触发的通用状态转换 | `commands.rs`暴露的`transition_task`（任意状态→`Queued`/`Running`） | `Database::transition_task`新增分支：`to IN (Queued, Running)`时，同样核验`legacy_classification`，冻结则拒绝转换（与`to==Completed`的新增校验是同一类"目标状态附加检查"模式，仿照既有`to==PreviewReady`递增generation的写法） |
| 5 | 最终`Completed`转换 | 第3.2节`completed_gate_checks`第0步 | 已包含，防御性最后一道 |

### 5.4 允许的操作边界

- **只读查看**（加载任务详情、证据时间线、技术文档）：不受冻结影响，始终允许。
- **取消**（`transition_task(to=Cancelled)`）：允许——`Cancelled`是终态，不是"继续执行"，与冻结目标（防止继续/完成）不冲突。
- **`generate_requirement_snapshot`**（第1节WP2A-0命令）：允许调用（它只是把已确认的需求字段落成结构化快照，不涉及继续执行），**但明确不解除冻结**——调用后`legacy_classification`列不变，入口1/2/4/5的检查继续生效。这是本轮对上一轮"可能被误解为既补需求确认又解除冻结"的明确澄清。

### 5.5 拒绝后的状态/事件一致性

与第3.4节同一原则：冻结拒绝只返回API错误，不执行任何状态转换，不写入声称发生了转换的`task_state_events`行——任务停留在冻结判定前的原状态。

### 5.6 解冻机制——明确不在本轮范围内

本轮**不提供**任何"人工解冻"路径。一旦`legacy_classification`落在冻结区间，该任务除了只读查看和取消外，没有其他出路，直到未来某个版本显式设计并实现解冻决策——这不是本轮遗漏，是明确的范围边界（PM未要求本轮设计解冻机制）。

---

## 6. `preview_ready`与`completed`分离（维持已关闭结论，原样内联）

`adopted_passing_generation_round()`本身不改动，不受本规格任何新增条件影响。`preview_ready`的进入条件只看真实测试/构建证据是否通过（与`result_attempt`无关——这是因为`preview_ready`本来就是"看一眼当前结果"的状态，不是"确认可以完工"，不需要比对执行归属）；只有`AwaitingUserAcceptance → Completed`这一条边会触发第3节的完整门禁。

---

## 7. 已关闭规则重申（维持不变，防止被后续轮次误改）

- 自检必须`self_reported_status == self_reported_complete`且两类数组均为空，三者独立判定，不互相替代。
- `requirement_coverage`门禁要求与`confirmed_requirement_items`权威集合完全一致（无漏项/重复/伪造/partial/not_covered）。
- 门禁失败时拒绝转换、保持`AwaitingUserAcceptance`，不写`NeedsUserAction`。
- 不默认新增AI调用，不突破一次修复上限（`MAX_AUTO_FIX_CYCLES`/`current_pending_round`现状不变）。
- 既有无测试/无构建授权规则（`mechanical_not_applicable`/用户确认两条路径）保留，只新增`result_attempt`维度的归属校验。

---

## 8. 验收矩阵（全表重写，整合全部已知场景）

| # | 场景 | 预期结果 | 原因/说明 |
|---|---|---|---|
| 1 | 有效完成（happy path） | `Completed` | 全部7步检查通过 |
| 2 | 旧证据直接完成尝试（attempt已变，evidence仍是旧的） | 拒绝，停留`AwaitingUserAcceptance` | `EvidenceAttemptMismatch` |
| 3 | 旧授权直接完成尝试（no_tests/no_build确认是旧attempt的） | 拒绝 | `EvidenceAttemptMismatch` |
| 4 | 新attempt重新授权 | 新确认行可独立写入（唯一键含result_attempt） | 不拒绝，正常授权 |
| 5 | 旧执行晚到（旧attempt的测试/构建结果在新attempt产生后才写入） | `AlreadySucceeded`比对`result_attempt`不匹配，fail-closed为insufficient_evidence，更早阶段（`Verifying`）已拦截 | 不会到达`AwaitingUserAcceptance` |
| 6 | 历史`result_attempt=NULL`的证据/确认 | 永远不匹配当前attempt（`IS NOT NULL AND =`判定），不能满足门禁 | `EvidenceAttemptMismatch` |
| 7 | 旧非终态任务：`legacy_classification`冻结，尝试`run_core_loop`启动 | 拒绝，不进入`Running` | `LegacyTaskFrozen` |
| 8 | 旧非终态任务：尝试`unblock_test_execution_for_retry` | 拒绝排队 | `LegacyTaskFrozen` |
| 9 | 旧非终态任务：通用`transition_task`到`Queued`/`Running` | 拒绝 | `LegacyTaskFrozen` |
| 10 | 旧非终态任务：调用`generate_requirement_snapshot` | 允许执行，但冻结状态不变 | 不解除冻结 |
| 11 | 旧非终态任务：只读查看 | 允许 | 不受冻结影响 |
| 12 | 旧非终态任务：取消 | 允许，转入`Cancelled` | 终态，不是继续执行 |
| 13 | 旧非终态任务：直接尝试`Completed` | 拒绝 | `LegacyTaskFrozen`（第0步） |
| 14 | 新建任务（迁移后创建），自然没有审核痕迹 | 不冻结，正常走单AI闭环 | `legacy_classification`恒为NULL |
| 15 | 旧未完成任务无需求快照 | 拒绝完工，提示确认沿用生成清单 | `RequirementSnapshotMissing` |
| 16 | 历史已完成任务 | 不重新评估，只读保留 | 不适用 |
| 17 | 用户确认绕过（直接调用通用命令） | 同样经过`Database::transition_task`唯一入口 | 视具体缺失条件 |
| 18 | 直接状态转换绕过 | 全仓库唯一写`completed`的SQL已核验 | — |
| 19 | 并发变化（判定后、写入前数据被覆盖） | `BEGIN IMMEDIATE`事务内完成，无交错窗口 | — |
| 20 | 自检相关各失败分支（缺失/无效/未完成/阻断/覆盖不全） | 拒绝，停留`AwaitingUserAcceptance` | 对应各自原因 |
| 21 | 缺少用户确认 | 停留`AwaitingUserAcceptance`，非"拒绝"而是"尚未操作" | `UserAcceptanceMissing` |
| 22 | 真实证据门禁回归 | 不满足则停在既有失败/修复状态 | 不适用 |
| 23 | 一次修复上限回归 | round上限仍为2 | — |
| 24 | 可预览不可完工 | `preview_ready`达成，`completed`被拒 | 第6节分离结论 |
| 25 | 拒绝原因重新计算（m1） | 关闭任务重新打开后，只读预检命令给出与之前一致的原因 | 不依赖专门事件表 |

---

## 9. 本文件状态声明

本文件是**规格设计稿**。`self_checks`、`confirmed_requirement_items`、`tasks.legacy_classification`列、`test_evidence`/`build_evidence`/`no_tests_confirmations`/`no_build_confirmations`的`result_attempt`列、`Database::transition_task`的门禁扩展、`ActionGate`签名改动、5个入口的冻结检查、`generate_requirement_snapshot`命令、全部测试**均未实现**。需经 Codex 独立复审通过后，才能转入编码工作包（WP2A-0 → WP2A主体）。
