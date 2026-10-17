# WP2A 整改独立复审（Claude Code）

生成时间：2026-10-15 14:57 Asia/Shanghai（实际UTC 06:57）。项目：D:\nova。审核人：Claude Code（本轮独立复审方；Codex为本轮开发/整改方）。

## 唯一结论：PASS

CODEX_WP2A_IMPLEMENTATION_REVIEW10151053.md 列出的 3 Blocker + 6 Major + 3 Minor + 1 Suggestion，经本轮源码级独立核验与真实自动化测试复现，**全部关闭**，未发现新增回归。真实桌面UI/IPC、安装器运行、真实AI CLI、真实用户库仍为 **NOT RUN**（与原审核、整改报告一致，非本轮新增缺口），不阻塞本轮代码验收结论，但在生产发布前仍需补验（见第五节）。本结论仅覆盖代码正确性验收，不构成发布批准，不建立里程碑。

## 现场与交付版本核验

- 分支：`feature/v0.1.0-mvp`；HEAD：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`；`milestone/evidence-gate^{}` 解引用一致。未切换/清理/回退/stash/整文件恢复；保留全部既有修改与未跟踪文件。
- 独立重算 `audit/checkpoints/wp2a-remediation-20261015-1112/source-remediation.patch` 的 SHA256：`30315bac6f0f0bcd432c72facccafa6d772608b70a0fb0319f4278f9323778ed`，与报告给出值及 `delivery-manifest.json` 记录值一致。
- 对 `delivery-manifest.json` 列出的 20 个交付源码/测试文件逐一重算 SHA256，**全部与当前工作区文件字节一致**——本轮审核的是实际交付版本，不是旧报告快照。
- `git apply --reverse --check source-remediation.patch` 本机独立重跑：退出码 0，补丁边界与当前 diff 一致（仅检查，未执行）。
- NSIS 安装包 `apps/desktop/src-tauri/target/release/bundle/nsis/Nova_0.1.0_x64-setup.exe` 独立重算 SHA256：`40b18ce4eb8828cc47bca50005bc4723be0719e64467b1879e2fadae21814df1`，与 `delivery-manifest.json`/整改报告一致；`tauri-package-final.log` 显示真实 `npx tauri build` 流程（vue-tsc→vite→cargo release→makensis），非伪造日志。

## 逐项关闭表

| 编号 | 问题 | 本轮判定 | 核验方式 |
|---|---|---|---|
| B1 | 自检生产输入合同缺失，Mock硬编码答案 | **已关闭** | 直读 `core_loop.rs:3523-3582` `write_task_package`：现列出 `confirmed_requirement_items` 的 `field:index`+文本、完整stdout合同、枚举、覆盖要求，无快照时 `Err(Config)`；`ai_adapter.rs:805-820` Mock 从 `request.package_path` 真实文件解析 `===NOVA_REQUIREMENT_ITEMS_START/END===` 之间的JSON并据此生成coverage，非默认`goal:0`；缺合同时输出 `"missing or incomplete self-check contract"` 导致门禁拒绝。测试 `wp2a_b1_*`、`wp2a_self_check_rejection_matrix_*`（18类拒绝）均通过 |
| B2 | 门禁在采纳不匹配后回退搜索旧代/另一轮 | **已关闭** | `storage/tasks.rs:405-446` `evaluate_result_gate`：先调用 `adopted_passing_generation_round` 一次性冻结 `(generation, adopted_round)`，随后 dev/test/build/自检/覆盖/确认检查全部基于该固定 tuple，不含任何回退到其他 generation/round 的分支。回归测试 `wp2a_b2_fixed_adoption_cannot_fall_back_to_older_generation_or_round` 精确复现原审核场景（generation1/round1 attempt99 不匹配当前attempt1；generation0/round2 attempt1 本可“救活”）并断言 `transition_task` 失败、状态/事件数不变 |
| B3 | 事务内门禁使用事务前Task，当前代确认可被绕过 | **已关闭** | `storage/tasks.rs:544-547`：`self.task(task_id)?` 发生在 `BEGIN IMMEDIATE` **之后**（同一闭包内），门禁基于这次重读的 `task.result_generation` 评估。回归测试 `wp2a_b3_two_connection_generation_aba_rechecks_current_acceptance` 用真实文件型SQLite+两个独立连接+Barrier，让主线程完整走 `SafeFixing→Verifying→PreviewReady→AwaitingUserAcceptance`（代际真实推进到2）后才放行worker线程进入其 `transition_task`，断言 worker 必须失败、状态保持、`user_acceptance_for_task` 为空、事件数不变 |
| M1 | `generate_requirement_snapshot`未实现/未注册/无入口 | **已关闭** | `storage/requirement_items.rs:58-78` 实现（BEGIN IMMEDIATE、归属校验、状态校验先于幂等读取、显式排除Draft/终态——状态集合与 `NOVA_WP2A_FINAL_AMENDMENT10141726.md` 第3.2节`SNAPSHOT_ALLOWED_STATES`完全一致）；`commands.rs:1296`/`lib.rs:151` 注册；`coreLoop.ts:757-766`/`CoreLoopView.vue:365-376` 只读展示四字段+明确"确认沿用，生成需求清单"按钮，`v-if` 排除 draft/completed/failed/cancelled。测试 `wp2a_m1_*`（14状态矩阵、插入失败整体回滚、空快照、Tauri注册断言）通过 |
| M2 | 新attempt非Started时旧结果误判为Config错误 | **已关闭** | `storage/actions.rs:161-205` `finish_action`：CAS失败时按"attempt不同→`StaleActionAttempt`、仍是对应attempt但非started→`Config`"精确区分；`error.rs:23` 新增 `StaleActionAttempt` 变体。`core_loop.rs` `run_tests_for_round`/`run_build_for_round` 全部分支（无测试适用/allowlist拒绝/取消/实际结果）均"先CAS后写evidence"，且显式 `match` 到 `NovaError::StaleActionAttempt` 才返回 `Stale`，其余错误 `Err(other) => return Err(other)` 原样上抛——与 `NOVA_WP2A_FINAL_AMENDMENT10141726.md` 第1-2节的顺序/错误区分要求逐字一致。测试 `wp2a_m2_*` 通过 |
| M3 | legacy_unknown通过任意attempt确认获得通用授权 | **已关闭** | `documents.rs:310-373`：`no_tests_applicable_is_authorized`/`no_build_applicable_is_authorized` 签名改为 `result_attempt: Option<i64>`，`None` 直接 `Ok(false)`；`no_build_confirmation.rs` 已删除旧 `find_any`，`find_no_build_confirmation` 按精确 `(generation, round, result_attempt)` 四元组查询。测试 `wp2a_m3_null_and_other_attempt_never_authorize_current_build` 通过 |
| M4 | Case C（已确认+action已reset为Planned）无法可达 | **已关闭** | `core_loop.rs:1295-1323` `no_build_confirmation_needed`：新增 `ActionStatus::Planned` 分支，要求 `evidence_ok`（同attempt insufficient_evidence）且存在对应确认才返回true（之前 Planned 直接 false）。前端 `CoreLoopView.vue:432-441` 恢复卡在 `noBuildConfirmationNeeded` 为真时展示，确认后转 `Queued`，`Queued` 态单独要求用户点击"开始任务"才启动AI（第443-465行），不自动触发。测试 `wp2a_m4_case_c_query_and_explicit_resume_do_not_repeat_confirmation_or_reset` 通过 |
| M5 | 回滚说明破坏混合工作区，DB恢复步骤不安全 | **已关闭** | `NOVA_WP2A_REMEDIATION_NOTES10151146.md` 回滚章节改为仅反向本轮20文件hunk、三方处理重叠编辑、保留检查点，不整文件checkout/stash；DB恢复流程改为停连接→SQLite online backup→校验→关闭全部连接后在新路径恢复→隔离环境核验。源码 `migrations.rs:1166-1226` `backup_database` 确认使用 `conn.backup(DatabaseName::Main, ...)`（真实online backup API）+ 临时文件 + `integrity_check` + rename，不是裸文件复制。本机独立 `git apply --reverse --check` 验证20文件补丁边界可逆（见上节），未执行真实回滚 |
| M6 | 核心负向/并发验证缺乏对应测试 | **已关闭** | 新增 `core_loop/wp2a_remediation_tests.rs`（24个针对性测试）+ `migrations.rs` 3个新测试，经本机独立 `cargo test --lib wp2a_` 重跑全部通过（见第三节）。抽样核读 `wp2a_m6_post_cas_evidence_write_failure_propagates_and_recovery_is_fail_closed`（真实SQL触发器注入evidence写失败，断言 `NovaError::Database` 向上传播、action仍Succeeded、恢复路径只写`insufficient_evidence`转`NeedsUserAction`且不重跑命令）与 `wp2a_m6_frozen_execution_confirmation_retry_and_completion_are_zero_write`（跨`run_core_loop`/两个确认入口/retry/三个transition目标的零写入矩阵，含`generate_requirement_snapshot`补建不解冻的断言），均直接调用生产函数、非空壳断言 |
| m1 | 时间线误用test/build自身attempt，同代旧attempt未标历史 | **已关闭** | `evidence_overview.rs:596-618` 新增 `is_current_attempt` 字段，按 `dev_attempt_for_round` 比对而非test/build自身attempt。测试 `wp2a_m1_m3_timeline_uses_dev_attribution_and_marks_same_generation_old_attempt` 通过 |
| m2 | 预检与"可以验收"语义冲突 | **已关闭** | `core_loop.rs:649-686` `CompletionPrecheck` 拆分 `ok`（直接完成）与 `can_submit_acceptance`（预测显式验收提交，不要求已有确认）两个字段，`evaluate_acceptance_submission`(require_acceptance=false) 与 `evaluate_completion_gate`(true) 复用同一 `evaluate_result_gate` | 
| m3 | 迁移/备份/warning描述不实 | **已关闭** | `migrations.rs:920-992` 确认v11对四表（test_evidence/build_evidence/no_tests_confirmations/no_build_confirmations）均为"复制新表→DROP旧表→RENAME"模式；`cargo check --lib --tests` 本机重跑无任何warning输出，确认此前两条dead-code警告已消除 |
| S1 | parser仅取首个标记块、忽略未知字段、check_results无默认校验 | **已关闭（建议性问题，非必须）** | `self_check_parser.rs:17,27,59-61`：`#[serde(deny_unknown_fields)]`覆盖两个结构体；`extract_block`要求`START`/`END`标记各恰好出现一次（`matches(...).count() != 1`即拒绝，拒绝多块/矛盾块）；`check_results: Vec<String>`为必填字段（无`#[serde(default)]`），缺失即Invalid |

