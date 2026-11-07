//! 删除投影的现行契约（计划 `.planning/2026-11-08-projection-store-root-only` Phase 0）：统一删除入口之前先把
//! 现状钉住，入口接手之后这些用例照样要成立。带 `#[ignore]` 的是已经复现、留给后面阶段修的缺口。

use e3d_io::refno::RefNo;
use e3d_model::elmodl::GeometryId;
use glam::DMat4;

use super::fast_delete::{ModelFaceGenerationBusy, wipe_dbnum_model_face};
use super::mesh_purge::ClearGeometry;
use crate::fast_model::e3d_mesh_store::geometry_record_id;
use crate::fast_model::e3d_model_service::{delete_persisted_geometry_root, lock_db_generation};
use crate::fast_model::model_memory_store::{
    ModelMemoryStore, ProjectedGeometry, ProjectedShape, ProjectionCommit, ProjectionScope,
    RootReceipt, packed_refno,
};
use crate::fast_model::model_record_reader::overlay_records_under_node;

/// 投影是进程级单例，别的用例也在用：测完把这几根从里面摘掉。
struct RetireOnDrop(Vec<RefNo>);

impl Drop for RetireOnDrop {
    fn drop(&mut self) {
        for root in &self.0 {
            ModelMemoryStore::global().retire_deleted_root(ProjectionScope::Current, *root);
        }
    }
}

/// 一条元素记录，祖先链 = 自己 + `ancestors`（由近到远）。
fn record(source: RefNo, ancestors: &[RefNo]) -> ProjectedGeometry {
    let geometry_id = GeometryId::Element {
        refno: format!("{}/{}", source.word0, source.word1),
    };
    ProjectedGeometry {
        record_id: geometry_record_id(&geometry_id, source),
        geometry_id,
        source_refno: source,
        dbnum: 8000,
        source_sesno: 12,
        noun: "EQUI".into(),
        mesh_id: format!("mesh-{}", source.word1),
        world_transform: DMat4::IDENTITY,
        world_bounds: [[0.0; 3], [1.0; 3]],
        ancestors: std::iter::once(source)
            .chain(ancestors.iter().copied())
            .map(packed_refno)
            .collect(),
        shape: ProjectedShape::Baked,
        tube: None,
    }
}

fn receipt(geometry_count: usize, manifest_hash: &str) -> RootReceipt {
    RootReceipt {
        source_sesno: 12,
        model_target: None,
        manifest_hash: manifest_hash.into(),
        geometry_count,
        published_via: "regen".into(),
        room: None,
    }
}

/// S1：嵌套根靠祖先链共享记录，外层根的回执把内层根的记录也数了进去。投影那条路（`delete_projected_subtree`）
/// 删内层根时把共享记录的外层根一起作废；摄入那条路（`delete_model_subtree` →
/// `delete_persisted_geometry_root`）只退役被删的根，外层根的回执还按删前的条数记着，`/model/records`
/// 读外层根就报 incomplete。
#[tokio::test]
#[ignore = "S1：摄入路删内层根不作废外层根的回执，Phase 2 修"]
async fn deleting_an_inner_root_keeps_the_outer_root_readable() {
    let outer = RefNo::new(24384, 30500);
    let inner = RefNo::new(24384, 30501);
    let store = ModelMemoryStore::global();
    let _cleanup = RetireOnDrop(vec![inner, outer]);
    for root in [inner, outer] {
        store.retire_deleted_root(ProjectionScope::Current, root);
    }

    let own = (0..3u32)
        .map(|index| record(RefNo::new(24384, 30510 + index), &[outer]))
        .collect::<Vec<_>>();
    let nested = (0..4u32)
        .map(|index| record(RefNo::new(24384, 30520 + index), &[inner, outer]))
        .collect::<Vec<_>>();
    store.try_commit_current(vec![
        ProjectionCommit {
            upserts: nested.clone(),
            receipt: Some((inner, receipt(4, "inner"))),
            ..Default::default()
        },
        ProjectionCommit {
            upserts: own.into_iter().chain(nested).collect(),
            receipt: Some((outer, receipt(7, "outer"))),
            ..Default::default()
        },
    ]);
    assert_eq!(
        overlay_records_under_node(store, outer, outer)
            .expect("outer root before the delete")
            .len(),
        7
    );
    assert_eq!(
        overlay_records_under_node(store, inner, inner)
            .expect("inner root before the delete")
            .len(),
        4
    );

    delete_persisted_geometry_root(inner)
        .await
        .expect("ingest delete of the inner root");

    assert!(
        overlay_records_under_node(store, inner, inner)
            .expect("the deleted root is retired")
            .is_empty()
    );
    if let Err(error) = overlay_records_under_node(store, outer, outer) {
        panic!("删完内层根，外层根读不了：{error:#}");
    }
}

/// 整库模型清理碰上同库在飞的生成，在拿生成锁那一步就回 busy：后面的收键、删行都没走到。
/// dbnum 取一个没有别的用例会生成的库，按住它的生成锁不会卡住别人。
#[tokio::test]
async fn a_model_face_wipe_is_refused_while_the_dbnum_is_generating() {
    let dbnum = 6917;
    let _generation = lock_db_generation(dbnum).await;
    let error = wipe_dbnum_model_face(dbnum, ClearGeometry::Keep)
        .await
        .expect_err("a generation holds the dbnum");
    assert_eq!(
        error
            .downcast_ref::<ModelFaceGenerationBusy>()
            .map(|busy| busy.dbnum),
        Some(dbnum),
        "{error:#}"
    );
}
