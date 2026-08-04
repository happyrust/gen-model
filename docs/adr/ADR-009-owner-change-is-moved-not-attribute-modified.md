# ADR-009：OWNER 变化走 Moved（elementIncluded）语义，不按普通属性修改处理

状态：已接受（代码与批次 B 单测已落地，本文件补记决策）
日期：2026-07-26
关联：`docs/2026-07-25_test-plan-core-dll-model-update-complete-matrix-v2.md` §1（逆向证据）；
`src/data_interface/model_impact.rs`（`ChangeBucket` / `user_change_buckets` /
`classify_children_delta_gated` / `primary_list_hint`）；ADR-002（core.dll 权威范围）

## 背景

v1 计划把 `OWNER` 变化归入 `StructuralMembership` 普通属性处理。对 core.dll
`DB_DB::elementsChangedBetween`（`0x58ffc50`）的逆向复核推翻了这一口径：

- `OWNER` 属性变化**不走** `attributeModified`，而是先 `switchToOldSession` 读旧
  owner，再调 `elementIncluded(elem, oldOwner)`（`0x5987ea0`）——这是会话区间差分里
  表达「搬迁」的唯一手段，离线增量必须实现，不能记 N/A。
- `DB_UserChanges` 六个变化桶按对象偏移排列：Created(+0) / Deleted(+8) / Moved(+16) /
  MemberChanged(+24) / Reordered(+32) / Modified(+40)。写入规则（反汇编取证）：
  - `elementCreated`（`0x5987a90`）：元素记 Created，**其 owner 记 MemberChanged**；
  - `elementIncluded`：元素记 Moved，**旧、新两个 owner 都记 MemberChanged**；
    若新 owner 本身是本窗口新建（`isElementCreated` 分支），元素改记 Created；
  - `elementReordered`（`0x5988040`）：成员记 Reordered，owner 记 MemberChanged；
  - 成员表差分**仅当** `DB_Noun::primaryList(noun)` 为真才执行，顺序变化码固定为 `3`。

## 决策

1. 增量影响判定按 core.dll `DB_UserChanges` 写入语义建模（`ChangeBucket` 六桶 +
   `user_change_buckets` 纯函数）：
   - `Modified` 含 OWNER 变化 → `Moved(elem)` + `MemberChanged(旧 owner)` +
     `MemberChanged(新 owner)`；纯 OWNER 变化**不**记 `Modified` 桶（G1）。
   - `Add` → `Created(elem)` + `MemberChanged(owner)`（G2）。
   - 成员/顺序差分按 `primaryList` 门控（G3）：同集合换序 → `Reordered`，集合增删 →
     `MemberChanged`；两者都触发父生成根重生成，但事件类型必须可区分。
2. `primaryList` 不在 dabacon 字典（走 `db_get_element_info(hash, 297853135)`，
   A-DICT-01 已断言字典不可得），离线取不到值。`primary_list_hint` 当前对所有 noun
   返回保守值 `true`——宁多勿漏，绝不因门控丢成员变化；待接入活 E3D 名单（P8 一类）
   后改为数据驱动。门控**机制**由 `classify_children_delta_gated` 提供并可显式传
   `false` 验证（B-EVT-03）。
3. 净变化折叠（「新建后搬迁 = 净 Created 而非 Moved」，对齐 `elementIncluded` 的
   `isElementCreated` 分支）由 `manual_update::fold_net_op` 在窗口层处理，
   不在单操作层（B-EVT-05/06）。

## 结果 / 约束

- 旧 owner 侧不再漏刷新（v1 口径下搬迁只会刷新新 owner 一侧，G1 缺口关闭）。
- 验收挂钩：批次 B 单测 B-EVT-01…07（`model_impact.rs`），全部对齐
  `.ida_scratch/analysis/db_userchanges.c` 的取证。
- 保守 `primaryList=true` 的代价是对非 primaryList 类型多算成员事件（宁多勿漏方向，
  不产生正确性风险）；数据驱动名单落地前该偏差保留。
