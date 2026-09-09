# Claude Code 最终独立复审报告 — Nova Technical MVP

- 审核性质：最终上线门禁复审，只读检查 + 实际运行测试/构建，未修改任何代码、配置、文档或 Git 历史。
- 审核日期：2026-08-30
- 本轮唯一新增/修改的文件：本报告本身 (`D:\nova\audit\CLAUDE_REVIEW_NOVA_TECHNICAL_MVP_FINAL.md`)。运行 `cargo test`/`npm run build`/`npm run tauri -- build` 等指令产生的 `target/`、`dist/`、`nova.exe` 等构建产物按任务要求执行，但均已被 `.gitignore` 排除，不属于源码/配置/文档变更。

## 1. 最终结论

**CONDITIONAL PASS**

F1、F2、F3、F4 均已被正确修复并有充分的自动化回归测试与真实构建验证；F5 的技术债文档本身撰写完整、准确。但本轮复审在验证"当前 UI 确实禁止并发 `send_chat_message`"这一 F5 前提时，发现前端 `sendMessage()` 存在一个未被文档覆盖的窄口径并发入口（见下文 F5 小节与"剩余问题"）。该问题严重级别低，后果有界（不会造成单条消息内容损坏或密钥/越权问题），且有明确、廉价的修复方式，因此不构成 REJECT 理由，但按用户既定的 PASS 判定规则（"F5 已登记且当前 UI 确实禁止并发请求"）不能被判定为无条件 PASS，故给出 CONDITIONAL PASS。

## 2. 当前分支与 HEAD Commit

```
cd D:\nova
git branch --show-current   -> feature/nova-technical-mvp
git rev-parse HEAD          -> 465d3c8b29ef6b62b66e6de70a2ab91ede416ad8
```

`git status --short`（审核开始时）：

```
?? audit/CLAUDE_REVIEW_NOVA_TECHNICAL_MVP.md
```

只有一个未跟踪文件，是上一轮"只读审核"阶段生成的原始审核报告本身（该文件从未被要求提交），不属于任何代码/配置/文档的未提交修改。**工作区在审核开始前对源码而言是 clean 的**，不影响本次结论。

`git log -10 --oneline --decorate`：

```
465d3c8 (HEAD -> feature/nova-technical-mvp) fix: preserve stream integrity and cancellation
e0de353 chore: harden CSP and build traceability
3e1d2dd feat: add nova technical mvp
e0f1a7b (master) chore: initialize nova audit baseline
```

`git branch -a`：只有 `feature/nova-technical-mvp`（当前）和 `master`（仍停留在基线提交 `e0f1a7b`）。**本地不存在 `develop` 分支，未发生任何合并**。

`git diff`（working tree vs HEAD）：空，无任何未提交的源码/配置/文档改动。

`git diff HEAD~2..HEAD --stat`：

```
 .gitignore                               |   3 +
 apps/desktop/src-tauri/Cargo.lock        |  13 ++
 apps/desktop/src-tauri/Cargo.toml        |   1 +
 apps/desktop/src-tauri/src/api.rs        | 296 ++++++++++++++++++++++++++++---
 apps/desktop/src-tauri/src/commands.rs   |  95 +++++++---
 apps/desktop/src-tauri/src/credential.rs |   4 +-
 apps/desktop/src-tauri/src/db.rs         |  62 +++++--
 apps/desktop/src-tauri/tauri.conf.json   |   3 +-
 apps/desktop/src/App.vue                 |   8 +-
 apps/desktop/src/chatEvents.test.ts      |  29 +++
 apps/desktop/src/chatEvents.ts           |  26 +++
 docs/NOVA_DEVELOPMENT_WORKFLOW.md        |  19 ++
 docs/NOVA_SECURITY_SPEC.md               |   8 +
 docs/NOVA_TECHNICAL_DEBT.md              |  97 ++++++++++
 docs/NOVA_TEST_PLAN.md                   |  10 ++
 scripts/verify-build.ps1                 | 256 ++++++++++++++++++++++++++
 16 files changed, 855 insertions(+), 75 deletions(-)
```

（`git diff HEAD~2..HEAD` 全文已逐文件审阅，关键部分摘录于第 5 节。）

## 3. 审核范围

- `D:\nova\docs\*`（全部 8 篇文档）
- `D:\nova\audit\*`（含本报告依据的原始审核报告与 SDKMAX 接入审计三件套）
- `D:\nova\apps\desktop`（Rust/Tauri 后端 + Vue 前端全部源码，逐文件重新审阅，不信任上一轮的"已修复"结论）
- `D:\nova\scripts\verify-build.ps1`、`D:\nova\test-results\*`
- Git 历史：`e0f1a7b` → `3e1d2dd` → `e0de353` → `465d3c8`（当前 HEAD）
- 实际运行：`cargo fmt --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test`、`npm test`、`npm run build`、`npm run tauri -- build`，以及一次生产可执行文件的启动/渲染核验

`D:\sdkmax` 未被读取用于修改判断之外的任何操作，也未做任何修改核验之外的访问；本轮确认其内容较基线审计时间点无任何更新（见第 8 节）。

## 4. 两个整改 Commit 及修改范围

### `e0de353` — "chore: harden CSP and build traceability"（对应上一轮 Claude Code 受限开发会话，负责 F3/F4/F5）

```
.gitignore                             |   3 +
apps/desktop/src-tauri/tauri.conf.json |   3 +-
docs/NOVA_DEVELOPMENT_WORKFLOW.md      |  19 +++
docs/NOVA_SECURITY_SPEC.md             |   8 ++
docs/NOVA_TECHNICAL_DEBT.md            |  97 +++++++++++++
docs/NOVA_TEST_PLAN.md                 |  10 ++
scripts/verify-build.ps1               | 256 +++++++++++++++++++++++++++++++++
```

