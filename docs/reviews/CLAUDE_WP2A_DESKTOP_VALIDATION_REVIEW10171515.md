# WP2A 隔离桌面验证设施与运行证据独立复审（Claude Code）

生成时间：2026-10-17 15:15 Asia/Shanghai。项目：D:\nova。审核人：Claude Code（本轮独立复审方；Codex为本轮开发/设施构建方）。

## 唯一结论：PASS（限本轮范围：新增设施代码 + DOM/IPC 功能验证；原生输入/视觉/安装验证 NOT RUN，不计入本结论）

覆盖版本：分支 `feature/v0.1.0-mvp`，HEAD `6a90df951c6c72ab7ae95228d5f2cc4de7e17273`（未提交）；本轮7个源码文件改动（`Cargo.toml`/`lib.rs`/`runtime/process.rs` 三处既有接缝 + `isolated_validation.rs`/两个测试bin/一个driver.js 四个新文件），证据根 `audit/wp2a-desktop-validation-20261015-1533`（run-07 为最终运行）。本结论**不覆盖**已在 `CLAUDE_WP2A_REMEDIATION_REVIEW10151457.md` 中通过的WP2A业务代码本身（本轮未发现对该范围的改动），也**不覆盖**原生鼠标/键盘交互、像素截图、安装后启动（均为 NOT RUN，详见第五节），**不构成发布批准**。

## 一、现场与版本核验

- 分支/HEAD/tag 核验：`feature/v0.1.0-mvp`，HEAD `6a90df951c6c72ab7ae95228d5f2cc4de7e17273`，与预期基线一致；未切换/清理/回退/stash/整文件恢复；全部既有修改与未跟踪文件原样保留（本轮 `git status` 与审核前完全一致，仅本报告为新增）。
- `before/`、`baseline-manifest.json`：独立核对声明"33个现场保护文件中仅Cargo.toml/lib.rs/runtime/process.rs三个既有文件改变"——本机独立对这33个文件重算SHA256（含21个此前已批准的WP2A交付文件、7个本轮改动/新增文件、`error.rs`/`models.rs`等5个WP2A整改轮之外更早的既有修改文件），**结果与报告完全一致**：21个此前已批准交付文件字节不变（未发生"重新开展已关闭WP2A问题的无关整改"）；仅Cargo.toml/lib.rs/runtime/process.rs三处既有接缝改变；4个新文件存在且哈希与`source-delivery-manifest.json`一致。
- `source-delivery-manifest.json`（7文件）、`handoff-manifest.json`、`build-artifact-hashes.json`：逐一独立重算SHA256，**全部一致**（含对`target/release/nova.exe`、NSIS安装包、两个debug测试bin的独立重算）。`handoff-manifest.json`排除1463个可变WebView缓存文件及自身，不自引用，与声明一致。
- `git apply --reverse --check audit/wp2a-desktop-validation-20261015-1533/this-turn-source.patch`：本机独立重跑，退出码0（仅检查，未执行）。

**结论：本轮审核的是实际交付版本，且确认本轮未重新整改任何已关闭的WP2A业务问题。**

## 二、新增设施代码审核（源码直读 + 独立重跑）

