# Tasks: 增量窗口净收集

标注：✅ 完成 / ⬜ 待办；[P] 可并行。每项带文件路径与钉住它的测试。

## P0 工具层（✅ 2026-08-13）

- ✅ T1 双根差分核心：`src/data_interface/session_index_diff.rs`
  （剪枝/哨兵/flag/同键去重/键范围路由；单测 11 条 + 零 SUL_DB 源码断言）
- ✅ T2 Python 面：`python/src/lib.rs` `parse.net_changes` +
  `python/pysrc/aios_db/parse.pyi` + `python/README.md`
- ✅ T3 探针：`python/testbed/net_changes_probe.py`（`--verify` 点查仲裁归因）
- ✅ T4 夹具断言：`tests/db8000_session_pairs.rs` 性质 h；
  `python/tests/test_parse_offline.py` 净三态 3 条
- ✅ T5 live：`live_ams8000_net_diff_agrees_with_point_lookups_and_covers_replay`
  （台账 D 组）；amssys 审计（回放净集 43% 盲区归因）

## P0 引擎接线（✅ 2026-08-13 晚）

- ✅ T6 合成器：`src/data_interface/net_window.rs`（`collect_net_window` +
  `diff_ele_data` 复刻；`the_net_window_module_never_touches_the_database`）
- ✅ T7 派发点：`IncrementPipeline::collect_window`（4 个概念路径 / **5 个源码调用
  点**：increment_pipeline fresh + recovery 2、manual preview + execute 2、
  batch_worker 尾段 1；`execute_one_dbnum_collects_the_window_exactly_once` 禁直调）
- ✅ T8 灰度开关：`src/options.rs`（默认 off；
  `net_window_collection_defaults_to_replay` /
  `the_net_window_env_override_wins_in_both_directions`）
- ✅ T9 对拍：性质 i `net_window_collector_matches_replay_ops_on_every_case_window`
  （Modified 九桶逐桶相等）+
  `live_ams8000_net_window_payloads_match_replay_on_single_touch_refnos`
  （6,499 条 Add 渲染逐字符相等）
- ✅ T10 留痕：evidence「引擎接线」节、live 台账、changelog、DbOption.toml 注释

## P1 灰度证据收口（⬜）

- ✅ T11 live A/B 全链路执行（**已实跑的 A/B 形态**）：testbed 8071 内存库，同库
  同窗口(105..=209)两口径各走解析→计划→落库→水位，7 维终态签名等价、差异全部
  归因，连续两轮全绿（evidence「live A/B 全链路执行」节、台账、
  `python/tests/test_net_window_ab.py`）。
- ✅ T11b 「起点早于删除会话的真实存量库」形态（2026-08-13）：
  `test_net_and_replay_agree_on_a_stock_deletion`。切点 **K=24**、窗口 **25..=209**，
  文件层净删除 oracle **4 条**；起点库里确为活行、窗口内被净口径**真立碑 2 条**
  （`24384_24778` / `24384_24779`），**共同活行 6,536 逐字段一致、0 未归因差异**。
  强制空跑变异（`AIOS_T11B_FORCE_EMPTYRUN=1`，用全量文件做基线）**准确变红**——
  证明本用例不是恒绿。存量基线由 `_session_snapshot.py`（`session_cut.rs` 的 Python
  镜像）切 @K 得到，文件替换走**同卷临时文件 + fsync + `os.replace` 原子替换** +
  `pristine` 备份 + `finally` SHA 校验；最终 live **118s 全绿**，源文件
  **16,504,832 字节**无损恢复。**判定分工**：删除判据是纯文件（`parse.net_changes`，
  core.dll `elementsDeletedBetween` 键集差的纯文件复刻）；**DB 查询只用于验证窗口
  前活行与窗口后墓碑两个状态，不作删除判据**。
