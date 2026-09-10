//! 模型覆盖凭证的**只读取数**（ADR-058 D5 / ADR-054）：`gen_root` 上「这个库 / 这个根的模型是照
//! 哪一版数据与目录算出来的」怎么读——按需 `ensure` 的凭证命中判据（[`generation_roots_cache_current`]，
//! 设计会话 + 目录落款两半）、启动一级过滤的凭证下限（[`startup_coverage_floors`]）、空闲轮的模型门
//! （[`model_coverage_current`]）、已发布根落在哪一版目录上（[`published_catalogue_sessions_on`]）。
//! 全部**只读 `gen_root`**，一条都不碰 `model_update_pending`。
//!
//! 2026-09-10（ADR-071 S7）从 `model_update_pending.rs` 搬出：那张队列表随 S4 后半 / S7 退役，而这里
//! 是凭证真值（d-227：`gen_root` 凭证留库）的正经读法，与队列无关——**名字与行为不变，只换住址**
//! （同 S5a 搬 `model_watermark.rs` 的做法）。同日稍后**目录版本核对**（[`reconcile_catalogue_versions`]
//! 点名 → 收窗 → 反查 → 排工，与运行期的 `_after_sweep` / `_now` 两个入口、`/health.catalogue_reconcile`
//! 快照）也搬了进来：它的排工自 ADR-071 S4 起是**排带明确根名单的 ModelOnly 任务**
//! （`DiscoveredBatch::regen_roots`），不再写 pending 行，于是不再依赖队列表。还留在隔壁的只有
//! `sync_and_seed_model_coverage`（`/model/rebuild` 用，把缺凭证的根 `render_upsert` 进 pending），随 S4 改。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use aios_core::SUL_DB;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::dbnum_state::escape_surql_str;

/// `SELECT count() … GROUP ALL` 的那一行。
#[derive(Debug, Deserialize)]
struct CountRow {
    count: usize,
}
#[derive(Debug, Deserialize)]
struct RootCompletionCredentialRow {
    #[serde(default)]
    publication_status: Option<String>,
    #[serde(default)]
    desired_revision: Option<u64>,
    #[serde(default)]
    published_revision: Option<u64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    source_end_sesno: Option<i32>,
    /// `published_model_target.catalogue`：这份几何是照哪一版**目录**算出来的，每个 CATA
    /// 依赖一条（H1 (c)，2026-09-08）。此前凭证只比设计库会话，目录改了、设计没改的根
    /// 在按需 `ensure` 眼里永远「当前」。
    #[serde(default)]
    catalogue: Vec<CatalogueStamp>,
}

/// `gen_root.published_model_target.catalogue[*]` 里与凭证判据有关的两格。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct CatalogueStamp {
    #[serde(default)]
    pub dbnum: Option<u32>,
    #[serde(default)]
    pub session: Option<u32>,
}

/// 目录落款是否仍覆盖此刻的目录文件（纯函数，H1 (c)）。
///
/// 每条落款 `session` 不早于该目录库文件最新会话才算覆盖（与设计库那半 `credential_covers`
/// 同为单调判据：ensure 用更新的目录算过、记录比文件新，不是过期）。`latest(dbnum) == None`
/// ——不在当前 MDB、stat 不到——是**未知**，不算过期，与 [`stale_catalogue_dbnums`] 同一口径：
/// 判成过期会让一个读不到的目录库把每次显示都变成重算。缺 `dbnum` / `session` 的历史落款同样放行。
pub(crate) fn catalogue_stamps_current(
    stamps: &[CatalogueStamp],
    latest: impl Fn(u32) -> Option<u32>,
) -> bool {
    stamps
        .iter()
        .all(|stamp| match (stamp.dbnum, stamp.session) {
            (Some(dbnum), Some(session)) => latest(dbnum).is_none_or(|now| session >= now),
            _ => true,
        })
}

/// 这个生成根的完成凭证是否覆盖当前生成源版本。
///
/// 仅有 `inst_relate` 行不构成缓存命中：它可能属于较早会话。生成根必须处于完成终态，
/// 且凭证会话号**不早于**文件最新会话（ADR-054 实施约束 4：单调，不判等值）；旧行缺
/// 会话号按未命中处理。文件被换回更旧版本时凭证会显得「超前」，那是文件异常，由重建
/// 批次（ADR-021）清凭证，不在这里猜。
pub async fn generation_root_cache_current(root_refno: &str) -> anyhow::Result<bool> {
    Ok(generation_roots_cache_current(&[root_refno.to_string()])
        .await?
        .contains(root_refno))
}

fn render_generation_root_credentials(root_refnos: &[String]) -> String {
    let keys = root_refnos
        .iter()
        .map(|root| format!("gen_root:{}", escape_surql_str(&root.replace('/', "_"))))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "SELECT record::id(id) AS root_id, status, source_end_sesno, publication_status, desired_revision, published_revision, \
         published_model_target.catalogue ?? [] AS catalogue FROM [{keys}];"
    )
}

/// Batch form used by subtree display: resolve file versions off-thread once,
/// then load every completion credential with one primary-key query.
///
/// 命中 = 完成终态 ∧ 设计库凭证不早于文件最新 ∧ **目录落款不早于各目录文件最新**
/// （[`catalogue_stamps_current`]，H1 (c)）。目录文件最新会话按 dbnum 只读一次
/// （`latest_source_version` 自己还有按文件戳的缓存），一次子树显示多付的是几个目录库的头。
pub async fn generation_roots_cache_current(
    root_refnos: &[String],
) -> anyhow::Result<HashSet<String>> {
    if root_refnos.is_empty() {
        return Ok(HashSet::new());
    }
    let requested = root_refnos.to_vec();
    let versions = tokio::task::spawn_blocking(move || {
        requested
            .into_iter()
            .map(|root_refno| {
                let root = crate::fast_model::e3d_model_service::parse_refno(&root_refno)?;
                let version = crate::data_interface::model_source::root_source_version(root)
                    .with_context(|| format!("生成根 {root_refno} 无法判定生成源版本"))?;
                anyhow::Ok((root_refno, version.dbnum, version.sesno as i32))
            })
            .collect::<anyhow::Result<Vec<_>>>()
    })
    .await
    .map_err(|error| anyhow::anyhow!("resolve model sources task failed: {error}"))??;

    let mut response = SUL_DB
        .query(render_generation_root_credentials(root_refnos))
        .await
        .map_err(|error| anyhow::anyhow!("load model credentials failed: {error}"))?
        .check()
        .map_err(|error| anyhow::anyhow!("load model credentials statement failed: {error}"))?;
    #[derive(Debug, Deserialize)]
    struct BatchRow {
        root_id: String,
        #[serde(flatten)]
        credential: RootCompletionCredentialRow,
    }
    let rows: Vec<BatchRow> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("decode model credentials failed: {error}"))?;
    let rows = rows
        .into_iter()
        .map(|row| (row.root_id, row.credential))
        .collect::<HashMap<_, _>>();
    // 目录文件最新会话：这批凭证落款提到的每个目录库问一次。读不到的留 `None`（未知）。
    let catalogue_dbnums: BTreeSet<u32> = rows
        .values()
        .flat_map(|row| row.catalogue.iter().filter_map(|stamp| stamp.dbnum))
        .collect();
    let catalogue_latest: BTreeMap<u32, Option<u32>> = if catalogue_dbnums.is_empty() {
        BTreeMap::new()
    } else {
        tokio::task::spawn_blocking(move || {
            catalogue_dbnums
                .into_iter()
                .map(|dbnum| {
                    let latest = crate::data_interface::model_source::latest_source_version(dbnum)
                        .ok()
                        .map(|version| version.sesno);
                    (dbnum, latest)
                })
                .collect()
        })
        .await
        .map_err(|error| anyhow::anyhow!("resolve catalogue versions task failed: {error}"))?
    };
    let mut covered = BTreeMap::new();
    for dbnum in versions.iter().map(|(_, dbnum, _)| *dbnum).collect::<BTreeSet<_>>() {
        if let Some(sesno) = library_coverage_on(&SUL_DB, dbnum).await? {
            covered.insert(dbnum, sesno);
        }
    }
    Ok(versions
        .into_iter()
        .filter_map(|(root_refno, dbnum, required_sesno)| {
            let root_id = root_refno.replace('/', "_");
            rows.get(&root_id)
                .is_some_and(|row| {
                    super::window_root_plan::root_is_settled(row.status.as_deref(), row.source_end_sesno,
                        row.publication_status.as_deref(), row.desired_revision, row.published_revision)
                    && (covered.get(&dbnum).is_some_and(|sesno| *sesno >= required_sesno)
                    || crate::data_interface::model_source::credential_covers(
                        row.source_end_sesno,
                        required_sesno,
                    )) && catalogue_stamps_current(&row.catalogue, |dbnum| {
                        catalogue_latest.get(&dbnum).copied().flatten()
                    })
                })
                .then_some(root_refno)
        })
        .collect())
}

/// 库级模型覆盖：这个库追平到哪一版——就是 `dbnum_watermark.model_sesno`。
///
/// 任务收尾只在这一版的根**全部**落进去时才盖它（ADR-072 D4 / `task_end::render_task_end_tail`
/// 的 `TaskModelOutcome::all_roots_ok` 守卫），所以这一格本身就是凭据；原先另开一张
/// `CompletionReceipt` 收据再交叉核验的那一路随 publish 阶段退了。`None` = 从没追平过。
/// 生产库走水位镜像（`DbnumState::read`），测试夹具与旁路连接直查那一行。
async fn library_coverage_on(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<Option<i32>> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        model_sesno: Option<i32>,
    }
    let model_sesno = if std::ptr::eq(db, &*SUL_DB) {
        crate::data_interface::dbnum_state::DbnumState::read(dbnum)
            .await?
            .map(|state| state.model_sesno)
    } else {
        let row: Option<Row> = db
            .query(format!("SELECT model_sesno FROM ONLY dbnum_watermark:{dbnum};"))
            .await?
            .check()?
            .take(0)?;
        row.and_then(|row| row.model_sesno)
    };
    Ok(model_sesno.filter(|sesno| *sesno > 0))
}

