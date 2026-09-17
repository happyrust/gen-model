//! 面板自身的变化 → 本库整库房间归属重算（`RoomOnly`，d-819 / ADR-073 D13）。
//!
//! 逐根路径（ADR-073 D1）只算「本根在哪些面板里」。面板那一侧变了——PANE 的几何 / 属主、
//! FRMW / SBFR 改名（房间号、进出 `room_key_word`）、`project_hd` 下 CWALL / CFLOOR 搬迁——
//! 它名下**别的根**的成员边与 `room_num` 一个都不会被碰到：这是逐根路径的天然盲区
//! （ADR-073 §未决 U1），原来填它的结构触发账本（ADR-010 D12 / ADR-040 D4）已随房间流程退役。
//!
//! 现行填法（d-819）：三条触发，同一种处置。
//!
//! 1. **规划期**：窗口规划器看本窗变更集（e3d-model 的 `ChangeLedger`），碰到房间定义族的
//!    元素（[`ROOM_DEFINITION_NOUNS`]，增 / 删 / 改属主 / 改成员 / 改属性都算）→ 给本库排一条
//!    `ModelWorkAction::RoomOnly` 工作项（[`room_only_work_item`]）。**不解析 `detail` 散文**
//!    ——DESC 之类只改元数据的面板改动会多触发一次整库重算，接受：那是 O(本库构件) 的内存判定
//!    加一批边的重写，而漏掉一次就是静默陈旧。
//! 2. **执行期**：任务开工前对齐（ADR-073 D12）换了房间代号——别的库的面板 / 房间变了、进了
//!    常驻——本库的成员边同样可能陈旧，一并重算（跨库那一半只覆盖「变了之后第一个开工的库」，
//!    §未决 U3 仍开着）。
//! 3. **电平型再武装**（2026-09-17）：库上 durable 侧的整库对齐代号（`dbnum_watermark.model_room_generation`，
//!    ADR-073 D14）与此刻常驻代号不等 = 这个库欠一次整库重算（[`library_alignment_owed`]）。前两条是
//!    边沿型——原因只活在本任务里，拆窗 / 单元失败 / 刷盘没齐 / 重算写库失败都会把它丢掉，而下一窗的
//!    账本不再含那条 PANE 变更、再探又是 `Unchanged`，边就永久陈旧到下一次不相关的 PANE 变化。这一条
//!    按库上的事实判：直到某个任务把 `Aligned` 盖上（整库重算齐了、初始化、整库重排），每个任务开工
//!    都会再点一次。整库重算对不在投影里的根回读库上几何（下段），落盘形态下也盖得上，所以这一条在每个库上
//!    只多跑一次就安静；没有别的去重。
//!
//! 处置：任务在本批根**刷盘之后、收尾之前**（`batch_worker::execute_frozen_batch`），冻一台计算器，先枚举
//! 本库 `DbSet@T` 的根清单，对**投影里**有回执的根按投影里的几何、对清单里**不在投影里**的根按库上读回的
//! 几何（`model_record_reader::durable_records_of_root`，U1 第二半）重算归属、重写两张边
//! （`model_db_adapter::recompute_dbnum_room_edges`），不重生几何、不动模型行、不动空间树。
//! 算不出的根按 D2 记失败、`model_sesno` 不盖。**覆盖面要自证**：投影只装本进程生成过的根——
//! 初始化的模型行不进投影（ADR-077 D5），落盘形态冷重启后投影是空的——回读库补的正是这一半；重算完
//! 拿回执里覆盖到的根对一遍清单，每一根都在才说「整库」、durable 侧那一格「整库对齐到这一代」才盖；
//! 清单枚举不出来（覆盖面未知）只算了投影那一半、那一格**不盖**（`batch_worker::RoomRecomputeVerdict::PartialCoverage`），
//! 下一个任务由第三条触发再来。它是任务内部的一步，不入队、没有 HTTP 口（ADR-073 §不做「不给房间开新的
//! HTTP 触发口」）。

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

