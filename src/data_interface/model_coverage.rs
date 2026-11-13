//! 模型覆盖的**只读取数**（ADR-058 D5 / ADR-054 / ADR-079）：「这个库的模型是照哪一版数据与目录
//! 算出来的」怎么读——按需 `ensure` 的凭证命中判据（[`generation_roots_cache_current`]，设计会话 +
//! 目录落款两半）、空闲轮的模型门（[`model_coverage_current`]）、已发布的模型落在哪一版目录上
//! （[`published_catalogue_sessions_on`]）。
//!
//! `gen_root` 表退役之后（ADR-079）这里读的只有两样：**水位行**（`dbnum_watermark.model_sesno` 是模型面
//! 唯一的持久事实；`model_catalogue_sessions` 是库级目录落款——逐根的 `published_model_target.catalogue`
//! 收成每库一份）与**进程内的生成根账本**（[`crate::data_interface::model_root_ledger`]：本进程里哪些根
//! 失败了、哪些按需生成到了更新的一版）。一条 SurrealDB 逐根凭证行都不读，一条都不碰 `model_update_pending`。
//!
//! 2026-09-10（ADR-071 S7）从 `model_update_pending.rs` 搬出。同日稍后**目录版本核对**（[`reconcile_catalogue_versions`]
//! 点名 → 收窗 → 反查 → 排工，与运行期的 `_after_sweep` / `_now` 两个入口、`/health.catalogue_reconcile`
//! 快照）也搬了进来：它的排工自 ADR-071 S4 起是**排带明确根名单的 ModelOnly 任务**
//! （`DiscoveredBatch::regen_roots`），ADR-079 起任务还带着「成了就盖」的库级目录落款
//! （`DiscoveredBatch::catalogue_stamps`）。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use aios_core::SUL_DB;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

/// 一条目录落款里与凭证判据有关的两格（库级 `model_catalogue_sessions[*]`，ADR-079）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct CatalogueStamp {
    #[serde(default)]
    pub dbnum: Option<u32>,
    #[serde(default)]
    pub session: Option<u32>,
}

impl From<crate::data_interface::dbnum_state::CatalogueStampRow> for CatalogueStamp {
    fn from(row: crate::data_interface::dbnum_state::CatalogueStampRow) -> Self {
        Self {
            dbnum: Some(row.dbnum),
            session: Some(row.session),
        }
    }
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
/// 仅有 `inst_relate` 行不构成缓存命中：它可能属于较早会话。凭证会话号**不早于**文件最新会话
/// 才算覆盖（ADR-054 实施约束 4：单调，不判等值）。文件被换回更旧版本时凭证会显得「超前」，
/// 那是文件异常，由重建批次（ADR-021）清凭证，不在这里猜。判据见 [`generation_roots_cache_current`]。
pub async fn generation_root_cache_current(root_refno: &str) -> anyhow::Result<bool> {
    Ok(generation_roots_cache_current(&[root_refno.to_string()])
        .await?
        .contains(root_refno))
}

/// 库级凭证的两半（ADR-079）：这个库的模型追平到哪一版、照的是哪几版目录。`None` = 库上没有水位行。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LibraryCredential {
    /// `dbnum_watermark.model_sesno`；`0` = 从没追平过。
    pub model_sesno: i32,
    /// `dbnum_watermark.model_catalogue_sessions`。
    pub catalogue: Vec<CatalogueStamp>,
}

/// 一根的凭证判据（纯函数，ADR-079）：**库级承诺 ∨ 根级按需生成**，且这根在本进程里没有失败悬着。
///
/// - 库级：`model_sesno` 不早于这根生成源的文件最新（任务收尾只在这一版的根全部落库时才盖它，
///   ADR-072 D4，所以它覆盖到的每个根都算追平）∧ 库级目录落款不早于各目录文件最新
///   （[`catalogue_stamps_current`]，H1 (c)）；
/// - 根级：账本里这根 `Generated` 且 `source_end_sesno` 不早于文件最新——按需 `ensure` 在库水位之后
///   生成的根走这一半（它照的目录版本由投影回执那一半核，`on_demand_model::memory_root_current`）；
/// - 账本里这根还欠着（失败 / 永久放弃）→ 未命中，不管库级承诺说什么：那份几何不可信。
///
/// 从没生成过、账本里没有的根，只看库级承诺：库追平了它就追平了（S 时刻每个根都已发布正是水位
/// 承诺的东西）。
pub(crate) fn root_credential_covers(
    library: Option<&LibraryCredential>,
    ledger_entry: Option<&crate::data_interface::model_root_ledger::RootEntry>,
    required_sesno: i32,
    catalogue_latest: impl Fn(u32) -> Option<u32>,
) -> bool {
    if ledger_entry.is_some_and(|entry| !entry.is_settled()) {
        return false;
    }
    let library_covers = library.is_some_and(|library| {
        library.model_sesno > 0
            && library.model_sesno >= required_sesno
            && catalogue_stamps_current(&library.catalogue, &catalogue_latest)
    });
    let root_covers = ledger_entry.is_some_and(|entry| {
        crate::data_interface::model_source::credential_covers(
            Some(entry.source_end_sesno),
            required_sesno,
        )
    });
    library_covers || root_covers
}

/// Batch form used by subtree display: resolve file versions off-thread once,
/// then judge every root from the watermark mirror and the process ledger — no
/// per-root table read (ADR-079).
///
/// 命中 = [`root_credential_covers`]。目录文件最新会话按 dbnum 只读一次（`latest_source_version`
/// 自己还有按文件戳的缓存），一次子树显示多付的是几个目录库的头。
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

    let mut libraries: BTreeMap<u32, Option<LibraryCredential>> = BTreeMap::new();
    for dbnum in versions
        .iter()
        .map(|(_, dbnum, _)| *dbnum)
        .collect::<BTreeSet<_>>()
    {
        libraries.insert(dbnum, library_credential_on(&SUL_DB, dbnum).await?);
    }
    // 目录文件最新会话：这批库的落款提到的每个目录库问一次。读不到的留 `None`（未知）。
    let catalogue_dbnums: BTreeSet<u32> = libraries
        .values()
        .flatten()
        .flat_map(|library| library.catalogue.iter().filter_map(|stamp| stamp.dbnum))
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
    Ok(versions
        .into_iter()
        .filter_map(|(root_refno, dbnum, required_sesno)| {
            let entry = crate::data_interface::model_root_ledger::parse_root(&root_refno)
                .and_then(|root| crate::data_interface::model_root_ledger::entry(dbnum, root));
            root_credential_covers(
                libraries.get(&dbnum).and_then(Option::as_ref),
                entry.as_ref(),
                required_sesno,
                |dbnum| catalogue_latest.get(&dbnum).copied().flatten(),
            )
            .then_some(root_refno)
        })
        .collect())
}

/// 库级凭证：这个库追平到哪一版（`dbnum_watermark.model_sesno`）、照的哪几版目录
/// （`model_catalogue_sessions`，ADR-079）。
///
/// 任务收尾只在这一版的根**全部**落进去时才盖水位（ADR-072 D4 / `task_end::render_task_end_tail`
/// 的 `TaskModelOutcome::all_roots_ok` 守卫），所以这一格本身就是凭据；原先另开一张
/// `CompletionReceipt` 收据再交叉核验的那一路随 publish 阶段退了。`None` = 库上没有水位行。
/// 生产库走水位镜像（`DbnumState::read`），测试夹具与旁路连接直查那一行。
async fn library_credential_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<Option<LibraryCredential>> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        model_sesno: Option<i32>,
        #[serde(default)]
        model_catalogue_sessions:
            Option<Vec<crate::data_interface::dbnum_state::CatalogueStampRow>>,
    }
    if std::ptr::eq(db, &*SUL_DB) {
        return Ok(crate::data_interface::dbnum_state::DbnumState::read(dbnum)
            .await?
            .map(|state| LibraryCredential {
                model_sesno: state.model_sesno,
                catalogue: state
                    .model_catalogue_sessions
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            }));
    }
    let row: Option<Row> = db
        .query(format!(
            "SELECT model_sesno, model_catalogue_sessions FROM ONLY dbnum_watermark:{dbnum};"
        ))
        .await?
        .check()?
        .take(0)?;
    Ok(row.map(|row| LibraryCredential {
        model_sesno: row.model_sesno.unwrap_or_default(),
        catalogue: row
            .model_catalogue_sessions
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect(),
    }))
}

