# DbOption.toml 配置变更清单（基线 2025-06-30 → 2026-08-06）

## 背景

`DbOption.toml` 最后一次入库改动是提交 `7bba5fd4`（2025-06-30，“实现P0级并行优化和完整文档体系”）。
其后 2026-07 下旬开始的增量更新 / 按需生成 / Web 服务等工作只改了工作区文件，一直未提交，
配置语义已与基线明显分叉。本文以 `7bba5fd4:DbOption.toml` 为基线，对照当前工作区版本，
逐项记录**新增键**与**取值变更**，并给出各变更的引入日期（依据同目录带日期的备份文件
`DbOption.toml.bak-7997gen-20260727` / `DbOption.toml.bak-empty127-20260727-143519` /
`DbOption.toml.bak-aabbidx-20260729` 与文件内注释交叉推定）。

文件内的对应位置均已就地加注 `【新增 日期】` / `【变更 日期】` 标签。

## 一、新增配置键（基线中不存在）

| 键 | 当前值 | 引入日期 | 说明 |
|---|---|---|---|
| `http_api_addr` | `"0.0.0.0:8022"` | 2026-07-27（初值 `0.0.0.0:8021`） | Web 服务（REST + WebSocket）监听地址；未配置则不启动，需编译 `http_api` feature。随手动更新 / 按需生成 Web 服务引入（`docs/specs/web-service-api.md`、ADR-011）。2026-07-29 因 8021 端口被占改为 8022。消费方：`src/web_service/mod.rs` |
| `http_api_cors` | `["*"]` | 2026-07-27 | Web 服务 CORS 允许来源列表，`["*"]` 全放行（局域网前端联调）。与 `http_api_addr` 同批引入 |
| `delivery_unit_types` | 未启用（注释） | 2026-07-25 | 最小交付单元类型——增量 / 手动 / 按需生成共用的“生成根”口径；配置后**完全取代**默认集合 `["BRAN","HANG","SUPPO","EQUI"]`，`[]` 表示不使用交付单元。trim + 大写 + 去重，拒绝 WORL/WORLD/SITE/ZONE 与 FTUB。随批量根重生成能力引入（ADR-012、`docs/specs/manual-model-update.md`）。消费方：`data_interface/generation_root.rs` |
| `append_delivery_unit_types` | 未启用（注释） | 2026-07-25 | 在默认交付单元集合之外**追加**类型，仅当未配置 `delivery_unit_types` 时生效。规则同上 |
| `startup_autorun` | `true`（2026-08-10 引入时为 `false`，08-14 翻正） | 2026-08-10 | 启动是否自动干活。`true`（当前默认）时启动重扫发现的未解析库、回退库与追平幽灵水位库会直接交给 worker；`false` 时这些行仍入队但以 `held` 形态等待 watch 事件或人工执行按 dbnum 放行，持久积压与启动全量房间重建也保持原挂起契约。环境变量 `AIOS_STARTUP_AUTORUN` 压过本键（认 `1/true/yes/on` 与 `0/false/no/off`，认不出的值退回本键）。属本仓扩展键（`options.rs::DbOptionExtFields`），非 rs-core `DbOption` 字段，缺键采用 `true`。消费方：`options.rs::startup_autorun`、`lib.rs::skip_startup_room_build`、`batch_queue.rs::DataBatch::held`、`batch_scheduler.rs::arm_auto_work` |
| `room_incremental` | `true`（2026-08-10 引入时为 `false`，08-12 翻正） | 2026-08-10 | 房间归属的**增量**重算开不开。`false` 时两个写入点都不再排 `room_recalc` 目标（位姿/删除刷新包围盒后的直写事务、暂存窗口收口的 `merge_room_recalc_changes`），空闲轮也不再收房间轮；已排在 `model_update_pending` 里的目标原样留着，开关一开照常收。只管增量这一条链——启动全量重建、人工重建、`drain_rooms` 直调（房间对拍夹具走的就是它）都不看本键。环境变量 `AIOS_ROOM_INCREMENTAL` 压过本键（取值规则同 `AIOS_STARTUP_AUTORUN`）。属本仓扩展键（`options.rs::DbOptionExtFields`），缺键不会起不来。消费方：`options.rs::room_incremental`、`batch_worker.rs::room_round`、`model_update_pending.rs::merge_room_recalc_changes`、`occ_generate.rs` 直写事务 |
| `catalogue_project_priority` | 与 `included_projects` 同顺序 | 2026-08-14 | ADR-025 的跨项目 DICT/CATA 裸 dbnum 选主顺序，大小写不敏感。它是**覆盖层不是全部顺序**：点到名的排最前，没点到的按 `included_projects` 书写顺序接着排，因此整键不写或只写一半都不阻断。重复项目、`included_projects` 之外的未知项目仍阻断对应阶段（那是打错字，不是没意见）。被遮蔽候选只进回执/健康状态与 `[manifest] … 被项目优先级遮蔽` 日志，不写 observation、水位或队列。**2026-08-20 修正**：此前 `initialization_phase.rs` 只认显式名单、把没点到名的项目当「排不出主」，于是漏写一个项目 = 整个 Catalogue 相位阻断 = 其后所有 Design 批次卡在屏障后（现场：`跨项目 CATA/DICT dbnum=7000 冲突且没有 catalogue_project_priority 选主`）；`versioned_db/database.rs` 的全量同步侧一直是按本行口径排的，两条路径现已一致。消费方：`options.rs`、`initialization_phase.rs`、`increment_manager.rs`、`manual_update.rs`、`versioned_db/database.rs` |
| `room_key_word` | `["-RM"]` | 2026-07-29 | 房间名关键词**列表**，用于房间-面板关系匹配（`fast_model/room_model.rs::get_room_key_word`）。**替代旧键 `room_keyword`（单字符串，已废弃删除）**：一次支持多关键词，且 AMS 房间名（如 `/1RX-RM03-R301`）末段由 `project_hd` 规则校验 |
| `watermark_realign` | **已移除**（2026-08-13，ADR-021） | 2026-08-12 | ~~水位不对齐（F6 回退）的检查与处置档位~~。**2026-08-13 随 ADR-021 整键退役**：回退的默认且唯一处置改为「扫描只入队重建批次，worker 冻结点复核仍判回退才整库清空（`wipe_dbnum_for_reinit`）并按首次导入重新解析」；档位、`AIOS_WATERMARK_REALIGN` 环境变量与单库端点 `POST /dbnums/{dbnum}/realign` 一并移除，「先别动我看看」由 `startup_autorun` / 队列暂停承担。以下为历史记录：水位不对齐（F6 回退：文件被还原/替换，`file_latest_sesno < applied_sesno`）的检查与处置档位。`off`=现状只阻断；`check`=扫描时逐库输出 `[水位审计]` 行（不动数据）；`rebaseline`=检测到回退自动对齐——`prune_above_watermark` 清掉高于文件水位的行与队列残留、`applied_sesno` 写 0（写值不删行）、由基线路径按首次导入全量重建，**只对回退生效**，其余异常照旧阻断（ADR-001 2026-08-12 修订的唯一 opt-in 例外）。环境变量 `AIOS_WATERMARK_REALIGN` 压过本键（认 `off/check/rebaseline`，认不出退回本键）。属本仓扩展键（`options.rs::DbOptionExtFields`），缺键不会起不来。消费方：`options.rs::watermark_realign`、`increment_manager.rs::scan_and_check_file`、`manual_update.rs::realign_rolled_back_dbnum`；另有单库端点 `POST /api/v1/dbnums/{dbnum}/realign`（spec §4.9）**不看本键** |

