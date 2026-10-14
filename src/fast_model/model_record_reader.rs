//! Select one complete root from the process overlay or the durable baseline.
//!
//! A resident receipt shadows RocksDB for the whole root. Durable reads are
//! returned directly and never hydrate kv-mem.
use std::collections::HashMap;

use aios_core::{GeomInstQuery, RefU64, RefnoEnum};
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use super::model_memory_store::{
    ModelMemoryStore, ProjectedGeometry, ProjectedShape, ProjectionScope,
};

pub async fn records_of_root(
    db: &Surreal<Any>,
    root: RefnoEnum,
) -> anyhow::Result<Vec<GeomInstQuery>> {
    let packed = root.refno().0 as u64;
    let e3d_root = e3d_io::refno::RefNo::new((packed >> 32) as u32, packed as u32);
    let store = ModelMemoryStore::global();
    if let Some(records) = overlay_records_of_root(store, e3d_root)? {
        return Ok(records
            .into_iter()
            .map(|record| projection_record_to_query(record, e3d_root))
            .collect());
    }
    durable_records_of_root(db, root).await
}

/// 投影里有回执的根：整根记录按库上的顺序（`inst_relate` 先、`tubi_relate` 后，各按记录 id）；没有回执回 `None`（走冷读）。
fn overlay_records_of_root(
    store: &ModelMemoryStore,
    root: e3d_io::refno::RefNo,
) -> anyhow::Result<Option<Vec<ProjectedGeometry>>> {
    let Some((receipt, mut records)) = store.root_snapshot(ProjectionScope::Current, root) else {
        return Ok(None);
    };
    anyhow::ensure!(
        records.len() == receipt.geometry_count,
        "memory overlay root {root} is incomplete: receipt expects {}, store has {}",
        receipt.geometry_count,
        records.len()
    );
    // Match the durable order: inst_relate first, then tubi_relate,
    // each ordered by record id. Eviction must not change cursor offsets.
    sort_overlay_records(&mut records);
    Ok(Some(records))
}

/// 一段隐式管身的身份（方案 B T2，2026-09-22；spec §4.5「`tube`」）：容器就是这条记录的 `refno`（BRAN），加上两端管件与同
/// `(from, to)` 内的序号——与房间层级树 `tubes=1` 的直段 `(BRAN, from, to, ordinal)` 同一个四元组，前端拿它把直段行与场景对象对上。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TubeMeta {
    pub ordinal: u32,
    /// `a_b`
    pub from: String,
    /// `a_b`
    pub to: String,
}

/// `/model/records` 的一条：库行 / 投影记录折成的 [`GeomInstQuery`]（形状一字不改，`serde(flatten)`）+ 隐式管身才有的
/// [`TubeMeta`]。放在记录一级而不是 `insts[]` 里：`ModelHashInst` 在 `aios_core`，这一格只属于本仓的读面；一条管身记录恰好一个实例。
#[derive(Debug, Serialize)]
pub struct ModelRecord {
    #[serde(flatten)]
    pub inst: GeomInstQuery,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tube: Option<TubeMeta>,
}

/// [`records_of_root`] 的带直段元数据版（`/model/records` 用）：投影路直接来自 `ProjectedTube`；冷读路读 `tubi_relate` 的
/// `leave` / `arrive` / `route_ordinal`，`route_ordinal` 没落盘的老行按同 `(from, to)` 内的出现序补号（行按记录 id 排，
/// 与 `spatial_tree_tubes::from_durable_rows` 同一规则）。两端缺失的历史行 `tube` 为 `None`。
pub async fn model_records_of_root(
    db: &Surreal<Any>,
    root: RefnoEnum,
) -> anyhow::Result<Vec<ModelRecord>> {
    let packed = root.refno().0 as u64;
    let e3d_root = e3d_io::refno::RefNo::new((packed >> 32) as u32, packed as u32);
    let store = ModelMemoryStore::global();
    if let Some(records) = overlay_records_of_root(store, e3d_root)? {
        return Ok(records
            .into_iter()
            .map(|record| {
                let tube = record.tube.as_ref().map(|tube| TubeMeta {
                    ordinal: tube.route_ordinal,
                    from: refno_text(tube.from),
                    to: refno_text(tube.to),
                });
                ModelRecord {
                    inst: projection_record_to_query(record, e3d_root),
                    tube,
                }
            })
            .collect());
    }
    durable_model_records_of_root(db, root).await
}

/// 投影里一根**名下、某个节点以下**的记录（ADR-086 D1：`/model/records` 拿根以下的节点——BRAN 里的一个 VALV——来问时，
/// 内存路由等价于库上的 `anc CONTAINS <节点>`）。记录仍按整根的序排，`owner` 仍是根；根没有回执时为空
/// （调用方只在 [`ModelMemoryStore::receipt_root_above`] 给出了根时才走到这里）。
pub fn overlay_records_under_node(
    store: &ModelMemoryStore,
    root: e3d_io::refno::RefNo,
    node: e3d_io::refno::RefNo,
) -> anyhow::Result<Vec<ModelRecord>> {
    let Some(records) = overlay_records_of_root(store, root)? else {
        return Ok(Vec::new());
    };
    let packed_node = crate::fast_model::model_memory_store::packed_refno(node);
    Ok(records
        .into_iter()
        .filter(|record| record.ancestors.contains(&packed_node))
        .map(|record| {
            let tube = record.tube.as_ref().map(|tube| TubeMeta {
                ordinal: tube.route_ordinal,
                from: refno_text(tube.from),
                to: refno_text(tube.to),
            });
            ModelRecord {
                inst: projection_record_to_query(record, root),
                tube,
            }
        })
        .collect())
}

fn refno_text(refno: e3d_io::refno::RefNo) -> String {
    RefU64::from_two_nums(refno.word0, refno.word1).to_string()
}

