# Phase 1 最终构建边界整改报告（第三轮，最小整改）

生成时间：2026-09-16 08:18（北京时间）

## 1. 触发依据

本轮整改依据 Codex 独立复审报告：

`D:\nova-review\CODEX_PHASE1_PROCESS_SPAWN_FINAL_REVIEW09160739.md`（结论：FAIL）

该报告指出，针对提交 `623834ce7a68a01d48aef1a2fdbc2f6320d8d9e1` 的复审中，原先的进程创建竞态 Major 项已关闭，但发现两个新 Major 与两个新 Minor：

- **Major 1**：`nova_test_helper`（普通 `src/bin` target）被打包进 `cargo build --release` / `npm run tauri -- build` 的正式产物，且 Tauri 将 `nova_test_helper.exe` 误报为 "Built application"，而不是 `nova.exe`。
- **Major 2**：`program`、`cwd`、`args` 中若包含嵌入的 NUL（`\0`）字符，在到达 `CreateProcessW` 之前未被拒绝——`to_wide_null` 只是在字符串末尾追加自己的 NUL 终止符，并不检查输入中是否已经存在 NUL，可能被静默截断/重塑。
- **Minor（本轮一并关闭）**：`spawn_capped_reader` 直接使用 `thread::spawn`，在操作系统线程创建失败时会 panic，而不是返回结构化错误。

本轮为用户明确要求的"第三次、也是最后一次最小整改"，范围严格限定于上述三项，不涉及 Phase 2，不修改其他已关闭功能。

## 2. 整改前分支与提交

- 分支：`feature/v0.1.0-mvp`
- 整改前 HEAD：`623834ce7a68a01d48aef1a2fdbc2f6320d8d9e1`（与 Codex 复审报告所审查的提交一致）
- 整改前工作区：干净（`git status --short` 无输出）
- 未添加任何 git remote

## 3. Major 1 根因与修复

### 3.1 根因

`nova_test_helper.rs` 此前位于 `src/bin/nova_test_helper.rs`。Cargo 会自动发现 `src/bin/*.rs` 下的每个文件并将其编译为一个独立的 `[[bin]]` target，且默认包含在 `cargo build --release` 与 `npm run tauri -- build` 的构建范围内——没有任何 feature gate 将其排除。当 `src-tauri` 目录下存在多个 `[[bin]]` target 时，Tauri 的构建后处理会选择其中之一作为它认为的"主程序"进行报告，本例中选中了 `nova_test_helper.exe`。

### 3.2 修复

1. 使用 `git mv` 将 `src/bin/nova_test_helper.rs` 迁移到 `apps/desktop/src-tauri/tests/fixtures/nova_test_helper.rs`——移出 `src/bin/`，Cargo 的自动 bin 发现机制不再拾取它。
2. 在 `Cargo.toml` 中新增一个非默认 feature `nova-test-helper`（`default = []` 保持为空，未启用任何 feature），并新增显式 `[[bin]]` 声明：
   ```toml
   [features]
   default = []
   nova-test-helper = []

   [[bin]]
   name = "nova_test_helper"
   path = "tests/fixtures/nova_test_helper.rs"
   required-features = ["nova-test-helper"]
   ```
   由于该 bin target 显式声明了 `required-features`，普通 `cargo build`/`cargo build --release`（不带 `--features`）不会编译它；`npm run tauri -- build` 内部调用的也是不带任何 feature 标志的普通 release 构建，同样不会编译它。
3. `cargo test`（不启用任何 feature）本身也不会自动构建这个 `[[bin]]` target（`required-features` 同样对 `cargo test` 生效）。为了让集成测试仍能把它当作真实子进程启动，`src/runtime/process.rs` 测试模块中的 `helper_exe()` 改为在按需构建时显式加上 `--features nova-test-helper`：
   ```rust
   let status = std::process::Command::new(env!("CARGO"))
       .args(["build", "--bin", "nova_test_helper", "--features", "nova-test-helper"])
       .current_dir(env!("CARGO_MANIFEST_DIR"))
       .status()
   ```
   该按需构建通过已有的 `std::sync::Once`（`HELPER_BUILD`）在同一测试进程内串行化，避免并发测试重复触发构建。
