# 增量更新待办复核与本轮修复（2026-07-27）

本文替代 `2026-07-26_increment-update-chain-audit-report.md` /
`…-round2.md` 里的**待办清单部分**作为现状基准。那两份审计的**问题分析**依然有效，
但它们的「还需完善」列表在写下之后又被陆续做掉了一部分，清单本身没跟着更新——
2026-07-27 这一轮逐项对代码复核时，16 项里有 4 项已经是完成态、3 项部分完成。

在真实剩余集不清楚的情况下继续动手，会反复重演「做到一半发现它已经做完了」。
这份文档的目的就是把基准钉死，并记录本轮实际改了什么。

---

## 1. 摘要

| 类别 | 结果 |
|---|---|
| 复核 | 归档 16 项：**4 项已完成、3 项部分完成、9 项真开放** |
| 修复 | B3（两条路径）、T903、T901（pdms-io 半边）、collect 双跑、`read_index_data` 页缓存 |
| 文档 | 抢救并合并了 `_p1_section.tmp.md`（§8 批次 A 执行记录） |
| 测试 | `cargo test --lib` **189 passed / 0 failed / 38 ignored** |
| 性能 | 手动增量的 collect 阶段：4 批次合计 **~737 秒 → ~7 秒** |

---

## 2. 环境实况（e2e 沙箱）

复核期间确认的拓扑，供后续会话省去重新摸底：

| 端口 | 后端 | NS / DB | 状态 |
|---|---|---|---|
| **8042** | `empty1\e2e-test\surreal-data` | 2077 / **AvevaPlantSample**(218 表)、TEST(99 表) | 主力，满数据 |
| 8043 | `empty1\ams-probe\surreal-data` | 1516 / AvevaCatalogue(95)、AvevaMarineSample(115) | CATA 探针 |
| 8009 | memory | 1516 / AvevaMarineSample(17 表) | 单测夹具，进程一死即失 |
| 8020 | ams7997 rocksdb（versioned） | 1516、main | **两个库都是 0 张表** |

主力库计数：`pe` 519,749 / `pe_owner` 5,048,260 / `SCTN` 7,216 / `SUPPO` 11,801 /
`BRAN` 651 / `EQUI` 317 / `PIPE` 308 / `MESH` 41 / `ref_rev` 221 /
`dbnum_watermark` 117 / `dbnum_info_table` 98 / `design_states` 31 /
`increment_update_attempt` 0 / `neg_relate` 0 / `nearest_relate` 0。

两点容易踩的坑：

- `empty1/.probe.txt` 里 `pe` 计数为 0 是**打在 8020 上的**，与 e2e 沙箱无关。
- 主力库 218 张表里**没有 `inst_info`、没有 `geo_relate`**。库里那 41 行 `MESH` 是
  E3D 的 MESH 元素 noun（字段是 DESP/UNIPAR/SPLT 那套），不是生成出来的模型网格。
  这直接卡住 P0-1 与大多数 `live_*` 测试——不是「库是空的」，是「没有生成几何」。

---

## 3. 16 项待办的复核现状

### P0

| # | 项 | 状态 | 依据 |
|---|---|---|---|
| 1 | POSS/POSE/CPOS 实库对拍 | **阻塞** | 无 `inst_info`/`geo_relate`，没有观测对象。SCTN 的 `POSS`/`POSE` 字段实测存在 |
| 2 | C-REF-02/03 级联上下界 | **已完成** | `manual_update.rs` 的 `c_ref_02_…` / `c_ref_03_…`，实跑绿 |
| 3 | B5 死信复活顺序守护 | **已完成** | `model_update_pending.rs` 的 `revival_clauses_run_before_the_watermark_field_they_read` |

第 2 项有个陷阱值得记一笔：两个 C-REF 测试都带
`if schema_names.is_empty() { return; }` 的**静默早退**，schema 加载失败时会「绿得毫无
意义」。已用 `att_meta_all_attributes_classify_and_references_affect_model` 验证 schema
确实加载（**6556 属性 / 1421 引用类**），且两个扫描末尾的反空转断言
（`checked > 100` / `checked > 300`）都在早退之后——测试通过即证明早退没触发。

### P1

| # | 项 | 状态 | 依据 |
|---|---|---|---|
| 4 | T804/D-01 源触发 | 未开始 | 需活 E3D 授权 |
| 5 | 视觉验收 | **阻塞** | `empty1/e2e-test/evidence` 18 个文件**全是 .txt，零张截图**；桌面捕获 0x80070057 未解决 |
| 6 | D-07/08/09 | 未开始 | D-07 换库目标现在有了：AvevaPlantSample 有 SUPPO 11,801；但 D-08 要的 `neg_relate` 在主力库是 0 行 |
| 7 | primaryList 名单 + DCHC 码表 | 未开始（有意保守） | `model_impact.rs` 的 `primary_list_hint()` 仍 `-> true`；门控机制可显式传 `false`，有 B-EVT-03 单测 |

