//! `GET /api/v1/spatial/surface-clearance`（plant3d-web
//! `docs/plans/2026-09-17-component-to-wall-surface-clearance-plan.md` §4–§5，决策 d-428）：
//! 给两个构件（典型是「选中的构件 × 选中的墙」），用**两侧真实三角网格**算外表面到外表面的
//! 最近距离与两侧最近点；命中墙主面时再沿命中三角的法向补一条**垂距**射线。
//!
//! **与 [`super::spatial_clearance`] 的分工**：那条是 AABB / 中心线粗算、一对多、走
//! `GLOBAL_AABB_TREE`；这条是一对一精算、走磁盘网格，不碰空间树、不取空间串行锁。
//!
//! **网格从哪来**：只读磁盘（用户 2026-09-17 拍板「mesh 都是提前生成的」）——
//! `inst_relate` 行的 `insts_flat[{geo_hash, transform}]` × `world_trans`，或布尔后整块
//! `{booled_id}.mesh` × `world_trans`；隐式管身在 `tubi_relate`（`out` 就是网格键）。叶子集合
//! 用 `anc CONTAINS <packed refno>` 一次取齐（自身 + 全部后代，`idx_*_anc` 索引）。
//! **不走** `tessellate_libgm_param`：缺网格文件的叶子记 warning 跳过，一个都没有 → `NoModelMesh`。
//! 源与目标有祖先关系时两侧叶子会重合（FIXING × 它所在的 WALL），重合叶子从祖先那一侧剔掉再算
//! （[`split_shared_leaves`]），否则自对必回 0；剔完一侧空了 → `SharedLeavesOnly`（422）。
//!
//! **算法**（§4.2–§4.4）：叶子对按 AABB 距离升序，`aabb_distance >= best` 即停；每对调
//! parry `closest_points(max_dist = best)`（TriMesh × TriMesh 走 composite 嵌套 BVH，精确三角对）；
//! 命中三角法向给面分类（顶 / 底 / 端 / 侧；弧墙内 / 外），再从源侧最近点沿 −n 打 `cast_ray`
//! 得垂距。顶点是 f32：世界 mm 量级 1e5 下 ~1e-2 mm 误差，低于弦高容差 0.5 mm。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use aios_core::RefU64;
use aios_core::shape::pdms_shape::PlantMesh;
use anyhow::{Context, Result};
use dashmap::DashMap;
use glam::{Mat4, Quat, Vec3};
use once_cell::sync::Lazy;
use parry3d::bounding_volume::Aabb;
use parry3d::math::{Isometry, Point, Real, Vector};
use parry3d::query::{self, ClosestPoints, PointQuery, Ray, RayCast};
use parry3d::shape::{FeatureId, TriMesh, TriMeshFlags};
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use crate::fast_model::spatial_query::{aabb_min_distance, parse_f32, parse_flag};

/// 响应里 `unit` / `method` / `accuracy_class` 的字面值（09-11 计划 D2 的命名）。
pub const UNIT: &str = "mm";
pub const METHOD: &str = "surface_to_surface";
pub const ACCURACY_EXACT_SURFACE: &str = "exact-surface";
/// 曲面离散的弦高容差（`aios_core::prim_geo::libgm_discretise::FACET_TOL_MM`，全库唯一一份）：
/// 网格与真实曲面的最大偏差，直接作为结果的误差上界报出去。直墙也照报——一期不判目标是否含曲面。
pub const ERROR_BOUND_MM: f32 = 0.5;
/// `max_distance_mm` 缺省与上限。
pub const DEFAULT_MAX_DISTANCE_MM: f32 = 50_000.0;
pub const MAX_MAX_DISTANCE_MM: f32 = 1_000_000.0;
/// 墙族：CWALL 是 owner，其余四个与 `/nearest-clearance` 的预置组 `wall` 逐字相同。
pub const WALL_NOUNS: [&str; 5] = ["CWALL", "WALL", "PANE", "GWALL", "STWALL"];

/// 单位网格缓存上限（按 geo_hash 内容寻址，永不过期；超过就整体清空，下次再热）。
const UNIT_MESH_CACHE_CAP: usize = 8_192;
/// 世界网格缓存上限（键 = 叶子行 key + model_sesno + 摆放指纹：换版换 sesno，`model/ensure force=true`
/// 重算不换 sesno 只换摆放——两种都要自然失效）。
const LEAF_MESH_CACHE_CAP: usize = 1_024;

pub fn is_wall_noun(noun: &str) -> bool {
    let upper = noun.trim().to_ascii_uppercase();
    WALL_NOUNS.iter().any(|w| *w == upper)
}

// ───────────────────────────── 参数 ─────────────────────────────

/// query 串原文：全部按字串收，理由同 [`super::spatial_query::RawSpatialQuery`]。
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct RawSurfaceClearanceQuery {
    pub source_refno: Option<String>,
    pub target_refno: Option<String>,
    /// `wall`（缺省）：目标必须是墙族，否则 422；`any`：任何有网格的构件都收（抽屉复用）。
    pub target_kind: Option<String>,
    /// `1`（缺省）：命中墙主面时附垂距射线。
    pub perpendicular: Option<String>,
    /// mm；缺省 50000，上限 1e6。超出即 `result: null` + warning。
    pub max_distance_mm: Option<String>,
    pub debug: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Wall,
    Any,
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wall => "wall",
            Self::Any => "any",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SurfaceClearanceParams {
    pub source: RefU64,
    pub target: RefU64,
    pub target_kind: TargetKind,
    pub perpendicular: bool,
    pub max_distance_mm: f32,
    pub debug: bool,
}

fn parse_refno(name: &str, raw: Option<&str>) -> Result<RefU64, String> {
    let raw = raw
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("{name} 必填：构件 refno（a_b / a/b）"))?;
    raw.parse::<RefU64>()
        .ok()
        .filter(RefU64::is_valid)
        .ok_or_else(|| format!("{name} 格式非法（应为 a/b 或 a_b）: {raw}"))
}

impl SurfaceClearanceParams {
    /// 解析并校验 query 串。错误信息直接作为 400 `bad_request` 的 `message`。
    pub fn parse(raw: &RawSurfaceClearanceQuery) -> Result<Self, String> {
        let source = parse_refno("source_refno", raw.source_refno.as_deref())?;
        let target = parse_refno("target_refno", raw.target_refno.as_deref())?;
        if source == target {
            return Err("source_refno 与 target_refno 不能是同一个构件".to_string());
        }
        let target_kind = match raw
            .target_kind
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("wall") => TargetKind::Wall,
            Some("any") => TargetKind::Any,
            Some(other) => return Err(format!("target_kind 只接受 wall/any（收到 {other}）")),
        };
        let perpendicular = parse_flag("perpendicular", raw.perpendicular.as_deref(), true)?;
        let max_distance_mm = match raw.max_distance_mm.as_deref().map(str::trim) {
            None | Some("") => DEFAULT_MAX_DISTANCE_MM,
            Some(text) => {
                let value = parse_f32("max_distance_mm", text)?;
                if value <= 0.0 || value > MAX_MAX_DISTANCE_MM {
                    return Err(format!(
                        "max_distance_mm 必须在 (0, {MAX_MAX_DISTANCE_MM}] 内（收到 {text}）"
                    ));
                }
                value
            }
        };
        let debug = parse_flag("debug", raw.debug.as_deref(), false)?;
        Ok(Self {
            source,
            target,
            target_kind,
            perpendicular,
            max_distance_mm,
            debug,
        })
    }
}

// ───────────────────────────── 几何内核（不碰库） ─────────────────────────────

/// 一片参与计算的叶子网格（世界 mm）。
pub struct LeafMesh {
    /// `a/b`；隐式管身是它所属 BRAN 的 refno（`tubi_relate.in`）。
    pub refno: String,
    /// 行 key（`inst_relate` 的 record id，或 `tubi_relate:[pe:⟨…⟩, n]` 原文），只进 debug。
    pub key: String,
    pub noun: String,
    pub mesh: Arc<TriMesh>,
    pub model_sesno: Option<i64>,
}

impl LeafMesh {
    pub fn aabb(&self) -> &Aabb {
        self.mesh.local_aabb()
    }

    pub fn triangle_count(&self) -> usize {
        self.mesh.num_triangles()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceClearanceOptions {
    pub perpendicular: bool,
    pub max_distance: f32,
}

/// 命中墙面的分类。直墙两侧对称，只分得出 `side`；弧墙分内（凹）/ 外（凸）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FaceKind {
    Inner,
    Outer,
    Side,
    Top,
    Bottom,
    End,
    Unknown,
}

impl FaceKind {
    /// 主面：可以从源侧最近点沿法向打垂距的那几种面。
    fn is_main_face(self) -> bool {
        matches!(self, Self::Inner | Self::Outer | Self::Side)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FaceInfo {
    pub kind: FaceKind,
    /// 命中三角的法向，已翻成指向源侧。
    pub normal: Vector<Real>,
    /// `pca`：按墙的水平主轴分端 / 侧；`geometric`：按拟合出的弧轴分内 / 外；`normal-only`：只有法向。
    pub confidence: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct Perpendicular {
    pub distance: f32,
    pub from: Point<Real>,
    pub to: Point<Real>,
}

#[derive(Debug, Clone)]
pub struct ClearanceHit {
    pub distance: f32,
    pub intersects: bool,
    pub source_point: Point<Real>,
    pub target_point: Point<Real>,
    pub source_leaf: usize,
    pub target_leaf: usize,
    pub target_face: Option<FaceInfo>,
    pub perpendicular: Option<Perpendicular>,
    /// `closest-points`：两点是 parry 给的精确最近点对；`aabb-overlap-center`：相交时 parry 不给点，
    /// 取两叶子 AABB 交集中心占位。
    pub witness: &'static str,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PairStats {
    pub total: usize,
    pub evaluated: usize,
    pub pruned: usize,
}

pub struct ClearanceOutcome {
    pub hit: Option<ClearanceHit>,
    pub stats: PairStats,
    pub warnings: Vec<String>,
}

/// 两组叶子网格之间的外表面最近距离（§4.2–§4.4）。纯几何、同步、可单测。
pub fn surface_clearance(
    source: &[LeafMesh],
    target: &[LeafMesh],
    opts: SurfaceClearanceOptions,
) -> ClearanceOutcome {
    let mut warnings = Vec::new();
    let mut stats = PairStats::default();
    if source.is_empty() || target.is_empty() {
        return ClearanceOutcome {
            hit: None,
            stats,
            warnings,
        };
    }

    // 叶子对按 AABB 下界升序；下界 ≥ 当前最优就整体停下。
    let mut pairs: Vec<(f32, usize, usize)> = Vec::with_capacity(source.len() * target.len());
    for (i, s) in source.iter().enumerate() {
        for (j, t) in target.iter().enumerate() {
            pairs.push((aabb_min_distance(s.aabb(), t.aabb()), i, j));
        }
    }
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    stats.total = pairs.len();

    let identity = Isometry::identity();
    let mut best: Option<ClearanceHit> = None;
    let mut best_distance = opts.max_distance;
    for (lower_bound, i, j) in pairs {
        if lower_bound >= best_distance {
            stats.pruned += 1;
            continue;
        }
        stats.evaluated += 1;
        let s = &source[i];
        let t = &target[j];
        // margin 一律给无穷大：这版 parry 的 composite × composite 在「外层三角进了 margin、内层
        // 一个三角都不在 margin 内」时不回 Disjoint 而是 panic（`closest_points_composite_shape_shape.rs`
        // 27 的 expect）。剪枝由上面的 AABB 下界做，这里只要精确值。panic 再兜一层，当 Disjoint 记 warning。
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            query::closest_points(&identity, &*s.mesh, &identity, &*t.mesh, Real::MAX)
        }));
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(_) => {
                warnings.push(format!(
                    "closest_points 在源叶子 {} × 目标叶子 {} 上 panic，已按 Disjoint 跳过",
                    s.refno, t.refno
                ));
                continue;
            }
        };
        match outcome {
            Ok(ClosestPoints::Intersecting) => {
                let overlap_center = s
                    .aabb()
                    .intersection(t.aabb())
                    .map(|box_| box_.center())
                    .unwrap_or_else(|| s.aabb().center());
                return ClearanceOutcome {
                    hit: Some(ClearanceHit {
                        distance: 0.0,
                        intersects: true,
                        source_point: overlap_center,
                        target_point: overlap_center,
                        source_leaf: i,
                        target_leaf: j,
                        target_face: None,
                        perpendicular: None,
                        witness: "aabb-overlap-center",
                    }),
                    stats,
                    warnings,
                };
            }
            Ok(ClosestPoints::WithinMargin(p, q)) => {
                let distance = (p - q).norm();
                if distance < best_distance {
                    best_distance = distance;
                    best = Some(ClearanceHit {
                        distance,
                        intersects: false,
                        source_point: p,
                        target_point: q,
                        source_leaf: i,
                        target_leaf: j,
                        target_face: None,
                        perpendicular: None,
                        witness: "closest-points",
                    });
                }
            }
            Ok(ClosestPoints::Disjoint) => {}
            Err(_) => warnings.push(format!(
                "closest_points 不支持这对形状（源叶子 {} × 目标叶子 {}），已跳过",
                s.refno, t.refno
            )),
        }
    }

