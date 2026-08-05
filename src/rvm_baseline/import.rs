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
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

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

        let bbox = geometry.bbox_world;
        let (bbox_world_mm, degenerate) = if bbox.is_valid() {
            let scaled = [
                bbox.min.x as f64 * M_TO_MM,
                bbox.min.y as f64 * M_TO_MM,
                bbox.min.z as f64 * M_TO_MM,
                bbox.max.x as f64 * M_TO_MM,
                bbox.max.y as f64 * M_TO_MM,
                bbox.max.z as f64 * M_TO_MM,
            ];
            let degenerate = (scaled[3] - scaled[0]).abs() < 1e-6
                && (scaled[4] - scaled[1]).abs() < 1e-6
                && (scaled[5] - scaled[2]).abs() < 1e-6;
            (Some(scaled), degenerate)
        } else {
            (None, true)
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
            extra: json!({
                "color": geometry.color,
                "color_rgb": geometry.color_rgb,
                "transparency": geometry.transparency,
                "sample_start_angle": geometry.sample_start_angle,
            }),
        }
    }
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
}
