//! RVM/ATT → 基准快照。
//!
//! 只做解析与结构化，不连数据库、不做对拍。身份解析（组名 → 真实 refno）
//! 需要站点库参与，单独一步接入；本步先把 noun / owner / 序号解析出来并落盘。

use anyhow::{Context, Result, anyhow};
use rvm_rs::store::Store;
use rvm_rs::store::geometry::{Geometry, GeometryKind, GeometryType};
use rvm_rs::store::node::{NodeId, NodeKind};
use rvm_rs::{parse_att, parse_rvm};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::fast_model::libgm_discretise::RES_TOL_MM;

use super::att::{AttIndex, refno_from_att_name};
use super::identity::{noun_from_name, parse_default_name, stable_id};
use super::snapshot::{
    ExportScope, RvmGeometry, RvmMember, RvmSnapshot, SNAPSHOT_VERSION, SnapshotMeta,
};

/// rvm-rs 把**几何**坐标（bbox_world、geometry.transform.translation）换算成米，
/// E3D world 与生成侧都是毫米，所以这两处要乘回去。
///
/// 唯一的例外是 `group.translation`（CNTB 记录）：RVM 原生就是毫米，rvm-rs 原样
/// 透传，**不要**再乘——已用真实快照核对过（translation 与 aabb_world_mm 同量级，
/// 见 test_data/rvm/C-IY-1R330-B.rvm.json）。
const M_TO_MM: f64 = 1000.0;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub dbnum: u32,
    pub rvm_path: PathBuf,
    pub att_paths: Vec<PathBuf>,
    pub out_path: PathBuf,
    /// 根元素的真实 refno（如 `24384/22404`）。命名元素的 refno 不在 ATT 里，
    /// 只能外部给定或后续查站点库；给了就直接钉死，省一次反查。
    pub root_refno: Option<String>,
    /// 导出这份 RVM 的口径。RVM 流里读不出来，只能由导出方声明。
    pub export_scope: ExportScope,
    pub verbose: bool,
}

pub fn import_rvm(options: &ImportOptions) -> Result<RvmSnapshot> {
    let root_refno = options
        .root_refno
        .as_deref()
        .map(|value| {
            refno_from_att_name(&format!("={}", value.trim()))
                .ok_or_else(|| anyhow!("非法 --root-refno: {value}"))
        })
        .transpose()?;
    let mut store = Store::new();

    let rvm_bytes = fs::read(&options.rvm_path)
        .with_context(|| format!("读取 RVM 文件失败: {}", options.rvm_path.display()))?;
    parse_rvm(&rvm_bytes, &mut store)
        .with_context(|| format!("解析 RVM 文件失败: {}", options.rvm_path.display()))?;

    for att_path in &options.att_paths {
        let att_text = fs::read_to_string(att_path)
            .with_context(|| format!("读取 ATT 文件失败: {}", att_path.display()))?;
        parse_att(&att_text, &mut store)
            .with_context(|| format!("解析 ATT 文件失败: {}", att_path.display()))?;
    }
    // rvm-rs 的 parse_att 不会把属性挂回 group.attributes（实测 40 个成员全空），
    // 所以自己再解一遍，身份解析要用里面的 NAME/TYPE。
    let att = AttIndex::load(&options.att_paths)?;

    let mut builder = Builder::new(options.dbnum, options.verbose, att);
    for &root in store.roots() {
        builder.walk(&store, root, &mut VecDeque::new(), None)?;
    }

    // 导出根优先取 ATT 头里的 Element；没有 ATT 时退回「第一个默认命名成员的
    // owner」——不能取层级最浅的组，RVM 会把 SITE/ZONE/PIPE 祖先一并带出来。
    let root_name = builder
        .att
        .root_element()
        .map(|s| s.to_string())
        .or_else(|| {
            builder
                .members
                .iter()
                .find(|m| m.ordinal.is_some())
                .and_then(|m| m.parent_stable_id)
                .and_then(|parent| {
                    builder
                        .members
                        .iter()
                        .find(|m| m.stable_id == parent)
                        .map(|m| m.name.clone())
                })
        })
        .or_else(|| builder.members.last().map(|m| m.name.clone()));

    // 根元素是命名元素，refno 不在 ATT 里，外部给了就钉上。
    if let (Some(refno), Some(root)) = (root_refno.as_ref(), root_name.as_ref()) {
        if let Some(member) = builder.members.iter_mut().find(|m| &m.name == root) {
            member.refno = Some(refno.clone());
            member.identity_source = "cli_root".to_string();
            member.resolved = true;
        }
    }

    let meta = SnapshotMeta {
        version: SNAPSHOT_VERSION,
        dbnum: options.dbnum,
        root_name,
        rvm_file: options.rvm_path.display().to_string(),
        att_files: options
            .att_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        imported_at: now_string(),
        member_count: builder.members.len(),
        geometry_count: builder.geometry_count,
        resolved: builder.members.iter().filter(|m| m.resolved).count(),
        unresolved: builder.members.iter().filter(|m| !m.resolved).count(),
        export_scope: options.export_scope,
        geo_type_counts: builder.geo_type_counts,
        degenerate_bbox_count: builder.degenerate_bbox_count,
        facet_aabb_from_vertices: builder.facet_aabb_from_vertices,
        subresolution_shell_count: builder.subresolution_shell_count,
    };

    Ok(RvmSnapshot {
        meta,
        members: builder.members,
    })
}

