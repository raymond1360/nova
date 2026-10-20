# RAV01 仅重新验证现有修改：开发交付

时间：2026-10-21T07:08:28.690554+08:00（Asia/Shanghai）。状态：本地实现和验证完成，未提交，等待 Claude Code 独立审核。不是审核 PASS 或真实 RAV01 验收结论。

## 基线与原因

D:\nova；feature/v0.1.0-mvp；HEAD `9e1de55eddb2f991df5d5614675a14d2693d34de`，与指定基线一致。已读取候选交付报告、CODEX_RAV01_TEST_THREAD_FIX_REPORT10201146.md、CLAUDE_RAV01_TEST_THREAD_FIX_REVIEW10201513.md。根目录/祖先及仓库适用 AGENTS.md、CLAUDE.md 检查未发现额外规则。

已确认的两个缺口：已有 test_command_source 时，原前端仅显示命令，重新确认入口被 v-else 隐藏；普通 run_core_loop 对真实测试/构建失败的 NeedsFix 会进入 SafeFixing 并调用一次开发 AI。因此不能用普通恢复路径承诺“仅验证”。本轮不修改测试解析逻辑，也不修改普通开发流程。

## 实现范围与后端约束

新增 core_loop/verify_only.rs，提供 eligibility 和 run；此路径没有 AI adapter 参数、构造、CLI readiness probe、开发/审核调用，也不进入 run_core_loop 或 run_single_ai_round。调用链仅为：后端核验/Action Ledger → Git 证据 → 独立测试 → 独立构建 → 原有成功收口/文档。失败和异常分支均不会转入自动返修。

恢复入口只对后端判定符合条件的任务显示，展示原测试命令及“不调用开发 AI、审核 AI 或自动返修；失败停止、通过仍需用户验收”的说明；不要求修改或重新确认命令。

后端检查：

- project/task 归属、needs_user_action、非历史冻结；轮次由 current_pending_round 推导，代际由任务读取。
- 当前轮次开发 action 必须 succeeded；自查必须属于该开发 attempt，valid / self_reported_complete，未完成和阻塞项为空。
- 已确认需求快照非空，自查 coverage 必须逐项 covered、无重复/额外/缺项。
- 当前代际/轮次至少一项测试或构建 action 失败；started、uncertain、cancelled 不自动确认或重跑；测试命令非空。
- 前端带回所展示的代际、轮次、开发 attempt 和命令；命令入口及执行线程再次与真实后端比较，不采信前端条件。执行各阶段还复查代际、轮次、开发 action/attempt、命令及 verifying 状态。

独立操作使用 verification_only Action Ledger，key 绑定 task/generation/round/dev-attempt。只有失败动作可显式恢复，沿用 reset_failed_action_for_retry 的 attempt 递增，以及 start/finish CAS；并用与普通核心循环共用的任务运行槽拒绝同应用并发。前端有在途禁用和重复请求保护，异步资格响应按 query token 与项目/任务/代际/状态丢弃过期结果。

测试和构建复用既有执行器、安全允许列表、取消/超时、脱敏证据与 TestActionGuard。仅重置 failed 动作；已成功动作仍须有当前开发 attempt 的有效证据才复用，不重复运行不必要步骤。历史 evidence 不删除，开发 action 和项目源文件不改写。

断言失败：保存真实失败证据，转 needs_user_action，并明确记录“仅验证未通过、不会调用 AI 或自动返修”；测试失败不继续构建。基础设施异常：沿用已审核的详细脱敏异常及失败证据路径，停止处理。成功：复用 finish_success_path，进入 awaiting_user_acceptance；没有写用户接受记录，没有自动完成任务，也没有放宽原完工门禁。普通开发流程的一次自动返修规则未改。

## 八文件交付及原始字节 SHA256

| 文件 | SHA256 |
| --- | --- |
| `apps/desktop/src-tauri/src/commands.rs` | `becde28959525cd61f13255e8f19f569e73f26bddc14a68c7b2d2eab3acfbd35` |
| `apps/desktop/src-tauri/src/core_loop.rs` | `2ed7be5d35d012dec258885555839f510dac8acbb8d4b54a57f66a4dc7dbff3a` |
| `apps/desktop/src-tauri/src/core_loop/verify_only.rs` | `75d9422656a3b24fa8dcae428fe863ed8c6cb6e3a84b47fec596fcc5a8c4bd3c` |
| `apps/desktop/src-tauri/src/core_loop/wp2a_remediation_tests.rs` | `e037b6d00bb10d69a6f36f7e5eaf424a1b0b17fac73474cbf84cb81fc75ba1e8` |
| `apps/desktop/src-tauri/src/lib.rs` | `0db43cad68cd89103419f39871329db72232d89d277a4711d61e0cc543a8bd8d` |
| `apps/desktop/src/components/CoreLoopView.vue` | `b8635274df5ccd69add8516223272d37349b4ad4b144957f56ca8c74b5c1108a` |
| `apps/desktop/src/stores/coreLoop.ts` | `1adff0d76a0030ad807285aefb5686a366a47f00e82231fbe64a6a0a6c8130c3` |
| `apps/desktop/src/uiRemediation.interaction.test.ts` | `39396b98bda5c7dcbd19f0b7d7531d6e2f400048589aedf38c90b3c2fbfa8c01` |

