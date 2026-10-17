# WP2A 整改开发报告

生成时间：2026-10-15 11:46:32 Asia/Shanghai。项目：D:\nova。

## 交付状态与责任边界

本轮完成针对 CODEX_WP2A_IMPLEMENTATION_REVIEW10151053 的整改代码与自动化自检，**等待Claude Code独立复审**。本报告不是独立审核PASS，不是最终验收或发布许可。未commit/push/发布/创建里程碑，未调用真实Codex/Claude CLI或付费服务。完成报告后停止。

分支feature/v0.1.0-mvp；HEAD及milestone/evidence-gate^{}仍为6a90df951c6c72ab7ae95228d5f2cc4de7e17273；tag对象da46c23c7055b7440534452cb37bd388073a4b84。`git remote -v`为空：没有配置远程。没有交付commit SHA；上述SHA只标历史基线。

读取了审核、Claude实现报告及实现说明、FINAL_AMENDMENT10141726、SELF_CHECK_SPEC10141417、MIGRATION_PLAN10141420；仅按明确引用核对10141347对应门禁/schema节。仓库及适用上级未发现AGENTS.md/CLAUDE.md规则。没有切换/清理/回退/stash/整文件恢复，历史产品和文档修改全部保留。

## 整改前检查点与版本定位

开始改源码前保全78个既有修改/未跟踪相关文件，位置：
`D:\nova\audit\checkpoints\wp2a-remediation-20261015-1112`

- `files/`：78文件原字节副本；只含既有rs/vue/ts/md，无密钥文件或真实用户数据库。
- `manifest.json`：整改前路径、字节数、SHA256、tracked标识。
- `tracked-before.patch`、`index-before.patch`、`status-before.txt`：原始tracked diff、index diff（空）、状态清单。状态采集时检查点自身已存在，其自身条目不属于原始应用变更；78文件manifest为精确原始集合。
- `source-remediation.patch`：**相对于检查点而非HEAD**的本轮20文件补丁，包含新增测试。原先干净的三个tracked文件以HEAD内容作前像，新测试以/dev/null作前像。文本patch统一LF，原始字节哈希单独保留。
- `source-manifest.json`：20文件前后字节哈希，SHA256 `0f940029e9a56fa32d6d3e9ba6958a3c6d342b7c65e88850293a5d9ccc092a17`。
- 本轮source patch SHA256：`30315bac6f0f0bcd432c72facccafa6d772608b70a0fb0319f4278f9323778ed`。
- `initial-files-after.json`：原78文件最终核对；62文件字节完全未改，16文件为本轮目标代码修改。两个product文件及全部既有docs逐一哈希不变。
- `rollback-drill-result.json`：在检查点内隔离副本反向补丁，20路径文本恢复通过，实际工作区交付哈希全部保持不变。没有在真实工作区撤销任何内容。

新增交付文档为本报告和 `docs/release-v0.1.0/NOVA_WP2A_REMEDIATION_NOTES10151146.md`；历史报告不覆盖。检查点在audit下，不属于应用构建输入。所有最终文件哈希另见delivery-manifest.json（不包含自身，避免自引用）。

## 逐项整改

位置均相对D:\nova。状态“已整改”指开发方代码与所列自检完成，不代替Claude复审关闭意见。

