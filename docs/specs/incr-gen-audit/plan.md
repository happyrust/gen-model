# 增量更新链路 · 当前开发计划（Plan）

状态：审核稿（2026-10-27）
对应：`docs/specs/incr-gen-audit/spec.md`（规则全集）、`tasks.md`（原子任务）、`review.md`（结论）

> 本文件回答 **HOW 与顺序**：哪些工作包、互相依赖、每包的 ADR 支撑状态、风险与验收门。不给日历时间；里程碑按「门」划分。工作包里的 `T-*` 是 `tasks.md` 的任务 ID。

## 1. 现状基线（已落地、可以当作施工前提）

| 层 | 现行形态 | 支撑 ADR | 源码锚 |
|---|---|---|---|
| 发现 | watcher 脏位 + `sweep_dirs` + 300s 对账；read-through 另有 2s `poll_generations` | ADR-011 | `increment_manager.rs`、`lib.rs` |
| 队列/调度 | `BatchIntent::{ApplyWindow, ModelOnly, Reinitialize}`；同库串行、`BehindRunning`、`held`；数据批次独占（D10-A）；park 一次即停 | ADR-011/021/048/056 | `batch_queue.rs`、`batch_scheduler.rs`、`batch_worker.rs` |
| 范围 | MDB CURD 闭包、`UpdateScope` 读登记表、SYST 落地刷新 | ADR-078/085 | `mdb_closure.rs`、`mdb_membership.rs`、`update_scope.rs` |
| 文件异常 | `FileAnomaly` 七种 + `GenerationChange` 三种；冻结锚身份 | ADR-021/089 | `dbnum_state.rs`、`db_generation.rs`、`frozen_source.rs` |
| 窗口规划 | `plan_update(S→T)` + 四去向 + 三级联 + #76 种子门 + D10 分桶 | ADR-063/066/079/080 D10 | `window_root_plan.rs`、`model_impact.rs`、`model_update_plan.rs` |
| 选根 | `generation_root.rs` 单一权威；MDU + significant 位表 | ADR-079 | `generation_root.rs` |
| 发布 | 三条路 regen / transform / unit，根回执 + `CommitScope`；闸 `receipt ≤ S` | ADR-066/080/089 | `e3d_model_service.rs`、`rigid_motion.rs`、`model_memory_store.rs` |
| 落库 | 切片删 + `INSERT IGNORE`；房间边同事务；`pe_ptset` 随根 | ADR-060/073/088 | `model_db_adapter.rs`、`model_initial_persist.rs`、`ptset_rows.rs` |
| 收尾 | `task_end` 一笔：`finished` + `model_sesno`（仅 `all_roots_ok`）+ 目录落款 + 房间代号 | ADR-071/076/077/079 | `task_end.rs` |
| 账本/判决 | `model_root_ledger` 三态；`dbnum_consistency::verdict` 三态 | ADR-079 | `model_root_ledger.rs`、`dbnum_consistency.rs` |
| 缓存 | overlay 根数闸（durable-only）、元素缓存满则清、GenerationInput LRU、闭包缓存、历史快照 16 格 | ADR-082/084 | `model_memory_store.rs`、`model_write_behind.rs`、`cata_closure_cache.rs` |
| HTTP | `/model/generate`（别名 ensure）、`/model/records` 按祖先、`/update/preview|execute`、`/tasks/*`、`/health` | ADR-086/089/071 D8 | `web_service/mod.rs`、`handlers.rs`、`on_demand_model.rs`、`model_read_route.rs` |
| 配置 | `store_mode` 三层、默认 `rocksdb`；`data_face` 派生；CWD 单实例锁 + 写者锁 | ADR-074/083 | `options.rs`、`lib.rs` |
| 测试 | 登记表 56 条（has 41）；L2 8 条链；`run_dir` + `stage_surreal_scripts` | — | `tests/increment_replay/`、`tests/fixtures/increment_cases.json` |

**已退役、不得再作为施工对象**：`model_update_pending` 表与 `drain`、`gen_root` / `root_dependency`、`room_sesno`、`model_drain` / `room_recalc` 任务种类、`included_projects` 名单、`legacy_adopted` 判定、`SCOPE_CACHE`、`ModelWatermarkProbe` / `advance_model_watermark_on` / `startup_coverage_floors`、`replace_exist` / `save_instance_data` 作为增量写路、`compensate_owners` / `resolve_significant_owner` / `conservative_regen`、`skip_startup_room_build`、`GET/POST /update/pending-units`。