#[cfg(test)]
#[test]
fn generation_root_credentials_use_one_primary_key_query() {
    let query =
        render_generation_root_credentials(&["24384/26229".to_string(), "24384/23264".to_string()]);
    assert_eq!(
        query,
        "SELECT record::id(id) AS root_id, status, source_end_sesno, publication_status, desired_revision, published_revision, \
         published_model_target.catalogue ?? [] AS catalogue FROM \
[gen_root:24384_26229,gen_root:24384_23264];"
    );
    assert!(!query.contains(" WHERE "));
}

/// H1 (c)：凭证判据的目录那一半。设计会话够新、目录落款却早于目录文件最新 → 未命中；
/// 落款不早于（等于或更新）→ 命中；目录库读不到 / 历史落款缺格 → 未知，放行。
#[cfg(test)]
#[test]
fn catalogue_stamps_are_current_only_when_no_stamp_is_behind_its_file() {
    let stamp = |dbnum: u32, session: u32| CatalogueStamp {
        dbnum: Some(dbnum),
        session: Some(session),
    };
    let latest = |dbnum: u32| match dbnum {
        7000 => Some(12),
        5052 => Some(185),
        _ => None,
    };
    // 没有目录依赖的根（历史行 / 纯设计几何）：目录那一半恒真。
    assert!(catalogue_stamps_current(&[], latest));
    // 两个目录库都追平 → 命中；其中一个早于文件最新 → 未命中。
    assert!(catalogue_stamps_current(
        &[stamp(7000, 12), stamp(5052, 185)],
        latest
    ));
    assert!(!catalogue_stamps_current(
        &[stamp(7000, 11), stamp(5052, 185)],
        latest
    ));
    assert!(!catalogue_stamps_current(
        &[stamp(7000, 12), stamp(5052, 184)],
        latest
    ));
    // 落款比文件还新（ensure 用更新的目录算过、文件随后没变）：单调判据，仍命中。
    assert!(catalogue_stamps_current(&[stamp(7000, 13)], latest));
    // 不在当前 MDB / 读不到的目录库：未知不算过期，不许把每次显示变成重算。
    assert!(catalogue_stamps_current(&[stamp(9999, 1)], latest));
    // 历史落款缺 dbnum 或 session：同样放行。
    assert!(catalogue_stamps_current(
        &[CatalogueStamp {
            dbnum: Some(7000),
            session: None,
        }],
        latest
    ));
    assert!(catalogue_stamps_current(
        &[CatalogueStamp {
            dbnum: None,
            session: Some(1),
        }],
        latest
    ));
}

/// ADR-025 模型门：这个库的每个生成根是否都有覆盖**文件最新会话**的完成凭证。
///
/// 标尺是文件最新，不是 `applied_sesno`（ADR-054：水位只承诺数据落库，不是生成时点；
/// ADR-058 D4 的启动复核也以文件最新为右端）。两边若不同尺，会出现启动复核刚判定
/// 「落后、已开窗排队」而空闲轮同一刻判定「覆盖完整、模型就绪」——两个都对，只是
/// 各拿一把尺。文件读取是同步 I/O（stat + 会话页），扔进阻塞线程。
///
/// 与 [`gen_root_credential_is_current`] 同一口径（ADR-054 实施约束 4）：凭证会话号**不早于**
/// 标尺即算覆盖，`0`（未认领）永不算；时刻列只记不比。
pub async fn model_coverage_current(dbnum: u32) -> anyhow::Result<bool> {
    let target = tokio::task::spawn_blocking(move || {
        crate::data_interface::model_source::latest_source_version(dbnum)
    })
    .await
    .map_err(|error| {
        anyhow::anyhow!("model coverage dbnum={dbnum} source read panicked: {error}")
    })??;
    let covered = library_coverage_on(&SUL_DB, dbnum)
        .await?
        .is_some_and(|sesno| sesno >= target.sesno as i32);
    let settled = super::model_watermark::settled_root_predicate();
    let lag = if covered { String::new() } else {
        format!(" OR (source_end_sesno?:0) <= 0 OR (source_end_sesno?:0) < {}", target.sesno)
    };
    let started = Instant::now();
    let coverage_result = SUL_DB
        .query(format!(
            "SELECT count() AS count FROM gen_root WHERE dbnum = {dbnum} AND (\
             NOT ({settled}) {lag}) GROUP ALL;"
        ))
        .await;
    crate::data_interface::model_concurrency::record_surreal_read(started.elapsed());
    let mut response = coverage_result?.check()?;
    let rows: Vec<CountRow> = response.take(0)?;
    Ok(covered && rows.first().map_or(0, |row| row.count) == 0)
}

#[derive(Debug, Deserialize)]
struct GenRootDbnumRow {
    #[serde(default)]
    dbnum: Option<u32>,
}

/// 已经有生成根凭证记录的库（`gen_root` 至少一行）。
///
/// 一次 GROUP BY 全表——`gen_root` 每根一行，量级远小于 `inst_relate`，且只挂启动
/// 序列（与 FR-1「全表谓词只许启动序列与人工诊断入口」同一口径）。`dbnum` 缺失的
/// 历史行没有归属，核对不了也补不了，跳过并由调用方计数报告。
pub async fn coverage_tracked_dbnums() -> anyhow::Result<Vec<u32>> {
    coverage_tracked_dbnums_on(&SUL_DB).await
}

pub(crate) async fn coverage_tracked_dbnums_on(db: &Surreal<Any>) -> anyhow::Result<Vec<u32>> {
    let mut response = db
        .query("SELECT dbnum FROM gen_root GROUP BY dbnum;")
        .await?
        .check()?;
    let rows: Vec<GenRootDbnumRow> = response.take(0)?;
    let mut dbnums: Vec<u32> = rows.into_iter().filter_map(|row| row.dbnum).collect();
    dbnums.sort_unstable();
    dbnums.dedup();
    Ok(dbnums)
}

/// 每个库的**模型凭证下限**（ADR-058 D5）：安定生成根里最小的 `source_end_sesno`，
/// 外加没有可用凭证的根数。
///
/// 模型那一枚凭证（ADR-056 N5；数据那一枚是 `dbnum_watermark.applied_sesno`）。与
/// 文件最新会话一比就答得出「这个库的模型按哪一版数据生成」——启动一级过滤、覆盖
/// 清单与 `/dbnums` 的凭证两列全部照这一份取数，**不另开第二套口径**：同一个库在
/// 启动日志与接口上给出两个不同的「落后到哪一版」，看的人分不出哪个是真的。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StartupCoverageFloor {
    /// 已安定行里最小的 `source_end_sesno`；一条安定行都没有时是 `None`。
    pub settled_floor: Option<i32>,
    pub settled_roots: usize,
    pub unsettled_roots: usize,
}

/// 把只有嵌套归属的历史 `gen_root` 行补上顶层 `dbnum`，返回补了几行。
///
/// `render_upsert` 从 2026-09-03 起在建行时就写顶层 `dbnum`；在那之前由模型队列创建的行只有
/// `desired_target.dbnum`，顶层是 `NONE`。而启动的凭证下限、覆盖清单与 `/dbnums` 的模型凭证
/// 下限**全部**按 `WHERE dbnum != NONE` 取数——不补，这些行就一直看不见，而且不会报错，
/// 只是每一处按库取数的地方都少算（ams 现场实测 2245 行里 2244 行如此）。
///
/// 幂等，补完就不再命中。全表谓词，只挂启动序列（与 [`coverage_tracked_dbnums`] 同一口径）。
/// 失败只告警：它是补齐不是前提，下一次启动还会再试。
pub(crate) async fn backfill_gen_root_dbnum_on(db: &Surreal<Any>) -> anyhow::Result<usize> {
    #[derive(Debug, Deserialize)]
    struct Backfilled {
        #[allow(dead_code)]
        id: surrealdb::sql::Thing,
    }
    let mut response = db
        .query(
            "UPDATE gen_root SET dbnum = desired_target.dbnum \
             WHERE dbnum = NONE AND (desired_target.dbnum ?: 0) > 0 RETURN id;",
        )
        .await?
        .check()?;
    let rows: Vec<Backfilled> = response.take(0)?;
    Ok(rows.len())
}

/// 一次请求两条聚合语句问完全部库——不是逐库点查，更不是逐库开窗（ADR-058 D5）。
///
/// 「安定」= 状态在 [`SETTLED_ROOT_STATUSES`] 里且凭证大于 0，与
/// `window_root_plan::PersistedRootState::is_settled` 同一口径。未安定的行没有可用的凭证，
/// 它们既进不了下限、也一定要重排，所以单独计数——一个库只要有一行未安定就得开窗，
/// 哪怕其余凭证全都追平了。
pub async fn startup_coverage_floors() -> anyhow::Result<BTreeMap<u32, StartupCoverageFloor>> {
    startup_coverage_floors_on(&SUL_DB).await
}

