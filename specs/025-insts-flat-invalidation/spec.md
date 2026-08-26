# 025 `insts_flat` 失效协议：缓存维护退出全表扫

- 依据：`docs/adr/ADR-043-insts-flat-invalidation-protocol.md`
- 前置：ADR-038（journal 纯数据）、ADR-011（单队列单派发器）、ADR-025（阶段序）、
  `docs/plans/2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md` §P2/§P4、
  `specs/019-booled-flat-backfill-closure/`
- 证据：`docs/evidence/2026-08-23-insts-flat-invalidation/`

## 目标

两件事，缺一不可：

1. **`insts_flat` 的维护开销只与本轮变更量成正比**，不与 `inst_relate` 表容量成正比。
2. **一个已物化的 `insts_flat` 永不变旧**；真变旧不了的地方，必须在变旧之前 durable
   失效，让读侧掉回兜底（慢，不错）。

第 2 条不是顺带的：今天已经存在一条能产出「旧值且非 NONE」的路径，而现有清扫两段都
命中不了它。

## 非目标

- **不删缓存**。读侧收益已实测（子查询一项 +8.7s / 5.3 万行）。
- **不做「Rust 终态字面量」那条根治路线**（ADR-043 长期方向）。它的前置是本 spec 的
  FR-4/FR-5 先闭环，另开一轮。
- **不动读侧 pass1/pass2 三分法的骨架**，只加 FR-8 的行内自检与版本位。
- **不改 `DRAIN_PAGE_SIZE`、不碰 `gen_cata_geos` 缓存**。同一份日志里量到的另外两笔
  开销，与本 spec 正交，各走各的。
- **不新建索引**（除非 FR-6 的 `EXPLAIN` 前置实测证明必要）。

## 现状（已核对）

| 事实 | 落点 |
| --- | --- |
| 清扫三段谓词都不可索引，`inst_relate` 只有 `anc` / `dbnum` 两个索引 | `sweep_inst_relate_flat` 与 `init_inst_relate_indices`，同在 `src/fast_model/pdms_inst.rs` |
| 第三段脏值计数无 `LIMIT`，只为打一行警告 | `sweep_inst_relate_flat` 末段 |
| 脏位是进程级 `AtomicBool`，由「packets 非空」触发 | `INSTS_FLAT_DIRTY` / `mark_insts_flat_dirty`；调用点是 `save_instance_data` 尾与 `src/fast_model/occ_generate.rs` 的 AABB 段尾 |
| 失效集与「本轮写过的行」不是同一个集合 | 脏位由 `!plan.packets.is_empty()` 触发（含 `inst_geo` / `geo_relate` / 共享内容 packet），而 `SavePlan::written_refnos` 只收实际写出的 `inst_relate` 行 |
| 共享 geo 可以从 `bad` 恢复成 `meshed` | `render_inst_geo_upsert` 的第三个参数为真时发 `UPDATE inst_geo:⟨hash⟩ SET bad = false;` |
| 回填段与修复段对空串 `booled_id` 的判定相反 | 回填 `IF booled_id != NONE THEN [{geo_hash: booled_id}]`（空串命中布尔分支）vs 修复/脏值段把 `''` 与字面 `'none'` 当作「无成品，不得写入平表」 |
| 布尔成功路径已内联写平表 | `src/fast_model/occ_generate.rs` 的 `set booled_id=…, booled=true, insts_flat=[{geo_hash:…}]`；manifold 侧同形 |
| 读侧正确性不依赖物化覆盖率 | P4 §读侧两段式：三副本齐活直接用 / 副本缺走 pass2 `query_insts_slim` / 无链接丢弃 |
| 空闲轮清扫挂点 | `src/data_interface/batch_worker.rs` 空闲轮的 `sweep_inst_relate_flat_if_dirty` |
| 启动清扫挂点 | `src/lib.rs` 启动预加载段的 `sweep_inst_relate_flat` |

