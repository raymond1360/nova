# WP2A 方案补齐说明（回应PM本轮指出的7项遗漏）

**性质声明：** 本文件只修订方案文档，**未修改任何产品代码**。"补齐/关闭"指方案层面已给出具体设计，**不代表功能已实现，不代表已通过运行验收，也不代表已通过Codex复审**——下一步仍需交Codex独立复审。
**保存位置变更：** 按PM本轮要求，本文件保存到 `D:\nova\docs\reports\`（与前两轮的 `docs/release-v0.1.0/` 不同）；规格与迁移计划仍保存在 `D:\nova\docs\release-v0.1.0\`。未移动、未删除任何旧位置的历史文件（包括`D:\nova-review\`下的既有审核报告）。
**基线：** 分支`feature/v0.1.0-mvp`，HEAD `6a90df951c6c72ab7ae95228d5f2cc4de7e17273`，tag `milestone/evidence-gate`，本轮未产生任何代码/配置改动。

---

## 逐条补齐说明

### 1. 需求覆盖细粒度

**源码依据：** `models.rs:319-322`（四个自由文本字段）、`core_loop/documents.rs:103-113`（无子item结构）、`apps/desktop/src/stores/coreLoop.ts:468-476`（"确认需求"只是通用`transition_task`，非专门函数）、`storage/tasks.rs:364-369`（`transition_task`已有"目标状态附加逻辑"先例，可复用其模式）。
**方案：** 新增`confirmed_requirement_items`表，在`transition_task`的`to==AwaitingConfirmation`分支里按换行分割四个字段生成细粒度需求项并快照，一次性、不可变。自检`requirement_coverage`门禁改为对照这张表的全部`(field, item_index)`组合，而非笼统的4个字段名。
**验收用例：** `NOVA_WP2A_SELF_CHECK_SPEC10141243.md` 第7节#6–#9。
**工作包边界：** 独立为WP2A-0，须先于WP2A主体落地。

### 2. 历史分类矛盾与Action Ledger完整核验

**源码依据：** 本轮完整读完`run_core_loop_dual_ai_v0_2`（`core_loop.rs:323-471`）、`run_one_round`、`run_review_round`（`core_loop.rs:1685-1802`）全部action写入调用，确认开发步骤与单AI路径完全复用同一函数（不可区分），审核步骤独有`action_type="ai_review"`、key`review-{task_id}-round-{round}`（`core_loop.rs:1697-1698`，任何执行结果都会留痕）。
**方案：** 重新定义分类算法——"双AI"由三项独立目击证据（ai_review action/task_reviews行/审核文档实质内容）任一命中即可判定（OR逻辑，三者是同一事件的不同记录渠道，不要求同时命中）；"单AI"需要正向证据（开发成功+以上三项全部未命中+已到达终态或待验收）；两类证据源互相矛盾（如一个说有一个说无）才归unknown，与"数据不足"（schema历史版本证据不全）分开报告，不混为一谈。
**修订位置：** `docs/release-v0.1.0/NOVA_SINGLE_AI_MIGRATION_PLAN10141246.md` 第2节全重写。
**无遗留"编码阶段确认"事项。**

### 3. 自检写入顺序

**源码依据：** 本表所有`CREATE TABLE`均为`id TEXT PRIMARY KEY`（非`INTEGER PRIMARY KEY`），确认SQLite隐藏`rowid`独立存在且随插入单调递增，不需要新列。
**方案：** 排序改为`WHERE task_id=? AND round=? AND dev_action_attempt_number=? ORDER BY rowid DESC LIMIT 1`——先用attempt_number做强过滤（解决跨attempt的"晚到"问题），`rowid`排序只在同一attempt被重复解析时才实际发挥作用（解决同一attempt内的"最后到达"问题），两者分工明确，不是同一个问题的两次尝试。
**验收用例：** `NOVA_WP2A_SELF_CHECK_SPEC10141243.md` 第7节#21–#24。

### 4. 当前结果一致性全链路

**源码依据：** 本轮完整追踪`run_dev_and_test_round`→`run_tests_for_round`/`run_build_for_round`→`transition_task(PreviewReady)`的真实执行顺序，确认：evidence写入时`result_generation`尚未递增，递增只发生在成功转入`PreviewReady`的同一事务内（`storage/tasks.rs:364-369`）；`adopted_passing_generation_round()`正是为此才需要同时检查`[当前generation, 当前generation-1]`。
**方案：** `NOVA_WP2A_SELF_CHECK_SPEC10141243.md` 第3节给出完整7步调用链，证明self_checks不需要存generation列——round+attempt_number已经通过这条链路间接、唯一地绑定到正确的generation，新增generation列是冗余字段。同时核验"round1跨generation复用"场景在当前可达命令下不会发生（无`requeue`类命令），排除了B4的理论风险在当前范围内成为真实风险。
**验收用例：** 第7节#12、#13、#26。

### 5. 状态与原子门禁

**源码依据：** `storage/tasks.rs:32-82`的`LEGAL_TRANSITIONS`表——本轮核验发现**上一版方案有误**：`(PreviewReady, NeedsUserAction)`不是合法边，只有`(AwaitingUserAcceptance, NeedsUserAction)`合法；`(AwaitingUserAcceptance, SafeFixing)`也合法（用户拒绝可直接回到修复，不需要新建任务）；`storage/tasks.rs:88`的`MAX_AUTO_FIX_CYCLES=3`是非用户触发重入的通用安全上限，不等同于产品"一次修复"规则（后者由`current_pending_round`落实）。
**方案：** 更正门禁挂载点——全部自检相关"Blocked"分支只在`AwaitingUserAcceptance→Completed`这条已存在的合法边上求值，统一落到已合法存在的`NeedsUserAction`，不新增转换边；`DevActionNotSucceeded`/`NoAdoptedEvidence`降级为防御性断言（正常路径下不会触发）。原子性方案由"必须塞进一条UPDATE"放宽为"同一个`BEGIN IMMEDIATE`事务内完成判定+写入"，与现有`transition_task`的事务边界一致，不要求形式上的单一SQL语句。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141243.md` 第5节。

