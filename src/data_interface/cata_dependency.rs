//! CATA 依赖闭包按需入库（`docs/plans/2026-09-06-rocksdb-batch-write-and-cata-dependency-plan.md` §3 D5–D7，共识 d-29）。
//!
//! **口径**：CATA **必须**入库，但只入被本批 DESI 元素所依赖的那部分（评审批注 2026-09-06：「需要保存，按 DESI
//! 解析的依赖的 CATA，按依赖保存」）。每一批 DESI 写回（基线、稳态窗口、模型根发布）都以本批 DESI 元素为 seed
//! 算依赖闭包，把闭包里的 CATA 元素与 DESI 行一起写进 `pe` / `<NOUN>` / `ATT_UDA` / `pe_owner`。
//! 一个 DESI 元素落库而它的目录链在库里缺行，算缺陷。ADR-058 D1 / S3「CATA 不入持久层」随之修订为
//! 「CATA 只按 DESI 依赖入库，不做整库基线」。
//!
//! **判据只有一份**：
//!
//! ```text
//! seed(写回批) = 本批 DESI 元素上目录链引用属性（CATALOGUE_CHAIN_ATTRIBUTES，与 e3d-model
//!                reference_reads_guard 的名单同源）的出向 refno
//! closure      = CataClosureResolver（precise：容器子树只展开几何 / 点集容器，owner 链到 WORL，
//!                db_type 收口 CATA，排除 DESI 自身）
//! ```
//!
//! 闭包比 e3d-model 实读**宽**是允许的（多备份几行），窄就是漏。对拍 oracle 是 e3d-model
//! `GenerationOutcome::catalogue_visited`（一次生成真读到的目录元素，记录点在 e3d-io 读入口，2026-09-07 落地）：
//! `live_ams8000_catalogue_reads_are_covered_by_the_dependency_closure` 对 ams8000 每个生成根钉
//! `catalogue_visited ⊆ closure(seed(visited))`，`unexplained = 0`。
//!
//! **行**经同一个 `element_batch` 渲染（`pe.dependency = true` 标它是按依赖入库的，不代表该 CATA 库整库在场），
//! 首次 `Fresh`（`INSERT IGNORE`，命中即零成本）；CATA 文件版本前移的 `Replace` 由 `cata_dependency_state:{dbnum}`
//! 驱动（P5）。三个写入点：数据窗口（排在 DESI 块与水位之前）、基线收尾、模型根发布前的独立事务。
//! 旧的惰性兜底（`cata_closure::ensure_cata_refnos_parsed`：`cata_ref_rev` 补偿队列与 resolve 层 miss）
//! 自 2026-09-07 起也经 [`persist_resolved_rows`] 落库（ADR-064 Q2）——目录行进库只有这一个出口。
//! CATA 库**不进** `dbnum_watermark.applied_sesno`（ADR-001 语义不变）。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aios_core::options::DbOption;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64, SUL_DB, get_db_option};
use anyhow::Context;
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::cata_closure::{
    CataClosureConfig, CataClosureManifest, CataClosureResolver, CataDbLocator,
    InMemoryCataLocator, is_valid_ref0,
};
use crate::data_interface::ele_data::EleData;
use crate::data_interface::ele_ops::{ElementRows, UdaKeyMap};
use crate::data_interface::element_batch::{BatchItem, WriteMode, render_item_transactions};

/// e3d-model 几何管线实读的目录链引用属性（`vendor/e3d-model/tests/reference_reads_guard.rs`：
/// 有反向表 12 个里指向 CATA 的那一族 + 无表跨单元的 `HSRO` / `LSRO`）。`POSREF` / `STAREF` / `ENDREF`
/// 指向设计侧，不在这里。名单变了两边一起改，`catalogue_chain_attributes_match_e3d_model_guard` 钉着。
pub const CATALOGUE_CHAIN_ATTRIBUTES: &[&str] = &[
    "SPRE", "CATR", "PRTREF", "PTRE", "GMRE", "GSTR", "PSTR", "HSTU", "LSTU", "HSRO", "LSRO",
];