struct Builder {
    dbnum: u32,
    verbose: bool,
    att: AttIndex,
    members: Vec<RvmMember>,
    geometry_count: usize,
    geo_type_counts: BTreeMap<String, usize>,
    degenerate_bbox_count: usize,
    /// 世界 AABB 由面片顶点实算的几何数（见 [`facet_group_shell_components`]）。
    facet_aabb_from_vertices: usize,
    /// 亚容差壳的连通分量数（见 [`ShellComponent::is_subresolution`]）。
    subresolution_shell_count: usize,
}

impl Builder {
    fn new(dbnum: u32, verbose: bool, att: AttIndex) -> Self {
        Self {
            dbnum,
            verbose,
            att,
            members: Vec::new(),
            geometry_count: 0,
            geo_type_counts: BTreeMap::new(),
            degenerate_bbox_count: 0,
            facet_aabb_from_vertices: 0,
            subresolution_shell_count: 0,
        }
    }

    fn walk(
        &mut self,
        store: &Store,
        node_id: NodeId,
        path: &mut VecDeque<String>,
        parent_stable: Option<u64>,
    ) -> Result<()> {
        let node = store
            .get_node(node_id)
            .ok_or_else(|| anyhow!("无效的节点 ID: {}", node_id.0))?;

        match &node.kind {
            // File 节点带的是 RVM 导出横幅（"AVEVA E3D Design Mk3.1.9…"），
            // 不是层级的一部分，放进路径只会污染 stable_id 和报告可读性。
            NodeKind::File(_) => {
                self.walk_children(store, node, path, parent_stable)?;
            }
            NodeKind::Model(model) => {
                let name = sanitize(store.get_string(model.name), || {
                    format!("model_{}", node_id.0)
                });
                path.push_back(name);
                self.walk_children(store, node, path, parent_stable)?;
                path.pop_back();
            }
            NodeKind::Group(group) => {
                let name = sanitize(store.get_string(group.name), || {
                    format!("group_{}", node_id.0)
                });
                path.push_back(name.clone());
                let full_path = join_path(path);
                let id = stable_id(self.dbnum, &full_path);

                let default_name = parse_default_name(&name);
                // ATT 的 TYPE 是权威 noun；没有 ATT 时才退回从默认命名里猜。
                let att_section = self.att.get(&name).cloned();
                let noun = att_section
                    .as_ref()
                    .and_then(|s| s.get("TYPE"))
                    .map(|t| t.trim().to_ascii_uppercase())
                    .filter(|t| !t.is_empty())
                    .or_else(|| noun_from_name(&name));

                // 未命名元素的 ATT NAME 就是 `=ref0/ref1`，直接就是真实身份。
                let refno = att_section
                    .as_ref()
                    .and_then(|s| s.get("NAME"))
                    .and_then(|v| refno_from_att_name(v));
                let (identity_source, resolved) = match refno {
                    Some(_) => ("att_direct", true),
                    None if att_section.is_some() => ("att_name", false),
                    None => ("stable_hash", false),
                };

                let mut geometries = Vec::new();
                let mut union: Option<[f64; 6]> = None;
                let mut link = group.first_geometry;
                let mut index = 0usize;
                while let Some(geometry_id) = link {
                    let geometry = store
                        .get_geometry(geometry_id)
                        .ok_or_else(|| anyhow!("无效的几何 ID: {}", geometry_id.0))?;
                    index += 1;
                    let entry = self.build_geometry(index, geometry);
                    union = merge_geometry_aabb(union, entry.bbox_world_mm, entry.bbox_degenerate);
                    geometries.push(entry);
                    link = geometry.next;
                }

                if self.verbose {
                    println!(
                        "[rvm-import] {} noun={} geos={} attrs={}",
                        full_path,
                        noun.as_deref().unwrap_or("-"),
                        geometries.len(),
                        group.attributes.len()
                    );
                }

                let translation = group.translation;
                self.members.push(RvmMember {
                    path: full_path,
                    name,
                    noun,
                    owner_desc: default_name
                        .as_ref()
                        .map(|d| d.owner_desc.clone())
                        .or_else(|| {
                            att_section
                                .as_ref()
                                .and_then(|s| s.get("OWNER"))
                                .map(|v| v.trim().to_string())
                        }),
                    ordinal: default_name.as_ref().map(|d| d.ordinal),
                    refno,
                    identity_source: identity_source.to_string(),
                    resolved,
                    stable_id: id,
                    parent_stable_id: parent_stable,
                    translation_mm: [translation.x, translation.y, translation.z],
                    aabb_world_mm: union,
                    attrs: att_section
                        .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null))
                        .filter(|v| !v.is_null())
                        .or_else(|| group_attrs(store, group)),
                    geometries,
                });

                self.walk_children(store, node, path, Some(id))?;
                path.pop_back();
            }
        }

        Ok(())
    }

    fn walk_children(
        &mut self,
        store: &Store,
        node: &rvm_rs::store::node::Node,
        path: &mut VecDeque<String>,
        parent_stable: Option<u64>,
    ) -> Result<()> {
        let mut child = node.first_child;
        while let Some(child_id) = child {
            let child_node = store
                .get_node(child_id)
                .ok_or_else(|| anyhow!("无效的子节点 ID: {}", child_id.0))?;
            self.walk(store, child_id, path, parent_stable)?;
            child = child_node.next;
        }
        Ok(())
    }

    fn build_geometry(&mut self, index: usize, geometry: &Geometry) -> RvmGeometry {
        self.geometry_count += 1;
        let geo_type = geo_type_name(geometry.geo_type).to_string();
        *self.geo_type_counts.entry(geo_type.clone()).or_insert(0) += 1;

        let mut subresolution_shells: Vec<ShellComponent> = Vec::new();
        let (bbox_world_mm, degenerate) = match facet_group_shell_components(geometry) {
            Some(components) => {
                self.facet_aabb_from_vertices += 1;
                let (aabb, dropped) = aabb_of_resolved_shells(components);
                self.subresolution_shell_count += dropped.len();
                subresolution_shells = dropped;
                match aabb {
                    Some(aabb) => (Some(aabb), is_degenerate(&aabb)),
                    // 整条记录一张能定包围盒的面都没有（全是亚容差壳，或一个三角形
                    // 都铺不出来）：与「退化成一个点」同样处理，不参与空间对拍。
                    None => (None, true),
                }
            }
            None => {
                let bbox = geometry.bbox_world;
                if bbox.is_valid() {
                    let scaled = [
                        bbox.min.x as f64 * M_TO_MM,
                        bbox.min.y as f64 * M_TO_MM,
                        bbox.min.z as f64 * M_TO_MM,
                        bbox.max.x as f64 * M_TO_MM,
                        bbox.max.y as f64 * M_TO_MM,
                        bbox.max.z as f64 * M_TO_MM,
                    ];
                    let degenerate = is_degenerate(&scaled);
                    (Some(scaled), degenerate)
                } else {
                    (None, true)
                }
            }
        };
        if degenerate {
            self.degenerate_bbox_count += 1;
        }

        let m = &geometry.transform.matrix3;
        let t = geometry.transform.translation;
        RvmGeometry {
            index,
            kind: kind_name(&geometry.kind).to_string(),
            geo_type,
            detail: detail_payload(&geometry.kind),
            transform: json!({
                "matrix3": [
                    m.x_axis.x, m.x_axis.y, m.x_axis.z,
                    m.y_axis.x, m.y_axis.y, m.y_axis.z,
                    m.z_axis.x, m.z_axis.y, m.z_axis.z,
                ],
                "translation_mm": [
                    t.x as f64 * M_TO_MM,
                    t.y as f64 * M_TO_MM,
                    t.z as f64 * M_TO_MM,
                ],
            }),
            bbox_world_mm,
            bbox_degenerate: degenerate,
            extra: {
                let mut extra = json!({
                    "color": geometry.color,
                    "color_rgb": geometry.color_rgb,
                    "transparency": geometry.transparency,
                    "sample_start_angle": geometry.sample_start_angle,
                });
                // 被摘掉的壳留在快照里可查：位置、体积、面积、平均厚度。对拍侧不读它，
                // 它是给人看的——「基准哪一处退化了、退化成什么样」。
                if !subresolution_shells.is_empty() {
                    extra["subresolution_shells"] = json!(
                        subresolution_shells
                            .iter()
                            .map(|shell| {
                                json!({
                                    "aabb_world_mm": shell.aabb_world_mm,
                                    "volume_mm3": shell.volume_mm3,
                                    "area_mm2": shell.area_mm2,
                                    "mean_thickness_mm": shell.mean_thickness_mm(),
                                })
                            })
                            .collect::<Vec<_>>()
                    );
                }
                extra
            },
        }
    }
}

