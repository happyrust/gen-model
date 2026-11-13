//! Select one complete root from the process overlay or the durable baseline.
//!
//! A resident receipt shadows RocksDB for the whole root. Durable reads are
//! returned directly and never hydrate kv-mem.
use std::collections::HashMap;
use std::sync::{Mutex as StdMutex, OnceLock};

use aios_core::{GeomInstQuery, ModelHashInst, RefU64, RefnoEnum};
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};
use tokio::sync::Mutex;

use super::model_memory_store::{
    ModelMemoryStore, ProjectedGeometry, ProjectedShape, ProjectionScope,
};

pub async fn records_of_root(
    db: &Surreal<Any>,
    root: RefnoEnum,
) -> anyhow::Result<Vec<GeomInstQuery>> {
    let packed = root.refno().0 as u64;
    let e3d_root = e3d_io::refno::RefNo::new((packed >> 32) as u32, packed as u32);
    let store = ModelMemoryStore::global();
    if let Some(records) = overlay_records_of_root(store, e3d_root)? {
        return Ok(records
            .into_iter()
            .map(|record| projection_record_to_query(record, e3d_root))
            .collect());
    }
    durable_records_of_root(db, root).await
}

/// 投影里有回执的根：整根记录按库上的顺序（`inst_relate` 先、`tubi_relate` 后，各按记录 id）；没有回执回 `None`（走冷读）。
fn overlay_records_of_root(
    store: &ModelMemoryStore,
    root: e3d_io::refno::RefNo,
) -> anyhow::Result<Option<Vec<ProjectedGeometry>>> {
    let Some((receipt, mut records)) = store.root_snapshot(ProjectionScope::Current, root) else {
        return Ok(None);
    };
    anyhow::ensure!(
        records.len() == receipt.geometry_count,
        "memory overlay root {root} is incomplete: receipt expects {}, store has {}",
        receipt.geometry_count,
        records.len()
    );
    // Match the durable order: inst_relate first, then tubi_relate,
    // each ordered by record id. Eviction must not change cursor offsets.
    sort_overlay_records(&mut records);
    Ok(Some(records))
}

/// 一段隐式管身的身份（方案 B T2，2026-09-22；spec §4.5「`tube`」）：容器就是这条记录的 `refno`（BRAN），加上两端管件与同
/// `(from, to)` 内的序号——与房间层级树 `tubes=1` 的直段 `(BRAN, from, to, ordinal)` 同一个四元组，前端拿它把直段行与场景对象对上。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TubeMeta {
    pub ordinal: u32,
    /// `a_b`
    pub from: String,
    /// `a_b`
    pub to: String,
}

/// `/model/records` 的一条：库行 / 投影记录折成的 [`GeomInstQuery`]（形状一字不改，`serde(flatten)`）+ 隐式管身才有的
/// [`TubeMeta`]。放在记录一级而不是 `insts[]` 里：`ModelHashInst` 在 `aios_core`，这一格只属于本仓的读面；一条管身记录恰好一个实例。
#[derive(Debug, Serialize)]
pub struct ModelRecord {
    #[serde(flatten)]
    pub inst: GeomInstQuery,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tube: Option<TubeMeta>,
}

/// [`records_of_root`] 的带直段元数据版（`/model/records` 用）：投影路直接来自 `ProjectedTube`；冷读路读 `tubi_relate` 的
/// `leave` / `arrive` / `route_ordinal`，`route_ordinal` 没落盘的老行按同 `(from, to)` 内的出现序补号（行按记录 id 排，
/// 与 `spatial_tree_tubes::from_durable_rows` 同一规则）。两端缺失的历史行 `tube` 为 `None`。
pub async fn model_records_of_root(
    db: &Surreal<Any>,
    root: RefnoEnum,
) -> anyhow::Result<Vec<ModelRecord>> {
    let packed = root.refno().0 as u64;
    let e3d_root = e3d_io::refno::RefNo::new((packed >> 32) as u32, packed as u32);
    let store = ModelMemoryStore::global();
    if let Some(records) = overlay_records_of_root(store, e3d_root)? {
        return Ok(records
            .into_iter()
            .map(|record| {
                let tube = record.tube.as_ref().map(|tube| TubeMeta {
                    ordinal: tube.route_ordinal,
                    from: refno_text(tube.from),
                    to: refno_text(tube.to),
                });
                ModelRecord {
                    inst: projection_record_to_query(record, e3d_root),
                    tube,
                }
            })
            .collect());
    }
    durable_model_records_of_root(db, root).await
}

/// 投影里一根**名下、某个节点以下**的记录（ADR-086 D1：`/model/records` 拿根以下的节点——BRAN 里的一个 VALV——来问时，
/// 内存路由等价于库上的 `anc CONTAINS <节点>`）。记录仍按整根的序排，`owner` 仍是根；根没有回执时为空
/// （调用方只在 [`ModelMemoryStore::receipt_root_above`] 给出了根时才走到这里）。
pub fn overlay_records_under_node(
    store: &ModelMemoryStore,
    root: e3d_io::refno::RefNo,
    node: e3d_io::refno::RefNo,
) -> anyhow::Result<Vec<ModelRecord>> {
    let Some(records) = overlay_records_of_root(store, root)? else {
        return Ok(Vec::new());
    };
    let packed_node = crate::fast_model::model_memory_store::packed_refno(node);
    Ok(records
        .into_iter()
        .filter(|record| record.ancestors.contains(&packed_node))
        .map(|record| {
            let tube = record.tube.as_ref().map(|tube| TubeMeta {
                ordinal: tube.route_ordinal,
                from: refno_text(tube.from),
                to: refno_text(tube.to),
            });
            ModelRecord {
                inst: projection_record_to_query(record, root),
                tube,
            }
        })
        .collect())
}

fn refno_text(refno: e3d_io::refno::RefNo) -> String {
    RefU64::from_two_nums(refno.word0, refno.word1).to_string()
}

/// 管身行的直段三格（第三句 SQL 的行，与第二句的管身行**同序**——两句同一个作用域、同一个 `ORDER BY record_order`）。
/// 不与 `GeomInstQuery` 合成一行 `serde(flatten)` 读：SurrealDB 的反序列化器过 flatten 缓冲时 `RefnoEnum` 这类 untagged 枚举
/// 会报「untagged and internally tagged enums do not support enum input」，所以管身行读两遍、按序配对。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
struct TubeColumns {
    #[serde(default)]
    tube_from: Option<String>,
    #[serde(default)]
    tube_to: Option<String>,
    #[serde(default)]
    tube_ordinal: Option<u32>,
}

/// 管身记录 + 同序的三格 → [`ModelRecord`]：`route_ordinal` 缺的按同 `(容器, from, to)` 内出现序补号；两端缺的没有 `tube`。
fn pair_tube_records(tubes: Vec<GeomInstQuery>, metas: Vec<TubeColumns>) -> Vec<ModelRecord> {
    let mut seen: HashMap<(String, String, String), u32> = HashMap::new();
    tubes
        .into_iter()
        .zip(metas)
        .map(|(inst, meta)| {
            let tube = match (meta.tube_from, meta.tube_to) {
                (Some(from), Some(to)) => {
                    let container = inst.refno.to_string();
                    let slot = seen
                        .entry((container, from.clone(), to.clone()))
                        .or_insert(0);
                    let ordinal = meta.tube_ordinal.unwrap_or(*slot);
                    *slot += 1;
                    Some(TubeMeta { ordinal, from, to })
                }
                _ => None,
            };
            ModelRecord { inst, tube }
        })
        .collect()
}

/// 冷读一页多少条。整根一次取回会把响应体撑到几十 MB，SurrealDB 的 ws 单条响应撑不住，
/// 报 `receiving from an empty and closed channel`（2026-11-07 现场：`23717/2` 34.1 MB、
/// `24381/101405` 14.2 MB 都失败，7.4 MB 的 `23711/1` 正常）。5000 条一页实测 0.65 s / 2.11 MB。
const DURABLE_READ_CHUNK: usize = 5000;

async fn durable_model_records_of_root(
    db: &Surreal<Any>,
    root: RefnoEnum,
) -> anyhow::Result<Vec<ModelRecord>> {
    let packed = root.refno().0;
    let owner = root.to_pe_key();
    let insts = read_durable_in_chunks(db, root, DurableTable::InstRelate, packed, &owner).await?;
    let tubes = read_durable_in_chunks(db, root, DurableTable::TubiRelate, packed, &owner).await?;
    let mut records = insts
        .into_iter()
        .map(|row| ModelRecord {
            inst: row.inst,
            tube: None,
        })
        .collect::<Vec<_>>();
    // 管身三格与管身记录必须同序配对，所以两路都按 `record_order` 分块读回（见 [`read_durable_in_chunks`]）。
    let (tube_rows, metas) = tubes
        .into_iter()
        .map(|row| (row.inst, row.meta))
        .unzip::<_, _, Vec<_>, Vec<_>>();
    records.extend(pair_tube_records(tube_rows, metas));
    Ok(records)
}

/// 冷读的一页：记录本体 + 管身三格（只有 `tubi_relate` 有）。
struct DurableRow {
    inst: GeomInstQuery,
    meta: TubeColumns,
}

/// 一张表的两种冷读形状。
#[derive(Clone, Copy)]
enum DurableTable {
    InstRelate,
    TubiRelate,
}

