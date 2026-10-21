//! 强制范围重生成的未完成留痕 `model_scope_regen`（issue #59 根因 2）。
//!
//! 右键「重新生成模型」= 先删后生成（`on_demand_model::clear_model_scope`）。摄入形态下那一删动的
//! 是库 + 投影 + 账本，而模型面唯一的持久事实 `dbnum_watermark.model_sesno` 不跟着退：进程死在
//! 「删完、还没生成完」之间，重启后水位说这个库已追平、账本是空的，没有哪条恢复路知道这片范围
//! 被删过，洞会一直留到有人再点一次。这一行就是那一删的持久留痕：
//!
//! - 清理**之前**落（[`begin`]），落不下来就不清、请求整体失败；
//! - 范围内每一根都生成成功、且 write-behind 确认这些根已落盘，才删（[`settle`]）。删按 `run_id`
//!   比对：删之前又来了一轮强制重生成的，旧一轮的确认删不掉新一轮的行；
//! - 有根没成：行留着，`last_error` 记最近一次的失败摘要；此后同一范围的 `force = false` 请求
//!   （客户端补齐、人再点一次）全部成功一样删得掉（[`adopt`]）；
//! - 启动就绪之后对残留的每一行以 `force = false` 续跑同一范围（[`resume_pending`]）；
//! - 「清理模型」显式清掉的范围，连同被它覆盖的留痕一并撤掉（[`discard_covered_by`]），续跑不把它
//!   生成回来。
//!
//! 它不是模型面的第二份真值（ADR-072 D5 退掉的那类逐根 pending 行）：不进规划、不拦水位、不参与任何
//! 「已追平」判据，只回答「哪个范围被先删了、还没补回来」。只写内存与读透两种形态不落这一行——
//! 它们的清理不删权威库。进程内另有一份同内容的登记表：`/health` 与按需请求只读它，不为此读库。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aios_core::{RefnoEnum, SUL_DB};
use anyhow::Context;
use e3d_io::refno::RefNo;
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use super::dbnum_state::escape_surql_str;
use super::direct_store::DirectStoreError;
use super::direct_tree::{DirectTreeProvider, DirectTreeService};
use super::generation_root::GenerationRoot;
use super::on_demand_model::{OnDemandModelResult, root_failure_is_retryable};
use super::tidb_manager::AiosDBManager;

/// 一范围一行的留痕表（`model_scope_regen:scope_{ref0}_{ref1}`）。
pub const SCOPE_REGEN_TABLE: &str = "model_scope_regen";

/// `/health` 里点名的条数上限；总数在 `pending`。
const HEALTH_SAMPLE_LIMIT: usize = 16;

/// 启动续跑：一轮没了结、等一等可能就好的范围（根锁被占、库还没回到 rocksdb……）隔多久再试、最多
/// 试几轮。试满还没了结的行留着，下一次同范围请求或下一次启动接着收。
const RESUME_RETRY_INTERVAL: Duration = Duration::from_secs(120);
const RESUME_PASSES: usize = 30;
const READINESS_POLL: Duration = Duration::from_secs(5);

/// 一行留痕：库里读回的形态，也是 `/health.model_scope_regen.samples` 的一项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingScope {
    /// 被先删的请求节点（`a/b`）。
    pub refno: String,
    pub dbnum: u32,
    /// 最近一次认领这一行的那一轮。
    pub run_id: String,
    /// 落行时范围内的生成根数（只作展示）。
    #[serde(default)]
    pub root_count: usize,
    /// 这一行挂着期间失败过几轮。
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    /// 这片范围最早被先删的时刻：之后的轮次接着用这一行，不改它。
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    /// 写下这一行的强制重生成还在跑。只在进程内有意义，库里没有这一格。
    #[serde(default)]
    pub in_flight: bool,
}

/// 一轮范围生成认领的那一行。记失败、删行都按 `run_id` 比对：行被后来的一轮换过身份，就不归这一轮管。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeRun {
    refno: String,
    dbnum: u32,
    run_id: String,
}

/// `/health.model_scope_regen`：进程内登记表的读数，不读库。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ScopeRegenHealth {
    /// 还没补回来的范围数。
    pub pending: usize,
    /// 启动续跑走到哪了；`null` = 本进程没起续跑（读透形态，或服务还没起来）。
    pub resume: Option<&'static str>,
    pub samples: Vec<PendingScope>,
}

