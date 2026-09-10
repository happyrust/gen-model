//! 每库双水位的一致性判决。
//!
//! 数据面和模型面保持异步；本模块只读取两边已经写下的承诺与实时根普查，产出
//! `in_sync / lagging / not_judged`。HTTP、健康检查和运维界面不得自行重拼判据
//! （ADR-060 D9/M8）。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::dbnum_state::DbnumState;
use crate::data_interface::model_update_pending::{MAX_ATTEMPTS, TABLE as MODEL_PENDING_TABLE};
use crate::data_interface::window_root_plan::SETTLED_ROOT_STATUSES;
use crate::options::DataFace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotJudgedReason {
    ReadThrough,
    DataWatermarkUnestablished,
    /// No roots and no valid authoritative empty-inventory completion receipt.
    NoGenerationRoots,
}

impl NotJudgedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadThrough => "read-through",
            Self::DataWatermarkUnestablished => "data_watermark_unestablished",
            Self::NoGenerationRoots => "no_generation_roots",
        }
    }
}

/// 生成根四格。`lazy = None` 表示尚未观察，不能解释成零。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootQuadrants {
    caught_up: u32,
    chasing: u32,
    abandoned: u32,
    lazy: Option<u32>,
    observed_at: Option<i32>,
}

impl RootQuadrants {
    pub fn caught_up(&self) -> u32 {
        self.caught_up
    }
    pub fn chasing(&self) -> u32 {
        self.chasing
    }
    pub fn abandoned(&self) -> u32 {
        self.abandoned
    }
    pub fn lazy(&self) -> Option<u32> {
        self.lazy
    }
    pub fn observed_at(&self) -> Option<i32> {
        self.observed_at
    }
}

/// 实时根普查。死信与 `http_422` 已从 `chasing` 中扣除并合入 `abandoned`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootCensus {
    settled: u32,
    chasing: u32,
    abandoned: u32,
    has_roots: bool,
    verified_completion: Option<i32>,
    completion_invalid: bool,
}

impl RootCensus {
    pub fn new(settled: u32, chasing: u32, abandoned: u32, has_roots: bool) -> Self {
        Self {
            settled,
            chasing,
            abandoned,
            has_roots,
            verified_completion: None,
            completion_invalid: false,
        }
    }

