# WP2A：结构化自检契约与完工门禁规格说明（第二版，回应 CODEX_WP2A_SPEC_REVIEW10141205.md FAIL）

**与上一版关系：** 本文件是 `docs/release-v0.1.0/NOVA_WP2A_SELF_CHECK_SPEC10140153.md` 的修订版，原文件保留，冲突处以本文件为准。本版逐条回应 `D:\nova-review\CODEX_WP2A_SPEC_REVIEW10141205.md` 的 5 个 Blocker（B1–B5）、4 个 Major（M1–M4）、2 个 Minor（m1–m2）。
**状态：** 规格设计稿，**未实现任何代码**。本文件所有"受影响函数/字段/行号"均为对 `D:\nova` 当前代码的**只读核验结果**（核验方法与引用行号见各节"代码依据"），不是方案假设；凡本轮未能找到对应代码事实的内容，本文件明确写"无此字段/无此机制"，不留"编码阶段再确认"的空白。
**方案通过声明：** 本文件通过复审，代表"方案可实施"，**不代表功能已实现，不代表已通过运行验收**。

---

## 0. 本版逐项关闭对照表

| 编号 | 上一版问题 | 本版修订位置 |
|---|---|---|
| B1 | 完工条件未要求 `self_reported_status = self_reported_complete` | 第3.2节新增独立必要条件 |
| B2 | `requirement_coverage` 完整性无门禁，权威集合未定义 | 第1.2节重新定义为固定字段集合，第3.2节新增完整性校验 |
| B3 | 自检契约按`(result_generation, round)`匹配用户确认，但schema无round | 第1.1/4.1节选定方案：不改schema，改为"按当前被采纳(generation,round)与自检round比对"，解释清楚 |
| B4 | `run_id`只存储不校验，dev action key无generation | 第1.1节重新定义键为`(task_id, round, dev_action_attempt_number)`，第3.2/4.5节给出实际校验逻辑 |
| B5 | 验收矩阵首行遗漏用户确认 | 第6节全表重写，每个完成态场景显式含用户确认 |
| M1 | 写入策略自相矛盾 | 第2.3节选定"追加写入+按created_at取最新一行"语义，明确区别于confirmations和evidence两种既有模式 |
| M2 | 挂载点可能破坏preview_ready语义 | 第3.1/3.3节：不改动`adopted_passing_generation_round()`，新增独立函数 |
| M3 | 原子性要求不具体 | 第5节：复用`storage/tasks.rs`现有`UPDATE...WHERE...EXISTS`原子模式，给出实际SQL形态 |
| M4 | 历史证据源留白"编码阶段确认" | 对应文档`NOVA_SINGLE_AI_MIGRATION_PLAN`本轮同步修订，列出已核验证据源表，不留白 |
| m1 | 同构表述不准确 | 第2节明确：self_checks是第三种独立语义，不等同confirmations也不等同evidence |
| m2 | "needs_user_action或更早等待状态"过于开放 | 第3.4节固定每个失败点的具体落点状态 |

---

## 1. 结构化契约定义（修订 B3、B4）

### 1.1 关联键——重新定义（回应B3、B4的代码事实）

**代码依据：**
- `UserAcceptance` 结构体只有 `id/project_id/task_id/result_generation/accepted/user_note/accepted_at/created_at`，**没有`round`字段**（`models.rs:650-664`）。
- `completed`门禁的原子SQL只比较 `ua.result_generation = tasks.result_generation`（`storage/tasks.rs:317-333`），同样不涉及round。
- `result_generation`在生产代码里**只有一处真实递增点**：任务转入`PreviewReady`状态时，在同一事务内执行`UPDATE tasks SET result_generation = result_generation + 1`（`storage/tasks.rs:364-369`），且注释明确说明这正是"让旧确认记录对新结果失效"的机制。
- 开发/修复的Action Ledger key是`format!("dev-{task_id}-round-{round}")`（`core_loop.rs:1553`），**不含generation**；与之相对，测试/构建key是generation-aware的`test-{task_id}-generation-{result_generation}-round-{round}`/`build-{task_id}-generation-{result_generation}-round-{round}`（`core_loop.rs:2040-2041`、`core_loop.rs:2368-2369`）。
- `task_actions`表有`attempt_number`字段，且在重试/恢复路径上会真实递增（`storage/actions.rs:80`、`198`、`229`；测试`storage/actions.rs:391,452,469`验证了递增行为）。

