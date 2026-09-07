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

use aios_core::{NamedAttrMap, NamedAttrValue, RefU64, SUL_DB};
use anyhow::Context;

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
    if seeds.is_empty() {
        return Ok(CataDependencyRows::default());
    }
    let locator = InMemoryCataLocator::build_for_project(project)
        .await
        .context("build CATA locator for dependency closure")?;
    resolve_dependency_rows_with(&locator, design_dbnum, seeds).await
}

/// 同 [`resolve_dependency_rows`]，定位器由调用方给：闭包路径只有这一条，对拍 oracle
/// （`live_ams8000_catalogue_reads_are_covered_by_the_dependency_closure`）用文件扫描出来的
/// 定位器走**同一条**路径，而不是另抄一份闭包。
pub async fn resolve_dependency_rows_with<L: CataDbLocator>(
    locator: &L,
    design_dbnum: u32,
    seeds: &BTreeSet<RefU64>,
) -> anyhow::Result<CataDependencyRows> {
    resolve_rows_with_config(locator, closure_config(Some(design_dbnum)), seeds).await
}

/// 惰性兜底（`cata_closure::ensure_cata_refnos_parsed`）走的**同一条**闭包路：seed 已经是目录侧 refno，
/// 没有「本批 DESI 库」可排除；db_type 照样只收 CATA——这条路以前拿 `precise()` 原样跑，会把回指到
/// 别的 DESI 库的元素也解出来写进去，而 DESI 行只能由它自己的数据窗口写（ADR-064 Q2）。
pub async fn resolve_catalogue_rows_with<L: CataDbLocator>(
    locator: &L,
    seeds: &BTreeSet<RefU64>,
) -> anyhow::Result<CataDependencyRows> {
    resolve_rows_with_config(locator, closure_config(None), seeds).await
}

async fn resolve_rows_with_config<L: CataDbLocator>(
    locator: &L,
    config: CataClosureConfig,
    seeds: &BTreeSet<RefU64>,
) -> anyhow::Result<CataDependencyRows> {
    if seeds.is_empty() {
        return Ok(CataDependencyRows::default());
    }
    let mut resolver = CataClosureResolver::new(locator, config).with_retain_attmaps(true);
    resolver.seed(seeds.iter().copied());
    let manifest = resolver
        .resolve()
        .await
        .context("resolve CATA dependency closure")?;
    let mut attmaps = resolver.take_attmaps();

    let mut by_dbnum: BTreeMap<u32, Vec<(u32, EleData)>> = BTreeMap::new();
    for (dbnum, refnos) in &manifest.by_dbnum {
        let sesno = catalogue_sesno(locator, *dbnum);
        let rows = by_dbnum.entry(*dbnum).or_default();
        for refno in refnos {
            let Some((att, children)) = attmaps.remove(refno) else {
                continue;
            };
            rows.push((sesno, element_from_attmap(*refno, att, children)));
        }
    }
    Ok(CataDependencyRows { by_dbnum, manifest })
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
    let dbnums = dbnums.into_iter().collect::<BTreeSet<_>>();
    if dbnums.is_empty() {
        return Ok(Vec::new());
    }
    let locator = InMemoryCataLocator::build_for_project(project)
        .await
        .context("build CATA locator for dependency refresh")?;
    let mut out = Vec::new();
    for dbnum in dbnums {
        let Some((base_sesno, _rows)) = dependency_rows_of(dbnum).await? else {
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
                (all_dependency_refnos(dbnum).await?, Vec::new())
            }
            Err(error) => {
                refresh.full_refresh = true;
                refresh.degraded_reason = Some(format!("{error:#}"));
                (all_dependency_refnos(dbnum).await?, Vec::new())
            }
        };
        // 只刷已入库的依赖行：与 `pe … dependency = true` 求交（点查，不扫表）。
        let present = dependency_rows_among(&targets).await?;
        let gone = dependency_rows_among(&deleted).await?;
        if !present.is_empty() {
            let seeds = present.iter().copied().collect::<BTreeSet<_>>();
            let mut resolver = CataClosureResolver::new(&locator, {
                let mut cfg = CataClosureConfig::precise();
                cfg.cata_db_types = ["CATA".to_string()].into_iter().collect();
                cfg
            })
            .with_retain_attmaps(true);
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
                    crate::data_interface::dbnum_stats::execute_on(&SUL_DB, &sql)
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
                crate::data_interface::dbnum_stats::execute_on(&SUL_DB, &sql)
                    .await
                    .with_context(|| {
                        format!("tombstone deleted catalogue rows of dbnum {dbnum}")
                    })?;
            }
            refresh.tombstoned = gone.len();
        }
        SUL_DB
            .query(render_dependency_state(dbnum, target_sesno, 0))
            .await
            .and_then(|response| response.check())
            .with_context(|| format!("advance CATA dependency state of dbnum {dbnum}"))?;
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

