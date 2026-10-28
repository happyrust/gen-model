//! Fast, Ref0-range based removal of one DBNUM's persisted data.

use std::time::Instant;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use surrealdb::sql::Thing;
use surrealdb::{Surreal, engine::any::Any};

use aios_core::SUL_DB;

use crate::data_interface::mesh_purge::{self, ClearGeometry, MeshPurgeReport};

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
    // **不在**这份清单里的两张交付台账（plan 2026-09-22 D7）：`datacenter_version`（rs-server 的
    // 提资状态账）与 `datacenter_delete`（待交中台删的老位号）。它们记的是「中台那边有什么 /
    // 该删什么」，与本库重不重建无关——回退 / 重初始化 / 删库都不清，源码钉
    // `the_wipe_never_touches_the_delivery_ledgers`。
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

/// `GET /api/v1/dbnums/{dbnum}/data/above/{watermark}` 的回执（ADR-073 D5 保留的只读半边）：
/// 水位之上还有多少 `pe` 行、抽样几个 refno。它是清库之后自证「真的干净了」的唯一现成只读工具。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PruneAboveWatermarkPreview {
    pub dbnum: u32,
    pub target_watermark: i32,
    pub applied_sesno: i32,
    pub pe_rows: usize,
    pub sample_refnos: Vec<String>,
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
/// 整库清理。而 ADR-073 D3 / D5 之后库上也只剩整库清理这一种删法：按水位的部分裁剪
/// （`sesno > S` 谓词删 / 逐元素双向删）整段退役，`/data/above/{watermark}` 只剩 GET 预览的
/// `SELECT`（[`preview_prune_above_watermark`]）。
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

