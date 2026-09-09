# Nova v0.1.0-beta.1 阶段0独立审核报告（Claude Code）

审核人：Claude Code（独立只读审核）
生成时间：2026-09-09
仓库：`D:\nova`
审核对象：Codex 产出的阶段0只读基线审计与范围冻结（`docs/release-v0.1.0/00_SCOPE09091050.md`、`01_BASELINE_AUDIT09091050.md`、`02_GAP_MATRIX09091050.md`、`03_IMPLEMENTATION_MAP09091050.md`）
工作方式：全程只读。本报告未修改任何应用代码、配置、测试或既有审计文档；仅新增本文件。

---

## 一、审核范围

按用户任务书要求，对 Codex 阶段0交付物进行独立核对，覆盖：Git/版本基线真实性、技术基线真实性、产品差距矩阵完整性、历史 Claude 审核问题闭环情况、阶段划分是否基于真实代码、以及是否遗漏/越界。审核过程使用直接 `git`/文件核对与3个并行只读子任务分别核实代码行级证据、历史问题闭环、产品P0覆盖，均为独立复核，不采信 Codex 自述结论。

## 二、读取的文件

产品文档：
- `D:\nova\product\NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`
- `D:\nova\product\NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md`（全文直读）
- `D:\nova\docs\product\NOVA_V1_PRODUCT_DESIGN_PLAN.md`（旧文档，供对照）

阶段0审计文档（全文）：
- `docs/release-v0.1.0/00_SCOPE09091050.md`
- `docs/release-v0.1.0/01_BASELINE_AUDIT09091050.md`
- `docs/release-v0.1.0/02_GAP_MATRIX09091050.md`
- `docs/release-v0.1.0/03_IMPLEMENTATION_MAP09091050.md`

配置/规则类：
- 根 `package.json`、`apps/desktop/package.json`、`apps/desktop/src-tauri/Cargo.toml`、`apps/desktop/src-tauri/tauri.conf.json`、`apps/desktop/src-tauri/capabilities/default.json`、`.gitignore`（master 与 HEAD 两版）
- 全仓搜索 `AGENTS.md`/`CLAUDE.md`/`README.md`/`.github`（含 `.cloude` 内部），确认均不存在于项目实际规则位置
- `test-results/build-info.txt`、`test-results/verify-build-npm-test.log`、`test-results/verify-build-cargo-test.log`

历史审核报告（未纳入 Git 版本控制，直接读取磁盘）：
- `audit/CLAUDE_REVIEW_NOVA_TECHNICAL_MVP.md`
- `audit/CLAUDE_REVIEW_NOVA_TECHNICAL_MVP_FINAL.md`

代码抽查（直接读取 + 子任务独立复核）：
- `apps/desktop/src/App.vue`（全文）
- `apps/desktop/src-tauri/src/api.rs`、`commands.rs`、`db.rs`、`security.rs`、`validation.rs`、`credential.rs`、`lib.rs`
- `apps/desktop/src/stores/nova.ts`、`nova.test.ts`、`security.ts`
- 全仓 `grep`：`localStorage`/`sessionStorage`、`codex`/`claude` 进程调用痕迹

Git 元数据：`git status --porcelain=v2 --untracked-files=all`、`git log --all --oneline --graph`、`git show --stat`、`git diff master...feature/nova-technical-mvp --stat`、`git ls-tree master`。

## 三、Git与版本基线核对

独立执行结果如下，与 `01_BASELINE_AUDIT09091050.md` 声称的数值逐项比对：

