# 开发方案：层级投影层整改（DuckLake + kv-mem）

> 规格见 `docs/plans/incremental-update-refno-only-greenfield.md` §4.5「kv-mem 驻留状态」与 §4.6「新层级查询接口」；实现见 `src/data_interface/hierarchy_projection.rs`、`src/data_interface/hierarchy_mem_query.rs`、`src/data_interface/current_hierarchy.rs`。本文件只写「改什么、按序、怎么验收」，不重述已成立的一致性模型。
>
> **范围变更（2026-07-30 第二轮）**：本文件首版只覆盖 kv-mem 缓存层（H1–H4 / M1–M3 / L1）。第二轮把 **DuckLake 权威层 `hierarchy_projection.rs`** 纳入同一份方案，新增 C1–C2 / H5–H8 / M4–M10 / L2–L5。两层的改造顺序互相牵制（例如 C2 的锁拆分是 H1 读会话的前置条件），因此不拆成两份文档。首版条目编号一律保持不变。

## 1. 目标与验收

- 目标一（缓存层）：在**不削弱**「不得观察到混合 release」这一不变量的前提下，把 kv-mem 从「每次查询都要打一次远端持久库」的形态，改成真正的进程内热缓存；并给分片加上内存上界。
- 目标二（权威层）：把 DuckLake 从「被当成 OLTP 库使用」拉回列存湖该有的用法——消除全表扫描重建索引、消除 N+1 点查、消除事务内 `COUNT(*)`；并让发布协议在所有入口上一致。
- 验收（性能）：
  - 一次深度为 `d` 的 `ancestors_inclusive` / `descendants_inclusive` 调用，对 `SUL_DB` 的 readiness 查询次数从 `O(d)` 降到 `O(1)`。
  - `refnos_by_nouns(has_children = Some(_))` 不再随库总行数线性增长。
  - 单个 dbnum 的发布代价不再依赖**项目内其他库的总行数**（当前是依赖的，见 C1）。
  - `current_release(dbnum)` 的代价不再随 DuckLake 提交历史长度增长（见 M4）。
- 验收（正确性）：现有 `hierarchy_mem_query.rs` 与 `hierarchy_projection.rs` 的单测全绿，且新增守护——
  - (a) 会话期间发生 release 切换时，会话内查询必须失败而不是返回旧数据；
  - (b) mem 驱逐失败不得让 `apply_one_file` 失败；
  - (c) 任何发布入口（含 `sync_pdms` 独立全量同步）都不得在水位收口前激活 locator；
  - (d) 增量维护出的 `external_owner_routes` 必须与全量重建结果逐字节相等。
- 验收（内存）：常驻分片总量可配上限，超限时按策略驱逐 ready 分片，`loading` / `syncing` 中的分片不被驱逐；DuckDB 连接有显式 `memory_limit`。
- 验收（可运维）：`/api/v1/health` 能报出常驻分片数、估算内存、DuckLake 快照数与数据目录大小。

## 2. 事实基线

### 2.1 首版复核（2026-07-30，kv-mem 侧）

- 三层结构成立：DuckLake 为当前态层级的唯一权威，per-dbnum 的内嵌 SurrealDB 内存实例为可丢弃缓存，`current_hierarchy.rs` 为兼容包装层；当前 refno 走 fail-closed，不回退旧持久查询。
- 全局服务以 `enforce_readiness = true` 构造（`hierarchy_mem_query.rs:462`）。`get_many` / `ordered_children` / `refnos_by_nouns` / `descendants_inclusive` / `children_for_located_owners` 每一次都调 `ensure_state_ready`（`:444`）。
- `ensure_state_ready` 转调 `model_update_pending::hierarchy_readiness`（`model_update_pending.rs:372`），后者是一条发往 `SUL_DB` 的查询。生产下 `SUL_DB` 走 `protocol-ws` 连独立进程，因此是网络往返。
- `ancestors_inclusive`（`:570`）逐层循环调 `get_many`；`descendants_inclusive`（`:603`）逐层 BFS 调 `children_for_located_owners`。两者都是模型生成与增量规划的按元素热路径。
- `refnos_by_nouns` 在 `has_children` 有值时，先跑 `SELECT VALUE owner FROM hierarchy_node WHERE owner != NONE` 把全库 owner 拉成 `HashSet<String>`，再在 Rust 侧过滤（`:748-752`）。
- `shards: HashMap<u32, Arc<HierarchyMemShard>>`（`:45`）只增不减，只有失败驱逐与显式 `evict` / `clear` 会移除。规格 §4.5 末尾明确「首版已加载 shard 常驻到进程退出……只记录 RSS，不实现 LRU」，LRU 亦在「本期明确不增加」清单内。
- `increment_pipeline.rs:744`：replay 分支 `evict_global_if_resident(...).await?` 用 `?`，失败会中断整个 `apply_one_file`；正常分支 `sync_global_if_resident` 失败只 push warning。
- `locate_dbnum`（`:198`）每次调用都 `spawn_blocking` 一个任务去查 locator。`hierarchy_projection.rs` 侧已有按文件 mtime+len 的 `LOCATOR_CACHE`（`:34`），但任务调度开销每次都付。
- `apply_mem_change_set`（`:756`）把全部 DELETE/UPSERT 语句 `format!` 拼成单个字符串一次发出，不分块；对比冷加载是 `chunks(2_000)` 分批（`:394`）。
- `sync_resident_change_set` 只在 `enforce_readiness = true` 时做 `base_release_id` 的 CAS（`:338-345`）；测试用的 `HierarchyMemService::new()`（`:145`，转调 `with_readiness(store, false)`，`:150`）完全不走 CAS。