static REGISTRY: LazyLock<Mutex<BTreeMap<String, PendingScope>>> = LazyLock::new(Default::default);
static RESUME_STATE: Mutex<Option<&'static str>> = Mutex::new(None);

fn registry() -> MutexGuard<'static, BTreeMap<String, PendingScope>> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

fn set_resume_state(state: &'static str) {
    *RESUME_STATE.lock().unwrap_or_else(PoisonError::into_inner) = Some(state);
}

pub(crate) fn health_snapshot() -> ScopeRegenHealth {
    let registry = registry();
    ScopeRegenHealth {
        pending: registry.len(),
        resume: *RESUME_STATE.lock().unwrap_or_else(PoisonError::into_inner),
        samples: registry
            .values()
            .take(HEALTH_SAMPLE_LIMIT)
            .cloned()
            .collect(),
    }
}

/// 跨进程也不重号：重启后 `SEQ` 从零数，时间戳把两次进程分开。
fn next_run_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{nanos:x}-{:x}", SEQ.fetch_add(1, Ordering::Relaxed))
}

fn record_id(refno: &str) -> String {
    let key = refno
        .chars()
        .map(|c| if c.is_ascii_digit() { c } else { '_' })
        .collect::<String>();
    format!("{SCOPE_REGEN_TABLE}:scope_{key}")
}

/// 失败摘要压成一行、截 2000 字（与账本 `last_error` 同一口径）。
fn summarize(error: &str) -> String {
    error
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(2000)
        .collect()
}

/// 落行 / 换上新一轮的身份。`started_at`、`failures`、`last_error` 沿用已有的那一行。
pub(crate) fn render_begin(run: &ScopeRun, root_count: usize) -> String {
    format!(
        "UPSERT {} SET refno = '{}', dbnum = {}, run_id = '{}', root_count = {root_count}, \
         failures = failures ?? 0, started_at = started_at ?? time::now(), updated_at = time::now();",
        record_id(&run.refno),
        escape_surql_str(&run.refno),
        run.dbnum,
        escape_surql_str(&run.run_id),
    )
}

/// `UPDATE` 而不是 `UPSERT`：行已被收口删掉（或换给了后来的一轮）时，迟到的失败不许把它造回来。
pub(crate) fn render_failure(run: &ScopeRun, error: &str) -> String {
    format!(
        "UPDATE {} SET failures = (failures ?? 0) + 1, last_error = '{}', updated_at = time::now() \
         WHERE run_id = '{}';",
        record_id(&run.refno),
        escape_surql_str(&summarize(error)),
        escape_surql_str(&run.run_id),
    )
}

pub(crate) fn render_finish(run: &ScopeRun) -> String {
    format!(
        "DELETE {} WHERE run_id = '{}';",
        record_id(&run.refno),
        escape_surql_str(&run.run_id)
    )
}

async fn execute_on(db: &Surreal<Any>, sql: String) -> anyhow::Result<()> {
    db.query(sql.as_str())
        .await
        .with_context(|| format!("scope regeneration record: {sql}"))?
        .check()
        .with_context(|| format!("scope regeneration record statement: {sql}"))?;
    Ok(())
}

/// 两格时刻逐格判 NONE 再转字符串（`type::string(NONE)` 在这个 fork 上直接报错，见 `write_state`）。
const TIME_COLUMNS: &str = "IF started_at = NONE THEN NONE ELSE type::string(started_at) END \
     AS started_at, \
     IF updated_at = NONE THEN NONE ELSE type::string(updated_at) END AS updated_at";

pub(crate) async fn load_all_on(db: &Surreal<Any>) -> anyhow::Result<Vec<PendingScope>> {
    let mut response = db
        .query(format!(
            "SELECT refno, dbnum, run_id, root_count, failures, last_error, {TIME_COLUMNS} \
             FROM {SCOPE_REGEN_TABLE} ORDER BY dbnum, refno;"
        ))
        .await
        .context("load pending scope regenerations")?
        .check()
        .context("load pending scope regenerations statement")?;
    response
        .take(0)
        .context("decode pending scope regenerations")
}

