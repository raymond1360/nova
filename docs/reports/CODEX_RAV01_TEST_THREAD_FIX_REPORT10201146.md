# RAV01-title-retest-02 独立测试线程异常定位与最小修复

时间：2026-10-20T11:46:30+08:00（Asia/Shanghai）。本轮仅本地未提交开发，交 Claude Code 独立审核。

## 结论

**已在隔离夹具中确定性复现并修复测试结果解析的 UTF-8 越界 panic。** Node 24 的成功摘要含 `ℹ skipped 0`，旧计数解析器错误地按单字节跨过多字节字符 ℹ，导致 Rust 字符串切片 panic。错误发生于进程执行返回之后、结果计数解析阶段，不是 npm 无法启动，也没有依据归因于旧 Codex sandbox 文件占用。

修复字符边界与 Node 原生/TAP 计数格式，并把结果解析也纳入异常捕获，保存脱敏阶段/具体错误、真实进程退出码与已捕获输出；解析基础设施异常不会冒充测试通过或触发自动 AI 修复。相关回归及完整 Rust lib **688 项通过**。

原现场未保留完整 panic payload/backtrace，本轮不声称拿到了原进程堆栈或排除了所有其他现场因素。定位依据是：同五份源文件隔离副本的实际 Node 输出、原版本生产测试执行器的真实进程重放、两份确定性 panic 堆栈及修复后的通过结果。**原任务本轮没有重试，真实 RAV01 闭环未据此宣布通过。**

## 基线、规则与现场保护

D:\nova，feature/v0.1.0-mvp，HEAD `e6472f12fe65353efc07f14d3bf7d7d1d03ec327`，符合当前候选。适用规则检查未发现 AGENTS.md/CLAUDE.md。起始跟踪修改仅两份 product 历史文档；全部保留。未操作并行 UI/UX 文档文件，没有 reset/clean/stash、暂存、提交、推送或打包。

证据先保全至新目录：`D:\nova\audit\rav01-test-thread-fix-20261020-113337`。

对 RAV01-title-retest-02 定向读取 tasks、task_actions、task_state_events、timeline_events、command_evidence、test_evidence、build_evidence、self_checks、git_evidence、file_change_evidence；SQLite 用 mode=ro + PRAGMA query_only=ON，不修改任务状态。未复制整个用户库或读取认证。原项目只读保存 git diff、五文件哈希及状态；最终保持一致。

原始任务/命令输出可能含需求、路径和会话信息，仅 audit 本地保存，不提交公开仓库。evidence-sha256.json 保存证据大小与哈希，不在报告附原始会话正文。

## 原任务实际证据（北京时间）

| 项目 | 观察结果 |
| --- | --- |
| 开发调用 | 10:42:53 开始，10:46:39 结束，退出 0 |
| 自查 | valid / self_reported_complete，blocking_issues 为空；开发 AI 自报两项测试通过、构建成功，仅属其自查，不等同 Nova 独立验证 |
| Git diff | 仅 index.html：title 和 h1 各由 Nova Single AI BEFORE 改为 Nova Single AI PASS，共两行替换 |
| 独立验证开始 | 10:46:40.154 转 verifying；10:46:40.163 test_execution started |
| 动作异常收口 | 10:46:40.825 test_execution failed，error_summary 为测试执行提前结束或异常终止，未获得有效结果 |
| 任务停点 | 10:46:40.832 转 needs_user_action，原因是核心循环执行线程异常终止的通用错误 |
| 独立证据 | test_evidence、build_evidence 均空；command_evidence 仅开发调用 |

这说明上一候选的 guard 已将 started 收口，但具体错误和独立测试结果未保存。没有证据支持“原独立测试已通过”或“原构建已通过”。原测试项目五文件没有由本轮改写，也未在其中执行 npm test/build。

## 隔离复现与精确错误链

### A. 同源码副本的真实命令

将 index.html、acceptance.test.mjs、build.mjs、package.json、.gitignore 复制到 `D:\nova\audit\rav01-test-thread-fix-20261020-113337\isolated-fixture`，没有复制 .git、认证或数据库。使用 Node **v24.14.0**，独立空 npm user/global 配置和独立缓存，仅在该副本执行 npm test，不安装依赖。

实际退出 **0**，两项测试通过，stdout 包含：

