//! 数据库架构版本一键升级（`POST /api/v1/storage-version/upgrade`）。
//!
//! # 升级是什么
//!
//! `storage_version:current` 回答的是「**整个数据库**的架构是哪一版」，不是某张表、
//! 某个 dbnum 的属性。版本对不上时本进程读不懂库里的模型行，出路只有一条：
//! 清掉旧数据，重新解析，再重新生成模型。
//!
//! 这里把那条出路做成一次可记账的批处理：
//!
//! 1. 名单缺省 = 当前 MDB 声明的全部设计库 ∩ 监听范围；显式名单必须是这个范围的子集；
//!    整库（`full_scope`）时先清目录库解析行、`cata_dependency_state` 和 `parse_error`；
//!    部分名单不清这些共享行，因为别的设计库还在用；
//! 2. 每个库排一条 `intent=reinitialize` 的既有批次（`manual_update` 的逐库入口，
//!    不在本模块重写清库 / 解析 / 生成）。升级进行中的库，worker 会连网格几何一起清；
//! 3. 进度写在 `storage_upgrade:current`（目标版本、请求名单、完成名单、失败名单、起止时刻）；
//! 4. 只有**全部请求的库都 `Succeeded` 且请求集就是全范围**时才写
//!    `storage_version:current`。部分库升级不写——那会把一份仍然老形状的库冒充成新版。
//!
//! # 判定放在空闲轮
//!
//! 完成后要写库，不能在 handler 里等：一次升级是几分钟到几小时。判定点放在数据批次
//! worker 的空闲轮（`batch_worker::idle_round`）与 `/health` 的按需刷新里，两处都调用
//! [`reconcile_progress`]。任务终态从 `TaskRegistry` 读；进度记录持久在库里，进程重启
//! 后还能接着看。
//!
//! # 与 `generator_reconcile` 的区别
//!
//! 生成器变了可以自动整库重排；架构版本变了不行——升级会清空旧数据，必须由人按下按钮。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, RwLock};

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::data_interface::task_registry::TaskState;

/// 进度记录的表名与唯一记录 id。
pub const STORAGE_UPGRADE_TABLE: &str = "storage_upgrade";
pub const STORAGE_UPGRADE_RECORD: &str = "storage_upgrade:current";

/// 一个请求库对应的重新解析任务。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeTaskRef {
    pub dbnum: u32,
    pub task_id: String,
}

/// 失败的库与原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeFailure {
    pub dbnum: u32,
    pub reason: String,
}

/// `storage_upgrade:current` 的形状。
///
/// 字段与计划 §4 C7 对齐；`tasks` / `full_scope` 是实现完成判定所需的账：
/// 没有任务号就只能靠「同库最近一条任务」猜，那在重复触发时必然猜错。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageUpgradeProgress {
    /// 本进程期望的架构版本。
    pub target_version: u32,
    /// 本次请求的库名单（升序去重）。
    pub requested: Vec<u32>,
    /// 已 `Succeeded` 的库。
    #[serde(default)]
    pub done: Vec<u32>,
    /// 已终态且未成功的库。
    #[serde(default)]
    pub failed: Vec<UpgradeFailure>,
    pub started_at: String,
    #[serde(default)]
    pub finished_at: Option<String>,
    /// 请求库 → 重新解析任务。`already_covered` 的库从队列快照里补。
    #[serde(default)]
    pub tasks: Vec<UpgradeTaskRef>,
    /// 请求集是否覆盖了开始时的全范围（只有全范围完成才写版本记录）。
    #[serde(default)]
    pub full_scope: bool,
    /// 整库升级开始前清掉的目录库解析数据。部分名单没有这一格。
    #[serde(default)]
    pub shared_wipe: Option<crate::data_interface::fast_delete::SharedParseWipeReport>,
    /// 批次还在入队。这段时间任务号还没写上，缺任务号不能判失败。
    #[serde(default)]
    pub scheduling: bool,
}

