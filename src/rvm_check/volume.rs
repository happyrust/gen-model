//! 配上的构件再比一次体积（GitHub #127 / #171）。
//!
//! 对拍对配上的件只比包围盒与图元种类：构件摆错了、包围盒恰好不变时会被判成一致。2026-11-10 的实例是
//! 三本 APS 库里 48 根竖杆（`/MDS/FT54x/n-BAR-1`）端部切口摆反 180°——包围盒与 E3D 相同，对拍一直判
//! `exact`，实际体积多 0.60%–2.34%。同一批 108 件里体积差超过 0.05% 的正好只有其中 16 根有问题的杆，
//! 其余最大 0.016%（e3d-model#34 / gen-model#127）。
//!
//! 报告里没有体积（`Pair::volume_mm3` 在 RVM ↔ RVM 这条路上恒为 0——RVM 不带体积），所以这里直接读两份
//! RVM，按成员算体积：
//!
//! - **面片**（`FacetGroup`）：每条轮廓按扇形三角化，用散度定理算有向体积（局部 mm³，与刚体变换无关）。
//!   算出来接近 0 的（开面、纯平面）标成不可比——体积没有意义，不能拿来判。
//! - **解析图元**：按参数算闭式体积（盒 / 圆柱 / 球 / 棱台 / 圆台 / 圆环 / 矩形环 / 球冠）。
//!   椭球碟（`EllipticalDish`，折边碟）与不认识的图元标成不可比。
//! - **只有中心线**（`Line`）：没有体，不比。
//!
//! 两侧按**主键**对：生成侧按成员路径（报告的 `pairs[].refno`）接，基准侧按显示名归一后的主键接
//! （[`display_key`]，与 vendor 建表同一口径）。按几何中心配上的管身两侧主键不同，找不到对侧——照旧只比
//! 包围盒，并在构件行上写明没比体积。
//!
//! ## 两侧都是面片时的两道体积门（GitHub #171）
//!
//! 解析图元与同格面片的体积门限 [`VOLUME_DELTA_LIMIT`] 取 0.05%。但两侧都是面片时，散度定理累加出的数值
//! 还会因为另外两件事与 E3D 拉开几个百分点——不能直接当成几何错翻成 `off`：
//!
//! 1. **面片非单层闭流形或多壳体相交重叠**：目录件或组合件（`NOZZ` / `SJOI` / `FLAN` / `PFIT` / `HELE` /
//!    `BPANEL`）往往把多个子基本体打进同一个 `FacetGroup`而不做布尔并，交叠区体积被重复计入；还有共面内贴边
//!    或零厚薄片在轮廓半边配对上留下边界边 / 非流形边。散度定理求得的不是占据空间体积，此时**只记账、不翻判档**。
//! 2. **曲面分格疏密不同**：两侧都是水密无重叠流形时，同一张圆弧 / 旋转体 / 样条面按不同段数折线化，体积天然差
//!    几个百分点（aps250162 的 GENPRI 包围盒全通过，但 8 段对 16 段差 +8.2%，4 段对 16 段差 −34.7%）。
//!    按世界系双向表面偏距 `d_s` 与面积 × 偏距体积上界判：同时满足
//!    `d_s ≤ facet_chord_limit_mm(noun)` 且 `|ΔV| ≤ A_avg · d_s` 时归为分格疏密差异，**只记账、不翻判档**。
//!
//! 真几何错（#127 切口摆反 180° 的 `d_s` = 34.6–52.7 mm，墙板漏开孔 `d_s` = 123–828 mm，套管壁厚错
//! `d_s` = 13.1–13.9 mm）两侧都是水密单壳且表面偏距远超分格弦高容差，照常判不一致。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use e3d_model::rvm_compare::chord_tolerance_mm;
use e3d_model::rvm_read::{Polygon, Primitive, RvmBytes, RvmGeometry, RvmScan};
use serde::{Deserialize, Serialize};

/// 体积差门限（相对值）：两侧体积差超过它进入复核。见模块文档的取值依据。
pub const VOLUME_DELTA_LIMIT: f64 = 0.0005;

/// 判档里的原因种类：体积差超门限（进 `rvm_check_mismatch` 的 `reason_kind`）。
pub const REASON_VOLUME_DIFFERS: &str = "volume_differs";

/// 轮廓顶点量化步长（1/100 mm = 0.01 mm）：RVM 顶点是单精度浮点数，相邻多边形共享顶点在世界系下的
/// 舍入残差在 1e-3 mm 量级，按 0.01 mm 格焊合即可准确配对轮廓半边，又远小于最小实体特征。
const WELD_GRID_PER_MM: f64 = 100.0;

/// 两个正体积连通壳在三轴方向上的包围盒重叠都超过此值（mm）时，视为多壳体内部相交重叠（散度体积含重复计入）。
/// 共面贴合的相邻壳在法向上的重叠不超过单精度舍入噪声（~1e-3 mm），不会被误判。
const SHELL_OVERLAP_TOL_MM: f64 = 0.1;

/// 二维截面轮廓与挤出圆弧的弦高容差（mm），对应 `e3d_model::discretise::PROFILE_CHORD_TOL_MM`（10.0 mm）。
/// `GENPRI` 之外的大量面片件（`SCTN` 的 `SANN` 圆管弧、`SBFI` 套管、`GENSEC` 扫掠弧、`FLAN` / `HANDRA`）
/// 均存在按 10 mm 轮廓弦高离散的圆弧面，不能只按曲面基本体的 0.5 mm 卡面片表面偏距。
const FACET_PROFILE_CHORD_MM: f64 = 10.0;

/// SMLib B-rep（`GENPRI`）导出面片的弦高容差（mm），对应 `e3d_model::genpri` 的 `E3D_EXPORT_CHORD_MM`（6.0 mm）。
const GENPRI_CHORD_MM: f64 = 6.0;

/// 一个成员算出来的体积。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemberVolume {
    /// 局部 mm³；不可比时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_mm3: Option<f64>,
    /// 不可比的原因（给人看的一句话）；可比时为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_not: Option<String>,
}

impl MemberVolume {
    fn measured(volume_mm3: f64) -> Self {
        Self {
            volume_mm3: Some(volume_mm3),
            why_not: None,
        }
    }

    fn unmeasured(why: impl Into<String>) -> Self {
        Self {
            volume_mm3: None,
            why_not: Some(why.into()),
        }
    }
}

/// 纯面片成员在世界系（mm）下的拓扑与表面几何摘要，只在两侧体积差超 `VOLUME_DELTA_LIMIT` 时用于区分
/// 「分格疏密 / 非流形多壳重叠」与「真几何错」。含任何解析图元记录的成员不建此结构（为 `None`）。
#[derive(Clone, Debug)]
struct FacetInspection {
    area_mm2: f64,
    unique_vertices: usize,
    /// 非退化三角形（世界 mm），供对侧表面采样点求最近距离。
    triangles: Vec<[[f64; 3]; 3]>,
    /// 表面采样点（世界 mm：去重轮廓顶点 + 轮廓边中点 + 独立三角形重心）。
    samples: Vec<[f64; 3]>,
    /// 轮廓半边没有边界边、非流形边或同向重边：闭合且绕向一致。
    watertight: bool,
    boundary_edges: usize,
    nonmanifold_edges: usize,
    winding_conflicts: usize,
    positive_shells: usize,
    /// 存在至少两个正体积连通壳在三维包围盒上实质重叠（未做布尔并的组合体）。
    overlapping_shells: bool,
}

impl FacetInspection {
    /// 散度定理求出的有向体积是否等于几何体真实占据的空间体积。
    fn has_trusted_solid_volume(&self) -> bool {
        self.watertight && !self.overlapping_shells
    }

    fn untrusted_reason(&self) -> String {
        if !self.watertight {
            format!(
                "非单层闭流形（边界边 {}、非流形边 {}、绕向冲突 {}）",
                self.boundary_edges, self.nonmanifold_edges, self.winding_conflicts
            )
        } else {
            format!("{} 个正体积壳体三维重叠（未布尔并）", self.positive_shells)
        }
    }
}

