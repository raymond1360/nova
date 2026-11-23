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

## API-2离线任务闭环补充（2026-11-23，待独审）

现有CoreLoopView的API操作区调用原prepare/approve/execute/status/cancel。prepare新增allow_repair；最多一次整改由schema14的Task scope及不可删收据约束，不依靠组件内计数。approve仍需完整计划digest；前端以普通范围批准呈现，digest仅技术绑定。

- api_only_execute复用task_loop::execute_loop：有同范围预授权且首轮确有已验证问题时，自动准备新preimage+前轮自检/测试材料，校验相同配置/文件/测试后消费继承授权一次；否则停在待整改。异常、UNKNOWN、第二轮失败都不循环。新增外发文件、测试或配置须api_only_repair生成PREPARED并新批准。
- api_only_task_report(task_id)：只读返回目标、两轮Plan/收据、diff、实际测试、自检、次数、阻塞与决策事项；角色引用config_id，PM未配置，不绑ChatGPT。刷新/重开不执行。
- api_only_user_decision(id,accepted,note)：技术通过后通过原Task状态/record_user_acceptance/完成门禁提交用户决定。未测试、UNKNOWN、错绑定或交付后文件漂移拒绝；拒绝验收进入需用户处理，可在剩余一次额度内准备整改。
- api_only_cleanup_status：固定清理状态/计数，不返回Key或槽内容。轮换旧槽/失败CAS新槽清理失败持久记录；显式删除失败/并发冲突不报成功。
- FixedTest新增recipe；ApprovedTest保存实际program身份/hash、argv、cwd、显式environment及trust_notice。content为Python隔离只读断言；python_unittest固定-I -S -B -m unittest discover -s tests -p test_*.py；nova_vitest_api2固定Node执行apps/desktop/node_modules/vitest/vitest.mjs run src/apiTaskPanel.interaction.test.ts --maxWorkers=1 --minWorkers=1，cwd为apps/desktop。项目配方执行项目/依赖代码，可写文件/子进程/联网，不是沙箱；不自动装依赖。环境仅SystemRoot/WINDIR，不继承PATH、模型Key或供应商变量，不声称程序hash绑定全部依赖。

旧schema13收据原样保留；缺少新测试环境/argv的旧待执行计划保守拒绝，不能把新权限继承给旧批准。真实API、OS凭据及原生界面仍须另获具体授权验收。本包不实施PM自动派单或真实模型调用。

同范围用户反馈也可消费首次计划中的一次整改授权：user_decision(false)仅登记用户决定；前端随后明确调用原execute，后端重新核定原grant、期限、当前Task/根/文件、配置与测试后生成并消费继承批准。范围或preimage漂移不继承，必须新Plan/新批准。未发送的PREPARED/APPROVED计划可通过原cancel_core_loop取消释放串行槽，收据及Task scope保留；该取消计划不能重新执行，继续工作需新Task。Task发布失败保守UNKNOWN，不把技术收据冒充已可验收。