impl DurableTable {
    fn name(self) -> &'static str {
        match self {
            Self::InstRelate => "inst_relate",
            Self::TubiRelate => "tubi_relate",
        }
    }
}

/// 按 `record_order`（= 记录 id）分页冷读一根名下的全部行。
///
/// 每页在**库端**带 `LIMIT`，并把上一页最后一条 `id` 当游标（`id > '<上一页末条>'`）——索引顺序与
/// `ORDER BY id` 逐条相同（现场核对过 `START 40000` 与 `ORDER BY id` 同段，2026-11-07），
/// 所以分页不改变行集与顺序，`/model/records` 的游标偏移不受影响。
///
/// 两条实测纪律（ams7333 的 `23717/2`，79409 行）：
/// - **不带 `ORDER BY`**：整根 `ORDER BY` 要 3.3 s（排序整根），只加 `LIMIT` 是 3 ms。
/// - **游标必须写成带引号的字符串**：`id > inst_relate:23717_179010`（record-id 字面量）
///   每页 3.3 s，索引退化；`id > 'inst_relate:23717_179010'` 是 0.33 s。
async fn read_durable_in_chunks(
    db: &Surreal<Any>,
    root: RefnoEnum,
    table: DurableTable,
    packed: u64,
    owner: &str,
) -> anyhow::Result<Vec<DurableRow>> {
    let mut out: Vec<DurableRow> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let sql = durable_page_sql(table, packed, owner, cursor.as_deref(), DURABLE_READ_CHUNK);
        let mut response = db.query(sql).await?.check()?;
        let insts: Vec<GeomInstQuery> = response.take(0)?;
        // `inst_relate` 那一句只投一行记录，没有三格那一句；`tubi_relate` 才有。
        let metas: Vec<TubeColumns> = match table {
            DurableTable::InstRelate => Vec::new(),
            DurableTable::TubiRelate => response.take(1)?,
        };
        anyhow::ensure!(
            matches!(table, DurableTable::InstRelate) || insts.len() == metas.len(),
            "durable {} rows of {root}: {} records but {} identity rows",
            table.name(),
            insts.len(),
            metas.len()
        );
        if insts.is_empty() {
            break;
        }
        let last = insts
            .last()
            .map(|inst| inst.refno.to_string())
            .expect("non-empty page has a last row");
        let short_page = insts.len() < DURABLE_READ_CHUNK;
        if metas.is_empty() {
            out.extend(insts.into_iter().map(|inst| DurableRow {
                inst,
                meta: TubeColumns::default(),
            }));
        } else {
            out.extend(
                insts
                    .into_iter()
                    .zip(metas)
                    .map(|(inst, meta)| DurableRow { inst, meta }),
            );
        }
        if short_page {
            break;
        }
        // `refno` 在 SQL 里是 `in AS refno`（`pe:<a>_<b>`），转成记录 id 要加表名前缀。
        cursor = Some(format!("{}:{last}", table.name()));
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
struct IdRow {
    id: String,
}

fn record_id_cursor(id: &str) -> String {
    id.replace(['⟨', '⟩', '<', '>'], "")
}

/// 与 [`durable_page_sql`] 同一窗口的记录 id。游标用这一列的最后一条，不用 `refno` 拼出来的 id。
fn durable_id_page_sql(
    table: DurableTable,
    packed: u64,
    owner: &str,
    cursor: Option<&str>,
    limit: usize,
) -> String {
    let cursor = cursor
        .map(|cursor| format!(" AND id > {cursor}"))
        .unwrap_or_default();
    let [by_anc, by_owner] = durable_scope_reads(table.name(), "type::string(id) AS id", packed, owner);
    let page = |sql: String| format!("{sql}{cursor} LIMIT {limit}");
    format!(
        "SELECT * FROM array::union(({}), ({}))",
        page(by_anc),
        page(by_owner)
    )
}

/// `/model/records` 的一页。`skip_consumed` 是这一根里被跳过、没有放进 `rows` 的行数
/// （跨根游标用它往下减）。`more` 表示这一根在 `rows` 之后还有行。
pub struct RecordWindow {
    pub rows: Vec<ModelRecord>,
    pub skip_consumed: usize,
    pub more: bool,
}

/// `len` 是已经读进缓存的行数，`finished` 表示缓存后面没有更多行。
pub(crate) fn project_cached_window(
    len: usize,
    finished: bool,
    skip: usize,
    limit: usize,
) -> (usize, usize, usize, bool) {
    if len <= skip {
        return (0, 0, len, !finished);
    }
    let end = (skip + limit).min(len);
    let more = end < len || !finished;
    (skip, end, skip, more)
}

pub fn slice_model_records(records: Vec<ModelRecord>, skip: usize, limit: usize) -> RecordWindow {
    let (start, end, skip_consumed, more) = project_cached_window(records.len(), true, skip, limit);
    let rows = records.into_iter().skip(start).take(end.saturating_sub(start)).collect();
    RecordWindow { rows, skip_consumed, more }
}

fn clone_inst(inst: &ModelHashInst) -> ModelHashInst {
    ModelHashInst {
        geo_hash: inst.geo_hash.clone(),
        transform: inst.transform.clone(),
        is_tubi: inst.is_tubi,
        is_invalid_tubi: inst.is_invalid_tubi,
    }
}

fn clone_query(query: &GeomInstQuery) -> GeomInstQuery {
    GeomInstQuery {
        refno: query.refno,
        old_refno: query.old_refno,
        owner: query.owner,
        world_aabb: query.world_aabb,
        world_trans: query.world_trans.clone(),
        insts: query.insts.iter().map(clone_inst).collect(),
        has_neg: query.has_neg,
        generic: query.generic.clone(),
        pts: query.pts.clone(),
        date: query.date.clone(),
    }
}

fn clone_record(record: &ModelRecord) -> ModelRecord {
    ModelRecord {
        inst: clone_query(&record.inst),
        tube: record.tube.clone(),
    }
}

#[derive(Default)]
struct DurableProgress {
    records: Vec<ModelRecord>,
    inst_cursor: Option<String>,
    inst_done: bool,
    tubi_cursor: Option<String>,
    tubi_done: bool,
    tube_seen: HashMap<(String, String, String), u32>,
}

impl DurableProgress {
    fn finished(&self) -> bool {
        self.inst_done && self.tubi_done
    }
}

fn durable_windows() -> &'static StdMutex<HashMap<u64, std::sync::Arc<Mutex<DurableProgress>>>> {
    static SLOTS: OnceLock<StdMutex<HashMap<u64, std::sync::Arc<Mutex<DurableProgress>>>>> = OnceLock::new();
    SLOTS.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// 冷读一根的一个窗口。`skip == 0` 丢掉这根的进程内续读缓存，从库上重新读；
/// `skip > 0` 接着上次的游标往后读，不再把前面的页重读一遍。
async fn durable_model_records_window(
    db: &Surreal<Any>,
    root: RefnoEnum,
    skip: usize,
    limit: usize,
) -> anyhow::Result<RecordWindow> {
    let key = root.refno().0;
    let slot = {
        let mut map = durable_windows().lock().expect("durable record windows");
        if map.len() > 16 && !map.contains_key(&key) {
            map.clear();
        }
        map.entry(key)
            .or_insert_with(|| std::sync::Arc::new(Mutex::new(DurableProgress::default())))
            .clone()
    };
    let mut progress = slot.lock().await;
    if skip == 0 {
        *progress = DurableProgress::default();
    }
    let target = skip.saturating_add(limit);
    while progress.records.len() < target && !progress.finished() {
        pull_durable_chunk(db, root, &mut progress).await?;
    }
    let finished = progress.finished();
    let (start, end, skip_consumed, more) =
        project_cached_window(progress.records.len(), finished, skip, limit);
    let rows = progress.records[start..end].iter().map(clone_record).collect();
    Ok(RecordWindow { rows, skip_consumed, more })
}

async fn pull_durable_chunk(
    db: &Surreal<Any>,
    root: RefnoEnum,
    progress: &mut DurableProgress,
) -> anyhow::Result<()> {
    let packed = root.refno().0;
    let owner = root.to_pe_key();
    if !progress.inst_done {
        let sql = durable_page_sql(
            DurableTable::InstRelate,
            packed,
            &owner,
            progress.inst_cursor.as_deref(),
            DURABLE_READ_CHUNK,
        );
        let mut response = db.query(sql).await?.check()?;
        let insts: Vec<GeomInstQuery> = response.take(0)?;
        if insts.is_empty() {
            progress.inst_done = true;
            return Ok(());
        }
        let short = insts.len() < DURABLE_READ_CHUNK;
        let next_cursor = insts.iter().map(|inst| format!("inst_relate:{}", inst.refno)).max();
        if progress.inst_cursor.is_some() && next_cursor.as_ref() == progress.inst_cursor.as_ref() {
            progress.inst_done = true;
            return Ok(());
        }
        progress.records.extend(insts.into_iter().map(|inst| ModelRecord { inst, tube: None }));
        if short {
            progress.inst_done = true;
        } else if let Some(next_cursor) = next_cursor {
            progress.inst_cursor = Some(next_cursor);
        } else {
            progress.inst_done = true;
        }
        return Ok(());
    }
    if progress.tubi_done {
        return Ok(());
    }
    let sql = durable_page_sql(
        DurableTable::TubiRelate,
        packed,
        &owner,
        progress.tubi_cursor.as_deref(),
        DURABLE_READ_CHUNK,
    );
    let mut response = db.query(sql).await?.check()?;
    let insts: Vec<GeomInstQuery> = response.take(0)?;
    let metas: Vec<TubeColumns> = response.take(1)?;
    anyhow::ensure!(
        insts.len() == metas.len(),
        "durable tubi page of {root}: {} records but {} identity rows",
        insts.len(),
        metas.len()
    );
    if insts.is_empty() {
        progress.tubi_done = true;
        return Ok(());
    }
    let short = insts.len() < DURABLE_READ_CHUNK;
    let last = insts.last().expect("non-empty tubi page").refno.to_string();
    for (inst, meta) in insts.into_iter().zip(metas) {
        let tube = match (meta.tube_from, meta.tube_to) {
            (Some(from), Some(to)) => {
                let container = inst.refno.to_string();
                let slot = progress
                    .tube_seen
                    .entry((container, from.clone(), to.clone()))
                    .or_insert(0);
                let ordinal = meta.tube_ordinal.unwrap_or(*slot);
                *slot += 1;
                Some(TubeMeta { ordinal, from, to })
            }
            _ => None,
        };
        progress.records.push(ModelRecord { inst, tube });
    }
    if short {
        progress.tubi_done = true;
    } else {
        progress.tubi_cursor = Some(format!("tubi_relate:{last}"));
    }
    Ok(())
}

/// 投影里有回执就从内存切窗口；没有就按页冷读。两种都不把整根读完再丢掉前面的页。
pub async fn model_records_window(
    db: &Surreal<Any>,
    root: RefnoEnum,
    skip: usize,
    limit: usize,
) -> anyhow::Result<RecordWindow> {
    let packed = root.refno().0 as u64;
    let e3d_root = e3d_io::refno::RefNo::new((packed >> 32) as u32, packed as u32);
    let store = ModelMemoryStore::global();
    if let Some(records) = overlay_records_of_root(store, e3d_root)? {
        let owned = records
            .into_iter()
            .map(|record| {
                let tube = record.tube.as_ref().map(|tube| TubeMeta {
                    ordinal: tube.route_ordinal,
                    from: refno_text(tube.from),
                    to: refno_text(tube.to),
                });
                ModelRecord {
                    inst: projection_record_to_query(record, e3d_root),
                    tube,
                }
            })
            .collect();
        return Ok(slice_model_records(owned, skip, limit));
    }
    durable_model_records_window(db, root, skip, limit).await
}

fn sort_overlay_records(records: &mut [ProjectedGeometry]) {
    records.sort_by(|a, b| (a.tube.is_some(), &a.record_id).cmp(&(b.tube.is_some(), &b.record_id)));
}

/// 只读库上的基线：一根名下全部 `inst_relate` / `tubi_relate` 行（有世界盒与世界变换的），不看投影。
/// 整库房间重算（ADR-073 D13）对**不在投影里**的根——初始化的模型行（ADR-077 D5）、落盘形态重启前
/// 生成的——从这里读回几何再算归属（`room_model::durable_room_inputs`）；投影里有回执的根不该走这里，
/// 投影那份才是本进程的现值。
pub(crate) async fn durable_records_of_root(
    db: &Surreal<Any>,
    root: RefnoEnum,
) -> anyhow::Result<Vec<GeomInstQuery>> {
    let packed = root.refno().0;
    let owner = root.to_pe_key();
    let mut response = db
        .query(durable_records_sql(packed, &owner))
        .await?
        .check()?;
    let mut records: Vec<GeomInstQuery> = response.take(0)?;
    records.extend(response.take::<Vec<GeomInstQuery>>(1)?);
    Ok(records)
}

/// 冷读要不要先读行上的扁平副本（A6，2026-11-10）。缺省开；环境变量 `AIOS_MODEL_RECORDS_FLAT_READ=0` 退回逐条解引用的
/// 旧写法。两种写法回的内容相同（`flat_and_dereferencing_cold_reads_return_the_same_rows` + 真库逐条对拍），开关只为
/// 两件事：在同一个二进制上量前后差；出了意外不必重编就能退回。进程启动后读一次。
fn flat_read() -> bool {
    static FLAT: OnceLock<bool> = OnceLock::new();
    *FLAT.get_or_init(|| {
        flat_read_from(
            std::env::var("AIOS_MODEL_RECORDS_FLAT_READ")
                .ok()
                .as_deref(),
        )
    })
}

/// 开关的取值规则：只有明写 `0` / `false` / `off` / `no` 才关，没设、空白、认不出的值都按开。
fn flat_read_from(raw: Option<&str>) -> bool {
    !matches!(
        raw.map(|raw| raw.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "false" | "off" | "no")
    )
}

/// 「这一行有几何」的那两个谓词。
///
/// `inst_relate` 行上带着包围盒与世界变换的扁平副本（`aabb_d` / `world_trans_d`；`model_db_adapter` 把它们和
/// `aabb` / `world_trans` 两个指针写在同一行、同一笔里，值是同一份），先读副本就不必为每一行再点查两张表。
/// 没有副本的行——管身行从来不写、模型面之前的老行——照旧解引用，所以 `??` 的右边留着。
/// 2026-11-10 真库量得（ams7333 `23717/2`，一页 5000 行，库内语句时间）：这两个谓词解引用 250–370 ms，读副本几乎不要钱。
fn geometry_predicates(table: &str, flat: bool) -> &'static str {
    if flat && table == "inst_relate" {
        "(aabb_d ?? aabb.d) != NONE AND (world_trans_d ?? world_trans.d) != NONE"
    } else {
        "aabb.d != NONE AND world_trans.d != NONE"
    }
}

