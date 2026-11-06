//! 显示用粗精度网格（`meshes_dir()/lod1-v1/{L0 mesh_id}.mesh`）：L0 之外再准备一份只给
//! 浏览器远景用的粗网格（plant3d-web 的 DTX 层按屏幕投影直径选档）。
//!
//! 边界（ADR-096）：
//!
//! - **文件名借 L0 的内容键**，前端手里只有那一个键。内容不是那个键的摘要：不过
//!   `verify_content_addressed_mesh`，不进任何身份键，不进投影 / 房间判定 / RVM 对拍。
//! - **目录名带策略版本**（[`LOD1_DIR`]）：容差、比例、门槛任一变动就升 [`LOD1_POLICY_VERSION`]，
//!   新目录从空开始；旧目录不自动删，启动时点名。
//! - **标准图元（单位圆柱）的粗网格是 L0 键的纯函数**（[`lod1_segments_for`]）：同一个 L0 键
//!   永远对应同一份粗网格，启动 / 回填开始时一次写全（[`prepare_canonical_lod1`]），生成期不再
//!   按元素真实半径重算——0.5 mm 与 4 mm 两套段数的分带边界不对齐，按真实半径算会让同一个
//!   L0 键在两种段数之间来回改写。
//! - **目录件 / CSG 件**：盘上已有 `.mesh` 就复用；有 `.none` 标记（求过、省不够）就跳过；只对
//!   两者都没有的键把整根在弦高容差下限 [`LOD1_MIN_CHORD_TOL_MM`] 下再求值一遍
//!   （[`bake_lod1_root`]）。稳态下一根里没有待办就一次粗精度求值都不做。
//! - **失败只记日志**：L0 已经发布，这一份只是显示增强。
//!
//! 默认关（`AIOS_MESH_LOD1`，[`lod1_enabled`]）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use aios_core::shape::pdms_shape::PlantMesh;
use anyhow::Context;
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use e3d_model::catalogue::CatalogueMeshCache;
use e3d_model::discretise::{CHORD_TOL_MM, circle_segments_uncapped, with_min_chord_tol};
use e3d_model::elmodl::{GeneratedElement, GeometryId};
use e3d_model::pipeline::generate_subtree_with_cache;
use e3d_model::primitive_instance::PrimitiveMeshKey;
use e3d_model::solid::MAX_SEGMENTS;
use manifold_csg::Manifold;

use crate::fast_model::e3d_mesh_store::{MeshWrite, canonical_mesh_id, ensure_mesh_file};
use crate::fast_model::e3d_model_service::{BakedElement, element_mesh, local_generated_mesh};
use crate::fast_model::manifold_csg::manifold_to_plant_mesh;

/// 粗网格策略版本：容差、比例、门槛任一变动就 +1，目录随之换名（见 [`LOD1_DIR`]）。
pub const LOD1_POLICY_VERSION: u32 = 1;
/// 粗网格子目录名（`meshes_dir()/lod1-v1/`）。版本号必须与 [`LOD1_POLICY_VERSION`] 一致，
/// 单测钉着。
pub const LOD1_DIR: &str = "lod1-v1";
/// 显示用粗精度副本的弦高容差下限（mm）。L0 是 libgm 复刻口径 0.5 mm。
pub const LOD1_MIN_CHORD_TOL_MM: f64 = 4.0;
/// L0 少于这么多三角形的几何不出 L1：省下的顶点抵不上多一次请求。plant3d-web 的
/// `DTX_LOD1_MIN_SOURCE_TRIANGLES` 与之同值（它据此决定要不要来问）。
pub const LOD1_MIN_SOURCE_TRIANGLES: usize = 256;
/// L1 至少要比 L0 少 30% 的三角形才落盘，否则读 L1 没有意义（接口回 404，前端用 L0）。
pub const LOD1_MAX_KEEP_RATIO: f64 = 0.7;
/// 「求过了、省不够」的标记文件后缀（`lod1-v1/{id}.none`，零字节）。HTTP 只拼 `.mesh`，看不见它。
pub const LOD1_NONE_MARKER_EXT: &str = "none";

/// 生成时要不要多产一份粗网格（`AIOS_MESH_LOD1=1`）。默认关：开着时新键第一次出现要多求值一遍。
pub fn lod1_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("AIOS_MESH_LOD1")
            .map(|value| matches!(value.trim(), "1" | "true" | "on"))
            .unwrap_or(false)
    })
}

/// 粗网格目录：`{mesh_dir}/lod1-v1`。
pub fn lod1_dir(mesh_dir: &Path) -> PathBuf {
    mesh_dir.join(LOD1_DIR)
}

/// 某个 L0 键的粗网格文件路径。
pub fn lod1_mesh_path(mesh_dir: &Path, mesh_id: &str) -> PathBuf {
    lod1_dir(mesh_dir).join(format!("{mesh_id}.mesh"))
}

/// 某个 L0 键的「求过、省不够」标记路径。
pub fn lod1_marker_path(mesh_dir: &Path, mesh_id: &str) -> PathBuf {
    lod1_dir(mesh_dir).join(format!("{mesh_id}.{LOD1_NONE_MARKER_EXT}"))
}