/// 强制重生成清理之前落行（只在摄入形态调）。同一范围已有一行（上一轮没收尾）就接着用：换上这一轮
/// 的 `run_id`，其余沿用——这片范围从那时起就没补齐过。
pub(crate) async fn begin(requested: RefnoEnum, root_count: usize) -> anyhow::Result<ScopeRun> {
    let run = ScopeRun {
        refno: requested.to_pdms_str(),
        dbnum: super::generation_root::refno_to_e3d(requested).dbno(),
        run_id: next_run_id(),
    };
    execute_on(&SUL_DB, render_begin(&run, root_count))
        .await
        .with_context(|| format!("范围 {} 的重生成留痕落不下来，未清理", run.refno))?;
    register(&run, root_count);
    Ok(run)
}

fn register(run: &ScopeRun, root_count: usize) {
    let now = chrono::Utc::now().to_rfc3339();
    let mut registry = registry();
    let scope = registry
        .entry(run.refno.clone())
        .or_insert_with(|| PendingScope {
            refno: run.refno.clone(),
            dbnum: run.dbnum,
            run_id: String::new(),
            root_count,
            failures: 0,
            last_error: None,
            started_at: Some(now.clone()),
            updated_at: None,
            in_flight: true,
        });
    scope.run_id = run.run_id.clone();
    scope.root_count = root_count;
    scope.updated_at = Some(now);
    scope.in_flight = true;
}

/// `force = false` 的同一范围请求接手一行：只接**没有一轮在跑**的（启动读回的、或上一轮已收口却没删成的）。
/// 在跑的那一轮清理可能还没做完，此刻判出的「凭证当前」替它作不了证。
pub(crate) fn adopt(requested: RefnoEnum) -> Option<ScopeRun> {
    let refno = requested.to_pdms_str();
    let registry = registry();
    let scope = registry.get(&refno).filter(|scope| !scope.in_flight)?;
    Some(ScopeRun {
        dbnum: scope.dbnum,
        run_id: scope.run_id.clone(),
        refno,
    })
}

/// 这一轮不再在跑：此后同一范围的 `force = false` 请求可以接手它留下的行。
fn release(run: &ScopeRun) {
    if let Some(scope) = registry().get_mut(&run.refno)
        && scope.run_id == run.run_id
    {
        scope.in_flight = false;
    }
}

/// 进程内那一条随删行一起摘；被后来的一轮换过身份的留着。
fn forget_run(run: &ScopeRun) {
    let mut registry = registry();
    if registry
        .get(&run.refno)
        .is_some_and(|scope| scope.run_id == run.run_id)
    {
        registry.remove(&run.refno);
    }
}

/// 一轮跑完的收口。每一根都成了 → 后台等这些根落盘，全部确认了才删行；有根没成 / 整轮报错 → 行留着、
/// 记下失败摘要。
pub(crate) async fn settle(run: ScopeRun, outcome: Result<&OnDemandModelResult, &anyhow::Error>) {
    release(&run);
    match outcome {
        Ok(result) if result.failed_roots.is_empty() => {
            let roots = result
                .generation_roots
                .iter()
                .filter_map(|root| crate::fast_model::e3d_model_service::parse_refno(root).ok())
                .collect::<Vec<_>>();
            tokio::spawn(finish_when_durable(run, roots));
        }
        Ok(result) => {
            let first = &result.failed_roots[0];
            let error = format!(
                "{} / {} 个生成根没生成出来，首个 {}: {}",
                result.failed_root_count,
                result.generation_root_count,
                first.generation_root,
                first.error
            );
            record_failure(&run, &error).await;
        }
        Err(error) => record_failure(&run, &format!("{error:#}")).await,
    }
}

async fn record_failure(run: &ScopeRun, error: &str) {
    let summary = summarize(error);
    if let Some(scope) = registry().get_mut(&run.refno)
        && scope.run_id == run.run_id
    {
        scope.failures += 1;
        scope.last_error = Some(summary.clone());
        scope.updated_at = Some(chrono::Utc::now().to_rfc3339());
    }
    if let Err(db_error) = execute_on(&SUL_DB, render_failure(run, &summary)).await {
        eprintln!(
            "[范围重生成] {} 的失败摘要没写进留痕（行仍在，重启照样续跑）: {db_error:#}",
            run.refno
        );
    }
}

