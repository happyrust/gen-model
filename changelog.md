# 变更记录

## 2026-08-12

### 新增

- **空间树一致性闭环：V2 单文件快照、进程状态机、空间串行锁与降级自愈**（方案
  `docs/plans/2026-08-12-spatial-tree-consistency-closure-plan.md`，D1–D8 已定；
  ADR-010 2026-08-12 增补（二）、ADR-017 2026-08-12 补记）：
  - 快照介质：`accel_tree_{project}.bin` + `.meta.json` 退役，改为单文件
    `accel_tree_{project}.snapshot`（V2：树载荷 + SHA-256 自校验 + project/namespace
    身份 + 双字段指纹，原子 rename 发布）。读侧全套校验任一失败即指针重建，不回落
    旧格式；旧文件仅作一次性迁移候选（双指纹匹配且无 pending → verdict=`migrated`），
    首次 V2 发布成功后**删除**——旧二进制对 bin 缺失是无条件重建，任何回退自动安全。
  - 状态机 `spatial_state.rs`：8 态；房间消费者（启动全量重建、RoomRecalc、空闲
    房间轮）仅 Ready/ReadyEmpty 放行（`SPATIAL_TREE_NOT_READY`，durable 行保留），
    解析/生成/重放/重建/`model.spatial.bounds` 不受门禁。启动判据修正：pending
    优先仅对可读快照成立、进入 ReplayRequired 立即重放（不等派发门）、
    「树非空即 preloaded」收窄为显式夹具标记。
  - 空间串行锁 `SPATIAL_STATE_SERIAL`（`STAGED_COMMIT_SERIAL → SPATIAL_STATE_SERIAL
    → GLOBAL_AABB_TREE`）：staged 提交后收敛、direct 写路径、重建换树/发布、快照
    落盘、Python `spatial.*` 同一串行线（修掉 Python reconcile/persist 与 worker
    并发动树的竞态）；journal 写回与尾事务不持锁。
  - 指针重建：record-range 分页（fork 兼容套件双跑钉住页间无漏无重）；口径
    current-only（排除版本化数组 id 行、`in.deleted` 软删行，Rust 侧排除
    NaN/Inf/反向 AABB 并计数采样）；分页读锁外、stamp 前后比对 + 换树 + 发布锁内，
    三连漂移/查询失败进 DegradedBlocked。房间覆盖率分母同口径。
  - 降级自愈：后台 revalidator（30s 指数退避至 5min）只管 DegradedReuse/
    DegradedBlocked，恢复 Ready 唤醒调度器。
  - 崩溃注入：`AIOS_FAILPOINT=<name>` 五个注入点覆盖方案 §8 崩溃窗口。
  - **对外契约变化**：/health `spatial_tree` 九键作废换十五键（台账 G-02 契约
    迁移，形状钉随迁）；`startup_verdict` 枚举改
    reused/replayed/rebuilt/migrated/degraded/preloaded；Python
    `spatial.persist(force=True)` 在非 Ready/ReadyEmpty 拒绝。
  - 沙箱验收（testbed @8019，六场景：首启重建/快路径复用/截断/删除/rename 前
    崩溃注入/崩后收敛）全过，证据
    `docs/2026-08-12_spatial-tree-consistency-acceptance.md`；E3D 侧场景
    （TTY 复制恢复对拍、伪造旧 epoch、房间边对拍）留 runbook 待跑。

- **db8000 会话快照夹具通用管线阶段一**（方案
  `docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md` §1）：
  切割（`session_cut`）/格式（`aios-session-fixture-v1`）/打包（`archive_util`）
  /管线（`pipeline`）四个模块接上 `db_session_fixture` bin——`pack` 把
  「recording.json + 源 DB 文件」打成只入库最终文件的夹具（台账 sesno 逐切过
  sesno+存在性验证闸、散列入 manifest、6 MiB 预算、收尾即复验），`verify` 对
  夹具目录零外部依赖离线复验（解 zip → 逐台账**现切** → SHA256/大小对账 +
  验证闸，与阶段三回归同一套裁决）。阶段一验收由
  `tests/db_session_fixture_selfcheck.rs` 钉住：用通用模块从 issue-019 zip 的
  final（sesno 26）现切 24/25/26，字节散列与该夹具 manifest 台账逐一相等——
  「任意历史可从最终文件精确还原」在真实 db8000 会话链上成立。同一测试还覆盖
  **pack 往返**（真实源文件 → 夹具 → 复验全绿；台账散列与 issue-019 独立录制
  的那份相等；台账改一位后复验必须变红），因为阶段二的 E3D 录制是一次性的、
  pack 出错要再占一个生产空窗重录。issue-019 专用实现保持冻结不动。该测试已
  接进 CI（`windows-tests.yml` 的 `db8000-model-increment` job，参数与
  issue-019 步骤逐字同款），同批把一直漏在门外的离线解析边界用例
  `--test pdms_record_boundary` 也接了进来。

