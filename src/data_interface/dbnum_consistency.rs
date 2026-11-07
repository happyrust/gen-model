//! 每库双水位的一致性判决。
//!
//! 数据面和模型面保持异步；本模块只读取两边已经写下的承诺（水位行）与**进程内的实时根普查**
//! （[`crate::data_interface::model_root_ledger`]，ADR-079），产出 `in_sync / lagging / not_judged`。
//! HTTP、健康检查和运维界面不得自行重拼判据（ADR-060 D9/M8）。
//!
//! 2026-09-15（ADR-079）之前普查是 `gen_root` 整表一扫（全 AMS 12 万根 4–6 s，于是要一份 20 s
//! 的缓存）；`gen_root` 退役后普查读账本，零 I/O，缓存与它的作废口一起退了。重启后账本为空
//! ——那时四格里只剩水位能说的话：`model_sesno == applied_sesno` 就是追平，否则就是在赶。

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::dbnum_state::DbnumState;
use crate::data_interface::model_root_ledger::LedgerCensus;
use crate::options::DataFace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotJudgedReason {
    ReadThrough,
    DataWatermarkUnestablished,
}

impl NotJudgedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadThrough => "read-through",
            Self::DataWatermarkUnestablished => "data_watermark_unestablished",
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

/// 实时根普查（进程内账本的三格，ADR-079）。`http_422` 一类永久放弃的根在 `abandoned`，
/// 不在 `chasing`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootCensus {
    settled: u32,
    chasing: u32,
    abandoned: u32,
}

