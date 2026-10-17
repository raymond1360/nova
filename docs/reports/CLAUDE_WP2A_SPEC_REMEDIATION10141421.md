# WP2A 方案关闭说明（回应 CODEX_WP2A_SPEC_CLOSURE_REVIEW10141407.md 剩余3个Major）

**性质声明：** 本文件只修订方案文档，**未修改任何产品代码**。关闭指方案层面已给出具体设计，不代表功能已实现，不代表已通过运行验收，也不代表已通过Codex复审。
**审核来源：** `D:\nova\docs\reviews\CODEX_WP2A_SPEC_CLOSURE_REVIEW10141407.md`，FAIL。原B1/B2/B3/m1已被该审核判定关闭（本轮未重新设计）；剩余原M1、原M2、B4衍生Major三项为本轮范围。
**基线：** 分支`feature/v0.1.0-mvp`，HEAD `6a90df951c6c72ab7ae95228d5f2cc4de7e17273`，tag `milestone/evidence-gate`，本轮未产生任何代码/配置改动。

---

## 逐条关闭

### 原M1：执行上下文attempt绑定未真正关闭

**代码依据（本轮核验）：** `storage/actions.rs:67-108`的`start_action`已经在同一次调用内`find_action`读回完整`TaskAction`返回，只是上层代码丢弃；`storage/actions.rs:146-179`的`finish_action`的`UPDATE`条件不含`attempt_number`比对。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141417.md` 第1节。
**方案：** `ActionGate::Started`/`AlreadySucceeded`改为携带完整`TaskAction`（直接转发`start_action`/`plan_action`已有的返回值，不新增查询）；`Database::finish_action`新增`expected_attempt`参数，`UPDATE`条件追加`AND attempt_number=?`，不匹配时`rows_affected=0`、fail-closed拒绝，不读当前行冒充成功。调用方在调用AI**之前**捕获`result_attempt`，之后原样传递，不在完成后重新`find_action`查询"当前"状态。明确陈旧执行允许保留的审计记录（原始命令日志，不参与门禁判定）与不会产出的产物（自检/测试证据/构建证据——因为这些写入时机本就在CAS校验成功之后，陈旧路径根本执行不到那一步）。
**验收用例：** 第8节#1、#2（attempt1晚到、attempt2已started的完整场景）。

### 原M2：当前规格完整性（WP2A-0契约缺失）

**代码依据：** 核验确认`goal/in_scope/not_in_scope/done_criteria`四个自由文本字段创建后不可编辑（无任何更新命令）。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141417.md` 第2节。
**方案：** 完整给出`confirmed_requirement_items`DDL（含唯一约束、索引）；精确定义四字段拆分规则（按`\n`分割、丢弃空行、保留顺序与重复项、`(field,item_index)`为稳定标识，附拆分示例）；明确新任务快照写入时机（`transition_task`的`AwaitingConfirmation`分支内、同一事务）与"不静默覆盖"规则；完整定义`generate_requirement_snapshot`命令契约（项目/任务归属、允许状态、只读输入来源、幂等重复调用、空需求处理、失败条件、不受冻结限制）。
**验收用例：** 第8节#3-#5。

### B4衍生Major：冻结入口未覆盖确认/恢复命令在状态转换前的写入

**代码依据（本轮核验）：** `core_loop.rs:1204`的`confirm_no_tests_applicable`、`core_loop.rs:818`的`confirm_no_build_applicable`，其内部调用链（`reconcile_no_tests_confirmation_attempt`等，约1293-1404行）依次写`confirmation`行、可能`reset_failed_action_for_retry`、最后才`transition_task(...,Queued,...)`——前两步此前完全没有冻结检查。
**修订位置：** `NOVA_WP2A_SELF_CHECK_SPEC10141417.md` 第5.3节。
**方案（选定第一种，不做事务化合并）：** 在两个公共入口函数**最开头**、调用`reconcile_...`之前新增冻结检查，一次检查保护整条调用链，天然保证冻结时不写任何action/confirmation变化，不需要把检查、reset、写入、transition四步包进同一事务再回滚。明确区分`run_startup_recovery`的"中断记账"（只读性质，不受影响）与这两个命令的"继续执行准备动作"（受冻结检查保护）。
**验收用例：** 第8节#6、#7，第5.4节给出拒绝前后状态/action/confirmation/事件的逐项对比。

## 已维持关闭项（未重新设计）

原B1（历史分类）、原B2（最终completed门禁7步伪代码）、原B3（唯一键扩展含result_attempt）、原m1（拒绝原因仅API返回）——均原样内联维持，未改动核心设计。

## Minor顺手澄清

- `no_build_applicable_is_authorized`/`no_tests_applicable_is_authorized`明确写出完整函数签名（含`result_attempt`参数），不再用省略号表达同构实现（`NOVA_WP2A_SELF_CHECK_SPEC10141417.md`第3.6节）。
- 预览展示陈旧attempt证据时，文案不得称"当前已验证"，应如实说明来自更早执行（第6节）。

---

## 本轮仍未关闭/需要注意的事项（如实说明）

- `is_legacy_frozen`辅助函数、`split_requirement_fields`拆分函数的具体Rust实现细节留给编码阶段，本轮只定义输入输出契约和精确规则（如2.2节的拆分规则已足够精确到可以直接编码，但函数命名/模块归属等细节未定）。
- `legacy_classification`迁移脚本的具体批量计算SQL仍未给出（与上一轮一致，留给编码阶段）。
- 本文件与两份权威spec/plan均为方案文档，尚未经Codex复审，不写"已全部解决"。

## 范围边界重申

本轮不涉及Phase5A新建产品功能/UI、GitHub搜索或导入扩展、双AI启用、全站改造，不开始迁移实现、测试实现。未修改任何产品代码、配置、锁文件、数据库；未运行AI CLI；未读取真实用户数据库；未提交、未push、未发布、未建立里程碑。

## 下一步

交 Codex 仅对本轮3个Major（原M1、原M2、B4衍生Major）做关闭复审：
1. `D:\nova\docs\reports\CLAUDE_WP2A_SPEC_REMEDIATION10141421.md`（本文件）
2. `D:\nova\docs\release-v0.1.0\NOVA_WP2A_SELF_CHECK_SPEC10141417.md`
3. `D:\nova\docs\release-v0.1.0\NOVA_SINGLE_AI_MIGRATION_PLAN10141420.md`
