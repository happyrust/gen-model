# 会话上下文 — 2026-08-30 · direct 按参考号取属性 + Python 绑定测试

> 会话：BajieAsk-agent-1-1e0bfaa9（接力自 ZTEU/9b3a88c2）。前序工作见
> `会话-2026-08-29-模型生成重构审核.md`（审核 + 回写已完成）。

## 任务
用户问：「现在数据解析的接口，比如说获取某个参考号的属性数据是否已经能够直接使用了？」
并要求添加 Python 绑定来测试。

## 任务状态：进行中（方案已定，编码中）

## 对「是否已能直接使用」的结论（已核实）

**底层能力可用、已验证；正式门面（D1/D2 DirectMdb/DbElement）未开工。**

- 可用链路（ADR-053 P0 探针验证过，dbnum 8000/7333 共 200 样本 0 真值冲突）：
  `PdmsIO::open` → `init_ses_range_map` → `search_latest_refno(refno, sesno_pin)`
  → `parse_element(offset)`（全量属性解码 + SESNO 戳）→ `WholeAttMap::merge()`
  （常规 attmap 打底、显式属性补缺）→ `NamedAttrMap`。纯文件直读，不连库。
- Python 绑定现状：`aios_db.parse.element(path, refno, sesno=None)` 已存在，
  但走 `parse_raw_element`（原始 dump、不处理 UDA、serde tagged 值形态）。
  **缺生成期语义合并视图的绑定** —— 本次补 `aios_db.parse.attmap`。

## 关键事实（本次核实）

- `EleData.whole_attmap: WholeAttMap { attmap, explicit_attmap, uda_atts }`
  （aios_core::types::whole_attmap.rs）；`merge()` = attmap 克隆 + explicit 只补缺。
- `NamedAttrMap { #[serde(flatten)] map: BTreeMap<String, NamedAttrValue> }`；
  `NamedAttrValue` serde 为**外部标签**形态（untagged 被注释掉了）——16 个变体清单
  见 `src/bin/direct_attmap_probe.rs::canon`（穷尽匹配）。
- `parse_element`（io.rs L3002）内 `get_sesno(pgno)` 戳 SESNO，依赖
  `init_ses_range_map()`（probe 调了、旧 element 绑定没调）。
- 词属性在 direct 原始视图 = 词哈希整数；按 schema 反哈希对齐 DB 视图是 **D2
  同源转换器（Q4）的职责**，绑定不做第二实现（宪法 II，前一会话刚写进计划）。
- 离线测试夹具：`tests/fixtures/issues/issue-019-*` zip 内 db8000 三份快照
  （sesno 24/25/26；refs：zone=24384_24775、parent_equi=24384_24778(EQUI)、
  child=24384_24779(BOX)；25 删 child、26 删 parent）。
- `.pyi` 存根有看守测试（test_stubs_offline.py），加函数必须同步 parse.pyi。
- 构建：`python/` 下 maturin develop（README 记载 ~2min release，共享 target）；
  测试：`.venv\Scripts\python.exe -m pytest -m offline -q`。

## 改动方案（进行中）

| # | 文件 | 内容 | 状态 |
|---|---|---|---|
| 1 | `python/src/convert.rs` | `plain_attr_value`（16 变体→平面 JSON，refno 一律 a_b）+ `ele_data_to_merged_json` | 待做 |
| 2 | `python/src/lib.rs` | `parse.attmap(path, refno, sesno=None)`：探针同款链路 + block_on(parse_element) | 待做 |
| 3 | `python/pysrc/aios_db/parse.pyi` | attmap 存根 | 待做 |
| 4 | `python/tests/test_parse_offline.py` | attmap 用例：与 element 同源一致 / 平面值 / children 含子件 / 历史回放 / SESNO 戳 | 待做 |
| 5 | 验收 | cargo check → maturin develop → pytest -m offline | 待做 |

## 工作日志

- 00:52 收到任务；核实探针链路、python crate 结构、convert 层、离线夹具、.pyi 看守
- 01:0x 方案定稿：补 parse.attmap（生成期语义视图），不做词反哈希（留给 D2 转换器）