```
ℹ tests 2
ℹ pass 2
ℹ fail 0
ℹ skipped 0
```

实际命令、目录、退出码及副本哈希在 isolated-execution.json，原始输出在 isolated-npm-test.stdout.log / .stderr.log。

### B. 修复前的红灯证据

先只增加回归用例，未先改生产解析逻辑：

1. rav01_node_native_summary_with_unicode_prefix_is_parsed 把实际捕获摘要送入原 parse_counts，测试退出 101，panic：`start byte index 156 is not a char boundary; it is inside 'ℹ'`。
2. rav01_real_npm_node_summary_in_isolated_fixture 在 Rust 测试创建独立临时 Node 项目，通过 **原生产 run_tests → process::execute_blocking** 真正运行 npm test，不使用假执行器。测试也退出 101，panic：`start byte index 146 is not a char boundary; it is inside 'ℹ'`。

两份 backtrace 均定位修复前 test_runner.rs:410 的 extract_count_before 切片；真实执行器堆栈进一步包含 parse_counts（旧 388 行）和 run_tests（旧 342 行）。不同字节位置来自前面测试名称/输出长度不同，触发字符与代码位置相同。

证据文件：parser-reproduction-before-fix.log、real-runner-reproduction-before-fix.log。它们保留失败结果，不把红灯测试写成通过。

### C. 根因与阶段区分

旧 extract_count_before 对 `ℹ skipped 0` 搜索 skipped，trim_end 后前缀末字符为三字节 ℹ；rfind 得到该字符的**字节起点**，代码却用 i+1 当下一字符边界，切进 ℹ 内部。原算法还只识别“数字在标签前”，与 Node 的 pass 2 / skipped 0 相反。

原 run_tests 的 catch_unwind 只包含 process::execute_blocking，后面的 parse_counts 不在捕获范围；于是进程即使已正常退出，解析 panic 仍穿出测试函数，TestActionGuard 仅记录通用异常，report_run_failure 又丢弃 panic payload。test_evidence 写入尚未发生，因此界面保持“尚未运行”。这条链与现场记录相符；它不是证据保存 SQL 自身失败的复现，也不是进程启动失败的复现。

## 最小修改范围

1. **test_runner.rs**：使用 char_indices + char.len_utf8 计算安全字节边界；增加 Node 原生/TAP 全行摘要识别，保留 Cargo/pytest/Jest 原有计数规则，无法识别仍为 None，不伪造零通过。异常边界扩展至结果解析；区分“测试进程启动/执行”与“测试结果解析”。解析异常保留真实退出码（可能为 0）、原始已脱敏捕获输出及具体原因，计数为未知、process_succeeded=false。
2. **core_loop.rs**：新增内部 infrastructure_error 判断，即使进程退出 0，解析异常也记录 failed test_evidence/action 和具体原因，转需要用户处理，不走自动 AI 修复。外层 caught panic 同样保存脱敏 payload，不再只剩通用文字。原 attempt/CAS、代际与完成门禁保留。
3. **security.rs**：共享 panic_diagnostic，支持 String / &str / 非文本 payload，先按既有规则脱敏，再按字符截断至 2000 字符；不安装全局 panic hook，不改变全局 Codex 或系统设置。
4. **wp2a_remediation_tests.rs**：为旧隔离模拟结果补内部字段默认值 None；增加解析异常实际退出 0 时的证据落库/无自动 AI 重试回归、外层异常脱敏回归。旧断言未降低。

infrastructure_error 仅为 Rust 内部结果结构字段，无数据库 schema、IPC DTO、依赖、锁文件、workflow 或产品配置修改。测试命令和错误将保存在独立测试证据中；原任务现场不补造记录，已安装应用也不会因工作区源码变化自动获得修复。

| 本轮文件 | SHA256（工作区原始字节） |
| --- | --- |
| `apps/desktop/src-tauri/src/core_loop.rs` | `18626745b817abb49523d1e8fb237724e716962f57283776145d307e467694b2` |
| `apps/desktop/src-tauri/src/core_loop/test_runner.rs` | `126148d933f130ed74961def5ff9b8cdef25ac20cc0360272323439efe87d0a7` |
| `apps/desktop/src-tauri/src/core_loop/wp2a_remediation_tests.rs` | `31a7275994cf7c1a2a47b12ed37466e49aa97e5b37a69dd1e6460e07e7aad1af` |
| `apps/desktop/src-tauri/src/security.rs` | `5b77a0108b88d98f3ca110f05da8b209186c56f5139e52451bc20f5a79127f23` |