### 2.2 第二轮复核（2026-07-30，DuckLake 侧）

首版的 H1–H4 / M1–M3 / L1 **截至本次复核全部尚未实现**，事实基线仍然成立。第二轮新增以下事实：

- `prepare_locator`（`hierarchy_projection.rs:1532-1551`）调 `query_all_rows`（`:2011-2017`），后者是 `SELECT ... FROM hierarchy.hierarchy_node ORDER BY dbnum, refno`，**不带 dbnum 谓词**，把全项目所有库的层级行物化成 `Vec<HierarchyRowV1>`（每行两个 `String`），只为重算 `external_owner_routes`。
- 该路径的调用点：`publish_baseline_inner`（`:620`，幂等短路之后）、`prepare_change_locator`（`:1554`）→ 而 `prepare_change_locator` 又被 `build_change_set`（`:1150`）、`apply_change_set_inner`（`:819`）、`prepare_change_locator_hash`（`:1592`）各调一次。
- `PROJECTION_GATE`（`:24`）是进程级 `std::sync::Mutex<()>`，被**所有**公开方法持有，含纯读路径：`load_dbnum`（`:1301`）、`current_release`（`:1315`）、`load_current_snapshot`（`:1348`）、`load_dbnum_at_snapshot`（`:1391`）、`committed_window`（`:1206`）、`committed_identity`（`:1250`）等。每处都是 `.lock().map_err(|_| anyhow!("hierarchy projection lock poisoned"))?`，**没有 `into_inner()` 恢复**。
- 每个公开方法都执行 `self.connect()`（`:1466`）+ `self.initialize(&conn)`（`:1481`）：新建 in-memory DuckDB、`LOAD` 扩展、`ATTACH ducklake:` catalog、跑一次 `information_schema.tables` 探测。无连接复用。
- `connect()` 不设 `memory_limit` / `threads`。
- `publish_baseline`（`:522-529`）用 `self.locator_publication_marker()?.is_none_or(|marker| marker.dbnum != baseline.dbnum)` 决定 `activate`。只有基线路径调 `begin_baseline_locator_publication`（`manual_update.rs:2594-2602`），增量路径调 `begin_locator_publication`（`increment_pipeline.rs:692`），而生产上唯一的 `publish_baseline` 调用点在 `versioned_db/database.rs:1169`——它同时服务基线路径和独立全量同步，后者没有 marker，因此**无 marker 时立即激活**。marker 是单个全局文件（`locator_publication_pending.json`，`:1579`）。
- `apply_change_set_inner` 的删除是逐 refno 执行预编译语句（`:824-833`）；`count_dbnum`（`:1563`）是分区全量 `COUNT(*)`，在事务内执行（`:849`）。
- `build_change_set` 逐 refno 调 `query_refno`（`:991-996`），再逐 parent 调 `query_refno` + `query_children`（`:1033-1054`）。
- `current_release`（`:2634`）与 `latest_release`（`:2650`）都 `JOIN hierarchy.snapshots() ORDER BY snapshot_id DESC`；`release_snapshot`（`:2612`）按 `commit_message` 过滤 `snapshots()`。
- 全仓库搜不到 `ducklake_expire_snapshots` / `ducklake_merge_adjacent_files` / `ducklake_cleanup_old_files` / `VACUUM`：**DuckLake 没有任何维护动作**。
- `LocatorPointerV1`（`:426-429`）只有 `{schema_version, locator_hash}`，不含 `release_id` / `snapshot_id`。绑定校验只发生在 `activate_published_locator`（`:1926`）的那一刻，不落盘。
- `content_hash`（`:2731`）= `serde_json::to_vec(value)` 后 SHA-256。基线的 `change_hash` 是对全部 `rows` 求的（`:554`）。
- `atomic_write_json`（`:1996`）与 `activate_locator`（`:1876`）都是「写临时文件 → `sync_all` → `rename`」，**无父目录 fsync**。
- `locators/{hash}.json` 无回收；`LOCATOR_CACHE`（`:34`）无淘汰。
- `ducklake_extension`（`:2416`）搜索路径硬编码 `windows_amd64`，且回退项 `resource/duckdb/{version}/...` 是 CWD 相对路径。
- `initialize_dbnum_baseline`（`manual_update.rs:2587-2593`）在 `needs_full_parse` 时取 `hierarchy_publication_guard()`，并持有到函数结束——**横跨整个全量解析**（解析在 `:2603` 之后才开始）。

### 2.2.1 测试基线（2026-07-30 实测）

`cargo check --lib` 通过；`cargo test --lib` 结果 **310 passed / 6 failed / 58 ignored**。六条失败**全部先于本轮改动存在**，且已逐条在隔离、单线程下复现：