/// 库级模型覆盖：这个库追平到哪一版——就是 `dbnum_watermark.model_sesno`。`None` = 从没追平过。
async fn library_coverage_on(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<Option<i32>> {
    Ok(library_credential_on(db, dbnum)
        .await?
        .map(|library| library.model_sesno)
        .filter(|sesno| *sesno > 0))
}

/// ADR-079：凭证判据 = 库级承诺 ∨ 根级按需生成，账本里悬着的失败一票否决。
#[cfg(test)]
#[test]
fn a_root_is_current_when_the_library_promise_or_its_own_ledger_entry_covers_the_file() {
    use crate::data_interface::model_root_ledger::{RootEntry, RootOutcome};
    let stamp = |dbnum: u32, session: u32| CatalogueStamp {
        dbnum: Some(dbnum),
        session: Some(session),
    };
    let latest = |dbnum: u32| (dbnum == 7000).then_some(12);
    let library = LibraryCredential {
        model_sesno: 26,
        catalogue: vec![stamp(7000, 12)],
    };
    let generated = |sesno: i32| RootEntry {
        noun: "BRAN".into(),
        outcome: RootOutcome::Generated,
        source_end_sesno: sesno,
        failure_streak: 0,
        consecutive_failures: 0,
        retry_at: None,
    };
    let failed = RootEntry {
        noun: "BRAN".into(),
        outcome: RootOutcome::Failed {
            error: "boom".into(),
        },
        source_end_sesno: 26,
        failure_streak: 1,
        consecutive_failures: 1,
        retry_at: None,
    };
    // 库追平到 26、目录落款追平：账本里没有的根命中；要求 27 就不命中。
    assert!(root_credential_covers(Some(&library), None, 26, latest));
    assert!(!root_credential_covers(Some(&library), None, 27, latest));
    // 库级不够，按需生成到了 27：根级那一半命中。
    assert!(root_credential_covers(
        Some(&library),
        Some(&generated(27)),
        27,
        latest
    ));
    assert!(!root_credential_covers(
        Some(&library),
        Some(&generated(26)),
        27,
        latest
    ));
    // 账本里悬着失败：库级承诺也救不了。
    assert!(!root_credential_covers(
        Some(&library),
        Some(&failed),
        26,
        latest
    ));
    // 目录落款早于目录文件最新：库级那一半失效，只剩根级。
    let stale_catalogue = LibraryCredential {
        model_sesno: 26,
        catalogue: vec![stamp(7000, 11)],
    };
    assert!(!root_credential_covers(
        Some(&stale_catalogue),
        None,
        26,
        latest
    ));
    assert!(root_credential_covers(
        Some(&stale_catalogue),
        Some(&generated(26)),
        26,
        latest
    ));
    // 没有水位行 / 从没追平：只看根级。
    assert!(!root_credential_covers(None, None, 1, latest));
    let never = LibraryCredential {
        model_sesno: 0,
        catalogue: Vec::new(),
    };
    assert!(!root_credential_covers(Some(&never), None, 1, latest));
    assert!(root_credential_covers(
        Some(&never),
        Some(&generated(1)),
        1,
        latest
    ));
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

/// ADR-025 模型门：这个库的模型是否追平**文件最新会话**、且本进程里没有根悬着失败。
///
/// 标尺是文件最新，不是 `applied_sesno`（ADR-054：水位只承诺数据落库，不是生成时点；
/// ADR-058 D4 的启动复核也以文件最新为右端）。两边若不同尺，会出现启动复核刚判定
/// 「落后、已开窗排队」而空闲轮同一刻判定「覆盖完整、模型就绪」——两个都对，只是
/// 各拿一把尺。文件读取是同步 I/O（stat + 会话页），扔进阻塞线程。
///
/// 判据（ADR-079）：`model_sesno` **不早于**标尺（ADR-054 实施约束 4：单调；`0` = 从没追平，永不算）
/// ∧ 进程内账本里这个库没有还在赶的根。库里不再有逐根的行可数，也不需要——水位只在这一版的根
/// 全部落库时才盖，本进程之后又失败的根在账本里。
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
    let chasing = crate::data_interface::model_root_ledger::census(dbnum).chasing;
    Ok(covered && chasing == 0)
}

/// 每个设计库的库级目录落款（`dbnum_watermark.model_catalogue_sessions`，ADR-079）：
/// `DESI dbnum → (CATA dbnum → 会话)`。没有落款的库不在里面。
///
/// 一条语句问完全部库的水位行（表很小，每库一行）；不逐库点查。
pub(crate) async fn library_catalogue_stamps_on(
    db: &Surreal<Any>,
) -> anyhow::Result<BTreeMap<u32, BTreeMap<u32, u32>>> {
    #[derive(Debug, Deserialize)]
    struct Row {
        dbnum: u32,
        #[serde(default)]
        model_catalogue_sessions:
            Option<Vec<crate::data_interface::dbnum_state::CatalogueStampRow>>,
    }
    let started = Instant::now();
    let mut response = db
        .query(
            "SELECT dbnum, model_catalogue_sessions FROM dbnum_watermark \
             WHERE model_catalogue_sessions != NONE AND array::len(model_catalogue_sessions) > 0;",
        )
        .await?
        .check()?;
    crate::data_interface::model_concurrency::record_surreal_read(started.elapsed());
    let mut out: BTreeMap<u32, BTreeMap<u32, u32>> = BTreeMap::new();
    for row in response.take::<Vec<Row>>(0)? {
        let stamps = row.model_catalogue_sessions.unwrap_or_default();
        if stamps.is_empty() {
            continue;
        }
        let entry = out.entry(row.dbnum).or_default();
        for stamp in stamps {
            entry
                .entry(stamp.dbnum)
                .and_modify(|kept| *kept = (*kept).min(stamp.session))
                .or_insert(stamp.session);
        }
    }
    Ok(out)
}

/// 已发布的模型落在哪一版目录上：`CATA dbnum → 记在案的最旧会话`（纯函数，输入是各库的落款）。
///
/// 同一个 CATA 库会被不同 DESI 库、不同时刻的任务记成不同会话；取**最小**那个：这里问的
/// 是「有没有库落在旧目录上」，最旧的那一版说了算。
pub(crate) fn oldest_catalogue_sessions(
    libraries: &BTreeMap<u32, BTreeMap<u32, u32>>,
) -> BTreeMap<u32, u32> {
    let mut oldest: BTreeMap<u32, u32> = BTreeMap::new();
    for stamps in libraries.values() {
        for (&dbnum, &session) in stamps {
            oldest
                .entry(dbnum)
                .and_modify(|kept| *kept = (*kept).min(session))
                .or_insert(session);
        }
    }
    oldest
}

/// 已发布的模型落在哪一版目录上：`CATA dbnum → 记在案的最旧会话`。
///
/// 来源是水位行上的库级目录落款（ADR-079：随 `gen_root` 退役，逐根的 `published_model_target.catalogue`
/// 收成每库一份）。这是**模型面自己**记下的「我是照哪一版目录算出来的」，不经 `pe`（ADR-056 N7 / ADR-054）。
pub(crate) async fn published_catalogue_sessions_on(
    db: &Surreal<Any>,
) -> anyhow::Result<BTreeMap<u32, u32>> {
    Ok(oldest_catalogue_sessions(
        &library_catalogue_stamps_on(db).await?,
    ))
}

/// 一组设计库共用的「目录文件宇宙」：与 MDB 声明那份**不同**的目录库 → 这组库生成真读的副本。
/// 空 = 全按 MDB 声明（绝大多数库）。
pub(crate) type CatalogueFileOverrides = BTreeMap<u32, PathBuf>;

/// 按目录文件把设计库分组（纯函数，GitHub #47）：库级落款记的是生成**真读的那份**文件的会话，
/// 核对要在同一份文件上比。`file_for(设计库, 目录库)` 是生成的选法（[`crate::fast_model::e3d_model_service::CatalogueFileChoice::for_library`]），
/// `declared(目录库)` 是 MDB 声明那份；两者不同（路径不分大小写比）的目录库进这一库的覆盖表，覆盖表相同的库
/// 进同一组。生成解不出的目录库不进覆盖表——它在核对里照旧落「读不到」。
pub(crate) fn group_libraries_by_catalogue_files(
    libraries: &BTreeMap<u32, BTreeMap<u32, u32>>,
    file_for: impl Fn(u32, u32) -> Option<PathBuf>,
    declared: impl Fn(u32) -> Option<PathBuf>,
) -> BTreeMap<CatalogueFileOverrides, BTreeMap<u32, BTreeMap<u32, u32>>> {
    let same_file = |a: &Path, b: &Path| {
        a.to_string_lossy().to_ascii_lowercase() == b.to_string_lossy().to_ascii_lowercase()
    };
    let mut groups: BTreeMap<CatalogueFileOverrides, BTreeMap<u32, BTreeMap<u32, u32>>> =
        BTreeMap::new();
    for (&library, stamps) in libraries {
        let overrides: CatalogueFileOverrides = stamps
            .keys()
            .filter_map(|&catalogue| {
                let read = file_for(library, catalogue)?;
                let is_copy =
                    declared(catalogue).is_none_or(|declared| !same_file(&declared, &read));
                is_copy.then_some((catalogue, read))
            })
            .collect();
        groups
            .entry(overrides)
            .or_default()
            .insert(library, stamps.clone());
    }
    groups
}

/// **落款仍在旧目录上**的设计库（纯函数，ADR-079 的库级替身，原 `roots_behind_catalogue_on`）：
/// 库级落款里记着 `cata_dbnum` 且 `session < target_sesno` 的库。没有该目录落款的库不在名单上
/// （它的模型没照过这个目录，或从没生成过）。
pub(crate) fn libraries_behind_catalogue(
    libraries: &BTreeMap<u32, BTreeMap<u32, u32>>,
    cata_dbnum: u32,
    target_sesno: u32,
) -> BTreeSet<u32> {
    libraries
        .iter()
        .filter(|(_, stamps)| {
            stamps
                .get(&cata_dbnum)
                .is_some_and(|session| *session < target_sesno)
        })
        .map(|(dbnum, _)| *dbnum)
        .collect()
}

/// 库级目录落款按监听限定分成域内 / 域外两份（纯函数，GitHub #67）。
///
/// 目录版本核对只为**域内**的库点名 → 收窗 → 反查 → 排工：本进程的模型工作单只处理 `watch_dbnums`
/// 里的库（[`crate::data_interface::watch_scope`]），域外的库排了也没人跑，每拍为它收窗、点查上千次再报成
/// 「已发布的根落在旧目录上」就是把一个永远不会动的库报成待办。域外的那份只走
/// [`libraries_behind_any_catalogue`] 点个名。没限定时 `admits` 恒真，域外为空，行为与此前逐位相同。
pub(crate) fn split_libraries_by_watch_scope(
    libraries: BTreeMap<u32, BTreeMap<u32, u32>>,
    admits: impl Fn(u32) -> bool,
) -> (
    BTreeMap<u32, BTreeMap<u32, u32>>,
    BTreeMap<u32, BTreeMap<u32, u32>>,
) {
    libraries.into_iter().partition(|(dbnum, _)| admits(*dbnum))
}

/// 落款落在旧目录上的设计库（纯函数，GitHub #67）：任一目录落款 `session < 文件最新` 即算，升序。
/// `latest(设计库, 目录库)` 是该设计库生成真读的那份目录文件的最新会话（GitHub #47 的选法）；
/// `None`（读不到）是**未知**，不算落后——与 [`stale_catalogue_dbnums`] 同一口径。
pub(crate) fn libraries_behind_any_catalogue(
    libraries: &BTreeMap<u32, BTreeMap<u32, u32>>,
    mut latest: impl FnMut(u32, u32) -> Option<u32>,
) -> Vec<u32> {
    libraries
        .iter()
        .filter(|(library, stamps)| {
            stamps.iter().any(|(&catalogue, &session)| {
                latest(**library, catalogue).is_some_and(|now| now > session)
            })
        })
        .map(|(&library, _)| library)
        .collect()
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
    /// 同上的库号——按目录文件分组核对（GitHub #47）之后几组并起来时去重用，不上回执。
    unreadable_dbnums: BTreeSet<u32>,
    /// 每个过期库一条：`记在案的最旧会话 → 文件最新` 这一窗变了哪些目录件、该库有没有
    /// 反向引用表可供反查。收不了的窗口也在，带 `error`。
    pub(crate) windows: Vec<crate::data_interface::cata_cascade_window::CatalogueWindowReport>,
    /// P3：对上面那些窗口做的反查与排工。`None` = 没有过期库，没跑。
    pub(crate) cascade: Option<crate::data_interface::reverse_reference::CatalogueCascadeReport>,
    /// 监听限定（`watch_dbnums`）之外、库级目录落款却落在旧目录上的设计库，升序（GitHub #67）。
    /// 本进程不为它们收窗 / 反查 / 排工，`stale` 里也没有它们的目录库——这一格只把事实说出来，免得
    /// 「排 RegenRoot 0 根」被当成「没有库落后」。没限定时恒空。
    pub(crate) behind_out_of_scope: Vec<u32>,
    /// A3（2026-09-08）：数据面那一半——已入库依赖行（`pe.dependency = true` 与它们的 `ref_rev`）
    /// 随目录前移的账。每次核对都跑，候选是全部有 `cata_dependency_state` 行的目录库，
    /// 与上面模型面的「过期」名单无关。
    pub(crate) dependency_refresh:
        crate::data_interface::cata_dependency::CatalogueDependencyRefreshReport,
}

impl CatalogueReconcile {
    /// 按目录文件分组核对（GitHub #47）之后把一组的结果并进来：过期库号与读不到的库号取并集，
    /// 窗口挨个接上（同一个库号读两份文件就有两窗），反查回执相加。`dependency_refresh` 不在组里，不动。
    fn absorb(&mut self, other: CatalogueReconcile) {
        self.stale.extend(other.stale);
        self.stale.sort_unstable();
        self.stale.dedup();
        self.unreadable_dbnums.extend(other.unreadable_dbnums);
        self.unreadable = self.unreadable_dbnums.len();
        self.windows.extend(other.windows);
        self.cascade = match (self.cascade.take(), other.cascade) {
            (Some(mut mine), Some(theirs)) => {
                mine.absorb(theirs);
                Some(mine)
            }
            (mine, theirs) => mine.or(theirs),
        };
        self.behind_out_of_scope.extend(other.behind_out_of_scope);
        self.behind_out_of_scope.sort_unstable();
        self.behind_out_of_scope.dedup();
    }
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
/// （external / rocksdb，需显式配置）的文件事件走的是数据面 sweep，目录库在那条路上
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
    // 目录核对的排工串行（防两次核对把同一批根排两遍）；数据面依赖刷新与模型面
    // 互不依赖、且自带远端窗口读取——它走自己的串行闸，不占 CATALOGUE_RECONCILE_SERIAL。
    let mut reconcile = {
        let _serial = CATALOGUE_RECONCILE_SERIAL.lock().await;
        reconcile_published_root_stamps().await
    };
    let project = aios_core::get_db_option().project_name.clone();
    let _dep_serial = DEPENDENCY_REFRESH_SERIAL.lock().await;
    reconcile.dependency_refresh =
        crate::data_interface::cata_dependency::refresh_dependency_rows_of_moved_catalogues(
            &project,
        )
        .await;
    reconcile
}

/// 模型面那一半：点名 → 收窗 → 反查排工（三步见 [`reconcile_catalogue_versions`] 的文档）。
/// 只在串行闸里被调；`dependency_refresh` 留空，由调用方补数据面那一半。
///
/// **按目录文件分组**（GitHub #47）：库级落款记的是生成**真读的那份**目录文件的会话。同一个 CATA 库号在
/// 闭包里有几份文件时（AMS `/ALL` 实测：ACP 的 8 个设计库读 `acp7000`、AMS 的 20 个读 `ams7355`，MDB 声明的却是
/// `ams7000` / `aps7355`），拿 MDB 声明那份去比，比的是两份文件的会话号——判过期、收窗、盖落款全落在别的文件上。
/// 所以先按生成的选法（[`crate::fast_model::e3d_model_service::CatalogueFileChoice`]）把设计库分成「目录文件
/// 宇宙」相同的几组（[`group_libraries_by_catalogue_files`]），每组在自己读的那几份文件上走一遍三步，结果合并。
/// 全按 MDB 声明的库（绝大多数）走的仍是原来那一份。
async fn reconcile_published_root_stamps() -> CatalogueReconcile {
    let libraries = match library_catalogue_stamps_on(&SUL_DB).await {
        Ok(libraries) => libraries,
        Err(error) => {
            println!(
                "目录版本核对失败：读不到已发布根的目录落款，本次说不出目录有没有前移\
                 ——是未知，不是「没变」: {error:#}"
            );
            return CatalogueReconcile::default();
        }
    };
    // 监听限定（GitHub #67）：域外的库本进程不排，它们的落款不进点名 → 收窗 → 反查 → 排工——否则每一拍都为一个
    // 永远不会排的库收窗、点查上千次，还把它报成「已发布的根落在旧目录上」而一字不提它被限定挡着。
    // 域外的库只做最便宜的那一步（读它真读的那几份目录文件的头），落后的记进 `behind_out_of_scope`。
    let (libraries, outside) =
        split_libraries_by_watch_scope(libraries, crate::data_interface::watch_scope::admits);
    let recorded = oldest_catalogue_sessions(&libraries);
    if recorded.is_empty() && outside.is_empty() {
        return CatalogueReconcile::default();
    }
    // 选法读不到（MDB 成员名单读挂）：每个记在案的目录库这次都是未知——与此前逐库 `latest_source_version`
    // 全部失败时同一个结论。
    let unknown = |error: String| {
        println!(
            "目录版本核对失败：读不到当前 MDB 的目录文件选法，{} 个记在案的 CATA 库这次都是未知: {error}",
            recorded.len()
        );
        CatalogueReconcile {
            unreadable: recorded.len(),
            unreadable_dbnums: recorded.keys().copied().collect(),
            ..CatalogueReconcile::default()
        }
    };
    let choice = match tokio::task::spawn_blocking(
        crate::fast_model::e3d_model_service::CatalogueFileChoice::current,
    )
    .await
    {
        Ok(Ok(choice)) => Arc::new(choice),
        Ok(Err(error)) => return unknown(format!("{error:#}")),
        Err(error) => return unknown(format!("blocking task failed: {error}")),
    };
    let groups = group_libraries_by_catalogue_files(
        &libraries,
        |library, catalogue| choice.for_library(library, catalogue),
        |catalogue| choice.declared(catalogue),
    );
    let mut merged = CatalogueReconcile::default();
    for (overrides, members) in groups {
        let part = reconcile_catalogue_group(choice.clone(), overrides, members).await;
        merged.absorb(part);
    }
    merged.behind_out_of_scope = libraries_behind_outside_scope(&choice, &outside).await;
    merged
}

/// 域外（`watch_dbnums` 之外）的设计库里落款落在旧目录上的那些（GitHub #67）：只读它们生成真读的那几份
/// 目录文件的头（GitHub #47 的选法，同一份文件只读一次），不收窗、不反查、不排工。读不到最新的目录库是未知，
/// 不算落后。文件 I/O，阻塞任务里做。
async fn libraries_behind_outside_scope(
    choice: &Arc<crate::fast_model::e3d_model_service::CatalogueFileChoice>,
    outside: &BTreeMap<u32, BTreeMap<u32, u32>>,
) -> Vec<u32> {
    if outside.is_empty() {
        return Vec::new();
    }
    let files: BTreeMap<(u32, u32), PathBuf> = outside
        .iter()
        .flat_map(|(&library, stamps)| {
            let choice = choice.clone();
            stamps.keys().filter_map(move |&catalogue| {
                let file = choice
                    .for_library(library, catalogue)
                    .or_else(|| choice.declared(catalogue))?;
                Some(((library, catalogue), file))
            })
        })
        .collect();
    let outside = outside.clone();
    tokio::task::spawn_blocking(move || {
        let mut latest_by_file: BTreeMap<(u32, PathBuf), Option<u32>> = BTreeMap::new();
        libraries_behind_any_catalogue(&outside, |library, catalogue| {
            let file = files.get(&(library, catalogue))?;
            *latest_by_file
                .entry((catalogue, file.clone()))
                .or_insert_with(|| {
                    crate::data_interface::model_source::latest_source_version_of_file(
                        catalogue, "CATA", file,
                    )
                    .ok()
                    .map(|version| version.sesno)
                })
        })
    })
    .await
    .unwrap_or_default()
}

/// 一组目录文件宇宙相同的设计库走一遍点名 → 收窗 → 反查排工（GitHub #47 之前这就是整个核对）。
/// 目录库读哪份文件：覆盖表里有就是那份副本，没有就是 MDB 声明那份；落款、窗口、排工都只在这组库里。
async fn reconcile_catalogue_group(
    choice: Arc<crate::fast_model::e3d_model_service::CatalogueFileChoice>,
    overrides: CatalogueFileOverrides,
    members: BTreeMap<u32, BTreeMap<u32, u32>>,
) -> CatalogueReconcile {
    let recorded = oldest_catalogue_sessions(&members);
    if recorded.is_empty() {
        return CatalogueReconcile::default();
    }
    let files: BTreeMap<u32, Option<PathBuf>> = recorded
        .keys()
        .map(|&dbnum| {
            let file = overrides
                .get(&dbnum)
                .cloned()
                .or_else(|| choice.declared(dbnum));
            (dbnum, file)
        })
        .collect();
    let latest = {
        let files = files.clone();
        tokio::task::spawn_blocking(move || {
            files
                .into_iter()
                .map(|(dbnum, file)| {
                    let sesno = file.and_then(|file| {
                        crate::data_interface::model_source::latest_source_version_of_file(
                            dbnum, "CATA", &file,
                        )
                        .ok()
                        .map(|version| version.sesno)
                    });
                    (dbnum, sesno)
                })
                .collect::<BTreeMap<u32, Option<u32>>>()
        })
        .await
        .unwrap_or_default()
    };
    let (stale, unreadable) =
        stale_catalogue_dbnums(&recorded, |dbnum| latest.get(&dbnum).copied().flatten());
    let unreadable_dbnums: BTreeSet<u32> = recorded
        .keys()
        .copied()
        .filter(|dbnum| latest.get(dbnum).copied().flatten().is_none())
        .collect();
    debug_assert_eq!(unreadable, unreadable_dbnums.len());
    // 只在这组真有过期库时出声：周期复核 300 s 一拍，没事的那一拍一个字不打。
    if !overrides.is_empty() && !stale.is_empty() {
        println!(
            "目录版本核对：设计库 {:?} 读的是目录库副本 {}——这一组按副本比会话、收窗、盖落款（GitHub #47）",
            members.keys().collect::<Vec<_>>(),
            overrides
                .iter()
                .map(|(dbnum, file)| format!("{dbnum}={}", file.display()))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    // 每个过期库收一窗：S = 记在案的最旧会话，T = 这组读的那份文件最新。只读文件，阻塞任务里做。
    let spans: Vec<(u32, u32, u32, Option<PathBuf>)> = stale
        .iter()
        .filter_map(|&dbnum| {
            let base = *recorded.get(&dbnum)?;
            let target = latest.get(&dbnum).copied().flatten()?;
            Some((dbnum, base, target, files.get(&dbnum).cloned().flatten()))
        })
        .collect();
    let collected = tokio::task::spawn_blocking(move || collect_stale_catalogue_windows(&spans))
        .await
        .unwrap_or_default();
    // P3：反查 + 排工。窗口本体在这里用掉，回执留给 `/health`。
    let cascade = cascade_stale_catalogue_windows(&collected, &overrides, &members).await;
    let windows = collected.into_iter().map(|item| item.report).collect();
    CatalogueReconcile {
        stale,
        unreadable,
        unreadable_dbnums,
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

/// 数据面依赖刷新自己的串行闸：它与模型面排工互不依赖，但两个刷新彼此不能并跑
/// （同一窗口两次 Replace/推进 `cata_dependency_state` 是浪费）。从
/// [`CATALOGUE_RECONCILE_SERIAL`] 里拆出——它带的远端窗口读取不该压住排工闸。
static DEPENDENCY_REFRESH_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 合并后的下一轮核对请求：保留最近触发上下文与前移库号并集，避免只用布尔位丢失
/// `trigger` / `advanced` 元信息。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReconcileRequest {
    origin: String,
    trigger: &'static str,
    advanced: Vec<u32>,
}

impl ReconcileRequest {
    fn merge(&mut self, incoming: ReconcileRequest) {
        let mut merged: BTreeSet<u32> = self.advanced.iter().copied().collect();
        merged.extend(incoming.advanced);
        self.advanced = merged.into_iter().collect();
        // 真实目录文件前移比周期复核更具体：两轮合并时优先保留前移触发的嗓音与点名，
        // 避免 periodic 把新前移静音掉。
        if self.trigger == "periodic" || incoming.trigger == "catalogue_file_advanced" {
            self.origin = incoming.origin;
            self.trigger = incoming.trigger;
        }
    }
}

/// 另起任务的目录核对单飞状态（以同步互斥保护，避免双原子位在「清 running 与复检
/// pending」之间的窗口丢唤醒或并发起两轮任务）。
#[derive(Default)]
struct ReconcileCoordinator {
    running: bool,
    pending: Option<ReconcileRequest>,
}

static RECONCILE_COORDINATOR: Mutex<ReconcileCoordinator> = Mutex::new(ReconcileCoordinator {
    running: false,
    pending: None,
});

struct ReconcileRunningGuard<'a> {
    coordinator: &'a Mutex<ReconcileCoordinator>,
    active: bool,
}

impl Drop for ReconcileRunningGuard<'_> {
    fn drop(&mut self) {
        if self.active {
            let mut state = self
                .coordinator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.running = false;
        }
    }
}

/// sweep / 事件快道的核对触发：至多一轮在飞，在飞期间的触发合并成「下一轮一起核」。
/// 返回 true = 本轮由调用方这边新起了一轮（调用方记日志用），false = 已合并进
/// 在飞轮的 pending。
fn request_reconcile_run(origin: &str, trigger: &'static str, advanced: &[u32]) -> bool {
    request_reconcile_run_with(
        &RECONCILE_COORDINATOR,
        origin,
        trigger,
        advanced,
        |origin, trigger, advanced| async move {
            reconcile_catalogue_versions_now_with(&origin, trigger, &advanced).await;
        },
    )
}

fn request_reconcile_run_with<F, Fut>(
    coordinator: &'static Mutex<ReconcileCoordinator>,
    origin: &str,
    trigger: &'static str,
    advanced: &[u32],
    runner: F,
) -> bool
where
    F: Fn(String, &'static str, Vec<u32>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let req = ReconcileRequest {
        origin: origin.to_string(),
        trigger,
        advanced: advanced.to_vec(),
    };
    let mut current = {
        let mut state = coordinator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.running {
            match &mut state.pending {
                Some(pending) => pending.merge(req),
                None => state.pending = Some(req),
            }
            return false;
        }
        state.running = true;
        req
    };
    tokio::spawn(async move {
        let mut guard = ReconcileRunningGuard {
            coordinator,
            active: true,
        };
        loop {
            runner(current.origin, current.trigger, current.advanced).await;
            let next = {
                let mut state = coordinator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match state.pending.take() {
                    Some(next) => Some(next),
                    None => {
                        state.running = false;
                        guard.active = false;
                        None
                    }
                }
            };
            match next {
                Some(next) => current = next,
                None => break,
            }
        }
    });
    true
}

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
    /// 监听限定（`watch_dbnums`）之外、库级目录落款却落在旧目录上的设计库，升序（GitHub #67）。本进程
    /// 不为它们收窗 / 反查 / 排工，`stale_catalogue_dbnums` 里也没有它们的目录库；要重算得把库加进限定域或
    /// 在不限定的进程里跑。没限定时恒空。
    pub behind_out_of_scope_libraries: Vec<u32>,
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
            behind_out_of_scope,
            dependency_refresh,
            ..
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
            behind_out_of_scope_libraries: behind_out_of_scope,
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
        // 域外落后的库（GitHub #67）：周期拍只在本来就要出声时附上一句，安静的那一拍照旧不打；
        // 文件前移 / 人工触发总说——这一句就是「为什么这个库没被排」的答案。
        if !self.behind_out_of_scope_libraries.is_empty()
            && (self.trigger != "periodic" || !self.stale_catalogue_dbnums.is_empty())
        {
            lines.push(format!(
                "[{origin}] {}",
                behind_out_of_scope_line(&self.behind_out_of_scope_libraries)
            ));
        }
        if let Some(line) = self.catalogue_dependency_refresh.banner_line() {
            lines.push(format!("[{origin}] {line}"));
        }
        lines
    }
}

