//! 房间增量更新审核（2026-09-07，`docs/2026-09-07-room-increment-audit.md`）的**回退即红**源码钉。
//!
//! 每条钉住一条审核里定为 High 的缺口，落地时红、修复后绿：
//!
//! - **F-A**（已修，2026-09-07）默认规划器（direct + DESI → `window_root_plan`）没有房间结构触发：
//!   FRMW 改名、CWALL/CFLOOR/PANE 的 OWNER 变更排不出 `RoomRecalcPanel`（ADR-010 D12 / ADR-040 §4
//!   回退）。现在 `WindowRootSources::collect` 多一条 `room_structural_trigger` 臂，文件侧读两端。
//! - **F-B**（已修，2026-09-07）房间消费不区分数据面形态：读透（`spawned-mem` / `embedded-mem`）下没有文件轮次、
//!   `room_published_inst` 停在启动基线，房间任务却照算照销账，`e3d.room.lookup` 报 `current`。
//!   现在视图随 `options::data_face()`（`room_model::consumption_view`），读透回 `unsupported`。
//! - **F-C** 一个根 `Failed` 让文件轮次停在 `collecting`，空闲重试成功之后没有任何地方再调
//!   `publish_completed_file_round`。
//!
//! 只 `include_str!` 相关源文件、不连库、不碰全局状态，可以与任何测试并行。

