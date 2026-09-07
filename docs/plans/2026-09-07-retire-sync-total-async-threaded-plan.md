# 2026-09-07 开发方案：`sync_total_async_threaded` 真退役——基线摄入只剩一个按文件的入口

- 承接：`2026-09-04-parse-pdms-db-retirement-inventory.md` §3.2 路 (a)（B8「⏳ 长杆」）、
  `2026-09-04-legacy-parser-retirement-endgame-plan.md` B8.4（当时拍的是「签名不变、六个调用点一个不动」）、
  ADR-058 D3/D4/D8、ADR-064 D4（基线与增量共用一个渲染器）、共识 d-29。
- 用户 2026-09-07 原话：「真退役」（fable-5-1-40 会话，承接同日「为什么还在用 sync_total_async_threaded」的答复）。
- **与正在进行的 ADR-058 D8 工作的关系**：同一棵工作树里另一条会话正在把 `sync_pdms` 与 `total_sync / incr_sync /
  only_sync_sys / is_sync_history` 四个开关退役（未提交，14:03–14:07 仍在改 `database.rs / baseline.rs / lib.rs /
  manual_update.rs / sync_sys_only.rs / 全部 DbOption*.toml`），并新增 `database::bootstrap_sys_metadata`——它**仍以
  `sync_total_async_threaded` 为底座**（SYS 相 + DICT 相各调一次）。本方案**排在 D8 提交之后**动手（§4 P0），
  不与它在同一批文件上并行改。

## 0. 一句话

`sync_total_async_threaded` 今天只剩壳：解析器（B8.2/B8.3）、渲染器（ADR-064 D4）、执行器（`surreal_retry`）、
CATA 闭包（`cata_dependency`）、统计（`dbnum_stats`）都已是与增量共用的那一份，壳里留下的是 legacy 全量同步的
编排形状（多类型 / 多文件 / DashSet 去重 / `included_db_files` 过滤 / `is_replace` / MySQL 分支 / 进度参数）。
本方案把壳换成一个**按单个文件**的基线摄入入口，`sync_total_async_threaded` 连同它的消息类型、写者常量、收尾函数
一起删除，`bootstrap_sys_metadata` 改为逐文件调新入口。数据形状、语句形状、事务切块、单写者、失败清库、
水位收口**一律不变**——这是编排层重构，不是数据面变更，所以不需要再过 T404 那种对拍门（见 §5）。

## 1. 现状盘点（D8 落地后的形状）

| 调用点 | 现状 | 本方案 |
|---|---|---|
| `manual_update.rs::initialize_dbnum_baseline` 叶子基线 | `baseline_sync_options` 绑 `replace_dbs=false / included_db_files=[本文件] / manual_db_nums=[dbnum] / gen_model=false / gen_mesh=false` 后调 `sync_total_async_threaded(&options, project, DashSet::new(), &[db_type], 100)`，再从 `HashMap` 里 `.get(&dbnum)` | 直接 `baseline::ingest_file(project, &path, dbnum, db_type, Some(file_latest_sesno))` |
| 同函数的父层补缺（`parent_gap_refno_count > 0`） | 同上，对母库文件再来一次 | `ingest_file(project, &parent, dbnum, db_type, None)`（目标会话取母库最新） |
| `database::bootstrap_sys_metadata`（D8 新增；`bin/sync_sys_only`、l3 套件空店引导） | SYS 相：`&["SYST","COMM","MISC","GLB","GLOB"]` 全目录扫；DICT 相：按 `sys_metadata_files(..,"DICT")` 的清单逐项目 | `baseline::ingest_sys_metadata(db_option)`：同两步，内部按文件调 `ingest_file` |
| `versioned_db/baseline.rs::a_reparse_lands_exactly_one_site_per_name`（`#[ignore]` live） | 直接调 `sync_total_async_threaded` | 改调 `ingest_file` |
| 源码钉 `manual_update.rs` 「`baseline_has_uncommitted_rows` 在 `sync_total_async_threaded(` 之前」 | 字面找旧名 | 改找 `ingest_file(` |
| `data_interface/mod.rs::the_baseline_orchestration_has_no_legacy_parser_intermediates` | 扫 `database.rs` | 改扫 `baseline_ingest.rs`（或整个 `versioned_db/`） |
| `docs/diagrams/data-parse-pipeline.drawio`（两页已标 stale） | 画的是壳 | 第 2 页按新入口重画，或删掉 stale 页 |