## 本机独立重跑验证（均为内存/临时目录/Mock，未触碰真实AI CLI或真实用户库）

| 命令 | 结果 |
|---|---|
| `cargo test --lib wp2a_ -- --nocapture`（项目目录 `apps/desktop/src-tauri`） | **24 passed; 0 failed**，9.73s |
| `cargo test --lib`（全量） | **674 passed; 0 failed; 0 ignored**，74.22s，与整改报告声称数字一致 |
| `cargo check --lib --tests`（touch后强制重编译，grep warning） | 无任何warning输出 |
| `npx vitest run`（`apps/desktop`） | **109 passed**，5文件，1.48s |
| `npm run build`（vue-tsc --noEmit && vite build） | 通过，37模块，与此前日志一致 |
| `git apply --reverse --check source-remediation.patch` | exit 0，补丁可逆、边界准确（仅检查） |
| 20个交付文件SHA256逐一重算 | 全部与`delivery-manifest.json`一致 |
| NSIS安装包SHA256重算 | 与`delivery-manifest.json`一致；`tauri-package-final.log`为真实构建日志 |
| `git status` 前后对比 | 本轮操作后仅新增本报告，tracked修改清单、HEAD不变 |

未执行（NOT RUN，原因与是否阻断本轮结论）：

- **真实桌面UI/IPC点击、安装器运行、应用启动**：NOT RUN。原因：当前 `lib.rs::run` 固定使用真实 `app.path().app_data_dir()` 并迁移真实 `nova.sqlite3`，尚无经验证的隔离app-data+Mock CLI启动模式；按指令不得为完成审核擅自给产品代码新增测试模式，也不得仅凭更改环境变量猜测隔离成功。**不阻断本轮判定**：本轮全部B/M/m/S问题均为后端业务逻辑（并发、门禁、授权、CAS、迁移）缺陷，原审核的独立复现方式本身就是直接调用`storage`/`core_loop`源码的真实函数（而非UI点击），本轮沿用且升级为仓库内可重复执行的`#[test]`（含真实文件型SQLite双连接并发），这是比手工点击更强的验证方式，不依赖桌面IPC链路。M1/M4涉及的Tauri注册与前端store/view，本轮核验的是类型级注册断言与源文本边界测试，同样不冒充真实IPC——与此前两轮报告的措辞一致，非本轮新增缺口或遗漏。
- **真实Codex/Claude CLI调用、真实用户数据库迁移/恢复**：NOT RUN，且本轮按指令禁止执行。
- **生产代码签名/安装器交互安装**：NOT RUN，未配置签名凭据，本轮不声明发布验收。

