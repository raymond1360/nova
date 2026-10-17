# WP2A 整改实现与回滚说明

生成时间：2026-10-15 11:46:32 Asia/Shanghai。对应报告：`docs/reports/CODEX_WP2A_REMEDIATION_REPORT10151146.md`。

本文件是开发交接材料，不是独立审核PASS。历史HEAD仍为6a90df951c6c72ab7ae95228d5f2cc4de7e17273，所有改动未提交；没有交付commit SHA。

## 实现边界

- 开发和修复共享同一个任务包生成器，列出真实已确认需求field:index及文本，要求同一次AI stdout的唯一结构化自检块。Mock从实际包读取，不默认猜goal:0。解析拒绝多块、未知字段、缺check_results。
- 完工固定权威采纳tuple后检查当前dev、同attempt证据/授权、自检、完整需求覆盖和用户确认；不回退其他组合。合法PreviewReady证据落后一代仍支持。
- BEGIN IMMEDIATE后读取Task并检查合法状态和当前代；预检分direct completion与显式用户验收提交。前者仍要已有当前确认，后者不把“用户还没点首次确认”当失败；自检不合格不能提交通过，仍可看结果及拒绝。
- 显式generate_requirement_snapshot业务事务核验归属、状态后才幂等；Draft/终态零写入。UI只读展示原需求，点击“确认沿用，生成需求清单”调用；不自动补建、不改变legacy冻结。
- 旧attempt不论新状态均Stale，真实SQL错误传播；历史NULL不能获得实时授权。历史列表与实时授权分离，时间线用dev归属标同代旧执行。
- Case C可查询并显式恢复到Queued，不重复确认/reset/AI调用；Queued可再由用户显式启动。没有自动绕过门禁。
- 双AI休眠、Phase5A/GitHub不启用，历史product及报告不改。

## 未提交交付版本

检查点：`D:\nova\audit\checkpoints\wp2a-remediation-20261015-1112`。
整改前78文件原字节在files/，SHA256在manifest.json；tracked-before.patch保存原始未提交diff。新增干净tracked文件以前置HEAD为准，不会把较早Claude实现或产品文档当成本轮撤销对象。

本轮20文件增量：source-remediation.patch，SHA256 `30315bac6f0f0bcd432c72facccafa6d772608b70a0fb0319f4278f9323778ed`。
源文件前后哈希：source-manifest.json，SHA256 `0f940029e9a56fa32d6d3e9ba6958a3c6d342b7c65e88850293a5d9ccc092a17`。
初始文件保留核验：initial-files-after.json。两个product及全部原docs哈希保持不变。新增报告与本说明不覆盖历史文档，最终清单在delivery-manifest.json。

## 代码回滚：仅补丁边界，不整文件恢复

本轮没有对真实工作区执行回滚。不要采用旧实现报告的整文件checkout/stash或清理未跟踪文件建议。

1. 先停开发写入并保存当时所有tracked diff及新增文件。对照source-manifest逐项校验当前文件是否仍为本轮交付哈希。
2. 若存在后续编辑，先将它们单独保全并识别重叠hunk，人工三方处理；不能强制覆盖或直接套反向补丁。保留检查点，不删除整目录。
3. 仅使用source-remediation.patch所列20文件hunk做反向处理，先 `git apply --reverse --check`，检查成功也不等于授权立即执行。只有确实计划回滚时才执行对应反向补丁。不要对整个HEAD diff反向，更不要恢复整个源文件。
4. 新增测试文件仅在其哈希仍与交付相同、且确认为本轮新增时随精确补丁撤销；不是删除任意未跟踪文件。两个交付文档另作精确清单处理，保留审计证据。
5. 文本patch将CRLF规范为LF，隔离演练验证的是文本一致；需要字节级对照时参考files/和原始manifest，不以换行差异为理由整文件覆盖混合修改。复核原product/文档及无关源码哈希和git status，再运行相应检查。