**决定（不留给编码阶段二选一）：**

1. **不给`user_acceptance`加`round`列，不做迁移。** 一致性改为依赖已经存在、已经原子化的机制：generation只在进入`PreviewReady`时才递增，所以"当前`result_generation`"本身就唯一对应"当前这一次通过了真实证据的那个(generation,round)"——不需要在user_acceptance里重复记一个round。
2. **`self_checks`的关联键是 `(task_id, round, dev_action_attempt_number)`，不含`result_generation`列。** 原因：dev action key本就不含generation，给self_checks强行编一个generation列只是假精度，不代表真实可核验的归属。真正能证明"这份自检对应哪一次开发/修复执行"的，是`task_actions`里`dev-{task_id}-round-{round}`这一行**在自检被解析时的`attempt_number`**——这是已存在、已测试、会在重试时真实变化的字段，直接复用，不新增ID体系（回应B4"不要新增没有必要的复杂ID系统"）。
3. **完工判定时如何把"自检的round"和"被采纳的(generation,round)"对上：** 完工判定（第3节）先调用既有`adopted_passing_generation_round()`得到`(adopted_generation, adopted_round)`，再用`adopted_round`去查`self_checks`——两边通过`round`值自然对齐，不需要self_checks再存一份generation。`run_id`字段的具体定义见下。

### 1.2 内容字段

| 字段 | 说明 |
|---|---|
| `requirement_coverage` | 结构化列表，**权威key集合固定为**：任务创建时非空的 `{goal, in_scope, not_in_scope, done_criteria}` 子集（见第1.3节代码依据），每条含`{item: "goal"\|"in_scope"\|"not_in_scope"\|"done_criteria", status: covered\|partial\|not_covered, note}` |
| `check_results` | 结构化列表，开发AI自己执行过的检查项与结果 |
| `unfinished_items` | 字符串列表，可为空数组，不可省略字段本身 |
| `blocking_issues` | 字符串列表，可为空数组，不可省略字段本身 |
| `self_reported_status` | 枚举：`self_reported_complete` \| `self_reported_incomplete` \| `self_reported_blocked`，见第3.2节作为**独立必要条件**，不与数组字段互相替代 |

### 1.3 `requirement_coverage`的权威集合——重新定义（回应B2）

**代码依据：**
- `Task`结构体的`goal/in_scope/not_in_scope/done_criteria`均为`Option<String>`自由文本字段（`models.rs:319-322`），`REQUIREMENT.md`直接由这四个字段拼接生成，没有任何更细粒度的稳定item id（`core_loop/documents.rs:103-113`的`requirement_doc()`函数）。
- 全仓库核验：`commands.rs`/`storage/tasks.rs`**没有任何命令可以在任务创建后修改这四个字段**（只有`create_task`在创建时一次性写入，核验方法：对`commands.rs`、`storage/tasks.rs`搜索`update_task`/`edit_task`/等价命令，均无匹配）。

**决定（不留多个候选方案）：**

- **权威item集合 = 任务创建时非空的`{goal, in_scope, not_in_scope, done_criteria}`子集，在任务整个生命周期内固定不变。** 不新增独立的验收项数组表，不做版本化/快照机制——因为当前代码根本不存在"需求被编辑"这个场景，版本化是在为不存在的功能预先设计，不是最小方案。
- **"需求变化后失效"规则：** 在现有命令集合下，这个场景**不会发生**。若未来新增需求编辑命令，该命令必须同时要求任务退回到需要重新产出自检的状态（例如强制新建一轮或新建任务），这是那个未来功能自己要解决的设计问题，不是本规格需要现在解决的问题——本规格明确声明这一点，不假装已经设计了一个不存在的场景的方案。
- **完工门禁要求（第3.2节细化）：** `requirement_coverage`（去重后）的key集合必须与上述权威集合**完全相等**（无缺项、无多余伪造项、无重复），且**每一条的`status`必须是`covered`**（出现任意一条`partial`/`not_covered`即拒绝），列表本身不能为空（若权威集合本身为空——即任务创建时四个字段全部留空——这是创建阶段应该阻止的异常情况，完工门禁遇到这种任务时同样拒绝并给出"任务未填写任何需求字段"的原因，不尝试自检）。

