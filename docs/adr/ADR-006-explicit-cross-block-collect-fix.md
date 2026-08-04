# ADR-006：显式数据块跨块收集修复（CURD/DBLS）——移植 fork collect_explict_data

状态：已接受
日期：2026-07-24
关联：ADR-005（refno 索引解析对齐 fork）；`vendor/aios-parse-pdms/src/parse.rs`；参考 `../../pdms-io-fork/crates/parse_pdms_db`

## 背景

rs-plant3-d 加载 AvevaMarineSample 模型树为空。逐层排查发现第一层根因在解析器：

- SYS 元数据里的设计 MDB（如 `/MHULLFWD` / `/MHULLAFT`，noun=MDB）与 DB 元素带有 **CURD（当前数据库列表）/ DBLS** 这类**很长的引用列表属性**，其数据**跨多个记录块(block)**存储，块之间可能夹有其它块头或 `0x00 00 00 07` 追加段(continuation segment)。
- vendor 的 `collect_explict_data`（收集元素显式属性字节）在遇到「块头 flag!=1 / 块内 self-ref 与本元素 refno 不符 / 声明长度非法」时**直接 `break`**，导致跨块列表的后续块被丢弃；收集缓冲区正好停在 CURD 属性的 4 字节 hash 处 → `parse_raw_explicit_attrs` 读不到 8 字节头 → 报「显式属性退出」→ CURD/DBLS 属性丢失。
- 真库实测：amssys 里 `24575/1309`(MDB /MHULLFWD)、`24575/1478`(MDB /MHULLAFT)、`24575/1494` 等元素的 CURD/DBLS 反复解析失败。

fork（pdms-io-fork）已重构此段：遇不匹配块**按 word 对齐 resync** 继续寻找下一个匹配块（带 MAX_RESYNC 上限），并用 `collect_segmented_payload` 收集 `0x07` 追加段，主段起点用「可解析性」自适应判定(offset 12/20)；并配 `test_cases/test_amssys.rs`。

## 决策

按「最小忠实移植」（对齐 ADR-005 的 B1 范式）把 fork 的 `collect_explict_data` 逻辑移植进 vendor，用 vendor 现有 helper（`convert_to_hash` / `check_is_expr` / `parse_expression_attr_nom` / `get_explicit_attr_type` / `parse_to_i32` / `parse_to_u16`）适配：

1. 遇不匹配/非法块**不再 break，改 resync**（跳 4 字节继续找下一个 0x0001 块，MAX_RESYNC=64 防跑飞）。
2. 新增 `collect_explicit_segmented_payload`：主段 payload 从 offset 12 起，收集其后的 `0x07` 追加段（payload 从 offset 24 起）。
3. 主段 8 字节保留区**自适应裁剪**（全 0 → drain 8/12；否则用 `looks_like_attr_stream_start` 判断 offset 12 vs 20），兼容两种块布局、对普通元素与旧实现等价。
4. 旧实现原样保留为 `collect_explict_data_legacy`（`#[allow(dead_code)]`）作对照/兜底。

## 验证

- `cargo check -p parse_pdms_db` / `--bin curd_parse_probe` EXIT=0。
- 新增只读探针 `src/bin/curd_parse_probe.rs`：解析真实 amssys，确认 MDB `/MHULLAFT`/`/EQUIPMENTFWD` 现在解析出完整 CURD/DBLS 列表；覆盖统计 MDB=50 带 CURD=48、DB=110 带 STYP=110/110。
- 重解析 SYS 后日志「显式属性退出」= **0**（此前多次）。

## 结果 / 约束

- 修复了跨块引用列表属性（CURD/DBLS 等）解析；对普通单块元素与旧实现字节等价（回归面小）。
- 自适应 offset 判定存在极小概率误判（保留区恰好像属性流起点），由真库重解析的 pe 计数/noun 分布回归门把关。
- 本修复是模型树为空的**第一层**根因；第二层见 ADR-007。