/// 一份 RVM 里各成员的体积：按成员路径、按显示名归一后的主键各一份索引，外加路径 → 主键的对应。
#[derive(Clone, Debug, Default)]
pub struct RvmVolumes {
    by_path: HashMap<String, MemberVolume>,
    by_key: HashMap<String, MemberVolume>,
    keys_by_path: HashMap<String, String>,
    facets_by_path: HashMap<String, FacetInspection>,
    facets_by_key: HashMap<String, FacetInspection>,
}

impl RvmVolumes {
    pub fn by_path(&self, path: &str) -> Option<&MemberVolume> {
        self.by_path.get(path.trim())
    }

    pub fn by_key(&self, key: &str) -> Option<&MemberVolume> {
        self.by_key.get(key)
    }

    /// 成员路径 → 该成员显示名归一后的主键。
    pub fn key_of_path(&self, path: &str) -> Option<&str> {
        self.keys_by_path.get(path.trim()).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

/// 显示名归一成主键：`TUBE 3 of BRANCH /X` → `TUBI 3 BRANCH /X`，命名元素原样（`/X`）。
///
/// 与 vendor `rvm_compare` 建表同一口径：`NOUN n of OWNER` 取 noun 前四字（`TUBE` 归一成 `TUBI`）、
/// 序号、属主键；其余按名字。属主键只保留命名属主的 `/名`，无名属主保留每一层 `noun ordinal of`。
pub fn display_key(name: &str) -> String {
    let name = name.trim();
    match name.split_once(" of ") {
        Some((head, owner)) => match head.rsplit_once(' ') {
            Some((noun, ordinal)) if ordinal.trim().parse::<usize>().is_ok() => {
                let noun = if noun.trim() == "TUBE" {
                    "TUBI".to_string()
                } else {
                    noun.trim().chars().take(4).collect::<String>()
                };
                format!("{} {} {}", noun, ordinal.trim(), owner_key(owner))
            }
            _ => name.to_string(),
        },
        None => name.to_string(),
    }
}

/// 属主显示名归一化为稳定键（与 vendor `owner_key` 同一口径）。
fn owner_key(desc: &str) -> String {
    let desc = desc.trim();
    if let Some((left, parent)) = desc.split_once(" of ")
        && let Some((noun, ordinal)) = left.rsplit_once(' ')
        && ordinal.parse::<usize>().is_ok()
    {
        return format!(
            "{} {} of {}",
            noun.chars().take(4).collect::<String>(),
            ordinal,
            owner_key(parent)
        );
    }
    match desc.rsplit_once(' ') {
        Some((noun, name)) if !noun.contains(' ') && name.starts_with('/') => name.to_string(),
        _ => desc.to_string(),
    }
}

/// 一条 FacetGroup 轮廓按扇形三角化累加有向体积（散度定理）。
fn facet_volume(polygons: &[Polygon]) -> f64 {
    let mut sum = 0.0;
    for polygon in polygons {
        for contour in &polygon.contours {
            let points = &contour.vertices;
            if points.len() < 3 {
                continue;
            }
            let a = points[0];
            for pair in points[1..].windows(2) {
                let (b, c) = (pair[0], pair[1]);
                sum += dot(a, cross(b, c)) / 6.0;
            }
        }
    }
    sum
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn quantize_mm(p: [f64; 3]) -> [i64; 3] {
    [
        (p[0] * WELD_GRID_PER_MM).round() as i64,
        (p[1] * WELD_GRID_PER_MM).round() as i64,
        (p[2] * WELD_GRID_PER_MM).round() as i64,
    ]
}

fn quantize_sample(p: [f64; 3]) -> [i64; 3] {
    [
        (p[0] * 1000.0).round() as i64,
        (p[1] * 1000.0).round() as i64,
        (p[2] * 1000.0).round() as i64,
    ]
}

fn aabb_volume(points: &[[f64; 3]]) -> f64 {
    if points.is_empty() {
        return 0.0;
    }
    let mut min = points[0];
    let mut max = points[0];
    for point in points {
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    (0..3).map(|axis| (max[axis] - min[axis]).abs()).product()
}

/// 单条几何记录的体积。不可比时给原因。
fn record_volume(geometry: &RvmGeometry) -> MemberVolume {
    match &geometry.primitive {
        Primitive::Box([x, y, z]) => MemberVolume::measured(x.abs() * y.abs() * z.abs()),
        Primitive::Cylinder([r, h]) => {
            MemberVolume::measured(std::f64::consts::PI * r * r * h.abs())
        }
        Primitive::Sphere([r]) => {
            MemberVolume::measured(4.0 / 3.0 * std::f64::consts::PI * r * r * r)
        }
        // 棱台：上下面平行、侧面是直纹面，按棱台（prismatoid）公式——h/6 ·(A底 + A顶 + 4·A中)。
        Primitive::Pyramid([bx, by, tx, ty, _ox, _oy, h]) => {
            let bottom = bx.abs() * by.abs();
            let top = tx.abs() * ty.abs();
            let middle = ((bx.abs() + tx.abs()) / 2.0) * ((by.abs() + ty.abs()) / 2.0);
            MemberVolume::measured(h.abs() / 6.0 * (bottom + top + 4.0 * middle))
        }
        // 圆台：π·h/3 ·(r底² + r底·r顶 + r顶²)。偏心 / 斜切不改体积。
        Primitive::Snout([rb, rt, h, _ox, _oy, _bsx, _bsy, _tsx, _tsy]) => MemberVolume::measured(
            std::f64::consts::PI * h.abs() / 3.0 * (rb * rb + rb * rt + rt * rt),
        ),
        // 圆环：管截面 πr² 绕轴扫过角度 θ，重心半径 `offset`。整圈 θ = 2π。
        Primitive::CircularTorus([offset, radius, angle]) => {
            let sweep = sweep_radians(*angle);
            MemberVolume::measured(sweep * offset.abs() * std::f64::consts::PI * radius * radius)
        }
        // 矩形环：截面 (outer−inner)·h，重心半径 (inner+outer)/2。
        Primitive::RectangularTorus([inner, outer, h, angle]) => {
            let sweep = sweep_radians(*angle);
            let section = (outer - inner).abs() * h.abs();
            MemberVolume::measured(sweep * ((inner + outer) / 2.0).abs() * section)
        }
        // 球冠：π·h²·(3R − h)/3，R = (r² + h²)/(2h) ⇒ π·h·(3r² + h²)/6。
        Primitive::SphericalDish([r, h]) => {
            MemberVolume::measured(std::f64::consts::PI * h.abs() * (3.0 * r * r + h * h) / 6.0)
        }
        Primitive::FacetGroup(polygons) => {
            let volume = facet_volume(polygons).abs();
            let points: Vec<[f64; 3]> = polygons
                .iter()
                .flat_map(|polygon| polygon.contours.iter())
                .flat_map(|contour| contour.vertices.iter().copied())
                .collect();
            let boxed = aabb_volume(&points);
            // 开面 / 纯平面：有向体积不可信，不能拿来判。留一个数量级的余量。
            if !(volume > boxed * 1e-6) {
                return MemberVolume::unmeasured("面片不闭合或退化成平面，算不出体积");
            }
            MemberVolume::measured(volume)
        }
        Primitive::Line(_) => MemberVolume::unmeasured("只有中心线，没有体"),
        Primitive::EllipticalDish(_) => {
            MemberVolume::unmeasured("折边碟（EllipticalDish）的体积还没算")
        }
        Primitive::Unknown { kind_id, .. } => {
            MemberVolume::unmeasured(format!("不认识的图元 kind {kind_id}"))
        }
    }
}

/// 扫过角度换成弧度。vendor 在 1e-6 度内把 0 与 360 都当整圈。
fn sweep_radians(angle_deg: f64) -> f64 {
    let sweep = angle_deg.abs();
    if sweep <= 1e-6 || (sweep - 360.0).abs() <= 1e-6 || sweep > 360.0 {
        std::f64::consts::TAU
    } else {
        sweep.to_radians()
    }
}

struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            let grand = self.parent[self.parent[x]];
            self.parent[x] = grand;
            x = grand;
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

/// 从成员的全部世界系多边形（`Vec<Vec<Vec<[f64; 3]>>>`：多边形 → 轮廓 → 世界顶点）构建拓扑与表面采样摘要。
///
/// 半边检查按**多边形的原始轮廓边**做，而非扇形剖分后的内部对角线：E3D 导出的带孔平面（如法兰孔、墙洞端面）
/// 用「外环 + 反向内孔环」表示，直接扇形剖分内孔会产出一组穿过孔口的对角线，唯有原始轮廓边才是面的真实边界。
fn inspect_world_polygons(world_polygons: &[Vec<Vec<[f64; 3]>>]) -> Option<FacetInspection> {
    let mut valid_polys: Vec<Vec<(Vec<[i64; 3]>, &[([f64; 3])])>> = Vec::new();
    let mut triangles: Vec<[[f64; 3]; 3]> = Vec::new();
    let mut unique_verts: HashSet<[i64; 3]> = HashSet::new();
    let mut seen_samples: HashSet<[i64; 3]> = HashSet::new();
    let mut samples: Vec<[f64; 3]> = Vec::new();
    let mut area_mm2 = 0.0;

    let mut push_sample = |p: [f64; 3]| {
        if seen_samples.insert(quantize_sample(p)) {
            samples.push(p);
        }
    };

    for poly in world_polygons {
        let mut contours_out = Vec::new();
        for contour in poly {
            let m = contour.len();
            if m < 3 {
                continue;
            }
            let mut welded: Vec<[i64; 3]> = Vec::with_capacity(m);
            for idx in 0..m {
                let p = contour[idx];
                let q = quantize_mm(p);
                unique_verts.insert(q);
                push_sample(p);
                let next = contour[(idx + 1) % m];
                push_sample([
                    0.5 * (p[0] + next[0]),
                    0.5 * (p[1] + next[1]),
                    0.5 * (p[2] + next[2]),
                ]);
                if welded.last().copied() != Some(q) {
                    welded.push(q);
                }
            }
            if welded.len() >= 2 && welded.first() == welded.last() {
                welded.pop();
            }
            if welded.len() >= 3 {
                contours_out.push((welded, contour.as_slice()));
            }
            if poly.len() == 1 && m == 3 {
                let (a, b, c) = (contour[0], contour[1], contour[2]);
                push_sample([
                    (a[0] + b[0] + c[0]) / 3.0,
                    (a[1] + b[1] + c[1]) / 3.0,
                    (a[2] + b[2] + c[2]) / 3.0,
                ]);
            }
            let a = contour[0];
            for pair in contour[1..].windows(2) {
                let (b, c) = (pair[0], pair[1]);
                let tri_area = 0.5 * norm(cross(sub(b, a), sub(c, a)));
                if tri_area > 1e-9 {
                    area_mm2 += tri_area;
                    triangles.push([a, b, c]);
                }
            }
        }
        if !contours_out.is_empty() {
            valid_polys.push(contours_out);
        }
    }

    if valid_polys.is_empty() || triangles.is_empty() {
        return None;
    }

    let mut dsu = Dsu::new(valid_polys.len());
    let mut directed: HashMap<([i64; 3], [i64; 3]), usize> = HashMap::new();
    let mut undirected: HashMap<([i64; 3], [i64; 3]), usize> = HashMap::new();
    let mut edge_owner: HashMap<([i64; 3], [i64; 3]), usize> = HashMap::new();

    for (pi, contours) in valid_polys.iter().enumerate() {
        for (pts, _) in contours {
            let m = pts.len();
            for idx in 0..m {
                let u = pts[idx];
                let v = pts[(idx + 1) % m];
                if u == v {
                    continue;
                }
                *directed.entry((u, v)).or_insert(0) += 1;
                let ue = if u < v { (u, v) } else { (v, u) };
                *undirected.entry(ue).or_insert(0) += 1;
                if let Some(&first) = edge_owner.get(&ue) {
                    dsu.union(pi, first);
                } else {
                    edge_owner.insert(ue, pi);
                }
            }
        }
    }

    let boundary_edges = undirected.values().filter(|&&c| c == 1).count();
    let nonmanifold_edges = undirected.values().filter(|&&c| c > 2).count();
    let winding_conflicts = directed
        .iter()
        .filter(|&(&(u, v), &c)| c > 1 || directed.get(&(v, u)).copied().unwrap_or(0) != c)
        .count();
    let watertight = boundary_edges == 0 && nonmanifold_edges == 0 && winding_conflicts == 0;

    let mut shell_polys: HashMap<usize, Vec<usize>> = HashMap::new();
    for pi in 0..valid_polys.len() {
        shell_polys.entry(dsu.find(pi)).or_default().push(pi);
    }

    let origin = triangles[0][0];
    let mut pos_boxes: Vec<[f64; 6]> = Vec::new();
    for indices in shell_polys.values() {
        let mut signed_vol = 0.0;
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for &pi in indices {
            for (_, raw_contour) in &valid_polys[pi] {
                if raw_contour.len() < 3 {
                    continue;
                }
                for p in *raw_contour {
                    for axis in 0..3 {
                        min[axis] = min[axis].min(p[axis]);
                        max[axis] = max[axis].max(p[axis]);
                    }
                }
                let a = sub(raw_contour[0], origin);
                for pair in raw_contour[1..].windows(2) {
                    let b = sub(pair[0], origin);
                    let c = sub(pair[1], origin);
                    signed_vol += dot(a, cross(b, c)) / 6.0;
                }
            }
        }
        if signed_vol > 1.0 {
            pos_boxes.push([min[0], min[1], min[2], max[0], max[1], max[2]]);
        }
    }

    let mut overlapping_shells = false;
    'outer: for i in 0..pos_boxes.len() {
        for j in (i + 1)..pos_boxes.len() {
            let b1 = &pos_boxes[i];
            let b2 = &pos_boxes[j];
            if (0..3).all(|ax| b1[ax + 3].min(b2[ax + 3]) - b1[ax].max(b2[ax]) > SHELL_OVERLAP_TOL_MM)
            {
                overlapping_shells = true;
                break 'outer;
            }
        }
    }

    Some(FacetInspection {
        area_mm2,
        unique_vertices: unique_verts.len(),
        triangles,
        samples,
        watertight,
        boundary_edges,
        nonmanifold_edges,
        winding_conflicts,
        positive_shells: pos_boxes.len(),
        overlapping_shells,
    })
}

/// 点 `p` 到三角形 `(a, b, c)` 的精确欧氏距离平方（Christer Ericson, *Real-Time Collision Detection*）。
fn point_to_triangle_dist_sq(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return dot(ap, ap);
    }

    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return dot(bp, bp);
    }

    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 && (d1 - d3) > 0.0 {
        let v = d1 / (d1 - d3);
        let proj = [a[0] + v * ab[0], a[1] + v * ab[1], a[2] + v * ab[2]];
        let d = sub(p, proj);
        return dot(d, d);
    }

    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return dot(cp, cp);
    }

    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 && (d2 - d6) > 0.0 {
        let w = d2 / (d2 - d6);
        let proj = [a[0] + w * ac[0], a[1] + w * ac[1], a[2] + w * ac[2]];
        let d = sub(p, proj);
        return dot(d, d);
    }

