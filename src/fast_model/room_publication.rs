//! e3d 发布事务里的房间派生面副作用（ADR-010 §4 / ADR-040 §3 / ADR-057 D2）。
//!
//! 旧生成器的房间触发住在 AABB 刷新里（`aabb_refresh.rs` → `render_room_recalc_upserts`），
//! e3d-model 接管生成之后那条链不再被走到：`E3dModelService` 直写 `aabb` 行、bump spatial
//! epoch、同步空间树，但从不排 `RoomRecalc*`，也不清被移除几何的 `room_relate` /
//! `room_panel_relate` 边（`helper.rs` 写着：少清一个方向，`fn::room_relate_of` 照样把悬空边
//! 取出来）。本模块把两件事折回**同一个发布事务**（ADR-040 §3 的同事务纪律）：
//!
//! - **重算名单**：本次发布 upsert 的每个 `GeometryId::Element` 来源元素按 noun 分流
//!   （PANE / 其余）折成 `recalc` 名单——ADR-040 §1 的保守口径：定向生成的目标就是这一窗
//!   真正重写的元素。2026-09-10（ADR-072 D6）起它**只进回执与日志**、不再渲染成
//!   房间待办表的 `RoomRecalc*` 行：房间归属由本 dbnum 任务的房间段
//!   （`room_task::finish_task_rooms`）按内存投影整库算完写边。隐式管身（`ImpliedTube`）
//!   不进名单：房间系统不读 `tubi_relate`。
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

use aios_core::{GeomInstQuery, RefnoEnum};
use anyhow::Context;
use e3d_io::refno::RefNo;
use e3d_model::elmodl::GeometryId;

use crate::data_interface::generation_root::refno_from_e3d;
use crate::fast_model::aabb_refresh::AabbChange;

/// 在册面板的几何从 `inst_relate` **实时行**读（d-21 / ADR-072 D5 白名单里的「启动水化：
/// 常驻房间模型几何（实时行）」）。发布视图几何表（按清单令牌选版本）
/// 随 publish 阶段一起退役：库里只有一份几何，就是任务同步写进去的那份。
///
/// 投影形状与从前发布视图那一份逐字同形（`owner` 缺省从 `anc[1]` 折出生成根；`insts`
/// 先看 `booled_id`，再看 `insts_flat`，最后才走 `geo_relate`），所以下游 `assemble` 一字不改。
/// 只按 `in IN [...]` 点名查（`idx_inst_relate_in`），不扫表。
pub(crate) fn render_live_room_inst_query(refnos: &[RefnoEnum]) -> String {
    let keys = refnos
        .iter()
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT in AS refno, in.old_pe AS old_refno, \
           (in.owner ?? type::thing('pe', string::concat(<string><int>math::floor(anc[1] / 4294967296), '_', <string>(anc[1] % 4294967296)))) AS owner, \
           generic, (aabb_d ?? aabb.d) AS world_aabb, \
           (world_trans_d ?? world_trans.d) AS world_trans, out.ptset.d.pt AS pts, \
           IF booled_id != NONE {{ [{{ geo_hash: booled_id }}] }} \
           ELSE {{ insts_flat ?? (SELECT (transform ?? trans.d) AS transform, record::id(out) AS geo_hash \
                                  FROM out->geo_relate \
                                  WHERE visible AND out.meshed AND (transform ?? trans.d) != NONE AND geo_type = 'Pos') }} AS insts, \
           booled_id != NONE AS has_neg, dt AS date \
         FROM inst_relate \
         WHERE in IN [{keys}] \
           AND (aabb_d ?? aabb.d) != NONE \
           AND (world_trans_d ?? world_trans.d) != NONE;"
    )
}

/// 按 refno 点名读实时行几何。`db` 显式传入：启动水化喂 `SUL_DB`，`mem://` 门直接喂。
pub(crate) async fn query_live_room_insts_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<Vec<GeomInstQuery>> {
    if refnos.is_empty() {
        return Ok(Vec::new());
    }
    let mut response = db
        .query(render_live_room_inst_query(refnos))
        .await
        .context("live room inst query")?
        .check()
        .context("live room inst statements")?;
    response
        .take::<Vec<GeomInstQuery>>(0)
        .map_err(anyhow::Error::from)
}

