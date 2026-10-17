# WP2A：结构化自检契约与完工门禁规格说明

**状态：** 规格设计稿，**未实现任何代码**。本文件只回应 Codex Blocker 1（"自检不作为门禁条件"不可接受），作废旧 `NOVA_SINGLE_AI_MIGRATION_PLAN` 系列文档里 WP2 对自检的定位，提出可实施的替代契约。"方案已修正"不等于"功能已实现"——本文件产出后仍需 Codex 复审方案本身，复审通过后才能排入编码工作包。
**命名变更：** 旧 WP2 作废，新工作包命名为 **WP2A：结构化自检契约与完工门禁**，范围扩大为：开发/修复提示词输出要求、自检报告解析与校验、自检结果存储、`completed` 后端门禁、用户确认入口及所有完成路径、中断恢复与结果失效规则、对应自动测试。
**执行方式不变：** 自检由同一个开发 AI 在同一次调用结束前完成，默认不增加独立 AI 调用——这一条是产品决定（见 `NOVA_V010_SINGLE_AI_SCOPE_CHANGE10132326.md` 第1.1节），本文件只解决"自检结果如何变成一个真正的后端门禁"，不改变"谁来做自检"。

---

## 1. 结构化契约定义

### 1.1 关联键（至少覆盖 PM 要求的四项）

自检记录必须与以下四项强绑定，缺一不可：

- `task_id`
- `result_generation`
- 产生该自检的开发/修复轮次 `round`（1 = 首次开发，2 = 唯一一次修复，与现有 `current_pending_round` 的 round 定义完全一致，不新增round语义）
- 运行标识 `run_id`——复用现有 Action Ledger 的 `dev` action key 格式 `dev-{task_id}-round-{round}` 作为运行标识来源，不新造一套ID体系（与现有 `test_action_key`/`build_action_key` 的 `(task_id, result_generation, round)` 三元组保持同构，自检记录多存一个`run_id`字段仅用于审计追溯，不参与门禁判定逻辑）。

**现存同构先例：** `no_tests_confirmations`/`no_build_confirmations` 表已经是"按 `(task_id, result_generation, round)` 存一条用户确认记录"的模式；自检记录复用同一建模思路，不是新发明一套机制。

### 1.2 内容字段（至少覆盖 PM 要求的五项）

| 字段 | 说明 |
|---|---|
| `requirement_coverage` | 结构化列表，每条含 `{item, status: covered\|partial\|not_covered, note}`，对照已确认需求（`REQUIREMENT.md`/任务的 goal/in_scope 字段）逐条列出 |
| `check_results` | 结构化列表，开发AI自己执行过的检查项与结果（如"已跑eslint""已手动走一遍主流程"） |
| `unfinished_items` | 字符串列表，可为空数组，不可省略该字段 |
| `blocking_issues` | 字符串列表，可为空数组，不可省略该字段 |
| `self_reported_status` | 枚举：`self_reported_complete` \| `self_reported_incomplete` \| `self_reported_blocked`。**命名刻意避免使用"completed"/"passed"等词**，防止与任务真实完工状态混淆；这是开发AI自己的陈述，不是任务状态 |

### 1.3 输出格式与解析规则（回应 PM 第7条："不从任意文本中的'通过'二字推断"）

- 开发/修复提示词**强制要求**输出一个固定结构的区块（建议：在 `DEVELOPMENT_REPORT.md`/`FIX_REPORT.md` 末尾追加一个用唯一起止标记包裹的 YAML/JSON 块，字段即上表五项），不依赖自由文本语义理解。
- Nova 侧用**结构化解析器**（不是关键词搜索）提取该区块。解析失败（找不到起止标记、字段缺失、JSON/YAML语法错误、`unfinished_items`/`blocking_issues`不是数组类型等）→ 记录 `parse_status = invalid_format`，视为无效自检，不等同于"自检未完成"（两者原因不同但后果相同：都不能进入completed）。
- 空壳（起止标记内为空）或只有标题没有字段内容 → 同样落入 `invalid_format`，不是单独分支，复用同一判定。

## 2. 存储设计

新增表 `self_checks`（迁移新增，不改动任何既有表结构）：

```
self_checks(
  id, project_id, task_id, result_generation, round, run_id,
  parse_status,            -- 'valid' | 'invalid_format'
  requirement_coverage_json,
  check_results_json,
  unfinished_items_json,
  blocking_issues_json,
  self_reported_status,
  raw_block_redacted,       -- 原始区块文本，已脱敏，供技术详情展示
  created_at
)
UNIQUE(task_id, result_generation, round)  -- 与test_evidence/build_evidence同构，同一轮次只认最后一次写入
```

**不删除、不覆盖历史行**：若同一 `(task_id, result_generation, round)` 被重新解析（例如重跑文档生成），按现有 evidence 表的既有写入策略处理（覆盖或追加，取决于当前 `test_evidence`/`build_evidence` 的既有实现方式，本规格不改变该既有策略，只要求 `self_checks` 与它们保持同一行为，不单独发明新规则）。

