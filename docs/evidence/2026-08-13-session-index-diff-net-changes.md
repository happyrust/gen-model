# 会话索引差分（sesno 窗口净增删改）验收证据

- **日期**：2026-08-13
- **交付物**：`src/data_interface/session_index_diff.rs`（差分核心）、
  `aios_db.parse.net_changes`（绑定）、`python/testbed/net_changes_probe.py`
  （CLI + `--verify` 对拍审计器）
- **计划**：`.cursor/plans` 「sesno 窗口净变化快速判定」；术语见 `CONTEXT.md`
  「会话索引差分」

## 差分口径的三条实测规则（均有诊断探针记录与回归单测钉住）

诊断工具：`live_ams8000_diagnose_reachable_paths_for_one_refno`
（`AIOS_DIAG_REFNO` / `AIOS_DIAG_SESNO`）。对象：
`D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001`（sesno 1–214）。

1. **同键子指针首见者胜**（幽灵 `24384_22234`）：根页 8116 上
   `#1 key=24384_19889 → 8114` 与 `#2..#5 同键 → 4667`——4667 是 Save Work
   重写后被抛弃的旧子树（其叶 3095 留着整排 flag=0 墓碑镜像），跟进它会捞出
   与回放路径「剔除 19,611 条临时 Add」同源的幽灵。回归：
   `stale_duplicate_key_child_pointers_are_ignored_and_never_pollute_shared_roots`
   （含「陈旧指针不得混入共享根集合，否则 base 侧漏报删除」的第二重断言）。
2. **路由不看 flag**（幽灵 `24384_3952`）：内页 7875 上同键两组指针
   `#52 flag=0 → 1504`（发布后的子树，生产搜索首见即选它）与
   `#53..#62 flag=1 → 2802`（陈旧）——flag 取值与新旧**无关**，按 flag 过滤
   反而选错。回归：`level_regressions_and_routing_anomalies_are_counted_and_flags_stay_blind`。
3. **键范围路由**（幽灵 `24384_25843`）：该键的条目躺在覆盖 [7415, 7790) 的
   叶 5648/5676 里（回收页残留），点查按键路由永远到不了。差分带
   `[本条目键, 下一条目键)` ∩ 父界下降，范围外叶条目剔除并计数。回归：
   `out_of_range_leftover_leaf_entries_are_invisible_like_the_point_search`。

## live 对拍（差分 ≡ 生产 B+ 点查，逐 refno 仲裁）

`live_ams8000_net_diff_agrees_with_point_lookups_and_covers_replay`
（2026-08-13，debug 构建，仲裁集 = 差分结果 ∪ 回放触达集）：

| 窗口 | 差分 | 回放 | 加速 | 净结果 | 仲裁 |
|---|---|---|---|---|---|
| 1..=214 | 695ms | 10,772ms | 15.5× | added 6,609 | 6,695 refno 全一致 |
| 107..=214 | 84ms | 2,169ms | 26× | +5 / -51 / ~18 | 95 全一致 |
| 209..=214 | 17ms | 471ms | 28× | +1 / -1 / ~3 | 6 全一致 |
| 214..=214 | 11ms | 376ms | 34× | ~1 | 1 全一致 |

目标侧读页 119（全窗口）；相邻窗口共享子树剪枝 51–157 枝，IO 正比于变更量。

## amssys（SYST 多会话形态）全量窗口审计

`net_changes_probe.py --file amssys --verify`（dbnum 8191，sesno 1..=169，
2026-08-13 补测）：

- 差分 430ms（目标侧读页 23）vs 回放 4,572ms（151 会话 / 4,267 op），**10.6×**。
- 净集：差分 1,338 added；回放折叠净集 1,917 条，其中 **818 条（43%）为旧口径
  盲区**，点查仲裁全部站差分一边：孤儿 Deleted 腿误报 653、删除误报实为存在 76、
  漏报存在 74、modified 误报实为 added 15。SYST 的临时记录churn远高于 DESI，
  逐会话回放在这类库上的净口径噪声最重——这正是净窗口工程最先受益的形状。
- 结构观察：同键重复子指针 5、范围外叶残留 1,197（均已按点查口径排除）。

