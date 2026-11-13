//! 房间层级树 `tubes=1` 的直段来源（plant3d-web 方案 B；计划
//! `docs/plans/2026-09-22-room-tree-tube-segments-backend-dev-plan.md` §2.2 / §2.5）。
//!
//! 隐式管身不进空间树（ADR-072 D4，`aabb_tree::projected_tree_entry`），所以两条树路由折完管件之后按树里出现的
//! **BRAN 单元**读它根名下的管身记录：投影里有回执的根取 [`ModelMemoryStore::root_snapshot`]（与 `/model/records` 同一谓词
//! ——回执在就是本进程的现值），其余一次冷读 `tubi_relate`（[`durable_tube_rows_of_branches`]，只按 `in` 一路）。
//! 这里只负责「拿到 [`TubeRecord`]」；量距离 / 过滤 / 归房 / 挂单元在 `spatial_tree_view::TreeDraft::finish`。
//!
//! 身份是 `(BRAN, from, to, ordinal)` 四元组（D3）：内存路直接来自 `ProjectedTube`；冷读路 `route_ordinal` 要到 T2 才落盘，
//! 老行按同 `(from, to)` 内的出现序补号（与 e3d-model `slot_ordinals` 同一规则，行按分支槽位排）。

use std::collections::HashMap;

use aios_core::{RefU64, RefnoEnum};
use parry3d::bounding_volume::Aabb;
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::generation_root::{refno_from_e3d, refno_to_e3d};
use crate::fast_model::model_memory_store::{ModelMemoryStore, ProjectedGeometry, ProjectionScope};
use crate::fast_model::model_record_reader::{DurableTubeRow, durable_tube_rows_of_branches};

/// 一段直管在树里用到的全部：身份四元组 + 世界盒 + 长度 + 告警。
#[derive(Debug, Clone, PartialEq)]
pub struct TubeRecord {
    /// 容器 BRAN。
    pub bran: RefU64,
    /// 同 `(from, to)` 内的序号（`route_ordinal`）。
    pub ordinal: u32,
    /// 离开侧构件（容器头那一段是 BRAN 自己）。
    pub from: RefU64,
    /// 到达侧构件（容器尾那一段是 BRAN 自己）。
    pub to: RefU64,
    pub aabb: Aabb,
    /// 直管长度，mm：存下的世界变换 z 轴列的模——直管是标准圆柱 `z∈[0,1]` 摆到 `T(0,0,-L/2)·S(r,r,L)`
    /// （e3d-model `instantiate_centered_cylinder`），本地摆放已折进世界变换（`persisted_world_transform`）。
    pub length: f32,
    pub invalid: bool,
}

/// 直段身份键 `(BRAN, from, to, ordinal)`。
pub type TubeKey = (RefU64, RefU64, RefU64, u32);

impl TubeRecord {
    pub fn key(&self) -> TubeKey {
        (self.bran, self.from, self.to, self.ordinal)
    }

    /// 投影记录 → 直段；不是管身 / 盒不可用（NaN / Inf / 反向）回 `None`。
    pub fn from_projected(record: &ProjectedGeometry) -> Option<Self> {
        let tube = record.tube.as_ref()?;
        let aabb = usable_aabb(record.world_bounds[0], record.world_bounds[1])?;
        Some(Self {
            bran: refno_from_e3d(record.source_refno).refno(),
            ordinal: tube.route_ordinal,
            from: refno_from_e3d(tube.from).refno(),
            to: refno_from_e3d(tube.to).refno(),
            aabb,
            length: record.world_transform.z_axis.truncate().length() as f32,
            invalid: tube.invalid,
        })
    }
}

fn usable_aabb(mins: [f32; 3], maxs: [f32; 3]) -> Option<Aabb> {
    let finite = mins.iter().chain(maxs.iter()).all(|v| v.is_finite());
    let ordered = (0..3).all(|axis| mins[axis] <= maxs[axis]);
    (finite && ordered).then(|| Aabb::new(mins.into(), maxs.into()))
}

/// 冷读行 → 直段。`tube_ordinal` 没落盘的按同 `(BRAN, from, to)` 内的出现序补 0、1、2…（行已按槽位排）；
/// refno 解析不出 / 盒不可用的行丢弃。
pub fn from_durable_rows(rows: &[DurableTubeRow]) -> Vec<TubeRecord> {
    let mut seen: HashMap<(RefU64, RefU64, RefU64), u32> = HashMap::new();
    rows.iter()
        .filter_map(|row| {
            let parse = |text: &str| text.parse::<RefU64>().ok().filter(RefU64::is_valid);
            let bran = parse(&row.container)?;
            let from = parse(&row.tube_from)?;
            let to = parse(&row.tube_to)?;
            let aabb = usable_aabb(row.world_aabb.mins.into(), row.world_aabb.maxs.into())?;
            let slot = seen.entry((bran, from, to)).or_insert(0);
            let ordinal = row.tube_ordinal.unwrap_or(*slot);
            *slot += 1;
            Some(TubeRecord {
                bran,
                ordinal,
                from,
                to,
                aabb,
                length: row.world_trans.scale.z.abs(),
                invalid: row.invalid,
            })
        })
        .collect()
}

