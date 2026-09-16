# Phase 2 开发报告：任务状态机、本地存储与中断恢复

生成时间：2026-09-16 09:50（北京时间）

## 1. Phase 1 Codex PASS 报告路径

`D:\nova-review\CODEX_PHASE1_FINAL_ACCEPTANCE_REVIEW09160851.md`

## 2. Phase 1 PASS SHA

`f91dd62ad12f934ad40ccff87c2a966df24fc620`

## 3. milestone/runtime-foundation 验证

Phase 2 开始前，该 tag 不存在，按要求新建：

```
git tag -a milestone/runtime-foundation f91dd62ad12f934ad40ccff87c2a966df24fc620 -m "Accepted Phase 1 runtime foundation"
```

验证：

```
$ git rev-parse milestone/runtime-foundation
b84902901e99479f33b9d1123d2fd333c4a138d4

$ git show --no-patch --format=fuller milestone/runtime-foundation
tag milestone/runtime-foundation
...
commit f91dd62ad12f934ad40ccff87c2a966df24fc620
    fix(runtime): isolate test helper and validate process input
    ...
```

tag 对象 SHA 为 `b84902901e99479f33b9d1123d2fd333c4a138d4`，其指向的提交为 `f91dd62ad12f934ad40ccff87c2a966df24fc620`，与 Phase 1 PASS SHA 完全一致。未 push 该 tag（当前无 remote）。

## 4. Phase 2 开始前 Git 状态

- 分支：`feature/v0.1.0-mvp`
- HEAD：`f91dd62ad12f934ad40ccff87c2a966df24fc620`
- `git status --short --untracked-files=all`：无输出（干净）
- `git status --porcelain=v2 --untracked-files=all`：无输出（干净）
- `git log --oneline --decorate -12`：显示 Phase 1 三轮提交历史，HEAD 位于预期提交
- `git remote -v`：无输出（未配置 remote，如实记录，未自行添加）
- 未发现任何 Word 生成的 `~$` 临时文件（已用 `find /d/nova -iname '~$*'` 全目录核实，无匹配）
- 未发现 HEAD 不一致或未知提交

## 5. 现有数据库审计

审计文件：`db.rs`、`models.rs`、`lib.rs`、`commands.rs`。

发现：

- `Database::migrate()` 此前直接对 `providers`/`app_settings`/`conversations`/`messages`/`request_logs` 五张表执行 `CREATE TABLE IF NOT EXISTS`，**完全没有版本管理**（未使用 `PRAGMA user_version`），也没有备份机制。
- 无项目、任务、状态机、动作、恢复、证据相关的任何表。
- `AppState` 持有唯一一个 `Arc<Mutex<Database>>`，所有 Tauri command 通过 `with_db` 辅助函数串行访问——这意味着单进程内所有命令已经天然互斥，真正的并发场景只可能来自“同一 SQLite 文件被两个独立连接打开”（例如用户误开两个 Nova 进程）。
- SDKMAX 聊天/Provider/设置数据模型与 Phase 2 新增数据完全独立，未见任何命名冲突。

结论：Phase 2 的 schema 变更必须在**不破坏现有 SDKMAX 聊天/设置/Provider 数据**的前提下，把无版本的既有 schema 纳入正式的版本化迁移体系，并新增 Phase 2 所需的表。

## 6. Schema 设计

新增 15 张表（见第 7 节版本号）：

- `nova_projects`：项目注册（含 `normalized_path` 唯一索引，用于防止同一目录被重复注册）
- `project_memory`：每个项目恰好一行的结构化记忆
- `project_memory_revisions`：记忆字段变更的追加式修订历史
- `tasks`：任务主表（含状态字段）
- `task_state_events`：状态转换审计流水
- `task_actions`：动作账本（幂等性核心，`UNIQUE(task_id, idempotency_key)`）
- `recovery_checkpoints`：中断恢复检查点
- `timeline_events`／`command_evidence`／`git_evidence`／`file_change_evidence`／`test_evidence`／`user_acceptance`／`generated_documents`／`evidence_refs`：Phase 4 证据结构预留

