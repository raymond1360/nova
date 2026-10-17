# WP2A Windows CI 工作流独立审核（Claude Code）

生成时间：2026-10-17 17:02 Asia/Shanghai。项目：D:\nova。审核人：Claude Code；开发/整改：Codex；产品管理：ChatGPT。

## 唯一结论：PASS（限本轮CI实现审核；不代表云端构建或发布成功）

覆盖版本：分支 `feature/v0.1.0-mvp`，完整HEAD `3216da284d90240b5e10853e6b03cf4289e0f4b4`（未提交），待审工作流 `.github/workflows/windows-ci.yml` 原始字节SHA256 `4a6bb5a516c3866958a33c72505c1a81b68e3f5231af223b5593d62dd39699b4`。审核范围：该工作流文件本身、其直接引用的package.json/锁文件/Cargo配置/Tauri配置/测试源码，以及目标仓库`https://github.com/raymond1360/nova`的只读核实。**未发现阻断缺陷**；发现一项此前两份Codex报告均未提及的Major级运营风险（锁文件注册表来源，见第二节表格），已记录但判断为不足以单独构成FAIL（理由见该行"判断"列）。

## 一、现场与版本核验

- 分支/HEAD：`feature/v0.1.0-mvp`，完整HEAD `3216da284d90240b5e10853e6b03cf4289e0f4b4`，与预期一致。`git describe --tags` → `milestone/evidence-gate-1-g3216da2`，确认HEAD为该里程碑tag之后新增的1个提交。
- 该提交（`3216da2`，作者Codex，2026-10-17 16:03:55 +0800，信息"Archive reviewed WP2A implementation and isolated desktop validation"）是对我此前两轮已PASS的WP2A业务代码与隔离桌面验证设施的封存整理，**不在本轮审核范围内**（本轮任务明确为"只读核实…不要修改实现"，且该提交内容已在`CLAUDE_WP2A_REMEDIATION_REVIEW10151457.md`/`CLAUDE_WP2A_DESKTOP_VALIDATION_REVIEW10171515.md`中审过）。未在本轮重新展开该部分审核。
- 工作流文件独立重算SHA256：`4a6bb5a516c3866958a33c72505c1a81b68e3f5231af223b5593d62dd39699b4`，与预期值完全一致——**但工作流本身未提交**（`.github/`为Git未跟踪目录），`git diff`本身不会显示它；已用`git status`确认其为untracked而非被.gitignore隐藏或已暂存。
- `git status`核对：暂存区为空；tracked修改仅两份既有product文档（与此前所有轮次一致，未被本轮触及）；历史docs/design、历史报告、audit原始证据、我此前独立复验留下的`audit/wp2a-desktop-validation-claude-20261017-1513/`均原样保留。`git remote -v`无输出，确认本地未配置远程。
- 本轮审核过程中**未执行**reset/clean/stash/add/commit/配置remote/push，仅执行只读`git status`/`git log`/`git show`/`git diff --stat`、本地只读构建/测试命令（见第二节）、对公共GitHub API的只读HTTP请求。

## 二、工作流审核（10项核查，逐一独立验证，非转述报告）