/// 启动时点名同级的旧策略目录（`lod1/`、`lod1-v0/` 之类）：不自动删——那是运维的决定。
pub fn stale_lod1_dirs(mesh_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(mesh_dir) else {
        return Vec::new();
    };
    let mut stale = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("lod1") && name != LOD1_DIR)
        })
        .collect::<Vec<_>>();
    stale.sort();
    stale
}

/// L0 够大、L1 非空且至少省下 30% 三角形才值得落盘。
pub(crate) fn lod1_worth_keeping(l0_triangles: usize, l1_triangles: usize) -> bool {
    l0_triangles >= LOD1_MIN_SOURCE_TRIANGLES
        && l1_triangles > 0
        && l1_triangles as f64 <= l0_triangles as f64 * LOD1_MAX_KEEP_RATIO
}

/// 单位圆柱（`Manifold::cylinder`）按段数的三角形数：两端盖各 `n − 2`、侧面 `2n`。
pub(crate) fn unit_cylinder_triangles(segments: i32) -> usize {
    (4 * segments.max(3) as usize) - 4
}

/// L0 键为 `n` 段时，按 0.5 mm 口径严格落在这一分带**内部**的半径：取使
/// `2·acos(1 − tol/r) = 360/(n − 2)` 的那个 `r`（段数公式再向上补到 4 的倍数正好回到 `n`）。
fn band_radius(segments: i32) -> f64 {
    CHORD_TOL_MM / (1.0 - (std::f64::consts::PI / f64::from(segments - 2)).cos())
}

/// 标准单位圆柱：L0 键的段数 → 粗网格的段数。**L0 键的纯函数**，与元素真实半径无关。
///
/// 太小（L0 不足 [`LOD1_MIN_SOURCE_TRIANGLES`]）或省不够 30% 的给 `None`。段数走 e3d-model
/// 同一条 `circle_segments_uncapped`（45° 封顶、补到 4 的倍数），再封顶 [`MAX_SEGMENTS`]；
/// 用 `with_min_chord_tol(0.0)` 把线程局部的下限清掉，结果不随调用现场变。
pub(crate) fn lod1_segments_for(l0_segments: i32) -> Option<i32> {
    if l0_segments < 8 {
        return None;
    }
    let radius = band_radius(l0_segments);
    let l1 = with_min_chord_tol(0.0, || {
        circle_segments_uncapped(radius, LOD1_MIN_CHORD_TOL_MM)
    })
    .min(MAX_SEGMENTS);
    (l1 < l0_segments
        && lod1_worth_keeping(
            unit_cylinder_triangles(l0_segments),
            unit_cylinder_triangles(l1),
        ))
    .then_some(l1)
}

/// 标准图元键 → 粗网格用的键。盒不出粗网格（12 个三角形）。
pub(crate) fn canonical_lod1_key(key: PrimitiveMeshKey) -> Option<PrimitiveMeshKey> {
    match key {
        PrimitiveMeshKey::BoxV1 => None,
        PrimitiveMeshKey::CylinderV3 { segments } => {
            lod1_segments_for(segments).map(|segments| PrimitiveMeshKey::CylinderV3 { segments })
        }
    }
}

/// 与 e3d-model `primitive_instance::build_canonical` 同一只单位圆柱（半径 1、高 1、z 从 0 起），
/// 但**不进它的进程级缓存**：这里只为算 L0 键和写 L1，234 个键都缓存起来要几十 MB 常驻。
/// 单测钉着它与 `canonical_primitive_mesh` 的内容键逐位相同。
fn unit_cylinder_mesh(segments: i32) -> Option<PlantMesh> {
    let solid = Manifold::cylinder(1.0, 1.0, 1.0, segments, false);
    if solid.is_empty() || solid.num_tri() == 0 {
        return None;
    }
    Some(manifold_to_plant_mesh(&solid))
}

/// 会出粗网格的标准单位圆柱 L0 键：段数是 4 的倍数（`circle_segments_uncapped` 补的）、
/// 封顶 [`MAX_SEGMENTS`]、且 L0 三角形数过门槛——68、72、…、1000，共 234 个。
pub(crate) fn canonical_lod1_source_segments() -> impl Iterator<Item = i32> {
    (8..=MAX_SEGMENTS)
        .step_by(4)
        .filter(|segments| unit_cylinder_triangles(*segments) >= LOD1_MIN_SOURCE_TRIANGLES)
}

/// 一轮粗网格落盘的账。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Lod1Bake {
    /// 新写（含盘上内容不符而重写的）。
    pub written: usize,
    /// 盘上已有、逐位相同。
    pub reused: usize,
    /// 盘上有 `.none` 标记（此前求过、省不够），这次没求。
    pub known_skipped: usize,
    /// 这次求了、没省够（或 L0 本来就小、粗网格空）——不落盘，留 `.none` 标记。
    pub skipped: usize,
    /// L0 里要粗网格的件在粗精度快照里找不到（粗容差下 CSG 失败 / 被丢 / 变成了标准图元）。
    /// 不留标记，下次再试。
    pub unmatched: usize,
    /// 求值或写盘失败。
    pub failed: usize,
}

