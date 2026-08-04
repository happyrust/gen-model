# 解析改用 fork「索引定位」建表（refno_index 移植）实施计划

依据：

- 本仓 `docs/adr/ADR-005-refno-index-parse-from-fork.md`（本次 grill 决策 Q1=B / Q2=B1）
- 参考 `../../pdms-io-fork/crates/parse_pdms_db/src/refno_index.rs`、`.../src/parse.rs::gen_ref_type_pos_table`
- 本仓 `vendor/aios-parse-pdms/src/{lib.rs,parse.rs}`、`src/data_interface/cata_closure.rs`、`src/versioned_db/database.rs`

## 决策速查表

| # | 决策点 | 结论 |
|---|------|------|
| Q1 | 范围 | B｜索引定位移植进 vendor 库（不整库替换 crate、不动 aios_core / nom） |
| Q2 | 深度 | B1｜最小忠实移植：refno_index + 索引优先建表；回退扫描保留 vendor 现状 |
| — | 开关 | 无；索引解码失败自动回退 |
| — | 调用点 | 零改动 |
| — | 证据 | 真库 probe「索引表 == 扫描表」+ `cargo check --lib` |

## 实施原则

- 零回归：索引解不出即回退 vendor 现有扫描，行为逐字节一致。
- vendor 现有 `gen_ref_type_pos_table`（rfind 扫描）原样保留、更名 `gen_ref_type_pos_table_scan` 作回退兼正确性 oracle。
- 逐字移植 fork `refno_index.rs`，仅按本仓 aios_core rev 适配编译（`RefU64` / `EleDataEntry` API）。
- 改动 Rust 文件跑 `cargo fmt` + `cargo check --lib`；遵循仓库既有 test 约定（不编译 test 目标，单测随源码留存）。

## 阶段

### 阶段 0：地基确认
状态：待办
- 确认本仓 `aios_core`(rev 5667b70) 的 `RefU64` 具备 `get_0 / get_1 / from_two_nums` 与 `From<&[u8]>`；`EleDataEntry{pos, noun_hash}` 字段齐备（parse.rs 已在用 `.pos` / `.noun_hash`）。缺失项就地按字节适配。
- 确认 vendor `parse.rs` 内 `EleDataEntry` / `RefU64` / `DashMap` 的 use 路径，供 refno_index 复用。

### 阶段 1：移植 refno_index.rs
状态：待办
- 新增 `vendor/aios-parse-pdms/src/refno_index.rs`（自 fork 逐字移植）：`find_refno_entry` + `gen_ref_type_pos_table_from_index` + 内部页解析（`detect_page_size` / `latest_index_root_pgno` / `parse_index_page` / `choose_child_pages` / `entry_from_loc` / `skip_record_padding` …）。
- `lib.rs` 加 `pub mod refno_index;`。

### 阶段 2：索引优先建表
状态：待办
- `parse.rs`：现有 `gen_ref_type_pos_table` 主体更名 `gen_ref_type_pos_table_scan`；新 `gen_ref_type_pos_table` = 先 `refno_index::gen_ref_type_pos_table_from_index(input)`，`None` 则回退 `gen_ref_type_pos_table_scan(input)`（对齐 fork 分派）。
- 导出 `find_refno_entry`（`pub use`）备后续单点定位用；本轮不接 cata_closure（B1）。

### 阶段 3：正确性 probe
状态：待办
- 新增 / 复用 `src/bin/` 下 probe（参考 `cata_parse_probe`）：对真库（AvevaMarineSample）分别用 `gen_ref_type_pos_table_from_index` 与 `gen_ref_type_pos_table_scan` 建表，断言：refno 集合相等、逐 refno `pos` 与 `noun_hash` 相等、`world_refno` 相等。
- 随源码留存 refno_index 自带单测（合成 B-tree 页）。

### 阶段 4：校验 + 收尾
状态：待办
- `cargo check --lib`（EXIT=0）+ 改动文件 `cargo fmt`。
- 真库 probe 跑通（索引 == 扫描）。

## 文件清单

- 新增：`vendor/aios-parse-pdms/src/refno_index.rs`。
- 改：`vendor/aios-parse-pdms/src/lib.rs`（+`pub mod refno_index;` / 可选 re-export）。
- 改：`vendor/aios-parse-pdms/src/parse.rs`（拆 scan + 索引优先分派）。
- 新增（可选）：`src/bin/refno_index_probe.rs`（索引 == 扫描 等价校验）。

## 验证

- `cargo check --lib`。
- 真库：索引表与扫描表逐元素一致；不一致直接定位发散 refno。
- 性能佐证：同库 `gen_ref_type_pos_table` 索引路径 vs 扫描路径耗时对比（日志）。

## 风险

- **R1** aios_core rev 缺 `from_two_nums` / `get_1` / `From<&[u8]>` → 就地按字节适配（低）。
- **R2** 库无 B-tree 索引 / 损坏页 → 自动回退扫描（probe 覆盖）。
- **R3** 索引「最新会话」根定位（0x28 / 0x40、page_size 探测）在本仓样本库的适配——probe 若整库回退即暴露，据此核对。

## Non-Goals（本轮不做）

- 整库替换 `parse_pdms_db` crate / 对齐 `aios_core`(rs-core) / nom8（范围 A）。
- 回退扫描换 fork 单趟 O(len)（B2）、cata_closure 惰性单点改 `find_refno_entry`（B3）。
- 移植 fork `parser/` 组合子重构（C）。

## 实施状态（2026-07-23 落地）

- **阶段 0–2 完成**：`refno_index.rs` 逐字移植（无需按字节适配——pin 的 `aios_core rev 5667b70` 已具 `RefU64::{get_0,get_1,from_two_nums}` / `From<&[u8]>` / `EleDataEntry`）；`lib.rs` 注册模块 + 再导出 `find_refno_entry` / `gen_ref_type_pos_table_from_index`；`parse.rs` `gen_ref_type_pos_table` 改索引优先、原实现更名 `gen_ref_type_pos_table_scan`。三处 `cargo check`（`-p parse_pdms_db` / `--lib` / `--bin refno_index_probe`）EXIT=0，rustfmt 干净。
- **阶段 3 完成**：`src/bin/refno_index_probe.rs`（建两表比对 + `find_refno_entry` 交叉验证）。
- **关键结论（详见 ADR-005 修订）**：index=最新会话存活集 vs scan=全物理，语义本就不同；干净 CATA 库逐元素一致（ams251000 / ams251001）、编辑库 index<scan（ams8000 14269/30692、ams1112 422107/424342）；交叉验证证明索引更权威（scan-only 抽样全不在 B-tree、pos 分歧 find 恒=索引、index-only 为 scan 漏的活元素）⇒ 采**决策 A 全局索引优先**。
- **children_map 结构门（已跑，无需 SurrealDB）**：索引表 / 扫描表分别建 owner→children 树比对——干净库完全一致；ams8000 索引找回 91 个 scan 漏掉的活 child、ams1112 找回 6 个，**仅扫描 owner=0 / 扫描被丢 child=0**（索引 ⊇ 扫描，只多不少、不缺件）。
- **待活环境**：DESI 主管线开 / 关索引生成模型逐元素一致冒烟（需 SurrealDB + 全量生成，对齐 ADR-004）。

真库 probe：`cargo run --bin refno_index_probe -- --file "D:\AVEVA\...\ams000\ams8000_0001"`