未直接照抄用户建议的表名而不加审视——例如“是否为最后活动项目”没有作为 `nova_projects` 的冗余布尔列存储，而是通过 `app_settings.last_active_project_id` 单一事实来源联查得出（`Project.is_last_active` 在读取时计算），避免出现“多行同时标记为最后活动项目”的不一致状态。

完整表结构见 `apps/desktop/src-tauri/src/storage/migrations.rs` 中的 `PHASE2_SQL` 常量。

## 7. Migration 版本

- 版本 1（`requires_backup = false`）：既有 Phase 1 baseline schema（`providers`/`app_settings`/`conversations`/`messages`/`request_logs`），原样保留为 `CREATE TABLE IF NOT EXISTS`，对已有数据库是无操作的幂等确认，因此不需要备份。
- 版本 2（`requires_backup = true`）：Phase 2 新增的全部 15 张表。
- `CURRENT_SCHEMA_VERSION = 2`。
- 使用 `PRAGMA user_version` 追踪，迁移器按顺序应用每个未应用的版本，每步一个独立事务。
- 版本高于程序支持版本（`user_version > CURRENT_SCHEMA_VERSION`）时返回 `MigrationError::VersionTooNew`，不做任何写操作，不擅自降级——已用专门测试验证。

## 8. 数据库备份机制

- 使用 SQLite 官方在线备份 API（`rusqlite` 的 `backup` feature，`Connection::backup`），而非裸文件复制，避免在日志/WAL 文件处于写入中间状态时复制出不一致的快照。
- 备份先写入 `<app_data_dir>/backups/.tmp-<uuid>.sqlite3` 临时文件，再校验（大小非零 + `PRAGMA integrity_check(1)` 返回 `ok`），最后 `fs::rename` 原子改名为正式文件 `nova-backup-<北京时间YYYYMMDDHHMMSS>-<uuid>.sqlite3`。
- 校验失败或备份 API 调用失败时，删除临时文件、直接返回错误，**不执行本次迁移**（`MigrationError::BackupFailed`）。
- 只有版本 2 这类非平凡迁移会触发备份；版本 1（baseline 确认）不触发。
- 从不删除历史备份文件；每次调用的文件名包含北京时间 + 随机 UUID，不会覆盖已有备份。
- 迁移失败时整条语句在同一事务内执行 `ROLLBACK`，原数据库文件从未被截断或覆盖，迁移前生成的备份仍然完整保留在磁盘上。
- 测试覆盖：全新数据库初始化、旧版本升级不丢数据、多版本顺序升级、重复启动不重复迁移/不重复备份、非平凡迁移触发备份、备份文件名含北京时间戳且每次运行唯一、中文/空格路径下备份可用、迁移失败时事务回滚且原库仍可用、备份先于失败步骤写入并在失败后仍存在、损坏数据库文件不会导致 panic 或误建备份目录。

## 9. 项目注册

`storage::projects`：

- `create_project`：校验 `source_type`（`local`/`github`/`template`，非法值在写入前被拒绝，附带清晰中文错误，而不是让 SQLite 的 `CHECK` 约束报出原始错误）；对 `local_path` 做 `fs::canonicalize` + 全小写规范化得到 `normalized_path`，`UNIQUE` 约束 + 应用层预检查双重防止同一目录被注册为两个项目。
- Windows 大小写规则：规范化路径统一转小写比较，测试用不同大小写拼写同一目录确认被判定为重复。
- 中文、空格、长路径：均已用真实临时目录测试通过。
- 最后活动项目：单一来源存于 `app_settings.last_active_project_id`，不作为每行冗余字段存储，切换项目、项目已删除（悬空指针安全返回 `None`）均有测试。
- 目录消失/被移动：`check_project_directory_state` 只读检查（不创建/不写入/不删除），移动后的目录同样安全返回 `Missing`（不猜测新位置）。
- 本阶段未执行任何 GitHub Clone 操作，`source_type = "github"` 仅作为枚举值预留。

