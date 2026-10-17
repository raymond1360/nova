# WP2A 编码实现报告

- 报告时间（Asia/Shanghai）：2026-10-15 00:58
- 分支：`feature/v0.1.0-mvp`
- 起始 HEAD：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`（tag `milestone/evidence-gate`）
- 本轮结束时 HEAD：未变更（本轮全部为工作区未提交修改，未执行任何 commit/push/milestone 操作）
- 本轮授权依据：`docs/reviews/CODEX_WP2A_AMENDMENT_REVIEW10141751.md`（PASS WITH NOTES）
- 适用规范优先级：`NOVA_WP2A_FINAL_AMENDMENT10141726.md`（最高优先级补充） > `NOVA_WP2A_SELF_CHECK_SPEC10141417.md` + `NOVA_SINGLE_AI_MIGRATION_PLAN10141420.md`（基线规范）

本报告仅覆盖**本轮真实执行的编码与本地测试工作**，不代表功能已完成最终评审、不代表可发布验收。按照任务要求，所有未实际执行的验证项在下文中明确标注为 **NOT RUN**。

---

## 1. 变更文件清单与行为摘要

### 1.1 新增文件

| 文件 | 作用 |
|---|---|
| `apps/desktop/src-tauri/src/storage/self_check.rs` | `self_checks` 表的存取层：`record_self_check` / `current_self_check`（按 `rowid DESC` 取最新一条）/ `list_self_checks_for_task`（审计用）。 |
| `apps/desktop/src-tauri/src/storage/requirement_items.rs` | `confirmed_requirement_items` 表的存取层：`split_requirement_field`（按行拆分 goal/in_scope/not_in_scope/done_criteria）、`insert_confirmed_requirement_items_if_absent`、`list_confirmed_requirement_items`。 |
| `apps/desktop/src-tauri/src/core_loop/self_check_parser.rs` | 从开发 AI 进程标准输出中提取 `===NOVA_SELF_CHECK_START===...===NOVA_SELF_CHECK_END===` 结构化 JSON 块的严格解析器；任何不满足预期结构的内容统一归类为 `invalid_format`，从不做自由文本搜索。 |

### 1.2 修改文件（按模块分组）

**数据库 schema / 迁移**
- `storage/migrations.rs`：`CURRENT_SCHEMA_VERSION` 10→11；新增 `V11_SQL`：
  - `tasks` 表新增 `legacy_classification TEXT CHECK(...)` 列；
  - 新建 `confirmed_requirement_items`、`self_checks` 两张表；
  - `test_evidence` / `build_evidence` / `no_tests_confirmations` / `no_build_confirmations` 四张表新增 `result_attempt INTEGER`（历史行迁移为 `NULL`）；
  - `no_tests_confirmations` / `no_build_confirmations` 的 `UNIQUE` 键扩展为 `(task_id, result_generation, round, result_attempt)`（SQLite 的 NULL 不参与唯一性冲突，历史 NULL 行与新行天然共存）；
  - 一次性迁移期分类：仅对**迁移发生时仍处于非终态**的任务计算 `legacy_classification`（`dual_ai_evidenced` / `unknown_insufficient`），终态任务与迁移之后新建的任务永远保持 `NULL`，不会被冻结。
- `error.rs`：新增 `NovaError::StaleActionAttempt(String)` 变体，`public_message()` 单独处理。

**模型**
- `models.rs`：`Task.legacy_classification: Option<String>`；`TestEvidence` / `BuildEvidence` / `NoTestsConfirmation` / `NoBuildConfirmation` 新增 `result_attempt: Option<i64>`；新增 `ConfirmedRequirementItem`、`SelfCheck` 结构体。

**Action Ledger（CAS）**
- `storage/actions.rs`：`finish_action` 新增必传参数 `expected_attempt: i64`，通过 `UPDATE ... WHERE attempt_number = ?expected` 实现比较并交换；不匹配返回 `NovaError::StaleActionAttempt`，其它数据库/IO 错误照常以 `NovaError::Database`/`Config` 等原样传播，从不与 Stale 混同。

**完工门禁（WP2A 核心）**
- `storage/tasks.rs`：
  - 新增 `CompletionRejectReason` 枚举（`LegacyTaskFrozen` / `NoAdoptedEvidence` / `DevActionNotSucceeded` / `SelfCheckMissing` / `SelfCheckInvalidFormat` / `SelfCheckNotComplete` / `UnfinishedItemsPresent` / `BlockingIssuesPresent` / `RequirementSnapshotMissing` / `RequirementCoverageIncomplete` / `UserAcceptanceMissing`），附带人类可读 `message()` 与机器可读 `code()`；
  - 新增 `is_legacy_frozen(task)` 纯函数；
  - 新增 `Database::evaluate_completion_gate(task)`：在同一次只读调用中完成"查找已采纳的 (generation, round, dev_attempt) → 校验自检 → 校验需求覆盖 → 校验用户验收"全部检查，可被前端作为只读预检复用，也被 `transition_task` 自身在同一事务内直接复用；
  - `transition_task`：`to == Completed` 时，在 `BEGIN IMMEDIATE` 事务内调用上述门禁，结果折入 `UPDATE ... WHERE (... OR gate_ok = 1)` 的原子写入，杜绝"决定完工"与"写入完工"之间的 TOCTOU 窗口；失败时保持原状态、不写入虚假的 `task_state_events`；
  - `to IN (Queued, Running)` 且任务 `legacy_classification` 为冻结值时直接拒绝（历史任务保护）；
  - `to == AwaitingConfirmation` 时自动调用需求快照生成（见下）。

**需求快照（WP2A-0）**
- 快照仅在 `Draft → AwaitingConfirmation` 这一次合法转换中生成（`confirm_requirement` 是唯一到达该边的入口），对 `goal`/`in_scope`/`not_in_scope`/`done_criteria` 四个字段按行拆分、去空行、保序保留重复，写入 `confirmed_requirement_items`；该函数本身是"若已存在则不重复插入"的幂等操作。

**自检合同**
- `core_loop.rs` 新增 `write_self_check_for_round`：在开发/修复动作成功结束后，从**同一次调用**的进程标准输出中解析自检块并落库，不发起任何额外 AI 调用；解析失败记为 `invalid_format`，仍落库供审计，但永不满足完工门禁。

**执行归属与授权（`result_attempt` 绑定）**
- `core_loop.rs`：
  - `ActionGate` 枚举改造为携带 `TaskAction`（`Started(TaskAction)` / `AlreadySucceeded(TaskAction)` / `Uncertain`），`gate_action`/`finish_gated_action` 相应改造，`finish_gated_action` 新增 `expected_attempt` 参数；
  - `run_dev_and_test_round`：在发起 AI 调用**之前**捕获开发/修复动作自身的 `attempt_number`（`result_attempt`），调用结束后仅用该值做 CAS 与证据绑定，从不在事后重新查询；
  - `run_tests_for_round` / `run_build_for_round`：新增 `dev_result_attempt` 参数；每个写入证据的分支都先对**测试/构建动作自身**的 attempt 做 `finish_action` CAS，CAS 成功后才写入 `test_evidence`/`build_evidence`（`result_attempt` 字段固定为 `dev_result_attempt`，而非测试/构建动作自身的 attempt）；CAS 遇到 `StaleActionAttempt` 时整条路径短路为 `Stale`，不写入任何证据；其它错误原样向上传播；
  - 新增 `RoundOutcome`/`DevTestOutcome`/`SingleAiRoundOutcome`/`TestRoundOutcome`/`BuildRoundOutcome` 的 `Stale` 分支，调用方统一归入"本次无状态变更、无证据写入"处理；
  - **关键缺陷修复**：`no_tests_confirmations`/`no_build_confirmations` 的 `result_attempt` 必须绑定到**开发轮次自身**的 attempt（新增 `dev_attempt_for_round` 辅助函数），绝不能绑定到测试/构建动作自身的 attempt——原因：`reset_failed_action_for_retry` 每次"用户确认无需测试/构建"都会把测试/构建动作自身的 attempt 加一，若确认记录绑定到那个会被加一的值，则该确认在下一次幂等调用、以及 `run_tests_for_round`/`run_build_for_round` 自身按 `dev_result_attempt` 查询时都无法再被命中。此缺陷在本轮本地测试中被发现并修复，影响 `reconcile_no_build_confirmation_attempt`、`reconcile_no_tests_confirmation_attempt`、`no_build_confirmation_likely_converging`、`no_tests_confirmation_likely_converging`、`no_tests_confirmation_needed` 五处。
- `storage/no_tests_confirmation.rs` / `storage/no_build_confirmation.rs`：`record_*`/`find_*` 新增必传 `result_attempt: i64`。
- `storage/evidence.rs`：`RecordTestEvidenceRequest`/`RecordBuildEvidenceRequest` 新增必传字段 `result_attempt: i64`。
- `core_loop/documents.rs`：`no_tests_applicable_is_authorized`/`no_build_applicable_is_authorized` 新增 `result_attempt: Option<i64>` 参数，`None`（历史遗留/异常行）一律 fail-closed 为未授权——`no_build_applicable_is_authorized` 对 `legacy_unknown`（v10 迁移产生、天生没有 attempt 概念的行）保留一条结构性例外：仅当存在某个**真实、非 NULL attempt** 的确认记录命中同一 `(generation, round)` 时才算授权，绝不做 NULL 与 NULL 的比较（对应新增的 `Database::find_any_no_build_confirmation_for_round`）。
- `core_loop/evidence_overview.rs`：时间线/证据概览的状态归一化同步改为按 `result_attempt` 校验授权。

**历史任务冻结（Layer 7，5 个入口）**
- `storage/tasks.rs::transition_task`：`Queued`/`Running`/`Completed` 三个方向均已覆盖（冻结检查 + 完工门禁本身的 `is_legacy_frozen` 前置检查）；
- `core_loop.rs::run_core_loop`：不单独加检查——它唯一能推进任务的途径就是调用 `transition_task(Queued/Running)`，冻结任务在那一步就会被拒绝，错误通过 `?` 原样向上传播；
- `core_loop.rs::confirm_no_tests_applicable` / `confirm_no_build_applicable`：已在最上层加入冻结检查（复用 `CompletionRejectReason::LegacyTaskFrozen`）；
- `core_loop.rs::unblock_test_execution_for_retry`：本轮新增冻结检查，置于 `reset_failed_action_for_retry` 之前，防止被冻结任务的失败动作被重置为可重跑状态。

**最小必要前端打通**
- `core_loop.rs` 新增 `CompletionPrecheck` 结构体 + `completion_precheck()`：只读包装 `evaluate_completion_gate`，供前端在真正尝试"标记完成"之前提前展示会被拒绝的原因；新增 `list_self_checks()`：只读返回一个任务的全部历史自检记录（审计展示用）。
- `commands.rs` / `lib.rs`：新增并注册两个 Tauri 命令 `get_completion_precheck`、`list_task_self_checks`。
- 前端 `types.ts` / `stores/coreLoop.ts` / `components/CoreLoopView.vue`：新增 `CompletionPrecheck`/`SelfCheck` 类型、对应的 `loadCompletionPrecheck`/`loadSelfChecks` store action（与既有 `loadEvidenceOverview` 完全一致的"按 token 失效过期响应"模式）、以及验收区块内的"自检摘要（仅供参考）"折叠面板与完工预检失败提示横幅。未改动任何既有交互路径、未新增页面、未涉及 GitHub/新建产品/全站 UI 改版。

**产品/迁移配套文档的历史遗留问题**
- 本轮未再修改 `product/` 目录下两份 V3 规格/发布计划（它们在更早的"单 AI 范围纠正"轮次已加过纠正性声明，WP2A 未引入新的冲突），故 `git status` 中这两份文件显示为已修改属于更早轮次遗留，非本轮改动。

---

## 2. 规范条款 → 代码落点对照

| 规范编号 | 要求摘要 | 落点 |
|---|---|---|
| WP2A-0 需求快照 | 稳定拆分规则、确认时一次性生成、显式回填命令排除 Draft/终态、回填前先查状态再幂等返回、回填不解除历史冻结 | `storage/requirement_items.rs`、`storage/tasks.rs::transition_task` 的 `AwaitingConfirmation` 分支 |
| 执行上下文与 CAS | 在 `start_action` 返回时捕获 attempt，从不事后重查；完工时 CAS；仅 Stale 短路，其它错误正常传播 | `storage/actions.rs::finish_action`、`core_loop.rs::run_dev_and_test_round`/`run_tests_for_round`/`run_build_for_round` |
| 自检合同 | 同一 AI 同一次调用；结构化严格解析；持久化；"当前记录"采纳/失效规则；缺失/格式错误/未完成/被阻断/覆盖不足/归属不匹配均不可完工；不发起额外 AI 调用 | `core_loop/self_check_parser.rs`、`storage/self_check.rs`、`storage/tasks.rs::evaluate_completion_gate` |
| 执行归属与授权 | 证据绑定 dev 的 `result_attempt`；test/build 自身独立 CAS；CAS 成功才写入门禁相关证据；确认表随之更新；新 attempt 可重新授权；历史 NULL 归属永不满足新门禁 | `core_loop.rs`（见上）、`storage/evidence.rs`、`storage/no_tests_confirmation.rs`、`storage/no_build_confirmation.rs` |
| 最终后端门禁 | 唯一权威事务；重新核验执行/自检/覆盖/证据或授权/用户确认；失败保持原状态；只读预检命令 | `storage/tasks.rs::transition_task` + `evaluate_completion_gate`、`core_loop.rs::completion_precheck` |
| 历史任务保护 | 仅迁移时分类，不活分类；保护 start/retry/resume/通用转换/无测试无构建确认写入入口及完工入口；恢复记账不重跑 AI/命令 | `storage/migrations.rs` V11_SQL 的两条 `UPDATE`、`storage/tasks.rs::is_legacy_frozen`、本节"历史任务冻结"小节列出的 5 个入口 |

---

## 3. 本地测试：命令、结果、NOT RUN 清单

> 全程使用进程内 SQLite（`Database::memory()`）或临时目录下的全新文件数据库（`std::env::temp_dir()` 下随机子目录），**从未打开、修改任何真实用户数据库文件**；全部测试通过受控的 `MockAdapter`（`src/runtime/ai_adapter.rs` 内 `#[cfg(test)] pub mod mock`）模拟开发/审核 AI 的进程输出，**未执行任何真实 Codex/Claude CLI 调用，未产生任何外部费用**。

