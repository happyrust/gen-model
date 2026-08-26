# ADR-042 分块解析基线：按遍历分批装入 kv-mem，整批写回后释放

- 状态：已接受（2026-08-22）
- 关联：ADR-004（按需元件库解析）、ADR-017（暂存与写回）、ADR-021 §3（基线不走窗口协议）、
  ADR-025（严格初始化阶段）、ADR-038（有界写回）、ADR-041（并行生成与切片流水线）
- 取代：ADR-041 关于「ZONE 作为内存分片单位」的表述由本 ADR 细化并部分改写（见 §9）

## 背景

全量基线解析今天是「整文件塞进内存」：`DabaconSnapshot::read_full_basic_data` 一次
`read_exact_prefix_from_opened_file(opened_len)` 把冻结前缀读成一个 `Vec<u8>`，交给
`parse_db_basic_data` 建出 `DbBasicData`——而 `DbBasicData` 继续持有那份 `bytes`，外加
`refno_table_map`（refno → 记录位置）与 `children_map`（父子关系）。三样都是全量常驻，
整个基线期间不释放。

属性解码那一层**已经是分批的**：`versioned_db::database` 里
`all_refnos.chunks(chunk_size)` 逐块调 `parse_file_with_chunk` 解出 `total_attr_map`，
写完即丢。批大小旋钮是 `DbOption` 的 `sync_chunk_size`，但默认 `unwrap_or(10_0000)`
——对绝大多数 dbnum 等于不分批。那一行上方还留着一句 `//按照SITE划分？`。

所以基线解析的常驻内存只有两块：`DbBasicData`（无旋钮，全量）与属性层（有旋钮，
默认等于不分批）。

按需分页解析的能力本仓**已经具备**，只是基线没接：

- `data_interface::on_demand_db` 的 `ReadMode::configured()` 默认返回 `paged`，
  `PagedDbSession` 带页缓存、预取与逐项统计（`physical_pages_read` / `bytes_read` /
  `cache_hits` / `index_pages_read` / `record_pages_read` / `parsed_records`），
  还有 `compare` 档同时跑 legacy 与 paged 并断言相等；多 extent 文件自动回退 legacy。
- `pdms_io::PdmsIO::read_index_data(pgno)` 是 `seek + read_exact(PAGE_SIZE)` 的单页随机读，
  带 `index_page_cache`；B-tree 叶层条目的 `(pgno, offset)` 就是元素记录的物理位置。
- `data_interface::cata_closure::collect_design_subtree_outbound` 已经能「给定设计元素根
  refno（如 BRAN / PIPE / ZONE），在其所属 DESI 库内沿 children 做子树 BFS（部分解析，
  不整库解析）」——它今天只服务 CATA 闭包发现。

## 决策

### 1. 不改 `old-pdms-io`

按需分页解析已经落地且默认开启。本轮的差距在**基线没接这套**，不在解析器能力。

### 2. 用 `mem://` 实例 + 读路由，不上 ADR-017 提交协议

两者可分：

- **实例 + 读路由**：`connect("mem://")` → `use_ns/use_db` → `init_staging_schema` →
  `StagingReadContext::new(db, label)` → `with_staging_reads(ctx, fut)`。作用域内被接线
  的读自动打到该实例。
- **提交协议**：`REGISTRY` 登记、`ResourceGauge`、`commit_token`、finalize plan、receipt、
  水位尾事务、终态清扫。

`create_window_on` 一次性给两样，本 ADR 只要第一样。第二样被 ADR-021 §3 明令禁止用于
基线（「基线不走窗口协议，开了窗口也等不来 finalize plan」），且它买的是原子提交与崩溃
恢复，不是内存有界。

`ResourceGauge` 可单独取用作批上限器：它本来就分开记 `record_staged` 与 `record_journal`，
按字节与行数给出三档（告警 / 拒绝吸收 / 废弃）。

### 3. 批的边界由上限定，分块类型只决定遍历顺序与封批点

遍历从 WORL 出发沿 children BFS；走到配置列出的 noun 类型节点时，把该节点整棵子树连着
走完，并优先在该处封批；批的实际封口由上限触发。

**不重不漏由遍历的 `visited` 集合保证，与配置无关。** 配置项 `baseline_shard_noun_types`
默认 `["ZONE"]`，形状照 `generation_root.rs` 的 `delivery_unit_types`（整体替换或
`append_` 追加，并校验非法值）。这样配错只影响批的形状，不会丢数据。

被否决的写法是「配置列出的类型的子树就是全部分块」：任意 noun 不一定构成划分——不在
任何该类型节点下的元素会静默漏掉，两个会嵌套的类型会重复。

### 4. 补链轮保留，数据源换成 B-tree 索引

纯沿成员表遍历会在「选中的记录不带成员块」处断链，下面整棵真实子树静默丢失——补链轮
（`relink_children_by_owner`）当初就是为这个毛病写的。分片后按同一语义重建：

1. 走 B-tree 索引树（只读索引页，不读数据记录）拿全表 refno 清单；
2. 与遍历结果对账，差集按元素自带的 owner 补回；
3. 跑 `member_prune::prune_resurrected_members` 同一套裁剪。

`authoritative_members` 的权威口径（元素自己那条记录的成员块）不变，取数从整份 `bytes`
换成 `OnDemandDbSession::parse_element(refno).children`。

### 5. 拓扑全程常驻，只有属性按批释放

