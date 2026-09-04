# 2026-09-03 old-pdms-io 退役清单与顺序（按 crate API 面清点）

- 起因（用户）：「现在不应该有 old-pdms-io 的使用了」。本文只**清点与排序，不动码**。
- 与既有文档的分工：`2026-09-02-gen-model-increment-cleanup-inventory.md` 按**模块归属**分拣
  （e3d-model 拥有增量逻辑 / gen-model 留队列与存库），spec 035 T401–T411 按**任务**排期。
  两份都只覆盖「增量」那一段。本文换一根轴——**按 `pdms_io` crate 暴露的 API 逐个清点**——
  因为 ADR-056 Q2 问的是「能否把 `pdms_io` 从默认依赖图摘掉」，那是个依赖图问题，
  只有沿 API 面数才数得全。**结果：三组用法既没有替身、也不在任何现有计划里**（§3）。
- 事实基线：2026-09-03 工作树，`rg pdms_io GEN/src/` 共 31 个文件、约 70 处。
    10|
## 0. 一句话与完成判据

历史快照中 `pdms_io` 曾是 direct 数据面的入口；当前目标是将它明确降为 legacy 参照探针。
此前 direct 服务上的 TTY 窗口回执只作为历史证据保留：

```
收集口径：净窗口（会话索引差分，ADR-031/036 单一口径）added=10 deleted=0 modified=1 … 差分 44ms
```

**完成判据**（三条都成立才算「没有 old-pdms-io 的使用」）：

    20|1. `GEN/Cargo.toml` 里 `pdms_io` 变成 `optional = true`，只由 `legacy_pdms_io` / `legacy_session_replay` 打开；
2. `cargo build --lib --bins`（默认 features）不再把 `pdms_io` 拉进依赖图（`cargo tree -e normal | rg pdms_io` 为空）；
3. 一条源码护栏钉住 `src/` 下除 feature 门后的探针外不含 `pdms_io::`。

这段是退役前的历史基线：当时三条**一条都不成立**；当前实现已补上 feature 门和源码护栏，
但 workspace 级 parse_pdms_db/Python 依赖仍待后续阶段收口。`legacy_pdms_io` 这个 feature 在 `Cargo.toml:36` 已经声明，
但 `rg legacy_pdms_io GEN/src/` **零命中**——门是空的，没有任何代码在它后面。

## 0.0 落地状态（2026-09-03 晚，会话 f968e2e4 → c930d44b）

用户拍板「old-pdms-io 设为 legacy，默认不启用、不编译；数据解析都用 e3d-io，模型生成用 e3d-model」，**一次做完**
而不是按 §4 分八个阶段：

| §0 判据 | 状态 | 证据 |
|---|---|---|
| 1 `pdms_io` optional、只由 `legacy_pdms_io` 打开 | ✅ | `Cargo.toml` `legacy_pdms_io = ["dep:pdms_io"]`，`debug_parse` / `legacy_session_replay` 叠在其上 |
| 2 默认依赖图无 `pdms_io` | ✅ | `scripts/Check-LegacyPdmsIo.ps1`：`cargo tree -e normal` 零命中；`--features legacy_pdms_io -i pdms_io` 命中 |
| 3 源码护栏 | ✅ | `data_interface::mod.rs::legacy_pdms_io_guard`（代码行含 `pdms_io::` 的文件必须带 legacy 门或是 legacy `required-features` bin） |

各组落点：A → `e3d_collector.rs`；B → `frozen_source.rs`；C → `e3d_collector::contains_refnos_at`（仍只给老规划器）；
D → `ele_ops.rs`（原样搬入，渲染逐字节不变）；E → `db_file_header.rs`；F → `db_file_registry.rs`（**事实修正**：`PdmsWatcher`
在本仓只是状态壳，e3d-io 侧无 watcher 可写，§7 的设计不再需要）；G → `file_sync/`；H → `legacy_pdms_io` 门后
（**2026-09-04 起 H 整批删除**，门也不再有：`pdms_io` 从工作区退出，`cargo tree --workspace -i pdms_io` 匹配不到任何包；
判据换成 E3D TTY，见 §2.H 下方的撤回说明）。
**跳过的门**：T404 三条硬门与 §8 的 429 库行级对拍没在切换前跑；`shape_conflicts` 阻断、`TypeChanged` 收口、成员仲裁退役
三条按 §8 做了但只有单测 / 源码钉，没有 live evidence。§6 `parse_pdms_db` 原样留下（阶段 8）。详见 `changelog.md` 2026-09-03。

