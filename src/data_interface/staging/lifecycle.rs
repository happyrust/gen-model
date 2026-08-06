//! staging database 生命周期（ADR-017 §2 / 开发方案 T0.3）。
//!
//! 命名 `staging_{dbnum}_{window_id}`：window_id 进程内单调分配；窗口的逻辑
//! 会话区间只记在元数据里，冻结吸收扩窗**不改名**——名字是暂存身份，区间是
//! 暂存内容，两者解耦（Oracle 审核 A4 的采纳结论）。
//!
//! 生命周期：建库（初始化表定义与 fn::，与生产启动序列同一套）→ 窗口执行
//! （跨重试保留，重试只重跑失败根）→ 提交 / 废弃后 DROP。进程内登记表记录
//! 在册窗口，窗口终态清扫兜底孤儿库残留（DROP 失败、代码路径遗漏等）。
//!
//! 共享句柄纪律：ADR §2 是「一个常驻 mem 实例、每提交单元一个 database」。
//! `Surreal<Any>` 的克隆共享同一会话，`use_db` 是会话级切换——所以**窗口每次
//! (重)进入执行前必须 [`ActiveStagedWindow::activate`]**。数据批次队列是单 worker
//! （ADR-011），任一时刻至多一个窗口在执行，阻断窗口的暂存库只是驻留、不被
//! 查询，这条纪律因此够用。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, bail};
use once_cell::sync::Lazy;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use super::executor::{ExecMode, JournalEntry, StagedExecutor};
use super::resources::{ResourceBand, ResourceGauge, ResourceThresholds};
use super::write_context::{
    DeferredSpatialMutations, HeldRootLocks, StagedFinalize, StagingWriteContext,
};

/// 暂存实例上所有 staging database 所在的 namespace。
pub const STAGING_NS: &str = "staging";

static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct RegisteredWindow {
    meta: StagingWindowMeta,
    gauge: Arc<ResourceGauge>,
    writeback_stalled: Option<String>,
}

/// 进程内登记表：label → 窗口元数据与资源面板。终态清扫以它裁定孤儿。
static REGISTRY: Lazy<Mutex<BTreeMap<String, RegisteredWindow>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));

/// 常驻暂存实例的一次性连接守卫。
static STAGE_INSTANCE_READY: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// 确保进程常驻的 `STAGE_DB`（`mem://`）已连接。幂等，可在任意入口调用。
pub async fn ensure_stage_instance() -> anyhow::Result<()> {
    STAGE_INSTANCE_READY
        .get_or_try_init(|| async {
            aios_core::staging::STAGE_DB
                .connect("mem://")
                .await
                .context("连接常驻暂存实例（mem://）失败")?;
            Ok::<(), anyhow::Error>(())
        })
        .await?;
    Ok(())
}

/// 一个提交单元的窗口元数据（登记表内容）。
#[derive(Clone, Debug)]
pub struct StagingWindowMeta {
    pub dbnum: u32,
    pub window_id: u64,
    pub label: String,
    /// 逻辑会话区间（含吸收扩窗后的最新值）。
    pub start_sesno: i32,
    pub end_sesno: i32,
}

/// 一个提交单元的暂存窗口：staging database + 资源面板 + 元数据。
pub struct ActiveStagedWindow {
    meta: StagingWindowMeta,
    db: Surreal<Any>,
    gauge: Arc<ResourceGauge>,
    executor: Arc<tokio::sync::Mutex<StagedExecutor>>,
    spatial: Arc<tokio::sync::Mutex<DeferredSpatialMutations>>,
    finalize: Arc<tokio::sync::Mutex<Option<StagedFinalize>>>,
    regen_settlements: Arc<tokio::sync::Mutex<Vec<(String, u64)>>>,
    mysql_changes: Arc<
        tokio::sync::Mutex<
            Option<std::collections::BTreeMap<u32, Vec<pdms_io::io::EleOperationData>>>,
        >,
    >,
    root_locks: Arc<tokio::sync::Mutex<HeldRootLocks>>,
}

