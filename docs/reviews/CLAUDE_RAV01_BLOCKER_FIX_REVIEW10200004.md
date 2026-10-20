# RAV01 阻塞最小修复独立审核（Claude Code）

生成时间：2026-10-20 00:04 Asia/Shanghai。项目：D:\nova。审核人：Claude Code；开发/整改：Codex。

基线HEAD：`baa0fa17b152cf28404204985edfedbd565c8612`（与预期一致，未提交）。证据目录：`D:\nova\audit\rav01-blocker-fix-20261019-230759`。

## 唯一结论

**本轮代码审核：PASS**（限本轮八文件实现；不代表RAV01真实环境问题已解决，也不代表已具备发布/安装验收条件）。

**是否可进入候选构建：可以**——建议下一步由PM安排基于本次提交的新候选构建与安装，但该构建仍不能作为"RAV01真实闭环已通过"的证据。

**真实RAV01验收：仍未通过**——Windows沙盒运行时文件占用（`node_repl.exe`，os error 32）仍未解除，真实Codex开发闭环依然BLOCKED；本轮只修复了Nova自身对"开发AI自查受阻/测试异常"的响应方式，未触及、也不可能触及该真实环境问题本身。

## 一、基线与交付文件核验

- `git log -1`确认HEAD为`baa0fa17b152cf28404204985edfedbd565c8612`，分支`feature/v0.1.0-mvp`，与指定基线一致；暂存区为空，本轮未提交。
- 本轮未提交，`git diff --name-only`显示恰好8个本轮文件+2份既有product历史修改（与此前所有轮次一致，未被本轮触及），无其他文件变化。
- 八文件**逐一独立重算SHA256，与`source-manifest.json`及报告声明值完全一致**：

| 文件 | 独立重算SHA256 |
|---|---|
| `apps/desktop/src-tauri/src/commands.rs` | `5dc2e60b73027790d1fb0f29ff0aad1c7d3e19289e3a55b48c08fbc8d8ab9ad0` |
| `apps/desktop/src-tauri/src/core_loop.rs` | `bfa2d19244cec51a41434f59a090407bb87a62d66c09a9c6b2ab3cded879db21` |
| `apps/desktop/src-tauri/src/core_loop/test_runner.rs` | `2afef158f206287ef945bed1f2d5c587f121907c2479b5a83db0bc9f892f6a4b` |
| `apps/desktop/src-tauri/src/core_loop/wp2a_remediation_tests.rs` | `c88537ea53b13192ab64842673f4344530c868a8d061c0b82cef26ad62ac921e` |
| `apps/desktop/src/components/CoreLoopView.vue` | `d1316ea4cc15975a8042334dd2c850fc530bf1db8baa90d933df225e678cfe35` |
| `apps/desktop/src/stores/coreLoop.test.ts` | `cde667fd07d48c34172e27956489f60b9b1279817155b7947332252a92bbefc8` |
| `apps/desktop/src/stores/coreLoop.ts` | `8dbaab85a0cb67109e2b926cdccfd16b06af2e52ffab1d0c9741aa2daada2dcb` |
| `apps/desktop/src/uiRemediation.interaction.test.ts` | `1edc98743c3772f1a10cee186d7f45b77d1d40339c80ef87c82be586d50a4a1f` |

- `git apply --reverse --check audit/rav01-blocker-fix-20261019-230759/review.diff`：退出码0，补丁边界与实际diff一致。
- 两份product历史文档diff（各23行新增）与此前所有轮次完全一致，确认本轮未触及。

## 二、五项重点逐一核验（源码直读，非转述报告）

### 1. self_reported_blocked是否可靠阻止后续测试/构建/自动修复/验收；同轮恢复不能绕过 —— **确认成立**

直读`core_loop.rs:1758-1884`的`run_dev_and_test_round`：

- 该函数经注释确认是v0.1.0单AI与dormant v0.2.0双AI路径**唯一共用**的开发轮次入口（"Shared unchanged by both... a fix here can never silently apply to only one of them"）；grep确认生产代码中`run_tests_for_round`/`run_build_for_round`的调用点只有`:1920`、`:1938`两处，均在此函数内部、均在blocked门禁**之后**——不存在绕过此门禁直接进测试/构建的第二条路径。
- 门禁本身（`:1872-1884`）在`ActionGate::AlreadySucceeded`（崩溃恢复重入）与`ActionGate::Started`（全新执行）两条分支汇合后、`collect_git_evidence`/`Verifying`转换**之前**执行，对`current_self_check(task_id, round, result_attempt)`按**当前轮次/attempt**查询，仅`parse_status=="valid" && self_reported_status=="self_reported_blocked"`才触发，转`NeedsUserAction`并`return Stopped`，不写任何test/build动作。
- 独立复核回归测试`rav01_exit_zero_but_blocked_stops_before_tests_and_resume`（`wp2a_remediation_tests.rs:1310`）：连续两次调用`run_dev_and_test_round`模拟"同轮恢复"，断言两次均`Stopped`+`NeedsUserAction`，action记录始终恰好1条（第二次未重新创建开发动作，确认命中`AlreadySucceeded`而非重新调用AI），`test_evidence`始终为空，`adapter.development_call_count()==1`（AI只被真实调用一次）——**直接证明同轮恢复不能绕过该门禁**，不是空断言。