pub(crate) fn query_memory_room_insts(refnos: &[RefnoEnum]) -> anyhow::Result<Vec<GeomInstQuery>> {
    if refnos.is_empty() { return Ok(Vec::new()); }
    let resident = crate::fast_model::room_resident::current_for_calculation()?;
    let mut rows = Vec::new();
    let mut members = BTreeSet::new();
    for refno in refnos.iter().copied().collect::<BTreeSet<_>>() {
        anyhow::ensure!(!resident.missing_panels.contains(&refno), "preloaded room panel {refno} has no geometry");
        if let Some(panel) = resident.panels.get(&refno) {
            rows.push(panel.room_inst());
        } else {
            members.insert(refno.refno().0 as i64);
        }
    }
    rows.extend(crate::fast_model::model_memory_store::ModelMemoryStore::global()
        .room_records_for_members(&members).iter()
        .filter(|record| matches!(record.geometry_id, GeometryId::Element { .. }))
        .map(|record| record.room_inst()));
    Ok(rows)
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

/// 渲染成发布事务里的语句：只清边。空影响渲染成空串。
///
/// 「排重算」那一半（`RoomRecalcPanel` / `RoomRecalcElement` 进房间待办表）随
/// ADR-072 D6 退役：房间归属由本 dbnum 任务的房间段（`room_task::finish_task_rooms`）按内存
/// 投影整库算完写边，不再靠逐元素的 durable 待办行；`recalc` 名单只剩回执与日志用。
/// `policy` / `room_incremental` 两个参数保留给调用方（落库适配器）的既有签名，不再影响渲染。
pub(crate) fn render_room_publication_effects(
    effects: &RoomPublicationEffects,
    policy: RoomEffectPolicy,
    room_incremental: bool,
) -> String {
    let _ = (policy, room_incremental);
    let mut out = String::new();
    for chunk in effects.cleared.chunks(ROOM_EDGE_DELETE_CHUNK) {
        let keys: Vec<String> = chunk.iter().map(RefnoEnum::to_pe_key).collect();
        out.push_str(&crate::data_interface::helper::render_room_membership_delete(&keys));
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

    /// 发布视图退役（ADR-072 D4）：本文件没有整库复制几何的写者、没有按清单令牌选版本的
    /// 读者；内存那条读法只经常驻面板与投影存储；实时行那条读法只按 refno 点名、不扫表。
    #[test]
    fn room_input_has_no_published_copy_and_the_live_read_is_keyed_by_refno() {
        let source = include_str!("room_publication.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        for needle in [
            "published_file_input",
            "commit_token",
            "model_update_pending",
            "side_effect_pending",
        ] {
            assert!(!production.contains(needle), "生产路径不该再出现 `{needle}`");
        }
        let input = source.split_once("pub(crate) fn query_memory_room_insts(").unwrap().1
            .split_once("/// 这次发布是定向生成").unwrap().0;
        assert!(!input.contains("SUL_DB") && !input.contains(".query("));
        assert!(input.contains("resident.panels.get") && input.contains("room_records_for_members"));

        let sql = render_live_room_inst_query(&[RefnoEnum::from("24384/7"), RefnoEnum::from("24385/8")]);
        assert!(sql.contains("FROM inst_relate"));
        assert!(sql.contains("WHERE in IN [pe:24384_7, pe:24385_8]"), "{sql}");
        assert!(!sql.contains("SELECT * FROM inst_relate"), "不许扫表: {sql}");

        let resident = include_str!("room_resident.rs").replace("\r\n", "\n");
        let published = resident.split_once("pub(crate) async fn load_published(").unwrap().1.split_once("\n}\n").unwrap().0;
        assert!(published.contains("load_file_backed("));
        assert!(!published.contains("SUL_DB") && !published.contains("load_published_on("));
    }

    /// `mem://` 门：实时行读法只回点名的 refno、投影形状与从前发布视图逐字同形（`owner`
    /// 缺省从 `anc[1]` 折出生成根，`insts` 取 `insts_flat`），行改了读到的就是改后的那份
    /// ——库里只有一份几何，没有「候选 / 已发布」两个版本可选。
    #[tokio::test]
    async fn the_live_read_returns_the_current_rows_for_the_named_refnos_only() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_publication")
            .use_db("live_input")
            .await
            .unwrap();
        db.query(
            "CREATE pe:24384_1 SET noun = 'SITE'; \
             CREATE pe:24384_7 SET noun = 'EQUI'; \
             CREATE pe:24385_8 SET noun = 'EQUI'; \
             CREATE inst_relate:24384_7 SET dbnum = 8000, in = pe:24384_7, \
               anc = [104728482619399, 104728482619393], generic = 'OLD', \
               aabb_d = { mins: [0f, 0f, 0f], maxs: [1f, 1f, 1f] }, \
               world_trans_d = { translation: [0f, 0f, 0f], rotation: [0f, 0f, 0f, 1f], scale: [1f, 1f, 1f] }, \
               insts_flat = [{ geo_hash: 'mesh-old' }], booled_id = NONE; \
             CREATE inst_relate:24385_8 SET dbnum = 8001, in = pe:24385_8, \
               anc = [104732777586696, 104732777586689], generic = 'B', \
               aabb_d = { mins: [2f, 0f, 0f], maxs: [3f, 1f, 1f] }, \
               world_trans_d = { translation: [0f, 0f, 0f], rotation: [0f, 0f, 0f, 1f], scale: [1f, 1f, 1f] }, \
               insts_flat = [{ geo_hash: 'mesh-b' }], booled_id = NONE;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let target = RefnoEnum::from("24384/7");
        let rows = query_live_room_insts_on(&db, &[target]).await.unwrap();
        assert_eq!(rows.len(), 1, "只回点名的那一行");
        assert_eq!(rows[0].generic, "OLD");
        assert_eq!(rows[0].refno, target);
        // anc[1] = 104728482619393 = 24384 << 32 | 73729：owner 从它折出生成根。
        assert_eq!(rows[0].owner, RefnoEnum::from("24384/73729"), "owner 从 anc[1] 折出");
        assert_eq!(rows[0].insts.len(), 1);
        assert_eq!(rows[0].insts[0].geo_hash, "mesh-old");

        db.query("UPDATE inst_relate:24384_7 SET generic = 'NEW', insts_flat = [{ geo_hash: 'mesh-new' }];")
            .await
            .unwrap()
            .check()
            .unwrap();
        let rows = query_live_room_insts_on(&db, &[target]).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].generic, "NEW");
        assert_eq!(rows[0].insts[0].geo_hash, "mesh-new");

        let mut both = query_live_room_insts_on(&db, &[target, RefnoEnum::from("24385/8")])
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.generic)
            .collect::<Vec<_>>();
        both.sort();
        assert_eq!(both, ["B", "NEW"]);
        assert!(query_live_room_insts_on(&db, &[]).await.unwrap().is_empty());
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

    /// 清边不看开关也不看生成种类；**重算不再渲染成待办行**（ADR-072 D6：房间归属由任务的
    /// 房间段整库算，`model_update_pending` 退役）；空影响是空串。
    #[test]
    fn rendering_always_clears_edges_and_never_enqueues_recalc_rows() {
        let effects = effects_of(
            &[
                (element("24384/7"), 7, "PANE"),
                (element("24384/5"), 5, "EQUI"),
            ],
            &[element("24384/9")],
            &[],
        );
        assert_eq!(effects.recalc.len(), 2, "名单照旧折出来，供回执与日志");
        for (policy, switch) in [
            (RoomEffectPolicy::Directed, true),
            (RoomEffectPolicy::Directed, false),
            (RoomEffectPolicy::FullDatabase, true),
        ] {
            let rendered = render_room_publication_effects(&effects, policy, switch);
            for needle in [
                "DELETE pe:24384_9<->room_relate;",
                "DELETE pe:24384_9<->room_panel_relate;",
            ] {
                assert!(rendered.contains(needle), "missing `{needle}` in {rendered}");
            }
            assert!(
                !rendered.contains("room_recalc_") && !rendered.contains("model_update_pending"),
                "{policy:?}/{switch}: 不许再排待办行: {rendered}"
            );
        }

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
    /// 成员原样；重写的成员**不再**排待办行（归属由任务房间段算）。库里**没有** pe 记录的
    /// 来源（零解析库）清边同样不报错。
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
            .query("SELECT VALUE id FROM model_update_pending;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let pending: Vec<surrealdb::sql::Thing> = response.take(0).unwrap();
        assert!(pending.is_empty(), "发布事务不再排房间待办行: {pending:?}");
    }
}
