//! 反向传播（ADR-073 D13 反查，2026-09-17）：面板变了 → 拿它的新旧盒问「谁在里面」→ 折成本库的根。
//!
//! 逐根路径（D1）只答「本根在哪些面板里」；面板那一侧变了，它名下**别的根**的成员边陈旧（§未决 U1），
//! 此前的填法是整库重算（`RoomOnly`，O(本库全部根)）。这里把「哪些根受影响」算出来，整库降为兜底：
//!
//! - **命中源三路**。① 全局空间树（真实元素、全库、`GLOBAL_AABB_TREE::locate_intersecting_bounds`，
//!   消费者门 [`ensure_spatial_ready`]——不就绪就说不了完整，调用方退整库）；② 投影里本库的隐式管身
//!   记录（**管身不进树**，`aabb_tree::projected_tree_entry` 对 `tube.is_some()` 回 `None`）；③ 库上
//!   `tubi_relate` 的 `aabb.d` 范围过滤（不在投影里的根的管身——初始化的、重启前生成的）。
//! - **元素 → 根**：`anc`（投影记录的 `ancestors` / 库行的 `anc`）∪ 自身，与本库 `DbSet@T` 的根清单求交；
//!   嵌套根共享记录时两根都算（多算一根是同一台计算器下的幂等重写）。折不出根的元素进 `unresolved`——
//!   调用方按「反查不完整」处置（退整库），不静默丢。
//! - **跨库命中只数不算**（§未决 U3，a 档）：面板在 A 库、成员在 B 库，任务按库、事务按库，本轮只重算
//!   本库的根；数进回执，b 档（给别的库排根级工作项）等数据再定。
//! - 面板本身不是成员（`noun == PANE` 剔掉，与 `calculate_edges` 同口径）。
//!
//! 不写树、不写库、不重算几何：这里只回答「谁」。

use std::collections::{BTreeMap, BTreeSet};

use aios_core::RefnoEnum;
use aios_core::accel_tree::acceleration_tree::AccelerationTree;
use aios_core::room::room::GLOBAL_AABB_TREE;
use e3d_io::refno::RefNo;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::generation_root::refno_to_e3d;
use crate::fast_model::model_memory_store::{ModelMemoryStore, packed_refno};
use crate::fast_model::room_resident::PanelDelta;
use crate::fast_model::spatial_state::ensure_spatial_ready;

/// 反查的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReverseLookup {
    /// 空间树不就绪（`SPATIAL_TREE_NOT_READY`：`Rebuilding` / `DegradedBlocked` …）：树上的答案不可信，
    /// 反查说不了完整。调用方退整库（D13）。
    TreeUnavailable(String),
    Resolved(AffectedRoots),
}

/// 受影响的根与账。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AffectedRoots {
    /// 本库命中的根（e3d-io `RefNo`，去重、按打包 refno 序）。
    pub roots: Vec<RefNo>,
    /// 树上命中的真实元素（去重、`PANE` 已剔、含跨库的）。
    pub tree_hits: usize,
    /// 命中的隐式管身来源元素（投影 + 库行，按来源去重、含跨库的）。
    pub tube_hits: usize,
    /// 命中却不属本库的元素数（只数不算，U3 a 档）。
    pub cross_library: usize,
    /// 本库命中却折不出根的元素（`anc` ∪ 自身与根清单不交）：反查不完整的证据，调用方据此退整库。
    pub unresolved: Vec<RefnoEnum>,
}

impl AffectedRoots {
    pub(crate) fn summary(&self) -> String {
        format!(
            "roots={} tree_hits={} tube_hits={} cross_library={} unresolved={}",
            self.roots.len(),
            self.tree_hits,
            self.tube_hits,
            self.cross_library,
            self.unresolved.len()
        )
    }
}

