# UIUX-01A 需求确认基础技术契约：独立设计审核

时间：2026-10-22 16:05，北京时间。性质：独立**设计**审核报告；审核对象是技术契约文档本身，不是实现、测试或安装验收结果。本轮未实施产品代码、未进行真实付费 AI 调用、未执行数据库迁移、未读取真实任务库/认证/密钥、未创建分支或 worktree、未暂存/提交/推送/安装/发布。

## 一、基线与审核材料定位

- 实际分支：`feature/v0.1.0-mvp`。
- 完整 HEAD：`git log -1 --format="%H %cd"` → `6ec3842f23f91b7b5d8606f2e1dfbcb2fce058d3`（Thu Oct 22 08:43:06 2026 +0800），与契约声明基线完全一致。本轮审核即覆盖该代码版本，未发现后续提交。
- `git status --short`（排除 `??`）：仅 `M product/NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md`、`M product/NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`，与此前全部 10 轮审核一致，与本设计无关，本轮未触碰。暂存区为空。未发现适用 AGENTS.md/CLAUDE.md。

5 份审核材料（均为未跟踪文件，`git status --short -- <path>` 返回 `??`），逐一计算工作区字节 SHA-256：

| 文件（绝对路径前缀 `D:\nova\`） | 行数 | SHA-256 | Git 状态 |
|---|---|---|---|
| `docs\reports\CODEX_UIUX_01A_CONTRACT_DESIGN10221531.md` | 247 | `229a3a11ebaf580ab0b912d74d1ee122cd246b0cc0a118173c1b8c37ec0704b8` | 未跟踪 |
| `docs\reports\CODEX_UIUX_MIGRATION_IMPLEMENTATION_PLAN10221411.md` | 184 | `2ab305d8a0d171761ca07d9bf5760c5a5c66851a713181d99bd6f9fad893c224` | 未跟踪 |
| `docs\reports\CODEX_RAV01_CLOSURE_MILESTONE_REPORT10221233.md` | 77 | `28b6ab947e7f6be10b0176b366feb0f9d1a18a09a98553061b92d8cb2e4a22fe` | 未跟踪 |
| `Novauiux\NOVA_V0.1.0_UI_UX_DESIGN10200953.md` | 212 | `d849f62dddbd90780db9db39b084a18b9854f642f5b05830d0e3c6beef91f3b8` | 未跟踪 |
| `Novauiux\NOVA_V0.1.0_INTERACTION_DESIGN10200953.md` | 205 | `17bf619efc6ddae4edb8a4f2b6f3f96700f6b2f0cfb6dedbe0c576607c4f5f0a` | 未跟踪 |

本次未自行提交这些文件以获得 commit SHA；上表的绝对路径+SHA-256+行数是唯一定位依据。5 份文件均已逐份完整阅读。

## 二、实际核对命令（只读，未产生副作用）

```
git status --short | grep -v '^??'
git branch --show-current
git log -1 --format="%H %cd"
git status --short -- <5份文档路径>
sha256sum <5份文档路径>
wc -l <5份文档路径及下列源码文件>
grep/Read: storage/tasks.rs（create_task、transition_task、LEGAL_TRANSITIONS、evaluate_completion_gate）
grep/Read: storage/requirement_items.rs（split_requirement_field、insert_confirmed_requirement_items_if_absent）
grep/Read: runtime/ai_adapter.rs（CodexCliAdapter、ClaudeCliAdapter，workspace-write/read-only/acceptEdits/--tools 标志；grep "PlanningAdapter" 全仓库零命中；grep mcp/hook/plugin/skill/resume 零命中）
grep/Read: runtime/path_guard.rs（validate_directory_boundary 全文）
grep/Read: runtime/win_spawn.rs、runtime/job_object.rs（JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE）
grep/Read: storage/migrations.rs（CURRENT_SCHEMA_VERSION=11、run() 函数第 1090-1135 行、VersionTooNew、backup_database）
grep/Read: db.rs（busy_timeout=5s）
grep/Read: lib.rs（AppState.core_loop_cancel: Arc<Mutex<HashMap<String, CancellationToken>>>）
Read: commands.rs（start_core_loop 第 854-961 行、cancel_core_loop）
grep/Read: core_loop/verify_only.rs（模块声明 "No AI adapter is accepted or constructed"）
grep: storage/actions.rs（既有 idempotency_key + attempt_number CAS 模式）
grep: stores/nova.ts（provider/model 绑定 SDKMAX_PROVIDER_ID）
grep: tauri.conf.json（minWidth=920, minHeight=640）
grep: migrations.rs 内 requirement_conversations 等新表名、"version 12" 全零命中（确认 schema 未变）
find: apps/desktop/src/stores 下无 workspace.ts（确认未新建）
```

## 三、NOT RUN（未执行、不代表失败）

未运行任何 `cargo test`/`npm test`/构建；未启动真实 Codex/Claude CLI；未执行数据库迁移（无 v12 存在，无可迁移）；未读取真实任务数据库或 D:\NovaRAV01Project；未创建分支/worktree；未触发 GitHub CI；第 10 节验收矩阵的 20 个场景全部 NOT RUN（契约自身亦如此声明）。这些均符合"设计尚未实现"的预期阶段，本身不构成设计缺陷。

## 四、八项重点逐条核查结论

### 1. 原子确认

**已核实为真**：`storage/tasks.rs` 的 `create_task`（[tasks.rs:215-253](apps/desktop/src-tauri/src/storage/tasks.rs:215)）与 `transition_task`（[tasks.rs:527-669](apps/desktop/src-tauri/src/storage/tasks.rs:527)）各自内部完整执行 `BEGIN IMMEDIATE ... COMMIT/ROLLBACK`，这意味着两者**不能被外部事务直接嵌套调用**（SQLite 不支持嵌套 BEGIN）——契约第 2 节证据表及第 6 节"提取私有核心函数"的建议与此完全吻合，是可行且必要的重构方向。

进一步核实：`Draft→AwaitingConfirmation→Confirmed` 确为 `LEGAL_TRANSITIONS`（[tasks.rs:138-141](apps/desktop/src-tauri/src/storage/tasks.rs:138)）中两段合法迁移；需求快照冻结仅发生在 `to == TaskState::AwaitingConfirmation` 分支（[tasks.rs:645-654](apps/desktop/src-tauri/src/storage/tasks.rs:645)）。契约第 6 节步骤 4"创建 draft Task，沿既有合法迁移到 awaiting_confirmation 并冻结需求快照，再到 confirmed"与真实状态机精确对应，不是简化描述。

**结论**：可行。实现时需要把 create_task 的"插入 task+事件"核心、transition_task 的"校验+UPDATE+事件(+快照)"核心各拆出一个不自行开/提交事务的私有函数，新确认服务在外层一次 `BEGIN IMMEDIATE` 内连续调用：建 Draft 核心 → 迁移到 AwaitingConfirmation 核心（触发快照）→ 迁移到 Confirmed 核心 → 插入 binding/receipt/系统消息，全部提交。此结构与现状完全兼容，不需要复制一套放宽门禁的裸写路径。未发现不可实现之处。

### 2. 幂等与并发

**已核实为真**：现有 `storage/actions.rs` 已存在成熟的 `idempotency_key + attempt_number` CAS 模式（`UNIQUE(task_id, idempotency_key)`、attempt 绑定、"已替代"/"已不是预期 attempt"拒绝逻辑），契约新增的 `client_request_id`/`requirement_mutation_receipts`/`expected_revision` CAS 是同一套思路在新领域的延伸，而非凭空设计，工程上有先例、可信度较高。

另确认该应用进程内**真实存在多条独立 SQLite 连接**：`AppState.db`（命令处理主连接）与 `commands::start_core_loop` 内 `crate::db::Database::open(&db_path)`（每次核心循环后台线程另开一条，见 [commands.rs:928-929](apps/desktop/src-tauri/src/commands.rs:928)）。这说明验收矩阵中"两连接从 R 保存""事务故障注入"等场景在单个应用实例内就是可真实触发的 busy_timeout/BEGIN IMMEDIATE 竞争，不是臆造的多进程假设。

**结论**：契约对"重发同请求/同 key 不同内容/不同 key 并发确认同修订/晚响应/多窗口保存冲突"的处理路径（receipt 先查、binding 唯一约束兜底、PK(draft_id,revision) 作为 CAS 兜底）在数据库层有真实约束支撑，不只是前端按钮禁用。未发现遗漏的并发场景；"事务故障注入"类场景需要专门的失败注入测试才能验证"任一写入点失败后不存在半 Task/半绑定"，契约已将其列入验收矩阵第 10 节，定位准确。

### 3. 原文与修订

**已核实为真**：`storage/requirement_items.rs` 的 `split_requirement_field`（[requirement_items.rs:25-33](apps/desktop/src-tauri/src/storage/requirement_items.rs:25)）确实执行 CRLF 归一化→按行 split→**trim**→丢弃空行→**保留顺序和重复行**；模块级文档注释明确写明"这四个字段在本代码库中没有任何编辑入口（只有 `Database::create_task` 写过它们）"。`create_task` 的 INSERT 直接写入 `request.goal` 等原始字符串，未做 redact（只有逐项快照 `item_text_redacted` 才脱敏）——这与契约"Task 接收四字段原文"的表述一致。

契约第 4.1 节"不 trim 后回写、不 Unicode 规范化"描述的是**新增的 revision 原始层**（`goal_raw` 等），与"冻结快照仍使用现有拆项规则"（即仍会 trim/去空行，但保留重复行与顺序）是两个不同的存储层，契约原文已明确区分，不构成自相矛盾。

**结论**：契约对现状的描述准确，新旧两层（原始 revision 文本 vs. 冻结拆项快照）的边界划分清晰，不会出现"用翻译文本替换自查覆盖依据"的风险。未发现设计缺陷。

### 4. 规划权限（本契约风险最高的结论）

**已核实为真，且现状比契约描述更紧**：

- `runtime/ai_adapter.rs` 确认：Codex 开发用 `--sandbox workspace-write`，审核用 `--sandbox read-only`；Claude 开发用 `--permission-mode acceptEdits`，审核用 `--tools Read --disallowedTools Bash ...`（[ai_adapter.rs:490,500,573,591-594](apps/desktop/src-tauri/src/runtime/ai_adapter.rs:490)）。
- 全仓库 `grep -rn "PlanningAdapter"` **零命中**——确认不存在独立 PlanningAdapter，契约陈述为实。
- 在 `ai_adapter.rs` 内对 `mcp|hook|plugin|skill|strict-mcp|resume|settings-file` 等关键词检索**全部零命中**——当前代码从未尝试过关闭 MCP/hooks/plugins/skills/自动配置加载/会话续接，不仅"未证明封闭"，而是**连尝试封闭的代码都不存在**。契约"规划能力默认 BLOCKED"的结论不是保守估计，而是对真实差距的准确反映，审核认为契约在这一点上没有夸大风险，反而是恰如其分地 fail-closed。
- `runtime/path_guard.rs` 的 `validate_directory_boundary` 只是"候选目录是否在允许根目录之内"的边界检查（含 canonicalize 防 `..`/符号链接逃逸），属于选 cwd 的校验，**不提供**文件写入拦截、网络拦截或"除 cwd 外完全不可读写"的沙箱能力——与契约表述完全一致。
- `runtime/job_object.rs` 的 Windows JobObject 仅设置 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`（子进程树随 Job 关闭而终止），**不含**任何网络/文件系统访问限制标志——契约"JobObject 仅管生命周期，不作为权限边界"的表述被直接证实。

