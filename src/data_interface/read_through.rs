//! 数据面读透（Read-through）：把一批元素在**钉住会话**上的数据面终态写进库。
//!
//! 出处：`docs/plans/2026-09-05-kv-mem-zero-ingest-read-through-plan.md` §3.2 / §3.3。
//! 读透形态（`options::data_face() == ReadThrough`）下不做基线、不开数据窗口，`pe` /
//! `ATT_<noun>` / `ATT_UDA` / `pe_owner` 行有两个来源，都走这里：
//!
//! * **写透**：e3d-model 生成一根时 DFS 已经把子树每个元素读过一遍
//!   （`GenerationOutcome::visited`），生成端把这份名单 ∪ 根的祖先链交进来；
//! * **按需**：`POST /api/v1/element/ensure` 把一个元素 ∪ 祖先链 ∪ 若干层后代交进来。
//!
//! 三条纪律：
//!
//! 1. **行形状只有一份**（宪法 II，计划不变量 R2）：渲染走 [`ElementRows::render`]，与
//!    基线（`versioned_db::baseline`）、增量（`increment_pipeline`）同一个渲染器；读取走
//!    [`read_ele_data`]，与生产收集器同一条读法。`pe.sesno` 仍是元素 **last-touch** 会话
//!    （终稿记录所在页反查，[`PageSessions`]），不许拿生成时点顶替——那会让同一张表里
//!    出现两种 `sesno` 含义。
//! 2. **动词 `UPSERT`**：缓存刷新是幂等的，同一元素再读一次就是刷新；元素**没了**由窗口
//!    软删（`pe.deleted = true`），不在这里做根范围先删后插。
//! 3. **只写本库**（ADR-058 S3，不变量 R4）：`RefNo::dbno()` 不等于 `source.dbnum` 的一律
//!    不写、计入 `foreign`——CATA 元素永不进 `pe` / `ATT_*`，别的 DESI 库的元素归它自己的
//!    那次写透。
//!
//! 它是缓存，不是前置（不变量 R3）：调用方拿到 `Err` 只许记警告，不许拦模型发布。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use aios_core::{RefU64, SUL_DB};
use dashmap::DashMap;
use e3d_io::db_element::DbSet;
use e3d_io::index::IndexCursor;
use e3d_io::refno::RefNo;
use e3d_io::session::{DbView, SessionSelector};
use sha2::{Digest, Sha256};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use super::db_file_header::session_chain;
use super::dbnum_stats;
use super::e3d_collector::{PageSessions, read_ele_data, read_ele_data_with_uda_catalog};
use super::ele_ops::{ElementRows, UdaKeyMap, UdaRowPolicy};

/// 一次读透读的是哪个 DESI 库的哪一版文件。
///
/// `set` 里那个库钉在 `sesno` 上；`file` 是同一个文件——last-touch 会话要从它的会话链
/// 与目标会话索引反查，`DbSet` 不露这两样。
#[derive(Debug, Clone)]
pub struct ReadThroughSource {
    pub dbnum: u32,
    pub file: PathBuf,
    pub sesno: u32,
}

/// 一次读透的回执。随 `E3dPersistReport` / `element/ensure` 回执透出。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ReadThroughReceipt {
    pub dbnum: u32,
    pub sesno: u32,
    /// 去重后要写的元素数（含被判 `foreign` / `missing` 的）。
    pub requested: usize,
    /// 真正渲染并落库的元素数。
    pub written: usize,
    pub skipped_stale: usize,
    pub cache_epoch: u64,
    /// 不属于 `dbnum` 的 refno（CATA / 别的 DESI），一律不写。
    pub foreign: usize,
    /// 目标会话索引里没有的 refno（PDMS 记法）。名单来自 DFS 或树，索引里却没有，
    /// 多半是文件在生成与写透之间前进了——不静默。
    pub missing: Vec<String>,
    /// 提交的语句块数（一个元素一块）。
    pub statements: usize,
    pub elapsed_ms: u128,
    /// UDA 键查不到、单个元素读不出来（schema 外的 noun 等）——都不拦，都要说。
    pub warnings: Vec<String>,
}

/// 渲染半边的产物：一个元素的终态行 + 它的 last-touch 会话。
pub struct RenderedElement {
    pub refno: RefU64,
    pub sesno: u32,
    pub rows: ElementRows,
}

