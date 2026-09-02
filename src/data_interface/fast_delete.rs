//! Fast, Ref0-range based removal of one DBNUM's persisted data.

use std::time::Instant;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;

use aios_core::SUL_DB;

const RANGE_END: &str = "9999999999";
const RANGE_TABLES: &[&str] = &[
    "pe",
    "inst_relate",
    "tubi_relate",
    "room_relate",
    // `room_panel_relate` 是 `room_relate` 的同源姐妹边：两者由同一面板重算入口
    // 先清后写维护（room_model 里从无全表清空，只按 in={room}/out={panel} 逐实体
    // DELETE），且其 record id `{room_refno}_{panel}` 与 `room_relate` 同为 Ref0
    // 前缀、可按 Ref0 区间寻址。少了它，回退整库重建（ResetForReinit）删掉了
    // room_relate 却把 room_panel_relate 留下——回退是文件退回更旧会话，重建后
    // 房间重算只对**当前存在**的房间/面板先清后写，此前存在、回退后不复存在的
    // 房间/面板留下悬空 room_panel_relate 边，两表就此对不上（ADR-010 D4 幽灵
    // 形态的同类）。
    "room_panel_relate",
    "ref_rev",
    "geo_relate",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FastDeleteDbnumResult {
    pub dbnum: u32,
    pub ref0s: Vec<String>,
    pub pe_rows: usize,
    pub noun_tables: usize,
    pub range_statements: usize,
    pub elapsed_ms: u64,
}

#[derive(Debug, Deserialize)]
struct Ref0Row {
    prefix: String,
    count: usize,
}

#[derive(Debug, Deserialize)]
struct InfoRef0Row {
    ref0: String,
}

#[derive(Debug, Deserialize)]
struct NounRow {
    noun: String,
}

#[derive(Debug, Deserialize)]
struct CountRow {
    count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PruneAboveWatermarkPreview {
    pub dbnum: u32,
    pub target_watermark: i32,
    pub applied_sesno: i32,
    pub pe_rows: usize,
    pub sample_refnos: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PruneAboveWatermarkResult {
    #[serde(flatten)]
    pub preview: PruneAboveWatermarkPreview,
    pub deleted_pe_rows: usize,
    pub rebuilt_info_rows: usize,
    pub remaining_rows_above_watermark: usize,
    pub elapsed_ms: u64,
}

#[derive(Debug, Deserialize)]
struct WatermarkRow {
    applied_sesno: Option<i32>,
    file_name: Option<String>,
    db_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PrunePeRow {
    id: Thing,
    noun: String,
}

fn valid_table_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn range_of(table: &str, ref0: &str) -> String {
    format!("{table}:{ref0}_0..{ref0}_{RANGE_END}")
}

/// 本 Ref0 下全部 owner 的成员块，按 `pe_owner` 的复合 record id 一段区间圈住。
///
/// 边 id 固定是 `[OWNER_PE, 槽位]`（`versioned_db::pe` 与 `cata_closure` 两个写口
/// 同形），所以 owner 落在 Ref0 区间内的边天然是 id 连续的一段，不必再从 `pe` 侧
/// 图遍历把边 id 全捞进内存——百万级 PE 的库上，那两句 `array::flatten(SELECT
/// VALUE ->/<-pe_owner …)` 正是整库清理的耗时大头。顺带换来幂等：本语句不读
/// `pe`，上一次清库半途失败、`pe` 行已没而边还在时，下一轮仍能把边清掉（图遍历
/// 从空区间出发永远够不着它们）。
///
/// 少掉的 `->pe_owner` 方向（child 在本 Ref0、owner 在别处）在**整库**清理里是
/// 空扫：所有权链不跨库，而本 dbnum 解析出的每个 Ref0 都会各出一条本语句，
/// 同库跨 Ref0 的边由 owner 自己那条兜住。
///
/// 这个**跨 owner** 的区间形状恰是 `staging::replay_safe::is_owner_scoped_range`
/// 明令拒绝的写法。那道闸门守的是重放路径上「圈到别人头上还不报错」；这里两端
/// Ref0 同源、由权威 Ref0 集合导出，整段区间本来就要全删，所以安全——**仅限**
/// 整库清理。按水位裁剪只删一部分元素，不满足这个前提，继续走逐元素双向删除。
fn owner_range_of(ref0: &str) -> String {
    format!("pe_owner:[pe:{ref0}_0, NONE]..=[pe:{ref0}_{RANGE_END}, ..]")
}

fn collect_ref0s(
    prefix_rows: Vec<Ref0Row>,
    info_rows: Vec<InfoRef0Row>,
) -> anyhow::Result<(usize, Vec<String>)> {
    let pe_rows = prefix_rows.iter().map(|row| row.count).sum();
    let mut ref0s = prefix_rows
        .into_iter()
        .map(|row| {
            row.prefix
                .strip_prefix("pe:")
                .filter(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
                .map(str::to_owned)
                .with_context(|| format!("unexpected PE id prefix: {}", row.prefix))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    for row in info_rows {
        if row.ref0.is_empty() || !row.ref0.bytes().all(|b| b.is_ascii_digit()) {
            bail!("unexpected dbnum_info_table Ref0: {}", row.ref0);
        }
        ref0s.push(row.ref0);
    }
    ref0s.sort_unstable();
    ref0s.dedup();
    Ok((pe_rows, ref0s))
}

fn pe_key(id: &Thing) -> anyhow::Result<String> {
    let rendered = id.to_string();
    let key = rendered
        .strip_prefix("pe:")
        .filter(|key| {
            !key.is_empty()
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'_')
        })
        .with_context(|| format!("unexpected PE record id: {rendered}"))?;
    Ok(key.to_owned())
}

async fn inspect_prune_rows(
    dbnum: u32,
    target_watermark: i32,
) -> anyhow::Result<(WatermarkRow, Vec<PrunePeRow>)> {
    if dbnum == 0 {
        bail!("dbnum must be greater than zero");
    }
    if target_watermark < 0 {
        bail!("target watermark must be non-negative");
    }
    let mut response = SUL_DB
        .query(format!(
            "SELECT applied_sesno, file_name, db_type FROM ONLY dbnum_watermark:{dbnum};\n\
             SELECT id, noun FROM pe WHERE dbnum = {dbnum} AND <int>sesno > {target_watermark} ORDER BY id;"
        ))
        .await
        .context("inspect rows above watermark")?
        .check()
        .context("inspect rows above watermark statement")?;
    let state = response
        .take::<Option<WatermarkRow>>(0)
        .context("decode dbnum watermark")?
        .with_context(|| format!("dbnum {dbnum} has no registered watermark"))?;
    let applied = state.applied_sesno.unwrap_or_default();
    if target_watermark > applied {
        bail!(
            "target watermark {target_watermark} is above applied_sesno {applied} for dbnum {dbnum}"
        );
    }
    let rows = response
        .take::<Vec<PrunePeRow>>(1)
        .context("decode rows above watermark")?;
    Ok((state, rows))
}

pub async fn preview_prune_above_watermark(
    dbnum: u32,
    target_watermark: i32,
) -> anyhow::Result<PruneAboveWatermarkPreview> {
    let (state, rows) = inspect_prune_rows(dbnum, target_watermark).await?;
    let mut sample_refnos = rows
        .iter()
        .take(20)
        .map(|row| pe_key(&row.id))
        .collect::<anyhow::Result<Vec<_>>>()?;
    sample_refnos.sort_unstable();
    Ok(PruneAboveWatermarkPreview {
        dbnum,
        target_watermark,
        applied_sesno: state.applied_sesno.unwrap_or_default(),
        pe_rows: rows.len(),
        sample_refnos,
    })
}

/// Remove partial/latest-state rows whose per-element session is newer than
/// the selected authoritative watermark, then lower the durable watermark and
/// rebuild the legacy Ref0 statistics from the surviving PE snapshot.
///
/// This is a residue cleanup operation, not a historical-version restore: PE
/// stores one latest row per refno. The normal watcher replays the now-pending
/// sessions after this operation.
pub async fn prune_above_watermark(
    dbnum: u32,
    target_watermark: i32,
) -> anyhow::Result<PruneAboveWatermarkResult> {
    let started = Instant::now();
    let _commit_guard = crate::data_interface::batch_worker::DATA_COMMIT_SERIAL
        .lock()
        .await;
    let _state_guard = crate::data_interface::dbnum_state::DBNUM_STATE_WRITE_GATE
        .write()
        .await;
    let (state, rows) = inspect_prune_rows(dbnum, target_watermark).await?;
    let applied_sesno = state.applied_sesno.unwrap_or_default();
    let mut statements = Vec::with_capacity(rows.len() * 6 + 4);
    let mut sample_refnos = Vec::new();
    for row in &rows {
        if !valid_table_name(&row.noun) {
            bail!("invalid noun table name for dbnum {dbnum}: {}", row.noun);
        }
        let key = pe_key(&row.id)?;
        if sample_refnos.len() < 20 {
            sample_refnos.push(key.clone());
        }
        statements.push(format!(
            "DELETE array::flatten(SELECT VALUE ->pe_owner FROM pe:{key});"
        ));
        statements.push(format!(
            "DELETE array::flatten(SELECT VALUE <-pe_owner FROM pe:{key});"
        ));
        for table in RANGE_TABLES.iter().filter(|table| **table != "pe") {
            statements.push(format!("DELETE {table}:{key};"));
        }
        statements.push(format!("DELETE {}:{key};", row.noun));
        statements.push(format!("DELETE pe:{key};"));
    }
    statements.push(format!(
        "DELETE model_update_pending WHERE dbnum = {dbnum} AND <int>source_end_sesno > {target_watermark};"
    ));
    statements.push(format!(
        "DELETE increment_update_attempt WHERE dbnum = {dbnum} AND <int>end_sesno > {target_watermark};"
    ));
    statements.push(format!(
        "DELETE incr_side_effect_pending WHERE dbnum = {dbnum} AND <int>end_sesno > {target_watermark};"
    ));
    statements.push(format!(
        "UPDATE dbnum_watermark:{dbnum} SET applied_sesno = {target_watermark}, sesno = {target_watermark};"
    ));
    execute_phase("prune rows above watermark", &statements).await?;

    // A zero-row cleanup already proved the PE postcondition while both write
    // gates were held. Avoid two full PE scans in that common diagnostic case.
    let (rebuilt_info_rows, remaining_rows_above_watermark) = if rows.is_empty() {
        (0, 0)
    } else {
        let rebuilt = crate::versioned_db::database::rebuild_dbnum_info_from_pe(
            dbnum,
            state.file_name.as_deref().unwrap_or_default(),
            state.db_type.as_deref().unwrap_or_default(),
        )
        .await?;
        let remaining = preview_prune_above_watermark(dbnum, target_watermark)
            .await?
            .pe_rows;
        (rebuilt, remaining)
    };
    if remaining_rows_above_watermark != 0 {
        bail!(
            "dbnum {dbnum} prune incomplete: {remaining_rows_above_watermark} PE rows remain above watermark {target_watermark}"
        );
    }
    sample_refnos.sort_unstable();
    Ok(PruneAboveWatermarkResult {
        preview: PruneAboveWatermarkPreview {
            dbnum,
            target_watermark,
            applied_sesno,
            pe_rows: rows.len(),
            sample_refnos,
        },
        deleted_pe_rows: rows.len(),
        rebuilt_info_rows,
        remaining_rows_above_watermark,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// 元数据阶段对水位行的两种处置。
///
/// 快删端点（运维排障）删行——库从此回到「从未登记」；回退重建（ADR-021）
/// **清值不删行**——登记身份必须原地留下，否则下一轮 classify 会把这个库误判
/// 成首次登记，而且删行会让启动播种从 `dbnum_info_table` 把旧水位灌回来
/// （2026-08-04 播种审计第 5 条；统计行本身也在同一阶段清空，双保险）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatermarkDisposal {
    DropRow,
    ResetForReinit,
}

/// Render separate checked phases. A giant optimistic transaction conflicts
/// with the watcher's periodic observation write on large DBNUMs. Metadata is
/// deliberately last, so a failed data phase never advertises an initialized
/// database as cleanly deleted; within it the watermark disposal is the final
/// statement — it is the wipe's commit point, so a half-done wipe keeps the
/// old watermark and the next verdict retries idempotently.
fn render_delete_phases(
    dbnum: u32,
    ref0s: &[String],
    noun_tables: &[String],
    watermark: WatermarkDisposal,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut relations = Vec::new();
    let mut ranges = Vec::new();
    for ref0 in ref0s {
        relations.push(format!("DELETE {};", owner_range_of(ref0)));
        for table in RANGE_TABLES.iter().filter(|table| **table != "pe") {
            ranges.push(format!("DELETE {};", range_of(table, ref0)));
        }
        for table in noun_tables {
            ranges.push(format!("DELETE {};", range_of(table, ref0)));
        }
        ranges.push(format!("DELETE {};", range_of("pe", ref0)));
    }
    let mut metadata = vec![
        format!("DELETE model_update_pending WHERE dbnum = {dbnum};"),
        format!("DELETE increment_update_attempt WHERE dbnum = {dbnum};"),
        format!("DELETE incr_side_effect_pending WHERE dbnum = {dbnum};"),
        format!("DELETE dbnum_info_table WHERE dbnum = {dbnum};"),
    ];
    match watermark {
        WatermarkDisposal::DropRow => {
            metadata.push(format!("DELETE dbnum_watermark:{dbnum};"));
        }
        WatermarkDisposal::ResetForReinit => {
            // 清库删掉了 room_relate / inst_relate 行：epoch 不递增的话，崩溃
            // 重启会按指纹相等复用一棵还留着被删构件包围盒的树（ADR-010 D4 的
            // 幽灵形态借崩溃复活）。放在水位处置之前、同一元数据阶段提交。
            metadata.push(crate::fast_model::aabb_tree::render_spatial_epoch_bump());
            // 同时抹掉指向旧历史的 applied_sesno_time（那条保存在当前文件里已
            // 不存在），基线收口会写上新的；legacy `sesno` 字段与 applied 同步
            // 归零，读侧迁移才不会把旧值当水位捞回来。
            metadata.push(format!(
                "UPDATE dbnum_watermark:{dbnum} SET applied_sesno = 0, sesno = 0, \
                 applied_sesno_time = NONE, confirmed_empty_baseline_sesno = NONE;"
            ));
        }
    }
    (relations, ranges, metadata)
}

/// 清库后置条件：PE 归零，且每个 Ref0 的 `pe_owner` 区间也归零。
///
/// 边现在按 id 区间删，残留检查就落在同一套坐标里——一次区间扫描的代价，换来
/// 这条语句自证：只数 PE 的话，删边语句写歪（区间圈错、上界漏写被引擎默默接受）
/// 不会让任何东西喊一声。语句顺序与 `ref0s` 一致，调用方按下标逐个取。
fn render_verify_query(dbnum: u32, ref0s: &[String]) -> String {
    let mut sql = format!("SELECT count() AS count FROM pe WHERE dbnum = {dbnum} GROUP ALL;");
    for ref0 in ref0s {
        sql.push_str(&format!(
            "\nSELECT count() AS count FROM {} GROUP ALL;",
            owner_range_of(ref0)
        ));
    }
    sql
}

async fn execute_phase(label: &str, statements: &[String]) -> anyhow::Result<()> {
    if statements.is_empty() {
        return Ok(());
    }
    SUL_DB
        .query(statements.join("\n"))
        .await
        .with_context(|| format!("{label} transport failed"))?
        .check()
        .with_context(|| format!("{label} statement failed"))?;
    Ok(())
}

fn render_transactional_phase(statements: &[String]) -> Option<String> {
    (!statements.is_empty()).then(|| {
        format!(
            "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
            statements.join("\n")
        )
    })
}

async fn execute_transactional_phase(label: &str, statements: &[String]) -> anyhow::Result<()> {
    let Some(sql) = render_transactional_phase(statements) else {
        return Ok(());
    };
    SUL_DB
        .query(sql)
        .await
        .with_context(|| format!("{label} transport failed"))?
        .check()
        .with_context(|| format!("{label} statement failed"))?;
    Ok(())
}

/// Delete all persisted rows owned by `dbnum` using Ref0 record-id ranges.
///
/// The HTTP caller stops dispatch first. These locks additionally serialize
/// against staged commit and scan-observation writes for internal callers.
pub async fn delete_dbnum_fast(dbnum: u32) -> anyhow::Result<FastDeleteDbnumResult> {
    wipe_dbnum_rows(dbnum, WatermarkDisposal::DropRow).await
}

/// 回退整库重建的清库半边（ADR-021）：数据、派生行、noun 行、统计与队列残留
/// 全删，但水位行**清值不删行**（登记身份保留、`applied_sesno = 0`）并在同一
/// 元数据阶段递增 spatial epoch。清完恰好落进 worker 现成的
/// `needs_initial_load` → `initialize_dbnum_baseline` 分支，由基线按首次导入
/// 重新解析当前文件。
///
/// 调用方限定：只在冻结点复核仍判回退（`FileAnomaly::requires_reinit`）时由
/// 数据批次 worker 执行体调用——扫描路径只分类入队，破坏性动作必须留在
/// `startup_autorun` / 队列暂停这道闸门之内（源码钉 `scan_paths_never_wipe`）。
/// 失败时水位处置尚未执行（它是元数据阶段的最后一句），下一轮仍判回退、
/// 幂等重放。
pub(crate) async fn wipe_dbnum_for_reinit(dbnum: u32) -> anyhow::Result<FastDeleteDbnumResult> {
    wipe_dbnum_rows(dbnum, WatermarkDisposal::ResetForReinit).await
}

async fn wipe_dbnum_rows(
    dbnum: u32,
    watermark: WatermarkDisposal,
) -> anyhow::Result<FastDeleteDbnumResult> {
    if dbnum == 0 {
        bail!("dbnum must be greater than zero");
    }
    let started = Instant::now();
    let _commit_guard = crate::data_interface::batch_worker::DATA_COMMIT_SERIAL
        .lock()
        .await;
    let _state_guard = crate::data_interface::dbnum_state::DBNUM_STATE_WRITE_GATE
        .write()
        .await;

    let mut response = SUL_DB
        .query(format!(
            "SELECT string::split(<string>id, '_')[0] AS prefix, count() AS count \
             FROM pe WHERE dbnum = {dbnum} GROUP BY prefix;\n\
             SELECT noun FROM pe WHERE dbnum = {dbnum} GROUP BY noun;\n\
             SELECT <string>record::id(id) AS ref0 FROM dbnum_info_table \
             WHERE dbnum = {dbnum} GROUP BY ref0;"
        ))
        .await
        .context("inspect dbnum rows for fast delete")?
        .check()
        .context("inspect dbnum rows for fast delete statement")?;
    let prefix_rows = response
        .take::<Vec<Ref0Row>>(0)
        .context("decode Ref0 groups")?;
    let noun_rows = response
        .take::<Vec<NounRow>>(1)
        .context("decode noun groups")?;
    let info_ref0_rows = response
        .take::<Vec<InfoRef0Row>>(2)
        .context("decode dbnum_info_table Ref0 groups")?;

    // `dbnum_info_table` 的 record id 才是这个 dbnum 所属的 Ref0。不能把 dbnum
    // 数值直接拼进 record range；例如 dbnum=7333 的真实 Ref0 是 23717/31909。
    // 同时从 PE 与统计表取并集：正常库由 PE 覆盖，PE 零行的幽灵水位仍能依靠
    // 启动播种所用的统计行找到 Ref0，继续走同一套 id-range 清理。
    let (pe_rows, ref0s) = collect_ref0s(prefix_rows, info_ref0_rows)
        .with_context(|| format!("resolve Ref0 ranges for dbnum {dbnum}"))?;

    let mut noun_tables = noun_rows
        .into_iter()
        .map(|row| row.noun)
        .collect::<Vec<_>>();
    noun_tables.sort_unstable();
    noun_tables.dedup();
    if let Some(invalid) = noun_tables.iter().find(|name| !valid_table_name(name)) {
        bail!("invalid noun table name for dbnum {dbnum}: {invalid}");
    }

    let (relations, ranges, metadata) =
        render_delete_phases(dbnum, &ref0s, &noun_tables, watermark);
    execute_phase("delete owner relations", &relations).await?;
    execute_phase("delete Ref0 ranges", &ranges).await?;
    execute_transactional_phase("delete dbnum metadata", &metadata).await?;

    let mut verify = SUL_DB
        .query(render_verify_query(dbnum, &ref0s))
        .await
        .context("verify fast delete")?
        .check()
        .context("verify fast delete statement")?;
    let remaining = verify
        .take::<Vec<CountRow>>(0)
        .context("decode fast delete verification")?
        .first()
        .map(|row| row.count)
        .unwrap_or_default();
    if remaining != 0 {
        bail!("dbnum {dbnum} fast delete incomplete: {remaining} PE rows remain");
    }
    let mut remaining_owner_edges = 0usize;
    for (offset, ref0) in ref0s.iter().enumerate() {
        remaining_owner_edges += verify
            .take::<Vec<CountRow>>(offset + 1)
            .with_context(|| format!("decode pe_owner residue for Ref0 {ref0}"))?
            .first()
            .map(|row| row.count)
            .unwrap_or_default();
    }
    if remaining_owner_edges != 0 {
        bail!(
            "dbnum {dbnum} fast delete incomplete: {remaining_owner_edges} pe_owner edges remain in the Ref0 id ranges"
        );
    }

    Ok(FastDeleteDbnumResult {
        dbnum,
        ref0s,
        pe_rows,
        noun_tables: noun_tables.len(),
        range_statements: ranges.len(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_ref0_ranges_and_keeps_metadata_last() {
        let (relations, ranges, metadata) = render_delete_phases(
            7997,
            &["24381".into()],
            &["EQUI".into(), "PANE".into()],
            WatermarkDisposal::DropRow,
        );
        assert_eq!(
            relations,
            vec!["DELETE pe_owner:[pe:24381_0, NONE]..=[pe:24381_9999999999, ..];".to_string()]
        );
        assert!(ranges.contains(&"DELETE EQUI:24381_0..24381_9999999999;".into()));
        assert!(ranges.contains(&"DELETE inst_relate:24381_0..24381_9999999999;".into()));
        assert_eq!(
            ranges.last().unwrap(),
            "DELETE pe:24381_0..24381_9999999999;"
        );
        assert_eq!(metadata.last().unwrap(), "DELETE dbnum_watermark:7997;");
    }

    /// owner 边走复合 id 区间，整库清理的 SQL 里不该再剩任何图遍历。
    ///
    /// 遍历要先把边 id 全捞进内存（百万级 PE 的库上就是清库耗时的大头），而且它读
    /// `pe`：上一次清库半途失败、`pe` 行已没而边还在时，从空区间出发的遍历再也够
    /// 不着那些边。
    #[test]
    fn the_owner_edges_go_by_id_range_not_by_graph_traversal() {
        let ref0s = ["23717".to_string(), "31909".to_string()];
        let (relations, ranges, metadata) = render_delete_phases(
            7333,
            &ref0s,
            &["EQUI".into()],
            WatermarkDisposal::ResetForReinit,
        );
        let rendered = [relations.clone(), ranges, metadata].concat().join("\n");
        assert!(
            !rendered.contains("->pe_owner") && !rendered.contains("<-pe_owner"),
            "整库清理不得再走 pe_owner 图遍历: {rendered}"
        );
        assert_eq!(
            relations,
            ref0s
                .iter()
                .map(|ref0| format!("DELETE {};", owner_range_of(ref0)))
                .collect::<Vec<_>>(),
            "每个 Ref0 只出一条 owner 区间删除"
        );
    }

    /// 上界漏写与跨 Ref0 是这条语句仅有的两个致命写法：前者一路删到表尾，后者圈到
    /// 别人头上，两者执行时都不报错。`staging::replay_safe::is_owner_scoped_range`
    /// 拦下过的正是这两发（其一 2026-08-07 真漏进过工作区），但快删不走 staging、
    /// 没有那道闸门，断言只能留在这里。
    #[test]
    fn the_owner_range_is_ref0_scoped_and_never_open_ended() {
        for ref0 in ["24381", "23717", "4294967294"] {
            let sql = owner_range_of(ref0);
            let (beg, end) = sql
                .strip_prefix("pe_owner:[")
                .and_then(|rest| rest.strip_suffix(']'))
                .and_then(|rest| rest.split_once("]..=["))
                .unwrap_or_else(|| panic!("闭区间的两端都必须写出来: {sql}"));
            let (low, low_slot) = beg.split_once(", ").expect("下界是 [owner, 槽位]");
            let (high, high_slot) = end.split_once(", ").expect("上界是 [owner, 槽位]");
            assert_eq!(low, format!("pe:{ref0}_0"), "{sql}");
            assert_eq!(high, format!("pe:{ref0}_{RANGE_END}"), "{sql}");
            assert_eq!(
                low.rsplit_once('_').map(|(prefix, _)| prefix),
                high.rsplit_once('_').map(|(prefix, _)| prefix),
                "两端必须同一个 Ref0，否则圈到相邻库头上: {sql}"
            );
            assert_eq!((low_slot, high_slot), ("NONE", ".."), "{sql}");
        }
    }

    /// 后置条件必须连 owner 边一起数。只数 PE 的话，删边语句圈错区间不会有任何
    /// 东西喊——清库照样报成功，残留边留给下一次重建去撞。
    #[test]
    fn the_postcondition_counts_owner_edges_in_every_ref0_range() {
        let ref0s = ["23717".to_string(), "31909".to_string()];
        let sql = render_verify_query(7333, &ref0s);
        let statements = sql.lines().collect::<Vec<_>>();
        assert_eq!(statements.len(), 1 + ref0s.len(), "{sql}");
        assert!(
            statements[0].contains("FROM pe WHERE dbnum = 7333"),
            "{sql}"
        );
        for (offset, ref0) in ref0s.iter().enumerate() {
            assert_eq!(
                statements[offset + 1],
                format!(
                    "SELECT count() AS count FROM {} GROUP ALL;",
                    owner_range_of(ref0)
                ),
                "语句顺序必须与 ref0s 一致，调用方按下标取残留数: {sql}"
            );
        }
    }

    /// 回退重建变体（ADR-021）：数据阶段与快删完全同源，元数据阶段的差别是
    /// 承诺——水位行清值不删行（登记身份保留）、统计同批清空、spatial epoch
    /// 同批递增，且水位处置是最后一句（清库的提交点：半途失败时水位未动，
    /// 下一轮仍判回退、幂等重放）。
    #[test]
    fn the_reinit_wipe_keeps_the_identity_row_and_bumps_the_epoch() {
        let (_, drop_ranges, _) = render_delete_phases(
            7997,
            &["24381".into()],
            &["EQUI".into()],
            WatermarkDisposal::DropRow,
        );
        let (_, ranges, metadata) = render_delete_phases(
            7997,
            &["24381".into()],
            &["EQUI".into()],
            WatermarkDisposal::ResetForReinit,
        );
        assert_eq!(
            ranges, drop_ranges,
            "数据阶段必须与快删同源，不许自己长一套"
        );
        assert!(
            metadata
                .iter()
                .all(|sql| !sql.contains("DELETE dbnum_watermark")),
            "登记身份必须原地留下: {metadata:?}"
        );
        assert!(
            metadata
                .iter()
                .any(|sql| sql.contains("DELETE dbnum_info_table WHERE dbnum = 7997")),
            "统计行必须同批清空，否则启动播种会把旧水位灌回来: {metadata:?}"
        );
        assert!(
            metadata
                .iter()
                .any(|sql| sql.contains("spatial_epoch:current")),
            "清库必须留下库侧空间痕迹（epoch bump）: {metadata:?}"
        );
        assert_eq!(
            metadata.last().unwrap(),
            "UPDATE dbnum_watermark:7997 SET applied_sesno = 0, sesno = 0, \
             applied_sesno_time = NONE, confirmed_empty_baseline_sesno = NONE;",
            "水位清值必须是元数据阶段的最后一句（清库的提交点）"
        );
    }

    #[test]
    fn metadata_is_one_explicit_transaction_with_the_watermark_last() {
        let (_, _, metadata) = render_delete_phases(
            7997,
            &["24381".into()],
            &["EQUI".into()],
            WatermarkDisposal::ResetForReinit,
        );
        let sql = render_transactional_phase(&metadata).expect("metadata transaction");
        assert_eq!(sql.matches("BEGIN TRANSACTION;").count(), 1, "{sql}");
        assert_eq!(sql.matches("COMMIT TRANSACTION;").count(), 1, "{sql}");
        assert!(sql.starts_with("BEGIN TRANSACTION;\n"), "{sql}");
        assert!(sql.ends_with("\nCOMMIT TRANSACTION;"), "{sql}");
        let watermark = sql
            .rfind("UPDATE dbnum_watermark:7997")
            .expect("watermark update");
        let commit = sql.rfind("COMMIT TRANSACTION;").expect("commit");
        assert!(
            watermark < commit,
            "水位更新必须是事务内最后一条元数据语句: {sql}"
        );
        assert_eq!(
            sql[watermark..commit].matches(';').count(),
            1,
            "水位之后不得再有元数据语句: {sql}"
        );
    }

    /// 待确认-8：`room_panel_relate` 必须与 `room_relate` 一起纳入 Ref0 区间清库。
    ///
    /// 两者是同源姐妹边（同一面板重算入口先清后写、都是 Ref0 前缀 record id）。
    /// 只删 `room_relate` 而漏删 `room_panel_relate`，回退整库重建后会残留指向已删
    /// 房间/面板的悬空边（房间重算只对当前存在的实体先清后写，够不到孤儿）。
    /// 从 `RANGE_TABLES` 移除 `room_panel_relate` 即让本测试变红。
    #[test]
    fn the_wipe_deletes_room_panel_relate_alongside_room_relate() {
        for disposal in [
            WatermarkDisposal::DropRow,
            WatermarkDisposal::ResetForReinit,
        ] {
            let (_, ranges, _) =
                render_delete_phases(7997, &["24381".into()], &["PANE".into()], disposal);
            assert!(
                ranges.contains(&"DELETE room_relate:24381_0..24381_9999999999;".into()),
                "room_relate 应在 Ref0 区间清库集内: {ranges:?}"
            );
            assert!(
                ranges.contains(&"DELETE room_panel_relate:24381_0..24381_9999999999;".into()),
                "room_panel_relate 是 room_relate 的同源姐妹边，必须一并纳入 Ref0 \
                 区间清库，否则回退重建后残留孤儿边: {ranges:?}"
            );
        }
    }

    #[test]
    fn rejects_dynamic_table_name_injection() {
        assert!(valid_table_name("STWALL"));
        assert!(valid_table_name("TYPE_2"));
        assert!(!valid_table_name("PANE; DELETE pe"));
        assert!(!valid_table_name("lower"));
    }

    /// PE 已空但旧统计仍在时，必须从 `dbnum_info_table` 的 record id 恢复真实
    /// Ref0，不能把 dbnum 冒充 Ref0，也不能渲染出一组空的 range 删除。
    #[test]
    fn ghost_watermark_recovers_ref0_ranges_from_info_rows() {
        let (pe_rows, ref0s) = collect_ref0s(
            Vec::new(),
            vec![
                InfoRef0Row {
                    ref0: "23717".into(),
                },
                InfoRef0Row {
                    ref0: "31909".into(),
                },
            ],
        )
        .expect("valid Ref0 rows");

        assert_eq!(pe_rows, 0);
        assert_eq!(ref0s, vec!["23717", "31909"]);
        assert!(!ref0s.contains(&"7333".to_string()));
        let (_, ranges, _) =
            render_delete_phases(7333, &ref0s, &[], WatermarkDisposal::ResetForReinit);
        assert!(ranges.contains(&"DELETE pe:23717_0..23717_9999999999;".into()));
        assert!(ranges.contains(&"DELETE pe:31909_0..31909_9999999999;".into()));
    }
}