**结论**：规划能力必须保持 BLOCKED 这一结论有充分源码依据，审核**支持**该结论，且认为不应被后续实现阶段放宽为"先用现有 read-only/acceptEdits 顶一下"。契约第 7.1 节列出的开放前置条件（零工具、零 MCP/hooks/plugins/skills、独立临时目录、配置不可重写为可写/执行、仅认证所需网络）在当前代码中没有任何一条已经具备，契约对此如实列为"仍须技术验证"而非"已满足"，判断准确。

### 5. 请求生命周期

契约第 7.3 节描述的 `reserved→dispatching→running→succeeded/failed/cancelled/uncertain` 状态机、"先持久化 request/receipt，CAS 写 dispatch_fence 后才 spawn"、"不得持有 DB 事务等待外部进程"，与现有 `core_loop_cancel: Arc<Mutex<HashMap<String, CancellationToken>>>`（[lib.rs:48](apps/desktop/src-tauri/src/lib.rs:48)）和 `start_core_loop` 的"先插入内存槽位再探测/启动"模式（[commands.rs:877-883](apps/desktop/src-tauri/src/commands.rs:877)）思路一致：该 HashMap 明确是**进程内、按 task_id 的内存机制**，在 `tauri::async_runtime::spawn` 返回后立即移除（[commands.rs:944-947](apps/desktop/src-tauri/src/commands.rs:944)），不跨进程、不持久化。