## 探针 `--verify` 全量窗口审计（差异归因）

`net_changes_probe.py --file ams8000_0001 --verify --with-noun`（窗口 1..=214，
输出存 `.scratch/net-changes-ams8000-full.json`）：

- 差分 1,788ms（含 6,609 次 noun 记录解析）vs 回放 11,932ms（211 会话 /
  6,936 op），6.7×。
- 与回放折叠净集不一致 **154 条**（点查为**同源判定基准**，列出并归类分歧、差分与
  点查零分歧；删除类判据的独立性另由 core.dll `elementsDeletedBetween` 键集差背书，
  report §4.4，**非**同源点查自证），逐类归因：
  - 67 条：回放**漏报存在**（元素在场，回放没有任何 op）；
  - 64 条：回放误报 modified（实际两端未变/不在场——临时 Add 被终态对账剔除
    后留下的孤儿 Modified 腿）；
  - 22 条：回放误报 deleted（同源孤儿 Deleted 腿）；
  - 1 条：回放误报 deleted（实际 added）。
- 结构观察（全部按点查口径排除，不进结果）：同键重复子指针 52、范围外叶残留
  6,897、读不动子页 53、层级异常 7。

## CI 面

- 模块纯单测 11 条（合成迷你 B 树，含剪枝不读页断言、纯文件纪律源码断言
  `the_diff_module_never_touches_the_database`）。
- `tests/db8000_session_pairs.rs` 性质 h
  `index_diff_matches_replay_folding_on_every_case_window`：issue-019 真实
  ams8000 会话链上，案例窗口 + 整链窗口差分 ≡ 回放折叠（台账腿由性质 e 闭环），
  19/19 全绿。
- Python 离线档 `test_parse_offline.py` 增 3 条：25..=26 净三态与夹具台账逐条
  相等（ZONE=modified、EQUI/子件=deleted，with_noun 从旧记录解出 BOX/EQUI/ZONE）、
  26..=26 base 落在 25 且自我抵消不出现、窗口超界响亮拒绝。离线档 65 passed。

## 引擎接线（ADR-022 P0，2026-08-13 晚）

净窗口收集器 `net_window::collect_net_window` 落地并接入执行链（灰度开关
`net_window_collection` / `AIOS_NET_WINDOW`，默认 off；预览、执行体、崩溃恢复
重收集、worker 尾段重收集（源码实测共 5 个调用点）全部经 `IncrementPipeline::collect_window`
唯一入口）：

- **复刻 diff 对拍**（`db8000_session_pairs` 性质 i，CI 常驻）：gen-model 侧
  `diff_ele_data` 与 vendor 内联 diff 在真实 ams8000 会话链上 Modified 负载
  **逐桶相等**（九个属性差量桶 + children 两端 + noun），净三态 ≡ 回放折叠，
  全部案例窗口全绿（样本为各窗口实际 Modified 条目、非 test binary 计数）。
- **live 收集器负载对拍**
  （`live_ams8000_net_window_payloads_match_replay_on_single_touch_refnos`）：
  ams8000 三个窗口，**6,499 条 Add 负载与回放渲染逐字符相等**；全窗口净收集
  1.24s（含 6.6k 终稿记录合成）vs 回放 10.9s（8.8×）。Modified 单触达样本
  本轮为 0（该文件当前形状多触达为主，负载等价由性质 i 钉住）。
- **真实文件逼出的第三条口径对齐**：字典缺项的系统记录家族（`MNUM not exist
  in attr_info_map`、`impl_len > data_len`，ams8000 全窗口 64 条）终稿解析
  失败——回放路径对它们落 `None` 操作从未入库，净收集对齐为跳过 + 计数 +
  聚合警告（`unparseable_finals`），不整批硬失败。
- 净修改条目现携带 base 端记录位置（`NetEntry::base_loc`），合成 diff 直读
  两端版本，不付点查。
- 已知偏差（灰度期记账）：`merged_sesnos` 回执仍取「有操作的会话」清单——
  净口径下显示为「有净变化的会话」，比回放少列自我抵消的会话；spec FR-6 的
  会话页清单口径留待翻默认值前落地。

## live A/B 全链路执行（ADR-022 验收 3，2026-08-13 深夜）

