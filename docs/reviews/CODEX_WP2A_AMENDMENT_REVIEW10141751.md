# CODEX WP2A Amendment Review 10141751

生成时间：2026-10-14 17:51 Asia/Shanghai  
项目：`D:\nova`  
复审类型：仅复审最新补充规格对剩余三项 Major 的关闭情况，以及补充稿直接引入的规则冲突。未运行测试/构建，未运行 AI CLI，未读取真实用户数据库。

## 最终结论

**PASS WITH NOTES**

允许按当前适用规格启动 WP2A 编码。  

这是方案层面的准入结论，不代表功能已实现或运行验收通过。实现后仍必须通过迁移、状态机、并发/恢复、证据归属、快照补建与门禁测试。

## 仓库状态核验

- 根目录规则文件：未发现 `AGENTS.md`、`CLAUDE.md`、`README`、`CONTRIBUTING`。
- 当前分支：`feature/v0.1.0-mvp`
- 当前 HEAD：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`
- `milestone/evidence-gate^{}`：`6a90df951c6c72ab7ae95228d5f2cc4de7e17273`
- `git status --short --untracked-files=all`：非空，既有修改和未跟踪文件保留；未切换、未清理、未回退。

## 已读材料

- `D:\nova\docs\reviews\CODEX_WP2A_SPEC_FINAL_CLOSURE_REVIEW10141613.md`
- `D:\nova\docs\release-v0.1.0\NOVA_WP2A_FINAL_AMENDMENT10141726.md`
- `D:\nova\docs\reports\CLAUDE_WP2A_FINAL_AMENDMENT_REPORT10141728.md`
- 按需核对：
  - `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141417.md`
  - `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141347.md`
  - `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141420.md`
  - `D:\nova\apps\desktop\src-tauri\src\core_loop.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\actions.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\tasks.rs`
  - `D:\nova\apps\desktop\src-tauri\src\storage\recovery.rs`
  - `D:\nova\apps\desktop\src\stores\coreLoop.ts`

## 1. 测试/构建 CAS 与证据写入

状态：**已关闭**

依据：
- 补充稿 `NOVA_WP2A_FINAL_AMENDMENT10141726.md:29-36` 明确区分开发 `result_attempt` 与 test/build action 自身 `test_action_attempt` / `build_action_attempt`，并要求两者都校验。
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:40-45` 覆盖全部分支：无测试/无构建适用、allowlist 拒绝、取消、timeout、passed/failed，统一改为 `finish_action(expected_attempt)` CAS 成功后才写入可被门禁采纳的 evidence。
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:61-74` 给出 test 代表性伪代码，`StaleActionAttempt` 时不写 evidence、不 transition；build 同构。
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:79-81` 保留 CAS 成功但 evidence 写入前崩溃的 fail-closed 语义：下次 `AlreadySucceeded` 找不到 evidence 时写 `insufficient_evidence` 并进入 `NeedsUserAction`，不假造证据、不自动重跑。
- 源码核验确认原问题真实存在：当前 `run_tests_for_round` 和 `run_build_for_round` 多条分支确实先写 evidence 后 `finish_action`，补充稿针对的正是正确位置。

影响：补充后，重点反例“dev attempt 未变，test/build attempt1 输出晚到，而对应 action 已进入 attempt2”在方案层面已关闭：旧输出 CAS 不匹配，只返回 Stale，不写当前有效证据。

通过条件：已满足方案准入。实现阶段必须为 test/build 每个 evidence 写入分支补测试，不只覆盖 happy path。

## 2. Stale 与真实错误

状态：**已关闭**

依据：
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:98-107` 定义专门 `NovaError::StaleActionAttempt`，且只由 `finish_action` CAS 未命中产生。
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:113-127` 明确数据库执行错误通过 `?` 正常传播，`rows_affected == 0` 才返回 `StaleActionAttempt`。
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:130-155` 给出 dev/test/build 统一调用方规则：只有 `Err(NovaError::StaleActionAttempt(_))` 转为对应 `Stale` outcome，其他 `Config/Credential/Database/Network/Api` 全部上抛。

影响：不再有 `Err(_) => Stale` 吞掉数据库、IO、配置等真实错误的规则冲突。

通过条件：已满足方案准入。实现阶段至少需要两个测试：CAS 不匹配得到 Stale；模拟真实数据库/写入错误时错误向上传播，不被当作 Stale。

## 3. 快照补建状态

状态：**已关闭**

依据：
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:161-164` 独立核验了真实前端确认链路：`confirmRequirement()` 先转 `awaiting_confirmation` 再转 `confirmed`；`AwaitingConfirmation` 是用户点击确认后的状态，不等于 Draft。
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:170-183` 明确 `SNAPSHOT_ALLOWED_STATES`，显式排除 `Draft` 和 `Completed/Failed/Cancelled`。
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:185-201` 要求状态校验先于快照幂等返回。
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:204` 明确即使 Draft 因未来 bug 已有快照，也必须先拒绝，不得幂等返回。
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:214-225` 覆盖 Draft 拒绝且零写入、AwaitingConfirmation、Confirmed、终态拒绝、重复调用、冻结任务不解冻。

影响：原“非终态包含 Draft”的冲突已关闭；补建命令不会把未确认草稿固化成 confirmed requirement items。

通过条件：已满足方案准入。实现阶段需确保状态校验在事务内且发生在读取/返回已有快照之前。

## 4. 权威关系与文字

状态：**PASS WITH NOTES**

依据：
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:8-17` 明确补充稿不是整体替代，而是对第 1/2/3 节具体机制的追加/覆盖；被补充稿明确覆盖的规则以补充稿为准。
- 同节明确 `10141417.md` 与 `10141420.md` 继续有效，`10141347.md` 只作为被 `10141417.md` 引用的保留权威附件。
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:17` 明确不得恢复已作废规则，包括 self_checks 覆盖写入、user_acceptance 加 round、历史分类错误单 AI 推断。
- `NOVA_WP2A_FINAL_AMENDMENT10141726.md:232-236` 已把 `run_startup_recovery` 修正为“恢复记账写入”，会标记 action 为 uncertain、转任务为 Interrupted 并记录 checkpoint，但不继续执行、不重跑命令/AI。

Notes：
- “10141347 作为保留权威附件”的写法可实施，但实现者必须严格遵守补充稿覆盖优先级，不能从旧稿恢复任何被 `10141417` 或补充稿替代的规则。
- 建议编码工作包入口文档列一个“适用规格顺序”：`10141726` 覆盖三项 Major 规则；其余按 `10141417` / `10141420`；仅在这些文件明确引用时查 `10141347` 对应章节。

## 三项状态汇总

- 测试/构建 CAS 与 evidence 写入：**已关闭**
- Stale 与真实错误：**已关闭**
- 快照补建状态：**已关闭**

## 是否允许启动 WP2A 编码

**允许按适用规格启动 WP2A 编码。**

边界：
- 只允许实现 WP2A-0 与 WP2A 主体中已审定的结构化自检、需求快照、执行归属、CAS、冻结、门禁与对应测试。
- 不代表功能运行验收通过。
- 不应混入 Phase5A、GitHub 搜索/导入、双 AI 启用、全站 UI 改造、无关重构。