| # | 核查项 | 判断 | 独立验证方式与结论 |
|---|---|---|---|
| 1 | 仅workflow_dispatch、contents:read、checkout不保留凭据 | **通过** | 直读YAML第3-7、22-25行：`on: {workflow_dispatch: null}`（PyYAML解析确认恰好一个触发器，无push/PR/tag）；顶层`permissions: {contents: read}`（job级未覆盖，故对全部step生效）；`actions/checkout`入参`persist-credentials: false`（官方action.yml该参数默认true，本工作流显式覆盖）。无release/deploy/签名/业务secrets引用 |
| 2 | action完整SHA与官方来源、输入匹配；Node/npm/Rust固定版本真实可用且兼容 | **通过** | 独立查询GitHub官方REST API：`actions/checkout@3d3c42e...`=v7.0.1、`actions/setup-node@949feb2...`=v7.1.0、`actions/upload-artifact@cf430e0...`=v7.0.2，三者`object.type=commit`且SHA与workflow完全一致（非猜测）。进一步拉取三个action在该**精确pinned SHA**下的`action.yml`源码，确认`persist-credentials`/`node-version`/`architecture`/`package-manager-cache`/`name`/`path`/`if-no-files-found`/`retention-days`/`include-hidden-files`等全部被使用的输入名在该版本确实存在。Node 24.14.0官方win-x64.zip SHA256独立比对与Codex报告完全一致；npm 11.9.0 tarball（`npm/-/npm-11.9.0.tgz`）独立请求返回200，其`engines.node`为`^20.17.0 \|\| >=22.9.0`，与Node 24.14.0兼容；Rust 1.98.0官方channel manifest确认`x86_64-pc-windows-msvc`目标`available=true` |
| 3 | PowerShell原生命令、管道及预期失败退出码处理 | **通过** | `$ErrorActionPreference='Stop'`+`$PSNativeCommandUseErrorActionPreference=$true`是PowerShell 7.3+引入、专门解决"管道中原生命令失败不终止脚本"问题的组合，windows-2022 runner自带pwsh版本已满足。第8步（release验证feature拒绝检查）正确地临时将`$PSNativeCommandUseErrorActionPreference`设回`$false`以允许该条命令的非零退出，检查完预期诊断后显式`$global:LASTEXITCODE = 0`——这是应对"GitHub的pwsh包装器以脚本结束时的`$LASTEXITCODE`而非是否抛出异常来判定该step是否失败"这一已知细节的正确处理，workflow注释中也准确说明了这点，并非想当然 |
| 4 | helper预构建路径与真实测试查找路径一致；嵌套cargo测试预热与内部实际编译参数匹配 | **通过** | 直读`runtime/process.rs:678-700`的`helper_exe_path()`：基于`current_exe()`回溯到`target/<profile>/`后查找`nova_test_helper.exe`；workflow第3步`cargo build --locked --features nova-test-helper --bin nova_test_helper`未覆盖目标目录，产物落在同一默认`target/debug/`下，路径精确匹配。直读`test_runner.rs:973-990`的`real_cargo_test_run_against_this_crate_produces_a_real_pass_count`：固定使用`std::env::temp_dir().join("nova-self-referential-cargo-test-cache")`作为`--target-dir`；workflow同一步骤的预热命令`cargo test --locked --lib app_paths::tests::accepts_ordinary_uuid_like_components --target-dir $nestedTarget`（`$nestedTarget`=`[System.IO.Path]::GetTempPath()`+同名）与源码完全一致，确认预热命中的是同一缓存目录，而非凑巧相似的另一路径 |
| 5 | 默认完整后端测试、前端测试、3项设施测试确实被执行 | **通过** | workflow第5步`cargo test --locked --lib`（默认feature，覆盖674项所在范围）；第4步`npm ci`+`npm test`，独立核对`apps/desktop/package.json`的`"test": "vitest run"`无任何过滤参数，确认跑的是全部5文件109项；第6步`cargo test --locked --features wp2a-desktop-validation --lib isolated_validation::tests`并显式`if ($log -notmatch 'test result: ok\. 3 passed; 0 failed') { throw ... }`——不是只看退出码，连输出摘要文本也校验，防止"部分跳过仍算通过"的误判 |
| 6 | 正常release/NSIS构建不启用测试feature；临时配置覆盖、--no-sign、--locked等参数正确 | **通过** | 直读`tauri.conf.json`：`bundle.active:false`、`bundle.targets:["nsis"]`，与两份Codex报告描述一致。workflow第7步临时JSON仅含`{"bundle":{"active":true}}`，未修改受审原始文件；查阅Tauri官方CLI文档确认`--config`是与默认配置**合并**而非整体替换，故`targets:["nsis"]`在合并后仍由原配置提供，不会因覆盖JSON未列出而丢失——即便合并细节本身有残余不确定性，workflow下游仍会在产物检查步骤对两个精确路径（含NSIS）做存在性+非空校验，合并行为若意外失效会在那一步直接失败而非悄悄产出错误产物，形成双重保险。独立执行`node .../tauri.js build --help`确认`--ci`/`--no-sign`/`-c/--config`/`-b/--bundles`均为该精确已安装2.11.4版本的真实参数（非凑测）；该步骤本身未传任何`--features`，不会意外启用`wp2a-desktop-validation` |
| 7 | release验证feature的负向检查只能接受指定compile_error，其他错误不能误判通过 | **通过（本机独立复现了真实失败输出并逐字段核对）** | 本机独立执行`cargo build --locked --release --features wp2a-desktop-validation --bin nova --message-format=json`：退出码**101**；对捕获的真实JSON逐行解析，**恰好1条**`reason=="compiler-message"`且`message.level=="error"`的记录，其`target.name=="nova_lib"`、`message.message`逐字符等于`"wp2a-desktop-validation is debug-only and must never ship"`、`message.code`为`null`——与workflow第9步PowerShell的全部判定条件（`$guardExit -ne 101`、`$errors.Count -ne 1`、`target.name`、`-cne`大小写敏感比较、`code`非空）逐项对应，**不是读了代码就假设成立，而是真实跑出了同样结构的失败输出并用相同逻辑验证**。另确认输出中混有2行非JSON文本（`Compiling nova...`与`error: could not compile...`），workflow的`$_.StartsWith('{')`过滤对此真实场景同样成立 |
| 8 | 锁文件与正式配置不变；两个产物路径、命名、未签名检查合理 | **通过** | workflow末两步分别用`git diff --exit-code`核对两锁与`tauri.conf.json`未变，并逐文件重算哈希与步骤开始时记录的`locks-before.json`比对——双重防线。产物路径`target/release/nova.exe`与`target/release/bundle/nsis/Nova_0.1.0_x64-setup.exe`与此前两轮本机实际构建产物路径完全一致；`Get-AuthenticodeSignature ... -ne 'NotSigned'`的未签名校验方向正确（要求未签名，与"不签名发布"的声明边界一致） |
| 9 | 上传白名单、哈希及构建SHA对应正确，不上传数据库/audit/helper/假CLI/验证程序 | **通过** | 上传清单恰为10个精确文件（2产物+SHA256.json+build-info.json+VALIDATION-ONLY.txt+5份日志），`path:`逐行精确指向`${{ runner.temp }}/wp2a-validation-artifacts/`下的具名文件，无任何通配符（如`target/**`、`audit/**`）。`if-no-files-found: error`确保缺任一文件即失败而非静默跳过。`SHA256.json`/`build-info.json`均在前序步骤由真实`Get-FileHash`/真实`$env:GITHUB_SHA`等生成，非预先编造 |
| 10 | 工作流不依赖本机绝对路径、未跟踪文件或本机缓存 | **通过** | 全文grep确认无`D:\nova`、`D:/nova`、`verify-build`、`audit/`、`C:\Users`等字样；全部路径基于`$env:GITHUB_WORKSPACE`/`$env:RUNNER_TEMP`/`[System.IO.Path]::GetTempPath()`。独立确认`scripts/verify-build.ps1`（确有硬编码`D:\nova`，与CI_READINESS_REPORT描述一致）未被本workflow引用。独立grep `Cargo.lock`确认全部481个package条目`source`均为`registry+https://github.com/rust-lang/crates.io-index`，无`git`/`path`来源，排除"锁定了本机专属依赖导致新clone装不上"的风险 |

