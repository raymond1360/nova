# WP2A 实际代码与测试独立审核

生成时间：2026-10-15 10:53 Asia/Shanghai（实际UTC 02:53）。项目：D:\nova。

## 唯一结论：FAIL

WP2A代码验收未完成。不得以“WP2A已验收”为前提进入提交/里程碑准备；先修复本文Blocker/Major并完成隔离验证，再独立复审。本报告不批准发布，前次方案PASS WITH NOTES不是代码PASS。

共3 Blocker、6 Major、3 Minor、1 Suggestion。现有Rust 651项、前端99项测试全绿，但直接调用实际存储代码的独立程序复现了错误完工、当前代无用户确认仍完工和迟到结果误分类。

## 现场保护和适用依据

- 分支符合预期：feature/v0.1.0-mvp。
- HEAD：6a90df951c6c72ab7ae95228d5f2cc4de7e17273。
- milestone/evidence-gate是annotated tag；tag对象da46c23c7055b7440534452cb37bd388073a4b84；解引用 milestone/evidence-gate^{} 正确指向上述HEAD，不是基线错误。
- 搜索仓库及适用上级位置未发现AGENTS.md/CLAUDE.md；未发现仓库README/CONTRIBUTING规则文件。遵循本轮用户保护要求。
- 初始已有22个tracked修改文件（含更早的两个product修改）、三个新增源码和多份未跟踪文档。读取tracked diff和新增源码，未切换、清理、回退、stash、整文件恢复、删除未跟踪文件。
- 验证后tracked修改清单、HEAD/tag解引用不变。仓库内本轮仅新增本报告；测试/构建产物正常生成。临时独立程序及新数据库放在系统临时目录。

已读实现报告CLAUDE_WP2A_IMPLEMENTATION_REPORT10150058.md、实现说明NOVA_WP2A_IMPLEMENTATION_NOTES10150058.md、准入审核CODEX_WP2A_AMENDMENT_REVIEW10141751.md。规格优先级：FINAL_AMENDMENT10141726 > SELF_CHECK_SPEC10141417 + MIGRATION_PLAN10141420；仅针对明确引用的schema/门禁细节查SELF_CHECK_SPEC10141347第2/3节，未恢复其中被替代的attempt捕获规则。

以下位置相对D:\nova，行号对应本次工作区。

## Blocker

### B1 自检生产输入合同缺失，Mock直接提供合格答案掩盖断链（功能缺失）

位置：apps/desktop/src-tauri/src/core_loop.rs:3514-3559、1766-1775、1825-1830；runtime/ai_adapter.rs:484-496、565-580、827-836。

条件：正常确认有需求的任务，AI按实际任务包完成第一轮开发或第二轮修复，stdout输出正常文字总结。

依据：write_task_package只有四字段原文、上一轮findings和范围约束；不读取confirmed_requirement_items、不传field:index标识；没有自检标记、JSON shape、状态枚举、coverage或stdout输出要求。真实Codex/Claude adapter把这个包送入CLI；解析器却只接受固定标记内JSON。普通成功总结会记invalid_format，门禁拒绝。开发和修复共享该缺口。

影响：生产成功路径没有结构化合同保证，AI也不知道权威item标识；不能以“模型可能自行猜中”视为实现。

最小修复：在共享开发/修复包中列出快照标识和文本，明确同次stdout的完整结构、枚举、逐项覆盖要求，不新增AI调用；快照缺失明确处理。

通过条件：Mock读取真实package内容而非默认补答案，覆盖开发/修复、多字段、多行重复文本；回传合同输出后可以完成。缺块、空壳、错误字段、incomplete/blocked、问题数组、漏项/重复/伪造/partial都必须拒绝。当前Mock默认硬编码goal:0 covered不能证明生产链路。

### B2 门禁在采纳结果不匹配后搜索旧代/另一轮（正确性缺陷，已动态复现）

位置：storage/tasks.rs:385-456；对照core_loop/documents.rs:237-302及10141347第3.2节。

复现：任务generation1；当前代round1有passed test/build，但result_attempt99与当前dev attempt1不符；上一代generation0 round2有attempt1完整证据、自检；用户确认generation1。

