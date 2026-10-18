# WP2A IA-01/IA-02/IA-03 前端整改独立审核（Claude Code）

生成时间：2026-10-18 10:32 Asia/Shanghai。项目：D:\nova。审核人：Claude Code；开发/整改：Codex；产品管理：ChatGPT。

## 唯一结论：PASS（限本轮IA-01～03前端实现；不代表新版安装验收或发布批准）

基线完整HEAD：`02be33aebefec6bf48e4d63739a8b1a10b3b0472`（未提交，与预期一致）。IA-01、IA-02、IA-03三项问题经本轮源码直读与独立命令复验，**均已关闭**，未发现新回归，未发现测试设施掩盖真实问题。138项前端测试（109原有+29新增）与生产构建均独立复跑通过。

## 一、基线与交付文件核验

- `git log -1` 确认HEAD为`02be33aebefec6bf48e4d63739a8b1a10b3b0472`，分支`feature/v0.1.0-mvp`，与指定基线一致；本轮未提交，暂存区为空。
- 本轮未提交，`git diff`不显示新增未跟踪文件（`testSupport/interactionRenderer.ts`、`uiRemediation.interaction.test.ts`、`vitest.config.ts`），已单独用`git status`/直接读取确认其存在。
- 六份交付文件**逐一独立重算SHA256，与报告声明值完全一致**：

| 文件 | 独立重算SHA256 | 与报告比对 |
|---|---|---|
| `apps/desktop/src/App.vue` | `9053d6f2a210abf00d4db7d8e0927c8aa33445c84c746881375fca333039067a` | 一致 |
| `apps/desktop/src/stores/nova.ts` | `a4108ee6409bb0f82ee432b7997a1109d01b8545d57d122fb403ff27ad2c27b4` | 一致 |
| `apps/desktop/src/components/CoreLoopView.vue` | `ffe5b5270dc3053d7dad19c5337bbd379fb9629ac0f7348cd1dea31956ed5f5f` | 一致 |
| `apps/desktop/src/testSupport/interactionRenderer.ts` | `10b8d7f9769fc542c1706e652ab4bcc42a30920117890561fb76cf1a3fb30c82` | 一致 |
| `apps/desktop/src/uiRemediation.interaction.test.ts` | `abc987cd352b0fbfaa5a01d7287b631ea5d0ebd74b2190199e3f147426763efd` | 一致 |
| `apps/desktop/vitest.config.ts` | `2433839bbb36e0e9ed93a808e2e01ceede6e3d1dec9d53f79c92d736c0b02c0a` | 一致 |

- `git apply --reverse --check audit/wp2a-ui-remediation-20261018-0959/ui-remediation.patch` 本机独立重跑：退出码0，补丁边界与实际diff一致（仅检查，未应用）。
- `git diff --stat -- apps/desktop/src-tauri/ .github/` 与对`package.json`/`package-lock.json`的`git status`核对：**均无变化**，确认本轮未触碰Rust、后端契约、workflow、依赖或锁文件，与报告边界声明一致。
- 两份product历史文档SHA256分别为`6f0eae6...`/`d37f768...`，与报告记录值一致，未被本轮触及。

## 二、IA-01～03逐项核验（源码直读，非转述报告）

### IA-01：发送前置校验与输入保护 —— **已关闭**

直读`App.vue:102-120`的`send()`与`stores/nova.ts:126-183`的`sendMessage()`：

| 检查项 | 核验结果 |
|---|---|
| 缺模型时提示可见、不调用发送IPC | `sendMessage`在任何`invoke()`之前先检查`!this.selectedModelId`，写入`lastError='请先选择模型后再发送消息'`并`throw`——**IPC从未被调用**。`App.vue`的`send()`在`!text && store.selectedModelId`才早退，缺模型时无论草稿是否为空都会继续调用`sendMessage`从而命中该校验，覆盖空/非空两种草稿 |
| 输入保留 | 异常被`App.vue`的`catch {}`吸收，清空逻辑`if (acceptedMessageId && ...)`从未执行到，草稿原样保留 |
| `Promise<string\|undefined>`语义与调用者一致 | 签名显式标注；`this.sending`为真时返回`undefined`（忙碌，早于任何`await`的同步检查）；接受后返回`assistantMessageId`（string）；拒绝则`throw`。grep确认生产代码中`sendMessage`唯一调用者就是`App.vue`的`send()`，无其他调用点需要对齐 |
| 仅接受后消费原草稿；等待期间新草稿不被误清空 | `if (acceptedMessageId && prompt.value === draft) prompt.value = ''`——用局部变量`draft`快照旧值，仅当当前内容仍等于发起请求时的草稿才清空，期间键入的新文本因`prompt.value !== draft`而保留 |
| 鼠标和快捷键不重复提交 | `App.vue`：按钮`:disabled="sendPending"`与`@keydown.ctrl.enter.prevent="send"`指向同一`send()`，其内部`if (sendPending.value \|\| store.sending) return`在首个`await`前同步执行；`nova.ts`的`sendMessage`自身也有独立的`if (this.sending) return`首行同步守卫，构成双层防重入 |
| 创建会话失败、发送失败均有反馈 | `createConversation()`与`send_chat_message`的IPC调用处于同一`try`，同一`catch`块统一写`lastError`、复位`sending=false`、`throw` |
| 接受后刷新失败不误报未发送、不诱发重复发送 | `assistantMessageId`赋值（真正代表Rust已接受）后，`loadMessages`单独包在**不复位`sending`**的`try/catch`中，失败只追加`lastError='消息已发送，刷新消息列表失败：...'`，仍`return assistantMessageId`——草稿按"已接受"清空，`sending`继续由`finishSending()`接管，不会被这里误判为"未发送"而允许用户立刻再发一次 |
| `sending`/`sendPending`不卡死 | `sendPending`在`App.vue`的`finally`无条件复位；`sending`在失败分支的`catch`中复位，在成功分支延后到`chat://done`/`chat://error`监听器调用`finishSending()`时复位——这两个监听器的注册代码（`App.vue:45-60`）本轮未改动，沿用既有契约 |
| 不把IPC接受等同AI完成 | 返回值只是接受ID，真正完成由异步事件`finishSending()`处理，`sendMessage`内部不写任何"已完成"状态 |