实测成本（27.5h 服务日志，watch 限定 dbnum 8000）：清扫 1,021 次 / **4.33h** / 均值
15.26s，同期真几何 1,199 次 / **1.50h**。中位战果「补 16 行」。

## 功能需求

**FR-1 空闲轮不得发无索引谓词。** 空闲轮里的 `insts_flat` 维护只允许按 record id 点名
`UPDATE`。全表扫只保留两处：启动序列、人工诊断入口。这条对将来新增的任何周期性检查
同样有效——`LIMIT 1` 不豁免（谓词无索引时仍可能扫很远）。

**FR-2 脏位换成失效集。** `AtomicBool` → `flat_invalidated_refnos`（refno 集合）。
语义是「本次提交之后，哪些行的 `insts_flat` 逻辑值可能变了」，**不是**「本轮写过哪些
行」。取批用整体 take（`std::mem::take`），成功即丢弃快照、失败即并回活动集；
**不得**「先取 ID、刷新完再逐个 remove」——那会把刷新期间新产生的同 refno 脏标记一起删掉。

**FR-3 失效集有上界且溢出可收敛。** 集合超过配置上限时，记一条告警并置「需要一次全表
自愈」，由启动或低频审计消费，同时清空集合。内存不得无界。

**FR-4 消费点在生成任务终态之后。** 收集 → mesh / OCC / manifold / AABB 全部完成 →
生成任务成功 → 才刷新。**建行时不得把正体实例写进 `insts_flat`**：带负实体的构件会在
OCC 跑完之前被读侧当成「三副本齐活」直接画出未做布尔的正体。这一条用源码顺序断言钉住。

**FR-5 变旧之前必须 durable 失效。** 任何可能让**已物化**缓存变旧的写，必须在同一事务
里要么写入新的正确值、要么把该行缓存 durable 置为无效。当前满足的：`inst_relate` 替换
写（`DELETE`+`INSERT`，新行不带 `insts_flat`）、OCC/manifold 布尔成功路径。当前不满足
的：共享 geo 的 `bad → meshed`（见 FR-7）。

**FR-6 刷新载体二选一，由前置实测定。**

- 选项 P：独立轻量 pending 表，record id 即目标 refno。只含脏行、崩溃安全、不需要任何
  二级索引、成功 UPDATE 与删除 pending 同事务、重放幂等。**不得塞进
  `model_update_pending`**——那张表没索引且已承担几何重试与死信语义。
- 选项 V：仅进程内失效集 + 启动全量自愈。

选 V 的充分条件是「丢一个失效 refno 只会让那行长期走兜底，不会让它变旧」。FR-5 成立
之后普通行满足这条；FR-7 的共享依赖不满足。**因此 FR-7 若走反向失效路线，载体必须是
选项 P。** 若选 P，它必须满足宪法 IV 的三条出路（可消费 / 可收口 / 可复活）。

**FR-7 共享 geo `bad → meshed` 必须闭环。** 二选一，由 T02 的前置实测定：

- 路线 A（不可变）：`geo_hash` 一旦终态 `bad` 永不在同一 hash 上恢复 `meshed`，重试改
  走版本化 hash。
- 路线 B（反向失效）：共享 geo 从「不参与 flat」变成「参与 flat」时，找出所有引用它的
  `inst_relate` 行，durable 置无效并入队刷新。

**无论选哪条，回归测试先于实现**：两个 refno 共用一个 `bad` geo，只对其中一个做定向
重生成，另一个的 `insts_flat` 不得停在旧值。

**FR-8 读侧行内自检与版本位。** pass1 多取一个 `booled_id`；`booled_id` 有效而
`insts_flat[0].geo_hash` 与之不符时当作缓存未命中转 pass2（不需要图查询）。另加
`insts_flat_ver`，读者只信当前版本，旧版本一律退化为兜底。

**FR-9 存量修复改为带标记的 migration。** 判据是**库上的迁移标记**，不是「本进程跑过
一次」：旧备份恢复、库拷贝、滚动部署期间短暂跑起旧 writer，都会重新产生老格式。
流程是「标记不存在 → 修复 → 复核无残留 → 落标记」。

