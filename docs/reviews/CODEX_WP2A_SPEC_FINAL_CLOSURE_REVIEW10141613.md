# CODEX WP2A Spec Final Closure Review 10141613

生成时间：2026-10-14 16:13 Asia/Shanghai  
项目：`D:\nova`  
复审类型：仅复审剩余 3 个 Major 及本轮修订直接引入问题；静态只读核验。未运行测试/构建，未运行 AI CLI，未读取真实用户数据库。

## 最终结论

**FAIL**

不允许启动 WP2A 编码。  

本轮修订已把 B4 衍生 Major 基本关闭，并补齐了需求快照的大量契约；但原 M1 仍有会导致陈旧 test/build evidence 被当前门禁采纳的关键缺口，原 M2 仍存在 `generate_requirement_snapshot` 允许状态与“已确认”前提冲突。方案通过不代表功能运行验收通过，本轮未做运行验收。

## 仓库状态核验

- 根目录规则文件：未发现 `AGENTS.md`、`CLAUDE.md`、`README`、`CONTRIBUTING`。
- 当前分支：`feature/v0.1.0-mvp`
- 当前 HEAD：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`
- `milestone/evidence-gate^{}`：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`
- `git status --short --untracked-files=all`：非空，既有修改和未跟踪文件已保留。包括：
  - `M product/NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md`
  - `M product/NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`
  - 多份未跟踪 docs/release、docs/reports、docs/reviews 文档。

未切换、未清理、未回退。

## 已读材料

