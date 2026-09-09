# Claude Code 只读审核报告 — Nova Technical MVP 阶段

- 审核日期：2026-08-30
- 审核角色：只读代码审核（Codex 负责开发/修复，本报告不修改任何代码）
- 审核范围：
  - `D:\nova\docs\*`
  - `D:\nova\audit\*`（含既有的 SDKMAX 接入审计与风险清单）
  - `D:\nova\apps\desktop`（Vue 前端 + Rust/Tauri 后端全部源码）
  - `D:\nova\services`（当前为空目录，符合 MVP 阶段"无独立后端服务"范围）
  - Git 历史：分支 `feature/nova-technical-mvp`，提交 `e0f1a7b`（初始化审计基线）→ `3e1d2dd`（feat: add nova technical mvp），当前工作区 clean，无未提交改动
  - 构建/测试产物：`D:\nova\test-results\*.log`，`apps\desktop\src-tauri\target\release\nova.exe`

## 结论

**CONDITIONAL PASS**

代码整体方向、数据边界和权限设计与 `NOVA_PRD_MVP.md`、`NOVA_SECURITY_SPEC.md`、`NOVA_ARCHITECTURE.md` 一致，未发现 API Key 明文存储、Authorization/Cookie 日志泄露、越权 SDKMAX 接口调用、SDKMAX 核心计费/渠道代码改动等高危合规问题。但发现 1 项 High 级别的数据完整性缺陷（SSE 流不完整时的静默重试会污染本地会话历史，且未按要求标记为 interrupted），以及 2 项 Medium 级别问题（取消请求非抢占式、生产 CSP 未设置）。建议在进入下一阶段前修复 F1，并评估 F2/F3。

---

## 一、PRD/安全规范符合性核对

