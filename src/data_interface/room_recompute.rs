//! 面板自身的变化 → 本库整库房间归属重算（`RoomOnly`，d-819 / ADR-073 D13）。
//!
//! 逐根路径（ADR-073 D1）只算「本根在哪些面板里」。面板那一侧变了——PANE 的几何 / 属主、
//! FRMW / SBFR 改名（房间号、进出 `room_key_word`）、`project_hd` 下 CWALL / CFLOOR 搬迁——
//! 它名下**别的根**的成员边与 `room_num` 一个都不会被碰到：这是逐根路径的天然盲区
//! （ADR-073 §未决 U1），原来填它的结构触发账本（ADR-010 D12 / ADR-040 D4）已随房间流程退役。
//!
//! 现行填法（d-819）：两条触发，同一种处置。
//!
//! 1. **规划期**：窗口规划器看本窗变更集（e3d-model 的 `ChangeLedger`），碰到房间定义族的
//!    元素（[`ROOM_DEFINITION_NOUNS`]，增 / 删 / 改属主 / 改成员 / 改属性都算）→ 给本库排一条
//!    `ModelWorkAction::RoomOnly` 工作项（[`room_only_work_item`]）。**不解析 `detail` 散文**
//!    ——DESC 之类只改元数据的面板改动会多触发一次整库重算，接受：那是 O(本库构件) 的内存判定
//!    加一批边的重写，而漏掉一次就是静默陈旧。
//! 2. **执行期**：任务开工前对齐（ADR-073 D12）换了房间代号——别的库的面板 / 房间变了、进了
//!    常驻——本库的成员边同样可能陈旧，一并重算（跨库那一半只覆盖「变了之后第一个开工的库」，
//!    §未决 U3 仍开着）。
//!
//! 处置：任务在本批根**刷盘之后、收尾之前**（`batch_worker::execute_frozen_batch`），在收尾段
//! 冻一台计算器，对本库全部有回执的根按投影里的**现有几何**重算归属、重写两张边
//! （`model_db_adapter::recompute_dbnum_room_edges`），不重生几何、不动模型行、不动空间树。
//! 算不出的根按 D2 记失败、`model_sesno` 不盖。它是任务内部的一步，不入队、没有 HTTP 口
//! （ADR-073 §不做「不给房间开新的 HTTP 触发口」）。
//!
//! **欠账（2026-09-20，审核 P1-2）**：触发是边沿型的——它只在碰到那条 PANE 变更的那一窗出现一次。
//! 那一窗要是没跑到收尾段（预算拆窗、单元失败、刷盘没齐）或重算写库失败，原因随任务回执一起丢，
//! 下一窗的账本里不再有那条 PANE 变更、开工前对齐再探也是 `Unchanged`：本库的成员边就此**永久陈旧**
//! 到下一次不相关的 PANE 变化。所以触发一出现就落库记成一条**欠账**
//! （[`ROOM_RECOMPUTE_OWED_TABLE`]`:{dbnum}`），每个 DESI 任务的收尾段把本窗自己的触发与欠账合成
//! 一句一起跑（[`reason_with_owed`]）；只有整库对齐真盖上了（跑齐了整库重算、或整库重排 /
//! 初始化让每一根都是这一代算的）才销账（[`discharge`]）。欠账是 durable 的：普通增量窗口成功
//! 后 `model_sesno` 会越过那条变更，重启后按水位缺口重排的 ModelOnly 也看不到它，进程内存记不住。

use anyhow::Context;
use surrealdb::{Surreal, engine::any::Any};

use aios_core::SUL_DB;

use super::dbnum_state::escape_surql_str;
use crate::data_interface::model_update_plan::{ModelWorkAction, ModelWorkItem};
use crate::fast_model::room_resident::RoomReferenceRefresh;

/// 房间定义族：房间节点（FRMW / SBFR）、`project_hd` 的容器（CWALL / CFLOOR）、面板本体（PANE）。
/// 与 `room_topology::RoomHierarchy` 两种口径的并集——按 noun 判，不分口径：不在本项目层级里的
/// noun 在本项目的文件里本来就不出现。
pub(crate) const ROOM_DEFINITION_NOUNS: [&str; 5] = ["PANE", "FRMW", "SBFR", "CWALL", "CFLOOR"];

