# 方案：空间树启动初始化——分层判据取代「盲信文件」

状态：已评审通过并实施（两轮 Plannotator + 决策卡确认，2026-08-11 落地）
日期：2026-08-11
关联：ADR-010 增补 4（epoch 校验与指针重建）、v0.1.7 提交 `cd3ea9e9`（默认复用树文件）、
`docs/2026-08-04_dboption-config-changelog.md`（三个启动开关）

## 1. 背景与问题

当前（v0.1.7）启动加载 `load_project_tree_verified`（`src/fast_model/aabb_tree.rs:297`）：

- 默认**无条件复用** `accel_tree_{project}.bin`，sidecar 的 epoch 只打印不比对；
- 只有设置 `AIOS_FORCE_SPATIAL_REBUILD` 才从库指针重建；
- 文件缺失/损坏 → 空树启动，等人工干预。

而 epoch 基础设施**两条写路径都还在维护**（暂存尾事务 `model_update_pending.rs:884-887`、
直写事务 `occ_generate.rs:1144`），sidecar 落盘照常盖章——处于「基础设施在白维护、
消费者被删」的状态。

遗留风险：

- R1 静默陈旧树 + 启动全量房间重建 = 历史「重启回退 room_relate」缺陷的复发向量
  （90% 覆盖率闸门防不了「条数对、位置旧」）。
- R2 直写应急模式（`GEN_MODEL_DIRECT_INCREMENT=1`）不产生空间意图，崩溃丢失的
  内存树变更启动时无人认出——v0.1.7 移除了「直写模式 → 强制重建」联动，
  ADR-010 记录的残余重新敞开。
- R3 文件缺失/损坏不自愈：指针重建又快又只读（本项目实测产物 4.8MB），却要等
  人工设环境变量重启；期间房间任务 fail-closed 积压。
- R4 外部事故（拷旧 bin、库快照回滚、换库同项目名）无检测。
- R5 `AIOS_FORCE_SPATIAL_REBUILD` 用 `is_ok()` 判定：部署模板写 `=0` 想关闭，
  实际**每次启动都强制全量重建**（与 2026-08-08 审核 P2-1 在
  `GEN_MODEL_DIRECT_INCREMENT` 上修掉的是同一类问题）。

历史上放弃 epoch 校验的真实痛点（要在新方案里保住）：崩溃后带着待重放空间意图
（`spatial_reconcile` pending）启动时，epoch 必然失配 → 旧方案每次都触发全量指针
重建，而其实意图重放就能便宜地自愈。

## 2. 目标

- G1 启动能认出陈旧树文件，并以**最小代价**收敛（能重放就不重建）。
- G2 常态启动保持零额外重负载：快路径仍是「读文件直接用」，只多一次 epoch 单行查询。
- G3 崩溃-带意图场景**不**触发全量重建（消除旧方案被废的原因）。
- G4 文件缺失/损坏自动自愈，不再等人工。
- G5 修复 R5 的真值判定。
- 非目标：不改变落盘机制（脏位 + 空闲轮 + 原子写）、不改变 epoch 的写入侧、
  不动 `sync_aabb_tree_with_db` / `manual_update_aabbs` 的人工修复工具定位。

## 3. 核心设计：启动分层判据

一致性指纹（评审反馈 2026-08-11：**要与数据库对时间戳**）：sidecar 不再只存 epoch
数值，改存 **(epoch 值, 库侧该 epoch 的 `updated_at` 时间戳)** 双字段指纹。
`spatial_epoch:current` 每次 bump 都 `SET value += 1, updated_at = time::now()`，
落盘盖章时把两者一并抄进 sidecar；启动时**两个字段都精确相等**才算「对得上」。
两侧时间戳同源于库端时钟（`time::now()`），不存在进程与库之间的时钟偏差问题；
单靠 epoch 数值在「库快照回滚恰好回到同一计数」时会撞值，加上时间戳后指纹碰撞
在实践上不可能。