/// 域外落后的库那一句（GitHub #67），启动横幅与运行期横幅共用。必点名限定是 `watch_dbnums` /
/// `--watch-dbnum` 划的（[`crate::data_interface::watch_scope`] 的纪律：说不清是谁跳的就是 issue #10）。
pub(crate) fn behind_out_of_scope_line(libraries: &[u32]) -> String {
    format!(
        "目录版本核对：{} 个设计库的库级目录落款落在旧目录上，但在监听限定（{} / {}）域外，本进程不收窗、\
         不反查、不排工 {libraries:?}——要重算请把它们加进限定域，或在不限定的进程里跑（GitHub #67）",
        libraries.len(),
        crate::data_interface::watch_scope::WATCH_CONFIG_KEY,
        crate::data_interface::watch_scope::WATCH_SWITCH
    )
}

/// 进程内：最近一次运行期目录核对 + sweep 上一轮观察到的目录文件会话。
struct CatalogueReconcileState {
    latest: Option<CatalogueReconcileSnapshot>,
    /// `None` = 还没有任何一轮 sweep 记过观察（进程刚起）。
    observed: Option<ObservedCatalogueFiles>,
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
pub(crate) fn catalogue_files_advanced<K: Ord + Clone>(
    previous: Option<&BTreeMap<K, u32>>,
    observed: &BTreeMap<K, u32>,
) -> Vec<K> {
    observed
        .iter()
        .filter(|(key, now)| {
            previous
                .and_then(|previous| previous.get(*key))
                .is_none_or(|seen| *now > seen)
        })
        .map(|(key, _)| key.clone())
        .collect()
}

/// sweep 一轮观察到的目录库文件：`(库号, 文件) → 最新会话`。按**文件**记（GitHub #47）：同号的项目副本
/// 也在里面——设计库所在项目里有副本时，生成读的就是那份，它前移同样要开一次核对；同一个项目里
/// extract family 的下层也各占一格，彼此不覆盖。
pub type ObservedCatalogueFiles = BTreeMap<(u32, PathBuf), u32>;

/// 这一轮 sweep 要不要跑目录核对（纯函数）：第一轮只记观察；看见前移就跑；`force`
/// （周期对账那一拍）没看见也跑。
pub(crate) fn catalogue_sweep_verdict<K: Ord + Clone>(
    previous: Option<&BTreeMap<K, u32>>,
    observed: &BTreeMap<K, u32>,
    force: bool,
) -> (CatalogueSweepVerdict, Vec<K>) {
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
    observed: ObservedCatalogueFiles,
    force: bool,
) -> CatalogueSweepVerdict {
    let (verdict, advanced) = {
        let mut state = catalogue_reconcile_state();
        let (verdict, advanced) =
            catalogue_sweep_verdict(state.observed.as_ref(), &observed, force);
        state.observed = Some(observed);
        // 回执按库号点名：同一个库号两份文件都前移只记一次。
        let advanced: Vec<u32> = advanced
            .into_iter()
            .map(|(dbnum, _)| dbnum)
            .collect::<BTreeSet<u32>>()
            .into_iter()
            .collect();
        (verdict, advanced)
    };
    if let CatalogueSweepVerdict::Run { trigger } = verdict {
        request_reconcile_run(origin, trigger, &advanced);
    }
    verdict
}

fn observe_catalogue_files_in(
    state: &mut CatalogueReconcileState,
    observed: &ObservedCatalogueFiles,
) -> (CatalogueSweepVerdict, Vec<u32>) {
    let previous = state.observed.clone();
    let merged = state.observed.get_or_insert_with(BTreeMap::new);
    for (key, sesno) in observed {
        merged.insert(key.clone(), *sesno);
    }
    let Some(previous) = previous else {
        return (CatalogueSweepVerdict::Seeded, Vec::new());
    };
    // 只对「这轮快道看到的键」判前移：没扫到的库上一轮是什么还是什么。
    let advanced_keys = catalogue_files_advanced(Some(&previous), observed);
    let verdict = if !advanced_keys.is_empty() {
        CatalogueSweepVerdict::Run {
            trigger: "catalogue_file_advanced",
        }
    } else {
        CatalogueSweepVerdict::Unchanged
    };
    let advanced = advanced_keys
        .iter()
        .map(|(dbnum, _)| *dbnum)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    (verdict, advanced)
}

/// 事件快道的目录观察（[`sweep_changed_files` 的快道路径]）：只并入变了的那几份
/// 目录文件——按 (库号,文件) 键覆盖对应观察，没扫到的库的观察原样保留（与整面
/// 重扫的「整体覆写」相对：子集没看其余目录，没资格替它们表态）。
///
/// 前移判定与整扫同一谓词：并入后新值高于旧值、或键是新出现的 → 触发一次目录
/// 版本核对（经 [`request_reconcile_run`] 与 sweep 共用同一条合并通道）。第一轮
/// 观察表还没有时只播种不触发（启动那次核对另有人负责）。
pub fn observe_catalogue_files_partial(
    origin: &str,
    observed: &ObservedCatalogueFiles,
) -> CatalogueSweepVerdict {
    let (verdict, advanced) = {
        let mut state = catalogue_reconcile_state();
        observe_catalogue_files_in(&mut state, observed)
    };
    if let CatalogueSweepVerdict::Run { trigger } = verdict {
        request_reconcile_run(origin, trigger, &advanced);
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

    // GitHub #67：限定域外落后的库——安静的周期拍照旧不打；人工 / 前移触发或域内真有过期库时附上一句，
    // 点名是 `watch_dbnums` 划的；`/health` 上那一格不管哪种触发都在。
    let outside_only = |trigger: &'static str| {
        CatalogueReconcileSnapshot::from_reconcile(
            "reconcile",
            trigger,
            &[],
            std::time::Duration::from_millis(3),
            CatalogueReconcile {
                behind_out_of_scope: vec![7997],
                ..CatalogueReconcile::default()
            },
        )
    };
    let quiet_outside = outside_only("periodic");
    assert!(quiet_outside.banner_lines().is_empty());
    assert_eq!(quiet_outside.behind_out_of_scope_libraries, vec![7997]);
    let json = serde_json::to_value(&quiet_outside).expect("serialises");
    assert_eq!(
        json["behind_out_of_scope_libraries"],
        serde_json::json!([7997])
    );
    let lines = outside_only("requested").banner_lines();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("没有落在旧目录上"), "{lines:?}");
    assert!(
        lines[1].contains("[reconcile]")
            && lines[1].contains("[7997]")
            && lines[1].contains("watch_dbnums")
            && lines[1].contains("不排工"),
        "{lines:?}"
    );
    let both = CatalogueReconcileSnapshot::from_reconcile(
        "reconcile",
        "periodic",
        &[],
        std::time::Duration::from_millis(3),
        CatalogueReconcile {
            stale: vec![7000],
            behind_out_of_scope: vec![7997],
            ..CatalogueReconcile::default()
        },
    );
    let lines = both.banner_lines();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("1 个 CATA 库已前移"), "{lines:?}");
    assert!(lines[1].contains("[7997]"), "{lines:?}");
}