未触碰 `api.rs`/`commands.rs`/`active_cancel` 相关文件。范围与承诺一致。

### `465d3c8` — "fix: preserve stream integrity and cancellation"（对应 Codex，负责 F1/F2）

```
apps/desktop/src-tauri/Cargo.lock        |  13 ++
apps/desktop/src-tauri/Cargo.toml        |   1 +
apps/desktop/src-tauri/src/api.rs        | 296 ++++++++++++++++++++++++++++---
apps/desktop/src-tauri/src/commands.rs   |  95 +++++++---
apps/desktop/src-tauri/src/credential.rs |   4 +-
apps/desktop/src-tauri/src/db.rs         |  62 +++++--
apps/desktop/src/App.vue                 |   8 +-
apps/desktop/src/chatEvents.test.ts      |  29 +++
apps/desktop/src/chatEvents.ts           |  26 +++
```

范围基本符合 F1/F2 职责，但包含两处轻微越界（均已核实为低风险、非功能性）：

- `Cargo.toml`/`Cargo.lock` 新增 `tokio`（`macros`/`rt-multi-thread`/`time` features）依赖 — 这是实现 `tokio::select!` 抢占式取消（F2）**必须**的依赖，属于合理范围内变更，不算越界。
- `credential.rs` 有 2 行与 F1/F2 无关的纯风格重写：`bytes.len() % 2 != 0` → `!bytes.len().is_multiple_of(2)`，`chunks_exact(2)` → `chunks(2)`。经核实：`decode_blob` 的输入字节数在调用前已由上一行保证是偶数，`chunks_exact(2)` 与 `chunks(2)` 在偶数长度下产生完全相同的切片序列，**功能等价，无回归风险**；应为运行 `cargo clippy -D warnings` 或类似格式化工具时顺手产生的改动。建议后续提交中说明或收窄提交范围，但不构成本轮阻断项。

两次提交**没有互相覆盖**：`e0de353` 改的 `tauri.conf.json`/`docs/NOVA_TECHNICAL_DEBT.md` 等文件在 `465d3c8` 中未再出现，`465d3c8` 改的 `api.rs`/`commands.rs` 等文件在 `e0de353` 中未出现过。两者是纯粹的增量叠加。

**未发现**：对 `D:\sdkmax`、SDKMAX 登录/OAuth/`/api/nova/device-token` 相关代码的任何触碰或开工痕迹（已用 `grep` 核实，见第 8 节）。**未发现**混入与 F1-F5 无关的大规模新功能。

## 5. F1–F5 逐项复审结论

### F1（SSE 数据完整性）— **已正确修复，判定：通过**

核心修复：`apps/desktop/src-tauri/src/api.rs`

1. `ApiErrorDetail` 新增字段 `received_delta: bool`，在 `stream_chat_once` 内部统一由本地变量 `saw_delta` 驱动，覆盖了该函数**所有**错误返回路径（POST 发送失败、字节读取失败、SSE 解析失败、`sse_incomplete`、`user_canceled`）。
2. 重试判定条件从原来"错误码是否等于 `stream_started`"的间接推断，改为直接判定 `!detail.received_delta`（`api.rs:141`）——**修复了根因**，不再依赖单一错误码推断"本次尝试是否已经输出过 Delta"。
3. `SseParser::push` 返回类型改为携带 `events: Vec<StreamEvent>` 的 `SseParseError`：即使本次 `push()` 调用最终因畸形 JSON 失败，之前在同一批数据里已经成功解析出的 Delta 事件仍会被取出并通过 `on_chunk` 上抛（`api.rs:209-229`），不会因为同一 chunk 里后半段畸形而丢失前半段已经到手的内容。
4. `commands.rs` 新增 `stream_error_status`/`stream_error_message`：只要 `received_delta == true`（不论具体错误码是 `sse_incomplete`、`sse_parse_error` 还是普通网络读取错误），一律映射为 `"interrupted"`，且展示统一的"回答中断，可重新生成"提示，不再逐错误码硬编码。`Ok(request_id)`（映射为 `"complete"`）**只有**在 `saw_done == true` 时才可达，验收清单第 6 条成立。

**逐条验证结果**（含真实 TCP mock server 的异步集成测试，非纯函数级 mock）：

| 验收标准 | 结果 | 证据 |
|---|---|---|
| 1. 正常流：Delta→多 Delta→`[DONE]`→complete | 通过 | `api.rs::streams_normal_delta_and_done`；1 次请求，内容正确 |
| 2. 无 Delta 时的一次性可重试网络错误：最多重试 1 次、总请求 ≤2、无重复消息 | 通过 | `api.rs::retries_once_when_temporary_failure_has_no_delta`：mock 首次连接立即关闭（0 字节），第二次返回完整内容；断言 `requests==2`，`chunks==["retry-ok"]`（无重复） |
| 3. 已收到 ≥1 Delta 后发生网络错误：禁止重试、保留部分内容、状态 interrupted、不追加拼接 | **通过（代码可证，见下方说明）** | 逻辑上与验收标准 4 共享同一 `saw_delta`/`received_delta` 机制，见下 |
| 4. 已收到 Delta，流直接结束且无 `[DONE]`：识别为 `sse_incomplete`、不重试、不 complete、保留内容、标记 interrupted | 通过 | `api.rs::does_not_retry_when_stream_breaks_after_delta`：`error.code=="sse_incomplete"`，`error.received_delta==true`，`chunks==["partial"]`，`requests==1` |
| 5. 已收到 Delta 后出现畸形 SSE：不重试、不丢失/不重复、状态 interrupted | 通过 | `api.rs::malformed_sse_after_delta_does_not_retry`：`error.code=="sse_parse_error"`，`received_delta==true`，`chunks==["partial"]`（未丢失也未重复），`requests==1` |
| 6. 只有真实收到 `[DONE]` 才能 complete | 通过 | 代码结构上 `Ok(request_id)` 只有 `if !saw_done { return Err(..) }` 之后才可达，无其他返回 `Ok` 的路径 |
| DB 落盘层面：interrupted 消息只保留单份未拼接内容 | 通过 | `db.rs::interrupted_partial_stream_preserves_single_partial_content` |
| 前端展示层面：与 DB 状态一致 | 通过（异步收敛后一致） | 见下方说明 |

