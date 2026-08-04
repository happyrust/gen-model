# 容器位移与单元变换级联：排查、修正后的结论与修复（2026-08-04）

> **修订记录**：本文初版断言「执行链路缺容器变换级联、模型与数据分叉且无自愈出口」。
> 深入执行计划层与实测批次日志后，该断言**不成立**——执行路径早已通过
> `ModelWorkAction::Transform` 完整处理容器位姿变更。真正的缺陷是**预览/报告层
> 与执行计划层口径分歧**：预览把「执行阶段会正确处理的变更」报告成「跳过模型
> 生成」。本版为修正后的结论与已实施的修复。

## 一句话

ZONE/SITE 容器的 `POS`/`ORI` 变更在**执行阶段**由 `Transform` 工作项正确处理
（`update_world_transforms`：整棵子树的实例世界变换 + 包围盒 + 空间树 + 房间归属，
不重建网格）；但**预览**用未分区的净变更做交付单元 rollup，把同一变更计入
`no_generation` 并告警「跳过模型生成」，还会把成员级纯位姿变化错报成整单
`will_generate`。预览说的不是执行要做的。

## 实测证据（AMS，2026-08-04）

### 执行侧是好的（会话 35+36 批次日志，task db-20260804-171539-000000）

```
开始执行数据批次 dbnum=8000 sesno 35..=36
开始更新 1 个元素及其子树的world transform      ← Transform(ZONE 24384/22400)
子树中有inst_relate数据的节点数量: 67           ← 整棵 ZONE 子树的模型节点
执行world transform更新SQL，批次大小: 50 / 17
world transform更新完成
开始更新 1 个元素及其子树的world transform      ← Transform(BEND 24384/22456)
子树中有inst_relate数据的节点数量: 1
world transform更新完成
数据批次执行完毕 …（状态 succeeded）
```

- 没有任何 RegenRoot：两个工作项全是 `Transform`，BEND 9 的 +300mm 位移正是经
  这条便宜路径落库的（AABB z 从 [630,730] → [927,1030]，plant-ui 可见）。
- `update_world_transforms` 尾部显式接了 `update_inst_relate_aabbs_by_refnos_with_spatial_tree`
  （replace_exist=true）与 `enqueue_room_recalc`（ADR-010 §4），包围盒/空间树/房间
  归属都在便宜路径覆盖内（`increment_manager.rs:2108-2128`）。

### 预览侧是错的（会话 35 预览响应）

- `no_generation=1` + 告警「1 个变更无法解析合法生成根，跳过模型生成（样例:
  24384/22400）」——但执行阶段会为它建 Transform 工作项，**并不会跳过**；
- BRAN 单元因成员 BEND 的纯位姿变化被报 `will_generate=true`——但执行阶段
  只做 Transform(BEND)，**并不整单重生成**。两个方向都与执行不符。

## 根因

预览与计划各自分类：

| | 预览（修复前） | 执行计划 `build_model_update_plan` |
|---|---|---|
| rollup 输入 | **全量**净变更（`model_affecting` 含 TransformOnly） | `mask_details_to_regen` 后仅 Regen 类 |
| 纯位姿目标 | 无处安放 → 容器落 `no_generation`，成员误推 `will_generate` | `transform_refnos` → `ModelWorkAction::Transform` |

`generation_root.rs:217-220` 对容器自身返回 `None` 与「no whole-ZONE fallback」
决策本身都是对的——错在预览没有复刻计划的「位姿/重建」分区，把位姿目标硬塞进
只为重建设计的 rollup 口径里。

## 修复（本次已实施）

单一事实源 + 预览对齐（`model_update_plan.rs` / `manual_update.rs`）：

1. **抽出共享分区** `partition_operation_impacts(range_eles, details) ->
   { regen_refnos, transform_refnos }` 与 `mask_details_to_regen`（原是
   `build_model_update_plan` 的内联代码），执行计划改为调用同一函数；
2. **预览对齐**：`preview_one_dbnum` 的 DESI 分支先分区，rollup 只吃掩码后的
   Regen 类净变更——容器位姿不再计入 `no_generation`，成员位姿不再误推
   `will_generate`；
3. **预览新增 `transform_targets`**（`DbnumPreview`，serde default 兼容）：
   逐个列出纯位姿目标的 `refno/noun/name` 与 `container` 标志（容器目标 =
   执行时刷新整棵子树），UI「模型更新」面板由此能把便宜路径的工作显式摆出来；
4. 模块头注释与 `no_generation` 字段文档同步改写；
5. 单测：`partition_splits_pose_from_regen_and_respects_cancellation`（位姿/重建
   归属、取消剔除、同元素重建吞并位姿）、
   `mask_details_keeps_only_regen_class_model_affecting`（净变更不丢、只掩调度语义）。

## 验证

- 单测：`cargo test --release --lib -- model_update_plan manual_update::tests`；
- 实库复验（`.surreal/site-8000-incrtest` 副本）：新造 E3D 会话 ZONE `BY U 500`
  → 预览应报 `transform_targets=[ZONE /1RX03-LCT (container)]`、`no_generation=0`、
  单元无误报 → execute → 子树全部实例 AABB 整体 +500 → 再造会话 `BY D 500`
  还原 → 复验回落。

## 残留事项（不在本次范围）

- Regen 类变更落在容器上（如 ZONE 的未知 UDA）仍走 `no_generation` + 告警：
  这类变更没有几何语义，现状合理；若未来出现真实用例再评估。
- `dbnum_statuses` 与 execute 的口径一致性（SC-002）是同族问题，另行跟踪
  （见 2026-08-04 增量实测报告问题二）。

## 关联

- `docs/evidence/2026-08-04-incremental-update-live-revalidation.md`（同日实测基线）
- `teach/cases/case-02-transform-only-was-too-wide.md`（TransformOnly 名单史）
- `docs/adr/ADR-010-room-membership-incremental-update.md`（便宜路径的房间归属义务）
- gen-model#8（本文的 issue 化跟踪，含初版误诊与修正说明）
