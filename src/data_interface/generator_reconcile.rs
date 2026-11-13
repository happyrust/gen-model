//! 生成器身份核对（GitHub #73）：水位行的库级落款 `dbnum_watermark.model_generator` ↔ 本进程的生成器指纹。
//!
//! 库级水位 `model_sesno` 追平的库在启动 / `update/execute` 时一根都不碰（ADR-079：两枚水位就是全部的持久证据），
//! 逐根回执里的 `generator_fingerprint` 再准也拦不住它——换了 e3d-model（几何修复）之后旧行照用，直到有人手打
//! `model/rebuild`（#73 的 1112 / 7997 / 7999 / 7329 就是这样留下的）。这里给水位行补上「这个库的几何是哪一版生成器
//! 算的」，不一致就整库排 `ModelOnly` 起点 0（与 `model_rebuild::start` 同一条任务形状，由 worker 照常跑；收尾盖
//! `model_sesno` 时把新指纹一并盖上，`task_end::render_watermark_update`）。
//!
//! 两个入口：启动复核（`startup_reconcile`，与模型水位核对同一个循环，判决由 [`generator_recovery_plan`] 给）；
//! `update/execute`（[`reconcile_generator_now`]，与目录版本核对对称地另起任务跑一次）。旧行没有这一列（`None`）
//! 也算不一致——那正是换过 vendor 却从没按新口径整库算过的库。开关 `model_rebuild_on_generator_change`（默认开）
//! 关着时只点名不排工。落款读不到（库抖动）这一轮不按生成器排——不许一次读失败把每个库都排一遍。

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

use serde::Serialize;

use crate::data_interface::dbnum_state::DbnumState;
use crate::data_interface::startup_reconcile::StartupModelRecoveryPlan;

/// 本进程的生成器指纹（`fast_model::generator_identity`）。
pub fn current_generator() -> &'static str {
    crate::fast_model::generator_identity::generator_fingerprint()
}

/// 纯函数：这个库要不要因生成器变了而整库重算。只看盖过模型水位的库（`model_sesno > 0`）；旧行没有落款
/// （`None`）与落款不等都算变了。
pub(crate) fn generator_stale(model_sesno: i32, stamped: Option<&str>, current: &str) -> bool {
    model_sesno > 0 && stamped != Some(current)
}

/// 启动复核用的判决：生成器变了 → `ModelOnly` 起点 0（整库重排），理由 `model_generator_changed`；开关关着 /
/// 没变 → `None`。它压过「只欠一窗」的水位判决：那一窗的根也在整库里。
pub(crate) fn generator_recovery_plan(
    model_sesno: i32,
    stamped: Option<&str>,
    current: &str,
    enabled: bool,
) -> Option<StartupModelRecoveryPlan> {
    (enabled && generator_stale(model_sesno, stamped, current)).then_some(
        StartupModelRecoveryPlan {
            model_start_sesno: 0,
            reason: "model_generator_changed",
        },
    )
}

/// 水位行上的库级生成器落款：`dbnum → model_generator`。只选两列、不经水位镜像（镜像行没有这一格，读一次表就够）；
/// 盖过模型水位的库才有意义，没盖过的不收。
pub(crate) async fn stamped_generators() -> anyhow::Result<BTreeMap<u32, Option<String>>> {
    #[derive(serde::Deserialize)]
    struct Row {
        dbnum: u32,
        #[serde(default)]
        model_generator: Option<String>,
    }
    let mut response = aios_core::SUL_DB
        .query(
            "SELECT dbnum, model_generator FROM dbnum_watermark \
             WHERE dbnum != NONE AND model_sesno != NONE AND model_sesno > 0;",
        )
        .await?
        .check()?;
    let rows: Vec<Row> = response.take(0)?;
    Ok(rows
        .into_iter()
        .map(|row| (row.dbnum, row.model_generator))
        .collect())
}