用例 `python/tests/test_net_window_ab.py`（房间增量档，opt-in：`AIOS_NET_AB=1`；
conftest 自起一次性内存 SurrealDB @8071）。同一起始库态、同一增量窗口，
`AIOS_NET_WINDOW` off/on 各走一遍**完整执行**（扫描 → 入队 → worker 冻结 →
ADR-017 暂存窗口 → 窗口内模型生成 → 提交 → 水位收口），终态签名逐维对拍。
连续两轮全绿（`.scratch/net-ab-run6.log` / `net-ab-run7.log`，各 ~3 分 16 秒）。

- **目标库与窗口**：testbed 副本 ams8000（dbnum 8000，sesno 1..209，基线 pe
  6,542 行，Ref0 24384）；窗口 `105..=209`（K = file_latest // 2 = 104，105 个
  会话）。文件仲裁（`parse.net_changes`，与生产 B+ 点查逐字对齐）：净三态
  added=6 / deleted=51 / modified=16，其中 7 条为「原样重写 / 改了又改回」
  （两端内容逐字段相同）。
- **臂序列**：镜像 `render_delete_phases` 三阶段清库 + 生产基线入口
  `initialize_project_dbnum_baseline`（`aios_db.sync.baseline`）重建（12.3s /
  12.5s，两臂重置后 7 个维度签名逐项相等）→ 只拨水位到 104 → 设口径 env →
  预览断言口径自报（净臂必须出现「收集口径：净窗口…」，off 臂必须不出现；
  预览与执行共用 `collect_window` 唯一入口）→ `incr.execute_manual` 执行。
  刻意不走 ADR-021 回退批次做臂重置：其重建半边会把基线登记的全部 2,229 个
  交付根在批次内当场生成（run4 实测 22 分钟/臂，`.scratch/net-ab-run4.log`），
  基线生成不是本用例被测面；同理重置尾清空基线登记的 pending 积压（两臂同样
  处理，起点同构由断言 0 直接钉住）。
- **耗时**：窗口全链路执行 回放 35.0s vs 净 11.0s（**3.2×**，含窗口内真实模型
  生成与提交）；收集阶段净口径自报差分 154ms（回放收集同窗口 ~2.2s，probe
  对拍 4.4×）。
- **签名维度与结论**（逐维）：
  1. `dbnum_watermark`（applied/sesno/file_latest）：相等，applied 回到 209。✅
  2. pe 全量行（按 Ref0 record-id 区间取，含 dbnum 缺失行；id/sesno/noun/name/
     dbnum/deleted 六字段）：共同 6,543 行逐字段相等；差异恰 2 行且**全部归因**
     ——净臂多持 `24384_26184`（/AIOS-INC-ROOM-MEMBER-BOX @118）与
     `24384_26185`（/AIOS-INC-ADD-BOX @131），文件仲裁均为窗口内新增且终点在场：
     **回放（连同旧基线解析）漏报存在，净口径持文件真值**。零未归因差异。✅
  3. noun 属性表全量内容（6,543 行）：相等。✅
  4. `pe_owner` 边（含复合 id 序号）：6,542 条相等。✅
  5. `ref_rev` 出边：净 11 ⊂ 回放 24；13 条差异边的引用者**全部**落在上述 7 个
     原样重写元素上（SPRE/CATR → ams5052 目录行）——§5.1「改了又改回不再触发」
     家族：回放对内容未变元素照发 Modified 并顺手重建其出向边，净口径不发操作；
     生产店里这些边在窗口前就已在位（增量维护装的），重置后的空 ref_rev 店放大
     了差异。净臂凭空多出的边：0。✅（豁免逐条打印在用例输出里）
  6. `model_update_pending`（action/target/source_end_sesno）：0 == 0。✅
  7. `dbnum_info_table`：count 按记账恒等式核平（终态 = 起点 + 本臂新建 −
     对基线活行立碑），sesno 两臂相等；`dbnum` / `max_ref1` 两字段不进签名——
     `update_dbnum_event` 对两者是「最后一个事件说了算」（墓碑创建事件把 dbnum
     MERGE 成 NONE、max_ref1 直接覆写），属事件层既有噪声、与收集口径无关。✅
