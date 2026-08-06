# issue #7 修复复验（2026-08-06）

- 复验对象：[gen-model#7](https://github.com/happyrust/gen-model/issues/7)「增量后没有进行房间
  计算」，在 **HEAD `b0b90fc8` + 当时的未提交工作树** 上（那批改动此后并入 `0822f481`；
  `room_model.rs` 一字未动）。
- 为什么要复验：修复落在 `270d71c5`（08-05），此后又并进了 44 个提交，其中
  `a81dd81b feat(staging): compute element rooms in window` 把元素房间计算整个搬进了暂存
  窗口。修复是否活过那次重构，不能靠读提交日志断定。
- 方法：源码核对（两条消费路径）+ 合成夹具 live 回归 + **真库现场复现** + 两处**回退即红**
  反证 + 离线全量。
- 上一轮：`docs/2026-08-05_issue-7-room-incremental-audit.md`（定性与根治）。

---

## 0. 结论

**修复活着，而且这次是在报告人那个构件本身上钉住的。**

`270d71c5` 拆掉的那个依赖（元素分支到空间树的反向依赖）没有在 staging 重构里长回来。
两处证据，都是「回退即红」而不只是「测试绿」：

- **合成夹具**（§2、§3）：issue #7 的两步在一次性内存实例上绿；把那个依赖临时接回去，
  同一条用例在同一份代码上逐字复现症状——任务算成功、队列行删除、日志一行没有、被删
  的边没回来。
- **真库**（§5）：同样两步，跑在 `=24383/66460`（报告人改的那个 CAP）与它真实所属的
  房间 `/1RX-RM05-R512` 上，绿；回退，红，`left: []`。

到这一步，issue #7 的成因不再是「最可能」，而是**在他那个构件上被复现、并被这一处代码
消掉的**那一个。还欠的是他那台机器上的那份数据，以及全库口径的对拍（§6）。
## 1. 修复在两条消费路径上都还在

staging 重构之后房间任务有两条入口，它们都经 `run_room_task` 落到同一个元素分支，
候选面板一律取自本轮加载的 `PanelIndex`（库内面板几何），都不看空间树：

| 入口 | 位置 | 候选来源 |
|---|---|---|
| 空闲轮 | `batch_worker.rs:1868` → `drain_rooms` → `model_update_pending.rs:1500` | `load_panel_index` |
| 暂存窗口 | `batch_worker.rs:606` → `run_staged_room_work` → `model_update_pending.rs:1069` | `load_panel_index` |
| 单件执行 | `execute_item` → `model_update_pending.rs:922` | `load_panel_index` |

同轮吸收的封闭性检查也同源（`candidate_panel_refnos`，`model_update_pending.rs:1694`）——
这条当初若分叉，元素分支本会写的边会被错误吸收、永久跳过。

## 2. live 回归：issue #7 的两步，绿

`fast_model::room_fixture::tests::live_room_deleted_edges_come_back_after_a_move` 逐字复刻
报告人的两步（先 `DELETE room_relate`，再挪构件），并把主嫌单独隔离：**业务库里的面板
几何保持完整，只把 PANE 从空间树上摘掉**。走的是生产消费路径
（`enqueue_room_recalc` → `drain_rooms` → 本轮 `PanelIndex` → 元素分支），不直调分支函数。

```text
# 服务端：仓库自带 SurrealDB 2.1.4+20250317.45013fc9，一次性内存实例
./scripts/Start-Surreal8009.ps1 -Memory -Bind 127.0.0.1:8072
$env:AIOS_LIVE_WS="ws://localhost:8072"
cargo test --lib --features http_api \
    fast_model::room_fixture::tests::live_room_deleted_edges_come_back_after_a_move \
    -- --ignored --exact --nocapture

房间归属重建: 1 间房 / 2 块面板
房间归属重建完成: 写入 6 条成员边
[房间增量] 构件 4000000001_20 归属: 无房间 -> K100      ← 被删的边回来了
房间归属重建: 1 间房 / 2 块面板
房间归属重建完成: 写入 6 条成员边
test result: ok. 1 passed; 0 failed
```

同一实例上 9 条房间 live 用例逐个跑（`SUL_DB` 是进程级全局，只能一个进程一条），全绿：

```text
PASS  live_room_fixture_parity
PASS  live_room_incremental_parity
PASS  live_room_deleted_edges_come_back_after_a_move
PASS  live_room_panel_move_parity
PASS  live_room_panel_task_absorbs_element_task_in_the_same_round
PASS  live_room_cross_panel_move_defeats_absorption
PASS  live_room_delete_clears_membership
PASS  live_room_structural_triggers_enqueue_panel_recalc
PASS  live_room_tubi_row_enters_tree_and_tracks_regen
```

## 3. 回退即红：这条用例确实还咬得动

绿本身不说明什么——用例可能早就测不到修复了。所以在**当前工作树**上把候选口径临时改回
`270d71c5` 之前的样子（候选必须同时出现在空间树里且 `noun == "PANE"`），只改
`recalc_element_membership` 里的一处：

```rust
-    let candidates = panels.candidates(&element_aabb);
+    let candidates: Vec<&PanelEntry> = {
+        let tree = GLOBAL_AABB_TREE.read().await;
+        let in_tree: HashSet<RefU64> = tree.tree.iter()
+            .filter(|bbox| bbox.noun == "PANE").map(|bbox| bbox.refno).collect();
+        panels.candidates(&element_aabb).into_iter()
+            .filter(|entry| in_tree.contains(&entry.panel.refno())).collect()
+    };
```

同一条 live 用例立刻红，而且是**报告人那个症状的逐字复现**：

```text
thread '…live_room_deleted_edges_come_back_after_a_move' panicked at room_fixture.rs:1267:
assertion `left == right` failed: 删掉的边必须被增量原样建回来（issue #7）
  left: []
 right: [Edge { panel: "4000000001_10", part: "4000000001_20", room_num: "K100" }]
```

注意红在哪一行：`assert_eq!(done, 1, "那条元素任务必须被消费掉")` 是**过了**的——
`drain_rooms` 返回成功、队列行被删、没有任何日志，库里只剩那条 DELETE。这正是 issue #7
「删掉边 → 改模型 → 房间号还是查不到，而且再怎么改也回不来」的机制。

离线源码守卫同轮也红（不需要 live 库就能拦住回归）：

```text
test fast_model::room_model::tests::the_element_branch_does_not_depend_on_the_spatial_tree ... FAILED
test result: FAILED. 20 passed; 1 failed
```

改动已还原，还原后按文件哈希核对与实验前逐字节一致，用例回绿。

## 4. 离线全量

```text
cargo test --lib --features http_api
test result: ok. 430 passed; 0 failed; 66 ignored
```

## 5. 真库现场复现

§2 那条跑在合成夹具上。这一节把同一件事搬到**报告人那个构件本身**上——本机 8009 的
`AvevaMarineSample`（ns 1516）这套库里现在有 `pe:24383_66460`，而且它的 `POS.z` 正是
**5821.669921875**，就是 issue 里那次修改。

新增 live 用例 `fast_model::room_live_issue7::tests::live_issue7_real_db_deleted_edges_come_back`，
靶子全部取自 issue 原文：

| | refno | noun | dbnum | 出处 |
|---|---|---|---|---|
| 构件 | `24383_66460` | `CAP` | 7999 | 报告人改的那个（`CAP 1 of /1WCC1135/B1`） |
| 面板 | `24381_35844` | `PANE` | 7997 | 被删的边 `room_relate:⟨24381_35844_24383_66460⟩` 的 in 端 |
| 房间 | `24381_35842` | `FRMW` | 7997 | `/1RX-RM05-R512` |

（另一条边 `room_relate:⟨24381_1391_24383_66460⟩` 的 `pe:24381_1391` 在这套库里不存在。）

三段：

1. **备料**——按需生成两侧几何。跑之前 `inst_relate WHERE in = pe:24383_66460` 与
   `... = pe:24381_35844` 都是空的、全库 `room_relate` 为 0；生成根分别落在
   `24381/35843`（SBFR `-VOLU`）与 `24383/66459`（BRAN `/1WCC1135/B1`），量级都很小。
   **这一步本身就是 ADR-010 §9 一直被卡住的前提**（「结构库从未生成、
   `inst_relate WHERE in.noun = 'PANE'` 为 0」），这次终于过了。
2. **全量基线**——只重建 `/1RX-RM05-R512` 这一间，算出 1466 条成员边，其中恰好包含
   issue 里被删的那条：

```text
Edge { panel: "24381_35844", part: "24383_66460", room_num: "R512" }
```

   到这里已经能说一句此前说不出的话：**这个构件确实属于 R512，而且全量路径算得出来。**
3. **两步复现**——与 §2 同一个隔离手法：业务库里的面板几何保持完整，只把空间树上的
   13396 条 PANE 条目全部摘掉（报告人现场就是这个形态，`accel_tree.bin` 落在结构库
   生成之前）。然后走报告人的两步：`DELETE room_relate WHERE out = pe:24383_66460`，
   再把 `POS.z` 抬高 100，并走生产上纯位姿变更那条链（`clear_all_caches_batch` →
   `update_world_transforms` → 刷新包围盒 → `enqueue_room_recalc` → `drain_rooms` →
   元素分支）。

结果：

```text
[issue7] 从空间树上摘掉 13396 条 PANE 条目（隔离 issue #7 的主嫌）
[issue7] 移动后队列: ["room_recalc_element_24383_66460"]
[issue7] 移动后 aabb: ["{ maxs: [-4982.45, 10724.729, 5940.67], mins: [-5020.45, 10686.891, 5907.67] }"]
[房间增量] 构件 24383_66460 归属: 无房间 -> R512
test result: ok. 1 passed; 0 failed
```

**同一条用例上的回退即红**（把候选口径改回 `270d71c5` 之前，其余一字不动）：

```text
[issue7] 移动后队列: ["room_recalc_element_24383_66460"]     （任务确实排出来了）
assertion `left == right` failed
  left: []
 right: [Edge { panel: "24381_35844", part: "24383_66460", room_num: "R512" }]
```

任务入了队、包围盒确实变了、drain 报成功、日志一行没有，边就是不回来——**报告人描述的
那个现象，在他那个构件上逐字复现，而且差别只在这一处代码。**

用例收尾把 `POS` 写回 5821.669921875、清掉自己排的房间队列行。跑完核过：`POS` 是原值、
`room_relate` 里那条边在、`model_update_pending` 里一条房间行都没有（余下 2967 行全是
既有的 `regen_root`）。

### 备料这一步顺带证伪的一条

第一次跑时第二步没点火：`get_world_transform` 带进程级 `#[cached]`，改完 `POS` 不失效的
话，`update_world_transforms` 读到的还是旧矩阵，包围盒不变、任务不入队。这**不是**产品
缺陷——生产上 `IncrementPipeline::invalidate_caches` 在任何消费者之前就
`clear_all_caches_batch` 过了（`increment_pipeline.rs:817`）。用例照抄了这个顺序。

## 6. 仍然欠着的

- **报告人自己那套库**：§5 跑的是本机 8009 上的 `AvevaMarineSample`——同一个项目、同一个
  构件、同一次改动，但不是报告人那台机器上的那份数据。他那边还得按同样两步走一次才算
  收口，尤其要确认他的 `room_relate` 现在能长回来。
- **E3D 侧那一段没走**：§5 的第二步是直接改库里的 `POS` 再走 `Transform` 工作项，没有经过
  「E3D 改坐标 → 增量解析」。那一段与 issue #7 无关（报告人的模型**确实**变了，说明解析与
  位姿更新都是好的），但严格说仍是补的一环，`scripts/e3d/projams_incr_pos_apply.mac` 那套
  夹具可以接上。
- **整库口径的 ADR-010 §9 对拍**：§5 只重建了 `/1RX-RM05-R512` 一间。全库 124 间的
  「增量 == 全量」仍欠着，前提是结构库要整体生成一次（本轮只生成了两个根）。