# 方案：直写路径空间树变更的 epoch 痕迹补齐——消除崩溃后静默漂移

状态：**已落地，后续演进被吸收取代**（2026-08-12；as-built 见文末 §8。
superseded by `docs/plans/2026-08-12-spatial-tree-consistency-closure-plan.md` D7：
空间串行锁 `SPATIAL_STATE_SERIAL`、状态机门禁、V2 单文件快照、record-range 指针
重建在该方案落地，本文的锁纪律与 bump 协议自此以那份方案为唯一事实来源）
日期：2026-08-12
关联：`docs/2026-08-11_spatial-tree-startup-init-plan.md`（启动分层判据，已实施）、
ADR-010 §4（删除分支）/ 增补 4（epoch 校验）/ 增补 2026-08-12（本方案的 ADR 记录）

## 1. 背景与问题

启动分层判据（2026-08-11 方案 §3）的正确性依赖一条前提：**凡是改变了「树应有内容」的
已提交变更，都会在库侧留下可检测的痕迹**——要么 `spatial_reconcile` 意图行（走 3b
重放自愈），要么 epoch bump（走 3c 指针重建）。该方案 §3 的论证原文是「直写路径不产生
意图，其崩溃丢失自然落入 3c 被重建接住」。

但这句论证只对**会 bump epoch 的直写分支**成立。盘点全仓 `GLOBAL_AABB_TREE` 的
写变更点，有两处既不写意图行、也不 bump epoch：

- **H1 直写删除分支**（`src/data_interface/helper.rs:350-374`
  `delete_room_membership`，窗口外分支）：先幂等删 `room_relate` /
  `room_panel_relate` 边，然后 `remove_by_refnos` 摘内存树 + 标脏。库侧无任何痕迹。
- **H2 普通直写刷新分支**（`src/fast_model/occ_generate.rs:1172-1190`，
  `update_inst_relate_aabbs_by_refnos_mode` 的 `durable_room_trigger=false` 且无窗口
  分支；全量生成与 `manual_update_aabbs` 走这里）：指针 UPDATE 直接执行，随后
  `sync_refnos` + 标脏。不 bump epoch。

两处的共同失效形态（`batch_worker.rs:2342-2346` 注释自己承认「直写路径的变更没有
epoch 痕迹，仍要靠这里的落盘闭环」）：

> 变更已提交进主库 → 任务标记完成（不再重放）→ 内存树已同步、仅剩脏标记 →
> **空闲轮落盘前进程崩溃** → 重启时 sidecar 指纹与库 epoch **相等** →
> 判据走 Reuse 复用陈旧文件 → 树 ≠ 库，且 /health 的 drift 恒为 false，无人可见。

后果按路径分别是：

- H1：被删构件的旧包围盒留在树上，启动全量房间重建会把它按旧位置**重新收编**进
  `room_relate`——这正是 ADR-010 D4 修掉的缺陷借崩溃复活，且 DeleteCleanup 任务已
  done，没有任何重放会再清一次。
- H2：全量生成中途崩溃时，「单元已 done、树文件未落盘」的那部分根，其新包围盒
  永远进不了树（done 的单元不重跑）；树上留旧盒或缺盒，房间候选与射线查询按旧
  几何作答，直到下一次碰巧触发指针重建。

对照组（已经安全、本方案不动）：

- durable 直写分支（`occ_generate.rs:1147-1171`）：`[指针 + room_recalc + epoch bump]`
  同事务，且写锁跨「读输入→事务→树同步」防盖章竞态——崩溃后指纹失配落入 3c ✅。
- 暂存窗口路径：意图行 + bump 随尾事务原子提交，提交后收敛靠重放，落盘后才销账 ✅。
- 启动重建 / 提交后收敛（`aabb_tree.rs`）：自身就是自愈动作 ✅。

## 2. 目标与非目标

