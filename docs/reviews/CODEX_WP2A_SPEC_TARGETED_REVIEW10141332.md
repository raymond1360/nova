# CODEX WP2A Spec Targeted Review 10141332

生成时间：2026-10-14 13:32 Asia/Shanghai  
项目：`D:\nova`  
复审类型：定向只读方案复审，不运行测试/构建，不运行 AI CLI，不读取真实用户数据库。  

## 最终结论

**FAIL**

不允许启动 WP2A 主体编码。当前方案已经比上一轮更接近可实施状态，但仍存在会影响历史分类、最终完工门禁、无测试/无构建重新授权、旧任务冻结入口的阻断问题。方案通过不等于功能运行验收通过；本轮也没有进行运行验收。

## 仓库状态核验

- 仓库规则：根目录未发现 `AGENTS.md`、`CLAUDE.md`、`README` 类规则文件。
- 当前分支：`feature/v0.1.0-mvp`
- 当前 HEAD：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`
- `milestone/evidence-gate^{}`：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`
- `git status --short --untracked-files=all`：非空。已有修改包括：
  - `M product/NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md`
  - `M product/NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`
  - 多个未跟踪文档，包括本轮读取的 `docs/release-v0.1.0/NOVA_WP2A_SELF_CHECK_SPEC10141315.md`、`docs/release-v0.1.0/NOVA_SINGLE_AI_MIGRATION_PLAN10141317.md`、`docs/reports/CLAUDE_WP2A_SPEC_REMEDIATION10141318.md`、上一轮审核 `docs/reviews/CODEX_WP2A_SPEC_REREVIEW10141259.md`。

以上状态按用户要求保留，未切换、未清理、未回退。

## 已读材料