| 失败用例 | 归属 |
|---|---|
| `hierarchy_projection::committed_window_recovers_its_locator_before_finalize` | locator |
| `hierarchy_projection::reopening_store_restores_locator_from_latest_committed_release` | locator |
| `hierarchy_projection::replay_restores_latest_global_locator_without_losing_other_dbnums` | locator |
| `hierarchy_projection::operation_window_builds_final_rows_and_reorders_untouched_siblings` | change-set overlay |
| `hierarchy_mem_query::ancestor_walk_crosses_shards_and_rejects_missing_intermediates` | 跨库子节点（见 H9） |
| `model_update_plan::cata_geometry_changes_seed_deferred_cascade_expansion` | 级联展开 |

**六条里有五条落在 locator / `external_owner_routes` / 跨库路由这一片**，也就是 C1、H5、H8 指向的同一块代码。这说明那几条不是纸面推演——这块目前就是红的。修 C1 之前应先让这批用例恢复绿色，否则改完无从判断是修好了还是换了个坏法。

> 更正一条历史记录：`docs/2026-07-29_test-ams-incremental-update-summary-report.md` 称 `cargo test --lib` 有 4 个编译错误。现已不成立——库和测试都能编译，问题是运行期断言失败。

第二轮结论经 Oracle（GPT-5.5 Pro，浏览器模式，session `kvmem-ducklake-round2`）独立复核。**可信度提醒**：该次运行 65.5k 输入、50 秒返回、输出仅 3k tokens，与上一轮 46 秒同样快得可疑；模型选择证据显示确为 Pro，但推理深度可能未跑满，不应视为穷尽式审查。下文标注「本地复核修正」处为本仓库侧对 Oracle 结论的更正。

### 2.3 关于「规格是否被误实现」的澄清

规格 §4.5 写的是「查询获得锁后再次比较 active release，禁止等待期间跨 release 误读」——**每查询一次**这个粒度实现是照做了的。规格没有约束这个「比较」有多贵，而实现把它落成了一次远端查询。问题因此不在粒度违规，而在于：单次成本被低估，且热路径的祖先链遍历与 BFS 会把它乘上层数。整改要保住的是这条不变量本身，不是它当前的实现方式。

### 2.4 readiness 同时是崩溃窗口的防线

增量写路径的顺序是：持久 Surreal 写 pe（步骤 3）→ DuckLake 提交（步骤 5）→ kv-mem 同步（步骤 6）→ 水位推进（步骤 8）。在步骤 3 与 5 之间崩溃时，持久库已有新 PE 而 DuckLake 仍在旧 release。此时**只要有查询路径绕过 readiness，就会读到 new PE 配 old hierarchy 的混合态**。因此 readiness 不能简单去掉或降级为尽力而为，只能换粒度。

## 3. 问题清单

### C1 · `prepare_locator` 全表扫描重建 `external_owner_routes`

**症状**：每发布一个 dbnum，都要把**全项目**的 `hierarchy_node` 拉进内存重算一次跨库 owner 索引。全量同步 N 个库时，第 k 个库的发布要扫前 k-1 个库已写入的所有行，**总扫描量是 O(N²) 级而非 O(N)**。增量侧一个窗口至少走两遍（`build_change_set` 一次、`apply_change_set_inner` 一次）。

**根因**：`external_owner_routes` 被当成「全量派生数据」，每次重算；而它实际上是可增量维护的跨库索引。

**改法**：`external_owner_routes` 语义是 `owner_refno -> set(dbnum)`，即「该 owner 不在本库内、但有子节点落在本库」。从 change-set 增量维护：

- 删除一行：若其 `owner` 非空且不在本库 → `remove (owner, dbnum)`。
- upsert 一行：按 `old_owner` / `new_owner` 分别做 remove / add。

只在两种情况下全量重建：首次基线、显式 rebuild。

**风险（本方案新增的最大一致性面）**：增量漏算会让 locator 静默出错，而 locator 决定跨库 owner 路由，错了就是模型树跳不过去。必须保留一个 **debug-only 校验器**：`rebuild_external_owner_routes(all_rows) == incremental_routes`，在测试与离线校验中跑，不在生产热路径跑。

**验收**：验收标准 (d)；另加一条「跨库 owner 的最后一个子节点被删除后，该 owner 必须从 routes 中消失」的测试。

### C2 · `PROJECTION_GATE` 全局串行 + poison 永久熔断

**症状**：两个独立问题叠加。

1. **读也串行**。一次 kv-mem 冷加载（`load_current_snapshot` 读整个 dbnum 分区）会阻塞所有其他 dbnum 的 `current_release` / readiness 检查，典型的队头阻塞。
2. **poison 永久熔断**。任意一次在 guard 内的 panic 会毒化这把 `std::sync::Mutex`，此后每个 `.lock()` 都失败且没有恢复路径，整个层级投影层对本进程**永久不可用**，只能重启。

**改法**：

- 立刻消除熔断：所有 `.lock()` 改为从 poison 恢复（`unwrap_or_else(PoisonError::into_inner)`），或整体换 `parking_lot::Mutex`（无 poison 语义）。这一步单独就能合入。
- 再拆锁：写锁只保护 `publish_baseline_inner` / `apply_change_set_inner` / locator 激活；纯读路径（`current_release` / `load_dbnum` / `resolve_ref0` / `committed_release`）不再取写锁。
- `load_current_snapshot` 是需要单独论证的一个：它的注释说加锁是为了「release 元数据与行数据不能分两次读」。但实现上它已经是在**同一个 connection 内**先解析出 `snapshot_id`、再 `AT (VERSION => snapshot_id)` 读行——快照隔离本身就保证了一致性，这把锁在此处是冗余的。去掉前需要先确认 DuckLake 的 `AT VERSION` 读确实不受并发提交影响。

