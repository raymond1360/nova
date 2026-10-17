# WP2A：结构化自检契约与完工门禁规格说明（第三版，补齐PM本轮指出的7项遗漏）

**与上一版关系：** 本文件是 `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141220.md` 的修订版，原文件保留，冲突处以本文件为准。本版补齐PM本轮指出的7项遗漏：需求覆盖细粒度、历史分类矛盾、自检写入顺序、当前结果一致性全链路、状态与原子门禁、验收矩阵补项、文档引用准确性。
**上一轮复审结论更正（回应第7项"文档准确性"）：** `D:\nova-review\CODEX_WP2A_SPEC_REVIEW10141205.md` 的结论是 **5个 Blocker（B1–B5）+ 4个Major（M1–M4）+ 2个Minor（m1–m2），FAIL**。上一份关闭说明 `docs/release-v0.1.0/CLAUDE_WP2A_SPEC_REMEDIATION10141223.md` 标题区误写"1个Blocker、2个Major"，为之前更早一轮（`CODEX_SINGLE_AI_...`，非WP2A专项复审）的数字，本轮已核对并更正，特此声明：**那是两次不同的复审，数字不能混用**。
**状态：** 规格设计稿，**未实现任何代码**。方案通过只是条件性声明——"本版方案在静态代码核验下自洽、可实施"，**不暗示已通过Codex复审，不代表功能已实现**。
**代码核验方式：** 本轮新增核验的代码位置见各节"代码依据"，均为本轮重新读取，不是转抄上一版或审核意见。

---

## 0. 本版新增/修订对照表

| 编号 | PM本轮要求 | 本版位置 |
|---|---|---|
| 1 | 需求覆盖细粒度 | 第1节重写，新增"确认需求项快照"机制 |
| 2 | 历史分类矛盾+完整读完双AI action代码 | 迁移计划同步修订（见配套文件），本文件第4.2节引用其结论 |
| 3 | 自检写入顺序 | 第2.2节重写，弃用纯`created_at`，改用`rowid`+attempt过滤 |
| 4 | 当前结果一致性全链路 | 第3节新增完整调用链追踪 |
| 5 | 状态与原子门禁 | 第5节重写：合法转换边核验+事务（非单SQL）方案 |
| 6 | 验收矩阵补项 | 第7节全表重写 |
| 7 | 文档引用准确性 | 本节声明+全文交叉引用核对 |

---

## 1. 需求覆盖——细粒度结构化确认（回应第1项）

### 1.1 问题重述

上一版把"权威集合"定为固定4个字段名（`goal/in_scope/not_in_scope/done_criteria`），本质上只能证明"AI提到了这4个字段分别覆盖没覆盖"，**不能证明"这个字段里的每一项具体要求都被处理了"**——例如`in_scope`字段里用户写了3行不同的要求，AI完全可以笼统说"in_scope: covered"就通过，没有逐项核对。

### 1.2 代码依据（本轮核验）

- `Task.goal/in_scope/not_in_scope/done_criteria`均为`Option<String>`自由文本（`models.rs:319-322`），内部没有任何分隔结构被解析。
- 前端`CoreLoopView.vue`的"新建任务"表单里，这四个字段各自是一个`<textarea>`，用户天然会用换行分隔多条要求（符合常见填写习惯，但代码里没有强制）。
- `store.confirmRequirement`（前端）调用的是通用`transition_task`命令，`to_state='awaiting_confirmation'`（`apps/desktop/src/stores/coreLoop.ts:468-476`），**不是专门的"确认需求"后端函数**——也就是说，当前"确认需求"这个动作，在后端只是一次普通的状态转换，没有任何需求快照/锁定逻辑。
- `transition_task`内部已有先例：当`to == PreviewReady`时，在同一事务内额外执行一次`result_generation`自增（`storage/tasks.rs:364-369`）——这是"在某个特定目标状态上附加一段额外的、仅在该分支触发的持久化逻辑"的现成模式，可以直接复用于本节方案。