裁剪需要全局可达性，因此必须等所有批爬完才能做；拓扑（refno + owner + children）随之
全程常驻。属性层是内存大头，它按批释放。这是本方案能省下的东西的准确边界：

| 常驻块 | 分片后 |
| --- | --- |
| 整份文件字节 | **省掉** |
| `refno_table_map` / `children_map` | 仍全量常驻 |
| 属性层 | 按批释放（今天靠 `sync_chunk_size`，默认等于不分批） |

### 6. 幽灵允许中途进库，收口前删干净

今天裁剪发生在落库之前，幽灵从不进库。分片后裁剪被推到最后，被裁元素已经写进去了。
接受这个中间态（基线期间库不对外、水位未推进），并在收口前把 `dropped_elements` 从库里
删掉。

**删不干净就不推进 `applied_sesno`、不转 published，且必须显式钉一道**——不拿为别的事
写的完整性闸当保险丝。

### 7. 失败整库重来，零新持久表

任何一批失败即整个基线失败，不推进水位；下次由现成路径清库重建
（`baseline_has_uncommitted_rows` 已经在做「中断基线写入的行没有已提交水位，不能混进
下一次重放」的判定）。不做断点续批：它与第 5 条冲突（断点恢复仍要重建全量拓扑，省下的
只有属性解码那一半），且一分钱不买内存。

### 8. 原子性靠水位，不靠事务

批级单事务在 ADR-038 的上限下不可能（32 条 / 64 KiB / 250 行）。今天的「原子性」是
「写到一半崩了 → `applied_sesno` 没推进 → 那些行不算数 → 清库重来」，外部看到的效果是
全有或全无。第 7 条即该原子性的实现方式。

### 9. 分批买到的性质：批未完成时，持久库里没有这批的任何一行

这是分批相对于流式直写的实质区别，也是「解析时同时写 mem 与持久库」被否决的理由。

### 10. 写回走 journal + 分块重放

每批在 mem 内完成后，用 `StagedExecutor` 的 journal 与 `replay_journal_chunked` 整批写回
持久库：复用现成的 TX_CHUNK 分块、逐块序号与指纹、背压，以及 ADR-041 §6 的封本定序。

代价是每批多一份 SQL 文本常驻（`JournalEntry { sql: String, .. }`）。接受它，换
「写回卡住时能唯一定位到哪一块」——全量基线的写入量比增量窗口大一两个量级，卡死概率
只会更高（见 `docs/evidence/2026-08-19-db8000-staging-writeback-stall-fix.md`）。
`ResourceGauge` 能把 mem 与 journal 两份分开计量。

### 11. 完整性闸不动

`pe_count - root_count == parsed_count` 保持为硬闸。它能抓住分片重复（`parsed_count`
偏大）与收口删除失败（`pe_count` 偏大），抓不住分片漏解析（两边同时变小）——漏解析由
第 4 条的索引对账负责。

### 12. 先拧旋钮再开工

属性层已有 `sync_chunk_size` 旋钮。先做两档对照（默认 100000 vs 小值），同时采分段峰值
RSS：

- 解析段峰值应声而降 → 大头在属性层，改默认值即可，本 ADR 的实现全部不必做；
- 峰值不动 → 大头在 `DbBasicData`，本 ADR 成立，且省下的量就是那个差额。

本 ADR 的实现以该实测为前置。理由是第 4、5、6 条都在动 issue #10（已删子树整棵复活，
AMS 7999 的 `/1WCC-PIPEBJ` 出了两棵一样的子树）那块已修好的死角，拿未知收益换已知风险
不成立。

## 后果

- 基线解析路径从 `read_full_basic_data` 改走 `OnDemandDbSession`，`compare` 档可直接用于
  等价性验收。
- 生成阶段不在本 ADR 范围内。它接入的位置是「批在 mem 内完成 → 写回」之间；读路由的
  fail-closed 纪律（上下文在场时读只打暂存库，miss 原样上抛）意味着届时片内必须预载全部
  依赖闭包（跨片引用、CATA、祖先变换），`staging/preload.rs` 与 `ancestor_preload.rs`
  是现成入口。
- ADR-041 原文把 ZONE 记为「内存分片单位」，本 ADR 把它降为「遍历顺序与封批点的默认
  配置值」，并明确批边界由上限决定。ADR-041 §5「生产端禁止回读本次产物」不变——它在
  实现上正是读路由的 fail-closed 纪律。
- ADR-021 §3 不动：本方案不开暂存窗口，只用 `mem://` 实例。

## 被否决的选项

- **改 `old-pdms-io` 加按需入口**：能力已存在且默认开启。
- **纯按需、零预扫**（从 ZONE 记录出发递归下降，不建全量拓扑）：每个元素一次 B-tree 下降
  加一次随机 seek，无页局部性；且不先扫一遍就不知道有几个分块节点、各多大，批切不均匀，
  而「每批可控」正是目标。
- **mmap 替代整文件读**：常驻交给 OS 页缓存可回收，但索引仍全量建，「按需」是假的。
- **两阶段（先只走拓扑不落库，再按批解析属性）**：成员块与属性在同一条记录里，跨批后
  页缓存失效，等于一倍随机读。
- **按 refno 区间分批**：I/O 顺序、批大小精确，但批不是语义单元，生成阶段接不上。
- **解析时双写 mem 与持久库**：破坏第 9 条。
- **断点续批**：见第 7 条。
