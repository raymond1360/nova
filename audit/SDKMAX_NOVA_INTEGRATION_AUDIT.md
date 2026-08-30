# SDKMAX -> Nova 第一阶段只读接入审计

审计日期：2026-08-30  
审计范围：`D:\sdkmax\new-api` 与 `D:\nova` 本地目录。未连接生产数据库，未访问或修改 `api.sdkmax.com`，未修改 `D:\sdkmax`，未提交代码。

## 总结结论

SDKMAX 的核心能力应继续由 New API 承担：用户、Token、计费、倍率、分组、渠道选择、扣费和故障切换都已有完整链路。Nova 第一阶段可只复用“登录态查询、账户状态、Token 管理、用户可用模型、OpenAI 兼容调用”这些接口，但不建议桌面端直接复用全部 Web Session/OAuth 流程作为最终方案。

推荐 MVP：Nova 使用 SDKMAX Web 登录或嵌入浏览器完成登录，随后通过一个新增的 Nova 专用后端包装接口创建或复用 `nova-desktop` Token；AI 调用走 `/v1/chat/completions`、`/v1/responses`、`/v1/models`，账户轮询走 `/api/user/self` 与 `/api/usage/token/`。不要让 Nova 直接接触管理员接口、渠道接口、倍率配置接口、上游 Key、Root AccessToken 或数据库。

## 一、账号与登录

### 1. 当前支持的登录方式

可确认的登录方式：

- 用户名密码：`POST /api/user/login`，证据：`D:\sdkmax\new-api\router\api-router.go:63`、`D:\sdkmax\new-api\controller\user.go:33`。
- 用户名密码 + 2FA：密码正确后返回 `require_2fa`，再 `POST /api/user/login/2fa`，证据：`D:\sdkmax\new-api\controller\user.go:68`、`D:\sdkmax\new-api\controller\twofa.go:382`。
- Passkey/WebAuthn：`/api/user/passkey/login/begin`、`/finish`，证据：`D:\sdkmax\new-api\router\api-router.go:65`。
- 标准 OAuth 回调统一路由：`GET /api/oauth/:provider`，支持已注册 provider，包括 GitHub、Discord、OIDC、LinuxDO 和自定义 OAuth，证据：`D:\sdkmax\new-api\router\api-router.go:52`、`D:\sdkmax\new-api\controller\oauth.go:43`、`D:\sdkmax\new-api\oauth\provider.go:10`。
- WeChat、Telegram 为非标准 OAuth/登录路径，证据：`D:\sdkmax\new-api\router\api-router.go:47` 到 `D:\sdkmax\new-api\router\api-router.go:49`。

登录开关默认值在 `D:\sdkmax\new-api\common\constants.go:84` 到 `D:\sdkmax\new-api\common\constants.go:91`，实际运行时可能由配置覆盖。

### 2. 登录接口、参数、响应结构

密码登录：

```http
POST /api/user/login
Content-Type: application/json

{
  "username": "alice",
  "password": "***REDACTED***"
}
```

成功且无 2FA：

```json
{
  "success": true,
  "message": "",
  "data": {
    "id": 123,
    "username": "alice",
    "display_name": "Alice",
    "role": 1,
    "status": 1,
    "group": "default"
  }
}
```

证据：请求结构在 `D:\sdkmax\new-api\controller\user.go:33` 到 `D:\sdkmax\new-api\controller\user.go:45`；响应和 Session 写入在 `D:\sdkmax\new-api\controller\user.go:93` 到 `D:\sdkmax\new-api\controller\user.go:118`。

需要 2FA：

```json
{
  "success": true,
  "message": "require 2FA",
  "data": {
    "require_2fa": true
  }
}
```

2FA 完成：

```http
POST /api/user/login/2fa
Content-Type: application/json

{
  "code": "***REDACTED***"
}
```

