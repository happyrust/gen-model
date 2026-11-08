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
    // 管件目录 P 点分表（ptset 分表计划 W2，`fast_model::ptset_rows`）：record id 与 `pe` 同键 `<ref0>_<ref1>`，
    // 行随模型行同一笔发布写出，所以是模型面：整库清模型 / 重初始化跟 `inst_relate` 一起按 Ref0 区间删，
    // 不然重建后成员已经没了的管件还留着旧 P 点行，长度函数静默算旧值。
    "pe_ptset",
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

fn collect_ref0s(prefix_rows: Vec<Ref0Row>) -> anyhow::Result<(usize, Vec<String>)> {
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
        // `pe_ptset` 行上记的是这根发布时的 `sesno`（= 模型行的 `model_sesno` 口径），超过回退目标的行跟模型行一起退。
        deletes.push(format!(
            "DELETE pe_ptset WHERE dbnum = {dbnum} AND sesno > {target_watermark};"
        ));
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
/// 成首次登记。
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
     model_roots_observed_at = NONE";

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
        // `ATT_UDA` 不是 `pe.noun`，也不进 `RANGE_TABLES`（那份清单会流进整库模型清理）。
        // 基线重解析是 `INSERT IGNORE`，不清它的话旧属性行会赢，删掉的元件留下孤儿。
        // 三种水位处置都清：它是数据面，`RestartInitializationData` 同样要清数据面。
        ranges.push(format!("DELETE {};", range_of("ATT_UDA", ref0)));
        ranges.push(format!("DELETE {};", range_of("pe", ref0)));
    }
    debug_assert!(model_deletes.next().is_none());
    // 第二条异步账的三张表（模型意图队列 / 恢复记录 / 副作用队列）不再逐库清：它们整体退役
    // （ADR-072 D5），随 `retire_publish_run_tables` 的 `REMOVE TABLE IF EXISTS` 一次删掉，
    // 清库路上再点名它们只是给退役表续命。
    // 逐根凭证行不再有（`gen_root` 退役，ADR-079）：整库 wipe / 删库之后「这根已经生成到 T」的旧说法
    // 只可能活在进程内账本里，由调用方 `model_root_ledger::forget_dbnum` 一页作废（`wipe_dbnum_for_reinit`
    // 的调用方与删库口都做）。
    let mut metadata = Vec::new();
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
    crate::data_interface::projection_removal::clear_dbnum(dbnum, false)?;
    // 生成根账本这一页随库一起没了（ADR-079）：库里没有逐根的行，进程里也不该再记着它们。
    crate::data_interface::model_root_ledger::forget_dbnum(dbnum);
    crate::data_interface::cata_dependency::forget_persisted_catalogue_rows(
        crate::data_interface::cata_dependency::ForgetScope::Everything,
    );
    Ok(result)
}

/// 回退整库重建的清库半边（ADR-021）：数据、派生行、noun 行与队列残留
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
    wipe_dbnum_for_reinit_on(&SUL_DB, dbnum).await
}

/// 同 [`wipe_dbnum_for_reinit`]，删的是 `db` 那个库的行（`pe` 事件核对查 `db`，两把写库锁照拿）。
/// 进程内那两份记忆是本进程的，照样作废。
pub(crate) async fn wipe_dbnum_for_reinit_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<FastDeleteDbnumResult> {
    let result = wipe_dbnum_rows_guarded_on(db, dbnum, WatermarkDisposal::ResetForReinit).await?;
    // 进程内模型面与整库模型清理同一串。目录 memo 是数据面，只有整库重建才作废。
    forget_dbnum_model_in_process(dbnum, false)?;
    crate::data_interface::cata_dependency::forget_persisted_catalogue_rows(
        crate::data_interface::cata_dependency::ForgetScope::Everything,
    );
    Ok(result)
}

/// 进程内「这个库的模型」作废：投影（走删除入口，`spool_tombstones` 时先落读透墓碑，落不下就一样都
/// 不动）、生成根账本、空间树，并把读路由翻到内存。整库重建与整库模型清理共用；**不**动目录 memo
/// （那是数据面）。
fn forget_dbnum_model_in_process(dbnum: u32, spool_tombstones: bool) -> anyhow::Result<()> {
    crate::data_interface::projection_removal::clear_dbnum(dbnum, spool_tombstones)?;
    crate::data_interface::model_root_ledger::forget_dbnum(dbnum);
    crate::fast_model::aabb_tree::mark_aabb_tree_dirty();
    crate::data_interface::model_read_route::announce_source_changed(
        dbnum,
        crate::data_interface::model_read_route::ModelReadSource::Memory,
    );
    Ok(())
}

