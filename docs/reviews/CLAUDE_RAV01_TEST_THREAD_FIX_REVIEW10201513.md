# RAV01 测试线程异常修复独立审核（Claude Code）

生成时间：2026-10-20 15:13 Asia/Shanghai。项目：D:\nova。审核人：Claude Code；开发/整改：Codex。

基线HEAD：`e6472f12fe65353efc07f14d3bf7d7d1d03ec327`（与预期一致，未提交）。证据目录：`D:\nova\audit\rav01-test-thread-fix-20261020-113337`。

## 唯一结论

**代码审核：PASS**，未发现阻断问题（详见第五节的一项非阻断性观察）。

**是否允许进入候选构建：允许**——建议PM安排基于本次提交的新候选构建，用于后续"仅复验测试解析这一步"的定向复验。

**后续复验方案**：现有恢复机制（`confirm_task_test_command` → `unblock_test_execution_for_retry`）**支持**只复验未完成的验证阶段、无需重跑开发AI、无需直接改库或重置项目——详见第六节。

**重要边界重申**：隔离夹具/本机Rust测试通过**不等于**真实RAV01闭环已通过；原任务现场仍为`needs_user_action`，本轮及本次审核均未重试原任务、未改原任务数据库。

## 一、基线与交付文件核验

- `git log -1`确认HEAD为`e6472f12fe65353efc07f14d3bf7d7d1d03ec327`，分支`feature/v0.1.0-mvp`，与指定基线一致；暂存区为空。
- `git diff --name-only`确认恰好4个本轮文件+2份既有product历史修改（与此前所有轮次一致），无其他变化；未发现并行UI/UX文档（`Nova UIUX/`、`Novauiux/`等）被触及。
- 四文件**逐一独立重算SHA256，与`source-manifest.json`及报告声明值完全一致**：

| 文件 | 独立重算SHA256 |
|---|---|
| `apps/desktop/src-tauri/src/core_loop.rs` | `18626745b817abb49523d1e8fb237724e716962f57283776145d307e467694b2` |
| `apps/desktop/src-tauri/src/core_loop/test_runner.rs` | `126148d933f130ed74961def5ff9b8cdef25ac20cc0360272323439efe87d0a7` |
| `apps/desktop/src-tauri/src/core_loop/wp2a_remediation_tests.rs` | `31a7275994cf7c1a2a47b12ed37466e49aa97e5b37a69dd1e6460e07e7aad1af` |
| `apps/desktop/src-tauri/src/security.rs` | `5b77a0108b88d98f3ca110f05da8b209186c56f5139e52451bc20f5a79127f23` |

- `git apply --reverse --check review.diff`：退出码0，补丁边界与实际diff一致。
- `git diff --stat`确认改动量极小、集中：`core_loop.rs`仅10行、`test_runner.rs`169行（解析逻辑主体）、`wp2a_remediation_tests.rs`50行（新测试+字段默认值补齐）、`security.rs`25行（新增`panic_diagnostic`）——未发现超出本轮根因范围的改动，`run_tests_for_round`中既有的attempt/CAS/TestActionGuard逻辑本身未被触及（只是新增了对`infrastructure_error`的读取分支）。

## 二、修复前证据核验 —— UTF-8边界问题与真实生产执行器复现 **均独立确认成立**

直接核对原始保全证据，不只转述报告：

- `test_evidence.json`为空`[]`，`task_actions.json`中`test_execution`动作`status=failed`、`error_summary="测试执行提前结束或异常终止，未获得有效结果"`（10:46:40.163→10:46:40.825，约662ms）、`task_state_events.json`最终事件`reason="执行受阻：配置错误：核心循环执行线程异常终止，未获得有效结果"`——三者互相印证，与报告"test_evidence/build_evidence均空、动作异常收口已生效但原因是通用文字"的描述完全一致，确认原任务确实卡在当时已有的guard通用兜底文案上，而非证据保存SQL本身失败或进程启动失败。
- `isolated-npm-test.stdout.log`独立确认真实Node 24输出含`ℹ pass 2`/`ℹ fail 0`/`ℹ skipped 0`（三字节UTF-8字符'ℹ'紧邻ASCII文本）。
- `parser-reproduction-before-fix.log`：真实panic堆栈`start byte index 156 is not a char boundary; it is inside 'ℹ'`，定位修复前`test_runner.rs:410`的`extract_count_before`、经`parse_counts`（388行）调用。
- `real-runner-reproduction-before-fix.log`：**第二份独立复现**，panic位置相同（`test_runner.rs:410:30`，字节索引146，同样落在'ℹ'内部），调用链为`run_tests`（真实生产测试执行器，非仅隔离调用`parse_counts`）→`parse_counts`→`extract_count_before`——确认缺陷通过**真实生产执行路径**可复现，不是仅理论上存在于某个孤立单测里。

