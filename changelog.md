# 变更记录

## 2026-08-12

### 修复

- **直写路径的空间树变更补上 epoch 痕迹，消除崩溃后的静默漂移**（方案
  `docs/plans/2026-08-12-spatial-tree-direct-mutation-epoch-trace-plan.md`，
  ADR-010 2026-08-12 增补）：
  - 钉死不变量——**凡是改变了「树应有内容」的已提交变更，都在同一事务内 bump
    `spatial_epoch:current`**。此前只有 durable 增量与暂存窗口尾事务 bump，
    全量生成 / `manual_update_aabbs` 的普通直写刷新与删除清理两条路既不写
    `spatial_reconcile` 意图行、也不 bump：树同步完、空闲轮落盘前崩溃，重启时
    sidecar 与库指纹相等，启动判据按 Reuse 复用一棵陈旧的树，而 /health 的
    `drift` 恒为 false，无人可见。删除路径的后果尤其重——启动全量房间重建会把
    被删构件按旧包围盒重新收编进 `room_relate`（ADR-010 D4 借崩溃复活，而
    `DeleteCleanup` 任务早已 done，没有重放会再清一次）。
  - `update_inst_relate_aabbs_by_refnos_mode` 的直写事务门控从
    `durable_room_trigger && !chunk_changes.is_empty()` 放宽为
    `!chunk_changes.is_empty()`；`durable_room_trigger` 从此只决定「要不要随事务
    发布 `room_recalc` 任务」，不再决定「要不要事务与 bump」。重算值与树上旧值
    逐位相等的重刷仍走普通写、不 bump——没动树的提交不该作废别人的树文件。
  - `delete_room_membership` 的窗口外分支改为按块「取写锁 → 锁下探测这些 refno
    在不在树上 → 在则把房间边删除与 bump 包成一个事务、不在则照旧普通写 →
    摘树 → 标脏」。探测在锁下做，「要不要 bump」与「树到底动没动」由同一个快照
    裁决。暂存分支不变（意图行 + bump 仍由窗口尾事务收口）。
  - 普通直写分支补上写锁，跨度 [变更判定 → 事务 → 树同步]（durable 增量的全跨度
    锁不变）。顺带关掉一个此前没盘到的交错窗口：并发的删除清理挤在事务与同步
    之间时，刚摘掉的条目会被这里同步回树上，成为要等下次指针重建才自愈的幽灵。
  - 行为变化：全量生成、`manual_update_aabbs`、删除清理的直写提交现在会推进
    spatial epoch（按块，一次全量生成约产生「条目数/100」次 bump）。多次 bump
    语义无害（判据只比相等），代价是这些路径跑过之后，下次启动的全量房间重建
    对账凭据（`room_build:main`）会判为「空间状态已变」而照跑一次。

### 变更

- **`aios_db.model.export_obj` 改为整树单文件导出，子树收集走 anc 索引**
  （2026-08-12 增量审查修复计划 P3）：
  - 对外契约变化：此前每个实例根一个 `{refno}.obj`，现在整棵子树合成一个
    `{refno}.obj`、内部按「实例_geo_hash」分 `o` 组，`files` 恒为单元素数组；
    交付单元根（EQUI/BRAN…）自身没有直接实例行也能导出整树。
  - 子树实例收集只走 `anc CONTAINS`（`idx_inst_relate_anc` 索引查询，anc 含
    自身故根自己的实例行同谓词圈住），不再 OR 无索引的 `in = …` 臂——那会把
    整条谓词退化回全表扫（preload.rs 实测账：1.57s vs 3.1ms）。
  - 响亮失败取代静默空集：refno 解析失败直接报错（此前静默成 0、谓词永不命中）；
    空结果时按 rs-core `inst_relate_anc_ready` 同口径探一次 `anc = NONE`，
    存量未回填的库给自愈指引（启动一次 gen-model 回填）而不是谎报「没有实例」。
  - testbed 全链路（`python/testbed/run_full_loop.py`）导出步骤补形状断言：
    单文件、`o` 组数 == 导出实例数、triangles > 0、无缺失 mesh。

## 2026-08-11

### 变更