/// 渲染半边的全部产物；`write_through_on` 把它落库，对拍测试直接比它。
pub struct RenderedElements {
    pub elements: Vec<RenderedElement>,
    pub missing: Vec<RefU64>,
    pub foreign: usize,
    pub requested: usize,
    pub warnings: Vec<String>,
}

/// Serialises data-plane commits per database while leaving file parsing free
/// to run in parallel.  The SQL emitted below supplies the second line of
/// defence: a transactional element-session guard.
pub struct ReadThroughCommitCoordinator {
    lanes: DashMap<u32, Arc<tokio::sync::Mutex<()>>>,
}

struct CommitOutcome {
    statements: usize,
    skipped_stale: usize,
    cache_epoch: u64,
}

impl ReadThroughCommitCoordinator {
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<ReadThroughCommitCoordinator> = OnceLock::new();
        INSTANCE.get_or_init(|| Self {
            lanes: DashMap::new(),
        })
    }

    async fn commit(
        &self,
        db: &Surreal<Any>,
        rendered: &RenderedElements,
        source: &ReadThroughSource,
        prerequisite_statements: &[String],
    ) -> anyhow::Result<CommitOutcome> {
        let lane = self
            .lanes
            .entry(source.dbnum)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = lane.lock().await;
        let mut current_sql = String::new();
        for element in &rendered.elements {
            current_sql.push_str(&format!(
                "SELECT VALUE sesno FROM ONLY {};\n",
                element.rows.pe_key()
            ));
        }
        current_sql.push_str(&format!(
            "SELECT VALUE cache_epoch FROM ONLY read_through_cache:{};",
            source.dbnum
        ));
        let mut current = db.query(current_sql).await?;
        let mut changed = false;
        let mut skipped_stale = 0usize;
        for (index, element) in rendered.elements.iter().enumerate() {
            let existing: Option<i64> = current.take(index)?;
            match existing {
                Some(existing) if existing > i64::from(element.sesno) => skipped_stale += 1,
                Some(existing) if existing == i64::from(element.sesno) => {}
                _ => changed = true,
            }
        }
        let epoch_index = rendered.elements.len();
        let previous_epoch: Option<i64> = current.take(epoch_index).unwrap_or_default();
        let cache_epoch = previous_epoch.unwrap_or_default().max(0) as u64 + u64::from(changed);
        let epoch_statement = changed.then(|| {
            format!(
                "UPSERT read_through_cache:{} CONTENT {{ dbnum: {}, cache_epoch: {} }};",
                source.dbnum, source.dbnum, cache_epoch
            )
        });
        let statements = execute_serial(
            db,
            rendered,
            source,
            epoch_statement,
            prerequisite_statements,
        )
        .await?;
        Ok(CommitOutcome {
            statements,
            skipped_stale,
            cache_epoch,
        })
    }

    pub async fn soft_delete_missing(
        &self,
        db: &Surreal<Any>,
        dbnum: u32,
        deletion_sesno: u32,
        ids: &[String],
    ) -> anyhow::Result<(usize, u64)> {
        #[derive(serde::Deserialize)]
        struct Existing {
            sesno: i64,
            #[serde(default)]
            deleted: bool,
        }
        let lane = self
            .lanes
            .entry(dbnum)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = lane.lock().await;
        let mut sql = String::new();
        for id in ids {
            sql.push_str(&format!("SELECT sesno, deleted FROM ONLY pe:{id};\n"));
        }
        sql.push_str(&format!(
            "SELECT VALUE cache_epoch FROM ONLY read_through_cache:{dbnum};"
        ));
        let mut response = db.query(sql).await?.check()?;
        let mut candidates = Vec::new();
        for (index, id) in ids.iter().enumerate() {
            let existing: Option<Existing> = response.take(index)?;
            if existing.is_some_and(|row| !row.deleted && row.sesno <= i64::from(deletion_sesno)) {
                candidates.push(id.clone());
            }
        }
        let previous_epoch: Option<i64> = response.take(ids.len()).unwrap_or_default();
        let epoch =
            previous_epoch.unwrap_or_default().max(0) as u64 + u64::from(!candidates.is_empty());
        if candidates.is_empty() {
            return Ok((0, epoch));
        }
        let mut statements = vec![(
            None,
            format!(
                "UPSERT read_through_cache:{dbnum} CONTENT {{ dbnum: {dbnum}, cache_epoch: {epoch} }};"
            ),
        )];
        statements.extend(candidates.iter().map(|id| {
            (
                Some(id.clone()),
                format!(
                    "LET $delete_sesno_{id} = (SELECT VALUE sesno FROM ONLY pe:{id});\n\
                 IF $delete_sesno_{id} != NONE AND $delete_sesno_{id} <= {deletion_sesno} {{\n\
                 UPDATE pe:{id} SET deleted = true, sesno = {deletion_sesno};\n\
                 DELETE pe:{id}<-pe_owner;\n}};"
                ),
            )
        }));
        for transaction in dbnum_stats::batches(dbnum, &statements)? {
            dbnum_stats::execute_on(db, &transaction).await?;
        }
        Ok((candidates.len(), epoch))
    }
}

