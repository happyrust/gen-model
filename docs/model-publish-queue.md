# DESI 初始化的持久发布队列

RocksDB / ingest 的 DESI 首次初始化使用独立生成器和单个落库消费者。属性解析与生成可以并行；消费者在对应属性源的水位及摘要提交后启动。生成的成功回执表示本地日志已经持久化，不表示数据库发布已经完成。

## 配置

在 DbOption.toml 中配置：

```toml
init_async_publish = true
model_publish_queue_dir = "D:/service-data/model-publish-queue"
model_publish_memory_bytes = 536870912
model_publish_disk_bytes = 21474836480
```

`AIOS_INIT_ASYNC_PUBLISH=0` 只关闭新建异步运行。保留的队列仍必须恢复，不能切回同步生成同一轮。默认目录位于运行目录的 `.gen-model/model-publish-queue-v1`；目录名称保持兼容，实际协议号在日志载荷中。协议不匹配会保留现场并拒绝执行。

`AIOS_INIT_PARALLEL_GENERATION=0` 是串行入口对照开关。此入口还包含原来的数据库清单回读，因此与默认入口的时间差不能全部归因于并行计算。

## 数据库响应延迟

自带存储子进程和 `scripts/Start-Surreal8009.ps1` 默认设置服务端环境变量 `SURREAL_WEBSOCKET_RESPONSE_BUFFER_SIZE=0`，显式已有值优先。外部管理的 SurrealDB 需要在它自己的启动环境配置同一变量；只配置模型客户端不会改变已运行的数据库。

这仅关闭 RPC 响应的定时缓冲，不修改 RocksDB WAL、事务或刷盘设置。7997 的本机对照中，默认缓冲使持久 WebSocket 空查询约需 15.4 ms；关闭后约为 0.17 ms。

## 持久边界

0. 初始化只接受模型投影为空的库（`inst_relate` / `tubi_relate` 无该 dbnum 的行）。清库按 dbnum → ref0 区间做（`fast_delete::wipe_dbnum_for_reinit`），是 `needs_initial_load` 之外唯一的入口。登记时投影非空直接拒绝；保留下来的旧清单若是在非空投影上登记的，消费时以 FailedHold 停下。发布路径不再对非空库逐根对账（2026-09-07：批内跨根 reparent 时冻结的 removal 会删掉新根刚写的几何）。
1. 冻结源、依赖版本、根 revision、目标摘要、运行令牌和数据库 incarnation 写入完整任务清单。
2. 源文件的结构化房间拓扑先写入日志，再登记完整任务清单和数据库运行归属；任务归属登记完成后写本地 Reserved 标记，属性提交后写 Activate 标记。
3. 生成结果与 mesh 文件完成本地持久交接。日志按 8 MiB 或 20 ms 批量刷盘。mesh 文件写完先 `fsync` 临时文件再建硬链接（POSIX 下另刷父目录）：结果帧已经落盘并引用着这个 mesh id，文件却只在页缓存里的话，掉电后那条结果只能作废重生成。队列给每件 mesh 记的基准是**内容摘要**而不是字节数，发布前逐件比摘要——长度相同的损坏文件正是长度比较看不见的那一类；重启后重建账本时，`e3d_baked_v2_` 那一档还要回到它自己的内容 id 上验一遍，不拿当前文件当新基准（图元那一档的短 id 从文件复原不出 key，只能靠字节摘要盯它入册之后有没有变过）。
4. 精确 SQL 分块计划与 Inflight 标记共享一次持久化屏障，完成后才允许执行 SQL。屏障按**批**而不是按根：一批里每根先追加自己的计划与 Inflight 帧，发出本批第一条 SQL 之前一次 `fsync` 覆盖整批（32 根一批就是 1 次而不是 32 次）；拿不到这次屏障就一条 SQL 都不发，这些根留在 Inflight，重启后按 UnknownHold 走 ACK 对账。每块效果与数据库确认标记在同一事务内提交；请求超时进入不确定状态并暂停消费者。期限随请求体量走：单根单块 30 秒，批量请求每多一根加 1 秒、每 MiB SQL 加 8 秒，上限 180 秒。批量上限（32 根 / 4 MiB）在连接或超时类错误后减半、连续 8 批成功后翻倍回升。
5. 最后一块才写根回执、登记空间工作、按版本条件收口模型任务。随后写本地确认。
6. 全部根确认后，将冻结拓扑按校验和分块发布，按完整已发布源清单组合房间拓扑并收敛空间和房间阶段。模型水位正确且无未完成任务后，先写数据库运行完成标记，再写本地 Finish。数据库运行登记持续覆盖最后一个根收口到最终发布的间隙。