**历史任务兼容：** 表是v0.1.0新增概念，历史任务（在本表存在之前完成的任务）没有对应行，属于正常情况，不回填、不假造历史自检记录；完工门禁对历史任务的处理见第4.4节。

## 3. 完工门禁改动（唯一真正改变判定逻辑的点）

### 3.1 现状（核验自 `NOVA_SINGLE_AI_IMPLEMENTATION_AUDIT` 系列文档）

`completed` 目前只需要 `user_acceptance` 记录，不读取任何AI产出内容；`preview_ready` 由 `adopted_passing_generation_round()`（`core_loop/documents.rs`）基于测试/构建真实证据判定。

### 3.2 改动点

在 `adopted_passing_generation_round()`（或其直接上游，即决定是否可以进入 `awaiting_user_acceptance`/`completed` 的那个判定函数——具体挂载点留给编码阶段按实际函数边界确认，本规格给出的是判定条件，不是具体代码行）新增一条**必要条件**（与现有测试/构建证据条件是"与"关系，不是"或"关系）：

> 存在且仅存在一条 `self_checks` 记录，其 `(task_id, result_generation, round)` 与当前被采纳为"通过"的测试/构建证据所属的 `(task_id, result_generation, round)` **完全相同**，且该记录 `parse_status = 'valid'`，且 `unfinished_items` 为空数组，且 `blocking_issues` 为空数组。

不满足时，任务不得进入 `completed`，停在 `needs_user_action`（或恢复到更早的等待状态，具体选择留给编码阶段，原则是不得静默停在一个看起来"正常"的中间态），并给出具体原因（自检缺失/格式无效/存在未完成项/存在阻断问题/世代或轮次不匹配——五类原因分别对应下面验收矩阵的五行，不合并成一个笼统的"需要处理"）。

### 3.3 `preview_ready` 不变（回应 PM 第4条）

`preview_ready` 继续只由测试/构建真实证据决定，**不要求**自检门禁——这是"可以查看当前结果"的状态，不是"可以确认完工"。自检门禁只卡在 `awaiting_user_acceptance → completed` 这一步，不卡 `preview_ready` 的进入条件。两个状态的判定函数必须明确分离，不得共用同一个"是否通过"布尔值。

### 3.4 历史任务豁免

迁移到有本表的版本之前已经处于 `completed` 状态的历史任务，不回溯要求自检记录（它们完工时自检这个概念还不存在）。本规则只约束**本次迁移之后新产生的** `awaiting_user_acceptance → completed` 转换。

## 4. 与用户确认/恢复/绕过相关的硬性规则（逐条回应 PM 第1–7条）

### 4.1 用户确认不能绕过后端门禁（PM第2条）

`accept_result`（或等价命令）在**服务端**执行时必须重新查一次第3.2节的条件，不能只依赖前端"按钮是否可点"来把关。前端隐藏/禁用按钮是体验层优化，不是安全边界；即使有人直接调用底层命令跳过UI，服务端判定逻辑必须独立拒绝。

### 4.2 自检不能替代真实证据（PM第3条）

第3.2节的条件是"自检有效"**加上**"测试/构建证据已经通过"，两者都是必要条件，不是互相替代的两个选项。本规格不改变现有真实证据门禁的任何既有判定，只是在其基础上叠加一条新的必要条件。

### 4.3 自检与用户确认必须针对同一有效结果（PM第5条）

- 若修复（新一轮 round 或新一个 result_generation）发生在"自检已记录但用户尚未确认"之后，旧自检记录的 `(result_generation, round)` 不再等于新一轮的 `(result_generation, round)`，第3.2节的比较天然失效，无需额外加"是否已修复"的旁路判断——**完全依赖键值比较，不依赖额外的状态标记**，这样不会漏改某个遗留的布尔字段。
- 若用户确认（`user_acceptance` 记录）发生在旧 generation/round，之后又发生了修复，旧确认记录同理因为键值不匹配而对新结果"自动失效"——`completed` 判定必须同时核对 `user_acceptance` 记录的 `(result_generation, round)` 是否等于当前轮次，不能只看"曾经有过一条确认记录"就放行。

### 4.4 应用恢复后的规则（PM第6条）

- 恢复流程（`storage/recovery.rs`）不得在恢复过程中触发任何新的AI调用去"补一份自检"。
- 恢复后，若当前 `(task_id, result_generation, round)` 没有有效自检记录，任务就停在相应的"需要处理"状态，等待正常的round流程产出自检（即走一遍开发/修复的既有路径），**不允许用历史上某个旧generation/round的自检记录来"补齐"当前要求**——键值比较本身就排除了这种情况，恢复逻辑不需要专门写"禁止复用旧自检"的代码，只要完工判定统一走第3.2节的键值比较，就不会发生误用。

### 4.5 结构合法≠独立审核（PM第7条，重申边界）

