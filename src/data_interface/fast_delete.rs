//! Fast, Ref0-range based removal of one DBNUM's persisted data.

use std::time::Instant;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use surrealdb::{Surreal, engine::any::Any};

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
    // 几何行的另外三张表（2026-09-07）：record id 与 `inst_relate` 同为 `<ref0>_<ref1>`
    // （`model_db_adapter::collect_record`：`type::thing('aabb', id)` / `trans` / `inst_info`），
    // 此前不在清单里，清库后留成孤儿——旧发布路每根先删后插把它们顺手盖掉，看不出来。
    // 空库初始化改走 `WriteMode::Initial`（只插不删）之后，「`inst_relate` / `tubi_relate`
    // 为空」这枚证据必须真的意味着没有任何几何行，否则 `INSERT IGNORE` 让旧盒 / 旧变换赢。
    // `aabb:<mesh id>` 共享本地盒是 64 位十六进制、不含下划线，落不进 `<ref0>_0..<ref0>_9999999999`
    // 这个字符串区间（第 len(ref0) 位是 `_`，十六进制字符排在它两侧），照旧只增不删（ADR-060 D2）。
    "inst_info",
    "aabb",
    "trans",
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
/// 边 id 固定是 `[OWNER_PE, 槽位]`（`ele_ops::pe_owner_edge` 与 `cata_closure` 两个写口
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