- **墓碑 sesno 归一（§5 预告的唯一预期偏差）实际为 0 条**：fork 2.1.4 的
  `UPDATE pe:{id} SET deleted = true` 对不存在的行是无操作，而本 A/B 的起点是
  「当前文件的基线」——窗口内被删元素在基线里本就无行，两臂的删除语句都落空，
  谁也没立碑。该偏差留待「起点早于删除会话的真实存量店」形态验证。
- **两臂一致的副产物**：都补建了基线缺失的 WORL 系统记录 `16192_0`（窗口内
  modified@138；基线解析不落系统记录）——共同平面内容逐字段相等，不构成偏差。
  房间面：回放臂发布 4 个房间重算目标、净臂 0 个（§5.1 无操作重生成的下游），
  均在批内收敛（room_failed=0），不进数据签名。
- **抓到并修复一个真实引擎缺陷（与口径无关，两臂共用的生成写路径）**：
  `inst_geo` 几何参数写入用 `UPSERT … MERGE`（2026-08-13 `276aa5f6` 引入，替换
  `INSERT IGNORE`），而普通 LCylinder 与非切角 SCylinder 按设计共享同一个单位
  网格行（`hash_unit_mesh_params` 同返 `CYLINDER_GEO_HASH`）——两个变体先后
  MERGE 把 `param` 深合并成 `{PrimLCylinder, PrimSCylinder}` 双键对象，enum
  反序列化永久失败，**所有**引用该共享行的根从此生成不出来（run4 实测：净臂
  2,229 根批量重生成全灭 + 逐根重试全灭）。修复
  `render_inst_geo_upsert`：`param` 整值 `SET` 覆盖（保留 meshed/aabb/pts 派生
  字段，双键坏行下次参数刷新即自愈）；回归
  `a_variant_switch_on_a_shared_unit_row_replaces_param_wholesale`（回退到
  MERGE 写法当场变红）。受影响 Rust 面：lib 定向 12 条全绿、
  `db8000_session_pairs` 20/20 全绿。

## 合成器纯单测（T20，2026-08-13）

`collect_net_window` 原先零纯单测——它吃 `&mut PdmsIO`（vendor 具体类型、非 trait），
三形状与降级路径全部进不了 CI。现抽出**纯合成内层**：

```rust
fn synthesize_net_window<F>(net: NetChangeSet, mut resolve: F) -> anyhow::Result<NetWindowOutcome>
where F: FnMut(RecordLoc) -> anyhow::Result<EleData>
```

`NetChangeSet` 按值接收（`stats` 直接移交产物，不 clone）；resolver 收窄成「给我这个
位置的记录」，「谁的记录 / 哪一端 / 页与偏移」的错误文案由合成器内的 `resolve_record`
包装——只有一处权威，测试不必复刻文案。缝的先例是 `session_index_diff` 的 `MemPages`。

**七条纯单测**（三形状 + 两条降级 + 一条硬失败 + 原样重写）：

| 测试 | 钉住的行为 |
|---|---|
| `a_net_added_entry_becomes_an_add_on_its_last_touch_session` | Add 挂 last-touch，不挂窗口终点；`stats` 原样移交 |
| `a_net_deleted_entry_hangs_on_the_window_end_session` | Deleted 挂 `target_sesno`；空桩 + `unparseable_finals == 0` 反证「删除不解析记录」 |
| `a_net_modified_entry_diffs_both_versions_exactly_once` | `seen` 向量钉住解析序与次数恰为 `[终稿, 基版本]` |
| `a_base_parse_failure_degrades_to_add_and_names_the_refno` | 基版本失败 → `Add(latest)` + 点名 refno |
| `an_unparseable_final_is_skipped_counted_and_aggregated` | 终稿失败 → 不入窗口 + 计数 + **聚合**警告带样例 |
| `a_missing_base_loc_fails_hard_and_names_the_refno` | `base_loc` 缺失 = **硬失败**，不许降级 |
| `an_identical_rewrite_emits_nothing_but_is_counted` | 原样重写不发操作、无警告 |