**关于验收标准 3 的说明**：`item.map_err(...)`（字节流读取时的真正 I/O 级错误，`api.rs:201-207`）与 `sse_incomplete`（干净的流结束但无 `[DONE]`，`api.rs:250-257`）在代码结构上共享同一个 `saw_delta` 变量并用同样方式设置 `received_delta`，因此两者在"是否允许重试 / 是否标记 interrupted"上是**同一套逻辑**，不是分别硬编码。测试套件中显式覆盖了"干净流结束无 `[DONE]`"（标准 4）和"同一 chunk 内先 Delta 后畸形 JSON"（标准 5），但没有单独构造一个"流已经在读取字节阶段中途真正 I/O 出错"（例如连接被强制 RST）的场景。这属于**测试覆盖的轻微遗漏（Low）**，不是代码缺陷——通过直接阅读代码可以确认该分支同样正确设置了 `received_delta: saw_delta`，行为与已测试分支完全对称。建议后续为该分支补一个专项测试（例如让 mock server 写入部分响应后直接 `TcpStream` 强制关闭连接触发真实 I/O 错误），但不构成阻断项。

**前端一致性说明**：`chatEvents.ts::applyStreamError` 在收到 `chat://error` 事件时会先把本地消息**乐观地**标记为 `interrupted`（不区分具体错误码），随后 `finishSending()` 会 `await this.loadMessages(...)` 从 SQLite 重新拉取权威状态并整体替换 `store.messages` 数组。也就是说，即使某次错误在后端被正确判定为 `"error"`（`received_delta==false` 的真错误，例如从未连接上服务器），前端会有一个极短暂（毫秒级、一次异步往返内）的"乐观 interrupted"闪烁，随后被数据库读回的真实状态覆盖为准确值。**最终稳定状态**（用户实际会看到并持久化的状态）与 SQLite 完全一致，这是可接受的实现方式，不构成数据不一致问题。

未产生重复消息、未产生无限重试（`attempts == 1` 硬性限制总请求数 ≤2）——均已确认。

### F2（抢占式取消）— **已正确修复，判定：通过**

核心修复：`apps/desktop/src-tauri/src/api.rs:184-200`

```rust
let item = tokio::select! {
    biased;
    _ = cancel.cancelled() => { return Err(ApiErrorDetail { code: Some("user_canceled"), .. }); }
    item = stream.next() => item,
};
```

这不是"函数里出现了 `tokio::select!` 字样"式的表面检查，而是完整验证了控制流：

1. **真正抢占**：`cancel.cancelled()` 与 `stream.next()` 被 `select!` 并发轮询，取消不需要等待下一字节到达。已用真实测试证明：`api.rs::blocking_stream_can_be_cancelled_immediately_and_does_not_retry` 让 mock server 发送 HTTP 头后 `sleep(5s)` 完全不发数据，测试等待仅 50ms 后调用 `cancel()`，随后要求整个任务在 **500ms 超时窗口内**完成——测试通过，证明取消确实不需要等 5 秒的数据停顿，也远不需要等 45 秒的 `reqwest::Client` 超时（`lib.rs:29`）。
2. **不重试**：取消分支返回的 `ApiErrorDetail.status = None`，但 `stream_chat` 的重试判定显式包含 `!cancel.is_cancelled()`，因此即使 `status.is_none()` 满足其他重试条件也不会重试；测试断言 `server.requests.load() == 1`（只有一次请求）。
3. **保留部分内容 / 状态 interrupted / 错误码稳定**：`code: Some("user_canceled")` 字符串字面量未变；`commands.rs::stream_error_status` 对 `user_canceled` 一律返回 `"interrupted"`；对应单测 `commands::tests::user_cancel_maps_to_interrupted_without_retry_hint` 通过。
4. **清理**：`commands.rs:230-232`，无论 `stream_result` 是 `Ok` 还是 `Err`，`active_cancel` 锁内容都会被设回 `None`（此逻辑本身未被此次改动触碰，属于既有正确实现，本轮确认未被破坏）。未发现锁跨 `await` 持有的情况（`std::sync::Mutex` 的所有 `.lock()` 均在单条语句或无 `await` 的 `if let` 块内使用并立即释放）。

**唯一的覆盖建议**：目前的取消测试场景是"连接已建立但从未收到任何 Delta 时取消"。没有单独构造"已经收到过若干 Delta 后再取消"的场景断言"已收到的内容会被保留"。由于该部分逻辑（`on_chunk` 回调持久化）在本次改动中完全没有变化，属于既有正确行为的自然延伸，风险极低，建议补充但不阻断。

### F3（生产 CSP）— **判定：通过**

`apps/desktop/src-tauri/tauri.conf.json`：

```json
"csp": "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self' ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
"devCsp": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self' ipc: http://ipc.localhost ws://127.0.0.1:1420; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
```

逐条核对：

