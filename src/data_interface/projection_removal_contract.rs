//! 删除投影的现行契约（计划 `.planning/2026-11-08-projection-store-root-only`）：统一删除入口之前先把
//! 现状钉住，入口接手之后这些用例照样要成立。带 `#[ignore]` 的是已经复现、留给后面阶段修的缺口。

use std::collections::HashSet;
use std::time::Duration;

use aios_core::RefnoEnum;
use e3d_io::refno::RefNo;
use e3d_model::elmodl::GeometryId;
use glam::DMat4;

use super::fast_delete::{ModelFaceGenerationBusy, wipe_dbnum_model_face};
use super::manual_update::generation_root_lock;
use super::mesh_purge::ClearGeometry;
use super::on_demand_model::ModelGenerationInProgress;
use super::projection_removal::{RemovalRequest, TombstoneSink, remove_subtree};
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
        dbnum: source.dbno(),
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

/// 一根盖好回执的投影，名下每个 `sources` 一条记录。
fn commit_root(store: &ModelMemoryStore, root: RefNo, sources: &[RefNo]) {
    store.try_commit_current(vec![ProjectionCommit {
        upserts: sources
            .iter()
            .map(|source| record(*source, &[root]))
            .collect(),
        receipt: Some((root, receipt(sources.len(), "root"))),
        ..Default::default()
    }]);
}

fn refno_enum(refno: RefNo) -> RefnoEnum {
    RefnoEnum::from(format!("{}/{}", refno.word0, refno.word1).as_str())
}

/// S1：嵌套根靠祖先链共享记录，外层根的回执把内层根的记录也数了进去。投影那条路（删除入口
/// `projection_removal::remove_subtree`）删内层根时把共享记录的外层根一起作废；摄入那条路
/// （`delete_model_subtree` → `delete_persisted_geometry_root`）只退役被删的根，外层根的回执还按删前的
/// 条数记着，`/model/records` 读外层根就报 incomplete。
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

/// 覆盖范围的根正在后台生成（根锁被占），删除入口立刻回「正在生成」、一行不删。同库的生成锁
/// 这时也被占着：入口要是先排队等它，就会卡住而不是回绝，等完再删会把别人刚发布的根清掉。
#[tokio::test]
async fn a_removal_is_refused_while_a_covered_root_is_generating() {
    let root = RefNo::new(6918, 200);
    let store = ModelMemoryStore::global();
    let _cleanup = RetireOnDrop(vec![root]);
    commit_root(store, root, &[RefNo::new(6918, 201), RefNo::new(6918, 202)]);
    let subtree = refno_enum(root);
    let root_lock = generation_root_lock(&subtree.to_pdms_str());
    let _generating = root_lock.lock().await;
    let _same_dbnum = lock_db_generation(root.dbno()).await;

    let error = tokio::time::timeout(
        Duration::from_secs(5),
        remove_subtree(RemovalRequest {
            subtree,
            lock_roots: Vec::new(),
            tombstone: TombstoneSink::None,
        }),
    )
    .await
    .expect("根锁被占要立刻回绝，不排队等库级生成锁")
    .expect_err("the covered root is generating");
    assert!(
        error.downcast_ref::<ModelGenerationInProgress>().is_some(),
        "{error:#}"
    );
    assert_eq!(
        store.records_of_root(ProjectionScope::Current, root).len(),
        2
    );
    assert!(store.receipt(ProjectionScope::Current, root).is_some());
}