/// 祖先链最多走这么多层：E3D 的 owner 链是有限的，超过就是文件里的 owner 有环。
const MAX_ANCESTOR_DEPTH: usize = 128;

/// `root` 的祖先链，自下而上直到 WORL（WORL 的 owner 无效，链在那儿停）。走的是 `set` 上的
/// owner 导航（e3d-io `DbElement::owner`，池级缓存命中），不另建一份 owner 表。
///
/// 祖先链是 plant-ui `nouns_of(anc)`、房间的 `pe_owner` 上溯与隐式直管的 `in.owner` 都要的；
/// 一条链几层，便宜。
pub fn ancestors_of(set: &Arc<DbSet>, root: RefNo) -> anyhow::Result<Vec<RefNo>> {
    let mut seen: BTreeSet<(u32, u32)> = BTreeSet::from([(root.word0, root.word1)]);
    let mut out = Vec::new();
    let mut current = set.element(root);
    for _ in 0..MAX_ANCESTOR_DEPTH {
        let Some(owner) = current.owner().map_err(|error| {
            anyhow::anyhow!("上溯 {} 的 owner 失败: {error}", pdms_str(current.refno()))
        })?
        else {
            break;
        };
        let refno = owner.refno();
        if !refno.is_valid() || !seen.insert((refno.word0, refno.word1)) {
            break;
        }
        out.push(refno);
        current = owner;
    }
    Ok(out)
}

/// 写透名单：`visited`（DFS 访问名单，含根）∪ 根的祖先链。去重、保序：先名单，再祖先自下而上。
pub fn subtree_with_ancestors(
    set: &Arc<DbSet>,
    root: RefNo,
    visited: &[RefNo],
) -> anyhow::Result<Vec<RefNo>> {
    let mut seen: BTreeSet<(u32, u32)> = BTreeSet::new();
    let mut out = Vec::with_capacity(visited.len() + 8);
    for refno in visited.iter().copied().chain(std::iter::once(root)) {
        if seen.insert((refno.word0, refno.word1)) {
            out.push(refno);
        }
    }
    for refno in ancestors_of(set, root)? {
        if seen.insert((refno.word0, refno.word1)) {
            out.push(refno);
        }
    }
    Ok(out)
}

fn refu64(refno: RefNo) -> RefU64 {
    RefU64::from_two_nums(refno.word0, refno.word1)
}

fn pdms_str(refno: RefNo) -> String {
    aios_core::RefnoEnum::from(refu64(refno)).to_pdms_str()
}

/// 渲染半边（同步、只读文件、不碰库）：每个元素 → 终态行 + last-touch 会话。
///
/// 与 `stream_baseline` 对同一元素渲出的行必须逐字段相同——它是这条路唯一的验收
/// （`tests/read_through_rows_parity.rs`）。
pub fn render_rows(
    set: &Arc<DbSet>,
    source: &ReadThroughSource,
    refnos: &[RefNo],
    uda_keys: &UdaKeyMap,
) -> anyhow::Result<RenderedElements> {
    render_rows_with_catalog(set, source, refnos, uda_keys, None)
}

