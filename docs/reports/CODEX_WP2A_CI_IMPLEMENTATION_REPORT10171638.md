# WP2A 最小 Windows CI 本地实现交付

生成时间：2026-10-17T16:38:31.751016+08:00（Asia/Shanghai）。开发/本地验证：Codex；独立审核待 Claude Code 执行；阶段验收由 PM 安排。

## 结果与基线

已新增 `.github/workflows/windows-ci.yml`，实现仅手动触发的 Windows x64 测试、正常 release 与未签名 NSIS 构建、release 验证 feature 拒绝检查及精确产物上传。**这是待审核的本地未提交改动，GitHub 构建未运行，不宣称云端成功或发布验收通过。**

- 项目：D:\nova。
- 完整 HEAD：`3216da284d90240b5e10853e6b03cf4289e0f4b4`。
- 分支：`feature/v0.1.0-mvp`；基线一致，无需切换或恢复。
- 仓库/上级未发现适用 AGENTS.md 或 CLAUDE.md。
- 已阅读指定 CI_READINESS_REPORT10171617、LOCAL_ARCHIVE_REPORT10171604 和两份 Claude WP2A 审核。
- `git remote -v` 无输出；GitHub 远程地址仍待 PM 提供。
- 既有 product 两份修改、未跟踪文档、封存/准备报告和 audit 证据保留。未修改源码、测试断言、依赖/锁、正式 tauri.conf.json 或已有文档。

本轮新增文件只有：

1. `.github/workflows/windows-ci.yml`
2. 本报告 `docs/reports/CODEX_WP2A_CI_IMPLEMENTATION_REPORT10171638.md`

没有新增永久辅助脚本：11 个步骤的 PowerShell 直接内联在 workflow，Claude 可在单个文件核对完整调用链。临时本地校验工具/夹具放系统 Temp，不作为 CI 依赖。

Workflow 原始字节 SHA256：`4a6bb5a516c3866958a33c72505c1a81b68e3f5231af223b5593d62dd39699b4`。

## 触发、权限与工具链

仅 `workflow_dispatch`，无 push、PR、tag、release、部署触发器。全局权限仅 `contents: read`；checkout `persist-credentials: false`。无业务 secrets、发布/部署/签名/仓库写操作。上传的是 Actions 验证 artifact，非 GitHub Release。

runner 为 `windows-2022`，90分钟 job 上限。Node 固定24.14.0、npm固定11.9.0、Rust固定1.98.0-x86_64-pc-windows-msvc。运行时检查准确版本、Node x64、Rust MSVC host、VS x64 C++组件和Windows SDK，并记录实际 runner image/VS路径。runner 镜像仍会更新，不声称位级可复现。

采用版本前完成的官方可用性/兼容性核验：

