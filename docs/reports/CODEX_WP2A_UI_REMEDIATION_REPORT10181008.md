# WP2A IA-01 / IA-02 / IA-03 前端整改交付

生成时间：2026-10-18 10:08:19 Asia/Shanghai。状态：开发与本地回归完成，待 Claude 独立审核；不是新版安装验收通过结论。

## 1. 基线与范围

- 项目：`D:\nova`；分支：`feature/v0.1.0-mvp`。
- 实际开始和结束 HEAD：`02be33aebefec6bf48e4d63739a8b1a10b3b0472`，与指定基线一致，未提交，暂存区为空。
- 依据：`docs/reports/CODEX_WP2A_MANUAL_INSTALL_ACCEPTANCE_REPORT10180104.md`。
- 已检查适用仓库规则；未发现适用的 AGENTS.md / CLAUDE.md。未切换分支，未执行 reset、clean、stash、整文件恢复或暂存操作。
- 仅改动下面三份前端业务文件，新增两份交互测试支持文件、一份 Vitest 配置及本报告。没有修改 Rust、后端契约、schema、依赖、锁文件、workflow 或正式 Tauri 配置。
- 没有读取真实用户库/凭据，没有调用 AI 或付费服务，没有启动宿主正式 Nova 程序。

## 2. 根因、修复与关闭证据

### IA-01：发送前置校验和输入保护

根因：App 原先在等待 store 前清空输入；缺模型错误在 store 的 try/catch 之前抛出，未写入可见 lastError。请求已接受后若刷新消息失败，又会表现为发送拒绝。

改动定位：`apps/desktop/src/App.vue:102`、`apps/desktop/src/stores/nova.ts:126`。

- 明确沿用返回约定：`Promise<string | undefined>`；消息 ID 表示 Rust IPC 已接受，undefined 表示忙碌而未提交，抛错表示拒绝且已写入 lastError。未修改后端协议。
- 缺模型时显示“请先选择模型后再发送消息”，不调用发送 IPC；包括空输入点击时。拒绝时保留输入原字节。
- 仅收到接受 ID 后清空原草稿；等待期间用户写入不同的新草稿则保留新内容。局部 sendPending 和 store.sending 同时防重入，Ctrl+Enter 也使用同一入口。
- 接受后列表刷新失败只报告“消息已发送，刷新消息列表失败”，仍返回接受 ID并消费原草稿；流的 sending 状态继续由既有完成/错误事件管理，避免诱发重复发送。
- 接受是 IPC 接受语义，不代表 AI 成功完成；未来流失败不会自动恢复已接受消息为未发送草稿。

本轮组件回归 7 项：无模型（空和非空草稿）可见提示/不发 IPC；延迟接受才清空并阻止快捷键重复；创建会话拒绝；发送 IPC 拒绝；已接受但刷新失败；等待期间新草稿保护。均通过。

### IA-02：测试与构建状态的文字和颜色

根因：原汇总行采用“不是失败就用 ok 绿色”，所以空证据“尚未运行”被标绿，未知状态也落入成功样式。

改动定位：`apps/desktop/src/components/CoreLoopView.vue:238`、`:610`。统一汇总状态标签和样式：

| 输入 | 展示 | 样式 |
| --- | --- | --- |
| 无记录、空值、not_run | 尚未运行 | hint，中性 |
| planned | 已计划 | hint，中性 |
| started / running / queued | 进行中 | hint，中性 |
| failed / cancelled / timed_out / insufficient_evidence / uncertain / rejected | 对应失败、取消、超时、证据不足等标签 | bad |
| passed | 已通过 | ok，绿色 |
| not_applicable | 不适用（依据已核验） | hint，中性 |
| 原始 no_tests_applicable / no_build_applicable | 不适用依据尚未核验 | bad |
| 其他值 | 未知状态（原值） | hint，中性 |

不适用依据来自既有后端 `src-tauri/src/core_loop/evidence_overview.rs` 的 normalize_test_evidence_status / normalize_build_evidence_status：按当前代际/轮次/attempt 验证确认或构建授权，再返回 not_applicable；无授权返回 insufficient_evidence。本轮未以缺少记录推断不适用，也未修改后端判断。

