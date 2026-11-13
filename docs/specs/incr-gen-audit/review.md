# 增量更新链路 · 审核结论（Review）

日期：2026-10-27
对象：`gen-model` 模型增量更新链路（工作树现状，含未提交的 `pe_ptset` W2/W3 与 L2 `run_dir` 改动）
产出：`spec.md`（规则全集 ≈ 200 条）、`plan.md`（11 个工作包）、`tasks.md`（≈ 80 条原子任务）、本文件

## 0. 方法与覆盖

- 19 条只读审计线并行：A 双水位/收尾；B 生成根/账本；C 单元级发布/版本；D 切片删/几何指针；E 缓存；F store/配置；G 房间；H MDB/文件异常；I 测试实体；J 规划文档/open issues；K1/K2 core3d 核对表逐条；L1/L2/L3 ADR-060～089 逐决策；M `incr-gen-fixes` 逐条；N 按需/HTTP；O 调度/队列；P 变化分类/窗口。
- 证据只来自 `read`/`grep`（`gen-model/src`、`tests`、`docs`、`DbOption.toml`、`.devin/skills`）。未跑 `cargo test`、未连真库、未读 vendor `e3d-io`/`e3d-model` 源码。「零命中」限 `gen-model/src`（`.scratch/` 备份不计）。
- 行号是 2026-10-27 工作树快照；符号名和 ADR 编号是稳定锚。
- 未向 issue tracker 发布（to-spec 技能的发布步骤按用户「只写文件、不执行命令」跳过）。

## 1. 现状

链路本体**已经是一套自洽的新架构**，与 2026-09～10 的 ADR-060～089 一致，主路径没有发现正确性级的代码缺陷：

1. **双水位 + 进程内账本**：`applied_sesno` 在数据尾事务、`model_sesno` 在 `task_end` 且仅 `all_roots_ok`；`gen_root`/pending 表已退役；三态判决唯一口径；崩溃一律整库 Reinitialize（§A、§B）。
2. **三条发布路**：regen / 刚体 transform / 单元级 unit，都在根回执下；`CommitScope::Partial` 显式删除；闸 `receipt ≤ S`；单元级与刚体默认开（§C、§P）。
3. **落库**：有回执谓词整片删 + `INSERT IGNORE`，legacy 带指针行同键替换、不跟指针；房间边同事务；`pe_ptset` 随根（§D、§G）。
4. **范围与发现**：MDB CURD 闭包、SYST 落地刷新、七种文件异常单入口、冻结锚身份；watcher 只是脏位（§H、§O）。
5. **缓存闸**：overlay 根数（durable-only 逐出）、元素缓存满则清、GenerationInput LRU、历史快照 16 格（§E）。
6. **HTTP**：`/model/generate` 正名、records 按祖先 200 空、lagging 按根回读（#74）（§N）。
7. **测试实体**：登记表 56 条（has 41 / partial 6 / todo 7 / corpus 2 ≈ 73%）；L2 8 条链，`run_dir` + `stage_surreal_scripts` 已改并有源码钉（§I）。

**文档面严重滙后于代码**：规格、ADR 正文、源码注释三层都有与现行行为相反的句子。这是本次审核最大的结论。

## 2. 缺口

### 2.1 规格断层（最高优先）

- `incr-gen-fixes/*`（2026-07-24）整体停在旧管线；F1–F9 里没有一条的根因叙述还对应现行写路（§4 逐条）。它不覆盖：双水位、账本、单元级发布、切片删、房间边、缓存、配置面、HTTP 面。
- `core3d-partial-update-conformance.md` 多条 ✅ 项引用已删符号：R6 `discard_pending_for_full_rebuild` / `sync_and_seed_model_coverage` / `stale_queue_is_discarded_before_the_rebuild_seeds_its_own_work`；R22 `persist_root_in_chunks` / `projected_geometry_ids_on`；R6 矩阵 `clear_stale_at_process_start`；§10「从 `gen_root` 回灌」；★0907 电缆默认关（代码默认开）；R2 正文「预览与 `build_model_update_plan` 共用筛门」（DESI 生产只观察）；R1 `admits` 形状不同；R8 类比对象错；C3-1 测试名与 `tests/staged_regen_e2e.rs` 不存在。
- `web-service-api.md`：§4.6 pending-units 仍是活路由（代码已卸）；`pending_model_retries`/`model_drain`/`room_recalc` 仍在；§4.12 缺 `WatermarksInconsistent` / `ModelWatermarkUnavailable`；§4.5 超时自相矛盾（504 vs「202 契约」）、容器恒 422 不对；`held`/`batch_failures` 未写；版本/历史六条路由无专节；`rooms_after` 拍点写「收尾后」。
- `CONTEXT.md`：「待重试单元」仍在术语表正文，「切片删」无词条。