### IA-02：测试/构建状态文字与样式 —— **已关闭**

直读`CoreLoopView.vue:238-253`的`evidenceStatus()`及`:610-611`的实际渲染点：

- 逐一核对报告给出的8类映射（无记录/空/not_run→尚未运行/hint；planned→已计划/hint；started/running/queued→进行中/hint；6个`badStatuses`→对应标签/bad；passed→已通过/ok；not_applicable→不适用（依据已核验）/hint；原始`no_tests_applicable`/`no_build_applicable`→不适用依据尚未核验/**bad**；其他→未知状态（原值）/hint）**与源码逐条一致**，未发现遗漏分支或与报告描述不符之处。
- `evidenceStatus()`只有`currentTestStatus`/`currentBuildStatus`两处computed使用，渲染点`:610-611`也只有这一对，未发现另一条平行/残留的旧判定逻辑（旧版`badStatuses.has(...) ? 'bad' : 'ok'`已通过`git diff`确认被整体替换，不是被新逻辑遮蔽共存）。
- CSS确认`.hint`（#999灰）、`.ok`（#4caf50绿）、`.bad`（#d9534f红）三者样式独立、非别名，不是只改文字不改色。
- `currentTests`/`currentBuilds`两个computed仍用`is_current_generation && is_current_attempt`双重过滤（`:232-237`，本轮未改动该过滤逻辑本身），`not_applicable`判定依据直读确认来自后端`evidence_overview.rs`既有的`normalize_test_evidence_status`/`normalize_build_evidence_status`（本轮未修改后端），未发现前端自行从空列表或原始`no_tests_applicable`/`no_build_applicable`推断"已核验不适用"。

### IA-03：已有任务原需求只读查看 —— **已关闭**

直读`CoreLoopView.vue:334-404`：

- 新读点`:373-379`的`<div class="original-requirements">`位于`<section v-if="store.currentTask">`内，**无任何state过滤**——覆盖draft与全部终态（completed/failed/cancelled）及中间状态，直接解决原缺陷"查看入口排除draft/completed/failed/cancelled"。
- 四字段用`{{ store.currentTask.xxx ?? '（空）' }}`直接插值（`??`而非会误判空字符串的`||`），配合`.requirement-text { white-space: pre-wrap; }`——Vue插值不做trim/去重/重排，`pre-wrap`只影响渲染可见性，故原文空格、换行、重复行、顺序均逐字保留。
- "2. 新建任务（独立表单）"标题已显式标注"独立表单"；其五个输入ref（`taskTitle`/`taskGoal`/`taskInScope`/`taskNotInScope`/`taskDoneCriteria`）只在`handleCreateTask`里读取和清空（`:89-102`），从未被`selectTask`/`currentTask`相关逻辑写入，结构上与已选任务数据互不污染。
- 只读区域没有input/textarea/button子节点（静态可确认：该`<div>`内只有`<p><strong>...</strong><span>...</span></p>`结构），不触发任何写入/确认/快照操作。
- 原"补建已确认需求清单"`<details>`（`:380-387`）的`v-if="!['draft','completed','failed','cancelled'].includes(...)"`门槛与内部`generateRequirementSnapshot`调用**逐字未改**，只是`<summary>`文案从"查看原需求并确认沿用"改为"补建已确认需求清单"——准确反映该入口现在是"补建操作"而非"查看入口"，历史冻结/终态限制未被放宽。

## 三、测试设施核验

- `vitest.config.ts`：`mergeConfig(viteConfig, {test: {testTransformMode: {web: ['**/*.interaction.test.ts']}}})`——只继承基础Vite配置并新增一个**只对`*.interaction.test.ts`文件名生效**的编译模式覆盖，其余5个既有测试文件不受影响；grep确认全部测试文件均无`.skip`/`.only`/`xit`/`xdescribe`/`it.todo`，不存在"为凑数而隐藏失败"的痕迹。
- `testSupport/interactionRenderer.ts`：基于Vue**真实**`createRenderer`低阶API（与`@vue/runtime-dom`同源机制）构建宿主，`click()`直接调用编译后组件挂到虚拟节点上的**真实**`onClick`处理函数，`input()`通过`EventTarget`派发真实`input`事件驱动Vue**真实**v-model指令更新响应式ref——不是手写的"模拟点击"桩代码；文件顶部注释明确自我限定"不是浏览器、像素、原生输入或Tauri运行时测试"，未虚报能力范围。
- 独立计数`uiRemediation.interaction.test.ts`的`it`/`it.each`参数化用例：IA-01共7项（2+1+2+1+1）、IA-02共16项（15+1，报告称"15组+另1项"）、IA-03共6项（1+1+1+3），**合计29项，与报告声明一致**，非凭数字转述。
- 其中IA-01对Ctrl+Enter的测试（`uiRemediation.interaction.test.ts:120`）直接调用`editor.props.onKeydown({key:'Enter',ctrlKey:true,...})`——这是Vue编译`@keydown.ctrl.enter.prevent`模板指令后生成的**真实**事件处理器（内含`withKeys`/`withModifiers`的按键匹配逻辑），不是绕过模板编译直接调用`send()`，确认该用例真实验证了快捷键路径，不仅是鼠标路径。
- 既有HTML嵌套警告（`<ul>`在`<p>`内，源码536-552行，`latestSelfCheckCheckResults`/`UnfinishedItems`/`BlockingIssues`三处）：`git diff`确认这三处**完全不在本轮diff范围内**，属于此前已存在、与本轮三项整改无关的遗留问题，报告"原样列为剩余风险、不扩大整改"的处理方式恰当。