fn is_degenerate(aabb: &[f64; 6]) -> bool {
    (aabb[3] - aabb[0]).abs() < 1e-6
        && (aabb[4] - aabb[1]).abs() < 1e-6
        && (aabb[5] - aabb[2]).abs() < 1e-6
}

/// FacetGroup 的世界 AABB，从这条记录**自己的顶点**实算（mm）。非 FacetGroup 返回 None。
///
/// PRIM 记录自带的那 6 个 f32 有**两个帧**，而且不按 kind 分。实测 `test_data/rvm/` 六份
/// E3D 3.1 导出物、432 条记录：
///
/// - 解析图元（Box 170 / Cylinder 15 / Snout 6 / CircularTorus 5）**196/196** 存的是图元
///   自身局部系的盒——`bbox.transform(&transform)` 对它们是对的，所以这条路径原样留给 rvm-rs。
/// - FacetGroup 236 条里有 **228 条**存的是 `AABB(matrix3 · v × 1000)`，即**乘过旋转、
///   未加平移**的帧。对这些记录再乘一次 transform 就是多转了一遍：`C-IY-1R330-B.rvm.json`
///   的 `aabb_world_mm` 因此 **18/18 个带面片的成员全错，最大 158.4 mm**。
/// - 剩下 8 条（`WALL 1..4` / `STWALL 1..4 of CWALL /1RS-WF03-W-C-RR001`）又落回局部系。
///   整族切换、两个候选差着上千毫米，不是浮点抖动——**帧由 E3D 写出端的代码路径决定，
///   不由 kind 决定**，所以按 kind 查表迟早翻车。
///
/// 顶点在手时这些都不必猜：`世界 mm = (matrix3 · v + translation) × 1000` 恒成立
/// （顶点存 mm、matrix3 含 0.001、translation 存米），直接实算即可绕开整个帧问题。
/// 这与 e3d-model `examples/rvm_facets_dump.rs` 的 `polygons_world_mm` 同一口径，
/// 因此 `rvm_compare` 给不给 `--facets` 得到的基准 AABB 一致。
///
/// 实算的粒度不是「整条记录的全部顶点」，而是**按共享顶点连成的每一片壳**
/// （[`ShellComponent`]）。E3D 在共壁接缝上会留下塌成零厚度的面片：1RS-WF03 的 GWALL 13
/// 在主壳之外 20 mm 处有一张两个 xy 点（相距 0.009 mm）、四个顶点、体积为 0 的「面」，
/// 它把基准的 AABB 往 x 正向顶出 20.148 mm；生成侧同一处是一根 4 µm 的针，已按
/// `2·V/A < RES_TOL_MM` 丢掉（e3d-model `solid::drop_subresolution_films`）。尺子两边要同一把，
/// 所以这里同样按 `2·V/A` 判每片壳，薄到那份上的不参与定包围盒——
/// 否则 gen 的几何更干净，AABB 这把尺反而判红。
fn facet_group_shell_components(geometry: &Geometry) -> Option<Vec<ShellComponent>> {
    use rvm_rs::export::Tessellate;
    use rvm_rs::export::tessellator::get_scale;

    let GeometryKind::FacetGroup(facet) = &geometry.kind else {
        return None;
    };
    // 与 `mesh_compare::add_geometry` 同一条三角化：FacetGroup 忽略容差与缩放参数。
    let scale = get_scale(&geometry.transform.matrix3.into());
    let tri = facet.tessellate(1.0e-3_f32, scale);
    // `to_cols_array()` 列主序 3×3 + 平移；手算避开 glam 版本冲突（rvm-rs 的 glam
    // 与本仓的 glam 是依赖图里两份不同的 crate）。
    let cols = geometry.transform.to_cols_array().map(|c| c as f64);
    let vertices: Vec<[f64; 3]> = tri
        .vertices
        .chunks_exact(3)
        .map(|v| {
            let (x, y, z) = (v[0] as f64, v[1] as f64, v[2] as f64);
            [
                (cols[0] * x + cols[3] * y + cols[6] * z + cols[9]) * M_TO_MM,
                (cols[1] * x + cols[4] * y + cols[7] * z + cols[10]) * M_TO_MM,
                (cols[2] * x + cols[5] * y + cols[8] * z + cols[11]) * M_TO_MM,
            ]
        })
        .collect();
    let triangles: Vec<[u32; 3]> = tri
        .indices
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    Some(shell_components(&vertices, &triangles))
}