/// 逐个过期目录库收窗；收不了的记 `error`，**不**让一个库的失败挡住别的库。
///
/// 每窗收在调用方给的那份文件上（GitHub #47：这组设计库生成真读的那份，不一定是 MDB 声明那份）；
/// `None` = 当前 MDB 里没有这个库的文件。
fn collect_stale_catalogue_windows(
    spans: &[(u32, u32, u32, Option<PathBuf>)],
) -> Vec<StaleCatalogueWindow> {
    use crate::data_interface::cata_cascade_window::{
        CatalogueWindowReport, collect_catalogue_window_bounded,
    };
    spans
        .iter()
        .map(|(dbnum, base, target, file)| {
            let (dbnum, base, target) = (*dbnum, *base, *target);
            let failed = |error: String| StaleCatalogueWindow {
                report: CatalogueWindowReport::failed(dbnum, base, target, error),
                window: None,
            };
            let Some(file) = file else {
                return failed(format!("当前 MDB 没有 dbnum {dbnum} 的库文件"));
            };
            match collect_catalogue_window_bounded(
                dbnum,
                file,
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
/// 上卷）。排工只对**库级落款仍在旧目录上**的设计库（ADR-079）：落款已到新目录的库不重排
/// （`already_current`），限定域外 / 被发布队列阻断的库跳过。退化目录库（窗口级的 + 反查自己判的）
/// 波及的库把全部根重排；落款仍旧却没被命中的库只排一条盖落款的空任务（`stamp_only`）。
///
/// 任何一步没跑成都不中止启动：回执带 `error`、一根不排，受影响的根只剩按需 `ensure` 的目录
/// 指纹失配兜底——这与 P3 之前的行为逐位相同，只是现在会说出来。
async fn cascade_stale_catalogue_windows(
    collected: &[StaleCatalogueWindow],
    overrides: &CatalogueFileOverrides,
    members: &BTreeMap<u32, BTreeMap<u32, u32>>,
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
    let catalogue_files = overrides.clone();
    let mut cascade = match tokio::task::spawn_blocking(move || {
        run_catalogue_cascade(&service, &pinned, &seeds, &catalogue_files)
    })
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
    // 设计侧引用者在 MDB 每个设计库里找；这一组只认组里的库（GitHub #47），别组的库在别组那一遍里各算各的。
    cascade.roots.retain(|dbnum, _| members.contains_key(dbnum));
    cascade
        .dependent_referrers
        .retain(|dbnum, _| members.contains_key(dbnum));
    cascade
        .design_referrers
        .retain(|dbnum, _| members.contains_key(dbnum));
    cascade
        .unrooted
        .retain(|dbnum, _| members.contains_key(dbnum));
    let mut report = CatalogueCascadeReport::from_cascade(&cascade);
    for (dbnum, reason) in window_degraded {
        report.degraded.entry(dbnum).or_insert(reason);
    }
    match enqueue_catalogue_cascade(
        &cascade,
        &report.degraded.keys().copied().collect(),
        collected,
        members,
    )
    .await
    {
        Ok(account) => {
            report.enqueued = account.enqueued;
            report.enqueued_degraded = account.enqueued_degraded;
            report.already_current = account.already_current;
            report.stamp_only = account.stamp_only;
            report.skipped_out_of_scope = account.skipped_out_of_scope;
            report.libraries_out_of_scope = account.libraries_out_of_scope;
        }
        Err(error) => {
            report.error = Some(format!("enqueue the reverse cascade roots: {error:#}"));
        }
    }
    Some(report)
}

/// 反查本体（阻塞）：MDB 成员 → [`crate::data_interface::reverse_reference::SourceDb`]，
/// 上卷用一份 `DbSet@最新`（设计库都在里面，目录经 locator 解）。
///
/// 目录库读哪份文件随这一组（GitHub #47）：`catalogue_files` 里有的读那份副本，其余读 MDB 声明那份。
fn run_catalogue_cascade(
    service: &crate::fast_model::e3d_model_service::E3dModelService,
    pinned: &BTreeMap<u32, u32>,
    seeds: &[crate::data_interface::reverse_reference::CatalogueSeed],
    catalogue_files: &CatalogueFileOverrides,
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
        .map(|pin| {
            let dbnum = pin.dbnum as u32;
            let file = if pin.db_type.eq_ignore_ascii_case("CATA") {
                catalogue_files.get(&dbnum).cloned().unwrap_or(pin.file)
            } else {
                pin.file
            };
            SourceDb {
                dbnum,
                sesno: pinned.get(&dbnum).copied(),
                db_type: pin.db_type,
                file,
            }
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
    stamp_only: Vec<u32>,
    skipped_out_of_scope: usize,
    libraries_out_of_scope: Vec<u32>,
}

/// 把反查命中的根与退化库排成**带明确根名单与库级目录落款的 ModelOnly 任务**（每个设计库一条）。
///
/// 「落款仍在旧目录上」是**库级**判据（ADR-079：随 `gen_root` 退役，逐根落款收成每库一份
/// `dbnum_watermark.model_catalogue_sessions`）：逐个过期目录库 `X → T_X` 问一次
/// [`libraries_behind_catalogue`]，落款里记着 `X` 且 `session < T_X` 的设计库才是「还落在旧目录上」。
///
/// - 命中根所在的库落款仍旧 → 排它（名单随任务）；库落款已到 `T` → `already_current`
///   （重启前 / 上一次核对已按新目录重算过整库，再排是白算）；
/// - 退化的目录库（窗口收不了 / 导入级 / 超阈 / 反查自己判的）→ 落款仍旧的库把 `DbSet@T` 上的
///   **全部**根都排（宁多算不漏算）；
/// - 落款仍旧、却一根都没被命中的库（反查精确、这个库没被这一窗波及）→ 排一条**只带落款**的
///   ModelOnly：名单为空，任务什么都不用算，收尾把落款盖到 `T`——不盖的话下次核对又点名它。
///
/// 每条任务随身带 [`DiscoveredBatch::catalogue_stamps`]（该库要盖到的 `X → T_X`），成了才盖
/// （`batch_worker::task_catalogue_stamps`）。任务的目标会话取设计库文件最新（ADR-054：生成时点是
/// 文件最新），`model_start_sesno` 也取它——规划器于是不开窗，只有名单里的根与账本里还欠着的根进工作单。
///
/// 2026-09-10（ADR-071 S4）之前这里写的是 `model_update_pending` 行等空闲轮 drain 领；现在名单随
/// [`DiscoveredBatch::regen_roots`] 进队列行、随冻结进任务，任务体并进自己的工作单——目录级联与
/// 别的模型工作一样，是一个 dbnum 任务的事，不是第二条队列。
async fn enqueue_catalogue_cascade(
    cascade: &crate::data_interface::reverse_reference::ReverseCascade,
    degraded: &BTreeSet<u32>,
    collected: &[StaleCatalogueWindow],
    libraries: &BTreeMap<u32, BTreeMap<u32, u32>>,
) -> anyhow::Result<CatalogueCascadeEnqueue> {
    let mut account = CatalogueCascadeEnqueue::default();
    // `libraries` = 这一组设计库的库级落款（GitHub #47：按目录文件分组，窗口的 `T_X` 是这组读的那份文件上的）。
    // 设计库 → 它要盖到的落款 `X → T_X`（只收落款仍旧的那几个目录库）；退化目录库另记一份。
    let mut behind: BTreeMap<u32, BTreeMap<u32, u32>> = BTreeMap::new();
    let mut behind_degraded: BTreeSet<u32> = BTreeSet::new();
    for item in collected {
        let (cata, target) = (item.report.dbnum, item.report.target_sesno);
        for dbnum in libraries_behind_catalogue(libraries, cata, target) {
            behind.entry(dbnum).or_default().insert(cata, target);
            if degraded.contains(&cata) {
                behind_degraded.insert(dbnum);
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
    // 1. 反查命中的根：所在库落款仍旧的排；库落款已到 T 的算「已重算过」。
    for (dbnum, roots) in &cascade.roots {
        if !in_scope(*dbnum) {
            account.skipped_out_of_scope += roots.len();
            continue;
        }
        if !behind.contains_key(dbnum) {
            account.already_current += roots.len();
            continue;
        }
        let version = latest_design_version(&mut versions, *dbnum)
            .await
            .with_context(|| format!("dbnum {dbnum} 取不到文件最新会话，命中的根排不了"))?;
        let bucket = &mut planned
            .entry(*dbnum)
            .or_insert_with(|| (version, BTreeMap::new()))
            .1;
        for root in roots {
            if bucket
                .insert(root.refno.clone(), root.noun.clone())
                .is_none()
            {
                *account.enqueued.entry(*dbnum).or_default() += 1;
            }
        }
    }
    // 2. 退化目录库：落款仍旧的库把 `DbSet@T` 上的全部根都排（已在 1 里排过的不重复计）。
    for dbnum in behind_degraded {
        if !in_scope(dbnum) {
            account.skipped_out_of_scope += 1;
            continue;
        }
        let version = latest_design_version(&mut versions, dbnum)
            .await
            .with_context(|| format!("dbnum {dbnum} 取不到文件最新会话，退化库的根排不了"))?;
        let roots =
            crate::data_interface::window_root_plan::enumerate_target_roots(dbnum, version.sesno)
                .await
                .with_context(|| {
                    format!("dbnum {dbnum} 枚举不出 DbSet@{} 的生成根", version.sesno)
                })?;
        let bucket = &mut planned
            .entry(dbnum)
            .or_insert_with(|| (version, BTreeMap::new()))
            .1;
        for root in roots {
            if bucket
                .insert(root.root.to_pdms_str(), root.noun.clone())
                .is_none()
            {
                *account.enqueued_degraded.entry(dbnum).or_default() += 1;
            }
        }
    }
    // 3. 落款仍旧、却一根都没排的库：只为盖落款排一条空名单任务。被监听限定挡掉的库记下来、横幅点名：
    //    静默 `continue` 就是 GitHub #67 的形状（核对已先按限定域分过一次，这里本该恒空，留作护栏）。
    for dbnum in behind.keys().copied() {
        if planned.contains_key(&dbnum) {
            continue;
        }
        if !in_scope(dbnum) {
            account.libraries_out_of_scope.push(dbnum);
            continue;
        }
        let Ok(version) = latest_design_version(&mut versions, dbnum).await else {
            continue;
        };
        planned.insert(dbnum, (version, BTreeMap::new()));
        account.stamp_only.push(dbnum);
    }
    // 4. 每个设计库一条 ModelOnly 任务，名单与落款随行。已有排队行就并进去（名单取并集、落款取
    //    较新），同库任务在跑就接在它后面——都是 `BatchScheduler::enqueue` 的事。
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
            // 下限 = 目标：规划器不开窗（模型水位等于右端即追平），进工作单的只有名单与账本里还欠着的根。
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
            catalogue_stamps: behind.get(&dbnum).cloned().unwrap_or_default(),
        };
        let outcome = crate::data_interface::batch_scheduler::BatchScheduler::global().enqueue(
            crate::data_interface::task_registry::TaskRegistry::global(),
            &found,
            false,
        );
        println!(
            "目录级联已排 ModelOnly 任务 dbnum={dbnum} roots={} stamps={:?} target={latest} outcome={:?} task={}",
            found.regen_roots.len(),
            found.catalogue_stamps,
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
            reconcile.contains("let spans: Vec<(u32, u32, u32, Option<PathBuf>)> = stale"),
            "只对过期库收窗，S/T 各取记在案与这组读的那份文件最新（GitHub #47）: {reconcile}"
        );
        assert!(
            reconcile.contains("group_libraries_by_catalogue_files(")
                && !reconcile.contains("model_source::latest_source_version(dbnum)"),
            "过期判据按设计库真读的那份文件比，不拿 MDB 声明那份比（GitHub #47）: {reconcile}"
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
        assert!(
            cascade_stale_catalogue_windows(&[], &CatalogueFileOverrides::new(), &BTreeMap::new())
                .await
                .is_none()
        );
    }

    /// 「落款仍在旧目录上」的过滤器（ADR-079：库级）真跑一遍：水位行上 `model_catalogue_sessions`
    /// 记着 `dbnum = X 且 session < T` 的设计库进名单；落款已到 T 的、记的是别的目录库的、没有落款的都不进。
    /// GitHub #47：设计库按「生成真读的目录文件」分组——AMS `/ALL` 的形状：ACP 的库读 `acp7000`、AMS 的库读
    /// `ams7355`，MDB 声明的是 `ams7000` / `aps7355`；全按声明读的库（SCB）自成一组、覆盖表为空。
    /// 路径不分大小写比；生成解不出的目录库不进覆盖表（核对里照旧落「读不到」）。
    #[test]
    fn libraries_group_by_the_catalogue_files_their_generation_reads() {
        let file = |name: &str| PathBuf::from(format!(r"D:\proj\{name}"));
        let libraries = BTreeMap::from([
            (7015, BTreeMap::from([(7000, 272), (7355, 15)])),
            (7022, BTreeMap::from([(7000, 272)])),
            (8000, BTreeMap::from([(7000, 274), (7355, 15)])),
            (6003, BTreeMap::from([(7000, 274), (7016, 3)])),
        ]);
        let declared = |catalogue: u32| match catalogue {
            7000 => Some(file("ams7000_0001")),
            7355 => Some(file("aps7355_0001")),
            _ => None,
        };
        let file_for = |library: u32, catalogue: u32| match (library, catalogue) {
            (7015 | 7022, 7000) => Some(file("acp7000_0001")),
            (8000, 7355) => Some(file("ams7355_0001")),
            // 同一份文件、大小写不同：不算副本。
            (6003, 7000) => Some(PathBuf::from(r"d:\PROJ\AMS7000_0001")),
            (_, 7016) => None,
            (_, catalogue) => declared(catalogue),
        };
        let groups = group_libraries_by_catalogue_files(&libraries, file_for, declared);
        let shape: Vec<(Vec<(u32, PathBuf)>, Vec<u32>)> = groups
            .iter()
            .map(|(overrides, members)| {
                (
                    overrides
                        .iter()
                        .map(|(dbnum, path)| (*dbnum, path.clone()))
                        .collect(),
                    members.keys().copied().collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (vec![], vec![6003]),
                (vec![(7000, file("acp7000_0001"))], vec![7015, 7022]),
                (vec![(7355, file("ams7355_0001"))], vec![8000]),
            ]
        );
        // 分组之后每组各取最旧：ACP 那组的 7000 是 272、不再把 AMS / SCB 的 274 拖成「落在旧目录上」。
        assert_eq!(
            oldest_catalogue_sessions(&groups[&CatalogueFileOverrides::new()]),
            BTreeMap::from([(7000, 274), (7016, 3)])
        );
        let acp = CatalogueFileOverrides::from([(7000, file("acp7000_0001"))]);
        assert_eq!(
            oldest_catalogue_sessions(&groups[&acp]),
            BTreeMap::from([(7000, 272), (7355, 15)])
        );
    }

    /// GitHub #47：几组的核对结果并成一份——过期库号取并集、读不到的库号去重计数、窗口挨个接上、反查回执相加。
    #[test]
    fn grouped_reconcile_parts_merge_into_one_report() {
        use crate::data_interface::cata_cascade_window::CatalogueWindowReport;
        use crate::data_interface::reverse_reference::CatalogueCascadeReport;
        let mut merged = CatalogueReconcile::default();
        merged.absorb(CatalogueReconcile {
            stale: vec![7000],
            unreadable: 1,
            unreadable_dbnums: BTreeSet::from([7016]),
            windows: vec![CatalogueWindowReport::failed(7000, 272, 274, "a".into())],
            cascade: Some(CatalogueCascadeReport {
                seeds: 2,
                enqueued: BTreeMap::from([(7015, 3)]),
                stamp_only: vec![7022],
                ..CatalogueCascadeReport::default()
            }),
            behind_out_of_scope: vec![7997],
            ..CatalogueReconcile::default()
        });
        merged.absorb(CatalogueReconcile {
            stale: vec![7355, 7000],
            unreadable: 1,
            unreadable_dbnums: BTreeSet::from([7016]),
            windows: vec![CatalogueWindowReport::failed(7355, 14, 15, "b".into())],
            cascade: Some(CatalogueCascadeReport {
                seeds: 1,
                enqueued: BTreeMap::from([(8000, 4), (7015, 1)]),
                error: Some("x".into()),
                ..CatalogueCascadeReport::default()
            }),
            behind_out_of_scope: vec![7997, 7333],
            ..CatalogueReconcile::default()
        });
        assert_eq!(merged.stale, vec![7000, 7355]);
        assert_eq!(merged.behind_out_of_scope, vec![7333, 7997]);
        assert_eq!(merged.unreadable, 1, "两组都读不到 7016，只算一个库");
        assert_eq!(merged.windows.len(), 2);
        let cascade = merged.cascade.expect("cascade merged");
        assert_eq!(cascade.seeds, 3);
        assert_eq!(cascade.enqueued, BTreeMap::from([(7015, 4), (8000, 4)]));
        assert_eq!(cascade.stamp_only, vec![7022]);
        assert_eq!(cascade.error.as_deref(), Some("x"));
    }

    /// GitHub #67：库级目录落款先按监听限定分成域内 / 域外；域外的只点名落后的那几个（任一落款 < 该库真读的
    /// 那份文件最新），读不到最新的目录库是未知、不算落后，回退也不算；没限定时域外为空。
    #[test]
    fn catalogue_stamps_split_by_watch_scope_and_only_behind_outsiders_are_named() {
        let libraries = BTreeMap::from([
            (7997, BTreeMap::from([(5052, 189), (7000, 274)])),
            (8000, BTreeMap::from([(7000, 274)])),
            (7333, BTreeMap::from([(7320, 306)])),
            (250705, BTreeMap::from([(250700, 23)])),
        ]);
        let (inside, outside) =
            split_libraries_by_watch_scope(libraries.clone(), |dbnum| matches!(dbnum, 7998 | 8000));
        assert_eq!(inside.keys().copied().collect::<Vec<_>>(), vec![8000]);
        assert_eq!(
            outside.keys().copied().collect::<Vec<_>>(),
            vec![7333, 7997, 250705]
        );
        let (all, none) = split_libraries_by_watch_scope(libraries.clone(), |_| true);
        assert_eq!(all, libraries);
        assert!(none.is_empty());
        // 7997 的 5052 落款 189 < 文件 190 → 落后；7333 追平；250705 的目录读不到 → 未知。
        let latest = |library: u32, catalogue: u32| match (library, catalogue) {
            (7997, 5052) => Some(190),
            (7997, 7000) => Some(274),
            (7333, 7320) => Some(306),
            (250705, 250700) => None,
            _ => unreachable!("{library}/{catalogue} 不在域外名单上"),
        };
        assert_eq!(libraries_behind_any_catalogue(&outside, latest), vec![7997]);
        let rolled_back = BTreeMap::from([(7997, BTreeMap::from([(5052, 189)]))]);
        assert!(libraries_behind_any_catalogue(&rolled_back, |_, _| Some(188)).is_empty());
    }

    /// GitHub #67：核对先按监听限定分域再按目录文件分组，域外的库不进收窗 / 反查 / 排工；排工第 3 步不再
    /// 静默跳过被限定挡掉的库；启动复核两个出口都带 `catalogue_behind_out_of_scope`。
    #[test]
    fn the_catalogue_reconcile_scopes_stamps_before_grouping_and_never_skips_silently() {
        let source = include_str!("model_coverage.rs").replace("\r\n", "\n");
        let driver = source
            .split_once("async fn reconcile_published_root_stamps()")
            .expect("driver exists")
            .1;
        let split = driver
            .find("split_libraries_by_watch_scope(libraries,")
            .expect("域内 / 域外先分开");
        let grouped = driver
            .find("group_libraries_by_catalogue_files(")
            .expect("再按目录文件分组");
        assert!(split < grouped, "分域要在分组之前，域外的库一窗都不收");
        let enqueue = source
            .split_once("async fn enqueue_catalogue_cascade(")
            .expect("enqueue exists")
            .1
            .split_once("async fn latest_design_version(")
            .expect("helper follows")
            .0;
        assert!(
            !enqueue.contains("|| !in_scope(dbnum)")
                && enqueue.contains("account.libraries_out_of_scope.push(dbnum);"),
            "被限定挡掉的库要记账、横幅点名，不许静默 continue"
        );
        let startup = include_str!("startup_reconcile.rs").replace("\r\n", "\n");
        let inner = startup
            .split_once("async fn reconcile_model_coverage_at_startup_inner()")
            .expect("inner exists")
            .1;
        assert!(
            inner.matches("catalogue_behind_out_of_scope").count() >= 4,
            "解构、启动横幅、两个出口都要带上它"
        );
    }

    /// 同一行上一个目录库只留最旧的会话；`published_catalogue_sessions_on` 再跨库取最旧。
    #[tokio::test]
    async fn libraries_behind_a_catalogue_are_those_still_stamped_before_its_target() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("libraries_behind_catalogue")
            .await
            .expect("select fixture db");
        db.query(
            "CREATE dbnum_watermark:8000 SET dbnum = 8000, model_sesno = 26, model_catalogue_sessions = [ \
               { dbnum: 7000, session: 184 }, { dbnum: 7001, session: 3 } ];\
             CREATE dbnum_watermark:8001 SET dbnum = 8001, model_sesno = 30, model_catalogue_sessions = [ \
               { dbnum: 7000, session: 100 }, { dbnum: 7001, session: 1 } ];\
             CREATE dbnum_watermark:8002 SET dbnum = 8002, model_sesno = 12, model_catalogue_sessions = [ \
               { dbnum: 7000, session: 185 } ];\
             CREATE dbnum_watermark:8003 SET dbnum = 8003, model_sesno = 5;\
             CREATE dbnum_watermark:8004 SET dbnum = 8004, model_sesno = 5, model_catalogue_sessions = [];",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed statements");

        let libraries = library_catalogue_stamps_on(&db).await.expect("read stamps");
        assert_eq!(
            libraries.keys().copied().collect::<Vec<_>>(),
            vec![8000, 8001, 8002],
            "没有落款的库不在里面"
        );
        assert_eq!(libraries[&8000], BTreeMap::from([(7000, 184), (7001, 3)]));
        assert_eq!(
            oldest_catalogue_sessions(&libraries),
            BTreeMap::from([(7000, 100), (7001, 1)]),
            "跨库取最旧"
        );
        assert_eq!(
            published_catalogue_sessions_on(&db).await.expect("oldest"),
            BTreeMap::from([(7000, 100), (7001, 1)])
        );
        assert_eq!(
            libraries_behind_catalogue(&libraries, 7000, 185),
            BTreeSet::from([8000, 8001]),
            "8002 已落在 185 上、8003 / 8004 没有落款——都不在名单里"
        );
        assert!(
            libraries_behind_catalogue(&libraries, 7000, 100).is_empty(),
            "T 不高于任何落款时名单为空"
        );
        assert_eq!(
            libraries_behind_catalogue(&libraries, 7001, 4),
            BTreeSet::from([8000, 8001])
        );
        assert_eq!(
            libraries_behind_catalogue(&libraries, 7002, 4),
            BTreeSet::new(),
            "没人照过的目录库没有落后的库"
        );
    }

    /// 模型门与内存判据必须是同一口径：不早于文件最新会话即覆盖，未认领永不覆盖，时刻不比；
    /// ADR-079 之后它只读水位镜像与进程内账本，一行 `gen_root` 都不问。
    #[test]
    fn model_gate_is_monotone_reads_the_watermark_and_the_ledger_only() {
        let body = include_str!("model_coverage.rs")
            .split_once("pub async fn model_coverage_current(")
            .expect("model gate exists")
            .1
            .split_once("pub(crate) async fn library_catalogue_stamps_on(")
            .expect("model gate ends before the stamps reader")
            .0;
        assert!(
            body.contains("library_coverage_on(&SUL_DB, dbnum)"),
            "库级覆盖读水位镜像，不再问收据"
        );
        assert!(
            body.contains("model_root_ledger::census(dbnum).chasing"),
            "还在赶的根从账本数（ADR-079）: {body}"
        );
        assert!(
            !body.contains("gen_root") && !body.contains("SELECT"),
            "模型门不再扫任何一张表（ADR-079）: {body}"
        );
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

    /// ADR-079：凭证判据不读任何一张逐根表——本文件的生产半边没有一句 `gen_root`。
    #[test]
    fn the_coverage_reader_never_touches_a_per_root_table() {
        let source = include_str!("model_coverage.rs");
        let production = source
            .split(concat!("#[cfg(test)]\n", "mod tests"))
            .next()
            .expect("production half");
        for forbidden in ["FROM gen_root", "gen_root:", "type::thing('gen_root'"] {
            assert!(
                !production.contains(forbidden),
                "覆盖取数不许再读逐根凭证行: {forbidden}"
            );
        }
    }

    #[tokio::test]
    async fn request_reconcile_run_coalesces_inflight_triggers_and_never_runs_two_at_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Notify;

        static COORD: Mutex<ReconcileCoordinator> = Mutex::new(ReconcileCoordinator {
            running: false,
            pending: None,
        });

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(Mutex::new(Vec::<(String, &'static str, Vec<u32>)>::new()));
        let first_entered = Arc::new(Notify::new());
        let release_first = Arc::new(Notify::new());
        let finished_all = Arc::new(Notify::new());

        let make_runner = || {
            let active = active.clone();
            let max_active = max_active.clone();
            let calls = calls.clone();
            let first_entered = first_entered.clone();
            let release_first = release_first.clone();
            let finished_all = finished_all.clone();
            move |origin: String, trigger: &'static str, advanced: Vec<u32>| {
                let active = active.clone();
                let max_active = max_active.clone();
                let calls = calls.clone();
                let first_entered = first_entered.clone();
                let release_first = release_first.clone();
                let finished_all = finished_all.clone();
                async move {
                    let cur = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(cur, Ordering::SeqCst);
                    let idx = {
                        let mut log = calls.lock().unwrap();
                        log.push((origin, trigger, advanced));
                        log.len()
                    };
                    if idx == 1 {
                        first_entered.notify_one();
                        release_first.notified().await;
                    }
                    active.fetch_sub(1, Ordering::SeqCst);
                    if idx == 2 {
                        finished_all.notify_one();
                    }
                }
            }
        };

        assert!(request_reconcile_run_with(
            &COORD,
            "watch",
            "catalogue_file_advanced",
            &[7000],
            make_runner(),
        ));
        first_entered.notified().await;

        // 在飞期间先后合并三条触发（含 periodic 与另一组 advanced dbnum）：
        // 返回 false、只并成一轮重跑、且保留前移嗓音与 [7001, 7002] 并集。
        assert!(!request_reconcile_run_with(
            &COORD,
            "watch",
            "catalogue_file_advanced",
            &[7001],
            make_runner(),
        ));
        assert!(!request_reconcile_run_with(
            &COORD,
            "reconcile",
            "periodic",
            &[],
            make_runner(),
        ));
        assert!(!request_reconcile_run_with(
            &COORD,
            "watch",
            "catalogue_file_advanced",
            &[7002, 7001],
            make_runner(),
        ));

        release_first.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), finished_all.notified())
            .await
            .expect("coalesced second run finishes");
        tokio::task::yield_now().await;

        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        let log = calls.lock().unwrap().clone();
        assert_eq!(
            log,
            vec![
                ("watch".to_string(), "catalogue_file_advanced", vec![7000]),
                (
                    "watch".to_string(),
                    "catalogue_file_advanced",
                    vec![7001, 7002]
                ),
            ]
        );
        assert!(!COORD.lock().unwrap().running);
        assert!(COORD.lock().unwrap().pending.is_none());
    }

    #[tokio::test]
    async fn request_reconcile_run_never_loses_wakeups_or_overlaps_under_concurrent_triggers() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static COORD: Mutex<ReconcileCoordinator> = Mutex::new(ReconcileCoordinator {
            running: false,
            pending: None,
        });

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let last_seen = Arc::new(AtomicUsize::new(0));
        let latest_submitted = Arc::new(AtomicUsize::new(0));

        // 并发制造大量触发，交叉在每一轮即将结束的窗口里，验证：
        // 1. 任何时刻并发在飞 <= 1；
        // 2. 最后一次提交的编号一定被最后一轮观测到（不丢唤醒）。
        let mut handles = Vec::new();
        for i in 1..=40usize {
            let active = active.clone();
            let max_active = max_active.clone();
            let last_seen = last_seen.clone();
            let latest_submitted = latest_submitted.clone();
            handles.push(tokio::spawn(async move {
                latest_submitted.fetch_max(i, Ordering::SeqCst);
                let runner = {
                    let active = active.clone();
                    let max_active = max_active.clone();
                    let last_seen = last_seen.clone();
                    let latest_submitted = latest_submitted.clone();
                    move |_origin: String, _trigger: &'static str, _advanced: Vec<u32>| {
                        let active = active.clone();
                        let max_active = max_active.clone();
                        let last_seen = last_seen.clone();
                        let latest_submitted = latest_submitted.clone();
                        async move {
                            let cur = active.fetch_add(1, Ordering::SeqCst) + 1;
                            max_active.fetch_max(cur, Ordering::SeqCst);
                            let observed = latest_submitted.load(Ordering::SeqCst);
                            tokio::task::yield_now().await;
                            last_seen.fetch_max(observed, Ordering::SeqCst);
                            active.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                };
                request_reconcile_run_with(&COORD, "watch", "catalogue_file_advanced", &[i as u32], runner);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !COORD.lock().unwrap().running && COORD.lock().unwrap().pending.is_none() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("coordinator drains cleanly");

        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        assert_eq!(last_seen.load(Ordering::SeqCst), 40);
    }

    #[tokio::test]
    async fn request_reconcile_run_recovers_running_flag_after_panic() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use tokio::sync::Notify;

        static COORD: Mutex<ReconcileCoordinator> = Mutex::new(ReconcileCoordinator {
            running: false,
            pending: None,
        });

        let should_panic = Arc::new(AtomicBool::new(true));
        let ran = Arc::new(AtomicUsize::new(0));
        let second_done = Arc::new(Notify::new());

        let make_runner = || {
            let should_panic = should_panic.clone();
            let ran = ran.clone();
            let second_done = second_done.clone();
            move |_origin: String, _trigger: &'static str, _advanced: Vec<u32>| {
                let should_panic = should_panic.clone();
                let ran = ran.clone();
                let second_done = second_done.clone();
                async move {
                    if should_panic.swap(false, Ordering::SeqCst) {
                        panic!("simulated reconcile failure");
                    }
                    ran.fetch_add(1, Ordering::SeqCst);
                    second_done.notify_one();
                }
            }
        };

        assert!(request_reconcile_run_with(
            &COORD,
            "watch",
            "catalogue_file_advanced",
            &[7000],
            make_runner(),
        ));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !COORD.lock().unwrap().running {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("panic guard clears running flag");

        assert!(request_reconcile_run_with(
            &COORD,
            "watch",
            "catalogue_file_advanced",
            &[7001],
            make_runner(),
        ));
        tokio::time::timeout(std::time::Duration::from_secs(2), second_done.notified())
            .await
            .expect("subsequent run executes after panic");
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn partial_catalogue_observation_merges_without_erasing_unscanned_keys() {
        let mut state = CatalogueReconcileState {
            latest: None,
            observed: None,
        };
        let f7000 = PathBuf::from("ams7000_0001");
        let f7001 = PathBuf::from("ams7001_0001");
        let f7002 = PathBuf::from("ams7002_0001");

        // 第一轮只播种，不触发。
        let first = BTreeMap::from([((7000, f7000.clone()), 10), ((7001, f7001.clone()), 20)]);
        assert_eq!(
            observe_catalogue_files_in(&mut state, &first),
            (CatalogueSweepVerdict::Seeded, Vec::new())
        );

        // 快道只扫到 7000 前移到 11：触发 7000，且 7001 的观察 20 原样保留。
        let partial_adv = BTreeMap::from([((7000, f7000.clone()), 11)]);
        assert_eq!(
            observe_catalogue_files_in(&mut state, &partial_adv),
            (
                CatalogueSweepVerdict::Run {
                    trigger: "catalogue_file_advanced"
                },
                vec![7000]
            )
        );
        assert_eq!(
            state.observed.as_ref().unwrap().get(&(7001, f7001.clone())),
            Some(&20)
        );

        // 再扫到 7001 仍是 20：没前移也不触发。
        let partial_same = BTreeMap::from([((7001, f7001.clone()), 20)]);
        assert_eq!(
            observe_catalogue_files_in(&mut state, &partial_same),
            (CatalogueSweepVerdict::Unchanged, Vec::new())
        );

        // 新出现 7002：触发 7002，且 7000=11 / 7001=20 均保留。
        let partial_new = BTreeMap::from([((7002, f7002.clone()), 1)]);
        assert_eq!(
            observe_catalogue_files_in(&mut state, &partial_new),
            (
                CatalogueSweepVerdict::Run {
                    trigger: "catalogue_file_advanced"
                },
                vec![7002]
            )
        );
        assert_eq!(state.observed.as_ref().unwrap().len(), 3);
    }
}
