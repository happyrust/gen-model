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
//!
//! **生产调用点是落库适配器**（`model_db_adapter::persist_root_in_chunks`，2026-09-04 接回）。
//! 同日早些时候模型生成改成只写进程内投影存储（`model_memory_store`），发布事务连同它携带
//! 的房间语句一起从 `e3d_model_service` 里撤了，这两支一度悬空；现在折算的输入换成投影记录
//! （upsert 是这一根的全部记录，removal 是对账出来的陈旧几何），渲染出来的语句仍与几何写在
//! 同一条事务上——只是切块之后清边跟着删除那一块走、排重算跟着回执那一块走，理由见适配器。

use std::collections::{BTreeMap, BTreeSet};

use aios_core::{GeomInstQuery, RefnoEnum, SUL_DB};
use anyhow::Context;
use e3d_io::refno::RefNo;
use e3d_model::elmodl::GeometryId;

use crate::data_interface::generation_root::refno_from_e3d;
use crate::fast_model::aabb_refresh::AabbChange;

const PUBLISHED_ROOM_INPUT_TABLE: &str = "room_published_inst";

/// Render one immutable room-geometry version from the current model rows.
///
/// The row set is written before the file manifest is switched.  Readers only
/// select tokens present in `published_file_input`, so a crash before the
/// spatial publication leaves this candidate invisible.  Retrying the same
/// frozen round replaces only its own candidate rows.
pub(crate) fn render_stage_published_room_input(dbnum: u32, commit_token: &str) -> String {
    let token = crate::data_interface::dbnum_state::escape_surql_str(commit_token);
    format!(
        "DELETE {PUBLISHED_ROOM_INPUT_TABLE} WHERE dbnum = {dbnum} AND commit_token = '{token}' RETURN NONE; \
         INSERT INTO {PUBLISHED_ROOM_INPUT_TABLE} \
         SELECT {dbnum} AS dbnum, '{token}' AS commit_token, in AS refno, in.old_pe AS old_refno, \
           (in.owner ?? type::thing('pe', string::concat(<string><int>math::floor(anc[1] / 4294967296), '_', <string>(anc[1] % 4294967296)))) AS owner, \
           generic, (aabb_d ?? aabb.d) AS world_aabb, \
           (world_trans_d ?? world_trans.d) AS world_trans, out.ptset.d.pt AS pts, \
           IF booled_id != NONE {{ [{{ geo_hash: booled_id }}] }} \
           ELSE {{ insts_flat ?? (SELECT (transform ?? trans.d) AS transform, record::id(out) AS geo_hash \
                                  FROM out->geo_relate \
                                  WHERE visible AND out.meshed AND (transform ?? trans.d) != NONE AND geo_type = 'Pos') }} AS insts, \
           booled_id != NONE AS has_neg, dt AS date \
         FROM inst_relate \
         WHERE (dbnum = {dbnum} OR string::starts_with(record::id(in), '{dbnum}_')) \
           AND (aabb_d ?? aabb.d) != NONE \
           AND (world_trans_d ?? world_trans.d) != NONE RETURN NONE;"
    )
}

/// Materialize a complete candidate for one frozen file version.  Publication
/// of the matching token is a separate, later transaction.
pub(crate) async fn stage_published_room_input(
    dbnum: u32,
    commit_token: &str,
) -> anyhow::Result<()> {
    let statements = render_stage_published_room_input(dbnum, commit_token);
    SUL_DB
        .query(format!(
            "BEGIN TRANSACTION; {statements} COMMIT TRANSACTION;"
        ))
        .await
        .context("stage immutable room input query")?
        .check()
        .context("stage immutable room input statements")?;
    Ok(())
}

/// Read room geometry only from file versions named by the committed manifest.
/// Candidate versions may already exist in the same table, but remain
/// invisible until their file token is published.
pub(crate) async fn query_published_room_insts(
    refnos: &[RefnoEnum],
) -> anyhow::Result<Vec<GeomInstQuery>> {
    query_published_room_insts_on(&SUL_DB, refnos).await
}

async fn query_published_room_insts_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<Vec<GeomInstQuery>> {
    if refnos.is_empty() {
        return Ok(Vec::new());
    }
    let sql = render_published_room_inst_query(refnos);
    let mut response = db.query(sql).await?.check()?;
    response
        .take::<Vec<GeomInstQuery>>(0)
        .map_err(anyhow::Error::from)
}

fn render_published_room_inst_query(refnos: &[RefnoEnum]) -> String {
    let keys = refnos
        .iter()
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT refno, old_refno, owner, generic, world_aabb, world_trans, pts, insts, has_neg, date \
         FROM {PUBLISHED_ROOM_INPUT_TABLE} \
         WHERE commit_token IN (SELECT VALUE commit_token FROM published_file_input) \
           AND (refno IN [{keys}] OR old_refno IN [{keys}]);"
    )
}

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

