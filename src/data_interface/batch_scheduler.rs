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
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::data_interface::batch_queue::{self, BatchIntent, BatchState, DataBatch, Enqueued};
use crate::data_interface::initialization_phase::{DataPhase, InitializationCoordinator};
use crate::data_interface::task_registry::{TaskRegistry, TaskState};

/// 一次发现（文件会话号超过水位）携带的全部入队信息。
#[derive(Debug, Clone)]
pub struct DiscoveredBatch {
    pub project: String,
    pub dbnum: u32,
    pub db_type: String,
    pub phase: DataPhase,
    pub epoch_id: u64,
    pub intent: BatchIntent,
    pub path: PathBuf,
    /// 完整文件名（含扩展名，由 `discover_batch` 从 path 现取；仅作展示与
    /// 冻结重扫失败时的 fallback，执行一律以 `path` 为准）。
    pub file_name: String,
    /// 当前水位（入队时定左端用；执行时 worker 会重新读）。
    pub applied_sesno: i32,
    pub file_latest_sesno: i32,
    /// `merged_sesnos` 的基线：本次触发**登记观察之前**的上一次扫描观察值，
    /// 由发现方从裁决（`ScanVerdict::previous_file_latest_sesno`）里取、在
    /// `record_observation` 覆盖它之前冻结。执行侧不得再现读（见
    /// [`batch_queue::DataBatch::previous_observed_sesno`]）。
    pub previous_observed_sesno: i32,
    /// 第一条待应用保存（`applied_sesno + 1`）的 E3D 写入时刻（RFC3339）。
    /// 队列「保存窗口」列的左端（plant-ui ADR-0019）；读不到就是 `None`，那一格留空。
    pub first_pending_sesno_time: Option<String>,
    /// `file_latest_sesno` 那条保存的 E3D 写入时刻，窗口右端。
    pub file_latest_sesno_time: Option<String>,
}

/// 端点对得上才把时刻贴上去。
///
/// 队列行的端点未必等于这次发现的端点：排在运行批次之后的那条左端是
/// `running_end + 1`，右端也可能已经被别的触发推得更高。时刻只有在端点对得上时
/// 才是**那条保存**的时刻，对不上就空着——ADR-0019 的降级规则是缺席不摆假数据，
/// 一个贴错行的时刻比没有时刻更糟。
fn time_for(row_sesno: i32, observed_sesno: i32, observed_time: &Option<String>) -> Option<String> {
    if row_sesno == observed_sesno {
        observed_time.clone()
    } else {
        None
    }
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
    pub phase: DataPhase,
    pub epoch_id: u64,
    pub intent: BatchIntent,
    pub path: PathBuf,
    pub file_name: String,
    pub start_sesno: i32,
    pub end_sesno: i32,
    /// 入队时冻结的 `merged_sesnos` 基线（见 [`batch_queue::DataBatch::previous_observed_sesno`]）。
    pub previous_observed_sesno: i32,
}