impl StorageUpgradeProgress {
    /// 对外的三态 + done/failed：`idle` 只在没有记录时出现。
    pub fn state(&self) -> &'static str {
        if self.finished_at.is_none() {
            "running"
        } else if self.failed.is_empty() {
            "done"
        } else {
            "failed"
        }
    }

    /// 是否所有请求库都已落终态。
    fn all_settled(&self) -> bool {
        self.done.len() + self.failed.len() >= self.requested.len()
    }

    fn task_for(&self, dbnum: u32) -> Option<&str> {
        self.tasks
            .iter()
            .find(|task| task.dbnum == dbnum)
            .map(|task| task.task_id.as_str())
            .filter(|task_id| !task_id.is_empty())
    }

    fn mark_done(&mut self, dbnum: u32) -> bool {
        if self.done.contains(&dbnum) {
            return false;
        }
        self.done.push(dbnum);
        self.done.sort_unstable();
        true
    }

    fn mark_failed(&mut self, dbnum: u32, reason: impl Into<String>) -> bool {
        if self.done.contains(&dbnum) || self.failed.iter().any(|failure| failure.dbnum == dbnum) {
            return false;
        }
        self.failed.push(UpgradeFailure {
            dbnum,
            reason: reason.into(),
        });
        self.failed.sort_by_key(|failure| failure.dbnum);
        true
    }
}

/// 最近一次进度记录的进程内副本，供同步的 `/health` 读取。
static LAST_PROGRESS: LazyLock<RwLock<Option<StorageUpgradeProgress>>> =
    LazyLock::new(|| RwLock::new(None));

/// 是否已经读过库。没有进度记录时 `LAST_PROGRESS` 也是 `None`，不能拿它当「没读过」。
static LOADED: AtomicBool = AtomicBool::new(false);

/// 进程启动时是否「整库为空」：没有 `storage_version:current`，也没有任何 `pe` 行。
/// D-7：这种库从零初始化完成时自动写版本记录，不需要人工点升级。
static FRESH_DATABASE: AtomicBool = AtomicBool::new(false);

fn progress_lock() -> std::sync::RwLockReadGuard<'static, Option<StorageUpgradeProgress>> {
    LAST_PROGRESS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn progress_lock_mut() -> std::sync::RwLockWriteGuard<'static, Option<StorageUpgradeProgress>> {
    LAST_PROGRESS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 只更新进程内进度。入队前调用，好让同进程的 worker 在回执落库之前就看得到。
fn publish_progress(progress: &StorageUpgradeProgress) {
    *progress_lock_mut() = Some(progress.clone());
    LOADED.store(true, Ordering::SeqCst);
}

/// 这个库是不是正在进行的升级名单里。是的话清库要连网格几何一起删。
///
/// 进度先写在进程内，收尾再落库；进程重启后从 `storage_upgrade:current` 接着看。
pub async fn purges_geometry(dbnum: u32) -> bool {
    ensure_loaded().await;
    purges_geometry_of(last_progress().as_ref(), dbnum)
}

pub(crate) fn purges_geometry_of(progress: Option<&StorageUpgradeProgress>, dbnum: u32) -> bool {
    progress.is_some_and(|progress| {
        progress.finished_at.is_none() && progress.requested.contains(&dbnum)
    })
}

/// 是否该写 `storage_version:current`：全部请求库成功，且请求集就是全范围。
///
/// 部分名单即使全部成功也不写——那会把一份仍有老形状库的数据库冒充成新版；
/// 有失败更不写。判定单独成一个纯函数，好让三条边界各自有断言。
fn should_write_version_record(progress: &StorageUpgradeProgress) -> bool {
    progress.all_settled() && progress.failed.is_empty() && progress.full_scope
}

/// 当前进度（同步，给 `/health`）。没有记录 = `None`（对外 `idle`）。
pub fn last_progress() -> Option<StorageUpgradeProgress> {
    progress_lock().clone()
}

/// 首次访问时从库里装载进度记录（进程重启后接着看）。
pub async fn ensure_loaded() {
    if LOADED.load(Ordering::SeqCst) || progress_lock().is_some() {
        return;
    }
    match read_progress().await {
        Ok(progress) => {
            let mut slot = progress_lock_mut();
            if slot.is_none() {
                *slot = progress;
            }
            LOADED.store(true, Ordering::SeqCst);
        }
        Err(error) => {
            log::warn!("读取 {STORAGE_UPGRADE_RECORD} 失败，升级进度暂不可见: {error:#}");
        }
    }
}