pub(crate) async fn startup_coverage_floors_on(
    db: &Surreal<Any>,
) -> anyhow::Result<BTreeMap<u32, StartupCoverageFloor>> {
    #[derive(Debug, Deserialize)]
    struct SettledRow {
        #[serde(default)]
        dbnum: Option<u32>,
        #[serde(default)]
        floor: Option<i32>,
        #[serde(default)]
        roots: usize,
    }
    #[derive(Debug, Deserialize)]
    struct UnsettledRow {
        #[serde(default)]
        dbnum: Option<u32>,
        #[serde(default)]
        roots: usize,
    }
    let settled = super::model_watermark::settled_root_predicate();
    let started = Instant::now();
    let mut response = db
        .query(format!(
            "SELECT dbnum, math::min(source_end_sesno ?: 0) AS floor, count() AS roots \
             FROM gen_root WHERE dbnum != NONE AND ({settled}) GROUP BY dbnum;\
             SELECT dbnum, count() AS roots FROM gen_root WHERE dbnum != NONE \
             AND NOT ({settled}) \
             GROUP BY dbnum;"
        ))
        .await?
        .check()?;
    crate::data_interface::model_concurrency::record_surreal_read(started.elapsed());
    let mut floors: BTreeMap<u32, StartupCoverageFloor> = BTreeMap::new();
    for row in response.take::<Vec<SettledRow>>(0)? {
        let Some(dbnum) = row.dbnum else { continue };
        let entry = floors.entry(dbnum).or_default();
        entry.settled_floor = row.floor;
        entry.settled_roots = row.roots;
    }
    for row in response.take::<Vec<UnsettledRow>>(1)? {
        let Some(dbnum) = row.dbnum else { continue };
        floors.entry(dbnum).or_default().unsettled_roots = row.roots;
    }
    for (dbnum, entry) in &mut floors {
        if entry.unsettled_roots == 0
            && let Some(sesno) = library_coverage_on(db, *dbnum).await?
        {
            entry.settled_floor = Some(sesno);
        }
    }
    Ok(floors)
}

/// 已发布的根落在哪一版目录上：`CATA dbnum → 记在案的最旧会话`。
///
/// 来源是 `gen_root.published_model_target.catalogue`——`ModelTarget` 的目录段，每个 CATA
/// 库一条 `{dbnum, db_type, file, session, digest}`。这是**模型面自己**记下的「我是照哪一版
/// 目录算出来的」，不经 `pe`、不经 `dbnum_watermark`（ADR-056 N7 / ADR-054）。
///
/// 同一个 CATA 库会被不同 DESI 库、不同时刻发布的根记成不同会话；取**最小**那个：这里问的
/// 是「有没有根落在旧目录上」，最旧的那一版说了算。
///
/// 一条聚合语句问完全部根（ADR-058 D5），不是逐库点查、更不是把几万行目录段拉回来。
pub(crate) async fn published_catalogue_sessions_on(
    db: &Surreal<Any>,
) -> anyhow::Result<BTreeMap<u32, u32>> {
    #[derive(Debug, Deserialize)]
    struct CatalogueEntry {
        #[serde(default)]
        dbnum: Option<u32>,
        #[serde(default)]
        session: Option<u32>,
    }
    #[derive(Debug, Deserialize)]
    struct CatalogueRow {
        #[serde(default)]
        catalogue: Vec<CatalogueEntry>,
    }
    // `GROUP BY catalogue` 是**服务端去重**：一个库的全部根共用同一份 `ModelTarget`，
    // 所以整库几万行在这里塌成个位数行。不要换成 `GROUP ALL` + `array::flatten`——
    // 聚合函数是**逐行**求值之后才被 GROUP 收集的，那样拿回来的是「数组的数组」，
    // 而且它不会报错，只会在反序列化那一步变成一句看不出所以然的类型不匹配。
    let started = Instant::now();
    let mut response = db
        .query(
            "SELECT published_model_target.catalogue AS catalogue FROM gen_root \
             WHERE published_model_target.catalogue != NONE GROUP BY catalogue;",
        )
        .await?
        .check()?;
    crate::data_interface::model_concurrency::record_surreal_read(started.elapsed());
    let mut oldest: BTreeMap<u32, u32> = BTreeMap::new();
    for row in response.take::<Vec<CatalogueRow>>(0)? {
        for entry in row.catalogue {
            let (Some(dbnum), Some(session)) = (entry.dbnum, entry.session) else {
                continue;
            };
            oldest
                .entry(dbnum)
                .and_modify(|kept| *kept = (*kept).min(session))
                .or_insert(session);
        }
    }
    Ok(oldest)
}

/// 记在案的目录版本与此刻文件上的一比：哪些 CATA 库已经前移了，哪些说不出来。
///
/// 纯函数，当前会话从 `latest` 注入——真正的文件读在调用方（`spawn_blocking`），
/// 这里只负责判据，好让它单测得起来。
///
/// - `latest(dbnum) == Some(now)` 且 `now > 记录值` → **过期**，进第一个返回值；
/// - `latest(dbnum) == None`（不在当前 MDB / stat 不到）→ 进第二个返回值的计数：
///   那是**不知道**，不是「已确认过期」，两者合成一句就是拿未知冒充结论；
/// - `now <= 记录值` → 追平（`<` 出现在「按需 ensure 用的是更新的目录、这条记录更旧」
///   之外只可能是文件回退，那由数据面的回退阻断管，这里不重复判）。
pub(crate) fn stale_catalogue_dbnums(
    recorded: &BTreeMap<u32, u32>,
    latest: impl Fn(u32) -> Option<u32>,
) -> (Vec<u32>, usize) {
    let mut stale = Vec::new();
    let mut unreadable = 0usize;
    for (&dbnum, &recorded_session) in recorded {
        match latest(dbnum) {
            Some(now) if now > recorded_session => stale.push(dbnum),
            Some(_) => {}
            None => unreadable += 1,
        }
    }
    stale.sort_unstable();
    (stale, unreadable)
}

/// 启动路径上一个目录窗口最多逐条读多少个候选的记录头：超过就只报候选总数
/// （[`crate::data_interface::cata_cascade_window::CatalogueWindow::truncated_candidates`]）。
/// 「改一个目录件」的窗口是个位数；几万起的是一次性导入，本来就不是精确反查的形状。
const STARTUP_CATALOGUE_WINDOW_RECORD_CAP: usize = 20_000;

/// 过期目录库一窗变动目录件数超过这个数就不逐件反查，该库退化为依赖根全部过期
/// （2026-09-06 计划 P3 护栏；阈值先取 09-04 inventory §2.10 的 50）。「改一个目录件」的窗口
/// 是个位数，几十起就是批量改目录——那时逐件点查的闭包多半也覆盖到整库，直接全过期更便宜。
const CATALOGUE_CASCADE_SEED_CAP: usize = 50;

/// 目录版本核对的结果：过期名单、未知数、每个过期库的**变动目录件窗口**，以及对这些窗口
/// 做的反查与排工。
#[derive(Debug, Default)]
pub(crate) struct CatalogueReconcile {
    /// 已确认前移的 CATA 库，升序。
    pub(crate) stale: Vec<u32>,
    /// 读不到最新会话的 CATA 库数（未知，不算过期）。
    pub(crate) unreadable: usize,
    /// 每个过期库一条：`记在案的最旧会话 → 文件最新` 这一窗变了哪些目录件、该库有没有
    /// 反向引用表可供反查。收不了的窗口也在，带 `error`。
    pub(crate) windows: Vec<crate::data_interface::cata_cascade_window::CatalogueWindowReport>,
    /// P3：对上面那些窗口做的反查与排工。`None` = 没有过期库，没跑。
    pub(crate) cascade: Option<crate::data_interface::reverse_reference::CatalogueCascadeReport>,
    /// A3（2026-09-08）：数据面那一半——已入库依赖行（`pe.dependency = true` 与它们的 `ref_rev`）
    /// 随目录前移的账。每次核对都跑，候选是全部有 `cata_dependency_state` 行的目录库，
    /// 与上面模型面的「过期」名单无关。
    pub(crate) dependency_refresh:
        crate::data_interface::cata_dependency::CatalogueDependencyRefreshReport,
}

/// 收成的一窗：回执 + 窗口本体（逐件清单是反查的种子，不上 `/health`）。
struct StaleCatalogueWindow {
    report: crate::data_interface::cata_cascade_window::CatalogueWindowReport,
    /// 收不了是 `None`，回执里带 `error`。
    window: Option<crate::data_interface::cata_cascade_window::CatalogueWindow>,
}

