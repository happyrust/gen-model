# 交接基线与既有证据

记录日期：2026-10-16；协作轮次：ROOM-20261016-01。
此文件是建立文档时的快照，不是文件锁；每个执行者必须重新读当前代码，不能靠历史快照强写。

## 当前工作区观察

- 工作区：`d:\work\plant-code\old\gen-model`；Cursor；Windows/cmd.exe。
- Git HEAD：`7a21aa1de4c3313b33d35577aa82d557fa86cc61`。
- 本轮只读命令：`git rev-parse HEAD && git status --short`，退出 0。
- workspace_brief 当时无活动文件、无运行中命令，编辑器报告 0 错误/0 警告；诊断只覆盖已分析文件。
- Git 工作区有多条线的修改及未跟踪文件；Bridge 仅列本窗口改过的文件，不等于完整 Git 状态。
- 已观察到 changelog、ADR-085、mdb_project_probe、increment_manager、mdb_closure、model_impact、lib、web/setup 等在途修改。该列表不是全部，禁止清理或覆盖其他工作线。

## 已完成修复（不是本轮新增代码）

1. 清边事务：两张边表删除、水位作废、完成记录清空一起提交/回滚。
2. 启动准入：核对当前模型代号与记录；跨代/空代号/缺含房库水位时保守完整初始化，同代水位完整时保留窗口增量。
3. 启动跨库依赖：外库先补齐，再允许本库面板写入；反查不可用时覆盖 MDB 设计库、含房库与源库的并集。

既有证据入口：`verification/room-startup-audit-20261016/verification.md`。
本轮回读的证据文件 SHA-256：`ffea2267ab6bd853c6b7eb8f9b1e5894d94d290c20ff1411b0bb81daac36f242`。

## 本轮读取到的源文件版本

| 文件 | SHA-256 |
|---|---|
| src/data_interface/room_recompute.rs | 82255cdd1327553e9cdd7b5483a536c29efe48533ce24662c2a40db806c95598 |
| src/web_service/handlers.rs | d7ceb3b75eed4faabdf2d82f5a1387c15f833107472ce2baf378d78116021d9c |
| src/web_service/room_rebuild_regression_tests.rs | f554d9fa317b3e3b673747f682b6d4fe7e636d35e44fdadd2bbaf075220c7507 |

## 历史测试，不冒充本轮重跑

- `cargo test --offline --lib room_rebuild_regression -- --nocapture --test-threads=1`：23 passed / 0 failed。
- `cargo test --offline --lib`：2535 passed / 0 failed / 132 ignored / 0 filtered out；历史测试用时 14.10s。
- 本轮建立协作文档时没有重跑上述命令。历史通过只适用于当时工作树，不证明当前所有其他文件未改变。
- 历史构建存在 vendor dead_code、unused cc 与 future-incompatibility 提示，不应写“零构建告警”。

## 仍未完成的验收

- 真实两库几何更新端到端、失败重试与重启恢复。
- 长期缺水位的重复补检与登记恢复。
- 跨代全库初始化、外库整库补算及反查兜底的真实成本；同轮可能重复补算。
- 剩余只读审核不能因为单测全绿而跳过。

当前没有授权操作真实项目数据库/源模型，没有提交或推送授权。需要实验时先提出独立隔离方案。
