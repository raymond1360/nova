# RAV01 实测阻塞最小修复交付

生成时间：2026-10-19T23:42:38+08:00（Asia/Shanghai）。交 Claude Code 独立审核；未提交、推送或构建安装包。

## 结论与基线

已完成 Nova 内部的 blocked 自查门禁、测试异常收口/错误保留、后台错误传播和页面刷新反馈修复，并通过相关本地检查。真实 Codex Windows 沙盒的文件占用问题仍未解除，真实闭环仍 BLOCKED；没有自动重试原任务或宣称业务验收通过。

仓库 D:\nova，分支 feature/v0.1.0-mvp，HEAD `baa0fa17b152cf28404204985edfedbd565c8612`。初始仅两份 product 历史修改，全部保留；本轮未碰产品规格文档、数据库 schema、依赖/锁文件、workflow 或正式 Tauri 配置。没有 reset/clean/stash、切换分支、暂存或提交。仓库及适用目录未发现 AGENTS.md/CLAUDE.md。测试目标 D:\NovaRAV01Project 未修改，基线仍 0dd25abe8b0f837ef7f0764e1ab97cb1483a0278，Git 干净。

## 先行证据保全

独立证据目录：`D:\nova\audit\rav01-blocker-fix-20261019-230759`。

修复前通过 SQLite URI mode=ro + PRAGMA query_only=ON，仅导出 RAV01-title 对应任务及关联 actions、state_events、timeline_events、command/test/build evidence 和 self_checks，未更新用户数据库。另复制本次相关 Codex .sandbox 的 sandbox-3c353afad1fa1387.log、sandbox.2026-10-19.log、setup_error.json；不读取认证/密钥文件，也不访问 .sandbox-secrets。原件全部保留。

原始快照清单、字节数和 SHA256 在 original-evidence-sha256.json。原始 audit 可能含项目需求、会话标识及本地路径，仅本地保留，不提交公开仓库、不上传；报告只给必要脱敏结论。SQLite 业务记录保存为定向查询快照，没有复制整个日常用户库或其他任务/凭据。

## 根因：已证实与尚未证实

### 1. Codex Windows 执行环境初始化失败（已证实，未改环境）

实际调用为日常 npm Codex 0.162.1：codex.cmd exec --sandbox workspace-write -C D:\NovaRAV01Project --skip-git-repo-check；Nova OS cwd 与 -C 一致，提示通过 stdin 输入。CLI 已接收任务并返回自查，不是找不到 Codex、错误项目路径或登录失败。没有证据要求修改参数或降低 sandbox，本轮未改 adapter 参数。

沙盒日志在北京时间 22:41:51 记录 offline firewall settings changed 并刷新 setup；22:41:54 的确定失败是 runtime read/execute validation 无法对 `C:\Users\Lenovo\AppData\Local\OpenAI\Codex\runtimes\cua_node\78fb63cda1a9e2b2\bin\node_repl.exe` 打开 ACL 更新目标，Windows **os error 32（另一个程序正在使用此文件）**。22:47:47 setup refresh 汇总该错误并退出 1，setup_error.json 为 helper_unknown_error / setup refresh had errors。

本轮只读进程表也发现同一路径的 node_repl.exe 进程存在；没有声称已定位哪个句柄持有者，更没有强行结束它们。此前初始化器的 ACL/防火墙操作属于原真实 CLI 调用日志，不是本轮实施的环境改动。

Codex 于 22:49:06 正常退出 0，但输出明确 self_reported_blocked、未改文件且未运行测试/构建。CLI 会话完成不等于开发完成，错误必须作为产品停点传播。无需凭此认定 token/登录有问题，未执行新登录或真实 AI。

### 2. blocked 自查没有阻止验证（已证实，已修复）

旧 run_dev_and_test_round 以 ExecutionOutcome::Success 结束开发动作，写入自查后无条件进入 verifying；blocked 仅在最终完工门禁检查。现增加执行前捕获 attempt 对应的 current_self_check 检查：valid + self_reported_blocked → NeedsUserAction，保存脱敏受阻原因，直接停止；不创建 test/build 动作，不进自动修复，不调用第二个 AI。AlreadySucceeded 的同一轮恢复也重新应用此门禁，不重新调用开发 AI。

Action Ledger 中开发动作 succeeded 仍表示“该次 CLI 调用正常结束”，不被当作任务完成；blocked 自查及 needs_user_action 共同说明实际业务结果。本轮不重写历史动作，也不放宽 existing attempt/CAS/代际及后端完工门禁。invalid/incomplete 等既有完工约束保留，不把自查当独立测试通过证明。