### 2.2 ADR 正文过时（文内未加横幅）

| ADR | 过时语句 | 现行 |
|---|---|---|
| 两份 ADR-060 | 撞号 | 需约定 `ADR-060(行id)` / `ADR-060(水位)` |
| ADR-060(行id) D6/D8/D11 | `pe->tubi_relate` 图遍历；`gen_root` 凭证行/栅栏 | `WHERE in = pe` 先捞指针；`gen_root` 退役 |
| ADR-060(水位) 状态行、D3/D5/D6/D7/D10 | `model_update_pending.rs`、`gen_root`、`ModelWatermarkProbe`、`startup_coverage_floors`、`legacy_adopted` 判定、`room_sesno` | 全部退役（ADR-073/079） |
| ADR-065 D2/D7 | `gen_root` 探针推进模型水位 | `task_end` 盖、账本普查 |
| ADR-071 D2/D9/状态行 | `gen_root` ON DUP KEY；`increment_update_attempt` 留用；S7–S8 未开工 | 表已 REMOVE；已落地 |
| ADR-072 D2/D3 | 按 sesno 分面回退 | ADR-073 D3 整库 Reinitialize；`lib.rs:1043-1048` 注释同样过时 |
| ADR-073 09-11 段、D7、文头 | `room_membership=false` 开关；`legacy_adopted` 不退；「作废 ADR-067 全篇」 | 开关 10-23 退役；079 退判定；常驻模型仍生产 |
| ADR-076 D5 | 三枚水位收尾同笔 | `applied` 数据尾、`model` 收尾（ADR-071 D4） |
| ADR-077 D5、背景 | `UPSERT gen_root`；`enqueue_model_watermark_write_behind` | 退役；该名仅作禁词 |
| ADR-078 D5 | priority 校验对象 | #47 追记改口；`included_projects_notice` 注释写 D3 |
| ADR-079 D6 | 回执半边等值判 | ADR-089 `NotBehind` |
| ADR-080 状态行、D4、D9、D10、10-01 追记 | 草案；「不在 S → 回退」；默认关；三条都关 | 已接线；`≤ S`；默认开；C8+P4 开 C2 关 |
| ADR-081 D5 | 不做 diff 接口 | 追记二 `node/diff-summary` |
| ADR-082 实施钉 3/7、D4 | 配置写 0；测试名 `_defaults_to_unbounded`；全池 vs 每池 | 50000/200000；`_then_to_the_default`；每池 |
| ADR-083 标题、D3/D5/D6 | env-only；回填 `mem` | 三层；回填 `rocksdb`（脚本已改，`options.rs:1296` 注释未改） |
| ADR-084 钉 | `factory_template_room_membership_matches_code_default` | 改名 |
| ADR-066 D1、追记 | `transform` 并列去向；「默认仍关」 | `transform ⊆ regen` 提示；默认开 |
| ADR-063 追记 | P4 默认关 | 默认开；CATA 执行体仍读 `ref_rev`（D4/P5 未做） |
| ADR-048 D3 | `skip_startup_room_build` | 函数不存在 |
| ADR-008 | pending | 被 063/065/079 修订 |
| ADR-087 | 无文件，号位被拟稿占用 | — |

### 2.3 源码注释/默认值漂移