/// 启动复核里的目录那一格（2026-09-04 启动增量审核 #4 → 2026-09-06 计划 P2 收窗 → P3 反查）。
///
/// 三步：**点名**（哪些 CATA 库在已发布的根落款之后前移了）→ **收窗**（每个过期库
/// `记在案的最旧会话 → 文件最新` 这一窗具体变了哪些目录件、该库有没有反向引用表）→
/// **反查排工**（[`cascade_stale_catalogue_windows`]：变动目录件 → 目录侧闭包 → 设计侧引用者 →
/// 上卷到根 → 落款仍在旧目录上的根排 `RegenRoot`；判不了的库退化为依赖根全部重排，
/// 并点名 `catalogue_cascade_degraded`——宁多算不漏算）。
///
/// ADR-058 D2 的触发源就在这里：direct / 读透形态下目录库没有数据批次，读透形态的文件事件也
/// 汇到 `reconcile_model_coverage_at_startup`，所以启动与文件事件共用这一处。**摄入形态**
/// （external / rocksdb，2026-09-07 起的默认）的文件事件走的是数据面 sweep，目录库在那条路上
/// 被 `UpdateScope::admits` 挡在数据批次之外——2026-09-08 之前它们到不了这里，目录改了要等
/// 重启；现在 sweep 收尾经 [`reconcile_catalogue_versions_after_sweep`]、`/update/execute` 经
/// [`reconcile_catalogue_versions_now`] 也汇到这一处（H1）。三条入口串行（同一把
/// [`CATALOGUE_RECONCILE_SERIAL`]），免得两次核对把同一批根排两遍。
///
/// 反查没跑成时的兜底：按需 `ensure` 的凭证判据自 2026-09-08 起也比目录落款
/// （[`crate::data_interface::model_coverage::catalogue_stamps_current`]），落款早于目录文件最新即判未命中、照常重算。
///
/// **数据面那一半（A3，2026-09-08）**：模型面排完根之后，再把 `pe` 里 `dependency = true` 的目录行
/// 与它们的 `ref_rev` 也随目录前移（[`crate::data_interface::cata_dependency::refresh_dependency_rows_of_moved_catalogues`]：
/// 变动目录件 ∩ 已入库依赖行 → `Replace` 重渲、被删的落墓碑、推进 `cata_dependency_state`）。此前这一步
/// 只在某个 DESI 批次的闭包碰到那个目录库时才跑（ADR-064 D5 的 D7）：目录改了、设计没改，属性面板看到的
/// 目录属性就一直是旧的。两半各自对文件、互不依赖：模型面没跑成（读不到落款 / 一个根都没发布）数据面照做。
pub(crate) async fn reconcile_catalogue_versions() -> CatalogueReconcile {
    let _serial = CATALOGUE_RECONCILE_SERIAL.lock().await;
    let mut reconcile = reconcile_published_root_stamps().await;
    let project = aios_core::get_db_option().project_name.clone();
    reconcile.dependency_refresh =
        crate::data_interface::cata_dependency::refresh_dependency_rows_of_moved_catalogues(
            &project,
        )
        .await;
    reconcile
}

/// 模型面那一半：点名 → 收窗 → 反查排工（三步见 [`reconcile_catalogue_versions`] 的文档）。
/// 只在串行闸里被调；`dependency_refresh` 留空，由调用方补数据面那一半。
async fn reconcile_published_root_stamps() -> CatalogueReconcile {
    let recorded = match published_catalogue_sessions_on(&SUL_DB).await {
        Ok(recorded) => recorded,
        Err(error) => {
            println!(
                "目录版本核对失败：读不到已发布根的目录落款，本次说不出目录有没有前移\
                 ——是未知，不是「没变」: {error:#}"
            );
            return CatalogueReconcile::default();
        }
    };
    if recorded.is_empty() {
        return CatalogueReconcile::default();
    }
    let dbnums: Vec<u32> = recorded.keys().copied().collect();
    let latest = tokio::task::spawn_blocking(move || {
        dbnums
            .into_iter()
            .map(|dbnum| {
                let sesno = crate::data_interface::model_source::latest_source_version(dbnum)
                    .ok()
                    .map(|version| version.sesno);
                (dbnum, sesno)
            })
            .collect::<BTreeMap<u32, Option<u32>>>()
    })
    .await
    .unwrap_or_default();
    let (stale, unreadable) =
        stale_catalogue_dbnums(&recorded, |dbnum| latest.get(&dbnum).copied().flatten());
    // 每个过期库收一窗：S = 记在案的最旧会话，T = 文件最新。只读文件，阻塞任务里做。
    let spans: Vec<(u32, u32, u32)> = stale
        .iter()
        .filter_map(|&dbnum| {
            let base = *recorded.get(&dbnum)?;
            let target = latest.get(&dbnum).copied().flatten()?;
            Some((dbnum, base, target))
        })
        .collect();
    let collected = tokio::task::spawn_blocking(move || collect_stale_catalogue_windows(&spans))
        .await
        .unwrap_or_default();
    // P3：反查 + 排工。窗口本体在这里用掉，回执留给 `/health`。
    let cascade = cascade_stale_catalogue_windows(&collected).await;
    let windows = collected.into_iter().map(|item| item.report).collect();
    CatalogueReconcile {
        stale,
        unreadable,
        windows,
        cascade,
        ..CatalogueReconcile::default()
    }
}

/// 目录版本核对的三条入口（启动 / sweep / `/update/execute`）共用的串行闸。
///
/// 两次核对前后脚跑，第二次会把第一次刚排、还没重算完的根再排一遍（落款仍旧），
/// 白算一轮；串行之后至多剩「第一次排完、第二次紧跟着又排」这一种，靠任务入队的并集
/// 同 `(action, target)` 一行收口，不会多出第二行。
static CATALOGUE_RECONCILE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 运行期目录版本核对的一次回执（H1，2026-09-08）：摄入形态的 sweep 与 `/update/execute`
/// 触发的那几次；启动那一次在 `StartupReconcile`（`/health.initialization.startup.reconcile`）。
/// `/health.catalogue_reconcile` 露的就是**最近一次**。
#[derive(Debug, Clone, Serialize)]
pub struct CatalogueReconcileSnapshot {
    /// 谁触发的：sweep 的 origin（`watch` / `reconcile` / `scope-refresh`）或 `update-execute`。
    pub origin: String,
    /// `catalogue_file_advanced` = sweep 看见某个目录库文件的会话前移了；`periodic` = 周期对账
    /// 那一拍的强制复核（sweep 没看见前移也跑，兜住启动那次反查失败 / 目录文件不在监控目录
    /// 里的形状）；`requested` = 人工 `/update/execute`。
    pub trigger: &'static str,
    /// sweep 观察到前移 / 新出现的目录库号（`periodic` / `requested` 下为空）。
    pub advanced_catalogue_dbnums: Vec<u32>,
    pub at: String,
    pub elapsed_ms: u128,
    /// 已确认前移、且有已发布根落在旧目录上的 CATA 库，升序。
    pub stale_catalogue_dbnums: Vec<u32>,
    /// 记在案却读不到最新会话的 CATA 库数：未知，不算过期。
    pub unreadable_catalogue_dbnums: usize,
    /// 前移了**且**反查没能精确回答（与 `StartupReconcile` 同一语义）。
    pub catalogue_cascade_degraded: bool,
    pub catalogue_windows: Vec<crate::data_interface::cata_cascade_window::CatalogueWindowReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalogue_cascade: Option<crate::data_interface::reverse_reference::CatalogueCascadeReport>,
    /// A3：数据面那一半——已入库依赖行随目录前移的账（候选 = 全部有 `cata_dependency_state` 行的
    /// 目录库；`refreshed` 空且 `errors` 空 = 这一次没有依赖行要前移）。
    pub catalogue_dependency_refresh:
        crate::data_interface::cata_dependency::CatalogueDependencyRefreshReport,
}

impl CatalogueReconcileSnapshot {
    fn from_reconcile(
        origin: &str,
        trigger: &'static str,
        advanced: &[u32],
        elapsed: std::time::Duration,
        reconcile: CatalogueReconcile,
    ) -> Self {
        let CatalogueReconcile {
            stale,
            unreadable,
            windows,
            cascade,
            dependency_refresh,
        } = reconcile;
        Self {
            origin: origin.to_string(),
            trigger,
            advanced_catalogue_dbnums: advanced.to_vec(),
            at: chrono::Local::now().to_rfc3339(),
            elapsed_ms: elapsed.as_millis(),
            stale_catalogue_dbnums: stale,
            unreadable_catalogue_dbnums: unreadable,
            catalogue_cascade_degraded: cascade
                .as_ref()
                .is_some_and(|cascade| cascade.is_degraded()),
            catalogue_windows: windows,
            catalogue_cascade: cascade,
            catalogue_dependency_refresh: dependency_refresh,
        }
    }

    /// 本次排了多少根（精确命中 + 退化库整份名单）。
    pub fn enqueued_roots(&self) -> usize {
        self.catalogue_cascade
            .as_ref()
            .map_or(0, |cascade| cascade.enqueued_total())
    }

    /// 控制台上的话。周期复核没发现过期库时**一个字不打**（300 s 一拍，打了就是刷屏）；
    /// 文件前移 / 人工触发那两种总要回一句，哪怕结论是「没有根落在旧目录上」——
    /// 触发了却没声音，人分不清是没跑还是没事。数据面那一半（A3）真前移了行或有失败时
    /// 另占一行，不管是哪种触发——安静的一次不出声。
    pub fn banner_lines(&self) -> Vec<String> {
        let origin = &self.origin;
        let mut lines = Vec::new();
        if self.stale_catalogue_dbnums.is_empty() {
            if self.trigger != "periodic" {
                lines.push(format!(
                    "[{origin}] 目录版本核对（{}{}）：已发布的根没有落在旧目录上，不排模型工作{}",
                    self.trigger,
                    if self.advanced_catalogue_dbnums.is_empty() {
                        String::new()
                    } else {
                        format!("，前移的目录库 {:?}", self.advanced_catalogue_dbnums)
                    },
                    if self.unreadable_catalogue_dbnums > 0 {
                        format!(
                            "；{} 个记在案的 CATA 库读不到最新会话（未知，不算过期）",
                            self.unreadable_catalogue_dbnums
                        )
                    } else {
                        String::new()
                    }
                ));
            }
        } else {
            lines.push(format!(
                "[{origin}] 目录版本核对（{}）：{} 个 CATA 库已前移到新会话，而已发布的根还落在旧目录上 \
                 dbnum={:?}。下面每库一行是这一窗变了什么，末行是反查排了什么（2026-09-06 计划 P3）；\
                 反查没跑成 / 退化的库只剩按需 ensure 的目录落款判据兜底，或人工 \
                 POST /api/v1/dbnums/{{dbnum}}/model/rebuild：",
                self.trigger,
                self.stale_catalogue_dbnums.len(),
                self.stale_catalogue_dbnums
            ));
            for window in &self.catalogue_windows {
                lines.push(format!("  {}", window.banner_line()));
            }
            if let Some(cascade) = &self.catalogue_cascade {
                lines.push(format!("  {}", cascade.banner_line()));
            }
        }
        if let Some(line) = self.catalogue_dependency_refresh.banner_line() {
            lines.push(format!("[{origin}] {line}"));
        }
        lines
    }
}