/// 管身行的直段三格（第三句 SQL 的行，与第二句的管身行**同序**——两句同一个作用域、同一个 `ORDER BY record_order`）。
/// 不与 `GeomInstQuery` 合成一行 `serde(flatten)` 读：SurrealDB 的反序列化器过 flatten 缓冲时 `RefnoEnum` 这类 untagged 枚举
/// 会报「untagged and internally tagged enums do not support enum input」，所以管身行读两遍、按序配对。
#[derive(Debug, Deserialize)]
struct TubeColumns {
    #[serde(default)]
    tube_from: Option<String>,
    #[serde(default)]
    tube_to: Option<String>,
    #[serde(default)]
    tube_ordinal: Option<u32>,
}

/// 管身行的直段三格那一句：作用域与 [`durable_records_sql`] 的管身句相同（两路 `array::union`、`ORDER BY record_order`），
/// 只投 `record_order` + `leave` / `arrive`（折成 `a_b`）/ `route_ordinal`——两端缺的历史行读成 NONE。
fn durable_tube_meta_sql(packed: u64, owner: &str) -> String {
    let projection = "id AS record_order, \
         IF leave != NONE { record::id(leave) } ELSE { NONE } AS tube_from, \
         IF arrive != NONE { record::id(arrive) } ELSE { NONE } AS tube_to, \
         route_ordinal AS tube_ordinal";
    let [by_anc, by_owner] = durable_scope_reads("tubi_relate", projection, packed, owner);
    format!("SELECT * FROM array::union(({by_anc}), ({by_owner})) ORDER BY record_order;")
}

/// 管身记录 + 同序的三格 → [`ModelRecord`]：`route_ordinal` 缺的按同 `(容器, from, to)` 内出现序补号；两端缺的没有 `tube`。
fn pair_tube_records(tubes: Vec<GeomInstQuery>, metas: Vec<TubeColumns>) -> Vec<ModelRecord> {
    let mut seen: HashMap<(String, String, String), u32> = HashMap::new();
    tubes
        .into_iter()
        .zip(metas)
        .map(|(inst, meta)| {
            let tube = match (meta.tube_from, meta.tube_to) {
                (Some(from), Some(to)) => {
                    let container = inst.refno.to_string();
                    let slot = seen.entry((container, from.clone(), to.clone())).or_insert(0);
                    let ordinal = meta.tube_ordinal.unwrap_or(*slot);
                    *slot += 1;
                    Some(TubeMeta { ordinal, from, to })
                }
                _ => None,
            };
            ModelRecord { inst, tube }
        })
        .collect()
}

async fn durable_model_records_of_root(
    db: &Surreal<Any>,
    root: RefnoEnum,
) -> anyhow::Result<Vec<ModelRecord>> {
    let packed = root.refno().0;
    let owner = root.to_pe_key();
    let sql = format!(
        "{}\n{}",
        durable_records_sql(packed, &owner),
        durable_tube_meta_sql(packed, &owner)
    );
    let mut response = db.query(sql).await?.check()?;
    let insts: Vec<GeomInstQuery> = response.take(0)?;
    let tubes: Vec<GeomInstQuery> = response.take(1)?;
    let metas: Vec<TubeColumns> = response.take(2)?;
    anyhow::ensure!(
        tubes.len() == metas.len(),
        "durable tube rows of {root}: {} records but {} identity rows",
        tubes.len(),
        metas.len()
    );
    let mut records = insts
        .into_iter()
        .map(|inst| ModelRecord { inst, tube: None })
        .collect::<Vec<_>>();
    records.extend(pair_tube_records(tubes, metas));
    Ok(records)
}

fn sort_overlay_records(records: &mut [ProjectedGeometry]) {
    records.sort_by(|a, b| (a.tube.is_some(), &a.record_id).cmp(&(b.tube.is_some(), &b.record_id)));
}

/// 只读库上的基线：一根名下全部 `inst_relate` / `tubi_relate` 行（有世界盒与世界变换的），不看投影。
/// 整库房间重算（ADR-073 D13）对**不在投影里**的根——初始化的模型行（ADR-077 D5）、落盘形态重启前
/// 生成的——从这里读回几何再算归属（`room_model::durable_room_inputs`）；投影里有回执的根不该走这里，
/// 投影那份才是本进程的现值。
pub(crate) async fn durable_records_of_root(
    db: &Surreal<Any>,
    root: RefnoEnum,
) -> anyhow::Result<Vec<GeomInstQuery>> {
    let packed = root.refno().0;
    let owner = root.to_pe_key();
    let mut response = db
        .query(durable_records_sql(packed, &owner))
        .await?
        .check()?;
    let mut records: Vec<GeomInstQuery> = response.take(0)?;
    records.extend(response.take::<Vec<GeomInstQuery>>(1)?);
    Ok(records)
}

/// 一张表的两路作用域读——`(按 anc 的一句, 按 in 的一句)`：子树行 `anc CONTAINS <根>` 走 `idx_<表>_anc`，
/// 根自己的几何 / `anc` 缺失的历史行 `in = <根>` 走 `idx_<表>_in`。两句只差 WHERE 的第一个谓词，
/// 都不带 `ORDER BY`——排序留给并集之后那一层。
fn durable_scope_reads(table: &str, projection: &str, packed: u64, owner: &str) -> [String; 2] {
    [
        format!(
            "SELECT {projection} FROM {table} WHERE anc CONTAINS {packed} \
             AND aabb.d != NONE AND world_trans.d != NONE"
        ),
        format!(
            "SELECT {projection} FROM {table} WHERE in = {owner} \
             AND aabb.d != NONE AND world_trans.d != NONE"
        ),
    ]
}