### 独立发现的新风险（两份Codex报告均未提及）

| 严重程度 | 位置 | 触发条件 | 影响 | 最小修复建议 |
|---|---|---|---|---|
| **Major**（运营风险，非workflow代码缺陷；不单独判FAIL，理由见下） | `apps/desktop/package-lock.json`（既有文件，本轮未改） | `npm ci`严格按锁文件`resolved`字段指定的URL拉取精确tarball；本仓库该锁文件**全部198个`resolved`条目无一例外指向第三方镜像`registry.npmmirror.com`，0个指向官方`registry.npmjs.org`**（本轮独立grep核实：`registry.npmmirror.com`命中198次，`registry.npmjs.org`命中0次）。Windows runner位于GitHub/Azure海外数据中心，对该镜像的可达性/速率限制未经证实 | 若该镜像从GitHub Actions网络不可达/不稳定，首次`npm ci`步骤（workflow第4步）可能失败或极慢，直接阻塞后续全部步骤；本机验证无法代表该情况，因为本机curl经由本地代理（127.0.0.1:7897）访问，并不代表GitHub runner的真实网络路径 | 不改变本轮"不修改实现"的边界的最小做法：**不在本轮修改锁文件**，但PM/Codex应在首次真实Actions运行前，于`npm ci`前显式加一行`npm config set registry https://registry.npmjs.org/`（会被`--no-audit --no-fund`之外的参数采用，不影响`npm ci`严格校验tarball完整性的机制，因为`integrity`字段独立于`resolved`主机名）或改用`npm ci --registry https://registry.npmjs.org/`，以绕开对该未经证实镜像的依赖；或至少在首次运行失败时第一时间检查该步骤日志是否为网络/超时错误。这属于"首次手动构建"阶段需要关注的具体项，不要求本轮改代码 |