`sync_total_async_threaded` 体内今天**没有**任何一处是它独有的能力，逐段对照：

| 壳里的段 | 去向 |
|---|---|
| `resolve_project_root` + `collect_project_db_files` + `included_db_files` 过滤 + `is_parse_sys` 例外（ADR-007） | 叶子 / 父层路径调用方**已经知道文件**，不再扫目录；`ingest_sys_metadata` 保留一次目录扫（搬到新模块，函数与三条测试一起搬） |
| `is_replace → check_and_clear_db` | 基线永远 `replace_dbs=false`，删；回退重建由 worker 的 `wipe_dbnum_for_reinit` 负责（ADR-021），`check_and_clear_db` 的唯一调用方消失后一并删 |
| `dbno_set` 去重 | 单文件入口没有「重复 dbnum」；改成**显式校验**：冻结源反哈希出的 `dbnum / db_type` 必须等于调用方给的，不等就 bail（今天是静默按文件为准） |
| `sesno == 0 → continue`（调用方随后报「未返回目标文件结果」） | 显式 `bail!("文件没有会话，基线不执行")`——同一条失败，措辞直接 |
| `debug_root_refnos` → `is_debug` → 不落库只 `dbg!` | 删。它是 legacy 全量同步的调试钩子；基线不落库等于必然失败（`count == 0`）。要看某个 refno 的属性用 `bin/direct_attmap_probe` |
| `is_save_db()` 为 false → 解析不落库 | 入口顶部 `bail!("save_db=false 下不做基线")`，不再让它以「基线完成后仍没有 PE 数据」的形状失败 |
| `UdaKeyMap::load()` 每文件一次 | 保留 |
| `spawn_blocking(stream_baseline → render_baseline_batches → sender.send)` + `flume::bounded(100)` + 单写者（`BASELINE_WRITE_WORKERS=1`，`chunks(20)`）+ `dbnum_stats::execute_on` | **保留形状**（解析与写入重叠，5525 实测写者是瓶颈：排空 7.4s > 解析 4.25s），搬进新模块；`BASELINE_WRITE_WINDOW < BASELINE_QUEUE_CAPACITY` 那条测试跟着搬 |
| `SenderJsonsData::{ElementBatch, MysqlSql}` | 通道元素改成 `String`（一条消息 = 一笔事务 SQL）；`MysqlSql` 与 `#[cfg(feature = "sql")]` 的 `pools_clone` / `conn.execute` 分支删（`save_pes_mysql` 早已不在，这段代码没有生产者） |
| CATA 依赖闭包（seed 流式攒 → 文件收尾一次 `resolve_dependency_rows` → `render_dependency_transactions(Fresh)` → 同一条队列 → `render_dependency_state`） | 原样搬；缺失只 warning、解析器出错算基线失败的纪律不变 |
| `parse_error::note_element_failure / note_attrs_success / flush` | 原样搬（`flush` 仍排在 `?` 之前） |
| `finish_write_pipeline`（先关 sender、排空写者、再裁决） + `record_write_error` | 搬进新模块并收窄成单 dbnum：写者错误 → `wipe_dbnum_for_reinit(dbnum)` → Err；三条 `finish_write_pipeline_*` 测试跟着搬 |
| `baseline_cleanup_targets`（失败 + 「pipeline 失败则全部已调度」） | 单文件后只剩一个 dbnum，函数删，其测试改成「解析失败 / 写者失败 / 闭包失败三种都清了库」 |
| `settle_dbnum_info_after_baseline`（D8 改名）→ `dbnum_stats::initialize_on` | 搬进新模块并改成 **`rebuild_on`（无条件）**。理由：基线自 `1eefbd57` 起走 `StatisticsMode::Deferred`，事务里不再维护统计；`initialize_on` 只在 `dbnum_stats_state.version != 1` 时重建，父层补缺的第二次摄入（同一 dbnum、version 已是 1）会跳过重建，`pe_count != info_count` 直接报 `statistics mismatch`，且该错误在清库判断之后，不触发清库——下一轮 `baseline_has_uncommitted_rows` 清了重来照样死在同一处（mem-201）。单测 `baseline_and_parent_gap_replay_keep_actual_pe_counts` 用的就是 `rebuild_on`，本方案让生产与测试走同一个函数 |
| `stamp_dbnum_info_identity_sql` / `dbnum_stat_totals` | 搬（`pub(crate)`），`rebuild_dbnum_info_from_pe` 继续用 |
| `retire_dbnum_event()` 每次调 | 保留一次（老库摘 legacy 事件；`execute_on` 里的 `require_no_event_on` 是硬闸，这一步是让老库能过闸） |
| `is_retryable_surreal_write_error` / `conflict_retry_backoff` 在 `database.rs` 的副本 | 删，`surreal_retry` 里有同名同体的那份；`database.rs` 两条对应测试删（`surreal_retry` 已有） |
| `proj_progress_chunk` 参数、`dbg!(&project_dir)`、`//todo 按照文件大小排序` 等 | 删 |