- ✅ T12 [P] `CollectedWindow` / `merged_sesnos` 会话页清单口径（FR-6 尾项）：两种
  口径都从冻结范围的会话页映射取升序去重清单，贯穿预收集、崩溃重放与
  `IncrFileSuccess`，不再依赖「有操作的会话」；两臂第一条 warning 固定标注模式。
  Replay 清单与操作共用一次文件打开，回归覆盖空保存、自抵消、稀疏窗口、预收集、
  崩溃固定区间与后续计划失败仍保留口径 warning。
- ⬜ **T13 [BLOCKED，M1 Exit 的唯一未闭合项]** Added 夹具录制：`db_session_fixture`
  录「创建→Save Work」案例，`AIOS_SESSION_FIXTURE` 指入复用性质 h/i（不改测试代码）。
  **阻塞原因（2026-08-13 实查）**：仓内**不存在**同时满足「Added > 0」且
  「raw 净集 == 回放折叠集」的真实窗口——现有会话链上带 Added 的窗口都伴随回放旧
  口径盲区，raw 两集不等，性质 h/i 直接指过去必红。**必须**用受控 E3D 录一个
  `scratch-create` 案例（新建 SITE/ZONE → 建元素 → Save Work，窗口内无删除无临时态）。
  **绝不允许**为了点亮它而放宽性质 h/i 的断言，也不允许标完成。
- ⬜ T14 [P] vendor 合并项：`diff_ele_data` 提取进 pdms-io 与
  `get_refno_operation_status` 共用（ADR-022 决策 2 优先方案），落地后删
  gen-model 复刻分支。
- ⬜ T17 [翻默认前] 口径冻结快照：ADR-022 决策 4 规范上「同批次不换口径」，但当前
  `AIOS_NET_WINDOW` 每次收集实时读、无冻结快照。在批次冻结点取一次口径存进冻结
  批次（`src/options.rs` 读取入口 + `src/data_interface/increment_pipeline.rs`
  `collect_window` + `src/data_interface/manual_update.rs` 执行体），附「同批次内
  env 变化不换臂」的回归测试。
- ✅ T18a [方向性单点测量，**n=1、非性能门**]（2026-08-13，release）：只为回答
  「决策 4 会不会被推翻」。**高复触窗 104..=209**（106 会话，a/d/m = 6/51/16，
  回放 `ops_total` 215，**复触率 2.95**）：完整净收集 **3ms** vs 回放 **53ms ≈ 17.7×**。
  该窗 raw net / replay **发散 72 条，全部归因回放旧口径盲区，点查零分歧**。
  对照 **Add 地板窗 1..=209**（复触率 1.05）：126ms vs 792ms ≈ **6.3×**——地板形态
  本就不该快多少，不作判定。**结论仅限**：在净收集的**动机形状**（高复触）上
  决策 4 **不需修订**。**T18 的正式 5 次统计与 SYST 现场硬门仍未完成。**
- ⬜ T18 [翻默认前] 性能实测证据（当前**未达门**，如实标不得伪绿）：① ≥20 会话
  **完整收集**（含终稿合成，release / 代表形态）≥10× —— 含合成的唯一有效基线是
  **debug 8.8×**；**A/B probe 的 4.4× 是「净差分 vs 回放完整收集」的混层比较，
  仅作下界参考、不得当门证据**；纯差分 15–34× 同理不算数。T18a 已给出 release
  方向性单点 17.7×（n=1），但正式门要求 **1 warmup + ≥5 次、median/min/p95、
  warm 判定 cold 另报、两类窗口 + 复触率与环境项**，尚未跑。
  ② **250206（SYST）单趟 collect < 30s 是硬门，该库在客户现场**，本地 amssys
  只是代理形态，代理达标不等于硬门达标（未实测）。计时入 evidence；若 release
  实测仍不达 ≥10×，**必须显式修订 ADR-022 验收 4**（与翻默认分属两个提交），
  不得静默降门。