    let Some(mut hit) = best else {
        return ClearanceOutcome {
            hit: None,
            stats,
            warnings,
        };
    };

    let target_leaf = &target[hit.target_leaf];
    let face = classify_target_face(target_leaf, hit.target_point, hit.source_point);
    if opts.perpendicular && face.kind.is_main_face() {
        match perpendicular_ray(&target_leaf.mesh, hit.source_point, face.normal, hit.distance) {
            Some(perpendicular) => hit.perpendicular = Some(perpendicular),
            None => warnings.push(
                "perpendicular_ray_missed: 源侧最近点沿墙面法向的射线没有落回墙面，最近点在墙的棱 / 角上，垂距置空"
                    .to_string(),
            ),
        }
    }
    hit.target_face = Some(face);

    ClearanceOutcome {
        hit: Some(hit),
        stats,
        warnings,
    }
}

fn unit_or_none(v: Vector<Real>) -> Option<Vector<Real>> {
    let n = v.norm();
    (n > 1e-9).then(|| v / n)
}

/// 命中点所在三角的法向，翻成指向源侧；取不到三角时退到「源点 − 命中点」方向。
fn hit_normal(mesh: &TriMesh, q: Point<Real>, p: Point<Real>) -> (Vector<Real>, &'static str) {
    let toward_source = unit_or_none(p - q);
    let (_, feature) = mesh.project_local_point_and_get_feature(&q);
    let tri_normal = match feature {
        FeatureId::Face(index) if (index as usize) < mesh.num_triangles() => {
            mesh.triangle(index).normal().map(|n| n.into_inner())
        }
        _ => None,
    };
    match (tri_normal, toward_source) {
        (Some(n), Some(dir)) => (if n.dot(&dir) < 0.0 { -n } else { n }, "triangle"),
        (Some(n), None) => (n, "triangle"),
        (None, Some(dir)) => (dir, "normal-only"),
        (None, None) => (Vector::z(), "normal-only"),
    }
}

/// 一片墙的水平骨架：主轴（行进方向）、厚度方向、是否弧墙及弧轴（xy）。
#[derive(Debug, Clone, Copy)]
struct WallFrame {
    running: Vector<Real>,
    thickness: Vector<Real>,
    arc_center_xy: Option<(f32, f32)>,
}

fn horizontal_unit(v: Vector<Real>) -> Option<Vector<Real>> {
    unit_or_none(Vector::new(v.x, v.y, 0.0))
}

fn wall_frame(mesh: &TriMesh) -> WallFrame {
    // 水平 PCA：顶点 xy 协方差的主方向 = 行进方向。
    let verts = mesh.vertices();
    let n = verts.len().max(1) as f64;
    let (mut mx, mut my) = (0.0f64, 0.0f64);
    for v in verts {
        mx += v.x as f64;
        my += v.y as f64;
    }
    mx /= n;
    my /= n;
    let (mut sxx, mut sxy, mut syy) = (0.0f64, 0.0f64, 0.0f64);
    for v in verts {
        let dx = v.x as f64 - mx;
        let dy = v.y as f64 - my;
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let running = Vector::new(theta.cos() as f32, theta.sin() as f32, 0.0);
    let thickness = Vector::new(-running.y, running.x, 0.0);

    // 侧面法向里偏离 ±running / ±thickness 的面积占比过大 → 弧墙；随手用这些法向线拟合弧轴。
    let mut side_area = 0.0f64;
    let mut off_axis_area = 0.0f64;
    let (mut a11, mut a12, mut a22) = (0.0f64, 0.0f64, 0.0f64);
    let (mut b1, mut b2) = (0.0f64, 0.0f64);
    for tri in mesh.triangles() {
        let Some(normal) = tri.normal() else { continue };
        if normal.z.abs() >= 0.5 {
            continue;
        }
        let Some(h) = horizontal_unit(normal.into_inner()) else {
            continue;
        };
        let area = tri.area() as f64;
        side_area += area;
        let align = h.dot(&running).abs().max(h.dot(&thickness).abs());
        if align < 0.966 {
            off_axis_area += area;
        }
        // 最小化 Σ w |(I − h hᵀ)(c − q)|²：法向线 q + λh 都过弧轴。
        let (hx, hy) = (h.x as f64, h.y as f64);
        let c = tri.center();
        let (qx, qy) = (c.x as f64, c.y as f64);
        let (p11, p12, p22) = (1.0 - hx * hx, -hx * hy, 1.0 - hy * hy);
        a11 += area * p11;
        a12 += area * p12;
        a22 += area * p22;
        b1 += area * (p11 * qx + p12 * qy);
        b2 += area * (p12 * qx + p22 * qy);
    }
    let curved = side_area > 0.0 && off_axis_area / side_area > 0.25;
    let det = a11 * a22 - a12 * a12;
    let arc_center_xy = (curved && det.abs() > 1e-6 * (a11 * a22).abs().max(1e-12)).then(|| {
        let cx = (a22 * b1 - a12 * b2) / det;
        let cy = (a11 * b2 - a12 * b1) / det;
        (cx as f32, cy as f32)
    });
    WallFrame {
        running,
        thickness,
        arc_center_xy,
    }
}

/// 命中墙面分类（§4.3）。世界 Up = +Z（E3D 的 U）。
fn classify_target_face(target: &LeafMesh, q: Point<Real>, p: Point<Real>) -> FaceInfo {
    let (normal, normal_source) = hit_normal(&target.mesh, q, p);
    if normal.z.abs() >= 0.7 {
        return FaceInfo {
            kind: if normal.z > 0.0 {
                FaceKind::Top
            } else {
                FaceKind::Bottom
            },
            normal,
            confidence: "normal-only",
        };
    }
    let Some(h) = horizontal_unit(normal) else {
        return FaceInfo {
            kind: FaceKind::Unknown,
            normal,
            confidence: normal_source,
        };
    };
    let frame = wall_frame(&target.mesh);
    if let Some((cx, cy)) = frame.arc_center_xy {
        let Some(radial) = horizontal_unit(Vector::new(cx - q.x, cy - q.y, 0.0)) else {
            return FaceInfo {
                kind: FaceKind::Unknown,
                normal,
                confidence: "geometric",
            };
        };
        let along = h.dot(&radial);
        let kind = if along.abs() >= 0.707 {
            // 外法向指向弧轴 = 这面朝着圆心 = 凹的内侧。
            if along > 0.0 {
                FaceKind::Inner
            } else {
                FaceKind::Outer
            }
        } else {
            FaceKind::End
        };
        return FaceInfo {
            kind,
            normal,
            confidence: "geometric",
        };
    }
    let kind = if h.dot(&frame.running).abs() >= 0.707 {
        FaceKind::End
    } else {
        FaceKind::Side
    };
    FaceInfo {
        kind,
        normal,
        confidence: "pca",
    }
}

/// 从源侧最近点沿 −n 打到墙面（§4.4）。落点与最近距离对不上（差 > 1% 且 > 0.05 mm）视为落空。
fn perpendicular_ray(
    wall: &TriMesh,
    from: Point<Real>,
    normal: Vector<Real>,
    distance: f32,
) -> Option<Perpendicular> {
    let dir = unit_or_none(-normal)?;
    let ray = Ray::new(from, dir);
    let hit = wall.cast_local_ray_and_get_normal(&ray, distance * 4.0 + 1.0, false)?;
    let toi = hit.time_of_impact;
    let tolerance = (distance * 0.01).max(0.05);
    if (toi - distance).abs() > tolerance {
        return None;
    }
    Some(Perpendicular {
        distance: toi,
        from,
        to: from + dir * toi,
    })
}

// ───────────────────────────── 叶子加载（库 + 磁盘） ─────────────────────────────

/// 一片叶子的库行（`inst_relate` 或 `tubi_relate`），只存拼网格要用的那几格。
#[derive(Debug, Clone)]
pub struct LeafRow {
    pub key: String,
    /// `a_b`
    pub refno: String,
    pub noun: String,
    pub world_trans: Option<serde_json::Value>,
    /// `(geo_hash, 本地变换)`；布尔后的行与隐式管身只有一项、无本地变换。
    pub insts: Vec<(String, Option<serde_json::Value>)>,
    pub model_sesno: Option<i64>,
    /// `world_trans` + `insts` 的指纹（[`placement_fingerprint`]）。`model/ensure force=true` 重算一个根
    /// 不换源会话号，`model_sesno` 分不出重算前后的摆放——2026-09-18 `:8022` 上 FIXING `17496/137183`
    /// 重算后 `world_trans` 已对、同进程仍从缓存拿到旧网格；世界网格缓存键里要带上它。
    pub placement_fingerprint: u64,
}

/// 一行叶子「拼网格要用的那几格」的指纹：`world_trans` 与每个实例的 `geo_hash` / 本地变换。
/// 只在进程内当缓存键用，不落库、不进响应，所以用标准库的 hasher 就够。
fn placement_fingerprint(
    world_trans: Option<&serde_json::Value>,
    insts: &[(String, Option<serde_json::Value>)],
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    world_trans.map(|v| v.to_string()).hash(&mut hasher);
    insts.len().hash(&mut hasher);
    for (geo_hash, local) in insts {
        geo_hash.hash(&mut hasher);
        local.as_ref().map(|v| v.to_string()).hash(&mut hasher);
    }
    hasher.finish()
}

fn json_str(v: &serde_json::Value) -> Option<String> {
    v.as_str()
        .map(|s| s.trim().trim_matches(|c| c == '⟨' || c == '⟩').to_string())
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("none"))
}