| 问题 | 修改位置与行为 | 验证 | 开发方状态 |
|---|---|---|---|
| B1 | core_loop.rs write_task_package：共享开发/修复包列真实field:index和文本、完整stdout合同；runtime/ai_adapter.rs Mock从实际包解析item，缺合同不造成功输出 | B1三项+18拒绝矩阵，开发/修复到accept闭环 | 已整改 |
| B2 | storage/tasks.rs复用documents::adopted_passing_generation_round固定tuple；dev/test/build归属失败立即拒绝，不搜索救活组合；保留合法generation-1 | 原审核反例回归、成功闭环 | 已整改 |
| B3 | transition_task取BEGIN IMMEDIATE后读Task并校验合法状态、冻结、generation相关门禁和事件 | 双连接真实业务状态ABA回归 | 已整改 |
| M1 | storage/requirement_items.rs事务化generate_requirement_snapshot；commands.rs/lib.rs注册；store/view只读展示四字段+明确按钮 | 14×2状态、归属、幂等、空快照、部分失败回滚、注册、前端显式调用 | 已整改；真实桌面IPC未运行 |
| M2 | storage/actions.rs先判断attempt替代，再诊断同attempt状态；gate_action不再吞Database错误 | dev/test/build×6后继状态、真实开发迟到、15分支迟到、SQL ABORT | 已整改 |
| M3 | documents.rs NULL实时授权全fail-closed；删除find_any例外；storage确认列表仅供审计，保留NULL和旧attempt | NULL各来源/不同attempt/精确attempt/历史列表及旧legacy断言收紧 | 已整改 |
| M4 | no_build_confirmation_needed支持Planned+精确dev确认的Case C；store现有显式确认恢复；view恢复后Queued仍可由用户显式启动 | Case C业务重入+store查询/点击/零AI启动+view绑定 | 已整改；真实桌面点击未运行 |
| M5 | 新实现/回滚说明取代旧报告不安全建议；不修改历史报告 | 20文件隔离反向补丁，SQLite WAL在线备份和关连接后隔离恢复 | 已整改 |
| M6 | 新增wp2a_remediation_tests.rs、迁移三项、前端10项；测试hook仅cfg(test)、线程局部one-shot；真实入口负向断言 | 后端674、前端109；详细索引下列 | 自动化缺口已补；实机IPC仍NOT RUN |
| m1 | evidence_overview.rs确认按开发归属和历史列表，不用test/build自身attempt；同代旧attempt标历史；entry增加is_current_attempt，前端双维度过滤 | dev1/test2/build2及同代旧attempt回归；旧generation断言改为无dev归属也属于历史 | 已整改 |
| m2 | evaluate_acceptance_submission仅预测显式验收；direct gate仍需当前确认；CompletionPrecheck增加can_submit_acceptance；自检不合格不能点通过，结果/拒绝可用；读取失败清旧数据并显示错误 | 首次验收precheck can_submit=true而ok=false，accept成功；18拒绝矩阵、异步失效 | 已整改 |
| m3 | 说明准确写四表结构性重建及SQLite online backup；移除未用REQUIREMENT_FIELDS，DevActionNotSucceeded被真实使用 | 迁移保留/原子性/WAL测试，check无上述dead-code warning | 已整改 |
| S1 | parser拒绝多块（含矛盾及重复）、未知顶层/coverage字段、缺check_results；只接受三种覆盖status，问题数组严格类型 | 18拒绝矩阵走stdout/持久化/accept；无新产品能力 | 已整改 |

额外保护：冻结任务accept在写user_acceptance前拒绝；持久化问题数组即使被错误标valid，缺失/损坏也fail-closed。单AI默认路径不变，未启用双AI、Phase5A或GitHub功能，未改全站UI。

## 先红后绿与失败记录

先加入B1/B2/B3回归及仅测试使用的事务前屏障，**修复生产代码前实际运行** `cargo test --lib wp2a_b -- --nocapture`：3 failed/0 passed，分别为任务包无标识、错误Completed、旧代确认完成新代。修复后同三项3/3通过。摘要在regression-red-evidence.txt，测试现已留在仓库，复现不依赖先前审核临时程序。

首轮全量660 passed/1 failed：旧时间线用例把同代但无开发action的证据当作当前。已把断言改为“generation标志为当前但is_current_attempt=false，timeline历史”，没有补造合格dev夹具绕过。最终674/674通过。legacy_unknown旧测试也收紧为NULL即使有非NULL确认仍不授权；没有放宽当前门禁。

有两次命令路径/参数准备错误（checkpoint --output参数、cargo/日志相对目录）在实际修改或测试执行前报错，随后修正并重跑；未把这些失败当通过证据。

## 测试索引：名称、断言与结果

下表前20项在core_loop/wp2a_remediation_tests.rs，后三项在storage/migrations.rs。命令：`cargo test --lib wp2a_ -- --nocapture`；最终还执行全量cargo test --lib，23项新增后端测试全部包含其中。每个矩阵内部逐case断言，数量不是独立审核质量结论。