`options.rs:2348`「默认 false」vs `:2369` 默认开；`e3d_model_service.rs:2750` / `model_refresh.rs:131` / `data_interface/mod.rs:219`「默认关」；`options.rs:261` 电缆「默认关闭」；`e3d_model_service.rs:3489`「元素缓存默认 0」；`options.rs:2512`「不淘汰」；`options.rs:236-238` 元数据五名单；`options.rs:1294-1296,3171` 回填 `mem`；`options.rs:1199-1201,1549-1551` env-only；`options.rs:1818-1819` auto_initialize「等手动」；`options.rs:50-52` HTTP `127.0.0.1:9099`；`dbnum_state.rs:4-6` applied ONLY；`write_state.rs:36,102` 三水位；`model_db_adapter.rs:280-283,662,911` 走对账/`persist_root_chunks_impl`；`model_memory_store.rs:152-153` `gen_root.published_via`；`model_root_ledger.rs:18-19`「不参与放行」；`handlers.rs:1174-1175` pending 行；`cata_closure_cache.rs:14` S4 接 options；`mdb_route_directory.rs:166-167` `included_projects`；`update_scope.rs:37-38` STYP=DESI；`model_db_adapter.rs:729,964` 房间开关；`helper.rs:190-192,252,358` replace_exist/房间队列；`async_watch` `:4975` drain；`batch_scheduler.rs:378-380` `skip_startup_room_build`；`e3d_model_service.rs:3379` 长度+mtime。

### 2.4 功能/行为缺口

- **调度**：发现轮被任一 running 行整轮冻结（`increment_manager.rs:4770-4778`）；park 的 `PhaseBlocker` 可整相连坐（`initialization_phase.rs:473-486`）；跨库并行被 D10-A 独占关掉。「两库互不阻断」目前只是字面。
- **模型失败无 park**（#72）：数据失败一次即停，模型失败只记账；未拍板、零代码。
- **D10 在默认配置下不生效**：C2 关 → 整窗不分桶。
- **CATA 正确性仍绑 `ref_rev`**（ADR-063 D4/P5 未做）。
- **DESI 窗口 XGEOMETRY 只观察**：无书面口径是门还是观察。
- **初始化 × legacy 带指针行未验**（ADR-088 D7 自承）；`RestartInitializationData` 跳过模型面区间删。
- **失败条目 `source_end_sesno` 恒 0**：诊断四格「够新」不可判。
- **测试钩 release 生效**：`AIOS_TEST_FAIL_*` 无 `cfg(test)`。
- **ADR-084 D3 默认值未定**：GenerationInput 默认不限。
- **read-through mirror spool 死代码**，`/health.mirror` 仍报 ready。
- **测试门**：INC-C08 录完未跑；G03/H03/C05/C06 无夹具；F03 未对拍；I 组未进 nightly；K 组未登记；#73/#74/#76 代码在但 changelog 自陈未编译未测；计划 §4/§5.2 与登记表三方漂移（mem vs rocksdb、90s vs 600s）。
- **`pe_ptset`**（本轮新增）：无 ADR、无 L2 断言。

## 3. 风险（按优先级）

**P0（影响正确性或生产可用性）**
1. park 连坐 + 发现轮冻结：一库数据失败可让其它库停一个对账周期或更久（T-O-1 / T-O-2）。
2. 测试钩在 release 生效：误设环境变量 = 现场停跑（T-O-5）。
3. 按旧规格施工的风险：`incr-gen-fixes` 与 conformance R6/R22 仍被当作活规格，会修错层（WP-1）。

**P1（正确性边缘 / 诊断误判）**
4. 初始化撞 legacy 带指针行未验（T-D-6）；嵌套根整片删更脆（T-D-7）；`pdms_inst` 双写口（T-D-8）。
5. 同进程 Partial 且 `applied==file_latest`：lagging 可能无人追（T-A-7）；数据尾已提交、收尾未落就崩 → 整库 wipe（有意，需写进规格）。
6. 诊断技能把退役开关当原因、四格三套命名、`source_end_sesno=0`（T-G-1 / T-B-6 / T-B-3）。
7. `FileReplaced`（同 sesno 换文件）摄入面看不到（T-H-3）。
8. 本机 `watch_dbnums = [7997, 7998, 8000]` 非正常运行态（T-F-9）。