**结论**：契约"当前启动保留主要是每任务内存机制，不能宣称跨进程 exactly-once"的表述准确。新规划请求的 `dispatch_fence` CAS 思路与此机制同构，但两者是**两套独立的槎位**（一套管 core_loop 执行，一套管 planning 请求）——契约应在后续实施文档中明确:规划请求的"项目级活动态获取"检查（第 6 节步骤 3"检查当前项目持久化活动任务及本进程启动保留"）需要同时查询 `core_loop_cancel` 内存表和数据库 `tasks.state`，否则可能出现"规划确认时看不到另一个窗口刚启动的执行"的竞争窗口。契约虽已提到"01A 必须测试与现有启动的竞争"，但未指出这是**两套独立槎位必须协同检查**这一具体机制点，建议实现阶段补充（非阻塞，但建议列为明确验收项）。

### 6. 数据迁移与回滚

**已核实为真**：`storage/migrations.rs` 的 `CURRENT_SCHEMA_VERSION = 11`（[migrations.rs:12](apps/desktop/src-tauri/src/storage/migrations.rs:12)），`db.rs` 的 `busy_timeout(5s)`（[db.rs:34](apps/desktop/src-tauri/src/db.rs:34)），`run()` 函数（[migrations.rs:1090-1135](apps/desktop/src-tauri/src/storage/migrations.rs:1090)）确认：版本号 `current` 在进入 for 循环**之前**一次性读取（第 1091 行），随后每个迁移步骤才各自 `BEGIN IMMEDIATE`（第 1113 行）。

