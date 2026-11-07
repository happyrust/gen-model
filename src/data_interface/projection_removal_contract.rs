//! 删除投影的契约（计划 `.planning/2026-11-08-projection-store-root-only`）：三条删法都经删除入口
//! `projection_removal::remove_subtree`，这里钉住它的锁序与结算口径。投影两路删请求节点、锁它头上的生成根；
//! 摄入形态按生成根整根删（墓碑写库要接库，这里不留墓碑）。

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
use super::projection_removal::{RemovalReport, RemovalRequest, TombstoneSink, remove_subtree};
use crate::fast_model::e3d_mesh_store::geometry_record_id;
use crate::fast_model::e3d_model_service::lock_db_generation;
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

/// 一根盖好回执的投影，名下每个 `sources` 一条记录；整根提交带一份（空的）点集。
fn commit_root(store: &ModelMemoryStore, root: RefNo, sources: &[RefNo]) {
    store.try_commit_current(vec![ProjectionCommit {
        upserts: sources
            .iter()
            .map(|source| record(*source, &[root]))
            .collect(),
        receipt: Some((root, receipt(sources.len(), "root"))),
        point_sets: Some(Vec::new()),
        ..Default::default()
    }]);
}

fn refno_enum(refno: RefNo) -> RefnoEnum {
    RefnoEnum::from(format!("{}/{}", refno.word0, refno.word1).as_str())
}

/// 摄入形态的请求：按生成根整根删，根锁就是这几根。
fn whole_roots(roots: &[RefNo]) -> RemovalRequest {
    RemovalRequest {
        subtrees: roots.iter().copied().map(refno_enum).collect(),
        lock_roots: Vec::new(),
        tombstone: TombstoneSink::None,
    }
}