async fn read_progress() -> anyhow::Result<Option<StorageUpgradeProgress>> {
    let mut response = aios_core::SUL_DB
        .query(format!("SELECT * FROM ONLY {STORAGE_UPGRADE_RECORD};"))
        .await?;
    let row: Option<StorageUpgradeProgress> = response.take(0)?;
    Ok(row)
}

async fn write_progress(progress: &StorageUpgradeProgress) -> anyhow::Result<()> {
    let value = serde_json::to_value(progress)?;
    aios_core::SUL_DB
        .query(format!("UPSERT {STORAGE_UPGRADE_RECORD} CONTENT $record;"))
        .bind(("record", value))
        .await?
        .check()?;
    *progress_lock_mut() = Some(progress.clone());
    LOADED.store(true, Ordering::SeqCst);
    Ok(())
}

/// 本次启动观察到的「全库为空」状态：版本记录缺失且一条 `pe` 行都没有。
///
/// 只读一条 `LIMIT 1`：大库上不必数全表；空库上也不会误把「老库但没模型行」当成新库——
/// 版本记录缺失时，老库同样可能没有 `pe` 行，但那种库确实需要重新初始化，自动写版本
/// 与人工升级的差别只是少一次点击。判据写进 D-7 的落定说明。
pub(crate) async fn observe_startup(
    report: &crate::data_interface::storage_version_reconcile::StorageVersionReport,
) {
    if report.stored.is_some() {
        FRESH_DATABASE.store(false, Ordering::SeqCst);
        return;
    }
    let has_pe = match aios_core::SUL_DB
        .query("SELECT VALUE id FROM pe LIMIT 1;")
        .await
    {
        Ok(mut response) => match response.take::<Vec<surrealdb::sql::Thing>>(0) {
            Ok(rows) => !rows.is_empty(),
            Err(_) => true,
        },
        Err(_) => true,
    };
    FRESH_DATABASE.store(!has_pe, Ordering::SeqCst);
    if !has_pe {
        println!(
            "存储架构版本：库里没有版本记录，也没有任何模型行——按全新空库处理，首次初始化完成时自动写版本 {}",
            report.expected
        );
    }
}

/// 全新空库初始化完成时写版本记录（D-7）。
///
/// 只由数据批次 worker 在 `model_ready` 第一次翻真时调用；已经写过的库不会重复写。
pub async fn write_version_if_fresh_database() -> bool {
    if !FRESH_DATABASE.load(Ordering::SeqCst) {
        return false;
    }
    if crate::data_interface::storage_version_reconcile::last_report()
        .as_ref()
        .is_some_and(|report| report.is_caught_up())
    {
        return false;
    }
    if let Err(error) = write_storage_version_record().await {
        log::warn!("全新空库初始化完成，但架构版本记录没写进库: {error:#}");
        return false;
    }
    FRESH_DATABASE.store(false, Ordering::SeqCst);
    let _ = crate::data_interface::storage_version_reconcile::refresh_if_stale().await;
    println!(
        "存储架构版本：全新空库初始化完成，已写入 storage_version:current（version={}）",
        aios_core::model_storage::MODEL_STORAGE_VERSION
    );
    true
}

/// MDB 声明、且文件出自本库主项目的元素库，升序去重。
///
/// 这是「整库版本记录」唯一的分母：`watch_dbnums` 只是本进程的监听子集，
/// 不能拿它替整库作证。同号的外部项目设计库（SCB / ACP / APS …）也不进来：
/// 持久层按裸 dbnum 建键，一份 Surreal 库只服务一个主项目
/// （`increment_manager::is_foreign_design_data`），它们的行不在本库，
/// 不能由本库的版本记录作证。`project = None`（声明了却没文件）同样不写进分母。
fn owned_design_dbnums(
    membership: &crate::data_interface::mdb_membership::MdbMembership,
    project: &str,
) -> Vec<u32> {
    let project = project.trim();
    let mut dbnums: Vec<u32> = membership
        .element_databases()
        .filter(|database| {
            database
                .project
                .as_deref()
                .is_some_and(|owner| owner.eq_ignore_ascii_case(project))
        })
        .map(|database| database.dbnum)
        .collect();
    dbnums.sort_unstable();
    dbnums.dedup();
    dbnums
}