### 1.3 决定：新增"确认需求项快照"机制（不留候选方案）

**解析规则（固定，不留给编码阶段决定）：** 对`goal/in_scope/not_in_scope/done_criteria`四个字段中每一个非空字段，按换行符分割，去除首尾空白后丢弃空行，剩余每一行是一个独立的"需求项"，按其在字段内的出现顺序编号（从0开始）。

**新增表（新迁移，不改动既有表）：**

```
confirmed_requirement_items(
  id, project_id, task_id,
  field TEXT NOT NULL CHECK(field IN ('goal','in_scope','not_in_scope','done_criteria')),
  item_index INTEGER NOT NULL,
  item_text_redacted TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(task_id, field, item_index)
)
```

**写入时机（复用`transition_task`现有的"目标状态附加逻辑"模式）：** 当`transition_task`的`to == AwaitingConfirmation`时，在同一事务内按1.3节解析规则对当前任务的四个字段生成全部需求项并一次性写入该表——这个动作在任务生命周期里只会发生一次（因为`goal`等字段创建后不可编辑，重复触发也只会生成相同内容；为避免重复行，写入前先检查该`task_id`在表中是否已有行，若已有则跳过，不覆盖、不报错）。

**与上一版"4字段固定key"方案的关系：** 不是推翻，是细化——原来的`item: "goal"|"in_scope"|...`现在变成`field + item_index`的组合key（如`in_scope:0`、`in_scope:1`），权威集合从"至多4个"变成"每个任务实际有多少行文本就有多少项"，自然解决"一个字段多项要求"的问题。

**自检`requirement_coverage`门禁（替换上一版第3.2节第7条）：** 覆盖列表的key集合必须与`confirmed_requirement_items`表中该`task_id`的全部`(field, item_index)`组合**完全一致**（无缺项、无重复、无伪造多余项），且每一项`status`必须是`covered`，列表非空（若该任务的四个字段全部为空，`confirmed_requirement_items`里本就没有行，视为"无需求项可核对"的异常创建态，完工门禁直接拒绝并给出"任务未填写任何需求内容"）。

**工作包边界（回应PM"明确必要改动和工作包边界"）：** 本机制涉及一张新表+一次迁移+`transition_task`一个新分支，是WP2A里**独立于自检解析/完工门禁核心逻辑**的前置子工作包，建议命名为**WP2A-0：确认需求项快照**，必须先于WP2A主体（自检解析、完工门禁、原子性改造）落地，因为门禁逻辑依赖这张表已经存在数据。WP2A-0不涉及自检本身，可以独立提交、独立测试、独立复审。

---

## 2. 自检存储与写入顺序（回应第3项，修订上一版第2节）

### 2.1 语义维持（第三种独立语义，不类比confirmations/evidence）

与上一版一致：追加写入，不删除历史行。

### 2.2 排序依据——弃用纯`created_at`，改用`rowid`，并说明其与attempt过滤的关系

**问题：** `created_at`是字符串时间戳，同一秒内可能有多行（例如Nova对同一份`DEVELOPMENT_REPORT.md`短时间内重新解析两次），纯按`created_at DESC`排序在时间相同时顺序不确定。

**决定：** SQLite表在未声明`WITHOUT ROWID`时，每行自带一个单调递增的隐藏`rowid`（本表所有CREATE TABLE语句均为`id TEXT PRIMARY KEY`，不是`INTEGER PRIMARY KEY`，因此`rowid`是独立于`id`的隐藏列，天然随插入顺序单调递增，不需要新增任何列）。"当前判定"查询改为：

```sql
SELECT * FROM self_checks
  WHERE task_id = ? AND round = ? AND dev_action_attempt_number = ?
  ORDER BY rowid DESC LIMIT 1
```