依据：规格先由adopted_passing_generation_round固定采纳tuple，随后逐项核验，失败应拒绝。实际门禁另写嵌套搜索，遇到dev/test/build不满足就continue，可回退到其他generation/round，再配上当前代用户确认。

独立实际输出：fallback_gate=Ok(Ok(()))；fallback_transition=Ok(Completed)。调用未改动的实际Database方法，夹具是临时库对抗组合，不宣称来自真实桌面操作。

影响：旧结果替代当前采纳结果，通用transition_task自身就可错误完工。

最小修复：复用唯一采纳选择，固定tuple后fail-closed，不再寻找另一历史组合。保留PreviewReady导致证据generation落后一代的合法设计，不能简单删除全部generation-1支持。

通过条件：该组合拒绝且状态/事件不变；采纳轮dev失败/归属不符/test或build不符均不能由另一轮救活；合法上一代证据配当前验收仍通过。

### B3 事务内门禁使用事务前Task，当前代用户确认可被绕过（并发缺陷，已动态复现）

位置：storage/tasks.rs:545、575、587、593-600。

复现：A读awaiting_user_acceptance/generation1，B持写锁把代际推进generation2并保持/回到同名状态；A取得BEGIN IMMEDIATE后继续使用旧Task。旧代有确认，新代无确认。

依据：task/from在事务前读取；evaluate_completion_gate(&task)使用旧result_generation；UPDATE只核验id/state，不核验generation，无法识别离开后回同状态的ABA变化。

独立双连接锁竞争实际输出：generation_race_transition=Ok((Completed, 2))；generation_race_current_acceptance=None。B的代际变化由隔离测试SQL在锁内注入，用于证明数据库入口隔离缺口；未声称运行了完整桌面并发业务。

影响：BEGIN IMMEDIATE没有覆盖Task初始读取，可以在没有当前代用户确认时完成。

最小修复：取写事务后重读Task并校验合法状态、当前generation，在同一事务快照内完成门禁和写入；必要时state+generation CAS。

通过条件：双连接屏障在旧Task读取与取锁间推进代际，A必须拒绝或重新核验新代，不得Completed且当前代无确认；覆盖通用转换与accept入口。

## Major

### M1 generate_requirement_snapshot未实现、未注册、无用户入口（功能缺失）

位置：core_loop.rs、commands.rs:1280-1306、lib.rs:147-153、前端store/view；唯一辅助写入为storage/requirement_items.rs:65。

依据/条件：全源码无该命令实现、Tauri包装注册、前端调用及“确认沿用，生成需求清单”按钮。自动AwaitingConfirmation快照不是独立补建。报告对照表却把完整命令契约映射到自动分支。

影响：旧非终态缺快照无法按明确用户操作补建；项目归属、Draft/终态先拒绝再幂等、事务和冻结不变均无法验收。

修复：事务化业务入口+Tauri注册+原需求只读展示和明确按钮；严格补充稿允许状态，状态校验先于读/返已有快照，不改legacy分类。

通过：归属错误、Draft（包括故意已有快照）、Completed/Failed/Cancelled零写入拒绝；AwaitingConfirmation/Confirmed/幂等/空需求/冻结任务补建按契约验证。不能把不存在命令的用例报告为已通过。

### M2 新attempt非Started时，旧结果被误判普通Config错误（正确性缺陷，已复现）

位置：storage/actions.rs:187-197；调用方core_loop.rs:1786-1797、2429-2430、3070-3071。

条件：attempt1已被替代，attempt2处于Planned或已经Succeeded，attempt1迟到。

依据：UPDATE正确约束started和expected_attempt，阻止覆盖；但零行诊断先检查status，非Started返回Config，仅当前Started才Stale。独立两例均Err(Config("只有处于 started 状态的动作才能被结束"))。

影响：数据库未覆盖，但正常迟到被作为真实故障传播，不满足合同的Stale停止语义。

修复：零行优先识别attempt不同，各种被新attempt替代的结果均Stale；SQL错误仍原样传播。

通过：dev/test/build覆盖新attempt Planned/Started/Succeeded/Failed/Cancelled/Uncertain；旧结果不产有效证据、不transition；真实数据库错误不被折成Stale。

