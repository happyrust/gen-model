# 方案：空间树一致性闭环（修订版）——单文件快照、状态机与写路径全覆盖

状态：已评审定稿（吸收 2026-08-12 审核结论 A1–A4 / B1–B6，决策 D1–D8 已定）
日期：2026-08-12
关联：`docs/2026-08-11_spatial-tree-startup-init-plan.md`（启动分层判据，已实施，被本方案扩展）、
`docs/plans/2026-08-12-spatial-tree-direct-mutation-epoch-trace-plan.md`（**被本方案吸收取代**，见 D7）、
ADR-010 §4 / 增补 4、ADR-017 §5、`docs/2026-08-12_room-scoped-drain-commit-lock-assessment.md`

## 0. 相对原草案的已定修订（D1–D8）

- **D1（A1）树口径**：每个 refno 恰一条 current `inst_relate` 行。重建与覆盖率查询显式排除
  版本化历史行（数组 id，`!type::is::array(record::id(id))`——`fn::backup_data` 遗产，本仓
  管线已不产生但函数仍装在库里、老库可能有存量）；原草案"同一 refno 多实例不去重"删除：
  树条目键是折叠后的基础 `RefU64`（`RStarBoundingBox::new` 内部 `.refno()`），`sync_refnos`
  会把同 refno 的堆叠折叠成一条，多实例口径与数据结构自相矛盾。不变量
  `tree_entries == usable_pointer_rows` 在 current-only 口径下成立。
- **D2（A2）真值表修正**：pending 优先**仅在快照可读且校验通过时**成立；快照缺失/损坏一律
  `Rebuilding`（重建读已提交指针，已含意图效果，重放随后幂等销账）。启动进入
  `ReplayRequired` 时**立即**执行一次空间收敛，不等 worker 派发门——否则 `queue_paused`
  部署下 Ready 永远不来。
- **D3（A3）迁移处置**：V2 快照发布成功后**删除**旧 `accel_tree_{project}.bin` 与
  `.meta.json`。旧二进制对 bin 缺失是无条件指针重建，任何回退场景自动安全（消除
  "回退旧版本 + 恰有 pending → HealByReplay 复用任意陈旧文件"的静默复活窗口）；
  `AIOS_FORCE_SPATIAL_REBUILD=1` 降级为 runbook 兜底。
- **D4（A4）重建协议**：分页读在锁外；换树 + 终局 stamp 比对 + 快照发布在
  `SPATIAL_STATE_SERIAL` 锁内；漂移整轮重来，≤3 次后 `DegradedBlocked`。stamp 校验因此
  真正有效（锁外读期间的并发写可被检出），后台退避重试不再持锁扫全表阻塞 staged 提交。
- **D5（B1）Ready 语义**："无未知漂移；已知 pending 由消费前重放收口"（即现有派发门语义）。
  staged 尾事务提交**不**翻状态；`ReplayRequired` 仅是启动/复检的瞬态。重放成功
  （`reconcile_spatial_pending`）时若当前态为 ReplayRequired，晋升 Ready/ReadyEmpty。
- **D6（B2）锁范围**：staged 路径只在 [提交后收敛 → 发布] 段持 `SPATIAL_STATE_SERIAL`，
  journal 写回与尾事务不持（尾事务不动树，崩溃安全靠 pending 行）。durable direct 路径
  取锁点在读输入之前（先于既有树写锁——`occ_generate.rs` 的同 refno 乱序防护要求锁跨
  读输入段）；普通直写锁跨 [事务→树同步]。锁序恒为
  `STAGED_COMMIT_SERIAL → SPATIAL_STATE_SERIAL → GLOBAL_AABB_TREE`，源码钉住。
- **D7（B5）吸收直写 epoch 方案**：2026-08-12 trace plan 的 H1（删除分支无痕迹）/
  H2（普通直写不 bump）修复、present 探测门控（树上本来没有就不 bump）、按块 bump
  （其 D2/D3 决策）并入本方案阶段 2；该文档标注 superseded。
- **D8（B6）分页语法前置验证**：优先 record-range 形式（`inst_relate:⟨cursor⟩..`，天然按
  id 有序、免 ORDER BY，与现分页同一"自然序"假设），在 `src/test/fork_surreal_compat.rs`
  钉住 fork 上的页间稳定性与数组 id 排除；`WHERE id > $cursor ORDER BY id` 仅在 range
  语法不可用时评估。

## 1. 核心不变量

- `Ready` = 内存树与库指纹间不存在**无法解释**的差异：树 = 某次校验通过的快照/重建产物
  + 其后经串行锁完成的增量同步；已提交而未重放的空间意图（pending 行）是**已知**差异，
  由消费者门前的重放收口。
- staged 路径不变：意图行 + epoch bump + 水位同一尾事务原子提交（指针写在窗口 journal
  批次、先于尾事务）；提交后收敛在 `STAGED_COMMIT_SERIAL` 内进行。