fn json_i64(v: Option<&serde_json::Value>) -> Option<i64> {
    v.and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
}

fn json_string_any(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) if !other.is_null() => other.to_string(),
        _ => String::new(),
    }
}

/// `pe:⟨a_b⟩` / `pe:a_b` / `a_b` → `a_b`。
fn refno_text(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("pe:")
        .trim_matches(|c| c == '⟨' || c == '⟩')
        .to_string()
}

fn row_to_leaf(row: &serde_json::Value, tube: bool) -> Option<LeafRow> {
    let key = json_string_any(row.get("key"));
    let refno = refno_text(&json_string_any(row.get("refno")));
    if refno.is_empty() {
        return None;
    }
    let noun = if tube {
        "TUBI".to_string()
    } else {
        json_string_any(row.get("noun")).trim().to_ascii_uppercase()
    };
    let world_trans = row.get("wt").cloned().filter(|v| v.is_object());
    let model_sesno = json_i64(row.get("model_sesno"));
    let mut insts = Vec::new();
    if tube {
        if let Some(hash) = row.get("geo_hash").and_then(json_str) {
            insts.push((hash, None));
        }
    } else if let Some(booled) = row.get("booled_id").and_then(json_str) {
        insts.push((booled, None));
    } else if let Some(list) = row.get("insts_flat").and_then(|v| v.as_array()) {
        for inst in list {
            let Some(hash) = inst.get("geo_hash").and_then(json_str) else {
                continue;
            };
            let transform = inst.get("transform").cloned().filter(|v| v.is_object());
            insts.push((hash, transform));
        }
    }
    let placement_fingerprint = placement_fingerprint(world_trans.as_ref(), &insts);
    Some(LeafRow {
        key,
        refno,
        noun,
        world_trans,
        insts,
        model_sesno,
        placement_fingerprint,
    })
}

/// 打包成 `anc` 数组里的那个整数（与 `model_memory_store::packed_refno` 同一口径）。
fn packed(refno: RefU64) -> i64 {
    refno.0 as i64
}

/// 一个构件（含全部后代）在模型面上的叶子行：`inst_relate` + `tubi_relate` 各一句 `anc CONTAINS`，
/// 旧数据没有 `anc` 时再按自身 id 补一行。
pub async fn fetch_leaf_rows(db: &Surreal<Any>, refno: RefU64) -> Result<Vec<LeafRow>> {
    let packed = packed(refno);
    let self_key = refno.to_string();
    let sql = format!(
        "SELECT record::id(id) AS key, type::string(in) AS refno, generic AS noun, \
                world_trans.d AS wt, insts_flat, booled_id, model_sesno \
         FROM inst_relate WHERE anc CONTAINS {packed} AND model_sesno != NONE;\n\
         SELECT type::string(id) AS key, type::string(in) AS refno, record::id(out) AS geo_hash, \
                world_trans.d AS wt, model_sesno \
         FROM tubi_relate WHERE anc CONTAINS {packed} AND model_sesno != NONE;\n\
         SELECT record::id(id) AS key, type::string(in) AS refno, generic AS noun, \
                world_trans.d AS wt, insts_flat, booled_id, model_sesno \
         FROM type::thing('inst_relate', '{self_key}');"
    );
    let mut response = db
        .query(sql)
        .await
        .with_context(|| format!("查询 {refno} 的模型面叶子行失败"))?;
    let insts: Vec<serde_json::Value> = response.take(0).context("解析 inst_relate 叶子行失败")?;
    let tubes: Vec<serde_json::Value> = response.take(1).context("解析 tubi_relate 叶子行失败")?;
    let own: Vec<serde_json::Value> = response.take(2).context("解析自身 inst_relate 行失败")?;

    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    for (value, tube) in insts
        .iter()
        .map(|v| (v, false))
        .chain(tubes.iter().map(|v| (v, true)))
        .chain(own.iter().map(|v| (v, false)))
    {
        let Some(row) = row_to_leaf(value, tube) else {
            continue;
        };
        if row.insts.is_empty() {
            continue;
        }
        if seen.insert(leaf_identity(&row)) {
            rows.push(row);
        }
    }
    Ok(rows)
}

/// 一片叶子行的身份：有 record id 用 record id，旧数据没有时退到「refno + 首个网格键」。
/// `fetch_leaf_rows` 去重与两侧重合判定用同一把尺。
fn leaf_identity(row: &LeafRow) -> String {
    if row.key.is_empty() {
        format!("{}:{}", row.refno, row.insts[0].0)
    } else {
        row.key.clone()
    }
}

/// 两侧叶子行有重合时的处置记录（进 warnings / debug）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedLeafReport {
    /// 重合叶子从哪一侧剔掉了：祖先那一侧。
    pub trimmed_side: TrimmedSide,
    /// 剔掉的叶子数。
    pub removed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimmedSide {
    Source,
    Target,
}

impl TrimmedSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Target => "target",
        }
    }
}

/// 两侧叶子都按 `anc CONTAINS` 取，源若是目标的后代（门框 FIXING × 它所在的 WALL），目标集合会含源
/// 自身的行，自对必然回 0 / intersects、把真值盖掉；反过来（源是 owner、目标是它名下的一件）同理。
/// 处置：重合的叶子从**祖先那一侧**剔掉——问「门框到墙」得到的是门框到墙其余部分的距离，问「BRAN 到
/// 自己的弯头」得到的是 BRAN 其余部分到弯头的距离。源全部落在目标里 → 剔目标；目标全部落在源里 →
/// 剔源；两个都不成立的部分重合按 `anc` 的语义不该出现，一律当「源在目标里」剔目标。没有重合原样返回。
pub fn split_shared_leaves(
    source_rows: Vec<LeafRow>,
    target_rows: Vec<LeafRow>,
) -> (Vec<LeafRow>, Vec<LeafRow>, Option<SharedLeafReport>) {
    let source_ids: HashSet<String> = source_rows.iter().map(leaf_identity).collect();
    let target_ids: HashSet<String> = target_rows.iter().map(leaf_identity).collect();
    let shared: HashSet<String> = source_ids.intersection(&target_ids).cloned().collect();
    if shared.is_empty() {
        return (source_rows, target_rows, None);
    }
    let target_within_source =
        target_ids.is_subset(&source_ids) && !source_ids.is_subset(&target_ids);
    let trimmed_side = if target_within_source {
        TrimmedSide::Source
    } else {
        TrimmedSide::Target
    };
    let removed = shared.len();
    let keep = |row: &LeafRow| !shared.contains(&leaf_identity(row));
    let (source_rows, target_rows) = match trimmed_side {
        TrimmedSide::Source => (source_rows.into_iter().filter(keep).collect(), target_rows),
        TrimmedSide::Target => (source_rows, target_rows.into_iter().filter(keep).collect()),
    };
    (
        source_rows,
        target_rows,
        Some(SharedLeafReport {
            trimmed_side,
            removed,
        }),
    )
}

/// 剔除重合叶子那一下写进响应 `warnings` 的那句（`code: 说明` 形状，与其它 warning 同）。
pub fn shared_leaf_warning(report: SharedLeafReport) -> String {
    match report.trimmed_side {
        TrimmedSide::Target => format!(
            "source_within_target: 源是目标的一部分（目标 {} 片叶子与源重合，已从目标侧剔除），结果是源到目标其余部分的距离",
            report.removed
        ),
        TrimmedSide::Source => format!(
            "target_within_source: 目标是源的一部分（源 {} 片叶子与目标重合，已从源侧剔除），结果是源其余部分到目标的距离",
            report.removed
        ),
    }
}

fn f_at(v: &serde_json::Value, idx: usize) -> f32 {
    v.get(idx).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32
}

/// `{translation:[3], rotation:[x,y,z,w], scale:[3]}` → `Mat4`；缺 `scale` 当 1，缺整个对象当单位阵。
fn mat_from_trans(t: Option<&serde_json::Value>) -> Mat4 {
    let Some(t) = t else { return Mat4::IDENTITY };
    let tr = t.get("translation").cloned().unwrap_or_default();
    let ro = t.get("rotation").cloned().unwrap_or_default();
    let sc = t.get("scale").cloned().unwrap_or_default();
    let translation = Vec3::new(f_at(&tr, 0), f_at(&tr, 1), f_at(&tr, 2));
    let rotation = if ro.is_array() {
        Quat::from_xyzw(f_at(&ro, 0), f_at(&ro, 1), f_at(&ro, 2), f_at(&ro, 3)).normalize()
    } else {
        Quat::IDENTITY
    };
    let scale = if sc.is_array() {
        Vec3::new(f_at(&sc, 0), f_at(&sc, 1), f_at(&sc, 2))
    } else {
        Vec3::ONE
    };
    Mat4::from_scale_rotation_translation(scale, rotation, translation)
}

static UNIT_MESHES: Lazy<DashMap<String, Arc<PlantMesh>>> = Lazy::new(DashMap::new);
/// 键 = (叶子行 key, model_sesno, 摆放指纹)。见 [`LeafRow::placement_fingerprint`]。
static LEAF_MESHES: Lazy<DashMap<(String, i64, u64), Arc<TriMesh>>> = Lazy::new(DashMap::new);

/// 世界网格缓存键：没有 `model_sesno`（老数据）或 key 为空的行不进缓存。
fn leaf_cache_key(row: &LeafRow) -> Option<(String, i64, u64)> {
    if row.key.is_empty() {
        return None;
    }
    row.model_sesno
        .map(|sesno| (row.key.clone(), sesno, row.placement_fingerprint))
}

fn load_unit_mesh(meshes_dir: &Path, geo_hash: &str) -> Result<Arc<PlantMesh>> {
    if let Some(hit) = UNIT_MESHES.get(geo_hash) {
        return Ok(hit.clone());
    }
    let path: PathBuf = meshes_dir.join(format!("{geo_hash}.mesh"));
    let mesh = PlantMesh::des_mesh_file(&path)
        .with_context(|| format!("读取网格 {} 失败", path.display()))?;
    let mesh = Arc::new(mesh);
    if UNIT_MESHES.len() >= UNIT_MESH_CACHE_CAP {
        UNIT_MESHES.clear();
    }
    UNIT_MESHES.insert(geo_hash.to_string(), mesh.clone());
    Ok(mesh)
}