- **阶段二录制工具**（同方案 §2，待生产空窗执行）：
  `scripts/e3d/Record-Db8000SessionChain.ps1` + 清单驱动的
  `scripts/e3d/db8000_recording_cases.json`（加案例 = 加一对宏 + 一行），投递走
  ADR-019 的 `l3_suite --check-driver`。录制一次性且占生产空窗，所以三道闸都当场
  验：触碰 E3D 前静态审宏（恰好一个 `SAVEWORK`、无 `QUIT`/`FINISH`/`MERGE`/
  `PURGE`、`ALPHA LOG` 成对、`Q REF`+`Q NAME` 齐全）、每条腿后要求 sesno 恰好 +1、
  refno 从宏日志的 `Ref`/`Name` 相邻对回读。配套给 `db_session_fixture` 加
  `inspect` 子命令（只读打印会话链 JSON，与切割同一份解析）。`-CheckOnly` 只读档
  已对真实 `ams8000_0001` 实跑通过（baseline_sesno=210），检查器的拒绝面亦验过。

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

- **`aios_db.full_init` 增加同工程活服务探测**（行为变化：以前能起的场景现在
  可能被拒）：拿锁之后探本机 `http_api_addr` / 8022 / 9099 的 `/api/v1/health`，
  响应是合法 health JSON **且** `project` 与本配置一致就报错退出，
  `full_init(..., force=True)` 显式跳过。动机是单实例锁按「项目根」隔离，两个
  部署包各持各的锁却写同一个工程时锁根本不挡（实测踩过：`test-worklspace`
  的包在 9099、本仓库在 8022）。判据只认 project 名——`/health` 不报「它连的是
  哪个 SurrealDB」，所以隔离沙箱若与生产**重名**会被误伤，用 `force=True` 放行
  （`python/tests/conftest.py` 就是这么做的，并在注释里写明三条资源如何独立）。

- **互踩探测精确化：/health 补报库端点，探测端按「同库」而不是「同名」判**
  （上一条落地当天就被自己的测试沙箱误伤，这是补上的另一半）：
  - `/health` 的 `sul_db` 新增第六键 `endpoint`（配置的 `v_ip:v_port` 原样
    字符串），形状钉与 spec §4.1 同步；`sul_db` 其余五键语义不变。
  - `full_init` 的探测升级为三层判据：`project` 不同 → 无关；服务端报了
    `sul_db.endpoint` → 端点（localhost↔127.0.0.1 归一后）或 `namespace`
    不同都放行——同名工程各写各的库不构成互踩；老版本服务端（≤0.1.18）不报
    端点 → 分不清仍按最坏情况拦，拒绝文案会写明是哪种判法。判定函数是纯函数，
    8 条单元测试钉住（`cargo test -p aios-py --lib`，含对实测 9099@0.1.13
    响应形态的老服务端分支）。
  - `python/tests/conftest.py` 的 `force=True` 暂留：本机 9099 还跑着 0.1.13，
    等同机部署升到带 `endpoint` 的版本即可撤。

- **`aios_db.spatial.tree_status` 的文档与存根不再复述键面**：改为「原样透出
  /health `spatial_tree` 那份渲染，键面以 Rust 侧渲染半边为唯一权威」。此前注释
  与 `.pyi` 各抄了一份九键清单，而 G-02 契约迁移正把它往十五键上带——两处各说
  一套，过期是必然。Python 面只钉判漂移要用的稳定核（`entries` / `file_epoch` /
  `db_epoch` / `drift` / `startup_verdict`），全集的形状钉留在 Rust 侧一处。

- **`aios_db.db.inst` 去掉全表扫，改三段式取边**：① `anc CONTAINS`
  （`idx_inst_relate_anc` 索引查询，anc 含自身故一跳圈住整棵子树）；② 空结果
  回落 `array::flatten(SELECT VALUE ->inst_relate FROM [pe:…])` 图跳，只取元素
  自己那一跳（preload.rs 的实测账：`in` 谓词全表扫 1.57s vs 图跳 3.1ms），兜住
  `anc` 未回填的存量库与直接 `RELATE` 出来的测试夹具；③ 两条都空且库里还有
  `anc = NONE` 行时响亮报错——「查不全」不能被读成「没有」。refno 解析失败也
  改为直接报错（此前 `unwrap_or_default()` 静默成 0，谓词永不命中）。与
  `export_obj` 的差别是多了第 ② 段：那边空结果本就是错误条件，这边空是合法答案。

### 新增

