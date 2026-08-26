# 024 分块解析基线：内存有界

- 依据：`docs/adr/ADR-042-bounded-baseline-parsing.md`
- 前置：ADR-004、ADR-017、ADR-021 §3、ADR-025、ADR-038、ADR-041

## 目标

**全量基线解析的常驻内存有界，且上限可配。** 不是提速——提速是 023，另算。

## 非目标

- **模型生成不在本轮**。批在 mem 内完成后直接写回，生成阶段下一轮插在两者之间。
- **不开 ADR-017 暂存窗口**（ADR-021 §3 不动）。只用 `mem://` 实例与读路由。
- **不做断点续批**，不新建任何持久表。
- **不改 `old-pdms-io`**。

## 现状（已核对）

| 事实 | 落点 |
| --- | --- |
| 基线整文件读进内存 | `DabaconSnapshot::read_full_basic_data` → `read_exact_prefix_from_opened_file(opened_len)` → `parse_db_basic_data`；`DbBasicData` 继续持有 `bytes` |
| 属性层已分批，旋钮默认等于不分批 | `versioned_db::database` 的 `chunk_size = sync_chunk_size.unwrap_or(10_0000)`，`all_refnos.chunks(chunk_size)` |
| 按需分页解析已落地且默认开启 | `data_interface::on_demand_db`：`ReadMode::configured()` 默认 `paged`；`PagedDbSession` 带页缓存/预取/逐项统计；`compare` 档自带对拍 |
| 单页随机读与记录物理位置 | `pdms_io::PdmsIO::read_index_data(pgno)`；B-tree 叶层 `(pgno, offset)` |
| 按任意根 refno 走 DESI 子树已存在 | `data_interface::cata_closure::collect_design_subtree_outbound`（部分解析，不整库解析；注释里举的例子就是 BRAN / PIPE / ZONE） |
| 基线不开窗口 | `batch_worker::execute_frozen_batch` 在 `!staged_shape \|\| reroutes_to_initial_load` 处返回执行体 |
| 完整性硬闸 | `manual_update` 的 `baseline_parse_matches`：`pe_count - root_count == parsed_count` |
| 中断基线的残留会被清 | `baseline_has_uncommitted_rows`：`pe_count > 0 && applied_sesno == 0` |

## 功能需求

**FR-1 分批遍历。** 基线解析从 WORL 出发沿 children BFS；走到配置列出的 noun 类型节点时
把该节点整棵子树连着走完，并优先在该处封批；批的实际封口由上限触发。不重不漏由
`visited` 保证。

**FR-2 分块类型可配。** 新增 `baseline_shard_noun_types`，默认 `["ZONE"]`，形状照
`generation_root.rs` 的 `delivery_unit_types`（整体替换 + `append_` 追加 + 非法值校验）。
**任何取值都不得导致漏解析或重复解析**——配置只影响批的形状。

**FR-3 批上限。** 复用 `ResourceGauge` 的字节 + 行数三档作为批上限器，mem 与 journal
两份分开计量。达到上限即封批。

**FR-4 每批装入 `mem://` 实例。** `connect("mem://")` → `use_ns/use_db` →
`init_staging_schema` → `StagingReadContext` → `with_staging_reads`。不登记 `REGISTRY`、
不建 commit token、不走 finalize plan。

**FR-5 整批写回后释放。** 批在 mem 内完成后，经 `StagedExecutor` 的 journal 与
`replay_journal_chunked` 整批写回持久库，确认后 DROP 该 mem 实例。
**批未完成时，持久库里不得出现这批的任何一行**（禁止解析时双写）。

**FR-6 索引对账与补链。** 走 B-tree 索引树（只读索引页）取全表 refno 清单，与遍历结果
对账；差集按元素自带 owner 补回，再跑 `member_prune::prune_resurrected_members`。
`authoritative_members` 的权威口径不变，取数改为 `OnDemandDbSession::parse_element`。

**FR-7 幽灵清理是硬闸。** 裁剪在所有批之后进行；被裁元素此时已在库中，必须在收口前删除。
删除未完成即不推进 `applied_sesno`、不转 published，并由独立回归钉住（不复用完整性闸
作为保险丝）。

**FR-8 失败整库重来。** 任一批失败即整个基线失败，不推进水位；恢复走现成清库重建路径。

## 非功能需求

**NFR-1 内存。** 峰值常驻不得随 dbnum 元素总数线性增长的部分只允许有：拓扑
（refno + owner + children）与索引对账所需的 refno 清单。属性层、文件字节、journal 文本
必须随批释放。

**NFR-2 等价性。** 与改动前的全量基线逐表等价：`pe`、noun 属性、`ATT_UDA`、`pe_owner`、
`dbnum_info`。`AIOS_PDMS_ON_DEMAND_READ_MODE=compare` 用作解析侧对拍。

**NFR-3 完整性闸不动。** `pe_count - root_count == parsed_count` 保持硬闸。

**NFR-4 可观测。** 每批输出：批序号、封批原因（上限 / 分块边界 / 遍历结束）、元素数、
mem 字节、journal 字节、写回块数、峰值 RSS。收口输出 `dropped_elements` 与索引对账差额。

## 验收

1. 同一个库，改动前后逐表等价（NFR-2 列出的五张表全表 digest 一致）。
2. 峰值 RSS 随 `baseline_shard_*` 上限单调变化；上限调到极小时仍能跑完。
3. 分块类型分别配 `["ZONE"]`、`["SITE"]`、`["EQUI"]`、`[]` 各跑一次，`pe_count` 完全一致
   （FR-2：配置只影响批的形状）。
4. 注入一条「记录不带成员块」的断链，索引对账必须发现并补回（FR-6）。
5. 注入一个已删子树（issue #10 形状），收口后库里查不到（FR-7）。
6. 中途 kill 进程：`applied_sesno` 未推进，重跑清库后结果与一次跑完等价（FR-8）。

## 开工前置

ADR-042 §12：先做 `sync_chunk_size` 两档对照 + 分段峰值 RSS 实测。若解析段峰值随小 chunk
应声而降，则大头在属性层，本 spec 退化为「改默认值 + 补一条配置项」，FR-1 / FR-4 / FR-5 /
FR-6 全部不做。