/// 从一个 DESI 元素的属性表抽目录链引用的出向 refno（只看 [`CATALOGUE_CHAIN_ATTRIBUTES`]）。
pub fn seeds_from_attmap(att: &NamedAttrMap) -> BTreeSet<RefU64> {
    let mut out = BTreeSet::new();
    for name in CATALOGUE_CHAIN_ATTRIBUTES {
        let Some(value) = att.map.get(*name) else {
            continue;
        };
        match value {
            NamedAttrValue::RefU64Type(r) => {
                if is_valid_ref0(r.get_0()) {
                    out.insert(*r);
                }
            }
            NamedAttrValue::RefnoEnumType(re) => {
                let r = re.refno();
                if is_valid_ref0(r.get_0()) {
                    out.insert(r);
                }
            }
            NamedAttrValue::RefU64Array(arr) => {
                for re in arr.iter() {
                    let r = re.refno();
                    if is_valid_ref0(r.get_0()) {
                        out.insert(r);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// 一批 DESI 元素的 seed 合集。
pub fn seeds_from_elements<'a>(
    elements: impl IntoIterator<Item = &'a EleData>,
) -> BTreeSet<RefU64> {
    let mut out = BTreeSet::new();
    for element in elements {
        out.extend(seeds_from_attmap(element.att_map()));
    }
    out
}

/// 闭包解出来的行：按 CATA dbnum 分组的 `(sesno, EleData)`，外加闭包账。
#[derive(Default)]
pub struct CataDependencyRows {
    pub by_dbnum: BTreeMap<u32, Vec<(u32, EleData)>>,
    pub manifest: CataClosureManifest,
    /// 解析这一侧的拆项账（S0 取证）；经窗口工件过一遍线的行没有它（零值）。
    pub account: DependencyResolveAccount,
}

/// 一次 `resolve_dependency_rows` 的拆项账（plan 2026-09-15 S0 取证）。**只记账，不改行为。**
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyResolveAccount {
    /// 每库读一次文件头取目录会话号（`catalogue_sesno`）累计。
    pub identity_ms: u128,
    /// `CataClosureResolver::resolve()` 整段（含 open / parse / expand）。
    pub resolve_ms: u128,
    /// `take_attmaps` → `element_from_attmap` 装行。
    pub rows_ms: u128,
    /// 闭包引擎自己的拆项。
    pub closure: crate::data_interface::cata_closure::ClosureAccount,
}

fn zero_usize(value: &usize) -> bool {
    *value == 0
}

fn zero_u128(value: &u128) -> bool {
    *value == 0
}

impl CataDependencyRows {
    pub fn elements(&self) -> usize {
        self.by_dbnum.values().map(Vec::len).sum()
    }
}

/// 闭包配置：`precise()` 的容器白名单（几何 / 点集 / 轮廓容器），db_type **只收 CATA**（`precise()`
/// 还会下探回指 DESI 的引用，这里不要——DESI 行由数据窗口自己写），有本批 DESI 库的把它排除。
fn closure_config(excluded_design_dbnum: Option<u32>) -> CataClosureConfig {
    let mut cfg = CataClosureConfig::precise();
    cfg.cata_db_types = ["CATA".to_string()].into_iter().collect();
    match excluded_design_dbnum {
        Some(dbnum) => cfg.excluding_dbnum(dbnum),
        None => cfg,
    }
}

/// seed → 闭包 → 行。`project` 给 locator（`dbnum_watermark` 文件身份 + 依赖清单）；读不到的 refno
/// 进 `manifest.missing`（库不在 MDB / 文件缺失 / 元素不在），不是错误——调用方按 Q3 只记 warning。
pub async fn resolve_dependency_rows(
    project: &str,
    design_dbnum: u32,
    seeds: &BTreeSet<RefU64>,
) -> anyhow::Result<CataDependencyRows> {
    resolve_dependency_rows_on(&SUL_DB, get_db_option(), project, design_dbnum, seeds).await
}

/// 同 [`resolve_dependency_rows`]：locator 的文件身份从 `db` 读、目录根按 `option` 解析，
/// 闭包里的解析失败也记到 `db`。
pub async fn resolve_dependency_rows_on(
    db: &Surreal<Any>,
    option: &DbOption,
    project: &str,
    design_dbnum: u32,
    seeds: &BTreeSet<RefU64>,
) -> anyhow::Result<CataDependencyRows> {
    if seeds.is_empty() {
        return Ok(CataDependencyRows::default());
    }
    let locator = InMemoryCataLocator::build_for_project_on(db, option, project)
        .await
        .context("build CATA locator for dependency closure")?;
    resolve_rows_with_config(db, &locator, closure_config(Some(design_dbnum)), seeds).await
}

/// 同 [`resolve_dependency_rows`]，定位器由调用方给：闭包路径只有这一条，对拍 oracle
/// （`live_ams8000_catalogue_reads_are_covered_by_the_dependency_closure`）用文件扫描出来的
/// 定位器走**同一条**路径，而不是另抄一份闭包。
pub async fn resolve_dependency_rows_with<L: CataDbLocator>(
    locator: &L,
    design_dbnum: u32,
    seeds: &BTreeSet<RefU64>,
) -> anyhow::Result<CataDependencyRows> {
    resolve_rows_with_config(&SUL_DB, locator, closure_config(Some(design_dbnum)), seeds).await
}

/// 惰性兜底（`cata_closure::ensure_cata_refnos_parsed`）走的**同一条**闭包路：seed 已经是目录侧 refno，
/// 没有「本批 DESI 库」可排除；db_type 照样只收 CATA——这条路以前拿 `precise()` 原样跑，会把回指到
/// 别的 DESI 库的元素也解出来写进去，而 DESI 行只能由它自己的数据窗口写（ADR-064 Q2）。
pub async fn resolve_catalogue_rows_with<L: CataDbLocator>(
    locator: &L,
    seeds: &BTreeSet<RefU64>,
) -> anyhow::Result<CataDependencyRows> {
    resolve_rows_with_config(&SUL_DB, locator, closure_config(None), seeds).await
}

async fn resolve_rows_with_config<L: CataDbLocator>(
    db: &Surreal<Any>,
    locator: &L,
    config: CataClosureConfig,
    seeds: &BTreeSet<RefU64>,
) -> anyhow::Result<CataDependencyRows> {
    if seeds.is_empty() {
        return Ok(CataDependencyRows::default());
    }
    let mut resolver = CataClosureResolver::new(locator, config)
        .with_retain_attmaps(true)
        .with_db(db);
    resolver.seed(seeds.iter().copied());
    let resolve_started = std::time::Instant::now();
    let manifest = resolver
        .resolve()
        .await
        .context("resolve CATA dependency closure")?;
    let resolve_ms = resolve_started.elapsed().as_millis();
    let mut attmaps = resolver.take_attmaps();

    // 每库一次文件头：目录会话号（P5 版本前移的比较基准）。
    let identity_started = std::time::Instant::now();
    let sesnos: BTreeMap<u32, u32> = manifest
        .by_dbnum
        .keys()
        .map(|dbnum| (*dbnum, catalogue_sesno(locator, *dbnum)))
        .collect();
    let identity_ms = identity_started.elapsed().as_millis();

    let rows_started = std::time::Instant::now();
    let mut by_dbnum: BTreeMap<u32, Vec<(u32, EleData)>> = BTreeMap::new();
    for (dbnum, refnos) in &manifest.by_dbnum {
        let sesno = sesnos.get(dbnum).copied().unwrap_or(0);
        let rows = by_dbnum.entry(*dbnum).or_default();
        for refno in refnos {
            let Some((att, children)) = attmaps.remove(refno) else {
                continue;
            };
            rows.push((sesno, element_from_attmap(*refno, att, children)));
        }
    }
    let rows_ms = rows_started.elapsed().as_millis();
    let account = DependencyResolveAccount {
        identity_ms,
        resolve_ms,
        rows_ms,
        closure: manifest.account.clone(),
    };
    let rows = CataDependencyRows {
        by_dbnum,
        manifest,
        account,
    };
    if rows.elements() > 0 || rows.manifest.missing > 0 {
        let closure = &rows.account.closure;
        println!(
            "[cata_closure] seeds={} → {} 行 / {} 库 / {} 轮，缺失 {}：resolve {} ms（缓存 {} 命中 / {} 未命中，身份 {} ms，open {} ms × {} 会话，parse {} ms / {} 次请求 / {} 成功，expand {} ms），identity {} ms，装行 {} ms",
            rows.manifest.seed_count,
            rows.elements(),
            rows.by_dbnum.len(),
            rows.manifest.rounds,
            rows.manifest.missing,
            rows.account.resolve_ms,
            closure.cache_hits,
            closure.cache_misses,
            closure.identity_ms,
            closure.open_ms,
            closure.sessions_opened,
            closure.parse_ms,
            closure.parse_requests,
            closure.parsed,
            closure.expand_ms,
            rows.account.identity_ms,
            rows.account.rows_ms,
        );
    }
    Ok(rows)
}

/// CATA 文件当前的会话号——`pe.sesno` 要说得出这行是按哪一版目录文件渲的（P5 版本前移的比较基准）。
fn catalogue_sesno<L: CataDbLocator>(locator: &L, dbnum: u32) -> u32 {
    locator
        .file_of(dbnum)
        .and_then(|(_, path)| {
            crate::data_interface::frozen_source::FrozenSource::capture(&path)
                .ok()
                .map(|frozen| frozen.target_sesno())
        })
        .unwrap_or(0)
}

/// 闭包解析器交回的 `(属性表, 成员)` → `EleData`（与 DESI 行同一个渲染入口 `ElementRows::render`）。
/// 目录元素的 UDA 值不在闭包解析器的产物里，`uda_atts` 为空——目录 UDA 不参与几何，先不背。
fn element_from_attmap(refno: RefU64, att: NamedAttrMap, children: Vec<RefU64>) -> EleData {
    let owner = att
        .get_refno_by_att("OWNER")
        .map(|owner| owner.refno())
        .unwrap_or_default();
    let name = att
        .map
        .get("NAME")
        .and_then(|value| match value {
            NamedAttrValue::StringType(name) => Some(name.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let noun = aios_core::tool::db_tool::db1_hash(&att.get_type());
    let mut element = EleData::default();
    element.refno = refno;
    element.owner = owner;
    element.noun = noun;
    element.name = name;
    element.children = aios_core::RefU64Vec(children);
    element.whole_attmap.attmap = att;
    element
}

/// 一次依赖入库的账（回执 / 日志 / `/health`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CataDependencyReport {
    pub seeds: usize,
    pub elements: usize,
    pub dbnums: Vec<u32>,
    pub transactions: usize,
    pub missing: usize,
    pub missing_samples: Vec<String>,
    pub elapsed_ms: u128,
    /// D7：本批顺手做的目录版本前移刷新（每个前移了的目录库一条）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refreshed: Vec<CatalogueRefresh>,
    /// S0 取证（plan 2026-09-15）：解析 / 渲染的拆项账。零值不序列化——回执形状对旧读者不变；
    /// 经窗口工件过线的行没有解析账，这几格就是零。
    #[serde(default, skip_serializing_if = "zero_u128")]
    pub resolve_ms: u128,
    #[serde(default, skip_serializing_if = "zero_u128")]
    pub render_ms: u128,
    /// 渲出来送去执行的 SQL 字节数（`Fresh` 命中也算——那正是要省的那份）。
    #[serde(default, skip_serializing_if = "zero_usize")]
    pub sql_bytes: usize,
    #[serde(default, skip_serializing_if = "zero_usize")]
    pub parse_requests: usize,
    #[serde(default, skip_serializing_if = "zero_usize")]
    pub parsed: usize,
    /// S2：解析缓存命中 / 未命中的元素数（`cache_hits + parsed` = 这一批拿到的元素）。
    #[serde(default, skip_serializing_if = "zero_usize")]
    pub cache_hits: usize,
    #[serde(default, skip_serializing_if = "zero_usize")]
    pub cache_misses: usize,
    /// S3（D6）：这一版已经落过库、因此没有再渲的元素数。`elements` 仍是闭包解出来的总数——
    /// 「依赖多少行」与「这一批写了多少行」是两笔账。
    #[serde(default, skip_serializing_if = "zero_usize")]
    pub skipped_persisted: usize,
}

impl CataDependencyReport {
    /// 闭包缺失要在回执里出声（Q3：不拦水位，但不许静默）；整库刷新（差分退化）也要点名。
    pub fn warning(&self, design_dbnum: u32) -> Option<String> {
        let mut parts = Vec::new();
        if self.missing > 0 {
            parts.push(format!(
                "CATA 依赖闭包有 {} 个 refno 落不了库（库不在 MDB / 文件缺失 / 元素不在），样例 {:?}",
                self.missing,
                self.missing_samples.iter().take(5).collect::<Vec<_>>()
            ));
        }
        for refresh in self.refreshed.iter().filter(|refresh| refresh.full_refresh) {
            parts.push(format!(
                "cata_dependency_full_refresh: 目录库 {} 的 {}→{} 差分不可用（{}），已入库的 {} 行依赖整库重渲",
                refresh.dbnum,
                refresh.base_sesno,
                refresh.target_sesno,
                refresh.degraded_reason.as_deref().unwrap_or("unknown"),
                refresh.refreshed
            ));
        }
        (!parts.is_empty()).then(|| format!("dbnum={design_dbnum}: {}", parts.join("；")))
    }
}

/// D7：一个目录库的版本前移刷新账。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CatalogueRefresh {
    pub dbnum: u32,
    pub base_sesno: u32,
    pub target_sesno: u32,
    /// 窗口里变动的目录件数（含与依赖行无交集的）。
    pub changed: usize,
    /// 与已入库依赖行相交、被 Replace 重渲的元素数（含新闭包带进来的）。
    pub refreshed: usize,
    /// 被删目录件里已入库的，落成最小墓碑的条数。
    pub tombstoned: usize,
    /// 差分不可用（会话不在链上 / 候选超上限）→ 整库依赖行重渲。
    pub full_refresh: bool,
    pub degraded_reason: Option<String>,
}

/// 差分候选超过这个数就不逐条读，退化为整库依赖行重渲（与 `cata_cascade_window` 启动路径同一阈值）。
const CATALOGUE_WINDOW_MAX_RECORDS: usize = 20_000;

/// D7：目录文件前移 → 已入库依赖行里被改的 Replace 重渲、被删的落墓碑 → 推进 `cata_dependency_state.sesno`。
///
/// 只看 `dbnums` 里**有依赖状态行**的目录库；文件没前移的一步不走。差分不可用（记在案的会话被
/// MERGE CHANGES 折掉、候选超上限）时退化为「该库全部依赖行 Replace」，回执点名 `cata_dependency_full_refresh`。
pub async fn refresh_moved_catalogues(
    project: &str,
    dbnums: impl IntoIterator<Item = u32>,
    uda_keys: &UdaKeyMap,
) -> anyhow::Result<Vec<CatalogueRefresh>> {
    refresh_moved_catalogues_on(&SUL_DB, get_db_option(), project, dbnums, uda_keys).await
}

/// 同 [`refresh_moved_catalogues`]，目标库由调用方给：本地优先化的结构化 sink 在 backup lane 上
/// 落 CATA 依赖行时用它（`docs/plans/2026-09-09-local-first-data-window-plan.md` §2.3）；
/// 文件侧的差分不变。读写库的那几口、locator 的文件身份与闭包的解析失败都走 `db`，目录根按
/// `option` 解析。
pub async fn refresh_moved_catalogues_on(
    db: &Surreal<Any>,
    option: &DbOption,
    project: &str,
    dbnums: impl IntoIterator<Item = u32>,
    uda_keys: &UdaKeyMap,
) -> anyhow::Result<Vec<CatalogueRefresh>> {
    let dbnums = dbnums.into_iter().collect::<BTreeSet<_>>();
    if dbnums.is_empty() {
        return Ok(Vec::new());
    }
    let locator = InMemoryCataLocator::build_for_project_on(db, option, project)
        .await
        .context("build CATA locator for dependency refresh")?;
    let mut out = Vec::new();
    for dbnum in dbnums {
        let Some((base_sesno, _rows)) = dependency_rows_of_on(db, dbnum).await? else {
            continue; // 从没入过依赖行的库没有可前移的东西
        };
        let Some((_, file)) = locator.file_of(dbnum) else {
            continue;
        };
        let target_sesno = crate::data_interface::frozen_source::FrozenSource::capture(&file)
            .ok()
            .map(|frozen| frozen.target_sesno())
            .unwrap_or(0);
        if base_sesno == 0 || target_sesno <= base_sesno {
            continue;
        }
        let mut refresh = CatalogueRefresh {
            dbnum,
            base_sesno,
            target_sesno,
            ..Default::default()
        };
        // 差分是阻塞 I/O。
        let window = {
            let file = file.clone();
            tokio::task::spawn_blocking(move || {
                crate::data_interface::cata_cascade_window::collect_catalogue_window_bounded(
                    dbnum,
                    &file,
                    base_sesno,
                    target_sesno,
                    CATALOGUE_WINDOW_MAX_RECORDS,
                )
            })
            .await
            .map_err(|error| anyhow::anyhow!("catalogue window task failed: {error}"))?
        };
        let (targets, deleted): (Vec<RefU64>, Vec<RefU64>) = match window {
            Ok(window) if window.truncated_candidates.is_none() => {
                refresh.changed = window.changes.len();
                let mut targets = Vec::new();
                let mut deleted = Vec::new();
                for change in &window.changes {
                    let refno = RefU64::from_two_nums(change.key.0, change.key.1);
                    match change.kind {
                        crate::data_interface::cata_cascade_window::CatalogueChangeKind::Deleted => {
                            deleted.push(refno)
                        }
                        _ => targets.push(refno),
                    }
                }
                (targets, deleted)
            }
            Ok(window) => {
                refresh.full_refresh = true;
                refresh.degraded_reason = Some(format!(
                    "候选 {} 超过 {CATALOGUE_WINDOW_MAX_RECORDS}",
                    window.truncated_candidates.unwrap_or_default()
                ));
                (all_dependency_refnos_on(db, dbnum).await?, Vec::new())
            }
            Err(error) => {
                refresh.full_refresh = true;
                refresh.degraded_reason = Some(format!("{error:#}"));
                (all_dependency_refnos_on(db, dbnum).await?, Vec::new())
            }
        };
        // 只刷已入库的依赖行：与 `pe … dependency = true` 求交（点查，不扫表）。
        let present = dependency_rows_among_on(db, &targets).await?;
        let gone = dependency_rows_among_on(db, &deleted).await?;
        if !present.is_empty() {
            let seeds = present.iter().copied().collect::<BTreeSet<_>>();
            let mut resolver = CataClosureResolver::new(&locator, {
                let mut cfg = CataClosureConfig::precise();
                cfg.cata_db_types = ["CATA".to_string()].into_iter().collect();
                cfg
            })
            .with_retain_attmaps(true)
            .with_db(db);
            resolver.seed(seeds.iter().copied());
            let manifest = resolver
                .resolve()
                .await
                .with_context(|| format!("re-resolve moved catalogue rows of dbnum {dbnum}"))?;
            let mut attmaps = resolver.take_attmaps();
            let mut rows = CataDependencyRows::default();
            for (row_dbnum, refnos) in &manifest.by_dbnum {
                let entry = rows.by_dbnum.entry(*row_dbnum).or_default();
                for refno in refnos {
                    if let Some((att, children)) = attmaps.remove(refno) {
                        entry.push((target_sesno, element_from_attmap(*refno, att, children)));
                    }
                }
            }
            refresh.refreshed = rows.elements();
            for (row_dbnum, sqls) in
                render_dependency_transactions(&rows, uda_keys, WriteMode::Replace)?
            {
                for sql in sqls {
                    crate::data_interface::pe_batch::execute_on(db, &sql)
                        .await
                        .with_context(|| {
                            format!("refresh moved catalogue rows of dbnum {row_dbnum}")
                        })?;
                }
            }
        }
        if !gone.is_empty() {
            let changes = gone
                .iter()
                .map(
                    |refno| crate::data_interface::ele_ops::PersistElementChange {
                        refno: *refno,
                        sesno: target_sesno,
                        state: crate::data_interface::ele_ops::PersistElementState::Deleted {
                            base: None,
                        },
                    },
                )
                .collect::<Vec<_>>();
            let (sqls, _) = crate::data_interface::element_batch::render_transactions(
                &changes,
                &[],
                dbnum as i32,
                uda_keys,
                WriteMode::Replace,
            )?;
            for sql in sqls {
                crate::data_interface::pe_batch::execute_on(db, &sql)
                    .await
                    .with_context(|| {
                        format!("tombstone deleted catalogue rows of dbnum {dbnum}")
                    })?;
            }
            refresh.tombstoned = gone.len();
        }
        db.query(render_dependency_state(dbnum, target_sesno, 0))
            .await
            .and_then(|response| response.check())
            .with_context(|| format!("advance CATA dependency state of dbnum {dbnum}"))?;
        // D6：这一库的依赖行刚被 Replace / 墓碑动过，记忆整库作废（新 sesno 本来也不会命中，
        // 清是不把死条目留在表里）。
        forget_persisted_catalogue_rows(ForgetScope::Catalogue(dbnum));
        println!(
            "[cata_dependency] 目录库 {dbnum} 前移 {base_sesno}→{target_sesno}：变动 {}，刷新 {} 行，墓碑 {}{}",
            refresh.changed,
            refresh.refreshed,
            refresh.tombstoned,
            if refresh.full_refresh {
                "（差分不可用，整库依赖行重渲）"
            } else {
                ""
            }
        );
        out.push(refresh);
    }
    Ok(out)
}

/// 一次「目录版本核对之后」的数据面前移账（A3，2026-09-08）——**不挂在哪个 DESI 批次上**。
///
/// [`refresh_moved_catalogues`] 此前只在某个写回批的闭包碰到那个目录库时才跑（D7）：目录改了、
/// 设计没改，`pe` 里 `dependency = true` 的目录行与它们的 `ref_rev` 就停在旧版本上——模型面那一半
/// 由 `reconcile_catalogue_versions` 反查排根，属性面板 / `ref_rev` 看到的目录属性却还是旧的。
/// 现在每次目录版本核对排完根之后，经 [`refresh_dependency_rows_of_moved_catalogues`] 把**全部有
/// 依赖状态行**的目录库过一遍；这份账随核对回执进 `/health`。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CatalogueDependencyRefreshReport {
    /// 有 `cata_dependency_state` 行的目录库数（候选）。文件没前移的一步不走，不在 `refreshed` 里。
    pub candidates: usize,
    /// 文件比已入库依赖行新、真做了 Replace 重渲 / 墓碑 / 推进状态行的目录库，每库一条。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refreshed: Vec<CatalogueRefresh>,
    /// 没跑成的库（`dbnum: 原因`；候选名单读不到时是一条不带 dbnum 的话）：那一库一行都没
    /// 前移，是未知不是零。一库失败不挡别的库。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    pub elapsed_ms: u128,
}

impl CatalogueDependencyRefreshReport {
    /// 什么都没发生：没有库前移、也没有失败——横幅一个字不打（周期核对 300 s 一拍，打了就是刷屏）。
    pub fn is_quiet(&self) -> bool {
        self.refreshed.is_empty() && self.errors.is_empty()
    }

    /// 控制台上的一行；`None` = 没什么可说。逐库的明细 [`refresh_moved_catalogues`] 自己已经
    /// 打过（`[cata_dependency] 目录库 … 前移 …`），这里只收总账与失败。
    pub fn banner_line(&self) -> Option<String> {
        if self.is_quiet() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.refreshed.is_empty() {
            let dbnums: Vec<u32> = self.refreshed.iter().map(|refresh| refresh.dbnum).collect();
            let rows: usize = self.refreshed.iter().map(|refresh| refresh.refreshed).sum();
            let tombstoned: usize = self
                .refreshed
                .iter()
                .map(|refresh| refresh.tombstoned)
                .sum();
            let degraded: Vec<u32> = self
                .refreshed
                .iter()
                .filter(|refresh| refresh.full_refresh)
                .map(|refresh| refresh.dbnum)
                .collect();
            parts.push(format!(
                "目录库 {dbnums:?} 的已入库依赖行随目录前移：刷新 {rows} 行，墓碑 {tombstoned}{}",
                if degraded.is_empty() {
                    String::new()
                } else {
                    format!(
                        "；{degraded:?} 差分不可用、整库依赖行重渲（cata_dependency_full_refresh）"
                    )
                }
            ));
        }
        if !self.errors.is_empty() {
            parts.push(format!(
                "{} 个库没跑成（那一库一行都没前移，未知不是零）：{}",
                self.errors.len(),
                self.errors.join("；")
            ));
        }
        Some(format!(
            "数据面目录依赖行前移（{} 个候选库，{} ms）：{}",
            self.candidates,
            self.elapsed_ms,
            parts.join("；")
        ))
    }
}

/// A3：把**全部有依赖状态行**的目录库过一遍 [`refresh_moved_catalogues`]——逐库调用，一库失败记进
/// `errors`、不挡别的库；locator 有项目级缓存，逐库调不多花什么。
///
/// 候选名单来自 `cata_dependency_state`，**不是**模型面那份「过期」名单：依赖行落后与已发布根落后是
/// 两件事——一个根都没发布过的库也可能早就有依赖行（数据批次入的），反过来根落款到了新目录、
/// 依赖行却还停在旧版本（`ensure` 重算根不碰 `pe`）。文件没前移的库在 [`refresh_moved_catalogues`]
/// 里一步不走。
pub async fn refresh_dependency_rows_of_moved_catalogues(
    project: &str,
) -> CatalogueDependencyRefreshReport {
    let started = std::time::Instant::now();
    let mut report = CatalogueDependencyRefreshReport::default();
    let dbnums = match dependency_state_dbnums_on(&SUL_DB).await {
        Ok(dbnums) => dbnums,
        Err(error) => {
            report
                .errors
                .push(format!("读不到 cata_dependency_state 候选名单: {error:#}"));
            report.elapsed_ms = started.elapsed().as_millis();
            return report;
        }
    };
    report.candidates = dbnums.len();
    if dbnums.is_empty() {
        report.elapsed_ms = started.elapsed().as_millis();
        return report;
    }
    let uda_keys = match UdaKeyMap::load().await {
        Ok(uda_keys) => uda_keys,
        Err(error) => {
            report.errors.push(format!("读不到 UDA 键表: {error:#}"));
            report.elapsed_ms = started.elapsed().as_millis();
            return report;
        }
    };
    for dbnum in dbnums {
        match refresh_moved_catalogues(project, [dbnum], &uda_keys).await {
            Ok(refreshed) => report.refreshed.extend(refreshed),
            Err(error) => report.errors.push(format!("{dbnum}: {error:#}")),
        }
    }
    report.elapsed_ms = started.elapsed().as_millis();
    report
}

/// 有依赖状态行的目录库（升序去重）。`dbnum` 缺格的行跳过——它没法对到文件。
async fn dependency_state_dbnums_on(db: &Surreal<Any>) -> anyhow::Result<Vec<u32>> {
    let mut response = db
        .query("SELECT VALUE dbnum FROM cata_dependency_state;")
        .await?
        .check()?;
    let dbnums: Vec<Option<u32>> = response.take(0)?;
    Ok(dbnums
        .into_iter()
        .flatten()
        .collect::<BTreeSet<u32>>()
        .into_iter()
        .collect())
}

/// 这些 refno 里哪些是已入库的依赖行（`pe … dependency = true`）：点查，不扫表。
async fn dependency_rows_among_on(
    db: &Surreal<Any>,
    refnos: &[RefU64],
) -> anyhow::Result<Vec<RefU64>> {
    let mut out = Vec::new();
    for chunk in refnos.chunks(200) {
        let keys = chunk
            .iter()
            .map(|refno| format!("pe:{refno}"))
            .collect::<Vec<_>>()
            .join(",");
        let mut response = db
            .query(format!(
                "SELECT VALUE record::id(id) FROM [{keys}] WHERE record::exists(id) AND dependency = true;"
            ))
            .await?
            .check()?;
        let ids: Vec<String> = response.take(0)?;
        out.extend(ids.iter().filter_map(|id| id.parse::<RefU64>().ok()));
    }
    Ok(out)
}

/// 一个目录库全部已入库的依赖行（差分退化时的整库重渲名单）。
async fn all_dependency_refnos_on(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<Vec<RefU64>> {
    let mut response = db
        .query(format!(
            "SELECT VALUE record::id(id) FROM pe WHERE dbnum = {dbnum} AND dependency = true;"
        ))
        .await?
        .check()?;
    let ids: Vec<String> = response.take(0)?;
    Ok(ids
        .iter()
        .filter_map(|id| id.parse::<RefU64>().ok())
        .collect())
}

/// 把依赖行渲成事务（每个 CATA dbnum 各自一组——pe 统计观测按库记账）。
pub fn render_dependency_transactions(
    rows: &CataDependencyRows,
    uda_keys: &UdaKeyMap,
    mode: WriteMode,
) -> anyhow::Result<Vec<(u32, Vec<String>)>> {
    let mut out = Vec::new();
    for (dbnum, elements) in &rows.by_dbnum {
        out.push((
            *dbnum,
            render_one_catalogue(*dbnum, elements.iter(), uda_keys, mode)?,
        ));
    }
    Ok(out)
}

/// 一个目录库的一批元素 → 事务。两条渲染路（全量与 D6 摘剩的）共用这一处，
/// 否则「摘掉几行」迟早变成「渲得不一样」。
fn render_one_catalogue<'a>(
    dbnum: u32,
    elements: impl Iterator<Item = &'a (u32, EleData)>,
    uda_keys: &UdaKeyMap,
    mode: WriteMode,
) -> anyhow::Result<Vec<String>> {
    let items = elements
        .map(|(sesno, element)| BatchItem::Rows {
            rows: ElementRows::render(
                element,
                &element.refno.to_string(),
                dbnum as i32,
                *sesno,
                uda_keys,
            ),
            is_add: false,
            old_noun: None,
            dependency: true,
        })
        .collect::<Vec<_>>();
    let (transactions, _) = render_item_transactions(&items, dbnum as i32, mode)?;
    Ok(transactions)
}

/// D6：一个目录库这一批真要落的东西（memo 摘剩的）。
pub struct CatalogueBatch {
    /// 这一批行的目录会话号（同一库同一批只有一个）。
    pub sesno: u32,
    /// 真要落的元素，按渲染顺序——写成功之后按它记忆。
    pub refnos: Vec<RefU64>,
    pub transactions: Vec<String>,
}

/// D6：一批依赖行摘掉「已经落过库的那些」之后要渲的东西。
pub struct DependencyBatches {
    /// 每个还有东西要落的目录库一条。**摘光的库不在里面**：不渲事务，也不发状态行
    /// （`cata_dependency_state.rows` 是量级账，不加零）。
    pub by_dbnum: Vec<(u32, CatalogueBatch)>,
    /// 被 memo 摘掉的元素数（回执 `skipped_persisted`）。
    pub skipped: usize,
}

/// 同 [`render_dependency_transactions`]，但先按 D6 的已落库记忆摘：同库、同 sesno 已经写过的
/// 元素不再渲、不再发。
///
/// 闭包账（`manifest` / `missing` / `CataDependencyRows::elements`）**不摘**——那记的是
/// 「这一批依赖多少行」，不是「这一批写了多少行」。
pub fn render_dependency_transactions_unpersisted(
    rows: &CataDependencyRows,
    uda_keys: &UdaKeyMap,
    mode: WriteMode,
) -> anyhow::Result<DependencyBatches> {
    let mut skipped = 0;
    let mut kept: Vec<(u32, u32, Vec<&(u32, EleData)>)> = Vec::new();
    {
        // 锁里只做查表，渲染在锁外。
        let memo = persisted_dependency_memo();
        for (dbnum, elements) in &rows.by_dbnum {
            let sesno = elements.first().map(|(sesno, _)| *sesno).unwrap_or(0);
            let mut fresh = Vec::with_capacity(elements.len());
            for entry in elements {
                if memo.rows.get(&(*dbnum, entry.1.refno)) == Some(&entry.0) {
                    skipped += 1;
                } else {
                    fresh.push(entry);
                }
            }
            if !fresh.is_empty() {
                kept.push((*dbnum, sesno, fresh));
            }
        }
    }
    let mut by_dbnum = Vec::with_capacity(kept.len());
    for (dbnum, sesno, elements) in kept {
        let transactions = render_one_catalogue(dbnum, elements.iter().copied(), uda_keys, mode)?;
        by_dbnum.push((
            dbnum,
            CatalogueBatch {
                sesno,
                refnos: elements.iter().map(|(_, element)| element.refno).collect(),
                transactions,
            },
        ));
    }
    Ok(DependencyBatches { by_dbnum, skipped })
}

/// `cata_dependency_state:{dbnum}`：这个 CATA 库的依赖行是按哪一版文件渲的、有多少行。**不是水位**
/// （不进 `dbnum_watermark`、不参与 ADR-021 的判据），只给 P5 的版本前移与 `/health` 用。
pub fn render_dependency_state(dbnum: u32, sesno: u32, elements: usize) -> String {
    format!(
        "UPSERT cata_dependency_state:{dbnum} SET dbnum = {dbnum}, sesno = math::max([sesno ?: 0, {sesno}]), \
         rows = (rows ?: 0) + {elements}, updated_at = time::now();"
    )
}

/// 三个写入点共用的入口：seed → 闭包 → （目录文件前移的先 Replace 刷新，D7）→ `Fresh` 入库 → 状态行。
///
/// - 闭包解析失败（locator 建不起来、文件 I/O）→ `Err`：调用方决定拦不拦（数据窗口拦水位，模型发布只 warning）。
/// - 闭包缺失（库不在 MDB / 元素不在）→ 进 `report.missing`，不是 `Err`（Q3）。
pub async fn persist_cata_dependencies(
    project: &str,
    design_dbnum: u32,
    seeds: &BTreeSet<RefU64>,
) -> anyhow::Result<CataDependencyReport> {
    let started = std::time::Instant::now();
    let rows = resolve_dependency_rows(project, design_dbnum, seeds).await?;
    let mut report = persist_resolved_rows(project, &rows).await?;
    report.seeds = seeds.len();
    report.elapsed_ms = started.elapsed().as_millis();
    if report.elements > 0 || report.missing > 0 {
        println!(
            "[cata_dependency] dbnum={design_dbnum} seeds={} → CATA 依赖 {} 行 / {} 库 / {} 笔事务，缺失 {}，{} ms（resolve {} ms · render {} ms · SQL {} KB · parse {} 次 / {} 成功 · 缓存 {} 命中 / {} 未命中 · 已落库跳过 {}）",
            report.seeds,
            report.elements,
            report.dbnums.len(),
            report.transactions,
            report.missing,
            report.elapsed_ms,
            report.resolve_ms,
            report.render_ms,
            report.sql_bytes / 1024,
            report.parse_requests,
            report.parsed,
            report.cache_hits,
            report.cache_misses,
            report.skipped_persisted,
        );
    }
    Ok(report)
}

/// 已解出的行 → （目录文件前移的先 Replace 刷新，D7）→ `Fresh` 入库 → 状态行。三个写入点
/// （经 [`persist_cata_dependencies`]）与惰性兜底（`cata_closure::ensure_cata_refnos_parsed`）共用：
/// 目录行落库只有这一个出口，`pe.dependency = true` 只在这里打。`report.seeds` / `elapsed_ms` 由调用方填。
pub async fn persist_resolved_rows(
    project: &str,
    rows: &CataDependencyRows,
) -> anyhow::Result<CataDependencyReport> {
    persist_resolved_rows_on(&SUL_DB, get_db_option(), project, rows).await
}

/// 同 [`persist_resolved_rows`]，目标库由调用方给（结构化 sink 在 backup lane 上落本窗的
/// CATA 依赖行走这里；语句与判据一行不差）。前移刷新建 locator 时目录根按 `option` 解析。
pub async fn persist_resolved_rows_on(
    db: &Surreal<Any>,
    option: &DbOption,
    project: &str,
    rows: &CataDependencyRows,
) -> anyhow::Result<CataDependencyReport> {
    let uda_keys = UdaKeyMap::load_on(db).await?;
    // D7：本批碰到的目录库，文件比已入库依赖行的版本新的，先把被改的依赖行 Replace 刷新——
    // 排在 Fresh 之前，否则新闭包里「已有行赢」会把旧版本留下。
    let refreshed = refresh_moved_catalogues_on(
        db,
        option,
        project,
        rows.by_dbnum.keys().copied(),
        &uda_keys,
    )
    .await?;
    let mut report = land_dependency_rows_on(db, rows, &uda_keys).await?;
    report.refreshed = refreshed;
    Ok(report)
}

/// `Fresh` 入库那一段：D6 摘掉已落库的 → 渲 → 执行 → 状态行 → 记忆。
/// 与 [`persist_resolved_rows_on`] 分开是为了让它在 `mem://` 上可测——前移刷新要一份真定位器
/// （读 `dbnum_watermark` 与目录文件），那是另一条路的判据。
async fn land_dependency_rows_on(
    db: &Surreal<Any>,
    rows: &CataDependencyRows,
    uda_keys: &UdaKeyMap,
) -> anyhow::Result<CataDependencyReport> {
    let render_started = std::time::Instant::now();
    // D6：这一版已经落过库的行不再渲、不再发（`Fresh` 本来也是 no-op，省的是渲染与字节）。
    let batches = render_dependency_transactions_unpersisted(rows, uda_keys, WriteMode::Fresh)?;
    let render_ms = render_started.elapsed().as_millis();
    let sql_bytes = batches
        .by_dbnum
        .iter()
        .flat_map(|(_, batch)| batch.transactions.iter())
        .map(String::len)
        .sum();
    let mut report = CataDependencyReport {
        elements: rows.elements(),
        dbnums: rows.by_dbnum.keys().copied().collect(),
        missing: rows.manifest.missing,
        missing_samples: rows.manifest.missing_samples.clone(),
        resolve_ms: rows.account.resolve_ms,
        render_ms,
        sql_bytes,
        parse_requests: rows.account.closure.parse_requests,
        parsed: rows.account.closure.parsed,
        cache_hits: rows.account.closure.cache_hits,
        cache_misses: rows.account.closure.cache_misses,
        skipped_persisted: batches.skipped,
        ..Default::default()
    };
    for (dbnum, batch) in &batches.by_dbnum {
        for sql in &batch.transactions {
            crate::data_interface::pe_batch::execute_on(db, sql)
                .await
                .with_context(|| format!("persist CATA dependency rows for dbnum {dbnum}"))?;
            report.transactions += 1;
        }
        db.query(render_dependency_state(
            *dbnum,
            batch.sesno,
            batch.refnos.len(),
        ))
        .await
        .and_then(|response| response.check())
        .with_context(|| format!("record CATA dependency state for dbnum {dbnum}"))?;
        remember_persisted_rows(*dbnum, batch);
    }
    Ok(report)
}

/// 任务范围内「这一版已经落过库的 seed」。
///
/// 模型根发布**每根**调一次 [`persist_for_design_subtree`]，而目录链是海量共享的——同一个
/// 规格、同一个元件，几千根管子都指着它。7997 那一轮实测：2,469 次调用重解重写了 387,132 行，
/// 而这个库整份闭包只有 15,500 行（数据阶段基线已经一次性落过），25 倍的重复占掉模型阶段
/// 628.7 s 里的 323.7 s。每一次调用还都重建一遍 `InMemoryCataLocator`。
///
/// 与 `model_db_adapter` 的对账基线同一条纪律（ADR-071 D1：任务是这个 dbnum 唯一的写者，
/// **它知道自己写了什么**）：作用域绑在数据窗口上（`active_data_window()`），换窗即失效；
/// 窗口不在（`/model/ensure` 那类一根一调的路）就不记，照旧每根解一次。
struct CataSeedMemo {
    window: (u32, i32),
    seeds: BTreeSet<RefU64>,
}

static CATA_SEED_MEMO: std::sync::Mutex<Option<CataSeedMemo>> = std::sync::Mutex::new(None);

fn cata_seed_memo() -> std::sync::MutexGuard<'static, Option<CataSeedMemo>> {
    CATA_SEED_MEMO
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// D6：进程级「这一行这一版已经落过库」的记忆：`(cata_dbnum, refno) → sesno`。
///
/// 与 [`CataSeedMemo`] 同一条纪律（ADR-071 D1：任务是这个库唯一的写者，**它知道自己写了什么**），
/// 但记的不是 seed 而是落库出口真写进去的目录行。`Fresh` = `INSERT IGNORE`，行已经在（任何 sesno）
/// 本来就是 no-op；memo 只是把这个 no-op 提前到进程里——省掉的是渲染、SQL 字节与事务，语义不变。
///
/// 它只可能错在「以为在、其实不在」这一种，三道收口：只在写成功之后记
/// （[`persist_resolved_rows_on`]）、目录版本前移与整库 wipe 两处作废
/// （[`forget_persisted_catalogue_rows`]）、进程重启自然消失。「别的进程动了这个库」本来就在
/// ADR-071 D1 之外。
struct PersistedDependencyMemo {
    rows: BTreeMap<(u32, RefU64), u32>,
}

static PERSISTED_DEPENDENCY_MEMO: std::sync::Mutex<PersistedDependencyMemo> =
    std::sync::Mutex::new(PersistedDependencyMemo {
        rows: BTreeMap::new(),
    });

fn persisted_dependency_memo() -> std::sync::MutexGuard<'static, PersistedDependencyMemo> {
    PERSISTED_DEPENDENCY_MEMO
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// [`forget_persisted_catalogue_rows`] 的范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForgetScope {
    /// 一个目录库：它的依赖行刚被 Replace / 墓碑动过。
    Catalogue(u32),
    /// 全部：整库清空重来，或测试要一个干净起点。
    Everything,
}

/// 库被动过之后调：记忆不再等于库。两份记忆在同一处作废——目录 seed 记忆
/// （[`CataSeedMemo`]，「这些 seed 的闭包解过了」）与已落库行记忆
/// （[`PersistedDependencyMemo`]，「这些行写过了」）记的都是「这一版已经办过了」。
pub(crate) fn forget_persisted_catalogue_rows(scope: ForgetScope) {
    match scope {
        ForgetScope::Catalogue(dbnum) => persisted_dependency_memo()
            .rows
            .retain(|(cata_dbnum, _), _| *cata_dbnum != dbnum),
        ForgetScope::Everything => persisted_dependency_memo().rows.clear(),
    }
    *cata_seed_memo() = None;
}

/// 记下这一库这一批真落进去的行。**只在这一库的事务全部成功之后调**：失败的行必须让下一次
/// 重发，否则这个进程剩下的写回都当它在库里，缺行要等到下一个进程才补得回来。
fn remember_persisted_rows(dbnum: u32, batch: &CatalogueBatch) {
    let mut memo = persisted_dependency_memo();
    for refno in &batch.refnos {
        memo.rows.insert((dbnum, *refno), batch.sesno);
    }
}

/// 本窗还没落过库的那些 seed。窗口不在就原样返回——不记忆的路每次都得自己解。
fn unseen_seeds(window: Option<(u32, i32)>, seeds: &BTreeSet<RefU64>) -> BTreeSet<RefU64> {
    let Some(window) = window else {
        return seeds.clone();
    };
    match cata_seed_memo()
        .as_ref()
        .filter(|memo| memo.window == window)
    {
        Some(memo) => seeds.difference(&memo.seeds).cloned().collect(),
        None => seeds.clone(),
    }
}

/// 记下这些 seed 已经落过库。**只在写成功之后调**：失败的 seed 必须让下一根重试，
/// 否则整窗都以为它写过了。换窗时整份换掉，不与上一窗混。
fn remember_seeds(window: Option<(u32, i32)>, seeds: &BTreeSet<RefU64>) {
    let Some(window) = window else {
        return;
    };
    let mut memo = cata_seed_memo();
    match memo.as_mut().filter(|memo| memo.window == window) {
        Some(memo) => memo.seeds.extend(seeds.iter().cloned()),
        None => {
            *memo = Some(CataSeedMemo {
                window,
                seeds: seeds.clone(),
            });
        }
    }
}

/// 模型根发布前的入口：根的子树名单（e3d-model DFS `visited`）→ 从文件读属性 → seed → 入库。
/// 只 warning 不拦发布（ADR-056 D1 的对偶：依赖块也不拦模型）。
///
/// 本窗已经落过的 seed 不再解第二遍闭包（见 [`CataSeedMemo`]）：整根的 seed 都是老面孔时
/// 连定位器都不建，直接回一份「这一根 N 个 seed，没有新的」。
pub async fn persist_for_design_subtree(
    project: &str,
    design_dbnum: u32,
    set: &std::sync::Arc<e3d_io::db_element::DbSet>,
    visited: &[e3d_io::refno::RefNo],
) -> anyhow::Result<CataDependencyReport> {
    let mut seeds = BTreeSet::new();
    for refno in visited {
        let refno = RefU64::from_two_nums(refno.word0, refno.word1);
        if let Some(element) = crate::data_interface::e3d_collector::read_ele_data(set, refno)? {
            seeds.extend(seeds_from_attmap(element.att_map()));
        }
    }
    let window = crate::data_interface::batch_worker::active_data_window();
    let fresh = unseen_seeds(window, &seeds);
    if fresh.is_empty() {
        return Ok(CataDependencyReport {
            seeds: seeds.len(),
            ..Default::default()
        });
    }
    let report = persist_cata_dependencies(project, design_dbnum, &fresh).await?;
    remember_seeds(window, &fresh);
    Ok(report)
}

/// 一个 CATA 库已入库的依赖行数（`/health` / 回执用）。
pub async fn dependency_rows_of(dbnum: u32) -> anyhow::Result<Option<(u32, u64)>> {
    dependency_rows_of_on(&SUL_DB, dbnum).await
}

/// 同 [`dependency_rows_of`]，目标库由调用方给。
pub async fn dependency_rows_of_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<Option<(u32, u64)>> {
    #[derive(serde::Deserialize)]
    struct Row {
        sesno: Option<u32>,
        rows: Option<u64>,
    }
    let mut response = db
        .query(format!(
            "SELECT sesno, rows FROM ONLY cata_dependency_state:{dbnum};"
        ))
        .await?
        .check()?;
    let row: Option<Row> = response.take(0)?;
    Ok(row.map(|row| (row.sesno.unwrap_or(0), row.rows.unwrap_or(0))))
}

/// 一次写回里 seed 的来源（回执文案）。
pub fn describe_seeds(seeds: &BTreeSet<RefU64>) -> String {
    let mut by_ref0: HashMap<u32, usize> = HashMap::new();
    for seed in seeds {
        *by_ref0.entry(seed.get_0()).or_default() += 1;
    }
    let mut parts = by_ref0.into_iter().collect::<Vec<_>>();
    parts.sort();
    parts
        .into_iter()
        .map(|(ref0, count)| format!("ref0 {ref0}: {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refu(ref0: u64, n: u64) -> RefU64 {
        RefU64((ref0 << 32) | n)
    }

    fn design_element(spre: RefU64, catr: RefU64, name: &str) -> EleData {
        let mut ele = EleData::default();
        ele.refno = refu(24384, 1);
        let map = &mut ele.whole_attmap.attmap.map;
        map.insert("REFNO".into(), NamedAttrValue::RefU64Type(ele.refno));
        map.insert("TYPE".into(), NamedAttrValue::StringType("BRAN".into()));
        map.insert("NAME".into(), NamedAttrValue::StringType(name.into()));
        map.insert("SPRE".into(), NamedAttrValue::RefU64Type(spre));
        map.insert("CATR".into(), NamedAttrValue::RefU64Type(catr));
        // 设计侧引用与非引用属性都不算 seed。
        map.insert("POSREF".into(), NamedAttrValue::RefU64Type(refu(24384, 9)));
        map.insert("HREF".into(), NamedAttrValue::RefU64Type(refu(24384, 8)));
        map.insert("OWNER".into(), NamedAttrValue::RefU64Type(refu(24384, 0)));
        ele
    }

    #[test]
    fn seeds_come_only_from_the_catalogue_chain_attributes() {
        let spre = refu(5052, 100);
        let catr = refu(5052, 200);
        let element = design_element(spre, catr, "/B1");
        let seeds = seeds_from_attmap(element.att_map());
        assert_eq!(seeds, BTreeSet::from([spre, catr]), "{seeds:?}");
        let all = seeds_from_elements([&element, &design_element(spre, refu(5052, 300), "/B2")]);
        assert_eq!(all.len(), 3);
    }

    /// 名单必须与 e3d-model 的护栏同源：`FILE_TABLE_REFERENCES` 里指向 CATA 的那一族 + `HSRO` / `LSRO`。
    #[test]
    fn catalogue_chain_attributes_match_e3d_model_guard() {
        let guard = include_str!("../../vendor/shared/e3d-model/tests/reference_reads_guard.rs");
        for name in CATALOGUE_CHAIN_ATTRIBUTES {
            assert!(
                guard.contains(&format!("\"{name}\"")),
                "{name} 不在 e3d-model reference_reads_guard 的名单里：两边名单必须同源"
            );
        }
        for design_side in ["POSREF", "STAREF", "ENDREF", "VXREF"] {
            assert!(
                !CATALOGUE_CHAIN_ATTRIBUTES.contains(&design_side),
                "{design_side} 指向设计侧 / 单元内，不该当 CATA seed"
            );
        }
    }

    #[test]
    fn element_from_attmap_keeps_identity_name_and_members() {
        let refno = refu(5052, 100);
        let owner = refu(5052, 10);
        let mut att = NamedAttrMap::default();
        att.map
            .insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        att.map
            .insert("OWNER".into(), NamedAttrValue::RefU64Type(owner));
        att.map
            .insert("TYPE".into(), NamedAttrValue::StringType("SPCO".into()));
        att.map
            .insert("NAME".into(), NamedAttrValue::StringType("/SPCO-1".into()));
        let children = vec![refu(5052, 101)];
        let element = element_from_attmap(refno, att, children.clone());
        assert_eq!(element.refno, refno);
        assert_eq!(element.owner, owner);
        assert_eq!(element.name, "/SPCO-1");
        assert_eq!(
            element.children.iter().copied().collect::<Vec<_>>(),
            children
        );
        assert_eq!(element.att_map().get_type(), "SPCO");
    }

    /// 闭包配置只有一份：三个写入点与惰性兜底都只收 CATA（`precise()` 自带的 DESI 在这里去掉——
    /// DESI 行只能由它自己的数据窗口写），差别只在有没有「本批 DESI 库」可排除。
    #[test]
    fn closure_config_collects_only_cata_for_every_entry() {
        let precise = CataClosureConfig::precise();
        assert!(
            precise.cata_db_types.contains("DESI"),
            "前提：precise() 本身会下探 DESI"
        );

        let window = closure_config(Some(24384));
        assert_eq!(
            window.cata_db_types,
            std::collections::HashSet::from(["CATA".to_string()])
        );
        assert!(window.excluded_dbnums.contains(&24384));

        let lazy = closure_config(None);
        assert_eq!(lazy.cata_db_types, window.cata_db_types);
        assert!(lazy.excluded_dbnums.is_empty());
        assert_eq!(
            lazy.container_subtree_nouns,
            precise.container_subtree_nouns
        );
    }

    /// 依赖行经同一个 `element_batch` 渲染，`Fresh` 只插、`pe.dependency = true`，按 CATA dbnum 各成一组事务。
    #[test]
    fn dependency_rows_render_as_fresh_batches_flagged_dependency_per_catalogue_dbnum() {
        let mut att = NamedAttrMap::default();
        let refno = refu(5052, 100);
        att.map
            .insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        att.map
            .insert("OWNER".into(), NamedAttrValue::RefU64Type(refu(5052, 10)));
        att.map
            .insert("TYPE".into(), NamedAttrValue::StringType("SPCO".into()));
        att.map
            .insert("NAME".into(), NamedAttrValue::StringType("/SPCO-1".into()));
        att.map
            .insert("DBNUM".into(), NamedAttrValue::IntegerType(5052));
        let mut rows = CataDependencyRows::default();
        rows.by_dbnum
            .entry(5052)
            .or_default()
            .push((184, element_from_attmap(refno, att, vec![refu(5052, 101)])));
        let rendered =
            render_dependency_transactions(&rows, &UdaKeyMap::empty(), WriteMode::Fresh).unwrap();
        assert_eq!(rendered.len(), 1);
        let (dbnum, sqls) = &rendered[0];
        assert_eq!(*dbnum, 5052);
        assert_eq!(sqls.len(), 1);
        let sql = &sqls[0];
        assert!(sql.contains("INSERT IGNORE INTO pe ["), "{sql}");
        assert!(sql.contains("\"dependency\": true"), "{sql}");
        assert!(sql.contains("INSERT IGNORE INTO SPCO ["), "{sql}");
        assert!(
            sql.contains("INSERT RELATION IGNORE INTO pe_owner ["),
            "{sql}"
        );
        assert!(!sql.contains("DELETE"), "Fresh 只插:\n{sql}");
        // Ref0 统计已退役（ADR-071 D6）：写事务里没有观测块。
        assert!(
            !sql.contains("dbnum_stats_state") && !sql.contains("dbnum_info_table"),
            "写事务不得再带逐笔统计观测:\n{sql}"
        );
    }

    /// 渲出来的事务真能在库上跑：Fresh 入库 + 状态行，重跑一次行数不翻倍（已有行赢）、状态行单调。
    #[tokio::test]
    async fn dependency_batches_execute_idempotently_on_mem() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("dep").use_db("fresh").await.unwrap();
        let mut rows = CataDependencyRows::default();
        for n in [100u64, 101] {
            let refno = refu(5052, n);
            let mut att = NamedAttrMap::default();
            att.map
                .insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
            att.map
                .insert("OWNER".into(), NamedAttrValue::RefU64Type(refu(5052, 10)));
            att.map
                .insert("TYPE".into(), NamedAttrValue::StringType("SCOM".into()));
            att.map.insert(
                "NAME".into(),
                NamedAttrValue::StringType(format!("/SCOM-{n}")),
            );
            att.map
                .insert("DBNUM".into(), NamedAttrValue::IntegerType(5052));
            rows.by_dbnum
                .entry(5052)
                .or_default()
                .push((184, element_from_attmap(refno, att, vec![])));
        }
        let batches =
            render_dependency_transactions(&rows, &UdaKeyMap::empty(), WriteMode::Fresh).unwrap();
        for _ in 0..2 {
            for (dbnum, sqls) in &batches {
                for sql in sqls {
                    crate::data_interface::pe_batch::execute_on(&db, sql)
                        .await
                        .unwrap();
                }
                db.query(render_dependency_state(*dbnum, 184, 2))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
            }
        }
        let mut response = db
            .query(
                "SELECT VALUE dependency FROM pe;\n\
                 SELECT VALUE NAME FROM SCOM;\n\
                 RETURN cata_dependency_state:5052.sesno;\n\
                 RETURN dbnum_info_table:5052.count;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        let flags: Vec<bool> = response.take(0).unwrap();
        assert_eq!(flags, vec![true, true], "依赖行都带 dependency = true");
        let mut names: Vec<String> = response.take(1).unwrap();
        names.sort();
        assert_eq!(
            names,
            vec!["/SCOM-100", "/SCOM-101"],
            "重跑不翻倍（已有行赢）"
        );
        assert_eq!(response.take::<Option<i64>>(2).unwrap(), Some(184));
        // Ref0 统计已退役（ADR-071 D6）：写事务不会把统计表写出来。
        assert_eq!(
            response.take::<Option<i64>>(3).unwrap(),
            None,
            "写事务不再动 dbnum_info_table（Ref0 统计已退役）"
        );
    }

    #[test]
    fn dependency_state_is_not_a_watermark_and_grows_monotonically() {
        let sql = render_dependency_state(5052, 184, 37);
        assert!(sql.starts_with("UPSERT cata_dependency_state:5052 SET"));
        assert!(sql.contains("sesno = math::max([sesno ?: 0, 184])"));
        assert!(sql.contains("rows = (rows ?: 0) + 37"));
        assert!(!sql.contains("dbnum_watermark"), "依赖状态不是水位");
    }

    /// 三个写入点都接了线（plan §3 D6）：数据窗口（排在 DESI 块之前，`increment_pipeline` 另有顺序钉）、
    /// 基线收尾（经同一条写者队列）、模型根发布前（独立、只 warning）。少一处就是「某条路写回的 DESI
    /// 没带它的目录链」。
    #[test]
    fn all_three_write_points_persist_catalogue_dependencies() {
        // 数据窗口（ADR-071 任务内同步序列化）：Task 解闭包（`resolve_dependency_rows`，失败仍
        // 拦水位）、行装进窗口产物，`land_increment_window` 用同一个 `persist_resolved_rows_on`
        // 在尾事务之前同步落库——写入点没少，只是收进了一个落库入口。
        let pipeline = include_str!("increment_pipeline.rs")
            .split_once("async fn apply_one(")
            .expect("apply_one 还在")
            .1;
        assert!(
            pipeline.contains("cata_dependency::resolve_dependency_rows_on("),
            "数据窗口 apply_one 必须在 Task 侧解 CATA 依赖闭包"
        );
        let sink = include_str!("window_landing/mod.rs");
        assert!(
            sink.contains("cata_dependency::persist_resolved_rows_on(")
                && sink.contains("window.cata_rows()"),
            "落库入口必须用同一个写入口把窗口产物里的 CATA 依赖行落库"
        );
        let baseline = include_str!("../versioned_db/baseline_ingest.rs");
        assert!(
            baseline.contains("cata_dependency::resolve_dependency_rows_on(")
                && baseline.contains("cata_dependency::render_dependency_transactions_unpersisted(")
                && baseline.contains(".send_async(WriteJob::new(")
                && baseline.contains("sql.clone(),"),
            "基线必须经同一条属性 lane 入库 CATA 依赖（单写者不变，ADR-077 D2）"
        );
        let publish = include_str!("model_refresh.rs");
        assert!(
            publish.contains("cata_dependency::persist_for_design_subtree("),
            "模型根发布前必须入库该根子树的 CATA 依赖"
        );
        let publish_body = publish
            .split_once("cata_dependency::persist_for_design_subtree(")
            .unwrap()
            .1;
        assert!(
            publish_body.contains("cata_dependency_degraded") && publish_body.contains("不拦发布"),
            "模型侧的依赖块失败只 warning、不拦发布"
        );
    }

    fn seed_set(raw: &[(u32, u32)]) -> BTreeSet<RefU64> {
        raw.iter()
            .map(|(word0, word1)| RefU64::from_two_nums(*word0, *word1))
            .collect()
    }

    /// 目录 seed 的记忆只覆盖**一个数据窗口**：同一窗里老 seed 不再解第二遍闭包，换一窗
    /// 必须从头来，没有窗口的那条路（`/model/ensure` 一根一调）一个都不记。
    ///
    /// 钉的是 2026-09-10 那次实测：7997 模型阶段 2,469 次逐根调用重解重写了 387,132 行，
    /// 而这个库整份闭包只有 15,500 行——323.7 s，占模型阶段一半。
    #[test]
    fn the_catalogue_seed_memo_is_scoped_to_one_data_window() {
        let _guard = memo_test_guard();
        forget_persisted_catalogue_rows(ForgetScope::Everything);
        let window = Some((7997, 160));
        let first = seed_set(&[(24381, 1), (24381, 2)]);

        assert_eq!(unseen_seeds(window, &first), first, "第一根：全都是新的");
        remember_seeds(window, &first);
        assert!(
            unseen_seeds(window, &first).is_empty(),
            "同一窗里的第二根：老面孔一个都不该再解"
        );
        assert_eq!(
            unseen_seeds(window, &seed_set(&[(24381, 2), (24381, 3)])),
            seed_set(&[(24381, 3)]),
            "混着新老 seed 时只解新的那些"
        );

        assert_eq!(
            unseen_seeds(Some((7997, 161)), &first),
            first,
            "换一版目标就是另一窗：记忆不跨窗"
        );
        assert_eq!(unseen_seeds(None, &first), first, "没有窗口就不吃记忆");
        remember_seeds(None, &first);
        assert_eq!(unseen_seeds(None, &first), first, "也记不进去");

        forget_persisted_catalogue_rows(ForgetScope::Everything);
        assert_eq!(
            unseen_seeds(window, &first),
            first,
            "作废之后回到「什么都不知道」"
        );
    }

    /// 记忆只在**写成功之后**落：`?` 早退的那一根不能把自己的 seed 记成写过了，否则
    /// 整窗剩下的根都会跳过它，缺行到下一个任务才补得回来。
    #[test]
    fn the_catalogue_seed_memo_records_only_after_a_successful_write() {
        let body = include_str!("cata_dependency.rs")
            .split_once("pub async fn persist_for_design_subtree(")
            .expect("模型根发布的入口还在")
            .1;
        let persisted = body
            .find("persist_cata_dependencies(project, design_dbnum, &fresh)")
            .expect("落库那一句还在");
        let remembered = body
            .find("remember_seeds(window, &fresh)")
            .expect("记忆那一句还在");
        assert!(
            persisted < remembered,
            "remember_seeds 必须排在落库之后：写失败的 seed 要让下一根重试"
        );
        assert!(
            body[..persisted].contains("if fresh.is_empty()"),
            "整根都是老 seed 时必须直接返回——不建定位器、不解闭包、不发事务"
        );
    }

    /// D6 的两份记忆都是进程级的：碰它们的单测串行跑，别互相清账。
    static MEMO_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn memo_test_guard() -> std::sync::MutexGuard<'static, ()> {
        MEMO_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 一个目录库的 `n` 个依赖行（同一 sesno——一次 resolve 里同库同版本）。
    fn catalogue_rows(dbnum: u32, sesno: u32, count: u64) -> CataDependencyRows {
        let mut rows = CataDependencyRows::default();
        for n in 0..count {
            let refno = refu(dbnum as u64, 100 + n);
            let mut att = NamedAttrMap::default();
            att.map
                .insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
            att.map
                .insert("OWNER".into(), NamedAttrValue::RefU64Type(refu(dbnum as u64, 10)));
            att.map
                .insert("TYPE".into(), NamedAttrValue::StringType("SCOM".into()));
            att.map.insert(
                "NAME".into(),
                NamedAttrValue::StringType(format!("/SCOM-{dbnum}-{n}")),
            );
            att.map
                .insert("DBNUM".into(), NamedAttrValue::IntegerType(dbnum as i32));
            rows.by_dbnum
                .entry(dbnum)
                .or_default()
                .push((sesno, element_from_attmap(refno, att, vec![])));
        }
        rows
    }

    async fn dependency_facts(db: &Surreal<Any>, dbnum: u32) -> (usize, i64) {
        let mut response = db
            .query(format!(
                "SELECT VALUE record::id(id) FROM pe WHERE dependency = true;\n\
                 RETURN cata_dependency_state:{dbnum}.rows;"
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        let ids: Vec<String> = response.take(0).unwrap();
        let state: Option<i64> = response.take(1).unwrap();
        (ids.len(), state.unwrap_or(0))
    }

    /// D6：同一份行连落两次，第二次一笔事务都不发——`Fresh` 是 `INSERT IGNORE`，行在库里
    /// 本来就是 no-op，memo 把这个 no-op 提前到进程里：省的是渲染、SQL 字节与事务往返。
    /// 数据面必须一动不动：`pe` 行数不变，`cata_dependency_state.rows` 也不再加一遍。
    #[tokio::test]
    async fn rows_that_are_already_in_the_database_are_not_sent_a_second_time() {
        let _guard = memo_test_guard();
        forget_persisted_catalogue_rows(ForgetScope::Everything);
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("dep").use_db("memo").await.unwrap();
        let uda_keys = UdaKeyMap::empty();
        let rows = catalogue_rows(5052, 184, 2);

        let first = land_dependency_rows_on(&db, &rows, &uda_keys).await.unwrap();
        assert_eq!(first.elements, 2);
        assert_eq!(first.skipped_persisted, 0, "第一次谁都不认识");
        assert_eq!(first.transactions, 1);
        assert!(first.sql_bytes > 0);
        assert_eq!(dependency_facts(&db, 5052).await, (2, 2));

        let second = land_dependency_rows_on(&db, &rows, &uda_keys).await.unwrap();
        assert_eq!(second.elements, 2, "闭包账不摘：这一批仍然依赖这 2 行");
        assert_eq!(second.skipped_persisted, 2);
        assert_eq!(second.transactions, 0, "一笔事务都不发");
        assert_eq!(second.sql_bytes, 0, "一个字节都不渲");
        assert_eq!(
            dependency_facts(&db, 5052).await,
            (2, 2),
            "数据面一动不动：行不翻倍、状态行的 rows 也不加第二遍"
        );

        // 目录文件前移 = 新 sesno：memo 认不出，整库照常重发。
        let moved = catalogue_rows(5052, 185, 2);
        let third = land_dependency_rows_on(&db, &moved, &uda_keys).await.unwrap();
        assert_eq!(third.skipped_persisted, 0, "换了 sesno 就不是同一版");
        assert_eq!(third.transactions, 1);
    }

    /// 记忆只在**这一库的事务全部成功之后**落：写炸了的那一库不能记成写过了，否则这个进程
    /// 剩下的写回都跳过它，缺行要等到下一个进程才补得回来。
    #[tokio::test]
    async fn the_persisted_memo_records_only_after_every_transaction_of_that_dbnum_succeeded() {
        let _guard = memo_test_guard();
        forget_persisted_catalogue_rows(ForgetScope::Everything);
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("dep").use_db("memo_fail").await.unwrap();
        let uda_keys = UdaKeyMap::empty();
        let rows = catalogue_rows(5052, 184, 2);

        // 让 `pe` 的 dependency 字段只收整数：依赖行带的是 `true`，整笔事务必炸。
        db.query("DEFINE FIELD dependency ON pe TYPE int;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let failed = land_dependency_rows_on(&db, &rows, &uda_keys).await;
        assert!(failed.is_err(), "写不进去必须上浮");

        db.query("REMOVE FIELD dependency ON pe;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let retried = land_dependency_rows_on(&db, &rows, &uda_keys).await.unwrap();
        assert_eq!(
            retried.skipped_persisted, 0,
            "上一次是失败的，这两行一个都不许被记成已落库"
        );
        assert_eq!(retried.transactions, 1);
        assert_eq!(dependency_facts(&db, 5052).await, (2, 2));

        // 源码钉：记忆那一句排在执行之后。
        let body = include_str!("cata_dependency.rs")
            .split_once("async fn land_dependency_rows_on(")
            .expect("落库那一段还在")
            .1;
        let executed = body.find("pe_batch::execute_on(db, sql)").expect("执行那一句");
        let remembered = body
            .find("remember_persisted_rows(*dbnum, batch)")
            .expect("记忆那一句");
        assert!(
            executed < remembered,
            "remember_persisted_rows 必须排在 execute_on 之后"
        );
    }

    /// 摘掉的是**渲染名单**，不是闭包账：`elements` / `manifest` 照旧报「这一批依赖多少行」。
    /// 一个库被摘光就整个不出现——不渲事务，也不发一条 `rows + 0` 的状态行。
    #[test]
    fn persisted_rows_are_dropped_from_rendering_but_not_from_the_closure_account() {
        let _guard = memo_test_guard();
        forget_persisted_catalogue_rows(ForgetScope::Everything);
        let mut rows = catalogue_rows(5052, 184, 2);
        for (dbnum, elements) in catalogue_rows(7002, 98, 1).by_dbnum {
            rows.by_dbnum.insert(dbnum, elements);
        }
        rows.manifest.missing = 3;

        let all = render_dependency_transactions_unpersisted(
            &rows,
            &UdaKeyMap::empty(),
            WriteMode::Fresh,
        )
        .unwrap();
        assert_eq!(all.by_dbnum.len(), 2);
        assert_eq!(all.skipped, 0);

        // 把 5052 那一库记成已落库。
        let (dbnum, batch) = &all.by_dbnum[0];
        assert_eq!(*dbnum, 5052);
        remember_persisted_rows(*dbnum, batch);

        let left = render_dependency_transactions_unpersisted(
            &rows,
            &UdaKeyMap::empty(),
            WriteMode::Fresh,
        )
        .unwrap();
        assert_eq!(left.skipped, 2);
        assert_eq!(
            left.by_dbnum.iter().map(|(dbnum, _)| *dbnum).collect::<Vec<_>>(),
            vec![7002],
            "摘光的库整个不出现：不渲事务，也不发状态行"
        );
        assert_eq!(rows.elements(), 3, "闭包账不摘");
        assert_eq!(rows.manifest.missing, 3, "缺失账也不摘");
    }

    /// 三处作废都接上了：目录版本前移刷新（那一库）、整库 wipe 重来（全部）、运维口整库删除
    /// `delete_dbnum_fast`（全部）。少一处，记忆就会在库已经变了之后继续说「写过了」。
    #[test]
    fn a_moved_catalogue_and_a_wipe_both_forget_the_memo() {
        let _guard = memo_test_guard();
        forget_persisted_catalogue_rows(ForgetScope::Everything);
        let rendered = |rows: &CataDependencyRows| {
            render_dependency_transactions_unpersisted(rows, &UdaKeyMap::empty(), WriteMode::Fresh)
                .unwrap()
        };
        let cata = catalogue_rows(5052, 184, 2);
        let neighbour = catalogue_rows(7002, 98, 1);
        remember_persisted_rows(5052, &rendered(&cata).by_dbnum[0].1);
        remember_persisted_rows(7002, &rendered(&neighbour).by_dbnum[0].1);

        forget_persisted_catalogue_rows(ForgetScope::Catalogue(5052));
        assert_eq!(rendered(&cata).skipped, 0, "前移的那一库整库忘掉");
        assert_eq!(rendered(&neighbour).skipped, 1, "邻库不受牵连");

        forget_persisted_catalogue_rows(ForgetScope::Everything);
        assert_eq!(rendered(&neighbour).skipped, 0, "wipe 之后谁都不认识");

        // 源码钉：两处真接上了。
        let refresh = include_str!("cata_dependency.rs")
            .split_once("pub async fn refresh_moved_catalogues_on(")
            .expect("前移刷新还在")
            .1
            .split_once("\npub struct CatalogueDependencyRefreshReport")
            .expect("前移刷新的边界")
            .0;
        assert!(
            refresh.contains("forget_persisted_catalogue_rows(ForgetScope::Catalogue(dbnum))"),
            "Replace / 墓碑之后必须忘掉那一库"
        );
        let wipe = include_str!("fast_delete.rs")
            .split_once("pub(crate) async fn wipe_dbnum_for_reinit(")
            .expect("wipe 入口还在")
            .1;
        assert!(
            wipe.contains("forget_persisted_catalogue_rows(")
                && wipe.contains("ForgetScope::Everything"),
            "清库之后必须整份忘掉"
        );
        // 运维口 `DELETE /dbnums/{dbnum}`：同样清行，同样得忘——删的若是目录库，别的任务会因
        // memo 跳过它的依赖行不发；投影里那个库的根也一并清掉。
        let delete = include_str!("fast_delete.rs")
            .split_once("pub async fn delete_dbnum_fast(")
            .expect("delete_dbnum_fast 还在")
            .1
            .split_once("pub(crate) async fn wipe_dbnum_for_reinit(")
            .expect("delete_dbnum_fast 排在 wipe 之前")
            .0;
        assert!(
            delete.contains("forget_persisted_catalogue_rows(")
                && delete.contains("ForgetScope::Everything"),
            "整库删除之后必须整份忘掉: {delete}"
        );
        assert!(
            delete.contains("projection_removal::clear_dbnum(dbnum,"),
            "整库删除之后投影里那个库也得清（走删除入口）: {delete}"
        );
    }

    /// 一笔依赖事务里同一个 refno 只出现一次（今天已成立——闭包 `visited` 去重在前）：
    /// 重复的 id 会让 `INSERT IGNORE` 的「已有行赢」在同一笔里自己跟自己较劲。
    #[test]
    fn a_dependency_transaction_never_lists_one_refno_twice() {
        let rows = catalogue_rows(5052, 184, 3);
        let rendered =
            render_dependency_transactions(&rows, &UdaKeyMap::empty(), WriteMode::Fresh).unwrap();
        for (_, sqls) in &rendered {
            for sql in sqls {
                for n in 0..3u64 {
                    let id = format!("pe:{}", refu(5052, 100 + n));
                    assert_eq!(
                        sql.matches(&format!("\"id\": {id}")).count()
                            + sql.matches(&format!("id: {id}")).count(),
                        1,
                        "{id} 在一笔事务里只该出现一次:\n{sql}"
                    );
                }
            }
        }
    }

    #[test]
    fn missing_closure_entries_are_reported_not_swallowed() {
        let report = CataDependencyReport {
            missing: 2,
            missing_samples: vec!["a".into(), "b".into()],
            ..Default::default()
        };
        assert!(report.warning(8000).unwrap().contains("2 个 refno"));
        assert!(CataDependencyReport::default().warning(8000).is_none());
    }

    /// D7：差分不可用退化成整库重渲时必须点名 `cata_dependency_full_refresh`；正常前移不出声。
    #[test]
    fn a_degraded_catalogue_refresh_is_named_in_the_warning() {
        let quiet = CataDependencyReport {
            refreshed: vec![CatalogueRefresh {
                dbnum: 5052,
                base_sesno: 184,
                target_sesno: 185,
                changed: 1,
                refreshed: 3,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(quiet.warning(8000).is_none(), "正常前移不是告警");
        let degraded = CataDependencyReport {
            refreshed: vec![CatalogueRefresh {
                dbnum: 5052,
                base_sesno: 100,
                target_sesno: 185,
                full_refresh: true,
                degraded_reason: Some("会话不在链上".into()),
                refreshed: 4000,
                ..Default::default()
            }],
            ..Default::default()
        };
        let warning = degraded.warning(8000).unwrap();
        assert!(
            warning.contains("cata_dependency_full_refresh"),
            "{warning}"
        );
        assert!(
            warning.contains("5052") && warning.contains("100→185"),
            "{warning}"
        );
    }

    /// 前移刷新走 `Replace`：依赖行原地替换、noun / UDA 行先删后插、`dependency` 标记保留。
    #[test]
    fn moved_catalogue_rows_are_rendered_with_replace_and_keep_the_flag() {
        let mut att = NamedAttrMap::default();
        let refno = refu(5052, 100);
        att.map
            .insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        att.map
            .insert("OWNER".into(), NamedAttrValue::RefU64Type(refu(5052, 10)));
        att.map
            .insert("TYPE".into(), NamedAttrValue::StringType("SCOM".into()));
        att.map
            .insert("DBNUM".into(), NamedAttrValue::IntegerType(5052));
        let mut rows = CataDependencyRows::default();
        rows.by_dbnum
            .entry(5052)
            .or_default()
            .push((185, element_from_attmap(refno, att, vec![])));
        let rendered =
            render_dependency_transactions(&rows, &UdaKeyMap::empty(), WriteMode::Replace).unwrap();
        let sql = &rendered[0].1[0];
        assert!(
            sql.contains("type::thing('SCOM', '5052_100')"),
            "旧 noun 行先删: {sql}"
        );
        assert!(
            sql.contains("ON DUPLICATE KEY UPDATE"),
            "pe 行原地替换: {sql}"
        );
        assert!(sql.contains("dependency = $input.dependency"), "{sql}");
        assert!(sql.contains("\"dependency\": true"), "{sql}");
    }

    /// A3：数据面前移的账——安静的一次（没库前移、没失败）横幅一个字不打；前移了就报总账并
    /// 点名退化库；失败逐库点名、说清那一库一行都没前移。
    #[test]
    fn a_quiet_dependency_refresh_says_nothing_and_a_moved_one_reports_totals_and_failures() {
        let quiet = CatalogueDependencyRefreshReport {
            candidates: 3,
            elapsed_ms: 2,
            ..Default::default()
        };
        assert!(quiet.is_quiet());
        assert!(
            quiet.banner_line().is_none(),
            "候选有、没前移、没失败：不出声"
        );

        let moved = CatalogueDependencyRefreshReport {
            candidates: 3,
            refreshed: vec![
                CatalogueRefresh {
                    dbnum: 5052,
                    base_sesno: 184,
                    target_sesno: 185,
                    changed: 4,
                    refreshed: 3,
                    tombstoned: 1,
                    ..Default::default()
                },
                CatalogueRefresh {
                    dbnum: 7000,
                    base_sesno: 100,
                    target_sesno: 263,
                    full_refresh: true,
                    degraded_reason: Some("会话不在链上".into()),
                    refreshed: 4000,
                    ..Default::default()
                },
            ],
            elapsed_ms: 40,
            ..Default::default()
        };
        let line = moved.banner_line().expect("前移了要出声");
        assert!(line.contains("3 个候选库"), "{line}");
        assert!(line.contains("[5052, 7000]"), "{line}");
        assert!(
            line.contains("刷新 4003 行") && line.contains("墓碑 1"),
            "{line}"
        );
        assert!(
            line.contains("[7000]") && line.contains("cata_dependency_full_refresh"),
            "退化库要点名: {line}"
        );

        let failed = CatalogueDependencyRefreshReport {
            candidates: 2,
            errors: vec!["7000: build CATA locator for dependency refresh: boom".into()],
            ..Default::default()
        };
        let line = failed.banner_line().expect("失败要出声");
        assert!(
            line.contains("1 个库没跑成") && line.contains("7000:") && line.contains("未知不是零"),
            "{line}"
        );
    }

    /// A3 的接线（源码钉）：目录版本核对排完根之后要做数据面那一半，而且两个出口
    /// （启动的 `StartupReconcile`、运行期的 `/health.catalogue_reconcile`）都带上这份账。
    /// 候选名单来自 `cata_dependency_state`，不是模型面的「过期」名单；逐库调用、一库失败不挡别的库。
    #[test]
    fn the_catalogue_reconcile_refreshes_dependency_rows_after_scheduling_roots_and_exposes_it() {
        // 目录版本核对自 ADR-071 S4 起住在 `model_coverage.rs`（排的是带根名单的 ModelOnly 任务，
        // 不再写 pending 行）；启动那个出口自 S5a 起住在 `startup_reconcile.rs`。
        // 「两个出口都带上」这条判据于是对两份源码一起找。
        let pending = include_str!("model_coverage.rs").replace("\r\n", "\n");
        let both = format!(
            "{pending}\n{}",
            include_str!("startup_reconcile.rs").replace("\r\n", "\n")
        );
        let reconcile = pending
            .split_once("async fn reconcile_catalogue_versions() -> CatalogueReconcile {")
            .expect("reconcile exists")
            .1;
        let reconcile = &reconcile[..reconcile.find("\n}\n").expect("函数体收尾")];
        let stamps = reconcile
            .find("reconcile_published_root_stamps().await")
            .expect("先做模型面那一半（点名 → 收窗 → 反查排工）");
        let refresh = reconcile
            .find("cata_dependency::refresh_dependency_rows_of_moved_catalogues(")
            .expect("再做数据面那一半");
        assert!(stamps < refresh, "排完根之后再前移依赖行");
        assert!(
            !reconcile.contains("stale") && !reconcile.contains("if "),
            "数据面那一半不看模型面的过期名单、不带条件——两半各自对文件: {reconcile}"
        );
        // 解构末尾的 `..` 是 GitHub #47 按目录文件分组时加的私有格（`unreadable_dbnums`），不上回执。
        for probe in [
            "dependency_refresh,\n            ..\n        } = reconcile;",
            "catalogue_dependency_refresh: dependency_refresh,",
            "dependency_refresh: catalogue_dependency_refresh,\n        ..\n    } = reconcile_catalogue_versions().await;",
            "coverage.reconcile.catalogue_dependency_refresh = catalogue_dependency_refresh;",
            "        catalogue_cascade,\n        catalogue_dependency_refresh,\n    };",
        ] {
            // 旧钉要求 `catalogue_cascade,` 的下一行就是 `catalogue_dependency_refresh,` 并且再下一行
            // 是 `};`。现在这两行仍然相邻（startup_reconcile.rs:648-649），但后面还有
            // `generator_stale_dbnums`，所以带 `};` 的整段对不上。判据改成两者都在、且
            // dependency_refresh 在 cascade 之后：要钉的是两个出口都带上这份账，不是它是不是最后一格。
            if probe == "        catalogue_cascade,\n        catalogue_dependency_refresh,\n    };" {
                let cascade = both
                    .find("        catalogue_cascade,\n")
                    .expect("运行期出口要带上目录级联那一格");
                let refresh = both
                    .find("        catalogue_dependency_refresh,\n")
                    .expect("两个出口都要带上这份账：找不到 catalogue_dependency_refresh");
                assert!(
                    cascade < refresh,
                    "依赖行账排在目录级联之后：cascade={cascade} refresh={refresh}"
                );
                continue;
            }
            assert!(
                both.contains(probe),
                "两个出口都要带上这份账：找不到 {probe:?}"
            );
        }
        assert!(
            both.contains("self.catalogue_dependency_refresh.banner_line()")
                && both.contains("catalogue_dependency_refresh.banner_line() {"),
            "运行期横幅与启动横幅都要给数据面那一半一行"
        );

        let this = include_str!("cata_dependency.rs").replace("\r\n", "\n");
        let body = this
            .split_once("pub async fn refresh_dependency_rows_of_moved_catalogues(")
            .expect("入口存在")
            .1;
        let body = &body[..body.find("\n}\n").expect("函数体收尾")];
        assert!(
            body.contains("dependency_state_dbnums_on(&SUL_DB).await"),
            "候选来自 cata_dependency_state"
        );
        assert!(
            body.contains("refresh_moved_catalogues(project, [dbnum], &uda_keys).await")
                && body.contains("Err(error) => report.errors.push("),
            "逐库调用，一库失败记账、不挡别的库: {body}"
        );
    }

    /// 文件扫描出来的定位器（对拍 oracle 用）：`ref0 → dbnum` 走 e3d-io 的 `RefNo::dbno` 公式，
    /// `dbnum → (类型, 文件)` 来自 e3d-model `DirectoryDbResolver` 的扫描表；没有 SurrealDB。
    struct FileLocator {
        project: String,
        dbs: HashMap<u32, (String, std::path::PathBuf)>,
    }

    impl CataDbLocator for FileLocator {
        fn dbnum_of_ref0(&self, ref0: u32) -> Option<u32> {
            let dbnum = e3d_io::refno::RefNo::new(ref0, 0).dbno();
            self.dbs.contains_key(&dbnum).then_some(dbnum)
        }

        fn db_type_of(&self, dbnum: u32) -> Option<String> {
            self.dbs.get(&dbnum).map(|(db_type, _)| db_type.clone())
        }

        fn file_of(&self, dbnum: u32) -> Option<(String, std::path::PathBuf)> {
            self.dbs
                .get(&dbnum)
                .map(|(_, path)| (self.project.clone(), path.clone()))
        }
    }

    /// D5 的对拍 oracle（计划 §3 D5、ADR-064 Q1）：e3d-model 一次生成**真读到**的目录元素
    /// （`GenerationOutcome::catalogue_visited`，记录点在 e3d-io 的读入口 `parsed` / `extraction`，
    /// 与调用点无关）必须全部落在本模块按「名单 + 闭包」算出的依赖闭包里：
    ///
    /// ```text
    /// catalogue_visited(root) ⊆ closure(seed(visited(root)))      unexplained = 0
    /// ```
    ///
    /// 闭包比实读**宽**允许（多备份几行），窄就是漏——一个 DESI 元素落库而它的目录链在库里
    /// 缺行是缺陷（d-29）。seed 与闭包走的是生产同一条路（`seeds_from_attmap` +
    /// `resolve_dependency_rows_with`），只有定位器换成文件扫描的。每根一个空的目录网格缓存，
    /// 让实读集是每根**完整**的（缓存命中的目录件不会再被读）。
    ///
    /// 只读文件：`AIOS_PROJAMS_GEOMETRY_FILE`（默认 ams8000_0001）+ `AIOS_E3D_TEMPLATE_DIR`
    /// （默认 `E:\reverse\e3d\shadow_e3d31_aps_all`）；目录库从设计库所在目录扫（+ 同一 Projects
    /// 根下的 `AvevaCatalogue\acp000`，存在才扫），本项目那份 `ams7000_0001` 显式钉——与 e3d-model
    /// `tests/catalogue_visited_real.rs` 同一口径。
    /// 真库夹具（两个 live 测试共用）：ams8000 设计库 + 扫目录建的定位器 + 树根名单。
    ///
    /// 只读文件：`AIOS_PROJAMS_GEOMETRY_FILE`（默认 ams8000_0001）+ `AIOS_E3D_TEMPLATE_DIR`
    /// （默认 `E:\reverse\e3d\shadow_e3d31_aps_all`）；目录库从设计库所在目录扫（+ 同一 Projects
    /// 根下的 `AvevaCatalogue\acp000`，存在才扫），本项目那份 `ams7000_0001` 显式钉——与 e3d-model
    /// `tests/catalogue_visited_real.rs` 同一口径。
    fn live_ams_fixture() -> (
        std::sync::Arc<e3d_io::db_element::DbSet>,
        FileLocator,
        u32,
        Vec<e3d_io::refno::RefNo>,
    ) {
        use std::path::{Path, PathBuf};
        use std::sync::Arc;

        use e3d_io::db_element::{DbFilePin, DbFileResolver, DbSet, template_file_for};
        use e3d_io::engine::{ReadOnlyEngine, ScanTier};
        use e3d_io::refno::RefNo;
        use e3d_model::db_discovery::DirectoryDbResolver;

        use crate::data_interface::direct_store::DirectSchema;

        let file = PathBuf::from(std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(
            |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into(),
        ));
        let schema = DirectSchema::open_from_env().expect("E3D template directory");
        let project_dir = file.parent().expect("设计库文件在某个目录里").to_path_buf();
        let project = project_dir
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "AvevaMarineSample".into());
        let mut dirs = vec![project_dir.clone()];
        if let Some(projects) = project_dir.parent().and_then(Path::parent) {
            let acp = projects.join("AvevaCatalogue").join("acp000");
            if acp.is_dir() {
                dirs.push(acp);
            }
        }
        let resolver = Arc::new(
            DirectoryDbResolver::scan(&dirs, schema.template_dir())
                .expect("扫库目录建 dbno → 文件表"),
        );
        println!("{}", resolver.summary());

        struct Shared(Arc<DirectoryDbResolver>);
        impl DbFileResolver for Shared {
            fn resolve(&self, dbno: u32) -> Option<DbFilePin> {
                self.0.resolve(dbno)
            }
        }
        let set = Arc::new(
            DbSet::with_attlib_file_and_resolver(
                schema.template_dir().join("attlib.dat"),
                Box::new(Shared(resolver.clone())),
            )
            .expect("attlib"),
        );
        let design_dbnum = set
            .add_db(DbFilePin {
                file: file.clone(),
                template: template_file_for(schema.template_dir(), "DESI").expect("DESI template"),
                db_type: Some("DESI".into()),
                sesno: None,
            })
            .expect("pin DESI at its latest session");
        // 库 7000 在 ams000 与 acp000 各有一份，扫目录的 resolver 按设计拒仲裁：本项目那份显式钉。
        let project_catalogue = project_dir.join("ams7000_0001");
        if project_catalogue.is_file() {
            set.add_db(DbFilePin {
                file: project_catalogue.clone(),
                template: template_file_for(schema.template_dir(), "CATA").expect("CATA template"),
                db_type: Some("CATA".into()),
                sesno: None,
            })
            .expect("register the project's own catalogue");
        }

        // 定位器：主文件（ext_no <= 1）；同号多份时本项目目录下那份赢，与上面钉的一致。
        let mut dbs: HashMap<u32, (String, PathBuf)> = HashMap::new();
        for db in resolver.discovered() {
            if db.ext_no > 1 {
                continue;
            }
            let in_project = db.file.starts_with(&project_dir);
            match dbs.get(&db.dbno) {
                Some((_, existing)) if existing.starts_with(&project_dir) && !in_project => {}
                _ => {
                    dbs.insert(db.dbno, (db.db_type.clone(), db.file.clone()));
                }
            }
        }
        if project_catalogue.is_file() {
            dbs.insert(7000, ("CATA".into(), project_catalogue.clone()));
        }
        dbs.insert(design_dbnum, ("DESI".into(), file.clone()));
        let locator = FileLocator { project, dbs };

        // 树根 = owner 指向自己、无效、或不在本库索引里的元素（与 e3d-model `gen_ams` 同一口径）。
        let roots: Vec<RefNo> = {
            let mut engine = ReadOnlyEngine::open(&file).expect("open the design file");
            let mut owners = Vec::new();
            let mut indexed = std::collections::HashSet::new();
            for scanned in engine
                .scan_elements(ScanTier::Header)
                .expect("scan the index")
            {
                let scanned = scanned.expect("index entry");
                indexed.insert(scanned.refno);
                owners.push((scanned.refno, scanned.owner));
            }
            owners
                .iter()
                .filter(|(refno, owner)| {
                    owner == refno || !owner.is_valid() || !indexed.contains(owner)
                })
                .map(|(refno, _)| *refno)
                .collect()
        };
        assert!(!roots.is_empty(), "{} 扫不出树根", file.display());
        (set, locator, design_dbnum, roots)
    }

    /// 一个生成根读到的目录 seed（与生产同一条路：visited → 文件读属性 → `seeds_from_attmap`）；
    /// 目录实读集为空的根回 `None`（那种根跟目录没关系）。
    fn live_seeds_of_root(
        set: &std::sync::Arc<e3d_io::db_element::DbSet>,
        root: e3d_io::refno::RefNo,
    ) -> Option<(
        BTreeSet<RefU64>,
        std::collections::HashSet<e3d_io::refno::RefNo>,
        usize,
    )> {
        use e3d_model::catalogue::CatalogueMeshCache;
        use e3d_model::pipeline::generate_subtree_with_cache;

        use crate::data_interface::e3d_collector::read_ele_data;

        let mut mesh_cache = CatalogueMeshCache::default();
        let outcome = generate_subtree_with_cache(set.element(root), &mut mesh_cache)
            .unwrap_or_else(|error| panic!("{root} 生成失败：{error}"));
        if outcome.catalogue_visited.is_empty() {
            return None;
        }
        let mut seeds = BTreeSet::new();
        for refno in &outcome.visited {
            let refu = RefU64::from_two_nums(refno.word0, refno.word1);
            if let Some(element) = read_ele_data(set, refu).expect("read design element") {
                seeds.extend(seeds_from_attmap(element.att_map()));
            }
        }
        Some((
            seeds,
            outcome.catalogue_visited.iter().copied().collect(),
            outcome.visited.len(),
        ))
    }

    #[tokio::test]
    #[ignore = "manual live: needs the AvevaMarineSample project files and the E3D template directory"]
    async fn live_ams8000_catalogue_reads_are_covered_by_the_dependency_closure() {
        use e3d_io::refno::RefNo;

        let (set, locator, design_dbnum, roots) = live_ams_fixture();

        let to_refu = |refno: RefNo| RefU64::from_two_nums(refno.word0, refno.word1);
        let mut total_visited = 0usize;
        let mut total_catalogue = 0usize;
        let mut total_closure = 0usize;
        let mut total_missing = 0usize;
        let mut unexplained: Vec<(RefNo, String)> = Vec::new();
        for root in &roots {
            // 生产的 seed 路：模型根发布前 `persist_for_design_subtree` 就是 visited → 文件读属性 → seed。
            let Some((seeds, catalogue_visited, visited)) = live_seeds_of_root(&set, *root) else {
                continue;
            };
            let rows = resolve_dependency_rows_with(&locator, design_dbnum, &seeds)
                .await
                .expect("resolve the dependency closure");
            let closure: std::collections::HashSet<RefU64> = rows
                .manifest
                .by_dbnum
                .values()
                .flat_map(|refnos| refnos.iter().copied())
                .collect();
            total_visited += visited;
            total_catalogue += catalogue_visited.len();
            total_closure += closure.len();
            total_missing += rows.manifest.missing;
            println!(
                "root {root}: visited {visited} · seeds {} · catalogue_visited {} · closure {} rows / {} dbnums · missing {}",
                seeds.len(),
                catalogue_visited.len(),
                closure.len(),
                rows.by_dbnum.len(),
                rows.manifest.missing
            );
            for refno in &catalogue_visited {
                if !closure.contains(&to_refu(*refno)) {
                    let noun = set
                        .element(*refno)
                        .element_type()
                        .unwrap_or_else(|_| "?".into());
                    unexplained.push((*refno, noun));
                }
            }
        }
        println!(
            "total: visited {total_visited} · catalogue_visited {total_catalogue} · closure {total_closure} · closure missing {total_missing} · unexplained {}",
            unexplained.len()
        );
        assert!(
            total_catalogue > 0,
            "ams8000 有几百条管线，目录实读集不可能为空"
        );

        let mut by_noun: BTreeMap<String, usize> = BTreeMap::new();
        for (_, noun) in &unexplained {
            *by_noun.entry(noun.clone()).or_insert(0) += 1;
        }
        let samples: Vec<String> = unexplained
            .iter()
            .take(24)
            .map(|(refno, noun)| format!("{refno} ({noun})"))
            .collect();
        assert!(
            unexplained.is_empty(),
            "{} 个 e3d-model 真读到的目录元素不在依赖闭包里（闭包漏了；按 noun：{by_noun:?}）：{samples:?}",
            unexplained.len()
        );
    }

    /// S2（plan 2026-09-15 D4）真库钉：同一批 seeds 连解两次，第二次一次 `parse_element` 都不发、
    /// 一个会话都不开，闭包逐位相同。
    ///
    /// 单测那一条用的是桩身份；这里要验的正是桩证不了的那一步——真文件的指纹（size:mtime）与
    /// 目录会话号两次取值相等，缓存条目才认得出这是同一版文件。取不到身份（或对不上）时缓存
    /// 全程静默失效，命中率会掉到 0 而结果照样正确，只有真库跑一次才看得见。
    #[tokio::test]
    #[ignore = "manual live: needs the AvevaMarineSample project files and the E3D template directory"]
    async fn live_a_second_resolve_of_the_same_seeds_comes_from_the_closure_cache() {
        let (set, locator, design_dbnum, roots) = live_ams_fixture();
        let (seeds, _, _) = roots
            .iter()
            .find_map(|root| live_seeds_of_root(&set, *root))
            .expect("ams8000 至少有一个根读到目录件");

        let first = resolve_dependency_rows_with(&locator, design_dbnum, &seeds)
            .await
            .expect("first resolve");
        let second = resolve_dependency_rows_with(&locator, design_dbnum, &seeds)
            .await
            .expect("second resolve");

        assert_eq!(
            first.manifest.by_dbnum, second.manifest.by_dbnum,
            "两次闭包必须逐位相同"
        );
        assert_eq!(first.elements(), second.elements(), "装出来的行数也要一样");
        assert!(
            first.account.closure.cache_hits < first.account.closure.cache_misses,
            "第一次是冷的：{:?}",
            first.account.closure
        );
        let second_account = &second.account.closure;
        assert_eq!(
            second_account.cache_hits,
            first
                .manifest
                .by_dbnum
                .values()
                .map(BTreeSet::len)
                .sum::<usize>(),
            "闭包里每个元素都该是命中: {second_account:?}"
        );
        assert_eq!(
            second_account.parsed, 0,
            "第二次不该有任何元素是从文件里解出来的: {second_account:?}"
        );
        // 剩下的重来一遍是**库里根本没有**的那几个（`Ok(None)`）：按设计不进缓存，
        // 否则就是把「此刻查不到」记成了「永远没有」。它们的数量收在第一次的缺失数以内。
        assert_eq!(
            second_account.parse_requests, second_account.cache_misses,
            "未命中的才会去发请求: {second_account:?}"
        );
        assert!(
            second_account.cache_misses <= first.manifest.missing,
            "第二次的未命中只许是那几个库里没有的: {second_account:?} vs 第一次缺失 {}",
            first.manifest.missing
        );
        assert_eq!(
            second.manifest.missing, first.manifest.missing,
            "缺失数两次一样"
        );
        println!(
            "冷 {} 命中 / {} 未命中 · {} ms；热 {} 命中 / {} 未命中 · {} ms",
            first.account.closure.cache_hits,
            first.account.closure.cache_misses,
            first.account.resolve_ms,
            second_account.cache_hits,
            second_account.cache_misses,
            second.account.resolve_ms,
        );
    }

    /// 前移刷新的候选名单从调用方给的库读：升序去重，缺 `dbnum` 的行跳过。
    #[tokio::test]
    async fn dependency_state_candidates_come_from_the_given_database() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("dep").use_db("state_candidates").await.unwrap();
        db.query(
            "CREATE cata_dependency_state:7002 SET dbnum = 7002, sesno = 5; \
             CREATE cata_dependency_state:again SET dbnum = 7002, sesno = 6; \
             CREATE cata_dependency_state:7001 SET dbnum = 7001, sesno = 3; \
             CREATE cata_dependency_state:orphan SET sesno = 1;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        assert_eq!(
            dependency_state_dbnums_on(&db).await.unwrap(),
            vec![7001, 7002]
        );
    }

    /// 闭包定位读调用方给的库：任务库里一个文件都没登记，seed 全记 missing，而不是去读本进程
    /// 的库（单测里它没连，读了就是错误）。
    #[test]
    fn dependency_rows_resolve_against_the_given_database() {
        use crate::data_interface::watch_scope;
        // 建 locator 的收尾会 flush_on 进程级解析失败账本，和 parse_error 的测试共用一把锁。
        crate::data_interface::parse_error::with_ledger_locked(async {
            // 监听限定域是进程级的，跟着 DbOption.toml 的 watch_dbnums 走；开着时依赖清单为空，
            // build_for_project_on 直接报错。这条测的是读哪个库，先压成空表，收尾还原成未解析。
            let _watch = watch_scope::test_guard();
            watch_scope::set_dbnums_for_tests(Vec::new(), watch_scope::Origin::Config);
            let db = surrealdb::engine::any::connect("mem://").await.unwrap();
            db.use_ns("dep").use_db("rows_on").await.unwrap();
            // 空搜索根：目录兜底不碰任何真实项目目录。
            let mut option = DbOption::default();
            option.project_path = String::new();
            option.project_name = format!("Q5DepRowsOn{}", std::process::id());
            option.mdb_name = format!("Q5DEPROWSON-{}", std::process::id());
            option.project_dirs = None;
            let seeds = BTreeSet::from([refu(7600, 1), refu(7600, 2)]);

            let rows = resolve_dependency_rows_on(&db, &option, &option.project_name, 1112, &seeds)
                .await
                .expect("空任务库解出来是全 missing，不是错误");
            assert_eq!(rows.manifest.missing, seeds.len());
            assert!(rows.manifest.by_dbnum.is_empty());
            watch_scope::clear_for_tests();
        });
    }

    /// 前移刷新的 locator 建在调用方给的库上、不进进程级缓存。建在本进程库上的话，单测里没连就
    /// 报错，被别的测试连上了就按工程名写进缓存——两种都过不了这条。
    #[test]
    fn refreshing_moved_catalogues_builds_its_locator_on_the_given_database() {
        use crate::data_interface::{cata_closure, watch_scope};
        // 账本锁与监听限定域的处理同上一条：建 locator 要 flush 账本，watch_dbnums 开着时会直接报错。
        crate::data_interface::parse_error::with_ledger_locked(async {
            let _watch = watch_scope::test_guard();
            watch_scope::set_dbnums_for_tests(Vec::new(), watch_scope::Origin::Config);
            let db = surrealdb::engine::any::connect("mem://").await.unwrap();
            db.use_ns("dep").use_db("refresh_on").await.unwrap();
            let mut option = DbOption::default();
            option.project_path = String::new();
            option.project_name = format!("Q5DepRefreshOn{}", std::process::id());
            option.mdb_name = format!("Q5DEPREFRESHON-{}", std::process::id());
            option.project_dirs = None;

            let refreshed = refresh_moved_catalogues_on(
                &db,
                &option,
                &option.project_name,
                [7600],
                &UdaKeyMap::empty(),
            )
            .await
            .expect("空任务库上没入过依赖行，什么都不刷，不是错误");
            assert!(refreshed.is_empty());
            assert!(
                !cata_closure::project_locator_cached(&option.project_name).await,
                "任务库上建的 locator 不得写进进程级缓存"
            );
            watch_scope::clear_for_tests();
        });
    }
}