| 检查项 | 结果 | 说明 |
|---|---|---|
| 仅支持手动 API Key 配置，无 SDKMAX 登录/Token 自动创建 | 符合 | `commands.rs`/`stores/nova.ts` 只有 `save_provider` 接受 Key，未实现任何登录、OAuth、设备 Token 创建流程 |
| API Key 是否明文落盘 | 未发现明文存储 | `db.rs` 的 `providers` 表只写入 `mask_secret()` 处理后的 `masked_key`；原始 Key 只通过 `SystemCredentialStore` 写入 Windows Credential Manager (`credential.rs:65-93`) |
| 日志是否泄露 Authorization/Cookie | 未发现 | `security.rs::redact_sensitive_text` 对 `Authorization: Bearer`、`Cookie:`、`sk-` 前缀做清洗；`db.rs::log_request` 只写入 `status_code/error_code/message(已脱敏)/request_id(已脱敏)`，从不写入请求体或消息内容 |
| Tauri 权限是否最小化 | 基本符合，1 项待加固 | `capabilities/default.json` 仅声明 `core:default`，未引入 `fs`/`shell`/`http` 等插件权限；但 `tauri.conf.json` 的 `security.csp` 被显式设为 `null`（见 F3） |
| 前端是否绕过 Rust 直接读取密钥 | 未发现绕过 | 前端只在 `App.vue::saveProvider` 提交一次性明文 Key 给 `save_provider`，随后立刻 `apiKey.value=''`；后续调用一律只传 `provider_id`，真实 Key 只在 Rust 侧 `api_provider()` 从凭据库读取并用 `bearer_auth` 注入 |
| 端点allowlist（禁止 `/api/channel/*` 等管理面） | 符合 | `validation.rs::AllowedEndpoint` 只枚举 `/v1/models`、`/v1/chat/completions`、`/api/usage/token/` 三个路径，且 `endpoint_url` 始终基于枚举而非用户输入拼接路径，不存在路径注入空间 |
| SSE 流处理正确性 | **发现 1 项 High 缺陷** | 见 F1 |
| 请求取消正确性 | **发现 1 项 Medium 缺陷** | 见 F2 |
| 是否存在无限重试 | 未发现无限重试 | `api.rs::stream_chat` 用 `attempts == 1` 限制最多重试 1 次（总计最多 2 次请求），非无限循环；但重试触发条件本身有缺陷，见 F1 |
| 自动切换是否只针对允许的错误 | 不适用 | Nova 0.1 只接入单一 SDKMAX Provider，代码中未实现任何"自动切换 Provider/Model"逻辑，此检查项当前阶段无对应实现，不构成缺陷 |
| 跨模型切换是否保存历史 | 符合 | `messages` 表按消息记录各自的 `model_id`（`db.rs:219-236`），切换 `selectedModelId` 不影响历史消息内容，会话列表/消息列表按 `conversation_id` 持久化并可重启后恢复 |
| SQLite 迁移是否可回滚 | 基本符合（无破坏性变更） | `db.rs::migrate` 全部使用 `CREATE TABLE IF NOT EXISTS`，幂等可重复执行（已有单测 `migrations_are_repeatable_and_conversation_restores` 验证），当前阶段没有 `ALTER/DROP` 等不可逆操作，暂无"回滚脚本"但也没有需要回滚的破坏性变更 |
| 是否修改 SDKMAX/New API 计费、倍率、渠道或余额核心 | 未修改 | 本次提交范围仅为 `apps/desktop`、`docs`、`package.json`，未触及 `D:\sdkmax`；`audit/SDKMAX_NOVA_RISK_LIST.md` 中列出的禁止改动清单文件未出现在 diff 中 |
| 测试是否覆盖验收链路 | 部分覆盖，**F1 对应场景缺失测试** | 已有 Rust/前端单测覆盖 URL 校验、allowlist、脱敏、SSE 正常/[DONE]/畸形 JSON 解析、凭据存取、迁移幂等（见 `test-results/cargo-test.log` 13/13 通过、`npm-test.log` 3/3 通过）；但没有覆盖"收到部分 Delta 后流异常结束（无 [DONE]）触发重试"的场景，该场景正是 F1 缺陷所在 |
| 构建产物是否对应当前 Git commit | 无法严格验证，见 F4 | 时间线上 `nova.exe`(16:55) 与各测试日志(16:07–16:56)均早于提交时间 `3e1d2dd`(17:00)，顺序合理，但日志/产物未记录 commit hash，无法从产物本身强验证 |

---

## 二、发现问题

### F1（High）SSE 流未完整结束时的静默重试会污染已保存的会话历史，且不会标记为 interrupted

- **文件路径**：`D:\nova\apps\desktop\src-tauri\src\api.rs`（`stream_chat` 第 118-132 行，`stream_chat_once` 第 164-220 行）；连带 `D:\nova\apps\desktop\src-tauri\src\commands.rs`（`send_chat_message` 中的 `on_chunk` 回调，第 209-225 行，使用 `db.append_message_delta` 做内容累加）
- **证据**：
  1. `stream_chat_once` 在流正常收完字节但未见到 `data: [DONE]` 时，返回 `Err(ApiErrorDetail{ status: None, code: Some("sse_incomplete"), .. })`（`api.rs:212-219`），此时 `saw_delta` 完全可能已经为 `true`（即已经调用过 `on_chunk` 并把增量写入了 DB 和推送给前端）。
  2. `stream_chat` 的重试条件为：
     ```rust
     Err(detail)
         if attempts == 1
             && detail.status.is_none()
             && !cancel.is_cancelled()
             && detail.code.as_deref() != Some("stream_started") =>
     { continue; }
     ```
     该条件只排除了"字节读取本身出错且已产生过 Delta"的场景（`code == "stream_started"`，见 `api.rs:177-186`），**没有排除 `sse_incomplete`**。因此，当上游连接在推送若干增量后被正常关闭（未发送 `[DONE]`）——例如渠道切换、代理超时、SDKMAX 侧连接重置——Nova 会静默发起第二次完整的 `/v1/chat/completions` 请求。
  3. 第二次请求复用同一个 `on_chunk` 闭包，其在 `commands.rs` 中通过 `db.append_message_delta`（SQL: `content = content || ?1`）把新内容**追加**在第一次已经写入的部分内容之后，而不会先清空/回滚旧内容；前端 `App.vue` 的 `chat://chunk` 监听器同样是 `message.content += event.payload.content_delta`，同样是纯追加。