/// 入队回执的一行（HTTP 202 与日志共用；rollout 第九节第 7 条）。
#[derive(Debug, Clone, Serialize)]
pub struct EnqueuedBatchInfo {
    pub task_id: String,
    pub dbnum: u32,
    pub db_type: String,
    pub phase: &'static str,
    pub epoch_id: u64,
    pub intent: &'static str,
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

/// [`BatchScheduler::next_dispatch`] 的结果（ADR-011 2026-08-09 修订）。
#[derive(Debug)]
pub enum DispatchOutcome {
    /// 冻结了一条批次；`exclusive` = 该批次要求独占（派发方在它收敛前不得再派发）。
    Frozen { job: FrozenBatch, exclusive: bool },
    /// FIFO 首个可跑行要求独占但在飞非空：等在飞收敛，不越过它派发。
    HeadNeedsExclusive,
    /// 无事可派（空队列 / 暂停 / 可跑行的 dbnum 都在跑）。
    Idle,
}

/// 手动触发「扫描 + 入队”的整体回执（POST /update/execute 的 202 响应体）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ManualEnqueueReceipt {
    pub project: String,
    /// 本次执行范围照哪个 MDB 解的（带前导 `/`）。
    ///
    /// 预览与执行是两次独立解析，中间 MDB 可能被改过；不报出来的话，人只能假定
    /// 这次跑的范围跟预览时看到的那份一样。
    #[serde(default)]
    pub mdb: String,
    /// SurrealDB namespace actually used by the service.
    #[serde(default)]
    pub namespace: String,
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
    /// 跨项目裸 dbnum 冲突中被显式项目优先级遮蔽的候选。
    #[serde(default)]
    pub shadowed: Vec<ShadowedCandidate>,
    /// 水位已覆盖文件、无事可做的 dbnum 数。
    pub up_to_date: usize,
    /// 本次请求带了 `dbnums` 子集（ADR-020）而**没被勾选**的库：不扫描、不入队、
    /// 水位不动，等下一次执行。全范围请求时恒为空。
    pub unselected: Vec<u32>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockedDbnum {
    pub dbnum: u32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowedCandidate {
    pub project: String,
    pub dbnum: u32,
    pub file_path: String,
    pub selected_project: String,
}

/// 队列快照的一行（含运行中的），供面板/日志。
#[derive(Debug, Clone, Serialize)]
pub struct QueueRow {
    pub task_id: String,
    pub dbnum: u32,
    pub db_type: String,
    pub phase: &'static str,
    pub epoch_id: u64,
    pub blocked_by_phase: Option<&'static str>,
    pub intent: &'static str,
    /// `running` / `queued` / `held`。`held` = 重扫排出来的积压，等这个 dbnum
    /// 真的来一次增量才会被派发（见 [`batch_queue::DataBatch::held`]）。
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
    /// 本进程是否已经被「真实触发」上过弦。
    ///
    /// `startup_autorun` 关着时启动为 false，第一次非重扫入队（watch 事件 / 人工
    /// 执行）把它扳成 true 且不再落回。它管的是 worker 空闲轮那侧的持久积压
    /// （房间重算目标、模型单元）——那些行不按 dbnum 分，没法像队列行那样逐条挂起，
    /// 只能整体等一个「有人在干活了」的信号。批次侧的挂起是逐行的，两者互不替代。
    auto_work_armed: AtomicBool,
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
            // `startup_autorun=true` 就是历史行为：一上来就是上过弦的。
            auto_work_armed: AtomicBool::new(crate::options::startup_autorun()),
            notify: Notify::new(),
        })
    }

    /// 取队列锁，并从中毒中恢复。
    ///
    /// 队列不持久（ADR-011 §4），durable 语义在水位与 `model_update_pending` 表上，
    /// 所以中毒状态最坏是某一行元数据没更新完。而让每一次后续加锁都跟着 panic，
    /// 会把「一个批次挂了」放大成看门狗入队、队列面板、worker 全线连坐——偏偏
    /// `/health` 读的是 `AtomicBool`、不碰这把锁，外面还一直报 ok。
    fn queue(&self) -> MutexGuard<'_, QueueState> {
        self.inner.lock().unwrap_or_else(|poisoned| {
            log::error!("数据批次队列锁曾因 panic 中毒，已恢复继续使用");
            poisoned.into_inner()
        })
    }

    /// 把一次发现放进队列，并让注册表跟上（新行 / 并入 / 无动作）。
    ///
    /// 三种落点都会唤醒 worker：即便 `AlreadyCovered`，manual 触发的语义也是
    /// 「别等下一个 30s 轮询」——worker 醒来发现队列没变化也只亏一次空转。
    ///
    /// `hold` 见 [`batch_queue::DataBatch::held`]：重扫发现的行挂起，真实触发的
    /// 不挂起并顺带放行同 dbnum 的积压。真实触发同时给整个进程上弦
    /// （[`Self::arm_auto_work`]），空闲轮的持久积压也从那一刻起开始消化。
    pub fn enqueue(
        &self,
        registry: &TaskRegistry,
        found: &DiscoveredBatch,
        hold: bool,
    ) -> EnqueueOutcome {
        if !hold {
            self.arm_auto_work();
            InitializationCoordinator::global().arm();
        }
        let outcome = {
            let mut state = self.queue();
            let outcome = batch_queue::enqueue(
                &mut state.queue,
                found.dbnum,
                &found.db_type,
                found.applied_sesno,
                found.file_latest_sesno,
                found.previous_observed_sesno,
                hold,
                found.intent,
                found.phase,
                found.epoch_id,
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

            // 排队行缺席只有一条合法出口：`AlreadyCovered`——纯规则的 `covers` 守卫
            // 拦下了运行中批次冻结区间（或水位）已覆盖的触发。批次运行期间的重复执行、
            // 迟到的 watch 事件、只动 mtime 的重扫都落在这里，不是失步；回执带上
            // 运行行的 task_id 供对账。其余判定此刻必有排队行，真没有才是队列失步。
            // 这里持着锁，panic 会把锁毒掉、连累看门狗与面板，所以失步也只退回
            // 空回执并告警。
            let info = match queued_row {
                None => {
                    let task_id = if outcome == Enqueued::AlreadyCovered {
                        let running_task_id = state
                            .meta
                            .get(&(found.dbnum, true))
                            .map(|m| m.task_id.clone());
                        log::debug!(
                            "dbnum={} 的触发已被运行中批次冻结区间或水位覆盖，无需入队",
                            found.dbnum
                        );
                        running_task_id.unwrap_or_default()
                    } else {
                        log::error!(
                            "dbnum={} 入队判定为 {:?} 却找不到排队行，队列已失步",
                            found.dbnum,
                            outcome
                        );
                        String::new()
                    };
                    EnqueuedBatchInfo {
                        task_id,
                        dbnum: found.dbnum,
                        db_type: found.db_type.clone(),
                        phase: found.phase.as_str(),
                        epoch_id: found.epoch_id,
                        intent: found.intent.as_str(),
                        position,
                        start_sesno: found.applied_sesno + 1,
                        end_sesno: found.file_latest_sesno,
                    }
                }
                Some(row) => {
                    let merging = matches!(outcome, Enqueued::Merged | Enqueued::AlreadyCovered);
                    let existing = merging
                        .then(|| {
                            state
                                .meta
                                .get(&(found.dbnum, false))
                                .map(|m| m.task_id.clone())
                        })
                        .flatten();
                    let task_id = match existing {
                        Some(task_id) => {
                            // 文件可能在两次触发之间被挪动，执行按最后一次观察到的路径走。
                            if let Some(meta) = state.meta.get_mut(&(found.dbnum, false)) {
                                meta.path = found.path.clone();
                                meta.file_name = found.file_name.clone();
                            }
                            if outcome == Enqueued::Merged {
                                registry.replace_queued_range(
                                    &task_id,
                                    row.start_sesno,
                                    time_for(
                                        row.start_sesno,
                                        if found.intent == BatchIntent::Reinitialize {
                                            if found.file_latest_sesno == 0 { 0 } else { 1 }
                                        } else {
                                            found.applied_sesno + 1
                                        },
                                        &found.first_pending_sesno_time,
                                    ),
                                    row.end_sesno,
                                    time_for(
                                        row.end_sesno,
                                        found.file_latest_sesno,
                                        &found.file_latest_sesno_time,
                                    ),
                                );
                            }
                            task_id
                        }
                        None => {
                            if merging {
                                log::error!(
                                    "dbnum={} 有排队行却没有元数据，补建任务行",
                                    found.dbnum
                                );
                            }
                            let task_id = TaskRegistry::new_task_id("db");
                            registry.insert_queued_batch(
                                &task_id,
                                &found.project,
                                found.dbnum,
                                &found.db_type,
                                row.start_sesno,
                                time_for(
                                    row.start_sesno,
                                    found.applied_sesno + 1,
                                    &found.first_pending_sesno_time,
                                ),
                                row.end_sesno,
                                time_for(
                                    row.end_sesno,
                                    found.file_latest_sesno,
                                    &found.file_latest_sesno_time,
                                ),
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
                            task_id
                        }
                    };
                    EnqueuedBatchInfo {
                        task_id,
                        dbnum: found.dbnum,
                        db_type: found.db_type.clone(),
                        phase: row.phase.as_str(),
                        epoch_id: row.epoch_id,
                        intent: row.intent.as_str(),
                        position,
                        start_sesno: row.start_sesno,
                        end_sesno: row.end_sesno,
                    }
                }
            };
            EnqueueOutcome { outcome, info }
        };
        // 阶段闸的当下裁定跟着入队一起记。入队成功不等于会被执行：屏障是按
        // **阶段**关的，别的项目的一处身份歧义就能让本行永远停在 queued，而回执里
        // 只写着「已覆盖」。2026-08-17 的 7998 就是这么消失的，追踪必须把这两件事
        // 摆在同一条记录里，否则读的人还得自己去 /health 拼。
        // 协调器的 allows/snapshot 取证在闭包里：未启用追踪时一把锁都不多拿。
        crate::data_interface::debug_scope::trace(
            crate::data_interface::debug_scope::TracePoint::Enqueue,
            found.dbnum,
            || {
                let phase_admits =
                    InitializationCoordinator::global().allows(found.phase, found.epoch_id);
                let snapshot = InitializationCoordinator::global().snapshot();
                serde_json::json!({
                    "origin": "scheduler",
                    "task_id": outcome.info.task_id,
                    "outcome": format!("{:?}", outcome.outcome),
                    "held": hold,
                    "intent": outcome.info.intent,
                    "phase": outcome.info.phase,
                    "epoch_id": outcome.info.epoch_id,
                    "position": outcome.info.position,
                    "start_sesno": outcome.info.start_sesno,
                    "end_sesno": outcome.info.end_sesno,
                    "previous_observed_sesno": found.previous_observed_sesno,
                    "phase_admits_dispatch": phase_admits,
                    "initialization_status": snapshot.status,
                    "current_phase": snapshot.current_phase,
                    "blockers": snapshot.blockers,
                })
            },
        );
        self.notify.notify_one();
        outcome
    }

    /// FIFO 出队并冻结（暂停时恒 None）。注册表行随之转 running。
    pub fn freeze_next(&self, registry: &TaskRegistry) -> Option<FrozenBatch> {
        match self.next_dispatch(registry, true, |_| false) {
            DispatchOutcome::Frozen { job, .. } => Some(job),
            DispatchOutcome::HeadNeedsExclusive | DispatchOutcome::Idle => None,
        }
    }

    /// 并发口径的出队并冻结（ADR-011 2026-08-09 修订）。
    ///
    /// 规则在 [`batch_queue::freeze_next_concurrent`]：同 dbnum 恒串行、独占批次
    /// 保住 FIFO 位置。`is_exclusive` 由调用方（batch_worker）按 db_type / 基线 /
    /// 应急直写判定，队列层不掺业务口径。
    pub fn next_dispatch(
        &self,
        registry: &TaskRegistry,
        in_flight_empty: bool,
        is_exclusive: impl Fn(&batch_queue::DataBatch) -> bool,
    ) -> DispatchOutcome {
        let mut state = self.queue();
        let paused = self.paused.load(Ordering::SeqCst);
        let index = match batch_queue::freeze_next_concurrent(
            &mut state.queue,
            paused,
            in_flight_empty,
            &is_exclusive,
            |batch| {
                batch.epoch_id == 0
                    || InitializationCoordinator::global().allows(batch.phase, batch.epoch_id)
            },
        ) {
            batch_queue::NextDispatch::Freeze(index) => index,
            batch_queue::NextDispatch::HeadNeedsExclusive => {
                return DispatchOutcome::HeadNeedsExclusive;
            }
            batch_queue::NextDispatch::Idle => return DispatchOutcome::Idle,
        };
        let row = state.queue[index].clone();
        let exclusive = is_exclusive(&row);
        // 元数据缺失说明队列失步。此处持着锁，panic 会毒掉锁把整条链连坐，
        // 而这一行既然没有文件路径也就无从执行——把它摘掉、报错、让下一条上。
        let Some(meta) = state.meta.remove(&(row.dbnum, false)) else {
            log::error!("dbnum={} 被冻结却没有排队元数据，丢弃该行", row.dbnum);
            state.queue.remove(index);
            return DispatchOutcome::Idle;
        };
        state.meta.insert((row.dbnum, true), meta.clone());
        registry.mark_started(&meta.task_id);
        let epoch_id = row.epoch_id;
        let outcome = DispatchOutcome::Frozen {
            job: FrozenBatch {
                task_id: meta.task_id,
                project: meta.project,
                dbnum: row.dbnum,
                db_type: row.db_type,
                phase: row.phase,
                epoch_id: row.epoch_id,
                intent: row.intent,
                path: meta.path,
                file_name: meta.file_name,
                start_sesno: row.start_sesno,
                end_sesno: row.end_sesno,
                previous_observed_sesno: row.previous_observed_sesno,
            },
            exclusive,
        };
        let pending = state
            .queue
            .iter()
            .filter(|batch| batch.epoch_id == epoch_id)
            .map(|batch| (batch.phase, batch.state == BatchState::Running))
            .collect::<Vec<_>>();
        drop(state);
        InitializationCoordinator::global().reconcile_pending(epoch_id, pending);
        outcome
    }

    /// 批次执行完毕：把运行中的那行从队列里摘掉（终态只留在注册表历史里）。
    pub fn finish(&self, dbnum: u32) {
        let mut state = self.queue();
        state
            .queue
            .retain(|b| !(b.dbnum == dbnum && b.state == BatchState::Running));
        state.meta.remove(&(dbnum, true));
        let epoch_id = InitializationCoordinator::global().snapshot().epoch_id;
        let pending = state
            .queue
            .iter()
            .filter(|batch| batch.epoch_id == epoch_id)
            .map(|batch| (batch.phase, batch.state == BatchState::Running))
            .collect::<Vec<_>>();
        drop(state);
        InitializationCoordinator::global().reconcile_pending(epoch_id, pending);
    }

    /// 冻结点重扫定下真实上界之后，把它回写到运行中的队列行与任务行。
    ///
    /// ADR-011 §5 把冻结点定义为「执行真正开始之前」的那次重扫，而排队期间显示的
    /// 右端只是**入队时观察到的预期上界**——两次触发之间文件还在长。不回写会有两个
    /// 后果：面板上显示的区间比实际应用的窄；以及紧接着排在后面那条的左端
    /// （`running_end + 1`）建在一个过时的数上。
    /// `end_sesno_time` 是这个真实上界那条保存的 E3D 写入时刻；读不到就传 `None`，
    /// 那一格会空着——冻结把序号改了，入队时那个旧时刻立刻就是错的，不能留着。
    pub fn record_frozen_end(
        &self,
        registry: &TaskRegistry,
        dbnum: u32,
        end_sesno: i32,
        end_sesno_time: Option<String>,
    ) {
        let (task_id, absorbed_task_id, shifted_task_id) = {
            let mut state = self.queue();
            let changed = match state
                .queue
                .iter_mut()
                .find(|b| b.dbnum == dbnum && b.state == BatchState::Running)
            {
                Some(row) if row.end_sesno != end_sesno => {
                    row.end_sesno = end_sesno;
                    true
                }
                _ => false,
            };
            if !changed {
                return;
            }
            let task_id = state.meta.get(&(dbnum, true)).map(|m| m.task_id.clone());
            let mut absorbed_task_id = None;
            let mut shifted_task_id = None;
            if let Some(index) = state
                .queue
                .iter()
                .position(|b| b.dbnum == dbnum && b.state == BatchState::Queued)
            {
                if state.queue[index].intent == BatchIntent::Reinitialize {
                    // 控制意图必须真正到达下一冻结点；运行批次的数值覆盖不能销掉它。
                } else if state.queue[index].end_sesno <= end_sesno {
                    state.queue.remove(index);
                    absorbed_task_id = state.meta.remove(&(dbnum, false)).map(|m| m.task_id);
                } else if state.queue[index].start_sesno <= end_sesno {
                    state.queue[index].start_sesno = end_sesno + 1;
                    shifted_task_id = state.meta.get(&(dbnum, false)).map(|m| m.task_id.clone());
                }
            }
            (task_id, absorbed_task_id, shifted_task_id)
        };
        if let Some(task_id) = task_id {
            registry.set_frozen_range(&task_id, end_sesno, end_sesno_time);
        }
        if let Some(task_id) = shifted_task_id {
            // 后继行的左端变成 `end_sesno + 1`，那条保存的时刻这里读不到（要开文件），
            // 而它下一轮被并入时会连同时刻一起刷新。空着比留一个上一任左端的时刻好。
            registry.set_queued_start(&task_id, end_sesno + 1, None);
        }
        if let Some(task_id) = absorbed_task_id {
            let result = serde_json::json!({ "status": "absorbed_by_running" });
            registry.finish(&task_id, TaskState::Succeeded, result.clone());
            #[cfg(feature = "http_api")]
            crate::web_service::events::publish(
                crate::web_service::events::Topic::Tasks,
                "task_finished",
                Some(task_id.clone()),
                serde_json::json!({
                    "task_id": task_id,
                    "state": TaskState::Succeeded.as_str(),
                    "result": result,
                }),
            );
        }
    }

    /// 唤醒 worker 消化一轮（不入队任何东西）。
    ///
    /// 给绕过队列、直接改 `model_update_pending` 表的入口用（如死信人工复活）：
    /// 不叫醒的话，复活的行要等 `IDLE_WAKE` 兜底轮询才被捡走，最多晚 30 秒。
    pub fn wake(&self) {
        self.notify.notify_one();
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

    /// 本进程是否已被真实触发上过弦（见 [`Self::auto_work_armed`] 字段说明）。
    pub fn is_auto_work_armed(&self) -> bool {
        self.auto_work_armed.load(Ordering::SeqCst)
    }

    /// 上弦并唤醒 worker：有人真的动了数据，空闲轮那侧的积压可以开始收了。
    ///
    /// 只进不退，且不落库。它描述的是「本进程这一趟里发生过真实增量」，重启后
    /// 本来就该回到「等下一次触发」——这正是 `startup_autorun=false` 的用意，
    /// 不是需要跨重启保留的操作意图（那是 `queue_control:main` 管的暂停）。
    pub fn arm_auto_work(&self) {
        if !self.auto_work_armed.swap(true, Ordering::SeqCst) {
            println!("检测到真实增量触发：本进程开始消化持久积压（房间重算 / 模型单元）");
            self.notify.notify_one();
        }
    }

    /// 等新工作（入队 / 恢复）或超时。超时兜底轮询：唤醒丢失也只是晚一拍。
    pub async fn wait_for_work(&self, timeout: Duration) {
        let _ = tokio::time::timeout(timeout, self.notify.notified()).await;
    }

    /// 队列快照（含运行中行），按队列序。
    pub fn snapshot(&self) -> Vec<QueueRow> {
        let state = self.queue();
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
                    phase: b.phase.as_str(),
                    epoch_id: b.epoch_id,
                    blocked_by_phase: (b.epoch_id != 0
                        && !InitializationCoordinator::global().allows(b.phase, b.epoch_id))
                    .then(|| {
                        InitializationCoordinator::global()
                            .snapshot()
                            .current_phase
                            .unwrap_or("manifest")
                    }),
                    intent: b.intent.as_str(),
                    // 挂起行单列一个状态，不混进 queued：它不会自己往前走，
                    // 显示成排队会让人以为消费者卡住了，而那是完全不同的故障。
                    state: match (running, b.held) {
                        (true, _) => "running",
                        (false, true) => "held",
                        (false, false) => "queued",
                    },
                    start_sesno: b.start_sesno,
                    end_sesno: b.end_sesno,
                }
            })
            .collect()
    }
}

