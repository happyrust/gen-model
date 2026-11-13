# T-D-8 盘点：`pdms_inst` / `shape_save` 是否仍在增量窗口写 `inst_relate`

日期：2026-10-28
结论：**双写有先删契约**（不是「只出适配器」，也不是「双写无契约」）。
复核方式：`rg` + 定向 `Read`，未跑 `cargo`。行号按 2026-10-28 工作树（`refactor` 分支，含未提交改动）。

## 一、结论

增量窗口的 `inst_relate` 写路只有一条，且带**同一函数内的先删后插**契约：

```
gen_model::run_shape_save_receiver(SaveMode::TargetedReplace)   src/fast_model/gen_model.rs:321
  └─ shape_save::flush_batch                                    src/fast_model/shape_save.rs:307-315
       └─ pdms_inst::execute_save_plan(plan)                    src/fast_model/pdms_inst.rs:1375
            ├─ ① if plan.mode.replaces_existing() && !delete_refnos.is_empty()
            │      delete_inst_relate_cascade(&plan.delete_refnos, ..)   :1376-1378
            └─ ② for phase in [SharedContent, Relations, InstanceRelations] { execute_packet(..) }
                                                                        :1384-1388
```

- 先删：`:1376-1378`，条件是 `replaces_existing()` 且 `delete_refnos` 非空。级联删除本体在 `src/data_interface/helper.rs:161 pub async fn delete_inst_relate_cascade`。
- 后插：`:1384-1388`，三阶段**严格有序**，`inst_relate` 的 packet 在 `InstanceRelations` 阶段（`SavePhase::InstanceRelations`，`:919` 处构造、`:1389` 处参与循环、`:2823` 处排阶段序号 2）。
- 契约成立的关键：`execute_save_plan` 是**唯一**入口，先删在函数第一句，插入在它之后，中间没有早退分支。所以不存在「插了没删」或「删了又插回旧值」的窗口。

## 二、`replaces_existing()` 覆盖哪些模式

`src/fast_model/shape_save.rs:85-88`：

```rust
impl SaveMode {
    fn replaces_existing(self) -> bool {
        matches!(self, Self::TargetedReplace)
    }
}
```

- `SaveMode::TargetedReplace` → `true` → 走先删。这是增量窗口的模式（`gen_model.rs:321`）。
- `SaveMode::FullBuild` → `false` → 不走先删。这是整库构建（`gen_model.rs:251`），此时库是空的，不需要删。

两种模式的调用点都唯一，没有第三种模式。

## 三、`inst_relate` 的 packet 从哪来

`build_canonical_save_plan`（`src/fast_model/pdms_inst.rs:956`）构造 `SavePlan`，`inst_relate` 的 packet 在：

- `:1322` `push_inst_relate_packets(&mut packets, &normal_inst_relates);`
- `:1323` `push_inst_relate_packets(&mut packets, &tubi_inst_relates);`

两类来源：普通元素（`normal_inst_relates`）与隐含直管段（`tubi_inst_relates`）。同一 `record_id` 同时来自两者会在 `:1226-1228` 被判为 `SaveConflict`（错误文案见 `shape_save.rs:39`「同一个 inst_relate id 同时来自普通元素与隐含直管段」）。

## 四、`pdms_inst` 的其它 `inst_relate` 写点（都不是增量窗口）

| 位置 | 作用 | 是否在增量窗口 |
|---|---|---|
| `pdms_inst.rs:120 init_inst_relate_indices` | 建索引 | 否，启动期 |
| `pdms_inst.rs:171 backfill_inst_relate_anc` | 回填 `anc` | 否，启动期迁移 |
| `pdms_inst.rs:347 sweep_inst_relate_flat` | 刷 `insts_flat` | 否，启动期迁移 |
| `pdms_inst.rs:412 sweep_inst_relate_flat_if_dirty` | 同上，按 dirty 标记 | 否，启动期 |
| `staging/executor.rs:669`、`staging/parity.rs:264` | `UPDATE inst_relate SET anc = [1] WHERE anc = NONE` | 否，staging 迁移脚本 |

这些都不在 `execute_save_plan` 路径上。

## 五、`model_db_adapter.rs` 的三处 `INSERT INTO inst_relate`

`:3523`、`:3590`、`:4635` 全在 `#[tokio::test]` 内（`slice_delete_removes_every_representation_of_the_slice_and_nothing_else`、`a_spooled_tombstone_removes_every_representation_of_the_root_and_nothing_else`），是夹具，不是生产写路。

## 六、`shape_save` 不直接写 `inst_relate`

`shape_save.rs` 全文只有一处 `inst_relate` 字面（`:39` 的 `SaveConflict` 错误文案），没有 SQL 写语句。它的职责是攒批、冻批、判冲突，写库统一交 `execute_save_plan`。

## 七、验收判定

| 验收口径 | 结论 |
|---|---|
| 调用图证明增量 DESI 几何只出适配器 | 不成立——它确实写 `inst_relate` |
| 双写有先删契约 | **成立**——`execute_save_plan:1376-1378` 先 `delete_inst_relate_cascade`，`:1384-1388` 再按阶段插，同一函数、无早退 |

T-D-8 通过。