/// 进程内：最近一次运行期目录核对 + sweep 上一轮观察到的目录文件会话。
struct CatalogueReconcileState {
    latest: Option<CatalogueReconcileSnapshot>,
    /// `None` = 还没有任何一轮 sweep 记过观察（进程刚起）。
    observed: Option<BTreeMap<u32, u32>>,
}

static CATALOGUE_RECONCILE_STATE: Mutex<CatalogueReconcileState> =
    Mutex::new(CatalogueReconcileState {
        latest: None,
        observed: None,
    });

fn catalogue_reconcile_state() -> MutexGuard<'static, CatalogueReconcileState> {
    CATALOGUE_RECONCILE_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `/health.catalogue_reconcile`：最近一次运行期目录核对；`None` = 本进程还没跑过
/// （启动那一次不在这里，在 `initialization.startup.reconcile`）。
pub fn catalogue_reconcile_snapshot() -> Option<CatalogueReconcileSnapshot> {
    catalogue_reconcile_state().latest.clone()
}

/// 一轮摄入形态 sweep 对目录核对的裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogueSweepVerdict {
    /// 进程里第一轮 sweep：只记下观察。启动那一次核对由 `reconcile_model_coverage_at_startup`
    /// 负责（`run_cli` 里它排在 `init_watcher` 之后），这里再跑一遍就是把同一批根排两遍。
    Seeded,
    /// 与上一轮相比没有目录库前移，也不是强制复核那一拍。
    Unchanged,
    /// 要跑：`catalogue_file_advanced`（看见前移）或 `periodic`（强制）。
    Run { trigger: &'static str },
}

/// 与上一轮观察相比**前移**或**新出现**的目录库（纯函数）。
///
/// 只看这一轮观察到的：上一轮见过、这一轮读不到头的库不算前移——读不到是未知，
/// 而未知不触发任何工作（与 [`crate::data_interface::model_coverage::stale_catalogue_dbnums`] 同一口径）。
pub(crate) fn catalogue_files_advanced(
    previous: Option<&BTreeMap<u32, u32>>,
    observed: &BTreeMap<u32, u32>,
) -> Vec<u32> {
    observed
        .iter()
        .filter(|(dbnum, now)| {
            previous
                .and_then(|previous| previous.get(*dbnum))
                .is_none_or(|seen| *now > seen)
        })
        .map(|(dbnum, _)| *dbnum)
        .collect()
}

/// 这一轮 sweep 要不要跑目录核对（纯函数）：第一轮只记观察；看见前移就跑；`force`
/// （周期对账那一拍）没看见也跑。
pub(crate) fn catalogue_sweep_verdict(
    previous: Option<&BTreeMap<u32, u32>>,
    observed: &BTreeMap<u32, u32>,
    force: bool,
) -> (CatalogueSweepVerdict, Vec<u32>) {
    let Some(previous) = previous else {
        return (CatalogueSweepVerdict::Seeded, Vec::new());
    };
    let advanced = catalogue_files_advanced(Some(previous), observed);
    let verdict = if !advanced.is_empty() {
        CatalogueSweepVerdict::Run {
            trigger: "catalogue_file_advanced",
        }
    } else if force {
        CatalogueSweepVerdict::Run {
            trigger: "periodic",
        }
    } else {
        CatalogueSweepVerdict::Unchanged
    };
    (verdict, advanced)
}

/// 摄入形态 sweep 的收尾（H1 (a)）：记下这一轮在监控目录里看到的每个目录库文件的最新会话，
/// 与上一轮比；有前移（或 `force`）就**另起任务**跑一次目录版本核对——不在 sweep 里等：
/// sweep 的 await 睡在看门狗的事件循环上，反查开 MDB 每个设计库要好几秒，等就是积压文件事件。
///
/// 只收整面重扫的观察（子集补扫没看其余目录，拿它当全集会把没扫到的库当成消失又出现）。
/// 返回裁决给调用方写日志。
pub fn reconcile_catalogue_versions_after_sweep(
    origin: &str,
    observed: BTreeMap<u32, u32>,
    force: bool,
) -> CatalogueSweepVerdict {
    let (verdict, advanced) = {
        let mut state = catalogue_reconcile_state();
        let (verdict, advanced) =
            catalogue_sweep_verdict(state.observed.as_ref(), &observed, force);
        state.observed = Some(observed);
        (verdict, advanced)
    };
    if let CatalogueSweepVerdict::Run { trigger } = verdict {
        let origin = origin.to_string();
        tokio::spawn(async move {
            reconcile_catalogue_versions_now_with(&origin, trigger, &advanced).await;
        });
    }
    verdict
}

/// 人工 / 显式触发的目录版本核对（H1 (b)，`/update/execute` 的摄入分支）：跑完再回，
/// 回执同时落进 `/health.catalogue_reconcile`。
pub async fn reconcile_catalogue_versions_now(origin: &str) -> CatalogueReconcileSnapshot {
    reconcile_catalogue_versions_now_with(origin, "requested", &[]).await
}

async fn reconcile_catalogue_versions_now_with(
    origin: &str,
    trigger: &'static str,
    advanced: &[u32],
) -> CatalogueReconcileSnapshot {
    let started = Instant::now();
    let reconcile = reconcile_catalogue_versions().await;
    let snapshot = CatalogueReconcileSnapshot::from_reconcile(
        origin,
        trigger,
        advanced,
        started.elapsed(),
        reconcile,
    );
    for line in snapshot.banner_lines() {
        println!("{line}");
    }
    catalogue_reconcile_state().latest = Some(snapshot.clone());
    snapshot
}

/// H1 (a)：sweep 对目录核对的裁决。第一轮只记观察（启动那次核对另有人负责）；此后看见
/// 任一目录库文件会话前移、或新出现一个目录库，就跑；周期对账那一拍强制跑；读不到头的库
/// 从观察里消失不算前移。
#[cfg(test)]
#[test]
fn a_sweep_reconciles_catalogues_only_when_a_file_advanced_or_the_tick_forces_it() {
    use CatalogueSweepVerdict::{Run, Seeded, Unchanged};
    let observed = |pairs: &[(u32, u32)]| pairs.iter().copied().collect::<BTreeMap<u32, u32>>();
    let first = observed(&[(7000, 12), (5052, 185)]);
    // 进程里的第一轮：只记，不跑——即便是强制那一拍。
    assert_eq!(
        catalogue_sweep_verdict(None, &first, false),
        (Seeded, Vec::new())
    );
    assert_eq!(
        catalogue_sweep_verdict(None, &first, true),
        (Seeded, Vec::new())
    );
    // 没变、不强制：不跑。
    assert_eq!(
        catalogue_sweep_verdict(Some(&first), &first, false),
        (Unchanged, Vec::new())
    );
    // 没变、周期对账那一拍：跑，理由是 periodic，前移名单为空。
    assert_eq!(
        catalogue_sweep_verdict(Some(&first), &first, true),
        (
            Run {
                trigger: "periodic"
            },
            Vec::new()
        )
    );
    // 一个库前移：跑，点名它。
    assert_eq!(
        catalogue_sweep_verdict(Some(&first), &observed(&[(7000, 13), (5052, 185)]), false),
        (
            Run {
                trigger: "catalogue_file_advanced"
            },
            vec![7000]
        )
    );
    // 新出现一个目录库（刚进 MDB / 刚出现在监控目录）：也算前移。
    assert_eq!(
        catalogue_files_advanced(
            Some(&first),
            &observed(&[(7000, 12), (5052, 185), (7351, 3)])
        ),
        vec![7351]
    );
    // 上一轮见过、这一轮读不到头（从观察里消失）：未知，不算前移。
    assert_eq!(
        catalogue_sweep_verdict(Some(&first), &observed(&[(7000, 12)]), false),
        (Unchanged, Vec::new())
    );
    // 会话回退（文件被换回旧版）不是前移：那是数据面回退阻断管的事。
    assert_eq!(
        catalogue_files_advanced(Some(&first), &observed(&[(7000, 11), (5052, 185)])),
        Vec::<u32>::new()
    );
}

/// 周期复核没发现过期库时不许刷屏；文件前移 / 人工触发总要回一句；有过期库时逐库一行 + 反查一行。
#[cfg(test)]
#[test]
fn the_runtime_catalogue_reconcile_speaks_only_when_it_has_something_to_say() {
    let quiet = CatalogueReconcileSnapshot::from_reconcile(
        "reconcile",
        "periodic",
        &[],
        std::time::Duration::from_millis(3),
        CatalogueReconcile::default(),
    );
    assert!(quiet.banner_lines().is_empty());
    assert_eq!(quiet.enqueued_roots(), 0);

    let advanced = CatalogueReconcileSnapshot::from_reconcile(
        "watch",
        "catalogue_file_advanced",
        &[7000],
        std::time::Duration::from_millis(3),
        CatalogueReconcile {
            unreadable: 1,
            ..CatalogueReconcile::default()
        },
    );
    let lines = advanced.banner_lines();
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].contains("[watch]") && lines[0].contains("7000"),
        "{lines:?}"
    );
    assert!(lines[0].contains("没有落在旧目录上"), "{lines:?}");
    assert!(lines[0].contains("1 个记在案的 CATA 库读不到"), "{lines:?}");

    let stale = CatalogueReconcileSnapshot::from_reconcile(
        "update-execute",
        "requested",
        &[],
        std::time::Duration::from_millis(3),
        CatalogueReconcile {
            stale: vec![7000],
            windows: vec![
                crate::data_interface::cata_cascade_window::CatalogueWindowReport::failed(
                    7000,
                    11,
                    12,
                    "locked".into(),
                ),
            ],
            cascade: Some(
                crate::data_interface::reverse_reference::CatalogueCascadeReport::failed(
                    "open the current MDB".into(),
                ),
            ),
            ..CatalogueReconcile::default()
        },
    );
    assert!(stale.catalogue_cascade_degraded);
    let lines = stale.banner_lines();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[0].contains("1 个 CATA 库已前移") && lines[0].contains("requested"));
}

