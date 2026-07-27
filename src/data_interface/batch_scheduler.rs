//! 数据批次调度器——队列真身（ADR-011 §2/§5/§6/§9）。
//!
//! 合并 / 冻结 / FIFO 的**判定**只有一份，在 [`batch_queue`]；本模块负责把那份
//! 纯逻辑接到进程状态上：队列行与 [`TaskRegistry`] 任务行一一对应、入队唤醒
//! 消费者、暂停只挡出队。手动触发与 `async_watch` 自动发现两条路径都只调
//! [`BatchScheduler::enqueue`]，除此再无第二个入口。
//!
//! 队列不持久（ADR-011 §4）：durable 语义在水位与 `model_update_pending` 表上，
//! 重启后由 `init_watcher` 重扫水位把队列重建出来。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::Notify;

use crate::data_interface::batch_queue::{self, BatchState, DataBatch, Enqueued};
use crate::data_interface::task_registry::TaskRegistry;

/// 一次发现（文件会话号超过水位）携带的全部入队信息。
#[derive(Debug, Clone)]
pub struct DiscoveredBatch {
    pub project: String,
    pub dbnum: u32,
    pub db_type: String,
    pub path: PathBuf,
    /// 完整文件名（含扩展名，由 `discover_batch` 从 path 现取；仅作展示与
    /// 冻结重扫失败时的 fallback，执行一律以 `path` 为准）。
    pub file_name: String,
    /// 当前水位（入队时定左端用；执行时 worker 会重新读）。
    pub applied_sesno: i32,
    pub file_latest_sesno: i32,
}

/// 队列行的执行侧信息：`batch_queue::DataBatch` 只有纯粹的区间语义，
/// 执行还需要知道文件在哪、报进度记在哪个任务行上。
#[derive(Debug, Clone)]
struct RowMeta {
    task_id: String,
    project: String,
    path: PathBuf,
    file_name: String,
}

/// 冻结出队后交给 worker 的一份快照。
#[derive(Debug, Clone)]
pub struct FrozenBatch {
    pub task_id: String,
    pub project: String,
    pub dbnum: u32,
    pub db_type: String,
    pub path: PathBuf,
    pub file_name: String,
    pub start_sesno: i32,
    pub end_sesno: i32,
}

/// 入队回执的一行（HTTP 202 与日志共用；rollout 第九节第 7 条）。
#[derive(Debug, Clone, Serialize)]
pub struct EnqueuedBatchInfo {
    pub task_id: String,
    pub dbnum: u32,
    pub db_type: String,
    /// 在排队中的位置（1 起，含运行中的行不算）。
    pub position: usize,
    pub start_sesno: i32,
    pub end_sesno: i32,
}

/// 一次入队的结果：落点 + 对应任务行。
#[derive(Debug, Clone)]
pub struct EnqueueOutcome {
    pub outcome: Enqueued,
    pub info: EnqueuedBatchInfo,
}

/// 手动触发「扫描 + 入队”的整体回执（POST /update/execute 的 202 响应体）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ManualEnqueueReceipt {
    pub project: String,
    /// 本次扫描到的候选 dbnum 数（含最新与被阻断的）。
    pub scanned: usize,
    /// 新排的行（含接在运行中批次之后的）。
    pub enqueued: Vec<EnqueuedBatchInfo>,
    /// 并入既有排队行的（目标会话号被推高）。
    pub merged: Vec<EnqueuedBatchInfo>,
    /// 已被既有排队行覆盖、无需动作的 dbnum。
    pub already_covered: Vec<u32>,
    /// 阻断的库（同号多文件 / 回退）——压根不入队（ADR-011 结果段）。
    pub blocked: Vec<BlockedDbnum>,
    /// 水位已覆盖文件、无事可做的 dbnum 数。
    pub up_to_date: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockedDbnum {
    pub dbnum: u32,
    pub reason: String,
}

/// 队列快照的一行（含运行中的），供面板/日志。
#[derive(Debug, Clone, Serialize)]
pub struct QueueRow {
    pub task_id: String,
    pub dbnum: u32,
    pub db_type: String,
    pub state: &'static str,
    pub start_sesno: i32,
    pub end_sesno: i32,
}