- G1 建立并钉死不变量：**直写路径凡使「树应有内容」发生变化的已提交变更，必在
  同一事务内 bump spatial epoch**——使一切崩溃丢失都可被启动判据检出（最差落入
  3c 重建，绝不静默 Reuse）。
- G2 常态运行零新增重负载：不改落盘机制（脏位 + 空闲轮 + 原子写）、不改启动判据、
  不加新表新字段。
- G3 修复后 durable 分支与普通直写分支在「事务化 + bump + 锁纪律」上收敛成同一形状，
  减少 `update_inst_relate_aabbs_by_refnos_mode` 的分支心智负担。
- 非目标：不改暂存窗口路径；不做 GLOBAL_AABB_TREE 锁粒度优化（单列后续工作）；
  不引入 SQLite 持久化（2026-08-12 已另行分析定案保留内存方案）；不动
  `sync_aabb_tree_with_db` / `manual_update_aabbs` 的人工工具定位。

## 3. 核心设计

### 3a. 不变量（一句话）

> 直写路径动树之前，先让库里「说得出树该变」：变更与 epoch bump 同事务提交，
> 事务成功后才推进内存树；崩溃恢复统一交给启动判据（失配 + 无意图 → 指针重建）。

恢复通道刻意选 **bump-only（3c 重建兜底）** 而不是给直写路径补 `spatial_reconcile`
意图行（3b 重放自愈）：后者恢复更便宜，但需要为非窗口来源发明意图行 id 方案
（现有 id 是 `spatial_reconcile_{dbnum}_{end_sesno}`，窗口专属）并把 (dbnum, sesno)
一路下传到 `helper.rs`；而 2026-08-11 方案 D 系列已裁决「直写崩溃 → 重建接住」，
重建只读、分页、量级已实测（产物 4.8MB 级），崩溃窗口本身是小概率事件。
维持已批准的恢复通道，只补齐「可检测」这一缺失环节。——见决策点 D1。

### 3b. H2 修复：普通直写分支事务化（`occ_generate.rs`）

`update_inst_relate_aabbs_by_refnos_mode` 直写侧（无窗口）统一为：

```text
chunk_changes 非空（本块确有包围盒变化）：
    statements = [update_sql?, room_upserts?, epoch bump]
    → wrap_in_transaction → execute_surreal_checked → 成功后 sync_refnos + 标脏
chunk_changes 为空且 update_sql 非空（重算值与树上旧值逐位相等）：
    维持现状普通写，不 bump（库侧语义未变，无需作废别人的文件）
```

即把 `occ_generate.rs:1147` 的事务条件从 `durable_room_trigger && !chunk_changes.is_empty()`
放宽为 `!chunk_changes.is_empty()`；`room_upserts` 仍由
`durable_room_trigger && room_incremental()` 门控，`durable_room_trigger` 从此只决定
「要不要随事务发布房间任务」，不再决定「要不要事务与 bump」。

锁纪律（防「空闲轮把旧树盖上新 epoch 章」的盖章竞态，与 durable 分支同源）：
普通直写分支在**事务执行前**取得 `GLOBAL_AABB_TREE` 写锁并持有到 `sync_refnos`
结束。刻意**不**把锁提前到读输入段（durable 分支是全跨度）：全量生成的读输入段
含几何 join，最贵；镜像一致性只要求锁跨 [事务→树同步]——两个并发块即便乱序，
后提交者的指针与树条目仍然成对，树 == 库不被破坏。——见决策点 D2。

### 3c. H1 修复：删除分支带痕迹（`helper.rs`）

`delete_room_membership` 窗口外分支改为（每 chunk）：

```text
1. 取 GLOBAL_AABB_TREE 写锁；
2. 锁下探测 present = refnos ∩ 树现有条目；
3. present 为空 → 只执行现有房间边删除语句（无 bump，树本来就没这些条目）；
   present 非空 → [room_relate/room_panel_relate 删除语句 + epoch bump]
   wrap_in_transaction 原子执行；
4. 事务成功后 remove_by_refnos + 标脏；释放锁。
```