| 项目 | Codex 审计声称 | 独立核实结果 | 结论 |
|---|---|---|---|
| 当前分支 | `feature/nova-technical-mvp` | 相同 | 一致 |
| HEAD SHA | `e1e967548621b78c2b9a78e2e08b43159c3040be` | 相同 | 一致 |
| 最近5次提交 | `e1e9675/465d3c8/e0de353/3e1d2dd/e0f1a7b` | 相同（`git log --oneline -20` 全量核对） | 一致 |
| 分支列表 | `feature/nova-technical-mvp`、`master`，无 `develop` | 相同，无第三分支，无远程分支 | 一致 |
| 远程仓库 | 无 | `git remote -v` 输出为空，确认无远程、无备份路径 | 一致，且应登记为风险（见 Minor m6） |
| 标签 | 无 | `git tag -l` 输出为空 | 一致 |
| 工作区 | 无被跟踪文件被修改，241个未跟踪文件 | `git status --porcelain=v2 --untracked-files=all` 复核：**无任何已跟踪文件的修改（无 M/A/D 记录）**，未跟踪文件清单与审计所列示例逐一吻合（`.cloude/`、根目录重复的 V3 spec、两份历史 Claude 审核报告、`docs/chatgpt.txt`、`docs/product/`、`docs/release-v0.1.0/` 本身、`nova-ui-design.html`、`product/`） | 一致，且属实——**阶段0四份审计文件本身也在未跟踪清单中，尚未提交** |
| `master` 分支内容 | 未展开说明 | `git ls-tree master` 显示仅含 4 个 SDKMAX 审计文件 + `.gitignore`，与 `feature/nova-technical-mvp` 之间存在 44 个文件、约1.2万行差异（Tauri/Vue/Rust 全部应用代码仅存在于 feature 分支） | 补充事实：`master` 事实上是一个远早于 Nova V3 应用代码的孤立起点，审计未明确点出这一点，但不影响其"当前分支/HEAD"判断的准确性 |
| `test-results/build-info.txt` 中记录的 commit/branch | 与当前 HEAD 应一致 | 一致（commit `e1e9675`，branch `feature/nova-technical-mvp`），但该文件生成于 2026-08-31，早于本次审计日期（2026-09-09），其中记录的 `git status` 只有2个未跟踪文件，说明后续又新增了大量未跟踪内容——与"未重新运行测试/构建"的声明不矛盾 | 一致，无隐藏 |

**结论：Git 基线核对通过。** Codex 未隐藏工作区未提交修改、未隐藏分支/远程/标签状态，且**阶段0审计文档本身也诚实地把自己列入了未跟踪清单**，这是负责任的做法。但这也意味着——按用户任务书"不得因为总体方向正确而忽略具体证据"的要求——这是一个**当前仍然真实存在、尚未解决的阻断条件**，直接影响本次是否可以放行进入阶段1（见第九节最终结论）。

## 四、四份审计文档逐份结论

### 4.1 `00_SCOPE09091050.md`
- 文档冲突优先级声明（仓库规则 > V3 > 最小上线计划 > 旧文档）与用户任务书要求的优先级完全一致，且核实仓库确实不存在 `AGENTS.md`/`CLAUDE.md`/`README.md`（`.cloude` 内的同名文件属于第三方 vendored 材料，不构成项目规则，判断合理）。
- P0 范围、P1/延后范围、明确不做（Non-Goals）三部分与 `NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`、`NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md` 独立比对后，**未发现任何 P1/非目标功能被违规塞入 P0**（子任务逐条核对，见第五节）。
- 但存在**欠缺范围（under-scoping）**问题：非目标清单遗漏 V3 明确禁止的"多 Agent 自由编排/用户自定义工作流"（Major，见 M1）；P0 清单未把"修改前保护用户已有工作"列为独立条目（Major，见 M3）。
- "Stop/Risk Condition" 段落如实披露未跟踪文件问题，未淡化。
- 结论：**基本准确，但范围冻结存在实质性遗漏，需修正后才能作为后续阶段的权威基线。**