/// 在生产常驻实例上开一个新窗口。
pub async fn create_window(
    dbnum: u32,
    start_sesno: i32,
    end_sesno: i32,
) -> anyhow::Result<ActiveStagedWindow> {
    ensure_stage_instance().await?;
    create_window_on(
        &aios_core::staging::STAGE_DB,
        dbnum,
        start_sesno,
        end_sesno,
        ResourceThresholds::default(),
    )
    .await
}

/// 在显式实例上开窗口（测试与一致性套件用同一条生产路径）。
pub async fn create_window_on(
    instance: &Surreal<Any>,
    dbnum: u32,
    start_sesno: i32,
    end_sesno: i32,
    thresholds: ResourceThresholds,
) -> anyhow::Result<ActiveStagedWindow> {
    let window_id = NEXT_WINDOW_ID.fetch_add(1, Ordering::SeqCst);
    let label = format!("staging_{dbnum}_{window_id}");

    instance
        .use_ns(STAGING_NS)
        .use_db(&label)
        .await
        .with_context(|| format!("切换到暂存库 {label} 失败"))?;
    init_staging_schema(instance)
        .await
        .with_context(|| format!("初始化暂存库 {label} 失败"))?;

    let meta = StagingWindowMeta {
        dbnum,
        window_id,
        label: label.clone(),
        start_sesno,
        end_sesno,
    };
    let gauge = ResourceGauge::new(thresholds);
    REGISTRY.lock().expect("registry lock").insert(
        label,
        RegisteredWindow {
            meta: meta.clone(),
            gauge: gauge.clone(),
            writeback_stalled: None,
        },
    );

    let executor = Arc::new(tokio::sync::Mutex::new(
        StagedExecutor::new(instance.clone(), meta.label.clone()).with_gauge(gauge.clone()),
    ));
    let spatial = Arc::new(tokio::sync::Mutex::new(DeferredSpatialMutations::default()));
    let finalize = Arc::new(tokio::sync::Mutex::new(None));
    let regen_settlements = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let mysql_changes = Arc::new(tokio::sync::Mutex::new(None));
    let root_locks = Arc::new(tokio::sync::Mutex::new(HeldRootLocks::default()));
    Ok(ActiveStagedWindow {
        meta,
        db: instance.clone(),
        gauge,
        executor,
        spatial,
        finalize,
        regen_settlements,
        mysql_changes,
        root_locks,
    })
}

/// 暂存库建库初始化：与生产启动序列（`run_cli` 的 schema 段）同一套 DEFINE。
/// 单一事实来源——mem↔fork 一致性套件排练的就是这个函数。
///
/// 注意继承的两个既有行为（见 `docs/2026-08-05_fork-surreal-compat-findings.md`）：
/// `define_common_functions` 静默吞逐语句错误（全新库上 REMOVE 不存在的函数）；
/// `idx_inst_relate_zone_refno` 的 `TYPE BTREE` 语法在 2.1.4 不合法、生产从未建成
/// （F1），这里 1:1 复刻吞错行为。
pub async fn init_staging_schema(db: &Surreal<Any>) -> anyhow::Result<()> {
    aios_core::function::define_common_functions_on(db).await?;
    // run_cli 的 D11 矫正：project_hd 下 hh 文件按目录序后加载覆盖了 hd 版，
    // 加载完成后重放 hd 版把覆盖再覆盖回来。
    #[cfg(feature = "project_hd")]
    {
        let hd = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resource/surreal/fn_query_room_code.surql");
        let text =
            std::fs::read_to_string(&hd).with_context(|| format!("读取 {} 失败", hd.display()))?;
        db.query(text).await?;
    }
    // 刻意不装 update_dbnum_event（F4，见 findings 文档）：该事件体假定 pe 的
    // record id 是数组（历史行形制），对字符串 id 的最新行（`pe:24381_100677`，
    // fork 解析为字符串）任何 UPSERT/UPDATE 都会因 `array::at` 类型错误而**整条
    // 语句失败**——窗口的解析写入在暂存库里会全军覆没。它服务的
    // `dbnum_info_table` 是遗留水位迁移的记账面（dbnum_state 本就容忍其缺失/
    // 陈旧），不属于窗口数据语义。生产 `run_cli` 对 SUL_DB 的定义不动，留待
    // F4 与业主对齐后统一处置。
    aios_core::create_geom_index_on(db).await?;
    aios_core::define_room_index_on(db).await?;
    aios_core::define_owner_index_on(db).await?;
    aios_core::define_fullname_index_on(db).await?;
    aios_core::define_pe_index_on(db).await?;
    aios_core::define_ses_index_on(db).await?;
    // gen-model 侧唯一的启动期 DEFINE（init_inst_relate_indices）——F1：语法不合法，
    // 生产以 `let _` 吞错、索引从未建成，这里保持一致。
    let _ = db
        .query("DEFINE INDEX idx_inst_relate_zone_refno ON TABLE inst_relate COLUMNS zone_refno TYPE BTREE;")
        .await;
    Ok(())
}