/// 面板变化 → 本库受影响的根。`library_roots` 是本库 `DbSet@T` 的根清单（`batch_worker::library_generation_roots`）；
/// 枚举不出来时调用方不该走到这里（没有清单折不出根）。
pub(crate) async fn affected_roots(
    db: &Surreal<Any>,
    store: &ModelMemoryStore,
    dbnum: u32,
    delta: &PanelDelta,
    library_roots: &[RefNo],
) -> anyhow::Result<ReverseLookup> {
    if let Err(error) = ensure_spatial_ready() {
        return Ok(ReverseLookup::TreeUnavailable(format!("{error:#}")));
    }
    let boxes = delta.query_boxes();
    if boxes.is_empty() {
        return Ok(ReverseLookup::Resolved(AffectedRoots::default()));
    }
    // 读锁只罩住取候选，中间没有 await；候选收完即放锁（与 `spatial_query::scan_global_tree` 同一纪律）。
    let tree_hits = {
        let tree = GLOBAL_AABB_TREE.read().await;
        tree_element_hits(&tree, &boxes)
    };
    resolve(db, store, dbnum, &boxes, tree_hits, library_roots).await
}

/// 树上与任一查询盒相交的真实元素（打包 refno），`PANE` 剔掉。
pub(crate) fn tree_element_hits(tree: &AccelerationTree, boxes: &[Aabb]) -> BTreeSet<i64> {
    let mut hits = BTreeSet::new();
    for aabb in boxes {
        for entry in tree.locate_intersecting_bounds(aabb) {
            if entry.noun.eq_ignore_ascii_case("PANE") {
                continue;
            }
            hits.insert(entry.refno.0 as i64);
        }
    }
    hits
}

/// 树的答案之后的那一半：投影里的管身按盒扫、库上管身范围查、不在投影里的元素回库拿 `anc`、分库、折根。
/// 拆出来是为了不碰全局树也能对着 `mem://` 测。
pub(crate) async fn resolve(
    db: &Surreal<Any>,
    store: &ModelMemoryStore,
    dbnum: u32,
    boxes: &[Aabb],
    tree_hits: BTreeSet<i64>,
    library_roots: &[RefNo],
) -> anyhow::Result<ReverseLookup> {
    let mut out = AffectedRoots {
        tree_hits: tree_hits.len(),
        ..Default::default()
    };
    // 投影里本库的记录：元素 → 祖先链现成；管身记录按盒扫（管身不进树）。
    let (_, projected) = store.snapshot_dbnum(dbnum);
    let mut ancestors_of: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let mut tube_hits: BTreeSet<i64> = BTreeSet::new();
    for record in &projected {
        let source = packed_refno(record.source_refno);
        ancestors_of
            .entry(source)
            .or_insert_with(|| record.ancestors.clone());
        if record.tube.is_some() {
            let aabb = Aabb::new(record.world_bounds[0].into(), record.world_bounds[1].into());
            if boxes.iter().any(|query| query.intersects(&aabb)) {
                tube_hits.insert(source);
            }
        }
    }
    // 库上的管身（不在投影里的根的，以及已落库的）：范围过滤，回来源与祖先链。
    for (source, anc) in durable_tube_hits(db, dbnum, boxes).await? {
        tube_hits.insert(source);
        ancestors_of.entry(source).or_insert(anc);
    }
    out.tube_hits = tube_hits.len();

    // 分库：不属本库的只数。
    let mut pending: Vec<i64> = Vec::new();
    for source in tree_hits.iter().chain(tube_hits.iter()) {
        if e3d_of(*source).dbno() != dbnum {
            out.cross_library += 1;
        } else {
            pending.push(*source);
        }
    }
    pending.sort_unstable();
    pending.dedup();
    // 树上命中、投影里没有的元素（重启前生成 / 初始化的）：回库拿 `anc`。
    let unknown: Vec<i64> = pending
        .iter()
        .copied()
        .filter(|source| !ancestors_of.contains_key(source))
        .collect();
    for (source, anc) in durable_ancestors(db, &unknown).await? {
        ancestors_of.insert(source, anc);
    }
    let roots: BTreeSet<i64> = library_roots
        .iter()
        .map(|root| packed_refno(*root))
        .collect();
    let (matched, unresolved) = fold_to_roots(
        pending.iter().map(|source| {
            (
                *source,
                ancestors_of.get(source).cloned().unwrap_or_default(),
            )
        }),
        &roots,
    );
    out.roots = matched.into_iter().map(e3d_of).collect();
    out.unresolved = unresolved.into_iter().map(refno_of).collect();
    Ok(ReverseLookup::Resolved(out))
}