## 2. 工作包

### WP-1 规格层对齐（文档，不改行为）

- 目标：让仓内三份规格不再把退役路径当现行。
- 内容：
  - `incr-gen-fixes/{spec,plan,tasks}.md`：加「历史冻结」横幅，指向本规格；F1–F9/D1–D4 逐条按 `review.md` §4 改口或删除（T-M-1、T-B-1、T-D-1、T-H-2、T-O-3、T-A-4、T-P-2、T-C-2、T-E-1、T-F-8、T-G-3）。
  - `core3d-partial-update-conformance.md`：R6 改挂 `model_rebuild.rs` 排 `ModelOnly`；R22 改成谓词整片删 + 投影重插；R12 分开解析/枚举；删「从 `gen_root` 回灌」、`clear_stale_at_process_start`；★0907 电缆默认改开；R2 正文写清 PE 可筛 / DESI 只观察（T-C-4、T-D-2、T-B-5、T-P-1、T-P-7）。
  - `web-service-api.md`：卸 §4.6 pending-units、删 `pending_model_retries`/`model_drain`、§4.12 补两档 `MemoryReason`、§4.5 超时与 container 422 改口、补 `held`/`batch_failures`、补版本/历史专节、`rooms_after` 拍点（T-N-1、T-N-3、T-N-5、T-O-4、T-G-2、T-H-1、T-H-5）。
  - `CONTEXT.md`：术语表删「待重试单元」现行用法，补「切片删」「双水位」词条（T-M-2）。
- 依赖：无。可与所有 WP 并行。
- ADR 支撑：已有（引用现行 ADR 即可）。
- 验收：`rg -n "model_update_pending|compensate_owners|replace_exist|discard_pending_for_full_rebuild|persist_root_in_chunks|pending-units|model_drain" gen-model/docs/specs` 只剩标为「历史」的命中。

### WP-2 ADR 追记包（文档）

- 目标：旧 ADR 正文不再与代码相反；已落地但无 ADR 的决策补文。
- 内容：
  - 作废/修订横幅：ADR-072 D2/D3（被 073 D3 取代）；ADR-076 D5（两写点）；ADR-071 D2（`gen_root` ON DUP）/ D9（`increment_update_attempt` 已 REMOVE）/ 状态行 S7–S8；ADR-065 D2/D7（`gen_root` 探针）；ADR-060(水位) D3/D5/D6/D7/D10 与状态行 `model_update_pending.rs`；ADR-060(行id) D6/D8/D11 + N2 清理例外；ADR-073 D7 `legacy_adopted` 与 09-11 开关段、文头「作废 ADR-067」；ADR-077 D5 `gen_root` UPSERT；ADR-079 D6 回写 089 问法；ADR-080 状态行「草案」→ Accepted、D4 `≤ S`、D9 默认开、D10「三条都关」理由；ADR-083 标题/D3/D5/D6 回填 `rocksdb`；ADR-082 实施钉「配置写 0」、D4 全池 vs 每池；ADR-084 钉名；ADR-066 D1 `transform ⊆ regen` + 删「默认仍关」；ADR-063 追记 P4 默认开；ADR-048 D3 / ADR-078 D5 命名；ADR-008 正文 pending（T-A-1、T-A-3、T-C-1、T-C-5、T-D-3、T-D-4、T-F-1、T-F-2、T-P-3、T-O-7、T-L-1）。
  - 新 ADR / 追记：`NameChanged` 与 B07 撤回（挂 ADR-089 或 ADR-021/028）；#76 元数据种子（挂 ADR-080 D10 邻近）；刚体 P4 前置（ADR-066）；`DATA_BATCH_PARK_AFTER` 与 #72 对称性（新 ADR）；#73 生成器指纹整库重排（新 ADR）；#71 `rooms_after` 拍点（ADR-010/073 追记）；mesh-purge 不删管身键（ADR-060 N2 例外）；`pe_ptset` 随发布事务、无 backfill（新 ADR）；两份 ADR-060 撞号的区分方式（T-J-8、T-L-2）。