- ✅ T19 [非阻断，CLOSED] qualifier 恢复对拍（2026-08-13）：断言已落
  `tests/db8000_session_pairs.rs` 性质 i 的 Modified 分支——两臂经
  `classify_operation_effects` 恢复出的 `qualified_changes` 逐项相等，集成绿，
  **未扩任何公开 DTO**。**强度如实标**：当前 issue-019 夹具两个案例都是删除、
  对拍到的 Modified 里数组属性零变化，两侧恒为空，这条现在是 **empty == empty**
  ——**不是 qualifier 语义已被覆盖的证据**。它当前的唯一价值是防回归（将来有人
  把 helper 改成从 `current_data` 取 qualifier 会先红）。要长出牙齿需等录到带
  数组属性变化（PARA/POS 一类）的 data-modify 案例。
- ✅ T20 合成器纯单测（2026-08-13）：`collect_net_window` 抽出**纯合成内层**
  `synthesize_net_window(net, resolve)`（`NetChangeSet` 按值接收、resolver 收窄为
  `FnMut(RecordLoc) -> Result<EleData>`、解析上下文错误格式化留在合成器），**七条
  纯单测**覆盖三形状 + 基版本失败按新增 + 终稿失败跳过计数聚合 + `base_loc` 缺失
  硬失败 + 原样重写计数（原样重写**不是降级路径**，是正常判定的正常结果）。
  实测：`net_window` lib **13 passed / 0 failed / 1 ignored**（ignored 是那条需真实
  ams8000 的 live，**本轮未跑，不得记作已通过**）；`db8000_session_pairs` 集成目标
  **20 passed**（含性质 i；这是该目标的用例数，**不是**性质 i 覆盖的窗口数）；
  **5 处分支变异逐一准确变红**（变异代码不入库）；Python 离线档
  **66 passed / 20 deselected**。
- ✅ T21 审查修复：子页不可读/层级不下降、终稿解析失败、last-touch 缺失改为
  整窗 Err；保留 base 解析失败保守 Add 与已验证重复/越界残留计数。错误在预览按
  dbnum 显示、执行批次 Failed，不自动回退 replay
  ——`src/data_interface/session_index_diff.rs`、`net_window.rs`、`increment_pipeline.rs`
- ✅ T22 审查修复：`ref_rev_maintain` 补偿载荷全量严格解析，非法或空列表进入
  `mark_failed` 并保留行——`src/data_interface/side_effect_pending.rs`

## M1 / M2 状态（2026-08-13）

里程碑划分见
[`docs/plans/2026-08-13-net-window-default-on-development-plan.md`](../../docs/plans/2026-08-13-net-window-default-on-development-plan.md)。

- **M1 正确性闭环**：技术项 **T20 / T11b / T19 / T18a 全部完成**，**唯 T13 阻塞**
  （无合格真实窗口，须受控 E3D 录制）。**M1 Exit gate 因此仍未通过。**
- **M2 运行闭环与翻默认**：**不得启动**——它的 Entry gate 是 M1 Exit 全绿。

## P2 翻默认值（⬜，结果层门 T13/T17/T12/T18 全绿后；机制层已由 live IDA 闭合）

- ⬜ T15 `net_window_collection` 默认 on（改 `effective_net_window_collection`
  兜底值 + 单测 + DbOption.toml 注释 + changelog）。**前置证据门（ADR-022 验收 5）**：
  机制层已由 live IDA 闭合（双根差分 / 删除即集差非墓碑 / flag 不进变更检测链路 /
  哨兵，见
  `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`），**无需**再等
  flag 位编码逆向（已证权威变更检测链路不以 flag 作门；链路外语义未闭合但不影响
  净窗口正确性）；余下**结果层门**必须先绿——**已闭**：T11b（存量库删除等价）、
  T20（合成器纯单测）+ T12（会话页清单）；**未闭**：T13（Added 独立夹具，
  **BLOCKED**）+ T17（批次冻结快照）+ T18（完整收集 + SYST 硬门）。
- ⬜ T16 一个发布周期后：拆开关，回放收集退出执行路径接线（诊断入口保留），
  同步清 `fold_window` / 两个终态补丁的执行侧接线（诊断路径自用者保留）。