/// 升级批次的清库：与 [`wipe_dbnum_for_reinit`] 清同一份解析行和模型行，另外删掉这个库
/// 引用过的内容寻址几何（`inst_geo`、哈希 `aabb`、`.mesh`）。
///
/// 网格键在生成锁和写库锁之内、删行之前从库上的 `inst_relate` 收。`inst_geo` 不按 Ref0
/// 寻址，行一删就收不出来。升级当时读路由经常是内存，投影里没有这一代的键，所以这里
/// 固定收库上那一份，不看路由。几何文件排在删行之后。单位几何 1、2、3 不删。
/// 人工重新解析和回退重建不走这里。
pub async fn wipe_dbnum_for_upgrade(dbnum: u32) -> anyhow::Result<FastDeleteDbnumResult> {
    if dbnum == 0 {
        bail!("dbnum must be greater than zero");
    }
    let _generation = match crate::fast_model::e3d_model_service::try_lock_db_generation(dbnum) {
        Ok(guard) => guard,
        Err(_) => bail!("dbnum {dbnum} 有生成在飞，升级清库拒绝、一行不删"),
    };
    // 任务在飞锁不试：升级清库只在任务自己的重建阶段里跑，那把锁就在调用方手里。
    let queued = match crate::fast_model::model_write_behind::quiesce_dbnum_for_wipe(dbnum) {
        Ok(queued) => queued,
        Err(busy) => bail!("{busy}，升级清库拒绝、一行不删"),
    };
    if queued.purged + queued.stale > 0 {
        println!(
            "[upgrade-wipe] dbnum={dbnum} 摘掉写后台排队 {} 根（顺带摘掉过期条目 {} 根）",
            queued.purged, queued.stale
        );
    }
    super::pe_batch::require_no_event_on(&SUL_DB).await?;
    let started = Instant::now();
    let commit_guard = crate::data_interface::batch_worker::DATA_COMMIT_SERIAL
        .lock()
        .await;
    let state_guard = crate::data_interface::dbnum_state::DBNUM_STATE_WRITE_GATE
        .write()
        .await;
    // 升级清的是库上这一代。读路由这时常常已经是内存（模型水位落后，或重新解析任务在飞），
    // 进程内投影还没有键。键必须从即将删掉的 `inst_relate` 上收，不能按路由跳过。
    // 整库模型清理仍看路由：那条路在「只写内存」时不该去删库上另一代的几何。
    let keys = mesh_purge::dbnum_mesh_ids(dbnum, true).await?;
    let mut result = wipe_dbnum_rows_on(&SUL_DB, dbnum, WatermarkDisposal::ResetForReinit).await?;
    forget_dbnum_model_in_process(dbnum, false)?;
    crate::data_interface::cata_dependency::forget_persisted_catalogue_rows(
        crate::data_interface::cata_dependency::ForgetScope::Everything,
    );
    drop(state_guard);
    drop(commit_guard);
    let geometry = mesh_purge::purge_geometry(&SUL_DB, keys.clone(), true).await?;
    delete_aabb_mesh_rows(&SUL_DB, &keys).await?;
    println!(
        "存储架构升级：dbnum={dbnum} 已清网格几何（{}）",
        geometry.note()
    );
    result.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(result)
}

/// 整库升级前清掉的共享解析数据。`rows` 是这些目录库清掉之前的 `pe` 行数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedParseWipeReport {
    pub catalogues: usize,
    pub rows: usize,
}

/// 目录库号：依赖状态行 ∪ MDB 里的 CATA 库，去掉 0，升序去重。
pub(crate) fn union_catalogue_dbnums(
    from_state: impl IntoIterator<Item = u32>,
    from_mdb: impl IntoIterator<Item = u32>,
) -> Vec<u32> {
    let mut dbnums = from_state
        .into_iter()
        .chain(from_mdb)
        .filter(|dbnum| *dbnum != 0)
        .collect::<Vec<_>>();
    dbnums.sort_unstable();
    dbnums.dedup();
    dbnums
}

/// 整库升级开始时清共享解析数据：目录库的 `pe` / noun / `ATT_UDA` / `pe_owner` / `ref_rev`、
/// `cata_dependency_state`、全部 `parse_error`，然后作废目录 memo。
///
/// 不动 `dbnum_watermark` 和交付台账。失败时调用方不得排重新解析批次；再发起一次是幂等的。
/// 锁序与 [`wipe_dbnum_rows`] 相同。
pub async fn wipe_shared_parse_rows_for_upgrade() -> anyhow::Result<SharedParseWipeReport> {
    super::pe_batch::require_no_event_on(&SUL_DB).await?;
    let _commit_guard = crate::data_interface::batch_worker::DATA_COMMIT_SERIAL
        .lock()
        .await;
    let _state_guard = crate::data_interface::dbnum_state::DBNUM_STATE_WRITE_GATE
        .write()
        .await;
    let dbnums = catalogue_dbnums_for_upgrade().await?;
    let report = wipe_shared_parse_rows_on(&SUL_DB, &dbnums).await?;
    crate::data_interface::cata_dependency::forget_persisted_catalogue_rows(
        crate::data_interface::cata_dependency::ForgetScope::Everything,
    );
    Ok(report)
}

