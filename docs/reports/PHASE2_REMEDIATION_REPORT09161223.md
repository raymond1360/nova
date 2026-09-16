# Phase 2 Remediation Report

生成时间：2026-09-16 12:23（北京时间）

依据审核报告：`D:\nova-review\CODEX_PHASE2_TASK_STATE_RECOVERY_REVIEW09161035.md`（结论：FAIL，2 Blocker / 3 Major / 2 Minor / 2 Suggestion）

## 1. 修复前 HEAD

`92d1f7e8aa273adc033d666c0f08c0b52336fba4`（分支 `feature/v0.1.0-mvp`，`milestone/runtime-foundation` 仍指向 Phase 1 通过提交 `f91dd62ad12f934ad40ccff87c2a966df24fc620`，未被改动）。

## 2. 修复后提交 SHA

见本报告末尾"提交"一节（提交发生在本报告写入之后）；最终 SHA 见本轮开发的最终回复。

## 3. 完整变更文件列表

```
M  apps/desktop/src-tauri/src/commands.rs
M  apps/desktop/src-tauri/src/models.rs
M  apps/desktop/src-tauri/src/storage/actions.rs
M  apps/desktop/src-tauri/src/storage/concurrency_tests.rs
M  apps/desktop/src-tauri/src/storage/evidence.rs
M  apps/desktop/src-tauri/src/storage/memory.rs
M  apps/desktop/src-tauri/src/storage/migrations.rs
M  apps/desktop/src-tauri/src/storage/projects.rs
M  apps/desktop/src-tauri/src/storage/recovery.rs
M  apps/desktop/src-tauri/src/storage/tasks.rs
A  docs/reports/PHASE2_REMEDIATION_REPORT09161223.md（本报告）
```

未修改：`Cargo.toml`、`Cargo.lock`、`package.json`/`package-lock.json`、`tauri.conf.json`、任何前端文件、任何 CI/脚本文件。未新增第三方依赖，未升级任何依赖版本。未开始 Phase 3，未扩展产品功能。

## 4. Schema v3 迁移设计

`CURRENT_SCHEMA_VERSION` 从 2 升级为 3。新增 `Migration { version: 3, requires_backup: true, sql: V3_SQL }`，不修改已提交的 v1/v2 迁移 SQL。

`V3_SQL` 包含：

1. `ALTER TABLE tasks ADD COLUMN result_generation INTEGER NOT NULL DEFAULT 0;`
2. 为以下 9 张表补齐 `project_id -> nova_projects(id) ON DELETE CASCADE` 外键（SQLite 不支持 `ALTER TABLE ADD FOREIGN KEY`，全部采用 `CREATE TABLE ..._v3 -> INSERT ... SELECT -> DROP TABLE -> ALTER TABLE ... RENAME` 的数据保留式重建）：`task_state_events`、`recovery_checkpoints`、`timeline_events`、`command_evidence`、`git_evidence`、`file_change_evidence`、`test_evidence`、`generated_documents`、重新设计后的 `user_acceptance`。重建后为每张表重新创建原有索引。
3. `user_acceptance` 结构性重设计：`task_id TEXT NOT NULL UNIQUE` → `result_generation INTEGER NOT NULL` + `UNIQUE(task_id, result_generation)`；迁移时旧行通过 `SELECT id, project_id, task_id, 0, accepted, user_note, accepted_at, created_at FROM user_acceptance` 全部标记为 `result_generation = 0`——这是一个真实任务永远不会自然到达的代数（`bump_result_generation` 从 1 开始计数），因此历史验收记录天然无法满足新的完成门禁。

## 5. result_generation 与验收绑定规则

- `tasks.result_generation` 默认为 0；每次 `transition_task` 把任务转入 `preview_ready` 时，在**同一个事务**内执行 `UPDATE tasks SET result_generation = result_generation + 1 WHERE id = ?`——与状态写入、`task_state_events` 插入是同一次 `BEGIN IMMEDIATE ... COMMIT`，不存在中间可观察状态。
- `record_user_acceptance`（`storage/evidence.rs`）通过单条 `INSERT INTO user_acceptance (...) SELECT ... FROM tasks WHERE id = ? AND project_id = ? AND state = 'awaiting_user_acceptance'` 语句，把"任务当前是否处于 awaiting_user_acceptance"和"当前 result_generation 是多少"两个事实的读取与验收写入折叠进同一条原子语句——`rows_affected = 0` 时直接拒绝，不给出误导性成功；不存在"先查状态再写入"的竞态窗口。
- `transition_task` 的 `completed` 门禁同样折叠进它自身的原子条件更新语句：`WHERE ... AND (?to <> 'completed' OR (result_generation > 0 AND EXISTS (SELECT 1 FROM user_acceptance WHERE task_id = tasks.id AND result_generation = tasks.result_generation AND accepted = 1)))`。
- 效果：旧 generation 的 `accepted = 1` 记录永远不能用于完成新 generation；`accepted = false`（拒绝）不解锁完成；未处于 `awaiting_user_acceptance` 时无法写入验收；`result_generation = 0`（含迁移历史记录与从未产生结果的任务）永远不满足门禁。