/// 一张表的两路作用域读——`(按 anc 的一句, 按 in 的一句)`：子树行 `anc CONTAINS <根>` 走 `idx_<表>_anc`，
/// 根自己的几何 / `anc` 缺失的历史行 `in = <根>` 走 `idx_<表>_in`。两句只差 WHERE 的第一个谓词，
/// 都不带 `ORDER BY`——排序留给并集之后那一层。
fn durable_scope_reads(table: &str, projection: &str, packed: u64, owner: &str) -> [String; 2] {
    durable_scope_reads_from(table, projection, packed, owner, None, flat_read())
}

/// [`durable_scope_reads`] 的带游标版：只要 `id` 在 `cursor` 之后的行。
///
/// 游标谓词紧跟在索引谓词后面、排在「有几何」那两个谓词**之前**：翻到第 N 页时，库端仍从这一根的第一行扫起，
/// 前面读过的行靠 `id > 游标` 滤掉；谓词从左到右短路，游标排前面，被滤掉的行就不必再付「有几何」那两次判断
/// （不读副本时是两次点查）。同一批行、同一个顺序，只是少做事。
fn durable_scope_reads_from(
    table: &str,
    projection: &str,
    packed: u64,
    owner: &str,
    cursor: Option<&str>,
    flat: bool,
) -> [String; 2] {
    let cursor = cursor
        .map(|cursor| format!(" AND id > {cursor}"))
        .unwrap_or_default();
    let geometry = geometry_predicates(table, flat);
    [
        format!(
            "SELECT {projection} FROM {table} WHERE anc CONTAINS {packed}{cursor} AND {geometry}"
        ),
        format!("SELECT {projection} FROM {table} WHERE in = {owner}{cursor} AND {geometry}"),
    ]
}

/// 冷读一页的 SQL：`inst_relate` 一句、`tubi_relate` 的三格一句，调用方 `take(0)` / `take(1)`。
///
/// 作用域仍是 [`durable_scope_reads`] 的两路（`anc CONTAINS` + `in =`），**分开读、各走各的索引、
/// 再 `array::union`**，不写成一条 `anc CONTAINS … OR in = …`：优化器碰到 OR 再挂上别的 AND 谓词
/// （这里的 `aabb.d != NONE AND world_trans.d != NONE`）整条放弃索引退回 `Iterate Table`——单独一条 OR
/// 它会做索引并集，一加 AND 就不做了（SurrealDB 2.1.4 rocksdb 现场 EXPLAIN 实测：47,751 行的
/// `inst_relate` 上一根 0.76 s，拆开 2–3 ms；mem 引擎同样，
/// `durable_cold_read_scopes_are_planned_through_indexes` 钉住）。整库房间重算（ADR-073 D13 / ADR-082）
/// 对 overlay 之外的每一根都要来一次冷读，那 0.8 s 乘上根数就是 O(根 × 库)——d-149 真机 7997
/// （6,772 根、overlay 上限 4）上一窗跑成 90 分钟、看着像卡死。与 [`render_spooled_scope_cleanup`]
/// 同一条纪律。
///
/// 分页（[`read_durable_in_chunks`]）把 `id > '<游标>'` 和 `LIMIT` 加到**每一路**上，再 `array::union`：
/// 两路各自都在索引前缀上截断，并集的结果是「整根按 id 排的前 n 条」——因为两路的行都来自同一张表、
/// 同一套 id 序，截断后再取并集与整根排序后截断等价（管身行在 `anc` 含自身时两路重合，
/// `array::union` 去重，所以不会把同一行算两次）。
///
/// [`render_spooled_scope_cleanup`]: super::model_db_adapter
fn durable_page_sql(
    table: DurableTable,
    packed: u64,
    owner: &str,
    cursor: Option<&str>,
    limit: usize,
) -> String {
    durable_page_sql_with(table, packed, owner, cursor, limit, flat_read())
}

