# WP2A 隔离桌面运行验证报告

生成时间：2026-10-15T16:53:24+08:00（Asia/Shanghai）。

项目 `D:\nova`；分支 `feature/v0.1.0-mvp`；HEAD / `milestone/evidence-gate^{}` 均为 `6a90df951c6c72ab7ae95228d5f2cc4de7e17273`。无配置 remote；本轮未提交，不存在新的交付 SHA。

证据根目录：`D:\nova\audit\wp2a-desktop-validation-20261015-1533`。最终运行：`run-07`，早期失败/中间运行全部保留。

## 结论与边界

真实 Tauri / WebView2 debug 桌面运行时已启动，实际 Vue 页面、生产命令注册、Tauri 包装与业务函数链路已运行。最终 DOM/真实 IPC 自动化 9 组场景全部通过，17 个停点保留完整页面文本与数据库快照；只读证据复核 26 项通过。这里的“通过”仅指具体断言，不是 Codex 独立代码审核 PASS，也不批准发布。

**截图 0 张；原生鼠标/键盘交互、像素布局及安装后运行 NOT RUN。不能宣布完整桌面视觉验收通过。** `@oai/sky` 能枚举真实 Nova 窗口并取得 UIA 文本，但截图先后报 `FrameArrived timed out`、`window capture timed out`，激活报 `failed to activate captured window`，最大化点击报 `coordinate input geometry is unavailable`。重选窗口和正常窗口启动均未解决。详情见 `screenshots/NOT_CAPTURED.md`；UIA 文本和 DOM 文本没有冒充截图。需要 Claude 在具备可捕获交互桌面的隔离会话补充截图与原生输入验证。

本轮未重新整改已关闭 WP2A 业务问题，没有 Phase5A、双 AI 启用或无关 UI 改造。Claude 原 PASS 只覆盖原交付；本轮设施、启动改动和证据须另行独立审核。

## 现场与原交付保护

已读取用户指定的 Claude `CLAUDE_WP2A_REMEDIATION_REVIEW10151457.md`、Codex `CODEX_WP2A_REMEDIATION_REPORT10151146.md` 和 `NOVA_WP2A_REMEDIATION_NOTES10151146.md`。仓库未发现适用 AGENTS.md / CLAUDE.md 规则文件。启动前核对分支、HEAD、tag、remote、status，并逐一核验原 delivery manifest 的 22 个文件 SHA-256，全部相符（`approved-delivery-verification.json`）。

修改前保存 `before/`、`baseline-manifest.json`；本轮结束对 33 个现场保护文件复核，只有预期的 Cargo.toml、lib.rs、runtime/process.rs 三个既有文件改变，其余 30 个哈希相同。原 22 文件中只有 lib.rs 因测试启动包装改变，其余保持原哈希。历史产品文档及已存在的未跟踪实现均保留。未切换、清理、回退、stash、整文件恢复、提交或 push。

## 隔离证明

