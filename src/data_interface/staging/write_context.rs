//! 模型生成写路由：task-local 上下文在场时写活动窗口，否则沿用持久层路径。

use std::future::Future;
use std::collections::HashSet;
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
}

#[derive(Clone, Default)]
pub(crate) struct DeferredSpatialMutations {
    pub refresh: HashSet<aios_core::RefnoEnum>,
    pub remove: HashSet<aios_core::RefnoEnum>,
}

#[derive(Clone)]
pub(crate) struct StagedFinalize {
    pub dbnum: u32,
    pub end_sesno: i32,
    pub plan: crate::data_interface::model_update_plan::ModelUpdatePlan,
    pub window_statements: Vec<String>,
    pub cache_refnos: Vec<aios_core::RefnoEnum>,
}

impl StagingWriteContext {
    pub(super) fn new(
        executor: Arc<Mutex<StagedExecutor>>,
        spatial: Arc<Mutex<DeferredSpatialMutations>>,
        finalize: Arc<Mutex<Option<StagedFinalize>>>,
    ) -> Self {
        Self {
            executor,
            spatial,
            finalize,
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
        let window = create_window_on(
            &instance,
            7995,
            1,
            1,
            ResourceThresholds::default(),
        )
        .await
        .expect("create window");

        with_staging_writes(window.write_context(), async {
            let a = spawn_with_staged_io(async {
                crate::surreal_retry::execute_model_write(
                    "UPSERT pe:a SET noun = 'PIPE'",
                    "test a",
                )
                .await
            });
            let b = spawn_with_staged_io(async {
                crate::surreal_retry::execute_model_write(
                    "UPSERT pe:b SET noun = 'EQUI'",
                    "test b",
                )
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
        let window = create_window_on(
            &instance,
            7994,
            4,
            9,
            ResourceThresholds::default(),
        )
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
}