    let va = d3 * d6 - d5 * d4;
    let denom_edge = (d4 - d3) + (d5 - d6);
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 && denom_edge > 0.0 {
        let w = (d4 - d3) / denom_edge;
        let bc = sub(c, b);
        let proj = [b[0] + w * bc[0], b[1] + w * bc[1], b[2] + w * bc[2]];
        let d = sub(p, proj);
        return dot(d, d);
    }

    let sum = va + vb + vc;
    if sum <= 0.0 {
        return dot(ap, ap);
    }
    let inv = 1.0 / sum;
    let v = vb * inv;
    let w = vc * inv;
    let proj = [
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ];
    let d = sub(p, proj);
    dot(d, d)
}

fn directed_surface_distance_mm(samples: &[[f64; 3]], triangles: &[[[f64; 3]; 3]]) -> f64 {
    if samples.is_empty() || triangles.is_empty() {
        return 0.0;
    }
    let mut max_d2 = 0.0f64;
    for &p in samples {
        let mut best = f64::INFINITY;
        for &[a, b, c] in triangles {
            let d2 = point_to_triangle_dist_sq(p, a, b, c);
            if d2 < best {
                best = d2;
                if best <= 1e-6 {
                    break;
                }
            }
        }
        if best.is_finite() && best > max_d2 {
            max_d2 = best;
        }
    }
    max_d2.sqrt()
}