## 二、取值变更（键在基线中已存在）

| 键 | 基线值 | 当前值 | 变更日期 | 原因 |
|---|---|---|---|---|
| `sync_live` | `true` | `false` | 2026-07-27（07-29 曾临时回 `true`，08-04 复关） | 生产增量改为“启动重扫后持续监听”；Web 冒烟验证期间关闭启动期全量同步 |
| `replace_dbs` | `false` | `true` | 2026-07-27 | 增量重解析允许覆盖既有库文件记录 |
| `project_path` | `D:/AVEVA/Projects/E3D2.1` | `D:/AVEVA/Projects/E3D3.1` | 2026-07-27 | AMS 样例工程迁移到 E3D3.1 项目目录 |
| `included_projects` | 含 `"SCB"` | 移除 `"SCB"` | 2026-07-27 | E3D3.1 下未部署 SCB，避免扫描报错 |
| `included_projects` / `project_dirs` | `project_dirs` 可按下标覆盖项目物理位置，空名单时还会成为扫描名单 | `included_projects` 是 `project_path` 下的文件夹名且是唯一项目扫描名单；`project_dirs` 不再参与扫描 | 2026-08-24 | ADR-046：当期扫描不得被另一份路径表扩大或重定向；空名单就是不扫描项目 |
| `gen_model` | `true` | `false` | 2026-07-27 | 非 CATA 增量监听模式：启动不做全量重算，增量/按需路径内部自行强制 `true` |
| `gen_mesh` | `true` | `false` | 2026-07-27 | 同上 |
| `gen_spatial_tree` | `false` | `true` | 2026-07-29 | 配合房间增量与空间索引（aabbidx）验证启用 |
| `gen_model_batch_size` | `16` | `4` | 2026-07-29 | ams7997 全量生成 16 并发在 `save_instance_data` 撞 RocksDB 事务写冲突（empty127 轮） |
| `debug_refno_types` | `["CATA","LOOP","PRIM"]` | `["LOOP","PRIM"]` | 2026-07-27 | CATA 改走闭包按需解析（`cata_closure`），不再全量调试解析 |
| `manual_db_nums` | 注释（未启用） | `[7997, 7998, 7999, 8000]` | 2026-07-27 起启用 | 各轮实测窗口：07-27 `[7997,7999,8000]` → 07-29 `[8000]` → 08-04 `[7998,8000]`（7998 为 18 会话最小设计库，8000 验 up_to_date 分支） → 08-06 `[7997,7998,7999,8000]`（issue #10 的 E3D 实测窗口，重新纳入 7997/7999；跑完可收窄回 `[7998,8000]`） |
| `room_keyword` | `"-R-"` | **键已删除** | 2026-07-29 | 被 `room_key_word`（列表）取代，见上表 |
| `debug_root_refnos` | `[]` | 注释掉 | — | 清理：缺省即为空，无语义变化 |
| `incr_sync` | `false`（多一个空格） | `false` | — | 仅格式整理，无语义变化 |