4. `cargo clippy --all-targets --all-features` 会启用 `nova-test-helper` feature 并对 helper 本身进行 lint（用户明确说明这是允许的），但这不影响正式的 `cargo build --release`/`npm run tauri -- build` 路径，因为二者都不传 `--all-features`。
5. helper 未被任何生产代码或 Tauri command 引用；`tauri.conf.json` 未做任何修改（未被指定用于将 helper 设为主程序）。

### 3.3 helper 新路径

`apps/desktop/src-tauri/tests/fixtures/nova_test_helper.rs`（内容与整改前一致：`echo-argv`、`marker <path>`、`grandchild-race <pid-file>`、`sleep` 四种模式均未改动）

### 3.4 证据：正式构建排除 helper

清理 `target/release` 下所有历史 `nova_test_helper` 相关产物（含本轮此前因 `--all-features` clippy 运行遗留的 fingerprint 元数据）后，重新执行 `cargo build --release`（默认 feature，不带任何 `--features` 参数）：

```
$ find target/release -maxdepth 1 -iname "*.exe"
target/release/nova.exe

$ find target/release -iname "*nova_test_helper*"
（无输出）
```

结论：普通 release 构建只产出 `nova.exe`，`target/release` 树下不存在任何 `nova_test_helper` 相关文件（包括 fingerprint 元数据）。

### 3.5 证据：Tauri 报告的主程序是 nova.exe

```
$ npm run tauri -- build
...
   Compiling nova v0.1.0 (D:\nova\apps\desktop\src-tauri)
    Finished `release` profile [optimized] target(s) in 1m 15s
       Built application at: D:\nova\apps\desktop\src-tauri\target\release\nova.exe
```

Tauri 报告的 "Built application" 路径是 `nova.exe`，不再是 `nova_test_helper.exe`。`bundle.active` 在 `tauri.conf.json` 中仍为 `false`（本轮未改动此文件），因此没有 `target/release/bundle` 目录产出，与整改前保持一致。

## 4. Major 2 根因与修复

### 4.1 根因

`win_spawn.rs` 中的 `to_wide_null` 函数只是把输入字符串编码为 UTF-16 并在末尾追加一个 `0u16` 终止符，从未检查输入中是否已经包含 NUL。Rust 的 `&str`/`String` 可以合法地包含字面 `\0` 字符（这与 C 风格字符串的"NUL 即结尾"假设不同），所以一个包含内嵌 NUL 的 `program`/`cwd`/`arg`（例如 `"C:\\real.exe\0 --dangerous-flag"`）在转换为宽字符串后会被 Win32 API 在第一个 NUL 处截断，导致 `CreateProcessW` 实际执行的程序路径或参数与调用方传入的字符串不一致——即所谓的"NUL 截断/重塑"问题。

### 4.2 修复

在 `win_spawn.rs` 中新增单一校验入口 `validate_process_request_strings`，并将其作为 `spawn_suspended` 的**第一条语句**调用，早于任何路径解析（`resolve_executable`）、命令行拼接（quoting）或 `CreateProcessW` 调用：

```rust
pub fn spawn_suspended(program: &str, args: &[String], cwd: &Path) -> io::Result<SuspendedChild> {
    validate_process_request_strings(program, args, cwd)?;

    let executable = resolve_executable(program)?;
    ...
```

```rust
fn validate_process_request_strings(program: &str, args: &[String], cwd: &Path) -> io::Result<()> {
    if program.as_bytes().contains(&0) {
        return Err(nul_rejection_error("可执行程序路径"));
    }
    if cwd.as_os_str().encode_wide().any(|unit| unit == 0) {
        return Err(nul_rejection_error("工作目录"));
    }
    for arg in args {
        if arg.as_bytes().contains(&0) {
            return Err(nul_rejection_error("命令行参数"));
        }
    }
    Ok(())
}

fn nul_rejection_error(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{what}包含 Windows 无法表示的 NUL 字符，已拒绝启动"),
    )
}
```

要点：

