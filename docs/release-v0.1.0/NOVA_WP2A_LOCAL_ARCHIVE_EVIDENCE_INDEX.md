# WP2A 本地封存证据索引

本索引随完整 WP2A 实现、整改与隔离验证设施一并封存。审核依据为 docs/reviews/CLAUDE_WP2A_REMEDIATION_REVIEW10151457.md 与 docs/reviews/CLAUDE_WP2A_DESKTOP_VALIDATION_REVIEW10171515.md；两者分别覆盖业务正确性和新增设施/DOM/IPC，均不构成发布批准。

适用规格以 NOVA_WP2A_FINAL_AMENDMENT10141726.md 优先，其次 NOVA_WP2A_SELF_CHECK_SPEC10141417.md。历史规格、报告与审核记录保留原文，其中“未提交/待复审”为当时状态；本次实际提交 SHA 见提交完成后生成的 CODEX_WP2A_LOCAL_ARCHIVE_REPORT。

## Git 内轻量索引与本地原始证据

下列 JSON manifest 和两份复现辅助脚本逐项纳入 Git。manifest 是原始历史索引，所列文件并非全部入 Git，不能据此声称克隆仓库即拥有完整原始证据。

- `audit/checkpoints/wp2a-remediation-20261015-1112/delivery-manifest.json`
- `audit/checkpoints/wp2a-remediation-20261015-1112/source-manifest.json`
- `audit/checkpoints/wp2a-remediation-20261015-1112/manifest.json`
- `audit/checkpoints/wp2a-remediation-20261015-1112/handoff-documents.json`
- `audit/checkpoints/wp2a-remediation-20261015-1112/rollback-drill-result.json`
- `audit/wp2a-desktop-validation-20261015-1533/source-delivery-manifest.json`
- `audit/wp2a-desktop-validation-20261015-1533/baseline-manifest.json`
- `audit/wp2a-desktop-validation-20261015-1533/baseline-preservation-verification.json`
- `audit/wp2a-desktop-validation-20261015-1533/approved-delivery-verification.json`
- `audit/wp2a-desktop-validation-20261015-1533/handoff-manifest.json`
- `audit/wp2a-desktop-validation-20261015-1533/build-artifact-hashes.json`
- `audit/wp2a-desktop-validation-20261015-1533/evidence-assertions.json`
- `audit/wp2a-desktop-validation-20261015-1533/verify_evidence.py`
- `audit/wp2a-desktop-validation-20261015-1533/ack_checkpoints.ps1`

完整原始证据仅在本地保留：

- D:\nova\audit\checkpoints\wp2a-remediation-20261015-1112
- D:\nova\audit\wp2a-desktop-validation-20261015-1533（最终 run-07）
- D:\nova\audit\wp2a-desktop-validation-claude-20261017-1513（独立复审补充）

数据库/WAL/SHM、WebView 缓存、编译与安装产物、假 CLI 可执行文件、临时项目及嵌套 Git、完整现场副本、原始日志和补丁保留本地，不纳入本次 Git。manifest 中的绝对路径与原始证据哈希继续指向原现场，不改写为可下载资源。

## 复现与恢复

从仓库源码构建所需 Cargo.lock 与 package-lock.json 已在基线受控，无新增第三方依赖。完整源码包括此前新增的 requirement_items/self_check 存储、模型、迁移、错误类型及调用处，不依赖未提交 product 文档或设计稿。隔离复现命令见 NOVA_WP2A_ISOLATED_VALIDATION_NOTES10151653.md；使用全新审计根，不复用原 run-07。辅助脚本保留原路径假设，需按说明显式选择新 run 根。DOM 文本不等于截图。

Git revert 仅撤销源码，不自动撤销数据库 v11 迁移。恢复仍按 NOVA_WP2A_REMEDIATION_NOTES10151146.md：停止应用并关闭连接，使用 SQLite online backup 和 integrity_check 验证备份，关闭全部连接后在新路径恢复，在隔离环境核验；不得裸复制活动数据库或覆盖真实用户库。

发布前仍需原生输入、视觉与隔离安装/安装后启动验证；真实 CLI 和真实用户库的验证范围仍遵循原审核说明。本次只做本地提交，不 push、不发布、不创建 release 或 milestone 标签。