## 三、时间线速览

- **≤ 2026-07-27**：切换 E3D3.1 / 移除 SCB；关闭启动期全量生成（`gen_model`/`gen_mesh`→false）与 `sync_live`；`replace_dbs`→true；`debug_refno_types` 去 CATA；启用 `manual_db_nums`；新增 Web 服务键 `http_api_addr`(8021)/`http_api_cors`；（07-25 前后）交付单元键随 ADR-012 引入。
- **2026-07-27 → 07-29**：`http_api_addr` 8021→8022；`gen_spatial_tree`→true；`gen_model_batch_size` 16→4；`room_keyword`→`room_key_word=["-RM"]`。
- **2026-08-04**：`sync_live` 复关（Web 冒烟验证）；`manual_db_nums` 调至 `[7998, 8000]` 增量实测窗口。
- **2026-08-06**：`manual_db_nums` 放宽至 `[7997, 7998, 7999, 8000]`，重新纳入 issue #10 取证涉及的 7997（基线库 `.surreal/ams-7997-e3d-test-20260805`，applied=92）与 7999（此前被排除，applied=3、file=41）。
- **2026-08-06（增量范围只认 MDB）**：`manual_db_nums` / `exclude_db_nums` / `only_sync_sys` **不再参与增量判定**——增量范围只由 `mdb_name` 那个 MDB 声明的 DESI 决定（`data_interface/update_scope.rs`），代码上 `should_process_database` 并入 `increment_manager::in_scope_with`。起因正是上一条：手写名单挡掉 7999 时，日志与「MDB 里没有这个库」一模一样，现场只看得到每 30 秒重复一次的「不在本期执行范围，跳过数据库: 类型=DESI, 编号=7999」。这三个键仍对全量模型生成与按需基线解析生效，因此上表里 `manual_db_nums` 的取值仍有意义，只是**不再限制增量跑哪些库**——本地站点从此按 `/ALL` 声明的 29 个 DESI 全量增量。
- **2026-08-06（`gen_spatial_tree` 开关治理）**：结论钉死——开关保留，但角色从「功能形态开关」转为**运维开关**（止血降级 / 演练隔离），所有配置的默认姿态为 `true`。备用配置 `DbOption-ams.toml` / `DbOption-zsy.toml` / `DbOption_text.toml` 残留的 `false` 一并翻正，消除「整份拷回把 false 带回来」的 issue #7 复发路径。`load_spatial_tree` / `save_spatial_tree_to_db` 确认为**死键**（rs-core 仅定义、本仓与 rs-core 均零读取），但它们是必填 bool 字段，从 toml 删键会让 config 反序列化报 missing field 起不来——因此只就地标注、不删，待 rs-core 删除字段后随之移除。同批记录默认值改进方向：rs-core `DbOption` 的 bool 缺省为 false，「缺键=关」与「默认需要空间/房间计算」相悖，下次动 rs-core 时应将 `gen_spatial_tree` 缺省翻为 true（或连带清理死字段）。
- **2026-08-07（`gen_spatial_tree` 使用移除）**：推翻 08-06「保留为运维开关」的结论——空间/房间计算恒开启，代码不再读这个键。拆掉的门：启动树加载与启动全量房间重建（`lib.rs` 两处 + `run_app` 一处）、AABB 刷新的 `maintain_spatial_tree` 快速路径（`update_inst_relate_aabbs_by_refnos` 收并为单函数）、房间入队口（`enqueue_room_recalc` 不再收 `DbOption`）、暂存窗口房间语义/merge 门与房间轮早退（`batch_worker`）、暂存房间报告早退（`model_update_pending`）、`/health` 的 `gen_spatial_tree` 字段。相应的四条「回退即红」源码钉子一并退役。键本身降级为**死键**（与上一条的两个键同款处理）：必填 bool、删键起不来，待 rs-core 删字段后随之移除；运维止血从此只能靠 `AIOS_SKIP_STARTUP_ROOM_BUILD`（只跳启动全量重建，不冻结增量）。