成功后复用 `setupLogin` 的用户响应。证据：`D:\sdkmax\new-api\controller\user.go:68` 到 `D:\sdkmax\new-api\controller\user.go:87`，`D:\sdkmax\new-api\controller\twofa.go:393` 到 `D:\sdkmax\new-api\controller\twofa.go:467`。

### 3. OAuth、OIDC、PKCE、设备授权

支持 OAuth Authorization Code 回调，但当前普通用户登录链路不是桌面原生 PKCE 或设备授权。

- `GET /api/oauth/state` 生成 CSRF state 并存入 Session，证据：`D:\sdkmax\new-api\controller\oauth.go:22` 到 `D:\sdkmax\new-api\controller\oauth.go:40`。
- `GET /api/oauth/:provider?code=...&state=...` 校验 state、服务端交换 token、读取 userinfo、创建或登录用户，证据：`D:\sdkmax\new-api\controller\oauth.go:43` 到 `D:\sdkmax\new-api\controller\oauth.go:128`。
- OIDC 使用 `client_secret` + `authorization_code` + 固定 `redirect_uri`，未发现 PKCE `code_verifier` 参数，证据：`D:\sdkmax\new-api\oauth\oidc.go:51` 到 `D:\sdkmax\new-api\oauth\oidc.go:75`。
- 仓库中存在 Codex 渠道自身的 PKCE OAuth，但这是管理员配置 Codex 上游渠道用，不是 SDKMAX 用户登录，证据：`D:\sdkmax\new-api\controller\codex_oauth.go`、`D:\sdkmax\new-api\service\codex_oauth.go`。

未发现通用 OIDC Device Authorization Grant。

### 4. 桌面客户端能否安全复用现有登录机制

短期可以复用 Web 登录机制，但更适合“Web 登录完成后换取 Nova 专用设备 Token”。原因：

- `/api/user/*` 依赖 Cookie Session 和 `New-Api-User` 请求头双重校验，证据：`D:\sdkmax\new-api\middleware\auth.go:36` 到 `D:\sdkmax\new-api\middleware\auth.go:123`。
- OAuth 回调绑定当前 Web Session 的 `oauth_state`，桌面外部浏览器和 Tauri WebView 之间 Cookie 共享、SameSite、回调域名都会增加复杂度。
- 当前登录接口会把 Session 写到浏览器 Cookie；它不是面向桌面设备授权、Refresh Token 轮换、设备绑定、吊销审计设计的协议。

### 5. 当前 Session/Cookie 是否适合 Tauri

只适合临时复用，不适合作为长期桌面授权根凭证。

Cookie 配置：`MaxAge=2592000`、`HttpOnly=true`、`Secure=false`、`SameSite=Strict`，证据：`D:\sdkmax\new-api\main.go:183` 到 `D:\sdkmax\new-api\main.go:192`。

风险：

- `Secure=false` 对生产 HTTPS 桌面接入不理想。
- `SameSite=Strict` 对外部浏览器 OAuth 回跳到桌面或跨域 WebView 可能失败。
- Session 中保存 `id`、`username`、`role`、`status`、`group`，证据：`D:\sdkmax\new-api\controller\user.go:96` 到 `D:\sdkmax\new-api\controller\user.go:101`。
- 前端通过本地 `uid` 拼 `New-Api-User`，证据：`D:\sdkmax\new-api\web\default\src\lib\api.ts:149` 到 `D:\sdkmax\new-api\web\default\src\lib\api.ts:167`。

### 6. 获取当前用户 ID、用户名、邮箱、用户组和状态

接口：`GET /api/user/self`。需要登录 Session 或用户 AccessToken，并带 `New-Api-User`。

响应示例：

```json
{
  "success": true,
  "message": "",
  "data": {
    "id": 123,
    "username": "alice",
    "display_name": "Alice",
    "role": 1,
    "status": 1,
    "email": "a***@example.com",
    "group": "default",
    "quota": 500000,
    "used_quota": 12000,
    "request_count": 42
  }
}
```

