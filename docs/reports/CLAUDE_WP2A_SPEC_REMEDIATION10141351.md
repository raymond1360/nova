# WP2A 方案关闭说明（回应 CODEX_WP2A_SPEC_TARGETED_REVIEW10141332.md 4个Blocker+2个Major+1个Minor）

**性质声明：** 本文件只修订方案文档，**未修改任何产品代码**。关闭指方案层面已给出具体设计，不代表功能已实现，不代表已通过运行验收，也不代表已通过Codex复审。
**审核来源：** `D:\nova\docs\reviews\CODEX_WP2A_SPEC_TARGETED_REVIEW10141332.md`，FAIL，4个Blocker（B1-B4）、2个Major（M1-M2）、1个Minor（m1）。
**基线：** 分支`feature/v0.1.0-mvp`，HEAD `6a90df951c6c72ab7ae95228d5f2cc4de7e17273`，tag `milestone/evidence-gate`，本轮未产生任何代码/配置改动。

---

## 逐条关闭

### B1：历史分类仍把"无审核痕迹"当作单AI正面证据

**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141347.md` 第4节；`NOVA_SINGLE_AI_MIGRATION_PLAN10141350.md` 第1.2节。
**方案：** 删除"开发成功+两强信号皆无+终态/待验收→single_ai_evidenced"分支。诚实说明：当前代码对"任务从哪个顶层入口启动"完全没有留痕，这不是遗漏而是明确的范围选择，不为填补这个空白新增记录系统；`single_ai_evidenced`枚举保留但当前永不触发，所有任务只落入`dual_ai_evidenced`或`unknown_insufficient`。
**验收用例：** 第8节#14（新建任务不受冻结影响，天然区别于历史任务问题）。

### B2：最终completed门禁未重验同执行归属的真实证据/授权

**代码依据：** 核验确认全仓库唯一写`tasks.state='completed'`的SQL在`storage/tasks.rs`，`accept_task_result`/通用命令/恢复路径最终都调用它。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141347.md` 第3节。
**方案：** 给出完整`completed_gate_checks`伪代码，7步依次核验：历史冻结、adopted evidence所在(generation,round)、当前开发action真实attempt、同attempt测试证据或授权、同attempt构建证据或授权、当前有效自检、需求覆盖、用户确认——全部在`Database::transition_task`本体内、同一`BEGIN IMMEDIATE`事务内完成，不依赖Verifying阶段"曾经检查过"。
**验收用例：** 第8节#2、#3、#6、#17、#18、#19。

### B3：唯一键与新attempt重新授权冲突未解决

**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141347.md` 第2.3节。
**方案（选定，不留候选）：** 唯一键扩展为`(task_id,result_generation,round,result_attempt)`，利用SQL标准语义——UNIQUE约束中NULL永不与另一NULL冲突——让历史行(NULL)与任意新attempt行天然互不冲突；`ON CONFLICT DO NOTHING`语义不变，查询精确匹配当前`result_attempt`。
**验收用例：** 第8节#3、#4、#6。

### B4：旧非终态任务冻结缺少实际入口

**代码依据：** 核验`run_core_loop`（`core_loop.rs:101-155`）现状只检查`state`、`unblock_test_execution_for_retry`可重新排队。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141347.md` 第5节。
**方案：** 新增持久化`tasks.legacy_classification`列，只在迁移那一刻为当时非终态任务一次性计算，避免误伤迁移后新建任务；列出5个实际接入点（`run_core_loop`、`unblock_test_execution_for_retry`、恢复扫描、通用`transition_task`到`Queued`/`Running`、最终`Completed`门禁）；明确`generate_requirement_snapshot`不解除冻结；明确只读查看/取消允许，解冻机制不在本轮范围。
**验收用例：** 第8节#7-#13。

## Major

### M1：`result_attempt`捕获仍是事后查询

**代码依据：** 核验`ActionGate`枚举与`finish_gated_action`真实签名，确认`db.start_action`/`db.finish_action`的返回值被当前代码丢弃，`db.finish_action`本身已经是"写入后立即读回"而非分离查询。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141347.md` 第1节。
**方案：** `ActionGate::AlreadySucceeded`携带`attempt_number`，`finish_gated_action`返回`TaskAction`而非`()`，调用方直接使用这些返回值，不再调用`find_action`重新查询"当前"状态。

### M2：文档引用新旧混用

**方案：** 本轮两份文件（`...10141347.md`、`...10141350.md`）互相声明为唯一权威、自包含，所有此前版本的核心内容已原样内联重写，不写"与上一版一致"。

## Minor

### m1：拒绝原因持久化语义未定

**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141347.md` 第3.4节。
**方案（选定）：** 只通过API错误返回，不持久化专门审计记录，不新增事件系统；拒绝原因是对已持久化数据的只读推导，重新打开任务时通过只读预检命令重新计算即可得到一致结果。

---

## 交付前自查结果

逐项核对：B1-B4/M1-M2/m1均已给出修订章节、源码依据、方案、验收用例（见上表及第8节矩阵）。已关闭规则（门禁失败保持AwaitingUserAcceptance；预览与completed分离；自检必须complete且无未完成/阻断；不默认额外调用AI；一次修复上限及既有证据授权规则）已在权威spec第6/7节重新确认，未被本轮改动破坏。

## 本轮仍未关闭/需要注意的事项（如实说明）

- `completed_gate_checks`伪代码里"coverage_matches_exactly"等辅助函数的具体实现细节留给编码阶段，方案只定义了输入输出契约。
- `legacy_classification`迁移脚本的具体SQL批量计算逻辑（如何对迁移那一刻的全部非终态任务高效批量执行分类算法）本轮给出了语义定义，未给出具体SQL/批处理实现，留给编码阶段。
- 本文件与两份权威spec/plan均为方案文档，尚未经Codex复审，不写"已全部解决"。

## 范围边界重申

本轮不涉及Phase5A新建产品功能/UI、GitHub搜索或导入扩展、双AI启用、全站改造。未修改任何产品代码、配置、锁文件、数据库；未运行AI CLI；未读取真实用户数据库；未提交、未push、未发布、未建立里程碑。

## 下一步

交 Codex 按以下固定清单复审：
1. `D:\nova\docs\reports\CLAUDE_WP2A_SPEC_REMEDIATION10141351.md`（本文件）
2. `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141347.md`
3. `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141350.md`