/// 这些 refno 里哪些是已入库的依赖行（`pe … dependency = true`）：点查，不扫表。
async fn dependency_rows_among(refnos: &[RefU64]) -> anyhow::Result<Vec<RefU64>> {
    let mut out = Vec::new();
    for chunk in refnos.chunks(200) {
        let keys = chunk
            .iter()
            .map(|refno| format!("pe:{refno}"))
            .collect::<Vec<_>>()
            .join(",");
        let mut response = SUL_DB
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
async fn all_dependency_refnos(dbnum: u32) -> anyhow::Result<Vec<RefU64>> {
    let mut response = SUL_DB
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
        let items = elements
            .iter()
            .map(|(sesno, element)| BatchItem::Rows {
                rows: ElementRows::render(
                    element,
                    &element.refno.to_string(),
                    *dbnum as i32,
                    *sesno,
                    uda_keys,
                ),
                is_add: false,
                old_noun: None,
                dependency: true,
            })
            .collect::<Vec<_>>();
        let (transactions, _) = render_item_transactions(&items, *dbnum as i32, mode)?;
        out.push((*dbnum, transactions));
    }
    Ok(out)
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
            "[cata_dependency] dbnum={design_dbnum} seeds={} → CATA 依赖 {} 行 / {} 库 / {} 笔事务，缺失 {}，{} ms",
            report.seeds,
            report.elements,
            report.dbnums.len(),
            report.transactions,
            report.missing,
            report.elapsed_ms
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
    let uda_keys = UdaKeyMap::load().await?;
    // D7：本批碰到的目录库，文件比已入库依赖行的版本新的，先把被改的依赖行 Replace 刷新——
    // 排在 Fresh 之前，否则新闭包里「已有行赢」会把旧版本留下。
    let refreshed =
        refresh_moved_catalogues(project, rows.by_dbnum.keys().copied(), &uda_keys).await?;
    let transactions = render_dependency_transactions(rows, &uda_keys, WriteMode::Fresh)?;
    let mut report = CataDependencyReport {
        elements: rows.elements(),
        dbnums: rows.by_dbnum.keys().copied().collect(),
        missing: rows.manifest.missing,
        missing_samples: rows.manifest.missing_samples.clone(),
        refreshed,
        ..Default::default()
    };
    for (dbnum, sqls) in &transactions {
        for sql in sqls {
            crate::data_interface::dbnum_stats::execute_on(&SUL_DB, sql)
                .await
                .with_context(|| format!("persist CATA dependency rows for dbnum {dbnum}"))?;
            report.transactions += 1;
        }
        let sesno = rows
            .by_dbnum
            .get(dbnum)
            .and_then(|elements| elements.first())
            .map(|(sesno, _)| *sesno)
            .unwrap_or(0);
        let elements = rows.by_dbnum.get(dbnum).map(Vec::len).unwrap_or(0);
        SUL_DB
            .query(render_dependency_state(*dbnum, sesno, elements))
            .await
            .and_then(|response| response.check())
            .with_context(|| format!("record CATA dependency state for dbnum {dbnum}"))?;
    }
    Ok(report)
}

/// 模型根发布前的入口：根的子树名单（e3d-model DFS `visited`）→ 从文件读属性 → seed → 入库。
/// 只 warning 不拦发布（ADR-056 D1 的对偶：依赖块也不拦模型）。
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
    persist_cata_dependencies(project, design_dbnum, &seeds).await
}

