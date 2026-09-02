# 2026-09-01 方案：refno 范围的模型对账与后台补齐（reconcile）

关联：ADR-025（严格初始化相位 · 模型门）、ADR-050（`model_update_pending` 进程本地）、
ADR-015（pending 工作身份）、ADR-011（入队回执 · 进度去任务队列看）、
`resource/surreal/gen_root.surql`（生成根覆盖与凭证）、plant-ui ADR-0019（保存与时刻口径）、
`2026-08-30-core-dll-api-alignment.md` 与 `2026-08-31-core-aligned-increment-architecture.md`
（core 对齐系列；本方案的原生对标证据见文末附录）。

状态：**提案（未实施）**。一期不改任何既有判据与管线语义，纯组装现有件；
二期动判据，必须与 ADR-025 联动修订。

## 要解决的是什么

用户视角一句话：指定一个 refno（容器也行），把它下方「该有模型的」refno 全部核一遍——
**没生成过的**、以及**数据已更新而模型还停在旧会话号（sesno 落后）的**，只把这两类排进
后台生成；已经最新的一个不碰。要能先预览再执行，执行走后台，进度可看。

今天做不到的三点：

1. **没有 refno 范围的入口。** 库级有（启动 `reconcile_model_coverage_at_startup`、
   人工 `POST /model/rebuild`），单根与容器范围有同步 ensure（`/api/v1/model/ensure`、
   `ensure_model_scope_generated`），但 ensure 是同步 HTTP、120 秒预算，大范围会超时，
   且它对过期根**当场重生成**而不是入队后台。
2. **时效判据是库级水位等值**，粒度粗（见「现状判据及其毛病」）。
3. **没有预览。** ensure 只有干与不干，不回「哪些落后、哪些缺失」的清单。

## 地基：已经存在的四件设施（一期只做组装）

| 设施 | 位置 | 现职 |
| --- | --- | --- |
| `gen_root` 凭证表 | `resource/surreal/gen_root.surql` | 每生成根一行：`pe` / `dbnum` / `kind(mdu\|residue)` / `noun` / `subtree` / `status` / `source_end_sesno(_time)` / `publication_*` |
| `dbnum_watermark` | `data_interface/dbnum_state.rs` | 每库已应用水位 `applied_sesno`（+ 写入时刻） |
| `model_update_pending` | `data_interface/model_update_pending.rs` | durable 队列：record id = `action_target`，UPSERT 幂等、revision 复活死信、`MAX_ATTEMPTS = 5` |
| 空闲轮 drain | 同上 `drain_where_cooperative` | 后台消费：每页 100 根批量生成、根锁 DeferBusy、panic 记账、收口回写 `gen_root` 凭证 |

「该有模型的 refno」的枚举也是现成的：

- 库级 `fn::gen_root_cover($dbnum)`：MDU 根（BRAN/EQUI/HANG/SUPPO 最外层）+ residue 根 +
  断头节点；WORL/SITE/ZONE 与 pointish noun 永不成根；
- 任意子树 `resolve_generation_roots_on(子树)`（`generation_root.rs`，与 ensure / 删除共用同一策略）；
- 「已生成」集合 `inst_relate.anc CONTAINS root`（plant-ui 的 `generated_scope` 同款）。

## 现状时效判据及其毛病（按代码推演，未跑库验证）

`generation_root_cache_current` 的命中条件（`model_update_pending.rs`）：

    status ∈ {Generated, AlreadyAvailable, NoRenderableGeometry}
    ∧ source_end_sesno == 所在库 dbnum_watermark.applied_sesno
    ∧ source_end_sesno_time 一致

而收口尾事务（`render_finalize_tail_with_effects`）只为**本窗口变化根** upsert 工作项，
未变根的凭证不动。于是水位一动：

- ensure 对未变根判「过期」→ 当场重生成（无谓算力；unchanged manifest 的发布虽是廉价
  路径，几何计算本身还是付了）；
- 下次启动 `sync_and_seed_model_coverage` 会把整库凭证 ≠ 当前水位的根**全部**重排。

验证方式（落地前应做一次）：任意有存量凭证的库应用一个窗口后重启，看启动日志
「模型完整性扫描 dbnum=… 新排队=」那一行，若数量 ≈ 全库根数 − 本窗口变化根数，即坐实。

这正是「sesno 落后才生成」的直觉要补的洞。

## 原生对标：core.dll（dabacon）怎么做同一件事（逆向硬证据）

四个事实（证据地址见附录）：