impl Lod1Bake {
    pub fn absorb(&mut self, other: Lod1Bake) {
        self.written += other.written;
        self.reused += other.reused;
        self.known_skipped += other.known_skipped;
        self.skipped += other.skipped;
        self.unmatched += other.unmatched;
        self.failed += other.failed;
    }

    /// 这一轮有没有值得在 info 级点名的事（稳态下全是 reused / known_skipped，只配 debug）。
    pub fn is_noteworthy(&self) -> bool {
        self.written + self.skipped + self.unmatched + self.failed > 0
    }

    fn note_write(&mut self, write: MeshWrite, mesh_id: &str) {
        match write {
            MeshWrite::Written => self.written += 1,
            MeshWrite::Reused => self.reused += 1,
            MeshWrite::Repaired => {
                // 同一个 L0 键只该有一种粗网格内容；走到这里说明策略变了却没升版本，或别的写者写了别的东西。
                log::warn!("[mesh-lod1] {mesh_id} 盘上的粗网格与本次求出的不符，已重写");
                self.written += 1;
            }
        }
    }
}

impl std::fmt::Display for Lod1Bake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "written={} reused={} known_skipped={} skipped={} unmatched={} failed={}",
            self.written,
            self.reused,
            self.known_skipped,
            self.skipped,
            self.unmatched,
            self.failed
        )
    }
}

/// 一件已发布（或至少已算出身份）的 L0 几何，粗网格规划的输入。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Lod1Source<'a> {
    pub geometry_id: &'a GeometryId,
    pub mesh_id: &'a str,
    pub mesh_triangles: usize,
    /// 标准图元的键；`None` = 目录件 / CSG 件（按内容寻址的 baked 网格）。
    pub primitive_key: Option<PrimitiveMeshKey>,
}

impl<'a> From<&'a BakedElement> for Lod1Source<'a> {
    fn from(element: &'a BakedElement) -> Self {
        Self {
            geometry_id: &element.geometry_id,
            mesh_id: &element.mesh_id,
            mesh_triangles: element.mesh_triangles,
            primitive_key: element
                .primitive_instance
                .as_ref()
                .map(|instance| instance.key),
        }
    }
}

/// 还没有粗网格、也没有标记的一件：要靠粗精度重求才能定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lod1Pending {
    pub geometry_id: GeometryId,
    pub mesh_id: String,
    pub mesh_triangles: usize,
}

/// 一根的粗网格规划：按盘分拣的结果。`pending` 为空就不必粗精度求值。
#[derive(Debug, Default)]
pub(crate) struct Lod1Plan {
    /// 不经求值就定下来的那些（复用 / 已知不值得 / 标准图元按键补齐）。
    pub settled: Lod1Bake,
    pub pending: Vec<Lod1Pending>,
}

/// 标准图元 L0 键在本进程内的裁决，避免每根都去 stat 盘。按（网格目录，键）记：
/// 生产只有一个目录，单测各用各的临时目录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanonicalVerdict {
    Kept,
    NotWorth,
}

type CanonicalVerdicts = HashMap<(PathBuf, String), CanonicalVerdict>;

fn canonical_verdicts() -> &'static Mutex<CanonicalVerdicts> {
    static VERDICTS: OnceLock<Mutex<CanonicalVerdicts>> = OnceLock::new();
    VERDICTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remember_canonical(mesh_dir: &Path, mesh_id: &str, verdict: CanonicalVerdict) {
    canonical_verdicts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((mesh_dir.to_path_buf(), mesh_id.to_string()), verdict);
}

fn recall_canonical(mesh_dir: &Path, mesh_id: &str) -> Option<CanonicalVerdict> {
    canonical_verdicts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(mesh_dir.to_path_buf(), mesh_id.to_string()))
        .copied()
}

/// 一个标准图元 L0 键的粗网格：写 / 复用 / 不值得 / 失败，进程内每个键只真正碰一次盘。
/// 失败不记裁决，下次再试。
fn ensure_canonical_lod1(
    mesh_dir: &Path,
    key: PrimitiveMeshKey,
    l0_mesh_id: &str,
    l0_triangles: usize,
    report: &mut Lod1Bake,
) {
    match recall_canonical(mesh_dir, l0_mesh_id) {
        Some(CanonicalVerdict::Kept) => {
            report.reused += 1;
            return;
        }
        Some(CanonicalVerdict::NotWorth) => {
            report.known_skipped += 1;
            return;
        }
        None => {}
    }
    let l1_mesh = canonical_lod1_key(key).and_then(|l1_key| match l1_key {
        PrimitiveMeshKey::CylinderV3 { segments } => unit_cylinder_mesh(segments),
        PrimitiveMeshKey::BoxV1 => None,
    });
    let Some(mesh) = l1_mesh else {
        remember_canonical(mesh_dir, l0_mesh_id, CanonicalVerdict::NotWorth);
        report.skipped += 1;
        return;
    };
    if mesh.aabb.is_none() || !lod1_worth_keeping(l0_triangles, mesh.indices.len() / 3) {
        remember_canonical(mesh_dir, l0_mesh_id, CanonicalVerdict::NotWorth);
        report.skipped += 1;
        return;
    }
    match ensure_mesh_file(&lod1_mesh_path(mesh_dir, l0_mesh_id), &mesh) {
        Ok(write) => {
            remember_canonical(mesh_dir, l0_mesh_id, CanonicalVerdict::Kept);
            report.note_write(write, l0_mesh_id);
        }
        Err(error) => {
            log::warn!("[mesh-lod1] 标准图元 {l0_mesh_id} 粗网格写盘失败: {error:#}");
            report.failed += 1;
        }
    }
}