/// [`durable_page_sql`] 的本体；`flat` 明给，单测好把两种写法并排对。
fn durable_page_sql_with(
    table: DurableTable,
    packed: u64,
    owner: &str,
    cursor: Option<&str>,
    limit: usize,
    flat: bool,
) -> String {
    let (projection, meta_projection) = match table {
        DurableTable::InstRelate => (inst_projection_for(owner, flat), None),
        DurableTable::TubiRelate => (tubi_projection(owner), Some(tubi_meta_projection())),
    };
    // 记录 id 字面量才能滤掉已读行。带引号的字符串在这台 SurrealDB 上比较不生效，
    // 下一页仍从第一行开始。`id > inst_relate:24383_100006` 实测 2 ms 且落到下一行。
    // 游标谓词排在哪儿见 [`durable_scope_reads_from`]。
    let [by_anc, by_owner] =
        durable_scope_reads_from(table.name(), &projection, packed, owner, cursor, flat);
    let page = |sql: String| format!("{sql} LIMIT {limit}");
    let rows = format!(
        "SELECT * FROM array::union(({}), ({}))",
        page(by_anc),
        page(by_owner)
    );
    match meta_projection {
        None => rows,
        Some(meta) => {
            let [meta_by_anc, meta_by_owner] =
                durable_scope_reads_from(table.name(), &meta, packed, owner, cursor, flat);
            format!(
                "{rows};\nSELECT * FROM array::union(({}), ({}))",
                page(meta_by_anc),
                page(meta_by_owner)
            )
        }
    }
}

fn inst_projection(owner: &str) -> String {
    inst_projection_for(owner, flat_read())
}

/// `inst_relate` 行的投影。`flat` 时包围盒与世界变换先读行上的扁平副本，没有副本的行再解引用
/// （见 [`geometry_predicates`]）；两种写法投出来的行逐字段相同。`in.old_pe` 那一次点查两种写法都还在：
/// 行上没有它的副本。
fn inst_projection_for(owner: &str, flat: bool) -> String {
    let (world_aabb, world_trans) = if flat {
        ("aabb_d ?? aabb.d", "world_trans_d ?? world_trans.d")
    } else {
        ("aabb.d", "world_trans.d")
    };
    format!(
        "id AS record_order, in AS refno, in.old_pe AS old_refno, {owner} AS owner, \
         generic, {world_aabb} AS world_aabb, {world_trans} AS world_trans, \
         NONE AS pts, booled_id != NONE AS has_neg, \
         IF booled_id != NONE {{ [{{geo_hash: booled_id}}] }} \
         ELSE {{ insts_flat ?? (SELECT transform ?? trans.d AS transform, \
             record::id(out) AS geo_hash FROM out->geo_relate \
             WHERE visible AND out.meshed AND geo_type = 'Pos' \
               AND (transform != NONE OR trans.d != NONE)) }} AS insts"
    )
}

/// 管身行多三格直段身份（方案 B T2）：`leave` / `arrive` 折成 `a_b`、`route_ordinal` 老行读成 NONE；只有
/// `model_records_of_root` 消费它们，`records_of_root` 的 `GeomInstQuery` 不认这三格、原样忽略（同 `record_order`）。
fn tubi_projection(owner: &str) -> String {
    format!(
        "id AS record_order, in AS refno, in.old_pe AS old_refno, {owner} AS owner, \
         'TUBI' AS generic, aabb.d AS world_aabb, world_trans.d AS world_trans, \
         false AS has_neg, \
         [{{geo_hash: record::id(out), is_tubi: true, is_invalid_tubi: invalid ?? false}}] AS insts, \
         IF leave != NONE {{ record::id(leave) }} ELSE {{ NONE }} AS tube_from, \
         IF arrive != NONE {{ record::id(arrive) }} ELSE {{ NONE }} AS tube_to, \
         route_ordinal AS tube_ordinal"
    )
}

fn tubi_meta_projection() -> String {
    "id AS record_order, \
     IF leave != NONE { record::id(leave) } ELSE { NONE } AS tube_from, \
     IF arrive != NONE { record::id(arrive) } ELSE { NONE } AS tube_to, \
     route_ordinal AS tube_ordinal"
        .to_string()
}

/// 整根冷读的两句 SQL（不分页）：`inst_relate` 一句、`tubi_relate` 一句，各自 `ORDER BY record_order`
/// （= 记录 id），调用方 `take(0)` / `take(1)`。
///
/// 只服务 [`durable_records_of_root`]（房间重算等内部路径，一次要整根）。`/model/records` 走
/// [`read_durable_in_chunks`] 分页，见那里的实测纪律。
fn durable_records_sql(packed: u64, owner: &str) -> String {
    let [inst_by_anc, inst_by_owner] =
        durable_scope_reads("inst_relate", &inst_projection(owner), packed, owner);
    let [tubi_by_anc, tubi_by_owner] =
        durable_scope_reads("tubi_relate", &tubi_projection(owner), packed, owner);
    format!(
        "SELECT * FROM array::union(({inst_by_anc}), ({inst_by_owner})) ORDER BY record_order;\n\
         SELECT * FROM array::union(({tubi_by_anc}), ({tubi_by_owner})) ORDER BY record_order;"
    )
}

/// 库上一条隐式管身行里房间层级树 `tubes=1` 要的那几格（`spatial_tree_tubes` 折成 `TubeRecord`；计划
/// `docs/plans/2026-09-22-room-tree-tube-segments-backend-dev-plan.md` §2.2 / §2.7）。`tube_ordinal` 是 T2 才落盘的
/// `route_ordinal`，老行读成 `None`、由调用方按同 `(from, to)` 内的出现序补号——所以行**必须**按 `record_order`（分支槽位）排回来。
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub(crate) struct DurableTubeRow {
    /// 容器 BRAN，`a_b`（`record::id(in)`）。
    pub container: String,
    /// 离开侧构件，`a_b`（`record::id(leave)`）。
    pub tube_from: String,
    /// 到达侧构件，`a_b`（`record::id(arrive)`）。
    pub tube_to: String,
    #[serde(default)]
    pub tube_ordinal: Option<u32>,
    pub world_aabb: parry3d::bounding_volume::Aabb,
    pub world_trans: bevy_transform::components::Transform,
    #[serde(default)]
    pub invalid: bool,
}