1. **会话即版本快照，旧会话永远可读。** `.db` 文件 append-only，每次 SAVEWORK 产生一个
   session；`switchToOldSession / switchBackSession` 在任意会话视图间切换。gen-model 解析
   .db 会话窗口，就是这套机制的离线重放。
2. **变更检测 = 两个会话的元素索引表并行 diff，不是查每元素字段。**
   `DB_RawChanges::createListBetweenSessions` → `DB_IndexTableCompare`（dab
   `db_start_table_sesn_comp`）→ 逐条 `db_get_next_int_table_diff`，按状态码分
   **modified / inserted / deleted** 三桶。索引 entry 是存储定位对：元素改写 ⇒ 新页拷贝 ⇒
   entry 变 ⇒ modified；**删除 ⇒ entry 消失 ⇒ 天然可判**。
3. **单元素时效是同一数据的点查。** `DB_Element::hasChangedBetweenSessions(s1, s2)` → dab
   `db_element_changed_between_sesns`（DCHELE）；另有属性级 `db_comp_att_through_sesns`
   （DCMATS）与元素级 `db_comp_element_through_sesns`（DCMELS）。
4. **下游更新靠进程内事件即时消化，E3D 没有任何「生成状态缓存表」。**
   getwork / savework / flush 时 `createGetworkEvents` 把 RawChanges 装进事件广播给同进程
   模块就地更新。**它不需要缓存表，是因为下游又快又在进程内；我们的下游（模型生成）异步
   且昂贵，凭证表 + durable 队列才是正确形态——别在 E3D 里找对标物。**

对本方案的推论：

- 镜像里的 `pe.sesno`（每元素最后写入会话号）是对原生索引表的物化等价物；
- 「凭证 sesno ≥ root 子树 max(pe.sesno) ⇒ 最新」语义上等于对子树成员逐个
  DCHELE(凭证 sesno, latest)——判据方向与原生一致；
- 删除可判性在原生是免费的；镜像侧已核验为「软删墓碑 + sesno 前移」（见
  「删除路径核验」节），不需要新机制。

## 判据表（reconcile 的分类口径）

记号：对范围内解析出的每个生成根 `r`，`d = r.dbnum`，`wm(d) = dbnum_watermark:d.applied_sesno`，
`cred = gen_root:{r}` 行，`data(r) = r 子树成员（含 deleted = true 行）的 max(pe.sesno)`，
终态 = `{Generated, AlreadyAvailable, NoRenderableGeometry}`。

| # | 状态 | 判据 | 动作 |
| --- | --- | --- | --- |
| 1 | 在队 | `model_update_pending` 有该根 `regen_root` 行且 `attempts < 5` | 不动；预览单列 |
| 2 | 死信 | 同上但 `attempts ≥ 5` | enqueue 时按 revision+1 复活（`render_upsert` 既有语义） |
| 3 | 未生成 | 无 `cred` 行；或 `cred.status` 非终态且 `inst_relate` / `tubi_relate` 无产物 | 入队 |
| 4 | 最新 | `cred.status ∈ 终态 ∧ cred.source_end_sesno ≥ data(r)` | 跳过 |
| 5 | 落后 | `cred.status ∈ 终态 ∧ cred.source_end_sesno < data(r)` | 入队 |
| 6 | force | `force = true` 时 3 / 4 / 5 全入队并清零 attempts（与 `/model/rebuild` 的 `force_all` 同语义） | 入队 |

关键选择：**一期 reconcile 用 `data(r)` 判据（精确），不动 ensure 与启动 seed 的水位等值
判据**——reconcile 是新入口，用更精的判据不影响任何既有路径；把既有路径也切过来是二期（Q2）。

兜底纪律（宁枉勿漏）：

- `pe.sesno` 缺失（旧行；库里到处是 `sesno ?? 0` 的兜底）：该成员判不了 → 整根按**落后**入队。
- `wm(d) = 0`（需初始化库）：整根跳过，回执 `warnings` 点名，先走初始化。
- `anc` 未回填：入口探测 `inst_relate_anc_ready`，响亮失败（与模型查询同一句话）。
- 正在生成（`generation_root_lock` 被占）：入队仍安全（UPSERT 幂等），drain 侧 DeferBusy。

## 删除路径核验（原开放问题 Q1，2026-09-01 已答）

**三种窗口操作的落库语句都盖 `sesno`**（`vendor/old-pdms-io/src/io.rs`，
`EleOperationDetail::to_surql` / `ModifiedElement::to_modify_surql`）：