**"旧解析晚到"场景的真实处理机制（回应PM"不能把最后到达直接等同于当前执行的最新结果"）：** 关键在于上面SQL的`WHERE`子句已经按`dev_action_attempt_number = ?`（且`?`在查询时**现场重新读取**`task_actions`当前的`attempt_number`，不是缓存值）做了强过滤——如果"晚到的旧解析"携带的是一个**旧的**`attempt_number`（比如对应已经被真实重试覆盖掉的那次执行），它根本不会出现在这个查询结果集里，`rowid`排序完全不会被用到，因为它已经被`WHERE`条件排除了。**`rowid DESC LIMIT 1`这个排序规则只在"同一个attempt被重复解析多次"这一种场景下才会real发挥作用**（例如Nova的文档解析逻辑因为某种原因被调用了两次，针对完全相同的一次AI执行产出了两份self_checks行）——在这种场景下，两行的`dev_action_attempt_number`是相同的，"更晚插入的那次解析"确实应该覆盖"更早插入的那次解析"的判定效力，这才是"最后到达=当前判定"这句话成立的唯一场景，PM担心的"跨attempt的晚到"场景已经被attempt过滤挡在外面，不会触发这条排序规则。

**幂等重解析：** 同一attempt被重复解析、产出内容完全相同的两行，不需要特殊去重逻辑——`rowid DESC LIMIT 1`取到最新一行，内容相同则判定结果相同，无害。

**"新invalid使旧valid失效"：** 若同一`(task_id, round, dev_action_attempt_number)`先写入一行`parse_status='valid'`，后又写入一行`parse_status='invalid_format'`（例如重新解析时发现之前的解析其实有误），`rowid DESC LIMIT 1`会取到新的invalid行，门禁判定随之变为"自检无效"——旧的valid行不再被采用，符合Codex M1的要求。

---

## 3. 当前结果一致性——完整调用链追踪（回应第4项）

### 3.1 调用链（本轮通读`run_dev_and_test_round`/`run_single_ai_round`/`storage/tasks.rs`完整追踪，非方案假设）

```
① 开发/修复执行：run_dev_and_test_round(round)
   - dev_key = "dev-{task_id}-round-{round}"（core_loop.rs:1553，无generation）
   - gate_action(..."ai_development", dev_key)：Started → 调用AI → finish_gated_action
     （task_actions.attempt_number 在此过程中若发生重试，会在 storage/actions.rs:198/229 真实递增）
   - 此刻 task.result_generation 的值 = "尚未因这一轮而递增"的值（记为 G）
     （因为递增只发生在后面第④步进入PreviewReady时，同一事务内）

② 独立测试/构建：run_tests_for_round / run_build_for_round
   - test_action_key = "test-{task_id}-generation-{G}-round-{round}"（core_loop.rs:2040，确实generation-aware）
   - build_action_key 同理（core_loop.rs:2368）
   - test_evidence/build_evidence 行写入时的 result_generation 列 = G（写入时读取的就是当前值，此时还未递增）

③【本规格新增步骤】自检解析：在①的开发/修复动作记为Succeeded之后（具体挂载点见3.2节），
   解析DEVELOPMENT_REPORT.md/FIX_REPORT.md末尾的结构化区块，
   写入 self_checks 行，round=当前round，dev_action_attempt_number=①此刻task_actions的attempt_number

④ 若②全部通过：db.transition_task(..., PreviewReady, ...)
   - 同一事务内：result_generation = result_generation + 1（storage/tasks.rs:364-369）
   - 任务离开本轮时，task.result_generation 变为 G+1

⑤ adopted_passing_generation_round() 被调用时（在PreviewReady之后的任意时刻，
   包括FINAL_RESULT文档生成、用户查看证据、以及本规格的完工门禁）：
   - 循环 generation ∈ [当前值G+1, 当前值G+1-1=G]，round ∈ [1,2]
   - 实际命中的是 generation=G（②写入评据时的值），round=当前round
   - 返回 (adopted_generation=G, adopted_round=round)

⑥ 用户确认：record_user_acceptance 写入 result_generation = tasks.result_generation（此刻已是G+1）
   - 等等：这里有个需要澄清的细节，见3.2节

⑦ 完工判定：completion_gate_for_current_result()
   - 调用⑤得到 (adopted_generation=G, adopted_round)
   - 用 adopted_round 去查 self_checks（③写入的行），按 round + 当前attempt_number 过滤
   - 查 user_acceptance，按当前 tasks.result_generation（=G+1）过滤
```