## 6. Action Ledger CAS / 事务设计

`storage/actions.rs` 中 `start_action`、`finish_action`、`acknowledge_uncertain_action` 全部由"先 `find_action` 读、再无条件 `UPDATE`"改写为**单条带 `WHERE status = ...` 条件的 `UPDATE`**，并检查 `rows_affected`：

- `start_action`：非只读动作要求 `WHERE status = 'planned'`；只读动作额外允许从 `succeeded`/`failed`/`cancelled`/`uncertain` 重试，`attempt_number` 通过 `CASE WHEN status = 'planned' THEN attempt_number ELSE attempt_number + 1 END` 在同一条语句内正确递增。
- `finish_action`：`WHERE status = 'started'`，只允许写入 `succeeded`/`failed`/`cancelled`。
- `acknowledge_uncertain_action`：`WHERE status = 'uncertain'`。
- `rows_affected = 0` 时，进行一次**非权威性**的诊断读取（仅用于生成清晰错误信息，不参与决策），返回结构化 `NovaError`。

单条 `UPDATE ... WHERE ...` 语句本身在 SQLite 中是原子的，因此这里不需要额外的 `BEGIN IMMEDIATE` 包裹——决策和写入是同一条语句、同一次原子操作，彻底消除了原实现中"先读后写"的 TOCTOU 竞态窗口。

## 7. Tauri Actor 权限边界

- `commands::transition_task` 不再接收 `actor: String` 参数，函数体内硬编码调用 `db.transition_task(..., Actor::User, ...)`。
- `commands::update_project_memory_field` 不再接收 `changed_by: String` 参数，硬编码 `Actor::User`。
- `Actor::System`、`Actor::Nova`、`Actor::ConnectedAi` 以及 `is_recovery = true` 只能由后端内部代码（`storage::recovery`、`storage::tasks::Database::create_task` 等）直接调用 `Database::transition_task`/`update_project_memory_field` 时使用——这两个 Rust 方法本身仍完整保留 `actor: Actor` 参数，只是 Tauri command 这一层边界不再转发前端传入的任意字符串。这是**编译期结构性保证**：前端能调用的 `#[tauri::command]` 函数签名里已经不存在 `actor`/`changed_by` 字段，无法通过参数伪造，比任何运行时校验都更强。
- 持久化文本（`transition_task` 的 `reason`/`safety_summary`、`update_project_memory_field` 的 `reason`、`record_user_acceptance` 的 `user_note`）在写入数据库前统一经过既有 `redact_sensitive_text` 处理。

## 8. 全项目恢复算法

`storage::recovery::run_startup_recovery` 重构为两个独立阶段：

1. **数据修复阶段**（新增 `Database::interrupt_all_in_flight_tasks`）：`SELECT id, project_id, state FROM tasks WHERE state IN ('running', 'verifying', 'safe_fixing')`，对**每一个项目**符合条件的**每一个任务**，通过标准的 CAS 保护的 `transition_task` 路径转为 `interrupted` 并写入 `recovery_checkpoints`。完全不依赖、也不受限于哪个项目是"最后活动项目"，即使最后活动项目的目录已经丢失也照常执行。
2. **摘要生成阶段**（保留在 `run_startup_recovery` 主体）：仅使用最后活动项目来构建面向用户的自然语言摘要；此时该项目自己的任务（如果之前在途）已经在阶段 1 被处理完毕，这一阶段只读取状态，不再做任何状态转换。

幂等性：阶段 1 的 `SELECT ... WHERE state IN (...)` 只会选中仍处于在途状态的任务，一旦转为 `interrupted` 就不会再被选中，重复调用天然不产生重复转换或重复 checkpoint。并发一致性：每个任务的转换都走 `transition_task` 自身的 `WHERE state = from` 条件更新，两个连接同时扫描时，输家的这一条 `transition_task` 调用返回 `Err` 并被安全跳过，不会重试也不会中断整个扫描。

## 9. create_project 原子事务证明

`storage::projects::Database::create_project` 现在把 `INSERT INTO nova_projects` 与 `INSERT INTO project_memory` 包裹在同一个 `BEGIN IMMEDIATE ... COMMIT`/`ROLLBACK` 块内。新增测试 `a_failed_memory_insert_leaves_no_half_created_project_behind`：先 `DROP TABLE project_memory` 制造确定性失败，调用 `create_project` 后断言 `nova_projects` 表中新增行数为 0——证明失败时项目行不会残留。`a_duplicate_or_constraint_conflicting_project_leaves_no_half_created_row` 进一步验证重复目录注册失败时两张表的行数均无变化。

