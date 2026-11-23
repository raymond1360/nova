# Nova API Spec

## Tauri Commands

- `initialize_app()`
- `get_provider(providerId)`
- `save_provider({ provider_id, name, base_url, api_key })`
- `delete_provider_credential(providerId)`
- `fetch_models(providerId)`
- `test_provider_connection(providerId)`
- `fetch_token_usage(providerId)`
- `get_setting(key)`
- `set_setting(key, value)`
- `list_conversations()`
- `create_conversation(providerId, modelId)`
- `delete_conversation(conversationId)`
- `list_messages(conversationId)`
- `send_chat_message({ conversation_id, provider_id, model_id, content })`
- `cancel_active_chat()`

## SDKMAX Calls

`GET /v1/models`

Headers are created only in Rust:

```http
Authorization: Bearer [REDACTED]
```

Response shape:

```json
{
  "object": "list",
  "data": [{ "id": "model-id", "object": "model", "owned_by": "sdkmax" }]
}
```

`POST /v1/chat/completions`

```json
{
  "model": "model-id",
  "messages": [{ "role": "user", "content": "你好" }],
  "stream": true
}
```

SSE chunks use `data:` lines. Nova parses `choices[0].delta.content` and stops on `data: [DONE]`.

`GET /api/usage/token/`

Used only for display. Passing this endpoint does not prove chat calls are available; Nova combines it with `/v1/models` or a real chat request.

## 2026-11-23 API-only任务接口（后端首包，待独审）

当前任务入口为Tauri `api_only_*`；历史CLI任务启动/探测返回 `API_ONLY_ROUTE_REQUIRED`。此处不替代现行产品计划，不代表已完成用户界面或真实服务商验收。

调用顺序：

1. `api_only_catalog`读取七类供应商/协议及adapter_ready；`api_only_configs`仅返回配置元数据。
2. `api_only_save_config(config,key)`：config={id(UUID),provider,base_url,model}，Key另传临时字符串；Windows Credential Manager保存Key，SQLite只保存元数据及随机凭据槽引用。更换服务商/endpoint必须提供新Key。`api_only_delete_key(id)`删除所选凭据并更新版本。当前仅deepseek适配可执行，其余明确ADAPTER_NOT_IMPLEMENTED。
3. `api_only_test_connection(id,explicit_network)`是单独网络行为，false不发送；只查询models。reachable不代表development_verified。
4. `api_only_prepare(task_id,config_id,paths,tests)`冻结现有confirmed Task、完整需求/已有binding、供应商配置版本、最多3个已有UTF-8文件及固定测试。tests每项为{program,file,contains}，program是项目外明确批准的Python exe，实际参数固定为隔离只读内容断言；不执行项目模块或模型命令。返回完整Plan。
5. 按Plan的紧凑JSON保存字节计算SHA256（status返回同一digest），展示实际外发、费用、非原子写回及失败风险后，`api_only_approve(id,digest)`明确批准。不要重新序列化不同字段顺序来猜digest。
6. `api_only_execute(id)`：DEVELOP一次→严格提案/身份/preimage→持久原字节及intent→受保护写回/flush→固定测试→全新只读REVIEW上下文一次。取消复用`cancel_core_loop(task_id)`；取消/UNKNOWN不自动重发或写回。
7. `api_only_status(id)`返回变化/diff、真实测试收据、自检意见及phase。AWAITING_USER仅为技术通过待用户验收，NEEDS_REMEDIATION为待整改，UNKNOWN为需人工核对；process_liveness保持UNKNOWN，不能将收据当存活事实。Task不自动completed。

本包不提供自动整改、用户完成确认桥接或任意shell测试器。每个Task仅一个持久执行记录，UNKNOWN占用串行槽；重开不能绕过。多文件非原子，恢复只读核对，备份不能未经新授权自动覆盖文件。原字节与候选在本机执行收据中保留，Key不进入提示词/证据。生产使用仍须独审、必要CI、OS凭据及真实API另行具体授权验收。