- **2026-08-10（启动默认不自动干活 + 房间重建改为有状态对账）**：新增扩展键 `startup_autorun`，**默认 `false`**——启动只做「让库能用」的那些幂等自愈，不执行任何增量、不做全量房间重建。

  语义刻意**不是**全局暂停：那会把后来真正的实时增量也一起挡死，而这个默认要挡的只是「停机期间攒下、此刻谁都没要求处理」的积压。实现落在队列行上——`DataBatch` 新增 `held`，重扫（`init` / `scope-refresh` / `share-remount` 三个来源）排出来的行挂起，`freeze_next_concurrent` 跳过它们且**不算队首**（与独占行刻意相反：独占要保住 FIFO 位置，挂起是「这个库压根不参与本轮」，否则一条挂起行能把它后面所有真实增量堵死）。一次真实触发（watch 文件事件 / `POST /update/execute`）不挂起，并把同 dbnum 那条挂起行放行——合并本来就是既有语义，于是启动排的 `103..=132` 与新存的 `133` 合成 `103..=133` 一次跑完，积压不会被跳过。放行写在 `Merged` / `AlreadyCovered` 两条分支**之前**：一个新会话都没带来的迟到事件同样证明有人在动这个库。放行是单向的，后续重扫不会把已放行的行重新挂起（否则一次范围刷新就能把人工刚点下去的执行按回去）。

  worker 空闲轮那侧的持久积压（房间重算目标、模型单元）不按 dbnum 分，没法逐行挂起，改用一个进程级「上弦」旗标：`startup_autorun=false` 时启动为 false，第一次真实触发扳成 true 且只进不退、不落库（它描述的是本进程这一趟，不是需要跨重启保留的操作意图——那是 `queue_control:main` 的暂停）。空闲轮的门因此是 `!paused && armed`。

  同批把启动全量房间重建从**无条件**改成**有状态对账**：库侧 `room_build:main` 记下上次成功重建时的 `spatial_epoch` 与 `tree_entries`，与当前一致就跳过。两个字段各补一个盲区（epoch 认得出走意图队列的变更，条数认得出直写/全量生成那两条不递增 epoch 的路径），都对不上或从没建过才重建；被覆盖率闸门拦下、或逐面板有失败的那一轮**不盖章**，免得把一次失败永久固化成成功。三道门的优先级是 `AIOS_SKIP_STARTUP_ROOM_BUILD` → `startup_autorun` → 库侧对账，止血口排最前是因为「跑增量」与「跑 2 万面板级全量重建」是两件事（L3 夹具正是要前者不要后者，其两个 spawn 点已显式加上 `AIOS_STARTUP_AUTORUN=1`）。

  `/health` 新增 `startup_autorun` 与 `auto_work_armed`：「服务活着、队列有货、就是不动」有三种完全不同的成因（运维按了暂停 / 冷启动还没被真实增量上弦 / worker 死了），少了中间这个字段在接口上分不出前两种。`/queue` 的行状态相应多了 `held`——显示成 `queued` 的话，一条永远不动的行与「消费者卡住了」长得一模一样。

  起因是现场每次启动都为房间重建付 14~15 秒，且那 15 秒因空间树只有 22056 条（库里 105536 条，低于 90% 下限）被闸门整轮拒绝，一条边都没写；同时库里还压着 2580 个房间重算目标，按旧默认每次重启 worker 都会直接开始啃。