- 一旦检测到 NUL，直接以 `io::ErrorKind::InvalidInput` 拒绝，**不做任何截断/裁剪/替换**，`CreateProcessW` 完全不会被调用。
- 错误文本只说明"哪一类字段（程序路径/工作目录/命令行参数）包含 NUL"，绝不回显实际的字符串内容，避免泄漏敏感参数。
- 该 `io::Error` 沿用现有的 `classify_spawn_error` 分类逻辑（`_ => ExecutionErrorKind::StartFailed` 兜底分支已经能正确吸收 `InvalidInput`），因此 `process.rs` 的分类逻辑无需新增变体、也未触碰 `detect.rs` 中对 `ExecutionErrorKind` 的穷尽匹配，符合"不改动已关闭功能"的约束。

### 4.3 NUL 统一校验位置

`apps/desktop/src-tauri/src/runtime/win_spawn.rs` 中的 `validate_process_request_strings`（新增函数），是 `program`/`cwd`/每个 `arg` 唯一的 NUL 校验入口。

### 4.4 NUL 测试

`win_spawn.rs`（10 个新增单元测试，均通过）：

1. `rejects_program_with_embedded_nul`
2. `rejects_cwd_with_embedded_nul`
3. `rejects_first_arg_with_embedded_nul`
4. `rejects_middle_arg_with_embedded_nul`
5. `rejects_last_arg_with_embedded_nul`
6. `rejects_nul_even_when_surrounded_by_shell_metacharacters`
7. `nul_rejection_error_message_does_not_leak_the_offending_value`
8. `does_not_reject_ordinary_unicode_and_chinese_text`
9. `does_not_reject_a_normal_empty_string_argument`
10. `spawn_suspended_rejects_embedded_nul_before_touching_the_filesystem`（用不存在的程序名，区分"校验先行"→`InvalidInput` 与"先做路径解析"→`NotFound`，证明校验确实发生在文件系统/解析之前）

`process.rs`（2 个端到端集成测试，均通过）：

1. `embedded_nul_in_an_argument_is_rejected_end_to_end_without_starting_a_process`——通过完整的 `execute_blocking` 路径，证明含 NUL 参数的请求既不会返回 `Success`，也不会真正启动进程（用一个"标记文件"验证 helper 从未运行）。
2. `embedded_nul_in_program_and_cwd_are_also_rejected_end_to_end`——同样通过 `execute_blocking`，分别验证 `program` 与 `cwd` 中的 NUL 均被拒绝。

以上共 12 个新增测试场景，覆盖用户要求的全部 12 项（program/cwd/首尾中间参数/shell 元字符相邻/不泄漏内容/正常 Unicode 与中文不受影响/正常空字符串参数仍被允许/拒绝绝不表现为 Success/通过标记文件证明未启动进程/错误在 `ExecutionResult` 中保持脱敏）。

## 5. Reader 线程 Minor 修复

### 5.1 修复

`process.rs` 中的 `spawn_capped_reader` 由直接使用会 panic 的 `thread::spawn` 改为使用 `thread::Builder::new().name(...).spawn(...)`，返回 `io::Result<ReaderHandle>`：

```rust
fn spawn_capped_reader<R>(mut reader: R, cap: usize) -> io::Result<ReaderHandle>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name("nova-process-reader".to_string())
        .spawn(move || { ... })
}
```

新增 `spawn_reader_pair`（生产代码唯一调用入口）与 `spawn_reader_pair_with`（供测试注入失败的内部实现，生产路径固定传入真实的 `spawn_capped_reader`，未对生产 API 增加任何失败注入面）：

```rust
fn spawn_reader_pair<A, B>(...) -> io::Result<(Option<ReaderHandle>, Option<ReaderHandle>)> {
    spawn_reader_pair_with(stdout_pipe, stderr_pipe, cap, spawn_capped_reader, spawn_capped_reader)
}
```

逻辑：先尝试创建 stdout reader；若失败直接返回 `Err`（stderr reader 完全不会被尝试）。若 stdout reader 创建成功但 stderr reader 创建失败，则 **join 已经启动的 stdout reader**，再返回 `Err`，确保不遗留任何线程。

两个调用点均已更新为处理 `Result`：