`load_project_tree_verified` 改为：

```text
0. 内存树非空                      → 保持不动（夹具幂等，现状保留）
1. AIOS_FORCE_SPATIAL_REBUILD 为真 → 指针重建（判定改真值解析，见 §4-C1）
2. 树文件缺失/损坏                 → 指针重建（新增自愈，替代「空树等人工」）
3. 文件可读：
   3a. sidecar.(epoch, epoch_updated_at) == 库.(epoch, updated_at)
                                                → 直接复用（快路径，数值+时间戳
                                                  都对得上才信）
   3b. 指纹失配 且 has_pending_spatial_work()==true → 复用文件；日志说明
       「交给意图重放自愈」（worker 出队前的重放闸门本来就会先跑）
   3c. 指纹失配 且 无待重放意图                 → 指针重建（真正无法解释的漂移：
       直写崩溃 / 换文件 / 回滚库）
   3d. sidecar 缺失/损坏/缺新字段（旧版文件）   → 按失配处理（走 3b/3c，一次性
       自愈后指纹补齐）
4. 读库 epoch 指纹失败（DB 诊断查询挂了）       → 降级复用文件 + 告警
   （文件好过空树；worker 闸门后续兜底）——见决策点 D2
```

正确性依据：`reconcile_spatial_pending` 的顺序是「树同步 → 脏位落盘 → 才 mark_done」
（`side_effect_pending.rs:254-305`），因此**「文件 + 待重放意图」对暂存路径是完备集**；
3b 复用文件后由重放收敛，不丢数据。直写路径不产生意图，其崩溃丢失自然落入 3c
被重建接住——**无需单独恢复「直写模式 → 强制重建」联动**。

指纹时序论证（为何相等即新鲜）：`persist_project_tree_now` 在写文件**之前**读库侧
`(epoch, updated_at)`，写完才盖章——并发 bump 只会让 sidecar 偏旧（下次启动多做一次
重建，方向保守），永远不会把新章盖在旧内容上；这一纪律沿用现状注释并加源码钉。

## 4. 变更清单

### C1 `src/fast_model/aabb_tree.rs`
- 新增 `force_spatial_rebuild_enabled()`：真值解析（1/true/yes/on，忽略大小写与空白），
  复用 / 提取 `batch_worker::direct_increment_flag` 的解析逻辑为共享函数，
  认不出的值按关闭处理并告警一次。
- `read_db_spatial_epoch` 扩展为 `read_db_spatial_epoch_stamp() -> (u64, String)`：
  一并取回 `updated_at`（保持 `value` 反引号保护；记录缺失按 `(0, "")`）。
- `TreeFileMeta` 增加 `db_epoch_updated_at: String`（`serde(default)` 向后兼容；
  旧 sidecar 缺字段 → 反序列化得空串 → 指纹失配 → 走 3b/3c 一次性自愈补齐）。
- `persist_project_tree_now`：写文件前读 `(epoch, updated_at)` 双值，盖章时一并写入。
- `load_project_tree_verified` 按 §3 重写；每个分支打一行带原因的日志
  （复用/重放自愈/重建 + 指纹两侧的值）。

### C2 启动调用点语义统一（`src/lib.rs:393` 与 `:578`）
- 两处统一为：`load_project_tree_verified` 返回 Err 时**告警 + 空树继续启动**
  （空树有下游防线：全量重建拒跑、整间分支拒算、覆盖率闸门）。
  `run_cli` 前那处从 `?` 改为与 `run_app` 相同的告警兼容。——见决策点 D3

### C3 可观测性（`src/web_service/handlers.rs` /health）
- `spatial_reconcile` 字段旁新增 `spatial_tree`：
  `{ file_epoch, file_epoch_updated_at, db_epoch, db_epoch_updated_at,
  drift: bool, entries, saved_at, startup_verdict }`
  （startup_verdict = reused / healed_by_replay / rebuilt / empty，进程内记录一次）。