### P2

| # | 项 | 状态 | 依据 |
|---|---|---|---|
| 8 | B2 无 geo_relate 的 inst_info 永不删除 | **已完成** | `helper.rs` 的 `render_cascade_delete` 已在引用计数守卫内显式 `delete $old_inst;`，配单测 `cascade_delete_reclaims_inst_info_even_without_geo_relate_edges` |
| 9 | B3 record_scan 早于重复 dbnum 判定 | **本轮修复** | 见 §4.1 |
| 10 | B6 CATA 派生根按目录库 dbnum 记账 | **开放** | `model_update_pending.rs` 派生根仍 `dbnum: item.dbnum` 继承 |
| 11 | B4 init 递归 vs watch 一层 | **部分** | 重复 dbnum 的洞已补（每轮 recheck `duplicate_dbnums_across_watch_dirs()`）；但深度不对称还在：init 扫描无 `max_depth`（递归），watch/去重用 `max_depth(1)`。约定仍未写明 |

### P3

| # | 项 | 状态 | 依据 |
|---|---|---|---|
| 12 | 跨仓未提交 | **开放（最大风险）** | 复核时 gen-model 217 / pdms-io 37 / rs-core-pin 5 / plant-io 14，本轮成果又叠在其上 |
| 13 | T901 热路径 dbg! | **本轮完成** | 见 §4.3 |
| 14 | T903 看门狗 panic / 静默死亡 | **本轮修复** | 见 §4.2 |
| 15 | 持久化优化遗留 | **优先级需修正** | 见 §5——归档列的优化项全在 persist 侧，实测瓶颈在 collect |
| 16 | 文档一致性 | **本轮完成** | ADR-009 已存在；`DIRECT_GEOMETRY_ATTR_NAMES` 已清成 123 条、**0 条 noun 名死分支**；`_p1_section.tmp.md` 本轮合并后删除 |

---

## 4. 本轮修复

### 4.1 B3：重复 dbnum 阻断必须先于身份落库

`DbnumState::record_scan` 是 `UPSERT dbnum_watermark:{dbnum} SET file_name=…,
file_path=…, file_size=…, file_latest_sesno=…`——**按 dbnum 主键**。同一 dbnum 的第二个
文件只要先跑到这里，就把首见文件的身份覆盖了；之后即使阻断该 dbnum，回退 / 迁移
检测的基准也已经被污染。

归档只点了 `init_watcher` 一处，**实际两条自动路径都中招**，`async_watch` 里是同样的
顺序错误。两处都已调序为「先重复判定、再落库观察」。

新增 `duplicate_dbnum_guard_precedes_scan_record_on_both_auto_paths`：这两步嵌在依赖
实库的大函数里、无法用纯函数钉住，故直接在源码上钉顺序。marker 用 `concat!` 拼接，
避免测试自身的字符串字面量先于真函数被 `find` 命中。

### 4.2 T903：看门狗不再 panic，也不再静默死亡

- **挂载目录**：`.expect("文件监控设置失败")` 改为逐目录告警并继续挂其余目录；
  但**一个都没挂上时必须返回错误**——「看门狗在跑却什么都不监控」比 panic 更难发现。
- **事件流关闭**：末尾的 `Ok(())` 改为 `Err`。一个本该长驻的看门狗走到那里不是正常终止。
- **三个调用方**：`db_model.rs` 的 `exec_watcher` 改用 `?` 传播；`spawn_exec_watcher`
  在 tokio 任务里改为显式告警（后台任务 panic 只毒死自己且往往无人查看）；
  `lib.rs` 两处不再丢弃 Result。

### 4.3 T901：pdms-io 热路径 dbg! 清理

`increment_manager.rs` 那一处归档点名的 `dbg!` 此前已被改成 `log::debug!`；本轮补上
pdms-io 半边。**没有一律改成 debug，而是按实际严重程度分级**：

| 位置 | 原 | 现 | 理由 |
|---|---|---|---|
| `io.rs` ×2 | `dbg!(&e)` | **`log::error!`** | channel send 失败意味着**这一批 SQL 被直接丢弃** |
| `io.rs` | `dbg!((next_level, level))` | **`log::warn!`** | 索引层级没下降，再递归就是死循环 |
| `io.rs` | `dbg!(&session_numbers.len())` | `log::debug!` | 生产日志里刷屏的那一行 |
| `io.rs` | `dbg!(&range)` | `log::debug!` | 常规诊断 |
| `io.rs` | `dbg!(&index_data)` | `log::trace!` | 逐页转储，量太大 |
| `io.rs` | `if loc.pgno == 0x1564 { dbg!(…) }` | **整段删除** | 写死的魔术页号，某次排查留下的脚手架 |
| `sync/compress.rs` | `dbg!(&temp_file)` | `log::debug!` | — |