/// 变更集里碰到房间定义族的元素了没有。`Some(原因)` 点名前几个 `noun refno（改动种类）`；
/// `None` = 这一窗一个都没碰。`Unresolved`（连是什么都读不出）的条目不算——它的 noun 是 `"?"`。
pub(crate) fn room_definition_change(entries: &[e3d_model::ledger::LedgerEntry]) -> Option<String> {
    let hits: Vec<&e3d_model::ledger::LedgerEntry> = entries
        .iter()
        .filter(|entry| entry.kind != e3d_model::ledger::ChangeKind::Unresolved)
        .filter(|entry| {
            ROOM_DEFINITION_NOUNS
                .iter()
                .any(|noun| noun.eq_ignore_ascii_case(&entry.noun))
        })
        .collect();
    if hits.is_empty() {
        return None;
    }
    let sample = hits
        .iter()
        .take(5)
        .map(|entry| {
            format!(
                "{} {}（{}）",
                entry.noun.to_ascii_uppercase(),
                entry.refno,
                entry.kind.label()
            )
        })
        .collect::<Vec<_>>()
        .join("、");
    Some(if hits.len() > 5 {
        format!("{} 个房间定义族元素变了，例如 {sample}", hits.len())
    } else {
        format!("{} 个房间定义族元素变了：{sample}", hits.len())
    })
}

/// 本库这一窗的 `RoomOnly` 工作项：`target_refno = dbnum=<n>`（不是根，一窗最多一条）。
pub(crate) fn room_only_work_item(dbnum: u32, db_type: &str, end_sesno: i32) -> ModelWorkItem {
    ModelWorkItem {
        dbnum,
        db_type: db_type.to_string(),
        source_end_sesno: end_sesno,
        action: ModelWorkAction::RoomOnly,
        target_refno: format!("dbnum={dbnum}"),
        noun: String::new(),
    }
}

/// 这个任务要不要跑整库房间重算，以及为什么。两条触发合成一句：规划器排的 `RoomOnly` 工作项
/// （本窗碰了房间定义族）、开工前对齐换了房间代号。都没有 = `None`。
pub(crate) fn request_for_task(
    side_actions: &[ModelWorkItem],
    refresh: Option<&RoomReferenceRefresh>,
) -> Option<String> {
    let mut reasons = Vec::new();
    if side_actions
        .iter()
        .any(|item| item.action == ModelWorkAction::RoomOnly)
    {
        reasons.push("本窗变更集碰到房间定义族（PANE / FRMW / SBFR / CWALL / CFLOOR）".to_string());
    }
    if let Some(RoomReferenceRefresh::Rebuilt {
        generation_changed: true,
        diff,
        ..
    }) = refresh
    {
        reasons.push(format!(
            "开工前对齐换了房间代号{}",
            diff.as_deref()
                .map(|diff| format!("（在册视图之差 {diff}）"))
                .unwrap_or_default()
        ));
    }
    (!reasons.is_empty()).then(|| reasons.join("；"))
}

/// 一库一行的整库房间重算欠账表（审核 P1-2）。行在 = 这个库欠着一次整库重算；行不在 = 不欠。
/// 与 `dbnum_write_state` 同一种形态（控制面、一库一行、SCHEMALESS），不是水位、不参与任何放行判决。
pub const ROOM_RECOMPUTE_OWED_TABLE: &str = "dbnum_room_recompute_owed";

/// 库里那一行的解出形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoomRecomputeOwed {
    pub dbnum: u32,
    /// 最近一次记账时的触发原因（[`request_for_task`] 的那一句）。
    pub reason: String,
    /// 最近一次记账的任务。
    pub task_id: String,
    /// 第一次欠下的时刻；再记账不覆盖——欠了多久要从这里数。
    pub armed_at: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct OwedRow {
    dbnum: u32,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    task_id: String,
    #[serde(default)]
    armed_at: Option<String>,
}

impl From<OwedRow> for RoomRecomputeOwed {
    fn from(row: OwedRow) -> Self {
        Self {
            dbnum: row.dbnum,
            reason: row.reason,
            task_id: row.task_id,
            armed_at: row.armed_at,
        }
    }
}