/// 一次运行期核对的回执（`/health.generator_reconcile`，与 `catalogue_reconcile` 同款）。
#[derive(Debug, Clone, Serialize, Default)]
pub struct GeneratorReconcile {
    /// 谁触发的：`update-execute`。
    pub origin: &'static str,
    /// 本进程的生成器指纹。
    pub current: String,
    /// 开关 `model_rebuild_on_generator_change`。
    pub enabled: bool,
    /// 核对过的 DESI 库数（当前 MDB 里、监听域内、盖过模型水位的）。
    pub examined: usize,
    /// 落款与本进程指纹不一致的库（升序），含旧行没有落款的。
    pub stale: Vec<u32>,
    /// 本次为之新排了 `ModelOnly` 起点 0 的库。
    pub enqueued: Vec<u32>,
    /// 已有排队行覆盖（并进去了）的库。
    pub already_covered: Vec<u32>,
    /// 取不到文件最新 / 排队失败，这次没排的库；原因在日志里。
    pub skipped: Vec<u32>,
    /// 落款读不到 / 登记表读不到时的原因；那一轮一库都不排。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub elapsed_ms: u64,
    pub finished_at: String,
}

static LAST: LazyLock<Mutex<Option<GeneratorReconcile>>> = LazyLock::new(|| Mutex::new(None));