1. `security.csp` 不再为 `null`。✓
2. `default-src 'self'`。✓
3. `script-src` 无 `unsafe-eval`。✓（生产/开发均无）
4. 无通配 `*`。✓（全字段核对，无裸 `*`）
5. 无无条件 `https:`。✓（生产 `connect-src` 仅 `'self' ipc: http://ipc.localhost`，`devCsp` 额外只加了 `ws://127.0.0.1:1420` 这一具体地址，均非通配）
6. `connect-src` 未开放任意外部网络，只放行了 Tauri IPC 自身通道（`ipc: http://ipc.localhost`，经核实是 Tauri v2 官方 IPC 传输机制在 Windows 上的真实必需地址，见 `node_modules/@tauri-apps/api/core.js:207` 官方注释示例，非臆造放宽）。
7. `connect-src` **未包含** `https://api.sdkmax.com`；已核实前端源码（`.vue`/`.ts` 全量 diff 与全仓）没有任何 `fetch`/`XMLHttpRequest`/`axios` 调用，网络请求确实全部由 Rust `reqwest` 发起。✓
8. **唯一的 `unsafe-inline`** 出现在 `devCsp` 的 `style-src`（不在生产 `csp` 中）。已核实原因：`npm run tauri dev` 使用外部 Vite Dev Server，Vite 开发模式下通过 JS 动态注入 `<style>` 标签实现热更新，这是 Vite 开发态的固有机制；生产构建（`npm run build`/`tauri build`）产出的 `dist/index.html` 只有 `<link rel="stylesheet">` 外链样式表和 `<script type="module">` 外链脚本，**没有任何内联脚本或样式**（已直接读取生成的 `dist/index.html` 核实）。因此该 `unsafe-inline` 风险**仅存在于开发者本机 `npm run tauri dev` 场景**，不会进入最终交付给用户的生产可执行文件，风险评估：可接受。
9. CSP 未破坏生产构建：`npm run tauri -- build` 在本轮复审中重新执行，退出码 0，产出 `target\release\nova.exe`；已实际启动该 exe 并截图确认界面完整渲染（侧边栏、按钮、会话列表样式均正常显示，非空白/无样式页面）。
10. 用户聊天消息渲染路径：已对整个 `apps/desktop/src` 做 `grep` 全文搜索，**零处**使用 `v-html` 或 `innerHTML`；`App.vue` 中消息内容渲染方式为 `<p>{{ message.content }}</p>`（Vue 模板插值，默认自动转义 HTML 特殊字符）。这意味着即使消息内容包含 `<script>alert(1)</script>`，也只会被当作纯文本显示为字面量字符串，不会被解析为 DOM 元素或执行。**本项通过静态代码审查确认，未通过发送真实消息到 SDKMAX 的方式验证**，原因：触发完整聊天流程需要一个已保存的 Provider API Key，而 `send_chat_message` 会在创建消息之前就尝试用该 Key 连接 `https://api.sdkmax.com`，为避免向生产 SDKMAX 发起任何网络请求（即使只是验证 XSS 而非消耗计费模型），本轮选择只做静态代码证明，不做端到端真实请求。CSP 的 `script-src 'self'`（无 `unsafe-inline`/`unsafe-eval`）在此基础上提供纵深防御：即便未来代码回归引入了 `v-html` 之类的注入点，内联脚本仍会被 CSP 拦截而无法执行。

**未发现"表面设置但实质等同全放开"的情况**：所有指令都明确限定在 `'self'`、具体的 Tauri IPC 地址或 `'none'`，没有任何字段退化为事实上的通配放开。

### F4（构建可追溯性）— **判定：通过**

- `test-results\build-info.txt` 记录的 `commit: 465d3c8b29ef6b62b66e6de70a2ab91ede416ad8` **与本次复审的 `git rev-parse HEAD` 完全一致**。
- 该文件的 `Generated (UTC): 2026-08-30T14:51:50Z`，与 `465d3c8` 提交时间 `2026-08-30 22:51:34 +0800`（= `14:51:34Z`）仅相差 16 秒——证明验证脚本确实是在该 commit 刚提交后立即针对该 commit 运行的，不是复用旧记录。
- `build-info.txt` 记录的 `git status --porcelain` 状态仅有 `?? audit/CLAUDE_REVIEW_NOVA_TECHNICAL_MVP.md`（与本轮复审开始时观察到的状态完全一致）——工作区是 clean 的（相对源码而言）。
- 记录内容齐全：UTC 时间、完整 commit hash、分支、`git status --porcelain`、`node`/`npm`/`rustc`/`cargo` 版本、四条测试/构建命令、各自退出码、Tauri 构建产物路径。
- 已阅读 `scripts/verify-build.ps1` 全文：脚本用 `$LASTEXITCode` 判定每一步骤 `pass`/`fail`，用 `@(...)` 强制数组上下文计算 `$FailedSteps.Count`/`$MissingSteps.Count`（这是一个已知的 PowerShell 5.1 陷阱——单元素匹配结果若不强制转数组，`.Count` 会是 `$null` 导致误判为通过；该脚本已正确处理），任一步骤失败或工具缺失都会让 `Overall` 变为 `FAIL` 并 `exit 1`；本轮复审额外验证了这一点（构造了一个临时的、刻意失败的脚本副本运行后已删除，非提交产物，用于确认失败时确实返回非零退出码且不会误报 PASS——该临时文件不属于本报告新增文件，已在验证后清理）。
- 敏感信息核查：`build-info.txt` 与 `test-results/verify-build-*.log` 全部内容仅为编译器/测试运行器标准输出，未见 API Key、Authorization、Cookie、密码或聊天正文；脚本本身内置了 `Protect-Secrets` 正则脱敏（`Authorization: Bearer`、`Cookie:`、`sk-` 前缀、`*_KEY/SECRET/PASSWORD/TOKEN=` 形式）作为纵深防御。
- `.gitignore` 新增的仅有一行 `test-results/`，是**新增排除规则**，未删除或削弱任何既有的 `*.log`/`*.sqlite*`/`target/`/`.env*` 忽略规则（已核对 `.gitignore` 全文，原有规则逐条仍在）。
- **当前生产构建确实基于被审核 commit**：本轮复审重新执行了 `npm run tauri -- build`（在未做任何源码修改的前提下），产出的 `nova.exe` 时间戳为复审当次运行时刻，源码状态与 `465d3c8` 完全一致（`git diff` 为空），因此该次构建产物与被审核 commit 严格对应。

