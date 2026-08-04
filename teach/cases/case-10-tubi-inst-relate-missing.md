# 案例 10 · TUBI 的 aabb / world_trans 从未落 inst_relate

<sub>族 C 删除清理 · High · 已修 · 证据层 B+C（`ISSUE-001`）</sub>

## 一句话

两条数据流写着写着分了叉：普通元件走 `inst_info_map` → 建 `inst_relate` 记录，
TUBI 走 `inst_tubi_map` → **只收集 aabb 和 transform，从来不建记录**，于是那两个字段无处可存。

## 现象

- TUBI 元件无法正确显示；
- 基于包围盒的空间查询找不到 TUBI；
- `inst_relate` 表里 TUBI 记录的关键字段为空：

```sql
SELECT world_trans, aabb FROM inst_relate WHERE in = pe:tubi_refno;
-- 修复前：world_trans = none, aabb = none
```

## 证据

登记在 [`../../issues/ISSUE-001-TUBI-inst-relate-missing.md`](../../issues/ISSUE-001-TUBI-inst-relate-missing.md)。

数据流程对比：

| 元件类型 | 路径 | 结果 |
|---|---|---|
| 普通元件 | `inst_info_map` → 创建 `inst_relate` 记录 | ✅ aabb / world_trans 有处可落 |
| TUBI | `inst_tubi_map` → 只收集 aabb / transform | ❌ 无 `inst_relate` 记录 |

代码里其实留了自白：

```rust
// 更新 aabb 和 transform，保存 relate 已经在别的地方加了，这里后面需要重构
```

「已经在别的地方加了」这半句对普通元件成立、对 TUBI 不成立，而注释没有区分。

## 根因

TUBI 因为形态特殊（`BRAN` 下的管子，数量大、由路径生成）被单独开了一条收集通道，
但这条通道只搬运了「几何结果」（aabb、transform），没有搬运「实例身份」（`inst_relate` 记录）。
两条流程在收尾阶段没有汇合，缺口被一条 TODO 注释掩盖了。

## 修法

`src/fast_model/pdms_inst.rs` 的**单线程版**（`save_instance_data_single`）与
**并发版**（`save_instance_data`）各补一段：为 TUBI 构造 `inst_relate` 记录并批量落库。

```rust
let tubi_relate_sql = format!(
    "{{id: {}, in: {}, out: inst_info:⟨{}⟩, world_trans: trans:⟨{}⟩, aabb: aabb:⟨{}⟩, \
      generic: '{}', has_cata_neg: {}, solid: {}}}",
    k.to_inst_relate_key(), k.to_pe_key(), v.id_str(),
    transform_hash, aabb_hash, v.generic_type, v.has_cata_neg, v.is_solid,
);
let inst_relate_sql = format!("INSERT RELATION INTO inst_relate [{}];", chunk.join(","));
```

提交 `b338a6a`。

## 验证

专用工具 `src/bin/test_tubi_inst_relate.rs`（`cargo run --bin test_tubi_inst_relate`）：
查询指定 BRAN 下的 TUBI `inst_relate` 记录、验证 `world_trans` / `aabb` 字段、统计记录数。

```sql
-- 修复后
SELECT world_trans, aabb FROM inst_relate WHERE in = pe:tubi_refno;
-- world_trans = trans:⟨hash⟩, aabb = aabb:⟨hash⟩
```

## 这个案例与增量更新的关系

TUBI 本身不是独立最小交付单元（`BRAN` 下的 `TUBI`/`FTUB` 归一到 BRAN，见案例 04），
但它的 `inst_relate` 记录是**增量清理与刷新的抓手**：

- 删除清理（案例 08、09）按 `inst_relate` 级联删几何——没有记录就没有清理对象，删不掉也查不出来；
- `TransformOnly` 路径更新的是 `inst_relate.world_trans`——没有记录就无处可更新；
- 空间索引（AABB 树）从 `inst_relate.aabb` 建——没有记录就永远命不中。

也就是说，这个「保存 bug」在增量语境下会伪装成三种完全不同的症状。

## 规律

**为特例开的旁路，必须在收尾处与主路径汇合。** 一旦某类对象因为性能或形态被单独收集，
就要逐项检查主路径在收尾时做了哪些事——尤其是那些「建立身份」的写入。
只搬运数据、不搬运身份，数据就会落在没有归属的地方。

**「后面需要重构」的注释是缺陷登记，不是免责声明。** 这行注释准确描述了缺口的位置，
却因为没有变成 issue 而在库里放了很久。看到这类注释时，正确动作是当场判断它是否已经在造成后果。

## 关联

- [`../../issues/ISSUE-001-TUBI-inst-relate-missing.md`](../../issues/ISSUE-001-TUBI-inst-relate-missing.md) · `TUBI_inst_relate修复说明.md`
- 案例 [04 生成根归一](case-04-generation-root-must-be-one-rule.md)（TUBI 是字典级伪类型，不做交付单元）
- 案例 [08](case-08-deleted-element-orphan-geometry.md)、[09](case-09-cascade-delete-transaction.md)（清理为什么依赖 `inst_relate`）
