# ADR-005：解析改用 pdms-io-fork 的「索引定位」建表（会话 B-tree 索引优先）

状态：已接受
日期：2026-07-23
关联：ADR-004（按需解析元件库 CATA 移植）——本 ADR 是其自然续作；参考 `../../pdms-io-fork/crates/parse_pdms_db`（`src/refno_index.rs`、`src/parse.rs::gen_ref_type_pos_table`）——**这条同级检出本机已不存在**（`D:\work\plant-code\pdms-io-fork` 落空；对照 ADR-004 同写法的 `../../plant-model-gen` 是能落地的，所以这里不是写法问题），该 crate 今天的权威位置是 `Cargo.toml:155` 钉的 `old-parse-pdms-db.git @ 005936a6`，而那一行本身正要在退役收口 B9 里摘掉。当年点名改动的 `vendor/aios-parse-pdms/src/{parse,refno_index}.rs` **已从关联清单摘除**，理由与 ADR-004 同：该目录不被任何 manifest 引用、`git ls-files vendor/*` 也是 0 条，只是本机残留的旧解析器副本，拿它当取证入口会指错人。今天的落点见文末「2026-09-04 · 取证引用重指」。

## 背景

ADR-004 落地的「按需解析 CATA」靠 `parse_file_db_basic_data` 建的 `refno_table_map`(refno→文件偏移) 定位单元素。但该表由 `gen_ref_type_pos_table` 构建，其现实现是**对每个 ref0 反向全缓冲 `rfind`**（≈O(n_ref0 × 文件长)）——即便只想定位少量 refno，也得先把整库扫一遍建全表，与「按需 / 部分解析」的初衷相悖，是当前按需路径的主要成本。

`pdms-io-fork/crates/parse_pdms_db` 已重构解析：E3D db 文件的「最新会话」本身带一棵 **B-tree 索引**（refno→记录页 / 偏移）。fork 新增 `refno_index.rs`：

- `find_refno_entry(input, target)`：走索引 **O(log n)** 单点定位一个 refno；
- `gen_ref_type_pos_table_from_index(input)`：遍历索引叶子**直接建表**，免全文件扫描；

并把 `gen_ref_type_pos_table` 改为**索引优先、解码失败回退扫描**。

本仓当前 pin `aios_core` = git `rev 5667b70`、`nom 7`；fork 的 crate 则改用本地 `rs-core`(v0.3.2) 路径 + `nom 8` + rkyv/glam。整库替换会波及 gen-model 整棵依赖树。

> 以上是 2026-07-23 当天的地基。**本 ADR 通篇讲的那套建表机制（`parse_file_db_basic_data` / `refno_table_map` /
> `gen_ref_type_pos_table` / `find_refno_entry` / `refno_index.rs`）2026-09-04 已随 `parse_pdms_db` 整体退出本仓**
> （退役收口方案 B8.2–B9）。这不同于 ADR-004 只是「举证的代码位置搬了家」——这里是**被作证的那个机制本身**
> 换了实现方。逐条落点见文末「2026-09-04 · 取证引用重指」；**决策 Q1 / Q2 与决策 A 的结论没有被推翻，
> 反而变成了无条件成立**，见该节末尾。

## 决策（grill Q1–Q2）

| # | 决策 | 结论 |
|---|------|------|
| Q1 | 对齐范围 | **B｜把 fork 的「索引定位」移植进当前 vendor 库**（不整库替换 crate、不动 aios_core / nom） |
| Q2 | 落地深度 | **B1｜最小忠实移植**：只移植 `refno_index.rs` + `gen_ref_type_pos_table` 改索引优先；**回退扫描保留 vendor 现有实现不动**（当正确性基准 oracle） |
| — | 调用点 | **零改动**：`parse_file_db_basic_data` / `cata_closure` / `database.rs` 透明受益 |
| — | 开关 | **不加环境开关**：索引解码失败自动回退扫描（零回归） |
| — | 正确性证据 | probe 在真库断言「索引建表 == 现有扫描建表」（refno 集 + pos + noun_hash 全等）+ `cargo check --lib` |