/// 同库有生成在飞（攅着库级生成锁），删除入口排队等它放手再删，不绕过；等的时候投影一条没动。
#[tokio::test]
async fn a_removal_waits_for_a_generation_on_the_same_dbnum() {
    let root = RefNo::new(6919, 300);
    let store = ModelMemoryStore::global();
    let _cleanup = RetireOnDrop(vec![root]);
    commit_root(store, root, &[RefNo::new(6919, 301), RefNo::new(6919, 302)]);
    let generation = lock_db_generation(root.dbno()).await;

    let removal = tokio::spawn(remove_subtree(RemovalRequest {
        subtree: refno_enum(root),
        lock_roots: Vec::new(),
        tombstone: TombstoneSink::None,
    }));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!removal.is_finished(), "同库在生成，删除要排队等");
    assert_eq!(
        store.records_of_root(ProjectionScope::Current, root).len(),
        2
    );

    drop(generation);
    let report = removal
        .await
        .expect("removal task")
        .expect("removal after the generation");
    assert_eq!(report.removed_records, 2);
    assert_eq!(report.invalidated_roots, vec![root]);
    assert!(
        store
            .records_of_root(ProjectionScope::Current, root)
            .is_empty()
    );
    assert!(store.receipt(ProjectionScope::Current, root).is_none());
}

/// 删一棵子树要结算的根：外层根、子树里的嵌套根、已作废的根、子树根自己（常驻时）都在；只挂在
/// 旁支上的根和别处的空根不在。存储在一把读锁里按祖先链反查，结果必须与逐根扫记录一致。
#[test]
fn the_subtree_closure_matches_the_per_root_scan() {
    let store = ModelMemoryStore::default();
    let scope = ProjectionScope::Current;
    let site = RefNo::new(24384, 31000);
    let zone = RefNo::new(24384, 31001);
    let inner = RefNo::new(24384, 31002);
    let sibling = RefNo::new(24384, 31003);
    let empty = RefNo::new(24384, 31004);
    let stale = RefNo::new(24384, 31005);
    let in_zone = record(RefNo::new(24384, 31010), &[zone, site]);
    let in_inner = record(RefNo::new(24384, 31011), &[inner, zone, site]);
    let in_sibling = record(RefNo::new(24384, 31012), &[sibling, site]);
    let in_stale = record(RefNo::new(24384, 31013), &[stale, zone, site]);
    store.try_commit_current(vec![
        ProjectionCommit {
            upserts: vec![in_inner.clone()],
            receipt: Some((inner, receipt(1, "inner"))),
            ..Default::default()
        },
        ProjectionCommit {
            upserts: vec![in_sibling.clone()],
            receipt: Some((sibling, receipt(1, "sibling"))),
            ..Default::default()
        },
        ProjectionCommit {
            upserts: vec![in_stale.clone()],
            receipt: Some((stale, receipt(1, "stale"))),
            ..Default::default()
        },
        ProjectionCommit {
            receipt: Some((empty, receipt(0, "empty"))),
            ..Default::default()
        },
        ProjectionCommit {
            upserts: vec![in_zone, in_inner, in_sibling, in_stale],
            receipt: Some((site, receipt(4, "site"))),
            ..Default::default()
        },
    ]);
    store.invalidate_receipt(scope, stale);

    let ids = |records: Vec<ProjectedGeometry>| {
        records
            .into_iter()
            .map(|record| record.record_id)
            .collect::<Vec<_>>()
    };
    for node in [
        zone,
        site,
        inner,
        sibling,
        empty,
        stale,
        RefNo::new(24384, 31099),
    ] {
        let closure = store.subtree_closure(scope, node);
        assert_eq!(
            ids(closure.records),
            ids(store.records_of_root(scope, node)),
            "{node}"
        );
        assert_eq!(closure.roots, per_root_scan(&store, node), "{node}");
    }
    assert_eq!(
        store.subtree_closure(scope, zone).roots,
        vec![site, inner, stale]
    );
}

/// 入口接手之前投影删法的算法：逐个常驻根取记录，看有没有和子树共有的。
fn per_root_scan(store: &ModelMemoryStore, node: RefNo) -> Vec<RefNo> {
    let scope = ProjectionScope::Current;
    let ids = store
        .records_of_root(scope, node)
        .into_iter()
        .map(|record| record.record_id)
        .collect::<HashSet<_>>();
    store
        .roots(scope)
        .into_iter()
        .filter(|candidate| {
            *candidate == node
                || store
                    .records_of_root(scope, *candidate)
                    .iter()
                    .any(|record| ids.contains(&record.record_id))
        })
        .collect()
}