### 6. 验收矩阵补项

**方案：** 第7节重写，新增：旧valid后新invalid（#21）、同时间多行（#22）、旧解析晚到（#23）、幂等重解析（#24）、当前attempt配旧证据（#26），以及更正后的状态落点（不再使用"或更早状态"类开放表述）。

### 7. 文档准确性

**更正：** 上一轮`CLAUDE_WP2A_SPEC_REMEDIATION10141223.md`标题区误写"1个Blocker、2个Major"，与`CODEX_WP2A_SPEC_REVIEW10141205.md`实际的"5个Blocker（B1-B5）+4个Major（M1-M4）+2个Minor"不符，本文件已更正并说明原因（混用了更早一轮非WP2A专项复审的数字）。全文交叉引用已核对指向实际存在的文件路径。

---

## 本轮仍未关闭/需要注意的事项（如实说明）

- WP2A-0（需求项快照）与WP2A主体是否需要分两次提交两次复审，还是合并为一次，留给PM/Codex在编码排期时决定，本文件只给出工作包边界，不代表已经做出排期决定。
- 第2节"数据不足"分类下，具体哪个迁移版本号对应哪个上线时间点，本轮未逐一核对`storage/migrations.rs`的版本历史，留待WP5实际执行历史核查时处理，不影响分类算法本身的正确性。
- 本文件与`NOVA_WP2A_SELF_CHECK_SPEC10141243.md`、`NOVA_SINGLE_AI_MIGRATION_PLAN10141246.md`均为方案文档，尚未经Codex复审，**不写"已全部解决"**。

## 范围边界重申

本轮不涉及Phase5A新建产品功能/UI、GitHub搜索或导入扩展、双AI启用、全站改造。未修改任何产品代码、配置、锁文件、数据库；未运行AI CLI；未读取真实用户数据库；未提交、未push、未发布、未建立里程碑。

## 下一步

交 Codex 对以下三份文件做独立只读复审：
1. `D:\nova\docs\reports\CLAUDE_WP2A_SPEC_REMEDIATION10141248.md`（本文件）
2. `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141243.md`
3. `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141246.md`

复审通过后，由 PM 下达 WP2A-0/WP2A 编码任务顺序。