两份修复前panic日志、原任务四张证据表、隔离npm输出三方互相印证、细节逐字核对无夸大或虚构。

## 三、Node原生/TAP计数解析审核 —— **准确性与回归保护均确认成立**

直读`test_runner.rs:395-445`：

- **UTF-8安全修复**：`extract_count_before`改用`trimmed.char_indices().rfind(|(_,c)| !c.is_ascii_digit()).map(|(i,c)| i + c.len_utf8())`——按**字符**定位、用该字符真实的`len_utf8()`（而非假设的`+1`）计算下一字符的字节起点，确保切片边界永远落在合法UTF-8字符边界上，不论该字符是1字节ASCII还是3字节的'ℹ'或中文字符。独立复核新增测试`rav01_unicode_and_tap_summaries_do_not_invent_counts`第一个用例`"中文3 passed; 1 failed; 0 skipped"`→`(3,1,0)`：中文字符紧邻数字也不会误切或崩溃，验证的是比原panic样本更严格的多字节边界场景，非凑巧绕过。
- **新增`extract_node_count`**（`:406-416`）专门处理"标签在前、数字在后"的Node/TAP整行摘要格式（与Cargo"数字在前、标签在后"相反），要求整行**恰好**两个词（可选前缀`ℹ`/`#`后剥离）、第二个词必须全为ASCII数字，否则返回`None`——独立复核`rav01_unicode_and_tap_summaries_do_not_invent_counts`最后一个用例`"中文 failed\nℹ skipped unknown\npass 2 is a test name"`→`(None,None,None)`：非数字"unknown"与三词"pass 2 is a test name"均正确拒绝，不误读普通日志行、不把未知结果当零。
- **不重复计数**：`parse_counts`用`.or_else(...)`链式调用两个解析器，命中其一即停，不存在两者同时匹配相加的路径。
- **既有Cargo/pytest/Jest行为未回归**：`extract_count_before`函数签名、"扫描全部出现位置、从后往前找第一个有效数字"的核心算法、及其应对"stderr可能携带无数字的countless文本"的既有设计（函数头注释原样保留）均未改变，只是内部字节边界计算方式被修正；完整`cargo test --locked --lib`681项非rav01测试（本轮之前已有）本机独立重跑**全部保持通过**（见第六节），未发现任何既有框架的计数回归。

## 四、解析异常处理 —— **真实退出码/脱敏输出/具体阶段保留，且阻止验收与自动AI返修** 均确认成立

直读`test_runner.rs:301-383`与`core_loop.rs:2653-2714`：