impl RootCensus {
    pub fn new(settled: u32, chasing: u32, abandoned: u32) -> Self {
        Self {
            settled,
            chasing,
            abandoned,
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
}

impl From<LedgerCensus> for RootCensus {
    fn from(census: LedgerCensus) -> Self {
        Self::new(census.settled, census.chasing, census.abandoned)
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
    // 零根库（只有 WORL + SITE 之类）没有「验证过的空清单收据」可看了（ADR-072 D4）：
    // 它的完成态由水位行自己说——`model_sesno` 只由任务收尾那一笔盖，而且只在这一版的根
    // （零个）全落进去时盖，所以 `model_sesno == applied_sesno` 就是那张收据。
    //
    // 水位不等就是在赶（ADR-079）：库里不再有逐根的行可以「不判」——`model_sesno` 是模型面唯一
    // 的持久事实，它没盖到数据水位，欠的就是 `model_sesno → applied_sesno` 这一窗，重启后
    // 启动核对排的也正是它。
    if row.model_sesno == row.applied_sesno && census.chasing == 0 && census.abandoned == 0 {
        ConsistencyVerdict::InSync {
            sesno: row.model_sesno,
            time: row.model_sesno_time.clone(),
            caught_up_at: row.model_settled_at.clone(),
            roots,
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

/// 任务收尾那一刻的判决（ADR-072 §2.2「`verdict_for` 出任务」）：水位读**镜像**里的这一行，
/// 根普查用**本任务的单元清单**——任务不回问库自己刚写的东西（ADR-071 D1）。
///
/// 口径与 [`verdict`] 同一份纯函数：本任务生成成功的根算追平、失败的算在赶；「已放弃」
/// 那一格随 pending 死信退了，恒 0。零单元的任务（这一窗没触到任何根）以水位行说话：
/// `model_sesno > 0` 就是这个库有过完成态。
pub fn task_verdict(
    row: &DbnumState,
    units: &[crate::data_interface::manual_update::ModelUnitResult],
    data_face: DataFace,
) -> ConsistencyVerdict {
    use crate::data_interface::manual_update::UnitGenStatus;
    let generated = units
        .iter()
        .filter(|unit| unit.status == UnitGenStatus::Generated)
        .count() as u32;
    let chasing = units.len() as u32 - generated;
    let census = RootCensus::new(generated, chasing, 0);
    verdict(row, &census, data_face)
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
            model_lazy_roots: row.model_lazy_roots,
            model_dead_roots: row.model_dead_roots,
            model_roots_observed_at: row.model_roots_observed_at,
            initialized: applied_sesno > 0,
            ..Default::default()
        }
    }
}

type CensusSnapshot = (BTreeMap<u32, DbnumState>, BTreeMap<u32, RootCensus>);

async fn watermark_states(db: &Surreal<Any>) -> anyhow::Result<BTreeMap<u32, DbnumState>> {
    let mut response = db
        .query(
            "SELECT dbnum, db_type, applied_sesno, model_sesno, model_sesno_time, \
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

/// 水位行从库里读（一张小表一遍），根普查从进程内账本取（ADR-079，零 I/O）。
///
/// 账本是进程全局的，不分连接：测试夹具用 `mem://` 时四格来自同一份账本，各测试要用自己的
/// dbnum，别互相踩。「验证过的空清单收据」那一格随 `CompletionReceipt` 退了（ADR-072 D4）：
/// 零根库的完成态由水位行自己说——任务只在这一版的根全落进去时才盖 `model_sesno`，见 [`verdict`]。
async fn states_and_census(db: &Surreal<Any>) -> anyhow::Result<CensusSnapshot> {
    let states = watermark_states(db).await?;
    let census = crate::data_interface::model_root_ledger::census_all()
        .into_iter()
        .map(|(dbnum, census)| (dbnum, RootCensus::from(census)))
        .collect();
    Ok((states, census))
}

pub async fn verdict_for(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<ConsistencyVerdict> {
    let (states, census) = states_and_census(db).await?;
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
    let (states, census) = states_and_census(db).await?;
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
    let (states, census) = states_and_census(db).await?;
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
            verdict(&row(26, 26), &RootCensus::new(10, 0, 2), DataFace::Ingest),
            ConsistencyVerdict::Lagging { .. }
        ));
        assert!(matches!(
            verdict(&row(26, 26), &RootCensus::new(10, 0, 0), DataFace::Ingest),
            ConsistencyVerdict::InSync { .. }
        ));
        assert!(matches!(
            verdict(&row(26, 20), &RootCensus::new(9, 1, 0), DataFace::Ingest),
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
        // 数据水位在、模型水位 0、账本空（重启后 / 从没跑过模型任务）：欠的就是 0 → 4 这一窗，
        // 是在赶，不是「不判」（ADR-079：库里没有逐根的行可以拿来说「没有根」）。
        let never_stamped = verdict(&row(4, 0), &RootCensus::default(), DataFace::Ingest);
        assert!(matches!(never_stamped, ConsistencyVerdict::Lagging { .. }));
        assert_eq!(never_stamped.as_str(), "lagging");
        assert_eq!(never_stamped.columns().model_verdict_reason, None);
        assert!(never_stamped.describe(5522).contains("模型在追赶"));
        assert!(
            matches!(
                verdict(&row(4, 0), &RootCensus::new(0, 0, 1), DataFace::Ingest),
                ConsistencyVerdict::Lagging { .. }
            ),
            "有过根（哪怕全放弃了）就还是模型面的事，照旧 lagging"
        );

        let mut caught_up = row(26, 26);
        caught_up.model_settled_at = Some("2026-09-05T17:01:43Z".into());
        // 零根库：没有「验证过的空清单收据」了（ADR-072 D4），水位相等就是任务盖的那张收据。
        let completed_empty = RootCensus::default();
        let caught_up = verdict(&caught_up, &completed_empty, DataFace::Ingest);
        assert_eq!(caught_up.caught_up_at(), Some("2026-09-05T17:01:43Z"));
        assert_eq!(caught_up.as_str(), "in_sync");
        assert_eq!(
            verdict(&row(26, 25), &RootCensus::default(), DataFace::Ingest).caught_up_at(),
            None
        );
    }

    /// 普查读进程内账本（ADR-079）：`failed` 的根在赶、`http_422` 已放弃、`Generated` 追平；
    /// 账本里没有那个库的页就是三格全零。库里一行 `gen_root` 都不需要。
    #[tokio::test]
    async fn bulk_census_separates_abandoned_roots_but_keeps_strict_verdict_lagging() {
        use crate::data_interface::model_root_ledger as ledger;
        use aios_core::RefU64;
        use surrealdb::engine::any::connect;

        // 账本是进程全局的：用别处不会碰的库号，别与其它测试串账。
        const A: u32 = 993_000;
        const B: u32 = 993_001;
        const C: u32 = 993_002;
        let r = |id: u32| aios_core::RefnoEnum::from(RefU64::from_two_nums(993_000, id));
        ledger::forget_dbnum(A);
        ledger::forget_dbnum(B);
        ledger::forget_dbnum(C);
        ledger::record_generated(A, r(1), "BRAN", 26);
        ledger::record_failed(A, r(2), "BRAN", 26, "boom");
        ledger::record_refused(A, r(3), "PANE", 26, "no root");
        ledger::record_failed(B, r(4), "EQUI", 20, "boom");

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("dual_watermark_census")
            .await
            .unwrap();
        db.query(format!(
            "CREATE dbnum_watermark:{A} SET dbnum={A}, db_type='DESI', applied_sesno=26, model_sesno=26;\
             CREATE dbnum_watermark:{B} SET dbnum={B}, db_type='DESI', applied_sesno=20, model_sesno=10;\
             CREATE dbnum_watermark:{C} SET dbnum={C}, db_type='DESI', applied_sesno=0, model_sesno=0;"
        )).await.unwrap().check().unwrap();

        let (states, census) = states_and_census(&db).await.unwrap();
        // `failed` 的根在赶（死信那一格随 pending 表退了，ADR-072 D5）；`http_422` 才是已放弃。
        assert_eq!(census[&A], RootCensus::new(1, 1, 1));
        assert_eq!(census[&B], RootCensus::new(0, 1, 0));
        assert!(!census.contains_key(&C), "账本没有这一页");
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
        assert!(matches!(verdicts[&A], ConsistencyVerdict::Lagging { .. }));
        assert!(matches!(verdicts[&B], ConsistencyVerdict::Lagging { .. }));
        assert!(matches!(verdicts[&C], ConsistencyVerdict::NotJudged { .. }));

        // 那根生成成了：追平格 +1、在赶格清零，A 随即 in_sync 之外只剩已放弃那一格拦着。
        ledger::record_generated(A, r(2), "BRAN", 26);
        let (_, census) = states_and_census(&db).await.unwrap();
        assert_eq!(census[&A], RootCensus::new(2, 0, 1));
        ledger::forget_dbnum(A);
        ledger::forget_dbnum(B);
        ledger::forget_dbnum(C);
    }

    #[tokio::test]
    async fn health_summary_ignores_non_model_databases() {
        use crate::data_interface::model_root_ledger as ledger;
        use surrealdb::engine::any::connect;

        const DESI: u32 = 993_010;
        const DICT: u32 = 993_011;
        ledger::forget_dbnum(DESI);
        ledger::record_generated(
            DESI,
            aios_core::RefnoEnum::from(aios_core::RefU64::from_two_nums(993_010, 1)),
            "BRAN",
            26,
        );
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("dual_watermark_health_scope")
            .await
            .unwrap();
        db.query(format!(
            "CREATE dbnum_watermark:{DESI} SET dbnum={DESI}, db_type='DESI', applied_sesno=26, model_sesno=26;\
             CREATE dbnum_watermark:{DICT} SET dbnum={DICT}, db_type='DICT', applied_sesno=12, model_sesno=0;"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        let (states, census) = states_and_census(&db).await.unwrap();
        let summary = summarize_states(&states, &census, DataFace::Ingest, |_| true);
        assert_eq!(summary.judged, 1);
        assert_eq!(summary.in_sync, 1);
        assert_eq!(summary.lagging, 0);
        ledger::forget_dbnum(DESI);
    }
}