**P2（资源 / 可维护性）**
9. 未 ACK / Blocked 根钉死 overlay；GenerationInput 默认不限；无界附属结构（`LOCATOR_CACHE`、`CATALOGS`）。
10. ADR 互相覆盖未改旧文，改水位写点极易抄错（WP-2）。
11. HTTP 超时双标准（110s 409 / 120s 504）、`excluded` 与 watch 门混淆。

## 4. 与 `incr-gen-fixes` 的对照结论

| 项 | 规格说法 | 现行 | 判定 | 处置 |
|---|---|---|---|---|
| F1 孤儿 `inst_relate` | `replace_exist` 只删本次键 | `DeleteCleanup` + 切片删（ADR-088）+ `Partial` removals（ADR-080）；`replace_exist`/`save_instance_data` 在 `pdms_inst.rs` 零命中 | 已修，根因叙述被架构替换 | 改写（T-D-1） |
| F2 mesh panic | `.expect` 炸看门狗；失败进 pending drain | `?` 已落地；失败进账本 `Failed`，不排队；`live_generation_failure_keeps_pending_and_watermark` 零命中 | 已修；验收绑 pending 已失效；live 故障注入测试丢失 | 改写 + 补测（T-A-4） |
| F3 生成根归一 | 主/兜底/补偿三套 | `generation_root.rs` 单一权威；`compensate_owners`/`resolve_significant_owner`/`conservative_regen` 零命中 | 已被替换，分叉消失 | 删叙事，改守门（T-B-1） |
| F4 `pe_owner` 幂等 | pdms-io `to_surql` | `ele_ops.rs:1720` / `element_batch.rs:15-21`；测试仍在 | 已修，写口换了 | 改路径 |
| F5 SurrealQL 转义 | `update_datacenter_version` 插值 | `escape_surql_str` 在 `ele_ops`/`element_batch`；函数名零命中，现 `resolve_datacenter_statements_with` | 已修 | 改函数名 |
| F6 watcher 未接 `record_scan` | 未接入；Rollback 阻断；改名自动更新路径 | `scan_and_check_file` + `classify_scan`；Rollback = `ScanGate::Reinit`；`NameChanged` 阻断不更新 | 已修，条文与现码不一致 | 改写（T-H-2 / T-O-3） |
| F7 datacenter Add | 待确认 | 结论仍对（未发布 UPDATE 空操作） | 无需实现 | 并入附录 |
| F8 共享 CATA 反向 | `ref_rev` + pending | `CascadeExpand` + 文件引用表（ADR-063）；执行体仍读 `ref_rev`；源 session/UI 触发仍缺 | **部分；真缺口** | T-P-5 + 留票 |
| F9 50 条截断 | watcher 先消费 pending | 任务内 `run_model_side_actions`；drain 退役 | 架构替换 | 删（T-O-3） |
| D1 `dbg!/println!` | 绑 `compensate_owners` | 函数已删；`println!` 仍多 | 部分 | 改为日志卫生项 |
| D2 子树 10 层 | — | `collect_pe_subtree_refnos` BFS | 已修 | 删 |
| D3 `channel(1)` | 死锁 | `try_send` 脏位 + `WatchSweepGate` | 已改写 | 改描述（T-O-3） |
| D4 `raw_dchc_code` | 仅 REDRAW/INTUBE | 4270 条字典快照 | 已修 | 改写（T-P-2） |

**结论**：`incr-gen-fixes` 应**冻结为历史**（加横幅指向 `incr-gen-audit`），不再作为施工对照；其中仍有效的只有全局约束「`applied_sesno` 不回滚、`model_sesno` 不虚进」、F4/F5 语句级单测、F6 三条文件异常验收、F1 live 软删子树测试、F8 源触发真缺口——这些已迁入 `spec.md` §A/§D/§H/§P 与 `tasks.md`。

## 5. ADR 建议

**新 ADR**（已落地或待拍板，无文）
- 数据批次 park（`DATA_BATCH_PARK_AFTER=1`）与模型侧对称性（#72 拍板结果）。
- 生成器指纹整库重排（#73，`generator_identity.rs`）。
- `pe_ptset` 随发布事务落地、无 backfill、旧库重新生成。
- 发现轮与 running 行的关系、park blocker 作用域（T-O-1/T-O-2 的结果）。

