# 数据库存储形态与备份范围

2026-09-13（ADR-074）：`store_mode` 只剩两档，**本进程从不拉起数据库**。存储介质决定备份范围（2026-09-05 的口径不变）。

| `store_mode` | 库在哪 | 端口 | 活得过重启 | 数据库内容 | 属性读取 |
| --- | --- | --- | --- | --- | --- |
| `mem`（默认） | 本进程内嵌 `mem://` | 没有 | 否 | 模型及其发布所需数据；不摄入 `pe`、`ATT_*`、`pe_owner` | 从源文件读取（read-through） |
| `rocksdb` | 配置 `v_ip:v_port` 上**已经在跑**的 SurrealDB（rocksdb 后端） | 有 | 是 | 基线与增量数据、属性、模型 | 文件读取仍可用；数据库保留备份（ingest） |

`rocksdb` 档服务**只连、不拉起**：端口上没有库就是启动失败，不会自己起一台空库。要一台本地库先跑
`scripts/Start-Surreal8009.ps1`（仓内）或包内 `Start-Surreal.ps1`，它们会挡住版本不匹配的二进制（必须是本仓 fork 的 2.1.x）。
客户端配置不会替外部服务更换存储引擎——那台服务须以 RocksDB 后端启动。

## 退役写法

| 旧字面值 | 现在落到 | 含义有没有变 |
| --- | --- | --- |
| `embedded-mem` / `kv-mem` / `memory` | `mem` | 只是改名 |
| `spawned-mem` | `mem` | **变了**：以前服务自己拉一台有端口的内存库，现在库在进程里、没有端口 |
| `external` | `rocksdb` | 只是改名 |
| `spawned-rocksdb` | `rocksdb` | **变了**：以前服务自己拉一台落盘库，现在只连——端口上得先有一台 |

旧写法仍然解析得出来，但启动时 `store_mode_notice` 会把「你写的是退役写法，已按 X 生效，含义变成了什么」打出来；
配置向导只写新名。`in_memory_db` 这个更老的开关在没写 `store_mode` 时仍按两态映射（`true` → `mem`，`false` → `rocksdb`），
两个键打架时以 `store_mode` 为准并告警。

## 数据面

`data_face` 由 `store_mode` 派生（`mem` → `read-through`，`rocksdb` → `ingest`），配置里的 `data_face` 与
`AIOS_DATA_FACE` 只是声明，与介质给出的那一档不符时提示并采用介质的策略。

内存形态的属性备份 / ensure 返回 `skipped-memory`，不生成属性备份任务，也不消费磁盘上遗留的备份队列。
模型直接发布到内存数据库，因此接受模型请求前会等待数据库连接与函数安装完成。

RocksDB 使用完整摄入流程和模型持久化。已有数据库若带有 `read-through` 数据面签名，切换后仍会触发
`DATA_FACE_SIGNATURE_MISMATCH`：需要显式迁移，程序不会自动清空或改写其归属标记。

## mem 档连不到

`mem` 档没有端口：`/sql` 探针、`rvm_verify`、`Capture-*Evidence.ps1` 与 plant-ui 的「库供数」读面在这一档都够不着
（plant-ui 按 `/health.mirror.endpoint = "embedded:mem"` 自己降级，主启动序列不受影响）。要外部诊断就用 `rocksdb` 档，
没有第三条路。

本次未改变 CATA 的既有文件直读规则，也未将磁盘网格文件改为数据库二进制存储。