- **影响**：用户会在本地聊天记录中看到"旧的部分回答 + 新一次完整回答"拼接在一起的错乱/重复内容，并被永久写入 SQLite（`messages` 表），无法被用户察觉是数据损坏；同时该消息最终状态很可能被标记为 `complete`（如果第二次请求正常收到 `[DONE]`），而不是按 PRD "Stop an active stream and keep partial assistant output marked as interrupted" 的语义应有的 `interrupted`。这直接违反了本次审核明确要求的"是否正确处理 SSE 流"与"是否正确取消/中断标记"验收点。
- **建议修复方法**：
  - 在 `stream_chat_once` 的返回值中携带 `saw_delta`（例如把 `ApiErrorDetail` 扩展一个字段，或改为返回一个 tuple/struct），`stream_chat` 的重试条件里增加 `&& !saw_delta_from_previous_attempt`，一旦已经有增量落地就不允许静默重试，直接把错误向上传递为 `interrupted`/`error`。
  - 或者：允许重试，但重试前必须先通过一次数据库操作清空/重置该 assistant 消息在第一次尝试中已写入的内容（同时通知前端丢弃已渲染的增量），保证"重试"在语义上是"重新开始"而不是"接着写"。
  - 两种方案二选一即可，但不能保持现状（静默追加）。
- **验证方法**：
  1. 补充单测：构造一个假的 SSE 序列，先产生一次 `Delta`，随后流直接结束（无 `[DONE]`），断言 `stream_chat_once` 报告的错误足以让上层判定"不可重试"或"需要先清空内容"；或直接在 `stream_chat` 层面加集成测试，验证重试发生时 assistant 消息内容不会同时包含两次响应的拼接。
  2. 手动验证：可用本地 mock HTTP 服务器，先写入若干 `data: {...}` chunk 后直接关闭连接（不发送 `[DONE]`），运行 `npm run tauri dev` 发消息，检查最终消息内容与 `request_logs`/消息 `status` 是否符合预期（应为 `interrupted` 或内容不重复）。

### F2（Medium）取消请求（Stop）在上游长时间未发送新数据时不能立即生效

- **文件路径**：`D:\nova\apps\desktop\src-tauri\src\api.rs`，`stream_chat_once` 第 168-176 行
- **证据**：
  ```rust
  while let Some(item) = stream.next().await {
      if cancel.is_cancelled() {
          return Err(ApiErrorDetail { code: Some("user_canceled"), .. });
      }
      ...
  }
  ```
  取消检查只发生在 `stream.next().await` **返回之后**，即必须等到下一段字节到达（或连接关闭/45 秒超时）才会被检测到；取消令牌本身不会打断正在进行中的 `.await`。
- **影响**：当 SDKMAX 上游推流出现停顿（渠道慢响应、网络抖动等）时，用户点击"停止"不会立即生效，Nova 会继续挂起等待，直至下一字节到达或客户端超时（45 秒，`lib.rs:29`）。不符合"是否正确取消请求"的验收要求，属于用户可感知的功能缺陷。
- **建议修复方法**：使用 `tokio::select!` 让取消信号可以抢占字节读取，例如：
  ```rust
  loop {
      tokio::select! {
          biased;
          _ = cancel.cancelled() => { return Err(user_canceled_detail); }
          item = stream.next() => { match item { ... } }
      }
  }
  ```