**判断依据：** 该风险的根源是既有、本轮未改动的`package-lock.json`内容，不是本轮新增workflow文件的逻辑缺陷——workflow本身只是正确地调用了标准`npm ci`；是否真的失败取决于GitHub基础设施对该第三方镜像的实际可达性，这一点本轮无法验证（无GitHub runner访问权限），不能凭空判定"必然失败"从而FAIL一个本身逻辑正确的workflow步骤。因此计入Major级已知风险并给出具体缓解建议，而不升级为阻断本轮CI实现审核结论的FAIL项；但**首次手动触发前应提醒PM/Codex关注这一具体步骤的实际结果**。

### 本机独立执行的验证命令（均只读或在独立临时目录执行，未改动受审文件）

| 命令 | 结果 |
|---|---|
| `actionlint.exe -shellcheck= -pyflakes= .github/workflows/windows-ci.yml`（下载后独立核对官方v1.7.12 windows_amd64.zip校验和与Codex报告一致，再本机运行，非复用其运行结果） | 退出码0，无诊断 |
| PyYAML `yaml.safe_load()`解析workflow | 成功，11个step，顺序与两份报告描述一致 |
| 3个action的官方`git/ref/tags/*`只读API查询 | SHA与workflow完全一致 |
| 3个action在精确pinned SHA下的`action.yml`抓取 | 全部用到的输入字段确认存在 |
| Node 24.14.0官方SHASUMS256.txt核对 | win-x64.zip哈希与Codex报告完全一致 |
| npm 11.9.0 registry tarball请求 | HTTP 200，engines兼容Node 24 |
| Rust 1.98.0官方channel manifest | msvc目标available=true |
| `cargo build --locked --release --features wp2a-desktop-validation --bin nova --message-format=json`（本机，cwd apps/desktop/src-tauri） | 退出码101；JSON逐字段核对与workflow期望结构完全匹配（见第七项） |
| `node node_modules/@tauri-apps/cli/tauri.js build --help` | 确认`--ci`/`--no-sign`/`--config`/`--bundles`均为已安装2.11.4真实参数 |
| grep核对`Cargo.lock`全部481个registry来源、`package-lock.json`的198个resolved主机名 | 结果见上表 |
| grep核对workflow全文无本机绝对路径/verify-build.ps1引用 | 未命中，确认 |

## 三、目标仓库只读核实（`https://github.com/raymond1360/nova`）

全部通过GitHub公开REST API未认证只读请求完成，未安装gh、未登录、未修改任何账户/仓库配置。