### 3.1 已实际执行并通过

| 命令 | 结果 |
|---|---|
| `cd apps/desktop/src-tauri && cargo check --lib` | 通过，仅 2 条既有的无关 dead-code 警告 |
| `cargo check --lib --tests` | 通过，同上 |
| `cargo build --lib` | 通过 |
| `cargo test --lib` | **651 passed; 0 failed; 0 ignored**（含本轮新增的 migrations v11、self_check.rs、requirement_items.rs、self_check_parser.rs 单测，以及为适配新完工门禁而补全的既有测试夹具） |
| `cd apps/desktop && npx vue-tsc --noEmit` | 通过，0 错误 |
| `npm run build`（`vue-tsc --noEmit && vite build`） | 通过，产物正常生成 |
| `npx vitest run` | **99 passed; 0 failed**（含既有 `CoreLoopView.text.test.ts` 17 项，未因本轮前端改动回归） |

`cargo test --lib` 覆盖的关键场景（对应 PM 列表中的条目，非逐字列出全部 20 项，按类别概述）：
- 迁移兼容性：v10→v11 升级后历史数据保留、`legacy_classification` 仅对迁移时非终态任务生效、终态任务与迁移后新建任务永不冻结、备份数量随新迁移步骤正确递增（10 份，覆盖迁移 2–11）；
- 需求拆分边界情况：空行/重复行/字段缺失；
- Draft 回填排除与终态拒绝、零写入保证；
- dev/test/build 各类迟到的 stale attempt 组合、CAS 成功但本应短路为 Stale 的路径；
- stale 证据 / stale 授权拒绝完工；
- 新 attempt 重新授权 + 历史 NULL 归属拒绝完工；
- 自检各类失败模式（缺失/格式错误/未完成/被阻断/未完成项非空/阻断项非空/覆盖不完整/归属不匹配）；
- 通用 `transition_task` 对 `Completed` 的绕过防止（没有专门的"绕过"入口可用，门禁已折入该函数自身）；
- 用户确认 + 新一代结果使旧确认失效 + 并发写入 user_acceptance；
- 冻结入口拒绝后状态/动作/确认/事件均未变化；
- 历史 Completed 任务保留、迁移后新任务不冻结；
- preview 与 completion 的分离（`evidence_overview`/`final_result_doc` 展示与真实完工解耦）；
- 自动修复次数上限回归（既有测试，确认未被本轮改动破坏）。