/// 模型面按 `model_sesno` 谓词删的三张表。逐根凭证行不在其中（`gen_root` 退役，ADR-079）：
/// 回退 / 清库时进程内账本那一页由调用方 `model_root_ledger::forget_dbnum` 作废。
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
///
/// **意图决定退哪几面**（t-664 ⑥，ADR-072 D3 的适用范围）：`ApplyWindow` 写过数据面 + 模型面
/// （房间边随模型块落，ADR-073 D1），两面都退、两水位都 min 到 S；`ModelOnly` 的 S 是模型水位，
/// 这类任务**没写过数据面**——退数据面等于把上一窗已经收口的 (S, applied] 数据行删掉、而重排的
/// ModelOnly 又不补数据，库就此缺一段
/// （`direct_window_replay_parity::a_model_only_task_killed_in_a_model_block_…` 钉的正是这一处）
/// ——所以只退模型面 + 房间边、`model_sesno` 退到 S，`applied_sesno` / `sesno` 不动；
/// `RoomOnly` 只写过房间边，只退房间边——库上没有房间水位可退（ADR-073 D2）。
/// `Initialize` / `Reinitialize` 不走这里（整库 wipe）。
pub(crate) fn render_rollback(
    dbnum: u32,
    target_watermark: i32,
    noun_tables: &[String],
    intent: crate::data_interface::write_state::WriteIntent,
) -> RollbackRender {
    use crate::data_interface::write_state::WriteIntent;
    let (data_face, model_face) = match intent {
        WriteIntent::ModelOnly => (false, true),
        WriteIntent::RoomOnly => (false, false),
        WriteIntent::ApplyWindow | WriteIntent::Initialize | WriteIntent::Reinitialize => {
            (true, true)
        }
    };

    let mut deletes = Vec::with_capacity(noun_tables.len() + MODEL_FACE_TABLES.len() + 5);
    if data_face {
        for table in noun_tables {
            deletes.push(format!(
                "DELETE {table} WHERE dbnum = {dbnum} AND sesno > {target_watermark};"
            ));
        }
    }
    // `pe` 行不在这张清单里（见本函数文档）。`ATT_UDA` 已经在 `noun_tables` 里——
    // 它同样是全大写表名，`noun_tables_on` 的 `INFO FOR DB` 过滤天然圈到它。
    if model_face {
        for table in MODEL_FACE_TABLES {
            deletes.push(format!(
                "DELETE {table} WHERE dbnum = {dbnum} AND model_sesno > {target_watermark};"
            ));
        }
    }
    // 房间面整库删（房间本来就是整库重算，边上不带 sesno）。谓词落在 `in` 而不是 `dbnum`：
    // `room_model::render_room_edge_row` / `render_room_panel_relate_write` 写出的边只有
    // `in / out / room_num / …`，行上根本没有 `dbnum` 列；`in` 是本库的面板 / 房间 `pe` 行，
    // 它才带 dbnum。给边加列是 T2/T3 的 `room_*`，本轮不碰。
    deletes.push(format!("DELETE room_relate WHERE in.dbnum = {dbnum};"));
    deletes.push(format!(
        "DELETE room_panel_relate WHERE in.dbnum = {dbnum};"
    ));

    // 水位那一句：退哪几面就 min 哪几枚，都是 min 不是赋值——已经更低的水位不许被抬上来；
    // `state_revision` 前进，读者据它知道这一格换过代。
    let mut sets = Vec::with_capacity(5);
    if data_face {
        sets.push(format!(
            "applied_sesno = math::min([applied_sesno ?: 0, {target_watermark}])"
        ));
    }
    if model_face {
        sets.push(format!(
            "model_sesno = math::min([model_sesno ?: 0, {target_watermark}])"
        ));
    }
    if data_face {
        sets.push(format!("sesno = {target_watermark}"));
    }
    sets.push("state_revision = (state_revision ?? 0) + 1".to_owned());
    sets.push("updated_at = time::now()".to_owned());
    let control = vec![
        format!("UPDATE dbnum_watermark:{dbnum} SET {};", sets.join(", ")),
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

    // 进程内那一半（§4.3-7）：镜像格作废（下一次读回填）、空间树置脏，启动判据落到指针重建；
    // 生成根账本这一页整页作废（ADR-079）——S 之后生成的根的「成了 / 失败了」都随痕迹一起退，
    // 由 S→T 重放重记。
    crate::data_interface::dbnum_state::DbnumState::invalidate_mirrored(dbnum);
    crate::data_interface::model_root_ledger::forget_dbnum(dbnum);
    crate::fast_model::aabb_tree::mark_aabb_tree_dirty();

    Ok(RollbackReport {
        elapsed_ms: started.elapsed().as_millis() as u64,
        ..report
    })
}

/// [`rollback_to_watermark`] 的持久层那一半：渲染 → 合块 → 执行。
///
/// 单拎出来是为了让 `mem://` 崩溃点重放对拍能在自己的实例上跑同一份渲染（对拍的是生产
/// 写法，不是测试自己拼的 SQL）。锁与进程内作废都在公开入口那边；整库 wipe 那一档
/// （`Initialize` / `Reinitialize`，§4.3-1）这里也走得通——同一份 [`wipe_dbnum_rows_on`]
/// 渲染，只是不拿锁、不翻按需读路。
pub(crate) async fn rollback_to_watermark_on(
    db: &Surreal<Any>,
    dbnum: u32,
    target_watermark: i32,
    intent: crate::data_interface::write_state::WriteIntent,
) -> anyhow::Result<RollbackReport> {
    let started = Instant::now();
    if intent.wipes_whole_dbnum() {
        // 初始化的 S 恒为 0：库里没有「S 版本」可退，整库清空（水位行清值不删行）；
        // 标记行最后删——它是回退的提交点（§4.3-6）。
        let wipe = wipe_dbnum_rows_on(db, dbnum, WatermarkDisposal::ResetForReinit).await?;
        db.query(crate::data_interface::write_state::render_clear(dbnum))
            .await
            .with_context(|| format!("rollback dbnum {dbnum} clear write marker after wipe"))?
            .check()
            .with_context(|| {
                format!("rollback dbnum {dbnum} clear write marker after wipe statement")
            })?;
        return Ok(RollbackReport {
            dbnum,
            target_watermark: 0,
            intent: intent.as_str(),
            wiped: true,
            statements: 0,
            transactions: 0,
            legacy_rows_note: None,
            elapsed_ms: started.elapsed().as_millis().max(wipe.elapsed_ms as u128) as u64,
        });
    }
    let noun_tables = noun_tables_on(db).await?;
    let render = render_rollback(dbnum, target_watermark, &noun_tables, intent);
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
    let control =
        render_transactional_phase(&render.control).expect("control plane is never empty");
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

/// 模型水位那半句（ADR-060 D8）。整库清库的 `ResetForReinit` 把它接在数据水位后面、
/// 同一句 `UPDATE` 里；整库模型清理单独成句，字面量必须同一份。
const MODEL_WATERMARK_RESET: &str = "model_sesno = 0, model_sesno_time = NONE, \
     model_settled_at = NONE, model_lazy_roots = NONE, model_dead_roots = NONE, \
     model_roots_observed_at = NONE, model_storage_version = NONE";

fn is_model_face_range_table(table: &str) -> bool {
    table != "pe" && table != "ref_rev"
}

/// 整库模型面要删 / 要改的语句。区间删的表清单就是 `RANGE_TABLES` 去掉 `pe` / `ref_rev`
/// （不另抄一份）；房间两边再按 `in.dbnum` 删一次——边上没有 `dbnum` 列，Ref0 区间
/// 够不着「id 前缀不在本库 Ref0 集合里」的边。元数据是 spatial epoch 与模型水位半边，
/// 并让 `state_revision` 前进一步。
pub(crate) struct ModelFaceRender {
    pub range_deletes: Vec<String>,
    pub room_predicates: Vec<String>,
    pub metadata: Vec<String>,
}

pub(crate) fn render_model_face_wipe(dbnum: u32, ref0s: &[String]) -> ModelFaceRender {
    let mut range_deletes = Vec::new();
    for ref0 in ref0s {
        for table in RANGE_TABLES
            .iter()
            .copied()
            .filter(|table| is_model_face_range_table(table))
        {
            range_deletes.push(format!("DELETE {};", range_of(table, ref0)));
        }
    }
    let room_predicates = vec![
        format!("DELETE room_relate WHERE in.dbnum = {dbnum};"),
        format!("DELETE room_panel_relate WHERE in.dbnum = {dbnum};"),
    ];
    let metadata = vec![
        crate::fast_model::aabb_tree::render_spatial_epoch_bump(),
        format!(
            "UPDATE dbnum_watermark:{dbnum} SET {MODEL_WATERMARK_RESET}, \
             state_revision = (state_revision ?? 0) + 1;"
        ),
    ];
    ModelFaceRender {
        range_deletes,
        room_predicates,
        metadata,
    }
}

/// Render separate checked phases. A giant optimistic transaction conflicts
/// with the watcher's periodic observation write on large DBNUMs. Metadata is
/// deliberately last, so a failed data phase never advertises an initialized
/// database as cleanly deleted; within it the watermark disposal is the final
/// statement — it is the wipe's commit point, so a half-done wipe keeps the
/// old watermark and the next verdict retries idempotently.
///
/// 模型面的区间删与 spatial epoch 来自 [`render_model_face_wipe`]，和整库模型清理
/// 是同一份渲染。`ref_rev` / noun / `pe` 仍是数据面，按原顺序插回去，整段语句与
/// 抽出之前逐字相同。
fn render_delete_phases(
    dbnum: u32,
    ref0s: &[String],
    noun_tables: &[String],
    watermark: WatermarkDisposal,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let model = render_model_face_wipe(dbnum, ref0s);
    let mut model_deletes = model.range_deletes.iter();
    let mut relations = Vec::new();
    let mut ranges = Vec::new();
    for ref0 in ref0s {
        relations.push(format!("DELETE {};", owner_range_of(ref0)));
        for table in RANGE_TABLES.iter().copied().filter(|table| *table != "pe") {
            if watermark == WatermarkDisposal::RestartInitializationData && table != "ref_rev" {
                if is_model_face_range_table(table) {
                    let _ = model_deletes.next();
                }
                continue;
            }
            if is_model_face_range_table(table) {
                ranges.push(
                    model_deletes
                        .next()
                        .expect("model-face range delete")
                        .clone(),
                );
            } else {
                ranges.push(format!("DELETE {};", range_of(table, ref0)));
            }
        }
        for table in noun_tables {
            ranges.push(format!("DELETE {};", range_of(table, ref0)));
        }
        ranges.push(format!("DELETE {};", range_of("pe", ref0)));
    }
    debug_assert!(model_deletes.next().is_none());
    // 第二条异步账的三张表（模型意图队列 / 恢复记录 / 副作用队列）不再逐库清：它们整体退役
    // （ADR-072 D5），随 `retire_publish_run_tables` 的 `REMOVE TABLE IF EXISTS` 一次删掉，
    // 清库路上再点名它们只是给退役表续命。
    // 逐根凭证行不再有（`gen_root` 退役，ADR-079）：整库 wipe / 删库之后「这根已经生成到 T」的旧说法
    // 只可能活在进程内账本里，由调用方 `model_root_ledger::forget_dbnum` 一页作废（`wipe_dbnum_for_reinit`
    // 的调用方与删库口都做）。
    let mut metadata = vec![
        format!("DELETE dbnum_info_table WHERE dbnum = {dbnum};"),
        format!("DELETE dbnum_stats_state:{dbnum};"),
    ];
    match watermark {
        WatermarkDisposal::DropRow => {
            metadata.push(format!("DELETE dbnum_watermark:{dbnum};"));
        }
        WatermarkDisposal::ResetForReinit | WatermarkDisposal::RestartInitializationData => {
            // 清库删掉了 room_relate / inst_relate 行：epoch 不递增的话，崩溃
            // 重启会按指纹相等复用一棵还留着被删构件包围盒的树（ADR-010 D4 的
            // 幽灵形态借崩溃复活）。放在水位处置之前、同一元数据阶段提交。
            // 与整库模型清理同一句 epoch。模型水位半边接在数据水位后面，仍是同一句
            // UPDATE（分开写就有半清状态）。四格计数回到 NONE 而不是 0——清完还没
            // 观察过，「未观察」不是「观察到零个」（M5）。
            metadata.push(model.metadata[0].clone());
            metadata.push(format!(
                "UPDATE dbnum_watermark:{dbnum} SET applied_sesno = 0, sesno = 0, \
                 applied_sesno_time = NONE, confirmed_empty_baseline_sesno = NONE, \
                 {MODEL_WATERMARK_RESET};"
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

async fn execute_phase(
    db: &Surreal<Any>,
    label: &str,
    statements: &[String],
) -> anyhow::Result<()> {
    if statements.is_empty() {
        return Ok(());
    }
    db.query(statements.join("\n"))
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

async fn execute_transactional_phase(
    db: &Surreal<Any>,
    label: &str,
    statements: &[String],
) -> anyhow::Result<()> {
    let Some(sql) = render_transactional_phase(statements) else {
        return Ok(());
    };
    db.query(sql)
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
///
/// 库里的行没了，进程里记着「这个库有什么」的两份记忆也得跟着作废——与 [`wipe_dbnum_for_reinit`]
/// 同一条纪律：模型投影 `clear_dbnum`；CATA 已落库 memo（plan 2026-09-15 D6）整份忘掉。删的若是
/// 目录库，它的依赖行正是别的任务会因 memo 跳过不发的那些；删的是 DESI 库时按目录库分不出哪些该留。
/// 不作废的话这个进程剩下的任务都当那些行还在，缺行要等到重启才补得回来。
pub async fn delete_dbnum_fast(dbnum: u32) -> anyhow::Result<FastDeleteDbnumResult> {
    let result = wipe_dbnum_rows(dbnum, WatermarkDisposal::DropRow).await?;
    crate::fast_model::model_memory_store::ModelMemoryStore::global().clear_dbnum(dbnum);
    // 生成根账本这一页随库一起没了（ADR-079）：库里没有逐根的行，进程里也不该再记着它们。
    crate::data_interface::model_root_ledger::forget_dbnum(dbnum);
    crate::data_interface::cata_dependency::forget_persisted_catalogue_rows(
        crate::data_interface::cata_dependency::ForgetScope::Everything,
    );
    Ok(result)
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
    // 进程内模型面与整库模型清理同一串。目录 memo 是数据面，只有整库重建才作废。
    forget_dbnum_model_in_process(dbnum);
    crate::data_interface::cata_dependency::forget_persisted_catalogue_rows(
        crate::data_interface::cata_dependency::ForgetScope::Everything,
    );
    Ok(result)
}

/// 进程内「这个库的模型」作废：投影、生成根账本、空间树，并把读路由翻到内存。
/// 整库重建与整库模型清理共用；**不**动目录 memo（那是数据面）。
fn forget_dbnum_model_in_process(dbnum: u32) {
    crate::fast_model::model_memory_store::ModelMemoryStore::global().clear_dbnum(dbnum);
    crate::data_interface::model_root_ledger::forget_dbnum(dbnum);
    crate::fast_model::aabb_tree::mark_aabb_tree_dirty();
    crate::data_interface::model_read_route::announce_source_changed(
        dbnum,
        crate::data_interface::model_read_route::ModelReadSource::Memory,
    );
}

/// 整库生成锁拿不到：有按需 / 整库生成在飞。一行都不删。
#[derive(Debug)]
pub struct ModelFaceGenerationBusy {
    pub dbnum: u32,
}

impl std::fmt::Display for ModelFaceGenerationBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "dbnum {} 有生成在飞，整库模型清理拒绝、一行不删",
            self.dbnum
        )
    }
}

impl std::error::Error for ModelFaceGenerationBusy {}

/// `DELETE /api/v1/dbnums/{dbnum}/model` 的回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelFaceWipeResult {
    pub dbnum: u32,
    pub ref0s: Vec<String>,
    pub range_statements: usize,
    pub elapsed_ms: u64,
    /// `purge_meshes=true` 那一格的账（`inst_geo` 行 + `.mesh`）；不勾时 `None`。回执里由 handler
    /// 逐格展开，不整件序列化。
    #[serde(skip)]
    pub geometry: Option<MeshPurgeReport>,
}

/// 只清一个库的模型面：模型表区间、房间两边、模型水位归零。不碰 `pe` / noun / 数据水位 /
/// 目录 memo。`Database` 路由才写库；其余只作废进程内投影，读透另把每个根渲成 spool 墓碑。
///
/// `geometry = Purge`（库行「清理模型（整库）」的 `purge_meshes=true`）时连这个库引用过的几何一起清：
/// 键在生成锁之内、删行之前收（`inst_geo` 不按 Ref0 寻址，行一删就再也收不出来；收不动整趟中止、
/// 一行不删），`inst_geo` 行与 `.mesh` 在删行之后清。库是权威面（`Database` 路由）才收、删库上那一份；
/// 其余路由只删投影指向的文件。口径与单位几何的例外见 [`mesh_purge`] 模块文档。
pub async fn wipe_dbnum_model_face(
    dbnum: u32,
    geometry: ClearGeometry,
) -> anyhow::Result<ModelFaceWipeResult> {
    if dbnum == 0 {
        bail!("dbnum must be greater than zero");
    }
    let _generation = crate::fast_model::e3d_model_service::try_lock_db_generation(dbnum)
        .map_err(|_| ModelFaceGenerationBusy { dbnum })?;
    let _commit_guard = crate::data_interface::batch_worker::DATA_COMMIT_SERIAL
        .lock()
        .await;
    let _state_guard = crate::data_interface::dbnum_state::DBNUM_STATE_WRITE_GATE
        .write()
        .await;
    let started = Instant::now();
    let route = crate::data_interface::model_read_route::route_for(dbnum).await?;
    let durable = route.is_database();
    // 几何键在删行之前收（已在生成锁之内，没有生成收尾能插进来换路由）：行一删掉，库与投影
    // 都再也说不出这个库引用过哪些网格。收不动就整趟中止（一行不删）。
    let keys = if geometry.purges() {
        Some(mesh_purge::dbnum_mesh_ids(dbnum, durable).await?)
    } else {
        None
    };
    let mut result = if !durable {
        if crate::options::data_face().is_read_through() {
            spool_dbnum_model_tombstones(dbnum)?;
        }
        forget_dbnum_model_in_process(dbnum);
        ModelFaceWipeResult {
            dbnum,
            ref0s: Vec::new(),
            range_statements: 0,
            elapsed_ms: 0,
            geometry: None,
        }
    } else {
        let result = wipe_dbnum_model_face_on(&SUL_DB, dbnum).await?;
        forget_dbnum_model_in_process(dbnum);
        result
    };
    // 几何排在库行之后：反过来会先把模型还要用的几何删掉、而产物还在。整库的文件可能几万个，
    // 写库串行闸与水位写门到这里已经没有用，先放掉，别让数据任务等一轮删盘；生成锁照持，删文件期间
    // 不许有生成往这个库写新键。
    drop(_state_guard);
    drop(_commit_guard);
    if let Some(keys) = keys {
        result.geometry = Some(mesh_purge::purge_geometry(&SUL_DB, keys, durable).await?);
    }
    result.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(result)
}

/// [`wipe_dbnum_model_face`] 的库那一半，给 `mem://` 行为用例在自己的实例上跑同一份渲染。
pub(crate) async fn wipe_dbnum_model_face_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<ModelFaceWipeResult> {
    if dbnum == 0 {
        bail!("dbnum must be greater than zero");
    }
    let started = Instant::now();
    let (_pe_rows, ref0s, _noun_tables) = collect_dbnum_ref0s_on(db, dbnum).await?;
    let rendered = render_model_face_wipe(dbnum, &ref0s);
    let mut statements = rendered.range_deletes;
    statements.extend(rendered.room_predicates);
    execute_phase(db, "delete model face", &statements).await?;
    execute_transactional_phase(db, "reset model watermark", &rendered.metadata).await?;

    let mut verify = db
        .query(render_model_face_verify(dbnum, &ref0s))
        .await
        .context("verify model face wipe")?
        .check()
        .context("verify model face wipe statement")?;
    let range_tables = RANGE_TABLES
        .iter()
        .filter(|table| is_model_face_range_table(table))
        .count();
    let queries = range_tables * ref0s.len() + 2;
    for index in 0..queries {
        let remaining = verify
            .take::<Vec<CountRow>>(index)
            .with_context(|| format!("decode model face residue {index}"))?
            .first()
            .map(|row| row.count)
            .unwrap_or_default();
        if remaining != 0 {
            bail!(
                "dbnum {dbnum} model face wipe incomplete: query {index} still has {remaining} rows"
            );
        }
    }
    crate::data_interface::dbnum_state::DbnumState::invalidate_mirrored_on(db, dbnum);
    Ok(ModelFaceWipeResult {
        dbnum,
        ref0s,
        range_statements: statements.len(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        geometry: None,
    })
}

fn render_model_face_verify(dbnum: u32, ref0s: &[String]) -> String {
    let mut sql = String::new();
    for table in RANGE_TABLES
        .iter()
        .copied()
        .filter(|table| is_model_face_range_table(table))
    {
        for ref0 in ref0s {
            sql.push_str(&format!(
                "SELECT count() AS count FROM {} GROUP ALL;\n",
                range_of(table, ref0)
            ));
        }
    }
    sql.push_str(&format!(
        "SELECT count() AS count FROM room_relate WHERE in.dbnum = {dbnum} GROUP ALL;\n\
         SELECT count() AS count FROM room_panel_relate WHERE in.dbnum = {dbnum} GROUP ALL;"
    ));
    sql
}

fn projected_roots_for_dbnum(dbnum: u32) -> Vec<e3d_io::refno::RefNo> {
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    let scope = crate::fast_model::model_memory_store::ProjectionScope::Current;
    store
        .roots(scope)
        .into_iter()
        .filter(|root| {
            store.receipt(scope, *root).is_some_and(|receipt| {
                receipt
                    .model_target
                    .as_ref()
                    .is_some_and(|target| target.design.dbnum == dbnum)
            }) || store
                .records_of_root(scope, *root)
                .iter()
                .any(|record| record.dbnum == dbnum)
        })
        .collect()
}

fn spool_dbnum_model_tombstones(dbnum: u32) -> anyhow::Result<()> {
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    let scope = crate::fast_model::model_memory_store::ProjectionScope::Current;
    let roots = projected_roots_for_dbnum(dbnum);
    if roots.is_empty() {
        return Ok(());
    }
    let option = aios_core::get_db_option();
    let spool = crate::data_interface::mirror_spool::MirrorSpool::for_process(
        &option.surreal_ns,
        &option.project_name,
        &option.mdb_name,
    )?;
    for root in roots {
        let records = store
            .records_of_root(scope, root)
            .into_iter()
            .filter(|record| record.dbnum == dbnum)
            .collect::<Vec<_>>();
        let tube_branches = records
            .iter()
            .filter(|record| record.tube.is_some())
            .map(|record| record.source_refno)
            .collect::<Vec<_>>();
        let transaction = crate::fast_model::model_db_adapter::render_spooled_scope_deletion(
            root,
            &[root],
            &tube_branches,
        );
        let sessions = records
            .iter()
            .map(|record| (record.dbnum, record.source_sesno))
            .collect::<Vec<_>>();
        let source_sesno_epoch = sessions
            .iter()
            .map(|(_, sesno)| u64::from(*sesno))
            .max()
            .unwrap_or_default();
        let epoch = crate::data_interface::mdb_memory_store::get(
            &option.project_name,
            &option.mdb_name,
        )
        .map(|snapshot| snapshot.epoch())
        .unwrap_or(source_sesno_epoch);
        spool.persist(
            epoch,
            sessions,
            format!("model-tombstone-dbnum:{dbnum}:{root}"),
            vec![transaction],
        )?;
    }
    Ok(())
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

    let result = wipe_dbnum_rows_on(&SUL_DB, dbnum, watermark).await?;
    Ok(FastDeleteDbnumResult {
        elapsed_ms: started.elapsed().as_millis() as u64,
        ..result
    })
}

/// `wipe_dbnum_rows_on` 开头那三句：PE 前缀、noun、`dbnum_info_table` 的 Ref0，取并集。
/// 整库快删与整库模型清理共用，避免两份圈定。
async fn collect_dbnum_ref0s_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<(usize, Vec<String>, Vec<String>)> {
    let mut response = db
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
    Ok((pe_rows, ref0s, noun_tables))
}

/// [`wipe_dbnum_rows`] 的持久层那一半：Ref0 区间圈定 → 三个删除阶段 → 核对，在给定实例上跑。
///
/// 单拎出来是为了让 `mem://` 崩溃点重放对拍能在自己的实例上跑同一份渲染——初始化在第 k 块
/// kill → 回退 = 整库 wipe → 重排（计划 §4.3-1），对拍的是生产写法，不是测试自己拼的 SQL。
/// 锁、`pe` 事件核对与按需读路翻面都在公开入口那边；镜像作废按实例判（`_on` 纪律）。
pub(crate) async fn wipe_dbnum_rows_on(
    db: &Surreal<Any>,
    dbnum: u32,
    watermark: WatermarkDisposal,
) -> anyhow::Result<FastDeleteDbnumResult> {
    if dbnum == 0 {
        bail!("dbnum must be greater than zero");
    }
    let started = Instant::now();
    let (pe_rows, ref0s, noun_tables) = collect_dbnum_ref0s_on(db, dbnum).await?;

    let (relations, ranges, metadata) =
        render_delete_phases(dbnum, &ref0s, &noun_tables, watermark);
    execute_phase(db, "delete owner relations", &relations).await?;
    execute_phase(db, "delete Ref0 ranges", &ranges).await?;
    execute_transactional_phase(db, "delete dbnum metadata", &metadata).await?;

    let mut verify = db
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

    crate::data_interface::dbnum_state::DbnumState::invalidate_mirrored_on(db, dbnum);

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
    use crate::data_interface::write_state::WriteIntent;

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
             model_roots_observed_at = NONE, model_storage_version = NONE;",
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

    /// 交付台账不随库清（plan 2026-09-22 D7）：`datacenter_version` 是 rs-server 的提资状态账，
    /// `datacenter_delete` 是待交中台删的老位号——待删的老位号仍待删，与库重不重建无关。
    /// 两张表都**不在** `RANGE_TABLES`，三种水位处置渲出的清库语句一条都不点名它们；
    /// 往 `RANGE_TABLES` 加任一张即让本测试变红。
    #[test]
    fn the_wipe_never_touches_the_delivery_ledgers() {
        for ledger in ["datacenter_version", "datacenter_delete"] {
            assert!(
                !RANGE_TABLES.contains(&ledger),
                "{ledger} 是交付台账，不进 Ref0 区间清库集: {RANGE_TABLES:?}"
            );
        }
        for disposal in [
            WatermarkDisposal::DropRow,
            WatermarkDisposal::ResetForReinit,
            WatermarkDisposal::RestartInitializationData,
        ] {
            let (relations, ranges, metadata) =
                render_delete_phases(7997, &["24381".into()], &["BRAN".into()], disposal);
            for statement in relations.iter().chain(&ranges).chain(&metadata) {
                assert!(
                    !statement.contains("datacenter"),
                    "{disposal:?} 的清库语句不得碰交付台账: {statement}"
                );
            }
        }
    }

    fn rollback_sql(dbnum: u32, target: i32, nouns: &[&str]) -> String {
        let nouns = nouns
            .iter()
            .map(|n| (*n).to_string())
            .chain(std::iter::once("ATT_UDA".to_string()))
            .collect::<Vec<_>>();
        let render = render_rollback(dbnum, target, &nouns, WriteIntent::ApplyWindow);
        [render.deletes, render.control].concat().join("\n")
    }

    /// t-664 ⑥：ModelOnly 的 S 是模型水位，这类任务没写过数据面——回退只退模型面 + 房间边，
    /// `applied_sesno` / `sesno` 与 noun 行一个都不动，否则上一窗已收口的 (S, applied] 数据行被删、
    /// 重排的 ModelOnly 又不补数据。RoomOnly 更窄：只退房间边，水位一枚不动（ADR-073 D2）。
    #[test]
    fn a_model_only_or_room_only_rollback_leaves_the_data_face_alone() {
        let nouns: Vec<String> = vec!["ATT_UDA".into(), "EQUI".into()];
        let model_only = render_rollback(7997, 41, &nouns, WriteIntent::ModelOnly);
        let sql = [model_only.deletes.clone(), model_only.control.clone()]
            .concat()
            .join("\n");
        assert!(
            !sql.contains("DELETE EQUI") && !sql.contains("DELETE ATT_UDA"),
            "ModelOnly 不删数据面: {sql}"
        );
        for table in MODEL_FACE_TABLES {
            assert!(
                sql.contains(&format!(
                    "DELETE {table} WHERE dbnum = 7997 AND model_sesno > 41;"
                )),
                "ModelOnly 仍退模型面 {table}: {sql}"
            );
        }
        assert!(
            !sql.contains("gen_root"),
            "逐根凭证行随 gen_root 退役（ADR-079），回退没有它要删: {sql}"
        );
        assert!(sql.contains("DELETE room_relate WHERE in.dbnum = 7997;"));
        let watermark = &model_only.control[0];
        assert!(
            !watermark.contains("applied_sesno") && !watermark.contains(" sesno ="),
            "ModelOnly 不动数据水位: {watermark}"
        );
        assert!(watermark.contains("model_sesno = math::min([model_sesno ?: 0, 41])"));
        assert!(
            !watermark.contains("room_"),
            "库上没有房间水位可退: {watermark}"
        );
        assert!(watermark.contains("state_revision = (state_revision ?? 0) + 1"));
        assert_eq!(
            model_only.control.last().unwrap(),
            "DELETE dbnum_write_state:7997;"
        );

        let room_only = render_rollback(7997, 41, &nouns, WriteIntent::RoomOnly);
        let sql = [room_only.deletes.clone(), room_only.control.clone()]
            .concat()
            .join("\n");
        assert!(
            !sql.contains("DELETE EQUI")
                && MODEL_FACE_TABLES
                    .iter()
                    .all(|table| !sql.contains(&format!("DELETE {table} ")))
                && !sql.contains("gen_root"),
            "RoomOnly 只退房间面: {sql}"
        );
        assert!(sql.contains("DELETE room_relate WHERE in.dbnum = 7997;"));
        assert!(sql.contains("DELETE room_panel_relate WHERE in.dbnum = 7997;"));
        let watermark = &room_only.control[0];
        assert!(
            !watermark.contains("math::min"),
            "RoomOnly 不动任何一枚水位: {watermark}"
        );
        assert!(watermark.contains("state_revision = (state_revision ?? 0) + 1"));
        // 三种意图的 SQL 仍然一句 SELECT 都没有（W3）。
        for intent in [
            WriteIntent::ApplyWindow,
            WriteIntent::ModelOnly,
            WriteIntent::RoomOnly,
        ] {
            let render = render_rollback(7997, 41, &nouns, intent);
            let sql = [render.deletes, render.control].concat().join("\n");
            assert!(!sql.contains("SELECT"), "{intent:?}: {sql}");
        }
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

    /// §4.3 的 2–6 逐条：数据面 / 模型面 / 房间边 / 两水位 / 标记行，且顺序固定
    /// ——`pe` 顶点排在 noun 行之后（删顶点级联带走挂在它身上的边），标记行最后删
    /// （它是回退的提交点）。
    #[test]
    fn the_rollback_covers_every_face_and_ends_at_the_marker() {
        let render = render_rollback(
            7997,
            41,
            &["ATT_UDA".into(), "EQUI".into(), "PANE".into()],
            WriteIntent::ApplyWindow,
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
                "DELETE {table} WHERE dbnum = 7997 AND model_sesno > 41;"
            ));
        }
        assert!(
            !sql.contains("gen_root"),
            "逐根凭证行随 gen_root 退役（ADR-079）:\n{sql}"
        );
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

        // 两水位一起退（D3 崩溃不分面），且是 min 不是赋值——已经更低的水位不许被抬上来；
        // 行上没有第三枚（ADR-073 W3）。
        let watermark = &render.control[0];
        assert!(!watermark.contains("room_"), "{watermark}");
        for field in ["applied_sesno", "model_sesno"] {
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
        let render = render_rollback(7997, 41, &nouns, WriteIntent::ApplyWindow);
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
            body.contains("if intent.wipes_whole_dbnum()")
                && body.contains("wipe_dbnum_for_reinit"),
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
        assert!(
            sql.contains("DELETE EQUI WHERE dbnum = 7997 AND sesno > 41;"),
            "{sql}"
        );
        assert!(
            sql.contains("DELETE ATT_UDA WHERE dbnum = 7997 AND sesno > 41;"),
            "{sql}"
        );
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

    /// 模型面的表清单从 `RANGE_TABLES` 滤出来，不另抄一份。整库清库把 `ref_rev` 插回原位，
    /// 所以抽完之后三种处置的语句与抽出前逐字相同——这条钉住「清库语句里没有房间谓词删」。
    #[test]
    fn model_face_tables_are_range_tables_without_pe_and_ref_rev() {
        let filtered = RANGE_TABLES
            .iter()
            .copied()
            .filter(|table| is_model_face_range_table(table))
            .collect::<Vec<_>>();
        assert_eq!(
            filtered,
            vec![
                "inst_relate",
                "tubi_relate",
                "room_relate",
                "room_panel_relate",
                "geo_relate",
                "inst_info",
                "aabb",
                "trans",
            ]
        );
        let rendered = render_model_face_wipe(7997, &["24381".into()]);
        for table in &filtered {
            assert!(
                rendered
                    .range_deletes
                    .iter()
                    .any(|sql| sql.starts_with(&format!("DELETE {table}:"))),
                "{table} 必须在模型面区间删里: {:?}",
                rendered.range_deletes
            );
        }
        assert!(rendered.range_deletes.iter().all(|sql| !sql.contains("WHERE")));
        assert_eq!(
            rendered.room_predicates,
            vec![
                "DELETE room_relate WHERE in.dbnum = 7997;".to_string(),
                "DELETE room_panel_relate WHERE in.dbnum = 7997;".to_string(),
            ]
        );
        for disposal in [
            WatermarkDisposal::DropRow,
            WatermarkDisposal::ResetForReinit,
            WatermarkDisposal::RestartInitializationData,
        ] {
            let (relations, ranges, metadata) =
                render_delete_phases(7997, &["24381".into()], &["BRAN".into()], disposal);
            for statement in relations.iter().chain(&ranges).chain(&metadata) {
                assert!(
                    !statement.contains("WHERE in.dbnum"),
                    "整库清库仍走 Ref0 区间，不改用房间谓词: {statement}"
                );
            }
        }
    }

    /// 模型水位半边与 `ResetForReinit` 逐字相同；整库模型清理不碰 pe、noun、交付台账、数据水位行。
    #[test]
    fn the_model_face_wipe_matches_the_reinit_watermark_and_leaves_the_data_face() {
        let (_, _, metadata) = render_delete_phases(
            7997,
            &["24381".into()],
            &["EQUI".into()],
            WatermarkDisposal::ResetForReinit,
        );
        let full = metadata.last().expect("watermark update");
        assert!(
            full.contains(MODEL_WATERMARK_RESET),
            "回退重建的水位句必须含同一段模型半边: {full}"
        );
        let rendered = render_model_face_wipe(7997, &["24381".into()]);
        assert_eq!(
            rendered.metadata[0],
            metadata
                .iter()
                .find(|sql| sql.contains("spatial_epoch:current"))
                .expect("epoch")
                .as_str()
        );
        assert!(
            rendered.metadata[1].contains(MODEL_WATERMARK_RESET),
            "模型清理的水位句必须是同一段: {}",
            rendered.metadata[1]
        );
        assert!(rendered.metadata[1].contains("state_revision = (state_revision ?? 0) + 1"));
        let sql = [
            rendered.range_deletes.clone(),
            rendered.room_predicates.clone(),
            rendered.metadata.clone(),
        ]
        .concat()
        .join("\n");
        assert!(!sql.contains("DELETE pe:") && !sql.contains("DELETE pe;"), "{sql}");
        assert!(!sql.contains("datacenter"), "{sql}");
        assert!(!sql.contains("dbnum_info_table"), "{sql}");
        assert!(!sql.contains("DELETE dbnum_watermark"), "{sql}");
        assert!(!sql.contains("applied_sesno"), "{sql}");
        for noun in ["EQUI", "BRAN", "PANE", "ATT_UDA"] {
            assert!(!sql.contains(&format!("DELETE {noun}")), "{noun}: {sql}");
        }
        assert!(
            !sql.contains("forget_persisted_catalogue_rows"),
            "渲染层不该点目录 memo"
        );
    }

    /// 整库清理连几何一起清的先后（2026-10-22）：生成锁 → 收键 → 删库行 → 删几何。键在锁内、行前收
    /// （`inst_geo` 不按 Ref0 寻址，行一删就收不出来），几何在行后删（反过来会先删掉模型还要用的）；
    /// 库是权威面才收、删库上那一份。
    #[test]
    fn the_model_face_wipe_collects_geometry_before_the_rows_and_purges_after() {
        let body = include_str!("fast_delete.rs")
            .split_once("pub async fn wipe_dbnum_model_face(")
            .expect("model face wipe exists")
            .1
            .split_once("pub(crate) async fn wipe_dbnum_model_face_on(")
            .expect("library half follows")
            .0;
        let lock = body.find("try_lock_db_generation(dbnum)").expect("生成锁");
        let collect = body
            .find("mesh_purge::dbnum_mesh_ids(dbnum, durable)")
            .expect("收键");
        let wipe = body
            .find("wipe_dbnum_model_face_on(&SUL_DB, dbnum)")
            .expect("删库行");
        let purge = body
            .find("mesh_purge::purge_geometry(&SUL_DB, keys, durable)")
            .expect("删几何");
        assert!(
            lock < collect && collect < wipe && wipe < purge,
            "顺序必须是 生成锁 → 收键 → 删库行 → 删几何: {body}"
        );
        assert!(
            body.contains("let durable = route.is_database();"),
            "库是权威面才收、删库上那一份: {body}"
        );
        assert!(
            body.contains("geometry.purges()"),
            "要不要连几何清由调用方的 ClearGeometry 定: {body}"
        );
    }

    /// 写两根几何和房间边，清掉模型面：八张表该库为零，`pe` / noun / 别的库不动，
    /// 模型水位归零且 `state_revision` 加一，数据水位留着。
    #[tokio::test]
    async fn wiping_the_model_face_on_mem_zeroes_eight_tables_and_keeps_pe() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("model_face").use_db("test").await.unwrap();
        db.query(
            "CREATE pe:24381_1 SET dbnum = 8000, sesno = 4, noun = 'BRAN';\
             CREATE pe:24381_10 SET dbnum = 8000, noun = 'BRAN';\
             CREATE pe:24381_20 SET dbnum = 8000, noun = 'BRAN';\
             CREATE inst_relate:24381_10 SET in = pe:24381_10, dbnum = 8000;\
             CREATE inst_relate:24381_20 SET in = pe:24381_20, dbnum = 8000;\
             CREATE inst_info:24381_10 SET dbnum = 8000;\
             CREATE tubi_relate:24381_20 SET in = pe:24381_20, dbnum = 8000;\
             CREATE geo_relate:24381_10 SET dbnum = 8000;\
             CREATE aabb:24381_10 SET dbnum = 8000;\
             CREATE trans:24381_10 SET dbnum = 8000;\
             CREATE ref_rev:24381_1 SET dbnum = 8000;\
             CREATE BRAN:24381_10 SET dbnum = 8000;\
             CREATE room_relate SET in = pe:24381_1, out = pe:24381_10;\
             CREATE room_panel_relate SET in = pe:24381_1, out = pe:24381_20;\
             CREATE aabb:deadbeef SET mesh = true;\
             CREATE pe:31909_1 SET dbnum = 8001, noun = 'EQUI';\
             CREATE inst_relate:31909_1 SET in = pe:31909_1, dbnum = 8001;\
             CREATE dbnum_watermark:8000 SET applied_sesno = 4, sesno = 4, model_sesno = 4, \
                 state_revision = 2;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let result = wipe_dbnum_model_face_on(&db, 8000).await.unwrap();
        assert_eq!(result.dbnum, 8000);
        assert_eq!(result.ref0s, vec!["24381".to_string()]);

        #[derive(Debug, serde::Deserialize)]
        struct Mark {
            applied_sesno: i64,
            model_sesno: i64,
            state_revision: i64,
        }
        let mut response = db
            .query(
                "SELECT applied_sesno, model_sesno, state_revision FROM ONLY dbnum_watermark:8000;\
                 SELECT count() AS count FROM pe WHERE dbnum = 8000 GROUP ALL;\
                 SELECT count() AS count FROM ref_rev:24381_0..24381_9999999999 GROUP ALL;\
                 SELECT count() AS count FROM BRAN:24381_0..24381_9999999999 GROUP ALL;\
                 SELECT count() AS count FROM inst_relate:31909_0..31909_9999999999 GROUP ALL;\
                 SELECT count() AS count FROM aabb:deadbeef GROUP ALL;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        let mark: Option<Mark> = response.take(0).unwrap();
        let mark = mark.expect("水位行还在");
        assert_eq!(mark.model_sesno, 0, "模型水位归零");
        assert_eq!(mark.state_revision, 3, "state_revision 加一");
        assert_eq!(mark.applied_sesno, 4, "数据水位不动");
        for (index, label) in [(1, "pe"), (2, "ref_rev"), (3, "BRAN"), (4, "别的库"), (5, "共享盒")]
        {
            let count = response
                .take::<Vec<CountRow>>(index)
                .unwrap()
                .first()
                .map(|row| row.count)
                .unwrap_or_default();
            assert!(count >= 1, "{label} 不该被整库模型清理删掉，剩下 {count}");
        }
    }
}