证据：路由在 `D:\sdkmax\new-api\router\api-router.go:82`；字段在 `D:\sdkmax\new-api\controller\user.go:395` 到 `D:\sdkmax\new-api\controller\user.go:443`；用户模型字段在 `D:\sdkmax\new-api\model\user.go:24` 到 `D:\sdkmax\new-api\model\user.go:54`。

### 7. 刷新登录状态和退出

- 刷新登录状态：调用 `GET /api/user/self`。401 代表未登录或 Session 失效；`success:false` 且 message 表示业务失败。证据：`D:\sdkmax\new-api\web\default\src\lib\api.ts:179` 到 `D:\sdkmax\new-api\web\default\src\lib\api.ts:185`。
- 退出登录：`GET /api/user/logout` 清空 Session，证据：`D:\sdkmax\new-api\router\api-router.go:66`、`D:\sdkmax\new-api\controller\user.go:121` 到 `D:\sdkmax\new-api\controller\user.go:134`。

## 二、API Token

### 1-2. 普通用户 Token CRUD 接口

均位于 `tokenRoute := apiRouter.Group("/api/token")`，使用 `middleware.UserAuth()`，证据：`D:\sdkmax\new-api\router\api-router.go:276` 到 `D:\sdkmax\new-api\router\api-router.go:286`。

| 操作 | 方法与路径 | 证据 |
|---|---|---|
| 查询列表 | `GET /api/token/` | `D:\sdkmax\new-api\controller\token.go:34` |
| 搜索 | `GET /api/token/search?keyword=&token=` | `D:\sdkmax\new-api\controller\token.go:48` |
| 查询单个 | `GET /api/token/:id` | `D:\sdkmax\new-api\controller\token.go:65` |
| 查看完整 Key | `POST /api/token/:id/key` | `D:\sdkmax\new-api\controller\token.go:80` |
| 创建 | `POST /api/token/` | `D:\sdkmax\new-api\controller\token.go:167` |
| 更新 | `PUT /api/token/` | `D:\sdkmax\new-api\controller\token.go:249` |
| 状态更新 | `PUT /api/token/?status_only=1` | `D:\sdkmax\new-api\controller\token.go:249` 到 `D:\sdkmax\new-api\controller\token.go:300` |
| 删除 | `DELETE /api/token/:id` | `D:\sdkmax\new-api\controller\token.go:235` |
| 批量删除 | `POST /api/token/batch` | `D:\sdkmax\new-api\controller\token.go:318` |
| 批量查看完整 Key | `POST /api/token/batch/keys` | `D:\sdkmax\new-api\controller\token.go:337` |

### 3. 创建 Token 是否需要 New-Api-User 请求头

需要。`/api/token` 使用 `UserAuth()`，而 `authHelper` 在 Session/AccessToken 通过后仍强制校验 `New-Api-User` 与当前用户 ID 一致，证据：`D:\sdkmax\new-api\middleware\auth.go:95` 到 `D:\sdkmax\new-api\middleware\auth.go:123`。

### 4. 如何创建标记为 nova-desktop 的专用 Token

现有可行但不完美方式：用 `name` 标记，例如 `nova-desktop:<device_hash_prefix>`。请求示例：

```http
POST /api/token/
Cookie: session=***REDACTED***
New-Api-User: 123
Content-Type: application/json

{
  "name": "nova-desktop:win-7f3a2c91",
  "expired_time": -1,
  "remain_quota": 0,
  "unlimited_quota": true,
  "model_limits_enabled": false,
  "model_limits": "",
  "allow_ips": "",
  "group": "auto",
  "cross_group_retry": true
}
```

响应会返回脱敏 key：

```json
{
  "success": true,
  "message": "",
  "data": {
    "id": 456,
    "user_id": 123,
    "key": "abcd**********wxyz",
    "status": 1,
    "name": "nova-desktop:win-7f3a2c91"
  }
}
```

证据：可写字段在 `D:\sdkmax\new-api\model\token.go:14` 到 `D:\sdkmax\new-api\model\token.go:31`；创建时复制字段在 `D:\sdkmax\new-api\controller\token.go:208` 到 `D:\sdkmax\new-api\controller\token.go:222`。