#[cfg(test)]
impl BatchScheduler {
    /// 测试里的「真实触发」入队：不挂起，与 watch 事件 / 人工执行同口径。
    fn enqueue_live(&self, registry: &TaskRegistry, found: &DiscoveredBatch) -> EnqueueOutcome {
        self.enqueue(registry, found, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(dbnum: u32, applied: i32, latest: i32) -> DiscoveredBatch {
        // 时刻用「sesno 的分钟数」编出来，好在断言里一眼认出它贴到了哪条保存上。
        let at = |sesno: i32| Some(format!("2026-08-07T10:{:02}:00+08:00", sesno % 60));
        DiscoveredBatch {
            project: "P".into(),
            dbnum,
            db_type: "DESI".into(),
            phase: DataPhase::Design,
            epoch_id: 0,
            intent: BatchIntent::ApplyWindow,
            path: PathBuf::from(format!("D:/proj/db{dbnum}")),
            file_name: format!("db{dbnum}"),
            applied_sesno: applied,
            file_latest_sesno: latest,
            // 与 batch_queue 的测试助手同一约定：基线拿水位顶替。
            previous_observed_sesno: applied,
            first_pending_sesno_time: at(applied + 1),
            file_latest_sesno_time: at(latest),
        }
    }

    fn reinit_found(dbnum: u32, applied: i32, latest: i32) -> DiscoveredBatch {
        DiscoveredBatch {
            intent: BatchIntent::Reinitialize,
            ..found(dbnum, applied, latest)
        }
    }

    #[test]
    fn reinitialize_intent_is_visible_and_not_absorbed_by_a_running_row() {
        let (scheduler, registry) = fresh();
        scheduler.enqueue_live(&registry, &found(7997, 0, 10));
        scheduler.freeze_next(&registry).expect("running row");
        let successor = scheduler.enqueue_live(&registry, &reinit_found(7997, 42, 0));
        assert_eq!(successor.outcome, Enqueued::BehindRunning);
        assert_eq!(successor.info.intent, "reinitialize");
        scheduler.record_frozen_end(&registry, 7997, 12, None);
        let rows = scheduler.snapshot();
        assert_eq!(rows.len(), 2, "重建后继不得按会话号被运行行吸收");
        assert_eq!(rows[1].intent, "reinitialize");
        assert_eq!((rows[1].start_sesno, rows[1].end_sesno), (0, 0));
    }

    fn fresh() -> (BatchScheduler, TaskRegistry) {
        (
            BatchScheduler {
                inner: Mutex::new(QueueState::default()),
                paused: AtomicBool::new(false),
                // 下面的性质与冷启动挂起无关，起手就当作已上弦；挂起那条路径由
                // `batch_queue` 的纯规则测试与本模块末尾几条单独覆盖。
                auto_work_armed: AtomicBool::new(true),
                notify: Notify::new(),
            },
            TaskRegistry::default(),
        )
    }

    #[test]
    fn enqueue_creates_a_queued_task_row_linked_to_the_batch() {
        let (scheduler, registry) = fresh();
        let outcome = scheduler.enqueue_live(&registry, &found(7997, 1023, 1034));
        assert_eq!(outcome.outcome, Enqueued::New);
        assert_eq!(outcome.info.position, 1);
        assert_eq!(outcome.info.start_sesno, 1024);

        let entry = registry.get(&outcome.info.task_id).expect("任务行已建");
        assert_eq!(entry.state.as_str(), "queued");
        assert_eq!(entry.dbnum, Some(7997));
        assert_eq!(entry.end_sesno, Some(1034));
    }

    /// 入队时冻结的基线要原样走完「发现 → 队列行 → 冻结快照」全程，合并只认最早
    /// 那一次——worker 拿到的 `FrozenBatch` 就是它计算 `merged_sesnos` 的唯一依据。
    #[test]
    fn the_frozen_job_carries_the_earliest_enqueue_baseline() {
        let (scheduler, registry) = fresh();
        let mut first = found(7997, 1023, 1034);
        first.previous_observed_sesno = 1030;
        scheduler.enqueue_live(&registry, &first);
        // 排队期间的第二次触发：它的「上一次观察」已被首次入队扫描推到 1034。
        let mut second = found(7997, 1023, 1041);
        second.previous_observed_sesno = 1034;
        scheduler.enqueue_live(&registry, &second);

        let job = scheduler.freeze_next(&registry).expect("有排队项");
        assert_eq!((job.start_sesno, job.end_sesno), (1024, 1041));
        assert_eq!(
            job.previous_observed_sesno, 1030,
            "冻结快照带的必须是最早那次观察，不是合并触发的"
        );
    }

    #[test]
    fn merge_updates_the_same_task_row_instead_of_adding_one() {
        let (scheduler, registry) = fresh();
        let first = scheduler.enqueue_live(&registry, &found(7997, 1023, 1034));
        let second = scheduler.enqueue_live(&registry, &found(7997, 1023, 1041));
        assert_eq!(second.outcome, Enqueued::Merged);
        assert_eq!(
            second.info.task_id, first.info.task_id,
            "合并不该另开任务行"
        );
        assert_eq!(
            registry.get(&first.info.task_id).unwrap().end_sesno,
            Some(1041)
        );
        assert_eq!(scheduler.snapshot().len(), 1);
    }

    #[test]
    fn freeze_marks_the_task_running_and_finish_removes_the_row() {
        let (scheduler, registry) = fresh();
        let queued = scheduler.enqueue_live(&registry, &found(7997, 1023, 1038));
        let job = scheduler.freeze_next(&registry).expect("有排队项");
        assert_eq!(job.task_id, queued.info.task_id);
        assert_eq!(job.end_sesno, 1038, "冻结时区间已定死");
        assert_eq!(
            registry.get(&job.task_id).unwrap().state.as_str(),
            "running"
        );

        // 运行期间新保存：另起一行接在右端之后（同 dbnum 两行）。
        let behind = scheduler.enqueue_live(&registry, &found(7997, 1023, 1041));
        assert_eq!(behind.outcome, Enqueued::BehindRunning);
        assert_eq!(behind.info.start_sesno, 1039);
        assert_eq!(scheduler.snapshot().len(), 2);

        scheduler.finish(job.dbnum);
        let rows = scheduler.snapshot();
        assert_eq!(rows.len(), 1, "终态行不留在队列里");
        assert_eq!(rows[0].state, "queued");
    }

    /// 批次运行中、无新保存时的重复触发（再点一次执行 / 迟到的 watch 事件 /
    /// 只动 mtime 的重扫）：`covers` 守卫判 AlreadyCovered 且不产生排队行——
    /// 这是纯规则的合法出口，不是失步；回执要对到运行中的任务行，且不多排行。
    #[test]
    fn a_trigger_covered_by_the_running_batch_maps_to_its_task_row() {
        let (scheduler, registry) = fresh();
        scheduler.enqueue_live(&registry, &found(7997, 1023, 1038));
        let job = scheduler.freeze_next(&registry).expect("有排队项");

        let repeat = scheduler.enqueue_live(&registry, &found(7997, 1023, 1038));
        assert_eq!(repeat.outcome, Enqueued::AlreadyCovered);
        assert_eq!(repeat.info.task_id, job.task_id, "回执应对到运行中的任务行");
        assert_eq!(scheduler.snapshot().len(), 1, "不该因重复触发多排一行");
    }

    /// 挂起行在 `/queue` 上必须是 `held` 而不是 `queued`，被放行后才转回 `queued`。
    ///
    /// 界面读的就是这个字段。显示成 `queued` 的话，一条永远不动的行与「消费者卡住了」
    /// 长得一模一样——而后者是要立刻叫人的故障，前者是本来就该这样。
    #[test]
    fn a_held_row_says_so_in_the_queue_snapshot() {
        let (scheduler, registry) = fresh();
        scheduler.enqueue(&registry, &found(7997, 102, 132), true);
        let rows = scheduler.snapshot();
        assert_eq!(rows.len(), 1, "挂起行照样入队占位");
        assert_eq!(rows[0].state, "held");
        assert!(scheduler.freeze_next(&registry).is_none(), "挂起行不出队");

        scheduler.enqueue_live(&registry, &found(7997, 102, 133));
        assert_eq!(scheduler.snapshot()[0].state, "queued", "真实触发放行了它");
        let job = scheduler.freeze_next(&registry).expect("放行后可出队");
        assert_eq!(
            (job.start_sesno, job.end_sesno),
            (103, 133),
            "积压与新会话合成一条一起跑"
        );
    }

    /// 上弦只由真实触发扳动，重扫入队多少行都不算。
    #[test]
    fn only_a_real_trigger_arms_the_process() {
        let (scheduler, registry) = fresh();
        scheduler.auto_work_armed.store(false, Ordering::SeqCst);
        scheduler.enqueue(&registry, &found(7997, 102, 132), true);
        assert!(!scheduler.is_auto_work_armed(), "重扫不上弦");
        scheduler.enqueue_live(&registry, &found(8000, 34, 40));
        assert!(scheduler.is_auto_work_armed(), "真实触发上弦");
    }

    #[test]
    fn pausing_blocks_freeze_but_not_enqueue() {
        let (scheduler, registry) = fresh();
        scheduler.enqueue_live(&registry, &found(7997, 0, 10));
        scheduler.pause();
        assert!(scheduler.freeze_next(&registry).is_none(), "暂停期间不出队");
        let outcome = scheduler.enqueue_live(&registry, &found(7997, 0, 12));
        assert_eq!(outcome.outcome, Enqueued::Merged, "暂停挡的是出队不是入队");
        scheduler.resume();
        assert!(scheduler.freeze_next(&registry).is_some());
    }

    #[test]
    fn positions_count_queued_rows_only() {
        let (scheduler, registry) = fresh();
        scheduler.enqueue_live(&registry, &found(1, 0, 5));
        scheduler.freeze_next(&registry).unwrap();
        let second = scheduler.enqueue_live(&registry, &found(2, 0, 5));
        let third = scheduler.enqueue_live(&registry, &found(3, 0, 5));
        assert_eq!(second.info.position, 1, "运行中的行不占排队位置");
        assert_eq!(third.info.position, 2);
    }

    #[test]
    fn frozen_rescan_recomputes_the_successor_under_the_scheduler_lock() {
        let (scheduler, registry) = fresh();
        scheduler.enqueue_live(&registry, &found(7997, 0, 10));
        scheduler.freeze_next(&registry).unwrap();
        let absorbed = scheduler.enqueue_live(&registry, &found(7997, 0, 12));
        scheduler.record_frozen_end(&registry, 7997, 12, None);
        assert_eq!(scheduler.snapshot().len(), 1);
        let entry = registry.get(&absorbed.info.task_id).unwrap();
        assert_eq!(entry.state, TaskState::Succeeded);
        assert_eq!(entry.result.unwrap()["status"], "absorbed_by_running");

        let (scheduler, registry) = fresh();
        scheduler.enqueue_live(&registry, &found(7997, 0, 10));
        scheduler.freeze_next(&registry).unwrap();
        let shifted = scheduler.enqueue_live(&registry, &found(7997, 0, 15));
        scheduler.record_frozen_end(&registry, 7997, 12, None);
        let rows = scheduler.snapshot();
        assert_eq!((rows[1].start_sesno, rows[1].end_sesno), (13, 15));
        let shifted_entry = registry.get(&shifted.info.task_id).unwrap();
        assert_eq!(shifted_entry.start_sesno, Some(13));
        assert!(
            shifted_entry.start_sesno_time.is_none(),
            "后继行的左端被推到 13，原来那个属于 sesno 1 的时刻必须一起清掉"
        );
    }

    /// 入队时贴上去的时刻必须真的属于队列行的那两个端点。
    ///
    /// 排在运行批次之后的那条，左端是 `running_end + 1` 而不是这次发现的水位 + 1——
    /// 照着 `found` 里的时刻直接贴，就会把一条别的保存的时刻写在这一行上。
    #[test]
    fn a_window_time_is_only_attached_when_the_endpoint_matches() {
        let (scheduler, registry) = fresh();
        let first = scheduler.enqueue_live(&registry, &found(7997, 0, 10));
        let entry = registry.get(&first.info.task_id).unwrap();
        assert_eq!(
            (entry.start_sesno, entry.end_sesno),
            (Some(1), Some(10)),
            "第一条的两端就是这次发现的两端"
        );
        assert!(entry.start_sesno_time.is_some() && entry.end_sesno_time.is_some());

        // 冻结之后再触发：新行排在运行批次后面，左端是 11，而 `found` 手里的左端
        // 时刻描述的是 sesno 1 那条保存——端点对不上，这一格只能空着。
        scheduler.freeze_next(&registry).unwrap();
        let behind = scheduler.enqueue_live(&registry, &found(7997, 0, 15));
        let entry = registry.get(&behind.info.task_id).unwrap();
        assert_eq!((entry.start_sesno, entry.end_sesno), (Some(11), Some(15)));
        assert!(
            entry.start_sesno_time.is_none(),
            "左端是 11 而手里的时刻属于 sesno 1，不许贴上去"
        );
        assert!(
            entry.end_sesno_time.is_some(),
            "右端 15 与发现的右端一致，时刻照贴"
        );
    }
}