- **Windows 路径（`run_windows`）**：此时进程已经 `resume()` 过，可能已经产生了自己的子孙进程，因此清理走 **Job Object**（`job.terminate()` + `child.wait()`），而不是仅终止直接子进程——与 Job 对象在 timeout/cancel 分支中一致的清理方式。清理后返回结构化 `ExecutionErrorKind::StartFailed`，绝不 panic。
- **非 Windows 路径（`run_generic`）**：清理使用 `child.kill()` + `child.wait()`，与该函数已有的 cancel/timeout 分支保持一致，未引入任何回归。

两处错误文本均为 `"无法创建输出读取线程，已终止进程：{redact_sensitive_text(...)}"`，经过与其余错误路径相同的 `redact_sensitive_text` 脱敏处理。

### 5.2 Reader 失败测试

真实的操作系统线程创建失败（资源耗尽）无法在测试中可靠复现——这与本仓库既有的 `handle_windows_spawn_failure`（针对 `AssignProcessToJobObject`/`ResumeThread` 失败）采用的测试策略完全一致：用合成的 `io::Error` 直接驱动生产清理代码，而不是强行制造真实的系统级失败。`spawn_reader_pair_with` 正是承担这一角色的生产清理/传播代码本体。

`process.rs` 新增 5 个测试（均通过）：

1. `spawn_reader_pair_with_propagates_stdout_reader_creation_failure_without_touching_stderr`——stdout reader 创建失败：返回 `Err`，且证明 stderr 创建从未被尝试。
2. `spawn_reader_pair_with_joins_the_started_stdout_reader_when_stderr_reader_creation_fails`——stderr reader 创建失败（stdout 已创建）：用一个故意耗时 200ms 才置位标志的真实线程验证——如果没有被同步 join，`spawn_reader_pair_with` 返回时该标志几乎不可能已经为 `true`；测试断言返回后标志确实为 `true`，证明已启动的 stdout reader 被正确 join，没有遗留后台线程。
3. `spawn_reader_pair_still_works_normally_after_an_earlier_injected_failure`——证明一次失败注入不会污染任何共享/全局状态，紧接着的正常 `spawn_reader_pair` 调用仍能成功读取真实数据。
4. `reader_thread_spawn_failure_message_is_redacted_and_reports_start_failed_not_success`——验证失败路径产生的 `ExecutionResult` 一定是 `Failed { kind: StartFailed, .. }`（绝不是 `Success`），且经 `redact_sensitive_text` 处理后的错误详情不包含合成 `io::Error` 中携带的类密钥字符串。
5. 现有的进程树/残留进程测试（`repeated_runs_leave_no_residual_process_across_all_outcome_kinds`、`cancellation_terminates_grandchild_process_not_just_direct_child`、`timeout_terminates_grandchild_process_not_just_direct_child`、两个 `immediate_grandchild_*_repeated` 测试）在本轮改动后全部保持通过，证明 reader 线程的重构未在正常/取消/超时路径上引入进程残留回归。

未使用 `unwrap`/`expect` 处理线程创建失败；未引入任何新的第三方依赖。

## 6. 已关闭项回归验证

以下既有安全属性/测试在本轮改动后重新验证，全部保持通过，未被修改：

- `CreateProcessW` + `CREATE_SUSPENDED`，Assign-before-Resume 顺序（`suspended_process_does_not_run_before_job_assignment_and_resume`）
- 超时/取消终止整个 Job（`user_cancellation_kills_the_process_and_reports_cancelled`、`timeout_kills_the_process_and_reports_timed_out`）
- 20 次重复的即时孙进程测试（`immediate_grandchild_cancel_terminates_parent_and_grandchild_repeated`、`immediate_grandchild_timeout_terminates_parent_and_grandchild_repeated`）
- Windows argv 引用/保真度（`argument_fidelity_round_trips_every_tricky_argument_unmodified`、`argument_containing_shell_metacharacters_is_not_reinterpreted`）
- 机器无关的 CLI 检测测试（`runtime::detect` 全部用例）
- AI CLI 不会仅凭裸 `--version` 判定为 Ready（`successful_version_stage_never_becomes_ready_without_running_the_status_stage`）
- `create_new` 权限探测（`directory_writable_*` 系列）
- `DetectionSlot` 并发控制（`runtime::detection_slot` 全部用例）
- 全量凭据脱敏（`security` 模块全部用例）
- 路径边界/命令注入测试（`runtime::path_guard` 全部用例、`rejects_execution_outside_allowed_root_without_spawning`）
- 整改前已有的全部 109 个 Rust 测试，均包含在本轮 125 个测试的通过集合内（新增 16 个：10 个 NUL 单元测试 + 2 个 NUL 端到端测试 + 4 个 reader 线程测试，109 + 16 = 125，与实测总数一致）