- 非 staged 路径：凡使"树应有内容"变化的已提交变更，必在同一事务 bump `spatial_epoch`，
  并在空间串行锁内完成 [DB 事务 → 内存树同步 → 标脏]。
- 房间等空间消费者仅在 `Ready/ReadyEmpty` 运行（错误码 `SPATIAL_TREE_NOT_READY`，durable
  房间任务保留待重试）；解析、模型生成、durable 重放、指针重建、`model.spatial.bounds`
  DB 直查不受门禁。

## 2. 状态机

```text
Uninitialized / Loading（进程内瞬态）
Ready / ReadyEmpty（可消费；ReadyEmpty = 校验成功且 usable 指针为 0）
ReplayRequired（快照可用 + 有 pending；启动立即重放，成功即晋升）
Rebuilding（快照缺失/损坏/失配无 pending；重建成功晋升）
DegradedReuse（DB 指纹暂不可读、快照可用；revalidator 复检）
DegradedBlocked（重建连续失败/漂移；revalidator 退避重试）
```

启动判据（快照 = V2 单文件；阶段 1–3 期间暂为旧 bin+meta）：

```text
0. 显式夹具标记 + 树非空            → preloaded（仅测试装载模式；生产入口一律校验）
1. AIOS_FORCE_SPATIAL_REBUILD 真值   → Rebuilding
2. 快照缺失/损坏/校验失败            → Rebuilding（无论 pending）
3. 快照可用：
   3a. 读库指纹失败                  → DegradedReuse（复用快照 + 告警）
   3b. 有 pending（读失败按有算）     → ReplayRequired：复用快照，立即重放一次；
                                        成功 → Ready/ReadyEmpty；失败留态给派发门重试
   3c. 指纹双字段一致                → Ready（快路径）
   3d. 失配且无 pending              → Rebuilding
4. 重建：成功且 usable>0 → Ready；usable==0 → ReadyEmpty；失败/三次漂移 → DegradedBlocked
```

## 3. V2 单文件快照

```rust
struct SpatialTreeSnapshotV2 {
    format_version: u32,        // 2
    project: String,            // DbOption.project_name
    namespace: String,          // DbOption.surreal_ns
    epoch: u64,
    db_epoch_updated_at: String,
    entries: u64,
    usable_pointer_rows: u64,
    invalid_pointer_rows: u64,
    tree_sha256: String,        // 对 tree_bytes 的 SHA-256（sha2 crate）
    saved_at_unix: u64,
    tree_bytes: Vec<u8>,        // AccelerationTree 的 bincode（避免双重序列化歧义）
}
```

- 文件 `accel_tree_{project}.snapshot`；发布 = 序列化整个结构 → `.tmp` 写入 + `sync_all`
  → 原子 rename（沿用 `write_file_atomic`）。
- 接受条件：完整反序列化 + `format_version==2` + project/namespace 匹配 +
  `sha256(tree_bytes)==tree_sha256` + `tree.size()==entries`。
- 发布门：仅 Ready/ReadyEmpty（及重建/重放路径自身）允许发布；发布失败保留 pending 与脏标记。
- 迁移：V2 缺失 → 读旧 bin+meta；双指纹匹配且无 pending → 封装发布 V2、**删除旧文件**、
  verdict=`migrated`；其余情况 Rebuilding。任何一次 V2 发布成功都顺手删除残留旧文件。

## 4. 可验证指针重建

```text
attempt ≤ 3:
  stamp_before = 读 (epoch, updated_at)          # 锁外
  分页读全部指针（record-range 分页，锁外）：
    排除数组 id 行、in.deleted == true 行、world_trans.d/aabb.d 缺失行
    Rust 侧排除 NaN/Inf/反向 AABB（计数 + ≤10 样本）
  取 SPATIAL_STATE_SERIAL:
    stamp_after = 读 (epoch, updated_at)
    stamp 变了 → 放锁重来
    换树（bulk load）→ 校验 entries == usable → 发布快照 → Ready/ReadyEmpty
失败/三连漂移 → DegradedBlocked（revalidator 退避重试）
```

锁外读期间的 staged journal 写回（不 bump、bump 在尾事务）可能让扫描读到半窗口指针；
其空间意图必在尾事务留 pending，随后重放按已提交指针把这些 refno 追平——与 D5 的
"已知 pending 收口"一致，无需扩大锁。

覆盖率分母（`usable_aabb_pointer_count`）同步 current-only + 未删除口径；
"AABB 有效"不进 SQL（SurrealQL 判不了 NaN），invalid 极少 + 10% 容差吸收。

## 5. 统一空间修改协议（吸收 trace plan）

- `durable_room_trigger` 只决定"要不要随事务发布房间任务"；只要指针实际变化
  （`chunk_changes` 非空）就同事务 bump epoch（H2）。