### 3. 测试 started 无结果与异常丢失（代码缺口已修复，历史精确停点未知）

原记录在 22:49:07 留下 test_execution started，但没有 test_evidence；进程快照没有相应测试子进程。现有日志不足以证明其当时是创建前阻塞、线程 panic、数据库错误还是其他异常，不能将缺失记录直接说成“测试失败”。

可确认代码缺口：start_core_loop 把数据库打开失败、run_core_loop Err 和 JoinError 通过 .ok() 静默丢弃；test_runner 仅保留进程 stderr，可能丢掉未创建进程时只存在于 ExecutionOutcome 的错误 detail；异常路径没有 dispatch 级收口保护。

修复为：

- 测试进程的失败 detail 经脱敏合入 stderr；进程边界 unwind 转为明确失败结果，不伪造退出码或通过数。
- 保存失败 action error_summary 与 test_evidence.reason；启动/执行器异常（无退出码）转 NeedsUserAction，**不触发自动 AI 修复**。真实断言失败仍保持原有验证/修复规则，超时与取消仍区别记录。
- TestActionGuard 在测试动作作用域提前退出时，用捕获的 attempt CAS 结束自身 started。不能改新 attempt 或已终结记录；失败收口不产生成功证据。数据库持续不可写或进程硬崩溃仍不能承诺落库，原重启 uncertain 恢复机制保留。
- 后台执行边界捕获可恢复 Rust unwind/Err，将仍在运行阶段的任务转需要用户处理；命令层发送 scoped core-loop://done error，前端接收并显示。不再静默 .ok()。

本轮没有为原 started 行直接补写结果；对于无返回的外部/OS 阻塞，guard 也不能凭空证明终结。现有正常进程超时机制未移除；硬崩溃/不可恢复阻塞仍依赖恢复与人工判断。

### 4. 刷新按钮与可见反馈（已修复）

旧“刷新时间线与证据”只 loadEvidenceOverview，不更新 currentTask，任务标题可能仍显示旧状态；读取错误被吞掉，用户看不到停点。自动轮询在用户现场为何未及时更新，缺少当时前端控制台记录，不能作更强归因。

按钮现在刷新 task、时间线、证据及自查；显示已刷新反馈，任务/证据读取失败显示错误，不假称已成功。选择切换时不会把原刷新任务状态写到新选择。后台结束/错误事件按 task_id 过滤，显示错误并只读刷新；组件卸载清理 listener。最近一次 blocked 自查原因在主区域可见，不要求展开技术详情。未增加第二 AI 产品门槛。

## 修改文件与唯一交付哈希

以下八文件为本轮未提交实现；review.diff 仅含这八文件，不夹带历史 product 修改。逐文件 SHA256 对应工作区原始字节，保存 source-manifest.json。保留原有主要文件 CRLF，不调整全局 Git 配置。

| 文件 | SHA256 |
| --- | --- |
| `apps/desktop/src-tauri/src/commands.rs` | `5dc2e60b73027790d1fb0f29ff0aad1c7d3e19289e3a55b48c08fbc8d8ab9ad0` |
| `apps/desktop/src-tauri/src/core_loop.rs` | `bfa2d19244cec51a41434f59a090407bb87a62d66c09a9c6b2ab3cded879db21` |
| `apps/desktop/src-tauri/src/core_loop/test_runner.rs` | `2afef158f206287ef945bed1f2d5c587f121907c2479b5a83db0bc9f892f6a4b` |
| `apps/desktop/src-tauri/src/core_loop/wp2a_remediation_tests.rs` | `c88537ea53b13192ab64842673f4344530c868a8d061c0b82cef26ad62ac921e` |
| `apps/desktop/src/components/CoreLoopView.vue` | `d1316ea4cc15975a8042334dd2c850fc530bf1db8baa90d933df225e678cfe35` |
| `apps/desktop/src/stores/coreLoop.test.ts` | `cde667fd07d48c34172e27956489f60b9b1279817155b7947332252a92bbefc8` |
| `apps/desktop/src/stores/coreLoop.ts` | `8dbaab85a0cb67109e2b926cdccfd16b06af2e52ffab1d0c9741aa2daada2dcb` |
| `apps/desktop/src/uiRemediation.interaction.test.ts` | `1edc98743c3772f1a10cee186d7f45b77d1d40339c80ef87c82be586d50a4a1f` |

主要定位：core_loop.rs 的 run_core_loop_guarded/report_run_failure、约 1869 行自查门禁、TestActionGuard/run_tests_for_round；test_runner.rs run_tests；commands.rs start_core_loop 后台执行段；CoreLoopView.vue handleRefreshTask、core-loop://done 监听及受阻提示；stores/coreLoop.ts refreshCurrentTask。

