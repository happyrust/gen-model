# ADR-097：投影删除只有一个入口——锁序、受影响根闭包、退役与作废

状态：**已接受**（2026-11-08，`refactor`；实施提交 `54e8111a9`、`09ecc5170`、`a9645a9f1`、`d6432de43`）。
日期：2026-11-08
关联：ADR-082（逐出不是删除：本文只管删除，逐出判据不动）、ADR-080 D2 / D5（只有 `WholeRoot` 提交能拿缺席推删除；回执由存储在写锁里盖章）、ADR-079（生成根账本只在进程内）、计划 `.planning/2026-11-08-projection-store-root-only/task_plan.md`

## 背景

进程内投影（`ModelMemoryStore`）里删东西原先有三条路，各写各的锁序和收尾：

- **投影 / 读透**（`on_demand_model.rs` 的 `clear_projected_scope` + `delete_projected_subtree`）：逐根 try-lock，再拿按库生成锁；扫全部根，找与被删记录共有记录的根，一律 `invalidate_receipt`；先写 spool 墓碑，再发删除提交。被删根的点集、包围盒、提交序号留在存储里。
- **摄入**（`delete_model_subtree` → `e3d_model_service::delete_persisted_geometry_root`）：只拿逐根锁，不拿按库生成锁；只退役被删根本身（`retire_deleted_root`），外层根的回执不动；先改投影，再把墓碑 SQL 打进 `SUL_DB`。
- **整库清理**（`fast_delete.rs` 各入口）：自己拿锁，直接调存储的 `clear_dbnum`；读透形态的整库墓碑也写在 `fast_delete` 里。

由此有三个问题：

1. **外层根读不了**。记录按祖先链挂到根上，嵌套的根会共有记录。摄入删内层根时外层根的回执不作废，之后读外层根报 `receipt expects 7, store has 3`（用例复现）。
2. **锁序靠源码文本守卫钉**：按字符串在函数体里的位置判先后、数出现次数。改个变量名就红；锁序真弄反了，只要字符串还在就测不出来。
3. **存储把删除方法公开给整个 crate**，「只有一份实现」只能靠守卫数调用点。

## 决定

**D1　投影删除只有一个入口：`data_interface::projection_removal`。** 对外两个函数：

- `remove_subtree(RemovalRequest) -> Removal`：按子树删。投影两路（`clear_projected_scope`）和摄入（`delete_model_subtree`）都走它，墓碑去向由调用方选 `TombstoneSink::{Spool, Database, None}`。
- `clear_dbnum(dbnum, spool_tombstones)`：整库清理的投影那一半。整库快删、整库模型清理、回退重建、升级清库都走它。

入口放在 `data_interface`，不放进存储：逐根生成锁、镜像 spool、MDB 快照都在这一层；存储保持同步，也不认识数据库。

**D2　`remove_subtree` 的锁序定死在入口里。**

1. 锁根排序去重后逐个 try-lock，任一被占就回「正在生成」，一行不删；
2. 按库生成锁，多库时按 dbnum 升序排队拿；
3. 拿齐之后才结算受影响的根（D3）、写墓碑、发删除提交、收尾回执（D4）。

返回的 `Removal` 带着这些锁：摄入那路要在同一把锁下删完 `inst_relate`、摘完账本才放；投影两路拿到就放。

`clear_dbnum` 不拿锁。整库清理各入口的锁序（按库生成锁 → `DATA_COMMIT_SERIAL` → `DBNUM_STATE_WRITE_GATE`）和「收键 → 删库行 → 删几何」都留在 `fast_delete`，投影在删库行之后清。

**D3　受影响的根在一把读锁里算成闭包。** `ModelMemoryStore::subtree_closure(scope, nodes)` 取子树下的记录，按子树根在每条记录祖先链里的位置，把常驻的根（有回执或已作废）分成两类：

- **被删根**：子树根及其以下、整根落在删除范围里的根。常驻的子树根自己没有记录也算。
- **外层根**：删除范围头上、与被删记录共有记录的根。

一次删多个子树时，一个根只要对其中某个子树整根落在里面，就算被删根。

**D4　收尾两种语义，同一把写锁。** 存储的 `retire_roots(removed, outer)`：

- **退役**被删根：回执、记录、作废标记、点集、包围盒、提交序号一起摘，记录已经没了也摘回执；
- **作废**外层根：摘回执与 ACK、记成作废，剩下的记录照旧可显示，等下一次整根重新发布。