/// 一条 FacetGroup 记录里按共享顶点连成的一片壳。
#[derive(Debug, Clone, PartialEq)]
struct ShellComponent {
    aabb_world_mm: [f64; 6],
    /// 绕这片壳自身重心算的有向体积的绝对值（mm³）。闭壳与参考点无关；一张零体积
    /// 的开面绕自身重心算恒为 0，不会因为离原点远而虚高。
    volume_mm3: f64,
    area_mm2: f64,
}

impl ShellComponent {
    /// `2·V/A`：闭壳的平均厚度（薄板给厚度，针给截面半径），与 e3d-model
    /// `solid::drop_subresolution_films` 同一把尺。零面积按 0 算。
    fn mean_thickness_mm(&self) -> f64 {
        if self.area_mm2 > 0.0 {
            2.0 * self.volume_mm3 / self.area_mm2
        } else {
            0.0
        }
    }

    /// 薄于 libgm 面级判定容差 [`RES_TOL_MM`]（0.051 mm）的壳：E3D 自己就分辨不出它，
    /// 它不该定包围盒。
    fn is_subresolution(&self) -> bool {
        self.mean_thickness_mm() < RES_TOL_MM
    }
}

/// 同一位置的顶点在相邻多边形里各存一份，按这个距离合并成一个点再连片。
/// RVM 顶点是 f32，世界 mm 下的量化噪声约 1e-3 mm；取 5 倍，且远小于 [`RES_TOL_MM`]。
const WELD_TOL_MM: f64 = 0.005;