`main.rs`、`src/bin/*`、`src/test/*` 里的 `dbg!` **故意保留**：开发用二进制与测试，
不在服务热路径上。

### 4.4 文档：`_p1_section.tmp.md`

不能直接删——里面是一整节**尚未合并**的内容，而 v2 测试计划当时只到第 7 节。

它也不只是「UTF-16」，而是**双重编码损伤**：10,908 字节按 UTF-16LE 解出 5,454 字符，
其中 2,727 个是 NUL，正好一半，每个真字符后面跟一个 NUL。剔掉 NUL 后得到干净的
2,727 字符 Markdown，已作为 **§8 批次 A 执行记录（2026-07-25）** 合并进
`2026-07-25_test-plan-core-dll-model-update-complete-matrix-v2.md`，原文件删除。

### 4.5 collect 双跑

`manual_update::execute_one_dbnum` 收集一次增量窗口，结果在 match 臂末尾被丢掉——
只留下 `merged_sesnos` 和 `changed_elements` 两个标量（DESI 额外多一份单元归并）——
随后 `IncrementPipeline::apply` 内部又把**同一文件、同一窗口**完整解析第二遍。

`apply_one` 自身没问题，它用 `collected: Option<…>` 保证内部只收一次。问题纯粹在跨层：
上层收过了，但没有任何通道把结果传进去。而 `timings.collect` 只统计第二趟，**日志里
印出来的 collect 耗时只有真实成本的一半**。

对非 DESI 库（SYST / CATA / DICT）尤其亏：第一趟的整份结果只换来两个标量。

修法：`apply` 变薄封装，新增 `apply_with_precollected`；`apply_one` 多一个
`precollected` 参数。复用条件是 `range == requested_range` 完全相等——**崩溃重放走的是
持久化的固定区间，可能与 `requested_range` 不同，那种情况永远重新收集**。原签名不变，
`increment_manager.rs` 的自动路径和既有测试一行不用改。

新增 `execute_one_dbnum_collects_the_window_exactly_once` 钉住接线。

> 本节的分析由 **empty-87 会话**完成并以交接文档形式移交（他们发现有另一会话在同一棵
> 脏树上迭代同一路径，主动改为只读）。本轮对当前代码逐条复验后采纳，仅更正了函数名：
> 交接文档写作 `execute_one_batch`，实际是 `execute_one_dbnum`。

### 4.6 `read_index_data` 页缓存

`read_index_data` 原先**零缓存**：每次调用都 `seek` + 读一个 2 KB 页 + 新分配 `Vec` +
重新 Deku 解析。而 B+ 树下降会反复碰同几个上层页，每个 refno 至少下降两次
（latest 一次、prev 一次）。250206 那个窗口有 5148 个 refno——上层索引页被重复解析了
成千上万次。

改为 `HashMap<u32, Arc<IndexPageData>>`，命中时只是引用计数加一。文件在一个 `PdmsIO`
实例的生命周期内只读不写，**不需要失效逻辑**。返回 `Arc` 而非引用，是因为调用方拿到
结果后往往紧接着再调 `&mut self` 的方法（如 `filter_index_data`），返回借用会撞借用检查。

**35 个调用点全部通过 Deref 自动适配，一个都不用改**，也没波及 gen-model：
`io.rs` 13 处、`src/test/test_collect_latest_eles.rs` 22 处。后者虽在 `test/` 目录下，
但 pdms-io 的 `lib.rs` 里是 `pub mod test;`（**没有** `#[cfg(test)]` 门控），
所以它随依赖库一起编译——这也意味着这 35 处在本轮 gen-model 构建里全部被真实检查过。

---

## 5. 性能实测

用 `src/bin/incr_fold_probe.rs`（只读，不连 SurrealDB）在真实工程文件上测：

| dbnum | 类型 | 元素 | 改前单趟 collect | 改后 | 倍率 |
|---|---|---:|---:|---:|---:|
| 8191 | SYST | 1,829 | 42,016 ms | **2,269 ms** | 18.5× |
| 250206 | DICT | 17,006 | 322,186 ms | **4,532 ms** | 71× |

叠加双跑修复（原本每个库跑两遍），2026-07-26 那次 4 批次运行的 collect 阶段：

```
旧：(46.3 + 322.2 + 0.07 + 0.07) × 2 ≈ 737 秒 ≈ 12.3 分钟
新： 2.3 +   4.5 + …              ≈   7 秒
```

**这也修正了归档 P3 第 15 项的优化方向**：那里列的全是 persist 侧改造
（plan/rev_index 两阶段、批量 BFS、参数绑定），但实测 DICT 250206 是
`collect=322186ms` 对 `persist=24802ms`——把 persist 优化到 0 也只省 7%。

