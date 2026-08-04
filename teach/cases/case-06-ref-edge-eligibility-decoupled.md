# 案例 06 · 建边资格与效果分类解耦：NGMR / ORRF / VXREF

<sub>族 B 反向级联 · High · 已修 · 证据层 B（单测 + schema 全量核对）+ C（隔离实例实测）</sub>

## 一句话

「它是不是一条引用」和「改了它会产生什么效果」是两个正交问题，把建边资格挂在效果分类上，
就会让归在「直接几何」表里的引用属性**一条边都不建**。

## 现象

`ref_rev` 反向索引的建边准入原本是「效果分类 == `DependencyCascade`」。三条真实的 ELEMENT 引用属性
被归在 `DIRECT_GEOMETRY` 表里，于是：

- `NGMR`、`ORRF`、`VXREF` 指向的目标发生变化时，**引用者不级联**；
- 布尔运算结果、朝向结果保持陈旧，且无任何告警。

最能说明这是疏漏而非设计的是 `NGMR` 与同族 `GMRE`：后者在 CASCADE 表里正常建边，前者不建——
**一进一漏**。而 `ORRF` 挂在 143 个 noun 上，影响面最广。

## 证据

- 静态侧探针 `output/audit_ref_gap_probe.py`：用运行库 schema 全量核对
  **116 条 ELEMENT 引用 × 五张分类表**，找出「是 ELEMENT 引用但不在 CASCADE 表」的属性。
  注意判据不能简单写成「不在 CASCADE 表」——未列入任何表的引用会被 A2 元数据规则
  从 `Unknown` 升级为 `DependencyCascade`、**旧准入下本来就建边**。真正漏网的恰好是这三条。
- 动态侧探针 `output/ref_edge_delta_probe.py`：对库中实际存在的宿主表只发 `SELECT count()`，
  按 noun 统计非 nulref（`<表>:0_0`）的实例数。全程只读。

实测环境是专门新起的隔离环境（在跑的三个实例都不具备条件：`:8009` 是 memory 模式的合成小数据集、
`:8042` 被 e2e baseline 占用、`:8020` 的 ns 1516 是空的）——`empty1/ams-probe` 目录下单独一份
`DbOption.toml`，`gen_model` / `gen_mesh` 全关（只要属性数据），配独立 Surreal `:8043`，
用 `initialize_ams_dbnums` 走完整解析导入基线。两轮导入：

| 库 | 导入内容 | NGMR | ORRF | VXREF | 边增量 |
|---|---|---|---|---|---|
| AvevaMarineSample | 设计库（DESI） | 宿主表不存在 | 字段存在、值全空 | 宿主表不存在 | 0 |
| AvevaCatalogue | 16 个 CATA 库、约 26 万元素 | **2015**（JOIN 310 / SFIT 1679 / STCA 26） | 宿主表不存在 | 宿主表不存在 | **2015** |

## 根因

一个布尔判断被复用成了两个语义。`classify_attribute_effect` 回答的是
「改了它要做什么」（重生成 / 只更新变换 / 级联 / 跳过），而建边需要回答的是
「它指向另一个元素吗」。这两个问题的答案在大多数属性上恰好一致，于是被顺手合并——
直到出现「是引用、但改了它要直接重建几何」的属性为止。

## 修法

提交 `728a7123`。新增独立判据 `reference_edge_eligible`：

- 凡 schema 里 `att_type == ELEMENT` 的引用**都建边**，唯一排除 `OWNER`（所有权关系走 ownership 图，
  塞进来会让这张交叉引用表退化成「什么都有」的大杂烩，级联范围失控）；
- curated CASCADE 名单**兜底**，覆盖 `PRTREF` 这类 `att_type` 非 ELEMENT 的引用数组；
- **效果分类完全不动**——两件事从此各管各的。

行为增量经探针核实**恰为那三条漏网属性**，没有意外扩大。

## 验证

- 三条属性的建边钉子测试；
- `all_schema_element_refs_except_owner_are_edge_eligible`：全 schema ELEMENT 引用的建边资格扫描；
- 全量 `cargo test --lib`：181 passed / 0 failed / 38 ignored；
- 真实数据边增量 **2015 条**（见上表）。

## 规律

**两个判断恰好同时为真，不等于它们是同一个判断。** 复用一个已有的布尔量最省事，但一旦两个语义
在某个边缘 case 上分道扬镳，故障是**静默的**——没有报错，只有「本该级联的没级联」。
拆开的成本只是多一个函数，收益是每一侧都能被独立地全量核对。

**分侧分布会制造假阴性。** 这三个属性的宿主是分侧的：`NGMR` 纯目录侧（设计库里连宿主表都不会出现），
`ORRF` 的 144 个宿主 noun 是设计侧的（样例项目里字段建了但没填值）。只测设计库会得出
「增量为 0 → 这个修复是空转」的**误判**。凡是涉及跨库引用的验证，必须两侧都过一遍。

## 关联

- 案例 [05 共享 SPCO 反向传播](case-05-shared-spco-reverse-cascade.md)（这条索引是干什么用的）
- 案例 [03 分类名单单一事实源](case-03-attribute-effect-single-source.md)（同一轮审核里的另一处「两件事被绑在一起」）
- [`../../docs/2026-07-26_p3-t903-t904-assessment.md`](../../docs/2026-07-26_p3-t903-t904-assessment.md) 「实现审核与修复」一节
- 遗留：`VXREF` 仍未取到样本（宿主只有 `LOOPTS` 一个 noun，两侧都没有）；三个大 CATA 库
  （7000/7021/7320，合计约 1 GB）未导入。
