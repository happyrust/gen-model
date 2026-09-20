//! Select one complete root from the process overlay or the durable baseline.
//!
//! A resident receipt shadows RocksDB for the whole root. Durable reads are
//! returned directly and never hydrate kv-mem.
use aios_core::{GeomInstQuery, RefU64, RefnoEnum};
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
    if let Some((receipt, mut records)) = store.root_snapshot(ProjectionScope::Current, e3d_root) {
        anyhow::ensure!(
            records.len() == receipt.geometry_count,
            "memory overlay root {e3d_root} is incomplete: receipt expects {}, store has {}",
            receipt.geometry_count,
            records.len()
        );
        // Match the durable order: inst_relate first, then tubi_relate,
        // each ordered by record id. Eviction must not change cursor offsets.
        sort_overlay_records(&mut records);
        return Ok(records
            .into_iter()
            .map(|record| projection_record_to_query(record, e3d_root))
            .collect());
    }
    durable_records_of_root(db, root).await
}

fn sort_overlay_records(records: &mut [ProjectedGeometry]) {
    records.sort_by(|a, b| (a.tube.is_some(), &a.record_id).cmp(&(b.tube.is_some(), &b.record_id)));
}

/// 冷根的**房间视图**（ADR-073 D13，d-149 2026-09-20 改口）：整库房间重算对**不在 overlay 里**的根
/// 把库里的几何行读回来、装成 [`ProjectedGeometry`] 交给 `MemoryRoomCalculator`。与 [`records_of_root`]
/// 的冷读同一条 SQL——读的就是读路服务给 Plant UI 的那批行。
///
/// 只有房间计算读的那几格是忠实的：`source_refno` / `noun` / `mesh_id` / `world_bounds` / `world_transform`
/// （标准图元的本地摆放已折进世界变换，形状一律记 `Baked`——`room_model::memory_model_world_points`
/// 对两种形状算出的世界点相同）。库行不带的东西这里也没有：`record_id` 是 `<构件>@durable#<序>`
/// （只进错误信息）、`ancestors` 只有构件自身与这根、`source_sesno` 为 0、`tube` 为 `None`（管身那一行
/// 的 noun 仍是 `TUBI`）。**别把它当成投影记录装回 overlay**——它是算房间用的一次性输入，不是那根的
/// 模型。没有网格的行（不该有：写路对每一行都落 `booled_id` 或一条 `geo_relate`）跳过，不编一个空
/// `mesh_id` 让读网格那一步报错。
pub(crate) async fn durable_room_records_of_root(
    db: &Surreal<Any>,
    root: e3d_io::refno::RefNo,
) -> anyhow::Result<Vec<ProjectedGeometry>> {
    let key = RefnoEnum::from(RefU64::from_two_nums(root.word0, root.word1));
    let rows = durable_records_of_root(db, key).await?;
    Ok(rows
        .into_iter()
        .enumerate()
        .filter_map(|(ordinal, row)| room_record_from_durable(row, root, ordinal))
        .collect())
}

fn room_record_from_durable(
    row: GeomInstQuery,
    root: e3d_io::refno::RefNo,
    ordinal: usize,
) -> Option<ProjectedGeometry> {
    let inst = row.insts.into_iter().next()?;
    let source = crate::data_interface::generation_root::refno_to_e3d(row.refno);
    let ancestors = if source == root {
        vec![super::model_memory_store::packed_refno(root)]
    } else {
        vec![
            super::model_memory_store::packed_refno(source),
            super::model_memory_store::packed_refno(root),
        ]
    };
    Some(ProjectedGeometry {
        record_id: format!("{source}@durable#{ordinal}"),
        geometry_id: e3d_model::elmodl::GeometryId::Element {
            refno: source.to_string(),
        },
        source_refno: source,
        dbnum: root.dbno(),
        source_sesno: 0,
        noun: row.generic,
        mesh_id: inst.geo_hash,
        world_transform: dmat4_of(&row.world_trans) * dmat4_of(&inst.transform),
        world_bounds: [
            [
                row.world_aabb.mins.x,
                row.world_aabb.mins.y,
                row.world_aabb.mins.z,
            ],
            [
                row.world_aabb.maxs.x,
                row.world_aabb.maxs.y,
                row.world_aabb.maxs.z,
            ],
        ],
        ancestors,
        shape: ProjectedShape::Baked,
        tube: None,
    })
}

fn dmat4_of(transform: &bevy_transform::components::Transform) -> glam::DMat4 {
    glam::DMat4::from_scale_rotation_translation(
        transform.scale.as_dvec3(),
        transform.rotation.as_dquat(),
        transform.translation.as_dvec3(),
    )
}