- `D:\nova\docs\reviews\CODEX_WP2A_SPEC_CLOSURE_REVIEW10141407.md`
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141417.md`
- `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141420.md`
- `D:\nova\docs\reports\CLAUDE_WP2A_SPEC_REMEDIATION10141421.md`
- 必要源码：
  - `D:\nova\apps\desktop\src-tauri\src\storage\actions.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\tasks.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\recovery.rs`
  - `D:\nova\apps\desktop\src-tauri\src\core_loop.rs`
  - `D:\nova\apps\desktop\src-tauri\src\commands.rs`
  - `D:\nova\apps\desktop\src\stores\coreLoop.ts`

## 原 M1：执行上下文与 CAS

状态：**部分关闭**

已关闭部分：
- `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:30-49` 把 `ActionGate::Started/AlreadySucceeded` 改为携带完整 `TaskAction`，来自 `start_action` / `plan_action` 返回值。
- `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:53-72` 要求 `Database::finish_action` 增加 `expected_attempt`，`UPDATE` 条件追加 `AND attempt_number=?`。
- 源码依据成立：当前 `storage/actions.rs:67-107` 的 `start_action` 已返回 `TaskAction`；当前 `storage/actions.rs:146-179` 的 `finish_action` 确实只校验 `status='started'`，新 CAS 方向正确。
- CAS UPDATE 后 `find_action` 读回的问题：只要 `finish_action` 在 `rows_affected == 0` 时直接返回陈旧错误、不读当前行冒充成功，`rows_affected == 1` 后再读回同一行用于返回 `TaskAction` 可以接受；调用方使用 `finished.attempt_number` 不会重新误归属，因为 CAS 已经保证它等于 `expected_attempt`。

未关闭问题 1：test/build evidence 当前写入顺序会绕过 CAS 目的。

源码依据：
- 当前 `run_tests_for_round` 在多条路径中先写 `test_evidence`，后调用 `finish_action`：
  - 无测试适用：`core_loop.rs:2161-2177`
  - allowlist 拒绝：`core_loop.rs:2183-2205`
  - 取消/实际测试结果：`core_loop.rs:2226-2300`
- 当前 `run_build_for_round` 同样先写 `build_evidence`，后调用 `finish_action`：
  - allowlist 拒绝：`core_loop.rs:2550-2569`
  - 取消/实际构建结果：`core_loop.rs:2591-2660`
- 新规格第 1.4 节仅声明陈旧开发执行不会产出自检/测试/构建证据，见 `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:114-121`；但没有明确 test/build 自己的 action 在 CAS 失败前不得先写 evidence。

影响：
- 如果 test/build attempt 1 启动后输出晚到，而 action 已被重试到 attempt 2，单纯给 `finish_action(expected_attempt=1)` 加 CAS 只能阻止 action 完成状态覆盖；若 evidence 已经在 CAS 前写入，它仍可能带着当前 dev `result_attempt` 写入 `test_evidence` / `build_evidence`。
- completed gate 后续只按 dev `result_attempt` 采纳 evidence；因此陈旧 test/build 输出可能留下会被当前门禁采纳的真实证据行。

最小修正：
- 对 `run_tests_for_round` / `run_build_for_round` 的所有 `ActionGate::Started(action)` 路径，先捕获该测试/构建 action 的 `attempt_number`；
- 命令执行后必须先 `finish_action(..., expected_attempt=test_or_build_attempt)` CAS 成功，再写入可被门禁采纳的 `test_evidence` / `build_evidence`；或新增 evidence 级别的 test/build action attempt 归属并在 completed gate 中同时校验；
- 对 no-tests/no-build、allowlist 拒绝、取消、timeout、passed/failed 全部分支统一适用，不能只覆盖 happy path。

未关闭问题 2：`Err(_)` 统一当 Stale 会吞掉真实错误。

位置：
- `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:95-105`

证据：伪代码 `Err(_) => return Ok(DevTestOutcome::Stale)` 会把 CAS stale、数据库写失败、连接错误、数据损坏、权限错误等全部静默处理。

影响：真实数据库错误会被误报为陈旧执行，任务静默停住，既不前进也不给用户或日志明确错误。

最小修正：
- `finish_action` 对 `rows_affected == 0` 返回专门错误，例如 `NovaError::StaleActionAttempt` 或结构化 `ActionFinishError::StaleAttempt`；
- 调用方只把该专门错误转为 `DevTestOutcome::Stale` / test/build 对应 stale outcome；
- 其他 `NovaError::Database` / `Config` / IO 错误继续向上传播，按真实故障处理。

通过条件：
- 开发、自检、test、build 的所有被门禁采纳产物都只能在对应 action CAS 成功后写入，或带足够归属字段并由 completed gate 校验；
- 陈旧完成与真实错误有可区分的错误类型；
- 验收矩阵增加“test/build attempt 晚到但 dev attempt 未变”的反例。

## 原 M2：需求快照契约

状态：**部分关闭**

已关闭部分：
- `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:136-149` 给出 `confirmed_requirement_items` DDL、唯一约束和索引。
- `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:154-165` 给出四字段拆分规则，包含换行、空行、顺序、重复项、稳定 `(field,item_index)`。
- `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:173-188` 给出不覆盖、事务和幂等规则。
- `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:190-215` 给出 `generate_requirement_snapshot` 命令契约。

仍未关闭问题：`generate_requirement_snapshot` 允许状态与“已确认”前提冲突。

位置：
- `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:202-205`
- 真实前端调用链 `apps/desktop/src/stores/coreLoop.ts:463-478`
- 真实状态机 `storage/tasks.rs:31-36`

源码/调用链依据：
- 前端 `confirmRequirement()` 是用户点击确认后连续调用：
  1. `transition_task(toState: 'awaiting_confirmation')`
  2. `transition_task(toState: 'confirmed')`
- 因此“转入 AwaitingConfirmation”在当前 UI 链路中确实发生在用户点击确认之后，而不是用户确认前的纯展示状态。
- 但 `generate_requirement_snapshot` 契约写“任务必须处于非终态”，这包含 `Draft`、`AwaitingConfirmation`、`Confirmed`、`Queued`、`NeedsUserAction` 等。
- 同一行契约又说命令存在前提是“任务已经在更早的某次 AwaitingConfirmation 转换里被用户确认过需求”，并用“非 Draft 状态”来保证。该保证没有落实到允许状态，直接调用命令仍可对 `Draft` 任务生成快照。

影响：
- 任何后端/前端/测试直接调用 `generate_requirement_snapshot(project_id, draft_task_id)` 都可能把尚未确认的草稿内容固化为 `confirmed_requirement_items`。
- 这会污染 completed gate 的权威 coverage 目标，违背“快照必须对应用户实际确认内容”的要求。

最小修正：
- 将允许状态改为“非终态且非 Draft”，或更严格地列举允许状态：`AwaitingConfirmation`、`Confirmed`、`Queued`、`Running`、`Verifying`、`SafeFixing`、`PreviewReady`、`AwaitingUserAcceptance`、`NeedsUserAction`、`Interrupted`；
- 如果认为 `AwaitingConfirmation` 仍可能由非 UI 路径进入，则补充后端依据：只有 `Draft -> AwaitingConfirmation` 转换本身生成快照，补建命令只允许状态已经离开 Draft；
- 命令实现必须在后端校验状态，不能只依赖“前端先展示并确认”的产品流程约束。

关于旧稿引用：
- 最新规格第 3/4/6/7/8 节仍引用 `10141347` 作为已关闭设计来源，见 `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:219-247`、`251-253`、`339`。
- 对本轮剩余 3 个 Major 来说，这不构成新的阻断，因为用户明确要求不重审已关闭项；但若后续作为唯一实施规格使用，建议把 `10141347` 明确列为“保留的权威附件”，或在最终实施版中内联所有必须编码的 schema/门禁/矩阵。当前 WP2A-0 第 2 节自身已基本自包含。

通过条件：
- `generate_requirement_snapshot` 后端状态校验排除 Draft；
- 明确 Draft / AwaitingConfirmation / Confirmed 的真实语义与命令可调用边界；
- 增加验收用例：Draft 直接调用补建命令必须拒绝，不写任何 item。

## B4 衍生 Major：写入前冻结检查

状态：**已关闭，带 Minor 文字修正**

证据：
- `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:267-286` 要求在两个公共入口 `confirm_no_tests_applicable` / `confirm_no_build_applicable` 最开头检查 `legacy_classification` 冻结，且在调用 `reconcile_...` 前返回。
- 真实可达入口核验：
  - Tauri 命令 `commands.rs:1186-1193` 调用 `core_loop::confirm_no_tests_applicable`
  - Tauri 命令 `commands.rs:1235-1242` 调用 `core_loop::confirm_no_build_applicable`
  - 公共 core_loop 入口当前在 `core_loop.rs:812-819`、`1204-1210`
- 内部 `reconcile_no_build_confirmation` / `reconcile_no_tests_confirmation` 是私有函数；生产可达路径经公共入口进入。顶部冻结检查足以保护内部“写 confirmation -> reset action -> transition”调用链。
- `unblock_test_execution_for_retry` 属于前一版 B4 已列入口；本轮未削弱该要求。

影响：冻结任务在这两个确认入口下不会写 confirmation、不会 reset action、不会新增 state event。

Minor 文字修正：
- `NOVA_WP2A_SELF_CHECK_SPEC10141417.md:288` 称 `run_startup_recovery` 是“只读/记账性质”。源码 `storage/recovery.rs:73-75`、`199-208` 显示它会写入：`mark_started_actions_uncertain()`、`interrupt_all_in_flight_tasks()`、`transition_task(... Interrupted)` 和 recovery checkpoint。应准确表述为“恢复记账写入，不继续执行、不重跑命令/AI”，不要称纯只读。

通过条件：主体已满足；文案把“只读/记账”改为“恢复记账写入”即可。

## 已关闭项是否被破坏

- 历史分类 B1：本轮未改变，未发现破坏。
- 最终 completed 门禁 B2：本轮未改变核心 7 步；但 M1 的 test/build evidence 写入顺序若不修正，会削弱门禁输入可信度。
- 重新授权 B3：本轮未改变，未发现破坏。
- 失败状态、预览分离、拒绝原因 API 返回：本轮未发现破坏。

## 问题清单

### Blocker

无。

### Major

1. **M1 部分关闭：test/build evidence 仍可能在 action CAS 前写入。**
   - 影响：陈旧 test/build 输出可能留下可被 completed gate 采纳的 evidence。
   - 最小修正：所有 test/build evidence 写入必须发生在对应 action `finish_action(expected_attempt)` CAS 成功之后，或 evidence 增加并校验 test/build action attempt 归属。

2. **M1 部分关闭：`Err(_)` 统一当 Stale 会吞掉真实错误。**
   - 影响：数据库/配置错误可能被静默处理。
   - 最小修正：为 CAS 不匹配引入专门 stale 错误类型，其他错误继续上抛。

3. **M2 部分关闭：`generate_requirement_snapshot` 允许 Draft，与“已确认需求”前提冲突。**
   - 影响：可把未确认草稿固化为 confirmed requirement items。
   - 最小修正：后端允许状态排除 Draft，并增加 Draft 直接调用拒绝用例。

### Minor

1. `run_startup_recovery` 不是纯只读；应称为恢复记账写入，不继续执行。
2. 若最终实施版仍引用旧规格承载已关闭门禁/矩阵，需明确旧规格是保留权威附件，避免“唯一权威方案”与旧稿引用冲突。

### Suggestion

- 对 `ActionGate::Started(TaskAction)` 的改动，测试应覆盖 dev/test/build 三类 action，而不仅是 dev。

## 是否允许启动 WP2A 编码

**不允许启动 WP2A 编码。**

允许的下一步最小方案修订：
1. 补齐 test/build action CAS 与 evidence 写入顺序；
2. 区分 stale attempt 与真实错误；
3. 修正 `generate_requirement_snapshot` 后端允许状态，排除 Draft；
4. 将 recovery 描述改为“记账写入，不继续执行”。