探测放在锁下，保证「要不要 bump」与「树到底动没动」由同一快照裁决，不会出现
bump 了却无人落盘追平（无谓触发下次重建）、或动了树却没 bump（回到 H1）的错位。
房间边删除语句本身幂等，DeleteCleanup 任务重试语义不变。

### 3d. 修复后的失效矩阵

| 崩溃时机 | 修复前 | 修复后 |
|---|---|---|
| 直写事务提交后、树同步前 | （H2 无事务概念）| 指纹失配 + 无意图 → 重建 ✅ |
| 树同步后、空闲轮落盘前 | 指纹相等 → 静默 Reuse 陈旧树 ❌ | 指纹失配 + 无意图 → 重建 ✅ |
| 落盘后 | 一致 ✅ | 一致 ✅ |

## 4. 变更清单

### C1 `src/fast_model/occ_generate.rs`
- 直写事务条件放宽为 `!chunk_changes.is_empty()`（§3b），`room_upserts` 门控不变；
- 普通直写分支补写锁跨 [事务→sync_refnos]（新增锁获取点，durable 分支现有的
  1012 行全跨度锁保持不动）；
- 函数文档同步改写（现注释称「全量生成路径不递增 epoch」的两处一并更正：
  本文件与 `aabb_tree.rs:272` persist 文档）。

### C2 `src/data_interface/helper.rs`
- `delete_room_membership` 按 §3c 重写窗口外分支；渲染函数
  `render_room_membership_delete` 增加可选 bump 拼接（或在调用点追加语句后
  `wrap_in_transaction`，实现取简）。

### C3 测试
- 渲染纯函数测试：删除事务渲染含 `spatial_epoch:current` bump 且与边删除同事务；
  直写块事务渲染含 bump（复用 `epoch_bump_targets_the_singleton_record` 手法）。
- 源码钉（回退即红）：
  - 普通直写分支不得存在「chunk_changes 非空却无事务无 bump」的指针直写；
  - `helper.rs` 删除分支 bump 必须在 `remove_by_refnos` 之前且经 `wrap_in_transaction`；
  - 两处锁获取必须先于事务执行（盖章竞态钉）。
- 行为级测试（补 `update_inst_relate_aabbs_by_refnos_mode` 缺失的分支覆盖）：
  - 窗口分支：不动树、不 bump、意图寄存进窗口（现有 4130 附近夹具扩展）；
  - 直写分支：DB 事务失败 → 树保持旧基线（失败原子性）；
  - bump 条件：chunk_changes 为空 → 无 bump；present 为空 → 删除不 bump。
- live 用例（ignore，手动）：直写删除后不落盘即重启 → 断言
  `startup_verdict = rebuilt` 且重建后树中无被删 refno。

### C4 文档
- ADR-010 追加增补：记录「直写无痕迹残余」的关闭，更正 2026-08-11 方案 §3
  「直写崩溃自然落入 3c」论证的适用范围（当时只对 durable 分支成立）。
- `changelog.md` 记录行为变化：全量生成/删除清理的直写提交现在会推进 spatial epoch。

## 5. 决策点（待评审定夺）

- **D1 恢复通道**：推荐 **bump-only（3c 重建兜底）**。备选：给直写删除补
  `spatial_reconcile` 意图行（恢复更便宜、与窗口路径完全同构），代价是发明
  非窗口意图行 id 方案 + (dbnum, sesno) 下传；若评审认为删除频度或重建成本
  值得，可切换，本方案其余部分不受影响。
- **D2 普通直写锁跨度**：推荐 **[事务→树同步]**（不含读输入段），理由见 §3b；
  备选全跨度（与 durable 分支完全一致，代价是全量生成的读输入段串行化）。