/// 按 BRAN 取直段：投影里有回执的根取投影，其余一次冷读；每个点到的根在返回表里都有一格（没有直段就是空）。
/// 同一根的直段按身份键排，两条路答案同序。
pub async fn load_tube_records_from(
    store: &ModelMemoryStore,
    db: &Surreal<Any>,
    roots: &[RefU64],
) -> anyhow::Result<HashMap<RefU64, Vec<TubeRecord>>> {
    let mut by_root: HashMap<RefU64, Vec<TubeRecord>> =
        roots.iter().map(|root| (*root, Vec::new())).collect();
    let mut cold = Vec::new();
    for root in roots {
        match store.root_snapshot(
            ProjectionScope::Current,
            refno_to_e3d(RefnoEnum::from(*root)),
        ) {
            Some((_, records)) => {
                for record in records.iter().filter_map(TubeRecord::from_projected) {
                    by_root.entry(record.bran).or_default().push(record);
                }
            }
            None => cold.push(*root),
        }
    }
    if !cold.is_empty() {
        let rows = durable_tube_rows_of_branches(db, &cold).await?;
        for record in from_durable_rows(&rows) {
            by_root.entry(record.bran).or_default().push(record);
        }
    }
    for records in by_root.values_mut() {
        records.sort_by_key(TubeRecord::key);
    }
    Ok(by_root)
}