| 关联 | 测试名 | 核心断言 | 结果 |
|---|---|---|---|
| B1 | `wp2a_b1_development_and_fix_packages_include_snapshot_and_stdout_contract` | 开发/修复包列全五个需求项（含goal重复文本的两个索引）、stdout合同和所有状态枚举 | 通过 |
| B1 | `wp2a_b1_actual_package_drives_development_and_repair_through_user_acceptance` | 实际任务包驱动Mock，开发及修复各一次，精确授权测试/构建后通过用户验收；2次开发、0次审核调用 | 通过 |
| B1 | `wp2a_b1_mock_without_dispatched_contract_does_not_fabricate_self_check` | 无合同任务包不能让Mock生成合格自检 | 通过 |
| B1/S1 | `wp2a_self_check_rejection_matrix_uses_real_stdout_and_accept_entry` | 18类stdout反例走真实解析/落库/accept入口，均拒绝且状态事件不变，结果文档仍可查看 | 通过 |
| B2 | `wp2a_b2_fixed_adoption_cannot_fall_back_to_older_generation_or_round` | 当前代采纳tuple attempt99不匹配，不能由旧代另一轮救活 | 通过 |
| B3 | `wp2a_b3_two_connection_generation_aba_rechecks_current_acceptance` | 双连接屏障，另一连接用实际SafeFixing→Verifying→PreviewReady→AwaitingUserAcceptance造成同状态ABA，旧用户确认不能完成新代 | 通过 |
| M1 | `wp2a_m1_snapshot_state_ownership_idempotence_and_freeze_matrix` | 14状态×已有/无快照，先归属及状态检查再幂等；Draft/终态零写入；冻结不变 | 通过 |
| M1 | `wp2a_m1_snapshot_insert_failure_rolls_back_every_item` | 第二个item触发SQL失败，首个item也回滚 | 通过 |
| M1 | `wp2a_m1_empty_snapshot_stays_empty_and_cannot_complete` | 空需求补建为空，不发任务包，不可完成 | 通过 |
| M1 | `wp2a_m1_tauri_snapshot_command_is_registered_with_ownership_parameters` | 静态注册/包装校验；不冒充真实IPC调用 | 通过 |
| M2 | `wp2a_m2_late_attempt_all_successor_states_and_real_sql_error` | dev/test/build × Planned/Started/Succeeded/Failed/Cancelled/Uncertain，旧attempt全Stale；SQL trigger ABORT为Database | 通过 |
| M2 | `wp2a_m2_real_development_late_results_stop_for_every_successor_state` | 实际开发入口执行中被替代，六种状态均不写自检/test/build、不transition | 通过 |
| M3 | `wp2a_m3_null_and_other_attempt_never_authorize_current_build` | NULL含机械/legacy来源全拒绝，其他attempt确认无效，精确attempt有效且历史两条保留 | 通过 |
| M4 | `wp2a_m4_case_c_query_and_explicit_resume_do_not_repeat_confirmation_or_reset` | Planned+既有确认可查询恢复，点击业务入口后Queued，不重复确认、不再增加attempt | 通过 |
| M6 | `wp2a_m6_all_test_build_evidence_branches_cas_before_write_and_late_results_stop` | 15分支×正常/迟到：检测不足、无适用、allowlist、取消、超时、pass/fail；触发器验证CAS在证据前；Stale无证据/事件 | 通过 |
| M6 | `wp2a_m6_post_cas_evidence_write_failure_propagates_and_recovery_is_fail_closed` | test/build CAS后写入故障传播Database；AlreadySucceeded缺证据恢复只写insufficient，停NeedsUserAction且不重跑 | 通过 |
| M6 | `wp2a_cached_evidence_for_another_dev_attempt_stops_test_and_build_recovery` | 缓存证据属于其他dev attempt，test/build均不可复用为通过 | 通过 |
| M6 | `wp2a_m6_frozen_execution_confirmation_retry_and_completion_are_zero_write` | 两种冻结分类启动/确认/重试/通用转换均零副作用；快照补建不解冻 | 通过 |
| M6 | `wp2a_frozen_existing_failed_actions_and_acceptance_are_unchanged_on_rejection` | 已有Failed action不reset，确认不写，冻结accept也不先写user_acceptance | 通过 |
| m1 | `wp2a_m1_m3_timeline_uses_dev_attribution_and_marks_same_generation_old_attempt` | dev1/test2/build2授权正确展示，其他dev attempt与同代旧证据标历史 | 通过 |
| M5/m3 | `wp2a_v11_preserves_four_tables_null_attribution_and_indexes` | 四表历史ID/字段/NULL保留，索引/FK完整，新attempt独立插入，重复自然键拒绝 | 通过 |
| M5/m3 | `wp2a_v11_failure_after_test_table_rebuild_rolls_back_the_whole_step` | 在test表DROP/RENAME后制造build表冲突，DDL/数据/user_version整步回滚，备份存在 | 通过 |
| M5 | `wp2a_online_backup_and_closed_connection_restore_include_wal_commits` | WAL未checkpoint时在线备份含已提交记录，关连接后恢复到新库路径，版本/内容/integrity/FK验证 | 通过 |