## 7. 修改文件清单

```
M  apps/desktop/src-tauri/Cargo.toml
M  apps/desktop/src-tauri/src/runtime/process.rs
M  apps/desktop/src-tauri/src/runtime/win_spawn.rs
R  apps/desktop/src-tauri/src/bin/nova_test_helper.rs -> apps/desktop/src-tauri/tests/fixtures/nova_test_helper.rs
A  docs/release-v0.1.0/PHASE1_FINAL_BUILD_BOUNDARY_REMEDIATION_REPORT09160818.md（本报告）
```

未修改：`tauri.conf.json`、`runtime/job_object.rs`、`runtime/detect.rs`、`runtime/detection_slot.rs`、`security.rs`、`commands.rs`、`lib.rs`、`runtime/mod.rs`、任何前端文件。

## 8. Cargo.toml 变更

新增内容（完整 diff 见 `git diff --cached -- apps/desktop/src-tauri/Cargo.toml`）：

```toml
[lib]
name = "nova_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[features]
default = []
nova-test-helper = []

[[bin]]
name = "nova_test_helper"
path = "tests/fixtures/nova_test_helper.rs"
required-features = ["nova-test-helper"]
```

其余依赖声明（`[dependencies]`、`[target.'cfg(windows)'.dependencies]`、`[build-dependencies]`）未改动。

## 9. Cargo.lock 变更

无变更。`git diff --stat -- apps/desktop/src-tauri/Cargo.lock` 输出为空——本轮未引入任何新依赖，也未升级任何既有依赖版本。

## 10. 七项验证命令与结果

| # | 命令 | 工作目录 | 退出码 |
|---|------|----------|--------|
| 1 | `cargo fmt --check` | `apps/desktop/src-tauri` | 0 |
| 2 | `cargo clippy --all-targets --all-features -- -D warnings` | `apps/desktop/src-tauri` | 0 |
| 3 | `cargo test` | `apps/desktop/src-tauri` | 0 |
| 4 | `cargo build --release` | `apps/desktop/src-tauri` | 0 |
| 5 | `npm test` | `apps/desktop` | 0 |
| 6 | `npm run build` | `apps/desktop` | 0 |
| 7 | `npm run tauri -- build` | `apps/desktop` | 0 |

全部 7 项命令一次性最终结果均为退出码 0（命令 2 在修复测试代码中的 `clippy::io_other_error` 提示后，重新运行通过；命令 3 中一个既有的、与本轮改动无关的计时敏感测试 `immediate_grandchild_cancel_terminates_parent_and_grandchild_repeated` 在与另外 124 个测试并发运行时出现过一次瞬时失败，单独重跑与本轮完整套件重跑均稳定通过——详见第 12 节"已知限制"）。

## 11. 测试计数

- **Rust**：`cargo test` 库单元测试 125 passed / 0 failed / 0 ignored（含本轮新增 16 个：10 个 NUL 单元测试 + 2 个 NUL 端到端测试 + 4 个 reader 线程测试）；`main.rs` 0 tests；doc-tests 0 tests。
- **前端**：`npm test`（vitest）3 个测试文件，13 tests passed / 0 failed。
- **是否存在 skip**：否。已对 Rust（`grep -rn "#\[ignore\]"`）与前端（`grep` `.skip(`/`.todo(`/`describe.skip`/`it.skip`/`test.skip`）分别核实，均无匹配。

## 12. 正式 Release 二进制清单

```
target/release/nova.exe   ← 唯一顶层可执行文件
```