/// 最近一次运行期核对的回执；没跑过是 `null`。
pub fn snapshot() -> serde_json::Value {
    LAST.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|receipt| serde_json::to_value(receipt).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn finish(mut receipt: GeneratorReconcile, started: std::time::Instant) -> GeneratorReconcile {
    receipt.elapsed_ms = started.elapsed().as_millis() as u64;
    receipt.finished_at = chrono::Utc::now().to_rfc3339();
    *LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(receipt.clone());
    receipt
}

/// 排一条整库重排：`ModelOnly` 起点 0，与启动恢复 / `model_rebuild::start` 同一形状；右端由任务自己取文件最新。
async fn enqueue_full_rebuild(
    state: &DbnumState,
) -> anyhow::Result<crate::data_interface::batch_queue::Enqueued> {
    let dbnum = state.dbnum;
    let source = tokio::task::spawn_blocking(move || {
        crate::data_interface::model_source::latest_source_version(dbnum)
    })
    .await
    .map_err(|error| anyhow::anyhow!("读文件最新会话的阻塞任务异常: {error}"))??;
    let found = crate::data_interface::batch_scheduler::DiscoveredBatch {
        project: state.owner_project.clone(),
        dbnum,
        db_type: state.db_type.clone(),
        phase: crate::data_interface::initialization_phase::DataPhase::Design,
        epoch_id: 0,
        intent: crate::data_interface::batch_queue::BatchIntent::ModelOnly,
        // 起点 0 = 整库重排（ADR-079）：任务按「从没追平过」那一档把 `DbSet@T` 上的每个根都重算。
        model_start_sesno: Some(0),
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
    println!(
        "生成器核对已排整库重排 dbnum={dbnum} model=0→{} intent={} outcome={:?} task={}",
        state.applied_sesno, outcome.info.intent, outcome.outcome, outcome.info.task_id
    );
    Ok(outcome.outcome)
}

/// 运行期入口（`update/execute`）：读登记表与落款，对当前 MDB 里、监听域内、盖过模型水位的 DESI 库逐个判，
/// 不一致的排 `ModelOnly` 起点 0。回执存进 [`snapshot`]。
pub async fn reconcile_generator_now(origin: &'static str) -> GeneratorReconcile {
    let started = std::time::Instant::now();
    let current = current_generator();
    let enabled = crate::options::model_rebuild_on_generator_change();
    let mut receipt = GeneratorReconcile {
        origin,
        current: current.to_string(),
        enabled,
        ..Default::default()
    };
    let registered = match DbnumState::list_registered().await {
        Ok(rows) => rows,
        Err(error) => {
            println!("生成器核对（{origin}）：读不到 DBNUM 注册表，本次一库都不排: {error:#}");
            receipt.error = Some(format!("registry: {error:#}"));
            return finish(receipt, started);
        }
    };
    let stamped = match stamped_generators().await {
        Ok(map) => map,
        Err(error) => {
            println!("生成器核对（{origin}）：读不到水位行的生成器落款，本次一库都不排: {error:#}");
            receipt.error = Some(format!("stamps: {error:#}"));
            return finish(receipt, started);
        }
    };
    let design = match crate::data_interface::startup_reconcile::current_mdb_design_dbnums().await {
        Ok(set) => set,
        Err(error) => {
            println!(
                "生成器核对（{origin}）：读不到当前 MDB 的 DESI 成员名单，本次一库都不排: {error:#}"
            );
            receipt.error = Some(format!("mdb members: {error:#}"));
            return finish(receipt, started);
        }
    };
    for state in registered {
        let dbnum = state.dbnum;
        if !state.db_type.eq_ignore_ascii_case("DESI")
            || state.applied_sesno <= 0
            || state.model_sesno <= 0
            || !design.contains(&dbnum)
            || !crate::data_interface::watch_scope::admits(dbnum)
        {
            continue;
        }
        receipt.examined += 1;
        let stamped_generator = stamped.get(&dbnum).and_then(|stamped| stamped.as_deref());
        if !generator_stale(state.model_sesno, stamped_generator, current) {
            continue;
        }
        receipt.stale.push(dbnum);
        if !enabled {
            continue;
        }
        match enqueue_full_rebuild(&state).await {
            Ok(crate::data_interface::batch_queue::Enqueued::New) => receipt.enqueued.push(dbnum),
            Ok(_) => receipt.already_covered.push(dbnum),
            Err(error) => {
                println!(
                    "生成器核对（{origin}）跳过 dbnum={dbnum}（这次没排，下次执行再核）: {error:#}"
                );
                receipt.skipped.push(dbnum);
            }
        }
    }
    receipt.stale.sort_unstable();
    if receipt.stale.is_empty() {
        println!(
            "[{origin}] 生成器核对：{} 个库的库级生成器落款都与本进程一致（{}）",
            receipt.examined,
            crate::build_info::vendor_build_ids()
        );
    } else {
        println!(
            "[{origin}] 生成器核对：{} 个库的库级生成器落款与本进程不一致 dbnum={:?}（{}）；{}：新排 {:?}、并进已排队行 {:?}、没排 {:?}",
            receipt.stale.len(),
            receipt.stale,
            crate::build_info::vendor_build_ids(),
            if enabled {
                "已按整库重排处理"
            } else {
                "开关 model_rebuild_on_generator_change 关着，只点名"
            },
            receipt.enqueued,
            receipt.already_covered,
            receipt.skipped
        );
    }
    finish(receipt, started)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 判据：只看盖过模型水位的库；落款缺席（旧行）或不等都算变了；相等不算。
    #[test]
    fn a_library_is_stale_when_its_stamp_is_missing_or_differs_from_the_running_generator() {
        assert!(
            generator_stale(26, None, "gen-v2"),
            "旧行没有落款：换过 vendor 却从没按新口径算过"
        );
        assert!(generator_stale(26, Some("gen-v1"), "gen-v2"));
        assert!(!generator_stale(26, Some("gen-v2"), "gen-v2"));
        assert!(
            !generator_stale(0, None, "gen-v2"),
            "从没盖过模型水位的库由水位核对整库重排，不归这里"
        );
        assert!(!generator_stale(-1, Some("gen-v1"), "gen-v2"));
    }

    /// 启动判决：变了就是整库重排（起点 0），理由点名生成器；开关关着一条都不排。
    #[test]
    fn a_changed_generator_rebuilds_the_whole_library_unless_the_switch_is_off() {
        let plan = generator_recovery_plan(26, Some("gen-v1"), "gen-v2", true).expect("stale");
        assert_eq!(plan.model_start_sesno, 0);
        assert_eq!(plan.reason, "model_generator_changed");
        assert!(generator_recovery_plan(26, Some("gen-v2"), "gen-v2", true).is_none());
        assert!(
            generator_recovery_plan(26, None, "gen-v2", false).is_none(),
            "关着只点名"
        );
        assert!(generator_recovery_plan(0, None, "gen-v2", true).is_none());
    }

    /// 排工只排任务，不自己动库：不改水位、不清库、不删模型——与 `model_rebuild::start` 同一条纪律。
    #[test]
    fn the_reconcile_only_enqueues_model_only_tasks_and_never_mutates_the_database() {
        let source = include_str!("generator_reconcile.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        assert!(production.contains("BatchIntent::ModelOnly"));
        assert!(production.contains("model_start_sesno: Some(0)"));
        assert!(production.contains("BatchScheduler::global().enqueue("));
        for forbidden in ["UPDATE dbnum_watermark", "DELETE", "wipe_dbnum", "REMOVE"] {
            assert!(!production.contains(forbidden), "{forbidden}");
        }
        assert!(
            production.contains("SELECT dbnum, model_generator FROM dbnum_watermark"),
            "落款只读两列"
        );
    }
}