**不采纳**：直接换成 `RwLock` 然后读路径拿读锁。DuckDB `Connection` 不是可共享对象，锁的粒度问题不在读写区分上，而在「读路径根本不需要进程级互斥」。

**验收**：新增测试——在 guard 内制造一次 panic 后，后续 `current_release` 仍能正常返回。

### H1 · readiness 每查询一次远端往返

**症状**：一次「内存缓存命中」= 一次远端 SurrealDB 查询 + 一次内存查询。深度为 8 的祖先链遍历要打 8 次远端。缓存的收益被大幅抵消。

**根因**：见 §2.3。

**改法（推荐）**：引入查询会话。

```
HierarchyReadSession {
    dbnum,
    shard,              // 持有读锁
    release_id,         // 会话开始时确认的 release
}
```

`open_session(dbnum)` 做一次 readiness RPC，拿到 shard 读锁后返回；会话内所有查询复用同一份确认结果。正确性来自「会话持有读锁期间，写侧拿不到写锁，无法替换 release」。

**约束**：会话必须限定在一次模型规划 / 一次生成任务内，不得跨发布边界长期持有；否则读锁会把增量同步饿死。实现时应给会话加最长持有时长的断言或日志。

**不采纳的备选**：

- *release epoch 推送*——热路径零 RPC，但需要可靠通知；通知一旦丢失仍必须有 readiness 兜底，等于两套机制并存，先不引入。
- *TTL 缓存*——TTL 窗口内可能看到旧 release，对生成任务不可接受。

**验收**：新增守护测试，会话开启后由另一任务提交 change-set 并推进 release，会话内的后续查询必须失败（而非返回旧数据）。

### H2 · `refnos_by_nouns` 全库 owner 物化

**症状**：`has_children` 有值时，每次调用都把全库 owner 列拉出来建 `HashSet<String>`。十万到五十万行的库上，每次调用都付一遍扫描 + String 分配 + hash 的代价。

**根因**：查询没有表达「该 noun 节点是否被别人当作 owner 引用」，只能靠把 owner 全集捞回来在 Rust 侧做集合判定。

**改法**：把 `has_children` 物化进 `MemRecord`。冷加载时从 DuckLake 侧一次 `owner -> count` 聚合算出，change-set 应用时按受影响 owner 增量维护。查询改为：

```sql
SELECT * FROM hierarchy_node WHERE noun IN $nouns AND has_children = true ORDER BY ref0, ref1
```

复杂度从 O(全库) 降到 O(noun 结果集)。若后续需要更细的判据，可存 `child_count: u32` 而非 bool。

**验收**：现有 `kv_mem_lazily_loads_queries_and_syncs_only_resident_dbnums` 中三条 `refnos_by_nouns` 断言保持不变；补一条「change-set 删掉最后一个子节点后，父节点的 `has_children` 必须翻转」的测试。

### H3 · 分片无内存上限

**症状**：`shards` 只增不减。多库大项目长期运行，常驻内存无界增长。每个分片是一个完整 SurrealDB 内存实例，除行数据外还要付 record 元数据、索引项与分配器开销。

**根因**：规格首版把 LRU 列为非目标，实现照做，但同时也没有落地「只记录 RSS」这一半——目前没有任何观测点能回答「现在常驻了多少」。

**改法**：不做传统 LRU（按访问顺序驱逐会与 release 一致性纠缠）。给分片挂元数据：

```
last_access_at
row_count
bytes_estimate
release_id
state          // loading | ready | syncing
```

后台任务按 RSS 阈值驱逐，**只驱逐 `ready` 状态的分片**，`loading` 与 `syncing` 中的一律跳过。被驱逐的分片下次查询时重新从 DuckLake 冷加载，语义与失败驱逐完全一致，因此不引入新的正确性面。

**分步**：先只加观测（行数 + 估算字节 + 常驻分片数，打进日志与 `/health`），跑一轮真实项目拿到量级，再定阈值与驱逐策略。

**验收**：`/api/v1/health` 能报出常驻分片数与估算内存；配置上限后压测不再无界增长。

### H4 · replay 分支驱逐失败中断整个窗口

**症状**：`increment_pipeline.rs:744` 的 `evict_global_if_resident(...).await?` 会让整个 `apply_one_file` 失败，而同一文件正常分支的 `sync_global_if_resident` 失败只记 warning。

**根因**：语义不对称。kv-mem 按设计是可丢弃缓存，驱逐一个可丢弃缓存的失败不应该比 durable 管线更强势。

**改法**：统一为 warning，与正常分支同构。

**验收**：新增测试——mock 驱逐失败，`apply_one_file` 仍成功且 warnings 中含对应条目。

### H5 · 三段发布协议会被绕过（正确性）

**症状**：`publish_baseline` 靠「有没有 pending marker」决定要不要立刻激活 locator。两条绕过路径：

1. 只有基线路径会调 `begin_baseline_locator_publication`。独立跑 `sync_pdms` 全量同步时没有 marker，于是 **DuckLake 一提交完就立刻激活 locator**，水位尚未推进，中间态直接暴露给读侧。
2. marker 是单个全局文件。多 dbnum 连续发布时，`marker.dbnum != 当前 dbnum` 恒成立，同样激活。