两条路统一用这套语义：摄入删内层根不再让外层根残缺；投影那路请求恰好是整根时也退役，不再留点集这类残留。

**D5　先持久，后投影。** 墓碑（spool 或直接写库）落下之后才改投影；墓碑写不进去就整趟中止，投影不动。读透形态的整库清理同理：这个库每个根的 spool 墓碑都落下了，才把它从投影里摘掉。

**D6　「只有一个入口」由编译器把住。** 存储的 `retire_roots`、`clear_dbnum` 要一张凭证 `RemovalAuthority`：字段私有，只有入口造得出来，测试另有 `#[cfg(test)]` 的 `for_tests()`。`pub(in path)` 只接受祖先模块，入口与存储共同的祖先只有 `crate`，所以用凭证。非删除场景的作废（`model_refresh`、`model_dbnum_ensure`）仍用公开的 `invalidate_receipt`。

## 选项与取舍

- **锁序放进 `ModelMemoryStore`**：存储要变 async，还要依赖 `manual_update` / `e3d_model_service` 的锁注册表；墓碑那一半（spool、`SUL_DB`）本来就进不了存储。
- **投影那路保留「全部只作废」，只给摄入补外层根**：两条路仍是两套语义，被删根的点集、包围盒、提交序号继续残留。
- **保留文本守卫、只加行为测试**：守卫钉的是字符串不是行为。能换的都换成了行为测试；「收键 → 删库行 → 删几何」要全局 `SUL_DB` 和盘上的 `.mesh`，单测里没有，仍按源码先后钉，守卫的注释写明了原因。
- **把房间代号簿记搬出存储**：ADR-082 D1 的房间证据排除要在同一把写锁里判，搬走就得给存储注入否决判据；本期不搬，字段保持私有。

## 实施约束

1. 锁的行为（`projection_removal_contract.rs`）：`a_removal_is_refused_while_a_covered_root_is_generating`（根锁被占，5 秒内回「正在生成」，记录和回执都在）、`a_removal_waits_for_a_generation_on_the_same_dbnum`（同库生成锁被占时挂起，放手后结算）、`a_model_face_wipe_is_refused_while_the_dbnum_is_generating`、`an_upgrade_wipe_is_refused_while_the_dbnum_is_generating`。
2. 闭包与语义：`the_subtree_closure_matches_the_per_root_scan`（与旧的逐根扫描逐一比对）、`deleting_an_inner_root_keeps_the_outer_root_readable`（背景第 1 条）、`deleting_a_root_whose_records_are_already_gone_drops_its_receipt`。
3. 调用点（`on_demand_model.rs`）：`scope_cleanup_has_exactly_one_implementation`（入口调用恰好两处）、`forced_regeneration_clears_the_requested_scope_before_generating`（入口里只有 `write_database_tombstone` 碰 `SUL_DB`）、`subtree_delete_locks_covered_generation_roots_but_deletes_the_requested_scope`（摄入那路的顺序）。
4. 整库清理（`fast_delete.rs`）：`the_model_face_wipe_collects_geometry_before_the_rows_and_purges_after`、`the_upgrade_wipe_collects_mesh_keys_before_the_rows_and_purges_after` 按源码先后钉「收键 → 删库行 → 删几何」；`cata_dependency.rs` 的整库删除守卫找 `projection_removal::clear_dbnum(dbnum,`。
5. 存储里只在测试里用的方法收进 `#[cfg(test)]`，没人调用的删掉。`cargo test --lib` 带 `cfg(test)` 编译，生产代码误调测试专用方法也编得过，收窄时要逐个核对调用点。
6. 文档不写访问端点、令牌（`AGENTS.md`）。

## 后果

- 删内层根之后外层根读得出来：回执作废，自有记录照旧可显示，下一次 `/model/ensure` 整根重新发布。外层根为此多重算一次，这正是投影那路原来的行为。
- 摄入删除多拿一把按库生成锁：同库有生成在飞时，删除排在它后面。
- 以后新增删除路径只能从入口进；绕过入口直接调存储的删除方法编译不过。

## 不做

- 不改 ADR-082 的逐出判据与时机：逐出不是删除，不走本入口。
- 不重塑回执 / ACK 写入接口，不改外部对 `receipt()` 的读法（另立第二期）。
- 不搬房间代号簿记。
- 不动 `generation_root_lock` 与 `db_generation_lock` 的实现和粒度。
