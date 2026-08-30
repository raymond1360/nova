# SDKMAX -> Nova 接入风险清单

审计日期：2026-08-30  
范围：只读审计，未实施修改。

## 高风险

### R1. 现有 Token 完整密钥可重复取回

证据：`POST /api/token/:id/key` 返回 `token.GetFullKey()`，见 `D:\sdkmax\new-api\controller\token.go:80` 到 `D:\sdkmax\new-api\controller\token.go:94`；批量接口也返回完整 key，见 `D:\sdkmax\new-api\controller\token.go:337` 到 `D:\sdkmax\new-api\controller\token.go:357`；`GetFullKey()` 直接返回数据库中的 `Key`，见 `D:\sdkmax\new-api\model\token.go:51`。

影响：如果 Nova 直接使用这些接口，桌面端或被劫持 Session 可重复导出 API Key，不符合设备 Token 最小暴露原则。

建议：Nova 禁止直接调用完整 Key 接口。新增 `POST /api/nova/device-token`，只在首次创建时返回完整 Key，后续只返回脱敏值。

### R2. Web Session + New-Api-User 不等于桌面设备授权

证据：`UserAuth()` 依赖 Session，并强制 `New-Api-User` 与 Session ID 一致，见 `D:\sdkmax\new-api\middleware\auth.go:36` 到 `D:\sdkmax\new-api\middleware\auth.go:123`；前端从 localStorage `uid` 注入该头，见 `D:\sdkmax\new-api\web\default\src\lib\api.ts:149` 到 `D:\sdkmax\new-api\web\default\src\lib\api.ts:167`。

影响：桌面端需要同时维护 Cookie 与 UID，本质仍是 Web 控制台登录态，缺少设备绑定、设备撤销、Refresh Token 轮换和授权审计。

建议：MVP 可临时复用 Web 登录，但必须尽快引入 Nova 专用授权/换票服务。

### R3. OIDC 用户登录未发现 PKCE 或设备授权

证据：OIDC token exchange 使用 `client_id`、`client_secret`、`code`、`grant_type=authorization_code`、`redirect_uri`，未设置 `code_verifier`，见 `D:\sdkmax\new-api\oauth\oidc.go:51` 到 `D:\sdkmax\new-api\oauth\oidc.go:75`。

影响：原生桌面客户端如果直接走公共客户端 OAuth，安全模型不足；如果依赖服务端 Web 回调，又会遇到 Cookie、SameSite 和浏览器/WebView 会话隔离问题。

建议：后续增加桌面 PKCE + loopback/deep link，或 Web 登录后后端签发一次性设备授权码。

### R4. 普通 Token 创建接口允许客户端控制敏感属性

证据：`AddToken` 从请求 JSON 读取 `RemainQuota`、`UnlimitedQuota`、`ModelLimitsEnabled`、`ModelLimits`、`AllowIps`、`Group`、`CrossGroupRetry`，见 `D:\sdkmax\new-api\controller\token.go:167` 到 `D:\sdkmax\new-api\controller\token.go:222`。

影响：Nova 直接暴露该接口会把分组、额度、模型限制等策略面交给客户端 UI。虽然用户只能为自己建 Token，但这会绕开 Nova 设备 Token 的产品约束。

建议：包装接口由服务端固定策略：名称、分组、模型限制、过期时间、是否无限额度均由 SDKMAX 后端决定。

## 中风险

### R5. `nova-desktop` 设备幂等只能靠 Token 名称约定

证据：Token 模型没有设备字段或唯一索引，见 `D:\sdkmax\new-api\model\token.go:14` 到 `D:\sdkmax\new-api\model\token.go:31`。搜索接口可按名称查找，见 `D:\sdkmax\new-api\controller\token.go:48`。

影响：同一设备并发登录可能重复创建多个 Token；改名后失去设备识别。

建议：新增 `client_type`、`device_id_hash`、`last_seen_at`，或新增 Nova 设备表；MVP 至少使用事务内按 `name` 二次检查。

### R6. `/api/user/models` 与 `/v1/models` 语义不同