- **层级查询优化 P3（gen-model 份额）：退役 `inst_relate`/`tubi_relate` 的
  `zone_refno` 列**（方案 `docs/plans/2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md`
  P3，as-built 记录 `docs/2026-08-07_journal-fn-dependency-audit.md` §4）：
  - 写侧全退：建行字面量（普通行 + TUBI 行）不再写 `zone_refno`，
    `ResolvedInstMeta` 的 zone 槽位与 `resolve_inst_meta` 的 noun 预取移除；
    回填 `backfill_inst_relate_anc` 与 OWNER 搬迁重算
    `render_anc_repair_statements` 不再连带 `zone_refno = fn::find_ancestor_type(...)`
    ——每行一次的 9 跳 owner 上溯从回填成本中整个消失，`fn::find_ancestor_type`
    自此离开 inst 写入链（函数定义保留给材料表等读侧）。
  - 收口探针收窄：`desi_finalize_preflight` / `selfcheck_surreal_functions`
    只探剩余的收口硬依赖 `fn::anc_u64`。
  - 索引迁移：`INST_RELATE_INDEX_SQL` 前两行 `REMOVE INDEX IF EXISTS` 在启动/建窗时
    摘除旧 zone_refno 索引的两个历史名字（`idx_inst_relate_zone_refno` 本仓 F1
    修复后建的、`inst_relate_zone_refno_index` plant-ui rs-core `define_pe_index`
    建的，AMS 实库两者并存实测在案）。存量行旧值保留不删，只是不再写入、不再被
    索引；「索引不存在 / 表都不存在 / 重复摘除」三种 no-op 情况由双跑用例
    `dual_inst_relate_anc_u64_contains_index_agrees` 连 INFO 终态一起钉住。

### 新增

- **`fn::zone_u64` / `fn::site_u64`（common.surql，P3 读侧便捷层）**：从 `anc`
  链尾 O(1) 定位 ZONE/SITE，与元素深度无关——链尾打包值 ref1==0 即 WORL 的
  自适应偏移，判据与 Rust 解析器同源；「含自身」语义与退役的
  `fn::find_ancestor_type` 口径一致，短链/空链返回 NONE。反向圈行（某 ZONE 下
  全部实例）仍走 `anc CONTAINS` 索引查询，不用它。两种链尾真实形态（悬空 WORL
  收尾 / 0_0 哨兵被滤止于 SITE）由 `zone_and_site_helpers_locate_from_the_anc_tail`
  与双跑用例 `dual_anc_u64_functions_execute_and_agree` 双引擎钉住。

## 0.1.18 - 2026-08-11

### 变更

- **空间树启动初始化改为分层判据**（方案与决策记录
  `docs/2026-08-11_spatial-tree-startup-init-plan.md`，ADR-010 2026-08-11 增补）：
  - sidecar 指纹从单一 epoch 数值扩成 **(epoch 值, 库侧 bump 时刻 `updated_at`)**
    双字段，两个字段都与库相等才直接复用树文件（评审要求：与数据库对时间戳；
    库快照回滚恰好撞回同一计数也认得出来）。旧版 sidecar 缺新字段按失配走，
    一次自愈后补齐。
  - 指纹失配但库里还有待重放空间意图 → 复用文件、交给 worker 出队前的意图重放
    自愈（不再像旧 epoch 校验那样每次崩溃重启都全量重建）；失配且无意图 →
    只读指针重建（直写崩溃 / 换文件 / 回滚库）。
  - 树文件缺失/损坏从「空树等人工」改为**自动指针重建**（决策 D1）；库侧诊断
    查询失败降级复用文件 + 告警（D2）；两处启动调用点统一为「告警降级空树、
    不阻断启动」（D3）。
  - /health 新增 `spatial_tree`：文件/库两侧指纹现读现比、`drift`、条目数与
    本次启动裁决（reused / healed_by_replay / rebuilt / empty / preloaded /
    reused_degraded）。

### 修复

- AMS/8000 房间增量灰度闭环：
  - `inst_geo` 的确定性落库从忽略重复改为 `UPSERT ... MERGE`，保留已有 mesh/AABB，
    同时补齐缺失参数并在显式重生成时清除 `bad`；重复执行同一生成批次可收敛。
  - 无圆角、共线回折的房间面板不再走一次性删点失败分支，统一使用逐交点修复器；
    加入 AMS 真实 PLOOP 参数回归。
  - `startup_autorun=false` 时，显式 `POST /update/execute` 即使所选 dbnum 已追平，
    也会为本进程上弦并放行 durable 模型/房间积压，避免人工 canary 永久停在
    `up_to_date`。
  - `Run-RoomE3DE2E.ps1` 新增复用现有 9099 服务的 `db8000-equi-copy` 案例；
    TTY 宏对 `=24384/24776` 执行 probe/apply/restore，并核对水位、新 EQUI、
    `inst_relate`、pending/dead-letter 与空间补偿。

