# 数据库启动模式与备份范围

2026-09-05：存储介质决定备份范围，取代此前 `store_mode` 与 `data_face` 正交的配置约定。

| 启动配置 | 数据库内容 | 属性读取 |
| --- | --- | --- |
| `store_mode = "kv-mem"` | 模型及其发布所需数据；不摄入 `pe`、`ATT_*`、`pe_owner` | 从源文件读取 |
| `store_mode = "rocksdb"` | 基线与增量数据、属性、模型 | 文件读取仍可用；数据库保留备份 |

`kv-mem` 是 `spawned-mem` 的别名，保留对外端口；`embedded-mem` 同属内存策略。
`rocksdb` 是 `external` 的别名，连接配置的 SurrealDB 服务，该服务须以 RocksDB 后端启动。
客户端别名不会替外部服务更换存储引擎。

旧 `data_face` 配置与 `AIOS_DATA_FACE` 环境变量若与启动模式冲突，会提示并采用启动模式的策略。
内存模式的属性备份/ensure 返回 `skipped-memory`，不会生成属性备份任务，也不会消费磁盘上遗留的备份队列。
模型直接发布到内存数据库，因此接受模型请求前会等待数据库连接与函数安装完成。

RocksDB 使用完整摄入流程和模型持久化。已有数据库若带有 `read-through` 数据面签名，切换后仍会触发
`DATA_FACE_SIGNATURE_MISMATCH`：需要显式迁移，程序不会自动清空或改写其归属标记。

本次未改变 CATA 的既有文件直读规则，也未将磁盘网格文件改为数据库二进制存储。