**FR-10 修掉回填段的空串分歧。** 回填段的布尔分支判定改为与修复/脏值段同一个谓词
（`!= NONE && != '' && lowercase != 'none'`）。这不是性能项——今天 `booled_id = ''`
的行会被回填成 `insts_flat = [{ geo_hash: '' }]`，与同函数下面两段的定义相反。

**FR-11 脏值计数退出热路径。** 改 `LIMIT 1` 只判有无，并随其余全表段一起移出空闲轮。

## 非功能需求

**NFR-1 常态开销。** 空闲轮里 `insts_flat` 维护的壁钟必须与本轮失效 refno 数成正比；
以 16 行/轮的常态计，单轮开销进入毫秒级。首次导入基线的清扫总开销对生成根数不得再是
平方级。

**NFR-2 正确性不退。** 读侧五口径（refno / owner / aabb / trans / insts 哈希）对拍与
改动前一致；`insts_flat = NONE AND aabb.d != none` 的残留在收敛后为 0。

**NFR-3 崩溃语义。** 任一步崩溃的最坏后果是「那几行长期走 pass2 兜底」，不得出现
「读者采信旧的非 NONE 缓存」。

**NFR-4 可观测。** 新增三个指标：`flat_pending_count`、`flat_oldest_pending_age`、
`flat_fallback_ratio`（读侧兜底占比）。缺了它们，下一次「正确性没坏但 plant-ui 又慢了
20 秒」只能靠用户反馈发现。

**NFR-5 双引擎一致。** 新增/改写的语句形态照既有惯例在 `fork_surreal_compat` 双跑套件
里钉住（mem 与 fork 2.1.4 行为一致）。生产 8009 与两者有已知分叉（对 `NONE` 实参的
函数调用），涉及处以 live 实测为准。

## 验收

1. 同一个库跑一轮完整首次导入基线，日志里 `平表副本清扫` 的总耗时 / `模型结点更新`
   的总耗时之比从 2.9 : 1 降到 < 0.05 : 1。
2. 读侧五口径对拍与改动前逐行一致；`live_sweep_inst_relate_flat_on_configured_db` 的
   覆盖复核仍为「无残留」。
3. 共享 geo 回归（FR-7）：两个 refno 共用一个 `bad` geo，只重试一个，另一个的
   `insts_flat` 不得停在旧值。**回退到本 spec 之前的写法时这条必须红。**
4. `booled_id = ''` 的行跑一遍回填，`insts_flat` 不得被写成 `[{ geo_hash: '' }]`（FR-10）。
5. 建行阶段的源码顺序断言：`insts_flat` 的写入点不得早于几何终态（FR-4）。
6. 中途 kill 进程：重启后受影响行要么已物化、要么是 NONE 走兜底；**不得**出现非 NONE
   的旧值（NFR-3）。
7. 失效集打到上限：告警出现、集合清空、后续由启动或低频审计收敛（FR-3）。
8. 空闲轮里不再出现任何 `inst_relate` 全表谓词——用源码形状门钉住（FR-1）。

## 开工前置

**T01：先把 FR-7 的反例做成一条会红的测试。** 在动任何清扫代码之前，先证明
`bad → retry → meshed` 这条路径今天真的能产出「旧值且非 NONE」。若证不出（例如现有
调用序确实保证所有 owner 同批重建），FR-7 退化为一条源码顺序断言，路线 A/B 都不做，
FR-6 可以选 V。若证得出，本 spec 的重心从「省时间」变成「补正确性缺口」，FR-6 必须选 P。

**T02：`EXPLAIN` 前置。** 只有在 FR-6 选了「`flat_valid` 布尔列 + 索引」这条备选时才
需要——先用双引擎 `EXPLAIN FULL` 验证 2.1.4 的 planner 会走索引，再决定。选项 P 不需要
任何索引，这一步可跳过。