### 5. 如何防止同一设备重复创建 Token

现有后端没有 `device_id` 唯一约束；只能先 `GET /api/token/search?keyword=nova-desktop:<device_hash_prefix>` 查找，再创建。这个方案不是强一致，存在并发重复风险。

推荐最小改造：新增 Nova 专用接口 `POST /api/nova/device-token`，服务端根据 `user_id + device_id_hash` 做幂等创建或返回已有 Token 元数据。若不改表，可把 `device_id_hash` 放进 `name` 并在事务中二次检查；更稳妥是新增 `client_type`、`device_id_hash`、`last_seen_at` 字段或单独设备 Token 表。

### 6. Token 创建后是否只能返回一次完整密钥

不是。现有创建响应返回的是脱敏 key，完整 key 仍可通过 `POST /api/token/:id/key` 或批量 `/api/token/batch/keys` 再取回。证据：创建响应使用 `buildMaskedTokenResponse`，`D:\sdkmax\new-api\controller\token.go:228` 到 `D:\sdkmax\new-api\controller\token.go:231`；完整 key 接口在 `D:\sdkmax\new-api\controller\token.go:80` 到 `D:\sdkmax\new-api\controller\token.go:94`；`GetFullKey()` 直接返回 `token.Key`，证据：`D:\sdkmax\new-api\model\token.go:51`。

这对桌面安全是高风险点：Nova 专用接口应只在首次创建设备 Token 时返回一次完整密钥；后续只返回脱敏 key 和状态。

### 7. 如何远程吊销设备 Token

现有方式：

- 用户自己删除：`DELETE /api/token/:id`。
- 用户禁用：`PUT /api/token/?status_only=1`，body 含 `id` 和 `status`。
- 管理员禁用/删除用户会清理该用户 Token Redis 缓存，证据：`D:\sdkmax\new-api\controller\user.go` 的 `ManageUser` 缓存失效逻辑、`D:\sdkmax\new-api\model\token.go:476` 到 `D:\sdkmax\new-api\model\token.go:500`。

Nova MVP 应实现“设备列表 + 远程撤销”包装，禁止桌面直接使用批量完整 Key 导出接口。

## 三、模型

### 1. `/api/user/models` 真实返回结构

```json
{
  "success": true,
  "message": "",
  "data": ["gpt-4o-mini", "claude-3-5-sonnet"]
}
```

它返回当前用户可见分组下的模型字符串列表，使用 `GetUserUsableGroups(user.Group)` + `GetCustomerVisibleGroupModels(group)`。证据：`D:\sdkmax\new-api\controller\user.go:531` 到 `D:\sdkmax\new-api\controller\user.go:555`，`D:\sdkmax\new-api\model\ability.go:147` 到 `D:\sdkmax\new-api\model\ability.go:150`。

注意：它不读取某个 API Token 的 `model_limits`。

### 2. `/v1/models` 真实返回结构

OpenAI 兼容格式：

```json
{
  "success": true,
  "object": "list",
  "data": [
    {
      "id": "gpt-4o-mini",
      "object": "model",
      "created": 1626777600,
      "owned_by": "openai",
      "supported_endpoint_types": ["chat_completions"]
    }
  ]
}
```

证据：路由在 `D:\sdkmax\new-api\router\relay-router.go:18` 到 `D:\sdkmax\new-api\router\relay-router.go:31`；响应在 `D:\sdkmax\new-api\controller\model.go:208` 到 `D:\sdkmax\new-api\controller\model.go:314`；模型对象构造在 `D:\sdkmax\new-api\controller\model.go:153` 到 `D:\sdkmax\new-api\controller\model.go:169`。

Anthropic/Gemini 兼容请求会返回不同结构，证据：`D:\sdkmax\new-api\controller\model.go:280` 到 `D:\sdkmax\new-api\controller\model.go:308`。

### 3. 哪个接口代表用户真正可调用模型