/// 一个 CATA 库已入库的依赖行数（`/health` / 回执用）。
pub async fn dependency_rows_of(dbnum: u32) -> anyhow::Result<Option<(u32, u64)>> {
    #[derive(serde::Deserialize)]
    struct Row {
        sesno: Option<u32>,
        rows: Option<u64>,
    }
    let mut response = SUL_DB
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
        let guard = include_str!("../../../vendor/e3d-model/tests/reference_reads_guard.rs");
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
        assert!(
            sql.contains("dbnum_stats_state:5052"),
            "pe 统计按 CATA 库记账:\n{sql}"
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
                    crate::data_interface::dbnum_stats::execute_on(&db, sql)
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
        assert_eq!(
            response.take::<Option<i64>>(3).unwrap(),
            Some(2),
            "pe 统计按 CATA 库记账"
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
        let pipeline = include_str!("increment_pipeline.rs");
        assert!(
            pipeline.contains("cata_dependency::persist_cata_dependencies("),
            "数据窗口 apply_one 必须入库 CATA 依赖"
        );
        let baseline = include_str!("../versioned_db/database.rs");
        assert!(
            baseline.contains("cata_dependency::resolve_dependency_rows(")
                && baseline.contains("cata_dependency::render_dependency_transactions(")
                && baseline.contains("SenderJsonsData::ElementBatch(sql.clone())"),
            "基线必须经同一条写者队列入库 CATA 依赖（单写者不变）"
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
    #[tokio::test]
    #[ignore = "manual live: needs the AvevaMarineSample project files and the E3D template directory"]
    async fn live_ams8000_catalogue_reads_are_covered_by_the_dependency_closure() {
        use std::path::{Path, PathBuf};
        use std::sync::Arc;

        use e3d_io::db_element::{DbFilePin, DbFileResolver, DbSet, template_file_for};
        use e3d_io::engine::{ReadOnlyEngine, ScanTier};
        use e3d_io::refno::RefNo;
        use e3d_model::catalogue::CatalogueMeshCache;
        use e3d_model::db_discovery::DirectoryDbResolver;
        use e3d_model::pipeline::generate_subtree_with_cache;

        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::e3d_collector::read_ele_data;

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

        let to_refu = |refno: RefNo| RefU64::from_two_nums(refno.word0, refno.word1);
        let mut total_visited = 0usize;
        let mut total_catalogue = 0usize;
        let mut total_closure = 0usize;
        let mut total_missing = 0usize;
        let mut unexplained: Vec<(RefNo, String)> = Vec::new();
        for root in &roots {
            let mut cache = CatalogueMeshCache::default();
            let outcome = generate_subtree_with_cache(set.element(*root), &mut cache)
                .unwrap_or_else(|error| panic!("{root} 生成失败：{error}"));
            if outcome.catalogue_visited.is_empty() {
                continue;
            }
            // 生产的 seed 路：模型根发布前 `persist_for_design_subtree` 就是 visited → 文件读属性 → seed。
            let mut seeds = BTreeSet::new();
            for refno in &outcome.visited {
                if let Some(element) =
                    read_ele_data(&set, to_refu(*refno)).expect("read design element")
                {
                    seeds.extend(seeds_from_attmap(element.att_map()));
                }
            }
            let rows = resolve_dependency_rows_with(&locator, design_dbnum, &seeds)
                .await
                .expect("resolve the dependency closure");
            let closure: std::collections::HashSet<RefU64> = rows
                .manifest
                .by_dbnum
                .values()
                .flat_map(|refnos| refnos.iter().copied())
                .collect();
            total_visited += outcome.visited.len();
            total_catalogue += outcome.catalogue_visited.len();
            total_closure += closure.len();
            total_missing += rows.manifest.missing;
            println!(
                "root {root}: visited {} · seeds {} · catalogue_visited {} · closure {} rows / {} dbnums · missing {}",
                outcome.visited.len(),
                seeds.len(),
                outcome.catalogue_visited.len(),
                closure.len(),
                rows.by_dbnum.len(),
                rows.manifest.missing
            );
            for refno in &outcome.catalogue_visited {
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
}