- 依赖：WP-4 的 C2/D10、WP-3 的 #72 拍板结果要回写进 ADR。
- ADR 支撑：本包就是补 ADR。
- 验收：`rg -n "NameChanged|metadata_only_change_seeds|generator_fingerprint|DATA_BATCH_PARK_AFTER|pe_ptset" gen-model/docs/adr` 非空；`rg -n "三枚水位|默认关|回填 \`mem\`" gen-model/docs/adr` 只在已标「已修订」的段落。

### WP-3 调度正确性（代码，P0）

- 目标：「两库互不阻断」从字面变成行为。
- 内容：
  - park 的 `PhaseBlocker` 不得把整相 `allows()` 打成 false（T-O-1）。
  - 有 `running` 行时其它 dbnum 仍能入队（epoch 收养与 running 闸拆开）（T-O-2）。
  - `AIOS_TEST_FAIL_*` 生产保护（T-O-5）。
  - GitHub #72 拍板：模型侧失败是否 park/退避/只记账（T-O-6）。
  - 核实稳态进程内 `applied==file_latest && model<applied` 是否有 idle 排 `ModelOnly`；没有则写入规格或补 sweep（T-A-7）。
- 依赖：无。测试靠 WP-9 的 H03 两库夹具。
- ADR 支撑：ADR-011 D7 有语义；#72 无 ADR（WP-2 补）。
- 风险：改发现轮入队条件可能打破「运行中任务的 epoch 收养」；需先读完 `enqueue_discovered` 与 `running_queue_row` 整段。
- 验收：一库 `DATA_BATCH_PARK_AFTER` 后另一库 `ApplyWindow` 仍能 `next_dispatch`；A running 时 B 的 sweep 产生 New/Merged。

### WP-4 窗口/级联决策（代码 + 拍板）

- 内容：
  - C2 `model_structural_connection_cascade` 是否翻开以激活 ADR-080 D10（当前 C8+P4 开、C2 关 → D10 整窗不分桶，纯戳窗白算）（T-P-4）。
  - CATA `CascadeExpand` 执行体仍读 `ref_rev`（ADR-063 D4/P5 未做）→ 改文件表或显式降级（T-P-5）。
  - XGEOM 在 DESI `from_window` 只观察：升门或写 ADR（T-N-4 / T-P-7）。
  - `BFORI` 不进便宜路：维持，文档写清（并入 T-P-1）。
  - DCHC 字典键数钉 4270（T-P-8）。
- 依赖：C2 翻开需 ADR-080 D9 ③ 类的语料门；与 WP-9 的 L3 场景联动。
- ADR 支撑：ADR-080 D10、ADR-063 有；结论需回写（WP-2）。
- 验收：`d10_refuses_to_classify_unless_all_three_cascades_are_on` 两态仍绿；CATA 执行体不再把空 `ref_rev` 当无引用者。

### WP-5 落库与删改（代码/测试）

- 内容：
  - 初始化 `WriteMode::Initial` 撞 legacy 带指针行的夹具（ADR-088 D7 自承未验）（T-D-6）。
  - 祖先投影不含嵌套记录时整片删清掉嵌套根的源码钉（T-D-7）。
  - `pdms_inst`/`shape_save` 是否仍在增量窗口写 `inst_relate`（双写口）盘点（T-D-8）。
  - `persist_root` 等过期注释（T-D-5）。
  - mesh-purge W0-1 `inst_geo` 缺行容错、V5–V7 留痕（T-J-10）。
  - `pe_ptset`：W2/W3 已在工作树，补 ADR（WP-2）与 L2 断言（按根 `pe_ptset` 行数与 `fn::bran_comp_len` 非零）（T-D-9）。
- 依赖：无。
- ADR 支撑：ADR-088 有；`pe_ptset` 缺。
- 验收：夹具要么红（证实 D7 风险）要么文档写明「初始化前必须 wipe 模型面」。

### WP-6 账本与诊断（代码，小）

- 内容：失败记账写真实 `source_end_sesno`（现恒 0）（T-B-3）；四格三套命名对照表（T-B-6）；`RootReceipt.published_via` 注释补 `"unit"`、去 `gen_root`（T-B-2 / T-C-3）；`/health` `parse_errors` 旁「pending 行」注释（T-B-4）；账本模块头「不参与放行」改口（T-N-6）；诊断技能改「能力门 + 退役键点名」（T-G-1）；可选「解析根 ⊆ 枚举根」源码钉（T-B-7）。
- 依赖：无。
- 验收：`rg -n "record_failed\(dbnum, root, noun, 0" gen-model/src` 空；skill 四格「够新」可判定。