这证实了契约第 8 节指出的真实缺口：**两个进程/线程同时调用 `migrations::run()` 时都会读到同一个旧 `current`**，第二个在等待第一个的 `BEGIN IMMEDIATE` 释放锁之后，仍会用循环前捕获的旧 `current` 判断"该迁移步骤需要执行"，进而重复执行已被对方应用过的 DDL（大概率因"表已存在"等错误触发该步骤的 ROLLBACK，而不是静默损坏数据，但会让第二个启动方看到一次可规避的迁移失败）。契约"建议在获得迁移锁后重读版本"的修复方向是对的，且契约已如实标注"这属于实现时必须隔离验证的最小兼容修正，不宣称现状已具备"——这一表述准确，不是夸大也不是掩盖。

`VersionTooNew`（[migrations.rs:16-19,1093-1098](apps/desktop/src-tauri/src/storage/migrations.rs:16)）确认旧 v11 程序打开 v12 数据库会被拒绝，与契约"旧 v11 程序面对 v12 会 VersionTooNew 拒绝"一致；`backup_database`（[migrations.rs:1166](apps/desktop/src-tauri/src/storage/migrations.rs:1166)）使用 SQLite 在线备份 API 并在失败时阻止升级，与契约"备份失败停止升级"一致。契约"代码回退不等于 DROP TABLE、降 user_version 或覆盖用户数据"的提醒准确对应当前机制（revert 只回退代码，不触碰已升级的数据库文件）。