### F5（技术债登记）— **文档本身通过；发现一个未被登记覆盖的窄口径并发入口，需要跟进**

`docs/NOVA_TECHNICAL_DEBT.md` 内容核对：

- ✓ 明确写明 `active_cancel` 是 `Arc<Mutex<Option<CancellationToken>>>` 单一全局槽位。
- ✓ 明确写明"MVP 前端只有一个聊天面，`sending=true` 时按钮替换为停止，因此当前不可能触发第二次 `send_chat_message`"这一前提假设，并作为"暂时可接受"的依据。
- ✓ 明确列出触发改造的条件：多会话并发流式、多窗口/多 WebView 共享 `AppState`、后台/自动生成与用户发送重叠。
- ✓ 明确推荐用 `HashMap<String, CancellationToken>`（按 `assistant_message_id` 或 `conversation_id` 建索引）替代单槽位。
- ✓ 明确要求"完成/取消/失败三种终态都必须清理对应 Token"，并具体点出必须在 `commands.rs` 现有清理单槽位的同一个 spawn 任务内完成，避免残留导致误取消或内存泄漏。
- ✓ 给出了 5 条明确的验收标准（多会话独立停止、互不影响、Map 大小归零、对已消失 key 的安全 no-op、既有单流程测试保持通过）。

**但本轮复审没有止步于"读文档是否写得好"，而是按指令要求独立核实了文档的前提假设**（"当前 UI 确实无法在一个请求未结束时再次触发 `send_chat_message`"）。核实过程：

审阅 `apps/desktop/src/stores/nova.ts::sendMessage`：

```ts
async sendMessage(content: string) {
  if (!this.selectedModelId) { throw new Error('请先选择模型') }
  if (!this.activeConversationId) {
    await this.createConversation()   // <- 这里有一次真实的异步 IPC 往返
  }
  this.sending = true                 // <- 但 sending 标志要等到这一行才置位
  ...
  const result = await invoke<{ assistant_message_id: string }>('send_chat_message', { ... })
  ...
}
```

`send()`（`App.vue`）在调用 `sendMessage` 前会同步执行 `prompt.value = ''`，这确保了"同一段文本"不可能被提交两次（第二次读到的 `prompt.value` 已经是空字符串，会被 `if (!text) return` 拦下）。**但如果用户在 `activeConversationId` 为空（全新会话、尚未发送过第一条消息）的状态下，先后提交两条不同内容的消息**（例如很快地连续两次按 `Ctrl+Enter` 并且中途重新输入了文字，或者两个不同触发源——点击按钮与快捷键——在极短时间内先后触发），第二次调用会在第一次的 `await this.createConversation()` 尚未返回、`this.sending` 尚未被置为 `true` 的窗口期内进入，同样判断 `!this.activeConversationId` 为真并再创建一个新会话，随后两次调用都会各自把 `this.sending` 置为 `true` 并各自调用一次 `send_chat_message`（分别对应两个不同的新会话）。

**实际影响**（已结合 Rust 侧代码复核，而非停留在前端假设）：

- 两次调用会各自 `db.create_message(...)` 到**各自独立的会话**里，不会出现同一条消息被两次拼接写入的数据损坏（`db.rs::create_message` 每次都插入新行，不存在 F1 那种"追加到同一条消息"的风险）。
- 但两次调用共享同一个 `AppState.active_cancel` 单槽位（`commands.rs:188-192`），后发起的那次会覆盖前一次的取消令牌。此时前端"停止"按钮（`store.stopGeneration()` → `cancel_active_chat`）只能取消**后发起**的那一路流，先发起的那一路会在没有用户可操作入口的情况下独立跑到自然结束（成功、出错或超时），这正是 `NOVA_TECHNICAL_DEBT.md` 里描述的"stop 只会停最新的流"风险，只是触发路径是一个此前未被文档记录的具体入口。

**严重级别评估**：Low–Medium。触发条件较窄（必须是"新会话的第一条消息"这一特定状态，且需要用户在很短的一次 IPC 往返时间窗口内二次提交**不同**文本——同一段文本会被前面提到的 `prompt.value=''` 同步清空机制自然拦下），后果有界（不会损坏已有消息内容、不涉及密钥/越权），且修复代价极低。

**建议修复**（供 Codex 后续处理，本轮未做任何代码修改）：在 `sendMessage` 顶部增加显式重入守卫，例如 `if (this.sending) return`，并将 `this.sending = true` 提到 `await this.createConversation()` 之前执行，使"是否已有一路在途"这件事不依赖"『发送』按钮多快切换成『停止』"这个时序细节。

**结论**：F5 的文档撰写质量本身没有问题，予以认可；但其所依赖的前提（"当前 UI 确实禁止并发"）经复核**不是在所有状态下都成立**，属于遗留在 MVP 原有代码里、未被本轮任何一次提交触碰过的既有缺口（`stores/nova.ts` 在 `e0de353`、`465d3c8` 两次提交中均未被修改）。按复审指令要求，这一发现被计入"剩余问题"并影响整体判定为 CONDITIONAL PASS 而非无条件 PASS。