同样把 `set_last_active_project` 的时间戳更新与 `app_settings` 写入纳入同一事务（Minor 1），并用 `set_last_active_project_is_atomic_with_the_app_settings_write` 测试验证。

## 10. 新增测试名称及覆盖内容

按文件列出本轮新增测试（共 21 个新增 `#[test]`，另有既有测试因行为变化而更新断言，见第 13 节"已知限制"之外的说明）：

**`storage/migrations.rs`**（+1）
- `upgrades_a_v2_database_to_v3_relabeling_old_acceptance_as_generation_zero` — v2 数据库安全升级到 v3，旧验收行迁移为 generation 0。

**`storage/tasks.rs`**（+6）
- `rejecting_the_current_result_does_not_allow_completion` — 显式拒绝不能完成。
- `acceptance_can_only_be_recorded_while_awaiting_user_acceptance` — 非 awaiting_user_acceptance 状态不能记录验收。
- `new_result_generation_invalidates_the_old_accepted_generation` — generation 1 验收后产生 generation 2，旧验收不能完成 generation 2（Blocker 1 核心测试）。
- `migrated_generation_zero_acceptance_cannot_complete_a_newer_generation` — 迁移产生的 generation 0 历史记录不能完成任务。
- `transition_reason_is_redacted_before_it_reaches_task_state_events` — 状态转换 reason 脱敏。
- `internal_callers_can_still_use_system_and_recovery_actors` — 内部恢复流程仍可使用 system/recovery 身份。

**`storage/evidence.rs`**（+1）
- `acceptance_note_is_redacted_before_storage` — 验收备注脱敏。

**`storage/memory.rs`**（+1）
- `revision_reason_is_redacted_before_storage` — 记忆修订 reason 脱敏。

**`storage/projects.rs`**（+4）
- `creating_a_project_also_creates_exactly_one_memory_row_in_the_same_transaction`
- `a_failed_memory_insert_leaves_no_half_created_project_behind`
- `a_duplicate_or_constraint_conflicting_project_leaves_no_half_created_row`
- `set_last_active_project_is_atomic_with_the_app_settings_write`

**`storage/recovery.rs`**（+4）
- `a_running_task_in_a_non_last_active_project_is_still_interrupted`
- `in_flight_tasks_across_more_than_two_projects_are_all_recovered`
- `multiple_in_flight_tasks_within_a_single_project_are_all_recovered`
- `missing_last_active_directory_does_not_block_the_database_repair_sweep`

**`storage/concurrency_tests.rs`**（+4，均使用真实文件级 SQLite + 真实 OS 线程 + 两个独立连接）
- `concurrent_start_action_for_a_non_read_only_action_only_one_wins`
- `concurrent_finish_action_for_the_same_started_action_only_one_wins`
- `concurrent_completion_and_a_fresh_result_never_completes_a_stale_generation`
- `concurrent_startup_recovery_from_two_connections_never_duplicates_records`

以下既有测试因行为按预期变严格而更新（非新增，行为修正）：`user_acceptance_is_uniquely_associated_with_one_task`（改为先驱动到 awaiting_user_acceptance）、`failed_and_cancelled_can_never_reach_completed`（改为断言 record_user_acceptance 本身即被拒绝）。

## 11. 全部命令及真实结果

| # | 命令 | 工作目录 | 退出码 |
|---|------|----------|--------|
| 1 | `cargo fmt --all -- --check` | `apps/desktop/src-tauri` | 0 |
| 2 | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | `apps/desktop/src-tauri` | 0 |
| 3 | `cargo test --workspace --all-targets --all-features` | `apps/desktop/src-tauri` | 0（219 passed / 0 failed / 0 ignored） |
| 4 | `cargo build --workspace --release` | `apps/desktop/src-tauri` | 0 |
| 5 | `npm test -- --run` | `apps/desktop` | 0（3 files / 13 tests passed） |
| 6 | `npm run build` | `apps/desktop` | 0 |
| 7 | `npm run tauri build` | `apps/desktop` | 0，`Built application at: ...\target\release\nova.exe` |

按模块的定向测试结果：

```
storage::migrations         13 passed
storage::projects           13 passed
storage::memory               8 passed
storage::tasks               19 passed
storage::actions              8 passed
storage::recovery            12 passed
storage::evidence            12 passed
storage::concurrency_tests    9 passed
```

（94 个 storage 测试 + 125 个 Phase 1 既有测试 = 219，与 `cargo test` 总数一致。）