pub(crate) fn render_rows_with_catalog(
    set: &Arc<DbSet>,
    source: &ReadThroughSource,
    refnos: &[RefNo],
    uda_keys: &UdaKeyMap,
    catalog: Option<&e3d_io::uda_catalog::UdaCatalog>,
) -> anyhow::Result<RenderedElements> {
    let mut ordered = Vec::with_capacity(refnos.len());
    let mut seen: BTreeSet<(u32, u32)> = BTreeSet::new();
    let mut foreign = 0usize;
    for refno in refnos {
        if !seen.insert((refno.word0, refno.word1)) {
            continue;
        }
        if refno.dbno() != source.dbnum {
            foreign += 1;
            continue;
        }
        ordered.push(*refno);
    }
    let requested = seen.len();

    let mut out = RenderedElements {
        elements: Vec::with_capacity(ordered.len()),
        missing: Vec::new(),
        foreign,
        requested,
        warnings: Vec::new(),
    };
    if ordered.is_empty() {
        return Ok(out);
    }

    // last-touch 会话：会话链把页号映射到写它的那个会话，目标会话的索引给出终稿记录
    // 所在页——与 `stream_baseline` 的 `pages.sesno_of_page(position.page.page)` 同一口径。
    let chain = session_chain(&source.file)?;
    let pages = PageSessions::from_chain(&chain);
    let mut view =
        DbView::open_at(&source.file, SessionSelector::Exact(source.sesno)).map_err(|error| {
            anyhow::anyhow!(
                "以会话 {} 打开 {} 失败: {error}",
                source.sesno,
                source.file.display()
            )
        })?;
    let mut cursor = IndexCursor::new(&mut view);
    let dbnum = source.dbnum as i32;
    for refno in ordered {
        let lookup = cursor.seek(refno).map_err(|error| {
            anyhow::anyhow!(
                "在 {} 会话 {} 的索引里找 {} 失败: {error}",
                source.file.display(),
                source.sesno,
                pdms_str(refno)
            )
        })?;
        let Some(found) = lookup.found else {
            out.missing.push(refu64(refno));
            continue;
        };
        let sesno = pages
            .sesno_of_page(found.position().page.page)
            .unwrap_or(source.sesno);
        let refno64 = refu64(refno);
        let data = match catalog {
            Some(catalog) => read_ele_data_with_uda_catalog(set, refno64, catalog),
            None => read_ele_data(set, refno64),
        };
        match data {
            Ok(Some(data)) => {
                let id = data.refno.to_string();
                let rows = ElementRows::render(&data, &id, dbnum, sesno, uda_keys);
                out.elements.push(RenderedElement {
                    refno: refno64,
                    sesno,
                    rows,
                });
            }
            Ok(None) => out.missing.push(refno64),
            // 与基线的 `allowed_ignored_final` 同一类：attlib 反不出名字的 noun、schema 外的
            // noun、存储形状冲突。基线对白名单外的直接中止；读透是缓存，一个元素读不出不
            // 该让整根的其余元素也不进库——但必须点名。
            Err(error) => out.warnings.push(format!(
                "read_through: {} 读不出数据面终态，未写：{error:#}",
                pdms_str(refno)
            )),
        }
    }
    Ok(out)
}

/// Convert a frozen render into the exact idempotent transactions stored in
/// the local mirror spool.  This deliberately performs no database reads.
pub(crate) fn rendered_transaction_batches(
    rendered: &RenderedElements,
    source: &ReadThroughSource,
    prerequisite_statements: &[String],
) -> anyhow::Result<Vec<String>> {
    let element_statements = rendered
        .elements
        .iter()
        .map(|element| {
            element
                .rows
                .read_through_guarded_statements(UdaRowPolicy::DeleteWhenEmpty)
        })
        .collect::<Vec<_>>();
    let mut statements = Vec::with_capacity(
        element_statements.len()
            + prerequisite_statements.len()
            + usize::from(!element_statements.is_empty()),
    );
    if !element_statements.is_empty() {
        let mut digest = Sha256::new();
        digest.update(source.dbnum.to_le_bytes());
        digest.update(source.sesno.to_le_bytes());
        for statement in prerequisite_statements.iter().chain(&element_statements) {
            digest.update((statement.len() as u64).to_le_bytes());
            digest.update(statement.as_bytes());
        }
        let commit_id = hex::encode(digest.finalize());
        statements.push((
            None,
            mirror_epoch_guard(source.dbnum, source.sesno, &commit_id),
        ));
    }
    statements.extend(
        prerequisite_statements
            .iter()
            .cloned()
            .map(|sql| (None, sql)),
    );
    statements.extend(
        rendered
            .elements
            .iter()
            .zip(element_statements)
            .map(|(element, sql)| (Some(element.refno.to_string()), sql)),
    );
    dbnum_stats::batches(source.dbnum, &statements)
}