### 4.2 `01_BASELINE_AUDIT09091050.md`
- Git 基线部分：见第三节，核实无误。
- 技术基线部分：Vue/Vite/TypeScript/Pinia/Vitest/Tauri/Rust/rusqlite/reqwest/tokio 版本描述与 `package.json`、`Cargo.toml` 实际内容逐一核对**完全一致**。
- 代码证据引用（`api.rs:24/138/141/185`、`commands.rs:298/309/317`、`nova.ts:123/129`、`db.rs:38/120`、`lib.rs:46`、`security.rs:30`、`security.ts:8`、`validation.rs:6/16/35/45/67`、`tauri.conf.json:23/27`、`capabilities/default.json:6`）**共14项独立核对，全部与真实代码内容相符**，仅个别行号指向对象小节而非精确字段（如 `tauri.conf.json:27` 指向 `bundle` 对象起始括号而非 `active` 字段本体），不影响结论正确性。
- "Existing Implemented Capabilities"与"Not implemented for V3"两份清单经代码抽查（`App.vue` 全文、`grep codex/claude` 全仓）验证：**未发现任何 Codex/Claude CLI 适配器代码，未发现 localStorage/sessionStorage 使用，App.vue 确实只有 聊天/供应商/设置 三个硬编码中文 Tab**，与审计描述完全一致。
- "Historical Claude Audit Status"部分：独立重新阅读两份历史 Claude 审核报告并逐条核对现状后确认，**唯一的 High 级历史问题（F1，SSE 增量重试污染）已被真实修复，有对应代码与专门测试**；Medium 级问题（非抢占式取消、生产 CSP 为 null）与 Final 复审中的 R1（并发提交竞态）也已修复，证据链完整、非自述。
  - 发现遗漏：F5（`active_cancel` 全局单槽位并发架构风险，首次审核标记为设计层面遗留问题）在本节完全未被提及（既未说"已修复"也未说"仍未修复"），实际上该问题**仍未修复**，只是被有意存档为 `docs/NOVA_TECHNICAL_DEBT.md` 的 TD-1 技术债——本身是合理决定，但本审计文档理应显式重申其"仍未修复/有意延后"状态，而不是略去不提。列为 Minor（m2）。
  - Final 复审中的 R2-R5（低优先级）未逐条列出，但独立核实后其中 R3、R4 实际上已被修复（有专门测试），R2、R5 本就是非缺陷/文档性质，遗漏不影响任何结论的正确性，仅为完整性瑕疵。
- "Major Baseline Risks"分级基本合理，但漏了本审核新发现的两处 Major（见 M1、M2）。
- 结论：**代码层面证据扎实、诚实、可复核，是本次四份文档中质量最高的一份**；历史问题闭环判断基本准确，仅有 F5 显式状态遗漏的完整性问题。

### 4.3 `02_GAP_MATRIX09091050.md`
- 47行需求-状态矩阵覆盆盖了任务书列出的全部重点检查项（环境检测、三入口、需求确认、修改前保护、Codex开发、Claude只读审核、一次修复二次审核、角色互换、状态机、中断恢复、时间线、证据中心、Markdown报告、用户确认、中英文、Windows构建）。
- 独立比对 V3 产品规格文档后，**未发现越权把 V1 明确禁止的功能（自动部署、私有仓库授权、无限返工、模型市场等）列入"应做"范围**——所有条目要么标注 Missing/Blocker 需要在后续阶段开发，要么明确是"P0应做但目前缺失"，没有一条属于产品文档非目标清单内容。
- 分类基本准确区分了"Complete/Partial/Missing/Conflict/Unverified"五态，且 Complete 状态均限定在"现有 SDKMAX 聊天 MVP"范围内，未被误判为"V3 需求已完成"，判断严谨（例如"SSE/stream integrity"标注为"Complete for current chat MVP"而非笼统"Complete"，正确区分了现有能力与V3新需求的边界）。
- 缺口：CLI 登录态检测（而非仅"是否安装"）未作为独立行列出，混在"Git/Codex/Claude CLI detection"一行内，存在被简化实现满足文字但达不到 V3 §14.2 登录/额度/权限/进程错误区分要求的风险。列为 Minor（m1）。
- 结论：**矩阵内容真实、无越界、无遗漏历史 Blocker 级问题，风险分级（Blocker/Major/Minor）基本恰当**，仅登录态检测颗粒度可以更细。