- 测试 app-data：`D:\nova\audit\wp2a-desktop-validation-20261015-1533\run-07\app-data`；数据库：同目录 `nova.sqlite3`；WebView2 user-data：同目录 `webview`；临时项目：`run-07\project`。
- 独立 debug-only 入口 `nova_wp2a_validation.exe`，非默认 feature `wp2a-desktop-validation`，不经正常发布入口；普通入口在该 feature 下未先初始化隔离配置会拒绝启动。运行时改用不同 identifier `com.sdkmax.nova.wp2a.validation`，正常 `com.sdkmax.nova` 不变。
- `prepare` 在打开连接前拒绝相对路径、已存在 run 根、非审计父目录、祖先 symlink/reparse；解析为 canonical 根，创建全新数据库路径和 marker。数据库打开只使用此显式路径，无默认目录回退；WebView 也显式指定 data_directory。`ISOLATED-VALIDATION.json` 是连接前记录，`startup-proof.jsonl` 是实际运行路径和固定假 CLI 哈希证明。路径比较兼容 Windows `\\?\` 前缀，没有用普通环境变量猜测隔离。
- 唯一 AI 进程是从本轮源码构建并复制至 `run-07\tools\nova_wp2a_fake_cli.exe` 的假 CLI，SHA-256 见 startup proof / build-artifact-hashes。它无网络/子进程调用代码，仅解析实际 stdin 中的需求清单并返回显式 FAKE 输出；不代表真实 AI 能力。
- 在共享进程执行器启动前，验证 cwd 必须在隔离根内，程序必须为固定假 CLI 或指定 Git，且每次核验假 CLI 文件哈希；真实 codex/claude 名称、根外 cwd、假 CLI 文件替换拒绝测试已通过。最终运行 44 个请求均为白名单程序和隔离 cwd，假开发调用恰为 4 次（正常、缺失、无效、未完成）。
- 本地 Git 仓库、全局空配置、禁用 system Git 配置均在测试启动时明确设置。Git 只用于本地夹具和生产项目观察。没有 remote/push。
- NoCredentials 禁止读取/保存/删除系统凭据；真实外部 API/聊天发送等命令在验证分派器拒绝。保留 WP2A 原生产命令分派，不使用前端 invoke mock。前端仍受原 CSP 约束，页面仅从本地 Vite 提供，没有真实 AI、付费服务请求或真实数据库访问。

## 桌面业务场景实际结果

下列证据位于 `run-07/checkpoints/`；每个同名 `.txt` 为实际 WebView DOM 文本，`.json` 包含对应完整数据库快照，不是截图。调用日志统一见 `run-07/ipc.jsonl`，假 CLI stdin 见 `fake-cli.jsonl`。

| 场景 | 实际操作与关键断言 | 结果 / 停点 |
|---|---|---|
| 1 需求确认生成快照 | 真实表单创建任务；Draft 调用补建拒绝且快照 0；点击确认需求后 Confirmed，6 个 item（包含重复文本的不同序号） | 通过；01-confirmation-snapshot |
| 2 明确补建 | Confirmed 缺快照夹具；展开 4 个只读原字段，无编辑输入；点击生成；再次点击 ID/行数保持一致；另验证 Completed 拒绝 | 通过；02-explicit-backfill、04-first-user-acceptance |
| 3 假开发摘要/预检 | 页面就绪检查和开始任务；实际任务包经 stdin 到假 CLI；stdout 严格解析入库；页面自评已完成、首次验收可点；预检 can_submit_acceptance=true、ok=false（尚无用户确认） | 通过；03-fake-cli-summary-precheck |
| 4 首次验收 | 点击生产“通过验收”，进入 Completed；恰好一条 accepted 用户确认；终态补建拒绝 | 通过；04-first-user-acceptance |
| 5 缺失/无效/未完成 | 三次独立假开发；最终结果文档实际打开，pre 内容包含当前 task ID；验收按钮 disabled、原因可见；生产 transition_task 强制 Completed 拒绝且不增加状态事件 | 通过；05-02、05-03、05-04 |
| 6 Case C | synthetic 夹具中确认已存在、build Planned attempt2、NeedsUserAction；点击真实恢复按钮后 Queued；确认数、完整 action 行均不变；后续场景结束仍 Queued，四次假开发调用之外无额外 AI | 通过；06-case-c-recovery |
| 7 异步失效 | 分别延迟生产 precheck/self-check IPC 1400ms，切换到无效任务，迟到成功不能启用验收或显示“自评已完成”；真实 SQLite 表改名产生两个读取错误；切换清除旧错误；夹具合法状态转换升至 generation2，旧代预检不能启用验收 | 通过；07-late-get_completion_precheck、07-late-list_task_self_checks、07-read-failure、07-stale-switch |
| 8 历史 attempt | synthetic 当前 dev attempt2、证据 attempt1；真实 overview 所有测试 is_current_attempt=false，时间线显示“历史” | 通过；08-old-attempt-history |
| 9 冻结历史 | 冻结夹具验收 disabled；真实 accept/start IPC 拒绝且无 acceptance 增写；实际打开文档；生产 transition_task 合法取消成功 | 通过；09-frozen-view、09-frozen-cancel |

补充边界：第 9 项取消通过真实生产 IPC 负载执行；该 Awaiting 状态原 UI 没有通用取消按钮，不能宣称测试了不存在的取消按钮。第 7 项代际变化和 SQL 故障由 debug-only 控制命令在隔离 DB 注入，不是假扮正常用户操作。双连接竞争等已审核代码测试不在本轮重新设计；本次验证的是前端晚响应处理。其他所有终态补建、所有状态组合未逐一通过桌面重演；已有单测与本轮 Draft/Completed 探针分开计量。

## 夹具来源与失败记录

`isolated_validation::seed` 创建 9 个公开标注的任务；normal/missing/invalid/incomplete 的开发与自检来自实际假进程，test/build 使用明确预置的无需测试/构建授权，不伪装真实测试/构建通过。Case C、旧 attempt、冻结、generation 场景含 synthetic 数据，详见 `fixtures.json`、`database-before.json` 和 seed 源码。部分夹具直接 SQL 设置分类/attempt/缺快照，仅用于构造入口前状态，不替换生产操作的门禁结果。

早期运行均保留：run-01 因 Git 不接受扩展路径格式全局配置而失败；run-02 驱动未加载项目列表即找按钮；run-03 等待截图 ack 超时；run-04 数据库已完成但驱动仍读取开发中列表，修正为页面刷新后验证；run-05 在新代证据缺失后注入表故障，预检合法提前返回证据缺失，未到预期 SQL 错误，修正故障注入顺序。run-06 全部通过；run-07 收紧最终结果文档与迟到摘要断言后再次全部通过。没有放宽生产门禁或已有测试断言。证据核验脚本的 Windows canonical 前缀和 UTF-8 读取也已修正，最终 26 项通过。

## 单元测试、构建及 NOT RUN

| 命令 / 检查 | 实际结果 | 日志 |
|---|---|---|
| cargo test --lib（默认 feature） | 674 passed，0 failed；临时/内存测试，无真实 AI | default-backend-tests.log |
| cargo test --features wp2a-desktop-validation --lib isolated_validation::tests | 3 passed，0 failed；相对/已有/非审计父路径拒绝、固定哈希/cwd/真实 CLI 名拒绝、feature 静态保护 | facility-tests-final.log |
| npm --prefix apps/desktop test | 109 passed，5 files | frontend-tests.log |
| npm --prefix apps/desktop run build | 类型检查 + Vite 通过 | frontend-build.log |
| cargo build --features wp2a-desktop-validation --bin nova_wp2a_validation --bin nova_wp2a_fake_cli | 两个 debug 测试入口构建成功 | validation-build-final.log |
| cargo build --release --bin nova | 默认发布可执行构建成功 | normal-release-build.log |
| npm --prefix apps/desktop run tauri -- build --bundles nsis | 完整 NSIS 包构建成功；没有启动/安装 | normal-tauri-package.log |
| cargo build --release --features wp2a-desktop-validation --bin nova_wp2a_validation | 预期拒绝，exit 101，compile_error debug-only and must never ship | release-feature-must-fail.log |
| python audit/wp2a-desktop-validation-20261015-1533/verify_evidence.py | 26 项通过；SQLite integrity_check=ok，foreign_key_check 空；9 组/17 停点/44 进程/4 假开发 | evidence-assertions.json |
| 截图、原生鼠标/键盘输入、像素排版 | NOT RUN：窗口图形采集/激活/坐标服务失败；有真实 DOM 自动化和 UIA 文本 | screenshots/NOT_CAPTURED.md |
| 一次性 VM 安装/安装后启动 | NOT RUN：无一次性 VM 或隔离安装环境，未覆盖既有安装 | — |
| 正常数据目录/真实 AI/真实数据库 | 有意不运行、不读取、不迁移、不恢复 | — |

默认后端测试含项目自有的真实 cargo/进程辅助测试，运行于其临时夹具；这不等于 AI CLI 调用。前端 109 项是单元测试，不与 9 组桌面场景混计。验证 feature 构建有 14 个 warning，主要为正常系统凭据/恢复路径未使用及 linker 消息；正常 release 为 1 个 linker warning；未掩盖。

## 正常发布行为与交付

新增设施未列入默认 features，测试 bin 在 tests/fixtures 且 required-features，release 开启验证 feature 会直接编译失败。生产命令注册名单与检查点完全一致；正常配置/CSP/前端文件/业务实现不变；默认启动仍走原数据库与凭据初始化。默认 release 二进制扫描未发现 validation_control、隔离 marker 或假 CLI 标识；保存的 normal-installer.nsi 只引用 normal nova.exe 作为应用主程序，无验证 bin/resources。构建成功不证明安装/正常用户数据启动已验证。

源文件交付边界如下；完整精确哈希见 `source-delivery-manifest.json`，本轮补丁 `this-turn-source.patch` 以 `before/` 为基线，可与已审核代码清晰区分。禁止用 HEAD diff 混淆既有实现和本轮设施。

| 文件 | 本轮边界 | SHA-256 |
|---|---|---|
| `apps/desktop/src-tauri/Cargo.toml` | modified this turn | `0dd7ebe1a14f18870a171e67569bf30a347fb9d9197f2f9b25833a746eac94ad` |
| `apps/desktop/src-tauri/src/lib.rs` | modified this turn | `4843eb9638f70f96ab14d50c16d8c2a8699a205ba3b8a4c26da1075492608386` |
| `apps/desktop/src-tauri/src/runtime/process.rs` | modified this turn | `7c5a0856abfe9d63ade9376f3f92f1472f29730739316f0ae5bb2c96a24707cd` |
| `apps/desktop/src-tauri/src/isolated_validation.rs` | new this turn | `197c3decd42bdf0e5d27889e8fb8881627cea357356f9ed805310176c47a3628` |
| `apps/desktop/src-tauri/tests/fixtures/wp2a_validation_main.rs` | new this turn | `539126d5e1ecf86a76b65983a3c2f5b9394e4121d763fae77518d6c2299dd575` |
| `apps/desktop/src-tauri/tests/fixtures/wp2a_fake_cli.rs` | new this turn | `b7a60120c7fbea6707b16df630e80c97c43854a19e4353f14b1e0c2b318aac5f` |
| `apps/desktop/src-tauri/tests/fixtures/wp2a_validation_driver.js` | new this turn | `f196bc6e0cedd8ff1cb4517446e40d4bec600a983da46b5a81f572339a27fb70` |

生成的审计脚本、日志、run 子目录、数据库、WebView profile、marker、停点文本和截图缺口说明都在证据根；审计脚本不进入应用构建。新增本文及隔离说明 `NOVA_WP2A_ISOLATED_VALIDATION_NOTES10151653.md`。完整交接清单 `handoff-manifest.json` 记录文档/辅助脚本/证据 SHA-256；生成脚本本身也被纳入，manifest 不自引用。构建产物 SHA 见 build-artifact-hashes.json。所有交付均未提交。

## Claude 独立审核索引

1. 先看原 `CLAUDE_WP2A_REMEDIATION_REVIEW10151457.md` 与 `approved-delivery-verification.json`，确认本轮边界。
2. 对照 `before/`、this-turn-source.patch 和 source-delivery-manifest，审核 3 个启动/进程接缝与 4 个测试源码；确认默认构建排除 debug 控制命令且没有门禁变更。
3. 审核隔离 preflight、NoCredentials、进程固定路径/哈希、导航/CSP 和 fake CLI 无外部调用；核对 run-07 startup proof、marker、process-boundary、fake-cli stdin。
4. 对照本表逐项复查 run-07 driver-result、17 个 checkpoint 与 DB 前后断言；关注 synthetic 来源、Case C 没自动调用 AI、真实读取错误、晚响应摘要/预检。
5. 复核正常 NSIS 构建与 feature release 拒绝日志，必要时在全新 run 根重跑设施。
6. 在可捕获桌面的隔离会话补截图/原生输入，若有一次性 VM 再补安装验证。Codex 本轮自检不能代替 Claude 独立复审。

本轮测试进程与 Vite 已停止；保留全部临时测试数据用于审计，不恢复真实数据库。不提交、不 push、不发布、不建立里程碑；交 Claude 审核新增设施与运行证据。

补充交付核验：`git apply --reverse --check audit/wp2a-desktop-validation-20261015-1533/this-turn-source.patch` 返回 0，仅检查未执行撤销；补丁统一使用 LF，避免 Windows 换行导致不可核验。最终临时库另通过 SQLite online backup 保存为 `run-07/database-after-consistent.sqlite3`，只涉及审计临时数据库，不涉及真实用户数据。handoff-manifest 的明确范围排除可变 WebView 缓存及其自身，正常构建产物另由 build-artifact-hashes 固定；其余审计文件和本轮源码、交接文档均列 SHA-256。