/// 一批 BRAN 名下隐式管身行的冷读 SQL：**只读 `tubi_relate`、只按 `in`**——管身行的 `in` 就是容器 BRAN，不需要
/// [`durable_scope_reads`] 的 `anc` 那一路；每根一句等值（`idx_tubi_relate_in`，`pdms_inst::INST_RELATE_INDEX_SQL`），
/// 不写 `in IN [...]`（本 fork 2.1.4 对 `IN` 的索引联合支持没复核过，`pdms_inst::render_watch_scope_filter` 的注记），
/// 拼成一条多语句一次往返、调用方 `take(i)`。两端缺失的历史行（`leave` / `arrive` 为 NONE）没有身份，在库端就滤掉。
/// 钉 `durable_tube_rows_are_planned_through_the_in_index`：每句 `EXPLAIN FULL` 不出现 `Iterate Table`、落在 `idx_tubi_relate_in`。
pub(crate) fn durable_tube_rows_sql(branches: &[RefU64]) -> String {
    branches
        .iter()
        .map(|branch| {
            let owner = RefnoEnum::from(*branch).to_pe_key();
            format!(
                "SELECT record::id(in) AS container, record::id(leave) AS tube_from, record::id(arrive) AS tube_to, \
                 route_ordinal AS tube_ordinal, aabb.d AS world_aabb, world_trans.d AS world_trans, \
                 invalid ?? false AS invalid, id AS record_order FROM tubi_relate \
                 WHERE in = {owner} AND aabb.d != NONE AND world_trans.d != NONE \
                 AND leave != NONE AND arrive != NONE ORDER BY record_order;"
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 冷读一批 BRAN 名下的隐式管身行（[`durable_tube_rows_sql`]）。空清单不碰库。返回顺序 = `branches` 顺序 × 各自槽位序。
pub(crate) async fn durable_tube_rows_of_branches(
    db: &Surreal<Any>,
    branches: &[RefU64],
) -> anyhow::Result<Vec<DurableTubeRow>> {
    if branches.is_empty() {
        return Ok(Vec::new());
    }
    let mut response = db.query(durable_tube_rows_sql(branches)).await?.check()?;
    let mut rows = Vec::new();
    for index in 0..branches.len() {
        rows.extend(response.take::<Vec<DurableTubeRow>>(index)?);
    }
    Ok(rows)
}

fn projection_record_to_query(
    record: ProjectedGeometry,
    root: e3d_io::refno::RefNo,
) -> GeomInstQuery {
    // Durable inst_relate encodes every non-tube Baked projection with a
    // booled_id; canonical rows and tubi_relate rows do not have one.
    let has_neg = record.tube.is_none() && matches!(&record.shape, ProjectedShape::Baked);
    let local_transform = match (&record.shape, &record.tube) {
        (
            ProjectedShape::Canonical {
                local_transform, ..
            },
            None,
        ) => *local_transform,
        _ => glam::DMat4::IDENTITY,
    };
    let (local_scale, local_rotation, local_translation) =
        local_transform.to_scale_rotation_translation();
    let (scale, rotation, translation) = record.world_transform.to_scale_rotation_translation();
    let source = RefnoEnum::from(RefU64::from_two_nums(
        record.source_refno.word0,
        record.source_refno.word1,
    ));
    GeomInstQuery {
        refno: source,
        old_refno: None,
        owner: RefnoEnum::from(RefU64::from_two_nums(root.word0, root.word1)),
        world_aabb: parry3d::bounding_volume::Aabb::new(
            record.world_bounds[0].into(),
            record.world_bounds[1].into(),
        ),
        world_trans: bevy_transform::components::Transform {
            translation: translation.as_vec3(),
            rotation: rotation.as_quat(),
            scale: scale.as_vec3(),
        },
        insts: vec![aios_core::ModelHashInst {
            geo_hash: record.mesh_id,
            transform: bevy_transform::components::Transform {
                translation: local_translation.as_vec3(),
                rotation: local_rotation.as_quat(),
                scale: local_scale.as_vec3(),
            },
            is_tubi: record.tube.is_some(),
            is_invalid_tubi: record.tube.as_ref().is_some_and(|tube| tube.invalid),
        }],
        has_neg,
        generic: record.noun,
        pts: None,
        date: None,
    }
}

#[cfg(test)]
mod tests {
    use aios_core::{RefU64, RefnoEnum};
    use e3d_io::refno::RefNo;
    use e3d_model::elmodl::GeometryId;
    use glam::DMat4;

    use super::project_cached_window;
    use super::super::model_memory_store::{ProjectedGeometry, ProjectedShape, ProjectedTube};

    fn projected(shape: ProjectedShape, tube: Option<ProjectedTube>) -> ProjectedGeometry {
        ProjectedGeometry {
            record_id: "24384_26609".into(),
            geometry_id: GeometryId::Element {
                refno: "24384/26609".into(),
            },
            source_refno: RefNo::new(24384, 26609),
            dbnum: 7997,
            source_sesno: 42,
            noun: "FLOOR".into(),
            mesh_id: "mesh".into(),
            world_transform: DMat4::IDENTITY,
            world_bounds: [[0.0; 3], [1.0; 3]],
            ancestors: vec![],
            shape,
            tube,
        }
    }

    #[test]
    fn project_cached_window_stops_at_the_page_and_reports_a_consumed_prefix() {
        let (start, end, consumed, more) = project_cached_window(10_001, false, 5_000, 5_000);
        assert_eq!((start, end, consumed, more), (5_000, 10_000, 5_000, true));

        let done = project_cached_window(8_000, true, 5_000, 5_000);
        assert_eq!(done, (5_000, 8_000, 5_000, false));

        let past = project_cached_window(3_000, true, 5_000, 5_000);
        assert_eq!(past, (0, 0, 3_000, false));

        let short = project_cached_window(0, true, 0, 5_000);
        assert_eq!(short, (0, 0, 0, false));
    }

    #[test]
    fn overlay_order_matches_durable_table_order_even_when_tube_id_sorts_first() {
        let mut tube = projected(
            ProjectedShape::Baked,
            Some(ProjectedTube {
                route_ordinal: 0,
                from: RefNo::new(7997, 1),
                to: RefNo::new(7997, 2),
                invalid: false,
            }),
        );
        tube.record_id = "0_tube".into();
        let mut first = projected(ProjectedShape::Baked, None);
        first.record_id = "a_element".into();
        let mut last = first.clone();
        last.record_id = "z_element".into();
        let mut rows = vec![tube, last, first];
        super::sort_overlay_records(&mut rows);
        assert_eq!(
            rows.iter()
                .map(|r| r.record_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a_element", "z_element", "0_tube"]
        );
    }

    #[test]
    fn memory_conversion_matches_the_durable_boolean_contract() {
        let root = RefNo::new(24384, 26606);
        let baked = super::projection_record_to_query(projected(ProjectedShape::Baked, None), root);
        assert!(baked.has_neg, "durable baked rows have booled_id");

        let tube = ProjectedTube {
            route_ordinal: 0,
            from: RefNo::new(24384, 1),
            to: RefNo::new(24384, 2),
            invalid: false,
        };
        let tube =
            super::projection_record_to_query(projected(ProjectedShape::Baked, Some(tube)), root);
        assert!(
            !tube.has_neg,
            "durable tubi_relate rows are never boolean rows"
        );
    }

    /// 房间判定的「网格 → 世界」在两条路上要对得上：投影记录直接算（逐根路径 / 整库重算对投影里的根）与
    /// 库上行回读（`durable_room_inputs`，整库重算对不在投影里的根；这里用 `projection_record_to_query`
    /// 折出与库行同形的查询行）。标准图元元素两边都是 `world × local`；**标准隐式管身**存下的世界变换已折进本地
    /// 摆放、库行实例变换是单位阵，投影记录那一侧也**不许再乘**一次本地摆放——此前多乘了（2026-09-17 修）。
    /// 库行经 f32 `Transform` 分解再合成，比到 f32 精度。
    #[test]
    fn a_canonical_implied_tube_has_the_same_mesh_to_world_on_the_projected_and_durable_paths() {
        use crate::fast_model::room_model::{RoomInput, durable_room_inputs};
        use e3d_model::primitive_instance::PrimitiveMeshKey;
        let root = RefNo::new(24384, 26606);
        let world = DMat4::from_scale_rotation_translation(
            glam::DVec3::ONE,
            glam::DQuat::from_rotation_z(0.7),
            glam::DVec3::new(10.0, 20.0, 30.0),
        );
        let local = DMat4::from_scale(glam::DVec3::new(100.0, 100.0, 3000.0));
        let canonical = |tube: Option<ProjectedTube>| {
            let mut record = projected(
                ProjectedShape::Canonical {
                    key: PrimitiveMeshKey::CylinderV3 { segments: 32 },
                    local_transform: local,
                    local_bounds: [[0.0; 3], [1.0; 3]],
                },
                tube,
            );
            // 与 `e3d_model_service::persisted_world_transform` 同口径：隐式管身把本地摆放折进存下的世界变换。
            record.world_transform = if record.tube.is_some() {
                world * local
            } else {
                world
            };
            record
        };
        let close = |a: DMat4, b: DMat4, what: &str| {
            for (x, y) in a.to_cols_array().iter().zip(b.to_cols_array()) {
                assert!(
                    (x - y).abs() <= 1e-3 * (1.0 + y.abs()),
                    "{what}: {a:?} vs {b:?}"
                );
            }
        };
        let tube = Some(ProjectedTube {
            route_ordinal: 0,
            from: RefNo::new(24384, 1),
            to: RefNo::new(24384, 2),
            invalid: false,
        });
        for (record, what) in [
            (canonical(None), "标准图元元素"),
            (canonical(tube), "标准隐式管身"),
        ] {
            let projected_path = record.mesh_to_world();
            close(
                projected_path,
                world * local,
                &format!("{what}：本地摆放只乘一次"),
            );
            let rows = vec![super::projection_record_to_query(record, root)];
            let durable = durable_room_inputs(&rows);
            assert_eq!(durable.len(), 1, "{what}");
            close(
                durable[0].mesh_to_world(),
                projected_path,
                &format!("{what}：库行回读与投影记录同一个「网格 → 世界」"),
            );
        }
    }

    #[tokio::test]
    async fn cold_read_includes_baked_canonical_and_tube_but_not_other_roots() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("cold_records").await.unwrap();
        let root: aios_core::RefnoEnum = "24384_26606".into();
        let packed = root.refno().0;
        db.query(format!(r#"
          CREATE aabb:box SET d={{mins:[0,0,0],maxs:[1,1,1]}};
          CREATE trans:world SET d={{translation:[120,0,0],rotation:[0,0,0,1],scale:[1,1,1]}};
          CREATE inst_relate:baked SET in=pe:⟨24384_26609⟩, anc=[{packed}],
            aabb=aabb:box,world_trans=trans:world,generic='FLOOR',booled_id='hole';
          CREATE inst_relate:canonical SET in=pe:⟨24384_26610⟩,anc=[{packed}],
            aabb=aabb:box,world_trans=trans:world,generic='BOX',
            insts_flat=[{{geo_hash:'box',transform:{{translation:[5,0,0],rotation:[0,0,0,1],scale:[2,3,4]}}}}];
          CREATE inst_relate:other SET in=pe:⟨24384_999⟩,anc=[1],
            aabb=aabb:box,world_trans=trans:world,generic='BOX',booled_id='other';
          CREATE tubi_relate:tube SET in=pe:⟨24384_26611⟩,anc=[{packed}],
            out=inst_geo:tube,aabb=aabb:box,world_trans=trans:world,invalid=true;
          -- 根自己的几何、`anc` 缺失的历史形态：只有 `in = 根` 那一路兜得住。
          CREATE inst_relate:own SET in=pe:⟨24384_26606⟩,anc=[1],
            aabb=aabb:box,world_trans=trans:world,generic='EQUI',booled_id='own';
          -- 根自己的管身、`anc` 含自身：两路都命中，并集里只许出现一次。
          CREATE tubi_relate:both SET in=pe:⟨24384_26606⟩,anc=[{packed}],
            out=inst_geo:tube,aabb=aabb:box,world_trans=trans:world;
        "#)).await.unwrap().check().unwrap();
        let records = super::durable_records_of_root(&db, root).await.unwrap();
        assert_eq!(
            records.len(),
            5,
            "两路并集：子树 2 + 根自己 1，管身 1 + 根自己的 1（去重）"
        );
        assert!(records.iter().all(|r| r.owner == root));
        assert_eq!(
            records
                .iter()
                .map(|r| r.refno.to_string())
                .collect::<Vec<_>>(),
            [
                "24384_26609",
                "24384_26610",
                "24384_26606",
                "24384_26606",
                "24384_26611"
            ],
            "inst_relate 先、tubi_relate 后，各按记录 id 排：与原来那一条 OR 的顺序相同"
        );
        assert_eq!(
            records[2].generic, "EQUI",
            "`in = 根` 那一路补进来的根自己的几何"
        );
        assert_eq!(
            records.iter().filter(|r| r.generic == "TUBI").count(),
            2,
            "两路都命中的管身行不重复"
        );
        let canonical = records.iter().find(|r| r.generic == "BOX").unwrap();
        assert_eq!(canonical.insts[0].transform.translation.x, 5.0);
        assert_eq!(canonical.world_trans.translation.x, 120.0);
        let tube = records
            .iter()
            .find(|r| r.generic == "TUBI" && r.refno.to_string() == "24384_26611")
            .unwrap();
        assert!(!canonical.has_neg);
        assert!(!tube.has_neg);
        assert!(tube.insts[0].is_tubi && tube.insts[0].is_invalid_tubi);
        let own_tube = records
            .iter()
            .find(|r| r.generic == "TUBI" && r.refno.to_string() == "24384_26606")
            .unwrap();
        assert!(own_tube.insts[0].is_tubi && !own_tube.insts[0].is_invalid_tubi);
        let baked = records.iter().find(|r| r.generic == "FLOOR").unwrap();
        assert!(baked.has_neg);
        assert_eq!(baked.insts[0].geo_hash, "hole");
    }

    /// 冷读的每一路作用域读都必须落在索引上（`anc CONTAINS` → `idx_<表>_anc`，`in =` → `idx_<表>_in`），
    /// 计划里不许出现 `Iterate Table`；两路合成一条 `… OR …` 时优化器整条退回扫表（改写前就是这样：
    /// 47,751 行的库上一根 0.76 s，整库房间重算逐根冷读跑成 90 分钟），所以并集只许在两路各自读完之后算。
    #[tokio::test]
    async fn durable_cold_read_scopes_are_planned_through_indexes() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("cold_read_plans").await.unwrap();
        db.query(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
            .await
            .unwrap()
            .check()
            .unwrap();
        let root: aios_core::RefnoEnum = "24384_26606".into();
        let (packed, owner) = (root.refno().0, root.to_pe_key());

        let sql = super::durable_records_sql(packed, &owner);
        let statements = sql
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(
            statements.len(),
            2,
            "inst_relate 一句、tubi_relate 一句: {sql}"
        );
        for statement in &statements {
            assert!(
                statement.starts_with("SELECT * FROM array::union((")
                    && statement.ends_with(")) ORDER BY record_order;"),
                "并集在库端算、并完再排: {statement}"
            );
            assert!(
                !statement.contains("OR in =") && !statement.contains("OR anc CONTAINS"),
                "两路作用域不许合成一条 OR 谓词——优化器会整条退回扫表: {statement}"
            );
        }

        for table in ["inst_relate", "tubi_relate"] {
            let reads = super::durable_scope_reads(table, "id", packed, &owner);
            for (read, index) in reads
                .iter()
                .zip([format!("idx_{table}_anc"), format!("idx_{table}_in")])
            {
                assert!(
                    statements.iter().any(|statement| {
                        statement.contains(read.split_once(" WHERE ").unwrap().1)
                    }),
                    "冷读 SQL 里找不到这一路的谓词: {read}"
                );
                let mut response = db
                    .query(format!("{read} EXPLAIN FULL;"))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                let plan: surrealdb::Value = response.take(0).unwrap();
                let plan = plan.to_string();
                assert!(
                    !plan.contains("Iterate Table") && plan.contains(&index),
                    "{read} must be planned through {index}, plan: {plan}"
                );
            }
        }

        // 对照：改写前的那一条 OR——单靠索引并集它能走索引，一挂上 AND 谓词就整条退回扫表。
        let mut response = db
            .query(format!(
                "SELECT id FROM inst_relate WHERE (anc CONTAINS {packed} OR in = {owner}) \
                 AND aabb.d != NONE AND world_trans.d != NONE EXPLAIN FULL;"
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        let plan: surrealdb::Value = response.take(0).unwrap();
        assert!(
            plan.to_string().contains("Iterate Table"),
            "the OR shape no longer falls back to a table scan on this engine—\
             the split is then only a style choice, update this note: {plan}"
        );
    }

    /// 冷读分页的两条纪律，2026-11-07 现场量出来的（ams7333 的 `23717/2`，79409 行）：
    ///
    /// - **不带 `ORDER BY`**：整根 `ORDER BY record_order` 要 3.3 s（排序整根），只加 `LIMIT` 是 3 ms。
    ///   分页后每页都是索引上的一次前缀扫描，所以整句里不许出现 `ORDER BY`。
    /// - **游标必须写成带引号的字符串**：`id > inst_relate:23717_179010`（record-id 字面量）每页 3.3 s，
    ///   索引退化；`id > 'inst_relate:23717_179010'` 是 0.33 s。
    ///
    /// 同时钉住：`LIMIT` 与游标必须落到**每一路**（`anc CONTAINS` 与 `in =`），否则截断只在并集之后做，
    /// 库端还是会把整根读出来——那正是 `23717/2` 报 `empty and closed channel` 的原因。
    #[test]
    fn durable_page_sql_pages_every_scope_without_ordering_the_whole_root() {
        for table in [
            super::DurableTable::InstRelate,
            super::DurableTable::TubiRelate,
        ] {
            let sql =
                super::durable_page_sql(table, 1234, "pe:1_2", Some("inst_relate:9_10"), 5000);
            assert!(
                !sql.contains("ORDER BY"),
                "分页句不许排序整根（3.3 s → 3 ms）: {sql}"
            );
            assert_eq!(
                sql.matches("LIMIT 5000").count(),
                if matches!(table, super::DurableTable::TubiRelate) {
                    4
                } else {
                    2
                },
                "LIMIT 必须落到每一路（tubi_relate 还有三格那一句）: {sql}"
            );
            assert_eq!(
                sql.matches("AND id > 'inst_relate:9_10'").count(),
                if matches!(table, super::DurableTable::TubiRelate) {
                    4
                } else {
                    2
                },
                "游标必须带引号、且落到每一路: {sql}"
            );
            assert!(
                !sql.contains("id > inst_relate:9_10"),
                "不带引号的 record-id 字面量会让索引退化（0.33 s → 3.3 s）: {sql}"
            );
        }

        // 首屏没有游标：只有 `LIMIT`，没有 `id >`。
        let first =
            super::durable_page_sql(super::DurableTable::InstRelate, 1234, "pe:1_2", None, 10);
        assert!(!first.contains("id >"), "首屏不该带游标: {first}");
        assert_eq!(first.matches("LIMIT 10").count(), 2, "{first}");
    }

    /// A6（2026-11-10）：读行上的扁平副本只换了「包围盒与世界变换从哪取」，别的一个字不动；游标谓词紧跟索引谓词、
    /// 排在「有几何」那两个谓词之前；管身行没有副本，两种写法逐字相同；开关只认明写的关。
    #[test]
    fn the_flat_read_only_changes_where_inst_rows_take_their_box_and_transform_from() {
        use super::DurableTable::{InstRelate, TubiRelate};
        let cursor = Some("inst_relate:9_10");
        let flat = super::durable_page_sql_with(InstRelate, 1234, "pe:1_2", cursor, 5000, true);
        let deref = super::durable_page_sql_with(InstRelate, 1234, "pe:1_2", cursor, 5000, false);
        assert!(
            flat.contains("aabb_d ?? aabb.d AS world_aabb")
                && flat.contains("world_trans_d ?? world_trans.d AS world_trans"),
            "{flat}"
        );
        assert!(
            flat.contains("(aabb_d ?? aabb.d) != NONE AND (world_trans_d ?? world_trans.d) != NONE"),
            "{flat}"
        );
        assert!(
            deref.contains("aabb.d AS world_aabb, world_trans.d AS world_trans")
                && !deref.contains("aabb_d")
                && !deref.contains("world_trans_d"),
            "{deref}"
        );
        assert_eq!(
            flat.replace("aabb_d ?? aabb.d", "aabb.d")
                .replace("world_trans_d ?? world_trans.d", "world_trans.d")
                .replace(
                    "(aabb.d) != NONE AND (world_trans.d) != NONE",
                    "aabb.d != NONE AND world_trans.d != NONE"
                ),
            deref,
            "两种写法只差这三处"
        );
        for sql in [&flat, &deref] {
            assert_eq!(
                sql.matches("WHERE anc CONTAINS 1234 AND id > inst_relate:9_10 AND ")
                    .count(),
                1,
                "{sql}"
            );
            assert_eq!(
                sql.matches("WHERE in = pe:1_2 AND id > inst_relate:9_10 AND ")
                    .count(),
                1,
                "{sql}"
            );
            assert!(
                !sql.contains("NONE AND id >"),
                "游标不许排在「有几何」的谓词后面——被滤掉的行会白付那两次判断: {sql}"
            );
        }
        assert_eq!(
            super::durable_page_sql_with(TubiRelate, 1234, "pe:1_2", cursor, 5000, true),
            super::durable_page_sql_with(TubiRelate, 1234, "pe:1_2", cursor, 5000, false),
            "管身行没有副本"
        );

        for on in [None, Some(""), Some("1"), Some("true"), Some("whatever")] {
            assert!(super::flat_read_from(on), "{on:?}");
        }
        for off in [Some("0"), Some(" FALSE "), Some("off"), Some("No")] {
            assert!(!super::flat_read_from(off), "{off:?}");
        }
    }

    /// A6：读副本的写法、带游标的写法仍然落在索引上（`??` 与 `id >` 都只是索引之后的残余谓词）。
    #[tokio::test]
    async fn flat_and_cursor_scope_reads_stay_on_the_indexes() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("flat_read_plans").await.unwrap();
        db.query(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
            .await
            .unwrap()
            .check()
            .unwrap();
        let root: aios_core::RefnoEnum = "24384_26606".into();
        let (packed, owner) = (root.refno().0, root.to_pe_key());
        for flat in [false, true] {
            for cursor in [None, Some("inst_relate:24384_26609")] {
                let reads = super::durable_scope_reads_from(
                    "inst_relate",
                    "id",
                    packed,
                    &owner,
                    cursor,
                    flat,
                );
                for (read, index) in reads
                    .iter()
                    .zip(["idx_inst_relate_anc", "idx_inst_relate_in"])
                {
                    let mut response = db
                        .query(format!("{read} EXPLAIN FULL;"))
                        .await
                        .unwrap()
                        .check()
                        .unwrap();
                    let plan: surrealdb::Value = response.take(0).unwrap();
                    let plan = plan.to_string();
                    assert!(
                        !plan.contains("Iterate Table") && plan.contains(index),
                        "{read} must be planned through {index}, plan: {plan}"
                    );
                }
            }
        }
    }

    /// A6：读扁平副本与逐条解引用回的内容相同。夹具里并排放：模型面写的行（副本与指针指向的值是同一份）、非布尔行
    /// （实例来自 `insts_flat`）、没有副本的老行（只能解引用）、指针悬空也没有副本的行（没有几何，两种写法都不给）、
    /// 根自己的几何（只有 `in = 根` 那一路兜得住）、别的根的行；其中一个元素带旧节点号。
    /// 一页读完、两行一页翻着读，两种写法逐条相同；翻着读不丢行、不重行。
    #[tokio::test]
    async fn flat_and_dereferencing_cold_reads_return_the_same_rows() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("flat_cold_read").await.unwrap();
        db.query(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
            .await
            .unwrap()
            .check()
            .unwrap();
        let root: aios_core::RefnoEnum = "24384_26606".into();
        let (packed, owner) = (root.refno().0, root.to_pe_key());
        db.query(format!(
            r#"
          CREATE pe:⟨24384_26609⟩ SET old_pe = pe:⟨9_9⟩;
          CREATE aabb:a SET d={{mins:[0,0,0],maxs:[1,2,3]}};
          CREATE aabb:b SET d={{mins:[-1,-1,-1],maxs:[4,5,6]}};
          CREATE trans:a SET d={{translation:[120,0,0],rotation:[0,0,0,1],scale:[1,1,1]}};
          CREATE trans:b SET d={{translation:[0,7,0],rotation:[0,0,0,1],scale:[1,1,1]}};
          CREATE inst_relate:⟨24384_26609⟩ SET in=pe:⟨24384_26609⟩, anc=[{packed}], generic='FLOOR', booled_id='hole',
            aabb=aabb:a, world_trans=trans:a,
            aabb_d={{mins:[0,0,0],maxs:[1,2,3]}},
            world_trans_d={{translation:[120,0,0],rotation:[0,0,0,1],scale:[1,1,1]}},
            insts_flat=[{{geo_hash:'hole'}}];
          CREATE inst_relate:⟨24384_26610⟩ SET in=pe:⟨24384_26610⟩, anc=[{packed}], generic='BOX',
            aabb=aabb:b, world_trans=trans:b,
            aabb_d={{mins:[-1,-1,-1],maxs:[4,5,6]}},
            world_trans_d={{translation:[0,7,0],rotation:[0,0,0,1],scale:[1,1,1]}},
            insts_flat=[{{geo_hash:'box',transform:{{translation:[5,0,0],rotation:[0,0,0,1],scale:[2,3,4]}}}}];
          CREATE inst_relate:⟨24384_26611⟩ SET in=pe:⟨24384_26611⟩, anc=[{packed}], generic='CYLI', booled_id='old',
            aabb=aabb:a, world_trans=trans:b;
          CREATE inst_relate:⟨24384_26612⟩ SET in=pe:⟨24384_26612⟩, anc=[{packed}], generic='GONE', booled_id='gone',
            aabb=aabb:missing, world_trans=trans:missing;
          CREATE inst_relate:⟨24384_26606⟩ SET in=pe:⟨24384_26606⟩, anc=[1], generic='EQUI', booled_id='own',
            aabb=aabb:b, world_trans=trans:a,
            aabb_d={{mins:[-1,-1,-1],maxs:[4,5,6]}},
            world_trans_d={{translation:[120,0,0],rotation:[0,0,0,1],scale:[1,1,1]}};
          CREATE inst_relate:⟨24384_999⟩ SET in=pe:⟨24384_999⟩, anc=[1], generic='BOX', booled_id='other',
            aabb=aabb:a, world_trans=trans:a;
        "#
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        // 与 `pull_durable_chunk` 同一个翻法：一页一句，游标取这一页最大的记录 id，不满一页就是读完了。
        async fn read_all(
            db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
            packed: u64,
            owner: &str,
            limit: usize,
            flat: bool,
        ) -> Vec<serde_json::Value> {
            let mut out = Vec::new();
            let mut cursor: Option<String> = None;
            for _ in 0..16 {
                let sql = super::durable_page_sql_with(
                    super::DurableTable::InstRelate,
                    packed,
                    owner,
                    cursor.as_deref(),
                    limit,
                    flat,
                );
                let mut response = db.query(sql).await.unwrap().check().unwrap();
                let rows: Vec<aios_core::GeomInstQuery> = response.take(0).unwrap();
                if rows.is_empty() {
                    break;
                }
                let short = rows.len() < limit;
                cursor = rows
                    .iter()
                    .map(|row| format!("inst_relate:{}", row.refno))
                    .max();
                out.extend(rows.iter().map(|row| serde_json::to_value(row).unwrap()));
                if short {
                    break;
                }
            }
            out
        }
        let sorted = |rows: &[serde_json::Value]| {
            let mut texts: Vec<String> = rows.iter().map(|row| row.to_string()).collect();
            texts.sort();
            texts
        };

        let whole_deref = read_all(&db, packed, &owner, 100, false).await;
        let whole_flat = read_all(&db, packed, &owner, 100, true).await;
        assert_eq!(whole_deref.len(), 4, "悬空的那一行与别的根的行不在里面: {whole_deref:?}");
        assert_eq!(whole_flat, whole_deref, "一页读完：两种写法逐条相同");
        let generics: Vec<&str> = whole_flat
            .iter()
            .map(|row| row["generic"].as_str().unwrap())
            .collect();
        assert_eq!(generics, ["FLOOR", "BOX", "CYLI", "EQUI"], "子树行按记录 id，根自己的几何在后");
        assert_eq!(
            whole_flat
                .iter()
                .filter(|row| !row["old_refno"].is_null())
                .count(),
            1,
            "旧节点号照旧带出来: {whole_flat:?}"
        );
        // 没有副本的老行读到的是指针指向的值：包围盒是 a 的、世界变换是 b 的。
        let legacy = &whole_flat[2];
        assert_eq!(legacy["world_trans"], whole_flat[1]["world_trans"]);
        assert_eq!(legacy["world_aabb"], whole_flat[0]["world_aabb"]);

        let paged_deref = read_all(&db, packed, &owner, 2, false).await;
        let paged_flat = read_all(&db, packed, &owner, 2, true).await;
        assert_eq!(paged_flat, paged_deref, "两行一页：两种写法逐条相同");
        assert_eq!(
            sorted(&paged_flat),
            sorted(&whole_flat),
            "翻着读不丢行、不重行"
        );
    }

    /// `/model/records` 的直段元数据（方案 B T2）：管身行的 `leave` / `arrive` / `route_ordinal` 折成记录一级的 `tube{ordinal, from, to}`；
    /// `route_ordinal` 没落盘的按同 `(容器, from, to)` 内出现序补号；两端缺失的历史行与 `inst_relate` 行没有 `tube`；
    /// `GeomInstQuery` 那一半与 `records_of_root` 逐字相同（多出来的三格它不认、原样忽略）；序列化时 `tube` 缺就不出键。
    #[tokio::test]
    async fn model_records_carry_tube_identity_from_durable_rows_and_number_legacy_ordinals() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test")
            .use_db("model_records_tube")
            .await
            .unwrap();
        let root: aios_core::RefnoEnum = "24384_26606".into();
        let packed = root.refno().0;
        db.query(format!(r#"
          CREATE aabb:box SET d={{mins:[0,0,0],maxs:[1,1,1]}};
          CREATE trans:world SET d={{translation:[120,0,0],rotation:[0,0,0,1],scale:[1,1,1]}};
          CREATE inst_relate:baked SET in=pe:⟨24384_26609⟩, anc=[{packed}],
            aabb=aabb:box,world_trans=trans:world,generic='FLOOR',booled_id='hole';
          -- 三段同一容器：前两段同 (leave, arrive) 且没有 route_ordinal（按出现序补 0、1）；第三段落了盘的 route_ordinal=5 原样
          CREATE tubi_relate:[pe:⟨24384_26606⟩,0] SET in=pe:⟨24384_26606⟩,anc=[{packed}],out=inst_geo:tube,
            aabb=aabb:box,world_trans=trans:world,leave=pe:⟨24384_1⟩,arrive=pe:⟨24384_2⟩;
          CREATE tubi_relate:[pe:⟨24384_26606⟩,1] SET in=pe:⟨24384_26606⟩,anc=[{packed}],out=inst_geo:tube,
            aabb=aabb:box,world_trans=trans:world,leave=pe:⟨24384_1⟩,arrive=pe:⟨24384_2⟩,invalid=true;
          CREATE tubi_relate:[pe:⟨24384_26606⟩,2] SET in=pe:⟨24384_26606⟩,anc=[{packed}],out=inst_geo:tube,
            aabb=aabb:box,world_trans=trans:world,leave=pe:⟨24384_2⟩,arrive=pe:⟨24384_3⟩,route_ordinal=5;
          -- 两端缺失的历史行：还是一条 TUBI 记录，只是没有 tube
          CREATE tubi_relate:[pe:⟨24384_26606⟩,3] SET in=pe:⟨24384_26606⟩,anc=[{packed}],out=inst_geo:tube,
            aabb=aabb:box,world_trans=trans:world;
        "#)).await.unwrap().check().unwrap();

        let records = super::durable_model_records_of_root(&db, root)
            .await
            .unwrap();
        assert_eq!(records.len(), 5, "1 构件 + 4 管身");
        assert!(records[0].tube.is_none() && records[0].inst.generic == "FLOOR");
        let tubes = records[1..]
            .iter()
            .map(|record| (record.inst.generic.as_str(), record.tube.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            tubes,
            [
                (
                    "TUBI",
                    Some(super::TubeMeta {
                        ordinal: 0,
                        from: "24384_1".into(),
                        to: "24384_2".into()
                    })
                ),
                (
                    "TUBI",
                    Some(super::TubeMeta {
                        ordinal: 1,
                        from: "24384_1".into(),
                        to: "24384_2".into()
                    })
                ),
                (
                    "TUBI",
                    Some(super::TubeMeta {
                        ordinal: 5,
                        from: "24384_2".into(),
                        to: "24384_3".into()
                    })
                ),
                ("TUBI", None),
            ]
        );
        assert!(records[2].inst.insts[0].is_invalid_tubi);

        // `GeomInstQuery` 那一半与 records_of_root 同形同序
        let plain = super::durable_records_of_root(&db, root).await.unwrap();
        assert_eq!(plain.len(), records.len());
        for (a, b) in plain.iter().zip(&records) {
            assert_eq!(
                (
                    a.refno.to_string(),
                    a.generic.clone(),
                    a.insts[0].geo_hash.clone()
                ),
                (
                    b.inst.refno.to_string(),
                    b.inst.generic.clone(),
                    b.inst.insts[0].geo_hash.clone()
                )
            );
        }

        let json = serde_json::to_value(&records).unwrap();
        assert!(json[0].get("tube").is_none(), "构件行没有 tube 键");
        assert_eq!(
            json[1]["tube"],
            serde_json::json!({ "ordinal": 0, "from": "24384_1", "to": "24384_2" })
        );
        assert_eq!(
            json[1]["generic"], "TUBI",
            "flatten：GeomInstQuery 的键仍在顶层"
        );
        assert!(json[1]["insts"][0]["is_tubi"].as_bool().unwrap());
        assert!(
            json[4].get("tube").is_none(),
            "两端缺失的历史行没有 tube 键"
        );
    }

    // ---- 房间层级树 tubes=1 的管身冷读 ----

    /// 两条 BRAN 各两段管身 + 一条缺 `arrive` 的历史行 + 一条别的 BRAN 的：只按 `in` 取、缺端的在库端滤掉、
    /// 行按槽位排、`route_ordinal` 没落盘的读成 `None`、落了的原样读回；两根一次往返、顺序 = 入参顺序 × 槽位序。
    #[tokio::test]
    async fn durable_tube_rows_come_back_per_branch_in_slot_order_without_endless_rows() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("tube_rows").await.unwrap();
        db.query(
            r#"
          CREATE aabb:t0 SET d={mins:[0,0,0],maxs:[100,10,10]};
          CREATE aabb:t1 SET d={mins:[100,0,0],maxs:[300,10,10]};
          CREATE trans:t0 SET d={translation:[50,5,5],rotation:[0,0,0,1],scale:[5,5,100]};
          CREATE trans:t1 SET d={translation:[200,5,5],rotation:[0,0,0,1],scale:[5,5,200]};
          INSERT RELATION INTO tubi_relate [
            {id:tubi_relate:[pe:⟨24384_100⟩,1],in:pe:⟨24384_100⟩,out:inst_geo:m,leave:pe:⟨24384_102⟩,arrive:pe:⟨24384_103⟩,
             aabb:aabb:t1,world_trans:trans:t1,invalid:true,anc:[1],dbnum:24384},
            {id:tubi_relate:[pe:⟨24384_100⟩,0],in:pe:⟨24384_100⟩,out:inst_geo:m,leave:pe:⟨24384_101⟩,arrive:pe:⟨24384_102⟩,
             aabb:aabb:t0,world_trans:trans:t0,invalid:false,anc:[1],dbnum:24384,route_ordinal:2},
            {id:tubi_relate:[pe:⟨24384_100⟩,2],in:pe:⟨24384_100⟩,out:inst_geo:m,leave:pe:⟨24384_103⟩,
             aabb:aabb:t0,world_trans:trans:t0,anc:[1],dbnum:24384},
            {id:tubi_relate:[pe:⟨24384_200⟩,0],in:pe:⟨24384_200⟩,out:inst_geo:m,leave:pe:⟨24384_200⟩,arrive:pe:⟨24384_201⟩,
             aabb:aabb:t0,world_trans:trans:t0,anc:[1],dbnum:24384},
            {id:tubi_relate:[pe:⟨24384_300⟩,0],in:pe:⟨24384_300⟩,out:inst_geo:m,leave:pe:⟨24384_301⟩,arrive:pe:⟨24384_302⟩,
             aabb:aabb:t0,world_trans:trans:t0,anc:[1],dbnum:24384}
          ];
        "#,
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let branches = [
            RefU64::from_two_nums(24384, 100),
            RefU64::from_two_nums(24384, 200),
        ];
        let rows = super::durable_tube_rows_of_branches(&db, &branches)
            .await
            .unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (
                    row.container.as_str(),
                    row.tube_from.as_str(),
                    row.tube_to.as_str(),
                    row.tube_ordinal,
                    row.invalid
                ))
                .collect::<Vec<_>>(),
            [
                ("24384_100", "24384_101", "24384_102", Some(2), false),
                ("24384_100", "24384_102", "24384_103", None, true),
                ("24384_200", "24384_200", "24384_201", None, false),
            ],
            "槽位 0 在 1 前、缺 arrive 的不来、别的 BRAN 不来、两根按入参顺序"
        );
        assert_eq!(rows[1].world_aabb.maxs.x, 300.0);
        assert_eq!(
            rows[1].world_trans.scale.z, 200.0,
            "长度就在世界变换的 z 缩放里"
        );
        assert!(
            super::durable_tube_rows_of_branches(&db, &[])
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 管身冷读每句都要落在 `idx_tubi_relate_in` 上、不许 `Iterate Table`（与 `durable_cold_read_scopes_are_planned_through_indexes`
    /// 同一条纪律）：`tubes=1` 在落盘形态下对每个 BRAN 单元来一句，走不上索引就是「每根扫一遍 tubi_relate」。
    #[tokio::test]
    async fn durable_tube_rows_are_planned_through_the_in_index() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("tube_rows_plan").await.unwrap();
        db.query(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
            .await
            .unwrap()
            .check()
            .unwrap();
        let branches = [
            RefU64::from_two_nums(24384, 100),
            RefU64::from_two_nums(24384, 200),
        ];
        let sql = super::durable_tube_rows_sql(&branches);
        let statements = sql.lines().collect::<Vec<_>>();
        assert_eq!(statements.len(), 2, "每根一句: {sql}");
        for (statement, branch) in statements.iter().zip(&branches) {
            assert!(
                statement.contains(&format!(
                    "WHERE in = {}",
                    RefnoEnum::from(*branch).to_pe_key()
                )) && !statement.contains("anc CONTAINS")
                    && !statement.contains(" IN ["),
                "只按 in 一路等值: {statement}"
            );
            let explain = statement
                .trim_end_matches(';')
                .split(" ORDER BY ")
                .next()
                .unwrap()
                .to_string()
                + " EXPLAIN FULL;";
            let mut response = db.query(explain).await.unwrap().check().unwrap();
            let plan: surrealdb::Value = response.take(0).unwrap();
            let plan = plan.to_string();
            assert!(
                !plan.contains("Iterate Table") && plan.contains("idx_tubi_relate_in"),
                "{statement} must be planned through idx_tubi_relate_in, plan: {plan}"
            );
        }
    }
}