/// 面片分格允许的最大双向表面偏距（mm）。
///
/// - `GENPRI`（SMLib B-rep）：导出弦高 `E3D_EXPORT_CHORD_MM = 6.0 mm`；
/// - 其余面片构件：截面 / 轮廓圆弧离散容差 `PROFILE_CHORD_TOL_MM = 10.0 mm`（与 `chord_tolerance_mm(noun)` 取大）。
pub fn facet_chord_limit_mm(noun: &str) -> f64 {
    let prefix: String = noun
        .trim()
        .to_ascii_uppercase()
        .chars()
        .take(4)
        .collect();
    if prefix == "GENP" {
        GENPRI_CHORD_MM
    } else {
        chord_tolerance_mm(noun).max(FACET_PROFILE_CHORD_MM)
    }
}

/// 一个成员的体积与（若全为 `FacetGroup`）世界系面片摘要：各条记录相加；有一条不可比就整件不可比。
fn member_volume_and_facets(
    scan: &RvmScan,
    bytes: &[u8],
    member: &e3d_model::rvm_read::ScannedMember,
) -> (MemberVolume, Option<FacetInspection>) {
    let _ = scan;
    if member.records.is_empty() {
        return (MemberVolume::unmeasured("没有几何记录"), None);
    }
    let mut total = 0.0;
    let mut all_facets = true;
    let mut world_polygons: Vec<Vec<Vec<[f64; 3]>>> = Vec::new();

    for record in &member.records {
        let geometry = match record.decode_without_normals(bytes) {
            Ok(geometry) => geometry,
            Err(error) => {
                return (
                    MemberVolume::unmeasured(format!("记录解不出来：{error:#}")),
                    None,
                );
            }
        };
        match record_volume(&geometry) {
            MemberVolume {
                volume_mm3: Some(volume),
                ..
            } => total += volume,
            MemberVolume { why_not, .. } => {
                return (
                    MemberVolume::unmeasured(
                        why_not.unwrap_or_else(|| "有记录算不出体积".to_string()),
                    ),
                    None,
                );
            }
        }
        if let Primitive::FacetGroup(polygons) = &geometry.primitive {
            if all_facets {
                for poly in polygons {
                    let contours: Vec<Vec<[f64; 3]>> = poly
                        .contours
                        .iter()
                        .map(|c| c.vertices.iter().map(|&v| geometry.to_world_mm(v)).collect())
                        .collect();
                    world_polygons.push(contours);
                }
            }
        } else {
            all_facets = false;
            world_polygons.clear();
        }
    }

    let facets = if all_facets {
        inspect_world_polygons(&world_polygons)
    } else {
        None
    };
    (MemberVolume::measured(total), facets)
}

/// 读一份 RVM，算出每个成员的体积（及纯面片成员的拓扑/采样摘要），并按成员路径与主键各建一份索引。
pub fn read_volumes(path: &Path) -> Result<RvmVolumes, String> {
    let bytes = RvmBytes::open(path).map_err(|error| format!("读 {} 失败：{error:#}", path.display()))?;
    let scan = RvmScan::scan(&bytes)
        .map_err(|error| format!("解析 {} 失败：{error:#}", path.display()))?;
    let mut volumes = RvmVolumes::default();
    for member in &scan.members {
        let (volume, facets) = member_volume_and_facets(&scan, &bytes, member);
        let path = member.path.trim().to_string();
        let key = display_key(&member.name);
        volumes.by_path.insert(path.clone(), volume.clone());
        volumes.keys_by_path.insert(path.clone(), key.clone());
        volumes.by_key.entry(key.clone()).or_insert(volume);
        if let Some(facets) = facets {
            volumes.facets_by_path.insert(path, facets.clone());
            volumes.facets_by_key.entry(key).or_insert(facets);
        }
    }
    Ok(volumes)
}

/// 一对构件的体积比对结果。
#[derive(Clone, Debug, PartialEq)]
pub struct VolumeVerdict {
    pub baseline_mm3: Option<f64>,
    pub generated_mm3: Option<f64>,
    /// 相对差（|生成 − 基准| / 基准）；两侧都比得出来才有。
    pub delta: Option<f64>,
    /// 只记账、不改判档（两侧面片分格疏密不同，或面片含非流形边/重叠子壳使散度体积不代表占据空间）。
    pub recorded_only: bool,
    /// 不可比、没超门限、只记账或超门限时的一句话说明。
    pub note: String,
    /// 超门限且不属于面片分格/多壳表示差异：这一对判不一致（把判档翻成 `off`）。
    pub differs: bool,
}

/// 两侧都是纯面片、且相对体积差已超 `VOLUME_DELTA_LIMIT` 时，判断是否属于「只记账、不翻判档」的两类面片表示差异；
/// 若是则返回完整的记账说明，若属于真几何差异则返回 `(表面偏距说明, None)`。
fn evaluate_facet_pair(
    base: &FacetInspection,
    ours: &FacetInspection,
    baseline_mm3: f64,
    generated_mm3: f64,
    percent: f64,
    noun: &str,
) -> Result<String, String> {
    if !base.has_trusted_solid_volume() || !ours.has_trusted_solid_volume() {
        let mut sides = Vec::new();
        if !base.has_trusted_solid_volume() {
            sides.push(format!("基准{}", base.untrusted_reason()));
        }
        if !ours.has_trusted_solid_volume() {
            sides.push(format!("生成{}", ours.untrusted_reason()));
        }
        return Ok(format!(
            "只记账（不改判档）：两侧面片体积差 {percent:.3}%（基准 {baseline_mm3:.0} mm³ / {} 顶点，生成 {generated_mm3:.0} mm³ / {} 顶点），\
             但{}——散度定理求出的不是占据空间体积",
            base.unique_vertices,
            ours.unique_vertices,
            sides.join("；")
        ));
    }

    let d_bg = directed_surface_distance_mm(&base.samples, &ours.triangles);
    let d_gb = directed_surface_distance_mm(&ours.samples, &base.triangles);
    let surface_delta_mm = d_bg.max(d_gb);
    let chord_limit_mm = facet_chord_limit_mm(noun);
    let abs_dv = (generated_mm3 - baseline_mm3).abs();
    let avg_area = 0.5 * (base.area_mm2 + ours.area_mm2);
    let chord_volume_cap = avg_area * surface_delta_mm;

    if surface_delta_mm <= chord_limit_mm && abs_dv <= chord_volume_cap {
        return Ok(format!(
            "只记账（不改判档）：两侧面片分格疏密不同，体积差 {percent:.3}%（基准 {baseline_mm3:.0} mm³ / {} 顶点，生成 {generated_mm3:.0} mm³ / {} 顶点），\
             双向表面最大偏距 {surface_delta_mm:.3} mm ≤ 分格弦高容差 {chord_limit_mm:.1} mm，\
             |ΔV| = {abs_dv:.0} mm³ ≤ 面积×偏距上界 {chord_volume_cap:.0} mm³",
            base.unique_vertices, ours.unique_vertices
        ));
    }

    Err(format!(
        "两侧均为水密单壳面片（顶点 {}/{}），双向表面最大偏距 {surface_delta_mm:.3} mm（分格弦高容差 {chord_limit_mm:.1} mm）；{}",
        base.unique_vertices,
        ours.unique_vertices,
        volume_hint(noun)
    ))
}