---

## 2. 存储设计（修订 M1、m1）

### 2.1 `self_checks`是第三种独立语义，不类比任何既有表（回应m1）

**代码依据对比：**
- `no_tests_confirmations`/`no_build_confirmations`：`INSERT ... ON CONFLICT(task_id, result_generation, round) DO NOTHING`（`storage/no_tests_confirmation.rs:49-53`、`storage/no_build_confirmation.rs:57-60`）——**先写入的赢**，同一key后续写入被静默忽略。这种语义**不适合**自检：如果开发AI先交了一份有阻断问题的自检，后来文档重新解析/修正后又交了一份干净的自检，"先写入的赢"会让错误的第一份永久卡住门禁。
- `test_evidence`/`build_evidence`：无唯一约束，同一`(task_id, result_generation, round)`可以有多行，查询返回全部行按`created_at ASC`排序，判定逻辑是"任意一行status=passed即算过"（`adopted_passing_generation_round()`的实现本身，`core_loop/documents.rs:237-293`）——**任意一行通过就算通过**。这种语义也**不适合**自检：如果第一份自检解析错误地标成了`self_reported_complete`，后来Nova发现解析其实该标为`invalid_format`并重新写入一行，"任意一行通过就算通过"会让那条错误的旧记录继续让门禁通过，正是Codex指出的"无效的新解析不能让旧有效记录继续生效"这个要求的反面。

**`self_checks`选定的语义（第三种，明确不同于以上两种）：追加写入，不删除历史行；"当前判定"永远取同一key下`created_at`最新的一行，新行天然覆盖旧行的判定效力，不依赖UNIQUE约束。**

```
self_checks(
  id, project_id, task_id, round, dev_action_attempt_number,
  parse_status,              -- 'valid' | 'invalid_format'
  requirement_coverage_json,
  check_results_json,
  unfinished_items_json,
  blocking_issues_json,
  self_reported_status,      -- 仅当parse_status='valid'时有意义
  raw_block_redacted,
  created_at
)
-- 无UNIQUE约束；查询时固定用：
-- SELECT * FROM self_checks
--   WHERE task_id=? AND round=? AND dev_action_attempt_number=?
--   ORDER BY created_at DESC LIMIT 1
-- 取到的这一行就是"当前判定"，不参考同key下更早的行。
```

**历史保留规则与覆盖写入规则不矛盾（回应M1"不能矛盾"）：** 历史行从不删除（供审计/技术详情展示用），但"当前判定"的查询规则本身只看最新一行——"保留历史"与"新行覆盖旧行的判定效力"是两件不冲突的事：前者是存储策略，后者是查询策略，不是同一回事。

### 2.2 历史任务兼容

与上一版一致：`self_checks`是v0.1.0新增概念，历史任务没有对应行，不回填、不假造历史记录。完工门禁对迁移前已完工的历史任务不回溯要求（第3.4节细化）。

---

## 3. 完工门禁改动（修订 B1、M2）

### 3.1 新增独立函数，不改动`adopted_passing_generation_round()`（回应M2）

**代码依据：** `adopted_passing_generation_round()`当前只查测试/构建证据，返回`Option<(generation, round)>`；`FINAL_RESULT`文档生成和证据展示都复用这同一个函数判断"是否有可用的通过证据"（`core_loop/documents.rs:237-293`、`367-388`）。

**决定：** 保留该函数不变。新增一个独立函数（建议命名`completion_gate_for_current_result()`，具体命名留给编码阶段，但职责边界本规格固定）：