### 3.2 关键澄清：`user_acceptance`的`result_generation`和"adopted evidence"的`generation`不是同一个数值，但这不是矛盾

**代码依据：** `storage/tasks.rs:327-330`的`EXISTS`子句写的是`ua.result_generation = tasks.result_generation`——即比较的是`user_acceptance`表里的值和**当前**`tasks.result_generation`（已经是递增后的G+1），而`adopted_passing_generation_round()`命中的是**递增前**的G。两者数值不同（G+1 vs G），但这是**设计上刻意如此**，不是bug：`user_acceptance`从来就只关心"当前任务指针指向的是哪一代"，而证据天然记录在"产生这代结果的那一轮"，两者通过"generation在进入PreviewReady时+1"这一个单一事实绑定——不需要二者数值相等，只需要：**user_acceptance的generation == tasks当前generation**（这个在现有代码里已经原子保证），**而"当前generation"本身就是由adopted evidence所在的那一轮触发递增而来的**，链路上不存在断点。

**self_checks不需要存generation列的最终确认：** 正是因为上面这条链路，self_checks只要通过`round`对齐`adopted_round`、通过`attempt_number`对齐"当前这次开发/修复执行"，就已经间接绑定了正确的generation——不需要在self_checks表里再存一份generation来"证明结果相同"，那会是多余的冗余字段，真正的绑定关系是通过"round+attempt_number → 对应哪一轮 → 哪一轮的evidence写在哪个generation → 该generation递增后变成当前generation"这条**单向、确定、无歧义**的链路完成的，本规格第3.1节的调用链追踪就是这条链路的完整证明。

### 3.3 "round1可能在新generation下重新出现"的边界核验（回应B4遗留疑虑）

**代码依据：** `core_loop.rs:2029-2039`的注释提到一个假设场景——"任务完成round1（generation递增）、被拒绝、又被重新排队到新generation的round1"。本轮核验：全仓库搜索`requeue`/`restart_task`/`retry_task`等命令名**均无匹配**，且已有历史文档（`docs/reports/PHASE3_SINGLE_AI_FINAL_ACCEPTANCE_EVIDENCE_REPORT09242012.md`第52行）明确记载"产品当前唯一可达的生产入口`run_core_loop`从Confirmed/Queued状态启动，一次调用内result_generation全程不变，跨generation重新触发round1目前没有任何已接线的Tauri命令能做到"。**结论：该注释描述的是防御性设计考量（或测试场景），不是当前可达的生产路径**；`dev-{task_id}-round-1`这个key在当前可达命令下，一个task_id的生命周期内只会被使用一次，不存在跨generation复用同一round数字造成key冲突的真实风险。**若未来新增"重新排队"类命令，该命令必须同步让dev action key变为generation-aware（仿照test/build key的做法），否则会产生真实的key冲突——这是对未来功能的设计约束，记录在此，不是本规格需要现在解决的问题。**

---

## 4. 历史分类矛盾与证据源（回应第2项）——与迁移计划文件同步修订

详细的组合证据分类算法与证据源表已移至配套文件 `docs/release-v0.1.0/NOVA_SINGLE_AI_MIGRATION_PLAN<本轮时间戳>.md` 第2节重写版本，本文件不重复抄录，只引用其结论：

- **关键新发现（本轮完整读完`run_core_loop_dual_ai_v0_2`及其调用的`run_one_round`/`run_review_round`函数体，不再推迟）：** 双AI路径的开发步骤（`run_dev_and_test_round`）与单AI路径**完全复用同一函数**，dev action的`action_type`/key格式在两条路径下**完全相同，不可区分**。但双AI路径独有的**审核步骤**（`run_review_round`，`core_loop.rs:1685`起）使用`action_type="ai_review"`、key格式`review-{task_id}-round-{round}`（`core_loop.rs:1697-1698`）——**这是真实存在、可核验、与单AI路径完全不会产生的专属标识**。
- 这推翻了上一版"Action Ledger证据源还需要下一步核验"的说法——本轮已经核验完成，不是遗留项。