### 4.4 `03_IMPLEMENTATION_MAP09091050.md`
- 声明"基于当前代码而非机械照抄产品计划"——核实属实：Phase 1-6 的"Likely affected files"均为仓库中真实存在的文件路径（已逐一核对存在性），且工作包划分（Runtime Foundation → Task State Machine → Local Project Core Loop → Timeline/Evidence → Create Product → GitHub Import → i18n → CI/Release）在语义上是把《最小上线版本指定开发计划》第6节的阶段0-8合理拆分/重排，并非机械复制中文原文，而是结合当前代码现状重新组织了阶段3与阶段5的实施顺序（先做本地项目闭环，再做模板创建和GitHub导入），这一调整有其工程合理性。
- 每个 Phase 均标注 Local tests、Claude review gate、Git boundary（单一提交）、Rollback point、Prerequisites、Forbidden expansion，结构完整，符合"可独立开发、测试、提交、审核、回滚"的要求。
- 发现结构性遗漏（Major，见 M2）：**产品计划第6节"阶段7：发布候选版、本地端到端验证"在本文档中没有对应的独立阶段**。该阶段要求的10项强制端到端场景（全新安装、模板创建闭环、保留未提交修改打开项目、GitHub导入不自动执行脚本、角色互换、首审失败-修复-复审通过、复审仍失败正确停止、执行中重启恢复、CLI缺失/未登录/网络中断/取消等异常路径）以及必须产出的6份发布文档（`04_TEST_PLAN_AND_RESULTS.md`、`05_CLAUDE_FINAL_REVIEW.md`、`06_RELEASE_CHECKLIST.md`、`07_ROLLBACK_PLAN.md`、`08_KNOWN_LIMITATIONS.md`、`CHANGELOG.md`、`PRIVACY.md`）在 Codex 的 Phase 8（"Windows Build, CI, and Release Preparation"）中均未被提及，Phase 8 只列出了 npm/Rust/Tauri 构建检查与安装包/SHA256验证，未覆盖端到端场景清单与文档清单。若不修正，后续团队有可能直接从 Phase 7（i18n）跳到 GitHub Actions 接入，漏掉强制的端到端回归与发布文档产出。
- 次要偏差（Minor，见 m4、m5）：各 Phase 的 Rollback point 用"回退该提交"代替产品计划规定的 `milestone/*` 标签命名规范，未做说明；未显式记录后续 Phase 是否继续沿用现有的 `feature/nova-technical-mvp` 分支或另开分支，产品计划第5.1节要求先审计现有分支规范并明确决定。
- 结论：**阶段划分整体基于真实代码、顺序合理、边界清晰，是可执行的实施地图**，但发布验证阶段（对应产品计划阶段7）的整体缺失是一个需要在进入阶段1前修正的结构性问题。

## 五、产品需求覆盖情况

独立通读 V3 产品规格与最小上线计划后，逐项核对任务书列出的18个重点模块，结果：

| 模块 | 覆盖情况 |
|---|---|
| 环境检测 | 覆盖，但登录态颗粒度不足（m1） |
| 新建产品Beta | 覆盖（Gap Matrix + Phase 5） |
| 打开本地项目 | 覆盖（Gap Matrix + Phase 3） |
| GitHub公开仓库导入 | 覆盖（Gap Matrix + Phase 6） |
| 需求确认 | 覆盖，但"禁止影响的原有功能"字段未被点名（次要，未单列） |
| 修改前保护 | Gap Matrix 中标注 Blocker，但未进入 00_SCOPE09091050.md 的 P0 正式清单（M3） |
| Codex开发 | 覆盖（Blocker，Phase 1&3） |
| Claude只读审核 | 覆盖（Blocker，Phase 1&3，且明确"技术强制只读，非仅靠提示词"） |
| 一次修复+二次审核 | 覆盖，且正确限定为"仅一次"，未被过度扩大为多轮 |
| 角色互换 | 覆盖，且正确限定为"任务开始前互换，开始后锁定" |
| 状态机 | 覆盖（Blocker，Phase 2） |
| 中断恢复 | 覆盖（Partial识别现状，Phase 2补齐） |
| 时间线 | 覆盖（Phase 4） |
| 证据中心 | 覆盖（Blocker，Phase 4） |
| Markdown报告 | 覆盖，七类文档清单在 Phase 3 中列出 |
| 用户完工确认 | 覆盖（Blocker，Phase 4） |
| 中英文界面 | 覆盖（Major，Phase 7） |
| Windows构建/安装包 | 覆盖（Major，Phase 8，但端到端验证清单缺失，见 M2） |