### 正确性交叉验证

缓存只应该快，不应该改变结果：

- 8191 改前改后逐项相同：操作总数 1768（Add 1143 / Modified 529 / Deleted 96 /
  None 61）、去重 refno 1143、SQL 体积 0.82 MB、保守折叠 1768→1589 省 179、
  激进折叠 1486 省 282
- 探针算出的「省 179 条」逐字对上 2026-07-26 生产日志的
  `增量窗口折叠：合并同 refno 的连续 Modified，省下 179 条语句（实际落库 1589 条）`
- 250206 的 5148 对上生产日志的 `增量主数据落库完成，共 5148 条`
- 唯一差异是「最热 refno」从 `24575_667` 变成 `24575_542`，两者都是「被写 13 次」，
  是 HashMap 遍历顺序对并列最大值的 tie-break，不是结果差异

### 口径声明

- 8191 的 42,016 ms 是用**补丁前的探针二进制**实测的，与生产日志的 46,308 ms 很接近
  ——这一点证明探针与生产路径可比
- 250206 的 322,186 ms 是**生产日志数字**，未用旧二进制重跑，所以那一行是探针对生产
  而非探针对探针
- 旧二进制建于 07-26 12:56，早于 §4.3 的 `dbg!` 清理。但那几处 `dbg!` 不在 collect
  热路径上（它们旁边的 `println!` 在生产日志里一行没出现），对倍率的贡献可忽略
- 全部是 **debug 构建**。release 绝对值会不同，但缓存命中省掉的是解析工作，比值应该站得住

---

## 6. 尚未做的验证

**端到端重放**（写库操作，未执行）：

1. `empty1/tools/snapshot_db.ps1 <before.txt>` 抓 TOTAL_PE、逐 dbnum pe 行数、水位
2. 把 8042 上 `dbnum_watermark:8191` 的 `applied_sesno` 改回 1
3. 重放
4. `snapshot_db.ps1 <after.txt>`
5. `compare_snapshots.ps1 <before.txt> <after.txt>` 断言逐行相同

`DbnumState::advance_applied` 用的是 `math::max`，只进不退，回退必须绕过应用层直接发
裸 SurrealQL。8042 是隔离实例，不碰 8009 / 8020。

重放一个已应用的窗口顺带验证落库幂等性——如果幂等，快照应当逐行不变。
双跑是否真的消失，数日志里 `collect sesno:` 的行数即可：8191 应从 200 降到 100。

---

## 7. 仍然开放

按现在的判断排序：

1. **跨仓未提交**（P3-12）——今天所有成果都还在工作区
2. **生成几何缺失**——P0-1 与大多数 `live_*` 测试的共同前置；38 个 ignored 测试里
   多数卡在这里
3. **视觉验收零证据**（P1-5）——D 批「数据+模型+视觉」三断言的第三项全线为空
4. **B6**（P2-10）——CATA 派生根仍继承目录库 dbnum，死信只能被 CATA 新会话复活
5. **B4 深度不对称**（P2-11）——需要先定约定：init 降为一层，还是 watch 改递归
6. **`get_refno_operation_status` 的重复下降**——`collect_increment_eles` 循环里已持有
   `loc`（含 offset），却只传 `(refno, sesno)`，后者又从根走一遍。页缓存上了之后这一项
   的边际收益已经小很多
7. 3 条 `A transaction was dropped without being committed or cancelled` 警告
   （`empty1/logs/surreal_8042.log.err`，UTC 12:34 与 14:01×2），未定位代码路径

---

## 8. 协作注记

复核期间 `src/versioned_db/database.rs` 在 01:02 被**本会话之外的写入方**修改
（`PeStatRow.sesno` 改为 `Option<i32>` 并配了反序列化测试）。加上 empty-87 的只读交接，
说明这棵树同时有多方在动。

本轮改动与之**文件不重叠**，但这意味着：**提交前需要先确认其他写入方的状态**，
否则会把别人未完成的改动一并卷入。

本轮碰过的文件：

```
gen-model/src/data_interface/increment_manager.rs   B3 两处调序 + T903 + 守护测试
gen-model/src/data_interface/db_model.rs            T903 调用方
gen-model/src/lib.rs                                T903 调用方
gen-model/src/data_interface/increment_pipeline.rs  apply_with_precollected
gen-model/src/data_interface/manual_update.rs       交出 collected + 守护测试
gen-model/docs/2026-07-25_…matrix-v2.md             合并 §8
gen-model/docs/_p1_section.tmp.md                   已删除（原为 untracked）
pdms-io/src/io.rs                                   dbg! 分级 + 页缓存
pdms-io/src/sync/compress.rs                        dbg! 分级
```