## 10. 项目记忆

`storage::memory`：

- `project_memory` 每个项目恰好一行，创建项目时自动建立空记忆行。
- 字段覆盖：产品目标、用户要求、已确认决定、待确认问题、暂不处理、已知问题、历史任务摘要、当前未完成任务 ID、恢复位置、最近使用的 AI 类型、项目规则索引、许可证摘要、安装/运行摘要。
- `update_project_memory_field` 只接受一份白名单字段（`MEMORY_FIELDS`），未知字段名直接拒绝，不静默忽略；写入前后值相同则不产生新的 revision（避免重复保存刷屏）。
- 每次实际变更在同一事务中写入 `project_memory_revisions`（含 `old_value`/`new_value`/`changed_by`/`reason`），满足“用户明确修改决定时保留变更历史”。
- 记忆完全按 `project_id` 隔离查询，两个项目的记忆/修订历史互不可见（已用跨项目测试验证）。
- 结构化上下文摘要 `project_context_summary` 已实现（供未来对接真实开发 AI 使用），仅拼接当前项目字段，跨项目内容不可能泄漏；本阶段未调用任何真实 AI，因此该函数目前只被自身测试使用。
- 未保存任何“私有思维链”字段——schema 中不存在此类列，是设计层面直接杜绝，而不是运行时过滤。

## 11. 任务状态机图 / 转换表

14 个状态：`draft` / `awaiting_confirmation` / `confirmed` / `queued` / `running` / `verifying` / `safe_fixing` / `preview_ready` / `awaiting_user_acceptance` / `completed` / `needs_user_action` / `interrupted` / `failed` / `cancelled`。

完整合法转换表（46 条边，`storage::tasks::LEGAL_TRANSITIONS`）：

```
draft -> awaiting_confirmation, cancelled
awaiting_confirmation -> confirmed, draft, cancelled
confirmed -> queued, cancelled
queued -> running, cancelled, interrupted
running -> verifying, needs_user_action, interrupted, failed, cancelled
verifying -> safe_fixing, preview_ready, needs_user_action, interrupted, failed, cancelled
safe_fixing -> verifying, needs_user_action, interrupted, failed, cancelled
preview_ready -> awaiting_user_acceptance, safe_fixing, interrupted, cancelled
awaiting_user_acceptance -> completed, needs_user_action, safe_fixing, interrupted, cancelled
needs_user_action -> running, verifying, safe_fixing, queued, cancelled, interrupted
interrupted -> queued, needs_user_action, cancelled
failed -> awaiting_confirmation, cancelled
completed -> （无出边，终态）
cancelled -> （无出边，终态）
```

未加入任何“first_review”/“second_review”状态或第二 AI 审核相关转换——`verifying` 是单一开发 AI 自身的验证阶段，不是复审。

## 12. 合法 / 非法转换

- 合法转换：由 `is_legal_transition` 严格按上表判断，`transition_task` 拒绝任何不在表中的转换。测试 `every_documented_legal_transition_is_accepted` 遍历全部 46 条边逐一验证。
- 非法转换：测试 `a_representative_sample_of_illegal_transitions_is_rejected` 覆盖代表性非法样本（如 `draft->completed`、`running->completed`、`cancelled->running`、`completed->running` 等）。
- `completed` 只能从 `awaiting_user_acceptance` 进入：结构上只有一条入边；并在 `transition_task` 中额外要求 `user_acceptance` 表存在 `accepted=1` 记录，否则拒绝（`completed_requires_a_prior_user_acceptance_record`）。
- `running` 不能直接进入 `completed`：结构上无此边（`running_cannot_go_directly_to_completed`）。
- `failed`/`cancelled` 不能自动进入 `completed`：两者均无到 `completed` 的边，无论 actor 是谁（`failed_and_cancelled_can_never_reach_completed`）。
- `cancelled` 不能自动恢复为 `running`：`cancelled` 是完全终态，无任何出边，结构上杜绝（同一测试内验证）。
- `interrupted` 恢复需要判断上一步是否可安全重试：`interrupted` 只能进入 `queued`/`needs_user_action`/`cancelled`，不能直接跳回 `running`（`interrupted_recovery_requires_judgement_not_a_direct_hop_to_running`）。
- 不允许无限自动修复循环：`assert_fix_cycle_allowed` 在 `actor != user` 且非恢复场景下累计 3 次自动进入 `safe_fixing` 后拒绝第 4 次，强制转入 `needs_user_action`；用户显式请求的修复不受此限制（两个专门测试）。
- 范围变更必须进入下一轮或重新确认：`preview_ready`/`awaiting_user_acceptance` 均无直接到 `running` 的边，防止在不重新确认的情况下静默扩大当前任务范围。

