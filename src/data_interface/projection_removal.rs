//! 投影删除的入口（`.planning/2026-11-08-projection-store-root-only`）。
//!
//! 锁序在这里定死：先对覆盖范围的生成根逐个 try-lock（排序去重；被占回「正在后台生成」、一行不删），
//! 再按库排队拿生成锁，拿齐之后才结算受影响的根、落墓碑、发删除。根锁必须先拿：同库每一次生成
//! 全程都攅着库级锁，反过来会排队等完同库在飞的生成，再把别人刚发布的根清掉。

use aios_core::RefnoEnum;
use e3d_io::RefNo;

use super::generation_root::refno_to_e3d;
use super::on_demand_model::try_generation_root;
use crate::fast_model::model_memory_store::{
    ModelMemoryStore, ProjectedGeometry, ProjectionCommit, ProjectionScope,
};

/// 删掉的投影要不要、往哪里留墓碑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TombstoneSink {
    /// 同一笔删除渲成 SurrealDB 事务进 mirror spool，由 durable 镜像那条线落到库里（读透形态的持久面）。
    Spool,
    /// 没有持久面（「只写内存」的库）。
    None,
}

#[derive(Debug, Clone)]
pub(crate) struct RemovalRequest {
    /// 删这个节点的子树：投影记录按祖先链取。
    pub subtree: RefnoEnum,
    /// 覆盖这片子树的生成根（`to_pdms_str` 形态）；为空就对 `subtree` 自己拿根锁。
    pub lock_roots: Vec<String>,
    pub tombstone: TombstoneSink,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RemovalReport {
    pub removed_records: usize,
    /// 回执作废了的根：子树里的根，以及与被删记录有交集的外层根。
    pub invalidated_roots: Vec<RefNo>,
}

/// 只清**进程内投影**里 `subtree` 子树下已发布的产物：记录按祖先链取、隐式管身所在的分支一并重置、
/// 受影响的根（含与之共有记录的嵌套根）回执作废，让下一次生成必须整根重新发布。
pub(crate) async fn remove_subtree(request: RemovalRequest) -> anyhow::Result<RemovalReport> {
    let mut lock_roots = request.lock_roots;
    if lock_roots.is_empty() {
        lock_roots.push(request.subtree.to_pdms_str());
    }
    lock_roots.sort_unstable();
    lock_roots.dedup();
    let _root_guards = lock_roots
        .iter()
        .map(|root| try_generation_root(root))
        .collect::<Result<Vec<_>, _>>()?;

    let node = refno_to_e3d(request.subtree);
    let _generation_guard =
        crate::fast_model::e3d_model_service::lock_db_generation(node.dbno()).await;
    let store = ModelMemoryStore::global();
    let scope = ProjectionScope::Current;
    let closure = store.subtree_closure(scope, node);
    let records = closure.records;
    let mut affected_roots = closure.roots;
    if affected_roots.is_empty() {
        affected_roots.push(node);
    }
    let tube_branches = records
        .iter()
        .filter(|record| record.tube.is_some())
        .map(|record| record.source_refno)
        .collect::<Vec<_>>();
    if request.tombstone == TombstoneSink::Spool {
        persist_spooled_tombstone(store, node, &records, &affected_roots, &tube_branches)?;
    }

    crate::fast_model::model_write_behind::publish(vec![ProjectionCommit {
        tube_branch_resets: tube_branches,
        removals: records
            .iter()
            .map(|record| record.record_id.clone())
            .collect(),
        ..Default::default()
    }])?;
    for affected in &affected_roots {
        store.invalidate_receipt(scope, *affected);
    }
    Ok(RemovalReport {
        removed_records: records.len(),
        invalidated_roots: affected_roots,
    })
}

fn persist_spooled_tombstone(
    store: &ModelMemoryStore,
    node: RefNo,
    records: &[ProjectedGeometry],
    affected_roots: &[RefNo],
    tube_branches: &[RefNo],
) -> anyhow::Result<()> {
    let option = aios_core::get_db_option();
    let mut sessions = records
        .iter()
        .map(|record| (record.dbnum, record.source_sesno))
        .collect::<Vec<_>>();
    sessions.extend(affected_roots.iter().filter_map(|affected| {
        store
            .receipt(ProjectionScope::Current, *affected)
            .map(|receipt| (affected.dbno(), receipt.source_sesno))
    }));
    if sessions.is_empty() {
        sessions.extend(
            super::mdb_memory_store::get(&option.project_name, &option.mdb_name)
                .into_iter()
                .flat_map(|snapshot| snapshot.session_fingerprint())
                .map(|(dbnum, _, sesno)| (dbnum, sesno)),
        );
    }
    sessions.sort_unstable();
    sessions.dedup();
    let source_sesno_epoch = sessions
        .iter()
        .map(|(_, sesno)| u64::from(*sesno))
        .max()
        .unwrap_or_default();
    let epoch = super::mdb_memory_store::get(&option.project_name, &option.mdb_name)
        .map(|snapshot| snapshot.epoch())
        .unwrap_or(source_sesno_epoch);
    let transaction = crate::fast_model::model_db_adapter::render_spooled_scope_deletion(
        node,
        affected_roots,
        tube_branches,
    );
    super::mirror_spool::MirrorSpool::for_process(
        &option.surreal_ns,
        &option.project_name,
        &option.mdb_name,
    )?
    .persist(
        epoch,
        sessions,
        format!("model-tombstone:{node}"),
        vec![transaction],
    )?;
    Ok(())
}