Claude 可使用 `git diff -- <上述八文件>` 或 `D:\nova\audit\rav01-blocker-fix-20261019-230759\review.diff` 复核。新增回归在 wp2a_remediation_tests.rs、test_runner.rs tests、uiRemediation.interaction.test.ts；coreLoop.test.ts 仅补齐刷新新增必需读取的 mock 默认响应，未降低已有断言。原 blocked 矩阵用例改为更早停止且拒绝验收，不再要求其先进入等待验收才拒绝。

## 实际本地验证

| 命令及目录 | 实际结果 |
| --- | --- |
| cargo test --locked --lib rav01 -- --nocapture（apps/desktop/src-tauri，新增端到端用例前） | 6 通过；包含故意注入并捕获 panic，日志中的 injected failure 为预期测试输入 |
| cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib core_loop::（D:\nova，最终） | **313 通过，0 失败**，退出 0；覆盖新增七项 Rust 回归及现有核心/证据/门禁回归 |
| npm.cmd test（最终 D:\nova 根脚本调用 desktop 测试） | **142 通过，0 失败**，退出 0；四项新增交互测试覆盖状态、blocked 提示、刷新失败、后台错误及其他任务事件隔离 |
| npm.cmd run build（apps/desktop，最终） | vue-tsc --noEmit + Vite 构建通过，退出 0；仅前端本地构建，非 Tauri/安装包 |
| git -c core.whitespace=cr-at-eol diff --check | 通过；仅本次命令将仓库原有 CRLF 视为行尾，未改持久配置 |

首次前端检查因旧 mock 未提供证据读取而失败，补全 mock 后完整通过；首轮 Rust 核心回归中旧 blocked 矩阵仍期待进入 Verified，按更严格新语义调整后最终全过。期间有一次误在仓库根运行未指定 manifest 的 cargo 命令，未执行测试；已使用上表正确命令补验，不算通过结果。

最终日志：rust-core-loop-final.log、frontend-tests-final.log、frontend-build-final.log。Vue 测试仍有原有 p 包含 ul 的三处模板警告，不属于本轮新增，构建/类型检查通过；未顺手重构 UI。

NOT RUN：全部其他 Rust 测试（本轮仅核心相关 313 项，其余 368 项未跑）、新的真实 Codex 调用、Windows 沙盒初始化复试、正式 Nova UI 人工复验、Tauri 安装包构建、CI。没有调用付费接口或测试/构建真实 RAV01 项目；Rust 集成测试只使用各自隔离临时夹具与 mock/受控子进程。

## 尚未解决与下一步

1. **Codex 环境仍 BLOCKED**：现有桌面运行时文件占用未解除，不改全局 Codex 配置、认证、ACL、防火墙或 sandbox 模式。后续应由 PM 安排在保存工作后正常退出占用相关运行时的工具，再另行批准一次环境检查/复验；本轮不结束进程、不升级 CLI，不建议关闭 sandbox 或扩大权限作为修复。若后续仍要求全局环境/权限变更，应先给具体证据及范围审批，不在此轮实施。
2. 原测试 started 的精确异常/阻塞栈没有现场证据，本轮不伪造结论。新增传播/收口回归通过，不等于已复现并消除原 Windows 环境错误。
3. 原安装候选仍运行旧实现；本轮源码未提交或打包，新反馈不会凭空出现在已安装程序中。应先交 Claude 独立审核，再由 PM 安排后续候选构建、安装及真实复验；不要立即重新点击原任务开始。
4. 不直接修改原任务数据库、状态、动作或测试项目来展示成功，不恢复 AppData，不重新执行真实任务。原任务现场及备份保留。

## 保护与回退

最终保护检查：`{"head_unchanged": true, "historical_product_diff_unchanged": true, "existing_status_entries_preserved": true, "index_empty": true, "test_project_head_unchanged": true, "test_project_clean": true}`。产品两份历史修改、历史未跟踪资料均保留；暂存区为空、HEAD/分支不变。没有 commit、push、main 更新、tag、release 或发布。

回退仅针对本轮八文件 diff，由审核者反向应用 `D:\nova\audit\rav01-blocker-fix-20261019-230759\review.diff` 的相应补丁；先确认此后没有新编辑，不能整文件覆盖或回退 product/历史资料。原 audit 日志和备份保留，不提交到公开仓库。本报告也保持未提交。

完成本轮本地实现与检查后停止，交 Claude Code 独立审核及 PM 安排环境处理。