/// 逐个过期目录库收窗；收不了的记 `error`，**不**让一个库的失败挡住别的库。
fn collect_stale_catalogue_windows(spans: &[(u32, u32, u32)]) -> Vec<StaleCatalogueWindow> {
    use crate::data_interface::cata_cascade_window::{
        CatalogueWindowReport, collect_catalogue_window_bounded,
    };
    spans
        .iter()
        .map(|&(dbnum, base, target)| {
            let failed = |error: String| StaleCatalogueWindow {
                report: CatalogueWindowReport::failed(dbnum, base, target, error),
                window: None,
            };
            let file = match crate::data_interface::model_source::source_file_of(dbnum) {
                Ok((_, file)) => file,
                Err(error) => return failed(format!("{error:#}")),
            };
            match collect_catalogue_window_bounded(
                dbnum,
                &file,
                base,
                target,
                STARTUP_CATALOGUE_WINDOW_RECORD_CAP,
            ) {
                Ok(window) => StaleCatalogueWindow {
                    report: CatalogueWindowReport::from_window(&window),
                    window: Some(window),
                },
                Err(error) => failed(format!("{error:#}")),
            }
        })
        .collect()
}

/// 把过期目录库的窗口分成**种子**与**退化库**（2026-09-06 计划 P3 护栏）：
///
/// - 窗口收不了（`error`）→ 退化：这一窗变了什么是未知，不是零；
/// - 导入级窗口（候选超上限、没逐件读）→ 退化：本来就不是精确反查的形状；
/// - 变动件数 > [`CATALOGUE_CASCADE_SEED_CAP`] → 退化；
/// - 其余的变动目录件（增 / 改 / 删都算，`build_cata_cascade_plan` 同一口径）进种子。
///
/// 零反向表 + 目录内部层种子那一档退化由 [`crate::data_interface::reverse_reference`] 自己判
/// （退路 (ii)，共识 d-665），不在这里重复。纯函数，单测钉它。
fn split_catalogue_seeds(
    collected: &[StaleCatalogueWindow],
) -> (
    Vec<crate::data_interface::reverse_reference::CatalogueSeed>,
    BTreeMap<u32, String>,
) {
    use crate::data_interface::reverse_reference::CatalogueSeed;
    let mut seeds = Vec::new();
    let mut degraded = BTreeMap::new();
    for item in collected {
        let dbnum = item.report.dbnum;
        match &item.window {
            None => {
                degraded.insert(
                    dbnum,
                    format!(
                        "目录库 {dbnum} 的窗口 {}→{} 收不了，这一窗变了什么未知：{}",
                        item.report.base_sesno,
                        item.report.target_sesno,
                        item.report.error.as_deref().unwrap_or("原因未记")
                    ),
                );
            }
            Some(window) if window.truncated_candidates.is_some() => {
                degraded.insert(
                    dbnum,
                    format!(
                        "目录库 {dbnum} 的窗口 {}→{} 索引候选 {} 个，超过逐条读记录的上限 \
                         {STARTUP_CATALOGUE_WINDOW_RECORD_CAP}（导入级窗口，不是精确反查的形状）",
                        window.base_sesno,
                        window.target_sesno,
                        window.truncated_candidates.unwrap_or_default()
                    ),
                );
            }
            Some(window) if window.changes.len() > CATALOGUE_CASCADE_SEED_CAP => {
                degraded.insert(
                    dbnum,
                    format!(
                        "目录库 {dbnum} 的窗口 {}→{} 变动目录件 {} 个，超过逐件反查的阈值 \
                         {CATALOGUE_CASCADE_SEED_CAP}",
                        window.base_sesno,
                        window.target_sesno,
                        window.changes.len()
                    ),
                );
            }
            Some(window) => {
                seeds.extend(window.changes.iter().map(|change| CatalogueSeed {
                    refno: change.refno(),
                    noun: change.noun.clone(),
                }));
            }
        }
    }
    (seeds, degraded)
}

/// P3 主体：过期目录库的窗口 → 反查 → 排工。返回回执；`None` = 没有过期库。
///
/// 反查在阻塞任务里（纯文件 I/O：MDB 每个 CATA / DESI 库开一次、点查反向表、设计库 owner 链
/// 上卷）。排工只对**落款仍在旧目录上**的 `gen_root` 行：重启前已按新目录重算过的根不重排
/// （`already_current`），从未生成过的根不排（懒生成，ADR-058 D6），限定域外 / 被发布队列
/// 阻断的库跳过。退化库（窗口级的 + 反查自己判的）的依赖根**全部**重排。
///
/// 任何一步没跑成都不中止启动：回执带 `error`、一根不排，受影响的根只剩按需 `ensure` 的目录
/// 指纹失配兜底——这与 P3 之前的行为逐位相同，只是现在会说出来。
async fn cascade_stale_catalogue_windows(
    collected: &[StaleCatalogueWindow],
) -> Option<crate::data_interface::reverse_reference::CatalogueCascadeReport> {
    use crate::data_interface::reverse_reference::CatalogueCascadeReport;
    if collected.is_empty() {
        return None;
    }
    let (seeds, window_degraded) = split_catalogue_seeds(collected);
    // 目录库钉在窗口的 T 端（= 文件最新）；设计库取最新。
    let pinned: BTreeMap<u32, u32> = collected
        .iter()
        .map(|item| (item.report.dbnum, item.report.target_sesno))
        .collect();
    let service = match crate::fast_model::e3d_model_service::E3dModelService::from_current().await
    {
        Ok(service) => service,
        Err(error) => {
            return Some(CatalogueCascadeReport::failed(format!(
                "open the current MDB for root resolution: {error:#}"
            )));
        }
    };
    let cascade =
        match tokio::task::spawn_blocking(move || run_catalogue_cascade(&service, &pinned, &seeds))
            .await
        {
            Ok(Ok(cascade)) => cascade,
            Ok(Err(error)) => {
                return Some(CatalogueCascadeReport::failed(format!("{error:#}")));
            }
            Err(error) => {
                return Some(CatalogueCascadeReport::failed(format!(
                    "reverse cascade task failed: {error}"
                )));
            }
        };
    let mut report = CatalogueCascadeReport::from_cascade(&cascade);
    for (dbnum, reason) in window_degraded {
        report.degraded.entry(dbnum).or_insert(reason);
    }
    match enqueue_catalogue_cascade(
        &cascade,
        &report.degraded.keys().copied().collect(),
        collected,
    )
    .await
    {
        Ok(account) => {
            report.enqueued = account.enqueued;
            report.enqueued_degraded = account.enqueued_degraded;
            report.already_current = account.already_current;
            report.lazy_roots = account.lazy_roots;
            report.skipped_out_of_scope = account.skipped_out_of_scope;
        }
        Err(error) => {
            report.error = Some(format!("enqueue the reverse cascade roots: {error:#}"));
        }
    }
    Some(report)
}

/// 反查本体（阻塞）：MDB 成员 → [`crate::data_interface::reverse_reference::SourceDb`]，
/// 上卷用一份 `DbSet@最新`（设计库都在里面，目录经 locator 解）。
fn run_catalogue_cascade(
    service: &crate::fast_model::e3d_model_service::E3dModelService,
    pinned: &BTreeMap<u32, u32>,
    seeds: &[crate::data_interface::reverse_reference::CatalogueSeed],
) -> anyhow::Result<crate::data_interface::reverse_reference::ReverseCascade> {
    use crate::data_interface::reverse_reference::{
        SourceDb, expand_reverse_cascade, resolve_root_from_set,
    };
    let (pins, _) = crate::fast_model::e3d_model_service::current_mdb_sources()?;
    let sources: Vec<SourceDb> = pins
        .into_iter()
        .filter(|pin| {
            pin.db_type.eq_ignore_ascii_case("DESI") || pin.db_type.eq_ignore_ascii_case("CATA")
        })
        .map(|pin| SourceDb {
            dbnum: pin.dbnum as u32,
            sesno: pinned.get(&(pin.dbnum as u32)).copied(),
            db_type: pin.db_type,
            file: pin.file,
        })
        .collect();
    let unit_types = crate::data_interface::generation_root::configured_delivery_unit_types();
    let mut set = None;
    expand_reverse_cascade(&sources, seeds, |dbnum, referrer| {
        if set.is_none() {
            set = Some(service.build_set(dbnum, None)?);
        }
        let set = set.as_ref().expect("just built");
        Ok(resolve_root_from_set(set, &unit_types, referrer))
    })
}

/// [`enqueue_catalogue_cascade`] 的账。
#[derive(Debug, Default)]
struct CatalogueCascadeEnqueue {
    enqueued: BTreeMap<u32, usize>,
    enqueued_degraded: BTreeMap<u32, usize>,
    already_current: usize,
    lazy_roots: usize,
    skipped_out_of_scope: usize,
}

