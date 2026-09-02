# 034 新版 pdms-io 的 Core3D 元素语义层规格

## 背景

新版 pdms-io（`pdmsdb_engine_v2`，db1~db5 五层引擎）已按 core.dll 复刻了页、会话与 B 树，
但「元素怎么被用」这一层——noun 位表分类、三模成员遍历、significant 祖先攀爬、库类型门、
CE 导航——在 db4 缺失或只有半成品（`NavDirection` 枚举无人使用）。这层语义的外部权威是
Core3D.dll 的 `PartialUpdateDesiMgr`，本仓已逆向到规则级（R0–R29 核对表 + C 编号用例 +
可执行参考模型 + 1931 noun 位表快照）。ADR-055 已裁决：db1–db3 以 core.dll 为准，
db4–db5 及以上以 Core3D 为准；pdms-io 只做读语义；本轮只读。

gen-model 是这层语义的直接消费者：direct 模式（ADR-053）的 `DbElement` 门面、
`generation_root` 名单判定、`model_impact` 影响分类都要转调它，而不是各自再实现一遍。

## 功能要求

1. **noun 位表来源可插拔且自证**（Q2）：存在 `NounBitSource` 抽象，significant 与
   primitive 两位**分开可查**（哪一位说了算永远问得出来）；生产实现读位表快照并校验
   `core_sha256`，**校验不过加载报错，不回落**；对拍实现经 core.dll FFI 现取。
   「字段未登记 = 该位为假」，但未登记命中要可统计，不许静默。
2. **三模成员遍历语义与 Core3D 逐条一致**（Q3，R11）：收集判据与下潜判据是两个独立谓词；
   mode 0 下非 significant 子节点挡住其下 significant 孙节点；mode 1 收集 primitive 但
   下潜整棵子树；mode 2 为死代码，实现但不暴露给生产调用方。遍历返回游标/迭代器，
   **不物化成员列表**。
3. **significant 祖先攀爬**（R14）：从元素**自身**开始判、终止条件是 noun 位、不设深度上限；
   环保护用已访问集合，不用深度计数截断。
4. **按 noun 攀爬**（R2）：`climb(e, noun)` 能沿 owner 链找到首个指定 noun 的祖先
   （XGEOMETRY 子树排除门用它实现）。
5. **库类型门**（R1）：库类型判定与 Core3D 的 `DB_DB::type(db) == 1 → DESI` 语义对齐，
   从库查找层暴露。
6. **有效性与存在性**（R3/R26）：元素有效性判定、递归全子树存在性检查可用。
7. **CE 导航栈补齐**（Q4）：`NavDirection` 五个方向真正驱动导航；保存/恢复位置语义对齐
   `DSAVE`/`DRESTO`；owner 链迭代不整树加载。
8. **三层对拍口径**（Q5）：在既有 `legacy_oracle`、`core_dll_oracle` 之外新增 `core3d_oracle`，
   以可执行参考模型为期望值、C 编号用例为数据驱动夹具；参考模型提升为**单一来源**，
   gen-model 与 pdms-io 不各留一份。
9. **页大小与会话时点硬化**：文件头 `0x34` 按「4 字节字」正确解释；探测器仅作兜底且需
   两条独立判据同时成立；支持按指定 sesno 打开（pin `applied_sesno` 与读最新共用一条实现）。
10. **多 extent**（Q7）：extent 寻址补齐前，打开多 extent 库**显式报错并点名文件**；
    补齐后跨 extent 的 refno 可定位解析，上游静默回落路径删除。
11. **上游薄封装**（Q6）：gen-model 的 `DbElement` 门面对分类/遍历/攀爬一律转调本层，
    不在 gen-model 侧出现第二份判据实现。

## 非目标

- 写回：`record_writer`、db5 的 mark/refresh/compact、`e3d31-writeback` 全部冻结（Q8）。
- 把 `PartialUpdateDesiMgr` 的队列/去重/三遍消费搬进 pdms-io（Q6）；
  视图 / ID 清单 / PML 相关的一切（R7/R8/R22/R23/R27 判 ⚪ 部分）。
- Negative 成员遍历（mode 2）与 `m_granularityMode ≠ 0` 分支的生产化（死代码，标注不实现）。
- 不改生成算法、`cata_hash` 复用、产物写入与房间管线（ADR-053 语义红线继续有效）。
- 不引入第二条数据批次消费路径（ADR-011）。

## 成功标准

1. 快照与 FFI 两个位表实现对 **1931 个 noun 全部一致**；快照 `core_sha256` 被篡改时加载
   **报错**（有对应回归测试）。
2. `core3d_oracle` 下 **C 编号用例全绿**；其中「非 significant 子节点挡住 significant
   孙节点」有独立用例钉死（实现错了会红）。
3. 深度 N 的子树遍历，记录页读取次数与元素数**同阶**，不随栈深出现二次增长；
   `NavDirection` 五个方向各有 round-trip 用例。
4. 真库中已知会骗过页大小探测器的文件（490 个中的 17 个，如 `ams7329_0001`），
   **不给 hint** 也读出 `page_size=2048` 与权威 sesno。
5. 双 extent 夹具能跨 extent 定位并解析；gen-model 侧 `on_demand_db.rs` 的 legacy 回退
   改为断言不再触发。
6. `direct_attmap_probe` 经新门面复跑 dbnum 8000/7333，**0 真值冲突**；
   `tests/model_impact.rs` 与位表快照的对账结论不变。
7. 每个实现 Core3D 语义的公开函数，文档注释含核对表 R 编号回引（抽查可验）。