**收口补记（2026-09-04）——最后一条尾巴与一个假绿灯**：上面第 2 条判据当天报绿的时候，`pdms-io.git` 这个仓其实还在
默认依赖图里：`parse_pdms_db → pdmsdb_engine_v2 v0.1.0 (git = https://github.com/happyrust/pdms-io.git?rev=efd2e19e…)`。
`Check-LegacyPdmsIo.ps1` 第 1 条没抓到，是因为它扫的是下划线的 `pdms_io`，而那一行里只有 URL 里的连字符 `pdms-io`
和包名 `pdmsdb_engine_v2`——**两个字符串都不匹配，判据于是在尾巴还挂着的时候报了绿**。2026-09-04 `parse_pdms_db` 整体
退役（见 `2026-09-04-parse-pdms-db-retirement-inventory.md` §0.1）把这条边剪断，`pdmsdb_engine_v2` 随之离开
`Cargo.lock`；同一轮把第 1 条的判据改成同时认连字符 `pdms-io`，免得下次换个包名再钻同一个洞。
教训按宪法 III 记一条：**「某某已整体退出」这句话的判据必须扫仓库来源，不能只扫包名**。

## 0.1 已拍板（2026-09-03，用户）

1. **watcher 在 e3d-io 侧新写一个**（不是搬 `PdmsWatcher`，也不是承认它永久留下）。设计见 §7。
2. **全面采用 e3d-io：数据的解析与存储也接入 e3d-io 的解析结果。** 即 `pe` / `ATT_*` 的写回
   不再由 old-pdms-io 的 `EleOperationData` 差分喂，改吃 e3d-io 的 `DbElement` →
   `direct_attmap::NamedAttrMap` 终态。设计见 §8。

第 2 条把目标从「摘掉 `pdms_io`」抬成「**旧解析栈整体退役**」——`parse_pdms_db` 因此**进入范围**
（§6 已按此改写）。完成判据 §0 的三条对 `parse_pdms_db` 同样适用，只是排在 `pdms_io` 之后。

## 1. 依赖现状

| 项 | 值 |
|---|---|
| 声明 | `Cargo.toml:159` `pdms_io = { git = "…/old-pdms-io.git", rev = "14e2836c" }`，**非 optional、无 feature 门** |
    30|| patch | `Cargo.toml:262` → `path = "../vendor/old-pdms-io"`（LOCAL-DEPS 三段当前处于打开状态，pre-push 会挡） |
| 相关 feature | `legacy_pdms_io = []`（空门，无消费者）、`debug_parse = ["pdms_io/debug_parse"]`、`legacy_session_replay = ["pdms_io/legacy_session_replay"]` |
| 兄弟遗留 crate | `parse_pdms_db`（`old-parse-pdms-db.git`，31 个文件）——**不随 old-pdms-io 退役**，见 §6 |

## 2. 按 API 面清点

### A. 增量窗口收集 —— 生产主路，替身已有，是全部后续的解锁条件

| API | 调用点 | 替身 |
|---|---|---|
| `net_window::collect_net_window` | `increment_pipeline.rs:1037`（`IncrementPipeline::collect_window`） | e3d-io `IndexDiff(base@S, target@T)` + e3d-model `collect_window` / `ChangeLedger` |
| `net_window::NetWindowOutcome` | `:503`（`net_caliber_warning`）、`:2288/:2297`（测试） | `IncrementReport` / `ChangeTally` |
    40|| `session_index_diff::{collect_net_changes, NetChangeStats}` | `:2600`（测试） | `e3d_io::index::diff` |

落点：spec 035 **T401**（影子收集器）→ **T403**（切换）→ **T404**（硬门）。
硬门原文：ams7999 45→46 出 22 Add / 0 Delete；ams1112 721→722 能收集并出 24673 Delete；429 库行级对拍零差。
这两个窗口不是随便挑的——它们是 `2026-09-02-planner-parity.md` §3/§4 坐实老收集器**幻删 2 个活元素、
漏掉 22 个新建元素、整窗断言失败**的现场。