### M3 legacy_unknown通过任意attempt确认获得通用授权（授权隔离缺陷）

位置：core_loop/documents.rs:393-403；storage/no_build_confirmation.rs:113-134；消费者adopted_passing_generation_round和evidence_overview.rs:194、223-224。

条件：历史NULL attempt/source=legacy_unknown证据，存在同generation/round任一非NULL确认，即使属于另一次开发结果。

依据：find_any只限制IS NOT NULL，未绑定当前dev attempt；helper返回true。现有legacy_unknown授权测试未覆盖不同attempt反例。

影响：历史兼容被升级为当前授权/采纳依据，展示可能称有效授权。准确边界：**最终tasks门禁另要求evidence.result_attempt==Some(current_attempt)，当前防线阻止NULL直接完工；不能夸大为该例外单独绕过Completed。** 但通用helper与同attempt契约冲突。

修复：NULL在实时授权函数fail-closed；历史中性展示独立实现，不进入adopted/acceptance_ready；不得以任意attempt推导当前授权。

通过：NULL+其他attempt确认不能变当前授权；精确当前attempt可授权；历史NULL保留审计且不被改写，最终门禁仍拒绝NULL。

### M4 Case C使正常授权恢复按钮不可达，不只是体验问题（已知问题的功能影响）

位置：core_loop.rs:1200-1242、1289-1318；CoreLoopView.vue:419、541-545。

条件：确认已写、build action已reset为Planned，在任务转Queued前崩溃；重开仍NeedsUserAction。

依据：reconcile支持Case C，但no_build_confirmation_needed要求Failed+marker，Planned直接false；前端唯一无需构建确认/恢复卡被该bool隐藏，启动卡仅confirmed可见，余下泛化“需要用户处理”。现有half-success查询测试只覆盖action仍Failed的Case B。

影响：后端可重入不等于用户有恢复入口；手工IPC/提供无关测试命令不构成正常授权恢复。

修复：按精确dev attempt确认识别Case C中间态，提供明确继续/重试入口，不再次调用AI或伪造确认。

通过：隔离Case C查询true、UI显示恢复，点击后Queued、不重复确认、不多reset；真实构建失败/无关attempt不显示。

### M5 回滚说明不保护混合工作区，数据库恢复步骤不安全（交付缺陷）

位置：实现报告CLAUDE_WP2A_IMPLEMENTATION_REPORT10150058.md:143-146。

依据/影响：建议checkout整文件或stash会抹掉/搬走既有工作，违反补丁边界。“备份替换当前文件即可”遗漏停应用、全部连接、WAL/SHM/journal及恢复验证。

最小修复：后续由实现者修正，不能直接采用报告回滚命令。代码先保全全部tracked/untracked，识别WP2A hunk和精确新增文件，仅反向本轮补丁，保留产品和重叠既有修改；不整文件恢复/清未跟踪文件。数据库：停应用和写入方→保全当前主库及sidecar一致集合或用SQLite在线备份→校验目标备份完整性/schema/user_version→全部连接关闭后隔离当前sidecar并恢复已验证备份，避免旧WAL回放→配套旧代码在隔离环境检查integrity_check/foreign_key_check、数量及关键记录，再审批实际恢复。

通过：可审阅本轮补丁边界和隔离回滚演练，含WAL一致性；本轮不执行真实恢复，不改既有报告。

### M6 所宣称的核心负向和并发验证缺乏对应测试（验证缺口）

位置：实现报告第3.1节；runtime/ai_adapter.rs:827-836；storage/tasks.rs:1053-1135；core_loop.rs:3818、3890及核心测试diff。

依据：actions/tasks/core_loop tracked diff增加参数与满足门禁的夹具，没有新增对应#[test]来专门检验WP2A门禁、冻结入口和全部迟到矩阵。新增解析/存储/迁移测试有效，但不等于报告宣称的Draft补建、全部拒绝模式、CAS分支、冻结零副作用均验证。generate命令甚至不存在。Mock硬编码合格自检，夹具直接写passed和完整自检。

影响：旧测试在人工补齐新前提后继续绿，不能证明新失败边界；本轮反例印证缺口。未据此推断故意造假，但覆盖陈述失实。