**当前暴露面**：`DbOption.toml` 现为 `total_sync = false`，独立全量同步不是活跃路径，所以实际风险低于理论风险。但这是配置层面的偶然安全，不是设计层面的保证。

**改法**：

- 激活与否不再依赖 marker 是否存在；所有发布入口一律走 `begin → bind → finish`，`publish_baseline` 默认 `activate = false`，激活只由 `finish_locator_publication` 触发。
- marker 改为 per-dbnum（`locator_publication/{dbnum}.json`），或改为可容纳多条 pending 的结构。

**验收**：验收标准 (c)；新增测试——`sync_pdms` 路径发布后，在 `finish_locator_publication` 之前 `active_locator_hash()` 必须仍指向旧 locator。

### H6 · DuckLake 无任何维护，且拖慢 readiness

**症状**：每次增量提交都写新 parquet + delete 文件 + 一个新 snapshot，永不回收。这不只是磁盘增长——`current_release` 与 `release_snapshot` 都要扫 `hierarchy.snapshots()`，**readiness 的延迟随提交历史线性增长**，系统跑得越久越慢。

**根因**：DuckLake 的快照/文件生命周期从未纳入设计。

**改法（Oracle 修正过的版本）**：不能简单按时间过期，因为 `load_dbnum_at_snapshot` 是时间旅行读。需要先有 pin 机制：

- 生成任务开始时登记它引用的 `(release_id, snapshot_id)`，结束时释放。
- 只回收同时满足三条的快照：不是任何 dbnum 的当前 release、不在 pin 集合内、超过保留窗口（按数量或天数，取配置）。
- 回收动作：`ducklake_expire_snapshots` → `ducklake_cleanup_old_files`；`ducklake_merge_adjacent_files` 独立按碎片度触发。
- 与 M4 配套：`current_release` 不再依赖 `snapshots()` 之后，快照回收对 readiness 的影响面才收敛。

**验收**：`/health` 报出快照数与数据目录大小；跑一轮长时间增量后，快照数稳定在保留窗口内，`current_release` 的 P99 不随时间上升。

### H7 · 每次调用新建 DuckDB 连接并 ATTACH

**症状**：每个公开方法都 `connect()` + `initialize()`：新建 in-memory DuckDB、LOAD 扩展、ATTACH DuckLake catalog、跑一次 `information_schema` 探测。`current_release` 在 readiness 热路径上。

**改法**：连接池，checkout / return，归还前确认无活跃事务。

**明确不要**：

- 不要 `Arc<Mutex<Connection>>`——那只是把 C2 的瓶颈换个地方。
- 不要让池内连接长期持有事务——会冻结快照可见性。

**一致性说明**：连接缓存本身**不会**破坏「不得观察到混合 release」。真正危险的是「release 元数据读」与「行数据读」跨连接进行；`load_current_snapshot` 已经在单连接内完成这两步，改造时必须保持这一点。

**验收**：`current_release` 的单次耗时基准较改造前显著下降；`load_current_snapshot` 仍在单连接内解析 snapshot_id 并读行。

### H8 · locator pointer 与 release 无持久绑定（正确性）

**症状**：`LocatorPointerV1` 只存 `locator_hash`。「这个 locator 属于哪个 release / 哪个 snapshot」这条信息只在 `activate_published_locator` 校验的那一瞬间存在，不落盘。一旦 locator 文件被错误恢复（例如从备份里挑错了一个），系统没有任何手段发现 locator 与当前 DuckLake release 不配对。

**改法**：pointer 扩展为 `{schema_version, release_id, snapshot_id, locator_hash}`，并在服务启动时做一次对账：`active_locator.release_id` 必须等于该 dbnum 当前 release，且 `locator_hash` 必须等于 `release.locator_hash`，不符即 fail-closed。

**验收**：新增测试——手工把 active pointer 指向一个历史 release 的 locator，启动对账必须拒绝。

### H9 · `ordered_children` 不走跨库路由（正确性）

**症状**：`hierarchy_mem_query.rs:806-816` 的 `ordered_children(dbnum, owner)` 只查 `owner` 所在的那一个分片：

```rust
let shard = self.shard(dbnum).await?;
let mut children = fetch_children(&state.db, &[owner]).await?;
```

它**从不查 `external_owner_routes`**。于是一个属主在 A 库、子节点落在 B 库的元素，它的子节点在这条路径上直接消失。跨库的那套路由只实现在多属主变体 `children_for_located_owners` 里。

**影响面比看起来大**：`current_hierarchy.rs` 的 `current_children_pes` / `current_children_refnos` / `current_filter_children` 全部走 `mem_children_rows` → `ordered_children`，而生成侧的 `current_children_named_attmaps`（`fast_model/query.rs:31`）又建立在它上面。也就是说**生成路径上的取子节点是不跨库的**。

`ancestor_walk_crosses_shards_and_rejects_missing_intermediates` 正是在断言这条，目前失败——用例是对的，实现有缺口。

**改法**：`ordered_children` 委托给 `children_for_located_owners`，与多属主变体共用同一条跨库合并逻辑，避免两份实现再次分叉。

**风险**：这是生成热路径，改动会让每次取子多一次 locator 查询（`external_child_dbnums`）。应与 M1 的 locator 内存快照一起做，否则会把 `spawn_blocking` 的开销乘到每个节点上。

