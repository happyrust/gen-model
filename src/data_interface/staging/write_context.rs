//! 模型生成写路由：task-local 上下文在场时写活动窗口，否则沿用持久层路径。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;

use tokio::sync::Mutex;

use super::executor::{ExecMode, StagedExecutor};

tokio::task_local! {
    static STAGING_WRITES: StagingWriteContext;
}

#[derive(Clone)]
pub(crate) struct StagingWriteContext {
    executor: Arc<Mutex<StagedExecutor>>,
    spatial: Arc<Mutex<DeferredSpatialMutations>>,
    finalize: Arc<Mutex<Option<StagedFinalize>>>,
    regen_settlements: Arc<Mutex<Vec<(String, u64)>>>,
    mysql_changes: Arc<Mutex<Option<BTreeMap<u32, Vec<pdms_io::io::EleOperationData>>>>>,
    root_locks: Arc<Mutex<HeldRootLocks>>,
}

#[derive(Clone, Default)]
pub(crate) struct DeferredSpatialMutations {
    pub refresh: HashSet<aios_core::RefnoEnum>,
    pub remove: HashSet<aios_core::RefnoEnum>,
    pub room_changes: HashMap<aios_core::RefnoEnum, String>,
}

#[derive(Clone)]
pub(crate) struct StagedFinalize {
    pub dbnum: u32,
    pub end_sesno: i32,
    pub plan: crate::data_interface::model_update_plan::ModelUpdatePlan,
    pub window_statements: Vec<String>,
    pub cache_refnos: Vec<aios_core::RefnoEnum>,
}

#[derive(Default)]
pub(crate) struct HeldRootLocks {
    roots: HashSet<String>,
    guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
}

impl StagingWriteContext {
    pub(super) fn new(
        executor: Arc<Mutex<StagedExecutor>>,
        spatial: Arc<Mutex<DeferredSpatialMutations>>,
        finalize: Arc<Mutex<Option<StagedFinalize>>>,
        regen_settlements: Arc<Mutex<Vec<(String, u64)>>>,
        mysql_changes: Arc<Mutex<Option<BTreeMap<u32, Vec<pdms_io::io::EleOperationData>>>>>,
        root_locks: Arc<Mutex<HeldRootLocks>>,
    ) -> Self {
        Self {
            executor,
            spatial,
            finalize,
            regen_settlements,
            mysql_changes,
            root_locks,
        }
    }

    pub async fn execute(&self, sql: impl Into<String>, mode: ExecMode) -> anyhow::Result<()> {
        self.executor.lock().await.execute(sql, mode).await
    }

    pub async fn execute_scoped_delete(&self, sql: impl Into<String>) -> anyhow::Result<()> {
        self.executor.lock().await.execute_scoped_delete(sql).await
    }

    pub async fn defer_spatial_refresh(&self, refnos: &[aios_core::RefnoEnum]) {
        let mut spatial = self.spatial.lock().await;
        for refno in refnos {
            spatial.remove.remove(refno);
            spatial.refresh.insert(*refno);
        }
    }

    pub async fn defer_spatial_remove(&self, refnos: &[aios_core::RefnoEnum]) {
        let mut spatial = self.spatial.lock().await;
        for refno in refnos {
            spatial.refresh.remove(refno);
            spatial.remove.insert(*refno);
        }
    }

    pub async fn defer_room_changes(
        &self,
        changes: &[crate::fast_model::occ_generate::AabbChange],
    ) {
        let mut spatial = self.spatial.lock().await;
        for change in changes {
            spatial
                .room_changes
                .insert(change.refno, change.noun.clone());
        }
    }

    pub async fn register_finalize(&self, finalize: StagedFinalize) -> anyhow::Result<()> {
        let mut slot = self.finalize.lock().await;
        if let Some(existing) = slot.as_ref()
            && existing.dbnum != finalize.dbnum
        {
            anyhow::bail!(
                "staging window already belongs to dbnum={}, cannot finalize dbnum={}",
                existing.dbnum,
                finalize.dbnum
            );
        }
        *slot = Some(finalize);
        Ok(())
    }

    pub async fn defer_regen_settlement(&self, root_refno: String, revision: u64) {
        self.regen_settlements
            .lock()
            .await
            .push((root_refno, revision));
    }

    pub async fn defer_mysql_changes(
        &self,
        changes: BTreeMap<u32, Vec<pdms_io::io::EleOperationData>>,
    ) {
        *self.mysql_changes.lock().await = Some(changes);
    }

    pub async fn hold_generation_root(&self, root_refno: &str) {
        {
            let mut held = self.root_locks.lock().await;
            if !held.roots.insert(root_refno.to_string()) {
                return;
            }
        }
        let guard = crate::data_interface::manual_update::generation_root_lock(root_refno)
            .lock_owned()
            .await;
        self.root_locks.lock().await.guards.push(guard);
    }
}

pub(crate) async fn with_staging_writes<F>(ctx: StagingWriteContext, future: F) -> F::Output
where
    F: Future,
{
    STAGING_WRITES.scope(ctx, future).await
}

