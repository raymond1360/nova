# CODEX_WP2A_SPEC_REREVIEW10141259

## 最终结论

**FAIL。**

本轮最新 WP2A 方案较 `CODEX_WP2A_SPEC_REVIEW10141205.md` 有实质改进：`self_reported_status`、需求项快照、rowid 排序、preview/completed 分离、用户确认缺失用例、历史 action 核验等方向均已补强。但方案仍存在会影响实现安全性的 Blocker，尤其是“当前开发 attempt 的自检”与“真实 test/build 证据”没有同一执行归属绑定，且历史分类规则仍自相矛盾。因此不允许启动 WP2A 编码。

本结论只代表静态方案复审；本轮未运行 AI CLI、未读取真实用户数据库、未运行测试/构建，不代表功能运行验收通过。

## 仓库核验

- 项目目录：`D:\nova`
- 当前分支：`feature/v0.1.0-mvp`
- 当前 HEAD：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`
- `milestone/evidence-gate^{}`：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`
- 根目录未发现 `AGENTS.md`、`CLAUDE.md`、`README`、`CONTRIBUTING`
- `git status --short --untracked-files=all` 在审核前非空：两份 product 文档 tracked 修改，以及多份 design/release/reports 未跟踪文件。本轮保留全部既有修改，不切换、不回退、不清理。
- 上一轮报告优先查找 `D:\nova\docs\reviews\CODEX_WP2A_SPEC_REVIEW10141205.md`：不存在；只读查到原位置 `D:\nova-review\CODEX_WP2A_SPEC_REVIEW10141205.md`。

## Blocker

### B1. 当前开发 attempt 与 test/build 证据没有绑定，方案仍可能放行“当前自检 + 旧证据”

**性质：** 上一轮 B4 部分未关闭，本轮新增具体反例。

**依据：**

- 新方案以 `dev_action_attempt_number` 过滤当前自检，见 `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141243.md:76-86`。
- 但真实测试/构建证据只记录 `task_id/result_generation/round`，不记录开发 action id 或 attempt：`D:\nova\apps\desktop\src-tauri\src\storage\evidence.rs:30-44`、`:59-70`、`:321-333`、`:473-485`。
- test/build action key 是 generation + round 维度，不含 dev attempt：`D:\nova\apps\desktop\src-tauri\src\core_loop.rs:2029-2054`、`:2368-2391`。
- `adopted_passing_generation_round()` 也只按 generation + round 查 evidence，不知道这些 evidence 属于哪个 dev attempt：`D:\nova\apps\desktop\src-tauri\src\core_loop\documents.rs:237-288`。
- 新验收矩阵 #26 已承认 `adopted_passing_generation_round()` 与 attempt 无关，但只处理“旧 generation”的情况；没有解决“同 task、同 generation、同 round，dev attempt 已更新，但 test/build evidence 仍来自旧 attempt”的核心场景，见 `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141243.md:214`。

**影响：** 可构造同一 task/round 内开发 attempt 更新，当前 attempt 自检有效，但 test/build 仍复用旧 attempt 的 passed evidence。按当前方案，self_checks 命中当前 attempt，adopted evidence 命中同 generation/round 的旧证据，用户确认后可能进入 `completed`，违反“当前结果真实验证”的门禁要求。

**最小修正建议：** 不强制新增 generation 列，但必须给“当前结果”一个统一执行归属。可选最小机制包括：在 test/build evidence 写入时同时记录 `dev_action_attempt_number` 或 `dev_action_id`；或新增一次开发成功后的 `result_attempt_id`，由 self_check、test_evidence、build_evidence 共同引用。完工门禁必须要求自检与 test/build 证据属于同一 dev attempt/result attempt。

**通过条件：** 验收矩阵必须包含并拒绝：“同 task、同 generation、同 round；当前 dev attempt 自检有效；test/build passed 证据来自旧 dev attempt”。测试/构建证据、self_check、用户确认三者必须由后端统一核验当前执行归属。

### B2. 历史任务分类规则仍自相矛盾

**性质：** 上一轮 M4 部分关闭，但历史分类仍未达唯一确定规则。

**依据：**

- 迁移计划说三项双 AI 证据任一命中即可分类为“有证据的双AI”：`ai_review action`、`task_reviews`、实质审核文档，见 `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141246.md:23-27`。
- 同一节又说 `task_reviews` 无行但 `FIRST_REVIEW.md` 有实质内容是“真正的矛盾”，归 unknown；`ai_review` 行存在但 `task_reviews` 无对应 round 也归 unknown，见 `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141246.md:34-36`。
- 最终算法再次写为任一审核证据存在即双 AI，见 `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141246.md:42-48`。

**影响：** 同一事实组合会得到两个分类：例如“审核文档有实质内容、task_reviews 无行”既满足“任一命中即双AI”，又被示例归为 unknown。历史任务处理会不可重复，无法形成可审计迁移。