/// 把世界 mm 三角网格按共享顶点分成连通片，逐片记 AABB / 面积 / 体积。
fn shell_components(vertices_mm: &[[f64; 3]], triangles: &[[u32; 3]]) -> Vec<ShellComponent> {
    let weld = weld_vertices(vertices_mm);
    let mut dsu = Dsu::new(vertices_mm.len());
    for t in triangles {
        let [a, b, c] = t.map(|i| weld[i as usize]);
        dsu.union(a, b);
        dsu.union(a, c);
    }

    // 先按片归拢三角形，重心用面积加权（零面积的片退回顶点平均）。
    let mut groups: HashMap<usize, Vec<[usize; 3]>> = HashMap::new();
    for t in triangles {
        let corners = t.map(|i| i as usize);
        groups
            .entry(dsu.find(weld[corners[0]]))
            .or_default()
            .push(corners);
    }
    let mut keys: Vec<usize> = groups.keys().copied().collect();
    keys.sort_unstable();

    keys.into_iter()
        .map(|key| {
            let tris = &groups[&key];
            let mut aabb: Option<[f64; 6]> = None;
            let mut area = 0.0_f64;
            let mut weighted = [0.0_f64; 3];
            let mut plain = [0.0_f64; 3];
            let mut corner_count = 0.0_f64;
            for &[ia, ib, ic] in tris {
                let (a, b, c) = (vertices_mm[ia], vertices_mm[ib], vertices_mm[ic]);
                for p in [a, b, c] {
                    aabb = Some(match aabb {
                        None => [p[0], p[1], p[2], p[0], p[1], p[2]],
                        Some(box_) => merge_aabb(box_, [p[0], p[1], p[2], p[0], p[1], p[2]]),
                    });
                    for axis in 0..3 {
                        plain[axis] += p[axis];
                    }
                    corner_count += 1.0;
                }
                let tri_area = 0.5 * norm(cross(sub(b, a), sub(c, a)));
                area += tri_area;
                for axis in 0..3 {
                    weighted[axis] += tri_area * (a[axis] + b[axis] + c[axis]) / 3.0;
                }
            }
            let centroid = if area > 0.0 {
                weighted.map(|w| w / area)
            } else {
                plain.map(|p| p / corner_count.max(1.0))
            };
            let mut signed_volume = 0.0;
            for &[ia, ib, ic] in tris {
                let a = sub(vertices_mm[ia], centroid);
                let b = sub(vertices_mm[ib], centroid);
                let c = sub(vertices_mm[ic], centroid);
                signed_volume += dot(a, cross(b, c)) / 6.0;
            }
            ShellComponent {
                aabb_world_mm: aabb.expect("每片至少一个三角形"),
                volume_mm3: signed_volume.abs(),
                area_mm2: area,
            }
        })
        .collect()
}