/// S1：嵌套根靠祖先链共享记录，外层根的回执把内层根的记录也数了进去。删内层根时共享记录的外层根要一起
/// 结算：内层根退役，外层根回执作废、自己那几条照旧可显示。修前摄入那条路（`delete_persisted_geometry_root`）
/// 只退役被删的根，外层根的回执还按删前的条数记着，`/model/records` 读外层根就报 incomplete。
#[tokio::test]
async fn deleting_an_inner_root_keeps_the_outer_root_readable() {
    let outer = RefNo::new(24384, 30500);
    let inner = RefNo::new(24384, 30501);
    let store = ModelMemoryStore::global();
    let scope = ProjectionScope::Current;
    let _cleanup = RetireOnDrop(vec![inner, outer]);
    for root in [inner, outer] {
        store.retire_deleted_root(scope, root);
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

    let report = remove_subtree(whole_roots(&[inner]))
        .await
        .expect("ingest delete of the inner root")
        .report;
    assert_eq!(
        report,
        RemovalReport {
            removed_records: 4,
            retired_roots: vec![inner],
            invalidated_roots: vec![outer],
        }
    );

    assert!(
        overlay_records_under_node(store, inner, inner)
            .expect("the deleted root is retired")
            .is_empty()
    );
    if let Err(error) = overlay_records_under_node(store, outer, outer) {
        panic!("删完内层根，外层根读不了：{error:#}");
    }
    assert!(
        store.receipt(scope, outer).is_none(),
        "外层根回执作废，等下一次整根重新发布"
    );
    assert_eq!(
        store.records_of_root(scope, outer).len(),
        3,
        "外层根自己那几条照旧可显示"
    );
}

/// 8022 `/test`（24384/26312）：生成后的 overlay 回执还在、记录已被删光时，`/model/records` 报 incomplete；
/// 再删一次必须把这张回执摘掉，而不是回「删了」却把残缺回执留着。删一个完整的根、再删一次，都不留回执。
#[tokio::test]
async fn deleting_a_root_whose_records_are_already_gone_drops_its_receipt() {
    let root = RefNo::new(24384, 26312);
    let store = ModelMemoryStore::global();
    let scope = ProjectionScope::Current;
    let _cleanup = RetireOnDrop(vec![root]);
    store.retire_deleted_root(scope, root);
    let sources = (0..9u32)
        .map(|index| RefNo::new(24384, 26313 + index))
        .collect::<Vec<_>>();

    commit_root(store, root, &sources);
    assert_eq!(
        overlay_records_under_node(store, root, root)
            .expect("complete overlay")
            .len(),
        9
    );
    store.try_commit_current(vec![ProjectionCommit {
        removals: store
            .records_of_root(scope, root)
            .into_iter()
            .map(|record| record.record_id)
            .collect(),
        ..Default::default()
    }]);
    let torn = overlay_records_under_node(store, root, root)
        .expect_err("torn overlay must still be rejected")
        .to_string();
    assert!(
        torn.contains("is incomplete")
            && torn.contains("expects 9")
            && torn.contains("store has 0"),
        "{torn}"
    );

    let report = remove_subtree(whole_roots(&[root]))
        .await
        .expect("delete of the torn root")
        .report;
    assert_eq!(
        report,
        RemovalReport {
            removed_records: 0,
            retired_roots: vec![root],
            invalidated_roots: Vec::new(),
        }
    );
    assert!(store.receipt(scope, root).is_none(), "残缺回执要摘掉");
    assert!(
        overlay_records_under_node(store, root, root)
            .expect("healed overlay")
            .is_empty()
    );

    commit_root(store, root, &sources);
    for attempt in ["first delete", "delete again"] {
        remove_subtree(whole_roots(&[root])).await.expect(attempt);
        assert!(store.receipt(scope, root).is_none(), "{attempt}");
        assert!(store.records_of_root(scope, root).is_empty(), "{attempt}");
        assert!(
            overlay_records_under_node(store, root, root)
                .expect(attempt)
                .is_empty()
        );
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

/// 覆盖范围的根正在后台生成（根锁被占），删除入口立刻回「正在生成」、一行不删：删根里的一件、锁它头上的根
/// （投影两路），和按根整根删（摄入形态），都一样。同库的生成锁这时也被占着：入口要是先排队等它，就会卡住
/// 而不是回绝，等完再删会把别人刚发布的根清掉。
#[tokio::test]
async fn a_removal_is_refused_while_a_covered_root_is_generating() {
    let root = RefNo::new(6918, 200);
    let piece = RefNo::new(6918, 201);
    let store = ModelMemoryStore::global();
    let _cleanup = RetireOnDrop(vec![root]);
    commit_root(store, root, &[piece, RefNo::new(6918, 202)]);
    let root_lock = generation_root_lock(&refno_enum(root).to_pdms_str());
    let _generating = root_lock.lock().await;
    let _same_dbnum = lock_db_generation(root.dbno()).await;

    for request in [
        RemovalRequest {
            subtrees: vec![refno_enum(piece)],
            lock_roots: vec![refno_enum(root).to_pdms_str()],
            tombstone: TombstoneSink::None,
        },
        whole_roots(&[root]),
    ] {
        let error = tokio::time::timeout(Duration::from_secs(5), remove_subtree(request.clone()))
            .await
            .expect("根锁被占要立刻回绝，不排队等库级生成锁")
            .expect_err("the covered root is generating");
        assert!(
            error.downcast_ref::<ModelGenerationInProgress>().is_some(),
            "{request:?}: {error:#}"
        );
    }
    assert_eq!(
        store.records_of_root(ProjectionScope::Current, root).len(),
        2
    );
    assert!(store.receipt(ProjectionScope::Current, root).is_some());
}

/// 同库有生成在飞（攥着库级生成锁），删除入口排队等它放手再删，不绕过；等的时候投影一条没动。删完按位置
/// 结算：删根里的一件（投影两路的形状），头上的根只少这一件、回执作废；整根删（摄入形态的形状，也是投影两路
/// 删一个根），这根退役，作废标记、点集都不留。
#[tokio::test]
async fn a_removal_waits_for_a_generation_on_the_same_dbnum() {
    let root = RefNo::new(6919, 300);
    let piece = RefNo::new(6919, 301);
    let whole = RefNo::new(6919, 310);
    let store = ModelMemoryStore::global();
    let scope = ProjectionScope::Current;
    let _cleanup = RetireOnDrop(vec![root, whole]);
    commit_root(store, root, &[piece, RefNo::new(6919, 302)]);
    commit_root(
        store,
        whole,
        &[RefNo::new(6919, 311), RefNo::new(6919, 312)],
    );
    let resident =
        || store.records_of_root(scope, root).len() + store.records_of_root(scope, whole).len();

    for (request, expected) in [
        (
            RemovalRequest {
                subtrees: vec![refno_enum(piece)],
                lock_roots: vec![refno_enum(root).to_pdms_str()],
                tombstone: TombstoneSink::None,
            },
            RemovalReport {
                removed_records: 1,
                retired_roots: Vec::new(),
                invalidated_roots: vec![root],
            },
        ),
        (
            whole_roots(&[whole]),
            RemovalReport {
                removed_records: 2,
                retired_roots: vec![whole],
                invalidated_roots: Vec::new(),
            },
        ),
    ] {
        let generation = lock_db_generation(root.dbno()).await;
        let before = resident();
        let removal = tokio::spawn(remove_subtree(request));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!removal.is_finished(), "同库在生成，删除要排队等");
        assert_eq!(resident(), before);

        drop(generation);
        let report = removal
            .await
            .expect("removal task")
            .expect("removal after the generation")
            .report;
        assert_eq!(report, expected);
    }
    assert_eq!(
        store.records_of_root(scope, root).len(),
        1,
        "头上的根只少了那一件"
    );
    assert!(store.receipt(scope, root).is_none());
    assert!(
        store.roots(scope).contains(&root),
        "作废的根还在投影里，等下一次整根重新发布"
    );
    assert!(store.records_of_root(scope, whole).is_empty());
    assert!(
        !store.roots(scope).contains(&whole),
        "整根删掉的根退役，连作废标记都不留"
    );
    assert!(store.point_sets_of_root(scope, whole).is_none());
}

/// 删一棵子树要结算的根，按祖先链上的位置分两类：整根落在子树里的（子树根自己常驻时、子树里的嵌套根、
/// 已作废的根）退役，头上的外层根作废；只挂在旁支上的根和别处的空根都不在。存储在一把读锁里按祖先链
/// 反查，两类合起来必须与逐根扫记录一致；几棵子树一起删时记录去重，已经整根落在某棵子树里的根不再算外层。
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
        let closure = store.subtree_closure(scope, &[node]);
        assert_eq!(
            ids(closure.records),
            ids(store.records_of_root(scope, node)),
            "{node}"
        );
        let mut roots = closure.removed_roots;
        roots.extend(closure.outer_roots);
        roots.sort_unstable_by_key(|root| packed_refno(*root));
        assert_eq!(roots, per_root_scan(&store, node), "{node}");
    }

    let classified = |nodes: &[RefNo]| {
        let closure = store.subtree_closure(scope, nodes);
        (
            closure.records.len(),
            closure.removed_roots,
            closure.outer_roots,
        )
    };
    assert_eq!(classified(&[zone]), (3, vec![inner, stale], vec![site]));
    assert_eq!(
        classified(&[site]),
        (4, vec![site, inner, sibling, stale], vec![])
    );
    assert_eq!(classified(&[inner]), (1, vec![inner], vec![site]));
    assert_eq!(classified(&[empty]), (0, vec![empty], vec![]));
    assert_eq!(
        classified(&[zone, inner]),
        (3, vec![inner, stale], vec![site]),
        "记录去重"
    );
    assert_eq!(
        classified(&[inner, site]),
        (4, vec![site, inner, sibling, stale], vec![]),
        "整根落在里面的不再算外层"
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