| 核查项 | 结果 |
|---|---|
| 可访问性 | 可访问（`GET /repos/raymond1360/nova` → 200） |
| 可见性 | `public`（`private:false`） |
| 默认分支 | 字段值为`"main"`——**但该仓库完全为空**（`size:0`，`created_at`与`pushed_at`相差仅1秒），`main`这个名字此刻只是仓库创建时配置的"计划默认分支名"，尚无任何实际commit/ref与之对应 |
| 现有分支/提交 | `GET /branches` → 空数组`[]`；`GET /git/refs` → `409 Git Repository is empty.`；`GET /contents/` → `404 This repository is empty.`；`GET /commits` → 409。**确认仓库内没有任何分支、引用或提交** |
| 与本地历史的关系/覆盖风险 | 因远程完全为空，本地`feature/v0.1.0-mvp`首次推送到任意分支名在Git层面**必然是纯粹的"从无到有"**，不存在非快进冲突或覆盖既有内容的风险——这一点不依赖猜测，是"目标无任何ref"这一已确认事实的直接推论 |
| 已有workflows/Actions运行 | `GET /actions/workflows` → `total_count:0`；`GET /actions/runs` → `total_count:0`（与仓库为空一致，非另有信息） |
| Actions启用状态/组织策略/Webhook/App集成/安全扫描 | **未知，明确列为未知**——`GET /actions/permissions`返回`401 Requires authentication`，本轮无授权凭据，不猜测结果。Webhook、组织级Actions策略、分支保护规则等同样需要写权限或仓库管理权限才可读取，本轮未核实，不代表不存在 |
| 账号/组织权限 | 未知——本轮未登录任何GitHub账号，无法判断当前使用者对该仓库的读写权限级别 |

### workflow_dispatch首次启用的最小可执行方案（仅说明，本轮不实施）

1. **GitHub官方规则确认**（已通过官方文档核实，非凭记忆断言）：`workflow_dispatch`要能在Actions UI/CLI/API中手动触发，其工作流文件**必须存在于仓库的默认分支**上；一旦满足这一条件，触发时可通过UI的Branch下拉菜单、`gh workflow run --ref`或REST API的`ref`参数选择在**哪个分支/ref**上实际运行。
2. **是否据此断言"选中分支会用该分支自己的workflow文件内容"**：官方文档未明确说明这一点，社区实测存在相互矛盾的报告（不同CLI/场景下表现不一致）——本轮**不对此做确定性断言**，仅如实列为需在首次真实运行后以该次run的实际checkout SHA/日志经验证实的待确认项。
3. **因当前仓库完全为空**，不存在"工作流已经在main、只是功能分支还没同步"这种常见场景；**最直接、不依赖上述未确定行为的最小方案**是：让承载本次待验证提交（`3216da284d90240b5e10853e6b03cf4289e0f4b4`，含该workflow文件）的分支本身就成为默认分支——即首次推送时直接推送为`main`（例如`git push origin feature/v0.1.0-mvp:main`，或先确保本地有`main`指向同一提交再推送），而不是先推一个空的/不同内容的`main`再推`feature/v0.1.0-mvp`。这样`workflow_dispatch`注册与"待验证功能分支"天然是同一个分支，不引入上一条未确定行为的风险。
4. 若PM坚持保留`main`与`feature/v0.1.0-mvp`分离（例如为后续长期分支策略铺路），**次优但仍可行**的方案是：先推送一个仅包含workflow文件（或完整内容亦可）的`main`，确认`workflow_dispatch`在Actions页面可见后，再推送`feature/v0.1.0-mvp`，手动触发时在Branch下拉菜单选择`feature/v0.1.0-mvp`，并在该次run的"Checkout selected commit"步骤日志中核对`git rev-parse HEAD`确实输出`3216da284d90240b5e10853e6b03cf4289e0f4b4`（workflow自身已内置此项SHA自检，见第34-39行），作为"确实跑的是待审内容"的直接证据，而不是假设分支选择机制必然如预期工作。
5. 无论采用3或4，**本轮均不实施**——仅在此说明两条路径，交PM决定并授权后续的remote配置、push与首次手动触发。