**分类澄清**：原样重写**不是降级路径**，是正常判定的正常结果；降级只有终稿解析失败
与基版本解析失败两条，`base_loc` 缺失是硬失败。

**验证策略（纯提取，不伪称先红）**：安全网 = 性质 i + 既有 live 负载对拍；新测试有效性
由**逐分支变异抽检**证明——5 处一次性变异逐一准确变红（变异代码不入库）。

实测：`net_window` lib **13 passed / 0 failed / 1 ignored**（ignored 是需真实 ams8000
的 `live_ams8000_net_window_payloads_match_replay_on_single_touch_refnos`，**本轮未跑，
不记作通过**）；`db8000_session_pairs` 集成目标 **20 passed**（含性质 i；这是该目标的
用例数，**不是**性质 i 覆盖的窗口数——窗口数由夹具决定，不写死）；Python 离线档
**66 passed / 20 deselected**。

## 存量库删除等价直证（T11b，2026-08-13）

本文「live A/B 全链路执行」节的删除腿是**空跑**——起点是「当前文件的基线」，窗口内被删
元素在基线本就无行，两臂删除语句都落空（墓碑归一实测 0 条）。**恒绿的测试不是证据**。
本节补的是「起点早于删除会话、库内确有活行」的形态。

用例：`python/tests/test_net_window_ab.py::test_net_and_replay_agree_on_a_stock_deletion`。

**存量基线构造**：`python/tests/_session_snapshot.py` 是 `src/bin/db_session_fixture/
session_cut.rs` 的 Python 逐字节镜像（PAGE_SIZE 0x800、头偏移 40、`previous`+4 /
`sesno`+12 / `latest_page`+20、截断到 `(latest_page+1)` 页并回写头指针）。镜像正确性
两道钉：① 合成文件不变量离线单测（与 `session_cut.rs` 同构，常跑）；② 真实文件上与
Rust 权威 `db_session_fixture inspect` 的会话链逐条对齐，且对现切的 @K 文件再 `inspect`
回读确认 `latest == K`。找不到 Rust 可执行档**默认硬失败**（`AIOS_T11B_ALLOW_NO_RUST_CHECK=1`
才降级）。另加一条便宜复核：@K 快照大小必须恰为 `(latest_page+1) * PAGE_SIZE`。

**结果**：切点 **K=24**，窗口 **25..=209**，文件层净删除 oracle **4 条**；起点库里确为
活行、窗口内被净口径**真立碑 2 条**（`24384_24778` / `24384_24779`，且 ⊆ 文件删除
oracle，无越权删除）；**共同活行 6,536 逐字段一致**；**0 条未归因差异**。最终 live
**118s 全绿**。

**判定分工（本节最易被读错的地方）**：
- 被测对象是**纯文件判定**——净收集只吃「文件 + 起止 sesno」给出 Deleted 集。
- 删除的独立机制基准是 core.dll `elementsDeletedBetween`（`0x5900250`）的**索引键集差**
  （旧根有键、新根无键，report §4.4）；`parse.net_changes` 的 deleted 集正是该判据的
  纯文件复刻，用作窗口删除 oracle。**不**拿 `pdms_io::search_latest_refno` 点查当独立
  证明——它与净路径同判据，属同源自证。
- **DB 查询只用于两件事**：① 窗口**前**证被删 refno 在起点是活行（`deleted = false`，
  空跑主防线）；② 窗口**后**证净口径真把活行立成了墓碑（下游附加断言）。**不作删除判据。**
- 允许 net / replay 在删除腿上**预期发散**（回放有跨会话删除盲区，issue-019 家族），
  逐条归因，净口径持文件真值。

**红证**：`AIOS_T11B_FORCE_EMPTYRUN=1` 故意用全量文件做基线 → 被删元素起点无活行 →
空跑主防线**准确变红**。本用例不是恒绿。

**文件安全**：testbed 库文件的换入换出走**同卷临时文件 + fsync + `os.replace` 原子替换**
（进程被 kill 只会看到旧文件或新文件，绝不留截断源库），另在同卷 scratch 里留 `pristine`
备份，`finally` 优先从 pristine 原子恢复、校 SHA256 之后才清理。收尾实测源文件
**16,504,832 字节**无损恢复。scratch 建在监控目录下的**子目录**里——`INGEST_MAX_DEPTH = 1`
只认直属文件，子目录内容（都是字节合法的 dbnum 8000 库）扫不到，不会触发 F6 同号重复阻断。

