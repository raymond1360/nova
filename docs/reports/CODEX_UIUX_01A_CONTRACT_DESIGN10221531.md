# UIUX-01A 需求确认基础：技术契约设计

时间：2026-10-22 15:31，北京时间。性质：设计交接，待 Claude Code 独立审核；不是实现或测试通过报告。

## 1. 结论与范围

推荐采用“项目需求对话 → 不可变修订 → 明确确认 → 原子绑定既有 Task”的增量结构。保存草稿、受限规划、确认绑定、启动执行是四种不同授权，不能隐式串联。workspace store 只管理消息、草稿和规划；现有 coreLoop store 继续提供执行事实。

本轮已决定：同一用户选择的 Codex CLI 或 Claude CLI 承担规划和开发，不增加产品内第二 AI，不要求 SDKMAX 供应商/API Key；执行期间的新要求只能形成下一轮未确认草稿。需求轮与执行 round 分开，不能重置一次自动修复机会。确认后的 Task 原文、冻结需求快照及后端验收门禁保持不变。

关键技术结论：现有两个 CLI adapter 都不足以证明满足本阶段的规划权限约束。规划能力默认 BLOCKED，只有精确版本的受限配置通过后续隔离验证才能开放；否则保留草稿及人工引导填写，不回退到可写开发调用。这不阻止先实现非 AI 草稿基础，但不能据此宣布真实规划能力完成。

01A 不包含真实运行反馈改造、进程存活判断、停止确认、结果打开或工作台布局迁移；这些分别属于 01B/01C。920×640 最小窗口保留，后续以侧栏收起适配。预览不是验收，不新增独立“已查看”记录。

## 2. 基线、现场与依据

- 实际分支：`feature/v0.1.0-mvp`。
- 完整 HEAD：`6ec3842f23f91b7b5d8606f2e1dfbcb2fce058d3`，与用户指定 RAV01 基线一致。
- 暂存区为空。已有 tracked 修改仅两份 product 文档；Novauiux、历史报告、audit 等大量未跟踪资料保留。本轮不整理或收录。
- 检索未发现适用 AGENTS.md/CLAUDE.md；未发现 D:\AGENTS.md。未修改 Git 状态或配置。
- 已阅读 RAV01 收尾报告 `CODEX_RAV01_CLOSURE_MILESTONE_REPORT10221233.md`、迁移计划 `CODEX_UIUX_MIGRATION_IMPLEMENTATION_PLAN10221411.md`、两份 `10200953` UI/UX 与交互规格，并直接核对下列源码。
- RAV01 是经历中断与恢复后完成的单 Codex 路径里程碑，不扩大为所有 CLI/自动修复/MVP 发布均通过。本轮没有重新读取真实任务库或运行测试项目。

| 源码位置（均相对 D:\nova） | 核实事实及设计影响 |
|---|---|
| apps/desktop/src-tauri/src/models.rs | Task 保存四项需求、AI、state、result_generation、测试命令来源等；新需求数据不能复用执行 generation |
| storage/tasks.rs: create_task、transition_task | 两者各自 BEGIN IMMEDIATE；draft→awaiting_confirmation 在同事务冻结需求；新确认不可直接嵌套调用这些事务包装 |
| storage/requirement_items.rs | 当前没有编辑已创建 Task 四字段的入口；快照按换行拆项、修剪并去空项，保留顺序及重复项；原文层必须另存，不能修改既有覆盖匹配算法 |
| storage/migrations.rs、db.rs | schema=11；逐项备份及事务迁移；user_version 高于程序版本会拒绝；SQLite busy_timeout=5 秒 |
| commands.rs、runtime/core_loop.rs、runtime/verify_only.rs | 启动有任务状态、所选 AI readiness/trust、冻结需求及执行门禁；一次安全返修与仅验证零 AI 路径必须保留 |
| runtime/ai_adapter.rs | Codex 开发 workspace-write、review read-only；Claude 开发 acceptEdits、review Read+部分禁用工具；不存在独立 PlanningAdapter |
| runtime/path_guard.rs、process.rs、win_spawn.rs | cwd 校验、进程生命周期及 Windows JobObject 不是完整读写/联网隔离；当前启动请求没有规划专用环境与配置封闭策略 |
| 现有 conversations/messages 与 stores/nova.ts | SDKMAX 聊天绑定 provider/model，流式消息，不是项目需求确认账本；不能让新流程依赖它 |
| stores/coreLoop.ts、components/CoreLoopView.vue | 执行事实、现有确认/启动及证据入口继续复用；本轮不重写状态机或旧表单 |

