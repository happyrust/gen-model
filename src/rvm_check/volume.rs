//! 配上的构件再比一次体积（GitHub #127）。
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
//! 门限 [`VOLUME_DELTA_LIMIT`] 取 0.05%：上例里正确构件最大 0.016%、有问题的杆最小 0.60%，中间留了一个
//! 数量级的余量。APS 39 根、AMS 56 根的全量分布还要再跑一遍才定得下来（见 GitHub #127 的验收项）。

use std::collections::HashMap;
use std::path::Path;

use e3d_model::rvm_read::{Primitive, RvmBytes, RvmScan};
use e3d_model::rvm_compare::chord_tolerance_mm;
use serde::{Deserialize, Serialize};

/// 体积差门限（相对值）：两侧体积差超过它判不一致。见模块文档的取值依据。
pub const VOLUME_DELTA_LIMIT: f64 = 0.0005;

/// 判档里的原因种类：体积差超门限（进 `rvm_check_mismatch` 的 `reason_kind`）。
pub const REASON_VOLUME_DIFFERS: &str = "volume_differs";

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

/// 一份 RVM 里各成员的体积：按成员路径、按显示名归一后的主键各一份索引，外加路径 → 主键的对应。
#[derive(Clone, Debug, Default)]
pub struct RvmVolumes {
    by_path: HashMap<String, MemberVolume>,
    by_key: HashMap<String, MemberVolume>,
    keys_by_path: HashMap<String, String>,
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
                format!(
                    "{} {} {}",
                    noun,
                    ordinal.trim(),
                    owner_key(owner)
                )
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
fn facet_volume(polygons: &[e3d_model::rvm_read::Polygon]) -> f64 {
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
fn record_volume(geometry: &e3d_model::rvm_read::RvmGeometry) -> MemberVolume {
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
        Primitive::SphericalDish([r, h]) => MemberVolume::measured(
            std::f64::consts::PI * h.abs() * (3.0 * r * r + h * h) / 6.0,
        ),
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
        Primitive::EllipticalDish(_) => MemberVolume::unmeasured("折边碟（EllipticalDish）的体积还没算"),
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

/// 一个成员的体积：各条记录相加；有一条不可比就整件不可比（只加一部分会算出个假体积）。
fn member_volume(
    scan: &RvmScan,
    bytes: &[u8],
    member: &e3d_model::rvm_read::ScannedMember,
) -> MemberVolume {
    let _ = scan;
    if member.records.is_empty() {
        return MemberVolume::unmeasured("没有几何记录");
    }
    let mut total = 0.0;
    for record in &member.records {
        let geometry = match record.decode_without_normals(bytes) {
            Ok(geometry) => geometry,
            Err(error) => return MemberVolume::unmeasured(format!("记录解不出来：{error:#}")),
        };
        match record_volume(&geometry) {
            MemberVolume {
                volume_mm3: Some(volume),
                ..
            } => total += volume,
            MemberVolume { why_not, .. } => {
                return MemberVolume::unmeasured(
                    why_not.unwrap_or_else(|| "有记录算不出体积".to_string()),
                );
            }
        }
    }
    MemberVolume::measured(total)
}

/// 读一份 RVM，算出每个成员的体积，并按成员路径与主键各建一份索引。
pub fn read_volumes(path: &Path) -> Result<RvmVolumes, String> {
    let bytes = RvmBytes::open(path).map_err(|error| format!("读 {} 失败：{error:#}", path.display()))?;
    let scan = RvmScan::scan(&bytes)
        .map_err(|error| format!("解析 {} 失败：{error:#}", path.display()))?;
    let mut volumes = RvmVolumes::default();
    for member in &scan.members {
        let volume = member_volume(&scan, &bytes, member);
        let path = member.path.trim().to_string();
        let key = display_key(&member.name);
        volumes.by_path.insert(path.clone(), volume.clone());
        volumes.keys_by_path.insert(path, key.clone());
        volumes.by_key.entry(key).or_insert(volume);
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
    /// 不可比、或没超门限时的一句话。
    pub note: String,
    /// 超门限：这一对判不一致。
    pub differs: bool,
}

/// 比一对：`generated_path` 是报告的 `pairs[].refno`（生成成员路径），基准侧按同一主键去查。
/// 两侧任一算不出体积就返回「没比」——**不能当成通过**。
pub fn compare_pair(
    baseline: &RvmVolumes,
    generated: &RvmVolumes,
    generated_path: &str,
    noun: &str,
) -> VolumeVerdict {
    let ours = generated.by_path(generated_path);
    let theirs = generated
        .key_of_path(generated_path)
        .and_then(|key| baseline.by_key(key));
    let (baseline_mm3, generated_mm3) = match (theirs, ours) {
        (Some(theirs), Some(ours)) => (theirs.volume_mm3, ours.volume_mm3),
        _ => (None, None),
    };
    match (baseline_mm3, generated_mm3) {
        (Some(baseline_mm3), Some(generated_mm3)) if baseline_mm3 > 1.0 => {
            let delta = (generated_mm3 - baseline_mm3).abs() / baseline_mm3;
            let percent = delta * 100.0;
            if delta > VOLUME_DELTA_LIMIT {
                VolumeVerdict {
                    baseline_mm3: Some(baseline_mm3),
                    generated_mm3: Some(generated_mm3),
                    delta: Some(delta),
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
                    note: format!("体积差 {percent:.3}%，在门限 {:.3}% 内", VOLUME_DELTA_LIMIT * 100.0),
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
                        .unwrap_or_else(|| "基准侧没算出来（主键对不上或没有这个成员）".to_string()),
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

impl RvmVolumes {
    fn by_key_member_path(&self, _path: &str) -> Option<()> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(primitive: Primitive) -> e3d_model::rvm_read::RvmGeometry {
        e3d_model::rvm_read::RvmGeometry {
            tag: "PRIM".to_string(),
            flags: 0,
            primitive,
            matrix3: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            translation_mm: [0.0; 3],
            record_bbox: [0.0; 6],
            bbox_frame: e3d_model::rvm_read::BboxFrame::Local,
            level: None,
        }
    }

    /// GitHub #127：配上的构件再比一次体积。包围盒相同的两件，体积差超门限就判不一致并写清数值；
    /// 门限内不判；两侧任一侧算不出体积时**不算通过**，写明没比。
    #[test]
    fn a_paired_element_is_compared_by_volume_after_its_box_passes() {
        // 解析图元的闭式体积。
        let boxed = record_volume(&geometry(Primitive::Box([10.0, 20.0, 30.0])));
        assert_eq!(boxed.volume_mm3, Some(6000.0));
        let cylinder = record_volume(&geometry(Primitive::Cylinder([100.0, 50.0])));
        assert!((cylinder.volume_mm3.unwrap() - std::f64::consts::PI * 100.0 * 100.0 * 50.0).abs() < 1e-6);
        let sphere = record_volume(&geometry(Primitive::Sphere([10.0])));
        assert!((sphere.volume_mm3.unwrap() - 4.0 / 3.0 * std::f64::consts::PI * 1000.0).abs() < 1e-6);
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
            (torus.volume_mm3.unwrap() - std::f64::consts::TAU * 500.0 * std::f64::consts::PI * 400.0).abs()
                < 1e-6
        );
        let rect_torus = record_volume(&geometry(Primitive::RectangularTorus([
            100.0, 200.0, 50.0, 360.0,
        ])));
        assert!(
            (rect_torus.volume_mm3.unwrap()
                - std::f64::consts::TAU * 150.0 * 100.0 * 50.0)
                .abs()
                < 1e-6
        );
        let dish = record_volume(&geometry(Primitive::SphericalDish([100.0, 50.0])));
        assert!(
            (dish.volume_mm3.unwrap() - std::f64::consts::PI * 50.0 * (30000.0 + 2500.0) / 6.0).abs()
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
        let open = e3d_model::rvm_read::Polygon {
            contours: vec![e3d_model::rvm_read::Contour {
                vertices: vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [10.0, 10.0, 0.0],
                    [0.0, 10.0, 0.0],
                ],
                normals: Vec::new(),
            }],
        };
        let flat = record_volume(&geometry(Primitive::FacetGroup(vec![open.clone()])));
        assert!(flat.volume_mm3.is_none(), "平面算不出体积：{flat:?}");
        assert!(flat.why_not.unwrap().contains("平面"));

        // 两侧按主键对上：主键归一与 vendor 建表同口径。
        assert_eq!(display_key("TUBE 3 of BRANCH /C-OR-1R345-C"), "TUBI 3 /C-OR-1R345-C");
        assert_eq!(display_key("/C-OR-1R345-C"), "/C-OR-1R345-C");
        assert_eq!(
            display_key("SUBEQUIPMENT 2 of EQUIPMENT /E1"),
            "SUBE 2 /E1"
        );

        let mut baseline = RvmVolumes::default();
        let mut generated = RvmVolumes::default();
        let path = "/GENERATED/BRAN 1 of SITE /S";
        let key = display_key("BRAN 1 of SITE /S");
        baseline
            .by_key
            .insert(key.clone(), MemberVolume::measured(1_000_000.0));
        generated.by_path.insert(
            path.to_string(),
            MemberVolume::measured(1_012_000.0),
        );
        generated
            .keys_by_path
            .insert(path.to_string(), key.clone());
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
        generated
            .by_path
            .insert(path.to_string(), MemberVolume::unmeasured("面片不闭合或退化成平面，算不出体积"));
        let verdict = compare_pair(&baseline, &generated, path, "BRAN");
        assert!(!verdict.differs);
        assert!(verdict.note.starts_with("没有比体积："), "{}", verdict.note);
        assert!(verdict.note.contains("面片不闭合"));
        assert_eq!(verdict.delta, None);
    }
}
