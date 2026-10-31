//! 启动期的模型覆盖复核：模型水位 ↔ 数据水位对齐，落后的库排 `model_sesno → 文件最新` 的
//! ModelOnly 任务（ADR-058 D4 / ADR-079）。
//!
//! `gen_root` 表退役之后（ADR-079），「这个库的模型欠到哪」只有两枚水位一个说法：
//! `dbnum_watermark.model_sesno` 是模型面唯一的持久事实（ADR-060：被写下的承诺，任务收尾一笔盖），
//! 它落后于 `applied_sesno` 就欠 `model_sesno → 文件最新` 这一窗；`0` = 从没追平过，整库重排。
//! 上一进程里哪一根曾经失败、哪一根「安定」——那些都是进程内账本（`model_root_ledger`）的事，
//! 进程终止就随它消失，没有一条恢复路需要知道它们：没做完的根都落在这一窗的差分里。
//! 上一进程死在写库中途的库另有出路：写入标记 `writing{S,T}` → 整库 `Reinitialize`（ADR-073 D3，
//! [`schedule_unfinished_writes_for_reinitialize`]）。
//!
//! 搬出 `model_update_pending.rs` 是计划 §4 **S5a**（缺口 G1）；三个调用点（`lib.rs`、
//! `increment_manager.rs`、`web_service::handlers`）一律写
//! `startup_reconcile::reconcile_model_coverage_at_startup`。
//!
//! 还留在隔壁的一样：目录版本核对 `model_coverage::reconcile_catalogue_versions`——它同时服务
//! 运行期 sweep，不只服务启动。

use std::collections::{BTreeMap, BTreeSet};

