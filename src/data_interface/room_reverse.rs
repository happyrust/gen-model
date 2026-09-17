//! 反向传播（ADR-073 D13 反查，2026-09-17）：面板变了 → 拿它的新旧盒问「谁在里面」→ 折成本库的根。
//!
//! 逐根路径（D1）只答「本根在哪些面板里」；面板那一侧变了，它名下**别的根**的成员边陈旧（§未决 U1），
//! 此前的填法是整库重算（`RoomOnly`，O(本库全部根)）。这里把「哪些根受影响」算出来，整库降为兜底：
//!
//! - **命中源三路，都不分库**。① 全局空间树（真实元素、全库、`GLOBAL_AABB_TREE::locate_intersecting_bounds`，
//!   消费者门 [`ensure_spatial_ready`]——不就绪就说不了完整，调用方退整库）；② 投影里的隐式管身记录按盒扫
//!   （**管身不进树**，`aabb_tree::projected_tree_entry` 对 `tube.is_some()` 回 `None`；一把读锁过全 MDB，
//!   不复制）；③ 库上 `tubi_relate` 的 `aabb.d` 范围过滤（不在投影里的根的管身——初始化的、重启前生成的），
//!   **不带 `dbnum =`**——面板在 A 库、成员在 B 库是常态，按库过滤会把 B 的命中漏在这一层。
//! - **元素 → 根**：`anc`（投影记录的 `ancestors` / 库行的 `anc`）∪ 自身，与本库 `DbSet@T` 的根清单求交；
//!   嵌套根共享记录时两根都算（多算一根是同一台计算器下的幂等重写）。折不出根的元素进 `unresolved`——
//!   调用方按「反查不完整」处置（退整库），不静默丢。
//! - **跨库命中按库分桶、存元素不折根**（§未决 U3，b 档，2026-09-17）：任务按库、事务按库，本轮只重算本库的根；
//!   别的库命中的元素连同祖先链进 [`AffectedRoots::foreign`]，由调用方停放给那个库、它的下一个任务折根
//!   （根清单在那个库的 `DbSet@T` 上，这里枚举不了；祖先链在这里就带上——管身的来源是 BRAN 那种容器，
//!   `inst_relate` 里没有它的行，那个库回库也取不到）。`cross_library` 是这些元素的总数。
//! - 面板本身不是成员（`noun == PANE` 剔掉，与 `calculate_edges` 同口径）。
//!
//! 不写树、不写库、不重算几何：这里只回答「谁」。
//!
//! **停放槽**（U3 b 档，2026-09-17，本模块下半）：A 库的任务定向重算成功之后，`foreign` 里 B 库的那一桶连同这一段
//! 换代（`from → to`）停进 B 的槽（[`park`]），B 的下一个任务开工时取出（[`pending`]）接进准入链、折成 B 的根只重算
//! 它们，成功后清槽（[`clear`]）。进程内、重启即丢——丢了是**多算不漏算**：③ 电平型触发（库上代号 ≠ 常驻代号）
//! 兜底整库。要不要落库等 live 看到重启后整库的频率再定。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{PoisonError, RwLock};
use std::time::Instant;

use aios_core::RefnoEnum;
use aios_core::accel_tree::acceleration_tree::AccelerationTree;
use aios_core::room::room::GLOBAL_AABB_TREE;
use e3d_io::refno::RefNo;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::generation_root::refno_to_e3d;
use crate::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope, packed_refno};
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
    /// 命中却不属本库的元素总数（= `foreign` 里元素的总数）。
    pub cross_library: usize,
    /// 命中却不属本库的元素，按属主库分桶：`dbnum → (打包 refno → 祖先链)`（U3 b 档）。不折根——根清单在那个库的
    /// `DbSet@T` 上；祖先链在这里带上（投影记录 / 库行 / 回库取），那个库的任务拿它与自己的根清单求交即可。
    /// 取不到祖先链的元素也留着（空链）：那个库折不出它就退整库，不在这里静默丢。
    pub foreign: BTreeMap<u32, BTreeMap<i64, Vec<i64>>>,
    /// 本库命中却折不出根的元素（`anc` ∪ 自身与根清单不交）：反查不完整的证据，调用方据此退整库。
    pub unresolved: Vec<RefnoEnum>,
}