fn mirror_epoch_guard(dbnum: u32, sesno: u32, commit_id: &str) -> String {
    format!(
        "LET $rt_seen = (SELECT VALUE id FROM ONLY read_through_commit:{commit_id});\n\
             IF $rt_seen = NONE {{\n\
               LET $rt_epoch = (SELECT VALUE cache_epoch FROM ONLY read_through_cache:{dbnum});\n\
               IF $rt_epoch = NONE {{\n\
                 UPSERT read_through_cache:{dbnum} CONTENT {{ dbnum: {dbnum}, cache_epoch: 1 }};\n\
               }} ELSE {{\n\
                 UPDATE read_through_cache:{dbnum} SET cache_epoch += 1;\n\
               }};\n\
               CREATE read_through_commit:{commit_id} CONTENT {{ dbnum: {dbnum}, sesno: {sesno}, content_hash: '{commit_id}' }};\n\
             }};",
    )
}

/// Render a complete frozen source job and durably enqueue it without touching
/// SurrealDB.  Model publication uses this as its fail-closed hand-off.
pub(crate) fn spool_write_through(
    set: &Arc<DbSet>,
    source: &ReadThroughSource,
    refnos: &[RefNo],
) -> anyhow::Result<ReadThroughReceipt> {
    let started = Instant::now();
    let option = aios_core::get_db_option();
    let snapshot = super::mdb_memory_store::ensure(&option.project_name, &option.mdb_name)?;
    let catalog = snapshot.uda_catalog();
    let keys = UdaKeyMap::from_catalog(&catalog);
    let prerequisites = catalog
        .definitions()
        .iter()
        .map(super::element_attributes::uda_definition_upsert)
        .collect::<Vec<_>>();
    let rendered = render_rows_with_catalog(set, source, refnos, &keys, Some(&catalog))?;
    let mut transactions = rendered_transaction_batches(&rendered, source, &prerequisites)?;
    if transactions.is_empty() {
        transactions.push("BEGIN TRANSACTION;\nRETURN NONE;\nCOMMIT TRANSACTION;\n".into());
    }
    let spool = super::mirror_spool::MirrorSpool::for_process(
        &option.surreal_ns,
        &option.project_name,
        &option.mdb_name,
    )?;
    let snapshot_epoch = snapshot.epoch();
    spool.persist(
        snapshot_epoch,
        vec![(source.dbnum, source.sesno)],
        "element",
        transactions,
    )?;
    Ok(ReadThroughReceipt {
        dbnum: source.dbnum,
        sesno: source.sesno,
        requested: rendered.requested,
        written: rendered.elements.len(),
        skipped_stale: 0,
        cache_epoch: u64::from(source.sesno),
        foreign: rendered.foreign,
        missing: rendered
            .missing
            .iter()
            .copied()
            .map(|value| aios_core::RefnoEnum::from(value).to_pdms_str())
            .collect(),
        statements: rendered.elements.len(),
        elapsed_ms: started.elapsed().as_millis(),
        warnings: rendered.warnings,
    })
}

/// 把渲染好的行按元素分块 `UPSERT` 进 `db`。
async fn execute_serial(
    db: &Surreal<Any>,
    rendered: &RenderedElements,
    source: &ReadThroughSource,
    epoch_statement: Option<String>,
    prerequisite_statements: &[String],
) -> anyhow::Result<usize> {
    let statements = rendered
        .elements
        .iter()
        .map(|element| {
            element
                .rows
                .read_through_guarded_statements(UdaRowPolicy::DeleteWhenEmpty)
        })
        .collect::<Vec<_>>();
    let element_statement_count = statements.len();
    let mut transaction_statements = Vec::with_capacity(statements.len() + 1);
    if let Some(epoch_statement) = epoch_statement {
        // Keep the epoch change in the first successful data transaction.  A
        // process crash can therefore never expose changed cache rows under
        // the old version stamp.
        transaction_statements.push((None, epoch_statement));
    }
    transaction_statements.extend(
        prerequisite_statements
            .iter()
            .cloned()
            .map(|sql| (None, sql)),
    );
    transaction_statements.extend(
        rendered
            .elements
            .iter()
            .zip(statements)
            .map(|(element, sql)| (Some(element.refno.to_string()), sql)),
    );
    for tx_sql in dbnum_stats::batches(source.dbnum, &transaction_statements)? {
        dbnum_stats::execute_on(db, &tx_sql).await?;
    }
    Ok(element_statement_count)
}