**验收**：上述用例转绿；补一条「属主与子节点分属两个 dbnum 时 `current_children_refnos` 必须返回该子节点」的测试。

### M1 · `locate_dbnum` 每次 `spawn_blocking`

`spawn_blocking` 适合毫秒到秒级阻塞，不适合几十微秒的 locator 查找。BFS 每层每个节点都会产生一个小任务（单次调用内虽有局部 memo，但跨调用无效）。

**改法**：把 locator 提升为进程内 `Arc<HashMap<u32, u32>>` 快照，查找退化为纯同步 `HashMap::get`；启动时与 release 变更时刷新。原 `LOCATOR_CACHE` 的 mtime+len 判新逻辑保留为刷新触发条件。

### M2 · `apply_mem_change_set` 不分块

大 change-set（例如整库重排产生数万行 upsert）会生成极大的查询串，带来解析内存峰值、单事务超时与错误定位困难。

**改法**：与冷加载对齐做分批构造。注意分批提交会破坏单次原子性——若 SurrealDB 内存引擎支持，优先保持单事务而只分批构造语句；确实要拆事务，则必须先确认「部分应用后驱逐分片」这条退路成立（当前失败即驱逐的语义下，它是成立的）。

### M3 · 测试构造器不做 CAS

`HierarchyMemService::new()` 以 `enforce_readiness = false` 构造，绕过 `base_release_id` 的 CAS 与 readiness 校验。测试因此覆盖不到 release 错配、stale shard 与 finalize 竞态。

**改法**：把默认改为 `true`，仅纯查询单测显式关闭；或改名为 `new_without_readiness_for_test` 让绕过在调用点可见。

### M4 · `current_release` 随提交历史退化

**症状**：`current_release` / `latest_release` 都 `JOIN hierarchy.snapshots() ORDER BY snapshot_id DESC`，代价随总提交数增长。它在 readiness、基线判据、kv-mem 冷加载三条路径上。

**Oracle 的改法与本地复核修正**：Oracle 建议给 `hierarchy_release` 加 `snapshot_id` 列。**本地复核不采纳**：`snapshot_id` 要等 `COMMIT` 之后 `last_committed_snapshot()` 才知道，写不进同一个事务；补一条 UPDATE 又会再产生一个快照，等于每个 release 占两个，反而加剧 H6。

**采用的改法**：给 `hierarchy_release` 加事务内自增的 `release_seq`（写入时取 `max(release_seq)+1`）。`current_release` 改成 `WHERE dbnum = ? ORDER BY release_seq DESC LIMIT 1`，完全不碰 `snapshots()`。当前这个 JOIN 同时承担了「排序」和「快照存在性校验」两件事，把它们拆开：存在性校验只保留在真正需要它的 `release_snapshot` 里。

**验收**：`current_release` 的执行计划不再包含 `snapshots()`；构造 1000 个历史 release 后其耗时不随之增长。

### M5 · 事务内 `COUNT(*)` 算 row_count

`count_dbnum` 是分区全量 `COUNT(*)`，每次增量提交都在事务内跑一遍，只为填 `hierarchy_release.row_count`。

**改法**：算术维护 `prev_row_count + 新增 - 实际删除`。注意**不能用 `deletes.len()`**——delete 列表里可能包含并不存在的 refno。`build_change_set` 已经查过 `preimages`，把「实际存在的删除数」一并带进 `HierarchyChangeSet` 即可；或在 apply 前做一次 `SELECT count(*) WHERE refno IN (...)`。

**约束**：`load_current_snapshot` 用 `rows.len() != release.row_count` 做完整性校验，这条不能弱化——算术维护出错必须能被它抓到，所以这条校验要保留并补测试。

### M6 · `build_change_set` 的 N+1 点查

逐 refno `query_refno`、逐 parent `query_refno` + `query_children`，每次都是一次 DuckDB 往返。

**改法**：批量拉取 `WHERE refno IN (...)` / `WHERE owner IN (...)`，在 Rust 侧建 map。

### M7 · `build_change_set` 丢弃 `prepare_change_locator` 结果

`:1150` 调用 `self.prepare_change_locator(&conn, &change)?` 后直接丢弃返回值。它不是纯死代码——内部跑了 `validate_change_overlay`，承担校验职责；但同时也跑了 C1 的全表扫描。而 `apply_change_set_inner` 稍后又完整跑一遍。

**改法**：把校验与 locator 构造拆成两个函数（`validate_change_set` / `prepare_locator`），`build_change_set` 只调前者。

### M8 · 逐行 DELETE

`apply_change_set_inner` 对 `deletes ∪ upserts` 的每个 refno 单独执行一次 DELETE。DuckLake 的删除会写 delete file，N 次单行删除会产生大量删除工件，远差于一次谓词删除。

**改法**：`DELETE ... WHERE dbnum = ? AND refno IN (...)`，refno 多时按 1000–5000 分块。

### M9 · `content_hash` 全量 JSON 序列化

`content_hash` 先 `serde_json::to_vec` 再 SHA-256。基线的 `change_hash` 是对全部 rows 求的，50 万行的库会在内存里先生成整份 JSON。