impl AffectedRoots {
    pub(crate) fn summary(&self) -> String {
        format!(
            "roots={} tree_hits={} tube_hits={} cross_library={} foreign_libraries={} unresolved={}",
            self.roots.len(),
            self.tree_hits,
            self.tube_hits,
            self.cross_library,
            self.foreign.len(),
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

/// 树的答案之后的那一半：投影里的管身按盒扫（全 MDB）、库上管身范围查（不分库）、不在投影里的元素回库拿 `anc`、
/// 按属主库分桶、本库的折根。拆出来是为了不碰全局树也能对着 `mem://` 测。
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
    // 投影一把读锁过一遍、不分库、不复制：管身记录按盒扫（管身不进树）；命中的（管身 / 树上点到的）把祖先链带出来
    // ——别的库的元素也带，停放给那个库时随元素一起交过去，它折根不必再回库。
    let mut ancestors_of: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let mut tube_hits: BTreeSet<i64> = BTreeSet::new();
    store.for_each_record(ProjectionScope::Current, |record| {
        let source = packed_refno(record.source_refno);
        let tube_hit = record.tube.is_some() && {
            let aabb = Aabb::new(record.world_bounds[0].into(), record.world_bounds[1].into());
            boxes.iter().any(|query| query.intersects(&aabb))
        };
        if tube_hit {
            tube_hits.insert(source);
        }
        if tube_hit || tree_hits.contains(&source) {
            ancestors_of
                .entry(source)
                .or_insert_with(|| record.ancestors.clone());
        }
    });
    // 库上的管身（不在投影里的根的，以及已落库的）：范围过滤不带库，回来源与祖先链。
    for (source, anc) in durable_tube_hits(db, boxes).await? {
        tube_hits.insert(source);
        ancestors_of.entry(source).or_insert(anc);
    }
    out.tube_hits = tube_hits.len();
    // 树上命中、投影里没有的元素（重启前生成 / 初始化的，本库与别的库都有）：回库拿 `anc`。
    let unknown: Vec<i64> = tree_hits
        .iter()
        .copied()
        .filter(|source| !ancestors_of.contains_key(source))
        .collect();
    for (source, anc) in durable_ancestors(db, &unknown).await? {
        ancestors_of.insert(source, anc);
    }

    // 分库：本库的折根；别的库的按库存下（元素 + 祖先链）不折——根清单在那个库的 T 上，由它的任务折（U3 b 档）。
    let hits: BTreeSet<i64> = tree_hits.iter().chain(tube_hits.iter()).copied().collect();
    let mut pending: Vec<(i64, Vec<i64>)> = Vec::new();
    for source in hits {
        let anc = ancestors_of.get(&source).cloned().unwrap_or_default();
        let owner = e3d_of(source).dbno();
        if owner == dbnum {
            pending.push((source, anc));
        } else {
            out.foreign.entry(owner).or_default().insert(source, anc);
        }
    }
    out.cross_library = out.foreign.values().map(BTreeMap::len).sum();
    let roots: BTreeSet<i64> = library_roots
        .iter()
        .map(|root| packed_refno(*root))
        .collect();
    let (matched, unresolved) = fold_to_roots(pending, &roots);
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

/// 库上的隐式管身里、世界盒与任一查询盒相交的：来源元素 + 祖先链。**不分库**（面板在 A、管身在 B 是常态，
/// 按库过滤会把 B 的命中漏掉；分库在 [`resolve`] 里按 `RefNo::dbno()` 做）。O(全部管身行) 的链接解引用，不读几何；
/// 慢了给 `tubi_relate` 补 `aabb_d` 平副本，不在这里加库过滤。
pub(crate) async fn durable_tube_hits(
    db: &Surreal<Any>,
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
            "SELECT in AS refno, anc FROM tubi_relate WHERE aabb.d != NONE AND ({predicate});"
        ))
        .await?
        .check()?
        .take(0)?;
    Ok(rows_into_pairs(rows))
}

/// 停放给别的库的根级工作项（U3 b 档）：停放方任务的开工前对齐把常驻从 `from_generation` 换到 `to_generation`
/// （= 它重算那台计算器的代号），反查里命中的这个库的元素连同祖先链都在 `elements` 里。这个库的任务拿它与自己
/// `DbSet@T` 的根清单求交（[`fold_to_roots`]）就是要重算的根；折不出根的元素 → 反查不完整 → 退整库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoomRoots {
    pub from_generation: String,
    pub to_generation: String,
    /// 打包 refno → 祖先链（自身 → 顶层）。
    pub elements: BTreeMap<i64, Vec<i64>>,
    /// 停放方（dbnum）。同一段被并进来时保留第一个。
    pub parked_by: u32,
    /// 第一次停进来的时刻（`/health` 报年龄）。
    pub parked_at: Instant,
}

