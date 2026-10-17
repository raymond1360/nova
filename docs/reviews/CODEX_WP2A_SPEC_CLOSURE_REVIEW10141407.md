# CODEX WP2A Spec Closure Review 10141407

生成时间：2026-10-14 14:07 Asia/Shanghai  
项目：`D:\nova`  
复审类型：WP2A 固定清单关闭复审，静态只读核验；未运行测试/构建，未运行 AI CLI，未读取真实用户数据库。  

## 最终结论

**FAIL**

不允许启动 WP2A 编码。新版方案已关闭上一轮 4 个 Blocker 中的 B1/B2/B3 主体问题，B4 也大幅收敛；但原 M1（执行上下文 attempt 绑定）仍未真正关闭，原 M2（规格自包含性，尤其需求快照/WP2A-0 契约）仍缺少实施所需核心规则。另发现冻结入口对无测试/无构建确认命令的 action 变更顺序仍需补充。

方案通过不代表功能运行验收通过；本轮没有做运行验收。

## 仓库状态核验

- 根目录规则文件：未发现 `AGENTS.md`、`CLAUDE.md`、`README`、`CONTRIBUTING`。
- 当前分支：`feature/v0.1.0-mvp`
- 当前 HEAD：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`
- `milestone/evidence-gate^{}`：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`
- `git status --short --untracked-files=all`：非空，保留既有修改和未跟踪文件。已见修改包括：
  - `M product/NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md`
  - `M product/NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`
  - 多份未跟踪方案/报告文档，包括本轮材料与既有审查报告。

按用户要求，未切换、未清理、未回退。

## 已读材料