- `D:\nova\docs\reviews\CODEX_WP2A_SPEC_REREVIEW10141259.md`
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md`
- `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141317.md`
- `D:\nova\docs\reports\CLAUDE_WP2A_SPEC_REMEDIATION10141318.md`
- 必要源码：
  - `D:\nova\apps\desktop\src-tauri\src\core_loop.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\tasks.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\actions.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\evidence.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\no_tests_confirmation.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\no_build_confirmation.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\migrations.rs`

## Blocker

### B1. 历史分类仍把“无审核痕迹”当作单 AI 正面证据

位置：
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md:271`
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md:289`
- `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141317.md:19`

证据：方案仍规定 `(ai_development Succeeded存在) AND (两个强信号都不存在) AND (任务已到达终态或awaiting_user_acceptance)` 分类为 `single_ai_evidenced`。但当前开发 action 是单/双 AI 路径共用步骤，开发成功只能证明开发执行过，不能证明没有进入或不需要进入双 AI 路径；“无 ai_review action / 无 task_reviews”是缺少审核痕迹，不是单 AI 路径的正面证据。

影响：历史任务可能被错误归为 `single_ai_evidenced`，从而绕过 `unknown_insufficient` 的冻结策略，后续继续执行或完成会掩盖历史执行路径不明的问题。

最小修正建议：`single_ai_evidenced` 必须依赖正面证据，例如任务创建/执行时间明确落在单 AI 路径唯一启用之后，或存在可审计的单 AI 模式标记/版本标记/入口记录。仅有开发成功且无审核痕迹时，应分类为 `unknown_insufficient`，除非方案能证明该历史 schema 的所有双 AI 证据源都完整存在并覆盖该任务。

通过条件：正文、算法、组合表和迁移计划对“三者全无但开发成功”的结论一致改为 unknown，或补充可核验的单 AI 正面证据来源。

### B2. 最终 `Completed` 门禁仍未明确重新核验真实证据/授权与当前执行归属

位置：
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md:183-215`
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md:238`
- 当前源码 `D:\nova\apps\desktop\src-tauri\src\storage\tasks.rs:316-332`
- 当前源码 `D:\nova\apps\desktop\src-tauri\src\core_loop.rs:562-583`

证据：新方案把自检和需求覆盖门禁内联到 `Database::transition_task(to == Completed)` 的事务内，这是正确方向。但第 3.2 节 SQL/说明只明确用户确认、需求快照、coverage、自检状态与 `result_attempt`，没有要求最终权威门禁重新读取并核验同一执行归属的 test/build 证据或有效无测试/无构建授权。第 2.4 节和验收矩阵第 2 行仍把“旧 attempt 证据”主要交给 Verifying 阶段拦截，并写成“根本不会走到 AwaitingUserAcceptance”。

当前源码中 `transition_task` 的 completed gate 只检查当前 `result_generation` 的用户验收记录，`accept_task_result` 只是先写验收再调用 `transition_task`。因此方案必须明确最终底层入口也检查真实证据/授权，不能把测试阶段检查当成最终权威检查的替代。

影响：直接调用通用 `transition_task(...Completed)`、恢复路径、或状态已经进入 `AwaitingUserAcceptance` 的异常数据，仍可能形成“当前自检 + 旧 attempt 证据/授权 + 当前用户确认”的错误完成。

最小修正建议：在 `Database::transition_task(to == Completed)` 的同一 `BEGIN IMMEDIATE` 事务中，计算并绑定当前 round/current result_attempt 后，必须同时核验：
- 当前开发 action/attempt；
- 当前有效 self_check；
- 完整 requirement_coverage；
- 同一 `result_attempt` 的 test/build passing evidence，或同一 `result_attempt` 的无测试/无构建有效授权；
- 当前 `result_generation` 的用户确认；
- 合法源状态。

历史 `result_attempt = NULL` 的 evidence/confirmation 不能满足新结果的归属门禁。

通过条件：规格中的 `Completed` 权威门禁明确包含 test/build evidence 或 no-test/no-build authorization 的同 attempt 检查，并覆盖通用 `transition_task`、确认命令、恢复/重入路径。

### B3. `UNIQUE(task_id,result_generation,round)` 与新 attempt 重新授权冲突未解决

位置：
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md:136-142`
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md:163-169`
- 当前源码 `D:\nova\apps\desktop\src-tauri\src\storage\no_tests_confirmation.rs:50-60`
- 当前源码 `D:\nova\apps\desktop\src-tauri\src\storage\no_build_confirmation.rs:58-68`

证据：方案保留 `UNIQUE(task_id,result_generation,round)`，同时要求新 attempt 需要独立确认。当前实现的写入语义是 `ON CONFLICT(task_id, result_generation, round) DO NOTHING`，随后按同一三元组查询旧行。若 attempt 1 已有确认，attempt 2 需要新确认时，唯一键会阻止新行；若沿用当前 `DO NOTHING`，新 attempt 永远拿不到 `result_attempt = 2` 的有效授权。若改成覆盖更新，又会破坏审计历史；方案没有给出安全写入、失效与审计语义。

影响：无测试/无构建场景要么永久无法重新授权，要么被迫复用旧 attempt 授权，要么覆盖历史记录。三者都不满足“当前结果、当前执行、当前用户确认”的完工边界。

最小修正建议：明确一种可实现的授权模型，例如：
- 将当前有效确认与历史审计分离，当前表可更新 `result_attempt`/`superseded_at`，历史表追加审计；
- 或把唯一键扩展为包含 `result_attempt`，并另设“当前采纳规则”确保只有当前执行归属的行有效；
- 或保留唯一键但定义显式失效/替换事务，且保留旧确认审计。

通过条件：新 attempt 重新确认可写入、旧 attempt 授权不可被误用、历史确认可审计，且与 completion gate 的采纳规则一致。

### B4. 旧非终态任务冻结缺少实际入口约束

位置：
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md:294`
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md:321`
- `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141317.md:31`
- 当前源码 `D:\nova\apps\desktop\src-tauri\src\core_loop.rs:101-155`
- 当前源码 `D:\nova\apps\desktop\src-tauri\src\core_loop.rs:721-746`

证据：方案说 `unknown_insufficient` 与 `dual_ai_evidenced` 的非终态任务冻结，不静默转换、不自动继续、不自动完成。但规格的旧任务无快照用例只写“拒绝转入 Completed”，没有定义冻结检查实际落在哪些入口。当前源码 `run_core_loop` 只检查状态是否为 `Confirmed|Queued`，`unblock_test_execution_for_retry` 可把 `NeedsUserAction` 重新排队；方案没有说明这些入口如何读取历史分类并拒绝 unknown/dual 的非终态继续执行。

影响：旧任务可能虽被 completed gate 阻挡，却仍能启动、恢复、重排队或继续执行，违反“不得静默转换、继续执行或自动完成”。`generate_requirement_snapshot` 也可能被误解为既补需求确认，又解除历史执行路径 unknown 的限制。

最小修正建议：定义历史分类结果的持久化或可重复计算位置，并把冻结检查接入至少这些入口：`run_core_loop`、`unblock_test_execution_for_retry`、恢复扫描/重入路径、用户触发的通用状态转换，以及最终 completed gate。`generate_requirement_snapshot` 只能补需求确认，不能解除 `unknown_insufficient` / `dual_ai_evidenced` 非终态冻结。

通过条件：方案明确哪些函数/命令会拒绝 frozen legacy task，拒绝后保持状态和事件语义一致，已完成历史任务仍只读保留。

## Major

### M1. `result_attempt` 捕获点仍有“事后读取当前 action”歧义

位置：
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md:76-91`
- 当前源码 `D:\nova\apps\desktop\src-tauri\src\core_loop.rs:1545-1578`
- 当前源码 `D:\nova\apps\desktop\src-tauri\src\core_loop.rs:2777-2808`