/// 读透的唯一入口（进程全局 `SUL_DB`）：渲染 + 落库 + 回执。UDA 键表从库里读一次
/// （`UDA` 表来自 DICT；读透形态下它多半是空的，查不到的键进 `warnings`，不静默）。
pub async fn write_through(
    set: &Arc<DbSet>,
    source: &ReadThroughSource,
    refnos: &[RefNo],
) -> anyhow::Result<ReadThroughReceipt> {
    let uda_keys = UdaKeyMap::load().await?;
    write_through_on(&SUL_DB, set, source, refnos, &uda_keys).await
}

/// 同上，但落到调用方给的库句柄——`mem://` 对拍用；生产走 [`write_through`]。
pub async fn write_through_on(
    db: &Surreal<Any>,
    set: &Arc<DbSet>,
    source: &ReadThroughSource,
    refnos: &[RefNo],
    uda_keys: &UdaKeyMap,
) -> anyhow::Result<ReadThroughReceipt> {
    write_through_on_with_prerequisites(db, set, source, refnos, uda_keys, &[]).await
}

pub async fn write_through_on_with_prerequisites(
    db: &Surreal<Any>,
    set: &Arc<DbSet>,
    source: &ReadThroughSource,
    refnos: &[RefNo],
    uda_keys: &UdaKeyMap,
    prerequisite_statements: &[String],
) -> anyhow::Result<ReadThroughReceipt> {
    write_through_on_with_catalog_and_prerequisites(
        db,
        set,
        source,
        refnos,
        uda_keys,
        None,
        prerequisite_statements,
    )
    .await
}