前端命令：`npx vitest run`（apps/desktop），新增10项全部通过：

- `snapshot generation is explicit and sends only project/task, not editable text`：选择任务不补建，明确操作仅传归属参数。
- `snapshot delayed success or failure cannot overwrite a different task`：旧响应/错误不污染新任务。
- `precheck ignores stale successes and errors across task or generation changes` 与 `selfcheck ignores stale successes and errors across task or generation changes`：两项it.each实例，覆盖成功/错误及任务/代际变化。
- `newer precheck response wins and a current query failure clears old success`：新token胜出，当前失败清旧结果并显示错误。
- `Case C backend query surfaces retry and click resumes to queued without starting AI`：显示恢复、点击后Queued、一次确认IPC、零start_core_loop。
- View文本测试：`shows the original four requirement fields before the explicit snapshot action`、`keeps result viewing and rejection available but gates acceptance on submission precheck`、`makes an explicitly resumed queued task startable without an automatic AI call`、`only labels evidence with both generation and dev-attempt match as current`。均通过；这些是源文本边界断言，非浏览器挂载/桌面IPC验证。

## 最终实际执行

| 命令/操作 | 结果及日志 |
|---|---|
| cargo test --lib | **674 passed / 0 failed / 0 ignored**，79.78秒；backend-final.log |
| cargo check --lib --tests | 通过；backend-check-final.log |
| cargo build --lib | 通过；backend-build-final.log |
| npm run build | vue-tsc --noEmit及vite生产构建通过；frontend-build-final.log |
| npx vitest run | **109 passed**，5文件；frontend-tests-final.log |
| npx tauri build --bundles nsis --config D:/nova/audit/checkpoints/wp2a-remediation-20261015-1112/tauri-bundle-overlay.json | **完整NSIS包成功**；tauri-package-final.log；原tauri.conf.json bundle.active=false未改，用audit内命令行overlay激活打包 |
| git diff --check | 通过；仅Git CRLF提示，不是代码错误 |
| git apply --reverse --check source-remediation.patch | 实际工作区只检查不执行；通过 |
| 隔离副本反向补丁 | 20文件通过，工作区哈希不变；rollback-drill-result.json |

最终安装包：`D:/nova/apps/desktop/src-tauri/target/release/bundle/nsis/Nova_0.1.0_x64-setup.exe`，3949603字节，SHA256 `40b18ce4eb8828cc47bca50005bc4723be0719e64467b1879e2fadae21814df1`。仅构建，未运行安装器、未安装、未启动桌面应用。构建下载NSIS编译工具并验证工具哈希，不是调用AI CLI。最终check无本轮此前两条dead-code warning；部分build仍有MSVC linker_messages“正在创建库/对象”的工具链提示，不写成零warning。

## NOT RUN及具体限制