/// 比一对：`generated_path` 是报告的 `pairs[].refno`（生成成员路径），基准侧按同一主键去查。
/// 两侧任一算不出体积就返回「没比」——**不能当成通过**。
pub fn compare_pair(
    baseline: &RvmVolumes,
    generated: &RvmVolumes,
    generated_path: &str,
    noun: &str,
) -> VolumeVerdict {
    let trimmed_path = generated_path.trim();
    let key = generated.key_of_path(trimmed_path);
    let ours = generated.by_path(trimmed_path);
    let theirs = key.and_then(|key| baseline.by_key(key));
    let (baseline_mm3, generated_mm3) = match (theirs, ours) {
        (Some(theirs), Some(ours)) => (theirs.volume_mm3, ours.volume_mm3),
        _ => (None, None),
    };
    match (baseline_mm3, generated_mm3) {
        (Some(baseline_mm3), Some(generated_mm3)) if baseline_mm3 > 1.0 => {
            let delta = (generated_mm3 - baseline_mm3).abs() / baseline_mm3;
            let percent = delta * 100.0;
            if delta > VOLUME_DELTA_LIMIT {
                let facet_pair = key.and_then(|k| {
                    Some((
                        baseline.facets_by_key.get(k)?,
                        generated.facets_by_path.get(trimmed_path)?,
                    ))
                });
                if let Some((base_facets, ours_facets)) = facet_pair {
                    match evaluate_facet_pair(
                        base_facets,
                        ours_facets,
                        baseline_mm3,
                        generated_mm3,
                        percent,
                        noun,
                    ) {
                        Ok(ledger_note) => {
                            return VolumeVerdict {
                                baseline_mm3: Some(baseline_mm3),
                                generated_mm3: Some(generated_mm3),
                                delta: Some(delta),
                                recorded_only: true,
                                note: ledger_note,
                                differs: false,
                            };
                        }
                        Err(detail) => {
                            return VolumeVerdict {
                                baseline_mm3: Some(baseline_mm3),
                                generated_mm3: Some(generated_mm3),
                                delta: Some(delta),
                                recorded_only: false,
                                note: format!(
                                    "体积差 {percent:.3}% 超过门限 {:.3}%：基准 {baseline_mm3:.0} mm³，生成 {generated_mm3:.0} mm³（{detail}）",
                                    VOLUME_DELTA_LIMIT * 100.0,
                                ),
                                differs: true,
                            };
                        }
                    }
                }

                VolumeVerdict {
                    baseline_mm3: Some(baseline_mm3),
                    generated_mm3: Some(generated_mm3),
                    delta: Some(delta),
                    recorded_only: false,
                    note: format!(
                        "体积差 {percent:.3}% 超过门限 {:.3}%：基准 {baseline_mm3:.0} mm³，生成 {generated_mm3:.0} mm³（{}）",
                        VOLUME_DELTA_LIMIT * 100.0,
                        volume_hint(noun)
                    ),
                    differs: true,
                }
            } else {
                VolumeVerdict {
                    baseline_mm3: Some(baseline_mm3),
                    generated_mm3: Some(generated_mm3),
                    delta: Some(delta),
                    recorded_only: false,
                    note: format!(
                        "体积差 {percent:.3}%，在门限 {:.3}% 内",
                        VOLUME_DELTA_LIMIT * 100.0
                    ),
                    differs: false,
                }
            }
        }
        (baseline_mm3, generated_mm3) => {
            let mut reasons = Vec::new();
            if baseline_mm3.is_none() {
                reasons.push(
                    theirs
                        .and_then(|volume| volume.why_not.clone())
                        .unwrap_or_else(|| {
                            "基准侧没算出来（主键对不上或没有这个成员）".to_string()
                        }),
                );
            }
            if generated_mm3.is_none() {
                reasons.push(
                    ours.and_then(|volume| volume.why_not.clone())
                        .unwrap_or_else(|| "生成侧没算出来".to_string()),
                );
            }
            if baseline_mm3.is_some_and(|volume| volume <= 1.0) {
                reasons.push("基准体积接近 0".to_string());
            }
            VolumeVerdict {
                baseline_mm3,
                generated_mm3,
                delta: None,
                recorded_only: false,
                note: format!("没有比体积：{}", reasons.join("；")),
                differs: false,
            }
        }
    }
}

/// 判档建议（只写进说明，不改判档）。
fn volume_hint(noun: &str) -> String {
    format!(
        "容差档 {} mm 管不到这种差异：包围盒相同、材料分布不同（切口摆反、局部翻转一类）",
        chord_tolerance_mm(noun)
    )
}

/// 作业目录里的体积复核明细账：每条被改判 `off` 或「只记账、不改判档」的构件一行，不静默。
pub const VOLUME_LEDGER_FILE: &str = "volume-ledger.json";

/// 一条体积超 `VOLUME_DELTA_LIMIT` 的复核记录（落 `volume-ledger.json`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VolumeLedgerEntry {
    pub key: String,
    pub path: String,
    pub noun: String,
    pub baseline_mm3: f64,
    pub generated_mm3: f64,
    pub delta: f64,
    pub flipped_to_off: bool,
    pub note: String,
}