impl ActiveStagedWindow {
    pub fn meta(&self) -> &StagingWindowMeta {
        &self.meta
    }

    pub fn label(&self) -> &str {
        &self.meta.label
    }

    pub fn gauge(&self) -> &Arc<ResourceGauge> {
        &self.gauge
    }

    /// 窗口(重)进入执行前必须调用：把共享会话切回本窗口的 database。
    pub async fn activate(&self) -> anyhow::Result<()> {
        self.db
            .use_ns(STAGING_NS)
            .use_db(&self.meta.label)
            .await
            .with_context(|| format!("激活暂存库 {} 失败", self.meta.label))
    }

    /// 通过本窗口唯一的执行器写暂存/journal。
    pub async fn execute(&mut self, sql: impl Into<String>, mode: ExecMode) -> anyhow::Result<()> {
        self.executor.lock().await.execute(sql, mode).await
    }

    pub async fn journal(&self) -> Vec<JournalEntry> {
        self.executor.lock().await.journal().to_vec()
    }

    pub(crate) async fn commit_to(
        &self,
        target: &Surreal<Any>,
        tail_transaction: Option<&str>,
    ) -> anyhow::Result<()> {
        self.executor
            .lock()
            .await
            .commit_to(target, tail_transaction)
            .await
    }

    pub fn staging_db(&self) -> &Surreal<Any> {
        &self.db
    }

    pub(crate) fn write_context(&self) -> StagingWriteContext {
        StagingWriteContext::new(
            self.executor.clone(),
            self.spatial.clone(),
            self.finalize.clone(),
            self.regen_settlements.clone(),
            self.mysql_changes.clone(),
            self.root_locks.clone(),
        )
    }

    pub(crate) async fn staged_finalize(&self) -> Option<StagedFinalize> {
        self.finalize.lock().await.clone()
    }

    pub(crate) async fn settle_staged_plan_items(
        &self,
        succeeded: &std::collections::BTreeSet<(
            crate::data_interface::model_update_plan::ModelWorkAction,
            String,
        )>,
    ) {
        if succeeded.is_empty() {
            return;
        }
        if let Some(finalize) = self.finalize.lock().await.as_mut() {
            finalize.plan.work_items.retain(|item| {
                !succeeded.contains(&(item.action, item.target_refno.clone()))
            });
        }
    }

    pub(crate) async fn merge_room_recalc_changes(
        &self,
        changes: &std::collections::HashMap<aios_core::RefnoEnum, String>,
    ) {
        if changes.is_empty() {
            return;
        }
        if let Some(finalize) = self.finalize.lock().await.as_mut() {
            let dbnum = finalize.dbnum;
            let end_sesno = finalize.end_sesno;
            crate::data_interface::model_update_pending::merge_room_recalc_changes(
                &mut finalize.plan,
                dbnum,
                end_sesno,
                changes,
            );
        }
    }

    pub(crate) async fn deferred_regen_settlements(&self) -> Vec<(String, u64)> {
        self.regen_settlements.lock().await.clone()
    }