证据：正文说在发起/执行上下文中捕获，不做事后贴标签；但伪代码是在开发成功后通过 `db.find_action(task_id,&dev_key)?.attempt_number` 读取当前 action。当前 `finish_gated_action` 不返回 action/attempt，只调用 `db.finish_action(...)` 后返回 `Ok(())`。如果重试、恢复或并发路径在旧输出返回和读取 action 之间改变了 attempt_number，方案没有给出更强的上下文绑定。

影响：存在把旧输出、自检或证据错误归入新 attempt 的实现风险，尤其是在恢复和重入路径下。

最小修正建议：不必新增不必要 ID 系统，但应把 `attempt_number` 作为执行上下文从 `start_action` / `finish_action` 返回值捕获并随本次 `ExecutionResult` 传递，或让 `finish_gated_action` 返回已完成的 `TaskAction`，由调用方使用该返回值，而不是完成后重新查询“当前 action”。

通过条件：规格伪代码与函数签名清楚表达 attempt 从本次 started action 返回值取得，并显式传给 self_check/test/build/authorization。

### M2. 核心规格引用仍有旧文件/章节混用

位置：
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md:244`
- `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141317.md:39`

证据：最新迁移计划是 `NOVA_SINGLE_AI_MIGRATION_PLAN10141317.md`，但规格仍引用 `NOVA_WP2A_SELF_CHECK_SPEC10141243.md` 的旧章节，并在迁移计划中保留“与 `NOVA_SINGLE_AI_MIGRATION_PLAN10141246.md` 一致”的旧稿引用。作为历史关系可以保留，但当前可实施规格不应让实现者在旧稿和新稿之间判断哪一个权威。

影响：实现时容易把旧版“门禁位置、验收矩阵、历史分类”带回新方案。

最小修正建议：明确 `10141315` 与 `10141317` 是本轮唯一权威方案；旧文件只作为被替代历史，不承载实现规则。

通过条件：所有实现引用指向实际最新文件和章节，旧稿引用仅放在变更历史/取代说明中。

## Minor

### m1. 门禁失败事件记录语义需要再明确

位置：
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md:217` 以后

证据：方案已经选择失败时保持 `AwaitingUserAcceptance`，不写 `NeedsUserAction` 状态事件，这是对上一轮 B3 的实质修正。但仍需说明结构化拒绝原因是否写入事件/审计表，还是仅作为 API 错误返回。

影响：不阻断方案继续修订，但会影响用户再次打开任务时能否理解为什么无法完成。

建议：明确“拒绝 completed”是否持久化一个非状态转换事件；如果不持久化，应在 UI/命令响应中稳定展示错误，并保证恢复路径可重新计算。

## Suggestion

### S1. 保留已关闭项，不再扩大讨论

以下方向本轮已基本可接受，不应因命名偏好阻止后续方案修订：
- 自检状态必须为 `self_reported_complete`，`incomplete/blocked` 不因问题数组为空被放行。
- `requirement_coverage` 不靠自由文本“通过”判断。
- 自检缺失/invalid/incomplete 的恢复不默认重复调用 AI，不突破一次修复限制。
- `adopted_passing_generation_round` 的预览语义与 completed 门禁分离。

## 上一轮问题状态

- 原 B1（当前 attempt 自检可配旧 attempt test/build 证据）：**部分关闭**。`result_attempt` 方向正确，但 completed 最终门禁未明确重验同 attempt 真实证据/授权，confirmation 重新授权也未解决。
- 原 B2（历史分类冲突）：**部分关闭**。强信号 OR 与仅文档命中 unknown 的冲突已改善，但“开发成功 + 无审核痕迹”仍被当作单 AI 正面证据。
- 原 B3（门禁失败状态语义冲突）：**基本关闭**。方案选择保持 `AwaitingUserAcceptance`，不写 `NeedsUserAction` 转换；仅剩拒绝原因持久化语义为 Minor。
- 原 M1（需求快照时机/旧任务）：**部分关闭**。快照方向改善，但旧非终态任务冻结入口缺失。
- 原 M2（自检写入/重解析/失效）：**基本关闭**。rowid 最新行和 invalid 失效方向可实施，仍需在编码时覆盖同 attempt 晚到测试。
- 原 M3（并发与原子性）：**部分关闭**。completed 入口内联到 `transition_task` 方向正确，但最终门禁内容不完整。
- 原 M4（历史任务回滚/兼容）：**部分关闭**。迁移说明更谨慎，但历史分类和冻结入口仍阻断。

## 是否允许启动 WP2A 编码

**不允许启动 WP2A 主体编码。**

允许的下一步最小工作包是继续修订方案文档，聚焦四个阻断点：
1. 历史 `single_ai_evidenced` 必须有正面证据，不能靠“无审核痕迹”推断；
2. `Database::transition_task(to == Completed)` 最终门禁必须重验当前执行归属的真实证据/授权；
3. 无测试/无构建确认表必须解决新 attempt 重新授权与审计历史；
4. 旧非终态任务冻结必须接入实际启动、恢复、重试、通用转换和 completed 入口。