/// 把一行叶子拼成世界 mm 的 `TriMesh`：每个实例 = 单位网格 × (world_trans × 本地变换)。
fn build_leaf_mesh(
    row: &LeafRow,
    meshes_dir: &Path,
    warnings: &mut Vec<String>,
) -> Option<Arc<TriMesh>> {
    let cache_key = leaf_cache_key(row);
    if let Some(key) = cache_key.as_ref()
        && let Some(hit) = LEAF_MESHES.get(key)
    {
        return Some(hit.clone());
    }
    let world = mat_from_trans(row.world_trans.as_ref());
    let mut verts: Vec<Point<Real>> = Vec::new();
    let mut idx: Vec<[u32; 3]> = Vec::new();
    for (geo_hash, local) in &row.insts {
        let unit = match load_unit_mesh(meshes_dir, geo_hash) {
            Ok(mesh) => mesh,
            Err(error) => {
                warnings.push(format!(
                    "叶子 {} 的网格 {geo_hash} 读不到，已跳过: {error:#}",
                    row.refno
                ));
                continue;
            }
        };
        if unit.indices.len() < 3 || unit.vertices.is_empty() {
            continue;
        }
        let matrix = world * mat_from_trans(local.as_ref());
        let base = verts.len() as u32;
        verts.extend(unit.vertices.iter().map(|v| {
            let w = matrix.transform_point3(*v);
            Point::new(w.x, w.y, w.z)
        }));
        idx.extend(
            unit.indices
                .chunks_exact(3)
                .map(|tri| [base + tri[0], base + tri[1], base + tri[2]]),
        );
    }
    if idx.is_empty() || verts.len() < 3 {
        return None;
    }
    let mesh = Arc::new(TriMesh::with_flags(verts, idx, TriMeshFlags::empty()));
    if let Some(key) = cache_key {
        if LEAF_MESHES.len() >= LEAF_MESH_CACHE_CAP {
            LEAF_MESHES.clear();
        }
        LEAF_MESHES.insert(key, mesh.clone());
    }
    Some(mesh)
}

/// 叶子行 → 叶子网格（同步、会读盘；调用方放进 `spawn_blocking`）。
pub fn build_leaf_meshes(
    rows: &[LeafRow],
    meshes_dir: &Path,
    warnings: &mut Vec<String>,
) -> Vec<LeafMesh> {
    rows.iter()
        .filter_map(|row| {
            let mesh = build_leaf_mesh(row, meshes_dir, warnings)?;
            Some(LeafMesh {
                refno: row.refno.replace('_', "/"),
                key: row.key.clone(),
                noun: row.noun.clone(),
                mesh,
                model_sesno: row.model_sesno,
            })
        })
        .collect()
}

// ───────────────────────────── 响应 ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PointJson {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl From<Point<Real>> for PointJson {
    fn from(p: Point<Real>) -> Self {
        Self {
            x: p.x,
            y: p.y,
            z: p.z,
        }
    }
}

