# 026 空间树按 refno 查旧条目规格

## 背景

初始化生成一个 dbnum 的模型很慢。按需基线（`initialize_dbnum_baseline`，新库进目录与
ADR-021 回退重建都走它）把整库排成 N 个生成根，`process_meshes_update_db_deep_with_policy`
逐根跑「深度查询 / 网格生成 / AABB落库 / 布尔运算」四段。现场证据里单根只有 1~2 个结点时
AABB落库 就占 124~200 ms，是四段里最大的一块
（`docs/evidence/2026-08-20-rm12-arc-pane-live/`、`.../2026-08-20-rm13-dome-live/`）。

根因不是几何算力，是一次查询的实现方式。`update_inst_relate_aabbs_by_refnos_mode` 要回答
「这批 refno 在空间树上现存的旧包围盒是什么」，用来做变更判定（`tree_box_changed`）。这个
问题今天以**遍历整棵 `GLOBAL_AABB_TREE`** 实现，而该树在 AMS 上有 105651 条；一个生成根还要
问两遍（布尔前刷一次、布尔后按最终关系再刷一次）。

初始化期间树是边生成边长大的：第 k 个根要扫的是前 k−1 个根的产物，总代价对库规模是**平方级**。
这就是「越往后越慢」的形状，也是为什么同一份代码在单根增量上看着还好、在整库初始化上不能忍。

`AccelerationTree` 内部本来就有一张 `refno_index`（refno → 该 refno 在树上现存的全部条目），
`sync_refnos` 与 `remove_by_refnos` 都已经用它，vendor 里还有
`sync_refnos_cost_is_not_proportional_to_tree_size` 钉着「单 refno 操作不得扫整树」。
**这条不变量已经存在于写路径，只是读路径漏了**——`refno_index` 是私有字段，对外没有读接口，
于是调用方只能退回 `tree.iter()`。

同一个毛病还有第二处：`delete_room_membership` 的窗口外分支持写锁跑
`tree.iter().any(|bbox| stale.contains(&bbox.refno))`，只为问一句「这几个 refno 在不在树上」。

本规格只做这一件事：把「按 refno 问空间树」这个查询从线性扫描改成索引查找，并让两处调用点
都用上它。**不碰 ADR-041 / `specs/023` 的并行方案**——那条路解决的是常数因子，这条解决的是
平方项，两者相乘才有意义，且本规格是它的前置：并行化一个平方级循环，结果仍是平方级。

## 要求

1. 存在**唯一**的「按 refno 取空间树现存条目」的权威能力，与 `sync_refnos` /
   `remove_by_refnos` 共用同一份索引。不得在 `gen-model` 侧另建一份 refno → 包围盒 的映射：
   第二份真值会与树漂移，而漂移的后果不是变慢，是房间归属算错。
2. 该能力的单次调用耗时**不随空间树总条目数线性增长**，只随被问的 refno 数量增长。
3. `src/fast_model/occ_generate.rs` 的变更判定与 `src/data_interface/helper.rs` 的删除清理
   探测都改用它；这两处不再存在「为按 refno 定位而遍历整棵树」的写法。
4. 索引不可用（与树不同步）时**行为必须可见**。答案仍须正确，但不得静默退化成线性扫描而
   无人知晓——生产路径上所有动树的调用都经过维护索引的 API，因此该情形一旦发生，就意味着
   有人新写了一处绕过 API 的树修改，那是缺陷本身，不只是变慢。
5. **语义严格不变**：变更判定的输入与结论、房间目标集合、spatial epoch bump 的次数与时机、
   锁序 `SPATIAL_STATE_SERIAL → GLOBAL_AABB_TREE`、以及现有源码顺序断言所钉的
   「判定在锁下 → 事务 → 树同步」次序，全部不变。本规格不改任何对外可观测的语义，只改一次
   查询的复杂度。
6. 可度量：改动前后，同一个库、同一份数据的「模型结点更新」四段耗时可以对照，且该查询本身
   有独立计时与当时的树尺寸，能单独归因。

## 非目标

- 不改 ADR-041 第 1 条的并行单位，不动 `specs/023` 的任何一条任务。
- 不改初始化的路由形状（仍是 N 个定向生成根，不塌缩成整库全量）。
- 不改队列、水位、房间归属、布尔运算的任何语义。
- 不改 `process_meshes_update_db_deep_with_policy` 的根间串行（那是 `specs/023` 的范围）。
- 不消除「布尔前后各刷一次 AABB」这两次刷新（语义上各有理由，见 ADR 引用处的注释）。
- 本轮不升 `aios_core` 的 git rev；依赖仓改动靠仓库既有的本地 `[patch]` 验证。发布另计。

## 兼容性

- `accel_tree.bin` 的序列化格式不变：`refno_index` 是 `#[serde(skip)]` 的纯内存派生数据，
  本规格不改变这一点，旧快照仍可直接加载。
- 不改 `/api/v1/*` 的响应形状。
- 不改 `DbOption` 的必填键，`python/testbed/DbOption-pytest.toml` 无需同步。

## 成功标准

- **等价**：同一份输入，改动前后逐表一致——`inst_relate.aabb` 指针与 `aabb.d`、
  `geo_relate`、空间树快照条目集合、`room_panel_relate`、`room_relate`、
  `model_update_pending` 中 `room_recalc_*` 行的集合。
- **提速可归因**：树规模 10⁵ 量级下，该查询的独立计时相对改动前显著下降，且改动后其耗时与
  树尺寸不再相关；「AABB落库」分段总耗时随之下降的幅度有数据支撑。
- **回归钉子**：存在一条「退回全树扫描就会红」的复杂度性质测试（形态照抄 vendor 既有的
  `sync_refnos_cost_is_not_proportional_to_tree_size`）。
- **形状钉子**：源码断言禁止这两处调用点回退到 `tree.iter()` 做按 refno 的定位。
- **可见性钉子**：索引不同步时的处置有对应测试，且该情形在回执/日志里看得见。

## 决策引用

- ADR-045：本规格实现它。「按 refno 问空间树」只有一份实现，读路径与写路径共用 `refno_index`。
- ADR-010：房间归属增量模型与空间树的角色；本规格不进入「N 次增量 ≡ 一次全量」那个口子。
- ADR-041 / `specs/023`：并行生成与收口提速。本规格是其**前置**而非替代，两者不冲突：
  023 处理常数因子（并发额度），本规格处理平方项（单次查询复杂度）。
- ADR-012：多根合批。合批的收益被本规格的平方项吃掉过，修完才兑现得出来。
- `specs/018`：房间失效正确性闭环。本规格不改变房间目标集合，因此不触及其结论。