/// 第三条触发（电平型，2026-09-17）的纯函数形态：库上 durable 侧的整库对齐代号
/// （`dbnum_watermark.model_room_generation`，ADR-073 D14）与此刻常驻代号不等 = 这个库欠一次整库重算。
///
/// `resident` 为 `None`（开关关着 / 常驻还在装 / `Blocked`）= 没有可对齐的目标，不触发——那种局面下
/// 计算器也冻不出，重算无从谈起。`library` 为 `None`（旧行、从没整库对齐过、关着开关生成的）当然不等：
/// 库上没有这句承诺，就欠一次；重算齐了盖上之后这一条就安静了，直到房间 / 面板真变过（代号变）。
/// 没有进程内的去重：整库重算对不在投影里的根回读库上几何（U1 第二半），落盘形态下也盖得上那一格，
/// 「同一代下反复点、每次都覆盖不全」这种局面不再有（此前为它设的 `PARTIAL_COVERAGE_SETTLED` 去重表
/// 2026-09-17 随之退役）。
pub(crate) fn library_alignment_owed(
    library: Option<&str>,
    resident: Option<&str>,
) -> Option<String> {
    let resident = resident?;
    if library == Some(resident) {
        return None;
    }
    let short = |generation: &str| generation.chars().take(12).collect::<String>();
    Some(match library {
        Some(library) => format!(
            "库上整库房间对齐代号 {} ≠ 此刻常驻代号 {}（欠一次整库重算，D14）",
            short(library),
            short(resident)
        ),
        None => format!(
            "库上没有整库房间对齐代号（从没整库对齐过 / 关着开关生成的），此刻常驻代号 {}（欠一次整库重算，D14）",
            short(resident)
        ),
    })
}

/// 第三条触发的取数：库上那一格与两枚水位同一行（`DbnumState::read`），常驻代号取
/// `query_service::resident_room_generation`（`Ready` / `Degraded` 才有）。开关关着直接 `None`；
/// 水位行读不出**不**触发（读错不是「欠着」的证据，只留一句 warn）。任务开工前对齐（D12）之后调，
/// 拿到的才是此刻这一代。
pub(crate) async fn durable_alignment_owed(dbnum: u32) -> Option<String> {
    if !crate::options::room_membership() {
        return None;
    }
    let resident = crate::query_service::resident_room_generation()?;
    let library = match crate::data_interface::dbnum_state::DbnumState::read(dbnum).await {
        Ok(state) => state.and_then(|state| state.model_room_generation),
        Err(error) => {
            log::warn!(
                "dbnum={dbnum} 读不出水位行上的整库房间对齐代号，这一轮不按「欠整库重算」处理: {error:#}"
            );
            return None;
        }
    };
    library_alignment_owed(library.as_deref(), Some(resident.as_str()))
}