use crate::data_interface::initialization_phase::{StartupCoverage, StartupReconcile};
use crate::data_interface::model_coverage::{CatalogueReconcile, reconcile_catalogue_versions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StartupModelRecoveryPlan {
    /// ModelOnly 任务的左端：`model_sesno`（`0` = 整库重排）。
    pub model_start_sesno: i32,
    pub reason: &'static str,
}

/// 把两枚水位折成调度器要的那一个决定（ADR-079：两枚水位就是全部的持久证据）。
///
/// - 数据水位未建立 → 没有东西可对，不排；
/// - `model_sesno >= applied_sesno` → 追平（零根库的空清单收据也是这一格，ADR-072 D4），不排；
/// - 其余 → 排 `model_sesno → 文件最新`；`model_sesno <= 0` 是「从没追平过」，整库重排。
///
/// 纯函数，独立于 MDB / 文件发现：RocksDB 真实停机重启测试直接调它，生产与测试用同一条判据。
pub(crate) fn startup_model_recovery_plan(
    applied_sesno: i32,
    model_sesno: i32,
) -> Option<StartupModelRecoveryPlan> {
    if applied_sesno <= 0 || model_sesno >= applied_sesno {
        return None;
    }
    Some(StartupModelRecoveryPlan {
        model_start_sesno: model_sesno.max(0),
        reason: if model_sesno <= 0 {
            "model_never_stamped"
        } else {
            "model_watermark_behind"
        },
    })
}

/// 启动期把模型水位与数据水位对齐，落后的库排 ModelOnly（ADR-058 D4 / ADR-079）。
///
/// 两级：
///
/// 1. 每个当前 MDB 里的 DESI 库读一次水位镜像行（启动水化那一次已经整表读过，这里零 I/O），
///    `model_sesno >= applied_sesno` 的库**连窗都不开**；
/// 2. 落后的库各排一条 ModelOnly，左端是 `model_sesno`，右端由任务自己取文件最新
///    （ADR-054：水位只承诺数据，生成时点从文件取）。开窗、选根、分桶全部在任务里
///    （[`crate::data_interface::window_root_plan::reconcile_startup_window`]），这里一条判据都不新增
///    （ADR-058 S1）。
///
/// 从没生成过的根按需生成（ADR-058 D6）：`model_sesno` 追平的库里那些 T 端新出现又没被波及的根
/// 是「懒生成」，启动一根不排。
///
/// 任何一库失败都**不中止启动**：告警并按「覆盖未确认」处理，人工出口是
/// `POST /api/v1/dbnums/{dbnum}/model/rebuild`。
///
/// 外壳只做两件事：把内层核出来的覆盖面
/// [落地][crate::data_interface::initialization_phase::InitializationCoordinator::record_startup_coverage]
/// 到协调器，然后无论内层从哪条出口返回，都把
/// [`begin_startup_reconcile`](crate::data_interface::initialization_phase::InitializationCoordinator::begin_startup_reconcile)
/// 落地。落地的含义是「已经排过了」——排出来的工作项由 worker 空闲轮照常消化，
/// `model_ready` 从此刻起才允许置位。漏掉任何一条出口，`model_ready` 就永远起不来。
///
/// 内层**返回**覆盖面而不是自己写进协调器，理由与 settle 同一条：这个函数有几条
/// 出口，靠每条自己记得写一次，迟早会漏掉一条，而漏掉的表现是 `/health` 上那一格
/// 停在 `pending` ——看着像复核卡住了。
pub async fn reconcile_model_coverage_at_startup() {
    let coordinator =
        crate::data_interface::initialization_phase::InitializationCoordinator::global();
    let coverage = reconcile_model_coverage_at_startup_inner().await;
    coordinator.record_startup_coverage(coverage);
    coordinator.settle_startup_reconcile();
    // 复核可能刚排进一批工作项；调度器的空闲等待是 30 秒一圈，不叫醒就白等一圈。
    crate::data_interface::batch_scheduler::BatchScheduler::global().wake();
}

/// 启动水化段的**第一步**（ADR-073 D3）：把上一进程死在写库中途的库排成
/// `Reinitialize`。这里只分类和排队；整库清空留在 worker 冻结点执行。
///
/// 位置有讲究——必须排在
/// [`hydrate_watermark_mirror`](crate::data_interface::dbnum_state::DbnumState::hydrate_watermark_mirror)
/// **之前**。镜像是一次整表替换，替换之后每格只走「按需装载 + 写后作废」；先水化再回退，
/// 发布出去的就是一版已经被删掉痕迹的水位，而回退那几格既没人作废、也没人重新装载。
///
/// 逐库独立：任何一库失败都不中止启动，只是把标记留在库里等下一次——回退是幂等的
/// （`writing{S,T}` 那一行到最后一步才删）。返回要打的行，调用方原样打出来。
pub async fn schedule_unfinished_writes_for_reinitialize() -> Vec<String> {
    let states = match crate::data_interface::write_state::writing_states().await {
        Ok(states) => states,
        Err(error) => {
            return vec![format!(
                "写入标记扫描失败：本次启动不能安排未完成写入重建（是未知，不是「都干净」），下次启动重试: {error:#}"
            )];
        }
    };
    if states.is_empty() {
        return Vec::new();
    }
    // 排队要的是登记行里的项目 / 库类型 / 文件路径。读不到就只回退不排队——watcher 首扫
    // 会把缺口补成一条新批次，比拿一份猜出来的身份去排要好。
    let registered = crate::data_interface::dbnum_state::DbnumState::list_registered()
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|state| (state.dbnum, state))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    let mut lines = vec![format!(
        "写入标记扫描：{} 个库上一次死在写库中途，逐库安排 Reinitialize（ADR-073 D3）",
        states.len()
    )];
    for state in states {
        lines.push(format!(
            "  dbnum={} interrupted_intent={} 窗口 {}→{}，保留标记等待 worker 整库重建",
            state.dbnum,
            state.intent.as_str(),
            state.start_sesno,
            state.end_sesno
        ));
        lines.push(format!(
            "    {}",
            schedule_reinitialize(registered.get(&state.dbnum), &state)
        ));
    }
    lines
}

/// 把未完成写入排成整库重建。排不成时保留 `writing` 标记；watcher 首扫产生的
/// 任意同库批次到 worker 冻结点后也会被该标记就地转成 Reinitialize。
fn schedule_reinitialize(
    state: Option<&crate::data_interface::dbnum_state::DbnumState>,
    marker: &crate::data_interface::write_state::WriteState,
) -> String {
    let Some(state) = state else {
        return format!(
            "dbnum={} 在登记表里没有行，Reinitialize 暂未排成；writing 标记保留",
            marker.dbnum
        );
    };
    if state.file_path.is_empty() {
        return format!(
            "dbnum={} 登记行没有文件路径，Reinitialize 暂未排成；writing 标记保留",
            marker.dbnum
        );
    }
    let path = std::path::PathBuf::from(&state.file_path);
    let intent = crate::data_interface::write_state::recovery_batch_intent(marker.intent);
    let applied = state.applied_sesno.max(0);
    let found = crate::data_interface::batch_scheduler::DiscoveredBatch {
        project: state.owner_project.clone(),
        dbnum: marker.dbnum,
        db_type: state.db_type.clone(),
        phase: crate::data_interface::initialization_phase::DataPhase::of_db_type(&state.db_type),
        epoch_id: 0,
        intent,
        model_start_sesno: None,
        path,
        file_name: state.file_name.clone(),
        applied_sesno: applied,
        file_latest_sesno: marker.end_sesno.max(applied),
        previous_observed_sesno: applied,
        first_pending_sesno_time: None,
        file_latest_sesno_time: None,
        reinit_requested: false,
        regen_roots: Vec::new(),
        catalogue_stamps: BTreeMap::new(),
    };
    let outcome = crate::data_interface::batch_scheduler::BatchScheduler::global().enqueue(
        crate::data_interface::task_registry::TaskRegistry::global(),
        &found,
        false,
    );
    format!(
        "dbnum={} 已排整库重建 intent={} task={}",
        marker.dbnum, outcome.info.intent, outcome.info.task_id
    )
}