**最小修正建议：** 明确区分“路径曾进入双 AI 审核”与“记录完整性”。建议分类为：`dual_ai_evidenced`（任一审核证据命中）、`single_ai_evidenced`、`unknown_insufficient`；另设 `integrity_warning` 标记记录渠道不一致。不要把“命中双 AI 证据但记录不完整”同时归 unknown，除非定义 unknown 为“不能决定后续处理”，并删除 OR 分类。

**通过条件：** 对以下组合给出唯一分类和处理：仅有 `ai_review action`；仅有 `task_reviews`；仅有实质审核文档；`ai_review action` 有但 `task_reviews` 无；审核文档有但 `task_reviews` 无；三者全无但开发成功；历史 schema 证据源不存在。

### B3. 拒绝 completed 时的状态/事务语义仍冲突

**性质：** 上一轮 m2 未关闭并升级为 Blocker，因为它直接影响实现口径。

**依据：**

- 正文说全部自检门禁未通过分支只在 `AwaitingUserAcceptance -> Completed` 求值，Blocked 结果统一走合法的 `(AwaitingUserAcceptance, NeedsUserAction)` 边，见 `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141243.md:161`。
- 验收矩阵 #2-#11 的“预期状态”写 `NeedsUserAction`，但“持久化结果”又写“回滚，无状态变化”或“同上”，见 `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141243.md:189-199`。
- 当前合法边确实有 `AwaitingUserAcceptance -> NeedsUserAction`，但如果事务回滚且无状态变化，任务会保持 `AwaitingUserAcceptance`，不是 `NeedsUserAction`：`D:\nova\apps\desktop\src-tauri\src\storage\tasks.rs:58-67`。

**影响：** 编码者无法判断门禁失败时应持久化 `NeedsUserAction` 事件，还是拒绝 completed 并保持 `AwaitingUserAcceptance`。这会影响 UI、恢复、事件记录、是否还能查看效果，以及用户如何触发修复。

**最小修正建议：** 二选一固定：  
1. `completed` 被拒绝时保持 `AwaitingUserAcceptance`，只返回结构化拒绝原因，用户仍可查看效果和重新确认；或  
2. 在同一事务中明确写入 `NeedsUserAction`，记录状态事件和恢复说明，并定义用户如何回到查看/确认或修复路径。  
矩阵中的“预期状态”和“持久化结果”必须一致。

**通过条件：** 所有自检门禁失败用例都明确最终持久化状态、事件记录、恢复/继续方式，且与合法状态转换表一致。

## Major

### M1. 所有 completed 入口的权威门禁落点仍需明确到 `Database::transition_task`

**性质：** 上一轮 M3 部分关闭。

**依据：**

- 当前暴露了通用 `transition_task` Tauri 命令，前端或调用方可传任意合法目标状态，见 `D:\nova\apps\desktop\src-tauri\src\commands.rs:536-553`。
- 当前 `accept_task_result` 只是先写用户确认，再调用 `db.transition_task(..., Completed)`，见 `D:\nova\apps\desktop\src-tauri\src\core_loop.rs:562-583`。
- 现有 completed 权威门禁在 `Database::transition_task` 的 SQL 中，只检查当前 generation 用户确认，见 `D:\nova\apps\desktop\src-tauri\src\storage\tasks.rs:316-332`。
- 新方案说 `completion_gate_for_current_result()` 在同一个 `BEGIN IMMEDIATE` 事务内完成判定和写入，见 `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141243.md:173`，但没有明确必须内聚到 `Database::transition_task(to == Completed)` 或等效唯一底层入口。

**影响：** 如果只在 `accept_task_result` 层加门禁，通用 `transition_task` 仍可能成为旁路；如果在新函数里加门禁但保留旧 `transition_task` 的 completed 写入，仍不满足“所有 completed 入口”。

**最小修正建议：** 方案明确：`Database::transition_task` 在 `to == Completed` 时调用同事务内的 completed gate，或删除/限制通用命令对 `completed` 的能力，使所有 completed 状态写入只有一个权威路径。

**通过条件：** 静态检查和测试都能证明任何到 `TaskState::Completed` 的生产路径都经过同一后端门禁。

### M2. 旧未完成任务缺少需求快照时的处理未定义

**性质：** 新引入问题。

**依据：**

- WP2A-0 只定义在 `transition_task(to == AwaitingConfirmation)` 时生成 `confirmed_requirement_items`，见 `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141243.md:54`。
- 当前前端确认需求确实是先 `Draft -> AwaitingConfirmation` 再 `AwaitingConfirmation -> Confirmed`，见 `D:\nova\apps\desktop\src\stores\coreLoop.ts:460-478`。
- 但方案未定义迁移前已经处于 `Confirmed`、`Queued`、`Running`、`PreviewReady`、`AwaitingUserAcceptance` 或 `NeedsUserAction` 的未完成任务如何获得或拒绝需求快照。

**影响：** 旧未完成任务可能没有 `confirmed_requirement_items`，完工门禁要么无法覆盖需求项，要么诱导实现者静默从当前自由文本补造“已确认”快照，违背 PM 要求。

**最小修正建议：** 对迁移前非终态且无快照任务定义策略：冻结并要求用户重新确认以生成快照；或标记为 `unknown_requirement_snapshot`，不得自动 completed。已 completed 历史任务继续只读保留。