```
fn completion_gate_for_current_result(db, project_id, task_id, task) -> NovaResult<CompletionGateResult>

CompletionGateResult 为以下其一：
  Ready                                   -- 可以进入 completed
  Blocked(BlockReason)                    -- 不能进入 completed，附具体原因（见3.4节固定列表）

内部逻辑：
  1. (adopted_generation, adopted_round) = adopted_passing_generation_round(...)?
     若为 None → Blocked(NoAdoptedEvidence)   -- preview_ready都没达到，不讨论自检
  2. 取 task_actions 中 dev-{task_id}-round-{adopted_round} 的当前 attempt_number（记为 current_attempt）
     若该 action 不存在，或 status != Succeeded，或 action_type != "ai_development"
       → Blocked(DevActionNotSucceeded)
  3. 查 self_checks WHERE task_id=? AND round=adopted_round AND dev_action_attempt_number=current_attempt
     ORDER BY created_at DESC LIMIT 1
     若无行 → Blocked(SelfCheckMissing)        -- 包括"曾有自检但attempt已变"的情况，因为attempt变了就查不到
     若 parse_status != 'valid' → Blocked(SelfCheckInvalidFormat)
     若 self_reported_status != 'self_reported_complete' → Blocked(SelfCheckNotComplete)   -- B1
     若 requirement_coverage 的key集合 != 权威集合，或存在重复/多余/非covered → Blocked(RequirementCoverageIncomplete)   -- B2
     若 unfinished_items 非空 → Blocked(UnfinishedItemsPresent)
     若 blocking_issues 非空 → Blocked(BlockingIssuesPresent)
  4. 查 user_acceptance WHERE task_id=? AND result_generation = tasks.result_generation AND accepted=1
     若无行 → Blocked(UserAcceptanceMissing)     -- B5
  5. 全部通过 → Ready
```

### 3.2 必要条件总览（逐条对应PM/Codex要求）

进入`completed`必须同时满足（"与"关系，不是"或"）：

1. `adopted_passing_generation_round()`返回非空（真实测试/构建证据通过）——**未改变既有逻辑**。
2. 该`(adopted_generation, adopted_round)`对应的开发/修复action真实成功，且当前`self_checks`判定行与该action的**当前**`attempt_number`严格匹配（B4）。
3. 该`self_checks`判定行`parse_status = 'valid'`。
4. 该行`self_reported_status = 'self_reported_complete'`（B1，与下面两条独立，不互相替代）。
5. 该行`unfinished_items`为空数组。
6. 该行`blocking_issues`为空数组。
7. 该行`requirement_coverage`的key集合与权威集合完全一致、无重复、无伪造、全部`covered`（B2）。
8. 存在`accepted=1`的`user_acceptance`行，且其`result_generation`等于任务当前`result_generation`（B5，复用既有机制，未新增字段）。

### 3.3 `preview_ready`不受影响（回应M2）

`preview_ready`的进入条件完全不变，继续只由`adopted_passing_generation_round()`是否为`Some`决定，**不调用**`completion_gate_for_current_result()`。文档/界面展示"可预览"与"可完工"必须分别调用这两个函数，不得共用同一个布尔值或互相替代。

### 3.4 失败点状态落点——固定列表，不用"或更早状态"（回应m2）

| `BlockReason` | 界面/状态落点（固定，不是"或"） |
|---|---|
| `NoAdoptedEvidence` | 任务停在当前真实状态（`running`/`verifying`/`safe_fixing`之一，由证据本身决定，不是本门禁引入的新状态） |
| `DevActionNotSucceeded` | `needs_user_action`，原因文案="开发/修复执行记录异常，无法确认归属" |
| `SelfCheckMissing` | `needs_user_action`，原因文案="缺少当前轮次的自检记录" |
| `SelfCheckInvalidFormat` | `needs_user_action`，原因文案="自检记录格式无效，无法解析" |
| `SelfCheckNotComplete` | `needs_user_action`，原因文案="开发AI自检认为尚未完成或被阻断" |
| `RequirementCoverageIncomplete` | `needs_user_action`，原因文案="自检的需求覆盖记录不完整或不一致" |
| `UnfinishedItemsPresent` | `needs_user_action`，原因文案="自检记录中存在未完成项" |
| `BlockingIssuesPresent` | `needs_user_action`，原因文案="自检记录中存在阻断问题" |
| `UserAcceptanceMissing` | 停在`awaiting_user_acceptance`（这是现状，未改变——用户还没确认，本来就该停在这里） |

**固定规则：** 除`UserAcceptanceMissing`（现状本就该停在`awaiting_user_acceptance`）和`NoAdoptedEvidence`（现状本就该停在证据产生前的真实运行状态）外，其余全部落在`needs_user_action`，不使用任何"或更早的等待状态"这类开放表述。

---

## 4. B3/B4 的进一步说明（修订对应）

### 4.1 修复后旧确认/旧自检如何失效（回应B3、PM第5条）