/// [`wipe_shared_parse_rows_for_upgrade`] 的库那一半，给 `mem://` 用例在自己的实例上跑。
/// 不碰进程内目录 memo，也不读 MDB：目录库号由调用方传入。
pub(crate) async fn wipe_shared_parse_rows_on(
    db: &Surreal<Any>,
    catalogue_dbnums: &[u32],
) -> anyhow::Result<SharedParseWipeReport> {
    let dbnums = union_catalogue_dbnums(catalogue_dbnums.iter().copied(), std::iter::empty());
    let mut planned = Vec::with_capacity(dbnums.len());
    let mut rows = 0usize;
    for dbnum in &dbnums {
        let (pe_rows, ref0s, noun_tables) = collect_dbnum_ref0s_on(db, *dbnum).await?;
        rows += pe_rows;
        let (relations, ranges) = render_catalogue_parse_wipe(&ref0s, &noun_tables);
        planned.push((*dbnum, ref0s, relations, ranges));
    }
    for (dbnum, _, relations, ranges) in &planned {
        execute_phase(db, "delete catalogue owner relations", relations).await?;
        execute_phase(db, "delete catalogue parse ranges", ranges).await?;
        execute_phase(
            db,
            "delete catalogue dependency state",
            &[format!("DELETE cata_dependency_state:{dbnum};")],
        )
        .await?;
    }
    execute_phase(
        db,
        "delete parse errors",
        &["DELETE parse_error;".to_string()],
    )
    .await?;
    for (dbnum, ref0s, _, _) in &planned {
        assert_parse_face_gone(db, *dbnum, ref0s).await?;
        crate::data_interface::dbnum_state::DbnumState::invalidate_mirrored_on(db, *dbnum);
    }
    Ok(SharedParseWipeReport {
        catalogues: dbnums.len(),
        rows,
    })
}

/// 目录库的解析行。不含模型面，也不动水位和交付台账：那些行按目录库号存放，
/// 设计库自己的清库够不着它们。
fn render_catalogue_parse_wipe(
    ref0s: &[String],
    noun_tables: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut relations = Vec::new();
    let mut ranges = Vec::new();
    for ref0 in ref0s {
        relations.push(format!("DELETE {};", owner_range_of(ref0)));
        ranges.push(format!("DELETE {};", range_of("ref_rev", ref0)));
        for table in noun_tables {
            if table == "ATT_UDA" {
                continue;
            }
            ranges.push(format!("DELETE {};", range_of(table, ref0)));
        }
        ranges.push(format!("DELETE {};", range_of("ATT_UDA", ref0)));
        ranges.push(format!("DELETE {};", range_of("pe", ref0)));
    }
    (relations, ranges)
}

async fn catalogue_dbnums_for_upgrade() -> anyhow::Result<Vec<u32>> {
    let from_state = dependency_state_dbnums_on(&SUL_DB).await?;
    let from_mdb = catalogue_dbnums_declared().await?;
    Ok(union_catalogue_dbnums(from_state, from_mdb))
}

async fn dependency_state_dbnums_on(db: &Surreal<Any>) -> anyhow::Result<Vec<u32>> {
    let mut response = db
        .query("SELECT VALUE dbnum FROM cata_dependency_state;")
        .await
        .context("读取 cata_dependency_state 失败")?
        .check()
        .context("读取 cata_dependency_state 失败")?;
    let dbnums: Vec<Option<u32>> = response
        .take(0)
        .context("解码 cata_dependency_state 的库号失败")?;
    Ok(dbnums.into_iter().flatten().collect())
}

async fn catalogue_dbnums_declared() -> anyhow::Result<Vec<u32>> {
    let option = aios_core::get_db_option().clone();
    tokio::task::spawn_blocking(move || {
        let membership = match crate::data_interface::mdb_membership::get(
            &option.project_name,
            &option.mdb_name,
        ) {
            Some(membership) => membership,
            None => std::sync::Arc::new(crate::data_interface::mdb_membership::resolve(
                &option,
                &option.project_name,
                &option.mdb_name,
            )?),
        };
        Ok::<_, anyhow::Error>(
            membership
                .of_type(crate::data_interface::mdb_membership::CATA_STYP)
                .map(|database| database.dbnum)
                .collect::<Vec<_>>(),
        )
    })
    .await
    .map_err(|error| anyhow::anyhow!("读取 MDB 目录库的阻塞任务异常: {error}"))?
}

