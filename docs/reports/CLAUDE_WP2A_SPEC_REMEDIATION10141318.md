# WP2A 方案关闭说明（回应 CODEX_WP2A_SPEC_REREVIEW10141259.md 3个Blocker+4个Major）

**性质声明：** 本文件只修订方案文档，**未修改任何产品代码**。"关闭"指方案层面已给出具体、与当前代码事实一致的设计，**不代表功能已实现，不代表已通过运行验收，也不代表已通过Codex复审**，下一步仍需交Codex复审。
**审核来源：** `D:\nova\docs\reviews\CODEX_WP2A_SPEC_REREVIEW10141259.md`，结论FAIL，3个Blocker（B1-B3）、4个Major（M1-M4）、2个Minor（m1-m2）。该文件本身记录"上一轮报告优先查找`D:\nova\docs\reviews\`路径不存在，只读查到原位置`D:\nova-review\`"——本轮的三份文档按PM指令分别保存到`docs/release-v0.1.0/`与`docs/reports/`，不再使用`D:\nova-review\`这个仓库外目录。
**基线：** 分支`feature/v0.1.0-mvp`，HEAD `6a90df951c6c72ab7ae95228d5f2cc4de7e17273`，tag `milestone/evidence-gate`，本轮未产生任何代码/配置改动。

---

## Blocker

### B1：当前开发attempt自检与test/build证据未绑定同一执行归属

**代码依据（本轮核验）：** `storage/evidence.rs:30-44,59-70,321-333,473-485`确认test/build evidence无attempt字段；`core_loop.rs:2029-2054,2368-2391`确认action key只按generation+round构造；`core_loop.rs:2077-2095`附近确认`AlreadySucceeded`分支直接复用旧证据，不核对归属。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141315.md` 第2节。
**方案：** 新增统一`result_attempt`字段，在`run_dev_and_test_round`开头、开发action确认成功的那一刻读取一次attempt_number，作为显式参数向后传给自检写入、`run_tests_for_round`、`run_build_for_round`，三者各自持久化这同一个值；`AlreadySucceeded`缓存复用分支新增比对，不匹配则按既有fail-closed模式处理（不新增AI调用）；无测试/无构建确认表同样新增该字段防止旧attempt授权被新attempt复用。
**验收用例：** 第7节#2、#7、#16、#17。

### B2：历史分类规则自相矛盾

**代码依据（本轮复核）：** `core_loop.rs:1697-1698`确认`ai_review` action_type是真实、唯二AI路径独有的强信号。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141315.md` 第5节；`NOVA_SINGLE_AI_MIGRATION_PLAN10141317.md` 第2节同步采用同一算法。
**方案：** 把"文档内容"降级为不参与分类决策的弱信号（普通文件可被编辑，不满足来源真实性要求），只剩两个数据库强信号做OR判定；三分类（dual_ai_evidenced/single_ai_evidenced/unknown_insufficient）两两互斥，不再有同一输入落入两类的情况；新增`integrity_warning`作为分类之外的独立标注。
**验收用例：** `NOVA_WP2A_SELF_CHECK_SPEC10141315.md` 第5.3节给出PM要求的全部7种组合的唯一判定。

### B3：拒绝completed时状态/事务语义冲突

**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141315.md` 第3.3节。
**方案（选定PM指定的方向）：** 完工门禁任一条件不满足时，拒绝转入`Completed`，任务保持`AwaitingUserAcceptance`原地不动，不写入`NeedsUserAction`、不记录声称已转入该状态的事件；返回结构化拒绝原因；用户仍可查看效果（因为`PreviewReady→AwaitingUserAcceptance`本就不受本门禁影响）；各失败原因对应的恢复方式逐一列出，不默认新增AI调用，不虚构自动恢复能力，数据异常类原因（如`SelfCheckMissing`）如实阻断，不提供假的自动恢复按钮。
**验收用例：** 第7节#3-#8、#12、#18。

## Major

### M1：completed权威入口未明确到`Database::transition_task`