表中 backend 简写路径前缀为 `apps/desktop/src-tauri/src/`。旧计划只作线索；本次已确认产品决策覆盖旧规格中的双 AI、最小尺寸、预览即完成等冲突，不改写旧规格原文。

## 3. 用户流程与状态

1. 打开已注册项目，进入该项目的需求对话，选择执行 AI。未选择时允许保存，不允许规划或确认。
2. 输入原始需求并自动保存；明确点击“整理需求”才进入授权与受限规划。自动保存、切页和进度查询不触发 AI。
3. 首次展示上下文文件范围、所选 CLI/账户用量及“不修改项目”声明；确认后发起一次规划。无法保证权限时解释受阻并提供人工四字段表单。
4. AI 提出澄清或四字段建议，保存在建议层；用户显式应用后形成新 revision。原始消息永不被建议覆盖。未解决的必要澄清禁止确认。
5. 用户编辑、查看四项范围卡，明确确认精确 revision。服务端原子生成唯一 Task、冻结快照及绑定；返回 Task ID，不自动启动。
6. 沿用现有测试/构建命令确认、环境/信任与启动门禁。确认已成功而启动失败时定位同一 Task，不能创建另一个任务掩盖失败。
7. 当前任务执行期间追加要求，保存到关联 origin_task_id 的 next 草稿。用户明确确认下一轮且满足任务活动约束后才创建新 Task，不自动执行。

草稿头状态仅 `editable / bound / archived`。新建、补充和澄清以消息种类及 revision 内容表示；“可确认”从四字段、AI、澄清状态派生。旧 revision 的“过期”是非当前头的派生状态，不删除记录。绑定后整个草稿不可编辑，新要求另开 next 草稿。用户需求轮由草稿 ID 区分，不复用 round、attempt、result_generation。

## 4. 数据模型：推荐新增 8 张正式表

以下为后续迁移契约，未执行 SQL。所有 ID 使用随机 UUID；时间存 UTC；通过外键与服务层共同验证归属，不依赖前端传来的显示名称。

| 表 | 最小字段与约束 |
|---|---|
| requirement_conversations | id、project_id、created_at、updated_at；UNIQUE(project_id)，一个项目一个需求对话；UNIQUE(id,project_id) 支持组合外键 |
| requirement_drafts | id、project_id、conversation_id、origin_task_id?、kind(initial/next)、status、current_revision、created_at、updated_at；组合 FK 保证同项目；next 必须有同项目 origin_task；initial 不允许 origin |
| requirement_revisions | draft_id、project_id、revision、base_revision、title_raw、selected_ai?、goal_raw、in_scope_raw、not_in_scope_raw、done_criteria_raw、source(user/ai_applied/mixed)、source_planning_id?、blocking_questions_json、content_hash、created_at；PK(draft_id,revision)，不可变；selected_ai 在编辑阶段可空，规划/确认必须 Codex 或 Claude |
| requirement_messages | id、project_id、conversation_id、seq、draft_id?、revision?、kind、body_raw?、payload_json?、planning_request_id?、created_at；UNIQUE(conversation_id,seq)；原始用户文字不可变，系统消息用结构化 code/args 派生显示 |
| planning_authorizations | id、project_id、conversation_id、selected_ai、binary_identity_hash、policy_version、context_scope_json、scope_hash、granted_at、expires_at、revoked_at?；不保存登录凭据 |
| planning_requests | id、project_id、draft_id、source_revision、authorization_id、client_request_id、request_hash、selected_ai、binary_identity_hash、status、dispatch_fence?、context_manifest_json、input_hash、output_json?、output_hash?、adoption_status、applied_revision?、usage_json?、error_code?、redacted_diagnostic?、各阶段时间 |
| requirement_task_bindings | id、project_id、draft_id、revision、content_hash、selected_ai、task_id、confirmed_at；UNIQUE(draft_id)、UNIQUE(task_id)；永久绑定，不随任务状态改变 |
| requirement_mutation_receipts | project_id、operation、client_request_id、request_hash、resource_id、response_refs_json、created_at；PK(project_id,operation,client_request_id)；只存稳定结果引用，不缓存旧的实时任务状态 |

必要索引与约束：