/// 不在 write-behind 队列里的根（命中复用的、非持久档发布的）按已确认算——与任务等本批根刷盘同一口径。
async fn finish_when_durable(run: ScopeRun, roots: Vec<RefNo>) {
    let mut by_dbnum: BTreeMap<u32, Vec<RefNo>> = BTreeMap::new();
    for root in roots {
        by_dbnum.entry(root.dbno()).or_default().push(root);
    }
    for (dbnum, roots) in by_dbnum {
        let flush = crate::fast_model::model_write_behind::await_roots_durable(
            dbnum,
            &roots,
            super::batch_worker::DURABLE_FLUSH_BUDGET,
        )
        .await;
        if !flush.all_acked() {
            let abandoned = flush.abandoned();
            let error = format!(
                "dbnum={dbnum} 有 {} 个生成根没等到落盘确认（首个 {}），留痕不删",
                abandoned.len(),
                abandoned
                    .first()
                    .map(ToString::to_string)
                    .unwrap_or_default()
            );
            record_failure(&run, &error).await;
            return;
        }
    }
    match execute_on(&SUL_DB, render_finish(&run)).await {
        Ok(()) => forget_run(&run),
        Err(error) => eprintln!(
            "[范围重生成] {} 已补齐，但留痕删不掉（下次启动续跑命中凭证后再删）: {error:#}",
            run.refno
        ),
    }
}