/// 当前 MDB 声明、归属本库主项目的元素库。
pub async fn declared_dbnums() -> anyhow::Result<Vec<u32>> {
    let option = aios_core::get_db_option().clone();
    let declared = tokio::task::spawn_blocking(move || {
        let membership = match crate::data_interface::mdb_membership::get(
            &option.project_name,
            &option.mdb_name,
        ) {
            Some(membership) => membership,
            None => std::sync::Arc::new(crate::data_interface::mdb_membership::resolve(
                &option,
                &option.project_name,
                &option.mdb_name,
            )?),
        };
        Ok::<_, anyhow::Error>(owned_design_dbnums(&membership, &option.project_name))
    })
    .await
    .map_err(|error| anyhow::anyhow!("读取 MDB 成员的阻塞任务异常: {error}"))??;
    Ok(declared)
}

/// 当前 MDB 声明的设计库 ∩ 监听范围，升序去重。升级端点接受的请求范围。
pub async fn scope_dbnums() -> anyhow::Result<Vec<u32>> {
    Ok(declared_dbnums()
        .await?
        .into_iter()
        .filter(|dbnum| crate::data_interface::watch_scope::admits(*dbnum))
        .collect())
}

/// 请求集是否覆盖了 MDB 声明的全部设计库。
///
/// **不能用监听范围当分母**：`watch_dbnums=[8000]` 的进程只升级 8000 时，
/// 请求集等于监听范围，却远不等于整库；那种情况写版本记录会把其余仍为旧形状的
/// 库一块冒充成新版（2026-10-29 用旧库演练时确认）。
pub fn covers_whole_database(requested: &[u32], declared: &[u32]) -> bool {
    let mut requested = requested.to_vec();
    requested.sort_unstable();
    requested.dedup();
    requested == declared
}

/// 名单解析：`None` = 全范围；显式名单去重排序后必须是范围的子集。
///
/// 返回 `(requested, full_scope)`。越界不在这里决定 HTTP 码——调用方按 400 处理。
pub fn resolve_requested(
    scope: &[u32],
    explicit: Option<Vec<u32>>,
) -> Result<(Vec<u32>, bool), Vec<u32>> {
    match explicit {
        None => Ok((scope.to_vec(), true)),
        Some(mut dbnums) => {
            dbnums.sort_unstable();
            dbnums.dedup();
            let out_of_scope = dbnums
                .iter()
                .copied()
                .filter(|dbnum| !scope.contains(dbnum))
                .collect::<Vec<_>>();
            if !out_of_scope.is_empty() {
                return Err(out_of_scope);
            }
            let full_scope = dbnums.as_slice() == scope;
            Ok((dbnums, full_scope))
        }
    }
}