证据：`/api/user/models` 使用用户可用组和客户可见过滤，见 `D:\sdkmax\new-api\controller\user.go:531` 到 `D:\sdkmax\new-api\controller\user.go:555`；`/v1/models` 还应用 Token 模型限制和计费配置过滤，见 `D:\sdkmax\new-api\controller\model.go:208` 到 `D:\sdkmax\new-api\controller\model.go:267`。

影响：Nova 如果用 `/api/user/models` 作为最终调用列表，可能展示 Token 实际不可调用模型。

建议：账号页用 `/api/user/models`；调用选择器以 `/v1/models` 为准。

### R7. `/pricing` 不等于可调用模型

证据：`GetPricing` 过滤 pricing 和 group ratio，返回价格展示数据，见 `D:\sdkmax\new-api\controller\pricing.go:36` 到 `D:\sdkmax\new-api\controller\pricing.go:76`。

影响：价格表可能包含无可用渠道、不可见、被 Token 限制或缺少计费配置的模型。

建议：仅用于价格展示，不用于权限判断。

### R8. Session Cookie 当前 `Secure=false`

证据：Cookie 配置见 `D:\sdkmax\new-api\main.go:185` 到 `D:\sdkmax\new-api\main.go:190`。

影响：生产 HTTPS 与桌面 WebView 复用时安全姿态不理想。

建议：生产环境启用 Secure Cookie；Nova 侧不要长期依赖 Web Session Cookie。

### R9. `TokenAuthReadOnly` 允许禁用/过期/耗尽 Token 查询自身用量

证据：`TokenAuthReadOnly` 注释和实现说明不检查 Token 状态、过期和额度，但检查用户封禁，见 `D:\sdkmax\new-api\middleware\auth.go:210` 到 `D:\sdkmax\new-api\middleware\auth.go:269`。

影响：这适合状态解释，但如果误用于敏感接口会扩大已撤销 Token 可访问面。

建议：仅用于 `/api/usage/token/` 等只读状态接口；Nova 不要把它当作有效调用凭据判断。

## 低风险

### R10. 客户端无法知道最终上游渠道

证据：普通用户指定渠道会被拒绝，见 `D:\sdkmax\new-api\middleware\auth.go:425` 到 `D:\sdkmax\new-api\middleware\auth.go:431`；渠道选择在 `D:\sdkmax\new-api\middleware\distributor.go:104` 到 `D:\sdkmax\new-api\middleware\distributor.go:156`。

影响：Nova 不能展示“本次使用哪个渠道”的强事实，只能展示模型和请求状态。

建议：保持黑盒，SDKMAX 负责容灾；如需排障只暴露 request_id，不暴露 channel_id。

### R11. 管理接口很多，必须做客户端 allowlist

证据：`/api/channel/*`、`/api/option/*`、`/api/models/*` 等管理路由集中在 `D:\sdkmax\new-api\router\api-router.go`。

影响：桌面端误接入管理员接口会扩大运维风险。

建议：Nova SDK/API 客户端硬编码 allowlist，只实现报告中“可直接复用”和“包装后调用”的接口。

## 禁止改动清单

第一阶段禁止实施任何功能修改。后续阶段也应避免直接改动以下核心，除非有专项设计、回归测试和回滚方案：

- `D:\sdkmax\new-api\model\user.go`
- `D:\sdkmax\new-api\model\token.go`
- `D:\sdkmax\new-api\model\ability.go`
- `D:\sdkmax\new-api\middleware\auth.go`
- `D:\sdkmax\new-api\middleware\distributor.go`
- `D:\sdkmax\new-api\service\channel_select.go`
- `D:\sdkmax\new-api\relay\helper\price.go`
- `D:\sdkmax\new-api\relay\*`
- `D:\sdkmax\new-api\setting\ratio_setting\*`
- `D:\sdkmax\new-api\service\pre_consume_quota.go`
- `D:\sdkmax\new-api\service\quota.go`

## MVP 风险接受建议

可接受的 MVP 风险：短期使用现有 Web 登录完成用户身份确认，然后换取 Nova 设备 Token。  
不可接受的 MVP 风险：Nova 直接保存用户密码、直接暴露完整 Key 导出接口、直接调用管理员渠道/倍率/模型管理接口、绕过 SDKMAX 扣费和渠道选择。