/// 「清理模型」显式清掉 `requested` 之后：被它覆盖的留痕（同一节点，或祖先链上有它）一并撤掉——用户要
/// 这片范围没有模型，续跑不能把它生成回来。正在跑的那一轮不动，由它自己收口。
pub(crate) async fn discard_covered_by(requested: RefnoEnum, tree: Arc<DirectTreeService>) {
    let candidates = registry()
        .values()
        .filter(|scope| !scope.in_flight)
        .map(|scope| ScopeRun {
            refno: scope.refno.clone(),
            dbnum: scope.dbnum,
            run_id: scope.run_id.clone(),
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return;
    }
    let covered = tokio::task::spawn_blocking(move || {
        candidates
            .into_iter()
            .filter(|run| {
                super::model_root_ledger::parse_root(&run.refno).is_some_and(|refno| {
                    refno == requested
                        || tree
                            .ancestors(refno.refno())
                            .is_ok_and(|chain| chain.contains(&requested))
                })
            })
            .collect::<Vec<_>>()
    })
    .await;
    let covered = match covered {
        Ok(covered) => covered,
        Err(error) => {
            eprintln!(
                "[范围重生成] 清理 {} 之后核对被覆盖的留痕失败，留痕原样保留: {error}",
                requested.to_pdms_str()
            );
            return;
        }
    };
    for run in covered {
        match execute_on(&SUL_DB, render_finish(&run)).await {
            Ok(()) => forget_run(&run),
            Err(error) => eprintln!(
                "[范围重生成] 范围 {} 已被显式清理，但留痕删不掉: {error:#}",
                run.refno
            ),
        }
    }
}

/// 读回的行进登记表；本进程已有一轮认领了同一范围的，以它为准。
fn install_loaded(rows: Vec<PendingScope>) {
    let mut registry = registry();
    for row in rows {
        registry.entry(row.refno.clone()).or_insert(PendingScope {
            in_flight: false,
            ..row
        });
    }
}

/// 启动续跑上一进程没补齐的范围：读回全部留痕 → 等就绪 → 逐个以 `force = false` 跑同一范围（凭证当前的
/// 根直接命中，只补删掉没生成回来的），成了由 [`settle`] 删行。一次一个范围，不与按需请求抢并发。
pub(crate) async fn resume_pending(mgr: Arc<AiosDBManager>, tree: DirectTreeProvider) {
    if crate::options::data_face().is_read_through() {
        return;
    }
    set_resume_state("loading");
    let rows = match load_all_on(&SUL_DB).await {
        Ok(rows) => rows,
        Err(error) => {
            set_resume_state("load_failed");
            eprintln!("[范围重生成续跑] 读不出未完成留痕，本次启动不续跑: {error:#}");
            return;
        }
    };
    let mut queue = rows.iter().map(|row| row.refno.clone()).collect::<Vec<_>>();
    install_loaded(rows);
    if !queue.is_empty() {
        println!(
            "[范围重生成续跑] 上一进程留下 {} 个没补齐的范围，就绪后以 force=false 续跑",
            queue.len()
        );
    }
    for pass in 1..=RESUME_PASSES {
        if queue.is_empty() {
            break;
        }
        if pass > 1 {
            tokio::time::sleep(RESUME_RETRY_INTERVAL).await;
        }
        set_resume_state("waiting_for_readiness");
        wait_until_ready().await;
        set_resume_state("running");
        let mut later = Vec::new();
        for refno in queue {
            if resume_one(&mgr, &tree, &refno).await == Resume::Later {
                later.push(refno);
            }
        }
        queue = later;
    }
    if !queue.is_empty() {
        println!(
            "[范围重生成续跑] 试满 {RESUME_PASSES} 轮仍有 {} 个范围没了结，留痕保留（见 /health.model_scope_regen）",
            queue.len()
        );
    }
    set_resume_state("finished");
}

#[derive(Debug, PartialEq, Eq)]
enum Resume {
    /// 了结了：补齐（删行在后台等落盘）、已被别人收口、或失败已记进留痕、再试也一样。
    Done,
    /// 等一等再试。
    Later,
}

async fn resume_one(mgr: &AiosDBManager, tree: &DirectTreeProvider, refno: &str) -> Resume {
    let Some(scope) = registry()
        .get(refno)
        .filter(|scope| !scope.in_flight)
        .cloned()
    else {
        return Resume::Done;
    };
    let Some(requested) = super::model_root_ledger::parse_root(refno) else {
        eprintln!("[范围重生成续跑] 留痕里的 `{refno}` 不是 a/b refno，跳过");
        return Resume::Done;
    };
    match super::model_read_route::route_for(scope.dbnum).await {
        Ok(route) if route.is_database() => {}
        // 这个库此刻不在 rocksdb 上（在重建 / 刚被清过）：它回来之前没有持久面可补。
        Ok(_) => return Resume::Later,
        Err(error) => {
            eprintln!(
                "[范围重生成续跑] {refno} 所属库 {} 解不出模型路由: {error:#}",
                scope.dbnum
            );
            return Resume::Later;
        }
    }
    let roots = match scope_roots(tree, requested).await {
        Ok(roots) => roots,
        Err(error) if scope_element_is_gone(&error) => {
            println!("[范围重生成续跑] {refno} 已不在当前文件里，名下没有可补的模型，留痕撤掉");
            let run = ScopeRun {
                refno: scope.refno,
                dbnum: scope.dbnum,
                run_id: scope.run_id,
            };
            match execute_on(&SUL_DB, render_finish(&run)).await {
                Ok(()) => forget_run(&run),
                Err(error) => eprintln!("[范围重生成续跑] {refno} 留痕删不掉: {error:#}"),
            }
            return Resume::Done;
        }
        Err(error) => {
            eprintln!("[范围重生成续跑] {refno} 解不出生成根，稍后再试: {error:#}");
            return Resume::Later;
        }
    };
    match mgr
        .ensure_model_scope_generated_from_roots(requested, roots, false)
        .await
    {
        Ok(result) if result.failed_roots.is_empty() => {
            println!(
                "[范围重生成续跑] {refno} 补齐：{} 根中重新生成 {}、命中 {}",
                result.generation_root_count, result.generated_root_count, result.cached_root_count
            );
            Resume::Done
        }
        Ok(result) if result.failed_roots.iter().all(|failure| failure.retryable) => Resume::Later,
        Ok(result) => {
            println!(
                "[范围重生成续跑] {refno} 仍有 {} 根生成不出来，留痕保留（见 /health.model_scope_regen）",
                result.failed_root_count
            );
            Resume::Done
        }
        Err(error) if root_failure_is_retryable(&error) => Resume::Later,
        Err(error) => {
            eprintln!("[范围重生成续跑] {refno} 整片失败，留痕保留: {error:#}");
            Resume::Done
        }
    }
}

async fn scope_roots(
    tree: &DirectTreeProvider,
    requested: RefnoEnum,
) -> anyhow::Result<Vec<GenerationRoot>> {
    let service = tree.get().await?;
    let root = requested.refno();
    let unit_types = super::generation_root::configured_delivery_unit_types();
    tokio::task::spawn_blocking(move || service.generation_roots_in_subtree(root, &unit_types))
        .await
        .map_err(|error| anyhow::anyhow!("direct generation-root task failed: {error}"))?
}

fn scope_element_is_gone(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<DirectStoreError>(),
            Some(
                DirectStoreError::NoSuchElement { .. }
                    | DirectStoreError::DbNotInMdb { .. }
                    | DirectStoreError::InvalidRefNo { .. }
            )
        )
    })
}

