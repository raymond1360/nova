# Nova v0.1.0-beta.1 Pre-Phase 1 Worktree Cleanup Report

Executed by: Claude Code
Generated: 2026-09-15 19:53 +0800 (Beijing time; local system clock verified UTC+8 earlier this session)
Repository: `D:\nova`
Trigger: `docs/release-v0.1.0/CODEX_STAGE0_FINAL_REVIEW09151833.md` — Blocker: "Current worktree is not clean" (23 untracked entries at Codex review time).
Task nature: worktree cleanup only. Not Phase 1 feature development. No application code, dependency, config, lockfile, or build-script change was made.

---

## 1. 整理前状态

Branch: `feature/nova-technical-mvp`
HEAD: `d75c28b5a2747404978b4857f1935c53ad8b9456` (`docs(release): close stage 0 gate`)

`git status --short --untracked-files=all` (before cleanup, 24 entries — 23 as listed by Codex's review plus `CODEX_STAGE0_FINAL_REVIEW09151833.md` itself, which was written to disk after Codex captured its own status snapshot):

```
?? GitHub_AI_OpenSource_Secondary_Development_Market_Research_20260910.docx
?? design/Nova UIUX设计交付.zip
?? design/Nova UIUX设计交付第三版.zip
?? design/Nova UIUX设计交付第二版.zip
?? design/Nova UIUX设计交付第四版.zip
?? design/Nova UIUX设计交付第四版/.thumbnail
?? design/Nova UIUX设计交付第四版/Canvas.dc.html
?? design/Nova UIUX设计交付第四版/NOVA_V0.1.0_UI_UX_DESIGN09141713.md
?? design/Nova UIUX设计交付第四版/Nova 原型 v3（旧版留档）.dc.html
?? design/Nova UIUX设计交付第四版/Nova 原型.dc.html
?? design/Nova UIUX设计交付第四版/screenshots/handle.png
?? design/Nova UIUX设计交付第四版/screenshots/work.png
?? design/Nova UIUX设计交付第四版/screenshots/work2.png
?? design/Nova UIUX设计交付第四版/support.js
?? design/Nova UIUX设计交付第四版/uploads/CLAUDE_NOVA_V0.1.0_UI_UX_DESIGN_PROMPT_09141713.md
?? design/Nova UIUX设计交付第四版/uploads/pasted-1789450123949-0.png
?? design/Nova UIUX设计交付第四版/uploads/pasted-1789450538928-0.png
?? docs/product/CLAUDE_NOVA_V0.1.0_UI_UX_DESIGN_PROMPT_09141713.md
?? docs/product/NOVA_V0.1.0_MINIMUM_MVP_DEVELOPMENT_DOCUMENT_V209141918.md
?? docs/product/NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md
?? docs/release-v0.1.0/CLAUDE_STAGE0_FINAL_REREVIEW09091448.md
?? docs/release-v0.1.0/CODEX_STAGE0_FINAL_REVIEW09151833.md
?? product/NOVA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md
?? product/~$VA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md
```

`git status --porcelain=v2 --untracked-files=all` confirmed the same 24 `?` entries, no staged/unstaged changes on tracked files.

`git remote -v`: no output — no remote configured (not guessed, not added).

Last 10 commits (before cleanup):
```
d75c28b docs(release): close stage 0 gate
e38269e docs(release): timestamp generated markdown files
da9d7f4 docs(release): close remaining stage 0 review gaps
823bbd9 docs(release): close stage 0 rereview gaps
5d5014e docs(release): record stage 0 baseline evidence
55e3cb9 chore: accept stage 0 remediation baseline
e1e9675 fix: prevent concurrent chat submission
465d3c8 fix: preserve stream integrity and cancellation
e0de353 chore: harden CSP and build traceability
3e1d2dd feat: add nova technical mvp
```

No `git reset`, `git clean`, `git checkout --`, `git stash`, `rebase`, or `merge` was executed at any point in this task.

## 2. 24 个未跟踪条目逐项分类

| # | Path | 分类 | 处理 |
|---|---|---|---|
| 1 | `GitHub_AI_OpenSource_Secondary_Development_Market_Research_20260910.docx` | 无关用户资料 | 移入备份 |
| 2 | `design/Nova UIUX设计交付.zip` | 设计交付 v1 归档（已被 v4 取代） | 移入备份 |
| 3 | `design/Nova UIUX设计交付第三版.zip` | 设计交付 v3 归档 | 移入备份 |
| 4 | `design/Nova UIUX设计交付第二版.zip` | 设计交付 v2 归档 | 移入备份 |
| 5 | `design/Nova UIUX设计交付第四版.zip` | 设计交付 v4 原始压缩包（内容已解压整理） | 移入备份 |
| 6 | `design/.../第四版/.thumbnail` | 工具生成缩略图缓存 | 移入备份（任务书明确排除） |
| 7 | `design/.../第四版/Canvas.dc.html` | 空壳包装文件，仅加载 support.js，无实际内容；未被 Nova 原型.dc.html 引用 | 移入备份 |
| 8 | `design/.../第四版/NOVA_V0.1.0_UI_UX_DESIGN09141713.md` | 正式设计文档 | 纳入 `design/uiux-v0.1.0/` |
| 9 | `design/.../第四版/Nova 原型 v3（旧版留档）.dc.html` | 明确标注的旧版存档；grep 确认当前原型不依赖它 | 移入备份 |
| 10 | `design/.../第四版/Nova 原型.dc.html` | 当前正式原型 | 纳入 `design/uiux-v0.1.0/` |
| 11-13 | `design/.../第四版/screenshots/{handle,work,work2}.png` | 任务书明确列为开发需要的参考资料 | 纳入 `design/uiux-v0.1.0/screenshots/` |
| 14 | `design/.../第四版/support.js` | 原型运行时依赖（`Nova 原型.dc.html` 通过 `./support.js` 相对引用） | 纳入 `design/uiux-v0.1.0/` |
| 15 | `design/.../第四版/uploads/CLAUDE_NOVA_V0.1.0_UI_UX_DESIGN_PROMPT_09141713.md` | 与 `docs/product/CLAUDE_NOVA_V0.1.0_UI_UX_DESIGN_PROMPT_09141713.md` **SHA256 完全一致**（`ea37c977...`），确认为重复副本 | 移入备份，保留 `docs/product/` 权威版 |
| 16-17 | `design/.../第四版/uploads/pasted-*.png` | 设计会话中粘贴的参考图片，未被原型/脚本引用，非任务书列出的必需资源 | 移入备份 |
| 18 | `docs/product/CLAUDE_NOVA_V0.1.0_UI_UX_DESIGN_PROMPT_09141713.md` | 正式产品/设计提示词文档 | 提交入 Git |
| 19 | `docs/product/NOVA_V0.1.0_MINIMUM_MVP_DEVELOPMENT_DOCUMENT_V209141918.md` | 正式产品文档；未发现内容重复的已跟踪版本 | 提交入 Git |
| 20 | `docs/product/NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md` | 与已跟踪权威版本 `product/NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md` **内容不同**（见第6节） | 移入备份，登记差异待确认 |
| 21 | `docs/release-v0.1.0/CLAUDE_STAGE0_FINAL_REREVIEW09091448.md` | Claude 阶段0最终复审报告 | 提交入 Git |
| 22 | `docs/release-v0.1.0/CODEX_STAGE0_FINAL_REVIEW09151833.md` | Codex 阶段0最终复审报告；本轮任务书明确要求正式纳入 | 提交入 Git |
| 23 | `product/NOVA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md` | 上一轮任务的提示词文件 | **拟移入备份，但文件被 Microsoft Word（进程 WINWORD.EXE，PID 3756）以独占方式打开，操作系统拒绝重命名/移动（"Device or resource busy"）；已重试两次，结果一致。已生成经 SHA256 校验的只读备份副本，原始文件因外部进程锁定暂时无法移出仓库，原样保留在 `product/` 下** |
| 24 | `product/~$VA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md` | 上述文件对应的 Word 临时锁定文件 | 移入备份成功 |

## 3. 纳入 Git 的文件及原因（提交1）

| 文件 | 原因 |
|---|---|
| `docs/release-v0.1.0/CODEX_STAGE0_FINAL_REVIEW09151833.md` | 任务书 A.2 明确列为应保留在仓库的正式审核依据 |
| `docs/release-v0.1.0/CLAUDE_STAGE0_FINAL_REREVIEW09091448.md` | 任务书 A.3，Claude 阶段0最终复审报告 |
| `docs/product/CLAUDE_NOVA_V0.1.0_UI_UX_DESIGN_PROMPT_09141713.md` | 任务书 A.4，正式产品/设计提示词文档 |
| `docs/product/NOVA_V0.1.0_MINIMUM_MVP_DEVELOPMENT_DOCUMENT_V209141918.md` | 任务书 A.5，经核对确认无重复的正式产品文档（见第6节） |

## 4. 纳入 Git 的第四版设计基线（提交2）

| 文件 | 原因 |
|---|---|
| `design/uiux-v0.1.0/NOVA_V0.1.0_UI_UX_DESIGN09141713.md` | 正式设计文档 |
| `design/uiux-v0.1.0/Nova 原型.dc.html` | 当前正式原型，开发真正需要 |
| `design/uiux-v0.1.0/support.js` | 原型运行时依赖，与原型同目录保持相对引用有效 |
| `design/uiux-v0.1.0/screenshots/handle.png`、`work.png`、`work2.png` | 任务书明确列为开发需要的参考截图 |

## 5. 移入备份的文件及准确路径

备份根目录：`D:\nova-backup\pre-phase1\20260915-194822\`

全部移动到 `D:\nova-backup\pre-phase1\20260915-194822\moved-from-repo\<与 D:\nova 下相同的相对路径>`，共 13 项成功移动 + 1 项因外部进程锁定未能移动（改为验证过的只读副本）：

1. `GitHub_AI_OpenSource_Secondary_Development_Market_Research_20260910.docx`
2. `design\Nova UIUX设计交付.zip`
3. `design\Nova UIUX设计交付第二版.zip`
4. `design\Nova UIUX设计交付第三版.zip`
5. `design\Nova UIUX设计交付第四版.zip`
6. `design\Nova UIUX设计交付第四版\.thumbnail`
7. `design\Nova UIUX设计交付第四版\Canvas.dc.html`
8. `design\Nova UIUX设计交付第四版\Nova 原型 v3（旧版留档）.dc.html`
9. `design\Nova UIUX设计交付第四版\uploads\CLAUDE_NOVA_V0.1.0_UI_UX_DESIGN_PROMPT_09141713.md`
10. `design\Nova UIUX设计交付第四版\uploads\pasted-1789450123949-0.png`
11. `design\Nova UIUX设计交付第四版\uploads\pasted-1789450538928-0.png`
12. `docs\product\NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md`
13. `product\~$VA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md`

未成功移动（外部锁定，见第2节 #23）：
14. `product\NOVA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md` — 只读副本已保存为 `D:\nova-backup\pre-phase1\20260915-194822\moved-from-repo\product\NOVA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md.readonly-copy-still-locked-in-repo`（SHA256 `1f351eaa6cb3f8e6fa7bd82108c041f4f60ec2824634bcf5e655712862b29ceb`，与原始文件锁定前记录值一致），原始文件仍在 `D:\nova\product\` 下，未被删除、未被覆盖。

移入 `design/uiux-v0.1.0/`（仓库内移动，非备份，见第4节）的 6 个文件已随提交2纳入版本控制，不再是未跟踪状态。

原 `design\Nova UIUX设计交付第四版\` 目录（含 `screenshots\`、`uploads\` 子目录）在全部内容移出后已确认为空，随即删除空目录本身（未删除任何文件内容）。

## 6. SHA256 清单和备份验证结果

预移动 SHA256 清单：`D:\nova-backup\pre-phase1\20260915-194822\checksums\SHA256_before_move_to_backup.txt`（14 项，移入备份的文件）与 `SHA256_before_move_to_design_baseline.txt`（6 项，移入设计基线的文件）。

验证方法：先记录移动前每个文件的 SHA256，执行同盘 `mv`（Windows 同卷重命名，原子操作），再对新位置文件重新计算 SHA256 并与移动前记录逐一比对。

结果：
- 13 个成功移动到备份的文件：移动后 SHA256 与移动前记录**全部一致**。
- 6 个移动到 `design/uiux-v0.1.0/` 的文件：移动后 SHA256 与移动前记录**全部一致**（其中 `Nova 原型.dc.html` = `e28c27e7fd3453aa3ff19dfae9722cfcc3bb261cd51768c8cdfccea639eac384`，`support.js` = `8fe7df74405f3c55f49b7249c74ea1397e65d07dea2b1bd3b4a489bec2e28cbe`，与移动前完全一致，确认内容未被改动）。
- 1 个因 WINWORD.EXE 锁定无法移动的文件：只读副本 SHA256 与移动前记录一致，原始文件仍在原位、未改动。

备份目录完整清单（含大小）：

```
BACKUP_MANIFEST.md                                                          8360 bytes
checksums\SHA256_before_move_to_backup.txt                                  1765 bytes
checksums\SHA256_before_move_to_design_baseline.txt                          757 bytes
moved-from-repo\GitHub_AI_OpenSource_Secondary_Development_Market_Research_20260910.docx   50444 bytes
moved-from-repo\design\Nova UIUX设计交付.zip                                57394 bytes
moved-from-repo\design\Nova UIUX设计交付第二版.zip                          79174 bytes
moved-from-repo\design\Nova UIUX设计交付第三版.zip                          98903 bytes
moved-from-repo\design\Nova UIUX设计交付第四版.zip                         587343 bytes
moved-from-repo\design\Nova UIUX设计交付第四版\.thumbnail                    17130 bytes
moved-from-repo\design\Nova UIUX设计交付第四版\Canvas.dc.html                  206 bytes
moved-from-repo\design\Nova UIUX设计交付第四版\Nova 原型 v3（旧版留档）.dc.html  112487 bytes
moved-from-repo\design\Nova UIUX设计交付第四版\uploads\CLAUDE_NOVA_V0.1.0_UI_UX_DESIGN_PROMPT_09141713.md  16564 bytes
moved-from-repo\design\Nova UIUX设计交付第四版\uploads\pasted-1789450123949-0.png  128525 bytes
moved-from-repo\design\Nova UIUX设计交付第四版\uploads\pasted-1789450538928-0.png  252501 bytes
moved-from-repo\docs\product\NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md  25671 bytes
moved-from-repo\product\~$VA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md   162 bytes
moved-from-repo\product\NOVA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md.readonly-copy-still-locked-in-repo  7030 bytes
```

文件数量核对：13 个成功移动文件 + 1 个只读副本 = 14 个条目在 `moved-from-repo\` 下（含子路径），与第5节清单一致。

## 7. 重复产品文档的比较结果

### `docs/product/NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md` vs `product/NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md`（已跟踪权威版本）

- SHA256 不同：未跟踪版本 `0bf0feecb624cd219924760b5f4b5380595cae43068828fc76a4dfd2ceab4df9`，权威版本 `fcd18e4ecaf0e28fedc55ef8ea063acec6f03c3d3410a9b783db62c85ff26f82`。
- 差异摘要（`diff`，共30行差异，集中在3处文件名引用）：未跟踪版本仍引用不带时间戳后缀的文件名（如 `00_SCOPE.md`、`01_BASELINE_AUDIT.md`、`02_GAP_MATRIX.md`、`03_IMPLEMENTATION_MAP.md`），而已跟踪权威版本已在 `e38269e` 提交中更新为带 `09091050` 时间戳后缀的实际文件名（如 `00_SCOPE09091050.md`）。除这些文件名引用外，未发现其他内容差异。
- 判断：未跟踪版本是 `e38269e`（Markdown 文件名时间戳重命名）提交**之前**的旧快照，非独立的新内容版本。
- 处理：未直接覆盖或合并；已移入备份（见第5节第12项）；**登记为需要后续确认的产品文档差异**——建议后续确认该旧快照是否可以直接归档（大概率可以，因为差异仅是已完成的文件名同步），但本轮不改变产品范围，不做该判断。

### `docs/product/NOVA_V0.1.0_MINIMUM_MVP_DEVELOPMENT_DOCUMENT_V209141918.md`

- 检查了仓库内所有已跟踪的 MVP/开发相关文档（`git ls-files | grep -iE "mvp|develop"`），未发现文件名或内容匹配的重复权威版本。与已跟踪的 `product/NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`（1089行，标题为"V3"）相比，本文档为806行、标题"V2.0"，是不同的文档。
- 判断：不存在重复版本。
- 处理：作为正式产品文档纳入 Git（见第3节）。

## 8. 第四版正式设计基线路径

`D:\nova\design\uiux-v0.1.0\`，结构：

```
design/uiux-v0.1.0/
├── NOVA_V0.1.0_UI_UX_DESIGN09141713.md
├── Nova 原型.dc.html
├── support.js
└── screenshots/
    ├── handle.png
    ├── work.png
    └── work2.png
```

原型验证结果：
- `Nova 原型.dc.html` 内唯一的相对资源引用为 `src="./support.js"`；移动后 `support.js` 仍与其同目录，引用未失效。
- 移动前后对 `Nova 原型.dc.html` 与 `support.js` 的 SHA256 逐一比对，完全一致，确认内容字节级未变（未修改原型视觉或交互内容，未转换为正式应用代码）。
- 全文 grep 未发现 `Nova 原型.dc.html` 或 `support.js` 对 `screenshots/`、`uploads/`、`Canvas.dc.html`、旧版存档文件的任何引用，因此这些文件的排除或重新定位不影响原型运行。
- 说明：本次验证为静态字节级与引用完整性核对（非真实浏览器渲染），因为该原型是通过 CDN 加载 React/Babel 运行时的独立 `.dc.html` 文件，未启动本地开发服务器。

## 9. 两个提交的精确 SHA

- 提交1（`docs(release): accept final stage 0 evidence`）：`802efa7b5836327a8ffc8e166c3800682ef68c14`
- 提交2（`docs(design): accept v0.1.0 uiux baseline`）：`883850cd7b545d6551d1b30c87122c1fcf87d6d8`

## 10. git remote 状态

```
git remote -v -> （无输出）
```
未配置任何远程仓库。本报告如实记录该事实，未猜测或添加远程地址，未执行任何 push。

## 11. 整理后完整 git status

```
?? product/NOVA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md
```

**仅剩 1 项未跟踪文件**，原因是该文件当前被 Microsoft Word（WINWORD.EXE，PID 3756）独占打开，操作系统拒绝移动（"Device or resource busy"，已重试两次，结果一致）。未强制关闭 Word 进程、未强制解锁、未删除、未覆盖——这些都可能导致用户在 Word 中的未保存工作丢失，属于本任务必须规避的破坏性操作。该文件已有经 SHA256 校验的只读备份副本（见第5、6节）。

## 12. 回滚方法

- 回滚提交2（设计基线）：`git revert 883850c --no-edit`（提交2晚于提交1，独立可回滚，不影响提交1）。
- 回滚提交1（Stage 0 证据/产品文档）：先回滚提交2（如已回滚则跳过），再执行 `git revert 802efa7b5836327a8ffc8e166c3800682ef68c14 --no-edit`。
- 回滚提交3（本报告，见下）：`git revert <提交3 SHA> --no-edit`（在下方"最终结论"部分给出后补充执行）。
- 恢复移入备份的任一文件：从 `D:\nova-backup\pre-phase1\20260915-194822\moved-from-repo\<相对路径>` 复制回 `D:\nova\<相同相对路径>`，并用第6节记录的 SHA256 校验。
- 恢复设计基线到移动前的目录结构：将 `design/uiux-v0.1.0/` 下的文件按第4节表格复制回 `design\Nova UIUX设计交付第四版\` 对应路径，并用 `SHA256_before_move_to_design_baseline.txt` 校验。
- 本仓库未配置远程且本轮未 push，任何本地回滚都不影响共享历史。

## 13. 是否满足 Codex 复审条件

**部分满足，存在一项已知例外，需用户或后续操作解决后才能视为完全满足。**

满足项：
- [x] 未删除、未覆盖任何用户文件；全部操作为同盘移动（rename）或经校验的只读复制。
- [x] 24 个未跟踪条目中 23 个已被分类处理（14 个移入备份、10 个纳入 Git 版本控制的两次提交）。
- [x] 未修改应用代码、测试、配置、依赖、锁文件、构建脚本、Git 历史。
- [x] 未执行 `git reset`/`git clean`/`git checkout --`/`git stash`/`rebase`/`merge`。
- [x] 全部备份文件均有移动前 SHA256 记录与移动后核对结果。
- [x] 设计基线 `design/uiux-v0.1.0/` 可正常读取，原型相对引用未失效。
- [x] 两个提交均为单一、可回滚的精确路径暂存提交，提交前已展示 `git diff --cached --name-status` 与 `--stat`。
- [x] 远程仓库状态如实记录为空，未猜测远程地址。

未满足项（已知例外）：
- [ ] `git status --short --untracked-files=all` **仍有 1 条输出**（`product/NOVA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md`），原因是该文件被 Word 进程独占锁定，本轮未强制处理。**建议**：用户关闭对应的 Word 窗口（或确认该 `.md` 文件并非有意在 Word 中编辑）后，重新执行一次该文件的移动（`mv "product/NOVA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md" "D:\nova-backup\pre-phase1\20260915-194822\moved-from-repo\product\NOVA_STAGE0_5_CLAUDE_CODE_EXECUTION_PROMPT09151635.md"`），该文件内容已有经校验的只读备份副本，不会丢失。
- [ ] `docs/product/NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md`（已移入备份的旧快照）与权威版本的差异仅记录，未做进一步归档判断，留待后续确认（见第7节）。

**本报告不构成 Stage 0 已通过的结论。是否可进入 Phase 1，以 Codex 独立只读复审的结果为准。**