## 13. Action Ledger（动作账本）

`storage::actions`，`task_actions` 表：

- `plan_action`：记录意图，同一 `(task_id, idempotency_key)` 重复调用是无操作（返回已有行）；即使在真实并发（见第 21 节）下，`UNIQUE` 约束冲突也被捕获并转换为“读回已有行”而不是报错。
- `start_action`：非只读动作若上次结果是 `succeeded` 或 `uncertain`，拒绝启动；只读动作（`is_read_only=true`）不受限制，可重复重试。
- `finish_action`：只接受 `succeeded`/`failed`/`cancelled` 三种终态，且只能从 `started` 状态调用。
- `acknowledge_uncertain_action`：唯一能让 `uncertain` 变回 `planned` 的路径，同时递增 `attempt_number`（这是真正授权“新一次尝试”的时刻）。
- `mark_started_actions_uncertain`：崩溃恢复扫描，只把仍处于 `started` 的行转为 `uncertain`，多次调用是幂等的（第二次起没有行可转换）。

本阶段**未实际执行任何真实动作**，仅建立并测试上述控制机制——`plan_action`/`start_action`/`finish_action`/`acknowledge_uncertain_action`/`find_action`/`list_actions_for_task` 因此尚未接入任何 Tauri command（`mark_started_actions_uncertain` 是唯一例外，被 `storage::recovery` 在启动时调用）。

## 14. 幂等策略

1. 相同 idempotency key 的非只读动作不能重复成功执行：`start_action` 对 `succeeded` 状态直接拒绝（`a_non_read_only_action_cannot_succeed_twice_under_the_same_key`）。
2. 应用崩溃时 `started` 但无完成记录的动作恢复为 `uncertain`：`mark_started_actions_uncertain`（`a_started_action_becomes_uncertain_after_a_simulated_crash`）。
3. `uncertain` 动作不自动重试：`start_action` 对非只读 `uncertain` 动作拒绝，必须先经 `acknowledge_uncertain_action`（`uncertain_actions_are_never_auto_retried`）。
4. 只读动作可安全重试：`is_read_only=true` 时 `succeeded`/`uncertain` 均可重新 `start_action`（`read_only_actions_may_be_retried_freely_even_after_success`）。
5. 文件写入、依赖安装、Git 修改等默认视为非幂等/高风险：命令层约定所有真实副作用动作都必须以 `is_read_only=false` 调用（本阶段无真实调用点，留待 Phase 3 接入时遵守）。

## 15. 中断恢复

`storage::recovery::run_startup_recovery`，在 `lib.rs` 的 `setup()` 中、任何 Tauri command 可被调用之前同步执行：

1. 全局扫描并将 `started` 的动作转为 `uncertain`。
2. 读取最后活动项目（`app_settings.last_active_project_id`），若无则直接返回 `None`（无需恢复）。
3. 若项目目录缺失/不可用：返回带 `has_risk=true` 的摘要，不触碰任何任务状态（“不猜测目录去向”）。
4. 找到项目当前未完成任务（`unfinished_task_for_project`：状态不在 `completed`/`cancelled`/`failed`）。
5. 若任务处于 `running`/`verifying`/`safe_fixing`（可能有真实动作在途）：通过标准 `transition_task` 路径（`actor=system, is_recovery=true`）转为 `interrupted`，并写入 `recovery_checkpoints` 记录；**不重新执行任何命令、不重新调用 AI、不重复任何文件写入**——这一步只做状态转换和记录。
6. 生成普通用户可读的恢复摘要（中文），包含：上次做到哪一步、是否存在风险（`uncertain` 动作数>0 时标记风险）、可继续/查看结果/取消。