/// 记账那一句：`UPSERT`——欠账可以反复记（第二次触发到来时上一次还没销），`armed_at` 只在第一次
/// 落下（`??` 取旧值），`reason` / `task_id` 换成最近的这一次。
pub(crate) fn render_arm(dbnum: u32, reason: &str, task_id: &str) -> String {
    format!(
        "UPSERT {ROOM_RECOMPUTE_OWED_TABLE}:{dbnum} SET dbnum = {dbnum}, reason = '{}', \
         task_id = '{}', armed_at = armed_at ?? time::now(), updated_at = time::now();",
        escape_surql_str(reason),
        escape_surql_str(task_id)
    )
}

/// 销账那一句：整行删掉。**只在整库对齐真盖上了之后**拼它——整库重算跑齐、或整库重排 / 初始化
/// 让本库每一根都是这一代算的；跑不齐、刷盘没齐、开关关着都不销。
pub(crate) fn render_discharge(dbnum: u32) -> String {
    format!("DELETE {ROOM_RECOMPUTE_OWED_TABLE}:{dbnum};")
}

/// `armed_at` 的读法：逐格判 NONE（`type::string(NONE)` 在这个 fork 上直接报错，同 `write_state`）。
const ARMED_AT_COLUMN: &str =
    "IF armed_at = NONE THEN NONE ELSE type::string(armed_at) END AS armed_at";

/// 记一笔欠账（触发一出现就记，早于刷盘栅栏——拆窗 / 失败 / 刷盘没齐都在它后面）。
pub(crate) async fn arm(dbnum: u32, reason: &str, task_id: &str) -> anyhow::Result<()> {
    arm_on(&SUL_DB, dbnum, reason, task_id).await
}

pub(crate) async fn arm_on(
    db: &Surreal<Any>,
    dbnum: u32,
    reason: &str,
    task_id: &str,
) -> anyhow::Result<()> {
    db.query(render_arm(dbnum, reason, task_id))
        .await
        .with_context(|| format!("arm room recompute owed for dbnum {dbnum}"))?
        .check()
        .with_context(|| format!("arm room recompute owed for dbnum {dbnum} statement"))?;
    Ok(())
}

/// 这个库欠着的那一笔（没有 = `None`）。
pub(crate) async fn owed(dbnum: u32) -> anyhow::Result<Option<RoomRecomputeOwed>> {
    owed_on(&SUL_DB, dbnum).await
}

pub(crate) async fn owed_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<Option<RoomRecomputeOwed>> {
    let mut response = db
        .query(format!(
            "SELECT dbnum, reason, task_id, {ARMED_AT_COLUMN} FROM {ROOM_RECOMPUTE_OWED_TABLE}:{dbnum};"
        ))
        .await
        .with_context(|| format!("load room recompute owed of dbnum {dbnum}"))?
        .check()
        .with_context(|| format!("load room recompute owed of dbnum {dbnum} statement"))?;
    let rows: Vec<OwedRow> = response
        .take(0)
        .with_context(|| format!("decode room recompute owed of dbnum {dbnum}"))?;
    Ok(rows.into_iter().next().map(RoomRecomputeOwed::from))
}

/// 销账。调用时机归调用方守：D14 那一格盖 `Aligned` 的同一处。
pub(crate) async fn discharge(dbnum: u32) -> anyhow::Result<()> {
    discharge_on(&SUL_DB, dbnum).await
}