## 关键取舍（Considered Options）

- **范围 B vs A（整库替换 crate）**：A 最彻底（gen-model 与 fork 共用一套解析），但要把 gen-model 整棵依赖树从 `aios_core rev 5667b70` / nom7 迁到 `rs-core v0.3.2` / nom8 + rkyv/glam，依赖地狱风险高、与本次「解析提速」目标不成比例。B 用最小面拿到核心收益。
- **深度 B1 vs B2 / B3**：B2（回退扫描也换 fork 单趟 O(len)）/ B3（cata_closure 惰性单点改 `find_refno_entry`）都是**严格增量优化**，但 B1 保留 vendor 现有扫描不动，使其成为**可信正确性基准**（索引结果与之逐位对齐即证毕），改动面最小、零回归。B2 / B3 留作后续可选项。
- **开关 vs 无开关**：无开关（选）——索引 `Option` 语义天然回退，无需灰度；保留 vendor 扫描即天然「关闭」路径。

## 后果（Consequences）

- 新增 `vendor/aios-parse-pdms/src/refno_index.rs`（自 fork 逐字移植，按本仓 `aios_core rev` 适配编译）；`lib.rs` 加 `pub mod refno_index;`；`parse.rs::gen_ref_type_pos_table` 改索引优先 + 保留原实现为 `gen_ref_type_pos_table_scan` 回退；导出 `find_refno_entry` 备用。
- **语义红线**：只改「refno→偏移表怎么建」，不改建表结果与后续解析；索引表必须与扫描表逐元素一致（`pos` 取最新会话记录、`world_refno` 一致），由 probe 把关。
- 收益：`parse_file_db_basic_data` 及所有建全表处从 O(n_ref0 × len) 降到 ~O(entries · log n)；按需 CATA 打开会话即受益。
- 风险：R1 fork 的 `refno_index.rs` 用到 `RefU64::{get_1, from_two_nums}` / `RefU64::from(&[u8])`，需确认本仓 pin 的 aios_core rev 具备（rs-core 路径版已具备）；缺失则就地按字节适配。R2 个别库无 B-tree 索引 / 损坏 → 自动回退扫描，probe 覆盖。

## 修订（决策 A：实施与验证，2026-07-23 落地）

**实施**（均 `cargo check` EXIT=0 + rustfmt）：`vendor/aios-parse-pdms/src/refno_index.rs`（自 fork 逐字移植，无需按字节适配——本仓 pin 的 `aios_core rev 5667b70` 已具备 `RefU64::{get_0, get_1, from_two_nums}` / `From<&[u8]>` / `EleDataEntry`）；`lib.rs` `pub mod refno_index;` + `pub use {find_refno_entry, gen_ref_type_pos_table_from_index}`；`parse.rs` `gen_ref_type_pos_table` 改索引优先、原实现更名 `gen_ref_type_pos_table_scan`；新增 `src/bin/refno_index_probe.rs`。

**关键发现（修正 B1 的「index==scan」假设）**：索引与扫描**语义本就不同**——索引 = 最新会话 B-tree 的**存活集**；扫描 = 全文件**所有物理记录**（含已删 / 旧会话 / 被覆盖）。真库 probe：

- **干净库（CATA / 单会话）逐元素完全一致**且索引更快：`ams251000`(967=967)、`ams251001`(2950=2950)。
- **编辑库** index<scan：`ams8000`(DESI) 索引 14269 / 扫描 30692；`ams1112`(42 万) 索引 422107 / 扫描 424342。

**决策**：Q2 的「index==scan 逐元素」验收**不成立**，改采**决策 A｜全局索引优先、与 fork 对齐**。索引=最新会话存活集，即 E3D 当前态的权威口径。

**交叉验证（独立单点 `find_refno_entry` 证移植正确、索引更权威）**：

