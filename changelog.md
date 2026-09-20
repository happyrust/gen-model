# 变更记录

## 2026-09-20

### `/model/ensure` 撞上「正在后台生成」回的是 500 不是 409：领域错穿过共享航班后只挂在 `source()` 上，`ensure_error` 改沿链找

- **为什么**：`ensure_exact_generation_root` 走 `generation_flight::Flights` 共享一个生产者，失败被包成 `OutcomeError`、
  原错只在它的 `source()` 上；HTTP 层 `ensure_error` 只看顶层 `downcast_ref`，`ModelGenerationInProgress` /
  `ModelStaleAfterGeneration` / `UnresolvableRoot` / `InitializationNotReady` 一个都认不出，全落 `from_domain` → 500 `internal`，
  文案还被 `{error:#}` 印成「生成根 X 正在后台生成: 生成根 X 正在后台生成」。E3D 增量夹具播种时正撞上启动的模型补齐任务，
  九个场景一个都起不来（`output/e3d-fixture/20260920-162108`）。不走航班的直路（同进程第一次请求）从来没坏过，所以单测没红。
- **改动**：`handlers::domain_cause<T>` = 顶层 `downcast_ref` 或 `error.chain()` 逐级 `downcast_ref`；`ensure_error` 四处分型都走它。
  `Flights` 本身不动（它已经把原错留在链上，`shared_error_preserves_the_domain_cause` 钉着）。
- **验证**：新 `ensure_error_classifies_domain_causes_buried_under_the_shared_flight`（直给 / `.context` / 经 `Flights` 三种形态 ×
  四种领域错，前提断言「共享航班顶层 downcast 看不见原错」；链上没有领域错照旧 500）；`cargo test --lib` **2349 passed / 0 failed / 121 ignored**。
  live：夹具 `20260920-165908` 播种 12 根，无一 500。

### E3D 增量夹具（`l3_suite --fixture-manifest`）：三处随架构漂移的断口修好，九个场景在真 AMS 项目 7998 上 9/9 PASS

- **为什么**：这套 9 场景（data / transform / geometry / boolean / owner / add / room-structure / room-member / delete）是「部署形态的
  增量 + 房间归属增量」唯一的端到端用例，09-11 隔离端口之后没再跑通过。今天从头跑到底，依次撞出：① 服务从 `run_dir` 起
  （8335b4800 隔离实例锁）而 aios_core 连库时按 CWD 找 `resource/surreal`，`Failed to define common functions … os error 3`；
  ② `project-mirror` 只拷目标 DESI，服务（ADR-078 MDB 闭包）从 SYS **文件**读 `/ALL` 声明，`已尝试 SYS 库：（没有找到任何 SYS 库文件）`，
  房间常驻模型建不起来；③ 配置没钉 `room_membership = true`（ADR-073 起默认 false，基础配置也写 false），两个 `room: true` 场景断言的
  `room_relate` 边根本不会写；④ 播种与启动的模型补齐任务抢同一批根（上一条的 409 → 500）；⑤ drain 助手写死金基线 `API`（8028），
  夹具 18028 上每个场景都去敲一台不存在的服务；⑥ 模型面断言按 `pe->inst_relate` / `->geo_relate` 图跳读——`inst_relate` 是普通表
  （`INSERT IGNORE`，烘焙行没有 `out`）、`geo_relate` 挂在 `inst_info` 上，两边永远空列表，「模型变了」永远为假；⑦ 「重生成的根」
  去 `model_drain.detail.roots` 里找，而 ADR-071 D1 起模型随 dbnum 任务生成，根在 `data_batch.result.units[].root_refno`；
  ⑧ `setup.mac` 的 `/AIOS-INC-DATA-EQ` 没给 POS、落在原点，整只 BOX 在夹具房间 R101 盒里，room-structure 改名房间时它的边跟着掉，
  被判成「无关夹具状态变了」；⑨ 只改名的根照样整根重算（候选照进闭包，e3d-model `affected_closure` 的「宁多勿漏」），
  `inst_relate.model_sesno` 随之跳，data 场景与每一次 restore 对拍都红。
- **改动**（只动运行器与夹具宏，不动服务）：`stage_surreal_scripts` 把 `resource/surreal/*.surql` 拷进 `<run>/resource/surreal`；
  `mirror_project_meta_dbs` 把 `<code>sys / com / mis` 拷到镜像目标 DB 旁（缺 SYS 直接报错）；`write_runtime_config` 钉 `room_membership = true`；
  播种前先 `wait_fixture_pending(…, "startup")`，`ensure_root_model` 对 409 退避重试（预算 5 分钟）；`model_drain_task_ids` /
  `wait_model_drain_settlement` 带 `api` 参数，金基线传 `API`、夹具传 `fixture_api()`；快照 SQL 改成
  `SELECT * FROM inst_relate WHERE in IN $ids` / `FROM geo_relate WHERE geom_refno IN $ids`（unrelated 同款）；
  `regenerated_roots` 从 `data_batch.result.units` 取（`model_drain.detail.roots` 仍认）；`strip_fixture_volatile` 多剥 `model_sesno`，
  `fixture_plane` 也过一遍它；`setup.mac` 给 DATA 设备 `POS E -5000 N 5000 U 0`。README 补「直接对 E3D3.1 真项目 + 7998 跑」的用法。
- **验证**：`cargo test --bin l3_suite` **39 passed**（新钉：脚本先于起服务、镜像带元数据库、`room_membership = true`、只重试 409 且
  settle→seed→settle 顺序、夹具不碰金基线端口、模型面比内容不比戳）。live：`l3_suite --fixture-manifest … --target-db-file
  D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7998_0001 --target-dbnum 7998`（`L3_ALLOW_EXISTING_E3D_SESSION=1`，桌面开着两个
  E3D GUI 会话），`output/e3d-fixture/20260920-165908`：**9/9 PASS**，5m51s；每个场景 apply / restore 两窗 `data_batch` 都 `succeeded`、
  `applied == model == file_latest`、四平面 changed 与期望逐位相同（data：只 attributes；transform / geometry / boolean：+model；
  owner / add / delete：+tree +model；room-structure：+room；room-member：+model +room），room-member 的 BOX `24382/353` R101 → R102 →
  restore 回 R101；teardown 后 7998 里 `/AIOS-INC-*` 一个不剩。前六次失败的现场留在同目录 `20260920-16{0315,1345,2108,2929,3357,5017}`。
- **顺带看见、没改**：⑨ 是行为不是夹具——只改 `NAME` 的 EQUI 根这一窗 `touched=1 regen=1`（`data_batch.result.units` 里
  `kind=regen`），而同一窗回执的候选账 `no_model=2 (metadata_only=1)`、`manual_update` 的预览口径说它 `will_generate=false`。
  执行与预览在这一点上不是一句话；`docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md` §6.2 G6 记的是「类别 0 → 不动 ✅」。
  待拍板：要不要让 `directly_touched` 对只由元数据候选撑起来的闭包命中放行（前移不重算）。

### 移除

- **`scripts/Run-RoomE3DE2E.ps1` / `scripts/Run-Issue7E2E.ps1` 退役（删除）。** 两份脚本的核心一步都是
  `cargo test --test issue7_e2e_increment … issue7_e2e_room_comes_back_after_e3d_save`，而那个测试目标随 ADR-072 R3/R4
  （`588e71915`）删掉了——今天跑它们只会得到「no test target」，房间五连 / 逐类型矩阵一条都验不了。用户 2026-09-20 拍板退役。
  **留下的**：`scripts/e3d/room_*.mac` / `issue7_cap_pos_*.mac` 与 `scripts/e3d/ams_model_type_cases.json`（宏与类型清单，
  别的编排还引用）、`db_options/DbOption-issue7-e2e.toml`（`Start-AiosDatabaseManual.ps1` 的默认配置）。
  **活的等价物**：`l3_suite --fixture-manifest`（`scripts/e3d/increment_fixture/`）的 room-member / room-structure 场景，同日在真
  AMS 7998 上 9/9 PASS。`docs/2026-08-12_live-test-ledger.md` 那一行、`docs/e3d-model-type-verification-ledger.md` 的复跑命令、
  `docs/2026-08-07_e3d-tty-test-plan.md` 顶部各补一句退役追记；`docs/plans/` 与 `specs/` 里的历史提法原样不动。

### `the_extra_stamp_knob_is_off_unless_it_is_given` 跟上 `SPAMAP` 进生产常量：名单外的样本换成合成名字

- **为什么**：e3d-model `cf6d8b6` 把 `PLACEMENT_STAMP_ATTRIBUTES` 定成 `["CACHID", "SPAMAP"]`、本仓 `2175dee93` 的戳表随之跟上，
  这条钉还拿 `SPAMAP` 当「名单外」样本断言 `!stamp_only_with(&diff(&["SPAMAP"]), &[])`，`cargo test --lib` 一直 1 红（09-19 的几条
  记录都写着「仍是那条 SPAMAP 戳」）。
- **改动**：样本换成 `PROBESTAMP`（永远不会进常量的名字）；空名单那一半改成「常量里的每一枚都放行、常量外一个不放行」，
  并逐个遍历 `PLACEMENT_STAMP_ATTRIBUTES` 断言——常量再加名字这条钉不用再改。`stamp_only` / `stamp_only_with` 的注释改口。
- **验证**：`cargo test --lib` 2349 passed / 0 failed；增量集成用例 12 个 target 全绿
  （ctray / equi / hvac_bran / pipe26229 increment、db8000_session_pairs、baseline_stream_8000、db8000_two_delete_fixture、
  db_session_fixture_selfcheck、read_through_rows_parity、model_impact、shared_read_view_invalidation、direct_index_contract：91 passed / 2 ignored）。

## 2026-09-19

### 任务收尾那一笔包进冲突重试：与刷盘 stray epilogue 同写水位行时谁后提交谁输，输了重发而不是让任务落 `Partial`（ADR-073 D13 收口总表第 10 行、钉 36）

- **为什么**：实测（`bin/surreal.exe` 起 8009，memory / 一次性 rocksdb 各 150 × 150 笔、事务内 `SLEEP 5ms` 逼重叠）两台引擎都报
  「Failed to commit transaction due to a read or write conflict. This transaction can be retried」、输的一笔整笔零提交、**两边都会输**
  （memory 刷盘侧 33 / 收尾侧 19，rocksdb 77 / 30）。刷盘侧 `commit_publication_transaction` 早就走 `retry_surreal_write_operation`；
  收尾侧 `commit_task_end_on` 是裸 `db.query` → 输一次 = 任务 `Partial`、模型水位 / `Aligned` 盖章 / 清格都没落（下次任务再欠一次、
  多算一遍），W1 兜底那一笔再输就「下一次启动把这个库退回写入起点重跑」。F3 落库给同一行多了一个写者之后更容易踩到。
- **改动**：`commit_task_end_on` 退成薄壳，发送与回执解读搬进 `commit_task_end_via(end, run)`（`run` = 「把这段 SQL 发出去」，生产
  `db.query`），那一句 + 逐条取错 + 取水位回执包进 `crate::surreal_retry::retry_surreal_write_operation`（每次 `run(sql.clone())`
  重发同一段；只有冲突文本才重试，`THROW` / 语法错照旧第一次就报）。W1 兜底、`dbnum_initialize` 那一处一并受惠。
- **验证**：新 `mem://` 钉 `the_tail_retries_past_a_conflict_with_the_flush_epilogue_on_the_same_row`——第一次发送时竞争者先按刷盘的样子
  `array::union(…, ['g3'])` 改同一行、发送口回 `Error::Db(TxRetryable)`，第二次真发 → 两次尝试、`model_sesno = 26`、
  `model_room_generation = 'g1'`、格空；`Thrown` 一次就报不重发；源码钉薄壳经 `_via`、`_via` 包在重试里。冲突由发送口注入：同一进程里
  一个 `mem://` 句柄上的请求是串行的（本地引擎 router 一次一条；先试过真并发，竞争者要等收尾那 1.5 s 跑完才轮到），撞不出真冲突
  ——顺带说明 `AIOS_STORE_MODE=mem` 下这一行根本撞不上，rocksdb 档才会。`task_end::tests` 26 项全绿；rustfmt hunk 数 == HEAD；CRLF 一致。
  ADR-073 总表第 10 行翻成已收、钉 36。

### 房间 F3 落库收口：水位行那一格里没人认领的代号 → 整库——投影上记「本进程这一世写进那一格的代号」，准入对定向多一问（ADR-073 D13 收口总表第 11 行、钉 35）

- **为什么**：`model_room_stray_generations` 记的是**代号**不是根。重启前那一世按 g2 写过的根，ACK 随重启丢了；重启后下一个任务之前
  又有根按 g3（或按同一代 g2 再）刷了盘，投影里代号不等的 ACK 就只有新的那几根——`scope_for_task` 只点名它们、定向盖章 `Aligned(g1)`
  把整格清掉，g2 那几根的陈旧边就此无人认领。原先「重启后没有 ACK → 整库」只对「一枚 ACK 都没有」成立。
- **改动**：① `ModelMemoryStore::note_stray_generation_written / stray_generations_written`（按库记本进程这一世写进那一格的代号；
  `restore_dbnum` / `clear_dbnum` / `invalidate_receipts_of_dbnum` / `invalidate_all_receipts` 作废 ACK 时一并清）。②
  `model_db_adapter::note_stray_generation_written(db, store, dbnum, rooms, epilogue)`：那一句随本组提交成功之后除作废镜像再记这一代；
  合块 / 二分重试 / 多写者三处同一句。③ `DurableAlignment.stray_written_here` + `unaccounted_stray_generations(on_chain)`：`stray` 去掉
  与库上同代的、链上的（库上 ∪ 各段两端 ∪ 链尾）、这一世写的，剩下的就是点不到的；`request_for_task` 在定向结论之后多一问，非空 →
  整库，原因多一句「……既不在准入链上、也不是本进程这一世写的……整库」。看的是**谁写的**不是**有没有 ACK**：同一代再刷时格里已含、
  这一世没写，新根作证不了旧根。
- **验证**：`cargo test --lib` 新纯函数钉 `a_stray_generation_nobody_here_wrote_and_no_chain_explains_forces_the_whole_library`（同代 / 没人写 /
  这一世写的 / 链上的 / 去重；第 11 行原形单看准入点名一根、`request_for_task` 整库；同一代再刷也整库；链 g1 → g3 时 g2 不在链上整库、
  g1 → g2 → g3 照旧定向）+ 新 `mem://` `a_stray_generation_written_before_the_restart_forces_the_whole_library_even_when_another_generation_is_witnessed`
  （旧投影按 g2 刷 A2、新投影按 g3 刷 A3、格里 `[g2, g3]`、常驻回 g1 → 整库 → 只剩 A1 的边；源码钉四处清理）；`the_library_owes_…` 那条
  「进程还活着」补 `stray_written_here`；钉 34 源码钉改口。相关模块 629 绿 / 1 红（仍是 vendor/e3d-model 那条 SPAMAP 戳）；
  `cargo check --all-targets` 零错；rustfmt hunk 数 == HEAD（0 / 2 / 4）；三个文件 LF 一致。live 待补同钉 34。

### 房间 F3 收口：ADR-073 D13 多一张「已收 / 未收」总表（`76a4dd480` / `3c932e17d` / `35e022318`），钉 34 补记回滚钉

- **为什么**：F3 这一串三笔、两条追记、两组钉散在 D13 / 实施约束里，「到底收了什么、还欠什么」得翻三处才拼得全；`35e022318`
  核掉的那条「合块事务一句失败不回滚」误判也该在 ADR 里有一行落点。
- **改动**：D13 F3 落库追记之后加 12 行总表——已收 8 行（空段定向 / 盖章对齐链上 ACK / 读侧一票否决 / stray 落库 / 事务原子性核实 /
  「进程活着丢 ACK」不构成第二个洞 / ensure 措辞改口 / `PartialCoverage` 同路径），待 live 1 行、未实测 1 行（rocksdb 上水位行两个写者撞事务）、
  **未收 1 行**（stray 里记着的一代在投影里一根 ACK 都没作证时——重启前按 g2 写过、重启后下一任务前又有一段 g3 刷了根——`scope_for_task`
  只点名 g3 那几根、盖章把整格清掉，g2 那几根无人认领；建议规则「stray 里有任一代在 `acked` 里找不到见证 → 整库」，未落）、接受不收 1 行。
  钉 34 多一条 `publication_error_retains_the_cause_after_rollback_errors`。只动文档。

### 网格目录只剩一个解析口：`PLANT_ASSET_ROOT` 压过配置 `meshes_path`，生成落盘 / `/assets/meshes` / 房间面板预热同一个目录（ADR-068 追记 D7，issue #030）

- **为什么**：`options::apply_asset_root` 只把 `PLANT_ASSET_ROOT\meshes` 写进 `DbOptionExt.inner` 那一份克隆，而全仓 22 处直接拿
  `aios_core::get_db_option().get_meshes_path()`（只认配置 `meshes_path`）。配了 `meshes_path` 又设了 `PLANT_ASSET_ROOT` 的部署
  （2026-09-19 f13 隔离部署 `0.1.28+g082d092d7e01`）：初始化把网格写到 `<ws>\meshes`，房间常驻模型去 `<ws>\assets\meshes` 找面板网格
  → 230 块面板 `stage=mesh-warm` 全缺、`room=degraded`、`Watch-InitRun` 永远到不了 DONE，而两库初始化照样 in_sync——两条路径各自都
  「成功」。f20 / f37b 靠 `assets\meshes → meshes` 的 junction 绕过去，没人看见。
- **改动**：`options::meshes_dir_of / meshes_dir_for / meshes_dir`（`PLANT_ASSET_ROOT` → 配置 `meshes_path` → `asset_root()/meshes`）
  是唯一解析口；22 处直调全改走它，`apply_asset_root` 退成把答案写回 `meshes_path`；`clippy.toml` `disallowed-methods` 禁
  `DbOption::get_meshes_path`（lib 顶上 `#![allow(warnings)]` 之后补 `#![deny(clippy::disallowed_methods)]`，负向探针实测拦得住），
  `meshes_dir_is_the_only_resolver` 扫 `src/` 再守一遍；配置被压掉时 `run_app` / `run_cli` 启动打一次
  `⚠ DbOption.toml 里的 meshes_path = … 被环境变量 PLANT_ASSET_ROOT 压过 …`（`meshes_dir_notice`）。`mesh_glb` 那条钉改口成钉
  `crate::options::meshes_dir()`。
- **验证**：`cargo test --release --lib -- meshes_dir apply_asset_root mesh_glb_handler …` 新增四条 + 改口一条全绿；live：f13 同一份
  配置、拆掉 junction、清库重跑——启动日志出现那句 ⚠，`[房间初始化] … 230/230`、`room=ready`，网格全部落在 `<ws>\assets\meshes`、
  配置指的 `<ws>\meshes` 保持为空，两库 in_sync、`Watch-InitRun` DONE（数字见 ADR-068 D7 末尾）。

### 房间 F3 落库：水位行多一格 `model_room_stray_generations`——盖章之后按另一代写过根，重启不丢（ADR-073 D13 追记 F3 落库、钉 34）

- **为什么**：F3 的证据（进程内 ACK 的房间代号）随重启消失。库上 g1、中间那一代 g2 由 write-behind 刷进库的根、常驻回到 g1、
  **重启**：③ 不欠（同代）、账本没段、投影空——读侧第二层替那几根说「就绪」，没人再点名它们，直到那块面板自己再动或某次整库。
  用户拍板 B：不逐根落（那是把 ADR-079 刚退役的 `gen_root` 换个名字请回来），不单开小表（借水位行现成的镜像读与清库路径）。
- **改动**：① `dbnum_watermark.model_room_stray_generations: [代号]`（`DbnumState` / 行结构 / 列清单 / 两处装配）。② 刷盘一组按 `rooms`
  这一代写边、而这一代 ≠ 库上 `model_room_generation` 时，`model_db_adapter::stray_room_generation_statement` 渲一句
  `UPDATE … SET model_room_stray_generations = array::union(model_room_stray_generations ?? [], ['g'])`，作 `persist_group` 的 epilogue
  **随本组的行同一笔事务**提交（一条几何都没有的组不带；合块 / 二分重试 / 多写者三处都带、成功后 `invalidate_mirrored_on`）；没有行 /
  没盖过 / 同代 / 已含都不写——稳态一个字都不写。③ `task_end` 的 `Aligned` / `Cleared` 两臂一并 `= []`，`Untouched` 不碰。
  ④ `room_recompute::DurableAlignment { stray }`，`owed()` 多一句 `stray_generations_owed`（去掉与库上同代的；非空 = 欠，「F3 落库」）；
  `request_for_task` 随之走 `scope_for_task`：进程内还有代号不等的 ACK → 空段定向点名；重启后没有 → 整库，盖章即清。
  ⑤ `/dbnums.room_alignment.stray_generations`（空时不出，形状不变）。ADR-073 F3 追记改口（ensure 只发投影、边是 write-behind
  刷盘那一刻写的）+ F3 落库追记 + D14 / W3 注 + 钉 34。
- **验证**：`cargo test --lib` 房间 / 水位相关 18 项全绿——新 `mem://` `a_stray_generation_survives_a_restart_on_the_watermark_row_and_rearms_the_recompute`
  （没有行 / 没盖过 / 同代不写；按 g2 刷 A2 同事务记下 `[g2]`、`failed` 为空、已含不再写；换空投影模拟重启 → `owed()` 含「F3 落库」→
  整库 → A2 陈旧边删掉；源码钉 epilogue 与三处作废镜像）、新纯函数 `the_library_owes_a_recompute_when_the_watermark_remembers_writes_at_another_generation`；
  `the_room_generation_stamp_follows_the_model_watermark`（种子 `['g2']`、普通窗口原样、盖章 / 清空都清）、`the_dbnum_row_carries_…`（空不出 /
  非空多一格）、`the_tail_is_one_transaction_…`（说不了整库的任务不碰这一格）、`room_membership_is_computed_per_root_…`（epilogue 字面）改口 / 扩。
  相关模块 627 绿 / 1 红（仍是 vendor/e3d-model 别的会话未提交的 SPAMAP 戳）；`cargo check --all-targets` 零错；rustfmt hunk 数 ≤ HEAD；
  CRLF 一致。踩坑一条：这一版 SurrealDB 不给数组做 `+`（"Cannot perform addition with '[]' and '[…]'"）。**更正（09-19 晚）**：原记的「合块事务里一句失败不回滚
  其余语句」是误判——用 `bin/surreal.exe`（2.1.4+45013fc9，与 SDK 同一提交、同一台执行器）在 memory 与 rocksdb 上都核过：`BEGIN … COMMIT` 里
  任一句报错（`THROW` / 数组 `+`）整笔零提交、其余各句回「not executed due to a failed transaction」；当时看见 A2 的边照样落库，是那一版 epilogue
  还没进同一笔事务。`publication_error_retains_the_cause_after_rollback_errors` 现在连回滚本身也钉住。测试仍要断言 `failed` 为空——否则那一句悄悄没写。live 待补：`.mesh` 暂时读不出 → 点「显示模型」
  → 恢复 → 重启 → 下一窗「库上记着盖章之后本库有根按另 1 代写过边 … F3 落库」+ 整库；稳态下那一列恒空。

### 房间定向重算：常驻代号回到旧值时中间那一代生成的根——投影里 ACK 代号不在链上的根点名重算、盖章时对齐链上 ACK、读侧一票否决（ADR-073 D13 追记 F3）

- **为什么**：常驻代号是输入摘要、不单调。库上盖 g1 → 一块 PANE 的 `.mesh` 暂时读不出（重建 → g2，进保护集这一段是空段、不进账本）
  → 期间「显示模型」`/model/ensure` 按 g2 生成了几根（P 不在索引，进 P 的根拿不到 P 的边）→ `.mesh` 恢复（重建 → g1，摘要同值）→ 下一个
  批次任务 `Unchanged`、库上 g1 == 冻的 g1 → ③ 不欠、账本没段 → `request_for_task` 回 `None`；那几根 ACK 是 g2，`e3d.room.lookup` 第一层
  不认、第二层「库上 g1 == 常驻 g1」→ 回 `ready`，边却是 g2 算的。面板挪走又挪回（撤销）同形。
- **改动**：① 投影里本库每一根 ACK 的房间代号进准入——`room_recompute::acked_roots_for_task`（只收回执与 ACK 对得上的），两个执行体
  在 `swaps_for_task` 之后取、传进 `request_for_task`；`scope_for_task(…, acked)` 在库上 == 常驻时把代号 ≠ 这一代的根点成
  `Targeted { swaps: [], parked: [], stale_roots }`（空段定向），链接得通时把代号不在链上任一代的根一并进 `stale_roots`；
  `request_for_task` 多第四句原因，库上同代、前三条都没点亮时 F3 单独触发，被 ①/② 问到仍整库。`run_targeted_room_recompute` 把点名的
  根与反查命中、停放折出的并起来。② 定向成功盖章那一刻 `model_db_adapter::align_room_acks_along_chain` 把投影里 ACK 代号在链上任一代、
  没被命中的根对齐到链尾（它那一代到链尾之间面板动了什么各段都反查过——与库上那一格是同一句话的逐根形态；没有它，下一个任务会把整个
  投影点名一遍）；链外的、没算房间的不动——那是证据。`recompute_dbnum_room_edges` 对投影里一条几何都没有的根也对齐 ACK。③ 读侧
  `query_service::room_calculation_status` 第 1 层改三态 `ProcessAck::{Missing, Current, Stale}`，`Stale`（有 ACK 而代号 ≠ 常驻 / 没算房间）
  一票否决，库上那一层再对也不替它说话。ADR-073 D13 追记 F3、D14「读」补一句、钉 33。
- **验证**：`cargo test --lib` 房间相关 35 项全绿——新 `a_stale_ack_in_an_aligned_library_is_the_debt_itself_and_goes_targeted_on_its_own`
  （纯函数十余例）、新 `mem://` `a_root_generated_in_a_transient_generation_is_repaired_when_the_resident_returns`（库上 g1 发 A1 → ensure 按 g2
  发 A2 与空根 A3 → 回 g1：空段定向只点名 g2 那两根、按 g1 重算 A2 陈旧边删掉、A1 不动、A3 的 ACK 也对齐、再问不欠、== 整库按 g1；
  `align_room_acks_along_chain` 链上对齐 / 链外与没算房间的不动）；钉 `the_room_only_recompute_is_wired_…`（`swaps < acked < request`）、
  `the_targeted_recompute_falls_back_…`（`stale_roots` 并根、对齐在盖章那一臂）、`the_durable_alignment_answers_…`（`Stale` 四例）、
  `the_room_status_reads_both_…`（三态字面）改口 / 扩。相关模块 575 绿 / 1 红（`the_extra_stamp_knob_is_off_unless_it_is_given`——
  `vendor/e3d-model` 工作树里别的会话未提交的 SPAMAP 戳，与房间无关）；`cargo check --all-targets` 零错；rustfmt 四个文件 hunk 数与 HEAD
  相同（6/0/3/5）。**没收的**：ACK 是进程内的——中间那一代生成的根若在下一个任务之前随重启丢了 ACK，库上 == 常驻之下没人再点名它；
  要收得把 ACK 代号落库，等 live 看到再定。live 待补：`.mesh` 暂时读不出 → 点「显示模型」→ 恢复 → 下一窗「… ACK 代号不在链上点名 N 根」、
  两窗之间那几根回 `not_computed`；普通定向之后「链上 K 代、没命中的 M 根 ACK … 对齐到 …」且下一窗不再点名整个投影。

### 房间定向重算：面板级语句挪到最后一笔、停放在反查完成就做，补 `recalc` 面板自己的 `room_panel_relate`（ADR-073 D13 追记 F1 / F2）

- **为什么**：第三遍复审读出 M1 的一条连带——ADR ⑤ 写的「`Failed` → 下一个任务整库」在 M1 之后不成立：账本里那一段还在，下一个
  任务照样接链、仍是定向；而定向的面板级删（不在册面板的 `{P}->room_relate` / `<-room_panel_relate`）装在第一笔事务、逐笔提交失败即返，
  第一笔提交后若后面某笔写库失败，P 名下成员的边已删、没提交那几笔里的根新边没写，下一个任务重走同一段时 `durable_panel_members(P)`
  回空、P 也没新盒——这些成员若该落到别的房间永远没人算（跨库更直接：`Failed` 不停放，B 库接段时证据已被 A 删掉）。另一条：
  `render_panel_delta_statements` 只处理 `removed` / `renumbered`，`recalc` 面板自己的「房间 → 面板」边只有它自己的根被重算才写，而反查
  命中的是成员的根——FRMW 改名进关键字（NAME 是元数据、根不重生成）→ 成员的 `room_relate` 齐了、面板的 `room_panel_relate` 一条没有，
  「两张边 == 从零初始化」在第二张表上不成立（今天没有生产读者，所以看不见）。
- **改动**：F1——`recompute_dbnum_room_edges` 里面板级语句从 `pack_publication_transactions` 的 prelude 挪到 epilogue（最后一笔）：命中的
  根都重写之后才删面板级的边，中途失败留下的是多算形状、证据还在；`run_targeted_room_recompute` 的 `park_foreign_hits` 从「结论定为
  `Targeted` 之后」提前到反查完成、分类过关之后、重算之前，不以结论为条件（同段再停是并集，重试幂等）。F2——
  `MemoryRoomCalculator::render_panel_topology_statements(panel)` + `model_db_adapter::render_recalc_panel_topology_statements(rooms, panels)`
  （按本任务计算器的在册视图，refno 序去重，在册先删后插、不在册只删），`run_targeted_room_recompute` 拼在各段面板级语句之后。
  ADR-073 D13 追记 F1 / F2、⑤ 与钉 22 / 23 改口、钉 32。
- **验证**：`cargo test --lib` 定向 30 项全绿——钉 `the_targeted_recompute_falls_back_…` 改口（`classified < park < recompute`、`park < decided`、
  epilogue 字面）；`mem://` `a_targeted_recompute_rewrites_only_the_named_roots_and_runs_the_panel_statements_last`（改名，终态不变）；新
  `recalc_panel_topology_statements_follow_the_calculators_registered_view`（五例）、新
  `a_targeted_recompute_writes_the_recalc_panels_own_topology_edge`（R2 新在册、只点名成员 A → 陈旧 999 → R2 删掉、601 → R2 写上、
  别的面板现行边不动、成员边照常；再跑一遍幂等）。相关模块 398 绿 / 1 红（`the_extra_stamp_knob_is_off_unless_it_is_given`——
  `vendor/e3d-model` 工作树里别的会话把 `PLACEMENT_STAMP_ATTRIBUTES` 改成 `["CACHID","SPAMAP"]` 未提交，与房间无关）；
  `cargo check --all-targets` 零错；rustfmt 四个文件 hunk 数与 HEAD 相同；CRLF 一致。live 待补：FRMW 改名进关键字 → 每块新在册面板
  各一条 `room_panel_relate`；定向中途写库失败一次 → 下一窗同一段重走仍完整。

### 房间反查：旧盒退役——只问新盒 + 库上成员边，`protected_bounds` 删掉（ADR-073 D13 追记 L1-P 第二步）

- **为什么**：L1-P（`1e7dbfd91`）之后旧盒只剩一个用处——在途还没落库的根靠它；而那些根随它自己那一台计算器落库、由 ③ 兜底，
  不值得每次定向多扫一遍旧盒，更不值得为它在模型上跨代带一份「上一次可用盒」（L1 的 `protected_bounds`，只活在进程内）。
- **改动**：`PanelDelta::query_boxes()` 只回 `recalc` 的新盒；`PanelMove::old` 只给日志 / 用例看（取旧代 `current_bounds`，保护中 /
  不在册 = `None`）；`ResidentRoomModel::protected_bounds` / `last_known_bounds` / `inherit_protected_bounds` 删掉、`install` 不再带盒，
  新 `current_bounds(panel)`。反查定型为「新盒问空间谁进来了、库上成员边答谁离开了」。ADR-073 D13 L1 段标退役、L1-P 段加第二步、
  钉 27 标历史、钉 30 改口。
- **验证**：`cargo test --lib` 定向 25 项全绿（钉 `a_panel_that_regains_geometry_…` 改名 `…_is_looked_up_by_its_new_box_and_its_durable_members`：
  保护 → 仍保护 `query_boxes == [2.0]` → 恢复 `[40.0]`、`leaving_panels == [8]`、账本末段一个盒；`install_reports_…` 第二代
  `query_boxes == [5.0, 20.0]`）；相关模块 394 绿 0 红；`cargo check --all-targets` 零错；rustfmt hunk 数与 HEAD 相同；CRLF 一致。

### 房间反查：离开的成员从库上按面板取、范围之外的历史几何不再把定向拉回整库（ADR-073 D13 追记 L1-P / M2-S；撤 L1-R 的门）

- **为什么**：复审 L1（`b21613278`）读出它的漏洞——上一次可用盒只活在进程内，而**丢盒的方向是漏算**：面板有几何、有边 → `.mesh`
  读不出进保护集 → **重启** → 第一代它仍在保护集、没有上一代可带 → 恢复且挪位 → `old = None`，只问新盒，旧位置里成员的陈旧边没人清。
  同日第一版（`2ba772b25`，L1-R）是保守门：第一代就在保护集的面板记成「旧盒说不出」，恢复那一段不许定向、整库兜底。这一笔改成精确
  做法、门撤掉。另一条：M2 分完之后 `stuck` 里的常客是「元素在 T 上、祖先链每一级也都在、却没有一级是生成根」的历史几何
  （`model_type_scope` 之外）——每一次盒扫得到它的定向都退整库，而整库也不算它，持久多算、零收益。
- **改动**：L1-P——「离开的成员」不问空间：此前算在面板名下的每一个成员都在库上（`{panel}->room_relate` 的 `out` 端，`in` 走索引），
  `room_reverse::durable_panel_members` 按 `PanelDelta::leaving_panels()`（`recalc` 每一块 + `removed` 每一块）一块一句取齐，与树上命中
  同一处置进 `resolve`（祖先链投影拿、回库取；`inst_relate` 没有行的管身来源再按 `in` 到 `tubi_relate` 取）；别的库的成员照样进 `foreign`
  停放。集合精确、不靠旧盒、重启不丢。旧盒 ∪ 新盒的空间查询照旧（在途未落库的根靠它）。顺手收掉同族漏算：`removed` 的面板此前只清边
  不重算，成员若还在别的面板盒里该落到那间房却没人算——现在它的成员根也重算。`AffectedRoots` 多 `member_hits` 等三格、`summary()`
  多一格、`durable_timing()` 多一句。撤 L1-R：`unknown_bounds` / `old_box_unknown` / `fully_known` 删掉，`walk_chain` 回到 `!is_empty()`。
  M2-S——反查 / 停放折不出根的元素连同祖先链一起回（`unresolved_ancestors`）；`run_targeted_room_recompute` 在 `stuck` 非空时再问一次
  `missing_at_target`（链上每一级），`split_stuck_by_ancestry`：链非空且每一级都在 = 范围之外，忽略（stdout 点名）；链空 / 有一级已不在
  （改过属主，链陈旧）= 仍卡住，退整库。ADR-073 D13 追记 L1-P / M2-S、源码钉 30（改口）/ 31。
- **验证**：`cargo test --lib` 定向 25 项全绿——`resolve_joins_…` 扩成四路（面板 7 名下四条成员边：投影里的、`inst_relate` 带 anc 的、
  盒远在 [50, 51] 只有 `tubi_relate` 两行的管身来源、别的库的 → `member_hits = 4`、`durable_ancestor_lookups = 7`、foreign 多一个；只给成员
  不给树与盒也折出 A/C/D）；两库端到端 `durable_panel_members(R1) == 树的答案模拟`；新 `stuck_hits_whose_whole_ancestor_chain_still_exists_…`
  （四例 + 拦 → 问祖先 → 分 → 只拦说不了的 → 重算的顺序钉）；`hits_fold_to_every_library_root_…` / `parked_elements_fold_…` 改口带链；
  `install_reports_…` / `a_panel_that_regains_geometry_…` 加 `leaving_panels` 断言。`batch_worker room_ model_db_adapter query_service
  task_end dbnum_initialize window_root_plan model_dbnum_ensure handlers` 394 绿 0 红；`cargo check --all-targets` 零错；rustfmt 五个文件
  hunk 数与 HEAD 相同；CRLF 文件行尾一致。live 待补：PANE `.mesh` 读不出 → 重启 → 恢复并挪位 → 「定向房间重算完成 … member_hits=N」
  且旧位置陈旧边被删；面板移出关键字 → 成员落到别的房间；盒扫到范围之外的历史几何 → 「… 忽略」仍定向。

### 房间反查：保护集成员带着上一次可用的盒、定向成功只清消费过的停放段、库读计时（ADR-073 D13 追记 L1 / L3 / L2）

- **为什么**：审核（`output/show-me-room-increment.html` §06）剩下的三条低档。L1 是一条窄漏算：一块面板「有几何、有边 → 某代
  几何不可用进保护集（旧边留着）→ 再一代几何可用但位置变了」，`PanelDelta::between` 的旧盒取自旧代 `panels`、保护中是 `None`
  → 反查只问新盒，旧位置里成员的陈旧边没人清。L3 是加固：`parked_for_task` 开工时快照本库的槽，收尾 `clear(dbnum)` 却清此刻的
  全部，任务期间别的库停进来的段没折过根就随手丢了（不漏算，账本里还有那一段）。L2 是性能：`durable_tube_hits` O(全部
  `tubi_relate` 行) 的链接解引用 live 从没量过，ADR 列的缓解（`aabb_d` 平副本）要么回填存量、要么查询分两半，没有数不动查询。
- **改动**：L1——`ResidentRoomModel::protected_bounds` + `last_known_bounds`，`ResidentRooms::install` 算面板变化之前从被换下的那一代
  给这一代 `missing_panels` 里每一块带一份上一次可用盒（跨几代都带，重新有几何就丢），`PanelDelta::between` 旧盒走
  `last_known_bounds`。L3——`room_reverse::clear_consumed(dbnum, consumed)` 按 `from → to` 对条、按元素去、去空整条撤；
  `run_room_recompute` 按 `(结论, 范围)` 分派：定向按接进准入链的快照 `clear_consumed`，整库仍整槽 `clear`。L2——
  `AffectedRoots { durable_tube_ms, durable_ancestor_lookups, durable_ancestor_ms }` + `durable_timing()`，「定向房间重算完成」那一行多一句
  「（库上管身范围查 Nms、回库取祖先链 K 个 Mms）」。ADR-073 D13 追记 L1 / L3 / L2、源码钉 27–29。
- **验证**：`cargo test --lib` 定向 32 项全绿（新 `a_panel_that_regains_geometry_after_protection_brings_its_last_usable_box_as_the_old_one`
  四代用例 + 反例；新 `clearing_the_consumed_segments_keeps_what_was_parked_while_the_task_ran`；`resolve_joins_…` 扩量具；分派处源码钉改口；
  `mem://` 两库端到端 B 收尾改按快照清）；`batch_worker room_ model_db_adapter query_service task_end dbnum_initialize window_root_plan
  model_dbnum_ensure handlers` 392 绿 0 红；`cargo check --all-targets` 零错；rustfmt 我改的块整平（三个文件的剩余 hunk 数与 HEAD 相同）。
  live 待补：一块 PANE 先让 `.mesh` 读不出再恢复并挪位 → 「定向房间重算完成 … recalc=1」且旧位置成员的陈旧边被删；7997 一次定向的
  「库上管身范围查 Nms」压过总耗时才做平副本。

### 房间反查折不出根的元素按「在 DbSet@T 上还在不在」分两半，删掉的残留不再把定向拉回整库（ADR-073 D13 追记 M2）

- **为什么**：审核（`output/show-me-room-increment.html` §06 M2）——反查命中却折不出根（`anc ∪ 自身 ∩ DbSet@T 根清单 = ∅`）
  的元素此前任一个就退整库，而最常见的来源是**本窗刚删掉的根**：树 / 库上的残留还没刷掉、被面板的盒扫到，它在 T 上没有边要
  维护（对账删除那一半清），却让每一次「删根 + 挪面板」同窗的定向都变整库。
- **改动**：`window_root_plan::missing_at_target(dbnum, T, refnos)` 拿同一个 `DbSet@T`（`build_set` 走生成输入缓存）逐个
  `element_exists`；`run_targeted_room_recompute` 把反查命中的与停放链上折不出根的元素合起来问一次，
  `split_unresolved_by_existence` 分两半——已不存在的忽略（stdout 点名）、还在 T 上的才是「反查不完整」退整库；问不出来（读文件
  失败）也退整库，不把「不知道」当「不在」。`room_reverse` 不变：仍如实把折不出根的进 `unresolved`，分类在调用方。ADR-073 D13
  追记 M2、源码钉 26。
- **验证**：`cargo test --lib` 定向 30 项全绿（新 `unresolved_hits_that_no_longer_exist_at_the_target_are_ignored_and_the_rest_block_the_targeted_path`：
  纯函数四例 + 源码钉「问 → 分 → 拦 → 重算」顺序、旧的一刀切分支已删）；`batch_worker room_ model_db_adapter query_service task_end
  dbnum_initialize window_root_plan model_dbnum_ensure handlers` 390 绿 0 红；`cargo check --all-targets` 零错。live 待补：一窗里
  删一个根 + 挪一块面板 → 日志「N 个命中的元素在 DbSet@T 上已不存在 … 忽略」且仍「定向房间重算完成」。

### 房间换代段账本：谁重建谁记，任何任务的定向准入都能沿它接段（ADR-073 D13 追记 M1）

- **为什么**：审核（`output/show-me-room-increment.html` §06 M1）指出定向常路在生产里很难走到——面板变化 `PanelDelta` 只随
  `Rebuilt` 回给做了重建的那一次 `refresh_before_task`。`/model/ensure`（UI「显示模型」）先探到 composite 变了、做了重建，
  随后每个库的任务都是 `Unchanged` + 库上欠着 + 槽空 → 链断 → 全 MDB 每个库各整库一次；截断窗口（大窗口的常态）跑不到收尾，
  自己那一段随任务丢掉，下一窗又 `Unchanged` → 整库；没被命中的库看不到「这一段与你无关」→ 整库。N 个 DESI 库一次面板变化最坏
  N 次整库（每次 O(本库根数) 次库读 + 重写全部边）。
- **改动**：`room_resident::SwapLedger`（`ResidentRooms` 的一个字段，与两代模型同一把锁）——`install()` 对着被换下的那一代算出
  非空 `PanelDelta` 就记一段 `PanelSwap { from, to, panels }`（两端 = 两代 `input_version`），空段不记、同两代替换、容量 64；
  `PanelSwap` 搬到 `room_resident`；`scope_for_task(library, resident, swaps, parked)` 改成沿账本段 / 停放段的深度优先路径搜索
  （同一起点停放段优先、一条路断了换另一条），`Targeted { swaps: Vec<PanelSwap>, parked, generation }` 链上可多段；
  `swaps_for_task` 在两个执行体里紧跟 `parked_for_task`；`run_targeted_room_recompute` 每一段各反查、面板级语句按链序拼、
  **按段**停放给别的库；`/health.room_build.resident.swap_ledger`。H1 那条竞态（对齐 g1→g2、ensure 中途 g2→g3、冻 g3）从
  「整库」升成「两段定向」。停放槽保留（省一次反查的捷径）。ADR-073 D13 追记 M1、源码钉 25。
- **验证**：`cargo test --lib` 定向 31 项全绿（`SwapLedger` 纯函数、`install` 记段、`scope_for_task` 两张真值表重写、`mem://`
  端到端 ④ 改成「T2 沿 ensure 记的 g2→g3 定向，与整库按 g3 同一终态」）；`batch_worker room_ model_db_adapter query_service task_end
  dbnum_initialize window_root_plan model_dbnum_ensure handlers` 389 绿 0 红；`cargo check --all-targets` 零错。
  live 待补：两个 DESI 库、UI 先点「显示模型」让 ensure 做重建 → 两个库的任务都「定向房间重算完成（… 换代段 1 段 …）」、
  `swap_ledger.entries ≥ 1`、没有一个库整库。

### 房间定向重算复用本任务开工冻的计算器——scope、边、盖的代号同一代（ADR-073 D13 追记 H1）

- **为什么**：审核（`output/show-me-room-increment.html` §06 H1）读出一条漏算。定向路（反向传播）的 scope 在开工时按读到的
  常驻代号定（库上 g1 → 常驻 g2，自己这一段面板变化 P），收尾却 `frozen_room_calculator()` **新冻一台**「此刻这一代」。P2-3 自己
  点名的竞态（一个 DESI 前进 + 用户同时点「显示模型」，`/model/ensure` 并发对齐在任务中途把常驻 g2 → g3、面板 Q 又动了）下：
  反查只用 P 的盒、命中根按 g3 算、`Targeted(g3)` 盖 D14 ← g3；Q 命中的根从此永久陈旧，③ 看到库上 g3 == 常驻 g3 就哑，
  `park_foreign_hits(g1 → g3)` 还把同一个洞停放给别的库。准入侧同病：`Rebuilt` 不带自己的新代号，`scope_for_task` 默认自己那一段
  的 `to` 就是此刻常驻，而「此刻常驻」是另一次独立的读。整库那一路没事（每一根都算）。
- **改动**：`RoomReferenceRefresh::Rebuilt` 多一格 `generation`（装上那一代的 `input_version`）；`PanelSwap` 带 `to_generation`，
  `RoomRecomputeScope::targeted_generation()` = 链尾那一段的 `to`；`scope_for_task` 取自己那一段时要求 `generation == resident`，
  不等整库（停放物不替它续）；`TaskRoomCalculator::generation()`，两个执行体 `durable_alignment(job.dbnum, rooms.generation())`
  ——「此刻常驻代号」只从冻好的那台取，不再另读生命周期；冻的那台随 `DataBatchTaskResult::room_calculator`（`#[serde(skip)]`）
  交给收尾段，`run_room_recompute(… task_rooms.as_ref() …)`，`run_targeted_room_recompute` 收 `expected_generation` 与那台计算器、
  **不新冻**，入口先对代号、不等 / 没冻出退整库，停放以 `&swap.to_generation` 收尾；`MemoryRoomCalculator` 实现 `Debug`（只报代号）。
  `run_room_only_recompute` 一字不改。ADR-073 D13 追记 + 源码钉 17 改口、新钉 24。
- **验证**：`cargo test --lib` 定向 28 项全绿（含新 `mem://` 端到端
  `a_mid_task_generation_swap_keeps_the_targeted_recompute_on_the_task_calculator`：T1 对齐 g1→g2、冻 g2、中途常驻 g3 → 按冻的 g2
  定向、反查命中 A1/A2 按 g2 算、盖 g2 之后 `library_alignment_owed(g2, g3)` 仍欠、盖 g3 就不欠——反例；T2 整库按 g3 == 从零；
  `scope_for_task` 真值表加三行；源码钉扩：执行体 `refresh → freeze → durable_alignment(job.dbnum, rooms.generation()) → parked_for_task →
  request_for_task`、`room_calculator: rooms.frozen().cloned()`、不含 `resident_room_generation()`；分派与定向函数体不含
  `frozen_room_calculator()`、入口代号守门在 `library_generation_roots(` 之前 `return None`）；`batch_worker room_ model_db_adapter
  query_service task_end dbnum_initialize window_root_plan model_dbnum_ensure` 352 绿 0 红；`cargo check --all-targets` 零错。
  live 待补：ams 一窗挪 PANE + 同时点「显示模型」→ 日志「定向房间重算完成（… 计算器 <开工那一代> …）」或「准入链终点 … 不是本任务
  冻的计算器代号 … 退整库」，下一窗 ③ 再点一次。

## 2026-09-18

### 存储介质只由环境变量 `AIOS_STORE_MODE` 决定；`store_mode` / `in_memory_db` 退出配置文件与向导（ADR-080）

- **为什么**：介质此前由三层叠加决定（环境变量 `AIOS_STORE_MODE` > `DbOption.toml` 的 `store_mode` > 旧开关
  `in_memory_db` / `AIOS_IN_MEMORY_DB`）。今天现场真踩了：文件写着 `store_mode = "rocksdb"`，上一条命令残留在会话里的
  `AIOS_STORE_MODE=mem` 把它压成内存库——服务照常起、模型照常对、属性行一直不进库（mem 派生 read-through），日志一个字
  没提。先补了「环境变量压过配置要出声」与横幅上的「谁定的」，用户随后拍板去根：**只允许环境变量控制**。介质是
  「这一次进程怎么起」的事，跟启动命令走；配置文件描述「这个工程长什么样」，不该替进程挑介质。
- **改动**：`options::store_mode()` 只读 `AIOS_STORE_MODE`（`mem` / `rocksdb`，不设 = `mem`，退役别名照认），
  `effective_store_mode(env)` 签名里没有配置那一格；`store_mode_notice` → `store_mode_notices()`，文件里残留的
  `store_mode` / `in_memory_db` 与旧环境变量 `AIOS_IN_MEMORY_DB` 逐条点名（文件说 X、实际跑 Y 时并排念出来并给出
  `AIOS_STORE_MODE=X`），`run_cli` 逐条打印；`in_memory_db()` 删除；`store_mode_source` 只剩「环境变量 / 默认」两种答案。
  向导第②步不再有两个 radio：只展示生效介质与出处（`GET /api/v1/setup/config` 的 `effective` 多出 `store_mode_source` /
  `store_mode_env` / `store_mode_env_value` / `stale_config_store_mode`），文件里残留过期键时提醒删掉；`PUT` 收下 `store_mode`
  但不写盘、不校验；`ensure_completion_ready` 以 `options::store_mode()` 为准。校审拒绝启用的三处文案改法指到
  `AIOS_STORE_MODE=rocksdb`。仓内 16 份 `DbOption*.toml`（含仓根与出厂模板）把 `store_mode = ...` 换成退役注释并记下原值；
  护栏从「每份必须声明」反转为 `no_db_option_fixture_still_carries_a_store_mode_key`。`New-LocalRelease.ps1` 生成的
  `Start-AMS.ps1` 新增 `-StoreMode mem|rocksdb`，冒烟 B / C 腿不再往临时配置写 `store_mode`、只递环境变量；
  `Start-Surreal8009.ps1` / `strt_plant_ui_runtime.py` / `e3d_persist_smoke` 改口。Linux 部署脚本（`deploy/linux/`）
  `aios-database` 单元加 `Environment=AIOS_STORE_MODE=rocksdb`、渲染不再写 `store_mode`（现役 0.1.27 也认这个环境变量）。
- **对现场的影响**：只靠文件写 `rocksdb` 起服务的手工习惯要改成带 `$env:AIOS_STORE_MODE='rocksdb'`
  （仓内 `Start-AiosDatabaseManual.ps1` / `Run-*.ps1` / `Verify-RocksDb8000.ps1` / `l3_suite` 本来就带，不必改）；不带的话
  服务按 `mem` 起来，启动日志会整行点名文件里那句 `rocksdb` 已不生效并给出该设的值。发布包要持久库：先起库、向导填地址、
  再 `Start-AMS.ps1 -StoreMode rocksdb`。
- **验证**：`cargo check --lib --tests` 零错；`cargo test --lib options::tests` 73 绿、`web_service::setup::tests` 22 绿、
  `web_service::review::db` 8 绿，`cargo test --lib store` 95 绿 0 红（含护栏 `no_db_option_fixture_still_carries_a_store_mode_key`
  真扫过 16 份 `DbOption*.toml`）。另补一条服务端侧的钉：`the_config_endpoint_reports_the_effective_store_mode_and_names_a_stale_key`
  ——此前只钉了页面读哪几格、没钉服务端发哪几格，改个名两条都不红而页面拿到 `undefined`（**已按负例验过**：
  把 `store_mode_source` 改名即红）。`New-LocalRelease.ps1`、内嵌的 `Start-AMS.ps1`（96 行）、`Start-Surreal8009.ps1` 通过
  PowerShell 语法解析，`strt_plant_ui_runtime.py` 通过 `py_compile`，`deploy/linux/install_backend.sh` 通过 `bash -n`、
  渲染出的 TOML 解析通过且不含 `store_mode`。（真机横幅与 `/setup.html` 第②步的实际渲染已在下一条里验了；
  发布包冒烟三腿在下面第三条里真跑过了。）

### 同日修订：漏设 `AIOS_STORE_MODE` 时落到 `rocksdb`，发布包改由 `Start-AMS.ps1` 显式回填 `mem`（ADR-080 D6）

- **为什么**：上一条落地后真机复验，把两种漏法的代价摆到了一起。兜底 `mem`（原 ADR-074 D3）时服务照常起、模型照常对，
  只有属性行一直不进库、校审域整域 503、重启即清空——2026-09-18 现场为此查了半天，而且是先怀疑增量窗口才找到这儿；
  兜底 `rocksdb` 时地址上没库就当场 `SurrealDB 连接失败（ws://…）` 退出（exit 1），一眼就知道漏了什么。默认值兜的是
  「人没说」，不该兜到一个会静默换掉数据面的档上。用户 2026-09-18 拍板翻过来。
- **改动**：`effective_store_mode` 的兜底 `Mem` → `RocksDb`（不设 / 空白 / 拼错都落 rocksdb，`mem` 仍可显式选）；
  `store_mode_source` 的那句「默认」改成「默认 rocksdb（环境变量 AIOS_STORE_MODE 没设或认不出）」，横幅与向导跟着变。
  「解压即跑」不再靠默认兜：生成的 `Start-AMS.ps1` 不给 `-StoreMode`、调用方环境里也没有时**显式回填**
  `AIOS_STORE_MODE=mem`（给了 `-StoreMode` 以它为准；环境里已有的照旧沿用，不被盖），老启动器 `start-local-release.ps1`
  同样回填。冒烟 A 腿有意仍不设介质——它考的正是「落到默认档也仍然拐进设置模式、向导打得开」这条逃生门。
  护栏 `the_default_store_mode_is_mem` → `the_default_store_mode_is_rocksdb`；ADR-074 D3 整条被 ADR-080 D6 取代。
- **对现场的影响**：配置填好、机器上没库还手工直起服务的，会从「起来了但跑在内存库上」变成「起不来，报连不上」——
  改法是 `$env:AIOS_STORE_MODE='mem'`，失败信息里带着地址。仓内所有起服务的脚本本来就显式带介质，逐个核过，一个不必改；
  服务器那台走 systemd `Environment=`，也不受影响。逃生门仍在：`project_path` 空、或 `v_ip` 空 / `v_port=0` 时照旧进设置模式。
- **验证**：`cargo test --lib` **2297 passed / 0 failed**（翻默认前后逐个字相同——事前先拿 `AIOS_STORE_MODE=rocksdb`
  跑过一遍全量当预演，同样 2297 绿，确认测试面一处都没押在默认上）。真机三跑（debug 二进制）：
  ①老配置（`store_mode = "rocksdb"` + `in_memory_db = true`）不设环境变量 → 本进程按 **rocksdb** 跑、两条点名照出、
  因两边同档不喊「两边不是同一档」；②配置残留 `store_mode = "mem"` 不设环境变量 → 按 rocksdb 跑并喊「两边不是同一档：
  启动前设 `AIOS_STORE_MODE=mem`」；③`project_path` 有效 + 地址上没库 → `SurrealDB 连接失败（ws://…）` exit 1（这就是新默认
  要的那种当场报错）。`Start-AMS.ps1` 的回填三个分支逐个实跑：不给且环境空 → `mem`；不给但环境已有 `rocksdb` → 沿用；
  给 `-StoreMode rocksdb` → `rocksdb`。向导第②步用 playwright 走真人路径截了图：介质栏显示「由 默认 rocksdb（…）决定」、
  地址两格与「测试连接」可用、橙色提醒点名文件里那句 `store_mode = "mem"` 已不生效。发布包冒烟三腿见下一条。

### 同日收尾：发布包冒烟三腿真跑了一遍——顺手发现 C 腿自 2026-09-15 起一次都没执行过

- **为什么**：ADR-080 那两条改的正是发布包这条路（`Start-AMS.ps1` 回填 `mem`、冒烟各腿只递环境变量），
  唯独「三腿真跑一遍」一直挂着未验。真跑之前先读了一遍冒烟门，发现 C 腿（包内 `surreal.exe` 起一台 rocksdb 库、
  服务以 `AIOS_STORE_MODE=rocksdb` 连它）挂在 `if ($IncludeWorkspace)` 上，而那个开关 **2026-09-15 随 workspace 档一起删了**——
  变量再没被赋过值，判断恒假。后果不是报错而是静默：从那天起每份 `release.json` 里 `rocks_leg` 都是
  `skipped: runtime tier does not bundle a workspace`，而 `$smoked` 里 C 腿那一格写着「不带 workspace 就不算数」，
  于是只过 A / B 两腿的包照样被判为「已冒烟」。连带 `-ReuseWebFromZip` 也一起死了：它要求来源包三腿全过，
  而 09-15 之后出的包没有一个满足得了——0.1.28 那两版只能回头去引 0.1.26。
- **改动**：C 腿改为总是执行，前置条件换成真实的那两条（端口 `9099`/`18010` 空闲、打包机现场那份 `DbOption.toml` 在）——
  它本来就不需要包里带 workspace：库由包内 `surreal.exe` 现起，配置取打包机现场那份，两样都在包外。B 腿补同一道前置
  （此前现场目录缺失时是读文件炸掉整个出包，与脚本开头那句「两腿会因此跳过」的警告对不上）。`$smoked` / `$legFailed`
  纳入 C 腿。README / CHANGELOG 生成处那 7 段按 `$IncludeWorkspace` 分叉的文案、以及把 `workspace\assets` 直灌进 zip 的
  6b 段一并删掉（同样恒走 runtime 那一侧）；`release.json` 的 `workspace_included` / `meshes_included` 落为字面 `false`。
- **验证**：`New-LocalRelease.ps1` 整跑一次（`-ReuseWebFromZip` 引 0.1.26 那份、`-SmokeHttpPort 9499`），
  **三腿全过**：A 出厂配置起服务 = 设置模式 + 向导页；B 本机配置全启动 = health 200 + 首页含 `plant_ui_app.js` +
  `medium=mem` + `durable=false` + 服务没拉 surreal 子进程；**C = 包内 `surreal.exe` 2.1.4 起库 + 服务连上 +
  `medium=rocksdb` + `durable=true` + 数据目录真落了 7 个文件**。**按负例验过**：把 18010 占上再跑，C 报
  `skipped: 18010 被 PID … 占用`，产物随即被标『未冒烟』——改之前同样的局面是 C 一声不吭、包照样算冒烟过。
- **当前交付件**：`ams-local-0.1.28_release.20260918113752.g5d528b5a5379.dirty-runtime.zip`
  （120.96 MB / 126,834,258 B），SHA-256 `90649b8cc962c518cc0ac56d5d629064154125286b3dc5304f8679b76fb5c447`
  （另行独立重算一致），构建自 `5d528b5a5379`（工作树带着别的会话的在途改动，故 `.dirty`），包内 78 个文件，
  Web 仍复用 0.1.26 那份已验证包（plant-ui `0.1.11`）。

### 0.1.28 第三版包：带上重启台账与 `Start-AMS` 两条修复；上一版同号包已删

- **当前交付件**（同 Cargo 版本、新 build_id）：
  `ams-local-0.1.28_release.20260918053140.g2bb1edc4e936.dirty-runtime.zip`（120.93 MB / 126,802,929 B），
  SHA-256 `80e31ec563cb6587f014f80caae95419552eb74cb166455ad976d698b7987cc8`（另行独立重算一致），
  构建自 `2bb1edc4e936`。A / B 两腿冒烟过（口 9499），C 腿按 runtime 档跳过；包内 78 个文件，
  Web 仍复用 0.1.26 那份已验证包（plant-ui `0.1.11`，13 个文件）。
- **验过两条修复真在包里**：zip 解到空目录起在 9599 —— ① 调用方接真管道跑**包里的**
  `Start-AMS.ps1`，**1.3s 拿到 `READY PID=2728 …` 并 EOF**（上一版同一台架 7.3s 还堵着）；
  ② `process.json` 记的就是 2728；③ `POST /setup/restart` 之后服务 2728 → 27512，台账跟着变成
  27512；④ 端口判据回归：9099 occupied / 9599 self / 9699 free，仍走 `listener_table`。
  包内 `Start-AMS.ps1` 已无 `-RedirectStandardOutput`。验完实例与目录都清掉。
- **上一版 `…20260918042801.g695cc85dc8bb…` 已删**（zip / sha256 / manifest 与它的构建日志），
  口径同前一次拍板：同一 Cargo 版本两个包摆一起，拿错的代价大过留着当 patch 基线。
  `releases/` 里 0.1.28 只此一份。
- **仍是 `.dirty`**：六个仓的在途改动照旧编进这只二进制——gen-model-model-cache `src/` 1 文件
  80+/1−（别的会话的 `model_impact.rs`）、e3d-model 6 文件 807+/252−、e3d-io `schema.rs` +14、
  e3d-attlib +4、old-aios-core 41+/6−、plant-mbd `crates/` 11 文件 622+/144−。
- **未验**：rocksdb（durable）真库重启、plant-ui 侧回归、干净树上的重现构建。

### 向导重启后进程台账跟着换；`Start-AMS.ps1` 拉起服务不再把调用方的管道堵死

- **为什么**：上一笔只把「解压即跑」路上撞见的这两处小毛病记下来、没改，这一笔改掉。
  ① 向导 ④→⑤ 之间那次 `/setup/restart` 换了进程，包根 `process.json` 里还写着上一条命的 pid
  （现场记到台账 56460 / 实跑 35152）。`Rollback.ps1` 按 exe 路径匹配，功能上不疼，但那个文件
  是运维手上唯一的台账，它骗人。② `Start-AMS.ps1` 用 `Start-Process -RedirectStandardOutput /
  -RedirectStandardError` 拉服务，这条路子走 `CreateProcess(bInheritHandles=TRUE)`，**服务连调用方的
  stdout 句柄一起继承过去**；调用方（脚本 / CI / 代理）只要接了管道，那一头就永远等不到 EOF——
  服务是长命的。双击 `Start-AMS.cmd` 走控制台句柄不受影响，所以这坑只有自动化会踩，而它每次都踩。
- **改动**：① `spawn_replacement` 拉起替身后回写包根 `process.json`——`pid` / `start_time` /
  `executable` 三键就地改写，别人加的键原样留着；**只改已经在那儿的那一份**（开发态直接跑 exe 的
  目录里没有这个文件，重启不该凭空造一个出来），写不动只警告：端口与单实例锁那时已经交出去了，
  重启不能卡在台账上。② 生成的 `Start-AMS.ps1` 改让 `cmd` 接那两路重定向——
  `Start-Process $env:ComSpec '/c ""<exe>" serve 1>"…" 2>"…""' -WindowStyle Hidden -PassThru`，
  这一侧就只剩 ShellExecuteEx，句柄一个都不带过去；服务自己按可执行路径认回来（同脚本顶上认
  既有实例那一招），`process.json` 与 `READY PID=` 记的仍是服务的 pid，不是 cmd 的。
- **验证**：
  - `cargo test --lib --features http_api -- web_service::setup::` **21 passed / 0 failed**，新钉 2 条：
    三键改写且未知键存活（坏 JSON 也得给得出一份能用的台账）、没有台账的目录不冒出一份来。
  - **句柄那条先量 A/B 再动手**：合成台架（调用方用真管道 + `ReadToEndAsync`）下，`-RedirectStandard*`
    那一形拿不到 EOF；先试「spawn 前摘掉三个标准句柄的可继承位」——**没用**，照堵；不给重定向参数
    （ShellExecuteEx）立刻通。cmd 这一路是这么定下来的，不是猜的。
  - **真包上的 A/B**：交付的 0.1.28 解到空目录、跑在 9599。原装脚本：launcher 1.3s 就退出、服务
    `health=ok`，而调用方的管道 7.3s 仍没 EOF；换上本笔的 exe + 脚本：2.2s 拿到
    `READY PID=20920 URL=http://127.0.0.1:9599/` 且 EOF，两路日志照旧落 `logs\<stamp>.std*.log`。
  - **台账**：同一台 `POST /setup/restart`，服务 20920 → 61984，`process.json` 跟着变成 61984
    （`executable` 与 RFC3339 的 `start_time` 一并更新）。顺手回归端口判据：9099 occupied /
    9599 self / 9699 free，仍走 `listener_table`。验完试跑实例与临时目录都已清掉。
- **已重出一版包带上**：见上一节的 `…20260918053140.g2bb1edc4e936…zip`；本笔之前那版
  `…20260918042801.g695cc85dc8bb…`（没有这两条）已删。

### 配置向导第③步的端口判据改查监听表——绑 `127.0.0.1` 试探在 Windows 上会把占着的端口报成「可用」

- **为什么**：0.1.28 出包后按「解压即跑」走了一遍首跑向导，撞见两道门对同一个端口给出相反的答案。
  9099 上实实在在有一台服务在听（`0.0.0.0:9099`，`/health` 能答），`Start-AMS.ps1` 用
  `GetActiveTcpListeners()` 一查就拒绝启动；而向导的 `probe/port` 回的是 `{"verdict":"free"}`。
  根因在 `setup.rs::probe_port` 的判据——注释写着「目标就算是 0.0.0.0，本机回环那一份也必须是空的」，
  **这条在 Windows 上不成立**：通配监听套接字不设 `SO_EXCLUSIVEADDRUSE`，别人持着 `0.0.0.0:P` 时
  再绑 `127.0.0.1:P` 照样成功（当场验过）。现场按向导挑了一个已被占的口，要到启动那一下才知道。
- **改动**：新增 `listening_ports()`——Windows 上走 `iphlpapi` 的 `GetTcpTable2` / `GetTcp6Table2`
  取 `MIB_TCP_STATE_LISTEN` 的端口集合（与 `Start-AMS.ps1` 同一张表、同一口径：只看端口、不问绑在
  哪个地址），非 Windows 回 `None`。判据抽成纯函数 `port_probe_verdict(bound_port, addr, listeners)`：
  自己绑着的口先判 `self`，其次查表，表问不出来才退回原来的绑定试探（POSIX 上通配与回环互斥，
  那条在那边本来就是对的）。回执多一格 `source` = `listener_table` / `bind_probe`，说清是哪一路给的。
  `Cargo.toml` 加 `[target.'cfg(windows)'.dependencies] windows-sys`（0.59，已在 lock 里）。
- **验证**：`cargo test --lib --features http_api -- web_service::setup::` **19 passed / 0 failed**，
  新钉 3 条——真 socket 的 `a_wildcard_listener_makes_the_port_occupied_even_when_loopback_still_binds`
  （自己开一个通配监听，断言判成 occupied，失败信息里带上「回环还绑得上」这个反例）、纯判据的
  `the_port_verdict_reads_the_listener_table_and_answers_self_first`、Windows 上
  `the_listener_table_is_readable_and_contains_a_port_we_just_opened`。**变异一次**：把监听表当作
  读不出来（退回旧判据），第一条立刻红在 `{"verdict":"free","source":"bind_probe"}`，撤回复绿。
  **live**（换上带本修的二进制、`_runs\ams-0.1.28-oob` 那台 0.1.28 在 9399 上）：`0.0.0.0:9099` 与
  `127.0.0.1:9099` 都回 occupied、`0.0.0.0:9399` 回 self、`0.0.0.0:9499` 回 free、
  `127.0.0.1` 上的 surreal `8009` 也认出来了，四个口与 `GetActiveTcpListeners()` 逐个一致。
- **不在 0.1.28 的包里**：`ams-local-0.1.28_release.20260918033726…zip` 是本修之前打的，要带上得重出一版。
- 顺带记下同一轮看到的两处小毛病（本笔未改）：① 向导 `/setup/restart` spawn 替身时不回写
  `process.json`，重启后那个文件里还是旧 pid（`Rollback.ps1` 按 exe 路径匹配，功能上不疼）；
  ② `Start-AMS.ps1` 真去拉起后端时，`Start-Process` 出来的服务继承调用方的 stdout 句柄——调用方若
  接了管道或重定向到文件，那一头永远等不到 EOF（双击 `Start-AMS.cmd` 走控制台句柄，不受影响）。

### 本地发布 0.1.28：房间归属三笔改写 + 空间净距新端点；`.dirty` 构建（六个仓的在途改动一并编进）

- **当前交付的是第二版包**（同一 Cargo 版本、不同 build_id）：
  `ams-local-0.1.28_release.20260918042801.g695cc85dc8bb.dirty-runtime.zip`（120.92 MB / 126,793,564 B），
  SHA-256 `b9dc094bd2fe1a3d24c4dc86536a062283925740852624010a8124cab66f17f3`，构建自 `695cc85dc8bb`
  （多带上面那条向导端口判据修复），A / B 两腿冒烟过（口 9499）、C 腿按 runtime 档跳过。
  **验过修复真在包里**：把这份 zip 解到空目录起在 9599，问它 `0.0.0.0:9099` / `0.0.0.0:9399`（两个口
  当时都真有监听）都回 `occupied  source=listener_table`，自己那个口回 `self`，空口回 `free`。
  本修之前还打过一版 `…20260918033726.g81d2c28c04cb…`（那一版的向导会把占着的端口报成可用），
  用户 2026-09-18 拍板**删掉**——同一 Cargo 版本两个包摆在一起，拿错的代价大过留着当 patch 基线的
  那点好处。`releases/` 里 0.1.28 只此一份。
- 后端 `0.1.28` 构建自 `81d2c28c04cb`（`chore(release): 0.1.28`），**工作树不干净**，build_id 带 `.dirty`。
  复用 0.1.26 那份已验证 Web（plant-ui `0.1.11`，`web_source = reused-from:ams-local-0.1.26…-runtime-workspace.zip`，13 个文件）；
  前端本轮没有发版，plant-ui 工作树里 31 个在途文件**没有**进包。
- 产物 `releases/ams-local-0.1.28_release.20260918033726.g81d2c28c04cb.dirty-runtime.zip`（120.93 MB / 126,803,926 B），
  SHA-256 `455b8c411ccb96cd83e7710c4316f588d0655b00abbaf1a5c25f1f13756d7e41`（另行独立重算一致）；包内
  `aios-database.exe` 自报 `0.1.28+release.20260918033726.g81d2c28c04cb.dirty`（打包脚本对着 EXE 校过版本与构建号）。
- 包型 `runtime`（同 0.1.27）：包根 `aios-database.exe` / `web\`（13）/ `bin\surreal.exe`（2.1.4+20250317.45013fc9）/
  `resource\surreal\*.surql`（7）/ `schema\{e3d210,e3d31}`（48）/ 出厂 `DbOption.toml`（设置模式）/ `.first-run-pending` /
  `Start-AMS` / `Start-Surreal` / `Rollback`，共 78 个文件。
- 覆盖 0.1.27（`2cb5de1f112c`）之后的 **73 个提交**：房间归属改在 bake 时算、边随根回执走（ADR-073 D1 / D12）；面板变化的
  反向传播与跨库停放（D13 的 D / U3 b）；点包含换成三方向射线奇偶、边数只减不增（ADR-010 §3）；新端点
  `GET /spatial/surface-clearance`、`GET /spatial/centerline`、`GET /spatial/nearest-clearance`、`nearby` 的
  `source_mode=bran_centerline`、`POST /element/ptset`、`POST /element/plines`；元数据门第二档开关退役
  （`34ae04e00`，行为逐位不变、残留配置启动告警）；刚体前移补 P23 / 属主消费几何 / EVALAT 规则 M 三条排除；
  校审三处修复；隐式管身重发布不再被首次发布的 aabb / trans 钉死（`7d67ca5f8`）。用户口径见 `docs/release-notes/0.1.28.md`。
- **`.dirty` 的确切内容**（都真编进了这只二进制，作者与验证不在本次发布账上）：
  - gen-model-model-cache `src/`：`data_interface/model_impact.rs` 与 `mbd/branch_query.rs`，2 文件 107+/6−；
  - vendor/e3d-model（`5d92b9759eaf` 之上）`src/` 7 文件 892+/260−，含 `route.rs` 的容器两端折属主链；
  - vendor/e3d-io（`97798ee34877` 之上）`src/schema.rs` +14；
  - vendor/e3d-attlib `src/lib.rs` +4；vendor/old-aios-core `Cargo.toml` + `src/shape/pdms_shape.rs`（rkyv `validation`，
    0.1.26 / 0.1.27 也带着）；vendor/plant-mbd `crates/` 11 文件 622+/144−。
  - 其中「头 / 尾隐式管身按属主系读坐标」那条修复（`route.rs` + `branch_query.rs`）已写进 0.1.28 发布说明，
    而**它的源码尚未提交**——撤掉那两处改动就要同步撤那一条说明。
- 冒烟（口 9199；9099 被 `_runs\ams-0.1.27` 的服务占着）：A 出厂配置起服务 = 设置模式 + 向导页 → **过**；
  B 本机配置全启动 `medium=mem` / `durable=false` / 服务没拉 surreal 子进程 → **过**；C rocksdb 腿按 runtime 档设计跳过
  （不带 workspace）。**未做**：rocksdb（durable）真库重启验证、plant-ui 侧回归、本包在干净树上的重现构建。

### 房间归属的点包含换成射线奇偶：parry `contains_point` 在退化盖面三角上把盒外点判成成员，且结论随浮点末位翻（ADR-010 §3 追记，一笔）

- **为什么**：ams 1112 + 8000 从零初始化的 live 复核（`D:\adr078-verify\live-run-1112-8000\evidence\`）里，边集与 09-16 基线差
  1112 −15/+5、8000 −14/+22，而两座库的构件行、六块面板行、`.mesh` 文件逐字相同、`room_predicate.rs` 两版零 diff——差的只能是谓词在
  同一份几何上给了两个答案。拿库上真实行重放今天的谓词 + 独立 f64 射线奇偶复核：`17496/122987` 在 R320 足迹外 1.1 m（今天判 0/8 对、
  09-16 判 8/8 错）；今天多出来的 `128587` / `129717`（R330）、`141798`（R311）、`106302`、`140998` 也全在房间外；两版都认的
  `17496/72903 ∈ R320`（8/8）盒角 x = −1183 在面板世界盒 x_min = 0.03 之外。根因在 parry（gitee fork rev `06fd686`）
  `point_triangle.rs`：点落在**零面积三角**（`va+vb+vc == 0`）的面 Voronoi 区时滑进给 2D 留的 `if solid { OnSolid, is_inside = true }`，
  `point_composite_shape.rs` 的 best-first 访问器再 `if solid && is_inside { ExitEarly }`——房间棱柱的盖面是两百多个船体型线点的
  三角剖分，共线处就有零面积碎片，哪些碎片经 f32 变换 / 焊接后恰好零面积决定了结论。本地盒内 2700 点网格对照：R330 假阳性
  221/2700（8%）、R311 46、R320 盒内 0 但盒外中招；只焊接不剔退化不够（焊后 `122987` 又回 8/8）。
- **改动**：`room_predicate::panel_tri_mesh`（面板 `.mesh` → 判定用三角网的唯一口径：顶点按 0.01 mm 焊接、剔顶点重合 / 三点共线的三角、
  乘世界矩阵建 BVH，不带 `ORIENTED`），常驻 `room_resident::build_tri_mesh` 与库行 `PanelMeshes::from_rows` 都经它；
  `room_predicate::point_inside_panel`（三条歪斜方向各数一次穿越、奇数为内、三取二；BVH 取候选、f64 Möller–Trumbore，行列式相对边长为零
  不计），`count_vertices_inside` / `any_point_inside` 改调它。§3 口径（8 顶点 / 逐点兜底）与 `element_in_panel` 形状不变。
- **验证**：`room_predicate` 8 条绿，新钉 3 条——`a_degenerate_cap_triangle_cannot_turn_an_outside_point_into_a_member`（盒 + 共线碎片 +
  只差 1e-4 mm 的角点副本：剔成 12 片 / 焊成 8 点、盒外点仍在外）、`welding_keeps_first_seen_coordinates_and_drops_nothing_from_a_clean_box`、
  **真网格** `a_real_room_prism_never_admits_points_outside_its_own_box`（夹具 `tests/fixtures/room_panel_24381_34950_R320.mesh` = R320 那块面板
  68,772 B + 库上世界变换：`72903` / `122987` 两个盒 0/8、六个面外各 1 m 的探测点全在外、最大盖面三角重心落到半高在内；同一夹具走旧路
  `ORIENTED | contains_point` 的假阳性也钉着，parry 修了先红）。定向 `cargo test --lib -- room_predicate room_model room_resident` 84 绿、
  `room_reverse room_recompute batch_worker model_db_adapter::a_/recomputing` 106 绿。**live**（:9198 换新二进制 `POST /setup/restart`，
  同一座 :8050 库 `POST model/rebuild`）：1112 6161 → **5858** 条（少 303 对 (构件, 面板)、110 对 `inside_count` 下调、**0 对新增**），
  8000 4379 → **4284**（少 95 对、12 对下调）；少掉的集中在 R330（76 + 78）、R362、R342 / R343 / R345、R320；钉过的几个构件全部按预期
  （`122987` 只剩 R346、`72903` 只剩 R101 / R144、`128587` / `129717` 只剩 R445、`141798` 只剩 R221）。同进程再重算与冷重启后重算
  边 dump **逐字相同**（1112 `E7AC8737…` / 8000 `3A72E777…`）。重算耗时不变（8000 21 s、1112 12 s）。
- **代价 / 要跟的**：① `inside_count` 数值口径变了（此前含伪法线误计）；② **存量 `room_relate` 不会自己重算**——常驻代号只摘要拓扑与面板几何、
  不含谓词版本，要各 DESI 一次 `POST /dbnums/{dbnum}/model/rebuild`；③ 「横穿凹形房间的长管八角全在外」那条已知盲区照旧（本轮不改口径）。

## 2026-09-17

### 反向传播的跨库停放：命中的别的库排成根级工作项 `RoomRoots`，由那个库的下一个任务沿准入链消费（ADR-073 D13 追记 U3 b，五笔）

- **为什么**：D 五笔落完，反查里别的库的命中只**数**（`cross_library`），一窗内两个库都有任务时只有先开工的库走定向，后开工的库
  `Unchanged` 却欠着 → 整库（③ 电平型兜底）——面板在 A 库、成员在 B 库是常态，B 每次都整库就把定向省下的又花回去。用户 2026-09-17
  拍板 U3 b 拆单（停放槽先进程内、一项一笔）。
- **改动（按提交）**：
  1. `6cf5f1d61` 反查放开到全 MDB：库上 `tubi_relate` 范围查询不再带 `dbnum =`，投影管身一把读锁扫全域（新
     `ModelMemoryStore::for_each_record`，不复制）；命中按 `RefNo::dbno()` 分桶——本库的折根，别的库的进
     `AffectedRoots::foreign = dbnum → (元素 → 祖先链)` 存元素不折根、祖先链在这里就带上（管身来源 BRAN 在 `inst_relate` 里没有行，
     那个库回库取不到；拆单写的「B 的任务补 `anc`」落地时改成停放方带上），`cross_library` = 其总数。
  2. `a894eb1be` 停放槽 `room_reverse::RoomRoots { from_generation, to_generation, elements, parked_by, parked_at }`，进程内
     `park`（同段并条、空条不停）/ `pending` / `clear` / `parked_health`；A 库定向重算**结论定为 `Targeted` 之后** `park_foreign_hits`
     把每一桶连同这一段换代（上一代 → 重算那台的代号）停进那个库的槽；`RoomRecomputeScope::Targeted { swap: Option<PanelSwap>, parked }`
     定下最终形状；`/health.room_build.parked` 一格。
  3. `a73b40e12` 准入链：`scope_for_task(refresh, library, resident, parked)` 从库上那一代出发沿停放物 `from → to` 逐段接、最后一段
     可以是本任务自己的换代（自己那一段优先、库上已越过的一段不要、链断 / 绕圈 / 从没盖过 / 常驻说不出代号退整库），今天的四条准入
     是链长 1 的特例；两个执行体 `durable_alignment` 之后 `parked_for_task` 取槽（库上已同代顺手清槽）；`request_for_task` 接了停放链
     多一句原因。
  4. `a4ad36cd3` 消费：`run_targeted_room_recompute` 把停放的元素沿带来的祖先链折成本库的根（`fold_parked`，不回库、同一元素几段都有
     只折一次），与自己那一段反查命中的根并起来 `RootScope::Targeted`（本任务没换代就只有停放那一路、没有面板级语句）；折不出根退整库；
     `run_room_recompute` 按结论清槽（`WholeLibrary` / `Targeted` 才清）。
  5. 本笔：ADR-073 D13 追记 U3 b、钉 23、§未决 U3 跨库那一半关（算法输入形态那一半仍开）；CONTEXT「整库房间重算」词条加跨库停放；changelog。
- **不动的**：③ 电平型触发原样——停放槽进程内、重启即丢，丢了是多算不漏算；`run_room_only_recompute` 一字不改；今天的四条准入照旧。
  **仍欠**：没命中的库看不到「这一段与你无关」，`Unchanged` 且槽空 → 仍整库一次（给全 MDB 停放空段要知道库的全集，本轮没做）。
- **验证**：每笔定向 `cargo test --lib` 绿（U3-1 room_reverse 5 / U3-2 room_reverse+room_recompute+batch_worker 65 / U3-3 66 / U3-4 90 含
  adapter）。新钉 5 条：`the_durable_tube_query_is_not_filtered_by_library`、`the_parking_slot_merges_the_same_segment_and_clears_per_library`、
  `the_admission_walks_the_parked_chain_from_the_library_generation_to_the_resident`（链准入真值表 14 种局面）、
  `parked_elements_fold_to_the_library_roots_on_their_carried_ancestor_chains`、`mem://` 两库端到端
  `a_panel_move_parks_the_other_librarys_hits_and_its_next_task_recomputes_only_those_roots`（A 库面板搬家 → 只 A1 重写、B1 停放 → B 的任务
  接链只重写 B1、B2 的边与 ACK 一字不动、与整库同终态、清槽；槽空 / 链断退整库）；`resolve_joins_…` 扩三个库、
  `the_targeted_recompute_falls_back_…` / `the_room_only_recompute_is_wired_…` 扩停放 / 清槽 / 取槽的位置。**变异一次**：`resolve` 不分库
  → 端到端与 `resolve_joins_…` 同时红，撤回复绿。全量 `cargo test --lib` **2263 passed / 5 failed / 119 ignored**——五条红与 D 收口时
  同一批（`model_impact` ×2、`window_root_plan` ×2、`review::platform` ×1，都不在本轮碰过的文件里）。**未 live**：两个 DESI 库都有任务的
  一窗里挪一块跨库面板（见 ADR-073 钉 23 的 live 待补）；Q6 管身范围查询放到全 MDB 的解引用成本要 live 量。

### 房间归属的反向传播：面板变了拿新旧盒反查成员、只重算命中的根，整库重算降为兜底（ADR-073 D13 追记 D，五笔）

- **为什么**：逐根路径（D1）只答「本根在哪些面板里」；面板那一侧变了（PANE 几何 / 属主、FRMW / SBFR 改名、CWALL / CFLOOR 搬迁），
  它名下**别的根**的边陈旧，此前唯一的填法是 D13 整库重算——O(本库全部根) 的回读几何 + 点检查，而真正陈旧的只有与那块面板新旧盒
  相交的几根。用户 2026-09-17 拍板（grill 两轮，五题全按推荐）：拆三类 / 按根 / 全局树 + 不可用退整库 / 先同库、目标跨库工作项 /
  定向成功即盖 `Aligned`、③ 保留整库。拆单时多出一题 **Q6：隐式管身不在空间树上**（`projected_tree_entry` 对 `tube` 回 `None`，直管穿房
  两端都不在房里是常态、只查树会漏）→ 按默认「库上 `tubi_relate` 范围查询」办，不动树的契约。
- **改动（按提交）**：
  1. `41d9e10b7` 面板变化结构化：`room_resident::PanelDelta { removed, renumbered, recalc: [(面板, 旧盒?, 新盒?)], geometry_changed }`，
     `ResidentRooms::install()` 对着被换下的那一代与 `TopologyDiff` 同一次算——`recalc` = 拓扑差点到的 ∪ **两边都在册但 `world_aabb` /
     实例集变了的**（PANE 只动几何时拓扑差是空的）；`PanelBounds` 按位 `Eq`；随 `InstallOutcome.panels` → `StartupInitOutcome.panels` →
     `RoomReferenceRefresh::Rebuilt { panels, previous_generation }`（上一代的计算摘要，准入要对它）。
  2. `94c02b346` 反查 `data_interface::room_reverse`：`affected_roots` 先 `ensure_spatial_ready()`（不过 → `TreeUnavailable`），树读锁只罩
     取候选（`tree_element_hits`，剔 `PANE`、跨盒去重）；`resolve` 合三路——树上元素、投影里本库管身按盒扫、库上 `tubi_relate`
     `aabb.d` 范围过滤（`durable_tube_hits`，闭区间逐轴谓词）——按 `RefNo::dbno()` 分库（跨库只数 `cross_library`）、树上不在投影里的
     元素按记录 id 批量取 `anc`（`durable_ancestors`）、`fold_to_roots` 与本库 `DbSet@T` 根清单求交（嵌套根两根都算），折不出根的进
     `unresolved`。不写树。
  3. `d32fdc246` 处置：`recompute_dbnum_room_edges(…, library_roots, RootScope)`——`All` 与今天逐字同形；`Targeted { roots, panel_statements }`
     投影里的按集合筛、回读清单 = 集合 ∖ 投影，`render_panel_delta_statements`（不在册：`DELETE {panel}->room_relate` + 清
     `room_panel_relate` 入边，`room_model::render_unregistered_panel_statements`；只改号：`render_renumber_statements`）作
     `pack_publication_transactions` 的 prelude 进第一笔；`RoomEdgeRecompute.panel_statements`。
  4. `fa97f8758` 分派与盖章：`room_recompute::durable_alignment`（库上那一格 + 常驻代号，一次读；③ 与准入共用）、
     `RoomRecomputeRequest { reason, scope: WholeLibrary | Targeted(PanelDelta) }`、`scope_for_task` 四条准入（真换了代 + 面板变化非空 +
     说得出上一代 + 库上正是上一代）；收尾段 `run_room_recompute` 分派——定向 `run_targeted_room_recompute`（根清单 → 反查 → 定向重算），
     `TreeUnavailable` / `unresolved` 非空 / 根清单枚举不出来 / 覆盖面对不上 → `None` 退 `run_room_only_recompute`；成功
     `RoomRecomputeVerdict::Targeted(重算那台计算器的代号)` → `LibraryRoomClaim::Recomputed` → `Aligned`；算不出的根两路同走
     `settle_room_recompute_failures`。`DataBatchTaskResult.room_recompute` 改带 `RoomRecomputeRequest`；`PersistTarget::db()` 访问器。
  5. 本笔：ADR-073 D13 追记 D、钉 17 更名 / 钉 22、§未决 U1 库内部分关 / U3 记 a 档；CONTEXT「整库房间重算」词条；changelog。
- **不动的**：③ 电平型触发（重启后 / 从没盖过那一格只整库一次）；`run_room_only_recompute` 一字不改；D1 钉「元素分支不依赖空间树」
  只钉正向；树的写点白名单三处不变。**代价**：一窗内两个库都有任务时只有先开工的库走定向（库上代号是上一代），后开工的库
  `Unchanged` 却欠着 → 整库——U3 b 档才收。
- **验证**：每笔定向 `cargo test --lib` 绿（D1 room_resident 40 / D2 room_reverse 4 / D3 adapter+room 135 / D4 room_recompute+batch_worker 62）；
  新钉：`install_reports_which_panels_moved_and_where_their_boxes_are`、`hits_fold_to_every_library_root_on_their_ancestor_chain`、
  `tree_candidates_exclude_panels_and_deduplicate_across_boxes`、`mem://` `resolve_joins_tree_projection_and_durable_tubes_into_library_roots`、
  `the_box_predicate_is_a_closed_interval_test_per_axis`、`panel_level_statements_clear_unregistered_panels_and_renumber_rooms_only`、
  `mem://` `a_targeted_recompute_rewrites_only_the_named_roots_and_runs_the_panel_statements_first`、
  `a_task_goes_targeted_only_when_this_swap_is_the_librarys_whole_debt`、
  `the_targeted_recompute_falls_back_to_the_whole_library_when_the_reverse_lookup_cannot_answer`。全量 `cargo test --lib`
  **2257 passed / 5 failed / 118 ignored**——五条红与 B/T6 收口时同一批（`model_impact` ×2、`window_root_plan` ×2、`review::platform` ×1，
  都不在本轮碰过的文件里）。**未 live**：ams 1112 挪一块 PANE 的那一窗（见 ADR-073 钉 22 的 live 待补）。

### 标准隐式管身的房间判定点不再多乘一次本地摆放（ADR-073 D1 追记 B 的一处修正）

- **为什么**：`persisted_world_transform` 对隐式管身把标准图元的本地摆放折进存下的世界变换（Plant UI 只拿这一个变换摆标准网格），
  而投影记录的 `ProjectedGeometry::mesh_to_world()` 与 bake 侧 `baked_room_input` 对 `Canonical` 一律再乘 `local_transform`——
  标准管身就成了 `world × local × local`：跨面板边、真走点检查的那段管身，判定点落在画出来的网格之外；库上 `tubi_relate` 行回读
  （实例变换单位阵，`durable_room_inputs`）算的是 `world × local`，整库重算 / 从零初始化与逐根路径对同一段管身可能不逐边相等
  （ADR-010 §9 硬验收）。B/T6 收口时顺手看见、单列一笔。
- **改动**：`room_model::projected_mesh_to_world(world, canonical_local, implied_tube)` 多一格——隐式管身不乘本地；
  `ProjectedGeometry::mesh_to_world()` 传 `tube.is_some()`，`baked_room_input` 传 `matches!(geometry_id, ImpliedTube)`；测试助手
  `calculate_with_component_bvh` 改调 `record.mesh_to_world()`（不再自己复述公式）。
- **验证**：`bake_time_room_input_matches_the_projected_record` 加一句「三种形状本地摆放都只乘一次」；新钉
  `model_record_reader::a_canonical_implied_tube_has_the_same_mesh_to_world_on_the_projected_and_durable_paths`（投影记录 vs
  `projection_record_to_query` → `durable_room_inputs` 回读，标准图元元素与标准管身两例，f32 精度内相同）。

### 房间归属在 bake 时算、边随根回执走、flush 只渲染；一任务一台房间计算器（ADR-073 D1 / D12 追记 B，六笔）

- **为什么**：ADR-073 D1「模型计算之后用候选面板做点包含筛选」此前落在 write-behind flush 里：每次 flush 冻一台计算器，
  拿投影记录**回读盘上 `.mesh`** 取判定点（`memory_model_world_points`），而生成 worker 在 `bake_element` 里刚把同一份网格
  算出来又丢掉；计算器按 flush 冻，任务中途常驻换代（`/model/ensure` 并发对齐）会让同一任务各批根的边不是同一代算的
  （P2-3 为此打过补丁）。用户 2026-09-17 拍板（grill 两轮）：时点提早到生成 worker、省掉回读，边随凭证同事务，
  D2 失败前移，`RootReceipt` 带边，只覆盖批次任务的整根重生成、其余入口留 flush 兜底。
- **改动（按提交）**：
  1. `bd9957bfd` 对等用例：`calculate_edges` 拆成 `edges_for_record`（逐记录）+ `merge_root_edges`（同 (构件, 面板) 取最强、按键排序），
     `memory_model_world_points` 拆成读盘 + `world_points_of`（f32 变换、按三角遍历去重）——**取点全仓只有这一处定义**；
     用例 `component_points_from_the_in_memory_mesh_equal_the_disk_read_back`：内存网格取点 vs 盘上回读逐位相等，逐元素算边收拢后
     与 `calculate_for_root` 逐字相同。
  2. `c953c6cc9` 一任务一台：`model_db_adapter::TaskRoomCalculator { Disabled, Frozen(Arc), Unavailable(原因) }::freeze()`，两个执行体在
     D12 对齐之后冻一次、传 `&rooms` 给 `run_unit_worklist`；`Unavailable` 时单元入口按 D2 把全部单元记失败（`room calculator unavailable
     for this task`，与此前 flush 同一句）、不白跑几何；`generate_roots_report` / `generate_roots_with_rooms` / `generate_unit_model_with_rooms`
     带 `rooms` 下传，process-store 形态优先用任务那一台（`TaskOrGroupRooms`）。
  3. `9520d7321` bake 时算边：`rooms` 沿 `generate_and_persist_roots → produce_slices → spawn_generation_worker → bake_snapshot → bake_elements →
     bake_element` 显式传递；`bake_element` 用 `room_model::BakedRoomInput`（构件 = `geometry_source_refno`、标签 = 投影记录 id、世界变换 =
     `persisted_world_transform`、「网格 → 世界」= `projected_mesh_to_world`——与 `project_geometry_delta` 折投影记录**同一份配方**）调
     `edges_for_record`，判定点经 `world_points_of(&mesh, …)`，取的是本地 / 标准图元那份**就要写盘的网格**，不是 manifold 在 f64 里折出来的
     `world_mesh`（差几个 ulp 会让贴面板边的点翻面，破 ADR-010 §9）；`BakedElement.room_edges`。单元级局部发布 / 初始化 lane / 读透即时生成 /
     路由轮询给 `None`。
  4. `956a3991c` 边随回执：`RootReceipt.room: Option<CarriedRoomEdges { generation, elements: Vec<String>, edges }>`（`#[serde(default)]`，旧
     `model_artifact` 读回 `None`；`RefnoEnum` 无 serde `Deserialize`，按字串存）；`bake_snapshot` 用那台计算器的代号 + 这一根**全部**构件
     （有边没边都在册）+ `merge_root_edges` 装 `BakedSnapshot.room`，`persist_memory_batch` 挂到回执；刚体前移 / 单元级发布 `room: None`。
     40 处 `RootReceipt` 字面量补 `room: None`。
  5. `f38aca8b7` flush 三道闸：`carried_edges_for`（纯函数）——代号 == 此刻计算器代号、构件集 == 本根投影记录的构件集（少一个多一个都不行：
     渲染对每个构件先删后插，拿只覆盖部分构件的边去渲会把没动的构件的边删掉而不补；嵌套根共享记录、局部发布后的整根形状在此挡下）、
     计算器在 → `room_stage` 只 `render_root_room_edges`（`Carried`），否则 `calculate_for_root` 回读现算（`Recomputed`）；
     `BatchedPersist.room_carried / room_recomputed`，`[model-persist]` 与 `[model-write-behind]` 日志带上，稳态下 `recomputed` 应为 0。
  6. `d02ab8929` D2 前移 + 端到端：bake 失败文案「room membership of {投影记录 id} could not be computed: …」（与 flush 同族、带
     「room membership」），`produce_slices` 消费侧 bake `Err` 本就成死信不发布（补钉）；真 cube manifold + 1000³ 面板：完全在内 / 跨边
     （真走点检查）/ 盒不交三件，**bake 的边 == 同一件折成投影记录后 flush 路回读算的边**；面板网格缺 → `Err`。
- **不动的**：D13 `RoomOnly` 整库重算、`model/rebuild`、`/model/ensure`、初始化双 lane、刚体路、单元路——全走 flush 兜底，同一台渲染器，
  硬验收不多一种写点形状。ADR-073 D1 / D12 追记、钉 21；CONTEXT「逐根房间归属」「任务开工前对齐」词条。
- **验证**：每笔各自 `cargo test --lib` 定向绿（room_model 42 / rooms 相关 145 / T1 300 / T2 165 / T3 144 / T4 236 / T5 e3d 48）；
  六笔之后全量 `cargo test --lib` **2236 passed / 5 failed / 118 ignored**——5 条失败都不在本轮触碰的文件里：
  `model_impact::tests::the_metadata_tier_is_curated_only_when_every_name_is_on_the_old_list` /
  `the_skip_verdict_uses_the_same_double_vote_as_the_window_metadata_gate`（在 `c8d5f6b92`——本轮第一笔之前——的临时 worktree 上复跑
  同样红，既有）、`window_root_plan::tests::a_change_is_metadata_only_when_every_attribute_is_data_only_and_nothing_structural_moved` /
  `the_element_tier_is_the_widest_tier_any_of_its_attributes_needed`（同一元数据门家族）、
  `web_service::review::platform::tests::test_verify_active_skips_annotation_gate_even_with_open`（另一路会话正在改的 review 区）。
  rustfmt 各文件本轮新增行无 diff（`e3d_model_service.rs` 11 / `model_db_adapter.rs` 2 / `batch_worker.rs` 6 / `model_refresh.rs` 4 /
  `room_resident.rs` 3 处漂移为既有）。**未 live**：ams 1112 增量窗 `room_carried=N room_recomputed=0` 与「增量 == 全量逐边」
  硬验收脚本没跑。
- **顺手看见、没动**（要另开卡）：`ProjectedGeometry::mesh_to_world` 对**隐式管身**是 `world_transform × local_transform`，而
  `persisted_world_transform` 对隐式管身已把 `local` 折进 `world_transform`——点检查对跨面板边的隐式管身可能多乘一次本地摆放；
  库上回读那条路（`durable_room_inputs`，管身 local = 单位阵）没有这一乘，两条路对同一根管身可能不逐字相等。本轮 bake 侧按投影
  记录同一条公式算（与 flush-from-projection 对等），没改语义。

### ADR-066 D1 规则 2 · 账本去向 `no_model` 的候选不再把根踢回 regen——「挪 EQUI + 子元素改个名」走得了 transform

- **为什么**：`collect_rigid_motion_sources` 把账本每一条都折成候选，不是锚点的一律 `Other`；`rigid_motion::classify_one`
  （D1 规则 2，探针注释里叫「规则 4」）在根的子树 / 祖先链上碰到一个 `Other` 就整根 regen。可 e3d-model 是先 `ledger.record`
  再进元数据门：两票门放行的 `no_model` 候选（子元素只改 `NAME` / `DESC` 那类）照样在账本里，账本也记了去向
  （`Disposition::NoModel`），gen-model 一处没读。结果「挪 EQUI + 子元素改个名」两件事单独发生分别是 transform 与 skip，
  落在同一窗却整根 regen——真实编辑几乎必带这种候选，便宜路基本打不着。
- **改动**：e3d-model `ChangeLedger::dispositions()` 新访问器（原样给出 `(refno, Disposition)` 表，守恒仍由 `accounts_for` 判）；
  `rigid_motion::CandidateKind` 加 `NoModel(String)`，`classify_one` 对它既不收锚点也不报错、只数——规则 2 从此只数去向
  `RolledUp` 的候选；根里**只有** `no_model` 候选（没有一个 Δ）照旧 regen，原因「touched only by N no_model candidate(s)」
  （零锚点的 transform = 只推回执不动记录，是选根的另一道题，这里不开）。`window_root_plan::no_model_candidates(&ledger)`
  取只登记了 `no_model` 的 refno（同时登记了别的去向的不算，宁可挡路），`collect_rigid_motion_sources` 的非锚点候选按它分
  `NoModel` / `Other`；锚点先于去向——Δ 是真的，乘上去永远不错。两处探针 `match rigid_anchor(...)` 补 `NoModel` 臂（与 `Other`
  同判）。ADR-066 状态行 + D1 规则 2 末注记下这条修订与三条边界：`Unresolved` 照旧挡路（那时窗口本就退化）；只 `CACHID`
  跳动的子元素今天**不是** `no_model`（§8 Q6 未落地），仍挡路——ZONE / STRU / 带 SUBE 的 EQUI 挪位要等 Q6 才打得着这条路。
- **验证**：新单测 `a_no_model_candidate_under_the_root_does_not_kick_it_back_to_regen`（锚点在根之上 / 锚点即根两种形状同窗、
  锚点集不混入 no_model、`Other` 对照仍只踢自己那根）、`a_root_touched_only_by_no_model_candidates_stays_regen`、
  `no_model_candidates_follow_the_ledger_dispositions` 过；`cargo test --lib -- rigid_motion::` 25 过、`window_root_plan::`
  33 过 10 ignored（探针）；rustfmt 本轮新增行无差异（文件里剩的三处差异都不是本轮的）；CRLF 未翻（e3d-model `ledger.rs`
  本就是 LF）。本轮与 codex/model-projection-cache 的合并撞在一起：改动先被另一路会话按 wip 落进 8eabda89a、随 56e1b47ea
  合入，单测在 8eabda89a 的独立 worktree 上先跑过一遍、合并落地后在共享工作树复跑。
- **量了一下还剩什么挡路**（新探针 `probe_transform_classification_for_window`，`#[ignore]`：钉一窗、`AIOS_MODEL_TRANSFORM_CHEAP_PATH=1`
  跑生产同款 `collect → impact → plan_window_roots → rigid_motion`，候选表按类 + 每根判决打出来，只读文件、不下断言）：
  - aps250110 **32→33**（STRFLT `BY E 300`，窗 C）：16 候选 = 5 rolled_up + 11 **no_model**（10 个 TREAD 只 `CACHID`、MNUM `MAPEXS`）
    ——这 11 个改前全是 `Other`、改后不挡路；剩下挡路的：STRFLT 锚点本身按 P23 排除（设计如此，6 根全因它 rejected），以及
    **4 个只 `CACHID` 的 GENSEC / HANDRA**（模型单元，rolled_up，Q6 那一档）。所以窗 C 上规则 2 只是把路让出一半，要打通得 Q6。
  - ams8000 **195→196**（EQUI 位姿）：1 候选 = 合格锚点，**transform=1 rejected=0**——生产同款分桶在自然 EQUI 挪位上真走了便宜路。
  - ams8000 **267→268**（BRAN `BY U 2000`）：BRAN 自己 `CACHID / HPOS / TPOS`、FTUB `POS / SPAMAP` 都是 `Other`，BEND 在路由容器内被排除
    → regen，与 D1 规则 3 一致。
- **未做**：Q6（只 `CACHID` → `no_model` 第三档）没动——上面窗 C 的 4 件就是它的账；ADR-066 验收 2 live #2 / 验收 3 量没跑；开关
  `model_transform_cheap_path` 默认仍关。e3d-model 的 `dispositions()` 访问器只在 vendor 工作树里、**未提交**——gen-model HEAD 已经依赖它。

### ModelOnly 执行体补上开工前对齐与 `RoomOnly` 接力（ADR-073 D12 / D13 的漏接，审核 P1-1）

- **为什么**：审核 `codex/model-projection-cache` 在途实现（会话 fable-5-1-35，2026-09-17）发现
  `batch_worker::execute_model_only_body` 整条绕开了 D12 与 D13：没有 `refresh_room_reference_before_task`
  （源码钉 15 与它的测试只钉了 `execute_frozen_batch_body`），回执里 `room_recompute: None` 硬编码。而
  `window_root_plan::reconcile_startup_window` 与稳态窗口共用 `build_model_update_plan_from_window`，本窗碰了
  房间定义族时 `RoomOnly` 工作项**确实**随 `plan.work_items` 进了 `side_actions`，然后被 `run_model_side_actions`
  的白名单过滤掉、无人消费。后果：重启后「落后就排 `model_sesno → 文件最新`」的启动追平任务是 ModelOnly——
  停机期间改了 PANE / FRMW 改名，起来后这一窗把触发①静默吃掉；`model/rebuild`（起点 0）拿**未对齐**的常驻算全库，
  再把 `Aligned` 盖上（对那一代诚实，但那一代可能已过期，`room_alignment.current` 跟着报 `true`）。
- **改动**（代码已随另一路会话的落库进 `d8fc12de7`，本条只补文档）：
  - `execute_model_only_body` 规划成功后、`pre_generation` 之前调 `refresh_room_reference_before_task(job.dbnum)`，
    回执留着；收尾 `room_recompute = if failed || whole_library_model_only(job) { None } else
    { request_for_task(&side_actions, Some(&room_reference)) }`——两条触发都带上；整库重排（起点 0）例外：
    `DbSet@T` 上每一根都刚用对齐后的计算器算过，再来一遍是 O(本库) 的空转，且 durable 侧的对齐承诺已由
    `whole_library_model_only` 那一句盖上；规划失败的早退仍 `None`（模型半边没跑）。
  - 源码钉 15（`room_resident::tests::the_task_start_refresh_precedes_every_calculator_freeze_and_stays_cheap`）
    改成对两个执行体各钉一段——函数体到顶格 `}` 为止，后一个函数的字样不替前一个作证；源码钉 17
    （`room_recompute::tests::the_room_only_recompute_is_wired_after_the_durable_barrier_and_before_task_end`）加
    ModelOnly 那一半：`request_for_task(` + `Some(&room_reference)` 在、`room_recompute: None` 恰一处（早退）、
    `whole_library_model_only(job)` 在。
  - ADR-073 D12 正文与钉 15 / 17、CONTEXT「任务开工前对齐」词条（三处 → 四处）同步。
- **验证**：`cargo test --lib -- room_recompute:: the_task_start_refresh_precedes… the_whole_library_eligibility…
  nothing_outside_the_task_writes_model_sesno` 8 passed / 0 failed（在 `56e1b47ea` 双向同步之后的树上复跑一遍仍绿）；
  rustfmt 对本轮新增行无 diff（三个文件里既有的 9 处漂移属别的会话）。**未 live**：停机期间改 PANE → 重启 → 启动追平
  ModelOnly 任务日志出「整库房间重算完成」这一条真机用例没跑。
- **审核里同批点出、本条未动**：P1-2 触发是边沿型、没有再武装（拆窗 / 单元失败 / 刷盘没齐 / 重算写库失败时原因被丢，
  下一窗账本不再含那条 PANE 变更、再探又是 `Unchanged`，边永久陈旧到下一次不相关的 PANE 变化）；P2-3 收尾盖章读的是
  任务结束那一刻的 `required_room_generation()`，并发 `/model/ensure` 中途换代时 `Aligned` 可能盖上一个边不是它算的代号
  （`dbnum_initialize` 那一路用 `rooms.resident_generation()` 是对的）；P2-4 ADR D13「拿开工时同一台计算器」与实现
  「收尾时新冻一台」措辞漂移；F3 / F4 / U3 照旧。

### 整库房间重算的第三条触发（电平型再武装）与覆盖面自证（ADR-073 D13 / D14，审核 P1-2）

- **为什么**：D13 的两条触发都是边沿型——「本窗碰了房间定义族」「开工前对齐换了代号」的原因只活在本任务里。拆窗（大窗口的
  常态：本批只应用到 sesno X、余量排回队列）、任一单元没生成、`settlement_failed`、刷盘没齐、重算中途写库失败——这些情形下
  `room_recompute` 的原因被丢掉，而下一窗的账本不再含那条 PANE 变更（触发 ① 没了）、本任务开工时已把常驻对齐到文件最新、
  下一窗再探是 `Unchanged`（触发 ② 也没了）：别的根在这块面板里的边**永久陈旧**到下一次不相关的 PANE 变化。ADR 只写了
  「刷盘没齐就不跑」，没写怎么补。审核给了两条路（电平型 / 进程内 `owed` 集合），用户 2026-09-17 07:36 选电平型。
- **改动**（会话 opus-5-36 起草代码、fable-5-1-38 接手收口）：
  - `room_recompute::library_alignment_owed(library, resident, settled_partial)`（纯函数）+ `durable_alignment_owed(dbnum)`
    （取数：`DbnumState::read` 那一行的 `model_room_generation`、`query_service::resident_room_generation()`）：库上 D14 那一格
    与此刻常驻代号不等（含库上没有那一格）= 欠一次整库重算。常驻说不出代号（开关关 / 还在装 / `Blocked`）不触发；水位行读不出
    不触发（只 warn）。两个执行体（`execute_frozen_batch_body`、`execute_model_only_body`）都在 `refresh_room_reference_before_task`
    **之后**读它、以第三个形参喂给 `request_for_task`——顺序钉在源码钉 17 里（对齐之前读到的是上一代）。
  - **覆盖面自证**：起草时发现 `recompute_dbnum_room_edges` 只遍历投影里有回执的根，而初始化的模型行不进投影（ADR-077 D5）、
    落盘形态冷重启后投影是空的——原实现在这种局面下照样把 D14 那一格盖成 `Aligned`，等于替没算过的根说话（而且一盖，电平型
    触发就哑了）。现在 `RoomEdgeRecompute` 多一格 `covered_roots`，`run_room_only_recompute(job, &reason, &mut result, file_latest_sesno)`
    重算齐了再拿 `window_root_plan::enumerate_target_roots(dbnum, T)` 的根清单对一遍（`room_recompute_uncovered` 纯函数），结论收成
    `RoomRecomputeVerdict { WholeLibrary | PartialCoverage | Disabled | Failed }`：收尾分开问两句——`rooms_ok()`（只有 `Failed`
    拖水位）与 `speaks_for_library()`（只有 `WholeLibrary` 盖 D14 那一格）。`PartialCoverage` 水位照盖、那一格不动、warn 点名前 5
    个没覆盖到的根并指向 `model/rebuild`；枚举不出来按不全处理。
  - **去重（本会话补）**：`PartialCoverage` 下电平型触发会在每个任务上再点一次，而同一代下再点结果逐字相同（投影里的根已对齐、
    之后进投影的根都是这一代算的、不在投影里的根这条路够不着）——生产落盘形态下每个初始化过的库在任一 PANE 变化之后都会
    变成这样，代价是每个任务一遍 O(投影) 边重写 + 一次 `DbSet@T` 根枚举 + 一句同样的 warn，直到人工 `model/rebuild`。
    加进程内表 `room_recompute::PARTIAL_COVERAGE_SETTLED: dbnum → 常驻代号`：点了名的覆盖不全 `settle_partial_coverage`，
    同一代不再触发 ③；失败 / 覆盖面未知（枚举失败可能是暂时的）/ 整库 `rearm_library`；代号变了键对不上、重启表空——都重新
    武装。它**不是**库上的事实：`room_alignment.current` 照旧 `false`、`e3d.room.lookup` 照旧 `not_computed`。
  - ADR-073 D13 正文补触发 ③、覆盖面自证、去重、以及**没收的第二半**（生产形态下初始化过的根这条路够不着，「整库」只到投影，
    归 U1 另开卡）；顺手把 P2-4 的措辞漂移改掉（「拿开工时同一台计算器」→「收尾时新冻一台，此刻这一代」，与实现一致）、并把
    P3-5「此刻本库没有第二个写者」改成实情（`/model/ensure` 的同库根可能仍在 write-behind 队列里，同一台计算器下幂等）。D14 写
    规则加「且覆盖到 `DbSet@T` 的每一根」。源码钉 17 / 18 扩到上述钉点；U1 追记。CONTEXT「整库房间重算」「整库房间对齐代号」
    「ModelOnly」三词条同步。
- **验证**：`cargo test --lib -- room_recompute:: the_task_start_refresh… the_whole_library_eligibility… a_partial_coverage_recompute…
  nothing_outside_the_task_writes_model_sesno a_window_with_no_model_work_still_stamps recompute_dbnum_room_edges room_edge
  the_room_generation_stamp only_a_task_that_speaks` **22 passed / 0 failed**（含新用例
  `the_library_owes_a_recompute_until_its_durable_alignment_matches_the_resident`、
  `a_partial_coverage_settles_the_level_trigger_for_that_generation_only`、
  `a_partial_coverage_recompute_keeps_the_watermark_but_never_claims_library_alignment`，源码钉 15 / 17 全绿）；
  `cargo test --lib` 全量 **2216 passed / 4 failed / 118 ignored**——4 个失败全在 `model_impact` / `window_root_plan` 的
  CACHID 元数据档（`the_metadata_tier_is_curated_only…`、`the_skip_verdict_uses_the_same_double_vote…`、
  `a_change_is_metadata_only_when…`、`the_element_tier_is_the_widest_tier…`），随 vendor `e3d-model` 工作树里另一路会话未提交的
  `element_diff.rs` / `increment.rs`（Q6 只 `CACHID` → `no_model`）而来，与本条无关、本条前后同样失败。rustfmt 对本轮新增行无 diff
  （`batch_worker.rs` 6 处、`model_db_adapter.rs` 2 处漂移为既有）。**未 live**：「PANE 变化 → 拆窗 / 刷盘没齐丢掉触发 → 下一个任务
  日志出『库上整库房间对齐代号 … ≠ 此刻常驻代号 …』并跑整库重算」与「初始化过的库 → `PartialCoverage` warn 一次、下一个任务不再重复」
  两条真机用例没跑。
- **未动**：P2-3（收尾盖章读的是任务结束那一刻的 `required_room_generation()`）紧接着下一条收掉；U1 第二半（初始化过的根不在投影里）
  要么让重算回读库上几何、要么接受滞后到整库重建，另开决策卡；F3 / F4 / U3 照旧。

### `model_room_generation` 盖的是边真正用的那一代，不是收尾那一刻的常驻代号（ADR-073 D14，审核 P2-3）

- **为什么**：`task_room_generation_stamp` 此前对两路整库任务都拿收尾那一刻的 `required_room_generation()` 盖 `Aligned`。批次 worker
  单消费者，但 `/model/ensure` 与初始化 lane 都会并发调开工前对齐（D12）——一个 DESI 前进 + 用户同时点「显示模型」就会在任务中途
  换代：整库重排的 ModelOnly 各批根由 write-behind 每次刷盘时 `frozen_room_calculator()` 那台算，前半 G1、后半 G2，收尾盖 G2；
  整库房间重算的边是收尾冻的那台（G1）算的，盖章时常驻已到 G2，也盖 G2。两种都是「拿上一代的边冒充这一版」——D14 要挡的事；
  电平型触发 ③（上一条）读的正是这一格，盖错了它就哑了。`dbnum_initialize` 那一路用 `rooms.resident_generation()` 本来就对。
- **改动**：
  - `RoomRecomputeVerdict::WholeLibrary` 带载荷 = 重算那台计算器的 `resident_generation()`（`speaks_for_library()` →
    `library_generation() -> Option<&str>`），收尾盖它——边是它算的、ACK 已对齐到它，此刻常驻是哪一代、取不取得到都无关。
  - `DataBatchTaskResult` 加 `room_generation_at_start`（不进回执）：ModelOnly 执行体在开工前对齐**之后**、生成之前记一次
    `required_room_generation()`；整库重排收尾拿它与此刻比——相等 = 每次刷盘冻的都是这一代，盖；不等 / 没记下 / 此刻取不到 →
    `Untouched`，下一个任务由 ③ 补一次整库重算。
  - 判据收成 `room_generation_stamp_of(now, LibraryRoomClaim { None | Recomputed(代号) | Rebuilt { started_at } })`：`Ok(None)`
    一律 `Cleared`（开关关着），其余按上两条。ADR-073 D14 写规则加「代号出处」一段、钉 19 扩到它；CONTEXT「整库房间对齐代号」同步。
- **验证**：`cargo test --lib -- room_recompute:: … only_a_task_that_speaks … a_partial_coverage_recompute …` **23 passed / 0 failed**
  （`only_a_task_that_speaks_for_the_whole_library_stamps_the_durable_room_generation` 重写成三路真值表 + 源码钉：载荷来自
  `rooms.resident_generation()`、开工代号落在对齐之后 / `run_unit_worklist(` 之前、收尾从回执里取；`a_partial_coverage_recompute…`
  的四态表改成 `library_generation()`；源码钉 17 的 `tail` 改认 `RoomRecomputeVerdict::library_generation`）。rustfmt 本轮新增行无 diff。
  **未 live**：任务中途并发 `/model/ensure` 换代 → 整库重排收尾 `Untouched`、下一个任务由 ③ 补整库重算，这条真机用例没跑。
- **接受的角**：代号 A→B→A 在任务内翻回去（开工 A、中途 B、收尾 A）两路都判不出——整库重排要逐根记刷盘代号才判得出，
  整库重算不受影响（单台计算器）。面板改动在一个任务里改完又改回，实际不会发生，不为它加逐根记账。

### 开工前对齐的在册视图之差只算一遍（审核 P3-7）

- **为什么**：`room_resident::refresh_before_task` 重建成功后对 `installed` 与新模型调 `TopologyDiff::between`，而
  `initialize_file_backed_at_startup` 里 `ResidentRooms::install()` 刚对同两份视图算过一份、被 `.model` 丢掉。差是 O(在册面板)
  的两张表比对，重建本身已经是 O(文件) 的事，多算一遍不伤大局，但两处对着同一对输入各算各的，哪天一处改了口径另一处就漂。
- **改动**：`StartupInitOutcome` 加 `diff: Option<TopologyDiff>`，两个启动装载器（文件形态 / 读透 `latest`）用
  `let InstallOutcome { model, diff } = …install(model)` 把它接住；`refresh_before_task` 改用 `outcome.diff`。语义不变：
  `install()` 是对着被换下的那一代算的，与 `installed` 是同一份——本函数持 `refresh_lock`，启动段的 `Loading` 在入口已挡开。
  新源码钉 `the_task_start_refresh_reuses_the_install_diff_instead_of_computing_it_twice`：`refresh_before_task` 函数体不含
  `TopologyDiff::between(`、含 `let diff = outcome.diff;`；两个装载器都接 `diff`；生产代码里 `TopologyDiff::between(` 只剩启动对比
  （`StartupRoomPlan`）与 `install()` 两处。
- **验证**：`cargo test --lib -- room_resident::` **29 passed / 0 failed / 9 ignored**；rustfmt 本轮新增行无 diff（该文件 100 / 958 / 1249
  三处漂移为既有）。

### 开工前对齐不许在空间串行锁里调——源码钉（审核 P3-8）

- **为什么**：`room_resident::refresh_before_task` 换代走 `initialize_file_backed_at_startup`，那里取 `spatial_state::lock_spatial_serial()`
  整份替换在册视图；而那是一把 tokio Mutex、不可重入——任何持着它的调用方再调开工前对齐就是自锁：不报错、不 panic，那个任务永远
  收不了尾。今天五处调用点（适配层转手、两个执行体、初始化模型半边、ensure 准入）都在锁外，但这条规矩只活在人脑里，D12 再接一处
  新调用点时没有东西挡。
- **改动**：生产代码不动，只加源码钉 `room_resident::tests::the_task_start_refresh_is_never_called_under_the_spatial_serial_lock`：
  扫全仓 `src/**/*.rs` 的生产段（每个文件到第一个内联 `#[cfg(test)] mod … {` 为止），每一处 `refresh_before_task()` /
  `refresh_room_reference_before_task(` 的调用，所在函数体全文不含 `lock_spatial_serial(`；调用点清单钉成五处的表
  （`batch_worker::execute_frozen_batch_body` / `execute_model_only_body`、`dbnum_initialize::model_half_inner`、
  `model_dbnum_ensure::start_with_purpose`、`model_db_adapter::refresh_room_reference_before_task`），新调用点得先确认它与它的
  调用方不在锁内再进表；另钉规矩的由来——`refresh_before_task` 经 `initialize_file_backed_at_startup(` 换代、自己不持锁，装载器体内
  `lock_spatial_serial().await` 在。钉只看直接的函数体；深一层的持锁者（生产里 13 处：`query_service::room` / `spatial_bounds`、
  AABB 刷新 / 持久化 / 装载、`helper::delete_room_membership`、两个启动装载器、`room_read_through::lookup`）本轮人工核过一遍，
  没有一处走到那五个入口。
- **验证**：`cargo test --lib -- the_task_start_refresh_is_never_called_under_the_spatial_serial_lock` 1 passed；变异一次——在
  `model_dbnum_ensure::start_with_purpose` 调对齐之前插一句 `lock_spatial_serial().await`，钉红并点名
  `data_interface/model_dbnum_ensure.rs::start_with_purpose`，撤掉后复绿；`cargo test --lib -- room_resident::`
  **30 passed / 0 failed / 9 ignored**；rustfmt 本轮新增行无 diff（该文件 100 / 958 / 1249 三处漂移为既有）。

### 开工前对齐的重建失败按同一文件版本指数退避，不再每个任务整份重建再失败（ADR-073 D12，审核 P3-6）

- **为什么**：`room_resident::refresh_before_task` 在启动 `Blocked`（或上一次换代没成、留着旧代）且原因持续——MDB 读不了、某个 DESI
  文件打不开——时，每个 dbnum 任务开工都走一遍 `initialize_file_backed_at_startup`：O(全部文件) 的拓扑 + 面板几何重建，然后以同一个
  原因再失败一次、`last_failure` 再刷一条一模一样的。探针给的 composite 就是重建的全部输入（MDB 成员清单 + 每个 DESI 最新会话 + 令牌），
  composite 没变而上一次没成，多半还是不成；但也不能永久跳过——文件被别的写者锁着、盘暂时读不到这类暂时性故障，D12 承诺「不再要重启
  才能恢复」。
- **改动**：`room_resident::RebuildBackoff { key, failures, last_error, retry_at }` 进程内一份（持 `refresh_lock` 时读写；重启为空）：
  同一 composite 下连续失败 `cooldown(n) = 30s × 2^(n-1)`、10min 封顶；`refresh_before_task` 探针之后、重建之前先问
  `backoff.remaining(&probe.composite, now)`，冷却期未到回新变体 **`RoomReferenceRefresh::Deferred { key, error, kept_previous,
  failures, retry_in_ms }`**——不重建、生命周期与 `last_failure` 都不再动，任务日志一行「文件版本 … 下常驻房间模型重建已连续失败 n 次，
  退避中（还有 …ms 再试）」；composite 变了 `remaining` 为 `None` 照常重建、失败从 1 数起；建成了 `set_rebuild_backoff(None)` 清账。
  启动装载器失败**不**记这本账：进程起来后第一个任务总要亲自试一次，那就是自愈的机会。`/health.room_build.resident` 加一格
  `rebuild_backoff: { key, failures, last_error, retry_in_ms } | null`。`request_for_task` 只认 `Rebuilt { generation_changed: true }`，
  `Deferred` 不触发整库重算（常驻没换代）。ADR-073 D12 正文与钉 20、CONTEXT「任务开工前对齐」词条同步。
- **验证**：`cargo test --lib -- room_resident:: room_recompute:: the_task_start_refresh` **39 passed / 0 failed / 9 ignored**（含新增
  `a_failed_rebuild_backs_off_under_the_same_composite_and_rearms_when_the_files_move`——冷却 / 翻倍 / 封顶 / 换 composite 归 1 /
  冷却期到即再试 / health 格 / `Deferred` 日志行——与源码钉 `the_task_start_refresh_backs_off_before_rebuilding_and_settles_the_ledger_on_both_outcomes`；
  既有钉 15 / P3-7 / P3-8 仍绿）；变异一次——删掉 `refresh_before_task` 里的退避早退，钉红在「退避回执」、撤掉后复绿；rustfmt 本轮
  新增行无 diff（该文件 100 / 958 / 1250 三处漂移为既有）。**未 live**：「MDB 不可读启动 → 连续任务开工只重建一次、之后日志出『退避中』」
  这条真机用例没跑。

### 整库房间重算对不在投影里的根回读库上几何——「整库」不再只到投影（ADR-073 D13 / D14，U1 第二半）

- **为什么**：D13 的整库重算只遍历投影里有回执的根，而投影只装本进程生成过的根——初始化的模型行不进投影（ADR-077 D5）、
  落盘形态冷重启后投影是空的。生产形态下面板一变，那些根的边这条路够不着：`e3d.room.lookup` 对它们恒 `not_computed`、D14 那一格
  永远盖不上，电平型触发 ③ 每个任务都点、每次都 `PartialCoverage`（上午为此还加了一张进程内去重表），点亮只有 `model/rebuild`。
  用户 2026-09-17 拍板：让重算为不在投影里的根回读库上几何、把边补齐（不选「接受滞后到整库重建」）。
- **改动**：
  - `room_model`：房间判定的输入抽成 trait `RoomInput`（构件、世界盒、网格、「网格 → 世界」变换、报错点名），`ProjectedGeometry`
    实现它；新增 `DurableRoomGeometry` + `durable_room_inputs(&[GeomInstQuery])`——把库上读回的 `inst_relate` / `tubi_relate` 行折成
    同一形状（一行一条或多条实例、共用该行的世界盒；变换 = 世界 × 实例本地，烘焙 / 管身读回的本地是单位阵，与投影 `Baked` 同义）。
    `calculate_for_root` / `render_root_room_edges` / `calculate_edges` / `memory_model_world_points` 泛化到 `G: RoomInput`，算法一行
    没改；`calculate`（读透）仍收 `ProjectedGeometry`。
  - `model_record_reader::durable_records_of_root` 提成 `pub(crate)`（SQL 不动）。
  - `model_db_adapter::recompute_dbnum_room_edges(…, library_roots: Option<&[RefNo]>)`：调用方点名 `DbSet@T` 的根清单；投影里有回执的
    照旧拿投影，清单里不在投影里的（去重、只认本库、按 refno 序）逐根 `durable_records_of_root` → `durable_room_inputs` → **同一个**
    `calculate_for_root` / `render_root_room_edges`（两种来源共用一段 `recompute_root`），读回零行的算覆盖到、什么都不写；回读失败
    整轮 `Err`（与写库失败同一处置）。回执加 `projected_roots` / `durable_roots` / `durable_records` / `durable_read_ms`，`covered_roots`
    = 两种来源之和；ACK 只对投影里的根对齐（回读的根没有进程内 ACK，就绪由 D14 那一格作答）。
  - `batch_worker::run_room_only_recompute`：根清单**先**枚举、作为靶子传进去；`WholeLibrary` 从此在落盘形态下也成立。`PartialCoverage`
    只剩「清单枚举不出来、覆盖面未知」（点名了却没覆盖到的臂保留、如实不盖，回读按清单逐根走、理论上不再出现）；任务日志多报
    「投影里 N、回读库 M：读回几何 K 条、T ms」。
  - **去重表退役**：`room_recompute::PARTIAL_COVERAGE_SETTLED` / `settle_partial_coverage` / `rearm_library` /
    `partial_coverage_settled_at` 删掉，`library_alignment_owed(library, resident)` 少一个形参——它的存在理由（同一代下反复点、每次都
    覆盖不全）随回读消失：③ 在每个库上多跑一次就盖上那一格、自己安静。
  - 文档：ADR-073 D13 正文（覆盖面那段重写、「没收的两半」收成一半）、D14 写规则、钉 17 / 18、U1 追记「第二半已收」；CONTEXT
    「整库房间重算」「整库房间对齐代号」「ModelOnly」；spec §4.7 `room_alignment` 的「要点亮」一句改成「下一个任务会自己盖上，不想等再
    rebuild」。
- **验证**：新 `mem://` 真测 `a_root_outside_the_projection_gets_its_room_edges_recomputed_from_the_durable_rows`：A 经逐根路径落库后
  换成空投影、面板搬家 → 点名 `[A]` 回读重算：`(projected, durable, records) = (0, 1, 1)`、`recomputed=1 / room_edges=0 / acks_refreshed=0`，
  旧边删掉、`inst_relate` 行不动；搬回来 `room_edges=1`、边与逐根落库逐字相同；点名 `[B, A, 别库根]` → `covered_roots=[A, B]`、B 零行算
  覆盖到。新源码钉 `the_library_recompute_reads_back_the_durable_geometry_of_roots_outside_the_projection`（枚举在重算之前并真的传进去；
  `recompute_dbnum_room_edges` 签名 / 投影根不回读 / 先读后折 / 两个入口各恰一处；去重表不再定义）；既有
  `recomputing_a_librarys_room_edges…` 加 `(projected, durable, records) == (2, 0, 0)`；`a_partial_coverage_settles_the_level_trigger…`
  随表删掉。`cargo test --lib -- room_recompute:: room_model:: model_record_reader:: room_read_through:: model_initial_persist::
  room_resident:: …` **99 passed / 0 failed / 9 ignored**；全量 `cargo test --lib` **2225 passed / 4 failed / 118 ignored**（4 失败仍是
  `model_impact` ×2 + `window_root_plan` ×2 那四条 CACHID 元数据档，本条前后同样红）。rustfmt：五个 Rust 文件的漂移数与 HEAD 逐个相等
  （room_model 0/0、model_db_adapter 2/2、model_record_reader 4/4、batch_worker 6/6、room_recompute 0/0），EOL 未翻。**未 live**：
  落盘形态冷重启 → 改一块 PANE → 下一个任务日志「整库房间重算完成…回读库 N」且 `/dbnums.room_alignment.current` 翻 `true`、
  `e3d.room.lookup` 对初始化过的根回 `ready`，这条真机用例没跑。
- **代价与边界**：回读是 O(本库根数) 次库读（每根两条 SELECT，按 `anc` / `in` 索引），只在真触发时付；跨库那一半（U3）照旧；变换经
  TRS（f32）往返读回，与 Plant UI 读的是同一份。

### 元数据门第二档的开关退役：`model_skip_unread_attribute_changes` 不再存在，两票就是谓词本身

- **为什么**：`gen-model-refactor` 与 `codex/model-projection-cache` 各自实现过一遍 S6 第二档。双向同步
  （`56e1b47ea`）留下的是 refactor 那一版——`change_metadata_tier` → `model_impact::metadata_only_tier`，
  「字典 DCHC=0 ∧ 生成侧没有一处按名字读它」是 `attribute_is_metadata_only` 的**定义**，没有独立开关。
  另一版带来的 `model_skip_unread_attribute_changes` 因此成了空挡：配置里还在、预览薄壳还读它往下传，
  可谓词根本不看。设成 `false` 的那一份字面意思（只放行老五个名字）与实际行为（两票常开、放行面宽得多）
  恰好相反——少算方向的静默失效，代价是一张停在旧形状上的网格。
- **改动**：
  - `options.rs` 删掉配置字段、`DbOptionExtFields` 那一格、`model_skip_unread_attribute_changes()`
    与 `effective_…`、环境变量 `AIOS_MODEL_SKIP_UNREAD_ATTRIBUTE_CHANGES` 的读取，以及那条默认值用例；
  - 照 ADR-031 的退役纪律**不是一删了之**：新增 `retired_unread_attribute_tier_notice()`，独立读原始 TOML
    与环境变量，残留设置时在启动期打一条告警（`lib.rs` 与 `retired_net_window_notice()` 并排调用）。
    `probe_retired_net_window_config` 抽成按键名取值的 `probe_retired_config_key`，两条退役键共用；
  - `model_impact.rs`：`classify_operation_effects_with(op, unread_gate)` / `classify_operation_impact_with`
    合回不带开关的 `classify_operation_effects` / `classify_operation_impact`；
  - `manual_update.rs`：`merge_net_change_details_with(range_eles, metadata_gate)` 少一个形参，
    预览与窗口规划器天然同口径；源码钉从「薄壳必须读第二个开关」翻成「薄壳不许再读它」。
- **行为**：逐位不变——第二档本来就已常开，这一刀只是把那个不起作用的旋钮撤掉、并让残留配置出声。
- **验证**：`cargo check --all-targets` 绿；`cargo test --lib` 2216 passed / 0 failed / 117 ignored。

## 2026-09-16

### 属性变化分类对齐 core · S4 探针读法——被属主消费的锚点两态相等报「属主不变量未变」而非「无判别力」

- **为什么**：负体 / 开孔 / 剖面 / 几何集这类锚点自己没有挂在自己 refno 上的记录，锚点子树里由 Δ 前移的记录数 `moved` 恒 0，
  两态相等时 `probe_rigid_exclusion_windows` 一律报「无判别力」——读起来像样本没挑好，其实是属主的体积 / 三角数 / 亏格 / 包围盒
  看不见这一挪（db8000 609→610：NBOX 在 BOX 体内平移 200 mm 报相等，可两端 BOX 体积都是 1e9 − 300²×1000、亏格 1，生成器早就
  扣了；614→615 孔越出 +E 面才在属主链 BOX 上判出 triangles / genus / volume 不等）。
- **改动**（`window_root_plan.rs` 测试模块，生产代码零改动）：`category_consumed_by_owner` 圈出四类——负体（属主 CSG 消费）/
  非单元挂在消费成员的属主下（PLOPEN 在 PANE 下那类开孔）/ 剖面 · 路径数据 / 几何集成员；`ParityOutcome::summary(consumed_by_owner)`
  对这四类相等且 `moved == 0` 报「两态相等，属主不变量未变（…；锚点由属主消费、自己无记录）——分不清「属主没消费它」与「体积 /
  三角数 / 亏格 / 包围盒看不见这一挪」，要坐实排除得挑会改属主体积 / 亏格 / 包围盒的挪法」，别的类照旧「无判别力」；报告表头那一列
  改成「无判别力 / 属主不变量未变」、口径段补这一句读法、读法列对这四类只有这种样本时给「属主不变量未变：分不清「没消费」与「看不见
  这一挪」，未判——换会改属主体积 / 亏格 / 包围盒的挪法再造窗」。判别力口径不变：这种样本照旧不算有判别力、不占抽样名额。
- **验证**：新单测 `consumed_by_owner_equal_reads_as_owner_invariants_unchanged` 过（四类判真、五个别的类判假、两句读法互斥、
  `moved > 0` 的相等不受类别影响）；探针钉 db8000 609→610 复跑（`output/e3d-s4/probe_window_A_609_610_reading.md`）：样本行与
  读法列都成了「属主不变量未变」，test ok；`cargo check --lib --tests` 绿；`rustfmt --check` 本文件只剩两处原有的差异；CRLF 未翻。

### ADR-078 D1 / D6 / D8：真生产方 `UnitDeltaProducer` 接进单元级执行体，真库探针 ams8000 五窗五根与整根重算逐字同章

- **生产方**（`e3d_model_service.rs`）：`UnitDeltaProducer::open(service, dbnum, S, T)` 一窗算一次——`collect_window` →
  两端 `generation_input`（与整根重算共用缓存的 `DbSet` / 索引）→ `plan_update`，**计划口径与选根那一步同一份**
  （元数据门开着走 `plan_update_with(change_metadata_tier)`，关着走裸 `plan_update`；ADR 加实施约束 12）。
  `UpdatePlan` 会被 `execute_plan` 整个吃掉、又不 `Clone`，拆成 `WindowPlanSets` 四个集合放着；`produce(root, old_receipt)`
  逐根切子计划（`regenerate` / 活着的派生容器按 T 端属主表的祖先链归根，`remove` / 退休的容器按 S 端属主表，
  `derived_stale` 跟容器；祖先链读挂按错误上抛）→ `execute_plan(target, 子计划)` → `bake_elements` →
  `project_geometry_delta(…, None, owners@T)` → `UnitDelta`。`execute_plan` 从此有了 gen-model 侧第一个调用者。
  入口 `E3dModelService::republish_roots_by_unit_delta_from_window(dbnum, S, T, units)`，unit 的 `source_start_sesno`
  必须就是这一窗的 S。
- **生产方的回退五种**（各自 `bail!`，执行体接住写进 `fallbacks`，D8）：① 窗计划退化成整库重建（Q4：空产出不是「没变」）；
  ② 计划期有读挂（`unresolved` / `derived_stale_unreadable`）；③ 旧回执没带目标、或**不可变目标在设计窗之外变了**
  （`same_target_outside_the_design_window`：只有 `design.session` / `design.digest` 前移是局部该看到的；目录会话 /
  模板 / 生成器指纹 / 细分档任何一格变了都是 core `REPALL` 那一档，ADR 加实施约束 11）；④ `execute_plan` 报
  `regen_failed` / `derived_failed`；⑤ removal 回溯不到子计划。
- **removals 来源守卫**（D6 第一条）：`removals_trace_back_to_the_plan`——产出里每条 removal 都要回溯到子计划的三处之一：
  `remove` 的单元、`regenerate` 里画不出 / 画失败被擦掉的单元、派生容器 base 端旧管段键。ADR D6 第一条按源码补了
  中间那一处（原文只写了两处）。
- **管段账的来源**（`UnitDelta::tube_segments`）：**e3d-model** `IncrementReport` 新加 `tube_segments: BTreeMap<String, usize>`
  ——`execute_plan` 在 `route_tubes_of` 返回处逐容器记「这一轮真建成几段」（退休 / 类型已变记 0，读挂不记），与
  `upserts.push` 分开记；`accounts_for` 多一条恒等式（各值之和 == `tube_upserts`，条数 == 建成 + 退休容器数），
  单测 `per_container_tube_segments_must_split_the_tube_upserts`，原 `derived_execution_accounting_must_close` 夹具补账。
  e3d-model 真库门 `an_increment_says_what_two_full_generations_say` 等三条跑过（22 min），六窗账全平。
- **真库探针** `probe_unit_publish_one_root_against_regen`（`#[ignore]`，`from_current` 的 MDB + E3D 模板目录；旋钮
  `AIOS_UNIT_PUBLISH_PROBE_DBNUM` / `_WINDOW=S:T` / `_ROOT`）：一窗里挑一根 regen 根，S 端整根重算灌进本地投影当基线
  （回执停在 S、目标是 S 的），走生产方 + 执行体，再与 T 端整根重算比整根 `manifest_hash`，不等时按 `geometry_id`
  逐条报 DIFF / EXTRA / MISSING。**ams8000_0001 五窗五根全部逐字相同**：609→610 与 614→615（S4 造的 NBOX 挪位窗，
  EQUI `24384/24776`，2 件）、45→46（改挂窗，新根 EQUI `24384/25803`，S 端 0 件 → 1 件）、195→196（级联窗，
  EQUI `24384/26186`，1 件）、**249→250（管路窗，BRAN `24384/26229`，16 件：重建 1 单元 + 1 派生容器，delta
  5 upsert / 3 removal / 1 branch reset，管段门放行）**。这一根单元路 0.02 s 对 regen 0.07 s；根都很小，A7 的净收益
  要在大根上另量。默认窗（链尾 618→619）regen 根为 0，探针会明说「pass AIOS_UNIT_PUBLISH_PROBE_ROOT」。
- **单测三条**：`a_removal_that_does_not_trace_back_to_the_plan_is_refused`（三处来源放行；计划外单元 / 别的容器旧键拒收，
  原因点名那一条）、`a_target_change_outside_the_design_window_disqualifies_a_partial_delta`（设计会话前移放行；目录 /
  生成器 / 模板 / 细分档各变一格都拒）、`a_degraded_or_unreadable_window_plan_admits_no_partial_publish`（Q4 收口：
  算本 ADR 的门，落在 gen-model 侧）。
- 验证：gen-model `cargo test --lib` **2146 绿 / 0 红 / 115 ignored**（+1 ignored = 探针；常规比 D6 多 3），
  e3d-model **321 绿 / 0 红**；e3d-model 真库门三条 1349 s 全绿；`rustfmt --edition 2024 --check` 我改到的每一段都干净
  （`e3d_model_service.rs` 剩 10 处、`increment.rs` 0 处，按行号对过全是别人的 hunk 或 HEAD 里就有）；两文件行尾 LF 未变；
  changelog / ADR 全 CRLF。
- **没做**：D9 开关 `model_unit_level_publish` 未立，`republish_roots_by_unit_delta_from_window` 没有生产调用点；
  A6 全语料多根对拍未跑（只有单根探针）；A7 净收益未量；实施约束 7（transform 优先于 unit）、D2 那条「删空不走 Partial」
  未接。`produce` 是同步阻塞的 I/O + CPU，今天在 async 执行体里直接调（与刚体路同款），接生产调用点时要放
  `spawn_blocking`。

### ADR-078 D6 / A3：管段完整性门夹在执行体的产出与提交之间——触到的 BRAN 少带一段就整根回退，不发

- **门**：`e3d_model_service::tube_segment_completeness(&UnitDelta) -> Result<(), String>`，在
  `republish_roots_by_unit_delta_on` 里位于 `produce` 之后、`publish` 之前；不成就 `fallbacks` 一条
  「tube segment completeness: …; falling back to regen」，一个字都不发（D8）。守卫
  `the_unit_publish_gate_precedes_production_and_the_executor_reads_no_records` 多核一格：产出 < 管段门 < 提交。
- **账**：`UnitDelta` 新加 `tube_segments: BTreeMap<(word0, word1), usize>`——本次触到的每条 BRAN 在 T 端**应有**的
  管段数，生产方按 `route_tubes_of` **另记一账**、不从 `upserts` 数回来（数回来就是自证）。键用 `(u32, u32)`：
  `RefNo` 不实现 `Ord`，与 `owners` 表同一约定。
- **实施约束 4 按源码改口**：原写「== `execute_plan` 对 C 算出的槽位数」，但 `execute_plan` 的 `kept` 只收**真建成**的管身、
  退化槽位不出几何——拿 `route_slot_keys` 的全部槽位数去核会把正常的洞误拦成漏段。改成「== 真建成的管段数」，
  只数段数、不核 `route_ordinal` 连不连续。
- **核三件事**：① 带了 C 的段就必须重置 C（不重置，旧段留在新段旁边成鬼影）；② 触到的每条 C 都要有账；
  ③ 带上的段数 == 账上的段数（少了是漏段、多了是账错）。账上 0 段 + 只重置不带段 = 容器退休的合法整段清空。
- **护栏**（均实跑）：`the_tube_segment_gate_rejects_unreset_unaccounted_and_short_branches`（四种拦、三种放，逐条核原因文本）；
  `a_partial_delta_missing_one_tube_segment_falls_back_before_publishing`（**A3**：一条 BRAN 两段 + 一件元素，只带 0 号段来
  → 回退、原因含 `BRAN 24383/721` / `carries 1 tube segment(s), plan expects 2`、发布口没被叫到、`root_snapshot` 前后逐字相同；
  补齐两段再来 → 走成局部提交，两段整段换新、元素原地不动、回执前移到 43 / 数 3）。把门短路成恒放行后 A3 那条红
  （mutation 核过）。
- 验证：gen-model `cargo test --lib` **2143 绿 / 0 红 / 114 ignored**（比 D4 多 2），e3d-model **320 绿 / 0 红**；
  `rustfmt --edition 2024 --check` 我改到的每一段都干净（`e3d_model_service.rs` 剩 10 处全在别人的 hunk 里或 HEAD 里就有）；
  行尾 LF 未变；changelog / ADR 全 CRLF。
- **没做**：D6 第一条「`removals` 只许来自 `UpdatePlan.remove` ∪ 派生旧键差集」还没在执行体里核——今天 `UnitDelta::removals`
  是生产方给什么算什么，接上 `execute_plan` 时钉成守卫。仍没有生产调用点（D9 开关未立）。

### ADR-078 D4 / A4：单元级发布执行体骨架——`republish_gate` 当 `Partial` 的幂等闸，闸先于产出，执行体自己不读记录

- **执行体立起来了**：`E3dModelService::republish_roots_by_unit_delta(dbnum, T, units, produce)`，与刚体路
  `republish_roots_by_rigid_motion` 同形、同一把 `db_generation_lock(dbnum)`、同一道 `ensure_not_older_than_persisted`
  （D4：不新增锁）。本体拆成无锁的 `republish_roots_by_unit_delta_on(store, mesh_dir, T, units, produce, publish)`，
  好在一个本地存储、一个本地发布口上把顺序钉住。每根 **完整性前置 → 闸 → 产出 → `Partial` 提交**（实施约束 5）。
- **闸就是 ADR-066 那一道**：`republish_gate(old_receipt.source_sesno, unit.source_start_sesno, T)`，同一个函数、
  同一个 `RepublishGate`，不另造判据。回执已在 T → `replayed`（跳过、按已完成计）；回执不在规划基线 S 上 →
  `fallbacks`，原因写明「receipt at sesno X / planned against baseline S / target T / falling back to regen」。
  两种否决都在**叫生产方之前**——一件几何都不求、一个字都不发。残缺 / 无回执 / 解析不了的根在闸之前回退；
  生产方报错照 D8 回退并记原因；`publish` 报错（write-behind 不收件）整轮上抛，与刚体路一致。
- **执行体不读任何记录、不自己算章**：删什么由产出点名（`UnitDelta::removals`），整根的章由存储在写锁里盖（D5），
  所以闸后面没有第二个能读到旧状态的窗口——用户那句「闸先于读 `records_of_root`」在这里落成「执行体正文里
  根本没有 `records_of_root`」。`include_str!` 守卫 `the_unit_publish_gate_precedes_production_and_the_executor_reads_no_records`
  钉住：`records_of_root(` / `geometry_ids_of_root(` / `root_snapshot(` / `manifest_hash(` 一个都不许出现，
  且 完整性前置 < 闸 < 产出 < 提交 的位置序、`scope: CommitScope::Partial`、`published_via: "unit"`、
  与刚体路同锁同闸，逐条核文本。
- **新类型三个**（`e3d_model_service.rs`）：`UnitPublishUnit { root_refno, source_start_sesno }`（与 `RigidMotionUnit`
  同形、少 anchors）、`UnitPublishReport { completed, replayed, fallbacks }`（与 `RigidMotionReport` 同形）、
  `UnitDelta { tube_branch_resets, removals, upserts, model_target }`——`scope` 与回执**不在里面**，由执行体盖，
  生产方替不了它决定这一笔是不是局部。`produce` 是注入的 `FnMut(&UnitPublishUnit, RefNo, &RootReceipt) -> Result<UnitDelta>`：
  接上 `execute_plan` 之前由调用方给，接上之后就是它。
- **护栏**（均实跑）：`the_unit_gate_sorts_replays_and_baseline_mismatches_before_anything_is_produced`
  （重放 / 基线不符 / 残缺 / 解析不了 / 生产方报错五种一次排开：只有过了闸的那根叫到生产方，没有一根走到发布，
  四根 `root_snapshot` 前后逐字相同，原因逐条核文本）；`a_unit_root_on_its_baseline_publishes_a_partial_commit_and_replays_idempotently`
  （**A4**：第一次走成 `Partial` / `"unit"` / `source_sesno` 前移到 T / 章由存储盖 == 对提交后整根跑配方 /
  没点名那件停在旧会话 42 上——正是 D3 放宽 `<=` 接的形状 / 提交后仍 `root_projection_complete`；第二次同一个计划再提，
  闸判 `replayed`，生产方与发布口一次都没被叫到，`root_snapshot` 前后逐字相同）；守卫一条见上。
  把闸的 `continue` 拿掉后两条行为测试全红（mutation 核过）。中途写坏过一次夹具：四根共用同一批成员 refno，
  记录 id 相撞、后一根把前一根的记录「搬」走，`root_projection_complete` 变假——成员改按根号偏移，不是产品问题。
- 验证：gen-model `cargo test --lib` **2141 绿 / 0 红 / 114 ignored**（比 D5 多 3），e3d-model **320 绿 / 0 红**；
  `rustfmt --edition 2024 --check` 我改到的每一段都干净（`e3d_model_service.rs` 剩下 10 处按行号对过，全在别人的
  hunk 里或 HEAD 里就有）；行尾 LF 未变；changelog / ADR 全 CRLF。
- **仍没有生产调用点**（D9 开关未立，`republish_roots_by_unit_delta` 无人调）。实施约束 4（管段完整性门，D6 / A3）
  与 7（transform 优先于 unit）还没接进执行体；D2 那条「一根被删空不走 `Partial`」也还没在执行体里挡。

### 属性变化分类对齐 core · S8/Q6 成因表的反向一格补判：只 `CACHID` 的候选没有一个是「祖先被编辑」带出来的，Q6 的账一格不动

- **为什么要查**：S4 窗 C 现场纠正了 S8 的机制措辞（`CACHID` 不只盖顶层件，**改谁就盖谁整棵子树**）。S8 的成因表只问过一个方向——
  「有没有真改动**以我为严格祖先**」（改动在我**之下**），12476 个因此落进「子树内一个都没变」。反向那一格（**我的祖先**在同窗真变了）从来没判过，
  而它正是窗 C 那个形状。用户点名要核：这会不会推翻 Q6 那 12734 个只 `CACHID` 候选的账。
- **怎么核的**：临时探针 `src/bin/zz_tmp_q6_ancestor.rs`（未跟踪）**独立重实现** `stamp_only` 五条判据（用 `PLACEMENT_STAMP_ATTRIBUTES` 常量，不写字面量）
  与属主链上溯，同一份语料、每库整条会话链逐窗，两个方向一起判（深度上限 64，`ANCESTRY_LIMIT = 5000` 同 S8）。
- **一个必须先滤掉的假象**：第一版「之上有变」12474 / 12497 几乎满格，最近的那个变了的祖先**永远是同一个** `WORL [UPDT]`——全库根的每会话时间戳，
  它是所有元素的祖先，不滤掉这一格恒真。滤法：祖先的差分只有 `UPDT` 一个名字就算内务、继续往上找。滤后这一格从 12474 塌到 0。
- **结果（全语料 2977 窗，只 `CACHID` 候选 12792）**：两个方向都没有真改动 **12476**、只「之下」有变 260、只「之上」有变 **56**、上下都有 0、未判 0。
  那 56 个**全是我们自己造的窗**（aps250110 四窗 × 每窗 14 件，祖先每次都是同一个 `2013286654/8575 STRFLT [CACHID+POS]`）；与 S8 的 12734 = 12476 + 258 逐项对得上，
  多出的 58 全来自 S8 跑完之后我们写进语料的窗。9172 多算根的那 6 个库单跑：12497 个候选里「之上」**0**。
- **判读**：**Q6 的账（12734 候选 / 9172 多算根 / 四门）一格不动**——多算根是 `regen_A \ regen_B`、两态各跑一遍规划器的集合差，按构造就不读成因表；
  祖先真变了的话那个祖先自己是两态都在的种子，根落进 `regen_B`、不会被记成多算。12476 那一格反而**更硬**：标签可以从「子树内一个都没变」写成「上下两个方向都没有真改动」。
  被改写的只有 S8 的机制措辞，一个数字都没动——因为语料里的 `CACHID` 跳动全是整库扫写形状，没有一个是「有人编辑了某个祖先」带出来的。
  **往前看一条**：语料以后若收进真实设计编辑，只戳候选里会出现「子树顺带被盖」的一类，它们的根本来就要 regen、不该记成多算，成因表那时两个方向都要判。
- 文档：`docs/evidence/2026-09-16-q6-cachid-only-cause-direction.md`（按政策不入库）；计划 §8 Q6 补一段、S8 结论 (3) 的纠正句里加了「推不翻」的指向。
  验证：全语料一跑 152s（`output/e3d-s4/q6_ancestor_corpus.txt`）+ 六库一跑 14s（`q6_ancestor_six_dbs.txt`），两跑口径一致、与 S8 的 12476 逐字对上。生产代码零改动。

### 属性变化分类对齐 core · S4 E3D 造窗半程收口：负体排除在 db8000 造出的窗上坐实有内容；开孔造不出；P23 绕道 APS 自然 `STRFLT` 造出窗 C、两态相等 = 这形状上保守，S4 关闭

- **做了什么**（会话 fable-5-1-7 开、fable-5-1-20 接手收口；E3D 3.1 TTY 经 l3_suite `--check-driver` 跑 `scripts/e3d/s4_db8000_*.mac`，
  全部落在 scratch 库 db8000 = `ams8000_0001`，会话 607→619；探针 `probe_rigid_exclusion_windows` 用 `AIOS_PROJAMS_GEOMETRY_FILE` + `AIOS_TRANSFORM_WINDOW` 钉窗）：
  EQUI `/1-LNR-Q005-PJ` 下建 BOX 1000³ + 竖穿它的 NBOX 300×300×1400，只挪 NBOX 的 `POS` 两次，最后删掉——db8000 最新状态回到造窗前（45 BOX、无 `CODEX_S4_*`）。
- **负体在正体下——排除有内容 ✔**：窗 609→610（E 0 → E 200，孔仍整个在体内）探针判「负体（属主 CSG 消费）」、Δ 200 mm，两态相等但报「无判别力」。
  **不是生成器没扣**：两端 BOX 记录体积都是 9.09999e8（= 1e9 − 300²×1000）、亏格 1、32 三角——`elmodl::collect_negatives` 收直属负体成员——是通孔平移不改体积 / 三角数 / 亏格 / 包围盒，六个不变量全同。
  于是再挪一窗 614→615（E 200 → E 400，孔越出 +E 面成槛）：两态**不等、落在属主链** `24384/26484 BOX [triangles genus volume]`（体积 9.09999e8 → 9.24999e8、32 → 28 三角、亏格 1 → 0、窗内另改 0）。
  若把 NBOX 当合格锚点前移，BOX 记录原样不动就是错的——S3 那条负体排除有内容。
- **开孔在消费属主下——造不出**：SBFR 下 PANE + PLOO(HEIG 12) + 4 PAVE 都正常，CE = PANE 时 `NEW PLOPEN` 让 des.exe 0xC0000005 崩掉（分段试探 `s4_db8000_plopen_dryrun.mac` 把崩点钉在这一行；崩的那次不写会话、不落盘）。
  五个语料项目里 PLOPEN / WLOPEN / BPOPEN 零实例。`anchor_owner_exclusion` 今天只有单测 `a_consumed_member_anchor_regenerates_its_owner_unit` 护着。
- **P23——造不出「只动了摆放」的窗**：字典里 `HRPOST`（及 HRTERM / HRGATE / HRPANE / RAIL / KICKPL / CLNTIL）**没有 `POS` / `ORI`**（HR* 四个由 `POSREF` + `ZDIS` 派生，RAIL / KICKPL / CLNTIL 只有 `SPRE`），
  `is_placement_only` 对它们恒假；带 `POS` / `ORI` 的十个 P23 noun 在 db8000 从零建不出带几何的子树（PLTFRM 能建但拥有不了 SBFR / SCTN / GENSEC / FLOOR / PANE / FRMW；
  FIXTUR / FURNIT / CTSUPP / ELFITT 在 ZONE / STRU / FRMW / SBFR 下一律 "Cannot create … at this level"；STRFLT 真实实例之下是 ASL 应用生成的 STRSTR / TREADS / RPATH / HANDRA）。
  剩两条路要拍板：登录 APS 挪自然 STRFLT / CTSUPP（写进语料库，动 S6 / S7 / Q6 的分母），或接受「e3d-model 对 23 个 noun 都没有 builder、P23 排除按构造保守，只有单测护着」。
- **P23 改道——窗 C 造出来了，这形状上排除保守 ✔**（用户当日批了第一条路；会话 fable-5-1-20 跑 move 腿与首拍、opus-5-23 接手跑复原腿与位精确核对）：
  绕过 `l3_suite --check-driver`（它把 evar 钉在 AMS），直接 `run_ams_c_entrymacro.bat` + 自写 driver 宏（`L3_E3D_PROJECT=APS`），挪**语料库 `aps250110` 的自然实例** 2013286654/8575
  `STRFLT /B01_LEVEL_03_ACCESS/SF_001` `BY E 300` —— 窗 C = 32→33。探针判「P23 摆放派生 noun」、Δ 300 mm、锚点即根，**两态相等且有判别力**（锚点子树里两件 HANDRA 记录 984 三角 / 亏格 3，
  B 态全部由 Δ 前移，与 T 端整根重算逐个不变量相同）。读法：core 遇 P23 改 `POS` 会 `REPELE` 整件重建、我方左乘 Δ 也对，**S2 那条排除的代价是「少算」，方向安全；到今天既没有反例、也没有必须它不可的样本**。
- **造窗留痕已位精确复原**：复原腿 33→34（`BY W 300`，同一 binade 加减 300 位精确可逆）跑完 `Q POS` 回到 `W 302571mm N 320059mm D 1800mm`，与 move 腿开头打印的基线逐字相同。
  三层核过（`zz_tmp_s4_census SESDIFF`，`ScanTier::Full`）：32 / 33 / 34 三端都是 26187 件、零增删；32→34 动过的 16 件与 32→33 **是同一批**，`members` 逐字相同；
  STRFLT 记录 324 字节里 32→33 差 12 字节（页指针 8 + 属性区 2 + **位姿载荷 2**），32→34 只差 10 —— **位姿那两字节消失，回到基线原值**。留痕只剩 `CACHID` 与 append-only 的页指针 / 项目会话计数。
- **副产品：Q5 的第三票 + S8 措辞纠正**：窗 C 这一挪，E3D 给 STRFLT 自己**和它整棵子树里 14 个带 `CACHID` 的元素**（10 TREAD / 2 GENSEC / 2 HANDRA，连自己共 15 件）各盖了一次（hash 62869719），
  容器 STRSTR / TREADS / RPATH 不带这个字、一个没盖。**Q5 接线前 STRFLT 自己那个戳就会让 `is_placement_only` 判假、窗 C 根本进不了候选**；接线后才判成「只动了摆放」并两态全等——现场又一票。
  `CACHID` 因此**不只盖在「顶层件」上，改谁就盖谁整棵子树里每个带这个字的元素**（S8 结论 (3) 与计划 §3 的措辞已同步改）。
- **窗 C 的账与局限**：aps250110 会话链 32 → 36，引用「453 库 / 2961 窗」时这一库多 4 窗（APS 一腿只写一个 sesno，SAVEWORK 后不跟内务会话——「一腿两号」是 db8000 的脾气，不是通则）。
  其中 35 / 36 是**两个会话撞车**留下的：交接给 opus-5-23 之后 fable-5-1-20 仍在线、不知道有人接手，复原腿被各跑一遍（34 opus-5-23 20:12:07 正确复原；35 fable-5-1-20 20:13:44 又 `BY W 300`），
  fable-5-1-20 用会话页的 user / date / comment（e3d-io 例程 `zz_tmp_seslist`）看出 34 带同一条复原注释后，20:21:02 以 `s4_aps_strflt_fix.mac`（`BY E 300`）写 36 纠偏——
  36 的两件 HANDRA `world.t` 与 32 **逐位相同**，32↔36 显式属性只差 `CACHID`（→ `Integer(4)`）与项目 MNUM 计数（取证文档 §4.6）。两会话不在同一协同组、`send_to` 不通，撞车只能靠用户转达。
  探针侧复核 20:30 补跑成（那条在途线落地后 lib 又编得过）：钉 **32→36 只动了摆放的候选 0**——复原后的净效果对刚体探针是空操作；钉 33→34 / 34→35 / 35→36 三窗各判 P23、Δ 300 mm、两态相等（2 件 HANDRA 由 Δ 前移），与窗 C 同一结论。
- **探针读法一条**：负体 / 开孔类锚点自己没有记录，`moved` 恒为 0，两态相等时一律报「无判别力」——它分不清「属主没消费」和「不变量看不见那一挪」；判别力只能来自属主记录的不变量变化，造窗要挑会改体积 / 亏格 / 包围盒的挪法。探针代码未动。
- **E3D TTY 副产品**：不 SAVEWORK 的 QUIT 也写一条只改内务 / `CACHID` 戳的会话（612 / 613 / 614 / 617），崩掉的不写；每次 SAVEWORK 后紧跟一条内务会话；
  `DELETE BOX` 连带 NBOX 成员的整段收尾腿崩掉，先 `DELETE NBOX` 再 `DELETE BOX` 干净（`s4_db8000_cleanup.mac` 已改成这个顺序）。ams8000 会话链 607 → 619，引用「453 库 / 2961 窗」时 ams8000 多 12 窗。
- 文档：计划 §4 S4 行改 ✅、新增「★0916 S4 E3D 造窗结论」（(3) 已按窗 C 改写）、§8 Q5 补第三票、S8 结论 (3) 与 §3 的 `CACHID` 措辞纠正；
  `docs/evidence/2026-09-16-rigid-exclusion-e3d-windows.md`（按政策不入库）新增 §1.1 APS 会话账、§4.4 窗 C、§4.5 复原核对。
  验证：探针报告 `output/e3d-s4/probe_window_A_609_610.md` / `probe_window_A2_614_615.md` / `probe_window_C_aps250110_{32_33,32_36,33_34,34_35,35_36}.md`；
  临时探针 `zz_tmp_s4_gen` 逐端 `generate_subtree` 核了 db8000 六个会话的 BOX 记录不变量与 aps250110 32 / 33 / 34 / 35 / 36 的 HANDRA `world.t`；
  `zz_tmp_s4_census` 出会话差分 `sesdiff_aps250110_{32_33,33_34,34_35,32_36}.txt`；e3d-io 例程 `zz_tmp_seslist` 读会话页 user / date / comment 定的撞车责任——三个临时探针收尾时**都已删**（用法记在取证文档）。
  宏新增 `scripts/e3d/s4_aps_strflt_{move,restore,fix}{,_driver}.mac` 六个。生产代码零改动；未跑全量 `cargo test`（本轮没有可退步的东西）。

### ADR-078 D5：局部提交的回执由存储在写锁里盖——「应用 delta + 重算 `manifest_hash` + 盖回执」成为一个原子操作

- **章盖在 `ModelMemoryStore` 的写锁里。** `ScopeState::apply_upserts_and_receipt` 落完 upserts 后，
  `scope == Partial` 且带回执的提交由新加的 `ScopeState::stamp_partial_receipt` 按 `record_ids_under(root)`
  **此刻**的全部记录重算 `manifest_hash` / `geometry_count` 再入库——与 `apply_removals`（含
  `tube_branch_resets`）、upserts 在同一次 `write()` 之内。记录集合与 `root_projection_complete` 同一把尺子，
  盖出来的 `geometry_count` 正好是它随后要核的那个数。
- **为什么不能在锁外算**：`Partial` 的 `upserts` 只是这根的一角，整根的章只能在 delta 落进去之后算；
  调用方先读 `records_of_root` 再算再 `publish`，读与写之间另一批提交 / `evict_missing_root` / `clear_dbnum`
  都插得进来，章对的是一个已经不存在的版本，且没有一处报错。这就是上一棒（D2 收尾）点出的那条 TOCTOU。
- **配方仍是一份**：`e3d_model_service::manifest_hash_of`——`manifest_hash` 的借用形、同一个函数体，
  只是不要求记录连成一片（锁里的记录散在 `BTreeMap` 里，整根克隆一遍再喂切片就是白扛一次 O(n) 复制）。
  regen / transform / unit 三条路对同一批记录算出同一个章；`regeneration_and_rigid_motion_share_one_manifest_recipe`
  那道守卫照旧绿。
- **调用方的分工**：`Partial` 回执里 `manifest_hash` / `geometry_count` 是占位、会被覆盖；`source_sesno` /
  `model_target` / `published_via` 照调用方填的。`CommitScope::Partial` 与 `ProjectionCommit::receipt` 的文档
  写明了这一条；ADR-078 加实施约束 10。
- **`commit` / `try_commit_current` 改为返回实际入库的回执**（`Vec<(RefNo, RootReceipt)>`，按提交序）。
  顺手修了一处会静默咬人的地方：`model_write_behind::publish_commits` 原先在**提交前**从 `commits` 抄回执去登记
  durable 身份——对 `Partial` 就是拿占位值登记，ACK 永远对不上、这根永远 pending 且不报错。现改用返回值登记；
  非持久档的 `publish` 也从返回值取根。`apply_geometry_delta_on` 的 Historical 分支加了个分号吞掉返回值，
  别无改动。
- **`WholeRoot` 一字不动**（ADR-078 实施约束 9）：回执照调用方写的入库，存储不替它算。
  `cached_root_commit` 与「目标变了、清单没变」两条路的 `upserts` 是空的，替它们重算就是把完整投影盖成空章。
- **护栏四条**：`a_partial_receipt_is_stamped_from_the_root_as_it_stands_after_the_delta`（占位不入库；
  章 == 对提交后 `records_of_root` 跑配方；数 == 记录数；与不经存储、直接对 `[a, c']` 算的逐字相同；
  返回值 == 入库值；`source_sesno` / `published_via` 照调用方）；`a_whole_root_receipt_is_stored_exactly_as_the_caller_wrote_it`；
  `a_partial_receipt_never_describes_a_different_version_than_its_records`（写者 2000 次 `Partial` 提交，
  读者每一次 `root_snapshot` 读到的章与记录都对得上——「原子」二字就钉在这条上）；write-behind 侧
  `a_partial_publish_registers_the_receipt_the_store_actually_stamped`（登记的 `RootIdentity` == 存储盖完的回执，
  不是递进来的占位）。把 stamp 关掉后三条针对性测试全红（mutation 核过），不是空转。
- 验证：gen-model `cargo test --lib` **2138 绿 / 0 红 / 114 ignored**，e3d-model **320 绿 / 0 红**；
  `rustfmt --edition 2024 --check` 三个改到的文件逐个跑，我改到的每一段都干净，剩下的差异按行号对过 HEAD
  （`model_memory_store.rs` 4 处、`model_write_behind.rs` 7 处，HEAD 里就有）全是旧的；三个源文件行尾 LF，
  与 HEAD 一致未翻；changelog / ADR 全 CRLF。
- **仍然没有打开任何行为**：生产里没有任何一处构造 `Partial` 提交，D5 只是把「盖章」这一步搬进了锁里。
  D5 那笔 O(根内记录数) 的 digest 账没变（省的只是克隆），Q1 照旧挂着。下一步是 D4 的幂等闸
  （搬 ADR-066 `republish_gate`）或 D6 / 实施约束 4 的管段完整性门。

### ADR-078 D2 / A2：`CommitScope::{WholeRoot, Partial}` 落地——但不变量不在存储里，在上游那条「缺席 ⇒ 删除」的做差上

- **动手时发现初稿把不变量放错了层。** 存储（`ScopeState::apply_removals`）**本来就只删**
  `ProjectionCommit::removals` 与 `tube_branch_resets` 点名的那些，从来不拿缺席当删除。
  「未提及 = 删除」这条活在**上游**：`generate_refs` 拿 `上一版有的 \ 这一版生成的` 算 `removals`，
  而那条推导只有在 `upserts` 确实是这根全集时才成立。初稿设计的「存储照 `CommitScope` 决定删不删」
  是个不会生效的开关。ADR 的 D2 与实施约束 2 已就地改口。
- **`CommitScope` 仍然加了**（`model_memory_store.rs`），职责改成两条：让一笔提交自己说得清它是
  整根全集还是局部，以及给那条做差一个能被机器钉住的前提。带 `#[serde(default)]`、默认 `WholeRoot`
  ——那正是今天的语义，也是磁盘上那些没有这一格的旧 `model_artifact` 应有的读法。
  初稿写的「必填、不给默认值」随之作废：这一格既然不决定删除，给默认值反而是对的。
- **真正的护栏放在推导发生的地方**：做差抽成 `e3d_model_service::whole_root_removals`，
  函数文档写明它是整条发布链路上唯一一处拿缺席推删除的地方；`include_str!` 守卫
  `only_whole_root_removals_derives_a_deletion_from_absence` 钉住生产代码里只有这一处，
  且 `prior[…]` 只剩「喂给它做差」与「判空」两个用处——多出第三处就红。
- **三条行为护栏**：`a_partial_commit_only_removes_what_it_names`（一根三件只重建一件，另两件
  逐字不动；显式点名才删得掉）；`a_partial_commit_that_resets_a_branch_still_clears_every_segment_of_it`
  （钉住 D6 那条**相反**的规矩真实存在——整段重写一碰就清光那条分支的全部管身，局部提交漏带一段
  就是静默丢管）；`a_commit_serialized_before_the_scope_field_reads_back_as_whole_root`。
- 验证：gen-model `cargo test --lib` **2134 绿 / 0 红 / 114 ignored**，e3d-model **320 绿 / 0 红**；
  `rustfmt --edition 2024 --check` 四个改到的文件逐个跑，剩下的差异按行号与 HEAD 对过**全是旧的**，
  我改到的每一段都干净；四个文件行尾 LF，与 HEAD 一致未翻。
- **仍然没有打开任何行为**：今天所有生产提交都是 `WholeRoot`，`Partial` 只有测试在构造；
  局部生成、幂等闸、`model_unit_level_publish` 都还没有。

### ADR-078 A1：投影记录与根回执的对账从 `==` 放宽成 `<=`——记录带的是自己最后一次生成的会话，根推进到哪了看回执

- **改的是两处，不是三处。** `model_db_adapter.rs` 的 `RootShare::load` 与 `persist_root_impl` 里那条
  `record.source_sesno == receipt.source_sesno && record.ancestors.contains(&packed_root)` 改判 `<=`；
  越界那一半（祖先链里没有这个根）一字未动。**比回执新的记录仍然报错**——那意味着有人把几何投进来却没盖章，
  这根的全集就不是回执说的那一份。
- **为什么要动它**：单元级发布（ADR-078）只重建变了的那几件，没动的记录仍停在上一次真正生成它的会话上，
  而回执整根前移到 T——落盘那一刻就撞这条等号，现场只看得见一句 "projection contains records outside its
  receipt scope"。顺带解释了一件旧事：**ADR-066 刚体前移明明不重算网格，为什么仍要把整根记录全部 upsert 一遍**
  ——它就是在绕这一条。它省的是生成，不是写入。
- **ADR-078 初稿 D3 列了三处，动手时核掉一处**：`room_resident.rs` 的
  `row.source_sesno == source.target_sesno()` **不该改**。它校的是 `generate_frozen_roots` 的产出，
  而产出永远来自一次新鲜生成、每条都盖着 `batch.source_sesno`，根本不经过投影里那些没动的旧记录；
  放宽它只是白削一道还成立的检查。`room_model.rs` 读活投影、本来就是 `<=`，也不动。ADR 的 D3 与 A1 已改口。
- **护栏**：新增 `a_record_may_lag_its_root_receipt_but_never_lead_it`——陈旧记录（回执 42、记录 41）放行，
  超前记录（回执 42、记录 43）拦下**且逐字核错误文本**（只判 `is_err()` 分不出它是被 "incomplete" 那条拦的
  还是被 "outside its receipt scope" 拦的），被拦的那轮库里一行不变。
- 验证：gen-model `cargo test --lib` **2129 绿 / 0 红 / 114 ignored**，e3d-model **320 绿 / 0 红**；
  `rustfmt --edition 2024 --check src/fast_model/model_db_adapter.rs` 干净；该文件行尾 LF，与 HEAD 一致未翻。
- **本条不打开任何行为**：`CommitScope`、局部清单、`model_unit_level_publish` 一样都还没有，这一步只是
  把那道等号从单元级发布的必经之路上挪开（ADR-078 实施约束 1：先改语义，后接执行体）。

### 属性变化分类对齐 core · S7 收口：图元级重建先量出 86.5%（真·重算口径），§8 Q4 关闭，单元级发布立 ADR-078（草案）

- **S7 探针**（`window_root_plan::tests::probe_unit_level_rebuild_ratio`，`#[ignore]` 手动，工作树内未提交，作者协同组 dchc 的 opus-5-74）：
  沿 ams8000_0001 整条会话链逐窗走生产口径选根，对 `plan.regen` 里的每一根**两端各 `generate_subtree` 一次**，逐件按 `GeometryId` 配对、
  比 `ParityInvariants`（与 S4 对拍门同一把尺子：noun / 体积 / 三角数 / 亏格 / 包围盒 / 世界矩阵，旋转绝对 1e-6、其余相对 1e-6，不比网格顶点）。
  计划里原写的「manifest 逐条比」是草稿口径，实际没有比 manifest 文本。**只量不接线**：回执未加 `unchanged_unit_ratio`，`WindowRootPlan` 一字未动。
- **数**：606 窗 / 0 跳过 / 退化窗 4；regen 根 6122（3153 落在退化窗里），跨名单等距抽实量 978 根（16%），进统计 792 根、T 端单元 3672 件、
  一字没变 2442 件、生成失败 0。三轮重跑跨 65 分钟、跨 Q5 接线，聚合数逐个一致——探针确定性，且 Q5 没扰动选根。
- **结论对分母敏感**（这是本条的要点，口径纠错见 `docs/evidence/2026-09-16-unit-level-rebuild-measure-addendum.md`）：真·重算（稳态窗 ∧ S 端已有这根）
  488 根 / 2670 单元 / 按单元加权 **86.5% 过线**；再剔整根白算 233 根 / 1440 单元 / **75.0% 过线**；稳态窗含新根 721 根 / 3366 单元 / 68.6% 不过；
  全部合计 792 根 / 3672 单元 / 66.5% 不过。分歧只在**新根（275 个）算不算进分母**——占比按定义恒 0，而 core 遇新根也走 `REPELE` 整件建、
  `SEGREP` 本来就省不了它，故不进分母。**引用这个数必须带口径。**
- **形状是双峰**：488 根里 0% 档 116 根、100% 档 255 根，中间只 117 根。按单元过线、**按根不过线**（中位数 50.0%、均值 40.4%，>70% 的根只有 84/233）；
  收益几乎全集中在 `BRAN`（380 根扛 2812 单元、75.9%，而 `EQUI` 256 根才 273 单元）。收益模型按单元算，按根数拍会拍高。
- **顺带量到一笔不属于 S7 的账**：整根白算 255/488 根，按单元占真·重算总量 **1230/2670（46.1%）**——一件都没变却被整根重算。那是选根的题
  （S6 / 计划 §8 Q6），单元级发布在这些根上一个图元都不碰。它比 S7 能省的还大，另开一条线。
- **计划 §8 Q4 关闭**：阈值 70% 没动，动的是分母；按真·重算口径过线。用户 2026-09-16 拍板立 ADR。
- **新增 `docs/adr/ADR-078-unit-level-publish-rebuilds-only-changed-units-under-a-root-receipt.md`（草案）**——ADR-056 **D3 末句**留的那个口子
  （「单元级落库（`execute_plan` 直写）另立 ADR」），也是 D3 的第二个例外（第一个是 ADR-066）。九条决策，核心三点：
  ① 单元级的**判定与生成早就现成**（`plan_update` / `execute_plan` / `apply_geometry_delta` / `ProjectionCommit` 记录级 / 房间 overlay 记录级），
  缺的是「局部清单怎么在一个根级回执之下安全落地」；② `ProjectionCommit` 加 `CommitScope::{WholeRoot, Partial}`，`Partial` 下**未提及 ≠ 删除**
  （今天靠 `prior \ generated` 推 removals，直接喂局部产出的后果是静默删件而非报错）；③ 第一道硬墙是 `persist_root_impl` 的
  `record.source_sesno == receipt.source_sesno` 断言——局部提交后没动的记录还停在旧会话，落盘即撞（**这也解释了 ADR-066 刚体路为什么不重算网格却仍整根 upsert**）；
  决策是把记录上的 `source_sesno` 改读「最后一次真正生成的会话」、三处断言 `==` 放宽成 `<=`（`model_db_adapter.rs:629` / `:1498`、`room_resident.rs:1013`）。
  回执仍是根级、提交后从投影重算 `manifest_hash`（`published_via` 加第三档 `"unit"`）；不新增锁，幂等闸照搬 ADR-066 `republish_gate`；
  任何一件失败整根回退 regen；开关 `model_unit_level_publish` 默认关，开门要过全语料两态对拍 + 净收益为正。
  **有意留的两处半程**：D5 的整根 digest 重算是 O(根内记录数)、D7 的 durable 落盘第一版仍按整根刷——两处都进验收门与开放问题，不当遗漏。
- 本条只动文档：计划头部状态 / S7 行 / §6 不做 / §8 Q4 四处补账，changelog 这一条，新 ADR 一份。**代码零改动**，探针本身仍是别人工作树里未提交的改动。

### 属性变化分类对齐 core · Q5 接线：`is_placement_only` 把 `CACHID` 剔出比较集——`[POS, CACHID]` 算「只动了摆放」，全语料合格锚点 79 → 163、对拍 139 根 108 个有判别力的两态全等 0 红；只 `CACHID` 变照旧整根 regen

- e3d-model `src/element_diff.rs`：新增 `pub const PLACEMENT_STAMP_ATTRIBUTES: [&str; 1] = ["CACHID"]`（**戳**：Design 应用随每次图形重建写下的缓存
  `+1` 计数，不是几何输入——S8 普查 453 库 2961 窗，13724 次跳动全是 `0→1` / `n→n+1`），`is_placement_only` 由「`!attributes.is_empty()` ∧ 全部 ∈
  `PLACEMENT_ATTRIBUTES`」改判「**至少一项** ∈ `PLACEMENT_ATTRIBUTES` ∧ 其余只允许戳」。`any` 在空集合上为假，所以「什么都没动」与「**只有戳变了**」
  照旧回 `false`——摆放一项都没动就不是只动了摆放，那是整库缓存重建的扫写，照旧上卷重算（要不要另给它开一档是计划 §8 Q6，不在本条）。
- 边界：`is_placement_only` 其余四条排除（换类型 / 改挂 / 成员表 / `opaque`）、`placement_input_changed`（戳不是矩阵输入，不触发级联）、
  `transform.rs` 的 `PLACEMENT_ATTRIBUTES`、`options.rs` 的 `model_transform_cheap_path` 默认值（仍默认关）、gen-model `metadata_only_tier`
  （字典没给 `CACHID` 投票，元数据门照旧不放行）一律未动。
- 读取集合按**有名字的排除口径**处理，不把字面量藏到扫描够不着的地方——名字一消失，守卫就钉不住「它是戳、不是几何输入」这件事：`output/gen_read_set.py`
  加 `STAMP_LITERALS = {"CACHID"}`，分类时单列进产物的 `stamp_literals`、**永不进任何 `names`**；例外写在脚本里——真被传进 e3d-io 读取口
  （`.get_integer("CACHID")`）的照旧算读、照旧进 `names`。四条守卫钉住：e3d-model `tests/attribute_reads_guard.rs`（产物 `stamp_literals` ≡
  `element_diff::PLACEMENT_STAMP_ATTRIBUTES`，且名单不许养闲人——源码里已不提的戳要摘掉），gen-model `src/data_interface/model_impact.rs` 三条
  （与常量逐名相等、两仓产物逐名相等、戳不许拿到「生成器读它」那一票）。两仓产物 `vendor/e3d-model/data/attribute-read-set.json` 与
  `src/data_interface/generation_read_set.json` 的 `names` 里都没有 `CACHID`。
- **复验**（接线后**不带** `AIOS_RIGID_PROBE_IGNORE_CACHID` 全语料重跑对拍门，五个目录 2961 窗 / 跳过 0 / 488s，报告
  `docs/evidence/2026-09-16-rigid-exclusion-parity.md` 按政策不入库）：rigid 合格锚点候选 **163**（= 接线前 79 + S4 剔掉 `CACHID` 多出的 84，
  整台 EQUI 挪位为主），对拍 139 根——两态相等 **108**、无判别力 22、**不等·锚点 0**、不等·窗内另改 9（这一窗根里另有改动，生产按 D1 规则 4 整根
  regen，样本不作数）；三条排除仍有内容：规则 M 结构件之下 62 候选 / 不等·锚点 2，负体 50 / 2，路由容器之内 519 / 5。报告里「仅
  `IGNORE_CACHID=1` 才多出的候选」= 0，即排除已固化进 `is_placement_only`，环境变量再无额外增量。
- 验证：真值表单测（`element_diff` 9 绿）覆盖 `[POS]`✓ `[ORI, CACHID]`✓ `[POS, CACHID]`✓ `[POS, ORI, CACHID]`✓ / `[CACHID]`✗ `[POS, NAME]`✗
  `[POS, HEIG]`✗ `[POS, CACHID, HEIG]`✗ `[CACHID, NAME]`✗ 换类型·改挂·成员表·`opaque`✗ 与空集合✗，`placement_input_changed` 旧用例全绿；两仓
  `cargo test --lib` 不退（e3d-model 320 绿 / 0 红，gen-model 2127 绿 / 0 红 / 113 ignored）；e3d-model `cargo test --test increment_real
  an_increment_says_what_two_full_generations_say` 1 绿（1380.70s）；`cargo test --test attribute_reads_guard` 2 绿、gen-model
  `cargo test --lib model_impact` 48 绿；ams8000 九窗 `live_ams8000_pinned_windows_land_in_the_expected_buckets` 与
  `live_ams8000_nine_pinned_windows_are_identical_with_the_cascade_on_and_off` 各 1 绿（桶位不变）；改动文件 `rustfmt --edition 2024 --check`
  无差异，文档 CRLF / 源码原有换行符保持。未跑服务、未提交。
- 文档：ADR-066 D1 规则 2 加 2026-09-16 修订注（判「摆放之外还动了别的」时忽略戳、只 `CACHID` 变照旧 regen、不接的代价与两票出处）；
  计划 §8 Q5 从「第二票已到、剩拍板、本会话只记不动」改口为「已拍板并接线」并记上复验数字。

### 属性变化分类对齐 core · S8：`CACHID` 一致性普查——DESI 全语料 453 库 2961 窗三维交叉表；它是「顶层件图形缓存 `+1` 计数」不是「本元素几何变了」，单向 oracle 98.9% 成立、反向 2.7%；只作结论，不接线

- 新探针 `window_root_plan::tests::probe_cachid_consistency`（`#[ignore]` 手动；与全语料两态门同一份五个项目目录，`AIOS_POSREF_DIRS` /
  `AIOS_CACHID_PROBE_STEPS_PER_DB` / `AIOS_CACHID_PROBE_REPORT` 可改）：对每个真变了的 Modified 候选记三维——**`CACHID` 四态**（变 / 有值未变 /
  0·unset 未变（没人写过）/ noun 无此属性）× **除 `CACHID` 之外的变更类别**（换类型改挂 / 含 DCHC 4 / 1·2 / 3 / 字典外 / 只 0 / 只成员表 /
  只 CACHID / opaque，取最重一档）× **我方判定**（`change_metadata_tier` 放行 = skip、`is_placement_only` ∧ `rigid_anchor` 判刚 = transform、
  其余 regen；按候选判、不问开关）。另判「只 CACHID」的成因（同窗真变 / 新建 / 删除的元素有没有以它为严格祖先的）、「忽略 CACHID 再判一次」的
  判定位移、只 DCHC=0 / 只字典外候选上的属性名 × 四态。报告 `docs/evidence/2026-09-16-cachid-consistency-corpus.md`（按政策不入库），耗时 142s。
  落点从计划里写的 e3d-model `tests/increment_real.rs` 改到 gen-model `window_root_plan` 测试模块——DCHC 字典、两票门、`rigid_anchor` 三样都只在
  gen-model 一侧，e3d-model 里放不下「DCHC 类别 × 我方判定」两维（计划 §4 S8 记了）。
- **结论**（取证文档 §4.4 末条 / 计划 §4 S8 ★块）：(1) 值是**计数**不是旗标：13724 次变化全是 `+1`（`0→1` 5241、`n→n+1` 8483），`WORD` 原始字就是
  计数，`e3d_collector.rs` 记的 `"" → "1"` 其实是 `0 → 1`；(2) 写方按应用分：带 `CACHID` 的 26627 个候选里只有 13753 个非 0，写过的只有
  `GENSEC HANG PANE BRAN SUBE EQUI SCTN HANDRA TRNCUR STRFLT GENPRI PNOD SUBS FLOOR TMPL STRU TRNPRI` 十七个 noun，船体 / `CABLE` / 栏杆件 /
  `CWBRAN` / `VOLM` 上一次没写过；(3) 语义是**顶层件的图形缓存版本**：13724 次跳动里 369（2.7%）伴随本元素 DCHC≠0 / 结构变化，621（4.5%）伴随
  只 DCHC=0 / 字典外 / 成员表变化（成员增删 132/132 跳动——子树图形变记在属主身上），**12734（92.8%）只有它变**，其中 12476 连子树里都没有同窗
  变化，形状是整库扫写（aps7343 6→7 全部 SCTN / PNOD / GENSEC `0→1`，GENSEC 2794 个 `0→1` 再 2794 个 `1→2`，HANG 1266 × 3 轮）；(4) 单向 oracle
  成立：有值元素上「有 DCHC≠0 变 ⇒ CACHID 变」369/373 = **98.9%**（4 个反例全是 BRAN 头尾类别 1/2），没写过的元素（213 个 DCHC≠0 变化停在 0）是
  覆盖不是纪律；反向「CACHID 变 ⇒ 几何变了」2.7%，不能当信号；(5) 副作用：字典没收它，混进差分就既不放行也不刚——有值元素上 88 次含 `POS`/`ORI`
  的变化全部同时跳动（`is_placement_only` 为假 → regen，84 次若忽略它本可 transform），只 DCHC=0 的 83 次跳动里 67 次从 skip 推成 regen
  （`LCHKDA` 81/83、`HREF` 4/5、`TREF` 1/1、`NAME` 2/25），12734 个只 CACHID 的候选今天全部上卷重算。两处待拍板记进计划 §8 **Q5**
  （`is_placement_only` 要不要剔掉 `CACHID`——推荐倾向剔、但等 S4 对拍）/ **Q6**（只 CACHID 要不要放进门——推荐先不动、先量）。
- 顺带：字典外名字 `SPAMAP` 单独出现在 14289 个真变了的候选上（PANE / GENSEC / SCTN，占 36162 的 40%），有值元素上 406/406 与 `CACHID` 同跳，
  今天全部 regen；`MAPEXS` 1161 / `MAPCSV` 1027 / `UPDT` 967 / `UPGDNO` 321 / `RATM` `RREF` `RATS` 各 ~195 / `DBMVAL` 190 都在无 `CACHID` 的
  noun 上。不在 S8 范围，记下备查。
- 验证：`cargo test --lib --no-run` 干净（新代码无 warning）；探针先在 ams000 目录链末 60 步冒烟（292 库 1246 窗 52s）通过，再全语料跑两遍
  （第二遍加成因 / 位移 / 属性名表）：453 库 / 2961 窗 / 0 跳过 / 2503710 索引候选 / 36162 真变了的 Modified 候选，与 S6 全语料门的库、窗、候选数
  逐字相同；`rustfmt --edition 2024 --check window_root_plan.rs` 只剩 1233 / 2234 两处旧差异；CRLF 保持。未改生产代码、未跑服务。

### 属性变化分类对齐 core · S4（离线半程）：刚体前移两态对拍门建成——全语料 710 个只动摆放的候选，合格锚点 27 个有判别力的根两态全等（0 红），负体 / 规则 M / 路由容器三条排除坐实有内容；剔掉 `CACHID` 再判多出 84 个整台 EQUI 挪位、108 根全等（Q5 第二票）；P23 / 开孔 / CWBRAN 等七条排除语料里无自然窗，等 E3D 造窗

- 新探针 `window_root_plan::tests::probe_rigid_exclusion_windows`（`#[ignore]`；ADR-066 验收 1 的对拍门，落点同 S8 改到 gen-model——排除判决
  `rigid_anchor` 只在这一侧）：沿五个目录每一窗找只动了摆放的候选（`ElementDiff::is_placement_only`），逐个过 `rigid_anchor` 分类（合格 / 十种排除），
  再**按生成根归组**（一根里所有只动了摆放的候选一起前移，与生产「兄弟锚点各自 Δ、祖孙取最内层」同口径），合格锚点的根全拍、被排除的每类抽 10 个
  有判别力的根：A = e3d-model `generate_subtree` 在 T 上整根重算，B = 在 S 上整根生成、每件记录左乘最近锚点的 `Δ = W_T·W_S⁻¹`，逐件比 `GeometryId`
  集合与体积 / 三角数 / 亏格 / 包围盒 / 世界矩阵（1e-6）。不一致的记录按位置归四处——子树内 / 属主链（消费锚点的属主）/ 邻件（窗内未改，被锚点带动）/
  窗内另改过（自己、祖先或后代在这一窗改过——生产按 D1 规则 4 整根 regen，样本不作数）；相等但锚点子树无生成物 = 无判别力；`Δ` 是 |t| < 1e-3 mm
  的纯平移 = 重写 POS 的末位噪声，只数不拍。目录库经 `DirectoryDbResolver` 扫五个目录按需补开。`AIOS_PROJAMS_GEOMETRY_FILE` + `AIOS_TRANSFORM_WINDOW`
  钉造窗；`AIOS_RIGID_PROBE_IGNORE_CACHID=1` 把 `CACHID` 剔出比较集再判；`AIOS_RIGID_PROBE_STRICT=1` 合格锚点一个不等就红；`AIOS_RIGID_PROBE_DUMP=1`
  逐条印不一致记录的两端矩阵 / 体积 / 包围盒与 L2 差分。报告 `docs/evidence/2026-09-16-rigid-exclusion-parity.md`（+ `-ignore-cachid.md`，按政策不入库）。
- **结果**（453 库 / 2961 窗，359s）：只动摆放的候选 710 = 合格 79 / 路由容器之内 519 / 规则 M 62（FIXING 51、POINSP 11）/ 负体 50（NCYL）；
  P23、开孔在消费属主下、CWBRAN、点环容器、桥架件、剖面、几何集**零个自然窗**；51 个 `Δ≈0`。
  - **门**：合格锚点 56 根——27 个有判别力的两态全等（HANG 下 BWLD / CLEV / EYRD / HROD / SCLA / SLUG / TRNB / VSPR、SUBS 下 CYLI、SVOLM 锚点即根、
    acw5525 四个 EQUI 各 2–3 个 NOZZ 同根各自 Δ），**0 红**；22 根锚点子树无生成物（ams7997 HVAC 分支的 BEND / ATTA / DAMP、ams8000 的 ELCONN / STRT）、
    7 根窗内另有改动。
  - **排除有内容**：负体——同根 5 个 / 2 个 NCYL 挪 200 mm，属主 FLOOR 三角 / 亏格 / 体积变（2 干净样本）；规则 M——7 个 FIXING、2 个 POINSP 挪位，
    属主 GENSEC 体积 / 包围盒 / 亏格变（2）；路由容器——VALV 挪 500 mm 管身增删、ELBO 挪 25 mm 两根管身变、GASK 转向管身增删（5）；保守的形状也看见了：
    全是显式 FTUB、没有隐式管身的分支里挪 BEND、单独挪 FTUB，Δ 前移也对（5）。
  - **Q5 第二票**：剔掉 `CACHID` 多出 84 个合格锚点（整台 EQUI 挪位为主、锚点即根、1–30 件全部由 Δ 前移，另有 FLOOR 一例），对拍 139 根，
    108 个有判别力的全等、**0 红**。E3D 真实的 EQUI 挪位就是刚体，`CACHID` 只是同一次编辑写下的缓存计数——计划 §8 Q5 推荐从「倾向剔、等 S4」
    改成「第二票已到，剩拍板」；本会话不动 e3d-model `is_placement_only`（接线）。
  - 中途两次改口都写进了探针：单锚点拍多锚点根会把兄弟锚点的记录判成「不等」（acw5525 EQUI 里 7 个图元同挪）→ 按根归组；只查「自己 / 祖先改过」
    会把后代改过的属主（POINSP 挪了的 GENSEC）当成锚点带动的邻件 → 后代也算；剔 `CACHID` 首跑 3 个「红」全是子树里另有 `HEIG` 改动的 CYLI / NOZZ
    （生产按 D1 规则 4 本就不前移）→ 被挪的记录也过「窗内另改」判定。改口后两遍全语料 0 红。
- 计划 §4 S4 标 ◐（离线半程）、§8 Q5 补第二票；ADR-066 验收 1 加 2026-09-16 修订注；取证文档 P1-c 加 ★。**未做**：E3D 造窗那一半（`HRPOST` /
  `PLOPEN` / `NBOX` 等七条排除）；对拍是文件侧生成物的不变量，不是生产投影记录的 `record_id` / `mesh_id`（ADR-066 验收 4 的 `mem://` 真跑另算）。
- 验证：`cargo test --lib --no-run` 干净；探针 ams000 冒烟后全语料跑五遍（三次改口 + 两种口径），最后两遍 359s / 451s 全过；`cargo test --lib`
  **2127 绿 / 0 红 / 112 ignored**（+1 ignored = 新探针）；`rustfmt --edition 2024 --check window_root_plan.rs` 只剩 1233 / 2234 两处旧差异；
  CRLF 保持。未改生产代码、未跑服务。

## 2026-09-15

### 属性变化分类对齐 core · S6：元数据门改两票——「字典 DCHC=0 ∧ 生成器不读」替掉五个名字，`no_model` 分 `metadata_only` / `unread_attribute` 两档，DESI 全语料两态门 453 库 2961 窗全过

- `model_impact::attribute_is_metadata_only(name)` 改为 `raw_dchc_code(name) == Some(0) && !generation_reads_attribute(name)`（两票缺一不放行；
  S6 之前是效果表 DataOnly ∧ DCHC=0 的 `NAME DESC PURP FUNC LCHKDA`）。老名单退成 `attribute_is_curated_metadata`（效果表 DataOnly ∧ DCHC=0，
  恰好五个、按构造是新门的子集），只作报表分类与分档；`metadata_only_tier(names)` 是唯一一份名单 + 分档：全部放行才有档，全在老名单记
  `MetadataOnly`，否则 `UnreadAttribute`。字典 3941 个 DCHC=0 的名字里读取集合挡 `DIR FILL GTYP PKEY UKEY VXREF` 六个，其余 3935 个放行
  （守卫钉 3941 − 6）；UDA / 字典没收的名字（含 `CACHID`）/ `opaque` 照旧不放行。
- **两票压过手写表**：五张手写表里登记为几何 / 级联输入、却字典 DCHC=0 且生成器不读的名字恰好 **16** 个——`DEPENDENCY_CASCADE` 的
  `HREF TREF PSPE DKEY DDPR FSPREF SCREF STYP CURD JFRE PTYP` 与 `DIRECT_GEOMETRY` 的 `SIZE MAXA NGMR ORRF PTOF`；只改它们 → Skip，表项不动
  （建边资格、报表分类照旧），守卫 `the_two_votes_override_the_hand_tables_for_exactly_sixteen_names` 逐个点名。取证文档 §1.4「13 + 6 个 DCHC=0
  却判影响模型的是我们的几何输入」被 S5 的扫描推翻一半：真读的只有 `GTYP PKEY VXREF`。
- **预览同口径**（0908 纪律）：`classify_operation_effects` 逐属性**先**问 `attribute_is_metadata_only`，放行的记 `DataOnly` 不排工作，哪怕手写表
  登记在别处；`query_service::change_impact` 的 `action` / `affects_model` 由两票说（`HREF` → skip，`CACHID` / `DIR` → regen），回执多
  `dchc` / `generator_reads` 两格；`manual_update::merge_net_change_details_with` 不动（本来就问 `classify_operation_impact`）。
- **e3d-model `increment.rs`**：`plan_update_with` 的谓词从 `Fn(&ElementDiff) -> bool` 改成 `-> Option<NoModelTier>`；`IncrementReport` 多
  `unread_attribute` 一格（与 `metadata_only` 互斥、都含在 `no_model` 里），`totals_line` 跟着多一格，`accounts_for` 钉两档之和 ≤ `no_model`；
  `plan_update_bounded` 传 `|_| None`。gen-model `window_root_plan::change_metadata_tier(diff)`（四条结构条件 ∧ `metadata_only_tier`）接进
  `WindowRootSources::collect`，`change_is_metadata_only` 退成布尔包装（源码钉）；回执 warning 两档分列。开关 `model_skip_metadata_only_changes`
  名字不变、语义变宽（计划 §8 Q2 按推荐），`options.rs` 文档与 CONTEXT.md「模型影响」词条改口。
- 守卫：`metadata_only_needs_dchc_zero_and_an_unread_attribute`（真值表 + 放行数 = 3941 − 6）、
  `the_metadata_tier_is_curated_only_when_every_name_is_on_the_old_list`、`the_two_votes_override_the_hand_tables_for_exactly_sixteen_names`、
  `the_skip_verdict_uses_the_same_double_vote_as_the_window_metadata_gate`（扩到新档）、
  `query_service::the_impact_preview_skips_exactly_what_the_metadata_gate_admits`、`manual_update` 口径测试扩 `DUTY` / `HREF` / `DIR`；
  e3d-model `the_two_gate_tiers_are_a_subset_of_no_model_and_both_show_on_the_totals_line`。
- **两态门**：三条契约抽成 `assert_metadata_gate_two_state`，ams8000 单库门（`live_ams8000_metadata_only_gate_never_adds_and_never_shrinks_the_closure`，
  `AIOS_PROJAMS_GEOMETRY_FILE` + `AIOS_METADATA_ONLY_WINDOW=base:target` 可钉任意一库一窗）与新的 DESI 全语料门
  `live_corpus_metadata_only_gate_two_state_over_every_desi_db`（五个项目目录每个 DESI 库整条会话链，`AIOS_POSREF_DIRS` / `AIOS_METADATA_GATE_STEPS_PER_DB`
  可改）共用；后者顺带出「DCHC=0 属性变化频次表」写到 `docs/evidence/2026-09-15-metadata-gate-two-vote-corpus.md`（按政策不入库）。
- **全语料门第一次跑撞出契约 2 要改口**（2026-09-16，会话 fable-5-1-44 接手 fable-5-1-40）：aps250514 3→4 / aps250517 3→4 两窗各两个桥架分支候选只改
  `HCREF` / `TCREF`（DCHC=0、生成器不读，新档放行），关着时 e3d-model 桥架分支**粗级联**（`cascade_cable_tray_branch`：CWBRAN 子树里改了任何东西就整条
  重建，按 noun 不按属性判）拖进 20 / 27 个 CTRAY，闭包多 42 / 56 个 refno（CTMTRL / CTSTRA / CTTEE / CTRAY）；开着时不上卷不级联，那片单元连同祖先就不进
  闭包。原文「两态祖先闭包逐字相同」在 ams8000 链上成立只是因为那条链没有这种窗；这是两票该省的（core 不为 `HCREF` 重画、`cable_tray.rs` 也不读它，
  托盘网格不可能变），不是漏收。契约 2 改成「闭包只少不多 ∧ 账本种子两态逐字相同（候选与祖先链照进闭包，候选所在的根一个都不少收）∧ 少掉的 refno
  全是少重建的单元或其祖先」，少了别的照红；回执 / 报告多一格 `closure_dropped`。0908「凭证前移那一层不因此少收」相应改口为「候选所在的根不少收；
  粗级联扇出的根按两票前移」（`increment.rs` / `options.rs` / `change_metadata_tier` 文档），计划 D2 ★S6 (5) 与取证文档 P2.3 记下。
- 验证：gen-model `cargo test --lib` **2127 绿 / 0 红 / 110 ignored**（+3，+1 ignored）；e3d-model `cargo test --lib` 318 绿（+1）、
  `--test attribute_reads_guard` 2 绿（`reference_reads_guard` 那条红仍是别人在途的 `catalogue.rs` 改动，未动）；`rustfmt --edition 2024 --check`
  本次改动无差异（`window_root_plan.rs` / `options.rs` / `manual_update.rs` / `query_service.rs` 剩下的差异用 HEAD 版逐一核过都是旧的，未动）。
  真库：ams8000 九个钉死窗桶位不变、`regen` 两态逐字相同（3/1/2/0/1/1/1/1/0）；ams7999 45→46 仍 regen 2 / no_model 3；ams8000 链末 40 步两态门全过
  （无一窗只改了不影响模型的属性）。**DESI 全语料两态门**：453 库 / 2961 窗 / 0 跳过，每窗三条契约全过，耗时 48 分；36162 个真变了的 Modified 候选里
  门放行 **1057**（`metadata_only` 750 / `unread_attribute` 307），134 窗有放行；`regenerate` 1007907 → 1007782、连派生容器共省 **515** 个单元；
  闭包少收只在上述两窗（98 个 refno）。频次表头部：`NAME` 799（放行 774）、`LCHKDA` 276（4，几乎总与别的改动同候选）、`AMEMRF` 112 / `LNFA` 88 /
  `HREF` 68（42）/ `TREF` 66（46）/ `DESC` 32（24）/ `AELRFA` `PPCUTL` 各 19 / `INCO` 13 / `ASSTAR` 12 / `HCON` 12（4）/ `PPMODI` 10 / `BENDMC` 9 /
  `TCON` 21、`STACOM` 9、`VALREF` 9（都 0，与别的改动同候选）；读取集合里的六个 DCHC=0 名字在语料里一次都没变过。未跑服务。

### 属性变化分类对齐 core · S5：生成读取集合成为受护栏的产物——138 个名字、DCHC=0 恰好六个，元数据门第二票的料备齐

- 新生成脚本 `output/gen_read_set.py`：按一条**宽口径**规则扫生成器源码里的属性读点——全部大写字面量 `"[A-Z][A-Z0-9]{1,7}"` ∩
  （字典 4270 ∪ 运行库 schema 4478）∪ 直接传进 e3d-io 读取口（`.get_*("X")` / `.attribute("X")`）的字面量；每文件截到第一个
  cfg(test) 标记，`src/bin` 除外；noun 名自动放行，其余大写字面量记成 `non_attribute_literals` 让守卫分清「新冒出的」与「早知道不是属性的」。
  间接读点（`get_double(name)`）不必逐处登记：名字总来自某处字面量。多收只少跳一次，漏收是一张停在旧形状上的网格。
- 两个仓各一份产物 + 守卫：e3d-model `data/attribute-read-set.json`（**133** 名 + 44 个非属性字面量；新模块 `src/attribute_reads.rs`
  暴露 `attribute_read_set()` / `reads_attribute()`；`tests/attribute_reads_guard.rs` 用逐字相同的规则重扫、与产物逐名相等才绿，另钉直接读点一个不漏；
  `category::dictionary_noun_names()` 新增给守卫用）。gen-model `src/data_interface/generation_read_set.json`（`e3d_model` 原样一份 + `gen_model` 模型面
  11 个模块的 **8** 名 `PARA CFRA CREF JOIS JOIE PFRE STAREF ENDREF` + 判决用并集 **138** 名），`model_impact::generation_read_set()` /
  `generation_reads_attribute()` 嵌入；三条守卫：`the_generation_read_set_mirrors_e3d_model_and_the_model_side_modules`（两仓产物相等、
  模块重扫相等、并集恰好）、`the_e3d_io_catalogue_evaluation_reads_are_documented_but_not_a_vote`、
  `the_generation_read_set_pins_its_dchc_zero_members_and_excludes_the_metadata_five`。
- 集合里 DCHC=0 的成员逐个点名：`DIR FILL GTYP PKEY UKEY VXREF`（全来自 e3d-model：catalogue_point / section / route / udet / polyhedron），
  gen-model 侧 0 个；老五个元数据名 `NAME DESC PURP FUNC LCHKDA` 一个都不在——S6「`NAME` 等仍放行」的前提已钉。字典没有却被直接读的伪属性 `CPAR` 收进集合。
- 计划没提、扫出来的第三处读点：**e3d-io 目录求值环境**（`db_param_env.rs` / `record/catalogue_eval.rs` / `record/catalogue_expr.rs`）替生成器读
  设计侧 `ANGL RADI HEIG DESP UNIPAR ISPE HBOR TBOR PBOR` 与目录侧 `DATA.PURP/DKEY/NUMB/PPRO/DPRO`、`SELE.QUES/ANSW/MAXA/TANS`（23 名）。产物里单列、
  `vote: false`：设计侧那几个 DCHC≠0 或已在 e3d-model 一侧，DCHC=0 的七个（`ANSW DKEY MAXA PSPE PURP QUES TANS`）全在目录元素上读，而元数据门只跑
  DESI 窗口（`plan_window_roots` 非 DESI 返回空计划）。守卫把这七个钉死，名单变了就得重新回答「新名字是在设计元素上读的吗」。
- 验证：gen-model `cargo test --lib` **2124 绿 / 0 红 / 109 ignored**（+3）；e3d-model `cargo test --lib` 317 绿（+1）、`--test attribute_reads_guard`
  2 绿；`rustfmt --edition 2024 --check` 本次改动的文件干净。**顺带发现**：e3d-model `tests/reference_reads_guard.rs::indirect_reference_reads_are_each_registered_with_their_names`
  在本轮之前就红（`catalogue.rs`/`catalogue_point.rs` 的 `entry`、`rpath.rs` 的 `name` 三处间接读点没登记、`cable_tray.rs name` 那处已不存在）——
  stash 掉本轮改动复现，属别人在途的 `catalogue.rs`/`elmodl.rs`/`udet.rs` 改动，未动。判决未换（S6 才换），生产行为逐位不变。未跑服务。

### 属性变化分类对齐 core · S3：刚体前移补「被属主消费的几何」与 `EVALAT` 规则 M 两条排除——G2 正确性缺口关闭

- `window_root_plan::rigid_anchor` 的 noun 级判决全部抽成纯函数 `anchor_lineage_exclusion(refno, noun, lineage)`，自下而上三层：
  (a) 候选自己（`anchor_noun_exclusion`）：桥架件、P23 之外新加**负体**（`Category::is_negative`，属主正体拿它做 CSG，core 规则 J）、
  **剖面 / 路径数据**（`ProfileData`：`PLOO PAVE LOOP VERT SPINE CURVE RPATH POINTR`，core 规则 S）、**几何集成员**；
  (b) 直接属主（`anchor_owner_exclusion`）：候选**不是模型单元**而属主 `consumes_members()`（正体 / 目录件 / 复合件 / `TMPL`）——
  `PLOPEN`/`WLOPEN`/`BPOPEN` 开孔在 PANE / WALL / BPANEL 下、`CMPF` 负体包、分类表没认领的 noun 在正体下；单元（BOX 在 TMPL 下、NOZZ 在
  EQUI 下）与 `List` 属主下的非单元（SUBE、ELCONN）不受影响；
  (c) 严格祖先：`CWBRAN` / 点环容器（原有）+ **规则 M 结构件** `PANE GENSEC SCTN BPANEL`（新）——型钢 / 板上的附件挪位 = 结构件重建。
  路由容器那条仍要读元素（`derives_implied_tubes`），留在 `rigid_anchor` 里、在 noun 级判决之后。
- 先核了 `category` 再写（计划 §7 的提醒）：**`FIXING` 是模型单元**（09-14 起 GMRE），计划 D1-b「不是独立单元 ∧ 祖先含结构件」那条接不住它，
  规则 M 改成对任何锚点看严格祖先、结构件自己挪位不在此列；`PLOPEN`/`WLOPEN`/`BPOPEN` 带 `IPFCOM`、被判 `Composite(Structural)` 非单元，
  D1-b 把 `Composite` 当纯容器是错的，改成看属主 `consumes_members()`；`PLOO` 的 gb 是 0，`PANE → PLOO → PAVE` 那条老的「gb==1 容器之下」
  接不住，剖面数据改按自身 noun 排除。三处都记进计划 D1 与 ADR-066 修订注。
- 守卫：`a_consumed_member_anchor_regenerates_its_owner_unit`（先断言 category 口径：FIXING 单元 / TMPL、NBOX、PLOPEN、PAVE 非单元、
  PANE / WALL / BPANEL 消费成员；三族纯函数逐名；NBOX 在 BOX 下 / PLOPEN 在 PANE 下 / FIXING 在 SCTN 下三形状经 `classify_transform_roots`
  都回 regen、BOX 在 EQUI 下照旧 transform）、`a_pure_container_anchor_still_qualifies`（ZONE / SITE / STRU / FRMW / PIPE / EQUI / SUBE / TMPL /
  BOX / CYLI / NOZZ 各自谱系放行；挪 ZONE 名下两根随同一锚点前移、无 rejected）；S2 那条把 `NBOX` 从「自身不排除」名单移到 S3。
  `rigid_motion::CandidateKind::Anchor` 文档同步。ADR-066 D1 规则 3 修订注补第五类；取证文档 §6.2 G2 标 ✅；计划 §4 S3 ✅。
- 验证：`cargo test --lib` 2121 绿 / 0 红 / 109 ignored（+2）；`rustfmt --edition 2024 --check` 本次改动无差异（`window_root_plan.rs` 两处旧差异未动）。
  真库：九个钉死窗桶位不变、`regen` 两态逐字相同；ams7999 45→46 仍 regen 2 / no_model 3。`model_transform_cheap_path` 仍默认关（D5：
  S4 对拍样本过了再翻），生产行为逐位不变。未跑服务。

### 属性变化分类对齐 core · S2：刚体前移补 P23 排除——core `UPGRPH` 对 23 个「几何由摆放派生」的 noun 位姿变化也整件重建

- e3d-model `category.rs` 新增 `PLACEMENT_DERIVED_NOUNS`（23 个：`DRAW CLNTIL STRFLT INFITT PLTFRM ELFITT STRLNG HVACFI RAIL DOOR
  WINDOW FPFITT LINDIM FLRCOV MLABEL FIXTUR HRPOST HRTERM HRGATE HRPANE KICKPL CTSUPP FURNIT`，Core3D `change/UPGRPH` `0x1022EAEE`
  内联常量逆 hash 解出）与 `is_placement_derived(noun)`。字典里 23 个全是 primitive，14 个带 `IPFCOM`、`CTSUPP` 带 `ICABCO`。
  守卫 `placement_derived_nouns_are_exactly_the_twenty_three_from_upgrph`：逐个点名、无重复、每个都在 3.1 字典里、大小写 / 空白
  不敏感，`HANDRA` 容器 / 设备 / 型材 / 桥架件 / 负体 / 开孔不在其中。
- `window_root_plan::rigid_anchor`：候选自身 noun 的排除抽成纯函数 `anchor_noun_exclusion(refno, noun)`——桥架件（原有）+ P23（新）
  ——再沿 T 端祖先查路由容器 / 点环容器 / `CWBRAN`（不变）。P23 的原因文本 `placement-derived noun (…) … (P23, 0x1022EAEE)` 原样进
  `CandidateKind::Anchor { rigid: Err }`，回执 `transform_fallback_reason` 带出。守卫
  `a_placement_derived_anchor_never_qualifies_for_rigid_motion`：HRPOST（锚点即根）/ DOOR（锚点在 WALL 根之下）/ CTSUPP（锚点即根，
  与正常 EQUI 锚点并列）三形状经 `classify_transform_roots` 都回 regen、只有 EQUI 走便宜路；开关关着时无 transform 也无 rejected。
- ADR-066 D1 规则 3 落修订注（第四类排除 = P23；第五类「被属主消费的几何 / 规则 M」归 S3）；取证文档 §6.2 G1 标 ✅；
  计划 §4 S2 ✅。`model_transform_cheap_path` 仍默认关（D5：S3 补齐 + 对拍门过了再翻）。
- 验证：gen-model `cargo test --lib` 2119 绿 / 0 红 / 109 ignored（新增 1）；e3d-model `cargo test --lib` 316 绿（新增 1）；
  两处 `rustfmt --edition 2024 --check` 本次改动无差异（`window_root_plan.rs` 1120 / 2121 两处是 HEAD 就有的旧差异，未动）。
  真库：`live_ams8000_pinned_windows_land_in_the_expected_buckets` 九窗桶位不变；`live_ams8000_nine_pinned_windows_are_identical_with_the_cascade_on_and_off`
  九窗 `regen` 两态逐字相同；`live_ams7999_45_to_46_…` 仍 regen 2 / no_model 3。开关默认关，生产行为逐位不变。未跑服务。

### 属性变化分类对齐 core · S1：字典 DCHC 第六张效果表消掉 213 个 `Unknown`、`PLCF` 随 DCHC 导出、三处取证改正

- `src/data_interface/model_impact.rs` 新增第六张效果表 `dictionary_change_class_attr_names()`（嵌入
  `dictionary_change_class_attr_names.json`，由 `output/gen_dchc_fixture.py` **生成**，不手抄）：字典 DCHC ∈ {1,2,4} 而五张手写表
  一个都没登记的 **217** 个名字——DCHC=1/2 的 `HZAXI LHEA LTAI TZAXI` 与 18 个 schema `ELEMENT` 引用（`STAREF ENDREF JOIS JOIE CFRA
  PFRE CRFA HPREF TPREF POSREF …`）登记 `DependencyCascade`，其余 195 个 `DirectGeometry`。**判决不变**：`Unknown` 与 `DirectGeometry`
  都是 `Regen`；引用名此前经 A2 升级也是 `DependencyCascade`，建边资格逐名不变；direct 窗口规划器只问 `attribute_is_metadata_only`
  （未动）。只让回执 / `change_impact` 不再把 core 明说「会重画」的属性显示成「未知属性」。守卫：
  `dictionary_change_class_table_covers_every_unregistered_nonzero_attribute`（与快照两向差集为空、217 条、两侧严格升序）、
  `dictionary_change_class_table_maps_to_its_declared_effects_and_actions`、
  `dictionary_change_class_table_routes_schema_references_to_the_cascade_side`（带 / 不带 meta 对 217 个名字同答）；
  `attribute_effect_tables_have_no_duplicate_names` 扩到第六张。
- `PLCF`（字典字段 652066，`change/IDCHNG` 读 DCHC 之后读的第二个字段，`== 1` 才置 INTUBE 旗标）随 DCHC 同批导出为
  `src/data_interface/plcf_flags.json`：58 个置位——位姿 `POS ORI BFORI`、54 个 DCHC=4（`SPRE HEIG PARA DESP CREF JOIS JOIE CFRA PFRE
  POSS POSE NPOS YDIR ZDIR PX PY PZ` 与楼梯 / 栏杆 / 桥架尺寸 …）、一个 DCHC=0 的 `CLGREF`；头尾连通的 DCHC=1/2 一个不置。
  `model_impact::raw_plcf(name)` 三态；**只作报表**，`plcf_is_report_only_and_matches_the_dictionary_snapshot` 钉住本文件之外无调用方。
- `output/gen_dchc_fixture.py`：字典导出找不到 `output/noun_attr_fields.json` 时回退到 `../vendor/e3d-io/schema/e3d31/catalog/` 的同一份
  （DCHC 快照重生成后与入库版逐字节相同）；顺手修正它读运行库 schema 的那一段——`all_attr_info.json` 里 `named_attr_info_map` 在盘上
  是空的（aios_core 载入时才填），改读 `noun_attr_info_map` + `output/noun_layout.json`，此前脚本的「dead-entry 复算」一直在空集合上跑
  （现在算出的 11 / 37 个 unmatched 与 `curated_tables_are_reconciled_against_the_runtime_schema` 钉的名单一致）。
- 三处取证改正 + 一条词条：核对表 R5 加注「core 这一层没有 TransformOnly」只对 DRAFT 侧成立，3D 侧 `change/UPGRPH` 类别 3 =
  `css/STRTRA`/`SEGTRA` 只改段矩阵；0906 `IDCHNG` 行改成 INTUBE 旗标读 `PLCF`（该证据档按政策不入库，从 `gen-model-p3b-verify`
  快照取回 `docs/evidence/` 本地副本后改）；0831 上下文 §4「`changesBetweenSessions` 完全不看 CACHID」改成只对 `DDES_GraphicsMngr`
  路成立、`DES_DrawListManager::changesBetweenSessions` 的 `DES_DrawListCompare::attChange` 只看 CACHID；CONTEXT.md「模型影响」
  词条补三档的 core 对应物（类别 0 / 类别 3 `STRTRA`·`SEGTRA` / 类别 4 `REPELE`·`SEGREP`）与「GETWORK 路 core 不分属性」。
  开发计划 `docs/plans/2026-09-15-attribute-change-class-alignment-dev-plan.md` 标已批、S1 ✅，D3 记下「引用名归级联侧」这一处收紧。
- 验证：`cargo test --lib model_impact` 43 绿；`manual_update` 建边资格 / C-REF 上下界 8 绿；全量 `cargo test --lib` 2118 绿 / 0 红 /
  109 ignored；`rustfmt --edition 2024 --check` 干净。真库：`live_ams8000_pinned_windows_land_in_the_expected_buckets` 九窗桶位不变，
  `live_ams8000_nine_pinned_windows_are_identical_with_the_cascade_on_and_off` 九窗 `regen` 两态逐字相同，
  `live_ams7999_45_to_46_plans_the_new_stru_roots_without_the_legacy_recheck` 仍 regen 2 / no_model 3。未跑服务。

### ida-bridge 取证：core 的 3D 侧确有「只动方位」路径（`change/UPGRPH` 类别 3 → `css/STRTRA`/`SEGTRA` 只改段矩阵），三类属性答案表与刚体前移开门前置

- 新文档 `docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md`：用 `core.dll` = `idalib-33060` /
  `Core3D.dll` = `idalib-49136` 把「哪些属性触发几何重算 / 只动方位 / 不影响模型」按 core 的三条通路列全——
  **会话内交互路**按字典 DCHC 四档走 `EVALAT` → QCHGLS → **`change/UPGRPH`（`0x1022EAEE`）**：类别 3（`POS`/`ORI`/`BFORI`）
  只改图形段矩阵（顶层件 `css/STRTRA`、图元 `css/SEGTRA`、容器 `change/TRASCN` 走子树 TOPF 逐个改矩阵），例外是 23 个
  「几何由摆放派生」的 noun（`DOOR WINDOW HRPOST … CTSUPP`，逆 hash 解出）与管件两侧隐含管段；类别 4 整件 `add/REPELE`
  （`TOPDEL` + `ADDDES`）或**单图元** `change/SADREP` → `css/SEGREP`，目录件 `REPALL`；类别 0（3941 个属性）什么都不做。
  **GETWORK 路**（我们的场景）`DB_RawChanges` 只走 `DB_IndexTableCompare`，记录被重写即整件重画、不分属性；
  **图形缓存路** `hasTopLevelGraphicsChanged`（逐指令）与 `DES_DrawListCompare::attChange` 只认 `CACHID`，
  `include*`/`reorder*` 回调全空。核对表 R5「core 这一层没有 TransformOnly」只对 DRAFT 侧成立。
- 两处改正既有取证：`IDCHNG` 的 INTUBE 旗标读的是第二个字典字段 `PLCF`（652066），不是 `DCHC == 1`；
  「`changesBetweenSessions` 完全不看 CACHID」只对 `DDES_GraphicsMngr` 那条路成立。
- 与现状对照（§6）：刚体前移（ADR-066，默认关）缺 core 类别 3 的两族例外——P23 名单、被属主消费的负体 / 开孔 /
  `PANE·GENSEC·SCTN·BPANEL` 成员（规则 M/J/P/R）——后者是正确性缺口，开门前必补（P1）；「不影响模型」只放行 5 个名字、
  core 会话内是 3941 个，e3d-model 实际只读 131 个运行库属性（DCHC=0 的 6 个：`DIR FILL GTYP PKEY UKEY VXREF`），
  建议以「DCHC=0 ∧ 不在生成读取集合」为第二票扩门（P2）；213 个 DCHC=4 属性我方未登记、落 `Unknown`（行为对、报表噪音，P0）。
- 只写文档、未改代码、未跑服务；伪码级结论在 §8 逐条标「未逐指令核」。
### core 属性修改 → 模型增量规则取证补全：`putAtt` 到 `descases/VDESPT` 的入口链、DCHC 门之外的四条属性专项规则、QCHGLS 全部写入者（ida-bridge，只改文档）

- 起因：用户 2026-09-16 要求「用 ida-bridge 继续分析完善属性修改导致的模型增量更新的规则、继续完善文档」。0915 那份取证
  （`docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md`，此前只在 `gen-model-refactor` 工作树）把会话内交互路画到
  `descases/VDESPT` 就停了：没说 C++ 的属性 put 怎么落到它头上、它除 DCHC 门还做什么、QCHGLS 除 `EVALAT` 还有谁写。
- 落地（E3D 3.1，`core.dll` = `idalib-33060`、`Core3D.dll` = `idalib-49136`）：0915 文档取到本工作树（§0–§8 原样、头注说明出处），**新增 §9**：
  - §9.1 入口链：`DB_Element::putAtt`（全部重载）→ `internalPutAtt`（**同值 put 直接返回、不发事件**；ref 版写前先发 `PostSetRefAttribute`）→ 写 →
    `DB_Element::postSetAttribute`（受影响集 = 克隆属性 ? `DB_Clone::getRelatedElements` : 自身；逐个 `DB_ElementChangesPlugger` vtbl+52 `PostSetAttribute`、NAME 另发 +68、
    受控 UDA 复验、`evaluateRules` 规则引擎）→ 订阅者 **`DESDRA_SCPlugs`**（`Init` 一次订阅 12 个变更钩子 + 16 个 Allowed）按库类型分流：PADD → `dra_scase/*`，其它 → `descases/*`。
    十二个钩子逐个落到哪个 `descases` 例程、怎么推 QCHGLS 列成一张表；`EVALAT` 8 处调用 5 个函数闭合；入口门第二项 `sub_1050DCA0` = `cacheGetworkChanges` 旗标。
  - §9.2 `VDESPT` 全文（DCHC 门与 `PLOO.HEIG` 分支逐指令）：**DCHC=0 在 `VDESPT` 就挡住、也不进空间地图包围盒清单 `MLSADD`**；规则 U `PLOO.HEIG` → 当 owner 的变化评估；
    规则 V `SJOI.JFRE` 翻真 → 位置差折进 `DELP` 写回；规则 W `COCO.CTYP` → 不匹配的 `COCORE` 置空；规则 X `DATA.PPRO/DPRO` → `SCRUSE`；自动着色 `colsel/AUTCO2` 换画列表；`designEvents` 事件。
  - §9.3 QCHGLS 全部写入者（`EVALCD` 21 处 9 个函数）：属性触发的只有 `VDESPT → EVALAT` 一条；`VDESAC` / `VDESRO` / `VDESR1`（拷贝 / 排序，直接 `EVALCD(·,4)` 不经规则表；
    `BRAN` 内排序另记前一成员）、`descat/ATTABK` 与 `rac/RACKO` 两个命令处理器、`EVALRF`、`UpdateChangeList`。`maplist` = core 的空间地图（`mapboxlib/MPBOX`）包围盒维护清单，
    `MLSADD` 按 noun 字段 **`IMAP`（847458）** 路由——就是我方快照 `noun_caps.json` 的 `spatial_map` 列（0906 「无消费者」的那个字段到此有主）。
  - §9.4 `change/UPGRPH` 分派骨架逐指令核（312 条 cmp/j/test/call）：`cmp 3` / `cmp 4` 分派、**P23 的 23 个常量逐个逆 hash 与 §4 名单同集同序**、三级分叉与每叉调用目标全部对上 §4 伪码 → §8 那条结案。
  - §9.5 `change/IDCHNG` 全函数逐指令：两次 `ATAINT`（DCHC / PLCF）与旗标对上 §3.2；边角：第二次不检错且出参共用一格（字典外属性早退，实际碰不到）。
  - §9.6 `PLCDEL` 已读：清 Core3D 一张 100 槽、按 `(DB_Ref, int)` 键、存 3 个 double 的缓存表；`VDESPT` 入口无条件清一次。「p-line 缓存」是四条腿撑的推断，`PLCF` 仍只作报表。
  - §9.7 ref 属性的另一半：`VDESPF` / `VDESFA` → `structures/BAKREF` / `BREAKF` 维护 back-ref（0007 学习记录「订阅者尚未定位」结案）；`SPCO.PRTREF` 设一次拷七个引用；
    `PSARFA PSLRFA ALERFA` 不建 back-ref。
  - §9.8 差距表续 G13–G20：同值早退 ≙ `unchanged`；规则 U ≙ loop 容器上溯；写回类规则解释「一次操作多属性同窗」、不需模拟；`MLSADD` 与 DCHC 门同一个 `if` 给 Skip 档不动 AABB 一票；
    会话内路对 CATA 元素也进 `EVALAT`（R1 不受影响）。**§5 三张答案表的判词一条没变。**
  - 同步：`docs/specs/core3d-partial-update-conformance.md` R5 补上 0915 那段「只对 DRAFT 侧成立」（本工作树此前缺）与 0916 一句；`output/show-me-attribute-increment.html`
    加第 07 节「对着 core 看」（三条路 × 属性判别力 × 我方对应，`VDESPT` 门外做的事，core 三档 vs 我方三档）。
- 验证：全部地址本轮活桥实读（复查命令在文档附录）；无代码改动，`cargo` 未跑。**未验**：`internalPutAtt` 只读了 ref 版一个重载，其余九个重载「同值早退 / 事件顺序」是推定；
  `maplist` 的「空间地图包围盒」是从 `mapboxlib` 埋点名推的；`sub_1023190B` 模块码没对到模块名。

### 新增 `GET /api/v1/spatial/centerline`：一条 BRAN 的中心线线段表原样给前端——管-管间距改按「两条 BRAN 平行直段的中心距」算（计划 §3.3 ④）

- 起因：用户纠正「管-管净距」的口径——「两个排管的距离测量是指两条 BRAN 的直段之间，有平行的部分就标注它们的间距」。此前
  前端 `measure_pipe_to_pipe` 拿的是两个网格采样出的最近点对（任意两个构件表面最近处），在管-管场景下答的不是这个问题。
  前端要做这件事缺的只有一样：两条 BRAN 的**真实中心线**（隐式管身在网格里没有 refno、ELBO 的到达→离开是弦），而后端 P1 已经
  拼出了这份走廊（`spatial_centerline::BranCenterline`），只是没有一条路由把线段表原样交出去。
- 后端（`fast_model::spatial_centerline` + `handlers::spatial_centerline` + 路由）：`GET /api/v1/spatial/centerline?refno=a_b`
  回 `{refno, dbnum, segment_count, outside_diameter_mm, centerline_bbox, segments[], warnings}`，`segments` 按成员序、每段
  `{refno, order, noun, implicit, start, end, length_mm, outside_diameter_mm}`（E3D 世界 mm，与 `/nearest-clearance` 的
  `annotation` 端点同一坐标系）：隐式管身 `implicit=true` / `noun=TUBI`，成员段带自己的 noun（大写；没给 → `UNKNOWN`，不猜）与外径。
  走廊取法与 `prepare_centerline` 抽成同一个 `query_branch`（`spawn_blocking` 读盘；解析失败 400、库里没有 404、不是 BRAN /
  一段都拼不出 422，与 §4.13.1 同映射）。**只读库、不碰空间树**——没有 503 `spatial_not_ready` 一档，库没 ensure 过也答得出。
  路由钉 `spatial_routes_are_read_only_get_endpoints` 收进第五条。契约进 spec §4.13.3。
- 前端（`plant3d-web`，本仓外）：`genModelV1Api.genModelV1SpatialCenterline`；`measure_pipe_to_pipe` 改成「点两根管 → 各自所属
  BRAN → 取两条中心线 → 连续共线的段合成直段（ELBO / BEND 不算直段）→ 配平行对（夹角 ≤ 0.5°、沿轴投影有重叠）→ 每对一条
  中心距尺寸，落在重叠区中点、两端都在轴线上」，写进 Dock「BRAN 中心线最近清距」同一份结果（每对一条候选，可多条同目标；
  来源登记 `parallel-centerline / exact-centerline`，不带「≈」），表里给中心距、重叠长度与扣两侧外径后的净距。
  旧的网格采样管-管路径与最后一处「暂不创建尺寸」Toast 一起删掉。顺手修了 `instanceMapping.buildNounIndex`：真机 `model/records`
  里 BRAN 的隐式管身挂在 BRAN 自己的 refno 上、`generic=TUBI`，照字面收会让整条 BRAN 下构件的 `owner_noun` 变成 `TUBI`，
  点选认属主全部落空——`TUBI` 记录现在只说明「这个 refno 是一条 BRAN」。
- 验证：隔离树 `cargo test --lib spatial_centerline` 5 passed（含新用例 `centerline_response_carries_nouns_and_diameters_per_segment`：
  段序 / 隐式管身标记 / 成员 noun 与外径 / 穿过件不出现 / 没给 noun → UNKNOWN）、路由钉 1 passed；主树此刻因别的会话在途的
  vendor `e3d-model` 签名变更编不过，本条与那处无关（提交的树在只加一处不提交垫片的隔离 worktree 里编译验证）。
  真机：隔离树构建的 0.1.27 起在 `:8023`（`embedded-mem`），`/centerline` 对 ZONE 24381_144870 下 165 条 BRAN 全部 200；
  浏览器 e2e（:3111）点 `24381_145018` 与 `24381_144924` 各一根管：6 对平行直段、6 条中心距尺寸（1584 / 1920 / 1980 / 2719 /
  1920 / 6829 mm）画在两条轴线之间，反着再点同一对整组替换仍 6 条（2026-09-17）。
- 未动：`/nearest-clearance` 的量距口径（D4）、legacy 后端；服务端不算平行——几何留在前端（理由见 spec §4.13.3）。

### `/api/v1/spatial/*` 接上 BRAN 真实中心线：`nearby` 加 `source_mode=bran_centerline`（P1）、新增 `GET /api/v1/spatial/nearest-clearance`（P2）

- 起因：「指定 BRAN → 沿真实中心线找周边构件 / 找最近墙柱并给出可画尺寸的两个端点」此前三段会话把后端改动全落在了 legacy
  `plant-model-gen`（`/api/sqlite-spatial/*`），而前端 `plant3d-web` 用的后端是本仓（`/api/v1/spatial/*`，候选来自进程内
  `GLOBAL_AABB_TREE`，没有 SurrealDB / `tubi_relate`）。用户 2026-09-16 纠正后按 `docs/plans/2026-09-16-bran-centerline-nearest-clearance-v1-dev-plan.md`
  重新对准本仓落地；契约进 spec §4.13（新 §4.13.1 / §4.13.2）。legacy 那一路今后不再动。
- **P1**（`fast_model::spatial_centerline` 新文件 + `spatial_query` + `handlers::prepare_centerline`）：`nearby` / `nearby/refnos` 多一格
  `source_mode=aabb|bran_centerline`（缺省 `aabb`，只配 `refno`）。中心线来自 `mbd::branch_query`——MBD 管道尺寸用的同一份成员序
  （到达 / 离开点，含按 E3D 规则合成的隐式管身 `a_b~c_d`），每成员一段、穿过件不成段；handler 在树**读锁之前** `spawn_blocking` 预取
  （读库不压在树锁下）。候选逐段「段盒外扩 r」探针取、按 refno 去重，距离是整条走廊到候选盒的精确最小距离（线段↔AABB 最近点对，
  从 legacy 移植、单测钉住）；自身 = 投影子树 ∪ `branch_query` 成员。响应 `center.source = "bran_centerline"`、多 `source{kind,refno,
  segment_count,centerline_bbox,outside_diameter_mm}` 与非空时的 `warnings`，非中心线响应一字不变。错误：找不到 refno 404、不是 BRAN /
  一段都拼不出 422（与 `/api/mbd/v2/pipe` 同映射）。
- **P2**（`fast_model::spatial_clearance` 新文件，handler 只做身份 / 取服务 / 状态码）：`GET /api/v1/spatial/nearest-clearance`，
  与 legacy 同名同形（入参名、`wall` / `column` 预置组、`group_by=target_groups|noun`、`exclude_nouns`、`noun_counts`、
  `excluded_self_members`、`resolved_filters`），前端类型一字不改；两点不同——失败走状态码 + 信封（成功体仍 `success:true`），
  多 `surface=1`（距离扣 `outside_diameter/2`、起点推到管外表面，只在中心线模式生效，`distance_method` 相应变
  `centerline_surface_aabb_clearance_mm`）。缺省 `source_mode=bran_centerline`、`radius=5000`、`max_per_group=1`（钳 1..100）、
  **`include_self=false`**；`target_groups` 分桶下两种目标都不给 → 400（不补 `wall,column`，与 legacy 同）；`noun` 分桶不给目标 =
  半径内全部类型、负几何默认剔。借 `/nearby` 的 `scan_global_tree` 取候选，NOUN / 库号 / 自身过滤全在锁外本模块做（要计数）；
  每条候选带 `nearest{source_segment_refno,order,source_point,target_point,vector}` 与 `annotation{start_point,end_point,label_mm}`，
  前端不再算、直接画。`target_groups` 空桶保留 + warning；`noun` 桶序按最近距离。路由钉 `spatial_routes_are_read_only_get_endpoints`
  收进第四条；`spatial_clearance.rs` 自带「不碰 `GLOBAL_AABB_TREE`」源码钉（被禁字面量用 `concat!` 拼，免得 `aabb_tree.rs`
  的写点白名单整文件 `contains` 误判——第一版就撞过这一条红）。
- 前端（`plant3d-web`，本仓外、未提交）：`genModelV1Api.genModelV1SpatialNearestClearance`；`useSpatialCompute` 按数据源分流
  （legacy 打旧后端同名路由，v1 打这条），请求口径 `group_by=noun&max_per_group=3&exclude_nouns=WELD,ATTA`，Dock 的目标 chips 改成
  由 `noun_counts` 驱动的类型 facet、每类默认标注最近 1 条；`GEN_MODEL_V1_SPATIAL_CAPABILITIES.branCenterline` 翻 `true`。
- 验证：`cargo test --lib` **2151 passed / 0 failed / 111 ignored**（含 `spatial_clearance` 9 条：参数解析与拒收、每组最近 + 两端点、
  空桶保留、`group_by=noun` 分桶与桶序、自身剔除与 `include_self=true` 反例、`surface` 扣径 + aabb 模式下的忽略、盒到盒量距、
  `dbnums` / `same_dbnum` 限定与答不出时出声、不碰全局树源码钉；`spatial_query` 中心线走廊 / 自身并集 / cube-sphere 用例；
  `spatial_centerline` 段↔盒最近点纯函数用例）；`cargo clippy --lib --tests` 改动文件零告警；rustfmt 干净。
  **未 live**：计划 §5 那两条真机 GET（`24381_145018`）没跑——起服务是长驻进程。
- 未动：legacy `plant-model-gen`（D1 留 / 回退未拍板）；SCTN / GENSEC 按姿态判柱（P4）；`surface` 缺省（D4）仍是 `false`；
  精算到目标网格留给前端。

### room_membership 真机跑通 + 六条「未 live」验收逐条落实（S0–S4 的 live 收尾）

- 起因：S0–S4（D8–D13，源码钉 10–18）每一刀结尾都挂着「未 live：源码钉 14 / 16 / 18 的六条 mem:// 真测都没跑」。
  用户 2026-09-16 拍板：在 ams 上开 room_membership 跑一遍，把这六条（缺 DESI / 缺面板几何 / 文件前进 / 改 PANE）补上。
- 落实（证据 `docs/evidence/2026-09-16-room-membership-live-and-real-file-acceptance.md`）：
  - `cargo test --lib` **2142 passed / 0 failed / 111 ignored**，覆盖 (a) 缺面板几何、(d) 前进无房间 DESI 代号不变、
    (e) 改 PANE 整库重算、(f) 只改 BRAN 不触发这四条合成 mem:// / 单元验收。
  - 三条真 AMS /ALL 文件用例（`--ignored`，暖 meshes 缓存、bundled e3d31 schema）全过：(b) `ams_missing_desi_degrades…`
    ——缺 SCB 6003 → 常驻 `Degraded` 不判死；(c) `ams_task_start_refresh…`——陈旧键 → `Rebuilt`（215 房/230 面板/0 缺）→
    再探 `Unchanged` 23ms；§9 硬验收 `ams_cached_room_vertex_optimization_parity`——1112 6171 条 `room_relate` 优化路 == 参照路逐边相等、0 失败。
  - 真机 `serve`（dev 二进制、`store_mode=mem` 隔离库不碰任何真库、`room_membership=1`、真 /ALL）从空库把 dbnum 1112 生成一遍：
    常驻 215 房/230 面板/0 缺、全 29 个 MDB DESI 到位；`POST /dbnums/1112/model/ensure` 18/18 根 succeeded、
    **`[model-persist] room_edges=12342 … failed_roots=0`**；`/health` `status=ok / room=available / degraded=[]`、
    `e3d.room.lookup` `calculation_status=computed`。
- 边界：真机那台是读透 mem 库，没摄入 `pe` 数据面，所以 `room.lookup` 抽样回 `memberships=[]`（读透无 pe 行的既有形态，非房间没算——
  房间边已落库 12342、正确性由 1112 parity 钉住）；要 `room.lookup` 回出非空房间号需另起摄入数据面的 rocksdb 真库——同日下午补上（下一条）。
  跨库成员传播（§未决 U3）、S5 死代码仍未动。
- **rocksdb 真库（摄入数据面）复跑，钉 9 收尾**（证据同一份 §6）：外部 `bin\surreal.exe` 8049 `rocksdb:D:\adr078-verify\live-rocks` 空目录起步，
  同一枚 dev 二进制 `store_mode=rocksdb` / `data_face=ingest` / `room_membership=1` / 真 /ALL。1112 首次初始化 32s（pe=30942、18 根）→ 冷重启
  不重摄入（`核对 1 库，落后 0 库`，10.7s）→ 扩 `watch_dbnums=[1112,7997,8000]` 再把面板所在的两个 DESI 初始化：7997 219s（pe=170085、6772 根）、
  8000 25s（pe=6639、953 根），**三库 7743 根 0 失败根**，库上 `room_relate` **70386**（1112 6171 = parity 那个数 / 7997 59844 / 8000 4371）。
  `e3d.room.lookup` 对三个库的构件都回出**非空** `memberships`（如 `17496/72903` → R144 / R320 / R101 / R201，与 `fn::room_relate_of` 同序）、
  `published_version` / `stale=false` / `publication_status=current`；`GET /dbnums?project&mdb` 20 行 `write_state.status=finished`、整份 0 个
  `room_sesno*` 键、三库 `applied_sesno == model_sesno`；三份 stdout 无房间段耗时行。材料表：`fn::room_num_of` / `fn::get_room_number` 回
  `R144` 等；**`fn::room_code`（hd 版）恒回 NONE**——不是房间没算，是 `fn_query_room_code.surql` 里 `let $code = if … { $code = …; }` 在
  SurrealDB 2.1 下块值为 NONE（`RETURN (IF true { LET $x = 5; })` → NONE），同一函数体改成表达式形式在库上直接得 `1RX-R144`；面板库没摄入时它
  还会报错而非回 NONE。**未改代码**，另三条一并列在 evidence §6.5 待拍：F2 durable 形态下 `calculation_status` 恒 `not_computed`
  （判据是进程内 `RootPublicationAck`，模型 lane 直写不留投影，`ready_empty`/`ready_nonempty` 不可达）；F3 `room_panel_relate` 新库上不物化
  （唯一写者 `build_room_panels_relate` 无人调用，CONTEXT「两类边」与 ADR-073 钉 7/8 有漂移）；F4 日志 `房间边=` 计数 ≠ 库上条数。
  钉 9 四句里三句落实、材料表那一句挂在 F1 上——用户随即拍板修（下一条）。

### 修 `fn::room_code`（hd 版）恒回 NONE：`if` 块改表达式形式（evidence §6.7）

- 起因：上一条 F1。`resource/surreal/fn_query_room_code.surql` 里 `let $code = if (…) { $code = $res.room_num; } else if … { … } else { $code = none; };`
  在 SurrealDB 2.1 下存成 `{ LET $code = … }`，块的值是块内最后一条语句的值、赋值语句的值是 NONE，于是每个构件都走到 `RETURN NONE`；
  材料表 20 多个 surql 经它取房间码，全部拿到空。首版 `e43c1d0fd` 起就是这个写法，此前没有任何一条真测调过它。
- 修法：`$id` 与 `$code` 两处 `if` 块各分支只放一个表达式（`{ $res.room_num }` / `{ string::concat(…) }` / `{ none }`），`let $id = return if …`
  的 `return` 一并去掉；`$noun` 取法、`fn::room_relate_of($pe)`（ADR-010 §5 排序边）、`$prefix` 取面板属主名首段、四字 / 五字房间号、
  `string::concat($prefix, '-', $code)` 一字不动；`$id` 仍算完不用（与首版一致，语义不在本笔动）。hh 版走 `fn::room_num_of`，不受影响。
  函数体是 `include_str!` 进二进制的编译期快照（`embedded_surql`），改文件要重编才落库——旧库不用手工灌，启动一次就抬上去。
- 钉：`embedded_surql::tests::hd_room_code_returns_prefix_and_room_number_for_a_member_with_an_edge`（`project_hd`，mem://）——
  SBFR `/1RX-RM01-R144-VOLU` → PANE → `room_relate` → FLOOR：四字 `R144` 与五字 `R0144` 都回 `1RX-R144`，没有边的成员回 NONE；修前红。
- 验证：先把新函数体整段在 8049 `/sql` 跑一遍 → `1RX-R144 / 1RX-R212 / 1RX-R232 / 1RX-R231`；重编 dev 二进制（42s，`0.1.27+g1dbedf8c347a…dirty`），
  8049 上先把旧函数体灌回去（复现 NONE）再起新二进制 `serve` → 启动 12.97s、`已灌入内置 surreal 函数集`、`/health status=ok / room=available`，
  `INFO FOR DB` 的存储体已是表达式形式，同四个构件 `fn::room_code` → `1RX-R144 / 1RX-R212 / 1RX-R232 / 1RX-R231`。
  `cargo test --lib` **2152 passed / 0 failed / 111 ignored**。钉 9 四句至此全落实；F2 / F3 / F4 仍开着。

### 修 F2：`e3d.room.lookup` 加 durable 侧就绪判据 `model_room_generation`（ADR-073 追记 **D14**，evidence §6.8）

- 起因：上一条 F2。落盘形态下 `calculation_status` 恒 `not_computed`——边、房间号、版本三元组全对，判据却只有**进程内**的
  `RootPublicationAck`，而 ACK 按 ADR-079 进程终止即消失：库是上一次运行灌的，冷重启后一条 ACK 都没有，
  `ready_empty` / `ready_nonempty` 在生产存储形态下不可达。缺的不是事实，是够得着的判据字段。用户 2026-09-16 拍板补。
- 修法（ADR-073 新增 **D14** + 源码钉 19）：水位行上加一格 `dbnum_watermark.model_room_generation` = 「这个库的房间边整库对齐到
  哪一代常驻模型」，取值就是根 ACK 里那个 `room_generation`（`ResidentRoomModel::input_version`，D12）。
  - **写**（`task_end::RoomGenerationStamp` 三档 + `batch_worker::task_room_generation_stamp` / `dbnum_initialize`）：随 `model_sesno`
    由收尾那一笔同盖，与库级目录落款（ADR-079 D4）同一条纪律——只有能替**整库**说话的任务才 `Aligned`：初始化、整库重排的
    ModelOnly、本任务跑过整库房间重算（D13）；普通增量窗口 `Untouched`（那一格原样）；`room_membership=false` 下盖水位
    `Cleared`（这一版只承诺几何，不许留着替旧边说话）；`all_roots_ok` 为假一律不写。「起点为 0 的 ModelOnly」这句资格判定
    抽成 `whole_library_model_only`，落款与房间代号共用一处（源码钉）。
  - **读**（`query_service::room_edges_ready` + `QueryService::library_room_generation`）：判据变成两层——进程内该根 ACK 命中，
    **或**库上那一格与此刻常驻模型同一代且进程账本里这根没欠着（ADR-079 的一票否决）。两层同一个代号，所以房间 / 面板真变过
    时一起作废回 `not_computed`。只在 `store_mode=rocksdb` 下启用第二层：`mem://` 活不过重启，库与水位行一起消失，那一档仍
    只认 ACK（d-381 的启动屏障要的就是本进程逐根算完那一句）。
  - **看**（`manual_update::RoomAlignmentColumns`，spec §4.7）：`/dbnums` 每行多一格可选的
    `room_alignment{generation, resident_generation, current}`（DESI 行才有）——`current: false` 就是「这个库此刻只能回
    `not_computed`」的解释，是这句话的唯一出处。那句「成不成立」与查询侧共用一处判据
    （`query_service::library_room_alignment_is_current`），两处各写一遍就会出现面板说「对齐了」而查询回 `not_computed`。
  - **不是第三枚水位**：没有自己的推进规则、不参与任何放行 / 恢复判决，只是模型水位的一个属性；W3 的 `room_*` 五列一列没回来
    （`task_end` 那条钉逐个列名守着）。
- 验证（8049 那座 rocksdb 真库，`D:\adr078-verify\live-rocks`，三库 70386 条边原样）：
  1. 新二进制起服务先**复现** F2——`17496/72903` 四条边、`published_version=729 / stale=false / current`，`calculation_status=not_computed`
     （库是旧二进制灌的，那一格还是 `null`）。
  2. `POST /dbnums/1112/model/rebuild` → `model_only` / `model_start_sesno=0` / 18 根全 `generated` / 38s succeeded →
     库上 `dbnum_watermark:1112.model_room_generation = efd4ebe1…ad269`，7997 / 8000 那一格仍 `null`（它们没跑过整库对齐，
     判据如实说不知道）；1112 的 `room_relate` 重算后仍是 **6171** 条（与 parity 那个数逐字相同）。
  3. **冷重启**（12.33s，`核对 3 库，落后 0 库`，`model_cache.resident_roots=0` = 一条 ACK 都没有）后再查：
     `17496/72903` → **`ready_nonempty`**（4 条边）、`17496/100011` → `ready_nonempty`（6 条）、`17496/105823` → `ready_nonempty`（3 条）；
     一根边都没有的 `17496/121765` / `17496/121728` → **`ready_empty`**；此时 7997 / 8000 的构件仍 `not_computed`（那一格还是 `null`，如实）。
     `/health.features.room.generation` 与库上那一格逐字相同，`dbnum_consistency` 三库 `in_sync`。
  4. **三库都点亮**：`model/rebuild` 8000（953/953 根、46.5s）与 7997（6772/6772 根、570.7s）各一次，三库那一格现在同为
     `efd4ebe1…`；**边数逐库与首次从零初始化一字不差**（1112 6171 / 7997 59844 / 8000 4371 / 合计 70386）——ADR-010 §9
     「整库重算 == 从零初始化，逐边相等」在三个真库上又走一遍。再冷重启后八个抽样全部 `ready_nonempty`
     （`24381/105161` R232/R332、`24381/72538` R332/R231/R330、`24384/23030` R231/R333/R330…），无边的那个仍 `ready_empty`，
     房间号与点亮前那一轮逐条相同——变的只有判据那一格。
  5. `/dbnums` 那一格：46 行里 29 个 DESI 行各带一格 `room_alignment`、非 DESI 行整格缺失；三个点亮过的库
     `current=true` 且 `generation == resident_generation`，其余 26 个（这座库从没摄入过）`generation=null / current=false`。
  6. `cargo test --lib` **2159 passed / 0 failed / 111 ignored**（新增 7 条钉：收尾三档落地、只有替整库说话的任务才盖、
     资格判定只有一处、库上那一层替没有 ACK 的根答话、第 2 层要落盘且同代、两层判据都接在 `calculation_status` 上、
     `/dbnums` 那一格与两枚水位同一趟读回且判据只有一处）；clippy 对改动文件零告警，rustfmt 新增行干净。
- 未动：F3（`room_panel_relate` 不物化）、F4（日志 `房间边=` 计数 ≠ 库上条数）、§未决 U3 跨库成员传播；
  普通增量窗口按 D14 不盖那一格，一个库要回 `ready_*` 得先有一次能替整库说话的任务给它盖上。

### 面板自身的变化 → 本库整库房间边重算（`RoomOnly` 落地 d-819，ADR-073 追记 D13，收 §未决 U1 的库内那一半）

- 起因：逐根路径（ADR-073 D1）只算「本根在哪些面板里」；面板那一侧变了——PANE 的几何 / 属主、FRMW / SBFR 改名、CWALL / CFLOOR
  搬迁——它名下**别的根**的成员边与 `room_num` 一个都点不着，仪表全绿。d-819 定的填法（本窗变更集触发、给该库排一条内部 `RoomOnly`）
  此前只有 `WriteIntent::RoomOnly` 枚举与回退渲染，规划器不产出、执行体不执行。用户 2026-09-16 拍板继续 S4。
- 修法（ADR-073 2026-09-16 那节新增 **D13** + 源码钉 17–18；CONTEXT 新词条「整库房间重算」，「逐根房间归属」「ModelOnly 任务」改口；
  §未决 U1 标已收）：
  - 新模块 `data_interface::room_recompute`：`room_definition_change(ledger.entries())`——本窗变更集里碰到 `PANE / FRMW / SBFR / CWALL /
    CFLOOR` 的任一条目（`Unresolved` 不算；**只按 noun 判，不解析 `detail` 散文**——DESC 之类只改元数据的面板改动多触发一次，接受）→
    原因一句；`room_only_work_item(dbnum, …)`（`ModelWorkAction::RoomOnly`，`target_refno = dbnum=<n>`，一窗一条）；
    `request_for_task(side_actions, refresh)`——两条触发合一句：规划器排了 `RoomOnly` / 开工前对齐（D12）换了房间代号。
  - `model_update_plan.rs`：`ModelWorkAction::RoomOnly`（`room_only`），不进 `is_room_recalc`——`manual_update` 那道「房间重算项不在内」
    的旧过滤只拦 `RoomRecalc*`，`RoomOnly` 随 `side_actions` 交给任务。
  - `window_root_plan::build_model_update_plan_from_window` 末尾：命中就 `warnings` 一句 + 工作项一条（另一路会话在途改动的文件，只加了这一段）。
  - `model_db_adapter::recompute_dbnum_room_edges(target, store, dbnum, rooms)`：本库全部有回执的根，按投影里的现有几何
    `calculate_for_root` → `render_root_room_edges`（与逐根路径同一套渲染，D9 的保护集照用），按 `PERSIST_CHUNK` 装块、一串事务提交，
    提交全过后把这些根 ACK 的 `room_generation` 对齐到本台计算器；没有几何的根跳过，算不出的根边原样进 `failed`。不重生几何、不动
    模型行、不动空间树。
  - `batch_worker`：`DataBatchTaskResult` 加 `#[serde(skip)] room_recompute: Option<String>`（执行体内部接力）；`execute_frozen_batch_body`
    留着开工前对齐的回执、收尾前 `request_for_task`；`execute_frozen_batch` 的 DESI 收尾段在 `await_units_durable` **之后**、`model_outcome_of`
    **之前**、`flush.all_acked()` 门内跑 `run_room_only_recompute`（stage `rooms`）——那一刻本批根的模型行与边已在库里、本库没有第二个
    写者；算不出的根按 D2 进账本 `failed` + 单元清单补 `Failed`、任务 `Partial`、`all_roots_ok = false`、`model_sesno` 不盖；开关关
    （`Ok(None)`）如实记一句按齐了算；计算器冻不出 / 写库失败 → 不盖。刷盘没齐就不跑（那批根已判死）。
- 没收的那一半：跨库成员（面板在 A、成员在 B）只在 A 变了之后**第一个开工**的库上被第二条触发带到，其余库等自己下一次触发——U3 仍开着。
- 验证：`cargo test --lib` **2130 passed / 0 failed / 109 ignored**（新钉 6 条：`room_recompute` 5 条——房间定义族 noun 全覆盖 / 原因取样 /
  工作项目标是库不是根 / 两条触发 / 接线位置（刷盘之后、收尾之前、`all_acked` 门内、旧过滤不拦）；`model_db_adapter`
  `recomputing_a_librarys_room_edges_reuses_the_per_root_rendering_and_refreshes_acks`——`mem://` 上重算后的边与逐根落库逐字相同、NaN 根进
  `failed`、ACK 代号对齐；`action_names_roundtrip_through_parse` 收 `room_only`）；`cargo check --all-targets` 绿；`cargo clippy --lib` 本仓零告警；
  rustfmt 本轮改动的行干净。**已 live**（2026-09-16）：(e) 改 PANE 整库重算 = mem:// `moving_a_panel…` / `recomputing…` 全绿；
  (f) 只改 BRAN 不触发 = `a_change_to_any_room_definition_noun…` 全绿；真机 1112 生成 12342 边 0 失败。见本节顶「room_membership 真机跑通」与 `docs/evidence/2026-09-16-room-membership-live-and-real-file-acceptance.md`。
- 未动：S5 死代码（`room_resident.rs` / `room_model.rs` 里 ADR-067 / ADR-010 时代约 3000 行）；`ModelWorkAction::RoomRecalcElement / RoomRecalcPanel`
  两个无人产出的枚举值也在其中。

### 常驻房间模型在每个任务开工前对齐到此刻的文件版本；房间代号改为计算输入摘要（ADR-073 追记 D12）

- 起因：常驻房间模型启动建一次、进程活多久用多久——换代点 `room_resident::ensure_current` 自 ADR-067 作废后在生产里
  **零调用方**，新增的 PANE、搬走的面板、前进了的 DESI 都进不了它，逐根算出的归属按启动那一刻的面板集算，且不报错；
  同时 `required_room_generation` / ACK 的 `room_generation` 拿全 MDB DESI 的文件 composite 当代号，任一库前进本该换代却没人
  重建，键与实物脱节。用户 2026-09-16 拍板继续 S3。
- 修法（ADR-073 2026-09-16 那节新增 **D12** + 源码钉 15–16；CONTEXT 新词条「任务开工前对齐」「房间代号」）：
  - `room_topology::latest_frozen_sources` 改成**便宜的探针**：MDB 成员清单（进程内缓存）+ 每个 DESI 的最新会话
    （`latest_source_version_of_file`，按文件戳缓存）+ 冻结令牌，不再 `E3dModelService::from_current()`（那一步整套装 schema，
    每个任务付一次就把任务拖垮）；重建那一侧 `load_room_groups_from_files` 自己 `from_frozen_sources`，行为不变。
  - `room_resident::refresh_before_task()`：开关关 / 无体制 / `Loading` → `NotApplicable`；探针 composite == 装好的 `manifest.key`
    → `Unchanged`（常路，零重建）；不等或此前没建成 → 复用 `initialize_file_backed_at_startup` 按文件重建并在
    `lock_spatial_serial` 里装上（两层按文件的缓存让没变的文件只付重开校验的钱），生命周期 `lifecycle_for(new)`，
    `/health.room_build.resident.last_swap` 记 `note = task-start refresh` + 与上一份在册视图的 `TopologyDiff`；重建失败 → 留旧代
    继续算、`Degraded` 多一条 `stage=refresh`（`lifecycle_after_refresh_failure`，同一条只留最新）、没有旧代仍 `Blocked`。并发开工
    共用一把 `tokio::Mutex`，后到的等完再探。新 `ResidentInputs::configured()` 收口层级 / 关键字 / 校验函数 / 网格目录。
  - 入口 `model_db_adapter::refresh_room_reference_before_task(dbnum)`（`Rebuilt` / `Failed` 打一行任务日志），接在三处冻计算器 /
    冻代号**之前**：`batch_worker::execute_frozen_batch_body` 的模型半边（`run_model_side_actions` / `run_unit_worklist` 前）、
    `dbnum_initialize::model_half_inner` 的 `frozen_room_calculator()` 前、`model_dbnum_ensure` 准入的 `required_room_generation()`
    前（`StartupRoomBootstrap` 显式带启动代号，不换）。执行体不直接摸 `room_resident`，ADR-073 源码钉 1 照旧。
  - **代号**：`MemoryRoomCalculator::resident_generation()` 回 `definition_version`（= `ResidentRoomModel::input_version`），
    `required_room_generation()` 与生命周期的 `generation` 同改——拓扑 + 在册 + 有几何面板的几何的摘要。一个 DESI 前进却没动
    房间 / 面板 → 代号不变 → 旧 ACK 有效；真变了 → 旧 ACK 作废、`e3d.room.lookup` 回 `not_computed` 直到重生成（d-381 语义）。
    文件 composite 只留作探针键。
- 验证：`cargo test --lib` **2124 passed / 0 failed / 109 ignored**（新钉 `the_task_start_refresh_precedes_every_calculator_freeze_and_stays_cheap`、
  `the_resident_generation_follows_the_calculation_input_not_the_file_composite`、
  `a_failed_refresh_keeps_the_previous_generation_and_exactly_one_refresh_problem`；`empty_kv_mem_publishes_an_indoor_root_without_attribute_ingestion`
  的 ACK 代号断言改成 `rooms.resident_generation()`）；`cargo check --all-targets` 绿；`cargo clippy --lib` 本仓零告警；rustfmt 本轮改动的行干净。
  **已 live**（2026-09-16）：源码钉 16 由真文件 `ams_task_start_refresh_rebuilds_from_a_stale_key_and_then_reuses_the_generation`
  覆盖——陈旧键 → `Rebuilt`（真 /ALL 215 房/230 面板/0 缺、代号换真值、`note=task-start refresh`）→ 再探 `Unchanged` 23ms（(d) 的真文件侧）；
  见 `docs/evidence/2026-09-16-room-membership-live-and-real-file-acceptance.md`。
- 未动：U1 面板自身变化对**其余根**的传播（`RoomOnly`，S4）——本条只保证下一个任务用的是新面板集；死代码清理（S5）。

### 房间输入不全 = 降级，不判死：`room_membership=true` 能开起来不炸（ADR-073 追记 D8–D11）

- 起因：0.1.25 现场把 `room_membership` 开着，MDB 缺 6003 SCBDESI 的文件 → 常驻房间模型 `Blocked` → 每个任务
  `frozen_room_calculator()` Err → 1112 `18/18`、7997 `6772/6772` 全部根「room calculator unavailable」，`model_sesno`
  永不前进；当时只把模板改回 `false`，房间门政策留待本条（计划 2026-09-15 §8 Q1）。用户 2026-09-16 拍板：先把「能开起来不炸」
  做出来，先写 ADR 追记再动代码。源码里两道**整份**闸把一处输入缺失放大成全库瘫痪，都不是 D2 说的逐根：
  `room_topology::latest_frozen_sources` → `require_design_files()?`（缺一个 DESI 文件整份 Err）；
  `MemoryRoomCalculator::new` → `PanelIndex::ensure_complete()?`（在册面板任一块没几何就 Err，与 2026-09-09 W1
  「从未生成不阻代」自相矛盾——`warm_meshes` 放行、`new()` 又拦）。另外 `warm_meshes` 对「有几何行却包围盒不可用」与
  「`.mesh` 读不出」整份 Err，`preload_panel_projection` 一块面板生成失败 `try_join` 整份 Err。
- 口径（ADR-073 新增「2026-09-16 修订」一节，D8–D11 + 源码钉 10–14；CONTEXT 新词条「房间输入缺口」）：
  **D8** 缺 DESI 文件按有的文件建，缺的库记 `missing_dbnums`、不在册；**D9** 在册面板无可用几何（从未生成 / 预生成失败 /
  包围盒不可用 / `.mesh` 读不出）一律不进索引、逐根写边时作为**保护集**让开、留痕，不阻断；**D10** 生命周期 `Degraded { problems }`
  放行计算器，`ensure_ready` 只拦 `Loading` / `Blocked`（`Blocked` 仍是「房间体制本身没起来」，维持 D2）；**D11** `Degraded` 下
  `model_sesno == applied_sesno` 的房间承诺只覆盖在册且有几何的面板，`/health` 必须并排说清。逐根失败（构件包围盒不可用、
  候选面板网格取不到）不变。
- 修法：
  - `room_model.rs`：`MemoryRoomCalculator::new` 删 `ensure_complete()?`（方法一并删除，零调用方），新 `missing_panels()`；
    `render_root_room_edges` 在保护集非空时先 `LET $room_protected_panels = [pe:…]` 一句（一根一份名单，不逐构件展开——
    几何覆盖稀疏的实例上缺几何面板可能两百多块），每个构件的 `DELETE {element}<-room_relate` 带
    `WHERE in NOT IN $room_protected_panels`；为空时语句逐字不变。`render_element_relate_statements` 抽出
    `_guarded` 底座，旧签名（内联数组）原样保留。
  - `room_topology.rs`：`latest_frozen_sources` 回 `FrozenDesignSources { composite, tokens, sources, missing_design }`，
    不再 `require_design_files()?`；`load_room_groups_from_files` 回 `RoomGroupsFromFiles`，把 `missing_design` 带出来。
    `mdb_membership::require_design_files` → `missing_design_files() -> Vec<(u32, String)>`。
  - `room_resident.rs`：`ResidentRoomModel` 加 `missing_dbnums` / `problems`；`assemble` 把 `unusable_panels` 记成缺口；
    `warm_meshes` 面板级失败 `demote_panel`（搬进 `missing_panels`、共用同一 `.mesh` 的面板一并搬、几何摘要重算），
    只对预热 worker 崩溃 Err；`preload_panel_projection` 单面板失败 / 单文件 E3D 服务起不来只记缺口；新 `lifecycle_for(model)`
    （无缺口 `Ready`、有缺口 `Degraded`），`record_startup_init` 用它，`mark_failed` 一律 `Blocked`；`ensure_ready` 放行 `Degraded`；
    `stats` / `/health.room_build.resident` 出 `missing_dbnums` / `problems`；启动横幅一句「degraded：N 条缺口」。
  - `handlers.rs`：`features.room.status` 新档 `degraded`（带 `generation` / `executor` / `problems`），`/health.degraded_sections`
    含 `room`；`query_service.rs` `e3d.room.lookup` 的 `calculation_status` 在 `Degraded` 下照 `Ready` 判；`lib.rs` 横幅措辞。
- 验证：`cargo test --lib` **2121 passed / 0 failed / 109 ignored**（新钉 `a_registered_panel_without_geometry_is_protected_not_blocking`、
  `eager_mesh_warm_degrades_on_unusable_geometry_and_never_generated_panels_alike`、
  `eager_mesh_warm_deduplicates_keys_and_demotes_a_panel_whose_mesh_is_missing`、`missing_design_files_are_named_for_degradation_not_thrown`；
  ready 门的钉改为 `Degraded → Ok`；live 用例 `update_scope` 那条改断言「冻结源 = 声明的 DESI − 缺文件的」）；
  `cargo check --all-targets` 绿；`cargo clippy --lib` 本仓零告警；rustfmt 在本轮改动的行上干净（各文件其余漂移属别的会话）。
  **已 live**（2026-09-16）：源码钉 14 (a) 缺面板几何 = mem:// `a_registered_panel_without_geometry…` 全绿；
  (b) 缺一个 DESI = 真文件 `ams_missing_desi_degrades_the_room_reference_instead_of_blocking_it`——缺 SCB 6003 → 常驻 `Degraded` 不判死；
  见 `docs/evidence/2026-09-16-room-membership-live-and-real-file-acceptance.md`。
- 未动（下一刀）：U1 面板自身变化的传播（`RoomOnly` 只有 `WriteIntent` 枚举，规划器不产出）、常驻模型启动后不换代、
  `room_resident.rs` / `room_model.rs` 里 ADR-067 / ADR-010 时代约 3000 行死代码。

### 本地发布 0.1.27：ADR-079 `gen_root` 表退役、掉线 UNC 15 s 上限修复（附 ADR-079 真库重启验证）

- 后端 `0.1.27` 构建自 `2cb5de1f112c`（**干净树**，build_id 不带 `.dirty`），复用 0.1.26 那份已验证 Web
  （plant-ui `0.1.11`，`web_source = reused-from:ams-local-0.1.26…-runtime-workspace.zip`，13 个文件）。
- 产物 `releases/ams-local-0.1.27_release.20260916011419.g2cb5de1f112c-runtime.zip`（119.9 MB / 125,720,324 B），
  SHA-256 `ccc6f12f0be3df075685908c55521b8369c63b56186d517682ea22f787d11ccd`；包内 `aios-database.exe` 自报 0.1.27。
- 包型 `runtime`（`047dad5af` 起 workspace\ 一律不进包，比 0.1.26 的 runtime-workspace 档小一半）：包根带
  `aios-database.exe` / `web\` / `bin\surreal.exe` / `resource\surreal\*.surql` / `schema\{e3d210,e3d31}` /
  出厂 `DbOption.toml`（设置模式）/ `.first-run-pending` / `Start-AMS` / `Start-Surreal` / `Rollback`。
- 覆盖 0.1.26（`f0369365e`）之后的提交：ADR-079 `gen_root` 表退役（`0247da413`，**破坏性**：存量库首启删
  `gen_root` / `root_dependency` 两表 + 12 个 `fn::gen_root_*` / `sync_*` 函数，`/model/ensure` 回执少四格）、
  掉线 UNC 搜索根绕过 15 s 上限修复（`e0742aab3`）、校核回退次数 `count(action='return')`（`2758c434d`）、
  RVM 基线亚容差壳不定 AABB（`a7d5314ee`）、发布包只出 runtime 一档（`047dad5af`）。用户口径见
  `docs/release-notes/0.1.27.md`。
- 冒烟（口 19099）：A 出厂配置起服务=设置模式+向导页、B 本机配置全启动 `medium=mem` / `durable=false` /
  服务没拉 surreal 子进程——两腿通过；C rocksdb 腿在 runtime 档按设计跳过（不带 workspace）。rocksdb（durable）
  路径另由下面的 ADR-079 真库验证覆盖，比冒烟 C 腿彻底。
- **补上 ADR-079「真库重启一次」验证**（`docs/plans/2026-09-15-retire-gen-root-table-dev-plan.md` §3 原列「未验证」）：
  用本包同源 `2cb5de1f1` 的 0.1.27 二进制，在隔离 rocksdb 库（bundled surreal，`initialize_ams_dbnums --with-model 1112`
  真部署 30,942 元素 / 18 根 / `model_sesno=729`）上：
  - 先确认新库不含 `gen_root` / `root_dependency` 与任何 `fn::gen_root_*`（149 表 / 74 函数里都没有）；
  - 播下 pre-ADR-079 遗留（`gen_root` 2 行 + `root_dependency` + `fn::gen_root_of`/`sync_gen_roots`/`gen_root_cover`），
    并把 1112 的 `model_sesno` 压到 100（< `applied_sesno` 729）；
  - 起 0.1.27：stdout 打「…15 张退役表（及 12 个退役函数）已确认不存在（一次性迁移，幂等）」，`INFO FOR DB` 里
    那两表与三个函数全没了；启动核对读水位「`[model-completion] dbnum=1112 applied=729 model=100
    reason=model_watermark_behind` → 排 1 项 ModelOnly（`model=100→729`）」，跑完 `model_sesno` 追平 729；
  - 再重启一次：迁移那句照打（幂等、表仍不在），启动核对「核对 1 库，落后 0 库，排队 0 项」——水位齐平就不排。
  证实删表迁移与启动核对排队两条都按 ADR-079 设计工作。仍未验：plant-ui 对 `/model/ensure` 少四格的消费（前端）。
- 构建输入：gen-model 干净树 `2cb5de1f1`；vendor 取各自 committed HEAD（e3d-model `7da42b5` 等），另叠上 0.1.26
  也在用的两处稳定未提交增量——e3d-io 的 `DbSet::metadata()`（committed e3d-model HEAD 依赖它）与 old-aios-core 的
  rkyv `validation` 特性（`pdms_shape` 走 `check_archived_root` + 索引下标校验，防损坏 `.mesh` 反序列化溢出）。
  e3d-model 今日在途的 `increment.rs`（`plan_update_with → Option<NoModelTier>`，与另一路会话 gen-model
  `window_root_plan` 的在途改配套）**未纳入**——它与本包的 committed gen-model 不配套。`cargo test --lib`
  **2114 passed / 0 failed / 109 ignored**。

### 修：搜索根去重与闭包 key 改用字面 `path_key`，掉线 UNC 不再绕过扫描线程的 15 s 上限（ADR-078 追记）

- 起因：用现场服务器 `\\10.181.11.245\PDM`（445 不通）实测 `POST /api/v1/setup/probe/mdb`：wall **21,083 ms**，
  可 `roots[1].scan_ms = 0`、problem 报的是 OS 的 `os error 53` 而不是「扫描 15 s 未答」——等的不是扫描线程，
  是线程起来**之前**调用方就对每个根调了 `project_paths::path_identity` → `std::fs::canonicalize`，它在掉线 UNC
  上把调用方线程阻塞到 OS 自己放弃（PowerShell `[IO.Directory]::Exists` 同一路径同样 21 s）。三处：
  `mdb_closure::search_roots` 的去重、`plan_key`（最近闭包的 key）、`setup.rs` 探测的根去重。上一条里
  「掉线 UNC 下启动 28 s 就绪」也是这一条兑的 OS 超时，不是我们的 15 s。
- 修法：`project_paths::path_key` 转 `pub`（斜杠方向 / 大小写 / 末尾分隔符归一，**不碰文件系统**），注释写清与
  `path_identity` 的分界——起扫描线程之前对根算 key 的地方只能用它；新 `mdb_closure::collect_search_roots`
  把三处收口成一处，向导探测与启动同一套规则，`search_roots` 只负责拼 `[project_path] ++ project_search_paths`；
  `plan_key` 同样改字面，同一个根在线 / 掉线两个时刻不再算出两个 key，共享盘抖一次 `latest_for` 不再白重扫一遍。
- 代价：联接与其目标、8.3 短名这类「字面不同、实为一处」的根不再合并，各扫一遍、后到的列 `shadowed`（不阻断、
  不算冲突）；UNC 与本地同一目录本来 `canonicalize` 也认不成一处，实际行为差异很小。
- 验证：dev 构建以设置态起在 19101，负缓存过期后同一探测 wall **15,557 ms**、`roots[1].reachable = false`、
  `scan_ms = 15000`、problem =「扫描 15 s 未答，按不可达处理…」，其余 5 项目 40/40 照常；跨根 / 影子 / 重复写法
  三例与修前逐项相同，同一路径三种写法合成 1 个根。新钉 `search_roots_are_deduplicated_literally_and_in_order`、
  `plan_key_does_not_depend_on_whether_the_root_is_reachable`；`cargo test --lib` **2119 passed**（唯一 1 failed 是
  `window_root_plan` 另一路会话的在途改动）。evidence 追记 `docs/evidence/2026-09-15-adr078-mdb-project-closure.md` §9。
  **未进 0.1.26 包**（出包在 `f0369365e`，本修 `e0742aab3` 在其后），要到现场得等下一版。

## 2026-09-15

### 本地发布 0.1.26：项目集合由 MDB 派生、`computer_name` 别名删除

- 后端 `0.1.26` 构建自 `f0369365e8a2`（**干净树**，build_id 不带 `.dirty`——打包跑在隔离 worktree
  `gen-model-adr078` 上，主工作树里另外几路会话的在途改动一个字节都没进包），路径依赖 e3d-io
  `6431ee7120`；前端 plant-ui 保持 `0.1.11` 并**复用 0.1.25 那份已验证 Web**
  （`web_source = reused-from:ams-local-0.1.25…-runtime-workspace.zip`，13 个文件，脚本校验了来源包哈希），
  只有 `web\setup.html` 按脚本约定从仓里覆盖成本版的向导页。plant-ui 工作树当时也有在途改动，
  为此另开干净 worktree `plant-ui-0.1.11-clean` 取 `index.html` 与版本号。
- 产物 `releases/ams-local-0.1.26_release.20260915133913.gf0369365e8a2-runtime-workspace.zip`
  （209.3 MB / 219,485,672 B），SHA-256
  `195b4280657c85e5d27621414754565e0557d6f5374de642cbefb6f15c0cf1f3`；包内 `aios-database.exe`
  SHA-256 `f6f3c20a014f7510241f2ae1f5ead39893bcb719b940aa3ab53195aea5b696aa`。
- 包型 `runtime-workspace`（带 `workspace\`、不带 `assets\meshes`），与 0.1.24 / 0.1.25 同档：
  assets 灌入 165 个文件 / 115.6 MB，meshes 排除 83,130 个文件 / 1,142.4 MB。先误打过一份 `-full`
  （465.3 MB，把本机 1.26 GB 的 meshes 也灌了进去），已删，`releases/` 里只留 runtime-workspace 这一份。
- 覆盖 0.1.25（`0c6c62b3b5`，2026-09-15 出包）之后的 5 个提交。用户可见的两条主线写在
  `docs/release-notes/0.1.26.md`：项目集合由 MDB 派生、项目目录在人指定的搜索根里按 `<code>000` 找
  （ADR-078 取代 ADR-046，`included_projects` 退役、新键 `project_search_paths`、向导第①步重排、
  出厂模板 `room_membership` 改 `false`）；`aios_db.parse.sessions` 的 `computer_name` 别名按
  0.1.25 的预告删除（**破坏性**：仓外脚本读 `row["computer_name"]` 会 `KeyError`）。
- 三腿冒烟全过：A 出厂配置起服务 `mode=setup` + 向导标记；B 本机配置全启动 `medium=mem` +
  `durable=false` + 服务没拉 surreal 子进程；C 包内 `surreal.exe` 在 18010 起 rocksdb 库、服务连上
  `medium=rocksdb` + `durable=true` + 数据目录落 7 个文件。冒烟口改用 19099——9099 上还挂着本轮
  ADR-078 的真部署验证。
- 验证口径：打包前在同一棵隔离 worktree 上跑过 `cargo test --lib` **2110 passed / 0 failed / 109 ignored**
  与 `cargo check --all-targets`（绿）；python 绑定 `maturin develop` 后 `pytest tests/test_parse_offline.py`
  13 passed；ADR-078 的真部署三条用例见 `docs/evidence/2026-09-15-adr078-mdb-project-closure.md`。
### `gen_root` 表退役：生成根的成败只活在进程里，模型面只留水位（ADR-079）

- 起因：用户拍板——生成根的信息不该存表，放内存就够，进程终止就消失；库的同步以水位为准，同步了一半的清理掉重新增量 / 初始化。
  `gen_root` 一表五用（回执 / 失败留痕 / 规划输入 / 启动证据 / 目录落款），全 AMS 12 万根时一致性普查整表一扫 4–6 s 每 30 s 一次、
  水位探针 avg 5.5 s、每根落库多一笔 `UPSERT`，而 ADR-071 / 072 / 076 之后这五个角色没有一个需要持久化。
- 新模块 `data_interface::model_root_ledger`：进程内账本，每根一条 `Generated | Failed{error} | Refused{error}` + `source_end_sesno`，
  一把 `RwLock`。写者：落库适配器（全部笔提交后记成功，只对写全局库的 `PersistTarget`）、任务执行体（失败）、初始化双 lane（排空后逐根）、
  `DeleteCleanup` / 删子树（`forget_root`）、回退 / 整库清空 / 删库（`forget_dbnum`）。读者：窗口规划器、启动 / ModelOnly 规划、一致性判决、
  `/model/ensure` 回执、按需 ensure 凭证判据。
- 模型面唯一的持久事实是 `dbnum_watermark.model_sesno`：窗口规划器的 `roots_S` = `DbSet@model_sesno` 枚举、不读任何凭证行；
  「未触根凭证前移」（ADR-056 P2-2）退役，`advance` 名单只进回执（`CredentialAdvanceReceipt::from_plan`，形状不动）；
  启动核对只比两枚水位（`startup_model_recovery_plan(applied, model)`：`model < applied` 才排 ModelOnly，左端 `model`，0 = 整库）；
  `model_watermark.rs`（ADR-060 D6 / D7 的探针、下调、幽灵清零、`legacy_adopted`）整个模块删除；`startup_coverage_floors` /
  `backfill_gen_root_dbnum_on` / `coverage_tracked_dbnums` 删。
- 目录落款收成库级一份 `dbnum_watermark.model_catalogue_sessions = [{dbnum, session}]`，任务收尾随 `model_sesno` 合并写
  （`task_end::render_catalogue_stamp_merge`，只在 `all_roots_ok`）；有资格盖的任务：初始化 / 起点 0 的 ModelOnly（从回执取最旧）、
  目录级联排的任务（`DiscoveredBatch::catalogue_stamps`，名单全成才盖，不从回执反推）。目录版本核对的过滤器改为**库级**
  （`libraries_behind_catalogue`）：命中根所在库落款仍旧才排，落款仍旧却没被波及的库排一条只带落款的空任务（`CatalogueCascadeReport.stamp_only`），
  退化目录库波及的库整库重排；`lazy_roots` 恒 0。`/model/rebuild` 改排起点 0 的 ModelOnly（不再随身带全量名单）。
- 一致性判决：四格读账本（零 I/O，20 s 普查缓存与 `invalidate_census` 退）；`NotJudgedReason::NoGenerationRoots` 退役——水位不等就是落后。
  `/model/ensure` 回执的 `publication_status` 改由投影 ACK（`ready` / `stale`）与账本（失败带 `last_error`）合成，逐根 revision / target 四格不再出现。
  `/health.startup.no_credential_dbnums` = 模型水位从没盖过的 DESI 库。
- 存量库：`retire_publish_run_tables` 追加 `gen_root` / `root_dependency` 两表与 12 个 `fn::gen_root_*` / `fn::sync_*` 函数的
  `REMOVE … IF EXISTS`（幂等）；`resource/surreal/gen_root.surql` 删除、内置快照与核验面摘掉。回退 / 清库 / 删库不再点名 `gen_root`。
- 验证：`cargo check --all-targets` 绿，`cargo clippy --lib` 本仓零告警，`cargo test --lib` **2114 passed / 0 failed / 109 ignored**
  （含账本 3、凭证判据 1、库级落款过滤 1、模型门源码钉 2、调度器落款并入 1、任务失败进账本 1、启动决策 4；`direct_window_replay_parity`
  夹具去 `gen_root` 并新增「落库不再建 `gen_root` 表」断言）。**未验证**：真库重启一次看删表与启动核对排队；plant-ui 对 `/model/ensure`
  少掉四格的消费。见 ADR-079 与 `docs/plans/2026-09-15-retire-gen-root-table-dev-plan.md`。

### 项目集合由 MDB 派生、项目目录在搜索根里按 `<code>000` 找；`included_projects` 退役（ADR-078，取代 ADR-046）

- 起因：部署实例 `_runs\ams-0.1.25` 只勾了 `AvevaMarineSample`，AMS `/ALL` 声明的 41 个外部库
  （ACP 34 / SCB 4 / APS 2 / ZDJ 1）全部落空——启动 stderr 40 条「声明了 dbnum … 没有它的文件」，
  「显示模型」按需生成报「db 7001 没注册进这个 DbSet」；开发机那份写了四个项目的名单同样漏了 APS。
  core.dll 没有「项目名单」：`DB_DB::makeDirectoryName`（`0x590b4a0`）拿声明自带的项目码拼
  `/%<PROJ>000%`，项目全集就是环境变量（`DLL_PDMSProject::BuildProjectsList`，`0x5840b80`）。
- 新模块 `data_interface::project_locator`（D2）：在搜索根 `[project_path] ++ project_search_paths`
  里按 `set_project_evars.bat` 文档化的三种摆法找 `<code>000`；同根同码两处 = 冲突不选，跨根先到的根赢、
  其余记影子；每个根独立线程 + 15 s 超时，掉线的共享盘只让自己缺席；不读环境变量、不解析 evars、不开库文件。
- 新模块 `data_interface::mdb_closure`（D1）：主项目按 `project_name` 在搜索根里定位（文件夹名，兜底当
  项目码），它的 SYS 里该 MDB 的 CURD 按 `PROJ` 分组，每个外部项目码交给定位器；声明按 `(project, mdb)`
  读一次缓存在进程内，`latest()` 留最近一份闭包。`project_paths::plan_watch_dirs / resolve_project_root`
  与新的 `project_names` 都改由它给；`mdb_membership::resolve` 拆成「读未绑定声明」+「按闭包目录绑定文件」。
- `included_projects` 不再参与任何解析：`DbOption` 字段保留（aios-core 加 `serde(default)`，删掉键也能装），
  非空时启动打一句「已忽略，本次由 MDB 派生：AMS(主) / ACP→AvevaCatalogue / …」。新扩展键
  `project_search_paths`（默认空）进 `DbOptionExt` 与出厂模板。`catalogue_project_priority` 保留，校验对象改为闭包项目名。
- 缺口可见（D5 / D6）：路由目录的 `declared_without_file` 带项目码，`RouteError::NoFileForDbnum` 加
  `project_code`，措辞变成「项目 ACP … 搜索根里没有 acp000 …加进 project_search_paths」；启动横幅把逐库
  eprintln 归并成按项目码一段 + 每个未定位项目一句 + 搜索根清单，另打一句闭包摘要；`/health.mdb_closure`
  `{ roots, projects, seen_projects, unresolved, problems, declaration_error }`，有缺口时进
  `degraded_sections`（不拒绝启动）；按需生成的失败消息与 `/model/ensure` 的错误尾巴追加
  `explain_unregistered_dbs` 给出的路由目录原因。
- 向导第①步（D7）：去掉项目勾选框，改成「主搜索根 + 其它搜索根（一行一个，本地或 UNC）+ 主项目单选 + MDB →
  看闭包」；新探测 `POST /api/v1/setup/probe/mdb` 直接跑闭包（不依赖运行中的服务），页面画成
  项目码 / 目录 / 所在根 / 库数 / 影子的表，未定位项目码与不可达根红字；保存前有缺口要确认。
  `SetupConfigPatch` 不再写 `included_projects`（旧页面带过来也不落盘），改写 `project_search_paths`。
- 顺手：出厂模板 `room_membership = true` → `false`（与代码默认一致；开着时缺一个 DESI 整批模型根判死，
  向导又不暴露这个键），单测 `factory_template_room_membership_matches_code_default` 钉住。
- 补一刀：`project_locator::subdirs` 原先用 `entry.file_type().is_dir()` 判子目录，而 Windows 的目录联接（junction）
  与目录符号链接在这里报 symlink 不报 dir——搜索根里的项目目录只要是联接，整个根就扫成空。现在跟一步
  `path.is_dir()`，新钉 `a_project_folder_behind_a_junction_is_still_scanned`。
- 验证：隔离 worktree（`cfed5fcce` + 本批改动）`cargo test --lib` **2110 passed / 0 failed / 109 ignored**
  （含新钉 18 条：定位器 9、闭包 6、路由目录 2、向导探测 1），`cargo check --all-targets` 绿；
  `watch_dirs_probe` 对真盘 `D:\AVEVA\Projects\E3D3.1` 的闭包结果见 `docs/plans/2026-09-15-mdb-project-closure-and-room-default-dev-plan.md` 状态行。
  真部署（§5.3 三条）已在 `_runs\ams-0.1.25` 上跑过——换本批 release 二进制、删掉 `included_projects` 之后：
  声明落空 **40 → 0**、闭包 5 个项目 40/40 定位、1112 的「显示模型」（`17496/8518`）直接出 1,731 个实例；
  缺 ACP 时 `unresolved` 带 34 个 dbnum、`degraded_sections` 含 `mdb_closure`、按需生成的错误里点名「项目 ACP」；
  第二个搜索根命中 ACP@根#2；掉线 UNC（`\\10.255.255.1\...`）下启动 28 s 就绪不卡死。逐条见计划 §5.4。

### 本地发布 0.1.25：增量以落盘为边界、初始化双写道、CATA 闭包缓存与 ZONE 取根

- 后端 `0.1.25` 构建自 `0c6c62b3b5`（**干净树**，build_id 不带 `.dirty`），路径依赖 e3d-io
  `6431ee7120`；前端 plant-ui 升到 `0.1.11` 并**重建 wasm**，未复用旧包的 Web
  （`web_source = built-from-plant-ui`）。产物
  `releases/ams-local-0.1.25_release.20260915063626.g0c6c62b3b521-runtime-workspace.zip`，
  SHA-256 `490c3c4b1b241e44ac7a541c89fc2e2a5c5b947764d87aba3e1fe6e004ba68ba`。
  **这份 zip 出过两次**：首次打包（14:47，zip 哈希 `98807349c8d9…`）之后那个文件被本机另一路
  会话取走并删掉，只在 `releases/` 留下一个剥掉 `web\` 与 `workspace\` 的
  `aios-database-0.1.25.zip`。第二次用 `-SkipBuild` 从那份剥壳包里取回 `aios-database.exe`
  重新组包：exe 的 SHA-256 与首版清单逐位相同（`b3638d9f39ee…`）、`build_id` 不变，
  11 个 `web/` 文件也与首版清单逐位相同，**只有 zip 容器本身的哈希变了**（条目时间戳不同）。
  发布身份以 `build_id` 与 exe 哈希为准，两次是同一个二进制。
- 包型 `runtime-workspace`（带 `workspace\`、不带 `assets\meshes`），与 0.1.24 同档：
  assets 灌入 165 个文件 / 115.6 MB，meshes 排除 83,130 个文件 / 1,142.4 MB。
- 覆盖 0.1.24（`80bee35e65f2`，2026-09-14 出包）之后的 27 个提交。用户可见的四条主线写在
  `docs/release-notes/0.1.25.md`：增量任务以落盘为边界（ADR-076 S1–S3）、初始化双写道
  （ADR-077）、CATA 依赖闭包进程内缓存、ZONE 取根与池级元素缓存转正。**`/health` 因 `settle`
  整条退役少掉四格**（`settlements` / `settlement_state_by_dbnum` / `durable_target_by_dbnum` /
  `memory_target_by_dbnum`），读它们的脚本要改。
- 三腿冒烟全过：A 出厂配置起服务 `mode=setup` + 向导标记；B 本机配置全启动
  `medium=mem` + `durable=false` + 服务没拉 surreal 子进程；C 包内 `surreal.exe` 在 18010 起
  rocksdb 库、服务连上 `medium=rocksdb` + `durable=true` + 数据目录落 7 个文件。跑完
  9099 / 18010 / 18011 三个端口均空、临时冒烟目录已清。
- 打包前把工作树里两批在途改动提交掉（下面两条），并把一批只改了行尾（CRLF）的文件还原成
  索引里的 LF 版本，没有把行尾扰动带进历史。
- 验证口径写清楚：本轮跑的是 `cargo check --features http_api,rvm_verify`（干净）与发布脚本的
  三腿冒烟；**没有**跑全量 `cargo test --lib`，别把这次当成「全绿」。

### `parse.sessions` 的会话用户键改叫 `user`；`computer_name` 留作别名，**下一版（0.1.26）删**

- 那一格是 core.dll 的会话属性 `USER`（写这一会话的登录名，`+0x7C`，长度在 `+0x78`），从来不是
  机器名：rs-core 时代叫 `get_computer_name`，e3d-io 叫过 `machine`，2026-09-12 `c7cc6c8` 对照
  core.dll 改正为 `user`——只是有些登录名长得像主机名（`pdms-bj-c103-47`）。`8e86aa3af` 里
  `session.machine → session.user` 是跟着改名，输出的字节一个没变；一直叫错的是键名。
- Python 绑定 `aios_db.parse.sessions` 每行新增 `user`；`computer_name` 同值保留。`parse.pyi` 与
  `sessions()` 文档写明来历；`smoke_m1.py` 改读 `user`。仓内其余读者（`test_extract_tree_offline` /
  `testbed/spatial_tree_8000.py` / `run_full_loop.py`）只取 `sesno`，不受影响。钉
  `test_session_user_is_named_user_and_computer_name_is_its_alias`（issue-019 三份快照逐行
  `computer_name == user`）。
- **预告：`computer_name` 别名在下一个本地发布（0.1.26）删除**，之后会话页只剩 `user`；仓外脚本读
  `row["computer_name"]` 的请在此之前改成 `row["user"]`。
- 验证：`cargo check --manifest-path python/Cargo.toml` 干净；`maturin develop` 重建扩展后
  `pytest tests/test_parse_offline.py` 13 passed（12 旧 + 1 新）。

### 按 dbnum 点名的入口不受 `watch_dbnums` / `--debug-dbnum` 两道监听门管

- 那两道门收窄的是 watcher 自己扫出来的活；点名的库是调用方指定的，再关着只会把点名挡掉，
  报出来的还是「未找到 dbnum」——2026-09-15 现场：1112 不在 `watch_dbnums` 里，
  `initialize_project_dbnum_baseline` 找不到它，配置里因此多了一条把 1112 写进名单的绕法。
- `in_scope_with` 对 `UpdateScope::is_unrestricted()` 跳过监听 / 调试两道门；跨项目那两道过滤
  照旧生效（那是数据域，不是口味）。点名入口的「未找到」改成说得出现场：目录下扫到几个库、
  列前 24 个、带上扫描告警。`DbOption.toml` 撤回 1112，仍是 `[7998, 8000]`；CONTEXT.md
  「监听限定域」词条写明点名入口不在此列。钉
  `a_named_initialization_bypasses_the_watch_and_debug_gates`。

### RVM 基线对拍：面差报「谁多了面、多在哪」；名词第三种拼法（截 6 字）也认

- `surface_mismatch` 的 payload 加一格 `where`：双向合并会把方向抹掉，而方向正是要问的那件事——
  `rvm_to_gen` 大 = 我们少了材料，`gen_to_rvm` 大 = 我们多了材料；再给两侧最远的 5 个点与它们在
  对侧的最近点，成簇是一块局部差异、散开才是姿态或尺寸问题。
- `pe.noun` 实测有三种形态（2026-09-15 在 1112 上逐个量过）：原样（`GWALL`）、截到 6 字
  （`PLDATUM` → `PLDATU`）、字典短名词（`FTUBE` → `FTUB`）。只按 `full_noun_to_short` 数，
  前两种一个都对不上——1RS-WF03 的 118 个 `GWALL n of CWALL /…` 与 38 个
  `PLDATUM 1 of JLDATUM /…` 就是这么全数解不出来的。三种按序试、一次只认一种。钉
  `a_noun_capped_at_six_characters_still_resolves`。

## 2026-09-14

### 三维校审后台移植合入主线（ADR-075；plan 2026-09-12 T1–T5 · 落主线计划 2026-09-14 P1）

- `codex/review-backend-closure@1283e8856` 以 `--no-ff` 合入（`afa0ecb69`）：来源 `feature/review-backend-port@4400d8bab`
  × `e3d-direct@02d445ba4` × 主线 `54a7c8b9d`，外加合入前收尾 6 个提交（ADR 引用 072→075 改号 16 处、
  `review/db.rs` 行尾归一 LF、持久化闸在单测里活着、changelog 倒序、`DbOption.toml` 注释对齐主线、闭环记录改口）。
- 校审域四前缀 `/api/review` `/api/users` `/api/auth` `/files/review_attachments` 进 gen-model；37/37 条路由、
  21 个响应结构体与旧端逐字对照，前端零改动；10 张 `review_*` 表同住 `SUL_DB`（ADR-075 D1）。
- 持久化闸（d-398）：判据只认 `StoreMode::is_durable()`；出厂默认 `mem` 下四前缀一律 503
  `review.persistence.refused` + 启动横幅，生产启用校审前先起持久 SurrealDB 并改 `store_mode = "rocksdb"`；
  逃生门 `PLANT_REVIEW_ALLOW_EPHEMERAL_STORE=1` 只给演示。闸在单测里照常活着（d-410）：`platform::tests`
  夹具用 `cfg(test)` 进程级开关显式声明「知道会丢」，撤掉了 `#[cfg(not(test))]`。
- A2 / A5 / A10 与旧端对拍改用替代判据（d-408）：静态逐字对照 + gen-model 单边实跑 = PASS-静态；
  不再要求复活旧 `plant-model-gen`。
- `e3d-direct` 分支退场（d-412）：本地已删，兜底标签 `archive/e3d-direct-20260914 → 02d445ba4`，其 8 个提交
  （`New-LocalRelease.ps1 -ReuseWebFromZip`、回灌 8 处模型改动等）经本次合入进主线；主线唯一是
  `gen-model-refactor`，远端 `e3d-direct` 随落主线计划 P4.3 删除。
- 验证（合入树与 `1283e8856` 逐字节相同）：`cargo check --all-targets --features http_api` 干净；
  `cargo test --lib --features http_api` 2037 passed / 0 failed / 107 ignored；校审域 152 / 152；
  `integrated-acceptance.json`（`8537a9c3f`）A3 46/46、A6 PASS、A9 PASS；走查 U1 / U2 / U3 PASS；
  真模型 AMS 8000 `24384_22402` 在前端能开。
- 主线工作树 28 个非校审在途改动为合入暂存又即刻回放（3-way 零冲突），另存分支 `wip/mainwt-stash-20260914`（Q3）。

### 修：启动竞态——别的发现轮排下的排队行被新 epoch 遗弃，干等 300 s 周期重扫才动

ams7997 冷初始化实测（`verification/init-timing/run-7997-two-tier.py`，A 轮）：`/health` 一答话
（`initialization.status=discovering`）就 POST `/update/execute`，14 条行入队盖着 epoch 1；紧跟着启动序列自己的
发现轮 `begin_discovery()` 把共享 epoch 拨到 2、装上它自己的清单——`auto_initialize_new_dbnums=false`，从未初始化的库
它一条都不排（视时序，这份清单可以干脆是空的）。`allows()` 要 `epoch_id == 当前`，没被它再扫到的行从此
`blocked_by_phase:manifest`，worker 空转（看门狗 60 s 报 14 条 `AIOS-QUEUE-STALL`），直到 300 s 后
`[reconcile] 重扫（重建队列）` 重新盖章才开工：入队→开工 313 s，基线 12 s。plant-ui 上「服务刚起来就点执行」原样撞上。

- **收养（`batch_scheduler.rs` / `initialization_phase.rs`）**：`BatchScheduler::adopt_stale_rows(epoch)` 把仍在
  **排队**、盖着别的非零 epoch 的行改盖成当前 epoch，回执带 dbnum / 相 / 挂起位 / 任务号 / 原 epoch；
  `InitializationCoordinator::adopt_rows` 按回执补阶段 total / pending、重算当前相，有一条不是挂起的就拨到 `Running`
  （全是重扫排的挂起行则停在 `AwaitingTrigger`，不替人上弦），被阻断的相收养后仍 `Blocked`。两步分开是锁序
  （activation gate → queue → coordinator）的要求；`adopt_stale_rows_into(origin, epoch)` 一处收口并打一行日志。
  运行中的行不动（带着自己的 epoch 跑完，`finish` 按当前 epoch 对账），epoch 0 的旁路 / 测试行不动，目标 epoch 为 0 不收。
- **三处调用**：启动 / 周期重扫（`increment_manager`）与人工执行（`manual_update`）装完清单、排完自己的行各收一次；
  worker 每次 `drain_queue_until_empty` 开头再收一次兜底（活动闸内、在 `retire_covered_window` 收口迟到行之后、
  第一处出队之前）——出现错配当场重盖章，不等 300 s。
- **人工执行的 epoch 挪进活动闸（`manual_update.rs`）**：此前 `begin_discovery()` 开在逐库裁决那一圈之前、行边裁决
  边入队、圈后才 `install_manifest`——那一圈读水位、落观察带 `.await`，启动重扫只要插在中间，已排出去的行全盖着过期章，
  随后的 `install_manifest` 又因 epoch 对不上被整轮拒收。现在圈里只裁决、只收集（行先盖 `epoch_id: 0`），
  开 epoch → 装清单 → 入队 → 收养挪到圈后同一把活动闸内一口气做完，没有 `.await`，与 `increment_manager` 同序同形。
  副作用是好的：人工扫描期间协调器不再被拨到 `Discovering`，旧 epoch 的行照常出队。
- **测试**：`initialization_phase` 三条（收养重开数据相并即刻放行 / 全挂起行不上弦、混有真实触发则 `running` /
  epoch 对不上与 epoch 0 不收、阻断相收养后仍 blocked）；`batch_scheduler::adopting_stale_rows_restamps_only_queued_rows_from_other_epochs`
  （只改排队行、运行中 / 旁路 / 已是当前 epoch 的不动、挂起位原样带回、二次收养为空）；源码钉
  `manual_update::a_manual_execute_opens_its_epoch_inside_the_activation_gate_and_stale_rows_are_adopted`
  （收集 → 闸 → 开 epoch → 装清单 → 入队 → 收养的顺序、闸内无 `.await`、圈里不入队；重扫装完清单收养且闸内无 `.await`；
  worker 收养早于出队）。
- **文档**：CONTEXT.md「纪元激活门」补两条发现路的形状，新词条「过期排队行收养」。
- 验证：`cargo test --lib -- data_interface::initialization_phase data_interface::batch_scheduler data_interface::batch_worker`
  + 上述 `manual_update` 三条 **92 passed / 0 failed**。**复现轮实跑**（`run-7997-two-tier.py`，`INIT_WAIT_SETTLED=0`：
  `/health` 一答话 1.0 s 后就 POST execute，rocksdb 档，工作树二进制 `g54a7c8b9de64.dirty`）：入队→`initial_load`
  **12 s**（修前同形 A 轮 313 s；等启动收敛再执行的 B 轮 13 s），`AIOS-QUEUE-STALL` **0 条**（A 轮 14 条），
  `server.log` 一行 `[init] 收养 3 条别的发现轮排的排队行进 epoch=2：dbnum=7997(design)、8190(meta)、8191(meta)`——
  另 11 条 DICT / MISC 文件启动重扫自己也扫到、按「已覆盖」并入时顺手改了章，这 3 条正是
  `auto_initialize_new_dbnums=false` 挡在重扫清单外、修前干等 300 s 的那几条；任务 322 s / 全程 385 s，
  与 B 轮（304 / 369）、基线（349 / 382）同一噪声带，`succeeded`，无残留进程。结果目录
  `verification/init-timing/ams7997-rocksdb-race-fixed-20260914-031218`。

## 2026-09-13

### ADR-074：`store_mode` 收成 `mem` / `rocksdb` 两档，本进程从不拉起数据库；`store_process.rs` 整删

用户拍板（1a + 2a + 默认 mem）：**内存就在进程里，落盘就连配置指的那台；本进程不再拉起任何数据库。**
四档（`embedded-mem` / `spawned-mem` / `spawned-rocksdb` / `external`）里后三档的连接语句本来一个字不差，区别只在
「那台 surreal 是谁拉起的」——为此背着的 `store_process.rs`（约 1800 行：端口探针 / 署名 / 心跳 / 四格裁决 / 孤儿回收 /
收尸）随两档退役整个删除。

- **`options.rs`**：`StoreMode { Mem, RocksDb }`，字面值 `mem` / `rocksdb`，默认 `mem`；旧写法全部当别名认
  （`embedded-mem` / `kv-mem` / `spawned-mem` → `mem`，`external` / `spawned-rocksdb` → `rocksdb`）但 `store_mode_notice`
  启动时大声说清换成了什么——`spawned-rocksdb` 必点名「本进程不再拉起，请确认那台库已在跑」。`has_endpoint()` 删：
  「持久」与「别的进程连得上」两档下是同一个问题（`PersistMedium::is_reachable()` 同理并入 `is_durable()`）。
- **`lib.rs`**：`ensure_store_connected` 两分支——`connect_in_memory_store`（`mem://`，不 signin）/
  `connect_configured_store`（`init_surreal` 连 `v_ip:v_port`，连不上就地退出）；`ensure_own_store` 与 `startup` 参数删。
  单实例锁与机器级写者锁只有 `mem` 不拿。
- **向导** `web/setup.html` 第②步四档 → 两档：`mem` 默认、不用地址；`rocksdb` 须填地址并通过「测试连接」（连通 + 2.1.x）。
  「端口预检」按钮与 `workspace\.surreal\<代号>` 目录提示随 spawn 两档删除；读旧配置时把退役字面值折到两档显示，
  写出去只有新名（`validate_patch` 拒收退役值）。源码钉 `the_wizard_offers_exactly_the_two_store_modes_and_writes_only_new_names`
  同时钉「退役 value / return / 比较一处不留」。`ensure_completion_ready` 不再抄一份别名表，复用 `options::parse_store_mode`。
- **`/health.sul_db`**：`medium` 报 `mem` / `rocksdb`；`endpoint` 在 `mem` 下报 `embedded:mem`；`durable` 只对 `rocksdb` 为真；
  `in_memory_db` 栏为老客户端保留。水位镜像身份的 `endpoint` 同步改用 `embedded:mem`。
- **配置**：仓内全部 `DbOption*.toml` 夹具与出厂模板 `scripts/templates/DbOption.factory.toml` 改写新名
  （`external` → `rocksdb`，`embedded-mem` / `spawned-mem` → `mem`）。
- **脚本**：`New-LocalRelease.ps1` **两种包出厂都是 `mem`**（用户追加拍板「两档都真正解压即跑」）：出厂模板原样进包，
  runtime 档那三条按档位改写 `store_mode` / 注释的正则随四档退役；README 改说「要持久先跑 `Start-Surreal.ps1` 再在向导改选
  rocksdb」。冒烟 B 腿默认 `mem` 且断言服务没拉 surreal 子进程，C 腿改为**脚本**用包内 `surreal.exe` 在 18010 起 rocksdb 库、
  服务以 `rocksdb` 连它（凭据从配置 `v_user` / `v_password` 读）；生成的 `Start-AMS.ps1` 不再递 `AIOS_SURREAL_EXE`。`Verify-OutOfBox.ps1` 改考
  「一台库都不拉」（`medium=mem` + `endpoint=embedded:mem` + 无 surreal 子进程）。`Start-Surreal8009.ps1` 头部口径改两档。
  各 live 脚本 `AIOS_STORE_MODE=external` → `rocksdb`；`strt_plant_ui_runtime.py` / `Start-StrtPlantUiValidation.ps1`
  `embedded-mem` → `mem`。**`Run-KvMemRoomBootstrap.ps1` 退役**（开头直接报错说明）：它对着 `spawned-mem` 的端口发 SQL
  挑正例 / 反例，读透形态只剩没端口的 `mem`——要救回来须把挑选改走服务 HTTP 面。
- **文档**：新 ADR-074；ADR-070 作废；ADR-068 / ADR-069 状态行加修订指向；CONTEXT.md「本地发布包」「冒烟门」「设置模式」
  「配置向导」改写，「自带持久库」词条换成「存储形态 (Store Mode)」；`docs/database-storage-modes.md` 重写为两档 + 退役对照表。
- **后果要知道的**：`mem` 档没有端口——`/sql` 探针、`rvm_verify`、`Capture-*Evidence.ps1` 与 plant-ui「库供数」读面在这一档
  都够不着（plant-ui 按 `mirror.endpoint = "embedded:mem"` 自己降级）；要外部诊断就用 `rocksdb` 档。两种发布包出厂都不持久，
  要数据活过重启得自己起库再改选 `rocksdb`。
- 验证：`cargo check --lib --all-targets` 干净；`cargo test --lib -- options:: web_service::setup web_service::handlers
  data_interface::dbnum_state data_interface::database_writer_lock instance_lock` 156 passed；全量 `cargo test --lib`
  **1874 passed / 2 failed / 107 ignored**——两条失败均与本条无关、是工作树里别的在途改动
  （`model_refresh::one_bad_generation_root_does_not_charge_every_sibling_in_the_dbnum` 的 `failures_are_attributed` 重构
  没改源码扫描测试；`model_memory_store::room_spatial_accepts_large_incremental_pipe_like_cohorts` 的
  `room_spatial_rows_rebuilt` 改成累加后期望值没跟），未动。plant-ui 侧 `data::tests::embedded_mirror_never_falls_through_to_a_configured_tcp_store`
  等 2 条通过。全部改动脚本经 PowerShell 语法解析通过。**发布冒烟实跑通过**：用当前源码 release 构建
  （build `0.1.23+release.20260913165955.g86f4a0b9417f.dirty`）跑 `New-LocalRelease.ps1 -SkipBuild -IncludeWorkspace`，
  A 腿 `mode=setup` + 向导标记 ✓；B 腿 `medium=mem + durable=false + 服务没拉 surreal 子进程` ✓；C 腿包内 `surreal.exe`
  在 18010 起库 → 服务以 `rocksdb` 连上 → `medium=rocksdb + durable=true` + 数据目录落 7 个文件 ✓；`release.json.smoke`
  三腿全 `passed`，跑完 9099 / 18010 / 18011 均空、临时冒烟目录已清。

## 2026-09-11

### 本地发布 0.1.22：后端重建、Web 复用、E3D 版本显式配置

- 后端 `0.1.22` 构建自 `b1e4c878e`，路径依赖 e3d-io
  `2c6f11d23d`；plant-ui 保持 `0.1.10`，从三腿冒烟全过的
  `ams-local-0.1.21-full.zip` 复用 12 个 `web/` 文件，逐文件 SHA-256
  对拍无差异。发布清单新增后端/e3d-io 提交与 Web 来源整包哈希。
- SchemaPack 版本只认 `DbOption.toml.e3d_version`。`UPGDNO` 两版相同，
  `Saving Meta Data.` 只是会话注释，`COMM.VERS` 又会被 `clearComm`
  在客户端退出时清空；三者都只作诊断，不再猜偏移。
- `New-LocalRelease.ps1 -ReuseWebFromZip` 支持只重建后端，并强制校验来源
  release.json 的 plant-ui 版本、三腿冒烟状态与 zip SHA-256，避免把
  plant-ui 工作树的未提交内容夹进旧版本号。
- 严格冒烟 A（设置模式）、B（spawned-mem 正常模式）、C
  （spawned-rocksdb 持久模式）全部通过；端口与本轮临时目录已清理。
- 产物 `releases/ams-local-0.1.22-full.zip`，485,739,832 bytes，
  SHA-256 `c2c3c44a56a0c24241916634c0e1a63afb4b9772f4a59beac8f5bf3969f666cb`。
- 定向版本测试、`cargo check --lib` 与 Clippy 通过；全量 lib 仍有 9 个
  与本次发布文件无关的既有源码契约失败（1989 passed / 136 ignored），
  未把它们写成“全绿”。

## 2026-09-10

### ADR-072 R3 / R4 收尾：房间侧过渡壳删净、退役表名只剩迁移清单（W4 源码钉）、`/dbnums` 出 `write_state`、ops.html 退发布时代的格

分支 `refactor/retire-publish-stage` 到 `042ce26ea` 时 R1–R5 的主体已落（下表），这一刀收 R3 / R4 的残留，并把
ADR-072 W4「退役表名只在迁移里」钉成源码钉。

| 步 | 状态 | 落点 |
|---|---|---|
| R1 写入标记 + 回退 | 已落 | `8391fba0a` → `3f24cac56` → `54795e2e6`（d-570 不删 pe）→ `9e02ec077`（ModelOnly / RoomOnly 只退对应面；d-732 追认，ADR-072 D3 随 `183c4664c` 改写）→ `d127e336a`（mem:// 崩溃点重放） |
| R2 任务内去回读 | 已落 | `f2bdaa21c`（空间树就地收敛）、`1ac2d0f89`（房间门读任务结果）、`0025edaa4`（`model_complete` / `CompletionReceipt` / `verdict_for` 出执行体）；对账读 `existing_geometry_for_window` 按 §9-7 (a) 保留（d-568） |
| R3 删 publish | 已落 + 本条收尾 | `0025edaa4`；本条删 `room_topology.rs` 三个过渡壳 |
| R4 删第二条账 + 退表 | 已落 + 本条收尾 | `0025edaa4` / `042ce26ea`；本条补 W4 源码钉与它拓出的残留 |
| R5 退 `room_published_*` | 已落 | `1ac2d0f89` / `3deaa1f30` |
| R6 文档 | 大半 | ADR-072 / ADR-062 作废 / ADR-071 修订 / CONTEXT.md / spec §4.7 已落；spec §4.7.1 `window_blocks` 行「本轮不删」已过时（文件此刻在 t-704 手里，未改） |

- **R3 残留**：`room_topology.rs` 里为发布路保编译的 `render_stage_published_room_topology` /
  `stage_candidate_room_topology` / `ensure_published_room_topology` 三个 no-op 壳删，源码钉
  `room_topology_has_no_published_view_and_never_queries_the_database` 把三个名字加进禁用清单。分支
  `c2/retire-room-shells` 的 `47faa7f1e` 做的是同一件事，但它还改 `side_effect_pending.rs`（refactor 上已整删），
  并不进来——在 refactor 上重做，那条提交作废。
- **R4 W4 源码钉**（`lib.rs` 末尾 `retired_table_names_pin`）：扫 `src/` 下每个 `.rs` 的生产部分——`#[cfg(test)]`
  条目按花括号配对整段摘掉、整行注释去掉、父模块以 `#[cfg(test)] mod x;` 声明的整文件测试模块跳过——六张退役表名
  （`model_update_pending` / `incr_side_effect_pending` / `published_file_input` / `increment_update_attempt` /
  `room_published_inst` / `room_published_topology`）只许出现在 `retire_publish_run_tables` 的 RETIRED 清单里。
  `room_topology_source` 不在名单：那个名字同时是磁盘分片模块（d-89），表退了、模块留着。钉拓出并收掉的残留：
  - `dbnum_state.rs` 启动 `DEFINE TABLE` 仍建 `increment_update_attempt` / `model_update_pending` /
    `incr_side_effect_pending` 三张（`REMOVE` 完又 `DEFINE` 回来）——三行删；
  - `bin/l3_suite.rs` + `bin/l3_suite/fixture.rs` 四处 SQL：首次导入后的「积压重置」删（模型随任务同步生成，没有积压）；
    `wait_fixture_pending` 改读 `dbnum_watermark` + `dbnum_write_state`（不 `writing` 且 `model_sesno == applied_sesno`
    连续两次即收敛，证据落 `{phase}-settlement.json`）；快照 SQL 去 `pending` / `room_pending` 子查询，I-2「模型欠账」改按
    双水位判；`wait_model_drain_settlement` 不再问已删的 `/update/pending-units`；
  - `model_type_scope.rs` / `watch_scope.rs` 两条日志文案里的「留在 model_update_pending 里不消费」「遗留的
    model_update_pending 已清理」改成任务口径。
- **编译面**：HEAD 上 `cargo check --bins --tests` 编不过——`bin/node_gen_room_probe.rs` 与
  `tests/issue7_e2e_increment.rs` 仍 `use model_update_pending::drain_rooms`。探针去掉 drain 路（房间归属由该库任务的
  房间段整库算，探针只汇报库里现有的归属边）；issue7 e2e 整文件删（它驱动的就是 drain 路）。这三处 21:04 已以未署名 WIP
  躺在共享工作树里，随本条一起提交。
- **契约 §7 / Q13**：`/dbnums` 每行加 `write_state`——`DbnumStatus.write_state: Option<WriteStateColumns>`
  （`status / start_sesno / end_sesno / intent / started_at / finished_at`，`skip_serializing_if` 缺失即整格不出、不写
  `null`）；取数 `write_state::load_all()` 整表一趟（§4.5 白名单里 `dbnum_write_state` 那一格的读法），读不到只记
  warning、整格留空。钉：`the_dbnum_row_carries_the_write_marker_as_one_optional_cell`。
- **ops.html**（spec §4.7.1 / 附录 C.1「对应格删」）：删「窗口阻断 · 数据侧终态」卡与 `renderStaging`
  （`/health` 已无 `window_blocks` / `staging_windows` / `staging_commit`）、删「重试模型死信」按钮与
  `model_update_pending` 读取、删 `spatial_reconcile` / 模型死信两级判决与 `mupPhrase`。换上写入标记读法：判决多一档
  「未写完」（`write_state = writing` 且该库不在运行行 = 上一次死在写库中途，点名 intent / S→T / 开工时刻），KPI
  「待重试 / 死信」改「写入中 / 未写完」（`/dbnums` 读不到报 `? / ?`，不报 0）；`MAX_ATTEMPTS` 注释改指
  `batch_worker` 的进程内连败账。
- 验证：`cargo check --lib --bins --tests` 0 error（aios-database 0 warning）；全量 `cargo test --lib -- --skip damaged_`
  **1803 passed / 5 failed / 115 ignored**——5 红 = 基线既有（`pe_batch`×2、`window_root_plan`×3），无新增
  （HEAD `0025edaa4` 自报 1802 / 5 / 115）；`cargo test --bin l3_suite` 的 4 条 payload 归一化用例过；ops.html 两段
  `<script>` `node --check` 语法过；rustfmt 本条改动的 hunk 干净（这些文件里既有的 4 处偏差不动）。**未验证**：live 7997
  移动 / 恢复重放（计划 §6-3，R5 后一次）；ops.html 在真服务上的渲染。
- **未收**：`l3_suite` 的 I-3 仍按 `model_drain` 任务判——那一族任务随 drain 退了，L3 套件的模型断言要按任务模型重基，
  不在 R4 内；`9e02ec077` 自报的 `requeue_after_rollback` 用 `report.target_watermark` 填 `applied_sesno`（ModelOnly 回退
  后应读库里的 applied）仍待对齐。

### ADR-072 已接受：发布阶段退役——任务写库带写入标记，中断按 sesno 回退到水位重做（计划已批准；R1–R6 进度见上一条）

grill-with-docs 两轮（Q1–Q15 全按推荐，共识 d-441）+ Plannotator 批准。口径：一个 dbnum 任务跑完把内存结果**同步**写库、最后盖水位；
写库期间 `dbnum_write_state:{dbnum} = writing{S,T}`，收尾那一笔与水位同事务置 `finished`；启动 / 开工见 `writing` → 按 `(dbnum, sesno > S)`
谓词删除整库回退到 S → 排 S→T 从头跑（初始化 S=0 即 wipe）。publish 九子步、任务内四处回读库、`model_update_pending` /
`side_effect_pending` / `model_publish_queue` / `model_completion`、`room_published_*` 三表一次清干净；对库的读只剩水位 / sesno / 写入状态
与启动水化。文档：`docs/adr/ADR-072-…`、`docs/plans/2026-09-10-retire-publish-stage-and-write-marker-plan.md`；ADR-062 作废、ADR-071 S3 / D4 / D9
修订；CONTEXT.md 新增「写入标记」「回退到水位」「库读白名单」，改写「任务内序列化」，退役「模型窗口意图」「提交后收敛」。
worktree `gen-model-refactor` / 分支 `refactor/retire-publish-stage`，基线 `6f0813f6b`（e3d-direct 上 ADR-071 S1–S6 在途的整份 WIP 提交）。

### S4（目录级联那一块）：反查命中的根改排带明确根名单的 ModelOnly 任务，目录版本核对搬进 `model_coverage.rs`（ADR-071 D1 / S4）

目录级联（`reconcile_catalogue_versions` 点名 → 收窗 → 反查 → 排工）此前的「排工」是把命中的根写成
`model_update_pending` 行、`wake()` 一下等空闲轮 drain 领——那是 ADR-071 要拆的第二条队列。现在它排的是任务：

- **队列行随身带名单**：`DiscoveredBatch` / `FrozenBatch` 多一格 `regen_roots: Vec<RegenRoot{refno, noun}>`
  （空 = 由规划器按凭证定，绝大多数入队）。并入已排队行时名单取并集、去重（`RowMeta.regen_roots`），冻结时整份进
  `FrozenBatch`。**「已覆盖」不能吞名单**：级联的 ModelOnly 以 `model_start_sesno = 目标 = 文件最新` 入队，区间语义上
  什么都不用做，`batch_queue::enqueue` 于是判 `AlreadyCovered`、不排行；`BatchScheduler::enqueue` 在这一档给它排一条
  退化区间 `[T, T]` 的 ModelOnly 行接在运行行之后（运行中的批次工作单在冻结时就定了，不会重算这几根）。
- **任务体并进工作单**（`batch_worker::explicit_regen_units`）：ModelOnly 体 `merge_unit_worklist(名单, 待重试)`，
  ApplyWindow 体（级联并进了一条数据窗口时）把名单接在本窗新单元后面；生成时点取任务目标会话，`attempts = 0`、
  无队列 revision，与规划器的待重试按 `(dbnum, root)` 去重。下限 = 目标 → `reconcile_startup_window` 不开窗
  （凭证等于右端即追平），进工作单的只有名单与本就未安定的根，不会把整库拖进来。
- **`enqueue_catalogue_cascade`**：按设计库聚成 `refno → noun`，每库一条 `DiscoveredBatch{ModelOnly, regen_roots}`
  经 `BatchScheduler::enqueue`；账目（`enqueued` / `enqueued_degraded` / `already_current` / `lazy_roots` /
  `skipped_out_of_scope`）与 `/health.catalogue_reconcile` 的形状不变。`enqueue_plan` 在这条路上不再出现。
- **搬家**：目录版本核对整块（`CatalogueReconcile*` / `reconcile_catalogue_versions{,_after_sweep,_now}` /
  `catalogue_reconcile_snapshot` / `CatalogueSweepVerdict` / `catalogue_sweep_verdict` / 收窗、反查、排工与
  `roots_behind_catalogue_on`，962 行 + 5 条测试）从 `model_update_pending.rs` 搬进 `model_coverage.rs`——它现在
  一行 pending 都不写，与那张表无关。调用点改路径：`startup_reconcile` / `increment_manager` / `handlers` /
  `reverse_reference`；`cata_dependency` 那条「排完根再前移依赖行」的源码钉改认新文件。`model_update_pending.rs`
  12,344 → 11,257 行。还留在那边写 pending 的只剩 `sync_and_seed_model_coverage`（`/model/rebuild`）。
- **钉**：`batch_scheduler` 新增「已覆盖也为名单排行 / 并入取并集去重 / 冻结整份带走 / 无名单的普通发现照旧
  AlreadyCovered」；原来的 5 条级联测试随代码搬家。
- 验证：`cargo check --lib --tests --bins` 干净；全量 `cargo test --lib -- --skip damaged_` **1982 passed / 0 failed /
  136 ignored**（10 s）；`rustfmt --check` 改动十一文件干净。**未验证**：live（改一个目录件 → 反查 → 看 `/tasks` 里
  是否出现 `model_only` 行、根被重算、`/health.catalogue_reconcile.enqueued` 计数）。

### S2 后半：Ref0 统计从目标索引序列化，逐笔 ±1 观测块退役（ADR-071 D6 / ADR-039 #3）

`dbnum_info_table.count` 此前由 `dbnum_stats::render` 的观测块逐笔维护：每笔元素写都带 `$before_` / `$after_`
两次点读、一段 `FOR $row IN $observed` 解释器循环、每个 Ref0 一次热点键 UPSERT，口径是「pe 行数含墓碑」。
ADR-071 D6 把它改成任务收尾**从内存序列化**、`count` = 目标会话上的**活元素数**；S2-a 落了渲染器
（`task_end::Ref0Snapshot`），生产者一直欠着（`batch_worker` 里 `info: None` 那一句）。这一刀补上，并把观测块拆掉。

- **生产者 `data_interface/ref0_snapshot.rs`**：`snapshot_at(source, end)`——`FrozenSource::reopen_verified_at(end)`
  （路径身份 + 会话链复核，文件被换过就在这里报错）→ `ReadOnlyEngine::indexed_refnos()`（`DbSet@end` 的 RefNo 树只列
  活元素，一趟遍历 24 B / 元素，零库读）→ `fold_ref0_stats`：按 `word0` 分组数、取最大 `word1`。`sesno` 取 `end`：
  逐元素的最后修改会话不在索引里（要走全部会话 delta），而这一列唯一的消费者是兼容回退水位
  （`dbnum_state::LEGACY_INFO_WATERMARKS` 取 `math::max(sesno)`），「数据序列化到了哪一版」正是 `end`。
- **写点**：`batch_worker` 在数据批次应用到 `committed_end_sesno` 之后、模型发布之前，单独一笔
  `TaskEnd{ model: None, rooms: None, info: Some(snapshot) }`（`spawn_blocking` 取快照 → `commit_task_end_on`）；
  库类型不限（CATA / SYST 的统计也在这张表里，此前只有 DESI 才走收尾）；ModelOnly 不动数据、不做。失败只留痕、
  标 Partial：数据行与水位不受影响，等下一次该库任务再序列化。模型 / 房间那一笔照旧 `info: None`——统计描述的
  是数据，模型失败不该让它停在上一版。
- **观测块退役**（`dbnum_stats.rs`）：`render` 只校验 `ids` 规范性、正文原样返回（末尾缺分号补一个——老观测块一直替
  `read_through` 那类调用方兜着这一下）；`batch_rows` 不再每 Ref0 +1；`render` / `transaction` / `batches` 签名不动，
  十来个调用方零改动。`rebuild_body`（`bin/rebuild_dbnum_stats` 与基线收尾用）改 `FROM pe WHERE … AND !deleted`
  ——与快照同为活元素口径，ADR-039 #3「二选一不并存」里两条路终于同一口径（审核项 I6）。
  **必须同一刀**：观测块的一致性闸 `IF ($old.count) < array::len($before) THROW` 拿的是「含墓碑」口径，快照写进
  活元素数之后它会在下一窗对着墓碑行 THROW，把数据窗口打死——两个写者不能并存一分钟。
- **钉**：`ref0_snapshot` 纯函数口径 + 真文件（ams1112，装了样例才跑）「count 之和 == 索引树元素数、max_ref1 ≥ 树里
  同 Ref0 任一 word1、sesno 全等于目标、按 Ref0 升序、有无冻结源同一份」；`dbnum_stats`「render 原样 / 补分号 / 拒不规范
  id / 生产正文无 `LET $before_` 等四个字眼」「rebuild 只数活行、历史 Ref0 留行清零、幂等」；`batch_worker` 收尾顺序钉
  加「快照在数据批次之后、模型发布之前、不等模型」；`element_batch` / `cata_dependency` / `read_through` /
  `mirror_spool` / `direct_window_replay_parity` 五处「写事务带观测」的钉翻成「写事务不碰 `dbnum_info_table`」。
- **没退、留给 S7**：`StatisticsMode` 枚举（`versioned_db::baseline` 还按它选，两档现在无差别）；`execute_on`
  每笔的 `require_no_event_on`（`INFO FOR TABLE pe`）；`baseline_ingest::settle_statistics` 的 `rebuild_on` +
  `pe_count == info_count`（基线现在也走同一笔快照，rebuild 是同口径的第二个写者）；`classify_stats_settlement` /
  `baseline_stats_need_rebuild` / `seed_suspect_dbnums` 三处对账。
- 验证：`cargo check --lib --tests --bins` 干净；全量 `cargo test --lib -- --skip damaged_` **1981 passed / 0 failed /
  136 ignored**（10 s）；`rustfmt --check` 改动九文件干净。**未验证**：live（7997 一窗收尾多一次索引遍历 + 一笔 info
  事务，预估毫秒级；`/dbnums` 的 `cached_pe_rows` 直接数 `pe`，不受口径变化影响）。

### 修：`/health` 两个常亮的假警报（空间树标签停在冷启动那一刻 + 房间凭据属于已退役机制）

三轮 live 里 `/health` 一直报 `spatial_tree.state = ready_empty` 与 `room_build.rebuild_required = true`，
而同一份 payload 里写着 `entries = 52346`、`resident.status = ready` / 215 间房 / 230 块面板 / 0 块无几何。
两个都是标签问题，不是数据问题——但它们让 run 守望脚本的 DONE 判据**永远不成立**，07:23 那轮就是这么
空转到 150 min 超时的。

- **空间树标签跟着树走**（`spatial_state::refresh_ready_kind`）。`ReadyEmpty` 的意思是「校验成功且库里
  没有可用几何指针，语义上无候选」；冷启动打在空库上时它是对的，可紧接着的初始化就把 5 万多条几何
  发布进树里，标签却没人翻。现在每次空间发布收敛（`promote_state_after_replay`，三个调用点共用）
  都把 `Ready` ⇄ `ReadyEmpty` 这一格对齐树。**只在已就绪的那两档之间翻**：`Loading` / `Rebuilding` /
  `ReplayRequired` / 两档 `Degraded` 各自管自己的迁移，一格不碰；也不盖验证时刻、不清 `last_error`
  ——这不是一次校验。要紧的是 `vacuous_room_build_reason` 正是拿 `ReadyEmpty` 判「全量重建无事可做」的。
- **房间凭据不再被渲染成「欠一次重建」**。那枚章由「项目级启动全量房间重建」盖，而那套机制已经退役
  （`lib.rs`：「房间计算采用逐 dbnum 实时内存模式；启动项目全算与数据库计算输入已停用」），
  `stamp_room_build` 的两个调用点都在退役了的启动臂上——现役形态下**永远盖不上**。`/health` 因此
  改成只报**持久标志本身**（`render_room_rebuild_required`：数据窗口尾事务在结构枚举失败时写、
  只有成功的全量重建清除，这一条一格没动）；章在不在另开一格 `stamped` 如实报，没有藏起来。
  读不出 / 超时那两支照旧 fail-closed 报 true——「不知道」与「没人要求过」不是一回事。
  **启动那一侧的判据（`room_build_record_verdict`）不跟着改**：它决定要不要真跑一次重建，
  「没建过就跑」是那里该有的保守。
- **`scripts/Watch-InitRun.ps1`**（新）取代各 run 目录里那份一次性的 `monitor.ps1`。旧脚本除了上面两条，
  还读着三处已经不存在的路径：`room_build.status`（房间结论在 `room_build.resident.status`，
  `room_build` 这一层从来没有这一格）、`model_publish_queue.*` 与 `local_runtime.*`（随发布队列 /
  backup lane 一起删了）；另有一处 PowerShell 的坑——`$Dbnums | Where-Object { … [int]$_.dbnum -eq $_ … }`
  内层 `$_` 遮住了外层库号，那个判据实际恒真。新脚本逐格取值（缺字段读成 `$null` 不会报错，
  得先确认它真的在），并把「三水位齐」也纳入判据。
- **钉**：3 条——标签只在两档就绪态之间翻且不动验证时刻 / 错误（其余六档逐个断言不被翻）；
  每一次空间收敛都要刷这一格（源码钉在 `promote_state_after_replay` 体内）；章不在时
  `rebuild_required` 跟持久标志走，而持久标志说要重建时照样报 true、读不出时仍 fail-closed。
- 验证：全量 `cargo test --lib -- --skip damaged_` **1979 passed / 0 failed / 136 ignored**；
  `rustfmt --check` 干净。live：新二进制重启到 12:30 那一轮的库上，`/health` 现在是
  `status=ok`、`spatial_tree.state=ready`（52,346 条）、`rebuild_required=false`、
  `room_build.resident.status=ready`。
- **第三轮冷启动整条走通了**（13:37 新 run `20260910-133645`，端口 18622 / surreal 18609）：守望脚本
  `13:48:58 DONE after 11 min` 自行判定并退出（写下 `final-health.json`），七条判据无一豁免。
  中途逐帧看得见标签跟着树走：开跑时 `spatial=ready_empty/0`，模型阶段中 `spatial=ready/4702`，
  终态 `spatial=ready/52346`——正是这一刀修的东西。终帧两库 `applied=file=model=room`、
  `data_batch:succeeded=15 running=[]`、6,787 根 0 失败。

### S7 前置（续）：`gen_root` 凭证的只读取数搬出 `model_update_pending.rs`（`data_interface/model_coverage.rs`，ADR-071 S7 B3）

`model_update_pending.rs` 里有一串**只读 `gen_root`、一行都不碰 pending 表**的取数——它们是凭证真值
（d-227：`gen_root` 凭证留库）的正经读法，与队列无关，却要跟着队列表一起被删。搬出来，名字与行为不变
（同 S5a 搬 `model_watermark.rs` 的做法），最后那一刀就是纯删除：

- **搬到 `model_coverage.rs`（455 行 + 3 条测试）**：按需 `ensure` 的凭证命中判据 `generation_root_cache_current` /
  `generation_roots_cache_current` / `render_generation_root_credentials` + 目录落款那一半 `CatalogueStamp` /
  `catalogue_stamps_current`；启动一级过滤 `startup_coverage_floors(_on)` / `StartupCoverageFloor` /
  `backfill_gen_root_dbnum_on`；空闲轮模型门 `model_coverage_current`；`coverage_tracked_dbnums(_on)`；
  已发布根的目录落款 `published_catalogue_sessions_on` / `stale_catalogue_dbnums`。调用点改路径：
  `startup_reconcile` / `on_demand_model` / `batch_worker` / `model_rebuild`；`model_update_pending` 自己的目录版本核对
  改 `use` 新住址；三条源码钉（`window_root_plan` 的凭证 > 0、mup 两条以 `model_coverage_current` 为边界的）改认新文件 /
  新边界；`reverse_reference` 那条 live 测试的 `use` 改指。
- **没搬、为什么**：`sync_and_seed_model_coverage`（把缺凭证的根 `render_upsert` 进 pending）与目录版本核对
  `reconcile_catalogue_versions*` / `CatalogueReconcile*` / `catalogue_sweep_verdict` / `roots_behind_catalogue_on`
  ——后者经 `cascade_stale_catalogue_windows → enqueue_catalogue_cascade → enqueue_plan` 排 `RegenRoot`，**写队列**。
  上一条把它们记成「不含 pending 内部引用」是只看了函数本体没看被调链，这里纠正：它们随 S4 把「排 pending 行」改成
  「排 ModelOnly 任务」时再搬。`record_id_of` / `source_time_for` / `root_identity_key` 是队列行身份的事，留在队列那边。
- 验证：`cargo check --lib --tests --bins` 干净；定向 7 passed；全量 `cargo test --lib -- --skip damaged_`
  **1979 passed / 0 failed / 136 ignored**（10 s）；`rustfmt --check` 改动九文件干净。**未验证**：live。

### S7 前置：搬家与死格——`digest` / 数据窗口落库口搬出 `model_publish_queue`，删零引用死代码与 `/health.increment_mode`（ADR-071 S7 A+B）

S7「删文件」这一步现在还整刀下不去：`model_update_pending.rs`（12,826 行）里 73 个 pub 项仍被外部调用
——它今天**仍是活的工作队列**（非 regen 项、房间目标 `RoomRecalcPanel` / `RoomRecalcElement`、延迟文件
模型动作、drain 页、`retry_pending_unit`），`side_effect_pending.rs` 剩下的全是文件轮次 / 空间对账 /
`published_file_input` 清单。这正是 d-73 的约束（退表不得先于房间进 Task），随 S3 后半 / S4 后半走。
这一刀只做**现在就能做**的两档：把活的从要退的目录里搬出去，删真死的。

- **搬**：`model_publish_queue::digest` → `data_interface/digest.rs`（SHA-256 小写十六进制，口径钉
  `digest_is_plain_lowercase_hex_sha256`——房间输入版本 / 目录版本令牌 / `manifest_key` 都靠它不漂；8 处调用
  改路径）；`model_publish_queue/{structured_data_sink,increment_window_artifact}.rs` → `data_interface/window_landing/`
  （`mod.rs` = `land_increment_window` 同步落库口，`artifact.rs` = `IncrementWindowArtifact` 产物）——它们是
  去账本之后的**生产路**，只是还住在叫「发布队列 / sink / artifact」的目录里；`apply_one` 的调用与四条源码钉
  （`the_data_window_lands_…` / `apply_one_has_no_staging_fork` / `cache_invalidation_runs_…` /
  `all_three_write_points_…`）改认新住址。`model_publish_queue/mod.rs` 只剩根发布信封（`FrozenRootEnvelope` /
  `PreparedRootPublication` / `RootIntent` / `render_spatial_fragment`）。
- **信封分支不是死的**：`persist_root_for_file_round` 仍被 `batch_worker.rs:2025`（文件轮次发布路）调，
  `frozen: Some(..)` 在 batch_worker / increment_pipeline 也还在——S5 条目里「生产已无人走到」那句不准，
  它随 S3 后半（去文件轮次）一起删。
- **删**：`model_update_pending::has_dead_data_work`（空闲轮的 `has_dead_work` 已随死信扣门退役，探针零调用；
  钉 `dead_letters_no_longer_gate_the_model_phase` 翻过来）；`SideEffectCompensator::mark_file_rooms_released_for_project_rebuild(_on)`
  （零引用）；`/health.increment_mode`（只剩 `direct` 一个值，plant-ui / ops.html / spec 都不读；`batch_worker::increment_mode`
  与它的钉一起删，钉改成 `the_direct_increment_switch_family_stays_retired`）；`web/ops.html` 的「复活副作用死信」
  按钮（端点已删、打 404）与两处 `side_effect_pending` 读格，「停摆」判决只剩空间对账那一支。
- **计划文本纠正**（用户拍板）：v3 S6 / S7 把 `room_published_*` 列进了 `REMOVE TABLE`——`room_published_topology` /
  `room_published_inst` 是 ADR-067 D1 房间参照的库侧已发布物，**不退**，已从清单划掉；CONTEXT.md「已发布房间几何」
  词条随之不退。
- **留下的**：`model_coverage.rs` 那一搬（`startup_coverage_floors(_on)` / `StartupCoverageFloor` /
  `backfill_gen_root_dbnum_on` / 目录版本核对 `reconcile_catalogue_versions*` + `CatalogueReconcile*` /
  `catalogue_stamps_current` / `roots_behind_catalogue_on` / `model_coverage_current` / `generation_root(s)_cache_current`
  / `root_identity_key`，约 560 行不含 pending 内部引用；`record_id_of` 与 `sync_and_seed_model_coverage` 写 pending 行、
  留在原处）——下一刀。`MAX_ATTEMPTS` 63 处随 pending 表退。
- 验证：`cargo check --lib --tests --bins` 干净；全量 `cargo test --lib -- --skip damaged_` **1976 passed / 0 failed /
  136 ignored**（11 s；-2 随死代码删的测试 +2 新钉）；`rustfmt --check` 改动文件干净（`model_db_adapter.rs` 那一处
  `GEOMETRY_BASELINE` 的格式差是别的会话在途的）。**未验证**：live；ops.html 只做了静态改动没开浏览器。

### S6：启动统一水化段收口——镜像整表水化只做一次、取数源路由按 D8 改「三水位相等」（ADR-071 D7 / D8 / S6）

S5 把 `recover_before_cleanup` 换成 `hydrate_model_projections` 之后，`run_cli` 里已经长出了统一水化段
（同日 12:0x，那一段没进 changelog，这里一并记）；这一刀把它收口。

- **统一水化段的顺序**（源码钉 `startup_hydrates_mirrors_once_after_seeding_and_before_the_model_reconcile`）：
  状态表播种（ADR-001 `ensure_increment_state_storage`）→ 兼容播种（ADR-060 D10 `seed_legacy_model_watermarks`）
  → **开镜像 → `DbnumState::hydrate_watermark_mirror`** → `hydrate_model_projections`（每个 `model_sesno > 0`
  的 DESI 库 `restore_projection_on`，连带每根 `RootReceipt`）→ `retire_publish_run_tables` → 启动模型核对
  （ADR-060 D6 双向对账在里面，读刚水化的镜像、写后逐格作废）→ worker / watcher。播种排在水化**之前**
  （审核项 B3）：接管的历史库才带着 `model_sesno > 0` 进镜像；`run_app` 不得抢在前面水化投影。
- **镜像整表替换只在启动做一次**（新 `hydrate_watermark_mirror` / `hydrate_watermark_mirror_on`）：
  一句读全水位行 + 一句聚合 `dbnum_info_table` 旧水位，每格与 `read_row` 按需装载时**同一口径**（含
  `info_max` 兼容回退），单次 `mirror::replace` 发布。`list_registered` **不再碰镜像**——它被 `/dbnums` 轮询、
  项目扫描、房间恢复在运行期反复调，每次整表替换会有「SELECT 已发出 → 某任务落库并作废那一格 → 用 SELECT
  之前的旧行整表盖回去」的窗口，那一格从此陈旧且再没人作废它。`gen_root` 凭证的整库口径（安定下限 /
  未安定根）不另建镜像：启动核对与规划器按需读库里刚落的那一份（d-227 的镜像只覆盖水位行）。
- **一次性迁移** `retire_publish_run_tables`：`REMOVE TABLE IF EXISTS` × 6（`model_publish_run / _fragment /
  _ack / _database / _error / _superseded`，S5 后零写者），幂等、失败只出声。`model_update_pending` /
  `incr_side_effect_pending` / `published_file_input` / `room_published_*` **仍有写者**，随 S3 后半 / S4 后半 /
  S7 各自那一刀退，不在这里。
- **取数源路由按 D8 改「三水位相等」**（`model_read_route::route` 多一个 `room_sesno` 入参）：
  `Database ⇔ ingest ∧ applied_sesno > 0 ∧ 该库无数据批次任务在飞 ∧ model_sesno == applied ∧ room_sesno == applied`；
  新理由 `room_lagging`（数据与模型到了 T、房间那一枚没盖上），优先序 在飞 > `model_lagging` > `room_lagging`；
  跑到数据前面不算落后（M2 违反另有对账管）。`route_for` 三枚水位同一次读镜像那一格。`model_lagging` 是
  12:0x 那一段加的（数据到 T、模型没追上 → memory），当时没记，这里一并记。
- **`/dbnums` 的 `annotate_model_source`** 喂三枚水位；**没有判决列的行不判落后**（判决列只给 DESI 行，
  普查失败时整格缺席）——「不知道」不能写成 `model_lagging`。这也修掉了别的会话在途那条红测
  `dbnum_rows_carry_their_model_source_from_the_single_predicate`（夹具没跟上 `ModelLagging`）。
  spec §4.12 同步改口。
- **钉**：`startup_hydration_mirrors_every_registered_row_field_by_field`（`mem://`：写库 → 水化 → 镜像每格
  与库行逐字段相等、`info_max` = `math::max(sesno)`、不经作废的库改动读不到、作废后回库）；`model_read_route`
  新增「房间落后单独一条理由」「模型落后先于房间落后」「五个理由字面值不撞」；`run_cli` 两条源码钉改认
  `hydrate_watermark_mirror`，并钉「镜像整表水化只做一次」。
- **留下的**：① 任务跑完翻成 `database` 的 `model_source_changed` 事件自 S5 起没有发送者了
  （原来在 run 收口里），plant-ui 靠轮询 `/dbnums` 兜着；接线点在 `batch_worker` 的 `commit_task_end_on`
  之后（那一段的回执 `Ok(tail)` 目前也没被消费），随 S3 后半那一刀补。② `wait_for_model_ready` 的进度源仍是
  空闲轮的 `model_coverage_current`（读 `gen_root`）与 pending / 副作用判据，随 S4 后半 / S7 走。
  ③ `reconcile_model_coverage_at_startup` 里那次 `list_registered` 是启动的第二次整表读（只读，不再替换）。
- 验证：`cargo check --lib --tests --bins` 干净；全量 `cargo test --lib -- --skip damaged_` **1976 passed /
  0 failed / 136 ignored**（11 s）；`rustfmt --check` 改动四文件干净。**未验证**：live（重启后 `/dbnums`
  三水位与重启前一致那一条要下一轮冷启动看）。

### 模型阶段的第二处 O(N·M)：CATA 依赖闭包收成一个任务一次（ADR-071 D1）

模型根发布前的目录依赖块（`cata_dependency::persist_for_design_subtree`）是**逐根**调的，而目录链
海量共享——同一个规格、同一个元件，几千根管子都指着它。7997 那一轮把日志聚出来：

| | 实测 |
|---|---|
| `[cata_dependency]` 调用 | **2,469 次**（6,787 根），平均 131 ms，最大 1,148 ms |
| 写入行数 | **387,132 行** |
| 而这个库整份闭包 | **15,500 行**（数据阶段基线已经一次性落过） |
| 合计 | **323.7 s**，占 7997 模型阶段 628.7 s 的 **51.5%** |

每一次调用还都重建一遍 `InMemoryCataLocator`（扫目录文件身份 + 依赖清单）。25 倍的重复写的是
`Fresh`（`INSERT IGNORE`）——语义上全是空转，代价却是实的。

- **改法与对账基线同一条纪律**（ADR-071 D1：任务是这个 dbnum 唯一的写者，**它知道自己写了什么**）：
  `CataSeedMemo` 记住本窗已经落过库的 seed，作用域绑在数据窗口（`active_data_window()`）上，换窗即
  失效；一根的 seed 全是老面孔时**连定位器都不建**，直接回一份「N 个 seed，没有新的」。
- **只在写成功之后记**：`?` 早退的那一根不能把自己的 seed 记成写过了，否则整窗剩下的根都会跳过它，
  缺行要等下一个任务才补得回来。窗口不在（`/model/ensure` 那类一根一调的路）一个都不记，照旧每根解一次。
- **两个写者并发**（`root_inflight` 最多 2）时最坏只是重复做一遍已经幂等的事，不会漏。
- **钉**：两条——记忆只覆盖一个数据窗口（同窗跳过 / 混着新老只解新的 / 换窗重来 / 无窗不记 / 作废归零）；
  `remember_seeds` 必须排在落库之后且空集要早退。
- 验证：`cargo check --lib --tests` 干净；全量 `cargo test --lib -- --skip damaged_` **1972 passed /
  0 failed / 136 ignored**（另有 1 条**别的会话在途**的红，见下）；`rustfmt --check` 干净。
- **live 已复跑坐实**（12:30 冷启动新 run `20260910-122504`，端口 18422 / surreal 18409，空 rocksdb）：

  | 项 | 07:23（改造前） | 10:53（八刀后） | **12:30（两处 O(N·M) 都收）** |
  |---|---|---|---|
  | 7997 模型阶段 | 2,287 s ≈ 38 min | 628.7 s ≈ 10.5 min | **159.7 s ≈ 2.7 min** |
  | 7997 任务总耗时 | — | 1,185 s ≈ 19.8 min | **571.4 s ≈ 9.5 min** |
  | 　CATA 依赖 | — | 323.7 s，2,469 次，387,132 行 | **35.8 s，280 次，25,602 行** |
  | 　模型行落库 | — | 174.7 s，单笔 400 → 1,280 ms | **72.3 s，单笔 313 → 383 ms（不再随表长）** |
  | 　几何生成本体 | — | 130.3 s | 51.6 s |
  | `surreal.exe` CPU | ≈2,600 s | 1,010 s | **512 s** |
  | `watermark_probe` / `spatial_fragments` | 237 / 237 次 | 0 / 0 | 0 / 0 |
  | 交付单元 | — | — | 6,769 / 6,769 成功，失败 0；三水位 `in_sync` |

  **S8 那条「7997 模型阶段 ≤ 5 min」达成（2.7 min）。** 落库单笔耗时前后十批 313 → 383 ms，
  O(N²) 那条增长曲线没了（原先 400 → 1,280 ms）。
- **13:37 第三轮冷启动复现**：模型阶段 **146.4 s ≈ 2.4 min**（CATA 34.2 s / 280 次、落库 63.8 s、
  几何 48.4 s），6,787 根 0 失败。CATA 那一格两轮分别是 35.8 s / 280 次与 34.2 s / 280 次——
  收敛值稳定，不是一次侥幸。
- **落库结果与未记忆的旧码逐行相等**（把 10:53 那个 rocksdb 另起一个 surreal 对拍）：15 张表计数
  全等，`pe.dependency`（15,557）、`room_result`（65,319）、`inst_relate` / `tubi_relate` 的 id 集合
  **两个方向的差都是 0**。写入次数从 387,132 降到 25,602，落地的行一行不差。10:53 / 12:30 / 13:37
  三轮两两相等。
- **引用完整性**：库里点名的 19,445 个 mesh **悬空 0**（磁盘 19,446 个，多的 1 个是内容寻址的孤儿）；
  房间边点名的 37,942 个构件 / 202 块面板在 `pe` 里**悬空 0**；每个库恰好一个活的 `result_version`，
  `room_result_part` 同版本。
- **拿 E3D 自己当真值对了一遍几何**（`scripts/e3d/ams7997_geom_crosscheck.mac`，只读 TTY 会话、
  无 SAVEWORK）：7997 里随机十件 BOX / CYLI，E3D `Q POS WRT WORLD` 与本地 `inst_relate.aabb_d`
  的中心逐件比——**10/10 类型一致、世界坐标差 ≤ 0.5 mm**（E3D TTY 只打到整毫米，残差就是取整）。
  其中两件在 W 向（负 X），符号也对上了，量的不只是数值大小还有坐标系口径。

**同时纠正一条上一轮写下的判断**：那时把模型阶段拆成「生成 454 s + 落库 174.7 s」，并推断 454 s 是
19,705 次 mesh 小文件写盘的系统调用风暴（`19,700 × 20 ms ≈ 394 s`）。**这条是错的，已实测证伪**：在同一
块盘上逐项复刻 `ensure_mesh_file` 的调用序列（exists → create_dir_all → 写临时 → 读回校验 → `sync_all`
→ hard_link → 删临时），单件 **1.63 ms**，19,705 件合计 **约 32 s**；其中 `sync_all` 只占 0.26 ms/件。
真正的拆分是：**CATA 依赖 323.7 s（51.5%）+ 落库 174.7 s（27.8%）+ 几何生成本体 130.3 s（20.7%）**，
三段相加正好 628.7 s。几何生成本体只有 130 s，「≤ 5 min」这条验收并非不可达。

### 模型落库的对账读收成一个任务一次（`GeometryBaseline`，ADR-071 D1）

`persist_roots_batched` 每调一次就整库扫一遍 e3d-model 几何身份。7997 实测：68 组累计读约 **180 万行**，
写入行数一路没变而单笔事务从 **400 ms 涨到 1,280 ms**——典型 O(N²)，读的是「这个库至今为止的全部几何
身份」，而写的人正是自己。

- 一个任务读**一次**基线，之后每组落完把自己写的那部分就地更新进快照（`advance_geometry_baseline`），
  快照始终等于「现在再读一次会得到的东西」，不是近似。作用域绑数据窗口；窗口不在就不缓存；
  任一组落库失败 → 整份作废（那一组可能已经提交了前几块，快照不再可信）。
- **钉**：同一窗里连落两批，吃内存快照与每批都重读一遍的**终态必须完全相同**（陈旧行漏删是数据错，不是慢）。
- 验证：全量 `cargo test --lib -- --skip damaged_` 绿；`rustfmt --check` 干净。
  live（12:30 那一轮，见上一条的对照表）：落库 174.7 s → **72.3 s**，单笔 400 → 1,280 ms 变成 313 → 383 ms，
  写入行数与事务笔数一格没变（54,441 行 / 180 笔），也就是说省掉的全是那条随表长大的读。

### 去账本：初始化发布 run / ACK / journal 整体退役，启动改为 D7 水化（ADR-071 S5）

S1 把 `init_async_publish` 缺省翻成 `false` 之后，这套机制在缺省配置下就已经是死代码：没有 run，
`blocks_dbnum` / `blocked_dbnums` / `preparing_dbnum` / `planned_roots` / `blocks_data_batch` /
`can_adopt_retained_initialization` / `initialization_source` 恒回「无」，`finish_ready_initializations` /
`kick` / `prepare_baseline` / `admit_baseline` 是空转，只剩 `recover_before_cleanup` 会在启动时替**有 run 清单
的库**水化模型投影。这一刀把它们连根拔掉，调用点按「无 run」那一支化简：

- **删文件**：`model_publish_queue/{runtime,journal,local_state,tests,generation_handoff_tests}.rs`（约 4,400 行）、
  `bin/model_publish_sql.rs`、`data_interface/baseline_root_parity_tests.rs`（它测的 `frozen_baseline_model_plan` 只服务 run）。
  `model_publish_queue/mod.rs` 只剩 `IncrementWindowArtifact` + `land_increment_window` + `digest` + 根发布信封
  （`FrozenRootEnvelope` / `PreparedRootPublication` / `RootIntent` / `FORMAT_VERSION` / `render_spatial_fragment`
  ——`model_db_adapter` 的 `frozen: Some(_)` 分支还引用着、生产已无人走到，随文件轮次那一刀一起删）。
- **「某库在飞」的唯一来源改成任务注册表**：`TaskRegistry::has_running_data_batch_for(dbnum)` /
  `running_data_batch_dbnums()`（新）。`model_read_route::route_for`（Memory ⇔ 任务在跑）、`/dbnums` 的
  `annotate_model_source`、`dbnum_state::resolve_dbnum_applied`（任务在跑时只认专用水位行，不拿半截基线的
  info max 当 applied）、播种 / 迁移 / 待办清理的 protected 名单、`model_update_pending` 的初始化排除，
  全部改吃它。`MemoryReason::InitializationPublishing` 字面值不变（plant-ui 已认），含义 = 「这个库正在被写」。
- **调用点化简**：`batch_worker` 删 `finish_ready_initializations`、ModelOnly 的 run 接管分支
  （`initialization_source` + `wait_for_task_root_acks`）、`refresh_candidate` 的 retained token、两处
  `blocks_dbnum` 守卫；`batch_scheduler` 出队谓词去 `blocks_data_batch`；`manual_update` 基线路只剩
  `wipe_dbnum_for_reinit` + `baseline_model_plan`（`frozen_baseline_model_plan` / `AIOS_INIT_PARALLEL_GENERATION`
  分支删）；`fast_delete::wipe_uncommitted_initialization_data` 删；`startup_reconcile` 去「保留 run 才接管」条件。
- **启动**：`recover_before_cleanup` → `hydrate_model_projections()`（`lib.rs`）：durable 形态下对每个
  `model_sesno > 0` 的 DESI 库 `restore_projection_on`，失败出声不拦启动（按需生成会自行重建）。这是 D7
  「库 = 序列化，启动反序列化」的第一版，S6 再并进统一的水化段。
- **对外面**：`/health` 去 `model_publish_queue` 一格与 `model_publish_queue.held` 阻断条件；
  `POST /api/v1/model-publish/jobs/{id}/replay`、`POST /api/v1/model-publish/resume` 端点与路由删。
  `DbOption` 的 `init_async_publish` / `model_publish_queue_dir` / `model_publish_memory_bytes` /
  `model_publish_disk_bytes` 与 `AIOS_INIT_ASYNC_PUBLISH` 删（无人再读）。
- **钉**：`batch_worker` 的收尾顺序钉改成 `the_task_tail_runs_publish_receipt_rooms_then_stamp_without_a_publish_run`
  （四个 run 名字一个不许回来）；`model_update_pending` 的启动核对钉改成
  `startup_reconcile_no_longer_consults_a_retained_publish_run`。
- **留给 S6 / 文件轮次那一刀**：一次性 `REMOVE TABLE IF EXISTS model_publish_run / model_publish_fragment /
  model_publish_ack`（表已无写者）；`model_db_adapter` 的信封分支（~800 行）与 `persist_root_for_file_round`。
- 验证：`cargo check --lib --tests --bins` 干净；全量 `cargo test --lib -- --skip damaged_` **1969 passed /
  0 failed / 136 ignored**（较上一刀的 2023 少的是随 run 删掉的测试）；rustfmt 改动文件干净。**未验证**：live。
  被删文件的副本在 `%TEMP%\gen-model-lane-retired-20260910\`。

### live 复跑（S8 那条验收）：7997 模型阶段 38 min → 10.3 min，两条 O(N) 读**归零**

新二进制冷启动重跑 `ams-init-1112-7997`（新 run 目录 `output/ams-init-1112-7997/20260910-104959`，
独立端口 18322 / surreal 18309，空 rocksdb），与今早 07:23 那轮逐项对照：

| 项 | 旧（07:23，S1 之前） | 新（10:53，八刀之后） |
|---|---|---|
| 7997 模型阶段墙钟 | 2,287 s ≈ **38 min** | 619.7 s ≈ **10.3 min** |
| `[model-query] query=watermark_probe` | **237 次**，合计 1,309 s | **0 次**（全程 29 次，全在启动对账，每次 3–4 ms） |
| `spatial_fragments=` | **237 次**，合计 875 s | **0 次** |
| 7997 数据+模型总计 | — | 1,185 s ≈ 19.8 min |
| `surreal.exe` CPU | 1.03 核跑满 42 min ≈ 2,600 s | 1,010 s（≈0.7 核 × 24 min） |
| 三水位终态 | model 追平、room 未盖 | 1112 与 7997 都 `applied = model = room`，`in_sync` |

- 计划预期的「≤ 5 min」**没达到**：剩下的 10.3 min 是真活——6,769 个根的几何生成 + 68 批
  `persist_roots_batched`（每批 100 根、1.7–8.4 s）。收口循环那 2,184 s 的空转确实一秒不剩了，
  但生成本身没变快，也不该由这条线变快。
- **`room_build.rebuild_required=True` 与 `spatial_tree=ready_empty` 两个老问题照旧在**，与今早那轮
  一模一样——不是本轮改动带来的，但它们让 monitor 的 DONE 判据永远不成立（今早那轮就是这么
  空转到 150 min 超时的）。下一步要单独查。

### 修：收据搬到水位行之后漏了作废镜像那一句（live 抓到）

`model_completion::commit_on` 现在写的是 `dbnum_watermark`，却没跟着作废水位镜像那一格。
后果在 live 里当场现形：1112 的房间那一段读到 `applied_sesno=0`（库里是 725），被自己的前置条件
挡下，`房间水位保留待恢复`。启动恢复随后补上了，终态没错，但那一轮白算一次房间。

补上 `DbnumState::invalidate_mirrored_on(db, dbnum)`——**任何写 `dbnum_watermark` 的人都欠这一句**，
这是 d-227 镜像的成文契约，收据搬家时漏了。

### 去账本：非空间副作用补偿队列（SYST 派生 / ref_rev 补偿 / cata_ref_rev）连 drain / 死信 / 复活 / 清扫整体退役（ADR-071 D9，S4 后半第一刀）

用户指派「side_effect_pending（空间树 reconcile / ref_rev 补偿 / SYST 派生）改随任务收尾就地做，删表」。核下来
**非空间那三档根本没有活可就地做**：SYST 派生的执行体 `team_data::sync_team_data` 自 2026-09-07 起只打一行声就返回
（TEAM_DATA 镜像随 feature = "sql" 退役），补偿队列里排的是空转作业；`ref_rev` 补偿早已没有生产入队点（`enqueue_ref_rev`
零调用），如今 `ref_rev` 维护失败直接让数据窗口的同步落库失败、整窗重放，没有可补偿的东西；`cata_ref_rev` 09-07 已退役。
于是这一刀是**纯删除**：

- **`side_effect_pending.rs`**（2,749 → 2,028 行）：删 `enqueue_from_incr` / `enqueue_syst` / `enqueue_ref_rev` /
  `upsert_pending` / `mark_failed` / `mark_abandoned` / `drain` / `revive_dead_letters` / `sweep_done` / `has_dead_work` /
  `side_effect_status` / `side_effect_error_status` / `parse_ref_rev_payload` / `MAX_ATTEMPTS`；`SideEffectKind` 只剩
  `SpatialReconcile`，`render_mark_done` 随之只剩空间那一笔；8 条非空间测试随删。模块头改口：只剩「文件轮次」那一行与
  它的发布清单，随 S4 后半 / S5 并进 `task_effects`。
- **`batch_worker.rs`**：三处 `SideEffectCompensator::drain` 删（模型恢复前置、批次后、空闲轮）；SYST 批次落库后只剩
  作废范围缓存 + `SCOPE_DIRTY`；空闲轮的 `sweep_done` / `has_dead_work`（曾以死信扣模型门）删。钉
  `a_syst_batch_books_its_derived_sync_…` 翻成 `a_syst_batch_invalidates_the_scope_cache_without_booking_a_derived_sync`
  （不得再有 `enqueue_syst(` / `SideEffectCompensator::drain(`）。
- **`/health`** 去 `side_effect_pending` 一格；**`POST /api/v1/update/side-effects/retry`** 端点与路由删。
  钉 `health_exposes_side_effect_pending_dead_letters` 翻成 `health_no_longer_exposes_the_retired_side_effect_queue`。
  `web/ops.html` 读这一格是 `|| {}` 兜底、不会炸，但「复活死信」按钮现在打到 404——随 S7 的 ops.html 卡片清理一起删。
- **没动**（这一刀之外、仍在 `side_effect_pending.rs` 里）：空间 `spatial_reconcile_{dbnum}_{sesno}` 行、
  `published_file_input` manifest、`CompletedFileRound` / `promote_file_round` / `mark_file_rooms_released` /
  `has_collecting_file_rounds` 等文件轮次机制——它们与 `finish_file_room_round`、`aabb_tree` 启动短路、`room_resident`
  基线交织，正由「去账本第一～四刀」那条线往 `task_effects` / `task_registry` 搬（第一刀条目末段列的就是这张清单）。
- 验证：`cargo check --lib --tests --bins` 干净；全量 `cargo test --lib -- --skip damaged_` **2023 passed /
  0 failed / 137 ignored**（30.2 s）；rustfmt 改动文件干净。**未验证**：live 复跑。

### 去账本：数据窗口的本地 journal / backup lane 整条拆掉，结构行回 dbnum 任务同步落库（ADR-071 D1 / D11，S4 前半）

用户 10:0x 原话「不需要账本……我们都是按照一个 dbnum task 处理，初始化也好，增量更新也好。不需要以前的方式了，
帮我清理干净……不要把架构弄的复杂了」。此前工作树里的数据窗口已经是 (a) 的 lane 形态（d-183 / d-279，共识
d-285 / d-288 已作废）：`apply_one` 一行结构行不写，把 pe / ATT_ / CATA 依赖 / `ref_rev` / 随窗语句装进工件落本地
journal，`accept` → 尾事务 → `complete`，再由 `start_increment_backup_lane` 起的 sink 异步落库；启动还要对账
Prepared 窗口、刷新数据库绑定。ADR-071 D10 那句「数据窗口的写法本来就是同步的」对这棵树不成立——这一刀把它变成立。

- **`increment_pipeline::apply_one`**：窗口产物 `IncrementWindowArtifact::from_window` 仍在内存里装（事实不是 SQL），
  随即 **同步** `land_increment_window(&SUL_DB, T, &artifact)`（计入 `timings.persist`），失败即返回——水位不动、
  attempt 记录保留，整窗重放收敛；然后才 `reopen_verified` → `finalize_attempt`（尾事务，随窗语句形参仍为空：已随
  产物落了）。`LocalInput` / `increment_local_runtime` / `observe_database_applied` / `store_backup_bundle` /
  `accept` / `complete` 全部删除；回执 `warnings` 接落库回执的 UDA 缺键 / 墓碑退化 / CATA 缺失。
- **`lib.rs run_cli`**：删本地 runtime 打开、`reconcile_prepared_data_windows`、`record_startup_reconciliation`、
  `bind_database`、`start_increment_backup_lane` 整段；`/health` 两处 `local_runtime` 块删除；`DbOption` 的
  `local_runtime_root` 与 `options::local_runtime_limits` 删除。
- **`model_publish_queue/`**：删 `local_runtime.rs`、`local_backup.rs`、`local_state_tests.rs`、`reconcile_tests.rs`，
  `mod.rs` 去掉 lane 的四个 static、`increment_local_runtime*` / `local_runtime_health` / `PreparedWindowReconciliation` /
  `reconcile_prepared_data_windows` / `start_increment_backup_lane`；`structured_data_sink.rs` 只剩 `LandingReport` +
  `land_increment_window`（同步落库入口，`SurrealStructuredDataSink` 与 `LocalBackupSink` impl 删除；随窗语句批的
  错误改为 `take_errors` 全收，THROW 原话不再被 `.check()` 吞掉）；`increment_window_artifact.rs` 去掉
  `into_bundle` / `from_bundle`（不再落盘）。`fast_model/local_initialization.rs`、`bin/local_init_producer.rs` 删除。
  **留到 S5 / S7 一并删**：`journal.rs` 里的 `LocalWindow` / `LocalCompletion` 事件与 `local_state.rs`（journal 还
  引用着，发布 run 退役时整目录一起走）。
- **钉**：`apply_one_has_no_staging_fork` 改钉同步序列化顺序（闭包 → 失效 → ref_rev 渲染 → 随窗渲染 → 产物 →
  落库 → 复核 → finalize）；新钉 `the_data_window_lands_its_structured_rows_synchronously_before_the_tail`
  （落库口只有一处、在尾事务之前、lane 六个名字一个不许回来、老逐段直写口也不许回来）；
  `cache_invalidation_runs_before_the_landing_and_again_after_the_rows_land`；lib.rs 新钉
  `startup_neither_opens_a_local_journal_nor_starts_a_backup_lane`；sink 测试改为
  `landing_the_window_product_equals_the_segment_by_segment_direct_write`（产物落库 == 逐段直写，逐表）、
  `landing_the_same_window_twice_converges`（不动）、`a_failing_statement_errors_out_and_a_replay_converges`
  （坏语句 → Err 带 THROW 原话 → 重放与一次成功逐表相等）。`a_dbnum_unknown_to_the_binding_…` /
  `the_window_is_prepared_before_the_tail_…` / lane 三条 completion 测试随 lane 删。
- **顺手核出一条 `mem://` 怪癖**（写进测试注释）：一张表若**首次隐式定义发生在被回滚的事务里**，之后对它的写会
  静默落空；生产里随窗语句的目标表都在启动 DEFINE 过，风险低，但新表别指望隐式定义。
- 验证：`cargo check --lib --tests --bins` 干净；全量 `cargo test --lib -- --skip damaged_` **2031 passed /
  0 failed / 137 ignored**（28.7 s；较 S2-a 后的 2060 少的是随 lane 删掉的测试）。**未验证**：live 复跑。
- 被删文件的副本留在 `%TEMP%\gen-model-lane-retired-20260910\`（仅本机，几小时后可清）。

### 去账本第四刀：完成收据从账本那一行搬到水位行

收据原先寄居在「文件轮次」那一行（`incr_side_effect_pending:spatial_reconcile_{dbnum}_{sesno}`
的 `model_completion` 字段）——账本时代它要跟着那一轮的发布状态走，所以住在那儿。它描述的
本来就是「这个库追平到哪一版」，账本退役之后自然的家是 `dbnum_watermark:{dbnum}`。

- **`model_completion::round_id()` → `receipt_row(dbnum)`**：收据 + 新字段
  `model_completion_sesno`（这张收据是给哪一版开的）都写在水位行上，读者拿它与 `model_sesno`
  对齐，对不上就当没有。
- **退了一道闸**：原先第一道是「文件轮次那一行还是 `done` 且 `commit_token` 没换」
  （`rejection::FILE_ROUND_CHANGED`）。没有那一行可问了，同一件事由任务自己担保——走到这里
  它刚把这一轮的空间发布完，令牌就是它手里那一个（`complete_model_task_on` 已断言收据的
  `source_token` 与之相等）。**其余七道库侧证据闸一道没少**（数据水位、四种根证据、模型待办为空）。
- **同理退掉的交叉校验**：读者原先要比「收据的 `source_token` == 那一轮的 `commit_token`」。
  收据当时寄居在别人的行上，得问两边同不同意；现在它住在任务自己盖章的那一行，
  没有第二方可以不同意。探针（`read_model_watermark_probe` 第 10 句）与
  `dbnum_consistency` 的普查查询都改读水位行。
- 钉：收据表驱动用例从 13 格减到 10 格（去掉的三格钉的正是那道退役的闸），
  新增断言「收据要说清自己是给哪一版开的」。
- 验证：`cargo test --lib`（排除下面说的那个模块）**2028 passed / 0 failed**。

### 去账本第三刀：空间发布改由任务直发，账本不再是集合的来源

`publish_model_file_round` 末尾那句 `reconcile_spatial_pending(mgr)` 换成
`publish_task_spatial_effects(dbnum, end_sesno, db_type, commit_token)`：这一轮的空间增删
直接从 `task_effects` 那本内存账拿，不再「写进行里 → `SELECT` 回来 → 应用 → 销账」绕一圈。

- **`PublishedRoundIdentity`（新）**：一轮发布的身份只有四样（dbnum / db_type / end_sesno /
  commit_token）。候选清单与快照 manifest 原先从 `&[PendingJob]` 算——也就是从账本那几行算；
  抽出这个结构之后，**任务内存也能造出同一份候选**，而启动那套「快照头 vs 库侧清单」的对拍
  逻辑一行没动（这是用户选的 (a) 路）。`From<&PendingJob>` 让账本那条路原样可用。
- **`publish_one_spatial_round`（新）**：拓扑候选 → 应用增删 → 发布快照，**账本重放与任务直发
  共用同一段身体**。两条路走同一段，是为了账本拆掉时不会有一条路悄悄少做一步；
  「拓扑先于快照」那条纪律的钉子也随之移到它身上。
- **任务直发顺手销账**：发布成功后 `mark_done` + `promote_state_after_replay`，出队前那道闸
  （`reconcile_spatial_pending`，扫全局）就不必再重放一遍。别的库遗留的 pending 行仍由那道闸兜。
- **内存那本取不到时回落到库里那一行**：内存跨不过重启。过渡期两本账内容相同，行拆掉之后
  回落也一起没——届时兜住崩溃窗口 ②/④ 的是「收尾那一笔没盖章 = 这一窗没发生过，整窗重跑」。
- 钉：`batch_worker` 的发布顺序钉改看 `publish_task_spatial_effects(`；`side_effect_pending`
  的发布纪律钉改钉在共用执行体上（`persist_aabb_tree_if_dirty_locked_for_publication(rounds)`）。

### 去账本第二刀（半刀）：`e3d.room.lookup` 的「在飞」改问任务注册表

`task_registry::has_running_data_batch()`（新）替掉 `query_service` 里那句
`SideEffectCompensator::has_collecting_file_rounds()`。后者问的是「文件轮次那一行还开着吗」，
那一行随账本退役；同一件事注册表里就有答案，还省一次库读。

- **只看 `Running`**：`Queued` 还没开始动这个库的数据，算进「在飞」会让读路在一整条积压队列的
  存续期间都说「更新中」。
- **崩溃留下的未收敛意图不归这一问管**：它由同一个函数上面那句 `ensure_spatial_ready()` 拦住
  ——空间树是 `ReplayRequired` 时，`e3d.room.lookup` 根本走不到这一行。
- **`aabb_tree` 那两处 `has_collecting_file_rounds` 没动**：启动短路判据与 `file_round_private`
  裁决要和「行不再产生」同一刀换，单独换会让启动在还有 `collecting` 行时判错。
- 钉：`only_a_running_data_batch_counts_as_a_publication_in_flight`（排队 / 在跑 / 终态三态各答什么）。
- 验证：全量 **2064 passed / 0 failed / 138 ignored**（26.6 s）。

### 去账本第一刀：副作用集合搬进内存（`data_interface/task_effects.rs`，ADR-071 D1）

用户 2026-09-10 拍板「**不需要账本**，我们都是按照一个 dbnum task 处理，初始化也好，增量更新也好」。
所谓账本就是 `incr_side_effect_pending:spatial_reconcile_{dbnum}_{sesno}` 那一行（「文件轮次」）：
生成阶段把「空间要刷新的 / 要删的 refno」「房间要清的边」写进去，同一个任务收尾时再读回来用。
写进去又读回来这一趟是异步时代的遗产——那会儿模型行、空间重放、房间收尾分属三条路，得有一本
**持久的**待办把它们串起来；ADR-071 之后三段都在同一个任务里同步跑完，串它们的应该是内存。

- **`task_effects`（新）**：键是（dbnum, 这一轮的目标 sesno），与库里那一行同一把钥匙。
  集合代数与 `render_file_spatial_reconcile_upsert` 逐条对齐——一个 refno 从 refresh 挪到 remove
  （或反过来）时另一边同时摘掉，否则同一根会既被刷新又被删除。`open` 时丢掉这个 dbnum 的**全部**
  历史键：上一轮半途没收尾的残留漏进新一轮，比少记一条危险得多。
- **喂养点与库那一行一一对应**：`model_db_adapter` 两处（合块落库 / 逐根落库）、`aabb_refresh` 一处、
  `batch_worker` 的开轮处。同一处调用同时喂两边，过渡期两本账内容相同。
- **房间那一段改吃内存那本**：`finish_file_room_round` 的清边名单来自 `task_effects::snapshot`，
  库里那一行只作**崩溃回落**（进程中途重启内存那本没了，名单还在库里）。S4 把那一行拆掉之后
  回落也没了——届时语义是「收尾没盖章 = 这一窗没发生过，整窗重跑」。
- **钉**：4 条新用例——没开轮是 `None`、开了轮但空是 `Some(空)`（两者要分得开，前者才回落）、
  改主意的 refno 从另一边摘掉且清边名单只增不减、开新一轮丢掉同库旧账但不碰别的库。
- 验证：全量 `cargo test --lib -- --skip damaged_` **2063 passed / 0 failed / 138 ignored**（27.8 s）。
- **下一刀（S4，还没做）**：账本真正拆掉要等空间重放进任务内存。今天 `publish_model_file_round`
  末尾那句 `reconcile_spatial_pending(mgr)` 已经**在任务里同步跑**了——它只是绕了库一圈：
  把集合写进行里、再 `SELECT` 出来、`apply_deferred_spatial_mutations`、然后销账。改成直接吃
  `task_effects` 就少这一圈。真正要换的是**崩溃语义**：今天靠那一行 `pending` 跨重启重放
  （崩溃窗口 ②/④），拆掉之后靠「收尾那一笔没盖章 → 整窗重跑」。这一换必须和
  `has_collecting_file_rounds`（`e3d.room.lookup` 的 `updating` 判据、启动 `file_round_private`）
  改吃 `task_registry` 一起做，否则读路会永远说「更新中」。

### S3（前半）+ S2-b（后半）：房间排到盖章之前，任务收尾一笔盖三水位（ADR-065 G2 (i)，修 d-73）

顺序翻过来了。`batch_worker` 的 Task 末尾从「文件轮次发布 → **模型盖章** → 房间」变成
「文件轮次发布 → 完成收据 → **房间** → 一笔收尾盖三水位」——d-73 指出的「模型水位先于房间前移」
就此消失：房间没成，模型那一枚照盖（房间失败不擦掉已经成的模型），房间那一枚不盖，启动恢复再补。

翻不动的原因原本是三道闸都要求**模型水位已经盖在库里**，本轮把它们换成了任务手里的事实：

| 闸 | 原来 | 现在 |
|---|---|---|
| `calculate_and_commit_memory_rooms` 前置 | `state.model_sesno == model_sesno && applied >= model_sesno` | 只剩 `state.applied_sesno >= model_sesno` |
| `PreparedRoomResult::require_on` CAS | `$watermark.model_sesno = $input.model_sesno AND …` | `$watermark.applied_sesno >= $input.model_sesno` |
| `PreparedRoomResult::commit_on` CAS | 同上 + 房间单调 + 义务相等 | 去掉模型那一条，另两条留着 |

**挡住陈旧房间结果的仍然是义务 CAS**（`room_required_input = $input`）：另一个任务一旦
`require_on` 换了义务，旧结果就提交不了——这条由
`boundary_change_during_calculation_preserves_new_obligation_and_rejects_stale_commit` 钉着，
本轮另加一条反向钉「模型水位自己动了**不再**挡房间提交」，免得哪天被顺手加回去。

- **`PreparedRoomResult::commit_on` 只写边与结果版本**：`room_sesno` / `room_sesno_time` /
  `room_settled_at` 三格挪进 `task_end`。生产路径上 `room_sesno` 现在**只有 `task_end` 一个写者**。
- **两个调用方各自盖**：`batch_worker` 的 Task 末尾在房间之后一笔盖（模型 + 房间 + `state_revision`）；
  `room_task::recover_missing_room_watermarks`（启动恢复）自己盖，且**只在确实落后时**盖——
  否则每次启动都会给每个已完成的库白涨一次 `state_revision`。
- **`complete_model_task_on` 退成纯收据写者**：不再盖水位，`target_time` 参数一并去掉（时刻跟着
  收尾那一笔走）。它那四道整库证据闸留在收据里，位置也没变——房间算之前就该确认「整库追平到 T」。
- **钉**：`batch_worker` 的阶段顺序钉改成 `publish < 收据 < 房间 < 收尾 < finish_ready`；
  `room_task` 源码钉「计算段不得再拿库里那枚模型水位当前置条件」；`room_completion` 三条测试按
  新的两段式改（`commit_on` 之后房间水位**不动**，收尾那一笔盖了才到 12）。
- 验证：`cargo check --lib --tests` 干净；全量 `cargo test --lib -- --skip damaged_`
  **2059 passed / 0 failed / 138 ignored**（27.7 s）。
- **未动**（S3 剩下的一半 + S4）：`finish_file_room_round` 仍认文件轮次
  （`file_round` / `room_released` / `retire_file_room_pending` / `mark_file_rooms_released` /
  `clear_deferred_room_edges(published.room_clear_refnos)`），`publish_model_file_round` 仍挂着延迟
  模型动作、`stage_published_room_input`、房间候选拓扑与 `reconcile_spatial_pending`。清边名单改由
  任务内存合成、`missing_panels` 整库拒算改成留痕 + 排除集（ADR-067 D6），都还没做。
  **房间失败挡住牵涉根凭证前移**（G2 (i) 的后半）也没做：今天仍是「模型整枚照盖」，
  按根剔除要等根级凭证前移的集合就位。

### S2-b（前半）：任务收尾接上 `task_end`，`read_model_watermark_probe` 退出热路径（ADR-071 D3）

收尾那一问从库挪回了任务自己手里。`complete_model_task_on` 原先开头就跑一次
`read_model_watermark_probe`——10 条语句、两趟 `gen_root` 全库扫，7997 实测 avg 5.5 s——问的三件事
任务其实都知道：「还有根在赶吗」是本任务单元清单的成败、「凭证下限够不够到 T」是本任务刚把这一版
每一根都落了库（S1）、「这库是不是空模型」是本任务规划出来的根数。

- **`ModelTaskCompletion` 多两格**：`model: TaskModelOutcome`（本任务这一版的成败与放弃根数）与
  `roots: usize`（落库的根数，`0` = 权威空清单，走 `receipt.empty` 那条）。`batch_worker` 从
  `result.units` 现算，不查库；走到那一步 `round_complete` 已经担保每个交付单元都是 `Generated`。
- **`model_completion::commit_on` 只写收据，不再盖水位**：原先它把
  `render_model_watermark_advance` 嵌在同一笔事务里。水位统一归 `task_end::commit_task_end_on`。
  收据那四道**库侧证据闸**一道没退——闸问的是「整库都追平到 `sesno` 了吗」（含别的轮次留下的
  未安定根），任务自己的单元清单答不了这一问，它只知道本任务那些根。闸是 `LIMIT 1` 的存在性判定，
  一趟；探针那两趟是全量取回。
- **收据落完立刻接收尾那一笔**：`task_end::commit_task_end_on(model: Some(..), rooms: None,
  info: None)`。房间水位仍由 `PreparedRoomResult::commit_on` 盖、Ref0 统计仍由 `dbnum_stats` 的逐笔
  观测块维护——这两半随 S3 并进同一笔（见下「未动」）。收据刚证过 `applied_sesno >= T`，紧接着
  收尾又被数据水位闸挡下的话，只可能是这个库有第二个写者：**报错**，不是一句 `AheadOfData`。
- **钉**：`model_update_pending` 两条新源码钉（收尾段内不得再含 `read_model_watermark_probe(`；
  空清单判据必须是 `completion.roots == 0`，不得再读 `probe.has_roots` / `probe.settled_floor`），
  `model_completion` 那条钉翻面（收据落地但**水位不动**，改由收尾那一笔盖后才到 12）。
- 验证：`cargo check --lib --tests` 干净；全量 `cargo test --lib -- --skip damaged_`
  **2053 passed / 0 failed / 138 ignored**（31.3 s）。
- **未动**：`batch_worker` 末尾的顺序还是「文件轮次发布 → 模型盖章 → 房间」，**没翻成** ADR-065 G2 (i)
  要求的「模型行 → 房间 → 收尾一笔」。翻不动的原因是硬的：房间那一段
  （`calculate_and_commit_memory_rooms` 的 `state.model_sesno == model_sesno` 前置、
  `PreparedRoomResult::require_on` / `commit_on` 的两道 CAS）**要求模型水位已经盖在库里**才肯算，
  所以水位挪到房间之后就得同时改房间的三道闸——那是 S3 本体。`publish_model_file_round` 同理不能删：
  它里面还挂着延迟模型动作、`stage_published_room_input`、房间候选拓扑与 `reconcile_spatial_pending`
  （分属 S3 / S4）。`read_model_watermark_probe` 本体保留，剩下的调用者是启动对账（D6）、
  `advance_model_watermark_on` 与空闲循环的 `republish_settled_file_rounds`——它们问的都是整库口径，
  任务内存答不了，随 S5 / S6 一起收。

### S2-a：任务收尾的一笔控制面事务落地（`data_interface/task_end.rs`，ADR-071 D3/D6）

计划 §2 S2 的第一半：**渲染器 + 落库器 + 钉**，还没接进 `batch_worker`（接线要连着 S3 的
`finish_task_rooms` 一起换，见下）。它要替掉的三处写入各有各的代价——`complete_model_task_on`
背后那次 `read_model_watermark_probe` 是 10 条语句两趟 `gen_root` 全库扫（7997 实测 avg 5.5 s）、
`PreparedRoomResult::commit_on` 把房间提交与水位耦在一笔 CAS 里（房间先于模型盖章，d-73）、
`dbnum_stats` 的逐笔 ±1 观测块每笔元素写都带 `$before_` / `$after_` 读回。

- **`task_end::render_task_end_tail(&TaskEnd) -> String`（新）**：一笔 `BEGIN/COMMIT`，先 Ref0
  统计块、后水位那一句。统计块 = 本库现有行先 `count = 0`（历史 Ref0 留行不留数，供清理发现），
  再按快照逐个**绝对**覆盖（`count` 来自任务手里的目标索引，不再逐笔加减；`sesno` / `max_ref1`
  仍 `math::max` 单调；Ref0 归属冲突照旧 `THROW`——三个项目的 sys 库都是 8191，这道闸不能省）。
  水位那一句留着 ADR-060 的三条纪律：单调 `math::max`、时刻子句排在赋值之前、
  `WHERE (applied_sesno ?: 0) >= T` 是数据水位的第二道闸（M2）；`UPDATE` 不是 `UPSERT`。
- **谁盖章谁不盖**：`model_sesno` 只在 `model.all_roots_ok` 时推，`room_sesno` 只在 `rooms.ok` 时推
  （ADR-065 G2 (i)：房间失败挡住牵涉根的凭证前移，但模型失败不挡房间已成的那一半）。
  `model_dead_roots` / `model_roots_observed_at` 是现场观测，成败都记。一枚水位都不盖时整句退成
  `state_revision + 1`，**不带**数据水位闸。
- **`commit_task_end_on`**：落库 + 作废该 dbnum 的水位镜像格（d-227）与一致性普查缓存，回执说清
  「盖了哪枚水位 / 写了几个 Ref0 / 是不是被数据水位挡下」。回滚的事务会给出错语句**之前**的每一条
  都报一句笼统的「事务失败」，所以按 `check_publication_response` 的老办法把带序号的错误全留下——
  只看第一条就永远看不到 `THROW` 的原话。
- **口径**：`TaskEnd.info` 是**目标索引的全量快照**（`None` = 本任务没有权威快照，一行都不碰
  info 表；`Some(vec![])` = 该库在 `T` 上没有活元素，全部清零）。统计块排在水位闸之前是有意的：
  统计描述的是**已经落库的数据**，水位闸管的是模型/房间承诺，两者各管各的。
- **钉**：10 条新用例——6 条渲染钉（一笔事务 / 两枚水位与闸 / 时刻先于赋值 / 模型失败只留观测 /
  空尾只推 revision / 收尾不得回查 `gen_root` 等四个形状 / 文件名与时刻的引号逃不出字面量），
  4 条 `mem://` 真跑（三样事实一次落地且历史 Ref0 清零留行 / 数据水位挡住两枚水位但不挡统计 /
  模型失败房间照推 / 别库的 Ref0 让整笔回滚且一格没改）。
- 验证：`cargo check --lib --tests` 干净；全量 `cargo test --lib -- --skip damaged_`
  **2051 passed / 0 failed / 138 ignored**（24.6 s，S1 之后是 2041，新增 10 条即本条）。
- **未接线**（S2-b + S3 一起做）：`batch_worker` Task 末尾仍走 `publish_model_file_round` →
  `complete_model_task_on` → `finish_file_room_round` → `finish_ready_initializations` 那一串；
  `read_model_watermark_probe` / `ModelWatermarkProbe` / `advance_model_watermark_on` 还都在。
  新顺序（模型行 → 房间 → 收尾尾事务 → 镜像 → 回执）要等 `room_task::finish_task_rooms` 就位，
  否则房间那一段还认文件轮次。Ref0 快照的**生产者**（从任务手里的 `DbSet@T` 按 Ref0 分组数活元素）
  也还没写，`info` 目前只有测试在喂。

### S1：模型行由 dbnum 任务跨根合块**同步**落库，初始化不再开异步发布 run（ADR-071）

ADR-071（已接受）计划 §2 的第一步。今早 `ams-init-1112-7997` 那 40 分钟里 95% 花在「每 16 根一次的收口循环」
拖着的两条 O(N) 读上（`watermark_probe` 237 次 1,309 s、`spatial_fragments` 237 次 875 s），它们长在 pending +
异步发布这套模式上；S1 把模型行的写者从队列消费者换回任务自己——生成完就地写，不排队、不等 ACK、不落中间 SQL 文件（D11）。

- **`model_db_adapter::persist_roots_batched`（新）**：一批根的记录**跨根合块**，按 `tx_limits::data_tx_limits()`
  的字节上限装满切块（超限劈半、管身槽位跨块延续），一块一笔事务；语句形状仍是 ADR-060 的「按记录 id 先删后插 +
  每表一条 `INSERT IGNORE`」。对账读从每根一问收成整库一次（`existing_geometry_of_dbnums_on`，各根按祖先链在客户端
  分摊）；嵌套根共有的记录一组内只渲一次；`gen_root` 凭证行末块盖章，之前的块由 `render_publication_fence_batch`
  标出中间态。一组 ≤ `BATCHED_PERSIST_ROOTS_PER_GROUP`（512）根，**组是赔付边界**：组内任一块失败，该组根全算落库
  失败（回执没盖、栅栏留着，窗口规划器眼里仍未发布，下次任务重生成），其余组照常。没有 per-root CAS / ACK——同一
  dbnum 只有本任务一个写者。
- **`model_refresh::generate_roots_report`**：`wait_for_task_root_acks` / `wait_for_dbnum` /
  `render_file_model_publish_root` / `initialization_source` / 逐根 `persist_root` 循环全部去掉，整批根只走一次
  `persist_roots_batched`；`memory_only` 库照旧只写内存（R2 / ADR-064 D7），读透形态照旧「先写透 `pe` 再发布模型行」，
  CATA 依赖块仍只 warning 不拦发布。落不进去的根按组进回执 `failures`。新增 `[model-persist]` 一行日志
  （roots / groups / rows / removed / receipts / commits / failed_roots / elapsed_ms）。
- **`init_async_publish` 缺省翻成 `false`**（`options.rs`）：新 run 不再开异步发布，显式 `AIOS_INIT_ASYNC_PUBLISH=1`
  只为回放旧 journal 留门，S5 拆掉 run 时连门一起删。
- **钉**：`model_db_adapter` 四条新 `mem://` 用例——跨根合块且每根都盖章、一次对账读就删净陈旧行、按字节劈块时管身
  槽位跨块延续且栅栏末块才抬、拒收别库的根；`model_refresh` 源码钉「不得再含 ACK / 逐根发布那五个形状，整批只落一次」。
- 验证：`cargo check --lib --tests` 干净；全量 `cargo test --lib -- --skip damaged_` **2041 passed / 0 failed /
  138 ignored**（26.6 s）。**未验证**：7997 live 复跑没做——S8 那条验收（模型阶段 ≤ 5 min、`serve.stdout.log` 零
  `watermark_probe` / `spatial_fragments`、surreal 不再一核满跑）要等下一次 `ams-init-1112-7997`。
- 未动（S4 / S5 再收）：`persist_root` / `persist_root_in_chunks` / `persist_root_for_file_round` 与 `PERSIST_CHUNK`
  仍在，服务尚未退役的文件轮次路径；`batch_worker` 的初始化接管分支与 `blocks_dbnum` 判据、`model_update_pending` /
  `model_publish_queue` 本体、根级 CAS 的库侧半边也都留到那两步。

### 已有数据库的兼容初始化：模型 / 房间水位缺省按数据水位追平（ADR-060 D10）

用户拍板「模型水位 + 房间水位默认按数据水位 保持一致」，起因是核对「gen-model 兼容一个已有数据的数据库的
初始化，主要是水位的初始化」：数据那一枚早有 ADR-001 的 pe 兼容播种（`ensure_increment_state_storage`，按
`max(pe.sesno)` 填 `applied_sesno`、`queue_control:watermark_seed` 标记、完整性比对不齐不播）；模型那一枚却
**没有任何播种路径**——历史库（`pe` + 旧管线 `inst_relate` / `tubi_relate` 投影、无 `gen_root`、`model_sesno` 从未
写过）在 D6 对账里只得到 `AwaitingTaskCompletion`，随后被启动核对排成从 0 起的 `ModelOnly` 恢复 Task；
**兼容一个已有模型数据的库 = 全库模型重建**，且 `complete_model_task_on` 对「无根却有投影」的库永远盖不了章。

- **播种**（`dbnum_state.rs` 新增 `seed_legacy_model_watermarks_on` / `DbnumState::seed_legacy_model_watermarks` /
  `legacy_model_watermark_seed_lines`）：候选 = `applied_sesno > 0 ∧ model_sesno = NONE`（`NONE` 与显式 `0` 分开，
  `0` 是 D7 / D8 / 本条写下的判定，不改写）；初始化在飞的库跳过。有旧管线投影（`projection_presence_expr`，
  与 D6 探针同一段谓词）→ **接管**：`model_sesno = model_sesno ?? applied`、`room_sesno = room_sesno ?? applied`，
  只填空、不贴 `*_sesno_time`（D1 时刻纪律）、`state_revision` 自增、逐格作废水位镜像；数据在模型无 → 显式记
  `model_sesno = 0`（语义不变，只为退出候选集——投影探针大库上不便宜），房间不动。两格各一行启动日志带样本，
  安静的一次不出声。
- **启动序列**（`lib.rs run_cli`）：播种排在 `init_inst_relate_indices` / `backfill_inst_relate_anc` **之后**
  （探针的 `WHERE in = $parent.id` 没 `idx_inst_relate_in` 时是逐 pe 行全表扫）、`reconcile_model_coverage_at_startup`
  **之前**（否则那一轮给这些库排从 0 起的恢复 Task，接管等于没做）；失败只提示不上抛，下次启动幂等重来。
- **启动核对**（`model_update_pending.rs`）：`ModelWatermarkProbe::legacy_adopted`（无根 ∧ 有属性 ∧ 有投影 ∧
  `model_sesno > 0` ∧ `model_sesno >= applied_sesno`）命中即跳过恢复 Task，逐库 `reason=legacy_adopted` 出声、
  汇总行加「兼容接管 N 库」。探针 `supported()` 对该形状本就回 `applied`，所以接管的库在 D6 对账里是 Aligned。
  数据一旦前移就不再是这一格，照常开窗。
- **已知边界（有意留下，写进 ADR-060 D10）**：接管的库在新管线写下第一行 `gen_root` 之前没有严格完成凭据；
  这期间一窗零 regen 的 Task 收尾仍按「无根须无投影」判 Partial（水位已由热路径盖到 T、房间那一窗不跑）——
  是 `CompletionReceipt.empty` 只有一种形态的问题，本条不改，要收另起一条给回执加「兼容接管」形态。
- 文档：ADR-060 状态行 + D10 追记；CONTEXT.md 新词条「兼容接管 (Legacy Adoption)」。
- 验证：全量 `cargo test --lib -- --skip damaged_` **2028 passed / 0 failed / 138 ignored**（24.1 s）。新钉 7 条：
  `mem://` 六库一表（dbnum 直写投影接管 / 只有 `in` 的历史投影接管 / 数据在模型无记 0 且房间不动 / 显式 0 不动 /
  `applied = 0` 不是候选 / protected 跳过、解除后再接管、同名单重跑幂等），接管语句只填空不贴时刻、`SET` 顺序
  （`room_settled_at` 判断排在 `room_sesno` 赋值前），日志两格分开且安静不出声；探针 `legacy_adopted` 四条件缺一
  不可；播种后的历史库对账 `Aligned{26}` 且 `legacy_adopted()`；启动核对源码钉「接管判定 < 取文件最新 < 开窗」
  且出声进汇总；`run_cli` 源码钉「索引 → 回填 → 播种 → 启动模型核对」且失败不上抛。既有 `legacy_inst_projection_
  waits_for_a_completion_task` / `legacy_tubi_projection_…` 照旧绿——对账本身仍不上调，上调只由播种与 Task 做。
  **未做实机 live**（需要一台带旧管线投影的历史库）：接管后首启应见「兼容接管 N 库」且 `/dbnums` 该行 `in_sync`。

## 2026-09-09

### 自带持久库档 `spawned-rocksdb`：「创建数据库实例」是一档配置（0.1.21，ADR-070）

计划 `docs/plans/2026-09-09-spawned-rocksdb-store-and-create-instance.md`（一轮 6 问全按推荐拍板），
承接向导计划 Q5 / ADR-069 后果节留给 v2 的「一键用包内 surreal 起持久库」。需求原话：数据库要能
创建实例——指定 ip（默认本机）、端口、项目名、代号等。答案是**一档配置而不是一次性动作**：
一次性动作起的实例没有属主（机器重启后不在，external 连不上又回到手动）；档位形态重启即自愈。

- **新档位**：`store_mode = "spawned-rocksdb"`——服务每次启动用包内 surreal（fork 2.1.x，版本闸
  不放宽）在 `v_ip:v_port`（向导默认 127.0.0.1:8010）拉起一台 **rocksdb 后端**的库，数据落
  `workspace\.surreal\<project_code>`（代号空/"0" 回落项目名再回落 ams，目录名滤路径不安全字符；
  **换代号 = 新目录新库**，旧目录留盘人工清），退出收尸只杀进程、**数据活得过重启**。署名/心跳/
  四格裁决/孤儿回收/AttachOnly 与 spawned-mem 逐字同一套——唯一真改动是把写死的数据源位置参数
  `"memory"` 参数化成 `SpawnDatastore`（memory / `rocksdb:<目录>`）。档位派生一张表跟上：
  `is_durable()` 真、data_face 派生 ingest、实例锁照拿、`/health.medium="spawned-rocksdb"`（as_str
  直通，`handlers.rs` 零改动）、`PersistMedium` 归 RocksDb；`in_memory_db=false` 与本档不再误报冲突。
- **向导第②步改三档**：外接（连通+版本硬闸）/ **新建本机持久库（新部署推荐，带端口预检与
  0.0.0.0 暴露警告——凭据默认 root/root，先改 `v_user`/`v_password` 再暴露）**/ 试用内存库；
  数据目录只读展示并随项目代号联动；自带两档地址留空由前端自动补默认（8010/8009）。顺带把
  radio 单选整块包进 `<label>`（0.1.20 走查发现 13px 圆圈难点中）。
- **判据连锁**（修订 ADR-069 D3 一句）：设置模式判据从「external 且地址空」扩成「**需要端点的
  档位**（除 embedded-mem）且地址空」——spawn 两档空地址是必坏形状（bind 端口 0 / `ws://:0`），
  手改坏形状从「启动死在绑定错误」变成「进设置模式」；写口合并后校验同律扩档（422、盘上不动、
  错误带各自默认端口）。出厂模板不改（仍 external + 空地址；「出厂直接改本档」单独拍）。
- **脚本**：冒烟门第三腿 C（本机配置副本改 `spawned-rocksdb`@18010、整份包 workspace 打底，考
  正常模式 medium/durable + `.surreal` 目录真落盘，绝不碰真 8010）；包内 `Start-Surreal.ps1` 修
  「出厂 `v_port="0"` 哨兵被当端口」的坑（读到 0 回落 8010）并改口「向导第三档可替代本脚本」。
- 文档：ADR-070 + CONTEXT 词条「自带持久库」（并同步「设置模式/配置向导/冒烟门」三词条）+
  `docs/release-notes/0.1.21.md`。
- 验证：全量 `cargo test --lib` **1973 passed / 0 failed / 137 ignored**（28.4 s，新钉：档位真值表、
  rocksdb 位置参数形态、目录命名回落、设置模式判据扩档真值表、写口拦截扩档）；`rustfmt --check`
  五文件干净、`git ls-files --eol` 无行尾漂移。**spawned-rocksdb 实机 E2E**（release exe + 出厂模板
  @19099 + 真项目）：设置模式 → 空地址 422 拦 → PUT 15 键 → 重启交接进正常模式（medium=
  spawned-rocksdb / durable=true / `.surreal\1516` 落盘）→ **强杀后端** → 孤儿 surreal 留 18011 →
  再起服务按署名收编（「属主已经不在…杀掉 pid=22428」）且**同一数据目录接上**（创建时刻未变）→
  complete 删哨兵，全链 16 s，跑完 19099/18011 归零、临时目录已清。0.1.21-full 出包**三腿冒烟
  全过**（A 设置模式 / B 本机全启动 / C 持久库落盘），`ams-local-0.1.21-full.zip` 394.7 MB +
  sha256 + release.json（含 rocks_leg）。首跑 C 腿曾暴露临时工作区缺 `resource\surreal\*.surql`
  导致 define functions 失败——已改为与 B 腿同底（整份包 workspace 拷底再换配置）后通过。

### 首跑配置向导 `/setup.html` + 设置模式启动臂 + 出厂模板包形态（0.1.20）

计划 `docs/plans/2026-09-09-setup-wizard-web-frontend.md`（两轮 16 问全按推荐拍板 + 两条补拍：
出厂 external 空地址、默认监听全 0），口径 ADR-069（修订 ADR-068 的 D3 包内配置与 D6 冒烟门），
CONTEXT.md 收「配置向导 / 设置模式 / 首跑哨兵 / 引导子集」四词条并改写「冒烟门」词条。
解决的事：-full 包解压到新机器，配置是打包机快照，服务死在任何网页能送出去之前——现在解压
首跑自动进浏览器向导，一步步配好再初始化。

- **设置模式最小启动臂**（`lib.rs`）：配置无效（`project_path` 缺/不存在，或 `external` 且
  `v_ip` 空 / `v_port=0` 哨兵）→ 不连库、不扫描、不起 watcher/队列，只绑 HTTP 供向导 +
  `/health`（`mode:"setup"`）。**库地址填了但连不上不算**——照旧正常启动失败退出，不盖真故障。
  首跑哨兵 `.first-run-pending` 只管 `/` 302 到向导、**不影响启动臂**（向导 ④→⑤ 之间要重启一次，
  重启后必须正常启动才能初始化——两判据拆开的原因，ADR-069 D3）。
- **setup 面**（新文件 `web_service/setup.rs`，8 端点 `/api/v1/setup/*`）：state / config 读写 /
  三探测（项目目录形态、SurrealDB 连通 + 2.1.x 版本硬闸、端口预检）/ restart / complete。
  全部按 **TCP 对端**回环限定（出厂监听全 0，监听地址判会拒掉所有人；不认代理头）。写盘走
  `toml_edit` 外科手术：注释与未知键原样保留、整数键不漂移、先落 `.bak`、空补丁不碰盘；
  合并后校验拦「spawned-mem + 空地址」（落盘必坏：下次启动把内存库绑到端口 0）——前端试用档
  对空地址自动补 `127.0.0.1:8009`。
- **向导页** `web/setup.html`（exe 内嵌 + 磁盘副本优先 + `x-setup-panel-source`，同 ops.html
  模式）：五步（数据源探测勾选 → 存储两档带测试 → 端口预检 → 初始化两档单选「只导数据/一步到位」
  写回 `gen_model`/`gen_mesh` + 保存并重启 → 初始化薄壳）；⑤ 全复用现口
  `/dbnums` + `/update/preview` + `/update/execute`，勾选执行、水位轮询到全同步；403 整页引导
  「在服务器本机用 127.0.0.1 打开」。
- **重启交接**（唯一高危面）：restart 口 202 → 300ms 后 spawn 同 exe/参数/环境的子进程（带
  `AIOS_RESTART_HANDOFF=1`）→ 父进程退出交出端口与单实例锁；子进程对锁与端口在 ≤10s 窗口内
  重试（`open_lock_within_handoff_window` / `bind_with_handoff`），窗口只在环境变量在场时存在。
  拉起失败父进程继续跑并出声，不静默。
- **包形态**（`New-LocalRelease.ps1` + 新模板 `scripts/templates/DbOption.factory.toml`）：包内
  `workspace\DbOption.toml` 换成中性出厂模板（`project_path=""`、external + 空地址哨兵、
  `0.0.0.0:9099`、gen 全关）+ 落首跑哨兵；`web\setup.html` 磁盘副本进包；README 加「首跑：
  配置向导」一节（含不用向导的手改路径）。冒烟门改**两腿**：A 腿出厂配置起包内 exe 考
  `mode:"setup"` + 向导页标记，B 腿本机真配置全启动（首页判定放宽为 plant-ui 或向导页二择一）；
  `-StrictSmoke` 任一腿失败即不出包。
- 验证：全量 `cargo test --lib` **1970 passed / 0 failed / 137 ignored**（28.1 s，含 13 条
  setup 钉：判据真值表、回环闸真值表、不读请求头源码钉、写盘保注释/保未知键/整数不漂移、
  先备份后写、spawned-mem 空地址拦截、路由接线钉、内嵌标记钉、探测形态钉、哨兵往返幂等、
  交接窗口要环境变量）；`rustfmt --edition 2024 --check` 对 `setup.rs` 干净。**重启交接实机
  E2E**（release exe + 出厂模板改 19099 + 包内三缓存目录）：设置模式 health/内嵌页/302 →
  项目探测（9 子目录识出 7 个 E3D 形态）→ 端口预检 self → PUT 15 键（`.bak` 落盘、模板注释
  保留）→ 空地址试用档被 422 拦、盘上不动 → restart 后旧进程退出、新进程接手端口与锁
  （pid 64956 → 46092）→ 正常模式 `/` 仍 302（哨兵在）→ complete 删哨兵、`/` 恢复 200，
  全链 10.5 s；跑完 19099/18009 归零、临时目录已清。0.1.20-full 出包与两腿冒烟见
  `releases/ams-local-0.1.20-full.release.json`。

### 补齐两篇悬空 evidence（collar 拷回 + coincidence-tolerances 重建取证）+ Snout/Pyramid/CircTorus facet 逐面复核

承上条 facet 复核线，把 `sweep_mesh.rs` / `libgm_discretise.rs` / `manifold_csg.rs` / ISSUE-022 引用却
在本工作树缺失的两篇 evidence 补齐，并把 CircTorus 管向相位、Snout、Pyramid 也逐面对过——全程只读取证，无生产代码改动：

- **`2026-08-24-ida-gm-collar-ruled-solid.md`**：从 `gm-verify-tmp` 工作树拷回 gen-model 主树
  （与另外几棵树逐行同内容，仅 EOL 差；取 CRLF 变体，14093B）。`sweep_mesh.rs:485` /
  `Ruled` 臂注释的引用即刻解析。
- **`2026-08-25-ida-libgm-coincidence-tolerances.md`**：此前**从未在任何工作树落盘**（纯悬空），
  按 ISSUE-022 + changelog 已记的 08-25 发现**重建 + 逐条在活库 IDB 重新取证**：Core3D
  `adp_gm_mk_body`（`sub_104DA260`）建体前连设 `gm_SetResolutionTolerance(0.051)` /
  `gm_SetDefaultNormalisationTolerance(0.051)` / `gm_SetDefaultTangentTolerance(0.0087266)` /
  `gm_SetDefaultFacetTolerance(0.5)`——最后一个就是本仓 `FACET_TOL_MM`，前面 restol/normtol=0.051
  就是 `RES_TOL_MM`（三个自由函数 0x1003CAC0/0x1003C8A0/0x1003C580 各写 `restol_/normtol_/arctol_`，
  都带 `>0` 门）。**负体机制本轮逐帧复跑，更正了 ISSUE-022 的描述**：`addStandAloneNegative`
  (Core3D 0x10726620) → `gm_CreateCombination(3)` 建 `GM_SolidCombination`（不是 AggregateCombination）
  → 自有 `GM_SolidCombination::calcFacets`(0x100A00D0) → `gm_combine`(`sub_1009BD60`, `GM_SetOp.cxx`)
  **真三维 facet 布尔**：交线 `GM_IntCurve` + `buildFacets` + `normalisePostSetOp(GM_User::normtol_)`
  + squeeze；共面重合由 **`normtol_`(0.051)** 治理，**不是** `restol_`、**也不是** `obscureFaces`
  面内二维相减（obscureFaces 0x10068710 的调用者只有 `GM_Facets::obscure`/`selfHide`，是隐藏面/出图；
  `D2_PolySet::booleanPrecise` 是二维截面布尔——ISSUE-022 把这两者误挂成负体减法）。数值 0.051 不变，
  ISSUE-022/`manifold_csg` 注释里「面内二维相减/restol」的机制描述**已同步更正**为「三维 set-op 的
  normtol 共面重合」（用户 2026-09-09 拍板；数值 0.051 不动、`RES_TOL_MM` 常量名与 `NEGATIVE_INFLATE`
  行为不变，只改注释）：改了 `issues/ISSUE-022-*.md`、`manifold_csg.rs`、`libgm_discretise.rs` 的
  `RES_TOL_MM` 注释；ADR-044 经查不含该机制措辞、无需改。rustfmt 干净、`fast_model::libgm_discretise`
  + `fast_model::manifold_csg` 48 passed（含源码扫描钉与负体回归钉）。顺带坐实 `normtol(double)` 成员 setter
  零调用（2 条 xref 均导出表 RVA 数据、非 call），运行期真值 0.051 而非注释旧写的 1e-6。
  08-25 evidence 文件头标注 2026-09-09 重建。
- **facet 复核补三节**（写进 `2026-09-09-ida-dish-rtorus-collar-facet-recheck.md`）：§3b CircTorus
  管向 `k·2π/n` 全圆格（常量 0x100D0420=2π）+ 环向沿扫角均分（0x100D0398=π/180）、接缝重复末点是
  ADR-049 有意表示；§3c Snout 段数 `circle(max(rBtm,rTop))` + 偏移上下各摊一半（T050 那件事的 libgm
  坐实）；§3d Pyramid 与 Snout 同构的偏移各摊一半、退化成线/点、无段数无相位。地址对照表补
  Snout / Pyramid 两行。
- 验证：本轮纯只读（ida-bridge SQL/汇编/IDAPython 读常量 + 目录统计 + 文件拷贝/换行规范化），
  未改生产代码，未跑构建/测试；两篇 evidence 现均在 `gen-model/docs/evidence/`，引用不再悬空。

### ADR-044「份数量出来再定」欠项关闭（实测 48,590 份 / 636.8 MB）+ 碟与矩形环面 facet 分布 libgm 复核通过

09-09 上午差异盘点（fable-5-26 交接线）剩余两项本日收口，均为取证 / 文档，无生产代码改动：

- **网格份数与磁盘占用（ADR-044 后果第 1 条）**：`assets/meshes` 离线实测 48,590 份
  `.mesh`、636.8 MB（内容寻址含历史代数，为上界；与 0.1.19 发布轮剔除的 636 MB web 侧
  网格冗余互证）。字节大头是近唯一的挤出 / 布尔网格；段数身份的单位网格增量上限 =
  段数档位数（圆柱 / 球每类 ≤249 档，环面 / 碟为小枚举组合），对该库份数 <1%、字节是
  噪声。判定**无需存储侧缓解**，ADR-044 补「2026-09-09 回填」一节关闭欠项；per-type
  物化档位盘点留作下次活栈顺手一条 SurrealQL，不立专项。
- **高级原语 facet 分布复核（盘点第 2 项）**：用真实 libgm.dll（idalib-72996）反编译 +
  关键处下到汇编，坐实 `GM_SDish` / `GM_EDish` / `GM_RectTorus` /
  `GM_Collar::form{Base,Top}Facet` 与本仓逐面一致——碟母线两段均分（hub θ/n、knuckle
  (π/2−θ)/n）、SDish 经向 θ/meridional 均分、RectTorus 截面居中 ±h/2 且环向沿扫角均分、
  EDish `isSpherical` = `|a−h| ≤ 1e-6`、2(hub+knuckle)>1000 时各自 4n>1000 夹 250；源码
  注释引用的全部地址与真实符号一一对上，无需改码。取证
  `docs/evidence/2026-09-09-ida-dish-rtorus-collar-facet-recheck.md`（顺带记账：
  `sweep_mesh.rs:485` 与 aios-core `libgm_discretise.rs:55` 引用的两篇 08-24/25 evidence
  在本工作树悬空，只在旧工作树里有——未处置）。
- 验证：本线两轮均纯只读（ida-bridge SQL / 汇编 + 目录统计），未改生产代码，未跑构建 /
  测试；e3d-model `flatten_ring` 负 FRAD 补丁经查已由后续会话落地（见下方「负 FRAD 收尾」
  条），未重做。

### 初始化准入门的嗓音收口：重扫汇总一句 + /health 点名最近一轮拦下的库（含方案与词条并轨）

方案 `docs/plans/2026-09-09-new-dbnum-manual-init-default-plan.md`（Plannotator 审阅通过）对下面
「新库冷初始化收口为开关」那条的收尾：门与开关本体见那条；本条补可见性与文档，并把两会话同日
各记的同义词条并成一条。

- **重扫收尾汇总**：`discover_batch` 的三种结局拆成 `DiscoveryOutcome::{Batch, UpToDate,
  AwaitingManualInitialization}`——「拦下等手动初始化」不再与「水位已覆盖」折叠在同一个 `Ok(None)`
  里；被拦的库进桶，重扫收尾汇总一句「N 个库从未初始化，本轮自动发现不入队…」（对齐 out_of_scope /
  watch_excluded / debug_excluded 各说各话，不逐库刷屏——原先门内那句逐库日志随之删除）。
- **/health 点名**：两个分支新增 `awaiting_manual_initialization`（最近一轮重扫快照：整面覆写**含空轮
  清空**——手动初始化完成后下一轮自然熄掉；share-remount 子集补扫只并入，它没看其余目录、没资格清
  别人）。逐库真值仍以 `/dbnums.awaiting_manual_initialization` 为准（按水位现算，不依赖重扫跑过）。
- **词条并轨**：CONTEXT.md 两条同义词条（「初始化准入门」/「新库自动初始化开关」）并成
  「初始化准入门 (Initialization Admission Gate)」一条，后者进 _Avoid_；下面那条的词汇表行已同步改口。
- 钉：`the_awaiting_manual_init_snapshot_replaces_on_full_sweeps_and_merges_on_subsets`（快照整面/子集
  两种语义）、`the_sweep_gives_the_initialization_admission_gate_its_own_voice`（桶 → 快照 → 汇总 →
  「声明了却没文件」顺序、快照更新不许挂在非空门底下——空轮覆写正是清 /health 那一格的一步）、
  `health_names_the_dbnums_awaiting_manual_initialization`（两分支同一快照源，不许各自手搓）。
- 验证：全量 `cargo test --lib -- --skip damaged_` 首跑 **1955 passed / 3 failed / 134 ignored**（26.8 s）
  ——3 个失败均为启动房间段源码钉（`the_startup_path_asks_the_gate_before_rebuilding_rooms`、
  `startup_reconciles_models_before_waiting_for_data_and_keeps_rooms_last`、
  `the_degraded_data_wait_skips_model_wait_and_rooms_and_is_named_on_the_banner`）：`lib.rs` 与 HEAD
  逐字相同、钉的标记（`skip_startup_room_build().await` 等）在 HEAD 启动段里已不存在，属房间流在途
  重排的既有欠账、与本条无关。处置（用户 2026-09-09 拍板）：三钉挂 `#[ignore]`（理由指回 T4 工单
  T4.2 与 mem-107，随反向重钉一并按新启动形状重钉或删除，别让 ignore 长住），T4 工单 T4.2 加
  「顺带欠账」一行；复跑全量 **1955 passed / 0 failed / 137 ignored**（26.5 s）。定向 5 条全绿；
  `rustfmt --edition 2024 --check` 对本轮改动行干净（两文件其余漂移属别的会话在途区，未动）。

### 本地发布一键脚本 `New-LocalRelease.ps1` + `ams-local-0.1.19-full` 一体化发布包

把 09-09 上午手工拼的 0.1.19 发布包沉淀成脚本化流程（9 项裁决全按推荐拍板：包内已验证 exe、全资源档、一体化布局、独立 CHANGELOG、`-full` 命名、本机 PowerShell 全链路、拒覆盖 + changelog 缺版只警告、冲烟门不静默放行）：

- **脚本**：`scripts/New-LocalRelease.ps1` 一键全链路——构建/复用 → 组包 → 组 CHANGELOG（gen-model 提炼段 + plant-ui 用户口径段）→ 冲烟门 → zip + sha256 + release.json 清单。开关 `-SkipBuild`/`-SkipSmoke`/`-Force`/`-SmokeStoreMode`/`-StrictSmoke`/`-CompressionLevel`。版本纪律：版本号以两仓 Cargo.toml 为准，`releases` 已有同名 zip 拒绝执行（`-Force` 才覆盖）；changelog 找不到当版条目只警告不中止。冲烟门：`spawned-mem` 自带内存库把包内服务拉起，`/health` 200 + 首页含 `plant_ui_app.js` 才放行压 zip；端口被占或起不来 → 明确报错并在产物标「未冲烟」，不静默放行。
- **产物**：`releases/ams-local-0.1.19-full.zip`（394.2 MB，62381 条目）——包根 `aios-database.exe`（逐字节 = 已验证 0.1.19，非 `D:\Rust\target` 里未验证的 15:40 构建）+ `web\` + `workspace\`（`assets`/`config`/`resource`/`search-index`/accel 快照/`DbOption.toml`/`bin\surreal.exe` + `Start-Surreal.ps1`，与 `test-worklspace\bin` 同构）；`web/assets/meshes` 清零（剔 636 MB 冗余，后端从 `workspace\assets` 供 mesh）；`Start-AMS.ps1` 默认 `-Workspace` 指向包内，解压即跑；旁挂 `.zip.sha256` 与 `.release.json`。
- **口径**（ADR-068）：资源进包、库数据与源模型不进包——已生成模型三维可离线看（文件 + assets + accel 快照），属性/搜索/补齐/更新要连回原 8010 库或先导数据；SurrealDB 必须本仓 fork 的 2.1.x（包内 `bin\surreal.exe`），不得用 PATH 上的 3.x。`CONTEXT.md` 收「本地发布包 / 依赖资源包 / 冲烟门」三词条；后端提炼版更新说明另存 `docs/release-notes/0.1.19.md` 供下版复用。
- 验证：冲烟实跑通过（`spawned-mem` 起包内服务，`/health` 200 + 首页含 `plant_ui_app.js`，不依赖外部 8010）；zip 62381 条目、`web/assets/meshes = 0`、`workspace/assets` 62288 文件、`config`/`resource`/`search-index` 齐；`aios-database.exe` 与 web 各文件 sha256 与源 `release.json` 逐一一致；跑完 9099/8010 均空、临时冲烟目录已清。（本条目补记：脚本与文档本体已随 bbf037112 / c5de1040b 入库，当时漏了 changelog 条目。）

### 新库冷初始化收口为开关：缺省只维护旧库，新 dbnum 等 plant-ui 手动初始化

增量数据面的自动发现（启动重扫 / watch 事件 / 周期对账 / scope-refresh / 共享盘重挂，全部经 `discover_batch`）对**从未初始化**的库（解析不出应用水位）不再自动铺首次导入，改由 `auto_initialize_new_dbnums` 门控（`DbOption.toml` **缺省 `false`**；环境变量 `AIOS_AUTO_INITIALIZE_NEW_DBNUMS` 双向可压，`serve --auto-initialize-new-dbnums` 单次打开）。缺省形态下新库由 plant-ui 走手动预览 / 执行初始化，服务端只维护已初始化库的增量。

- **准入门**：`automatic_discovery_admits(initialized, auto_initialize_new_dbnums, already_queued)`——已初始化库照旧维护（增量窗口、回退重建、幽灵水位都是旧库维护，不受门控）；已入队的行放行（接受了的手动初始化要活过 Meta → Design 清单刷新）；从未初始化的库缺省落「等待前端手动初始化」一声、不入队。门只在 `discover_batch` 一处，重建批次（Rollback → Reinitialize）那条绕行路不属于冷初始化。
- **手动路径不看开关**：plant-ui `/update/preview` + `/update/execute`（ADR-020 的 dbnum 子集照常可点名新库）与规格工具 `bin/initialize_ams_dbnums` 照常初始化；`/dbnums` 每行新增 `awaiting_manual_initialization`（未初始化 ∧ 未排除 ∧ 在项目里 ∧ 开关关着），`/health` 两个分支都露最终生效值。
- **live 钉改口**（开关落地时三条 live 用例还钉着旧缺省「全程不需要人工放行」）：`live_startup_sweep_baselines_a_never_parsed_db` 改两拍——第一拍缺省 false 重扫不入队，第二拍开关打开走全量基线；`live_startup_sweep_routes_a_new_db_to_baseline_beside_an_applied_one` 与 `live_scope_refresh_baselines_a_db_the_mdb_just_declared` 显式开着开关，各自只钉路由分家 / MDB 范围门。测试侧进程内覆盖 `AutoInitializeNewDbnumsOverride`（形状同 `RoomIncrementalOverride`：`set_var` 在 2024 edition 是数据竞争，CLI 那个 OnceLock 又翻不回来）。
- **词汇表**：CONTEXT.md 增「初始化准入门 (Initialization Admission Gate)」词条（两会话同日各记了一条同义词条，已并成这一条；「新库自动初始化开关」进 _Avoid_）——它门的是冷初始化准入，不是范围（MDB 声明 / 监听限定域）也不是消费（`startup_autorun` / 增量阶段控制）。
- 验证：全量 `cargo test --lib -- --skip damaged_` **1955 passed / 0 failed / 134 ignored**（25.9 s，rustfmt 后复跑同绿）；定向 `automatic_discovery_keeps_existing_databases_and_cold_databases_are_opt_in` + `new_dbnum_initialization_requires_explicit_opt_in` 2 passed；`rustfmt --edition 2024 --check` 对 `options.rs` / `increment_manager.rs` 全文件干净。**未验证**：三条 live 用例要 testbed（E3D 项目夹具 + Surreal），本轮到编译与断言改写为止。

### T4.1 房间收尾搬进 `fast_model::room_task`（S8 模块搬家，行为不变）

T4 第二片（开工单 `docs/plans/2026-09-09-t4-room-into-task-work-order.md` §T4.1）：在途会话已收笔移交后，把 Task 房间收尾从 `batch_worker.rs` 原样搬进新模块，为 T4.2（D1 顺序恢复）腾出落点。

- **搬家**：`finish_file_room_round` 整段（清边 → 启动屏障早退 → `room_incremental` 开关 → 定向消费 → 回执）逐字搬进 `src/fast_model/room_task.rs`，仅签名加 `pub(crate)`；顺序、回执、`println!` 日志一字未改。`batch_worker.rs` 两处调用点（`execute_frozen_batch` 主线、`publish_completed_file_round` 旧路径）改指 `crate::fast_model::room_task::finish_file_room_round`，函数体不再留在 batch_worker。
- **顺序钉随迁**：原 `completed_file_round_publishes_spatial_before_rooms` 是跨两函数的混合钉——`publish_model_file_round` 内部（候选落库 → 发布提升 → 空间收敛）留在 batch_worker；收尾内部（清边先于定向消费、清边先于开关早退，审核 F-E）随函数搬进 `room_task` 的新钉 `room_finalization_clears_before_draining`；两阶段先后仍由调用序钉 `an_initialization_publish_run_is_closed_by_the_owning_task`（publish < complete < rooms < finish）兜住。
- 验证：全量 `cargo test --lib -- --skip damaged_…` **1955 passed / 0 failed / 134 ignored**（27.2 s；新钉与三个受影响钉逐一 ok）；`rustfmt --edition 2024 --check` 对新文件 `room_task.rs` 干净，`batch_worker.rs` 本片改动行（两处调用点 + 钉拆分）无格式差（该文件其余漂移属别的会话在途区，未动）。
- 未动：D1 顺序恢复、房间失败扣根、mem:// 真测留 T4.2；消费就地重算 / `ensure_at` 拆分留 T4.3；旧形态退役留 T4.5。

### T4.0 GroupCache 跨代命中即提升（Q16）：邻库前进后没变的文件连磁盘工件都不读 + T4 开工单

T4（S8 房间内联）按开工单 `docs/plans/2026-09-09-t4-room-into-task-work-order.md` 开工，第一片是审核 W2 留给 T4 的内存侧另一半（ADR-067 实施约束 3，共识 d-89 预告）：

- **提升**：`room_topology.rs` 的进程内 GroupCache 代内条目带上本文件冻结令牌；`cached_groups` 代内未中时，从新到旧在别的代里找**同一份文件**（dbnum / sesno / 令牌逐字相同、层级同口径），命中就提进当前 composite 那一代再返回——任一邻库前进（composite 变、没变文件令牌不变）时，没变的文件此前是「内存 miss → 磁盘工件（W2 后才有）→ 重解析（ams7997 实测 6–7 s / 文件）」，现在内存直接命中且**命中跟着代走**，连续两次换代不丢。同号换文件（令牌不同）与层级不同一律不借，与 d-89 磁盘键同判。
- **写入收口**：remember 与提升共用唯一写入口 `insert_groups`，两代淘汰上限只守一次；结构钉 `the_group_cache_has_an_eviction_bound` 扩成「淘汰在唯一入口 + `remember_groups` / `cached_groups` 不得自带第二条写入路径」。两代语义（composite 为代键、只留两代）不动。
- **钉**：新增 `unchanged_files_are_promoted_across_generations_and_survive_two_swaps`（跨代命中、两次换代不丢、同号换文件不借、层级不借）；在进程级缓存上操练换代的两条测试加串行闸，互不挤代。
- 验证：定向 `room_topology::` **10 passed / 0 failed**；全量 `cargo test --lib -- --skip damaged_…` **1952 passed / 0 failed / 134 ignored**（27.8 s）；`rustfmt --edition 2024 --check` 在本轮改动行上干净（该文件其余未提交区域的格式漂移属别的会话，未动）。
- **开工单**：T4 全序列拆成 T4.0–T4.6（模块搬家 → D1 顺序恢复 + 反向重钉 + mem:// 真测（d-73 义务）→ 消费搬家（ensure_at 拆分 / ③' / 护栏随迁 / Q21 几何就地替换）→ 回执房间段定形（照 plant-ui 消费者输入）→ 旧形态退役 → live #2 验收门），每片全量收口；协调前提单列（`batch_worker.rs` 等在途未提交区域须先收笔或移交，S5a 建议排在 T4.5 前）。

### 负 FRAD 收尾：e3d-model `flatten_ring` 同款符号分流（`gen_ams`/RVM 对拍那条离散器）+ 两处上游夹签摘除

下一条把 gen-model 生产离散器补齐后，取证文档「顺带发现」记的另一条离散器欠项本轮落地——vendor/e3d-model `discretise.rs::flatten_ring` 此前 `p.frad.abs()` 把负 FRAD 当**凸**圆角（既不是旧 gen-model 的尖角、更不是 E3D 的凹弧），`gen_ams` / RVM 对拍走的正是它：

- **分流**：负支照 Core3D `sub_10621980`——退距恒 `|FRAD|`、弧以角点为心、半径 `|FRAD|`、取劣弧；Core3D 的两道门只加在负支（`fabs < 0.1` 不倒角、单位邻边叉积 `sin 2α ≤ 0.1` 不倒角），正支（含微小正 FRAD 小弧、近共线 1e-9 门、越界吸附 / 重接）一字不动——那些是 RVM 对拍钉住的行为（差异记在 `NEG_FRAD_EPS` / `NEG_CROSS_EPS` 注释）。负支越界只记账不裁不吸附（语料负 FRAD 全部远小于邻边，越界无实测样例）。
- **上游夹签摘除**：`catalogue.rs` 目录剖面 `PRAD.abs()`（全库 124 条环 PRAD 全 0，无观测差）与 `section.rs` SPVE `PRAD.max(0)`（截面 PRAD 与设计侧 FRAD 同义）——不摘的话负值在进 `flatten_ring` 之前就被翻正 / 夹零，分流永远走不到。
- **对拍**（六条新单测）：凹弧以角点为心、挖掉内接扇（面积逐位钉死）；正负同弦反向鼓（凸弧内点 < R、凹弧恒 = R）；微小负 FRAD 留尖角；近共线负 FRAD 留尖角（不然几乎拉直的角会被挖出近半圈大凹弧）；超尺寸负 FRAD 不裁只记账；真实 ZDJ 甲板板 `/DK_11000_TOS_P01` 外环（顶点 (402.105, 299.084) FRAD=−25，`out/neg-frad-verify/` 同源坐标，绕向不翻、挖量落在理论扇形的内接界内）。
- 验证：e3d-model `cargo test --lib discretise` **15 passed / 0 failed**（含新增 6 条）；全量 `cargo test --lib` **277 passed / 0 failed**；`rustfmt --edition 2024 --check` 对 `discretise.rs` / `catalogue.rs` 干净（`section.rs` 仅剩前一会话遗留未提交区块的格式差，本轮改动区无差）。取证文档补「flatten_ring 补修」一节。

### 负 FRAD 倒角走 `mthArcRadius`：以角点为心的凹圆角，补齐 `profile_spans` 的欠项

`libgm_discretise::profile_spans` 此前只处理正 FRAD（`mthArcFillet` 相切凸圆角），负 FRAD 注释「尚未移植，暂按尖角处理」。这轮结合 ida-bridge 反编译把它补齐：

- **取证**：Core3D `MDR_Point` 倒角方法（`sub_10621980`）按 FRAD 符号分流——开头 `if (fabs(radius) < 0.1) return 0` 正负同门槛；`FRAD > 0` 调 `mthArcFillet`，`FRAD ≤ 0` 把角点沿两单位边向量各退 `|FRAD|` 得两端点、`push 0`（劣弧）后调 `mthArcRadius`（libgeom 3.1 `0x10042ba0`，Core3D 从 `libgeom` 导入）。反编译 `mthArcRadius`：过两端点、半径 `R=|FRAD|` 的弧，圆心离弦中点 `sqrt(R²−(弦/2)²)`；两端点都距角点 `|FRAD|`，代入圆心恰落在**角点**。故负 FRAD = **以角点为心、半径 `|FRAD|` 的劣弧**（凹圆角），与正 FRAD 同弦反向鼓。取证 `docs/evidence/2026-09-09-ida-negative-frad-mtharcradius.md`（新建 libgeom IDB `D:\ida_scratch\plant3\libgeom.dll.i64`）。
- **落地**：`profile_spans` 按符号分流——退距正 `R·cot(φ/2)` / 负 `|FRAD|`，bulge 正 `-sign(φ)·tan((π−φ)/4)` / 负 `+sign(φ)·tan(φ/4)`（`φ`=两邻边夹角）。`|FRAD| < 0.1` 改判 `fabs`（正负同门槛；正 / 零 FRAD 路径逐字不变）。`R² ≥ (弦/2)²` 恒成立，负支不会因半径太小失败。
- **对拍**：新增四条单测，用本仓 `span_arc` 把展开的弧解回圆心 / 半径，钉死直角与 60° 两例圆心落角点、半径等于 `|FRAD|`、与正 FRAD 反向鼓、`|FRAD|<0.1` 不倒角。
- 验证：`cargo test --lib -- fast_model::libgm_discretise` **39 passed / 0 failed**（含新增 4 条）；下游 `sweep_mesh` / `manifold_tessellate` / `mesh_primitives` **101 passed / 0 failed**；`rustfmt --edition 2024 --check` 干净、无 lint。只改 `libgm_discretise.rs` 一个生产文件。

### S4b 刚体前移（ADR-066）：执行体幂等闸——transform 重放不再双重搬移（审核 P0 修复）

审核（计划 §9「审核 + P0 幂等闸」）发现 B6 执行体对投影记录无条件 `world' = Δ·world`：窗口重放（发布成功但回合未收尾就崩 / 水位未推进重跑）时投影已在 T，再乘一次 Δ 就是双重搬移，且不会有任何报错——regen 从文件@T 重算天然幂等，transform 不是。照 mem-42 两道闸修（会话 fable-5-11）：

- **基线随计划走**：`TransformUnit.source_start_sesno`（`serde(default)`）带上规划器算 `Δ` 的窗口基线 S；`UnitTask.transform` 改携整条
  `TransformUnit`（锚点 + S 不拆散）。旧计划线格式没有这格 → 默认 0 → 基线闸挡回 regen，保守正确。
- **闸（纯函数）**：`rigid_motion::republish_gate`——回执已在 T = 窗口重放，跳过按完成计（`RigidMotionReport.replayed`，投影一字未动，
  登记 / 收口照做、回执 `message` 注明）；回执 ≠ S = `Δ` 对不上（半截 / 旧计划 / 别的路径动过它），回退整根重算。执行体入参改
  `&[RigidMotionUnit]`，取旧回执后、动记录前过闸。
- 同轮按审核 P1 修订对拍门判法（计划 §5 门 3：`manifest_hash` 相等降为观察项、含旋转的 Baked 记录不判 `mesh_id`）；B7 夹具加第 (vii) 条
  幂等重放钉。
- 验证：targeted 54 passed / 0 failed；全量 `cargo test --lib -- --skip damaged_…` **1919 passed / 0 failed / 121 ignored**（24.9 s）；
  `rustfmt --edition 2024 --check` 五份改动文件干净。开关 `model_transform_cheap_path` 默认关，行为只在便宜路内变。

## 2026-09-08

### S4b 刚体前移（ADR-066）B4：窗口规划器接线——候选链 / `Δ` / 三条排除在文件侧一次算完，计划带上 `transform_units` 提示

计划 `docs/plans/2026-09-08-s4b-transform-republish-by-rigid-motion-plan.md` §4 **B4**（B1–B3 见 20:15 那条；落地形态与方案的四处差别在该计划 §9）。
规划器是唯一同时拿着两端 `DbSet` 与索引属主表的地方——判据与 `Δ` 全在这里（`spawn_blocking` 内）算完，执行体（B6）从此一个文件都不必读。

- **候选表**（`window_root_plan.rs`）：`WindowRootSources.rigid`——`collect` 末尾把账本逐 refno 去重折成 `WindowCandidate`：
  `placement_only`（B1 接通的那格）的候选走 `rigid_anchor`（`Δ = W_T·W_S⁻¹` 两端世界矩阵；三条排除沿 T 端**严格祖先**判：
  路由容器 / 点环容器 / `CWBRAN`，候选自身是桥架件也不刚），其余一律 `Other(账本明细)`；Deleted / Reparented / TypeChanged
  另走 base 端属主链（旧根只有它说得出）；读挂降 `Other` 并进 `notes`。属主链沿索引属主表上溯 ≤ 64 级、不含自身。
- **分桶**：`impact()` 抽出 `directly_touched`；`rigid_motion()` 算 `expanded = touched − direct`（C2 / C8 / P4 加进来的根一律
  不走便宜路，D1 规则 5）再调 `classify_transform_roots`（B3）；排在 `carry_pending_generation_on` 之后——带 pending 重生成的
  根 `desired` 已抬高、不在 `settled` 里，判据自己就拒它。安定判据 `root_is_settled` 与 `render_credential_advance` 的 SQL
  同口径（`settled_means_the_same_thing_wherever_it_is_judged` 扩到第三个用户）；`load_persisted_roots_with_settlement`
  只在开关开着时多查五列，关着走原 SELECT。
- **计划载体**：`transform_units_for` 把过判据的根折成 `ModelUpdatePlan.transform_units: Vec<TransformUnit>`（`serde(default)`，
  旧恢复记录反序列化为空；CATA 与老 PE 两个构造点填空）。**它是 `regen` 的子集加提示，不是第五个去向**：`work_items` / `units`
  一字不改（既有钉 `work_items_carry_only_regen_and_delete` 原样绿），pending drain 与 B6 之前的执行体看到的就是今天的计划——
  开关开着行为也不变，变的只有 warnings 里多一行账（transform / anchors / rejected 前 6 条 + notes；关着不出这一行）。
- 验证：`window_root_plan` + `rigid_motion` + 开关三处 47 passed / 0 failed；全量 `cargo test --lib -- --skip damaged_…`
  **1911 passed / 0 failed / 121 ignored**（27.2 s）；`rustfmt --edition 2024 --check` 五份相关文件干净。**生产行为一字未变**
  （开关 `model_transform_cheap_path` 默认关）。未起服务；规划器级端到端留给 B7 的 `mem://` 夹具。
  落地 20:34–22:39（会话 fable-5-1-64，差异 22:39 被接受）；接班会话 23:03 核对、跑全量并补本条记录。

### `occ_generate.rs` 改名 `mesh_generate.rs`，三个零引用结构体删除，四处指错文件的注释归位

ADR-030 / specs/009 把 OCC 三角化退役之后，这个文件里连一条 OCC 路径都不剩——`gen_inst_meshes` 开头写着
「形状生成只有 manifold 一台引擎，`occ` 已从本仓摘除」，源码钉 `gen_inst_meshes_has_no_occ_shape_fallback`
盯着不许它长回来。名字却还指着那台仓里已经没有的引擎。

- **改名**：`src/fast_model/occ_generate.rs` → `mesh_generate.rs`（`legacy_model` feature 后面那台旧生成器，
  ADR-030；`git mv`，97% 相似度）。跟着改的有 `mod.rs` 的模块声明、`gen_model.rs` 4 处 use、本文件 7 处
  `include_str!` 自指断言（文件名写错当场 panic，这一组绿了就是改名对上了的证据）、`concurrency.rs` 里
  `the_retired_future_wide_gate_gains_no_new_call_sites` 的文件名清单。
- **守卫脚本**：`check-production-no-legacy.ps1` 的禁词换成 `fast_model::mesh_generate`——**不能只删不补**：
  原有的 `fast_model::legacy` 分支盖得住 `legacy_*` 那类命名，盖不住 `mesh_generate`，少这一条，生产目录里
  再引旧生成器就没人拦。
- **死代码**：`OccGeoTransQuery`（全仓零引用，在用的是 `manifold_types.rs` 的 `ManiGeoTransQuery`）、`GeoParam`
  （在用的是同文件 `QueryGeoParam`）、`ParamNegInfo`（唯一用处就是 `OccGeoTransQuery.neg_ts`）三个结构体连同
  随之失效的 `Transform` import 一并删除。三个都不报 dead_code——`ParamNegInfo` 是 `pub`，逃过了 lint，
  只靠编译器扫不出来。
- **四处指错地方的注释**：`options.rs` 与 `spatial_state.rs`（两处）说的「`occ_generate` 直写分支 / 锁序源码钉」
  实际已随函数体搬到 `aabb_refresh.rs`，机械改名只会把错指针换个新名字接着骗人，改成指 `aabb_refresh`；
  `main.rs` 引的线程名早就从 `gensec-occ-regression` 改成了 `gensec-spro-regression`。
- **文档**：ADR / specs / plans / issues / 上下文 / teach 共 81 处引用同步改名（`docs/evidence/` 里那几处在
  工作区已是待删状态，没动）。
- 验证：`--features legacy_model` 下 `fast_model::mesh_generate` 10 passed / 1 ignored、`fast_model::concurrency`
  16 passed、`fast_model::aabb_` 33 passed / 3 ignored；默认 feature（不带 `legacy_model`）下并发闸清单测试
  passed；`check-production-no-legacy.ps1` → `PRODUCTION_LEGACY_SCAN_OK`。**未 live**：改名与删死代码不触及运行期行为。
- 有意没做：`main.rs` / `options.rs` / `pdms_inst.rs` 里那 5 行注释改动没进本笔 commit（`f4e124f1`）——它们纯注释，
  却埋在另一路会话正在改的同一批文件里，留在工作区等对方收工再提；HEAD 里因此还剩 5 行 `occ_generate` 注释。

### 目录前移的数据面那一半：目录版本核对排完根之后，已入库依赖行（`pe.dependency = true` 与其 `ref_rev`）也随目录前移（A3）

12:41 那份 external 增量分析的 A3，重构计划 §7.1 明写「本计划不接管、别丢」，用户 20:0x 拍板修。**问题**：H1 接的是
模型面——目录库前移 → `reconcile_catalogue_versions` 反查排 `RegenRoot`，几何会重算；但 `pe` 里按依赖入库的目录行
（ADR-064 D5，`dependency = true`）与它们的 `ref_rev` 只在某个 **DESI 批次的闭包碰到那个目录库**时才前移
（`persist_resolved_rows` → `refresh_moved_catalogues`，D7）。目录改了、设计没改：几何新了，属性面板 / `ref_rev` 看到的
目录属性还是旧版本，没有任何东西会去刷。

- **`cata_dependency::refresh_dependency_rows_of_moved_catalogues(project)`**：候选 = `cata_dependency_state` 里**全部**有状态行的
  目录库（不是模型面那份「过期」名单——依赖行落后与已发布根落后是两件事：一个根都没发布过的库也可能早有依赖行；
  `ensure` 重算根不碰 `pe`）；逐库调既有的 D7 `refresh_moved_catalogues`（文件没前移的一步不走；前移了就
  变动目录件 ∩ 已入库依赖行 → `Replace` 重渲、被删的落墓碑、推进 `cata_dependency_state.sesno`），**一库失败记进
  `errors`、不挡别的库**。回执 `CatalogueDependencyRefreshReport { candidates, refreshed[], errors[], elapsed_ms }`，
  安静的一次（没前移、没失败）`banner_line()` 回 `None`。
- **接线**：`reconcile_catalogue_versions` 拆成模型面 `reconcile_published_root_stamps()`（原正文，点名 → 收窗 → 反查排工）
  + 数据面这一步，**排完根之后**、仍在 `CATALOGUE_RECONCILE_SERIAL` 里；不带条件——模型面读不到落款 / 一个根都没发布
  也照做，两半各自对文件。三个入口（启动 / 摄入 sweep 看见目录前移或 300 s 一拍 / `/update/execute`）因此都带上了。
- **曝光**：`/health.catalogue_reconcile.catalogue_dependency_refresh`（运行期）与 `/health.initialization.startup.reconcile.
  catalogue_dependency_refresh`（启动）各一格；横幅在真前移了行或有失败时多一行（周期那一拍安静照旧不出声），逐库明细
  仍由 `refresh_moved_catalogues` 自己那句 `[cata_dependency] 目录库 … 前移 …` 打。
- **钉子**：`a_quiet_dependency_refresh_says_nothing_and_a_moved_one_reports_totals_and_failures`（账的三态）、
  `the_catalogue_reconcile_refreshes_dependency_rows_after_scheduling_roots_and_exposes_it`（源码钉：先模型面再数据面、
  不看过期名单不带条件、两个出口与两处横幅都带、候选来自 `cata_dependency_state`、逐库失败不挡）。
- 验证：`cargo check --lib --tests` 干净；`cargo test --lib -- cata_dependency:: …` 17 passed；全量
  `cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected` **1906 passed / 0 failed / 120 ignored**；
  `rustfmt --edition 2024 --check` 三份文件干净。**未 live**：要一台 external 实例 + 改一个目录件（与 H1 端到端同一条门，
  `Run-ExternalIncrementLive.ps1` 第五类变化）；`refresh_moved_catalogues` 本体是 D7 已在真库跑过的那份，本笔只换了它的
  触发源与名单。
- 有意没做：A4（`dbnum_consistency` 对目录落后加第四态 / 并排一格，口径拍板）；两次核对前后脚跑时数据面与某个正在跑的
  DESI 批次会对同一目录库各做一次 `Replace`——幂等（同内容、状态行 `math::max`），多算不漏算。

### S4b 刚体前移（ADR-066）B1–B3：e3d-model 账本的「只动摆放」索引接通、开关 `model_transform_cheap_path`、纯函数层 `rigid_motion.rs`、回执配方抽成一份

计划 `docs/plans/2026-09-08-s4b-transform-republish-by-rigid-motion-plan.md`（G5 已拍 (a)，Plannotator 过目 19:57）的前三步：
不撞另一会话文件的那一半先落。**生产行为一字未变**——开关默认关、规划器与执行体都还没接线（B4–B6）。

- **e3d-model `ledger.rs`**：`ChangeLedger.placement_only` 那个写好两个月、从没人写入也没访问器的字段接通——`record()` 对
  `ElementDiff::is_placement_only()` 的候选记一条，`pub fn placement_only()` 供 gen-model 窗口规划器读。它只说「谁只动了摆放」，
  不说「能不能刚体前移」（路由容器 / 点容器 / `CWBRAN` 三条排除在 T 端属主链上判，是消费者的事）。vendor 仓后续已成笔：
  B1 = `cbc47f2`；core3d 复核线其余 39 个在飞文件由边界 commit `f1697b6` 钉住（23:5x，用户过目后提交，
  `.gitignore` 加 `/.scratch`；`cargo test --lib` 270 passed），vendor 树自此 clean。
- **`options.rs`**：`model_transform_cheap_path`（`DbOption.toml`）/ `AIOS_MODEL_TRANSFORM_CHEAP_PATH` / `effective_*` 三件套，照元数据门
  的形状；**默认 false**（少算方向、尚无实证，ADR-066 D7）。
- **`rigid_motion.rs`（新）**：分桶判据 `classify_transform_roots`（D1 五条：退化 / 未安定 / 级联扩展进来的 / 归不了因 / 任一候选非刚 → regen；
  归因看两个方向——候选在根之下走候选的 T / S 端链，候选在根之上走根的 T 端链）；`nearest_anchor` 取**最内层**锚点（`Δ` 由两端世界矩阵算出，
  已含外层运动，草稿写「最外层」是错的，ADR-066 D3 同日修订）；`pure_translation` / `translate_bounds` / `bounds_of_points` /
  `republish_records`（盒：纯平移加 `t`，含旋转交给调用方注入的闭包读网格顶点——存下来的 `world_bounds` 是紧 AABB，盒过矩阵在旋转下一律偏松；
  每条记录 `source_sesno = T`，同根会话一致是 `persist_root_chunks_impl` 的断言）；线格式 `TransformAnchorWire`。
- **`e3d_model_service.rs`**：根回执的清单配方从 `project_geometry_delta` 抽成 `manifest_entry` / `manifest_hash`，重生成与刚体前移两条路
  共用一份（对拍门比的就是这个章）；源码钉 `regeneration_and_rigid_motion_share_one_manifest_recipe` 钉住 `project_geometry_delta` 必须调它、
  不许自己再拼 `Sha256::digest`。
- 验证：vendor `cargo test --lib ledger` 9 passed；`rigid_motion` 20 条 + 开关 1 条全绿；全量
  `cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected` 20:15 **1902 passed / 0 failed / 120 ignored**；
  aios-database 源码零告警；`rustfmt --edition 2024 --check` 五份文件干净。未起服务。

### S4 残余收口：`direct_read_mode()` 本体与 7 处调用删除，legacy「noun 表 + `pe_owner`」房间读路由与预览的 PE 分区分支退役

refactor plan（`docs/plans/2026-09-08-external-increment-refactor-plan.md`）S4 / Q24 的尾巴：`options.rs` 里
`pub const fn direct_read_mode() -> bool { true }` 上午已退成常量，但 7 处调用还挂着，`lib.rs` 那条钉写着「这个函数不存在了」
却不成立。本笔按「先改钉、再删调用」收口，数据面从此**只有一条读路由**，没有任何代码再问「是不是 direct」。

- **`room_topology.rs`（先改钉）**：`room_model_routes_both_topology_entries_by_read_mode` → `..._to_file_topology`——钉的变成
  「发布清单先于实时文件；`direct_read_mode` / `load_room_panel_groups(` / `active_data_db()` 一个都不许回来；legacy
  `load_room_panel_groups` 函数体不许回来」。模块文档改口。
- **`room_model.rs`**：`load_room_panel_groups_by_mode` 去掉 `if direct_read_mode()`，发布清单 → 实时文件两步到底；legacy
  `load_room_panel_groups`（SurrealDB `FRMW` / `SBFR` + `pe_owner` SQL）删除。函数名 `_by_mode` 留作历史（T4 收掉发布清单
  分支时一并改名 / 内联）。
- **`on_demand_model.rs`**：5 处化简——凭证判「当前」= `generation_root_cache_current && memory_root_current`；三处计数一律
  `memory_counts`（进程内投影）；legacy `instance_counts` / `written_instance_count` / `COUNT_CHUNK`（读 `inst_relate` 表数实例）删除。
- **`manual_update.rs`**：`preview_one_dbnum` 的 `if direct_read_mode() { 窗口规划器 } else { legacy PE 分区五步 }` 只留前半；
  钉 `the_preview_replays_the_execute_partition_step_for_step`（钉 legacy 五步次序）改为
  `the_preview_takes_units_from_the_window_planner_not_a_replayed_partition`（钉走 `build_effective_model_update_plan`，legacy 五步与
  `direct_read_mode` 一个都不许回来）。老 PE 规划器本体（`model_update_plan::build_model_update_plan` 与 `Transform` 那一档）
  **未动**——删它之前先裁计划 §7.1 G5（Transform 档搬不搬）/ G6（Q17 的 CATA `ref_rev` 臂要不要留）。
- **`options.rs`**：`direct_read_mode` 本体与文档删除；`DATA_READ_MODE_ENV` 那句「没有任何代码再问是不是 direct」从此为真。
- 验证：`cargo check --lib --tests` 通过（aios-database 零告警）；`rustfmt --edition 2024 --check` 五份文件干净；全量
  `cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected` 16:54 **1872 passed / 2 failed**——两条红是
  另一会话同时把库级水位盖章点搬到 Task 尾部（ADR-065）后过期的旧钉，不在本笔文件里，该会话 17:01 自己改成负向钉；17:03 复跑
  **1874 passed / 0 failed / 120 ignored**，17:16 再跑（含元数据门与 H1 两笔）**1875 passed / 0 failed / 120 ignored**。
  未起服务。`--skip` 的那一条自 09-07 起硬崩测试二进制，属既有问题（计划 §5 已把门写成带 `--skip` 的准确命令）。

### 预览的「影响不影响模型」接到双票制谓词上：同一份回执里 `details` 与 `units` 不再是两句话

元数据门落地之后，一份回执里的两句话仍出自两个判据：`details.model_affecting` 走
`merge_net_change_details` → `classify_operation_impact != Skip`，而 `Skip` 那一侧当时只问效果表的六名单；
`units.will_generate` 走 direct 窗口规划器的双票制（效果表 `DataOnly` **且** E3D 字典 DCHC = 0）。差集就是
`FUNCTION` / `CACHID`：字典没投票，规划器照旧整根重算，预览却说「不影响模型」。范围比修门之前窄，但同一份回执里
`details.model_affecting = false` 与 `units.will_generate = true` 还是会同时出现。

- **判据合一**：`classify_operation_effects` 的 `DataOnly` 那一支改问
  `model_impact::attribute_is_metadata_only`——只拿到效果表一票的名字落 `Regen` 而不是 `Skip`，与
  `window_root_plan::change_is_metadata_only` 逐名相同。`OperationImpact::Skip` 这句承诺因此重新为真：它说的是
  「执行侧不会为它排模型工作」，而不是「我们这张表觉得它是元数据」。
- **跟着同一个开关走**：`merge_net_change_details` 退成读一次 `options::model_skip_metadata_only_changes()` 的薄壳，
  纯形态是新的 `merge_net_change_details_with(range_eles, metadata_gate)`。门关上时规划器逐位回到「任何属性变了就
  整根重算」，于是这里一条 `Skip` 都不许剩（`Skip => !metadata_gate`）——否则关掉开关反而让预览开始说谎。
- **`units` 不用另接**：`build_unit_rollup` 的 `will_generate` 本来就只在 `model_affecting` 时置位，两句话从此
  同源。计数不跟着变：门管的是生成不生成，`modified` / `added` / `deleted` 照记。
- **老 PE 路径的位移**：`partition_operation_impacts` 直接问 `classify_operation_impact`，所以 `FUNCTION` / `CACHID`
  在那条路上也从 Skip 变成 Regen（「多算」方向，与 direct 一致）。门关上那一支不动它的模型工作项——
  `mask_details_to_regen` 会拿分区结果再掩一次，掩的依据是原始操作；变的是预览计数与 CATA 反向级联种子
  （`build_cata_cascade_plan` 直读 `model_affecting`），两处都是多算。
- **钉子**：`the_skip_verdict_uses_the_same_double_vote_as_the_window_metadata_gate`（`DATA_ONLY_ATTR_NAMES` 逐名
  对齐 + 混一个没拿到第二票的名字整次操作即 Regen + 源码钉门那侧问的是同一个谓词）、
  `with_the_gate_on_only_the_double_voted_names_are_not_model_affecting` / `with_the_gate_off_even_a_rename_is_model_affecting`
  （薄壳两态）、`the_public_merge_reads_the_same_switch_as_the_window_planner`（源码钉薄壳读的是窗口规划器那同一个
  开关）、`the_details_verdict_and_the_unit_verdict_are_one_sentence`（**整条链**：一窗走完 `merge → build_unit_rollup`，
  钉 `model_affecting` 与 `will_generate` 逐例同真同假、计数不受门影响——两头各钉半条看不出口径分叉）。
- 验证：`cargo test --lib` 全量 **1869 passed / 0 failed / 120 ignored**（22.9 s）；`rustfmt --edition 2024 --check`
  改动文件干净。**没做**：live 现场没复跑（本轮无 E3D 会话）；`CACHID` 这类字典外 DataOnly 仍在门外，要放行得先补
  字典快照或改判据。

### direct 窗口规划器的元数据门：只改了 `NAME / DESC / PURP / LCHKDA` 的元素不再让它的根整根重算（`model_skip_metadata_only_changes`，默认开）

起因是用户问「方位的变化、模型的变化、不影响模型的变化这几类是否处理了」。答案是：三类在老 PE 规划器上分开
处理（`classify_operation_impact` → Regen / Transform / Skip，CONTEXT「模型影响」词条），2026-09-04 direct 成为
默认之后这一层**没有搬过来**——e3d-model L2 `diff_element` 只回答「有没有属性值变了」，变了就 `nearest_unit`
上卷 → 根进 `touched` → `RegenRoot`；改一个 ELBO 的 NAME 就是它的 BRAN 整根重算，改 ZONE 描述就是整区。
用户拍板先接「不影响模型」这一档（「只动方位」那档要先定发布语义，另议）。

- **e3d-model `increment.rs`**：新入口 `plan_update_with(base, target, window, limits, metadata_only)`，
  `plan_update_bounded` 退成 `|_| false` 的薄壳。Modified 候选在 `ledger.record` 与 `is_unchanged` **之后**、
  `nearest_unit` **之前**问一次谓词：为真就 `settle(contributed = false)`——记 `no_model`、不上卷、不级联
  （不进变换级联也不进桥架分支级联），`continue`。账本已经记过 → 闭包种子照收 → 凭证前移那一层一个根都不少收；
  读账本的房间结构触发照常看得见它（FRMW 改名命中关键字仍出 `RoomRecalcPanel`）。`IncrementReport` 多一格
  横切计数 `metadata_only`（已含在 `no_model` 里），`totals_line` 跟着多 `(metadata_only=N)`。名单**不在**
  e3d-model 里写第二份：它只收闭包。
- **gen-model 谓词双票制**：`model_impact::attribute_is_metadata_only(name)` = 效果表 `DataOnly` **且** E3D 字典
  DCHC = 0（core 自己也不为它重画）。两票缺一不放行：`FUNCTION` / `CACHID` 效果表说 DataOnly 但不在字典快照里
  （`raw_dchc_code` 回 `None`）→ 不放行，照旧上卷——所以 ams8000 255→256 那窗（PANE 改 CACHID）仍 regen 3，
  九钉死窗口逐字不变；`NGMR` / `PKEY` / `STYP` 字典说 0 但效果表认它是几何 / 依赖输入 → 不放行。今天真正放行的
  就四个名字：`NAME DESC PURP LCHKDA`（有测试把这份名单钉死，多一个少一个都红）。
  `window_root_plan::change_is_metadata_only(diff)` 再加四条结构条件：没换类型、没换属主、成员表没动、不 `opaque`，
  且 `attributes` 非空——空的是「没变」或「只动了结构」，都不是这里的事。
- **接线**：`WindowRootSources::collect` 在开关 `model_skip_metadata_only_changes`（`DbOption.toml`，环境变量
  `AIOS_MODEL_SKIP_METADATA_ONLY_CHANGES` 压过，**默认 true**）开着时走 `plan_update_with(.., change_is_metadata_only)`，
  关着时逐位走 `plan_update`——这是「少算」方向的开关，关的位置必须留着。计划回执 `warnings` 在门判掉过候选时多一行
  `metadata-only gate — N modified candidate(s) …`。
- **钉子**：`metadata_only_needs_both_the_effect_table_and_the_dictionary`（双票制 + 名单钉死）、
  `a_change_is_metadata_only_when_every_attribute_is_data_only_and_nothing_structural_moved`（谓词真值表：四条
  结构条件各红一次、混一个几何 / 位姿 / 依赖属性即红、CACHID / FUNCTION 不放行）、
  `the_metadata_gate_is_wired_through_plan_update_with_behind_its_switch`（源码钉：开 → 谓词 → 关 的顺序、谓词只读
  `model_impact` 的双重判据、本文件不许再写属性名单）、`the_metadata_only_gate_is_on_by_default_and_can_be_shut`。
- **live**（`#[ignore]` 手动，都跑绿）：
  - `live_ams8000_pinned_windows_land_in_the_expected_buckets` 门开着照跑：九窗 regen / delete **逐字相同**，
    每窗 `metadata_only=0`（255→256 的 CACHID 因字典没投票而不被判元数据）。
  - 新门 `live_ams8000_metadata_only_gate_never_adds_and_never_shrinks_the_closure`：沿 ams8000 链逐窗算两态
    （`plan_update` vs `plan_update_with`）钉三条契约——`regenerate_on ⊆ regenerate_off`、`regenerate_derived_on ⊆
    regenerate_derived_off`、`remove` 逐字相同；两态**祖先闭包逐字相同**（种子照收的实证）；`no_model` 只多不少且
    多出的 ≤ `metadata_only`，`unchanged` 相同，账本守恒两态都过。默认最后 40 步；`AIOS_METADATA_GATE_STEPS=500`
    跑完**整条链（1→415）**：**25 个自然窗口**有候选被判元数据（5→6、6→7、49→50 … 133→134、304→305），其中 24 窗
    那个元素本来就没有单元（关着也是 `no_model`，省 0），**304→305 省下 1 个派生容器**——一个成员只改了名，关着时
    上卷路过 BRAN `24384/26338` 把它记进派生集整链重算管身，开着时不算；管身不读名字，这一链省得对。第一版契约把
    `regenerate_derived` 写成逐字相同，就是这一窗把它改成了子集。
  - 要验自己在 E3D 里造的改名窗：`AIOS_METADATA_ONLY_WINDOW=base:target` 指定那一窗，门要求 `metadata_only > 0`
    且 regen / derived 严格变少，否则红。13:25 那一轮没有 E3D 会话没跑，语料里 25 个自然窗口是当时的替代证据；
    **19:48 接班会话补跑**（`l3_suite --check-driver` 的守卫写入，`outcome=Completed`）：`scripts/e3d/codex_md_rename_apply.mac`
    只改 db8000 SCTN `/-SR-CSV-S6137-BAR-3`（`=24384/25639`，属主 FRMW `/-SR-CSV-S6137/MAIN`）的 NAME 并 SAVEWORK → 会话 433；
    `codex_md_rename_restore.mac` 写回原名 → 435（434 / 436 是 TTY 退出时的「Saving Meta Data.」记账会话，只动
    `DBMDAT.DBMVAL`，没有单元祖先）。`AIOS_METADATA_ONLY_WINDOW=432:433` 与 `434:435` 各 **1 passed**：
    `candidates=1 metadata_only=1 regen off=1 on=0 saved_units=1`，两态闭包逐字相同、账本守恒；默认 40 步自然扫描把这两窗
    点名为元数据窗，其余 38 窗（含同日 8 轮 FTUBE 位姿窗 417–431）契约 1/2 照过。挑 SCTN 不挑 EQUI：EQUI 是根不是单元，
    改它的名关着门也是 `no_model`，钉「regen 严格变少」的门会红（saved=0）。库净变更为零（名字已写回），链多 4 个会话。
    顺带坐实 15:0x 那三份改名宏为什么每次都 `0xC0000005`：文件第 10 / 20 行带着编辑器读出的 `    10|` 行号前缀，
    PML 解析不了——与写入无关（去掉 NAME 与 SAVEWORK 的对分探针同样崩）。
- 验证：`cargo test --lib -- metadata_only metadata_gate a_change_is_metadata_only …` 9 passed；e3d-model 自己的
  `cargo test --lib -- increment element_diff ledger` 24 passed；两条 live 门如上；`rustfmt --edition 2024 --check`
  四份文件干净。**全量 `cargo test --lib` 当时没能跑**：同一时段另一会话正在做 S4（`options.rs` 里 `direct_read_mode()`
  已删、15 个调用点还在），共享工作树暂时编不过，与本笔无关；本笔的定向测试是在那一刀落下之前跑绿的。
  **S4 落定之后补跑**：`cargo check --lib --tests` 干净，全量 `cargo test --lib`
  **1869 passed / 0 failed / 120 ignored**（共享工作树当时的总数）；两条 live 门在 S4 之后原样重跑仍绿——九窗逐字不变，
  `AIOS_METADATA_GATE_STEPS=500` 整链复现 25 个元数据窗口、304→305 仍是那 1 个派生容器的省；
  e3d-model 自己的 `increment element_diff ledger` 24 passed。
- **没做**：「只动方位」那一档（direct 路径没有 `Transform` 执行物：根级 CAS / 内存投影 / 落款 / 变更通告四件事要先定，
  是 ADR-056 D3 的一条例外，另议）；把 `CACHID` 这类字典外的 DataOnly 放进门（要么补字典快照要么改判据，先不动）。
  预览 `details.model_affecting` 与规划器口径对齐当时也列在这里，**已由本文件上一条补上**（同日，见「预览的
  『影响不影响模型』接到双票制谓词上」）。与同日重构计划 S4 的关系：S4 删老 PE 规划器时，Skip 那一档
  从此只剩 direct 这一份实现；Transform 那一档若不先搬就随之消失，已在会话里点名。

### external 形态下目录库前移不再是运行期盲区：sweep / `/update/execute` 触发目录版本核对，`ensure` 的凭证判据比目录落款，`/health.catalogue_reconcile`

起因是「默认 external（rocksdb + ingest）形态下数据 / 模型增量还有哪些地方要完善」的审核，H1：**目录库
（CATA）的会话在运行期前移，摄入形态下没有任何东西知道**。证据链全在现码——数据面 `UpdateScope::admits`
只放 SYS meta 与 MDB 声明的 DESI / ISOD（ADR-058 D1），变了的 CATA 文件在 sweep 里落进「不在 MDB 声明名单里」
那个桶就结束；P2 / P3 的目录级联入口 `reconcile_catalogue_versions` 全仓只有三个调用方——启动那一次
（`lib.rs` 的 `reconcile_model_coverage_at_startup`）、`/update/execute` 与 `sweep_watch_dirs` 的**读透**分支——
摄入分支两处都没有；按需 `ensure` 的凭证判据 `generation_roots_cache_current` 只比设计库会话，目录指纹
`cached_root_target_matches` 是根已经被拉去重算时「重盖章还是真算」的省算门，不是触发器。后果：改一个 SCOM
参数 / GMSET 并 SAVEWORK，external 现场的库与 plant-ui 上所有引用它的根几何原样不动、`/health` 一声不响、
`dbnum_consistency` 照样 `in_sync`，直到下次重启。09-06 级联计划 P2 那句「摄入形态的 CATA 数据批次照旧走
`build_cata_cascade_plan`（老路）」在 ADR-058 D1 之后已不可达，是文档假设漂在代码后面。与同日
`2026-09-08-external-increment-refactor-plan.md` 互补：那份把「CATA→DESI 级联换 dab 反向表」列为另起，本笔
不换源，只把已有的 P2 / P3 入口接到摄入形态的运行期。

- **(a) 摄入分支的 sweep 观察目录会话**（`increment_manager.rs` `sweep_dirs`）：选中的 CATA 正本在范围门
  **之前**读一次最新会话记进 `catalogue_observed` 并 `continue`——不再落进「不在 MDB 声明名单里」的桶（对它
  那句是事实性错误：它在 MDB 里，只是不是数据面的类型）；DICT 照旧往下走进 Meta 批次。整面重扫收尾把这份
  观察交给 `model_update_pending::reconcile_catalogue_versions_after_sweep`：进程里第一轮只记（启动那次核对归
  `reconcile_model_coverage_at_startup`，`run_cli` 里它排在 `init_watcher` 之后，再跑就是同一批根排两遍）；
  之后任一目录库前移 / 新出现 → **另起任务**跑一次目录版本核对（不在 sweep 里等：sweep 的 await 睡在看门狗
  事件循环上，反查要开 MDB 每个设计库）；周期对账那一拍（`origin == "reconcile"`，默认 300 s）**强制**复核一次，
  兜住 sweep 看不见的形状（启动那次反查失败、目录文件不在监控目录里）。上一轮见过、这一轮读不到头的库不算
  前移（未知不触发），会话回退不算前移（那是数据面回退阻断的事）。子集补扫（share-remount）不交观察。
  三条入口（启动 / sweep / execute）共用一把串行闸 `CATALOGUE_RECONCILE_SERIAL`。与 d-635 Q2「CATA 文件会话前进
  → 文件差分 → 走 side_effect_pending 通道」的偏离与 P2 落地记同一条：不新增任务种类，观察是进程内的，
  排出去的 `RegenRoot` 行本身 durable，崩在观察与排工之间由 300 s 那一拍补。
- **(b) `/update/execute` 摄入分支**（`handlers.rs`）：扫描入队之后另起任务 `reconcile_catalogue_versions_now("update-execute")`，
  与读透分支的 `reconcile_model_coverage_at_startup` 对称；回执 `warnings` 多一句「目录版本核对已随本次执行
  另起任务……结果见 /health.catalogue_reconcile」。
- **(c) `ensure` 的凭证判据多一半**（`model_update_pending.rs` `generation_roots_cache_current`）：那条主键查询
  多取 `published_model_target.catalogue ?? []`，命中 = 完成终态 ∧ 设计凭证不早于文件最新 ∧ **每条目录落款
  不早于该目录文件最新**（纯函数 `catalogue_stamps_current`）。目录库读不到 / 不在当前 MDB / 历史落款缺格 =
  未知，放行——与 `stale_catalogue_dbnums` 同一口径，判成过期会让一个读不到的目录库把每次显示都变成重算。
  目录文件最新按 dbnum 只读一次（`latest_source_version` 自带按文件戳的缓存）。
- **(d) `/health.catalogue_reconcile`**：最近一次运行期核对的回执（进程内，不读库）——`origin` / `trigger`
  （`catalogue_file_advanced` / `periodic` / `requested`）/ `advanced_catalogue_dbnums` / `stale_catalogue_dbnums` /
  `unreadable_catalogue_dbnums` / `catalogue_cascade_degraded` / 逐库窗口 / 反查回执，形状与
  `initialization.startup.reconcile` 那一格同源；`null` = 本进程还没跑过运行期核对。控制台：周期复核没发现过期库
  **一个字不打**（300 s 一拍），文件前移 / 人工触发总回一句，有过期库时逐库一行 + 反查一行（与启动横幅同款）。
- **源码钉 +3**：`the_ingest_sweep_hands_catalogue_sessions_to_the_model_face`（观察在范围门之前且随即 continue、
  不进 `out_of_scope`；交接门在 `SweepSurface::Full`、`reconcile` 那一拍强制；顺序 三个桶 → 第四句 → 交接 → 入队，
  不借三种嗓音）、`explicit_execute_reconciles_catalogue_versions_and_health_exposes_it`、
  `generation_root_credentials_use_one_primary_key_query` 改钉新语句。纯测 +3：`catalogue_stamps_are_current_only_when_no_stamp_is_behind_its_file`、
  `a_sweep_reconciles_catalogues_only_when_a_file_advanced_or_the_tick_forces_it`（第一轮只记 / 前移 / 新库 /
  读不到不算 / 回退不算 / 周期强制）、`the_runtime_catalogue_reconcile_speaks_only_when_it_has_something_to_say`。
- **live 门 `live_ams7000_catalogue_window_regenerates_referrers_and_fails_stale_stamps`**（`reverse_reference.rs`，
  `#[ignore]` 手动）：09-06 计划 P3 说主门该改到 `ams7000_0001`、却一直没跑。实跑发现本机语料 `ams7000_0001`
  的会话链只留 5 个会话（1 / 237 / 238 / 262 / 263，MERGE CHANGES 折过）：262→263 零变更，其余是 69,054 /
  69,076 / 274,544 件的导入级窗——**语料里没有精确窗**。门于是退到 C8 / P4 live 门的同一手法：从 DESI 的 `SPRE`
  反向表拿一条指向 7000 的真边（`7000/98` SPCO ← `24381/180852` in 7997）当假定变更源，反查回 **11 个设计引用者 →
  11 个根**（点查 638 次），那条边的 referrer 必须在结果里；(c) 的判据对链上每个旧会话判过期、对 263 判命中。
  跑绿；它证的是文件半边 + 判据，DB 半边（`roots_behind_catalogue_on` / 排工）仍由 mem:// 单测钉。
- 验证：`cargo check --lib` 过；`cargo test --lib -- catalogue sweep health explicit_execute reverse_reference generation_root_credentials`
  **125 passed / 0 failed / 10 ignored**；全量 `cargo test --lib` **1859 passed / 0 failed / 119 ignored**（合并后的
  共享工作树）；live 门 1 passed；`rustfmt --edition 2024 --check` 四份文件干净。
  **未起服务**：external 真库上「改一个目录件 → SAVEWORK → 不重启看 gen_root / /health」这一条端到端没跑
  （要一台 external 实例 + E3D 会话），审核里 H1 的验收原文就是它，仍按未证记。
- **没做**：目录落后不进 `dbnum_consistency` 判决（ADR-060 刻度是数据窗口，有意不改）；
  `out_of_scope_reason` 里「参与类型为 Meta、CATA 与 MDB 声明的 DESI」那句过期文案与 `DataPhase::Catalogue`
  的删除（ADR-058「Catalogue 相删除仍未实施」）不在本笔；09-06 级联计划 P2 那句过期假设未回灌。

### external（ingest）增量审核结论 + 重构计划 S0：启动复核两栏口径、数据等待受阻降级、live #1 操作单

用户问「增量更新是不是都针对 kv-mem、external 下还要重构哪些」。grill 三轮 26 条裁决全按推荐项，
计划落 `docs/plans/2026-09-08-external-increment-refactor-plan.md`。结论一句：**增量主链只在 ingest
（= external / rocksdb）跑，kv-mem = 读透是跳过数据面的减配**；external 真欠的是 PE 规划器 + legacy 读路由退役、
稳态 SAVEWORK 链**一次 live 都没有**，以及——同日下午核出并由用户改判的那一条——**模型工作没有按 dbnum Task 内联**：
`execute_frozen_batch_body` 的 `defer_model_phase` 含 `job.epoch_id > 0`，而每次 sweep 都给本轮全部批次盖
`begin_discovery()` 的纪元，于是每一批（含稳态 SAVEWORK 与人工 execute）都把模型推进 `model_update_pending`
等空闲轮全局 drain，`run_unit_worklist` 那条 Task 内算模型的臂不可达（计划 §1.4）。用户随之拍板（计划 §3.0 U1–U5）：
**模型工作一律按 dbnum Task 执行、以数据 / 模型两枚水位为基准、`model_update_pending` 整体退役（含房间任务）、
初始化基线同样内联**；原「L1 模型车道」与 ISSUE-029 那组 attempts / 死信裁决作废，T1–T5 排进计划 §4。
**ADR-065**（`docs/adr/ADR-065-model-work-runs-inside-the-dbnum-task-and-pending-retires.md`，D1–D8）同日 13:10
用户过目通过、已接受：作废 ADR-050，修订 ADR-056 N1/N2、ADR-025 两阶段、ADR-060 D5–D7、ADR-062、ADR-058 D4；
代码一行未动，S4 起等工作树里另一会话收口后开。本笔是 S0（不撞车的那些）：

- **#6（09-04 审核）**：`/health.startup.no_credential_dbnums` 的分母从 `DbnumState::list_registered()` 改成
  **当前 MDB 声明的 DESI**（新 `model_source::mdb_design_dbnums`）——登记表里有 SYST / DICT / CATA 与换过 MDB
  的历史行，拿它当分母会把没有模型面的库报成「无凭证」。监听限定域照旧套在这一栏上。
- **#7**：`StartupReconcile` 多一栏 `not_in_mdb`——`gen_root` 里有凭证、却不在当前 MDB 的历史行（新
  `model_source::source_file_lookup` 把「不在 MDB」与「成员名单读不到」分成两个答案）。它**不参与** `result`
  取重（抽成纯函数 `startup_reconcile_result(window_failed, unreadable)`）：此前留着一行历史库
  `result` 就一辈子出不来 `ok`。控制台单独点名（最多 12 个）。
- **#10**：`run_cli` 等数据清单的循环有了退出口。判据是「**受阻**」不是「久」（Design 基线在大项目上合法地跑
  几小时）：`blockers` 非空**且**队列里没有 `queued/running` 批次，连续 `STARTUP_DATA_STALL_DEGRADE_SECS = 180`
  → 放弃等待、降级进入监听：`mark_startup_data_wait_degraded` 记进协调器（`/health.initialization.startup_data_wait_degraded`）、
  跳过模型收敛等待与启动房间重建、完成横幅标「数据受阻，降级进入监听」。**不动** `data_ready`、不清阻断——
  阻断是真的，只是主线不再为它挂死。
- **live #1 操作单**：`scripts/Run-ExternalIncrementLive.ps1`（prepare / serve / baseline-wait / mark / watch /
  report 六阶段）：从 `db_options/DbOption-init-7997.toml` 派生隔离配置（隔离 rocksdb 目录、18109 / 18122 端口、
  `watch_dbnums = [7997, 7999]`、`startup_autorun = true`、`room_resident_model = false`），每次 SAVEWORK
  `mark` 打点、`watch` 盯到 `model_verdict = in_sync` 并按 120 s 预算记 PASS / SLOW / TIMEOUT 进 `ledger.md`，
  顺手核 `/health` 的手动执行计数没动。长驻进程都在用户终端里跑，脚本只派生配置与记账。
- 护栏 +4：`the_no_credential_column_is_measured_against_the_mdb_not_the_registry`、
  `startup_reconcile_names_libraries_outside_the_mdb_separately_from_unreadable`（含纯函数取重真测）、
  `the_data_wait_is_stalled_only_when_blocked_and_nothing_is_running`（纯函数）、
  `the_degraded_data_wait_skips_model_wait_and_rooms_and_is_named_on_the_banner`（源码钉六处顺序 + 不许假装就绪）。
- 验证：`cargo check --lib --tests` 通过；`cargo test --lib -- startup initialization model_coverage …`
  **96 passed / 0 failed**；四份 .rs `rustfmt --edition 2024 --check` 本笔改动干净（`lib.rs` 余下一处是别的
  会话的 F-H 测试行，没动）；`Run-ExternalIncrementLive.ps1 -Phase prepare` 干跑一次，派生键逐格核过后清掉。
  全量 `cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected` **1858 passed / 0 failed /
  119 ignored**（既有护栏 `the_catalogue_flag_is_named_separately_and_never_downgrades_the_result` 原来按文本钉
  `result` 的两条分支，随取重抽成纯函数改为钉函数体，判据不变）。**未起服务**：#10 的降级路只有纯函数 +
  源码钉，真受阻的现场要等 live。
- **没做 / 后排**：#8 python `full_init`；S1 之后各步（ISSUE-029 / PE 退役 / L1 / L2）等工作树里另一会话的
  37 个文件收口后再动（计划 Q26）。

### 常驻房间模型 P4：启动对比留下的尾巴不再被一轮干净的 drain 洗白；换代 diff 接上护栏与 `/health`

修一个**静默数据错误**加落一条护栏。开关 `room_resident_model` **默认仍关**，关着时每一处走今天的路。
计划见 `docs/plans/2026-09-07-resident-room-model-plan.md` §3 P4（P4 落地记）。

- **洞（M2）**：写点 ③（`settle_resident_reference`）放行的前提只有「本轮无失败」与「房间表连死信都不剩」，
  而启动对比（`apply_startup_plan`）里失败或无几何的面板此前**既不入队、也不标脏**——它们不在房间表里，
  也不产生 drain 失败，两条前提对那一摊**恒真**。于是启动之后任意一轮干净的 drain 都会把参照推进到当前
  拓扑，而参照的含义是「库里的归属边就是按这份拓扑算出来的」；下次启动的 diff 就把那几块从没算成的面板
  判成「没变」，陈旧的边**永久留下、且无日志**。这正是写点 ③ 自己的注释声称要防的事，守卫只覆盖了 drain
  侧、漏了启动对比侧。
- **修法两条一起做**，它们各自堵的是对方够不着的那一半：
  - **(a) 没算成的面板折成 `RoomRecalcPanel` durable 目标**（`enqueue_room_panel_recalcs`，走既有入队入口，
    不在 `room_startup.rs` 里拼 SQL）。四条路一视同仁：删边失败、重算失败、**无几何**那一摊、以及改列失败
    那间房的面板（改成整间重算，多算不漏算）。入队之后 `room_work_is_drained()` 自动为假，第二条前提真的
    挡得住，`/health` 现成的房间队列与死信观测也看得见。`dbnum` / `source_end_sesno` **都取 0**，与几何触发
    那条路一字不差：启动对比不是一次保存窗口（宁可留空不填假值），更要紧的是 drain 与清空判定共用的范围
    谓词是 `(dbnum?:0) = 0 OR (dbnum?:0) IN [watch_dbnums]`——填真实库号会让未监听库的面板（「无几何」那一
    摊的主力）入队之后既没人消费也不被看见，等于白入队。
  - **(b) 进程内闸**：`StartupCompareReport` 多一格 `unsettled`，**缺省是「有尾巴」**，只有 `Unchanged` 零写入
    与显式全量**写上参照**两处才清成 `None`；写点 ③ 多第三条前提 `startup_compare_tail()`。它堵的是入队本身
    失败、以及**启动对比压根还没跑**——worker 起在 `run_cli` 的房间段之前，它的空闲轮完全可能先跑一轮干净的
    drain。重启丢标志不构成漏洞：没有回执 = 拒写，缺省就在保守那一边；而参照没写 → 下次启动同一份 diff →
    同一摊尾巴 → 闸重新立起。
- **护栏（M3，P4 本体的一半）**：`ensure_resident_rooms` 不再把 `EnsureOutcome.diff` 丢掉（改回
  `ResidentRound{model, diff}`）；换代那一轮 diff 非空而本轮面板目标一块都没落在 diff 上 → 限流告警
  （理由变了或隔 300 s，与 `announce_file_round_holds` 同一条纪律）。三种不出声的形状分开写：diff 为空、
  有命中、**面板页满**（判据不完整，那是「判不了」不是「没响」）。护栏排在 `loaded == 0` 那道早退之前——
  一个目标都没有恰恰是最该出声的形状。
- **只告警、不标脏**（与 P4 原文的有意偏离）：标脏会把下次启动拖进全量重建，而护栏本身还没在真库上验过，
  误报的代价太大。理由写进了告警文案本身。
- **`/health.room_build.resident` 多一格 `last_swap`**：`{form, key, diff, touched_panels, matched_targets,
  round_targets, guard_fired, note, at}`，每次换代都记（不然「护栏没报过」与「压根没换过代」分不开）；
  `startup_compare` 那一格多 `requeued` / `unsettled` 两项。
- **源码钉 +5**：写点 ③ 的第三条前提且零 I/O 的闸排在两条查询之前；`diff` 不许再被丢掉；护栏排在空轮早退
  之前；启动对比四条没算成的路各自收进 `unsettled` 且**收完才入队**；尾巴缺省有、`unsettled = None` 恰两处。
- 验证：`cargo test --lib` **1849 passed / 0 failed / 118 ignored**（本笔之前 1841，新增 8 条真测）；
  `cargo check --tests` 通过；四份 .rs `rustfmt --edition 2024 --check` 干净。**未起服务**。写点 ③ 那条闸的
  **端到端**没测——`settle_resident_reference` / `room_work_is_drained` / `persist_reference` 都钉死在进程级
  `SUL_DB` 上，`mem://` 喂不进；测到的是判据（纯测）+ 入队真的落成待消费目标（`mem://`）+ 写点 ③ 真的问了
  这道闸（源码钉），中间那段接线按未证记（07 计划 §7 新增五条）。

### 常驻房间模型 P2-b：几何侧接常驻模型，三角网经 `MeshCache` 跨代复用，`mem://` 两态逐边门

P2-a 只接了拓扑，几何侧照旧每轮 `load_consumption_panel_index` 查一遍库、每块面板各读一次 `.mesh`
——跨代复用（`MeshCache`）在增量侧一直没生效。这一笔把面板侧几何也接上，并补上 P2 唯一的硬验收。
开关 `room_resident_model` **默认仍关**，关着时每一处走今天的路。计划见
`docs/plans/2026-09-07-resident-room-model-plan.md` §3 P2（P2-b 落地记）。

- **接缝是显式参数，不是读开关**：新类型 `room_model::ResidentSource<'a> = Option<&'a ResidentRoomModel>`
  ——`Some` = 用常驻那份，`None` = 走今天那条查询。`room_model.rs` 里一处开关也不读（源码钉守着），
  开关仍只在换代点判一次（`ensure_resident_rooms`，串行锁之内）。多一处判定就多一个能和换代点漂开的
  地方：两处判出不同答案时，拓扑来自常驻而几何来自库内查询，先清后写会按两份不同的输入改写同一批边。
- **`PanelIndex` 开着时不查库**：模型收在索引身上（`PanelIndex::resident`），`load_panel_index_from`
  在查库**之前**早退；整间分支从 `panel_index.resident()` 取的必然是元素分支同一份，类型上不可能一个
  走常驻一个走查询。对外形状（`missing_panels()` 的口径与顺序、`candidate_panel_refnos()`、
  `usable_panels()`）与今天一致，元素分支的判定顺序一字未改。
- **三角网经 `MeshCache`**（键 = `geo_hash` + 世界矩阵位模式）：`PanelMeshes` 拆成 `from_rows`（今天：
  各读一次 `.mesh`）与 `from_resident`（`panel_tri_meshes`，跨代复用）；后者**全有或全无**，一块面板
  的网格有一张读不出来就整块判不了，不留半份。启动对比（`room_startup::apply_startup_plan`）顺路也
  改成用它已经装好的那份模型，此前白读一遍盘。
- **全量重建恒不接常驻**：`load_panel_index` / `cal_room_refnos` / `recalc_panel_membership` 在类型层面
  永远传 `None`——它是显式修复入口，常驻模型不可信时正是靠它重算出正确的边。`execute_item` 的房间臂
  同样恒 `None`：单件路径不在 `lock_spatial_serial` 之内，而换代点的前提是调用方已持这把锁。
- **`GroupCache` 只留两代**（与 `ResidentRooms` 同口径）：一代 = 一个 `(composite, 层级)`，一代里每个
  文件各一条，同代再来一个文件是并进去而不算新一代。此前从不淘汰——ams7997 一个文件是 1 077 组 /
  3 937 面板，读透侧每 SAVEWORK 攒一代，进程活得越久留下的死代越多。
- **常驻的 `missing_panels` 口径核过了，有一处有意更严**：一条行都没拿到、唯一那行包围盒不可用，两边
  同集合同序；**只有「半份几何」不同**——一行可用一行坏，今天既进索引（拿半份几何去算）又进 `missing`
  （保护存量边），两件互相矛盾的事同时做了；常驻侧整块不进 `panels`，这一轮不算它。按保守那边办：
  没改常驻去迁就今天，而是让开关开着时更严。关着时行为一字未动，有专门的测试钉住这个差。
- **`mem://` 两态逐边门**：一份内存库夹具（两间房、四块在册面板——两块有几何、一块一条行都没有、一块
  包围盒溢出成 inf；三个构件——完全落在里面 / 谁都不沾 / 跨边界），同一份拓扑建两份 `PanelIndex`，比
  ① 索引对外形状、② 整间分支的输入（`world_aabbs` 与三角网顶点/索引）、③ **逐边**：元素分支第一轮的
  产物渲染成 `room_relate` 写入语句**逐字**比（边 id、目标、`room_num`、`inside_count`、`center_dist`、
  保护子句的 `NOT IN` 名单都在那段文本里）、④ `MeshCache` 的 `Arc::ptr_eq` 跨代命中。门里有非空断言
  （三个构件必须各走到三条不同的路），并做过变异检验（给常驻侧 `world_aabb` 加 7.0，②③都变红）。
  为此把元素分支第一轮抽成纯函数 `element_first_pass`——它本来就不碰库，抽出来两态才能同进程对拍。
- **门只比到语句，没落库比表**；面板侧 `room_panel_relate` 的成员集合要 `GLOBAL_AABB_TREE`、读透形态的
  `query_valid_insts` 没有 `_on(db, …)` 变体，两者 `mem://` 都喂不了，只比了输入。P2 验收原文「`room_relate`
  / `room_panel_relate` 逐边相同」里 `room_panel_relate` 那一半**没做到**，已记进 §7。
- **源码钉 +5**：全量入口恒 `None` 且本文件不判开关；关着时走的仍是 `query_room_insts` + `from_rows` 且
  开着时必须在查库之前早退；常驻分支不许出现 `des_mesh_file` / `get_tri_mesh_with_flag`；`GroupCache`
  的代数上限与 `pop_front`；外加 `GroupCache` 的行为门。
- 验证：`cargo test --lib` **1841 passed / 0 failed / 118 ignored**；`cargo check --tests` 通过；
  `rustfmt --edition 2024 --check` 四个文件干净。未起服务，真库两形态逐边仍未跑。

### 常驻房间模型 P2-a：增量的拓扑源改成常驻模型，换代点落在串行锁里，一轮全成写参照

常驻房间模型（`room_resident.rs`）此前只在启动那一下用得上——`ensure_current` 的生产调用方只有启动段两处，
稳态的每一轮 drain 照旧各读一次房间拓扑（ams7997 实测 6–7 s / 文件）。这一笔把它接进 `drain_rooms_selected`。
开关 `room_resident_model` **默认仍关**，关着时每一处走今天的路。计划见
`docs/plans/2026-09-07-resident-room-model-plan.md` §3 P2（P2-a 落地记）。

- **换代点在串行锁之内、读任务之前**：新入口 `ensure_resident_rooms(db_option)` 排在 `lock_spatial_serial()` 与
  `published_spatial_version()` 之后、`let panels =` 之前，内部只调 `room_resident::ensure_current(data_face(), …)`
  ——哪一路模型由数据面决定，不在调用方点名形态。身份键没变是常路（一次探针查询，直接复用上一轮那份 `Arc`）。
  锁外换代 = 算到一半模型被替掉、拿旧拓扑写新边，两边都不报错；读完任务再换代则是拿新拓扑配旧派生的任务。
- **拓扑从常驻模型取**：`let rooms` 开着取 `model.map.clone()`（与 `load_room_panel_map` 同形，都出自
  `registered_map`），关着走原来那一行。`None` 有三种——开关关着、这个构建没有房间层级、摄入侧还没有发布
  清单——都回到今天的读法；建不起来才是 `Err`，整轮止步，不拿半份拓扑改写存量边。
- **写点 ③：一轮 drain 全成之后写参照**：末尾仍在串行锁之内调 `settle_resident_reference`，要求本轮
  `report.failures.is_empty()` **且** `room_work_is_drained()`——房间表里连死信都不剩。少了后一条就可能有一块
  面板的边还停在上一份拓扑上，参照却宣称「边 == 这份拓扑」，下次启动的 diff 把它算成没变，那条陈旧的边
  永久留下且无日志。写失败只出声、不让本轮 drain 失败（退路是下次启动 `NoReference` → 全量一次）。
- **源码钉 +3**：换代在锁之后、两态各自的取法与开关门控、写点 ③ 的两条前提（死信也算没消费）。P1 那条
  `membership_paths_only_take_the_model_through_for_membership` 放宽为「经 `for_membership` 或 `ensure_current`」
  并补禁直接摸 `resident_rooms()`——`ensure_current` 内部是唯一调 `for_membership` 的地方。
- **几何侧没动**（P2-b）：`load_consumption_panel_index` 仍每轮查库、面板三角网仍按面板各读一次 `.mesh`，
  `GroupCache` 仍不淘汰。跨代 `MeshCache` 因此在增量侧还没生效。`mem://` 两态逐边门未跑。
- 验证：`cargo test --lib` **1829 passed / 0 failed / 118 ignored**（本笔之前 1826）；`cargo check --tests` 通过；
  `rustfmt --edition 2024 --check` 干净。未起服务。

### 房间增量审核 F-C：一根失败单元不再把整个库的文件轮次永久钉在 `collecting`

`publish_completed_file_round` 的完成屏障（追平冻结右端 + 批次成功 + 交付单元全部生成）只在数据批次跑完那
一刻判一次。任一根生成失败，这一轮就停在 `collecting`；失败单元随后被空闲重试救回来，**却没有任何地方再调
一次发布**——全仓只有 `execute_frozen_batch` 与发布队列收口两个调用点。后果不是慢一点而是永久：该轮房间目标
不释放，`has_collecting_file_rounds()` 恒真，而它是**全局**判定，于是所有库的 `e3d.room.lookup` 恒为
`updating`，启动也一直走 `file_round_private`。plant-1 三轮实测每轮都有 4 根 `generation_hold`，现场可达。
定级与修法见 `docs/2026-09-07-room-increment-audit.md` F-C（决议 Q10 (a)+(c)）。

- **完成屏障拆成同名一对，判据各写一次**：`batch_round_complete`（批次侧，手上有内存里的单元回执）与
  `file_round_readiness`（durable 侧，输入只有 `ModelWatermarkProbe` + 这一轮冻住的右端，因此**批次跑完之后
  还能再判**）。后者是纯函数，回 `FileRoundReadiness::{Ready, DataBehind, RootsChasing, RootsAbandoned}`。
- **空闲轮补上那个缺失的判点**：`idle_round` 在数据侧本轮 `Settled` 之后、房间轮之前调
  `republish_settled_file_rounds`——`SideEffectCompensator::collecting_file_rounds()`（新增）列出全部收集期
  轮次，逐个用 durable 判据复核，`Ready` 的走 `publish_completed_file_round`。发布过折成 `MoreWork`（下一圈
  立刻回来收房间），失败折成 `Failed` 走 `IDLE_WAKE` 退避、轮次原样留在 `collecting`。非 DESI 与被发布队列
  接管的库跳过，两条守卫与批次侧同形。
- **已放弃的根不自动放行**：`http_422` 与死信让判据停在 `RootsAbandoned`（ISSUE-029 的确定性失败），硬发布
  等于拿残缺几何换掉上一份好的。但不再是**静默**卡着：`announce_file_round_holds` 按「理由变了或隔 300 s」
  出声，`/health` 的 `spatial_reconcile.target_sessions[]` 多一格 `blocked`，与空闲轮同一判据渲染——面板上
  写的就是它下一轮会做的判断；判据读不出时把错误写进这一格，不让 /health 整块报错。
- **`updating` 的按 dbnum 收窄没做**（Q10 (c) 的后半，故意留白）：`published_file_input` 是复合清单，房间可以
  跨文件（ADR-062「跨文件视图 = A 旧成功版 + B 新版」）。只看「本元素所在库有没有 collecting」会把一间跨库
  房间报成 `current`，那是拿可见性不变量换一句好看的状态。留待与 F-H 的口径一并处理。
- 验证：`cargo test --lib` **1826 passed / 0 failed / 118 ignored**（本笔之前是 1823 / 1 failed，唯一那条红的
  `room_increment_audit::f_c_idle_round_republishes_file_rounds_settled_by_retries` 现已转绿）；新增两条纯测
  把两侧屏障各钉一遍，durable 那条走的正是「一根失败 → `collecting` → 重试成功 → `Ready`」的转折点。
  `rustfmt --edition 2024 --check` 对两份改动文件干净，`cargo check --tests` 通过。**未 live**：Q10 验收要的
  mem:// 端到端（失败批次 → 重试成功 → 轮次 `done`、房间目标 `pending→done`）没跑，它要一台带
  `E3dModelService` 的实例，与 F-F、S5 的真库门一起做。

### 房间启动对比与应用（常驻房间模型 P3 后半截，S1–S3）：扫描 → 三层版本对比 → 只对变了的面板删 / 重算 / 改列 → 参照落款

用户口径「启动时扫描全部房间数据，与 SurrealDB(rocksdb) 里已有的房间信息对版本，有增量就重算变了的
房间、删旧关系写新关系」——此前四步里只有前两步落地（`initialize_at_startup` / `plan_startup` 纯函数，
无生产调用），启动段仍是整库全量；默认的摄入形态下对比还是恒等式（参照就是 `room_published_topology`
本身，`advance_orphaned_manifest_rows` 让拓扑在启动时变了却无人重算）。方案与拍板见
`docs/plans/2026-09-08-room-startup-compare-and-apply-plan.md`（D1–D4 全按推荐，共识 d-42）。

- **版本判定三层，不用时间**：① 单文件 = E3D `sesno` + 稳定文件身份 → `commit_token`；② 文件组 = 令牌
  排序后 SHA-256 = `ResidentReference.key`；③ 内容 = `topology_digest` = `.digest`。`plan_startup` 先比 ③
  （等 → `Unchanged` 零写入），再看 ②（同键不等 → `IntegrityMismatch`：库被恢复 / 手改），键不同才拿 ①
  的令牌从 `room_topology_source` 分片合成旧视图 → `TopologyDiff` → `Apply`；旧视图读不回 →
  `ReferenceUnreadable`。**两形态同一张臂表**：摄入侧的 `source_tokens` 改填清单 DESI 行的 `commit_token`，
  参照不再拿 `room_published_topology` 旧键行当依据（那些行无人管、不保证在）。
- **参照落款**（`room_resident::persist_reference` / `persist_reference_on`）：两形态都核对每个令牌的分片在且
  完整（复用 `room_topology_source::exists_on`），缺一片或没有令牌都拒写并点名——宁可下次多算一次，不写
  一条读不回来的参照。只在「边 == 这份拓扑」成立的瞬间写：启动对比应用全成（写点 ①）、显式全量全成
  （写点 ②）；一轮 drain 全 `done` 那处（写点 ③）随 P2。
- **新文件 `src/fast_model/room_startup.rs`**：`startup_room_reconcile` 编排（止血口 → 空树盖平凡章 →
  `lock_spatial_serial` → `ensure_current` → 显式入口 `AIOS_ROOM_FULL_REBUILD=1` / 持久 `rebuild_required`
  → 读参照 → 需要时合成旧视图 → 计划 → 处置）；`apply_startup_plan` 删 → 重算 → 改列，每块面板 / 每间房
  一个事务，**删与写只经 `recalc_panel_membership_for_consumption`**（面板不在新视图它自己落进「已不在册」
  分支清边），**改列只经 `render_renumber_statements`**，本文件不另写删除 / 插入语句；无几何的面板先按常驻
  模型分出来不算、边不动、按**文件** dbnum 进回执，且挡参照。全量降为五条显式入口，`startup_room_reconcile`
  只**判出**、交回 `lib.rs::reconcile_rooms_with_resident_model` 跑——只有这一臂受 `startup_autorun`，
  对比本身不受；凭据对账（`reconcile_startup_room_build`）不再决定跑不跑，每次成功全量照旧盖章。同键不等与
  参照不可读先把理由落进 `room_build:main.rebuild_required`，全量被开关挡住下次也知道要跑。写点 ② 多一道
  身份核对：全量在锁外跑，期间清单换了代就不写参照（回执点名，下次启动 `NoReference` → 再全量一次）。
- **`lib.rs`**：房间段多一个 `else if room_resident_model()` 分支排在 `skip_startup_room_build` 之前，开关关着
  时老路逐字不变；新常量 `ROOM_FULL_REBUILD_ENV`；`vacuous_room_build_reason` 放开给新文件。
- **`/health.room_build.resident`** 多两格：`reference`（读库 2 s 上限；`null` = 没有参照，`{error}` = 读失败
  / 超时）与 `startup_compare`（本进程最近一次对比的回执：计划摘要、删 / 重算 / 无几何 / 失败 / 改列计数、
  `reference_written` 与原因、走了全量的理由、耗时）。启动日志一行同款：
  `[房间启动对比] {form} 参照 K@D（时间）→ 现状 K@D：{计划}；删 a/b / 重算 n（成功 · 无几何 · 失败）/ 改列 …；参照已更新 | 参照未更新（原因）；耗时`。
- **源码钉**：`room_startup.rs` 生产代码不含 `DELETE` / `INSERT RELATION` / `RELATE `；`apply_startup_plan`
  不含 `build_room_relations` / `RoomWriteGuard` / `startup_autorun`；编排顺序 止血口 → 空树 → 锁 → 换代 →
  读参照 → 计划，不含 `reconcile_startup_room_build`；`persist_reference(` 生产调用点恰两处、各紧邻
  `all_settled()` / `manifest == *expected`；`lib.rs` 里 `build_room_relations(` 生产调用点恰两处；对比臂排在
  老门之前、全量臂顺序 对比 → `startup_autorun` → 全量 → `Ok(()) =>` 写点 ②；`room_files_never_consult_the_watch_scope`
  扩到新文件；`plan_startup` 摄入侧键不等不得回 `IntegrityMismatch`。
- 验证：`cargo test --lib -- room_startup room_resident room_topology_source startup_room_build_gate_tests handlers`
  **84 passed / 0 failed / 1 ignored**；`cargo test --lib` 全量 **1823 passed / 1 failed / 118 ignored**，那 1 条是
  `room_increment_audit::f_c_idle_round_republishes_file_rounds_settled_by_retries`（F-C 空闲轮重发布的审计钉，
  钉的是 `batch_worker.rs`，本笔未碰、当时 HEAD 上同红；**它已由上面 F-C 那一笔转绿**，2026-09-08 实测全量
  0 failed）。`rustfmt --edition 2024 --check` 对 `room_startup.rs` /
  `room_resident.rs` / `lib.rs` / `handlers.rs` 干净。**未起服务、未在真库上跑**：方案 §5 S5 的七条真库门
  （三形状逐边相等、`watch_dbnums=[8000]` 改 7997、手删一片分片、读透三条、原地重启两次第二次零写入）一条没跑，
  ADR-010 §9 那道硬门仍按未证记；`apply_startup_plan` 的删 / 重算走的是增量今天同一条路，单测层只钉了纪律与
  改列事务，边集正确性靠真库门。开关 `room_resident_model` 默认仍关。工作树里同时有别的会话的在途改动
  （57 个文件），本笔只提交自己那几处 hunk。

### 增量数据面的范围口径对账：MDB 声明的 DESI ∩ `watch_dbnums`；sweep 收尾点名「声明了却没解析到文件」的库

起因是一次审核「增量启动初始化怎么管内存、MDB 全部 DESI dbnum 的数据检查有没有实现」：审核稿把
「`[库一致性]` 只出 watch 内两行、518 个库被 `watch_dbnums` 跳过」记成了「所有 DESI 的检查没跑」。
用户拍板：**MDB + watch_dbnums 就是限制了范围的**。代码本来就是这么写的，文档没把话说全。

- **口径记成共识（d-25）并写进 `CONTEXT.md`**：新增术语「本期执行范围 (Update Scope)」，把
  `in_scope_with` 的四道门（外项目运行态 sys → 监听限定 → 调试限定 → MDB 范围）、`[库一致性]` /
  凭证核对 / `/health` / `sync_and_seed_model_coverage` 同套监听门这几处出处写进去，结论一句：
  **增量数据面的有效范围 = MDB 声明的 DESI ∩ `watch_dbnums`**；限定时只出域内几行是预期输出，
  审核与改动都不得拿「MDB 全量」去要求数据面（那是房间 P3-init 自己的原则）。「监听限定域」
  词条补一句它与本期执行范围是叠加不是替代。
- **`sweep_dirs` 收尾第四句**（`increment_manager.rs`）：范围之内**声明了、本轮却没在盘上解析到
  文件**的设计库，聚合成一句 `N 个 MDB /ALL 声明的设计库本轮在监控目录里没有解析到文件…`。
  sweep 是文件驱动的（`WalkDir` 找到文件 → 逐个过门），这样的库在「不在 MDB 名单」「监听限定跳过」
  「调试限定跳过」三个桶里都不会出现，数据面对它一声不响——2026-09-07 现场的 6003 SCBDESI 只在骨架
  `problems` 与手动预览的 `not_in_project` 里出现过。**只点名、不记 blocker、不改范围定义。**
  - 「见过」登记在所有门之前（`seen_design_dbnums`，DESI/ISOD 两种 STYP，不问归属项目）：它回答的是
    「盘上有没有」，不是「进不进本轮」。
  - 候选名单先过 `watch_scope_admits` 与 `debug_scope_admits` 再比对：域外的库不是「没文件」，是本轮
    「没有问过」，点进这一句就是又一次 issue #10 式的嗓音混同。
  - 新增 `SweepSurface { Full, RemountedSubset }` 随 `sweep_dirs` 传入：共享盘重挂后的子集补扫没看
    其余目录，没资格说「没文件」，这一句只在整面重扫（init / reconcile / scope-refresh）里出。
  - 源码钉 `the_sweep_names_unseen_declared_design_databases_only_inside_the_scope`：登记先于范围门、
    第四句排在三个桶之后、门在 `SweepSurface::Full`、过滤里必须问过两道限定门、措辞不借另外三种嗓音。
- **合并计划 §2 / D13 / §5 A2 / §8 对账**：`refresh_if_stale`（2026-09-07 `2352e7fcb`）已经让骨架跟着
  文件走——摄入形态每轮 sweep 开头判旧、读透形态 `lib.rs` 2 秒轮询，`is_current()` 逐库比文件头
  `latest_sesno`，整份重扫后 `install` 原子换代，失败进 `stale_registry`。文档此前仍写「今天骨架永远停在
  启动那一刻」。剩下的是代价不是洞：每轮 28 次读头与 sweep 自己读的 `file_latest_sesno` 重复（未做）。
- 验证：`cargo test --lib -- increment_manager watch_scope update_scope` **73 passed / 0 failed / 10 ignored**
  （含新钉与 `sweep_skip_blocker_pins`），`rustfmt --edition 2024 --check` 对 `increment_manager.rs`
  干净。未启服务、未在真库上跑。对照 2026-09-07 两份现场日志推演：`/ALL` 声明 29 个 DESI、骨架装了 28 个，
  唯一声明了却没文件的 6003 (*MASTER/SCBDESI) 在 `watch_dbnums`（8000 / 7998,8000）之外——按 d-25 它属于
  「监听限定跳过」那一桶的沉默，**不进**第四句；watch 内的库都有文件，所以那两份现场按新逻辑不会多出一行。
  要看到这一句，得在不带 `watch_dbnums` 的进程里跑一次启动序列（6003 那时会被点名），或把域内某个库的文件挪走。
  工作树里同时有别的会话的在途改动（`increment_manager.rs` 的 MySQL 镜像删除等 58 个文件），本次
  **未提交**，只改了上述四处。

## 2026-09-07

### `store_mode` 默认翻回 `external`（Q5 翻案，2026-09-04 那次翻转只活了三天）

用户拍板。`effective_store_mode` 里 `Some(false) => External` / `None => SpawnedMem` 合成
`Some(false) | None => External`——**改的只有「哪一档是默认」这一格**：三档形态、`store_process.rs`
的署名裁决与子进程收尸、`/health` 的 `medium` / `durable`、`PersistMedium` 三值、实例锁口径
一个字都没动，`spawned-mem` 与 `embedded-mem` 照旧要得到，只是从此要显式写。

- **翻回来的理由是一个撞出来的现场**：`test-worklspace/bin/DbOption.toml` 两个形态键都没写，
  默认 `spawned-mem` 下被判成「自己拉一台 memory surreal 绑 8009」，而 8009 上蹲着一台手工起的
  memory 后端 surreal，`owner:current` 空表 → `SignatureProbe::Absent` → `judge_port` 落 `Refuse`
  → **拒绝启动**。两边都没错：那台库不可能有本进程的署名，配置又没说要连它——是**默认值替一份
  没说话的配置做了一个它不想要的决定**，而症状是起不来。
- **护栏的判据一个字没动**，只改了它的说法。`every_db_option_fixture_declares_its_store_mode_and_data_face`
  守的是「不许有配置去依赖那个会变的默认」，与默认当前指向哪一档无关——它在 2026-09-04 那次翻转
  与这次翻回来之间是同一条。失败信息改成「默认翻过两次，依赖它的配置每翻一次就静默换一回介质」。
- 测试 `the_default_store_mode_is_spawned_mem` → `the_default_store_mode_is_external`，并补一句
  「显式写 `spawned-mem` 仍然要得到」——只钉住新默认而不钉住这一句的话，下一次有人把
  `parse_store_mode` 里的别名删了，测试照样绿。
- **`data_face` 跟着翻**：它是从 `store_mode` 派生的（`data_face_for_store`），所以没写形态键的
  配置，数据面同时从 `read-through` 变回 `ingest`。护栏逼着两个键都写，为的就是这个连带。
- **口径反了的注释一起改**：13 份夹具 / 现场配置里那句「默认值**将**翻成 spawned-mem，漏写这一行
  的配置届时会静默换介质」、`Start-Surreal8009.ps1` 的 `.SYNOPSIS` / `.DESCRIPTION`
  （「开箱即用不要跑这个脚本」→ external 是默认、要先跑它）、四个 `Run-*` / `Start-*` 脚本里
  `AIOS_STORE_MODE=external` 的理由（环境变量本身**保留**，不靠默认本来就是对的）、
  `python/src/{lib,exec_api}.rs` 的「只附着不拉起」。上一轮排查的结论就是过期注释会把人送进同一堵墙。
- **顺带清掉 `data_face` 那两句更早就假了的话**（用户点名）：13 份配置里的「默认值是 read-through」
  「环境变量 AIOS_DATA_FACE 压过本键」，以及**源码里同一句谎**——`options.rs` 的 `DATA_FACE_ENV`
  与 `data_face()` 的文档注释都还写着「压过它」。事实是 `data_face()` 自打改成
  `data_face_for_store(store_mode())` 就是**派生量**：配置键与环境变量都只喂 `data_face_notice()`，
  与介质给出的那一档不符时报一行警，决定不了任何事。前一句是本次翻转弄假的（派生值随 `external`
  变成 `ingest`），后一句是更早那次改动留下的。`l3_suite/fixture.rs` 里 `AIOS_DATA_FACE=ingest`
  的理由同款改口——**环境变量都保留**，它们现在的作用是「把意图写出来，写错了当场被点名」。
- 验证：**`cargo test --lib` 1775 passed / 1 failed / 116 ignored**，`options::` 那 58 条
  （含 `the_default_store_mode_is_external`、`every_db_option_fixture_declares_its_store_mode_and_data_face`、
  `data_face_follows_storage_backup_scope`）全绿；唯一那条红是
  `fast_model::room_increment_audit::tests::f_c_idle_round_republishes_file_rounds_settled_by_retries`
  ——同日房间增量审核给**尚未修复的 F-C 缺口**留的「回退即红」钉，只读 `batch_worker.rs`，与本改动无关。
  `rustfmt --check` 对碰过的 `options.rs` / `l3_suite/fixture.rs` / `python/src/*.rs` 干净。
  过程中整仓一度编不过（`vendor/plant-mbd` 与 `src/mbd/branch_query.rs` 的在飞改动，另一个会话
  同时段在改 MBD Phase 5），期间先用「把那几个纯函数与 11 条测试逐字抄进独立文件 `rustc --test`」
  拿到过一次 13 passed 的旁证；**上面那次 `cargo test --lib` 是编得过之后补跑的真结果**。
- **live 实测（同一份配置、同一个端口，只差一个环境变量）**：隔离运行目录里拿
  `test-worklspace/bin/DbOption.toml` **删掉 `store_mode` 那一行**，`v_port` 指一台现起的、
  没有署名的 memory surreal（`owner:current` 空表，与 8009 那台同形）。**A**（无环境变量 = 新默认）
  `数据库已经连接到 AvevaMarineSample, 站点: ws://127.0.0.1:18777`，启动序列走完；
  **B**（`AIOS_STORE_MODE=spawned-mem` = 翻转前的默认）`Error: 端口上是一台没有署名的
  SurrealDB……所以拒绝启动。`——逐字复现 16:25 那次起不来。**署名裁决没松**，只是不再由默认值
  把所有沉默的配置推进那一格。`AIOS_STARTUP_AUTORUN=0`，没碰 8010 真库，没动机器上在跑的服务。
- **真环境复验**：`test-worklspace/bin`（8010 那台 880 万行 `pe` 的 rocksdb 库）换上新二进制、
  把 `store_mode = "external"` 注释掉（`data_face` 本来就没写、无 `AIOS_*` 覆盖）后照常起服务，
  16.53 s 起完：`存储介质：ws://127.0.0.1:8010`、`数据面：ingest`，`/health.sul_db` =
  `{"connected":true,"endpoint":"127.0.0.1:8010","medium":"external","durable":true}`。
  `data_face` 是白拿的连带证据——配置一个字没说，它随 `external` 派生成 `ingest` 而不是
  `read-through`。空间树指纹一致、跳过全量房间重建、`dbnum=8000` 一致到 379，与换二进制前
  那个进程的状态接得上。旧二进制与原配置各留了一份
  `*.bak-storemode-default-20260907` 在同目录。
- 记录：计划 `docs/plans/2026-09-04-spawned-memory-store-out-of-box.md` 新增 §9（翻案经过、
  改动清单、三条在案未做）。
- **落地补记（2026-09-08）**：上面那一格代码当时只改在 `p1p2/gen-model` 那棵工作树里，**从没进过
  任何提交**——`git log --all -S "the_default_store_mode_is_external"` 只搜得到**引用**它的文档
  （`docs/2026-09-07-room-increment-audit.md:8` 点名的那个测试在主树上并不存在）。主树 `e3d-direct`
  收到的只有 13 份配置注释、脚本改口与 `python/src/*.rs` 那一半，`effective_store_mode` 仍是
  `None => SpawnedMem`。这半截状态最贵的一格在现场：`test-worklspace/bin/DbOption.toml` 已经按
  「默认是 external」把 `store_mode` 那一行注释掉了，拿主树二进制跑它就会重演本节开头那个 `Refuse`。
  今天把那一格补进主树，验证：`cargo test --lib -- options:: store_process::` **94 passed / 0 failed /
  3 ignored**（含 `the_default_store_mode_is_external` 与两条护栏），`rustfmt --edition 2024 --check`
  对 `options.rs` 干净。`src/bin/l3_suite/fixture.rs` 里同款的注释改口与另一条线的端口迁移
  （8048/8028 → 18048/18028）混在同几个 hunk 里，**不在本笔**。

### 级联查询换源 P4：设计侧通用一跳反查接进窗口规划器（MDB 逐库并集，默认关）

direct 窗口规划器此前对「谁引用了这一窗变了的元素」一个字都不问：`plan_update` 只做 owner 链闭包 +
变换子树 + CTRAY 整条。P4 把这一跳接上，源仍是**文件自带的 kind-16 反向引用表**（不读 `ref_rev`，
ADR-063）：`data_interface/reverse_reference.rs` 新增 `DesignReferenceCascade`，与 P3 的
`expand_reverse_cascade` 同模块、共用同一份「哪些表是反向引用表」的判据与同一条点查。

- **逐库并集**：一张反向表只答它自己那个库的引用者（表在引用方的库里），所以要逐个开 MDB 的设计
  成员（`E3dModelService::design_sources()`，各带 ADR-054 Q1 的生成会话），本库钉窗口 T 端。这正是
  C8 `cable_endpoint`「只查本库」留下的另一半。
- **行为只有 `POSREF` 一条**：783 库差表（`docs/evidence/2026-09-06-ref-rev-vs-file-table-coverage.md`
  §1.3）里设计→设计、跨单元、e3d-model 几何真读的引用边就它一个（电缆桥架成员的位置参照）。它的
  引用者上卷成根并进 `touched`；其余设计侧表照查，只落进 `edges_by_attribute` 记账。入口与账都在
  原位，几何覆盖扩到读别的属性那天，改动就是往 `DESIGN_CASCADE_ATTRIBUTES` 里加个名字。
- **判不了不静默**：成员库打不开 / 点查读挂 / 这一窗要发的点查次数超 `DESIGN_CASCADE_QUERY_BUDGET`
  （20 万，超了一次都不发）→ 经 `WindowImpact::cascade_degraded` **放弃凭证前移**，已持久化的根全部
  重算。反查答不上来时空集与「真的没有引用者」长得一模一样，取前者就是漏算。预算钉在**点查次数**
  （变更元素数 × 要开的库里反向表总数）上而不是变更元素数上——逐库并集把成本乘上了成员库个数。
- **在案缺口**：`POSREF` 命中但引用者在别的设计库里时只计数不排工（本窗口的 `roots_T` 是本库的）。
  差表说 `POSREF` 目标 0 跨库，这一格今天该恒为 0。
- 开关 `model_design_reference_cascade`（`AIOS_MODEL_DESIGN_REFERENCE_CASCADE`）**默认关**，关着时
  连 MDB 成员名单都不解析，四桶与引入前逐字相同。
- **真库实测，整个语料的批量**（`reverse_reference::tests::live`，手动 `#[ignore]`）：带 `POSREF` 表的
  设计库由测试自己在 5 个项目目录里扫，扫出 **31 个**，与 P0 差表那句「DESI:31 库」逐字对上。
  31 个库逐个拿自己表里现成一条边当假定变更源，反查回来的根与正向 owner 链算出的期望**逐字相等**；
  这一条真的走了逐库并集（peers = 同目录其余 `POSREF` 库），`foreign_referrers` 全语料为空——P0 那句
  「`POSREF` 目标 0 跨库」有了跨库点查的实证。**93 窗两态 `regen` 逐字相同、0 窗不同**，最大一窗
  7 139 个变更元素 / 71 390 次点查；整趟对拍 200 s，点查成本淹没在索引扫描里量不出来。
- **这批对拍改了一处实现**：预算原本钉在变更元素数上（2 000），上面那 7 139 个元素的窗因此判不了 →
  放弃凭证前移 → `regen` 从 251 涨到 495，白重算 244 个根；而它真查也只要 71 390 次、量不出来。
  改成钉点查次数之后，93 窗全部逐字相同。
- **未做**：计划里写的 `increment_planner_parity` 对拍没做——它对的是老规划器的 `ref_rev`，而那条路
  在读透形态下已由 ADR-063 D2/D3 停维护并报错，对拍只在摄入形态下才有意义。两态门只证了「开关不改
  今天任何一窗的结果」，没证「改了被参照件、参照它的根会进 `regen`」——语料里没有这种自然窗口。
  老规划器那三个 `ref_rev` 读点仍未拆：拿 `POSREF` 一条换 603 个名字的资格是收窄不是等价，留给 P5 收口。

### `feature = "sql"` 整条退役：`sqlx` 依赖删除，持久层从此只有 SurrealDB 一个

承接同日 `database.rs` 清底那一条。用户拍板「整条退役」。判据在上一条里已经立住：**这个 feature
一开就是七个编译错**（`AiosDBMgr::get_global_pool` / `get_project_pools` / `get_project_pool` 都不在
`aios_core` 里了），而它从来没被任何默认集 / CI / 脚本打开过——门后的代码跟着上游接口一起腐烂，
没人发现。

编译到的 sql 门只有 6 个文件（仓里另有一批满篇 `sqlx` 的**孤儿文件**：`ssc.rs` / `mdb.rs` /
`metadata.rs` / `pcf/*` / `plug_in/*` / `rvm/*` / `api/*` … 全都没进模块树，`lib.rs` 里那几行
`// pub mod` 就是它们的现状，一行都不参与编译）：

- **`db_model.rs`**（740 → 596 行）：删八个连接串 / 连接池入口（`get_default_conn_str`、
  `default_conn_str`、`puhua_conn_str`、`get_global_pool`、`get_db_pool`、`get_puhua_pool`、
  `get_fuzzy_query_pool`、`get_default_pool`）与三个取池方法（`get_project_pool` /
  `get_cur_project_pool` / `get_project_pool_by_refno`），连同 `init` 里那个只赋值不使用的
  `default_conn`。
- **`tidb_manager.rs`**：`AiosDBManager` 去掉 `project_map` 字段（每项目一个 MySQL 池）。
- **`increment_manager.rs`**（5141 → 4802 行）：删 MySQL 镜像五件套
  （`update_mysql_pdms_elements_simple` / `update_mysql_pdms_elements` /
  `process_mysql_{insert,update,delete}_elements`）与钉它们的源码钉
  `the_mysql_mirror_binds_name_and_reports_missing_dbno`。
- **`manual_update.rs`**：删镜像在生产路上的唯一挂点（每个应用成功的批次同步一次）。
- **`data_state` 整个模块删掉**：它只有 `save_data_state.rs` 一个文件，而那个文件从第一行到
  最后一行都在 sql 门后（往 MySQL 的 `data_status` 表写状态）。`lib.rs` 的 `pub mod data_state;` 一并去。
- **`team_data.rs` 收口**（172 → 23 行）：这是唯一一处**删了门就会留下坑**的地方——它的写出口
  只有 MySQL 的 `TEAM_DATA` 表，门一摘，`sync_team_data` 就变成「把整棵 DB / TEAM 树查一遍再把结果
  丢掉」，而每个 SYS 窗口的 `SideEffectKind::SystDerived` 作业都会付这一趟。按 `CataRefRev`
  的同一口径收口：**队列变体与入队点原样留着**（旧库里排进表的行还要认得、`/health` 的计数不变），
  drain 到它打一行声直接返回成功。真要 TEAM_DATA，它的家是 Surreal 而不是第二个持久层。
- **`Cargo.toml`**：`sql = ["dep:sqlx"]` 与整个 `[dependencies.sqlx]` 段删除（`Cargo.lock` 随之掉
  6 个 sqlx 包）。
- **源码钉** `no_manifest_declares_sqlx_or_the_sql_feature`：钉在 **manifest** 上而不是源码上——
  那批孤儿文件满篇 `sqlx`，源码级的钉会永远红；而只要 manifest 里没有这个依赖，任何一个想复活的
  文件都会当场编不过，这才是真闸。
- 验证：`cargo check --lib --tests --bins` 过；`cargo test --lib` 1764 passed / 0 failed / 117 ignored；
  碰过的 .rs rustfmt 干净。
- **留给下一次**：`SideEffectKind::SystDerived` 的**入队**还在（`batch_worker` 每个 SYS 批次记一条，
  有源码钉钉着），现在排进去只为立刻收口。要不要连入队一起摘，是单独一件事。

### `versioned_db::database` 清底：MySQL 那一摊（`feature = "sql"`）删干净，515 → 411 行

同日三次退役（`sync_pdms` → `sync_total_async_threaded` → `bootstrap_sys_metadata`）之后，这个模块里剩下的
legacy 只有 MySQL：`MySqlMethods` / `create_project_database` / `create_info_database` / `execute_sql`，
四样全在 `feature = "sql"` 后面，仓里**没有任何调用点、也没有任何 impl**。

真正的判据不是「没人调」，是**它已经编不过**：`cargo check --lib --features sql` 停在
`AiosDBMgr::get_global_pool` / `get_project_pools` 上——这两个函数早已不在 `aios_core` 里。
这个 feature 从来没人开（`Cargo.toml` 的默认集、CI 的 `--features`、全部脚本里都找不到它），
于是那段死码烂在那儿很久也没人看见。留着它比没有更坏：它让人以为还有一条 MySQL 落库路，
而持久层只有 SurrealDB 一个（宪法 I）。

- **删**：上述四个函数 / trait，连同它们里面的 6 处 `dbg!`（那是调试残留，不是日志）。
  死掉的 import 一并清：`sqlx`（两行）、`AiosDBMgr`、`DbOption`、`aios_core::{pdms_types,types}::*`、
  `Cow`、`take`、`tokio::fs` 三样、`crate::{consts,tables}::*`、`AiosDBManager`——
  只剩 `SUL_DB` / `Deserialize` / `BTreeMap` / `Path` / `PathBuf` 五个。
- **模块注释**：写清这个模块今天只剩两件事（库文件清单 + `pe` 统计修复），以及四次退役的顺序与理由。
- **源码钉** `the_legacy_full_sync_entry_point_stays_retired` 加第三段：本文件的**代码行**（注释行豁免，
  墓志铭要留得住）不许再出现 `sqlx` / `MySql` / `feature = "sql"` / `dbg!`。实测塞一行 `dbg!(1)` 即红。
- **没做**：`feature = "sql"` 本身还在 `Cargo.toml` 里，另外 6 个文件（`db_model.rs` 12 处、
  `increment_manager.rs` 6 处、`save_data_state.rs` 5 处、`manual_update.rs` / `tidb_manager.rs` 各 2 处、
  `team_data.rs` 1 处）仍有 `#[cfg(feature = "sql")]` 段，`team_data.rs:75` 也编不过。整条 feature 的退役
  是单独一件事，另议。
- 验证：`cargo check --lib --tests` / `--bins` 过；`cargo test --lib` 1764 passed / 0 failed / 117 ignored；
  `database.rs` rustfmt 干净。

### `sync_sys_only` / `bootstrap_sys_metadata` 退役：SYS 元数据不是一道引导，是初始化的一部分

用户口径：「`sync_sys_only` 这个不需要了。默认 SYST 就是初始化时需要检查的，以及增量更新需要处理的。」
ADR-058 D8 那条注记里点名留着的最后一截（「留下的只有 `database::bootstrap_sys_metadata`……」）随之清零。
留它的理由（DESI 解析经 `MDB` / `WORL` 反查世界 refno，没有 SYS 的空库每个 dbnum 都解成「只有根、0 个元素」）
依然成立，但它要的从来不是一道**单独的引导**——那六类库在两条常规路上早就各就各位：

- **初始化**：`needs_initial_load` 明写「从未解析过的库都要先补一次全量基线，**SYS meta 也算**」
  （2026-09-04 起基线与重放同一个收集器之后，把它们排除在外的旧理由就不成立了）。服务首轮重扫
  把 SYST/COMM/MISC/GLB/GLOB 与 DICT 当普通库入队首载，`DataPhase::Meta` 排在 Catalogue / Design 之前。
- **增量**：`UpdateScope::admits`、`watch_scope_admits`、`debug_scope_admits` 对这六类**恒放行**
  ——MDB 的成员名单就存在它们里面，圈掉只会得到一个「什么都没发现」的假现场；水位缺失、行还在的
  情形另有 `SesnoRangeResolver` 的 cold start 兜底。`IncrResult::geometry_changed_refnos` 把它们排除在
  几何刷新之外：SYS 库的窗口只落行与水位。

- **删** `src/bin/sync_sys_only.rs`（整个 bin）与 `database::bootstrap_sys_metadata`
  （及其独占的 `sys_metadata_files`）——`database.rs` 717 → 511 行；`baseline_ingest::SYS_META_DB_TYPES`
  一并删，`ingest_frozen` 收回成模块私有（`ingest_file` 是唯一入口，点名与文件头必须先对上）。
  空库那三个索引不再由引导私铺，交给与服务启动同一套 DEFINE 的 `table_parity::init_schema_on`。
- **新** `AiosDBManager::initialize_project_sys_metadata`：不起服务的规格工具走它——同一个
  `initialize_dbnum_baseline`，顺序 SYS 五类 → DICT（`UdaKeyMap` 要 DICT 先于 DESI），**只补还没有水位的库**
  （已有水位的后续会话归 watcher，不在这条路上追）。`bin/initialize_ams_dbnums` 先跑它再建点名的设计库基线，
  每库打一行 `SYSMETA|项目|dbnum|db_type|ok|元素数`（跳过的打 `skipped|already_initialized`）。
- **调用点**：l3 套件的空店引导与 fixture 各去掉一步 `sync_sys_only.exe`；
  `scripts/Invoke-SiteDbnumParse.ps1` 四步变三步（`-SkipSysSync` 开关一并删）；
  `scripts/Run-E3DFixtureSuite.ps1` 的构建清单去掉这个 bin。
- **源码钉**：`the_legacy_full_sync_entry_point_stays_retired` 的退役名单加 `bootstrap_sys_metadata`；
  新增 `data_interface::legacy_parse_pdms_db_guard::sys_metadata_has_no_bootstrap_of_its_own`
  （`bin/sync_sys_only` 的文件不许回来，`initialize_project_sys_metadata` 的「只补没有水位的库」
  这条前提也不许被摘掉——摘了就又有两套「什么时候算已初始化」的判据）。实测把 bin 放回去即变红。
- **文档**：ADR-058 D8 实施注记四；CONTEXT.md「分块解析基线」词条与 Avoid（新增「SYS 引导 / SYS 同步」
  这两个不该再说的词）；`runbook-sys-reparse-for-model-tree.md` 与 `runbook-local-stack-and-dbnum-parse.md`
  两份跑法改写（后者顺带修掉一处早已失效的 `baseline_sync_options` 描述）。

### ADR-063：`ref_rev` 从「正确性依赖」降为「UI 便利」，反向查询的权威源改成文件里的引用表

用户拍板「接着做 ADR-063（`ref_rev` 降级，级联计划 P5）」。三件事同时成立才逼出这个决策：
① **默认路径已经不读它**——direct + DESI 走 `window_root_plan::build_model_update_plan_from_window`
→ e3d-model `plan_update`（owner 链闭包 + 变换子树 + CTRAY），一次反向引用查询都没有；
② **它正在失去维护者**——`ref_rev` 只在摄入链上维护，而读透（默认 `data_face`）的 G5 定了跳过摄入链；
③ **core 里根本没这个问题**——E3D 的引用表由 dabacon 在 `db_put_reference` 写时同步维护，读侧
`DB_MDB::findElements` 逐库点查、零维护，「联动查询的索引怎么更新」被文件格式吞掉了。
覆盖面 P0 差表已验：783 库实测 75 张反向引用表，**F \ R 在 DESI / CATA 两侧都为空**。

- **新增** `docs/adr/ADR-063-ref-rev-is-a-ui-convenience-not-a-correctness-dependency.md`：D1 选根不读、
  D2 维护降为摄入形态的 UI 便利（读透形态不跑）、D3 读透形态下读它**必须报错**、D4 护栏不对称
  （判不了 → 该库全部根过期 + 点名 `catalogue_cascade_degraded`）。修订 ADR-003 B1-emit。
- **`increment_pipeline.rs`**：`maintain_reverse_index` 与它的补偿入队在读透形态整段跳过；那段
  「`ref_rev` 是关联模型也要更新的权威来源，缺一条边就是某个设计实例静默不重生成」的注释按新口径改写
  （摄入形态仍维护、仍进补偿队列，只是理由从「正确性」换成「别让 UI 看到半张表」）。
- **`manual_update.rs`**：生产侧唯一反查入口 `fetch_ref_rev_edges` 在读透形态 `ensure!` 报错。**这一条是
  D2 的安全前提**——不维护之后查出来的空集与「真的没有引用者」长得一样，静默返回空集就是漏算。报错让
  调用方走既有退化路：`resolve_unit_rollup` 置 `cascade_deferred` + 告警，`expand_live_reverse_cascade`
  让那条工作项失败，两条都看得见。`fetch_ref_rev_edges_on` 保持纯函数，测试照旧直接用它。
- **源码钉** `ref_rev_is_a_ui_convenience_and_never_answers_root_selection`：默认窗口规划器全文无反查、
  维护前必须先问形态、反查入口必须有 `is_read_through()` + `ensure!`。三条钉在一起看才成立。
- **没动**（等 P4 把文件表反查接进规划器）：老规划器的 `resolve_unit_rollup` / `direct_root_allowed` 与
  CATA 级联执行器 `expand_live_reverse_cascade` 仍读 `ref_rev`。先删读点等于把能力从老路上拿掉而新路
  还没有；它们今天的可达性也已经很窄（direct 下 DESI 走窗口规划器、CATA 没有数据批次），且有 D3 兜底。
  `docs/2026-09-04_model-cascade-update-inventory.md` #4 / #5 一并留到 P4。`fast_delete` 清库表单里的
  `ref_rev` 照旧保留——它是 `pe` 的边，与本决策无关。
- 验证：`cargo test --lib --features http_api` **1751 passed / 0 failed / 115 ignored**；两份 .rs rustfmt
  干净。**未 live**：默认形态下改一个 SCOM 参数只有引用它的根重算，那条验收属于 P4，本 ADR 不认领。

### `sync_total_async_threaded` 真退役：基线摄入只剩一个按文件的入口（`versioned_db::baseline_ingest`）

用户拍板「真退役」（承接同日「为什么还在用 sync_total_async_threaded」的答复；方案
`docs/plans/2026-09-07-retire-sync-total-async-threaded-plan.md`，2026-09-04 退役清单 §3.2 路 a / B8 的编排半边）。
它今天只剩壳：收集器（B8.2）、渲染器（ADR-064 D4）、执行器、CATA 闭包、统计五个部件早已与增量共用，
壳里留下的是 legacy 全量同步的编排形状（多类型 / 多文件 / DashSet 去重 / `included_db_files` 过滤 /
`is_replace` / MySQL 分支 / 进度参数）。

- **新增** `src/versioned_db/baseline_ingest.rs`：`ingest_file(db_option, project, path, dbnum, db_type, target_sesno)`
  按**单个文件** `Fresh` 整库写——`spawn_blocking(stream_baseline → render_baseline_batches → 有界队列)` → 单写者
  `dbnum_stats::execute_on` → 文件收尾 CATA 依赖闭包（同一条队列）→ 排空 → 裁决 → 统计收尾。**不动水位**；
  解析 / 写者 / 闭包 / 统计任一失败先 `wipe_dbnum_for_reinit` 再 Err。新硬校验：冻结源反哈希出的
  `dbnum / db_type` 与调用方点名不符就一行都不落（legacy 壳以文件为准、静默落进另一个 dbnum）；
  `save_db = false` 与「文件没有会话」都是显式 Err，不再以「基线完成后仍没有 PE 数据」的形状失败。
  `ingest_frozen` 给 `bootstrap_sys_metadata` 用（目录扫出来的文件以文件头为身份）。
- **统计收尾改无条件重建**（`rebuild_on`，此前 `initialize_on` 只在版本标记落后时算）：基线自 `1eefbd57`
  起走 `StatisticsMode::Deferred`、事务里不再维护统计，父层补缺是同一 dbnum 的第二次摄入、版本标记早已是
  当前——条件重建会跳过，`pe_count != info_count` 直接报 `statistics mismatch`，且该错误在清库判断之后、
  不触发清库，下一轮清库重来照样死在同一处（mem-201）。单测 `baseline_and_parent_gap_replay_keep_actual_pe_counts`
  一直用的就是 `rebuild_on`，生产与测试从此走同一个函数。
- **删除**（`database.rs` 1514 → ~680 行）：`sync_total_async_threaded`、`SenderJsonsData`、`BASELINE_*` 三常量、
  `finish_write_pipeline`、`baseline_cleanup_targets`、`record_write_error`、`check_and_clear_db`、
  `settle_dbnum_info_after_baseline`、`is_retryable_surreal_write_error` / `conflict_retry_backoff` 的副本
  （`surreal_retry` 有同名同体的那份）、`#[cfg(feature = "sql")]` 的 MySQL 写者分支；`versioned_db/task.rs`
  整个删（唯一消费者是 `SenderJsonsData`，且从未被调）；`manual_update::baseline_sync_options` 及其测试删
  （入口直接吃路径与身份，没有清单可绑）。`collect_project_db_files / dbnum_stat_totals /
  stamp_dbnum_info_identity_sql` 升 `pub(crate)` 留在 `database.rs`。
- **调用点**：`initialize_dbnum_baseline` 叶子基线 `ingest_file(.., Some(file_latest_sesno))`、父层补缺
  `ingest_file(.., &parent, .., None)`；`bootstrap_sys_metadata` 改为 SYS 五类全目录扫（ADR-007 语义）+ DICT
  按 `sys_metadata_files` 清单（现回路径），逐文件 `ingest_frozen`，同 dbnum 只落一次、无会话的文件跳过。
- **源码钉**：新模块 I1（`stream_baseline(` 生产调用点唯一）/ I2（不含 legacy 形状）/ I3（身份校验）/ I4
  （排空 → 裁决 → 清库 → 统计 → 回执，两条失败路都清库）/ I5（无条件 `rebuild_on`）；`database.rs`
  的 D8 退役钉加上 `sync_total_async_threaded`；`manual_update` 的两条钉改找 `ingest_file(`，新钉
  `on_demand_baseline_ingests_exactly_the_named_file`；`cata_dependency` 三写入点钉改扫新模块；
  `legacy_parse_pdms_db_guard` 与 `Check-LegacyParsePdmsDb.ps1` 第 4 条改为两处都扫。
- **顺手（用户拍板一起落）**：`dbnum_consistency::verdict` 新增 `NotJudgedReason::NoGenerationRoots`——数据水位
  已建立、模型水位落后、但 `gen_root` 一行都没有（连规划过的都没有）的库判「不判」而不是 `lagging`。现场
  runycyk pass 3 的 5521 / 5523 / 5524 / 5526 / 7027 与 pass 2 的 5522（只有 WORL + SITE）启动日志永远是
  「模型在追赶：数据水位 4，模型水位 0，还在赶 0」。**不能**靠给零根库盖模型水位来消掉——零根库上的非零
  模型水位正是 ADR-060 D7 的幽灵水位（`advance_model_watermark_on` 对 `settled_floor = None` 的库本来就
  `Unchanged`，这是设计）。`model_ready` 不看这个判决（看 `model_coverage_current`），所以此前只是观测失真。
- 验证：`cargo check --lib` / `--bin sync_sys_only --bin l3_suite` 过；`cargo test --lib -- versioned_db
  dbnum_consistency legacy_parse_pdms_db_guard all_three_write_points_persist_catalogue_dependencies
  manual_update::tests` **137 passed / 0 failed / 3 ignored**；`Check-LegacyParsePdmsDb.ps1` 六判全过。
  **真库编排等价对拍已过**（方案 §5 第 3 条，14:52 `plant-1\runycyk3-ingest`）：新 exe 空店一趟跑完 8 SYS/DICT + 10 DESI，
  31 个 dbnum 的 `pe` 行数、`dbnum_info_table`、10 库水位、`MDB / WORL` 与 pass 3 逐一相等（总 71,896 行），5525 抽 200 个
  refno 五种行逐字段 0 处不同；六个零根库在 `/health.dbnum_consistency` 上从 lagging 变为 not_judged。父层补缺与
  `sync_sys_only` 两条路仍未在真库上跑（手头没有母库 + 叶子并存的项目）。

### `sync_live` 退役：watcher 无条件起，「一个库都不监听」这个状态删掉（ADR-058 D8）

用户拍板选 (a)：「永远起 watcher，删掉不监听这个状态，testbed 多付一次扫描无所谓」。前一轮先定了
另一半口径——`watch_dbnums` 的「键缺失」与「空表」**不区分**，两者都等于默认监听本 MDB 声明的全部
DESI（共识 `d-207`）；所以 `watch_dbnums` 永远表达不了「一个都不监听」，`sync_live` 的关闭态没有
接盘人，只能删。

`sync_live` 此前把两件事绑在一个布尔上：①「起不起 watcher」②「做不做启动数据重扫 + 等 `data_ready`」。
现在 ① 恒为是，② 由 `data_face` 单独回答（`read-through` 跳过重扫并直接
`mark_data_ready_without_manifest`，那本来就是这个形状的先例）。

- `lib.rs`：`let sync_live = …` 删除；`init_watcher` 与 `async_watch` 的 spawn 都改成无条件
  （read-through 仍只跳过启动重扫、watcher 照起作脏信号）；`startup_data_ready` 收成
  `read-through || is_auto_work_armed()`；`print_startup_complete_banner` 去掉那个参数与
  「sync_live=false：不进文件监听」那一支；进程长驻的判据从「手动模式 + 配了 HTTP」改成「配了 HTTP」。
  `startup_autorun=false` 那段里「⚠ 同时 sync_live=false，没有文件事件来解封任何一行」的告警随之
  失效，一并删。
- `/health`：`sync_live` 这一格从 `AppState` 与两处载荷里去掉——它现在只能是 `true`，报一个不可能
  为假的布尔没有信息量。plant-ui 侧读的是 `Option<bool>`（`model_update.rs`），缺字段会退化成
  「无记录」，不会崩；那边的清理留给 plant-ui 自己。spec §4.1 的 `/health` 样例同步去掉。
- 配置与脚本：19 份 `DbOption*.toml` 去掉 `sync_live`；`scripts/Invoke-SiteDbnumParse.ps1`、
  `scripts/Verify-RocksDb8000.ps1`、`scripts/strt_plant_ui_runtime.py`、
  `python/scripts/smoke_m2_client.py`、`bin/l3_suite` 夹具生成的配置一并去掉。
  vendor `old-aios-core` 的 `sync_live: Option<bool>` 字段没删（`Option` 缺键即 `None`，别的消费者
  还在读；ADR-056 约束 7，vendor 走上游）。
- 护栏：`direct_read_mode_only_selects_the_data_read_route` 加两条——启动序列不许再读
  `db_option.sync_live`、看门狗必须是无条件 spawn（`let watch_task = {`）；
  `read_through_startup_cannot_enter_the_ingest_sweep` 的两句锚点改到现在唯一还看形态的那两处。

### `UpdateScope` 库侧 MDB 表为空时回落到 SYS 文件声明——新项目首次启动不必再空跑一趟

YCYK-E3D（`D:\AVEVA\Projects\E3D3.1\YCYK-E3D`，MDB `/SLYK`）首次初始化实测撞出来的：全新命名空间上
第一趟只解析 SYS meta、设计库一个都不跑，要跑第二趟才拿得到真范围。但同一个进程在 **82 毫秒之前**
已经把同一份名单读出来了——日志上这两行挨着：

```
13:30:26.451 MDB /SLYK / YCYK-E3D / module=DESI：声明 49 个库（按 STYP {1:10, 2:24, …}）
13:30:26.533 [init] 库里还没有任何 MDB——存放 MDB/CURD 的 SYS meta 库尚未解析。本期只解析 SYS meta…
```

前一行是 `mdb_membership::resolve` 从 `<项目>sys` **文件**读的（e3d-io，不经持久层），后一行是
`UpdateScope::fetch` 查 **SurrealDB 的 `MDB` 表**。两份名单的成员判据本来就同一个（文件侧
`element_databases()` 过滤 `DESI_STYP | ISOD_STYP`，库侧 `MDB_DBNOS` 绑的 `element_db_types` 正是这两个值）。
退化那条注释写的防死结理由（「想更新得先有范围、想有范围得先更新」）在 `init_mdb` 落地之后已经不成立：
死结的另一头有出口了。

- **改**：`interpret` 末尾加 `from_files: impl FnOnce() -> Option<Vec<u32>>`，**只在 `desi` 与 `known` 都空
  那一档**求值；`finish` 的 `Ok` 分支传 `|| file_side_declared(&name)`；新增 `file_side_declared`
  （`mdb_membership::get(project, name)` → `element_databases()` 的 dbnum）。
- **只动一档**：库侧非空的三个出口（正常范围 / 名字打错 `bail` / 有这个 MDB 但没声明 DESI）一个字不看
  文件侧。那三档库里是有内容的，拿文件侧去盖就成了「两个来源不一致时听文件的」，那是另一个决定，没做。
- **用 `get` 不用 `resolve`**：看门狗事件路径缓存未命中时每隔几秒会走到这里，背后不能藏一次 SYS 库全树扫
  （`mdb_membership::get` 自己的文档就是这么定的）。拿不到回 `None` → 维持退化行为；`run_cli` 里
  `init_mdb` 失败只告警不中止，「没登记」是真会发生的状态。
- **不串台**：`get` 按 `(project, mdb)` 精确匹配，所以「请求带的 MDB 名与配置不同」自然落空——拿配置那个
  MDB 的名单去服务另一个 MDB 的请求，正是 `manual_update::update_scope` 明文要挡的东西。
- **不静音**：回落成功换一句新 warning，说清名单取自 SYS 文件、几个设计库、SYS meta 一落库即切回库侧。
- 空表不算回答：`init_mdb` 跑过但这个 MDB 一个设计库都没声明时，退化那句话才是对的。

验证：`update_scope` **13 passed / 0 failed**（四个新用例，九个既有用例一字未改；
`a_non_empty_store_never_consults_the_file_side` 给的 `from_files` 是个 `panic!`，库侧非空只要问一次就红）。
**live 一趟到位**：全新 store，`watch_dbnums` 给 `/SLYK` 全部 10 个 DESI，只跑一趟——8 个 SYS meta 与
10 个 DESI 同一次扫描入队，阶段屏障成立（`design 10/0/0/0/0`、`meta 8/0/0/0/0`），产物与两趟版**逐项相等**：
`pe` 71896、`pe_owner` 83506、依赖行 5168、`gen_root` 2438、`inst_relate` / `aabb` / `trans` / `inst_info` /
`geo_relate` / `tubi_relate` 全等，10 个库的三枚 sesno 全等，9 张 noun 属性表 5682 行整行 sha256 全等。
回落那句 warning **只出现一次**，7 秒后的 `[scope-refresh]` 已从库侧解出同一份名单（自动切回是实测的）。
全库 `cargo test --lib -- --test-threads 1` **1748 passed / 0 failed / 115 ignored**。
计划与完整验收记录：`docs/plans/2026-09-07-update-scope-file-side-mdb-fallback-plan.md`。
**本轮未追**：`fast_model::e3d_mesh_store::tests::damaged_existing_content_addressed_file_is_rejected`
硬崩测试二进制（单独跑那一个模块也崩，与本改动无关，上面那条命令 `--skip` 掉了它）；
`data_interface::project_paths` 那 12 个用例只在并行下红、串行与单独跑全绿（进程级共享状态互踩）。

### legacy 全量解析整条退役：`sync_pdms` 与 `total_sync` / `incr_sync` / `only_sync_sys` / `sync_history` 四个开关一起删掉（ADR-058 D8）

用户拍板「先做第一步：删 total_sync 族与 sync_pdms」。**先清点存量**：30 份配置里 `total_sync` 全是 `false`
（只有样例 `DbOption_text.toml` 写过 `true`），而 direct 自 09-04 是默认形态——`legacy_sync_requested`
在任何一份配置下都是 `false`，那条路**一次都没被走到过**，只剩 `lib.rs` 里一行「忽略 total_sync=… 」的打印。
它还与水位、模型、`gen_root` 凭证全程脱钩，于是有那个写在告警里的坑：`total_sync + sync_live=true` 会让
watcher 首扫把这些库判成「有数据、水位 0」按首次导入 wipe + 重解析，**整库解析两遍**。路没了，坑随之没。

- **删**：`database::sync_pdms`（Meta → Catalogue → Design 三道全局屏障的编排）、私有辅助
  `full_sync_catalogue_files`、`lib.rs` 里的 `legacy_sync_requested` 分支与那段 `start_sync_task` 死注释、
  钉三道屏障先后的源码测试 `sync_pdms_awaits_global_meta_then_catalogue_then_design`。
- **留**：① `sync_total_async_threaded`——它不是 legacy，是**首载 / 重建的单文件基线写者**，由
  `manual_update` 的 `baseline_sync_options` 按单个文件调（`total_sync = true` 那一行删掉，它现在只有这一种
  调法，`is_total_sync && is_save_db` 收成 `is_save_db`，`settle_dbnum_info_after_total_sync` 改名
  `settle_dbnum_info_after_baseline`）；② 新 `database::bootstrap_sys_metadata`——`sync_pdms` 里唯一还有人
  要的那一半：空库灌 SYS 元数据（SYST/COMM/MISC/GLB/GLOB + DICT）加三个索引。理由是 DESI 解析要经
  `MDB` / `WORL` 反查世界 refno，没同步过 SYS 的空库每个 dbnum 都解成「只有根、0 个元素」；`bin/sync_sys_only`
  与 l3 套件的空店引导（`bootstrap_store`）都吃它。它**不看任何配置开关**，调用即执行。
- **顺带删掉的死分支**：`lib.rs` 里 `if db_option.only_sync_sys { sync_team_data }`——TEAM_DATA 生成的唯一开关
  就是那个 legacy 键，仓里没有一份配置打开过，这段从来没跑过。要它就给它自己的开关，`sync_team_data` 本体留着。
- **配置**：30 份 `DbOption*.toml` 去掉这四个键（`docs/evidence/` 下的证据快照与 `vendor/` 里的不动）；
  `scripts/Invoke-SiteDbnumParse.ps1` 生成的站点配置、`bin/l3_suite` 夹具生成的配置同步去掉。
  `vendor/old-aios-core` 的 `DbOption` 三个 `bool` 字段加 `#[serde(default)]`——`config` crate 对缺字段的
  `bool` 是硬报错（`missing field total_sync`），不加这一下所有读根配置的测试当场全红。字段本身没删：
  `vendor/aios-parse-pdms` 等别的消费者还在读（ADR-056 约束 7，vendor 走上游）。
- 护栏 `the_legacy_full_sync_entry_point_stays_retired` 替掉原来那条屏障顺序钉：`database.rs` 里不许重新出现
  那个入口，也不许再读那四个开关；`lib.rs` 的 `direct_read_mode` 出现位置钉从四处收到两处（绑定 + 一行播报）。

验证：`cargo check --lib --tests --bins --features http_api` 干净；定向
`-- versioned_db:: manual_update:: increment_manager:: element_batch::` **191 passed**；全库
`cargo test --lib --features http_api` **1747 passed / 0 failed / 115 ignored**；`cargo clippy --lib --bins`
本轮碰过的文件零警告；七份 .rs rustfmt 干净。**未 live**：`bin/sync_sys_only` 换底之后要在空库上真跑一遍
（l3 空店引导 `bootstrap_store` 那一步），本机没有 E3D31-L3 语料，留给用户终端。

### rocksdb 写回重构 ADR-064 Q2：惰性 CATA 落库切到统一渲染器，`cata_ref_rev` 补偿队列退役——D5 修好库类型字后它第一次真写行，而 `UPSERT … CONTENT` 会抹掉 `dependency` 标记

计划 §1.1 #9 给这条路的处置本来就是「复活时不走这条，走统一渲染器」；它复活了。`cata_closure::ensure_cata_refnos_parsed`
（`cata_ref_rev` 补偿队列 → `preload_cata_for_roots`、resolve 层 `try_lazy_cata_fallback`、staging 预加载共用的惰性兜底）
此前逐元素 `UPSERT pe:X CONTENT {gen_sur_json}` / `UPSERT <NOUN>:X CONTENT` 外加自己攒的 owner / UDA replacement 批。
三步：① 落库切到统一渲染器；② 用户拍板「直接把 `cata_ref_rev` 补偿队列退役（P4 已在水位前落行），不等 ADR-063」；
③ 用户拍板「那一簇死代码现在就清掉」。

- **③ 清掉按需解析的旧机制**（`cata_closure.rs` 2 897 → 1 774 行）：删去根子树播种 `collect_design_subtree_outbound` /
  `collect_database_subtree_outbound(_on)` / `run_cata_closure_pass_for_refnos`、`ensure_cata_parsed_for_roots`、resolve 层
  `try_lazy_cata_fallback`、依赖缓存 `CataDepCache` / `DEFERRED_CATA_CACHE` / `publish_deferred_cache` /
  `discard_deferred_cache` / `preload_cata_for_roots` / `DependencyCacheContext` / `dependency_manifest_fingerprint` /
  `require_all_roots_resolved`、env 开关 `cata_closure_enabled`（`AIOS_CATA_CLOSURE_MODE` 从此不再被读）、开/关几何冒烟
  `GeoSmokeReport` / `geo_smoke_digest`；`src/bin/cata_smoke.rs` 整个删除（它存在的前提就是那个开关）；`batch_worker`
  不再 `discard_deferred_cache`；`bin/node_gen_room_probe` 不再要求开关、不再 `CATA-PRELOAD`（目录行由该库数据批的依赖块
  在水位前落）。留下的：`CataDbLocator` / `InMemoryCataLocator` / `CataClosureResolver`（依赖入库的引擎）、
  `ensure_cata_refnos_parsed` 薄壳（`staging::preload` 引用，随 spec 035 P3 走）、`render_pe_owner_replace`（双引擎对拍
  标准形态）。只钉旧机制的 8 条测试一并删（dep cache ×4、cache context、required roots、staging subtree、preload 段）。
  ADR-004 状态改读「Phase 1 / 2 在用（被 ADR-064 D5 承接），Phase 3 ～ 6 已退役」；ADR-064 Q2 ③、CONTEXT.md 旧章加注。

- **② 补偿队列退役**（`increment_pipeline.rs` / `side_effect_pending.rs`）：`apply_one` 收口后不再 `enqueue_cata_ref_rev`
  （函数与 `cata_ref_rev_summary` 删除）；`SideEffectKind::CataRefRev` 变体只为认得旧库里已排进表的行，drain 到它直接
  `Ok(())` 收口——不解析、不写；`/health.side_effect_pending.by_kind` 不再报这一档（旧行仍计入顶层 `pending`，下一轮 drain
  就收掉）。**影响评估**：(a) 目录行——P4 三个写入点已按 D5 唯一判据在水位**之前**落行，oracle 证明覆盖全部几何实读；
  退役后不再落的只有几何链**之外**的目录引用（ISPEC / TSPEC 指向的保温 / 示踪规格那类），属性面板对它们只显 refno——
  要它就把属性名加进 `CATALOGUE_CHAIN_ATTRIBUTES`，不另开一条路。(b) `ref_rev`——这条路**从未写过一条边**（只写 `pe` /
  noun / UDA / `pe_owner`）；DESI→CATA 边在 `apply_one` 的 reverse_index 步、CATA→CATA 边只有 `rebuild_reverse_index`
  （`bin/rebuild_ref_rev`）会建，维护点一个没变（级联计划「CATA→CATA 边靠 cata_ref_rev 补」那句是写错了，已更正）。
  (c) `CataDepCache` / `DEFERRED_CATA_CACHE` 失去唯一的生产写者，`batch_worker` 里的 `discard_deferred_cache` 成空转，
  仓内没有别的读者（老规划器的 CATA 级联在 direct 下不可达）；`preload_cata_for_roots` / `ensure_cata_parsed_for_roots` /
  `try_lazy_cata_fallback` 从此只剩诊断 bin `node_gen_room_probe` 与 staging 预加载引用，随 ADR-063 / spec 035 P3 一起清。
  (d) 每个 DESI 窗口少一条副作用作业、少一遍按根子树**全部**出向引用的闭包重解析（ams8000 一窗就是 14k 行的重写）。
  ADR-064 Q2 划掉；ADR-056 D8 / ADR-034 / ADR-058 实施约束 4 / spec 035 T126 / 级联计划 §现状 B 与 P5 范围各加注记。
  护栏 `cata_ref_rev_is_retired_and_legacy_rows_are_only_settled`（两处生产正文无 `enqueue_cata_ref_rev(`；drain 仍认得
  `cata_ref_rev` 且臂内无 `preload_cata_for_roots` / `ensure_cata_refnos_parsed`；by_kind 无此键），替掉原来钉入队 / 派发 /
  warning 口径的三条。

- **① 为什么现在**：D5 对拍抓到的第 1 处漏（`on_demand_db::open_single` 反哈希 `descriptor().noun`）也挡着这条路——真库上每个
  目录文件都被拒，它一行也写不进去（plant-1 `run2-forget-cache` 04:52 的 7997 初始化日志还是
  `[cata_closure] 闭包缺失明细: missing=705 … noun 0x00000002 is not a database type`）。库类型字修好后，每个 DESI 窗口
  收口 `enqueue_cata_ref_rev` → 补偿队列一跑，它就把 P4 刚落的 CATA `pe` 行整行覆盖：`gen_sur_json` 没有 `dependency`
  字段，`CONTENT` 整行替换 → `dependency = true` 被抹 → P5 的前移刷新（`dependency_rows_among` / `all_dependency_refnos`
  都按 `dependency = true` 找行）再也认不出这些行，`/dbnums` 的依赖行账也对不上。另一处：它拿 `CataClosureConfig::precise()`
  原样跑，`cata_db_types` 里带 DESI，回指到**别的 DESI 库**的元素连 owner 链到 WORL 都会被解出来写进 `pe`——DESI 行只能由
  它自己的数据窗口写（E1 / E4 两条都破）。
- **① 改法**（`cata_dependency.rs` / `cata_closure.rs`）：闭包配置 `closure_config(Option<u32>)`——三个写入点与惰性兜底同一份
  （`precise()` 容器白名单、db_type **只收 CATA**），差别只在有没有「本批 DESI 库」可排除；新 `resolve_catalogue_rows_with`
  （无排除）与 `resolve_dependency_rows_with` 共用 `resolve_rows_with_config`。`persist_cata_dependencies` 拆出
  `persist_resolved_rows(project, rows)`：D7 前移先 `Replace` 刷新 → `element_batch` `Fresh`（`pe.dependency = true`，
  命中即零成本）→ `cata_dependency_state`。`ensure_cata_refnos_parsed` = 定位器 → `resolve_catalogue_rows_with` →
  `persist_resolved_rows`，`LazyFallbackOutcome` 语义不变（`parsed` = 渲出的元素数、`missing` = 闭包缺失）。
  删掉 `render_content_replacements` / `render_uda_statements` / `render_uda_replacement` / `INSERT_CHUNK` /
  `OwnerReplaceBatches` / `UdaReplacementBatches` 及其 5 条测试（owner 前缀范围删 + 同事务整块重插、UDA 先删后插、切块
  这些保证现在由 `element_batch` 与 `tx_limits` 承担并各有钉子）；`render_pe_owner_replace` 留给 `fork_surreal_compat`
  双引擎对拍当标准形态。（做完 ① 时补偿队列还留着；同日 ② 把它退了，惰性兜底从此只剩 staging 预加载与诊断 bin 会走到。）
- ① 护栏：`lazy_fallback_lands_rows_through_cata_dependency_and_renders_no_upsert`（惰性兜底体内必须走
  `resolve_catalogue_rows_with` + `persist_resolved_rows`、不含 `UPSERT` / `execute_model_write`；`cata_closure.rs`
  生产正文逐行零 `UPSERT `）、`closure_config_collects_only_cata_for_every_entry`。ADR-004 加 2026-09-07 注记、
  计划 §9.2 加两行 / §9.3 改写。

验证：① `cargo test --lib --features http_api -- cata_closure:: cata_dependency:: element_batch::` **46 passed**；全库
`cargo test --lib --features http_api` **1730 passed / 0 failed / 113 ignored**；live 对拍
`live_ams8000_catalogue_reads_are_covered_by_the_dependency_closure`（闭包路径共用 `resolve_rows_with_config` 之后重跑）
仍是 catalogue_visited 5,009 ⊆ closure 13,995 行 / 6 个目录库 / missing 4，**unexplained = 0**（数字与切换前逐项相同）。
② `-- side_effect_pending:: increment_pipeline:: cata_closure:: cata_dependency:: element_batch::` **107 passed**；全库
`cargo test --lib --features http_api` **1729 passed / 0 failed / 113 ignored**（被替掉的 `cata_ref_rev` 三钉换成一钉）；
`cargo clippy --lib --features http_api` gen-model 零警告；四份文件 rustfmt 干净。
③ `cargo check --lib --tests --bins` 干净；`-- cata_closure:: cata_dependency:: side_effect_pending:: increment_pipeline::
element_batch:: batch_worker::` **145 passed**；全库 `cargo test --lib --features http_api` **1735 passed / 0 failed / 115 ignored**
（同一棵树上别的会话同时在加测试，绝对数只对 0 failed 负责）；`cargo clippy --lib --bins` 本轮碰过的文件零警告（其余 bin 的
`too_many_arguments` / `type_complexity` 是既有的）；live 对拍第三次重跑仍 **unexplained = 0**；六份 .rs rustfmt 干净。
**未 live**：真库上一个 DESI 窗口收口后 `incr_side_effect_pending` 里不再出现 `cata_ref_rev_*` 行、`pe … dependency = true`
行数不掉——要起 fork surreal + `aios-database serve`，与真库门一起留给用户终端（plant-1 `run7997.py` 的 7997 初始化
就会走到：数据批 `apply_one` 0..=155 之后旧版本会排 6769 根的 `cata_ref_rev`，新版本不排）。

### rocksdb 写回重构 D5 oracle：e3d-model `catalogue_visited` + 对拍 `catalogue_visited ⊆ closure`——首次对拍抓到三处漏

计划 §3 D5 / ADR-064 Q1。oracle 的口径：e3d-model 一次生成**真读到**的目录元素，必须全部落在 gen-model 按「名单 + 闭包」
算出来的依赖闭包里；闭包比实读宽允许，窄就是漏。

- **e3d-io**（`../vendor/e3d-io`，未提交）：`read_stats::trace_element_reads()` 线程局部、可嵌套的读记录器，
  `DbElement::parsed` / `extraction` 读成功即记（缓存命中也算，悬空引用 `NotFound` 不算）；`DbSet::db_type(dbno)`、
  `DbElement::db_set()`。记录点在读入口而不是逐个调用点：目录链每一跳、P 点求值、剖面、桥架目录件都在里面，新增读法不用改。
- **e3d-model**（`../vendor/e3d-model`，未提交）：`GenerationOutcome::catalogue_visited`——DFS 前开记录器、DFS 后收账，留下
  「不在根所在库 ∧ 所在库为 CATA」的；源码钉 `catalogue_visited_is_traced_around_the_whole_dfs`；真库门
  `tests/catalogue_visited_real.rs`（只有目录侧 / BRAN 成员 SPRE→SPCO 一个不漏 / 链走到 SCOM 与点集几何集）。
- **gen-model**：`cata_dependency::resolve_dependency_rows_with(locator, …)`（闭包只有一条路，定位器可注入）；live 对拍
  `live_ams8000_catalogue_reads_are_covered_by_the_dependency_closure`（`--ignored`，文件扫描定位器，与 e3d-model 真库门同一口径）。
- **对拍抓到并修掉三处**（都是真库上才显形、`mem://` 单测看不见的）：
  1. `on_demand_db::open_single` 反哈希 `descriptor().noun`（第 0 页自身的 noun 字）当库类型——真库上**每个目录文件都被拒**
     （`noun 0x00000002 is not a database type`），闭包一行也解不出、全部计 missing、只出 warning：P4 的三个写入点在真库上
     一直没写进过一行 CATA。改读 `db_type_name()`（`db_type_hash`），与 `scan_db_header` / `db_file_header` 同口径。
  2. `CataClosureConfig::precise()` 容器名单没有正体：Core3D 对每一个有成员的正体开差集，e3d-model 照做，所以 SBOX / SCYL /
     LCYL / SCON / LSNO / LPYR / SCTO / SRTO / SSPH / SSLC / SDSH / SDIS 名下的负体（NSBO / NLPY / NSRT / NLCY）是实读；补进名单。
  3. 名单里写的是属性名 `PSTR`，p-line 集的 noun 是 `PTSS`（`SPRF.PSTR → PTSS → PLIN`，型材对齐）；改为 `PTSS`。
- 顺手：`dbnum_stats::rebuild_on(db, dbnum)`（`versioned_db::baseline` 的测试引用它而它不存在，lib test 编不过——08:31 那次
  整批文件重写把两份文件带成了不同版本）。
- ADR-057 B2 白名单追记 `GenerationOutcome::catalogue_visited`（当前只有对拍消费）；ADR-064 Q1、计划 §9.3 D5 划掉，§9.2 加行。

验证：e3d-io `cargo test --lib` **234 passed**；e3d-model `cargo test --lib` **269 passed**、`cargo test --test catalogue_visited_real`
**1 passed**（ams8000 三根：visited 6,120 / catalogue_visited 5,009 / BRAN 成员 SPRE 抽查 2,634 条全中）；gen-model
`cargo test --lib --features http_api -- --ignored cata_dependency::tests::live_ams8000_catalogue_reads_are_covered_by_the_dependency_closure`
**`unexplained = 0`**（catalogue_visited 5,009 ⊆ closure 13,995 行 / 6 个目录库；closure missing 4 = ref0 23984 的库不在项目 + 5052 里 3 个
悬空引用）；修前同一门 unexplained 5,012（目录文件全被拒）→ 24（负体 + PLIN）→ 0。全库 `cargo test --lib --features http_api`
**1732 passed / 0 failed / 113 ignored**。
另：e3d-model `tests/reference_reads_guard.rs::indirect_reference_reads_are_each_registered_with_their_names` 现在红，是另一条会话
把 `cable_tray.rs` 的间接读点挪进 `rpath.rs`（08:30 那批）还没登记，与本节无关、没动。

### rocksdb 写回重构 U1（plant-ui）落地收口；spec §5.3 补 `model_source_changed` 一行

- **U1 在 plant-ui 仓落地**（`D:\work\plant-code\old\plant-ui`，ADR-0025「模型从哪儿读由服务端按库判决；客户端只消费，
  一次装载里一个库只认一个源」）：`task_queue::DbnumStatus.model_source` / `model_source_reason`（认不出归 `None` 整格不画）、
  `ModelSource` 枚举、库行字形 + 悬停整句 + 行内明细；`model_update_api::model_records` 经新 `model_record_union.rs` 按 `dbnum`
  分桶、同库两源 / 同根分页中途换源报错、老服务端无 `dbnum` 整次一桶；`EnsureReply.model_source` 与装载日志的「其中 dbN 由 API
  从内存供数」半句；WS `model_source_changed` → 只改那一格、不重载场景。Q7 取 (a)。计划 §9.2 加 U1 行、§9.3 与 ADR-064 Q3 划掉。
  验证（plant-ui 仓）：`cargo check --workspace --tests` 干净；`cargo test -p plant-ui-app -- model_record_union model_update_api
  model_update_ws` 14 passed；`cargo test -p plant-ui -- task_queue` 40 passed。gen-model 本仓**只改文档**。
- `docs/specs/web-service-api.md` §5.3 事件表补 `model_source_changed` 一行（§4.12 早已引用它，表里却没有）。
- 仍欠：真库门（P0-1/2/4、P1/P2/P4 哈希对照、§8.4 四道读侧门）要起 fork surreal + `aios-database serve`，留给用户终端，
  命令见 09-06 那节末尾；D5 oracle `catalogue_visited`（vendor/e3d-model，走上游）；ADR-064 Q2（旧惰性 CATA 落库去留，挂 ADR-063）。

## 2026-09-06

### rocksdb 写回重构 P5：CATA 依赖版本前移（Replace）、`inst_geo` 降为 `INSERT IGNORE`、ADR-064、CONTEXT.md 词条

承上一节（同日 P0-3 / P6 / P1–P4），服务端至此全部落地；U1（plant-ui）与真库验收门仍留给用户终端（U1 已于 09-07 落地，见上）。

- **D7 · 目录文件前移**（`cata_dependency::refresh_moved_catalogues`，三个写入点各自入口先跑）：目录文件最新 >
  `cata_dependency_state.sesno` 时，`cata_cascade_window` 在该目录库上差分出变动目录件，与已入库依赖行求交
  （`dependency_rows_among`：`pe.dependency = true` 且 dbnum 相同），被改的重解析闭包经 `element_batch` **`Replace`**
  重渲（边 / UDA / noun 行先删、`pe` 原地整行替换），被删的落最小墓碑；然后推进 `sesno`。差分不可用（会话不在链上 /
  候选超 20,000）退化为该库**全部**依赖行 `Replace` 重渲，回执点名 `cata_dependency_full_refresh`；正常前移不出声。
  只写内存的库（P6 `memory`）不触发。
- **Q4 · `inst_geo` 降为 `INSERT IGNORE`**（`model_db_adapter::RowBatch::render`）：此前那条 `ON DUPLICATE KEY UPDATE
  direct_model, aabb` 是模型面唯一残留的 UPDATE，为修旧渲染器留下的描述符而设；它是内容寻址的（ADR-060 N3 第一种前提），
  `direct_model` 是来源账、`inst_geo.aabb` 读侧（plant-ui `plant-ui-data` 只取 `out.meshed` / `record::id(out)`）不消费，
  改不了任何读者看得见的东西。模型面从此零 UPDATE；旧描述符照旧留着，与「共享网格只增不删」（ADR-060 N2）同口径。
  `shared_prototype_is_identical_in_both_publication_orders_and_repairs_legacy` 改钉新契约
  `…_and_existing_rows_win`（手改过的原型行再发布**不被修回**）；新钉 `inst_geo` 渲 `INSERT IGNORE` 且整段正文无
  `ON DUPLICATE KEY UPDATE` / `UPSERT`。
- **ADR-064** 起草（`docs/adr/ADR-064-element-rows-are-batched-delete-then-insert-and-cata-enters-by-dependency.md`）：
  D1 元素批只有先删后插 / D2 `pe` 原地整行替换（图顶点） / D3 墓碑 / D4 基线与增量同渲染器 / D5 CATA 按依赖入库 /
  D6 `inst_geo` 降级 / D7 模型取数源按就绪度选；新不变量 E1–E5；开放问题 Q1 `catalogue_visited` oracle、Q2 旧惰性 CATA
  落库去留（挂 ADR-063）、Q3 U1。**ADR-058 D1 / S3** 加 2026-09-06 修订注记（「CATA 不入持久层」→「只按 DESI 依赖入库」，
  整库基线仍不做）；**ADR-060 D4** 加实施注记（`inst_geo` 是 D4 唯一漏网，已收口）。
- **CONTEXT.md**：「分块解析基线」改述为经 `element_batch` `Fresh`、`pe_chunk` / `att_chunk` 入 Avoid；新词条
  「元素批 (Element Batch)」「墓碑 (Tombstone)」「依赖入库 (Dependency Ingestion)」「依赖状态 (Dependency State)」；
  「按需解析元件库」整章标为旧章（2026-09-03 ～ 09-06 的过渡口径），现行词是「依赖入库」。
- 计划 §9 实施记录补 P5 行；§9.3 欠账收窄到 U1、真库门、`catalogue_visited` oracle、旧惰性 CATA 落库（ADR-064 Q2）。

验证：`cargo test --lib --features http_api` **1721 passed / 0 failed / 110 ignored**；`cargo clippy --lib --features http_api`
gen-model 零警告。**未 live**：真库门命令同上一节末尾；P5 的目录前移可用现成的 ams5052 184→185 那窗做 before / after。

### rocksdb 写回重构 P0-3 / P6 / P1 / P2 / P3 / P4：元素数据行批量先删后插、CATA 依赖按需入库、模型取数源按 dbnum 路由

计划 `docs/plans/2026-09-06-rocksdb-batch-write-and-cata-dependency-plan.md`（Plannotator 三轮 approved，共识 d-29）。
用户原话：「写入 rocksdb 尽量批量、先删后批量插入、不要单独 upsert / update；写回的数据也要按需保存依赖的 CATA；
未初始化的 dbnum 查模型时 rocksdb 没准备好就由 API 供数给 plant-ui，已初始化的以 rocksdb 查询接口为准」。
本轮落地六段，P5 见上一节，U1 未开工，真库验收门未跑（见末尾）。

- **P1 · 元素数据行只有一种写法**（新 `data_interface/element_batch.rs`）：前置块清 Add / 墓碑自己的旧出向边 →
  每块「边（id 区间）→ `ATT_UDA` → noun 行 按 id 批量删，再 `pe` / noun 行 / `ATT_UDA` / `pe_owner` 每表一条
  `INSERT IGNORE`（边表 `INSERT RELATION IGNORE`）」。被删元素落**墓碑** `pe` 行（`deleted = true`，noun / owner 保留，
  收集器从 `DbSet@S` 读基侧；读不到退化成最小墓碑并在回执出声），被删元素的 noun / UDA 行与换 noun 的旧表行随删除块消失
  （此前软删把它们留在库里）。逐元素 `UPSERT … CONTENT` / `UPDATE pe SET deleted` 从增量路退出；
  `ElementRows::upsert_statements` 改名 `read_through_statements`——它只服务读透形态，不是 rocksdb 口径。
  **`pe` 行不删、原地整行替换**（计划 §9.1）：`pe` 是图顶点，`DELETE pe:X` 会级联删掉 `pe_owner` / `ref_rev` / `tubi_relate`
  这些挂在它身上的边，而这些边由别的块 / 别的窗口 / 模型面维护——崩溃重放对拍（元素上限压到 1）当场抓到 EQUI 少一条
  成员边。改成一块一条 `INSERT INTO pe [..] ON DUPLICATE KEY UPDATE <gen_sur_json 的全部字段>`，仍无逐元素 UPSERT / UPDATE。
- **P1 · 切块上限搬家重定**（新 `data_interface/tx_limits.rs`）：`dbnum_stats` 不再借 `staging::executor` 的 64 KiB / 250 行 /
  100 元素，改为 2 MiB / 5000 行 / 1000 元素（模型侧已验证的口径），`DbOption.toml` 三键 `data_tx_max_bytes` /
  `data_tx_max_write_rows` / `data_tx_elements` 可调；超限劈半。
- **P2 · 基线并轨**：`versioned_db::baseline::render_baseline_batches` 改调同一渲染器（`Fresh`：只插、已有行赢——父层补缺
  语义不变），有界队列每条消息 = 一笔事务（`SenderJsonsData::ElementBatch`），`PEJson / AttJson / PERelateJson` 退役，
  `pe_chunk` / `att_chunk` 不再参与切块。单写者不变。
- **P3 · `ref_rev` 与 `datacenter_version` 批量化**：`build_reverse_index_statements` 全部 referrer 的邻接 DELETE 在前、
  整窗口的边攒成 `INSERT RELATION IGNORE`（2000 边一条）；`datacenter` 的 Modify 归组成多目标
  `UPDATE a, b, c SET status = 'Modify'`（500 目标一条），Delete 仍逐条（各带 `belong_zone`）。
- **P6 · 模型取数源按 dbnum 路由**（新 `data_interface/model_read_route.rs`，spec §4.12）：`database` ⇔ ingest ∧
  `applied_sesno > 0` ∧ 该库没有活着的初始化发布 run；其余 `memory`。`memory` 库上 `/model/ensure` 不等初始化队列、
  不进 durable 队列、不落 rocksdb，直接从文件生成进进程内投影（有初始化 run 就钉它的 `source_sesno`），
  `/model/records` 从投影供数（没 ensure 过回 409 `not_generated:`）；`database` 库一字不改。`/dbnums` 行与两条接口回执加
  `model_source` / `model_source_reason` / `durable`；翻面（run 收口 / 回退重建清库）发 WS `model_source_changed`。
  此前 `/model/records` 按进程介质选源、`/model/ensure` 在初始化期原地等或报错，plant-ui 对正在初始化的库要么等要么错。
- **P0-3 · 双引擎语法门**：`fork_surreal_compat` 加 5 条 mem / rocksdb 双跑（`DELETE $array`、`INSERT RELATION IGNORE`
  撞 id 保留旧行且边是图边、多目标 UPDATE、整块先删后插幂等、`pe` 原地替换保边 + 「删顶点级联删边」对照）。
- **P4 · CATA 依赖闭包按需入库**（新 `data_interface/cata_dependency.rs`；修订 ADR-058 D1/S3「CATA 不入持久层」→
  「CATA 只按 DESI 依赖入库，不做整库基线」，ADR 正文随 ADR-064 落）：seed = 本批 DESI 元素上目录链引用属性
  （`SPRE CATR PRTREF PTRE GMRE GSTR PSTR HSTU LSTU HSRO LSRO`，名单与 e3d-model `reference_reads_guard` 同源、有钉子）
  的出向 refno；闭包复用 `CataClosureResolver`（`precise()` 容器白名单：几何 / 点集 / 轮廓容器，owner 链到 WORL，
  db_type **只收 CATA**、排除本批 DESI 库）；行经同一个 `element_batch` `Fresh` 入库（`INSERT IGNORE`，命中即零成本），
  `pe.dependency = true` 标它是按依赖入库的；每个目录库一行 `cata_dependency_state:{dbnum} { sesno, rows }`
  （不是水位，给 P5 版本前移与 `/dbnums` 的 `dependency_rows` / `dependency_sesno` 用）。三个写入点：
  数据窗口 `apply_one`（排在 DESI 块与水位**之前**，闭包解析出错拦水位、「库不在 MDB / 元素不在」只 warning，
  源码钉顺序）、基线（流式收集时攒 seed，文件收尾一次闭包，经**同一条写者队列**入库，单写者不变；失败按基线失败
  登记清库）、模型根发布前（`visited` 子树从文件读属性做 seed，独立事务、只 warning 不拦发布）。
  **欠账**：D5 的对拍 oracle（e3d-model `GenerationOutcome.catalogue_visited`，纯输出字段）是 vendor crate 改动，
  未做——现在的闭包是「名单 + 闭包」的上界，宽于实读；`catalogue_visited ⊆ closure` 的机器验证留待 vendor 提交。

验证：`AIOS_COMPAT_REQUIRE=1 cargo test --lib fork_surreal_compat::dual_` 16 passed（含既有 11 条）（真起 `bin/surreal.exe` rocksdb 后端）；
`cargo test --lib --features http_api` **1718 passed / 0 failed / 110 ignored**（含 `cata_dependency` 9 条：名单同源钉、
seed 只取目录链、Fresh 批带 `dependency` 标记按目录库分组、`mem://` 上重跑不翻倍、三个写入点接线钉；`direct_window_replay_parity`
生产路每个事务边界 kill → 重放 → 逐表一致，元素上限 1 / 2 / 1000 三种切法；`the_production_window_lands_the_expected_shapes`；
`datacenter_tests::fixed_target_updates_hit_the_rows_the_server_side_walk_hit` 归组后语义对拍照旧绿）；
`cargo check --all-targets --features http_api` 干净；新文件 rustfmt 干净。**未 live**：真库验收门
（`scripts/Verify-RocksDb8000.ps1` 复跑 + 逐表哈希对照 `output/rocksdb-8000-20260905/before-manifest.json`、
一窗 SAVEWORK 计时）要起 fork surreal + `aios-database serve` 跑 ams8000 基线，留给用户在自己的终端里跑：
`cargo build --bin aios-database --features http_api` → 新建一份 run 目录（脚本里的 `$run` 改日期）→
`pwsh scripts/Verify-RocksDb8000.ps1 start` → 基线完成后 `python scripts/rocksdb8000_evidence.py` 取 before / after。

### 浏览器口径三件套：`/api/v1/meshes/{hash}.glb`、树节点带 `dbnum`、`/dbnums` 带 `ref0s`

计划 `plant3d-web/docs/plans/2026-09-06-gen-model-v1-tree-and-viewer-adapter-plan.md` 的 P0（共识 d-641，
D1–D7 全按推荐项）。plant3d-web 要把模型树与三维显示的数据源换到本仓 `/api/v1`，摸底摸出三个硬缺口，
全在服务端补最省——三项都是**加端点 / 加字段**，既有响应形状一个字节没动，plant-ui 不受影响。

- **`GET /api/v1/meshes/{geo_hash}.glb`**（新模块 `src/web_service/mesh_glb.rs`）：把 `.mesh`（rkyv 0.7 归档的
  `PlantMesh`）现场编成 GLB。浏览器解不动 rkyv——那套布局与 rkyv / glam 的内存形状强耦合，让前端手解，
  任何一边升级都会静默读歪（D1 否掉的方案 B）；生成期旁写 `.glb` 要磁盘翻倍加回填 48k 网格（方案 C）。
  一个三角面 primitive：`POSITION` + 可选 `NORMAL` + u32 `indices`，坐标是 E3D 原生 mm、不换算；法向量数量
  与顶点数不等时整段省掉（错位的法向量把光照画歪且没有任何东西报错）；`wire_vertices` 不进——前端现有
  解析只读第一个 primitive。`geo_hash` 只放行 `[0-9A-Za-z_]{1,128}`，既是内容寻址键的字符集，也是唯一一道
  目录穿越的门。响应带 `Cache-Control: public, max-age=31536000, immutable` + `ETag`（内容寻址，永不变）；
  缺文件 404 `not_found`，编不成三角网格 422 `precondition`（数据的终局，不该让客户端当 500 重试）。
- **`tree/roots` / `tree/children` 的节点外包一格 `dbnum`，`search` 的 items 加 `noun`**：refno 前半段是 Ref0
  不是库号（`CONTEXT.md`「Ref0 库归属」），而浏览器端的几何缓存、可见性、选中全按库号分桶，库号只有
  服务端骨架答得出。`dbnum` 不加在 `EleTreeNode` 类型上（plant-ui 用 serde 解它，多余字段忽略即兼容），
  在 handler 的 JSON 层外包；答不出给 `null`，不是 0 号库。`noun` 只给当前这一页解——名称索引里没有类型，
  要它就得回记录读一次 `TYPE`，对全集解会把大库搜索拖成秒级；读不出给空串而不是 `null`（plant-ui 那边
  `noun: String` 带 `serde(default)`）。
- **`GET /api/v1/dbnums` 每行加 `ref0s`**（该库拥有的全部 Ref0，升序，`MdbMemoryStore::ref0s_of`）：
  客户端只拿得到 refno，此前得靠旧后端一份 `db_meta_info.json` 才拼得出 `ref0 → dbnum`。骨架没预热过的形态
  **整格不写**——空数组读起来是「这个库一个 Ref0 都没有」，那是另一句话。
- 顺带：`DirectTreeService` 两个同名 `dbnum_of` 合成一个（旧的走 `DirectStore` 定位器回 `Result<i32>`，新的走骨架
  回 `Option<u32>`；定位器本来就是拿骨架的 `locator_parts()` 建的，同一份数据两道门，留一道）。
  `direct_tree_tty_compare` 的 `--dbnum` 过滤跟着改。

单位几何对表（P0-5，`cargo test -- --nocapture` 的实测包围盒）：`1.mesh` 单位盒 `[-0.5,-0.5,-0.5]..[0.5,0.5,0.5]`、
`2.mesh` 单位圆柱 `[-0.5,-0.5,0]..[0.5,0.5,1]`（半径 0.5、高 1、Z 轴、底面在 z=0）、`3.mesh` 单位球
`[-0.5,-0.5,-0.5]..[0.5,0.5,0.5]`（半径 0.5、原点居中）——与 plant3d-web 本地造的三份
（`getUnitBoxGeometry` / `getUnitTubiGeometry` / `getUnitSphereGeometry`）半径、高度、轴向、原点**逐项一致**，
前端不必改参数。只有细分数不同（前端 16 段，仓内 `2.mesh` 124 面、`3.mesh` 1080 面），那是光滑度不是位置。

验证：`cargo test --lib -- web_service` **36 passed / 0 failed**（含新 `mesh_glb` 6 条：GLB 分块与对齐、
法向量数量不符整段省掉、退化网格拒收、`geo_hash` 门放行内容键且挡住 `../` 一类、仓内三份单位几何整份转出；
`handlers` 2 条：`ref0s` 按行 `dbnum` 写且没有 `dbnum` 的行不碰、`mesh_glb` 的两道门排在读盘之前）；
`cargo check --bins` 干净。**未 live**（本轮没起服务，`curl` 序列与 plant3d-web 对拍留给 P1–P3）。
`docs/specs/web-service-api.md` 同步 §4.7 的 `ref0s` 与新的 §4.10（直读模型树 + 搜索）/ §4.11（网格 GLB 口径）。

### 文档 · 模型增量「联动查询」核对与换源计划（只读分析 + ida-bridge 取证，未改代码）

- 问题：增量更新是否已处理模型增量的「联动查询」（反向引用查询）的更新处理。结论：**没有处理完**——
  老规划器有 `ref_rev` 反查；**direct + DESI 的窗口规划器（今天的默认）不做任何反向引用查询**；direct 下 CATA 没有
  数据批次，`build_cata_cascade_plan` 不可达，#4 洞原样；`ref_rev` 的唯一维护点在摄入链 `apply_one`，
  读透 G5「跳过摄入链」落地后无人维护。e3d-io `referrers_of` 与 `direct_store::referrers_of` 已就位、仓内零调用方。
- core 侧新取证 `docs/evidence/2026-09-06-dab-ref-table-write-path.md`（`core.dll` = `idalib-33060`）：引用表由
  dabacon 在 `db_put_reference`（opcode 140 → `sub_5AD4690` → `sub_5AB8D20` → kind-16 分流 `sub_5AB91B0`）**写时同步维护**
  ——删 `(旧目标, 自己)`、插 `(新目标, 自己)`（`sub_5AFDF10` op 0/1）、无表则 `sub_5AFF6D0` 建表注册根；仅描述符 kind 16 的属性
  有表（`HREF`/`TREF`/`CREF` 为 kind 4 无表）；`OWNER`（attId 10206636）硬编码排除；表根随会话页版本化，读侧零维护；
  `findElements` 的全部消费者（UDA / 集合 / 修订链接 / 伪属性插件 / `GATXRF`）无一在 GETWORK 刷新路上。
  顺带闭合 09-04 evidence §5 两条（根来自会话页 `+0x31C/+0x320`；叶无值、4 字键）。
- 计划 `docs/plans/2026-09-06-cascade-query-via-dab-ref-tables-plan.md`（待 Plannotator 评审）：照 core 从 dab 引用表读反向查询，
  P0 覆盖差表 → P1 e3d-io 收尾 → P2 CATA 文件差分触发 → P3 反查接线（CATA 侧闭包 + DESI 一跳 + 上卷）→ P4 direct DESI
  窗口规划器接同源 → P5 `ref_rev` 降级（ADR-063）；Q1–Q4 待拍板。取代 09-04 CATA 反向级联计划的 P1–P4 排期。
- 回填：09-04 inventory §3.4 进展一段；09-04 dab 引用表计划 P0 标闭合并指向本计划。共享记忆 mem-631。
- **计划已批准（Q1–Q4 按推荐项，共识 d-635），P0 当天做完**：`docs/evidence/2026-09-06-ref-rev-vs-file-table-coverage.md`
  ——783 个 E3D 3.1 库文件实测 75 张反向引用表（DESI 40 / CATA 19），`ref_rev` 资格 603 个名字；e3d-model 几何读的
  15 个引用属性里 12 个有表、3 个没有（`VXREF` 单元内、`HSRO`/`LSRO` 仅 stub 备选），DESI/CATA 侧没有一张表在 `ref_rev`
  资格之外。护栏 `vendor/e3d-model/tests/reference_reads_guard.rs`（新文件）：每个引用读归「有表 / 单元内 / 在案缺口」
  三档、间接读点逐处登记；`cargo test --test reference_reads_guard` 2 passed，负例红并点名；rustfmt 干净。
  Q4 修正一格：`POSREF` 是唯一有表且几何真读的设计→设计跨单元边，P4 要把它的引用者并进 `touched`。
  顺带：`XRPNTR` 在 ENGI 库有表（ADR-061 触发条件 2 的观测，不触发复议）。
- **P1 做完（e3d-io 提交 `4eeecf9`，只含 reference.rs 的 rustfmt、四条新测试与一条 CHANGELOG；别人的 WIP 未动）**：
  「是不是引用表」的判据写进测试——4 字键、无值、**引用者全是本库 refno**；`TYPEX` 与 7 张 Draft 表同形不同义；
  语料门改为按判据走全部具名表（543 库 / 913 张引用表 / 58 个名字 / 8 个 lookalike，无新面孔）。成本门落在语料里最大的
  引用表 `acp7320_0001 CATR`（744 925 条）：遍历 7 907 页、点查 4 页，门 ≤ 16 页且 ≤ 遍历 1/100。
  **★ `ams7351_0001` 一张引用表都没有**（只有 `NAME`/`TYPEX`）——原计划拿它做成本门的前提不成立；钉成
  `a_database_can_state_no_reference_table_at_all`：`NoSuchTable` 必须当独立结果，不能当「没人引用」。P3 开工前要先定
  这种目录库的退路（扫 SPCO 建内存表 / SCOM 层变动退化为依赖者全部过期并点名）。`cargo test --test reference_table_real`
  6 passed / 2 ignored（两条 ignored 的语料门本轮也跑绿）。用户随后拍板退路 **(ii)**（共识 d-665）。
- **P2 做完（触发源）**：新模块 `src/data_interface/cata_cascade_window.rs`——CATA 窗口 S→T 的变动目录件集合
  （e3d-io `diff_sessions` + 模板无关的记录头读 noun/owner，剔除 `relocated_only`）+ T 端反向引用表清单（判据同 P1）；
  `CatalogueWindowReport` 上回执 / `/health`，收不了的窗口是「未知」带 `error`，候选超 20 000 只报总数。挂点在
  `model_update_pending::reconcile_catalogue_versions`：对每个过期目录库收一窗（S = 根落款里最旧的目录会话，T = 文件最新），
  横幅逐库一行，`/health.startup.reconcile.catalogue_windows` 新栏——读透形态下文件事件也汇到这条路
  （`sweep_watch_dirs` 读透分支 → `reconcile_model_coverage_at_startup`），所以启动与文件事件共用；**没有**新任务种类、
  **没有**新水位、**仍不排工作**（P3 未接）。验收门 `ams5052_0001 184→185` = 1 个 Modified（noun `SDTE`）、0 增删、
  12 张反向表在。验证：模块 5 条测试在仓外同源 crate 上全绿；仓内 `cargo check --lib --tests` 只剩别人 WIP 的两处
  编译错误（`direct_tree.rs:249` 重复 `dbnum_of`、`handlers.rs:1014` `anyhow::Error: Serialize`），本改动零告警；
  仓内 `cargo test` 待那两处修好。**未 live**。

### PE 统计事件退役

- 废弃 `pe.update_dbnum_event`，启动幂等移除旧事件。初始化、增量、读透/镜像补写、目录补写和局部删除共用显式统计事务，按本批 PE 写前/写后的实际行数计算差额，与 PE 原子提交；重试不会重复计数。
- `count` 包含软删墓碑；`sesno`、`max_ref1` 保留已见最大值。首次写入旧 dbnum 时同事务重建统计并记录版本，常态批次不扫全库。整库清理同步删除统计版本。
- 镜像任务格式升级到 v2；旧版 element 待办任务保留并阻断回放，升级前须用旧版排空。部署与验收说明见 `docs/pe-statistics-explicit-maintenance.md`。

### 修复

- **rocksdb 空库首启不再被 SYS meta 的「文件轮次」焊死；空间收敛改为不重试、一次失败即死信并落错误日志。**
  实测（`output/rocksdb-7997-perf-20260906/run1-stall/`）：`store_mode=rocksdb`、空库、`--watch-dbnum 7997` 启动，
  第 1 个批次 amscom（COMM，dbnum 8190）成功后就永久停摆——冻结点对**任何**库类型都落一条 `spatial_reconcile`
  意图行（`db_type` 还写死 `'DESI'`），批次完成后 `publish_completed_file_round` 无条件发布，收敛时
  `E3dModelService::from_frozen_sources` 找不到 8190 的 DESI pin（`current MDB has no DESI dbnum 8190`），
  行被标 `failed`；`reconcile_spatial_pending_locked` 的选行谓词 `status IN ['pending','failed']` 没有上限，
  每轮同一个确定性错误，出队门 `dispatch_allowed=false`，余下 11 个 meta 批次挂死，MDB 名单建不起来，
  DESI 7997 连入队资格都没有。
  - `batch_worker.rs`：文件轮次的意图行（冻结点）与 `publish_completed_file_round` 只对 `db_type == DESI`
    ——房间输入基线、空间发布、房间任务本来就只认设计库。
  - `side_effect_pending.rs`：空间收敛**不重试**（用户 2026-09-06 拍板：出错只进错误日志、方便排查）。
    选行只取 `pending`；逐行收敛，一行失败 → 原因落 `logs/batch-failures-*.jsonl`
    （新 `event = side_effect_dead_letter`，带 id / dbnum / db_type / end_sesno / commit_token / 意图行数与
    解封步骤）→ 行标 `dead`（终态，任何选行谓词都看不见），出队门与房间轮照常放行；同批其它行不受牵连。
    `failed`（老版本的重试中间态）一并视为死信。`/health.spatial_reconcile` 新增 `dead_letters`，
    `stalled` 改为「有死信」；`web/ops.html` 停摆卡文案同步。
  - 验证：`cargo test --lib -- side_effect_pending batch_failure_log completed_file_round_publishes_spatial_before_rooms
    a_stale_spatial_tree_also_holds_back_the_room_round` 32 项通过；live 验证见同目录 `REPORT.md`。

### 性能

- **模型积压消化（`model_update_pending`）去掉两处 O(n) 往返：收口按 record id、页尾释放批量化。**
  rocksdb 7997 首启实测（`output/rocksdb-7997-perf-20260906/REPORT.md` §3）：18,675 个生成根按 0.73 根/s 消化、
  预计 7 h，而几何 16 个 worker 空转（服务 CPU 0.04 核）、SurrealDB 一核跑满。两个单项：
  ① `render_complete_regen_revision` / `render_delete_revision` / `render_mark_failed_revision` / 房间收口用
  `WHERE action = … AND target_refno = … AND revision = …` 寻址，表上没有索引，事务内每条全表扫 18k 行 140 ms，
  16 根一笔 4.5–5.8 s（`surreal_write_p50`）；② `release_unsettled_claims_on` 对整 cohort 认领的 18k 行**逐条**发单行
  UPDATE，页因 EpochChanged 每 100–270 根结束一次，每页释放 3 分钟。
  - `model_update_pending.rs`：新 `settle_target` = `<record_id_of(action, target)> WHERE <原谓词>`——按 id 取行（O(1)），
    谓词留作 revision 校验，四处收口/标失败统一走它（历史 `{dbnum}_` 前缀旧行由 `clear_stale_at_process_start`
    每次启动清掉，不再需要谓词兜底）；`release_unsettled_claims_on` 改为 `render_release_unsettled_claims` 按 500 条一个
    请求（逐行 status / revision / lease_owner 谓词原样保留）。
  - 同一隔离目录复跑 7997（run4-perf）：`surreal_write_p50` **5.2 s → 15 ms**，页内 16 根 **7.5 s → 3.0 s**，页间间隙
    **180–236 s → <15 s**，消化速率 **0.73 → 3.35 根/s（4.6×）**，18,675 根 ETA **7 h → 1.4 h**。SurrealDB 仍是一核跑满
    （服务 0.08 核）：剩下的大头是每页把整 cohort 17k 行 `UPDATE … RETURN AFTER` 重新认领、无索引表上的
    `has_pending_work` / `count_pending_work` / 认领 ORDER BY 扫描（读 p50 316 ms）、以及每根一笔 `persist_root`。
  - 测试：`cargo test --lib -- model_update_pending` 136 通过（改写 `settlement_addresses_the_row_by_its_id_and_guards_it_by_its_fields`、
    `batch_settlement_is_revision_safe_and_bounded`，新增 `releasing_unsettled_claims_is_batched_per_round_trip_and_keeps_the_row_guards`）；
    全量 `cargo test --lib` 1592 通过 / 1 失败（`staging::lifecycle::production_windows_have_independent_sessions`，
    Connection uninitialised，需要库连接、与本改动无关）。

### 变更

- **Plant UI 增量监控增加可收藏的逐库入口。** `web/ops.html?dbnum=<N>` 现在把 dbnum 作为整页作用域：
  顶部保留全部已登记库的入口并标出异常/滞后，进入单库后集中显示数据水位、模型水位、根追赶情况、
  当前队列、最近任务与落盘错误；队列、连败、窗口阻断、任务、错误日志、实时事件和双水位表同步按该库过滤。
  错误日志请求也下推 `dbnum` 到服务端，避免前端 40 条窗口把目标库旧记录截掉；“全部库”可返回总览。
  AMS8000 RocksDB 现场验证：页面列出 41 个入口，`?dbnum=8000` 显示数据 `543/543`、模型 `543 · 一致`、
  追赶根 0、放弃根 0、懒生成 1，水位表只有 8000，历史错误 6 条均属 8000；另进入 7006 后各面板切换为
  7006 且错误为 0，证明作用域不是只改标题。同步修复窗口面板读取新旧字段名断裂的问题。

## 2026-09-05

### 变更

- **双水位统一判决完成，并用 AMS8000 RocksDB 捕获真实的异步追赶窗口。**
  新增 `dbnum_consistency` 深模块，把 `applied_sesno`、`model_sesno` 与实时生成根普查合成
  `in_sync / lagging / not_judged` 三态；`/dbnums`、`/health` 与 `web/ops.html` 只消费这一份结论。
  健康聚合只判监听范围内的 DESI 库，DICT/SYST/CATA 不再制造模型假滞后；模型滞后仍不进入
  `blocking_conditions`，数据和模型继续异步推进。AMS8000 的 COUP `ORIL` 实改把源会话从 541 推到 542，
  100ms 采样依次捕获 `541/541 in_sync`、`542/541 lagging`、`542/542 in_sync`；Plant UI 可见
  `542 / 542` 与「一致」。独立 E3D RVM 对拍中，TUBE 世界 AABB 最大误差 0.00001 mm，带负体的
  COUP（`has_neg=true`）误差 0 mm（容差 0.01 mm）。全量库测试 1580 项中先由浏览器抓到并修复
  运维页残留变量/DOM 契约回归，最终验证见 C2 计划。后续补齐预览现状判决、单库任务终态的
  `model_verdict` / `model_caught_up_at`，以及启动水位对账后的逐库 `describe()` 日志；这些读数失败都只留伴随告警，
  不改变批次成败、不让数据事务等待模型。

- **模型水位加第二触发点：零 regen 的窗口在凭证前移之后自己盖章，不再等下次启动对账（ADR-060 D5 修订）。**
  起因是用户补述「增量更新时属性数据已写入 Surreal，模型水位却没跟进」。核查（共享记忆 mem-583）：`model_sesno`
  唯一热路径盖章点在 `run_regen_group` 收口之后，由 `settled_window_cohorts` 只从 **`RegenRoot`** 行里取 cohort；
  而 DESI 窗口零 touched 根（改的属性不碰几何）或只删根时，`WindowRootPlan::work_items` 里没有 `RegenRoot`，
  模型排空里根本不会出现它的 cohort——`applied_sesno = T`、安定下限 = `T`、`model_sesno = T_prev` 在本进程内一直
  不等，要到下次启动才「热路径漏记，已补记」。ADR-060「代价」一节原先没列这一格，而它是最日常的一格。
  - **`finalize_attempt_on`**（`model_update_pending.rs`）：尾事务 → 凭证前移之后，若计划里没有 `RegenRoot`，
    调 `stamp_model_watermark_on`（`advance_model_watermark` 的带库版本，出声措辞同一份）。判据仍只有
    `ModelWatermarkProbe::supported` 一处；三道闸（拦路根 / `min(下限, applied)` 夹逼 / `WHERE applied >= 候选`）
    原样生效，提前调最多空转一次、不可能多盖；成败不上抛，数据链不因它等待（M7 不破）。
  - **`reconcile_startup_window`**（`window_root_plan.rs`）：`enqueued == 0` 的库在凭证前移之后同样盖一次——
    启动对账排在开窗之前、这一轮已经跑过，不盖就要等**再下一次**启动（09-05 审核 N2 / mem-579）。
  - **ADR-060**：D5 标题与正文改为「判据唯一、触发点两处」，列出残格（只有 `DeleteCleanup` 的窗口仍由启动对账补）；
    M1 / 约束 / 代价三处同步。
  - **护栏四条**：源码顺序钉 `finalize_attempt_on` 的「尾事务 → 凭证前移 → 盖章」与 `RegenRoot` 分流、
    钉 `reconcile_startup_window` 的「前移 → 盖章」与 `enqueued == 0` 分流；`mem://` 上一窗 20→26 零 regen
    收口即 `(26, 时刻)`，有 `RegenRoot` 的同一窗收口处不盖、留给 cohort。
  - **验证**：`cargo test --lib -- a_zero_regen_window startup direct_read_mode initialization model_coverage reconcile`
    **77 passed / 0 failed / 3 ignored**；模型水位与 finalize 相关 15 条全绿；`rustfmt --check` 两文件通过。
    **未 live**。
  - **没做**：P4 露出（`model_sesno` 在 `/dbnums` / `handlers.rs` / `web/` 仍零命中，`/dbnums` 仍露
    `model_settled_floor`，`model_lazy_roots` 从未被写入）；读透形态下 `applied_sesno` 不动使 M2 封顶永远写不出
    `model_sesno`，两枚水位在那个形态下不可判等——口径待拍。

- **数据面多了一种形态：读透（`data_face = "read-through"`，默认），`pe` / `ATT_*` 行随模型生成写透进库（G0–G3）。**
  用户拍板（计划 `docs/plans/2026-09-05-kv-mem-zero-ingest-read-through-plan.md`，Q1–Q5 全按推荐项，Plannotator `approved`）：
  A′ 把「库」做成了开箱即用，「数据」那一半还没有——默认形态启动仍跑整库 DESI 基线，不跑的话 `pe` 0 行，
  而模型行 `inst_relate.in = pe:⟨…⟩` 是**悬空引用**（`model_db_adapter.rs:635`），plant-ui 的隐式直管取
  `in.owner` 得 NONE、一根 BRAN 整批解不出。读透形态下 `pe` 行是**缓存**：缺行 = 还没读到，不 = 不存在。
  - **G0 `data_face` 三件套**（`src/options.rs`）：`DataFace { Ingest, ReadThrough }` / `data_face()` / `AIOS_DATA_FACE` /
    `data_face_notice()`，与 `store_mode` **正交**不派生（`external + read-through` 是合法的落盘读透缓存）。
    默认翻转的到达面与 09-04 P0 同款：13 份现场 / 夹具配置逐份补 `data_face = "ingest"`（仓根 `DbOption.toml` 不写），
    护栏改名 `every_db_option_fixture_declares_its_store_mode_and_data_face` 两键都守（按负例验过：漏一行即红并点名），
    五处「先起库再起服务」补 `AIOS_DATA_FACE=ingest`。`/health.data_face`、`/dbnums.data_face`、横幅一行。
    顺手把此前有定义没人打的 `store_mode_notice()` 接到启动日志上。
  - **G1 e3d-model 报访问名单**：`GenerationOutcome.visited: Vec<RefNo>`——DFS 本来就逐个 pop 到子树每个元素，
    「子树是什么」只有这一份答案，另起一次枚举就是第二份。`GeneratedSnapshot` / `BakedSnapshot` 原样带过来。
  - **G2 `src/data_interface/read_through.rs`**：读透的唯一入口 `write_through`。**行形状只有一份**（宪法 II）：
    读取走 `e3d_collector::read_ele_data`、渲染走 `ElementRows::render`、动词 `UPSERT`（增量路 `Modified` 口径，幂等刷新）；
    `pe.sesno` 仍是元素 last-touch 会话（`session_chain` → `PageSessions` + `IndexCursor::seek` 反查，与 `stream_baseline` 同一口径），
    **不许拿生成时点顶替**——那会让同一张表里出现两种 `sesno` 含义；`RefNo::dbno()` 不等于本库的 refno 一律不写
    （ADR-058 S3：CATA 永不进 `pe`）。验收是**行级对拍** `tests/read_through_rows_parity.rs`（issue-019 真实 db8000 归档）：
    64 个样本与基线逐字段相同（`sesno` 在内）、祖先链到 WORL 收尾、真 `mem://` 上别库不写 / 索引外点名 / 重写幂等，4 passed。
  - **G3 挂钩在编排层，不在生成服务**：`e3d_model_service.rs` 有源码钉「模型面不认识库」，所以生成服务只在
    `E3dPersistReport` 上多报 `source_sesno` 与 `visited_by_root`；`model_refresh::generate_roots_report` 在读透形态下
    用 `open_pinned_set` 钉同一版会话，每根 `visited ∪ 祖先链` 写透，**排在该根 `persist_root` 之前**
    （护栏 `write_through_precedes_root_publication`）。**缓存不是前置**：源打不开 / 上溯失败 / 落库失败三条路只记
    `read_through_degraded` 日志，模型照常发布（护栏 `write_through_failure_never_fails_the_root`，钉的是那段代码里没有 `?`）。
  - **验证**：全量 `cargo test --lib` **1530 passed / 0 failed / 104 ignored**；`tests/read_through_rows_parity` 4 passed；
    `vendor/e3d-model` `cargo check --all-targets` 通过；`rustfmt --check` 对本轮文件通过。**未 live**：没起过服务，
    `POST /model/ensure` → `pe` 行出现这一格等 G6 的 `Verify-OutOfBox.ps1` 扩展一起验。
  - **没做**：G4 `POST /api/v1/element/ensure`、G5（读透形态跳过摄入链 / 文件事件 = 退化窗口 / `initialization_required` 恒 false）、
    G6、plant-ui U0–U5。**G5 之前默认形态启动仍会跑 DESI 基线**——基线 `INSERT IGNORE` 与写透 `UPSERT` 落同一形状的行，共存无冲突。

## 2026-09-04

### 变更

- **持久层形态从一个 bool 升成三值 `store_mode`，并给默认翻转先铺好护栏（P0；行为一个字没改）。**
  起因是用户拍板走 A′：「让 gen-model 自己拉起 memory 后端的 surreal 子进程，plant-ui 零改动」
  （新增 `docs/plans/2026-09-04-spawned-memory-store-out-of-box.md`；plant-ui 侧的对照分析在
  `plant-ui/docs/plans/2026-09-04-kv-mem-out-of-box-adaptation.md`）。A′ 的意义在于
  **内存形态从此有真端口**——`in_memory_db` 那条 `mem://` 路把库关在进程里，plant-ui 的启动序列
  第一步就是连 `v_ip:v_port`，于是**外壳压根起不来**，`/sql` 探针 / `rvm_verify` /
  `Capture-*Evidence.ps1` 也全部够不着（`options.rs` 自己的注释早就写了「要让别的进程读库就别开」）。
  - **新增 `StoreMode`**（`external` / `spawned-mem` / `embedded-mem`）与 `store_mode()`、
    `AIOS_STORE_MODE`、`store_mode_notice()`。`in_memory_db` 只表达得了 `embedded-mem` 那一档，
    保留为兼容回退（没写 `store_mode` 时它说了算，行为与今天逐字相同）；两个键打架时以
    `store_mode` 为准**并出声**——旧配置里那句 `in_memory_db = true` 会让读配置的人得出相反结论。
    认不出的值按未设置处理、同样出声（`AIOS_DATA_READ_MODE` 同款纪律：静默换掉持久层介质是
    本仓最贵的那类沉默，一个 `spawn-mem` 的错字会让部署以为自己在内存形态、实际连着落盘库）。
  - **默认值本轮不翻**，护栏 `introducing_store_mode_does_not_flip_the_default_yet` 钉住这一点。
    翻转（计划 P3）会让**每一份没写这一项的配置静默换介质**：最难看的是 l3 套件——它自己在 8048 上
    起了一台 rocksdb surreal，服务却会去 spawn 自己那台，症状是「测试莫名其妙全查不到数据」而不是
    一条报错。
  - **13 份夹具 / 现场配置逐份补上 `store_mode = "external"`**（`db_options/` 8 份 + 仓根
    `DbOption-{ams,aba,KMX,zsy}.toml` 与 `DbOption_text.toml`；仓根 `DbOption.toml` 不写——
    它就是开箱即用形态本身，默认值正是为它准备的），并加护栏
    `every_db_option_fixture_declares_its_store_mode` 扫这两个目录、**新加的夹具漏写就红**
    （已按负例验过：注释掉一行即红，且错误里点名是哪一份）。默认值翻转这种事只在翻的那一天
    有人记得，护栏是唯一能让半年后新加的夹具也不踩的东西。
  - **验证**：`cargo test --lib -- options::tests` **55 passed / 0 failed**（新增 6 条）；
    `cargo check --lib --tests` 通过；`rustfmt --check` 通过。**没有任何行为改动**——
    `store_mode()` 目前还没有调用方，接线是计划里的 P1–P3。

- **新增 `src/store_process.rs`：自带 memory 后端 SurrealDB 子进程（同一计划的 P1，同样还没有调用方）。**
  `SpawnedStore::start` 做五件事：按 `AIOS_SURREAL_EXE` → 可执行文件旁的 `bin/` → 仓根 `bin/` 找二进制
  （**刻意不回退 PATH**）、跑一次 `surreal version` 过版本闸、`start --user … --pass … --bind ip:port memory`、
  轮询到端口能连上、`Drop` 与 `shutdown_spawned_store()` 两条路收尸（Windows 走 `taskkill /T /F`，
  与 `bin/l3_suite` 那份 `kill_tree` 同一写法）。查找顺序与版本闸**逐条复刻
  `scripts/Start-Surreal8009.ps1`**（计划 P5 要把「两份判据不许分叉」钉成护栏）。
  - **版本闸分两档，不是一档**：大版本不是 2 → **硬拒**，错误里点名那两个必现的坑（要求不可逆迁移 /
    `Server sent no subprotocol`）——人看到后者只会去查网络；`Cargo.lock` 的 fork rev 对不上 → **只警告**，
    因为那只是*可能*出怪问题，做成硬拒会让任何一次 fork 更新先卡住所有人。拿不到 `Cargo.lock`
    （部署形态下是常态）不编造警告。
  - **`v_ip` 是主机名就早拒**：`--bind` 只吃 socket 地址，`localhost` 透传会让 surreal 在参数解析阶段
    就退出、日志里只剩一行用法提示，所以 `localhost` 映射成 `127.0.0.1`；`localhost` 之外的主机名一律
    拒绝并给出路（改 IP 或改 `store_mode = "external"`）——它可能压根不在本机，而「在本机拉起一个绑到
    别人机器上的进程」做不到。绑 `0.0.0.0` 时探回环。回执与 `/health` 用的 `endpoint` **保持配置里的字面
    形态**，因为跨部署互踩探测按那一栏比对（`handlers.rs`），改写成解析后的 IP 会让同一台库在两处长得不一样。
  - **子进程输出落 `logs/surreal-<port>.log`，不继承控制台**：surreal 自己的启动横幅会插进 gen-model 的
    启动日志中间，而那一段正是出问题时第一批要读的东西。等端口超时的那条错误**带日志尾巴**（20 行），
    「一个字都没输出」也单独成话——不带尾巴等于让人自己猜。起不来的那个也会被收掉，
    免得一边占着端口一边没人认领。
  - **失败一律响亮，不回落别的介质**：静默回落等于「用户以为自己在内存形态，实际连上了昨天那台落盘库」，
    而那正是 `store_mode` 要消灭的沉默。
  - **验证**：那条 `#[ignore]` 的**真起真杀**手动跑过并通过（`bin/surreal.exe` 未纳入 git，CI 上没有）：
    `pid=3468 监听 127.0.0.1:18009`、二进制自报 `2.1.4+20250317.45013fc9`（rev 与锁一致、无告警）、
    `shutdown()` 之后端口确实放开、事后 `Win32_Process` 里无残留。**未接进 `run_app`**（P3）。

- **端口被占时的四格裁决与那枚署名（同一计划的 P2，依旧没有调用方）。** `spawned-mem` 一旦是默认值，
  「端口上已经有东西」就从边角变成日常：服务在跑的时候，`direct_model_persist` / `db_window_repair` /
  python `full_init` 这些探针**正常局面**就是撞上服务手上那个库。`judge_port()` 给四格——
  **空闲**起一个 / **自家还活着**附着 / **自家孤儿**收掉重起 / **别人的库或分不清**拒绝启动。
  - **判据是写在库里的一枚署名，不是磁盘上的 owner 文件。** 从外面看，一台 memory 后端与一台
    rocksdb 后端长得一模一样；而磁盘文件与端口上那台服务没有任何绑定关系——一份陈旧的 owner 文件
    配上一个被复用的 pid，就足以让我们**把别人的库当成自家孤儿杀掉**。写进库里则是自证：
    能从这个端口上读出那一行，就说明听在这个端口上的正是我们建的那个库。署名住在**固定**的
    `gen_model/store` ns/db（与工程无关——配错工程的那个 gen-model 也要读得到它，那正是要拦下的局面
    之一），带 `owner_pid` / `child_pid` / `nonce` / `project` / `namespace` / `heartbeat_ms`。
  - **活没活看心跳，不看 pid。** 属主每 10 s 敲一次，60 s 没动才算停（六倍余量：GC、换页、机器很忙
    都不该让活着的服务被判成孤儿，因为判成孤儿要**杀进程**）。pid 只用来在判死之后确认「它确实不在了」，
    并在收孤儿时作为要杀的那个。
  - **分不清一律拒，绝不往「杀掉它」那一边猜。** 心跳停了但属主 pid 还在（卡住？正在退出？）、
    或者压根查不出 pid 在不在——两格都是 `Refuse` 并把原因说全。杀错的代价是把一个正在跑的服务的库
    端掉；多等一次的代价只是人来看一眼。读不出署名（连不上 / 登录不了 / 查不动）与读出来没有那一行
    分开报，但处置相同：都不许动它，出路给两条（换端口，或改 `store_mode = "external"`）。
  - 收孤儿之后**等端口真的放开**再起：不等的话紧接着那次 `--bind` 会撞 `address already in use`，
    而那条错误只会落在子进程自己的日志里。
  - **验证**：`cargo test --lib -- store_process::` **29 passed / 0 failed / 2 ignored**；两条 `#[ignore]`
    手动跑过、均通过。第二条是**署名走真库**（`a_real_store_answers_with_the_signature_we_stamped`）——
    这条不能只靠纯函数：语句形状或字段名对不上时的表现是「读不出署名 → 拒绝启动」，看上去像端口被别人
    占了，是最难查的那一类。它验了盖之前 `Absent`、盖之后原样读回、判据落 `Attach`、重复盖不留第二行。
    `cargo check --lib`（含 test profile）与 `rustfmt --check --edition 2024` 通过；两轮 `#[ignore]` 跑完
    `Win32_Process` 里无本轮残留。

- **接进 `run_app`，默认翻成 `spawned-mem`（同一计划的 P3）——这一条起，行为真的变了。**
  `run_app` 的 ws 连接从二分支换成按 `store_mode()` 的三分支：`embedded-mem` 走原来的 `mem://`；
  `spawned-mem` 先 `ensure_own_store()` 把库拉起来（或附着 / 收孤儿 / 拒绝）再连；`external` 直连配置里
  那台。**`spawned-mem` 与 `external` 连的是同一句** `init_surreal()`——两档的分别只在「那台服务是谁起的」，
  到连接这一步它已经在监听了。
  - **默认值的口径比「默认 spawned-mem」细一格**：两个键都没写才是 `spawned-mem`；写了
    `in_memory_db = false` 而没写 `store_mode` 的，**视为显式要过外置库**，回 `external`——那句 `false`
    是人写下的意图，不该被新默认盖掉，而它正是升级时最容易被静默改掉的一格。
  - **实例锁改由 `store_mode` 把门（Q4）**：只有 `embedded-mem` 跳过。`spawned-mem` 的库有真端口、
    别人连得上，「两个实例不共享库」这个跳过理由在那一档不成立——两个 gen-model 配同一份
    `DbOption.toml` 就是冲着同一个端口去的。护栏 `memory_mode_needs_no_single_instance_lock`
    翻成 `only_the_embedded_store_needs_no_single_instance_lock`，三档各钉一句。
  - **装了 `Ctrl+C` 钩子**：`Drop` 兜正常退出与 panic，兜不住 `Ctrl+C`——那一路 tokio 直接结束进程、
    析构不跑。少这一条不是「偶尔漏一个」，是**每一次 Ctrl+C 都漏一个占着端口的孤儿**。
  - 启动横幅按三档分别说话；`embedded-mem` 那句的出路从「用部署脚本的 -InMemory」改成
    「改 `store_mode = "spawned-mem"`」——那个脚本要做的事现在进程自己会做了。
  - **顺带修一条护栏的锚点**：`startup_cleanup_precedes_spatial_watcher_worker_and_model_startup`
    原先钉 `match init_surreal().await` 这个字面量，而连库已经不只一条路。锚点换成介质分派那一句
    （`match crate::options::store_mode()`）——这条测试守的是**顺序**，不是某一条连接语句长什么样。
  - **验证**：全量 `cargo test --lib` **1508 passed / 4 failed / 103 ignored**。四条失败**全部与本改动无关**，
    且都验过：三条 `increment_manager::tests::*scope*` 是工作树里 `DbOption.toml` 今天启用了
    `watch_dbnums = [7998, 8000]`（另一会话的 S5 live 验收窗口）——用一份没有该键的配置
    （`DB_OPTION_FILE=db_options/DbOption-e2e-8009`）复跑，同一批 **44 passed / 0 failed**；
    第四条 `the_embedded_panel_is_real_and_a_disk_copy_still_wins` 是 `web/ops.html` 改动中的已知红
    （09-04 审核 §6 已记）。新增 `#[ignore]` 的**整条路径**用例
    `ensure_spawns_once_and_attaches_the_second_time` 手动跑过并通过：第一次起（pid=37320 @ 18011）、
    第二次附着、换一份工程身份被拒。`rustfmt --check` 通过。
  - **没做**：探针与 python `full_init` 那几个自己 `init_surreal()` 的入口还没收进统一的
    `ensure_connected()`（Q1 的后半）——服务在跑时它们照旧连得上（附着是自然发生的），
    服务没起时与今天一样连不上，不是回归。

- **P0 补齐两半 + `/health` 认得三种介质（P4）。**
  - **凡是「先起 surreal 再起服务」的地方，服务那一段显式带 `AIOS_STORE_MODE=external`**：
    `bin/l3_suite/fixture.rs`（它自己在 8048 上起了一台 rocksdb surreal，服务却会按新默认自己再拉一台
    memory 库——症状是整套断言对着空库跑、「莫名其妙全查不到数据」而不是一条报错）、
    `Run-LiveBatch.ps1`（尤其要紧：一部分 live 用例**不读 `DB_OPTION_FILE`**，只能靠环境变量收口）、
    `Run-Issue7E2E.ps1`、`Run-RoomE3DE2E.ps1`、`Start-AiosDatabaseManual.ps1`。这些脚本指的都是
    `db_options/*` 那几份已经写了 `store_mode = "external"` 的配置，环境变量是**第二道**——
    给「换了配置忘了改」留的。CI（`windows-tests.yml`）只跑 `cargo test`、不起服务，无需改。
  - **`Start-Surreal8009.ps1` 改口**：`.SYNOPSIS` 加一句「**只有 `store_mode = "external"` 的部署才需要它**」，
    `.DESCRIPTION` 开头补一段说清默认形态下**不要跑它**，以及「先手工跑了它又忘了把配置改成 external，
    服务会明确报错而不是悄悄连上你这台库」——那正是 P2 的署名判据买到的东西。
  - **`/health.sul_db` 加 `medium` / `durable` 两栏，`endpoint` 在 `spawned-mem` 下报真地址。**
    这一格不改就是个坑：那台库是本进程拉起来的，但**端口是真的**，报 `embedded:mem` 会让
    `aios_db.full_init` 的跨部署互踩探测**放行两个抢同一个端口的实例**。只有 `embedded-mem`
    才回那个占位串。顶层 `in_memory_db` 保留给老客户端，语义收窄成「持久层不落盘」（`!durable`）——
    它**不回答「连不连得上」**，那个问题归 `endpoint` / `medium`。
    新护栏 `only_the_embedded_store_reports_a_placeholder_endpoint` 钉住分叉判据是 `has_endpoint()`，
    并钉 `health` 全文不再直接读退役的两态开关；既有的 `sul_db` 六键承诺扩成八键。
  - **验证**：全量 `cargo test --lib` **1511 passed / 4 failed / 104 ignored**，四条失败与上一条同源
    （三条 `watch_dbnums`、一条 `ops.html`），已分别验过与本改动无关。`rustfmt --check` 通过。

- **P4 收尾：`PersistMedium` 三值 + 连库收成唯一入口，旁路入口一律「只附着、不拉起」。**
  - **`PersistMedium` 从两值改三值**（`RocksDb` / `SpawnedMem` / `EmbeddedMem`），`PersistTarget::process()`
    改按 `store_mode()` 逐档判。`is_durable()` 的答案不变（只有 `RocksDb` 为真），但回执不再把
    `spawned-mem` 说成「InMemory」——那个词会让人以为是进程内嵌、外面读不到，而它恰恰读得到。
    新增 `is_reachable()` 把这两件事分开：**`spawned-mem` 不 durable，但别的进程读得到**。
  - **新增 `ensure_store_connected(db_option, startup)`：连持久层的唯一入口。** `run_app` 里那段三分支
    搬进来，旁路入口共用同一条。配套 `attach_store_connected()` 是给旁路入口的便捷档。
  - **新增 `StoreStartup::{Spawn, AttachOnly}`——这一格是本轮真正要紧的东西。**
    只有服务主进程给 `Spawn`。探针（`direct_model_persist` / `db_window_repair` / `world_refno_probe`）、
    MCP `query_service` 的独立路径、python `full_init` 与 `connect` 一律 `AttachOnly`：
    **端口空着时响亮地说「服务没起」，而不是自己拉一台空库然后在上面作业**——后者的失败一声不响，
    现象是「查了半天什么都没有」「纠正跑完什么都没变」「取证落完随进程消失」。
    `AttachOnly` 撞上自家孤儿同样拒绝：探针没有资格杀别人留下的进程。
  - **护栏锚点再修一次**：`startup_cleanup_precedes_spatial_watcher_worker_and_model_startup` 改钉
    `ensure_store_connected(&db_option`——分派已经从 `run_app` 搬走了。
  - **验证**：全量 `cargo test --lib` **1511 passed / 4 failed / 104 ignored**（同上四条，均已验与本改动无关）；
    三条 `#[ignore]` 真库用例全部手动跑过通过，其中整条路径那条已扩到验 `AttachOnly`：
    端口空着 → 报「没有自带存储在跑」；起来之后 `Spawn` 与 `AttachOnly` **两档都附着**。
    `cargo check --lib`、三个 bin 各自 `cargo check --bin`、`cargo check -p aios-py` 全通过；
    `rustfmt --check` 通过（`python/src/lib.rs` 有一处 261 行的既有格式偏差，不是本轮改的，没动它）。
  - **没做**：P5（PS1 与 Rust 两份版本闸判据不许分叉的护栏）。

- **P6 实测：开箱即用在真机上跑通了，并且当场撞出一个真缺陷（已修）。**
  用真的 `aios-database.exe`（release）在一份换了端口、**故意不写 `store_mode`** 的配置上跑
  （测的就是新默认），`AIOS_STARTUP_AUTORUN=0` = 不预解析，`watch_dbnums=[7997]`。
  - **拒绝那一格先验的**：对着被别人占着的 8009 起 → 进程 `exit=1`，stderr 就是那句
    「端口上是一台**没有署名**的 SurrealDB…拒绝启动」。这一条是**真进程真端口**，不是单测。
  - **开箱即用成立**：换到空闲端口，**启动总耗时 6.47 s**，`/health.sul_db` =
    `{endpoint: "localhost:18022", medium: "spawned-mem", durable: false, connected: true}`，
    子进程 `surreal.exe … --bind 127.0.0.1:18022 memory` 在跑，横幅照三档说话。
    启动日志明说「**gen_root 尚无凭证记录，本次启动不排任何模型工作**——从未生成过的根等按需
    `ensure`（ADR-058 D6）」，`/api/v1/tree/roots` 立刻回 37 个 SITE（`source: "direct"`，走文件）。
  - **按需生成成立**：`POST /api/v1/model/ensure refno=24381/34109`（SITE `/1RX-ARCHHB`）
    **89.3 s** 回 `status: Generated`、`model_available: true`、416 个实例 / 560 个生成根。
    直连那台 kv-mem 查回来：`inst_relate` **416 行**、`gen_root` 560 行、
    **416/416 平表三件齐活**（plant-ui 的快路一行都不用退回 slim）、**`pe` 0 行**——
    「不预解析、按需生成、落进 kv-mem」这条链在真机上是通的。
  - **⚠️ 由此坐实一条 plant-ui 侧的边界**：`pe` 是 0 行，而 plant-ui 查隐式直管那条子查询取
    `in.owner`，没有 `pe` 行就是 `NULL`，`GeomInstQuery.owner` **不是 `Option`** → 一根 BRAN 的
    直管会整批解不出来。非管件走平表快路（`owner` 从 `anc[1]` 还原，**零解引用**）不受影响。
    本轮实测的 SITE 全是 PANE，没有触发；这一条按码坐实、未实测。
  - **撞出的缺陷 —— 心跳新不等于属主还活着（已修）**：强杀服务后 60 s 内重启，心跳看着还新、
    写它的人却已经没了，于是新进程判「附着」。**附着的进程不认领这台库、也不接着敲心跳**，
    它就变成一台没有主人的库；再过 60 s 会被下一个启动的进程当孤儿杀掉——**而那时它正被人用着**。
    实测原样复现（新服务 pid 54872 附着到属主已死的 pid 21716 那台库，416 行旧数据还在，
    署名再没人更新）。`judge_port` 改成：心跳新**且**属主明确不在 = 没有歧义（写心跳的那个已经死了），
    照孤儿收；查不出属主在不在时仍然附着（新鲜心跳是够强的证据，而这一格的反面是杀进程）。
    新护栏 `a_fresh_heartbeat_from_a_dead_owner_is_still_an_orphan`。**修完重跑同一场景，
    真机日志变成**「回收孤儿存储进程：自家存储的属主 pid=41996 已经不在了（心跳 8 秒前还在写——
    多半是被强杀，析构没跑），杀掉 pid=44388」，随后起了一台干净的新库。
  - **新增 `scripts/Verify-OutOfBox.ps1`**：把上面能自动判的那半自动化（起服务 / 等就绪 /
    打 `sul_db` 三栏 / 树根数 / 收尾查孤儿），剩下一半（点树、点眼睛、看三维）列成清单给人。
  - **验证**：全量 `cargo test --lib` **1513 passed / 4 failed / 104 ignored**（四条同前，均已验与
    本改动无关）；`rustfmt --check` 通过；跑完本机无本轮残留进程。
    **仍未做**：plant-ui 端到端（本机没有 plant-ui 的构建产物，`target/release` 里一个 exe 都没有）。

- **模型增量「联动更新」十一类落成一张表，并当天收掉两条（文档 / ADR，未动源码）。**
  起因是「现在的联动更新情况是否梳理整理完毕了」这个问题在仓内**答不出来**——十一类联动边各自散在
  4–5 份文档里，没有一处能回答「漏了哪一类」。新增
  `docs/2026-09-04_model-cascade-update-inventory.md`：§1 分类表（触发边 / 承担者 / 状态 / 依据），
  §2 本轮 ida-bridge 新取证，§3 待补项，§4 不许当已证。
  - **`CONTEXT.md` 加两条词条**：**联动更新 (Cascade Update)**（指定那张表为唯一索引——新增或改动任何
    一条联动边先改表）与**图形行为 (Graphics Behaviour)**（0/1/2/3 四档语义）。
  - **ADR-061：不复刻 core 的 `XRPNTR` 正向依赖扩散。** 它此前是清单上**唯一连决策都没有**的边。
    四条理由：那条递归在 `DB_UserChanges`（会话内交互编辑账本），而本轮实读的 GETWORK 全程
    **没有任何依赖递归**，照抄是与 core 分叉；`XRPNTR` 的 DCHC = 0；1878-noun 快照里无属主；
    反向那一侧 `ref_rev` 本就通用覆盖（缺的只是 core 那条正向边）。带四条复议触发条件。
  - **ida-bridge 五条新取证**（`core.dll` = `idalib-33060`、`Core3D.dll` = `idalib-25128`；
    既有文档取的都是数据层，本轮补的是**图形层**——core 拿到变更集后究竟扩散成哪些重画）：
    ① 找到 GETWORK 的级联入口 `DDES_GraphicsMngr::postGetWork`（Core3D `0x1052e9f0`，`op==4`；
    5/6 是 postUndo / preRedo）；② 它**只吃 `DB_DB::type ∈ {1, 6}` = DESI + ISOD**，
    **CATA 根本不进这条路**（类型号从 `DB_DatabaseTypes::LoadDatabaseTypes_Generated`
    core.dll `0x5a2a9bb` 逐条读出）——这独立第二次证实 core 里目录件变更不靠变更扩散，
    并说明 **gen-model 的 CATA 反向级联是超出 core 的能力、没有 core 侧对照物可对拍**；
    ③ 变更→重画的完整判据（Core3D `0x1052f2e0`）：
    `重画 el ⟺ el.type ∉ {WORL, MNUM} ∧ ( gb(el) ∉ {1,2} ∨ ( gb(el)==1 ∧ gb(el.owner()) ∉ {1,2} ) )`；
    ④ `DES_DrawList::hasGraphicsIgnoreFlag`（Core3D `0x1051a920`，自身一层）是
    `isGraphicsIgnoredBetween`（上溯多层）的兄弟——**P1-2b 落守卫时是两条规则不能合并**；
    ⑤ **`gb == 3` 全产品无任何代码路径**：全 E3D 2.10 安装 868 模块 / 0.65 GB 扫 `push 4DCE6Fh`
    只有 `Core3D.dll` 命中，导出符号 `?graphicsBehaviour@DB_Noun@@QBEHXZ` 零导入方，
    所有读点只与 `1` / `2` 比较 ⇒ `3` 对每个消费点都与 `0` 等价，**守卫只需管 `{1, 2}`**；
    那 72 个 gb==3 noun 与 e3d-model family 对表几乎正落在 `ICABCO`(17) / `IPFCOM`(20) / `IFCOMP`(3+1) 上，
    语义 = 「照画、但画法由应用侧专用绘制路径提供」，属**几何覆盖度**问题而非增量问题。
    09-02 文档 §5 开放问题 4（gb==2/3 语义）就此两半都结掉；09-02 计划 P1-2b 第一条
    「矩阵导出加 `gb` 列」经核对**已经做掉了**，可划掉。
  - **核实第 11 类其实早已落地**（此前记的「在案未做」是错的）：09-02 计划 P1-2b 在 e3d-model
    `5da8510` 就做完并提交了，且比计划写的还全（矩阵 `gb` 列、`graphics_behaviour()`、
    109-noun 护栏、`nearest_unit` 只看祖先、`collect_unit_subtree` 不下钻、外加计划里没写的
    `pipeline::push_members` 消费口径）。
  - **补上缺的那一档：`gb == 2` 护栏**（e3d-model `src/category.rs`，本轮唯一的代码改动，
    只有测试与注释）。core 判据里 `gb == 2` 有独立一道门（层级容器永不单独重画），
    e3d-model 这边由 `classify` 收成 `List` / `Composite` 顺带承担、**没有任何东西钉住它**；
    而它不是空转——98 个 gb==2 noun 里**有 10 个同时带字典 `primitive` 位**
    （`CSURF` `PLTGRD` `HRPANE` `STRLNG` `CPANEL` `INTFRM` `HICPAN` `ENVLOP` `HTFEAT` `STRFLT`），
    今天靠 `classify` 的字典兜底臂不成件是**巧合不是保证**：谁照着 `primitive` 位补一条
    `Primitive(_)` 臂就会让这些容器静默出几何（`SBFI` 静默漏件的同型，方向相反）。
    新测试 `graphics_behaviour_two_containers_are_never_units`（98 写死 + 10 个逐个点名），
    并把 `graphics_behaviour` 文档里**已过期的**「2 / 3 的含义未读，不要拿来分派」换成四档语义
    与 core 判据原文，写明 **`3` 不该有对应护栏**——为它加分派才是分歧。
  - **启动复核对 CATA 变更不再零感知**（09-04 启动增量审核 #4 里写的「便宜的半步」，
    `model_update_pending.rs` + `initialization_phase.rs`）。此前一级过滤只比 DESI 文件最新，
    目录库前移了启动一根不排、而且**一声不吭**；direct 形态下连 CATA 数据批次都没有，
    唯一兜底是按需 `ensure` 撞上 `ModelTarget.catalogue` 指纹失配——那要等有人点到那个根。
    现在：`published_catalogue_sessions_on` 从 `gen_root.published_model_target.catalogue` 取
    「上一次发布落在哪一版目录」（`CATA dbnum → 最旧会话`；一条 `GROUP BY catalogue`
    **服务端去重**——一个库的全部根共用同一份 `ModelTarget`，几万行塌成个位数行，不是逐库点查），
    与 `model_source::latest_source_version` 一比得出过期名单；启动横幅点名 dbnum 并明说
    「不为此排任何模型工作」+ 人工出口，`/health.startup.reconcile` 新增
    `catalogue_cascade_degraded` / `stale_catalogue_dbnums` / `unreadable_catalogue_dbnums`。
    **只点名不排工作**——反查那一半（S4 / 09-02 计划 P1-4）仍未开工。
    两条刻意的克制各有守卫：目录那一格**不参与 `result` 取重**
    （`the_catalogue_flag_is_named_separately_and_never_downgrades_the_result`；混进去会让每一次
    有目录变更的启动都报成失败态，人就分不清哪次是真出岔子），以及「已确认过期」与「不知道」
    **分两栏**（`a_catalogue_that_cannot_be_read_is_unknown_not_stale`）。
    踩到并记进注释的坑：`GROUP ALL` + `array::flatten` 是错的——聚合函数逐行求值之后才被 GROUP
    收集，拿回来的是「数组的数组」，且不报错，只在反序列化那一步变成一句看不出所以然的类型不匹配。
  - **第 5 类「连接 / 邻居」的验收窗口：找过了，本语料压不出来**（新探针
    `e3d-model/tests/increment_real.rs::probe_chain_for_connection_change`，`#[ignore]`，
    与既有三条探针同款只做 `collect_window` + `plan_update`）。它值一条自己的探针是因为
    **NOZZ 跟 BRAN 不在同一条 owner 链上**：变换级联走链**内**的后代，这条走链**外**的引用。
    `PROBE_LIBS=ALL_DESI` 扫 **292 个 DESI 库、7 秒**：44 个连接改动窗口，**0 个能判别**，
    **0** 个 NOZZ 记录变过，**0** 个「名下有 NOZZ 的元素挪了位」。44 个窗口形状一致——
    变的是 **BRAN 自己的 `HPOS`/`TPOS`**（E3D 把接好之后的头尾位置回写在 BRAN 记录里），
    BRAN 因此是它自己的索引候选，路由容器照常进重算集。与 `Shape::Reparented` 同一类结论：
    **没有语料窗口能把这条联动做成判别性验收**。
  - **第 5 类就此结掉：`HPOS`/`TPOS` 是存储属性，不是由 `HREF` 解出的伪属性**（ida-bridge 实读）。
    core.dll 里 `ATT_HPOS` **只有一条 xref 且是初始化写点**（`0x56d379a`），零次读，所以
    core.dll 内唯一的 `addGetPosPlug` 注册点（`0x5a60f14`）没给它挂插件；Core3D 的三个位置
    伪属性注册点（`sub_10425CD0` / `sub_105EF3F0` / `sub_1062CCE0`）也一个都不碰它。
    写它的是 `MDR_ApplicationManager::disconnectHead` / `disconnectTail` 的 `putAtt`
    （接 / 断的时候把解出来的位置回写进 BRAN 记录），读它的一路是普通 `getAtt`，
    含 `GFX_GraphicsManager::Update`。**画的那一版与存的那一版是同一个**，于是两头堵死：
    E3D 回写了 → BRAN 记录变 → 我们照常重算；E3D 没回写 → 它自己也照陈旧值画 → 我们跟它一样。
    「接头动了、管路没跟着动」这条分歧在 SAVEWORK 语义下**构造性不可达**，探针留作回归。
    未证的一格已写明：`PDMS_HposElement::getHPOS`（`0x106690c0`）那次 vtable 派发（槽 `+64`）
    本轮没解，结论走的是「伪属性注册表里没有它」这条路。
  - **第 4 类反查的验收窗口先探明了：不缺，31 个**（新探针 `probe_catalogue_chains`，
    `PROBE_CATA_ALL_WINDOWS=1` 走完整条链）。30 个 CATA 库全部 ≥2 环；**`ams5052_0001`（链 34 环）
    一家就有 24 个窗口只改 1–3 件**，`184→185` 恰好一个 Modified 目录件——正是 P1-4 那句
    「改一个 SCOM 参数的 CATA 窗口」的天然形状；`ams6890_0001` 另有 4 个、`ams5054_0001` /
    `acp7041_0001` 各 1 个。其余 26 个库链头一律是一次性导入（几万到上百万个 Inserted），
    不能当门。**所以第 4 类与第 3/5 类不是一回事：不是没有窗口，是东西还没做**，挡在前面的
    只有反查表本身。下一个要问、本轮没答的：`ams5052` 有没有被本语料的哪个 DESI 库引用
    （没有则这道门空转）——这一问需要反查表，是 P1-4 第一步的副产品，不是前置。
  - **★ core 的 CATA→DESI 反查怎么做查清了：读 dab 文件自带的引用表，不扫库**
    （顺手结掉 09-02 文档 §5 开放问题 3）。`sub_5AAF4E0` / `sub_5AAE610` 是 dabacon **opcode 派发**，
    名表在 `off_6003900`（`const char*` 数组——**不是** `.ida_scratch/_opcodes.py` 里那张 6 字节
    内联表，那是另一个 IDB 的基址）：**326 = `db_start_ref_table_search`**、
    **328 = `db_get_next_ref_table_entry`**，配 `192 db_finish_table_search`；同族还有
    `122 db_start_int_table_search` / `132 db_start_name_table_search` / `260 db_start_sys_table_search`。
    `DB_RefTableIterator::increment` 的「键一变就 `db_finish_table_search`」说明**这张表按
    (属性, 目标 ref) 有序、搜索返回的是游标**，且**按库文件**存。
    **对 P1-4 的意义**：计划给反查源留了 ①（e3d-io 读 dab 引用表）/ ②（开库时全库扫描建内存表）
    两条路、定的是先做 ②；现在有正面证据 **core 走的是 ①**。这不推翻排期（② 落地快），
    但把 ① 从「也许可行」变成「一定存在且是权威口径」——`ams7351` 那种 334 万候选的目录库上
    两者不是一个量级。**未证**：只坐实了原语身份与调用/终止形状，**没读出引用表的页布局**，
    P2-2 的取证因此只完成一半。
  - **收口后的状态**：8 闭环（含第 5 类）/ 2 已记录为决策·不做（UDA 递归、`XRPNTR`）/
    1 有机制、验收门本语料压不出来（改挂新旧两根）/ **1 洞（CATA→DESI 的反查本身）**。
  **验证**：e3d-model `cargo test --lib -- category::` **25 passed / 0 failed**（含两条 gb 护栏）；
  新探针 `cargo test --test increment_real -- --ignored probe_chain_for_connection_change`
  在 `PROBE_LIBS=ALL_DESI`（292 库）与 `ams8000_0001` 下各实跑一遍，`rustfmt --check` 干净；
  gen-model `cargo test --lib -- data_interface::model_update_pending` **124 passed / 1 failed**，
  那 1 条是 `startup_coverage_reconcile_covers_unscoped_and_never_aborts_startup`，
  **与本改动无关**：它 `include_str!("../lib.rs")` 找带 `\n` 的字面量，而工作树里 `src/lib.rs`
  此刻是 CRLF（另一会话的编辑翻的；同一类事故本轮也发生在我改的
  `initialization_phase.rs` 上，已就地改回 CRLF）。`rustfmt --check` 三个改过的 `.rs` 全干净。
  均**未提交**、**未 live**。
  引用的 core / Core3D 地址全部本轮实读，可按地址复查（文档 §2 附地址表）；
  868 模块扫描与导出符号 findstr 均实跑。未验证项与边界写在文档 §4 与 ADR-061 末节。

- **落库审核的 P2–P7 一次收口（ADR-060 D10/D11/D12 + 迁移/影响面回写）。** 同日那份「模型生成落库审核」
  列了七条，P1（共享网格只增不删）经用户确认是**设计如此**——内容寻址的东西没有「属于哪一根」这回事，
  所以既不按根回收也不打算有可达性 GC，这一条改写进 ADR 的 D2 而不是当欠账修。其余六条：
  - **P2 旧前缀清扫接上生产调用点，挂在 `persist_dbnum` 的收尾，按库一次，且只在旧命名空间真的悬空
    之后。** 守卫两道都写进语句里：标记行（`queue_control:model_legacy_prefix_sweep`）＋「七处指针
    一个都不许还指着 `direct_*`」。**为什么不用进程内 `OnceCell`**：「一次」的作用域是**库**——进程记的
    那一份，换个进程连同一个库又扫一遍；语句自带守卫不多一趟往返，扫到一半崩掉也是两样都不算数。
    另开 `sweep_legacy_prefixes()` 给「已经整库重生成过、想单独补一次」的运维口。

    **第二道守卫与「挂收尾而不是挂单根」都是真库实测撞出来的，见下面的验证段。** 初版跟着单根落库的
    第一块走、且只判标记行——那一版在 AMS 7997 上会把整库还在用的包围盒与世界变换删掉。
  - **P3 房间派生面与空间树收敛从投影记录上重新驱动**（`room_publication` 从此不再是悬空模块）。
    **清边跟着删除那一块、排重算跟着回执那一块，两笔不能对调**：清边落在删除后面的块，中途失败就留下
    一批指着已删几何的悬空边，而下一轮对账再也看不见那些几何（行已经没了），悬空边永远不会被清；排重算
    落在前面的块，任务一被消费就在照着只写了一半的几何算房间归属。定向落库排重算 + 清边，全库落库只清边
    （ADR-010 §4 收窄 1）。动过几何的每一块同事务 `spatial_epoch` +1；提交之后按已提交指针收敛进程内空间树
    （新 `aabb_tree::converge_tree_after_persist`），**只有 `PersistTarget::process()` 才收敛**——树镜像的
    是全局那个库，自带句柄的目标去动它就是拿另一个库的指针污染这棵树。管身不进收敛名单：它不写
    `inst_relate`，树上本来就没有它的条目，照来源同步只会把这个 BRAN 判成「指针已消失」白摘一次。
  - **P4 ADR-060 的「未做」章过期**：「适配器还没有生产调用点」早就不成立（`model_refresh` 定向收尾与
    `exec_api::gen_dbnum` 整库收尾），房间那一条也随 P3 消。连同 core3d 一致性表里同一句陈述一并改判。
  - **P5 多块发布在凭证行上留一道可见性栅栏**：第一块写 `publication_status='publishing'`，最后一块的回执
    翻回 `ready`。凭证前移判的就是 `ready`，半截根因此不会被当成「S 时刻几何有效」的证明。
    **只在会盖章的多块发布上写**——没有回执就没人解除它，那时写栅栏等于把这根永久钉死；只有一块时事务
    本身就是原子的，中间态根本不存在。
  - **P6 拼 SQL 分两档：值转义，id 拒绝。** 字符串字面量位置（`noun` / `generic` / `manifest_hash` /
    `dependency_digest` / `root_refno` / `type::thing` 的 id 段）一律走 `sql_string()`；`表:⟨…⟩` 位置没有
    转义序列可用，走 `bracketed_id()` **拒绝**超出 `[0-9A-Za-z_]` 的东西。`noun` 撞上引号的概率约等于零，
    但这是**口径不是概率**：裸插值留一处，下一个往行上加字段的人就会照抄。转义之后渲染出来的字节与改前
    逐字相同，原有 15 条护栏一条没动就全绿。
  - **P7 取证探针不再自带一套渲染器**：`bin/direct_model_persist.rs` 只把生成结果摊成
    `ProjectedGeometry`，SQL 由 `model_db_adapter::render_records` 出。它原先出的是 `legacy-instanced-v2`
    口径（每个元素都写 `inst_info`、`geo_relate` 带 `pts:[]`、`inst_geo` 上多一份 `param.primitive_key`），
    与生产的行内容本来就不同——**取证探针出的行要是与生产不同，它取的证就不是生产的证**。顺带删掉
    重复的 `shared_mesh_identity` / `transform_value` / `LegacyRows` 一套（约 110 行）。

  **本轮七条新护栏**（进 `model_db_adapter::tests`）：PDMS noun 带引号与反斜杠时不逃出字面量且行上的值
  一字不差／记录 id 出了机器字母表整块渲染失败／旧前缀清扫按库一次（第二次落库不再重扫）／半截根停在
  `publishing`、补齐之后回到 `ready`／没有回执的落库一个字都不动凭证行／房间边与 `spatial_epoch` 跟着
  发布事务走（被移除的来源清边、留下的排重算、epoch 推高）。
  第七条是 core3d 一致性表 §8 点名要、当时因「文件被别的会话占着」没写的那一条：
  `reconciling_an_ancestor_root_does_not_delete_a_nested_roots_rows`——**按根对账的两边必须同一个口径**
  （库那边 `anc CONTAINS <根>`、存储那边 `records_of_root`，同一张祖先索引）。嵌套生成根 D 的行既挂在 D 下
  也挂在祖先根 A 下，任一边收窄成「只算直属这一根的」，另一边就把对方名下的行当孤儿删掉。两个方向各落
  一次断言，真孤儿照删。今天不咬人靠的是「祖先链是全库的 + 生成是整棵子树的」两个互不相干的事实，
  哪条被优化掉都由它先红。
  **验证（本轮实跑）**：`cargo test --lib -- fast_model::model_db_adapter` 全绿（收尾时 23 条，多出的那条
  `a_projection_that_stops_covering_the_nested_root_deletes_its_rows` 是并行的另一条会话在同一文件上补的
  绊线，与本轮这条正反一对）；
  `cargo test --bin direct_model_persist` 8 passed / 0 failed；`cargo clippy --lib --bins --all-targets`
  改动文件零告警；`rustfmt --edition 2024 --check` 四个改动文件干净。

  **真库实测（AMS 7997，`ws://localhost:8009`，SurrealDB 2.1.4，全库重生成 + 落库 + 清扫走了一整趟）。**
  这一趟改了两次结论，是本轮最值钱的部分：

  **① 落库前的现场根本不是 ADR 假设的那个状态。** 22,040 行 `inst_relate`（`dbnum=7997`、e3d-model 写的）
  **全部**还指着 `aabb:direct_world_*` 与 `trans:direct_*`，其中 7,908 行的 `out` 指着
  `inst_info:direct_*`。旧命名空间共 67,950 行，**没有一行是悬空的**——`direct_` 当年只戴在那三张表上，
  `inst_relate` 一直用不带前缀的 id，所以它没被新写入接管，反倒是活着的引用方。
  **初版清扫（跟着单根落库的第一块走、只判标记行）会把这 67,950 行连根删掉。**

  **② 加了守卫之后先空跑一次确认拦得住**：把生产那条语句原样打到真库上，`$legacy_live` 命中，
  **删 0 行、标记没盖**，计数一字未变。

  **③ 整库重落一遍**（`e3d_persist_smoke --dbnum 7997 --chunk 5000`，release）：

  ```
  GENERATE upserted=52098 shared=20167 baked=31931 unique_meshes=20878 failed=0  用时 36.4s
  PERSIST  介质=RocksDb 可持久=true 块大小=5000 事务=12 落行=52098 对账删=0 盖章=2  用时 4498s
  READBACK inst_relate=47475 tubi_relate=4623 平表三件齐活=47475 geo_relate带本地变换=15544
  ```

  生成 36 秒、落库 75 分钟——**瓶颈整个在 ws 往返 + rocksdb 写**，不在几何。

  **④ 重落之后清扫又被自己拦住了，暴露出守卫的两个错判**（这才是真跑一趟的价值）：

  - `geo_relate.in` 指着旧 `inst_info` 的那 7,908 行**自己就是要删的旧行**，把它们算成活引用等于
    永远拦住自己；
  - `inst_geo.aabb` 指着的 22 个旧共享本地盒是**真活引用**，但 `inst_geo` 按 N2 只增不删，这一条
    永远为真——处置不该是拦住整次清扫，而是把那 22 个盒子从删除里排除。

  改完（判据收到五处、`aabb` 删除加 `NOTINSIDE $shared_boxes`）再跑一次真库：

  | 项 | 落库前 | 清扫后 |
  | --- | --- | --- |
  | `inst_info:direct_*` | 7,908 | **0** |
  | `trans:direct_*` | 33,964 | **0** |
  | `geo_relate`（`in` 是旧 `inst_info`） | 7,908 | **0** |
  | `aabb:direct_*` | 26,078 | **22**（`inst_geo` 还指着的共享本地盒，刻意留） |
  | 清扫标记 | 无 | **true** |

  共回收 **75,836 行**，事后 `inst_relate.aabb` / `.world_trans` / `.out` 与 `geo_relate.in`
  **悬空数全为 0**；`inst_geo` 34 行一个没动（唯一一个没有 `aabb` 的是隐式管身，本来就不写）。

  **⑤ 平表快路真的被点亮了**：落库前 22,040 行里 **0 行**带齐 `aabb_d` / `world_trans_d` / `insts_flat`
  （Plant UI 每一行都在退 slim 兜底）；落库后 **47,475 / 47,475 全带齐**。同一批行上两条读法：平表投影
  **1.45s**、解引用 + 子查询 **3.39s**（ADR 里那对 0.7s / 10.7s 是另一个库、另一批行的数，别直接比）。

  **⑥ 发布栅栏与 `spatial_epoch` 都动了**：12 笔事务里 `spatial_epoch` 从 259 推到 **270**；跑完
  **没有一根卡在 `publishing`**，盖过章的根都回到 `ready`。

  **⑦ 房间重算改走定向那一档验，通过。** 整库那一趟一条 `room_recalc_*` 都没写，两个原因都成立：
  这个库 `room_relate` 本来就是 0 行（没有边可清），而 `persist_dbnum` 按 ADR-010 §4 收窄 1
  **本来就只清边不排重算**。所以给 `e3d_persist_smoke` 补了 `--root <a/b>`，走 `model_refresh` 的生产
  两步（`generate_and_persist_roots` → `persist_root`，`Directed` 口径）——**房间重算那一支只有这条路
  走得到，只跑整库那一档等于永远不覆盖它**。拿 7997 的 `24381/100060` 跑：

  ```
  GENERATE root=24381/100060 upserted=14 removed=0 failed=0  用时 0.81s
  PERSIST  事务=1 落行=14 对账删=0 盖章=1                      用时 0.35s
  ```

  库里随即多出 **14 条 `room_recalc_element`**（`status='pending'`，`target_refno` 与来源元素逐一对上，
  noun 是 SCTN / CYLI / EXTR），`spatial_epoch` 270 → **271**。

  落库前把 15 张表全量导出到仓外 `backup-7997-20260904-195215/`（~50MB）。

- **模型水位 P3 落地：双向对账 + 幽灵模型水位（ADR-060 D6/D7），挂进启动覆盖核对。**
  探针抽成一份共用取数 `ModelWatermarkProbe`（拦路根 / 已放弃根 / 安定下限 / 应用水位 / 行上已记的水位 /
  这个库有没有 `gen_root` 行），热路径与对账**共用同一份判据**，漂不了；支撑值
  `supported = 有拦路根 ? 0 : min(安定下限, applied_sesno)` 只写在 `ModelWatermarkProbe::supported()` 一处。
  **对账是双向的**：`model_sesno > supported` → 「失去支撑，已下调」；`< supported` → 「热路径漏记，已补记」。
  后一半是真异步的必然产物——模型跑到数据前面那一次按 D5 条件 3 跳过、或收口前进程被
  `clear_stale_at_process_start` 清空，**cohort 行删掉之后没人会再回来盖这一章，不补就永久丢**。
  上调不是凭空盖章：`settled_floor` 就是证据、`applied_sesno` 封顶，与热路径那次写入手上的证据一样多。
  **两个方向措辞不同**，混成一句现场就分不出是哪一种。
  **对账写 `=` 不写 `math::max`**，且改了号就把时刻抹成 `NONE`（行上那个时刻描述的是旧号，升降都对不上）。
  与热路径那条单调语句故意不合并：承诺要单调，更正要能往下写；合并的后果是「降不下来」静默失效，
  表现为一枚永远偏高的水位。
  **覆盖面是「有凭证的库 ∪ 没凭证的库」**：幽灵模型水位恰恰只长在后一堆里——`floors` 是一条
  `GROUP BY gen_root`，零行的库根本不出现在里面，只对它对账就永远发现不了「水位说追平过、模型面一行都没有」。
  幽灵的处置是清零 + 点名 + 走懒生成，**不删数据、不阻断、不触发重建**（与 ADR-021 的幽灵水位不是一回事）。
  **Q10 在这条路上比热路径更凶**：对账要是把已放弃根算进「还在赶」，`supported` 恒为 0，
  **每一次启动都会把刚追平的水位抹掉**，而现场只看到一枚永远为 0 的水位——护栏
  `an_abandoned_root_does_not_zero_the_watermark_at_reconcile` 钉住。
  **启动那条 `!body.contains("applied_sesno")` 的老护栏保持红线不变**：对账确实要读数据水位（M2 封顶），
  但那与「拿什么当生成时点」（ADR-054）是两回事，所以整体委派给 `reconcile_model_watermarks`，
  函数体里一个 `applied_sesno` 都不许出现；测试文档补了这条边界，并新增一条钉「覆盖面必须并上没有凭证的库」。
  **八条新护栏**（合计 16 条），`cargo test --lib` 我这部分全绿。

- **模型水位 P2 落地：`dbnum_watermark` 行上多一枚 `model_sesno`，在模型产物真的保存到数据库之后才盖章（ADR-060 D1/D5/D8）。**
  `dbnum_state.rs` 加六列（`model_sesno` / `model_sesno_time` / `model_settled_at` / `model_lazy_roots` /
  `model_dead_roots` / `model_roots_observed_at`，后三列 `NONE` = **未观察**，读侧不许降级成 0）；
  两条读路径共用一份投影常量，漂不了；`model_time_of` 守「水位为 0 时时刻一起消失」；
  `DbnumState::model_caught_up()` 只答行内能答的那一半，**不替实时未安定根数打包票**。
  **写入点**挂在 `run_regen_group` 的收口之后（`clear_regen_work_batch` → `settle_unechoed_roots` → 盖章），
  三个前置条件：除已放弃根外无未安定行、候选值 > 0、候选值 ≤ `applied_sesno`（语句里 `WHERE` 再挡一次，
  防探针与写入之间数据面掉队）。
  **实施时相对 ADR 原文收紧一处，已回写 D5**：盖章的值**不是队列刚跑完的 `T`，是安定凭证的下限**被
  `applied_sesno` 夹住之后的那个数。理由：这一窗没被触到的根在行上仍是旧凭证却算「安定」，照 `T` 盖章
  等于拿没人验过的数当承诺（凭证前移退化时正是这个形状）。夹逼之后启动退化窗口那条路也不必再靠名单
  排除——它写不出文件最新来，**名单会漏，夹逼不会**。`T` 只剩两个用处：候选值恰好等于它才贴时刻、
  记进 `model_roots_observed_at`。
  **D8 提前到本期**：`fast_delete` 的回退清库与 `applied_sesno = 0` 同一句归零，否则 P2 到 P3 之间这一版
  会带着幽灵模型水位；现有那条断言字面量的护栏同步改。
  **八条护栏**：盖章排在收口之后（源码顺序）／模型跑到数据前面时一个字都不写／不认领数据窗口的四类
  工作不触发／已放弃根（死信 + `http_422`）不卡整库而是照数露出／还在赶的根照旧拦住／盖的是凭证下限
  不是窗口右端／时刻跟着单调条件且排在覆盖之前／**M7 两枚水位是仪表不是闸门**（数据写回全文不读
  `model_sesno`、模型生成不以 `applied_sesno` 为前置、生成这一组的过程本身不看数据水位）。
  **验证**：`cargo test --lib` 1448 passed / 4 failed / 101 ignored；4 条失败全部与本改动无关且改动前即存在
  ——3 条 `increment_manager` 是working tree 里 `DbOption.toml` 打开了 `watch_dbnums = [7998, 8000]`
  这个调试开关所致，1 条 `ops.html` 缺 `id="verdict"` 是 d-34 已记录的存量。
  **未做**：`model_lazy_roots` 还没接（要从窗口规划器的 `lazy` 桶取，随 P4 露出一起）；D6 双向对账是 P3。

- **每库第二枚水位定案：ADR-060「模型水位是被写下的承诺，与应用水位相等才叫一致」（P1 定义期，代码未动）。**
  用户当日提出：「数据解析的和模型生成的 `dbnum_watermark`，模型那一枚也要以对应 sesno 的模型生成完、
  **保存到数据库之后**再设置；两枚相等才表明这个 dbnum 处理的是最新一致的——因为两边现在是异步并发的。」
  **先查清的实况**：模型侧的库级进度**已经存在**，但形态不对——根级是 `gen_root.source_end_sesno`（发布 CAS 时写下，
  `model_update_pending.rs:1417-1423`），库级只有一个查询时算出来的派生量 `startup_coverage_floors()`
  （`:1825-1925`，安定根里最小的凭证），已并排露在 `/dbnums` 上（`manual_update.rs:2810`）。
  所以缺的不是「有没有第二枚」，是**那一枚不是被承诺写下的**。
  **五条会正面打架的既有约束**逐条摊开后拍板（Q1① Q2a Q3c Q4② Q5① Q6b Q7① Q8① Q9①，方案 C）：
  ① `CONTEXT.md` 两处 `_Avoid_: 模型水位` 删掉、该词转正；
  ② 刻度取**数据窗口右端 T** 而不是文件最新——ADR-054 的生成时点语义一个字不改，两者管的是不同的事，
  模型按文件最新生成到更新版本时这枚水位仍只记 T（**宁可保守不许虚报**），由此两枚同域可判等；
  ③ 懒生成 / 冷库的根不拉低水位，单出 `model_lazy_roots`，且**未观察写 `NONE` 不写 0**；
  ④ 一致判据 = `applied_sesno > 0 且 model_sesno == applied_sesno 且无未安定根`，`lazy_roots` 不参与
  （ADR-058 D6 下算进去等于灯永远不亮）；
  ⑤ 共识 d-22「模型凭证下限只有一套口径」由「承诺 + 支撑校验」保住——`startup_coverage_floors` 降为校验者，
  不再单独露给读者。
  **形状是复刻不是发明**：承诺（D1）+ 支撑校验（D6）+ 幽灵态处置（D7）+ 清库同事务归零（D8），
  正是 ADR-021 给数据水位定的那一套在模型面的对偶。
  **拟 ADR 时挖出两个会静默毁掉它的坑，同日一并拍板**：
  ⑴ 一个重试穷尽的死信根 / `http_422` 根**永远**是未安定行，会把该库的 `model_sesno` 永久卡死——
  其余几千个根都好好的、每一窗都正常追上，灯却从此永红，且看不出被谁卡。裁决：已放弃根不计入「未安定」，
  生成根改按**四格**分类（已追平 / 还在赶（拦水位）/ 已放弃（不拦但必须并排陈述）/ 没生成过（不拦）），
  互不重叠、各有嗓音；过去这四格里有三格挤在「未安定」一个数里。
  ⑵ 用户同轮补述「我的目的就是要保证数据解析的内容保存和模型生成的保存能够异步」→ 立为
  **M7：这两枚水位是仪表不是闸门**，没有任何代码路径可以等它、轮询它、或以它为条件放行另一条链
  （源码护栏钉数据提交路径不读 `model_sesno`、模型生成路径不以 `applied_sesno` 为前置）；
  连带把 D6 对账改成**双向**——下调是「失去支撑」，上调是「热路径漏记补回」（模型跑在数据前面那一次
  按 D5 条件 3 跳过、或收口前被 `clear_stale_at_process_start` 清掉那一次，不补就永久丢）。
  上调不是凭空盖章：`settled_floor` 就是证据、`applied_sesno` 封顶，与热路径那次写入手上的证据一样多。
  **不做**：不把两面重新绑成一个提交单元（ADR-056 Q1 明写「不回 kv-mem」）、不让模型失败阻断数据水位、
  不新建同粒度第二张表、不改 ADR-054。
  **改动**：新增 `docs/adr/ADR-060-*.md` 与 `docs/plans/2026-09-04-model-watermark-per-dbnum-plan.md`
  （后者保留备选方案 A/B 与取证）；`CONTEXT.md` 新增三条词条（模型水位 / 模型水位失去支撑 / 幽灵模型水位）、
  改两条 `_Avoid_`。**P2–P4 未实施，读代码仍以现码为准。**

- **写入口径改成「先删后插」，没有 UPDATE / UPSERT（用户 2026-09-04 拍板：「初始化时不需要考虑更新，
  直接都是 insert batch ignore。然后增量更新时，我们对模型数据都要采用针对 ids 先删除，然后再
  insert batch ignore。所以不存在 update 等」）。** 新增 `WriteMode`：`Initial` 直接批量插、撞上已有
  id 就跳过；`Incremental` 先按 id 把这些记录的**整组表示**删掉（`geo_relate` 按 `in` 删、`inst_info` /
  `inst_relate` / `aabb:<id>` / `trans:<id>` 按 id 删，两种形状一起删），再批量插。**删这一步不是保险
  而是必需**：`INSERT IGNORE` 撞上已有 id 会保留旧行，不删就等于把上一轮的几何留在库里。渲染形态从
  「每元素一串 UPSERT」换成「一张表一条批量 `INSERT IGNORE INTO <表> [ … ]`」，被引用的行（盒、变换、
  网格、实例信息）排在引用它们的关系之前。
  **两处必须区别对待**：`geo_relate` 与 `tubi_relate` 走 `INSERT RELATION IGNORE INTO`（关键字顺序是
  `RELATION` 在 `IGNORE` 前）——普通 `INSERT` 只把 `in` / `out` 当两个字段写下去，**不登记成图上的边**，
  `out->geo_relate` 与 `pe:⟨branch⟩->tubi_relate` 就都走不通（这一条是 `mem://` 真跑打出来的：先用普通
  INSERT，实例子查询取回空、分支重置删不掉旧管身）；`inst_relate` 反过来**不能**带 `RELATION`，
  烘焙实体那一行只有 `in` 没有 `out`（网格挂在 `booled_id` 上），而关系插入两端都是必填。
  护栏两条：`an_initial_batch_is_insert_ignore_only`（整段钉死，正文里不许有 UPDATE / UPSERT）、
  `an_incremental_batch_deletes_the_ids_first`（前半是按 id 的五条删除，后半与初始化那一档逐字相同）。
  **取证探针 `bin/direct_model_persist.rs` 一并收口**：同样先删后插 + 批量 `INSERT IGNORE`
  （`geo_relate` 带 `RELATION`），顺带把 `aabb_d` / `world_trans_d` 两个行内副本直接写进 `inst_relate`
  行（原先是插完再 `UPDATE` 补一遍）。它那套「写前备份行 → 产出 `rollback.sql` 用 UPDATE 退回去」随之
  **取消**——没有 UPDATE 就没有可退回的旧值，要退回就重跑上一版生成；`inst-relation-backup.json` 与
  `rollback.sql` 两份产物不再产生，`backup_query` / `old_or_none` 与 `DIRECT_PERSIST` 那行的
  `db_records=` 字段一起删。它自带的 Plant UI 形状断言测试也改成先摆一行上一轮的 `inst_relate`、
  走一遍删除再插——不删的话 `INSERT IGNORE` 会撞上同 id 直接跳过，这条测试本身就是那个语义的证据。

- **落库接上生产调用点，并在 7997 上端到端跑通（用户 2026-09-04：「把 persist_root 接到生成消费端，
  开 in_memory_db=true 跑一遍 7997 看真实效果」）。**
  接线放在**编排层**而不是模型面（ADR-057：模型面不认识库）：
  `data_interface::model_refresh::generate_roots_report` 在 `generate_and_persist_roots` 成功之后逐根
  `model_db_adapter::persist_root`——落不进去的那根算它自己失败、进重试账（D1 的根级口径），
  **投影在内存里是对的，但 Plant UI 看不见的东西不能算完成**；整库那条路
  （`python/exec_api::gen_dbnum`）收尾调新增的 `persist_dbnum(dbnum)`，它按
  `ModelMemoryStore::roots()`（新增，取回执的键——一根投影完哪怕零变更也有回执，而祖先链在空根上
  整个消失）逐根对账，一根一个事务。
  **新增一次性烟测 `src/bin/e3d_persist_smoke.rs`**：走的就是生产那两步，跑完即止不驻留；
  **介质由配置说了算，程序不自己挑**——第一版让它自己接 `mem://`，回执立刻就说自己是 `RocksDb`
  （`PersistTarget::process()` 读的是配置），正是 `PersistTarget` 想防的那种「说错介质的回执」，
  于是改成跟 `run_app` 同一个开关。
  **实跑（`AIOS_IN_MEMORY_DB=1`，dbnum 7997）**：
  ```
  GENERATE upserted=52098 shared=21137 baked=30961 unique_meshes=20123 failed=0 用时=29.2s
  PERSIST  介质=InMemory 可持久=false 落行=52098 对账删=0 盖章=2 用时=22.1s
  READBACK inst_relate=47475 tubi_relate=4623 平表三件齐活=47475 geo_relate带本地变换=16514 inst_geo=34
  ```
  三处对得上：`47475 + 4623 = 52098`；`47475 = 30961 烘焙 + 16514 共享非管身`；
  `4623 = 21137 共享 − 16514`（管身全是标准图元）；`inst_geo=34` 是 20123 个网格里真正共享的那几种
  标准体（BoxV1 与各分段数的 CylinderV3），实例化的收益就在这个数上。**47475 行全部三件齐活**，
  Plant UI 的平表快路一行都不用退回 slim。
  **顺带看清一件事**：整个 7997 只有 **2 个生成根**，也就是两笔各约 2.6 万件几何的事务，落库 22.1s。
  这个粒度与旧发布事务一致（一根一提交），但大根上的单条语句确实很大，要不要按块切开是下一个话题。

- **`inst_relate` 行补上 Plant UI 平表快路要的两个行内副本 `aabb_d` / `world_trans_d`。** 那条快路
  （`query_insts_flat`）要三件齐活——`aabb_d` / `world_trans_d` / `insts_flat`——才能零解引用零子查询直接
  成型；我们此前只写了 `insts_flat`（旧发布事务就没写那两个），于是 e3d 行**整批退回 `query_insts_slim`
  现值兜底**。只慢不错：实库 53,582 行上那两档解引用与子查询约 10.7s，平表读约 0.7s。取证探针原先是插完
  再 `UPDATE` 补一遍，现在两边都直接写在行上。护栏 `the_flat_fast_path_finds_all_three_inline_copies`
  起 `mem://` 按 `query_insts_flat` 的投影原形查一遍，顺带钉住 `owner` 从 `anc[1]` 还原这条约定
  （祖先链顺序不只是给 `anc CONTAINS` 用的集合）。管身不需要：`query_tubi_insts_by_brans` 走的是
  `aabb.d` / `world_trans.d` 解引用。

- **新增 `docs/adr/ADR-060-model-row-ids-are-record-ids-and-writes-are-delete-then-insert.md`**：把这套
  id 规则（D1–D2）、`geo_relate.transform` 新口径（D3）、写入口径（D4–D6）、只服务当前投影（D7）、
  `gen_root` 回执少了 revision CAS（D8）与平表快路的三个行内副本（D9）连同四条不变量、影响面、迁移与
  未做项一次写清。

- **模型行 id 一律就是记录 id，五层前缀全撤（用户 2026-09-04 拍板：「我不需要前缀，帮我开始修改正确，
  包括 plant-ui」「我们在更新时都是要先删除，然后再增加。所以用 ID 就好了，不要加什么前缀」）。**
  | 行 | 旧 id | 新 id |
  | --- | --- | --- |
  | `inst_relate` | `<记录 id>` | 不变 |
  | `inst_info` | `direct_<记录 id>` | `<记录 id>` |
  | `aabb` 世界盒 / `trans` 世界变换 | `direct_world_<记录 id>` | `<记录 id>` |
  | `trans` 本地变换（仅共享图元） | `direct_local_<记录 id>` | **整行取消**，改写成 `geo_relate.transform` |
  | `aabb` 共享本地盒 | `direct_shared_<mesh id>` | `<mesh id>` |
  | `inst_geo` | `<mesh id>` | 不变 |
  | 记录 id 本身（隐式管身） | `derived_<64 位摘要>` | `<64 位摘要>` |
  | 历史快照的库行 | `hist_<key>_<记录 id>` + `snapshot_key` 字段 | **不再渲染**（见下） |
  那层前缀原本防的是**旧生成器同表撞 id**：`inst_info` / `aabb` / `trans` 这三张表在旧世界里是按几何实例
  做键的（旧行长这样：`inst_info:zz_f1_10`、`inst_info:⟨1⟩`、`type::thing('inst_info', cata_hash)`），
  e3d-model 要在同一张表里自己铸 id，就先划了个 `direct_` 命名空间。今天这三张表**只剩本模块一个生产
  写口**——旧生成器的入口在非默认的 `legacy_model` feature 后面，`aios_core` 与 plant-ui 侧都没有 SQL 写口
  （全仓扫过：其余命中全是 `zz*` 测试夹具与 `direct_model_persist` 取证探针）——前缀于是没有再付的理由。
  **`derived_` 与 `hist_` 一并撤掉**：派生几何的记录 id 就是身份的 64 位十六进制摘要，与 refno 的
  `<十进制>_<十进制>` 形状天生分得开，不需要前缀自证身份——它当年的理由是「按根删除时普通行能靠 refno
  猜到 id，派生行猜不到」，而删除路径早改走 `geometry_id` 清单、更新一律先删后插，**猜 id 这件事本身
  没有了**。`hist_` 那一层随之消失的方式是**把历史快照的库行渲染整个撤掉**：没有前缀，两个投影就不能
  共存在同一批表里；而历史快照本来也不进生产库（按需快照由 `historical_model` 直接从存储回包 JSON，
  `persist_root` 一直只服务当前投影）。适配器因此不再收 `ProjectionScope` 入参，`snapshot_key` /
  `source_refno` / 历史那种 `tubi_relate:⟨<id>_<n>⟩` 关系 id 一起下线。
  **本地变换整行挪到边上**（用户同轮拍板：「把本地变换改成写在 `geo_relate` 行上，连 `_local` 那个后缀
  一起去掉（要改 plant-ui 读口）」）。它原本非带标记不可，是因为一个共享图元元素在 `trans` 表里要占
  两行（世界一行、本地一行），而世界那一行占了裸 `<记录 id>`。现在本地变换直接以 `transform` 字段写在
  `geo_relate` 边上（对象形状与旧 `trans.d` 逐字相同，与 `insts_flat[].transform` 同名同形），
  `trans` 表回到「一个元素一行」，前缀与后缀一起消失，读侧还少一次解引用。
  **读口改成 `transform ?? trans.d`，两种形状一起认**（旧生成器写的行与切换前的存量仍是 `trans` 链接）：
  `plant-ui/vendor/rs-core/src/rs_surreal/inst.rs`（`query_insts` × 2 与 `query_insts_slim`）、
  `plant-ui/crates/plant-ui-data/src/lib.rs` 的实例子查询、同一份 `old-aios-core` 的
  `rs_surreal/inst.rs`，以及 gen-model 自己的四个读口（`pdms_inst` 的 `insts_flat` 物化回填、
  `aabb_refresh` 的几何盒重算、`aabb_tree` 的可建行计数、`manifold_bool` 的布尔取数）。
  `query_insts_flat` 那条快路读的是行内 `insts_flat`，不受影响。
  **其余 Plant UI 读口零改动，这是查过的**：读侧全程走记录链接（`aabb.d` / `world_trans.d` /
  `out->geo_relate ... trans.d` / `booled_id` / `record::id(out) as geo_hash`，见
  `plant-ui/vendor/rs-core/src/rs_surreal/inst.rs`），从不解析 id 形状；全库唯一提到 `direct` 的是
  `crates/plant-ui-data/tests/direct_model_legacy.rs`，它匹配的是 `direct_model.format` 这个**字段**，
  不是 id。`direct_model` 字段名本轮不动。
  **切换要跑一次清扫**：新 id 的逐条删除语句够不着旧行（`inst_relate` 会被同 id UPSERT 盖掉，但它原先
  指向的 `inst_info:direct_*` / `aabb:direct_world_*` / `trans:direct_*` 会悬空），所以新增
  `render_legacy_prefix_cleanup()`——按 `string::starts_with(<string>record::id(id),'direct_')` 扫
  `inst_info` / `aabb` / `trans` 与指向它们的 `geo_relate`，每个库跑一次即可；模型本来就是从文件重生成的，
  删干净不丢东西，`inst_geo` 与盘上的 `.mesh` 不在其中（从来没带过前缀，且是内容寻址的）。
  **改动**：`model_db_adapter.rs` 的六处渲染与两处清理（并去掉 `ProjectionScope` 入参）、
  `e3d_mesh_store::geometry_record_id`；取证探针 `bin/direct_model_persist.rs` 一并改口径
  （它自带一套渲染，不改就会往库里写回旧命名空间——id、本地变换挪位、它自己那两条 Plant UI
  形状的断言查询都跟着走）；上面列的七个读口；plant-ui 侧三处提到
  `inst_relate:derived_*` 的注释（那条查询本来就按 `in` 反查 + 排除 refno 键，不按 id 形状匹配，
  所以只是注释过时）。
  **护栏**：新增三条——`no_row_id_carries_a_prefix`（渲染正文里 `'direct_` / `⟨direct_` /
  `direct_world_` / `direct_shared_` / `_local` / `derived_` / `hist_` / `snapshot_key` 一个都不许出现，
  同时钉住 `direct_model` 字段名照旧）、
  `the_plant_ui_instance_subquery_still_resolves_the_local_transform`（起 `mem://` 真跑一遍 Plant UI 那条
  insts 子查询的原形，拿回 `geo_hash` 与本地变换）、
  `the_one_shot_sweep_takes_the_old_namespace_and_leaves_the_new_rows`（起 `mem://` 真跑清扫，旧行清空、
  新行不动）；原有那条整行钉死的烘焙断言与共享图元断言同步换成新形状。
  **验证（本轮实跑）**：`cargo test --lib -- fast_model::model_db_adapter` 12 passed / 0 failed；
  `cargo test --bin direct_model_persist` 8 passed / 0 failed；`cargo test --lib` **1448 passed / 4 failed**
  （四条同一批既有环境红：三条 `increment_manager` 读工作区 `DbOption.toml` 的 `watch_dbnums`，
  一条 `handlers` 查内嵌 `web/ops.html`）；`cargo check --all-targets` 无新增警告；`rustfmt --check` 干净；
  plant-ui 侧 `cargo check -p plant-ui-data` 通过。

- **落库适配器 `fast_model::model_db_adapter`：从投影存储读记录，拼回 Plant UI 一直在消费的那套库行
  （用户 2026-09-04 提出：「接着做落库适配器：从 `model_memory_store` 读记录，拼回 `inst_relate` /
  `tubi_relate` 那套库行，注意和之前的数据格式保持一致」）。**
  它是上一条（生成只写内存）的另一头：`inst_relate` / `tubi_relate` / `inst_info` / `aabb` / `trans`，
  外加共享网格的 `inst_geo` 与 `geo_relate`。一条 `ProjectedGeometry` 携带的就是当年拼进 SQL 的同一份
  内容，所以这里只渲染，不回头重算几何、不重新读盘。
  **格式一致是核对过的**（**行 id 随后按上一条去掉了 `direct_` 前缀，字段与取值仍逐字相同**）：把旧文件
  （`HEAD:src/fast_model/e3d_model_service.rs`）与新模块里的格式串按
  占位符归一后逐条比对，九条模板逐字相同——世界盒与世界变换两行、共享图元那一整组
  （`aabb:direct_shared_<mesh>` + `trans:direct_local_<id>` + `inst_geo` + `inst_info` + `geo_relate` +
  `inst_relate`）、烘焙实体的 `inst_relate`、隐式管身的 `INSERT RELATION INTO tubi_relate`（关系 id 仍是
  分支全局的 `[pe:⟨branch⟩,n]`）、换表示清理、几何删除补的两行、当前投影与历史快照两种分支重置、
  `root_dependency` 依赖收据。记录 id 的 `hist_<key>_` 前缀与各表的 `snapshot_key` 字段由适配器在渲染时
  补回去——存储按投影域分区，前缀只有库里需要。
  **有意不同的只有 `gen_root` 回执两处**：`published_revision` 从 `={revision}` 改成 `=desired_revision`
  （回执里没有 revision）；revision CAS 那条守卫撤了，「同一根不许被两个源会话混着发布」那条留着，
  判据改读活行上的 `desired_target.source_end_sesno`，与写在同一条事务边界上。
  **落库走对账而不是回放**：存储是快照不是流水，所以 `persist_root` 按根对——库里由 e3d-model 写过、
  存储里已经没有的几何就是要删的陈旧行，碰到的 BRAN 先整段重置再插这一轮的管身，整批一个
  `BEGIN/COMMIT`。只服务当前投影：历史快照从来不进生产库，它的记录由 `historical_model` 直接回包。
  **两种落库介质，一套行**（用户同轮补充：「保存到数据库可以分为内存模式和 rocksdb 模式」）：库可以是
  进程内嵌的 kv-mem（`in_memory_db=true`），也可以是 rocksdb（默认那台 ws 连的 SurrealDB，rocksdb 在服务器
  侧；不带 `ws` 的 `local` 构建是进程内嵌的 `rocksdb://<project>.rdb`）。**渲染那一半拿不到介质**——
  `render_records` 的签名里没有它，所以想分叉也分不出来，两种模式下写出去的行逐字相同。新的
  `PersistTarget` 把库句柄与 `PersistMedium` 绑在一起（`Surreal<Any>` 自己说不出连的是谁，拆开传迟早出现
  一份说错介质的回执），`PersistTarget::process()` 按 `options::in_memory_db()` 判、判法与 `lib.rs` 的连库
  顺序对齐；回执 `PersistedRows` 带上 `medium`，`is_durable()` 点明**内存模式的行活不过进程重启，那不是
  「落库了」**。注意本仓 surrealdb 依赖没开 `kv-rocksdb`（Cargo.toml 里注释着），所以 `local` 那条
  进程内嵌 rocksdb 今天连不上——要用得先开 feature。
  **改动**：新增 `model_db_adapter.rs`；`e3d_model_service` 的 `geometry_source_refno` / `transform_value`
  提到 `pub(crate)` 给适配器复用（这两支不是库口径，护栏照旧绿）；`fast_model/mod.rs` 注册新模块并顺手
  按 rustfmt 的顺序排了那一组 `mod`（`model_memory_store` 那行原先就没排）。
  **护栏**：`model_db_adapter::tests` 十条——烘焙那一行整条钉死（含 `anc` 打包值与 `direct_model` 对象）、
  共享图元那一组的六条断言、管身走 TUBI 契约且不碰 `inst_relate`、同分支两段的槽位不撞、历史快照补前缀
  与 `snapshot_key`、删几何不碰共享网格、回执落在窗口规划器认的那几列上；三条起 `mem://` 真跑：语句能执行
  且行数落在对的表上、换表示后库里只剩新的那一个、对账把存储不再持有的元素与上一轮管身都删掉。
  **验证（本轮实跑）**：`cargo check --lib --tests` 通过、无新增警告；
  `rustfmt --edition 2024 --check` 三个改动文件干净；`cargo test --lib -- fast_model::model_db_adapter`
  10 passed / 0 failed；`cargo test --lib` 见下（与上一条同一批既有红）。
  **未做**：适配器还没有生产调用点——什么时候落库、按根还是按批，是上层的事，本轮不替它决定；
  房间派生面与空间树收敛（`room_publication` / `render_spatial_epoch_bump` /
  `sync_tree_from_committed_pointers`）仍停在上一条那里，要从存储的记录上重新驱动。

- **模型生成只写进程内投影，`e3d_model_service.rs` 全文不再认识数据库（用户 2026-09-04 提出：
  「清理这里面的 surreal 查询的所有内容，模型生成先只考虑保存到内存」，落库改由另一层适配接口承担）。**
  这一步撤掉的是发布事务那一整套：`aios_core::SUL_DB` 直写、`BEGIN/COMMIT` 包裹、
  `inst_relate` / `tubi_relate` / `inst_info` / `aabb` / `trans` 五张表的 UPSERT 与 DELETE 渲染、
  `gen_root` 凭证行的 revision CAS 与依赖收据、pre-e3d 旧行清理（`anc CONTAINS` 谓词那三条查询）、
  以及顺着事务被拉进来的房间语句与空间 epoch bump。**mesh 文件照旧写盘**——那是内容寻址的文件，
  不是库。
  **新模块 `fast_model::model_memory_store`**：按投影域（`Current` / `Historical(<key>)`）分区的记录表。
  一条 `ProjectedGeometry` 携带原先散在五张表里的同一份内容（几何身份、来源 PE、祖先链、世界盒与
  世界变换、mesh 身份、图元共享信息），适配器照它拼库行即可，不必回头再算一遍几何。三条旧语义
  照搬：投影域分区取代 `snapshot_key` 字段与 `hist_` 前缀（记录 id 因此不再需要前缀）、
  `tube_branch_resets` 取代 `DELETE pe:⟨branch⟩->tubi_relate`、`RootReceipt` 取代 `gen_root` 行上的
  `published_model_target` / `published_manifest_hash` / `published_geometry_count`。
  **没跟过来的是 revision CAS**：那把锁护的是多个进程抢同一个库行，进程内投影没有那个竞争；
  「不可变目标没变就整根不重算」这条判据完整保留，只是判据两端都从库行换成了回执。
  祖先索引（`by_ancestor`）是 `anc CONTAINS <packed>` 的内存形态，按根查 / 按根删走的还是同一条路。
  **改动**：`e3d_model_service.rs` 3747 → 2580 行（新增 `model_memory_store.rs` 624 行）——
  `prepare_geometry_delta` 换成纯结构化的
  `project_geometry_delta`（返回 `ProjectionCommit`，不再返回 SQL 串）、`apply_geometry_delta_on`
  去掉 `&Surreal<Any>` 入参改收投影域、`ProjectionScope` 迁进存储模块（它原本的职责就是给 SQL id
  加命名空间）、`RootPublishClaim` 三个反序列化行结构与 `load_root_publish_claim` 删除、
  `flush_deferred_model_spatial_refresh` 与 `DEFERRED_MODEL_SPATIAL` 删除（连同
  `model_update_pending.rs` 那个调用点）、`ensure_not_older_than_persisted` /
  `persisted_session_newer_than` 改读存储的单调水位。`historical_model.rs` 的 `mem://` SurrealDB 整个
  去掉：几何进存储的 `Historical(<key>)` 那一格，回执进一张进程内表，`query()` 的四个工具
  （snapshot / instances / tubes / geometry）改成从记录上拼 JSON——`inst_info_id` / `aabb_id` /
  `world_trans_id` 这三个只在库里有意义的指针字段换成了 `world_bounds` / `world_transform` /
  `local_transform` 这些真数据。`room_publication` 暂时没有生产调用点（发布事务撤了，它渲染的房间
  语句跟着撤），整模块 `#![allow(dead_code)]` 留在原地等适配器接手。
  **护栏**：`e3d_model_service::tests` 头一条 `the_generation_service_never_reaches_for_a_database`
  ——正文里出现 `SUL_DB` / `surrealdb` / `type::thing` / `UPSERT` / `INSERT RELATION` /
  `BEGIN TRANSACTION` / `gen_root` / `inst_relate` / `tubi_relate` 任意一个即红（连注释也不许再用库口径
  说话）；`a_reprojected_root_resets_its_branch_tubes_and_drops_stale_geometry` 钉一根重投影的完整形状
  （分支管身整段重置、上一轮多出来的几何被删、每条记录都带到根的祖先链、水位推进）；
  `an_unchanged_manifest_commits_only_the_receipt` / `..._still_repairs_an_incomplete_projection` /
  `an_identical_immutable_target_reprojects_before_any_geometry_build` /
  `a_missing_or_different_target_never_hits_the_cache` 接手原先四条 SQL 文本断言；
  `projection_reads_the_store_once_before_the_root_loop` 接手 `publication_prefetches_database_state_...`。
  `model_memory_store::tests` 六条（按根可达且按库筛选 / 删记录收干净祖先索引 / 分支重置只清本分支管身 /
  投影域互不可见且可整格丢 / 水位只涨不落 / 记录数与 mesh 文件同时对上才算投影完整）；
  `historical_model::tests::dropping_snapshot_removes_its_namespace_but_leaves_other_snapshots`。
  **顺手修的一处既有脆弱**：几条按文本形状判定的护栏（`split_once("\n}\n")` 那一类）原先直接
  `include_str!` 生文，`core.autocrlf` 检出的 CRLF 会让它们无故变红；统一走 `service_source()` 归一行尾。
  **验证（本轮实跑）**：`cargo check --all-targets` 通过、无新增警告；
  `rustfmt --edition 2024 --check` 四个改动文件干净；
  `cargo test --lib` **1428 passed / 4 failed**，四条红全部与本次改动无关且改前即红——
  三条 `increment_manager` 读的是工作区 `DbOption.toml` 里既有的 `watch_dbnums = [7998, 8000]`，
  一条 `handlers::the_embedded_panel_is_real_and_a_disk_copy_still_wins` 查的是内嵌的 `web/ops.html`。
  **未做**：落库适配器本身（读存储、拼库行、写 `SUL_DB`）本轮不在范围内，因此**模型面暂时不落库**；
  房间派生面与空间树的收敛也随发布事务一并停在这里，等适配器接手时从存储的记录上重新驱动。

### 修复

- **增量路终态渲染器的两处既有缺陷：不写 `ATT_UDA`、`pe_owner` 边方向写反（退役收口方案
  `docs/plans/2026-09-04-legacy-parser-retirement-endgame-plan.md` F5 / F6，阶段 B8.1；与基线换底座无关，单独可回滚）。**
  起因是给 `parse_pdms_db` 退役做 B8.0 核实时对了一遍两条写路：`ele_ops::render_terminal_element_state`（生产直写
  `persist_latest_main_data → render_terminal_persist_statements → PersistElementChange::to_surql` 的唯一渲染器）只出
  `pe / ATT_<noun> / pe_owner`，e3d-io 收集器装进 `uda_atts` 的 UDA 值到库里就没了；同一函数把 `pe_owner` 写成
  `in = owner, out = child`，而仓里其它所有写口（基线 `pe.rs::save_pe_relates`、`cata_closure::render_pe_owner_statements`、
  legacy `EleOperationDetail::to_surql`、rs-core `export_data`）与全部读口（rs-core `{owner}<-pe_owner` 取 `in` 当 children、
  `->pe_owner.out` 取 owner、`unique_pe_owner (in, out)`）都是 `in = child, out = owner`——增量路写出的子树在树面板上读不到，
  `DELETE pe:{id}<-pe_owner` 清的也不是它自己刚写的边。对拍 `direct_window_replay_parity` 没抓到，是因为它跑的是兼容路
  `render_persist_statements`（legacy `to_surql`），不是生产渲染器。
  **改动**：新模块 `data_interface::uda_keys::UdaKeyMap`——一次 `SELECT id, UKEY, UDNA FROM UDA` 建表，`resolve()` 与
  `aios_core::get_uda_refno` 同判据（`UKEY == hash`，查不到按 dehash 四字名 + 序号回退），查不到的键记账
  （`misses_warning` 进回执 `warnings`，不静默丢、不拦水位）；`render_terminal_element_state` 增写 `ATT_UDA:{id}`，CONTENT
  由 legacy 基线同一个 `NamedAttrMap::gen_sur_json_uda` 产出（`{ id, TYPE, refno: <TYPE>:<id>, udas: [{ u: UDA:<def>, v }] }`，
  标量 `v` 是 NamedAttrValue 带标签形——现库就是这个形状，rs-core `NamedAttrValue::from((utype, Object))` 取首字段解回），
  `Modified` 无 UDA 值时 `DELETE ATT_UDA:{id}`；`Add` 的前置清理多一条 `DELETE ATT_UDA:{pe}`（refno 复用旧世代行）；
  `pe_owner` 边改经 `ele_ops::pe_owner_edge`（`{ id: pe_owner:[owner, slot], in: child, out: owner }`，与 `cata_closure`
  逐字节同形）。`PersistElementChange::to_surql` / `render_terminal_persist_statements` 多一个 `&UdaKeyMap` 入参
  （`persist_latest_main_data` 与 `window_repair` 两个调用点各读一次表）。
  **护栏**：`ele_ops::terminal_render_guard_tests` 四条——ATT_UDA CONTENT 与 legacy `gen_sur_json_uda` 逐字节同、Modified
  清空删行 / Add 不发、查不到的 UKEY 记账不编链接、`pe_owner` INSERT 与 `cata_closure::render_pe_owner_replace` 同一份
  children 逐字节同且 owner 不出现在 `in` 端；`uda_keys::tests` 三条（精确键 / 回退 / 记账）。
  **验证（本轮实跑）**：`cargo check --lib --tests` 通过；`cargo test --lib -- ele_ops uda_keys direct_window_replay_parity`
  33 passed / 0 failed。**未 live**：已受影响的库（增量路写过反向边的窗口）要走一次 `POST /model/rebuild` 而不是手改数据；
  UDA 的 TTY 抽样对拍（方案 B8.5 第 5 条）等真库机器。

### 新增

- **e3d-io 池级元素缓存 `DbSet::with_element_cache()` + 读取计数 `e3d_io::read_stats`：生成期的瓶颈是重复读取，
  不是几何（ADR-059；`vendor/e3d-io/src/db_element.rs`、`src/read_stats.rs`、`vendor/e3d-model/src/bin/gen_ams.rs`）。**
  起因是给 e3d-model 的第一步计划做 W6「分段计时」，第一次跑就发现分档全压在一处：ams8000（6088 个设计元素、会话 264、
  五个目录库）总 6861ms 里，生成 6752ms，三角化 18ms、写 OBJ 16ms——而生成里 5955ms 是**读取**，88 万次记录解析 +
  79 万次描述符抽取，**每元素真读 145 遍**。重复不在遍历上：`DbSet::element` 每次造新句柄、`OnceLock` 是空的，于是
  `owner()` 的祖先链上溯与 `get_element()` 的目录链走查（`SPRE→SPCO→CATR→SCOM→GMRE`）把同一批祖先和同一批目录件
  反复查索引、反复解析；句柄级缓存只挡下 24%。
  **改动**：`DbSet` 加两张按 `RefNo` 索引的表（`Arc<ParsedElement>` / `Arc<ElementExtraction>` 分开，只导航的路径
  不为描述符抽取买单），**默认关**，`with_element_cache()` 才开；`add_db` 换库时按 dbno 清掉对应条目（换库=换时点，
  留着旧解析等于一直喂上一个会话的树，而且两边都不报错）；`read_stats` 把句柄命中 / 池级命中 / 真读分三档记，
  只给真读计时。`gen_ams` 开这个开关并打印分档计时。
  **代价**：开了就是无界、不淘汰——一次生成一棵子树是有界的，长驻进程按库全量读不是，那种场景该另建短命 `DbSet`。
  这条推翻了 `DbElement` 上原来那句「池级无界缓存是消费方的取舍，格式库不做」：跨句柄的重复发生在 `owner()` /
  `get_element()` 内部，消费方够不着那个位置，自己补不上。
  **验证（本轮实跑）**：同一条命令、同一个库，6861ms → **523ms（13.1×）**；读取 5955ms → 116ms（51×）；
  每元素真读 145 → 1.91 遍（11651 次 miss ≈ 6088 设计 + 约 5500 目录/引用元素，即每个不同元素读一遍）。
  `model.obj` / `elements.json` / `report.json` 三份 SHA-256 **逐字节一致**，账照旧平（visited 6088 + consumed 517 =
  索引 6605）。`e3d-io` `cargo test` 全绿；`e3d-model` `cargo test --lib` 201 绿、`--test noun_coverage` 7 绿。
  **未验证**：只跑了 ams8000 一个库（管道库目录链重，13× 是这一类的上界，结构库会低），`--test increment_real` 因
  并行会话正占着该 exe 链接不上（`LNK1104`），待补跑；gen-model 生产路径 `e3d_model_service` 尚未开这个开关。

- **模型增量类型限定 `model_incremental_types = ["BRAN", "EQUI"]`（环境变量 `AIOS_MODEL_INCREMENTAL_TYPES=BRAN,EQUI` 压过它）：
  只有名单内类型的生成根做模型增量，数据增量照旧（用户 2026-09-03 15:53 提出，会话 ff015543 落核心门，本轮补齐「有多大声」
  与文档）。** 不写 / 空表 = 不限类型，行为与引入前逐位相同；归一 trim + 大写 + 去空 + 去重，空白的环境变量当没设。
  **口径是消费门，不是计划门**：`model_update_pending` 的 drain select / claim / cohort mate claim / `count_pending_work` /
  `has_pending_work` / `/health` 状态查询共用一条 SQL 谓词 `render_automatic_type_filter`——
  `AND (action NOT IN ['regen_root','delete_cleanup'] OR (noun?:'') = '' OR string::uppercase(noun?:'') IN [...])`，
  与 `watch_dbnums` 的 `render_automatic_scope_filter` 同一条纪律（留在 SQL 里、不过滤解码后的行，两侧对「哪些行算数」
  答案唯一）。名单外类型的根**照常规划、照常入队、`gen_root` 照常标 stale**，只是本进程不认领：凭证前移
  （`render_credential_advance` 只放过 `ready ∧ desired == published`）因此不会给它盖新章，重启后 `reconcile_startup_window`
  把它当未安定行重排，名单一撤即照常消费——在计划层摘掉工作项反而是错的（不标 stale = 下一窗前移凭证 = 陈旧几何盖新章且永不
  回补）。数据增量（窗口收集、`pe` / `ATT_*` 写回、水位推进）、`Transform`（老规划器便宜路径、noun 空）、`CascadeExpand`、
  `PostRegenAabb`、房间任务、按需 `ensure_model_generated` 一律不看名单；noun 未知（空）的行不挡。与 `watch_dbnums`
  （按库、动数据面）、`delivery_unit_types`（决定根怎么选）三者互不代替。
  **改动**：新模块 `data_interface::model_type_scope`（对标 `watch_scope`：`Origin` / `resolved` / `admits` / `held_reason` /
  `mode_notice` / `window_plan_notice`）；`options.rs` 新键 `model_incremental_types` + `MODEL_INCREMENTAL_TYPES_ENV` +
  `model_incremental_types_resolved()`；`model_update_pending.rs` 六条语句接谓词、`ModelPendingStatus` 新增
  `held_by_model_incremental_types`（互补谓词 `render_automatic_type_holdback` 单独一条计数）。
  **有多大声（与 `watch_dbnums` 同一套护栏）**：`run_cli` 启动一句 + 初始化完成横幅一行 + python `full_init` 同句；`/health`
  新增 `model_incremental_types` / `model_incremental_types_origin` 两栏；preview / execute 回执 `warnings` 与调试 / 监听声明
  同批播种；数据窗口计划 `warnings` 按类型点名本窗暂扣的根（`dbnum=N: 本窗 K 个生成根因 model_incremental_types [...] 暂扣不生成
  （HANG×2, SUPPO×1；来自…）`）；worker 空闲轮模型门开着时首次一句（`Once`）。每一句都点名 `model_incremental_types` 与来源
  （配置 / 环境变量），措辞与监听限定、MDB 范围判定无交集。`DbOption.toml` 在 `watch_dbnums` 后补一段说明（默认注释掉）；
  `CONTEXT.md` 新增术语「模型增量类型限定」。
  **护栏**：`model_type_scope::tests` 七条（空名单放行一切 / 只放行成员且空 noun 不挡 / 纯谓词与进程级同口径 / 每一句点名来源 /
  措辞独立 / 计划暂扣只数受限两种行且名单外 / 窗口 warnings 一句的形状与静默条件）；`options::tests` 归一与环境变量覆盖；
  `model_update_pending::tests` 三条（六条语句都带谓词且与库域谓词共存 / `/health` 互补计数恰一条 / 空名单逐字不变）；
  `manual_update::tests::the_model_type_scope_announces_itself_in_both_receipts` 源码自检（声明排在第一条早退回执与任何业务
  告警之前）。
  **验证（本轮实跑）**：`cargo check --lib --tests` 与 `cargo check --manifest-path python/Cargo.toml` 通过；定向
  `cargo test --lib -- model_type_scope model_incremental_types type_scope announces_itself` **15 passed / 0 failed**，
  `options::tests` 三条模型类型用例 3 passed；全量 `cargo test --lib` **1422 passed / 4 failed / 101 ignored**——4 条全与本条
  无关：`the_embedded_panel_is_real_and_a_disk_copy_still_wins` 仍是 `web/ops.html` 丢 `id="verdict"`；
  `increment_manager::tests` 的 `an_unset_debug_scope_leaves_the_scope_verdict_untouched` /
  `handwritten_dbnum_lists_no_longer_narrow_the_increment_scope` / `the_debug_exclusion_never_speaks_with_the_scope_verdicts_voice`
  三条读的是工作树 `DbOption.toml`，而它此刻被 S5 live 验收开着 `watch_dbnums = [7998, 8000]`（`in_scope_with` 里的
  `watch_scope::admits` 把 1001 / 7997 挡下）；把 `DB_OPTION_FILE` 指回 HEAD 的 `DbOption.toml`（该行注释着）三条即绿。
  **未 live**：没有起过服务确认 `/health` 两栏与横幅实际输出；被暂扣的根在真库上「名单一撤即照常消费」的闭环靠 `gen_root`
  stale 标记推理，没跑过实例。

### 变更

- **几何 CPU 执行域落地（ADR-052 第 1/2/3/4 条的地基，specs/033 T006 完成、T007 部分完成），e3d 生成端的根级 CPU 段
  改成 `min(额度, 件数)` 个常驻 worker 从共享游标动态领取（用户 2026-09-04 在四条后续里选了这条）。**
  ADR-052 自 2026-08-27 接受起「实现未开始」：许可罩的是整个 `Future`（库查询、跨 `.await` 的锁、同步文件写都在一张
  许可之内），于是许可可以被「等库 / 等锁」的任务全部占住而 CPU 空着（背景第 1、2 条那个容量倒置）；而且「攥着许可去
  await 另一个过闸子任务」这条真正会死锁的写法在类型层挡不住。分块还是静态均分（`chunk_size` 只保证块数不超过额度，
  额度 16、17 件只切出 9 块），几何负载天然长尾，一块里摆两个重 BRAN 就把其余 worker 空在那儿。
  **改动（`src/fast_model/concurrency.rs`）**：新增 `run_gated_cpu(FnOnce() -> anyhow::Result<T>)`——只收同步闭包
  （闭包里写不出 `.await`，上面那条死锁写法在类型层就不成立），计算送进**私有** Rayon 池（`num_threads` = 闸额度，
  线程名 `geometry-cpu-N`；**不用全局 Rayon 池**，那个被 `room_model` / `db_model` 的 `par_iter` 共用、宽度不归闸管，
  接进来就是第二套并发口径）。许可在派发前取得、随闭包搬到 worker 线程上归还，池宽与额度相等故派发排队为零，
  于是这一段的持有时长就是 CPU 忙的时长。闭包 panic 就地 `catch_unwind` 转成错误——rayon 的默认处置是**整个进程
  abort**，一根坏构件不该把服务打死（去掉这层直接崩测试进程，实测 `STATUS_STACK_BUFFER_OVERRUN`）。
  新增 `resident_workers(jobs) = min(额度, 件数)`（第 4 条的宽度来源，件数为 0 时不派 worker）。
  `GeometryGate::acquire` 与 `GeometryPermit` 由 `pub` 收成模块私有（模块外零调用点），对外只剩 `run_gated_cpu`，
  调用方无法自行把许可作用域扩大到 CPU 段之外。
  **改动（`src/fast_model/e3d_model_service.rs`）**：上一条那个串行 for 循环换成**无栏的常驻 worker 池**。
  新增 `RootQueue`：整条根列表一份，`min(额度, 还能领的件数)` 个 worker 各持一张许可、各带一份 catalogue 缓存，
  从共享队列按索引领取，算完写回**自己那一格**；装配端只从队头按源顺序取，取走一格窗口就向前滑一格。
  **窗口（= `model_regen_execution_group` 根）是「无栏并行」与「内存有界」之间那根绳子**：worker 最多跑在装配端前面
  一个窗口，再多就把许可**还回去**（攥着许可等装配端正是 ADR-052 开篇那个容量倒置），装配端滑动窗口后再把它派回来。
  一开始写的是按 cohort 分波、波与波之间设栏，但那等于每个 cohort 都赔上其中最慢的那一根，而几何负载天然长尾；
  改成滑动窗口之后只有队头能挡住人。
  按源顺序装配不是装饰：切片边界与 `Required` 裁决（在哪一根 bail、哪一根记死信）因此与「谁先算完」无关，
  `geometry_workers = 1` 逐字节复现改前那条串行路径。缓存池跨窗口存活——缓存跟着一批死掉会丢掉「一次生产就是一个
  复用窗口」这条。`SourceIndex` 顺手改成 `Arc`（`GenerationInput` 与 `GeneratedMemoryBatch` 各持一份共享指针），
  否则每个 worker、每一片都要复制一遍整库的 `roots` 表。
  **回滚**：这条路径上 `geometry_workers = 1` 是**精确**回滚（一个 worker、一份缓存、按根原序），因为它不从额度派生
  任何别的东西——不用 `chunk_size`、不攒 SQL。ADR-052 第 6 条否掉「靠 geometry_workers 回滚」针对的是额度同时决定
  SQL 包大小的那些调用点，与此处不同。
  **护栏**：`concurrency::tests` 四条新增（并发峰值不超额度 / CPU 段跑在 `geometry-cpu-` 线程上而不是 tokio worker /
  panic 接住且许可归还、闸不被漏掉的一张堵住 / `resident_workers` 上下界）加一条
  `the_retired_future_wide_gate_gains_no_new_call_sites`（六个文件逐个点名计数，`spawn_gated_leaf` 只出不进）；
  `no_hardcoded_fanout_width_survives_in_fast_model` 的 spawn 探测加上 `run_gated_cpu(`，`indices.len() / <字面量>`
  单独放行并写明理由（那是「每三角形三个索引」的网格算术，此前靠「那些文件不 spawn」侥幸躲开）；
  `e3d_model_service::tests` 三条钉生产端——
  `generation_workers_claim_dynamically_and_never_share_one_catalogue_cache`（源码形状：宽度只来自
  `resident_workers(queue.claimable())`、CPU 段只进 `run_gated_cpu`、写回按领取序号落槽、缓存从池里借还，
  不许 `chunk_size(` / `tokio::spawn(` / `roots.chunks(`）、
  `the_generation_window_bounds_how_far_workers_may_run_ahead`（行为级：窗口满了不许再领；队头没好时先算完的那根
  不许插队；取走一格窗口滑一格）、`finished_roots_are_assembled_in_source_order_not_completion_order`
  （完成序 3/1/2/0，交出去必须是 0/1/2/3）。后两条是真跑 `RootQueue` 的并发不变式，不是扫源码。
  **验证（本轮实跑）**：`cargo check --lib --tests` 通过、无新警告；  `rustfmt --edition 2024 --check` 四个改动文件全干净；定向
  `cargo test --lib -- e3d_model_service concurrency` **58 passed / 0 failed**；
  全量 `cargo test --lib` **1437 passed / 4 failed / 101 ignored**，4 条仍是下面已记的同一批。
  **未做 / 未 live**：**T004/T005 的基线与可重复性证据没采**，所以本条不含任何加速比结论，也没跑真库；
  T008 的 `geometry_executor = "tokio_legacy" | "blocking_pool"` 开关没做；T011～T018 那七处 `spawn_gated_leaf`
  一处未动，因此 `active` 目前是**混读**（新路径读作 CPU 段，那七处仍读作「许可被占住多久」），模块头已写明
  ——叶子迁完前不得把利用率整体当 CPU 利用率写进 A/B 结论。ADR-052 第 5 条（SQL 攒批与额度解耦）也未做。
- **模型生成的内存批改成有界切片 + 深度 1 有界 channel，`generate_dbnum_in_memory` 那个「整库一次产进内存」的形态删除
  （用户 2026-09-04 14:2x 在四条后续里选了这条；收 ADR-041 §8）。** 生产/消费拆开之后，生产端一次把**调用方给的全部根**
  产进一个 `GeneratedMemoryBatch`，消费端再按 `model_regen_execution_group` 切块发布——一页 100 根就是 100 根的
  `GeneratedElement`（世界系 f64 CSG 实体，整条链上最胖的表示）同时在内存里，`generate_dbnum_in_memory` 对 7997 那 2720 个根
  一跑更是 GB 级。ADR-041 §8 把「无界」当否决方案写进了 ADR（“真不阻塞，代价是内存无界”），这正是它点名的形态。而且拆开
  之后两段是**先全产、再全发**，`(P+C)×N` 一分钟没省。
  **改动**：`generate_roots_in_memory` 换成私有的 `produce_slices(…, sink)`——按根遍历，攒够**任一预算**即封片交出：
  `model_regen_execution_group` 那一档根数（发布 cohort 宽度，与改前的事务边界同口径）或新键 `model_memory_slice_elements`
  那一档元素数（默认 8000）。只按根数算不是内存预算：一个 BRAN 和一个 EQUI 差两个数量级，「只有 16 根」的一页照样可以是
  进程里最肥的东西。单根不拆（根是发布的原子单位），所以真实峰值是「一份预算 + 一根」。新增公开入口
  `generate_and_persist_roots`：`tokio::sync::mpsc::channel(1)` 一条，两端用 `futures::future::join` 并跑（不是 `tokio::spawn`
  ——不需要 `Send`/`'static`，也不碰 `src/fast_model` 那条 fan-out 扫描线）；消费端出错先 `receiver.close()` 再返回，生产端下一次
  交片拿到 Err 就安静收工，让消费端那个错上浮。同时活着的最多三片（发布中 / 缓冲 / 在攒），不是整页。
  `model_refresh::generate_roots_report` 改调它；`generate_dbnum_in_memory` 与 `ModelRefreshPolicy::generate_roots_in_memory`
  两个无调用点的死接口一并删除（前者是被否决形态本身，后者是它的包装；全量 `generate_dbnum` 本来就是逐根流过
  `generate_refs`，不经内存批，已经是有界的）。
  **要知道的一处语义放松**：改前一次 `persist_memory_batch` 在**一把** `db_generation_lock` 里发完整页；现在每片各取一次锁、
  锁内各复核一次 `ensure_not_older_than_persisted`。重叠与「整页一把锁」是互斥的——锁跨整条流水线就等于把生成又关回锁里，
  正是上一条刚摘掉的东西。片间因此留了缝：同 dbnum 的另一个生成者可以插进来发布，之后本页剩下的片会带着陈旧源报错退回
  （这是正确结局，不是数据风险；根级 CAS 凭证与 `session_not_stale` 各自独立成立）。分块发布本来就不是原子的——改前
  chunk 3 失败时 chunk 1/2 也已提交——变的只是「不被别人插队」这一条。
  **护栏**：`e3d_model_service::tests` 两条新增——`the_generation_pipeline_hands_over_bounded_slices`（流水线函数体必须出现
  `mpsc::channel::<GeneratedMemoryBatch>(1)` 与 `futures::future::join(produce, consume)`；生产端必须同时读两条预算；
  生产代码段里不许再出现 `fn generate_dbnum_in_memory(`）、`a_slice_seals_on_either_budget_and_never_splits_a_root`
  （`MemorySlice` 行为级：空片不满、死信不占元素预算、到 cohort 宽度封片、单根撑破预算立刻封片且不拆根）。原有三条源码钉的
  切分标记跟着函数改名更新（`memory_producer_body` / `memory_consumer_body` 两个 helper，避免三处各写一遍）。
  **验证（本轮实跑）**：`cargo check --lib --tests` 通过、无新警告；`rustfmt --edition 2024 --check` 三个改动文件均干净；
  定向 `cargo test --lib -- memory_producer generation_pipeline slice_seals owner_index memory_generation_reads`
  5 passed / 0 failed；全量 `cargo test --lib` **1428 passed / 4 failed / 101 ignored**——4 条仍是下面那两条已记的同一批。
  **未 live**：没跑真库。8000 这个默认值是按「16 根 × 500 元素」推的，不是量出来的；重叠到底省了多少（`max(P,C)` 对
  `(P+C)`）也没实测——要量就在一页 100 根的 drain 上记生成段与发布段各自的墙钟。**没动的**：生产端仍是串行 for 循环，
  三角化与 `ensure_mesh_file` 还留在消费端；ADR-052 的 `run_gated_cpu` 未实现（specs/033 T006/T007 未勾）；发布粒度、
  生成前读合并、`inst_relate`/`tubi_relate` 索引三条仍在原处。
- **内存批的三处形态修正：owner 索引一批共享一份、单根硬错降级为死信、`db_generation_lock` 从生产端挪到消费端
  （接 2026-09-04 14:04 那轮「为什么需要加锁，我们都是只读」的审读结论；生产/消费拆分本身是上一批的改动）。**
  `generate_roots_in_memory` / `persist_memory_batch` 把模型生成拆成「先产进内存、再串行发布」之后，三个原本无害的写法
  变成了真问题：① `generate_snapshot_from_set` 给每个快照填 `owners: index.owners.clone()`——那是整个 dbnum 的 owner 表。
  拆分前快照在循环一轮内生并死，同一时刻只活一份；拆分后一页 100 根就是 100 份同一张表同时在内存里，而全仓拼语句读的
  一直是批自己带的 `&index.owners`（`generate_refs:722`、`apply_geometry_delta`），`snapshot.owners` 唯一的消费者是单根
  历史快照路径 `historical_model.rs:268`。② 生产端是 `generate_snapshot_from_set(...)?`——一个硬错误（读不动子树、
  `DbElementError`）把整个内存批抛掉，而旧的 `generate_refs` 在 `BestEffortFallback` 下是 `total.failed += 1` 后 `continue`；
  生产链路用的就是 `BestEffortFallback`（`model_refresh.rs:145`），这条直接撞 ADR-041 §6「单根失败记死信，不毁整片」，也撞
  commit `52ae62a5` 修过一次的现场：必现失败的坏构件把整页永久卡死。③ 生产端开头就取 `db_generation_lock(dbnum)`，而那
  一整段是纯读——这把锁护的是「`source_session` pin → `ensure_not_older_than_persisted` 单调判据 → 发布事务 → 写 `gen_root`
  凭证」这条写侧链，拿在读段只是把所有生产者串起来。
  **改动**：`SourceIndex.owners` 与 `GeneratedSnapshot.owners` 改成 `Arc<BTreeMap<(u32, u32), RefNo>>`（`scan_index` 建一份、
  `generate_snapshot_from_set` 只 `Arc::clone`；两处消费点靠 deref 强转，签名一行没改）；`GeneratedMemoryBatch` 新增
  `failures: Vec<(RefNo, String)>`，生产端 `Required` 之外的硬错误记一条死信并 `continue`，消费端开批时把它们逐条打日志并
  计进 `total.failed`（那些根保留上一次已发布的几何，调用方看到的 `failed` 不为零）；生产端不再取 `db_generation_lock`，
  消费端那把锁与锁内的 `ensure_not_older_than_persisted(batch.source_sesno)` 原样保留——批带出去的只是 pin 好的
  `source_sesno`，期间若有更新会话发布过，这一批就带着陈旧源报错退回。
  **护栏**：`e3d_model_service::tests` 两条——`the_memory_producer_neither_locks_publication_nor_drops_a_page_for_one_root`
  （生产端函数体不许出现 `db_generation_lock`、必须是 `match generate_snapshot_from_set(` 且有 `failures.push(`；消费端函数体
  必须同时出现 `db_generation_lock` / `ensure_not_older_than_persisted` / `batch.failures`）、
  `every_snapshot_in_a_batch_shares_one_owner_index`（两处字段声明都是 `Arc<BTreeMap<…>>`，快照构造只许 `Arc::clone(&index.owners)`、
  不许 `index.owners.clone()`）。既有的 `memory_generation_reads_only_the_e3d_source_projection` 未改，仍在盯生产端不读投影。
  **验证（本轮实跑）**：`cargo check --lib` 通过、无新警告；`rustfmt --edition 2024 --check src/fast_model/e3d_model_service.rs`
  干净（上一轮记的那两处既有红已不复现）；`cargo test --lib -- e3d_model_service` 34 passed、`-- model_refresh` 1 passed / 2 ignored、
  `-- historical` 4 passed、`-- spatial` 29 passed，均 0 failed；全量 `cargo test --lib` **1426 passed / 4 failed / 101 ignored**
  ——4 条与本条无关，就是上面「模型增量类型限定」那条已记的同一批（`the_embedded_panel_is_real_and_a_disk_copy_still_wins`
  缺 `id="verdict"`，`increment_manager` 三条读工作树 `DbOption.toml` 的 `watch_dbnums`）。
  **未 live**：没跑真库，owner 表省下的实际字节数没量（真值可从 `WindowRootSources.index_keys` 读），死信路径也没在真库上
  触发过。**没动的**：内存批仍然无界——`generate_dbnum_in_memory` 一次把整个 dbnum 的根全产进内存，正是 ADR-041 §8 明文否决
  的形态；按元素数 / 字节预算切片 + 深度 1 有界 channel 那一步没做。生产端仍是串行 for 循环，三角化与 `ensure_mesh_file`
  也还留在消费端（ADR-052 的 `run_gated_cpu` 至今未实现，specs/033 T006/T007 未勾）。
- **模型发布后的空间树收敛改回「提交后就地定向」，cohort 级整树重建永久删除（用户 2026-09-04 12:56 在甲/乙两案里选乙）。**
  上一版把 `generate_refs` / `apply_geometry_delta` 提交后的定向 `sync_tree_from_committed_pointers` 连同整树重建一起删了，
  换成进程内旗子 `MODEL_SPATIAL_REFRESH_PENDING` + `SpatialTreeState::ReplayRequired` 门禁，等 `drain` /
  `drain_data_phases_disposition` 三段队列全空再跑一次 `rebuild_tree_from_pointers()`。这个形态有三个洞：
  `side_effect_pending::promote_state_after_replay` 只看 durable `spatial_reconcile` 队列、不认识那个进程内旗子，
  而 `batch_worker::reconcile_spatial_pending` 每领一批都调它，于是树还是旧的就把 `ReplayRequired` 升成 `Ready`，
  房间轮拿陈旧树算归属；持续 SAVEWORK 流下 `drain_data_phases_disposition` 每次都在 regen 未空处提前返回，收尾刷新饥饿；
  `/model/ensure` 按需路径同样置旗，没有空闲轮的生产形态无人来刷。
  **改动**：`e3d_model_service` 恢复提交后就地 `sync_tree_from_committed_pointers(&spatial_refnos)`（`generate_refs` 与
  `apply_geometry_delta` 各一处，仍在 `lock_spatial_serial()` 之下、与同事务的 `render_spatial_epoch_bump()` 配对），
  紧随其后的 `rebuild_tree_from_pointers()` 永久删除——它当初是给「只删 legacy 行、不产 GeometryId 的根」兜底，而
  `pre_e3d_spatial_refnos` 早已把那些源键并进 `spatial_refnos`（`prepare_geometry_delta` 的 `report.removed += expected_count`
  保证 `persisted.removed > 0` 这个入集条件成立），整树扫描是冗余的；删掉 `MODEL_SPATIAL_REFRESH_PENDING` /
  `mark_model_spatial_refresh_pending()` / `refresh_spatial_tree_after_model_generation()` 与 `model_update_pending::drain`、
  `drain_data_phases_disposition` 的两处调用（后者尾部还原成原来的 `match aabb` 形），模型生成不再置
  `SpatialTreeState::ReplayRequired`。收敛代价从「每 cohort 一次 O(库) 指针全扫」回到 O(本次变更量)；整树重建只剩启动时
  epoch 失配的 `rebuild_at_startup_locked` 这一处兜底。
  **护栏**：`e3d_model_service::tests::publication_converges_the_spatial_tree_without_a_full_rebuild` 取代
  `generation_defers_spatial_tree_refresh_until_dbnum_completion`——`generate_refs` 与 `apply_geometry_delta` 两段函数体都
  必须出现 `sync_tree_from_committed_pointers`、都不许出现 `rebuild_tree_from_pointers`；
  `model_update_pending::tests::automatic_regen_and_aabb_drains_are_bounded_and_barriered` 去掉收尾刷新那两段序关系，
  还原成「regen 屏障在 AABB 之前」。
  **验证（本轮实跑）**：`cargo check --lib` 通过；`cargo test --lib -- spatial` 29 passed / 0 failed；
  `cargo test --lib -- e3d_model_service` 30 passed、`-- model_update_pending` 105 passed / 12 ignored、
  `-- aabb_tree` 21 passed / 1 ignored，均 0 failed。**未 live**：没跑真库，一页 100 根 drain 的三段耗时
  （生成前读 / 发布事务 / 树收敛）仍未实测。**没动的**：同一次审核提出的另外四条（发布粒度降到每根一事务、生成前读合并成
  cohort 级 1–2 次查询、`inst_relate` / `tubi_relate` 建索引、离散与哈希搬进 `spawn_blocking`）都还在原处。
- **数据读取模式默认 direct：`AIOS_DATA_READ_MODE` 不设即 direct，只有显式 `=db` 才退回 legacy 读路由（用户 2026-09-04 10:27
  拍板「AIOS_DATA_READ_MODE=direct 改成默认的，全部默认切换过来」；ADR-058 Q3 部分收口）。** 此前 `options::direct_read_mode()`
  只在环境变量恰等于 `direct` 时为真：生产早就跑 direct，而每条启动脚本、每次手起服务都得记着带这个变量，忘了就静默退回
  ADR-058 D8 已宣布退役的 db 形态——数据面改读 SurrealDB 的 `pe` / `ATT_*` 行、模型计划走 `pe` 打头的旧规划器，日志上只有
  一行「数据读取模式：db」可辨。
  **改动**：`options.rs` `direct_read_mode()` 改为「非显式 `db` 即 direct」（`parse_data_read_mode` 只认 `direct` / `db`，大小写与
  首尾空白不敏感）；新增 `data_read_mode_notice()`——设了却认不出的值（如 `dierct`）按未设置走 direct，但必须出声，`run_cli`
  与 python `full_init` 都打（与 `retired_net_window_notice` 同款：静默吃掉一个设了的开关是最高级别缺陷之一）；`run_cli` 的
  读模式播报与完成横幅改口：direct 标「默认形态」，db 变成 `***` 告警行，点名「由 AIOS_DATA_READ_MODE=db 显式选中、已退役、
  只留作退路」；`DbOption.toml` 在 `sync_live` 旁补一段说明（这个开关不在配置文件里，只认环境变量）。启动序列一行不动：
  读模式仍只回答「数据面读哪儿」，watcher / worker / 模型复核 / 房间与它无关，S5 护栏
  `direct_read_mode_only_selects_the_data_read_route` 原样绿。
  **护栏**：`options::tests::direct_is_the_default_read_mode_and_db_is_the_explicit_legacy_exit`（不设 / `direct` / ` DIRECT ` /
  空串 / 拼错 → direct；`db` / ` Db ` → db）、`an_unrecognised_read_mode_value_is_announced_not_swallowed`（拼错出声且提示
  `=db` 退路；没设 / 设对 / 空白不吵）。
  **验证（本轮实跑）**：`rustfmt --check` `options.rs` / `lib.rs` 通过；`cargo check --lib --tests` 通过；全量 `cargo test --lib`
  **1417 passed / 1 failed / 101 ignored**——那 1 条 `the_embedded_panel_is_real_and_a_disk_copy_still_wins` 仍是另一会话半改
  `web/ops.html` 丢了 `id="verdict"` 锚点，与本条无关。**未 live**：没有起过服务确认不带环境变量时横幅打
  「direct / e3d-io（默认形态）」。**没动的**：S5-4 db 形态整体退役 / git tag——`=db` 这条退路与 `sync_pdms` legacy 分支保留到
  那一步一起收；`.github/context-cold.md` 是生成物，不手改。
- **启动形态合一（ADR-058 D8 / refactor plan S5-1～3）：direct 形态也起 watcher，`direct_read_mode` 在启动序列里只回答
  「数据面读哪儿」。** 起因是 2026-09-04 对增量更新启动流程的审核（`docs/plans/2026-09-04-startup-increment-audit-and-next-plan.md`
  #1 / #3 / #14）：`sync_live = configured_sync_live && !direct_read_mode` 让生产形态启动后没有数据链、没有 manifest、
  没有文件事件，模型只靠人工执行与按需 `ensure`，「改元素 → SAVEWORK → 自动更新」做不到；direct 分支又在复核落地后
  立刻 `mark_model_ready()`，`/health.model_ready=true` 时复核排出的行还在跑，并跳过启动房间重建。
  **改动（全部在 `src/lib.rs` `run_cli`）**：`let sync_live = db_option.sync_live.unwrap_or(false);`（watcher 只听这一个开关）；
  删 direct 专用的「开门 + 立刻宣告收敛 + 跳过房间」两处分支，direct 与 db 形态同走 `wait_for_data_ready → open_model_phase →
  wait_for_model_ready → skip_startup_room_build 三门`，收敛由 worker 空闲轮宣告；direct 下 `total_sync / incr_sync /
  only_sync_sys / sync_history` 不再静默忽略，打一行「direct 形态不跑 sync_pdms…」（legacy `sync_pdms` 分支本身不动）；
  播报与完成横幅的 direct 标签改口。护栏 `direct_mode_starts_the_batch_worker_but_not_the_watcher` 翻成
  `direct_read_mode_only_selects_the_data_read_route`（worker 之前 `direct_read_mode` 只许在绑定 / 一行播报 / 两条 legacy
  sync 门四行出现，worker 之后到 `run_cli` 结束零出现，主线无 `mark_model_ready()`）。
  **验证**：`rustfmt --check`（仅 `lib.rs`）/ `cargo check --lib --tests` 通过；
  `cargo test --lib -- startup direct_read_mode initialization model_coverage` 59 passed / 0 failed / 3 ignored；
  全量 `cargo test --lib` 1414 passed / 1 failed / 103 ignored——那 1 条 `the_embedded_panel_is_real_and_a_disk_copy_still_wins`
  是另一会话同时在改的 `web/ops.html` 丢了 `id="verdict"` 锚点，与本条无关。**未 live**：direct + watcher 第一次面对真实文件事件流，按 refactor plan §4 风险行
  先在 `watch_dbnums` 限定到一两个库下起一次；direct 从此也走 `wait_for_data_ready`，它没有超时退出口（审核 #10）。
  **未做**：S5-4 db 形态退役 / git tag。同日回填 refactor plan S2 状态（`/health` 半边已落地）与 ADR-058 状态行。

### 移除

- **old-pdms-io 连「对拍参照臂」这个身份也摘了：`pdms_io` 从整个工作区退出，对拍以 E3D TTY 的数据为准
  （用户 2026-09-03 23:57 拍板「对拍要以 e3d tty 的数据为准，删除和 pdms-io 的对拍」）。** 09-03 把它降成 legacy 时
  留了一句「对拍尺子要活到两边合一为止」（退役清单 §2.H）——今天把这句话撤回：判一个窗口收得对不对的，是 E3D TTY
  真做过的事（TTY 宏录下的会话链 / 现场 `Q ATT` 读数），不是另一套解析器；old-pdms-io 自己就是 09-02 那 22 条漏
  Add、两条幽灵删除的出处，拿它当尺子只会把尺子的缺陷判成被测方的分歧。
  **删掉的**：`Cargo.toml` 的 `legacy_pdms_io` / `debug_parse` / `legacy_session_replay` 三个 feature、`pdms_io`
  optional 依赖、old-pdms-io 的 `[patch]`（`Toggle-LocalDeps.ps1` 从此只管 aios_core / parse_pdms_db 两仓）；
  bin `legacy_v2_read_parity` / `incr_fold_probe` / `db8000_copy_probe`；`increment_planner_parity --probe`
  （两读法交叉点查）；`ele_ops::legacy`（`From<pdms_io::io::*>` 桥）；`IncrementPipeline::collect_changes` /
  `collect_changes_from_open_io` 与三条只为回放存在的终态补丁 `retain_finally_live_adds` /
  `restore_finally_live_deletes` / `final_record_payload`（连它们的 4 条单测）；`increment_pipeline` 的两条 live
  对拍（`live_ams8000_net_diff_agrees_with_point_lookups_and_covers_replay` /
  `…_payloads_match_replay_on_single_touch_refnos`）、`folding_a_real_window_preserves_final_state`、`bench_tests`
  整个模块、`live_real_ftub_delete_move_and_reorder`（它的核心断言「保留 Add→Deleted 序列」是逐会话回放的前提，净口径
  按设计丢弃瞬态 refno，ADR-031）；`model_update_plan` 的 `live_projams_real_attribute_sessions_plan_and_execute_distinctly`
  （多会话窗口按「每会话一条」写死期望，同理）；`tests/db8000_session_pairs` 性质 h / i；aios-py 的
  `collect_changes` / `net_changes` / `element` / `attmap` / `subtree` / `generate_*` / `noun_dict` 绑定及其
  `legacy_pdms_io` feature（`parse_pdms_db` 随之从 python crate 退出）与对应离线用例。
  **改口径而不是删的**（TTY 会话本来就是它们的判据，只是收集换走生产入口 `collect_window`，均为 `#[ignore]`
  手动 live、**改后未复跑**）：`live_real_delete_session_cleans_up_model_and_regenerates_branch`（单会话窗口）、
  `live_projams_nested_created_routes_and_generates_delivery_roots`（单会话窗口）、
  `live_ams8000_single_caliber_release_timing`（只剩生产臂计时）。python 离线用例把 `net_changes` 那三条判据
  （净三态、单会话窗口 base 落前一会话、越界响亮报错）搬到 `net_window` 上，另加「分区键 = TTY 动它的那个会话」。
  **护栏收紧**：`data_interface::mod.rs::legacy_pdms_io_guard` 从「`pdms_io::` 必须在 legacy 门后」改成两条
  「manifest 与源码一处都不许有」；`scripts/Check-LegacyPdmsIo.ps1` 同款四判（工作区依赖图 / `-i pdms_io` 无包 /
  两份 manifest / 三处源码），CI 新增一步跑它，`db8000_two_delete_fixture` / `db8000_session_pairs` 的 CI 命令去掉
  `legacy_session_replay`。
  **验证（本轮实跑）**：`cargo check --lib --bins --tests` 默认档通过（只剩既有 2 条 warning）；
  `cargo check -p aios-py --all-targets` 通过；`cargo test --lib -- legacy_pdms_io_guard` 2 passed；
  `cargo test --test db8000_session_pairs` **19 passed**（性质 a–g 全绿，夹具即 TTY 会话链）；其余见本条下方补记。
  **没做**：`parse_pdms_db` 仍是 aios-database 的依赖（清单 §6，另一会话在摘）；T404 第三条 429 库行级对拍与 T416 现场不变。
  **补记（同日，开发脚本清理）**：`aios_db.parse.element` 以 e3d-io 底座**重生**——新增
  `e3d_collector::read_element_at(frozen, refno)`（`open_pinned_set` → `read_element`，与生产收集器同一条读法，
  不查库不动水位），绑定返回 `{refno, sesno, noun_hash, noun, name, owner, children, attrs, explicit_attrs}`
  （`sesno` 是钉住的会话，old-pdms-io 版的 `found_sesno` 语义不再有；`attrs` 里 POS 是 `{"Vec3Type": [...]}`
  而非老版的 `F32VecType`）。靠它活着的 TTY 往返门 `scripts/e3d/Test-TtyNetWindow.ps1` 只改了 `pos()`
  取值方式（按标签取值、不认死标签名），**未在 E3D 现场复跑**。开发脚本：`python/testbed/net_changes_probe.py`
  （old-pdms-io 两口径对拍）与 `python/scripts/demo_noun_caps.py`（`parse.noun_dict`）删除；
  `testbed/run_full_loop.py` / `testbed/spatial_tree_8000.py` / `scripts/smoke_m1.py` / `scripts/smoke_m4.py` /
  `scripts/demo_element_diff.py` 的 `collect_changes` 改走 `net_window`（`spatial_tree_8000` 的血统判定不再按
  「25 删子、26 删父」分区，改判两条 deleted 都在净窗口里——墓碑挂窗口终点是净口径的语义），`smoke_m4` 去掉
  `noun_dict` 段。python 离线用例加两条 `element` 用例（baseline 读得到 BOX、final 读不到、钉回 24 读回同一份属性）：
  `pytest -m offline` **12 passed**。

- **`parse_pdms_db` 整体退出本仓：基线全量解析换成 e3d-io 的退化窗口，两条数据路从此一份收集、一份渲染
  （退役收口方案 `docs/plans/2026-09-04-legacy-parser-retirement-endgame-plan.md` B8.2–B9，Q1 = A「整体删」，
  用户 2026-09-04 08:44 拍板）。** 这条同时结掉上一条「old-pdms-io 退出」里那句「**没做**：`parse_pdms_db` 仍是
  aios-database 的依赖」——它就是 old-pdms-io 那个仓留在依赖图里的最后一条尾巴（`parse_pdms_db → pdmsdb_engine_v2`，
  git = `pdms-io.git`），本条一起断。
  **基线换底座（B8.2–B8.4）**：新收集器 `e3d_collector::stream_baseline`——把基线当成 base 为空、target = 冻结目标
  会话的**退化窗口**，与 `collect_net_window` 共用 `open_frozen_target / enumerate_target_index / read_element /
  allowed_ignored_final`（源码钉 `the_baseline_and_the_net_window_share_one_enumeration`），每 `sync_chunk_size`
  个元素交一块、块交出去就释放，峰值内存随块不随库；新模块 `versioned_db::baseline::render_baseline_batches` 把一块
  渲成写者消息，行形状（`pe` / `ATT_<noun>` / `ATT_UDA` / `pe_owner`）与增量路共用 `ele_ops::ElementRows`。
  `database.rs` 的解析段整段换成 `FrozenSource::capture → UdaKeyMap::load → spawn_blocking(stream_baseline →
  render_baseline_batches → sender.send)`，`sync_total_async_threaded` 的签名与六个调用点（`database.rs:729 / 768 /
  817 / 830`、`manual_update.rs` 两处）一个未动。**与方案写法不同的两处，理由记这里**：① 写路**保留** `SenderJsonsData`
  三种消息与 `INSERT IGNORE` 批，没有改成逐行 `UPSERT`——`manual_update` 的父层补缺把 extract 母库整库再同步进同一个
  dbnum（`replace_dbs = false`），靠的就是「已有行赢」，叶子 extract 里正在改的版本不许被母库旧版本覆盖，`UPSERT` 会把
  这条语义改掉（B8.5 第 4 条那个「逐行超基线 1.5× 就加批量写者」的触发对象因此不存在，该条改成单纯量耗时与峰值内存）；
  ② WORL 按普通元素写全三样行（legacy 只写 `ATT_WORL` + `pe_owner`，不写它的 `pe` 行），`BaselineStats::elements`
  不计它，形状 diff 的「预期差」要加这一条。
  **删掉的**：`Cargo.toml` 的 `parse_pdms_db` 依赖、old-parse-pdms-db 的 `[patch]`、`legacy_parse_pdms_db` feature
  与它那段已经过期的注释；8 个取证探针 bin（`attr_info_compat_probe` / `cata_closure_probe` / `cata_parse_probe` /
  `curd_parse_probe` / `noun_layout_parse_probe` / `refno_attr_probe` / `refno_index_probe` / `world_probe`）与 2 个
  门后集成测试（`tests/full_attributes_real.rs` / `tests/pdms_record_boundary.rs`）；`ele_data.rs` 的
  `From<legacy EleData>`（`LIB_ALLOWLIST` 随之空掉）；`versioned_db/member_prune.rs`（成员事后裁——目标索引差分本身
  就是终态）与 `versioned_db/pe.rs`（`save_pes` / `save_pes_mysql` / `save_pe_relates`，整个文件即这三个函数，上一条
  B8.1 的取证链里点到的 `pe.rs::save_pe_relates` 指的就是它）；`read_full_basic_data` / `legacy_session_ranges` /
  `preserve_unparsed_pe_metadata`，以及唯一调用方随之消失的 `FrozenSource::read_frozen_bytes`（基线不再把冻结前缀
  整个读成一个 `Vec<u8>`）。`Cargo.lock` 少 5 个包（`parse_pdms_db`、`pdmsdb_engine_v2`、`dynfmt`、`hash32-derive`、
  `erased-serde 0.3.31`），**只少不多**——唯一一行 `+` 是 `erased-serde 0.4.6` 去掉重名消歧后的 `"erased-serde"`。
  **护栏收紧**：`data_interface::mod.rs::legacy_parse_pdms_db_guard` 从「必须在 legacy 门后」翻成「manifest 与源码
  一处都不许有」，另加 `the_baseline_orchestration_has_no_legacy_parser_intermediates`（`DbBasicData` / `member_prune`
  不许回到 `database.rs`）；`scripts/Check-LegacyParsePdmsDb.ps1` 改成依赖图 / `-i parse_pdms_db` 无包 / 两份 manifest /
  扫描器自检 / 源码扫描 / 基线无 legacy 中间态六判，CI（`windows-tests.yml`）新增一步跑它；`Toggle-LocalDeps.ps1` 只剩 aios_core，
  `Assert-NoLocalPatch.ps1` 白名单去掉 `parse_pdms_db` / `pdms_io`。**顺手补一个洞**：`Check-LegacyPdmsIo.ps1` 第 1 条
  原先只扫下划线的 `pdms_io`，而 `pdmsdb_engine_v2 v0.1.0 (…/pdms-io.git?rev=…)` 这一行两个字符串都不匹配——
  上面那条尾巴就是这么在「四判全绿」下挂了一整天的；判据改成同时认连字符 `pdms-io`（本轮反例验证：旧模式漏、新模式抓）。
  **文档**：`CONTEXT.md`「分块解析基线」词条按新底座重写（退化窗口 / 无 `mem://` 暂存实例 / 无 `member_prune` /
  `pe.sesno` 从链头 0 变成真实 last-touch 会话），并从「已退役词条」小节下面挪回现行词条区；新增「pe_owner 边」词条
  （`in = child, out = owner`，读侧与 `unique_pe_owner (in, out)` 索引定权威）。
  **验证（本轮实跑）**：`cargo check --workspace --all-targets --locked` 通过（只剩 3 条既有 warning）；
  `cargo test --locked --lib -- legacy_parse_pdms_db_guard legacy_pdms_io_guard ele_ops e3d_collector versioned_db
  uda_keys baseline manual_update noun_dict db_file_header frozen_source` **195 passed / 0 failed / 12 ignored**；
  `cargo test --locked --test baseline_stream_8000` **6 passed**（issue-019 db8000 归档夹具，`CHUNK_SIZE = 64`）；
  `Check-LegacyParsePdmsDb.ps1` 六判、`Check-LegacyPdmsIo.ps1` 四判全绿；
  `cargo tree --workspace -e normal --locked -i parse_pdms_db`、`-i pdmsdb_engine_v2` 与 `-i pdms_io` 三条均报
  `did not match any packages`。
  **`Assert-NoLocalPatch.ps1` 这一轮是红的，与本条无关，也没有顺手关掉**：`aios_core` 的 LOCAL-DEPS PATCH 在
  `60691756` 及更早都是注释态，是上一条提交（`014ddb0b`）把生效态的 `[patch]` 连同去掉 `source` 的 `Cargo.lock`
  一起提了进去。关掉它要让 cargo 把 `aios_core` 重新解析回 git rev，而此刻同一棵工作树上另有一条会话正拿
  `../vendor/old-aios-core` 编译——那等于在别人脚下换掉依赖来源。留作单独一条提交。
  **真库进度（B8.5，前几轮实跑，见方案 §0.2）**：`ams7997_0001` 由 e3d-io 完成基线（`170048` 行 `pe` / `dbnum_info`，
  文件最新会话与应用水位同为 `106`，排入 `18675` 个生成根）；SYS/DICT meta 阶段完成（SYST 8191 写入 `1205` 行 `pe`，
  `DTWL` 这种 `attribute_count = 0` 的合法零属性 noun 返回空属性提取）。
  **未做 / 未验证**：B8.5 第 3–6 条的外部裁决——5 库形状 diff、448 库耗时与峰值内存、E3D TTY `Q ATT` / `Q MEMBERS`
  抽样、`live_ams8000_*` 回归；**TTY 值 / 成员序对照完成之前不宣称外部等价**（当前实跑只证明 e3d-io 自洽与持久化闭合）。
  另：`docs/adr/ADR-004`、`ADR-021`、`docs/plans/on-demand-cata-parsing-port.md`、`docs/diagrams/data-parse-pipeline.drawio`
  与 `.github/context-cold.md` 里指向已删的 `pe.rs` / `save_pes` 的取证引用**没动**——ADR 是历史决策记录，改写它们要单独决定。
  **（用户当日拍板改写，见下条。）**

- **悬空取证引用重指：ADR-004 / ADR-021 / 数据解析流程图**（用户 2026-09-04 指定这三处）。改的一律是**指针不是决策**：
  三份文件的结论、不变量与状态行一个字未动，动的只是「拿哪段代码作证」。
  - `ADR-004`：关联行删掉 `src/versioned_db/pe.rs` 与 `vendor/aios-parse-pdms/src/parse.rs`（后者不被任何 manifest
    引用、也不在 git 里，是本机残留的旧解析器副本，拿它当取证入口会指错人），换成 `data_interface/ele_ops.rs`；
    背景段与 Q3 就地标注哪几个名字已不在仓里；文末新增「2026-09-04 · 取证引用重指」一张对照表（`pe.rs` →
    `ele_ops::ElementRows::render`（基线与增量共用一份，宪法 II）；`save_pes` → `baseline::render_baseline_batches`，
    Q3 的动词与落点不变；`parse_file_db_basic_data` / `refno_table_map` → **无对应物，是被取消不是被改名**，
    改走 `FrozenSource::capture` + `stream_baseline`；`parse_db_basic_info` → `db_file_header::DbBasicInfo::read`）。
    按需 CATA 闭包那条路不经过基线全量解析，本 ADR 决策面一处未受影响。
  - `ADR-021` §4：`pe_owner` 边 id 形状 `[OWNER_PE, 槽位]` 的两个「同形写口」证据由 `versioned_db/pe.rs:174` 与
    `cata_closure.rs:1574`（前者已删、后者行号已漂）重指为 `ele_ops::pe_owner_edge` 与
    `cata_closure::render_pe_owner_replace`，并**把举证从行号换成可执行守护**——单测
    `pe_owner_edges_point_child_to_owner_exactly_like_cata_closure` 对两处输出逐字节 `assert_eq!`（宪法 VI）。
  - `docs/diagrams/data-parse-pipeline.drawio`：`save_pes` / `save_pe_relates` 两个节点重写成今天的
    `render_baseline_batches` / `ElementRows.owner_edges` → `SenderJsonsData` → `INSERT IGNORE`。
    **顺带查出这张图的腐坏远不止 `pe.rs`**：逐标识符核对后，图中 193 个代码标识符有 **58 个不同的名字在 `src/` 里
    已查无此名**；横幅上「页 1 十二个、页 2 四十二个、页 3 二十六个」是**按页各自计数**，合计 80 处而不是 58——
    `DuckLake` / `hierarchy_release` / `hierarchy_node` / `release_id` / `locator_hash` 这一族跨页反复出现，
    去重后才是 58（两个数都对，别把 12+42+26≠58 当成笔误；换一套分词器独立重数是 61 个 / 按页 81 处，差在
    要不要把 `db_basic` 这类两字段名算作标识符，结论不变）。整条 **DuckLake 层级投影 / locator 三段发布**支路
    （`HierarchyRowV1`、`BaselineProjection`、门 A/B/C、`hierarchy_release` 一族）连同 DuckLake 本身早已移除——
    这张图与它的导出图是**全仓最后一处还在描述它**的地方。改一半会让没标注的节点显得可信，因此三页各加一条
    带日期与计数的过期横幅、标题加「（历史形态）」，并**重新导出 3 SVG / 3 PNG（scale 2）/ PDF**——横幅只写进
    `.drawio` 而读图的人看的是导出图，等于没写（宪法 III）。**整图重画尚未做，待定。**
  - `docs/plans/on-demand-cata-parsing-port.md`（4 处；这条原先记的是「未动、不在本次指定范围」，当日已一并改完，
    此处按事实改回）：「依据」清单删掉 `src/versioned_db/pe.rs` 与 `vendor/aios-parse-pdms/src/parse.rs`、
    换成 `data_interface/ele_ops.rs`；文首加一张与 ADR-004 同口径的重指表；阶段 0 与「文件清单」两处就地划掉
    失效路径；「关键适配」段补一条——`pe_owner` 的「对齐 `save_pe_relates`」从「照着抄」升级成单测钉住同形。
    **决策、阶段状态与实施记录一个字未改。**
  - `.github/context-cold.md`（3 处）**没手改，也已经干净**：它是 SigMap `gen-context.js` 的生成物、标着
    DO NOT EDIT，那 3 处就是 `### src\versioned_db\pe.rs` 这个标题下的 `save_pes` / `save_pes_mysql` /
    `save_pe_relates` 三条签名；文件重新生成（v8.22.0 → v8.28.1）后整个标题段随源文件一起消失。
    **生成物就该走重新生成这条路**——手改它下一次生成就被冲掉，等于没改。

## 2026-09-03

### 变更

- **old-pdms-io 设为 legacy：默认不启用、不编译；数据面解析全部换到 e3d-io，模型面继续 e3d-model
  （用户 2026-09-03 拍板；`docs/plans/2026-09-03-old-pdms-io-retirement-inventory.md` §0 三条完成判据今天全部成立）。**
  `Cargo.toml` 里 `pdms_io` 改 `optional = true`，只由 `legacy_pdms_io`（`debug_parse` / `legacy_session_replay`
  叠在其上）打开；`cargo tree -e normal` 默认图里再无 `pdms_io`（`scripts/Check-LegacyPdmsIo.ps1` 三条护栏 +
  `data_interface::mod.rs::legacy_pdms_io_guard` 源码钉）。**数据面按 API 面逐组换底座**：
  ① 类型与渲染 `EleOperationData / EleOperationDetail / ModifiedElement` 及 `to_surql / to_modify_surql /
  diff_ele_data` 等纯函数**原样搬入** `data_interface/ele_ops.rs`（约 1400 行连测试），`pe` / `ATT_*` 语句逐字节不变；
  ② 收集器换成 `data_interface/e3d_collector.rs`：`IndexDiff(base@S, target@T)` + 两端 `to_named_attmap` →
  `diff_ele_data`，每 refno 恰一条、挂 last-touch 会话；refno 复用（换 noun）按 `Add` 写终稿并在尾事务
  `DELETE {old_noun}:{id}`（`RetiredTypeRow`，关掉清单 §8 第 2 条今天就有的残留漏洞）；`shape_conflicts` 非空整窗阻断；
  **不再做 ADR-036 成员仲裁补删**（索引差分即终态，`membership_deleted` 恒 0，evidence 09-02 §3 坐实旧仲裁幻删）；
  ③ 身份门 `SnapshotToken` / `DabaconSnapshot::open_verified*` → `data_interface/frozen_source.rs::FrozenSource`
  （Windows 卷序列号 + 文件索引，nightly `windows_by_handle`；`capture / capture_at / reopen_verified /
  reopen_verified_at / verify_path_identity / read_frozen_bytes`），`apply_one` 无条件复核、尾事务前再复核一次；
  T 端存活复核改 `e3d_collector::contains_refnos_at`（仍只给老规划器）；
  ④ 文件头 `DbPageBasicInfo` / `PdmsIO` → `data_interface/db_file_header.rs`（`DbFileHeader` / `SessionStamp` /
  `latest_sesno` / `session_at` / `session_chain`，会话时刻与旧 `get_dt` 同一条解法；`legacy_session_ranges` 逐字复刻旧
  `ses_range_map` 喂基线全量解析），`apply*` / `SesnoUpdatePlan` 签名随之换；
  ⑤ `PdmsWatcher` → `data_interface/db_file_registry.rs::DbFileRegistry`——核对发现它在本仓只是状态壳
  （`init_local_watcher` / `async_watcher` / `scan_db_headers` 零调用，真 watcher 一直是 `increment_manager::async_watch`
  的 `notify::PollWatcher`），所以**清单 §0.1 第 1 条「在 e3d-io 侧新写 watcher」实际无事可写**，按事实修正；
  ⑥ `sync::clone / compress` 原样搬入 `data_interface/file_sync/`（`dpcsync` / `blake2` / `reqwest` / `url` 直列，
  `atty` 换 `std::io::IsTerminal`）；⑦ 基线全量解析（`sync_total_async_threaded`）改 `FrozenSource::read_frozen_bytes`
  + `parse_pdms_db::parse_db_basic_data`（`parse_pdms_db` 留到清单阶段 8）；`window_repair` 的成员审计收成
  「持久层孤儿 ∧ 冻结目标索引里不存在 → Deleted」。**门后**：`increment_planner_parity --probe`、
  `legacy_v2_read_parity`（`required-features`）、`db8000_session_pairs` 性质 h / i、`increment_pipeline` 三条 live 对拍
  进 `legacy_pdms_io`；`ele_ops::legacy` 提供 `From<pdms_io::io::*>` 桥让参照臂继续对拍。T401 影子收集器保留，从此
  是「同一根索引差分上两条合成路径互查」（`e3d_collector` vs e3d-model L2）。`cargo check --lib` 默认 / `legacy_session_replay`
  两档通过；`cargo tree` 护栏通过。**未做**：T404 三条硬门与 429 库行级对拍没在切换前跑（用户要求直接切换）；
  `db8000_session_pairs` 默认档实跑见本条下方补记。
  **补记（当日晚，切换后补跑 T404 前两条）**：在生产收集入口 `IncrementPipeline::collect_window` →
  `e3d_collector::collect_net_window` 上，ams7999 45→46 出 **22 Add**（`24383/101198`–`101219` 连号子树：
  STRU/FRMW/SCTN/PANE/PLOO/PAVE）+ 1 Modified（ZONE `24383/68481` 只变 `MEMBERS`）+ **0 Delete**——
  老收集器在这一窗幽灵删的 `24383/72318` / `72319` 与 ghost key `24383/101122` 现在**一条删除操作都不产生**；
  ams1112 721→722（老收集器整窗报错的那一窗）出 **24673 Delete** + 1 Modified，353 ms、读页 638。
  两窗与 e3d-model L2 臂逐 refno 零分歧（硬 0 软 0），期望写在命令行上、不满足退出码 1，含负向对照。
  三个 ams8000 窗口同路复跑也零差，且 T401 那几条 `CACHID` **软**分歧随之清零——代价是 255→256 的
  PANE `24384/26250`（只变 `CACHID`）从「原样重写跳过」变成一条真的生产 `Modified`，`ATT_*` 会多这一列的写
  （退役清单 §8 ⑤ 说的回执重定基线，别当回归）。
  `shadow_collector_parity` 第一臂的标签随之改回事实（`[prod]`，切换后它就是 e3d_collector），
  并补上判生产臂的 `--expect-prod-add/-delete/-modified`，`--must-not-delete` 改成两臂都判。
  evidence `docs/evidence/2026-09-03-t404-production-gate.md`。**仍未做**：T404 第三条 429 库全量基线行级对拍
  （渲染层，归 T406）与落库 / 水位 / 批次编排的 live 现场（T416）。

### 新增

- **T401 影子收集器落地：old-pdms-io 净窗口旁并行跑 e3d-io 索引差分 + e3d-model L2，同窗逐 refno 对拍，只记账
  （spec 035 P4 的长杆；用户 2026-09-03 拍板「先开长杆」）。** 起因是核对「解析是否已切到 e3d-io」：模型面已全走
  e3d-io，数据面（`collect_window` → `pe` / `ATT_*`）仍是 old-pdms-io，且 direct DESI 窗口**同一窗读两遍**
  （退役清单 F4）。新模块 `data_interface/shadow_collector.rs`：把 `IndexDiff(base@S, target@T)` 的候选折成
  「每 refno 一条 `Add / Deleted / Modified`」（`Modified` 候选过 `diff_element`，逐字节相同记 `unchanged` 不算操作；
  读不出记 `unresolved`；候选账 `ops + unchanged + unresolved = candidates` 必须闭合），与 old-pdms-io 的
  `range_eles` 逐 refno 比**种类**、`Modified` 再比**属性名集**；分歧沿 09-02 planner-parity evidence §3/§4 的词归桶——
  硬桶 `phantom_delete`（old 删了、e3d 说 T 端还在）/ `ghost_key`（两端都没有）/ `missed_insert|delete|modify`
  （e3d 有、old 无）/ `old_only_*` / `l2_unchanged` / `kind_mismatch` / `shadow_unresolved`，软桶 `attr_set_differs`、
  `missed_modify_data_only`（e3d 只看见 `CACHID` 一类 DataOnly 属性变了）。**生产接线骑在双读的 e3d 半边上**：
  `apply_one` 在 direct DESI 分支把刚收好的 old 窗口折成 `LegacyWindowOps` 交给
  `build_model_update_plan_from_window(.., Some(legacy))`，`WindowRootSources::collect_shadowed` 用同一次
  `collect_window` 的 `WindowDiff` 折影子窗口——**不开第三次文件**；对拍在阻塞任务里做（`phantom_delete` 归因要对两端
  `DbSet` 点查），结果只有两条出口：回执 `warnings` 一行、`logs/shadow-collector-<日期>.jsonl` 一条（`report_parity`）。
  不改计划、不拦窗口、不动水位；启动复核路传 `None` 不开影子。离线探针 `src/bin/shadow_collector_parity.rs`
  （`--expect-shadow-add/-delete/-modified`、`--must-not-delete`、`--expect-clean` 不满足退出码 1，JSON 明细按
  `--json-rows-cap` 截断）。**五窗 evidence** `docs/evidence/2026-09-03-shadow-collector-parity.md`：ams8000 三窗硬分歧 0
  （软分歧全是 e3d 多报 CACHID）；ams7999 45→46 e3d 出 22 Add / 0 Delete，old 侧 `phantom_delete`=2（REST 72318 /
  HANG 72319）`ghost_key`=1（101122）`missed_insert`=22（STRU 101198 子树）；ams1112 721→722 old 整窗报错
  （vendor 前进后死法变成「记录页 11526 无法反查 last-touch 会话」）、e3d 655 ms 出 24673 Delete——**T404 前两条硬门
  在 e3d 侧已量得出且通过**。T403「零差」口径随之写进 tasks.md：要为零的是反方向四桶（`old_only_*` / `l2_unchanged` /
  `kind_mismatch` / `shadow_missed_insert`），五窗今天全零。**口径收窄两处**：没做「→ `EleOperationData` 全量转换」
  （属性值层对拍已由 `direct_attmap_probe` 覆盖、写回换源归 T406）；`Reparented` 不单立 `Moved`（两侧都是
  `Modified` + `OWNER`）。护栏：`shadow_collector` 九条单测（含 ams7999 形状回放、桶归因、落盘截断、
  「不碰持久层」源码钉）、`window_root_plan::the_shadow_parity_only_reaches_warnings_and_the_log`
  （对拍留在阻塞任务里、规划器不得直接消费对拍结果、启动路不开影子）。`cargo test --lib -- window_root_plan
  increment_pipeline shadow_collector` 76 passed。生产接线**未 live 验证**（要 8009 + direct 服务 + 一次真 TTY 窗口，
  与 T416 同一现场）。

- **ADR-058（已接受）：初始化启动收敛为退化窗口；CATA 不入持久层；监听限定域约束启动。**
  起因是审核「当前的增量更新，初始化启动时按新架构要如何重构」，四条发现：生产（direct）启动
  跳过全部模型复核（`lib.rs:795` 的 `!direct_read_mode`）、ADR-050 承诺的「重启靠凭证 vs 文件最新
  重新发现」没有实现者（比较代码 `model_source_floor_by_dbnum` 只喂 `/dbnums`）、非 direct 那条
  复核路的标尺是水位不是文件（与 ADR-054 冲突）、覆盖物化仍是 `pe` 打头的全表遍历
  （`gen_root.surql:44`）。稳态窗口早已搬到新架构（`window_root_plan.rs`「一行 `pe` 都不读」），
  启动一步没动。ADR 定九条：D1 持久层只剩模型面状态与增量摄入的 DESI，**CATA 不解析进库**
  （`ModelTarget.catalogue` 本就是文件身份摘要、`ref_rev` 的边从引用侧建，两者都不需要 CATA 行）；
  D3 保留 DESI 基线（放弃它要重写 `applied_sesno` 语义与 ADR-021 三条判据）；D4 启动 = 退化窗口
  （`S` = 凭证下限、`T` = 文件最新，与稳态共用 `WindowRootPlan`，`fn::sync_gen_roots` 退出启动路径）；
  D5 两级过滤（一条 GROUP BY + 只对落后库开窗）；D6 冷库一根不排；D7 阶段屏障塌成两条并行链、
  Catalogue 阶段取消、模型面不等 `data_ready`；D8 `direct` 只管数据面读哪儿、watcher 接回来持续追新；
  D9 监听限定域仍是调试开关但约束启动，且收窄不在扫描层（`dbnum` 要读文件头才知道）。
  实施见 `docs/plans/2026-09-03-startup-initialization-refactor-plan.md` S1–S5。**未实施。**
- **`docs/plans/2026-09-03-old-pdms-io-retirement-inventory.md`：按 crate API 面清点 old-pdms-io 的退役清单。**
  既有两份文档（09-02 模块归属清单、spec 035 T401–T411）都沿「增量」那一段数，本文换一根轴——沿
  `pdms_io` 暴露的 API 逐个点——因为 ADR-056 Q2 问的是依赖图问题，只有沿 API 面数才数得全。数出
  **三组既没有替身、也不在任何计划里**的用法：① **watcher**（`PdmsWatcher` 是仓里唯一实现，而
  ADR-058 D8 刚把它从「direct 下不起」改成「一直起」，份量正在变重，方向与退役相反）；
  ② **`sync::clone` / `sync::compress`**（MQTT 远端库文件增量 clone 与 `.cba` 归档发布，是文件分发
  功能，与增量和 dabacon 解析都无关）；③ **`legacy_pdms_io` 是个空门**（feature 在 `Cargo.toml:36`
  声明，`src/` 里零消费者——T403 的「只留 feature 后探针」进度是 0%，不是差最后一步）。
  按用户 2026-09-03 拍板「watcher 在 e3d-io 侧新写一个」「全面采用 e3d-io，数据的解析存储也接入
  e3d-io 的解析结果」补了 §7 / §8 两节设计，阶段 0–3 登记为 spec 035 **T412–T415**（四条都不依赖
  T403，可并行开）。§8 记了一条容易漏的：`direct_attmap::to_named_attmap` 已经把
  `outside_schema` / `unset` / `shape_conflicts` 当回执交出来，而今天的消费者只有探针、处置是记一笔；
  数据面接上之后 `shape_conflicts` 必须**阻断窗口**而不是告警——`BANG` 那种按错读法解出来的数字
  长得和对的一模一样，写进 `ATT_*` 之后没有任何东西会报。

### 变更

- **模型门不再等 `data_ready`：数据链与模型链并行（ADR-058 D7 门的部分，从计划 S3 拆出提前落地，
  用户 2026-09-03 拍板）。** 起因是问「现在的启动是否已按需要让解析与模型生成并行」——核对下来没有：
  `model_generation_allowed` 要 `data_ready && model_phase_open`、`install_manifest` 只在数据相全部清零时
  开门、`run_cli` 主线先阻塞在「等待 Meta → Catalogue → Design 数据清单完成」再跑启动模型复核、护栏
  `startup_waits_for_data_then_models_before_rooms` 把 `data < open_model` 钉死；而 `CONTEXT.md`「严格
  初始化阶段」与 ADR-025 的 2026-09-03 对照块已按目标态写成「两条并行链」，读文档会以为做完了。
  现在：`install_manifest` 装好清单（文件身份解析完成）即置 `model_phase_open = !full_model_required`；
  `model_generation_allowed` = `!model_in_flight && (epoch 0 旁路 ‖ model_phase_open)`，去掉 `data_ready`；
  `configure_model_bootstrap` 只按策略关门 / 非发现期开门；`reconcile_model_coverage_at_startup()` 从数据
  等待之后提到 `watch_task` 之后、数据等待之前（它只读文件与 `gen_root`，仍是 `run_cli` 顶层无条件）。
  **仍等数据的两处一字不改**：`mark_model_ready`（收敛宣告——数据尾事务还会往模型积压添行）与
  `postprocess_allowed`（房间吃模型产物）；`open_model_phase` 也仍要 `data_ready`，因为它把 `status` 拨到
  `ModelRunning`，`allows()` 会因此拒掉未完的数据相。实际收益：启动模型复核不再等数据基线；数据摄入期间
  按需生成不再返回 `initialization_not_ready`；数据相被挡不再连带关模型门。**如实说没做到的**：worker
  仍是单消费者，`drain_queue_until_empty` 跑空才进空闲轮、`model_drain_yield_reason` 的 `DataQueued` 让位
  照旧——模型积压只在数据队列空隙消化，字面意义的两条车道同时跑是 D10-B；`Catalogue` 相仍在（S3）；
  direct 仍不起 watcher（S5）。护栏：`startup_waits_for_data_then_models_before_rooms` 改为
  `startup_reconciles_models_before_waiting_for_data_and_keeps_rooms_last`（钉 `worker < reconcile < data`、
  `data < model_ready < room`）；协调器新增 `model_gate_opens_with_the_manifest_not_with_data_ready` /
  `a_blocked_data_phase_does_not_close_the_model_gate` / `a_required_full_model_pass_still_closes_the_gate_after_the_manifest`。
  `CONTEXT.md`、ADR-025 对照块、ADR-058 状态行、refactor plan S3 第 3 条同步补「实施状态」。
  `cargo test --lib` 1351 passed / 0 failed / 100 ignored。**未 live 验证**。
- **启动的模型复核换成退化窗口，两级过滤（ADR-058 D4 / D5，计划 S1）。** 此前它是自己一套：
  `fn::sync_gen_roots` 物化根覆盖（`gen_root.surql:44` 第一句就是 `select value id from pe`）、
  拿 `applied_sesno` 当「要求时点」（与 ADR-054「生成时点是文件最新」冲突）、限定域逐库跑完整 sync /
  未限定域只核有凭证记录的库。现在：`startup_coverage_floors()` 一次请求两条 GROUP BY 拿全部库的
  **安定凭证下限**与**未安定根数**，与 `latest_source_version` 给的文件最新会话比，追平的库连窗都不开；
  落后的库各调一次 `window_root_plan::reconcile_startup_window`，`S` = 凭证下限、`T` = 文件最新，
  选根整条转调稳态的 `build_model_update_plan_from_window`——**一条选根判据都没新增**（ADR-058 S1）。
  启动特有的只有三件：左端从凭证取、未安定的行无条件重排（上一进程的积压刚被 ADR-050 清掉，
  不排就没人再排）、结果自己 `enqueue_plan` + `advance_root_credentials`（稳态那条由数据尾事务带走）。
  开窗失败退到「全部持久根按过期处理」，与比例护栏同一去向。`sync_and_seed_model_coverage` 与
  `fn::sync_gen_roots` 退出启动路径，前者仍服务人工 `POST /model/rebuild`。四条护栏：函数全文不含
  `sync_gen_roots` / `applied_sesno`、书写顺序钉「取下限 → 不落后就跳过 → 才开窗」、两条聚合对
  `mem://` 真跑一遍、「安定」判据的三处用户共用同一份名单与同一条 `> 0`。
  同批把 `lib.rs` 上那道 `if startup_data_ready && !direct_read_mode` 的 direct 门删掉（S5 的第一行
  提前落地）：核对读的是文件与 `gen_root`、不需要任何数据摄入，而 direct 正是它的主场——门在的时候
  生产形态启动一根都不核，ADR-050 承诺的「重启靠凭证 vs 文件最新重新发现」于是没有实现者
  （审核 A1 / A2）。护栏用与 worker 那条同一个手法：往前找最近的 `if`，断言它不含 `direct_read_mode`。
  `cargo test --lib` 1335 passed / 0 failed。**未 live 验证**（零解析库门与「新旧口径差集」要真库）。
- **初始化编排层修复：「模型已收敛」横幅只在它为真时打出来（审核 P1–P4，计划
  `docs/plans/2026-09-03-startup-orchestration-p1-p3-fix-plan.md` T1–T9）。** 审核发现
  `run_cli` 编排层有四条缝，本批全部收口：
  ①（P1）`if initialization.open_model_phase() {…}` 返回 false 时整段模型等待被静默跳过——
  重扫刚发现新会话、数据阶段回到未就绪的那一刻正是这种情况。现改为返回值只用于播报，
  之后照常叫醒调度器并进入收敛等待；门由 `install_manifest` 在清单就绪时自动打开。
  ②（P1）协调器新增 `startup_reconcile_pending` 位：`run_cli` 在配好模型门之后、worker
  起来之前调 `begin_startup_reconcile()`，此后到复核落地为止 `mark_model_ready` 恒 false；
  `reconcile_model_coverage_at_startup` 拆成外壳 + 内函数，内函数每条出口之后外壳统一
  `settle_startup_reconcile()` 并唤醒调度器（`reconcile_startup_window` 入队非空也唤醒）。
  修的是「复核还在逐库读文件头、空闲轮已宣告收敛」的竞态。
  ③（P2，ADR-050 修订）`clear_stale_at_process_start` 从整表 DELETE 收窄为五种数据动作，
  `room_recalc_*` 保留并当面播报行数——房间任务没有凭证补偿路径，direct / env /
  `autorun=false` / `model_incremental=false` 下连唯一的整库房间重建补偿都不跑，删了即真丢工作。
  补偿核对同时改为 `run_cli` 顶层无条件执行（此前被 `startup_data_ready` 把门）。
  ④（P2）`model_coverage_current` 的标尺从 `applied_sesno` 换成文件最新会话
  （`spawn_blocking(latest_source_version)`），与启动复核同尺——此前会出现复核判「落后已排队」
  而空闲轮同刻判「覆盖完整」的双尺并存。
  ⑤（P3）`wait_for_data_ready` 包进与模型等待同款的 60s 超时播报循环（快照 status / 阶段计数 /
  blockers，指纹不变退 300s、Blocked 保持 60s）；Web 服务提前到 watcher 首扫之前启动；
  `latest_source_version` 与 `build_set` 等同步文件 I/O 全部进 `spawn_blocking`；
  开窗失败的退化文案明说「T 端已删的根将生成失败进死信，需人工 pending-units/retry 或
  model/rebuild」。
  ⑥（P4）python `full_init` 连接成功后立即执行同一道启动清空，失败上抛。
  护栏：协调器两条单测（pending 期间 `mark_model_ready` 恒 false / begin 打回已置位）、
  外壳必落地的源码顺序断言、清理与整库重建共用同一份动作清单的源码断言、模型门 SQL
  「含 `latest_source_version`、不含 `applied_sesno」、退化文案必带死信后果。
- **`gen_root` 行在建行时就写顶层 `dbnum`，并回填只有嵌套归属的历史行。** 2026-09-03 在 ams 现场
  查启动数据时发现的：2245 行 `gen_root` 里 **2244 行 `dbnum = NONE`**，库归属只躺在
  `desired_target.dbnum` 这个嵌套字段里——`render_upsert` 建行时从来不写顶层那一列，历史上只有
  `fn::sync_gen_roots` 写它。而按库取数的每一处（启动凭证下限 `startup_coverage_floors`、覆盖清单
  `coverage_tracked_dbnums`、`/dbnums` 的 `model_source_floor_by_dbnum`）都是 `WHERE dbnum != NONE`，
  于是这些行**当面消失**，不报错、只是少算。`fn::sync_gen_roots` 随 ADR-058 D4 退出启动路径之后，
  这个洞会从「历史行看不见」变成「新根永远看不见」。现在 `render_upsert` 的 `gen_root` UPSERT 加
  `dbnum = {dbnum}`，启动核对开头跑一次幂等回填
  （`UPDATE gen_root SET dbnum = desired_target.dbnum WHERE dbnum = NONE AND (desired_target.dbnum ?: 0) > 0`，
  失败只告警，下次启动再试）。两条测试：建行语句必须带顶层 `dbnum`；回填只动「只有嵌套归属」的行，
  已有归属的不碰、两处都没有的也不碰（没有可信归属可补，硬写就是编一个），跑第二遍零命中。
- **`CONTEXT.md`：「按需解析元件库（On-demand CATA）」整章标 retired（ADR-058 D1）**——按需解析 /
  引用闭包 / 部分解析 / 生成根闭包 / 惰性兜底 / 闭包漏边六条只作读旧文档用；CATA 的**文件身份裁决**
  （项目优先级、同号选主、`CataDbLocator`）不在退役之列。「严格初始化阶段」改写为两条并行链、
  删掉 Catalogue 一段；新增「启动退化窗口」词条。「监听限定域」补两句：**初始化启动同样受它约束**
  （限定时只盖域内库，未限定时盖全部候选库），CATA 只从文件读；`_Avoid_` 加「服务边界（它不是）」
  ——它的性质仍是调试开关，只是到达面扩到了启动。
- **`ADR-025` 加 2026-09-03 对照块**：五段顺序收缩为两条并行链，决策 2 的 `Catalogue` 一句与
  「2026-08-25 实施说明」第一条的 `fn::sync_gen_roots` 物化失效；manifest / epoch / 纪元激活门、
  决策 4/5、§7 与决策 8/9 全部不变。

### 修复

- **窗口规划器的计划不再拿 old-pdms-io 复核一遍——那条复核能把库水位永久打死（spec 035 T412）。**
  `apply_one` 此前对每一份模型计划都调 `reconcile_plan_final_presence`，用
  `DabaconSnapshot::contains_refnos_at(T, 根集)` 问「这些根在 T 还在不在」。对 PE 图上溯出来的老规划器
  这是必要的（那张图是窗口**之前**的状态），但对 `window_root_plan` 的根**完全多余**：它们本来就是
  `enumerate_generation_roots(DbSet@T)` 从 target 端枚举出来的，自身即存活证明。而且它是个硬失败
  向量——ams7999 45→46 上 `contains_refnos_at(46, {24383/101198})` 报
  `IndexPageData.noun == 0xCC47DF`（0830 坐实的「`0x18` 是 `free_dwords` 不是条目数」那一族解码 bug），
  而 `24383/101198` 正是 46 新建的那棵 STRU 子树的顶。一报错整窗 `Err` → 批次 Failed →
  `prepare_attempt` 行已落 → 下一轮进 replay 分支按固定区间重放同一份计划 → 同一页再报错 →
  **该库水位永久卡在 45**，只能手删 attempt 行。
  修法按 T407 的拆分口径落：函数一分为二——`verify_frozen_source`（冻结源身份门，`apply_one`
  **无条件**调，纯数据窗口也不许在收集与持久化之间被换掉文件）与 `reconcile_plan_final_presence`
  （存活复核，只在 `!model_plan.window_planned` 时调）。`ModelUpdatePlan` 新增
  `#[serde(default)] window_planned`，由 `WindowRootPlan::into_model_update_plan` 置 `true`，老规划器的
  两处构造点显式 `false`——旧 durable attempt 没有这个字段，反序列化成 `false` 正是老规划器的行为。
  整块放进同步作用域：`DabaconSnapshot` 不是 `Send`，跨 `.await` 持有会让 `apply_one` 的 future 失去
  `Send`，而 `drain_queue_until_empty` 要求它 Send。
  验证：`cargo test --lib` **1338 通过 / 0 失败 / 100 ignored**；新增两条源码钉
  `every_window_verifies_the_frozen_source_before_any_planner_branch`（身份门必须排在任何规划器分支之前）
  与 `window_planned_plans_skip_the_legacy_liveness_recheck`（存活复核只有一个调用点、且挂在标记后面）；
  **新增真库验收门 `live_ams7999_45_to_46_plans_the_new_stru_roots_without_the_legacy_recheck` 通过**——
  同一窗 e3d-io 侧索引差分 `+22 ~1 -0`、`unresolved=0`、闭包完整，4431 个根里只排出
  `24383/101198 STRU` 与 `24383/101199 FRMW` 两个 regen 根、4429 个凭证前移、`degraded=None`，
  老读法报错的 101198 / 101201 都在账本里；ams8000 九窗门复跑仍绿。
  **未 live 验证**：真起服务把 ams7999 45→46 跑到水位 46（本轮那台服务 `--watch-dbnum 8000`，
  7999 不在监听域内）。

## 2026-09-02

### 新增

- **ADR-057（提议）：e3d-model 是模型面的纯函数层，模型面的状态与副作用归 gen-model。**
  起因是用户问「模型生成和模型增量不是都应该发生在 e3d-model 吗」——算法层面两件事确已在
  e3d-model（`pipeline` 全量生成 + `increment` L0–L4），漂的是三处：生产选根还没接 e3d-model
  （legacy 走 `model_impact`，direct 无窗口差分即审核 S8）、e3d-model 的 `execute_plan` 在 spec 035 P1
  删 `apply_window` 臂后将没有生产消费者、纯文件函数 `enumerate_generation_roots` 却在 gen-model。
  ADR 把边界写成一句口径「对 `(库文件, 会话)` 是纯函数的进 e3d-model；碰 SurrealDB / 凭证 / 队列 /
  锁 / 房间 / 空间树 / MQTT 的留 gen-model」，D3 定 `execute_plan` 为参考实现 + 离线工具而非生产
  执行器（不删、不接线），D4 / D5 把根枚举与 `touches_roots` 划归 e3d-model（与 spec 035 P2-1 同批），
  D6 钉依赖方向单向、e3d-model 不得依赖 `surrealdb` / `aios_core` / `pdms_io` / `tokio`。
  同批落地：`vendor/e3d-model/src/increment.rs` 模块文档新增「生产定位」一节（只改 `//!` 注释）；
  `CONTEXT.md` 新增「模型面 / 数据面 / 模型面状态」词条。状态**提议**，待拍板后 `record_decision`。

### 变更

- **direct 形态也启动数据批次 worker（只不起 watcher）；`/health.worker_alive` 在 direct 下报真值。**
  此前 `AIOS_DATA_READ_MODE=direct` 把 watcher 与 worker 一起跳过，而 `POST /update/execute` 没有
  任何 direct 守卫、一律 202 入队——人工执行的批次进了队列没人出队，永远「排队中」，`/health` 又把
  `worker_alive` 报成 `null`，客户端连「不会被处理」都无从得知（plant-ui 计划
  `docs/plans/manual-update-and-get-work-on-direct-mode.md` §二、拍板 D1）。现在 `run_cli` 无条件
  `ensure_batch_worker`，direct 分支开完模型门顺手 `scheduler.wake()`；`sync_live` 仍
  `configured && !direct`，自动追新照旧不开。worker 本来就是队列的唯一消费者、与读取底座无关：
  数据面走直写路径（ADR-056 P1），模型面走 `generate_roots`（e3d-io 读文件）。ADR-053 里「跳过
  watcher/worker」那句自此只剩 watcher 一半。源码钉两条：`direct_mode_starts_the_batch_worker_but_not_the_watcher`
  （lib.rs）、`health_reports_worker_liveness_in_every_read_mode`（handlers.rs）。**未 live 验证**
  （要 direct 形态起服务 + 真库 + 人工执行一次）。
- **五类复用曲面原语的身份键带上段数，单位行携带段数落库（specs/009 T041，ADR-044 决策 2；
  改身份 = 整库重建）。** 单位行的半径恒为 1、真实尺寸只在实例变换里，所以 `hash_unit_mesh_params()`
  与 `gen_unit_shape()` 都在**原件**上按真实半径把段数算好：柱 `unit_cylinder_identity(n)`
  （`LCylinder` 与非切角 `SCylinder` 同函数同行）、球只混 `n`（`stacks` 恒 `n/2` 不进键）、同心
  Snout 混大端段数、`CTorus` 混 `(ring, tube)` 二元组、`RTorus` 只混 `ring`、碟把 raw `prad` 换成
  `f32_round_3(prad/pdia)`（落库同一个量化值）再混 `DishSegments` 枚举（球碟二元 / 椭圆碟三元，
  分支自带，前缀撞不上）。段数随单位参数携带（`segments` / `ring_segments`，
  `skip_serializing_if = None`，原件与旧行都不带），分发臂经 vendor 访问器读携带值、原件按同一条
  规则现算；`manifold_tessellate::unit_mesh_identity`（写死 32 / 16×36）整组删除。为此把
  `libgm_discretise` 进键的那一半（容差、整圆 / 部分回转公式、§7.9.1 调用点表、两种碟母线）搬进
  aios-core `prim_geo::libgm_discretise`，本仓原样 `pub use`——定义仍只有一处，只是住到了算键的
  crate。`SCylinder` / `LSnout` **不改 bincode 字节**（SSCL 与偏心 Snout 的键逐位不变，`t041_b6`
  钉住）：非切角柱的单位形状直接是 `LCylinder::unit_with_segments(n)`，`convert_to_unit_param`
  按实际类型落变体，`canonical_unit_param_json` 不再按 id 特判柱；`CYLINDER_GEO_HASH` /
  `SPHERE_GEO_HASH` 删除，`TUBI_GEO_HASH`（`inst_geo:⟨2⟩`）保留为**唯一**按固定 id 寻址的曲面行，
  其段数 32 收成具名欠账 `pdms_inst::TUBI_UNIT_SEGMENTS`（原状，管段共用一行带不了口径）。
  T039 源码门改判：不再接受 `unit_mesh_identity::`，接受携带值访问器。顺带修 `t041_b1` 的夹具
  （`dish_of` 第二参相对直径，`0.25` 得到的是 `h/a = 0.5`，与自检不是同一件碟；改 `0.125`
  并钉断言）。验证：`cargo test --lib` **1328 通过 / 0 失败 / 99 ignored**（此前 1308 / 8 红），
  `t041_*` 11/11、`aabb_tree` 白名单、T039 / T042 源码扫描全绿；vendor `prim_geo` 六模块 22/22；
  `cargo check --lib --bins --tests` 绿。**仍欠**：整库重建（endgame plan D1 同批）、C4 整体门
  （库 A 副本五类应 474 行）、vendor 推上游 + `Cargo.toml` 升 rev（提交态钉的仍是 `d97586c7`，
  升 rev 前提交态编不过）、T049 RVM 抽检在重建后跑。
- **未指定时点的模型生成一律取文件最新会话（ADR-054，取代 ADR-053 Q3）。**
  用户拍板原话「如果没有指定时间，默认就是要使用最新的数据去生成模型」。当前投影的时点
  与库文件权威换源：`E3dModelService::from_current` 的 pin 来自 MDB 成员、时点留空，生成时
  按目标库解一次文件最新会话（新模块 `data_interface/model_source.rs`，复用历史投影
  `historical_model::resolve_session(Latest)` 这一把尺子，按文件长度 + 修改时刻缓存）；
  生成根所在库不再 `SELECT dbnum FROM pe`，改在 MDB 的 DESI 文件里按会话索引点查
  （零解析项目从此能点看即生成；命中零个或多个都报错，不猜）。完成凭证判据由等值改
  单调（`gen_root.source_end_sesno >= 要求时点`，`0` 永不算覆盖，时刻列只记不比），
  `generation_root_cache_current` / `gen_root_credential_is_current` / ADR-025 模型门
  `model_coverage_current` 同批改；`apply_window` 不再断言 `pin.sesno == target`，窗口右端
  按显式时点开库，已发布的更新版本覆盖旧窗口时收口为成功而不是失败。`DirectStore` 的
  「没钉 pin 即报错」翻成「定位器认识的库开库解最新、不认识的才报 `NoFileForDbnum`」，
  `NotPinned` 变体随之退役；`pins_from_watermark` 保留为对拍探针显式钉水位的工具。
  已知代价：`ModelTarget` 带 `(file, session)` 摘要，切换后存量凭证全部失效一次，首显整片
  重生成。验证：`cargo check --lib --bins` 绿；lib 单测 1297 绿，8 红均为本变更之外的在飞
  工作（`aabb_tree` 白名单、`pdms_inst` t041 段数分行），与本条无关。

### 修复

- **`aabb_tree` 树写点白名单跟上 `aabb_refresh.rs` 的搬家。** AABB 直写刷新
  （`update_inst_relate_aabbs_by_refnos*`）已从 `occ_generate.rs` 搬到 `fast_model/aabb_refresh.rs`
  （旧生成器退到 `legacy_model` feature 后面），可 `tree_write_sites_stay_on_the_audited_whitelist`
  的白名单仍写着 `occ_generate.rs`、源码钉也还留在那边——新文件写树却没登记，门红得正确。
  白名单条目换成 `aabb_refresh.rs`，`aabb_write_order_tests` 那组源码钉（记录先于指针、锁跨
  判定→事务→同步、串行锁先于树锁、每次变化同事务 bump epoch、崩溃注入点位置）连同
  `aabb_change_tests` / `flexible_geometry_deserialize_tests` / live 用例一起搬进 `aabb_refresh.rs`，
  钉的边界改为「函数签名起到紧随其后的 `tree_box_changed`」；`occ_generate.rs` 里只留
  生成器自己的钉，边界改成 `pub(crate) use super::aabb_refresh::*;` 那条 re-export 接缝。

## 2026-08-30

### 新增

- **legacy↔v2 读取对拍探针（P1 尺子）落地并批跑全绿**
  （`src/bin/legacy_v2_read_parity.rs`；判读 `docs/evidence/2026-08-30-legacy-v2-read-parity.md`，
  逐文件原始数据与自动汇总同名 `-raw.jsonl` / `-raw-summary.md`）。431 个 ams000 真库、
  文件级错误 0、探针自检 0：活叶页尾部多读 **779,558** 条（168 库，D3-1 陈旧槽位坐实，
  **P3 门实测开**）、纯幽灵键 8,807 个、抽样幽灵 35.6% 可被旧栈**点查**够到
  （`ams8000_0001` 上 64 个——幽灵不止污染穷举，点查也漏）；两栈同中的点查位置不一致
  **0**（D3-4 文件态位拆一致再证）；v2 活键数与 e3d-io 429 库硬门 789,831 **逐键吻合**，
  两条独立 core.dll 对齐实现互证通过；活条目 flag 全 1、陈旧槽位 98.9% flag=0，
  「认领扫描 flag==1 vs 点查无视 flag」口径分裂拿到第一份定量证据（D3-5/V2）。
  会话链侧顺带钉住 D2-2 的具体形态：旧栈 `cur_ses_pgno > 4` 系统性看不见落在第 2–4 页
  的创世会话（429/431 库）。顺带：`old-parse-pdms-db` 整体再导出 `pdmsdb_engine_v2`，
  下游做对拍不必二次钉 rev。
- **dabacon 索引页布局裁定：AoS + `free_dwords` + 子树最小键**
  （`docs/evidence/2026-08-30-dabacon-index-page-layout-adjudication.md`，原始输出
  `docs/evidence/2026-08-30-dabacon-index-layout-probe-raw.txt`）。仓里先后四份索引解析
  对同一批字节有三处互不相容的读法，且没有任何现存测试能区分它们——`pdmsdb_engine_v2`
  那条真库 round-trip 是「同一套解码 + 同一套编码」，错的读法照样逐字节相等。用纯文件
  探针（`old/vendor/e3d-io/examples/index_layout_probe.rs`，不依赖 IDA、不调用任何现存
  索引代码）把 2×2×2 八种组合在 429 个真库文件上按七个结构不变量打分，唯一解是
  **AoS 条目 + 页头 `0x18` 反推条目数 + 非叶键为子树最小键**（429/429 全零；SoA 三种
  组合 0/429）。三条结论：① **推翻** `2026-08-13_reverse-core-dll-index-leaf-report.md`
  标「事实」级的 C2 条（结点不是 SoA）；② `0x18` 是 `free_dwords` 不是 `pfno`，「扫到
  全零槽为止」在 ams8000 上凭空多出 7025 条叶条目、6790 个重复 refno、52 个重复子指针、
  7 处层级异常、53 个读不动的子页——`session_index_diff.rs` 那一整套异常计数**是计数
  bug 的产物，不是文件格式的性质**；③ 非叶键是子树最小键，仓内所有注释都写反了，但路由
  代码碰巧实现的是最小键语义（所以点查一直对，写下来的模型一直错）。
- **e3d-io L0/L1 重写：页 I/O 与会话链按 core.dll 的真实寻址落地**
  （`old/vendor/e3d-io`，`aaf14a5`）。`PageSize` 改 newtype，`words()`/`bytes()` 分开——
  文件头 `0x34` 按 4 字节字计、被当字节读，490 个真库里 17 个中招（`ams7329_0001` 读出
  `sesno=0`，权威值 221），这类混淆现在编译期就不成立；页大小不合法即 `Err`，删掉
  `else { DEFAULT_PAGE_SIZE }`（那个默认值是 4096，真实页大小 2048，正好差一倍）；
  `PageId{ext,page}` 的 `ext` 真的用来选文件句柄，取不到 extent 报 `MissingExtent`，
  不再把 extent 2 的第 N 页安静地读成 extent 1 的第 N 页；`read_into(&mut [u8])` 是唯一
  原语（旧实现每读一页分配一次，一次树下降就是树高次堆分配）；positioned read；预读页
  与按需读分开计数，否则「点查读了几页」量不准。L1 侧 `open()` 只是 `open_at(Latest)`，
  没有第二套 latest 逻辑；会话不存在是报错不是 `None`；链走有环保护与页数上限。顺带删掉
  `engine::sessions()` 里那套**错的**第二份会话解析——它从 `0x30`（最旧会话）起步、只读
  一页、不跟 `last_ses_pgno`，在 264 个会话的库上返回单元素 vec 且报成功。
- **e3d-io L2 索引页解码器 + B+ 树游标**（`6be8847`、`a9ade97`）。解码按上条裁定实现且
  不提供任何回退读法；畸形页一律报错命名失败字段，绝不降级为空条目列表。游标从**会话页
  携带的索引根**下降，点查与顺序枚举共用同一个结点读取与同一条路由规则——因此二者对
  「什么算存在」给出同一个集合，这条已设为**硬门**：429 个真库、枚举出的 **789831** 个键
  逐个点查可达且落到同一条记录，枚举没给出的邻近键点查一律 `None`（旧的「扫到零为止」
  计数下这道门根本不可能通过——它凭空造出的条目没有任何下降路径能路由到）。冷缓存点查
  代价用**单页缓存**度量（读两次就计两次），实测恰好等于树高，语料里最深的树是 3。
  `ReadOnlyEngine::search_index` 那份自带的下降已删除、改为委派——它既不校验子层级递减
  也无防环，指回自己的树要靠深度计数器兜底。新增源码顺序断言：索引模块之外再出现第二条
  路由规则就红（这个栈继承过来的是四份）。
- **e3d-io L3 记录层重写：按记录自己写下的地址取，不再靠找**（`522a252`）。记录头第 6/7
  字是**显式属性流地址**、第 8/9 字是**成员表地址**，都用索引叶值那套 `(page_no, packed)`
  编码；每个块尾同样写着自己的续接地址。438 个真库、2 923 428 条记录、这些地址够到的
  **3 195 015** 个块里，指向空处的 **0** 个、类型对不上槽位的 **0** 个。旧读法三处全错：
  ① 把 `0x00000007` 当结束标记往后找，可它只出现在 **29 027 / 789 831（3.7%）** 条记录的
  真实末尾，其余 96% 多吃了下一条记录的一截，另有 1 033 条一路吃到 64 KiB 上限、把完好的
  记录报成截断；② 块之间靠「跳过填充继续找」搭桥，这一步在 **36 106** 条记录上触发且**无一
  例外**越过了本页边界——它把同一元素的**后来副本**接到了正在读的记录尾巴上；③ 相邻性根本
  够不到 **46 566** 个块（分布在 40 263 条记录上）：`ams5100_0001` 的 ROOM_NO 定义
  13292/122 离页尾只剩 24 字节，塞不下一个块，整条属性流在下一页，按相邻读它就是个没名字、
  没属性、没有「适用于哪些 noun」列表的空壳，**BRAN 因此丢了 Dictionary 给它定义的 22 个
  UDA 里的 2 个**。块头那两个「保留字」是续接地址（BRAN 24383/85432 的成员表里那个
  18010/8193 就是它被当成员读出来的），所以块 payload 恒从 +20 起，显式解码器里「0 和 8
  两个起点都试一遍」的兜底也一并删掉。硬门：429 个真库 **789 831 / 789 831** 全部读出且
  自报 RefNo 一致（旧读法直接失败 1033 条）；交出去的字节数与解析器消费的字节数逐字相等。
- **e3d-io：查元素只从会话根下降一次**（`357512d`）。`find_element` 原先叠了三层兜底——
  搜遍文件里所有树、取最靠后的副本、都不认就把整个文件倒着扫一遍找 RefNo 字节模式——
  三层都在给「根找错了」擦屁股。根来自「读每一页、留下没被别的索引页指过的索引页」，索引
  是 copy-on-write，于是历次会话遗留的树全被当成根。现在是一次下降加一次读记录；记录自报
  别的名字是**报错**而不是没找到（429 个库 789 831 个键里 0 例）。`ams1112_0001` 的
  17496/950 那「十一个叶子、十个指向别人」是十棵死树加一棵活树，而那棵活树属于会话 12——
  它自那以后就没再被任何会话索引过。`scan_all_indexed_refnos` 换成 `indexed_refnos`：
  旧的取全文件所有索引页的叶条目共 940 694 条，活树只有 789 831 条，**16% 是够不到的树**。
  连带发现：本仓 TTY 采样清单 `sample.tsv` 正是从旧清单里挑的，**211 行里有 77 行是此后
  再没被任何会话索引过的元素**（光 `ams8000_0001` 就贡献 26 个止于会话 8 / 共 264 会话的
  图元），另有 4 行记的是被取代副本的隐式字数。
- ~~pdms-io 直读底座新增 Core3D session-pinned `open_at`、严格 page type 5 解码、
  `IndexCursor` 实例查询和 raw index slot 类型；多 extent 不再隐式退回 legacy 扫描。~~
  **（此条描述的是 `pdms-io-fork-engine-v2` 工作副本里的改动，该目录已从磁盘消失、
  代码不复存在；留着划掉以免日后查史被它带偏。上面 e3d-io 几条是重新实现的等价能力。）**

### 修复

- `Cargo.toml` 里指向 `../../pdms-io-fork-engine-v2/crates/pdmsdb_engine_v2` 的手动
  `[patch]` 改回注释态：该工作副本已从磁盘上消失，带着悬空 patch 连 manifest 都解析不了。
  `cargo metadata --no-deps --offline` 恢复正常。
- Python direct 解析新增 `parse.attmap` 与 `parse.subtree`，通过 `pdms-io` 直读指定 refno 及全部后代；补充 AMS 8000 ZONE 离线验收与 IDA DbElement 交叉证据。

## 2026-08-29

### 新增

- **ADR-055：新版 pdms-io（db1~db5）的元素语义以 Core3D.dll 为准**（grill Q1–Q8 全采
  推荐项）。分层定权威——db1–db3（页/会话/B 树）继续以 core.dll 为准，db4–db5 及以上
  （noun 位分类、三模成员遍历、significant 攀爬、库类型门、CE 导航）以 Core3D 为准；
  noun 位表走 `trait NounBitSource` 双实现（生产读 `core_sha256` 钉版的快照、校验不过
  报错不回落，对拍走 core.dll FFI 现取）；证伪靠三层 oracle（legacy + core_dll + 新增
  `core3d_oracle`，以可执行参考模型为期望值、C 编号用例数据驱动）；pdms-io 只做读语义，
  队列/去重/三遍消费留在 gen-model；本轮写侧冻结。配套 `specs/034-core3d-semantics/`
  三件套（spec / plan[Constitution Check] / tasks，P0 尺子 → P1 db4 语义层 → P2 CE 栈 →
  P3 库类型+extent → P4 页大小/时点收尾 → P5 gen-model 升 rev 联动）与
  `docs/plans/pdms-io-v2-core3d-alignment.md`（拷问全文，已镜像到 pdms-io 两个工作副本）。
  实施基线核查发现 P4 两条根因（文件头 `0x34` 按 4 字节字解释、页大小探测假匹配拒绝）
  已在 engine-v2 分支以 `348d187`/`cb7dd95` 落地，P4 缩为真库回归 + `open_at(sesno)`。

## 2026-08-28

### 新增

- **core 的 noun 粒度位表导出入库：`significant` 127 / `primitive` 374**
  （模型增量更新向 Core3D 对齐计划的 P0）。`PartialUpdateDesiMgr` 决定「改了之后重画
  什么」用的不是类型名单，而是 noun 描述符上的两个 bool 位；本轮把它们从 live E3D 3.1
  取下来落进 `tests/fixtures/core-noun-granularity-e3d31.json`（schema 2，三张表
  unknown 与 not-found 全为 0）。采集脚本同批泛化成多字段（`--field granularity`），
  单字段仍写 schema 1 且序列化逐字节不变（1931 行回放验过）；跑之前先问 core 自己的
  `DB_Noun::fieldType` 决定用 bool 还是 int 重载而不是猜——**用错重载是静默的**，
  实测四个 bool 位返回 0、`negative` 返回 3，独立复核了逆向取证的类型划分。
  数据本身回答了计划里的分叉点：core 的 significant 是 127 个而不是原先估的「数百」，
  对我们 4 个 MDU 类型仍是 32 倍差距；`primitive_b` **不是** `primitive_a` 的子集
  （或式差 27 个，合并成一个位会丢 noun）；1514 个 noun 两位都不带——core 对它们的
  变化什么都不做，而我们走 `Unknown → Regen`，这是差距最大的一格。第一处对账也出来了：
  `DEFAULT_DELIVERY_UNIT_TYPES` 四个里 core 认 BRAN / EQUI / HANG，**不认 `SUPPO`**。
  三条守卫测试落在 `generation_root.rs`（快照自洽、两个 primitive 位不冗余、SUPPO 是
  唯一分歧），快照重导后结论若变会直接变红。生产判据本轮不动，那是 P1/P2 的事。

- **失败记录带走落记录那一刻的暂存窗口残留（ISSUE-025 §四 4a 的记录一半）**。
  `BatchFailure` 新增 `staging` 一格，取 `staging::lifecycle::resource_snapshots_for(dbnum)`
  的原样输出写进 `logs/batch-failures-*.jsonl`，与 `/health` 的 `staging_windows` 共用
  同一个成形函数——面板卡与失败记录说的必须是同一句话，不许出现第二种形状。语义三分：
  `[]` = 记录时该库名下已无窗口（回滚干净，或这一批走直写没建窗口）；非空 = 残留，
  重跑多半撞同一堵墙；字段缺席 = 老版本记录，答不了这一问。面板失败卡照此渲染（残留
  红字带写回原话与档位，空一句带说明，老记录不画）。这一格的动机就是 8191 现场：
  「暂存窗口」卡活在进程内、重启即清空，而人来问「`staging_8191_1` 回滚没有」的时候
  往往已经重启过了——落盘那一份才是异机与事后能看到的。测试
  `a_record_carries_the_staging_leftovers_at_write_time` 钉住空数组不许藏格。

### 修复

- **`primaryList` 快照里那 52 个「core 读不出来」是读取通道的假象，现已补全并按 core 改判**
  （ADR-009 修订）。`scripts/e3d/dump_core_primary_list.py` 一直走
  `db_get_element_info`，而那个导出**根本不是通用字段读取器**：它转发到
  `sub_5B05280`，后者把 field id 写死成一个五路 switch（`642215` / `297853135` /
  `243803617` / `11037101` / `282170750`），落在外面一律置错误码 542 返回 0；
  `primaryList` 能读通只是因为它恰好是这五个之一。更要命的是外壳在**内部 noun 查找
  失败**时同样报错返回，2026-08-18 记的那 52 个 unknown 正是查找失败的那批，被当成
  「core 没答」保守取真。改走 core 自己导出的 `DB_Noun::findNoun` +
  `getField(int|bool)` 之后一次读通：1931 个 noun **全部解析**，与旧快照重叠的 1879 个
  **逐值相同、零处不一致**，那 52 个**全部为 `false`**（其中 8 个还带着真实粒度位，
  说明记录是实的不是零值）。按 ADR-002 的 core 权威口径改判，`false_count` 737 → 789，
  这 52 个类型不再多做成员差分；保守兜底保留，但只覆盖「`noun_flags.json` 之外的
  noun」，快照测试把 `unknown` 为空钉死——它若回来就说明导出器退回了旧通道。顺带答了
  08-18 记的那条待决项：`core_sha256` 对不上是因为本机 core.dll 在 08-23 被改写过
  （大小不变），而值逐个相同，**是产物层面的溯源漂移不是数据漂移**。

- **错误日志按登录项目筛：`?dbnum=8191` 不再把上一份配置留下的同号记录一并端出来**
  （ISSUE-025 §五 5a/5b 收口）。`logs/` 落在进程工作目录下、跨配置改动活着，同一个
  目录先后跑过两个 `project_name` 时，8191 这类号在每个项目的 sys 库里都存在。
  `/api/v1/error-log` 与 `/api/v1/batch-failures` 现在认 `?project=`：缺省只回本服务
  登录的项目（服务自己知道它登录的是哪个项目，这件事不该问人），`all` 不筛，其余值
  指定项目。**不带 `project` 字段的行不筛**——`queue_stall` 与老版本记录本来就没有
  这一格，按项目筛把它们静默滤没，「这台机器没停滞过」就成了筛出来的假话。回执带
  `project_filter`（`null` = 没筛）：「这个库没失败过」与「被筛掉了」必须分得开。
  面板同批收口：错误日志卡加「全部项目」开关（服务端重取，口径以**已取回那份**回执
  为准、不抢答），空状态分「本项目名下没有」与「真的没有」两句话；`byDb` 同号显式
  择优（取归属 `h.project` 的那一行，一行都对不上退回头一行，此前是后来者静默覆盖），
  连败卡的撞号候选表只在当前项目一个都对不上时才摆出来。测试
  `the_project_filter_keeps_own_and_unattributed_rows` 钉住三件事：本项目 + 无归属
  留下、别的项目滤掉、回执报筛子。

## 2026-08-27

### 新增

- **内存模式：持久层可以整个换成进程内嵌的 kv-mem，rocksdb 不再出现在链路里**。
  `DbOption.toml` 新增 `in_memory_db`（默认 `false`，环境变量 `AIOS_IN_MEMORY_DB` 压过它）。
  开着时 `run_app` 不再连 `v_ip:v_port` 那台 SurrealDB，而是把进程全局 `SUL_DB`
  接到 `mem://` 上：初始化解析、增量窗口写回、模型与房间派生数据全部走原来那条路，
  只是落点从 rocksdb 变成内存。没有第二条代码路径——`SUL_DB` 是 `Surreal<Any>`，
  引擎选择关在句柄里头，上层每一处查询与写入一个字都不用改。嵌入式引擎没有认证面，
  因此不 `signin`；ns/db 仍取配置里那一对，同一份配置在两种介质下指向同一个逻辑库。
  与 ADR-017 的暂存窗口不是一回事，也不冲突：窗口本来就各自占一个独立 `mem://` 实例，
  这里连的是另一个实例，journal 分块重放与水位发布的语义原样保留。
  **这是非正常运行状态，因此三处都要出声**：连接时打两行取舍、启动横幅点名、
  `/health` 加 `in_memory_db` 栏。代价写在配置注释里，两条都不可逆——进程一退整库就没了
  （崩溃现场只剩日志），库进了进程内也就没有端口可连（`rvm_verify`、`/sql` 探针、
  `Capture-*Evidence.ps1` 全部够不着）。要留证据或要让别的进程读库，用部署脚本的
  `-InMemory`：那是让**外部** surreal 进程换 memory 后端，端口还在。管不到的东西也说清楚：
  `assets/meshes` 与 `accel_tree` 快照仍然落盘，它们是 aios-database 自己写的派生数据；
  空间树启动判据照旧按库侧指纹裁决，空库走重建这条不变。

- **运维面板内嵌进二进制，部署只剩一个 exe**。`ops.html` 由 `include_str!` 烘进
  `aios-database`，`GET /ops.html` 走显式路由、压过底下那个 `ServeDir` 兜底，所以
  现场不再需要跟着同步一个 `PLANT_UI_WEB_ROOT` 目录，也不会出现「后端升了、面板还是
  上一版」这种两份东西各走各的。**磁盘副本仍然优先**：`ui_root/ops.html` 存在就用它，
  且是每次请求现读——「改一句判决措辞、落个文件刷新就行」这条不能被内嵌收走，重编一次
  是分钟级还得停服务换 exe；靠目录部署的机器因此行为一字不变。响应带
  `x-ops-panel-source: embedded|disk`，两份内容不一致时这是唯一说得清的地方；另加
  `Cache-Control: no-cache`——页面此后跟着 exe 版本走，而它恰恰是用来回答「现在是什么
  状态」的，自己被缓存住最要命。启动时也把用的是哪一份打在控制台上。
  测试 `the_embedded_panel_is_real_and_a_disk_copy_still_wins` 同时钉住两头：内嵌的
  确实是那一页（`include_str!` 指错文件或 `web/ops.html` 被清空，编译照过，直到有人
  打开一个白页才发现，而那多半发生在出事的时候），以及磁盘副本压过内嵌。
  同批把 `http_api` 并进了 `default` feature：内嵌面板的意义就是「一个 exe 拷过去就能用」，
  而 08-27 当天真踩过一次反例——默认 feature 编出的 exe 里根本没有 Web 端口，拿去部署，
  面板和 REST 一起消失，且没有任何报错。现在裸 `cargo build --release` 与 CI 的显式清单
  （`ws,gen_model,manifold,project_hd,http_api`）等价，`--no-default-features` 的场合才需要
  自己点名；编没编进去（能力）与要不要监听（`DbOption.toml` 的 `http_api_addr`，注释掉即
  不开端口）仍是两回事。web_service 的单测也从此跟着默认口径一起跑，不再要求单独点 feature。
- **`/assets/ops.html` 加实时事件流，并把失败原因、当前阶段、撞号候选摊到页面上**。
  轮询回答「现在是什么状态」，新的「实时事件」面板经 `WS /api/v1/ws → tasks` 回答
  「刚才发生了什么」：批次起止与模型单元逐个完成按发生顺序各出现一次，失败带原话。
  它与轮询各活各的——WS 连不上（老后端、代理不转 upgrade）只让这一块变灰，页面其余
  部分照常；`seq` 只用来认掉帧（服务端跳帧时出提示条）而不当行 key，因为它由每条连接
  自己编号、重连从头开始。按 `ws.rs` 的协议每 30 秒 ping 一次：服务端 90 秒收不到
  **客户端**消息就断，广播出去的事件不算数，不心跳的连接每 91 秒必断一次，而空闲恰恰
  是最该盯着这一块的时候，断口里的事件没有重放、补不回来；`pong` 也占一个 `seq` 号，
  先记再退出，否则自己的心跳会被一条条数成服务端跳帧。同批三处是「数据一直都在、只是
  没画出来」：失败任务的 `result.batch.message` 整段摊开（此前查一次失败要么开
  devtools、要么另开窗口打 `/tasks/{id}`）；running 行接上 `current_stage` 与交付单元
  计数，并单报**本阶段多久没有新进展**（「慢」和「卡住」在耗时那一列上长得一模一样）；
  连败卡片改为并列展示「批次回执 —— 权威」与「`batch_failures.reason` —— 取的是
  `warnings.last()`」，后者在净窗口硬失败时只剩「收集口径：净窗口 …」那条标注，照它
  写卡片就成了一句正常统计。另外 `dbnum` 不是主键——SYS 库在每个项目里都是 8191，撞号
  是常态，而水位、重试记录与连败账本三个键又都只认号，于是撞号时把候选按 `file_path`
  全摆出来让人自己认。默认主题改为亮色：值班屏幕反光下深色那组红/琥珀拿到白底只有
  2~3:1，三色重挑到 ≥5:1；深色加 `?theme=dark`，刻意不跟随 `prefers-color-scheme`
  ——跟着操作系统走等于两台机器看到两个样子。

- **面板「最近任务」的每一行都能点开，摊出这条 `TaskEntry` 的全部字段**。
  上一批把 `result.batch.message` 摊了出来，但那只是整行里的一句话：任务下面还压着
  入队／开跑／结束三个时刻、会话区间与它对应的保存窗口、`current_stage` 与
  `stall_deadline`、依赖 refno 的总数／已解析／缺失、以及 `result` 与 `detail` 两段
  完整 JSON——**这些数据一直都在轮询回来的 `/tasks` 里**（`TaskRegistry::list()` 返回的
  就是完整 `TaskEntry`），只是没画出来，于是想看一眼仍旧只能开 devtools 或者另开一个
  窗口打 `/tasks/{id}`。现在点行即展开，**不发任何请求**，同时只开一行（五行全摊开这张卡
  就成了滚动条）。缺席的字段画「—」而不是把那一格藏起来：「这一批没有依赖阶段」和
  「这版后端还不写这个字段」处置完全不同，藏起来两者就长得一模一样；只有整组都不适用
  （依赖计数）时才整组不画，那时一排「—」只是噪音。三处按状态分叉，因为同一个字段在
  running 与终态上问的不是同一件事：跑着的行报「本阶段静默多久」（超 60 秒转琥珀）、
  终态行改报「末次进展」的时刻——一条三小时前失败的任务说它「静默三小时」是把死亡读成了
  卡顿；`stall_deadline` 只在 running 且已过期时标红；`current_stage` 在终态行改叫
  「停在哪一步」（`finish` 不清这个字段，所以它就是任务停下的地方）。排队多久与跑了多久
  分两格报，`created_at` 是入队、`started_at` 才是开跑，只报一个「耗时」会把等待算进执行里。
  展开块底部两个出口：`→ {dbnum} 的落盘错误记录` 复用队列卡那颗跳转按钮的逻辑（此前它
  只长在被 phase 挡住的队列行上，从任务卡看到 dbnum 还得自己手打进筛选框），以及直通
  `GET /tasks/{id}` 原文的链接。
  同批把这张卡「只画最近 5 条」的硬上限做成开关：勾上「列出全部」列出这一轮取回来的
  全部终态任务，**并把取数上限从 12 提到 60**——只放开渲染不放开取数，页面会一边说
  「全部」一边只有 12 条，是自己骗自己。收起时**如实报出还有多少条没画**（「另有 N 条
  跑过的任务 —— 勾上「列出全部」」）：截断了却不说，等于让人以为跑过的就这几条，而这张
  卡恰恰是用来回答「刚才都跑了什么」的。展开后若取回来的条数顶到这一轮要的上限，再说
  一句这不是全部并给出 `?limit=160`（服务端硬顶）——注册表留到 1000 条，一次全取会撑破
  单响应体积契约，而这一份是每 2 秒都要拉的。排队中的行任何时候都不混进来，它们归队列卡。

- **面板画出「暂存窗口」卡：失败之后那个窗口到底回滚了还是残留着**（ISSUE-025 §四 4a
  的面板一半）。数据一直都在 `/health` 里（`staging::lifecycle::resource_snapshots()`），
  此前一个字都没画——于是 8191 现场那屏「使用 kv-mem 暂存窗口 `staging_8191_1`
  （sesno 36..=37）」之后窗口的下落，只能去翻控制台。它直接决定重跑会不会撞同一堵墙，
  也是资源泄漏的第一现场。新卡摆在熔断卡正下方，一行一个窗口：dbnum、窗口名、会话区间、
  档位、体量。体量报的是**合计**（暂存 SQL + journal），因为配额判的就是合计，两个分量
  塞进 title——分开报会让人拿单个分量去对阈值。字节与行数摞成两行而不是挤在一行：
  `ResourceThresholds` 里 `*_bytes` 与 `*_rows` 是两组各自判档、取高者的阈值，两个数
  谁也不能省，而一行在这一列上会把 `1.19 GB · 812,344 行` 截成 `812,34…`——截掉的
  恰好是数量级那一头。列宽按 480px 排：`.page` 封顶 1440、`.col-r` basis 就是 480，
  任何 ≥1440 的屏幕上这张卡都恰好这么宽，是常态不是极端。
  `state = writeback_stalled` 的行另起一行
  摊开写回原话：那是「窗口没放掉」而不是「正在跑」，两者在表格里长得一样。同屏还带
  `staging_window_blocks`（`increment_update_attempt` / `window_block`，**跨重启活着**，
  内存窗口清空之后它是唯一还说得出话的证据）与 `staging_commit`（上次提交耗时与重试次数，
  摆脚注不摆表里——窗口提交完就没了，这两个数还在，进表会被读成「还有一个窗口」）。
  三条免责声明钉在卡上，少一条这一屏就会骗人：**空不等于干净**（没有批次在跑、这一批走
  直写没建窗口、窗口刚被废弃，在这一栏上是同一个空白）；**这一份活在进程内**，重启必然
  为空，那时该问的是水位表；**档位阈值来自 `AIOS_STAGING_*` 环境变量**，面板不知道数值，
  所以只报档位、不画进度条——编一个百分比出来比不画更糟。字段缺席（老后端不报这一栏）与
  空列表分开渲染，`staging_window_blocks` 落进 `degraded_sections` 时明说「此刻是未知不是零」。
  上次提交耗时为 0 写成「本进程还没提交过窗口」而不是 `00:00`：后者恰恰是「窗口一直没收口」
  的旁证，跟「提交是瞬时的」不能同形。
  落盘那一格（`BatchFailure.staging`）同批也画进了错误日志卡：残留时逐个列窗口名、`sesno`
  区间、状态与档位，写回卡住的另附原话；空时明说「已无暂存窗口挂着」，并点出它是**记录时的
  快照**、与上面那张进程内的卡不是一回事——它活得过重启，而 8191 现场恰恰是重启之后才来问的。
  档位在两处走同一张 `SW_BAND` 表：一处写「拒绝扩窗」一处写 `refuseabsorb`，读的人得先花时间
  确认它们是不是同一件事。

- **`init_mdb` 不再是空壳：启动时从 SYS 库文件解出本 MDB 声明的库清单**。
  「哪些字典库适用」是 MDB 的属性而不是项目目录的属性——`/ALL` 声明的六个字典库
  有四个在 `AvevaCatalogue` 与 `SCB` 底下，靠扫目录猜会两头错（第一次手工建 UDA
  快照就漏两个、多一个），而且漏一个字典库跟「这个 UDA 没值」长得一模一样。新增
  `src/data_interface/mdb_membership.rs`，在 manager 进 `Arc` 之前解析 `CURD` 并
  登记进程级注册表，按 `STYP` 分类取用；它读文件而不是 SurrealDB，所以在任何东西
  被同步之前就能回答（`UpdateScope` 那条查询要等 SYS 先同步，拿它定字典范围是
  死结）。声明了却不在盘上的库逐条告警——那是部署缺件，不是「本 MDB 没有这个库」；
  解析失败只告警不中止（尚无消费者）。DICT 的 `STYP` 按 AMS SYS 实测钉为 8，
  aios_core 的 `DBType::DICT = 6` 在该库里一个都配不上。文件定位复用 extract-family
  命名解析（ADR-028）而非后缀匹配：`ends_with("100_0001")` 会把 `ams8100_0001`
  也吞进来无声配错库，无后缀主库与非 `_0001` 抽取又是同一逻辑库的合法身份、原先
  永远配不上被误报缺件；抽取叶子压过主库，多命中先排序再取，两条回归测试各钉一个
  缺陷。附 `mdb_dict_probe`：独立跑同一套解析，给 `e3d-descriptor emit-uda-table`
  打印现成的 `--dictionary-db-list`（`CURD` 序，`UKEY` 首定义胜出）。

- **`/health` 新增 tokio 调度延迟，CPU 段挤掉调度这件事第一次有数**（specs/033 T003）。
  ADR-052 说要把几何 CPU 段挪出 tokio worker，理由是三角化和布尔占住 worker 会挤掉
  shape receiver、SurrealDB response、watcher、`/health` 与 timer——但这句话在改动前
  无法证伪，手上只有「/health 感觉有点卡」这种印象。新增 `src/runtime_lag.rs`：一个
  只睡觉的采样任务，量自己「睡 100ms」实际睡了多久，超出的部分就是它排队等 worker
  的时间，在 `run_cli` 定死几何额度之后立即起（`run_app` 转手调 `run_cli`，`OnceLock`
  保证只起一次）。每轮独立计时、不补追落后的轮次：一次 3 秒卡顿应当留下一个 3 秒的
  样本，而不是三十个越来越小的样本把 p50 拉回正常。512 样本的滚动窗口之外单独保留
  进程期最坏值——现场最狠的那次卡顿常常发生在几十分钟前，只留滚动窗口等于没留。
  `sampling: false`（采样没起来）与「延迟为 0」在 `/health` 上分得开；分位数复用
  `model_concurrency::percentile`，两个区块的 p95 是同一把尺子。测试里顺带钉住一件
  容易误读的事：五个样本撑不起 p99，最坏值只能看 `max_micros`。

- **几何闸开始记许可持有时长，闸利用率第一次算得出来**（specs/033 T002）。原来只有
  在飞、在等和累计**等待**时长，没有累计**持有**时长——于是「额度开到 8 到底兑现了
  几路」这个问题在现场无法回答，只能靠 CPU/wall 事后倒推。现在许可从拿到手那一刻起
  算持有，`GeometryConcurrencySnapshot` 多出 `active_permit_micros` 与
  `observed_micros`，利用率由 `utilization_since` 对两次快照做差得到。刻意不提供
  「进程至今」的平均利用率：现场的形态是模型本体 258s 跑完、之后等死信干耗 22 分钟，
  把空闲摊进分母得到的是一个既真实又没有意义的小数。计量同时从模块级 static 收进闸
  实例，单测里的临时闸各记各的，读数不再被同一个测试二进制里并行跑的用例污染。
  三条新测试钉住：利用率大于 1 不夹回去（那是计量出错的信号，藏起来等于把 bug 显示成
  100% 健康）、墙钟增量为 0 时返回 `None` 而不是编个 0、排队时间不计进持有（额度 1
  时持有不得超过闸自己的墙钟，这条上界是结构性的，不看机器快慢）。当前许可仍罩着
  整个 `Future`，所以这个读数现在是「许可被占住多久」而不是「CPU 忙了多久」，两者要
  到 ADR-052 的执行域切换落地之后才重合——模块文档里写明了，免得被当成 CPU 利用率
  写进结论。

- **ADR-052 转 Accepted，`specs/033-geometry-execution-domain` 补齐计划与任务**。
  几何并发额度此前装在整个 `Future` 上，一张许可里同时罩着 SurrealDB 查询、跨
  `.await` 持有的暂存 mutex 和同步文件写，于是 `active` 与真实 CPU 占用脱钩、
  16 张许可可以全在等同一把暂存锁（容量倒置），现场 `permits = 8` 只兑现约 2.3 路。
  plan 按 ADR-052 六条决策拆成六个阶段（可归因基线 → 执行域地基 `run_gated_cpu`
  → `manifold_bool` 首站 → 其余叶子与动态领取 → SQL 攒批解耦与 shape 观测
  → 控制器归位与正式 A/B），tasks 三十三条逐条带文件路径与并行标注。Constitution
  Check 记下两处偏离：新增专用有界执行域与 `model_concurrency` 模块头「不创建新线程池」
  的措辞冲突，以及 ISSUE-023 未闭合时「进程内全局额度」在 CentOS 7 上不成立——
  后者登记为发布阻断项，不是可以边做边看的风险项。与 `specs/032` 的排他也写进计划：
  `cata_model.rs` 与 `gen_model.rs` 同一时间只允许一条线在改。

- **属性 schema 换成描述符权威表，noun 从 339 涨到 1878，已有取值一条没变**。
  `all_attr_info.json` 由 `e3d-descriptor emit-attr-info` 从 20 个 `*vir.dat` +
  `attlib.dat` + 逻辑目录合成，`--legacy-attr-info` 把老表覆盖到的地方逐字照抄，
  所以「不回退」是构造性的而不是靠测试兜：老表 6556 对在 `name` / `hash` /
  `offset` / `att_type` / `default_val` 五个字段上逐条相等。规模落在计划靶心：
  20 库 / 1875 noun / 35734 三元组 / `flat_conflicts` 恰好 14。产物 35655 对，
  比 2026-08-26 那份中间件多 1023 对——那 1023 条是 `TYPE` / `LOCK` / `OWNER` /
  `CLAIDB` / `UDNA` 这类描述符里没有的结构属性，中间件把它们丢了，新表按
  「老表 ∪ 新表」保留。代价写在这里：`--legacy-attr-info` 保证不改值，也就保证
  不纠错，`SLOREF`（表说 STRING、描述符说引用）与 `MDSYSF`（表说 ELEMENT、
  描述符是 21 字的非引用）两条错误类型原样留着，测试里具名钉住。

- **新增离线核对设施：一个元素的全属性（含 UDA）第一次能不连库复现 `q att`**。
  `all_attr_info.json` 描述不了 UDA——UDA 不在 `*vir.dat` 里，E3D 把它存成字典库
  下的 `UDA` 元素（`UKEY` / `ELEL` / `DFLT`）。新增 `src/uda_table.rs` 读
  `e3d-descriptor emit-uda-table` 导出的字典快照，`full_attribute_view()` 把
  schema 默认值、UDA 字典默认值与记录里真存的值三层叠成一张表。**运行时取 UDA 的
  路径一行未动**：仍是每个项目自己的字典库同步进 `UDA` / `ATT_UDA`。快照是夹具不是
  配置，两张表都作为参数显式传入，不设隐式默认——UDA 定义属于某一个 MDB，能被环境
  悄悄替换的默认值在这里等于埋雷。夹具 `tests/fixtures/AvevaMarineSample_uda_info.json`
  跨三个项目五个字典库：`:SCHrefHole` 来自 SCB、`:PFILoose` 一家来自 AvevaCatalogue，
  只取 AMS 自己的字典会缺 BRAN 27 个 UDA 里的 5 个。
  `tests/full_attributes_real.rs` 拿 E3D 实打实的 `q att` 当权威，覆盖两条路：
  PIPE `=24383/73958` 79 行全部复现（20 个 UDA 全是字典默认值），BRAN
  `=24383/85432` 覆盖存值那条（`:ROOM_NO = NB122`，`UKEY` 离线解名，不查
  SurrealDB）。诊断入口 `src/bin/refno_attr_probe.rs`，另带 `--scan-uda` 全库扫描
  ——7999 库 44535 个元素里 1858 个真的存了 UDA 字节，所以那条路不是理论存在。

- **连败卡上加「只重扫这个库」，把全范围那一推收窄到一个 dbnum**。此前面板只推得动
  全范围（`POST /update/execute` 空体），而人在连败卡上想做的事从来只是「把这一个再试
  一次」——为了一个库去扫全范围，代价是别的库跟着入队、水位跟着动。服务端其实早就认
  `dbnums[]`（ADR-020 第 3 项的子集选择），缺的只是面板这一侧的入口。
  **名字叫「重扫」不叫「重跑这一批」**：它不是把那段冻住的会话窗口原样重放，而是从水位
  重新扫一遍再入队；文件没长出新会话的话算出来还是同一个窗口，于是确定性失败会一字不差
  地再来一遍。这句话与「SYS meta（SYST/DICT/GLB/GLOB）不是可勾选对象、永远随批」一起
  写在确认行上——不说的话，点完看到同样的报错，人会以为按钮没生效；而「只推这一个」
  在 SYS meta 上根本不是字面意思。回执新增 `本次没勾 N` 一栏（`unselected`，只在带
  `dbnums[]` 时才有），否则看着「入队 1」会以为其余的库也被顺带处理了。
  按钮**不自己发请求**，只是把页顶那一行确认填好：写操作全页只有那一处出口，确认样式、
  二次确认、回执展示都在那儿，在卡片里再写一遍就会有第二套。那一行在页顶而这张卡在半屏
  以下，所以点完顺带滚过去——确认摆在看不见的地方等于点了没反应。`act()` 的确认态因此
  改为按 **key + 目标**一起认：只认 key 的话，先点 8191 再点 3021，第二下会被当成对 8191
  那一下的确认，而这两条命令推的是不同的库、屏幕上却只有一行请求在变。`?readonly=1`
  下这个按钮不长出来，与顶部那排同一道门。

- **错误日志卡加 `task_id` 与时间范围两个筛选维度，并把「筛的只是取回来那一窗」说出来**。
  此前只筛得了 `event` 与 `dbnum`，而排查里最常问的两句是「**这一次**任务在磁盘上留下了
  什么」和「**只看最近这一阵**」。`task_id` 用包含匹配——真实 id 是
  `db-20260827-114844-000000` 这种，要人一字不差敲全等于这个格子没人用；任务下钻里那颗
  `→ 这一条的落盘记录` 填的则是整条 id，包含匹配一样命中。时间范围给四档现成的
  （不限 / 近 1 小时 / 近 24 小时 / 近 7 天），下界按此刻算：这一格问的是「最近这一阵」，
  不是一个冻住的区间。
  类型计数改为按**其余筛选之后**的集合算：勾着「近 1 小时」时「批次失败 12」若数的是
  全窗，点下去只出来 2 条，那两个数里必有一个在骗人。
  同批补上两句这张卡一直欠着的话。一是**窗口边界**：筛选在前端做（切一下不用等一轮网络），
  所以它筛的是取回来那 40 条而不是磁盘上全部；窗口一旦是满的，任何筛选下的「0 条」都
  可能只是被切在了外面，此时把去磁盘上重问的那条 URL 拼好摆出来，并注明服务端只认
  `kind` 与 `dbnum`——`task_id` 与时间范围它不认，这两个维度天然只能在窗内筛。二是
  **park 记录没有 `task_id`**（它记的是某个库连败到上限，不属于哪一次任务），所以按
  `task_id` 筛必然把 park 全灭掉；不说的话，「这条任务没被 park」与「park 压根不带
  task_id」在这一屏上是同一个空白。跳转一律先把其余筛选清干净再落一个：留着上一次的
  （比如还开着「近 1 小时」），跳过去多半是一屏「没有记录」，而人会把它读成「这个库没出过错」。

### 修复

- **初始化解 MDB 声明时的同号选主，改用摄入侧那套项目排名**。`/ALL` 的成员按 dbnum
  声明，而 dbnum 只在项目内唯一——`AvevaMarineSample` 与 `AvevaCatalogue` 在同一份
  配置里就同时有一个 7000。过去 `mdb_membership::locate` 把所有配置项目的库目录混成
  一个列表、只按号匹配，靠**路径字典序**决胜，两个分支方向还相反：抽取叶子取字典序
  最大的路径、无后缀主库取最小的。AMS 那份 7000 能被选中纯粹因为
  `AvevaMarineSample` > `AvevaCatalogue`——改一个项目名、给外项目一个 `_0002`、或者
  现场用无后缀主库部署，同一份声明就会静默绑到另一个项目的文件上，**而这份名单正是
  UDA 字典的来源，从错的项目读字典跟「这个 UDA 没值」长得一模一样**。它同时与摄入侧
  各说各话：`select_catalogue_candidates` 早就按 `catalogue_project_priority` →
  `included_projects` 的顺序选主并打 `[manifest] … 被项目优先级遮蔽`，一个进程里对
  「7000 是哪个文件」给出两个答案。现在 `locate` 拆成「项目内按 ADR-028 挑」加
  「跨项目按同一套排名选主」；SYS 记录里一直被解析掉的 `PROJ` 也留了下来，压在排名
  之上决定**先看哪个项目**，且**只排先后、不做排除**——`*MDU/CATA`(7355) 声明
  `PROJ=3` 而 `AvevaMarineSample` 是唯一有文件的项目，硬分桶会把一个解得出来的库
  报成部署缺件。`PROJ` 也确实只够用来排先后：`AvevaCatalogue` 的库与未部署的
  `SCB` 整块（`6000`–`6003`）读出来都是 3，它区分不出是哪一个外项目。落选的同号
  候选不再静默丢弃（`MdbDatabase::shadowed`），文件归属项目一并记下
  （`MdbDatabase::project`），启动日志与 `mdb_dict_probe` 逐条打出来。
  `the_ranking_agrees_with_the_ingest_side_adjudicator` 把两边摆在一起断言同一个
  赢家，谁改了任一边的口径谁红。实测沙箱：7000 解成
  `[AvevaMarineSample] ams7000_0001` 并遮蔽 `acp7000_0001`，与 `[manifest]` 那一行
  逐字对上。

- **`init_mdb` 的字典库计数不再自相矛盾，缺件自己点名**。过去那一行左边写
  「按 STYP {8: 6}」、右边写「其中字典库 5 个」，而唯一解释这 6→5 的那句话走
  `log::warn!`——`enable_log = false` 时 logger 压根没初始化，2026-08-27 沙箱的
  stdout 与 stderr 两份日志搜下来零命中，差额就这么在同一行里自相矛盾地摆了一整天。
  现在报数行同时给出「盘上几个、缺几个」，缺的字典库逐条 `println!` 点名（带 dbnum、
  名字、`PROJ`，并写明它定义的 UDA 之后一律读成「没值」）。非字典缺件合成一行：
  它们不像字典那样会把缺失伪装成一个合法的空值，逐条铺开只会把上面那几行冲掉——
  而合成一行又恰好让 `6000/6001/6002/6003` 这一整块 `*MASTER/SCB*` 并排出现，
  「SCB 这个项目整个没部署」的结论自己就跳出来了，过去它们分散在五条看不见的 warn 里。
  `log::warn!` 保留给有 logger 的部署。一条读自己源文件的顺序断言钉住三件事：报数行
  必须同时给两个数、缺件必须排在它之后逐条点名、且那一段里必须有 `println!`。

- **面板上所有 `<details>` 此前每 2 秒自己合上一次**。`render()` 是整块换
  `innerHTML` 的，而它每个轮询周期都跑一遍，于是刚点开的「原始记录」「伴随告警」在人
  把那段 JSON 选中之前就没了。这块折叠区恰恰是为「排查到最后总要把原话贴给别人」准备的
  ——它打不开，等于没有。现在展开状态记在 `S.openDetails` 里，重画时照着把 `open` 写回去；
  键取记录自身的身份（`task_id`、`dbnum` + 时刻）而不是序号，列表一重排序号指的就是另一条
  记录了，人会看着一段 JSON 在眼前换成别人的。`toggle` 不冒泡，所以监听挂在 document 的
  捕获阶段。错误日志卡、连败账本、新的任务下钻三处共用这一条。

- **model_impact 对账快照跟上 descriptor 属性表**。339→1878 换表（d32b06ff）让六个此前
  「字典有名、快照无属主」的属性有了属主——CACHID / LCHKDA（业务元数据表）、FSPREF / SCREF
  （级联表）、PTOF / SIZE（直接几何表）——而
  `curated_tables_are_reconciled_against_the_runtime_schema` 的钉死名单没跟着走，全量 lib
  一跑就红。按新 schema 重钉：DATA_ONLY 只剩 FUNCTION，级联 13→11，直接几何 39→37。
  这条测试的价值恰在于此：换表时逼着人把「哪些名字从此有主了」过目一遍，而不是让对账悄悄失真。

- **RVM 对拍里那条「什么都没量到也报绿」的门补上了下限**。
  `mesh_pipe_surface_distance` 是取证型用例，此前**一条断言都没有**：RVM 里找不到 group
  就 `continue`、库里没有生成几何也 `continue`，于是「九件件件贴合」与「一件都没量到」
  共用同一个退出码 0。2026-08-24 的作废通告里那唯一一条「1 passed」就是这么来的——在一个
  没有 1112/8000 生成几何的库上，它是八条里唯一报绿的，而它恰恰什么都没量到。现在结尾
  断言九件件件量到、跳过谁点谁的名。顺带把这条门的口径限制写进注释：C-OR 九件在 E3D 侧
  全是 12 三角，两个 BEND 也一样（gen 侧 618 / 908 三角），所以它量的是「gen 细网格贴不
  贴 E3D 粗替身」，量不到真实弯头形状；弯头的可信判据在
  `mesh_branch_union_surface_distance` 与 `mesh_full_branch_union_surface_distance` 的整段
  union 上（腿归属差在装配层自洽抵消）。同轮八条 mesh 对拍在
  `.surreal/ams-rvm-rebuild-20260824` 上 **8 passed / 0 failed**（44.1s），GWALL 三个可归因
  读数与 08-25 逐位相同——自那以来落地的改动没有动到几何。

- **失败批次在控制台上自己说出原因，不再把人指向一个当时拿不到的回执**。完成行只报
  `状态=failed`，真话在 `result.batch.message` 里，而那句话此前**只**进任务回执，屏幕
  上唯一的提示是「失败原因见本批回执」——取回执要 HTTP，现场可能没开 `http_api`、端口
  不通、或者人根本不在那台机器前面。2026-08-27 的 SYST 8191 就是这个形状：一整屏阶段
  日志把「死在收集增量这一步」说得清清楚楚，唯独缺了「为什么」，而收集口十几个各自具名
  的硬失败出口只能靠那一句分辨。现在 `render_failure_reason_lines` 紧跟完成行打印，三条
  纪律各带一条测试：① `batch` 缺席（冻结重扫就失败、批次压根没建起来）时回落到
  `warnings.last()`，并**报出这一句是从哪儿来的**——两个来源权威性不同，混成一句会让人
  把净窗口的口径标注当成错误；② 伴随告警只出前 3 条并报剩余条数与 `/api/v1/tasks/{id}`
  ——一条口径标注就上百字，整串打出来会把上方的阶段行冲掉，而阶段行正是判断死在哪一步
  的依据；③ 一条都没有也仍留一行，点名「引擎缺陷」，静默失败比错误的原因更难查。成功
  批次一行都不加。「初始化批次未收口」那句同步改口指向下方的原因行，源码顺序断言钉住它
  不许与「见本批回执」并存。

- **模型队列整页空转：生成器回报的根与队列行的 `target_refno` 拼法对不上**。
  2026-08-27 现场 1462 个根连着 118 页 `page_claimed=100 / page_completed=0 /
  remaining=1462` 烧掉 50 分钟，而每一页的任务都报 `succeeded`、`/health` 一路 `ok`、
  `attempts` 全是 0、死信是空的、日志一个字都没有。根因是同一个身份的两种写法：队列
  行存 E3D 的 `24381/100817`，而 `TargetedGenerationReport` 里那一串经 `RefnoEnum`
  走了一趟、`Display` 打成 `24381_100817`。`run_regen_group` 拿两边直接比字符串，于是
  `completed.contains(&job.target_refno)` 恒为 false（`settlements` 为空 →
  `clear_regen_work_batch` 一行不删 → 队列行原样留着被下一页再认领），`failures` 里那
  一串也配不上任何 job（`record_failure` 一次不调 → `attempts` 永远 0 →
  `MAX_ATTEMPTS` 永远够不着 → 死信永远是空的）。「收不掉」与「没失败」同时成立，所以
  它安静得跟正常跑完一模一样。这行比较由 `5f7ef21f`（08-26，合批生成那个特性）引入，
  两侧同出一个提交，事故发生在 08-27——不是潜伏很久，是落地即坏，第一次跑大批就现形。
  现在两边都过 `root_identity_key` 归一再比；解析不出来的
  **原样返回**，不能 `unwrap_or_default()`——那会把所有坏值折成同一个零值，两个不同的
  坏根反而被认成同一个。`settlements` 本身仍存队列行自己的写法：它要拿去寻址那一行，
  不是拿去比（ADR-011 队列纪律：寻址按行内实际字段）。单测钉住两种拼法归一、不同根不
  许归一、解析不出的原样留着，外加调用点守卫（改回直接比字符串即红）。
  `live_bran_pending_is_actually_regenerated` 一比一复现了这个形状；归一后 @8019 六条
  相关 live 全绿（BRAN / HANG / ZONE-EQUI / `live_generation_failure_keeps_pending_and_watermark`
  / `live_failed_queue_cleanup_does_not_stall_the_rest` /
  `live_non_regen_drain_consumes_the_whole_queue`）。证据
  `docs/evidence/2026-08-27-model-drain-root-key-mismatch.md`。

- **模型页整页空转不再无声：未回报的根按失败计次，页级停滞进 `/health` 与启动等待行**。
  上一条那种「收不掉又没失败」此前没有任何东西会发现，两道兜底同批补上。①
  `run_regen_group` 里生成没报错、根却既不在完成名单也不在失败名单时，那一行无声落地，
  下一轮被原样认领、再无声落地一次。新增 `settle_unechoed_roots`：点名（有界样本）并走
  `record_failure` 计次，撞满 `MAX_ATTEMPTS` 进死信。这与 2026-07-30 审计 C2「收口失败
  不涨 attempts」是两回事——C2 防的是 flaky 的 DELETE（根已经进了收口集合，下一轮重试
  就好），而这里根本没有 DELETE 可言，根压根没进得了收口集合，重跑一万次结果都一样；
  `settlements` 因此必须先并进 `disposed` 再做清扫，源码顺序断言钉住，否则 C2 修掉的
  那个 bug 会原样回来（`live_failed_queue_cleanup_does_not_stall_the_rest` 复验通过）。
  ② `ModelDrainTelemetry` 那三个数（`last_page_claimed` / `last_page_completed` /
  `last_remaining`）一直都在，却没有任何人拿它们做判断。新增 `starved_pages` 在认领下
  一页时结算上一页，三个条件缺一不可（认领过、零收口、待办总数纹丝不动）——只看
  `completed == 0` 会把「整页被根锁挡住」这种正常让位算进来，只看待办不变会把「收掉几个
  又排进来几个」误判成停滞。连撞 `MODEL_DRAIN_STARVED_PAGES_ALERT = 3` 页（现场每页约
  26 秒，≈80 秒）即判定停滞，推进 `/health` 的
  `blocking_conditions.model_drain.page_starved`（`degraded`）。启动等待行的「数字不变
  就退到 300 秒一行」这条退避在卡死时是反的，越卡越安静：现场前 19 分钟每 60 秒一行，
  真卡住之后反而变成 300 秒一行。停滞一旦确认就按 60 秒照常出声，并把停滞页数带上。

- **启动覆盖回填写进队列的根拼法与其余入队方不一致**（ISSUE-024 名单里扫出来的第三处）。
  `sync_and_seed_model_coverage` 用 `pe_thing_to_refno(..).to_string()` 落 `target_refno`，
  那是 `RefnoEnum` 的 `Display`、下划线 `A_B`；而增量、级联、按需生成三条路走的都是
  `to_pdms_str()` 的斜杠 `A/B`。`record_id_of` 会把斜杠折成下划线，所以两种拼法抢的是
  **同一行**——不会长出重复行，但字段值变成「谁最后 upsert 谁说了算」。代价落在那些拿
  字符串精确查这个字段的地方：`staged_settlement_revision` 经 `current_regen_revision`
  查 `target_refno = '<斜杠>'`，撞上一行当前存着下划线就命中零行、返回 `Ok(None)`——与
  「本来就没有这行」完全无从区分，于是本窗口跳过收口，那个根在提交后的空闲轮里被原样
  重生成一遍，而只有 `Err` 那一支会出声；`retry_pending_unit`（死信人工复活的唯一 HTTP
  出口）同样按精确字段寻址，拼法不对读起来就是「这行不存在」。现在统一成斜杠。新增一条
  测试先钉住前提（两种拼法确实不同、且折成同一个 record id），再用源码断言守住这一处
  不许退回 `Display` 拼法。**库里已存的行不迁移**：`record_id_of` 归一保证下一次入队会
  把字段改写成斜杠，从没再被入队过的行仍是旧拼法，根治见 ISSUE-024 的 newtype 方案。

- **WS 事件流的掉帧终于露得出来：滞后错过的那几条也要占号**。信封的 `seq` 是在
  **发送时**才分配的，而慢消费者滞后（`RecvError::Lagged`）那一臂什么都没做，于是
  丢帧之后编号照样连续——客户端「相邻两号差 1 就是没丢」的判断永远为真。这比没有
  掉帧指示器更坏：它是一句错误的保证，一条残缺的事件流在面板上与完整的流长得一模
  一样，而 `ops.html` 的实时事件面板正是照着模块头那句「滞后跳帧只体现为 seq 空洞」
  建起来的。现在滞后把错过的条数补进 seq，空洞真的出现在客户端那一侧，它据此提示
  「以 REST 读数为准」。计数偏保守：`missed` 含订阅之外的事件，那些本来也不占号，
  所以空洞可能略大于真实丢失——宁可说「最多漏了这么多」，也不能像原先那样说「一条
  没漏」。编号收进 `SeqCounter`，只增不减、饱和不回绕（回绕会被读成「服务端重连
  了」），撞上限也不 panic：一个计量数字不配把整条事件流拽下水。两条测试：编号语义
  （退回空分支即红）、以及滞后那一臂确实在跳号的源码断言（纯函数钉不住「有没有人
  调它」，而这个缺陷的形状恰恰就是那一臂空着）。

- **闸利用率不再把「正在算」读成 0：许可持有改为读时结算**（specs/033 T002 补）。
  持有时长原先只在许可 `Drop` 时累加，快照落在长布尔段中间时，正被占着的那几张许可
  一微秒都不计——单个 BRAN 布尔能跑几分钟，而 T004 的基线采集与 T013 的 0.7 验收线
  偏偏都要在 CPU 密集段内取样，读数被系统性压低，达标的配置会被判成不达标（额度 1、
  一张许可从头占到尾这种极端形态，旧写法读出来是 0.0）。账本改记两笔——已归还许可的
  持有之和、在飞许可各自的取得时刻之和——读数时补上「在飞张数 × 当下 − 取得时刻之
  和」。两笔是耦合量，各用一个原子量读会撕出一整段凭空的持有，因此收进同一把 std
  `Mutex`（取还许可的频次就是几何叶子任务的频次，锁内只有几条整数运算、不跨
  `.await`）；「当下」也必须在同一次持锁内取，否则在飞那项会算成负数被 saturating
  抹平回 0——那正是要修的形态本身。快照方法挂到闸实例上，单测的临时闸各读各的账。
  两条回归测试各钉一个形态（段中读数不为 0 / 一张许可占满整窗读作 1.0），退回旧写法
  双双转红。读数口径不变：许可仍罩着整个 `Future`，这是「许可被占住多久」而不是
  「CPU 忙了多久」，两者要到 ADR-052 的执行域切换落地才重合。

- **非 Windows 也有真实的进程单实例锁了**（ISSUE-023 / specs/033 T001）。
  `acquire_process_instance_lock` 的非 Windows 分支曾是一个空 `Ok(())`：部署到
  CentOS 后第二个进程直接放行，成为同一份 dabacon、同一个 SurrealDB、同一批 mesh
  的第二个写入者，几何闸额度、暂存串行、启动清理这些「进程内全局即全局」的前提
  一起失效。现在 Unix 走 `flock(LOCK_EX | LOCK_NB)`（`File::try_lock`）：锁挂在
  open file description 上，与手柄同生共死，SIGKILL 后由内核回收——刻意不做
  「看锁文件存不存在」那种会留陈旧锁的写法；冲突时读回持锁者写进文件的
  project/pid/started_at 一起报。函数体抽成 advisory 辅助在**所有平台**编译
  （Unix=flock、Windows=LockFileEx 同形），守卫测试从 `#[cfg(all(test, windows))]`
  改为三条跨平台——这个缺口的成因正是非 Windows 代码在 Windows 机器上零编译
  零测试。Windows 生产路径维持 deny-share 不变。真机 Linux 复跑仍待办（登记在
  issue），在那之前性能结论照旧只按 Windows 单实例口径引用。

- **UDA 快照的来源从「扫目录猜字典」换成 MDB 声明本身**。
  `AvevaMarineSample_uda_info.json` 重建为 `/ALL` 声明的六个字典库（用
  `mdb_dict_probe` 打印的 `CURD` 序清单），`all_attr_info.json` 同批刷新。「快照说
  适用但 `q att` 不打印」的台账从 2 条 `Psi*` 扩到 5 条（补进 `:MDSComment` /
  `:MDSDType` / `:MDSTrun`），按名字与数量双断言钉住；五条的共同点是归属应用
  （PSI 应力 / MDS 综合支吊架）而 `q att` 跑在 Design 模块下——模块过滤能解释它，
  但当前解码的字典元素里没有任何字段支持这个说法，先记为开放问题不猜。测试文档
  同时写明部署警示：`SCB` 不在 `included_projects` 里，`scb6002_0001` 会被报成
  不在盘上，而 `/ALL` 声明了它、`q att` 也真打印它的 `:SCHrefHole`。

## 2026-08-26

### 修复

- **启动模型覆盖核对补上未限定域的口子，且不再中止启动**（ADR-050 补偿闭环）。
  ADR-050 的启动清空是无条件的，但补偿（生成根凭证核对与补种）此前只挂在监听
  限定域上：未声明 `watch_dbnums` 的进程在模型积压期间重启，剩余工作单被清掉后
  没有任何东西重建它们——watcher 重扫按水位比对不再排模型工作，`model_ready`
  仍按 pending=0 判真，缺的模型静默消失。现在启动核对由
  `reconcile_model_coverage_at_startup` 统一承担：限定域照旧逐库
  `fn::sync_gen_roots` + 补种；未限定域改按 `gen_root` 凭证点查（`gen_root_dbnum`
  索引，一次 GROUP BY 列清单），发现凭证过期才对那一个库做完整 sync + 补种——
  全库跑 `fn::gen_root_cover` 在 448 库的现场付不起；没有凭证记录的库当面播报
  「核不了」，不许静默当作完整。同时任何一库核对/补种失败都只告警不再 `?` 上抛：
  原写法会把一个 watch dbnum 的配置手误（或 `sync_live=false` 空库首启）变成整个
  服务的启动崩溃循环。内存库测试钉住清单查询跳过无归属行，源码测试钉住
  「不得只挂限定域」与「不得中止启动」两条。

- **人工执行在宽松身份冲突档下不再虚计阶段总数**。watcher 路径对「仅记录并跳过」
  的库会 `manifest_totals.retain(..)` 摘掉阶段总数，人工路径此前不摘：默认配置
  （`block_file_identity_conflicts=false`）下人工执行遇到身份异常文件，/health 的
  阶段 total 会为一个永远不会来的批次虚高一格——正是此前修过的「阶段就绪显示失真」
  形态。现在人工路径把这类库记入 `manifest_lenient_excluded`，收集总数时排除；
  两路径共用性测试同步加严。

- **模型生成的三处小收口**：根级失败摘要（「N model root(s) failed: 前三条」）三处
  拷贝合并为 `occ_generate::summarize_root_failures`；按需 ensure 在没有在飞重建时
  不再为 `reject_ensure_during_rebuild` 多付一次数据库往返；adaptive 并发额度裁决
  抽成纯函数 `next_effective` 并把原来只断算术常量的测试换成对裁决本体的六条断言
  （压力减半有下限、进展加一有封顶、空窗不抖动、压力优先于进展）。

- **空闲模型页的进度簿记收进一个类型**。`drain_where_cooperative` 此前用一个
  7 参自由函数在四个调用点重复上报任务 detail 与 telemetry，收口/让位/锁挡三种
  终态又各自手拼 finish——现在认领时定死的 task_id / queue_total / page_claimed
  钉进 `DrainPageProgress`，子阶段推进走 `report`、页终态走 `settle` /
  `settle_deferred_for_lock` 一处收口。行为逐位不变（detail 缺失时整个跳过的旧
  语义保留），既有源码钉（合批先于逐根循环、探测先于消费、锁覆盖结算顺序）与
  全模块 76 条测试复跑通过。

### 新增

- **新增属性描述双读对拍探针 `attr_info_compat_probe`**。它加载解析库生成的
  `PdmsDatabaseInfo` 兼容文件，先逐项验证内嵌旧 schema 全部保留，再用新旧两份
  schema 解析同一 Dabacon 文件的最新记录并逐属性比较，为逐步退役
  `all_attr_info.json` 建立不切换生产路径的验收门。

- **E3D TTY 与导出的 AMS Agent 使用教程**。集中说明当前权威入口宏通道、AMS 8000
  FTUB apply/restore、RVM geometry-only、RVM+ATT 成对发布、全局 noun/属性字典导出、
  证据与回滚判据，以及继续提交正常服务增量并检查 task、水位、staging、side effect、
  worker 和 health 的完整退出门。

## 2026-08-25

### 修复

- **`watch_dbnums` 同时收窄 `inst_relate` 缓存维护，启动不再被一次全表扫压住**
  （ADR-048 决策 2 的延伸，specs/025 的一段前置）。`insts_flat` 上没有索引，命中
  稀少时 `LIMIT` 一格也省不下来——引擎要把整张表走完才敢说「不足一批」，而
  `aabb.d` 是记录链接，每条 `insts_flat = NONE` 的行还要多付一次 `aabb` 点查
  （issue #21 在库 A 的普查：NONE 行里约 97% 是读者不可见的）。448 个 dbnum 的
  现场库上，启动序列停在「正在清扫 inst_relate 平表副本」不再往下走。现在回填
  圈行、脏值探测、RM13 修复循环与复核、老格式再现探针这五条全表谓词，在声明了
  限定域时一律带 `dbnum = …` / `dbnum IN […]` 前缀走 `idx_inst_relate_dbnum`；
  列名裸着出现，不套 `(dbnum?:0)`——那会把索引藏进表达式里让 planner 回落全表。
  **收窄跑过的 RM13 migration 不落完成标记**：标记的含义是「这一库全表收敛过」，
  域内干净担保不了它，落了域外那批老格式行就永远没人再看。收窄时启动日志点名
  限定域、来源，以及「域外的行与 dbnum 为 NONE 的历史行这一轮不维护、读侧走 slim
  兜底」。不声明限定域时逐位不变，全表扫照旧。
- **`project_dirs` 恢复为项目目录的同位置映射**。`included_projects` 继续作为唯一项目
  范围；提供 `project_dirs` 时，watcher、初始化解析、CATA 路径和进程单实例锁统一使用
  其中同下标的真实文件夹名，不再错误拼成 `project_path/included_projects`。映射缺项或
  非单层文件夹名会明确报错，名单外项目仍不会进入扫描范围。

- **7997 并发版现场启动修正**。生成根覆盖查询把 `subtree` 加入投影，兼容
  SurrealDB 2.1 对 `ORDER BY` 字段必须出现在 `SELECT` 中的限制；adaptive 的 RSS/写入
  基线改为在整个 K=1 观测期累计峰值，避免用进程刚启动的低水位样本把并发永久误压为1。
  `test-worklspace` 实测识别2720根、保留528个当前凭证、补种2192根，并观察到 K=1→2→3
  后按 AABB/RSS 压力回落，期间零失败、零死信。

- **7997 模型完整性与根级并发生成闭环**（ADR-011 / ADR-025）。启动数据阶段后用
  `fn::sync_gen_roots` 对齐权威生成根，完成凭证必须与当前 `applied_sesno` 及保存时刻
  一致；缺失根进入原有 `regen_root` 队列，`model_ready` 不再把 `pending=0` 当作模型
  完整。每页仍预取 100 根，但只按 16 根 execution group 获取根锁，mesh/布尔后半程
  有界并发、Shape writer 与 AABB 提交保持单路；健康接口新增模型、Shape 和 geometry
  并发遥测。新增 `POST /api/v1/dbnums/{dbnum}/model/rebuild`，范围校验、幂等任务、旧模型
  保留和水位不变均复用现有协调器；`legacy` 配置可即时退回串行并关闭端点。

- **7997 首次初始化的模型工作恢复有界消费**（ADR-011 / ADR-025）。
  自动空闲轮不再用无上限 `SELECT` 一次认领 18674 个 `regen_root`；Regen
  固定每页 100 根、后置 AABB 每页 256 条，Regen 清空前的二次探测禁止
  AABB 越过阶段。忙根锁原样延后且不消耗 attempts，全页忙时按 30s 退避。
  `model_drain` 任务只保留 10 个根样本并报告页进度；启动等待日志改为
  真实 Regen/AABB 阶段与剩余数，相同无进展信息最多每 300s 输出一次；
  `/api/v1/tasks` 单次最多返回 160 条，最大响应保持在 256 KiB 内。
- **模型开关不再导致每次启动整库全量生成**（ADR-051）。`gen_model` / `gen_mesh`
  只表示允许增量批次进入模型与网格阶段；服务启动统一经 watcher 重扫，把文件最新
  会话号与 `applied_sesno` 比对后，仅为首次导入、文件回退或真实增量建立工作。移除
  `run_cli` 中对 `gen_all_geos_data` 的直达与延期全量分支；显式探针/工具的全量入口保留。
  启动日志会明确播报当前是水位比对策略，源码回归测试禁止三个全量锚点重新进入启动路径。
- **停止逐会话打印 `[paged_db] path=...` 分页读取统计**。按需读取及快照校验逻辑
  保持不变，仅移除会话销毁时输出的页数、缓存命中和解析记录数明细，避免初始化阶段刷屏。
- **`watch_dbnums` 同时约束持久模型工作单的自动消费**。此前它只挡新数据批次，
  空闲轮仍会把 `model_update_pending` 里范围外的历史根全部捎进来；现场只监听 8000，
  却继续生成 1112 的 5025 个根。现在自动 drain、是否还有活、死信门和 `/health`
  共用 `(dbnum?:0) IN watch_dbnums`，本进程新建的范围外/无库号行不参与自动消费；
  不声明限定域时行为不变，当前批次的精确键 scoped drain 与只读待重试清单不变。
- **启动时先清空 `model_update_pending`**（ADR-050）。这张表只承载本进程内的模型
  调度，不是跨重启恢复日志；增量解析数据与模型数据已从 kv-mem 作为一个整体写回
  RocksDB，重启后重放旧工作单会把两个快照混在一起。现在数据库连接成功后立即全表
  清理，且严格早于空间树、预加载、db manager、watcher、worker 与模型阶段；清理失败
  直接阻断启动。启动日志报告实际删除数，定向内存库测试和源码顺序测试钉住该边界。
- **停止逐批打印 `shape_save_flush` 控制台明细**。实例保存仍按原阈值执行并累计
  `ShapeSaveRunOutcome`，每轮结束的 `shape_save_summary` 汇总保留，避免 MaxWait
  小批次持续刷屏。
- **启动数据身份冲突默认降级为“记录并跳过”**。文件名/文件头库号不一致、同项目
  同 dbnum 多文件仍不摄入、不写观察值，控制台与人工执行回执保留完整告警；但冲突库
  不再把 Meta/Catalogue/Design 阶段永久标成 blocker，其余库可以完成初始化。
  watcher 与手动执行共用 `block_file_identity_conflicts`，默认 `false`；需要恢复旧的
  严格阻断行为时在 `DbOption.toml` 设为 `true`。
- **声明了 `watch_dbnums` 就算启动上弦**（ADR-048）。`startup_autorun=false` 与
  `sync_live=false` 撞在一起时，重扫行挂起、持久积压不消化，而解封条件「某个 dbnum
  真的来一次增量」永远不会发生——watcher 压根没起。现场
  （`DbOption-rvm-rebuild` + `AIOS_STARTUP_AUTORUN=0`）`model_update_pending.retryable`
  卡在 7655 一动不动，进程 33 分钟只吃 74.9s CPU（单核 2.1%），
  `model_drain.last_claimed_epoch = null`：那 7655 行「可消费 / 可收口 / 可复活」
  三条出路一条都没有，对外还长得像「在慢慢跑」。起手上弦改由纯函数
  `batch_scheduler::initial_auto_work_armed(startup_autorun, watch_scope_active)`
  裁决，两个来源任一成立即上弦；限定域仍然只收窄不放宽，**也不拖回启动全量房间重建**
  （那道门直读 `startup_autorun()`，单独钉了测试）。两句启动播报补上第二条出路，
  并在 `sync_live=false` 时额外喊明「不会有文件事件来解封任何一行」。三条回归
  （真值表 / `global()` 源码断言 / 房间重建边界）均实测过「回退旧写法即红」。
  `db_options/DbOption-rvm-rebuild.toml` 同步写上 `watch_dbnums = [1112, 8000]`，
  重启实测：`model_in_flight` false→true、CPU 2.1%→14%，而 `startup_autorun` 仍为 false。
- **网格里不许留一条 f32 表达不出来的缝**（ADR-049）。CFLOOR
  `/1RS-WF03-F-C-F002` 一带三个负体在布尔入口报 NotManifold，三件三角化全成功、
  全死在读回——同一本账的三个常数：① `sin(TAU_f32) ≈ 1.75e-7 ≠ 0` 让回转体环向
  接缝两侧焊不到一起（CTORUS 实测 10 条边界边，与 `2RT+2T=64` 解出的 `T=8,R=3`
  逐项对得上），改由 `ring_angle_table` 建表、`j == segments` 回用 `j == 0`，
  `gen_circular_torus`（管壁与两个端盖共表）/ `gen_sphere` / `gen_spherical_dish` /
  椭圆封头四处接上；② `sin(PI_f32) ≈ -8.7e-8` 把球的南极摊成一圈针状面
  （`2 × slices` 条边界边），两极硬置 `(0, ±1)`——**这条是修 ① 时顺带查出来的，
  现场还没人撞上**；③ `assemble_ring` 的合并门槛 `POS_EPS = 1e-4` 是绝对量，而 f32
  在 1250mm 处的分辨率约 1.5e-4，f64 里刻意留的缝落盘即被抹平、焊回去多出三面共享边
  （`99813` 那个 `{3: 1}`），改为 `POS_EPS.max(scale × 1e-6)`。新增
  `a_revolved_seam_closes_bit_exactly_so_the_boolean_can_read_it_back` 钉 ①②，
  ③ 由现场夹具 `field_floor_negatives_can_be_read_back_by_the_boolean` 直接钉——
  该夹具三件现已全绿。
- 房间归属两条真库 live 用例的断言漂移（`live_issue7_real_db_deleted_edges_come_back` /
  `live_issue13_c2_moving_out_of_the_room_clears_membership`，2026-08-19 @8019 分别红在
  「实得 0」与「起点无归属边」）。两条都不是引擎回归，是用例自己的问题，各修一条：
  - **issue7 断言错了对象**：`drain_rooms(..).done >= 1` 是**本次调用**的吞吐计数，而共享
    实库上还跑着生产 worker，它的空闲轮 `room_round` 会先把队列行收走；更糟的是这条计数
    断言排在 `after_move == baseline` 之前，引擎即便完全做对，用例也说不出来。改为
    `wait_for_room_convergence`——轮询到 `room_recalc_element_{ELEMENT}` 消失为止，
    **不问是谁收的**，并把边集断言提到最前、收敛诊断塞进失败信息。判定刻意不看「进场时
    队列行在不在」：worker 可能在第一眼之前就收走了，拿它当判据等于把同一个竞态换个地方
    再犯一次。
  - **issue13-c2 用例不自足**：它把 `edges_of_element()` 的当前值直接当基线并要求非空，
    于是隐式依赖「issue7 先跑过」，同批次 #01 一红它就报一个不属于自己的前置阻断。新增
    共用的 `build_room_baseline`（备料两侧几何 → `rebuild_tree_from_pointers` → 只重建
    `-RM05-R512` 这一间）由两条用例各自铸基线，可任意顺序单独运行。指针重建不能省：
    `build_room_relations` 前面那道覆盖率门要树的条目数达到库内可用包围盒指针数的 90%。

  - **重建范围与对拍范围不一致**（跑之前没人发现，一连库就撞上）：两条用例都把
    `ROOM_KEY_WORD` 卡到 `-RM05-R512`（那间房只有 1 块面板）以避开全库两百多间房的
    重建，却把 `baseline` 取在靶件的**全库**归属边上。`python/testbed/.surreal/pytest-ams`
    上那个 CAP 同时挂着 `24381_35844 -> R512` 与 `24381_1391 -> R142`，于是增量最多
    只能复现一半，边集断言必红；更糟的是元素分支发的 `DELETE {element}<-room_relate`
    只避开 `protected_panels`（在册但缺几何的面板），R142 那块在这个 keyword 下压根
    不在册，它的边会被抹掉且收尾不写回——**跑一次少一条，共享沙箱里没人补得回来**。
    2026-08-06 那次能过是因为当时库里 `pe:24381_1391` 不存在，验证报告把它记成了
    脚注。现改为：断言收进 `scoped_panels()`（`24381_35842` 名下子 + 孙两层 PANE），
    范围外的边进场时按完整载荷备份、收尾最后一步原值写回。

  仓内已有同类先例（`live_shared_spco_expands_to_generation_roots` 改自足、
  `live_shared_spco_cascade_regenerates_every_consumer` 把钉死计数拆成动态口径）。

  验证：`db_options/DbOption-room-live-8029`（由 `DbOption-pytest` 拷贝，只改 v_port
  与 `room_incremental`）+ 8029 上的 `rocksdb:python/testbed/.surreal/pytest-ams`。
  两条各 20s 通过，日志复现 08-06 的黄金形态 `无房间 -> R512` 与反方向的
  `R142, R512 -> 无房间`；又按 c2 → issue7 反序各自独立进程复跑一遍仍全绿，顺序依赖
  确实断开。跑完核过：两条归属边载荷逐字回到进场值、`POS.z` 回到 5821.669921875、
  房间队列零行。唯一的持久变化是那块面板的成员边收敛到 632 条（全库 `room_relate`
  41370 → 41372），来自本轮的指针重建 + 定向全量重建，不是用例残留。
  台账两行已更新，日志在 `output/room-live-20260825/`。

- 直线扫掠体的 path 坐标系与实例旋转重复计（aios-core `0e391ff1`，本仓跟进三个 rev 到
  `0e391ff1` / `5344440b` / `257ea253`）。两个各自独立的缺陷叠出 STWALL 4
  （`pe:17496_105816`）整块墙绕 Z 转 90°：`create_profile_geos` 的 POSS→POSE 分支把
  **世界系**的 `pose - poss` 直接存进局部系的 path 字段（同一处的 `DRNS` / `DRNE` 都过了
  `inv_quat`，只有它没过；`height` 是标量所以长度一直对、只错方向）；`SweepSolid::get_trans()`
  又把 path 切向折进实例旋转，而两台建体引擎（manifold 的 `sweep_solid_mesh`、f9f1bf0
  删除前的 `gen_occ_shape`）对直线扫掠一律沿局部 +Z 挤、只取 `line.length()`——方向属于
  元素的 `world_trans`，再带一次就是重复计。只有走复用单位体的件会炸：另外三堵 STWALL 带
  `drns` / `drne`，`is_sloped()` 为真、`get_trans()` 返回单位阵，脏方向没人读。
  修后 STWALL 4 的世界 AABB 与 E3D 逐位相同、四堵墙双向表面距离全 0.00mm，
  带非单位实例旋转的行 63 → 61（消失的正是 STWALL 那 2 行）。
  八条 mesh 级对拍 6/2 → **7 passed / 1 failed**，仅剩
  `mesh_gwall_extra_against_cwall_union` 卡 105828（另一条线）。
  证据 `docs/evidence/2026-08-25-sweep-path-frame-fix.md`，台账已更新。

- 负体做差前的「让量」从 `1e-6` 等比改成**逐轴各向外让绝对 0.051mm**
  （新常量 `libgm_discretise::RES_TOL_MM`，就是 libgm 的 `GM_User::restol_`），
  收掉 ISSUE-022 那层外皮。`manifold_csg.rs` 里本来就有这一步，坏在量级与形状：
  等比量在薄方向上等于没让——那堵墙的负体沿墙厚只有 750mm，`1e-6` 给出 0.000375mm，
  比实测那道缝还小一个量级；而三个轴 2600 × 750 × 2180 差着一个数量级，等比放大
  要么长轴让太多、要么薄轴让不够。退化到零厚的轴不缩放。
  回归钉 `fast_model::manifold_csg::tests::a_negative_stopping_a_hair_short_still_opens_the_exit_face`
  ——出口停在差 0.01mm 处，挖穿了亏格 1、留皮亏格 0；退回旧等效量（该负体上 0.000055mm）
  实测即红（`genus=0`、`volume=3360061.89`，多出的约 62mm³ 正是那层皮）。
  现场（8009，删掉 8 堵带负体 GWALL 的 booled `.mesh` 就地重算布尔）：105828 gen→gwall
  p95 **753.9 → 0.1**、max 1296.9 → 65.2、三角数 188 → 184；105880 p95 9.4 → 8.9；
  116569 p95 147.4 → 137.3；GWALL union both mean/p95/hausdorff
  **10.53 / 8.44 / 1286.31 → 4.75 / 5.33 / 647.09**。八条 mesh 级对拍
  **7/1 → 8 passed / 0 failed**，台账已更新。

- 定位：八条对拍里剩下的那条红 `mesh_gwall_extra_against_cwall_union` 卡的
  `pe:17496_105828`，**不是摆位也不是尺寸，是一张本该挖掉的外表面还在**。逐三角形对照
  E3D GWALL 18：洞的两侧门垛（`[±1300, −17018, 1433]`）与过梁底面（z=2160）gen 都切出来了，
  唯独 y ≈ −16651 那张外表面，E3D 在 x∈[−1300,1300]、z≤2160 一个三角形都没有，gen 铺满。
  负体 `pe:17496_105841`（NXTR，HEIG=750）的出口面与墙体外表面共面（都在 y ≈ −16651.40，
  f32 在这个量级的 ulp 约 0.001mm），布尔把它留成一层外皮。两个数因此都能对上：
  max 1296.9 ≈ 洞半宽 1300、p95 753.9 ≈ 墙厚 748。`manifold_bool.rs` 里没有任何沿挤出轴
  给负体加余量的处理。开 `issues/ISSUE-022-coplanar-negative-leaves-outer-skin.md`，
  属 ADR-044「共面留一层壁」同族。

- IDA 取证（同日续查 ISSUE-022 的 ε 口径）：**libgm 的「多近算同一个」是 0.051mm，
  而 E3D 的负体根本不是三维实体 CSG**，证据
  `docs/evidence/2026-08-25-ida-libgm-coincidence-tolerances.md`。Core3D 建体前
  （`0x104da260`，MTR 标签 `adp_geometry/adp_gm_mk_body`；另一处 `0x108e6a80` 同值）
  连调四次：`gm_SetResolutionTolerance(0.051)` / `gm_SetDefaultNormalisationTolerance(0.051)`
  / `gm_SetDefaultTangentTolerance(0.0087266)`（0.5°）/ `gm_SetDefaultFacetTolerance(0.5)`
  ——最后那个就是本仓 `FACET_TOL_MM`，说明这处调用点正是我们该抄的那处。负体侧：
  `addStandAloneNegative` 建 `gm_CreateCombination(3)`，`GM_AggregateCombination::calcFacets`
  把 `restol_` 传给 `GM_CompFacets::aggregateWith`，真正相减在 `GM_Facets::obscureFaces`
  （libgm `0x10068710`）**面内做二维多边形相减**，切分 side 判定与 `D2_PolySet::normalise`
  都吃 `restol`。所以 ISSUE-022 不是「有条共面规则没抄」，是**「libgm 有 0.051mm 的重合
  容差，我们一个都没有」**——`plant_mesh_to_manifold` 焊顶点用的是 `to_bits()` 逐位相等。

- 同上取证顺带纠正一条仓内注释：`libgm_discretise.rs` 的 `NORM_TOL = 1e-6` 写着
  「没有人改它……运行期恒为初值」。成员写入器 `GM_User::normtol(double)` 确实零调用，
  但 Core3D 走的是自由函数 `gm_SetDefaultNormalisationTolerance`（同一处调用点），
  运行期真值是 **0.051**，差 51000 倍。`normtol_` 的读者是 `gm_CreateBody` /
  `gm_CreateNormalisedItem` / `gm_CreateFacetStructure` / `gm_QueryMass`，还管回转轮廓的
  轴心吸附。改它会动到所有回转体，**本次只记录、未动代码**。

- 补上两条原本谁也拦不住这类漂移的钉子：aios-core 的
  `world_path_direction_lives_in_instance_rotation` 直接断言的是旧行为，改写为
  `path_direction_stays_out_of_instance_rotation`（+X 的 path 必须得到 `Quat::IDENTITY`）；
  gen-model 新增 `fast_model::sweep_mesh::tests::a_reused_unit_lands_where_the_direct_build_lands`
  —— +Z / +X / 斜向三档 path 下，`sweep_solid_mesh(单位体) × get_trans()` 必须逐顶点等于
  `sweep_solid_mesh(原件)`。原有 `get_trans()` 用例的 path 全是 `Vec3::Z * k`，恰好是所有
  分歧点都退化的那一档；这条等价性还顺带盖住同族两处潜伏偏差（非 +Y `na_axis` 时的 plax、
  无条件乘 bang）。

### 新增

- T041 的 A / B 两组门**写成了真单测**（`src/fast_model/pdms_inst.rs`，`t041_` 前缀
  11 条）：判据先落地、实现随后，**6 条按设计红着**，逐条登记在
  `specs/009-retire-occ/tasks.md` 新增的「预期红测」一节（每条写明现在为什么红、
  转绿的条件）；另 5 条是不许变红的反向门。CI 口径全量
  **1119 passed / 6 failed / 85 ignored**，失败逐名就是那 6 条，无旁落。
  写测试时看清两件事：(1) 配对必须是「**同一形状比例、不同绝对尺寸**」——单位行的
  半径恒为 1，拿两个不同比例的件对比问不出「段数有没有进键」；(2) `t041_b1`（碟的
  三元组）**今天绿得不作数**——两键不同只是因为 `Dish::hash_unit_mesh_params` 哈希的
  是未归一化的 `prad`，等 `b1b` 把它收成比值这条才开始真的量三元组。`b1b` 顺带把
  T053 第 (3) 条那个「读码所见、未构造用例」的双键疑点变成了有用例的红测。
- T041 的门按 T053 的新范围写全（`specs/009-retire-occ/tasks.md`）：从「柱与球」扩到
  **五类**，并把此前没有的**元数**一维补上——每类混几个段数是不同的
  （柱 / 球 / Snout 一元，圆环面二元，矩形环面一元，球碟二元，椭圆碟三元），
  只混第一个数的写法 A 组门盖不住。B 组四条各带一对**实算出来的判别样本**：
  椭圆碟 `a=1000` 的 `h=5` 与 `h=20` 绕轴同为 100 而 `(hub,knuckle)` 是 (2,2) 与 (3,3)；
  圆环面 `rins/rout=0.5` 下 `rout=104` 与 `105` 环向同为 36 而管向是 16 与 20；
  球碟 `(100,2)` 与椭圆碟 `(100,2,2)` 前两位逐位相同（分支今天靠 `prad` 分，
  加段数时不许把变长元组摊平成不带长度/不带分支的写法）。
  另钉三条容易反向做错的：矩形环面**只有一元**（矩形截面无管向）、球**只混 n**
  （stacks 恒 `n/2`，IDA 已钉）、SSCL 与偏心 Snout 两支本来就带真实尺寸，
  **键必须逐位不变**、不得重复混。整体门 C4 用 T053 的脚本口径复核五类行数落在 474——
  偏多是混了不该混的，偏少是漏了一维。
- T053 范围盘点：**段数进身份键之后，五类复用曲面原语合计 392 行 → 474 行（+82）**，
  证据 `docs/evidence/2026-08-25-t053-segment-identity-scope.md`（库 A 一次性副本 @8039，
  `FACET_TOL_MM = 0.5`，规则逐行照抄 `libgm_discretise` 并先跑其单测对照表自检）。
  逐类：单位柱 1→44、`PrimCTorus` 95→102、`PrimRTorus` 167→174、`PrimLSnout` 112→133、
  `PrimDish` 17→21。量级与旧估同档，ADR-044 决策 2 与 D1「同批重建」都不必回开；
  plan 的 G3 / V5 与 tasks 的 T053 已按新口径改写。

- 盘点口径澄清：T045 的「柱 1 → 37」算在 `inst_relate.insts_flat[]` 上，那是带
  `geo_type = 'Pos'` 的**读侧投影**；而 `.mesh` 按 `geo_hash` 存、负体同样吃
  （`apply_cata_neg_boolean_manifold` 取操作数走 `geo_type == "Neg" or
  "CataCrossNeg"` 再 `record::id(out)`，与正体同一张表）。按 `geo_type` 拆开：
  Pos/Compound 392→460（柱 1→37，与 T045 逐位吻合）、负体族 392→432、
  全部吃 `.mesh` 的 392→474（柱 1→44）。**排期按最后一档**——差异集中在
  8,000–41,800 mm 的负体柱，负体分错段数正是 ADR-044 要治的「共面留一层壁」。
- `insts_flat` 的 11,992 个空数组判为**非缺陷**：逐行对着回填式的四道过滤分类，
  11,979 行（99.9%）的边全是负体、13 行有 Pos 边但全部不可见，「有合格边却仍为空」
  与「`booled_id` 有效却仍为空」两个缺陷桶都是 0。真残留在另一头——
  `insts_flat = NONE` 的 1,479 行里 **40 行对读者可见**，正是清扫段 `WHERE` 圈的、
  也是 `pdms_inst` live 断言禁止残留的那一类（三行还带 `booled_id`）。
  开 `issues/ISSUE-021-insts-flat-visible-none-residue.md` 记录，**当日查完判为非缺陷**：
  `queue_control` 上根本没有 `booled_flat_repair_migration` 标记（只有 `main`
  `paused = true` 与 `watermark_seed`），把清扫段 SQL 原样重放到一次性副本上
  40 行**一批归零**——既不是「标记落早」也不是「清扫有漏」，是一份冻结基线。
  同趟量出 RM13 修复 migration 在这库上有 **6,599 行在等**，原样重放 14 批 6,595 行
  收敛到 0。库 A 是**前 migration 的基线**：RVM 对拍走 `tessellate_libgm_param`
  不读 `insts_flat`，不受影响；但拿它起服务给人看，读侧会端出 RM13 那种错误正体，
  直到启动序列跑完。ADR-043 决策 5 在回填侧的脏位缝仍是真问题，但**不是这 40 行的
  成因**，留在 specs/025。

## 2026-08-24

### 新增

- IDA 取证：**挤出轮廓在 libgm 侧没有清理层**，`docs/evidence/2026-08-24-ida-extrusion-profile-no-cleanup.md`。
  `mth::mthArcFillet`（libgeom 3.1 `0x10043470`，Core3D 走同一份）只有五条早退——两条邻边
  退化、`|R| ≤ 1e-6`、θ≈0°、θ≈180°——过了就按 `T = R/tan(θ/2)` 硬算切点，**从不把 T 与邻边
  长度比较**，不夹取也不裁剪；`GM_Extrusion::calcFacets`（libgm 3.1 `0x10056f10`）的全部
  过滤器只有「按存的标志翻绕向 / 起止点位级全等才跳过 span / `fabs(bulge) >= 0.0000306`
  分弧直 / 圆面按 `normtol_` 去重」，不查自交、不查闭合，输出是「侧壁四边形 + 两个不三角化的
  n 边形盖」——E3D 本就不把挤出当闭合实体。结论是否定式的：**没有可照搬的 libgm 规则**，
  `flatten_profile_loop` / `extrude_flat_polygons` 现有注释与 `FillRule::NonZero` 选型至此
  有反编译背书。顺带改写了现场三件参数的性质：`FRAD = 4553.95` 不是超发值，它在 172.336°
  的近直角上算出 `T = 305.023`、邻边 305.030，是**故意做成弧接弧、直段长度为零**的轮廓，
  残留 ±0.0007mm 只是坐标三位小数的舍入。
- CFLOOR `/1RS-WF03-F-C-F002` 负体 NotManifold 的离线回归：
  `field_floor_negatives_can_be_read_back_by_the_boolean`（`src/fast_model/manifold_tessellate.rs`）
  + 夹具 `tests/fixtures/floor_wf03_bool_neg_not_manifold.json`（三件参数自
  `.surreal/ams-rvm-rebuild-20260824` 的 `inst_geo` 原样取下）。它把失败切成「三角化」与
  「布尔读回」两段分别报告：三件**三角化全部成功**、全部死在
  `manifold_csg::plant_mesh_to_manifold`，而对照组（单位箱 / 单位柱 / 方形挤出）同一往返全绿
  ——红的是几何本身而非往返。不连库、0.00s，进得了 CI。

- spec 025 T06（FR-9）：RM13 布尔平表存量修复从「每轮清扫必跑的常驻全表段」改制为
  **带库上标记的一次性 migration**（`run_booled_flat_repair_migration_on`，标记
  `queue_control:booled_flat_repair_migration`，流程「标记不存在 → 修复到收敛 →
  复核无残留 → 落标记」）。标记已落的库每轮只付一次 record id 点查；复核有残留不落
  标记、下轮从头重跑；旧备份恢复/库拷贝带回无标记状态即自动重跑。源码顺序钉 +
  mem 行为钉（标记落一次、再跑跳过、标记消失重跑）+ 双跑补标记语句形态；
  spec 019 状态注记同步。
- spec 025（`insts_flat` 失效协议）阶段 0 落地：共享 geo `bad→meshed` 反例 live 用例
  `live_shared_geo_bad_retry_must_refresh_sibling_insts_flat_on_disposable_db`
  （`src/fast_model/pdms_inst.rs`），在 8019 一次性内存沙箱按设计红——只对一行做定向
  重生成后，共用同一 `inst_geo` 的另一行 `insts_flat` 停在旧值（非 NONE），现有清扫
  两段都够不着（ADR-043 的缺口）。T02 判读「能复现」：FR-6 定持久 pending 表
  （选项 P）、FR-7 定反向失效（路线 B）；结论入 `specs/025-insts-flat-invalidation/plan.md`
  R1，留证 `docs/evidence/2026-08-23-insts-flat-invalidation/t01-shared-geo-counterexample.md`，
  live 台账同步。双跑套件的清扫语句同步到 `VALID_BOOLED` 共享判据形态并双引擎复验
  （T07）。

### 修复

- 收口依赖追齐后暴露的两条纯函数红：T054 的 SSCL 三角化删除本地剪切角折叠副本，
  直接消费 aios-core `f9f1bf0f` 的规范折叠值，并让「折叠后仍出界」诊断先于 bool
  `check_valid()`，不再把 90° / 271° / NaN 误报成尺寸退化；specs/023 的元件库
  顺序分批与并行优化 fan-out 同步改由全局几何并发闸推导块宽，移除遗留的固定 4 路
  和 1/2/4 分块，并扩展源码守护覆盖按输入规模选择固定 fan-out 块宽的写法。T054 三条定点门、
  并发闸 6 条纯函数门及同 feature 全量 **1104 passed / 0 failed / 86 ignored**。

- RVM mesh 对拍不再可能量到 OCC 的答案（specs/009 X1a 前半）。`mesh_compare` 的 gen 侧
  在 `tessellate_libgm_param` 返回 `Ok(None)`（非形状）或报错时会回退 `gen_occ_shape`
  ——正是 T037 从 `occ_generate.rs` 拆掉的那条形状回退，活在打分的那一侧。它也不是
  休眠的：`occ` 在 default 里，`rvm_verify` 是叠加 feature，跑法与 `Run-LiveBatch.ps1`
  都不带 `--no-default-features`，所以台账里 2026-08-14 那 8 条 mesh 对拍**全部**是带
  `occ` 量出来的，已在台账整体标注为口径失效、按未验资产对待。分支与只服务它的
  `gen_tol` 参数一并摘除，`gen_side_has_no_second_shape_engine` 钉住不许长回来。
  重新取证的前置是一个带 dbnum 1112 + 8000 生成几何的库——`.surreal/ams-8009`
  已被 3.x 写坏（fork 2.1.4 报 `format_version: 7`），2026-08-24 实测确认。

- `occ` 退出 gen-model 的 default / console（specs/009 X1a 收尾）。cfg 挂点归零之后
  这个 feature 已经和当初的 `truck = []` 一样是空壳，区别只在于它还拖着
  `dep:opencascade` 和 `aios_core/occ` 白编一遍——feature、依赖、以及随之变成
  「未被用到」的 occt-rs `[patch]` 段一并删除，`Run-L3Suite.ps1` /
  `Run-E3DFixtureSuite.ps1` / `Record-Db8000SessionChain.ps1` 三个脚本的 features 串
  同步去掉 occ。ADR-030 决策 7 的「回滚 = 把 occ 加回来」自 T037 起就已经不成立
  （加回来形状也只走 manifold），真回滚是 git revert。
  **Cargo.lock 不在本次提交内**：工作区正卡在 V4 中途，根 `Cargo.toml` 已把
  aios_core 升到 `f9f1bf0f`，而 `python/Cargo.toml`、parse_pdms_db、pdms_io 仍钉
  `29c91f48`，依赖图里因此有两份 aios_core（`RefnoEnum: From<RefU64>` 一类的红全出自
  这里，与本次改动无关）。锁文件要由收尾 V4 的那一步一起重生成。

- 摘掉 gen-model 里剩下的 occ 挂点（specs/009 X1a）。`occ_generate.rs` 的
  `apply_insts_boolean_occ` / `apply_cata_neg_boolean_occ` 两个死布尔函数、它们的
  三条 import 与全部注释调用点删除（ADR-029 布尔单轨 manifold 已一年量级）；
  两条只在 `occ` 下编译的回归用例改挂 `manifold`：房间面板自交修复走
  `tessellate_libgm_param`（轮廓修复在 wire 层，与后端无关）、GENSEC 直脊 SPRO 走
  `PdmsGeoParam::PrimLoft`。CI 不带 `occ`，所以这两条此前**从未在 CI 跑过**，
  现在两条都进 CI 且通过。`feature = "occ"` 在 gen-model 的 cfg 挂点自此归零。

- 收紧当期项目扫描范围：`included_projects` 统一解释为 `project_path` 下的文件夹名，
  名单外项目不再解析或扫描；`project_dirs` 不再重定向、扩大范围，也不再在空名单时
  充当回退名单（ADR-046 / specs/027）。

- 曲面法向的分组改由**轮廓**说了算，不再拿夹角猜（ADR-047 / specs/028 Phase 1，
  接替 specs/009 挂了一年的 T040b）。此前本仓两套做法都不是 E3D 的：`manifold_csg`
  按「同位置夹角 ≤ 10° 才平均」（`d0088e93` 引入，10° 是猜的），`sweep_mesh` 的侧壁
  干脆逐四边形写面法向、等价于**全是硬边**，弧墙 / 斜切墙 / 环形截面一律渲染成折面。
  E3D 的判据在 `GM_Profile::getPolygonForFacet` 的第二出参上：相邻两段不切线连续就把
  该顶点取负，判据 `D2_Span::leadsSmoothlyTo` 是 `|1 − 点积| ≤ 1e-6`（≈0.081°）。
  **这跟夹角阈值不是精度差异，是判据类型不同**：粗弧（面片夹角 45°）在 E3D 是光顺
  曲面、10° 会判成八棱柱；浅折角（5.7°）在 E3D 是硬边、10° 会抹平——一个阈值同时
  回答「面片多粗」和「形状多折」，两头都会错，而且错法随离散密度漂移。
  现在硬边在截面离散的同一趟里逐点算出，侧壁法向软点跨面片平均、硬点不平均、
  扫掠方向永远平均。**拓扑一位不动**：顶点数、索引、绕向、体积、包围盒全不变，
  变的只有 `normals`——所以看到弧墙从折面变光顺时，几何并没有动。
  弧形墙（`CurveType::Spline`）同批从 `Manifold::extrude` 改走 `sweep_mesh` 的挤出
  （specs/028 T06）：它的环是解析出来的四段、不可能自交，用不上 `CrossSection` 的
  NonZero 填充；一般挤出（PLOO 带倒角）**要**那道填充来化解自交轮廓，所以留在
  manifold 上等 Phase 2。半圆环的折痕自此正好是四个轮廓角 × 两个 z 层，八个，
  不多不少。
  布尔之后那一段仍走 10°（manifold 交回来的三角汤没有轮廓出处），这是 ADR-047
  写明接受的过渡态，退役条件在 specs/028 的 Phase 2 / 3。
  同批把**交线边**那一半也反完了（ADR-047 决策 6，证据
  `docs/evidence/2026-08-24-ida-edge-types-and-smoothing-groups.md`）：硬边在 libgm 里
  是边上的一个枚举值不是几何量，`normaliseStage2` 把布尔新建的边默认判硬、再用
  `isTangentDiscontinuity` 把**相切的缝主动合回软的**。判据是法向弦长 0.8182 ＝夹角
  **48.297°**——常量虽是 cos/sin 22.5°，但**有效阈值是 48.3° 不是 22.5°**，差一倍，
  按 22.5° 抄会把一大批该软的缝判硬。这条同时修正了 plan 里原先那句「交线是硬边会
  自然落出来、不需要额外规则」：自然落出来的只是硬的那一半。
- 扫掠体的截面离散补上另外两套口径（specs/009 T056）。libgm 里挤出 / 回转 / 放样
  是三个类三条路，而 `sweep_mesh` 三支共用挤出那一套折线化——T040 当时只修了
  `manifold_tessellate::tessellate_revolution`，没往扫掠这条路上看，于是弧墙与斜切墙
  一直在用 `GM_Extrusion` 的段数。现在建环与离散拆成两步（新增 `RawLoop` 与
  `ProfileCaliber`，`Polyline` 中间层去掉），三支只在「每段分几步」这一处分叉：
  挤出逐 span 自算、回转按配对 span 取大、放样再由 `GM_Collar::setSpanSteps` 在
  **外环与全部孔环之间**按 span 下标取大。
  **回转与放样必须是两个入口**：跨环取大是 `GM_Collar` 独有的，`GM_Revolution` 的
  `polygonForFacet` 按 `GM_Profile` 对象逐个调，没有那一层——合并入口等于给弧墙
  加一条 E3D 没有的规则。跨环步数表对不齐时硬失败，不猜。
  现场可见的变化是整环截面（SANN 360°）内孔的点数被抬到与外环一致；能量它的
  RVM 门（T047 / T048）仍未跑。
- 偏心 Snout 的偏移改成**上下各摊一半**（specs/009 T050）。libgm
  `GM_Snout::calcFacetsWithoutSurfaces` 逐顶点写的是 `r·cosθ ∓ xShift/2`，`calcRange`
  的支撑函数独立佐证同一约定，`GM_Pyramid` 也完全同构；本仓 `gen_snout` 与 aios-core 的
  `gen_occ_shape` 却都把偏移整个加在顶圈，相对 E3D 整体平移 `(XOFF/2, YOFF/2)`。
  **两条后端互相一致，所以此前任何双后端对比都发现不了**——只有对着 libgm 的绝对位置
  才照得出来。vendor 侧同批抽出 `LSnout::end_centers()`，OCC 与 truck 两条路共用它，
  约定不留第二个版本，并因此变得不依赖 `occ` / `truck` feature 就能测。
  活库实测有 1 件 2 实例（`poff = 12.06`，错位 6.03mm ＝ 容差的十二倍），不是纯防御。
- 「椭圆碟」（DISH 且 `RADI > 0`）建的曲面族换对了（specs/009 T038a）。libgm 的
  `GM_EDish` 是**托里球形封头**——一段球冠加一圈与它相切的环面拐角，本仓画的是半个
  旋转椭球。补经向段数补不上这个差，`gen_elliptical_dish` 整个重写，形状三量
  （拐角半径 / 球冠半径 / 交接角）与三个方向的段数改由
  `libgm_discretise::elliptical_dish_facets` 一次解出。
  两处照抄陷阱写进了函数文档：`RADI` **只是开关**，Core3D 读了它却把数值丢掉、
  实参是现算的 `r_k = h / (1 + (a − h)/√(a²+h²))`；交接角是 `acos(1 − q)` 而不是
  `acos(q)`（后者是 Hex-Rays 吞掉 acos 实参之后的伪码假象，抄错会让碟身留一道折痕）。
  绕轴喂的是底半径而不是球冠半径。活库实测 15 / 17 行是椭圆碟、102 个实例。
- 三条 `gm_Create*` 的参数顺序补上外部权威（specs/009 T011 / T013 / T016）。两头对照
  libgm 构造函数的字段序与 Core3D `CSG_Basic???::getPrimGeom` 的调用点，**顺序全部
  与实现一致**；`gm_CreateSlopeEndedCylinder` 的 wrapper 内部会把两对剪切角换位，
  对外签名是底面在前，本仓写法正确。顺带记下两条现成欠项：CTOR/RTOR 的内半径
  Core3D 会夹 `fmax(RINS, 0)`、SSCL 的四个剪切角 Core3D 会折进 (−90, 90]，本仓两条都没做。
- 上面那两条欠项当日销掉（specs/009 T054 / T055）。SSCL 剪切角折叠落在 vendor
  `SCylinder` 一处（`fold_shear_angle_deg`，Core3D `0x107272D0` 的单次折叠，非取模），
  身份哈希、落库规范值、occ/truck 后端与 gen-model 三角化臂消费同一份，135° 与 −45°
  自此同键同网格，折完仍出界响亮失败；180° 的剪切角折回直柱身份，不再白拆复用。
  CTOR/RTOR 的 `From` 构造点补上 `fmax(RINS, 0)`；顺着 validate 反编译
  （`GM_CircTorus` `0x10030bb0` / `GM_RectTorus` `0x10030780`，判据 `rIns ≥ −1e-6`）
  发现 `RTorus::check_valid` 的 `rins > 0.0` 比 libgm 还严、错拒合法的 RINS=0 构件，
  已放宽为 `>= 0.0`。`rins = 0` 的喇叭环面 / 实心扇柱由新增的贴轴收拢
  （`mesh_primitives::collapse_onto_axis`，仅 `rins < rout·1e-5` 档启用，吸附哲学同
  T035）收成可布尔的闭合实体，体积对帕普斯过门。vendor 改动仍未发布，
  与 snout 一批推上游（收官计划 Phase V）。

### 新增

- 斜切延伸段的长度规则落成纯函数（specs/009 T021）：`sweep_mesh::mitre_extension_reach`
  与 `mitre_extension_length`，出处是 Core3D 的扫掠段构建器 `sub_107318E0` 及它调的
  `sub_10733720`。规则是「轮廓每个顶点 + 每条弧上 9 个内点，逐点算在切面法向上的
  伸出量，取最大绝对值；超过 1 再加 1，最后与端点间距相加」。那 9 个点是本仓遇到的
  **第四套离散口径**，与容差无关、只服务这个包围盒，已收成具名常量并写明「别拿去铺
  三角」。还没接生产（挂到段 CSG 是 T023，依赖斜切墙 RVM 门）。
- 两份活库盘点证据。`docs/evidence/2026-08-24-eccentric-snout-census.md`：偏心 Snout
  两库一个 0 一个 1，**翻掉了上一轮「两库定性一致」的隐含预期**——再有「预期为 0」的
  专项，一个库查出 0 不能当证明。
  `docs/evidence/2026-08-24-unit-normalised-curved-primitives.md`：碟 / 两种环面 /
  不偏心 Snout 在 `inst_geo` 里**全是单位几何**（`pdia` / `rout` / `pbdm` 恒为 1.0），
  于是 WP-G 那套「段数由真实半径算出」的权威规则拿到的是单位半径，`tol/R = 1` 直接撞
  45° 下限——**不论多大都是 8 段**。碟的实例尺度跨 13mm 到 48.9m，最大那件应当 492 段，
  弦高差出容差的 3700 倍。  规则对了但现场还没生效，挡在前面的仍是单位网格身份键：
  **G3 / T041 的范围不是「柱与球」而是所有参与复用的曲面原语**，已开 T053。
- 第三份活库盘点证据 `docs/evidence/2026-08-24-negative-rins-census.md`（T055 收口）：
  负 RINS 两库均为 **0**，夹取在两库范围内是纯防御；但库 A 有 **1 行 `rins = 0` 的
  PrimCTorus（12 实例，`scale = 33` 的 90° 实心弯）**——旧生成器对它产出的是退化
  网格，T055 的贴轴收拢治的正是它（现役修复，不是防御），同时它是 T049 圆环面
  抽检的现成样本。库 B 用的是 init 快照（3,631 行，比 T052 在跑实例少 6 行增量），
  结论只覆盖到快照点。
- ADR-044 补 2026-08-24 修订：`GM_Sphere` 的权威面片规则进正文——绕轴
  `n = circle(r, tol)` 硬截 1000，经向带数**恒 = n/2**（`GM_Facets` 构造实参反推，
  与球碟同构），对决策 2 收紧为**球的身份键只需混一个 n**；顺带把「现状 16×36 的
  36 从来没对过」钉死在口径对照表里。出处 `0x100A20F0` 反编译，证据同上一条 IDA
  复核文档。
- 收官计划四个决策点 D1–D4 拍板并落进计划正文：整库重建不设独立窗口（V1–V4 同批、
  混合期目标为零）；球/SSCL/多面体缺样本走 **E3D 造样本**而非找第三个库；RVM 基准
  单跑库 A（7997 副本），偏心 Snout 与球两条例外走库 B 运行态或造样本；双会话分工
  以 specs/009 文件面为界，vendor `wire.rs` 归净窗口线，Phase V 推上游前对表。
- IDA 复核证据 `docs/evidence/2026-08-24-ida-occ-retire-audit.md`：T050 的 ±shift/2、
  T013 的 `fmax(RINS, 0)`、T016 的单次角度折叠三条依据在 idb 里逐位复核属实；
  T021 的 `t = k/10` 从推断升级为实证（乘数常量读出 = 0.1，调试日志就叫
  `"/10 point of span"`）；新钉 `GM_Sphere` 面片规则——绕轴 `n = circle(r, tol)`
  硬截 1000、经向恒 `n/2`，球的身份键只需混一个 n，且现状 16×36 里的 36 在
  「幸运尺寸」下也从来没对过。SSCL 折叠与 RINS 夹取两条欠项复核仍欠，已开
  T054 / T055 排进 WP-K。
- IDA 补齐最后一个没读过的形状臂：证据
  `docs/evidence/2026-08-24-ida-gm-collar-ruled-solid.md`——`GM_Collar`
  （= `gm_CreateRuledSolid`，斜切墙那一支）在 libgm 3.1 逐位读出，此前手上只有
  teach 记录里一行 2.10 转述。四条与本仓有落差：**两端点数一一对应是 libgm 的前置
  条件**（`validate` 比两端跨度数，不等报 −61 拒建），不是算法凑出来的；`setSpanSteps`
  的配对表与步数表是**两端外环加全部孔环共一份**（初值 8、逐 span 取
  `max(自身半径, 配对半径)`、容差逐轮廓、写回只增不减），而**「只增不减」正是 collar
  强制两端同段数的机制本身**——`polygonForFacet` 冷路径会主动清掉「已设定」标志重算，
  统一值全靠取大活下来；摆位是 **z = 0 → z = height** 而不是 box / snout 那套 ±h/2，
  两个盖各是一个 n 边形面；侧壁是双指针归并，步长与硬边共用 `polygonForFacet` 第二
  出参——**该出参一参两用，T040b 只做法向那一半补不齐**。
  同一份证据里把 **T040b 的规则也反完了**（实现仍不在本期）：`getPolygonForFacet`
  的第二出参逐顶点记「有几条 span 在这里收尾」，相邻段不平滑时取负，**闭合处一次负
  两个**；判据 `D2_Span::leadsSmoothlyTo` 是 `1 − 点积 ≤ 1e-6`（≈0.081°），
  **与布尔那边的 22.5° 和归一化那边的 48.3° 是三个不同判据，不得互顶**；
  切线里 `0 < |bulge| < 3.06e-5` 走一条非单位的退化分支，效果是「极小非零 bulge 必判
  硬边」，照抄时别顺手归一化。
  顺带撞出 T056（当日已修，见「修复」一节）。specs/009 的 T020 / T022 / T047 / T048
  已补上依赖与验收门。
- `CONTEXT.md` 补上 ADR-041~045 各自引入的那个概念，五条一次到位，每条按主题进它该在的
  分节而不是堆在文末：**libgm 面片口径**（紧跟「单位网格身份」，因为 ADR-044 改的就是那把
  身份键）、**切片流水线** 与 **insts_flat 失效协议**（模型生成执行）、**分块解析基线**
  （暂存与写回）、**refno 反查**（紧贴「空间树」）。
  `_Avoid_` 那行写的是各决策真正要挡的叫法，不是凑同义词：切片流水线禁「ZONE 并行 / 按片
  并行」——ADR-041 第一节整段就在拒绝这个说法，并行单位是生成根，切片只决定一次拉多少数据
  进来；分块解析基线禁「暂存窗口」——它只取「实例 + 读路由」那一半，基线不开窗口；libgm
  面片口径禁「默认段数 / 细分级别」——段数不是画质旋钮，共面抵消只消全等重叠，差一段布尔
  就留一层内壁；refno 反查禁「自建 refno 索引」，理由留在正文：第二份「树上现在有什么」的
  真值漂了不是变慢，是把已经不在那儿的构件算进某间房，而这类错误没有任何东西会报出来。

### 移除

- 删除 `truck` 死接线（收官计划 Phase X 的 X0a / X0b）。gen-model 侧 `truck = []`
  是零挂点的空壳 feature，`gen_brep_shell` 在本仓零调用；vendor（aios-core）侧
  `truck` 的依赖在 Cargo.toml 里整组被注释，**开了 feature 也编不过**，17 个文件
  90 处 `#[cfg(feature = "truck")]` 门是纯死文本，还一直在吃同批维护（T050 / T054
  都给它做过等价改写）。两侧一并删除：feature、注释依赖、全部门与所属条目；
  `LSnout::end_centers()` 无 feature 依赖，保留。vendor 半区与 snout / T054 / T055
  同批推上游。
- 删除浸水插件 `src/plug_in/water_calculation.rs`（业务上不再需要），连同
  `plug_in` 的模块声明与 `consts::AQL_WATER_CALCULATION_COLLECTION`。它是 ADR-030
  背景一节点名的「第二个 OCC 抑留点」，这条说法自此作废——抑留点只剩扫掠体
  （PrimLoft）一处。删除时它对 OCC 的依赖其实早已不存在：唯一那处 BRep STP 导出挂在
  从未定义过的 feature `opencascade_rs` 下，历来所有构建编的都是写死字符串的占位，
  死分支已于 2026-08-23 删掉。剩下的 `save_stp_data_to_arangodb` 与四个 ArangoDB
  查询函数在仓内**零调用点**。`aios_core::water_calculation` 自此在本仓无引用。
  见 ADR-030 决策 11。

## 2026-08-23

### 修复

- 模型生成收口三个「有意回退 OCC」的口子（specs/009-retire-occ WP-F，依据 ADR-030
  修订二的 IDA 实证）：出平面回转轴改硬失败并带出实际轴向（libgm 的轴参数是
  `D2_Point`，E3D 在 API 层就表达不出出平面轴）；`CurveType::Spline` 按其 OCC 权威
  实现的本义落地为**弧形墙截面**（三点圆 + thick 内外偏移的环形扇区，弧折线走
  libgm 角度格子），点数≠3 / 三点共线 / 出平面 / thick 吃穿半径一律硬失败；
  `Unknown`/`CompoundShape` 直接标 `bad`。`gen_inst_meshes` 自此只有 manifold
  一台形状引擎，OCC 形状回退整段拆除（`occ` feature 仍在，只服务对拍参照与
  历史断言）。活库盘点三类口子出现次数均为 0，收口不动任何现存实例。
- 回转轮廓补上 libgm `movePointsOntoYAxis` 的轴心吸附：半径坐标在
  `GM_User::normtol_ = 1e-6`（实测初值，运行期无人改写）内的顶点精确置 0，
  贴轴轮廓带浮点噪声时不再在轴心留一圈纳米级针状面。
- 弦高容差收成全库唯一一份 `libgm_discretise::FACET_TOL_MM = 0.5mm`（绝对量）。
  扫掠体（墙）此前用的是 `SweepSolid::tol()` = 0.01 × 轮廓外接球半径的**比例**容差，
  于是 `tol/R` 恒定、段数与构件尺寸无关——同一个半径的弧在墙上和在与它相交的原语上
  分成不同段数，而 `cancelFacets` 只消全等重叠，共面处就留一层壁。**这会改变墙的
  弧段段数**，而 RVM 门要等 `mesh_compare` 从 `occ` 解绑（T043）才跑得起来。
  `the_facet_tolerance_has_a_single_source` 按源码扫住第二个容差来源的回流。
- 曲面生成器不再自带默认段数：`mesh_primitives` 删掉 `DEFAULT_CIRCULAR_SEGMENTS`，
  `unit_sphere` 的经纬段数改由调用方给。柱与球仍锁死段数是**单位网格身份键**的欠账
  （ADR-044 / T041），现在收成 `manifold_tessellate::unit_mesh_identity` 一处具名常量，
  不再是散在三个 match 臂里、看着跟旁边算出来的段数一模一样的裸字面量。取值一位未动。
- 弦高容差删掉三处兜底默认值。折线化那三条路（挤出轮廓、回转轮廓、弧墙截面）各写着
  `if chord_tol > 0.0 { chord_tol } else { 1.0 }`，常量只定义一处并不等于「唯一一份」——
  第二个值藏在分支里，而且只在非正容差时现身。今天不可达（生产喂的都是
  `FACET_TOL_MM`），可一旦容差接成配置项或按构件算，它就会把 0.5mm 静默换成 1.0mm：
  段数减半、`cancelFacets` 的共面抵消随之失效，现场只看得到布尔结果多一层内壁，
  没有一行日志指向容差。改为 `libgm_discretise::chord_tol_is_usable` 判定后硬失败。
- 椭圆碟的经向段数从调用处的 `(around / 2).max(4)` 收进具名
  `libgm_discretise::elliptical_dish_meridional_segments`。它是 T038a 的欠账（libgm 把母线
  拆成球冠段与过渡角段两次分算，`knuckleRadiusToUse` / `radiusOfHub` 还没反完），
  混在 `d.pdia, d.pheig` 中间时跟旁边真按规则算的段数长得一模一样，改动它一位不会有
  测试变红。段数自此只有三个出处：权威规则、点名的身份键欠账、点名的口径欠账。
  （该函数 2026-08-24 随 T038a 落地删除——欠账还上了，具名占位一并撤走。）
- 结构楼板极端倒角那条护栏改量真实路径（specs/009 T044）：`loop_model` 的整体断言
  原挂在 `occ` 的 BRep 后端上，CI 口径不编 `occ`，它是一条从来不跑的断言；
  现改走生产同款 `tessellate_libgm_param` + `compute_aabb`，顶点与包围盒的有限性
  在 CI 里真被验到。
- 删除浸水 STP 导出的死分支（specs/009 T044，ADR-030 决策 6）：真 BRep 导出挂在
  从未定义过的 `opencascade_rs` feature 下，发布二进制历来只编「写一句固定字符串」
  的占位；其唯一调用点 `test_water_calculation_stp.rs`（1382 行）的 `mod` 声明本身
  是注释，连测试都到不了。死分支与死测试文件一并删除，占位实现的文档写明重启路径。

### 新增

- 新增源码断言 `the_curve_primitives_are_not_shape_arms`（specs/009 T017）：libgm 的
  曲线/标记图元（`gm_CreateNull` / Mark / Straight / Arc / Bezier 走
  `calcFacetsWithoutSurfaces` 出折线，不产实体）不得成为 `tessellate_libgm_param`
  的成功分支——五个名字不许出现在生产半区，分发臂集合钉死为 14 形状 + 2 非形状，
  新变体必须先过清单再进 match。
- `geom_error` 扩展 `primitive` 基本体错误：缺失/非法 BREP 与 NaN 变换按参考号持久落库，
  保存 noun、尺寸诊断、累计次数和首末时间；成功生成后精确销账。
- health 新增 `model_update_pending` 单查询快照与 `blocking_conditions`：模型/房间死信会把
  顶层状态降为 `degraded`，普通可重试积压不降级；查询失败仍沿用 2 秒预算并只进入
  `degraded_sections`。
- 新增零尺寸 NCYL 现场修复编排、严格守卫 E3D 宏和成对基线回滚脚本；默认 dry-run，
  只在参考号、owner、noun、空名称、尺寸与位姿全部匹配时删除 `24381/38635`。

### 修复

- `CYLI/SLCY/NCYL` 生成无效 BREP 时在诊断中带出参考号、noun、`DIAM` 与 `HEIG`，
  零尺寸根因不再只显示“invalid BREP shape”。
- 坏基本体不再按请求模式升级成生成失败：缺失 BREP、非法 BREP 与 NaN 变换一律记进
  `geom_error` 后跳过这一件，剩下的照常生成；账本写不进去也只发一句 warn，与布尔
  那条链的 `note_skip` 同一纪律。此前这三处按 `targeted`
  （`debug_root_refnos.is_some()`）分叉 bail 掉整个生成根——请求模式不是正确性边界，
  源库里那个没名字也没尺寸的空 NCYL `24381/38635` 因此让 FRMW `24381/38614` 常驻
  500、`regen_root` 连撞 5 次成死信，同一份数据走全量入口却只是少画一件。世界变换
  与属性**查询失败**仍然硬失败：读不到与读到坏数据不是一回事。
- 模型死信公告改为状态指纹驱动：首次/变化立即输出，相同内容最多 300 秒一次，清零仅
  输出一次恢复消息；30 秒 worker 退避和 Model→Room 阻断顺序保持不变。

## 2026-08-20

### 新增

- 新增独立的数据批次停滞看门狗：任务在 `queued` / `held` 超过 60 秒时，不再只靠
  在线 `/health` 与 `/queue` 排查，而是把暂停、上弦、阶段屏障、epoch、worker 存活、
  会话窗口和初始化 blockers 组成一条 `AIOS-QUEUE-STALL` JSON，同时写 stderr 与
  `logs/queue-stalls-YYYY-MM-DD.jsonl`；同一任务每 5 分钟续记一次。看门狗与唯一 worker
  分属两个 Tokio 任务，因此 worker 卡在数据库 await 或已经退出时仍能留下异机可带走的
  离线证据。

- 复验 issue-019 的 T11b 固定存量删除 live：在仅含 SYST+8000 的隔离项目副本中，
  `test_net_window_agrees_on_a_stock_deletion` 以 `1 passed in 20.65s` 通过；窗口
  25..=26 的净三态为 `0/1/2`，固定 EQUI/BOX 从活行准确变为墓碑，写回水位为 26，
  finally 恢复后的 db8000 SHA 与起点一致。证据与 live-test ledger 已同步。

- 复验 BRAN/TUBI 房间计算合成 live 链：`live_room_tubi_row_enters_tree_and_tracks_regen`
  在 8071 一次性空库通过，确认 BRAN 重生成后的隐含 TUBI 进入空间树并新增正确的
  `room_relate` 成员边；证据见 `docs/evidence/2026-08-20-bran-room-tubi-live.md`，并已
  回填 live-test ledger。

- `PrimRevolution` 接进 `tessellate_libgm_param`（specs/009 的回转支）：此前它掉在
  `_ => Ok(None)` 里，而 `Ok(None)` 就是「回退 OCC」的信号——PANE 的负实体大量是
  NREV，正体走 manifold、负体走 OCC，这一减整条设计布尔又被拖回 OCC。新增
  `tessellate_revolution` 语义逐条对齐 OCC 权威实现 `Revolution::gen_occ_shape`：
  倒角复用挤出那份 `gen_polyline_original` 离散（`flatten_extrusion_loop` 改名
  `flatten_profile_loop` 两边共用），角度按「≈360 / >360 / ==0 一律当整圈」归一，
  回转分段数按弦高容差 `R(1−cos(π/n)) ≤ tol` 算并夹在 [12, 512]。换算进
  「(半径, 轴向)」二维系那一步行列式是 −1 会翻绕向，而 `FillRule::Positive` 只填
  逆时针环，所以按外环有向面积统一翻一次（所有环一起翻，保住外环与孔的相对绕向）；
  摆回本地系的变换刻意保持 det = +1，轮廓落在轴负侧时半径方向与出平面基向量一起
  取反，否则网格被镜像、负实体法向朝里，减出来是反的。**出平面的轴仍回 `None`**：
  manifold 的 revolve 只认一种摆放，PDMS 的 REVO/NREV 都满足，不满足的宁可走 OCC
  也不硬凑形状。三条单测：=24381/36945（1RX-RM13 穹顶）那颗「圆柱 − 半球」负实体
  与 ⅓πR³ 对拍（轮廓含「倒角吃光两条腿、四顶点里两个坐标重合」的退化写法）、
  轴负侧顺时针轮廓不镜像、出平面轴回退行为钉死。

- 新增 `src/fast_model/libgm_discretise.rs`：圆怎么分段，全库唯一一份，逐条移植
  libgm 的权威规则（IDA 逆出 `d2_numberOfSegmentsForCircle` / `d2_numberOfSegmentsForPartRev`，
  两者都是 libgeom 导出、libgm 只是导入方，跟 `leftShadow` 同一类；全文与常量记在
  `plant-4/libgm-boolean-algorithm.md` §7.9）。原先两处各写各的：
  `manifold_tessellate::circular_segments_for` 的弦高公式 `π/acos(1 − tol/R)` 与
  libgm 一致但漏了**段数取到 4 的倍数**与**步长封顶 45°（整圆最少 8 段）**（下限写的
  是 12）；`sweep_mesh::arc_segments` 则是拿扫角直接除步长，而 libgm 是**先算整圈段数
  再按角度等比例缩、最少 2 段**——两种算法的结果会差一段。
  那个「4 的倍数」不是凑整：它保证 0/90/180/270 落在网格上。少了它段数会跟 E3D 差
  1~3 段，而 §6.11 的 `cancelFacets` **只消全等重叠**，共面两层侧壁段数一差抵消就整个
  放弃，结果里留一层内壁。这是布尔收不收敛的问题，不是画质问题。
  单测按 Core3D 主初始化实际用的 `gm_SetDefaultFacetTolerance(0.5)` 钉一张表
  （R=25→16、100→32、250→52、3000→176、23400→484），另按 `arctol_` 初值 0.1 钉一张，
  并断言段数恒为 4 的倍数、恒落在 [8, 512]。R=100 恰好是 32 —— 那正是本仓一直写死 32
  的来处，也说明它只在那一个尺寸上对。512 上限是我们自己的护栏（libgm 没有），
  差异写在常量注释里。
  **两件还没做的写在模块文档里**：(1) 容差口径未对齐——libgm 是一个全局 `arctol_`
  绝对量，我们是每个原语按自身尺度给 `tol()`，比例容差会让段数与尺寸无关；
  (2) 其余曲面原语（圆柱/球/碟/锥台/两种环面/切角柱）仍写死 32 段，尚未走这条规则，
  因为改段数会打断「所有普通圆柱共用一个 `CYLINDER_GEO_HASH` 单位网格」的复用，
  得先把段数并进 hash。

- 补齐 libgm 截面弧的**取点相位**：`GM_Extrusion::calcFacets` 实际逐 span 调
  `D2_Span::getApproxPolyLine(tol)`，并非把每段弧按扫角均分；它先按整圆容差求 `n`，
  再只插入落在弧角区间内的固定格点 `k·2π/n`，首尾保留真实端点。现将
  `manifold_tessellate::flatten_profile_loop` 与 `sweep_mesh::flatten_loop` 同时切到
  `libgm_discretise::span_polyline_by_tol`，避免挤出端面与扫掠截面对同一 PAVE 得到不同
  顶点。回归钉住 `R=100, tol=0.5, 5°→95°` 的 10 点格子、正反 bulge、跨 0°、
  近零 bulge 与 RM12 两道大圆弧；部署后 `24381/36931` 的单位网格为
  4392 顶点 / 1464 三角，Plant UI 中弧面恢复且 ERROR=0。

- `PrimLoft`（SweepSolid，结构件扫掠体）接进 `tessellate_libgm_param`：内核
  `sweep_mesh::sweep_solid_mesh` 早已三支齐全、纯函数单测全绿，卡着没接的是各自的
  RVM 门（T019/T020/T022）。这一轮先接线、RVM 门欠着——它是活库里数量最大的一类，
  不接等于 OCC 退不掉。分派仍走 `do_solid_segments()`（Core3D `DB_Gensec` 的权威
  三支），与 `SweepSolid::gen_occ_shape` 同一份输入、同一个局部坐标系：直脊无斜切
  → 挤出，真斜切 → 放样（端面变换用 OCC 那边 `Solid::loft` 用的同一个
  `get_face_mat4`），弧脊 → 回转。核对过 OCC 那条路上唯一看着像分歧的地方——SANN
  的 btm/top 两条 wire 其实是同一条（`gen_occ_sann_wire` 里区分二者的那段是注释掉
  的），所以直脊 SANN 走 loft 与走挤出同形。新增两条接线测试：三支都不得回 `None`
  且各自过实体体检、未知截面响亮失败（OCC 那边同样是 `Err`）。
  **至此 `PdmsGeoParam` 的 14 个 Prim 变体全部由 manifold 路径覆盖。**

- `tessellate_libgm_param` 的兜底从 `_ => Ok(None)` 改成穷举
  `Unknown | CompoundShape`：变体全覆盖之后，`_` 的意思就从「还没做的类型」变成了
  「以后新增的类型自动悄悄回退 OCC」。往 `PdmsGeoParam` 加变体现在是一条编译错误。

- `PrimPolyhedron` 接进 `tessellate_libgm_param`（specs/009 T010）：面片壳本来就是
  现成的封闭壳，**不需要任何 CSG**，没有理由把整条链路拖回 OCC。解析阶段已带
  `mesh` 的直接用，否则逐面剖分——每张面按 Newell 法向定面内二维基、`loops[0]`
  外环其余是孔走 earcutr（面片常有孔，扇形三角化会填实），顶点不跨面复用所以
  平面片按面着色，最后 `sweep_mesh::orient_outward` 按有向体积把整壳翻成外向。
  容错口径与 `Polyhedron::gen_occ_shape` 一致（单张面建不出就跳过），但一张都剖
  不出来是 hard fail，不回 `None` 悄悄溜去 OCC。五条单测：立方体六面对拍体积与
  包围盒并过闭合可定向体检、整壳反绕向被体积兜底翻回、带孔面积 = 外环 − 孔、
  自带网格优先、无面片与全共线两种响亮失败。

### 修复

- 跨项目 DICT/CATA 裸 dbnum 选主改为「显式名单 + `included_projects` 顺序」两段式，
  与全量同步侧对齐。`select_catalogue_candidates` 的 rank 此前只从
  `catalogue_project_priority` 建，没被点名的项目一律 `usize::MAX`，一组候选里全是
  `usize::MAX` 就抛「没有 catalogue_project_priority 选主」——于是**配置里漏写一个项目
  就阻断整个 Catalogue 相位**，而 ADR-025 的相位屏障会把它后面每一条 Design 批次一起
  钉在队列里（现场：`blockers:["catalogue: 跨项目 CATA/DICT dbnum=7000 冲突且没有
  catalogue_project_priority 选主"]`，同时 `blocked_by_phase:"catalogue"`、
  `design:{total:1,pending:1}`、`CATA 依赖清单已激活：0 个选中文件`）。这既不是文档口径
  ——2026-08-04 配置变更记录里这个键的默认值一直写着「与 `included_projects` 同顺序」
  ——也不是另一条路径的口径：`versioned_db::database` 的全量同步本来就是先推显式名单、
  再把剩下的 `included_projects` 按书写顺序接在后面。现在 rank 补上同样的尾巴
  （偏移量从 `priority.len()` 起跳，保证点名的恒压过没点名的；`or_insert` 让
  `included_projects` 里写重的名字只认第一次）。**打错字仍然阻断**：名单里出现
  `included_projects` 之外的项目或重复项目照旧记 blocker——漏写是「没意见」，写错是
  另一回事，两者处置相反。落选方仍照打 `[manifest] … 被项目优先级遮蔽`，只是不再
  停下来等人。三条单测：空名单按 included 顺序选主（候选刻意倒序传入，钉死「赢家来自
  配置顺序而不是遍历顺序」）、半份名单里点名的在前其余按 included 排、未知项目仍阻断。

- 房间增量的四条清边语句与纠正窗口的 `pe_owner` 清理改走图遍历边目标：
  `render_room_relate_statements`、`render_room_panel_relate_write`、
  `render_panel_room_topology_statements`、`render_element_relate_write` 以及
  `existing_members_of_panel` 此前都是 `WHERE in =` / `WHERE out =` 的谓词形式，
  `window_repair` 的硬删除则只把成对语句改了一半（`increment_pipeline` 那条早已是
  `DELETE {pe}->pe_owner`）。**DELETE 拿不到二级索引**：8009 现场三张边表其实都有
  `(in, out)` UNIQUE 索引（`unique_room_relate` / `unique_room_panel_relate` /
  `unique_pe_owner`，前两条在仓库源码里找不到创建处——索引口径不能只看源码），但
  10 万条边、索引在场的隔离实例实测 `DELETE … WHERE in = X` 仍是 3.132s，
  `DELETE X->room_relate` 244.973ms；`out` 侧 2.953s vs 24.455ms，删除行数逐对相同。
  面板重算按面板发、元素分支按元素发——全量重建就是面板数乘边表全扫。
  读侧另算：`out` 侧连 SELECT 都够不着索引前缀（8009 只读实测 1.12s vs 392µs），
  而 `existing_members_of_panel` 是按 `in` 的 SELECT，本来就走索引，改成边目标
  **没有收益**（791.9µs vs 1.1236ms），只为四条房间语句形状一致。取证见
  `docs/evidence/2026-08-20-edge-scan-sweep/`。排除子句从
  `AND in NOT IN [..]` 变成挂在边目标后的 `WHERE in NOT IN [..]`，形状仍过
  ReplaySafe（`is_bounded_target` 认边目标）。新增两条回归：一条钉住五条语句的边目标
  形状并禁止谓词写法回流，另一条在 mem 库上实测「边目标 DELETE 真的删掉
  `INSERT RELATION` 写入的边、且不误删同面板的其它成员边」——这是本次唯一会静默出错
  的地方，若引擎只在 `RELATE` 时维护邻接索引，先清后写会退化成只写不清且不报错。

- 同一条纪律补到另外两处：`query_service::spatial_bounds` 的
  `FROM inst_relate WHERE in = pe:{refno}` 改成按记录 id 直接寻址
  `FROM inst_relate:{refno}`（写口 `pdms_inst` 用同一对 `to_inst_relate_key()` /
  `to_pe_key()` 渲染 id 与 in，两者选中同一行；数组 id 的版本化历史行的 `in` 是
  版本化 pe，本来就不在命中集里）——`inst_relate` 是唯一连 `(in, out)` 索引都没有的
  边表（只有 `anc` / `dbnum`），`LIMIT 1` 也救不了没有 aabb 的 refno，那要一路扫到
  表尾；8009 现场只读实测 968.4ms vs 直址 121µs。`staging::preload` 删除子树的 `pe_owner`
  拓扑拷贝从 `WHERE in IN [..] AND out IN [..]` 改为按成员出边走图再过滤 `out`
  （`IN` 不是 `=`，`unique_pe_owner` 用不上），与它上面那条「`pe` 按记录 id 直接
  寻址、895 万行 64.4s → 0.5ms」的注释同源。`preload` 的 `pe_owner` 夹具顺带从
  `RELATE` 换成生产写口的复合 id `INSERT RELATION`：子树闭包与删除桶拷贝都走图遍历，
  夹具用 `RELATE` 的话「邻接索引是否也对 `INSERT RELATION` 维护」这个区别测不出来，
  而它一旦不成立就是删除级联在暂存里走不到后代、`status = OK` 地少删一片。

- RVM 基准对拍工具的四处边表全扫一并收口：`compare::render_children_select` 的
  `FROM pe_owner WHERE out IN [..]` 改成 `{owner}<-pe_owner`——它在
  `load_subtree_refnos` 的 BFS 里每层每块各发一次，是全仓边表全扫**次数**最多的地方，
  一次子树对拍要扫几十遍整张 `pe_owner`（8009 现场 912 万条边）；`compare::load_gen_side` 的
  `FROM inst_relate WHERE in IN [..]` 改成按记录 id 直接寻址；`mesh_compare` 的
  `gen_world_mesh` 与 `ensure_booled_mesh_files` 的 `WHERE in = {pe_key}` 改成
  `{pe_key}->inst_relate`（这里拿到的是字符串键，走图不必再做 id 字符串手术）。
  mem 库实测确认两件事：`INSERT RELATION` 写入的 `inst_relate` 行经
  `{pe}->inst_relate` 取得到、计划为 `Iterate Edges`；直接寻址一组 id 时不存在的那些
  **不出行**，与 `WHERE in IN [..]` 逐字同一份结果——`load_gen_side` 拿整棵子树当输入，
  绝大多数节点没有 `inst_relate` 行，多出 NONE 行就会让 `GenRow` 反序列化整批失败。

- 边表全扫收口完毕：诊断 bin、夹具与 live 集成测试里剩下的谓词写法一并改为边目标
  或记录 id 直址——`test_tubi_inst_relate`（嵌套两层全扫）、`l3_suite` 的场景快照
  （`inst` / `owner` / `room` 三项，每场景拍三次）、`room_fixture` 的清理与两处
  `WHERE out =`、`room_live_issue7` 七处、`issue7_e2e_increment` 四处、
  `staged_transform_e2e`、`increment_pipeline` 与 `cata_closure` / `cata_model` /
  `model_update_plan` / `fork_surreal_compat` 的 live 断言、`spatial_tree_8000.py`。
  `room_fixture` 那四条 `WHERE in IN [..] OR out IN [..]` 换成 `{pe}<->{table}`
  （mem 实测确认 `<->` 一次删掉两个方向）。

  两处**刻意没动**：`l3_suite` 快照里的 `geo_relate WHERE in IN [pe:..]` 传的是
  `pe:` 键，而全仓每个 `geo_relate` 写口（`pdms_inst`、`occ_generate`、
  `manifold_bool`、`increment_manager`）的 `in` 都是 `inst_info:`，照此它应当恒空，
  可 m1 的 I-3 断言又要求它恰好是 5——两者不可能同时成立，得连断言一起判，不是性能
  收口该顺手改的；`staged_regen_e2e` 与 `Run-RoomE3DE2E.ps1` 的 `WHERE in = X OR
  anc CONTAINS Y` / `OR in.owner = X` 带解引用与 `OR`，图遍历不等价。

  `test_tubi_inst_relate` 的重写踩到一个坑，记在这里：从**一组**记录出发的
  `$var->inst_relate` 会把每个字段裹一层数组（`refno: [pe:xxx]`），必须
  `array::flatten(...)`，否则下游 `RefnoEnum` / `String` 反序列化整批失败。

- 修复 `node_gen_room_probe` 在大表上用 `WHERE out IN (...)` / `WHERE in IN (...)`
  扫描 `room_relate`、`room_panel_relate` 的性能问题：改为从 `pe` 记录执行
  `<-room_relate`、`->room_relate`、`->room_panel_relate` 边遍历。8019 实测执行计划
  从 `Iterate Table` 变为 `Iterate Edges`，BRAN 根 `24384/23257` 的 10 元素子树汇报
  即时完成；新增源码回归测试，禁止旧扫描 SQL 回流。

- 整库快删与 ADR-021 回退重建的 `pe_owner` 清理改走 OWNER 复合 id 区间：每个权威
  Ref0 一条 `DELETE pe_owner:[pe:{ref0}_0, NONE]..=[pe:{ref0}_9999999999, ..]`，取代
  原来 `array::flatten(SELECT VALUE ->/<-pe_owner FROM pe:{ref0}_0..)` 那两句图遍历。
  边 id 固定是 `[OWNER_PE, 槽位]`，owner 在区间内的边本就是 id 连续的一段，不必先把
  边 id 全捞进内存——百万级 PE 的库上那是清库耗时的大头。少掉的 `->pe_owner` 方向
  在整库清理里是空扫（所有权链不跨库，且本 dbnum 的每个 Ref0 各出一条），**部分
  裁剪不满足这个前提**，`prune_above_watermark` 一行未动。这个跨 owner 的区间形状是
  `staging::replay_safe` 明令拒绝的写法，该限制不放宽，边界就地写在 `fast_delete.rs`。
  顺带换来幂等：新语句不读 `pe`，上一轮清库半途失败留下的边下一轮能清掉（遍历从空
  区间出发永远够不着）。后置条件同步加严，逐 Ref0 数 `pe_owner` 残留必须为 0，删边
  语句写歪时当场报错而不是报成功。三条回归测试 + 隔离库实测（目标区间 3 → 0，上下
  两侧相邻 Ref0 与前缀延长型 Ref0 全部保留）留证于
  `docs/evidence/pe-owner-range-fast-delete-20260820/`。

- 修正 `db8000_two_delete_fixture`、`db8000_session_pairs` 的 CI 依赖图：移除
  `legacy_session_replay` required-feature，所有窗口采集改走生产权威入口
  `IncrementPipeline::collect_window`。跨会话断言改为“窗口右端每个 refno 恰好一个
  终态操作”，索引差分与 vendor 对拍直接比较净窗口，不再把逐会话操作并集当生产语义。
  固定工具链下默认 feature 命令通过 6/6 与 21/21，Cargo metadata 确认两个 target
  `required_features` 均为空。

- 为 `/health.room_build` 增加 2 秒硬超时。现场新版启动期 SurrealDB 繁忙时该新增
  查询曾让整个健康接口超过 10 秒无响应；现在返回 `rebuild_required=true` 与超时原因，
  不再让可观察字段拖死探活，并有 pending future 回归测试。

- 修复布尔成品已经写入 `booled_id`、但 `insts_flat` 尚未回填时 Plant UI 仍加载正体
  原语的问题：Manifold/OCC 两条布尔成功路径现在与 `booled_id` 同步写入单位变换的
  成品实例，平表补扫也优先使用该成品；Plant UI 的旧库回退查询同样优先读取
  `booled_id`。`=24381/36945` 不再加载带 `Z×234` 变换的正体圆柱，右视图恢复为
  宽高比 2:1 的半球。新增写入不变量、平表优先级和读侧 identity transform 回归。

- 修复 Plant UI 中 `=24381/36945`（1RX-RM13 穹顶）与 E3D 外观不一致：首先发现
  Plant UI 实际读取 `test-worklspace/bin/assets/meshes`，其中仍部署着 2026-07-20 的
  旧网格，而本轮生成结果在仓库 `assets/meshes`，两者 SHA256 不同；已按哈希暂存并
  原子替换部署网格。其次把 `manifold_to_plant_mesh` 从无条件逐三角面法线改成按 f64
  精确位置归组、面积加权的折痕感知法线：夹角小于 10° 的曲面片光顺，端盖/侧壁和
  箱体棱边继续拆组。新增球面共享法线与箱体三组硬边回归。定向重生成后 AABB 保持
  `46919.106 × 46919.106 × 23400 mm`；网格逐点球面方程回归也已补齐。最终界面实例
  选路问题由上一条修复；部署前网格、源码、补丁和可执行回滚均已留证。

- **`=24381/36945`（1RX-RM13 穹顶）现在能端到端出一个干净的半球。** 新增
  `rm13_dome_pane_minus_nrev_is_a_hemisphere`：按活库里存的两个参数三角化，再走生产
  那条 manifold 差集，判据是 `Manifold::genus() == 0` 加 f64 体积对 ⅔πR³（0.1%）加
  包围盒。这颗构件把两层「倒角把直边吃光」的把戏叠在一起（PLOO 四角 FRAD 等于半边长
  → 方变圆；NREV 四个顶点里两个坐标重合 → 倒角吃掉两条腿只剩一段圆弧），是整条链路
  最硬的压力测试，一口气挖出四个独立缺陷，逐条记在下面。

- 修复 `NREV` / `REVO` 同时命中 LOOP owner 与 primitive noun 表时被重复派发的问题：
  这两类参数必须先由子 LOOP/PLOO 拼装，现统一从 primitive worker 路由中排除。
  定向生成不再因 `NREV` 的预期 `None` 误报 hard fail；`24381/36945` live 强制替换
  返回 `Generated`、可渲染 1、写入 2，几何/AABB/空间树计数保持稳定。

- 挤出的顺时针轮廓建不出任何东西。`tessellate_extrusion` 靠 `FillRule::Positive` 挖孔，
  而它只填逆时针环——PDMS 的轮廓**不保证绕向**（那颗 PANE 的 PLOO 就是顺时针），
  于是截面直接是空的、`bail!`、回退 OCC。按外环有向面积统一翻一次，所有环一起翻，
  外环与孔的相对绕向不变。回转支 2026-08-19 就修过同一个坑，挤出支漏了。

- 布尔结果的法向在 f32 上算，23400mm 处只剩三四位有效数字：轻则法向歪，重则两个顶点
  舍入到同一个 f32 → 叉积为零 → `normalize()` 给出 **NaN 法向**，一路写进 `.mesh`。
  `manifold_to_plant_mesh` 改为在 f64 顶点上算法向，并丢掉 0.1µm 内塌陷的三角
  （丢它不开洞：设 A、B 重合，这个三角贡献的是一条自环加上互为反向的两条边，
  自己跟自己抵消，其余边的配对一条没动）。

- **共壁负体的差集会碎成一地薄片。** PDMS 里负体常与母体共壁（那颗穹顶的 NREV 就是
  跟 PANE 同一个圆柱，只在内部多挖半球）。两个圆柱各自离散出的 484 边形只要不是逐位
  相同，差集就沿着共壁碎开——实测 **亏格 −131，即 132 个互不相连的壳**：一个半球加
  131 片碎屑。碎屑总体积只有 1e-7，体积对拍根本发现不了，但它们会进 `.mesh`、会
  z-fighting、会让后续布尔更难收敛。`subtract_negatives` 现在把每个负体按**自身包围盒
  中心**放大 1e-6（那颗穹顶上是 0.023mm）再做差，亏格回到 0，体积偏差 0.018%。
  E3D 不需要这一步，它靠共面反向面逐面全等抵消（§6.11），而那条路要求两侧段数与相位
  完全一致。

- 挤出与回转的弦高容差改用**绝对量** `FACET_TOL_MM = 0.5`（Core3D 主初始化传给
  `gm_SetDefaultFacetTolerance` 的就是这个值）。原来用的是 `BrepShapeTrait::tol()`
  ——按自身尺度给的**比例**容差，`tol/R` 恒定，段数于是与尺寸无关：同一个圆在挤出侧
  和回转侧只要包围盒不同就分成不同段数（那颗穹顶正是正体 60 段、负体 84 段）。
  改成绝对量之后两侧同为 484 段，配合 §7.9「段数取到 4 的倍数」保证相位也一致
  （顶点都落在 0/90/180/270 上）。**其余曲面原语仍是写死 32 段**，见上面 `libgm_discretise` 那条。

- 修复提交查询超时被当成「确定性拒绝」、一次尝试就把暂存窗口永久判死的问题。
  等满 `COMMIT_QUERY_TIMEOUT` 没等到服务端回话，语句既没被接受也没被拒绝，这是
  活性问题；但非 outcome-sensitive 的那条分支只报「终止本查询」，
  `staged_writeback_failure_is_transient` 认得的三个标记一个都不沾，于是走确定性
  出口直接落终态阻断——现场 dbnum=8000 的 `242..=243` 就是这样卡在一个 32 行 /
  1.6 KB 的块上、水位停在 241。两个超时分支现在共用 `COMMIT_QUERY_TIMEOUT_MARKER`，
  分类器据此把它归进瞬时桶；journal 块本来就按幂等重放设计，重放安全。预算是有限的
  （`STAGED_COMMIT_TIMEOUT_ATTEMPTS = 3`）：无限重放会走到另一个极端，一条真的跑不完
  的语句抱着 `STAGED_COMMIT_SERIAL` 每 30 秒烧一个 120s 死线。`is_transient` 判据
  因此多收一个「这是第几次」。

- 修复同 dbnum 多文件的阻断一律记在 Design 阶段的问题：CATA/DICT 的同号冲突也会
  因此把 design 拽成 blocked，而 design 侧可能一个问题都没有，阶段就绪判定从此
  说谎。`blocked_dupes` 现在带着 `DataPhase::of_db_type` 判出的阶段入账，与同一段里
  `manifest_totals` 的口径一致。顺带修掉 `dependency_manifest_version` 连着被赋两次、
  第一个直接被遮蔽的问题——那行日志此前数的是依赖清单的文件数、打的却是身份清单的
  版本号；两份清单各有独立计数器，现在各报各的。

- `boolean_generation_refreshes_aabb_after_final_relations_exist` 这道顺序门自己红了：
  它按源码文本找 `apply_insts_boolean_manifold(&target_visible_refnos`，而那处调用早已
  被 rustfmt 拆成多行，带实参的针扎不中，`expect` 直接 panic。针改成只认函数名。
  值得记一笔的是失败方向：源码断言找不到锚点时是「测试红」而不是「守卫失效」，
  这次运气好；反过来写（找不到就当通过）会安静地把不变量丢掉。

- 修复增量看门狗在共享盘抖动时可能整体死锁、且此后不再发现任何增量的问题。
  三件事凑成一个锁环：notify 8.0.0 的 poll 线程**同时持有 `watches` 与
  `data_builder` 两把互斥锁**同步调用事件回调（整轮 rescan 都在锁内）；回调过去是
  往容量 1 的 futures channel 上阻塞发送，而该 channel 的真实容量是
  `buffer + sender 数` = 2，一轮 rescan 里变化文件超过两个就把 poll 线程堵在发送上
  （与「单轮重扫比轮询间隔慢」无关，光靠数量就能触发）；唯一能腾出容量的是
  `async_watch` 的事件循环，而它同一时刻可能正走重挂分支，`MountState::mount` →
  `PollWatcher::watch()` 要的正是 poll 线程手里那两把锁。回调现在只做一次非阻塞
  `try_send` 置位，永不等待；`the_poll_watcher_callback_never_waits_on_the_event_loop`
  钉住这条（复现它要真的把共享盘挂掉，只能钉源码）。

- 修复一次 SAVEWORK 会换来 K 轮背靠背完整重扫的问题：notify 的 `rescan` 对**每一个**
  变化 path 单独发一条事件、事件之间零合并，而每条候选事件此前都独立触发一轮完整
  清单重扫，K 轮扫出来的还是同一份结论。现在事件只把 `WatchSweepGate` 置脏，首事件
  立刻开一轮，最小间隔（`AIOS_WATCH_MIN_SWEEP_GAP_SECS`，默认 5s）内的后续变化攒成
  一位脏，闸一开补**恰好一轮**。这是限频不是丢弃——闸内到达的变化完全可能是上一轮
  重扫**之后**才发生的新保存，脏位只准被真正跑出去的那一轮清掉，否则它就只能等下一
  拍周期对账（300s）。两条纯状态机回归钉住这两半语义。与「事件只标脏、判定必须走
  完整清单」不冲突：闸只改什么时候调那条权威路径，不新增任何按事件的局部判定。

- 修复 PollWatcher 轮询线程死亡时无人知晓的问题。此前 `async_watch` 把「信号流关闭」
  当作看门狗死亡的判据，而那个条件永远不会成立：发送端活在事件回调里、回调活在
  `watcher` 的 `data_builder` 里，只要 `watcher` 对象还在栈上，poll 线程死透了发送端
  也不会被丢弃。notify 8.0.0 又把 `thread::spawn` 的返回值直接 `let _ =` 丢掉，线程
  起不来与 panic 掉一样没有声音——外在表现就是「服务在跑、文件改了没反应」，只有
  周期对账还在兜。现在每一拍对账都主动 `PollWatcher::poll()` 探一次（控制 channel
  的 Receiver 正是被那条线程拿着的，线程没了 send 就失败），失败即按永久性故障逐拍
  告警并说明当前退化成多少秒一次的发现。探不了「线程活着但卡死」：从外面看它和
  「厂里没人动」一模一样，需要 notify 自己吐进度。

- 共享盘重挂轮的 `MissedTickBehavior` 补成 `Delay`（此前是 tokio 默认的 `Burst`）：
  本循环里每一拍都可能 await 一轮完整重扫，被挡久了以后 `Burst` 会把欠下的拍一次性
  连补出来，等于在最忙的时刻再排队几轮重挂。周期对账那一拍早已是 `Delay`。

- `async_watch` 的两个间隔默认值抽成 `DEFAULT_WATCH_POLL_SECS` /
  `DEFAULT_WATCH_RECONCILE_SECS`，并加一条守卫断言子集轮的默认值必须比全集对账轮密
  （调反不会有任何东西报错，但最贵的那一轮会变成主路径）。**两个值本身都没动**：
  子集轮询仍是 30s，周期对账仍是 300s。曾计划把子集轮询降到 5s 以缩短发现延迟，
  评审后撤回——决定负载的不是「单轮重扫多快」（本地实测 0.35~0.83s），而是「一次
  保存产生多少轮重扫」，而后者在共享盘上一个数都还没量到；上面两条修复正是把这个
  乘数压掉的前置条件。顺带更正一处此前写错的算式：`MAX_ATTEMPTS = 5` 撑出的
  `5 × 300s = 25 分钟`故障容忍窗口只在**文件安静**时成立，park 计数的条件是
  「同 dbnum 窗口右端没前进」、不区分是哪种来源把它重新入的队，watch 重扫一样在烧
  这份预算。

- 修复相邻构件方向不共线时直段被静默丢弃、三维里只剩一段空白的问题：轴线判定不再决定
  「写不写这一行」，只决定这一行是不是实体管。不可成管的连接照样落库，带
  `invalid` 与 `invalid_reason`（`direction` / `no_bore`），位姿改用局部 `+Z` 贯穿两个
  连接点，让查看端既有的虚线中心线正好画在 E3D 画点线的位置。口径解析提前到判定之前，
  拐死的连接也报得出它本该有的口径。查询层的诊断标记从「端点已删除」扩成
  「记录自带标记 or 端点已删除」，缺字段的历史行仍按可成管处理。

- 修复 BRAN 直管关系在增量重生成后保留旧高位索引、且新关系引用未落库
  `trans` / `aabb` 内容的问题：每个 BRAN 现以单个事务删除旧出边、持久化内容寻址记录并
  写入完整 `tubi_relate` 集合，空集合同样生效；直接写与 ADR-017 暂存写共用
  `execute_model_write`。元素级联清理同时删除其直管出边，并补充变短、空集合、幂等重放、
  内容可解引用及事务失败保旧回归。

## 2026-08-19

### 新增

- 引入 ADR-037 dabacon 快照完整性契约：`pdms-io` 以单一打开句柄生成稳定文件身份
  `SnapshotToken`，净窗口、冻结会话和模型计划终态存在性复核共享同一文件世代与
  target root；`parse_pdms_db` 增加不依赖属性字典的最小元素身份解码。

- `watch_dbnums` 限定模式增加权威 CATA 引用清单与元素级引用闭包：完整扫描继续裁决
  CATA 身份、重复文件和项目优先级，但不排全量 Catalogue 批次；模型窗口只索引被监听
  DESI 与裁决后的 CATA 文件。任务与健康接口新增依赖阶段、文件、解析/缺失计数和
  300 秒停滞截止时间。

- 暂存窗口按 sesno 拆窗（ADR-017 修订二，形态 C：预算式定窗 + 触顶收窄兜底）。
  第一层：`AIOS_STAGING_WINDOW_MAX_SESSIONS` 会话预算在**执行侧**收窄应用窗口右端，
  切点只落在真实 SAVEWORK 边界（会话号稀疏，取第 N 个真实会话而非按号算术）；预览与
  看门狗仍看整段待应用窗口。第二层：资源废弃档位触顶不再只记阻断，把该 dbnum 的会话
  预算收窄一档（减半、地板 1 个会话）再交还，预算 1 仍触顶才是真阻断；收窄记录追平
  `file_latest` 时清除。截断窗口提交成功后余量**立即重排**（不等下一轮重扫），
  `file_latest_sesno` 与并入基线沿用冻结批次那份——余量不是新观察，上界也绝不改写
  `FileObservation.file_latest_sesno`（源码钉）。相位纪元批次（`epoch_id > 0`）一律
  不参与收窄（ADR-025 phase totals 按批次记账，截断算不算相位完成未厘清）。提交侧
  一行未改：水位部分推进与 `align_end_sesno` 本来就容纳更窄的右端。原子单元变小
  （子窗口间可见真实保存点的中间态、跨子窗口的根会重复生成）待业主签字。

- 增量链增加独立阶段控制：`data_incremental`、`model_incremental`、
  `room_incremental` 分别控制数据、模型、房间消费，缺省均开启，并支持对应
  `AIOS_*_INCREMENTAL` 环境变量覆盖。关闭数据阶段仍扫描入队；关闭模型阶段时数据、
  水位与 durable 模型计划照常提交，模型积压留待恢复；下游不得越过未完成上游。
  `/api/v1/health` 同时暴露三个最终生效值，供单阶段调试直接确认运行姿态。

- 净窗口收集下沉到 pdms-io：`session_index_diff`（会话索引双根差分）与 `net_window`
  （净三态 → 操作流合成）整体从 `src/data_interface/` 迁到 `pdms_io::session_index_diff`
  / `pdms_io::net_window`，上层按新路径直接引用，`data_interface` 不做转发。理由是
  **被它替代的逐会话回放本来就在那一层**（`PdmsIO::collect_increment_eles` 一家，
  `legacy_session_replay` feature 门后），把替代品建在上一层等于同一个问题两份实现
  分居两个 crate；更要紧的是 `walk_tree` 复刻的是 pdms-io 的 `btree_search_optimized_recursive`
  路由规则（同键首见者胜、升序前缀、哨兵最左分支、`[本键,下一键)` 区间），复刻件与
  正本隔着 crate 边界时编译器帮不上忙，正本一改这边不会红、只会悄悄口径漂移（后果是
  漏报删除）。两个模块本就零 `crate::` 依赖（只用 `aios_core` + `pdms_io`），迁移是
  平移不是解耦。纯单测 24 条跟着下沉，在 pdms-io 里原样通过。
  **留在本仓**的是批次层的关切：`IncrementPipeline::collect_window`（会话页清单截取、
  `net_caliber_warning` 口径标注、`CollectedWindow` 形状、唯一入口源码断言），以及三条
  跨结构 live 对拍——它们的参照臂 `collect_changes` 是 legacy 回放外加两个 Save Work
  终稿补丁的包装、计时对象是生产入口 `collect_window`，在 pdms-io 里够不着，现迁入
  `increment_pipeline.rs` 的 `cache_tests`。性质 h/i（`db8000_session_pairs`）不动，
  改调 pdms-io 的收集器——它是这次平移的验收面：20 条全绿，其中
  `index_diff_matches_replay_folding_on_every_case_window` 与
  `net_window_collector_matches_replay_ops_on_every_case_window` 证明净口径逐案例
  仍与回放折叠一致。行为零改动，不涉及口径变更。

### 修复

- 修复 dbnum=8000 复制 BRAN 的增量数据已进入但 plant-ui 无节点/模型的链式故障：
  Watch Scope 现保留 PROP 与多项目 CATA 的 Ref0 身份，无关项目 DESI 副本不再阻断
  8000；CATA OWNER 替换按硬行数上限语义分块；Add 的旧入边清理改用图遍历目标，
  避免 `DELETE ... WHERE in` 全边表扫描卡满 120 秒。LSnout 的单元参数现与三位小数
  mesh hash 使用同一归一化值，消除 `0.5555555/0.5555556` 共用同一
  `inst_geo` id 却内容冲突的模型死信。直管段现在只写 `tubi_relate`，不再通过
  `ShapeInstancesData::insert_tubi` 用 `leave_refno` 覆盖 ELBO/TEE/VALV/BEND 的
  `inst_relate` 元件库关系。现场窗口 `242..=243` 已提交到水位 243；新 BRAN
  `24384/26229` 强制重算后保留 17 条元件实例关系和 7 条直管关系，可绘制网格实例由
  25 恢复为 57。

- 修复 manifold 路径生成的 `.mesh` 法线数组为空，导致 plant-ui 将同一 EXTR 端盖
  按三角形渲染出随机明暗的问题：Manifold 输出现在展开为带面法线的硬边网格；回送
  CSG 前按变换后坐标焊接顶点，兼顾 E3D 外观与闭合拓扑。增加 dbnum=8000 会话 239
  V 形 EXTR 回归，验证法线、绕向、闭合性和 CSG 往返。

- 修复 D: AMS 直接图形文档中 `Limits CE` 无响应：该文档暴露的视图类型为 `G3D`，
  原命令只接受 `GM3D` 并静默返回；启动修复现在同时接受两种 3D 视图，并在本地
  Drawlist 为空时先加入、更新当前元素，再执行 limits 与刷新。进一步统一 Model
  Explorer Add 与 Limits 的直接 AMS 路由：`ViewId=0` 时固定选择最新注册的 Drawlist，
  不再让已写入内容的旧 `DL[0]` 通过“成员最多”启发式夺走命令。相机 limits 现在也
  使用该 Drawlist 实际 attachment 的可见 G3D gadget，避免元素已加入但仍只在视口边缘
  露出一段。启动后不再 kill/re-register 已注册命令，因为 Ribbon 的托管委托会继续
  指向被销毁实例；改为保留并验证启动时实例。活动与 shadow PMLLIB 同时幂等修复，
  Frida 临时目录移至 E:，避免系统盘耗尽导致图形 finisher 半途退出。新增 CE 图形物化
  刷新：`drawlist.update()` 后先刷新 attachment view，再读取 limits，修复冷启动第一次
  点击灰屏、第二次才显示的问题；现场清理 5 个并发 AMS 实例后，以唯一 PID 61168 验证
  复制 BRAN 首次点击即可完整显示并居中。

- 修复 D: AMS 启动后 Steelwork 命令刷新反复报告 PML `(2,751)`：shadow
  `PMLCOMMANDMANAGER` 先创建 `STEELWORKGSETTINGS`，并将自动命令事件注册移到支持命令
  装载之后；Design 完全就绪后再运行带 trace 的全局初始化宏，启动失败时直接阻断而非
  把缺失变量留给每次 CE 刷新重复报告。

- 非白名单终稿解析失败、已选中索引 child 读取失败和层级未下降现在硬失败，不再构造
  可推进水位的不完整窗口；MNUM 仅按代码白名单记录结构化诊断。基线 chunk 失败会
  停止后续调度、等待 writer 收口并清空本轮全部已调度 dbnum；冻结 token 跨队列传递，
  同文件 append 仍只读冻结长度与 target，同 sesno 路径换代和空模型计划旁路均在提交前
  拒绝；初次捕获用前后 length/header 复核阻止 append 期间混合世代。CATA 扫描失败、
  空扫描、旧格式空缓存、Required 未解析根或 closure `missing` 均不再形成成功缓存，
  下一次触发会重新扫描。

- 修复复制大量节点后暂存写回把 167 条 journal/869 行合成单事务而永久停在
  `commit`：改为按条数、字节和预计行多维分块，增加块级进展、SQL 指纹和 120 秒
  单查询停滞边界，水位仍只由尾事务推进。

- 修复 dbnum=8000 净窗口删除漏判：父元素 `children_changed` 净减少现在使用
  目标会话 OWNER 成员表仲裁，区分删除与跨 OWNER 搬迁，并沿基会话展开
  不可达子树；控制台新增“成员补删”计数。新增停服维护工具
  `db_window_repair`，以独立 staging journal 纠正已提交窗口并保持水位不变。

- 增量 worker 控制台补齐可观测信息：检测保存时打印保存时间与 sesno 会话区间，
  各执行阶段打印当前窗口，完成时打印新增、修改、删除及合计数量；暂存提交超过
  10 秒时持续输出等待心跳，避免长事务期间看起来没有响应。

- 修复 `scripts/e3d/run_ams_gui.bat` 只启动裸 `des.exe`、未完成 AMS 模型树与
  DrawList 初始化的问题：启动流程现复用 repaired AMS 链，显式绑定
  `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample` 的项目 evars，隔离用户/工作目录，
  跳过串入 YCYK 的通用修复宏，并以 graphics finisher 与 runtime health probe 验证
  MDB、模型树、3D 文档和编辑运行时。

- 收口 Oracle 二次审核缺陷：CATA 缓存使用实际分窗右端并按 Ref0 冲突精确阻断；
  维护纠正复用冻结快照；epoch 激活与任务冻结共用激活门；尾事务使用稳定 commit token
  幂等确认并区分 `commit_tail/commit_reconcile`；planner 对包装后 SQL、行数和条目数执行
  硬上限。manifold 失败改为双策略：暂存窗口 `Required` 阻断水位，窗口外
  `BestEffortFallback` 保留诊断与旧几何。三个依赖仓库和 manifold-csg 均固定已发布 revision。

- 新增布尔降级账本 `geom_error` 表：跳过之后 `model_update_pending` 那一行不再产生，
  控制台那句也会滚走，于是「哪些件的洞没切」事后无从查起。现在每次跳过按
  `(kind, target)` 归行落库——`kind` 为 `bool_pos` / `bool_neg`，带栽在哪块几何
  （`geom`）、累计次数、首末时刻与最近一句错误，同一元素下次布尔做成时自动销账。
  `SELECT * FROM geom_error ORDER BY last_seen_at DESC` 直接可查，`/api/v1/health`
  新增 `geom_errors` 摘要（表空是 `null`）。写的是 `SUL_DB` 而不是暂存路由：诊断账本
  要在窗口回滚后依然留着，累加计数本身也不满足 journal 的幂等要求。

- CATA 引用元素改用确定性 `CONTENT` replacement，UDA 与 owner 集合先清后写；缓存键
  同时包含源窗口右端和 CATA 文件指纹，且只在 DESI 水位提交成功后发布。依赖错误、
  `missing > 0` 与连续 300 秒无实质进展现在使暂存窗口失败并保持水位。周期 reconcile
  遇到运行任务时保留活动 epoch，新的 manifest 延后到任务终态后的重扫激活。

- 修复启动 epoch 延后模型阶段时绕过 CATA Required 依赖，以及 CATA replacement 用
  `FOR $row` 被 staging journal ReplaySafe 拒绝的问题；replacement 改为显式记录目标，
  UDA 以 500 元素为上限合并事务。8000 live 重放解析 404、缺失 0，水位 33→232，
  journal 由 1284 条降至 404 条，失败重放与成功提交均保持水位/暂存原子语义。

- 修复会话预算枚举对 `pdms_io::PdmsIO::get_nearest_large_sesno` 现行 `Option<i32>`
  返回值仍按旧 `Result` 形状解构造成的测试编译失败；缺少后继会话时现在按既有语义
  结束稀疏会话枚举，预算切点仍只落在真实 SAVEWORK 边界。

- 修正 issue #5 真实房间 live 用例的计划断言：生产计划在 BRAN `RegenRoot` 后还会为
  被移动管件排 `PostRegenAabb`，旧断言只接受单个重生成工作项，导致正确计划被误报；
  现在同时钉住整根重生成与靶件 AABB 后处理，仍拒绝管件自己的 `Transform` 路线。

- 修正 `test_cal_rooms` 把旧 AMS 切面的 124 间房/147 块面板写死为永久不变量的问题：
  常规 live 对拍只要求房间与面板集合非空，精确切面计数改由
  `AIOS_EXPECT_ROOM_COUNT` / `AIOS_EXPECT_ROOM_PANEL_COUNT` 显式钉住；核心判据仍是单构件
  增量重算与同轮全量基线逐边相等。测试的空间树前置也改走生产启动同款
  `load_project_tree_verified`，让空间状态机进入 Ready；旧 `load_aabb_tree` 只读文件却不
  完成发布门，当前状态机会正确报 `SPATIAL_TREE_NOT_READY`。

- 生产布尔的空差集不再静默吞件（specs/009 T025 的静态半、ADR-029 决策 3 对齐到
  manifold 生产路径）：`apply_insts_boolean_manifold_single` 与
  `apply_cata_neg_boolean_manifold` 在 `subtract_negatives` 之后各加一道门——差集
  网格为空（verts/idx < 3）就不写 `.mesh`、不写 `booled_id` / `inst_geo`，标记
  `bad_bool` 并出声，与 OCC 对拍路径「切洞结果为空不覆盖已有 booled_id」同一条
  不变量。顺手拆掉同函数里恒假的 `found_need_occ` 分支与恒真的 `success` 死代码。
  源码断言 `empty_difference_is_bad_bool_not_a_silent_swallow` 钉住两条路径。

- 挤出截面的 FRADIUS 倒角不再静默变直角：`tessellate_extrusion` 之前把轮廓顶点 z
  （倒角半径）直接丢掉，带倒角的 `PrimExtrusion` 全部出直角、无警告、不回退。
  现在每环先过 aios-core `wire::gen_polyline_original`（OCC 路径同一份权威实现，
  z>0 顶点换成圆弧）再按 `Extrusion::tol()` 的弦高容差 `arcs_to_approx_lines`
  折线化；首环建不出即失败，孔环建不出跳过（与 `gen_occ_wires` 同一容错口径）。
  样条轮廓（`CurveType::Spline`）没有 libgm 等价实现，回 `None` 走 OCC，不再拿
  控制点连线冒充。新增体积对拍（四角 r=20 方截面 vs 解析值 1%）、带孔轮廓不破、
  样条回退三条单测。

### 新增

- 增量监听限定域：新增 `DbOption.toml` 的 `watch_dbnums` 与 `aios-database serve
  --watch-dbnum 7998,8000`（命令行压过配置），把增量摄入的数据批次圈到指定 dbnum，
  调试时不必再让整个项目 287 个 DESI 陪跑。SYS meta（SYST/DICT/GLB/GLOB）不受限
  ——MDB 的成员名单就存在那些库里，圈掉只会得到「什么都没发现」的假现场。两者都
  没给时判定与本特性引入前逐位相同（`an_unset_watch_scope_leaves_the_scope_verdict_untouched`
  钉住）。它与 `--debug-dbnum` 各管各的（那个额外带链路追踪、刻意进不了配置文件），
  与早已被剥夺增量否决权的 `manual_db_nums` / `exclude_db_nums` 也无关。
  **因为形状与坑出 issue #10 的手写名单一样，护栏比 `--debug-dbnum` 只多不少**：
  跳过理由、重扫聚合、回执声明三处都点名 `watch_dbnums` 并说清限定来自配置还是
  命令行（配置里的名单能躺一个月，命令行的进程一停就没了，两者的处置不同），
  与 MDB 范围判定、调试限定两种嗓音两两无交集；启动横幅同时挂在 `run_cli` 与
  Python `full_init` 上，`/health` 新增 `watch_dbnums` / `watch_dbnums_origin` 两栏。
  新增单测 12 条，含两条源码顺序断言（重扫三个桶的分桶次序、两个入口回执的声明
  位置）。

- 元素 diff 与 core.dll 的对照收口成两条显式已知边界（ADR-032 +
  `docs/evidence/2026-08-19-core-element-diff-boundary-audit.md`），行为未变。原先怀疑
  的五条分歧查证后三条不成立：OWNER 走 `elementIncluded` 我们已按 ADR-009 做了；属性
  宇宙用 schema 表还是键并集在结果上等价；core 按 `DB_Attribute::type()` 分十二类的比较
  **每一类最终都是精确比较**（`D3_Vector::operator==` 就是三个 double 逐个比，没有
  epsilon），分类只因为它拿到的是 typed 值。UDA 的 `isUdaUnset` 需要
  `hasAttributeChangedBetween` 第八参为真，而 `elementsChangedBetween` 传 0——core 自己在
  这条链上就关着。剩下两条记为边界：**A** 成员差分只有整表三态，没有 `DB_MemberCompare`
  的逐成员 kind（`kind == 3` → `elementReordered`），不实现是因为 `ChangeBucket::Reordered`
  今天既没人产也没人读，新增守卫 `the_reordered_bucket_has_no_producer_and_no_consumer`
  钉住这两头、任一被打破即红并指回 ADR（两条回退各自实测变红：`src/` 下多一处提及会被
  点名，`user_change_buckets` 里多一行产出会报「提及数 2 ≠ 1」）；**B** 没有 `DB_Uda::oldToNew` 的旧键归一化——它
  不是值的语义归一化而是键迁移重映射（值 > `0x171FAD39` 时查 `DB_Attribute::findOldKey` /
  `DB_Noun::findOldKey` 换成当前 id），门是 `ityp ∈ {51, 52}`，且同一调用挂在
  `DB_Element::getAtt` 七个重载与 `getInt` 上——**是读路径归一化而非 diff 语义**，真要对齐
  落点在 parse 层。受影响的属性是可枚举的九个（ityp 51：`GTYP`/`USYSTY`/`QUES`/`ATNA`/
  `AKEY`/`CURTYP`/`ATTSET`；ityp 52：`BASETYPE`/`DBELET`），全是 `TYPE=6`/`SIZE=1`，其中
  `GTYP` 就挂在我们解析用 schema 的 55 个 noun 上；但重映射只在**值** > `0x171FAD39`（即指向
  用户自定义的 UDA/UDET）时才动手，暴露面收窄到「用了 UDET/UDA 且定义重编号过」的项目。
  现有 db8000 语料是常规模型数据、本就不含这类事件，探针跑出 0 也只能证明「本语料未观测
  到」，故按 ADR-002「仅在发现与 core.dll 分歧时才对齐」两条都不预先实现。配套把
  `AttrInfo` 加上 `#[serde(default)] pub ityp: Option<i32>`（`None` = 尚未采集，不是取值 0）：
  `all_attr_info.json` 是 JSON 而非 bincode，旧文件不带该键照样反序列化，行为零变化；仓外
  唯一构造点 `noun_layout.rs` 填 `None`。ityp 的数据其实已经在
  `output/noun_attr_fields.json`（`NounLayoutExport.cs` 顺带产出的 57 字段字典转储，4271 个
  属性、ITYP 零缺失），不需要 live E3D 采集。同时记下一条待决项：
  `core-primary-list-e3d31.json` 的 `core_sha256` 与本机 E3D 3.1 `core.dll` 实际哈希对不上
  （字节数精确相同、文件早于采集两个月未改动），而现有断言把该字段钉成硬编码字面量，
  结构上抓不到这类溯源漂移。

- 逐会话实体回放升级为跨仓编译隔离：`old-pdms-io` 与主仓新增默认关闭的
  `legacy_session_replay` feature，生产构建不再编译回放 API；Python 调试绑定、诊断
  探针与 replay oracle 显式启用。以无 feature compile-fail 和生产 check 取代可被
  helper 绕过的 `include_str!` 字符串禁调，净窗口、HTTP DTO、水位和暂存协议不变。

### 修复

- ADR-017 phase-1 审计的四处收口。① **写回的确定性失败原来没有出口**：它无条件走
  `retry_until_recovered`，那个函数返回 `(T, u32)` 而不是 `Result`——4 次快重试之后每
  30 秒重放同一份 journal，永远不返回；而这一整段持 `STAGED_COMMIT_SERIAL`，于是一条被
  持久层确定性拒绝的语句会把 `fast_delete`、提交后空间收敛与其余 dbnum 一起停摆，外在
  表现是「增量跑了、模型没变、重启还是同一区间」，控制台每 30 秒刷同一行。现在按
  `staged_writeback_failure_is_transient` 分流：断连与写冲突照旧无限等（必然自愈），其余
  判死——记 `window_block`（reason 带原始错误）、DROP 窗口、放锁、批次转 Failed；水位没动、
  持久层零痕迹，journal 随窗口丢，恢复路径与崩溃同一条。journal 入口早有
  `ReplayUnsafeRejection` 判死确定性拒绝，这是它在写回端缺失的对偶物。② **而「确定性写回
  失败」不是假想**：暂存库刻意不装 `update_dbnum_event`、持久库装，这道有意的不对称制造了
  一类「暂存全绿、写回逐条被拒」的语句——pe 上一旦生效的是那版对数组形制 record id 用
  `array::at` 的旧实现，整窗口写不回去，而发现它的时刻是解析与生成都已白跑之后。
  `create_window` 现在进程内一次性读回事件定义验指纹（好版含 `string::split`），坏版直接
  拒绝开窗、不建实例；读不到按瞬时故障放行，只缓存成功，排毒后下一次开窗自动放行。
  ③ **资源废弃档位没有可执行的出口**：`Abandon` 只在语句入口 bail，冒泡上来与普通失败无从
  区分，而这类失败重算不会好（同一会话区间必然再次触顶，窗口只被吸收扩大、没有按 sesno
  拆窗的机制）。现在按档位单独记 `window_block`，reason 带测得的字节 / 行数、生效上限并点名
  `AIOS_STAGING_ABANDON_BYTES` / `AIOS_STAGING_ABANDON_ROWS`。④ 退役
  `sweep_orphan_staging_databases_on`：每窗口一个独立 `mem://` 实例之后跨窗口孤儿库不存在，
  生产侧无调用方，留着只会让人以为还有一层兜底回收。回归四条（三条实测过回退变红）：
  `a_deterministic_writeback_failure_returns_instead_of_holding_the_lock`、
  `only_transport_and_conflict_count_as_transient_writeback_failures`、
  `a_rejected_writeback_records_a_block_and_releases_the_window`（源码钉：记阻断 → 放窗口 →
  交还终态）、`the_writeback_schema_gate_runs_before_the_window_instance`（源码钉：兼容门
  必须排在建实例之前）。ADR-017 同步修订，并把两处「文档与实现不一致」写进正文：规则④的
  水位行只读拷入例外，以及 §6 之外那档更宽的模型让位口径（`epoch_id > 0`，重扫排出来的稳态
  DESI 也在内，这批的模型走窗口外逐根直写——决策 1 的整窗口原子在这条路径上按 ADR-025 §7
  被交换掉了）。已知仍未解决并留在台账：拆窗机制不存在；资源计量用 SQL 文本字节做摄入代理，
  `estimate_write_rows` 对带 WHERE 的集合写按 1 行计，两处都低报。

- 去除跨仓构建对宿主 `protoc` 的依赖：`dpcsync` 删除 `build.rs` 与
  `prost-build`，检入字节一致的 `prost` 生成文件；`old-pdms-io` 钉住该提交，主仓
  删除未使用的直接 `dpcsync` 依赖。PROTOC/PROTOC_INCLUDE 均未设置时三仓构建与
  回归通过，依赖树中不再出现 `prost-build`。

## 2026-08-18

### 新增

- 增量窗口收集统一到净窗口单一口径（ADR-031）。`collect_window` 不再读灰度开关、不再走逐会话回放；预览与执行共用 `collect_net_window`（与 core.dll `elementsChangedBetween` 同思想的双根 B+ 差分）。`net_window_collection` / `AIOS_NET_WINDOW` 退役，残留设置在 `run_cli` 与 `aios_db.full_init` 打显式告警——配置层没有 `deny_unknown_fields`，删字段会让 `net_window_collection = false` 被安静忽略，字面意思与实际相反。`collect_changes` 降为 legacy 诊断入口（性质 h/i、live 对拍、逐会话取证），生产链四个函数体的源码断言禁调。口径升级对外可见（原 ADR-022 §5，现无条件生效）：改了又改回不再触发 regen；加了又删不再留墓碑行；删了又建判净修改；逐会话明细退出主口径。回退手段是 `git revert` 单路径提交，不是解开配置键。T11b 改净臂单跑；执行层双臂 A/B 退役为历史证据（2026-08-13 两轮全绿仍在档）。T18 release 完整收集按协议入档（记录项，非门）：testbed 8000 高复触窗 warm median 10ms vs 53ms ≈5.3×，Add 地板窗 128ms vs 908ms ≈7.1×；SYST 250206 列为上线后现场复测。

- 两条启动检查 live 用例（`increment_manager::tests`，live 8019），钉住「已解析的库旁边新增一个库」这个现场形状。此前只有 08-17 那条单库用例，够不着的正是下面两件事：
  - `live_startup_sweep_routes_a_new_db_to_baseline_beside_an_applied_one`——**两条路由在同一份清单里不串味**。存量库（默认 8000，`applied=file_latest` 且 pe 有数据支撑）与从未解析的新库（默认 7998）放进同一个一次性监控目录：一轮启动重扫里存量库一行都不排（`discover_batch` 的水位早退），新库排 `apply_window` 窗口 `1..=file_latest`、worker 走 `needs_initial_load` → `initialize_dbnum_baseline`、回执含「首次按需初始化完成」，收尾断言存量库水位一格没动。存量库那侧的两个前提（追平、有支撑）写成断言而不是注释——差一个会话是普通增量、pe 零行是幽灵水位，两种都会入队，「一行都不排」也就无从断言。库号可配（`AIOS_STARTUP_APPLIED_DBNUM` / `AIOS_STARTUP_NEW_DBNUM`），默认走秒级的 7998，真靶按现场口径设 7999。
  - `live_scope_refresh_baselines_a_db_the_mdb_just_declared`——**MDB 才是增量范围的定义**。库文件全程躺在监控目录里，变的只有 MDB：把目标库从当前 MDB 的 `CURD` 里摘掉 → 重扫一行都不排，而且**连观察值都不写**（范围门排在 `record_observation` 之前，这条断言正是它的证据）；装回 CURD → `resweep_for_scope_change` 的 `scope-refresh` 重扫把它发现出来，照样走首次导入基线。复刻的是生产里「有人往 MDB 里加一个库」——那些刚进范围的设计库自己没有任何文件变更事件，不重扫就得等下次重启。夹具只动 `CURD`（`DBLS` 不碰，摘除态仍是「库存在、只是本期不在成员名单里」的合法现场），原样 CURD 存进 `queue_control:test_mdb_curd_backup` 再按原样写回；主体断言包在 `isolate_panic` 壳里，保证无论红绿都先扶正 MDB，用例开头另无条件还原一次以自愈上一轮的中断。
  夹具助手一并抽出共用（`locate_watched_db` / `isolated_watch_dir` / `queued_row` / `restore_registered_path`）：隔离目录这一手不是图省事——沙箱里躺着二十多个范围内却从未解析的 DESI 与一批 CATA，整面重扫会把它们一起排进多相位清单，而相位屏障要靠生产 worker 的重扫循环才走得完，`drain_queue_until_empty` 单独消化不了。两条各跑两个靶、**2026-08-18 四轮全通过**（默认 7998 靶 13.13s / 13.23s，现场口径 7999 靶 75.86s / 72.37s，窗口 `1..=120`，@8019），跑后实测沙箱完全还原（CURD 71 项、备份行 0、7999 水位回到 120 且 pe 有支撑、8000 停在 209 未动），台账已同步。

### 修复

- core.dll 净窗口的第二层“候选元素两端属性/成员 diff”收敛为单一实现：将
  `diff_ele_data` 提取到本地 `old-pdms-io`，legacy 会话回放与生产净窗口共同调用，
  删除 `net_window.rs` 的复刻分支并保留 re-export 兼容原符号路径。共享纯函数覆盖
  普通属性、显式属性、UDA（按 hash）和有序 children；vendor 新增 2 条纯单测，
  主仓净窗口 13 项、pipeline 48 项、会话夹具 20 项、两条真实文件对拍及 issue-019
  全链 `2 passed` 均通过。以后任一桶语义修正只改一处，不再产生净路径/回放漂移。

- core.dll `DB_Noun::primaryList` 门控从恒 `true` 改为权威快照驱动。通过已初始化
  E3D 3.1 进程直接调用 `db_get_element_info(noun_hash, 297853135)`，冻结 1931 个
  noun：1879 resolved（true 1142 / false 737）、52 unknown；resolved false 现在
  真正抑制 DB_UserChanges 成员事件，unknown 单列并保守为 true。快照钉住 core.dll
  SHA，采集脚本可复跑；B-EVT-03 同时覆盖 DAMP=true、TP=false、ROD=unknown 与
  `user_change_buckets` 实际调用。净窗口三态、children 两端持久化、模型 Regen 与
  公开 DTO 均未改变。

- 净窗口审核缺口闭环：退役键探测从 `DbOptionExtFields` 整体反序列化中拆出，独立读取原始 TOML，键值无论布尔/字符串/整数都报警；配置缺失、读取或语法错误显式报告，`AIOS_NET_WINDOW` 用 `var_os` 覆盖空值与非 Unicode 值，CLI/Python 接线断言收窄到函数体。Python 全链签名与 T11b 改用已跟踪 issue-019 baseline@24/final@26 作为固定真值，严格钉住 `changed_elements=3`、会话 `[25,26]`、水位 26、a/m/d=`0/1/2`、精确两个墓碑与活行恰减 2；移除运行时同源 oracle 和可变切点。正常档 `2 passed`，强制空跑在起点活行断言准确变红，清变量立即复跑通过，原 db8000 SHA 无损恢复。固定窗口还揭示 preview 会漏掉“冻结页存在但净操作为空”的会话，现改为从冻结会话页清单预建 `sessions[]` 并补纯单测。live 台账、ADR-031 回执、Web API 规格与证据同步。

- `rebuild_dbnum_info_from_pe` 把整个库的 pe 行拉回客户端再在 Rust 里按 Ref0 分组（`SELECT record::id(id) AS key, sesno FROM pe WHERE dbnum = N`），目录库量级直接把 ws 连接打死：2026-08-18 现场 ams7351 有 3,345,853 行，语句吊了 9 分钟后 router 任务连同 channel 一起没了，报 `read PE stats dbnum=7351 failed: Internal error: receiving from an empty and closed channel`，把前面那趟 **2.6 小时**的全量解析整个作废（数据其实已经全部落库、统计也由事件维护到位，死在的是这次纯多余的回读），批次 failed 后按相位门连坐把 catalogue 之后的 design 相位一起堵死。两处一起改：① 聚合下沉到服务端 `GROUP BY ref0`，返回行数等于 Ref0 个数（个位数），传输量与内存与库大小无关——SQL 抽成 `pe_stat_groups_sql()` 单一事实来源供测试与生产共用；② `sync_total_async_threaded` 结尾不再无条件重算——这条路径**不摘事件**（摘事件的是 `sync_pdms` / `sync_sys_only`），统计一路由 CREATE 分支维护到位，先用两条索引支撑的便宜计数问一句「对不对得上」（`classify_stats_settlement`），对得上就只补事件写不出的身份字段（`UPDATE`，无 DELETE，事件那条行原地留着），对不上才付全量重算的代价。回归测试三条：`stats_rebuild_aggregates_server_side_one_row_per_ref0`（两个 Ref0 五行 pe，聚合必须返回 2 行；退回逐行回读即报 `missing field 'ref0'`）、`stamping_identity_keeps_event_maintained_stats_in_place`（靠事件写的 `updated_at` 区分「原地补写」与「删了重建」；退回无条件重算即红）、`absent_or_mismatched_stats_still_pay_for_a_full_rebuild`（统计整体缺席时两侧和同为 0，不许被认成「对得上」）。三条都实测过回退变红。

- 初始化批次让位模型相位时，`defer_model_phase` 分支无条件打「初始化数据与水位已收口」，**失败批次照打**：2026-08-18 现场 ams7351 数据批次 failed，日志里却先宣告收口、下一行才是失败记账，排查只能绕到 `/api/v1/tasks/<id>` 回执里才拿到真错。改为按 `applied` 分叉，没推上水位的批次在同一行说清楚「未收口、模型工作不领取、原因见回执」。

- `pe` 表统计事件 `update_dbnum_event` 的 DELETE 分支在统计行缺席时把整条删除语句打死：写法是 `UPSERT type::thing('dbnum_info_table', $ref_0) MERGE { count: count - 1, ... } WHERE count > 0`，而 UPSERT 在目标行不存在时走的是**创建**路径，`WHERE` 拦不住，`NONE - 1` 当场报 `Cannot perform subtraction with 'NONE' and '1'`。统计行缺席是常态而非异常——`sync_pdms` / `sync_sys_only` 为了性能先 `REMOVE EVENT` 再写 pe，那批行天生没有统计行（2026-08-18 现场：ams5100 有 236 条 pe、零条统计行）——于是 `fast_delete` 的 Ref0 range DELETE 必炸，首次按需初始化与整库重建都过不去，批次 failed 后还按相位门连坐阻断同相位其余库（现场 dbnum=5100 卡死 meta 相位，design 相位的增量窗口排在后面永远轮不到）。改为 `UPDATE ... count?:0 - 1`：`?:0` 对齐 CREATE 分支 2026-08-06 审计定下的 NONE 免疫惯例，`UPDATE` 在 SurrealDB 2.x 只改已存在的行、缺行空转——不能让删除事件凭空造一条统计行，那条 MERGE 压根不写 `dbnum`，造出来的行连 `DELETE dbnum_info_table WHERE dbnum = N` 都清不掉；缺席的统计交给 `rebuild_dbnum_info_from_pe` 重算。事件 SQL 抽成 `dbnum_event_sql()` 单一事实来源供测试与装载共用，回归测试 `deleting_pe_rows_without_a_stats_row_neither_fails_nor_fabricates_one` 走 mem 引擎复刻现场形状（事件装载**前**写 pe，再删），已实测：回退成旧写法即报同一个 `TrySub("NONE", "1")`。

- 净窗口收集（ADR-022）在一切真实库文件上必现失败：cea58087（08-14）把三种真实文件常态升成了整窗硬错误，回退为「跳过 + 记账」并把回归钉进单测。三处分别是——① 索引树下降时**子页读不动**、② **子页层级不低于父层级**：这两种都是回收页残留的形状，生产点查 `filter_index_data` 对它们同样静默跳过，点查到不了的分支本就不参与触达集，跳过整枝不损完整性，现在计入 `unreadable_child_pages` / `level_anomalies`（索引**根页**读不动仍是硬错误，那是证明不了完整性）；③ **终稿记录解析失败**：全窗 1..=230 实测 64 条字典缺项的系统记录必现（首例 `16192_1` 报 `MNUM not exist in attr_info_map`），而回放路径对同一批记录同样以 `None` 操作落空、从未入库，硬失败等于每个含系统段的窗口整批打死，现在跳过 + `unparseable_finals` 计数 + **聚合**警告（逐条刷屏会把回执淹掉）。三处容忍都不许静默：形状进 stats、明细随回执透出。单测 `level_regressions_and_routing_anomalies_are_counted_and_flags_stay_blind`、`unreadable_child_pages_are_skipped_with_a_count_and_a_bad_root_is_fatal`、`an_unparseable_final_is_skipped_counted_and_aggregated` 各自带回归背景注释，改回硬错误即红。ADR-022 与 specs/003（Edge Cases + FR-8）同步修订并附修订说明。两条 ams8000 纯文件 live 用例复验通过、台账已同步。

- 增量更新审核（2026-08-18）五项收口。① watcher 重扫把被 `--debug-dbnum` 圈掉的库混进「不在 MDB 声明名单」聚合——对它们那句是**事实性错误**（在名单里，只是被调试限定圈掉，范围判定本轮根本没问过），正是 issue #10 的嗓音混同：D7 护栏一只钉了 `skip_reason` 的两种说法无交集，没钉 sweep 真的走它（`skip_reason` 在生产路径上无人调用）。现分成两个聚合桶各说各话（保留聚合防刷屏），调试桶点名 `--debug-dbnum` 并自证「不是 MDB 范围判定」，分桶判定复用 `debug_scope_admits` 与 `skip_reason` 同序（调试门在前），源码形状回归 `the_sweep_keeps_debug_exclusions_out_of_the_scope_bucket` 钉住。② 净窗口两种容忍形状 `unreadable_child_pages` / `level_anomalies` 只进 stats，而 stats 在 `collect_window` 拼完口径 warning 后即被丢弃——批次回执上「跳过整枝」实际静默，只有 python 探针的 `to_json` 能看见，违 spec-003 FR-8「任何一种容忍都不许静默」。现随口径标注透出 `不可读子页(t/b)` / `层级异常(t/b)`（t/b = target/base，与台账叙述口径一致），口径标注抽成 `net_caliber_warning` 纯函数，单测 `the_net_caliber_warning_carries_the_tolerated_shape_counts` 钉住，从 warning 里删掉计数即红。③ `debug_scope::trace` 的载荷改为闭包惰性构造：json! 实参是急切求值的，此前八个追踪点在未启用时也逐次构造 JSON（扫描点每轮全面重扫逐文件走到），调度器入队点还无条件取 `InitializationCoordinator` 的 allows+snapshot 两把锁——与「未启用零成本」的自述不符；闭包在 debug_scope 锁外执行以免锁序问题，惰性由 `tracing_is_inert_until_the_switch_is_set` / `tracing_ignores_dbnums_outside_the_debug_scope` 用会 panic 的载荷闭包钉住。④ `mode_notice` 说「其余 DESI 一律跳过」但豁免名单是 COLD_START_DB_TYPES（SYST/DICT/GLB/GLOB），CATA 一样被圈掉——措辞改为如实点名「DESI、CATA 等非 SYS meta」（行为未变：调试限定本就只该放行 SYS meta，CATA 是 ADR-025 正式数据阶段，要不要豁免属设计裁决，本轮不动）。⑤ `aios-database trace` 子命令写死 `curl.exe`，在 CentOS 7 部署目标上必挂——按平台 cfg 选 `curl`/`curl.exe`。

## 2026-08-17

### 新增

- live 用例 `live_startup_sweep_baselines_a_never_parsed_db`（`increment_manager::tests`，live 8019）：钉住 ADR-023 §4 生产缺省形状——范围内**从未解析**的库（无水位行、无统计行、无 pe 行，`delete_dbnum_fast` DropRow 制造，对应「新库文件第一次进入监控目录」）被启动重扫自动发现入队（上弦后 queued 不挂起、窗口 `1..=file_latest`），worker 冻结点 `needs_initial_load(0, latest)` 路由进 `initialize_dbnum_baseline`，终态 succeeded 且回执含「首次按需初始化完成」，水位推到 `file_latest`、pe 有数据支撑。与既有幽灵水位用例的分界：那条留着撒谎的登记行，这条连登记行都没有。watcher 指向只含目标库副本的一次性目录以收窄清单（全目录重扫会把沙箱里其它未解析库一并入队，多相位清单要靠生产 worker 的相位重扫循环才能走完，`drain_queue_until_empty` 单独消化不了——首轮红跑实测确认）；结尾以正本路径补一次扫描裁决，`PathMigrated` 自动迁移还原登记路径。**2026-08-17 通过**（测试体 10.0s），证据 `docs/evidence/2026-08-17-never-parsed-auto-baseline-live.md`，台账已同步。

### 修复

- db7999 设备九场景夹具首次全绿（9/9 PASS，四平面断言全过，隔离副本 `test-increment`）。此前两轮全红于 `saved session N is absent from data task merged_sesnos`，`--debug-dbnum 7999` 追踪定位为**夹具缺陷而非引擎回归**：`execute_fixture_and_wait` 在 SAVEWORK 之后轮询 `POST /update/preview` 等窗口张开，而预览唯一的写操作 `record_observation` 会把 merged_sesnos 冻结基线推到本窗口右端（红轮 trace 里全部 19 条入队记录 frozen_prev==右端），并入名单按规格恒空。就绪门改为本地读镜像文件头（`file_latest_sesno`，镜像拷贝本就同步、无需轮询），门票与预期会话号落盘 `execute-gate.json`；删除只剩这一个调用方的 `find_observed_window`。源码形状回归测试 `the_execute_gate_reads_the_file_header_not_preview` 钉住「就绪门不得咨询 preview」。红轮诊断与绿轮证据：`test-increment/runs/fixture7999-20260817-{145932-trace,154724-gatefix}/`，台账已同步。
- 初始化执行过程审核（ADR-025 链路）四项收口：
  1. F6 重扫读不出 DESI 最新会话号时此前只 warn 就跳过——清单缺着这个库照样宣告 `data_ready`、模型门照开，库持续读不动时外面毫无痕迹（DICT/CATA 头不可读却是阻断 Meta 的，同一种「观察不完整」两副面孔）。现在读失败记进对应阶段 blockers，该阶段保持可见地不就绪，共享盘瞬态靠周期对账重扫（默认 300s）恢复即解。源码钉 `sweep_skips_always_leave_a_phase_blocker`。
  2. 批次终态阻断数据阶段的判据从任务标签改为数据窗口本身（`batch_failure_blocks_data_phase`）：数据 Applied 而模型/副作用失败的 Partial 不再 `mark_failed`——那些失败在 durable pending 的重试账与死信门槛里，再关数据门只会让同阶段其余库连坐一个对账周期；数据批次 Failed 折成的 Partial（有单元成功）照旧阻断。单测钉 `only_an_unsettled_data_window_blocks_the_data_phase`。
  3. 新增数据批次连败账本（进程内，`/health` 的 `batch_failures`）：确定性失败此前会被周期对账重扫以每 300s 一次的节奏无上限重跑。同 dbnum 同右端连败到 `MAX_ATTEMPTS` 后重扫停止自动重跑（park，记阶段 blocker 可见），文件长出新会话或人工执行（POST /update/execute）清零复活，成功即清零；panic 路径记同一本账。单测钉 park/复活/重数三条出路。
  4. 启动主线等待模型阶段收敛时每 60s 播报一次仍在等什么（收敛在空闲轮里，任一环失败按 30s 退避，此前主线干等像挂死）；worker 里与 `run_cli` 重复的队列暂停恢复播报静默化（独立入口兜底保留，失败仍出声）。并入名单的基线（上一次扫描观察值）过去由 worker 执行体到冻结点再现读，而入队扫描早已把 `file_latest_sesno` 推到本窗口右端，「相对预览新增合并的会话」于是永远算不出来（2026-08-16 pipeline-f5 现场，`restore-execute-receipt.json` 里 `merged_sesnos: []`）。改为在入队时冻结基线并随队列行传递：发现方（execute 端点 / F6 sweep）在 `record_observation` 覆盖之前从裁决取 `previous_file_latest_sesno`，经 `DiscoveredBatch → DataBatch → FrozenBatch` 一路带到 `execute_one_dbnum`；同 dbnum 排队合并时基线只认最早那一次观察（取最小值）。回归测试三层钉住：批次队列纯规则（`the_baseline_freezes_at_the_earliest_observation`）、调度器冻结快照（`the_frozen_job_carries_the_earliest_enqueue_baseline`）、执行体源码断言（`the_merge_baseline_is_frozen_at_enqueue_not_reread_at_execution`，执行体再出现 `previous_file_latest_sesno` 即红）。

## 2026-08-16

### 新增

- 扫掠体自建网格器 `src/fast_model/sweep_mesh.rs`（WP-C 的 C0/C1/C2/C3 内核）：截面 → 2D 闭合环 → 三角网格，三支分派直接用 `SweepSolid::do_solid_segments()`（Core3D `DB_Gensec` 的权威判定，本模块不另立一套）。截面语义不重写——倒角与弧段复用 aios-core `wire::gen_polyline_original`（OCC 路径用的同一个函数），弧转折线用 cavalier_contours 的 `arcs_to_approx_lines`，端盖用 earcutr 做带孔多边形三角剖分（结构截面 L/C/I 是凹的，扇形三角化会填掉凹口）。摆放变换逐行对应 `gen_occ_spro_wire` / `gen_occ_sann_wire`，斜切端面沿用 `get_face_mat4`。360° SANN 按外环加内孔一次成形，两段半圆弧拼（`bulge = tan(θ/4)` 在 360° 处发散，单段圆表达不出来），另有一条测试与两个半环之和对拍以满足 FR-006。**尚未接进 `tessellate_libgm_param`**：生产接线要等直墙/斜切墙/弧墙的 RVM 门，本轮只到纯函数验证。
- `src/fast_model/mesh_assert.rs`：网格体检断言抽成 test-only 共享模块，`mesh_primitives` 与 `sweep_mesh` 共用同一套判据。
- 依赖新增 `cavalier_contours`（与 aios-core 同一 gitee fork）与 `earcutr`，两者本就在 `Cargo.lock` 里，只多了两条依赖边。
- `mesh_primitives` 单测从「非空」升级为「可用于布尔的实体」：结构体检统一走 `assert_solid_mesh`（法线齐备且是单位向量、无零面积三角、随附 AABB 与顶点一致、顶点焊接后每条有向边正反各一次即闭合可定向、散度定理算出的有向体积为正即三角朝外），再逐个原语与解析包围盒和解析体积对拍（球冠 `πh²(3R−h)/3`、半椭球 `⅔πr²h`、圆环 `2π²Rr²`、棱台 prismatoid 公式等）。7 种原语共 22 条。

### 修复

- `gen_elliptical_dish` 母线参数写反：代码用了 `x=r·cos t, z=h(1−sin t)`，与自身注释相反，生成的碟上下颠倒——底圈半径落在 z=height，z=0 处只有一个点，而底面圆盘仍按半径 r 铺，网格既破洞又带零面积三角。改回 `x=r·sin t, z=h·cos t`，随之修正法线、三角绕向（母线自下而上，绕向与球体相反）与顶点处的退化三角。
- `gen_rectangular_torus` 侧面法线全是 `Vec3::ZERO`（注释写着「后面按面片计算」但没有下文），着色会全黑。改为四个侧面各自持有顶点，法线按面给，硬边不再被平均。
- 两个环面的端面 cap 绕向与外法线反了：起始端面本该朝 −φ、末端朝 +φ，实现把两者对调，切出来的扇环端盖朝内。同时补上负角度扫掠——`CTorus::check_valid` 只要求 `angle.abs() > 0`，负角沿 −φ 扫掠会让整体内外翻转，现在按扫掠方向翻转绕向。
- `gen_pyramid` 顶面退化成一条棱（`xtop=0` 而 `ytop>0` 的楔形）时会留下零面积三角和破洞：改为四边形统一出三角、逐个丢弃退化面，点退化与线退化走同一条路径；亚微米级边长先归零，避免留下狭长三角。
- `tessellate_libgm_param` 各分支加 `covered()` 收口：`check_valid()` 放行但仍然出空网格时报错而不是返回 `Ok(Some(空))`——空网格传下去只会表现为「模型悄悄少了一件」。

- ADR-028 抽取树收尾三件：`pe_owner` 批写入补上 `INSERT RELATION IGNORE`（边 id 显式，父层补缺重放叶子已写过的边必须幂等，否则重复 id 会把整次补缺同步打成失败）；`collapse_extract_families` 输出按（项目, 库号）排序钉住跨进程扫描序（原 HashMap 迭代序随机）；F6 重扫给「抽取树父层被叶子代表」补日志、给「文件名库号与文件头不一致」单独文案（原先与多副本共用一句「多个抽取/副本」，单文件 mismatch 时误导）。
- ADR-028 父层补缺静默空转：`collect_project_db_files` 归并抽取家族时会把被叶子 shadow 的主库从解析清单里删掉，而基线的父层补缺（`included_db_files` 点名主库）恰恰要解析它——补缺同步一个文件都不解析却返回 Ok，缺口留在库里。现在被 `included_db_files` 显式点名的 shadow 主库回到清单；补缺同步返回值里若没有目标 dbnum 直接报错、不推进水位。附回归测试 `collect_project_db_files_keeps_explicitly_named_shadowed_master`。（同批核实：pe/属性批写入本就是 `INSERT IGNORE`，父层同步天然只补叶子缺号，不会用父层旧会话覆盖共享 refno。）
- 订正三处与实现相反的默认值注释：`startup_autorun` 与 `room_incremental` 的缺省均已是 `true`（分别在 2026-08-14 与 2026-08-12 翻正），但 `options.rs` 的字段注释与 `/health` 的两条字段注释仍写着「默认 false」。同批把 `startup_autorun` 关闭时的机制描述改准：它挂起的是重扫排出的队列行与空闲轮的持久积压，不是「队列消费者启动即暂停」——那是另一道跨重启保留的暂停闸门。

## 2026-08-15

### 新增

- ADR-029：设计/目录负体布尔改走本地 `manifold-csg`（path `../manifold-csg`），不再调用 aios-core 的 `ManifoldRust` / 旧 `manifold-sys`。OCC 只保留三角化（扫掠体尚无替代）；OCC 布尔不进生产。
- ADR-030 据 Core3D.dll IDA 修订：三角化权威是 libgm `gm_Create*`（挤出/旋转/ruled + 目录原语），不是 OCC BRep；OCC 只是翻译层。
- `specs/009-retire-occ/plan.md` / `tasks.md`：按 libgm 符号拆工作包与带路径任务（B 目录原语 / C 扫掠三支 / D CSG / E 离散）。
- `manifold_tessellate`：单位箱/柱与挤出轮廓走 manifold-csg（对齐 `gm_CreateBox/Cylinder/Extrusion`）；空挤出 hard fail。aios-core 的 `gen_model` 不再捆绑旧 `manifold-sys`，避免与 manifold-csg 链接冲突（需本地 vendor patch）。
- T005/T007：`gen_inst_meshes` 无 `occ`/`manifold` 时 `bail!`（禁止静默跳过）；箱/柱/挤出先 `tessellate_libgm_param`，斜端柱回退 OCC；libgm 路径 AABB/`pts` 取网格八角。

### 修复

- OCC 布尔若切出空网格，不再覆盖已有 `booled_id` 文件。`116569` 的空 60 字节结果已用 manifold 重切恢复（p95=137）。

## 2026-08-14

### 新增

- 抽取树叠加（ADR-028）：同项目主库与唯一 `_NNNN` 归并为一个逻辑库，叶子为水位权威、父层只补缺号；兄弟抽取与人手副本仍 Duplicate。
- Python 解析层暴露 `parse.collapse_extract_files` / `parse.parent_gap_refno_count`；`python/tests/test_extract_tree_offline.py` 离线钉住归并，本机 AMS 7355 实文件再钉头/会话/parent_only=0。
- 新增 ADR-024 与 `specs/005-shape-save-coalescing`，定义模型实例保存的有界合批、先计划后修改、确定性分包和成功后计入产出约束。
- 新增 ADR-026：扫掠体公开步骤按 `DB_Gensec` 蛇形命名；可复用直线无斜切时单位网格身份只键目录截面。
- Python RVM AABB 对拍：`python/scripts/rvm_aabb_compare.py` 先打 AMS 1112 CWALL `/1RS-WF03-W-C-RR001` 的 WALL/STWALL（SweepSolid），口径对齐 `rvm_gate_c_or_1r345_c.ps1`（3mm / 3%）。
- Mesh 级对拍：`fast_model::shared::two_sided_surface_distance` 双向采样表面距离（mean/rms/p95/Hausdorff，parry3d，无新依赖、进 CI），三角化无关；`rvm_baseline::mesh_compare` 用 rvm-rs `Tessellate` 取 RVM 世界三角、gen 侧 `inst_geo.param` 就地 `gen_occ_shape`+`gen_occ_mesh`（param 为空的布尔/复合几何回退磁盘 `.mesh`）取世界三角。对 1112 CWALL 4 堵 WALL 与 8000 C-OR 管系（FTUB/BEND）做 mesh 对拍（live 8009+occ）：墙与 FTUB 几何忠实；BEND 逐元素多约 100mm，但 BEND+相邻 FTUB 的 union 与 E3D 只差 5.8mm——是元素边界拆分口径不同、装配无害。gen 几何经 mesh 级验证在装配层正确。
- Mesh 对拍扩到 1112 直线 STWALL（双向 ≤0.06mm）与 8000 `/C-IY-1R330-B`（ACP1000 槽盒，35 构件 union 的 gen→rvm p95=4.14 / max=24.9mm）；E3D 槽体外壳比 gen 管段大约 100mm 高，rvm→gen 大是范围差。
- Mesh 对拍 `gen_world_mesh` 与生产 `query_valid_insts` 对齐：有 `booled_id` 时加载切洞后网格，不再回退未开洞正挤出。1112 三堵大体量 GWALL 重跑 NXTR 布尔后 gen→rvm p95 从 870/786/591 降到 0.1/9.3/137。

### 修复

- 管道 FTUB 续测关闭三处增量缺口：staged 初始化尾事务保留未执行的 `RegenRoot`，模型 drain 优先最新真实保存而非历史更新时间，Add 复用旧世代 Refno 时先清理全部旧 owner/children 边；F5 同步换成当前文件可达的 FTUB 夹具并增加工程控制库与依赖 Refno 前置检查。
- 管道增量续测修复两处可重放缺陷：L3 变更宏现在识别带保存注释的 `SAVEWORK`，TTY 夹具显式传递目标 DB 与项目并按会话事实分类；水位仍为 0 的中断基线会先清除未提交 PE 再全量解析，避免 `INSERT IGNORE` 永久保留陈旧行。
- 模型实例纯数据写入的源码顺序守卫改为跟随当前 SavePlan 入口，并统一 CRLF/LF 后再切片；Windows 与 CI 现在验证同一段生产保存路径。
- 四份 e2e 探针（`staged_pane_replay_probe` / `staged_regen_e2e` / `staged_transform_e2e` / `issue7_e2e_increment`）的 `DiscoveredBatch` 补上 `phase`/`epoch_id`，`Run-LiveBatch.ps1` 的 `cargo build --lib --tests` 预编译不再被挡住。
- 模型持久工作改为每页至多 16 根、逐根执行，并在认领前及每根前后检查初始化 epoch、模型门和数据队列；数据到达时以 `model_drain/yielded` 收口，未执行行不改变状态、错误或重试次数，健康状态补充让位原因、耗时和 attempts 变化量。
- E3D 变更宏按 DONE、ALIVE、退出码及保存前后会话号分类；已保存但未确认的运行继续验证而不重放 apply，只有已知的保存前启动失败允许一次重试。L3 夹具统一采用 `preview → SAVEWORK → execute`，并验证保存会话进入 `merged_sesnos`。
- L3 的 Plant UI 运行改用隔离设置文件和仓库根工作目录；自动化按 `tree_item + refno` 定位，数据批次后等待关联 `model_drain` 与 pending 收敛，不再把数据任务的 `units` 当作模型完成证据。

- `inst_relate` 世界包围盒对 `PrimLoft` 圆弧扫掠按环扇取样（含世界轴交叉），不再把局部 AABB 当盒子做 8 角变换。
- 扫掠体斜切改为相对该段切向的垂直/平行抑制（`1e-6`），不再用世界 `±Z`；BANG/PLAX/镜像/路径方向进实例变换。目录路径组合 `get_trans` 旋转，SavePlan loft 夹具改用非 Z 切向方切。

- 合并生成器小尾批的重复元数据查询与 SQL/journal 写入；NaN 和持久化 ID 冲突在删除旧模型前响亮失败。
- 启动、回退重建及同轮稳态更新统一为 `SYS/DICT → CATA → DESI → 模型 → 房间`：
  完整清单按 epoch 安装，Watcher 事件只触发防抖全量重扫；早期阶段失败、身份阻断
  或目录不可读会关闭后续数据与模型门。队列、健康状态和手动回执新增阶段/epoch/
  blocker/shadowed 观测字段。
- 新增 `catalogue_project_priority`，跨项目 DICT/CATA 同 dbnum 由显式项目顺序选主；
  同项目重复、未知配置或无优先级候选整阶段阻断，被遮蔽文件不写 observation 与水位。
- 全量 `sync_pdms` 改为全局 Meta、Catalogue、Design 三次 await；启动提前开放 Web 与
  data-only worker，最后一个 DESI 水位完成后才执行全量模型、持久模型工作、AABB 与房间。
- 启动重扫现在会在应用水位恰好追平文件时校验数据支撑：`pe` 零行且没有匹配
  空基线凭据的库按首次导入入队，由 worker 重建基线；合法空库凭据与水位同事务
  收口。生产缺省 `startup_autorun` 同步翻为 `true`，未解析库和异常水位库无需再等
  下一次文件保存触发。
- 增量正确性阻断：数据队列新增 `apply_window` / `reinitialize` 显式意图，回退到
  零会话也会以 `0..=0` 到达 worker；排队重建意图占优、运行中保留后继，冻结点
  复核仍判 Rollback 才清库。空文件清库后直接 Applied 且水位保持 0。
- `fast_delete` 的统计/持久队列、spatial epoch 与水位清零纳入同一显式事务；
  关系和 Ref0 区间删除继续独立幂等，水位更新保持事务末句。
- 幽灵水位清库会从 `dbnum_info_table` 的 record id 恢复真实 Ref0，并与 PE 前缀
  取并集后删除对应区间；不再把 dbnum 数值冒充 Ref0，PE 零行时也能清理派生行。
- 净窗口把不可读子页、层级不下降、终稿解析失败和 last-touch 缺失升级为整窗
  错误，杜绝残缺触达集落库推进水位；Modified 基版本失败仍保守降级为 Add。
- `ref_rev_maintain` 补偿载荷改为非空、全量严格解析，任一非法 refno 均进入失败
  记账并保留队列行，不再以空修复调用静默销账。
- 收集接口统一为 `CollectedWindow`，把冻结窗口的实际会话页清单与操作流、口径和
  warnings 一起贯穿预收集、崩溃重放及成功回执；空保存、自抵消和稀疏会话现在
  正确进入 `merged_sesnos` 及平行保存时刻。Replay 清单与操作共用一次文件打开，
  两种模式首条 warning 固定自报口径，且后续计划失败也会保留该 warning。
- 基线入口开始硬消费共享 `ScanGate`：身份阻断和回退重建均在计数/解析/水位前退出；
  范围外 CATA 改为跨 scope 收齐全部候选后复用 watcher 的同项目 dbnum 判重，重复组
  零 observation、撤销旧 locator 路径，并在预览/入队回执列出全部路径。

## 2026-08-13

### 新增

- **会话索引差分：db 文件 sesno 窗口净增删改秒级判定**
  （`data_interface/session_index_diff` + `aios_db.parse.net_changes` +
  `python/testbed/net_changes_probe.py`）。每个会话页都带当时的索引根
  （copy-on-write B-tree），取窗口两端的根做双根差分：目标树只下降「页号 >
  base 会话末页」的新页、共享子树整枝剪掉，base 树按共享根集合剪——IO 正比于
  变更量，与窗口内会话数解耦。判定**纯文件**：不查库、不逐会话解析记录，窗口
  由调用方显式给定（源码断言钉死零 `SUL_DB`）。存在性口径与生产 B+ 点查逐字
  对齐，三条规则均由真实 ams8000 实测逼出并钉成回归单测：同键子指针首见者胜
  （Save Work 重写子树留下的陈旧指针，跟进会捞出 1.9 万条已被发布抛弃的临时
  记录）、路由不看 flag（flag=0 的首见指针才是发布后的子树）、键范围路由
  （回收页残留条目键在本叶范围之外，点查不可达）。验收：模块纯单测 11 条 +
  `db8000_session_pairs` 性质 h（差分 ≡ 回放折叠，台账腿由性质 e 闭环）+
  Python 离线档 3 条（issue-019 夹具）+ live 对拍 4 窗口差分 ≡ 生产点查零分歧
  （全窗口 695ms vs 回放 10.8s，debug 15–34×）；探针 `--verify` 全量窗口审计
  154 条差异全部点查仲裁归因为回放旧口径盲区（漏报存在 67 / 孤儿腿误报 86 /
  误判 1）。证据 `docs/evidence/2026-08-13-session-index-diff-net-changes.md`，
  live 台账 D 组两条已登记。同日补测 amssys（SYST 8191，169 会话）：10.6×，
  回放折叠净集 **43%（818 条）与生产点查仲裁的两端状态不符**（孤儿 Deleted 腿
  653 为主）；点查是**同源判定基准**（列出/归类分歧，非独立证明全是旧口径盲区），
  删除判据的独立性另由 core.dll 键集差佐证。
- **ADR-022 + specs/003：增量窗口收集改用会话索引差分（净窗口；已接受，P0 已落地、默认 off 灰度中，核心机制层已由 live IDA 闭合，翻默认余下受验收 5 结果层门阻断）**。
  工具层对拍收口后的引擎采纳决策：执行体与预览的收集阶段由
  `collect_net_changes` 接管（逐会话回放退为诊断工具），输出形状兼容——每
  refno 恰一条 `EleOperationData`，净修改由 base/终稿两端版本**一次 diff**
  合成（属性差量 + children 两端 + old/new owner，diff 实现与回放同源单一
  权威）；下游模型计划 / ref_rev / MySQL / 渲染零改动。灰度开关
  `net_window_collection`（默认 off）+ `AIOS_NET_WINDOW`，预览与执行同谓词。
  四条明示行为变化：改了又改回不再 regen、加了又删不留墓碑行、删了又建判净
  修改、逐会话明细退出预览主口径。窗口起点仍由水位给出（ADR-001），回退/
  幽灵水位仍走 ADR-021 整库重建，跨库级联仍走 ref_rev（ADR-003）。
  - 实现落地（同日晚，P0 引擎接线）：`net_window::collect_net_window`（净三态 →
    同形状操作流合成；`diff_ele_data` 忠实复刻 vendor 内联 diff，九桶 + children
    两端 + noun）+ `IncrementPipeline::collect_window` 唯一派发点（预览、执行体、
    崩溃恢复重收集、worker 尾段重收集四处接入，源码断言禁直调回放）；
    `NetEntry::base_loc` 让净修改直读两端版本不付点查；净口径回执首条警告自报
    口径与计数。真实文件逼出第三条口径对齐：字典缺项系统记录（ams8000 全窗口
    64 条）终稿解析失败按回放同口径跳过 + 计数 + 聚合警告，不整批硬失败。
    验收：`db8000_session_pairs` 性质 i（净收集 Modified 负载与回放**逐桶相等**，
    全部案例窗口全绿·样本为各窗口实际 Modified 条目非 test binary 计数）+ live
    负载对拍（6,499 条 Add 渲染逐字符相等，全窗口净收集 1.24s vs
    回放 10.9s）+ lib 710 passed + 离线档 65 passed。已知偏差记 evidence
    「引擎接线」节（`merged_sesnos` 会话页清单口径留待翻默认值前落地）。
  - **live A/B 全链路执行（同日深夜，切默认值前的最后一道证据，已收口）**：
    `python/tests/test_net_window_ab.py`（房间增量档，opt-in
    `$env:AIOS_NET_AB='1'; .venv\Scripts\python.exe -m pytest
    tests/test_net_window_ab.py -q -s`，@8071 一次性内存库）。testbed 8000
    （基线 6,542 行）同一起点、同一窗口 105..=209（净三态 +6/-51/~16，其中原样
    重写 7），off/on 各走一遍完整执行（暂存窗口 + 窗口内生成 + 提交 + 水位收口）：
    **终态逐维等价**——水位 / 共同活行 6,543（逐字段）/ noun 属性表 / pe_owner
    6,542 边 / pending / dbnum_info 记账恒等式全部相等；仅有的偏差全部归因：
    净臂多持 2 个文件真值元素（回放连同旧基线的最终索引漏报，点查仲裁站净一边）、
    13 条 ref_rev 边为回放对 7 个原样重写元素的顺手重建（§5.1 家族，重置后空
    ref_rev 店放大）。窗口全链路耗时回放 35.0s vs 净 11.0s（3.2×），收集阶段
    差分自报 154ms。连续两轮全绿（各 3 分 16 秒）；全量绑定档 83 passed +
    1 skipped（36.4s）。证据同文件「live A/B 全链路执行」节。
  - **M1 正确性闭环（同日，T20 / T11b / T19 / T18a 落地；T13 阻塞未闭）**：
    - **T20 合成器纯单测**：`collect_net_window` 抽出纯合成内层
      `synthesize_net_window(net, resolve)`（`NetChangeSet` 按值接收、resolver 收窄成
      `FnMut(RecordLoc) -> Result<EleData>`、解析上下文错误文案留在合成器），**七条
      纯单测**覆盖三形状 + 基版本失败按新增 + 终稿失败跳过计数聚合 + `base_loc`
      缺失硬失败 + 原样重写计数（原样重写**不是降级**，是正常判定的正常结果）。
      纯提取不伪称先红：安全网是性质 i + 既有 live 对拍，新测试有效性由**逐分支变异
      抽检**证明（5 处准确红，变异代码不入库）。`net_window` lib 13 passed / 0 failed /
      1 ignored（ignored 是需真实 ams8000 的 live，**本轮未跑**）+ `db8000_session_pairs`
      集成目标 20 passed（含性质 i，是用例数不是覆盖窗口数）+ Python 离线
      66 passed / 20 deselected。ADR-022 验收 1 就此满足。
    - **T11b 存量库删除等价直证**：补上「起点早于删除会话、库内确有活行」的形态——
      原 A/B 删除腿是空跑（被删元素在基线本就无行）。切点 K=24、窗口 25..=209，
      文件层净删除 oracle 4 条，起点确为活行且净口径**真立碑 2 条**
      （`24384_24778`/`24384_24779`，⊆ oracle），共同活行 6,536 逐字段一致、
      **0 未归因**，live 118s 全绿；`AIOS_T11B_FORCE_EMPTYRUN=1` 强制空跑变异准确变红。
      存量基线由 `python/tests/_session_snapshot.py`（`session_cut.rs` 的 Python 镜像，
      与 Rust `db_session_fixture inspect` 双向对拍）切 @K 得到；文件换入换出走**同卷
      临时文件 + fsync + `os.replace` 原子替换** + `pristine` 备份 + `finally` SHA 校验，
      收尾源文件 16,504,832 字节无损恢复。**删除判据是纯文件**（core.dll
      `elementsDeletedBetween` 键集差的复刻）；**DB 查询只验证窗口前活行与窗口后墓碑
      两个状态，不作删除判据**，也不用 `search_latest_refno` 点查自证。
    - **T19 qualifier 恢复对拍（非阻断，CLOSED）**：断言落 `db8000_session_pairs.rs`
      性质 i 的 Modified 分支，两臂 `qualified_changes` 逐项相等，集成绿，**未扩公开
      DTO**。强度如实标：当前 issue-019 夹具两案例都是删除、数组属性零变化，这条现在
      是 **empty == empty**，**不是 qualifier 语义已覆盖的证据**，价值只在防回归。
    - **T18a release 方向性单点测量（n=1，非性能门）**：高复触窗 104..=209（106 会话，
      a/d/m = 6/51/16，回放 `ops_total` 215，复触率 2.95）完整净收集 3ms vs 回放 53ms
      ≈ **17.7×**，该窗 raw 两臂发散 72 条全部归因回放旧口径盲区、点查零分歧；对照
      Add 地板窗 1..=209（复触率 1.05）126ms vs 792ms ≈ 6.3×（形态决定，不作判定）。
      **结论仅限**「在动机形状上 ADR-022 决策 4 不需修订」；T18 正式统计
      （1 warmup + ≥5 次 / median·min·p95 / warm 判定 cold 另报）与 **250206 SYST
      现场硬门仍未完成**。另：A/B probe 的 4.4× 已明确降级为「净差分 vs 回放完整收集
      的混层下界参考，非门证据」。
    - **T13 Added 夹具 BLOCKED（不得标完成）**：仓内**不存在**同时满足「Added > 0」
      且「raw 净集 == 回放折叠集」的真实窗口——带 Added 的窗口都伴随回放旧口径盲区，
      raw 两集不等，性质 h/i 指过去必红。须用受控 E3D 录 `scratch-create` 案例
      （新建 SITE/ZONE → 建元素 → Save Work，窗口内无删除无临时态）；**不得**为点亮
      它放宽 h/i 断言。**M1 Exit gate 因此仍未通过，M2（T17/T12/T18/T15）不得启动。**
  - **决策澄清（同日，评审后最小补写，不改决策主体）**：ADR-022 新增「算法来源
    与正确性边界」——会话索引差分**不是** core.dll `DB_DB::elementsChangedBetween`
    的复刻，而是 gen-model 吃 dabacon 追加式 CoW B+ 树形状推出来的加速。证据边界
    同时写死：core31-retrace 证据只显示其**外层语义**是元素 /（属性, qualifier）
    级的三阶段六桶差分、外层未见索引根双根页差分，但
    `attributesChangedBetween` / `elementsDeletedBetween` /
    `elementsInsertedBetween` 的页级实现**未逆向**，故**不断言**内核内部绝不触及
    索引根；core.dll 继续是属性/桶语义的唯一权威，本路径的索引差分不援引它作为
    算法来源。正确性契约写明：端点存在性以生产 B+ 点查可达性为 oracle、净三态
    由两端 leaf `(pgno, offset)` 集合差定义、净修改仍用两版本 `diff_ele_data` 对齐
    core.dll 语义，正确性靠三重对拍而非「复刻内核」。同时记两条机制层未闭合风险
    （叶 `flag` 的取值语义与取值全集未逆向、当前口径本就不依赖 flag；删除是移除
    leaf 还是墓碑 flag；`is_start_page` 只是索引条目起始哨兵行为、底层位定义未知
    ——三者均无 live IDA 证据，现有零分歧只证结果层，且**差分≡生产点查是同源**
    （二者都不看 flag），不能当 flag 机制的独立证明），并把翻
    `net_window_collection` 默认值的门写成验收 5：要么 (a) 在可达 core.dll/idb 上
    闭合机制；要么 (b) **显式接受机制层未闭合的残余风险**并补一份**结果层**样本——
    其独立 oracle 必须是**生产可见终态**（E3D/权威库侧对同一元素在删除/重建后的
    在场与否），而非同源的点查仲裁或带旧口径盲区的回放，样本覆盖已观察 flag 取值/
    删除重建/Added-Deleted-Modified 三态，走 (b) 机制层仍标未闭合。默认 off 的
    诊断与灰度不受此门阻断。
    **⚠ 本条的「机制层未闭合 / 无 live IDA 证据 / 只证结果层」口径已于同日晚被
    live IDA 逆向推翻，以下一条为准。**
  - **机制层闭合（同日晚，live IDA 逆向，推翻上条保守口径）**：
    `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`
    （ida-bridge / idalib，core.dll 3.1，SHA `3c1f…417d`，符号系二进制自带 MSVC
    修饰名、非猜名）证实 core.dll 会话变更枚举（`DB_DB::elementsChanged /
    Deleted / InsertedBetween` → `DB_IndexTableCompare`，dabacon 比较引擎 opcode
    266/270，主索引表 `13387743`）**本就是双根 B+ 索引归并差分**——与 gen-model
    `session_index_diff` **同思想**（gen-model 是纯文件重实现 + 共享子树剪枝，
    非逐指令复刻内核代码）。三处旧「机制未闭合」悬案就此闭合：① 删除 = 键在旧根
    不在新根的**集差、非墓碑 flag**（kind=3）；② 变更检测**全链路**（页取 + begin +
    双根归并）**不读 / 不按 flag 过滤**（flag 在链路外是否另有可见性门未闭合，不写
    功能性否定，report §4.5/C3/C4）；③ `0x80000001` 是**页内键哨兵**（核内以
    `-2147483647` 作键边界识别）。**残留（不阻断翻 on，仅登记）**：`flag` 自身位
    编码（存在 / 偏移 / 位宽 / 枚举）与 **flag 在变更检测链路之外是否另有可见性 /
    过滤门**均未逆向（report C3/C4，有意收口）——可断言的只是「权威变更检测链路
    不以 flag 作门」，不写「flag 全无功能」。据此把 ADR-022 / spec / plan / tasks
    的翻默认门从「(a) 闭合机制 / (b) 接受残余风险」改写为**结果层门**（存量库删除
    A/B、Added 独立夹具、批次冻结快照、会话页清单、SYST 性能——性能门当前**未达**，
    debug 完整收集仅 8.8× / probe 4.4×）。qualifier 维：core.dll 变更粒度含
    `(attr, qualifier)`，gen-model `ModifiedElement` 按属性名聚合会丢 qualifier；
    这是回放与净路径**共享的既有形状限制**、切臂不新增，翻 on 不阻断但**非无条件
    安全**，待评估（tasks T19）。

- **ADR-021 + specs/002：水位必须有数据支撑，回退默认整库重建（去档位）**。
  ADR-001 的「失败不推进水位」管的是写的一侧；读的一侧有两个洞。其一，
  `needs_initial_load` 只问水位不问数据，「水位非零、`pe` 零行」被判成正常
  增量，从 `applied+1` 起重放，`1..applied` 静默缺失（看得见数据的
  `baseline_needs_full_parse` 在 `initialize_dbnum_baseline` 内部，够不着
  路由）。其二，文件回退默认只阻断等人，而阻断会静默消失——`file_latest`
  一旦涨回 `applied` 之上，被替换的那段差异永久丢失。
  - 现场实证：8009 上 dbnum 7350 / 7353 / 7741 的 `applied_sesno` 为
    208 / 101 / 94 而 `pe` 零行；同日 8 个库因文件在 08-12 19:04 被整批换成
    更旧副本而回退阻断。证据 `.scratch/realign-20260813-114321`。
  - 决策要点（评审决议 2026-08-13）：回退**默认整库重建**——扫描只分类入队
    重建批次，worker 冻结点复核仍判回退才 `wipe_dbnum_for_reinit`（整库清空 +
    水位行清值不删行 + 统计与队列残留清空 + spatial epoch 递增），随后按首次
    导入全量解析；`watermark_realign` 档位、`AIOS_WATERMARK_REALIGN`、
    `POST /dbnums/{dbnum}/realign` 端点与 `aios_db.sync.realign` 绑定全部
    移除。幽灵水位（`applied>0` 零数据）由 `needs_initial_load` 的数据支撑
    维度路由到基线（判据落在路由、不落入队门——空库会无限重解析）。
    `TypeChanged` / `Duplicate` / `Missing` / `ForeignProject` 照旧阻断。
  - 实现落地（同日）：`needs_initial_load` 增加数据支撑维度 +
    `dbnum_has_any_pe_row` 存在性探针（只在有增量窗口要跑时付一次，读失败上浮为
    批次 Failed）；`scan_and_check_file` 返回三态 `ScanGate`（放行/阻断/重建），
    sweep 与 watch 对回退构造 `reinit_batch`（applied=0 形状）入队；
    `fast_delete::wipe_dbnum_for_reinit`（与快删同源的三阶段删除，元数据阶段改为
    统计清空 + epoch 递增 + 水位清值不删行且置尾作提交点）；执行体
    `execute_one_dbnum` 冻结点复核仍判回退才清库，清库失败计 Failed 幂等重放；
    预览 `blocked`/`initialization_required` 与执行体同谓词。
    `FileAnomaly::auto_realignable` 更名 `requires_reinit`。拆除面：
    `WatermarkRealign` 档位与环境变量、`realign_rolled_back_dbnum` /
    `realign_dbnum_checked`、HTTP `POST /dbnums/{dbnum}/realign`、
    `aios_db.sync.realign` 绑定、`AiosClient.realign_dbnum`、
    `python/tests/test_watermark_realign.py`（由 `test_rollback_reinit.py` 接棒，
    见下）。
  - Python 闭环用例 `python/tests/test_rollback_reinit.py`（房间增量档，@8071
    一次性内存库）：走与服务同一台机器（`incr.execute_manual` 子集），模块级
    引导一次（SYS meta 解析撑起 MDB 范围 + 7998 首次基线），三条用例分别钉
    回退整库重建（幸存位/幽灵位标记行都必须物理消失）、幽灵水位路由到基线
    （行数回到完整基线，增量重放做不到）、类型变更照旧阻断（水位与数据纹丝
    不动）。conftest 导入期补 `RUST_MIN_STACK=16777216`（执行链在默认线程栈
    上溢出，与 testbed 脚本同一惯例）。全套 `pytest -q` 80 绿（36.5s）。
  - live 首跑抓出并修复一个真缺陷：增量形状（start_sesno>1）的批次先开 ADR-017
    暂存窗口、执行体改道基线后窗口缺 finalize plan 而 failed——`batch_worker`
    开窗前新增冻结点预判 `batch_reroutes_to_initial_load`（applied=0 / 回退 /
    幽灵水位一律不开窗），与执行体共用同一个数据支撑探针。
  - 验证：CI 口径受影响模块单测 155 绿；live
    `live_rollback_wipe_clears_the_dbnum_for_reinit`（4.7s @8019）与
    `live_rollback_and_ghost_watermark_reinit_end_to_end`（22.3s @8019，两幕）
    通过，台账与 `docs/evidence/2026-08-13-adr021-rollback-reinit-live.md` 留痕；
    Python 离线档 62 绿。
  - 「在水位行上记录来源（基线收口 / 增量收口 / info 表播种）」与
    「`applied_sesno_time` 交叉核验（停机窗口内回退又长回去）」记为后续项。

- **增量模型生成单元测试总纲**：重写
  `docs/2026-08-06_model-increment-unit-test-plan.md`，把 S0–S13、U1–U13、
  暂存窗口 I1–I9、房间 RI-1–RI-15、离线夹具与 live / E3D 边界收进同一入口；
  明确 P0/P1/P2 待补项、具体文件落点、“回退即红”条件、Constitution Check
  和分波次门禁。当前源码枚举快照为 765 项（82 ignored），`http_api` 为 776 项
  （82 ignored）；长期状态仍以源码枚举和 live 台账为准，不再复制漂移总数。

### 修复

- **`inst_geo` 几何参数双变体深合并毒化共享单位行（live A/B 抓出的真缺陷）**：
  `render_inst_geo_merge`（2026-08-13 `276aa5f6` 用 `UPSERT … MERGE` 替换
  `INSERT IGNORE`）忽略了「不同 `PdmsGeoParam` 变体可以合法共享同一记录 id」——
  普通 LCylinder 与非切角 SCylinder 的单位网格同为单位圆柱，
  `hash_unit_mesh_params` 按设计同返 `CYLINDER_GEO_HASH`，两个变体先后 MERGE 把
  `param` 深合并成 `{PrimLCylinder, PrimSCylinder}` 双键对象，enum 反序列化永久
  失败，**所有**引用该共享行的根从此生成不出来（A/B run4 实测：2,229 根批量重
  生成全灭 + 逐根重试全灭，`decode mesh parameters failed`）。改为
  `render_inst_geo_upsert`：`param` 整值 `SET` 覆盖——行缺失补齐、半成品修复、
  meshed/aabb/pts 派生字段保留，且对已被旧写法打坏的双键行**自愈**（下次参数
  刷新整值盖掉即恢复可解，2026-08-13 后跑过生成的持久店无需手工修）。回归：
  `a_variant_switch_on_a_shared_unit_row_replaces_param_wholesale`（回退 MERGE
  写法当场红）+ 半成品修复用例改跟新入口 + 源码钉
  `production_inst_geo_writes_replace_param_wholesale`（禁 MERGE 回潮）。受影响
  面：lib 定向 12 条全绿、`db8000_session_pairs` 20/20 全绿、全量绑定档 83
  passed + 1 skipped。
- **`room_model` 无 project 特性构建编译修复（响亮拒绝）**：`configured_match_room_fn` /
  `load_room_panel_map` / `load_room_panel_map_from_pe` / `build_room_panels_relate` /
  `build_room_panels_relate_common` / `load_room_panel_groups` 此前只有 `project_hd` /
  `project_hh` 两条 cfg 分支，两者皆未启用（CI 单测组合 `ws,gen_model,manifold`）时
  `configured_match_room_fn` 无返回值（E0308）、`let sql` 门控外的取用点找不到 `sql`
  （E0425×2），整个 lib 编译不过。按宪法「禁止填近似值」改为**响亮拒绝**：无 project
  特性时各入口 `anyhow::bail!` 明示「需要 project_hd 或 project_hh」，原实现体整体入
  `cfg(any(...))`；`configured_match_room_fn` 同样门控（无 project 时其调用方已全部
  bail，不再被引用）。附回归单测
  `room_subsystem_loaders_loudly_refuse_without_a_project_feature`（仅在无 project 组合
  编译运行，断言两个 loader 返 Err 且提示特性名）。
- **增量流程文档一致性修复（2026-08-13 流程审计定案，纯文档面）**：
  - 宪法 v1.0.0 → **v1.1.0**（`.specify/memory/constitution.md`）：I 条回退语义按 ADR-021
    改写（回退默认整库重建、仅 `TypeChanged`/`Duplicate`/`Missing`/`ForeignProject` 身份
    歧义阻断、补「承诺必须有数据支撑」读侧对偶），附加约束「并发模型」按 ADR-011
    2026-08-09 修订改写（一个派发器 + 至多 8 个在飞批次）；Governance 增修订记录
    （动机 / 受影响 ADR / 迁移路径），Last Amended 2026-08-13。
  - AGENTS.md 水位段与配置段对齐（消除「回退阻断」与「回退默认整库重建」同文矛盾，
    补数据支撑一条），队列门控段的「同一个 worker」补派发器限定。
  - ADR-021 状态「提议中」→「已接受（2026-08-13 评审决议）」。
  - ADR-011 2026-08-09 修订下游同步：`docs/specs/web-service-api.md` §2/§4.3/§6、
    `specs/002-watermark-data-backing/spec.md` Assumptions、ADR-021 引言——「单 worker /
    一个消费者」措辞补「一个派发器、默认 1、可配至 8 在飞」限定，行为描述不变。
  - `specs/002-watermark-data-backing/` 补 `plan.md`（含 Constitution Check：I 条冲突
    处置 = 本次修宪）与 `tasks.md`（按已落地事实事后补记留痕，每条带文件路径）。
  - live 台账（`docs/2026-08-12_live-test-ledger.md`）：合计修正 86→**92**——A 27→28
    （08-13 新增的端到端用例漏计）；新增 E 组补录 tests/ 目录 5 条集成 `#[ignore]`
    待验行（staged_regen / staged_transform / staged_pane_replay / room_rebuild_repair /
    gen_one_root；`db8000_session_pairs` 的命中经复核是文档注释、无真实 ignore 用例），
    口径行同步扩展到 `tests/**`；C 组 issue7_e2e 行加注「旧语义现场，ADR-021 后需按
    新语义重估重跑」。
  - 房间增量测试计划（`docs/plans/2026-08-12-room-incremental-live-test-plan.md`）§7
    增 08-13 重估行：db1112 的 F6 阻断判词被 ADR-021 取代——部署新二进制后首轮重扫
    将排整库重建批次，Phase C 前置需纳入重建时序、代价与重新定标。
  - CONTEXT.md 增词条：数据支撑 / 幽灵水位 / 重建批次（各带 _Avoid_ 清单）。

- **fix(incr)：副作用补偿队列补齐死信可观测 / 人工复活（`/update/side-effects/retry`）/
  done 行清扫三出路，并将 `room_panel_relate` 纳入整库重建的 Ref0 区间清库（补
  `room_relate` 漏删的姐妹边）**。逆向核实确认：`room_panel_relate` 的 id 形态
  `{room_refno}_{panel}` 可按 Ref0 区间寻址，而房间重算只对现存实体先清后写、从不
  整表清空，整库重建后的孤儿边无人回收（ADR-010 D4 幽灵同类）；修复为
  `fast_delete.rs` `RANGE_TABLES` 增表 + 回归测试
  `the_wipe_deletes_room_panel_relate_alongside_room_relate`，ADR-021 §4 口径同步补记。

- **fix(incr)：重扫路径读不出文件最新会话号时不再吞成 0（消除假回退告警与失实的整库
  重建播报）；CATA 定位器读登记表失败上浮、缺 `db_type` 的库计入 missing 并告警；
  MySQL 镜像 NAME 改参数绑定、DBNO 缺失告警；MQTT 同步去重查询插值统一过
  `escape_surql_str`；各附回归测试**。

## 2026-08-12

### 新增

- **db8000 会话对回归进 CI：夹具格式的七类性质断言，数据驱动**（方案
  `docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md` 阶段三 + 四）：
  - 新增 `tests/db8000_session_pairs.rs`（18 passed）：对 `aios-session-fixture-v1`
    夹具里的**每个案例**跑七类断言——档案完整性、窗口切片、时点一致性、并集律、
    净变化折叠、快照差分对账、历史还原。不硬编码任何 sesno 或 refno。
  - **不等真实录制**：夹具来源两条，`AIOS_SESSION_FIXTURE` 指现成目录，缺省则
    从 issue-019 的 final 现场 `pack` 一份。后者是真实 db8000 会话链上的真数据
    （阶段一自检已证明现切台账与当年独立录制逐字节相等），只是案例集小。真实
    录制到货后改指环境变量即可，断言不动。
  - a) 与 g) 直接复用 `pipeline::verify_fixture`——它本就在做那两件事，重写会
    产生两套必须永远一致的口径。
  - f) 快照差分是新增的通用 oracle：`read_raw_records` + `parse_raw_ele_data`
    在 before/after 两份现切快照间逐元素比对（存在性 + noun/name/owner +
    **children** + 属性表），与净结果互证。**children 必须进比对**：实测
    issue-019 的删除序列里，父件与祖父的属性表一个字节都没动，Modified 的信号
    全在 children 列表上；只比属性会把它们误判成「增量说变了但文件没变」。
    噪声属性白名单目前为空——方案预判的 CACHID 类漂移在这条链上没有出现，
    常量留着是机制不是占位。
  - CI：`db8000_session_pairs` 接进同一 job；失败时 upload-artifact 传完整断言
    输出与夹具台账（新增 `AIOS_SESSION_FIXTURE_KEEP` 让合成夹具落在工作区，
    否则临时目录跑完就没了、远程红了无从对账）。
  - job 更名 `db8000-model-increment` → `offline-increment-regression`：它现在
    跑五步离线回归，早已不止 db8000 的模型增量。**配了分支保护必需检查项的话
    需要同步旧名字。**
  - 远程首跑（run 31572427572，2026-08-12 15:03 dispatch）：
    `offline-increment-regression` **首跑即绿，28m58s**——五步（issue-019 夹具、
    通用切割自检、session-pairs 回归、记录边界解析、删除清理 lib 用例）在
    GitHub Actions 上全过。同 run 的 `python-bindings` 在 wheel 冒烟一步红
    （runner 侧 DLL 解析，与离线回归无关），修复走 `4ddf32b9` 的 DLL 探针
    传递闭包，验证 run 另行跟进。

- **db8000 录制清单补齐到 6 类变更形态，并加了一道离线闸**（方案阶段二的离线前置；
  录制本身仍等生产空窗）：
  - 新增 8 个宏、5 个案例：`scratch-create`（added）→ `data-rename` /
    `transform-move` / `geometry-resize`（各 modified，apply+restore）→
    `delete-box`（deleted）。`-CheckOnly` 静态审查 12 条腿 / 7 案例全过。
  - **不动生产元素，改为自建 scratch 元素**：restore 腿必须把值放回原状，而
    生产元素当前的 POS / XLEN 离线不可知，照方案原文写就得先占一次空窗做探针。
    自建元素让每个 before/after 值都自决，整套宏离线可写可评审；末位 delete
    收尾后库在逻辑上回到原样。副产品是 `added` 净形态——原案例表里一个都没有。
  - 新增 `recording_manifest_survives_the_sesno_assignment_it_will_get`：按录制
    脚本的 sesno 分配规则预演清单，`plan_cases` 必须接受，`expected_net.element`
    必须已声明、宏文件必须在场。这类错原本要等占完空窗、录完一整轮才在打包时
    炸，现在 `cargo test` 就拦（防伪已验）。

- **live 用例台账 + 首批点亮**（7-27 测试计划 Gate 3 的首次执行）：
  `docs/2026-08-12_live-test-ledger.md` 给全仓 82 个 `#[ignore]` 用例建档
  （四类目标要求 + 最近通过记录），`scripts/Run-LiveBatch.ps1` 按清单逐项
  独立进程定靶批跑（`DB_OPTION_FILE` 与 `AIOS_LIVE_*` 两套寻址同源派生、
  恰一命中、逐项超时、JSON 报告）。批次 1（自建夹具类 26 项）全部得出结论：
  **23 项首次取得可复现通过**（12 @ testbed 8019；11 项 room_fixture @
  一次性空库 8071 专用清单——房间覆盖率闸门在带真实基线的库上对「只灌夹具」
  的树必拒，空库上夹具行自然对得上分母、闸门语义原样保留），3 项阻塞定性在案
  （积压前置 / 缺陷面板数据依赖 / 断言写死生产 MDB 语义）。顺带修三处测试
  腐化：白名单落地前的夹具命名（first/second 进不了判重）、状态机落地前的
  两个崩溃恢复用例缺测试装载模式声明。另补 IU-S8-05/S12-02 顺序钉（部分失败
  后缓存仍失效、水位不推进）与 IU-S0-05 的 warning 半边——7-27 矩阵点名的
  L0 缺口至此全部关闭或有台账去处。

- **房间增量默认打开（`room_incremental` 缺省值 `false` → `true`）**：`options.rs`
  的 `effective_room_incremental` 缺省翻真，`DbOption.toml` 同步写成 `true`，
  单测由 `room_incremental_is_off_unless_someone_asks_for_it` 改成
  `..._is_on_unless_someone_turns_it_off`，并补一条「认不出的环境变量值退回新
  默认」。
  - 2026-08-10 取假是为了压住现场那 2580 个查不到几何的房间目标（每页 256 个
    付两次全量查询、约 88 秒，把模型侧真正的失败埋进日志）。那批目标已经收干净
    （现场 `/update/pending-units` 的 `room_units` 为空），维持关闭的代价此刻更
    贵：关着时房间归属**只在删除路径**还会被清理（`helper.rs` 的
    `delete_room_membership` 从不看这个开关），搬家后的重算全靠启动全量重建
    回补，而那条兜底路径排在 `startup_autorun` 之后（`skip_startup_room_build`
    的三道门次序）——默认部署两个开关都关着，等于没有回补通道。
  - 门本身一处没动：两个写入点（直写事务的 `room_recalc` 语句、暂存窗口收口的
    `merge_room_recalc_changes`）与一个消费点（`room_round`）照旧读同一个函数，
    翻的只是缺省值。显式写了 `room_incremental = false` 的配置（`python/tests/
    DbOption-ci.toml`、`python/testbed/*`）行为不变；要临时关一次用
    `AIOS_ROOM_INCREMENTAL=0`，不必改文件。

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

- **空间树一致性闭环落地后的绑定侧跟进**（对方工作流 `445e3cd1` 合入后复核，
  绑定重建 + 全套复跑，77 条绿）：
  - conftest 的树产物搬挪清单补上 V2 单文件快照 `accel_tree_{project}.snapshot`
    ——房间档跑在一次性内存库上，但空间树落盘写的是**仓库根**、文件名与真项目
    同款；清单漏项就会拿测试产物顶掉真项目的快照（介质迁移期间已实际残留过一个）。
    并加源码钉：从 `aabb_tree.rs` 反查 `accel_tree_{}` 的全部后缀，与清单比对，
    下次换介质忘了跟进直接红。
  - `spatial.tree_status` 的断言从「稳定核子集」收紧为**逐键全等**。钉的不是
    契约本身（那在 Rust 渲染半边旁），而是绑定透传没掉键——最容易掉的是取值为
    null 的几个（`snapshot_sha256` / `pending`），掉了不报错，只会让照着 /health
    写的脚本在绑定上撞 KeyError。
  - 钉住「Python 夹具路径免标记通过消费者门」：Rust live 夹具要显式调
    `mark_spatial_tree_fixture_preloaded()`，Python 这条路不需要——`full_init`
    走正经装载器，空库上从指针重建出空树即进 ready 态。省得下次门禁收紧时对着
    一堆 `SPATIAL_TREE_NOT_READY` 猜是不是缺了那个标记。

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
## 2026-08-14

### 新增

- Python 解析层新增 `aios_db.parse.net_window(path, start, end, detail=False)`：复用生产
  `net_window::collect_net_window`，只读 dabacon 文件即可得到属性语义上的净增删改，
  并透出 `unchanged_rewrites` / warnings。与原 `parse.net_changes`（索引记录位置触达
  三态）分工明确：E3D TTY 的 apply + restore 合并窗口会过滤已恢复的业务属性，
  同时如实保留 E3D 自增的 `CACHID` 等保存期元数据，不需要反查 SurrealDB。
- 新增 `scripts/e3d/Test-TtyNetWindow.ps1`：自动执行 FTUB apply / 解析器断言 /
  restore / 合并窗口断言，`finally` 保证恢复腿执行，并产出 baseline 副本、语义 diff、
  命令退出状态与 rollback 验证记录。
## 2026-08-14

### 新增

- 定义严格的 SYS/DICT → CATA → DESI → 模型 → 房间初始化阶段与跨项目元件库优先级契约。

### 修复

- 初始化模型生成将受数据就绪门控，避免 DESI 或模型越过尚未完成的元数据、元件库阶段。