- [Node 24.14.0 官方校验清单](https://nodejs.org/dist/v24.14.0/SHASUMS256.txt)存在 Windows x64 zip，SHA256 为 `313fa40c0d7b18575821de8cb17483031fe07d95de5994f6f435f3b345f85c66`。
- [npm 11.9.0 官方 registry 元数据](https://registry.npmjs.org/npm/11.9.0)可取得，Node engines 为 `^20.17.0 || >=22.9.0`，与24.14.0相容。
- [Rust 1.98.0 官方分发 manifest](https://static.rust-lang.org/dist/channel-rust-1.98.0.toml)可取得，实际为1.98.0 (88d9e12ae 2026-08-18)，x86_64-pc-windows-msvc available=true。
- 提交中锁定 Vite 6.4.3 和 Vitest 2.1.9 的 Node engines 均允许24；本机当前版本也对应上述 Node/npm/Rust。真实本地 release 保护编译可运行到指定 compile_error，见本地验证。

### Action SHA 官方来源

下列官方 GitHub REST tag refs 均实际只读查询，返回 object.type=commit，并核对官方 action.yml 的输入与 Node24 runtime；没有猜测 SHA。与目标 Nova 仓库无远程交互。

| Action | release tag | 完整 SHA | 官方来源 |
|---|---|---|---|
| actions/checkout | v7.0.1 | `3d3c42e5aac5ba805825da76410c181273ba90b1` | [tag ref](https://api.github.com/repos/actions/checkout/git/ref/tags/v7.0.1) / [release](https://github.com/actions/checkout/releases/tag/v7.0.1) |
| actions/setup-node | v7.1.0 | `949feb2413d6458794dcd2491c4babbbce0c15c1` | [tag ref](https://api.github.com/repos/actions/setup-node/git/ref/tags/v7.1.0) / [release](https://github.com/actions/setup-node/releases/tag/v7.1.0) |
| actions/upload-artifact | v7.0.2 | `cf430e030ddbb5b0abf93d22962f4752f3646cd9` | [tag ref](https://api.github.com/repos/actions/upload-artifact/git/ref/tags/v7.0.2) / [release](https://github.com/actions/upload-artifact/releases/tag/v7.0.2) |

不启用依赖缓存，避免把本机/过往缓存命中当作前提。rustup直接安装准确版本，无额外Rust安装Action。首轮面向 GitHub.com hosted runner；若 PM 后续指定 GHES，需先核实所选 artifact Action 的支持范围与 runner 版本，不能直接宣称通用。

## 测试、构建与失败处理

工作目录均相对 checkout 的 GitHub workspace，临时输出使用 RUNNER_TEMP/System temp。无 D:\nova、未跟踪文档、audit 或本机缓存依赖。

顺序为：

1. 核验 checkout HEAD=GITHUB_SHA、工具链与受控锁文件，保存两份锁 SHA256与构建信息。
2. apps/desktop：`npm ci --no-audit --no-fund`，再 `npm test`。
3. apps/desktop/src-tauri：`cargo build --locked --features nova-test-helper --bin nova_test_helper`，预构建只供测试使用的helper。
4. 同目录：`cargo test --locked --lib app_paths::tests::accepts_ordinary_uuid_like_components --target-dir <系统Temp>/nova-self-referential-cargo-test-cache`。该路径严格匹配既有嵌套测试，默认feature相同，不设置CARGO_TARGET_DIR。把首次编译移到无内部300秒限额的独立步骤，不改测试断言/超时；不依赖预先存在缓存。
5. 同目录：`cargo test --locked --lib`运行完整默认后端库单测（既有674项所在范围）。
6. 同目录：`cargo test --locked --features wp2a-desktop-validation --lib isolated_validation::tests`，除退出码外核验摘要恰为3 passed、0 failed。
7. apps/desktop：临时JSON仅覆盖 `bundle.active=true`；执行：

```powershell
npm run tauri -- build --ci --no-sign --config $config --bundles nsis -- --locked
```

一次Tauri流程执行正式 beforeBuildCommand（vue-tsc+Vite）、默认正常release应用及NSIS，避免先no-bundle再重复构建。正式配置原文件不改；无 `--features`、`--all-features`或debug构建参数。不是用测试helper/隔离验证程序打包。

8. 独立执行 `cargo build --locked --release --features wp2a-desktop-validation --bin nova --message-format=json`，要求退出码101，恰有一条error级编译诊断、target为nova_lib、message精确为 `wp2a-desktop-validation is debug-only and must never ship`、code为空。成功退出、网络错误、无结构化指定诊断、其他错误或混合编译错误均不通过。正常release构建在它之前已必须成功。
9. 重算并比较两锁字节哈希，git diff再次检查两锁与正式配置无变化；核验两个精确产物非空且Authenticode状态为NotSigned，复制到专用临时目录，生成哈希与用途说明，上传白名单文件。

每个run步骤设置 `$ErrorActionPreference='Stop'`、`$PSNativeCommandUseErrorActionPreference=$true`，原生命令或管道失败立即终止。只有第8步临时允许非零，捕获LASTEXITCODE后恢复fail-fast；指定错误验证成功后显式将LASTEXITCODE清零，避免GitHub pwsh尾部包装器把预期失败再次当作步骤失败。未使用continue-on-error。依赖和构建失败时后续上传不会执行，日志仍在Actions步骤输出中。

## 精确产物与哈希

源路径（相对 GitHub workspace）：

- `apps/desktop/src-tauri/target/release/nova.exe`
- `apps/desktop/src-tauri/target/release/bundle/nsis/Nova_0.1.0_x64-setup.exe`

缺任一、为空或并非未签名，均fail；正常配置版本/产物命名以后改变时也会fail，不能静默上传其他exe。

上传白名单来自 `${{ runner.temp }}/wp2a-validation-artifacts/`，准确包含：

- `nova.exe`
- `Nova_0.1.0_x64-setup.exe`
- `SHA256.json`（Get-FileHash -Algorithm SHA256，包含每个产品原路径、文件名、字节数和小写哈希）
- `build-info.json`（实际GITHUB_SHA/ref/run_id、工具/镜像/VS信息、原始两锁哈希）
- `VALIDATION-ONLY.txt`
- `frontend-tests.log`、`backend-tests.log`、`facility-tests.log`、`normal-build.log`、`release-guard.log`

artifact名 `WP2A-validation-only-unsigned-NOT-A-RELEASE-${{ github.sha }}`，保留7天，if-no-files-found=error，禁止隐藏文件；预复制逐项检查避免多个上传路径仅部分存在也被视为成功。无target/audit通配符，无数据库/WebView、helper、假CLI或验证程序。SHA256.json在CI实际构建后生成，本轮不为尚未生成的云端产物编造哈希。

## 本地验证结果

| 检查/实际命令 | 结果与边界 |
|---|---|
| PyYAML 6.0.3解析，静态断言手动触发、只读权限、40位SHA、正常构建feature禁用、锁参数和作用域 | PASS |
| PowerShell 7 AST解析全部内联run脚本 | PASS，无语法错误 |
| actionlint v1.7.12：`actionlint.exe -shellcheck= -pyflakes= .github/workflows/windows-ci.yml` | PASS，退出0无诊断；PowerShell另由自身AST检查 |
| actionlint工具取得 | 官方release Windows amd64 zip，并对官方checksums核验 SHA256 `6e7241b51e6817ea6a047693d8e6fed13b31819c9a0dd6c5a726e1592d22f6e9`；仅存系统Temp |
| 已锁定CLI实际参数：`node apps/desktop/node_modules/@tauri-apps/cli/tauri.js build --help` | PASS，实际2.11.4的帮助确认--ci、--no-sign、--config JSON路径、--bundles nsis及`--`后的runner参数；并对照已提交package-lock锁定版本 |
| 保护检查PowerShell本地夹具：指定错误+101、成功退出、网络失败、其他编译错误、混合错误 | 五组PASS；仅第一组允许步骤成功，含GitHub尾部LASTEXITCODE行为模拟 |
| 原生失败管道夹具：`cmd /c exit 17`经Tee-Object | PASS，立即终止，未执行后续语句 |
| 真实本地保护构建：`cargo build --locked --release --features wp2a-desktop-validation --bin nova --message-format=json`（cwd apps/desktop/src-tauri） | PASS：Cargo退出101且唯一错误为指定compile_error；实际运行提取出的workflow检查块后退出0。Rust 1.98.0；不是正常release构建成功声明 |
| 额外离线metadata：`cargo metadata --locked --offline --format-version 1 --manifest-path apps/desktop/src-tauri/Cargo.toml` | 未通过：本机缓存缺android_system_properties v0.1.6，offline禁止下载；不能据此宣称全部平台依赖可离线解析。CI不设offline，会从registry取锁定依赖 |
| 新workflow diff/空白与引用输入检查 | 无空白诊断；所有输入属于Git受控源码/锁文件，node_modules/CLI为npm ci生成；workflow本身是本轮明确新增待纳入文件 |
| 历史保护 | tracked binary diff SHA256与本轮前一致，Git状态只多本轮新增文件；两锁、源码、正式配置无改动，暂存区空 |

上述本地夹具及真实保护编译日志在 `C:\Users\Lenovo\AppData\Local\Temp\nova-ci-harness`，不提交、不作为workflow输入。校验action公开源是只读HTTP查询，未操作Nova远程。

NOT RUN：本轮没有在本地重装npm依赖、重跑674/109/3整套测试、预热完整冷缓存或执行正常Tauri/NSIS构建；已有业务字节未改，本轮重点验证workflow、命令语义及保护检查。仍需首轮Actions检验完整环境、耗时、NSIS工具下载、实际产物命名/签名状态及上传权限。GitHub runner/Actions执行与artifact上传全部NOT RUN，因本轮禁止远程执行且目标未指定。原生输入/视觉、安装后启动验收仍待完成；安装包构建即使成功也不代表这些验收通过。

## Claude 独立审核定位与diff

本轮没有暂存，`git diff`不会自动显示未跟踪workflow。读取新增文件，或只读执行：

```powershell
git diff --no-index -- NUL .github/workflows/windows-ci.yml
```

有diff时退出1属于正常，不是工具失败；不要为查看diff执行git add。

关键位置：

- `.github/workflows/windows-ci.yml:4`：触发
- `.github/workflows/windows-ci.yml:7`：权限
- `.github/workflows/windows-ci.yml:11`：runner
- `.github/workflows/windows-ci.yml:34`：工具/锁基线
- `.github/workflows/windows-ci.yml:79`：helper/嵌套预热
- `.github/workflows/windows-ci.yml:90`：默认后端
- `.github/workflows/windows-ci.yml:97`：设施测试
- `.github/workflows/windows-ci.yml:107`：默认release+NSIS
- `.github/workflows/windows-ci.yml:121`：保护检查
- `.github/workflows/windows-ci.yml:149`：锁/产物检查
- `.github/workflows/windows-ci.yml:185`：上传白名单

重点复核：官方SHA/输入；手动触发与读权限；嵌套目录匹配及helper路径；单次正常NSIS构建无测试feature；PowerShell原生命令/预期失败处理；两锁检查；两个产物同时存在、未签名和上传白名单。源码仍以原封存审核为准，本轮不重新展开WP2A业务或Phase5A。

## 回滚与停止状态

当前未提交。若PM决定撤销本轮，仅在核对内容/哈希并确认无他人后续编辑后，移除本轮新增的 `.github/workflows/windows-ci.yml` 与本报告；不删除整个.github/docs目录，不动任何既有文件、product改动或audit，不执行reset/clean/stash/整文件恢复。本轮没有数据库或配置迁移需撤销。

工作流新增待Claude独立审核。未提交、未配置remote、未创建仓库、未push、未启动GitHub Actions、未发布、未创建tag或Release。远程地址仍待PM提供，本轮到此停止。