/// 冷读的两句 SQL：`inst_relate` 一句、`tubi_relate` 一句，各自 `ORDER BY record_order`（= 记录 id），
/// 调用方 `take(0)` / `take(1)`。
///
/// 每张表的作用域是两路（[`durable_scope_reads`]），**分开读、各走各的索引、再 `array::union`**，
/// 不写成一条 `anc CONTAINS … OR in = …`：优化器碰到 OR 再挂上别的 AND 谓词（这里的
/// `aabb.d != NONE AND world_trans.d != NONE`）整条放弃索引退回 `Iterate Table`——单独一条 OR 它会做
/// 索引并集，一加 AND 就不做了（SurrealDB 2.1.4 rocksdb 现场 EXPLAIN 实测：47,751 行的 `inst_relate`
/// 上一根 0.76 s，拆开 2–3 ms；mem 引擎同样，`durable_cold_read_scopes_are_planned_through_indexes`
/// 钉住）。整库房间重算（ADR-073 D13 / ADR-082）对 overlay 之外的每一根都要来一次冷读，那 0.8 s 乘上
/// 根数就是 O(根 × 库)——d-149 真机 7997（6,772 根、overlay 上限 4）上一窗跑成 90 分钟、看着像卡死。
/// 与 [`render_spooled_scope_cleanup`] 同一条纪律。
///
/// 并集在库端算（两路的行只在 `anc` 含自身的管身行上重合，`array::union` 去重）、按 `record_order`
/// 排：行集与顺序和原来那一条 OR 逐字相同（6 根现场对过），读路的游标偏移不变。
///
/// [`render_spooled_scope_cleanup`]: super::model_db_adapter
fn durable_records_sql(packed: u64, owner: &str) -> String {
    let inst_projection = format!(
        "id AS record_order, in AS refno, in.old_pe AS old_refno, {owner} AS owner, \
         generic, aabb.d AS world_aabb, world_trans.d AS world_trans, \
         out.ptset.d.pt AS pts, booled_id != NONE AS has_neg, \
         IF booled_id != NONE {{ [{{geo_hash: booled_id}}] }} \
         ELSE {{ insts_flat ?? (SELECT transform ?? trans.d AS transform, \
             record::id(out) AS geo_hash FROM out->geo_relate \
             WHERE visible AND out.meshed AND geo_type = 'Pos' \
               AND (transform != NONE OR trans.d != NONE)) }} AS insts"
    );
    // 管身行多三格直段身份（方案 B T2）：`leave` / `arrive` 折成 `a_b`、`route_ordinal` 老行读成 NONE；只有
    // `model_records_of_root` 消费它们，`records_of_root` 的 `GeomInstQuery` 不认这三格、原样忽略（同 `record_order`）。
    let tubi_projection = format!(
        "id AS record_order, in AS refno, in.old_pe AS old_refno, {owner} AS owner, \
         'TUBI' AS generic, aabb.d AS world_aabb, world_trans.d AS world_trans, \
         false AS has_neg, \
         [{{geo_hash: record::id(out), is_tubi: true, is_invalid_tubi: invalid ?? false}}] AS insts, \
         IF leave != NONE {{ record::id(leave) }} ELSE {{ NONE }} AS tube_from, \
         IF arrive != NONE {{ record::id(arrive) }} ELSE {{ NONE }} AS tube_to, \
         route_ordinal AS tube_ordinal"
    );
    let [inst_by_anc, inst_by_owner] =
        durable_scope_reads("inst_relate", &inst_projection, packed, owner);
    let [tubi_by_anc, tubi_by_owner] =
        durable_scope_reads("tubi_relate", &tubi_projection, packed, owner);
    format!(
        "SELECT * FROM array::union(({inst_by_anc}), ({inst_by_owner})) ORDER BY record_order;\n\
         SELECT * FROM array::union(({tubi_by_anc}), ({tubi_by_owner})) ORDER BY record_order;"
    )
}

/// 库上一条隐式管身行里房间层级树 `tubes=1` 要的那几格（`spatial_tree_tubes` 折成 `TubeRecord`；计划
/// `docs/plans/2026-09-22-room-tree-tube-segments-backend-dev-plan.md` §2.2 / §2.7）。`tube_ordinal` 是 T2 才落盘的
/// `route_ordinal`，老行读成 `None`、由调用方按同 `(from, to)` 内的出现序补号——所以行**必须**按 `record_order`（分支槽位）排回来。
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub(crate) struct DurableTubeRow {
    /// 容器 BRAN，`a_b`（`record::id(in)`）。
    pub container: String,
    /// 离开侧构件，`a_b`（`record::id(leave)`）。
    pub tube_from: String,
    /// 到达侧构件，`a_b`（`record::id(arrive)`）。
    pub tube_to: String,
    #[serde(default)]
    pub tube_ordinal: Option<u32>,
    pub world_aabb: parry3d::bounding_volume::Aabb,
    pub world_trans: bevy_transform::components::Transform,
    #[serde(default)]
    pub invalid: bool,
}