commands.rs：两个新 Tauri 命令、共享运行槽、独立数据库连接及完成/错误通知；lib.rs：注册命令；core_loop.rs：仅注册子模块；verify_only.rs：独立零 AI 恢复逻辑。wp2a_remediation_tests.rs：六个后端回归；CoreLoopView.vue / coreLoop.ts：入口和状态；uiRemediation.interaction.test.ts：三项交互回归及只读 mock 路由补齐。无 schema、依赖、锁文件、workflow、版本或正式配置改动。

完整可反查补丁：`D:\nova\audit\rav01-verify-only-20261021-015036\review.diff`（包含新增模块）；`git apply --reverse --check` 退出0。source-manifest.json 与 review-files 中八文件副本定位本轮唯一未提交交付。没有使用 git add。

## 实际验证结果

| 命令/检查 | 结果 |
| --- | --- |
| cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib rav01_verify_only -- --nocapture | 六项通过，退出0；最终完整测试也重新覆盖这些用例 |
| npm.cmd --prefix apps/desktop test -- --run | 6 文件、145 passed，退出0 |
| npm.cmd --prefix apps/desktop run build | vue-tsc --noEmit 与 Vite 通过，退出0 |
| cargo build --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --features nova-test-helper --bin nova_test_helper | 测试 helper 预编译通过，退出0；未构建安装包 |
| cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib app_paths::tests::accepts_ordinary_uuid_like_components --target-dir %TEMP%/nova-self-referential-cargo-test-cache | 既有嵌套测试目录预热，1 passed，退出0 |
| cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib（最终复核） | **694 passed，0 failed/ignored/filtered，84.75秒，退出0** |
| git -c core.whitespace=cr-at-eol diff --check | 退出0；无全局 Git 配置修改 |

六项后端回归涵盖：成功后仍需用户验收且原开发记录逐字段不变；测试断言失败与退出0解析基础设施异常均写 failed、停止且不构建；陈旧代际/轮次/attempt、自查缺失/blocked/unfinished/coverage不足、冻结、开发失败、终态和 uncertain 条件拒绝；已有在途操作拒绝；两独立数据库连接竞争只运行一次；真实隔离 npm build 的成功及失败。验证路径不接收任何 AI adapter，测试同时核对新增开发/审核动作数为零、原开发 attempt 不变、没有 SafeFixing。没有调用真实 AI 来验证零调用承诺。

三项前端回归：已确认命令仍有入口、显示沿用命令、重复点击只发一次专用 IPC；跨任务迟到资格响应不污染界面；错误可见且不回落 start_core_loop、不触发 readiness 或确认命令 IPC。

### 保留的失败与复核边界

初次编译发现使用了不可见的内部归属校验方法，改为在新模块显式比较任务 project_id；首轮定向测试的冻结夹具值不满足既有数据库枚举，修正为既有 unknown_insufficient。没有修改 schema 或放宽断言。

首次完整 lib 运行退出101：681 passed、13 failed，记录总用时18282.57秒。新增真实隔离构建观察到 timed_out 而非预期 failed；嵌套 Cargo 编译超时；既有模拟 CLI 一项失败后伴随多项共享锁 PoisonError，以及既有进程时序测试失败。日志出现约9890秒等待，不能据此断言系统休眠或具体外部根因；系统事件查询没有取得可确认原因的记录。

保留 full-rust-lib.log，未覆盖或隐瞒失败。按已有 CI 的 helper/嵌套目录预热安排后，以**相同断言、相同超时、相同完整 lib 命令和默认并行度**复核，最终694项通过（full-rust-lib-recheck.log）。未修改不相关测试、未跳过失败项、未用筛选结果代替完整测试。前端测试有既有 p 包含 ul 的 Vue 警告，helper 编译有 linker_messages 警告；本轮未顺手进行无关 UI 重构。

## 证据、现场保护与未验证项

证据目录：`D:\nova\audit\rav01-verify-only-20261021-015036`。包括测试/构建日志、初始及最终 Git 清单、product-before.patch、完整补丁、八文件副本/哈希及 preservation.json。测试只使用自建临时数据库、mock 和独立 Node 夹具；真实 npm build 用例不安装依赖、不联网调用 AI。测试夹具允许写测试数据库，不涉及用户真实库。

HEAD/分支未变，暂存区为空；两份历史 product diff 与起始逐字节一致，全部初始 Git 状态项保留，未触碰并行 UI/UX 文档。未访问、运行或修改 D:\NovaRAV01Project，未读取/修改原任务数据库、Codex 配置或认证；未结束用户进程、修改 ACL/防火墙。没有 reset、clean、stash、整文件恢复、提交、推送、CI、安装包构建、安装或发布。

NOT RUN：真实 RAV01-title-retest-02 的资格查询/恢复、正式应用原生点击复验、真实项目测试/构建、任何真实 AI、云端 CI、新候选安装。代码和隔离回归通过不等于真实 RAV01 已闭环。

## Claude 独立审核重点与回退

请审核八文件哈希与补丁，重点看资格/代际/attempt 绑定、所有分支零 AI 调用、独立操作和测试/构建动作的 CAS、并发拒绝、历史证据复用的 fail-closed 检查、成功仅到等待验收，以及前端资格过期保护/错误可见性。先完成独立审核，再由 PM 安排候选构建、备份安装和单次仅验证复验；本轮不执行后续步骤。

如需撤回，只在核对无后续编辑后对本轮 review.diff 反向应用；新增模块仅在确认仍为本轮字节且无其他引用时撤回。不整文件恢复、不触碰 product/UIUX/数据库/真实测试项目，不清除历史证据。本轮没有数据库迁移或用户状态更改需要恢复。