impl RoomRoots {
    pub(crate) fn summary(&self) -> String {
        format!(
            "{} 个元素（{} → {}，dbnum={} 停放）",
            self.elements.len(),
            short_generation(&self.from_generation),
            short_generation(&self.to_generation),
            self.parked_by
        )
    }
}

fn short_generation(generation: &str) -> String {
    generation.chars().take(12).collect()
}

/// 进程内的停放槽：`dbnum → 停放物（按停进来的先后）`。
static PARKED: RwLock<BTreeMap<u32, Vec<RoomRoots>>> = RwLock::new(BTreeMap::new());

fn parked_read() -> std::sync::RwLockReadGuard<'static, BTreeMap<u32, Vec<RoomRoots>>> {
    PARKED.read().unwrap_or_else(PoisonError::into_inner)
}

fn parked_write() -> std::sync::RwLockWriteGuard<'static, BTreeMap<u32, Vec<RoomRoots>>> {
    PARKED.write().unwrap_or_else(PoisonError::into_inner)
}

/// 往 `dbnum` 的槽里停一条。同 `from → to` 的并进已有那条（元素并集、保留最早的时刻与第一个停放方）——
/// 面板挪回去再挪回来会得到同一对代号，两次命中的是同一批元素。空条不停。回这个库槽里现在的条数。
pub(crate) fn park(dbnum: u32, entry: RoomRoots) -> usize {
    if entry.elements.is_empty() {
        return parked_read().get(&dbnum).map_or(0, Vec::len);
    }
    let mut slots = parked_write();
    let slot = slots.entry(dbnum).or_default();
    match slot.iter_mut().find(|parked| {
        parked.from_generation == entry.from_generation
            && parked.to_generation == entry.to_generation
    }) {
        Some(parked) => parked.elements.extend(entry.elements),
        None => slot.push(entry),
    }
    slot.len()
}

/// `dbnum` 槽里的全部停放物（按停进来的先后），不取走——消费成功才 [`clear`]，失败留着让下一个任务再接。
pub(crate) fn pending(dbnum: u32) -> Vec<RoomRoots> {
    parked_read().get(&dbnum).cloned().unwrap_or_default()
}

/// 清空 `dbnum` 的槽，回清掉的条数。只在这个库的房间重算**成功**之后调（定向 = 链上每一段都算齐；整库 = 覆盖一切），
/// 或它的库上代号已与常驻相等（停放物说的都是它已经越过的代）。失败不清——③ 兜底整库时再清。
pub(crate) fn clear(dbnum: u32) -> usize {
    parked_write().remove(&dbnum).map_or(0, |slot| slot.len())
}

/// 停放方任务的定向重算**结论定为 `Targeted` 之后**调：反查 `foreign` 的每一桶连同这一段换代（`from` = 它开工前对齐的
/// 上一代，`to` = 它重算那台计算器的代号）停进那个库的槽。回每个库一句（给日志）。退整库 / `Failed` 不调——那时反查
/// 说不了完整或本库自己都没算齐，别的库由 ③ 兜底整库。
pub(crate) fn park_foreign_hits(
    parked_by: u32,
    from_generation: &str,
    to_generation: &str,
    foreign: &BTreeMap<u32, BTreeMap<i64, Vec<i64>>>,
) -> Vec<String> {
    let now = Instant::now();
    foreign
        .iter()
        .filter(|(_, elements)| !elements.is_empty())
        .map(|(dbnum, elements)| {
            let entries = park(
                *dbnum,
                RoomRoots {
                    from_generation: from_generation.to_owned(),
                    to_generation: to_generation.to_owned(),
                    elements: elements.clone(),
                    parked_by,
                    parked_at: now,
                },
            );
            format!(
                "已为 dbnum={dbnum} 停放 {} 个元素（{} → {}），它的槽里现有 {entries} 段",
                elements.len(),
                short_generation(from_generation),
                short_generation(to_generation)
            )
        })
        .collect()
}