### B. 冻结快照与身份门 —— 一半要留，先拆再删

| API | 调用点 | 性质 |
|---|---|---|
| `snapshot::SnapshotToken` | `increment_pipeline.rs:42`、`manual_update.rs:2813`（结构体字段） | 载体 |
    50|| `snapshot::DabaconSnapshot::open_verified` | `increment_pipeline.rs:789`（复核函数内）、`:1330`（尾事务前） | **commit-generation gate，语义要留** |
| `snapshot::DabaconSnapshot::open_verified_at` | `:1031`、`window_repair.rs:168` | 同上 |
| `snapshot::DabaconSnapshot::open / open_at` | `:1033`、`:2684`(测试)、`batch_worker.rs:2607`、`versioned_db/database.rs:1329` | 打开 |

替身：`direct_store.rs` 已有 `FileIdentity`（len + mtime）与 `verify_file_identity()`——**但它没有任何
生产调用点**（`rg verify_file_identity` 只有定义与自调用三处）。所以这一组不是「有替身待切换」，
是「替身写好了没接线」。落点：**T407** 先拆成两个有名字的数据步（`verify_frozen_source` 保留 /
存活复核删除），拆完顺手把 `verify_file_identity` 接上生产路。

### C. 计划目标 T 端存活复核 —— 现在就能删，而且是个硬失败向量

    60|| API | 调用点 |
|---|---|
| `snapshot::DabaconSnapshot::contains_refnos_at` | `increment_pipeline.rs:825`（`reconcile_plan_final_presence`） |

它拿**老读法**去否决**新规划器**已经在 `DbSet@T` 上读过一遍的生成根。上一份审核（fable-5-1-6，F1）
在 ams7999 实测：`contains_refnos_at(46, {24383/101198})` 报 `IndexPageData.noun == 0xCC47DF`，
而这个 refno 正是 e3d-io 在 `DbSet@46` 上刚读到的新建 STRU 子树。后果链：整窗 `Err` → 批次
Failed → `prepare_attempt` 行已落 → 下次进 replay 分支按固定区间重放同一份计划 → 同一页再报错
→ **该库水位永久卡死**，只能手删 attempt 行。

对窗口规划器路径它是**纯多余**的：`roots_T` 本来就是从 `DbSet@T` 枚举出来的，自身即 T 端存活证明。
    70|落点：**T407** 的一半，是全清单里唯一可以脱离 T401 单独做的一条。

### D. 数据模型类型 —— 面最广，随收集器一起换

`io::{EleOperationData, EleOperationDetail, ModifiedElement}`：

| 分组 | 文件 |
|---|---|
| 生产写回 | `increment_pipeline.rs:17`、`model_update_plan.rs:6/623`、`manual_update.rs:54/7951`、`model_impact.rs:21`、`increment_manager.rs:28/4512`、`window_repair.rs:14` |
| 暂存层（P3 整体删） | `staging/lifecycle.rs:69/235/398`、`staging/write_context.rs:21/57/160/256`、`staging/parity.rs:134/307` |
| 测试夹具 | `issue10_direct_add_node.rs:26`、`direct_window_replay_parity.rs:24`、`room_fixture.rs:454/601`、`room_live_issue7.rs:743` |
| 探针 bin | `increment_planner_parity.rs:57`、`incr_fold_probe.rs:18`、`db8000_copy_probe.rs:5` |
    80|
注意 `render_persist_statements` 不是「同一份渲染换输入」：`Modified` 今天是**差分 MERGE**，
换成双端比较后要改**终态 MERGE**（不许 `CONTENT`），映射表还要补 `TypeChanged`（refno 复用时
旧 noun 的 `ATT_` 行今天就残留）。细节见 09-02 清单 §6.1。落点：**T406**。

### E. 文件头与扫描 —— 小、独立，可以提前做

