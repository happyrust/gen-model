//! e3d 发布事务里的房间派生面副作用（ADR-010 §4 / ADR-040 §3 / ADR-057 D2）。
//!
//! 旧生成器的房间触发住在 AABB 刷新里（`aabb_refresh.rs` → `render_room_recalc_upserts`），
//! e3d-model 接管生成之后那条链不再被走到：`E3dModelService` 直写 `aabb` 行、bump spatial
//! epoch、同步空间树，但从不排 `RoomRecalc*`，也不清被移除几何的 `room_relate` /
//! `room_panel_relate` 边（`helper.rs` 写着：少清一个方向，`fn::room_relate_of` 照样把悬空边
//! 取出来）。本模块把两件事折回**同一个发布事务**（ADR-040 §3 的同事务纪律）：
//!
//! - **重算**：本次发布 upsert 的每个 `GeometryId::Element` 来源元素按 noun 分流成
//!   `RoomRecalcPanel`（PANE）/ `RoomRecalcElement`——ADR-040 §1 的保守口径：定向生成的
//!   目标就是这一窗真正重写的元素，AABB 变没变都排（判定消费的是网格，不只是盒子）。
//!   隐式管身（`ImpliedTube`）不排：房间系统今天不读 `tubi_relate`，对容器排元素任务只会
//!   让 `recalc_element_membership` 查不到实例而把容器的存量入边清成空集。
//! - **清边**：被移除的 `Element` 几何、以及被 pre-e3d 清理删掉行却没有拿到新几何的旧来源，
//!   两个方向的房间边一并删（作为成员的入边；作为面板的出边与 `room_panel_relate`）。
//!   清边**不看** `room_incremental` 开关——它不是增量重算，是删除路径的当场清理（ADR-010 §4
//!   的删除例外），与 `delete_inst_relate_subtree` 同一口径。
//!
//! 全库生成（`generate_dbnum`）只清边不排重算（ADR-010 §4 收窄 1：全量以启动全量房间重建收尾，
//! 逐元素排任务等于给整库每个元素排一次重算）。

use std::collections::{BTreeMap, BTreeSet};

use aios_core::RefnoEnum;
use anyhow::Context;
use e3d_io::refno::RefNo;
use e3d_model::elmodl::GeometryId;

use crate::data_interface::generation_root::refno_from_e3d;
use crate::fast_model::aabb_refresh::AabbChange;

/// 这次发布是定向生成（窗口 `RegenRoot` / 按需 `ensure`）还是全库生成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoomEffectPolicy {
    /// 排房间重算 + 清边。
    Directed,
    /// 只清边；重算交给启动全量房间重建（ADR-010 §4 收窄 1）。
    FullDatabase,
}

/// 一次发布对房间派生面的两笔影响。两个名单都按 refno 排序去重，且互不相交：
/// 拿到新几何的来源只重算，不清边（重算自己先清后写）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RoomPublicationEffects {
    /// 要排 `RoomRecalcPanel` / `RoomRecalcElement` 的来源元素（按 noun 分流）。
    pub recalc: Vec<AabbChange>,
    /// 要清两个方向房间边的来源元素。
    pub cleared: Vec<RefnoEnum>,
}

impl RoomPublicationEffects {
    pub fn is_empty(&self) -> bool {
        self.recalc.is_empty() && self.cleared.is_empty()
    }
}

/// 一条 `DELETE … <->room_relate` 最多点名多少个来源（与 `delete_inst_relate_subtree` 的
/// 分块同一量级）。
const ROOM_EDGE_DELETE_CHUNK: usize = 300;