即使自检记录 `parse_status = 'valid'` 且各字段完整，这只代表"开发AI自己声称检查过且没发现阻断"，**不代表这个结论被独立复核过**。v0.1.0不做独立审核（见既有产品决定），所以这一条不是要求再加一层审核，而是明确写清楚"自检通过"这个状态的真实含义边界，防止未来有人把它误读成"等同于独立审核通过"，写进文档和界面文案时必须用"开发AI自检"而不是"已审核"。

## 5. 受影响的实际函数、存储结构、迁移需求和最小改动清单

| 类别 | 项目 | 改动性质 |
|---|---|---|
| 存储 | 新增 `storage/self_check.rs`（镜像 `storage/no_tests_confirmation.rs` 的既有模式） | 新文件，不改动既有 storage 文件 |
| 迁移 | `storage/migrations.rs` 新增一个迁移版本，新建 `self_checks` 表 | 新增迁移块，不改动既有表结构，无需数据回填 |
| 文档生成/解析 | `core_loop/documents.rs`：扩展 `DEVELOPMENT_REPORT`/`FIX_REPORT` 的内容要求+新增结构化区块解析函数 | 在现有文档生成函数基础上追加逻辑，不重写文档生成框架 |
| 完工门禁 | `core_loop/documents.rs` 的 `adopted_passing_generation_round()`（或其直接调用方）新增第3.2节条件 | 新增一个必要条件判断，不改变现有测试/构建证据判断本身 |
| 核心循环 | `core_loop.rs` 的 round 完成路径（`run_dev_and_test_round` 或等价函数）：在开发/修复轮次产出后插入"解析并写入self_checks"步骤 | 新增一步，不改变round/generation推进逻辑本身 |
| 命令层 | `commands.rs` 的用户确认命令：在服务端重新核对第3.2/4.3节条件 | 新增服务端校验，不改变命令签名 |
| 前端 | `CoreLoopView.vue`/`coreLoop.ts`：展示自检摘要（需求覆盖/未完成/阻断），自检缺失/无效/过期时对"通过验收"按钮给出具体原因提示 | 新增展示区块，复用现有证据展示区的卡片样式，不改变整体页面结构 |
| 测试 | 对应第6节验收矩阵的全部场景 | 新增测试用例，不改动既有测试 |

**明确不做：** 不新建独立状态机分支，不新增任务状态（复用现有 `needs_user_action` 等既有状态表达"自检门禁未通过"），不重写 `current_pending_round`/`result_generation` 的既有语义。

## 6. 验收矩阵

| 场景 | 预期结果 |
|---|---|
| 有效自检完成（字段齐全、无未完成项、无阻断、键值匹配当前轮次） | 可以进入 `completed`（前提测试/构建证据也通过） |
| 自检缺失（无对应 `self_checks` 行） | 不能进入 `completed`，停在需要处理状态，原因="自检缺失" |
| 自检未完成（`unfinished_items` 非空） | 不能进入 `completed`，原因="存在未完成项" |
| 自检存在阻断问题（`blocking_issues` 非空） | 不能进入 `completed`，原因="存在阻断问题" |
| 空壳/只有标题（区块存在但无实质字段内容） | `parse_status = invalid_format`，不能进入 `completed`，原因="格式无效" |
| 格式错误（JSON/YAML语法错误或字段类型不对） | 同上，`invalid_format` |
| 任务不匹配（自检记录的 `task_id` 与当前任务不一致，理论上不应发生，但要有防御性校验） | 被判定为与当前任务无关，等同"自检缺失" |
| 旧generation（存在自检但 `result_generation` 不是当前的） | 键值比较不匹配，等同"自检缺失" |
| 旧轮次（同一generation但round不匹配，如round1的自检用于round2的结果） | 同上，键值比较不匹配 |
| 修复后失效（round1自检+round1用户确认都已存在，之后发生round2修复） | round1的自检与确认均因键值不匹配而失效，必须重新产出round2的自检与确认才能进入completed |
| 重启恢复（应用在round完成但自检未产出前重启） | 恢复后不自动调用AI补自检，也不借用旧generation/round的自检；任务停在等待状态直到正常流程重新产出 |
| 用户确认绕过（直接调用确认命令，不经过前端按钮） | 服务端仍按第3.2/4.1节条件拒绝 |
| 直接状态转换绕过（代码层面任何试图跳过门禁函数直接写 `state=completed` 的路径） | 测试需断言：完工状态转换只有唯一入口（本规格指定的判定函数），不存在旁路写入路径 |
| 真实证据门禁回归 | 即使自检有效，测试/构建证据未通过时仍不能进入 `completed`（两条件同时必要） |
| 一次修复上限回归 | 自检门禁不引入任何新的重试循环，round上限仍为2（1次开发+1次修复） |

## 7. 本文件状态声明

本文件是**规格设计稿**，`self_checks` 表、相关解析/校验/门禁代码、前端展示、全部测试**均未实现**。所有"受影响函数"均为静态引用现有代码位置作为改动落点说明，不代表这些函数已经被修改。本规格需经 Codex 独立复审通过后，才能转入编码工作包。