pub async fn write_through_on_with_catalog_and_prerequisites(
    db: &Surreal<Any>,
    set: &Arc<DbSet>,
    source: &ReadThroughSource,
    refnos: &[RefNo],
    uda_keys: &UdaKeyMap,
    catalog: Option<Arc<e3d_io::uda_catalog::UdaCatalog>>,
    prerequisite_statements: &[String],
) -> anyhow::Result<ReadThroughReceipt> {
    let started = Instant::now();
    let set = set.clone();
    let source_for_render = source.clone();
    let refnos_for_render = refnos.to_vec();
    let uda_for_render = uda_keys.clone();
    let rendered = tokio::task::spawn_blocking(move || {
        render_rows_with_catalog(
            &set,
            &source_for_render,
            &refnos_for_render,
            &uda_for_render,
            catalog.as_deref(),
        )
    })
    .await
    .map_err(|error| anyhow::anyhow!("read-through render task failed: {error}"))??;
    let committed = ReadThroughCommitCoordinator::global()
        .commit(db, &rendered, source, prerequisite_statements)
        .await?;
    let mut warnings = rendered.warnings;
    if let Some(warning) = uda_keys.misses_warning(source.dbnum) {
        warnings.push(warning);
    }
    Ok(ReadThroughReceipt {
        dbnum: source.dbnum,
        sesno: source.sesno,
        requested: rendered.requested,
        written: rendered
            .elements
            .len()
            .saturating_sub(committed.skipped_stale),
        skipped_stale: committed.skipped_stale,
        cache_epoch: committed.cache_epoch,
        foreign: rendered.foreign,
        missing: rendered
            .missing
            .iter()
            .map(|refno| aios_core::RefnoEnum::from(*refno).to_pdms_str())
            .collect(),
        statements: committed.statements,
        elapsed_ms: started.elapsed().as_millis(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn explicit_stats_follow_guarded_write_and_spool_replay() {
        use aios_core::NamedAttrValue;
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("stats").use_db("read_through").await.unwrap();
        let refno = RefU64::from_two_nums(24384, 1);
        let mut data = crate::data_interface::ele_data::EleData::default();
        data.refno = refno;
        data.name = "/STATS-BOX".into();
        let map = &mut data.whole_attmap.attmap.map;
        map.insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        map.insert("TYPE".into(), NamedAttrValue::StringType("BOX".into()));
        map.insert(
            "NAME".into(),
            NamedAttrValue::StringType("/STATS-BOX".into()),
        );
        map.insert("DBNUM".into(), NamedAttrValue::IntegerType(8000));
        let rendered = RenderedElements {
            elements: vec![RenderedElement {
                refno,
                sesno: 5,
                rows: ElementRows::render(&data, &refno.to_string(), 8000, 5, &UdaKeyMap::empty()),
            }],
            missing: vec![],
            foreign: 0,
            requested: 1,
            warnings: vec![],
        };
        let source = ReadThroughSource {
            dbnum: 8000,
            sesno: 5,
            file: "unused-frozen-source".into(),
        };
        execute_serial(&db, &rendered, &source, None, &[])
            .await
            .unwrap();
        for _ in 0..2 {
            for sql in rendered_transaction_batches(&rendered, &source, &[]).unwrap() {
                dbnum_stats::execute_on(&db, &sql).await.unwrap();
            }
        }
        ReadThroughCommitCoordinator::global()
            .soft_delete_missing(&db, 8000, 6, &[refno.to_string()])
            .await
            .unwrap();
        // A stale read must not restore the tombstone or change its count.
        execute_serial(&db, &rendered, &source, None, &[])
            .await
            .unwrap();
        let mut result = db.query("RETURN dbnum_info_table:24384.count; RETURN pe:24384_1.deleted; RETURN dbnum_info_table:24384.sesno;").await.unwrap();
        assert_eq!(result.take::<Option<i64>>(0).unwrap(), Some(1));
        assert_eq!(result.take::<Option<bool>>(1).unwrap(), Some(true));
        assert_eq!(result.take::<Option<i64>>(2).unwrap(), Some(6));
    }

    #[tokio::test]
    async fn mirror_epoch_guard_is_persistent_and_idempotent() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem db boots");
        db.use_ns("epoch-test").use_db("epoch-test").await.unwrap();
        let sql = mirror_epoch_guard(7997, 106, "abcdef0123456789");
        db.query(&sql).await.unwrap().check().unwrap();
        db.query(&sql).await.unwrap().check().unwrap();
        let epoch: Option<i64> = db
            .query("SELECT VALUE cache_epoch FROM ONLY read_through_cache:7997;")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(epoch, Some(1), "the same durable job advances epoch once");

        db.query(mirror_epoch_guard(7997, 106, "fedcba9876543210"))
            .await
            .unwrap()
            .check()
            .unwrap();
        let epoch: Option<i64> = db
            .query("SELECT VALUE cache_epoch FROM ONLY read_through_cache:7997;")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(epoch, Some(2), "new content advances the persistent epoch");
    }

    /// 本模块与基线 / 增量共用同一个渲染器与同一条读法，自己不拼 `pe` 行、不另开一条读元素
    /// 的路（不变量 R2）；写语句只有 `UPSERT`（幂等刷新），没有 `INSERT` / `CREATE` / 根范围
    /// `DELETE`（Q2）。
    #[test]
    fn the_read_through_path_reuses_the_one_row_renderer_and_only_upserts() {
        let source = include_str!("read_through.rs");
        let body = source.split_once("#[cfg(test)]").expect("tests").0;
        assert!(
            body.contains("ElementRows::render("),
            "渲染必须走 ElementRows::render"
        );
        assert!(
            body.contains("read_ele_data("),
            "读取必须走 e3d_collector::read_ele_data"
        );
        assert!(
            body.contains("read_through_guarded_statements(UdaRowPolicy::DeleteWhenEmpty)"),
            "动词是 UPSERT（增量路的 Modified 口径）"
        );
        for forbidden in [
            "INSERT IGNORE",
            "INSERT INTO",
            "CREATE pe",
            "DELETE FROM pe",
            "gen_sur_json(",
            "pe_owner_edge(",
            "applied_sesno",
            "stream_baseline(",
        ] {
            assert!(
                !body.contains(forbidden),
                "read_through 不许出现 {forbidden}"
            );
        }
    }
}