/// 一批 BRAN 名下隐式管身行的冷读 SQL：**只读 `tubi_relate`、只按 `in`**——管身行的 `in` 就是容器 BRAN，不需要
/// [`durable_scope_reads`] 的 `anc` 那一路；每根一句等值（`idx_tubi_relate_in`，`pdms_inst::INST_RELATE_INDEX_SQL`），
/// 不写 `in IN [...]`（本 fork 2.1.4 对 `IN` 的索引联合支持没复核过，`pdms_inst::render_watch_scope_filter` 的注记），
/// 拼成一条多语句一次往返、调用方 `take(i)`。两端缺失的历史行（`leave` / `arrive` 为 NONE）没有身份，在库端就滤掉。
/// 钉 `durable_tube_rows_are_planned_through_the_in_index`：每句 `EXPLAIN FULL` 不出现 `Iterate Table`、落在 `idx_tubi_relate_in`。
pub(crate) fn durable_tube_rows_sql(branches: &[RefU64]) -> String {
    branches
        .iter()
        .map(|branch| {
            let owner = RefnoEnum::from(*branch).to_pe_key();
            format!(
                "SELECT record::id(in) AS container, record::id(leave) AS tube_from, record::id(arrive) AS tube_to, \
                 route_ordinal AS tube_ordinal, aabb.d AS world_aabb, world_trans.d AS world_trans, \
                 invalid ?? false AS invalid, id AS record_order FROM tubi_relate \
                 WHERE in = {owner} AND aabb.d != NONE AND world_trans.d != NONE \
                 AND leave != NONE AND arrive != NONE ORDER BY record_order;"
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 冷读一批 BRAN 名下的隐式管身行（[`durable_tube_rows_sql`]）。空清单不碰库。返回顺序 = `branches` 顺序 × 各自槽位序。
pub(crate) async fn durable_tube_rows_of_branches(
    db: &Surreal<Any>,
    branches: &[RefU64],
) -> anyhow::Result<Vec<DurableTubeRow>> {
    if branches.is_empty() {
        return Ok(Vec::new());
    }
    let mut response = db.query(durable_tube_rows_sql(branches)).await?.check()?;
    let mut rows = Vec::new();
    for index in 0..branches.len() {
        rows.extend(response.take::<Vec<DurableTubeRow>>(index)?);
    }
    Ok(rows)
}

fn projection_record_to_query(
    record: ProjectedGeometry,
    root: e3d_io::refno::RefNo,
) -> GeomInstQuery {
    // Durable inst_relate encodes every non-tube Baked projection with a
    // booled_id; canonical rows and tubi_relate rows do not have one.
    let has_neg = record.tube.is_none() && matches!(&record.shape, ProjectedShape::Baked);
    let local_transform = match (&record.shape, &record.tube) {
        (
            ProjectedShape::Canonical {
                local_transform, ..
            },
            None,
        ) => *local_transform,
        _ => glam::DMat4::IDENTITY,
    };
    let (local_scale, local_rotation, local_translation) =
        local_transform.to_scale_rotation_translation();
    let (scale, rotation, translation) = record.world_transform.to_scale_rotation_translation();
    let source = RefnoEnum::from(RefU64::from_two_nums(
        record.source_refno.word0,
        record.source_refno.word1,
    ));
    GeomInstQuery {
        refno: source,
        old_refno: None,
        owner: RefnoEnum::from(RefU64::from_two_nums(root.word0, root.word1)),
        world_aabb: parry3d::bounding_volume::Aabb::new(
            record.world_bounds[0].into(),
            record.world_bounds[1].into(),
        ),
        world_trans: bevy_transform::components::Transform {
            translation: translation.as_vec3(),
            rotation: rotation.as_quat(),
            scale: scale.as_vec3(),
        },
        insts: vec![aios_core::ModelHashInst {
            geo_hash: record.mesh_id,
            transform: bevy_transform::components::Transform {
                translation: local_translation.as_vec3(),
                rotation: local_rotation.as_quat(),
                scale: local_scale.as_vec3(),
            },
            is_tubi: record.tube.is_some(),
            is_invalid_tubi: record.tube.as_ref().is_some_and(|tube| tube.invalid),
        }],
        has_neg,
        generic: record.noun,
        pts: None,
        date: None,
    }
}

#[cfg(test)]
mod tests {
    use aios_core::{RefU64, RefnoEnum};
    use e3d_io::refno::RefNo;
    use e3d_model::elmodl::GeometryId;
    use glam::DMat4;

    use super::super::model_memory_store::{ProjectedGeometry, ProjectedShape, ProjectedTube};

    fn projected(shape: ProjectedShape, tube: Option<ProjectedTube>) -> ProjectedGeometry {
        ProjectedGeometry {
            record_id: "24384_26609".into(),
            geometry_id: GeometryId::Element {
                refno: "24384/26609".into(),
            },
            source_refno: RefNo::new(24384, 26609),
            dbnum: 7997,
            source_sesno: 42,
            noun: "FLOOR".into(),
            mesh_id: "mesh".into(),
            world_transform: DMat4::IDENTITY,
            world_bounds: [[0.0; 3], [1.0; 3]],
            ancestors: vec![],
            shape,
            tube,
        }
    }

    #[test]
    fn overlay_order_matches_durable_table_order_even_when_tube_id_sorts_first() {
        let mut tube = projected(ProjectedShape::Baked, Some(ProjectedTube {
            route_ordinal: 0, from: RefNo::new(7997, 1),
            to: RefNo::new(7997, 2), invalid: false,
        }));
        tube.record_id = "0_tube".into();
        let mut first = projected(ProjectedShape::Baked, None);
        first.record_id = "a_element".into();
        let mut last = first.clone();
        last.record_id = "z_element".into();
        let mut rows = vec![tube, last, first];
        super::sort_overlay_records(&mut rows);
        assert_eq!(rows.iter().map(|r| r.record_id.as_str()).collect::<Vec<_>>(),
            vec!["a_element", "z_element", "0_tube"]);
    }

    #[test]
    fn memory_conversion_matches_the_durable_boolean_contract() {
        let root = RefNo::new(24384, 26606);
        let baked = super::projection_record_to_query(
            projected(ProjectedShape::Baked, None),
            root,
        );
        assert!(baked.has_neg, "durable baked rows have booled_id");

        let tube = ProjectedTube {
            route_ordinal: 0,
            from: RefNo::new(24384, 1),
            to: RefNo::new(24384, 2),
            invalid: false,
        };
        let tube = super::projection_record_to_query(
            projected(ProjectedShape::Baked, Some(tube)),
            root,
        );
        assert!(!tube.has_neg, "durable tubi_relate rows are never boolean rows");
    }

    /// 房间判定的「网格 → 世界」在两条路上要对得上：投影记录直接算（逐根路径 / 整库重算对投影里的根）与
    /// 库上行回读（`durable_room_inputs`，整库重算对不在投影里的根；这里用 `projection_record_to_query`
    /// 折出与库行同形的查询行）。标准图元元素两边都是 `world × local`；**标准隐式管身**存下的世界变换已折进本地
    /// 摆放、库行实例变换是单位阵，投影记录那一侧也**不许再乘**一次本地摆放——此前多乘了（2026-09-17 修）。
    /// 库行经 f32 `Transform` 分解再合成，比到 f32 精度。
    #[test]
    fn a_canonical_implied_tube_has_the_same_mesh_to_world_on_the_projected_and_durable_paths() {
        use crate::fast_model::room_model::{RoomInput, durable_room_inputs};
        use e3d_model::primitive_instance::PrimitiveMeshKey;
        let root = RefNo::new(24384, 26606);
        let world = DMat4::from_scale_rotation_translation(
            glam::DVec3::ONE,
            glam::DQuat::from_rotation_z(0.7),
            glam::DVec3::new(10.0, 20.0, 30.0),
        );
        let local = DMat4::from_scale(glam::DVec3::new(100.0, 100.0, 3000.0));
        let canonical = |tube: Option<ProjectedTube>| {
            let mut record = projected(
                ProjectedShape::Canonical {
                    key: PrimitiveMeshKey::CylinderV3 { segments: 32 },
                    local_transform: local,
                    local_bounds: [[0.0; 3], [1.0; 3]],
                },
                tube,
            );
            // 与 `e3d_model_service::persisted_world_transform` 同口径：隐式管身把本地摆放折进存下的世界变换。
            record.world_transform = if record.tube.is_some() {
                world * local
            } else {
                world
            };
            record
        };
        let close = |a: DMat4, b: DMat4, what: &str| {
            for (x, y) in a.to_cols_array().iter().zip(b.to_cols_array()) {
                assert!(
                    (x - y).abs() <= 1e-3 * (1.0 + y.abs()),
                    "{what}: {a:?} vs {b:?}"
                );
            }
        };
        let tube = Some(ProjectedTube {
            route_ordinal: 0,
            from: RefNo::new(24384, 1),
            to: RefNo::new(24384, 2),
            invalid: false,
        });
        for (record, what) in [
            (canonical(None), "标准图元元素"),
            (canonical(tube), "标准隐式管身"),
        ] {
            let projected_path = record.mesh_to_world();
            close(
                projected_path,
                world * local,
                &format!("{what}：本地摆放只乘一次"),
            );
            let rows = vec![super::projection_record_to_query(record, root)];
            let durable = durable_room_inputs(&rows);
            assert_eq!(durable.len(), 1, "{what}");
            close(
                durable[0].mesh_to_world(),
                projected_path,
                &format!("{what}：库行回读与投影记录同一个「网格 → 世界」"),
            );
        }
    }

    #[tokio::test]
    async fn cold_read_includes_baked_canonical_and_tube_but_not_other_roots() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("cold_records").await.unwrap();
        let root: aios_core::RefnoEnum = "24384_26606".into();
        let packed = root.refno().0;
        db.query(format!(r#"
          CREATE aabb:box SET d={{mins:[0,0,0],maxs:[1,1,1]}};
          CREATE trans:world SET d={{translation:[120,0,0],rotation:[0,0,0,1],scale:[1,1,1]}};
          CREATE inst_relate:baked SET in=pe:⟨24384_26609⟩, anc=[{packed}],
            aabb=aabb:box,world_trans=trans:world,generic='FLOOR',booled_id='hole';
          CREATE inst_relate:canonical SET in=pe:⟨24384_26610⟩,anc=[{packed}],
            aabb=aabb:box,world_trans=trans:world,generic='BOX',
            insts_flat=[{{geo_hash:'box',transform:{{translation:[5,0,0],rotation:[0,0,0,1],scale:[2,3,4]}}}}];
          CREATE inst_relate:other SET in=pe:⟨24384_999⟩,anc=[1],
            aabb=aabb:box,world_trans=trans:world,generic='BOX',booled_id='other';
          CREATE tubi_relate:tube SET in=pe:⟨24384_26611⟩,anc=[{packed}],
            out=inst_geo:tube,aabb=aabb:box,world_trans=trans:world,invalid=true;
          -- 根自己的几何、`anc` 缺失的历史形态：只有 `in = 根` 那一路兜得住。
          CREATE inst_relate:own SET in=pe:⟨24384_26606⟩,anc=[1],
            aabb=aabb:box,world_trans=trans:world,generic='EQUI',booled_id='own';
          -- 根自己的管身、`anc` 含自身：两路都命中，并集里只许出现一次。
          CREATE tubi_relate:both SET in=pe:⟨24384_26606⟩,anc=[{packed}],
            out=inst_geo:tube,aabb=aabb:box,world_trans=trans:world;
        "#)).await.unwrap().check().unwrap();
        let records = super::durable_records_of_root(&db, root).await.unwrap();
        assert_eq!(
            records.len(),
            5,
            "两路并集：子树 2 + 根自己 1，管身 1 + 根自己的 1（去重）"
        );
        assert!(records.iter().all(|r| r.owner == root));
        assert_eq!(
            records
                .iter()
                .map(|r| r.refno.to_string())
                .collect::<Vec<_>>(),
            [
                "24384_26609",
                "24384_26610",
                "24384_26606",
                "24384_26606",
                "24384_26611"
            ],
            "inst_relate 先、tubi_relate 后，各按记录 id 排：与原来那一条 OR 的顺序相同"
        );
        assert_eq!(
            records[2].generic, "EQUI",
            "`in = 根` 那一路补进来的根自己的几何"
        );
        assert_eq!(
            records.iter().filter(|r| r.generic == "TUBI").count(),
            2,
            "两路都命中的管身行不重复"
        );
        let canonical = records.iter().find(|r| r.generic == "BOX").unwrap();
        assert_eq!(canonical.insts[0].transform.translation.x, 5.0);
        assert_eq!(canonical.world_trans.translation.x, 120.0);
        let tube = records
            .iter()
            .find(|r| r.generic == "TUBI" && r.refno.to_string() == "24384_26611")
            .unwrap();
        assert!(!canonical.has_neg);
        assert!(!tube.has_neg);
        assert!(tube.insts[0].is_tubi && tube.insts[0].is_invalid_tubi);
        let own_tube = records
            .iter()
            .find(|r| r.generic == "TUBI" && r.refno.to_string() == "24384_26606")
            .unwrap();
        assert!(own_tube.insts[0].is_tubi && !own_tube.insts[0].is_invalid_tubi);
        let baked = records.iter().find(|r| r.generic == "FLOOR").unwrap();
        assert!(baked.has_neg);
        assert_eq!(baked.insts[0].geo_hash, "hole");
    }

    /// 冷读的每一路作用域读都必须落在索引上（`anc CONTAINS` → `idx_<表>_anc`，`in =` → `idx_<表>_in`），
    /// 计划里不许出现 `Iterate Table`；两路合成一条 `… OR …` 时优化器整条退回扫表（改写前就是这样：
    /// 47,751 行的库上一根 0.76 s，整库房间重算逐根冷读跑成 90 分钟），所以并集只许在两路各自读完之后算。
    #[tokio::test]
    async fn durable_cold_read_scopes_are_planned_through_indexes() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("cold_read_plans").await.unwrap();
        db.query(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
            .await
            .unwrap()
            .check()
            .unwrap();
        let root: aios_core::RefnoEnum = "24384_26606".into();
        let (packed, owner) = (root.refno().0, root.to_pe_key());

        let sql = super::durable_records_sql(packed, &owner);
        let statements = sql
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(
            statements.len(),
            2,
            "inst_relate 一句、tubi_relate 一句: {sql}"
        );
        for statement in &statements {
            assert!(
                statement.starts_with("SELECT * FROM array::union((")
                    && statement.ends_with(")) ORDER BY record_order;"),
                "并集在库端算、并完再排: {statement}"
            );
            assert!(
                !statement.contains("OR in =") && !statement.contains("OR anc CONTAINS"),
                "两路作用域不许合成一条 OR 谓词——优化器会整条退回扫表: {statement}"
            );
        }

        for table in ["inst_relate", "tubi_relate"] {
            let reads = super::durable_scope_reads(table, "id", packed, &owner);
            for (read, index) in reads
                .iter()
                .zip([format!("idx_{table}_anc"), format!("idx_{table}_in")])
            {
                assert!(
                    statements.iter().any(|statement| {
                        statement.contains(read.split_once(" WHERE ").unwrap().1)
                    }),
                    "冷读 SQL 里找不到这一路的谓词: {read}"
                );
                let mut response = db
                    .query(format!("{read} EXPLAIN FULL;"))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                let plan: surrealdb::Value = response.take(0).unwrap();
                let plan = plan.to_string();
                assert!(
                    !plan.contains("Iterate Table") && plan.contains(&index),
                    "{read} must be planned through {index}, plan: {plan}"
                );
            }
        }

        // 对照：改写前的那一条 OR——单靠索引并集它能走索引，一挂上 AND 谓词就整条退回扫表。
        let mut response = db
            .query(format!(
                "SELECT id FROM inst_relate WHERE (anc CONTAINS {packed} OR in = {owner}) \
                 AND aabb.d != NONE AND world_trans.d != NONE EXPLAIN FULL;"
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        let plan: surrealdb::Value = response.take(0).unwrap();
        assert!(
            plan.to_string().contains("Iterate Table"),
            "the OR shape no longer falls back to a table scan on this engine—\
             the split is then only a style choice, update this note: {plan}"
        );
    }

    /// `/model/records` 的直段元数据（方案 B T2）：管身行的 `leave` / `arrive` / `route_ordinal` 折成记录一级的 `tube{ordinal, from, to}`；
    /// `route_ordinal` 没落盘的按同 `(容器, from, to)` 内出现序补号；两端缺失的历史行与 `inst_relate` 行没有 `tube`；
    /// `GeomInstQuery` 那一半与 `records_of_root` 逐字相同（多出来的三格它不认、原样忽略）；序列化时 `tube` 缺就不出键。
    #[tokio::test]
    async fn model_records_carry_tube_identity_from_durable_rows_and_number_legacy_ordinals() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("model_records_tube").await.unwrap();
        let root: aios_core::RefnoEnum = "24384_26606".into();
        let packed = root.refno().0;
        db.query(format!(r#"
          CREATE aabb:box SET d={{mins:[0,0,0],maxs:[1,1,1]}};
          CREATE trans:world SET d={{translation:[120,0,0],rotation:[0,0,0,1],scale:[1,1,1]}};
          CREATE inst_relate:baked SET in=pe:⟨24384_26609⟩, anc=[{packed}],
            aabb=aabb:box,world_trans=trans:world,generic='FLOOR',booled_id='hole';
          -- 三段同一容器：前两段同 (leave, arrive) 且没有 route_ordinal（按出现序补 0、1）；第三段落了盘的 route_ordinal=5 原样
          CREATE tubi_relate:[pe:⟨24384_26606⟩,0] SET in=pe:⟨24384_26606⟩,anc=[{packed}],out=inst_geo:tube,
            aabb=aabb:box,world_trans=trans:world,leave=pe:⟨24384_1⟩,arrive=pe:⟨24384_2⟩;
          CREATE tubi_relate:[pe:⟨24384_26606⟩,1] SET in=pe:⟨24384_26606⟩,anc=[{packed}],out=inst_geo:tube,
            aabb=aabb:box,world_trans=trans:world,leave=pe:⟨24384_1⟩,arrive=pe:⟨24384_2⟩,invalid=true;
          CREATE tubi_relate:[pe:⟨24384_26606⟩,2] SET in=pe:⟨24384_26606⟩,anc=[{packed}],out=inst_geo:tube,
            aabb=aabb:box,world_trans=trans:world,leave=pe:⟨24384_2⟩,arrive=pe:⟨24384_3⟩,route_ordinal=5;
          -- 两端缺失的历史行：还是一条 TUBI 记录，只是没有 tube
          CREATE tubi_relate:[pe:⟨24384_26606⟩,3] SET in=pe:⟨24384_26606⟩,anc=[{packed}],out=inst_geo:tube,
            aabb=aabb:box,world_trans=trans:world;
        "#)).await.unwrap().check().unwrap();

        let records = super::durable_model_records_of_root(&db, root).await.unwrap();
        assert_eq!(records.len(), 5, "1 构件 + 4 管身");
        assert!(records[0].tube.is_none() && records[0].inst.generic == "FLOOR");
        let tubes = records[1..]
            .iter()
            .map(|record| (record.inst.generic.as_str(), record.tube.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            tubes,
            [
                ("TUBI", Some(super::TubeMeta { ordinal: 0, from: "24384_1".into(), to: "24384_2".into() })),
                ("TUBI", Some(super::TubeMeta { ordinal: 1, from: "24384_1".into(), to: "24384_2".into() })),
                ("TUBI", Some(super::TubeMeta { ordinal: 5, from: "24384_2".into(), to: "24384_3".into() })),
                ("TUBI", None),
            ]
        );
        assert!(records[2].inst.insts[0].is_invalid_tubi);

        // `GeomInstQuery` 那一半与 records_of_root 同形同序
        let plain = super::durable_records_of_root(&db, root).await.unwrap();
        assert_eq!(plain.len(), records.len());
        for (a, b) in plain.iter().zip(&records) {
            assert_eq!((a.refno.to_string(), a.generic.clone(), a.insts[0].geo_hash.clone()),
                (b.inst.refno.to_string(), b.inst.generic.clone(), b.inst.insts[0].geo_hash.clone()));
        }

        let json = serde_json::to_value(&records).unwrap();
        assert!(json[0].get("tube").is_none(), "构件行没有 tube 键");
        assert_eq!(json[1]["tube"], serde_json::json!({ "ordinal": 0, "from": "24384_1", "to": "24384_2" }));
        assert_eq!(json[1]["generic"], "TUBI", "flatten：GeomInstQuery 的键仍在顶层");
        assert!(json[1]["insts"][0]["is_tubi"].as_bool().unwrap());
        assert!(json[4].get("tube").is_none(), "两端缺失的历史行没有 tube 键");
    }

    // ---- 房间层级树 tubes=1 的管身冷读 ----

    /// 两条 BRAN 各两段管身 + 一条缺 `arrive` 的历史行 + 一条别的 BRAN 的：只按 `in` 取、缺端的在库端滤掉、
    /// 行按槽位排、`route_ordinal` 没落盘的读成 `None`、落了的原样读回；两根一次往返、顺序 = 入参顺序 × 槽位序。
    #[tokio::test]
    async fn durable_tube_rows_come_back_per_branch_in_slot_order_without_endless_rows() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("tube_rows").await.unwrap();
        db.query(
            r#"
          CREATE aabb:t0 SET d={mins:[0,0,0],maxs:[100,10,10]};
          CREATE aabb:t1 SET d={mins:[100,0,0],maxs:[300,10,10]};
          CREATE trans:t0 SET d={translation:[50,5,5],rotation:[0,0,0,1],scale:[5,5,100]};
          CREATE trans:t1 SET d={translation:[200,5,5],rotation:[0,0,0,1],scale:[5,5,200]};
          INSERT RELATION INTO tubi_relate [
            {id:tubi_relate:[pe:⟨24384_100⟩,1],in:pe:⟨24384_100⟩,out:inst_geo:m,leave:pe:⟨24384_102⟩,arrive:pe:⟨24384_103⟩,
             aabb:aabb:t1,world_trans:trans:t1,invalid:true,anc:[1],dbnum:24384},
            {id:tubi_relate:[pe:⟨24384_100⟩,0],in:pe:⟨24384_100⟩,out:inst_geo:m,leave:pe:⟨24384_101⟩,arrive:pe:⟨24384_102⟩,
             aabb:aabb:t0,world_trans:trans:t0,invalid:false,anc:[1],dbnum:24384,route_ordinal:2},
            {id:tubi_relate:[pe:⟨24384_100⟩,2],in:pe:⟨24384_100⟩,out:inst_geo:m,leave:pe:⟨24384_103⟩,
             aabb:aabb:t0,world_trans:trans:t0,anc:[1],dbnum:24384},
            {id:tubi_relate:[pe:⟨24384_200⟩,0],in:pe:⟨24384_200⟩,out:inst_geo:m,leave:pe:⟨24384_200⟩,arrive:pe:⟨24384_201⟩,
             aabb:aabb:t0,world_trans:trans:t0,anc:[1],dbnum:24384},
            {id:tubi_relate:[pe:⟨24384_300⟩,0],in:pe:⟨24384_300⟩,out:inst_geo:m,leave:pe:⟨24384_301⟩,arrive:pe:⟨24384_302⟩,
             aabb:aabb:t0,world_trans:trans:t0,anc:[1],dbnum:24384}
          ];
        "#,
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let branches = [RefU64::from_two_nums(24384, 100), RefU64::from_two_nums(24384, 200)];
        let rows = super::durable_tube_rows_of_branches(&db, &branches)
            .await
            .unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.container.as_str(), row.tube_from.as_str(), row.tube_to.as_str(), row.tube_ordinal, row.invalid))
                .collect::<Vec<_>>(),
            [
                ("24384_100", "24384_101", "24384_102", Some(2), false),
                ("24384_100", "24384_102", "24384_103", None, true),
                ("24384_200", "24384_200", "24384_201", None, false),
            ],
            "槽位 0 在 1 前、缺 arrive 的不来、别的 BRAN 不来、两根按入参顺序"
        );
        assert_eq!(rows[1].world_aabb.maxs.x, 300.0);
        assert_eq!(rows[1].world_trans.scale.z, 200.0, "长度就在世界变换的 z 缩放里");
        assert!(
            super::durable_tube_rows_of_branches(&db, &[])
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 管身冷读每句都要落在 `idx_tubi_relate_in` 上、不许 `Iterate Table`（与 `durable_cold_read_scopes_are_planned_through_indexes`
    /// 同一条纪律）：`tubes=1` 在落盘形态下对每个 BRAN 单元来一句，走不上索引就是「每根扫一遍 tubi_relate」。
    #[tokio::test]
    async fn durable_tube_rows_are_planned_through_the_in_index() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("tube_rows_plan").await.unwrap();
        db.query(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
            .await
            .unwrap()
            .check()
            .unwrap();
        let branches = [RefU64::from_two_nums(24384, 100), RefU64::from_two_nums(24384, 200)];
        let sql = super::durable_tube_rows_sql(&branches);
        let statements = sql.lines().collect::<Vec<_>>();
        assert_eq!(statements.len(), 2, "每根一句: {sql}");
        for (statement, branch) in statements.iter().zip(&branches) {
            assert!(
                statement.contains(&format!("WHERE in = {}", RefnoEnum::from(*branch).to_pe_key()))
                    && !statement.contains("anc CONTAINS")
                    && !statement.contains(" IN ["),
                "只按 in 一路等值: {statement}"
            );
            let explain = statement
                .trim_end_matches(';')
                .split(" ORDER BY ")
                .next()
                .unwrap()
                .to_string()
                + " EXPLAIN FULL;";
            let mut response = db.query(explain).await.unwrap().check().unwrap();
            let plan: surrealdb::Value = response.take(0).unwrap();
            let plan = plan.to_string();
            assert!(
                !plan.contains("Iterate Table") && plan.contains("idx_tubi_relate_in"),
                "{statement} must be planned through idx_tubi_relate_in, plan: {plan}"
            );
        }
    }

    /// 8022 `/test`（24384/26312）：生成后的 overlay 回执还在、记录已被删光时，
    /// `/model/records` 报 incomplete；第二次删除必须摘掉这张回执，而不是再回 `deleted`
    /// 却把残缺回执留着。修前这条在第二次删除之后仍然是 Err。
    #[tokio::test]
    async fn second_delete_of_overlay_root_24384_26312_does_not_report_incomplete() {
        use super::super::e3d_mesh_store::geometry_record_id;
        use super::super::model_memory_store::{
            packed_refno, ModelMemoryStore, ProjectionCommit, ProjectionScope, RootReceipt,
        };
        use crate::fast_model::e3d_model_service::delete_persisted_geometry_root;

        let root = RefNo::new(24384, 26312);
        let store = ModelMemoryStore::global();
        struct Guard(RefNo);
        impl Drop for Guard {
            fn drop(&mut self) {
                ModelMemoryStore::global().retire_deleted_root(ProjectionScope::Current, self.0);
            }
        }
        let _guard = Guard(root);
        store.retire_deleted_root(ProjectionScope::Current, root);

        let seed = |store: &ModelMemoryStore| {
            let upserts = (0..9u32)
                .map(|index| {
                    let source = RefNo::new(24384, 26313 + index);
                    let geometry_id = GeometryId::Element {
                        refno: format!("{}/{}", source.word0, source.word1),
                    };
                    let record_id = geometry_record_id(&geometry_id, source);
                    ProjectedGeometry {
                        record_id,
                        geometry_id,
                        source_refno: source,
                        dbnum: 8000,
                        source_sesno: 12,
                        noun: "EQUI".into(),
                        mesh_id: format!("mesh-{index}"),
                        world_transform: DMat4::IDENTITY,
                        world_bounds: [[0.0; 3], [1.0; 3]],
                        ancestors: vec![packed_refno(source), packed_refno(root)],
                        shape: ProjectedShape::Baked,
                        tube: None,
                    }
                })
                .collect::<Vec<_>>();
            let ids = upserts
                .iter()
                .map(|row| row.record_id.clone())
                .collect::<Vec<_>>();
            store.try_commit_current(vec![ProjectionCommit {
                upserts,
                receipt: Some((
                    root,
                    RootReceipt {
                        source_sesno: 12,
                        model_target: None,
                        manifest_hash: "24384/26312".into(),
                        geometry_count: 9,
                        published_via: "regen".into(),
                        room: None,
                    },
                )),
                ..Default::default()
            }]);
            ids
        };

        let ids = seed(store);
        let ready = super::overlay_records_of_root(store, root)
            .expect("complete overlay")
            .expect("receipt present");
        assert_eq!(ready.len(), 9);

        store.try_commit_current(vec![ProjectionCommit {
            removals: ids,
            ..Default::default()
        }]);
        let torn = super::overlay_records_of_root(store, root)
            .expect_err("torn overlay must still be rejected")
            .to_string();
        assert!(
            torn.contains("is incomplete")
                && torn.contains("expects 9")
                && torn.contains("store has 0"),
            "{torn}"
        );

        delete_persisted_geometry_root(root)
            .await
            .expect("second delete");
        assert!(
            super::overlay_records_of_root(store, root)
                .expect("healed overlay")
                .is_none(),
            "second delete must drop the leftover receipt"
        );

        seed(store);
        delete_persisted_geometry_root(root)
            .await
            .expect("first delete");
        assert!(
            super::overlay_records_of_root(store, root)
                .expect("delete of a complete root")
                .is_none()
        );
        delete_persisted_geometry_root(root)
            .await
            .expect("delete again");
        assert!(
            super::overlay_records_of_root(store, root)
                .expect("second delete of a clean root")
                .is_none()
        );
        assert!(store
            .records_of_root(ProjectionScope::Current, root)
            .is_empty());
    }
}