- `ams8000`：scan-only=16514，抽样 200 **全部** `find_refno_entry`=None（确为已删 / 旧会话）；index-only=91（scan 的字节 heuristic **漏掉的活元素**，索引反而更全）；pos 不一致=0。
- `ams1112`：scan-only=2241，抽样全 None；pos 不一致=5，`find_refno_entry` **5/5 = 索引 pos**（scan 指向旧物理副本，索引才是当前记录）；index-only=6。
- 结论：索引建表与单点查询**自洽**；scan 会「多收已删 + 漏活元素 + 指向旧副本」⇒ **切换到索引是净正确性改进**，非回归。

**修订后的验收口径**（取代原「表逐元素相等」）：① 干净库 index==scan（已证）；② 编辑库差异经 `find_refno_entry` 证明为语义差异（scan-only 皆已删、pos 分歧 find 恒=索引、index-only 为 scan 漏的活元素）（已证）；③ **待活环境**：同批 DESI 开 / 关索引生成模型逐元素一致（几何 / inst hash，对齐 ADR-004 冒烟）。

**残留假设**：若别处代码**刻意**依赖扫描表里的历史 / 已删记录（如某 diff / history 特性），需单独确认；现有 `refno_table_map` 消费者（`parse_db_basic_data` 子树遍历、`cata_closure` 按 refno 查）均只需当前态，未见此依赖。

**children_map 结构门（无需 SurrealDB 的端到端结构证据）**：生成管线遍历的是 owner→children 树。用索引表 / 扫描表分别构建该树并比对（`refno_index_probe` ③）：

- 干净 CATA 库（ams251001）：完全一致。
- `ams8000`(DESI)：**仅扫描 owner=0、扫描被丢 child=0、索引找回 child=91**；91 个 index-only 均为真实元素（noun=0x4813573）。
- `ams1112`(42 万)：仅扫描 owner=0、扫描被丢 child=0、索引找回 child=6。
- 结论：children_map 上**索引 ⊇ 扫描**——索引只找回旧扫描 heuristic **漏掉的活元素**、从不丢件。故切换到索引对生成是「只多不少、只会更完整」、**不会缺件**（旧扫描反而有「漏活元素」的潜在 bug）。剩余的活环境几何冒烟仅需确认「新增件几何正确」这一层。

**被找回元素身份（noun 0x4813573 = SSREFE）**：`refno_index_probe` 对 index-only 元素取原始记录 + 试解析——记录合法（`00 00 00 11` 长度、ref0/ref1/noun 均正确），noun=**SSREFE**（规格选择引用类元素）；因当前 `all_attr_info.json` 未含 SSREFE，标准属性解析报 `SSREFE not exist in attr_info_map`（schema 完整性问题，与本改动正交）。即索引找回的是**真实存活的 SSREFE 引用元素**——旧扫描系统性漏掉了它们。SSREFE 属引用 / 规格类（非几何生成体），预计对最终几何**无影响**；生成期若遍历到其记录，行为与 fork 一致（fork 全局索引优先已长期处理该情形）。活环境几何冒烟仅需确认这一层。

**端到端安全（代码级确证，无需活环境）**：两条全解析路径——`parse_db`（`parse.rs:1065`）与 `parse_db_with_chunk_with_info`（`parse.rs:920`）——在解析单元素属性时均为 `if let Ok(ele) = parse_ele_data_with_info(...) { 入库 }`，**解析失败即跳过、不向上传播**（仅 world/根用 `?`，而根恒为 WORL、非 SSREFE）。故被索引找回的 SSREFE（无 schema、`parse_ele_data_with_info` 必 Err）在属性解析阶段被静默跳过：不进 `total_attr_map`、无属性、**不产生任何几何**。结论：纳入 SSREFE 只提升 refno 表 / children 树的「结构正确性」，对**最终几何输出为 no-op**，且不会让生成报错中断。至此「切换到索引安全」在 **parse 层 + 结构层 + 生成层三层均为代码级确证**；活环境几何冒烟仅作经验复核（非必需）。

## 2026-09-04 · 取证引用重指（`parse_pdms_db` 退役）