/// 启动 / 回填开始时把全部标准单位圆柱的粗网格一次写全（[`canonical_lod1_source_segments`]，
/// 234 个键）。幂等：第二次全 `reused`。L0 键按与 `bake_element` 同一条
/// `canonical_mesh_id(key, mesh)` 算，所以文件名就是浏览器会来问的那个键。
pub fn prepare_canonical_lod1(mesh_dir: &Path) -> Lod1Bake {
    let started = std::time::Instant::now();
    let mut report = Lod1Bake::default();
    for segments in canonical_lod1_source_segments() {
        let key = PrimitiveMeshKey::CylinderV3 { segments };
        let Some(l0) = unit_cylinder_mesh(segments) else {
            log::warn!("[mesh-lod1] 单位圆柱 {segments} 段建不出来，跳过它的粗网格");
            report.failed += 1;
            continue;
        };
        let l0_mesh_id = canonical_mesh_id(key, &l0);
        ensure_canonical_lod1(
            mesh_dir,
            key,
            &l0_mesh_id,
            l0.indices.len() / 3,
            &mut report,
        );
    }
    log::info!(
        "[mesh-lod1] 标准图元粗网格就绪: {report} dir={} elapsed={:.1}ms",
        lod1_dir(mesh_dir).display(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    report
}

/// 按盘分拣一根的 L0 几何：标准图元按键当场补齐；目录件 / CSG 件按 `mesh_id` 去重后看盘——
/// 有 `.mesh` 计 reused、有 `.none` 计 known_skipped、都没有进 `pending`。
pub(crate) fn plan_lod1<'a>(
    sources: impl IntoIterator<Item = Lod1Source<'a>>,
    mesh_dir: &Path,
) -> Lod1Plan {
    let mut plan = Lod1Plan::default();
    let mut seen = HashSet::new();
    for source in sources {
        if source.mesh_triangles < LOD1_MIN_SOURCE_TRIANGLES {
            continue;
        }
        if !seen.insert(source.mesh_id.to_string()) {
            continue;
        }
        if let Some(key) = source.primitive_key {
            ensure_canonical_lod1(
                mesh_dir,
                key,
                source.mesh_id,
                source.mesh_triangles,
                &mut plan.settled,
            );
            continue;
        }
        if lod1_mesh_path(mesh_dir, source.mesh_id).exists() {
            plan.settled.reused += 1;
        } else if lod1_marker_path(mesh_dir, source.mesh_id).exists() {
            plan.settled.known_skipped += 1;
        } else {
            plan.pending.push(Lod1Pending {
                geometry_id: source.geometry_id.clone(),
                mesh_id: source.mesh_id.to_string(),
                mesh_triangles: source.mesh_triangles,
            });
        }
    }
    plan
}

/// 写「求过、省不够」标记。已存在照旧；写不上只记日志——标记只是省下次的功，不是产物。
fn write_none_marker(mesh_dir: &Path, mesh_id: &str) {
    let path = lod1_marker_path(mesh_dir, mesh_id);
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        log::warn!("[mesh-lod1] 建不出 {}: {error}", parent.display());
        return;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => log::warn!("[mesh-lod1] 写标记 {} 失败: {error}", path.display()),
    }
}

/// 把粗精度快照里对应 `pending` 的件写成粗网格：值得的落 `.mesh`，不值得的留 `.none`；
/// 快照里找不到、或在粗容差下变成了标准图元（单位网格与 L0 的实尺寸网格不在同一个坐标系，
/// 写出来就是错的）计 `unmatched`，不留标记。
pub(crate) fn write_lod1(
    plan: &Lod1Plan,
    coarse: &[GeneratedElement],
    mesh_dir: &Path,
) -> Lod1Bake {
    let mut report = Lod1Bake::default();
    let by_id: HashMap<&GeometryId, &GeneratedElement> = coarse
        .iter()
        .map(|element| (&element.geometry_id, element))
        .collect();
    for pending in &plan.pending {
        let Some(element) = by_id.get(&pending.geometry_id) else {
            log::warn!(
                "[mesh-lod1] {} ({}) 在粗精度快照里没有对应的件，这次不出粗网格",
                pending.mesh_id,
                pending.geometry_id
            );
            report.unmatched += 1;
            continue;
        };
        if element.primitive_instance.is_some() {
            log::warn!(
                "[mesh-lod1] {} ({}) 在粗容差下成了标准图元，与 L0 的实尺寸网格不同系，这次不出粗网格",
                pending.mesh_id,
                pending.geometry_id
            );
            report.unmatched += 1;
            continue;
        }
        let mesh = local_generated_mesh(element);
        if mesh.aabb.is_none()
            || !lod1_worth_keeping(pending.mesh_triangles, mesh.indices.len() / 3)
        {
            write_none_marker(mesh_dir, &pending.mesh_id);
            report.skipped += 1;
            continue;
        }
        match ensure_mesh_file(&lod1_mesh_path(mesh_dir, &pending.mesh_id), &mesh) {
            Ok(write) => report.note_write(write, &pending.mesh_id),
            Err(error) => {
                log::warn!("[mesh-lod1] {} 写盘失败: {error:#}", pending.mesh_id);
                report.failed += 1;
            }
        }
    }
    report
}