## 6. 实际运行的测试及结果（本轮独立执行，未复用旧日志作为唯一依据）

| 命令 | 目录 | 退出码 | 结果摘要 |
|---|---|---|---|
| `cargo fmt --check` | `apps/desktop/src-tauri` | 0 | 无需重新格式化 |
| `cargo clippy --all-targets --all-features -- -D warnings` | `apps/desktop/src-tauri` | 0 | 无警告（`-D warnings` 下任何警告都会变成非零退出） |
| `cargo test` | `apps/desktop/src-tauri` | 0 | **21 passed; 0 failed; 0 ignored**（含 5 个新增 SSE mock-server 集成测试与 2 个新增消息状态映射单测；`0 ignored` 确认没有测试被跳过或仅编译未执行） |
| `npm test -- --run` | `apps/desktop` | 0 | **4 passed**（`security.test.ts` 3 个 + 新增 `chatEvents.test.ts` 1 个），0 skipped |
| `npm run build` | `apps/desktop` | 0 | `vue-tsc --noEmit && vite build` 成功，产出 `dist/` |
| `npm run tauri -- build` | `apps/desktop` | 0 | 生产构建成功，产出 `apps/desktop/src-tauri/target/release/nova.exe`（本轮复审已实际启动该 exe 并确认窗口正常渲染、进程 `Responding=True`，随后正常关闭，未长时间占用进程） |

F1/F2 专项测试确认**不是**被 `#[ignore]`/`.skip`/仅编译未执行：`cargo test` 输出逐条列出了 `api::tests::streams_normal_delta_and_done`、`retries_once_when_temporary_failure_has_no_delta`、`does_not_retry_when_stream_breaks_after_delta`、`malformed_sse_after_delta_does_not_retry`、`blocking_stream_can_be_cancelled_immediately_and_does_not_retry` 五个测试，均标记 `... ok`，且 `0 ignored`。

以上结果与 Codex 自行留存的 `test-results/cargo-test-f1-f2.log`、`npm-test-f1-f2.log`、`cargo-clippy-f1-f2.log`、`tauri-build-f1-f2.log` 交叉核对一致（数字、测试名称完全吻合），属于独立复现，非单方面转述。

## 7. 构建产物与 Commit 一致性

- `test-results/build-info.txt` 中记录的 `commit` 字段（`465d3c8b29ef6b62b66e6de70a2ab91ede416ad8`）与本轮复审 `git rev-parse HEAD` 的结果**完全一致**。
- 本轮复审在不修改任何源码的前提下重新运行了 `npm run tauri -- build`，成功产出 `apps/desktop/src-tauri/target/release/nova.exe`；由于运行期间 `git diff` 持续为空（未做任何源码改动），该次构建产物同样对应 `465d3c8`。
- 未发现"build-info 记录的 commit 落后于当前 HEAD"的情况，因此**无需**（也未）要求重新执行验证脚本；本轮复审也未自行重新运行 `scripts/verify-build.ps1`（避免产生新的、非报告类的文件写入），仅通过独立重跑四条底层命令达到同等验证效果。

## 8. 安全回归结果

| 检查项 | 结果 |
|---|---|
| API Key 只存 Windows Credential Manager | 通过；`credential.rs` 存取逻辑未变（仅 2 行等价重写），`commands.rs::save_provider`/`api_provider` 路径未变 |
| SQLite 只存 `masked_key` | 通过；`db.rs` 表结构与 `save_provider_metadata` 逻辑未变 |
| localStorage/sessionStorage 不存 Key | 通过；全仓 `grep` 未发现任何 `localStorage`/`sessionStorage` 使用 |
| Vue 持久化状态不存 Key | 通过；无 Pinia 持久化插件，`package.json` 依赖未变 |
| Authorization/Cookie/Key 不进日志 | 通过；`security.rs::redact_sensitive_text` 未变，`commands.rs` 新的 `stream_error_message` 对 `received_delta` 场景改用固定文案而非泄露原始错误文本，脱敏程度不降反升 |
| 新增 mock 测试无硬编码真实 Key | 通过；`api.rs` 新测试使用字面量 `"test-key"`，明显是占位符而非真实凭据格式 |
| 未新增任意 URL/路径请求能力 | 通过；`validation.rs::AllowedEndpoint` 三个路径未变，`endpoint_url` 仍强制基于枚举而非用户输入拼接 |
| 端点 allowlist 仍只有三个 | 通过（`/v1/models`、`/v1/chat/completions`、`/api/usage/token/`） |
| 未调用管理员/渠道/倍率等禁止接口 | 通过（源码内无任何相关路径字符串） |
| 未修改 SDKMAX 用户/Token/计费/倍率/渠道/分组/余额核心 | 通过；`D:\sdkmax` 未被触碰（见第 4、9 节） |
| 无新引入的 unwrap/panic/锁跨 await 风险 | 通过；本次 diff 中所有新增 `.unwrap()` 均位于 `#[cfg(test)] mod tests` 块内，生产代码路径零新增 `.unwrap()`/`.expect()`/`panic!`；所有 `Mutex::lock()` 用法均未跨越 `.await` |
| 错误详情向前端传递前经过脱敏 | 通过，且本次改动进一步收紧（interrupted 场景一律展示统一提示，不透出底层错误原文） |

敏感信息扫描说明：本轮对完整 2-commit diff 做了 `sk-`/`Authorization: Bearer`/`password=`/私钥头 等模式扫描，命中的全部是既有测试脚手架里的占位符（如 `"test-key"`）或文档中的 `***REDACTED***` 示例，**未发现任何真实凭据特征**；未复制任何疑似真实凭据的完整值到本报告。