- `D:\nova\docs\reviews\CODEX_WP2A_SPEC_TARGETED_REVIEW10141332.md`
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141347.md`
- `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141350.md`
- `D:\nova\docs\reports\CLAUDE_WP2A_SPEC_REMEDIATION10141351.md`
- 必要源码/schema/调用链：
  - `D:\nova\apps\desktop\src-tauri\src\core_loop.rs`
  - `D:\nova\apps\desktop\src-tauri\src\core_loop\documents.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\actions.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\tasks.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\evidence.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\no_tests_confirmation.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\no_build_confirmation.rs`
  - `D:\nova\apps\desktop\src-tauri\src\commands.rs`
  - `D:\nova\apps\desktop\src-tauri\src\lib.rs`

## 逐项关闭判断

### 原 B1：历史分类

状态：**已关闭**

证据：
- `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:306-317` 明确删除 “开发成功 + 无审核痕迹 + 终态/待验收 -> single_ai_evidenced” 推断。
- `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:321-329` 的组合表把“两强信号都无，但开发成功、任务已到达终态/待验收”改为 `unknown_insufficient`。
- `NOVA_SINGLE_AI_MIGRATION_PLAN10141350.md:17-24` 与正文一致。
- 源码核验：`run_core_loop` 与 `run_core_loop_dual_ai_v0_2` 共用开发步骤；双 AI 审核独有 `ai_review` action。开发 action 本身不能区分路径。

影响：原错误推断已删除；历史终态只读保留；新任务通过迁移时一次性 `legacy_classification` 为空避免误冻。

通过条件：已满足。

### 原 B2：最终 completed 门禁

状态：**已关闭，带实现注意**

证据：
- `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:214-283` 给出 `completed_gate_checks` 伪代码，覆盖旧任务冻结、adopted generation/round、开发 action 成功、同 `result_attempt` 的 test/build evidence 或有效授权、当前 self_check、需求覆盖、当前 user acceptance。
- `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:286-292` 明确接入 `Database::transition_task(to == Completed)`，在同一 `BEGIN IMMEDIATE` 事务内完成，且不依赖 Verifying 阶段曾经检查过。
- 当前源码 `storage/tasks.rs:316-332` 现有 completed gate 只检查 `user_acceptance`，因此新规格确实覆盖了需要扩展的位置。
- 历史 NULL 归属：`NOVA_WP2A_SELF_CHECK_SPEC10141347.md:406` 明确历史 `result_attempt=NULL` 永远不匹配当前 attempt，不能满足门禁。

adopted round 核验：
- 当前 `adopted_passing_generation_round` 位于 `core_loop/documents.rs:237-292`，会在 `task.result_generation` 与 `task.result_generation - 1` 中寻找 passing evidence。新版 completed gate 先用 adopted 的 `(generation, round)` 找 dev action，再要求 evidence/auth/self_check 的 `result_attempt` 与当前 dev action attempt 精确匹配。该设计可以防止单纯退回旧证据而完成，但编码时必须保留 `Some(current_attempt)` / 非 NULL 精确匹配。

实现注意：第 3.2 节中的 `no_build_applicable_is_authorized` 仍以注释 `...` 表示同构实现，编码时需要实际落到函数签名和 mechanical path 的 `build_evidence.result_attempt` 比对。

通过条件：方案层面已满足；实现阶段需补测试证明通用 `transition_task` 直接 completed、`accept_task_result`、恢复/重入路径都经过同一门禁。

### 原 B3：重新授权

状态：**已关闭**

证据：
- `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:147-196` 选定唯一键扩展为 `UNIQUE(task_id, result_generation, round, result_attempt)`。
- 写入语义改为 `ON CONFLICT(task_id, result_generation, round, result_attempt) DO NOTHING`，查询也带 `result_attempt`。
- 历史行迁移 backfill 为 `NULL`，既不删除历史，也不让 NULL 历史行匹配新 attempt。
- 当前源码 `storage/no_tests_confirmation.rs:50-60`、`storage/no_build_confirmation.rs:58-68` 仍是旧三元唯一键和三元查询；新版方案准确指出了需改位置。

影响：新 attempt 可独立写入确认，旧 attempt 授权不可被误用，历史可审计。

通过条件：方案层面已满足；实现阶段需迁移、模型字段、写入/查询、并发确认测试全部同步。

### 原 B4：冻结入口

状态：**部分关闭**

已关闭部分：
- `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:343-353` 定义迁移时一次性写入 `tasks.legacy_classification`，终态历史任务保持 NULL，新任务保持 NULL，不会被动态误冻。
- `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:357-363` 列出 `run_core_loop`、`unblock_test_execution_for_retry`、恢复扫描/重入、通用 `transition_task` 到 `Queued/Running`、最终 `Completed` 门禁。
- `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:367-369` 明确只读查看和取消保留，`generate_requirement_snapshot` 不解除冻结。

未完全关闭点：
- 当前源码中 `confirm_no_tests_applicable` / `confirm_no_build_applicable` 属于真实可达入口，命令在 `commands.rs:1185-1193`、`commands.rs:1234-1242` 暴露。
- `confirm_no_tests_applicable` 进入 `reconcile_no_tests_confirmation_attempt` 后，在当前源码 `core_loop.rs:1362-1376` 可先调用 `reset_failed_action_for_retry` 修改 action，再在 `core_loop.rs:1389-1397` 调用 `transition_task(...Queued)`。
- 新方案只要求 `transition_task(to=Queued/Running)` 拒绝冻结任务，未明确这些确认/恢复入口在 action reset、confirmation write 之前也要先做冻结检查，或将这些修改与状态转换纳入同一事务失败即回滚。

影响：冻结任务可能虽然无法真正转入 `Queued`，但先发生 action ledger 变更或确认写入。这不是“只读查看”或“取消”，也不只是启动恢复记账；它是继续执行准备动作，会污染冻结任务的审计状态。

最小修正建议：
- 在 `confirm_no_tests_applicable`、`confirm_no_build_applicable`、其 reconcile attempt 入口，以及任何会写 confirmation 或 reset action 的恢复/继续入口开头，先检查 `legacy_classification` 冻结并拒绝；
- 或将 confirmation/action reset/transition 合并为受冻结检查保护的事务，冻结时不写任何 action/confirmation 变化；
- 明确 `run_startup_recovery` 的“中断记账”允许范围，与用户触发的继续执行准备动作分开。

通过条件：冻结任务除只读查看、取消、`generate_requirement_snapshot` 外，不产生 action reset、confirmation write、Queued/Running/Completed 转换。

### 原 M1：执行上下文 attempt 绑定

状态：**未关闭**

证据：
- 新规格 `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:67-79` 对 `Started` 分支仍是在 AI 执行完成后调用 `finish_gated_action`，再从返回的 `TaskAction` 读取 `attempt_number`。
- `ActionGate::Started` 在新伪代码里仍不携带 attempt，`gate_action` 对 `db.start_action(...)` 的返回值仍是 `Ok(_) => Ok(ActionGate::Started)`，见 `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:50-51`。
- 当前源码 `storage/actions.rs:67-107` 的 `start_action` 实际返回 `TaskAction`，其中已包含启动时的 `attempt_number`；但当前代码和新伪代码都丢弃它。
- 当前源码 `storage/actions.rs:146-179` 的 `finish_action` 更新条件是 `WHERE task_id = ?5 AND idempotency_key = ?6 AND status = 'started'`，没有校验调用者启动时的 attempt。

为什么这仍不能解决“旧执行晚到不覆盖新 attempt”：
- 如果执行 A 在 attempt 1 启动后输出晚到，而中间 action 已经经由 recovery/acknowledge/retry 进入 attempt 2 的 `started`，旧执行 A 调用 `finish_action` 时仅凭 `status='started'` 就可能把 attempt 2 标成 succeeded。
- 随后 `finish_gated_action` 返回的 `TaskAction` 会是 attempt 2，调用方会把旧输出、自检、test/build 证据错误归属到新 attempt。
- 因此“完成后读回 TaskAction”不是执行前绑定；必须在执行前捕获上下文，并在完成时 CAS 校验同一 attempt。

最小修正建议：
- `ActionGate::Started` 携带 `TaskAction` 或至少携带 `attempt_number`，来自 `db.start_action` 返回值；
- `finish_gated_action` 增加 `expected_attempt: i64`；
- `Database::finish_action` 的 UPDATE 条件增加 `AND attempt_number = ?expected_attempt`，rows_affected 为 0 时拒绝陈旧完成；
- 记录 command evidence、自检解析、test/build evidence 写入都使用启动前捕获的 `result_attempt`，且旧输出晚到时不得写入当前 attempt 的证据。

通过条件：规格明确“执行前捕获 attempt、完成时校验同一 attempt、陈旧完成 fail-closed”，并覆盖 AI 输出、自检、测试/构建结果晚到。

### 原 M2：当前规格完整性

状态：**部分关闭**

已关闭部分：
- `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:1-4` 与 `NOVA_SINGLE_AI_MIGRATION_PLAN10141350.md:1-3` 明确最新两份文件为本轮唯一权威方案，旧文件只作历史。
- 自检表、evidence/confirmation result_attempt、completed gate、legacy_classification 的核心结构已经内联。

未关闭点：
- `confirmed_requirement_items` 只在 completed gate 中被调用，见 `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:269-274`；但本文件没有给出该表 DDL、字段语义、唯一约束、生成/重建幂等规则。
- `generate_requirement_snapshot` 只在冻结边界和验收矩阵中提到，见 `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:369`、`410`，没有完整命令契约：允许状态、输入来源、输出行、重复调用语义、旧任务缺失快照的处理、与用户确认内容的一致性。
- 需求拆分规则也未完整列出。当前源码可见 task 字段 `goal/in_scope/not_in_scope/done_criteria`，但规格没有定义如何从这些字段稳定生成 requirement items、空字段如何处理、是否允许按换行拆分、item id 如何稳定、是否保留顺序。
- 迁移计划 `NOVA_SINGLE_AI_MIGRATION_PLAN10141350.md:48-52` 仍写“其余章节维持不变，不重复抄录”。这对非 WP2A 历史章节可以接受，但不能替代 WP2A-0 的需求快照契约。

影响：WP2A-0 是 WP2A 主体的前置，需求覆盖门禁依赖 `confirmed_requirement_items` 权威集合；缺少该契约会导致实现者无法确定 coverage 校验目标，进而影响 completed gate 的正确性。

最小修正建议：
- 在权威 spec 内补充 `confirmed_requirement_items` DDL、字段、唯一约束、索引；
- 补充 `generate_requirement_snapshot(project_id, task_id)` 或等价命令契约：允许状态、幂等行为、旧行处理、是否可覆盖、失败条件、是否受冻结限制；
- 明确从 `goal/in_scope/not_in_scope/done_criteria` 到 requirement items 的稳定规则和 item id 生成方式；
- 明确旧非终态任务没有快照时只能挡 completed，不得静默伪造“用户已确认”。

通过条件：实现者不需要回查旧稿即可完成 WP2A-0。

### 原 m1：拒绝原因持久化语义

状态：**已关闭**

证据：
- `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:294-296` 明确拒绝原因只通过 API 错误返回，不写专门审计/事件记录。
- 同节说明重新打开任务时由只读预检命令复用 completed gate 只读部分重新计算原因。
- `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:371-373` 明确冻结拒绝不写状态转换事件。

影响：语义一致，不阻断编码；实现阶段需提供只读预检命令，避免 UI 失去可解释性。

## 已维持关闭项

- 失败保持原状态、预览与完工分离：保持关闭。
- 自检必须 `self_reported_complete`，且 `unfinished_items` / `blocking_issues` 独立为空：保持关闭。
- `requirement_coverage` 不靠自由文本“通过”判断：方向保持关闭，但依赖 M2 的需求快照契约补全。
- 一次修复上限、不默认额外调用 AI：保持关闭。
- 真实证据 / 无测试 / 无构建授权规则：方向保持关闭，但 implementation 必须纳入 `result_attempt` 精确匹配。
- 预览展示：新版仍允许 preview 不按 `result_attempt` 作为完工门禁；但 UI/文档不得把旧证据标成“当前 attempt 已验证”。建议在实现时在 evidence overview 中显示 evidence 的 attempt 归属或至少避免“当前验证”措辞。

## 问题汇总

### Blocker

无新增 Blocker。

### Major

1. **M1 未关闭：执行 attempt 仍未在执行前绑定，完成时未校验同一 attempt。**
   - 位置：`NOVA_WP2A_SELF_CHECK_SPEC10141347.md:50-79`；源码 `storage/actions.rs:67-107`、`146-179`。
   - 最小修正：`Started` 携带启动时 `TaskAction/attempt_number`；`finish_action` 增加 expected_attempt CAS；陈旧完成 fail-closed。

2. **M2 部分关闭：WP2A-0 需求快照核心契约缺失。**
   - 位置：`NOVA_WP2A_SELF_CHECK_SPEC10141347.md:269-274`、`369`、`410`、`431`；`NOVA_SINGLE_AI_MIGRATION_PLAN10141350.md:48-52`。
   - 最小修正：补 `confirmed_requirement_items` DDL、生成规则、幂等策略、命令契约、旧任务处理。

3. **B4 部分关闭衍生 Major：冻结入口需覆盖确认/恢复命令在状态转换前的 action/confirmation 写入。**
   - 位置：规格 `NOVA_WP2A_SELF_CHECK_SPEC10141347.md:357-363`；源码 `core_loop.rs:1362-1397`，同构 no-build 路径。
   - 最小修正：确认/恢复入口在任何 action reset 或 confirmation write 前先拒绝 frozen legacy task，或事务内失败回滚。

### Minor

1. `no_build_applicable_is_authorized` 的 result_attempt 处理在 completed gate 伪代码中以同构注释表达，编码前建议写成明确函数签名和返回规则。
2. 预览可保留不阻断，但 evidence overview / 文档展示应避免把旧 attempt evidence 描述为当前 attempt 验证结果。

### Suggestion

- `single_ai_evidenced` 当前永不触发但仍保留枚举值是可以接受的；实现时测试应覆盖“迁移后新任务 `legacy_classification=NULL` 不冻结”。

## 是否允许启动 WP2A 编码

**不允许启动 WP2A 编码。**

允许的下一步最小工作包：继续修订方案文档，只处理：
1. 执行前 attempt 捕获 + finish CAS；
2. `confirmed_requirement_items` / `generate_requirement_snapshot` 完整契约；
3. 冻结任务在确认/恢复入口中的写入前拦截。

不得开始产品代码实现、数据库迁移实现、测试实现或 Phase 范围外功能。