/// 从一次发布的 upsert / removal 折出房间副作用。
///
/// `upserts` 每项是 `(几何身份, 来源 refno, noun)`；`legacy_sources` 是 pre-e3d 清理将删掉
/// 行的旧来源（`pre_e3d_spatial_refnos`），其中没拿到新几何的那些视同被移除。
pub(crate) fn room_publication_effects<'a>(
    upserts: impl IntoIterator<Item = (&'a GeometryId, RefNo, &'a str)>,
    removals: &[GeometryId],
    legacy_sources: &BTreeSet<RefnoEnum>,
) -> anyhow::Result<RoomPublicationEffects> {
    let mut recalc: BTreeMap<RefnoEnum, String> = BTreeMap::new();
    for (geometry_id, refno, noun) in upserts {
        if matches!(geometry_id, GeometryId::Element { .. }) {
            recalc
                .entry(refno_from_e3d(refno))
                .or_insert_with(|| noun.trim().to_ascii_uppercase());
        }
    }
    let mut cleared: BTreeSet<RefnoEnum> = legacy_sources.clone();
    for geometry_id in removals {
        if let GeometryId::Element { refno } = geometry_id {
            let refno = crate::fast_model::e3d_model_service::parse_refno(refno)
                .with_context(|| format!("removed geometry {geometry_id}"))?;
            cleared.insert(refno_from_e3d(refno));
        }
    }
    cleared.retain(|refno| !recalc.contains_key(refno));
    Ok(RoomPublicationEffects {
        recalc: recalc
            .into_iter()
            .map(|(refno, noun)| AabbChange { refno, noun })
            .collect(),
        cleared: cleared.into_iter().collect(),
    })
}