已经完成的演练：仅复制20个交付源码/测试到检查点rollback-drill内，使用带--directory前缀的反向补丁；20文件恢复到整改前文本，新测试仅在隔离副本删除；实际工作区的20个交付哈希全部未变。结果见rollback-drill-result.json。该演练没有回退任何真实工作。

## 数据库迁移、备份和恢复

v10→v11并非“完全不删除或重命名表”：四张test_evidence/build_evidence/no_tests_confirmations/no_build_confirmations通过复制到新表、DROP旧表、RENAME新表和重建索引保留数据。历史result_attempt为NULL，新确认唯一键加入attempt；NULL历史不被推断成当前授权。legacy分类仅迁移时计算，历史终态和之后新建任务保持NULL。

迁移每步DDL、数据复制及user_version在同一BEGIN IMMEDIATE内；失败整步回滚。测试已在test表重建之后注入冲突验证v11原子性。

自动备份是SQLite online backup API，包含已提交WAL内容，不是原始主文件复制：写独立临时备份→检查非空与integrity_check→以北京时间戳+随机ID命名并rename；不会覆盖旧备份。迁移前备份不等于无需恢复验证。

实际恢复流程（本轮只在临时库演练）：

1. 停止应用、后台任务、全部读写连接和其他可能写同库的实例。不要在连接活跃时替换主文件。
2. 保全当前库及其-WAL/-SHM/journal的一致集合，或在连接可控时先用SQLite online backup生成一致快照。记录来源、user_version和哈希。不能只拷一个仍依赖WAL的主文件。
3. 只选择已验证备份；校验integrity_check、foreign_key_check、schema/user_version、关键ID/数量及业务记录。备份结构完整也不代表恢复点符合业务期望。
4. 全部连接关闭后，在新路径恢复备份并验证，不让旧路径残留WAL/SHM回放污染恢复库。若最终要替换真实路径，先隔离并保全原主库及sidecar，再一致部署；不能混用旧-WAL和新主文件。
5. 配套恢复点支持的代码。若仍运行v11新代码，旧库会再次迁移；不能把重新打开旧备份当作保持旧版本的保证。
6. 隔离环境先只读打开验证版本、完整性、外键、历史数据和关键状态，再决定是否恢复真实环境。失败时保留原库和全部恢复证据，不能继续运行产生新写入。

本轮测试wp2a_online_backup_and_closed_connection_restore_include_wal_commits在全新临时v10库启用WAL且不自动checkpoint，写历史记录后在线备份，再变更源数据、关闭连接，把备份恢复到另一个干净临时路径；只读检查恢复点的标题、版本10、测试行数、integrity/FK全部通过。未打开或恢复真实用户库。

## 验证与剩余限制

后端674/674、前端109/109；cargo check --lib --tests、cargo build --lib、vue-tsc/vite、完整NSIS包通过。原本新增的REQUIREMENT_FIELDS dead-code已移除，DevActionNotSucceeded已实际使用；MSVC构建仍可能输出创建库/对象的linker_messages提示。

完整包在 `D:/nova/apps/desktop/src-tauri/target/release/bundle/nsis/Nova_0.1.0_x64-setup.exe`，SHA256 `40b18ce4eb8828cc47bca50005bc4723be0719e64467b1879e2fadae21814df1`。原tauri.conf.json未修改，打包用audit内overlay。**打包与启动分别记录：打包已完成，安装器未运行，桌面未启动，生产签名/安装/运行验收未做。**

真实桌面IPC NOT RUN：启动入口固定使用app_data_dir并打开迁移真实nova.sqlite3，真实CLI集合无已验证Mock隔离启动模式。本轮没有安全证明就不启动；Tauri注册断言、业务测试及前端invoke mock不冒充真实桌面通过。真实AI CLI/用户库本轮禁止。独立Claude复审需保留这些边界，不能把Codex自检写成最终PASS。