**结论**：契约对迁移现状的描述真实、准确，且正确识别了一个现存但尚未修复的并发读版本缺口，并未宣称该缺口已解决。未发现虚假宣称。

### 7. 前端/UX

交叉核对两份 Novauiux 规格文档（`UI_UX_DESIGN`/`INTERACTION_DESIGN`，10200953 版）：两文档第 1/6/7 节已明确"单 AI + Nova 验证""每轮确认冻结范围""最多一轮自动修复""预览不等于验收"等决策，与契约第 1 节结论完全一致，不存在冲突。两文档第 11/15 节"源码映射与实现差距"表也已自行标注"新版核心待实现、不能据此宣布真实闭环"，与契约的保守表述口径一致。

`tauri.conf.json` 确认当前 `minWidth=920, minHeight=640`（[tauri.conf.json:18-19](apps/desktop/src-tauri/tauri.conf.json:18)），证实契约"920×640 最小窗口保留"指的是 01A 阶段**沿用现有真实窗口最小值**，而不是 UI/UX 设计文档里"目标最小窗口 1024×700"的新值——这是两个文档间一个容易混淆但契约已经正确处理的差异点：01A 明确不做工作台布局迁移（01B/01C 范围），因此继续用 920×640 而非设计稿的 1024×700 是合理且与自身声明的范围边界一致的选择。

**结论**：未发现前端/UX 层面的设计缺陷。IME、晚响应、多窗口冲突、手动降级文案等在契约第 9 节均有对应规则，且与两份规格文档的决策口径一致。

### 8. 复杂度与实现拆分

8 张新表逐一评估：

- `requirement_conversations`：当前 `UNIQUE(project_id)` 使其与项目是严格 1:1 关系，**可简化**——01A 范围内可以不建这张表，直接用 `project_id` 充当对话锚点，`requirement_messages`/`requirement_drafts` 的外键改为直接指向 `project_id`；未来若真的需要"一个项目多个需求对话"再引入该表更合理。此为**非阻塞的简化建议**。
- `requirement_drafts`/`requirement_revisions`/`requirement_messages`：必要，分别承载"头状态机""不可变版本化内容+CAS""原始对话流水"，三者职责不重叠。
- `planning_authorizations` 与 `planning_requests` 分离：合理——一次授权（24 小时有效）可覆盖同一范围内的多次规划请求，不必每次请求都重新走一遍用户同意流程；若合并成一张表，会让"撤销授权"与"取消单次请求"两种不同语义互相纠缠。
- `requirement_task_bindings` 独立于 `requirement_revisions`：合理——revision 表记录**全部**版本（包括被放弃、被覆盖的），binding 表只记录**唯一**真正变成 Task 的那一次，混用会让"不可变 revision 历史"与"永久绑定记录"的删除/归档策略产生冲突。
- `requirement_mutation_receipts`：**并非冗余**——`confirm_requirement_revision` 确实可以靠 `requirement_task_bindings` 的 `UNIQUE(draft_id)/UNIQUE(task_id)` 作为内容级兜底，但 `save_requirement_revision`/`apply_requirement_proposal` 没有这种天然的内容唯一性（同一 `expected_revision` 的重发在没有 receipt 的情况下会直接撞 `PK(draft_id,revision)` 导致客户端看到"保存失败"，即使服务端此前已经成功），receipts 表正是为了解决"服务端成功但客户端网络超时未收到响应"这一类场景，是必要的基础设施，和 `storage/actions.rs` 现有 idempotency 模式同构。