/// 一根的粗网格：先按盘分拣，只有 `pending` 非空才在弦高容差下限下把整根再求值一遍。
///
/// `cache` 必须是粗网格专用的那一份：目录网格缓存按求值结果复用，与 L0 共用会把两档精度混在一起。
/// 线程局部的容差下限只盖住这一次求值（e3d-model 的 DFS 内没有线程池），`Restore` 守卫在 panic
/// 展开时也恢复。
pub(crate) fn bake_lod1_root<'a>(
    set: &Arc<DbSet>,
    root: RefNo,
    sources: impl IntoIterator<Item = Lod1Source<'a>>,
    mesh_dir: &Path,
    cache: &mut CatalogueMeshCache,
) -> Lod1Bake {
    let plan = plan_lod1(sources, mesh_dir);
    let mut report = plan.settled;
    if plan.pending.is_empty() {
        if report.is_noteworthy() {
            log::info!("[mesh-lod1] root={root} {report}");
        } else {
            log::debug!("[mesh-lod1] root={root} {report}");
        }
        return report;
    }
    let coarse = match with_min_chord_tol(LOD1_MIN_CHORD_TOL_MM, || {
        generate_subtree_with_cache(set.element(root), cache)
    }) {
        Ok(outcome) => outcome.elements,
        Err(error) => {
            log::warn!(
                "[mesh-lod1] root {root} 粗精度求值失败，{} 件这次不出粗网格: {error:#}",
                plan.pending.len()
            );
            report.failed += plan.pending.len();
            return report;
        }
    };
    report.absorb(write_lod1(&plan, &coarse, mesh_dir));
    log::info!(
        "[mesh-lod1] root={root} pending={} {report}",
        plan.pending.len()
    );
    report
}

/// 回填里一根的账：粗网格账之外，还有「L0 不在盘上、视为未发布」和「算不出身份」两格。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Lod1RootBackfill {
    pub bake: Lod1Bake,
    /// L0 `.mesh` 不在网格目录里：这件没发布过（或已被清理），粗网格无从谈起。
    pub unpublished: usize,
    /// 网格身份算不出来（标准图元建不出 / 求值异常），这一件跳过。
    pub identity_failed: usize,
}

/// 一次回填（可跨几个库）的总账。
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Lod1BackfillReport {
    /// 跑完了的库（含有根失败的）。
    pub dbnums: Vec<u32>,
    /// 连根都枚举不出来（不在当前 MDB、会话解不出）的库，带原因。
    pub dbnums_failed: Vec<(u32, String)>,
    /// 枚举到的交付单元根总数。
    pub roots: usize,
    /// L0 求值失败、整根跳过的根数。
    pub roots_failed: usize,
    pub unpublished: usize,
    pub identity_failed: usize,
    pub bake: Lod1Bake,
}

impl Lod1BackfillReport {
    pub fn absorb_root(&mut self, root: Lod1RootBackfill) {
        self.unpublished += root.unpublished;
        self.identity_failed += root.identity_failed;
        self.bake.absorb(root.bake);
    }

    pub fn absorb(&mut self, other: Lod1BackfillReport) {
        self.dbnums.extend(other.dbnums);
        self.dbnums_failed.extend(other.dbnums_failed);
        self.roots += other.roots;
        self.roots_failed += other.roots_failed;
        self.unpublished += other.unpublished;
        self.identity_failed += other.identity_failed;
        self.bake.absorb(other.bake);
    }
}