**改法与约束**：**`release_id` 是持久化的 ABI**——它写进 DuckLake `hierarchy_release`、写进 Surreal `dbnum_watermark.hierarchy_release_id`、还参与幂等短路判定。直接换编码会让所有历史 release 对不上。必须走版本化：引入 `hash_schema_version`，新 release 用流式规范编码（`serde_json::to_writer` 直接喂 hasher，或改 bincode/自定义稳定编码），旧 release 按旧算法校验。过渡期双写两个 id，确认无回归后再切主。**不要静默换算法。**

### M10 · locator 写入缺目录 fsync

`stage_locator` / `activate_locator` / `atomic_write_json` 都是「写临时文件 → `sync_all` → `rename`」，缺父目录 fsync。断电时 rename 可能丢失，即使文件内容已落盘。

**本地复核修正**：Oracle 只说「Windows 需要另方案」。实际情况是**Windows 上没有对应的目录 fsync 语义**（本项目就是 Windows 部署，扩展路径都硬编码了 `windows_amd64`）。因此可行的补法不是 fsync，而是**启动时对账**：用 publication marker + DuckLake 当前 release 反推应有的 active locator，发现不一致就修正。H8 的启动对账正好可以覆盖这条，两项合并实现。

### L1 · `clear()` 末尾无锁清空

`clear()` 先逐个 gate 驱逐，最后再 `shards.write().await.clear()`（`hierarchy_mem_query.rs:188`），第二步不持任何 load gate。理论窗口下，gate 释放后新完成的冷加载会被这次 clear 抹掉。后果只是下次重新加载，不是数据错误，优先级最低。

**改法**：持全部 gate 完成驱逐后不再额外 clear；或引入 generation epoch，冷加载发布时校验 epoch。

### L2 · locator 工件与缓存都不回收

`locators/{hash}.json` 一个 release 一份，永不删除；每份含全项目 ref0 映射 + `external_owner_routes`。`LOCATOR_CACHE` 是进程级 `HashMap<PathBuf, _>`，读过多少个历史 locator 就留多少份。

**改法**：GC 保留「所有仍在 `hierarchy_release` 中的 `locator_hash` + active + pending」，其余删除，与 H6 的快照回收同一个维护任务。`LOCATOR_CACHE` 加 LRU 上界。

### L3 · DuckDB 无资源上限

`connect()` 不设 `memory_limit` / `threads`。DuckDB 默认吃到系统内存的 ~80%、用满所有核，而这个进程同时还跑着 SurrealDB kv-mem 分片和 OCCT 几何内核。

**改法**：`ATTACH` 后 `SET memory_limit` / `SET threads`，值随部署配置。**在 C1 修完之前，这是唯一的 OOM 防线**，因此实际优先级高于它的严重度分级。

### L4 · 扩展路径硬编码平台

`ducklake_extension` 搜索路径写死 `windows_amd64`，回退项用 CWD 相对路径。

**改法**：按 `cfg!(target_os)` / `cfg!(target_arch)` 组 platform triple；相对路径改为相对可执行文件位置而非 CWD。

### L5 · 基线发布全项目串行

`initialize_dbnum_baseline` 持有 `hierarchy_publication_guard()` 横跨整个全量解析。发布必须串行是设计意图，但把**解析**也圈进锁里，意味着建基线在项目内是严格串行的：一个大库解析慢，其余所有库的基线全排在后面。库数量一多就是明显的吞吐天花板。

**改法**：把锁的范围收缩到「发布 + 收口」，解析阶段放在锁外；需要重新论证的是解析结果与 release 的配对（解析完再取锁时，要确认基线判据仍成立，不成立则重来）。本项列为观察项，等 H6/M4 落地、拿到真实的基线耗时分布后再决定是否动。

## 4. 分期

**阶段 0 · 插队项**（新增；这三项一条决定规模上限、两条是正确性）

- C2 的第一步：`PROJECTION_GATE` 从 poison 恢复（单点改动，可独立合入）。
- H5 三段发布强制化 + marker 改 per-dbnum。
- C1 `external_owner_routes` 增量化 + debug-only 对账校验器。
- C2 的第二步：读路径去 gate、写锁收缩。
- L3 DuckDB `memory_limit` / `threads`（顺带做，C1 落地前的 OOM 防线）。

**阶段 1 · 低风险单点**（原首版阶段 1，顺序不变）

- ~~H3 的观测部分~~ **（2026-07-30 已完成）** `HierarchyMemStats`（常驻分片数 / 行数 / 内容字节 / 每片空闲秒数）已接进 `/api/v1/health` 的 `hierarchy_mem` 字段。
- ~~H3 的空闲回收~~ **（2026-07-30 已完成）** `evict_idle()` + 后台清扫任务，阈值 `AIOS_HIERARCHY_SHARD_IDLE_TTL_SECS`（默认 1800 秒，设 0 关闭），`with_idle_ttl()` 提供显式入口。**只驱逐拿得到 load gate 的分片**，因此 loading / syncing 中的一律跳过；被驱逐的分片下次查询照常冷加载，语义等同已有的失败驱逐。三条守护测试：正常驱逐后重载一致、持 gate 时跳过、关闭时常驻。
  > 关于 TTL 的定位：它是**回收策略，不是有效性策略**。分片能不能用永远由 `release_id` 的 CAS 决定。按时间判有效性会重新打开混合 release 窗口——release 何时变与时间无关，TTL 窗口内读到的旧 release 照样是错的，窗口外重读的又往往仍然有效。两头不讨好，所以没有做成 entry expiry。