未发现把 V1 明确禁止内容（自动部署、私有仓库授权、自动Push/PR、项目管理看板、SDKMAX账户体系、额度自动接力、400模型选择器、无限返工、原始思维链展示、自由多Agent编排）纳入 P0/Phase 1-8 开发范围。唯一需要修正的是"自由多Agent编排/自定义工作流"未被列入 Non-Goals 显式清单（M1），属于文档遗漏而非范围违规。

## 六、历史问题核查

两份历史 Claude 审核报告（`CLAUDE_REVIEW_NOVA_TECHNICAL_MVP.md`、`_FINAL.md`）共记录问题：

| ID | 级别 | 内容 | 当前状态（独立复核，非采信AI自述） |
|---|---|---|---|
| F1 | High | SSE流未完整时静默重试导致内容重复且被误判为完成 | **已修复**，`api.rs:24/138/141` 引入 `received_delta` 门控，`commands.rs:298-306` 正确映射为 `interrupted`，4个专门测试覆盖（含读取I/O错误、取消后保留内容两种此前缺失的场景） |
| F2 | Medium | 取消检查非抢占式，Stop 不能立即生效 | **已修复**，`api.rs:185` 起用 `tokio::select! { biased; ... }`，测试验证500ms内可取消 |
| F3 | Medium | 生产环境 CSP 显式为 null | **已修复**，`tauri.conf.json:23-24` 已配置严格 CSP |
| F4 | Low | 构建/测试产物未记录对应 commit | **已修复**，`scripts/verify-build.ps1` + `test-results/build-info.txt` |
| F5 | Low（设计层面） | `active_cancel` 全局单槽位，若未来允许并发发送存在架构风险 | **仍未修复**，`lib.rs` 中架构未变，属有意存档为技术债（`NOVA_TECHNICAL_DEBT.md` TD-1），本身合理，但 `01_BASELINE_AUDIT09091050.md` 未显式复述这一状态（m2） |
| R1 | Low-Medium | 前端并发提交存在竞态窗口（`sending`标记设置晚于`await`） | **已修复**，`nova.ts:114-129` 已将守卫前移，`nova.test.ts` 有专门测试 |
| R2-R5 | Low | 文档/测试补充类 | R3、R4 已修复（有专门测试）；R2、R5 非缺陷，属文档/风格性事项，未逐条复述但不影响结论 |
| R6 | 非代码 | 真实 SDKMAX 手动联调未验证 | **仍未验证**，`01_BASELINE_AUDIT09091050.md` 正确标注为外部人工验收环节，未被误判为已完成 |

**未发现历史 Blocker 级问题被遗漏或被谎报为已修复。** 唯一 High 级问题（F1）修复证据链完整（代码+测试+当前HEAD可复现）。仅 F5 存在"应显式复述其未修复状态"的文档完整性瑕疵。

## 七、问题清单

### Blocker（进入阶段1前必须解决）

**B1. 工作区存在大量未提交/未跟踪内容，阶段0审计文档本身也在其中，尚无任何决定或基线提交。**
- 对应文件：整个 `D:\nova` 工作区；`docs/release-v0.1.0/00_SCOPE09091050.md`（Stop/Risk Condition 段）、`01_BASELINE_AUDIT09091050.md`（Git Baseline 段）、`03_IMPLEMENTATION_MAP09091050.md`（Phase 1 Prerequisites）均已如实披露此问题。
- 真实证据：`git status --porcelain=v2 --untracked-files=all` 独立执行确认，未跟踪内容包括：`product/`、`docs/product/`、`docs/release-v0.1.0/`（本次审计的四份交付物）、两份历史 Claude 审核报告、根目录重复的 V3 spec、`nova-ui-design.html`、`docs/chatgpt.txt`、以及与项目无关的 `.cloude/` 第三方 vendored 目录（内含完整的 claude-code-main 仓库镜像和一个命名为"新建文件夹"的空目录）。
- 影响：产品基线文档、审计交付物、历史审核证据均无 Git 版本保护，存在被误删、误改、丢失且无法追溯的风险；违反最小上线计划 §4.2"不允许在工作区有来源不明的未提交修改时开始批量开发"的强制规则；也违反 §10 第一工作日执行顺序第6步"Claude复审通过后，冻结v0.1.0范围并建立首个基线提交"尚未执行。
- 修改要求：由用户对每一类未跟踪内容做出明确决定（保留并提交 / 移出仓库 / 删除），尤其 `.cloude/` 建议整体移出仓库（与项目无关）；决定后建立唯一的"接受基线"提交，且该提交范围应仅限文档与配置，不得混入应用代码变更。
- 通过条件：`git status --porcelain` 干净或仅剩已知且被 `.gitignore`覆盖的生成物；`docs/release-v0.1.0/*`、`product/*` 已提交在版本历史中，可通过 `git show` 复现。