幂等性验证：`repeated_recovery_scans_are_idempotent` 反复调用三次，`task_state_events`/`recovery_checkpoints` 行数保持不变（第二次起任务已是 `interrupted`，不满足"in-flight"条件，直接复用已有 checkpoint，不重复写入）。`recovery_never_re_executes_a_destructive_action` 验证恢复过程中已开始的动作只会变为 `uncertain`，绝不会被静默视为成功或重新启动。

## 16. 最后活动项目

- `set_last_active_project`：唯一写入点，若项目不存在直接拒绝（不会让悬空 ID 变成"最后活动"）。
- `last_active_project`：读取；若指向的项目已被删除，安全返回 `None`（`a_stale_last_active_pointer_to_a_deleted_project_returns_none_not_an_error`），不报错、不崩溃。
- 不自动创建新目录、不自动删除旧项目——`create_project`/`create_task` 等写操作均需显式调用触发。
- 并发切换：两个独立连接同时调用 `set_last_active_project` 指向不同项目，最终读取结果在两次读取之间保持一致（`concurrent_last_active_project_switches_converge_to_exactly_one_project`）。

## 17. Phase 4 证据结构预留

`storage::evidence`（`#![allow(dead_code)]`，模块级注释说明：本阶段仅建立 schema/模型/repository 接口和测试，未接入任何真实 AI/命令执行/完成门禁，`user_acceptance` 相关两个函数是唯一被 Phase 2 任务状态机实际调用的部分）：

- `timeline_events`／`command_evidence`／`git_evidence`／`file_change_evidence`／`test_evidence`／`generated_documents`／`evidence_refs`：完整 schema + repository 函数 + 独立测试，均未接入任何 Tauri command。
- `user_acceptance`：**是本阶段真正被使用的部分**——`record_user_acceptance` 由专门的 Tauri command 暴露，明确要求只能由前端"我已经看过，符合需要"这一确认动作触发；`transition_task` 的 `completed` 门禁直接查询该表判定是否放行。
- 所有证据写入函数在插入前校验 `task_id` 归属的 `project_id` 与传入的 `project_id` 一致，不一致直接拒绝（`evidence_referencing_a_task_from_a_different_project_is_rejected`），防止跨项目引用。
- 命令/测试文本类证据在写入前经过 `redact_sensitive_text` 脱敏（`command_evidence_text_is_redacted_before_storage`）。
- 未生成任何"已通过"的伪造证据——所有 repository 函数只会原样存储调用方传入的值，本阶段调用方只有测试代码。

## 18. 修改文件

```
M  apps/desktop/src-tauri/Cargo.toml
M  apps/desktop/src-tauri/src/commands.rs
M  apps/desktop/src-tauri/src/db.rs
M  apps/desktop/src-tauri/src/error.rs
M  apps/desktop/src-tauri/src/lib.rs
M  apps/desktop/src-tauri/src/models.rs
A  apps/desktop/src-tauri/src/storage/mod.rs
A  apps/desktop/src-tauri/src/storage/migrations.rs
A  apps/desktop/src-tauri/src/storage/projects.rs
A  apps/desktop/src-tauri/src/storage/memory.rs
A  apps/desktop/src-tauri/src/storage/tasks.rs
A  apps/desktop/src-tauri/src/storage/actions.rs
A  apps/desktop/src-tauri/src/storage/recovery.rs
A  apps/desktop/src-tauri/src/storage/evidence.rs
A  apps/desktop/src-tauri/src/storage/concurrency_tests.rs
A  docs/release-v0.1.0/PHASE2_TASK_STATE_RECOVERY_REPORT09160950.md（本报告）
```