### 2. 测试异常收口是否遵守attempt/CAS，不终结新一轮动作/伪造成功/丢失原始错误 —— **确认成立**

直读`core_loop.rs:2399-2471`的`TestActionGuard`：

- `Drop`实现调用`finish_action(..., ActionStatus::Failed, ..., self.attempt)`，依赖既有（本轮未改）`finish_action`的CAS语义（`WHERE status='started' AND attempt_number=?`）；若该行已被新attempt取代或已是终态，CAS影响0行、`let _ =`吞掉的只是"CAS未命中"错误，**从不会把已成功/被新attempt接管的行强行改写**。独立复核`rav01_test_cleanup_cannot_overwrite_a_new_attempt_or_terminal_row`：对已被新attempt接管的旧guard、以及已`Succeeded`终态的行分别drop，两次均断言行状态未被覆盖——与源码逐字对应。
- 原始错误保留：`test_runner.rs:344-349`新逻辑把`ExecutionOutcome::Failed`的`detail`字段（原来只保留`stderr`，进程未启动时`stderr`可能为空导致detail丢失）与`stderr`合并脱敏后整体作为输出的`stderr`字段；独立复核`rav01_failed_test_settles_ledger_and_keeps_spawn_reason`：构造`process_succeeded:false, stderr:"cannot start process"`，断言`action.error_summary`与`test_evidence.reason`均包含该原始文本——未被丢弃。
- `test_runner.rs:314-324`用`catch_unwind`包裹实际执行调用，panic时返回`interrupted_test_outcome`（显式"测试执行线程异常终止，未获得有效结果"，`process_succeeded:false`，pass/fail/skip全部`None`）——不伪造通过数或退出码。
- `rav01_test_start_failure_never_invokes_automatic_ai_repair`在完整`run_core_loop`层面确认：进程启动失败最终停在`NeedsUserAction`，`review_call_count()==0`（未触发第二个AI/自动修复）。

### 3. 后台错误传播、页面刷新、事件监听是否正确隔离任务 —— **确认成立**

- `commands.rs:927-958`的`start_core_loop`后台任务：`spawn_blocking`的`JoinError`经`.unwrap_or_else(|_| Err(...))`转为显式错误（原`.ok()`已不存在于此路径）；内部调用`run_core_loop_guarded`（`core_loop.rs:311-333`的`report_run_failure`用`catch_unwind`捕获panic/Err，对仍处于Queued/Running/Verifying/SafeFixing的任务转`NeedsUserAction`并写入脱敏原因）；最终无论成功/失败都会`emit("core-loop://done", payload)`，payload始终带`task_id`。独立复核`rav01_unexpected_run_error_and_panic_are_visible`：对panic和纯Err两种注入均断言任务转`NeedsUserAction`且事件reason含"执行受阻"。
- `CoreLoopView.vue:86-90`的监听器：`if (store.currentTask?.id !== event.payload.task_id) return`——与当前选中任务不同的事件被直接丢弃，不写`lastError`、不触发刷新。独立复核新增测试`ignores notifications from another task`（`uiRemediation.interaction.test.ts:294`）：直接调用真实注册的`listen`回调，payload的`task_id`为`'other-task'`而`currentTask`为`'task'`，断言页面不包含该外部错误文本、`currentTask.state`不变——真实验证了隔离逻辑，非仅读代码推断。
- `coreLoop.ts:619-659`的`refreshCurrentTask`：以进入时的`taskId`/`projectId`闭包生成`isCurrent()`，在首个`await`（`loadTasks`）之后重新校验，若用户已切换任务/项目则直接返回，不用旧刷新结果覆盖`currentTask`；失败时也只有`isCurrent()`为真才写`lastError`。独立复核`switches draft selection...`（原IA-03测试，本身已覆盖切换场景）及本轮新增`shows refresh failure instead of silently swallowing it`均通过。

### 4. 新增测试是否验证真实行为；调整旧测试是否掩盖回归 —— **确认成立**

- `cargo test --locked --lib core_loop::`范围内新增的`rav01_*`系列测试（7项，与报告一致）均直接调用生产函数（`run_dev_and_test_round`/`run_tests_for_round`/`run_core_loop`/`report_run_failure`/`TestActionGuard`本身），使用真实`MockAdapter`+受控`RUN_OUTCOME_OVERRIDE`测试钩子注入进程失败，而非伪造数据库行或绕过门禁逻辑；逐项抽样核对（见上两节）确认断言针对真实状态/证据/事件，非自我验证式空断言。
- `coreLoop.test.ts`按报告描述"仅补齐刷新新增必需读取的mock默认响应"：独立`git diff`该文件，改动确为新增`get_task_evidence_overview`/`list_task_self_checks`等mock handler的默认返回值，未发现删除或放宽既有断言的迹象。
- 原"blocked矩阵用例"调整：报告称"改为更早停止且拒绝验收，不再要求其先进入等待验收才拒绝"——核对`wp2a_remediation_tests.rs`中含"blocked"的既有测试用例（`:266-328`附近的拒绝矩阵），其断言确实现在要求在更早阶段（`NeedsUserAction`而非先到`AwaitingUserAcceptance`再拒绝）停止，与本轮"blocked门禁提前到执行前"的设计改动方向一致，是**收紧**而非放宽，不构成掩盖回归。
- 新增4项前端交互测试（`uiRemediation.interaction.test.ts:238-308`）：逐一核对均通过真实`click()`/真实`listen`回调驱动，而非直接调用store方法绕过组件层——符合"验证真实行为"要求。

