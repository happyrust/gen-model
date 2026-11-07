//! 投影删除的入口（`.planning/2026-11-08-projection-store-root-only`）。按子树删走 [`remove_subtree`]：
//! 「只写内存」与读透删请求的子树、墓碑进 spool 或不留；摄入形态按覆盖请求的生成根整根删、墓碑
//! 直接写库。整库清理（`fast_delete`）的投影那一半走 [`clear_dbnum`]。
//!
//! [`remove_subtree`] 的锁序在这里定死：先对覆盖范围的生成根逐个 try-lock（排序去重；被占回「正在后台
//! 生成」、一行不删），再按库号升序拿生成锁，拿齐之后才结算受影响的根、落墓碑、发删除。根锁必须
//! 先拿：同库每一次生成全程都攥着库级锁，反过来会排队等完同库在飞的生成，再把别人刚发布的根清掉。
//! [`clear_dbnum`] 不拿锁：整库清理各入口的锁序与「收键 → 删库行 → 删几何」都在 `fast_delete` 里。
//!
//! 墓碑先落、投影后改：墓碑落不下就整趟中止，投影一条没动；落下之后进程死了，重启后库里也没有这片产物。
//! 反过来先改投影，墓碑失败时投影说删了、库里还在，重启又回来。

use aios_core::RefnoEnum;
use e3d_io::RefNo;
use tokio::sync::OwnedMutexGuard;

use super::generation_root::refno_to_e3d;
use super::on_demand_model::try_generation_root;
use crate::fast_model::model_memory_store::{
    ModelMemoryStore, ProjectedGeometry, ProjectionCommit, ProjectionScope, packed_refno,
};

/// 删掉的投影要不要、往哪里留墓碑。留的都是同一份渲染（`render_spooled_scope_deletion`，每个子树根一笔事务）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TombstoneSink {
    /// 进 mirror spool，由 durable 镜像那条线落到库里（读透形态的持久面）。
    Spool,
    /// 直接写进权威库（摄入形态）。
    Database,
    /// 没有持久面（「只写内存」的库）。
    None,
}

#[derive(Debug, Clone)]
pub(crate) struct RemovalRequest {
    /// 删这几个节点的子树，投影记录按祖先链取：投影两路是请求节点，摄入形态是覆盖请求的生成根。
    pub subtrees: Vec<RefnoEnum>,
    /// 覆盖这片范围的生成根（`to_pdms_str` 形态）；为空就对 `subtrees` 自己拿根锁。
    pub lock_roots: Vec<String>,
    pub tombstone: TombstoneSink,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RemovalReport {
    pub removed_records: usize,
    /// 整根落在删除范围里的根：退役，回执、记录、点集、包围盒、提交序号一起摘。
    pub retired_roots: Vec<RefNo>,
    /// 删除范围头上、与被删记录共有记录的根：只少了一部分，回执作废，等下一次整根重新发布。
    pub invalidated_roots: Vec<RefNo>,
}

/// 删完的结算，连同删除时拿的锁：这个值活着，根锁与生成锁就都还攥着。摄入形态要在同一把锁下删完
/// `inst_relate`、摘完账本再放——中途放锁，同根的新一次生成刚写下的行与账本会被这些收尾删掉；投影两路拿到就放。
#[derive(Debug)]
pub(crate) struct Removal {
    pub report: RemovalReport,
    _generation_guards: Vec<OwnedMutexGuard<()>>,
    _root_guards: Vec<OwnedMutexGuard<()>>,
}

/// 存储的删除方法（[`ModelMemoryStore::retire_roots`]、[`ModelMemoryStore::clear_dbnum`]）要的凭证。字段私有，
/// 只有本模块造得出来：入口之外调不动这两个方法。
#[derive(Debug)]
pub(crate) struct RemovalAuthority(());

#[cfg(test)]
impl RemovalAuthority {
    /// 存储自己的单测直接调删除方法时用。
    pub(crate) fn for_tests() -> Self {
        Self(())
    }
}

/// 删掉 `subtrees` 子树下已发布的产物：记录按祖先链取、隐式管身所在的分支一并重置；整根落在里面的根退役，
/// 头上与之共有记录的根回执作废，让下一次生成必须整根重新发布。
pub(crate) async fn remove_subtree(request: RemovalRequest) -> anyhow::Result<Removal> {
    let mut lock_roots = request.lock_roots;
    if lock_roots.is_empty() {
        lock_roots.extend(request.subtrees.iter().map(RefnoEnum::to_pdms_str));
    }
    lock_roots.sort_unstable();
    lock_roots.dedup();
    let root_guards = lock_roots
        .iter()
        .map(|root| try_generation_root(root))
        .collect::<Result<Vec<_>, _>>()?;

    let mut nodes = request
        .subtrees
        .into_iter()
        .map(refno_to_e3d)
        .collect::<Vec<_>>();
    nodes.sort_unstable_by_key(|node| packed_refno(*node));
    nodes.dedup();
    let mut dbnums = nodes.iter().map(|node| node.dbno()).collect::<Vec<_>>();
    dbnums.sort_unstable();
    dbnums.dedup();
    let mut generation_guards = Vec::with_capacity(dbnums.len());
    for dbnum in dbnums {
        generation_guards
            .push(crate::fast_model::e3d_model_service::lock_db_generation(dbnum).await);
    }

    let store = ModelMemoryStore::global();
    let scope = ProjectionScope::Current;
    let closure = store.subtree_closure(scope, &nodes);
    let mut affected_roots = closure.removed_roots.clone();
    affected_roots.extend(closure.outer_roots.iter().copied());
    match request.tombstone {
        TombstoneSink::Spool => {
            for node in &nodes {
                let records = records_under(&closure.records, *node).collect::<Vec<_>>();
                persist_spooled_tombstone(store, *node, &records, &affected_roots)?;
            }
        }
        TombstoneSink::Database => {
            write_database_tombstone(&nodes, &closure.records, &affected_roots).await?;
        }
        TombstoneSink::None => {}
    }

    let commit = ProjectionCommit {
        tube_branch_resets: tube_branches(closure.records.iter()),
        removals: closure
            .records
            .iter()
            .map(|record| record.record_id.clone())
            .collect(),
        ..Default::default()
    };
    // 投影里本来就没有这片就不发：写后台停收之后连空提交也回绝。
    if !commit.is_empty() {
        crate::fast_model::model_write_behind::publish(vec![commit])?;
    }
    store.retire_roots(
        &RemovalAuthority(()),
        scope,
        &closure.removed_roots,
        &closure.outer_roots,
    );
    Ok(Removal {
        report: RemovalReport {
            removed_records: closure.records.len(),
            retired_roots: closure.removed_roots,
            invalidated_roots: closure.outer_roots,
        },
        _generation_guards: generation_guards,
        _root_guards: root_guards,
    })
}

/// 整库清理的投影那一半：`spool_tombstones`（读透形态）时先把这个库在投影里的每个根渲成 spool 墓碑，
/// 落不下就整趟中止、投影不动；再把这个库从投影里整个摘掉。库行、几何和锁都归调用方。
pub(crate) fn clear_dbnum(dbnum: u32, spool_tombstones: bool) -> anyhow::Result<()> {
    let store = ModelMemoryStore::global();
    if spool_tombstones {
        spool_dbnum_tombstones(store, dbnum)?;
    }
    store.clear_dbnum(&RemovalAuthority(()), dbnum);
    Ok(())
}

/// `node` 子树下的那几条：几个子树根各落各的墓碑。
fn records_under(
    records: &[ProjectedGeometry],
    node: RefNo,
) -> impl Iterator<Item = &ProjectedGeometry> {
    let packed = packed_refno(node);
    records
        .iter()
        .filter(move |record| record.ancestors.contains(&packed))
}

/// 隐式管身所在的分支（容器 BRAN）：整段重置，与按 id 摘记录一起发。
fn tube_branches<'a>(records: impl Iterator<Item = &'a ProjectedGeometry>) -> Vec<RefNo> {
    records
        .filter(|record| record.tube.is_some())
        .map(|record| record.source_refno)
        .collect()
}