对 Nova 的 AI 调用 Token 来说，`/v1/models` 更接近“该 Token 真正可调用模型”，因为它经过 `TokenAuth()`、用户状态、Token 状态、Token 分组、`model_limits`、用户组/auto group 和计费配置过滤。

`/api/user/models` 适合显示“账号可见模型”，但不代表某个 Token 的实际可调用集合。

### 4. `model_limits` 和用户组模型权限如何生效

- TokenAuth 把 `model_limits_enabled` 和 `model_limits` 写入上下文，证据：`D:\sdkmax\new-api\middleware\auth.go:405` 到 `D:\sdkmax\new-api\middleware\auth.go:424`。
- AI 调用分发前检查请求模型是否在 Token 模型限制内，证据：`D:\sdkmax\new-api\middleware\distributor.go:57` 到 `D:\sdkmax\new-api\middleware\distributor.go:77`。
- `/v1/models` 同样读取 Token model limit，证据：`D:\sdkmax\new-api\controller\model.go:230` 到 `D:\sdkmax\new-api\controller\model.go:246`。
- 用户组通过 `abilities` 表的 `group + model + enabled` 决定，证据：`D:\sdkmax\new-api\model\ability.go:16` 到 `D:\sdkmax\new-api\model\ability.go:23`、`D:\sdkmax\new-api\model\ability.go:140` 到 `D:\sdkmax\new-api\model\ability.go:157`。

### 5. `/pricing` 与用户可调用模型为什么可能不一致

`/api/pricing` 是展示/定价视图：按用户可用组过滤 pricing 数据、返回 vendors、group ratio、supported endpoint、auto groups。证据：`D:\sdkmax\new-api\controller\pricing.go:36` 到 `D:\sdkmax\new-api\controller\pricing.go:76`。

不一致原因：

- `/pricing` 受定价表与展示配置影响，不等于 `abilities` 实时渠道可用性。
- `/api/user/models` 走客户可见模型过滤。
- `/v1/models` 还受 Token `model_limits` 和是否有计费配置影响，证据：`D:\sdkmax\new-api\controller\model.go:209` 到 `D:\sdkmax\new-api\controller\model.go:267`。

### 6. Nova 应使用哪个接口获取模型列表

推荐：

- 登录后展示账号模型：`GET /api/user/models`。
- 绑定或创建设备 Token 后实际调用模型：`GET /v1/models`，使用 `Authorization: Bearer sk-***REDACTED***`。
- 价格页或模型详情才用 `/api/pricing`，不要用它判断能否调用。

## 四、账户

### 1. 获取余额、已用额度和剩余额度

用户账户级：`GET /api/user/self` 返回 `quota`、`used_quota`、`request_count`。证据：`D:\sdkmax\new-api\controller\user.go:425` 到 `D:\sdkmax\new-api\controller\user.go:427`。

Token 级：`GET /api/usage/token/` 使用 Bearer Token，返回：

```json
{
  "code": true,
  "message": "ok",
  "data": {
    "object": "token_usage",
    "name": "nova-desktop:win-7f3a2c91",
    "total_granted": 500000,
    "total_used": 12000,
    "total_available": 488000,
    "unlimited_quota": false,
    "model_limits": {},
    "model_limits_enabled": false,
    "expires_at": 0
  }
}
```

证据：路由在 `D:\sdkmax\new-api\router\api-router.go` 的 `/api/usage/token` 段；控制器在 `D:\sdkmax\new-api\controller\token.go:118` 到 `D:\sdkmax\new-api\controller\token.go:164`。

### 2. 判断封禁、Token 失效或余额不足