/// 元素 → 根：`anc` ∪ 自身与根清单求交；交集为空的进第二个返回值（折不出根）。
pub(crate) fn fold_to_roots(
    hits: impl IntoIterator<Item = (i64, Vec<i64>)>,
    library_roots: &BTreeSet<i64>,
) -> (BTreeSet<i64>, Vec<i64>) {
    let mut roots = BTreeSet::new();
    let mut unresolved = Vec::new();
    for (source, ancestors) in hits {
        let mut matched = false;
        for candidate in std::iter::once(source).chain(ancestors) {
            if library_roots.contains(&candidate) {
                roots.insert(candidate);
                matched = true;
            }
        }
        if !matched {
            unresolved.push(source);
        }
    }
    (roots, unresolved)
}

fn e3d_of(packed: i64) -> RefNo {
    let raw = packed as u64;
    RefNo::new((raw >> 32) as u32, raw as u32)
}

fn refno_of(packed: i64) -> RefnoEnum {
    crate::data_interface::generation_root::refno_from_e3d(e3d_of(packed))
}

#[derive(serde::Deserialize)]
struct AncestorRow {
    refno: RefnoEnum,
    #[serde(default)]
    anc: Option<Vec<i64>>,
}

fn rows_into_pairs(rows: Vec<AncestorRow>) -> Vec<(i64, Vec<i64>)> {
    rows.into_iter()
        .map(|row| {
            (
                packed_refno(refno_to_e3d(row.refno)),
                row.anc.unwrap_or_default(),
            )
        })
        .collect()
}

/// 一个盒的 SurrealQL 相交谓词（闭区间，与 `intersecting_panel_slots` 同口径）：`aabb.d` 是链接解引用，
/// `{mins:[x,y,z], maxs:[x,y,z]}`。
fn box_predicate(aabb: &Aabb) -> String {
    format!(
        "(aabb.d.mins[0] <= {} AND aabb.d.maxs[0] >= {} AND aabb.d.mins[1] <= {} AND aabb.d.maxs[1] >= {} AND aabb.d.mins[2] <= {} AND aabb.d.maxs[2] >= {})",
        aabb.maxs.x, aabb.mins.x, aabb.maxs.y, aabb.mins.y, aabb.maxs.z, aabb.mins.z
    )
}

/// 库上本库的隐式管身里、世界盒与任一查询盒相交的：来源元素 + 祖先链。O(本库管身行) 的链接解引用，不读几何。
pub(crate) async fn durable_tube_hits(
    db: &Surreal<Any>,
    dbnum: u32,
    boxes: &[Aabb],
) -> anyhow::Result<Vec<(i64, Vec<i64>)>> {
    if boxes.is_empty() {
        return Ok(Vec::new());
    }
    let predicate = boxes
        .iter()
        .map(box_predicate)
        .collect::<Vec<_>>()
        .join(" OR ");
    let rows: Vec<AncestorRow> = db
        .query(format!(
            "SELECT in AS refno, anc FROM tubi_relate WHERE dbnum = {dbnum} AND aabb.d != NONE AND ({predicate});"
        ))
        .await?
        .check()?
        .take(0)?;
    Ok(rows_into_pairs(rows))
}