**追记**
- ADR-089 或 ADR-021/028：`FileAnomaly::NameChanged` 与 B07 撤回。
- ADR-080：状态行 Accepted；D4 `≤ S`；D9 默认开与 10-14 门；D10 理由改「C2 关」；#76 种子门。
- ADR-066：D1 `transform ⊆ regen`；刚体 P4 前置；删「默认仍关」。
- ADR-073 / ADR-010：#71 `rooms_after` 拍点；10-23 开关退役；D7 `legacy_adopted`；文头取代表。
- ADR-060(行id)：D6 管身重置现形；N2 mesh-purge 例外；D8/D11 退役。
- ADR-060(水位)、ADR-065、ADR-071、ADR-072、ADR-076、ADR-077：`gen_root`/pending/三水位/分面回退横幅。
- ADR-079 D6 回写 089 `NotBehind`；ADR-081 D5 标题注；ADR-082 实施钉与 D4；ADR-083 标题/D3/D5/D6；ADR-084 钉名与 D3 默认值；ADR-063 P4 默认；ADR-048 D3；ADR-078 D5 命名；ADR-008。
- 两份 ADR-060 的引用约定；ADR-087 补文或改号。

## 6. 下一步（有序）

1. **立即、无需拍板**：批次 0 全部文档/注释对齐（WP-1/WP-10 + WP-2 的横幅部分），以及 `incr-gen-fixes` 冻结。这是其它一切的前提。
2. **P0 代码**：T-O-1 park 不连坐、T-O-2 running 不冻结发现、T-O-5 测试钩保护；配 H03 两库夹具。
3. **四项拍板**（用户决策）：#72 模型失败是否 park；C2 是否翻开以激活 D10；DESI 窗口 XGEOMETRY 门 vs 观察；`FileReplaced` 是否升格数据面重建。拍板后进 WP-2 落 ADR。
4. **P1 验证**：T-D-6 初始化 × legacy 夹具、T-D-7 嵌套根钉、T-D-8 双写口盘点、T-A-7 idle ModelOnly 核实、T-B-3 `source_end_sesno`。
5. **测试门**：T-I-1 rotate-chain 跑绿；T-J-2 #73/#74/#76 单测补跑；T-I-3 G03/H03 夹具；T-J-1 登记表单一事实；T-D-9 `pe_ptset` L2 断言；目标 has ≥ 80%。
6. **交付留痕**：T-J-9 tag（需授权）。

## 7. 没能确认的

- vendor `e3d-io` `DbSet::store_parsed` 满则清的现码；e3d-model `plan_update`/`collect_window`/`ElementDiff::is_placement_only` 实现。
- `increment_manager` 空闲轮是否为纯模型缺口排 `ModelOnly`（T-A-7）。
- `pdms_inst`/`shape_save` 在默认增量 DESI 窗口是否仍写实例行（T-D-8）。
- `RootPublicationAck` 生产写入点（只读到测试夹具与 struct）。
- `record_generated` 是否所有 `PersistTarget` 都调 `record_root_generated`。
- `model_lazy_roots` 在普通增量收尾是否仍写入。
- `dchc_change_classes.json` 是否恰 4270 键；1272 名单与 `noun_dict` 是否逐名相等。
- GitHub 票面当前 open/closed（只用文档与 changelog）；changelog 10-27 节自陈「未编译未测」之后有无补跑。
- rotate-chain / 带 `budget_gate` 三链在当前二进制上是否绿；python 是否在远程 PATH。
- `.planning/2026-10-15-room-startup-alignment-check/` 除 `gate_result.json` 外无 md（只能从 `room_recompute.rs` 与 verification 反推）。
- `Start-AMS.ps1` 现行回填字面值（只读了 `New-LocalRelease.ps1:591` / `start-local-release.ps1:36` 的 grep 命中与 ADR-083 修订）。
- `AIOS_SKIP_STARTUP_ROOM_BUILD` 是否仍被 L3 夹具设置。
- `handlers.rs` `model_records` 在 `source =` 之后如何标注 #74 冷读。