| API | 调用点 | 替身 |
|---|---|---|
| `defines::DbPageBasicInfo` | `increment_pipeline.rs:16`（`apply_one` 签名）、`increment_manager.rs:27`、`sesno_range.rs:19` | `e3d_io::meta::{DbDescriptor, FileInfo}` |
| `io::PdmsIO`（trait / `new`） | `increment_pipeline.rs:17`、`increment_manager.rs:28`、`sesno_range.rs:20`、`versioned_db/database.rs:18`、`historical_model.rs:389`、`l3_suite/fixture.rs:4`、`manual_scan_probe.rs:3`、`db8000_two_delete_fixture.rs:14`、`legacy_v2_read_parity.rs:39` | `e3d_io::ReadOnlyEngine` + `session` 模块 |

    90|`DbPageBasicInfo` 穿在 `apply_one` 的签名上，换它会碰整条编排链——建议**单独一个 PR**，
不要和 T401 混在一起评审。**新增任务**（现有 spec 无对应号）。

### F. ★ 文件监听 —— 没有替身，而且 ADR-058 D8 正在扩大它

| API | 调用点 |
|---|---|
| `watch::PdmsWatcher` | `db_model.rs:24/122/226/540`（`AiosDBManager` 持有，**生产构造点在 `:540`**）、`increment_manager.rs:29` + 五处测试夹具、`tidb_manager.rs:28/54`（结构体字段）、`project_paths.rs:498`（注释引用 `watch_dirs`） |

**e3d-io 没有 watcher，仓里也没有第二个实现。** 而 ADR-058 D8 刚刚（2026-09-03 已接受）把
`sync_live = configured && !direct_read_mode` 判掉，结论原文是「启动全量扫一次，**之后 watcher
   100|一直盯着自动追**」——也就是说 watcher 从「direct 下不起」变成「一直起」。

所以这一组的走向和其它组相反：**old-pdms-io 在这里的份量正在变重，而没有任何计划要替换它。**

**已拍板：在 e3d-io 侧新写一个**（§0.1 第 1 条）。设计与依赖代价见 §7。

### G. ★ 项目同步与发布 —— 没有替身，不在任何计划里，且与增量无关
   110|
| API | 调用点 | 干什么 |
|---|---|---|
| `sync::clone::{CloneOptions, execute_clone}` | `db_model.rs:22/158-159` | 从远端增量 clone E3D 库文件（MQTT 驱动的 `exec_delta_clone_remotes`） |
| `sync::compress::{CompressOptions, execute_compress}` | `sync_publisher.rs:10/61-62` | 把库文件压成 `.cba` 归档发布 |

这两个跟增量、跟 dabacon 解析都没关系，是**文件分发功能**。e3d-io 不提供、也不该提供。
现有三份计划（ADR-056 Q2、spec 035 P4、09-02 清单）**一处都没提到它们**。
处置同 F：搬进 gen-model（或独立 crate），否则完成判据不成立。**新增任务**。

### H. 探针 / 夹具 / 测试 —— 最后一批，且本来就该在 feature 门后

   120|`increment_planner_parity`（T207 要求它常驻 CI，直到 N6 收口）、`legacy_v2_read_parity`、
`incr_fold_probe`、`db8000_copy_probe`、`db8000_two_delete_fixture`、`manual_scan_probe`、
`l3_suite/fixture`、`historical_model`、`room_fixture`、`room_live_issue7`、`direct_window_replay_parity`、
`issue10_direct_add_node`。

其中 `incr_fold_probe` / `db8000_copy_probe` 已在 `legacy_session_replay` 后面（`Cargo.toml:60/65`），
其余**都在默认 features 里**。这一批的动作是「挪到 `legacy_pdms_io` 门后」而不是删——
对拍尺子要活到被对拍的两边合一为止。

> **2026-09-04 撤回上面这句**（用户 09-03 23:57 拍板：「对拍要以 e3d tty 的数据为准。删除和 pdms-io 的对拍。」）。
> old-pdms-io 不再是任何对拍的参照臂——`legacy_pdms_io` / `legacy_session_replay` / `debug_parse` 三个 feature、
> `pdms_io` 依赖与 `[patch]`、`legacy_v2_read_parity` / `incr_fold_probe` / `db8000_copy_probe`、
> `increment_planner_parity --probe`、`ele_ops::legacy`、`collect_changes`、`db8000_session_pairs` 性质 h / i、
> aios-py 的 legacy 绑定全部删除；护栏改成「manifest 与源码一处都不许有 `pdms_io`」。
> **判据从此只有 E3D TTY**：TTY 宏录下的会话链（`tests/db8000_session_pairs` 夹具、`tests/fixtures/issues/issue-019-*`、
> `scripts/e3d/Record-Db8000SessionChain.ps1`）、TTY 现场往返（`scripts/e3d/Test-TtyNetWindow.ps1`、
> `scripts/e3d/verify_db8000_bran_window.ps1`）与 TTY 只读点查（`direct_tree_tty_compare --tty`、`Q ATT` / `Q MEMBERS`）。
> `shadow_collector_parity` 的 e3d-model L2 臂是**两条 e3d-io 合成路径互查**，不是判据。详见 `changelog.md` 2026-09-04。