- 重算值与树逐位相等的块：普通写、不 bump（库侧语义未变，不作废他人快照）。
- 删除分支（H1）：锁下 present 探测，非空才 [房间边删除 + bump] 同事务，随后摘树标脏；
  present 为空只删边不 bump。
- 全量生成按块 bump（同 H2 路径），收尾发布一次快照。
- `reconcile_spatial_pending` 顺序不变：按 updated_at、id 读取 → 跨任务后者覆盖 →
  同任务 refresh/remove 冲突维持**渲染期拒绝**（现状）→ 更新树 → 发布 → mark_done；
  发布失败保留 pending 与脏位。

## 6. 降级恢复与消费者门禁

- revalidator：只管 `DegradedReuse`（复检指纹与 pending → Ready/ReplayRequired/Rebuilding）
  与 `DegradedBlocked`（重试指针重建）；30s 起指数退避至 5min；恢复 Ready 后唤醒 scheduler。
  Ready↔pending 往返仍归派发门，避免三处重试抢活。
- 门禁点：启动全量房间重建（`ensure_room_tree_coverage` 前）、`drain_rooms*`（RoomRecalc
  面板/元素消费）、空闲房间轮；被门禁返回 `SPATIAL_TREE_NOT_READY`，durable 行保留。
- 继续放行：文件扫描/解析/入队、模型生成与指针更新、durable 空间重放、指针重建、
  `model.spatial.bounds`。

## 7. 接口与可观测性

不新增 HTTP 路由。`/api/v1/health` 的 `spatial_tree` 换新形状（**九键契约作废**，台账
G-02 同步迁移；`pending` 权威仍是 `spatial_reconcile.pending`，此处为同源镜像）：

```json
{
  "state": "ready", "ready": true,
  "startup_verdict": "reused|migrated|replayed|rebuilt|degraded|preloaded|unknown",
  "format_version": 2, "entries": 62764,
  "usable_pointer_rows": 62764, "invalid_pointer_rows": 0, "pending": 0,
  "file_epoch": 1236, "db_epoch": 1236, "drift": false,
  "snapshot_sha256": "…", "last_verified_at": "…",
  "last_rebuild_attempts": 0, "last_error": null
}
```

Python `spatial.persist/rebuild/reconcile` 复用同一串行锁、状态机与发布实现；
`persist(force=True)` 在非 Ready/ReadyEmpty 拒绝。

## 8. 测试与验收

- 单测：启动真值表（含 D2 交叉项：快照损坏+pending → Rebuilding）、V2
  round-trip/哈希失败/截断/错项目/错 namespace、迁移矩阵、分页无漏无重 +
  版本化行排除（fork 兼容套件）、页间漂移重试、pending 顺序与幂等、
  源码钉（锁序、指针事务含 bump、present 门控、发布门、stamp 顺序）。
- 崩溃注入：env 门控 fail-point（`AIOS_FAILPOINT=<name>` 时显式 abort）五处——
  ①直写 DB 提交后树同步前；②树更新后发布前；③`.tmp` 写完 rename 前；④发布后
  pending 销账前；⑤重建分页中途（配合 epoch 注入测漂移重试）。重启后收敛到同一
  规范化树集合（entries/usable 口径 + 逐边对拍；**载荷字节 SHA 不作跨进程对拍
  凭据**——`AccelerationTree` 序列化含 HashMap 段，迭代序随每进程 SipHash 种子
  变化，`tree_sha256` 只护单文件完整性自校验。2026-08-12 沙箱实测确认，见验收
  记录 §2）。
- AMS/8000：usable==entries==snapshot 三方相等；正常重启走 V2 快路径不扫全表；
  删/截断/伪造旧 epoch 快照自动恢复；E3D TTY 复制恢复 `=24384/24776` 后增量树与
  强制重建集合相等；房间增量/全量对拍一致，pending/dead-letter/spatial reconcile
  归零；记录加载、重建、发布耗时与峰值内存进现有验收目录。

## 9. 实施顺序

1. 状态机 + 消费者门禁 + D2 真值表 + 启动立即重放 + health 新契约（快照仍为旧格式）。
2. 写路径 epoch 全覆盖 + `SPATIAL_STATE_SERIAL` 纳入全部写路径与 Python API（吸收
   trace plan；先于重建协议落地，过渡期不留"不 bump 的直写"）。
3. 重建协议（record-range 分页、口径过滤、锁外读/锁内换树 stamp 校验）+ 覆盖率分母同步。
4. V2 单文件快照 + 校验迁移 + 删除旧文件 + 发布门。
5. revalidator + fail-point 崩溃注入 + 单测矩阵 + AMS/8000 验收 + 文档收尾
   （ADR-010/017 增补、changelog、trace plan superseded、G-02 台账迁移）。
