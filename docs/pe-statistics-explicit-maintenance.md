# PE 统计的显式维护

`update_dbnum_event` 已退役。初始化、增量、按需读透及其镜像回放、目录补写、修复硬删使用 `data_interface::dbnum_stats`；调用者提供明确的 PE ID 和写入语句，不解析 SQL 来推断修改目标。

## 数据约定

- `dbnum_info_table:<ref0>.count` 是 PE 实际行数，含软删墓碑。重复 UPSERT、软删与撤销软删不改变计数。
- `sesno` 是已见最大元素会话号，`max_ref1` 是已见最大编号；硬删不回退这两个值。它们都不替代 `dbnum_watermark.applied_sesno`。
- 计数归零仍保留 Ref0 与文件身份，用于清理历史残留。整库清理同步移除统计及 `dbnum_stats_state:<dbnum>`。
- 正常写入每批点查涉及的 ID，执行原有写入，再按真实前后差额更新每个 Ref0 一次。PE 与统计同事务提交，冲突重试整批；归属冲突、负计数和统计失败均回滚。

## 初始化与恢复

首次写入尚无 `version=1` 标记的 dbnum 时，在同一事务内按 PE 聚合重建统计并落版本。迁移失败既不会留下版本标记，也不会提交首批 PE。已有标记的正常批次不再全库聚合。迁移本身需要一次全库读取，大库首次写入可能明显较慢；不是每次启动执行。

新库从空集合开始；基线保留 `INSERT IGNORE`，父层补缺不会覆盖叶子版本。基线完成时核验计数并补身份，增量数据和统计成功后才允许推进水位。窗口中断后按原区间重放已提交批次，计数仍收敛。

统计修复工具 `rebuild_dbnum_stats <dbnum>...` 重建统计并刷新版本。禁止在未接入共享维护逻辑的脚本中直接修改 PE；此类维护后须在恢复正常服务前执行统计修复。

## 事务边界与部署

每批最多 100 个元素，并按实际 SQL 字节和逻辑写行估算进一步分块；统计语句计入限制。单个不可拆分元素超过限制时明确失败，避免静默提交半个替换。一次性旧库迁移属于 dbnum 级维护，其工作量不受正常点查批次行数限制。

升级顺序：暂停新的镜像任务，使用旧版排空 v1 element 待办，停止旧写入服务，再启动新版。新版 CLI/Python 初始化会移除事件；写入前再次检查事件是否缺失，防止旧进程重新安装后双重计数。

v2 镜像任务保存完整的显式统计事务。若发现遗留 v1 element 任务，新版不执行、不删除，错误中给出任务路径；用旧版完成排空后再升级。无需清空业务库或删除应用水位。

## 验证入口

运行 `cargo test --lib dbnum_stats::tests`、`cargo test --lib versioned_db::baseline::tests`、`cargo test --lib data_interface::direct_window_replay_parity`、`cargo test --lib data_interface::mirror_spool::tests`，以及 `cargo test --test read_through_rows_parity`。这些用例覆盖真实内存引擎的迁移、重复提交、软删、回滚、并发与镜像提交后未确认的重放。