- **2026-08-10（房间增量默认关闭）**：新增扩展键 `room_incremental`，**默认 `false`**。

  上一条把 2580 个房间目标从「启动就啃」改成了「等上弦」，但上弦之后它照样要啃。现场实测那 2580 个目标全是构件、无一块面板，且**每一个都查不到几何实例**，于是每个都走「按空集收敛」——`room_model.rs` 在那行日志上方自己写着「这条路本不该走到」，因为元素任务的入队条件就是「包围盒确实变了」。一轮 256 个目标付两次全量查询、耗时约 88 秒，四轮下来刷了 768 行同样的日志，把同期那条真正失败的增量整个埋掉了。

  根因不在房间侧：数据批次因祖先链断裂反复失败 → 暂存窗口提交不了 → `batch_regen_is_allowed` 为假 → 交付单元一个都不生成 → 几何永远不出现 → 房间轮每页继续写空集。两件事互为因果，而房间那半边的噪音让模型侧的问题看不见。先把它关掉，让模型增量的正确性能被单独看清楚。

  三个门：两个写入点（直写事务只摘 `room_recalc` 那一条语句，指针写与 epoch bump 照旧——空间树确实动了，少 bump 一次会让重启后「文件之后还有空间提交」的判定看错）、一个消费点（`room_round` 早退，用 `Once` 只播报一次，空闲轮 30 秒一趟，每趟复述同一个配置项就是把日志刷成噪音）。`/health` 相应新增 `room_incremental`：关着时房间泳道永远是空的，而「没活」与「开关关着」在外面长得一模一样。

- **2026-08-12（房间增量默认打开）**：`room_incremental` 缺省值翻为 **`true`**，`DbOption.toml` 同步写成 `true`。三道门一处没改，翻的只是缺省值。

  上一条的止血目标已经达成：那 2580 个查不到几何的目标已经收干净（现场 `/update/pending-units` 的 `room_units` 为空），模型增量侧的正确性也已经能被单独看清。继续维持关闭的代价此刻更贵——关着时房间归属**只有删除路径**还在维护（`helper.rs::delete_room_membership` 从不看这个开关，元素删了边照样两个方向清掉），而「搬家之后重算」整条链是冻的；按设计这部分应由下一次启动的全量重建回补，但那条兜底路径排在 `startup_autorun` 之后（`skip_startup_room_build` 的门序是 `AIOS_SKIP_STARTUP_ROOM_BUILD` → `startup_autorun` → 库侧对账），而它自己默认也是 `false`：默认部署两个开关都关着，`room_build:main` 的对账根本到不了，等于既不增量也不回补，材料表的房间号会一直停在旧值。

  显式写了 `room_incremental = false` 的配置（`python/tests/DbOption-ci.toml`、`python/testbed/DbOption-*.toml`）行为不变——那几档是刻意只测别的链路。要临时关一次用 `AIOS_ROOM_INCREMENTAL=0`，不必改文件。打开之后要盯的仍是 08-10 那个形态：空闲轮日志里若再出现整页「按空集收敛」的房间目标，说明几何又没跟上，那是模型侧的信号，不是房间侧的。

- **2026-08-13（watermark_realign 移除，ADR-021）**：该键与 `AIOS_WATERMARK_REALIGN`
  环境变量、单库端点 `POST /dbnums/{dbnum}/realign` 一并退役。回退默认整库重建：
  扫描路径（sweep / watch / 手动入队）检测到回退只入队一条重建批次（applied=0
  形状，窗口 1..file_latest），worker 冻结点复核仍判回退才 `wipe_dbnum_for_reinit`
  （整库清空 + 统计与队列残留清空 + 水位行清值不删行 + spatial epoch 递增），随后
  落进现成基线路径按首次导入重新解析。缝合式对齐（只删高于文件水位的残留 +
  INSERT IGNORE 补洞）依赖「幸存行与新文件同史」假设，被整库重建取代。下一条为
  引入时的历史记录。

