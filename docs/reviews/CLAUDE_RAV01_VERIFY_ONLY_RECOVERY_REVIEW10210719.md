# RAV01「仅重新验证现有修改」独立审核（Claude Code）

生成时间：2026-10-21 07:19 Asia/Shanghai。项目：D:\nova。审核人：Claude Code；开发/整改：Codex。

基线HEAD：`9e1de55eddb2f991df5d5614675a14d2693d34de`（与预期一致，未提交）。证据目录：`D:\nova\audit\rav01-verify-only-20261021-015036`。

## 唯一结论

**代码审核：PASS**，未发现阻断问题；附一项非阻断的测试严格度观察（见第八节）。

**是否允许候选构建：允许**。

**真实任务资格仍未实测**：本轮全部验证均为隔离夹具/本机Rust-Vue测试，真实`RAV01-title-retest-02`任务的资格查询与恢复执行**从未对真实任务数据库运行**——详见第九节的最小操作步骤。

## 一、基线与交付文件核验

- `git log -1`确认HEAD为`9e1de55eddb2f991df5d5614675a14d2693d34de`，分支`feature/v0.1.0-mvp`，与指定基线一致；暂存区为空。
- 八文件（7份已跟踪修改+1份新增`core_loop/verify_only.rs`）**逐一独立重算SHA256，与`source-manifest.json`及报告声明值完全一致**：

| 文件 | 独立重算SHA256 |
|---|---|
| `apps/desktop/src-tauri/src/commands.rs` | `becde28959525cd61f13255e8f19f569e73f26bddc14a68c7b2d2eab3acfbd35` |
| `apps/desktop/src-tauri/src/core_loop.rs` | `2ed7be5d35d012dec258885555839f510dac8acbb8d4b54a57f66a4dc7dbff3a` |
| `apps/desktop/src-tauri/src/core_loop/verify_only.rs` | `75d9422656a3b24fa8dcae428fe863ed8c6cb6e3a84b47fec596fcc5a8c4bd3c` |
| `apps/desktop/src-tauri/src/core_loop/wp2a_remediation_tests.rs` | `e037b6d00bb10d69a6f36f7e5eaf424a1b0b17fac73474cbf84cb81fc75ba1e8` |
| `apps/desktop/src-tauri/src/lib.rs` | `0db43cad68cd89103419f39871329db72232d89d277a4711d61e0cc543a8bd8d` |
| `apps/desktop/src/components/CoreLoopView.vue` | `b8635274df5ccd69add8516223272d37349b4ad4b144957f56ca8c74b5c1108a` |
| `apps/desktop/src/stores/coreLoop.ts` | `1adff0d76a0030ad807285aefb5686a366a47f00e82231fbe64a6a0a6c8130c3` |
| `apps/desktop/src/uiRemediation.interaction.test.ts` | `39396b98bda5c7dcbd19f0b7d7531d6e2f400048589aedf38c90b3c2fbfa8c01` |

- `git apply --reverse --check review.diff`：退出码0。`git diff --name-only`确认恰为7份已跟踪文件+2份既有product历史修改，无其他变化；未发现并行UI/UX文档（`Nova UIUX/`、`Novauiux/`）被触及。

## 二、后端资格校验（`verify_only::eligibility`）—— **逐项确认成立**

直读`core_loop/verify_only.rs:20-100`：

| 要求 | 核验结果 |
|---|---|
| 项目归属 | `task.project_id != project_id`即拒绝 |
| 状态 | 必须恰为`TaskState::NeedsUserAction`，非此状态拒绝 |
| 冻结 | `is_legacy_frozen(&task)`为真即拒绝 |
| 代际/轮次 | `round`由`current_pending_round`推导（不信任调用方），`generation`取自任务自身字段，均为后端权威值 |
| 开发attempt | 必须找到`dev-{task_id}-round-{round}`动作且`status==Succeeded`；自查通过`current_self_check(task_id, round, dev.attempt_number)`按**精确**该attempt查找，不是"最新一条" |
| 自查coverage | 先验证`unfinished_items_json`/`blocking_issues_json`均为空数组；再构造已确认需求快照的`{field}:{item_index}`集合，逐条核对自查`requirement_coverage_json`的每一项`status=="covered"`、属于预期集合、无重复，最终要求`seen集合==expected集合`（不多不少）且`expected`非空——严格的双向集合相等检查，不是宽松的"至少覆盖"或"数量相等" |
| 测试命令 | `test_command_redacted`非空字符串才通过 |
| 必须存在真实失败 | 检查当前代际/轮次的test/build两个action：若存在且状态为`Started`/`Uncertain`立即拒绝（注释明确"Never acknowledge uncertain or live attempts"）；必须至少一个为`Failed`才认定"有可恢复的失败"，否则拒绝——避免对本不该出现"仅验证"入口的任务（例如尚无失败记录）误放行 |