**代码依据：** `commands.rs:536-553`确认通用`transition_task`命令暴露任意目标状态；全仓库核验`grep "SET state"`确认写`tasks.state`的SQL只有`storage/tasks.rs`一处。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141315.md` 第3.2节。
**方案：** 废弃"独立的`completion_gate_for_current_result()`函数"提法，新条件直接内联进`Database::transition_task`本已唯一的那条原子UPDATE语句（`to=='completed'`分支），通用命令和`accept_task_result`最终都调用这同一个函数，没有第二条路径能写`completed`。
**验收用例：** 第7节#9、#10、#11。

### M2：旧未完成任务无需求快照处理未定义

**代码依据：** `apps/desktop/src/stores/coreLoop.ts:460-478`确认"确认需求"只是`Draft→AwaitingConfirmation`的通用转换。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141315.md` 第1.4节。
**方案：** 不静默生成快照、不要求重新描述；新增显式命令`generate_requirement_snapshot(task_id)`，只读已确认且不可编辑的四个字段原样生成快照，仅非终态且表中无行时可调用；完工门禁遇到零行快照时拒绝并给出专属原因`RequirementSnapshotMissing`，区别于"覆盖不全"；已完成历史任务不回溯。
**验收用例：** 第7节#12、#13。

### M3：`self_checks`表结构不完整

**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141315.md` 第2.3节。
**方案：** 给出完整DDL（含project_id/task_id/round/result_attempt/parse_status/self_reported_status/四类JSON字段/raw_block_redacted/created_at，及索引），并说明"当前判定"权威查询、为何不设UNIQUE约束、historical行result_attempt如何backfill为NULL。
**验收用例：** DDL本身即可直接推导存储实现，第2.3/2.4/2.5节给出配套的test_evidence/build_evidence/no_tests_confirmations/no_build_confirmations新增列方案。

### M4：占位引用

**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141315.md` 第6节，已替换为实际路径`NOVA_SINGLE_AI_MIGRATION_PLAN10141246.md`（历史分类算法本身以本轮`...10141317.md`第5节为准，两份迁移计划文档已在交叉引用里说明新旧关系）。

## Minor

### m1：按换行拆分只提供形式覆盖

已在`NOVA_WP2A_SELF_CHECK_SPEC10141315.md` 第1.3节明确声明：形式覆盖不等于语义完整，不等于独立审核，自检陈述仍受真实证据和用户确认约束。

### m2：WP2A-0与WP2A主体提交/复审顺序

已在第6节说明：不强制分两次提交/复审，但编码时WP2A主体依赖WP2A-0的迁移已存在。

---

## 验收矩阵回归覆盖（PM要求的补项，逐一确认已在第7节覆盖）

旧未完成任务无快照（#12/#13）、直接状态转换绕过（#10）、当前attempt配旧证据（#2）、缺少用户确认（#8）、自检失败保持原状态（#3-#6）、并发变化（#11）——全部已覆盖；真实证据门禁、无测试/无构建授权、一次修复上限、中断恢复的回归要求也已在#14-#17列出。

## 本轮仍未关闭/需要注意的事项（如实说明，不写已全部解决）

- 第3.2节给出的SQL示意里，"当前有效self_checks判定行的coverage完整性比对"说明了必须由Rust应用层在事务内先读出来再绑定参数，这部分应用层代码的具体实现（如何在同一个`BEGIN IMMEDIATE`事务里组织只读查询与最终UPDATE的调用顺序）本轮只给出架构性说明，具体函数签名/错误类型设计留给编码阶段，不属于本轮方案层面需要解决的空白，但如实标注这里还有实现细节需要编码时确认。
- `integrity_warning`的具体展示方式（是否需要前端界面、以什么形式呈现给用户/Codex）本轮未设计，只定义了它在数据层面的触发条件。

## 范围边界重申

本轮不涉及Phase5A新建产品功能/UI、GitHub搜索或导入扩展、双AI启用、全站改造。未修改任何产品代码、配置、锁文件、数据库；未运行AI CLI；未读取真实用户数据库；未提交、未push、未发布、未建立里程碑。

## 下一步

交 Codex 对以下三份文件做定向复审：
1. `D:\nova\docs\reports\CLAUDE_WP2A_SPEC_REMEDIATION10141318.md`（本文件）
2. `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141315.md`
3. `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141317.md`