impl From<Vector<Real>> for PointJson {
    fn from(v: Vector<Real>) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DeltaJson {
    pub dx: f32,
    pub dy: f32,
    pub dz: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct EndpointJson {
    /// `a/b`
    pub refno: String,
    pub noun: String,
    pub leaf_count: usize,
    pub triangle_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FaceJson {
    pub kind: FaceKind,
    pub normal: PointJson,
    pub confidence: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PerpendicularJson {
    pub distance_mm: f32,
    pub from: PointJson,
    pub to: PointJson,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResultJson {
    pub distance_mm: f32,
    pub intersects: bool,
    pub source_point: PointJson,
    pub target_point: PointJson,
    /// `target_point − source_point`，dx / dy / dz 即 E / N / U。
    pub vector: DeltaJson,
    pub source_leaf_refno: String,
    pub target_leaf_refno: String,
    pub target_leaf_noun: String,
    pub target_face: Option<FaceJson>,
    pub perpendicular: Option<PerpendicularJson>,
    pub witness: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelJson {
    pub source_sesno: Option<i64>,
    pub target_sesno: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimingJson {
    pub load: u128,
    pub query: u128,
    pub total: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct LeafDebugJson {
    pub refno: String,
    pub key: String,
    pub noun: String,
    pub triangles: usize,
    pub model_sesno: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DebugJson {
    pub pairs_total: usize,
    pub pairs_evaluated: usize,
    pub pairs_pruned: usize,
    pub source_leaves: Vec<LeafDebugJson>,
    pub target_leaves: Vec<LeafDebugJson>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SurfaceClearanceResponse {
    /// 恒为 `true`：失败走 HTTP 状态码 + `ApiError`，不在 200 里报错。
    pub success: bool,
    pub unit: &'static str,
    pub method: &'static str,
    pub accuracy_class: &'static str,
    pub error_bound_mm: f32,
    pub target_kind: &'static str,
    pub source: EndpointJson,
    pub target: EndpointJson,
    /// `None` = `max_distance_mm` 内没有结果（warnings 里有 `beyond_max_distance`）。
    pub result: Option<ResultJson>,
    pub model: ModelJson,
    pub timing_ms: TimingJson,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<DebugJson>,
}

#[derive(Debug)]
pub enum SurfaceClearanceError {
    /// 任一侧没有模型面行 / 网格文件：404。
    NoModelMesh {
        refno: String,
        detail: String,
    },
    /// `target_kind=wall` 而目标不在墙族：422。
    TargetNotWall {
        refno: String,
        noun: String,
    },
    /// 两侧在模型面上是同一批叶子（剔除重合叶子后祖先那一侧空了，如目标名下只有源这一片几何）：422。
    SharedLeavesOnly {
        source: String,
        target: String,
        trimmed_side: TrimmedSide,
    },
    Other(anyhow::Error),
}

impl From<anyhow::Error> for SurfaceClearanceError {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

fn leaf_debug(leaves: &[LeafMesh]) -> Vec<LeafDebugJson> {
    leaves
        .iter()
        .map(|leaf| LeafDebugJson {
            refno: leaf.refno.clone(),
            key: leaf.key.clone(),
            noun: leaf.noun.clone(),
            triangles: leaf.triangle_count(),
            model_sesno: leaf.model_sesno,
        })
        .collect()
}

fn endpoint_json(refno: RefU64, noun: &str, leaves: &[LeafMesh]) -> EndpointJson {
    EndpointJson {
        refno: refno.to_string().replace('_', "/"),
        noun: noun.to_string(),
        leaf_count: leaves.len(),
        triangle_count: leaves.iter().map(LeafMesh::triangle_count).sum(),
    }
}

fn max_sesno(leaves: &[LeafMesh]) -> Option<i64> {
    leaves.iter().filter_map(|leaf| leaf.model_sesno).max()
}

/// 端到端：取两侧叶子行 → 拼网格 → 精算 → 组响应。`nouns` 是 handler 从属性面读到的两个 TYPE
/// （读不到给空串，这里再用叶子行上的 `generic` 兜底）。
pub async fn run(
    db: &Surreal<Any>,
    meshes_dir: PathBuf,
    params: &SurfaceClearanceParams,
    nouns: (String, String),
) -> Result<SurfaceClearanceResponse, SurfaceClearanceError> {
    let started = Instant::now();
    let source_rows = fetch_leaf_rows(db, params.source).await?;
    let target_rows = fetch_leaf_rows(db, params.target).await?;
    let own_noun = |rows: &[LeafRow], refno: RefU64, fallback: String| -> String {
        if !fallback.trim().is_empty() {
            return fallback.trim().to_ascii_uppercase();
        }
        let key = refno.to_string();
        rows.iter()
            .find(|row| row.refno == key)
            .map(|row| row.noun.clone())
            .unwrap_or_default()
    };
    let source_noun = own_noun(&source_rows, params.source, nouns.0);
    let mut target_noun = own_noun(&target_rows, params.target, nouns.1);
    let mut pre_warnings = Vec::new();
    if params.target_kind == TargetKind::Wall && !is_wall_noun(&target_noun) {
        // owner（CWALL）自己没有几何行，属性面又答不出 TYPE 时：名下叶子全是墙族就当墙收下。
        let leaves_all_walls =
            !target_rows.is_empty() && target_rows.iter().all(|row| is_wall_noun(&row.noun));
        if target_noun.is_empty() && leaves_all_walls {
            target_noun = "CWALL".to_string();
            pre_warnings.push(
                "目标 TYPE 读不到，按名下叶子全为墙族（WALL / PANE / GWALL / STWALL）当 CWALL 收下"
                    .to_string(),
            );
        } else {
            return Err(SurfaceClearanceError::TargetNotWall {
                refno: params.target.to_string().replace('_', "/"),
                noun: target_noun,
            });
        }
    }
    if source_rows.is_empty() {
        return Err(SurfaceClearanceError::NoModelMesh {
            refno: params.source.to_string().replace('_', "/"),
            detail: "没有模型面行（inst_relate / tubi_relate 里 anc 不含它，或 model_sesno 为空）"
                .to_string(),
        });
    }
    if target_rows.is_empty() {
        return Err(SurfaceClearanceError::NoModelMesh {
            refno: params.target.to_string().replace('_', "/"),
            detail: "没有模型面行（inst_relate / tubi_relate 里 anc 不含它，或 model_sesno 为空）"
                .to_string(),
        });
    }
    // 源是目标的后代（或反过来）时两侧叶子有重合：自对必回 0，先从祖先那一侧剔掉（见 split_shared_leaves）。
    let (source_rows, target_rows, shared) = split_shared_leaves(source_rows, target_rows);
    if let Some(report) = shared {
        pre_warnings.push(shared_leaf_warning(report));
        if source_rows.is_empty() || target_rows.is_empty() {
            return Err(SurfaceClearanceError::SharedLeavesOnly {
                source: params.source.to_string().replace('_', "/"),
                target: params.target.to_string().replace('_', "/"),
                trimmed_side: report.trimmed_side,
            });
        }
    }

    let opts = SurfaceClearanceOptions {
        perpendicular: params.perpendicular,
        max_distance: params.max_distance_mm,
    };
    let debug = params.debug;
    let (source_refno, target_refno) = (params.source, params.target);
    let computed = tokio::task::spawn_blocking(move || {
        let mut warnings = Vec::new();
        let load_started = Instant::now();
        let source = build_leaf_meshes(&source_rows, &meshes_dir, &mut warnings);
        let target = build_leaf_meshes(&target_rows, &meshes_dir, &mut warnings);
        let load_ms = load_started.elapsed().as_millis();
        if source.is_empty() {
            return Err(SurfaceClearanceError::NoModelMesh {
                refno: source_refno.to_string().replace('_', "/"),
                detail: format!(
                    "{} 行叶子一个网格文件都读不到: {}",
                    source_rows.len(),
                    warnings.join("; ")
                ),
            });
        }
        if target.is_empty() {
            return Err(SurfaceClearanceError::NoModelMesh {
                refno: target_refno.to_string().replace('_', "/"),
                detail: format!(
                    "{} 行叶子一个网格文件都读不到: {}",
                    target_rows.len(),
                    warnings.join("; ")
                ),
            });
        }
        let query_started = Instant::now();
        let outcome = surface_clearance(&source, &target, opts);
        let query_ms = query_started.elapsed().as_millis();
        warnings.extend(outcome.warnings);
        Ok((
            source,
            target,
            outcome.hit,
            outcome.stats,
            warnings,
            load_ms,
            query_ms,
        ))
    })
    .await
    .map_err(|join| anyhow::anyhow!("surface-clearance 计算任务异常结束: {join}"))??;
    let (source, target, hit, stats, computed_warnings, load_ms, query_ms) = computed;
    let mut warnings = pre_warnings;
    warnings.extend(computed_warnings);

    let source_sesno = max_sesno(&source);
    let target_sesno = max_sesno(&target);
    // 会话号只在同一个库里可比；跨库（dbnum 不同）各有各的计数，不同不算异常。
    if params.source.get_0() == params.target.get_0() {
        if let (Some(a), Some(b)) = (source_sesno, target_sesno) {
            if a != b {
                warnings.push(format!(
                    "两侧模型会话不同（源 {a} / 目标 {b}），结果按各自最新的一版算"
                ));
            }
        }
    }
    let result = match hit {
        Some(hit) => {
            let vector = hit.target_point - hit.source_point;
            Some(ResultJson {
                distance_mm: hit.distance,
                intersects: hit.intersects,
                source_point: hit.source_point.into(),
                target_point: hit.target_point.into(),
                vector: DeltaJson {
                    dx: vector.x,
                    dy: vector.y,
                    dz: vector.z,
                },
                source_leaf_refno: source[hit.source_leaf].refno.clone(),
                target_leaf_refno: target[hit.target_leaf].refno.clone(),
                target_leaf_noun: target[hit.target_leaf].noun.clone(),
                target_face: hit.target_face.map(|face| FaceJson {
                    kind: face.kind,
                    normal: face.normal.into(),
                    confidence: face.confidence,
                }),
                perpendicular: hit.perpendicular.map(|perp| PerpendicularJson {
                    distance_mm: perp.distance,
                    from: perp.from.into(),
                    to: perp.to.into(),
                }),
                witness: hit.witness,
            })
        }
        None => {
            warnings.push(format!(
                "beyond_max_distance: {} mm 内两侧网格没有靠近到一起",
                params.max_distance_mm
            ));
            None
        }
    };
    Ok(SurfaceClearanceResponse {
        success: true,
        unit: UNIT,
        method: METHOD,
        accuracy_class: ACCURACY_EXACT_SURFACE,
        error_bound_mm: ERROR_BOUND_MM,
        target_kind: params.target_kind.as_str(),
        source: endpoint_json(params.source, &source_noun, &source),
        target: endpoint_json(params.target, &target_noun, &target),
        result,
        model: ModelJson {
            source_sesno,
            target_sesno,
        },
        timing_ms: TimingJson {
            load: load_ms,
            query: query_ms,
            total: started.elapsed().as_millis(),
        },
        warnings,
        debug: debug.then(|| DebugJson {
            pairs_total: stats.total,
            pairs_evaluated: stats.evaluated,
            pairs_pruned: stats.pruned,
            source_leaves: leaf_debug(&source),
            target_leaves: leaf_debug(&target),
        }),
    })
}

/// 供 handler 复用的 debug 视图（`HashMap` 只是为了不把 `dashmap` 类型漏到 handler 层）。
pub fn cache_sizes() -> HashMap<&'static str, usize> {
    HashMap::from([
        ("unit_meshes", UNIT_MESHES.len()),
        ("leaf_meshes", LEAF_MESHES.len()),
    ])
}

// ───────────────────────────── 单测：合成网格 ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(refno: &str, noun: &str, mesh: TriMesh) -> LeafMesh {
        LeafMesh {
            refno: refno.to_string(),
            key: refno.replace('/', "_"),
            noun: noun.to_string(),
            mesh: Arc::new(mesh),
            model_sesno: Some(1),
        }
    }

    /// 轴对齐盒，外法向朝外的 12 个三角。
    fn box_mesh(min: [f32; 3], max: [f32; 3]) -> TriMesh {
        let p = |x: usize, y: usize, z: usize| {
            Point::new(
                if x == 0 { min[0] } else { max[0] },
                if y == 0 { min[1] } else { max[1] },
                if z == 0 { min[2] } else { max[2] },
            )
        };
        let verts = vec![
            p(0, 0, 0),
            p(1, 0, 0),
            p(1, 1, 0),
            p(0, 1, 0),
            p(0, 0, 1),
            p(1, 0, 1),
            p(1, 1, 1),
            p(0, 1, 1),
        ];
        let idx = vec![
            [0, 2, 1],
            [0, 3, 2], // -Z
            [4, 5, 6],
            [4, 6, 7], // +Z
            [0, 1, 5],
            [0, 5, 4], // -Y
            [2, 3, 7],
            [2, 7, 6], // +Y
            [1, 2, 6],
            [1, 6, 5], // +X
            [0, 4, 7],
            [0, 7, 3], // -X
        ];
        TriMesh::with_flags(verts, idx, TriMeshFlags::empty())
    }

    /// 环形扇区实体（弧墙）：内 / 外圆柱面 + 顶 / 底 + 两端盖，法向朝外。
    fn ring_sector_mesh(
        center: [f32; 2],
        r_in: f32,
        r_out: f32,
        z: [f32; 2],
        angle_deg: [f32; 2],
        segments: usize,
    ) -> TriMesh {
        let mut verts = Vec::new();
        let mut idx: Vec<[u32; 3]> = Vec::new();
        let step = (angle_deg[1] - angle_deg[0]).to_radians() / segments as f32;
        let a0 = angle_deg[0].to_radians();
        // 顶点布局：每个角度 k 有 4 个点：内下 内上 外下 外上。
        for k in 0..=segments {
            let a = a0 + step * k as f32;
            let (s, c) = a.sin_cos();
            for (r, zz) in [(r_in, z[0]), (r_in, z[1]), (r_out, z[0]), (r_out, z[1])] {
                verts.push(Point::new(center[0] + r * c, center[1] + r * s, zz));
            }
        }
        let v = |k: usize, which: usize| (k * 4 + which) as u32;
        for k in 0..segments {
            let (il0, il1, ol0, ol1) = (v(k, 0), v(k, 1), v(k, 2), v(k, 3));
            let (il0n, il1n, ol0n, ol1n) = (v(k + 1, 0), v(k + 1, 1), v(k + 1, 2), v(k + 1, 3));
            // 内圆柱面：外法向朝圆心（−r）；角度逆时针递增，(u × t) = −r。
            idx.push([il0, il1, il1n]);
            idx.push([il0, il1n, il0n]);
            // 外圆柱面：外法向朝外（+r）。
            idx.push([ol0, ol1n, ol1]);
            idx.push([ol0, ol0n, ol1n]);
            // 顶（+Z）：(r × t) = +Z。
            idx.push([il1, ol1, ol1n]);
            idx.push([il1, ol1n, il1n]);
            // 底（−Z）
            idx.push([il0, ol0n, ol0]);
            idx.push([il0, il0n, ol0n]);
        }
        // 端盖：起始角（法向朝 −tangent）与终止角。
        let (s0, s1) = (0usize, segments);
        idx.push([v(s0, 0), v(s0, 2), v(s0, 3)]);
        idx.push([v(s0, 0), v(s0, 3), v(s0, 1)]);
        idx.push([v(s1, 0), v(s1, 1), v(s1, 3)]);
        idx.push([v(s1, 0), v(s1, 3), v(s1, 2)]);
        TriMesh::with_flags(verts, idx, TriMeshFlags::empty())
    }

    /// 直墙盒 `[min, max]` 沿 X（厚度方向）开一个矩形通孔 `y ∈ hole_y, z ∈ hole_z`——手搭的「布尔后」
    /// 墙网格：外皮 8 点 + 两张开孔面上各 4 个孔角；±Y / ±Z 外面各 2 三角，−X / +X 开孔面各按
    /// 「外角连最近孔角」切成四条梯形 8 三角，四片洞壁各 2 三角，共 32 三角。法向一律朝实体外
    /// （洞壁的外法向朝洞里）。
    fn box_with_hole_mesh(
        min: [f32; 3],
        max: [f32; 3],
        hole_y: [f32; 2],
        hole_z: [f32; 2],
    ) -> TriMesh {
        let (x0, x1) = (min[0], max[0]);
        let (y0, y1) = (min[1], max[1]);
        let (z0, z1) = (min[2], max[2]);
        let (hy0, hy1) = (hole_y[0], hole_y[1]);
        let (hz0, hz1) = (hole_z[0], hole_z[1]);
        assert!(y0 < hy0 && hy0 < hy1 && hy1 < y1 && z0 < hz0 && hz0 < hz1 && hz1 < z1);
        let verts = vec![
            // 0–7：外皮，与 box_mesh 同序。
            Point::new(x0, y0, z0),
            Point::new(x1, y0, z0),
            Point::new(x1, y1, z0),
            Point::new(x0, y1, z0),
            Point::new(x0, y0, z1),
            Point::new(x1, y0, z1),
            Point::new(x1, y1, z1),
            Point::new(x0, y1, z1),
            // 8–11：−X 面上的孔角（y 小 z 小、y 大 z 小、y 大 z 大、y 小 z 大）。
            Point::new(x0, hy0, hz0),
            Point::new(x0, hy1, hz0),
            Point::new(x0, hy1, hz1),
            Point::new(x0, hy0, hz1),
            // 12–15：+X 面上的孔角，同序。
            Point::new(x1, hy0, hz0),
            Point::new(x1, hy1, hz0),
            Point::new(x1, hy1, hz1),
            Point::new(x1, hy0, hz1),
        ];
        // −Z / +Z / −Y / +Y 四个没开孔的外面，与 box_mesh 相同。
        let mut idx: Vec<[u32; 3]> = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [2, 3, 7],
            [2, 7, 6],
        ];
        let mut quad = |a: u32, b: u32, c: u32, d: u32| {
            idx.push([a, b, c]);
            idx.push([a, c, d]);
        };
        // −X 开孔面（法向 −X）：孔下 / 孔上 / y 小侧 / y 大侧四条梯形。
        quad(0, 8, 9, 3);
        quad(4, 7, 10, 11);
        quad(0, 4, 11, 8);
        quad(3, 9, 10, 7);
        // +X 开孔面（法向 +X）：同四条，绕向反过来。
        quad(1, 2, 13, 12);
        quad(5, 15, 14, 6);
        quad(1, 12, 15, 5);
        quad(2, 6, 14, 13);
        // 四片洞壁，外法向朝洞里：洞底 +Z、洞顶 −Z、y 小侧 +Y、y 大侧 −Y。
        quad(8, 12, 13, 9);
        quad(11, 10, 14, 15);
        quad(8, 11, 15, 12);
        quad(9, 13, 14, 10);
        TriMesh::with_flags(verts, idx, TriMeshFlags::empty())
    }

    fn opts() -> SurfaceClearanceOptions {
        SurfaceClearanceOptions {
            perpendicular: true,
            max_distance: DEFAULT_MAX_DISTANCE_MM,
        }
    }

    fn approx(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn g1_box_facing_straight_wall_gives_gap_side_and_perpendicular() {
        let wall = leaf(
            "1/1",
            "STWALL",
            box_mesh([0.0, 0.0, 0.0], [200.0, 5000.0, 3000.0]),
        );
        let part = leaf(
            "2/2",
            "EQUI",
            box_mesh([500.0, 1000.0, 500.0], [900.0, 1400.0, 900.0]),
        );
        let out = surface_clearance(&[part], &[wall], opts());
        let hit = out.hit.expect("hit");
        assert!(
            approx(hit.distance, 300.0, 1e-3),
            "distance {}",
            hit.distance
        );
        assert!(!hit.intersects);
        assert!(approx(hit.source_point.x, 500.0, 1e-3) && approx(hit.target_point.x, 200.0, 1e-3));
        let face = hit.target_face.expect("face");
        assert_eq!(face.kind, FaceKind::Side, "{face:?}");
        assert!(
            face.normal.x > 0.99,
            "normal should point +X toward the source: {:?}",
            face.normal
        );
        let perp = hit.perpendicular.expect("perpendicular");
        assert!(approx(perp.distance, 300.0, 1e-2), "perp {}", perp.distance);
        assert!(approx(perp.to.x, 200.0, 1e-2));
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[test]
    fn g2_box_beyond_wall_end_hits_edge_without_perpendicular() {
        let wall = leaf(
            "1/1",
            "STWALL",
            box_mesh([0.0, 0.0, 0.0], [200.0, 5000.0, 3000.0]),
        );
        // 在墙端（y=5000）之外、斜着靠近：最近点是墙的竖棱 (200, 5000, z)。
        let part = leaf(
            "2/2",
            "EQUI",
            box_mesh([500.0, 5400.0, 500.0], [900.0, 5800.0, 900.0]),
        );
        let out = surface_clearance(&[part], &[wall], opts());
        let hit = out.hit.expect("hit");
        assert!(
            approx(hit.distance, 500.0, 1e-2),
            "distance {}",
            hit.distance
        );
        assert!(
            approx(hit.target_point.x, 200.0, 1e-2) && approx(hit.target_point.y, 5000.0, 1e-2)
        );
        let face = hit.target_face.expect("face");
        assert!(
            matches!(face.kind, FaceKind::Side | FaceKind::End),
            "{face:?}"
        );
        assert!(
            hit.perpendicular.is_none(),
            "ray along a face normal must miss at the edge"
        );
        assert!(
            out.warnings
                .iter()
                .any(|w| w.starts_with("perpendicular_ray_missed"))
        );
    }

    #[test]
    fn g3_curved_wall_classifies_inner_and_outer_within_chord_error() {
        // 120° 弧墙，R 5000–5300，48 段 → 步 2.5°，弦高 R(1−cos 1.25°) ≈ 1.2 mm。
        let wall = ring_sector_mesh([0.0, 0.0], 5000.0, 5300.0, [0.0, 3000.0], [0.0, 120.0], 48);
        let wall = leaf("1/1", "WALL", wall);
        let (s60, c60) = 60f32.to_radians().sin_cos();
        let inner_box = {
            let r = 4500.0;
            let (cx, cy) = (r * c60, r * s60);
            leaf(
                "2/2",
                "EQUI",
                box_mesh([cx - 1.0, cy - 1.0, 1000.0], [cx + 1.0, cy + 1.0, 1200.0]),
            )
        };
        let outer_box = {
            let r = 5800.0;
            let (cx, cy) = (r * c60, r * s60);
            leaf(
                "3/3",
                "EQUI",
                box_mesh([cx - 1.0, cy - 1.0, 1000.0], [cx + 1.0, cy + 1.0, 1200.0]),
            )
        };
        let inner = surface_clearance(&[inner_box], std::slice::from_ref(&wall), opts());
        let hit = inner.hit.expect("inner hit");
        assert!(
            approx(hit.distance, 500.0, 3.0),
            "inner distance {}",
            hit.distance
        );
        let face = hit.target_face.expect("face");
        assert_eq!(face.kind, FaceKind::Inner, "{face:?}");
        assert_eq!(face.confidence, "geometric");
        assert!(
            hit.perpendicular.is_some(),
            "inner face is a main face: {:?}",
            inner.warnings
        );

        let outer = surface_clearance(&[outer_box], &[wall], opts());
        let hit = outer.hit.expect("outer hit");
        assert!(
            approx(hit.distance, 500.0, 3.0),
            "outer distance {}",
            hit.distance
        );
        let face = hit.target_face.expect("face");
        assert_eq!(face.kind, FaceKind::Outer, "{face:?}");
    }

    #[test]
    fn g4_intersecting_boxes_report_zero_and_intersects() {
        let wall = leaf(
            "1/1",
            "STWALL",
            box_mesh([0.0, 0.0, 0.0], [200.0, 5000.0, 3000.0]),
        );
        let part = leaf(
            "2/2",
            "EQUI",
            box_mesh([100.0, 1000.0, 500.0], [900.0, 1400.0, 900.0]),
        );
        let out = surface_clearance(&[part], &[wall], opts());
        let hit = out.hit.expect("hit");
        assert_eq!(hit.distance, 0.0);
        assert!(hit.intersects);
        assert_eq!(hit.witness, "aabb-overlap-center");
        assert!(hit.perpendicular.is_none() && hit.target_face.is_none());
    }

    #[test]
    fn g5_multi_leaf_picks_the_nearest_pair_and_prunes_the_rest() {
        let near_wall = leaf(
            "1/1",
            "STWALL",
            box_mesh([0.0, 0.0, 0.0], [200.0, 5000.0, 3000.0]),
        );
        let far_wall = leaf(
            "1/2",
            "STWALL",
            box_mesh([-20000.0, 0.0, 0.0], [-19800.0, 5000.0, 3000.0]),
        );
        let near_part = leaf(
            "2/1",
            "BOX",
            box_mesh([500.0, 1000.0, 500.0], [900.0, 1400.0, 900.0]),
        );
        let far_part = leaf(
            "2/2",
            "BOX",
            box_mesh([9000.0, 1000.0, 500.0], [9400.0, 1400.0, 900.0]),
        );
        let out = surface_clearance(&[far_part, near_part], &[far_wall, near_wall], opts());
        let hit = out.hit.expect("hit");
        assert!(approx(hit.distance, 300.0, 1e-3));
        assert_eq!((hit.source_leaf, hit.target_leaf), (1, 1));
        assert_eq!(out.stats.total, 4);
        assert!(out.stats.pruned >= 1, "{:?}", out.stats);
    }

    /// `max_distance` 剪枝：唯一一对的 AABB 下界就超了阈值 → 不算、无命中。（不是计划表的 G 编号。）
    #[test]
    fn beyond_max_distance_gives_no_hit() {
        let wall = leaf(
            "1/1",
            "STWALL",
            box_mesh([0.0, 0.0, 0.0], [200.0, 5000.0, 3000.0]),
        );
        let part = leaf(
            "2/2",
            "EQUI",
            box_mesh([5000.0, 1000.0, 500.0], [5400.0, 1400.0, 900.0]),
        );
        let out = surface_clearance(
            &[part],
            &[wall],
            SurfaceClearanceOptions {
                perpendicular: true,
                max_distance: 1000.0,
            },
        );
        assert!(out.hit.is_none());
        assert_eq!(out.stats.pruned, 1);
    }

    /// 顶面分类：构件骑在墙顶上方 → `top`，顶面不是主面、不打垂距。（不是计划表的 G 编号；
    /// 计划 G7「源为 BRAN owner」要库里的 `tubi_relate` 行，只有 live 用例。）
    #[test]
    fn top_face_is_classified_without_perpendicular() {
        let wall = leaf(
            "1/1",
            "STWALL",
            box_mesh([0.0, 0.0, 0.0], [200.0, 5000.0, 3000.0]),
        );
        let part = leaf(
            "2/2",
            "EQUI",
            box_mesh([50.0, 1000.0, 3400.0], [150.0, 1400.0, 3800.0]),
        );
        let out = surface_clearance(&[part], &[wall], opts());
        let hit = out.hit.expect("hit");
        assert!(approx(hit.distance, 400.0, 1e-3));
        assert_eq!(hit.target_face.expect("face").kind, FaceKind::Top);
        assert!(hit.perpendicular.is_none());
    }

    /// 09-17 计划 §7 表的 G6：带洞墙、构件在洞里 → 报到洞壁的距离，不是未开洞墙的 0。
    /// 墙 = 200 厚直墙沿厚度方向开 400 × 400 方孔；管 = 穿过孔的盒，离 y 小侧洞壁 50 mm、
    /// 离 y 大侧 100 mm、上下各 80 mm。对照：同一根管对没开孔的同尺寸墙就是 G4（0 + intersects）。
    #[test]
    fn g6_component_in_wall_hole_measures_to_hole_wall_not_zero() {
        let (hole_y, hole_z) = ([2000.0, 2400.0], [1000.0, 1400.0]);
        let wall_with_hole = leaf(
            "1/1",
            "STWALL",
            box_with_hole_mesh([0.0, 0.0, 0.0], [200.0, 5000.0, 3000.0], hole_y, hole_z),
        );
        assert_eq!(wall_with_hole.triangle_count(), 32);
        let pipe = || {
            leaf(
                "2/2",
                "FTUB",
                box_mesh([-500.0, 2050.0, 1080.0], [700.0, 2300.0, 1320.0]),
            )
        };

        let out = surface_clearance(&[pipe()], &[wall_with_hole], opts());
        let hit = out.hit.expect("hit");
        assert!(!hit.intersects, "洞里的管不该报穿墙：{hit:?}");
        assert_eq!(hit.witness, "closest-points");
        assert!(approx(hit.distance, 50.0, 1e-2), "distance {}", hit.distance);
        // 最近点对：墙侧在 y = 2000 那片洞壁上（x 在墙厚内、z 在孔高内），管侧在管的 y = 2050 面上。
        assert!(
            approx(hit.target_point.y, hole_y[0], 1e-2),
            "target {:?}",
            hit.target_point
        );
        assert!(
            hit.target_point.x >= -1e-2
                && hit.target_point.x <= 200.0 + 1e-2
                && hit.target_point.z >= hole_z[0] - 1e-2
                && hit.target_point.z <= hole_z[1] + 1e-2,
            "target point must lie on the hole wall: {:?}",
            hit.target_point
        );
        assert!(
            approx(hit.source_point.y, 2050.0, 1e-2),
            "source {:?}",
            hit.source_point
        );
        assert!(hit.target_face.is_some());
        assert_eq!((out.stats.total, out.stats.evaluated), (1, 1));

        // 对照：没开孔的同一堵墙，同一根管就是 G4。
        let solid_wall = leaf(
            "1/1",
            "STWALL",
            box_mesh([0.0, 0.0, 0.0], [200.0, 5000.0, 3000.0]),
        );
        let out = surface_clearance(&[pipe()], &[solid_wall], opts());
        let hit = out.hit.expect("hit");
        assert_eq!(hit.distance, 0.0);
        assert!(hit.intersects, "{hit:?}");
        assert_eq!(hit.witness, "aabb-overlap-center");
    }

    /// 洞壁面分类的**现状**（不是口径）：`classify_target_face` 只认「Up = +Z 分顶底、水平 PCA 分端 / 侧」，
    /// 不知道自己命中的是洞壁——沿厚度方向开的孔，两片竖洞壁的法向 ±Y 与墙的行进方向平行，读成
    /// `end`（pca）；洞底 / 洞顶法向 ±Z 读成 `top` / `bottom`（normal-only）。三种都不是主面，不打垂距、
    /// 不出 warning。以后要给洞壁单独口径，改这条就是改口径。
    /// 构件用不穿墙的短盒（x 50–150）贴近某一片洞壁，让最近点落在那片洞壁内部而不是棱上，分类才唯一。
    #[test]
    fn hole_wall_faces_currently_classify_as_end_and_top_bottom() {
        let (hole_y, hole_z) = ([2000.0, 2400.0], [1000.0, 1400.0]);
        let wall = || {
            leaf(
                "1/1",
                "STWALL",
                box_with_hole_mesh([0.0, 0.0, 0.0], [200.0, 5000.0, 3000.0], hole_y, hole_z),
            )
        };
        // （短盒 y 范围, z 范围, 期望面, 期望置信度, 期望法向——已翻成指向源侧）
        let cases: [([f32; 2], [f32; 2], FaceKind, &str, Vector<Real>); 4] = [
            // 贴 y 小侧洞壁 50 mm：法向 +Y 与行进方向平行 → end。
            ([2050.0, 2250.0], [1100.0, 1300.0], FaceKind::End, "pca", Vector::y()),
            // 贴 y 大侧洞壁：法向 −Y → end。
            ([2150.0, 2350.0], [1100.0, 1300.0], FaceKind::End, "pca", -Vector::y()),
            // 贴洞底：法向 +Z → top。
            ([2100.0, 2300.0], [1050.0, 1250.0], FaceKind::Top, "normal-only", Vector::z()),
            // 贴洞顶：法向 −Z → bottom。
            ([2100.0, 2300.0], [1150.0, 1350.0], FaceKind::Bottom, "normal-only", -Vector::z()),
        ];
        for (ys, zs, kind, confidence, normal) in cases {
            let stub = leaf(
                "2/2",
                "BOX",
                box_mesh([50.0, ys[0], zs[0]], [150.0, ys[1], zs[1]]),
            );
            let out = surface_clearance(&[stub], &[wall()], opts());
            let hit = out.hit.expect("hit");
            assert!(
                !hit.intersects && approx(hit.distance, 50.0, 1e-2),
                "{ys:?} {zs:?}: {hit:?}"
            );
            let face = hit.target_face.expect("face");
            assert_eq!(face.kind, kind, "{ys:?} {zs:?}: {face:?}");
            assert_eq!(face.confidence, confidence, "{ys:?} {zs:?}: {face:?}");
            assert!(
                face.normal.dot(&normal) > 0.99,
                "{ys:?} {zs:?}: normal {:?}",
                face.normal
            );
            assert!(
                hit.perpendicular.is_none(),
                "{ys:?} {zs:?}: 洞壁三种面都不是主面，不该有垂距"
            );
            assert!(out.warnings.is_empty(), "{ys:?} {zs:?}: {:?}", out.warnings);
        }
    }

    #[test]
    fn params_default_to_wall_and_reject_same_refno() {
        let raw = RawSurfaceClearanceQuery {
            source_refno: Some("24381/145018".into()),
            target_refno: Some("17496_105812".into()),
            ..Default::default()
        };
        let params = SurfaceClearanceParams::parse(&raw).expect("parse");
        assert_eq!(params.target_kind, TargetKind::Wall);
        assert!(params.perpendicular);
        assert_eq!(params.max_distance_mm, DEFAULT_MAX_DISTANCE_MM);
        assert_eq!(params.target.to_string(), "17496_105812");

        let same = RawSurfaceClearanceQuery {
            source_refno: Some("1/2".into()),
            target_refno: Some("1_2".into()),
            ..Default::default()
        };
        assert!(SurfaceClearanceParams::parse(&same).is_err());

        let bad_kind = RawSurfaceClearanceQuery {
            source_refno: Some("1/2".into()),
            target_refno: Some("1/3".into()),
            target_kind: Some("column".into()),
            ..Default::default()
        };
        assert!(SurfaceClearanceParams::parse(&bad_kind).is_err());
    }

    #[test]
    fn wall_nouns_cover_owner_and_members() {
        for noun in ["CWALL", "wall", "Pane", "GWALL", "STWALL"] {
            assert!(is_wall_noun(noun), "{noun}");
        }
        assert!(!is_wall_noun("EQUI"));
    }

    #[test]
    fn row_to_leaf_reads_booled_flat_and_tube_rows() {
        let booled = serde_json::json!({
            "key": "17496_105812", "refno": "pe:⟨17496_105812⟩", "noun": "STWALL",
            "wt": {"translation": [1.0, 2.0, 3.0], "rotation": [0.0, 0.0, 0.0, 1.0]},
            "insts_flat": [{"geo_hash": "ignored_when_booled"}],
            "booled_id": "e3d_baked_v2_abc", "model_sesno": 17
        });
        let row = row_to_leaf(&booled, false).expect("row");
        assert_eq!(row.refno, "17496_105812");
        assert_eq!(row.insts, vec![("e3d_baked_v2_abc".to_string(), None)]);
        assert_eq!(row.model_sesno, Some(17));

        let flat = serde_json::json!({
            "key": "1_2", "refno": "pe:1_2", "noun": "BOX", "booled_id": null,
            "insts_flat": [
                {"geo_hash": "h1", "transform": {"translation": [0, 0, 0], "rotation": [0, 0, 0, 1]}},
                {"geo_hash": "h2"}
            ],
            "model_sesno": 3
        });
        let row = row_to_leaf(&flat, false).expect("row");
        assert_eq!(row.insts.len(), 2);
        assert!(row.insts[0].1.is_some() && row.insts[1].1.is_none());

        let tube = serde_json::json!({
            "key": "tubi_relate:[pe:⟨1_2⟩, 0]", "refno": "pe:⟨1_2⟩", "geo_hash": "tube_hash", "model_sesno": 3
        });
        let row = row_to_leaf(&tube, true).expect("row");
        assert_eq!(row.noun, "TUBI");
        assert_eq!(row.insts, vec![("tube_hash".to_string(), None)]);
    }

    fn leaf_row(key: &str, refno: &str, noun: &str) -> LeafRow {
        let insts = vec![(format!("hash_{refno}"), None)];
        LeafRow {
            key: key.to_string(),
            refno: refno.to_string(),
            noun: noun.to_string(),
            world_trans: None,
            placement_fingerprint: placement_fingerprint(None, &insts),
            insts,
            model_sesno: Some(1),
        }
    }

    fn keys(rows: &[LeafRow]) -> Vec<&str> {
        rows.iter().map(|row| row.key.as_str()).collect()
    }

    /// 源是目标的后代（门框 FIXING × 它所在的 WALL）：目标集合含源自身，重合叶子从目标侧剔掉，
    /// warning 以 `source_within_target:` 开头；反过来（BRAN owner × 自己的弯头）从源侧剔掉。
    #[test]
    fn shared_leaves_are_trimmed_from_the_ancestor_side() {
        // FIXING × WALL：目标 = WALL + FIXING（后代），源 = FIXING。
        let fixing = || leaf_row("17496_137183", "17496_137183", "FIXING");
        let wall = || leaf_row("17496_105912", "17496_105912", "WALL");
        let (source, target, report) = split_shared_leaves(vec![fixing()], vec![wall(), fixing()]);
        assert_eq!(keys(&source), vec!["17496_137183"]);
        assert_eq!(
            keys(&target),
            vec!["17496_105912"],
            "重合的 FIXING 该从目标（祖先）侧剔掉"
        );
        let report = report.expect("有重合就该有记录");
        assert_eq!(report.trimmed_side, TrimmedSide::Target);
        assert_eq!(report.removed, 1);
        assert!(shared_leaf_warning(report).starts_with("source_within_target: "));

        // BRAN owner × 自己的弯头：源 = 三片叶子（含隐式管身），目标 = 弯头那一片。
        let elbo = || leaf_row("24384_22582", "24384_22582", "ELBO");
        let tube = || leaf_row("tubi_relate:[pe:⟨24384_22659⟩, 0]", "24384_22659", "TUBI");
        let flan = || leaf_row("24384_22590", "24384_22590", "FLAN");
        let (source, target, report) =
            split_shared_leaves(vec![elbo(), tube(), flan()], vec![elbo()]);
        assert_eq!(
            keys(&source),
            vec!["tubi_relate:[pe:⟨24384_22659⟩, 0]", "24384_22590"]
        );
        assert_eq!(keys(&target), vec!["24384_22582"]);
        let report = report.expect("有重合就该有记录");
        assert_eq!(report.trimmed_side, TrimmedSide::Source);
        assert_eq!(report.removed, 1);
        assert!(shared_leaf_warning(report).starts_with("target_within_source: "));

        // 没有祖先关系：一片不动、不出记录。
        let (source, target, report) = split_shared_leaves(vec![elbo()], vec![wall(), fixing()]);
        assert_eq!(keys(&source), vec!["24384_22582"]);
        assert_eq!(keys(&target), vec!["17496_105912", "17496_137183"]);
        assert!(report.is_none());

        // 两侧就是同一批叶子（目标名下只有源这一片几何）：剔完目标空了，由 run() 报 SharedLeavesOnly。
        let (source, target, report) = split_shared_leaves(vec![fixing()], vec![fixing()]);
        assert_eq!(keys(&source), vec!["17496_137183"]);
        assert!(target.is_empty());
        assert_eq!(report.map(|r| r.trimmed_side), Some(TrimmedSide::Target));

        // 旧数据没有 record id：身份退到 refno + 网格键，同样认得出重合。
        let mut legacy_a = leaf_row("", "1_2", "BOX");
        legacy_a.insts = vec![("h1".to_string(), None)];
        let mut legacy_b = leaf_row("", "1_2", "BOX");
        legacy_b.insts = vec![("h1".to_string(), None)];
        let owner_extra = leaf_row("1_3", "1_3", "CYLI");
        let (source, target, report) =
            split_shared_leaves(vec![legacy_a], vec![owner_extra, legacy_b]);
        assert_eq!(source.len(), 1);
        assert_eq!(keys(&target), vec!["1_3"]);
        assert_eq!(report.map(|r| r.removed), Some(1));
    }

    #[test]
    fn mat_from_trans_applies_scale_rotation_translation() {
        let t = serde_json::json!({"translation": [10.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [2.0, 2.0, 2.0]});
        let m = mat_from_trans(Some(&t));
        let p = m.transform_point3(Vec3::new(1.0, 0.0, 0.0));
        assert!((p - Vec3::new(12.0, 0.0, 0.0)).length() < 1e-5, "{p:?}");
        assert_eq!(mat_from_trans(None), Mat4::IDENTITY);
    }

    /// 摆放指纹：同一行两次读出来一样；`world_trans`、实例的本地变换、`geo_hash` 任一变了就不一样。
    /// `row_to_leaf` 算出来的就是它，世界网格缓存键的第三格靠它把重算前后分开。
    #[test]
    fn placement_fingerprint_tracks_world_trans_and_instances() {
        let row = |wt: serde_json::Value, insts: serde_json::Value| {
            serde_json::json!({
                "key": "17496_137183", "refno": "pe:17496_137183", "noun": "FIXING",
                "wt": wt, "insts_flat": insts, "booled_id": null, "model_sesno": 729
            })
        };
        let stale = serde_json::json!({"translation": [-2593.8, 12222.1, -5345.0], "rotation": [0.0, 0.0, 0.0, 1.0]});
        let fixed = serde_json::json!({"translation": [-15559.96, -2743.64, 2150.0], "rotation": [0.0, 0.0, 0.0, 1.0]});
        let inst = serde_json::json!([{"geo_hash": "e3d_baked_v2_c3edc9", "transform": {"translation": [0.0, 0.0, 0.0]}}]);
        let inst_moved = serde_json::json!([{"geo_hash": "e3d_baked_v2_c3edc9", "transform": {"translation": [1.0, 0.0, 0.0]}}]);
        let inst_other = serde_json::json!([{"geo_hash": "e3d_baked_v2_other", "transform": {"translation": [0.0, 0.0, 0.0]}}]);

        let a = row_to_leaf(&row(stale.clone(), inst.clone()), false).expect("row");
        let a_again = row_to_leaf(&row(stale.clone(), inst.clone()), false).expect("row");
        let b = row_to_leaf(&row(fixed, inst.clone()), false).expect("row");
        let c = row_to_leaf(&row(stale.clone(), inst_moved), false).expect("row");
        let d = row_to_leaf(&row(stale, inst_other), false).expect("row");
        assert_eq!(
            a.placement_fingerprint, a_again.placement_fingerprint,
            "同一行要稳定"
        );
        assert_eq!(
            a.placement_fingerprint,
            placement_fingerprint(a.world_trans.as_ref(), &a.insts)
        );
        assert_ne!(
            a.placement_fingerprint, b.placement_fingerprint,
            "world_trans 变了"
        );
        assert_ne!(
            a.placement_fingerprint, c.placement_fingerprint,
            "实例本地变换变了"
        );
        assert_ne!(
            a.placement_fingerprint, d.placement_fingerprint,
            "geo_hash 变了"
        );
        // 缓存键三格：key / sesno / 指纹——同 key 同 sesno 摆放不同，键必须不同；老数据没 sesno 不进缓存。
        let ka = leaf_cache_key(&a).expect("key");
        let kb = leaf_cache_key(&b).expect("key");
        assert_eq!((ka.0.as_str(), ka.1), ("17496_137183", 729));
        assert_eq!((ka.0.as_str(), ka.1), (kb.0.as_str(), kb.1));
        assert_ne!(ka.2, kb.2);
        let mut no_sesno = a.clone();
        no_sesno.model_sesno = None;
        assert!(leaf_cache_key(&no_sesno).is_none());
        let mut no_key = a;
        no_key.key.clear();
        assert!(leaf_cache_key(&no_key).is_none());
    }

    /// 2026-09-18 `:8022` 实测的那个坑：`model/ensure force=true` 重算一个根，库里 `world_trans` 已换、
    /// `model_sesno` 仍是 729，旧键 `(key, sesno)` 命中重算前的世界网格，FIXING × WALL 照旧回 10 358 mm。
    /// 现在同 key 同 sesno、只换 `world_trans` 的第二行必须拼出新位置的网格，而不是缓存里的旧网格。
    #[test]
    fn leaf_mesh_cache_misses_when_placement_changes_under_the_same_sesno() {
        let dir = std::env::temp_dir().join(format!(
            "gmmc-surface-clearance-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp meshes dir");
        let geo_hash = "unit_tri_for_cache_test";
        let unit = PlantMesh {
            indices: vec![0, 1, 2],
            vertices: vec![
                Vec3::ZERO,
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            normals: vec![Vec3::Z, Vec3::Z, Vec3::Z],
            ..PlantMesh::default()
        };
        unit.ser_to_file(&dir.join(format!("{geo_hash}.mesh")))
            .expect("write unit mesh");

        let row_at = |x: f64| {
            let json = serde_json::json!({
                "key": "cache_test_17496_137183", "refno": "pe:17496_137183", "noun": "FIXING",
                "wt": {"translation": [x, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0]},
                "insts_flat": [{"geo_hash": geo_hash}], "booled_id": null, "model_sesno": 729
            });
            row_to_leaf(&json, false).expect("row")
        };
        let mut warnings = Vec::new();
        let before = build_leaf_mesh(&row_at(0.0), &dir, &mut warnings).expect("mesh before regen");
        let after =
            build_leaf_mesh(&row_at(1000.0), &dir, &mut warnings).expect("mesh after regen");
        let same_again =
            build_leaf_mesh(&row_at(0.0), &dir, &mut warnings).expect("mesh before regen again");
        assert!(warnings.is_empty(), "{warnings:?}");
        let min_x = |mesh: &TriMesh| {
            mesh.vertices()
                .iter()
                .map(|v| v.x)
                .fold(f32::INFINITY, f32::min)
        };
        assert!(
            (min_x(&before) - 0.0).abs() < 1e-3,
            "重算前的网格在原位: {}",
            min_x(&before)
        );
        assert!(
            (min_x(&after) - 1000.0).abs() < 1e-3,
            "同 key 同 sesno 换了 world_trans，必须拼新网格而不是命中旧缓存: {}",
            min_x(&after)
        );
        assert!(
            Arc::ptr_eq(&before, &same_again),
            "摆放没变的行仍要命中同一份缓存"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// live 8009（ns 1516 / AvevaMarineSample，与 `_runs/*-8022|8023` 两台服务同一个库）：
    /// 真构件 × 真墙（ams1112 `CWALL /1RS-WF03-W-C-RR001` 的 WALL 1 = 弧墙 `17496/105912`、
    /// STWALL 1 = 直墙 `17496/105812`、CWALL owner `17496/105799`），量 load / query 耗时。
    /// 网格目录取 `PLANT_MESH_DIR`（缺省 `assets/meshes`）。只读库，不写。
    #[tokio::test]
    #[ignore = "live 8009：真构件 × 真墙的 surface-clearance 量耗时（PLANT_MESH_DIR 指向服务的 assets/meshes）"]
    async fn live_component_to_wall_timing() {
        use surrealdb::engine::any::connect;
        use surrealdb::opt::auth::Root;

        let db = connect("ws://127.0.0.1:8009").await.expect("connect 8009");
        db.signin(Root {
            username: "root",
            password: "root",
        })
        .await
        .expect("signin");
        db.use_ns("1516")
            .use_db("AvevaMarineSample")
            .await
            .expect("use ns/db");
        let meshes_dir = std::env::var_os("PLANT_MESH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("assets/meshes"));

        let pairs: [(&str, &str, &str, &str); 7] = [
            (
                "A ELBO × 弧墙 WALL 1",
                "24384/22582",
                "17496/105912",
                "wall",
            ),
            (
                "B BEND × 弧墙 WALL 1",
                "24384/22678",
                "17496/105912",
                "wall",
            ),
            (
                "C SCTN × 弧墙 WALL 1",
                "24383/68484",
                "17496/105912",
                "wall",
            ),
            (
                "D BOX × 直墙 STWALL 1",
                "24384/24830",
                "17496/105812",
                "wall",
            ),
            (
                "E BRAN owner × 弧墙 WALL 1",
                "24384/22659",
                "17496/105912",
                "wall",
            ),
            (
                "F BOX × CWALL owner（约 2200 叶子）",
                "24384/24830",
                "17496/105799",
                "wall",
            ),
            (
                "G ELBO × BEND（target_kind=any）",
                "24384/22582",
                "24384/22678",
                "any",
            ),
        ];
        let mut failures = Vec::new();
        for (label, source, target, kind) in pairs {
            let params = SurfaceClearanceParams::parse(&RawSurfaceClearanceQuery {
                source_refno: Some(source.into()),
                target_refno: Some(target.into()),
                target_kind: Some(kind.into()),
                debug: Some("1".into()),
                ..Default::default()
            })
            .expect("params");
            // handler 会从属性面读 TYPE；这里只给 owner 那对（CWALL 自己没有几何行、叶子里混着 FITT 等
            // 成员，不能靶叶子兜底），其余留空走叶子行 `generic` 兜底那条路。
            let nouns = if target == "17496/105799" {
                ("BOX".to_string(), "CWALL".to_string())
            } else {
                (String::new(), String::new())
            };
            for pass in ["cold", "warm"] {
                match run(&db, meshes_dir.clone(), &params, nouns.clone()).await {
                    Ok(resp) => {
                        let r = resp.result.as_ref();
                        let dbg = resp.debug.as_ref();
                        eprintln!(
                            "[{label}] {pass}: src {} {}×{}tri | tgt {} {}×{}tri | pairs {}/{} eval/pruned={}/{} | {} | timing load={}ms query={}ms total={}ms | warnings={:?}",
                            resp.source.noun,
                            resp.source.leaf_count,
                            resp.source.triangle_count,
                            resp.target.noun,
                            resp.target.leaf_count,
                            resp.target.triangle_count,
                            dbg.map(|d| d.pairs_total).unwrap_or_default(),
                            dbg.map(|d| d.pairs_total).unwrap_or_default(),
                            dbg.map(|d| d.pairs_evaluated).unwrap_or_default(),
                            dbg.map(|d| d.pairs_pruned).unwrap_or_default(),
                            match r {
                                Some(r) => format!(
                                    "distance {:.2} mm intersects={} face={:?}/{} perpendicular={} src_leaf={} tgt_leaf={} {} vector=({:.1},{:.1},{:.1})",
                                    r.distance_mm,
                                    r.intersects,
                                    r.target_face.as_ref().map(|f| f.kind),
                                    r.target_face.as_ref().map(|f| f.confidence).unwrap_or("-"),
                                    r.perpendicular
                                        .as_ref()
                                        .map(|p| format!("{:.2}", p.distance_mm))
                                        .unwrap_or_else(|| "null".into()),
                                    r.source_leaf_refno,
                                    r.target_leaf_refno,
                                    r.target_leaf_noun,
                                    r.vector.dx,
                                    r.vector.dy,
                                    r.vector.dz,
                                ),
                                None => "result=null".to_string(),
                            },
                            resp.timing_ms.load,
                            resp.timing_ms.query,
                            resp.timing_ms.total,
                            resp.warnings,
                        );
                        if let Some(r) = r {
                            assert!(r.distance_mm.is_finite() && r.distance_mm >= 0.0, "{label}");
                        }
                    }
                    Err(error) => {
                        eprintln!("[{label}] {pass}: ERROR {error:?}");
                        failures.push(format!("{label}: {error:?}"));
                        break;
                    }
                }
            }
        }
        assert!(failures.is_empty(), "live pairs failed: {failures:#?}");
    }
}