最小补验条件（供后续、非本轮范围）：在产品代码引入一个经审核认可的、仅用于验证目的的隔离启动路径（独立app-data目录+Mock/假CLI注入点，且有机制防止误连真实数据），或在CI/预发布环境用相同的NSIS包执行一次完整安装+启动+关键路径点击（确认需求→生成快照→开始任务→Case C恢复→验收），作为发布前的最后一道门禁；真实AI CLI与真实用户库迁移需在用户同意的真实环境下单独验收，不在本类代码审核范围内。

## 代码审核与运行验收的完成程度

- **代码级正确性审核**：完成。本轮对B1-B3/M1-M6/m1-m3/S1共13个问题逐一直读修复后的源码（`core_loop.rs`、`storage/tasks.rs`、`storage/actions.rs`、`core_loop/documents.rs`、`storage/requirement_items.rs`、`storage/no_build_confirmation.rs`、`core_loop/self_check_parser.rs`、`storage/migrations.rs`、`runtime/ai_adapter.rs`、`error.rs`、前端`coreLoop.ts`/`CoreLoopView.vue`），并与适用规格（`NOVA_WP2A_FINAL_AMENDMENT10141726.md`优先，`NOVA_WP2A_SELF_CHECK_SPEC10141417.md`次之）逐条对照，未发现规格与实现不一致、未发现回归。
- **自动化测试验收**：完成且独立重跑。24个WP2A专项回归测试、674个后端测试、109个前端测试本机全部重跑通过，未采信报告单方面数字。抽样核读了7个关键测试（B1×3、B2×1、B3×1、M6×2）的具体断言内容，确认断言针对真实生产函数与真实场景，非夹具空转或数量堆砌。
- **构建验收**：完成。`cargo build`/`cargo check`/`npm run build`/完整NSIS包构建均独立确认（部分复用已有产物并重算哈希，部分重新执行），构建记录真实、哈希一致。
- **运行验收（桌面IPC/安装/启动）**：未完成，NOT RUN，原因与范围见上节。这是本轮代码验收之外的独立验收环节，不代表代码验收本身存疑。

## 结论

WP2A整改经本轮独立复审，原3 Blocker、6 Major、3 Minor、1 Suggestion**全部关闭**，源码核验与独立重跑的自动化测试结果一致，未发现新回归或规格偏离。**PASS**（限代码验收范围；真实桌面IPC/安装/启动/真实CLI/真实用户库验收仍为独立的、尚未执行的后续环节，发布前需按上节最小补验条件单独完成，本结论不构成发布批准，不建立里程碑）。