## 三、零AI调用与不影响正常一次返修规则 —— **确认成立**

- `verify_only::run`函数签名**没有`adapter: &dyn DevReviewAdapter`参数**，整个模块没有任何构造/探测/调用开发或审核AI适配器的代码路径——不是"有调用但被条件跳过"，而是类型层面就不存在这条路径。
- 所有失败分支（`TestRoundOutcome::Failed`/`BuildRoundOutcome::Failed`）统一经`stop()`直接`transition_task(..., NeedsUserAction, ...)`，**从未**调用`transition_task(..., SafeFixing, ...)`。独立复核新增测试`assert_verify_only_no_ai`辅助函数（每个测试用例末尾调用）：断言`ai_development`动作数恒为1、无任何"review"类动作、`task_state_events`中从未出现`new_state==SafeFixing`的记录——这是对"零AI调用"最直接的断言，而不是从代码推断。
- 因为从不触发`SafeFixing`转换，普通开发流程用于限制"一次自动修复"的`count_automatic_safe_fixing_entries`计数逻辑（本轮未改动）**完全不会被此路径影响**——不是"共享计数但本轮不增加"，而是该计数的输入来源（`SafeFixing`转换记录）本就不包含仅验证路径产生的任何记录。

## 四、共享运行槛、Action Ledger、attempt/CAS、跨连接竞争 —— **确认成立**

- `commands.rs:999-1001`的`verify_existing_changes`命令直接使用`state.core_loop_cancel`——与`start_core_loop`（`commands.rs:878`附近）**完全相同**的共享HashMap运行槛，`contains_key(&task_id)`判重复、`insert`占位——不是另起一套独立的运行状态。
- `verify_only.rs:114-126`：独立操作用`verification_only`类型、`verify-only-{task_id}-g{gen}-r{round}-a{attempt}`为key的Action Ledger条目，复用既有`gate_action`（同一套CAS机制），只有`ActionGate::Started`（代表这是一次全新/刚被reset的尝试）才继续，其余情况（已成功、不确定）直接拒绝"请勿重复操作"。
- 测试/构建动作仅通过`reset_failed_action_for_retry`重置**已失败**的那个（`WHERE status='failed'`条件保证未失败的另一个不受影响），随后沿用既有`run_tests_for_round`/`run_build_for_round`内部的`finish_action(expected_attempt=...)`CAS（本轮未改动该部分逻辑本身）。
- 跨连接并发：独立复核`rav01_verify_only_two_connections_cannot_claim_same_operation`——用`VACUUM INTO`制作真实文件型SQLite副本，两个**真实OS线程**各自`Database::open`独立连接，`Barrier`同步后同时调用`verify_only::run`，断言恰好1个线程成功、最终恰好1条`test_evidence`、恰好1条`verification_only`动作——这是真实跨连接竞争测试，非模拟。本机独立重跑该测试（见第八节）通过。
- 前端侧：`verify_existing_changes`命令在拿到运行槛锁**之后**仍重新查询一次`eligibility`并与前端携带的`expected`比较（`commands.rs:1003-1006`），不信任前端缓存的资格快照；`verify_only::run`内部进入闭包后**再次**调用`eligibility`比较（注释"A competing request cannot pass this operation's Action Ledger CAS"），并在执行到测试/构建前各调用一次`ensure_identity`复查代际/轮次/开发attempt/测试命令——多层防御，不是单点检查。

## 五、历史证据复用条件 —— **确认成立**

验证前对已失败的测试/构建动作执行`reset_failed_action_for_retry`，使其重新经`run_tests_for_round`/`run_build_for_round`的`ActionGate::Started`分支**真实重跑**，不会复用旧的失败证据；若另一个（未失败）动作此前已`Succeeded`，则会命中`ActionGate::AlreadySucceeded`——此时沿用的是本仓库既有（未改动）的"`evidence.result_attempt == Some(dev_result_attempt)`才可信任缓存证据，否则`fail_closed_stale_test_evidence`拒绝"规则，本轮未放宽或绕过此既有fail-closed检查。

## 六、成功路径 —— **仅进入等待用户验收，不写验收记录，不放宽完工门禁**