### C4 测试
- 更新源码钉 `startup_reuses_project_tree_unless_rebuild_is_explicitly_requested`
  → 改为钉新分层：快路径必须比双字段指纹；失配+意图必须复用不重建；失配+无意图
  必须重建；文件损坏必须重建；默认路径仍不得出现 `sync_aabb_tree_with_db` /
  `manual_update_aabbs`（重算重写路径继续禁止）。
- 新增单测：`force_spatial_rebuild` 真值表（unset/0/false/off → 关；1/true/yes/on → 开）。
- 新增单测：分层判据纯函数化（抽 `startup_verdict(meta, db_stamp, has_pending) ->
  Verdict` 纯函数，真值表直接测：数值等+时间戳等 → 复用；数值等+时间戳不等 →
  失配；旧 sidecar 缺时间戳 → 失配；……），IO 壳保持薄。
- 新增源码钉：盖章前读指纹的顺序（读 `(epoch, updated_at)` 必须在写文件之前，
  防「新章盖旧内容」回归）。
- `tree_meta_roundtrip` 扩展双字段；补「旧格式 JSON（无新字段）反序列化 → 空串」用例。
- live 用例（ignore，手动）：制造 sidecar 失配 + 无意图 → 断言走指针重建且落盘后
  指纹追平。

### C5 文档
- ADR-010 追加一节增补（记录 v0.1.7 的摇摆与本次分层定案，含 3b 的完备集论证）。
- `changelog.md` 记录行为变化与 R5 修复。
- `docs/2026-08-04_dboption-config-changelog.md` 的三开关说明同步更新
  （`AIOS_FORCE_SPATIAL_REBUILD` 语义从「存在即触发」改为「真值触发」，
  部署模板需检查）。

## 5. 决策点（已定夺，2026-08-11 决策卡确认）

- **D1 文件缺失/损坏 → 自动指针重建**：✅ 接受自动重建——只读、分页、量级已实测；
  「等人工」期间房间队列积压更贵。
- **D2 读库 epoch 指纹失败的降级方向**：✅ 复用文件 + 告警（启动可用性优先；
  worker 出队前的意图闸门后续兜底）。
- **D3 两处启动调用点统一为「告警兼容」**：✅ 统一——空树有下游防线，启动不应被
  派生数据加载失败阻断；`run_cli` 前那处从 `?` 改为告警继续。
- **D4 逃生舱环境变量**：✅ 不加，保持判据单一；误触发重建有日志可定位且代价只是
  一次只读重建，非破坏性。

## 6. 验收

- `cargo test --lib --features http_api` 全绿（含更新后的源码钉与新真值表）。
- 场景表逐条过：正常重启（快路径复用，指纹双字段相等）、崩溃带意图（复用+重放
  自愈）、直写崩溃（失配无意图 → 重建）、删 bin（重建）、删 sidecar（按失配走）、
  库快照回滚且 epoch 数值恰好撞回（时间戳对不上 → 判失配，不再放行）、
  旧版 sidecar 无时间戳字段（一次性按失配自愈后补齐）、
  `AIOS_FORCE_SPATIAL_REBUILD=0`（不重建）/`=1`（重建）。
- /health 能看到 `spatial_tree.startup_verdict` 与 drift（含两侧指纹时间戳）。

## 7. 工作量与风险

- 改动集中：`aabb_tree.rs` ~120 行（含纯函数抽取）+ `lib.rs` ~10 行 +
  `handlers.rs` ~20 行 + 测试 ~100 行 + 文档。
- 风险：3c 误判触发重建（例如 epoch 写侧将来出现遗漏 bump 的新路径）→ 代价是
  一次只读重建，非破坏性；日志 + /health 可定位。
- 回退：整个分层判据收敛在一个函数与一个纯函数里，revert 即回 v0.1.7 行为。