/// 排升级任务并落进度记录。返回的是本次请求的回执（handler 的 202 体）。
pub async fn start(
    mgr: &crate::data_interface::tidb_manager::AiosDBManager,
    project: &str,
    mdb: Option<&str>,
    requested: &[u32],
    full_scope: bool,
) -> anyhow::Result<(
    StorageUpgradeProgress,
    crate::data_interface::batch_scheduler::ManualEnqueueReceipt,
)> {
    // 上一次升级还在跑时不允许再排一批：两批 reinitialize 叠在同一个库上会把进度账
    // 记到对方头上，而且第二次清库会打断正在解析的那一次。
    ensure_loaded().await;
    if let Some(previous) = last_progress() {
        if previous.finished_at.is_none()
            && previous.requested.iter().any(|dbnum| {
                previous
                    .task_for(*dbnum)
                    .and_then(|task_id| {
                        crate::data_interface::task_registry::TaskRegistry::global().get(task_id)
                    })
                    .is_some_and(|entry| !entry.state.is_terminal())
            })
        {
            anyhow::bail!(
                "已有存储架构升级在进行中（requested={:?}，done={:?}）；等它结束或查看 /health.storage_version.upgrade",
                previous.requested,
                previous.done
            );
        }
    }

    let shared_wipe = if full_scope {
        println!(
            "存储架构升级：先清目录库解析行、cata_dependency_state 和 parse_error，再逐库重新解析"
        );
        Some(
            crate::data_interface::fast_delete::wipe_shared_parse_rows_for_upgrade()
                .await
                .map_err(|error| {
                    anyhow::anyhow!("升级前清理目录库解析数据失败，未排重新解析批次: {error:#}")
                })?,
        )
    } else {
        println!(
            "存储架构升级：这次只升级部分库 requested={requested:?}，不清目录库解析行、cata_dependency_state 和 parse_error（别的设计库还在用）"
        );
        None
    };
    // 先让本进程看见「升级进行中」，再入队。worker 可能在回执落库之前就开始清库，
    // `purges_geometry` 靠这份未结束的进度决定要不要连网格一起删。
    let mut progress = StorageUpgradeProgress {
        target_version: aios_core::model_storage::MODEL_STORAGE_VERSION,
        requested: requested.to_vec(),
        done: Vec::new(),
        failed: Vec::new(),
        started_at: Local::now().to_rfc3339(),
        finished_at: None,
        tasks: Vec::new(),
        full_scope,
        shared_wipe,
        scheduling: true,
    };
    publish_progress(&progress);

    let receipt = mgr
        .enqueue_manual_update_with(
            project,
            mdb,
            Some(requested),
            crate::data_interface::batch_scheduler::ManualEnqueueOptions { reinitialize: true },
        )
        .await;

    let mut tasks: Vec<UpgradeTaskRef> = Vec::new();
    for info in receipt.enqueued.iter().chain(receipt.merged.iter()) {
        if requested.contains(&info.dbnum) && !info.task_id.is_empty() {
            tasks.push(UpgradeTaskRef {
                dbnum: info.dbnum,
                task_id: info.task_id.clone(),
            });
        }
    }
    // `AlreadyCovered` 不在回执里带任务号：从队列快照按「同库 + 重新解析」补。
    if tasks.len() < requested.len() {
        for row in crate::data_interface::batch_scheduler::BatchScheduler::global().snapshot() {
            if requested.contains(&row.dbnum)
                && !row.task_id.is_empty()
                && row.intent
                    == crate::data_interface::batch_queue::BatchIntent::Reinitialize.as_str()
                && !tasks.iter().any(|task| task.dbnum == row.dbnum)
            {
                tasks.push(UpgradeTaskRef {
                    dbnum: row.dbnum,
                    task_id: row.task_id,
                });
            }
        }
    }
    tasks.sort_by_key(|task| task.dbnum);
    tasks.dedup_by_key(|task| task.dbnum);
    progress.tasks = tasks;
    progress.scheduling = false;
    for blocked in &receipt.blocked {
        if progress.requested.contains(&blocked.dbnum) {
            progress.mark_failed(blocked.dbnum, blocked.reason.clone());
        }
    }
    for dbnum in requested {
        if progress.task_for(*dbnum).is_none() && !progress.failed.iter().any(|f| f.dbnum == *dbnum)
        {
            progress.mark_failed(
                *dbnum,
                "没有排入重新解析批次（查看 /api/v1/update/execute 回执的 warnings）",
            );
        }
    }
    write_progress(&progress).await?;
    println!(
        "存储架构升级已排：target={} requested={:?} done={:?} failed={:?} full_scope={}",
        progress.target_version,
        progress.requested,
        progress.done,
        progress.failed,
        progress.full_scope
    );
    Ok((progress, receipt))
}