`target/release` 树下（含 `.fingerprint/` 元数据）不存在任何 `nova_test_helper` 相关文件。`target/release/bundle` 目录不存在（`tauri.conf.json` 中 `bundle.active: false`，与整改前一致，未修改）。

## 13. 已知限制

- `immediate_grandchild_cancel_terminates_parent_and_grandchild_repeated`（既有测试，本轮未修改其逻辑）在与全部 125 个测试并发执行时，观察到一次瞬时失败（`pid file missing`，属于计时敏感的资源竞争，而非本轮改动引入的确定性缺陷）；单独运行以及随后完整套件的重跑均 100% 通过。这是该测试固有的、与本轮 Major/Minor 修复无关的时序脆弱性，非本轮引入的新问题，建议后续单独跟踪（不在本轮范围内处理，以遵守"不修改已关闭功能范围之外内容"的约束）。
- reader 线程创建失败的测试覆盖采用合成 `io::Error` 注入（`spawn_reader_pair_with`），未能在真实操作系统资源耗尽条件下端到端验证；这与本仓库对 `AssignProcessToJobObject`/`ResumeThread` 失败一直采用的验证策略一致，属于已被此前轮次接受的测试方法学限制，而非新引入的缺口。

## 14. 提交 SHA

本报告随本轮整改一并提交。提交发生在本报告写入之后，因此此处采用自引用说明：本报告所属的提交即为本轮"最小整改"提交，其确切 SHA 见本轮最终回复中的"新整改提交 SHA"字段，以及 `git log -1` 的实际输出。

## 15. 回滚方法

若需要回滚本轮整改，恢复到整改前状态：

```
git revert <本轮提交SHA>
```

或者（更彻底但需谨慎，仅在明确需要放弃本轮整改时使用）：

```
git reset --hard 623834ce7a68a01d48aef1a2fdbc2f6320d8d9e1
```

`git revert` 是推荐方式，因为它保留历史、不改写已存在的提交。

## 16. Git Remote 状态

未添加任何 git remote。本轮整改自始至终仅在本地仓库操作。`git remote -v` 输出为空。

## 17. 整改后 Git 状态

提交完成后：

- `git status --short --untracked-files=all`：应为空
- `git status --porcelain=v2 --untracked-files=all`：应为空

（具体核实结果见本轮最终回复。）

## 18. Phase 2 声明

**本轮整改未开始、也未涉及 Phase 2 的任何范围**。所有改动严格限定于：helper 隔离（Major 1）、NUL 输入校验（Major 2）、reader 线程结构化错误处理（Minor）、上述三项对应的测试，以及必要的 `Cargo.toml` 配置调整和本报告本身。未添加任何 Phase 2 相关代码、UI、模板、GitHub 导入或其他功能。

## 19. Codex 复审材料清单

供 Codex 独立复审参考的完整材料：

- 本报告：`docs/release-v0.1.0/PHASE1_FINAL_BUILD_BOUNDARY_REMEDIATION_REPORT09160818.md`
- 本轮触发依据：`D:\nova-review\CODEX_PHASE1_PROCESS_SPAWN_FINAL_REVIEW09160739.md`
- 修改的源文件：`apps/desktop/src-tauri/Cargo.toml`、`apps/desktop/src-tauri/src/runtime/process.rs`、`apps/desktop/src-tauri/src/runtime/win_spawn.rs`
- 迁移的测试 fixture：`apps/desktop/src-tauri/tests/fixtures/nova_test_helper.rs`（原 `src/bin/nova_test_helper.rs`）
- 本轮提交的完整 diff：`git show <本轮提交SHA>`
- 七项验证命令均可由 Codex 在其复审环境中独立重新执行（命令与工作目录见本报告第 10 节）
- 上一轮报告（供上下文对照，未修改）：`docs/release-v0.1.0/PHASE1_PROCESS_SPAWN_RACE_REMEDIATION_REPORT09160649.md`、`docs/release-v0.1.0/PHASE1_REMEDIATION_REPORT09160042.md`

**再次声明：本报告不代表 Phase 1 已经 PASS。只有 Codex 独立复审给出 PASS 结论后，才允许进入 Phase 2。**