async fn assert_parse_face_gone(
    db: &Surreal<Any>,
    dbnum: u32,
    ref0s: &[String],
) -> anyhow::Result<()> {
    let mut verify = db
        .query(render_verify_query(dbnum, ref0s))
        .await
        .context("核对目录库清库")?
        .check()
        .context("核对目录库清库失败")?;
    let remaining = verify
        .take::<Vec<CountRow>>(0)
        .context("解码目录库 PE 残留")?
        .first()
        .map(|row| row.count)
        .unwrap_or_default();
    if remaining != 0 {
        bail!("目录库 {dbnum} 清库未完成：还剩 {remaining} 行 PE");
    }
    let mut edges = 0usize;
    for (offset, ref0) in ref0s.iter().enumerate() {
        edges += verify
            .take::<Vec<CountRow>>(offset + 1)
            .with_context(|| format!("解码目录库 {dbnum} 的 pe_owner 残留 {ref0}"))?
            .first()
            .map(|row| row.count)
            .unwrap_or_default();
    }
    if edges != 0 {
        bail!("目录库 {dbnum} 清库未完成：Ref0 区间里还剩 {edges} 条 pe_owner");
    }
    Ok(())
}

const MESH_KEY_DELETE_BATCH: usize = 500;

fn render_aabb_mesh_delete(keys: &[String]) -> String {
    keys.iter()
        .filter(|key| crate::fast_model::e3d_mesh_store::is_valid_mesh_key(key))
        .filter(|key| !mesh_purge::is_shared_unit_key(key))
        .map(|key| format!("DELETE aabb:⟨{key}⟩;"))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn delete_aabb_mesh_rows(
    db: &Surreal<Any>,
    ids: &std::collections::BTreeSet<String>,
) -> anyhow::Result<()> {
    let keys = ids
        .iter()
        .filter(|key| crate::fast_model::e3d_mesh_store::is_valid_mesh_key(key))
        .filter(|key| !mesh_purge::is_shared_unit_key(key))
        .cloned()
        .collect::<Vec<_>>();
    for chunk in keys.chunks(MESH_KEY_DELETE_BATCH) {
        let sql = render_aabb_mesh_delete(chunk);
        if sql.is_empty() {
            continue;
        }
        db.query(sql)
            .await
            .context("delete hashed aabb rows")?
            .check()
            .context("delete hashed aabb rows")?;
    }
    Ok(())
}

/// 整库模型清理拿不到这个库：有按需 / 整库生成在飞（生成锁）、有任务在飞（写入标记到收尾之间），
/// 或写后台正在写这个库的一批根。一行都不删。
#[derive(Debug)]
pub struct ModelFaceGenerationBusy {
    pub dbnum: u32,
    /// 占着这个库的是谁：「生成」「任务」「写后台批次」。
    pub holder: &'static str,
}

impl std::fmt::Display for ModelFaceGenerationBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "dbnum {} 有{}在飞，整库模型清理拒绝、一行不删",
            self.dbnum, self.holder
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
    let busy = |holder| ModelFaceGenerationBusy { dbnum, holder };
    let _generation = crate::fast_model::e3d_model_service::try_lock_db_generation(dbnum)
        .map_err(|_| busy("生成"))?;
    let _task = crate::data_interface::batch_worker::try_lock_dbnum_task(dbnum)
        .map_err(|_| busy("任务"))?;
    let queued = crate::fast_model::model_write_behind::quiesce_dbnum_for_wipe(dbnum)
        .map_err(|_| busy("写后台批次"))?;
    if queued.purged + queued.stale > 0 {
        println!(
            "[model-face-wipe] dbnum={dbnum} 摘掉写后台排队 {} 根（顺带摘掉过期条目 {} 根）",
            queued.purged, queued.stale
        );
    }
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
        forget_dbnum_model_in_process(dbnum, crate::options::data_face().is_read_through())?;
        ModelFaceWipeResult {
            dbnum,
            ref0s: Vec::new(),
            range_statements: 0,
            elapsed_ms: 0,
            geometry: None,
        }
    } else {
        let result = wipe_dbnum_model_face_on(&SUL_DB, dbnum).await?;
        forget_dbnum_model_in_process(dbnum, false)?;
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

async fn wipe_dbnum_rows(
    dbnum: u32,
    watermark: WatermarkDisposal,
) -> anyhow::Result<FastDeleteDbnumResult> {
    wipe_dbnum_rows_guarded_on(&SUL_DB, dbnum, watermark).await
}

/// [`wipe_dbnum_rows`] 删 `db` 那个库：`pe` 事件核对查的是 `db`，两把写库锁照拿。
async fn wipe_dbnum_rows_guarded_on(
    db: &Surreal<Any>,
    dbnum: u32,
    watermark: WatermarkDisposal,
) -> anyhow::Result<FastDeleteDbnumResult> {
    if dbnum == 0 {
        bail!("dbnum must be greater than zero");
    }
    super::pe_batch::require_no_event_on(db).await?;
    let started = Instant::now();
    let _commit_guard = crate::data_interface::batch_worker::DATA_COMMIT_SERIAL
        .lock()
        .await;
    let _state_guard = crate::data_interface::dbnum_state::DBNUM_STATE_WRITE_GATE
        .write()
        .await;

    let result = wipe_dbnum_rows_on(db, dbnum, watermark).await?;
    Ok(FastDeleteDbnumResult {
        elapsed_ms: started.elapsed().as_millis() as u64,
        ..result
    })
}

/// `wipe_dbnum_rows_on` 开头那两句：PE 前缀与 noun。
/// 整库快删与整库模型清理共用，避免两份圈定。
pub(crate) async fn collect_dbnum_ref0s_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<(usize, Vec<String>, Vec<String>)> {
    let mut response = db
        .query(format!(
            "SELECT string::split(<string>id, '_')[0] AS prefix, count() AS count \
             FROM pe WHERE dbnum = {dbnum} GROUP BY prefix;\n\
             SELECT noun FROM pe WHERE dbnum = {dbnum} GROUP BY noun;"
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
    // Ref0 取自本库 PE 行的 id 前缀。不能把 dbnum 数值直接拼进 record range：一个库每个
    // extent 一个 `word0`，例如 dbnum=7333 的 Ref0 是 23717/31909（「RefNo 库归属」）。
    let (pe_rows, ref0s) = collect_ref0s(prefix_rows)
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
            "pe_ptset",
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
        assert!(ranges.contains(&"DELETE ATT_UDA:24381_0..24381_9999999999;".into()));
    }

    /// 升级之后按首次导入重写，写法是 `INSERT IGNORE`。`ATT_UDA` 不在 `pe.noun` 里，
    /// 也不在 `RANGE_TABLES` 里；三种水位处置都要按 Ref0 区间删掉它，并且排在 `pe` 之前。
    #[test]
    fn every_disposal_deletes_att_uda_before_pe() {
        for disposal in [
            WatermarkDisposal::DropRow,
            WatermarkDisposal::ResetForReinit,
            WatermarkDisposal::RestartInitializationData,
        ] {
            let (_, ranges, _) = render_delete_phases(
                7997,
                &["24381".into(), "31909".into()],
                &["EQUI".into()],
                disposal,
            );
            for ref0 in ["24381", "31909"] {
                let uda = format!("DELETE ATT_UDA:{ref0}_0..{ref0}_{RANGE_END};");
                let pe = format!("DELETE pe:{ref0}_0..{ref0}_{RANGE_END};");
                let uda_at = ranges
                    .iter()
                    .position(|sql| sql == &uda)
                    .unwrap_or_else(|| panic!("{disposal:?} 缺少 {uda}: {ranges:?}"));
                let pe_at = ranges
                    .iter()
                    .position(|sql| sql == &pe)
                    .unwrap_or_else(|| panic!("{disposal:?} 缺少 {pe}: {ranges:?}"));
                assert!(
                    uda_at < pe_at,
                    "{disposal:?} 的 ATT_UDA 必须排在 pe 之前: {ranges:?}"
                );
            }
        }
        assert!(
            !RANGE_TABLES.contains(&"ATT_UDA"),
            "ATT_UDA 是数据面，不能进模型面那份清单"
        );
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
    /// 承诺——水位行清值不删行（登记身份保留）、spatial epoch
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
                .all(|sql| !sql.contains("dbnum_info_table") && !sql.contains("dbnum_stats_state")),
            "Ref0 统计已退役（两张表启动时整表删除），清库不再点名它们: {metadata:?}"
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
        // P 点分表随模型面退（行上是发布时的 sesno）。
        at("DELETE pe_ptset WHERE dbnum = 7997 AND sesno > 41;");
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
                && body.contains("wipe_dbnum_for_reinit("),
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

    /// Ref0 取自 PE 前缀：一个库每个 extent 一个 `word0`，dbnum=7333 的 Ref0 是 23717/31909，
    /// 两者按位解码都回到 7333（`refno_route`）。不能把 dbnum 冒充 Ref0。
    #[test]
    fn ref0_ranges_come_from_pe_prefixes_not_from_the_dbnum() {
        let (pe_rows, ref0s) = collect_ref0s(vec![
            Ref0Row {
                prefix: "pe:31909".into(),
                count: 2,
            },
            Ref0Row {
                prefix: "pe:23717".into(),
                count: 3,
            },
        ])
        .expect("valid Ref0 rows");

        assert_eq!(pe_rows, 5);
        assert_eq!(ref0s, vec!["23717", "31909"]);
        for ref0 in &ref0s {
            let word0: u32 = ref0.parse().unwrap();
            assert_eq!(
                crate::data_interface::refno_route::refno_dbnum(aios_core::RefU64::from_two_nums(
                    word0, 1
                )),
                Some(7333),
                "{ref0}"
            );
        }
        let (_, ranges, _) =
            render_delete_phases(7333, &ref0s, &[], WatermarkDisposal::ResetForReinit);
        assert!(ranges.contains(&"DELETE pe:23717_0..23717_9999999999;".into()));
        assert!(ranges.contains(&"DELETE pe:31909_0..31909_9999999999;".into()));
        assert!(
            ranges.iter().all(|sql| !sql.contains("pe:7333_")),
            "{ranges:?}"
        );
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
                "pe_ptset",
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
        assert!(
            rendered
                .range_deletes
                .iter()
                .all(|sql| !sql.contains("WHERE"))
        );
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
        assert!(
            !sql.contains("DELETE pe:") && !sql.contains("DELETE pe;"),
            "{sql}"
        );
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

    /// 整库清理连几何一起清的先后（2026-10-22）：收键 → 删库行 → 删几何。键在行前收（`inst_geo` 不按
    /// Ref0 寻址，行一删就收不出来），几何在行后删（反过来会先删掉模型还要用的）；库是权威面才收、
    /// 删库上那一份。生成锁排在最前由行为测试钉（`projection_removal_contract`：同库在飞时一行不删）；
    /// 这三步要全局 `SUL_DB` 和盘上的 `.mesh`，单测里没有，只能按源码先后钉。
    #[test]
    fn the_model_face_wipe_collects_geometry_before_the_rows_and_purges_after() {
        let body = include_str!("fast_delete.rs")
            .split_once("pub async fn wipe_dbnum_model_face(")
            .expect("model face wipe exists")
            .1
            .split_once("pub(crate) async fn wipe_dbnum_model_face_on(")
            .expect("library half follows")
            .0;
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
            collect < wipe && wipe < purge,
            "顺序必须是 收键 → 删库行 → 删几何: {body}"
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
        for (index, label) in [
            (1, "pe"),
            (2, "ref_rev"),
            (3, "BRAN"),
            (4, "别的库"),
            (5, "共享盒"),
        ] {
            let count = response
                .take::<Vec<CountRow>>(index)
                .unwrap()
                .first()
                .map(|row| row.count)
                .unwrap_or_default();
            assert!(count >= 1, "{label} 不该被整库模型清理删掉，剩下 {count}");
        }
    }

    #[test]
    fn catalogue_dbnums_are_the_union_of_state_and_mdb() {
        assert_eq!(
            union_catalogue_dbnums([7002, 5052, 0], [7002, 8001]),
            vec![5052, 7002, 8001]
        );
        assert!(union_catalogue_dbnums([0], std::iter::empty()).is_empty());
    }

    /// 目录库清理解析行，不碰模型面、水位和交付台账。
    #[test]
    fn the_catalogue_wipe_only_renders_parse_rows() {
        let (relations, ranges) =
            render_catalogue_parse_wipe(&["5052".into()], &["SCOM".into(), "ATT_UDA".into()]);
        let sql = relations
            .iter()
            .chain(&ranges)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(sql.contains("DELETE pe_owner:[pe:5052_0, NONE]..=[pe:5052_9999999999, ..];"));
        assert!(sql.contains("DELETE SCOM:5052_0..5052_9999999999;"));
        assert_eq!(sql.matches("DELETE ATT_UDA:").count(), 1, "{sql}");
        assert!(sql.contains("DELETE ref_rev:5052_0..5052_9999999999;"));
        assert!(
            sql.ends_with("DELETE pe:5052_0..5052_9999999999;")
                || sql.contains("DELETE pe:5052_0..5052_9999999999;")
        );
        for forbidden in [
            "inst_relate",
            "room_relate",
            "datacenter",
            "dbnum_watermark",
            "cata_dependency_state",
        ] {
            assert!(
                !sql.contains(forbidden),
                "{forbidden} 不该出现在目录解析清理里: {sql}"
            );
        }
    }

    #[test]
    fn aabb_mesh_deletes_skip_shared_unit_keys_and_illegal_keys() {
        let sql = render_aabb_mesh_delete(&[
            "aa".into(),
            "1".into(),
            "2".into(),
            "3".into(),
            "../x".into(),
        ]);
        assert_eq!(sql, "DELETE aabb:⟨aa⟩;");
    }

    /// 升级清库先收网格键，再删行，最后删几何。收键若排到删行之后，哈希几何就再也对不上。生成锁排在
    /// 最前由行为测试钉（`projection_removal_contract`）；这几步要全局 `SUL_DB` 和盘上的 `.mesh`，只能按源码
    /// 先后钉。
    #[test]
    fn the_upgrade_wipe_collects_mesh_keys_before_the_rows_and_purges_after() {
        let body = include_str!("fast_delete.rs")
            .split_once("pub async fn wipe_dbnum_for_upgrade(")
            .expect("upgrade wipe")
            .1
            .split_once("pub async fn wipe_shared_parse_rows_for_upgrade(")
            .expect("shared wipe follows")
            .0;
        let collect = body
            .find("mesh_purge::dbnum_mesh_ids(dbnum, true)")
            .expect("收键");
        let wipe = body.find("wipe_dbnum_rows_on(").expect("删行");
        let purge = body.find("mesh_purge::purge_geometry(").expect("删几何");
        let aabb = body.find("delete_aabb_mesh_rows(").expect("删哈希盒");
        assert!(
            collect < wipe && wipe < purge && purge < aabb,
            "顺序必须是 收键 → 删行 → 删 inst_geo/.mesh → 删哈希 aabb: {body}"
        );
        assert!(
            body.contains("purge_geometry(&SUL_DB, keys.clone(), true)"),
            "升级清库必须删库上的 inst_geo，不能按读路由跳过: {body}"
        );
        assert!(
            !body.contains("route.is_database()"),
            "升级当时读路由经常是内存，按它跳过就收不到即将删掉的网格键: {body}"
        );
        assert!(
            body.contains("is_shared_unit_key")
                || include_str!("fast_delete.rs").contains("is_shared_unit_key"),
            "单位几何必须跳过"
        );
    }

    /// 共享清理在写库锁内做完，再作废目录 memo。memo 若先作废，并发任务会按空记忆把旧行当成没写过。
    #[test]
    fn the_shared_wipe_forgets_the_catalogue_memo_after_the_rows() {
        let body = include_str!("fast_delete.rs")
            .split_once("pub async fn wipe_shared_parse_rows_for_upgrade(")
            .expect("shared wipe")
            .1
            .split_once("pub(crate) async fn wipe_shared_parse_rows_on(")
            .expect("library half follows")
            .0;
        let rows = body.find("wipe_shared_parse_rows_on(").expect("清行");
        let forget = body
            .find("forget_persisted_catalogue_rows(")
            .expect("作废 memo");
        assert!(rows < forget, "先清行再作废 memo: {body}");
        assert!(body.contains("ForgetScope::Everything"), "{body}");
        assert!(body.contains("require_no_event_on"), "{body}");
        assert!(body.contains("DATA_COMMIT_SERIAL"), "{body}");
        assert!(body.contains("DBNUM_STATE_WRITE_GATE"), "{body}");
    }

    /// 目录库解析行、依赖状态和 parse_error 清掉；设计库、水位、交付台账留下。
    #[tokio::test]
    async fn wiping_shared_parse_rows_on_mem_zeroes_catalogue_rows_and_keeps_delivery_ledgers() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("shared_parse").use_db("test").await.unwrap();
        db.query(
            "CREATE pe:5052_10 SET dbnum = 5052, noun = 'SCOM';\
             CREATE pe:5052_11 SET dbnum = 5052, noun = 'SCOM';\
             RELATE pe:5052_10->pe_owner:[pe:5052_10, 0]->pe:5052_11;\
             CREATE SCOM:5052_10 SET dbnum = 5052, sesno = 3;\
             CREATE ATT_UDA:5052_10 SET dbnum = 5052, sesno = 3;\
             CREATE ref_rev:5052_10 SET dbnum = 5052;\
             CREATE cata_dependency_state:5052 SET dbnum = 5052, sesno = 9, rows = 2;\
             CREATE cata_dependency_state:7002 SET dbnum = 7002, sesno = 1, rows = 0;\
             CREATE parse_error:sample SET kind = 'element', target = '5052/10';\
             CREATE dbnum_watermark:5052 SET applied_sesno = 9, sesno = 9;\
             CREATE datacenter_version:keep SET mark = 'stay';\
             CREATE datacenter_delete:keep SET mark = 'stay';\
             CREATE pe:24381_1 SET dbnum = 7997, noun = 'EQUI';\
             CREATE ATT_UDA:24381_1 SET dbnum = 7997;\
             CREATE EQUI:24381_1 SET dbnum = 7997;\
             CREATE cata_dependency_state:7003 SET dbnum = 7003, sesno = 2, rows = 1;\
             CREATE pe:7003_1 SET dbnum = 7003, noun = 'SCOM';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let report = wipe_shared_parse_rows_on(&db, &[7002, 5052, 5052])
            .await
            .unwrap();
        assert_eq!(report.catalogues, 2);
        assert_eq!(report.rows, 2, "只数被清目录库的 pe 行");

        let mut response = db
            .query(
                "SELECT count() AS count FROM pe WHERE dbnum = 5052 GROUP ALL;\
                 SELECT count() AS count FROM SCOM:5052_0..5052_9999999999 GROUP ALL;\
                 SELECT count() AS count FROM ATT_UDA:5052_0..5052_9999999999 GROUP ALL;\
                 SELECT count() AS count FROM ref_rev:5052_0..5052_9999999999 GROUP ALL;\
                 SELECT count() AS count FROM cata_dependency_state:5052 GROUP ALL;\
                 SELECT count() AS count FROM cata_dependency_state:7002 GROUP ALL;\
                 SELECT count() AS count FROM parse_error GROUP ALL;\
                 SELECT count() AS count FROM dbnum_watermark:5052 GROUP ALL;\
                 SELECT count() AS count FROM datacenter_version:keep GROUP ALL;\
                 SELECT count() AS count FROM datacenter_delete:keep GROUP ALL;\
                 SELECT count() AS count FROM pe WHERE dbnum = 7997 GROUP ALL;\
                 SELECT count() AS count FROM ATT_UDA:24381_0..24381_9999999999 GROUP ALL;\
                 SELECT count() AS count FROM cata_dependency_state:7003 GROUP ALL;\
                 SELECT count() AS count FROM pe WHERE dbnum = 7003 GROUP ALL;\
                 SELECT count() AS count FROM pe_owner:[pe:5052_0, NONE]..=[pe:5052_9999999999, ..] GROUP ALL;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        for index in 0..7 {
            let count = response
                .take::<Vec<CountRow>>(index)
                .unwrap()
                .first()
                .map(|row| row.count)
                .unwrap_or_default();
            assert_eq!(count, 0, "查询 {index} 应该被清掉");
        }
        for (index, label) in [
            (7, "目录库水位"),
            (8, "datacenter_version"),
            (9, "datacenter_delete"),
            (10, "设计库 pe"),
            (11, "设计库 ATT_UDA"),
            (12, "未点名的目录状态"),
            (13, "未点名的目录 pe"),
        ] {
            let count = response
                .take::<Vec<CountRow>>(index)
                .unwrap()
                .first()
                .map(|row| row.count)
                .unwrap_or_default();
            assert!(count >= 1, "{label} 不该被共享清理删掉，剩下 {count}");
        }
        let owners = response
            .take::<Vec<CountRow>>(14)
            .unwrap()
            .first()
            .map(|row| row.count)
            .unwrap_or_default();
        assert_eq!(owners, 0, "目录库的 pe_owner 区间必须清空");
    }

    /// `wipe_dbnum_for_reinit_on` 只清给它的那个库：两个库同一 dbnum 各有行，清 `a`，`b` 一行不少。
    /// 清库会整份作废进程级目录记忆，拿着 memo 单测那把锁跑；锁留在同步栈上，不跨 `.await`。
    #[test]
    fn wiping_for_reinit_on_one_database_leaves_the_other_alone() {
        use crate::data_interface::dbnum_initialize::{
            baseline_counts_on, two_databases_sharing_a_dbnum,
        };
        let _memo = crate::data_interface::cata_dependency::memo_test_guard();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (a, b) = two_databases_sharing_a_dbnum(7963, 24363).await;

                let report = wipe_dbnum_for_reinit_on(&a, 7963).await.unwrap();

                assert_eq!(report.pe_rows, 3);
                assert_eq!(baseline_counts_on(&a, 7963).await.unwrap(), (0, 0));
                assert_eq!(
                    baseline_counts_on(&b, 7963).await.unwrap(),
                    (5, 2),
                    "另一个库一行不动"
                );
            });
    }
}