可复核补丁：`D:\nova\audit\rav01-test-thread-fix-20261020-113337\review.diff`。source-manifest.json 为唯一未提交源码交付清单。报告保持未提交。

## 回归与构建检查

| 命令 | 实际结果 |
| --- | --- |
| 隔离副本 npm test | 退出 0，两项通过；原测试项目未运行 |
| cargo test --locked --lib rav01_node_native_summary_with_unicode_prefix_is_parsed -- --nocapture（修复前） | 退出 101，UTF-8 越界 panic，红灯证据已保存 |
| cargo test --locked --lib rav01_real_npm_node_summary_in_isolated_fixture -- --nocapture（修复前） | 退出 101，经真实生产执行器确定性复现同一解析 panic |
| cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib rav01 -- --nocapture（修复后） | **14 通过，0 失败**，退出 0 |
| cargo test --locked --lib（apps/desktop/src-tauri） | **688 通过，0 失败、0 ignored**，83.32 秒，退出 0 |
| cargo check --locked（apps/desktop/src-tauri） | 默认开发构建检查通过，退出 0；非安装包构建 |
| npm.cmd run build（apps/desktop） | vue-tsc --noEmit、Vite 均通过，退出 0 |
| git -c core.whitespace=cr-at-eol diff --check | 通过，仅单次命令配置，保留原换行，不改全局配置 |

新增七项回归覆盖：原生 Unicode 摘要、真实 npm/Node 隔离夹具、中文/TAP/未知计数、解析 panic 保留真实退出与脱敏阶段、退出 0 解析错误的 failed 证据和无 AI 重试、外层异常具体文本脱敏、长 Unicode/非文本 panic 的边界。既有启动失败/取消/超时/代际/attempt 与门禁测试随完整 lib 测试通过。

故意注入 panic 的测试日志含人工构造的假 token，用于证明持久化/展示诊断中已脱敏；它们不是用户真实凭据。未新增打印真实 panic payload 的生产日志，也未更改 Rust 进程全局 panic hook。

NOT RUN：新的真实 AI、原 RAV01 任务重试、真实测试项目的测试/构建、正式应用人工复验、安装包/CI。前端测试本轮未重跑（无前端源码/IPC 契约修改；类型检查和前端构建已通过）。隔离设施 feature 未变，无需为此重新启动其桌面验证程序。

## 尚未解决与审核边界

- 原任务异常的完整 panic 栈在旧候选中已被吞掉，不能事后恢复；本报告用可重复的同输入格式生产路径复现定位，不冒充原进程日志。
- 同源码副本两项通过和修复后生产执行器回归通过，只证明本轮解析缺陷关闭，不代表原任务已完成或整个 RAV01 闭环通过。原任务保留 needs_user_action，不改状态、不补假证据。
- 真实环境是否另有问题需未来经 PM 授权复验；本轮未触发旧 sandbox 初始化，不以旧 node_repl.exe 占用作为新问题解释，不改 ACL/防火墙/认证或降低保护。
- 必须先由 Claude 独立审核本轮四文件及复现日志，再由 PM 决定候选构建、安装与一次定向复验；本轮不提交、不推送、不打包，不自动重试。

## 现场保护与回退

最终复核：`{"head_unchanged": true, "branch_unchanged": true, "historical_product_diff_unchanged": true, "initial_status_entries_preserved": true, "staging_empty": true, "original_project_files_unchanged": true, "original_project_status_unchanged": true, "original_task_scoped_records_unchanged": true}`。

五份真实项目文件哈希和 Git 状态与起始一致（index.html 原有 PASS 改动保留）；任务关联的全部已保全记录复查一致；历史两份 product 修改及全部初始未跟踪资料保留。没有操作并行 UI/UX 文档，不整文件恢复，不改用户数据库或认证配置。

若需撤回本轮源码，仅在确认无后续编辑后反向应用本轮 review.diff 的四文件补丁；不回退 product、并行文档、测试项目或数据库，不删除原始日志。audit 中原始证据不得提交公开仓库。完成后停止，交 Claude 独立审核与 PM。