/// Apply edge removals captured while a file round was still private. This is
/// deliberately called only after that round's spatial view has been
/// published; failures leave the room rows pending for retry.
pub(crate) async fn clear_deferred_room_edges(refnos: &[String]) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }
    let cleared = refnos
        .iter()
        .map(|raw| {
            raw.parse::<aios_core::RefU64>()
                .map(aios_core::RefnoEnum::from)
                .map_err(|err| anyhow::anyhow!("invalid deferred room refno {raw}: {err:?}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let effects = RoomPublicationEffects {
        recalc: Vec::new(),
        cleared,
    };
    let statements = render_room_publication_effects(&effects, RoomEffectPolicy::Directed, true);
    if statements.is_empty() {
        return Ok(());
    }
    aios_core::SUL_DB
        .query(format!(
            "BEGIN TRANSACTION;\n{statements}\nCOMMIT TRANSACTION;"
        ))
        .await?
        .check()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn room_input_candidates_stay_hidden_until_the_file_token_switches() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_publication")
            .use_db("versioned_input")
            .await
            .unwrap();
        db.query(
            "CREATE pe:24384_1 SET noun = 'SITE'; \
             CREATE pe:24384_7 SET noun = 'EQUI'; \
             CREATE inst_relate:24384_7 SET dbnum = 8000, in = pe:24384_7, \
               anc = [104728482619399, 104728482619393], generic = 'OLD', \
               aabb_d = { mins: [0f, 0f, 0f], maxs: [1f, 1f, 1f] }, \
               world_trans_d = { translation: [0f, 0f, 0f], rotation: [0f, 0f, 0f, 1f], scale: [1f, 1f, 1f] }, \
               insts_flat = [{ geo_hash: 'mesh-old' }], booled_id = NONE;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(format!(
            "BEGIN TRANSACTION; {} \
             CREATE published_file_input:8000 SET dbnum = 8000, commit_token = 'old-token'; \
             COMMIT TRANSACTION;",
            render_stage_published_room_input(8000, "old-token")
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query("UPDATE inst_relate:24384_7 SET generic = 'NEW';")
            .await
            .unwrap()
            .check()
            .unwrap();
        db.query(format!(
            "BEGIN TRANSACTION; {} COMMIT TRANSACTION;",
            render_stage_published_room_input(8000, "new-token")
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        let target = RefnoEnum::from("24384/7");
        let mut response = db
            .query(render_published_room_inst_query(&[target]))
            .await
            .unwrap()
            .check()
            .unwrap();
        let rows: Vec<GeomInstQuery> = response.take(0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].generic, "OLD");

        db.query("UPDATE published_file_input:8000 SET commit_token = 'new-token';")
            .await
            .unwrap()
            .check()
            .unwrap();
        let mut response = db
            .query(render_published_room_inst_query(&[target]))
            .await
            .unwrap()
            .check()
            .unwrap();
        let rows: Vec<GeomInstQuery> = response.take(0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].generic, "NEW");
    }

    #[tokio::test]
    async fn completed_file_publishes_with_the_other_files_previous_version() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_publication")
            .use_db("cross_file_versions")
            .await
            .unwrap();
        db.query(
            "CREATE pe:24384_7 SET noun = 'EQUI'; \
             CREATE pe:24385_8 SET noun = 'EQUI'; \
             CREATE inst_relate:a_old SET dbnum = 8000, in = pe:24384_7, \
               anc = [104728482619399, 104728482619393], generic = 'A-OLD', \
               aabb_d = { mins: [0f, 0f, 0f], maxs: [1f, 1f, 1f] }, \
               world_trans_d = { translation: [0f, 0f, 0f], rotation: [0f, 0f, 0f, 1f], scale: [1f, 1f, 1f] }, \
               insts_flat = [{ geo_hash: 'mesh-a-old' }], booled_id = NONE; \
             CREATE inst_relate:b_old SET dbnum = 8001, in = pe:24385_8, \
               anc = [104732777586696, 104732777586689], generic = 'B-OLD', \
               aabb_d = { mins: [2f, 0f, 0f], maxs: [3f, 1f, 1f] }, \
               world_trans_d = { translation: [0f, 0f, 0f], rotation: [0f, 0f, 0f, 1f], scale: [1f, 1f, 1f] }, \
               insts_flat = [{ geo_hash: 'mesh-b-old' }], booled_id = NONE;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(format!(
            "BEGIN TRANSACTION; {} {} \
             CREATE published_file_input:8000 SET dbnum = 8000, commit_token = 'a-old'; \
             CREATE published_file_input:8001 SET dbnum = 8001, commit_token = 'b-old'; \
             COMMIT TRANSACTION;",
            render_stage_published_room_input(8000, "a-old"),
            render_stage_published_room_input(8001, "b-old")
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        db.query(
            "UPDATE inst_relate:a_old SET generic = 'A-NEW', insts_flat = [{ geo_hash: 'mesh-a-new' }]; \
             UPDATE inst_relate:b_old SET generic = 'B-NEW', insts_flat = [{ geo_hash: 'mesh-b-new' }];",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(format!(
            "BEGIN TRANSACTION; {} {} COMMIT TRANSACTION;",
            render_stage_published_room_input(8000, "a-new"),
            render_stage_published_room_input(8001, "b-new")
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        let targets = [RefnoEnum::from("24384/7"), RefnoEnum::from("24385/8")];
        let mut before = query_published_room_insts_on(&db, &targets)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.generic)
            .collect::<Vec<_>>();
        before.sort();
        assert_eq!(before, ["A-OLD", "B-OLD"]);

        // A 的新轮次失败并保持旧清单；只发布 B。随后删除实时模型行，确保读取没有兜底。
        db.query(
            "UPDATE published_file_input:8001 SET commit_token = 'b-new'; \
             DELETE inst_relate;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let mut after = query_published_room_insts_on(&db, &targets)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.generic)
            .collect::<Vec<_>>();
        after.sort();
        assert_eq!(after, ["A-OLD", "B-NEW"]);
    }

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
}