- **`aios_db` 补齐测试支撑面导出，并新开房间增量 pytest 轨**：
  - 新增 `aios_db.fixture`（`create` / `drop` / `move_body` / `refnos`），直通
    `src/fast_model/room_fixture.rs` 的合成房间夹具（1 间 `/ZZ-R-K100` + 2 块
    PANE + 5 个盒形构件，其一骑在重叠区，保留 refno 段 4000000001）——与 Rust
    `room_fixture` live 轨共用同一套数据，两侧断言可互相印证。会写 pe/FRMW/
    inst_*/geo_relate/aabb/vec3 多张表并落 `zzfx_*.mesh`，**只对一次性测试库使用**。
  - 新增 `room.enqueue(changes)`（按 `model.update_aabbs` 的返回形态入队房间
    重算，PANE 走整间分支、其它走元素分支，不受 `room_incremental` 开关门控）、
    `model.delete_subtree(refnos)`（DeleteCleanup 补偿任务同一入口的级联删除）、
    `spatial.tree_status()`（空间树九键指纹，与 /health `spatial_tree` 同源、
    现读现比）、`model.update_aabbs(..., durable=True)`（生产 TransformOnly /
    定向 regen 走的直写事务路径：AABB 指针、`room_recalc` 任务与 spatial epoch
    同事务提交）。
  - 新增 `python/tests/`：对 conftest 自起的一次性内存 SurrealDB
    （`bin/surreal.exe` @8071，进程退出零残留）跑「房间增量收敛 == 全量重建」
    的**逐边**对拍，覆盖构件搬家、面板整间、空刷负例、删除清边留痕、durable
    直写五条；配置 `tests/DbOption-roomtest.toml`（`room_key_word=["ZZ-R-"]`
    只圈夹具房）。conftest 会把仓库根同名空间树文件挪开再还原，不毁真项目产物。
  - 类型存根补齐（新增 `fixture.pyi`，`model` / `room` / `spatial` /
    `__init__` 同步新入口），`py.typed` 对外契约不再漂移。

- **绑定的离线测试档进 CI**（`python/tests -m offline`，60 条）：解析层对着仓内
  `issue-019` 的 db8000 会话快照（与 Rust `db8000_two_delete_fixture` 同一份
  数据、同一串删除序列）、三层硬守护在干净子解释器里逐条验、`.pyi` 与运行时的
  名字集合逐模块对齐、HTTP 客户端对着打桩服务验 12 条 REST 路由与报文形状。
  这一档不连 SurrealDB、不碰 E3D 装机、不扫项目目录，秒级跑完。
  - `.github/workflows/windows-tests.yml` 新增 `python-bindings` job：复用
    `windows-binary.yml` 的 OCCT / protoc provisioning（绑定按 Q7 钉死「与服务
    同一套默认 feature」，必须有 OCCT）→ `maturin build` → 装 wheel → 跑离线档
    → wheel 作 artifact 上传。原 `db8000-model-increment` job 不动。
  - conftest 按选中集合裁定本进程那一份 DbOption（进程级 OnceCell，换库只能换
    进程）：有房间档用例就用 `DbOption-roomtest`，纯离线档用新增的
    `DbOption-ci`。离线用例在任一配置下都成立，两档同跑不冲突。
  - 新增连接层行为用例（`test_connection_layer.py`）：`db.inst` 三段式的每一段、
    `owner_chain` 的自 own 终止、`members`、`spatial.tree_status` 十五键形状钉。

- **版本护栏自锁**：`aios_client.EXPECTED_SERVER_VERSION` 是手抄常量，此前
  `chore(release)` 升 `Cargo.toml` 时没有任何东西提醒同步它——护栏自己先漂移，
  从「提醒你版本不一致」退化成「对着新服务端瞎报警、对着老服务端不报警」。
  离线档新增一条对表用例，bump 忘改常量时 CI 立刻红，红处文案直接写修法。
  `sul_db.endpoint` 恰好是 0.1.19 起才有的键，这条对表马上就有实际意义。

- **`scripts/smoke_m1..m5.py` 标注退役**：五个脚本全部钉在仓库根 `DbOption` +
  8009 正式库 + `D:/AVEVA/...` 真实工程上，而 8009 的数据目录已决定不修，照原样
  跑必失败。不删（它们是 M1–M5 的验收口径记录），改为每个脚本头注写明「历史
  验收记录 / 为何跑不了 / 等价物在哪」，README 脚本表合并成一行同款提示。
  多数段落已被两档 pytest 覆盖；`parse.noun_dict` 依赖 E3D 装机的 `attlib.dat`，
  没有自动化等价物，头注里点名说清。

- **`aios_client` 版本漂移护栏**：`health()` 比对服务端 `version` 与内置
  `EXPECTED_SERVER_VERSION`（现 0.1.18），不一致抛一次 `AiosVersionWarning`
  （同一个 client 不刷屏），`AiosClient(..., expected_version=None)` 关掉。
  回应实测踩过的 0.1.13 绑定对着 0.1.16 部署包查半天的坑——只告警不报错，跨版本
  多数字段仍通用，硬拦会把「凑合能用」变成「完全不能用」。

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