## 四、CI实现审核结论

**PASS。** 本轮10项核查范围内的工作流逻辑、官方action来源、工具链版本、PowerShell执行语义、helper/嵌套测试路径对应、测试覆盖范围、正常构建与保护性负向检查、产物与锁文件完整性、上传白名单，经独立源码直读、独立本机命令复现（而非转述Codex报告结论）核验，**均成立，未发现阻断缺陷**。额外发现一项既有锁文件的镜像来源运营风险（见第二节），已记录并给出不改代码的最小缓解建议，不单独构成FAIL。

**远程准备情况单独说明（不计入上述PASS/FAIL判断）：** 目标仓库`raymond1360/nova`可访问、公开、完全为空；Actions启用状态、组织策略、Webhook/App集成、当前账号权限均为**未知**，不是因为业务代码或CI实现有缺陷，而是本轮未获授权登录核实；这一不确定性**不应被解读为CI实现审核的缺陷**，但在PM授权推送/首次触发前必须单独确认，否则首次手动运行结果不可预测。

## 五、后续顺序（供PM安排，本轮不实施）

1. PM确认/决定：目标仓库默认分支与待验证分支的关系（见三、3-4两种方案二选一）、是否需要同时产出NSIS（当前workflow已包含，若不需要可留作备用能力）、首轮是否仅手动触发（当前已是）。
2. 获授权后配置本地remote（`git remote add origin https://github.com/raymond1360/nova.git`或等效），按选定方案推送（含该workflow文件的commit必须进入默认分支，见三、3-4）。
3. 首次在Actions页面手动触发`workflow_dispatch`，观察"Checkout selected commit"步骤的SHA自检输出，确认跑的是预期提交。
4. 密切关注"Install locked frontend and run frontend tests"步骤（`npm ci`）日志——这是本轮独立发现的唯一具体运营风险点（镜像可达性，见第二节）；若该步骤网络相关失败，优先按该节建议排查，不要误判为workflow逻辑问题。
5. 运行完成后，根据实际Actions run URL与运行产生的SHA，由Claude或指定人员对照本报告的10项核查结果与实际运行日志，报告云端结果；该结果独立于本轮PASS结论，本轮PASS不代表云端构建或安装/启动验收已完成。

## 六、未验证项清单（NOT RUN）

- GitHub真实Actions运行（任何形式）——本轮未授权执行，workflow未被触发。
- 安装后启动、原生视觉/输入验收——与此前两轮审核结论一致，仍为独立未完成项，与本轮CI工作流本身无关。
- 目标仓库Actions启用状态、组织Actions策略、Webhook/App集成、分支保护规则、当前账号实际读写权限——均因缺少认证访问，明确列为未知，不代表不存在问题，也不代表存在问题。
- `registry.npmmirror.com`从GitHub Actions hosted runner网络的真实可达性——本机验证经由本地代理，不能代表runner侧真实网络路径，已在第二节列为具体风险而非NOT RUN项（因为我确实独立验证了该风险"存在于锁文件内容"这一事实本身，只是其"运行时后果"未经证实）。

## 七、回滚方式（供PM/Codex参考，本轮未执行任何需回滚的操作）

本轮仅新增本报告，未修改源码、workflow、既有文档或Git配置，无需回滚。若后续决定撤销Codex本轮新增的workflow文件本身，按`CODEX_WP2A_CI_IMPLEMENTATION_REPORT10171638.md`第142-144行已给出的边界执行（仅移除`.github/workflows/windows-ci.yml`与该报告本身，不删除整个`.github`/`docs`目录，不触碰product修改或audit证据，不执行reset/clean/stash/整文件恢复）；本报告同理，若需撤销仅删除本文件本身。

完成审核，到此停止。未提交、未配置remote、未push、未触发Actions、未创建PR、未合并、未发布。