---

## 5. 状态与原子门禁（回应第5项，重写M3方案）

### 5.1 合法状态转换边核验（本轮新增核验，推翻上一版隐含假设）

**代码依据：** `storage/tasks.rs:32-82`的`LEGAL_TRANSITIONS`常量表——核验确认：
- **不存在** `(PreviewReady, NeedsUserAction)` 这条边。
- **存在** `(AwaitingUserAcceptance, NeedsUserAction)` 这条边（`storage/tasks.rs:64-67`）。
- **存在** `(AwaitingUserAcceptance, Completed)` 这条边（`storage/tasks.rs:63`）。
- **存在** `(AwaitingUserAcceptance, SafeFixing)` 这条边——用户拒绝验收可以直接触发修复，不需要新建任务（`storage/tasks.rs:68`）。

**修正（上一版第3.4节有误，本版更正）：** 本规格新增的全部"自检门禁未通过"分支（`SelfCheckMissing`/`SelfCheckInvalidFormat`/`SelfCheckNotComplete`/`RequirementCoverageIncomplete`/`UnfinishedItemsPresent`/`BlockingIssuesPresent`）**只在任务已经处于`AwaitingUserAcceptance`、且正在尝试转入`Completed`时才会被求值**——因为完工门禁只挂在这一条边上，不存在"任务还在PreviewReady就被这些分支拦下来"的情况（`PreviewReady → AwaitingUserAcceptance`这条边本身不受自检门禁影响，与M2的要求一致）。所有"Blocked"结果统一走已经合法存在的`(AwaitingUserAcceptance, NeedsUserAction)`边，不需要新增任何转换边。

**`DevActionNotSucceeded`/`NoAdoptedEvidence`的定位更正：** 这两个分支理论上不会在真实合法路径下触发（因为能走到`AwaitingUserAcceptance`，`adopted_passing_generation_round()`必然已经非空、对应的dev action必然已经Succeeded——这是到达这个状态的前提条件，不是需要另外证明的事），本规格保留它们作为**防御性的"永不应该发生"断言**（若真的触发，说明别处有bug，不是正常业务分支），而不是第3.4节列出的正常业务状态落点之一。

### 5.2 用户拒绝后回到`SafeFixing`与自检门禁的关系

`AwaitingUserAcceptance → SafeFixing`（用户"不通过"）这条路径**不经过**本规格新增的完工门禁（门禁只挂在→Completed这一条边），用户拒绝本身不受自检状态影响，与现有"无测试/无构建授权规则和一次修复上限"（`MAX_AUTO_FIX_CYCLES`/`current_pending_round`）完全不冲突——本规格不改变、不触碰这两套既有机制。**注意：** `MAX_AUTO_FIX_CYCLES = 3`（`storage/tasks.rs:88`）是对"非用户主动触发"的safe_fixing重入次数的通用安全上限，不等同于产品侧"只做一次自动修复"的业务规则（后者由`current_pending_round`只识别round∈{1,2}来实际落实）——两者是不同层面的限制，本规格不重新设计，如实记录这一区分，避免误读。

### 5.3 原子性——事务而非单条SQL（回应PM"不要求强行塞进单条复杂SQL"）

**代码依据：** `storage/tasks.rs:298`的`transition_task`已经用`self.conn().execute("BEGIN IMMEDIATE;", [])?`开启一个事务，内部的状态合法性检查、`result_generation`递增、状态写入、`task_state_events`插入都在**同一个事务**里完成，不是单条SQL独自承担全部逻辑。