/// 把反查命中的根与退化库的依赖根排成**带明确根名单的 ModelOnly 任务**（每个设计库一条）。
///
/// 「落款仍在旧目录上」逐个过期目录库问一次 [`roots_behind_catalogue_on`]（`gen_root` 里
/// `published_model_target.catalogue` 记着 `dbnum = X 且 session < T` 的行）：命中根只有在这份
/// 名单里才排，退化库则整份名单都排。任务的目标会话取设计库文件最新（ADR-054：生成时点是文件最新），
/// `model_start_sesno` 也取它——规划器于是不开窗，只有名单里的根与本就未安定的根进工作单。
///
/// 2026-09-10（ADR-071 S4）之前这里写的是 `model_update_pending` 行等空闲轮 drain 领；现在名单随
/// [`DiscoveredBatch::regen_roots`] 进队列行、随冻结进任务，任务体并进自己的工作单——目录级联与
/// 别的模型工作一样，是一个 dbnum 任务的事，不是第二条队列。
async fn enqueue_catalogue_cascade(
    cascade: &crate::data_interface::reverse_reference::ReverseCascade,
    degraded: &BTreeSet<u32>,
    collected: &[StaleCatalogueWindow],
) -> anyhow::Result<CatalogueCascadeEnqueue> {
    let mut account = CatalogueCascadeEnqueue::default();
    // 设计库 → 落款仍在旧目录上的根（refno 文本 → noun）；退化库那份单独留着。
    let mut behind: BTreeMap<u32, BTreeMap<String, String>> = BTreeMap::new();
    let mut behind_degraded: BTreeMap<u32, BTreeMap<String, String>> = BTreeMap::new();
    for item in collected {
        let rows =
            roots_behind_catalogue_on(&SUL_DB, item.report.dbnum, item.report.target_sesno).await?;
        for (dbnum, roots) in rows {
            let bucket = behind.entry(dbnum).or_default();
            for root in &roots {
                bucket.insert(root.refno.to_pdms_str(), root.noun.clone());
            }
            if degraded.contains(&item.report.dbnum) {
                let bucket = behind_degraded.entry(dbnum).or_default();
                for root in roots {
                    bucket.insert(root.refno.to_pdms_str(), root.noun);
                }
            }
        }
    }
    let in_scope = |dbnum: u32| crate::data_interface::watch_scope::admits(dbnum);
    // 设计库 → （文件最新版本，要重算的根 refno → noun）。
    let mut planned: BTreeMap<
        u32,
        (
            crate::data_interface::model_source::SourceVersion,
            BTreeMap<String, String>,
        ),
    > = BTreeMap::new();
    let mut versions: BTreeMap<u32, Option<crate::data_interface::model_source::SourceVersion>> =
        BTreeMap::new();
    // 1. 反查命中的根：落款仍旧的排；其余分「已重算过」（有 gen_root 行）与「从未生成」。
    for (dbnum, roots) in &cascade.roots {
        if !in_scope(*dbnum) {
            account.skipped_out_of_scope += roots.len();
            continue;
        }
        let stale_roots = behind.get(dbnum);
        let mut misses = Vec::new();
        for root in roots {
            if !stale_roots.is_some_and(|stale| stale.contains_key(&root.refno)) {
                misses.push(root.refno.as_str());
                continue;
            }
            let version = latest_design_version(&mut versions, *dbnum)
                .await
                .with_context(|| format!("dbnum {dbnum} 取不到文件最新会话，命中的根排不了"))?;
            planned
                .entry(*dbnum)
                .or_insert_with(|| (version, BTreeMap::new()))
                .1
                .insert(root.refno.clone(), root.noun.clone());
            *account.enqueued.entry(*dbnum).or_default() += 1;
        }
        if !misses.is_empty() {
            let persisted: BTreeSet<String> =
                crate::data_interface::window_root_plan::load_persisted_root_states(*dbnum)
                    .await?
                    .into_iter()
                    .map(|state| state.root.refno.to_pdms_str())
                    .collect();
            for refno in misses {
                if persisted.contains(refno) {
                    account.already_current += 1;
                } else {
                    account.lazy_roots += 1;
                }
            }
        }
    }
    // 2. 退化库的依赖根全部过期（已在 1 里排过的不重复计）。
    for (dbnum, roots) in behind_degraded {
        if !in_scope(dbnum) {
            account.skipped_out_of_scope += roots.len();
            continue;
        }
        for (refno, noun) in roots {
            if planned
                .get(&dbnum)
                .is_some_and(|(_, planned)| planned.contains_key(&refno))
            {
                continue;
            }
            let version = latest_design_version(&mut versions, dbnum)
                .await
                .with_context(|| {
                    format!("dbnum {dbnum} 取不到文件最新会话，退化库的依赖根排不了")
                })?;
            planned
                .entry(dbnum)
                .or_insert_with(|| (version, BTreeMap::new()))
                .1
                .insert(refno, noun);
            *account.enqueued_degraded.entry(dbnum).or_default() += 1;
        }
    }
    // 3. 每个设计库一条 ModelOnly 任务，名单随行。已有排队行就并进去（名单取并集），同库任务
    //    在跑就接在它后面——都是 `BatchScheduler::enqueue` 的事。
    let project = aios_core::get_db_option().project_name.clone();
    for (dbnum, (version, roots)) in planned {
        let latest = version.sesno as i32;
        let found = crate::data_interface::batch_scheduler::DiscoveredBatch {
            project: project.clone(),
            dbnum,
            db_type: version.db_type.clone(),
            phase: crate::data_interface::initialization_phase::DataPhase::Design,
            epoch_id: 0,
            intent: crate::data_interface::batch_queue::BatchIntent::ModelOnly,
            // 下限 = 目标：规划器不开窗（凭证等于右端即追平），进工作单的只有名单与本就未安定的根。
            model_start_sesno: Some(latest),
            file_name: version
                .file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: version.file.clone(),
            applied_sesno: latest,
            file_latest_sesno: latest,
            previous_observed_sesno: latest,
            first_pending_sesno_time: None,
            file_latest_sesno_time: version.session_time.clone(),
            reinit_requested: false,
            regen_roots: roots
                .into_iter()
                .map(
                    |(refno, noun)| crate::data_interface::batch_scheduler::RegenRoot {
                        refno,
                        noun,
                    },
                )
                .collect(),
        };
        let outcome = crate::data_interface::batch_scheduler::BatchScheduler::global().enqueue(
            crate::data_interface::task_registry::TaskRegistry::global(),
            &found,
            false,
        );
        println!(
            "目录级联已排 ModelOnly 任务 dbnum={dbnum} roots={} target={latest} outcome={:?} task={}",
            found.regen_roots.len(),
            outcome.outcome,
            outcome.info.task_id
        );
    }
    Ok(account)
}

/// 设计库文件最新会话，同一库只问一次（`latest_source_version` 自己还有按文件戳的缓存）。
async fn latest_design_version(
    versions: &mut BTreeMap<u32, Option<crate::data_interface::model_source::SourceVersion>>,
    dbnum: u32,
) -> anyhow::Result<crate::data_interface::model_source::SourceVersion> {
    if !versions.contains_key(&dbnum) {
        let version = tokio::task::spawn_blocking(move || {
            crate::data_interface::model_source::latest_source_version(dbnum)
        })
        .await
        .ok()
        .and_then(Result::ok);
        versions.insert(dbnum, version);
    }
    versions[&dbnum]
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no readable latest session for dbnum {dbnum}"))
}

