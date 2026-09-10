//! 启动期的模型覆盖复核：凭证 ↔ 文件最新会话对齐，缺口开退化窗口（ADR-058 D4）。
//!
//! [`clear_stale_at_process_start`](crate::data_interface::model_update_pending::clear_stale_at_process_start)
//! （ADR-050）把上一进程的队列清空之后，「这个库的模型欠到哪」只剩 `gen_root` 凭证这一个
//! 说法——本模块就是把那个说法读出来、和文件最新会话比一遍、缺口排成活的地方。ADR-065 D2
//! 之后它更是「欠什么」的正经入口，与那张要退役的队列表不是一回事。
//!
//! 搬出 `model_update_pending.rs` 是计划 §4 **S5a**（缺口 G1）；**名字与行为不变**，只换
//! 住址：三个调用点（`lib.rs`、`increment_manager.rs`、`web_service::handlers`）一律写
//! `startup_reconcile::reconcile_model_coverage_at_startup`，`model_update_pending` 不再
//! 再导出本模块。
//!
//! 还留在隔壁的两样：一级过滤的取数 `startup_coverage_floors` 与目录版本核对
//! `reconcile_catalogue_versions`——它们同时服务运行期 sweep，不只服务启动，
//! 归属另议（S9 之前不必定）。

use std::collections::{BTreeMap, BTreeSet};

use aios_core::SUL_DB;

use crate::data_interface::initialization_phase::{StartupCoverage, StartupReconcile};
use crate::data_interface::model_coverage::{
    CatalogueReconcile, StartupCoverageFloor, backfill_gen_root_dbnum_on,
    reconcile_catalogue_versions, startup_coverage_floors,
};
use crate::data_interface::model_watermark::reconcile_model_watermarks;

/// 启动期把生成根凭证与**文件最新会话**对齐——[`clear_stale_at_process_start`]（ADR-050）
/// 的补偿路径。清空之后，凭证（`gen_root.source_end_sesno[_time]`）是上一进程模型进度唯一
/// 活下来的记录；不在这里核对，重启就会把「积压被清掉」静默判成「模型完整」。
///
/// 两级（ADR-058 D5）：
///
/// 1. [`startup_coverage_floors`] 一次问完全部库的凭证下限，与文件最新会话比。追平的库
///    **连窗都不开**；
/// 2. 落后的库各开一次[启动退化窗口][crate::data_interface::window_root_plan::reconcile_startup_window]。
///
/// 标尺是文件最新，不是 `applied_sesno`（ADR-054：水位只承诺数据，生成时点从文件取）。
/// 选根判据整条来自稳态窗口，这里一条都不新增（ADR-058 S1）；`fn::sync_gen_roots` 不在这条
/// 路上——它的 `fn::gen_root_cover` 第一句就是查 `pe`，而 ADR-056 N7 要求模型面不以 `pe` 为前置。
///
/// `gen_root` 一行都没有的库不参与退化窗口：从未生成过的根按需生成（ADR-058 D6），
/// 启动一根不排；它仍进入库级水位探针，以兼容已有属性与模型投影但没有水位追踪表的历史库。
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
/// 内层**返回**覆盖面而不是自己写进协调器，理由与 settle 同一条：这个函数有五条
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
    };
    let outcome = crate::data_interface::batch_scheduler::BatchScheduler::global().enqueue(
        crate::data_interface::task_registry::TaskRegistry::global(),
        &found,
        false,
    );
    format!(
        "dbnum={} 已排整库重建 intent={} task={}",
        marker.dbnum,
        outcome.info.intent,
        outcome.info.task_id
    )
}

/// 启动那一刻**当前 MDB 声明了、却一条凭证记录都没有**的 DESI 库（ADR-058 D6：等按需
/// ensure，启动一根不排）。
///
/// 分母是 MDB 成员（`model_source::mdb_design_dbnums`，与右端「文件最新」同一份权威），
/// **不是** `DbnumState::list_registered()`（09-04 审核 #6）：登记表是数据面的，里面有
/// SYST / DICT / CATA 与换过 MDB 之后的历史行，拿它当分母会把没有模型面的库报成「无凭证」。
///
/// 读不到成员名单时回 `None`：「没有无凭证的库」与「不知道有没有」在这一栏上长得一
/// 模一样，而前者是绿灯、后者要人去翻日志。
async fn dbnums_without_credentials(
    floors: &BTreeMap<u32, StartupCoverageFloor>,
) -> Option<Vec<u32>> {
    let declared = current_mdb_design_dbnums().await;
    match declared {
        Ok(declared) => Some(
            declared
                .into_iter()
                .filter(|dbnum| {
                    crate::data_interface::watch_scope::admits(*dbnum)
                        && !floors.contains_key(dbnum)
                })
                .collect(),
        ),
        Err(error) => {
            println!(
                "模型凭证核对：读不到当前 MDB 的 DESI 成员名单，本次说不出哪些库还没有凭证记录\
                 ——是未知，不是「都有」: {error:#}"
            );
            None
        }
    }
}