pub struct BatchScheduler {
    /// 与 `meta` 同锁维护：`queue[i]` 的元数据在 `meta[(dbnum, is_running)]`。
    /// 运行时不变量（batch_queue 三条规则保证）：同一 dbnum 至多一行排队 +
    /// 一行运行中，因此 `(dbnum, state)` 是队列行的唯一键。
    inner: Mutex<QueueState>,
    /// 暂停只挡出队，不碰正在跑的那条（ADR-011 §9）。
    paused: AtomicBool,
    /// 入队 / 恢复时唤醒 worker。
    notify: Notify,
}

#[derive(Default)]
struct QueueState {
    queue: Vec<DataBatch>,
    meta: HashMap<(u32, bool), RowMeta>,
}

static SCHEDULER: OnceLock<BatchScheduler> = OnceLock::new();

impl BatchScheduler {
    pub fn global() -> &'static BatchScheduler {
        SCHEDULER.get_or_init(|| BatchScheduler {
            inner: Mutex::new(QueueState::default()),
            paused: AtomicBool::new(false),
            notify: Notify::new(),
        })
    }

    /// 把一次发现放进队列，并让注册表跟上（新行 / 并入 / 无动作）。
    ///
    /// 三种落点都会唤醒 worker：即便 `AlreadyCovered`，manual 触发的语义也是
    /// 「别等下一个 30s 轮询」——worker 醒来发现队列没变化也只亏一次空转。
    pub fn enqueue(&self, registry: &TaskRegistry, found: &DiscoveredBatch) -> EnqueueOutcome {
        let outcome = {
            let mut state = self.inner.lock().unwrap();
            let outcome = batch_queue::enqueue(
                &mut state.queue,
                found.dbnum,
                &found.db_type,
                found.applied_sesno,
                found.file_latest_sesno,
            );

            let queued_row = state
                .queue
                .iter()
                .find(|b| b.dbnum == found.dbnum && b.state == BatchState::Queued)
                .cloned();
            let position = state
                .queue
                .iter()
                .filter(|b| b.state == BatchState::Queued)
                .position(|b| b.dbnum == found.dbnum)
                .map(|i| i + 1)
                .unwrap_or(0);

            let info = match outcome {
                Enqueued::New | Enqueued::BehindRunning => {
                    let row = queued_row.expect("enqueue 刚排入的行必然存在");
                    let task_id = TaskRegistry::new_task_id("db");
                    registry.insert_queued_batch(
                        &task_id,
                        &found.project,
                        found.dbnum,
                        &found.db_type,
                        row.start_sesno,
                        row.end_sesno,
                    );
                    state.meta.insert(
                        (found.dbnum, false),
                        RowMeta {
                            task_id: task_id.clone(),
                            project: found.project.clone(),
                            path: found.path.clone(),
                            file_name: found.file_name.clone(),
                        },
                    );
                    EnqueuedBatchInfo {
                        task_id,
                        dbnum: found.dbnum,
                        db_type: found.db_type.clone(),
                        position,
                        start_sesno: row.start_sesno,
                        end_sesno: row.end_sesno,
                    }
                }
                Enqueued::Merged | Enqueued::AlreadyCovered => {
                    let row = queued_row.expect("合并目标行必然存在");
                    let meta = state
                        .meta
                        .get_mut(&(found.dbnum, false))
                        .expect("排队行必有元数据");
                    // 文件可能在两次触发之间被挪动，执行按最后一次观察到的路径走。
                    meta.path = found.path.clone();
                    meta.file_name = found.file_name.clone();
                    let task_id = meta.task_id.clone();
                    if outcome == Enqueued::Merged {
                        registry.update_queued_range(&task_id, row.end_sesno);
                    }
                    EnqueuedBatchInfo {
                        task_id,
                        dbnum: found.dbnum,
                        db_type: found.db_type.clone(),
                        position,
                        start_sesno: row.start_sesno,
                        end_sesno: row.end_sesno,
                    }
                }
            };
            EnqueueOutcome { outcome, info }
        };
        self.notify.notify_one();
        outcome
    }

    /// FIFO 出队并冻结（暂停时恒 None）。注册表行随之转 running。
    pub fn freeze_next(&self, registry: &TaskRegistry) -> Option<FrozenBatch> {
        let mut state = self.inner.lock().unwrap();
        let paused = self.paused.load(Ordering::SeqCst);
        let index = batch_queue::freeze_next(&mut state.queue, paused)?;
        let row = state.queue[index].clone();
        let meta = state
            .meta
            .remove(&(row.dbnum, false))
            .expect("被冻结的行必有排队元数据");
        state.meta.insert((row.dbnum, true), meta.clone());
        registry.mark_started(&meta.task_id);
        Some(FrozenBatch {
            task_id: meta.task_id,
            project: meta.project,
            dbnum: row.dbnum,
            db_type: row.db_type,
            path: meta.path,
            file_name: meta.file_name,
            start_sesno: row.start_sesno,
            end_sesno: row.end_sesno,
        })
    }

    /// 批次执行完毕：把运行中的那行从队列里摘掉（终态只留在注册表历史里）。
    pub fn finish(&self, dbnum: u32) {
        let mut state = self.inner.lock().unwrap();
        state
            .queue
            .retain(|b| !(b.dbnum == dbnum && b.state == BatchState::Running));
        state.meta.remove(&(dbnum, true));
    }

    /// 暂停出队（正在跑的那条会跑完为止——服务端没有中止接口）。
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// 恢复出队并唤醒 worker。
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.notify.notify_one();
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 设置暂停并**持久化**（ADR-011 §9：暂停是操作意图不是派生态，必须活过重启，
    /// 否则重启后队列立刻开吃，把暂停的用意整个抹掉且毫无提示）。
    ///
    /// 与水位同库（`queue_control:main`），不进队列表。先落库再改内存旗标：
    /// 持久化失败时宁可保持现状并报错，也不要「界面显示已暂停、重启后又开吃」。
    pub async fn set_paused_persistent(&self, paused: bool) -> anyhow::Result<()> {
        aios_core::SUL_DB
            .query(format!(
                "UPSERT queue_control:main SET paused = {paused}, updated_at = time::now();"
            ))
            .await
            .map_err(|e| anyhow::anyhow!("持久化队列暂停标志失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("持久化队列暂停标志语句失败: {e}"))?;
        if paused {
            self.pause();
        } else {
            self.resume();
        }
        Ok(())
    }

    /// 启动时恢复持久化的暂停状态（worker 起跑前调用）。
    ///
    /// 返回恢复后的暂停值；读不到记录视为未暂停。
    pub async fn restore_persisted_pause(&self) -> anyhow::Result<bool> {
        let mut response = aios_core::SUL_DB
            .query("SELECT VALUE paused FROM queue_control:main;")
            .await
            .map_err(|e| anyhow::anyhow!("读取队列暂停标志失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("读取队列暂停标志语句失败: {e}"))?;
        let stored: Vec<bool> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解码队列暂停标志失败: {e}"))?;
        let paused = stored.first().copied().unwrap_or(false);
        self.paused.store(paused, Ordering::SeqCst);
        Ok(paused)
    }

    /// 等新工作（入队 / 恢复）或超时。超时兜底轮询：唤醒丢失也只是晚一拍。
    pub async fn wait_for_work(&self, timeout: Duration) {
        let _ = tokio::time::timeout(timeout, self.notify.notified()).await;
    }

    /// 队列快照（含运行中行），按队列序。
    pub fn snapshot(&self) -> Vec<QueueRow> {
        let state = self.inner.lock().unwrap();
        state
            .queue
            .iter()
            .map(|b| {
                let running = b.state == BatchState::Running;
                let task_id = state
                    .meta
                    .get(&(b.dbnum, running))
                    .map(|m| m.task_id.clone())
                    .unwrap_or_default();
                QueueRow {
                    task_id,
                    dbnum: b.dbnum,
                    db_type: b.db_type.clone(),
                    state: if running { "running" } else { "queued" },
                    start_sesno: b.start_sesno,
                    end_sesno: b.end_sesno,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(dbnum: u32, applied: i32, latest: i32) -> DiscoveredBatch {
        DiscoveredBatch {
            project: "P".into(),
            dbnum,
            db_type: "DESI".into(),
            path: PathBuf::from(format!("D:/proj/db{dbnum}")),
            file_name: format!("db{dbnum}"),
            applied_sesno: applied,
            file_latest_sesno: latest,
        }
    }

    fn fresh() -> (BatchScheduler, TaskRegistry) {
        (
            BatchScheduler {
                inner: Mutex::new(QueueState::default()),
                paused: AtomicBool::new(false),
                notify: Notify::new(),
            },
            TaskRegistry::default(),
        )
    }

    #[test]
    fn enqueue_creates_a_queued_task_row_linked_to_the_batch() {
        let (scheduler, registry) = fresh();
        let outcome = scheduler.enqueue(&registry, &found(7997, 1023, 1034));
        assert_eq!(outcome.outcome, Enqueued::New);
        assert_eq!(outcome.info.position, 1);
        assert_eq!(outcome.info.start_sesno, 1024);

        let entry = registry.get(&outcome.info.task_id).expect("任务行已建");
        assert_eq!(entry.state.as_str(), "queued");
        assert_eq!(entry.dbnum, Some(7997));
        assert_eq!(entry.end_sesno, Some(1034));
    }

    #[test]
    fn merge_updates_the_same_task_row_instead_of_adding_one() {
        let (scheduler, registry) = fresh();
        let first = scheduler.enqueue(&registry, &found(7997, 1023, 1034));
        let second = scheduler.enqueue(&registry, &found(7997, 1023, 1041));
        assert_eq!(second.outcome, Enqueued::Merged);
        assert_eq!(second.info.task_id, first.info.task_id, "合并不该另开任务行");
        assert_eq!(
            registry.get(&first.info.task_id).unwrap().end_sesno,
            Some(1041)
        );
        assert_eq!(scheduler.snapshot().len(), 1);
    }

    #[test]
    fn freeze_marks_the_task_running_and_finish_removes_the_row() {
        let (scheduler, registry) = fresh();
        let queued = scheduler.enqueue(&registry, &found(7997, 1023, 1038));
        let job = scheduler.freeze_next(&registry).expect("有排队项");
        assert_eq!(job.task_id, queued.info.task_id);
        assert_eq!(job.end_sesno, 1038, "冻结时区间已定死");
        assert_eq!(
            registry.get(&job.task_id).unwrap().state.as_str(),
            "running"
        );

        // 运行期间新保存：另起一行接在右端之后（同 dbnum 两行）。
        let behind = scheduler.enqueue(&registry, &found(7997, 1023, 1041));
        assert_eq!(behind.outcome, Enqueued::BehindRunning);
        assert_eq!(behind.info.start_sesno, 1039);
        assert_eq!(scheduler.snapshot().len(), 2);

        scheduler.finish(job.dbnum);
        let rows = scheduler.snapshot();
        assert_eq!(rows.len(), 1, "终态行不留在队列里");
        assert_eq!(rows[0].state, "queued");
    }

    #[test]
    fn pausing_blocks_freeze_but_not_enqueue() {
        let (scheduler, registry) = fresh();
        scheduler.enqueue(&registry, &found(7997, 0, 10));
        scheduler.pause();
        assert!(scheduler.freeze_next(&registry).is_none(), "暂停期间不出队");
        let outcome = scheduler.enqueue(&registry, &found(7997, 0, 12));
        assert_eq!(outcome.outcome, Enqueued::Merged, "暂停挡的是出队不是入队");
        scheduler.resume();
        assert!(scheduler.freeze_next(&registry).is_some());
    }

    #[test]
    fn positions_count_queued_rows_only() {
        let (scheduler, registry) = fresh();
        scheduler.enqueue(&registry, &found(1, 0, 5));
        scheduler.freeze_next(&registry).unwrap();
        let second = scheduler.enqueue(&registry, &found(2, 0, 5));
        let third = scheduler.enqueue(&registry, &found(3, 0, 5));
        assert_eq!(second.info.position, 1, "运行中的行不占排队位置");
        assert_eq!(third.info.position, 2);
    }
}