/// 空闲轮 / `/health` 的完成判定。返回进度（没有记录时为 `None`）。
pub async fn reconcile_progress() -> Option<StorageUpgradeProgress> {
    ensure_loaded().await;
    let Some(mut progress) = last_progress() else {
        return None;
    };
    if progress.finished_at.is_some() {
        return Some(progress);
    }

    // 队列快照只在本轮有缺口时才取；正常路径全在进度记录的 tasks 里。
    let mut queue_rows: Option<Vec<crate::data_interface::batch_scheduler::QueueRow>> = None;
    let mut changed = false;
    for dbnum in progress.requested.clone() {
        if progress.done.contains(&dbnum)
            || progress.failed.iter().any(|failure| failure.dbnum == dbnum)
        {
            continue;
        }
        let mut task_id = progress.task_for(dbnum).map(str::to_owned);
        if task_id.is_none() {
            let rows = queue_rows.get_or_insert_with(|| {
                crate::data_interface::batch_scheduler::BatchScheduler::global().snapshot()
            });
            task_id = rows
                .iter()
                .find(|row| {
                    row.dbnum == dbnum
                        && !row.task_id.is_empty()
                        && row.intent
                            == crate::data_interface::batch_queue::BatchIntent::Reinitialize
                                .as_str()
                })
                .map(|row| row.task_id.clone());
            if let Some(task_id) = task_id.clone() {
                progress.tasks.push(UpgradeTaskRef { dbnum, task_id });
                changed = true;
            }
        }
        let Some(task_id) = task_id else {
            // 入队还没写上任务号。这时判失败会把 `finished_at` 填上，worker 就不再连网格一起清。
            if progress.scheduling {
                continue;
            }
            changed |= progress.mark_failed(dbnum, "没有找到重新解析任务");
            continue;
        };
        match crate::data_interface::task_registry::TaskRegistry::global().get(&task_id) {
            Some(entry) if entry.state == TaskState::Succeeded => {
                changed |= progress.mark_done(dbnum);
            }
            Some(entry) if entry.state.is_terminal() => {
                changed |= progress.mark_failed(
                    dbnum,
                    format!("重新解析任务 {} 结束于 {}", task_id, entry.state.as_str()),
                );
            }
            Some(_) => {}
            None => {
                changed |= progress.mark_failed(
                    dbnum,
                    format!("重新解析任务 {task_id} 不在任务注册表（进程重启或已被淘汰）"),
                );
            }
        }
    }

    if progress.all_settled() {
        progress.finished_at = Some(Local::now().to_rfc3339());
        changed = true;
        if should_write_version_record(&progress) {
            match write_storage_version_record().await {
                Ok(()) => {
                    let _ =
                        crate::data_interface::storage_version_reconcile::refresh_if_stale().await;
                    println!(
                        "存储架构升级完成：{} 个库全部重新解析，已写 storage_version:current（version={}）",
                        progress.requested.len(),
                        progress.target_version
                    );
                }
                Err(error) => {
                    let dbnum = progress.requested.first().copied().unwrap_or_default();
                    progress
                        .mark_failed(dbnum, format!("写 storage_version:current 失败: {error:#}"));
                    log::warn!("存储架构升级完成，但版本记录没写进库: {error:#}");
                }
            }
        } else if !progress.failed.is_empty() {
            println!(
                "存储架构升级结束但未全部成功：requested={:?} done={:?} failed={:?}；不写版本记录",
                progress.requested, progress.done, progress.failed
            );
        } else {
            println!(
                "存储架构升级结束（部分名单）：requested={:?} done={:?}；请求集不是全范围，不写版本记录",
                progress.requested, progress.done
            );
        }
    }

    if changed {
        if let Err(error) = write_progress(&progress).await {
            log::warn!("升级进度记录没写进库: {error:#}");
        }
    }
    Some(progress)
}