/// 这个任务要不要跑整库房间重算，以及为什么。三条触发合成一句：规划器排的 `RoomOnly` 工作项
/// （本窗碰了房间定义族）、开工前对齐换了房间代号、库上整库对齐代号欠着（[`library_alignment_owed`]）。
/// 都没有 = `None`。
pub(crate) fn request_for_task(
    side_actions: &[ModelWorkItem],
    refresh: Option<&RoomReferenceRefresh>,
    alignment_owed: Option<&str>,
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
    if let Some(owed) = alignment_owed {
        reasons.push(owed.to_string());
    }
    (!reasons.is_empty()).then(|| reasons.join("；"))
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
        // 第三条触发（电平型）：两个执行体都在开工前对齐**之后**读库上那一格、喂给 `request_for_task`。
        // 顺序要紧——对齐之前读到的常驻代号是上一代，比对会白点一次（或漏点一次）。
        for (label, body) in [
            ("批次执行体", executor),
            (
                "ModelOnly 执行体",
                function_body(worker, "async fn execute_model_only_body("),
            ),
        ] {
            let refresh = body
                .find("refresh_room_reference_before_task(job.dbnum)")
                .unwrap_or_else(|| panic!("{label}: 开工前对齐"));
            let owed = body
                .find("room_recompute::durable_alignment_owed(job.dbnum)")
                .unwrap_or_else(|| panic!("{label}: 要读库上的整库对齐代号（第三条触发）: {body}"));
            let request = body
                .find("room_recompute::request_for_task(")
                .unwrap_or_else(|| panic!("{label}: request_for_task"));
            assert!(
                refresh < owed && owed < request,
                "{label}: 对齐 → 读库上那一格 → 合成三条触发，顺序不能乱"
            );
            assert!(
                body.contains("alignment_owed.as_deref()"),
                "{label}: 第三条触发要真的传进 request_for_task: {body}"
            );
        }
        // ModelOnly 执行体同样：启动追平窗口与稳态窗口共用同一条规划入口，`RoomOnly` 工作项随
        // `side_actions` 进来，这里此前硬编码 `room_recompute: None`，停机期间的 PANE 改动被静默吃掉。
        let model_only = function_body(worker, "async fn execute_model_only_body(");
        assert!(
            model_only.contains("room_recompute::request_for_task(")
                && model_only.contains("Some(&room_reference)"),
            "ModelOnly 执行体也要把「要不要整库重算」交出去（三条触发都带上）: {model_only}"
        );
        // 规划失败的早退（模型半边没跑）仍是 `None`；跑完的那一份回执必须接住上面那句的结果。
        assert_eq!(
            model_only.matches("room_recompute: None").count(),
            1,
            "只有规划失败的早退才硬编码不重算: {model_only}"
        );
        assert!(
            model_only.contains("whole_library_model_only(job)"),
            "整库重排（起点 0）每一根都刚算过，不再空转一遍: {model_only}"
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
        let rooms = tail
            .find("run_room_only_recompute(job, &reason, &mut result, file_latest_sesno)")
            .expect("整库房间重算在收尾段，带着 T 好对 DbSet@T 的根清单自证覆盖面");
        let outcome = tail.find("model_outcome_of(&flush").expect("模型收尾判决");
        assert!(
            flush < rooms && rooms < outcome,
            "刷盘之后、模型收尾判决之前: {tail}"
        );
        assert!(
            tail.contains("Some(reason) if flush.all_acked()"),
            "刷盘没齐就不跑（那批根已判死）: {tail}"
        );
        assert!(
            tail.contains("outcome.all_roots_ok = false"),
            "算不齐扣 all_roots_ok、水位不盖: {tail}"
        );
        // 覆盖面不全（投影里没有本库全部根）只算「触到的根更新了」：durable 侧「整库对齐到这一代」那一格
        // 不许盖——盖了就是替没算过的根说话，正是 D14 要挡的「拿上一代的边冒充这一版」。
        assert!(
            tail.contains("RoomRecomputeVerdict::library_generation")
                && tail.contains("RoomRecomputeVerdict::rooms_ok"),
            "收尾要分开问两句：算齐了没有（水位）、覆盖整库了没有、对齐到哪一代（对齐代号）: {tail}"
        );

        // 旧过滤（「房间重算项不在内」）不得把 RoomOnly 拦在 side_actions 之外。
        let manual = include_str!("manual_update.rs");
        assert!(
            manual.contains("&& !item.action.is_room_recalc()"),
            "side_actions 的过滤只拦 RoomRecalc*，RoomOnly 经它交给任务"
        );
    }

    /// 三条触发各自成句，都没有就是 `None`；代号没变的重建不触发。
    #[test]
    fn a_task_recomputes_rooms_on_either_trigger() {
        let item = room_only_work_item(7997, "DESI", 163);
        assert!(request_for_task(&[], None, None).is_none());
        assert!(
            request_for_task(
                &[],
                Some(&RoomReferenceRefresh::Rebuilt {
                    previous_key: Some("a".into()),
                    key: "b".into(),
                    generation_changed: false,
                    previous_generation: Some("g".into()),
                    diff: None,
                    panels: None,
                    elapsed_ms: 1,
                }),
                None,
            )
            .is_none(),
            "文件前进但房间 / 面板没变：代号不变，不重算"
        );
        assert!(
            request_for_task(
                &[],
                Some(&RoomReferenceRefresh::Unchanged { key: "a".into() }),
                None,
            )
            .is_none()
        );
        let planned = request_for_task(std::slice::from_ref(&item), None, None).unwrap();
        assert!(planned.contains("房间定义族"), "{planned}");
        let swapped = request_for_task(
            &[],
            Some(&RoomReferenceRefresh::Rebuilt {
                previous_key: None,
                key: "b".into(),
                generation_changed: true,
                previous_generation: None,
                diff: Some("removed=1 recalc=0".into()),
                panels: None,
                elapsed_ms: 1,
            }),
            None,
        )
        .unwrap();
        assert!(
            swapped.contains("换了房间代号") && swapped.contains("removed=1"),
            "{swapped}"
        );
        // 第三条：库上欠着，原因原样带进来；单独一条也触发。
        let owed = request_for_task(
            &[],
            None,
            Some("库上整库房间对齐代号 old ≠ 此刻常驻代号 new"),
        )
        .unwrap();
        assert!(owed.contains("old ≠ 此刻常驻代号 new"), "{owed}");
        let all = request_for_task(
            std::slice::from_ref(&item),
            Some(&RoomReferenceRefresh::Rebuilt {
                previous_key: None,
                key: "b".into(),
                generation_changed: true,
                previous_generation: None,
                diff: None,
                panels: None,
                elapsed_ms: 1,
            }),
            Some("欠一次整库重算"),
        )
        .unwrap();
        assert_eq!(all.matches("；").count(), 2, "三条触发合成一句: {all}");
    }

    /// 第三条触发的真值表（电平型，2026-09-17）：库上那一格与此刻常驻代号不等就欠——包括库上没有那一格；
    /// 相等不欠；常驻说不出代号（开关关 / 还在装 / `Blocked`）没有可对齐的目标，不欠。
    #[test]
    fn the_library_owes_a_recompute_until_its_durable_alignment_matches_the_resident() {
        assert!(library_alignment_owed(Some("efd4ebe1"), Some("efd4ebe1")).is_none());
        let stale =
            library_alignment_owed(Some("00aa11bb22cc33dd"), Some("efd4ebe1ffff0000")).unwrap();
        assert!(
            stale.contains("00aa11bb22cc")
                && stale.contains("efd4ebe1ffff")
                && stale.contains("欠一次整库重算"),
            "{stale}"
        );
        let never = library_alignment_owed(None, Some("efd4ebe1")).unwrap();
        assert!(
            never.contains("没有整库房间对齐代号") && never.contains("efd4ebe1"),
            "{never}"
        );
        assert!(
            library_alignment_owed(Some("efd4ebe1"), None).is_none()
                && library_alignment_owed(None, None).is_none(),
            "常驻说不出代号 = 没有可对齐的目标"
        );
    }

    /// 源码钉（ADR-073 D13 / U1 第二半，2026-09-17）：整库重算对不在投影里的根**回读库上几何**——收尾段先枚举
    /// `DbSet@T` 的根清单、再把它交给 `recompute_dbnum_room_edges` 靶着回读；`recompute_dbnum_room_edges` 对点名
    /// 的根经 `durable_records_of_root` 读库、经 `durable_room_inputs` 折成同一形状、走**同一个**
    /// `calculate_for_root` / `render_root_room_edges`；投影里有回执的根不回读。此前为「同一代下反复点、每次都
    /// 覆盖不全」设的去重表（`PARTIAL_COVERAGE_SETTLED` / `settle_partial_coverage` / `rearm_library`）随之退役，
    /// 第三条触发只看库上的事实。
    #[test]
    fn the_library_recompute_reads_back_the_durable_geometry_of_roots_outside_the_projection() {
        let recompute = function_body(
            include_str!("batch_worker.rs"),
            "async fn run_room_only_recompute(",
        );
        let enumerate = recompute
            .find("library_generation_roots(dbnum, target_sesno)")
            .expect("先枚举 DbSet@T 的根清单");
        let call = recompute
            .find("recompute_dbnum_room_edges(")
            .expect("整库重算");
        assert!(
            enumerate < call && recompute.contains("expected.as_deref(),"),
            "根清单在重算之前取、并作为靶子交给 recompute_dbnum_room_edges: {recompute}"
        );
        for retired in ["settle_partial_coverage(", "rearm_library("] {
            assert!(
                !recompute.contains(retired),
                "去重表已退役，收尾段不该再有 {retired}: {recompute}"
            );
        }
        // 只看本模块的生产段——这段测试自己的字面量不算。
        let production = include_str!("room_recompute.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test module")
            .0;
        assert!(
            !production.contains("static PARTIAL_COVERAGE_SETTLED")
                && !production.contains("fn settle_partial_coverage(")
                && !production.contains("fn rearm_library("),
            "去重表已退役，本模块不该再定义它"
        );

        let adapter = include_str!("../fast_model/model_db_adapter.rs");
        let body = adapter
            .split_once("pub async fn recompute_dbnum_room_edges(")
            .expect("recompute_dbnum_room_edges")
            .1;
        let body = &body[..body
            .find("\n}\r\n")
            .or_else(|| body.find("\n}\n"))
            .expect("end")];
        assert!(
            body.contains("library_roots: Option<&[RefNo]>"),
            "点名的根清单进签名: {body}"
        );
        assert!(
            body.contains("!projected_set.contains(&packed_refno(*root))"),
            "投影里有回执的根不回读: {body}"
        );
        let read = body
            .find("model_record_reader::durable_records_of_root(")
            .expect("回读库上几何");
        let fold = body
            .find("room_model::durable_room_inputs(&rows)")
            .expect("折成房间判定的输入");
        assert!(read < fold, "先读库、再折形状: {body}");
        assert_eq!(
            body.matches("rooms.calculate_for_root(").count(),
            1,
            "两种来源走同一个判定入口（共用的那一段）: {body}"
        );
        assert_eq!(
            body.matches("rooms.render_root_room_edges(").count(),
            1,
            "两种来源走同一套渲染: {body}"
        );
    }
}