/// `/health.room_build.parked`：每个有停放物的库一格 `{ entries, elements, oldest_age_ms, segments: ["g1→g2 (dbnum=A)"] }`。
/// 空槽不出现（`{}` = 谁也不欠谁）。
pub(crate) fn parked_health() -> serde_json::Value {
    let slots = parked_read();
    serde_json::Value::Object(
        slots
            .iter()
            .filter(|(_, slot)| !slot.is_empty())
            .map(|(dbnum, slot)| {
                (
                    dbnum.to_string(),
                    serde_json::json!({
                        "entries": slot.len(),
                        "elements": slot.iter().map(|entry| entry.elements.len()).sum::<usize>(),
                        "oldest_age_ms": slot
                            .iter()
                            .map(|entry| entry.parked_at.elapsed().as_millis() as u64)
                            .max()
                            .unwrap_or(0),
                        "segments": slot
                            .iter()
                            .map(|entry| format!(
                                "{}→{} (dbnum={})",
                                short_generation(&entry.from_generation),
                                short_generation(&entry.to_generation),
                                entry.parked_by
                            ))
                            .collect::<Vec<_>>(),
                    }),
                )
            })
            .collect(),
    )
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
            dbnum: source.dbno(),
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
    /// 投影里没有的元素回库拿 `anc`；本库的折成根、折不出根的进 `unresolved`；**别的库的命中三路都进 `foreign`**
    /// ——按属主库分桶、带祖先链、不折根（U3 b 档）；取不到祖先链的也留着（空链）。
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
        // （根 D，盒 [50,51]，不该命中）；另一库（7998）的管身行 24382/9 盒相交——查询不分库，要进 `foreign`；
        // 另一库树上命中的 24382/8 不在投影里，回库取到 anc。
        db.query(format!(
            r#"
            CREATE aabb:near SET d={{mins:[5,5,5],maxs:[6,6,6]}};
            CREATE aabb:far SET d={{mins:[50,50,50],maxs:[51,51,51]}};
            CREATE inst_relate:⟨24381_300⟩ SET in=pe:⟨24381_300⟩, anc=[{}, {}], dbnum=7997;
            CREATE inst_relate:⟨24382_8⟩ SET in=pe:⟨24382_8⟩, anc=[{}, {}], dbnum=7998;
            CREATE tubi_relate:⟨t400⟩ SET in=pe:⟨24381_400⟩, anc=[{}], dbnum=7997, aabb=aabb:near;
            CREATE tubi_relate:⟨t500⟩ SET in=pe:⟨24381_500⟩, anc=[{}], dbnum=7997, aabb=aabb:far;
            CREATE tubi_relate:⟨t9⟩ SET in=pe:⟨24382_9⟩, anc=[{}], dbnum=7998, aabb=aabb:near;
            "#,
            packed(24381, 77),
            packed_refno(root_c),
            packed(24382, 88),
            packed(24382, 1),
            packed_refno(root_d),
            packed_refno(root_d),
            packed(24382, 1),
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        // 投影：根 A 的构件 100（盒 [0,1]）与根 B 的管身 200（盒 [4.5,5.5]，与查询盒 [5,6] 相交）；
        // 根 A 还有一段管身 101 在远处（不相交）；另一库（7998）根 24382/2 的管身 24382/10 也与 [5,6] 相交。
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
                    record(
                        "24382_10_tube",
                        RefNo::new(24382, 10),
                        RefNo::new(24382, 2),
                        (5.2, 5.8),
                        Some(tube(11, 12)),
                    ),
                ],
                ..ProjectionCommit::default()
            }],
        );
        // 树的答案（模拟）：投影里的 100、库上的 300、折不出根的 24381/600；别的库的 24382/8（库上有 anc）与
        // 24383/8（第三个库，哪里都没有它的祖先链）。
        let tree_hits = BTreeSet::from([
            packed(24381, 100),
            packed(24381, 300),
            packed(24382, 8),
            packed(24383, 8),
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
        assert_eq!(affected.tree_hits, 5);
        assert_eq!(
            affected.tube_hits, 4,
            "投影里的 200 与 24382/10 + 库上的 400 与 24382/9；远处的 101 / 500 不算——两路都不分库"
        );
        assert_eq!(
            affected.foreign,
            BTreeMap::from([
                (
                    7998,
                    BTreeMap::from([
                        (packed(24382, 8), vec![packed(24382, 88), packed(24382, 1)]),
                        (packed(24382, 9), vec![packed(24382, 1)]),
                        (packed(24382, 10), vec![packed(24382, 2)]),
                    ]),
                ),
                (7999, BTreeMap::from([(packed(24383, 8), Vec::new())])),
            ]),
            "别的库的命中按属主库分桶、带祖先链（树上的回库取、管身行自带、投影记录自带）；取不到的留空链"
        );
        assert_eq!(affected.cross_library, 4, "= foreign 里元素的总数");
        assert_eq!(affected.unresolved, vec![refno(24381, 600)]);
        assert_eq!(
            affected.summary(),
            "roots=4 tree_hits=5 tube_hits=4 cross_library=4 foreign_libraries=2 unresolved=1"
        );
    }

    fn parked(from: &str, to: &str, by: u32, elements: &[(u32, u32)]) -> RoomRoots {
        RoomRoots {
            from_generation: from.into(),
            to_generation: to.into(),
            elements: elements
                .iter()
                .map(|(word0, word1)| (packed(*word0, *word1), vec![packed(*word0, 1)]))
                .collect(),
            parked_by: by,
            parked_at: Instant::now(),
        }
    }

    /// 停放槽（U3 b 档）：按库分槽；同一段（同 `from → to`）并成一条（元素并集、保留第一个停放方与最早时刻）、
    /// 不同段各一条按先后排；空条不停；`pending` 不取走、`clear` 一次清空并回条数；别的库的槽不受影响；
    /// `/health` 那一格只列有停放物的库。
    #[test]
    fn the_parking_slot_merges_the_same_segment_and_clears_per_library() {
        // 别的用例（以及别的模块的 `mem://` 端到端）也会用槽——这里用只属本用例的 dbnum。
        let (b, c) = (9_001, 9_002);
        assert!(pending(b).is_empty());
        assert_eq!(park(b, parked("g1", "g2", 7_997, &[])), 0, "空条不停");
        assert_eq!(park(b, parked("g1", "g2", 7_997, &[(24_385, 8)])), 1);
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_eq!(
            park(b, parked("g1", "g2", 7_999, &[(24_385, 8), (24_385, 9)])),
            1,
            "同一段并进已有那条"
        );
        assert_eq!(park(b, parked("g2", "g3", 7_999, &[(24_385, 10)])), 2);
        assert_eq!(park(c, parked("g1", "g2", 7_997, &[(24_386, 8)])), 1);

        let slot = pending(b);
        assert_eq!(slot.len(), 2);
        assert_eq!(
            (
                slot[0].from_generation.as_str(),
                slot[0].to_generation.as_str(),
                slot[0].parked_by,
                slot[0].elements.keys().copied().collect::<Vec<_>>(),
            ),
            (
                "g1",
                "g2",
                7_997,
                vec![packed(24_385, 8), packed(24_385, 9)]
            ),
            "并集、第一个停放方"
        );
        assert!(
            slot[0].parked_at < slot[1].parked_at,
            "并进来不刷新时刻，先停的排前面"
        );
        assert_eq!(slot[0].summary(), "2 个元素（g1 → g2，dbnum=7997 停放）");
        assert_eq!(pending(b).len(), 2, "pending 不取走");

        let health = parked_health();
        let mine = &health[b.to_string()];
        assert_eq!(mine["entries"], 2);
        assert_eq!(mine["elements"], 3);
        assert!(mine["oldest_age_ms"].as_u64().is_some());
        assert_eq!(
            mine["segments"],
            serde_json::json!(["g1→g2 (dbnum=7997)", "g2→g3 (dbnum=7999)"])
        );
        assert_eq!(health[c.to_string()]["entries"], 1);

        assert_eq!(clear(b), 2);
        assert!(pending(b).is_empty());
        assert_eq!(clear(b), 0, "再清是 0，不报错");
        assert_eq!(pending(c).len(), 1, "别的库的槽不受影响");
        assert!(
            parked_health().get(b.to_string()).is_none(),
            "清空的槽不出现在 /health"
        );
        clear(c);
    }

    /// 源码钉（U3 b 档）：库上管身的范围查询**不带库过滤**——跨库命中要在这一层就进来，分库在 `resolve` 里按
    /// `RefNo::dbno()` 做。
    #[test]
    fn the_durable_tube_query_is_not_filtered_by_library() {
        let production = include_str!("room_reverse.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test module")
            .0;
        let query = production
            .split_once("pub(crate) async fn durable_tube_hits(")
            .expect("durable_tube_hits")
            .1;
        let query = &query[..query.find("\n}\n").expect("end")];
        assert!(
            query.contains("FROM tubi_relate WHERE aabb.d != NONE AND (")
                && !query.contains("dbnum ="),
            "{query}"
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