几何记录保存结构化投影；已提交结果可从数据库恢复到进程内存，不需要重新计算模型。恢复先构建完整替换图像，成功后一次替换对应数据库的 Current 投影。换进去这一步带条件：取镜像之前先记下该库的提交纪元，换的时候纪元变了就说明读库那几秒里有「只写内存」的按需生成往同一个库提交过（`model_read_route` R2），镜像已旧——重读，最多三次；仍撞上就把这一轮收口推迟到下一轮（计入 `/health` 的 `finalization_conflict_deferrals`），不暂停消费者。照旧镜像换进去的话，那些只在内存里的记录会被无声抹掉，而库里本来就没有它们。协议 3 的消费者及已交接结果恢复不重新读取 E3D；只有尚未生成的任务需要重新验证、打开冻结输入。已完成协议 2 运行仍可从数据库恢复内存投影，未完成旧协议运行必须由兼容版本处理。

## 观测与人工恢复

`GET /api/v1/health` 的 `model_publish_queue` 包含生成、持久交接、落库、待发布、失败、不确定、缓存、磁盘及最老积压时间。`consumer_timing_us` 累计记录 mesh 校验、准备、开始日志、提交及确认日志的耗时；不等于端到端墙钟时间。其中 `journal_begin` 含逐根追加与批级屏障两段，`consumer_work.journal_file_syncs` 是日志文件的落盘次数——它对 `prepared_plans` 的比值就是批级屏障实际摊薄了多少。`current_projection` 给出当前进程的几何和根回执数量。`disk_bytes` 为日志与未确认结果独占/共享 mesh 的合计（共享文件只计一次），另有 `log_bytes`、`queued_mesh_bytes`。最后一个引用确认后，mesh 转归已发布模型，文件不会被队列回收删除。队列预算不代表整个服务输出目录的容量上限。健康状态中的一致性普查有采样时间，不能把观测缓存当作完成判据。

错误进入现有 `/api/v1/error-log` 时间线。请求携带当前项目、MDB、命名空间身份，以下值需对应实际部署。按错误任务 ID 操作：

```http
POST /api/v1/model-publish/jobs/{id}/replay
Content-Type: application/json

{"project":"AvevaMarineSample","mdb":"/ALL","namespace":"你的运行命名空间"}
```

恢复被连接故障或发布错误暂停的消费者：

```http
POST /api/v1/model-publish/resume
Content-Type: application/json

{"project":"AvevaMarineSample","mdb":"/ALL","namespace":"你的运行命名空间"}
```

FailedHold 不自动重试。UnknownHold（连接、超时类错误）由消费者自动对账：暂停后先等 5 秒宽限期，再探活数据库（每 5 秒一次，最多 12 轮）；探不到就保持暂停等人工 `resume`。库可达后逐根查最终 ACK：已有 ACK 的记为已提交、不重发 SQL；确认没有 ACK 的才重放，同一根最多自动重放 3 次，用尽后留在 UnknownHold。只有全部不确定根收口、且当前暂停原因确为连接类时才自动恢复消费者；其它原因的暂停仍只等人工。`/health` 的 `model_publish_queue.auto_reconcile` 记录对账轮次、ACK 命中、重放与用尽次数，`publish_batch_limit` 是当前批量上限。自动对账跨进程重启：启动时持久化的暂停原因若是连接类，消费者第一轮就带着「待对账」起来（重放次数是进程内计数，重启后从零数）；其它原因照旧只等人工。日志 writer 因日志错误退出时把接收端停放回去、持久化暂停原因（`journal writer stopped: …`），生产者的交接在原地等待而不是失败；`resume` 会重建 writer，`/health` 的 `writer_running` 给出它的状态。人工重放仍检查源摘要、数据库身份、运行令牌、目标摘要和实际生成 revision。数据库已有完整确认但本地确认丢失时，恢复复用确认标记，不重复执行效果。来源、协议或日志损坏时保留队列，不通过清空队列来修复。

## 7997 验收证据

隔离测试脚本、输入和二进制摘要、分阶段时间及表内容校验位于 `output/model-publish-queue-7997`。`verify.py` 检查 18,675 个根、源/模型水位 112、空间和房间发布完成；协议 V2 还检查内存中 52,266 个几何记录及 18,675 个根回执。带故障恢复的运行与无中断计时运行分开记录。

实现中的验收进度及审查记录见同目录 `PROGRESS.md`；`PROTOCOL_V2.md` 保留历史问题与修复过程。在这些验收项闭合前，不把单次性能运行视为完整上线验收。