#[cfg(test)]
mod tests {
    /// 从 `start` 之后取到函数在第 0 列的 `}` 为止（rustfmt 布局）。
    fn fn_body<'a>(source: &'a str, start: &str) -> &'a str {
        source
            .split_once(start)
            .unwrap_or_else(|| panic!("源码里必须还有 `{start}`"))
            .1
            .split_once("\n}\n")
            .unwrap_or_else(|| panic!("`{start}` 之后必须有第 0 列的 `}}`"))
            .0
    }

    /// F-A：默认窗口规划器必须带上 ADR-010 D12 的三条结构触发（FRMW/SBFR 改名命中关键字、
    /// PANE 的 OWNER 变更、CWALL/CFLOOR 的 OWNER 变更），而不是只在老 PE 规划器
    /// （`model_update_plan::build_model_update_plan`）里有。
    #[test]
    fn f_a_default_window_planner_carries_room_structural_triggers() {
        let source = include_str!("../data_interface/window_root_plan.rs");
        // 收集臂：与其它级联臂一样在 `WindowRootSources::collect` 里读，且不带开关。
        let collect = fn_body(source, "    pub fn collect(");
        assert!(
            collect.contains("RoomStructuralTriggers::collect("),
            "direct+DESI 是默认路径，`WindowRootSources::collect` 必须收集房间结构触发\
             （ADR-010 D12 三条规则，文件侧）：{collect}"
        );
        assert!(
            !collect.contains("model_room_structural"),
            "房间结构触发是恢复一条已接受的保证，不许挂开关: {collect}"
        );
        // 落法：面板目标折成 `RoomRecalcPanel`，且排在 `into_model_update_plan` 之后（工作项
        // 已经成形才并进去），枚举读挂要落 `room_rebuild_required`。
        let planner = fn_body(
            source,
            "pub(crate) async fn build_model_update_plan_from_window(",
        );
        let into_at = planner
            .find("into_model_update_plan(")
            .expect("窗口计划要先成形");
        let append_at = planner
            .find("append_room_structural_work(")
            .expect("房间结构工作项必须并进窗口计划");
        assert!(into_at < append_at, "{planner}");
        let append = fn_body(source, "pub(crate) fn append_room_structural_work(");
        assert!(
            append.contains("ModelWorkAction::RoomRecalcPanel")
                && append.contains("room_rebuild_required = true"),
            "{append}"
        );
    }

    /// F-A 的行为面：面板目标 → `RoomRecalcPanel` 工作项（去重、排序），
    /// 枚举读挂 → `room_rebuild_required` + 原因；空触发对计划零改动。
    #[test]
    fn f_a_room_structural_work_lands_as_panel_recalcs_and_rebuild_requirements() {
        use crate::data_interface::model_update_plan::{
            ModelUpdatePlan, ModelWorkAction, ModelWorkItem,
        };
        use crate::data_interface::room_structural_trigger::RoomStructuralTriggers;
        use crate::data_interface::window_root_plan::append_room_structural_work;
        use aios_core::RefnoEnum;

        let panel_a = RefnoEnum::from("24384/3");
        let panel_b = RefnoEnum::from("24384/7");
        let mut triggers = RoomStructuralTriggers::default();
        triggers.panel_targets.extend([panel_b, panel_a]);

        let mut plan = ModelUpdatePlan::default();
        // 同一块面板已经因几何触发在计划里：并进去后只留一行。
        plan.work_items.push(ModelWorkItem {
            dbnum: 8000,
            db_type: "DESI".into(),
            source_end_sesno: 42,
            action: ModelWorkAction::RoomRecalcPanel,
            target_refno: panel_a.to_pdms_str(),
            noun: "PANE".into(),
        });
        append_room_structural_work(&mut plan, &triggers, 8000, "DESI", 42);
        let panels: Vec<&str> = plan
            .work_items
            .iter()
            .filter(|item| item.action == ModelWorkAction::RoomRecalcPanel)
            .map(|item| item.target_refno.as_str())
            .collect();
        assert_eq!(panels, vec!["24384/3", "24384/7"], "{:?}", plan.work_items);
        assert!(!plan.room_rebuild_required);
        assert!(
            plan.work_items
                .iter()
                .all(|item| item.dbnum == 8000 && item.source_end_sesno == 42)
        );

        let mut degraded = RoomStructuralTriggers::default();
        degraded
            .unreadable
            .push("房间改名的面板枚举失败: 夹具".into());
        let mut plan = ModelUpdatePlan::default();
        append_room_structural_work(&mut plan, &degraded, 8000, "DESI", 42);
        assert!(plan.work_items.is_empty());
        assert!(plan.room_rebuild_required);
        assert!(
            plan.room_rebuild_reason
                .as_deref()
                .unwrap_or_default()
                .contains("面板枚举失败")
        );

        let mut untouched = ModelUpdatePlan::default();
        append_room_structural_work(
            &mut untouched,
            &RoomStructuralTriggers::default(),
            8000,
            "DESI",
            42,
        );
        assert_eq!(untouched, ModelUpdatePlan::default());
    }

    /// F-B：房间消费的几何视图必须按数据面形态选——读透形态没有文件轮次，
    /// `room_published_inst` 永远停在启动基线，此时要么走 Live 视图、要么不算；
    /// 与之配套，`e3d.room.lookup` 在读透形态下必须回 `publication_status = unsupported`
    /// （ADR-062「读透不承诺文件级发布语义」），不能拿「无 collecting / 无 pending」凑成 `current`。
    #[test]
    fn f_b_room_consumption_and_lookup_status_follow_the_data_face() {
        let pending = include_str!("../data_interface/model_update_pending.rs");
        let drain = fn_body(pending, "async fn drain_rooms_selected(");
        assert!(
            drain.contains("data_face") || drain.contains("is_read_through"),
            "drain_rooms_selected 无条件走 Published 视图（`recalc_*_guarded` / `load_panel_index_published`），\
             没有按 `options::data_face()` 分流：{drain}"
        );

        let query = include_str!("../query_service.rs");
        let status = fn_body(query, "async fn published_derivation_status(");
        assert!(
            status.contains("unsupported"),
            "published_derivation_status 只会回 current/updating，读透形态下应回 unsupported：{status}"
        );
        // 读透的判定必须排在空间门与库查询之前：读透下没有可比的发布版本，问了也是白问。
        let unsupported_at = status.find("unsupported").unwrap();
        let gate_at = status
            .find("ensure_spatial_ready()")
            .expect("ingest 形态仍要过空间门");
        assert!(unsupported_at < gate_at, "读透分支必须先于空间门: {status}");
    }

    /// F-B 的行为面：视图选择是数据面的纯函数——ingest 读已发布房间几何，读透退回实时行。
    #[test]
    fn f_b_consumption_view_is_a_pure_function_of_the_data_face() {
        use crate::fast_model::room_model::{RoomGeometryView, consumption_view_for};
        use crate::options::DataFace;
        assert_eq!(
            consumption_view_for(DataFace::Ingest),
            RoomGeometryView::Published
        );
        assert_eq!(
            consumption_view_for(DataFace::ReadThrough),
            RoomGeometryView::Live
        );
    }

    /// F-C：空闲轮消化完模型积压之后，必须回头看「有没有 collecting 的文件轮次已经不再有
    /// 失败单元」，有就重新走 `publish_completed_file_round`——否则一个根失败一次，
    /// 该库的房间目标永远释放不了，`has_collecting_file_rounds()` 恒真，
    /// 所有库的 `e3d.room.lookup` 恒为 `updating`。
    #[test]
    fn f_c_idle_round_republishes_file_rounds_settled_by_retries() {
        let worker = include_str!("../data_interface/batch_worker.rs");
        let idle = worker
            .split_once("async fn idle_round(")
            .expect("idle_round 必须存在")
            .1
            .split_once("\nenum IdleOutcome")
            .expect("idle_round 之后是 IdleOutcome")
            .0;
        assert!(
            idle.contains("publish_completed_file_round(")
                || idle.contains("republish_settled_file_rounds"),
            "空闲轮里没有任何对 collecting 文件轮次的重新发布；今天 `publish_completed_file_round` \
             只在 execute_frozen_batch 与发布队列 runtime 各被调一次：{idle}"
        );
    }
}