未修改任何前端文件（`types.ts`/`nova.ts`/`App.vue`）——现有 UI 无需改动即可通过全部后端测试，按照 Phase 2 范围说明，为避免不必要地扩大范围而未触碰前端；`tauri.conf.json`、`runtime/*`、`security.rs`、`credential.rs`、`api.rs`、`validation.rs` 均未改动。

## 19. 新增测试

Rust 新增 73 个测试，按模块：

| 模块 | 数量 | 覆盖内容 |
|---|---|---|
| `storage::migrations` | 12 | 全新初始化、幂等重跑、旧库升级不丢数据、多版本顺序升级、备份触发、备份文件名唯一、中文/空格路径备份、拒绝高版本降级、失败回滚、失败前备份仍存在、损坏库安全失败、重复迁移不重复备份 |
| `storage::projects` | 9 | 创建/读取、重复目录拒绝、大小写重复拒绝、中文空格路径、最后活动项目切换、目录消失、目录移动、悬空指针安全、未知来源类型拒绝 |
| `storage::memory` | 7 | 空记忆初始化、写入产品目标、决定变更生成修订、重复写入不产生冗余修订、未知字段拒绝、跨项目隔离、上下文摘要不泄漏 |
| `storage::tasks` | 13 | 初始状态与事件、全部合法转换、代表性非法转换、running 不能直达 completed、completed 需要用户确认、failed/cancelled 不能进入 completed、interrupted 恢复判断、自动修复次数上限、用户发起的修复不受限、范围变更不可绕过、状态与事件同事务、事务失败两者均不变、未完成任务查询忽略终态 |
| `storage::actions` | 8 | 重复规划无操作、成功后不能重复执行、崩溃变 uncertain、uncertain 不自动重试、只读动作可重试、多次恢复扫描不重复、finish 拒绝非终态、动作列表排序 |
| `storage::recovery` | 8 | 无最后活动项目、三种在途状态均恢复为 interrupted、重复扫描幂等、不重新执行破坏性动作、最后活动项目正确呈现、目录缺失安全提示、恢复摘要内容准确、非在途未完成任务也给出摘要 |
| `storage::evidence` | 11 | 证据归属项目/任务、跨项目引用拒绝、命令文本脱敏、用户确认唯一关联、无确认不产生记录、时间线按项目隔离、非法变更类型拒绝、Git 证据往返、测试证据计数、无任务的生成文档、证据引用关联 |
| `storage::concurrency_tests` | 5 | 两连接并发转换同一任务仅一个胜出、并发规划相同 idempotency key 不产生重复行、8 线程并发写入在 busy_timeout 下全部安全完成不 panic、并发切换最后活动项目收敛一致、恢复扫描与普通读并发不互相破坏 |

## 20. 并发测试

见上表 `storage::concurrency_tests`（5 个），均使用真实文件级 SQLite 数据库 + 真实 OS 线程（而非仅测试单连接内的 `Arc<Mutex>` 场景，因为单进程内所有 Tauri command 已经通过 `AppState.db` 天然串行化；这组测试专门覆盖两个独立连接同时打开同一数据库文件的场景——最贴近"用户误开两个 Nova 进程"）。测试中发现并修复了两处真实的并发正确性问题：

1. `transition_task` 原先"先读状态、再开事务写入"存在 TOCTOU 竞态，可能让两个并发请求都判定转换合法并都写入历史事件。修复为条件更新 `UPDATE tasks SET state=? WHERE id=? AND state=?from`，`rows_affected=0` 时判定为并发冲突并中止，不写入历史事件。
2. `plan_action` 原先"先查后插"存在竞态，并发场景下第二个请求的 INSERT 会因 `UNIQUE(task_id, idempotency_key)` 约束报错。修复为捕获该约束冲突并转为读回已有行，保持"规划操作本身是幂等的"这一契约在真实并发下依然成立。

另外为 `Database::open` 增加了 `PRAGMA busy_timeout = 5000`（仅文件数据库，内存数据库不涉及），使短暂的写锁竞争（如迁移期间）表现为自动等待重试，而不是立即抛出 `SQLITE_BUSY` 错误。