async fn current_mdb_design_dbnums() -> anyhow::Result<BTreeSet<u32>> {
    tokio::task::spawn_blocking(crate::data_interface::model_source::mdb_design_dbnums)
        .await
        .map_err(|error| anyhow::anyhow!("读取 MDB 成员的阻塞任务异常: {error}"))?
        .map(|dbnums| dbnums.into_iter().collect())
}

/// 启动复核为一个有凭证的库取右端时的三种结局（09-04 审核 #7）。
enum StartupTargetLookup {
    Found(crate::data_interface::model_source::SourceVersion),
    /// 这个 dbnum 不在当前 MDB：换过 MDB 之后留下的历史 `gen_root` 行。没有文件可核，
    /// 只能点名——它**不是**「这次没确认」。
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

pub(crate) fn model_recovery_base(
    recorded_model_sesno: i32,
    settled_root_floor: Option<i32>,
    completion_inputs_invalid: bool,
) -> i32 {
    if completion_inputs_invalid {
        0
    } else {
        settled_root_floor.unwrap_or(recorded_model_sesno)
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
    // 先补归属再取下限：只有嵌套 `desired_target.dbnum` 的历史行不补就取不到，
    // 而取不到的表现不是报错，是那个库当面消失。
    match backfill_gen_root_dbnum_on(&SUL_DB).await {
        Ok(0) => {}
        Ok(rows) => println!("生成根库归属回填：{rows} 行只有嵌套归属的历史行补上了顶层 dbnum"),
        Err(error) => {
            println!("生成根库归属回填失败（本次核对可能少算若干库，下次启动再试）: {error:#}")
        }
    }
    let floors = match startup_coverage_floors().await {
        Ok(floors) => floors,
        Err(error) => {
            println!(
                "模型凭证核对失败：读 gen_root 凭证下限失败，本次启动无法核对模型覆盖\
                 ——覆盖状态是未知，不是完整: {error:#}"
            );
            coverage.reconcile.result = "floors_failed";
            return coverage;
        }
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
    // declared DESI set; historical gen_root/watermark rows outside that set are
    // retained but must not reopen work in this project startup.
    let mdb_design_dbnums = match current_mdb_design_dbnums().await {
        Ok(dbnums) => dbnums,
        Err(error) => {
            println!(
                "模型凭证核对失败：读取当前 MDB 的 DESI 成员名单失败，启动覆盖范围未知: {error:#}"
            );
            coverage.reconcile.result = "mdb_members_failed";
            return coverage;
        }
    };
    // Even equal watermarks require a valid source-bound completion receipt.
    let candidate_dbnums = mdb_design_dbnums.clone();
    coverage.candidates = candidate_dbnums.len();
    coverage.no_credential_dbnums = dbnums_without_credentials(&floors).await;
    // 目录那一格挂在开窗之前、也挂在 `floors.is_empty()` 早退之前：目录前移与本库
    // 有没有落后是两件独立的事，一个库凭证全追平、目录却换了版，照样要点名。
    let CatalogueReconcile {
        stale: stale_catalogue,
        unreadable: unreadable_catalogue,
        windows: catalogue_windows,
        cascade: catalogue_cascade,
        dependency_refresh: catalogue_dependency_refresh,
    } = reconcile_catalogue_versions().await;
    // 目录那一位的语义（P3 起）：前移了**且**反查没能精确回答——某个库退化、或整个反查没跑成。
    // 精确命中只排引用根的启动不算退化。
    let catalogue_cascade_degraded = catalogue_cascade
        .as_ref()
        .is_some_and(|cascade| cascade.is_degraded());
    if !stale_catalogue.is_empty() {
        println!(
            "目录版本核对：{} 个 CATA 库已前移到新会话，而已发布的根还落在旧目录上 \
             dbnum={stale_catalogue:?}。下面每库一行是这一窗变了什么，末行是反查排了什么\
             （2026-09-06 计划 P3）；反查没跑成 / 退化的库只剩按需 ensure 的目录指纹失配兜底，\
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
    // A3：数据面那一半——已入库依赖行随目录前移。安静的一次不出声。
    if let Some(line) = catalogue_dependency_refresh.banner_line() {
        println!("目录版本核对：{line}");
    }
    // 模型水位双向对账（ADR-060 D6/D7），挂在开窗之前：它只读 `gen_root` 与水位行，
    // 不依赖文件、也不影响下面选根开窗的任何一个判据。
    //
    // 覆盖面是**有凭证的库 ∪ 没凭证的库**：幽灵模型水位恰恰长在后一堆里——`gen_root`
    // 零行的库根本不会出现在 `floors`（那是一条 `GROUP BY gen_root`），只查前一堆
    // 就永远发现不了「水位说追平过、模型面一行都没有」。
    let completion_probes = reconcile_model_watermarks(candidate_dbnums.iter().copied()).await;
    if candidate_dbnums.is_empty() {
        println!("模型凭证与严格水位核对：没有需要恢复的库");
        coverage.reconcile.result = "ok";
        coverage.reconcile.catalogue_cascade_degraded = catalogue_cascade_degraded;
        coverage.reconcile.stale_catalogue_dbnums = stale_catalogue;
        coverage.reconcile.unreadable_catalogue_dbnums = unreadable_catalogue;
        coverage.reconcile.catalogue_windows = catalogue_windows;
        coverage.reconcile.catalogue_cascade = catalogue_cascade;
        coverage.reconcile.catalogue_dependency_refresh = catalogue_dependency_refresh;
        return coverage;
    }

    let mut examined = 0usize;
    let mut windows = 0usize;
    let mut enqueued = 0usize;
    let advanced = 0usize;
    let mut unreadable = 0usize;
    let mut legacy_adopted = 0usize;
    let mut not_in_mdb: Vec<u32> = Vec::new();
    let mut window_failed = 0usize;
    for dbnum in candidate_dbnums {
        if !crate::data_interface::watch_scope::admits(dbnum) {
            continue;
        }
        examined += 1;
        let Some(state) = registered_by_dbnum.get(&dbnum) else {
            not_in_mdb.push(dbnum);
            continue;
        };
        let Some(probe) = completion_probes.get(&dbnum) else {
            unreadable += 1;
            continue;
        };
        if !state.db_type.eq_ignore_ascii_case("DESI") || state.applied_sesno <= 0 {
            continue;
        }
        // 兼容接管的历史库（ADR-060 D10）：旧管线投影已按数据水位接管为追平，既不落后也没有
        // 凭证可补；排恢复 Task 只会把整库模型从 0 重算一遍。跳过，但要出声。
        if probe.legacy_adopted() {
            legacy_adopted += 1;
            println!(
                "[model-completion] dbnum={dbnum} applied={} model={} reason=legacy_adopted \
                 （旧管线模型投影已按数据水位接管为追平，不排恢复 Task）",
                state.applied_sesno, probe.recorded
            );
            continue;
        }
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
                    "模型凭证核对跳过 dbnum={dbnum}（取不到文件最新会话，按未确认处理，\
                     启动继续）: {error:#}"
                );
                continue;
            }
            Err(error) => {
                unreadable += 1;
                println!(
                    "模型凭证核对跳过 dbnum={dbnum}（读取文件最新会话的阻塞任务异常，\
                     按未确认处理，启动继续）: {error}"
                );
                continue;
            }
        };
        // 一级过滤：安定凭证已不早于文件最新，且没有未安定的行 —— 这个库连窗都不用开。
        let credential_behind = floors.get(&dbnum).is_some_and(|floor| {
            floor.unsettled_roots > 0
                || floor
                    .settled_floor
                    .is_none_or(|settled| settled < state.applied_sesno)
        });
        // 「完成收据覆盖到 T」那一格随 `CompletionReceipt` 退了（ADR-072 D4）：库级完成态就是
        // 任务收尾盖的 `model_sesno`（`probe.recorded`），凭证那一半由 `supported()` 说。
        let completion_covers = probe.supported() >= state.applied_sesno;
        let behind =
            probe.recorded < state.applied_sesno || credential_behind || !completion_covers;
        if !behind {
            continue;
        }
        println!(
            "[model-completion] dbnum={dbnum} applied={} model={} floor={:?} \
            reason={} credential_behind={credential_behind}",
            state.applied_sesno,
            probe.recorded,
            probe.settled_floor,
            if !probe.has_roots {
                "empty_inventory_unverified"
            } else if credential_behind {
                "root_credential_behind"
            } else {
                "model_watermark_behind"
            }
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
        // The generation window starts at the authoritative settled
        // root-credential floor, not at the library completion watermark.  The
        // latter may have been cleared after a stale receipt was discovered.
        // Taking `min(floor, recorded)` turns that local completion correction
        // into a full-history model rebuild even though the per-root evidence
        // still proves a much newer base.
        let model_start_sesno = model_recovery_base(
            probe.recorded,
            floors.get(&dbnum).and_then(|floor| floor.settled_floor),
            false,
        );
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
            "模型凭证核对：{} 个库在 gen_root 里有凭证记录、却不在当前 MDB（历史行，多半是换过 \
             MDB）；没有文件可核，只点名、不计入「未确认」：{sample}{}",
            not_in_mdb.len(),
            if not_in_mdb.len() > 12 { " …" } else { "" }
        );
    }
    println!(
        "模型凭证核对完成：核对 {examined} 库，开窗 {windows} 库，排队 {enqueued} 项，\
         前移凭证 {advanced} 根，取不到文件最新 {unreadable} 库，不在当前 MDB {} 库，\
         开窗失败 {window_failed} 库，兼容接管 {legacy_adopted} 库",
        not_in_mdb.len()
    );
    coverage.in_scope = examined;
    coverage.reconcile = StartupReconcile {
        // 最重的那一档说了算：开窗失败要人动手，取不到文件最新只是这一轮没确认；
        // 不在当前 MDB 的历史行不参与取重（#7）。
        result: startup_reconcile_result(window_failed, unreadable),
        unreadable,
        not_in_mdb: not_in_mdb.len(),
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
        catalogue_cascade,
        catalogue_dependency_refresh,
    };
    coverage
}