/// 生产入口：全局投影 + `SUL_DB`。
pub async fn load_tube_records(
    roots: &[RefU64],
) -> anyhow::Result<HashMap<RefU64, Vec<TubeRecord>>> {
    load_tube_records_from(ModelMemoryStore::global(), &aios_core::SUL_DB, roots).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::model_memory_store::{
        ProjectedShape, ProjectedTube, ProjectionCommit, RootReceipt, packed_refno,
    };
    use e3d_io::refno::RefNo;
    use e3d_model::elmodl::GeometryId;
    use glam::{DMat4, DQuat, DVec3};

    /// 本文件的用例独占 `4000000017/*` 这一组 refno。
    fn refno(seq: u32) -> RefU64 {
        RefU64::from_two_nums(4_000_000_017, seq)
    }

    fn e3d(seq: u32) -> RefNo {
        refno_to_e3d(RefnoEnum::from(refno(seq)))
    }

    /// 挂在 BRAN `bran` 上的一段直管：`from → to`，长 `length`，绕 z 转 0.3 rad 再平移——z 轴列的模仍是长度。
    fn tube(
        bran: u32,
        from: u32,
        to: u32,
        ordinal: u32,
        length: f64,
        invalid: bool,
    ) -> ProjectedGeometry {
        let world = DMat4::from_rotation_translation(
            DQuat::from_rotation_z(0.3),
            DVec3::new(10.0, 20.0, 30.0),
        );
        let local = DMat4::from_translation(DVec3::new(0.0, 0.0, -length / 2.0))
            * DMat4::from_scale(DVec3::new(50.0, 50.0, length));
        ProjectedGeometry {
            record_id: format!("tube-{bran}-{from}-{to}-{ordinal}"),
            geometry_id: GeometryId::ImpliedTube {
                container_refno: RefnoEnum::from(refno(bran)).to_pdms_str(),
                route_ordinal: ordinal,
                from_refno: RefnoEnum::from(refno(from)).to_pdms_str(),
                to_refno: RefnoEnum::from(refno(to)).to_pdms_str(),
            },
            source_refno: e3d(bran),
            dbnum: 4_000_000_017,
            source_sesno: 1,
            noun: "TUBI".into(),
            mesh_id: "cylinder".into(),
            world_transform: world * local,
            world_bounds: [[0.0, 0.0, 0.0], [length as f32, 100.0, 100.0]],
            ancestors: vec![packed_refno(e3d(bran))],
            shape: ProjectedShape::Baked,
            tube: Some(ProjectedTube {
                route_ordinal: ordinal,
                from: e3d(from),
                to: e3d(to),
                invalid,
            }),
        }
    }

    fn element(seq: u32, bran: u32) -> ProjectedGeometry {
        let mut record = tube(bran, seq, seq, 0, 1.0, false);
        record.record_id = format!("element-{seq}");
        record.geometry_id = GeometryId::Element {
            refno: RefnoEnum::from(refno(seq)).to_pdms_str(),
        };
        record.source_refno = e3d(seq);
        record.noun = "ELBO".into();
        record.tube = None;
        record
    }

    #[test]
    fn a_projected_tube_becomes_a_record_with_its_length_in_the_z_column() {
        let record = TubeRecord::from_projected(&tube(10, 11, 12, 3, 1234.5, true)).expect("tube");
        assert_eq!(record.key(), (refno(10), refno(11), refno(12), 3));
        assert!((record.length - 1234.5).abs() < 1e-2, "{}", record.length);
        assert!(record.invalid);
        assert_eq!(record.aabb.maxs.x, 1234.5);

        assert_eq!(
            TubeRecord::from_projected(&element(11, 10)),
            None,
            "构件不是直段"
        );
        let mut bad = tube(10, 11, 12, 0, 10.0, false);
        bad.world_bounds = [[5.0, 0.0, 0.0], [1.0, 1.0, 1.0]];
        assert_eq!(TubeRecord::from_projected(&bad), None, "反向盒丢弃");
        bad.world_bounds = [[f32::NAN, 0.0, 0.0], [1.0, 1.0, 1.0]];
        assert_eq!(TubeRecord::from_projected(&bad), None, "NaN 盒丢弃");
    }

    fn row(
        container: &str,
        from: &str,
        to: &str,
        ordinal: Option<u32>,
        length: f32,
    ) -> DurableTubeRow {
        DurableTubeRow {
            container: container.into(),
            tube_from: from.into(),
            tube_to: to.into(),
            tube_ordinal: ordinal,
            world_aabb: Aabb::new([0.0; 3].into(), [length, 1.0, 1.0].into()),
            world_trans: bevy_transform::components::Transform {
                translation: glam::Vec3::ZERO,
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::new(5.0, 5.0, length),
            },
            invalid: false,
        }
    }

    /// 补号只在同 `(BRAN, from, to)` 内数；落了盘的 `route_ordinal` 原样用；解析不出的行丢。
    #[test]
    fn durable_rows_are_numbered_within_the_same_end_pair_only() {
        let a = refno(10).to_string();
        let (m1, m2, m3) = (
            refno(11).to_string(),
            refno(12).to_string(),
            refno(13).to_string(),
        );
        let records = from_durable_rows(&[
            row(&a, &m1, &m2, None, 100.0),
            row(&a, &m2, &m3, None, 200.0),
            row(&a, &m1, &m2, None, 300.0),
            row(&a, &m3, &a, Some(7), 400.0),
            row("nonsense", &m1, &m2, None, 1.0),
            row(&a, &m1, "1_x", None, 1.0),
        ]);
        assert_eq!(
            records
                .iter()
                .map(|r| (r.from, r.to, r.ordinal, r.length))
                .collect::<Vec<_>>(),
            [
                (refno(11), refno(12), 0, 100.0),
                (refno(12), refno(13), 0, 200.0),
                (refno(11), refno(12), 1, 300.0),
                (refno(13), refno(10), 7, 400.0),
            ]
        );
        assert!(records.iter().all(|r| r.bran == refno(10)));
    }

    /// 有回执的根走投影、没有的走冷读，一次调用两路都走；每个点到的根都有一格。
    #[tokio::test]
    async fn resident_roots_read_the_projection_and_the_rest_are_read_cold_once() {
        let store = ModelMemoryStore::default();
        let receipt = RootReceipt {
            source_sesno: 1,
            model_target: None,
            manifest_hash: "m".into(),
            geometry_count: 3,
            published_via: "regen".into(),
            room: None,
        };
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![
                    element(11, 10),
                    tube(10, 11, 12, 0, 500.0, false),
                    tube(10, 12, 10, 0, 700.0, true),
                ],
                receipt: Some((e3d(10), receipt)),
                ..Default::default()
            }],
        );

        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("tube_records").await.unwrap();
        let cold_bran = refno(20);
        db.query(format!(
            r#"
          CREATE aabb:c SET d={{mins:[0,0,0],maxs:[900,10,10]}};
          CREATE trans:c SET d={{translation:[0,0,0],rotation:[0,0,0,1],scale:[5,5,900]}};
          INSERT RELATION INTO tubi_relate [
            {{id:tubi_relate:[pe:⟨{cold}⟩,0],in:pe:⟨{cold}⟩,out:inst_geo:m,leave:pe:⟨{cold}⟩,arrive:pe:⟨{m}⟩,
             aabb:aabb:c,world_trans:trans:c,anc:[1],dbnum:1}},
            {{id:tubi_relate:[pe:⟨{resident}⟩,0],in:pe:⟨{resident}⟩,out:inst_geo:m,leave:pe:⟨{resident}⟩,arrive:pe:⟨{m}⟩,
             aabb:aabb:c,world_trans:trans:c,anc:[1],dbnum:1}}
          ];
        "#,
            cold = cold_bran,
            resident = refno(10),
            m = refno(21),
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        let roots = [refno(10), cold_bran, refno(30)];
        let by_root = load_tube_records_from(&store, &db, &roots).await.unwrap();
        assert_eq!(by_root.len(), 3, "点到的根都有一格");
        assert_eq!(
            by_root[&refno(10)]
                .iter()
                .map(|r| (r.from, r.to, r.length.round() as i32, r.invalid))
                .collect::<Vec<_>>(),
            [
                (refno(11), refno(12), 500, false),
                (refno(12), refno(10), 700, true)
            ],
            "有回执的根只看投影——库里那条挂在它名下的行不来"
        );
        assert_eq!(
            by_root[&cold_bran]
                .iter()
                .map(|r| (r.bran, r.from, r.to, r.ordinal, r.length))
                .collect::<Vec<_>>(),
            [(cold_bran, cold_bran, refno(21), 0, 900.0)],
            "没回执的根冷读"
        );
        assert!(by_root[&refno(30)].is_empty(), "哪儿都没有的根是空格");
    }
}