## 21. 迁移失败恢复测试

见第 8 节；具体测试：`a_failing_migration_step_rolls_back_and_leaves_the_database_openable`（人为制造 `CREATE TABLE` 冲突，验证 `user_version` 停留在最后成功版本、原数据库仍可正常查询）、`backup_is_taken_before_the_failing_step_so_it_survives_the_failure`（验证失败发生前已完成的备份文件在失败后依然完整存在于磁盘）、`corrupted_database_file_fails_cleanly_without_touching_backups`（用非法字节构造损坏文件，验证不会创建 `backups` 目录、不会 panic）。

## 22. 七项验证结果

| # | 命令 | 工作目录 | 退出码 |
|---|------|----------|--------|
| 1 | `cargo fmt --check` | `apps/desktop/src-tauri` | 0 |
| 2 | `cargo clippy --all-targets --all-features -- -D warnings` | `apps/desktop/src-tauri` | 0 |
| 3 | `cargo test` | `apps/desktop/src-tauri` | 0（198 passed） |
| 4 | `cargo build --release` | `apps/desktop/src-tauri` | 0 |
| 5 | `npm test` | `apps/desktop` | 0（13 passed） |
| 6 | `npm run build` | `apps/desktop` | 0 |
| 7 | `npm run tauri -- build` | `apps/desktop` | 0，`Built application at: ...\target\release\nova.exe` |

## 23. Rust / 前端测试数量

- Rust：`cargo test` 库单元测试 **198 passed / 0 failed / 0 ignored**（Phase 1 既有 125 个全部保留通过 + 本轮新增 73 个）；`main.rs` 0 tests；doc-tests 0 tests。
- 前端：`npm test`（vitest）3 个测试文件，**13 tests passed / 0 failed**（未新增前端测试，未修改任何前端文件）。

## 24. Skipped / Ignored

无。已用 `grep -rn "#\[ignore\]" src/ tests/` 核实 Rust 侧无匹配；前端 `.skip(`/`.todo(`/`describe.skip`/`it.skip`/`test.skip` 同样无匹配。

## 25. 正式 Release 二进制

```
target/release/nova.exe   ← 唯一顶层可执行文件
```

`target/release` 树下不存在任何 `nova_test_helper` 相关文件；`target/release/bundle` 目录不存在（`tauri.conf.json` 的 `bundle.active: false` 未改动，与 Phase 1 一致）。

## 26. 精确提交 SHA

见本报告末尾"提交后"章节（提交在本报告写入之后发生）；最终 SHA 见本轮开发的最终回复。

## 27. 回滚方法

```
git revert <Phase 2 提交SHA> --no-edit
```

（本轮为单一提交，见第 28 节说明；如需彻底回退，也可使用 `git reset --hard f91dd62ad12f934ad40ccff87c2a966df24fc620`，但仅在明确需要放弃本轮全部工作时使用，日常回滚应优先 `git revert`。）

## 28. 提交策略说明（为何是一个提交而不是两个）

按照要求，本应尝试拆分为：

- 提交1 `feat(storage): add project memory and durable task recovery`
- 提交2 `feat(evidence): add phase 4 evidence storage foundation`

但实际审计代码依赖关系后发现：`storage::evidence` 中的 `record_user_acceptance`/`user_acceptance_for_task` 并非纯粹的"Phase 4 预留"，而是 Phase 2 任务状态机 `completed` 门禁的硬依赖（`transition_task` 直接查询 `user_acceptance` 表判断是否放行），同时也被 Phase 2 的 `record_user_acceptance` Tauri command 直接调用。若强行拆分，要么把 `user_acceptance` 相关代码从 `evidence.rs` 中抽出单独成文件（为了满足提交边界而进行的、与功能无关的纯粹文件切割，增加了不必要的改动面），要么让"提交1"依赖"提交2"尚未提交的代码（导致提交1单独检出时编译失败，破坏"每个提交都应可独立回滚"的初衷）。因此按照说明中"如果现有架构使两个提交不可安全拆分，可以使用一个提交，但必须在报告中解释"的规定，本轮使用**一个提交**，建议信息：