- **旧`user_acceptance`失效：** 已经是现状——generation在进入`PreviewReady`时自动+1（`storage/tasks.rs:364-369`），旧的`user_acceptance`行的`result_generation`永远对不上新的`tasks.result_generation`，`EXISTS`子句天然查不到，不需要本规格新增任何逻辑。
- **旧`self_checks`失效：** 修复（round从1变2，或同一round内发生真实重试）会改变`task_actions`里对应dev key的`attempt_number`；完工门禁第3.1节第3步永远用**当前**`attempt_number`去查，旧attempt对应的self_checks行永远查不到，自然失效。**两种失效都是"键不匹配天然失效"，不依赖额外的"是否过期"布尔字段**——这是故意的设计，避免"忘记清空一个过期标记"这类实现错误。

### 4.2 `run_id`的最终定义（回应B4）

废除上一版"run_id=dev action key，仅用于审计"的说法。**`run_id`概念由`(round, dev_action_attempt_number)`二元组承担实际校验职责**，不是一个仅供审计的字符串。本规格不新增复杂ID系统，直接复用`task_actions.attempt_number`这个已存在、已测试的字段。

---

## 5. 原子性（修订 M3）

**代码依据：** 现有`completed`门禁已经是原子的——`storage/tasks.rs:317-333`把"状态合法性"与"存在已确认的user_acceptance行"放进**同一条**`UPDATE tasks SET state=? WHERE id=? AND state=? AND (...)`语句的`WHERE`子句里，而不是先`SELECT`再`UPDATE`，原注释明确解释这是为了防止"两个并发的completed尝试都读到一份旧判断然后都成功"的TOCTOU竞态。

**决定：** `completion_gate_for_current_result()`在第3.2节描述的全部条件，**必须以同样的方式**折进这同一条`UPDATE...WHERE`语句（新增若干`EXISTS`子查询，查`self_checks`按当前`attempt_number`、查`requirement_coverage`完整性、查`user_acceptance`），**不允许**先调用`completion_gate_for_current_result()`拿到`Ready`结果、再单独调用一次`transition_task(..., Completed)`——中间间隔会重新打开TOCTOU窗口。`completion_gate_for_current_result()`本身可以作为"提前给用户/界面一个预判"的只读函数单独存在（用于UI提示"为什么现在不能确认"），但**真正写入`completed`的唯一权威路径**必须是这一条扩展后的原子SQL，`completion_gate_for_current_result()`的只读判断不能替代它。

**并发验证方案：** 构造测试：在调用方读到`completion_gate_for_current_result() = Ready`之后、真正执行`UPDATE`之前，另一个并发写入使`self_checks`的`attempt_number`发生变化（真实重试）或`requirement_coverage`被新的`invalid_format`行覆盖判定——断言这条`UPDATE`因为`WHERE`子句的`EXISTS`重新求值而返回0行，`completed`转换失败，不会用"旧的、已经过期的Ready判断"写入。

---

## 6. 验收矩阵（修订 B1、B5，全表重写）

