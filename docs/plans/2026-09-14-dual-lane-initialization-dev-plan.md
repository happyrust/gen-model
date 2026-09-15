# 初始化双 lane 落盘：开发计划（ADR-077）

## 目标

**一个 dbnum 的初始化任务把属性行与模型行正确地写进 RocksDB，两面并发、任务收尾一笔盖两枚水位。**
不是吞吐量、不是内存、不是增量——增量路（ADR-076 S1–S7、`ModelWriteBehind`）本计划一行不改。

## 形状

```
                 ┌─ 解析 spawn_blocking ──► 元素批 SQL ──► [属性 lane: flume(100) → 单写者] ─┐
initialize ──────┤                                                                          ├─► drain ×2 → 核对 → task_end（一笔）
                 └─ produce_slices ──► mpsc(1) ──► 渲染(Initial+UPSERT 回执+房间边) ──► [模型 lane: flume(2) → 单写者] ─┘
                                                     └─ 盒子攒着 → 排空后 converge 一次
```

任一路 Err → drop 两个 sender → 等两个写者排空 → `wipe_dbnum_for_reinit` → Err。

## 切片

| # | 内容 | 落点 | 验收 |
| --- | --- | --- | --- |
| S1 | `sql_write_queue` → `persist_writer`：`WriteJob { sql, label }`、每笔超时（30 s + 8 s/MiB，≤180 s）、冲突重试计数、`Lane` 句柄（sender + drain）；写者逐笔取不攒窗（`WRITE_WINDOW` 与它那条死锁纪律一并退役） | `src/data_interface/persist_writer.rs` | 单测：期限随字节走；排空等消费完；panic 写者记账；语句错记账 + `StopWriting` 不再发；两 lane 并发写同库行都在 |
| S2 | 模型侧纯函数：切片 → 逐根 `ProjectionCommit`（`project_geometry_delta`，无 removals、带回执）→ 逐根 SQL（`render_records(Initial)` + 房间边 + `UPSERT` 回执）→ 按 8 MiB 贪心装笔；单根超 cap = 死信 | `src/fast_model/model_initial_persist.rs` + `e3d_model_service::project_slice_for_initialization` | 单测：`mem://` 空库发一根 → `gen_root` 恰一行 `ready`；超 cap 死信不进队列；两根合一笔 / 超界拆两笔；零几何根有回执 |
| S3 | `dbnum_initialize` 编排：文件枚举根、解析 ∥ 生成 ∥ 两写者、失败清库、盒子收敛、一笔收尾（`TaskEnd.data`）| `src/data_interface/dbnum_initialize.rs`、`task_end.rs`、`baseline_ingest.rs`（拆出「只往 sender 流」）| 源码钉（不碰 `ModelMemoryStore` / `ModelWriteBehind` / `load_baseline_nodes`；水位在 drain 之后）；`mem://` 两 lane 并发写同库、行都在；一 lane 失败另一 lane 也排空后才 wipe |
| S4 | 接线：`initialize_dbnum_baseline` 收成薄壳；`execute_one_dbnum` 初始化分支不再回传 units；`execute_frozen_batch_body` 跳过为 Initialize 生成 / 收尾；删 ADR-077 D12 五项 | `manual_update.rs`、`batch_worker.rs`、`increment_tail.rs` | `cargo test --lib` 不退步；相关源码钉改口径 |
| S5 | 真库实测：`initialize_ams_dbnums --with-model <dbnum>`（与 dbnum 任务同一个编排；比另开一档烟测便宜——烟测只认 MDB 的模型面，没有属性面），同库双 lane 并发的总时长 + 两 lane 的冲突重试数（ADR-077 D3 门）；记 evidence | `src/bin/initialize_ams_dbnums.rs`、`docs/evidence/` | 数据进本文「实测」段 |
| S6 | ADR-077 状态 → 已接受；`CONTEXT.md` 三条词条；ADR-076 加一句「D4/D5/D11 由 077 落实」 | docs | 用户过目 |
| S7 | 生成输入跑完即弃：`model_half` 结束就 `forget_generation_input(dbnum)`；写者账 `elapsed_ms` 改记到最后一笔写完 | `dbnum_initialize.rs`、`persist_writer.rs` | 源码钉 + 空等尾巴单测；7997 A/B 工作集时间线（evidence「跑完即弃」） |

S1 / S2 互不依赖；S3 依赖 S1 + S2；S4 依赖 S3；S5 依赖 S4。