/// 写 `storage_version:current`。整库一条，三格一起替换。
pub async fn write_storage_version_record() -> anyhow::Result<()> {
    use aios_core::model_storage::{
        REBUILD_COLUMN, SHAPE_COLUMN, StorageVersionRecord, VERSION_COLUMN,
    };

    let record = StorageVersionRecord::current();
    let mut row = serde_json::Map::new();
    row.insert(VERSION_COLUMN.to_owned(), serde_json::json!(record.version));
    row.insert(SHAPE_COLUMN.to_owned(), serde_json::json!(record.shape));
    row.insert(REBUILD_COLUMN.to_owned(), serde_json::json!(record.rebuild));
    aios_core::SUL_DB
        .query(format!(
            "UPSERT {} CONTENT $record;",
            aios_core::model_storage::STORAGE_VERSION_RECORD
        ))
        .bind(("record", serde_json::Value::Object(row)))
        .await?
        .check()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn design_db(
        dbnum: u32,
        project: Option<&str>,
    ) -> crate::data_interface::mdb_membership::MdbDatabase {
        crate::data_interface::mdb_membership::MdbDatabase {
            dbnum,
            styp: crate::data_interface::mdb_membership::DESI_STYP,
            proj: 0,
            declared_project_code: None,
            name: format!("*DB{dbnum}"),
            path: Some(std::path::PathBuf::from(format!("D:/proj/db{dbnum}"))),
            project: project.map(str::to_owned),
            shadowed: Vec::new(),
        }
    }

    /// 同号的外部项目设计库不能进整库分母：持久层按裸 dbnum 建键，它们的行
    /// 不在本库；声明了却没文件的库同样不能替整库作证。
    #[test]
    fn only_design_databases_owned_by_this_project_count_as_the_whole_database() {
        let membership = crate::data_interface::mdb_membership::MdbMembership::test_with_databases(
            "AvevaMarineSample",
            "ALL",
            vec![
                design_db(8000, Some("AvevaMarineSample")),
                design_db(7997, Some("AvevaMarineSample")),
                design_db(6003, Some("SCB")),
                design_db(7009, Some("AvevaCatalogue")),
                design_db(7321, None),
            ],
        );
        assert_eq!(
            owned_design_dbnums(&membership, "AvevaMarineSample"),
            vec![7997, 8000]
        );
    }

    fn progress() -> StorageUpgradeProgress {
        StorageUpgradeProgress {
            target_version: 1,
            requested: vec![100, 200],
            done: Vec::new(),
            failed: Vec::new(),
            started_at: "2026-10-28T00:00:00+08:00".to_owned(),
            finished_at: None,
            tasks: vec![UpgradeTaskRef {
                dbnum: 100,
                task_id: "t-100".to_owned(),
            }],
            full_scope: true,
            shared_wipe: None,
            scheduling: false,
        }
    }

    #[test]
    fn default_list_is_the_whole_scope_and_an_explicit_full_list_is_also_full_scope() {
        assert_eq!(
            resolve_requested(&[100, 200], None),
            Ok((vec![100, 200], true))
        );
        assert_eq!(
            resolve_requested(&[100, 200], Some(vec![200, 100, 100])),
            Ok((vec![100, 200], true)),
            "显式全名单（含重复）去重后仍等于全范围"
        );
        assert_eq!(
            resolve_requested(&[100, 200], Some(vec![200])),
            Ok((vec![200], false)),
            "部分名单不写版本记录"
        );
    }

    #[test]
    fn an_out_of_scope_dbnum_is_rejected() {
        assert_eq!(
            resolve_requested(&[100, 200], Some(vec![200, 300])),
            Err(vec![300])
        );
    }

    #[test]
    fn partial_completion_does_not_settle_or_write() {
        let mut progress = progress();
        progress.mark_done(100);
        assert!(!progress.all_settled());
        assert_eq!(progress.state(), "running");
        progress.mark_done(200);
        assert!(progress.all_settled());
        assert_eq!(progress.state(), "running", "finished_at 还没落");
        progress.finished_at = Some("2026-10-28T01:00:00+08:00".to_owned());
        assert_eq!(progress.state(), "done");
    }

    #[test]
    fn a_failure_makes_the_state_failed_and_blocks_the_version_write() {
        let mut progress = progress();
        progress.mark_done(100);
        progress.mark_failed(200, "task failed");
        progress.finished_at = Some("2026-10-28T01:00:00+08:00".to_owned());
        assert_eq!(progress.state(), "failed");
        assert!(!progress.failed.is_empty());
        assert!(!should_write_version_record(&progress));
    }

    #[test]
    fn the_version_record_is_written_only_when_every_requested_dbnum_succeeded_in_full_scope() {
        let mut progress = progress();
        assert!(!should_write_version_record(&progress), "还没完成");

        progress.mark_done(100);
        assert!(!should_write_version_record(&progress), "部分完成");

        progress.mark_done(200);
        assert!(
            should_write_version_record(&progress),
            "全部成功且全范围才写"
        );
    }

    #[test]
    fn a_full_partial_list_never_writes_the_version_record() {
        // 全范围之外的部分名单：库都升级成功了，但数据库整体仍是混合状态。
        let mut progress = StorageUpgradeProgress {
            requested: vec![100, 200],
            full_scope: false,
            ..progress()
        };
        progress.mark_done(100);
        progress.mark_done(200);
        assert!(progress.failed.is_empty());
        assert!(!should_write_version_record(&progress));
    }

    /// 监听限定模式：请求集等于监听范围（[8000]），但 MDB 声明的是 [8000, 8001]。
    /// 这种「本地全范围」绝不能当成整库已完成。
    #[test]
    fn a_limited_watch_scope_is_never_the_whole_database() {
        assert!(!covers_whole_database(&[8000], &[8000, 8001]));
        assert!(covers_whole_database(&[8001, 8000], &[8000, 8001]));
        assert!(!covers_whole_database(&[8000], &[8000, 8001]));
    }

    #[test]
    fn geometry_is_purged_only_while_that_dbnum_is_still_being_upgraded() {
        let running = progress();
        assert!(purges_geometry_of(Some(&running), 100));
        assert!(purges_geometry_of(Some(&running), 200));
        assert!(!purges_geometry_of(Some(&running), 8000));
        let mut finished = running.clone();
        finished.finished_at = Some("2026-10-28T01:00:00+08:00".to_owned());
        assert!(!purges_geometry_of(Some(&finished), 100));
        assert!(!purges_geometry_of(None, 100));
    }

    /// 部分名单不清共享解析数据。目录行被别的设计库用着，而且部分升级本来就不写版本记录。
    #[test]
    fn a_partial_upgrade_does_not_wipe_shared_parse_rows() {
        let body = include_str!("storage_upgrade.rs")
            .split_once("pub async fn start(")
            .expect("start")
            .1
            .split_once("pub async fn reconcile_progress(")
            .expect("reconcile follows start")
            .0;
        let gate = body.find("if full_scope").expect("full_scope 分支");
        let wipe = body
            .find("wipe_shared_parse_rows_for_upgrade()")
            .expect("整库才清共享解析数据");
        let warn = body
            .find("不清目录库解析行")
            .expect("部分范围要说明为什么不清");
        let enqueue = body.find("enqueue_manual_update_with(").expect("入队");
        assert!(
            gate < wipe && wipe < enqueue,
            "共享清理必须在 full_scope 分支里，并且发生在入队之前: {body}"
        );
        assert!(
            gate < warn && warn < enqueue,
            "部分范围只打印说明，不清共享数据: {body}"
        );
    }

    /// 入队还没写上任务号时，缺任务号不能判失败，否则 `finished_at` 一填上就不再清网格。
    #[test]
    fn a_batch_still_being_scheduled_is_not_marked_failed_for_a_missing_task() {
        let body = include_str!("storage_upgrade.rs")
            .split_once("pub async fn reconcile_progress(")
            .expect("reconcile")
            .1
            .split_once("没有找到重新解析任务")
            .expect("missing-task failure")
            .0;
        assert!(
            body.contains("if progress.scheduling"),
            "缺任务号时要先看是不是还在入队: {body}"
        );
    }

    /// 升级只走既有逐库重新解析入口，不另写清库 / 解析 / 生成路径；这两个字串少了
    /// 任何一个，升级都会退化成「只重算模型」——版本记录写得再对，旧数据也没清。
    #[test]
    fn the_upgrade_reuses_the_reinitialize_enqueue_path() {
        let source = include_str!("storage_upgrade.rs");
        assert!(
            source.contains("enqueue_manual_update_with"),
            "必须复用既有入队入口"
        );
        assert!(
            source.contains("ManualEnqueueOptions { reinitialize: true }"),
            "必须带 reinitialize: true"
        );
    }

    #[test]
    fn a_task_is_never_counted_both_done_and_failed() {
        let mut progress = progress();
        assert!(progress.mark_done(100));
        assert!(!progress.mark_done(100));
        assert!(!progress.mark_failed(100, "late failure"));
        assert_eq!(progress.done, vec![100]);
        assert!(progress.failed.is_empty());
    }
}
