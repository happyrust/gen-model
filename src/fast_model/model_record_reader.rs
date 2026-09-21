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
    let tubi_projection = format!(
        "id AS record_order, in AS refno, in.old_pe AS old_refno, {owner} AS owner, \
         'TUBI' AS generic, aabb.d AS world_aabb, world_trans.d AS world_trans, \
         false AS has_neg, \
         [{{geo_hash: record::id(out), is_tubi: true, is_invalid_tubi: invalid ?? false}}] AS insts"
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
}