/// 对一份比对报告里配对成功、包围盒通过的构件做体积复核（GitHub #127 / #171）。
///
/// - 命名元素的 `pair.noun` 在 RVM 比对报告里为空串，这里优先从 `sources`（`member-sources.json`）接回真实 noun；
/// - 真几何差异（`verdict.differs`）把 `pair.verdict` 改成 `"off"`，同时更新 `report.by_noun` 的计数；
/// - 两侧面片分格疏密不同或多壳非流形重叠（`verdict.recorded_only`）不动判档，逐条写进 `<task_dir>/volume-ledger.json`
///   并在标准错误打印汇总，不静默。
pub fn apply_to_report(
    task_dir: &Path,
    report: &mut e3d_model::rvm_compare::CompareReport,
    sources: &[e3d_model::rvm_export::MemberSource],
) {
    let baseline_path = task_dir.join("e3d.rvm");
    let generated_path = task_dir.join("ours").join("generated.rvm");
    let (Ok(baseline), Ok(generated)) = (read_volumes(&baseline_path), read_volumes(&generated_path))
    else {
        // 读不动就整条跳过：体积是加严判据，读不到不该让整次核对失败。
        return;
    };
    let noun_by_path: HashMap<&str, &str> = sources
        .iter()
        .filter(|s| !s.noun.is_empty())
        .map(|s| (s.path.as_str(), s.noun.as_str()))
        .collect();

    let mut flipped = 0usize;
    let mut recorded_only = 0usize;
    let mut ledger: Vec<VolumeLedgerEntry> = Vec::new();

    for pair in report.pairs.iter_mut() {
        if pair.verdict == "off" || pair.aabb_source == "lines" {
            continue;
        }
        let noun = if pair.noun.is_empty() {
            noun_by_path
                .get(pair.refno.trim())
                .copied()
                .unwrap_or("")
        } else {
            pair.noun.as_str()
        };
        let verdict = compare_pair(&baseline, &generated, &pair.refno, noun);
        if verdict.differs {
            let old_verdict = pair.verdict;
            pair.verdict = "off";
            let tally_key: String = pair.noun.chars().take(4).collect();
            if let Some(tally) = report.by_noun.get_mut(&tally_key) {
                match old_verdict {
                    "exact" => tally.exact = tally.exact.saturating_sub(1),
                    "chord" => tally.chord = tally.chord.saturating_sub(1),
                    "reference_artifact" => {
                        tally.reference_artifact = tally.reference_artifact.saturating_sub(1);
                    }
                    "film" => tally.film = tally.film.saturating_sub(1),
                    _ => {}
                }
                tally.off += 1;
            }
            flipped += 1;
            if let (Some(b), Some(g), Some(d)) =
                (verdict.baseline_mm3, verdict.generated_mm3, verdict.delta)
            {
                ledger.push(VolumeLedgerEntry {
                    key: pair.key.clone(),
                    path: pair.refno.clone(),
                    noun: noun.to_string(),
                    baseline_mm3: b,
                    generated_mm3: g,
                    delta: d,
                    flipped_to_off: true,
                    note: verdict.note,
                });
            }
        } else if verdict.recorded_only {
            recorded_only += 1;
            if let (Some(b), Some(g), Some(d)) =
                (verdict.baseline_mm3, verdict.generated_mm3, verdict.delta)
            {
                ledger.push(VolumeLedgerEntry {
                    key: pair.key.clone(),
                    path: pair.refno.clone(),
                    noun: noun.to_string(),
                    baseline_mm3: b,
                    generated_mm3: g,
                    delta: d,
                    flipped_to_off: false,
                    note: verdict.note,
                });
            }
        }
    }

    if !ledger.is_empty()
        && let Ok(bytes) = serde_json::to_vec_pretty(&ledger)
    {
        let _ = std::fs::write(task_dir.join(VOLUME_LEDGER_FILE), bytes);
    }
    if flipped > 0 {
        eprintln!(
            "体积复核：{flipped} 件包围盒通过但体积超门限，判档改成 off（另 {recorded_only} 件面片表示/分格差异只记账，详见 {VOLUME_LEDGER_FILE}）"
        );
    } else if recorded_only > 0 {
        eprintln!(
            "体积复核：{recorded_only} 件面片分格疏密或多壳表示不同，只记账不改判档（详见 {VOLUME_LEDGER_FILE}）"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use e3d_model::rvm_read::Contour;

    fn geometry(primitive: Primitive) -> RvmGeometry {
        RvmGeometry {
            tag: "PRIM".to_string(),
            flags: 0,
            primitive,
            matrix3: [0.001, 0.0, 0.0, 0.0, 0.001, 0.0, 0.0, 0.0, 0.001],
            translation_mm: [0.0; 3],
            record_bbox: [0.0; 6],
            bbox_frame: e3d_model::rvm_read::BboxFrame::Local,
            level: None,
        }
    }

    fn insert_facet_member(
        store: &mut RvmVolumes,
        path: &str,
        key: &str,
        polygons: Vec<Polygon>,
    ) {
        let geom = geometry(Primitive::FacetGroup(polygons.clone()));
        let vol = record_volume(&geom);
        let world_polys: Vec<Vec<Vec<[f64; 3]>>> = polygons
            .iter()
            .map(|p| {
                p.contours
                    .iter()
                    .map(|c| c.vertices.iter().map(|&v| geom.to_world_mm(v)).collect())
                    .collect()
            })
            .collect();
        let facets = inspect_world_polygons(&world_polys).expect("有效面片");
        store.by_path.insert(path.to_string(), vol.clone());
        store.keys_by_path.insert(path.to_string(), key.to_string());
        store.by_key.insert(key.to_string(), vol);
        store.facets_by_path.insert(path.to_string(), facets.clone());
        store.facets_by_key.insert(key.to_string(), facets);
    }

    /// 绕 Z 轴的正 n 边形直棱柱（半径 `r`、高 `0..h`），外法向朝外，水密单壳。
    fn regular_prism_polygons(n: usize, r: f64, h: f64) -> Vec<Polygon> {
        let ring = |z: f64| -> Vec<[f64; 3]> {
            (0..n)
                .map(|i| {
                    let theta = std::f64::consts::TAU * i as f64 / n as f64;
                    [r * theta.cos(), r * theta.sin(), z]
                })
                .collect()
        };
        let bot = ring(0.0);
        let top = ring(h);
        let mut polys = Vec::with_capacity(n + 2);
        // 底面（外法向 -Z：顺时针）
        let mut bot_rev = bot.clone();
        bot_rev.reverse();
        polys.push(Polygon {
            contours: vec![Contour {
                vertices: bot_rev,
                normals: Vec::new(),
            }],
        });
        // 顶面（外法向 +Z：逆时针）
        polys.push(Polygon {
            contours: vec![Contour {
                vertices: top.clone(),
                normals: Vec::new(),
            }],
        });
        // n 张侧面四边形（外法向朝外）
        for i in 0..n {
            let j = (i + 1) % n;
            polys.push(Polygon {
                contours: vec![Contour {
                    vertices: vec![bot[i], bot[j], top[j], top[i]],
                    normals: Vec::new(),
                }],
            });
        }
        polys
    }

    fn world_polys_to_local_polygons(raw: &[Vec<Vec<[f64; 3]>>]) -> Vec<Polygon> {
        raw.iter()
            .map(|poly| Polygon {
                contours: poly
                    .iter()
                    .map(|c| Contour {
                        vertices: c.clone(),
                        normals: Vec::new(),
                    })
                    .collect(),
            })
            .collect()
    }

    /// GitHub #127：配上的构件再比一次体积。包围盒相同的两件，体积差超门限就判不一致并写清数值；
    /// 门限内不判；两侧任一侧算不出体积时**不算通过**，写明没比。
    #[test]
    fn a_paired_element_is_compared_by_volume_after_its_box_passes() {
        // 解析图元的闭式体积。
        let boxed = record_volume(&geometry(Primitive::Box([10.0, 20.0, 30.0])));
        assert_eq!(boxed.volume_mm3, Some(6000.0));
        let cylinder = record_volume(&geometry(Primitive::Cylinder([100.0, 50.0])));
        assert!(
            (cylinder.volume_mm3.unwrap() - std::f64::consts::PI * 100.0 * 100.0 * 50.0).abs()
                < 1e-6
        );
        let sphere = record_volume(&geometry(Primitive::Sphere([10.0])));
        assert!(
            (sphere.volume_mm3.unwrap() - 4.0 / 3.0 * std::f64::consts::PI * 1000.0).abs() < 1e-6
        );
        // 圆台 / 棱台 / 圆环 / 矩形环 / 球冠。
        let snout = record_volume(&geometry(Primitive::Snout([
            10.0, 20.0, 30.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ])));
        assert!(
            (snout.volume_mm3.unwrap()
                - std::f64::consts::PI * 30.0 / 3.0 * (100.0 + 200.0 + 400.0))
                .abs()
                < 1e-6
        );
        let pyramid = record_volume(&geometry(Primitive::Pyramid([
            10.0, 10.0, 5.0, 5.0, 0.0, 0.0, 12.0,
        ])));
        assert!(
            (pyramid.volume_mm3.unwrap() - 12.0 / 6.0 * (100.0 + 25.0 + 4.0 * 56.25)).abs() < 1e-9,
            "{pyramid:?}"
        );
        let torus = record_volume(&geometry(Primitive::CircularTorus([500.0, 20.0, 360.0])));
        assert!(
            (torus.volume_mm3.unwrap() - std::f64::consts::TAU * 500.0 * std::f64::consts::PI * 400.0)
                .abs()
                < 1e-6
        );
        let rect_torus = record_volume(&geometry(Primitive::RectangularTorus([
            100.0, 200.0, 50.0, 360.0,
        ])));
        assert!(
            (rect_torus.volume_mm3.unwrap() - std::f64::consts::TAU * 150.0 * 100.0 * 50.0).abs()
                < 1e-6
        );
        let dish = record_volume(&geometry(Primitive::SphericalDish([100.0, 50.0])));
        assert!(
            (dish.volume_mm3.unwrap() - std::f64::consts::PI * 50.0 * (30000.0 + 2500.0) / 6.0)
                .abs()
                < 1e-6
        );
        // 比不了体积的：中心线、折边碟、不认识的图元、开面。
        assert_eq!(
            record_volume(&geometry(Primitive::Line([0.0, 100.0]))).volume_mm3,
            None
        );
        assert_eq!(
            record_volume(&geometry(Primitive::EllipticalDish([100.0, 50.0]))).volume_mm3,
            None
        );
        let open = Polygon {
            contours: vec![Contour {
                vertices: vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [10.0, 10.0, 0.0],
                    [0.0, 10.0, 0.0],
                ],
                normals: Vec::new(),
            }],
        };
        let flat = record_volume(&geometry(Primitive::FacetGroup(vec![open])));
        assert!(flat.volume_mm3.is_none(), "平面算不出体积：{flat:?}");
        assert!(flat.why_not.unwrap().contains("平面"));

        // 两侧按主键对上：主键归一与 vendor 建表同口径。
        assert_eq!(
            display_key("TUBE 3 of BRANCH /C-OR-1R345-C"),
            "TUBI 3 /C-OR-1R345-C"
        );
        assert_eq!(display_key("/C-OR-1R345-C"), "/C-OR-1R345-C");
        assert_eq!(display_key("SUBEQUIPMENT 2 of EQUIPMENT /E1"), "SUBE 2 /E1");

        let mut baseline = RvmVolumes::default();
        let mut generated = RvmVolumes::default();
        let path = "/GENERATED/BRAN 1 of SITE /S";
        let key = display_key("BRAN 1 of SITE /S");
        baseline
            .by_key
            .insert(key.clone(), MemberVolume::measured(1_000_000.0));
        generated
            .by_path
            .insert(path.to_string(), MemberVolume::measured(1_012_000.0));
        generated.keys_by_path.insert(path.to_string(), key.clone());
        let verdict = compare_pair(&baseline, &generated, path, "BRAN");
        assert!(verdict.differs, "1.2% 超 0.05% 门限：{verdict:?}");
        assert!(verdict.note.contains("1.200%"), "{}", verdict.note);
        assert!(verdict.note.contains("门限 0.050%"));

        // 门限内：不判。
        generated
            .by_path
            .insert(path.to_string(), MemberVolume::measured(1_000_100.0));
        let verdict = compare_pair(&baseline, &generated, path, "BRAN");
        assert!(!verdict.differs);
        assert!(verdict.note.contains("在门限"), "{}", verdict.note);

        // 生成侧算不出体积：没比，不是通过。
        generated.by_path.insert(
            path.to_string(),
            MemberVolume::unmeasured("面片不闭合或退化成平面，算不出体积"),
        );
        let verdict = compare_pair(&baseline, &generated, path, "BRAN");
        assert!(!verdict.differs);
        assert!(verdict.note.starts_with("没有比体积："), "{}", verdict.note);
        assert!(verdict.note.contains("面片不闭合"));
        assert_eq!(verdict.delta, None);
    }

    /// GitHub #171（a）：同一圆柱面两侧分格疏密不同（E3D 8 段 vs 生成 16 段，半径 45 mm、高 20 mm，
    /// 对应 aps250162 GENPRI 2 / 12 号的 +8.23% 面积/体积比），包围盒一致——只记账、不翻判档，说明里带数值。
    #[test]
    fn facet_pairs_with_different_chord_tessellation_are_recorded_without_flipping_verdict() {
        let mut baseline = RvmVolumes::default();
        let mut generated = RvmVolumes::default();
        let path = "/GENERATED/GENPRI 2 of EQUIPMENT /MEI-JCAN01";
        let key = display_key("GENPRI 2 of EQUIPMENT /MEI-JCAN01");

        insert_facet_member(
            &mut baseline,
            path,
            &key,
            regular_prism_polygons(8, 45.0, 20.0),
        );
        insert_facet_member(
            &mut generated,
            path,
            &key,
            regular_prism_polygons(16, 45.0, 20.0),
        );

        let verdict = compare_pair(&baseline, &generated, path, "GENPRI");
        assert!(
            !verdict.differs,
            "8 段与 16 段圆柱面片只该记账，不该判 off：{verdict:?}"
        );
        let delta = verdict.delta.expect("两侧体积都可算");
        assert!(
            (delta - 0.0823).abs() < 0.001,
            "八边形与十六边形体积差约 8.23%，实得 {:.4}%",
            delta * 100.0
        );
        assert!(verdict.note.starts_with("只记账（不改判档）："), "{}", verdict.note);
        assert!(verdict.note.contains("分格疏密不同"), "{}", verdict.note);
        assert!(
            verdict.note.contains("16 顶点") && verdict.note.contains("32 顶点"),
            "应写出两侧顶点数：{}",
            verdict.note
        );

        // 组合壳体未做布尔并（两只正体积盒在三维上相交重叠 vs 合并后单壳）：散度体积含重复计入，只记账不翻。
        let mut base_single = RvmVolumes::default();
        let mut gen_overlap = RvmVolumes::default();
        let box_polys = |x0: f64, x1: f64, r: f64| -> Vec<Polygon> {
            let b = [
                [x0, -r, -r],
                [x1, -r, -r],
                [x1, r, -r],
                [x0, r, -r],
                [x0, -r, r],
                [x1, -r, r],
                [x1, r, r],
                [x0, r, r],
            ];
            let faces = [
                [0, 3, 2, 1],
                [4, 5, 6, 7],
                [0, 1, 5, 4],
                [2, 3, 7, 6],
                [1, 2, 6, 5],
                [3, 0, 4, 7],
            ];
            faces
                .into_iter()
                .map(|f| Polygon {
                    contours: vec![Contour {
                        vertices: f.map(|idx| b[idx]).to_vec(),
                        normals: Vec::new(),
                    }],
                })
                .collect()
        };
        insert_facet_member(&mut base_single, path, &key, box_polys(0.0, 100.0, 20.0));
        let mut two_overlapping = box_polys(0.0, 70.0, 20.0);
        two_overlapping.extend(box_polys(30.0, 100.0, 20.0));
        insert_facet_member(&mut gen_overlap, path, &key, two_overlapping);

        let overlap_verdict = compare_pair(&base_single, &gen_overlap, path, "FLAN");
        assert!(
            !overlap_verdict.differs,
            "多壳重叠未布尔并应只记账：{overlap_verdict:?}"
        );
        assert!(
            overlap_verdict.note.contains("正体积壳体三维重叠"),
            "{}",
            overlap_verdict.note
        );
    }

    /// GitHub #127 / #171（b）：形状真错（`/MDS/FT54A/1-BAR-1` 端部切口摆反 180°，包围盒完全相同、
    /// 两侧都是水密单壳面片、体积只差 +0.595% 且顶点数都是 16），必须照样判 `differs = true`。
    /// 即使现场没有 2026-11-10 的历史产物目录，也用内嵌的同构 L 型角钢切口反向面片兜底断言。
    #[test]
    fn reversed_end_cut_with_identical_aabb_and_small_volume_delta_still_flips_verdict() {
        // 先跑不依赖外部文件的确定性角钢端部切口反向样本（50×50×6 角钢长 300 mm，一端单肢斜切摆到另一侧，
        // 包围盒同为 [0..50, 0..50, 0..300]，两侧皆为水密单壳面片，体积差约 +1.5%，表面偏距 > 25 mm）。
        let notched_box = |cut_high_x: bool| -> Vec<Polygon> {
            // 50×50×300 直柱，顶面在一侧切去一块高 dz、宽 30 的楔形斜面（保留另一半顶面 z=300，故 AABB 不变）。
            let (dz, x_mid) = if cut_high_x { (28.0, 20.0) } else { (18.0, 20.0) };
            let pts = if cut_high_x {
                // 切在 x ∈ [20, 50] 一侧：z 从 300 降到 300 - dz
                [
                    [0.0, 0.0, 0.0],
                    [20.0, 0.0, 0.0],
                    [50.0, 0.0, 0.0],
                    [50.0, 50.0, 0.0],
                    [20.0, 50.0, 0.0],
                    [0.0, 50.0, 0.0],
                    [0.0, 0.0, 300.0],
                    [20.0, 0.0, 300.0],
                    [50.0, 0.0, 300.0 - dz],
                    [50.0, 50.0, 300.0 - dz],
                    [20.0, 50.0, 300.0],
                    [0.0, 50.0, 300.0],
                ]
            } else {
                // 切口摆反到 x ∈ [0, 30] 一侧：x ∈ [30, 50] 保持 z=300，故 AABB 仍是 [0..50, 0..50, 0..300]
                let xm = 50.0 - x_mid;
                [
                    [0.0, 0.0, 0.0],
                    [xm, 0.0, 0.0],
                    [50.0, 0.0, 0.0],
                    [50.0, 50.0, 0.0],
                    [xm, 50.0, 0.0],
                    [0.0, 50.0, 0.0],
                    [0.0, 0.0, 300.0 - dz],
                    [xm, 0.0, 300.0],
                    [50.0, 0.0, 300.0],
                    [50.0, 50.0, 300.0],
                    [xm, 50.0, 300.0],
                    [0.0, 50.0, 300.0 - dz],
                ]
            };
            let faces: &[&[usize]] = &[
                &[0, 5, 4, 3, 2, 1], // 底面 (-Z)
                &[6, 7, 10, 11],     // 顶左/斜面 1
                &[7, 8, 9, 10],      // 顶右/斜面 2
                &[0, 1, 7, 6],       // 前左 (-Y)
                &[1, 2, 8, 7],       // 前右 (-Y)
                &[2, 3, 9, 8],       // 右面 (+X)
                &[3, 4, 10, 9],      // 后右 (+Y)
                &[4, 5, 11, 10],     // 后左 (+Y)
                &[5, 0, 6, 11],      // 左面 (-X)
            ];
            faces
                .iter()
                .map(|f| Polygon {
                    contours: vec![Contour {
                        vertices: f.iter().map(|&i| pts[i]).collect(),
                        normals: Vec::new(),
                    }],
                })
                .collect()
        };

        let mut baseline = RvmVolumes::default();
        let mut generated = RvmVolumes::default();
        let path = "/GENERATED/MDS/FT54A/1-BAR-1";
        let key = display_key("/MDS/FT54A/1-BAR-1");
        insert_facet_member(&mut baseline, path, &key, notched_box(true));
        insert_facet_member(&mut generated, path, &key, notched_box(false));

        let verdict = compare_pair(&baseline, &generated, path, "SCTN");
        assert!(
            verdict.differs,
            "包围盒相同但端部切口反向的水密面片必须判 off：{verdict:?}"
        );
        assert!(verdict.note.contains("超过门限"), "{}", verdict.note);
        assert!(verdict.note.contains("双向表面最大偏距"), "{}", verdict.note);

        // 若本机存在 2026-11-10 主关节修复前的真实面片转储（#127 原始 16 根竖杆样本），逐根复核：
        // 修复前（pj0）16 根 BAR-1 必须 16/16 全判 differs=true；修复后（pj1）16 根 BAR-1 与 16 根 BAR-2 全不判。
        let dump_dir = Path::new(r"D:\work\plant-code\modelfix-20261110\out\pjoi");
        let e3d_json = dump_dir.join("e3d_ams7329_ft54.json");
        let pj0_json = dump_dir.join("pj0_ours_aps7329_ft54.json");
        let pj1_json = dump_dir.join("pj1_ours_aps7329_ft54.json");
        if e3d_json.is_file() && pj0_json.is_file() && pj1_json.is_file() {
            #[derive(Deserialize)]
            struct DumpGeom {
                polygons_world_mm: Vec<Vec<Vec<[f64; 3]>>>,
            }
            #[derive(Deserialize)]
            struct DumpMember {
                name: String,
                geometries: Vec<DumpGeom>,
            }
            let load_dump = |file: &Path| -> RvmVolumes {
                let members: Vec<DumpMember> =
                    serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
                let mut vols = RvmVolumes::default();
                for m in members {
                    let Some(first) = m.geometries.first() else {
                        continue;
                    };
                    let p = format!("/GENERATED/{}", m.name.trim_start_matches('/'));
                    let k = display_key(&m.name);
                    insert_facet_member(
                        &mut vols,
                        &p,
                        &k,
                        world_polys_to_local_polygons(&first.polygons_world_mm),
                    );
                }
                vols
            };
            let e3d_vols = load_dump(&e3d_json);
            let pj0_vols = load_dump(&pj0_json);
            let pj1_vols = load_dump(&pj1_json);

            for frame in ["A", "B", "C", "D"] {
                for size in 1..=4 {
                    let bar1_path = format!("/GENERATED/MDS/FT54{frame}/{size}-BAR-1");
                    let bad = compare_pair(&e3d_vols, &pj0_vols, &bar1_path, "SCTN");
                    assert!(
                        bad.differs,
                        "#127 原始样本 {bar1_path}（切口摆反 180°）必须被抓出：{bad:?}"
                    );
                    let fixed = compare_pair(&e3d_vols, &pj1_vols, &bar1_path, "SCTN");
                    assert!(
                        !fixed.differs,
                        "#34 修复后的 {bar1_path} 不该判 off：{fixed:?}"
                    );

                    let bar2_path = format!("/GENERATED/MDS/FT54{frame}/{size}-BAR-2");
                    let bar2 = compare_pair(&e3d_vols, &pj0_vols, &bar2_path, "SCTN");
                    assert!(!bar2.differs, "未摆反的对照杆 {bar2_path} 不该判 off：{bar2:?}");
                }
            }
        }
    }

    /// GitHub #171 真库 live 门：现场生成 `aps250162`（`=2013286706/1`）的 RVM，与 E3D 基准
    /// `aps250162_2013286706_1.rvm` 逐件跑 `compare_pair`——75 只 `GENPRI`（12 种分格不同、体积差
    /// 0.16%–34.66%）与 7 只多壳非流形 `NOZZ` 都只记账、不再有一件把判档翻成 `off`。
    #[test]
    #[ignore = "需要本机 APS 3.1 项目库与 aps-20261110 基准 RVM"]
    fn live_aps250162_genpri_tessellation_differences_do_not_flip_verdict() {
        let baseline_rvm =
            Path::new(r"D:\work\plant-code\baselines\_flat\aps-20261110\aps250162_2013286706_1.rvm");
        let db_file =
            Path::new(r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000\aps250162_0001");
        let project_dir = Path::new(r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000");
        assert!(baseline_rvm.is_file(), "缺基准 {}", baseline_rvm.display());
        assert!(db_file.is_file(), "缺设计库 {}", db_file.display());

        let template = crate::data_interface::direct_store::DirectSchema::template_dir_from_env()
            .expect("schema 模板目录");
        let out_dir =
            std::env::temp_dir().join(format!("rvm-check-171-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out_dir);
        std::fs::create_dir_all(&out_dir).unwrap();

        struct NoStop;
        impl e3d_model::GenerationHooks for NoStop {
            fn should_stop(&self, _visited: usize) -> Option<e3d_model::Stop> {
                None
            }
        }

        let opts = e3d_model::rvm_export::RvmExportOptions {
            rvm: true,
            attlib: template.join("attlib.dat"),
            db_file: db_file.to_path_buf(),
            template: template.to_path_buf(),
            db_type: "DESI".to_string(),
            sesno: None,
            root: Some(e3d_model::rvm_export::parse_refno("2013286706/1").unwrap()),
            out: out_dir.clone(),
            project_dirs: vec![project_dir.to_path_buf()],
            main_project_dir: Some(project_dir.to_path_buf()),
            ..e3d_model::rvm_export::RvmExportOptions::default()
        };
        let outcome = e3d_model::rvm_export::export_rvm(&opts, &NoStop).expect("生成 aps250162");
        let generated_rvm = outcome.rvm_path.expect("generated.rvm");
        let report =
            e3d_model::rvm_compare::compare_rvm_files(baseline_rvm, &generated_rvm).unwrap();
        let base_vols = read_volumes(baseline_rvm).unwrap();
        let gen_vols = read_volumes(&generated_rvm).unwrap();
        let _ = std::fs::remove_dir_all(&out_dir);

        let noun_by_path: HashMap<&str, &str> = outcome
            .member_sources
            .iter()
            .map(|s| (s.path.as_str(), s.noun.as_str()))
            .collect();

        let mut genpri_count = 0usize;
        let mut recorded_only = 0usize;
        let mut flipped = Vec::new();
        for pair in &report.pairs {
            assert_ne!(pair.verdict, "off", "包围盒基线应无 off：{}", pair.key);
            let noun = noun_by_path
                .get(pair.refno.as_str())
                .copied()
                .unwrap_or(&pair.noun);
            if noun == "GENPRI" {
                genpri_count += 1;
            }
            let verdict = compare_pair(&base_vols, &gen_vols, &pair.refno, noun);
            if verdict.differs {
                flipped.push((pair.key.clone(), verdict.note));
            } else if verdict.recorded_only {
                assert!(verdict.note.starts_with("只记账"), "{}", verdict.note);
                recorded_only += 1;
            }
        }
        assert_eq!(genpri_count, 75, "aps250162 应有 75 只配上的 GENPRI");
        assert!(
            flipped.is_empty(),
            "aps250162 不应再有因分格或内壁贴面被翻成 off 的构件：{flipped:?}"
        );
        assert_eq!(
            recorded_only, 67,
            "60 只分格不同的 GENPRI + 7 只内壁重合的 NOZZ 应逐件记账"
        );
    }
}