/// 渲染成发布事务里的语句：先清边，再排重算。空影响渲染成空串。
pub(crate) fn render_room_publication_effects(
    effects: &RoomPublicationEffects,
    policy: RoomEffectPolicy,
    room_incremental: bool,
) -> String {
    let mut out = String::new();
    for chunk in effects.cleared.chunks(ROOM_EDGE_DELETE_CHUNK) {
        let keys: Vec<String> = chunk.iter().map(RefnoEnum::to_pe_key).collect();
        out.push_str(&crate::data_interface::helper::render_room_membership_delete(&keys));
        out.push('\n');
    }
    if policy == RoomEffectPolicy::Directed && room_incremental && !effects.recalc.is_empty() {
        out.push_str(
            &crate::data_interface::model_update_pending::render_room_recalc_upserts(
                &effects.recalc,
            ),
        );
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(id: &str) -> GeometryId {
        GeometryId::Element { refno: id.into() }
    }

    fn tube(container: &str) -> GeometryId {
        GeometryId::ImpliedTube {
            container_refno: container.into(),
            route_ordinal: 0,
            from_refno: "24384/1".into(),
            to_refno: "24384/2".into(),
        }
    }

    fn r(id: u32) -> RefnoEnum {
        refno_from_e3d(RefNo::new(24384, id))
    }

    fn effects_of(
        upserts: &[(GeometryId, u32, &str)],
        removals: &[GeometryId],
        legacy: &[u32],
    ) -> RoomPublicationEffects {
        room_publication_effects(
            upserts
                .iter()
                .map(|(id, refno, noun)| (id, RefNo::new(24384, *refno), *noun)),
            removals,
            &legacy.iter().map(|id| r(*id)).collect(),
        )
        .unwrap()
    }

    /// upsert 的 Element 来源按 noun 分流；隐式管身不排；同一 refno 只出一条。
    #[test]
    fn element_upserts_become_room_targets_by_noun_and_tubes_are_skipped() {
        let effects = effects_of(
            &[
                (element("24384/7"), 7, "PANE"),
                (element("24384/5"), 5, "EQUI"),
                (element("24384/5"), 5, "EQUI"),
                (tube("24384/30"), 30, "TUBI"),
            ],
            &[],
            &[],
        );
        assert_eq!(
            effects.recalc,
            vec![
                AabbChange {
                    refno: r(5),
                    noun: "EQUI".into(),
                },
                AabbChange {
                    refno: r(7),
                    noun: "PANE".into(),
                },
            ]
        );
        assert!(effects.cleared.is_empty());
    }

    /// 被移除的 Element 与「旧行被清掉却没拿到新几何」的旧来源都清边；拿到新几何的旧来源
    /// 只重算；管身移除不清容器的边。
    #[test]
    fn removed_and_orphaned_legacy_sources_are_cleared_but_regenerated_ones_are_not() {
        let effects = effects_of(
            &[(element("24384/5"), 5, "EQUI")],
            &[element("24384/7"), element("24384/7"), tube("24384/30")],
            &[5, 6],
        );
        assert_eq!(effects.recalc.len(), 1);
        assert_eq!(effects.cleared, vec![r(6), r(7)]);
    }

    /// 清边不看开关也不看生成种类；重算只在定向生成且开关打开时渲染；空影响是空串。
    #[test]
    fn rendering_always_clears_edges_and_gates_recalc_on_policy_and_switch() {
        let effects = effects_of(
            &[
                (element("24384/7"), 7, "PANE"),
                (element("24384/5"), 5, "EQUI"),
            ],
            &[element("24384/9")],
            &[],
        );
        let directed = render_room_publication_effects(&effects, RoomEffectPolicy::Directed, true);
        for needle in [
            "DELETE pe:24384_9<->room_relate;",
            "DELETE pe:24384_9<->room_panel_relate;",
            "model_update_pending:room_recalc_panel_24384_7",
            "model_update_pending:room_recalc_element_24384_5",
            "action = 'room_recalc_panel'",
            "action = 'room_recalc_element'",
        ] {
            assert!(
                directed.contains(needle),
                "missing `{needle}` in {directed}"
            );
        }
        assert!(
            directed.find("<->room_relate").unwrap() < directed.find("room_recalc_").unwrap(),
            "清边排在重算之前: {directed}"
        );

        let switched_off =
            render_room_publication_effects(&effects, RoomEffectPolicy::Directed, false);
        assert!(switched_off.contains("DELETE pe:24384_9<->room_relate;"));
        assert!(!switched_off.contains("room_recalc_"), "{switched_off}");

        let full = render_room_publication_effects(&effects, RoomEffectPolicy::FullDatabase, true);
        assert!(full.contains("DELETE pe:24384_9<->room_relate;"));
        assert!(!full.contains("room_recalc_"), "{full}");

        assert_eq!(
            render_room_publication_effects(
                &RoomPublicationEffects::default(),
                RoomEffectPolicy::Directed,
                true
            ),
            ""
        );
    }

    /// 清边按 300 个来源一条语句分块。
    #[test]
    fn edge_deletes_are_chunked() {
        let removals: Vec<GeometryId> = (1..=301).map(|i| element(&format!("24384/{i}"))).collect();
        let effects = effects_of(&[], &removals, &[]);
        let sql = render_room_publication_effects(&effects, RoomEffectPolicy::FullDatabase, false);
        assert_eq!(sql.matches("<->room_relate;").count(), 2, "{sql}");
        assert_eq!(sql.matches("<->room_panel_relate;").count(), 2, "{sql}");
    }

    /// 真引擎门（`mem://`）：一块 PANE 被 e3d 发布移除后，它作为面板的出边（成员）、作为
    /// 面板的 `room_panel_relate` 入边、以及被移除成员的入边一个都不剩；没被碰的面板与
    /// 成员原样；重写的成员排进了 `room_recalc_element` 行。库里**没有** pe 记录的来源
    /// （零解析库）清边同样不报错。
    #[tokio::test]
    async fn deleting_a_pane_leaves_no_dangling_room_edges() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_publication")
            .use_db("dangling_edges")
            .await
            .unwrap();
        // 房间 R301 有面板 P(7) 与 Q(8)：P 收着 5、6，Q 收着 9、10。
        db.query(
            "CREATE pe:24384_1 SET noun = 'FRMW', name = '/1RX-RM03-R301';\
             CREATE pe:24384_7 SET noun = 'PANE'; CREATE pe:24384_8 SET noun = 'PANE';\
             CREATE pe:24384_5 SET noun = 'EQUI'; CREATE pe:24384_6 SET noun = 'BOX';\
             CREATE pe:24384_9 SET noun = 'EQUI'; CREATE pe:24384_10 SET noun = 'BOX';\
             RELATE pe:24384_1->room_panel_relate->pe:24384_7 SET room_num = 'R301';\
             RELATE pe:24384_1->room_panel_relate->pe:24384_8 SET room_num = 'R301';\
             RELATE pe:24384_7->room_relate->pe:24384_5 SET room_num = 'R301';\
             RELATE pe:24384_7->room_relate->pe:24384_6 SET room_num = 'R301';\
             RELATE pe:24384_8->room_relate->pe:24384_9 SET room_num = 'R301';\
             RELATE pe:24384_8->room_relate->pe:24384_10 SET room_num = 'R301';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        // 发布：面板 P 与成员 10 被移除；成员 9 重写；99 在库里没有 pe 记录。
        let effects = effects_of(
            &[(element("24384/9"), 9, "EQUI")],
            &[element("24384/7"), element("24384/10"), element("24384/99")],
            &[],
        );
        let sql = render_room_publication_effects(&effects, RoomEffectPolicy::Directed, true);
        db.query(format!("BEGIN TRANSACTION;\n{sql}COMMIT TRANSACTION;"))
            .await
            .expect("publication transport")
            .check()
            .expect("publication statements");

        async fn edges(
            db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
            table: &str,
            key: &str,
        ) -> usize {
            let mut response = db
                .query(format!(
                    "SELECT VALUE id FROM {table} WHERE in = {key} OR out = {key};"
                ))
                .await
                .unwrap()
                .check()
                .unwrap();
            response
                .take::<Vec<surrealdb::sql::Thing>>(0)
                .unwrap()
                .len()
        }
        assert_eq!(
            edges(&db, "room_relate", "pe:24384_7").await,
            0,
            "P 的成员边"
        );
        assert_eq!(
            edges(&db, "room_panel_relate", "pe:24384_7").await,
            0,
            "P 的房间边"
        );
        assert_eq!(
            edges(&db, "room_relate", "pe:24384_5").await,
            0,
            "P 的成员 5 的入边"
        );
        assert_eq!(
            edges(&db, "room_relate", "pe:24384_10").await,
            0,
            "被移除成员 10 的入边"
        );
        assert_eq!(
            edges(&db, "room_relate", "pe:24384_8").await,
            1,
            "Q 只剩 9 那条"
        );
        assert_eq!(
            edges(&db, "room_panel_relate", "pe:24384_8").await,
            1,
            "Q 的房间边原样"
        );
        assert_eq!(
            edges(&db, "room_relate", "pe:24384_9").await,
            1,
            "重写的成员 9 的边原样"
        );

        let mut response = db
            .query(
                "SELECT VALUE action FROM model_update_pending:room_recalc_element_24384_9;\
                 SELECT VALUE target_refno FROM model_update_pending:room_recalc_element_24384_9;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        let action: Option<String> = response.take(0).unwrap();
        let target: Option<String> = response.take(1).unwrap();
        assert_eq!(action.as_deref(), Some("room_recalc_element"));
        assert_eq!(target.as_deref(), Some("24384/9"));
    }

    /// 回退即红：两个发布入口都必须在渲染几何之后、事务提交之前把房间副作用并进去。
    #[test]
    fn both_publication_paths_carry_the_room_effects() {
        let source = include_str!("e3d_model_service.rs");
        let generate_refs = source
            .split_once("async fn generate_refs(")
            .expect("generate_refs")
            .1
            .split_once("fn pin(&self")
            .expect("generate_refs end")
            .0;
        let effects_at = generate_refs
            .find("room_publication_effects(")
            .expect("generate_refs 必须折出房间副作用");
        let prepare_at = generate_refs
            .find("prepare_geometry_delta(")
            .expect("prepare_geometry_delta");
        let render_at = generate_refs
            .find("render_room_publication_effects(")
            .expect("generate_refs 必须渲染房间副作用");
        let commit_at = generate_refs
            .find("publication_transaction(&publication)")
            .expect("publication commit");
        assert!(
            effects_at < prepare_at,
            "副作用要在 snapshot.elements 被搬走之前折出"
        );
        assert!(
            render_at < commit_at,
            "房间语句要在事务提交之前进 publication"
        );

        let apply = source
            .split_once("pub async fn apply_geometry_delta(")
            .expect("apply_geometry_delta")
            .1
            .split_once("pub(crate) enum ProjectionScope")
            .expect("apply_geometry_delta end")
            .0;
        let effects_at = apply
            .find("room_publication_effects(")
            .expect("apply_geometry_delta 必须折出房间副作用");
        let prepare_at = apply
            .find("prepare_geometry_delta(")
            .expect("prepare_geometry_delta");
        let render_at = apply
            .find("render_room_publication_effects(")
            .expect("apply_geometry_delta 必须渲染房间副作用");
        let commit_at = apply
            .find("publication_transaction(&statements)")
            .expect("publication commit");
        assert!(effects_at < prepare_at);
        assert!(render_at < commit_at);
    }
}