## 2. 目标形状

新模块 `src/versioned_db/baseline_ingest.rs`（`baseline.rs` 继续只做「渲染半边」，两边职责不混）：

```rust
/// 一个库文件的基线摄入回执。
pub struct BaselineIngest {
    pub dbnum: u32,
    pub db_type: String,
    pub file_name: String,
    /// 交给写者的元素数，不含 WORL（`manual_update::baseline_parse_matches` 的口径）。
    pub elements: usize,
    pub stats: e3d_collector::BaselineStats,
    /// CATA 依赖闭包的账（DESI 才有）。
    pub dependency: Option<DependencySummary>,
}

/// 把一个库文件按 `Fresh` 整库写进当前 SurrealDB：流式收集 → 每块渲成若干笔事务 → 有界队列 →
/// 单写者 → 文件收尾 CATA 闭包 → 排空 → 统计重算。**不动水位**（`finalize_baseline` 是调用方的事）。
/// 任何失败（解析 / 写者 / 闭包 / 统计对不上）先 `wipe_dbnum_for_reinit(dbnum)` 再 Err。
pub async fn ingest_file(
    project: &str,
    path: &Path,
    dbnum: u32,
    db_type: &str,
    target_sesno: Option<u32>,   // Some = 冻结在这一会话（叶子基线传 file_latest_sesno）；None = 文件最新（父层补缺）
) -> anyhow::Result<BaselineIngest>;

/// D8 的 `bootstrap_sys_metadata` 的底座：SYS 五类全目录扫 + DICT 按清单，逐文件 `ingest_file`。
pub async fn ingest_sys_metadata(db_option: &DbOption) -> anyhow::Result<Vec<BaselineIngest>>;
```

不变量（进源码钉）：

- **I1** 生产代码里基线元素行只从 `ingest_file` 一处出去（`rg 'stream_baseline\(' src` 只命中 `baseline_ingest.rs` 与测试）。
- **I2** `ingest_file` 全文不含 `total_sync / included_db_files / manual_db_nums / replace_dbs / DashSet / cfg(feature = "sql")`。
- **I3** 冻结源的 `dbnum / db_type` 与入参不等 → Err，不落一行。
- **I4** 失败路径三种（解析、写者、闭包）都以 `wipe_dbnum_for_reinit` 收尾（把 `baseline_failure_registers_the_dbnum_and_wipes_only_after_writers_finish` 改写成三态）。
- **I5** 统计收尾是无条件重建：`ingest_file` 全文含 `rebuild_on(`、不含 `initialize_on(`。

`initialize_dbnum_baseline` 的改动只有三处：两次调用换入口（不再造 `baseline_sync_options`，该函数与其测试删）、
`parsed_count` 直接取回执的 `elements`（不再 `HashMap.get(&dbnum).ok_or_else(...)`——「未返回目标文件结果」这条
失败被 I3 与「文件没有会话」两条更直接的失败取代）。`baseline_parse_matches / baseline_parse_confirmed_empty /
baseline_counts / finalize_baseline / admit_baseline` 一个字不动。