| 检查项 | 判定 | 依据 |
|---|---|---|
| Cargo feature非默认，验证bin仅显式启用 | **通过** | `Cargo.toml`：`default = []`，`wp2a-desktop-validation`为独立feature；两个`[[bin]]`均`required-features = ["wp2a-desktop-validation"]`且源码路径在`tests/fixtures/`外于`src/bin/`，不被自动发现 |
| release启用验证feature确实拒绝 | **通过，且比原报告验证范围更广** | `lib.rs:1-2`：`#[cfg(all(feature = "wp2a-desktop-validation", not(debug_assertions)))] compile_error!(...)`。原报告仅用`--bin nova_wp2a_validation`验证。**本次独立额外验证**：`cargo build --release --features wp2a-desktop-validation --bin nova`（即对正式`nova`二进制本身加上该feature）**同样编译失败**，报同一`compile_error`，证明该拒绝是crate级、不依赖具体bin目标，排除了"只保护了测试bin、未保护正式bin"的疑虑 |
| 正常构建不存在validation_control、注入脚本、假CLI或测试门禁旁路 | **通过** | 独立对`target/release/nova.exe`（哈希与`build-artifact-hashes.json`一致）执行字符串扫描：`validation_control`/`ISOLATED-VALIDATION`/`wp2a_fake_cli`/`wp2a-desktop-validation`/`NOVA_WP2A_FAKE_CLI`/`nova_wp2a_validation`/`ISOLATION_PREFLIGHT`均为0匹配；`verify_evidence.py`另按UTF-16LE编码额外扫描4个关键marker，本次独立复核逻辑一致。NSIS安装脚本`normal-installer.nsi`独立grep确认`MAINBINARYSRCPATH`只指向`target\release\nova.exe`，全文无wp2a/fake_cli引用 |
| lib.rs正常启动、凭据和生产handler未被破坏 | **通过** | 直读`lib.rs`：`#[cfg(not(feature = "wp2a-desktop-validation"))]`分支与此前审核版本逐行一致（app_data_dir→migrate→ensure_default_provider→run_startup_recovery→`SystemCredentialStore`），默认不编译`isolated_validation`模块（该mod声明本身带`#[cfg(feature=...)]`）；`invoke_handler`命令注册名单与此前已审核delivery的`commands::*`列表逐条比对一致（`verify_evidence.py`也用正则`commands::\w+`独立核对，本次人工比对确认无遗漏/新增）。独立重跑`cargo test --lib`（默认feature）：**674 passed，0 failed**，与上一轮一致，无回归 |
| runtime/process.rs默认执行行为未回归 | **通过** | `execute_blocking`中新增的`guard_process`调用整体包在`#[cfg(feature = "wp2a-desktop-validation")]`下，默认构建该分支不存在，其余逻辑（`path_guard::validate_directory_boundary`、Windows Job Object挂起-分配-恢复等）未改动一字；`runtime::process::tests`模块测试包含在上面的674项全量通过内 |
| 隔离设施单元测试 | **通过** | 独立重跑`cargo test --features wp2a-desktop-validation --lib isolated_validation::tests`：**3 passed, 0 failed**（相对路径/已存在根/非审计父目录拒绝；固定哈希+cwd+真实CLI名拒绝；feature静态保护） |

**结论：本节全部通过，且本次独立验证把"release拒绝"的测试范围从报告原有的测试bin扩展到了正式`nova`bin本身，缩小了潜在的验证盲区，结果仍为拒绝，无新发现问题。**

## 三、隔离保证审核（源码直读 + 本机全新独立重现）

直读`isolated_validation.rs`核心逻辑（`prepare`/`setup`/`process_policy`/`NoCredentials`）并逐项核验：

- **无真实目录回退**：`prepare()`要求`root`绝对路径且**必须不存在**（`root.exists()`则拒绝），其父目录名必须以`wp2a-desktop-validation-`开头，父目录及fake/git路径的全部祖先拒绝symlink/reparse；`setup()`额外核验`c.data`不等于该身份（`com.sdkmax.nova.wp2a.validation`，与正式`com.sdkmax.nova`不同）下解析出的默认app-data路径，且目标数据库文件必须不预先存在。DB仅用这条显式路径打开，WebView仅用`.data_directory(c.data.join("webview"))`显式指定，无默认目录分支。
- **假CLI/Git约束**：假CLI文件名必须恰为`nova_wp2a_fake_cli.exe`，且文件内容必须包含标识字符串`NOVA_WP2A_FAKE_CLI_NO_NETWORK_NO_SPAWN_V1`；复制进隔离root后用`runtime::binary_trust::compute_fingerprint`（真实SHA256+symlink/reparse拒绝，复用既有生产级实现）固定哈希。`process_policy`每次调用重新计算该哈希与启动时的`fake_sha`比较，cwd必须在`c.root`内，且目标程序必须正好等于该固定假CLI路径或固定git路径——三者同时成立才`allowed`；独立重跑的单元测试覆盖"codex"/"claude"裸名拒绝、cwd越界拒绝、文件内容被篡改后拒绝，全部如预期失败。
- **NoCredentials**：`save_api_key`/`read_api_key`/`delete_api_key`三个方法均无条件返回`Err(NovaError::Credential(...))`，结构上禁止读写系统凭据；`dispatch()`对非白名单IPC命令（含外部API/配置修改类）统一`reject`。