- **2026-08-14（启动默认修复未解析库与追平幽灵水位，ADR-023）**：`startup_autorun`
  缺省值与根配置翻为 **`true`**。启动重扫在 `file_latest_sesno == applied_sesno > 0`
  时先查数据支撑；`pe` 零行且没有匹配的空基线凭据，就按首次导入窗口入队并由
  worker 建立基线。显式配置或环境变量 `false` 仍保持 held，供测试和运维窗口使用。

- **2026-08-12（watermark_realign 引入）**：新增扩展键 `watermark_realign`（默认 `"off"`，配置文件中为注释示例）。

  起因是 test-workspace 现场的 F6 阻断：`ams7999_0001` 被还原后 `file_latest=114 < applied=120`，该库从此增量停摆，唯一出路是手工 SQL（改水位行、删 attempt/pending）加人工重建。这个档位把那套手工配方自动化：`check` 只审计（逐库一行 `[水位审计]`，启动重扫正好构成全量对齐清单）；`rebaseline` 对**回退**自动对齐——复用 `prune_above_watermark`（自持暂存收口与水位两把写闸）物理清掉高于文件水位的残留，水位写 0 落进现成的 `needs_initial_load` → `initialize_dbnum_baseline` 基线分支，同一条批次队列内完成全量重建。只对回退生效；类型变更/同号多文件/归属不符/文件缺失照旧阻断（自动处理等于替人拍板挑文件）。文件若被换成完全不同的历史，基线完整性校验会拒绝收口并持续报错——响亮失败，不静默缝合两段历史。这是 ADR-001「水位只进不退」的唯一 opt-in 例外（见该 ADR 2026-08-12 修订注记）；同批新增单库端点 `POST /api/v1/dbnums/{dbnum}/realign`（spec §4.9，不看本键，生产 `off` 也能修单个库）。

## 四、注意事项

- 本文件描述的是 **AMS 本地站点工作区**的配置状态，多数取值属于实测窗口调参（如
  `manual_db_nums`、`sync_live`），部署到其它站点时应按站点实际情况取值，不应照抄。
- `http_api_addr` / `delivery_unit_types` / `room_key_word` 三组新键是**功能性配置**，
  部署新站点时需要显式决策；其余多为运行窗口开关。
- `gen_spatial_tree` 自 2026-08-07 起为**死键**：代码零读取，空间/房间计算恒开启。
  键仍必须留在 toml（必填 bool，删键起不来），取值不再有任何效果；快速重启可用
  `AIOS_SKIP_STARTUP_ROOM_BUILD` 环境变量跳过启动全量房间重建（增量照常）。
- 自 2026-08-14 起，**服务默认启动即消费重扫发现的工作**（`startup_autorun = true`），
  包括未解析库、回退重建和追平幽灵水位。显式写成 `false` 后，若出现「服务活着、
  队列里有货、就是不动」，看 `/health` 的 `startup_autorun` / `auto_work_armed` 与
  `/queue` 的 `held`：在 E3D 里存一次盘或调用 `POST /api/v1/update/execute` 会按 dbnum
  放行该行；也可用 `AIOS_STARTUP_AUTORUN=1` 临时恢复启动自动执行。
- `POST /api/v1/queue/resume` 解的是**持久化暂停**，不是冷启动挂起：挂起行由各自 dbnum
  的真实增量放行，resume 放不动它们。两者是正交的两道门，排查时别混。
- 三个启动开关各管一段，别混用：`startup_autorun` 管「这次启动要不要自动干活」，
  `AIOS_SKIP_STARTUP_ROOM_BUILD` 管「要不要做启动全量房间重建」（前者开着时仍可单独
  关掉后者），`AIOS_FORCE_SPATIAL_REBUILD` 管「要不要丢掉落盘的空间树、从库指针重建」
  ——空间树残缺（房间重建被 90% 覆盖率闸门拒绝）时要用的是最后这一个。
  **（2026-08-11 起该开关只认明确真值 1/true/yes/on，写 `=0` 是关闭而不再是触发；
  且常规启动已带指纹校验、意图重放自愈与缺失/损坏自动重建，多数残缺场景不再需要
  手动设置它，见 `docs/2026-08-11_spatial-tree-startup-init-plan.md`。）**