pub(crate) async fn discharge_on(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<()> {
    db.query(render_discharge(dbnum))
        .await
        .with_context(|| format!("discharge room recompute owed of dbnum {dbnum}"))?
        .check()
        .with_context(|| format!("discharge room recompute owed of dbnum {dbnum} statement"))?;
    Ok(())
}

/// 收尾段这一次要跑的原因：本窗自己的触发（[`request_for_task`]）与库里欠着的那一笔合成一句。
/// 两个都没有 = `None`，不跑。欠账那半句点名是哪个任务、什么时候欠下的——日志里读得出「这不是本窗
/// 触发的，是在替上一窗还债」。
pub(crate) fn reason_with_owed(
    this_window: Option<&str>,
    owed: Option<&RoomRecomputeOwed>,
) -> Option<String> {
    let owed = owed.map(|owed| {
        format!(
            "上一窗欠下的整库房间重算未还（任务 {}{}：{}）",
            owed.task_id,
            owed.armed_at
                .as_deref()
                .map(|at| format!("，{at} 记账"))
                .unwrap_or_default(),
            owed.reason
        )
    });
    match (this_window, owed) {
        (None, None) => None,
        (Some(reason), None) => Some(reason.to_owned()),
        (None, Some(owed)) => Some(owed),
        (Some(reason), Some(owed)) => Some(format!("{reason}；{owed}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use e3d_io::refno::RefNo;
    use e3d_model::ledger::{ChangeKind, LedgerEntry};

    fn entry(noun: &str, kind: ChangeKind, id: u32) -> LedgerEntry {
        LedgerEntry {
            refno: RefNo::new(24383, id),
            noun: noun.into(),
            kind,
            detail: String::new(),
            preempted_by: None,
        }
    }

    /// 房间定义族任一 noun、任一种改动都触发；别的 noun 不触发；读不出的条目不算。
    #[test]
    fn a_change_to_any_room_definition_noun_triggers_and_nothing_else_does() {
        assert!(room_definition_change(&[]).is_none());
        assert!(
            room_definition_change(&[
                entry("BRAN", ChangeKind::AttributesModified, 1),
                entry("EQUI", ChangeKind::Reparented, 2),
                entry("ZONE", ChangeKind::MembersChanged, 3),
            ])
            .is_none(),
            "构件 / 容器变了是逐根路径的事，不是房间定义"
        );
        assert!(
            room_definition_change(&[entry("?", ChangeKind::Unresolved, 4)]).is_none(),
            "连是什么都读不出的条目不算"
        );
        for (noun, kind) in [
            ("PANE", ChangeKind::AttributesModified),
            ("pane", ChangeKind::Created),
            ("FRMW", ChangeKind::AttributesModified),
            ("SBFR", ChangeKind::Reparented),
            ("CWALL", ChangeKind::MembersChanged),
            ("CFLOOR", ChangeKind::Deleted),
        ] {
            let reason = room_definition_change(&[
                entry("BRAN", ChangeKind::AttributesModified, 1),
                entry(noun, kind, 7),
            ])
            .unwrap_or_else(|| panic!("{noun} {kind:?} 必须触发"));
            assert!(
                reason.contains(&noun.to_ascii_uppercase()) && reason.contains("24383/7"),
                "{reason}"
            );
        }
    }

    /// 原因点名前 5 个，多了说总数。
    #[test]
    fn the_reason_names_a_bounded_sample() {
        let entries: Vec<_> = (0..8)
            .map(|id| entry("PANE", ChangeKind::AttributesModified, id))
            .collect();
        let reason = room_definition_change(&entries).unwrap();
        assert!(
            reason.starts_with("8 个房间定义族元素变了，例如 "),
            "{reason}"
        );
        assert_eq!(reason.matches("PANE 24383/").count(), 5, "{reason}");
    }

    /// 工作项：`RoomOnly`，目标是库不是根，一窗一条（同键去重）。
    #[test]
    fn the_room_only_item_targets_the_library_not_a_root() {
        let item = room_only_work_item(7997, "DESI", 163);
        assert_eq!(item.action, ModelWorkAction::RoomOnly);
        assert_eq!(item.target_refno, "dbnum=7997");
        assert_eq!((item.dbnum, item.source_end_sesno), (7997, 163));
        assert!(item.noun.is_empty());
        assert!(
            !item.action.is_room_recalc(),
            "它要随 side_actions 交给任务，不能被 `is_room_recalc` 那道旧过滤拦下"
        );
    }

    /// 源码钉（d-819 / ADR-073 D13 / 源码钉 17）：接线的三处各在其位——规划器从账本判房间定义族并排
    /// `RoomOnly`；执行体把「要不要」交出去；收尾段在**刷盘之后、收尾之前**、`flush.all_acked()` 门内跑，
    /// 跑不齐就扣 `all_roots_ok`。
    /// 从 `start` 起到第一行只有 `}` 的地方（LF / CRLF 都认）。
    fn function_body<'a>(source: &'a str, start: &str) -> &'a str {
        let body = source.split_once(start).expect("function start").1;
        let end = body
            .find("\n}\r\n")
            .or_else(|| body.find("\n}\n"))
            .expect("function end");
        &body[..end]
    }

    #[test]
    fn the_room_only_recompute_is_wired_after_the_durable_barrier_and_before_task_end() {
        let body = function_body(
            include_str!("window_root_plan.rs"),
            "pub(crate) async fn build_model_update_plan_from_window(",
        );
        assert!(
            body.contains("room_definition_change(")
                && body.contains("sources.plan.ledger.entries()"),
            "规划器从本窗账本判房间定义族: {body}"
        );
        assert!(body.contains("room_only_work_item("), "排 RoomOnly 工作项");

        let worker = include_str!("batch_worker.rs");
        let executor = worker
            .split_once("async fn execute_frozen_batch_body(")
            .expect("执行体")
            .1
            .split_once("fn batch_regen_is_allowed(")
            .expect("执行体边界")
            .0;
        assert!(
            executor.contains("room_recompute::request_for_task("),
            "执行体把「要不要整库重算」交出去: {executor}"
        );
        // 触发不因单元失败而丢（审核 P1-2）：批次执行体只在模型半边没跑（让位）时给 `None`，
        // `settlement_failed` 不再拦它——失败的那一窗记成欠账，留到下一个任务还。
        let frozen_trigger = executor
            .split_once("let room_recompute = if")
            .expect("批次执行体交出「要不要」那一句")
            .1
            .split_once("};")
            .expect("那一句的结尾")
            .0;
        assert!(
            frozen_trigger.contains("defer_model_phase")
                && !frozen_trigger.contains("settlement_failed"),
            "批次执行体：模型半边让位才 None，单元失败照样交出去: {frozen_trigger}"
        );
        // ModelOnly 执行体同样：启动追平窗口与稳态窗口共用同一条规划入口，`RoomOnly` 工作项随
        // `side_actions` 进来，这里此前硬编码 `room_recompute: None`，停机期间的 PANE 改动被静默吃掉。
        let model_only = function_body(worker, "async fn execute_model_only_body(");
        assert!(
            model_only.contains("room_recompute::request_for_task(")
                && model_only.contains("Some(&room_reference)"),
            "ModelOnly 执行体也要把「要不要整库重算」交出去（两条触发都带上）: {model_only}"
        );
        // 规划失败的早退（模型半边没跑）仍是 `None`；跑完的那一份回执必须接住上面那句的结果。
        assert_eq!(
            model_only.matches("room_recompute: None").count(),
            1,
            "只有规划失败的早退才硬编码不重算: {model_only}"
        );
        let model_only_trigger = model_only
            .split_once("let room_recompute = if")
            .expect("ModelOnly 执行体交出「要不要」那一句")
            .1
            .split_once("};")
            .expect("那一句的结尾")
            .0;
        assert!(
            model_only_trigger.contains("whole_library_model_only(job)")
                && !model_only_trigger.contains("failed"),
            "整库重排（起点 0）每一根都刚算过，不再空转一遍；生成失败不再拦触发（P1-2）: {model_only_trigger}"
        );

        // 欠账（P1-2）：执行体一返回就记，早于拆窗余量 / 刷盘栅栏 / 重算本身。
        let outer = worker
            .split_once("async fn execute_frozen_batch(")
            .expect("外层执行体")
            .1
            .split_once("async fn execute_model_only_body(")
            .expect("外层执行体的边界")
            .0;
        let body_returned = outer
            .find("let body_ms = body_started.elapsed().as_millis();")
            .expect("执行体返回点");
        let armed = outer
            .find("room_recompute::arm(job.dbnum, reason, &job.task_id)")
            .expect("记欠账");
        let remainder = outer.find("requeue_window_remainder(").expect("拆窗余量");
        let durable = outer.find("await_units_durable(").expect("刷盘栅栏");
        assert!(
            body_returned < armed && armed < remainder && armed < durable,
            "欠账在执行体返回之后立刻记、早于拆窗余量与刷盘栅栏: {outer}"
        );
        assert!(
            outer[body_returned..armed].contains("!result.initialization_closed_task")
                && outer[body_returned..armed].contains("crate::options::room_membership()"),
            "初始化双 lane 不欠、开关关着不记: {}",
            &outer[body_returned..armed]
        );

        let tail = worker
            .split_once(
                "if !initialization_closed_task && round_complete && job.db_type.eq_ignore_ascii_case(\"DESI\")",
            )
            .expect("DESI 收尾门")
            .1
            .split_once("registry.set_stage(&job.task_id, \"task_end\")")
            .expect("收尾那一笔")
            .0;
        let flush = tail.find("await_units_durable(").expect("先等刷盘");
        let owed = tail
            .find("room_recompute::owed(job.dbnum)")
            .expect("读欠账");
        let merged = tail
            .find("room_recompute::reason_with_owed(")
            .expect("本窗触发与欠账合成一句");
        let rooms = tail
            .find("run_room_only_recompute(job, &reason, &mut result)")
            .expect("整库房间重算在收尾段");
        let outcome = tail.find("model_outcome_of(&flush").expect("模型收尾判决");
        let discharge = tail
            .find("room_recompute::discharge(job.dbnum)")
            .expect("销账");
        assert!(
            flush < owed
                && owed < merged
                && merged < rooms
                && rooms < outcome
                && outcome < discharge,
            "刷盘之后读欠账、合成一句、重算、模型收尾判决，最后才销账: {tail}"
        );
        assert!(
            tail.contains("Some(reason) if flush.all_acked()"),
            "刷盘没齐就不跑（那批根已判死）: {tail}"
        );
        assert!(
            tail.contains("outcome.all_roots_ok = false"),
            "算不齐扣 all_roots_ok、水位不盖: {tail}"
        );
        assert!(
            tail[outcome..discharge].contains("RoomGenerationStamp::Aligned(_)"),
            "只有 durable 侧整库对齐真盖上了（Aligned）才销账: {}",
            &tail[outcome..discharge]
        );
        assert!(
            tail[..owed].contains("whole_library_model_only(job)"),
            "整库重排的 ModelOnly 不替欠账再空转一遍，它盖 Aligned 时一并销: {}",
            &tail[..owed]
        );

        // 初始化双 lane 盖 `Aligned` 的同一处也销账。
        let init = include_str!("dbnum_initialize.rs");
        let commit = init
            .find("commit_task_end_on(&aios_core::SUL_DB, &end)")
            .expect("初始化收尾一笔");
        let init_discharge = init
            .find("room_recompute::discharge(dbnum)")
            .expect("初始化销账");
        assert!(
            commit < init_discharge
                && init[commit..init_discharge].contains("RoomGenerationStamp::Aligned(_)"),
            "初始化收尾一笔落了、盖的是 Aligned 才销账: {}",
            &init[commit..init_discharge]
        );

        // 旧过滤（「房间重算项不在内」）不得把 RoomOnly 拦在 side_actions 之外。
        let manual = include_str!("manual_update.rs");
        assert!(
            manual.contains("&& !item.action.is_room_recalc()"),
            "side_actions 的过滤只拦 RoomRecalc*，RoomOnly 经它交给任务"
        );
    }

    /// 欠账那一行（审核 P1-2）：`UPSERT` 记、可反复记、`armed_at` 只落第一次；`DELETE` 销；
    /// 读回逐格判 NONE。`mem://` 上走一遍。
    #[tokio::test]
    async fn the_owed_marker_round_trips_and_keeps_the_first_arming_time() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("room_recompute").use_db("owed").await.unwrap();

        assert_eq!(owed_on(&db, 7997).await.unwrap(), None, "没记过就不欠");

        arm_on(
            &db,
            7997,
            "本窗变更集碰到房间定义族（PANE / FRMW / SBFR / CWALL / CFLOOR）",
            "task-a",
        )
        .await
        .unwrap();
        let first = owed_on(&db, 7997).await.unwrap().expect("欠着");
        assert_eq!((first.dbnum, first.task_id.as_str()), (7997, "task-a"));
        assert!(first.reason.contains("房间定义族"), "{first:?}");
        let armed_at = first.armed_at.clone().expect("第一次记账落 armed_at");

        arm_on(&db, 7997, "开工前对齐换了房间代号", "task-b")
            .await
            .unwrap();
        let again = owed_on(&db, 7997).await.unwrap().expect("仍欠着");
        assert_eq!(again.task_id, "task-b", "任务与原因换成最近的这一次");
        assert_eq!(again.reason, "开工前对齐换了房间代号");
        assert_eq!(
            again.armed_at.as_deref(),
            Some(armed_at.as_str()),
            "第一次欠下的时刻不覆盖"
        );

        assert_eq!(owed_on(&db, 8000).await.unwrap(), None, "别的库不受影响");

        discharge_on(&db, 7997).await.unwrap();
        assert_eq!(owed_on(&db, 7997).await.unwrap(), None, "销了就不欠");
        discharge_on(&db, 7997).await.unwrap();
        assert_eq!(owed_on(&db, 7997).await.unwrap(), None, "重复销账是无操作");
    }

    /// 记账 / 销账两句的形状：记是 `UPSERT`（可反复），`armed_at ?? time::now()` 保第一次；
    /// 销是整行 `DELETE`；原因里的引号要转义。
    #[test]
    fn the_arming_statement_upserts_and_the_discharge_deletes_the_row() {
        let sql = render_arm(7997, "PANE 24383/7（属性变了）'x'", "task-1");
        assert!(
            sql.starts_with("UPSERT dbnum_room_recompute_owed:7997 SET"),
            "{sql}"
        );
        assert!(sql.contains("armed_at = armed_at ?? time::now()"), "{sql}");
        assert!(sql.contains("task_id = 'task-1'"), "{sql}");
        assert!(
            !sql.contains("'x'") && sql.contains("x"),
            "引号要转义: {sql}"
        );
        assert_eq!(
            render_discharge(7997),
            "DELETE dbnum_room_recompute_owed:7997;"
        );
    }

    /// 本窗触发与欠账合成一句：都没有 = 不跑；只有欠账也跑，并点名是哪个任务、什么时候欠下的。
    #[test]
    fn the_reason_merges_this_window_with_the_debt_and_names_the_debtor() {
        let owed = RoomRecomputeOwed {
            dbnum: 7997,
            reason: "本窗变更集碰到房间定义族".into(),
            task_id: "task-a".into(),
            armed_at: Some("2026-09-20T03:00:00Z".into()),
        };
        assert_eq!(reason_with_owed(None, None), None);
        assert_eq!(
            reason_with_owed(Some("开工前对齐换了房间代号"), None).as_deref(),
            Some("开工前对齐换了房间代号")
        );
        let only_debt = reason_with_owed(None, Some(&owed)).unwrap();
        assert!(
            only_debt.starts_with(
                "上一窗欠下的整库房间重算未还（任务 task-a，2026-09-20T03:00:00Z 记账："
            ) && only_debt.contains("房间定义族"),
            "{only_debt}"
        );
        let both = reason_with_owed(Some("开工前对齐换了房间代号"), Some(&owed)).unwrap();
        assert!(
            both.starts_with("开工前对齐换了房间代号；上一窗欠下的"),
            "本窗的在前、欠账在后: {both}"
        );
        let no_time = RoomRecomputeOwed {
            armed_at: None,
            ..owed
        };
        assert!(
            reason_with_owed(None, Some(&no_time))
                .unwrap()
                .starts_with("上一窗欠下的整库房间重算未还（任务 task-a：")
        );
    }

    /// 两条触发各自成句，都没有就是 `None`；代号没变的重建不触发。
    #[test]
    fn a_task_recomputes_rooms_on_either_trigger() {
        let item = room_only_work_item(7997, "DESI", 163);
        assert!(request_for_task(&[], None).is_none());
        assert!(
            request_for_task(
                &[],
                Some(&RoomReferenceRefresh::Rebuilt {
                    previous_key: Some("a".into()),
                    key: "b".into(),
                    generation_changed: false,
                    diff: None,
                    elapsed_ms: 1,
                }),
            )
            .is_none(),
            "文件前进但房间 / 面板没变：代号不变，不重算"
        );
        assert!(
            request_for_task(
                &[],
                Some(&RoomReferenceRefresh::Unchanged { key: "a".into() })
            )
            .is_none()
        );
        let planned = request_for_task(std::slice::from_ref(&item), None).unwrap();
        assert!(planned.contains("房间定义族"), "{planned}");
        let swapped = request_for_task(
            &[],
            Some(&RoomReferenceRefresh::Rebuilt {
                previous_key: None,
                key: "b".into(),
                generation_changed: true,
                diff: Some("removed=1 recalc=0".into()),
                elapsed_ms: 1,
            }),
        )
        .unwrap();
        assert!(
            swapped.contains("换了房间代号") && swapped.contains("removed=1"),
            "{swapped}"
        );
        let both = request_for_task(
            std::slice::from_ref(&item),
            Some(&RoomReferenceRefresh::Rebuilt {
                previous_key: None,
                key: "b".into(),
                generation_changed: true,
                diff: None,
                elapsed_ms: 1,
            }),
        )
        .unwrap();
        assert!(both.contains("；"), "两条触发合成一句: {both}");
    }
}