/// 每个顶点 → 它所在焊点的代表顶点下标。哈希网格按 [`WELD_TOL_MM`] 分格，查 27 邻格。
fn weld_vertices(vertices_mm: &[[f64; 3]]) -> Vec<usize> {
    let cell = |v: f64| (v / WELD_TOL_MM).floor() as i64;
    let mut grid: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
    let mut representative = vec![0usize; vertices_mm.len()];
    'vertices: for (index, p) in vertices_mm.iter().enumerate() {
        let (cx, cy, cz) = (cell(p[0]), cell(p[1]), cell(p[2]));
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(bucket) = grid.get(&(cx + dx, cy + dy, cz + dz)) {
                        if let Some(&hit) = bucket
                            .iter()
                            .find(|&&other| norm(sub(vertices_mm[other], *p)) <= WELD_TOL_MM)
                        {
                            representative[index] = representative[hit];
                            continue 'vertices;
                        }
                    }
                }
            }
        }
        representative[index] = index;
        grid.entry((cx, cy, cz)).or_default().push(index);
    }
    representative
}

/// 有面的壳的 AABB 并集，以及被摘掉的亚容差壳。
fn aabb_of_resolved_shells(
    components: Vec<ShellComponent>,
) -> (Option<[f64; 6]>, Vec<ShellComponent>) {
    let mut union: Option<[f64; 6]> = None;
    let mut dropped = Vec::new();
    for component in components {
        if component.is_subresolution() {
            dropped.push(component);
            continue;
        }
        union = Some(match union {
            None => component.aabb_world_mm,
            Some(a) => merge_aabb(a, component.aabb_world_mm),
        });
    }
    (union, dropped)
}

struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra.max(rb)] = ra.min(rb);
        }
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
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

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn group_attrs(store: &Store, group: &rvm_rs::store::node::GroupNode) -> Option<serde_json::Value> {
    let mut map = serde_json::Map::new();
    for attr in &group.attributes {
        let key = store.get_string(attr.key).trim();
        if key.is_empty() {
            continue;
        }
        let value = store.get_string(attr.value).trim();
        map.insert(key.to_string(), json!(value));
    }
    (!map.is_empty()).then(|| serde_json::Value::Object(map))
}

fn merge_aabb(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].min(b[2]),
        a[3].max(b[3]),
        a[4].max(b[4]),
        a[5].max(b[5]),
    ]
}

fn merge_geometry_aabb(
    union: Option<[f64; 6]>,
    bbox: Option<[f64; 6]>,
    degenerate: bool,
) -> Option<[f64; 6]> {
    if degenerate {
        return union;
    }
    match (union, bbox) {
        (Some(union), Some(bbox)) => Some(merge_aabb(union, bbox)),
        (None, bbox) => bbox,
        (union, None) => union,
    }
}

fn sanitize(raw: &str, fallback: impl FnOnce() -> String) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        fallback()
    } else {
        trimmed.to_string()
    }
}

fn join_path(path: &VecDeque<String>) -> String {
    let mut out = String::new();
    for part in path {
        out.push('/');
        out.push_str(part.trim_start_matches('/'));
    }
    out
}

fn now_string() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn geo_type_name(geo_type: GeometryType) -> &'static str {
    match geo_type {
        GeometryType::Primitive => "Primitive",
        GeometryType::Obstruction => "Obstruction",
        GeometryType::Insulation => "Insulation",
    }
}

fn kind_name(kind: &GeometryKind) -> &'static str {
    match kind {
        GeometryKind::Pyramid(_) => "Pyramid",
        GeometryKind::Box(_) => "Box",
        GeometryKind::RectangularTorus(_) => "RectangularTorus",
        GeometryKind::CircularTorus(_) => "CircularTorus",
        GeometryKind::EllipticalDish(_) => "EllipticalDish",
        GeometryKind::SphericalDish(_) => "SphericalDish",
        GeometryKind::Snout(_) => "Snout",
        GeometryKind::Cylinder(_) => "Cylinder",
        GeometryKind::Sphere(_) => "Sphere",
        GeometryKind::Line(_) => "Line",
        GeometryKind::FacetGroup(_) => "FacetGroup",
    }
}