/// `gen_root` 里**落款仍在旧目录上**的根：`published_model_target.catalogue` 记着
/// `dbnum = cata_dbnum 且 session < target_sesno` 的行，按设计库分。
///
/// 这是 P3 排工的过滤器：反查命中的根只有在这份名单里才需要重算——重启前已按新目录
/// 重算过的根落款已到 `T`，再排一遍是白算；退化库则整份名单都排。一条语句扫全表
/// （启动一次、每个过期目录库一问），不逐根点查。
pub(crate) async fn roots_behind_catalogue_on(
    db: &Surreal<Any>,
    cata_dbnum: u32,
    target_sesno: u32,
) -> anyhow::Result<BTreeMap<u32, Vec<crate::data_interface::window_root_plan::PersistedRoot>>> {
    #[derive(Debug, Deserialize)]
    struct Row {
        pe: surrealdb::sql::Thing,
        #[serde(default)]
        dbnum: Option<u32>,
        #[serde(default)]
        noun: String,
    }
    let started = Instant::now();
    let mut response = db
        .query(format!(
            "SELECT pe ?: type::thing('pe', type::string(record::id(id))) AS pe, dbnum, \
             noun ?: '' AS noun FROM gen_root WHERE dbnum != NONE AND \
             array::len(published_model_target.catalogue[WHERE dbnum = {cata_dbnum} \
             AND session < {target_sesno}] ?? []) > 0;"
        ))
        .await
        .with_context(|| {
            format!(
                "load gen_root rows still stamped on catalogue {cata_dbnum} before {target_sesno}"
            )
        })?
        .check()
        .with_context(|| format!("gen_root catalogue stamp statement for {cata_dbnum}"))?;
    crate::data_interface::model_concurrency::record_surreal_read(started.elapsed());
    let rows: Vec<Row> = response
        .take(0)
        .with_context(|| format!("decode gen_root rows stamped on catalogue {cata_dbnum}"))?;
    let mut out: BTreeMap<u32, Vec<crate::data_interface::window_root_plan::PersistedRoot>> =
        BTreeMap::new();
    for row in rows {
        let Some(dbnum) = row.dbnum else { continue };
        out.entry(dbnum).or_default().push(
            crate::data_interface::window_root_plan::PersistedRoot {
                refno: crate::data_interface::helper::pe_thing_to_refno(row.pe)?,
                noun: row.noun,
            },
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P2（2026-09-06 计划）：过期目录库的窗口要在两个出口都进 `/health`，横幅逐库一行，
    /// 而且收窗只对**过期**库做（S 取记在案的最旧会话、T 取文件最新）——不是对全部记在案的库。
    #[test]
    fn stale_catalogue_windows_reach_both_exits_and_the_banner() {
        // 这棵树 CRLF / LF 混用，`include_str!` 原样带回来；探针按 LF 写，先归一。
        let source = include_str!("model_coverage.rs").replace("\r\n", "\n");
        // `inner` 自 S5a 起住在 `startup_reconcile.rs`；目录版本核对本体还在本文件。
        let startup = include_str!("startup_reconcile.rs").replace("\r\n", "\n");
        let inner = startup
            .split_once("async fn reconcile_model_coverage_at_startup_inner()")
            .expect("inner exists")
            .1;
        for (probe, why) in [
            ("windows: catalogue_windows,", "从核对结果里解构出来"),
            (
                "coverage.reconcile.catalogue_windows = catalogue_windows;",
                "gen_root 无凭证的早退出口也要带上它",
            ),
            (
                "unreadable_catalogue_dbnums: unreadable_catalogue,\n        catalogue_windows,",
                "正常出口的结果结构里紧跟目录那三格",
            ),
        ] {
            assert!(inner.contains(probe), "{why}: 找不到 {probe:?}");
        }
        assert!(
            inner.contains("println!(\"  {}\", window.banner_line());"),
            "每个过期库的窗口要在横幅上占一行"
        );
        let reconcile = source
            .split_once("async fn reconcile_catalogue_versions()")
            .expect("reconcile exists")
            .1
            .split_once("fn collect_stale_catalogue_windows(")
            .expect("collector follows")
            .0;
        assert!(
            reconcile.contains("let spans: Vec<(u32, u32, u32)> = stale"),
            "只对过期库收窗，S/T 各取记在案与文件最新: {reconcile}"
        );
        assert!(
            reconcile.contains("spawn_blocking(move || collect_stale_catalogue_windows(&spans))"),
            "收窗是文件 I/O，必须在阻塞任务里"
        );
    }

    fn stale_window(
        dbnum: u32,
        changes: usize,
        truncated: Option<usize>,
        error: Option<&str>,
    ) -> StaleCatalogueWindow {
        use crate::data_interface::cata_cascade_window::{
            CatalogueChange, CatalogueChangeKind, CatalogueWindow, CatalogueWindowReport,
        };
        if let Some(error) = error {
            return StaleCatalogueWindow {
                report: CatalogueWindowReport::failed(dbnum, 1, 2, error.to_string()),
                window: None,
            };
        }
        let window = CatalogueWindow {
            dbnum,
            base_sesno: 1,
            target_sesno: 2,
            changes: (0..changes)
                .map(|seq| CatalogueChange {
                    refno: format!("{dbnum}/{seq}"),
                    key: (dbnum, seq as u32),
                    kind: if seq % 3 == 0 {
                        CatalogueChangeKind::Added
                    } else if seq % 3 == 1 {
                        CatalogueChangeKind::Modified
                    } else {
                        CatalogueChangeKind::Deleted
                    },
                    noun: (seq % 2 == 0).then(|| "SCOM".to_string()),
                    owner: None,
                })
                .collect(),
            relocated_only: 0,
            unreadable_records: 0,
            truncated_candidates: truncated,
            reference_tables: vec!["CATR".into()],
        };
        StaleCatalogueWindow {
            report: CatalogueWindowReport::from_window(&window),
            window: Some(window),
        }
    }

    /// P3 护栏（2026-09-06 计划）：收不了 / 导入级 / 超阈的窗口退化并各说各的原因，其余窗口的
    /// 变动目录件——增、改、删都算——进种子；退化库的一件都不进种子。
    #[test]
    fn catalogue_seeds_split_degraded_windows_from_seeds() {
        let collected = vec![
            stale_window(7000, 3, None, None),
            stale_window(7001, 0, None, Some("session 1 is not on the chain")),
            stale_window(7002, 0, Some(48_213), None),
            stale_window(7003, CATALOGUE_CASCADE_SEED_CAP + 1, None, None),
            stale_window(7004, CATALOGUE_CASCADE_SEED_CAP, None, None),
        ];
        let (seeds, degraded) = split_catalogue_seeds(&collected);
        assert_eq!(seeds.len(), 3 + CATALOGUE_CASCADE_SEED_CAP);
        assert!(seeds.iter().all(|seed| {
            let dbnum = seed.refno.dbno();
            dbnum == 7000 || dbnum == 7004
        }));
        let odd = |n: usize| (0..n).filter(|seq| seq % 2 == 1).count();
        assert_eq!(
            seeds.iter().filter(|seed| seed.noun.is_none()).count(),
            odd(3) + odd(CATALOGUE_CASCADE_SEED_CAP),
            "读不出 noun 的种子照收，闭包会再读记录头"
        );
        assert_eq!(
            degraded.keys().copied().collect::<Vec<_>>(),
            vec![7001, 7002, 7003]
        );
        assert!(degraded[&7001].contains("收不了"), "{}", degraded[&7001]);
        assert!(degraded[&7001].contains("not on the chain"));
        assert!(degraded[&7002].contains("48213 个"), "{}", degraded[&7002]);
        assert!(degraded[&7002].contains("导入级窗口"));
        assert!(
            degraded[&7003].contains("变动目录件 51 个"),
            "{}",
            degraded[&7003]
        );
        assert!(degraded[&7003].contains("阈值 50"));
    }

    /// 没有过期库就没有反查——`None`，而不是一份全 0 的回执（`/health` 上 `null` 与 0 是两句话）。
    #[tokio::test]
    async fn no_stale_catalogue_means_no_cascade_report() {
        assert!(cascade_stale_catalogue_windows(&[]).await.is_none());
    }

    /// 「落款仍在旧目录上」的过滤器真跑一遍：只有 `catalogue` 里记着 `dbnum = X 且 session < T`
    /// 的根进名单，按设计库分；落款已到 T 的、记的是别的目录库的、没有目录段的都不进。
    #[tokio::test]
    async fn roots_behind_a_catalogue_are_those_still_stamped_before_its_target() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("roots_behind_catalogue")
            .await
            .expect("select fixture db");
        db.query(
            "CREATE gen_root:8000_10 SET dbnum = 8000, noun = 'BRAN', published_model_target = { \
               catalogue: [ \
                 { dbnum: 7000, db_type: 'CATA', file: '/cat7000', session: 184, digest: 'd1' }, \
                 { dbnum: 7001, db_type: 'CATA', file: '/cat7001', session: 3, digest: 'd2' } \
               ] };\
             CREATE gen_root:8000_11 SET dbnum = 8000, noun = 'EQUI', published_model_target = { \
               catalogue: [ \
                 { dbnum: 7000, db_type: 'CATA', file: '/cat7000', session: 185, digest: 'd3' } \
               ] };\
             CREATE gen_root:8001_12 SET dbnum = 8001, noun = 'BRAN', published_model_target = { \
               catalogue: [ \
                 { dbnum: 7000, db_type: 'CATA', file: '/cat7000', session: 100, digest: 'd4' } \
               ] };\
             CREATE gen_root:8001_13 SET dbnum = 8001, noun = 'BRAN', published_model_target = { \
               catalogue: [ \
                 { dbnum: 7001, db_type: 'CATA', file: '/cat7001', session: 1, digest: 'd5' } \
               ] };\
             CREATE gen_root:8001_14 SET dbnum = 8001, status = 'Generated';",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed statements");

        let behind = roots_behind_catalogue_on(&db, 7000, 185)
            .await
            .expect("query catalogue stamps");
        let flat: Vec<(u32, String, String)> = behind
            .iter()
            .flat_map(|(dbnum, roots)| {
                roots
                    .iter()
                    .map(move |root| (*dbnum, root.refno.to_pdms_str(), root.noun.clone()))
            })
            .collect();
        assert_eq!(
            flat,
            vec![
                (8000, "8000/10".to_string(), "BRAN".to_string()),
                (8001, "8001/12".to_string(), "BRAN".to_string()),
            ],
            "8000/11 已落在 185 上、8001/13 记的是 7001、8001/14 没有目录段——都不在名单里"
        );
        assert!(
            roots_behind_catalogue_on(&db, 7000, 100)
                .await
                .expect("query")
                .is_empty(),
            "T 不高于任何落款时名单为空"
        );
        let other = roots_behind_catalogue_on(&db, 7001, 4)
            .await
            .expect("query");
        assert_eq!(other.len(), 2);
        assert_eq!(other[&8000].len(), 1);
        assert_eq!(other[&8001].len(), 1);
    }

    /// 模型门的 SQL 与内存判据必须是同一口径：不早于文件最新会话即覆盖，
    /// 未认领永不覆盖，时刻不比。
    #[test]
    fn model_gate_sql_is_monotone_and_ignores_the_time_column() {
        let body = include_str!("model_coverage.rs")
            .split_once("pub async fn model_coverage_current(")
            .expect("model gate exists")
            .1
            .split_once("#[derive(Debug, Deserialize)]")
            .expect("model gate ends before the next row type")
            .0;
        assert!(body.contains("(source_end_sesno?:0) <= 0 OR (source_end_sesno?:0) < {}"));
        assert!(body.contains("library_coverage_on(&SUL_DB, dbnum)"), "库级覆盖读水位镜像，不再问收据");
        assert!(
            !body.contains("source_end_sesno_time"),
            "时刻列只记不比（ADR-054）: {body}"
        );
        assert!(
            !body.contains("!= {}"),
            "等值判据会让 ensure 生成的更新版本被门判成缺口: {body}"
        );
        assert!(
            body.contains("latest_source_version(dbnum)"),
            "模型门必须与启动复核共用文件最新会话标尺: {body}"
        );
        assert!(
            !body.contains("applied_sesno"),
            "数据应用水位不代表模型生成时点（ADR-054）: {body}"
        );
    }
}