/// 给一根已有库补粗网格：L0 **只求值、只算身份**——不写 L0、不算房间、不碰库——再交给
/// [`bake_lod1_root`] 按盘分拣。L0 文件不在盘上的件视为未发布、跳过，免得给没人会来问的键造孤儿。
///
/// `l0_cache` / `l1_cache` 两份目录网格缓存必须分开（精度不同）。
pub(crate) fn backfill_lod1_root(
    set: &Arc<DbSet>,
    root: RefNo,
    mesh_dir: &Path,
    l0_cache: &mut CatalogueMeshCache,
    l1_cache: &mut CatalogueMeshCache,
) -> anyhow::Result<Lod1RootBackfill> {
    let outcome = generate_subtree_with_cache(set.element(root), l0_cache)
        .with_context(|| format!("generate root {root}"))?;
    let mut report = Lod1RootBackfill::default();
    let mut identities = Vec::with_capacity(outcome.elements.len());
    for element in &outcome.elements {
        let (mesh, mesh_id) = match element_mesh(element) {
            Ok(identity) => identity,
            Err(error) => {
                log::warn!(
                    "[mesh-lod1] {} 算不出网格身份，跳过: {error:#}",
                    element.geometry_id
                );
                report.identity_failed += 1;
                continue;
            }
        };
        if !mesh_dir.join(format!("{mesh_id}.mesh")).exists() {
            report.unpublished += 1;
            continue;
        }
        identities.push((
            element.geometry_id.clone(),
            mesh_id,
            mesh.indices.len() / 3,
            element
                .primitive_instance
                .as_ref()
                .map(|instance| instance.key),
        ));
    }
    drop(outcome);
    let sources = identities
        .iter()
        .map(|(geometry_id, mesh_id, triangles, key)| Lod1Source {
            geometry_id,
            mesh_id,
            mesh_triangles: *triangles,
            primitive_key: *key,
        });
    report.bake = bake_lod1_root(set, root, sources, mesh_dir, l1_cache);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use e3d_model::primitive_instance::canonical_primitive_mesh;

    #[test]
    fn the_directory_name_carries_the_policy_version() {
        assert_eq!(LOD1_DIR, format!("lod1-v{LOD1_POLICY_VERSION}"));
    }

    #[test]
    fn lod1_is_kept_only_for_large_sources_that_shrink_enough() {
        assert!(!lod1_worth_keeping(255, 10), "L0 太小不出 L1");
        assert!(!lod1_worth_keeping(1000, 0), "空网格不落盘");
        assert!(!lod1_worth_keeping(1000, 701), "省不到 30% 不落盘");
        assert!(lod1_worth_keeping(1000, 700));
        assert!(lod1_worth_keeping(256, 64));
    }

    /// 单位圆柱的三角形数公式与 Manifold 建出来的逐位一致（规划时不建体就能判门槛）。
    #[test]
    fn unit_cylinder_triangle_count_matches_the_manifold() {
        for segments in [8, 68, 100, 1000] {
            let mesh = unit_cylinder_mesh(segments).expect("unit cylinder");
            assert_eq!(
                mesh.indices.len() / 3,
                unit_cylinder_triangles(segments),
                "n={segments}"
            );
        }
    }

    /// 分带代表半径按 0.5 mm 口径回到同一个 L0 段数：这是「粗网格是 L0 键的纯函数」的前提。
    #[test]
    fn the_band_radius_lands_back_on_its_own_segment_count() {
        for segments in canonical_lod1_source_segments().filter(|n| *n < MAX_SEGMENTS) {
            let back = with_min_chord_tol(0.0, || {
                circle_segments_uncapped(band_radius(segments), CHORD_TOL_MM)
            });
            assert_eq!(back, segments, "n={segments} r={}", band_radius(segments));
        }
    }

    /// 234 个会出粗网格的 L0 键：段数都是 4 的倍数、粗网格段数单调不减、每个都省够 30%、
    /// 小键给 None。
    #[test]
    fn lod1_segments_are_a_pure_function_of_the_l0_key() {
        let sources = canonical_lod1_source_segments().collect::<Vec<_>>();
        assert_eq!(sources.len(), 234);
        assert_eq!(sources.first(), Some(&68));
        assert_eq!(sources.last(), Some(&1000));
        assert_eq!(lod1_segments_for(72), Some(28));
        assert_eq!(lod1_segments_for(64), None, "L0 不足 256 三角形");
        assert_eq!(lod1_segments_for(8), None);
        let mut previous = 0;
        for n0 in sources {
            let n1 = lod1_segments_for(n0).unwrap_or_else(|| panic!("n0={n0} 必须出粗网格"));
            assert_eq!(n1 % 4, 0, "n0={n0} n1={n1}");
            assert!(n1 >= previous, "n0={n0}: {n1} < {previous}");
            assert!(
                lod1_worth_keeping(unit_cylinder_triangles(n0), unit_cylinder_triangles(n1)),
                "n0={n0} n1={n1}"
            );
            previous = n1;
        }
        assert_eq!(canonical_lod1_key(PrimitiveMeshKey::BoxV1), None);
        assert_eq!(
            canonical_lod1_key(PrimitiveMeshKey::CylinderV3 { segments: 72 }),
            Some(PrimitiveMeshKey::CylinderV3 { segments: 28 })
        );
    }

    /// 线程局部的容差下限不改变 L0 键 → 粗网格段数的映射。
    #[test]
    fn the_mapping_ignores_an_ambient_chord_tolerance_floor() {
        let plain = lod1_segments_for(200);
        let inside = with_min_chord_tol(20.0, || lod1_segments_for(200));
        assert_eq!(plain, inside);
    }

    /// 不进缓存的单位圆柱与 e3d-model 缓存里的那只算出同一个 L0 内容键——文件名就是浏览器要问的键。
    #[test]
    fn the_uncached_unit_cylinder_has_the_same_content_id_as_the_canonical_one() {
        for segments in [68, 72, 100, 1000] {
            let key = PrimitiveMeshKey::CylinderV3 { segments };
            let cached = manifold_to_plant_mesh(&canonical_primitive_mesh(key).expect("canonical"));
            let ours = unit_cylinder_mesh(segments).expect("unit cylinder");
            assert_eq!(
                canonical_mesh_id(key, &ours),
                canonical_mesh_id(key, &cached),
                "n={segments}"
            );
        }
    }

    /// 预生成：234 个文件、文件名是 L0 的内容键、第二次全 reused、一个都不重写。
    #[test]
    fn prepare_writes_every_canonical_lod1_once_and_reuses_afterwards() {
        let dir = tempfile::tempdir().unwrap();
        let first = prepare_canonical_lod1(dir.path());
        assert_eq!(first.failed, 0, "{first}");
        assert_eq!(first.unmatched, 0, "{first}");
        assert_eq!(first.written + first.reused + first.skipped, 234, "{first}");
        let files = std::fs::read_dir(lod1_dir(dir.path()))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "mesh"))
            .count();
        assert_eq!(files, first.written + first.reused, "{first}");
        let key = PrimitiveMeshKey::CylinderV3 { segments: 72 };
        let l0_id = canonical_mesh_id(key, &unit_cylinder_mesh(72).unwrap());
        assert!(
            lod1_mesh_path(dir.path(), &l0_id).exists(),
            "文件名是 L0 的键"
        );
        let coarse = PlantMesh::des_from_bytes(
            &std::fs::read(lod1_mesh_path(dir.path(), &l0_id)).expect("readable"),
        )
        .expect("rkyv archive");
        assert_eq!(coarse.indices.len() / 3, unit_cylinder_triangles(28));

        // 本进程对这个目录的裁决已记住：第二轮一个文件都不碰。
        let again = prepare_canonical_lod1(dir.path());
        assert_eq!(again.written, 0, "{again}");
        assert_eq!(again.reused + again.known_skipped, 234, "{again}");
    }

    fn source<'a>(
        geometry_id: &'a GeometryId,
        mesh_id: &'a str,
        triangles: usize,
        key: Option<PrimitiveMeshKey>,
    ) -> Lod1Source<'a> {
        Lod1Source {
            geometry_id,
            mesh_id,
            mesh_triangles: triangles,
            primitive_key: key,
        }
    }

    fn element_id(refno: &str) -> GeometryId {
        GeometryId::Element {
            refno: refno.to_string(),
        }
    }

    /// 分拣：小件不进、同键去重、有 `.mesh` 计 reused、有 `.none` 计 known_skipped、都没有进 pending；
    /// 标准图元按键当场补齐、不进 pending。
    #[test]
    fn plan_sorts_sources_by_what_is_already_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lod1_dir(dir.path())).unwrap();
        std::fs::write(lod1_mesh_path(dir.path(), "e3d_baked_v2_have"), b"mesh").unwrap();
        write_none_marker(dir.path(), "e3d_baked_v2_known");
        let (a, b, c, d, e, f) = (
            element_id("1_1"),
            element_id("1_2"),
            element_id("1_3"),
            element_id("1_4"),
            element_id("1_5"),
            element_id("1_6"),
        );
        let key = PrimitiveMeshKey::CylinderV3 { segments: 72 };
        let l0_id = canonical_mesh_id(key, &unit_cylinder_mesh(72).unwrap());
        let plan = plan_lod1(
            [
                source(&a, "e3d_baked_v2_small", 255, None),
                source(&b, "e3d_baked_v2_have", 300, None),
                source(&c, "e3d_baked_v2_known", 300, None),
                source(&d, "e3d_baked_v2_new", 300, None),
                source(&e, "e3d_baked_v2_new", 300, None),
                source(&f, &l0_id, unit_cylinder_triangles(72), Some(key)),
            ],
            dir.path(),
        );
        assert_eq!(plan.settled.reused, 1, "{:?}", plan.settled);
        assert_eq!(plan.settled.known_skipped, 1, "{:?}", plan.settled);
        assert_eq!(
            plan.settled.written, 1,
            "标准图元按键当场写: {:?}",
            plan.settled
        );
        assert_eq!(
            plan.pending,
            vec![Lod1Pending {
                geometry_id: d,
                mesh_id: "e3d_baked_v2_new".to_string(),
                mesh_triangles: 300,
            }]
        );
        assert!(lod1_mesh_path(dir.path(), &l0_id).exists());
    }

    fn coarse_element(
        refno: &str,
        solid: Manifold,
        primitive_instance: Option<e3d_model::primitive_instance::PrimitiveInstance>,
    ) -> GeneratedElement {
        let word1 = refno.rsplit('_').next().unwrap().parse().unwrap();
        GeneratedElement {
            geometry_id: element_id(refno),
            refno: RefNo::new(1, word1),
            noun: "VALV".into(),
            is_invalid_tubi: false,
            name: None,
            solid,
            parts: None,
            part_catalogue_nouns: Vec::new(),
            degenerate_parts: Vec::new(),
            part_record_repeats: Vec::new(),
            world: glam::DMat4::IDENTITY,
            primitive_instance,
            analytic_primitive: None,
            negatives_applied: 0,
            own_ngms_cuts: 0,
            negatives_skipped: vec![],
            joint_cuts_explained: vec![],
            notes: vec![],
        }
    }

    /// 写盘：省够的落 `.mesh`，省不够的留 `.none`，粗快照里没有的和变成标准图元的计 unmatched、不留标记。
    #[test]
    fn write_lod1_keeps_marks_and_reports_unmatched() {
        let dir = tempfile::tempdir().unwrap();
        let (keep, flat, gone, canon) = (
            element_id("1_1"),
            element_id("1_2"),
            element_id("1_3"),
            element_id("1_4"),
        );
        let l0_triangles = unit_cylinder_triangles(100);
        let pending = |geometry_id: &GeometryId, mesh_id: &str| Lod1Pending {
            geometry_id: geometry_id.clone(),
            mesh_id: mesh_id.to_string(),
            mesh_triangles: l0_triangles,
        };
        let plan = Lod1Plan {
            settled: Lod1Bake::default(),
            pending: vec![
                pending(&keep, "e3d_baked_v2_keep"),
                pending(&flat, "e3d_baked_v2_flat"),
                pending(&gone, "e3d_baked_v2_gone"),
                pending(&canon, "e3d_baked_v2_canon"),
            ],
        };
        let coarse = vec![
            coarse_element("1_1", Manifold::cylinder(10.0, 5.0, 5.0, 32, false), None),
            coarse_element("1_2", Manifold::cylinder(10.0, 5.0, 5.0, 100, false), None),
            coarse_element(
                "1_4",
                Manifold::cylinder(10.0, 5.0, 5.0, 32, false),
                Some(e3d_model::primitive_instance::PrimitiveInstance {
                    key: PrimitiveMeshKey::CylinderV3 { segments: 32 },
                    local_transform: glam::DMat4::IDENTITY,
                }),
            ),
        ];

        let report = write_lod1(&plan, &coarse, dir.path());
        assert_eq!(
            report,
            Lod1Bake {
                written: 1,
                skipped: 1,
                unmatched: 2,
                ..Default::default()
            },
            "{report}"
        );
        let kept = PlantMesh::des_from_bytes(
            &std::fs::read(lod1_mesh_path(dir.path(), "e3d_baked_v2_keep")).unwrap(),
        )
        .unwrap();
        assert_eq!(kept.indices.len() / 3, unit_cylinder_triangles(32));
        assert!(
            lod1_marker_path(dir.path(), "e3d_baked_v2_flat").exists(),
            "省不够留标记"
        );
        assert!(!lod1_mesh_path(dir.path(), "e3d_baked_v2_flat").exists());
        for id in ["e3d_baked_v2_gone", "e3d_baked_v2_canon"] {
            assert!(!lod1_mesh_path(dir.path(), id).exists(), "{id}");
            assert!(
                !lod1_marker_path(dir.path(), id).exists(),
                "{id} 不留标记，下次再试"
            );
        }

        // 再规划一次：keep 已在盘上、flat 有标记、另外两个还是待办。
        let sources = [
            source(&keep, "e3d_baked_v2_keep", l0_triangles, None),
            source(&flat, "e3d_baked_v2_flat", l0_triangles, None),
            source(&gone, "e3d_baked_v2_gone", l0_triangles, None),
            source(&canon, "e3d_baked_v2_canon", l0_triangles, None),
        ];
        let again = plan_lod1(sources, dir.path());
        assert_eq!(again.settled.reused, 1, "{:?}", again.settled);
        assert_eq!(again.settled.known_skipped, 1, "{:?}", again.settled);
        assert_eq!(again.pending.len(), 2, "{:?}", again.pending);
    }

    /// 标记文件：第一次建、第二次照旧、HTTP 拼的 `.mesh` 路径看不见它。
    #[test]
    fn the_none_marker_is_idempotent_and_invisible_to_the_mesh_path() {
        let dir = tempfile::tempdir().unwrap();
        write_none_marker(dir.path(), "e3d_baked_v2_x");
        write_none_marker(dir.path(), "e3d_baked_v2_x");
        assert!(lod1_marker_path(dir.path(), "e3d_baked_v2_x").exists());
        assert_eq!(
            std::fs::metadata(lod1_marker_path(dir.path(), "e3d_baked_v2_x"))
                .unwrap()
                .len(),
            0
        );
        assert!(!lod1_mesh_path(dir.path(), "e3d_baked_v2_x").exists());
    }

    /// 旧策略目录只点名不删。
    #[test]
    fn stale_policy_directories_are_listed_but_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("lod1")).unwrap();
        std::fs::create_dir_all(dir.path().join(LOD1_DIR)).unwrap();
        std::fs::create_dir_all(dir.path().join("model-input-cache")).unwrap();
        std::fs::write(dir.path().join("lod1-v9.mesh"), b"x").unwrap();
        let stale = stale_lod1_dirs(dir.path());
        assert_eq!(stale, vec![dir.path().join("lod1")]);
        assert!(dir.path().join("lod1").exists());
    }
}