/// 原语参数原样落盘，供 L2 参数级对拍逐项比较。
/// FacetGroup 只记顶点规模——网格化几何没有解析参数，只能降级到 AABB 比对。
fn detail_payload(kind: &GeometryKind) -> serde_json::Value {
    match kind {
        GeometryKind::Pyramid(d) => json!({
            "bottom": d.bottom, "top": d.top, "offset": d.offset, "height": d.height,
        }),
        GeometryKind::Box(d) => json!({ "lengths": d.lengths }),
        GeometryKind::RectangularTorus(d) => json!({
            "inner_radius": d.inner_radius,
            "outer_radius": d.outer_radius,
            "height": d.height,
            "angle": d.angle,
        }),
        GeometryKind::CircularTorus(d) => json!({
            "offset": d.offset, "radius": d.radius, "angle": d.angle,
        }),
        GeometryKind::EllipticalDish(d) => json!({
            "base_radius": d.base_radius, "height": d.height,
        }),
        GeometryKind::SphericalDish(d) => json!({
            "base_radius": d.base_radius, "height": d.height,
        }),
        GeometryKind::Snout(d) => json!({
            "radius_bottom": d.radius_bottom,
            "radius_top": d.radius_top,
            "height": d.height,
            "offset_x": d.offset_x,
            "offset_y": d.offset_y,
            "bottom_shear_x": d.bottom_shear_x,
            "bottom_shear_y": d.bottom_shear_y,
            "top_shear_x": d.top_shear_x,
            "top_shear_y": d.top_shear_y,
        }),
        GeometryKind::Cylinder(d) => json!({ "radius": d.radius, "height": d.height }),
        GeometryKind::Sphere(d) => json!({ "radius": d.radius }),
        GeometryKind::Line(d) => json!({
            "start_radius": d.start_radius, "end_radius": d.end_radius,
        }),
        GeometryKind::FacetGroup(d) => {
            let polygons = d.polygons.len();
            let contours: usize = d.polygons.iter().map(|p| p.contours.len()).sum();
            let vertices: usize = d
                .polygons
                .iter()
                .flat_map(|p| p.contours.iter())
                .map(|c| c.vertices.len())
                .sum();
            json!({ "polygons": polygons, "contours": contours, "vertices": vertices })
        }
    }
}

/// 便于 CLI 直接落盘。
pub fn import_and_save(options: &ImportOptions) -> Result<RvmSnapshot> {
    let snapshot = import_rvm(options)?;
    snapshot.save(&options.out_path)?;
    Ok(snapshot)
}