**状态（2026-09-14）**：S1–S6 全部已落（`cargo test --lib` 2063 passed；8000 真库两轮 + 真进程 kill / 重起 / 再重起用例，见 evidence；
ADR-077 已接受，CONTEXT.md 三条词条已加，ADR-076 已加引用）。**2026-09-15**：7997 补量 + S7 已落。

## 不做

- 不改增量路、不动 `ModelWriteBehind`、不动 `POST /model/ensure`。
- 不切 ZONE 根（回执 / 覆盖 / 增量归根三处口径另议）。
- 不做写者二分找坏根；不做多写者配置键；不给 `/health` 加字段。
- 不做初始化期间的读可见性（读路由照旧落 Memory）。

## 删除清单（S4）

- `manual_update::baseline_model_plan` / `load_baseline_nodes` / `baseline_work_items`（读 `pe` 枚举根）
- `BaselineOutcome.units` 与 `execute_one_dbnum` 初始化分支回传 units 那一半
- `execute_frozen_batch_body` 为 Initialize 拿 units 生成的路径（Initialize 的模型已在编排里做完）
- `finalize_baseline` 的「摄入完先推 applied」（applied 并进 `task_end`）
- 初始化路上的 `enqueue_model_watermark_write_behind`

## 实测（S5，2026-09-14，每轮全新 rocksdb 库，详见 `docs/evidence/2026-09-14-adr077-dual-lane-initialization-8000.md`）

| 库 | 只属性面（≈ 今天的基线步） | 双 lane 并发（属性 + 952 根模型） | 冲突重试 / 超时 | 备注 |
| --- | --- | --- | --- | --- |
| 8000 | 19.9 s | **22.1 s / 24.7 s**（两轮） | 0 / 0 | 模型 lane 7.0 / 8.3 s 整个被属性 lane 盖住；串行估 ≈ 27–28 s；`READBACK` 六项与旧路逐字相同；`gen_root` 952 全 `ready`，两枚水位 574、标记 `finished` |
| 7997（2026-09-15 补量） | 155.4 s | **150.5 s**（6,772 根） | 0 / 0 | 模型半边约 58 s 收工、被属性 lane 盖住；`COUNTS` 四项与旧路烟测 `READBACK` 逐字相同；`gen_root` 6,772 全 `ready`，水位 184/184、`finished`。峰值 RSS 1,542 MB = 元素缓存 ≈720 + 属性面 481 + 模型半边 ≈340（关缓存双 lane 822 MB）。前一晚点不到它是 `watch_dbnums = [7998, 8000]` 那道门，不是 MDB |

D3 那道门：**并发，不退**——两 lane 在 RocksDB 上零冲突、零超时，8000 与 7997（记录量 24×）同一结论。

## 元素缓存：跑完即弃已落，有界化待拍板（2026-09-15）

7997 那 1.5 GB 峰值的大头是 e3d-io 池级元素缓存（≈720 MB），不是投影。它有两个问题，分开处理：

1. **驻留**（已落，S7）：初始化的生成输入（`DbSet` + 元素缓存 + 属主表）留在进程级 `GENERATION_INPUT_CACHE` 里直到进程退出。
   `dbnum_initialize::model_half` 结束时 `forget_generation_input(dbnum)`——7997 上模型半边收工那一刻驻留 ~1.35 GB → ~420 MB，
   峰值不变；钉 `the_model_half_forgets_its_generation_input_when_it_is_done`。增量路不动：它的条目只装那一窗碰过的元素，
   且同 sesno 的懒生成（`/model/ensure`）还会命中它。
2. **峰值**（待拍板）：要压 1.5 GB 只能让缓存有界。事实：缓存按 dbno 分表（`DbElementCaches = HashMap<u32, Arc<RwLock<ElementCache>>>`），
   每表 `parsed` + `extraction` 两张 `HashMap<RefNo, Arc<…>>`，不淘汰；关掉它 7997 生成慢 3×、**属性 lane 也慢 70 s**
   （无缓存的几百万次点查与解析器抢 CPU/IO），所以关位不是答案。候选：(a) 分库策略——CATA / DICT 表不设上限（7997 闭包
   15,500 行，小而热），目标 DESI 表设 LRU 上限；(b) 只给 `extraction` 表设上限（描述符抽取是内存大头，导航只用 `parsed`）；
   (c) 按生成根粒度清 DESI 叶子（一根的子树生成完就丢它的叶子，留祖先链）。三条都改 e3d-io，都要先量一次「DESI 元素被读几遍、
   parsed / extraction 各占多少」再定上限——不量就定是瞎猜。不做配置键。

## 验收

见 ADR-077「验收」。