**决定（修正上一版"必须塞进一条UPDATE语句"的说法）：** `completion_gate_for_current_result()`的全部判定（自检有效性、coverage完整性、用户确认）**在同一个`BEGIN IMMEDIATE`事务内完成**：先在事务内重新执行一次只读查询拿到最新的self_checks/coverage/user_acceptance状态，紧接着在同一事务内执行状态写入的`UPDATE`，中间不释放事务、不被其他连接insert。这与现有`transition_task`的事务边界完全一致，不需要把判定逻辑硬塞进`UPDATE`语句的`WHERE`子句（那是上一版过度追求形式一致导致的不必要约束）。

**并发验证方案（维持）：** 构造测试：在同一事务内完成判定与写入后提交；另起一个并发连接在该事务提交前尝试修改`self_checks`/`task_actions`的`attempt_number`——由于`BEGIN IMMEDIATE`持有写锁，并发连接必须等待本事务提交才能写入，不存在两者交错的可能，天然避免TOCTOU（这与SQLite`BEGIN IMMEDIATE`的锁语义一致，不是新发明的机制）。

---

## 6. B1–B5/M1–M4完工必要条件总览（更新版，整合第5节的边界澄清）

与上一版第3.2节逐条一致（self_reported_status/coverage/数组空/用户确认四项），唯一更新：全部判定只在"任务处于`AwaitingUserAcceptance`尝试转入`Completed`"这一个场景下发生，不再描述成可能发生在PreviewReady阶段。

---

## 7. 验收矩阵（回应第6项，全表重写，新增PM列出的全部场景）