pub fn default_snapshot_path(rvm_path: &Path) -> PathBuf {
    let mut path = rvm_path.to_path_buf();
    let stem = rvm_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("snapshot");
    path.set_file_name(format!("{stem}.rvm.json"));
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_aabb_ignores_degenerate_geometry() {
        let valid = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let point = [99.0; 6];

        assert_eq!(merge_geometry_aabb(None, Some(point), true), None);
        assert_eq!(
            merge_geometry_aabb(Some(valid), Some(point), true),
            Some(valid)
        );
        assert_eq!(merge_geometry_aabb(None, Some(valid), false), Some(valid));
    }

    /// 一只闭合盒壳，每个面自带四个顶点（相邻面不共享下标，只共享坐标）——
    /// 逼着焊点那一步真的工作。返回 (顶点, 三角形)。
    fn boxed_shell(min: [f64; 3], max: [f64; 3]) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let corner = |mask: u8| {
            [
                if mask & 1 != 0 { max[0] } else { min[0] },
                if mask & 2 != 0 { max[1] } else { min[1] },
                if mask & 4 != 0 { max[2] } else { min[2] },
            ]
        };
        // 每个面按逆时针（从外看）给四个角：法向朝外，体积为正。
        let faces: [[u8; 4]; 6] = [
            [0, 2, 3, 1], // -z
            [4, 5, 7, 6], // +z
            [0, 1, 5, 4], // -y
            [2, 6, 7, 3], // +y
            [0, 4, 6, 2], // -x
            [1, 3, 7, 5], // +x
        ];
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        for face in faces {
            let base = vertices.len() as u32;
            vertices.extend(face.iter().map(|&mask| corner(mask)));
            triangles.push([base, base + 1, base + 2]);
            triangles.push([base, base + 2, base + 3]);
        }
        (vertices, triangles)
    }

    #[test]
    fn shell_components_split_by_shared_vertices_and_measure_thickness() {
        // 一块 100 × 100 × 1 的板（闭壳），加一张 20 mm 之外、0.009 mm 宽、贯穿 50 mm 高的
        // 零体积面——正是 GWALL 13 那种退化的形状。
        let (mut vertices, mut triangles) = boxed_shell([0.0, 0.0, 0.0], [100.0, 100.0, 1.0]);
        let base = vertices.len() as u32;
        vertices.extend([
            [120.0, 0.0, 0.0],
            [120.009, 0.0, 0.0],
            [120.009, 0.0, 50.0],
            [120.0, 0.0, 50.0],
        ]);
        triangles.push([base, base + 1, base + 2]);
        triangles.push([base, base + 2, base + 3]);

        let components = shell_components(&vertices, &triangles);
        assert_eq!(components.len(), 2, "盒壳与飘在外面的面是两片：{components:?}");

        let plate = &components[0];
        assert_eq!(plate.aabb_world_mm, [0.0, 0.0, 0.0, 100.0, 100.0, 1.0]);
        assert!((plate.volume_mm3 - 10_000.0).abs() < 1e-6, "{}", plate.volume_mm3);
        assert!((plate.area_mm2 - 20_400.0).abs() < 1e-6, "{}", plate.area_mm2);
        // 2V/A = 20000 / 20400 ≈ 0.98 mm：真板，留。
        assert!(plate.mean_thickness_mm() > RES_TOL_MM);
        assert!(!plate.is_subresolution());

        let film = &components[1];
        assert_eq!(film.aabb_world_mm, [120.0, 0.0, 0.0, 120.009, 0.0, 50.0]);
        assert!(film.volume_mm3.abs() < 1e-9, "{}", film.volume_mm3);
        assert!((film.area_mm2 - 0.45).abs() < 1e-9, "{}", film.area_mm2);
        assert!(film.is_subresolution());

        // 成员 AABB 只由真板定：那张面不再把 x 上界顶到 120。
        let (aabb, dropped) = aabb_of_resolved_shells(components);
        assert_eq!(aabb, Some([0.0, 0.0, 0.0, 100.0, 100.0, 1.0]));
        assert_eq!(dropped.len(), 1);
    }

    #[test]
    fn a_real_thin_plate_is_kept_and_a_needle_is_dropped() {
        // 0.1 mm 的薄板在容差之上（libgm 分得出它），留。
        let (vertices, triangles) = boxed_shell([0.0, 0.0, 0.0], [50.0, 50.0, 0.1]);
        let components = shell_components(&vertices, &triangles);
        assert_eq!(components.len(), 1);
        assert!(!components[0].is_subresolution(), "{:?}", components[0]);

        // 1.25 × 20.7 × 3620 的「针」按包围盒最小边看是 1.25 mm 厚，按 2V/A 看只有
        // 3.9 µm——闭壳体积与实测同量级时才算得出这一点。这里做一根 4 µm 厚的斜针。
        let (vertices, triangles) = boxed_shell([0.0, 0.0, 0.0], [0.004, 20.7, 3620.0]);
        let components = shell_components(&vertices, &triangles);
        assert_eq!(components.len(), 1);
        assert!(components[0].mean_thickness_mm() < 0.005, "{:?}", components[0]);
        assert!(components[0].is_subresolution());
    }

    #[test]
    fn welding_joins_coincident_vertices_but_not_neighbours_past_the_tolerance() {
        let vertices = vec![
            [0.0, 0.0, 0.0],
            [0.003, 0.0, 0.0], // 焊点容差内：同一个点
            [0.02, 0.0, 0.0],  // 容差外：另一个点
        ];
        let weld = weld_vertices(&vertices);
        assert_eq!(weld[1], weld[0]);
        assert_ne!(weld[2], weld[0]);
    }

    #[test]
    fn a_shell_that_is_all_film_leaves_no_aabb() {
        let vertices = vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 0.0, 10.0],
            [0.0, 0.0, 10.0],
        ];
        let triangles = vec![[0, 1, 2], [0, 2, 3]];
        let components = shell_components(&vertices, &triangles);
        let (aabb, dropped) = aabb_of_resolved_shells(components);
        assert_eq!(aabb, None);
        assert_eq!(dropped.len(), 1);
    }
}