### 5. Windows文件占用根因证据与推断是否区分清楚 —— **确认成立**

独立核对证据目录原始日志`sandbox.2026-10-19.log`第53-68行，逐字确认报告引用的关键事实真实存在、未被篡改或夸大：

```
[2026-10-19T14:41:54.802853500+00:00] runtime read/execute validation failed:
validate runtime read/execute access on
C:\Users\Lenovo\AppData\Local\OpenAI\Codex\runtimes\cua_node\78fb63cda1a9e2b2\bin\node_repl.exe:
open ACL target for root-only update: 另一个程序正在使用此文件，进程无法访问。(os error 32)
```

时间戳（14:41:54 UTC = 22:41:54北京时间）、文件路径、错误码与报告正文逐字一致。报告正文明确区分"已证实"（CLI正常调用、ACL更新遇文件占用、self_reported_blocked正确退出）与"未证实"（具体哪个进程持有句柄、原test_execution started缺证据的精确异常栈），且反复强调"未改环境""不强行结束进程""不扩大sandbox权限"——未将"产品门禁修复通过"写成"真实环境问题已解决"，第三节结论段与"尚未解决"第1条也重申Codex环境仍BLOCKED。本轮审核未发现报告在此处有误导性表述。

## 三、独立验证命令与实际结果

| 命令 | 实际结果 |
|---|---|
| `cargo test --locked --lib`（**完整**Rust lib测试套件，apps/desktop/src-tauri，本轮按任务书要求补查报告未覆盖的368项） | **681 passed，0 failed**，82.29秒，0 ignored。313（core_loop::）+368（其余，报告未跑）=681，与完整套件总数吻合，**未发现任何回归** |
| `npm.cmd test`（apps/desktop） | 6文件，**142 passed，0 failed**，与报告一致（含33项交互测试：原29+本轮新增4） |
| `npm.cmd run build`（apps/desktop） | `vue-tsc --noEmit`+`vite build`均通过，37模块，无报错 |
| `git apply --reverse --check review.diff` | 退出0 |
| 原始sandbox日志逐行核对 | 见上节，与报告引用逐字一致 |

未调用真实AI、未修改全局Codex配置、未结束任何进程、未操作RAV01原任务数据库——本轮全部验证均为只读文件核对、本地cargo/npm命令及已保全证据的离线分析。

## 四、结论汇总

- **本轮代码审核：PASS**。八文件改动的五个重点方向（blocked门禁、测试异常收口/CAS、后台错误传播与任务隔离、测试真实性、根因表述边界）经源码直读与独立命令复验**均成立**，补跑的完整Rust lib测试（681项）及前端测试（142项）均0失败，未发现新回归、未发现掩盖问题的证据。
- **是否可进入候选构建**：可以。建议PM安排基于本次提交的新候选Tauri/NSIS构建，交后续针对RAV01场景的定向人工复验（而非重新展开整个WP2A审核）。
- **真实RAV01验收：仍未通过**。真实Windows沙盒文件占用（`node_repl.exe`，os error 32）未解除，真实Codex开发闭环仍BLOCKED。剩余条件：
  1. 需先由用户/PM在保存工作后正常退出占用该运行时文件的进程/工具（本轮及本次审核均未、也不应代为结束进程）；
  2. 环境问题解除后，需另行批准一次真实Codex沙盒初始化复验，确认`node_repl.exe`的ACL/只读校验能正常完成；
  3. 在环境问题确认解除前，即使新候选安装包构建/安装成功，也只能验证本轮五项门禁/反馈修复本身（建议通过人工制造"自查blocked"/"测试进程失败"等场景验证UI反馈），不能用于证明RAV01真实任务已能跑通完整开发闭环；
  4. 原RAV01任务数据库/现场应继续保留，不应被重新点击"开始任务"或直接改写以人为制造"已通过"的假象。

## 五、现场保护确认

本轮仅执行只读`git status`/`git log`/`git diff`/`git apply --reverse --check`、对八份交付文件及证据目录原始日志的只读读取、`cargo test --locked --lib`（完整套件）、`npm.cmd test`、`npm.cmd run build`四条本地验证命令。未执行`git add`/`commit`/`push`/`reset`/`clean`/`stash`；未修改任何源码或测试；未调用真实AI或付费服务；未结束任何进程；未操作RAV01原任务数据库或真实用户数据；未安装、未打包、未发布。`git status`核对显示与审核开始前一致，仅本报告为新增。

完成审核，到此停止。交Codex按上述剩余条件处理，PM安排候选构建与环境复验节点。