`run()`成功时调用既有`finish_success_path`（本轮未改动，`git diff`确认），该函数本身即是独立测试/构建证据的二次防御性门禁（无证据则拒绝进入`preview_ready`），最终落点为`AwaitingUserAcceptance`。独立复核`rav01_verify_only_success_preserves_development_and_requires_user_acceptance`：断言最终状态为`AwaitingUserAcceptance`；**逐字段比较**验证前后的开发动作完整JSON序列化结果完全相等（不仅是attempt_number，是整行记录逐字段不变）；并确认对同一`expected`再次调用`run`会失败（因任务已离开`NeedsUserAction`，资格天然失效，不能重复进入"验收"）。

## 七、前端入口 —— **逐项确认成立**

直读`stores/coreLoop.ts`与`components/CoreLoopView.vue`的新增diff：

- **可达性**：`watch`监听项目/任务/状态/代际变化、`immediate:true`立即触发`loadVerifyOnlyEligibility`；按钮区`v-if`要求状态为`needs_user_action`**且**`verifyOnlyEligibility`非空**且**`verifyOnlyTaskKey`与当前"project:task:generation"组合完全匹配——三重条件缺一不显示。
- **展示沿用命令**：按钮区上方`<p>沿用测试命令：{{ eligibility.test_command }}</p>`直接展示后端返回的真实命令字符串。
- **拒绝迟到资格响应**：`loadVerifyOnlyEligibility`用`token = ++this.verifyOnlyQuery`模式，`await`结束后同时核对token未过期**且**project/task/state/generation均与发起查询时一致，否则丢弃响应不写入store。独立复核新增交互测试`hides the entry when backend rejects eligibility and discards late foreign-task results`：真实构造"查询发起后立刻切换到另一任务，稍后原查询才resolve"的竞态场景，断言`verifyOnlyEligibility`保持`null`、按钮不出现——这是对真实竞态的复现，不是对token变量的孤立单测。
- **错误不回落普通核心循环**：`verifyExistingChanges`的catch块仅设置`this.lastError`，没有任何调用`store.startCoreLoop`或其他回退逻辑。独立复核新增交互测试`shows request errors and never falls back to ordinary development`：模拟后端拒绝，断言错误文字可见、`verifyingOnly`复位为`false`、且`start_core_loop`从未被调用。
- 另一新增测试`shows an entry... dispatches only verify-only once`：真实点击按钮，断言`verify_existing_changes`恰被调用1次且参数（`projectId`/`taskId`/`expected`）与预期完全一致，`start_core_loop`/`get_core_loop_readiness`/`confirm_task_test_command`均未被调用——直接证明新入口与既有入口互不混淆。
- `lib.rs`确认两个新命令（`get_verify_only_eligibility`/`verify_existing_changes`）已在正常（非`wp2a-desktop-validation`调试feature限定）的`invoke_handler`列表中注册，是面向正常用户路径的真实功能，非调试设施。

## 八、独立验证命令与实际结果

本机独立按既有CI模式预热后运行：

| 命令 | 实际结果 |
|---|---|
| `cargo build --locked --features nova-test-helper --bin nova_test_helper` | 通过，23.63秒 |
| `cargo test --locked --lib app_paths::tests::accepts_ordinary_uuid_like_components --target-dir <临时嵌套缓存>`（预热） | 1 passed，含真实冷编译26.14秒 |
| `cargo test --locked --lib`（完整，预热后） | **694 passed，0 failed**，81.02秒——与报告最终复核结果一致 |
| `cargo test --locked --lib rav01_verify_only -- --nocapture` | **6 passed，0 failed**，2.00秒 |
| `npm.cmd test -- --run`（apps/desktop） | 6文件，**145 passed，0 failed**（含本轮新增3项交互测试：142+3=145） |
| `npm.cmd run build`（apps/desktop） | `vue-tsc --noEmit`+`vite build`均通过 |

### 对"首次完整测试13项失败"的独立复核结论

直接核对`full-rust-lib.log`原始内容（非转述报告摘要），确认13项失败的具体性质：