修复/通过：为B1-B3/M1-M4补实际业务入口断言，失败前后状态/事件/action/confirmation对比，多个round/generation/attempt、CAS后证据写失败/AlreadySucceeded恢复、所有test/build分支迟到、SQL错误传播、冻结零写入。逐项可追溯，不用总数或合格夹具替代负向验证。

## Minor

### m1 时间线误用test/build自身attempt，且同代旧dev attempt未标历史

位置：core_loop/evidence_overview.rs:383、434-435、473、524-525、599、622。

条件：dev1确认写1，test/build reset后自身attempt2。时间线用2找dev归属确认会漏掉真实确认或显示其他结果确认；历史标识只比较generation，不能识别同代旧attempt。

影响：审计展示错误；最终门禁另有精确查询，不因此直接放行。

修复/通过：按dev归属或完整历史确认列表显示并标历史；dev1/test2/build2下确认不消失，同代旧attempt显示“更早执行”，不标当前已验证。

### m2 预检和“可以验收”语义冲突

位置：core_loop.rs:635-683；evidence_overview.rs:224；CoreLoopView.vue:488-493、587-588。

条件：证据自检合格，首次点击验收前没有user_acceptance，预检称“标记完成会被拒绝”，实际accept先写本次确认可成功；反之invalid自检时overview仍可称“可以验收”。预检也未校验合法来源状态。

影响：错误预测、结果查看与可完工混淆。

修复/通过：区分查看/提交验收/直接完成，保持后端当前用户确认门禁；首次正常验收不误报警，自检失败不显示可完工；验证store异步失效和真实IPC。

### m3 迁移/备份/warning技术描述不实

位置：实现报告第3/4节；storage/migrations.rs:920-992、1159-1195。

依据：四表执行复制、DROP、RENAME、索引重建，不是“不删除/不重命名任何表”；备份用SQLite online backup API+临时文件+完整性校验+rename，不是原始文件拷贝。两条warning来自本轮新增REQUIREMENT_FIELDS和DevActionNotSucceeded，不是既有无关警告。

影响：迁移风险与备份一致性解释错误，不表示实际迁移已丢数据。

修复/通过：准确写“保留数据的结构性重建”和online backup，补四表内容/NULL/索引/唯一键及v11中途失败原子性证据。

## Suggestion

S1：self_check_parser.rs:23-39、62-75只取第一个标记块、serde忽略未知字段、check_results省略默认空。建议合同明确后拒绝多块矛盾/未知拼错字段，明确必填。未以没有明示的格式细节单独判Blocker。coverage gate确有漏项/重复/伪造/partial拒绝代码，incomplete/blocked/非空问题数组亦有拒绝分支；入口专测不足见M6。

## 已成立防线与核验范围

1. 自动快照四字段按CRLF/CR规范、逐行trim、去空、保重复/顺序，UNIQUE(task_id,field,item_index)；与AwaitingConfirmation写入同事务，已有行不覆盖。空快照拒绝符合规格。独立补建缺失见M1。
2. 自检取同次开发stdout，不增AI调用；开发CAS后追加持久化；current按task/round/dev attempt精确匹配、rowid最新，invalid也留审计。dev成功但自检写前崩溃时AlreadySucceeded不重解析，门禁缺自检拒绝，安全但恢复验证不足。生产输入缺口见B1。
3. dev执行前捕获attempt；test/build捕获自身attempt；无适用、allowlist、取消、实际结果及构建检测不足分支均先CAS后写有效证据。Stale专门匹配，其余错误上抛；零行分类缺口见M2。CAS成功后证据写失败传播，恢复AlreadySucceeded缺失/归属不符写insufficient并NeedsUserAction，不假造passed、不自动重跑；诊断用attempt0不能算有效结果，故障矩阵未验足。
4. transition_task集中最终门禁，合法Completed仅AwaitingUserAcceptance；正常拒绝事务回滚状态/事件，accept和通用转换最终都到此，没有另一直接完工写state入口。B2/B3破坏其“当前”判定。
5. 新确认唯一键含dev attempt，旧NULL保留、新attempt可另插，精确find不命中NULL；多数确认/恢复调用已绑定dev。M3例外和m1展示另有问题。
6. v11两条一次性UPDATE仅分类迁移时非终态，强信号ai_review OR task_reviews；新任务和迁移时终态保持NULL，不实时分类。四表保留旧列内容、历史attempt补NULL，确认唯一键扩展，索引重建。DDL/复制/user_version同BEGIN IMMEDIATE，SQL失败回滚；backup API兼容WAL并校验副本。分类测试通过不等于v11故障注入/WAL恢复都验证。
7. 冻结：run_core_loop通过Queued/Running转换在执行AI前拒绝；通用Queued/Running/Completed、unblock在reset前、两个无适用确认入口在reconcile前检查。恢复扫描记账到Uncertain/Interrupted，不重跑AI。结构有防线，但缺入口零副作用矩阵；补建不得解冻而目前未实现。
8. 前端两个新增命令precheck/list_self_checks已注册；store成功响应按token+project/task防失效，切换清空；摘要取全历史最后一条而非采纳round current，仅供参考。读取失败被catch空处理可留旧摘要/预检，需交互验证。自检失败仍可看文档符合要求；Case C见M4。
9. 默认Tauri仍为单AI，双AI休眠函数未启用。未见Phase5A、GitHub导入、全站UI改造等混入；product既有修改保留。

