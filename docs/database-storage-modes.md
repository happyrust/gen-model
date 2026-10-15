# 数据库存储形态与备份范围

2026-09-13（ADR-074）：介质只剩两档，**本进程从不拉起数据库**。存储介质决定备份范围（2026-09-05 的口径不变）。
2026-10（ADR-083 二次修订）：介质按三层裁决——**环境变量 `AIOS_STORE_MODE` > `DbOption.toml` 的 `store_mode`（配置向导第②步落盘的选择）> 默认 `rocksdb`**。`in_memory_db` 与 `AIOS_IN_MEMORY_DB` 不再参与裁决，写了只会在启动时被点名。

| `AIOS_STORE_MODE` | 库在哪 | 端口 | 活得过重启 | 数据库内容 | 属性读取 |
| --- | --- | --- | --- | --- | --- |
| `mem` | 本进程内嵌 `mem://` | 没有 | 否 | 模型及其发布所需数据；不摄入 `pe`、`ATT_*`、`pe_owner` | 从源文件读取（read-through） |
| `rocksdb`（**不设 = 这一档**；向导里的 external） | 配置 `v_ip:v_port` 上**已经在跑**的 SurrealDB（rocksdb 后端） | 有 | 是 | 基线与增量数据、属性、模型 | 文件读取仍可用；数据库保留备份（ingest） |

`rocksdb` 档服务**只连、不拉起**：端口上没有库就是启动失败，不会自己起一台空库。要一台本地库先跑
`scripts/Start-Surreal8009.ps1`（仓内）或包内 `Start-Surreal.ps1`，它们会挡住版本不匹配的二进制（必须是本仓 fork 的 2.1.x）。
客户端配置不会替外部服务更换存储引擎——那台服务须以 RocksDB 后端启动。

## 怎么选介质

三层裁决：**`AIOS_STORE_MODE` > `DbOption.toml` 的 `store_mode` > 默认 `rocksdb`**。前两层各管一件事——
环境变量是「这一次进程怎么起」的显式覆盖（开发隔离与运维 pin 走它），`store_mode` 是配置向导第②步
**落盘的用户选择**（两档：external / memory，external 为默认）。

| 场景 | 写法 |
| --- | --- |
| 配置向导（持久化选择） | 第②步选 external / memory，保存写进 `store_mode`；向导触发的重启把它显式递给孩子进程的 `AIOS_STORE_MODE` |
| PowerShell 手起 | `$env:AIOS_STORE_MODE = 'rocksdb'; cargo run --release -- serve`（仓内 `Start-AiosDatabaseManual.ps1` / `Run-*.ps1` 已带） |
| 发布包 | `Start-AMS.ps1 -StoreMode rocksdb`；不给时按文件里的 `store_mode`，文件也没写才**显式回填 `mem`**（解压即跑照旧） |
| Linux systemd | 单元里 `Environment=AIOS_STORE_MODE=rocksdb`（`deploy/linux/install_backend.sh` 已写）——pin 住后向导改档会输给环境变量，启动日志并排点名两边 |

环境里设了与文件不同档的值：压过它，且启动横幅逐条点名「两边不是同一档、这一档是谁定的」——
2026-09-18 现场踩过静默覆盖，不许再无声。

## 漏设会怎样：落到 `rocksdb`，当场报错

不设、空白、拼错，一律落到 `rocksdb`（ADR-083 D6，2026-09-18 当日把 ADR-074 D3 的 `mem` 翻过来）。地址上没库时长这样：

```
数据库连接中...
Error: SurrealDB 连接失败（ws://localhost:8009）: 由于目标计算机积极拒绝，无法连接。(os error 10061)
→ exit 1
```

难看，但**一眼就知道漏了什么**。翻过来之前兜底是 `mem`：服务照常起、模型照常对，只有属性行一直不进库、校审域整域 503、
重启即清空——2026-09-18 现场为此查了半天，而且是先怀疑增量窗口才找到这儿。默认值兜的是「人没说」，
不该兜到一个会静默换掉数据面的档上。

逃生门还在：`project_path` 没填、或 `v_ip` 空 / `v_port=0` 时仍拐进设置模式，向导打得开、能改回来。
真被 rocksdb 卡住又暂时不想起库，`$env:AIOS_STORE_MODE='mem'` 就行。

为什么只留一个决定者：2026-09-18 之前是三层叠加（环境变量 > `store_mode` > `in_memory_db`），现场真踩过「文件写 `rocksdb`、
会话里残留 `AIOS_STORE_MODE=mem`、服务按 mem 起来、属性行一直不进库、日志一个字没提」。介质是「这一次进程怎么起」的事，
跟启动命令走；配置文件描述的是「这个工程长什么样」。

## 退役写法

| 旧字面值 | 现在落到 | 含义有没有变 |
| --- | --- | --- |
| `embedded-mem` / `kv-mem` / `memory` | `mem` | 只是改名 |
| `spawned-mem` | `mem` | **变了**：以前服务自己拉一台有端口的内存库，现在库在进程里、没有端口 |
| `external` | `rocksdb` | 只是改名 |
| `spawned-rocksdb` | `rocksdb` | **变了**：以前服务自己拉一台落盘库，现在只连——端口上得先有一台 |

旧写法写在 `AIOS_STORE_MODE` 或配置文件 `store_mode` 里都解析得出来，但启动时 `store_mode_notices` 会把
「你写的是退役写法，已按 X 生效，含义变成了什么，请改成规范值」打出来（文件层给的改法是改这一行或让向导重写）。
`in_memory_db`（配置键）与 `AIOS_IN_MEMORY_DB`（旧环境变量）不再决定介质，设了同样只会被点名。

## 数据面

`data_face` 由介质派生（`mem` → `read-through`，`rocksdb` → `ingest`），配置里的 `data_face` 与
`AIOS_DATA_FACE` 只是声明，与介质给出的那一档不符时提示并采用介质的策略。所以漏设 `AIOS_STORE_MODE`
换掉的不只是介质，还有整个数据面——mem 横幅会把 read-through 的后果一并念出来。

内存形态的属性备份 / ensure 返回 `skipped-memory`，不生成属性备份任务，也不消费磁盘上遗留的备份队列。
模型直接发布到内存数据库，因此接受模型请求前会等待数据库连接与函数安装完成。

RocksDB 使用完整摄入流程和模型持久化。已有数据库若带有 `read-through` 数据面签名，切换后仍会触发
`DATA_FACE_SIGNATURE_MISMATCH`：需要显式迁移，程序不会自动清空或改写其归属标记。

## mem 档连不到

`mem` 档没有端口：`/sql` 探针、`rvm_verify`、`Capture-*Evidence.ps1` 与 plant-ui 的「库供数」读面在这一档都够不着
（plant-ui 按 `/health.mirror.endpoint = "embedded:mem"` 自己降级，主启动序列不受影响）。要外部诊断就用 `rocksdb` 档，
没有第三条路。

本次未改变 CATA 的既有文件直读规则，也未将磁盘网格文件改为数据库二进制存储。