## 9. D:\sdkmax 是否被修改

**否。** 已核实 `D:\sdkmax` 目录下不存在任何文件的修改时间晚于既有审计基线（`SDKMAX_NOVA_RISK_LIST.md` 生成时间点）；`git diff HEAD~2..HEAD` 的文件列表中不包含任何 `D:\sdkmax` 路径；全仓 `grep` 未发现任何指向 SDKMAX 登录、OAuth、`/api/nova/device-token` 的实现代码。

## 10. 文档一致性

| 文档 | 结果 |
|---|---|
| `NOVA_PRD_MVP.md` | 一致，未改动，"Stop an active stream and keep partial assistant output marked as interrupted" 与当前实现完全吻合 |
| `NOVA_ARCHITECTURE.md` | 一致，未改动，端点边界描述与 `validation.rs` 实现吻合 |
| `NOVA_API_SPEC.md` | 一致，未改动，SSE 解析描述仍准确（未细化到重试策略，属于合理的抽象层级，不构成不一致） |
| `NOVA_SECURITY_SPEC.md` | 一致；新增 "WebView Content Security Policy" 小节准确描述了 `csp`/`devCsp` 的差异与生产/开发边界，与 `tauri.conf.json` 实际内容核对一致 |
| `NOVA_TEST_PLAN.md` | **基本一致，但有轻微滞后**：新增的 "CSP Verification (Manual)" 与 "Build/Commit Verification" 小节准确；但 "Implemented Automated Coverage / Rust" 列表未更新以列出 F1/F2 新增的 5 个 SSE 重试/取消回归测试（现有条目本身没有变得不准确，只是不完整，未点名这些新的关键回归测试）。建议后续补充，Low 级别文档完整性问题 |
| `NOVA_TECHNICAL_MVP_RUNBOOK.md` | 一致，未改动；"Stream interruption: partial content is preserved locally" 描述仍准确，新的错误文案变化（"回答中断，可重新生成" 替代 "流式响应中断，已保留收到的内容"）不影响该文档结论 |
| `NOVA_TECHNICAL_DEBT.md` | 内容准确完整（见第 5 节 F5），但其"当前 UI 确实禁止并发"的前提存在第 5 节所述窄口径例外，建议在下次更新该文档或修复 `sendMessage` 时一并说明 |
| `NOVA_DEVELOPMENT_WORKFLOW.md` | 一致；新增的 "Build Verification and Commit Traceability" 小节描述的"先提交再验证"工作流与实测的 `build-info.txt` 时间线（提交后 16 秒即生成）完全吻合 |

未发现把"规划中功能"写成"已完成"的情况；文档仍如实反映"当前阶段仍是手工填写 SDKMAX API Key"、"尚未实现 SDKMAX 登录和设备 Token"（`NOVA_PRD_MVP.md` Non-Goals、`NOVA_SECURITY_SPEC.md` Current Limitations 均未被改动，且第 8 节已核实代码现状与之相符）。

## 11. 未完成的人工验收

以下人工验收项在本仓库中**没有找到执行记录**（无论是测试日志、截图归档还是文档中的"已完成"标注）：

1. 针对真实 `api.sdkmax.com` 的正常聊天流式完成。
2. 针对真实网络环境点击停止后立即停止的人工体感确认。
3. 停止后部分内容保留的人工确认（真实场景，非 mock）。
4. 消息状态显示 interrupted 的人工确认（真实场景）。
5. 重启 Nova 后中断消息仍正确显示的人工确认。
6. CSP 未破坏界面——**本项已在本轮复审中完成**（生产构建 exe 实际启动并截图确认渲染正常，见第 5 节 F3）。
7. Provider 保存、模型刷新和会话恢复的真实 SDKMAX 联调确认。

**说明**：本轮复审严格遵守"不得调用生产收费模型完成测试、不得自行获取凭据"的限制，因此第 1/2/3/4/5/7 项**未被执行**，也不应由本轮自行执行。第 6 项因不涉及计费调用，已由本轮完成并确认通过。

**结论（按用户给定规则）**：代码审查与自动化测试（cargo/npm 全套 + clippy + fmt + 生产构建）**已通过**；真实 SDKMAX 人工联调**仍是合并 develop 前必须完成的外部人工门禁**，不应被本报告的结论替代或省略。

## 12. 是否允许合并 develop

**暂不允许直接合并。** 需要先完成：

1. 第 11 节列出的真实 SDKMAX 人工联调（外部人工门禁，本报告无法替代）。
2. 第 5 节 F5 发现的 `sendMessage` 窄口径并发入口——建议至少完成一行级别的重入守卫修复（`if (this.sending) return`），使 `NOVA_TECHNICAL_DEBT.md` 中"当前 UI 确实禁止并发"的前提在所有状态下都成立，而不仅仅是"通常情况下"成立。

以上两项完成并复核后，方可合并 `feature/nova-technical-mvp` → `develop`。**本轮复审未执行、也不会执行任何合并操作。**

## 13. 是否允许进入下一阶段

技术层面（代码质量、自动化测试、生产构建、CSP、构建可追溯性、密钥/日志安全边界、SDKMAX 只读接入范围）**已具备进入下一阶段规划的条件**，但"下一阶段"应严格限定为：

- 允许：开始"Nova SDKMAX 授权接口设计"——即产出设计文档、威胁模型、API 规范草案（如 `docs/` 下新增设计稿），供用户审阅确认。
- **不允许**：立即编写或部署 `/api/nova/device-token` 或任何 SDKMAX 服务端代码改动。`SDKMAX_NOVA_RISK_LIST.md`/`SDKMAX_NOVA_INTEGRATION_AUDIT.md` 中列出的"禁止改动清单"（`model/user.go`、`model/token.go`、`middleware/auth.go`、`middleware/distributor.go`、`relay/*`、`service/channel_select.go`、计费/倍率相关文件等）在本报告给出的任何结论下都**不构成豁免**，仍需专项设计、回归测试和用户明确批准后才能开始改动。