/// 运维端点 `DELETE /dbnums/{dbnum}/data/above/{watermark}` 的入口（ADR-072 之后**改调
/// 回退核心**）：预览（读）→ [`rollback_to_watermark`]（谓词删）→ 复核（读）。
///
/// 三段职责分明：预览与复核是端点自己的诊断读（`SELECT`，白名单外但不在回退渲染里，
/// 源码钉只钉 [`render_rollback`]）；真正动手的那一段与崩溃恢复走**同一份**渲染，
/// 端点从此不会与启动回退长出两套语义。
///
/// 行为随之从「只削数据面 + `applied_sesno`」扩成整库回退到 S（ADR-072 D3 不分面）：
/// 模型行、凭证、房间边与三枚水位一起退。这正是端点该有的语义——留着模型面不动，
/// 削完的库在下一次一致性判决里就是「数据退了、模型没退」的幽灵形态。
pub async fn prune_above_watermark(
    dbnum: u32,
    target_watermark: i32,
) -> anyhow::Result<PruneAboveWatermarkResult> {
    let started = Instant::now();
    let preview = preview_prune_above_watermark(dbnum, target_watermark).await?;
    rollback_to_watermark(
        dbnum,
        target_watermark,
        crate::data_interface::write_state::WriteIntent::ApplyWindow,
    )
    .await?;

    // A zero-row cleanup already proved the PE postcondition. Avoid a second
    // full PE scan in that common diagnostic case.
    let remaining_rows_above_watermark = if preview.pe_rows == 0 {
        0
    } else {
        preview_prune_above_watermark(dbnum, target_watermark)
            .await?
            .pe_rows
    };
    if remaining_rows_above_watermark != 0 {
        bail!(
            "dbnum {dbnum} prune incomplete: {remaining_rows_above_watermark} PE rows remain above watermark {target_watermark}"
        );
    }
    Ok(PruneAboveWatermarkResult {
        deleted_pe_rows: preview.pe_rows,
        rebuilt_info_rows: 0,
        remaining_rows_above_watermark,
        preview,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// 一次「回退到水位」的回执（ADR-072 D2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RollbackReport {
    pub dbnum: u32,
    /// 退回到的 S。
    pub target_watermark: i32,
    pub intent: &'static str,
    /// 走了整库 wipe（`initialize` / `reinitialize`，S 恒为 0）。
    pub wiped: bool,
    /// 谓词删除的语句数与它们合成的事务数（`wiped` 时为 0）。
    pub statements: usize,
    pub transactions: usize,
    /// 历史库里没有 `dbnum` / `sesno` 两列的旧 noun / `ATT_UDA` 行谓词删够不着（计划 §9-6）。
    /// 点名而不是装作删干净了：那种库只能靠整库 wipe 回退。
    pub legacy_rows_note: Option<String>,
    pub elapsed_ms: u64,
}

/// 回退的两段渲染：谓词删除 + 控制面收口。
///
/// 分两段是因为**收口那一段是回退的提交点**：三水位退回 S、spatial epoch 递增、标记行删除
/// 必须同生同死，且排在全部删除之后。删除半途失败 → 水位没退、`writing` 还在 → 下一轮
/// 幂等重放（与 `wipe_dbnum_rows` 的「元数据阶段最后」同一条纪律）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RollbackRender {
    pub deletes: Vec<String>,
    pub control: Vec<String>,
}

impl RollbackRender {
    fn statements(&self) -> usize {
        self.deletes.len() + self.control.len()
    }
}

/// 模型面按 `direct_model.sesno` 谓词删的三张表（`gen_root` 用 `source_end_sesno`，另渲染）。
const MODEL_FACE_TABLES: &[&str] = &["inst_relate", "inst_info", "tubi_relate"];

/// §4.3 的 2–6：全部是按 `(dbnum, sesno)` 的谓词 `DELETE`，一句 `SELECT` 都没有（W3）。
///
/// **不许给列套 `<int>`**（`docs/evidence/2026-09-10-rollback-predicate-index-measurement.md` §3.1）：
/// SurrealDB 2.1.4 的优化器见到列上的转型就整条放弃索引，EXPLAIN 直接回
/// `Iterate Table` + `Fallback {reason: "Unsupported value: <int> sesno"}`——每一次回退都变成
/// 全表扫（1112 实测 30 940 行的库上，扫的是 90 617 行的整张 `pe`）。去掉转型是纯收益：
/// 同一条谓词在 1112 上两种写法返回**一样的行数**（S=362 都是 30 939，S=724 都是 1），
/// 而计划变回 `Iterate Index`。老 `prune_above_watermark` 里那个 `<int>` 是防御性的，
/// 两个写口（`SPdmsElement::gen_sur_json` / `ElementRows::minimal_tombstone_row`）都写 JSON 数字。
///
/// **`pe` 行不删**（2026-09-10 改，计划 §4.3-2 原写着要删）。理由是量出来的，不是推的：
///
/// `pe` 是图顶点，删它会把挂在它身上的边一并带走——`tubi_relate` / `ref_rev` 那些是回退要的，
/// 可**「属主 → 本元素」那条 `pe_owner` 成员边也一起没了**，而边 id 是 `pe_owner:[属主, 槽位]`，
/// 槽位号只存在于**属主**的成员表里。属主没进本窗口（纯属性 Modified 的常态）时，S→T 重放
/// 只重建本元素自己名下的槽位，那条入边**补不回来**。一次性 `memory` 实例上的对照实测：
/// 属主 o31 名下 c30（sesno 43，本窗口改过）/ c33（41，没改），跑
/// `DELETE pe WHERE dbnum = 8000 AND sesno > 42` 之后 `pe_owner` 从 `[o31,0]+[o31,1]`
/// 只剩 `[o31,1]`（`docs/evidence/2026-09-10-rollback-predicate-index-measurement.md` §4；
/// fork 行为另见 `test::fork_surreal_compat::dual_pe_rows_replace_in_place_and_keep_their_edges`
/// 末段）。而 R1 验收②要求回退重跑之后 `pe_owner` 与一次成功逐表相等——两者打架。
///
/// 不删也收敛，因为**数据面本来就靠行幂等**（ADR-071 S3）：`pe` 是每个 refno 一行最新态，
/// 重放的 `INSERT … ON DUPLICATE KEY UPDATE`（`element_batch::PE_REPLACE_ASSIGNMENTS`，
/// 列清单与 `gen_sur_json` 一一对应）整行覆盖，先删那一步对 `pe` 从来不是必需的——元素批
/// 自己就是因为这条级联才不删 `pe`（见 `element_batch` 模块头「pe 行为什么不删」）。
/// noun 行与 `ATT_UDA` 行没有边挂着，照删不误：它们是「先删后插」的删那一半。
///
/// **代价说清楚**：`sesno > S` 的 `pe` 行会在回退之后、重跑之前短暂高于水位。同一窗口重放
/// 会把它们逐行覆盖，所以正常路径上是过渡态；只有「重放窗口比崩溃那次更窄」（会话预算截断）
/// 才会留下够不着的行，那一档由下一窗覆盖，或人工走
/// `DELETE /dbnums/{dbnum}/data/above/{watermark}`。文件被换回旧版本那一档不走这条路——
/// 它是 `Reinitialize`，整库 wipe。
pub(crate) fn render_rollback(
    dbnum: u32,
    target_watermark: i32,
    noun_tables: &[String],
) -> RollbackRender {
    let mut deletes = Vec::with_capacity(noun_tables.len() + MODEL_FACE_TABLES.len() + 5);
    for table in noun_tables {
        deletes.push(format!(
            "DELETE {table} WHERE dbnum = {dbnum} AND sesno > {target_watermark};"
        ));
    }
    // `pe` 行不在这张清单里（见本函数文档）。`ATT_UDA` 已经在 `noun_tables` 里——
    // 它同样是全大写表名，`noun_tables_on` 的 `INFO FOR DB` 过滤天然圈到它。
    for table in MODEL_FACE_TABLES {
        deletes.push(format!(
            "DELETE {table} WHERE dbnum = {dbnum} AND direct_model.sesno > {target_watermark};"
        ));
    }
    deletes.push(format!(
        "DELETE gen_root WHERE dbnum = {dbnum} AND source_end_sesno > {target_watermark};"
    ));
    // 房间面整库删（房间本来就是整库重算，边上不带 sesno）。谓词落在 `in` 而不是 `dbnum`：
    // `room_model::render_room_edge_row` / `render_room_panel_relate_write` 写出的边只有
    // `in / out / room_num / …`，行上根本没有 `dbnum` 列；`in` 是本库的面板 / 房间 `pe` 行，
    // 它才带 dbnum。给边加列是 T2/T3 的 `room_*`，本轮不碰。
    deletes.push(format!("DELETE room_relate WHERE in.dbnum = {dbnum};"));
    deletes.push(format!(
        "DELETE room_panel_relate WHERE in.dbnum = {dbnum};"
    ));

    let control = vec![
        // 三枚水位一起退回 S（ADR-072 D3 崩溃不分面）；`state_revision` 前进，读者据它
        // 知道这一格换过代。
        format!(
            "UPDATE dbnum_watermark:{dbnum} SET \
             applied_sesno = math::min([applied_sesno ?: 0, {target_watermark}]), \
             model_sesno = math::min([model_sesno ?: 0, {target_watermark}]), \
             room_sesno = math::min([room_sesno ?: 0, {target_watermark}]), \
             sesno = {target_watermark}, state_revision = (state_revision ?? 0) + 1, \
             updated_at = time::now();"
        ),
        // 回退删掉了 inst_relate / room_relate 行：epoch 不递增的话，重启会按指纹相等
        // 复用一棵还留着被删构件包围盒的树（ADR-010 D4 的幽灵形态）。
        crate::fast_model::aabb_tree::render_spatial_epoch_bump(),
        // 标记行最后删——它是回退的提交点：删除半途失败时 `writing` 还在，下一轮重放。
        crate::data_interface::write_state::render_clear(dbnum),
    ];
    RollbackRender { deletes, control }
}

/// 谓词删除按 [`tx_limits`](crate::data_interface::tx_limits) 合块：一笔事务装满字节 /
/// 写行上限就换一笔。
///
/// 换掉的是 `prune_above_watermark` 原先「逐 pe 键一笔事务」的写法——7997 一窗几千个元素
/// 回退要几千笔（计划 §9-1）。
pub(crate) fn chunk_into_transactions(
    statements: &[String],
    limits: crate::data_interface::tx_limits::DataTxLimits,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut chunk: Vec<&str> = Vec::new();
    let mut bytes = "BEGIN TRANSACTION;\nCOMMIT TRANSACTION;".len();
    for statement in statements {
        let cost = statement.len() + 1;
        let full = !chunk.is_empty()
            && (bytes + cost > limits.max_bytes || chunk.len() as u64 >= limits.max_write_rows);
        if full {
            out.push(wrap(&chunk));
            chunk.clear();
            bytes = "BEGIN TRANSACTION;\nCOMMIT TRANSACTION;".len();
        }
        bytes += cost;
        chunk.push(statement);
    }
    if !chunk.is_empty() {
        out.push(wrap(&chunk));
    }
    return out;

    fn wrap(chunk: &[&str]) -> String {
        format!(
            "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
            chunk.join("\n")
        )
    }
}

/// 库里现存的 noun 表名（`INFO FOR DB` 的表清单里全大写 / 数字 / 下划线的那些）。
///
/// 不走 `SELECT noun FROM pe … GROUP BY noun`（清库那条路的写法）：那是一次整库读，而回退
/// 的白名单里没有它（§4.5）。`INFO FOR DB` 读的是 schema 而不是行，多圈进来的表（`UDA`
/// 字典等）不带 `dbnum` / `sesno` 列，谓词天然落空。
async fn noun_tables_on(db: &Surreal<Any>) -> anyhow::Result<Vec<String>> {
    let mut response = db
        .query("INFO FOR DB;")
        .await
        .context("list tables for rollback")?
        .check()
        .context("list tables for rollback statement")?;
    let info: Option<serde_json::Value> = response.take(0).context("decode INFO FOR DB")?;
    let mut tables: Vec<String> = info
        .as_ref()
        .and_then(|value| value.get("tables"))
        .and_then(|value| value.as_object())
        .map(|map| {
            map.keys()
                .filter(|name| valid_table_name(name))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    tables.sort_unstable();
    tables.dedup();
    Ok(tables)
}

/// 见到 `writing{S,T}` 时把这个库 S 之后的痕迹删掉、三水位退回 S（ADR-072 D2）。
///
/// **它删痕迹让重放补齐，不是还原 S 版本的行**：一个 Modified 元素的 `pe` 行被整行替换成
/// (S, T] 版之后，回退是把它删掉，让 S→T 重放写回来（元素批 `Replace` 档整行覆盖、边
/// `INSERT RELATION IGNORE`）。所以名字叫「回退到水位」，不叫还原。
///
/// 排队那一步（§4.3-8）不在这里：本函数同时是运维端点 `DELETE /dbnums/{dbnum}/data/above/…`
/// 的核心，那条路的契约是「队列保持暂停，让人先看结果」。启动扫描的重排在
/// [`startup_reconcile::rollback_unfinished_writes`](crate::data_interface::startup_reconcile::rollback_unfinished_writes)，
/// 任务开工前那一次由任务自己的 S→T 顶上。
pub async fn rollback_to_watermark(
    dbnum: u32,
    target_watermark: i32,
    intent: crate::data_interface::write_state::WriteIntent,
) -> anyhow::Result<RollbackReport> {
    if dbnum == 0 {
        bail!("dbnum must be greater than zero");
    }
    if target_watermark < 0 {
        bail!("target watermark must be non-negative");
    }
    let started = Instant::now();

    // 初始化 / 重初始化的 S 恒为 0：库里本来就没有「S 版本」可退，整库清空重排（§4.3-1）。
    if intent.wipes_whole_dbnum() {
        let wipe = wipe_dbnum_for_reinit(dbnum).await?;
        crate::data_interface::write_state::clear_marker(dbnum).await?;
        return Ok(RollbackReport {
            dbnum,
            target_watermark: 0,
            intent: intent.as_str(),
            wiped: true,
            statements: 0,
            transactions: 0,
            legacy_rows_note: None,
            elapsed_ms: started.elapsed().as_millis() as u64 + wipe.elapsed_ms,
        });
    }

    let _commit_guard = crate::data_interface::batch_worker::DATA_COMMIT_SERIAL
        .lock()
        .await;
    let _state_guard = crate::data_interface::dbnum_state::DBNUM_STATE_WRITE_GATE
        .write()
        .await;

    let report = rollback_to_watermark_on(&SUL_DB, dbnum, target_watermark, intent).await?;

    // 进程内那一半（§4.3-7）：镜像格作废（下一次读回填）、空间树置脏，启动判据落到指针重建。
    crate::data_interface::dbnum_state::DbnumState::invalidate_mirrored(dbnum);
    crate::data_interface::dbnum_consistency::invalidate_census();
    crate::fast_model::aabb_tree::mark_aabb_tree_dirty();

    Ok(RollbackReport {
        elapsed_ms: started.elapsed().as_millis() as u64,
        ..report
    })
}

/// [`rollback_to_watermark`] 的持久层那一半：渲染 → 合块 → 执行。
///
/// 单拎出来是为了让 `mem://` 崩溃点重放对拍能在自己的实例上跑同一份渲染（对拍的是生产
/// 写法，不是测试自己拼的 SQL）。锁、整库 wipe 那一档与进程内作废都在公开入口那边。
pub(crate) async fn rollback_to_watermark_on(
    db: &Surreal<Any>,
    dbnum: u32,
    target_watermark: i32,
    intent: crate::data_interface::write_state::WriteIntent,
) -> anyhow::Result<RollbackReport> {
    let started = Instant::now();
    let noun_tables = noun_tables_on(db).await?;
    let render = render_rollback(dbnum, target_watermark, &noun_tables);
    let batches = chunk_into_transactions(
        &render.deletes,
        crate::data_interface::tx_limits::data_tx_limits(),
    );
    for (index, batch) in batches.iter().enumerate() {
        db.query(batch)
            .await
            .with_context(|| format!("rollback dbnum {dbnum} delete batch {index}"))?
            .check()
            .with_context(|| format!("rollback dbnum {dbnum} delete batch {index} statement"))?;
    }
    let control = render_transactional_phase(&render.control).expect("control plane is never empty");
    db.query(control)
        .await
        .with_context(|| format!("rollback dbnum {dbnum} watermark and marker"))?
        .check()
        .with_context(|| format!("rollback dbnum {dbnum} watermark and marker statement"))?;

    Ok(RollbackReport {
        dbnum,
        target_watermark,
        intent: intent.as_str(),
        wiped: false,
        statements: render.statements(),
        transactions: batches.len() + 1,
        legacy_rows_note: Some(format!(
            "noun 行与 ATT_UDA 行按 (dbnum, sesno) 谓词删（本库 schema 里 {} 张候选表）：\
             本次改造之前写下的旧行没有这两列，谓词够不着，那种库只能靠整库重建回退",
            noun_tables.len()
        )),
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
    RestartInitializationData,
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
            if watermark == WatermarkDisposal::RestartInitializationData && *table != "ref_rev" {
                continue;
            }
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
        format!("DELETE dbnum_stats_state:{dbnum};"),
    ];
    if watermark == WatermarkDisposal::RestartInitializationData {
        metadata.retain(|sql| {
            !sql.starts_with("DELETE model_update_pending")
                && !sql.starts_with("DELETE incr_side_effect_pending")
        });
    }
    match watermark {
        WatermarkDisposal::DropRow => {
            metadata.push(format!("DELETE dbnum_watermark:{dbnum};"));
        }
        WatermarkDisposal::ResetForReinit | WatermarkDisposal::RestartInitializationData => {
            // 清库删掉了 room_relate / inst_relate 行：epoch 不递增的话，崩溃
            // 重启会按指纹相等复用一棵还留着被删构件包围盒的树（ADR-010 D4 的
            // 幽灵形态借崩溃复活）。放在水位处置之前、同一元数据阶段提交。
            metadata.push(crate::fast_model::aabb_tree::render_spatial_epoch_bump());
            // 同时抹掉指向旧历史的 applied_sesno_time（那条保存在当前文件里已
            // 不存在），基线收口会写上新的；legacy `sesno` 字段与 applied 同步
            // 归零，读侧迁移才不会把旧值当水位捞回来。
            //
            // 模型水位那一组同一句归零（ADR-060 D8）：清库把这个库的模型产物一并
            // 删了，水位留着就是「水位说模型追平过、模型面一行都没有」的幽灵形态。
            // 四格计数回到 NONE 而不是 0——清完还没观察过，「未观察」不是「观察到
            // 零个」（M5）。
            metadata.push(format!(
                "UPDATE dbnum_watermark:{dbnum} SET applied_sesno = 0, sesno = 0, \
                 applied_sesno_time = NONE, confirmed_empty_baseline_sesno = NONE, \
                 model_sesno = 0, model_sesno_time = NONE, model_settled_at = NONE, \
                 model_lazy_roots = NONE, model_dead_roots = NONE, \
                 model_roots_observed_at = NONE;"
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
    let result = wipe_dbnum_rows(dbnum, WatermarkDisposal::ResetForReinit).await?;
    // 反向翻面（plan 2026-09-06 §8 R5）：水位清零后这个库在 rocksdb 里又「没准备好」了，
    // 按需请求改由进程内投影供数，直到重建的初始化任务跑完。
    crate::data_interface::model_read_route::announce_source_changed(
        dbnum,
        crate::data_interface::model_read_route::ModelReadSource::Memory,
    );
    Ok(result)
}

async fn wipe_dbnum_rows(
    dbnum: u32,
    watermark: WatermarkDisposal,
) -> anyhow::Result<FastDeleteDbnumResult> {
    if dbnum == 0 {
        bail!("dbnum must be greater than zero");
    }
    super::pe_batch::require_no_event_on(&SUL_DB).await?;
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

    crate::data_interface::dbnum_state::DbnumState::invalidate_mirrored(dbnum);

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
        // 几何行的三张伴生表同样按 Ref0 区间清（空库初始化 `Initial` 只插不删的前提）。
        for table in ["inst_info", "aabb", "trans"] {
            assert!(
                ranges.contains(&format!("DELETE {table}:24381_0..24381_9999999999;")),
                "{table} 必须随清库一起走: {ranges:?}"
            );
        }
        assert_eq!(
            ranges.last().unwrap(),
            "DELETE pe:24381_0..24381_9999999999;"
        );
        assert_eq!(metadata.last().unwrap(), "DELETE dbnum_watermark:7997;");
    }

    /// 重启初始化数据（`RestartInitializationData`）只清数据面与 `ref_rev`，模型面的六张表
    /// （含新加的 aabb / trans / inst_info）一张都不碰——那一档下初始化发布 run 还活着、几何行归它管。
    #[test]
    fn restarting_initialization_data_leaves_every_model_table_alone() {
        let (_, ranges, _) = render_delete_phases(
            7997,
            &["24381".into()],
            &["EQUI".into()],
            WatermarkDisposal::RestartInitializationData,
        );
        for table in [
            "inst_relate",
            "tubi_relate",
            "geo_relate",
            "inst_info",
            "aabb",
            "trans",
        ] {
            assert!(
                !ranges
                    .iter()
                    .any(|sql| sql.starts_with(&format!("DELETE {table}:"))),
                "{table} 不该在 RestartInitializationData 里被清: {ranges:?}"
            );
        }
        assert!(ranges.contains(&"DELETE ref_rev:24381_0..24381_9999999999;".into()));
        assert!(ranges.contains(&"DELETE EQUI:24381_0..24381_9999999999;".into()));
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
             applied_sesno_time = NONE, confirmed_empty_baseline_sesno = NONE, \
             model_sesno = 0, model_sesno_time = NONE, model_settled_at = NONE, \
             model_lazy_roots = NONE, model_dead_roots = NONE, \
             model_roots_observed_at = NONE;",
            "水位清值必须是元数据阶段的最后一句（清库的提交点）"
        );
        // ADR-060 D8：模型水位与数据水位同一句、同一事务归零。分开写就有半清状态，
        // 而两枚水位「相等」正是一致性判据本身——一枚归零一枚没归，判出来的一致是假的。
        assert!(
            metadata.last().unwrap().contains("model_sesno = 0"),
            "模型水位必须与数据水位同一句归零: {metadata:?}"
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

    fn rollback_sql(dbnum: u32, target: i32, nouns: &[&str]) -> String {
        let nouns = nouns
            .iter()
            .map(|n| (*n).to_string())
            .chain(std::iter::once("ATT_UDA".to_string()))
            .collect::<Vec<_>>();
        let render = render_rollback(dbnum, target, &nouns);
        [render.deletes, render.control].concat().join("\n")
    }

    /// ADR-072 W3 的源码钉：回退全部是按 `(dbnum, sesno)` 的谓词 `DELETE`，一句 `SELECT`
    /// 都没有——老 `prune_above_watermark` 那套「SELECT id, noun FROM pe … → 逐键删」是
    /// 这条纪律要换掉的东西。
    #[test]
    fn the_rollback_never_selects_before_it_deletes() {
        let sql = rollback_sql(7997, 41, &["EQUI", "PANE"]);
        assert!(!sql.contains("SELECT"), "回退渲染不得含 SELECT:\n{sql}");
        assert!(
            !sql.contains("->pe_owner") && !sql.contains("<-pe_owner"),
            "也不得走图遍历逐键删:\n{sql}"
        );
    }

    /// §4.3 的 2–6 逐条：数据面 / 模型面 / 房间面 / 三水位 / 标记行，且顺序固定
    /// ——`pe` 顶点排在 noun 行之后（删顶点级联带走挂在它身上的边），标记行最后删
    /// （它是回退的提交点）。
    #[test]
    fn the_rollback_covers_every_face_and_ends_at_the_marker() {
        let render = render_rollback(
            7997,
            41,
            &["ATT_UDA".into(), "EQUI".into(), "PANE".into()],
        );
        let sql = [render.deletes.clone(), render.control.clone()]
            .concat()
            .join("\n");
        let at = |needle: &str| {
            sql.find(needle)
                .unwrap_or_else(|| panic!("缺 {needle}\n{sql}"))
        };
        at("DELETE EQUI WHERE dbnum = 7997 AND sesno > 41;");
        at("DELETE PANE WHERE dbnum = 7997 AND sesno > 41;");
        at("DELETE ATT_UDA WHERE dbnum = 7997 AND sesno > 41;");
        for table in MODEL_FACE_TABLES {
            at(&format!(
                "DELETE {table} WHERE dbnum = 7997 AND direct_model.sesno > 41;"
            ));
        }
        at("DELETE gen_root WHERE dbnum = 7997 AND source_end_sesno > 41;");
        // 列上套 `<int>` 会让优化器整条放弃索引（证据页 §3.1），一处都不许有。
        assert!(
            !sql.contains("<int>"),
            "回退谓词不得给列套转型，否则每次回退都全表扫:\n{sql}"
        );
        // 房间边行上没有 dbnum 列，谓词落在 `in`（面板 / 房间那一端的 pe 行）。
        at("DELETE room_relate WHERE in.dbnum = 7997;");
        at("DELETE room_panel_relate WHERE in.dbnum = 7997;");
        assert!(
            !sql.contains("DELETE room_relate WHERE dbnum ="),
            "边行没有 dbnum 列，那个谓词会静默删不掉任何东西:\n{sql}"
        );

        // 三水位一起退（D3 崩溃不分面），且是 min 不是赋值——已经更低的水位不许被抬上来。
        let watermark = &render.control[0];
        for field in ["applied_sesno", "model_sesno", "room_sesno"] {
            assert!(
                watermark.contains(&format!("{field} = math::min([{field} ?: 0, 41])")),
                "{field} 必须 min 到 41: {watermark}"
            );
        }
        assert!(
            watermark.contains("state_revision = (state_revision ?? 0) + 1"),
            "{watermark}"
        );
        assert!(
            render.control[1].contains("spatial_epoch:current"),
            "回退删掉了 inst_relate / room_relate 行，epoch 必须递增: {:?}",
            render.control
        );
        assert_eq!(
            render.control.last().unwrap(),
            "DELETE dbnum_write_state:7997;",
            "标记行是回退的提交点，必须最后删: {:?}",
            render.control
        );
    }

    /// 计划 §9-1：回退按 `tx_limits` 合块，不再逐键一笔事务。
    #[test]
    fn the_rollback_batches_by_the_transaction_limits_instead_of_one_per_key() {
        let nouns = (0..300)
            .map(|index| format!("N{index}"))
            .collect::<Vec<_>>();
        let render = render_rollback(7997, 41, &nouns);
        let roomy = chunk_into_transactions(
            &render.deletes,
            crate::data_interface::tx_limits::DataTxLimits::default(),
        );
        assert_eq!(roomy.len(), 1, "生效上限下 306 条谓词删只要一笔事务");
        assert!(roomy[0].starts_with("BEGIN TRANSACTION;\n"));
        assert!(roomy[0].ends_with("\nCOMMIT TRANSACTION;"));

        let tight = chunk_into_transactions(
            &render.deletes,
            crate::data_interface::tx_limits::DataTxLimits {
                max_write_rows: 50,
                ..Default::default()
            },
        );
        assert_eq!(tight.len(), render.deletes.len().div_ceil(50));
        for batch in &tight {
            assert_eq!(batch.matches("BEGIN TRANSACTION;").count(), 1, "{batch}");
            assert!(batch.matches("DELETE ").count() <= 50, "{batch}");
        }
        // 一条都不许漏：合块只是换包装。
        let packed = tight.join("\n");
        for statement in &render.deletes {
            assert!(packed.contains(statement.as_str()), "漏了 {statement}");
        }

        // 字节上限也管用：单条就撑爆时每条自成一笔，而不是拼成一个超限事务。
        let byte_capped = chunk_into_transactions(
            &render.deletes,
            crate::data_interface::tx_limits::DataTxLimits {
                max_bytes: 1,
                ..Default::default()
            },
        );
        assert_eq!(byte_capped.len(), render.deletes.len());
    }

    /// 意图决定走哪条清理路（§4.3-1）：初始化 / 重初始化没有「S 版本」可退，整库 wipe。
    #[test]
    fn initialize_and_reinitialize_take_the_wipe_path() {
        use crate::data_interface::write_state::WriteIntent;
        let source = include_str!("fast_delete.rs");
        let body = source
            .split_once("pub async fn rollback_to_watermark(")
            .expect("rollback entry")
            .1
            .split_once("\n}\n")
            .expect("body")
            .0;
        assert!(
            body.contains("if intent.wipes_whole_dbnum()") && body.contains("wipe_dbnum_for_reinit"),
            "初始化那一档必须走整库 wipe: {body}"
        );
        assert!(WriteIntent::Initialize.wipes_whole_dbnum());
        assert!(!WriteIntent::ApplyWindow.wipes_whole_dbnum());
    }

    /// 回退**不删 `pe` 顶点**（2026-09-10 翻转，见 `render_rollback` 文档）。
    ///
    /// 删顶点会级联带走「属主 → 本元素」那条 `pe_owner` 成员边，而边 id 里的槽位号只存在于
    /// 属主的成员表里，属主没进本窗口时重放补不回来（实测见证据页 §4）。数据面靠行幂等收敛，
    /// `pe` 不需要先删——这一条与 `element_batch` 模块头「pe 行为什么不删」是同一条纪律。
    ///
    /// 本测试同时钉住「没有任何一句重建入边」：真去删顶点了，就得先把这一半补上。
    #[test]
    fn the_rollback_never_deletes_the_pe_vertex_so_the_membership_edge_survives() {
        let sql = rollback_sql(7997, 41, &["EQUI"]);
        assert!(
            !sql.contains("DELETE pe WHERE"),
            "回退不得删 pe 顶点——级联会带走属主名下那条成员边:\n{sql}"
        );
        assert!(
            !sql.contains("DELETE pe:") && !sql.contains("DELETE pe;"),
            "任何形态的 pe 行删除都不许有:\n{sql}"
        );
        // 卫星行照删：它们没有边挂着，是「先删后插」的删那一半。
        assert!(sql.contains("DELETE EQUI WHERE dbnum = 7997 AND sesno > 41;"), "{sql}");
        assert!(sql.contains("DELETE ATT_UDA WHERE dbnum = 7997 AND sesno > 41;"), "{sql}");
        assert!(
            !sql.contains("INSERT RELATION"),
            "既然不删顶点就不必重建入边；哪天真要删顶点了，这两条断言得一起改:\n{sql}"
        );
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
