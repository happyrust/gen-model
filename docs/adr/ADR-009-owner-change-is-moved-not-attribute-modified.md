# ADR-009：OWNER 变化走 Moved（elementIncluded）语义，不按普通属性修改处理

状态：已接受（2026-08-18：primaryList 权威快照已接入）
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
2. `primaryList` 不在普通 dabacon 属性字典；core.dll 的真实读取链是
   `DB_Noun::primaryList` → `ReadDataDab` →
   `db_get_element_info(noun_hash, 297853135)`，并以 `value == 1` 判真。
   2026-08-18 从已初始化的 E3D 3.1 进程直接调用同一导出函数，冻结
   `tests/fixtures/core-primary-list-e3d31.json`：1931 个 noun 中 1879 个有权威值
   （true=1142 / false=737），52 个读取失败显式列入 `unknown`。生产
   `primary_list_hint` 对已解析 noun 使用快照值；只对未知 noun 保守返回 `true`，
   绝不把「读取失败」混成 `false`。采集脚本为
   `scripts/e3d/dump_core_primary_list.py`，快照钉住 core.dll SHA-256
   `e4600d050a908f281d207bad52507dcbb82d4d8036c8d4d71e6e72eb290476d8`。
   门控机制仍由 `classify_children_delta_gated` 提供（B-EVT-03）。
3. 净变化折叠（「新建后搬迁 = 净 Created 而非 Moved」，对齐 `elementIncluded` 的
   `isElementCreated` 分支）由 `manual_update::fold_net_op` 在窗口层处理，
   不在单操作层（B-EVT-05/06）。

## 结果 / 约束

- 旧 owner 侧不再漏刷新（v1 口径下搬迁只会刷新新 owner 一侧，G1 缺口关闭）。
- 验收挂钩：批次 B 单测 B-EVT-01…07（`model_impact.rs`），全部对齐
  `.ida_scratch/analysis/db_userchanges.c` 的取证。
- 已解析的 737 个非 primaryList 类型不再多算成员事件；52 个未知类型仍保留
  `true` 的宁多勿漏偏差。该门只影响 DB_UserChanges 事件标签，不改变净窗口三态收集、
  `children_changed` 持久化或模型 Regen 判定。