即使本报告最终给出 PASS/CONDITIONAL PASS，也**不自动授权**任何 SDKMAX 代码开发；这一点由用户在任务说明中明确要求，本报告在此重申。

## 14. 剩余问题及严重级别

| 编号 | 严重级别 | 描述 | 位置 | 建议 |
|---|---|---|---|---|
| R1 | **Low–Medium** | `sendMessage()` 在 `activeConversationId` 为空时，`await this.createConversation()` 与 `this.sending = true` 之间存在异步窗口，窄口径下可被触发两次并发 `send_chat_message`（各自独立会话，不损坏单条消息，但会共享同一个 `active_cancel` 槽位，导致"停止"只能停最新一路） | `apps/desktop/src/stores/nova.ts::sendMessage` | 增加 `if (this.sending) return` 重入守卫，并将 `this.sending = true` 提前到 `await this.createConversation()` 之前 |
| R2 | Low | `NOVA_TEST_PLAN.md` 的 "Implemented Automated Coverage / Rust" 列表未列出 F1/F2 新增的 5 个 SSE 重试/取消回归测试名称 | `docs/NOVA_TEST_PLAN.md` | 补充列出 5 个新测试，便于后续维护者知晓其存在与用途，避免被无意间删除或弱化 |
| R3 | Low | F1 修复中，"流字节读取阶段发生真正 I/O 级错误（而非干净结束）且此前已收到 Delta"这一分支缺少专项 mock 测试，仅能通过代码共享变量结构证明其正确性 | `apps/desktop/src-tauri/src/api.rs`（`item.map_err` 分支） | 补充一个强制 TCP 连接中途异常关闭（而非干净 EOF）的测试用例 |
| R4 | Low | F2 修复中，"已收到部分 Delta 后再取消"的场景没有专项测试断言"已收到内容被保留" | `apps/desktop/src-tauri/src/api.rs` | 补充一个"先收到 1-2 个 Delta 再取消"的测试，断言 `chunks` 非空且等于已发送的部分 |
| R5 | Low | `465d3c8` 提交中混入了 2 行与 F1/F2 无关的 `credential.rs` 纯风格重写（`is_multiple_of`/`chunks`），虽经核实功能等价、无风险，但存在提交范围越界的实践问题 | `apps/desktop/src-tauri/src/credential.rs` | 后续提交尽量保持单一职责范围，或在提交信息中说明附带的风格改动来源（如 clippy 自动建议） |
| R6 | 非代码（外部门禁） | 真实 SDKMAX 人工联调尚未执行/记录 | 见第 11 节 | 合并 develop 前必须由人工完成并留存证据（截图或日志） |

以上均不构成 REJECT 级别问题：无密钥泄露、无越权接口调用、无 SDKMAX 核心修改、无构建/关键测试失败、F1/F2 的核心数据完整性与取消抢占问题均已被有效证据证明修复。

## 15. 本轮未修改任何代码的声明

本轮审核期间：

- 未使用 Edit/Write 工具修改任何 `.rs`/`.ts`/`.vue`/`.json`/`.md`/`.ps1` 源码、配置或文档文件。
- 未执行 `git add`、`git commit`、`git merge`、`git rebase`、`git reset`、`git checkout`、`git switch` 或任何写入 `.git` 状态的命令；全程只使用 `git status`/`git branch`/`git log`/`git diff`/`git show`/`git ls-files` 等只读命令。
- 按任务要求实际运行了 `cargo fmt --check`、`cargo clippy`、`cargo test`、`npm test`、`npm run build`、`npm run tauri -- build`，这些命令产生的构建/测试产物（`target/`、`dist/`、`nova.exe`、`node_modules` 缓存等）均落在 `.gitignore` 已排除的目录内，不属于源码/配置/文档变更。
- 为了肉眼确认 CSP 是否破坏渲染，本轮启动了一次生产 `nova.exe` 并截图核验后已正常关闭进程；截图文件事后已删除，未在 `test-results/` 或任何位置留下本轮新增的非报告类文件。
- 唯一新增/修改的文件是本报告：`D:\nova\audit\CLAUDE_REVIEW_NOVA_TECHNICAL_MVP_FINAL.md`。

---

## 针对 PASS 场景的补充回答（即使最终结论为 CONDITIONAL PASS，仍按用户要求的框架逐一说明）

- **Technical MVP 是否通过？** 技术层面（F1-F4 + 自动化测试 + 生产构建 + 安全边界）已通过；因 F5 关联的 R1 窄口径并发入口尚未修复，整体给 **CONDITIONAL PASS** 而非无条件 PASS。
- **是否允许合并到 develop？** 暂不允许，需先完成 R1 修复与第 11 节的真实 SDKMAX 人工联调（见第 12 节）。
- **合并前是否还有人工门禁？** 有，见第 11、12 节：真实 SDKMAX 人工联调是独立于本代码审核之外的强制人工门禁。
- **是否允许开始"Nova SDKMAX 授权接口设计"？** 允许（设计/文档/威胁模型阶段，不涉及 SDKMAX 代码改动）。
- **是否允许立即开发 `/api/nova/device-token`？** 不允许。需先完成授权接口设计、威胁模型和 API 规范，并由用户明确确认后，才能进入 SDKMAX 代码开发阶段；本报告的任何结论都不构成对此的预先授权。