- `real_cargo_test_run_against_this_crate_produces_a_real_pass_count`：panic信息"执行超时，已终止进程"后紧跟"Compiling nova v0.1.0"——确认是该嵌套自引用cargo测试**未经预热**时的冷编译超时，这是此前WP2A/RAV01多轮审核已明确记录的既知风险（需要先跑一次独立预热步骤），不是本轮新引入的缺陷。
- `immediate_grandchild_cancel_terminates_parent_and_grandchild_repeated`：panic信息明确写出"grandchild pid file never became ready **within 10s (waited 9890.4828047s)**"——预期最多等待10秒，实际观测到**9890秒**（约2.75小时）。这一数字本身就是证据：不是代码逻辑错误导致的"等待时间计算错误"，而是运行环境在测试执行期间经历了与此量级相当的真实挂起（最常见解释是系统睡眠/休眠被中途唤醒，但本机也无法找到可确认的系统事件记录加以证实，与报告"不能据此断言系统休眠或具体外部根因"的谨慎措辞一致）。
- `windows_unicode_and_space_containing_project_paths_work_through_the_real_process_boundary`：panic为`PoisonError`——是**另一个测试线程panic后共享Mutex被污染**的级联效应，不是该测试自身验证的Unicode路径处理逻辑有缺陷。
- `rav01_verify_only_real_isolated_build_success_and_failure_never_dispatch_ai`：`left: "timed_out", right: "failed"`——真实构建子进程（仅`process.exit(1)`，正常应在毫秒级完成）未能在该次运行的系统资源状况下及时完成，被安全地分类为`timed_out`而非误判为`passed`；这证明**verify_only的异常分类机制本身工作正常**（没有把超时悄悄说成通过），只是该测试的断言写法是严格`assert_eq!`到单一期望值"failed"，没有把"timed_out"也当作同样安全、可接受的替代结果——**这是测试严格度的非阻断观察，不是产品逻辑缺陷**；本机独立重跑该测试（预热后，正常系统状况）已验证通过。
- `immediate_grandchild_timeout_terminates_parent_and_grandchild_repeated`：等待2.0117964秒，略超2秒阈值——与上述系统性延迟同属一批，幅度小，大概率是前述大范围系统停顿恢复过程中的尾部抖动。
- 其余`runtime::ai_adapter::tests::*`多项失败均为对真实子进程/CLI探测场景的断言，在同一时间窗口内集中出现，与上述系统级停顿时间窗口吻合，判断为同一根因的连带效应，而非各自独立的逻辑缺陷。

**结论：这13项失败均可归因于该次运行过程中一次量级约2.5-2.75小时的系统级运行停顿（最可能是睡眠/休眠或同等严重的资源冻结），叠加一次已知的冷编译超时与随后的锁污染级联，不构成本轮`verify_only`实现或任何既有代码的未解决回归。** 本机独立重跑（预热后，同样的断言、同样的超时阈值、同样的完整lib命令、默认并行度）**694 passed，0 failed**，与报告最终复核结果一致，没有为了"跑到通过"而放宽任何断言或缩小测试范围——这点已通过逐命令比对确认。

## 九、安装后针对真实RAV01-title-retest-02任务的最小操作与核验步骤（供PM后续执行，本轮未执行任何一步）

1. 候选构建与安装：基于本次已审核的八文件变更构建新候选NSIS安装包，在已有的隔离安装环境中安装（复用此前已验证的隔离流程），**不覆盖、不接触任何真实用户现有Nova安装或数据**。
2. 打开`RAV01-title-retest-02`任务页面，确认其仍为`needs_user_action`状态，确认此时界面上**是否自动出现**"重新验证现有修改"入口及"沿用测试命令：npm test"文案——这是对`get_verify_only_eligibility`真实调用路径的**首次**真实验证（本轮全部为隔离夹具模拟）。
3. 若入口出现：点击"重新验证现有修改"，观察：
   - 按钮立即禁用并显示"正在请求仅验证…"；
   - 任务状态经`queued`→`running`→`verifying`最终到`awaiting_user_acceptance`（通过，本轮解析修复生效）或回到`needs_user_action`（仍失败，需查看新的失败原因文案）；
   - 全程确认**没有**任何开发/审核AI被调用的迹象（如CLI进程、新的开发轮次产物）；
   - 任务原开发记录（round 1的开发动作、自查内容）应与验证前完全一致。
4. 若入口未出现：说明真实任务的资格条件与预期不符（例如自查/快照记录与本轮校验逻辑假设的字段格式有出入），应如实记录具体缺口，**不应手动修改数据库字段或绕过UI强行调用命令**来"造出"入口。
5. 无论成功或失败，均不代表完整v0.1.0发布验收通过——后续仍需独立的安装/原生点击/视觉复验（沿用此前多轮审核已反复强调的边界）。

## 十、现场保护确认

本轮仅执行只读`git status`/`git log`/`git diff`/`git apply --reverse --check`、对八份交付文件及证据目录原始日志（含两份完整Rust lib运行日志）的只读读取、`cargo build`/`cargo test`（含预热步骤）、`npm.cmd test`/`npm.cmd run build`本地验证命令。未执行`git add`/`commit`/`push`/`reset`/`clean`/`stash`；未修改任何源码或测试；未调用真实AI；未访问、运行或修改`D:\NovaRAV01Project`或原任务数据库；未改动并行UI/UX文档或其他工作区资料；未安装、未打包、未发布。`git status`核对显示与审核开始前一致，仅本报告为新增。

完成审核，到此停止。交PM安排候选构建与第九节所列最小复验步骤。