**顺手一起做（可选，用户拍）**：F2——`roots == 0` 的基线在 `finalize_baseline` 之后 `stamp_model_watermark_on`，与增量路
`finalize_attempt_on` 的第二触发点对齐（今天零根库 `model_sesno` 永远 null，启动对账因 `gen_root` 无行也补不上，
`dbnum_consistency` 一直 `lagging`；现场 5521/5523/5524/5526/7027 与 pass2 的 5522 都是这样）。它与本方案不在同一个
函数里，放在同一提交只是省一次真库验证。

## 3. 删除清单（`database.rs`）

`sync_total_async_threaded`、`SenderJsonsData`、`BASELINE_QUEUE_CAPACITY / BASELINE_WRITE_WINDOW / BASELINE_WRITE_WORKERS`
（搬）、`finish_write_pipeline`（搬）、`baseline_cleanup_targets`、`record_write_error`（搬）、`check_and_clear_db`、
`collect_project_db_files`（搬）、`settle_dbnum_info_after_baseline`（搬并改 `rebuild_on`）、`is_retryable_surreal_write_error` /
`conflict_retry_backoff`（用 `surreal_retry` 的）、测试 `test_threads`（空转）、`surreal_write_conflicts_are_retryable_but_syntax_errors_are_not`
（`surreal_retry` 已有）、`conflict_retry_backoff_grows_exponentially_and_caps`（同）。`bootstrap_sys_metadata` 改为一行委托
`baseline_ingest::ingest_sys_metadata`。`database.rs` 剩下的是 `rebuild_dbnum_info_from_pe`、`retire_dbnum_event`、
MySQL 的 `create_*_database`（feature 门后）与 `AiosDBMgr` 相关——它已经不是「数据库同步」文件，改名留给下一次。

`manual_update.rs`：`baseline_sync_options` 及其测试删；源码钉改找 `ingest_file(`。

## 4. 顺序

| # | 内容 | 前置 | 备注 |
|---|---|---|---|
| P0 | 等 D8 提交；提交后 `git diff --stat HEAD~1 -- src/versioned_db src/data_interface/manual_update.rs` 核一遍 D8 的最终形状（尤其 `bootstrap_sys_metadata` 与 `settle_dbnum_info_after_baseline`） | — | 不与 D8 在同一批文件上并行改（d-154 的教训：`git add` 抓到别人半成品） |
| P1 | 新模块 `baseline_ingest.rs`：搬队列 / 写者 / 闭包 / 排空 / 清库 / 统计（`rebuild_on`）；`ingest_file` + `ingest_sys_metadata`；搬三条 `finish_write_pipeline_*` 与三条 `collect_project_db_files_*` 测试；新增 I1–I5 源码钉 | P0 | 与旧函数短暂并存，只到 P2 |
| P2 | 切调用点：`initialize_dbnum_baseline` 两处、`bootstrap_sys_metadata`、live 测试、两条源码钉；删 §3 清单；`cargo check --workspace --all-targets --locked`；`cargo test --lib -- versioned_db dbnum_stats element_batch manual_update::tests legacy_parse_pdms_db_guard` | P1 | 一个提交 |
| P3 | 文档：本文件 §7 实施记录；`CONTEXT.md`「基线全量解析」词条改成「基线摄入（`ingest_file`）」；ADR-058 D3 实施注记补一句；`2026-09-04-parse-pdms-db-retirement-inventory.md` §4 B8 从「⏳ 长杆」改「✅ 2026-09-07（路 a 的编排半边；收集器与渲染器早已共用）」；`changelog.md` 一条；drawio stale 页处理 | P2 | |
| P4（可选） | F2 零根基线补盖模型水位 | P2 | 用户拍 |

## 5. 验证

编排层重构的判据是**同样的输入落出同样的行**，不需要 TTY 对拍（数据形状没变）：

1. 单测：P1/P2 列出的那几组全绿；`the_baseline_orchestration_has_no_legacy_parser_intermediates` 改扫新模块后仍绿；
   `Check-LegacyParsePdmsDb.ps1` 六判照旧。