## 3. 三条既有清单没覆盖的（本文的净增量）

   130|1. **watcher（§F）**：唯一实现在 old-pdms-io，而 ADR-058 D8 刚把它从「direct 下不起」改成「一直起」。
   这是「摘掉默认依赖」最大的单点阻塞，而且方向与退役相反。
2. **clone / compress（§G）**：两个与增量无关的功能挂在同一个 crate 上，三份计划零提及。
3. **`legacy_pdms_io` 是个空门**：feature 声明了，`src/` 里零消费者。T403 写的「只留 feature 后探针」
   今天进度是 0%，不是「差最后一步」。

## 4. 顺序

按「能不能脱离 T401 独立做」排，不是按代码量排。

| # | 阶段 | 内容 | 前置 | 验收门 |
   140||---|---|---|---|---|
| 0 | **拆掉硬失败向量**（**T412**） | §C：窗口规划器路径不跑 T 端存活复核，只留身份门 | 无 | ams7999 45→46 收得完、水位到 46、STRU 子树根进 pending |
| 1 | **e3d-io watcher**（**T413**） | §7：`e3d-io` 新增 `watch` 模块（可选 feature），gen-model 三处换过去 | 无 | 与 `PdmsWatcher` 同目录、同事件序列对拍；`db_model.rs:540` 换掉后启动重扫逐条不变 |
| 2 | **换头文件读法**（**T414**） | §E `DbPageBasicInfo` / `PdmsIO` → e3d-io `meta` / `ReadOnlyEngine` | 无（可与 0/1 并行） | 扫描、定窗口、历史投影三处行为不变；单独 PR |
| 3 | **搬走 clone / compress**（**T415**） | §G 两个文件分发功能搬出 `pdms_io` | 无 | 编译过；MQTT 同步与 `.cba` 发布行为不变 |
| 4 | **收集器换底座** | §A T401 影子 → T403 切换 | P1 全部 | T404 三条硬门 + 429 库行级零差 |
| 5 | **数据面接 e3d-io 解析结果** | §8：`render_persist_statements` 换吃 `NamedAttrMap` 终态；T406 | 阶段 4 | §8 的五条门 |
| 6 | **拆身份门 + 清 B 桶** | §B T407 拆两步、接 `verify_file_identity`；T405/T408/T409 按 oracle 零差逐个删 | 阶段 5 | `increment_planner_parity` 各桶零差后再删 |
| 7 | **落门** | §H 探针挪到 `legacy_pdms_io` 后；`pdms_io` 改 `optional = true`；加依赖图护栏 | 阶段 0–6 | §0 三条完成判据 |
| 8 | **`parse_pdms_db`** | 第二张清单（§6） | 阶段 7 | 同 §0 三条，主语换成 `parse_pdms_db` |

阶段 0 / 1 / 2 / 3 互相独立，也都不依赖 T401，**可以现在就并行开**；真正的长杆是阶段 4 → 5。

## 5. 陷阱（删之前先看）
   150|
- **不要把 §B 整块删**：`open_verified` 是数据面的 commit-generation gate，住在一个模型名字的函数里
  （`reconcile_plan_final_presence`）。它挡的是「收集与持久化之间文件被换掉 / 回拨」，
  纯数据窗口（零模型候选）也必须验。
- **`propagate_deletes_to_descendants` 是存库事实，不是预览专属**：属主被删要清后代的持久行。
  换双端比较后传播自然消失，但存库侧要改成消费 `ledger.deleted` 全集。