- 用户封禁：`status == common.UserStatusDisabled` 时 UserAuth 返回业务失败，TokenAuth 返回 403，证据：`D:\sdkmax\new-api\middleware\auth.go:123` 到 `D:\sdkmax\new-api\middleware\auth.go:130`、`D:\sdkmax\new-api\middleware\auth.go:363` 到 `D:\sdkmax\new-api\middleware\auth.go:373`。
- Token 失效/过期/耗尽：`ValidateUserToken` 检查 `status`、`expired_time`、`remain_quota`，证据：`D:\sdkmax\new-api\model\token.go:180` 到 `D:\sdkmax\new-api\model\token.go:211`。
- 余额不足错误码包含 `insufficient_user_quota`、`pre_consume_token_quota_failed`，证据：`D:\sdkmax\new-api\types\error.go:85` 到 `D:\sdkmax\new-api\types\error.go:88`。

### 3. 是否存在适合 Nova 轮询的账户状态接口

现有可轮询：

- `GET /api/user/self`：账号状态与余额。
- `GET /api/usage/token/`：设备 Token 状态与剩余额度。注意 `TokenAuthReadOnly` 允许过期、耗尽或禁用 Token 查询自身用量，但仍检查用户封禁，证据：`D:\sdkmax\new-api\middleware\auth.go:210` 到 `D:\sdkmax\new-api\middleware\auth.go:269`。

推荐 Nova 轮询间隔 30-120 秒，窗口聚焦/调用失败后立即刷新。

## 五、AI 调用

### 1. OpenAI 兼容 Chat Completions

接口：`POST /v1/chat/completions`，需要 `Authorization: Bearer sk-***REDACTED***`。证据：`D:\sdkmax\new-api\router\relay-router.go:90` 到 `D:\sdkmax\new-api\router\relay-router.go:96`。

```http
POST /v1/chat/completions
Authorization: Bearer sk-***REDACTED***
Content-Type: application/json

{
  "model": "gpt-4o-mini",
  "messages": [{"role": "user", "content": "hello"}],
  "stream": true
}
```

### 2. 是否支持 Responses API

支持：

- `POST /v1/responses`
- `POST /v1/responses/compact`

证据：`D:\sdkmax\new-api\router\relay-router.go:98` 到 `D:\sdkmax\new-api\router\relay-router.go:103`，处理器在 `D:\sdkmax\new-api\relay\responses_handler.go:23`。

### 3. SSE 流式输出格式

Chat Completions 流式输出按 OpenAI SSE：多条 `data: {json}\n\n`，结束 `data: [DONE]\n\n`。证据：OpenAI 流处理在 `D:\sdkmax\new-api\relay\channel\openai\relay-openai.go:106`，辅助处理在 `D:\sdkmax\new-api\relay\channel\openai\helper.go:146` 到 `D:\sdkmax\new-api\relay\channel\openai\helper.go:205`；测试解析也按 `data:` 与 `[DONE]`，证据：`D:\sdkmax\new-api\controller\channel-test.go:619` 到 `D:\sdkmax\new-api\controller\channel-test.go:653`。

示例：

```text
data: {"id":"chatcmpl_xxx","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hi"}}]}

data: [DONE]
```

### 4. 错误响应结构和错误码

OpenAI 兼容错误：

```json
{
  "error": {
    "message": "insufficient user quota",
    "type": "new_api_error",
    "param": "",
    "code": "insufficient_user_quota"
  }
}
```

结构证据：`D:\sdkmax\new-api\types\error.go:13` 到 `D:\sdkmax\new-api\types\error.go:18`。错误码证据：`D:\sdkmax\new-api\types\error.go:40` 到 `D:\sdkmax\new-api\types\error.go:88`。

常见错误码：`invalid_request`、`sensitive_words_detected`、`model_price_error`、`get_channel_failed`、`channel:no_available_key`、`channel:invalid_key`、`access_denied`、`bad_request_body`、`bad_response_status_code`、`bad_response_body`、`model_not_found`、`insufficient_user_quota`。

### 5. New API 现有渠道故障切换机制

分发链路：

