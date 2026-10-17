# WP2A 隔离验证设施说明

生成时间：2026-10-15T16:53:24+08:00（Asia/Shanghai）。

项目 `D:\nova`；分支 `feature/v0.1.0-mvp`；HEAD / `milestone/evidence-gate^{}` 均为 `6a90df951c6c72ab7ae95228d5f2cc4de7e17273`。无配置 remote；本轮未提交，不存在新的交付 SHA。

证据根目录：`D:\nova\audit\wp2a-desktop-validation-20261015-1533`。最终运行：`run-07`，早期失败/中间运行全部保留。

## 用途及发布边界

仅为这次 WP2A 已通过代码复审后的运行验证服务，debug-only 非默认 feature。正常入口、真实用户数据、真实 AI 与付费服务均不用于本设施。没有扩展产品能力。新增设施尚待 Claude 独立复审。

三处既有文件改动：Cargo.toml 声明非默认 feature 和两个 required-feature 测试 bin；lib.rs 添加 debug-only 分支、独立 window/setup 和真实 handler 外层验证分派，正常 handler 命令名单保留；runtime/process.rs 仅在 feature 下执行进程固定路径/hash/cwd 策略。四个新增源码为 isolated_validation.rs、两个入口和 DOM driver。没有改变前端源码、数据库 DDL、WP2A 门禁、授权或 CAS 业务规则。

release 启用本 feature 故意 compile_error；普通构建不包含 validation_control 或注入脚本，NSIS 不打包假 CLI。调试入口也必须先显式 --root 初始化，不能偶然走真实 app-data。测试 handler 只在隔离进程中存在，其中 seed/advance_generation/break_reads 是造数和故障注入，不能用于正常发布。

## 路径及执行保证

根必须是不存在的绝对目录，其现存父目录名以 wp2a-desktop-validation- 开头，并拒绝祖先 reparse/symlink。prepare canonical 化根；数据库、WebView、项目、假 CLI 全在该根。打开连接前写 ISOLATED-VALIDATION.json，实际启动后记录 startup-proof.jsonl。不能把已有 run 根重新拿来启动；每次独立新库。

NoCredentials 不访问 OS 凭据；外部 API/聊天/配置任意修改命令被测试分派器拒绝。真实生产 IPC 的 WP2A 命令仍调用原生成 handler。process_policy 在启动前只允许 pinned fake 或 Git、隔离 cwd，且每次检查 fake SHA。fake CLI 必须具备源码 marker，复制后固定哈希，返回 --version/--help 的兼容探针响应；真正开发响应只从实际任务包需求 item 推导。它没有网络和 spawn 实现。

本地 Git 使用空全局配置和禁用 system 配置。Windows canonical 的 \\?\ 前缀在给 Git 的配置路径和证据比较时按实际 Windows 路径语义处理。app-data 隔离不依赖这些环境变量。

## 复现步骤（仅审计隔离目录）

1. 核对 source-delivery-manifest、原审核交付与当前分支状态。不要清理混合工作区。
2. 在 D:\nova 运行 `npm --prefix apps/desktop run dev`，确认是本地 127.0.0.1:1420；单独运行后端 `cargo build --features wp2a-desktop-validation --bin nova_wp2a_validation --bin nova_wp2a_fake_cli`（cwd apps/desktop/src-tauri）。
3. 在本证据根下选择**尚不存在**的 run 名，如 run-claude-01；直接运行 target/debug/nova_wp2a_validation.exe，显式参数 `--root <全新绝对目录> --fake-cli D:\nova\apps\desktop\src-tauri\target\debug\nova_wp2a_fake_cli.exe --git "C:\Program Files\Git\cmd\git.exe"`。先检查 marker / startup proof 实际路径，若不隔离立即停止。不要运行正常 nova.exe。
4. 注入脚本自动通过原 DOM 表单操作；各 checkpoint 暂停最多 180 秒，写入 DOM/数据库快照。在有可用图形桌面的环境通过真实窗口采集截图后，在该 run 根写 ack-<checkpoint-name> 空文件继续。截图文件放同一审计根并记录对应停点。
5. 若只复现无截图的自动化，可显式执行 `ack_checkpoints.ps1 -RunRoot <新 run 根>`。它只在已有 isolated marker 的根写 ack，不能把此操作当作截图通过。本次 run-07 由同等 ack 轮询以及该脚本结束剩余停点。
6. 最终读取 driver-result.jsonl，必须 completed=true 且九组 passed=true；保留失败日志，不放宽生产断言。退出只关本次 validation 进程和对应 Vite。按需将 verify_evidence.py 的 RUN 显式改为本次新 run 名后执行，它只以 mode=ro 打开该审计数据库，生成前后核对与来源补丁。不要改成真实数据库路径。

## 夹具与恢复边界

fixtures.json 标注所有 synthetic task 来源；无需测试/构建授权预置仅说明无适用测试/构建，不生成虚假“真实测试通过”。Case C / old / frozen / generation 夹具包含明确 synthetic action/evidence。最终四次开发为假进程真实 stdout 管道，不是纯前端 invoke mock。

DOM driver 使用原 button/textarea 事件及真实 Tauri IPC；validation_control 只用于快照、停点、延迟和隔离数据库故障/代际注入，不返回伪造的业务预检或自检响应。冻结取消使用已有 production transition_task IPC，因为原页面该状态无通用取消按钮。真实原生输入和像素布局仍须另验。

本轮未回滚任何现场。若后续决定移除设施，应先核验 this-turn-source.patch 和本轮文件哈希，仅逆向本轮三处接缝/四个新增源码，并保留其他开发修改；不以 checkout 整文件、stash 或清理未跟踪目录作为方案。日志/测试库独立保留至审核完成。没有真实数据库恢复步骤，也不复制或恢复真实用户数据库。

## 验证、缺口与交接

实际执行：默认后端674、前端109、设施3项通过；最终真实桌面 DOM/IPC 九组通过，17停点；证据26断言通过；正常 release 和完整 NSIS 构建成功，feature release 编译预期拒绝。截图0张，原生输入/视觉/VM安装 NOT RUN；失败信息保留 screenshots/NOT_CAPTURED.md。构建不等于安装或启动已验证。warning 与早期失败见运行报告。

完整结果和 Claude 索引：`D:\nova\docs\reports\CODEX_WP2A_DESKTOP_VALIDATION_REPORT10151653.md`。精确源文件SHA和本轮补丁：source-delivery-manifest.json、this-turn-source.patch；完整交付文件哈希：handoff-manifest.json。未提交，无新 SHA；不得沿用原 Claude PASS 为新增设施背书。

补充交付核验：`git apply --reverse --check audit/wp2a-desktop-validation-20261015-1533/this-turn-source.patch` 返回 0，仅检查未执行撤销；补丁统一使用 LF，避免 Windows 换行导致不可核验。最终临时库另通过 SQLite online backup 保存为 `run-07/database-after-consistent.sqlite3`，只涉及审计临时数据库，不涉及真实用户数据。handoff-manifest 的明确范围排除可变 WebView 缓存及其自身，正常构建产物另由 build-artifact-hashes 固定；其余审计文件和本轮源码、交接文档均列 SHA-256。