- **真实桌面UI/IPC：NOT RUN。** 当前lib.rs::run固定用app.path().app_data_dir()并创建/迁移nova.sqlite3，start_core_loop使用真实CLI集合；现有桌面启动没有可证明隔离的app-data+Mock模式。未为本轮额外引入产品测试启动功能，因而不启动真实实例。业务入口、store invoke mock和注册断言不等价桌面IPC已通过；Claude应在安全隔离运行设施具备后补验。
- **安装/启动/签名验证：NOT RUN。** 打包成功与运行是两件事，本轮没执行安装器。未配置生产代码签名凭据，不声明签名发布验收。
- **真实Codex/Claude、真实用户库迁移/恢复：NOT RUN，且本轮禁止。** 未接触真实用户数据/密钥。测试使用内存或UUID临时目录SQLite、MockAdapter、假CLI和受控本地命令，不产生AI外部费用。
- CAS矩阵的test/build过程结果由线程局部测试边界注入；验证实际编排和数据库CAS/证据顺序，不能声称所有超时场景均实际等待10分钟。原进程/假CLI边界测试也随全量通过。
- Tauri包装/注册是类型检查和源码断言，非真实IPC端到端。

## 整改前后文件清单与diff

完整前像78文件见manifest.json；以下20文件为本轮源码/测试净变化。checkpoint=既有未提交修改前像；HEAD-clean=整改前干净tracked文件；new=本轮新增。其余既有文件原样保留。**这些是文件SHA256，不是提交SHA。**