- `run_tests`现在用同一个`catch_unwind`同时包裹进程执行（`process::execute_blocking`）**和**结果解析（`classify_test_result`），用`stage`单元格在二者之间切换文案（"测试进程启动/执行"→"测试结果解析"），修复了原"`catch_unwind`只包进程执行、解析panic会穿透测试函数"的缺口。
- 进程结果通过外层`captured`变量提前捕获，即使解析阶段panic，`interrupted_test_outcome`仍能从`captured`取回**真实**`exit_code`（可能是`Some(0)`）与**真实**stdout/stderr，不会因为解析失败就丢失进程本身的真实结果。
- `TestRunOutcome`新增`infrastructure_error: Option<String>`字段，与"真实测试失败"严格区分（`classify_test_result`仅在`ExecutionOutcome::Failed`且`kind`不属于`NonZeroExit`/`TimedOut`/`Cancelled`时才置位）。
- `core_loop.rs:2708-2714`：`if outcome.infrastructure_error.is_some() || (...)`直接转`NeedsUserAction`并返回`TestRoundOutcome::ActionUncertain`，**不经过**会触发自动修复（`SafeFixing`/二次开发AI调用）的正常失败分支，注释明确"infrastructure failure... must never trigger the automatic developer repair"——独立复核新增端到端测试`rav01_parser_fault_after_exit_zero_is_saved_without_ai_retry`：用`RUN_OUTCOME_OVERRIDE`模拟"退出0+infrastructure_error"，跑真实`run_core_loop`，断言任务转`NeedsUserAction`、`adapter.development_call_count()==1`（未二次调用AI）、**`test_evidence`确实写入1条、status=`failed`、exit_code=`Some(0)`、reason含真实诊断文本**——直接证明原"界面保持尚未运行"的缺陷已被修复：证据现在会被写入，不再是空列表。

## 五、panic诊断脱敏/Unicode截断/既有门禁 —— **确认成立，附一项非阻断观察**

- `security.rs:65-70`新增`panic_diagnostic`：支持`String`/`&str`/非文本payload（后者给出固定中文提示，不崩溃也不猜测内容）；**先**调用既有共享的`redact_sensitive_text`（与全应用统一脱敏管线复用，非另起一套弱规则），**再**用`.chars().take(2000)`按**字符**截断——有意识地避免了本轮bug本身的"按字节截断导致越界"问题在新代码里重演。
- `core_loop.rs:324`的`report_run_failure`与`test_runner.rs:336`的panic捕获均已切换为调用`panic_diagnostic`，不再是此前版本的通用兜底文案——独立复核`rav01_outer_panic_reports_redacted_payload`与`rav01_parser_panic_keeps_real_exit_and_redacted_stage`两项测试，均用构造的假token（`sk-`+32个重复字符，非真实凭据）验证脱敏后的文本既保留了有意义的诊断信息又不含该token。
- grep确认全代码库无`std::panic::set_hook`/`take_hook`调用——未安装全局panic hook，符合声明。
- `git diff --stat`确认`core_loop.rs`改动仅10行，`run_tests_for_round`函数体内原有的`gate_action`/`TestActionGuard`/`finish_action(expected_attempt=...)`CAS逻辑、取消（`outcome.cancelled`分支）、超时（`outcome.timed_out`分支）代码位置与逐字内容均未改变，只是新增了对`infrastructure_error`字段的读取与一个新的早退分支——本机完整681项（本轮之前）既有测试全部保持通过，独立确认这些门禁未被扰动。

**非阻断观察**：`core_loop.rs:2708-2709`的早退条件为`outcome.infrastructure_error.is_some() || (!outcome.process_succeeded && outcome.exit_code.is_none() && !outcome.timed_out)`，后半句OR条件在`infrastructure_error`字段引入后看起来与`interrupted_test_outcome`（恒定设置`infrastructure_error: Some(...)`）产生的路径有重叠，可能是上一轮遗留、现在部分冗余的防御性分支。经核对它是**纯粹的OR增补**，不会与新字段冲突或制造反例（只会让更多情况一致地走向"需要用户处理，不自动修复"），**不构成正确性缺陷**，建议后续清理时留意，本轮不要求必须修改。

## 六、独立验证命令与实际结果

| 命令 | 实际结果 |
|---|---|
| `cargo test --locked --lib`（完整Rust lib测试，本机独立重跑） | **688 passed，0 failed，0 ignored**，76.54秒，退出0——与报告声明完全一致 |
| `cargo test --locked --lib rav01 -- --nocapture`（本机独立重跑，含panic注入可见） | **14 passed，0 failed**，0.83秒；控制台可见6处**预期注入**panic（均为测试构造的假token/假错误，非真实数据），每处后紧跟`... ok`，确认均被正确捕获而非真实失败 |
| 独立复核`rav01_real_npm_node_summary_in_isolated_fixture`源码 | 使用`temp_dir(...)`在系统临时目录创建全新、自建内容的隔离Node项目（独立`package.json`/`index.html`/`acceptance.test.mjs`，与`D:\NovaRAV01Project`无关），真实运行`npm test`，断言退出0且计数`(2,0,0)`，测试结束`fs::remove_dir_all`清理——确认该用例所用真实npm执行确实发生在隔离夹具内，未触碰原RAV01项目 |
| `git apply --reverse --check review.diff` | 退出0 |