    pub(crate) async fn take_deferred_mysql_changes(
        &self,
    ) -> Option<std::collections::BTreeMap<u32, Vec<pdms_io::io::EleOperationData>>> {
        self.mysql_changes.lock().await.take()
    }

    pub(crate) async fn render_finalize_tail(&self) -> anyhow::Result<String> {
        let finalize = self
            .staged_finalize()
            .await
            .context("staged window has no registered finalize state")?;
        if finalize.dbnum != self.meta.dbnum || finalize.end_sesno != self.meta.end_sesno {
            bail!(
                "staged finalize range mismatch: window dbnum={} end={}, finalize dbnum={} end={}",
                self.meta.dbnum,
                self.meta.end_sesno,
                finalize.dbnum,
                finalize.end_sesno
            );
        }
        let spatial = self.deferred_spatial().await;
        let mut refresh = spatial
            .refresh
            .into_iter()
            .map(|refno| refno.to_pdms_str())
            .collect::<Vec<_>>();
        let mut remove = spatial
            .remove
            .into_iter()
            .map(|refno| refno.to_pdms_str())
            .collect::<Vec<_>>();
        refresh.sort_unstable();
        remove.sort_unstable();
        crate::data_interface::model_update_pending::render_finalize_tail_with_effects(
            finalize.dbnum,
            finalize.end_sesno,
            &finalize.plan,
            &finalize.window_statements,
            &refresh,
            &remove,
            &self.deferred_regen_settlements().await,
        )
    }

    /// Replay the journal, close the authoritative watermark transaction, then invalidate
    /// persistent caches. Nothing after `commit_to` may make the committed data disappear.
    pub(crate) async fn commit_registered_to(
        &self,
        target: &Surreal<Any>,
    ) -> anyhow::Result<StagedFinalize> {
        let finalize = self
            .staged_finalize()
            .await
            .context("staged window has no registered finalize state")?;
        let tail = self.render_finalize_tail().await?;
        self.commit_to(target, Some(&tail)).await?;
        if !finalize.cache_refnos.is_empty() {
            aios_core::clear_all_caches_batch(&finalize.cache_refnos).await;
        }
        Ok(finalize)
    }

    pub(crate) async fn take_deferred_spatial(&self) -> DeferredSpatialMutations {
        std::mem::take(&mut *self.spatial.lock().await)
    }

    pub(crate) async fn deferred_spatial(&self) -> DeferredSpatialMutations {
        self.spatial.lock().await.clone()
    }

    /// 在本窗口的读写上下文中运行生成调用树；子任务用 `spawn_with_staged_io`
    /// 继续继承两种上下文。
    pub(crate) async fn scope<F>(&self, future: F) -> F::Output
    where
        F: std::future::Future,
    {
        aios_core::staging::with_staging_reads(
            self.read_context(),
            super::with_staging_writes(self.write_context(), future),
        )
        .await
    }

    /// 本窗口的读路由上下文。
    pub fn read_context(&self) -> aios_core::staging::StagingReadContext {
        aios_core::staging::StagingReadContext::new(self.db.clone(), self.meta.label.clone())
    }

    /// 把窗口元数据上的逻辑上界对齐到实际 finalize 登记的 `end_sesno`。
    ///
    /// 建窗时用的是冻结重扫的 `file_latest_sesno`；解析出的应用窗口可能因会话空隙
    /// 更窄。写回前必须一致，否则 `commit_registered_to` 的范围校验会拒绝。
    pub fn align_end_sesno(&mut self, end_sesno: i32) {
        self.meta.end_sesno = end_sesno;
        if let Some(entry) = REGISTRY
            .lock()
            .expect("registry lock")
            .get_mut(&self.meta.label)
        {
            entry.meta.end_sesno = end_sesno;
        }
    }