### WP-7 缓存闸收口（代码，小）

- 内容：GenerationInput 逐出日志只计真 LRU（T-E-3）；ADR-084 D3 默认值量完再定（T-E-4）；`WindowPlanCache::CAPACITY` 观测/配置（T-E-5）；过期注释（元素缓存「默认 0」「不淘汰」、闭包缓存头）（T-E-2）；闭包缓存淘汰 log（T-E-7）；overlay 驱逐 × 增量路径联测（T-E-6）。
- 依赖：T-E-4 需真机两库交替窗。
- ADR 支撑：ADR-082/084 有；D3 未决。

### WP-8 HTTP/API 契约（代码 + 文档）

- 内容：统一 generate 超时契约（110s 飞行 409 vs 120s HTTP 504）（T-N-2）；container 422 口径（T-N-3）；`/dbnums` `excluded` 与 `watch_dbnums` 分列（T-H-5）；read-through mirror spool 死代码删或接通（T-F-7）；异常词表统一（T-H-1）；poll `FileReplaced` 是否升格数据面重建（T-H-3）。
- 依赖：WP-1 的 `web-service-api.md` 改动。
- ADR 支撑：ADR-086/089 有；超时契约无 ADR（可只落 spec）。

### WP-9 测试套件补齐（测试）

- 内容：
  - 跑绿 `ams8000-equi-rotate-chain`，INC-C08 翻 has（T-I-1 / T-J-3）。
  - 录 G03（PANE `room_rebuild`）、H03（两库）、C05（PIPE）、C06（NOZZ）（T-I-3 / T-J-4）。
  - F03 变化清单与宏逐元素对拍（T-I-6）。
  - I 组预算门刷基线、I03/I06 写门、nightly 脚本（T-I-5 / T-J-6）。
  - 计划 §4/§5.2 与登记表对齐（rocksdb、超时），B07 改口（T-I-2 / T-J-1）。
  - K 组登记 + `web:` 前缀协议（T-J-7）。
  - #73/#74/#76 已写代码的验证门补跑（T-J-2）。
  - L3 场景 x1/l1/e1/p1/n1/u1 收编（T-J-6）。
  - 水位/恢复钉（T-A-5 / T-A-6）、快照时序钉（T-C-6）、退役键钉（T-G-5）、MDB 纯测回归（T-H-6）。
  - Core3D C0–C1 不塞 replay；C2-1/C2-2/C2-5 落 `model_update_plan.rs`（T-I-4）。
- 依赖：H03 与 WP-3；G03 与 WP-5 房间边；部分需远程 Windows 真机（`AIOS_REPLAY_PROJECT_ROOT`、`bin/surreal.exe`、python）。
- 验收：登记表 has 占比 ≥ 80%（现 73%）；八条 L2 全绿；`cargo test --test increment_cases_registry` 绿。

### WP-10 配置面卫生（代码注释/文档，小）

- 内容：ADR-083 env-only 残留文案（T-F-1）；`Start-AMS.ps1` 回填注释（T-F-2）；`memory`/`kv-mem` 点名（T-F-3）；`auto_initialize`/`startup_autorun` 缺省叙述（T-F-4）；HTTP 默认与「注释则不启动」（T-F-5）；`AIOS_SKIP_STARTUP_ROOM_BUILD` 退役或实现（T-F-6 / T-O-7）；本机 `watch_dbnums` 验收纪律（T-F-9）；单元级发布/电缆/元数据门注释（T-C-1 / T-P-1 / T-P-6）；`included_projects` 符号名残留（T-H-4）；`lib.rs` 启动段「退回 S」注释（T-A-2）。
- 依赖：无。

### WP-11 交付留痕

- 内容：打 `v0.1.43`/`v0.1.44` tag、入库 verification、关 #62/#63 交付项（T-J-9）。
- 依赖：用户授权（git 操作）。

## 3. 依赖与顺序