fn persist_spooled_tombstone(
    store: &ModelMemoryStore,
    node: RefNo,
    records: &[&ProjectedGeometry],
    affected_roots: &[RefNo],
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
        &tube_branches(records.iter().copied()),
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

/// 摄入形态的墓碑：与 spool 同一份渲染，逐个子树根直接写进权威库。整个入口只有这里碰权威库。
async fn write_database_tombstone(
    nodes: &[RefNo],
    records: &[ProjectedGeometry],
    affected_roots: &[RefNo],
) -> anyhow::Result<()> {
    for node in nodes {
        let tombstone = crate::fast_model::model_db_adapter::render_spooled_scope_deletion(
            *node,
            affected_roots,
            &tube_branches(records_under(records, *node)),
        );
        aios_core::SUL_DB.query(tombstone).await?.check()?;
    }
    Ok(())
}

/// 投影里属于 `dbnum` 的根：回执的模型目标是这个库，或名下有这个库的记录。
fn projected_roots_of_dbnum(store: &ModelMemoryStore, dbnum: u32) -> Vec<RefNo> {
    let scope = ProjectionScope::Current;
    store
        .roots(scope)
        .into_iter()
        .filter(|root| {
            store.receipt(scope, *root).is_some_and(|receipt| {
                receipt
                    .model_target
                    .as_ref()
                    .is_some_and(|target| target.design.dbnum == dbnum)
            }) || store
                .records_of_root(scope, *root)
                .iter()
                .any(|record| record.dbnum == dbnum)
        })
        .collect()
}

/// 读透形态整库清理的墓碑：这个库在投影里的每个根一笔，只带根下属于这个库的记录与它们的会话。
fn spool_dbnum_tombstones(store: &ModelMemoryStore, dbnum: u32) -> anyhow::Result<()> {
    let scope = ProjectionScope::Current;
    let roots = projected_roots_of_dbnum(store, dbnum);
    if roots.is_empty() {
        return Ok(());
    }
    let option = aios_core::get_db_option();
    let spool = super::mirror_spool::MirrorSpool::for_process(
        &option.surreal_ns,
        &option.project_name,
        &option.mdb_name,
    )?;
    for root in roots {
        let records = store
            .records_of_root(scope, root)
            .into_iter()
            .filter(|record| record.dbnum == dbnum)
            .collect::<Vec<_>>();
        let transaction = crate::fast_model::model_db_adapter::render_spooled_scope_deletion(
            root,
            &[root],
            &tube_branches(records.iter()),
        );
        let sessions = records
            .iter()
            .map(|record| (record.dbnum, record.source_sesno))
            .collect::<Vec<_>>();
        let source_sesno_epoch = sessions
            .iter()
            .map(|(_, sesno)| u64::from(*sesno))
            .max()
            .unwrap_or_default();
        let epoch = super::mdb_memory_store::get(&option.project_name, &option.mdb_name)
            .map(|snapshot| snapshot.epoch())
            .unwrap_or(source_sesno_epoch);
        spool.persist(
            epoch,
            sessions,
            format!("model-tombstone-dbnum:{dbnum}:{root}"),
            vec![transaction],
        )?;
    }
    Ok(())
}
