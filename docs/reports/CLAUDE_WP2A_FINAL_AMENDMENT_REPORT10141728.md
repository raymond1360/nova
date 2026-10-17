# WP2A 补充规格关闭说明（回应 CODEX_WP2A_SPEC_FINAL_CLOSURE_REVIEW10141613.md 剩余3个Major+2处文字）

**性质声明：** 本文件只新增补充规格文档，**未修改任何产品代码**。关闭指方案层面已给出具体设计，不代表功能已实现，不代表已通过运行验收，也不代表已通过Codex复审。
**审核来源：** `D:\nova\docs\reviews\CODEX_WP2A_SPEC_FINAL_CLOSURE_REVIEW10141613.md`，FAIL。B4衍生Major已被该审核判定关闭（本轮不重新设计，只做Minor文字修正）；剩余原M1两个未关闭问题、原M2一个未关闭问题为本轮范围。
**基线：** 分支`feature/v0.1.0-mvp`，HEAD `6a90df951c6c72ab7ae95228d5f2cc4de7e17273`，tag `milestone/evidence-gate`，本轮未产生任何代码/配置改动。

---

## 逐条关闭

### 原M1未关闭问题1：test/build evidence写入顺序会绕过CAS目的

**代码依据（本轮核验，确认审核意见引用属实）：** `core_loop.rs:2172-2300`（`run_tests_for_round`全部分支）、`core_loop.rs:2561-2661`起（`run_build_for_round`全部分支）均先`record_test_evidence`/`record_build_evidence`，后`finish_action`，且`finish_action`现状不校验attempt。
**修订位置：** `NOVA_WP2A_FINAL_AMENDMENT10141726.md` 第1节。
**方案：** 明确区分"开发result_attempt"与"测试/构建action自身attempt"是两个不同概念，缺一不可；统一把全部分支（无测试/无构建适用、allowlist拒绝、取消、timeout、passed/failed）的顺序改为"先`finish_action(expected_attempt)` CAS，成功后才写evidence"；给出写入顺序表和dev/test/build统一模式的最小伪代码；CAS成功但evidence写入前崩溃维持既有fail-closed规则（`AlreadySucceeded`分支"查无证据"的既有处理），不新增机制。
**验收用例：** 第1.5节，覆盖"test attempt晚到、dev attempt未变"与"build attempt晚到"两类场景。

### 原M1未关闭问题2：`Err(_)`统一当Stale会吞掉真实错误

**修订位置：** `NOVA_WP2A_FINAL_AMENDMENT10141726.md` 第2节。
**方案：** `NovaError`新增专门变体`StaleActionAttempt(String)`，只由`finish_action`的CAS未命中产生；给出dev/test/build三类调用方的统一匹配伪代码，明确`Err(NovaError::StaleActionAttempt(_))`才转为对应的`Stale`结果，其余`NovaError`变体（`Config`/`Credential`/`Database`/`Network`/`Api`）必须`Err(other) => return Err(other)`原样上抛。
**验收用例：** 第1.5节场景同时验证了这一点（CAS失败走专门错误路径，而非笼统`Err(_)`）。

### 原M2未关闭问题：`generate_requirement_snapshot`允许Draft与"已确认"前提冲突

**代码依据（本轮核验）：** `apps/desktop/src/stores/coreLoop.ts:460-478`确认"确认需求"是两次`transition_task`调用（先`awaiting_confirmation`再`confirmed`），证实`AwaitingConfirmation`已经是确认之后的状态，不应与`Draft`同等对待。
**修订位置：** `NOVA_WP2A_FINAL_AMENDMENT10141726.md` 第3节。
**方案：** 后端允许状态改为显式列举（排除`Draft`和三个终态），列出完整允许状态集合；明确状态校验在函数最开头执行，幂等返回（已有快照直接返回）在状态校验**之后**，不能绕过；给出`Draft`/`AwaitingConfirmation`/`Confirmed`三个状态的真实语义边界表。
**验收用例：** 第3.4节，覆盖Draft拒绝、AwaitingConfirmation（含自动快照缺失补建）、Confirmed、终态拒绝、重复调用幂等、冻结任务叠加场景共7项。

## B4衍生Major：已关闭，本轮只做Minor文字修正

**修正内容：** `run_startup_recovery`的描述从"只读/记账性质"改为"恢复记账写入——会标记action为uncertain、转任务为Interrupted并记录checkpoint，但不继续执行、不重跑命令或AI"，见 `NOVA_WP2A_FINAL_AMENDMENT10141726.md` 第4.1节。冻结检查的结论本身（恢复扫描不需要额外加检查）未变。

## 权威文件关系（回应"文字与权威关系"）

本轮采用**补充规格**，不声称旧稿完全不承载规则。`NOVA_WP2A_FINAL_AMENDMENT10141726.md` 第0节明确列出：`...SELF_CHECK_SPEC10141417.md`与`...MIGRATION_PLAN10141420.md`继续有效；`...SELF_CHECK_SPEC10141347.md`被明确列为"保留的权威附件"（其被`10141417.md`引用的completed门禁7步伪代码、历史分类组合表等内容仍以该文件对应章节为准，不是占位引用）；本补充规格只覆盖本轮3个Major对应的具体机制，其余一切以上述文件为准。明示了不得重新引入的已作废规则（self_checks覆盖写入语义、user_acceptance加round列、历史分类的错误单AI推断）。

---

## 本轮仍未关闭/需要注意的事项（如实说明）

- `test_action_attempt`/`build_action_attempt`在`run_tests_for_round`/`run_build_for_round`函数体内具体用哪个局部变量名、如何在多个分支间传递，留给编码阶段，本文件只给出语义契约和一个代表性分支的完整伪代码。
- `SNAPSHOT_ALLOWED_STATES`常量的具体Rust类型/模块位置留给编码阶段。
- 本文件与`NOVA_WP2A_FINAL_AMENDMENT10141726.md`均为方案文档，尚未经Codex复审，不写"已全部解决"。

## 范围边界重申

本轮不涉及Phase5A新建产品功能/UI、GitHub搜索或导入扩展、双AI启用、全站改造，不开始迁移实现、测试实现。未修改任何产品代码、配置、锁文件、数据库；未运行AI CLI；未读取真实用户数据库；未提交、未push、未发布、未建立里程碑。

## 下一步

交 Codex 仅对以下两份文件、针对本轮3个Major做关闭复审：
1. `D:\nova\docs\reports\CLAUDE_WP2A_FINAL_AMENDMENT_REPORT10141728.md`（本文件）
2. `D:\nova\docs\release-v0.1.0\NOVA_WP2A_FINAL_AMENDMENT10141726.md`
