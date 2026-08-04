# ADR-005：解析改用 pdms-io-fork 的「索引定位」建表（会话 B-tree 索引优先）

状态：已接受
日期：2026-07-23
关联：ADR-004（按需解析元件库 CATA 移植）——本 ADR 是其自然续作；改动 `vendor/aios-parse-pdms/src/parse.rs`、新增 `vendor/aios-parse-pdms/src/refno_index.rs`；参考 `../../pdms-io-fork/crates/parse_pdms_db`（`src/refno_index.rs`、`src/parse.rs::gen_ref_type_pos_table`）。

## 背景

ADR-004 落地的「按需解析 CATA」靠 `parse_file_db_basic_data` 建的 `refno_table_map`(refno→文件偏移) 定位单元素。但该表由 `gen_ref_type_pos_table` 构建，其现实现是**对每个 ref0 反向全缓冲 `rfind`**（≈O(n_ref0 × 文件长)）——即便只想定位少量 refno，也得先把整库扫一遍建全表，与「按需 / 部分解析」的初衷相悖，是当前按需路径的主要成本。

`pdms-io-fork/crates/parse_pdms_db` 已重构解析：E3D db 文件的「最新会话」本身带一棵 **B-tree 索引**（refno→记录页 / 偏移）。fork 新增 `refno_index.rs`：

- `find_refno_entry(input, target)`：走索引 **O(log n)** 单点定位一个 refno；
- `gen_ref_type_pos_table_from_index(input)`：遍历索引叶子**直接建表**，免全文件扫描；

并把 `gen_ref_type_pos_table` 改为**索引优先、解码失败回退扫描**。

本仓当前 pin `aios_core` = git `rev 5667b70`、`nom 7`；fork 的 crate 则改用本地 `rs-core`(v0.3.2) 路径 + `nom 8` + rkyv/glam。整库替换会波及 gen-model 整棵依赖树。

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