```
WP-1 规格对齐 ───────────────────────────────┐
WP-10 配置卫生  ──────────────────────────────┤  ← 全部可立即并行，不改行为
WP-6 账本/诊断  ──────────────────────────────┤
WP-7 缓存收口   ──────────────────────────────┤
                                                   │
WP-3 调度正确性 (P0) ──┬──→ WP-9 H03 两库夹具   │
WP-4 窗口/级联拍板 ────┤                           │
WP-5 落库与删改 ───────┤──→ WP-9 G03 房间夹具   │
WP-8 HTTP 契约 ────────┘                           │
                        │                           │
                        └──→ WP-2 ADR 追记包 ←─────┘  ← 拍板结果回写
                                   │
                              WP-11 交付留痕
```

顺序约束：
1. WP-1/6/7/10 不改行为，可先合，且必须在任何按旧规格开工前合（否则继续修错层）。
2. WP-3 的 T-O-1/T-O-2 是唯一 P0 行为改动；T-O-6（#72）与 WP-4 的 T-P-4（C2）是两个必须由用户拍板的决策，拍板前不写代码。
3. WP-2 收尾：所有拍板结果与新行为必须有 ADR 落点（DoD）。
4. WP-9 贯穿：每个改行为的任务必须挂到登记表或现有 L2 json。

## 4. ADR 支撑矩阵

| 决策 / 行为 | 现 ADR | 状态 | 需要动作 |
|---|---|---|---|
| 行 id = 记录 id；先删后插 | ADR-060(行id) | 有；D6/D8/D11 过时 | 追记 D6 改 `WHERE in = pe`；D8/D11 标被 079 退役；N2 加 mesh-purge 例外 |
| 双水位承诺、三态判决 | ADR-060(水位) | 有；D3/D5/D6/D7/D10 过时 | 状态行删 `model_update_pending.rs`；标 079 退役段 |
| 库是同步序列化；`applied` 数据尾、`model` 收尾 | ADR-071 | 有；D2/D9/状态行过时 | 追记 |
| writing → 整库 Reinitialize | ADR-073 D3 / ADR-076 D8 | 有 | ADR-072 D2/D3 加作废横幅 |
| 收尾等 durable ACK | ADR-076 D5 | 有；「三枚同笔」过时 | 改口两写点 |
| 房间随根、常驻模型 | ADR-073 | 有；开关段、D7、文头过时 | 追记 10-23 退役 |
| MDB 闭包、范围刷新 | ADR-078/085 | 有；D5 命名 | 注释改名 |
| 账本、`gen_root` 退役 | ADR-079 | 有；D6 未回写 089 | 追记 |
| 单元级发布 | ADR-080 | **状态行草案**；D4/D9/D10 过时 | 改 Accepted；D4 `≤ S`；D9 默认开；D10 理由 |
| C2 结构连接级联翻开 | ADR-090 D2 + ADR-080 追记 | 有（2026-10-27 拍板，带语料门） | — |
| DESI 窗 XGEOMETRY 仅观察 | ADR-090 D3 + ADR-080 追记 | 有（2026-10-27 拍板） | 核对表 R2 改口（T-P-7，`docs/specs/`） |
| 按需版本 | ADR-081 | 有；D5 字面被追记二突破 | 标题/注释 |
| 缓存闸 | ADR-082/084 | 有；实施钉、D4 内部矛盾、084 D3 未决 | 追记；量完定 D3 |
| store 三层、默认 rocksdb | ADR-074/083 | 有；083 标题/D3/D5/D6 过时 | 改标题或加横幅；回填 `rocksdb` |
| records 按祖先 | ADR-086 | 有 | — |
| 切片删 | ADR-088 | 有；文内「未提交」过时 | 追记 |
| 按需新鲜度、冻结锚、闸 `≤ S` | ADR-089 | 有 | 回写 079 D6 / 080 D4 / 066 |
| 刚体前移 | ADR-066 | 有；D1 去向、「默认仍关」过时 | 追记 `transform ⊆ regen`、P4 前置 |
| `ref_rev` 非正确性依赖 | ADR-063 | 有；P4 默认、D4/P5 未做 | 追记；WP-4 收口 |
| `NameChanged` / B07 撤回 | ADR-089 追记 + ADR-090 D4 | 有（2026-10-27） | — |
| #76 元数据不做种子 | — | **缺** | 080 D10 邻近追记 |
| 数据批次 park / #72 | ADR-090 D1 + ADR-065 追记 | 有（2026-10-27） | — |
| #73 生成器指纹整库重排 | — | **缺** | 新 ADR |
| #71 `rooms_after` 拍点 | — | **缺** | 073/010 追记 |
| mesh-purge 不删管身键 | — | **缺** | 060 N2 例外 |
| `pe_ptset` 随发布事务、无 backfill | — | **缺** | 新 ADR |
| generate 110s/120s 双超时 | — | **缺** | spec 即可 |
| ADR-087 号位 | 拟稿 | **无文件** | 补文或改号 |