- `AIOS_FORCE_SPATIAL_REBUILD` 只认明确真值（1/true/yes/on）：旧实现判
  `is_ok()`，部署模板写 `=0` 想关闭，实际每次启动都强制全量指针重建。三态解析
  收口在 `batch_worker::parse_explicit_flag`，与 `GEN_MODEL_DIRECT_INCREMENT`
  的 P2-1 纪律同款。

## 2026-08-06

### 新增

- **执行范围缓存 + 周期对账重扫**（现场：数据批次执行中 SUL_DB 连接抖动，watcher 的
  范围解析报 `receiving from an empty and closed channel`，整批文件事件被丢弃且无重试）：
  - 文件事件路径的 MDB 范围解析改走进程内单槽缓存（`UpdateScope::resolve_cached`）。
    名单只在 SYS meta 批次落库时才变，那一刻与 `SCOPE_DIRTY` 同点显式失效；TTL 兜底
    `AIOS_SCOPE_CACHE_SECS`（默认 300s，0 关闭）。SUL_DB 瞬时不可用时暖缓存放行并告警，
    冷缓存与配置错误（mdb_name 没填 / MDB 名不存在）维持 fail-closed 上抛。
    启动重扫、重挂补扫、周期对账与手动路径仍每次真查（fresh），它们就是缓存的刷新点。
  - watcher 事件循环新增周期对账重扫（`AIOS_WATCH_RECONCILE_SECS`，默认 300s，0 关闭）：
    按间隔整面重比「文件最新会话号 vs applied 水位」，把连接抖动、服务重启等一切来源
    丢掉的文件事件在一个周期内追回；入队按水位判定天然幂等，与启动重扫共用
    `sweep_watch_dirs`。

- **issue #10 复现套件**（`src/data_interface/staging/issue10_add_node.rs`，仅测试编译）：
  用真实渲染与真实暂存窗口（`stage_parsed_window` → `register_staged_finalize` →
  `commit_registered_to`）在 mem 引擎上模拟 E3D「复制 BRAN 并 SAVEWORK」的连续增量，
  钉住三条路径——连续多个窗口写回后新增节点必须出现在模型树（含父成员序边重建）；
  窗口因生成重试耗尽被阻断时「检测得到、树不动、水位原地」，吸收重置后重算收敛；
  journal 被持久层确定性拒绝（坏版 `update_dbnum_event` 对字符串 id 的 pe 行报
  `array::at` 类型错）时写回整体回滚、零半写，排毒后同一份 journal 重放收敛。
- **批次执行的阶段日志**（issue #12）：完成行补上 sesno 窗口与墙钟完成时间，在 E3D 里
  SAVEWORK 的人可以直接对上「屏幕上这批日志是不是我刚才那次保存触发的」；模型计划按
  action 分组计数（形如 `regen_root=3 transform=12`）而不再只报总数；交付单元、批量
  重生成、房间归属重算各自报耗时与成败；生成根列表超过 8 个截断并报出总量。

### 变更

- `DbOption.toml`：`manual_db_nums` 由 `[7998, 8000]` 放宽到 `[7997, 7998, 7999, 8000]`，
  纳入 issue #10 的 E3D 实测窗口（基线库 `.surreal/ams-7997-e3d-test-20260805`，7997
  applied=92）。取证结论就地记在配置注释里：issue 截图中的 `/1WCC0211` 属于 7999，而
  7999 一直被排除在手动窗口外（applied=3、file=41），库里的树是旧全量同步的残留。
  实测跑完可收窄回 `[7998, 8000]`。
- 房间轮次日志不再只在「队列跑空」那一轮打印，距上轮超过保底间隔触发的那轮同样报出
  目标数与死信数。

### 内部

- `attempts::record_window_block_at_on` 提升为 `pub(crate)`，`StagedFinalize` 增加
  `Debug`，供复现套件构造阻断现场。
- `src/bin/manual_scan_probe.rs`、`src/test/mod.rs` 仅 `cargo fmt` 排版整理。

---

## 历史记录

1、添加自动增量更新文件的修改，启动时会检查当前数据库和E3d数据库的一致性