## 四、独立验证命令与实际结果

工作目录：`D:\nova\apps\desktop`。

| 命令 | 实际结果 |
|---|---|
| `npm.cmd test` | **6个文件，138/138 passed**（109原有+29新增），耗时1.69s；日志中可见既有ul-in-p警告原样存在，与预期一致 |
| `npm.cmd run build` | `vue-tsc --noEmit` 类型检查通过，`vite build`生成37模块，产物正常，无报错 |
| `git -c core.whitespace=cr-at-eol diff --check` 三份已改源码 | 退出0（仅CRLF规范化提示，非错误） |
| `git apply --reverse --check ui-remediation.patch` | 退出0 |
| grep检查`.skip`/`.only`/`xit`/`xdescribe`/`it.todo` | 全部测试文件均未命中 |
| `git diff --stat` 后端/workflow/依赖/锁文件 | 空，确认未改动 |

未按要求重跑Rust全套测试（本轮无Rust改动，任务书明确"无Rust改动不要求无理由重跑全部后端测试"，故不重跑）；未调用真实AI/供应商服务/真实凭据或用户库（测试与构建均为本地mock/静态编译，未发起任何网络请求）。

## 五、结论与后续

**PASS**，限本轮IA-01、IA-02、IA-03三项前端实现与配套测试设施。三项原始问题（无模型发送无反馈且提前清空输入、未运行状态误标绿色、草稿需求无只读查看入口）经源码直读确认根因定位准确、修复路径对应问题本身、未引入新回归、未放宽既有后端门禁/冻结/终态限制。138项测试与生产构建独立复跑通过。

**是否可进入提交、新候选GitHub构建与三项定向人工复验**：具备条件。建议顺序：PM确认本轮PASS → 授权提交（本轮三文件diff+测试+配置，边界与`CODEX_WP2A_UI_REMEDIATION_REPORT10181008.md`第6节回滚说明一致）→ 基于新提交触发一次新的GitHub Windows CI（复用已审核的`.github/workflows/windows-ci.yml`，预期无需改动workflow本身，因本轮只改前端）→ 产出新候选NSIS安装包 → 仅针对IA-01/02/03这三项在隔离环境中做定向人工复验（无模型发送提示、测试/构建状态颜色、草稿需求只读查看），不要求重做此前已通过的安装/原生交互/卸载等基础验收项。

本轮PASS**不代表**新版安装验收已完成或发布已批准——原生点击、视觉截图、新候选包的安装/启动仍需在上述人工复验阶段单独完成。

## 六、现场保护确认

本轮仅执行只读`git status`/`git log`/`git diff --stat`/`git apply --reverse --check`、对六份交付文件及测试/配置源码的只读读取、`npm.cmd test`与`npm.cmd run build`两条独立验证命令。未执行`git add`/`commit`/`push`/`reset`/`clean`/`stash`；未修改任何源码、测试断言或配置；未重新触发GitHub Actions；未安装或启动宿主正式Nova程序；未操作真实数据库或凭据。`git status`核对显示与本轮开始前一致，仅本报告为新增。

完成审核，到此停止。交PM决定提交与下一阶段安排。