- Add：`UPSERT pe:{id} CONTENT {...}`，CONTENT 里带 `sesno = 本会话号`；
- Modified：语句固定以 `UPDATE pe:{id} SET sesno = {sesno}` 开头再 MERGE 字段；
- **Deleted：`UPDATE pe:{id} SET deleted = true, sesno = {sesno}`——软删墓碑保留，
  且会话号前移到删除它的那个会话。**

物理删 `pe` 行的只有三个运维路径（`fast_delete.rs`：整库快删、回退重建清库、按水位
裁剪），增量窗口不物理删行；`versioned_db/member_prune.rs` 只裁全量解析的内存成员树，
不碰库里的墓碑。所以墓碑上的删除信号不会随时间衰减。

两个必须写进实现注释的细节：

1. **墓碑可能从树上摘链**：删除元素的同窗口里，幸存 owner 的成员表重建
   （`DELETE pe:{owner}<-pe_owner` + 重插新 children）会把指向墓碑的边一并去掉，
   之后按 `pe_owner` 走子树就够不着它。因此 `data(r)` 的删除可见性**不依赖扫到墓碑**：
   删除必然伴随幸存 owner 的 Modified（成员表变化），owner 自己的 `sesno` 同窗口前移，
   子树 max 由它带动。「含 deleted 行一起算」仍保留——多算无害，墓碑未摘链时它是第一手信号。
2. **根整体被删**：`fn::sync_gen_roots` 会把不再成根的行清掉
   （`delete gen_root where dbnum = $dbnum and pe not in $keep`），该根从「该有模型」
   枚举中消失；残留产物由主路 DeleteCleanup（`delete_inst_relate_subtree`，含软删子树
   收集）清理，**不归 reconcile 管**。可选增强：preview 顺带报告「产物在、pe 已软删」
   的孤儿计数（`inst_relate.anc` 范围 + `in.deleted` 谓词，只报数不动手）。

结论：precise 判据对「新增 / 修改 / 删除」三类变化都成立，Q1 的两个备选补救
（删除时 bump 根 data_sesno / 删除类退回水位判据）都不需要。

## API 契约

    POST /api/v1/model/reconcile

请求体：

    {
      "refno": "24383/66460",          // 容器合法：本来就是范围操作
      "project": "ProjAMS",
      "mdb": "/ALL",
      "namespace": "hd",               // 三件套与 ensure 同口径同校验（不符 → 422 identity_mismatch）
      "mode": "preview" | "enqueue",
      "force": false,                   // 可省，默认 false
      "precise": true                   // 可省，默认 true；false 退回水位判据（对拍/旧行库用）
    }

回执（两种 mode 同构；`enqueued` 在 preview 恒 0）：

    {
      "refno": "24383/66460",
      "generation_root_count": 212,
      "current": 180, "stale": 17, "missing": 9,
      "in_queue": 4, "dead_letter": 2, "skipped_uninitialized": 0,
      "enqueued": 0,
      "sample": {
        "stale":   [{ "root_refno": "24383/70011", "noun": "BRAN", "credential_sesno": 41, "data_sesno": 44 }],
        "missing": [{ "root_refno": "24383/70538", "noun": "EQUI" }]
      },                                 // 每类 ≤ 50 条样本，全量不回（回执要过 WS/HTTP）
      "warnings": []
    }

错误分型沿用统一 `ApiError { code, message, detail }`：refno 不存在 → `not_found`；
`anc` 未回填 → `precondition`；身份不符 → `identity_mismatch`。

进度：**不新造面板协议**。enqueue 后行进 `model_update_pending`，plant-ui 任务队列已轮询
`/api/v1/update/pending-units` 能看到每根；另加只读
`GET /api/v1/model/reconcile/progress?refno=…`（scoped 版 `fn::gen_root_progress`：范围内根的
done / todo / dead 计数），供 UI 画一行汇总。

## 落地形状（一期，纯组装）

1. **`data_interface/model_reconcile.rs`（新）**：`scan_scope(refno)` →
   `{ roots, 每根 data_sesno, 凭证, 在队行 }` → `classify()`。子树收集
   `collect_pe_subtree_refnos`、根解析 `resolve_generation_roots_on` 原样复用；`data(r)` 由
   一次批量 pe 查询 `(id, sesno, deleted)` + 解析期已有的成员→根归属在内存聚合（有
   `root_covers` 就走一跳，没有不建）。分块纪律与 `nouns_of` 相同（1500/批）。direct 模式下
   子树可由 e3d-io 注入，与 `ensure_model_scope_generated_from_refnos` 同口径。