/// 与 `/model/generate` 同一组门：初始化门放行生成、房间参照模型在位（房间归属开着时）；另等启动凭证
/// 复核落地——它排的 ModelOnly 窗口先认领各库的缺口，续跑补的是那之外被先删的洞。
async fn wait_until_ready() {
    let coordinator = super::initialization_phase::InitializationCoordinator::global();
    loop {
        let rooms_ready = !crate::options::room_membership()
            || crate::fast_model::room_resident::current_for_calculation().is_ok();
        if !coordinator.startup_reconcile_pending()
            && super::initialization_phase::require_model_generation().is_ok()
            && rooms_ready
        {
            return;
        }
        tokio::time::sleep(READINESS_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn database(label: &str) -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("model_scope_regen").use_db(label).await.unwrap();
        db
    }

    fn run(refno: &str, run_id: &str) -> ScopeRun {
        ScopeRun {
            refno: refno.into(),
            dbnum: 8000,
            run_id: run_id.into(),
        }
    }

    #[test]
    fn every_statement_names_the_scope_row_and_only_the_begin_creates_it() {
        let begin = render_begin(&run("24384/1234", "r1"), 3);
        assert!(
            begin.starts_with("UPSERT model_scope_regen:scope_24384_1234 SET"),
            "{begin}"
        );
        for kept in [
            "failures = failures ?? 0",
            "started_at = started_at ?? time::now()",
            "run_id = 'r1'",
            "root_count = 3",
        ] {
            assert!(begin.contains(kept), "{kept}: {begin}");
        }
        assert!(
            !begin.contains("last_error"),
            "新一轮不抹掉上一轮的失败摘要: {begin}"
        );

        let failure = render_failure(
            &run("24384/1234", "r1"),
            "第一行\n  第二行 'quoted' \\ back",
        );
        assert!(
            failure.starts_with("UPDATE model_scope_regen:scope_24384_1234 SET"),
            "{failure}"
        );
        assert!(failure.ends_with("WHERE run_id = 'r1';"), "{failure}");
        assert!(!failure.contains('\n'), "摘要压成一行: {failure}");
        assert!(failure.contains("\\'quoted\\'"), "{failure}");

        assert_eq!(
            render_finish(&run("24384/1234", "r1")),
            "DELETE model_scope_regen:scope_24384_1234 WHERE run_id = 'r1';"
        );
    }

    /// 留痕只归认领它的那一轮：后来的一轮换了身份，旧一轮迟到的失败与确认都碰不到它；收口删掉之后，
    /// 迟到的失败也不会把行造回来。
    #[tokio::test]
    async fn only_the_run_that_owns_the_row_can_settle_it() {
        let db = database("ownership").await;
        let first = run("24384/1234", "r1");
        execute_on(&db, render_begin(&first, 3)).await.unwrap();
        let rows = load_all_on(&db).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (
                rows[0].refno.as_str(),
                rows[0].dbnum,
                rows[0].run_id.as_str()
            ),
            ("24384/1234", 8000, "r1")
        );
        assert_eq!((rows[0].root_count, rows[0].failures), (3, 0));
        assert_eq!(rows[0].last_error, None);
        let started_at = rows[0].started_at.clone().expect("落行时刻");

        execute_on(&db, render_failure(&first, "几何坏了 'x'"))
            .await
            .unwrap();
        let rows = load_all_on(&db).await.unwrap();
        assert_eq!(rows[0].failures, 1);
        assert_eq!(rows[0].last_error.as_deref(), Some("几何坏了 'x'"));

        let second = run("24384/1234", "r2");
        execute_on(&db, render_begin(&second, 4)).await.unwrap();
        let rows = load_all_on(&db).await.unwrap();
        assert_eq!(rows[0].run_id, "r2");
        assert_eq!(rows[0].root_count, 4);
        assert_eq!(rows[0].failures, 1, "新一轮接着用这一行");
        assert_eq!(rows[0].last_error.as_deref(), Some("几何坏了 'x'"));
        assert_eq!(rows[0].started_at.as_deref(), Some(started_at.as_str()));

        execute_on(&db, render_failure(&first, "迟到"))
            .await
            .unwrap();
        execute_on(&db, render_finish(&first)).await.unwrap();
        let rows = load_all_on(&db).await.unwrap();
        assert_eq!(rows.len(), 1, "旧一轮删不掉新一轮的行");
        assert_eq!(rows[0].failures, 1, "旧一轮的失败不记到新一轮头上");

        execute_on(&db, render_finish(&second)).await.unwrap();
        assert!(load_all_on(&db).await.unwrap().is_empty());
        execute_on(&db, render_failure(&second, "迟到"))
            .await
            .unwrap();
        assert!(
            load_all_on(&db).await.unwrap().is_empty(),
            "收口之后迟到的失败不造行"
        );
    }

    /// 在跑的那一轮别人不接手；它收口之后（不论成败）才可接手。登记表的摘除同样只认这一轮。
    #[test]
    fn a_scope_is_adoptable_only_after_its_run_has_settled() {
        let requested = RefnoEnum::from("6913/1");
        let refno = requested.to_pdms_str();
        let first = ScopeRun {
            refno: refno.clone(),
            dbnum: 6913,
            run_id: "adopt-r1".into(),
        };
        register(&first, 2);
        assert_eq!(adopt(requested), None, "在跑的那一轮别人不接手");
        release(&first);
        assert_eq!(adopt(requested), Some(first.clone()));

        let second = ScopeRun {
            run_id: "adopt-r2".into(),
            ..first.clone()
        };
        register(&second, 2);
        assert_eq!(adopt(requested), None);
        release(&first);
        assert_eq!(adopt(requested), None, "旧一轮放不开新一轮");
        forget_run(&first);
        assert!(registry().contains_key(&refno), "旧一轮摘不掉新一轮");
        release(&second);
        assert_eq!(adopt(requested), Some(second.clone()));
        forget_run(&second);
        assert!(!registry().contains_key(&refno));
    }

    /// 启动读回的行一律可接手；本进程已有一轮认领了同一范围的，以它为准。
    #[test]
    fn loaded_rows_are_adoptable_but_never_override_a_run_in_this_process() {
        let live = ScopeRun {
            refno: "6913/20".into(),
            dbnum: 6913,
            run_id: "live".into(),
        };
        register(&live, 1);
        let row = |refno: &str| PendingScope {
            refno: refno.into(),
            dbnum: 6913,
            run_id: "loaded".into(),
            root_count: 1,
            failures: 2,
            last_error: Some("boom".into()),
            started_at: None,
            updated_at: None,
            in_flight: true,
        };
        install_loaded(vec![row("6913/20"), row("6913/21")]);
        assert_eq!(registry()["6913/20"].run_id, "live");
        assert_eq!(
            adopt(RefnoEnum::from("6913/21")).map(|run| run.run_id),
            Some("loaded".into())
        );
        forget_run(&live);
        forget_run(&ScopeRun {
            refno: "6913/21".into(),
            dbnum: 6913,
            run_id: "loaded".into(),
        });
    }

    /// 续跑与按需请求同一组门、同一条生成路径，且只用 `force = false`——续跑不再删一遍。
    #[test]
    fn the_startup_resume_waits_for_readiness_and_never_forces() {
        let source = include_str!("model_scope_regen.rs");
        let production = source.split_once("#[cfg(test)]").expect("tests follow").0;
        let resume = production
            .split_once("pub(crate) async fn resume_pending(")
            .expect("resume exists")
            .1;
        let ready = resume.find("wait_until_ready().await").expect("等就绪");
        let run = resume
            .find("resume_one(&mgr, &tree, &refno)")
            .expect("逐范围续跑");
        assert!(ready < run, "{resume}");
        assert!(
            production
                .contains(".ensure_model_scope_generated_from_roots(requested, roots, false)"),
            "续跑只用 force = false"
        );
        assert!(!production.contains("clear_model_scope("), "续跑不删");
        let gate = production
            .split_once("async fn wait_until_ready()")
            .expect("gate")
            .1;
        for required in [
            "startup_reconcile_pending()",
            "require_model_generation()",
            "room_membership()",
        ] {
            assert!(gate.contains(required), "{required}: {gate}");
        }
    }
}