| # | 场景 | 输入/前置状态 | 调用入口 | 预期状态 | 持久化结果 | 拒绝原因 |
|---|---|---|---|---|---|---|
| 1 | 有效完成（happy path） | 真实证据通过+self_checks: valid/self_reported_complete/两数组空/coverage完整+user_acceptance已确认 | `completed`原子UPDATE | `completed` | 无新拒绝记录 | — |
| 2 | 自检`self_reported_incomplete`+两数组为空 | 同上但`self_reported_status=self_reported_incomplete` | 同上 | `needs_user_action` | 门禁拒绝，无状态变化 | `SelfCheckNotComplete` |
| 3 | 自检`self_reported_blocked`+两数组为空 | 同上但为`self_reported_blocked` | 同上 | `needs_user_action` | 同上 | `SelfCheckNotComplete` |
| 4 | 自检`self_reported_complete`但`unfinished_items`非空 | — | 同上 | `needs_user_action` | 同上 | `UnfinishedItemsPresent` |
| 5 | 自检`self_reported_complete`但`blocking_issues`非空 | — | 同上 | `needs_user_action` | 同上 | `BlockingIssuesPresent` |
| 6 | `requirement_coverage`空列表 | — | 同上 | `needs_user_action` | 同上 | `RequirementCoverageIncomplete` |
| 7 | `requirement_coverage`漏项（权威集合4项只报3项） | — | 同上 | `needs_user_action` | 同上 | `RequirementCoverageIncomplete` |
| 8 | `requirement_coverage`重复项 | 同一item出现两次 | 同上 | `needs_user_action` | 同上 | `RequirementCoverageIncomplete` |
| 9 | `requirement_coverage`额外伪造项（如虚构一个不在权威集合里的item） | — | 同上 | `needs_user_action` | 同上 | `RequirementCoverageIncomplete` |
| 10 | `requirement_coverage`存在`partial` | — | 同上 | `needs_user_action` | 同上 | `RequirementCoverageIncomplete` |
| 11 | `requirement_coverage`存在`not_covered` | — | 同上 | `needs_user_action` | 同上 | `RequirementCoverageIncomplete` |
| 12 | 空壳/只有标题 | 起止标记内无实质内容 | 同上 | `needs_user_action` | `parse_status=invalid_format` | `SelfCheckInvalidFormat` |
| 13 | 格式错误（JSON/YAML语法错误） | — | 同上 | `needs_user_action` | 同上 | `SelfCheckInvalidFormat` |
| 14 | 任务不匹配（自检行`task_id`与当前任务不一致） | 防御性场景 | 同上 | `needs_user_action` | 查询天然查不到（WHERE含task_id） | `SelfCheckMissing` |
| 15 | 旧generation（本设计已不给self_checks存generation列，此场景转化为"旧attempt"，见#16） | — | — | — | — | 见#16 |
| 16 | 旧round/旧attempt（self_checks行对应的`attempt_number`不是当前的） | 真实重试后`attempt_number`已递增 | 同上 | `needs_user_action` | 查询天然查不到当前attempt的行 | `SelfCheckMissing` |
| 17 | 修复后失效（round1自检+round1确认都存在，之后round2修复） | round2通过证据后`adopted_round=2` | 同上 | `needs_user_action`（直到round2产出新自检）或`awaiting_user_acceptance`（直到重新确认） | round1的self_checks行因`round`不匹配被天然忽略；旧`user_acceptance`因`result_generation`不匹配被天然忽略 | `SelfCheckMissing`，确认后再检查`UserAcceptanceMissing` |
| 18 | 重启恢复（round完成但自检未产出前重启） | 恢复流程不触发AI、不借用旧generation/round | `storage/recovery.rs`恢复路径 | 保持恢复前的等待状态，不自动completed | 无self_checks新行 | `SelfCheckMissing` |
| 19 | 用户确认绕过（直接调用确认命令，不经前端按钮） | — | 服务端`accept`命令 | 命令本身允许记录确认（这是`user_acceptance`自己的写入，不是completed门禁），但后续`completed`原子UPDATE仍按第3.2节条件判定 | `user_acceptance`行写入成功，但`completed`转换若其他条件不满足仍被拒绝 | 视具体缺失条件而定 |
| 20 | 直接状态转换绕过（代码层面任何试图跳过门禁函数直接写`state=completed`的路径） | 静态代码检查 | — | 断言：全仓库对`tasks`表写`state`为`completed`的SQL只有第5节描述的那一条扩展后的原子UPDATE | — | 测试断言唯一入口 |
| 21 | 真实证据门禁回归 | self_checks全部有效，但测试/构建证据未通过 | 同上 | 任务停在现有的失败/修复状态（未改变） | — | `NoAdoptedEvidence` |
| 22 | 一次修复上限回归 | — | — | round上限仍为2，本门禁不引入新重试循环 | — | — |
| 23 | 自检有效+证据通过但无用户确认（B5新增） | — | 同上 | `awaiting_user_acceptance` | — | `UserAcceptanceMissing` |
| 24 | 无自检但测试/构建已通过——可预览不可完工（M2对应用例） | — | `preview_ready`判定 vs `completed`判定分别调用 | `preview_ready`达成，`completed`被拒绝 | — | `preview_ready`: `NoAdoptedEvidence`不适用（证据已过）；`completed`: `SelfCheckMissing` |

---

## 7. 本文件状态声明

本文件是**规格设计稿**，`self_checks`表、`completion_gate_for_current_result()`、扩展后的原子`UPDATE`、前端展示、全部测试**均未实现**。所有代码引用均为核验现状的只读结果，不代表已被修改。需经 Codex 独立复审通过后，才能转入编码工作包。