async fn durable_records_of_root(
    db: &Surreal<Any>,
    root: RefnoEnum,
) -> anyhow::Result<Vec<GeomInstQuery>> {
    let packed = root.refno().0;
    let owner = root.to_pe_key();
    let mut response = db
        .query(format!(
            r#"
        SELECT id AS record_order, in AS refno, in.old_pe AS old_refno, {owner} AS owner,
          generic, aabb.d AS world_aabb, world_trans.d AS world_trans,
          out.ptset.d.pt AS pts, booled_id != NONE AS has_neg,
          IF booled_id != NONE {{ [{{geo_hash: booled_id}}] }}
          ELSE {{ insts_flat ?? (SELECT transform ?? trans.d AS transform,
              record::id(out) AS geo_hash FROM out->geo_relate
              WHERE visible AND out.meshed AND geo_type = 'Pos'
                AND (transform != NONE OR trans.d != NONE)) }} AS insts
        FROM inst_relate
        WHERE (anc CONTAINS {packed} OR in = {owner})
          AND aabb.d != NONE AND world_trans.d != NONE ORDER BY record_order;
        SELECT id AS record_order, in AS refno, in.old_pe AS old_refno, {owner} AS owner,
          'TUBI' AS generic, aabb.d AS world_aabb, world_trans.d AS world_trans,
          false AS has_neg,
          [{{geo_hash: record::id(out), is_tubi: true,
             is_invalid_tubi: invalid ?? false}}] AS insts
        FROM tubi_relate
        WHERE (anc CONTAINS {packed} OR in = {owner})
          AND aabb.d != NONE AND world_trans.d != NONE ORDER BY record_order;
    "#
        ))
        .await?
        .check()?;
    let mut records: Vec<GeomInstQuery> = response.take(0)?;
    records.extend(response.take::<Vec<GeomInstQuery>>(1)?);
    Ok(records)
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
        "#)).await.unwrap().check().unwrap();
        let records = super::durable_records_of_root(&db, root).await.unwrap();
        assert_eq!(records.len(), 3);
        assert!(records.iter().all(|r| r.owner == root));
        let canonical = records.iter().find(|r| r.generic == "BOX").unwrap();
        assert_eq!(canonical.insts[0].transform.translation.x, 5.0);
        assert_eq!(canonical.world_trans.translation.x, 120.0);
        let tube = records.iter().find(|r| r.generic == "TUBI").unwrap();
        assert!(!canonical.has_neg);
        assert!(!tube.has_neg);
        assert!(tube.insts[0].is_tubi && tube.insts[0].is_invalid_tubi);
        let baked = records.iter().find(|r| r.generic == "FLOOR").unwrap();
        assert!(baked.has_neg);
        assert_eq!(baked.insts[0].geo_hash, "hole");
    }

    /// d-149 / ADR-073 D13：冷根的房间视图从同一条冷读 SQL 装出来——只有房间计算读的那几格是忠实的
    /// （构件、noun、网格、世界盒、折进本地摆放的世界变换），别的根的行不进来，形状一律 `Baked`。
    #[tokio::test]
    async fn the_durable_room_view_keeps_only_what_the_room_calculator_reads() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("cold_room_view").await.unwrap();
        let root = RefNo::new(24384, 26606);
        let packed = super::super::model_memory_store::packed_refno(root);
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
        "#)).await.unwrap().check().unwrap();

        let records = super::durable_room_records_of_root(&db, root)
            .await
            .unwrap();
        assert_eq!(records.len(), 3, "别的根的那一行不进来");
        assert!(records.iter().all(|r| r.dbnum == root.dbno()
            && r.shape == ProjectedShape::Baked
            && r.tube.is_none()
            && r.source_sesno == 0
            && r.ancestors
                == vec![
                    super::super::model_memory_store::packed_refno(r.source_refno),
                    packed
                ]));

        let baked = records.iter().find(|r| r.noun == "FLOOR").unwrap();
        assert_eq!(baked.source_refno, RefNo::new(24384, 26609));
        assert_eq!(baked.mesh_id, "hole");
        assert_eq!(baked.world_bounds, [[0.0; 3], [1.0; 3]]);
        assert_eq!(
            baked.world_transform.w_axis.x, 120.0,
            "没有本地摆放：世界变换就是行上那份"
        );

        let canonical = records.iter().find(|r| r.noun == "BOX").unwrap();
        assert_eq!(canonical.mesh_id, "box");
        let (scale, _, translation) = canonical.world_transform.to_scale_rotation_translation();
        assert_eq!(translation.x, 125.0, "本地摆放 (+5) 折进世界变换 (+120)");
        assert_eq!(scale, glam::DVec3::new(2.0, 3.0, 4.0));

        let tube = records.iter().find(|r| r.noun == "TUBI").unwrap();
        assert_eq!(tube.mesh_id, "tube");
        assert_eq!(tube.source_refno, RefNo::new(24384, 26611));
        assert!(
            tube.record_id.starts_with("24384/26611@durable#"),
            "{}",
            tube.record_id
        );
    }
}