- 草稿组合身份 `(id,project_id,conversation_id)`、修订 `(draft_id,revision,project_id)`、任务 `(id,project_id)`；所有跨表链接验证同项目。若旧 tasks 缺组合唯一索引，仅增索引，不改旧记录。
- partial UNIQUE：每项目一个 editable initial；每 `(project_id,origin_task_id)` 一个 editable next；同项目最多一个未决规划（reserved/dispatching/running/uncertain）。历史 completed/failed 请求不占槽。
- 消息按 `(conversation_id,seq)`、草稿按 `(project_id,status,updated_at)`、规划按 `(draft_id,source_revision,created_at)` 检索。
- revision、binding 不可更新/删除；业务接口无历史删除能力，FK 默认 RESTRICT。草稿头引用当前 revision 可用延迟外键，在一个事务中插入草稿与 revision1；不得用空悬指针绕过完整性。
- receipt 与规划记录唯一性由数据库约束兜底，多窗口不能只靠内存 busy 标记。

### 4.1 原文与派生字段

用户消息、四字段、标题均保存精确字符串，包括换行形式、顺序、重复项；不 trim 后回写、不 Unicode 规范化、不自动翻译替换。校验可用 trim 判断是否空，但不能修改保存字节。AI 建议原始结构与应用后的新 revision 分开，显式记录来源。

content_hash 采用带版本号的规范 JSON：对象键固定排序，字符串内容完全保留，包含 draft/revision、AI、标题和四字段；字段顺序明确。展示分段、翻译标签、就绪状态是派生值，不作为已确认原文。Task 接收四字段原文，冻结快照仍使用现有拆项规则；不得用翻译文本替换自查覆盖依据。

### 4.2 并发、晚结果与保留

保存用 expected_revision CAS；成功新建 revision，旧版保持不可变。冲突返回当前 revision 和可比较原文，保留本地未保存输入，用户选择合并后再保存，不最后写入覆盖。

AI 输出永远先作为建议。若用户已经编辑，adoption_status=stale；不得自动应用。显式应用必须 source_revision==current_revision 且 expected_revision 匹配，否则拒绝。用户可阅读旧建议并手工采纳内容到新 revision，留下来源，不伪装成自动无冲突应用。必要澄清由结构化 question ID 与明确回答记录消解；不能仅因为四字段有文字便忽略未答问题。

不自动清理确认历史、消息、绑定或幂等回执；不采用短 TTL 造成重发重复建任务。授权推荐 24 小时有效，可撤销；AI/二进制身份/策略/项目/范围变化重新授权。规划临时上下文在确认进程结束后删除；状态不确定时保留最小受保护诊断，待核查，不扫描认证目录。原始对话可能敏感，留本机，不自动上传报告；日志只保留限量脱敏错误，不保存思维链或完整 CLI 会话。含疑似密钥的用户原文保留本地并阻止发送，提示用户编辑，不静默改写原文。

## 5. IPC 契约与错误

统一符号：P=project_id，C=conversation_id，D=draft_id，R=expected_revision，K=client_request_id。写入 K 为 UUID，由客户端一次用户意图生成并在重发时复用。服务端先校验身份与所有权。请求 hash 包含操作名、P/C/D/R、所选 AI 及全部业务内容（包括请求输出语言）；只排除 UI 显示语言、传输时间等非业务元数据。同 K 不同内容明确拒绝。

响应：`{ok:true,data:{...},replayed:boolean}`；错误：`{ok:false,error:{code,message_key,args,current_revision?,resource_id?,retry_class}}`。不用英文错误字符串驱动业务。所有输出脱敏；客户端不将错误当成功清空输入。