2. 夹具：`cargo test --locked --test baseline_stream_8000`（收集器不动，应原样绿）。
3. **真库对拍（编排等价）**：在 `plant-1` 的 rig 上用新 exe 对 YCYK 同一组 10 个 DESI 库跑一遍空店首载（`runycyk.py prepare/start`），
   与今天 pass 3 的 `verification.json` 比：`pe` 按 dbnum 的行数（5525=37,603 / 7015=12,042 / 5107=9,496 / 6015=6,144 / 7027=159 /
   5526=3 / 5521=5522=5523=5524=2）、CATA 依赖行按目录库的行数（212=2,626 / 7000=857 / …）、`dbnum_info_table` 与 `pe` 逐 dbnum
   相等、`dbnum_watermark` 十库 `applied == file_latest`。再取 5525 任意 200 个 refno `SELECT * FROM pe / ATT_<noun> / ATT_UDA /
   pe_owner` 两库逐字段比对。
4. 父层补缺路径（I5 的真正受益者）：找一个母库与 `_0001` 叶子并存的项目（AMS 的 `ams7997` + `ams7997_0001` 若在）跑一次首载，
   断言第二次摄入后 `dbnum_info_table` 与 `pe` 相等且基线成功——今天这条路在 `1eefbd57` 之后是必失败的。
5. 引导工具：`bin/sync_sys_only` 对一个空库跑一次，`MDB / WORL / db_info` 行数与 D8 提交时的记录一致；l3 套件的空店引导步骤照跑。
6. 耗时：同一组库的「解析任务完成」到「基线已建立」的间隔与 pass 3 同量级（写者形状没变，不该有可见变化）。

## 6. 风险与对策

| 风险 | 对策 |
|---|---|
| 与 D8 会话在同一批文件上互相覆盖 | P0 硬等提交；动手前 `check_paths`，改文件期间 `acquire_file_lock` |
| `bootstrap_sys_metadata` 的 SYS 相依赖「全目录扫 + 按 db_type 过滤」这一 legacy 行为（ADR-007） | `ingest_sys_metadata` 保留一次目录扫（搬 `collect_project_db_files`），只是每个文件走 `ingest_file`；顺序 SYS → DICT 不变（`UdaKeyMap` 要 DICT 先落） |
| 单文件入口后「同批全部清库」变成「只清本库」 | 本来每次调用就只有一个文件在 `included_db_files` 里，行为等价；`ingest_sys_metadata` 一个文件失败即整体 Err，已落的 SYS 库留着（Fresh 幂等，重跑无害） |
| `python/` 绑定或脚本仍引用旧名 | `rg sync_total_async_threaded python scripts` 今天为零；`Invoke-SiteDbnumParse.ps1` 走 `run_cli`，不受影响 |
| 工作树 HEAD 在别的会话未提交改动之外不能编译（d-154） | 验证只在共享工作树上做，不拿干净检出当证据 |

## 7. 实施记录（2026-09-07，fable-5-1-40）

用户 14:15 拍板「按方案执行：等 D8 提交后你接着做 P1–P3，F2 也一起落」。D8 于 14:15:57 提交（`de5fd378`），随后：

| # | 落地 | 与方案不同处 |
|---|---|---|
| P1 | `src/versioned_db/baseline_ingest.rs`：`ingest_file` / `ingest_frozen` / `BaselineIngest` / `DependencySummary`；队列（`QUEUE_CAPACITY=100` / `WRITE_WINDOW=20`）、单写者、CATA 闭包、`drain_writer`、`failure_summary`、`wipe_after_failure`、`settle_statistics`（`rebuild_on`）；源码钉 I1–I5 + 6 条单测 | 多了 `ingest_frozen`（目录扫出来的文件以文件头为身份，给 `bootstrap_sys_metadata` 用）与显式的 `db_option` 入参（只读 `is_save_db` / `sync_chunk_size`）；`ingest_sys_metadata` 没单独成函数，`bootstrap_sys_metadata` 留在 `database.rs` 直接逐文件调 `ingest_frozen` |
| P2 | `database.rs` 1514 → ~680 行（§3 清单全删，另删 `versioned_db/task.rs`）；`initialize_dbnum_baseline` 两处切换；`baseline_sync_options` 及测试删；`sys_metadata_files` 改回路径；六处源码钉改向（含 `Check-LegacyParsePdmsDb.ps1` 第 4 条两处都扫） | `collect_project_db_files` 与两个统计辅助留在 `database.rs` 升 `pub(crate)`，没搬 |
| P3 | `changelog.md`、`CONTEXT.md`「分块解析基线」、ADR-058 D8 实施注记三、退役清单 §4 B8 → ✅ | drawio stale 页未动 |
| P4（F2） | **不是**给零根基线盖模型水位——那正是 ADR-060 D7 的幽灵水位（`advance_model_watermark_on` 对 `settled_floor = None` 本来就 `Unchanged`，是设计）。改在判决侧：`dbnum_consistency::verdict` 新增 `NotJudgedReason::NoGenerationRoots`（数据水位在、模型落后、`gen_root` 空 → `not_judged / no_generation_roots`），`/health.dbnum_consistency` 与启动日志不再对只有 WORL + SITE 的库说「模型在追赶」 | 方案 §2 写的「补盖模型水位」口径作废 |