/// 树上命中、投影里没有的真实元素：按记录 id（真实元素的 `inst_relate` 记录 id 就是它的 refno）批量取 `anc`。
pub(crate) async fn durable_ancestors(
    db: &Surreal<Any>,
    sources: &[i64],
) -> anyhow::Result<Vec<(i64, Vec<i64>)>> {
    const CHUNK: usize = 400;
    let mut out = Vec::new();
    for chunk in sources.chunks(CHUNK) {
        let targets = chunk
            .iter()
            .map(|source| format!("inst_relate:⟨{}⟩", refno_of(*source)))
            .collect::<Vec<_>>()
            .join(", ");
        let rows: Vec<AncestorRow> = db
            .query(format!("SELECT in AS refno, anc FROM {targets};"))
            .await?
            .check()?
            .take(0)?;
        out.extend(rows_into_pairs(rows));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::RefU64;
    use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
    use e3d_model::elmodl::GeometryId;
    use glam::DMat4;
    use parry3d::math::Point;

    use crate::fast_model::model_memory_store::{
        ProjectedGeometry, ProjectedShape, ProjectedTube, ProjectionCommit, ProjectionScope,
    };

    fn packed(word0: u32, word1: u32) -> i64 {
        packed_refno(RefNo::new(word0, word1))
    }

    fn aabb(min: f32, max: f32) -> Aabb {
        Aabb::new(Point::new(min, min, min), Point::new(max, max, max))
    }

    fn refno(word0: u32, word1: u32) -> RefnoEnum {
        RefnoEnum::from(RefU64::from_two_nums(word0, word1))
    }

    /// 折根：`anc` ∪ 自身与根清单求交；嵌套根两根都算；交集为空的进 `unresolved`。
    #[test]
    fn hits_fold_to_every_library_root_on_their_ancestor_chain() {
        let roots = BTreeSet::from([packed(24381, 1), packed(24381, 2), packed(24381, 3)]);
        let (matched, unresolved) = fold_to_roots(
            [
                // 普通构件：祖先链上有根 1。
                (packed(24381, 10), vec![packed(24381, 5), packed(24381, 1)]),
                // 根自己就是几何来源（EQUI 带几何）。
                (packed(24381, 2), vec![packed(24381, 100)]),
                // 嵌套根：3 在 1 里，两根都算。
                (packed(24381, 30), vec![packed(24381, 3), packed(24381, 1)]),
                // 祖先链上没有清单里的根：折不出。
                (packed(24381, 40), vec![packed(24381, 999)]),
            ],
            &roots,
        );
        assert_eq!(
            matched,
            BTreeSet::from([packed(24381, 1), packed(24381, 2), packed(24381, 3)])
        );
        assert_eq!(unresolved, vec![packed(24381, 40)]);
    }

    /// 树上取候选：与任一盒相交的都算、去重；面板（`PANE`）不是成员，剔掉。
    #[test]
    fn tree_candidates_exclude_panels_and_deduplicate_across_boxes() {
        let tree = AccelerationTree::load(vec![
            RStarBoundingBox::new(aabb(0.0, 1.0), refno(24381, 10), "BOX".into()),
            RStarBoundingBox::new(aabb(0.5, 1.5), refno(24381, 11), "CYLI".into()),
            RStarBoundingBox::new(aabb(0.0, 2.0), refno(24381, 7), "PANE".into()),
            RStarBoundingBox::new(aabb(10.0, 11.0), refno(24381, 12), "BOX".into()),
        ]);
        let hits = tree_element_hits(&tree, &[aabb(0.0, 1.0), aabb(1.2, 1.4)]);
        assert_eq!(
            hits,
            BTreeSet::from([packed(24381, 10), packed(24381, 11)]),
            "两个盒都命中 11 只算一次；PANE 与远处的 12 不进"
        );
    }

    fn record(
        id: &str,
        source: RefNo,
        root: RefNo,
        bounds: (f32, f32),
        tube: Option<ProjectedTube>,
    ) -> ProjectedGeometry {
        ProjectedGeometry {
            record_id: id.into(),
            geometry_id: GeometryId::Element {
                refno: format!("{}/{}", source.word0, source.word1),
            },
            source_refno: source,
            dbnum: 7997,
            source_sesno: 3,
            noun: if tube.is_some() {
                "TUBI".into()
            } else {
                "BOX".into()
            },
            mesh_id: "mesh".into(),
            world_transform: DMat4::IDENTITY,
            world_bounds: [[bounds.0; 3], [bounds.1; 3]],
            ancestors: vec![packed_refno(root)],
            shape: ProjectedShape::Baked,
            tube,
        }
    }

    fn tube(from: u32, to: u32) -> ProjectedTube {
        ProjectedTube {
            route_ordinal: 0,
            from: RefNo::new(24381, from),
            to: RefNo::new(24381, to),
            invalid: false,
        }
    }

    /// 树的答案之后那一半（`mem://`，不碰全局树）：投影里的管身按盒扫进来、库上管身范围查进来、树上命中但
    /// 投影里没有的元素回库拿 `anc`；跨库命中只数；折不出根的进 `unresolved`；三条路折出的根合成一个集合。
    #[tokio::test]
    async fn resolve_joins_tree_projection_and_durable_tubes_into_library_roots() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("room_reverse").await.unwrap();
        let root_a = RefNo::new(24381, 1);
        let root_b = RefNo::new(24381, 2);
        let root_c = RefNo::new(24381, 3);
        let root_d = RefNo::new(24381, 4);
        let library_roots = [root_a, root_b, root_c, root_d];
        // 库上：不在投影里的元素 24381/300（根 C）带 anc；管身行 24381/400（根 D，盒 [5,6]）与 24381/500
        // （根 D，盒 [50,51]，不该命中）；另一库 24382/9 的管身盒相交但 dbnum 不同，查询按库、不该进来。
        db.query(format!(
            r#"
            CREATE aabb:near SET d={{mins:[5,5,5],maxs:[6,6,6]}};
            CREATE aabb:far SET d={{mins:[50,50,50],maxs:[51,51,51]}};
            CREATE inst_relate:⟨24381_300⟩ SET in=pe:⟨24381_300⟩, anc=[{}, {}], dbnum=7997;
            CREATE tubi_relate:⟨t400⟩ SET in=pe:⟨24381_400⟩, anc=[{}], dbnum=7997, aabb=aabb:near;
            CREATE tubi_relate:⟨t500⟩ SET in=pe:⟨24381_500⟩, anc=[{}], dbnum=7997, aabb=aabb:far;
            CREATE tubi_relate:⟨t9⟩ SET in=pe:⟨24382_9⟩, anc=[{}], dbnum=7998, aabb=aabb:near;
            "#,
            packed(24381, 77),
            packed_refno(root_c),
            packed_refno(root_d),
            packed_refno(root_d),
            packed(24382, 1),
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        // 投影：根 A 的构件 100（盒 [0,1]）与根 B 的管身 200（盒 [4.5,5.5]，与查询盒 [5,6] 相交）；
        // 根 A 还有一段管身 101 在远处（不相交）。
        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![
                    record(
                        "24381_100",
                        RefNo::new(24381, 100),
                        root_a,
                        (0.0, 1.0),
                        None,
                    ),
                    record(
                        "24381_101_tube",
                        RefNo::new(24381, 101),
                        root_a,
                        (80.0, 81.0),
                        Some(tube(100, 102)),
                    ),
                    record(
                        "24381_200_tube",
                        RefNo::new(24381, 200),
                        root_b,
                        (4.5, 5.5),
                        Some(tube(201, 202)),
                    ),
                ],
                ..ProjectionCommit::default()
            }],
        );
        // 树的答案（模拟）：投影里的 100、库上的 300、跨库的 24382/8、折不出根的 24381/600。
        let tree_hits = BTreeSet::from([
            packed(24381, 100),
            packed(24381, 300),
            packed(24382, 8),
            packed(24381, 600),
        ]);
        let boxes = [aabb(0.0, 1.0), aabb(5.0, 6.0)];
        let ReverseLookup::Resolved(affected) =
            resolve(&db, &store, 7997, &boxes, tree_hits, &library_roots)
                .await
                .unwrap()
        else {
            panic!("树就绪与否不在这一半判")
        };
        assert_eq!(
            affected.roots,
            vec![root_a, root_b, root_c, root_d],
            "{affected:?}"
        );
        assert_eq!(affected.tree_hits, 4);
        assert_eq!(
            affected.tube_hits, 2,
            "投影里的 200 + 库上的 400；远处的 101 / 500 不算，别的库的管身行（24382/9）按 dbnum 在库里就筛掉"
        );
        assert_eq!(
            affected.cross_library, 1,
            "树上的 24382/8 只数不算（树是全库的；管身查询按库，跨库管身进不来）"
        );
        assert_eq!(affected.unresolved, vec![refno(24381, 600)]);
        assert_eq!(
            affected.summary(),
            "roots=4 tree_hits=4 tube_hits=2 cross_library=1 unresolved=1"
        );
    }

    /// 盒谓词是闭区间、逐轴。
    #[test]
    fn the_box_predicate_is_a_closed_interval_test_per_axis() {
        let predicate = box_predicate(&Aabb::new(
            Point::new(1.0, 2.0, 3.0),
            Point::new(4.0, 5.0, 6.0),
        ));
        assert_eq!(
            predicate,
            "(aabb.d.mins[0] <= 4 AND aabb.d.maxs[0] >= 1 AND aabb.d.mins[1] <= 5 AND aabb.d.maxs[1] >= 2 AND aabb.d.mins[2] <= 6 AND aabb.d.maxs[2] >= 3)"
        );
    }
}