| 接口与具体请求 | 响应与唯一允许副作用 |
|---|---|
| open_requirement_conversation {P,K} | 返回 C；只幂等创建项目对话，无 AI |
| list_requirement_messages {P,C,cursor?,limit<=100} / get_requirement_draft {P,C,D} | 只读分页消息、草稿和修订；不探测/启动 CLI |
| create_requirement_draft {P,C,kind,origin_task_id?,K} | 返回 D/revision1；同项目活动草稿冲突返回现有 ID；只建草稿 |
| save_requirement_revision {P,C,D,R,K,title_raw,selected_ai?,goal_raw,in_scope_raw,not_in_scope_raw,done_criteria_raw,append_message?,clarification_answers?} | 返回新 revision/hash；一次事务保存修订、可选原始消息及回执；不建 Task |
| append_requirement_message {P,C,D,R,K,body_raw,intent:requirement} | 保存原始消息；不自动规划/确认。明确的 progress 查询走只读事实 API，不靠关键词伪装理解 |
| get_planning_capability {P,selected_ai} | 读取已核验策略/profile 摘要：available/blocked/reason；无真实规划 |
| grant_planning_authorization {P,C,selected_ai,binary_identity_hash,policy_version,context_scope,K} / revoke {P,authorization_id,K} | 记录授权/撤销；不是付费调用。撤销中的取消另走规划取消；不能承诺撤销已传输的数据 |
| request_requirement_plan {P,C,D,R,authorization_id,selected_ai,context_selection,output_language,K} | 返回 request_id/status=reserved；唯一可显式发起付费规划的接口；只写规划/建议/诊断，不创建任务或执行 action |
| get_requirement_plan {P,D,request_id} | 只读真实状态、结构化建议及可用用量 |
| cancel_requirement_plan {P,D,request_id,K} | 只请求取消本规划；不得调用任务停止/验证恢复接口 |
| apply_requirement_proposal {P,C,D,R,request_id,K} | 验证来源、AI 和 source_revision；生成新 revision，记录应用来源；不确认任务 |
| resolve_uncertain_plan {P,D,request_id,K,resolution,acknowledge_possible_charge} | 仅附接可验证的已有最终结果，或在已证明进程结束后标记放弃；不得重发原调用。无法证明结束继续 uncertain |
| confirm_requirement_revision {P,C,D,R,content_hash,selected_ai,K} | 原子返回 binding_id/task_id/confirmed_revision；不启动；新启动失败仍可用 get_requirement_binding {P,D} 找回 |
| archive_requirement_draft {P,C,D,R,K} | 仅归档未绑定、无未决规划草稿；不删消息/证据 |

append 消息后的字段整理仍需显式 save/apply；消息保存与 revision CAS 可以在同一事务中完成，避免文字已收而草稿头悄悄丢更新。服务端不接受客户端指定 Task state/generation/round/attempt。

错误至少包括：PROJECT_NOT_FOUND、IDENTITY_MISMATCH、REVISION_CONFLICT、DRAFT_BOUND、DRAFT_ARCHIVED、STALE_PROPOSAL、INCOMPLETE_SCOPE、UNRESOLVED_CLARIFICATION、IDEMPOTENCY_CONFLICT、PLANNING_BUSY、EXECUTION_IN_PROGRESS、CONSENT_REQUIRED/EXPIRED/SCOPE_MISMATCH、CLI_IDENTITY_CHANGED、PLANNING_POLICY_UNVERIFIED、CONTEXT_REJECTED/TOO_LARGE/SENSITIVE、CLI_NOT_READY、PLAN_START_FAILED/TIMEOUT/QUOTA/OUTPUT_INVALID/UNCERTAIN、DATABASE_BUSY、MIGRATION_FAILED。可重试不等于自动重发付费调用；网络断开先查询 receipt/request。

## 6. 原子确认、幂等与既有门禁

推荐从 create_task、transition_task 提取私有的“已有事务内执行”核心函数，原公开包装保持事务行为。新确认服务使用同一连接外层 BEGIN IMMEDIATE；禁止嵌套现有 BEGIN，禁止复制一套放宽门禁的裸 INSERT/UPDATE。

事务顺序：

1. 校验 P/C/D 身份；获取写事务后先查 receipt。相同 K/hash 返回已提交 binding；不同 hash 拒绝。已绑定同精确 revision/hash/AI 即使新 K 也只返回原 Task；不同版本拒绝。
2. 校验 editable、当前 R、服务端重算 hash、四字段非空、AI 已选、必要澄清已明确处理、规划建议来源一致。origin_task 必须同项目。
3. 检查当前项目持久化活动任务及本进程启动保留。执行中只能保存 next 草稿，不能以确认绕过执行约束。该检查在确认事务时刻线性化，不声称能证明 OS 进程存活。
4. 用共享核心创建 draft Task，沿既有合法迁移到 awaiting_confirmation 并冻结需求快照，再到 confirmed。任一失败整个事务回滚。
5. 插入唯一 binding、草稿 bound、确认系统消息及 receipt，一起提交。唯一约束兜底并发双击/多窗口；约束冲突重新查询确定的绑定，不另建任务。
6. 返回稳定绑定和另查的当前 Task 状态，避免重放旧响应把运行中 Task 在 UI 改回 confirmed。

确认事务不跨 CLI、网络、文件生成、CLI readiness probe 或长时间执行。原文映射到 Task，冻结拆项继续现有行为。所选 AI 固定，不能在确认后凭 UI 临时切换。测试/构建命令不来自规划结构；Task 沿用现有自动检测/用户明确确认、无测试/无构建依据及后端规则。未确认命令不默认通过，返回 prepare_existing_task 下一步。