    /// 写回重试耗尽后的进程内告警；窗口本体仍由 worker 持有，journal 不丢。
    pub(crate) fn mark_writeback_stalled(&self, error: &anyhow::Error) {
        if let Some(entry) = REGISTRY
            .lock()
            .expect("registry lock")
            .get_mut(&self.meta.label)
        {
            entry.writeback_stalled = Some(format!("{error:#}"));
        }
    }

    pub(crate) fn clear_writeback_stalled(&self) {
        if let Some(entry) = REGISTRY
            .lock()
            .expect("registry lock")
            .get_mut(&self.meta.label)
        {
            entry.writeback_stalled = None;
        }
    }

    /// 冻结吸收扩窗：只更新逻辑区间元数据，不改名、不换库。
    /// 资源状态机处于「拒绝吸收」及以上档位时拒绝——后继排队行保持独立窗口。
    pub fn absorb_extend(&mut self, new_end_sesno: i32) -> anyhow::Result<()> {
        let band = self.gauge.band();
        if band >= ResourceBand::RefuseAbsorb {
            bail!(
                "暂存 {} 资源档位 {band:?}（摄入 {} 字节），拒绝吸收扩窗——后继会话走独立窗口",
                self.meta.label,
                self.gauge.total_bytes()
            );
        }
        if new_end_sesno < self.meta.end_sesno {
            bail!(
                "吸收扩窗只能抬高上界：{} → {new_end_sesno}",
                self.meta.end_sesno
            );
        }
        self.meta.end_sesno = new_end_sesno;
        if let Some(entry) = REGISTRY
            .lock()
            .expect("registry lock")
            .get_mut(&self.meta.label)
        {
            entry.meta.end_sesno = new_end_sesno;
        }
        Ok(())
    }

    /// 终态清理（提交成功或废弃共用）：DROP 本窗口的 database 并出册。
    pub async fn drop_database(self) -> anyhow::Result<()> {
        if let Some(finalize) = self.staged_finalize().await
            && !finalize.cache_refnos.is_empty()
        {
            aios_core::clear_all_caches_batch(&finalize.cache_refnos).await;
        }
        let label = self.meta.label.clone();
        let result = self
            .db
            .query(format!("REMOVE DATABASE IF EXISTS `{label}`;"))
            .await
            .with_context(|| format!("DROP 暂存库 {label} 传输失败"))
            .and_then(|mut r| {
                r.check()
                    .map(|_| ())
                    .with_context(|| format!("DROP 暂存库 {label} 失败"))
            });
        // 无论 DROP 成败都出册：残留由窗口终态清扫按「不在册」兜底回收。
        REGISTRY.lock().expect("registry lock").remove(&label);
        result
    }
}

/// 登记表快照（观测 / 面板用）。
pub fn registered_windows() -> Vec<StagingWindowMeta> {
    REGISTRY
        .lock()
        .expect("registry lock")
        .values()
        .map(|entry| entry.meta.clone())
        .collect()
}

/// `/health` 使用的活动窗口资源快照。
pub fn resource_snapshots() -> Vec<serde_json::Value> {
    REGISTRY
        .lock()
        .expect("registry lock")
        .values()
        .map(|entry| {
            let snapshot = entry.gauge.snapshot();
            serde_json::json!({
                "dbnum": entry.meta.dbnum,
                "window_id": entry.meta.window_id,
                "label": entry.meta.label,
                "start_sesno": entry.meta.start_sesno,
                "end_sesno": entry.meta.end_sesno,
                "state": if entry.writeback_stalled.is_some() { "writeback_stalled" } else { "active" },
                "writeback_error": entry.writeback_stalled,
                "band": format!("{:?}", snapshot.band).to_lowercase(),
                "staged_sql_bytes": snapshot.staged_sql_bytes,
                "journal_bytes": snapshot.journal_bytes,
                "estimated_write_rows": snapshot.estimated_write_rows,
                "journal_entries": snapshot.journal_entries,
                "staged_statements": snapshot.staged_statements,
            })
        })
        .collect()
}