### 3.2 NOT RUN（明确标注，未执行，不得视为已验证）

| 项目 | 原因 |
|---|---|
| 启动完整 Tauri 桌面应用（`npm run tauri dev` / `cargo tauri build`） | 指令明确禁止，以避免对真实用户数据库触发自动迁移 |
| 浏览器中对新增前端交互（完工预检横幅、自检摘要折叠面板）的真实点击/渲染验证 | 同上——真实交互需要 Tauri IPC 后端，而不仅是纯前端 Vite 预览；纯前端预览下 `invoke()` 调用会直接失败，无法验证真实数据渲染 |
| 真实 Codex CLI / Claude CLI 调用 | 指令明确禁止，全部以 `MockAdapter` 替代 |
| 对已存在的真实用户数据库文件执行迁移 | 指令明确禁止；迁移测试均针对临时目录下全新创建的数据库文件 |
| Tauri 完整发布包构建（含签名/图标等） | 超出"后端 `cargo build` + 前端 `vite build`"已验证的编译正确性范围，且与真实应用启动路径overlap，为避免误触发被禁止的完整应用启动，未执行 |

---

## 4. 迁移兼容性、备份/恢复、代码回滚说明

- **迁移兼容性**：V11 迁移为纯增量（新增列/新增表/one-time UPDATE），不删除、不重命名任何既有列或表；对已存在的 `test_evidence`/`build_evidence`/`no_tests_confirmations`/`no_build_confirmations` 历史行，`result_attempt` 一律迁移为 `NULL`，不编造任何推断值。
- **备份**：按既有机制（`storage::migrations::run`），每条标记 `requires_backup: true` 的迁移（V11 已标记）在应用前会先对数据库文件做一次带北京时间戳的完整文件拷贝备份（`<app_data_dir>/backups/nova-backup-<时间戳>-<uuid>.sqlite3`），逻辑与此前 10 个迁移版本完全一致，未新增任何特殊处理。
- **回滚**：
  - 代码回滚：由于本轮所有改动均为**未提交的工作区修改**（无新 commit），回滚方式即是撤销这些工作区修改（例如 `git checkout -- <file>` 或 `git stash`），HEAD 本身未移动，无需任何额外操作。
  - 数据库回滚：若需要回退到迁移前状态，使用上述自动备份文件替换当前数据库文件即可；`user_version` 会在重新打开时被 SQLite 正常识别为迁移前的版本号，后续若不再运行本轮代码，不会再被升级。
  - 没有编写任何"反向迁移"（downgrade migration），这与此前 10 个迁移版本的既有约定一致，不是本轮引入的新限制。