## 实际验证和证据限制

执行前检查：数据库用memory/UUID临时目录；核心流程MockAdapter；adapter/process用nova_test_helper假CLI。测试也会运行临时项目git/受控命令和本地HTTP，不应把651项统称全部MockAdapter。未执行真实AI CLI、未打开真实应用数据目录或用户数据库。

| 本轮执行 | 结果 |
|---|---|
| cargo test --lib | 651 passed，0 failed/ignored，约77.66秒，两条新增dead-code warning |
| cargo check --lib --tests | 通过；覆盖库和测试，未额外重复独立cargo check --lib |
| cargo build --lib | 通过；不等于完整Tauri包 |
| npm run build | 通过，包含vue-tsc --noEmit及vite build，37模块 |
| npx vitest run | 5文件99 passed；含view文本17、store69；无新增WP2A交互专测 |
| 临时Rust独立程序直接path引用实际storage源码并链接既有依赖 | 运行成功；安全预期失败：B2错误Completed、B3当前代无确认Completed、M2迟到Config |

临时证据目录：
C:\Users\Lenovo\AppData\Local\Temp\nova-wp2a-review-11c361235c9a4c6fbf383eb3dac64e5e

含main.rs/audit.exe、race.rs/race.exe及新建隔离SQLite。rustc直接引用未修改应用模块，无已有测试改写；重跑应选全新空数据库避免重复夹具。关键输出：

```text
fallback_gate=Ok(Ok(()))
fallback_transition=Ok(Completed)
generation_race_transition=Ok((Completed, 2))
generation_race_current_acceptance=None
late_attempt1_while_attempt2_planned=Err(Config("只有处于 started 状态的动作才能被结束"))
late_attempt1_after_attempt2_succeeded=Err(Config("只有处于 started 状态的动作才能被结束"))
```

NOT RUN：完整Tauri包构建/签名安装；隔离app-data桌面启动及IPC真实点击；真实AI CLI；真实用户库迁移/恢复；所有分支迟到与证据写故障注入；v11专项中途失败和WAL恢复演练；Case C真实UI操作。**完整打包和启动应用是不同操作**，本轮未打包是验证缺口，不能说打包必然启动真实应用。本轮未回滚或恢复任何真实数据库。

## 后续通过条件

修复全部Blocker/Major后，至少执行任务包驱动的Mock端到端开发/修复与拒绝矩阵、补建命令全状态/Tauri/明确用户入口、双连接代际竞争、全部CAS分支/证据写故障恢复、冻结零副作用、NULL及不同attempt授权隔离、Case C前端恢复。再使用全新隔离app-data和Mock后端验证桌面IPC与异步切换；完整Tauri包另行构建验证。真实CLI和真实用户库不属于本轮授权动作。

当前WP2A未完成代码验收，不可进入基于验收通过的提交/里程碑准备，不批准发布。