- **验证方法**：模拟一个"建立连接后长时间不发送任何字节"的假上游，调用 `cancel_active_chat` 后断言在毫秒级内（而不是等到超时）收到 `chat://error` 且 `code == user_canceled`。

### F3（Medium）生产环境 WebView 未设置 CSP

- **文件路径**：`D:\nova\apps\desktop\src-tauri\tauri.conf.json` 第 22-24 行
- **证据**：
  ```json
  "security": {
    "csp": null
  }
  ```
  Tauri 官方安全基线建议桌面应用配置限制性 CSP 以降低 WebView 被注入执行任意脚本或加载非预期资源的风险；当前配置显式关闭。
- **影响**：与本次审核"Tauri 权限是否最小化"的要求不完全一致。虽然当前 Nova 前端内容都是本地构建产物、未见到明显的 XSS 注入点（消息内容通过 Vue 模板 `{{ }}` 插值渲染，默认会转义），但缺少 CSP 意味着一旦未来引入 Markdown/HTML 渲染、第三方脚本或依赖被污染，WebView 没有纵深防御兜底。
- **建议修复方法**：设置最小可用 CSP，例如 `default-src 'self'`，如无必须的内联脚本/样式需求则不额外放开 `unsafe-inline`；由于网络请求都在 Rust 侧发起（前端不直接 `fetch` 外部域名），`connect-src` 理论上可以保持严格，不需要放开到 `https://api.sdkmax.com`。
- **验证方法**：设置 CSP 后执行 `npm run tauri dev`，通过浏览器开发者工具或 `preview_console_logs` 确认无 CSP 违规报错，且聊天、模型刷新、Provider 保存等现有功能均正常；可额外手动在消息中输入 `<script>alert(1)</script>` 验证不会被执行（预期本来就不会执行，CSP 是纵深防御而非本次已知漏洞的修复）。

### F4（Low）构建产物与测试日志未记录对应的 Git commit hash

- **文件路径**：`D:\nova\test-results\cargo-test.log`、`npm-build.log`、`npm-test.log`、`tauri-build.log`；`D:\nova\apps\desktop\src-tauri\target\release\nova.exe`
- **证据**：各日志文件内容均只有编译器/测试运行器输出，没有 `git rev-parse HEAD` 或等价信息。时间线上 `nova.exe` 的文件时间（2026-08-30 16:55）与各测试日志（16:07–16:56）均早于提交 `3e1d2dd` 的时间戳（17:00），顺序看起来合理（先构建测试、后提交），但缺少显式绑定，无法仅凭产物本身证明其对应哪次提交。
- **影响**：低风险，属于可追溯性/流程改进项，不影响当前功能正确性；但如果未来出现"提交内容与实际构建产物不一致"的争议，缺少留痕依据。
- **建议修复方法**：在构建/测试脚本执行前后打印并写入 `git rev-parse HEAD` 与 `git status --porcelain` 到对应日志文件头部或单独的 `build-info.txt`。
- **验证方法**：重新运行构建/测试脚本，检查生成日志首行包含 commit hash，且与 `git log -1 --format=%H` 输出一致。

### F5（Low / 设计留意项）全局唯一取消令牌，未来若允许并发发送会导致早前流无法被停止

- **文件路径**：`D:\nova\apps\desktop\src-tauri\src\lib.rs`（`AppState.active_cancel: Arc<Mutex<Option<CancellationToken>>>`）；`D:\nova\apps\desktop\src-tauri\src\commands.rs` 第 187-191 行
- **证据**：`active_cancel` 是单一全局槽位，若连续两次调用 `send_chat_message`（无论是否被当前前端 UI 禁止），第二次会直接覆盖第一次的取消令牌。
- **影响**：当前前端通过 `store.sending` 状态在 UI 层隐藏了并发发送入口（`App.vue` 中发送中只显示"停止"按钮），正常单窗口交互下不会触发该问题；但这是一个architecture 层面的隐患——一旦以后扩展为允许多会话同时进行流式回复，先发起的那一路将无法再被用户主动停止（不会崩溃，只是"停止"按钮实际上只能停最新的一路）。
- **建议修复方法**：把取消令牌按 `assistant_message_id` 或 `conversation_id` 存放在 `HashMap` 中而不是单一 `Option`。
- **验证方法**：本阶段无需立即修复，作为后续多会话并行能力上线前的前置检查项跟踪即可；如需现在验证影响面，可确认当前 UI 在 `sending=true` 时是否有任何路径能再次触发 `send_chat_message`（审核未发现此类路径）。