- **回执计数口径会变**：老收集器数的是「窗口内操作数」，双端比较数的是「窗口净变化数」——
  窗口内建了又删的元素从此不出现。T174 的「与 before 逐字段一致」在阶段 3 之后必然红，
  要在 evidence 里重定基线，别当回归查。
- **`TypeChanged` 今天就漏**：refno 复用（删掉再用同一号建别的 noun）时旧 noun 的 `ATT_<old>:{id}`
   160|  行永久残留。换源时顺手关，别只做映射。

## 6. `parse_pdms_db`：随「全面采用 e3d-io」进入范围，但排在最后

`parse_pdms_db`（`old-parse-pdms-db.git`，`Cargo.toml:155`，patch 到 `../vendor/old-parse-pdms-db`）
是**另一个**遗留 crate，`rg parse_pdms_db GEN/src/` 命中 **31 个文件**——比 `pdms_io` 还广，
且分布在与增量无关的地方：`mdb_membership.rs`、`extract_family.rs`、`uda_table.rs`、`pcf/bran.rs`、
`versioned_db/database.rs`、`cata_closure.rs`、`fast_model/shared.rs` 以及一大批 `bin/*probe`。

`pdms_io` 自己也依赖它，所以**把 `pdms_io` 摘掉不会让 `parse_pdms_db` 跟着走**。

按 §0.1 第 2 条「全面采用 e3d-io」，它在范围内，但**排在阶段 8**、要单独一张清单，理由有三：

1. 它的用法性质与 `pdms_io` 不同。`pdms_io` 是「窗口收集 + 快照」，一条主路；`parse_pdms_db`
   是**共享 schema 与类型层**——`get_default_pdms_db_info()` / `named_attr_info_map` / `NounHash` /
   `DbBasicInfo` 这些被 `mdb_membership`、`extract_family`、`uda_table`、`pcf/bran`、`cata_closure`
   以及 `direct_attmap` 自己消费。**连 e3d-io 的替身 `to_named_attmap` 都在用它定形**
   （`direct_attmap.rs:135` `aios_core::get_default_pdms_db_info()`）。
2. 因此阶段 5 完成时 `parse_pdms_db` 不减反增——数据面换成 e3d-io 之后，仍然要靠它那份 schema
   把属性名对上。先动它会把阶段 4/5 的对拍基准一起换掉，两个变量同时动。
3. 它退役的前置是「schema 的权威搬到哪儿」（e3d-attlib？e3d-io `noun_catalog`？还是留在 aios_core），
   那是一条独立的 ADR，不该塞进本清单。

## 7. e3d-io watcher 设计（阶段 1）

`PdmsWatcher` 的真身很薄（`old-pdms-io/src/watch.rs`，167 行，其中约 40 行是注释掉的 `.cba` 死代码）：

| 成员 | 作用 | 迁移 |
|---|---|---|
| `watch_dirs: Vec<PathBuf>` | 监控目录清单 | 原样 |
| `headers: DashMap<PathBuf, DbPageBasicInfo>` | 文件头缓存 | **不迁**，见下 |
| `file_name_full_path_map: DashMap<String, PathBuf>` | 文件名 → 全路径 | 原样 |
| `async_watcher()` | `notify::RecommendedWatcher` + `futures` mpsc | 原样换 e3d-io 侧 |
| `init_local_watcher()` | `WalkDir` 遍历 + 逐文件读头 | 拆两半 |
| `scan_db_headers(paths)` | 批量读头 | 拆两半；注意 `increment_manager.rs:1698` 有一条源码钉**禁止**再长出它的消费路径 |
| `get_dbno(path)` / `save()` | 缓存查询 / 存 `watcher.json` | 原样 |

**两条设计纪律：**

1. **按 `CONTEXT.md`「文件观察」拆层。** 词条原文：文件观察「只陈述文件层面看得见的东西，
   **不含库号、库类型与最新会话号**——那三样都要读文件头，是分析层的裁决」。`PdmsWatcher`
   把 `headers` 缓存塞在收集层，正是该词条要避免的形状。新 watcher 只产 `FileObservation`
   `{path, modified, len, observed_at, source}`；读头归分析层（e3d-io `meta::FileInfo` /
   `DbDescriptor`，阶段 2 的产物）。这样 §E 与 §F 合成一件事，不是两件。