## 5. 风险登记

| # | 风险 | 证据 | 缓解 |
|---|---|---|---|
| R1 | 发现轮被任一 running 行整轮冻结，与「两库互不阻断」冲突 | `increment_manager.rs:4770-4778` | WP-3 T-O-2 |
| R2 | park 的 `PhaseBlocker` 整相连坐 | `initialization_phase.rs:473-486`；`batch_worker.rs:575-582` 自承 | WP-3 T-O-1 |
| R3 | 模型失败无 park → 对账空转（#72） | 10-27 审计 `:26` | WP-3 T-O-6 拍板 |
| R4 | 数据尾已提交、收尾未落就崩 → 整库 wipe（有意代价） | ADR-073 D3；`write_state.rs:88-94` | 写进规格；T-A-5 钉 |
| R5 | 同进程 Partial 且 `applied==file_latest`：lagging 可能挂着无人追 | `startup_reconcile.rs`；ADR-065 D4 | T-A-7 核实 |
| R6 | 测试钩 release 生效，误设环境变量 = 停跑 | `batch_worker.rs:974-990` | T-O-5 |
| R7 | C2 关 → D10 整窗不分桶，纯戳窗白算 | `window_root_plan.rs:1830-1845` | T-P-4 |
| R8 | CATA 正确性仍绑 `ref_rev`，读透空索引静默漏算 | `manual_update.rs:2163-2176`；ADR-063 `:59-76` | T-P-5 |
| R9 | 初始化 `Initial` 撞 legacy 带指针行未验；`RestartInitializationData` 跳过模型面区间删 | ADR-088 `:175`；`fast_delete.rs:666-670` | T-D-6 |
| R10 | 嵌套根比旧差集更脆：祖先投影不含嵌套记录则静默清行 | 测试 `model_db_adapter.rs:9024` | T-D-7 |
| R11 | `pdms_inst`/`shape_save` 双写口与适配器同 id IGNORE 语义 | `pdms_inst.rs:21-27` | T-D-8 |
| R12 | 未 ACK / Blocked 根钉死 overlay，600s 重试期 RSS 涨 | `model_write_behind.rs:37-43` | 观测 `receipt_room_bytes` |
| R13 | GenerationInput 默认 0 不限，长驻多库留 N 份 | ADR-084 D3 | T-E-4 |
| R14 | 同 sesno 换文件（`FileReplaced`）摄入面看不到异常 | `mdb_route_directory.rs:658-674` | T-H-3 |
| R15 | `watch_dbnums = [7997, 7998, 8000]` 是非正常运行态 | `DbOption.toml:266` | T-F-9 |
| R16 | ADR 互相覆盖未改旧文，改水位写点极易抄错 | §4 矩阵 | WP-2 |
| R17 | read-through mirror spool 不可达但 `/health.mirror` 报 ready | `lib.rs:2487-2490`；`handlers.rs:1095-1097` | T-F-7 |
| R18 | 读透形态回执永远没有 `rooms_*` | `unit_room_snapshot.rs:10,52` | T-G-6 |

## 6. 里程碑（按门）

- **M0 规格可信**：WP-1 + WP-10 合；`incr-gen-fixes` 标历史；conformance R6/R22 改挂；API 规格无退役路由。
- **M1 调度不连坐**：WP-3 T-O-1/T-O-2 合；H03 两库夹具绿。
- **M2 决策落文**：#72、C2/D10、XGEOM DESI 门、`pe_ptset` 四项拍板并有 ADR；WP-2 合。
- **M3 测试门 ≥ 80%**：登记表 has ≥ 45/56；rotate-chain 绿；G03/H03 至少 partial；#73/#74/#76 单测跑过。
- **M4 交付留痕**：tag + verification 入库。