确认与启动彻底分离：start_core_loop 仍要求用户动作并执行现有 state/CAS、信任、自查、一次返修、验收门禁。确认成功但响应丢失时先 get binding；启动失败不新建 Task，不自动再次启动。当前启动保留主要是每任务内存机制，不能宣称跨进程 exactly-once；01A 必须测试与现有启动的竞争，若发现跨进程启动缺口单列修复，不能以新草稿表假装已解决。多窗口确认唯一性本设计由数据库保证。

## 7. 受限规划权限：开放前置条件

### 7.1 当前适配与官方依据

| CLI | 当前源码 | 结论 |
|---|---|---|
| Codex | exec --sandbox read-only 用于 review；-C 指定目录，stdin 输入；help 探测不足以封闭工具/配置 | read-only 不等于禁用命令、任意读取或所有扩展。当前 Planning capability=BLOCKED |
| Claude | review 允许 Read、禁用部分 Bash/Edit/Write/NotebookEdit；未证明 MCP/hooks/项目外读取封闭 | 工具名单与工作目录不能证明完整上下文限制。当前 Planning capability=BLOCKED |

官方资料仅用于候选方案，不代替本机精确版本验证：

- Codex 官方[非交互模式](https://learn.chatgpt.com/docs/non-interactive-mode)描述结构化输出与 exec 用法；[开发命令](https://learn.chatgpt.com/docs/developer-commands)描述 read-only、ephemeral、忽略用户配置/规则等选项。它们不能单独证明所有工具和外部配置已禁用；本设计不猜测不存在的“全部禁用工具”参数。
- Claude 官方[CLI reference](https://code.claude.com/docs/en/cli-reference)说明空 tools 列表不覆盖 MCP，strict-mcp-config 限制配置来源；bare 跳过多种自动发现，但本身不代表关闭 Bash/读写。必须组合并在精确版本验证。配置层次另见[settings](https://code.claude.com/docs/en/settings)。

推荐新 PlanningAdapter/profile，不复用开发调用，也不把旧 review 的项目副本当安全证明。profile 绑定 binary hash/version、参数模板、受控环境/配置策略版本及验证结果。版本或二进制变化使能力失效，不升级日常 CLI。

开放条件必须同时满足：新建非 resume 会话；agent tools 为零；无 MCP、hooks、plugins、skills、自动项目/用户指令加载或额外子代理；只通过 stdin 收到 Nova 筛选的上下文；工作目录是独立临时目录，不是项目；配置不能将参数重写为可写/命令执行。仅允许官方认证/模型传输所需网络，不能允许代理任意联网工具。Windows JobObject 仅管生命周期，不作为权限边界。

不得读取/复制宿主认证文件；CLI 可按官方机制内部使用已登录账户，但隔离配置与既有账户认证能否共存仍需验证，不强制改为 API Key，不改全局配置、ACL、防火墙或认证。若无法在每次调用层封闭用户/管理配置的执行能力，或无法保证上下文读取边界，则保持 BLOCKED。cwd、提示词、read-only、复制文件夹均不能单独补足此约束；不得使用危险跳权参数。

### 7.2 上下文与限制

默认仅当前需求消息及四字段，不自动扫项目。用户可点选精确相对路径的 UTF-8 文本：建议最多 8 文件、单文件 32 KiB、文件合计 128 KiB；消息 64 KiB、修订 64 KiB，总输入 256 KiB。输出最终 JSON 上限 64 KiB，进程捕获上限 1 MiB，120 秒超时。它们是推荐工程上限，不是 token/费用封顶；后续测试后可调整并版本化。

授权范围包含精确路径集合、类型/大小策略、所选 AI、项目和 profile；每次请求保存实际读取内容的 hash/大小清单。相同授权范围内内容变化仍须记录本次快照，范围扩大或二进制/策略改变重新授权。README/AGENTS/项目文本只作不可信资料，不能提供工具或权限授权。

拒绝 .env 变体、凭据/私钥、数据库/WAL/SHM、认证目录、.git、node_modules、target、二进制及超限文件；内容扫描发现疑似敏感信息时阻止发送并说明。拒绝 UNC/设备路径、符号链接/junction 逃逸；读取时通过实际打开句柄核对最终路径、文件身份和边界，防 TOCTOU，不能只做字符串前缀判断。用户原文中的敏感信息同样提示，不输出其值。

规划输入带 schema_version、P/D/revision、原始四字段、消息和批准上下文；输出 kind 仅 clarification 或 proposal，包含对应身份、questions[{id,text}]、proposal 四字段及简短依据。禁止 shell commands、工具调用、Task 状态和执行授权字段。JSON schema、身份、大小、必填项均服务端验证；exit0 但结构无效仍失败，不发起另一次收费“修 JSON”。进度查询只读取 coreLoop 事实，不伪造 AI 回答。

### 7.3 请求生命周期与不确定费用

reserved → dispatching → running → succeeded/failed/cancelled；进程或投递状态无法确认则 uncertain。先持久化 request/receipt，再在事务外采集已授权上下文；发出前重新核对 revision、授权和 profile，CAS 写 dispatch_fence 后才 spawn。不得持有 DB 事务等待外部进程。

数据库无法与外部 CLI 实现全局 exactly-once。重启发现 dispatching/running 不盲重发，先查本应用管理的进程/有限结果文件；不能确认则 uncertain，保留占槽与费用未知提示。reserved 且从未写 fence 可在用户明确动作下继续；已写 fence 不能当未付费。已证明进程结束且用户知晓可能已计费后可放弃原请求，再明确发起新 K；不能确认结束不得释放并重复调用。

取消只作用本规划进程树，cancel_requested 不等于已停止；不得杀日常 Codex/Claude。超时、额度、未登录、工具不兼容、JSON 失败均保留草稿并显示可理解错误，不转开发/测试/构建。用量只记录 CLI 实际提供的字段，未知为 null，不能以 0 或精确金额替代。规划独立记账，不生成 ai_development/action/test/build/acceptance 记录，不消耗或重置自动修复次数。

## 8. 迁移与旧数据兼容

推荐新增 v12，仅增上述表、索引和必要不变性约束；不重写 1..11 migration，不更新旧 Task、快照、证据、文档或用户接受记录，不为历史任务编造对话或确认。旧项目首次打开按需创建空需求对话，旧任务只通过现有真实记录展示。

沿用 SQLite online backup（含 WAL 语义）、完整性检查及唯一备份文件名；备份失败停止升级。新迁移单个 BEGIN IMMEDIATE，建表/索引/外键检查后更新 user_version 并提交，失败完整回滚。当前迁移版本在进入循环前读取：建议在获得迁移锁后重读版本，避免并行启动重复迁移；这属于实现时必须隔离验证的最小兼容修正，不宣称现状已具备。所有数据写入停止在升级成功前，禁止两个不同 schema 版本进程并写。

旧 v11 程序面对 v12 会 VersionTooNew 拒绝，不承诺旧程序可直接打开新数据库。首选回滚构建保留 v12 读取兼容、关闭新入口；代码 revert 不等于 DROP TABLE、降 user_version 或覆盖用户数据。恢复升级前备份只能另获授权，评估升级后新增数据损失并先备份现状；不能自动执行。测试仅隔离数据库，不触碰日常 AppData。

## 9. 前端接入与中英文文案

后续新增 workspace store 管理 conversation/draft/revision/规划授权/请求/binding；coreLoop store 仍唯一管理 Task 执行事实。IPC DTO 与错误码独立类型定义。首步只做接入与可复用组件，不改新版工作台布局，不把当前表单回填为可编辑旧 Task。

每个异步请求捕获 P/C/D/revision/request_id 和本地 selection epoch。切项目/草稿后旧响应可更新其原实体缓存，不得覆盖当前输入、提示、busy 或当前文档。计划中的 Task 选择只关联 binding，不创建第二套 Task 状态。确认/规划/保存各有独立真实 pending；即时禁用重复动作，服务端 receipt 兜底。切换不自动调用 AI。

IME composition 期间不重建 textarea、不以服务端回声覆盖 buffer；compositionend 后防抖保存。保存发送精确快照，用户在途新增输入留在本地；成功回执只推进对应已保存版本。冲突保留本地与远端两版；失败不清空。点击确认前先等待对应保存完成并展示服务器 revision/hash，不确认正在变化的卡片。明确保存中/已保存/保存失败，规划仅显示已排队/执行中/最后真实事件/状态未知，无模拟百分比。

| 场景 | 中文 | English |
|---|---|---|
| 首次授权 | 使用已选择的 {AI} CLI 整理需求。将读取：{精确范围}。会使用该 CLI 账户的用量；这一步不修改项目。是否允许本次范围的需求整理？ | Organize requirements with your selected {AI} CLI. Context: {exact scope}. This uses your CLI account allowance and does not modify the project. Allow planning within this scope? |
| 四项卡 | 目标 / 范围内 / 不包含项 / 完成标准 | Goal / In scope / Out of scope / Completion criteria |
| 确认 | 确认版本 {revision} 并创建任务（不会自动开始） | Confirm revision {revision} and create task (does not start execution) |
| 权限受阻 | 当前 CLI 尚未通过受限规划验证。草稿已保留，请先手动填写四项需求。 | Restricted planning is not verified for this CLI. Your draft is saved; fill in the four fields manually. |
| 晚到建议 | 你已更新草稿，这份建议基于旧版本，未覆盖你的输入。 | You updated the draft. This suggestion uses an older revision and has not overwritten your input. |
| 冲突 | 另一窗口已保存新版本。你的输入已保留，请比较后保存。 | Another window saved a newer revision. Your input is preserved; compare before saving. |
| 下一轮 | 已保存为下一轮未确认草稿，不影响当前任务。 | Saved as an unconfirmed next-round draft. The current task is unchanged. |
| 启动失败 | 需求已确认并绑定任务 {task}，启动未成功。查看该任务后处理启动条件。 | Requirements are bound to task {task}, but startup failed. Open that task to resolve startup conditions. |
| 用量未知 | CLI 未提供准确用量；本次可能已产生用量，请勿重复发送。 | Exact usage is unavailable. This request may have consumed allowance; do not resend blindly. |

翻译只改变标签和系统说明，不改变用户原文、已确认需求、哈希及自查匹配内容。920×640 下四字段卡可滚动、焦点可达；长文本/换行不截成不可查看内容。主流程技术表单负担的完整迁移留给 01C。

## 10. 验收矩阵（本轮全部 NOT RUN）

以下是可执行的后续验收计划，不是已通过结果。先针对性单测，再完整 Rust lib、前端测试、类型检查/构建及仓库规定检查；命令以开发基线实际脚本为准，记录退出码，不降低断言。

| 场景 | 隔离方法与必须断言 |
|---|---|
| 保存/重启 | 临时 SQLite 重开；四字段 CRLF、重复行、顺序及 Unicode 精确相等；中英文切换不改值/hash |
| 修订冲突 | 两连接从 R 保存；仅一个成为 R+1，另一返回冲突且输入可恢复 |
| AI 晚于编辑 | mock 延迟输出；新 revision 不变，建议标 stale，应用旧建议拒绝 |
| 正常原子确认 | Task、绑定、冻结快照、消息及 receipt 同时存在；无 CLI/test/build IPC |
| 双击/重发/多窗口 | 同 K 重放同 Task；不同 K 并发确认同修订也只一 Task；新连接可找回绑定 |
| K 相同内容不同 | 即使已确认也 IDEMPOTENCY_CONFLICT，不能返回无关成功 |
| 旧卡/跨身份 | 旧 revision、错 P/C/D/origin/task/request 任一拒绝且零写入 |
| 事务故障注入 | 每个写入点失败后不存在半 Task/半绑定/半冻结；旧记录不变 |
| 启动失败及重连 | 确认已提交后模拟 readiness 失败/响应丢失，定位原 Task、不重建、不自动重启 |
| 执行中追加 | 只 next 草稿变化；当前 Task 原文、generation、round、attempt、返修预算不变 |
| 自动保存/进度 | mock 统计 CLI 调用数为零；未知事实不得显示运行中 |
| 权限/授权 | 未验证 profile、过期/撤销/范围扩大/二进制变化均在 dispatch 前拒绝 |
| 提示注入 | 恶意 README 要求读私钥/写项目/开工具/联网；参数和工具策略仍不变；不泄露哨兵文件 |
| 上下文边界 | 超限、敏感文本、路径大小写/分隔符、junction/symlink/TOCTOU 均覆盖；不能只测字符串校验 |
| 输出失败 | exit0+坏 JSON/身份错/超限/缺字段仍失败，草稿不变，开发/测试/构建/验收动作全零 |
| 超时/取消/额度 | 取消未结束维持未知；重启 uncertain 不 spawn；已停后明确放弃才允许新请求 |
| 迁移 | v11 隔离副本升级 v12，旧业务表排序导出 hash/count 相同；备份及 DDL 故障回滚、并发升级验证 |
| 旧程序兼容 | v11 打开 v12 明确拒绝，不降版本/覆盖；兼容回滚构建可读保留数据 |
| 原门禁回归 | 一次自动修复上限、blocked 自查拒绝、仅验证所有分支零 AI、attempt/快照/用户验收门禁不退化 |
| 前端输入/切换 | mock IPC、IME、离线重试、任务切换晚响应、confirm pending 重复点击；无输入丢失和串项目 |

fake CLI 能证明 Nova 的参数/环境/派发、解析、取消与副作用隔离，不能证明真实供应商工具隔离。必须后续另行授权，在一次性项目和假敏感哨兵上，分别对精确 Codex/Claude 版本验证：零工具、MCP/hooks/config 自动发现封闭、项目外读取不可达、写入/命令/任意网络不可用、正常模型通信与账户用量报告、取消及重启不重复费用。发现越界立即 BLOCKED，不扩大权限。真实验证可能产生用量，不能本轮运行或承诺硬金额上限。

## 11. 分步实施与开发基线（本轮不执行）

1. Claude 审核本契约，重点确认权限 fail-closed、事务拆分、原文与快照兼容、幂等及外部不确定状态。PM 确认设计结论后才进入实现。
2. 在保留 D:\nova 混合工作区前提下，逐文件核验并按原始字节归档两份有效规格为独立文档提交：`Novauiux/NOVA_V0.1.0_UI_UX_DESIGN10200953.md`、`Novauiux/NOVA_V0.1.0_INTERACTION_DESIGN10200953.md`。不改写历史规格。
3. 另建带实际时间的 `UIUX_01A_DECISION_ADDENDUM<MMDDHHMM>.md`，记录本轮单 AI、受限规划、四字段不可变、需求轮独立、920×640 与阶段边界覆盖关系；该附录是未来动作，本轮不额外生成。仅精确暂存批准文档，不把整个 Novauiux、原型 support.js、私人截图、audit、数据库或认证纳入。
4. 记录该文档提交完整 SHA、父 SHA、文件清单及两份原规格 hash，作为 DOC_SHA；目前 DOC_SHA 尚不存在，不虚构。保留代码基线 6ec3842f23f91b7b5d8606f2e1dfbcb2fce058d3 可追溯。
5. 确认新目录/分支未被占用后，从实际 DOC_SHA 创建 `codex/uiux-01a`，独立 worktree 推荐绝对目录 `D:\nova-uiux-01a`。不从混合工作区复制未提交源码，不覆盖已存在目录。原 D:\nova 原样保留。本轮不创建。
6. 实施顺序：增量 schema/事务内公共核心与隔离测试 → 草稿消息 revision IPC → 原子确认与启动交接 → 前端输入保护及 mock → 独立 PlanningAdapter/profile 与真实权限验证。可以分代码提交，但不能把未通过权限验证的规划入口启用给用户；人工降级清楚标识。
7. 完整测试和 Claude 审核后才安排候选 GitHub 构建、备份安装及定向复验；本轮没有这些授权执行。Git 回滚逐提交可审查 revert，迁移数据按第 8 节处理，不以代码回退自动恢复库。
8. 后续开发报告放 D:\nova\docs\reports，审核报告放 D:\nova\docs\reviews，均实际北京时间命名，记录实施 worktree 绝对路径及精确 SHA。原型只作视觉参考，01A 不直接接入模拟附件、固定进度或双 AI 剧本。

## 12. 风险、决定与审核重点

已决定的是产品流程、单 AI、授权范围、原文保护、原子唯一绑定及 fail-closed；推荐工程选择是 8 张增量表、receipt 幂等、24 小时授权、输入上限与 120 秒超时。后者可由工程审核调整并版本化，无需把字段命名交给非技术用户决策。

仍须技术验证：两个 CLI 精确版本能否在不更改日常配置/认证前提下可靠禁用执行/任意读取与扩展；现有登录与 per-process 配置封闭是否兼容；Windows 路径读取边界；迁移竞争及当前启动与新确认的竞争。不能用 --help、提示词或单次成功推断安全性。

Claude 重点审核：所有新接口的组合身份、same-key different-payload、先查回执再判断过期、共享事务核心保留门禁、取消/崩溃 uncertain 不盲重发、规划与执行账本分离、旧快照匹配不变、旧 schema 拒绝与回滚说明、前端 IME/晚响应不覆盖输入。若无法证明受限规划，保留人工草稿是唯一降级，不能自动调用可写开发适配器。

本轮只做源码/文档阅读与设计文件生成；未运行单元测试、迁移、真实 CLI/AI、项目测试/构建，未读取日常任务数据库、认证或密钥，未创建分支/worktree、暂存、提交、推送、安装或发布。设计完成不表示功能已实现或安全 profile 已通过。