保留 currentTests/currentBuilds 的当前 generation + attempt 过滤、时间线历史标记、后端完工门禁和 acceptance_ready。未重构时间线映射。本轮 15 组状态对测试/构建双行检查文字及 class，另 1 项历史 generation/attempt 不冒充当前成功，均通过。

### IA-03：已有任务原需求只读查看

根因：原四字段藏在排除 draft 与终态的补建快照 details 内；独立新建表单为空，易被误认为原需求丢失。

改动定位：`apps/desktop/src/components/CoreLoopView.vue:335`、`:373`。

- 新建区明确标为“新建任务（独立表单）”；当前任务单独显示“当前任务原需求（只读）”。
- 任一已选任务（包含草稿和 completed / failed / cancelled）直接展示 goal、in_scope、not_in_scope、done_criteria；不要求生成 REQUIREMENT.md。
- 直接插值保存字段，沿用 white-space: pre-wrap；保留顺序、重复项、换行和内部空格，不做解析/列表规范化。null 显示（空），不把空字符串改写为别的需求。
- 展示区域没有输入框或操作按钮；不回填新建表单，不写入任务、不确认、不生成快照。
- 原补建入口只改名称为“补建已确认需求清单”，其状态 v-if、按钮事件及历史冻结/后端限制均保留。

本轮 6 项回归：通过实际任务按钮选择和切换草稿（新建未提交文本不变）；创建后四字段仍只读显示而新建表单清空（核对 create_task 实参）；restoreLastSession 重载显示；三种终态经任务按钮选中可读且无确认/快照按钮。对相关 IPC 使用只读命令白名单，创建场景仅另准 create_task，均通过。

## 3. 文件清单与唯一交付哈希

下列为工作区文件原始字节 SHA256（不是 Git 换行规范化后的 blob hash），唯一定位此未提交实现；不包含历史 product 修改。

| 文件（相对 D:\nova） | 类型 | 字节 | SHA256 |
| --- | --- | ---: | --- |
| `apps/desktop/src/App.vue` | modified | 14300 | `9053d6f2a210abf00d4db7d8e0927c8aa33445c84c746881375fca333039067a` |
| `apps/desktop/src/stores/nova.ts` | modified | 8389 | `a4108ee6409bb0f82ee432b7997a1109d01b8545d57d122fb403ff27ad2c27b4` |
| `apps/desktop/src/components/CoreLoopView.vue` | modified | 34591 | `ffe5b5270dc3053d7dad19c5337bbd379fb9629ac0f7348cd1dea31956ed5f5f` |
| `apps/desktop/src/testSupport/interactionRenderer.ts` | added | 3422 | `10b8d7f9769fc542c1706e652ab4bcc42a30920117890561fb76cf1a3fb30c82` |
| `apps/desktop/src/uiRemediation.interaction.test.ts` | added | 13933 | `abc987cd352b0fbfaa5a01d7287b631ea5d0ebd74b2190199e3f147426763efd` |
| `apps/desktop/vitest.config.ts` | added | 327 | `2433839bbb36e0e9ed93a808e2e01ceede6e3d1dec9d53f79c92d736c0b02c0a` |

新增 `vitest.config.ts` 继承原 Vite 配置，仅将 `*.interaction.test.ts` 以 web 模式编译真实 Vue 模板，避免 Node 测试默认 SSR 编译无法挂载。没有新增 DOM 依赖或修改 package.json/锁文件。测试支持用现有 Vue createRenderer 驱动实际组件事件、v-model 和响应更新，IPC 全部 mock。

## 4. 实际验证

工作目录均为 `D:\nova\apps\desktop`。

| 命令 / 检查 | 结果 |
| --- | --- |
| npm.cmd test -- src/uiRemediation.interaction.test.ts | PASS，29/29 |
| npm.cmd test | PASS，6 文件、138/138，含原 109 项与新增 29 项 |
| npm.cmd run build | PASS，vue-tsc --noEmit 类型检查及 Vite 生产前端构建均退出 0 |
| git -c core.whitespace=cr-at-eol diff --check -- 三份已修改源码 | PASS，退出 0 |
| git apply --reverse --check 本轮 patch | PASS，仅检查，未应用回滚 |
| Rust 全套、Tauri release / NSIS、GitHub CI | NOT RUN；无 Rust/契约改动，本轮仅前端验证，按授权不运行云端 CI |
| 新候选安装、原生鼠标键盘和视觉复验 | NOT RUN；须审核通过后另安排新候选与定向人工验收 |
| 真实 AI / 服务 / 用户库 | NOT RUN；本轮全部隔离 mock |

