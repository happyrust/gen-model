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

#[derive(Clone, Debug)]
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

    /// 本窗口已经决定要从空间树上摘掉、但要等提交后才真摘的那些 refno。
    pub async fn deferred_spatial_removals(&self) -> HashSet<aios_core::RefnoEnum> {
        self.spatial.lock().await.remove.clone()
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

    async fn finalize_plan(
        &self,
    ) -> Option<crate::data_interface::model_update_plan::ModelUpdatePlan> {
        self.finalize
            .lock()
            .await
            .as_ref()
            .map(|state| state.plan.clone())
    }

    async fn settle_plan_items(
        &self,
        succeeded: &std::collections::BTreeSet<(
            crate::data_interface::model_update_plan::ModelWorkAction,
            String,
        )>,
    ) {
        if let Some(finalize) = self.finalize.lock().await.as_mut() {
            finalize
                .plan
                .work_items
                .retain(|item| !succeeded.contains(&(item.action, item.target_refno.clone())));
        }
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

    /// 持有这个生成根的锁直到窗口析构；同一个根重复调用是无操作（ADR-017 I8）。
    ///
    /// 整段在 `root_locks` 里做完——**登记与到手必须是同一步**。拆成「先登记、放开、
    /// 再去拿」的话，集合里有这个根只说明有人开始拿、而不是拿到了：并发的第二个调用者
    /// 看见它就直接返回，于是在锁还没到手时就认为锁在手，照样去改这个根的模型产物。
    /// 今天三个调用点都是顺序 for 循环、碰不到这个交错，但窗口本来就备着
    /// `spawn_with_staged_io` 这种并发子任务机制，这条不变量不该靠调用点的写法来保证。
    ///
    /// 跨 await 攥着 `root_locks` 不会自锁：它只有这一个使用者，而第二个调用者在这里
    /// 排队正是想要的语义。
    pub async fn hold_generation_root(&self, root_refno: &str) {
        let mut held = self.root_locks.lock().await;
        if held.roots.contains(root_refno) {
            return;
        }
        let guard = crate::data_interface::manual_update::generation_root_lock(root_refno)
            .lock_owned()
            .await;
        held.roots.insert(root_refno.to_string());
        held.guards.push(guard);
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

pub(crate) async fn active_staged_finalize_plan()
-> Option<crate::data_interface::model_update_plan::ModelUpdatePlan> {
    active_staging_writes()?.finalize_plan().await
}

pub(crate) async fn settle_staged_plan_items(
    succeeded: &std::collections::BTreeSet<(
        crate::data_interface::model_update_plan::ModelWorkAction,
        String,
    )>,
) -> bool {
    let Some(context) = active_staging_writes() else {
        return false;
    };
    context.settle_plan_items(succeeded).await;
    true
}

/// 当前窗口里已被删除、但空间树上还留着旧包围盒的构件（窗口外为空集）。
///
/// 摘树推迟到提交之后（[`StagingWriteContext::defer_spatial_remove`]），所以窗口内
/// 任何**从树上取候选**的计算都必须自己把这批排除掉，否则会拿它们的旧位置继续算。
pub(crate) async fn staged_spatial_removals() -> HashSet<aios_core::RefnoEnum> {
    match active_staging_writes() {
        Some(context) => context.deferred_spatial_removals().await,
        None => HashSet::new(),
    }
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

    /// 第二个调用者必须等到锁**真的到手**才返回，而不是看见「有人登记过」就走。
    ///
    /// 登记与到手若不是同一步，下面这次 `hold` 会在锁还捏在别人手里时立刻返回，调用方
    /// 于是以为自己持有这个根、照样去改它的模型产物（ADR-017 I8 名存实亡）。这里先在
    /// 窗口外把根锁占住，逼出那个交错：新实现下第二个调用者只能继续排队。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_second_hold_waits_instead_of_assuming_the_lock_is_already_ours() {
        use crate::data_interface::manual_update::generation_root_lock;
        use std::time::Duration;

        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7993, 1, 1, ResourceThresholds::default())
            .await
            .expect("create window");
        let context = window.write_context();
        let root = "16777216/97531";

        // 窗口外的持有者：模拟按需生成正攥着这个根。
        let outsider = generation_root_lock(root).lock_owned().await;

        let racer = context.clone();
        let first = tokio::spawn(async move { racer.hold_generation_root(root).await });
        // 让第一个调用者走到它的 await 上。
        tokio::time::sleep(Duration::from_millis(50)).await;

        let second = tokio::time::timeout(
            Duration::from_millis(200),
            context.hold_generation_root(root),
        )
        .await;
        assert!(
            second.is_err(),
            "锁还在窗口外的持有者手里，第二个调用者不许提前返回"
        );

        drop(outsider);
        first.await.expect("join first hold");
        assert!(
            generation_root_lock(root).try_lock().is_err(),
            "第一个调用者返回之后，这个根必须确实被窗口持有"
        );

        drop(context);
        window.drop_database().await.expect("cleanup");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn finalize_state_is_registered_without_entering_the_journal() {
        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7994, 4, 9, ResourceThresholds::default())
            .await
            .expect("create window");

        window
            .scope(async {
                let plan = crate::data_interface::model_update_plan::ModelUpdatePlan {
                    work_items: vec![
                        crate::data_interface::model_update_plan::ModelWorkItem {
                            dbnum: 7994,
                            db_type: "DESI".into(),
                            source_end_sesno: 9,
                            action: crate::data_interface::model_update_plan::ModelWorkAction::Transform,
                            target_refno: "4000000001/1".into(),
                            noun: "EQUI".into(),
                        },
                        crate::data_interface::model_update_plan::ModelWorkItem {
                            dbnum: 7994,
                            db_type: "DESI".into(),
                            source_end_sesno: 9,
                            action: crate::data_interface::model_update_plan::ModelWorkAction::CascadeExpand,
                            target_refno: "4000000001/2".into(),
                            noun: "SCOM".into(),
                        },
                    ],
                    ..Default::default()
                };
                register_staged_finalize(StagedFinalize {
                dbnum: 7994,
                end_sesno: 9,
                plan,
                window_statements: vec!["UPSERT datacenter_version:x SET ok = true;".into()],
                cache_refnos: Vec::new(),
                })
                .await
                .expect("register finalize");
                assert_eq!(active_staged_finalize_plan().await.unwrap().work_items.len(), 2);
                settle_staged_plan_items(&std::collections::BTreeSet::from([(
                    crate::data_interface::model_update_plan::ModelWorkAction::Transform,
                    "4000000001/1".into(),
                )]))
                .await
            })
            .await
            .then_some(())
            .expect("inside staged context");

        assert!(window.journal().await.is_empty());
        let finalize = window.staged_finalize().await.expect("finalize remains");
        assert_eq!(finalize.plan.work_items.len(), 1);
        assert_eq!(
            finalize.plan.work_items[0].action,
            crate::data_interface::model_update_plan::ModelWorkAction::CascadeExpand
        );
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