---

## 5. 已知限制 / 遗留问题

1. **双 AI（v0.2.0）路径与自检的耦合**：`run_core_loop_dual_ai_v0_2`（休眠路径，未被任何 Tauri 命令引用）的开发轮次同样会调用 `write_self_check_for_round`，因此它事实上也受新完工门禁约束。本轮仅确认了这一点不会导致编译或既有测试回归（`main_flow_approve_reaches_awaiting_user_acceptance` 等既有测试已验证通过），未对双 AI 路径做任何功能性改动——双 AI 启用明确在本轮范围之外。
2. **`no_build_confirmation_needed`（只读查询）未处理"Case C"分支**：与它的姊妹函数 `no_tests_confirmation_needed`不同，`no_build_confirmation_needed` 目前只识别"动作仍为 Failed 且带标记"的情形，不识别"确认已写入、动作已重置为 Planned、但任务转 Queued 的事务因崩溃未完成"的恢复中间态。这是本轮审查代码时发现的既有行为（非 WP2A 引入），由于真正的恢复逻辑（`confirm_no_build_applicable` 本身）已经能正确处理该中间态，只是前端"是否展示提示"的只读查询存在这一个小盲点，影响面仅限于"用户体验上可能在极罕见的崩溃时间点看不到提示"，不影响数据正确性，未在本轮修复（超出 WP2A 本轮已授权范围，建议作为独立小修复排期）。
3. **前端最小打通未做任何真实浏览器/IPC 验证**：见上文 NOT RUN 清单；已完成的验证层级为 TypeScript 类型检查 + 既有 Vitest 文本断言套件，未包含真实 Tauri 运行时下的交互测试。
4. **测试覆盖的"并发收敛"类测试仍基于内存数据库/单进程多线程模拟**，不是跨进程真实并发，与此前已有的并发测试约定一致，非本轮新增限制。