2. **enqueue 复用 `render_upsert`**（`ModelWorkAction::RegenRoot`，`source_end_sesno = wm(d)`、
   时刻 = 水位时刻）——与 seed 同形，凭证收口语义零新增；`force` 附加 attempts 清零语句
   （与 seed 的 `force_all` 同）。**入队即回**，生成交给空闲轮。
3. **web_service**：handler + 两条路由（reconcile / progress），回执如上。
4. **surql**：`fn::gen_root_progress_scoped($roots)`（输入根清单，输出 done/todo/dead 计数）。
5. **plant-ui**：右键容器行「检查并补齐模型…」→ preview 对话（六类计数 + 样本清单）→
   确认 enqueue。仿手动增量更新两拍（ADR-0011 口径：确认后回执即入队，进度去任务队列看）。
6. **定时兜底（可选，默认关）**：配置若干根（或整 MDB），低频（开了也 ≥ 24h）自动
   `reconcile(enqueue, precise)`。变更驱动主路与启动自愈保持原样——兜底捡的是死信、崩溃
   残留、人工删除后的缺口、从未全量跑过的库。

## 二期（判据统一，须先过 ADR-025）

`gen_root` 加 `data_sesno` 列：收口尾事务对本窗口变化根顺手写 `data_sesno = 窗口右端`
（工作项本来就逐个枚举了它们，尾事务行数不变量不破坏）；存量由一次性回填脚本按子树
max(`pe.sesno`) 补。之后 `generation_root_cache_current` 与 `sync_and_seed_model_coverage`
的判据改为 `cred.source_end_sesno ≥ gen_root.data_sesno`，消除「水位一动整库重排」。

**联动**：ADR-025 的模型门（`model_coverage_current`）用的是水位等值语义，必须同一批修订
并补测试；凭证时刻列的口径（plant-ui ADR-0019）不动。

## 必须守住的不变量

- **reconcile 是兜底不是替代**：变更驱动主路与启动自愈原样保留；reconcile 不删产物、不动水位。
- **幂等**：同一范围反复扫描 / 入队无害（record id UPSERT；revision 只在死信复活时 +1）。
- **判据宁枉勿漏**：判不了（sesno 缺失、成员归属存疑）一律按落后处理。
- **OWNER 迁移的盲区要当面说**：元素移出后，旧根的 per-root max(sesno) 看不出变化（元素已
  归新根）。主路的 anc 修复负责这类；reconcile 的 precise 判据对此类旧根可能误判「最新」，
  文档与回执 `warnings` 都要说清，`force` 或 `precise = false` 可兜（Q4 探讨根治）。

## 开放问题

- **Q1：已核验（2026-09-01）**——增量删除是软删墓碑且 `sesno` 前移，precise 判据对删除
  成立，两个备选补救都不需要。证据与两个实现细节见「删除路径核验」节。
- **Q2**：二期把 ensure / seed 切到 `data_sesno` 判据的口径与 ADR-025 修订文本。
- **Q3**：定时兜底的配置形态（`DbOption.toml` 键名、默认关、频率下限）。
- **Q4**：OWNER 迁移时是否在管线 anc 修复处顺手 bump 旧根 `data_sesno`，让 precise 判据完备。

## 附录：core.dll 逆向证据（IDB：`D:\ida_scratch\replica\core.dll.i64`，2026-09-01）

| 结论 | 证据 |
| --- | --- |
| 会话切换 / 查询全套 | `DB_DB::switchToOldSession / switchBackSession / switchToLatestSession`、`allSessions`、`sessionInfo`、`GetSessionDateTime`、`sessionBeforeDate` |
| 变更清单 = 索引表 diff | `DB_RawChanges::createListBetweenSessions` 0x59834b0 → `DB_IndexTableCompare` ctor 0x5a18b20（dab `DCMTBS/db_start_table_sesn_comp`，系统表 id 0xCC47DF）→ `DB_RawChanges::iterate` 0x5983c30（`DCMINX/db_get_next_int_table_diff`；状态 1=modified / 2=inserted / 3=deleted） |
| 单元素点查 | `DB_Element::hasChangedBetweenSessions` 0x593c8b0 → dab 16 号 `DCHELE/db_element_changed_between_sesns` |
| 属性级比较存在 | dab opcode 名表 0x6003900：`DCMATS/db_comp_att_through_sesns`、`DCMELS`、`DCMNNS`（名称表 diff） |
| 下游触发形态 | `DB_DB::createGetworkEvents` 0x58faf00（getwork 时把 RawChanges 装事件广播；字典库整建 UDA/UDET 缓存）；`DB_DBPlugger::Pre/PostDBFileChanges`；调用方清单 = SAVEWORK / GETWORK / FLUSH / QUIT 链路 |