**本机独立全新重现（未使用Codex的run-07，全新目录，全新进程）**：

1. 新建审计目录 `D:\nova\audit\wp2a-desktop-validation-claude-20261017-1513\`（满足`prepare()`的父目录前缀硬约束），以 `run-claude-01` 为全新`--root`；独立启动本机Vite（`127.0.0.1:1420`，HTTP 200确认）。
2. 直接运行已编译好的 `target/debug/nova_wp2a_validation.exe --root ... --fake-cli ... --git "C:\Program Files\Git\cmd\git.exe"`（未修改任何参数含义，未新增产品代码）。
3. 独立确认：`ISOLATION_PREFLIGHT`在打开数据库前输出；`startup-proof.jsonl`显示`real_credentials:false`、`all_paths_checked_before_database_open:true`、`fake_sha256`与独立计算一致、`distinct_identifier_default`为`com.sdkmax.nova.wp2a.validation`专属路径；`ipc.jsonl`在几秒内记录到**59条真实IPC调用**（`initialize_app`/`get_provider`/`list_conversations`/`get_setting`/`get_last_active_project`等），证明真实Vue前端确实在WebView2中加载并发出真实Tauri IPC，而非伪造或复用旧进程的日志。
4. 验证完成后用`taskkill /F /T`完整终止进程树（含6个WebView2子进程）并停止本机Vite，`ps`确认无残留；`git status`确认本轮操作后tracked修改清单与审核前一致。

**结论：隔离机制在真正打开数据库/凭据/进程前已生效，不依赖环境变量猜测，且本次脱离Codex原有进程、完全独立重现成功，未发现任何回退到真实目录/真实CLI/真实凭据的路径。**

## 四、运行证据审核（run-07，独立核验，不采信driver自报）

先确认`verify_evidence.py`本身是否具备独立核验能力（而非回显driver-result.jsonl）：直读其源码确认它以`mode=ro`只读打开数据库，直接查询`tasks`/`user_acceptance`/`confirmed_requirement_items`等表的真实行、核对`process-boundary.jsonl`/`fake-cli.jsonl`的真实内容、对二进制做字节级扫描、对源文件做真实diff——**具备真实独立核验能力，不是空壳**。

但遵照指令要求，**不直接信任该脚本的既有输出**，本次另行编写并运行了一份完全独立的只读Python核验脚本（只打开`run-07/database-after-consistent.sqlite3`的只读副本及`run-07`下的`.jsonl`原始日志，不依赖`verify_evidence.py`的任何中间产物，不写回`run-07`或证据根任何文件），结果：**29项独立断言全部通过**，关键项：

| 场景 | 独立核验内容 | 结果 |
|---|---|---|
| 01 首次验收 | `tasks.state='completed'`；`user_acceptance`恰好1行`accepted=1`；对应`self_checks`行`parse_status='valid'`且`self_reported_status='self_reported_complete'`，`raw_block_redacted`内容为假CLI真实输出（含`FAKE CLI, no real AI`字样及6项真实coverage） | 通过 |
| 02/03/04 缺失/无效/未完成自检 | 三个任务均停在`awaiting_user_acceptance`，`user_acceptance`表**零行** | 通过 |
| 05 快照补建 | `confirmed_requirement_items`表恰好6行，字段/序号与真实`goal`（3行，含2行重复"Export fixture"）+`in_scope`+`not_in_scope`+`done_criteria`完全对应 | 通过 |
| 06 Case C | 最终`state='queued'`；`no_build_confirmations`恰好1行（未被后续场景重复写入）；`ai_development` action的`attempt_number`始终为1（未被意外多调用一次AI） | 通过 |
| 07 历史attempt | 独立定位到该任务`task_actions`中`ai_development`的`attempt_number=2`（对应夹具制造的"旧attempt历史"场景） | 通过 |
| 08 冻结历史 | `legacy_classification='unknown_insufficient'`，`state='cancelled'`，`user_acceptance`零行 | 通过 |
| 09 代际/读故障 | `result_generation>=2`，`user_acceptance`零行（新代未被旧确认冒用） | 通过 |
| 进程边界 | `process-boundary.jsonl`44条记录全部`allowed=true`且程序恰为固定假CLI或固定git路径 | 通过 |
| 假CLI输入 | `fake-cli.jsonl`中恰4条带`stdin`的开发调用，且每条stdin都真实包含`===NOVA_REQUIREMENT_ITEMS_START===`与`===NOVA_SELF_CHECK_START===`标记（证明确实读了真实任务包，非空壳） | 通过 |
| IPC真实性 | `ipc.jsonl`462条记录，命令集合含`create_task`/`transition_task`/`accept_task_result`/`generate_requirement_snapshot`/`get_completion_precheck`等25种真实生产命令，非仅`validation_control` | 通过 |
| 停点数 | `checkpoints.jsonl`恰17条，`07-read-failure`停点DOM文本独立核验确实同时包含"完工预检读取失败"与"自检记录读取失败"两处真实错误文案 | 通过 |
| DOM真实性 | `01-confirmation-snapshot`停点DOM文本长度897字符（非空白/非占位符） | 通过 |

**九组场景确为真实业务链路而非测试分派器伪造**：`validation_control`的`seed`/`checkpoint`/`ack`/`delay`/`break_reads`/`advance_generation`仅用于造数、快照、延迟和数据库级故障注入（均为真实SQL操作，`break_reads`是真实`ALTER TABLE RENAME`断表），**从未直接返回伪造的业务预检/自检/完工结果**——业务结果（completed/awaiting_user_acceptance/queued/cancelled等状态、acceptance行、self_check行）均由真实DOM点击触发真实Tauri IPC调用真实生产函数（`transition_task`/`accept_task_result`/`generate_requirement_snapshot`等）写入，本次独立查库已直接确认这些行确实存在且内容正确。09项冻结取消场景使用的是已有生产IPC（`transition_task`到`Cancelled`），因为该状态原UI本就没有通用取消按钮——核验说明未夸大为"测试了不存在的按钮"，直读`CoreLoopView.vue`现状确认属实。

## 五、早期失败记录真实性核验（抽样）

抽样核对early run的原始日志（不采信报告文字复述）：

- **run-03**：`driver-result.jsonl`内容为`{"completed":false,"error":"Error: screenshot ack timeout 00-isolation","results":[]}`——与报告"等待截图ack超时"描述一致，真实失败，非伪造。
- **run-04**：无`driver-result.jsonl`（未走到finish）；`checkpoints.jsonl`最后一条停点名为`03-fake-cli-summary-precheck-FAILED`，与报告"驱动仍读取开发中列表"描述一致。
- **run-05**：`driver-result.jsonl`显示`completed:true`但场景7显式`"passed":false,"error":"Error: timeout: actual SQLite read error displayed"`，其余8组均`passed:true`——与报告"故障注入顺序导致预检提前合法返回证据缺失，未到预期SQL错误"描述完全一致，且该脚本在场景7失败后仍继续运行了08/09两组并记录真实结果，未静默跳过或篡改失败为通过。

**结论：早期失败到最终成功的变化有具体、可验证、彼此一致的原始日志支撑，未发现放宽生产断言或篡改失败证据的迹象。**

## 六、必要重跑结果汇总（本机独立执行，均在新建临时/隔离目录，未触碰run-07）

| 命令 | 结果 |
|---|---|
| `cargo test --lib`（默认feature） | 674 passed，0 failed（与上一轮一致，无回归） |
| `cargo test --features wp2a-desktop-validation --lib isolated_validation::tests` | 3 passed，0 failed |
| `cargo build --release --features wp2a-desktop-validation --bin nova`（独立新增，覆盖报告未测的正式bin） | 编译失败：`compile_error!("wp2a-desktop-validation is debug-only and must never ship")` |
| `target/release/nova.exe`／NSIS安装包／两个debug测试bin 哈希重算 | 与`build-artifact-hashes.json`全部一致 |
| 正式release二进制／安装包字符串扫描 | 0命中任何validation marker |
| `git apply --reverse --check this-turn-source.patch` | exit 0 |
| 33个现场保护文件哈希核对 | 与报告声明的"仅3个既有文件改变"完全一致 |
| 独立Python脚本核验run-07数据库只读副本+日志（29项） | 29 passed，0 failed |
| **全新目录全新进程端到端重现隔离启动**（`run-claude-01`，未复用run-07任何产物） | 成功：隔离path/哈希/身份全部如预期，真实IPC 59条，清理完整、无残留进程 |

## 七、三轴判断

1. **新增设施代码是否通过**：**通过**。Cargo feature隔离、release拒绝（含本次扩展验证的正式bin）、进程/路径/凭据隔离、单元测试均经源码直读与独立重跑确认，未发现门禁旁路或测试设施混入正常发布产物的证据。
2. **DOM/IPC功能验证是否成立**：**成立**。run-07的九组场景经本次独立数据库/日志查验（而非采信driver自报）全部得到真实业务数据佐证；早期失败记录真实可信；本次另外完成了一次完全独立于Codex产物的全新端到端隔离启动重现，结果一致。
3. **原生输入、视觉、安装验证还缺什么**：**仍缺**——截图0张（`FrameArrived timed out`等Windows窗口捕获API限制，本次审核环境同样不具备原生窗口截图/鼠标键盘注入能力，无法独立补齐）；原生鼠标/键盘交互未验证；安装后启动、一次性VM安装验证 NOT RUN（本次审核环境无一次性VM，且指令要求仅限已有隔离环境执行，不新建）。**这些缺口不等同于业务代码FAIL**——缺口性质是"桌面图形/安装环境能力限制"，而非"发现了业务逻辑缺陷"，第二节、第四节的独立核验均未发现任何正确性问题。同时，**这些缺口的存在意味着完整桌面视觉与安装验收仍未完成，不能据此宣布完整发布验收通过**。

## 八、最小补验条件（供后续、非本轮范围）

1. 在具备可捕获交互桌面（真实图形会话，非当前headless/服务式工具环境）的审核会话中，使用系统级截图API（而非依赖进程间UIA/帧捕获协议）对隔离运行的Nova窗口执行正常验收、无效自检、快照补建、Case C恢复四个关键场景的原生鼠标/键盘点击与截图取证。
2. 在已有的一次性VM或明确隔离的安装环境中，安装本轮或后续构建的NSIS包，验证安装、首次启动、基本交互，不覆盖现有真实Nova安装。
3. 不需要新增产品代码或新建大型测试系统来满足上述两点——现有隔离设施已足够支撑，只缺执行环境。

## 结论

**PASS**（限本轮范围：新增隔离验证设施的代码正确性 + 以DOM/真实IPC为载体的业务功能运行证据）。原生视觉/输入/安装验证仍为独立、尚未完成的后续环节，已在第八节给出最小补验条件；本结论不构成发布批准，不建立里程碑，不代表WP2A业务代码的本轮重新审核（该部分仍以`CLAUDE_WP2A_REMEDIATION_REVIEW10151457.md`为准，本轮未发现其所涉代码被改动）。

交PM决定封存节点。