pub(crate) fn active_staging_writes() -> Option<StagingWriteContext> {
    STAGING_WRITES.try_with(Clone::clone).ok()
}

pub(crate) async fn register_staged_finalize(finalize: StagedFinalize) -> anyhow::Result<bool> {
    let Some(context) = active_staging_writes() else {
        return Ok(false);
    };
    context.register_finalize(finalize).await?;
    Ok(true)
}

pub(crate) async fn defer_staged_regen_settlement(root_refno: String, revision: u64) -> bool {
    let Some(context) = active_staging_writes() else {
        return false;
    };
    context.defer_regen_settlement(root_refno, revision).await;
    true
}

pub(crate) async fn defer_staged_mysql_changes(
    changes: BTreeMap<u32, Vec<pdms_io::io::EleOperationData>>,
) -> bool {
    let Some(context) = active_staging_writes() else {
        return false;
    };
    context.defer_mysql_changes(changes).await;
    true
}

pub(crate) async fn hold_staged_generation_root(root_refno: &str) -> bool {
    let Some(context) = active_staging_writes() else {
        return false;
    };
    context.hold_generation_root(root_refno).await;
    true
}

/// 同时继承 rs-core 的读上下文与本仓的写上下文。
pub(crate) fn spawn_with_staged_io<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let writes = active_staging_writes();
    aios_core::staging::spawn_with_staging_reads(async move {
        match writes {
            Some(context) => with_staging_writes(context, future).await,
            None => future.await,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::staging::ResourceThresholds;
    use crate::data_interface::staging::lifecycle::create_window_on;
    use surrealdb::engine::any::connect;

    #[tokio::test(flavor = "multi_thread")]
    async fn spawned_model_writes_share_the_window_journal() {
        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7995, 1, 1, ResourceThresholds::default())
            .await
            .expect("create window");

        with_staging_writes(window.write_context(), async {
            let a = spawn_with_staged_io(async {
                crate::surreal_retry::execute_model_write("UPSERT pe:a SET noun = 'PIPE'", "test a")
                    .await
            });
            let b = spawn_with_staged_io(async {
                crate::surreal_retry::execute_model_write("UPSERT pe:b SET noun = 'EQUI'", "test b")
                    .await
            });
            a.await.expect("join a").expect("write a");
            b.await.expect("join b").expect("write b");
        })
        .await;

        assert_eq!(window.journal().await.len(), 2);
        let mut response = window
            .staging_db()
            .query("SELECT VALUE id FROM pe")
            .await
            .expect("query staged rows");
        let rows: surrealdb::Value = response.take(0).expect("take rows");
        let text = serde_json::to_string(&rows).expect("serialize");
        assert!(text.contains("a") && text.contains("b"), "{text}");

        window.drop_database().await.expect("cleanup");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn finalize_state_is_registered_without_entering_the_journal() {
        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7994, 4, 9, ResourceThresholds::default())
            .await
            .expect("create window");

        window
            .scope(register_staged_finalize(StagedFinalize {
                dbnum: 7994,
                end_sesno: 9,
                plan: Default::default(),
                window_statements: vec!["UPSERT datacenter_version:x SET ok = true;".into()],
                cache_refnos: Vec::new(),
            }))
            .await
            .expect("register finalize")
            .then_some(())
            .expect("inside staged context");

        assert!(window.journal().await.is_empty());
        let tail = window.render_finalize_tail().await.expect("render tail");
        assert!(tail.contains("datacenter_version:x"), "{tail}");
        assert!(tail.contains("dbnum_watermark:7994"), "{tail}");
        window.drop_database().await.expect("cleanup");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn staged_generation_lock_lives_until_the_window_ends() {
        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7991, 2, 3, ResourceThresholds::default())
            .await
            .expect("window");
        let root = "4000000001/8";
        let lock = crate::data_interface::manual_update::generation_root_lock(root);

        window.scope(hold_staged_generation_root(root)).await;
        assert!(
            lock.try_lock().is_err(),
            "commit boundary still owns the root"
        );
        window.drop_database().await.expect("cleanup");
        assert!(lock.try_lock().is_ok(), "window end releases the root");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mysql_changes_wait_for_window_commit_boundary() {
        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7990, 2, 3, ResourceThresholds::default())
            .await
            .expect("window");

        assert!(
            window
                .scope(defer_staged_mysql_changes(BTreeMap::new()))
                .await
        );
        assert!(window.take_deferred_mysql_changes().await.is_some());
        window.drop_database().await.expect("cleanup");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn changed_aabbs_are_kept_for_the_staged_room_round() {
        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7987, 2, 3, ResourceThresholds::default())
            .await
            .expect("window");
        let target = aios_core::RefnoEnum::from("4000000001/20");

        window
            .scope(async {
                active_staging_writes()
                    .expect("context")
                    .defer_room_changes(&[crate::fast_model::occ_generate::AabbChange {
                        refno: target,
                        noun: "EQUI".into(),
                    }])
                    .await;
            })
            .await;

        assert_eq!(
            window.deferred_spatial().await.room_changes.get(&target),
            Some(&"EQUI".to_string())
        );
        window.drop_database().await.expect("cleanup");
    }
}