/// 窗口终态清扫：DROP 暂存实例上所有「不在册」的 staging database。
/// 返回被清掉的库名。生产在每个窗口终态后调用一次（便宜：INFO FOR NS 一次）。
pub async fn sweep_orphan_staging_databases_on(
    instance: &Surreal<Any>,
) -> anyhow::Result<Vec<String>> {
    instance
        .use_ns(STAGING_NS)
        .await
        .context("清扫切换 namespace 失败")?;
    let mut response = instance
        .query("INFO FOR NS")
        .await
        .context("INFO FOR NS 传输失败")?;
    let value: surrealdb::Value = response.take(0).context("INFO FOR NS 取值失败")?;
    let json = serde_json::to_value(&value).context("INFO FOR NS 序列化失败")?;

    let registered: std::collections::BTreeSet<String> = REGISTRY
        .lock()
        .expect("registry lock")
        .keys()
        .cloned()
        .collect();

    let mut dropped = Vec::new();
    if let Some(databases) = json
        .pointer("/Object/databases/Object")
        .and_then(|v| v.as_object())
    {
        for name in databases.keys() {
            if name.starts_with("staging_") && !registered.contains(name) {
                instance
                    .query(format!("REMOVE DATABASE IF EXISTS `{name}`;"))
                    .await
                    .with_context(|| format!("清扫孤儿暂存库 {name} 传输失败"))?
                    .check()
                    .with_context(|| format!("清扫孤儿暂存库 {name} 失败"))?;
                dropped.push(name.clone());
            }
        }
    }
    Ok(dropped)
}