### Major（审计结论存在重要错误或遗漏，必须修正文档）

**M1. `00_SCOPE09091050.md` 的 Explicit Non-Goals 遗漏 V3 明确禁止的"自由多Agent编排/用户自定义工作流"。**
- 对应文件：`docs/release-v0.1.0/00_SCOPE09091050.md`（Explicit Non-Goals 段）
- 真实证据：`NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md` §4.2（"Nova V1不允许用户设计任意工作流"）、§5.3（"多Agent自由编排"列为V1禁止扩展项）；`00_SCOPE09091050.md` 当前非目标清单11条中无对应表述。
- 影响：这是 V3 明确命名的产品边界原则（"固定闭环优先"），而非普通功能项；若不显式登记，后续阶段可能在未违反文字条款的情况下引入自定义工作流/自由编排能力，造成范围蔓延。
- 修改要求：在 Explicit Non-Goals 中新增一条，明确写出"不支持用户自定义/自由编排的多Agent工作流，仅支持固定的开发-审核闭环"。
- 通过条件：Non-Goals 清单包含该条目，且与 V3 §4.2/§5.3 表述一致。

**M2. `03_IMPLEMENTATION_MAP09091050.md` 缺少对应产品计划"阶段7：发布候选版/本地端到端验证"的独立工作阶段。**
- 对应文件：`docs/release-v0.1.0/03_IMPLEMENTATION_MAP09091050.md`（Phase 7、Phase 8 之间）
- 真实证据：`NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md` 第6节"阶段7"列出10项强制端到端场景与7份发布材料（`04_TEST_PLAN_AND_RESULTS.md`至`08_KNOWN_LIMITATIONS.md`、`CHANGELOG.md`、`PRIVACY.md`）；`03_IMPLEMENTATION_MAP09091050.md` 的 Phase 8（"Windows Build, CI, and Release Preparation"）仅列 npm/Rust/Tauri 构建检查与安装包/SHA256验证，未提及上述场景或文档。
- 影响：若不修正，后续团队可能在完成 i18n（Phase 7 in Codex 编号）后直接进入 CI/发布接入（Phase 8），跳过强制的端到端回归测试与发布文档产出，违反发布阻断清单第10条（"Blocker或Major级Claude审核问题未关闭"关联的证据链缺失风险）。
- 修改要求：在 Phase 7（i18n）与 Phase 8（CI/Release）之间新增一个独立阶段（或在 Phase 8 中显式并入并列出全部10项场景与7份文档要求），与产品计划阶段7一一对应。
- 通过条件：`03_IMPLEMENTATION_MAP09091050.md` 中存在可对应到产品计划阶段7全部10项场景与全部7份文档产出的阶段定义。

**M3. `00_SCOPE09091050.md` 的 P0 Scope 未把"修改前保护用户已有工作"列为独立条目。**
- 对应文件：`docs/release-v0.1.0/00_SCOPE09091050.md`（P0 Scope 段）；对照 `docs/release-v0.1.0/02_GAP_MATRIX09091050.md` 中"Modification-before-protection/user work guard"一行（已标注 Blocker）
- 真实证据：`NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md` §8 发布阻断清单明确"用户既有未提交修改被覆盖或丢失"可单独阻断发布；§4.2 明确禁止"在工作区有来源不明的未提交修改时开始批量开发"。该能力在 Gap Matrix 中已被识别为 Blocker，但未作为 P0 Scope 的独立一行出现在 00_SCOPE09091050.md 中，只隐含在"打开本地项目"条目内。
- 影响：00_SCOPE09091050.md 是范围冻结的权威文档；若该项仅存在于 Gap Matrix 而不在 Scope Freeze 正式清单中，未来范围核对/验收时容易被忽略。
- 修改要求：在 P0 Scope 中新增独立一行，明确表述为"任务开始前检测并保护用户项目的未提交修改，脏工作区需用户确认后才可继续"。
- 通过条件：P0 Scope 清单包含该独立条目。