1. `TokenAuth` 验证 Token、用户状态、分组、模型限制，证据：`D:\sdkmax\new-api\middleware\auth.go:272` 到 `D:\sdkmax\new-api\middleware\auth.go:401`。
2. `Distribute` 从请求体解析 `model`，证据：`D:\sdkmax\new-api\middleware\distributor.go:32` 到 `D:\sdkmax\new-api\middleware\distributor.go:40`。
3. 优先使用渠道亲和缓存，再用 `CacheGetRandomSatisfiedChannel` 按组、模型、优先级、权重选择，证据：`D:\sdkmax\new-api\middleware\distributor.go:104` 到 `D:\sdkmax\new-api\middleware\distributor.go:156`。
4. auto group 和跨组重试逻辑在 `D:\sdkmax\new-api\service\channel_select.go:76` 到 `D:\sdkmax\new-api\service\channel_select.go:146`。

### 6. 客户端是否能够知道最终使用了哪个渠道

默认不应让普通客户端知道。代码中存在管理员专用“指定渠道”能力，普通用户通过 `sk-xxx-<channel_id>` 指定渠道会被拒绝，证据：`D:\sdkmax\new-api\middleware\auth.go:425` 到 `D:\sdkmax\new-api\middleware\auth.go:431`。SDKMAX 对渠道路由和上游容灾负责，Nova 不应依赖或展示最终渠道 ID。

## 六、安全与改造建议

### 可直接复用

- `GET /api/user/self`：登录态、账号状态、余额、用户组。
- `GET /api/user/models`：账号可见模型。
- `GET /v1/models`：设备 Token 实际可调用模型。
- `POST /v1/chat/completions`：OpenAI Chat Completions。
- `POST /v1/responses`：Responses API，模型支持时使用。
- `GET /api/usage/token/`：Token 用量/余额状态。
- `GET /api/user/logout`：Web Session 登出。

### 需要包装

- `POST /api/token/`：应包装为 Nova 幂等设备 Token 创建，不让客户端自由设置额度、分组、模型限制。
- `POST /api/token/:id/key`、`POST /api/token/batch/keys`：应限制为首次创建返回一次；现有接口可重复取完整 Key。
- OAuth 登录：建议包装成桌面授权流程，至少支持自定义 scheme/deep link 或 loopback redirect；最好新增 PKCE/OIDC 原生桌面流程。
- `GET /api/pricing`：只用于展示价格，不用于权限判断。

### 禁止 Nova 调用

- `/api/channel/*`，尤其 `/api/channel/:id/key`、渠道测试、余额更新、上游模型拉取。
- `/api/option/*`、`/api/ratio_sync/*`、`/api/openrouter_sync/*`、`/api/model_availability/*`、`/api/models/*` 管理接口。
- `/api/user/manage`、`/api/user/:id`、用户批量/管理员接口。
- `/api/redemption/*` 管理接口、支付 webhook、部署接口。
- 数据库直连、Redis 直连、上游 provider Key。

### 是否需要 Nova 专用授权服务

需要。理由：

- 现有 Web Session + `New-Api-User` 不是桌面设备授权协议。
- Token 完整密钥可重复取回，不符合桌面设备 Token 的“首次显示一次”预期。
- 缺少设备唯一性、设备撤销、设备 last_seen、客户端版本、风险审计。
- OIDC 当前未支持 PKCE，普通桌面 OAuth 安全性不足。

### 推荐 MVP 接入方案

1. Nova 打开 SDKMAX 登录页或内嵌 WebView 完成现有登录。
2. 登录成功后调用 `GET /api/user/self` 获取 `id`、`username`、`email`、`group`、`status`、`quota`。
3. 调用新增包装接口 `POST /api/nova/device-token`，传 `device_id_hash`、`device_name`、`platform`、`nova_version`。服务端幂等创建 `name=nova-desktop:<device_id_hash_prefix>` 的 Token，只首次返回完整 `sk-***REDACTED***`。
4. Nova 将完整 Token 存入系统安全凭据库，不写明文日志。
5. 调用 `GET /v1/models` 获取该 Token 实际可用模型。
6. AI 调用走 `/v1/chat/completions` 或 `/v1/responses`。
7. 轮询 `GET /api/usage/token/` 与 `GET /api/user/self`，遇到 401/403/余额不足错误立即刷新状态。
8. 提供“退出登录”和“撤销本设备 Token”。