- H4 驱逐失败统一为 warning。
- M3 测试构造器默认开启 readiness。
- DuckLake 侧观测：快照数与数据目录大小，同样接进 `/health`（H6 定阈值的前置）。
- **先让 §2.2.1 那六条失败用例转绿**，再动 C1。

**阶段 2 · 权威层收敛**

- M4 `release_seq` 排序，`current_release` 脱离 `snapshots()`。
- H6 快照 pin + 过期 + 文件合并；L2 locator 工件 GC 并入同一维护任务。
- H8 pointer 扩展 + 启动对账；M10 合并实现。
- H7 DuckDB 连接池。
- M1 locator 内存快照。

**阶段 3 · 热路径改造**

- H1 `HierarchyReadSession`，并改造 `current_hierarchy.rs` 的调用点。
- H2 `has_children` 物化。
- 阶段 1 拿到的常驻量级数据回填，定 H3 的驱逐阈值与策略。

**阶段 4 · 规模验证后再评估**

- M5 row_count 算术维护、M6 批量点查、M7 校验与 locator 构造拆分、M8 批量 DELETE。
- M9 `content_hash` 流式化（需 hash schema 版本化，单独评审）。
- M2 change-set 分块（等阶段 3 拿到真实 change-set 规模分布再定）。
- L1 `clear()` epoch、L4 平台三元组、L5 基线并行化。
- 是否把 kv-mem 换成原生 Rust 结构（见 §5）。

## 5. 明确不做（本期）

- **不替换 SurrealDB kv-mem。** 该层实际只需要三种访问模式（refno 点查、owner 取子、noun 过滤），用完整嵌入式数据库承载确有过度之嫌；但迁移要重写 `fetch_many` / 子查询 / noun 查询 / BFS / change 应用五处，收益要等阶段 1 的常驻量级数据出来才能定量。真正的架构价值在 DuckLake、change-set、`release_id`、readiness 这四样上，它们无论是否迁移都保留，因此迁移边界很干净、可以推迟。触发条件：dbnum 过百、常驻行数上千万、或 RSS 成为实测瓶颈。
- **不推翻 DuckLake + kv-mem 两层结构。** 第二轮复核（含 Oracle 独立意见）的结论一致：边界本身是对的——DuckLake 提供快照隔离、时间旅行、持久 `release_id`，kv-mem 提供可丢弃的热遍历加速。当前的病症不是分层错了，而是**DuckLake 被当成 OLTP 库在用**：事务内 `COUNT(*)`、N+1 点查、全表扫描重建索引，这三件事都不该在列存湖上做。C1 / M5 / M6 / M8 正是把这三件事拿掉。
- **不动 `MemRecord` 里 refno / owner 的 String 表示。** 冗余是真的（refno 同时以 String 和 ref0/ref1 双存），但要改就得连带改 `fetch_many`、`into_row`、重复检查与排序，风险大于收益；若将来迁移原生结构，这个问题自然消失。
- **不引入 release epoch 推送与 TTL readiness 缓存**，理由见 H1。
- **不放开 `GLOBAL_HIERARCHY_MEM` 的单 project 约束。** 一个进程绑定一个 project 与当前服务身份模型一致；真要支持同进程多 project，需要的是 `OnceCell<HashMap<Project, Service>>` 加一整套隔离，不是简单放开。
- **不静默更换 `release_id` 的哈希编码**，理由见 M9。

## 6. 风险与回归

- **C1 的增量维护是本方案最大的新一致性面。** locator 决定跨库 owner 路由，增量漏算会让模型树在跨库边界处静默断链，而且不会报错。对账校验器不是可选项，必须与实现同批合入，并在 CI 里对每个 change-set 测试用例都跑一遍。
- **C2 拆锁后要重新论证 `load_current_snapshot` 的一致性。** 现有注释把正确性归因于全局锁，实际来源是单连接内的快照隔离。去锁前必须确认 DuckLake `AT (VERSION => n)` 读不受并发提交影响，否则会把一个隐式保证变成真漏洞。
- **H5 改造会改变 `sync_pdms` 的可见行为**：全量同步跑完后 locator 不再立刻生效，必须等收口。任何依赖「跑完 sync_pdms 就能查」的脚本或运维习惯都会失效，需要同步更新 runbook。
- **H6 的快照回收依赖 pin 机制先就位。** 先做回收后做 pin 会直接打断正在跑的生成任务的时间旅行读。顺序不能反。
- **H1 的读锁持有时长**：会话期间增量同步拿不到写锁。必须给会话加最长持有时长的告警，并确认没有任何长任务（全量生成、房间重建）会开一个跨越整轮的会话。
- **H2 的 `has_children` 增量维护**是另一个新的一致性面：change-set 删除或新增子节点时必须同步翻转父节点标志，漏掉就会让 `refnos_by_nouns` 静默少报。测试必须覆盖「删最后一个子节点」和「给叶子加第一个子节点」两个方向。
- **M5 不得弱化 `load_current_snapshot` 的 row_count 校验**——它是算术维护出错时唯一的兜底。
- **H3 的驱逐**不引入新正确性面（等价于已有的失败驱逐路径），但会把冷加载延迟推到查询侧，需要观察 P99。
- 阶段 1 的三项都不改变任何查询语义，可以独立合入并单独回归。阶段 0 里只有「poison 恢复」和 L3 具备同样性质，其余三项都需要成组回归。
