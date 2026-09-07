# AMS 8000 真实 RocksDB 重启验证

日期：2026-09-05。结果：通过数据库进程重启后的持久性检查。

## 环境与步骤

- 使用本次编译的 `D:/Rust/target/debug/aios-database.exe`。
- 服务端为仓内 `bin/surreal.exe`，版本 `2.1.4+20250317.45013fc9`。
- 独立数据库端口 18049、HTTP 端口 18050；命名空间 `rocks8000_20260905`。
- 数据目录：`output/rocksdb-8000-20260905/store`，实际启动参数为 `rocksdb:<该绝对路径>`。
- 源：`D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001`，19,095,552 字节，sesno 285。
- 配置 `store_mode="rocksdb"`，生产启动走 `ingest`，限定设计库 8000；初始化仍包含必需的 SYS/DICT/COMM 等元数据。
- 完成基线后，通过生产 `/api/v1/model/ensure` 请求 BRAN `24384/22402`，返回 `Generated`、`model_available=true`、`publication_status=ready`。
- 后台模型队列同时生成了一部分模型。停止写入服务后，逐表保存全部行并计算排序、规范化 JSON 的 SHA-256。
- 终止测试 RocksDB 进程 PID 10908，以同一磁盘目录启动新进程 PID 56324。保持应用服务关闭，确保读到的是磁盘恢复的数据，而非重新摄入或重新生成。
- 再次读取全部表，逐表比较数量与完整内容哈希。

## 结果

152 张表、47,843 行，重启前后数量和完整内容哈希全部一致。

| 数据 | 行数 |
| --- | ---: |
| AMS 8000 元素 `pe` | 6,605 |
| `BRAN` 属性 | 560 |
| `EQUI` 属性 | 76 |
| `inst_relate` | 361 |
| `tubi_relate` | 26 |
| `inst_geo` | 5 |
| `aabb` | 388 |
| `trans` | 387 |

`dbnum_watermark:8000.applied_sesno` 保持 285。属性实际上按 noun 分表（例如 `BRAN`、`EQUI`），
不能仅凭旧注释中的 `ATT_*` 名称选择验证表；本次捕获全部数据库表。
BRAN 行中的 NAME、OWNER、HBOR、HPOS 等实际属性随完整行参与内容核对。

## 范围与证据

本次证明真实源数据摄入、生产模型发布及数据库进程重启后的数据保留。
模型为部分生成结果，不是 AMS 8000 全模型完整性验收；没有执行新的 SAVEWORK 增量或断电故障模拟。
额外抽样的 EQUI `24384/24776` 返回 `NoRenderableGeometry`，没有把该样本计为可渲染模型成功。
未改动原有服务或原有数据库目录，测试进程在验证后关闭，磁盘数据和证据保留。

- 证据目录：`output/rocksdb-8000-20260905/`
- `before-manifest.json`、`after-manifest.json`：逐表数量与哈希。
- `before-<table>.json`、`after-<table>.json`：完整行快照。
- `model-ensure-24384_22402.json`：生产接口发布回执。
- `result.json`：机器可读结论。
- 启停工具：`scripts/Verify-RocksDb8000.ps1`；取证工具：`scripts/rocksdb8000_evidence.py`。

隔离运行首次启动缺少 CWD 下的 `resource/surreal` 文件，补齐运行资源后启动成功；启停脚本已包含资源复制。