### 可能需要修改的文件

只作为后续阶段建议，第一阶段不得实施：

- `D:\sdkmax\new-api\router\api-router.go`：新增 `/api/nova/*` 路由组。
- `D:\sdkmax\new-api\controller\nova.go`：新增设备 Token 包装控制器。
- `D:\sdkmax\new-api\model\token.go` 或新增 `model\nova_device_token.go`：设备幂等、审计字段。
- `D:\sdkmax\new-api\middleware\auth.go`：如需桌面专用授权中间件或更清晰的 UserAuth 包装。
- `D:\sdkmax\new-api\oauth\oidc.go` 与 `controller\oauth.go`：若要支持 PKCE/loopback/deep link。
- `D:\sdkmax\new-api\docs\openapi\api.json`：补充 Nova 专用接口文档。

### New API 核心文件必须禁止随意修改

除非有专项设计和回归测试，不应改动：

- `D:\sdkmax\new-api\model\user.go`
- `D:\sdkmax\new-api\model\token.go`
- `D:\sdkmax\new-api\model\ability.go`
- `D:\sdkmax\new-api\middleware\auth.go`
- `D:\sdkmax\new-api\middleware\distributor.go`
- `D:\sdkmax\new-api\relay\*`
- `D:\sdkmax\new-api\service\channel_select.go`
- `D:\sdkmax\new-api\relay\helper\price.go`
- `D:\sdkmax\new-api\setting\ratio_setting\*`
- `D:\sdkmax\new-api\service\pre_consume_quota.go`
- `D:\sdkmax\new-api\service\quota.go`

这些文件承载认证、Token 校验、分组/渠道选择、倍率、预扣费、结算和上游容灾核心。

## 接口分级清单

### 可直接复用

| 接口 | 鉴权 | 用途 |
|---|---|---|
| `GET /api/user/self` | Session + `New-Api-User` | 账号状态、余额、用户资料 |
| `GET /api/user/models` | Session + `New-Api-User` | 账号可见模型 |
| `GET /api/user/self/groups` | Session + `New-Api-User` | 可用分组 |
| `GET /api/user/logout` | Session | 退出 |
| `GET /v1/models` | Bearer API Token | Token 实际可用模型 |
| `POST /v1/chat/completions` | Bearer API Token | AI 对话 |
| `POST /v1/responses` | Bearer API Token | Responses API |
| `GET /api/usage/token/` | Bearer API Token | Token 用量轮询 |

### 需要包装

| 接口 | 问题 | 包装建议 |
|---|---|---|
| `POST /api/token/` | 客户端可控额度/分组/模型限制；无设备幂等 | `POST /api/nova/device-token` |
| `PUT /api/token/` | 可更改敏感 Token 属性 | Nova 只允许改设备名/撤销 |
| `DELETE /api/token/:id` | 可复用但缺少设备语义 | 包装为撤销指定设备 |
| `POST /api/token/:id/key` | 完整 Key 可重复获取 | 禁止 Nova 直接调用 |
| `GET /api/oauth/:provider` | Web Session 回调，不是原生桌面授权 | 增加 PKCE/deep link/loopback |
| `GET /api/pricing` | 价格展示不等于可调用权限 | 只作为 UI 价格展示 |

### 禁止调用

| 接口 | 原因 |
|---|---|
| `/api/channel/*` | 渠道、上游 Key、路由核心 |
| `/api/option/*` | 系统配置/Root 权限 |
| `/api/ratio_sync/*`、`/api/openrouter_sync/*` | 倍率/上游同步核心 |
| `/api/models/*` 管理接口 | 模型元数据管理 |
| `/api/user/manage`、`/api/user/:id` 管理接口 | 用户管理权限 |
| `/api/redemption/*` 管理接口 | 兑换码管理 |
| 支付 webhook | 只允许支付平台回调 |