- **D3 全量生成的 bump 频度**：推荐**按块 bump**（chunk_changes 非空才 bump，
  一次全量生成约产生 条目数/100 次 bump）。多次 bump 语义无害（判据只比相等），
  但会使「生成中途的空闲轮落盘 + 崩溃」可靠落入重建而不是 Reuse 半新树——这正是
  想要的方向。备选「整次生成只 bump 一次」经论证不可行：空闲轮可能在 bump 后
  落盘一次，使后续块的变更重新回到无痕迹状态。

## 6. 验收

- `cargo test --lib --features http_api` 全绿（含新增渲染/源码钉/行为测试）。
- 场景表逐条过：
  - 直写删除 → 杀进程（落盘前）→ 重启：verdict=rebuilt，树无被删 refno，
    房间重建不再收编幽灵构件；
  - 全量生成中途杀进程 → 重启：verdict=rebuilt，树与库指针一致；
  - 正常增量/全量收尾后重启：快路径 Reuse 不受影响（指纹相等）；
  - 暂存窗口崩溃带意图：仍走 HealByReplay（本方案未触碰）；
  - chunk 全部无变化的重刷：不 bump、不作废他人文件。
- /health：上述崩溃场景重启前 `drift=true` 可见（修复前恒 false）。

## 7. 工作量与风险

- 改动集中：`occ_generate.rs` ~40 行、`helper.rs` ~35 行、测试 ~120 行、文档 ~40 行。
- 风险 1：崩溃后启动多做一次指针重建（只读、分页、秒级）——这是设计意图的
  显性化，不是回归。
- 风险 2：全量生成每块多一次单行 UPSERT 与事务包裹，往返成本可忽略；锁跨
  [事务→同步] 期间树读者短暂阻塞（毫秒级/块），全量生成期间房间重建尚未开始，
  实际读者稀少。
- 风险 3：将来新增直写变更点忘记 bump——用源码钉 + ADR 增补的不变量表述压住；
  /health drift 提供运行期兜底可见性。
- 回退：两处改动各自独立、语句级可 revert；回退即回到「静默漂移」现状，无数据损坏。

## 8. As-built（2026-08-12 落地记录）

三个决策点按推荐项执行：D1 bump-only、D2 窄跨度锁、D3 按块 bump。与方案文本的差异
只有一处，写在这里以免以后对着方案读代码时对不上：

- **D2 的锁跨度实际取 [变更判定 → 事务 → 树同步]，比方案写的 [事务 → 树同步] 宽一格。**
  方案 §3b 的理由（读输入段含几何 join，最贵，不能进锁）完全保留——锁仍在输入查询
  之后取得。多包进来的是变更判定与 `save_aabb_to_surreal`（内容寻址记录的幂等写）。
  这么做是为了让普通直写分支与 §3c 的删除分支遵守同一条纪律：「要不要 bump」与
  「树到底动没动」由同一个加锁快照裁决。判定留在锁外的话，两者之间插进来的并发写
  会让 bump 决策与实际树变化错位，正是 §3c 明确要避免的形态。
- **顺带关闭一个方案没有盘到的交错窗口。** 普通直写分支此前只在 `sync_refnos` 那一
  瞬取锁，于是「读输入 → 同步」之间可以插进一次删除清理：删除先摘掉条目，随后这里的
  `sync_refnos` 把已删 refno 的包围盒重新插回去，成为要等下次指针重建才自愈的幽灵
  条目。锁跨度扩到判定之前后，这个窗口不再成立。
- 测试按 C3 落了渲染纯函数测试与源码钉（含「暂存分支一条 bump 都不许有」这条方案
  未列、但同样会造成「拿未提交的窗口变更作废别人树文件」的反向缺陷）。C3 里的行为级
  测试与 live 用例**未做**：它们要么需要真库，要么需要杀进程后重启，不进 `--lib`。
  `cargo test --lib --features http_api` 661 通过 / 0 失败。