**改的是指针，不是决策；状态行按用户口径不动。** `parse_pdms_db` 整体退出本仓（退役收口方案 B8.2–B9，Q1 = A
「整体删」，用户 2026-09-04 08:44 拍板），数据面的解析底座换成 e3d-io。本 ADR 与同批重指的 ADR-004 /
ADR-021 / `docs/plans/on-demand-cata-parsing-port.md` / 数据解析流程图不同的地方在于：那几处丢的是**举证的代码
位置**，本 ADR 丢的是**被作证的那套机制本身**——它当年优化的对象（「怎么建 refno→偏移全表」）今天已经不存在
这一步了。下表逐条给落点。

| 正文里的名字 | 今天（2026-09-04） |
|---|---|
| `parse_file_db_basic_data` / `refno_table_map` | **无对应物，是被取消而不是被改名。** 不再有「整文件读进内存 + 建 refno→偏移全表」这一步：`frozen_source::FrozenSource::capture` 取 dbnum / 库类型 / 目标会话，`data_interface/e3d_collector.rs::stream_baseline` 按目标会话索引分块枚举，块交出去就释放 |
| `gen_ref_type_pos_table` / `_scan` / `_from_index`（本 ADR 的改造对象） | 三个都没了。「遍历索引叶子直接建表」这件事今天由 `e3d_collector::enumerate_target_index` 做，且**只许有一份实现**——源码钉 `整棵索引的 IndexCursor::at_root(..).walk() 只许出现在 enumerate_target_index 一处`。基线与净窗口共用它（`the_baseline_and_the_net_window_share_one_enumeration`） |
| `find_refno_entry`（单点 O(log n) 定位） | `e3d_collector::read_element_at(frozen, refno)`：`open_pinned_set` → `read_element`，与生产收集器同一条读法，不查库不动水位 |
| `parse_db_basic_info` | `data_interface/db_file_header.rs::DbBasicInfo::read`（同 ADR-004 重指表） |
| `vendor/aios-parse-pdms/src/{parse,refno_index}.rs` | 已从「关联」行摘除，理由见文首 |

**决策 A 没有被推翻，它变成了结构性事实。** 本 ADR 最有价值的那条发现——**索引 = 最新会话 B-tree 的存活集才是
E3D 当前态的权威口径，全文件扫描会「多收已删 + 漏活元素 + 指向旧物理副本」**——今天不再需要一个「索引优先、
解码失败回退扫描」的开关来表达：e3d-io 的读法**只有**会话索引这一条，生产路径里根本没有「全文件反向扫描建表」
这个回退分支了（`gen_ref_type_pos_table_scan` 在 `src/bin` 之外零出现；`mdb_membership.rs:251` 那处只是一句
「与旧读法整库 `gen_ref_type_pos_table` 同量级」的注释，不是调用）。也就是说 Q1 / Q2 当年选的
「索引优先」从一个**可回退的优化**升级成了**唯一的读法**。

**随之失效的两条尾巴**：① 「残留假设」（若别处刻意依赖扫描表里的历史 / 已删记录）——**问题消失**，扫描表这个
东西不存在了，需要历史态的地方走的是净窗口的 base / target 两端，不是同一个文件里的旧物理记录；②「验收口径」
第 ③ 条「待活环境：同批 DESI 开 / 关索引生成模型逐元素一致」——**开 / 关这个二元不存在了**，它的现代替身是退役
收口方案 B8.5 的真库外部裁决（5 库形状 diff、E3D TTY `Q ATT` / `Q MEMBERS` 抽样），那条仍然欠着。

**SSREFE 那一段**（索引找回旧扫描系统性漏掉的存活 SSREFE）作为**历史证据保留**：它当年证的是「切到索引只多不少、
不会缺件」，这个结论支撑了今天的底座选择；但它的取证工具 `refno_index_probe` 与结论里点到的 `parse_db` /
`parse_db_with_chunk_with_info` / `parse_ele_data_with_info` 都在 `parse_pdms_db` 那一侧，**读这一段时按 2026-07
的历史资料看，不要照着去仓里找**。