/// 生产常驻实例上的终态清扫。
pub async fn sweep_orphan_staging_databases() -> anyhow::Result<Vec<String>> {
    ensure_stage_instance().await?;
    sweep_orphan_staging_databases_on(&aios_core::staging::STAGE_DB).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb::engine::any::connect;

    async fn own_instance() -> Surreal<Any> {
        connect("mem://").await.expect("mem boots")
    }

    /// 命名单调、初始化落地、登记表在册。
    #[tokio::test(flavor = "multi_thread")]
    async fn windows_get_monotonic_labels_and_initialized_schema() {
        let instance = own_instance().await;
        let first = create_window_on(&instance, 7901, 1, 10, ResourceThresholds::default())
            .await
            .expect("create first");
        let second = create_window_on(&instance, 7902, 5, 8, ResourceThresholds::default())
            .await
            .expect("create second");

        assert!(first.label().starts_with("staging_7901_"));
        assert!(second.label().starts_with("staging_7902_"));
        let first_id = first.meta().window_id;
        let second_id = second.meta().window_id;
        assert!(second_id > first_id, "window_id 必须进程内单调");

        // 初始化 = 生产启动序列：关键 fn:: 在第二个窗口的库里也在场。
        second.activate().await.expect("activate");
        let mut response = instance.query("INFO FOR DB").await.expect("info");
        let value: surrealdb::Value = response.take(0).expect("take");
        let info = serde_json::to_string(&value).expect("serialize");
        assert!(info.contains("room_code"), "fn::room_code 应已定义");

        let registered = registered_windows();
        assert!(registered.iter().any(|m| m.label == *first.label()));
        assert!(registered.iter().any(|m| m.label == *second.label()));

        first.drop_database().await.expect("cleanup first");
        second.drop_database().await.expect("cleanup second");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn registered_finalize_commits_journal_and_watermark_together() {
        let instance = own_instance().await;
        let mut window = create_window_on(&instance, 7905, 3, 7, ResourceThresholds::default())
            .await
            .expect("create window");
        window
            .execute("UPSERT pe:x SET noun = 'PIPE'", ExecMode::Both)
            .await
            .expect("stage data");
        window
            .scope(super::super::register_staged_finalize(
                super::super::StagedFinalize {
                    dbnum: 7905,
                    end_sesno: 7,
                    plan: Default::default(),
                    window_statements: Vec::new(),
                    cache_refnos: Vec::new(),
                },
            ))
            .await
            .expect("register finalize");

        let target = own_instance().await;
        target
            .use_ns("test")
            .use_db("commit_target")
            .await
            .expect("target db");
        window
            .commit_registered_to(&target)
            .await
            .expect("commit registered window");

        let mut response = target
            .query("RETURN pe:x.noun; RETURN dbnum_watermark:7905.applied_sesno;")
            .await
            .expect("query committed state");
        assert_eq!(
            response.take::<Option<String>>(0).expect("noun"),
            Some("PIPE".into())
        );
        assert_eq!(response.take::<Option<i32>>(1).expect("watermark"), Some(7));
        window.drop_database().await.expect("cleanup");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn align_end_sesno_can_narrow_or_widen_window_meta() {
        let instance = own_instance().await;
        let mut window = create_window_on(&instance, 7906, 10, 20, ResourceThresholds::default())
            .await
            .expect("create");
        window.align_end_sesno(16);
        assert_eq!(window.meta().end_sesno, 16);
        let registered = registered_windows();
        let meta = registered
            .iter()
            .find(|m| m.label == window.label())
            .expect("registered");
        assert_eq!(meta.end_sesno, 16);

        window.mark_writeback_stalled(&anyhow::anyhow!("fork offline"));
        let snapshot = resource_snapshots()
            .into_iter()
            .find(|row| row["label"] == window.label())
            .expect("window snapshot");
        assert_eq!(snapshot["state"], "writeback_stalled");
        assert_eq!(snapshot["writeback_error"], "fork offline");
        window.clear_writeback_stalled();
        window.drop_database().await.expect("cleanup");
    }

    /// 吸收扩窗：改区间不改名；资源档位到「拒绝吸收」后拒绝。
    #[tokio::test(flavor = "multi_thread")]
    async fn absorb_extends_range_until_resources_refuse() {
        let instance = own_instance().await;
        let mut window = create_window_on(
            &instance,
            7903,
            1,
            10,
            ResourceThresholds {
                warn_bytes: 4,
                refuse_absorb_bytes: 8,
                abandon_bytes: 1 << 30,
                warn_rows: 1 << 30,
                refuse_absorb_rows: 1 << 31,
                abandon_rows: 1 << 32,
            },
        )
        .await
        .expect("create");
        let label_before = window.label().to_string();

        window.absorb_extend(15).expect("normal absorb");
        assert_eq!(window.meta().end_sesno, 15);
        assert_eq!(window.label(), label_before, "吸收不改名");
        assert!(window.absorb_extend(12).is_err(), "上界只升不降");

        // 摄入推到「拒绝吸收」档位。
        window.gauge().record_journal(16);
        let error = window.absorb_extend(20).expect_err("必须拒绝吸收");
        assert!(error.to_string().contains("拒绝吸收"), "{error}");
        assert_eq!(window.meta().end_sesno, 15, "拒绝时区间不动");

        window.drop_database().await.expect("cleanup");
    }

    /// 终态 DROP 出册 + 清扫兜底孤儿库。
    #[tokio::test(flavor = "multi_thread")]
    async fn drop_unregisters_and_sweep_reaps_orphans() {
        let instance = own_instance().await;
        let window = create_window_on(&instance, 7904, 1, 3, ResourceThresholds::default())
            .await
            .expect("create");
        let label = window.label().to_string();

        // 孤儿库：手工建、不在册。
        instance
            .use_ns(STAGING_NS)
            .use_db("staging_7904_9999999")
            .await
            .expect("use orphan");
        instance
            .query("UPSERT junk:x SET v = 1")
            .await
            .expect("write orphan")
            .check()
            .expect("written");

        let dropped = sweep_orphan_staging_databases_on(&instance)
            .await
            .expect("sweep");
        assert!(
            dropped.contains(&"staging_7904_9999999".to_string()),
            "孤儿应被清扫: {dropped:?}"
        );
        assert!(!dropped.contains(&label), "在册窗口不得被清扫: {dropped:?}");

        window.drop_database().await.expect("drop");
        assert!(
            !registered_windows().iter().any(|m| m.label == label),
            "终态后应出册"
        );
    }
}