    pub fn settled(&self) -> u32 {
        self.settled
    }
    pub fn chasing(&self) -> u32 {
        self.chasing
    }
    pub fn abandoned(&self) -> u32 {
        self.abandoned
    }
    pub fn has_roots(&self) -> bool {
        self.has_roots
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsistencyVerdict {
    InSync {
        sesno: i32,
        time: Option<String>,
        caught_up_at: Option<String>,
        roots: RootQuadrants,
    },
    Lagging {
        applied: i32,
        model: i32,
        time: Option<String>,
        roots: RootQuadrants,
    },
    NotJudged {
        reason: NotJudgedReason,
        model: i32,
        time: Option<String>,
        lazy: Option<u32>,
        dead: u32,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VerdictColumns {
    pub model_sesno: i32,
    pub model_sesno_time: Option<String>,
    pub model_caught_up_at: Option<String>,
    pub model_verdict: &'static str,
    pub model_verdict_reason: Option<&'static str>,
    pub model_caught_up_roots: u32,
    pub model_chasing_roots: u32,
    pub model_dead_roots: u32,
    pub model_lazy_roots: Option<u32>,
}

/// 房间阶段与模型阶段并排暴露的持久证据。这里不重新计算房间归属，
/// 只转述 `dbnum_watermark` 上已经原子提交的第三枚水位与输入凭证。
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct RoomVerdictColumns {
    pub room_sesno: i32,
    pub room_sesno_time: Option<String>,
    pub room_settled_at: Option<String>,
    pub room_input_version: Option<super::dbnum_state::RoomInputVersion>,
    pub room_required_input: Option<super::dbnum_state::RoomInputVersion>,
    pub room_result_version: Option<String>,
    pub room_input_version_matches: bool,
}

impl From<&DbnumState> for RoomVerdictColumns {
    fn from(row: &DbnumState) -> Self {
        Self {
            room_sesno: row.room_sesno,
            room_sesno_time: row.room_sesno_time.clone(),
            room_settled_at: row.room_settled_at.clone(),
            room_input_version: row.room_input_version.clone(),
            room_required_input: row.room_required_input.clone(),
            room_result_version: row.room_result_version.clone(),
            room_input_version_matches: row.room_input_version.is_some()
                && row.room_input_version == row.room_required_input,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ConsistencySummary {
    pub judged: u32,
    pub in_sync: u32,
    pub lagging: u32,
    pub not_judged: u32,
    pub chasing_roots: u32,
    pub dead_roots: u32,
    pub lazy_roots: u32,
    pub lagging_samples: Vec<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct ConsistencyObservation {
    pub applied_sesno: i32,
    pub initialized: bool,
    pub verdict: ConsistencyVerdict,
    pub room: RoomVerdictColumns,
}

pub fn verdict(row: &DbnumState, census: &RootCensus, data_face: DataFace) -> ConsistencyVerdict {
    if data_face.is_read_through() {
        return ConsistencyVerdict::NotJudged {
            reason: NotJudgedReason::ReadThrough,
            model: row.model_sesno,
            time: row.model_sesno_time.clone(),
            lazy: row.model_lazy_roots,
            dead: census.abandoned,
        };
    }
    if row.applied_sesno <= 0 {
        return ConsistencyVerdict::NotJudged {
            reason: NotJudgedReason::DataWatermarkUnestablished,
            model: row.model_sesno,
            time: row.model_sesno_time.clone(),
            lazy: row.model_lazy_roots,
            dead: census.abandoned,
        };
    }
    let roots = RootQuadrants {
        caught_up: census.settled,
        chasing: census.chasing,
        abandoned: census.abandoned,
        lazy: row.model_lazy_roots,
        observed_at: row.model_roots_observed_at,
    };
    if row.model_sesno == row.applied_sesno
        && census.chasing == 0
        && census.abandoned == 0
        && !census.completion_invalid
        && (census.has_roots() || census.verified_completion == Some(row.applied_sesno))
    {
        ConsistencyVerdict::InSync {
            sesno: row.model_sesno,
            time: row.model_sesno_time.clone(),
            caught_up_at: row.model_settled_at.clone(),
            roots,
        }
    } else if !census.has_roots() {
        // Table emptiness is not an authoritative inventory. Keep it unjudged
        // until the task commits a source-bound verified-empty receipt.
        ConsistencyVerdict::NotJudged {
            reason: NotJudgedReason::NoGenerationRoots,
            model: row.model_sesno,
            time: row.model_sesno_time.clone(),
            lazy: row.model_lazy_roots,
            dead: census.abandoned,
        }
    } else {
        ConsistencyVerdict::Lagging {
            applied: row.applied_sesno,
            model: row.model_sesno,
            time: row.model_sesno_time.clone(),
            roots,
        }
    }
}

impl ConsistencyVerdict {
    pub fn is_in_sync(&self) -> bool {
        matches!(self, Self::InSync { .. })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InSync { .. } => "in_sync",
            Self::Lagging { .. } => "lagging",
            Self::NotJudged { .. } => "not_judged",
        }
    }

    pub fn caught_up_at(&self) -> Option<&str> {
        match self {
            Self::InSync { caught_up_at, .. } => caught_up_at.as_deref(),
            Self::Lagging { .. } | Self::NotJudged { .. } => None,
        }
    }

    pub fn roots(&self) -> Option<&RootQuadrants> {
        match self {
            Self::InSync { roots, .. } | Self::Lagging { roots, .. } => Some(roots),
            Self::NotJudged { .. } => None,
        }
    }

    pub fn describe(&self, dbnum: u32) -> String {
        match self {
            Self::InSync { sesno, roots, .. } => format!(
                "dbnum={dbnum} 数据与模型一致到 {sesno}（已追平 {}，已放弃 {}，懒生成 {}）",
                roots.caught_up,
                roots.abandoned,
                roots
                    .lazy
                    .map_or_else(|| "未观察".to_owned(), |n| n.to_string())
            ),
            Self::Lagging {
                applied,
                model,
                roots,
                ..
            } => format!(
                "dbnum={dbnum} 模型在追赶：数据水位 {applied}，模型水位 {model}，还在赶 {}，已放弃 {}",
                roots.chasing, roots.abandoned
            ),
            Self::NotJudged {
                reason: NotJudgedReason::ReadThrough,
                ..
            } => {
                format!("dbnum={dbnum} 双水位不判：read-through 不建立数据水位")
            }
            Self::NotJudged {
                reason: NotJudgedReason::DataWatermarkUnestablished,
                ..
            } => {
                format!("dbnum={dbnum} 双水位不判：数据水位未建立")
            }
            Self::NotJudged {
                reason: NotJudgedReason::NoGenerationRoots,
                ..
            } => {
                format!("dbnum={dbnum} 双水位不判：库里没有生成根，尚无有效的空模型完成凭证")
            }
        }
    }

    pub fn columns(&self) -> VerdictColumns {
        let model_caught_up_at = self.caught_up_at().map(str::to_owned);
        let (state, reason, model, time, roots, lazy, dead) = match self {
            Self::InSync {
                sesno, time, roots, ..
            } => (
                "in_sync",
                None,
                *sesno,
                time.clone(),
                Some(roots),
                roots.lazy,
                roots.abandoned,
            ),
            Self::Lagging {
                model, time, roots, ..
            } => (
                "lagging",
                None,
                *model,
                time.clone(),
                Some(roots),
                roots.lazy,
                roots.abandoned,
            ),
            Self::NotJudged {
                reason,
                model,
                time,
                lazy,
                dead,
            } => (
                "not_judged",
                Some(reason.as_str()),
                *model,
                time.clone(),
                None,
                *lazy,
                *dead,
            ),
        };
        VerdictColumns {
            model_sesno: model,
            model_sesno_time: time,
            model_caught_up_at,
            model_verdict: state,
            model_verdict_reason: reason,
            model_caught_up_roots: roots.map_or(0, |r| r.caught_up),
            model_chasing_roots: roots.map_or(0, |r| r.chasing),
            model_dead_roots: roots.map_or(dead, |r| r.abandoned),
            model_lazy_roots: roots.and_then(|r| r.lazy).or(lazy),
        }
    }
}

#[derive(Debug, Deserialize)]
struct WatermarkRow {
    dbnum: u32,
    #[serde(default)]
    db_type: Option<String>,
    #[serde(default)]
    applied_sesno: Option<i32>,
    #[serde(default)]
    model_sesno: Option<i32>,
    #[serde(default)]
    model_sesno_time: Option<String>,
    #[serde(default)]
    model_settled_at: Option<String>,
    #[serde(default)]
    room_sesno: Option<i32>,
    #[serde(default)]
    room_sesno_time: Option<String>,
    #[serde(default)]
    room_settled_at: Option<String>,
    #[serde(default)]
    room_input_version: Option<super::dbnum_state::RoomInputVersion>,
    #[serde(default)]
    room_required_input: Option<super::dbnum_state::RoomInputVersion>,
    #[serde(default)]
    room_result_version: Option<String>,
    #[serde(default)]
    model_lazy_roots: Option<u32>,
    #[serde(default)]
    model_dead_roots: Option<u32>,
    #[serde(default)]
    model_roots_observed_at: Option<i32>,
}

impl From<WatermarkRow> for DbnumState {
    fn from(row: WatermarkRow) -> Self {
        let applied_sesno = row.applied_sesno.unwrap_or_default();
        Self {
            dbnum: row.dbnum,
            db_type: row.db_type.unwrap_or_default(),
            applied_sesno,
            model_sesno: row.model_sesno.unwrap_or_default(),
            model_sesno_time: row.model_sesno_time,
            model_settled_at: row.model_settled_at,
            room_sesno: row.room_sesno.unwrap_or_default(),
            room_sesno_time: row
                .room_sesno_time
                .filter(|_| row.room_sesno.unwrap_or_default() > 0),
            room_settled_at: row.room_settled_at,
            room_input_version: row.room_input_version,
            room_required_input: row.room_required_input,
            room_result_version: row.room_result_version,
            model_lazy_roots: row.model_lazy_roots,
            model_dead_roots: row.model_dead_roots,
            model_roots_observed_at: row.model_roots_observed_at,
            initialized: applied_sesno > 0,
            ..Default::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct RootRow {
    dbnum: u32,
    root: String,
}

/// 一根在 `gen_root` 上的普查视图。整表只扫一遍：全 AMS 12 万根时这张表一遍要 4–6 s
/// （行里带着 `published_model_target` 等大字段，逐行反序列化是主要开销），原先按
/// settled / blocking / http_422 三个谓词各扫一遍就是 15 s，每 30 s 一次，把 SurrealDB
/// 的一整核长期占满。谓词照 [`settled_root_predicate`] /
/// [`blocking_root_predicate`] 的口径在进程内判。
///
/// [`settled_root_predicate`]: crate::data_interface::model_watermark::settled_root_predicate
/// [`blocking_root_predicate`]: crate::data_interface::model_watermark::blocking_root_predicate
#[derive(Debug, Deserialize)]
struct RootStateRow {
    dbnum: u32,
    root: String,
    status: Option<String>,
    source_end_sesno: Option<i64>,
}

impl RootStateRow {
    fn settled(&self) -> bool {
        let status = self.status.as_deref().unwrap_or("");
        SETTLED_ROOT_STATUSES.contains(&status) && self.source_end_sesno.unwrap_or(0) > 0
    }
    fn refused(&self) -> bool {
        self.status.as_deref().unwrap_or("") == "http_422"
    }
}

fn root_key(value: &str) -> String {
    value.trim().replace('_', "/")
}

fn render_root_census_bulk() -> String {
    format!(
        "SELECT dbnum, record::id(id) AS root, status, source_end_sesno FROM gen_root WHERE dbnum != NONE;\n\
         SELECT dbnum, target_refno AS root FROM {MODEL_PENDING_TABLE} WHERE dbnum != NONE AND (attempts ?: 0) >= {MAX_ATTEMPTS} \
         AND action NOT IN ['room_recalc_element','room_recalc_panel'];"
    )
}

type CensusSnapshot = (BTreeMap<u32, DbnumState>, BTreeMap<u32, RootCensus>);

async fn watermark_states(db: &Surreal<Any>) -> anyhow::Result<BTreeMap<u32, DbnumState>> {
    let mut response = db
        .query(
            "SELECT dbnum, db_type, applied_sesno, model_sesno, model_sesno_time, \
             room_sesno, room_sesno_time, room_input_version, room_required_input, room_result_version, \
             IF room_settled_at != NONE THEN type::string(room_settled_at) ELSE NONE END AS room_settled_at, \
             IF model_settled_at != NONE THEN type::string(model_settled_at) ELSE NONE END AS model_settled_at, \
             model_lazy_roots, model_dead_roots, model_roots_observed_at FROM dbnum_watermark;",
        )
        .await
        .context("read dbnum consistency watermarks")?
        .check()
        .context("check dbnum consistency watermark query")?;
    let rows: Vec<WatermarkRow> = response.take(0).context("decode consistency watermarks")?;
    Ok(rows
        .into_iter()
        .map(|row| (row.dbnum, row.into()))
        .collect())
}

async fn states_and_census(db: &Surreal<Any>) -> anyhow::Result<CensusSnapshot> {
    let mut response = db.query(format!(
        "SELECT dbnum, db_type, applied_sesno, model_sesno, model_sesno_time, \
         room_sesno, room_sesno_time, room_input_version, room_required_input, room_result_version, \
         IF room_settled_at != NONE THEN type::string(room_settled_at) ELSE NONE END AS room_settled_at, \
         IF model_settled_at != NONE THEN type::string(model_settled_at) ELSE NONE END AS model_settled_at, \
         model_lazy_roots, \
         model_dead_roots, model_roots_observed_at FROM dbnum_watermark;\n{}",
        render_root_census_bulk()
    )).await.context("read dbnum consistency inputs")?.check().context("check dbnum consistency queries")?;
    let rows: Vec<WatermarkRow> = response.take(0).context("decode consistency watermarks")?;
    let roots: Vec<RootStateRow> = response.take(1).context("decode root census")?;
    let dead: Vec<RootRow> = response.take(2).context("decode dead-letter root census")?;

    let states: BTreeMap<u32, DbnumState> = rows
        .into_iter()
        .map(|row| (row.dbnum, row.into()))
        .collect();
    let mut census = BTreeMap::<u32, RootCensus>::new();
    let mut blockers = BTreeMap::<u32, BTreeSet<String>>::new();
    let mut refused = Vec::<RootRow>::new();
    for row in roots {
        let item = census.entry(row.dbnum).or_default();
        item.has_roots = true;
        if row.settled() {
            item.settled += 1;
        } else if row.refused() {
            refused.push(RootRow {
                dbnum: row.dbnum,
                root: row.root,
            });
        } else {
            blockers
                .entry(row.dbnum)
                .or_default()
                .insert(root_key(&row.root));
        }
    }
    let mut abandoned = BTreeMap::<u32, BTreeSet<String>>::new();
    for row in refused.into_iter().chain(dead) {
        abandoned
            .entry(row.dbnum)
            .or_default()
            .insert(root_key(&row.root));
    }
    for (dbnum, dead_roots) in abandoned {
        let item = census.entry(dbnum).or_default();
        item.has_roots = item.has_roots || !dead_roots.is_empty();
        item.abandoned = dead_roots.len() as u32;
        if let Some(raw) = blockers.get_mut(&dbnum) {
            raw.retain(|root| !dead_roots.contains(root));
        }
    }
    for (dbnum, roots) in blockers {
        census.entry(dbnum).or_default().chasing = roots.len() as u32;
    }
    #[derive(Deserialize)]
    struct ReceiptRow {
        dbnum: u32,
        end_sesno: i32,
        model_completion: String,
    }
    // 收据住在水位行上（`model_completion::receipt_row`），不再寄居在文件轮次那一行。
    let receipts: Vec<ReceiptRow> = db
        .query(
            "SELECT dbnum, model_completion_sesno AS end_sesno, model_completion \
         FROM dbnum_watermark WHERE model_completion != NONE AND model_completion_sesno != NONE;",
        )
        .await?
        .check()?
        .take(0)?;
    for row in receipts {
        if states
            .get(&row.dbnum)
            .is_none_or(|state| state.model_sesno != row.end_sesno)
        {
            continue;
        }
        let valid = crate::data_interface::model_completion::validated_receipt(
            Some(&row.model_completion),
            row.dbnum,
            row.end_sesno,
        )
        .is_some();
        let counts = census.entry(row.dbnum).or_default();
        counts.verified_completion = valid.then_some(row.end_sesno);
        counts.completion_invalid = !valid;
    }
    Ok((states, census))
}

struct CensusCache {
    taken: Instant,
    value: Arc<BTreeMap<u32, RootCensus>>,
}
static CENSUS_CACHE: Mutex<Option<CensusCache>> = Mutex::new(None);
static CENSUS_REFRESH: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
/// 比 `/health` 的 30 s 采样短一点，让采样器每次都拿到新样本，其余调用方共用它。
const CENSUS_CACHE_TTL: Duration = Duration::from_secs(20);

pub(crate) fn invalidate_census() {
    *CENSUS_CACHE.lock().unwrap() = None;
}

/// 生产路径共用一份普查。`/health` 的采样器、增量闸门（F6 扫描门）、数据批次收尾、
/// 预览接口各自都会来问一遍，每一问都是 `gen_root` 整表一扫（见 [`RootStateRow`]）。
/// 只对全局 `SUL_DB` 缓存；测试夹具与旁路连接直接查，互不串账。
async fn states_and_census_cached(
    db: &Surreal<Any>,
    cache: &Mutex<Option<CensusCache>>,
    refresh: &tokio::sync::Mutex<()>,
) -> anyhow::Result<CensusSnapshot> {
    let fresh = |cache: &Option<CensusCache>| {
        cache
            .as_ref()
            .filter(|c| c.taken.elapsed() < CENSUS_CACHE_TTL)
            .map(|c| c.value.clone())
    };
    let cached = {
        let guard = cache.lock().unwrap();
        fresh(&guard)
    };
    if let Some(census) = cached {
        return Ok((watermark_states(db).await?, census.as_ref().clone()));
    }
    let _refresh = refresh.lock().await;
    let cached = {
        let guard = cache.lock().unwrap();
        fresh(&guard)
    };
    if let Some(census) = cached {
        return Ok((watermark_states(db).await?, census.as_ref().clone()));
    }
    let (_, census) = states_and_census(db).await?;
    // The census can take several seconds on a large project. Read the cheap
    // watermarks afterwards so the response cannot report a stale data target.
    let states = watermark_states(db).await?;
    *cache.lock().unwrap() = Some(CensusCache {
        taken: Instant::now(),
        value: Arc::new(census.clone()),
    });
    Ok((states, census))
}

async fn states_and_census_shared(db: &Surreal<Any>) -> anyhow::Result<CensusSnapshot> {
    if !std::ptr::eq(db, &*aios_core::SUL_DB) {
        return states_and_census(db).await;
    }
    states_and_census_cached(db, &CENSUS_CACHE, &CENSUS_REFRESH).await
}

pub async fn verdict_for(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<ConsistencyVerdict> {
    let (states, census) = states_and_census_shared(db).await?;
    let row = states.get(&dbnum).cloned().unwrap_or_else(|| DbnumState {
        dbnum,
        ..Default::default()
    });
    Ok(verdict(
        &row,
        census.get(&dbnum).unwrap_or(&RootCensus::default()),
        crate::options::data_face(),
    ))
}

pub async fn verdicts_for_all(
    db: &Surreal<Any>,
    dbnums: &BTreeSet<u32>,
) -> anyhow::Result<BTreeMap<u32, ConsistencyVerdict>> {
    Ok(observations_for_all(db, dbnums)
        .await?
        .into_iter()
        .map(|(dbnum, observation)| (dbnum, observation.verdict))
        .collect())
}

pub(crate) async fn observations_for_all(
    db: &Surreal<Any>,
    dbnums: &BTreeSet<u32>,
) -> anyhow::Result<BTreeMap<u32, ConsistencyObservation>> {
    let (states, census) = states_and_census_shared(db).await?;
    let face = crate::options::data_face();
    Ok(dbnums
        .iter()
        .copied()
        .map(|dbnum| {
            let row = states.get(&dbnum).cloned().unwrap_or_else(|| DbnumState {
                dbnum,
                ..Default::default()
            });
            let result = verdict(
                &row,
                census.get(&dbnum).unwrap_or(&RootCensus::default()),
                face,
            );
            (
                dbnum,
                ConsistencyObservation {
                    applied_sesno: row.applied_sesno,
                    initialized: row.initialized,
                    verdict: result,
                    room: RoomVerdictColumns::from(&row),
                },
            )
        })
        .collect())
}

fn summarize(verdicts: &BTreeMap<u32, ConsistencyVerdict>) -> ConsistencySummary {
    let mut summary = ConsistencySummary::default();
    for (&dbnum, verdict) in verdicts {
        match verdict {
            ConsistencyVerdict::InSync { roots, .. } => {
                summary.judged += 1;
                summary.in_sync += 1;
                summary.chasing_roots += roots.chasing;
                summary.dead_roots += roots.abandoned;
                summary.lazy_roots += roots.lazy.unwrap_or_default();
            }
            ConsistencyVerdict::Lagging { roots, .. } => {
                summary.judged += 1;
                summary.lagging += 1;
                summary.chasing_roots += roots.chasing;
                summary.dead_roots += roots.abandoned;
                summary.lazy_roots += roots.lazy.unwrap_or_default();
                if summary.lagging_samples.len() < 5 {
                    summary.lagging_samples.push(dbnum);
                }
            }
            ConsistencyVerdict::NotJudged { dead, lazy, .. } => {
                summary.not_judged += 1;
                summary.dead_roots += *dead;
                summary.lazy_roots += lazy.unwrap_or_default();
            }
        }
    }
    summary
}

fn summarize_states(
    states: &BTreeMap<u32, DbnumState>,
    census: &BTreeMap<u32, RootCensus>,
    face: DataFace,
    admits: impl Fn(u32) -> bool,
) -> ConsistencySummary {
    let verdicts = states
        .iter()
        .filter(|(dbnum, row)| row.db_type.eq_ignore_ascii_case("DESI") && admits(**dbnum))
        .map(|(&dbnum, row)| {
            (
                dbnum,
                verdict(
                    row,
                    census.get(&dbnum).unwrap_or(&RootCensus::default()),
                    face,
                ),
            )
        })
        .collect();
    summarize(&verdicts)
}

pub async fn health_summary(db: &Surreal<Any>) -> anyhow::Result<ConsistencySummary> {
    let (states, census) = states_and_census_shared(db).await?;
    // 模型水位只描述会产出 Plant 模型的设计库；健康聚合还必须遵守本进程的
    // watch 范围，否则一个 `--watch-dbnum 8000` 实例会被别的元数据库制造假滞后。
    Ok(summarize_states(
        &states,
        &census,
        crate::options::data_face(),
        crate::data_interface::watch_scope::admits,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(applied: i32, model: i32) -> DbnumState {
        DbnumState {
            dbnum: 8000,
            applied_sesno: applied,
            model_sesno: model,
            ..Default::default()
        }
    }

    #[test]
    fn pure_verdict_has_three_states_and_failed_roots_block_in_sync() {
        assert!(matches!(
            verdict(
                &row(26, 26),
                &RootCensus::new(10, 0, 2, true),
                DataFace::Ingest
            ),
            ConsistencyVerdict::Lagging { .. }
        ));
        assert!(matches!(
            verdict(
                &row(26, 26),
                &RootCensus::new(10, 0, 0, true),
                DataFace::Ingest
            ),
            ConsistencyVerdict::InSync { .. }
        ));
        assert!(matches!(
            verdict(
                &row(26, 20),
                &RootCensus::new(9, 1, 0, true),
                DataFace::Ingest
            ),
            ConsistencyVerdict::Lagging { .. }
        ));
        assert!(matches!(
            verdict(&row(0, 0), &RootCensus::default(), DataFace::Ingest),
            ConsistencyVerdict::NotJudged {
                reason: NotJudgedReason::DataWatermarkUnestablished,
                ..
            }
        ));
        assert!(matches!(
            verdict(&row(26, 0), &RootCensus::default(), DataFace::ReadThrough),
            ConsistencyVerdict::NotJudged {
                reason: NotJudgedReason::ReadThrough,
                ..
            }
        ));
        // 零根库（只有 WORL + SITE 之类）：数据水位在、模型水位 0、`gen_root` 空——
        // 没有东西在赶，不是「追赶」。有根在赶的库照旧 lagging（上面 8000 那条）。
        let no_roots = verdict(&row(4, 0), &RootCensus::default(), DataFace::Ingest);
        assert!(matches!(
            no_roots,
            ConsistencyVerdict::NotJudged {
                reason: NotJudgedReason::NoGenerationRoots,
                ..
            }
        ));
        assert_eq!(no_roots.as_str(), "not_judged");
        assert_eq!(
            no_roots.columns().model_verdict_reason,
            Some("no_generation_roots")
        );
        assert!(no_roots.describe(5522).contains("空模型完成凭证"));
        assert!(
            matches!(
                verdict(
                    &row(4, 0),
                    &RootCensus::new(0, 0, 1, true),
                    DataFace::Ingest
                ),
                ConsistencyVerdict::Lagging { .. }
            ),
            "有过根（哪怕全放弃了）就还是模型面的事，照旧 lagging"
        );

        let mut caught_up = row(26, 26);
        caught_up.model_settled_at = Some("2026-09-05T17:01:43Z".into());
        let completed_empty = RootCensus {
            verified_completion: Some(26),
            ..RootCensus::default()
        };
        let caught_up = verdict(&caught_up, &completed_empty, DataFace::Ingest);
        assert_eq!(caught_up.caught_up_at(), Some("2026-09-05T17:01:43Z"));
        assert_eq!(caught_up.as_str(), "in_sync");
        assert_eq!(
            verdict(&row(26, 25), &RootCensus::default(), DataFace::Ingest).caught_up_at(),
            None
        );
    }

    #[tokio::test]
    async fn bulk_census_separates_abandoned_roots_but_keeps_strict_verdict_lagging() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("dual_watermark_census")
            .await
            .unwrap();
        db.query(format!(
            "CREATE dbnum_watermark:8000 SET dbnum=8000, db_type='DESI', applied_sesno=26, model_sesno=26;\
             CREATE dbnum_watermark:8001 SET dbnum=8001, db_type='DESI', applied_sesno=20, model_sesno=10;\
             CREATE dbnum_watermark:8002 SET dbnum=8002, db_type='DESI', applied_sesno=0, model_sesno=0;\
             CREATE gen_root:24384_1 SET dbnum=8000, status='Generated', source_end_sesno=26;\
             CREATE gen_root:24384_2 SET dbnum=8000, status='failed', source_end_sesno=0;\
             CREATE gen_root:24384_3 SET dbnum=8000, status='http_422', source_end_sesno=0;\
             CREATE {MODEL_PENDING_TABLE}:regen_root_24384_2 SET dbnum=8000, target_refno='24384/2', attempts={MAX_ATTEMPTS};\
             CREATE gen_root:24385_1 SET dbnum=8001, status='pending', source_end_sesno=0;"
        )).await.unwrap().check().unwrap();

        let (states, census) = states_and_census(&db).await.unwrap();
        assert_eq!(census[&8000], RootCensus::new(1, 0, 2, true));
        assert_eq!(census[&8001], RootCensus::new(0, 1, 0, true));
        let verdicts = states
            .iter()
            .map(|(&dbnum, row)| {
                (
                    dbnum,
                    verdict(
                        row,
                        census.get(&dbnum).unwrap_or(&RootCensus::default()),
                        DataFace::Ingest,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert!(matches!(
            verdicts[&8000],
            ConsistencyVerdict::Lagging { .. }
        ));
        assert!(matches!(
            verdicts[&8001],
            ConsistencyVerdict::Lagging { .. }
        ));
        assert!(matches!(
            verdicts[&8002],
            ConsistencyVerdict::NotJudged { .. }
        ));
    }

    #[tokio::test]
    async fn cached_root_census_never_caches_watermarks() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("fresh_watermarks_cached_census")
            .await
            .unwrap();
        db.query(
            "CREATE dbnum_watermark:8000 SET dbnum=8000, db_type='DESI', applied_sesno=26, model_sesno=26;\
             CREATE gen_root:24384_1 SET dbnum=8000, status='Generated', source_end_sesno=26;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let cache = Mutex::new(None);
        let refresh = tokio::sync::Mutex::new(());
        let (first, census) = states_and_census_cached(&db, &cache, &refresh)
            .await
            .unwrap();
        assert_eq!(first[&8000].applied_sesno, 26);
        assert_eq!(census[&8000].settled, 1);

        db.query("UPDATE dbnum_watermark:8000 SET applied_sesno=28, model_sesno=26;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let (second, cached_census) = states_and_census_cached(&db, &cache, &refresh)
            .await
            .unwrap();
        assert_eq!(second[&8000].applied_sesno, 28);
        assert_eq!(second[&8000].model_sesno, 26);
        assert_eq!(cached_census, census);
        assert!(matches!(
            verdict(&second[&8000], &cached_census[&8000], DataFace::Ingest),
            ConsistencyVerdict::Lagging { .. }
        ));
    }

    #[tokio::test]
    async fn health_summary_ignores_non_model_databases() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("dual_watermark_health_scope")
            .await
            .unwrap();
        db.query(
            "CREATE dbnum_watermark:8000 SET dbnum=8000, db_type='DESI', applied_sesno=26, model_sesno=26;\
             CREATE dbnum_watermark:5100 SET dbnum=5100, db_type='DICT', applied_sesno=12, model_sesno=0;\
             CREATE gen_root:example SET dbnum=8000, status='Generated', source_end_sesno=26;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let (states, census) = states_and_census(&db).await.unwrap();
        let summary = summarize_states(&states, &census, DataFace::Ingest, |_| true);
        assert_eq!(summary.judged, 1);
        assert_eq!(summary.in_sync, 1);
        assert_eq!(summary.lagging, 0);
    }
}