**通过条件：** 验收矩阵包含“旧未完成任务无 confirmed_requirement_items”的场景，且不静默补造、不自动继续、不自动完成。

### M3. `self_checks` 表结构在新版规格中不完整

**性质：** 上一轮 M1 部分关闭。

**依据：**

- 新版只给出查询 `WHERE task_id = ? AND round = ? AND dev_action_attempt_number = ? ORDER BY rowid DESC LIMIT 1`，见 `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141243.md:76-80`。
- 但新版没有完整 `self_checks` 表定义，没有列出 `project_id`、`parse_status`、`self_reported_status`、coverage JSON、两类数组、raw block、`dev_action_attempt_number`、必要索引、外键和脱敏字段。

**影响：** 方案要求编码，但 schema 合约不完整；尤其无法审查 `dev_action_attempt_number` 是否 NOT NULL、是否有索引、是否与 task/action 归属校验。

**最小修正建议：** 恢复完整 `self_checks` DDL，并明确追加写入、索引、外键、字段约束和查询权威函数。

**通过条件：** 规格中有完整 DDL/字段语义，能直接推导存储实现和测试。

### M4. 文档仍有占位引用

**性质：** 新引入文档问题，非核心 Blocker。

**依据：** `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141243.md:144` 仍引用 `docs/release-v0.1.0/NOVA_SINGLE_AI_MIGRATION_PLAN<本轮时间戳>.md`，而本轮实际文件为 `NOVA_SINGLE_AI_MIGRATION_PLAN10141246.md`。

**影响：** 后续审计路径不精确。

**最小修正建议：** 替换为实际文件名。

**通过条件：** 全文无 `<本轮时间戳>` 占位引用。

## Minor

### m1. 按换行拆分需求项只能提供形式覆盖，不代表语义完整

方案已比上一版更可执行，但需要在规格中明确：换行拆分只是 v0.1.0 的结构化最小机制，不是语义完整性证明，也不是独立审核。AI 对某行标 `covered` 仍只是自检陈述，必须继续受真实证据和用户确认约束。

### m2. WP2A-0 与 WP2A 主体可以按依赖顺序实施，但不必重复审批到拖慢反馈

WP2A-0 是主体门禁的前置依赖。建议实现时拆提交、同批复审或先后复审均可；重点是 WP2A 主体不得在缺少需求快照策略时进入编码。

## Suggestion

- 在 finished gate 的返回值中定义结构化错误枚举和用户可见恢复建议，避免 UI 只能展示普通字符串。
- 将“当前结果归属”命名为 `result_attempt` 或等价概念，比复用 `dev_action_attempt_number` 更容易表达 self_check/test/build 三者绑定。

## 上一轮问题关闭状态

- B1（缺少 `self_reported_status = self_reported_complete`）：**已关闭**。矩阵覆盖 incomplete/blocked 空数组场景。
- B2（coverage 完整覆盖门禁缺失）：**部分关闭**。已引入 `confirmed_requirement_items`，但旧未完成任务无快照处理未定义，且需明确形式覆盖边界。
- B3（user_acceptance round/schema 矛盾）：**已关闭**。新版改为解释 current task generation 与 adopted evidence generation 的关系，不再要求 user_acceptance 自身有 round。
- B4（run_id/attempt 不校验）：**部分关闭**。自检引入 attempt 过滤，但未绑定 test/build evidence 与同一 dev attempt，仍为本轮 Blocker。
- B5（有效完成用例遗漏用户确认）：**已关闭**。矩阵 #1/#19 已覆盖。
- M1（self_checks 写入策略矛盾）：**部分关闭**。rowid + attempt 方向可行，但完整表结构缺失，且当前结果归属仍未贯通到 evidence。
- M2（挂载点破坏 preview_ready）：**已关闭**。新版明确 preview 与 completed 分离。
- M3（并发与原子性不具体）：**部分关闭**。事务方向更合理，但权威 completed 入口必须明确落到所有 `transition_task(...Completed)` 路径。
- M4（历史分类关键依据留到编码阶段）：**部分关闭**。`ai_review` action 已核验，但分类规则仍自相矛盾。
- m1（同构表述不准确）：**已关闭**。新版改为第三种独立语义。
- m2（状态落点开放）：**未关闭**。矩阵仍同时写 `NeedsUserAction` 与“回滚无状态变化”。

## 是否允许启动 WP2A 编码

**不允许。**

允许的下一步是继续修订方案，最小修正范围为：当前结果归属绑定、历史分类唯一规则、门禁失败状态语义、所有 completed 入口权威落点、旧未完成任务无需求快照策略、完整 `self_checks` DDL、占位引用修正。

不应扩展 Phase5A、GitHub 功能、双 AI 启用或全站 UI 改造。

## 本轮未做事项

- 未运行 AI CLI。
- 未读取真实用户数据库。
- 未运行测试/构建。
- 未修改应用代码、配置、锁文件、既有文档或数据库。
- 未提交、未 push、未发布、未建立里程碑。