---

## 6. Codex 评审索引

| 文件 | 结论 | 说明 |
|---|---|---|
| `docs/reviews/CODEX_WP2A_SPEC_REREVIEW10141259.md` | — | 方案迭代过程记录 |
| `docs/reviews/CODEX_WP2A_SPEC_TARGETED_REVIEW10141332.md` | — | 方案迭代过程记录 |
| `docs/reviews/CODEX_WP2A_SPEC_CLOSURE_REVIEW10141407.md` | — | 方案迭代过程记录 |
| `docs/reviews/CODEX_WP2A_SPEC_FINAL_CLOSURE_REVIEW10141613.md` | — | 方案迭代过程记录 |
| `docs/reviews/CODEX_WP2A_AMENDMENT_REVIEW10141751.md` | **PASS WITH NOTES** | 本轮编码的直接授权依据 |

本报告交付后，按指令要求，不再继续推进，等待 Codex 对本轮**代码实现**（而非方案文档）的独立评审。

---

## 7. 停止声明

本轮已完成：WP2A-0 与 WP2A 主体的真实代码实现、后端全量编译与单元测试（651/651 通过）、前端类型检查与生产构建（通过）及既有前端单测（99/99 通过）、两份交付文档。

按指令要求，本报告 **不构成最终验收结论**，不代表"功能已最终评审通过"或"可发布"；本轮未执行 commit / push / 创建 milestone；现在停止，交由 Codex 进行独立评审。