```
feat(storage): add project registry, memory, task state machine, and phase 4 evidence foundation
```

该提交仍然是完整、自包含、可用单次 `git revert` 整体回滚的一次变更。

## 29. Git Remote 状态

未添加任何 git remote。本轮开发自始至终仅在本地仓库操作，`git remote -v` 输出为空。

## 30. 最终 Git 状态

提交完成后：

- `git status --short --untracked-files=all`：应为空
- `git status --porcelain=v2 --untracked-files=all`：应为空

（具体核实结果见本轮最终回复。）

## 31. 已知限制

1. `runtime::process::tests::immediate_grandchild_cancel_terminates_parent_and_grandchild_repeated` 是 Phase 1 遗留的计时敏感测试（Codex 在 Phase 1 PASS 报告中已记录为非阻塞 Minor，建议改用就绪信号而非固定 sleep）。本轮开发期间该测试单独运行/多次全量重跑均稳定通过；一次在与另外 197 个测试并发运行时出现瞬时失败，与本轮改动无关（`runtime::process` 模块本轮未修改），重跑后稳定通过。不在本轮范围内修复，以遵守"不修改已关闭功能范围之外内容"的约束。
2. `project_memory.current_unfinished_task_id` 字段已建立但本阶段未被任何代码主动维护同步（"当前未完成任务"改由 `unfinished_task_for_project` 实时查询 `tasks` 表得出，更不容易与实际状态脱节）；该字段保留供未来如需要"记忆快照"式而非实时查询式的展示时使用。
3. `storage::evidence` 中除 `user_acceptance` 外的全部函数（`timeline_events`/`command_evidence`/`git_evidence`/`file_change_evidence`/`test_evidence`/`generated_documents`/`evidence_refs`）尚未接入任何 Tauri command——这是刻意的范围控制（Phase 4 完整界面不在本阶段），仅通过其自身单元测试验证正确性。
4. Action Ledger（`plan_action`/`start_action`/`finish_action`/`acknowledge_uncertain_action`）本阶段同样未接入任何 Tauri command——本阶段不执行任何真实动作，Phase 3 引入真实执行循环时需要显式接入这些控制点。
5. 并发测试聚焦于"两个独立 SQLite 连接访问同一文件"这一真实可能发生的场景，未测试极端场景（如数百并发连接、磁盘 I/O 故障注入）；这些超出 Phase 2 "建立并测试控制机制"的范围。

## 32. Phase 3 / Phase 4 声明

**本轮未开始 Phase 3 真实 AI 执行闭环**：Action Ledger 建立并测试了幂等控制机制，但没有任何代码路径实际启动过 Codex CLI/Claude Code CLI 或执行真实命令；`ai_kind` 字段目前只是任务的一个可空元数据字段。

**本轮未提前宣称 Phase 4 已完成**：Phase 4 证据结构仅完成 schema、模型、repository 接口和测试这一"预留"范围，未实现完整证据中心界面，未接入真实命令/Git/测试执行采集，未产生任何"已通过"的模拟证据。

## 33. Codex 复审材料清单

- 本报告：`docs/release-v0.1.0/PHASE2_TASK_STATE_RECOVERY_REPORT09160950.md`
- Phase 1 PASS 报告（供对照）：`D:\nova-review\CODEX_PHASE1_FINAL_ACCEPTANCE_REVIEW09160851.md`
- 新增/修改的全部源文件（见第 18 节）
- 本轮提交的完整 diff：`git show <Phase 2 提交SHA>`
- 七项验证命令均可在 Codex 复审环境中独立重新执行（命令与工作目录见第 22 节）
- `milestone/runtime-foundation` tag（Phase 1 基线）与本轮提交的父子关系：`git log --oneline milestone/runtime-foundation..<Phase 2 提交SHA>`

**本报告不代表 Phase 2 已经 PASS。只有 Codex 独立审核返回 PASS 结论后，才允许建立 `milestone/task-engine` 并进入 Phase 3。**