Rust 测试无 skip/ignore（`grep -rn "#\[ignore\]" src/` 无匹配）；前端测试无 skip（`.skip(`/`.todo(`/`describe.skip`/`it.skip`/`test.skip` 均无匹配）。

正式 release 二进制清单：`target/release/nova.exe`（唯一顶层可执行文件）；`target/release` 树下无任何 `nova_test_helper` 相关文件；无 `bundle` 目录（`tauri.conf.json` 未改动）。

## 12. git status --short（提交前）

```
 M apps/desktop/src-tauri/src/commands.rs
 M apps/desktop/src-tauri/src/models.rs
 M apps/desktop/src-tauri/src/storage/actions.rs
 M apps/desktop/src-tauri/src/storage/concurrency_tests.rs
 M apps/desktop/src-tauri/src/storage/evidence.rs
 M apps/desktop/src-tauri/src/storage/memory.rs
 M apps/desktop/src-tauri/src/storage/migrations.rs
 M apps/desktop/src-tauri/src/storage/projects.rs
 M apps/desktop/src-tauri/src/storage/recovery.rs
 M apps/desktop/src-tauri/src/storage/tasks.rs
?? docs/reports/PHASE2_REMEDIATION_REPORT09161223.md
```

（提交后的 `git status --short` 见最终回复，应为空。）

## 13. git log --oneline --decorate -8（修复前）

```
92d1f7e (HEAD -> feature/v0.1.0-mvp) feat(storage): add project registry, memory, task state machine, and phase 4 evidence foundation
f91dd62 (tag: milestone/runtime-foundation) fix(runtime): isolate test helper and validate process input
623834c fix(runtime): suspend process before job assignment
16a30cb fix(runtime): close phase 1 safety review findings
c55b2c8 feat(runtime): add safe tool detection and process boundary
c074c28 (feature/nova-technical-mvp) docs(release): record pre phase 1 cleanup
b075a40 docs(release): record pre phase 1 cleanup
883850c docs(design): accept v0.1.0 uiux baseline
```

（修复后的 `git log --oneline --decorate -8` 见最终回复，顶部应新增本轮提交。）

## 14. 尚存风险

1. **Suggestion 事项未全部处理**：Codex 报告的 Suggestion 2（"为旧验收被新 generation 作废增加显式测试"）已通过 `new_result_generation_invalidates_the_old_accepted_generation` 覆盖；Suggestion 1（"为所有 project_id 列增加 FK"）已通过 v3 迁移覆盖 9 张表，`evidence_refs` 表本身没有 `project_id` 列（它通过 `event_id` 关联 `timeline_events`），未新增该列，因为这超出本轮 Blocker/Major 修复范围且会改变该表的既有语义（不属于本轮授权范围内的功能扩展）。
2. **`interrupt_all_in_flight_tasks` 内部的 `transition_task` 写入与 `recovery_checkpoints` 插入不是同一个事务**（详见 `storage/recovery.rs` 内联注释）：如果进程恰好在两者之间崩溃，任务状态本身已经安全落盘为 `interrupted`，唯一影响是下次生成摘要时退化为通用文案而非细节文案，不产生重复记录或错误状态。这是刻意的、有文档说明的权衡，不是新发现的缺陷。
3. **Phase 1 遗留的计时敏感测试**：`runtime::process::tests::immediate_grandchild_cancel_terminates_parent_and_grandchild_repeated`（Codex Phase 1 PASS 报告已记录为非阻塞 Minor）本轮未受影响，本轮全部测试运行（`cargo test`/`cargo test --workspace --all-targets --all-features`，各运行多次）均 100% 通过，未观察到该测试失败。
4. **`git remote -v` 仍为空**，与本轮修复无关，是既有的、已知的 GitHub 构建/发布环节阻塞项，不影响本地 Phase 2 remediation 的验收。

## 15. 明确声明

- **本轮未进入 Phase 3**：未新增任何真实 AI 调用、真实命令执行闭环相关代码；Action Ledger 与 Phase 4 证据结构仍然只被测试代码调用，未接入任何新的 Tauri command。
- **本轮未创建 `milestone/task-engine` 或任何新标签**：`git tag -n` 在提交后应仍只显示 `milestone/runtime-foundation`。
- **本轮未 push**：`git remote -v` 全程为空，未执行任何 push 操作。
- **本轮未 amend Phase 1 或 Phase 2 原提交**：`f91dd62`（Phase 1）与 `92d1f7e`（Phase 2 原始提交）均未被改写；本轮修复以一个新的独立提交追加在其之上。
- **本报告不代表 Phase 2 Remediation 已经 PASS。只有 Codex 独立复审给出 PASS 结论后，才允许创建 `milestone/task-engine` 并进入 Phase 3。**

## 提交

提交信息：`fix(phase2): close task recovery and concurrency safety gaps`

（提交 SHA 见本轮开发的最终回复。）