## release 方向性单点测量（T18a，2026-08-13，**n=1、非性能门**）

只为回答一个问题：ADR-022 决策 4 会不会被 release 实测推翻。**不是** T18 的性能门证据。

| 窗口 | 会话 | 净三态 a/d/m | 回放 `ops_total` | 复触率 | 完整净收集 | 回放 | 比值 |
|---|---:|---|---:|---:|---:|---:|---:|
| **104..=209**（高复触，判定形状） | 106 | 6 / 51 / 16 | 215 | **2.95** | **3ms** | 53ms | **≈17.7×** |
| 1..=209（Add 地板窗，不作判定） | 209 | — | — | 1.05 | 126ms | 792ms | ≈6.3× |

高复触窗的 raw net / replay 发散 **72 条，全部归因回放旧口径盲区，点查零分歧**。

复触率（= 回放 `ops_total` ÷ 净集大小）是解释倍数的关键变量：净收集的收益正比于它。
地板窗复触率 1.05，本就不该快多少，拿它测出的 6.3× 是形态决定的，**不能**用来判定门。

**结论仅限**：在净收集的**动机形状**（高复触）上，决策 4 不需修订。**T18 的正式统计
（1 warmup + ≥5 次、median/min/p95、warm 判定 cold 另报）与 250206 SYST 现场硬门
仍未完成。**

## 已知边界

- **Added 形态夹具（T13）BLOCKED**：仓内**不存在**同时满足「Added > 0」且「raw 净集
  == 回放折叠集」的真实窗口——现有会话链上带 Added 的窗口都伴随回放旧口径盲区，
  raw 两集不等，性质 h/i 直接指过去必红。必须用受控 E3D 录一个 `scratch-create`
  案例（新建 SITE/ZONE → 建元素 → Save Work，窗口内无删除无临时态）。**不得**为点亮
  它而放宽 h/i 断言。当前 Added 由纯单测 + live 全窗口（6,609 added 全过点查仲裁）兜底。
- **`Deleted` 的会话号是两层语义，别混**：① `session_index_diff::enrich` 用
  base 端旧记录 `loc.pgno` 反查，`NetEntry.last_touch_sesno = Some(删除前旧版本
  归属会话)`（**不是 None**，也不是删除动作发生的会话——净差分判不出删除发生在
  哪个会话）；② `collect_net_window`（`net_window.rs` L95-101）**故意忽略**该字段，
  把发出的 `EleOperationDetail::Deleted` 挂到 `target_sesno`——所以输出 Deleted 的
  `target_sesno` **不是**真实删除时刻，只是「窗口终点会话」。需要逐会话归属时仍用
  `parse.collect_changes`。
- 文件被 ADMIN 压缩/回卷（追加模型破坏）时差分响亮拒绝（`目标索引根不高于
  base 会话末页`），不给静默错答案。
- **机制层由 live IDA 闭合（2026-08-13 追加）**：
  `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`（core.dll 3.1，
  SHA `3c1f…417d`，符号自带非猜名）证实 core.dll 会话变更枚举
  （`DB_IndexTableCompare`，opcode 266/270，主索引表 `13387743`）**本就是双根 B+
  归并差分**、删除 = 键在旧根不在新根的**集差（非墓碑）**、变更检测**全链路**（页取 +
  begin + 双根归并）**不读 / 不按 flag 过滤**、`0x80000001` 是页内键哨兵——本文
  「路由/存在性不看 flag」「base 有 target 无 → Deleted」「`is_start_page` 哨兵」
  三处口径由此获**核内权威背书**。**仍未闭合**（report §4.5 / C3 / C4）：raw 叶内
  `flag` 的存在/偏移/位宽/取值枚举，**以及 flag 在变更检测链路之外是否另有可见性 /
  过滤门**——可断言的只是「权威变更检测链路不以 flag 作门」，故翻默认不受其阻断，
  但**不得**泛化为「flag 全无功能 / 功能上否定它是可见性门」。
