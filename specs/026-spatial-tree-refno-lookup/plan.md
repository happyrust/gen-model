# 026 空间树按 refno 查旧条目实施计划

## Constitution Check

- **I 水位是承诺**：不碰 `applied_sesno` 的推进条件、不碰尾事务、不碰队列收口谓词。本规格只
  改一次内存查询的复杂度，提交语义与失败语义原样不动。
- **II 一条规则只有一份实现**：这正是本规格的动机。「按 refno 问空间树现存条目」今天有两份
  实现——写路径（`sync_refnos` / `remove_by_refnos`）走 `refno_index`，读路径退回全树扫描。
  本轮把读路径收敛到同一份索引上。**明确拒绝**在 `gen-model` 侧自建一份 refno → 包围盒 映射：
  那会造出第二份「树上现在有什么」的真值，需要自己盯住删除清理、指针重建、启动加载三条路径
  去同步，漂移的后果是房间归属算错而不是变慢。
- **III 静默失效是最高级别缺陷**：索引与树不同步时，答案仍然正确（回退到扫描），但**必须计数
  并打日志**。生产路径上所有动树的调用都经过维护索引的 API（已逐处核对），因此该日志一旦响，
  就说明有人新写了一处绕过 API 的树修改——那是缺陷本身。这条日志既是安全网也是警报。
- **IV 队列任务可消费 / 可收口 / 可复活**：不新增 `ModelWorkAction`、不改 drain 过滤器、
  不改 `settle_predicate`。房间目标集合（`chunk_changes`）逐元素不变，因此入队的
  `room_recalc_*` 行集合也不变。
- **V 标识只用真值**：查询键就是 `RefU64` 本身，不构造派生标识、不做字符串拼接寻址。
- **VI 不变量由可执行的守护看住**：复杂度性质测试（退回全扫就红）、两处调用点的源码形状断言、
  索引不同步时的处置测试，三样都补。
- **并发模型**：不新增数据批次消费路径。锁序 `SPATIAL_STATE_SERIAL → GLOBAL_AABB_TREE` 不变——
  新接口刻意做成 `&self`，正是为了不把暂存分支现有的**读锁**升级成写锁（升级会在不持空间串行锁
  的情况下取树写锁，直接破坏锁序，且仓内有源码顺序断言钉着这一条）。
- **运行环境**：Windows / PowerShell，仓库锁定工具链，不执行 `cargo clean`。

## Complexity Tracking

**唯一一条需要登记的例外：本规格要改依赖仓 `../vendor/old-aios-core`。**

- 为什么无法避免：要用的 `refno_index` 是 `AccelerationTree` 的私有字段，而它必须私有——
  它是 `#[serde(skip)]` 的派生数据，暴露出去等于允许外部制造不一致。唯一符合原则 II 的做法
  就是在拥有它的类型上开一个只读接口。本仓自建索引是违反原则 II 的那个方案，已在上面否决。
- 边界：本轮**不升 `aios_core` 的 git rev**，靠仓库的 `[patch]`（指向
  `../vendor/old-aios-core`）在本地验证与量数。发布（vendor 提交 → push → 升 rev）是独立的
  后续步骤，不在本规格范围。
- **`[patch]` 当前是关的**（`scripts\Toggle-LocalDeps.ps1 -Status` 报 OFF，`Cargo.toml` 里那
  三段是注释掉的）。所以 T04 起要先 `-On`，量完 `-Off` 复原——别把开着的状态留在工作区。
- 因此本轮**不得推 `main`**：`.githooks/pre-push` 会拦下带本地依赖重定向的提交，这是预期行为。

## 设计要点

1. **在 `AccelerationTree` 上新增两个 `&self` 只读接口**，都走 `refno_index`：
   - 取一批 refno 在树上现存的全部条目（供变更判定用）；
   - 判断一批 refno 是否至少有一条在树上（供删除清理探测用）。
   第二个不是第一个的糖：删除路径只需要一个 bool，不该为此物化一份 `HashMap`。
2. **索引不新鲜时的处置：回退全树扫描 + 计数 + 打一次日志。**
   备选方案与否决理由：
   - *静默回退*：违反原则 III。一个悄无声息退回 O(N) 的快路径，正是「它跳过的东西谁会发现」
     答不上来的那种分支。
   - *不新鲜就返回 `Err`*：最诚实，但它把扫描兜底留在**每一个调用方**——现在是两处，以后只会
     更多，而删掉这个扫描恰恰是本规格的目的。
   - *做成 `&mut self`，进来先重建索引*（依赖仓 `AccelerationTree` 私有的那条重建路径）：
     暂存分支只持读锁，改成写锁会破坏锁序（见 Constitution Check 并发模型条），出局。
