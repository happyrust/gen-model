# ADR-004：DB_Noun 分类器数据——直接离线解析 dabacon 字典（不走运行期 dump）

状态：已接受（会话内据用户指示：改为「直接解析数据，不用 dump」）
日期：2026-07-23
关联：ADR-002；`teach/learning-records/0003`；`docs/plans/db-noun-classifier.md`

## 背景

要在 gen-model 实现与 core.dll 一致的类型分类器：`primitive`(#659518) / `geomset`(#859903) / `extrusion`(#663225) / `isPointsetPoint`(#290555737) / `graphicsBehaviour`(←5099119)（+ `hashValue` / `findNoun`），数据**以 core.dll 为准**。

实测（会话 `core31-retrace`）确定的可行性事实：

- 分类值来自 dabacon 数据字典的 per-noun 字段，运行期由 `internalGetField→sub_55BC98B((nounHash,fieldId))` 探**内存表** `dword_6C3F6C0[512*rec+field]`（+ 平行表 `6C08EE0/6C10EE0/6C18EE0`、计数 `6C3F6BC`、基址 `6C3F6B4`）。
- **这些表是 `.bss`（实测全 0、count=0），运行期加载**，不是 core.dll 里的静态常量 → **无法从二进制静态读取**。
- 填表的**加载器 = `sub_55F4290`**（0x55f4290, ~2.5KB，写入 count `6C3F6BC` 与各平行表）——即 dabacon 字典**文件格式的规格**。
- 分类 flag **不在** `all_attr_info.json`（它只是 `PdmsDatabaseInfo` 的 bincode 快照）。

## 决策

**直接离线解析 dabacon 字典 DB**（放弃运行期 dump）：

1. **数据源**：解析 E3D 字典 DB（dabacon），按 noun 提取分类 flag，产出 `noun_flags.json`。
2. **解析规格来自 `sub_55F4290`**：RE 该加载器读的字典文件 + 填表格式；**先验证**现有 dabacon 元素解析器能否直接读 DICT DB 的 noun 定义(UDET)记录（分类 flag = 这些记录上的属性值 659518/…），若能则大幅省力。
3. **验证（无 dump 黄金）**：与 gen-model 现有 curated 名单（`GNERAL_PRIM_NOUN_NAMES`/`GNERAL_LOOP_OWNER_NOUN_NAMES`/`TOTAL_LOOP/VERT_NOUN_NAMES`）交叉核对 + 已知 noun 抽查（SCYL=primitive、SCOM 系目录…）。

## 结果 / 约束

- 纯离线、无需活 E3D / dump；但**依赖 RE `sub_55F4290` + dabacon 字典文件格式**，是本方案主要成本/风险。
- 分类数据随 E3D 版本的字典而定，需记录来源字典版本。
- 若字典 DB 的 noun 定义可被现有元素解析器读取，则实现从"RE 引擎级加载器"降级为"复用现有解析 + 补 UDET 记录 schema"。