测试编译真实 SFC，但自定义宿主渲染器不是浏览器 DOM、原生输入或截图；class 断言结合既有样式证明选择了预期样式，不冒充像素/可访问性/安装验收。

调试测试设施期间出现 SSR 编译和宿主 getRootNode 缺失，已在测试配置/宿主修正；后续目标测试和完整测试无未处理异常。编译仍提示既有自检展示区 3 处 ul 在 p 内的 HTML 警告（源码约 536–552 行），这些未改动，不扩展为本轮整改；原样列为剩余风险。生产构建成功。

默认 git diff --check 把已在基线为 CRLF 的 CoreLoopView 新增行末 CR 判为尾随空白；git ls-files --eol 显示 i/crlf w/crlf。使用单次命令 cr-at-eol 检查通过，未改任何 Git 配置或规范化整文件。

## 5. 审核证据与保护检查

本轮本地证据目录：`D:\nova\audit\wp2a-ui-remediation-20261018-0959`。

- `ui-remediation.patch`：只含上述六个源码/测试/配置文件的可复核 diff，包括未跟踪新增文件；不含 product 或其他历史资料。
- `source-sha256.json`：上述六文件原始字节清单。
- `frontend-tests.log`：最终完整测试输出；`frontend-build.log`：最终类型检查和构建输出。
- `preservation-verification.json`：HEAD/分支、暂存区、product 前后哈希、历史未跟踪 status 条目存在性、CRLF 感知 diff 检查及反向 patch dry-run。
- `git-status-after.txt`：本轮报告生成前的完整 status；`before/`：本轮改动前三份源码字节留存，仅辅助审核，不用于整文件恢复。

两份 product 文件前后 SHA256 相同：NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md 为 `6f0eae6165d01baf6ed272b42b960f141316c491901ecedde3d0bbba07ff265b`；NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md 为 `d37f768ece2976376ca573bf7a85c569128a0811b4f85ec3b8a35c310d31f966`。既有未跟踪 status 条目无缺失；只验证存在性，不宣称已对全部历史 audit 原始数据做前后哈希复核。

除三份本轮源码及这两份历史 product 文件外，无其他已跟踪文件变化；依赖、锁文件、Rust、workflow 和正式配置与 HEAD 无 diff。已有未跟踪文档和 audit 保留原位；未整理、未暂存。没有将数据库、截图或原始 audit 材料纳入公开提交。

Claude 可从源码定位、此六文件哈希、独立 patch 与完整测试日志审核。复核命令可在仓库根执行 `git diff -- apps/desktop/src/App.vue apps/desktop/src/stores/nova.ts apps/desktop/src/components/CoreLoopView.vue`；新增三文件须单独查看或读取上述 patch，普通 git diff 不显示未跟踪文件。

## 6. 回滚与后续

本轮未提交，故没有新提交可 revert。若 PM 决定撤回：先核对六文件哈希并保存后来新增改动；再次执行 `git apply --reverse --check audit/wp2a-ui-remediation-20261018-0959/ui-remediation.patch`，经审查后只反向应用该 patch，撤销三份源码的本轮 hunks 并移除三份本轮新增测试/配置文件。不要整文件恢复源码，不触碰两份 product、历史未跟踪文档或 audit。若哈希不一致或 check 失败，停止自动回滚并逐 hunk 审核；此为回滚建议，本轮未执行。报告和本轮审计证据建议保留。

三项问题在本地交互回归层面已有关闭证据，独立审核与人工复验仍待完成。保持既有需求冻结、终态限制、attempt 归属及后端门禁。本轮不处理凭据机制、WebView2 联网路径、系统弹窗或卸载用户数据。

完成后停止，交 Claude 独立审核；审核通过后再由 PM 安排新候选构建及仅针对这三项改动的人工复验。未提交、未 push、未运行 GitHub CI、未发布。