| 文件 | 前像来源 | 整改前SHA256 | 整改后SHA256 |
|---|---|---|---|
| `apps/desktop/src-tauri/src/commands.rs` | checkpoint | `c18a20c8609fea250e7e5d99333edad05308ae493099694922b441529dd898b4` | `0196479d1cae5f58774c710ae3d44ad46fde9bab086774a43bf10136a06db2bc` |
| `apps/desktop/src-tauri/src/core_loop.rs` | checkpoint | `b6126d2a9a75493d1c2f3e44a03062338028a8d75b6d12a346f6693de5dfdee2` | `840247f16f415694f722723c799923542138c997c7ee594ac1aefb631b500222` |
| `apps/desktop/src-tauri/src/core_loop/documents.rs` | checkpoint | `24f7824b553bfa4ee8fd4c9c12a0055bae88d13efac729112f9c0986c2a71a20` | `77b38aa2212397b9f2ba401bb3f436f3dadf7d190ef150c8c7883e7681544f2b` |
| `apps/desktop/src-tauri/src/core_loop/evidence_overview.rs` | checkpoint | `e96ef192521684d97f57007c4c946b1b58cb427ac3cb25ec8bbbaa70c29e7fb8` | `ee98143332736835acc81ed4d28204603a3ad8af2ce1907462219a875f52bdde` |
| `apps/desktop/src-tauri/src/core_loop/self_check_parser.rs` | checkpoint | `76252d1c35a65bc4b96d49b58aefb4970ce651d9533e9859bc202a5d28141272` | `00131572c626916aec5e868bbf436b7c74da1376805cc39bb514f827d7d8b199` |
| `apps/desktop/src-tauri/src/core_loop/test_runner.rs` | HEAD-clean | `f9b54c51e9d632851d39f1386d4526b02edb9770b85e38287a922def6801ea2a` | `9c17824edd1465a819bf2f8d954907f60814a1547e4f72759f33e32463d19809` |
| `apps/desktop/src-tauri/src/core_loop/wp2a_remediation_tests.rs` | new | `NEW` | `a201fad07336084a9302766375169b5b4874b6835b061dfc34993a8d9420de4c` |
| `apps/desktop/src-tauri/src/lib.rs` | checkpoint | `69522f18a961fd91e6fd26f98c5b7e382b3aba0b87f9e84d4e0c2ce18b6fbc3a` | `c842a86b125abcff04fdc0c0b4c4362b5a761a73710a58d8e891398d8fd2ceea` |
| `apps/desktop/src-tauri/src/runtime/ai_adapter.rs` | checkpoint | `ab8932ae2a51eaf082b190fbcd098c8fc6f513e9682e2b44ffa3dc2e8f0cd0f7` | `036efe9ed322bcada72f5e103f10833e13078e69d867b8dd769a4782b0fb928b` |
| `apps/desktop/src-tauri/src/storage/actions.rs` | checkpoint | `0c860bbdef7601efcd9368ecd7784355a01318f2e674ba6d369d7807f063ba04` | `46d0a9dee59b6adad1d831b083a9d1dd172f80df10a8e123dfa5b4f561e115b0` |
| `apps/desktop/src-tauri/src/storage/migrations.rs` | checkpoint | `00e64b36fd5ae218b72f296aa0fc2f95158698329bf5007bc53b98fe1beb5aaf` | `a10da1fd34f936115b72b04d0a084d4657907aebf9e3c0a8fa3b6cef3d1d59f0` |
| `apps/desktop/src-tauri/src/storage/no_build_confirmation.rs` | checkpoint | `c10f149dbdba47b09f599e4fba4101f59264e6073c77fd214666269fb39221cc` | `47ec78ea63a51265d5996a21282bf0c83979e1c68da89604c946680e4ce44206` |
| `apps/desktop/src-tauri/src/storage/no_tests_confirmation.rs` | checkpoint | `b41f589610521b52a367aa02bac8473f6cad50ea9c373660e3aa4fdff26736d3` | `2b4deb4c5ba31a1eebd6aed501130e02136be9efb3aa7ae258225da3e0a6e4d9` |
| `apps/desktop/src-tauri/src/storage/requirement_items.rs` | checkpoint | `2a0097e8be63f4f37d4c0fbcf7c3b17762ec41c663324f0904ee29cf372e7fca` | `a4464d164cac4ebea8c728fd1bb1c76a954e2fe2076dbc80b05d0fcd2cb2993f` |
| `apps/desktop/src-tauri/src/storage/tasks.rs` | checkpoint | `b5a5536eabfda769a09f4d11387b8426201915b0a0fd7eb63befcd3cb9c9b740` | `c594c3b74cdc3efc65b760021d89d3a825713cdf49fd4529dac4a84e1496077c` |
| `apps/desktop/src/components/CoreLoopView.text.test.ts` | HEAD-clean | `ddedd37ab21749891c046a6a7cab54477916efe62f7c91fd2fce10ae17f32a34` | `896f7075d9b93506ac21570f03eae71ab66e745151dd938f65464927a0614a10` |
| `apps/desktop/src/components/CoreLoopView.vue` | checkpoint | `42f129b0221d8cb5a278dc99b297c61fabafe8c6fafc520fb0433f4e1e4fd1d7` | `5691d5d032b0c281e9ad50fbe2918d437ea6bf4ecb8173bc9866277ea72adbee` |
| `apps/desktop/src/stores/coreLoop.test.ts` | HEAD-clean | `409eec0f887e8fd8915041176c28cac29101e7ba14a76e3ddb8eba87663723a6` | `19ac0a0f30b7ebde697403754fd3873f1604a4e3680948db3816f9781a64b9c1` |
| `apps/desktop/src/stores/coreLoop.ts` | checkpoint | `dbc936cd429d7e6e0110bf73def965bfdd05017e4a62ddae330d883b36d8e838` | `23a4fa64b90a8c5c7ceb4c3bb8488b76eb789a3111622300dcc3f4a2e9951278` |
| `apps/desktop/src/types.ts` | checkpoint | `a89f9e49711322cb7c8174f508534571cb82ab8a20bba95d195f4b97a95adb93` | `ae19b8f118966d427c78f6c46357583c2aa8dcd8d6268c2619e487e34d5f86c6` |

## 给Claude Code的独立复审索引

1. 先读原审核 `docs/reviews/CODEX_WP2A_IMPLEMENTATION_REVIEW10151053.md`、本报告及 `docs/release-v0.1.0/NOVA_WP2A_REMEDIATION_NOTES10151146.md`；按适用规格顺序核对，不把Codex自检作为独立结论。
2. 校验source-manifest.json及source-remediation.patch哈希；相对于检查点审本轮20文件增量，同时回查完整未提交实现。不要把全部HEAD diff当成本轮整改。
3. 按B1→B2→B3追踪任务包、Mock合同、采纳tuple、事务内Task读取；复核B3屏障不是只改夹具绕过竞态。
4. 按上表测试名重跑，重点检查NULL/不同attempt、Case C可达性、冻结零写入、CAS后证据故障及恢复；测试hook只在cfg(test)，release不含这些注入路径。
5. 查看backend-final.log、frontend-tests-final.log、tauri-package-final.log和rollback-drill-result.json。审视剩余实机IPC限制，独立给出结论；不要推导已批准发布。

交接状态：**整改交付、独立复审待执行**。现在停止，不执行提交、推送或里程碑操作。