/// 启动那一刻**当前 MDB 声明了、模型水位却从没盖过**（`model_sesno <= 0`）的 DESI 库
/// （ADR-058 D6 / ADR-079）：它们的模型要么由下面的 ModelOnly 整库排，要么（数据水位也没建立时）
/// 等数据初始化连模型一起做，启动这里不单独排。
///
/// 分母是 MDB 成员（`model_source::mdb_design_dbnums`，与右端「文件最新」同一份权威），
/// **不是** `DbnumState::list_registered()`（09-04 审核 #6）：登记表是数据面的，里面有
/// SYST / DICT / CATA 与换过 MDB 之后的历史行，拿它当分母会把没有模型面的库报成「无凭证」。
///
/// 读不到成员名单时回 `None`：「没有无凭证的库」与「不知道有没有」在这一栏上长得一
/// 模一样，而前者是绿灯、后者要人去翻日志。
fn dbnums_without_model_watermark(
    declared: &BTreeSet<u32>,
    registered: &BTreeMap<u32, crate::data_interface::dbnum_state::DbnumState>,
    admits: impl Fn(u32) -> bool,
) -> Vec<u32> {
    declared
        .iter()
        .copied()
        .filter(|dbnum| {
            admits(*dbnum)
                && registered
                    .get(dbnum)
                    .is_none_or(|state| state.model_sesno <= 0)
        })
        .collect()
}

/// 登记表里还留着数据 / 模型面状态（`applied_sesno > 0` 或 `model_sesno > 0`）、当前 MDB 却不再声明的元素库
/// （DESI / ISOD），升序（GitHub #41，ADR-078 追记 2026-09-30）。
///
/// 这是「库被移出 MDB」的反方向账：[`dbnums_without_model_watermark`] 数的是声明了没盖水位的，这里数的是盖过水位
/// 却不再声明的。它们的 `pe` / noun 行、模型行、房间边、空间树条目与水位行都还在库里，本期执行范围已经不管它们
/// （ADR-085），也**不自动清**——点名之后由人决定：要清走 `DELETE /api/v1/dbnums/{dbnum}/data`（整库快删，先暂停队列）。
/// 只看元素库：SYST / DICT / CATA 不产生根，依赖入库的 CATA 行由依赖状态表管（ADR-078 D6），不在这一栏。
fn dbnums_retired_from_mdb(
    declared: &BTreeSet<u32>,
    registered: &BTreeMap<u32, crate::data_interface::dbnum_state::DbnumState>,
) -> Vec<u32> {
    registered
        .values()
        .filter(|state| {
            crate::data_interface::model_source::is_element_db(&state.db_type)
                && (state.applied_sesno > 0 || state.model_sesno > 0)
                && !declared.contains(&state.dbnum)
        })
        .map(|state| state.dbnum)
        .collect()
}

/// [`dbnums_retired_from_mdb`] 的启动横幅（空列表不出声）。
fn retired_from_mdb_banner(retired: &[u32]) -> Option<String> {
    if retired.is_empty() {
        return None;
    }
    let sample = retired
        .iter()
        .take(12)
        .map(|dbnum| dbnum.to_string())
        .collect::<Vec<_>>()
        .join("、");
    Some(format!(
        "MDB 退出核对：{} 个元素库已不在当前 MDB 的声明里，库里仍留着它们的数据 / 模型面状态与水位行：{sample}{}。\
         不自动清（GitHub #41）；确认不再需要时先暂停队列，再逐库 DELETE /api/v1/dbnums/{{dbnum}}/data 整库快删",
        retired.len(),
        if retired.len() > 12 { " …" } else { "" }
    ))
}