**A/B 拆分评估**：契约在第 11 节步骤 6 已经给出"增量 schema/事务内公共核心与隔离测试 → 草稿消息 revision IPC → 原子确认与启动交接 → 前端输入保护及 mock → 独立 PlanningAdapter/profile 与真实权限验证"的顺序，且明确"不能把未通过权限验证的规划入口启用给用户"。这与用户要求的"A）非 AI 草稿+原子确认基础可先实现 / B）规划能力必须继续独立验证"的拆分完全对应，且 A 不依赖 B 的任何产出（`get_planning_capability` 返回 `blocked` 时降级为人工四字段表单，A 侧的 draft/revision/binding/receipt 全部可以独立于真实规划能力落地并测试）。

**结论**：8 张表基本必要，唯一的简化空间是 `requirement_conversations`（非阻塞建议）。A/B 拆分边界清晰、可独立交付、不存在"A 完成即等于规划能力完成"的描述风险——契约第 1 节已明确写"这不阻止先实现非 AI 草稿基础，但不能据此宣布真实规划能力完成"，表述谨慎。

## 五、问题清单汇总

| # | 严重度 | 位置 | 触发条件 | 影响 | 建议 | 是否阻塞后续实现 |
|---|---|---|---|---|---|---|
| 1 | 低 | 契约第 6 节步骤 3 / `commands.rs:877-883` | 规划确认事务检查"项目活动任务"时，只查数据库 `tasks.state`，未明确同时查询内存级 `core_loop_cancel` 槎位 | 理论上存在极窄的竞争窗口：另一窗口刚调用 `start_core_loop` 插入内存槎位但尚未落库前，新确认可能短暂看不到"执行中"状态 | 实施时在该检查中显式注明需要同时读取内存槎位表，并把这一点列为独立验收场景 | 否——非阻塞，建议在实现阶段补充验收项 |
| 2 | 低 | 契约第 4 节表 / `requirement_conversations` | 该表当前设计与项目 1:1（`UNIQUE(project_id)`） | 增加一张本质可省略的表，略增实现与迁移复杂度 | 01A 范围内可直接用 `project_id` 作对话锚点，省去该表；如需保留应在契约中说明"为未来多对话扩展预留"的具体理由 | 否——简化建议，不影响正确性 |
| 3 | 信息 | 契约第 8 节 / `migrations.rs:1090-1135` | 两个进程/线程并发首次启动触发迁移 | 现状确认存在"循环前读版本、锁内不重读"的竞争，可能导致并发启动时一方出现可规避的迁移失败（非数据损坏） | 契约已给出正确修复方向（锁内重读版本）并如实标注未实现；建议实现阶段把这一项列为**必须有自动化测试覆盖**的场景（验收矩阵已含"迁移...并发升级验证"，建议明确写成断言用例而非笼统描述） | 否——契约已正确标注为已知未解决项，不是虚假宣称，实现前必须修但不构成"契约本身有缺陷" |

未发现严重或阻塞级问题；上述三项均为低严重度/信息性的完善建议，不影响契约作为后续实现依据的可用性。

## 六、最终结论

**设计审核结论：PASS（契约可作为后续实现依据，范围限定为契约自身声明的 01A 边界；"规划能力"子范围维持 BLOCKED，须独立验证后才能开放）。**

本结论仅表示"设计审核通过"，不代表实现已完成、测试已通过，也不构成任何后续开发、迁移、候选构建或发布的授权。

### 用户要求的四项必答结论

1. **非 AI 基础（草稿/消息/修订/原子确认/绑定/receipt）能否进入实现？** 能。该部分不依赖规划能力，表结构、CAS 机制、与现有 `create_task`/`transition_task`/`confirmed_requirement_items` 的整合方式均已核实可行，且架构上与现有 `idempotency_key`/CAS 模式同构，风险可控。建议实现顺序严格遵循契约第 11 节步骤 6 给出的顺序（schema/公共核心 → IPC → 原子确认 → 前端保护 → 规划能力），不要提前并行开发规划相关 IPC 的生产可用路径。

