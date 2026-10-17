# WP2A 方案复审问题关闭说明（回应 CODEX_WP2A_SPEC_REVIEW10141205.md）

**性质声明：** 本文件只修订方案文档，**未修改任何产品代码**。"已关闭"指"方案层面已给出可实施、与当前代码事实一致的设计"，**不代表功能已实现，不代表已通过运行验收**。
**审核来源核验：** `D:\nova-review\CODEX_WP2A_SPEC_REVIEW10141205.md` 已读取确认，结论FAIL，1个Blocker（原文列了B1–B5五条，习惯上称"Blocker"）、4个Major、2个Minor。本文件逐条对应该文件的编号。
**代码核验方式：** 本轮对每一条Blocker/Major都重新读取了审核意见引用的实际源码行，未直接采信审核文字，核验结果见下表"代码依据"列（凡本轮独立确认与审核意见一致的，标注"核验一致"；凡发现需要补充核验的，如实说明，不写"已全部解决"）。
**基线：** 分支`feature/v0.1.0-mvp`，HEAD `6a90df951c6c72ab7ae95228d5f2cc4de7e17273`，tag `milestone/evidence-gate`，本轮未产生任何代码/配置改动，`git status`只多出本轮新增的文档文件。

---

## Blocker

### B1：完工门禁缺少`self_reported_status = self_reported_complete`

**代码依据：** 不适用（本条是对方案文本自身逻辑漏洞的指正，不涉及`D:\nova`现有产品代码）。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141220.md` 第3.2节第4条、第6节验收矩阵#2–#5。
**确定方案：** 新增独立必要条件`self_reported_status = 'self_reported_complete'`，与`unfinished_items`/`blocking_issues`为空数组这两条并列，三者都必须满足，不互相替代。
**验收用例：** 验收矩阵#2（incomplete+空数组→拒绝）、#3（blocked+空数组→拒绝）、#4（complete+非空unfinished→拒绝）、#5（complete+非空blocking→拒绝），四个场景均已覆盖。

### B2：`requirement_coverage`完整性无门禁

**代码依据（本轮核验）：** `models.rs:319-322`确认`goal/in_scope/not_in_scope/done_criteria`为自由文本字段；`core_loop/documents.rs:103-113`确认`REQUIREMENT.md`直接拼接这四个字段，无稳定子item id；全仓库检索`commands.rs`/`storage/tasks.rs`确认**没有任何命令能在任务创建后修改这四个字段**（核验一致，且补充发现了审核意见未提及的"这四个字段创建后不可编辑"这一事实，用于简化方案）。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141220.md` 第1.3节、第3.2节第7条。
**确定方案：** 权威item集合固定为任务创建时非空的`{goal, in_scope, not_in_scope, done_criteria}`子集，生命周期内不变（因为现有代码无法编辑它们，不需要版本化/快照）。完工门禁要求覆盖列表key集合与权威集合完全一致、无重复无伪造、全部`covered`、列表非空。
**验收用例：** 验收矩阵#6–#11，覆盖空列表、漏项、重复项、伪造项、partial、not_covered六种场景。

### B3：`user_acceptance`按`(result_generation, round)`匹配但schema无round

**代码依据（本轮核验，核验一致）：** `models.rs:650-664`确认`UserAcceptance`只有`result_generation`无`round`；`storage/tasks.rs:317-333`确认`completed`门禁只比较`result_generation`；`storage/tasks.rs:364-369`确认`result_generation`在进入`PreviewReady`时原子+1，审核意见未明确指出这一递增时机，本轮补充核验确认了这一点，并用它作为B3方案的核心依据。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141220.md` 第1.1节、第4.1节。
**确定方案（二选一中选定第二项）：** 不给`user_acceptance`加`round`列、不迁移。依赖已有的"进入PreviewReady即generation+1"机制，使旧确认天然对不上新结果；`self_checks`自己的round匹配通过与`adopted_passing_generation_round()`返回的`adopted_round`比对完成，不依赖`user_acceptance`携带round。
**验收用例：** 验收矩阵#17（修复后失效场景），说明旧确认因result_generation不匹配天然失效。

### B4：`run_id`只存储不校验，dev action key无generation

**代码依据（本轮核验，核验一致并补充细节）：** `core_loop.rs:1553`确认dev key格式`dev-{task_id}-round-{round}`无generation；`core_loop.rs:2040-2041,2368-2369`确认测试/构建key是generation-aware的；补充核验`storage/actions.rs:80,198,229`确认`attempt_number`在重试/恢复路径上真实递增，且有测试覆盖（`storage/actions.rs:391,452,469`）——审核意见提到"attempt_number"但未指出它已有递增测试，本轮确认了这一点，支撑了方案选用该字段做校验而不是新造ID体系的决定。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141220.md` 第1.1节、第3.1节第2–3步、第4.2节。
**确定方案：** `self_checks`关联键改为`(task_id, round, dev_action_attempt_number)`，不含generation列；门禁时重新读取`task_actions`中对应dev key的**当前**`attempt_number`，与`self_checks`记录比对，不匹配即视为缺失。复用既有字段，不新增ID体系。
**验收用例：** 验收矩阵#16（旧attempt场景，真实重试后旧self_checks查不到）。

### B5：验收矩阵遗漏用户确认