| # | 场景 | 前置状态 | 输入 | 调用入口 | 预期状态 | 持久化结果 | 拒绝原因 |
|---|---|---|---|---|---|---|---|
| 1 | 有效完成（happy path） | `AwaitingUserAcceptance` | self_checks: valid/complete/两数组空/coverage与`confirmed_requirement_items`完全一致；user_acceptance已确认 | 完工命令 | `Completed` | 事务提交 | — |
| 2 | self_reported_incomplete+空数组 | 同上 | — | 同上 | `NeedsUserAction` | 回滚，无状态变化 | `SelfCheckNotComplete` |
| 3 | self_reported_blocked+空数组 | 同上 | — | 同上 | `NeedsUserAction` | 同上 | `SelfCheckNotComplete` |
| 4 | complete+unfinished非空 | 同上 | — | 同上 | `NeedsUserAction` | 同上 | `UnfinishedItemsPresent` |
| 5 | complete+blocking非空 | 同上 | — | 同上 | `NeedsUserAction` | 同上 | `BlockingIssuesPresent` |
| 6 | coverage漏掉`confirmed_requirement_items`里的一项 | 权威集合3项，coverage只报2项 | — | 同上 | `NeedsUserAction` | 同上 | `RequirementCoverageIncomplete` |
| 7 | coverage重复同一项 | — | — | 同上 | `NeedsUserAction` | 同上 | `RequirementCoverageIncomplete` |
| 8 | coverage伪造额外项 | — | — | 同上 | `NeedsUserAction` | 同上 | `RequirementCoverageIncomplete` |
| 9 | coverage存在partial/not_covered | — | — | 同上 | `NeedsUserAction` | 同上 | `RequirementCoverageIncomplete` |
| 10 | 空壳/只有标题 | — | 结构化区块无实质内容 | 同上 | `NeedsUserAction` | `parse_status=invalid_format` | `SelfCheckInvalidFormat` |
| 11 | 格式错误 | — | JSON/YAML语法错误 | 同上 | `NeedsUserAction` | 同上 | `SelfCheckInvalidFormat` |
| 12 | 旧attempt（真实重试后attempt_number已变） | — | — | 同上 | `NeedsUserAction` | 查询天然查不到当前attempt的行 | `SelfCheckMissing` |
| 13 | 修复后旧确认失效 | round2通过后adopted_round=2 | round1的self_checks+旧user_acceptance仍在表里 | 同上 | `NeedsUserAction`直至新自检产出，`AwaitingUserAcceptance`直至重新确认 | 旧行因round/result_generation不匹配被天然忽略 | `SelfCheckMissing`→之后`UserAcceptanceMissing` |
| 14 | 重启恢复（round完成但自检未产出前重启） | — | 恢复流程不触发AI、不借旧attempt | 恢复路径 | 维持恢复前等待状态 | 无新self_checks行 | `SelfCheckMissing` |
| 15 | 用户确认绕过 | — | 直接调用确认命令 | 服务端确认命令 | `user_acceptance`写入成功，但`completed`转换仍按本节条件判定 | 视其他条件而定 | 视具体缺失条件 |
| 16 | 直接状态转换绕过 | — | 静态代码检查 | — | 断言：全仓库对`tasks.state`写`completed`只有`storage/tasks.rs:319`一处 | — | 测试断言唯一入口（本轮已核验，见第5.3节代码依据） |
| 17 | 真实证据门禁回归 | self_checks全部有效但测试/构建未通过 | — | — | 停在证据失败对应的既有状态（未改变，不会先到AwaitingUserAcceptance） | — | 不适用（本门禁尚未被求值） |
| 18 | 一次修复上限回归 | — | — | — | round上限仍为2，`MAX_AUTO_FIX_CYCLES`现状不变 | — | — |
| 19 | 自检有效+证据通过但无用户确认 | `AwaitingUserAcceptance` | — | 同上 | 停在`AwaitingUserAcceptance`（不是`NeedsUserAction`，因为用户还没拒绝，只是没确认） | — | `UserAcceptanceMissing` |
| 20 | 可预览不可完工（M2对应） | `PreviewReady`/`AwaitingUserAcceptance` | 无自检 | `preview_ready`判定 vs `completed`判定分别调用 | `preview_ready`已达成，`completed`被拒 | — | `preview_ready`不受影响；`completed`：`SelfCheckMissing` |
| 21 | 旧valid后新invalid（M1新增场景） | 同attempt两次解析 | 第一次valid，第二次invalid | 同上 | `NeedsUserAction` | `rowid DESC LIMIT 1`取到新invalid行 | `SelfCheckInvalidFormat` |
| 22 | 同时间多行（M1新增场景） | 同attempt两次解析时间戳相同 | — | 同上 | 按`rowid`而非`created_at`稳定排序 | 取到插入顺序更晚的一行 | 视该行内容而定 |
| 23 | 旧解析晚到（M1/B4新增场景） | 旧attempt的解析在新attempt产生后才写入 | — | 同上 | 不受影响 | `WHERE`按当前attempt过滤，晚到的旧attempt行查不到 | 若当前attempt无自检，仍是`SelfCheckMissing` |
| 24 | 幂等重解析（M1新增场景） | 同attempt重复解析，内容相同 | — | 同上 | 判定结果不变 | 两行内容相同，取最新一行结果一致 | 不适用 |
| 25 | 跨generation（已转化为跨attempt，见#12） | — | — | — | — | 见#12 | — |
| 26 | 当前attempt配旧证据（新增场景） | attempt已变，但test/build evidence仍是旧的 | — | 同上 | 这种情况下`adopted_passing_generation_round()`本身会先按generation/round找evidence，与attempt无关；若evidence本身是旧generation的，`adopted_round`会相应指向旧一轮，此时查self_checks用的是`adopted_round`而非"当前attempt"，需要同时满足"该round的self_checks行的attempt_number=当前attempt" | 若不满足则`NeedsUserAction` | `SelfCheckMissing` |
| 27 | 需求多项漏掉其中一项（已并入#6） | — | — | — | — | 见#6 | — |

---

## 8. 本文件状态声明

本文件是**规格设计稿**。`confirmed_requirement_items`表、`self_checks`表、`completion_gate_for_current_result()`、事务化门禁、前端展示、全部测试**均未实现**。所有代码引用均为核验现状的只读结果。需经 Codex 独立复审通过后，才能转入编码工作包（WP2A-0先行，WP2A主体随后）。