---

## 三、本次审核中确认符合要求、未发现问题的项目

- API Key 全链路：仅 `save_provider` 接受明文 Key → 立即写入 Windows Credential Manager → SQLite 只存 `masked_key`/`has_key` → 前端输入框保存后立刻清空 → 后续调用只传 `provider_id`，Key 由 Rust 侧读取凭据库后用 `reqwest::RequestBuilder::bearer_auth` 注入，前端/JS 层完全不可见明文 Key。
- 日志与诊断信息脱敏：Rust `redact_sensitive_text`/`mask_secret`/`mask_request_id` 与前端 `sanitizeDiagnostic`/`maskKey` 逻辑一致且都有对应单测覆盖，`request_logs` 表结构本身也不包含请求体/Header 字段。
- 端点 allowlist：只有 `/v1/models`、`/v1/chat/completions`、`/api/usage/token/` 三个路径可被调用，`base_url` 校验强制生产环境必须是 `https://api.sdkmax.com`，且不允许 URL 内嵌用户名密码或额外路径，开发模式的 localhost 例外仅在 `debug_assertions` 下生效。
- Tauri 能力面：`capabilities/default.json` 只声明 `core:default`，未引入文件系统/Shell/HTTP 等高风险插件权限；所有网络请求都通过 Rust `reqwest` 发起，前端没有直接网络访问能力。
- 未触碰 SDKMAX 服务端代码或 New API 计费/渠道/倍率核心：本次提交 diff 仅限 `apps/desktop`、`docs`、根 `package.json`，`D:\sdkmax` 未被修改；`audit/SDKMAX_NOVA_RISK_LIST.md` 中列出的禁止改动清单文件均未出现在改动范围内。
- 重试次数有界：`stream_chat` 的重试受 `attempts == 1` 限制，最多两次请求，不存在无限重试（尽管重试触发条件本身有缺陷，见 F1）。
- 跨模型会话历史：消息按 `conversation_id` 持久化，各消息独立记录 `model_id`，切换模型不影响既有历史，重启应用后可从 SQLite 正常恢复（有对应单测 `migrations_are_repeatable_and_conversation_restores`）。
- 未发现任何提交的密钥/凭据文件；`.gitignore` 已排除 `*.sqlite*`、`*.log`、`.env*`、`src-tauri/target`、`src-tauri/gen`。

---

## 四、结论与建议

维持 **CONDITIONAL PASS**：

1. **必须处理**：F1（SSE 增量流不完整时的静默重试导致历史数据污染）。这是本阶段唯一的 High 级别问题，且直接对应审核任务中明确列出的"是否正确处理 SSE 流"验收点，建议在进入下一阶段前由 Codex 修复并补充对应回归测试。
2. **建议处理**：F2（取消请求非抢占式）、F3（CSP 未设置），两者都不构成当前已知的可利用安全漏洞，但分别影响"取消功能正确性"和"Tauri 权限最小化"两个明确的审核维度，建议在本阶段或下一次迭代中处理。
3. **可延后/跟踪**：F4（构建产物与 commit 的可追溯性）、F5（全局取消令牌的架构隐患），均为低风险改进项，不阻塞当前阶段推进。

本报告未对代码做任何修改，所有修复应由 Codex 在后续提交中完成，并在修复后提请复核。