### Minor（不阻止进入阶段1，但应登记修正）

- **m1.** CLI 环境检测未把"已安装但未登录/额度异常/权限异常"作为独立能力点名（`00_SCOPE09091050.md` P0 Scope、`02_GAP_MATRIX09091050.md` "Git/Codex/Claude CLI detection"一行），仅笼统写"environment checks"。建议在 Phase 1 验收标准中显式列出登录态区分。
- **m2.** `01_BASELINE_AUDIT09091050.md`"Historical Claude Audit Status"未显式复述 F5（`active_cancel`全局单槽位）的"仍未修复，已存档为技术债"状态，也未逐条列出 Final 复审的 R2-R5。建议补一行明确状态，避免读者误以为首次审核问题已全部闭环。
- **m3.** 根目录存在与 `product\NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md` 字节级相同的重复文件 `D:\nova\NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`，审计未提及需清理。建议在建立基线提交前删除根目录冗余副本，只保留 `product\` 下的规范位置。
- **m4.** `03_IMPLEMENTATION_MAP09091050.md` 各阶段 Rollback point 使用"回退该提交"代替产品计划规定的 `milestone/*` 标签命名（如 `milestone/runtime-foundation`），未说明为何偏离约定命名法。建议统一采用产品计划的里程碑标签命名，或显式说明简化理由。
- **m5.** 未记录阶段1及后续开发将沿用当前 `feature/nova-technical-mvp` 分支，还是依据产品计划 §5.2 新开 `feature/v0.1.0-mvp`。产品计划要求"若仓库已有分支规范，以现有规范为准"，但这一判断结论未被写入任何审计文档。建议在基线提交时一并写明分支策略决定。
- **m6.** 仓库无任何 Git 远程仓库配置，全部历史仅存在本机，无异地备份。建议登记为运营风险，在阶段8接入 GitHub 远程前妥善处理。

### Suggestion（后续优化建议，不得扩大当前范围）

- **s1.** 尽快按 `03_IMPLEMENTATION_MAP09091050.md` 自身建议，把"接受的产品/审计文档"作为阶段1第一个提交的前置步骤单独提交，不与运行骨架代码提交混合。
- **s2.** 将 `docs\NOVA_PRD_MVP.md`、`docs\product\NOVA_V1_PRODUCT_DESIGN_PLAN.md` 等与 V3 冲突的旧文档做归档标注（如移入 `docs/archive/`），避免未来被误当作当前范围引用。
- **s3.** `.cloude\` 目录（含完整 claude-code-main 仓库镜像、zip 包、名为"新建文件夹"的空目录）与项目无关，建议整体移出仓库而非仅"接受为未跟踪噪音"。

## 八、阶段1建议边界

在 Blocker B1 解决、Major M1-M3 修正后，阶段1应严格限定为 `03_IMPLEMENTATION_MAP09091050.md` 中 Phase 1 的范围，本次审核确认该范围本身定义清楚、可独立提交回滚：

**可以保留的现有实现**（无需重做）：
- Tauri 2 + Vue 3 + TypeScript + Rust + SQLite 应用骨架
- SDKMAX 流式聊天的 SSE 完整性、非抢占式取消、并发提交防护（均已验证修复，测试完整）
- API Key Windows 凭据管理器存储、掩码存储、请求日志脱敏（`security.rs`/`security.ts`/`credential.rs`）
- 生产 CSP 配置、Tauri capabilities 最小化（仅 `core:default`）
- 端点白名单校验模式（`validation.rs`，可作为后续 Codex/Claude CLI 命令白名单的参考实现范式）

**需要补测的部分**：
- 无（阶段1为全新功能，非对现有代码补测）

**必须先修复的缺陷**：
- 无功能性代码缺陷需要在阶段1前修复；唯一前置条件是 B1（未提交基线）与文档修正 M1-M3。

**不应该重复开发的内容**：
- 不应重新实现流式请求/取消/脱敏机制，Phase 1 的进程执行器应复用 `security.rs` 的脱敏模式与 `validation.rs` 的白名单校验模式，而非另起一套。
- 不应在阶段1提前实现 Codex/Claude CLI 的任务级调用、GitHub 导入或项目创建模板（`03_IMPLEMENTATION_MAP09091050.md` 已在"Forbidden expansion"中正确排除，本审核确认该边界合理，应严格执行）。

**阶段1准确的开发边界**（与 `03_IMPLEMENTATION_MAP09091050.md` Phase 1 一致，本审核确认无需扩大或缩小）：
- Git、Codex CLI、Claude Code CLI 可用性与登录态检测（须包含 m1 指出的登录/额度/权限状态区分，而不仅是"已安装"）。
- 统一进程/命令执行边界：超时、取消、退出码、stdout/stderr分流、敏感信息脱敏、禁止字符串拼接命令。
- 路径含空格/中文、命令注入尝试等安全测试。
- 单一提交，不得混入 UI 改版或项目入口流程代码。

## 九、最终结论

**FAIL**

理由：存在 1 项 Blocker（B1：工作区大量未提交内容，且阶段0审计交付物本身尚未建立版本化基线，未满足产品计划 §10 第一工作日执行顺序第6步"冻结范围并建立首个基线提交"的前置条件）与 3 项 Major（M1 非目标清单遗漏"自由多Agent编排"禁止项；M2 实施地图缺少发布候选/端到端验证阶段；M3 范围冻结文档未将"修改前保护用户工作"列为独立P0条目）。按用户任务书规则，存在 Blocker 或 Major 即不得进入阶段1。

需要特别说明：本次审核确认 Codex 阶段0审计**在证据真实性、代码引用准确性、历史问题闭环判断上质量很高**——14项代码行级证据独立核对全部准确，唯一 High 级历史问题（SSE重试污染）修复证据链完整可复现，未发现把AI自述当作证据、未发现隐藏工作区状态、未发现越权把V1禁止功能纳入开发范围。FAIL 的原因是**范围冻结文档存在需修正的实质性遗漏，且未提交基线的阻断条件客观仍然存在**，而非发现审计造假或重大失实。

## 十、Codex文档修正清单（不通过时必须完成）

1. 解决 B1：获得用户对全部未跟踪内容的明确决定，建立唯一的"接受基线"提交（仅含文档/配置，不含应用代码）。
2. 修正 `00_SCOPE09091050.md`：
   - 在 Explicit Non-Goals 增加"不支持用户自定义/自由编排的多Agent工作流"（对应 M1）。
   - 在 P0 Scope 增加独立条目"任务开始前检测并保护用户项目未提交修改"（对应 M3）。
   - 补充说明阶段1及后续开发沿用的 Git 分支策略（对应 m5）。
3. 修正 `03_IMPLEMENTATION_MAP09091050.md`：
   - 新增或扩展一个阶段，完整对应产品计划阶段7的10项端到端场景与7份发布文档要求（对应 M2）。
   - 统一 Rollback point 命名为产品计划的 `milestone/*` 标签规范，或显式说明简化理由（对应 m4）。
4. 修正 `01_BASELINE_AUDIT09091050.md`："Historical Claude Audit Status"补充 F5 的"仍未修复，已存档为技术债 TD-1"显式状态（对应 m2）。
5. 修正 `02_GAP_MATRIX09091050.md`："Git/Codex/Claude CLI detection"一行细化登录/额度/权限状态检测要求，不仅是"是否安装"（对应 m1）。
6. 清理根目录重复的 `NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`（对应 m3），并处理与项目无关的 `.cloude\` 目录（对应 s3）。

以上6项全部完成并再次提交独立复审、确认无新增 Blocker/Major 后，方可进入阶段1。