**代码依据：** 不适用（方案文本自身遗漏，非代码问题）。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141220.md` 第3.2节第8条、第6节验收矩阵#1、#23。
**确定方案：** 完工必要条件新增第8条"存在当前result_generation的`accepted=1`用户确认行"，与自检、真实证据并列为"与"关系。
**验收用例：** 验收矩阵#1（happy path显式含用户确认）、#23（自检有效+证据通过但无确认→拒绝，状态落`awaiting_user_acceptance`）。

## Major

### M1：`self_checks`写入策略自相矛盾

**代码依据（本轮核验，核验一致）：** `storage/no_tests_confirmation.rs:49-53`、`storage/no_build_confirmation.rs:57-60`确认`ON CONFLICT DO NOTHING`（先写赢）；`storage/migrations.rs:596-612,730-744`确认`test_evidence`/`build_evidence`无唯一约束；`storage/evidence.rs:351-364`等确认查询按`created_at ASC`返回多行、"任意一行通过即算通过"。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141220.md` 第2.1节。
**确定方案：** 选定第三种语义（明确不同于以上两种）：追加写入不删除历史，"当前判定"永远取同key下`created_at DESC LIMIT 1`——新行天然覆盖旧行的判定效力，不用UNIQUE约束，也不是"任意历史行通过即算通过"。
**验收用例：** 需新增测试"旧有效行+新invalid行→completed被拒绝"（已在第3.1节判定逻辑第3步体现，具体测试用例留给编码阶段按本节语义编写）。

### M2：挂载点可能破坏`preview_ready`语义

**代码依据（本轮核验，核验一致）：** `core_loop/documents.rs:237-293`确认`adopted_passing_generation_round()`现状职责与调用方（367-388行`FINAL_RESULT`文档生成）。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141220.md` 第3.1/3.3节。
**确定方案：** 不改动`adopted_passing_generation_round()`；新增独立函数`completion_gate_for_current_result()`承担自检+用户确认判定，`preview_ready`继续只依赖前者。
**验收用例：** 验收矩阵#24（无自检但证据通过→可preview不可completed）。

### M3：原子性要求不具体

**代码依据（本轮核验，核验一致并补充细节）：** `storage/tasks.rs:317-333`确认现有`completed`门禁已经是`UPDATE...WHERE...EXISTS`单语句原子模式，注释明确解释这是为了防止TOCTOU竞态——审核意见指出了这个现状，本轮确认了具体行号与注释措辞。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141220.md` 第5节。
**确定方案：** 要求新的自检/coverage/证据检查同样折进这同一条`UPDATE...WHERE`语句的`EXISTS`子查询里，不允许"先SELECT判定、再单独UPDATE"。
**验收用例：** 第5节给出的并发测试方案：判定后、写入前发生真实变化，断言UPDATE返回0行。

### M4：历史任务分类留白"编码阶段确认"

**代码依据（本轮核验，部分确认部分发现新结论）：** 全仓库检索`TaskState::FirstReview`/`TaskState::SecondReview`**无匹配**，确认休眠双AI路径不使用专属状态枚举，`task_state_events`的state取值**不能**作为区分双AI的证据源（这是本轮新发现，审核意见本身没有做到这一步）；逐列核对`test_evidence`/`build_evidence`表结构，确认**无**执行者/审核者字段。
**修订位置：** `docs/release-v0.1.0/NOVA_SINGLE_AI_MIGRATION_PLAN10141222.md` 第2.1/2.2节。
**确定方案：** 五类候选证据源核验后只剩三类可用（Action Ledger、生成文档实际内容、`task_reviews`），两类（状态历史、测试/构建元数据）正式排除并写明原因。
**无法完全关闭的部分（如实说明，不写已全部解决）：** Action Ledger是否存在双AI专属`action_type`/key格式，本轮**未逐行读完**`run_core_loop_dual_ai_v0_2`函数体（`core_loop.rs:323`起）的全部action写入调用，只确认了它不使用专属`TaskState`。这一项已在migration plan第2.3节明确列为"下一步只读核验步骤"，不是凭空留白，但**本轮确实没有关闭它**，如实报告。

## Minor

### m1：`self_checks`同构表述不准确

**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141220.md` 第2.1节标题与正文已明确"第三种独立语义，不类比任何既有表"。**已关闭。**

### m2："needs_user_action或更早等待状态"过于开放

**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141220.md` 第3.4节固定列表，9种`BlockReason`逐一对应具体落点状态，不再使用开放表述。**已关闭。**

---

## 本轮未关闭/部分关闭汇总（不写"已全部解决"）

- **M4中Action Ledger双AI专属key格式的核验尚未完成**，已排期为migration plan下一步只读核验步骤，本轮方案依据"若核验后找不到区分性key则此证据源不可用"的保守假设先行设计分类方法，不影响其余已关闭项，但严格说M4只能算"大部分关闭，一项遗留"。
- **M1的具体测试用例文本尚未编写**，只给出了语义和判定逻辑，测试代码本身留给编码阶段。

## 范围边界重申

本轮不涉及Phase5A新建产品功能/UI、GitHub搜索或导入扩展、双AI启用、全站改造。未修改任何产品代码、配置、锁文件、数据库；未提交、未push、未发布、未建立里程碑。

## 下一步

交 Codex 对本文件、`NOVA_WP2A_SELF_CHECK_SPEC10141220.md`、`NOVA_SINGLE_AI_MIGRATION_PLAN10141222.md` 三份文件做只读复审。复审通过后，由 PM 下达 WP2A 编码任务。