2. **真实规划能力是否必须继续保持 BLOCKED？** 必须。源码核查显示当前两个 CLI adapter（Codex `read-only`/Claude `--tools Read`）均未尝试过封闭 MCP/hooks/plugins/skills/配置自动加载/会话续接，`PlanningAdapter` 不存在，Windows JobObject 和 path_guard 均不构成权限边界。在精确版本的隔离验证（契约第 10 节"fake CLI 不能证明真实供应商工具隔离"所指的那一轮真实验证）完成并给出可复核证据之前，`get_planning_capability` 必须返回 `blocked`，不得因为"read-only 参数存在"或"--help 提到了某个选项"而默认放行，也不得在验证受阻时回退到可写的开发适配器。

3. **Codex 下一步必须修改/补充的设计内容：**
   - 在第 6 节步骤 3 补充说明:"项目活动任务"检查需要同时覆盖内存级 `core_loop_cancel` 槎位与数据库 `tasks.state`（问题清单 #1）；
   - 对 `requirement_conversations` 表给出保留或移除的明确理由（问题清单 #2），若保留，说明具体的未来多对话场景；
   - 把迁移并发重读版本号的修复（第 8 节已提及）列为**实现阶段的独立、可独立验证的最小修复任务**，并在验收矩阵第 10 节把"迁移...并发升级验证"场景写成具体断言（例如:两线程同时调用 `migrations::run()`，断言只有一方真正执行 DDL，另一方正常等待后发现 `current_schema_version` 已是最新且不重复执行）；
   - 在进入"独立 PlanningAdapter/profile 与真实权限验证"开发前，应先产出一份单独的、带精确 CLI 版本号的隔离验证计划/报告（契约第 10 节已经描述了方法，但尚未执行，也不在 01A 本次契约的交付范围内），作为该子阶段的独立准入门槛。

4. **后续实现的最小工作包与验收条件：**
   - **最小工作包（A，可独立交付）**：v12 迁移（仅 8 张新表+索引，不改 1-11）；`create_task`/`transition_task` 私有核心拆分 + 新确认服务的单一外层事务；`requirement_drafts`/`revisions`/`messages`/`task_bindings`/`mutation_receipts` 的 CRUD 与 CAS；`open_requirement_conversation`/`list_requirement_messages`/`get_requirement_draft`/`create_requirement_draft`/`save_requirement_revision`/`append_requirement_message`/`confirm_requirement_revision`/`get_requirement_binding`/`archive_requirement_draft` 等**不涉及真实 AI 调用**的 IPC；`get_planning_capability` 固定返回 `blocked`（真实 profile 验证完成前），前端走人工四字段表单降级路径；`workspace.ts` 新 store，不改动 `coreLoop.ts`。
   - **验收条件**：契约第 10 节验收矩阵中除"提示注入"/"上下文边界"/"真实供应商工具隔离"外的全部场景（即不依赖真实 PlanningAdapter 的场景）必须实际跑通并记录退出码；额外补充本报告问题清单 #1（内存槎位联合检查）与 #3（迁移并发重读版本）两个场景的自动化测试；Rust 全量测试、前端全量测试、`vue-tsc`/构建、既有仓库规定检查（如 `git diff --check`）全部通过，不降低既有断言；经 Claude Code 独立审核 PASS 后才能提交候选构建。
   - **规划能力子包（B，独立准入）**：仅在 A 完成并审核通过、且独立隔离验证报告（精确 Codex/Claude 版本、零工具/MCP/hooks/plugins/skills 封闭、项目外读取不可达、写入/命令/任意网络不可用、取消/重启不重复计费）给出可复核的 PASS 证据后，才可将 `get_planning_capability` 对特定已验证的 `(selected_ai, binary_hash, policy_version)` 组合改为 `available`，且必须保留随时因版本/哈希变化自动失效的机制。

## 七、审核边界声明

本轮仅进行只读源码核查、静态分析与文档阅读，未读取或修改任何用户数据、未运行任何 CLI/AI/测试/构建/迁移、未创建分支或 worktree、未暂存/提交/推送/安装/发布，也未修改任何产品代码或原始设计文档。D:\nova 混合工作区的既有未提交修改与未跟踪资料原样保留，未做整理。