2. **放在可选 feature 后面。** e3d-io 今天的依赖是 `thiserror / nom / e3d-attlib / serde_json /
   sha2 / encoding_rs`——没有 `notify`、没有 `tokio`、没有 `dashmap`。它的自我描述是
   「Standalone **read-only IO** library」。watcher 要拉进 `notify`（带三套平台后端）与一个
   channel 运行时，会改变这个 crate 的性质。做法：`e3d-io` 新增 `watch` feature（默认关），
   `e3d_io::watch` 模块在门后；gen-model 打开它。这样别的只读消费者（探针、`gen_ams`）不受影响。

**验收：** 同一组目录下与 `PdmsWatcher` 并行跑一段，事件序列（路径、顺序、去重后的观察集）
逐条相同；`db_model.rs:540` 换掉之后启动重扫的候选库判定、重复 dbnum 裁决、`watch_dirs`
解析结果逐条不变（`increment_manager.rs` 那五个夹具测试是现成的门）。

## 8. 数据面接 e3d-io 解析结果（阶段 5）

今天：`render_persist_statements(range_eles: &BTreeMap<u32, Vec<EleOperationData>>, dbnum)`
（`increment_pipeline.rs:1600`）→ `fold_window` 折叠 → 逐条 `to_surql` / `to_modify_surql`。
`Modified` 走的是**差分 MERGE**（只写改过的字段 + 被删字段置 null），输入是
`ModifiedElement{added/modified/deleted_*_attrs}`。

换成 e3d-io 之后输入是 `DbElement@T` → `direct_attmap::to_named_attmap()` → `DirectAttrs`。
五件事必须一起做，少一件就是静默写错：

1. **改终态 MERGE，不许 `CONTENT`。** `ATT_<noun>:{id} MERGE {T 时刻全部属性;
   base 有而 target 无的名字 → null}`。行从此自描述、固定区间重放天然幂等。
   不许整行替换：`pe` / `ATT_*` 上有别的写者拥有的字段（`pe.deleted` / `pe.sesno` /
   `datacenter_version`），`pe` 行继续只动 `name / sesno / deleted`。
2. **补 `TypeChanged`。** 双端比较能看见「删掉再用同一 refno 建一个别的 noun」（issue #27 refno 复用），
   e3d-model 报 `ChangeKind::TypeChanged`。今天这条路是 `Deleted` + `Add` 两条语句凑的，
   **旧 noun 的 `ATT_<old>:{id}` 行今天就永久残留**。换源时渲染成 `Add` 语义 + 显式
   `DELETE ATT_<old_noun>:{id}`。这是今天就存在的漏洞，顺手关。
3. **`DirectAttrs` 的三个回执列表要升级成阻断条件。** `to_named_attmap` 现在把
   `outside_schema` / `unset` / `shape_conflicts` 交出来给调用方判，而今天的调用方只有探针与
   provider——它们的处置是记一笔。**数据面不能这么处置**：`shape_conflicts` 的语义是
   「文件里的存储形状与 schema 声明的数值/引用类型对不上」，已知一例是 `BANG`
   （e3d-io 解成 `Word`，真库上 `4294958296` 应是 −90.0）。按错的读法解出来的数字
   **长得和对的一模一样**，写进 `ATT_*` 之后没有任何东西会报。所以：
   `shape_conflicts` 非空 → **窗口阻断**（水位不动、告警常驻），不是 warning。
4. **`fold_*` 三个函数随之删除**（T406）：双端比较给的就是净状态，没有中间态可折。
   `fold_attr_namespace` 的「删掉的属性 → null」半边由第 1 条的「base 有 target 无 → null」接住。
5. **回执计数重定基线。** 老收集器数的是「窗口内操作数」（`Add→Deleted` 各算一次），
   双端比较数的是「窗口净变化数」（窗口内建了又删的元素根本不出现）。
   `DataBatchResult.added/modified/deleted` 口径因此改变，T174 的「与 before 逐字段一致」
   必然红——要在 evidence 里重定基线并写进回执契约，别当回归查。

**门：** 上面五条各一条测试；加 429 库全量基线的**行级**对拍（同一库、同一窗口，两条渲染路径
产出的 `pe` / `ATT_*` 终态逐字段相同）；`shape_conflicts` 注入一个假样本能把窗口拦住。
