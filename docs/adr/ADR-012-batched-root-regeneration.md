# ADR-012：多根合批重生成——共享执行器，队列表即工作单

状态：已接受
日期：2026-07-29
关联：ADR-001（水位）；ADR-011（数据批次队列）；`docs/plans/progress-contract-for-the-frontend.md`；
`src/data_interface/batch_worker.rs`（`run_unit_worklist`）；`src/data_interface/model_update_pending.rs`（`drain_where`）

## 背景

一次数据批次算出 N 个 fresh 生成根后，`run_unit_worklist` 逐根调用一次完整生成。
每一趟 `gen_all_geos_data` 都要重做一遍解析 → 实例 → mesh/boolean 的启动开销，
并且**收尾无条件全量序列化空间树**（`persist_aabb_tree`，`accel_tree.bin` 现约 21 MB）。
N 个根就是 N 次启动加 N 次 21 MB 全量写盘。

生成器本身早就接受整个根集合：`generate_roots` 传 `incr_updates = None`，
`gen_geos_data` 内部按 100 根一块顺序跑完全部 chunk，每块末尾等齐 worker。
合批不会放大并发，只是少付 N−1 次启动与落盘。

同一套「fresh 合批 → 失败逐根定位」的语义 `model_update_pending::drain_where` 已经有了；
主批次路径缺的正是它。

## 决策

- 抽出**一个**批量执行器：入参是已带 `revision` 的根任务列表，出参是逐根成败。
  自动、手动、级联补偿、队列 drain 与按需 `ensure` 都先写入同一 durable pending，再调用
  该执行器；准入规则、加锁顺序、失败回退、生成结果写入与 revision 收口只有一份。
  不让主批次直接委托 `drain_where`——那个函数没有 dbnum 过滤，直接复用会把
  `manual_update.rs` 里记录在案的老毛病放回来：dbnum=A 的批次去跑 B/C/D 的根、还记在 A 名下。
- **主批次的模型工作单从 `model_update_pending` 读**（按 dbnum），不再用内存里那份
  `collect_unit_tasks` 副本。`finalize_attempt` 已把本窗口每个 `regen_root` 行连同水位原子提交，
  表里那份就是权威；一次 SELECT 同时拿到根、noun、sesno、attempts 与 `revision`，
  逐根 `current_regen_revision` 随之取消。rollup 退化为 `old_owner` / `new_owner` 装饰。
- 队列读取失败 **fail closed**：本批不生成、任务记 `Partial`、行留在队列，等空闲轮重试。
  没有 `revision` 就生成，收口时可能删掉一行已因新工作重新 upsert 的任务——那才是真漏生成。
- 合批期间**逐单元进度事件与分母语义不变**：批前一次性发 N 条 `ModelUnitStarted`，
  批成功后逐根发 `Finished` 并 bump N 次，失败回退逐根时不重发 `Started`。
  代价是进度条在合批期间不走，终态 `units_done == total_units` 不变，plant-ui 无需改动。
- 合批**持有全部 N 个根锁**直到整批收口。`ensure` 不绕过锁或另开生成路径；命中已有
  pending/执行中的根时，同步等待同一任务的完成结果。

## 考虑过但否决的方案

- **主批次直接委托 `drain_where`**：见上，dbnum 隔离会退化；且 `ModelUnitResult` 的 owner
  字段来自本窗口 rollup，队列行不存 owner。
- **自己按 100 根切批**（让进度条每批一跳）：AABB 全量落盘从 1 次变回 `ceil(N/100)` 次 × 21 MB，
  把这次优化的主要收益退回去一大截。
- **给生成器加 chunk 级进度回调**：进度条会动且不牺牲收益，但要把回调从 `data_interface`
  穿进 `fast_model`，扩生成器签名。留作后续，不在本次范围。
- **让 `ensure` 直接调用生成器**：拒绝，因为会绕过 durable pending、revision 收口和
  批次持有的根锁，形成第二套故障恢复语义。

## 后果

- N 个 fresh 根的正常路径只做一次完整生成、一次 AABB 持久化。
- 一批里混进一个坏根，代价是 1 次批量失败 + N 次逐根重跑。准入规则（`attempts == 0`
  且 refno 可解析）把已知会失败的根挡在批外，正是为了让这种情况罕见。
- 合批持锁把「本批待重生成的根」正确标成 in-progress，顺带堵上一个既有缺口：
  逐根跑法下，批次还没跑到某个根时 `ensure` 会认为它的旧实例可用，
  把一个已知马上要被重写的陈旧模型当成 `AlreadyAvailable` 交出去。
- 代价：同步 `ensure` 可能等待正在执行的整批；HTTP 超时只结束本次等待，不取消 durable
  pending 或后台执行。等待预算耗尽返回 `202 generation_pending`，客户端以非 force 请求
  重查同一根，不用 409/504 表示仍在正常推进的工作。
- 行为差异：`ModelUnitResult.attempts` 改从队列行读。`render_upsert` 在更新会话触及同一目标时
  会重置 `attempts`，而旧的 `merge_unit_worklist` 保留累计值——同一场景下上报值会从累计值变成 0。