3. **不收窄 `tree` 字段的可见性、不移除 `Deref`**。那是让索引「永远新鲜」的根治办法，但会波及
   依赖仓的其它下游，与本轮目标不对价。用第 2 条的可见回退兜住。
4. **两处调用点的语义严格照搬**：`src/fast_model/occ_generate.rs` 仍在锁下、仍在任何指针写入
   与树同步之前完成变更判定；`src/data_interface/helper.rs` 仍在写锁下、仍在事务之前完成
   present 探测。只把「怎么问树」换掉。

## 阶段

1. **先量基线。** 给 `stale_by_refno` 那一段埋独立计时，同时打印当时的树尺寸，并入
   「模型结点更新耗时」那行日志。不改任何行为，跑一轮初始化留数。没有这组数，后面的加速比
   无从归因，而且万一它只占一小半，优先级就该当场重排。
2. **依赖仓加只读接口。** 在 `../vendor/old-aios-core` 上实现两个 `&self` 接口与不新鲜回退，
   并在该仓补复杂度性质测试（小树 vs 大树，单 refno 查询耗时不成比例增长）。
3. **换掉生成路径。** `src/fast_model/occ_generate.rs` 的 `stale_by_refno` 改用新接口，
   直写与暂存两条分支都换；现有源码顺序断言全部保持通过。
4. **换掉删除路径。** `src/data_interface/helper.rs` 的 present 探测改用新接口，**同批更新**
   它那条按 `tree.iter().any(` 找位置的源码顺序断言——针不改，那道门会静默变成「找不到即 panic」。
5. **补守护。** 形状断言禁止两处回退到 `tree.iter()` 做按 refno 定位；索引不同步的处置补测试。
6. **对照与等价验收。** 同一个库、同一份数据各跑一遍，逐表比对，记录四段耗时与 stale 子项。
7. **文档与质量门。** `changelog.md`、live 台账、证据目录；`cargo fmt`、`cargo check`、
   相关 feature 单测与 isolated live 测试；`sigmap verify-plan` / `verify-ai-output` / `review-pr`。

## 风险

- **收益可能不如预期。** 如果实测 stale 只占 AABB落库 的小半，那大头在 `save_aabb_to_surreal`
  或那条 epoch bump 事务上，本次改动的收益就有限。阶段 1 的作用就是在动手前发现这一点；发现了
  也不算白做（这段代码本来就该修），但要当场把优先级重排到 P3/P4（根间串行、durable 分支恒排
  房间任务）。
- **依赖仓改动的编译面。** 本地 `[patch]` 开着才编得过，CI 与新克隆的仓库拿的是 git rev 的旧
  代码。本轮要手动 `Toggle-LocalDeps.ps1 -On`、量完 `-Off` 复原，且不推 `main`。
- **依赖仓工作区本来就是脏的。** `../vendor/old-aios-core` 相对它的 HEAD（`29c91f48`）已有大量
  未提交改动，`src/accel_tree/acceleration_tree.rs` 就在其中。动手前先看清那份 diff，别把别人
  在飞的改动跟本轮的混成一笔。
- **源码顺序断言的针会失效。** `src/data_interface/helper.rs` 那条按字符串找位置的测试，
  改完必须同批更新，否则
  它从「守着次序」退化成「找不到就 panic」——这正是仓内注释警告过的形态。
- **暂存分支的行为差异。** 暂存分支的 stale 只喂给 `tree_box_changed`，而 durable 触发下
  `room_target_required` 恒为 true，`aabb_change_count` 只进一句 println。换接口不改这条链，
  但等价验收要覆盖暂存路径，不能只测直写。

## 验证与回滚

- 基线与改动后的命令、输入、四段耗时、stale 子项与当时树尺寸、逐表 digest、退出状态写入
  `docs/evidence/2026-08-23-spatial-tree-refno-lookup/`。
- 回滚：新接口是**新增**，两处调用点的旧写法在 git 历史里完整可回。单 commit revert 即可，
  不需要依赖仓配合回退（旧代码不引用新接口）。

## 决策引用

ADR-045（本规格实现它）、ADR-010、ADR-012、ADR-041 / `specs/023`（本规格为其前置）、
`specs/018`。