pub(crate) async fn current_mdb_design_dbnums() -> anyhow::Result<BTreeSet<u32>> {
    tokio::task::spawn_blocking(crate::data_interface::model_source::mdb_design_dbnums)
        .await
        .map_err(|error| anyhow::anyhow!("读取 MDB 成员的阻塞任务异常: {error}"))?
        .map(|dbnums| dbnums.into_iter().collect())
}

/// 启动复核为一个落后的库取右端时的三种结局（09-04 审核 #7）。
enum StartupTargetLookup {
    Found(crate::data_interface::model_source::SourceVersion),
    /// 这个 dbnum 不在当前 MDB：没有文件可核，只能点名——它**不是**「这次没确认」。
    NotInMdb,
    /// 成员名单读不到、stat 失败、解会话失败：这一轮没确认。
    Unreadable(anyhow::Error),
}

/// `result` 那一格的取重：最重的说了算。`not_in_mdb` 刻意不在入参里——它不表示复核没跑成。
pub(crate) fn startup_reconcile_result(window_failed: usize, unreadable: usize) -> &'static str {
    if window_failed > 0 {
        "window_failed"
    } else if unreadable > 0 {
        "unreadable"
    } else {
        "ok"
    }
}

async fn reconcile_model_coverage_at_startup_inner() -> StartupCoverage {
    let mut coverage = StartupCoverage {
        scope: if crate::data_interface::watch_scope::active() {
            "watch"
        } else {
            "all"
        },
        in_scope: 0,
        candidates: 0,
        no_credential_dbnums: None,
        reconcile: StartupReconcile::default(),
    };
    let registered = match crate::data_interface::dbnum_state::DbnumState::list_registered().await {
        Ok(rows) => rows,
        Err(error) => {
            println!("模型水位恢复扫描失败：读不到 DBNUM 注册表，本次按未确认处理: {error:#}");
            coverage.reconcile.result = "watermarks_failed";
            return coverage;
        }
    };
    let registered_by_dbnum = registered
        .into_iter()
        .map(|state| (state.dbnum, state))
        .collect::<BTreeMap<_, _>>();
    // `watch_dbnums=[]` means unrestricted watcher intake, not unrestricted MDB
    // initialization. Startup model recovery is still scoped to the current MDB's
    // declared DESI set; historical watermark rows outside that set are retained
    // but must not reopen work in this project startup.
    let mdb_design_dbnums = match current_mdb_design_dbnums().await {
        Ok(dbnums) => dbnums,
        Err(error) => {
            println!(
                "模型水位核对失败：读取当前 MDB 的 DESI 成员名单失败，启动覆盖范围未知: {error:#}"
            );
            coverage.reconcile.result = "mdb_members_failed";
            return coverage;
        }
    };
    let candidate_dbnums = mdb_design_dbnums.clone();
    coverage.candidates = candidate_dbnums.len();
    coverage.no_credential_dbnums = Some(dbnums_without_model_watermark(
        &mdb_design_dbnums,
        &registered_by_dbnum,
        crate::data_interface::watch_scope::admits,
    ));
    // GitHub #41：反方向那一栏——盖过水位、却不再被当前 MDB 声明的元素库。只点名，不清、不排工。
    let retired_from_mdb = dbnums_retired_from_mdb(&mdb_design_dbnums, &registered_by_dbnum);
    if let Some(banner) = retired_from_mdb_banner(&retired_from_mdb) {
        println!("{banner}");
    }
    // 目录那一格挂在开窗之前、也挂在候选为空的早退之前：目录前移与本库有没有落后是两件
    // 独立的事，一个库模型水位追平、目录却换了版，照样要点名。
    let CatalogueReconcile {
        stale: stale_catalogue,
        unreadable: unreadable_catalogue,
        windows: catalogue_windows,
        cascade: catalogue_cascade,
        behind_out_of_scope: catalogue_behind_out_of_scope,
        dependency_refresh: catalogue_dependency_refresh,
        ..
    } = reconcile_catalogue_versions().await;
    // 目录那一位的语义（P3 起）：前移了**且**反查没能精确回答——某个库退化、或整个反查没跑成。
    // 精确命中只排引用根的启动不算退化。
    let catalogue_cascade_degraded = catalogue_cascade
        .as_ref()
        .is_some_and(|cascade| cascade.is_degraded());
    if !stale_catalogue.is_empty() {
        println!(
            "目录版本核对：{} 个 CATA 库已前移到新会话，而库级目录落款还在旧目录上 \
             dbnum={stale_catalogue:?}。下面每库一行是这一窗变了什么，末行是反查排了什么\
             （2026-09-06 计划 P3）；反查没跑成 / 退化的库只剩按需 ensure 的目录落款判据兜底，\
             或人工 POST /api/v1/dbnums/{{dbnum}}/model/rebuild：",
            stale_catalogue.len()
        );
        for window in &catalogue_windows {
            println!("  {}", window.banner_line());
        }
        if let Some(cascade) = &catalogue_cascade {
            println!("  {}", cascade.banner_line());
        }
    }
    if unreadable_catalogue > 0 {
        println!(
            "目录版本核对：{unreadable_catalogue} 个记在案的 CATA 库在当前 MDB 里读不到最新会话\
             （换过 MDB / 文件不在了），这几个是**未知**、不算已确认过期"
        );
    }
    // GitHub #67：限定域外落后的库启动时说一次——这是它们唯一会被说出来的时刻（周期拍安静时不重复）。
    if !catalogue_behind_out_of_scope.is_empty() {
        println!(
            "{}",
            crate::data_interface::model_coverage::behind_out_of_scope_line(
                &catalogue_behind_out_of_scope
            )
        );
    }
    // A3：数据面那一半——已入库依赖行随目录前移。安静的一次不出声。
    if let Some(line) = catalogue_dependency_refresh.banner_line() {
        println!("目录版本核对：{line}");
    }
    // 数据库架构版本：启动时全局核对一次，只点名、不排任务（与 generator 的自动重排刻意不同）。
    // 读失败这一轮就不点名——一次库抖动不该把库报成老库。
    // 放在候选早退之前：没有要恢复的库也要看见版本是否匹配，否则全新空库永远不写版本记录。
    // 升级范围 = 当前 MDB 声明的设计库 ∩ 监听范围——与升级入口的缺省名单同一分母。
    // 少了监听范围这一层，日志会把本次根本不会碰的库也列进「将被清空」清单；
    // 有 watch_dbnums 时那就是句假话（2026-10-29 演练发现：只放开 7352 却列了 29 个库）。
    // 分母与升级入口同一份：本库主项目拥有的元素库（同号外部项目库不进来，
    // 它们的数据不在这份库里），再叠监听范围。读不到就退回候选 ∩ 监听范围。
    let reconcile_dbnums = match crate::data_interface::storage_upgrade::scope_dbnums().await {
        Ok(dbnums) => dbnums,
        Err(error) => {
            log::warn!("数据库架构版本升级范围核对失败，按 MDB 候选 ∩ 监听范围退避: {error:#}");
            candidate_dbnums
                .iter()
                .copied()
                .filter(|dbnum| crate::data_interface::watch_scope::admits(*dbnum))
                .collect()
        }
    };
    match crate::data_interface::storage_version_reconcile::report_and_log(&reconcile_dbnums).await
    {
        Ok(report) => {
            crate::data_interface::storage_upgrade::observe_startup(&report).await;
            if !report.is_caught_up() {
                println!(
                    "数据库架构版本不匹配（库里 {} / 期望 {}）：请在运维面板或 POST /api/v1/storage-version/upgrade 升级，升级会清空并重新解析、生成以下库：{:?}",
                    report
                        .stored
                        .map_or_else(|| "none".to_owned(), |value| value.to_string()),
                    report.expected,
                    reconcile_dbnums
                );
            }
        }
        Err(error) => {
            println!("数据库架构版本核对：读不到 storage_version:current，本次跳过: {error:#}");
        }
    }

    if candidate_dbnums.is_empty() {
        println!("模型水位核对：没有需要恢复的库");
        coverage.reconcile.result = "ok";
        coverage.reconcile.retired_from_mdb = retired_from_mdb;
        coverage.reconcile.catalogue_cascade_degraded = catalogue_cascade_degraded;
        coverage.reconcile.stale_catalogue_dbnums = stale_catalogue;
        coverage.reconcile.unreadable_catalogue_dbnums = unreadable_catalogue;
        coverage.reconcile.catalogue_windows = catalogue_windows;
        coverage.reconcile.catalogue_cascade = catalogue_cascade;
        coverage.reconcile.catalogue_dependency_refresh = catalogue_dependency_refresh;
        coverage.reconcile.catalogue_behind_out_of_scope = catalogue_behind_out_of_scope;
        return coverage;
    }

    // GitHub #73：库级生成器落款。读不到（`None`）这一轮不按生成器排——不许一次库抖动把每个库都排一遍。
    let generator = crate::data_interface::generator_reconcile::current_generator();
    let generator_rebuild_enabled = crate::options::model_rebuild_on_generator_change();
    let stamped_generators =
        match crate::data_interface::generator_reconcile::stamped_generators().await {
            Ok(map) => Some(map),
            Err(error) => {
                println!(
                    "生成器核对：读不到水位行的生成器落款，本次启动不按生成器排重算: {error:#}"
                );
                None
            }
        };
    let mut generator_stale: Vec<u32> = Vec::new();

    let mut examined = 0usize;
    let mut windows = 0usize;
    let mut enqueued = 0usize;
    let mut unreadable = 0usize;
    let mut not_in_mdb: Vec<u32> = Vec::new();
    let window_failed = 0usize;
    for dbnum in candidate_dbnums {
        if !crate::data_interface::watch_scope::admits(dbnum) {
            continue;
        }
        examined += 1;
        let Some(state) = registered_by_dbnum.get(&dbnum) else {
            not_in_mdb.push(dbnum);
            continue;
        };
        if !state.db_type.eq_ignore_ascii_case("DESI") || state.applied_sesno <= 0 {
            continue;
        }
        // GitHub #73：生成器变了的库整库重排（起点 0），压过「只欠一窗」的水位判决——那一窗的根也在整库里。
        // 落款读不到时这一轮不按生成器排；开关关着只点名（`generator_stale_dbnums`）。
        let stamped_generator = stamped_generators
            .as_ref()
            .map(|stamped| stamped.get(&dbnum).and_then(|stamped| stamped.as_deref()));
        if let Some(stamped_generator) = stamped_generator
            && crate::data_interface::generator_reconcile::generator_stale(
                state.model_sesno,
                stamped_generator,
                generator,
            )
        {
            generator_stale.push(dbnum);
        }
        let generator_plan = stamped_generator.and_then(|stamped_generator| {
            crate::data_interface::generator_reconcile::generator_recovery_plan(
                state.model_sesno,
                stamped_generator,
                generator,
                generator_rebuild_enabled,
            )
        });
        // 一级过滤：两枚水位决定要不要排 ModelOnly（ADR-079）。这个纯决策也由真实停机重启测试
        // 直接调用，避免生产规则与测试里的手写判据漂移。
        let Some(recovery) = generator_plan
            .or_else(|| startup_model_recovery_plan(state.applied_sesno, state.model_sesno))
        else {
            continue;
        };
        // 先问「在不在当前 MDB」，再问「文件最新是哪一版」：两句失败的含义不同（#7），
        // 右端仍取 `latest_source_version`——启动的标尺是文件最新，不是应用水位。
        let source = match tokio::task::spawn_blocking(move || {
            match crate::data_interface::model_source::source_file_lookup(dbnum) {
                Ok(None) => StartupTargetLookup::NotInMdb,
                Err(error) => StartupTargetLookup::Unreadable(error),
                Ok(Some(_)) => {
                    match crate::data_interface::model_source::latest_source_version(dbnum) {
                        Ok(target) => StartupTargetLookup::Found(target),
                        Err(error) => StartupTargetLookup::Unreadable(error),
                    }
                }
            }
        })
        .await
        {
            Ok(StartupTargetLookup::Found(target)) => target,
            Ok(StartupTargetLookup::NotInMdb) => {
                not_in_mdb.push(dbnum);
                continue;
            }
            Ok(StartupTargetLookup::Unreadable(error)) => {
                unreadable += 1;
                println!(
                    "模型水位核对跳过 dbnum={dbnum}（取不到文件最新会话，按未确认处理，\
                     启动继续）: {error:#}"
                );
                continue;
            }
            Err(error) => {
                unreadable += 1;
                println!(
                    "模型水位核对跳过 dbnum={dbnum}（读取文件最新会话的阻塞任务异常，\
                     按未确认处理，启动继续）: {error}"
                );
                continue;
            }
        };
        println!(
            "[model-completion] dbnum={dbnum} applied={} model={} reason={}",
            state.applied_sesno, state.model_sesno, recovery.reason,
        );
        if source.sesno < state.applied_sesno as u32 {
            unreadable += 1;
            println!(
                "模型恢复跳过 dbnum={dbnum}：登记应用水位 {} 超过文件最新 {}",
                state.applied_sesno, source.sesno
            );
            continue;
        }
        windows += 1;
        let model_start_sesno = recovery.model_start_sesno;
        let found = crate::data_interface::batch_scheduler::DiscoveredBatch {
            project: state.owner_project.clone(),
            dbnum,
            db_type: state.db_type.clone(),
            phase: crate::data_interface::initialization_phase::DataPhase::Design,
            epoch_id: 0,
            intent: crate::data_interface::batch_queue::BatchIntent::ModelOnly,
            model_start_sesno: Some(model_start_sesno),
            path: source.file.clone(),
            file_name: source
                .file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| state.file_name.clone()),
            applied_sesno: state.applied_sesno,
            file_latest_sesno: state.applied_sesno,
            previous_observed_sesno: state.applied_sesno,
            first_pending_sesno_time: None,
            file_latest_sesno_time: state.applied_sesno_time.clone(),
            reinit_requested: false,
            regen_roots: Vec::new(),
            catalogue_stamps: BTreeMap::new(),
        };
        let outcome = crate::data_interface::batch_scheduler::BatchScheduler::global().enqueue(
            crate::data_interface::task_registry::TaskRegistry::global(),
            &found,
            false,
        );
        if outcome.outcome != crate::data_interface::batch_queue::Enqueued::AlreadyCovered {
            enqueued += 1;
        }
        println!(
            "启动模型恢复已排 Task dbnum={dbnum} model={}→{} intent={} task={}",
            model_start_sesno, state.applied_sesno, outcome.info.intent, outcome.info.task_id
        );
    }
    if !not_in_mdb.is_empty() {
        not_in_mdb.sort_unstable();
        let sample = not_in_mdb
            .iter()
            .take(12)
            .map(|dbnum| dbnum.to_string())
            .collect::<Vec<_>>()
            .join("、");
        println!(
            "模型水位核对：{} 个库在当前 MDB 里声明了、却没有登记行或不在成员文件里；\
             没有文件可核，只点名、不计入「未确认」：{sample}{}",
            not_in_mdb.len(),
            if not_in_mdb.len() > 12 { " …" } else { "" }
        );
    }
    if !generator_stale.is_empty() {
        println!(
            "生成器核对：{} 个库的库级生成器落款与本进程不一致 dbnum={generator_stale:?}（e3d-model {}）；{}",
            generator_stale.len(),
            crate::build_info::E3D_MODEL_BUILD_ID,
            if generator_rebuild_enabled {
                "已按整库重排处理（ModelOnly 起点 0，reason=model_generator_changed）"
            } else {
                "开关 model_rebuild_on_generator_change 关着，只点名；要重算请 POST /api/v1/dbnums/{dbnum}/model/rebuild"
            }
        );
    }
    println!(
        "模型水位核对完成：核对 {examined} 库，落后 {windows} 库，排队 {enqueued} 项，\
         取不到文件最新 {unreadable} 库，不在当前 MDB {} 库",
        not_in_mdb.len()
    );
    coverage.in_scope = examined;
    coverage.reconcile = StartupReconcile {
        // 最重的那一档说了算：开窗失败要人动手，取不到文件最新只是这一轮没确认；
        // 不在当前 MDB 的历史行不参与取重（#7）。
        result: startup_reconcile_result(window_failed, unreadable),
        unreadable,
        not_in_mdb: not_in_mdb.len(),
        retired_from_mdb,
        window_failed,
        lagging: windows,
        windows_opened: windows - window_failed,
        // 目录那一格**不参与** `result` 的取重：`result` 说的是「这次复核本身跑成没跑成」，
        // 而目录反查退化是一个在案的、回执点名的已知降级（ADR-058 实施约束 5），不是本次
        // 复核出了岔子。混进去会让每一次有目录变更的启动都报成 `unreadable` / 失败态。
        catalogue_cascade_degraded,
        stale_catalogue_dbnums: stale_catalogue,
        unreadable_catalogue_dbnums: unreadable_catalogue,
        catalogue_windows,
        catalogue_behind_out_of_scope,
        catalogue_cascade,
        catalogue_dependency_refresh,
        generator_stale_dbnums: generator_stale,
    };
    coverage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_watermark_gap_becomes_model_only_recovery_window() {
        let plan = startup_model_recovery_plan(43, 42)
            .expect("a durable model watermark gap must be rebuilt after restart");
        assert_eq!(plan.model_start_sesno, 42);
        assert_eq!(plan.reason, "model_watermark_behind");
    }

    #[test]
    fn aligned_watermarks_do_not_requeue() {
        assert!(startup_model_recovery_plan(43, 43).is_none());
        assert!(
            startup_model_recovery_plan(43, 44).is_none(),
            "模型水位不该高于数据水位（M2），真高了也不是欠账"
        );
        assert!(
            startup_model_recovery_plan(0, 0).is_none(),
            "数据水位未建立：没有东西可对"
        );
    }

    /// ADR-079：从没盖过模型水位的库整库重排（左端 0），不再有「安定凭证下限」可当左端。
    #[test]
    fn a_never_stamped_model_watermark_rebuilds_the_whole_library() {
        let plan = startup_model_recovery_plan(43, 0).expect("never stamped is behind");
        assert_eq!(plan.model_start_sesno, 0);
        assert_eq!(plan.reason, "model_never_stamped");
        let negative = startup_model_recovery_plan(43, -1).expect("negative is never stamped");
        assert_eq!(negative.model_start_sesno, 0);
    }

    #[test]
    fn no_credential_dbnums_are_the_declared_design_dbnums_without_a_model_watermark() {
        use crate::data_interface::dbnum_state::DbnumState;
        let state = |dbnum: u32, model: i32| DbnumState {
            dbnum,
            model_sesno: model,
            applied_sesno: 10,
            ..Default::default()
        };
        let registered: BTreeMap<u32, DbnumState> = [state(8000, 10), state(8001, 0)]
            .into_iter()
            .map(|state| (state.dbnum, state))
            .collect();
        let declared: BTreeSet<u32> = [8000, 8001, 8002, 8003].into_iter().collect();
        // 8000 盖过；8001 登记了没盖过；8002 声明了却没有登记行——后两者都在名单上；
        // 8003 在监听限定域外，不算。
        assert_eq!(
            dbnums_without_model_watermark(&declared, &registered, |dbnum| dbnum != 8003),
            vec![8001, 8002]
        );
    }

    /// GitHub #41：反方向那一栏——登记表里盖过水位（数据或模型任一）、当前 MDB 却不再声明的元素库才点名；
    /// 声明着的、非元素库（CATA 由依赖状态表管）、从没盖过水位的历史行都不算。只点名，不清、不排工。
    #[test]
    fn retired_from_mdb_names_stamped_element_rows_the_current_mdb_no_longer_declares() {
        use crate::data_interface::dbnum_state::DbnumState;
        let state = |dbnum: u32, db_type: &str, applied: i32, model: i32| DbnumState {
            dbnum,
            db_type: db_type.to_string(),
            applied_sesno: applied,
            model_sesno: model,
            ..Default::default()
        };
        let registered: BTreeMap<u32, DbnumState> = [
            state(8000, "DESI", 10, 10), // 还声明着
            state(7999, "DESI", 12, 12), // 移出：数据、模型都盖过
            state(7998, "DESI", 0, 5),   // 移出：只剩模型水位
            state(9002, "ISOD", 3, 0),   // 移出的 ISOD：只有数据水位
            state(7355, "CATA", 40, 0),  // 目录库不在这一栏
            state(7997, "DESI", 0, 0),   // 从没盖过水位的历史行没有东西可清
        ]
        .into_iter()
        .map(|state| (state.dbnum, state))
        .collect();
        let declared: BTreeSet<u32> = [8000, 8001].into_iter().collect();

        let retired = dbnums_retired_from_mdb(&declared, &registered);
        assert_eq!(retired, vec![7998, 7999, 9002]);
        let banner = retired_from_mdb_banner(&retired).expect("有移出的库要出声");
        assert!(
            banner.contains("3 个元素库")
                && banner.contains("7998、7999、9002。")
                && banner.contains("不自动清")
                && banner.contains("DELETE /api/v1/dbnums/{dbnum}/data"),
            "{banner}"
        );
        assert!(retired_from_mdb_banner(&[]).is_none(), "空列表不出声");
        let many: Vec<u32> = (1..=13).collect();
        let truncated = retired_from_mdb_banner(&many).expect("13 个");
        assert!(
            truncated.contains("13 个元素库") && truncated.contains("11、12 …。"),
            "超过 12 个只列前 12 个: {truncated}"
        );
    }
}