本轮未调用真实AI、未修改或运行`D:\NovaRAV01Project`、未重试原任务、未修改数据库或全局环境——全部验证均为本地只读文件核对、cargo命令及已保全证据的离线分析；未重跑前端测试（本轮无前端源码/IPC契约改动，与报告"本轮未重跑"的说明一致，经`git diff`确认属实）。

## 七、后续复验方案：如何只复验未完成的验证阶段

**现有恢复机制可以支持，不需要直接改库或重置项目**：

直读`core_loop.rs:836-873`的`unblock_test_execution_for_retry`（**本轮未改动**，确认是pre-existing、此前已审核过的既有逻辑）：

1. 该函数会把当前pending round的`test_execution`/`build_execution`两个动作中处于`failed`状态的那个（`reset_failed_action_for_retry`的`WHERE status='failed'`条件保证另一个未失败的动作不受影响）重置回`planned`；
2. 若任务当前恰为`needs_user_action`，将其转回`queued`——这是`start_core_loop`接受的合法状态。

grep确认该函数**唯一**调用点在`commands.rs:649`的`confirm_task_test_command`命令内部（通过Tauri暴露给前端，非直接改库）。对照`run_dev_and_test_round`（`core_loop.rs:1758`起）的逻辑：任务重新进入核心循环后，开发动作因早已`Succeeded`会命中`ActionGate::AlreadySucceeded`分支，**不会重新调用开发AI**（已有"PASS"的开发结果被保留复用）；随后自查门禁检查（本轮未改，仍为`self_reported_complete`）正常通过；最终到达`run_tests_for_round`时，测试动作因刚被重置为`planned`而重新`Started`，会**用本轮已修复的解析逻辑**重新执行并解析`npm test`的输出。

**具体操作路径**（本轮/本次审核均未执行，仅为静态代码核验结论，供PM后续授权）：在新候选构建安装后，用户在"测试命令"UI区域对该任务**重新提交同一条已生效的测试命令**（即使内容不变），会触发`confirm_task_test_command`→`unblock_test_execution_for_retry`→任务转`queued`；用户再点击"开始任务"（`start_core_loop`，此时任务状态`queued`满足其状态前置条件）即可仅重新跑一次测试步骤，不触碰已通过的开发结果。

**局限说明**：这是现有UI路径里唯一暴露`unblock_test_execution_for_retry`的入口，语义上是"重新确认测试命令"而非专门的"重试"按钮；若该任务当时是因别的原因（如构建检测）stalled，同一按钮也会一并尝试重置构建动作（无害：另一个未失败的动作重置是安全空操作）。本机未针对真实RAV01任务实际执行上述路径——按任务边界要求，本轮及本次审核均未触碰`D:\NovaRAV01Project`或原任务数据库，此结论仅为代码静态核验，建议PM在授权新候选构建安装后再实际验证一次该路径确实按此处分析运作。

## 八、现场保护确认

本轮仅执行只读`git status`/`git log`/`git diff`/`git apply --reverse --check`、对四份交付文件及证据目录原始日志（包括原任务SQLite只读导出JSON、两份修复前panic日志、隔离npm输出）的只读读取、`cargo test --locked --lib`（完整套件及`rav01`定向子集）两条本地验证命令。未执行`git add`/`commit`/`push`/`reset`/`clean`/`stash`；未修改任何源码或测试；未调用真实AI；未操作`D:\NovaRAV01Project`或原任务数据库；未改动并行UI/UX文档；未安装、未打包、未发布。`git status`核对显示与审核开始前一致，仅本报告为新增。

完成审核，到此停止。交PM安排候选构建与后续定向复验节点。