验证：`cargo check --lib` / `--bin sync_sys_only --bin l3_suite` 过；`cargo test --lib -- versioned_db dbnum_consistency
legacy_parse_pdms_db_guard all_three_write_points_persist_catalogue_dependencies manual_update::tests`
137 passed / 0 failed / 3 ignored；`Check-LegacyParsePdmsDb.ps1` 六判全过。

**§5 第 3 条真库编排等价对拍（2026-09-07 14:52，用户拍板后实跑）**：release exe（`D:\Rust\target-perf7997`，HEAD `b9d7c00b`，
`source_files_modified_after_exe_build = []`）在 `plant-1\runycyk3-ingest` 空店一趟跑完 8 个 SYS/DICT + 10 个 YCYK DESI 库
（14:52:35 → 14:53:14 `数据链已就绪`，一趟到位、没有 pass2）。`compare_counts.py`（回执 `compare_counts.txt`）对 pass 3
（`runycyk1-bootstrap/verification.json` + 13:46 的 `inspect_holds.py`）：**31 个 dbnum 的 `pe` 行数逐一相等**（5525=37,603 /
7015=12,042 / 5107=9,496 / 6015=6,144 / CATA 212=2,626 / 7000=857 / … / 5521–5524=2，总计 71,896）；31 个 dbnum
`dbnum_info_table` 与 `pe` 逐一相等；10 个设计库 `applied == file_latest` 与 pass 3 同值；`MDB=6 / WORL=31`；从 pass 3 店的
副本（另开 surreal :18103）随机抽 5525 的 200 个 refno，`pe / <NOUN> / ATT_UDA / pe_owner 出向 / 入向` 五种行逐字段
**0 处不同**。F2 现场：`/health.dbnum_consistency` = judged 4（in_sync 2 = 5525 / 7015；lagging 2 = 5107 / 6015，各有
held 根）/ **not_judged 6**（六个零根库，pass 3 时它们是 lagging）。耗时：5525 数据批 14.7s（pass 3 13.1s）、5107 6.5s（5.2s）、
7015 5.7s（5.1s）、6015 2.8s（2.1s）——机器上同时还有三套 rig 在跑（其一 14:48 刚起、正在初始化），没有做过隔离的
耗时对比，**不下结论**。模型面照旧停在同 4 根 `generation_hold`（ISSUE-029），与本方案无关。
§5 第 4 条（父层补缺）与第 5 条（`sync_sys_only` 空库引导）**未跑**：YCYK / AMS 手头没有母库与叶子并存的项目。

## 8. 不做 / 留档

- 不做：把基线并进 `IncrementPipeline::apply` 的**窗口对象**（整文件一次收进内存违背 ADR-042「峰值随块不随库」，
  增量的 `Replace` / 逐笔统计 / 自推水位也不是基线要的）。共用的是五个部件，不是那一个函数。
- 不做：`sync_chunk_size` 默认值调整（今天 `10_0000` 等于整文件一块；峰值内存 = 整块元素 + 整块渲出的 SQL，
  有界队列只限排队中的消息）。单独一条配置变更，另议。
- 留档：`data-parse-pipeline.drawio` 第 2 页是壳的最后一张图。
