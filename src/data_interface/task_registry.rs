//! 进程级任务注册表：队列与任务的 UI 视图（ADR-011 §3 / §11）。
//!
//! 从 `web_service::tasks` 搬到 feature 无关层：`web_service` 整个在
//! `http_api` 门后，而合流后的队列消费者（单 worker）不分编译形态都要写
//! 任务状态——队列真身只能有一份，不能随 feature 分叉（rollout 第九节第 4 条）。
//!
//! durable 语义仍由 `applied_sesno` 水位与 `model_update_pending` 表承担；
//! 本表仅内存、重启即清空，重启后由 `init_watcher` 重扫水位把队列重建出来
//! （ADR-011 §4——界面必须说得出「这是重建的队列」）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use chrono::Local;
use indexmap::IndexMap;
use serde::Serialize;

/// 一个数据批次（dbnum × 会话区间）的任务行。
pub const TASK_KIND_DATA_BATCH: &str = "data_batch";
/// 一页持久模型工作单的消费尝试。真值仍在 `model_update_pending` 表；这行只供观察。
pub const TASK_KIND_MODEL_DRAIN: &str = "model_drain";
/// 人工触发的指定 dbnum 全量模型重建；工作仍落入同一 durable 模型队列。
pub const TASK_KIND_MODEL_REBUILD: &str = "model_rebuild";
/// 一轮房间归属收敛（ADR-011 §10：与数据批次同构的一种 kind）。
pub const TASK_KIND_ROOM_RECALC: &str = "room_recalc";

/// 分层保留的兜底上限（ADR-011 §11 + rollout 第九节第 8 条）：
/// 首轮放宽 `manual_db_nums` 后 287 条排队 + 287 条终态就要 ≥574，
/// 200 差了一个量级；1000 = 574 打底 + 全局最近终态的余量。
const MAX_TASKS: usize = 1000;

/// 任务状态机：`queued -> running -> succeeded | partial | failed`；冻结重扫完全覆盖
/// 后继行时，该后继按 ADR-011 §5 直接 `queued -> succeeded`（`absorbed_by_running`）。
///
/// `queued` 随 ADR-011 §3 引入——数据批次在队列里排队时就要有一行可看。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    Succeeded,
    Partial,
    /// 初始化门或数据优先级在本页中途关闭；未执行的持久行原样保留。
    Yielded,
    Failed,
}

impl TaskState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
            Self::Yielded => "yielded",
            Self::Failed => "failed",
        }
    }

    /// 终态才可被容量剔除；queued / running 永不剔除（ADR-011 §11）。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Partial | Self::Yielded | Self::Failed
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskEntry {
    pub task_id: String,
    pub kind: &'static str,
    pub state: TaskState,
    pub project: String,
    /// 入队时刻。合流后它的语义不再是开跑时刻——「已排」与「已用」是两个起点
    /// （rollout 第二节第 2 项），开跑时刻见 [`Self::started_at`]。
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// 数据批次的库号（ADR-011 §3：队列行必须自带，它是排序键也是合并键）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dbnum: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db_type: Option<String>,
    /// 会话区间左端（入队时的水位 + 1）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_sesno: Option<i32>,
    /// 会话区间右端。排队中会被后来的触发推高（并入会话），冻结后不再变。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_sesno: Option<i32>,
    /// 区间左端那条保存在 E3D 里的写入时刻（RFC3339，ADR-020 第 2 项那把尺子）。
    ///
    /// 界面的「保存窗口」列显示的是这一对时刻而不是 sesno（plant-ui ADR-0019）；
    /// 序号仍是执行边界，时刻只是显示代理。读不到 → `None` → 那一格**留空**，
    /// 不许回落成 sesno，也不许拿挂钟时刻顶替。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_sesno_time: Option<String>,
    /// 区间右端那条保存的写入时刻。
    ///
    /// **它必须与 `end_sesno` 同生共死**：排队中被并入推高右端时一起刷新，冻结点重扫
    /// 定下真实上界时一起改写。只推序号不刷时刻的话，窗口会停在入队观察到的那一刻——
    /// 并入得越多，界面上这个时刻越骗人。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_sesno_time: Option<String>,
    /// 阶段二进度：本批次已生成的交付单元数（口径按数据批次，ADR-0007 迁移）。
    /// 房间轮任务复用同一对字段记 done/total。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units_done: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_units: Option<u32>,
    /// 该任务累计广播过的进度事件数（重连后前端用于对齐，见 spec §5.4）。
    pub events_seen: u64,
    /// 当前执行子阶段。数据批次使用稳定英文值，供 `/tasks` 与 `/health` 对账。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_last_progress_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_dbnum: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_refnos_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_refnos_parsed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_refnos_missing: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stall_deadline: Option<String>,
    /// kind 专属的详情（如房间轮的 `{panels, elements, dead_letters}`，
    /// ADR-011 §10）；建行时写入，与终态 `result` 互不替代。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
    /// 终态结果 JSON；queued / running 时为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

/// 进程启动时刻（RFC3339）。`ensure_batch_worker` 启动时触发初始化，因此它
/// 锚定的是服务真正起来的时间，而不是第一次被问到的时间——「队列已重建，
/// 排队时长从重启起算」那句话靠它才说得出来（ADR-011 §4）。
static PROCESS_STARTED_AT: OnceLock<String> = OnceLock::new();

pub fn process_started_at() -> &'static str {
    PROCESS_STARTED_AT.get_or_init(|| Local::now().to_rfc3339())
}

/// 插入序即时间序的注册表。
///
/// 容量剔除按三条规则（ADR-011 §11），顺序即优先级：
/// 1. queued 与 running 永不剔除；
/// 2. 每个 dbnum 保留最近一条终态（先剔「同 dbnum 有更新终态」的旧终态）；
/// 3. 剩余容量给全局最近若干条（最老的终态先走）。
#[derive(Default)]
pub struct TaskRegistry {
    inner: Mutex<IndexMap<String, TaskEntry>>,
}

static REGISTRY: OnceLock<TaskRegistry> = OnceLock::new();

/// 任务序号，进程内单调递增。
///
/// 曾经的后缀是 `rand::random::<u16>()`，而时间戳只到秒——`init_watcher` 的重扫
/// 在一个紧循环里逐个 dbnum 建行，整批都落在同一秒。放宽 `manual_db_nums` 后有
/// 287 个库要排队（rollout 六之二实测），16 位随机在 287 条下的生日碰撞概率约
/// 47%；而 `insert_entry` 是按 task_id 覆盖的，撞上就是一整行任务凭空消失、
/// 两个库的进度打在同一行上。序号把这个概率问题变成不可能。
static NEXT_TASK_SEQ: AtomicU64 = AtomicU64::new(0);

impl TaskRegistry {
    /// 进程级单例：worker（feature 无关）与 web_service（`http_api` 门内）
    /// 共用同一份，队列真身不随编译形态分叉。
    pub fn global() -> &'static TaskRegistry {
        REGISTRY.get_or_init(TaskRegistry::default)
    }

    /// 取表锁，并从中毒中恢复。
    ///
    /// 注册表只是一份 UI 视图，durable 语义在水位与 `model_update_pending` 表上。
    /// 持锁者 panic 之后让每一次后续访问都跟着 panic，会把「一个批次挂了」放大成
    /// 「队列面板、看门狗入队、HTTP 全线瘫痪」，而 `/health` 还在报 ok。中毒的
    /// 数据最坏是某一行状态没更新完，远好过连坐。
    fn entries(&self) -> MutexGuard<'_, IndexMap<String, TaskEntry>> {
        self.inner.lock().unwrap_or_else(|poisoned| {
            log::error!("任务注册表锁曾因 panic 中毒，已恢复继续使用");
            poisoned.into_inner()
        })
    }

    pub fn new_task_id(prefix: &str) -> String {
        format!(
            "{}-{}-{:06}",
            prefix,
            Local::now().format("%Y%m%d-%H%M%S"),
            NEXT_TASK_SEQ.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// 新排一条数据批次（state = queued）。返回该行 task_id。
    ///
    /// 两个时刻参数各自紧跟自己的 sesno：类型不同，写反了编译期就挡住。
    pub fn insert_queued_batch(
        &self,
        task_id: &str,
        project: &str,
        dbnum: u32,
        db_type: &str,
        start_sesno: i32,
        start_sesno_time: Option<String>,
        end_sesno: i32,
        end_sesno_time: Option<String>,
    ) {
        self.insert_entry(TaskEntry {
            task_id: task_id.to_string(),
            kind: TASK_KIND_DATA_BATCH,
            state: TaskState::Queued,
            project: project.to_string(),
            created_at: Local::now().to_rfc3339(),
            started_at: None,
            finished_at: None,
            dbnum: Some(dbnum),
            db_type: Some(db_type.to_string()),
            start_sesno: Some(start_sesno),
            end_sesno: Some(end_sesno),
            start_sesno_time,
            end_sesno_time,
            units_done: None,
            total_units: None,
            events_seen: 0,
            current_stage: None,
            stage_started_at: None,
            stage_last_progress_at: None,
            dependency_dbnum: None,
            dependency_path: None,
            dependency_refnos_total: None,
            dependency_refnos_parsed: None,
            dependency_refnos_missing: None,
            stall_deadline: None,
            detail: None,
            result: None,
        });
    }

    /// 新排一条房间收敛轮（房间轮不排队，创建即 running；ADR-011 §10）。
    ///
    /// `detail` 携带本轮的分项计数（面板 / 构件 / 死信），随任务详情带出。
    pub fn insert_running_room_round(
        &self,
        task_id: &str,
        project: &str,
        total: u32,
        detail: serde_json::Value,
    ) {
        let now = Local::now().to_rfc3339();
        self.insert_entry(TaskEntry {
            task_id: task_id.to_string(),
            kind: TASK_KIND_ROOM_RECALC,
            state: TaskState::Running,
            project: project.to_string(),
            created_at: now.clone(),
            started_at: Some(now),
            finished_at: None,
            dbnum: None,
            db_type: None,
            start_sesno: None,
            end_sesno: None,
            start_sesno_time: None,
            end_sesno_time: None,
            units_done: Some(0),
            total_units: Some(total),
            events_seen: 0,
            current_stage: None,
            stage_started_at: None,
            stage_last_progress_at: None,
            dependency_dbnum: None,
            dependency_path: None,
            dependency_refnos_total: None,
            dependency_refnos_parsed: None,
            dependency_refnos_missing: None,
            stall_deadline: None,
            detail: Some(detail),
            result: None,
        });
    }

    /// 建一条模型消费页。它不是第二份队列：重启恢复只看 durable pending，
    /// TaskRegistry 仅把本次认领的 epoch、来源与根暴露给 REST/Python/Plant UI。
    pub fn insert_running_model_drain(
        &self,
        task_id: &str,
        project: &str,
        total: u32,
        detail: serde_json::Value,
    ) {
        let now = Local::now().to_rfc3339();
        self.insert_entry(TaskEntry {
            task_id: task_id.to_string(),
            kind: TASK_KIND_MODEL_DRAIN,
            state: TaskState::Running,
            project: project.to_string(),
            created_at: now.clone(),
            started_at: Some(now),
            finished_at: None,
            dbnum: None,
            db_type: None,
            start_sesno: None,
            end_sesno: None,
            start_sesno_time: None,
            end_sesno_time: None,
            units_done: Some(0),
            total_units: Some(total),
            events_seen: 0,
            current_stage: None,
            stage_started_at: None,
            stage_last_progress_at: None,
            dependency_dbnum: None,
            dependency_path: None,
            dependency_refnos_total: None,
            dependency_refnos_parsed: None,
            dependency_refnos_missing: None,
            stall_deadline: None,
            detail: Some(detail),
            result: None,
        });
    }

    pub fn insert_running_model_rebuild(
        &self,
        task_id: &str,
        project: &str,
        dbnum: u32,
        total: u32,
        detail: serde_json::Value,
    ) {
        let now = Local::now().to_rfc3339();
        self.insert_entry(TaskEntry {
            task_id: task_id.to_string(),
            kind: TASK_KIND_MODEL_REBUILD,
            state: TaskState::Running,
            project: project.to_string(),
            created_at: now.clone(),
            started_at: Some(now),
            finished_at: None,
            dbnum: Some(dbnum),
            db_type: Some("DESI".to_string()),
            start_sesno: None,
            end_sesno: None,
            start_sesno_time: None,
            end_sesno_time: None,
            units_done: Some(0),
            total_units: Some(total),
            events_seen: 0,
            current_stage: Some("coverage_scan".to_string()),
            stage_started_at: Some(Local::now().to_rfc3339()),
            stage_last_progress_at: None,
            dependency_dbnum: None,
            dependency_path: None,
            dependency_refnos_total: None,
            dependency_refnos_parsed: None,
            dependency_refnos_missing: None,
            stall_deadline: None,
            detail: Some(detail),
            result: None,
        });
    }

    pub fn set_units_done(&self, task_id: &str, done: u32) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            if entry.units_done != Some(done) {
                entry.units_done = Some(done);
                entry.stage_last_progress_at = Some(Local::now().to_rfc3339());
                entry.events_seen = entry.events_seen.saturating_add(1);
            }
        }
    }

    fn insert_entry(&self, entry: TaskEntry) {
        let mut inner = self.entries();
        if inner.len() >= MAX_TASKS {
            Self::evict_one(&mut inner);
        }
        // `new_task_id` 之后撞键已不可能，真撞上就是编程错误。而 `IndexMap::insert`
        // 会把旧行连同它的进度与终态一并吞掉，界面上表现为一条任务凭空消失——
        // 这种事必须吵出来，不能靠人事后从「怎么少了一行」倒推。
        if let Some(existing) = inner.get(&entry.task_id) {
            log::error!(
                "task_id 撞键 {}：既有行(kind={} state={} dbnum={:?})将被新行(kind={} dbnum={:?})覆盖",
                entry.task_id,
                existing.kind,
                existing.state.as_str(),
                existing.dbnum,
                entry.kind,
                entry.dbnum
            );
        }
        inner.insert(entry.task_id.clone(), entry);
    }

    /// 容量剔除一条（找不到可剔的就任由超容——queued/running 永不剔除）。
    fn evict_one(inner: &mut IndexMap<String, TaskEntry>) {
        // 规则 2：先剔「同 dbnum 存在更新终态」的旧终态，最老优先。
        // IndexMap 迭代序即插入序（时间序），第一个命中的就是最老的。
        let superseded = inner
            .values()
            .filter(|t| t.state.is_terminal())
            .find(|t| {
                t.dbnum.is_some_and(|dbnum| {
                    inner.values().any(|other| {
                        other.task_id != t.task_id
                            && other.state.is_terminal()
                            && other.dbnum == Some(dbnum)
                            && other.created_at > t.created_at
                    })
                })
            })
            .map(|t| t.task_id.clone());
        let victim = superseded.or_else(|| {
            // 规则 3：没有可让位的旧终态时，全局最老的终态先走。
            inner
                .values()
                .find(|t| t.state.is_terminal())
                .map(|t| t.task_id.clone())
        });
        if let Some(id) = victim {
            inner.shift_remove(&id);
        }
    }

    /// 排队中的行被后来的触发并入会话：只推高右端（ADR-011 §5）。
    ///
    /// 时刻跟着序号一起动，且**只在序号真的抬高时才动**：否则一次没抬高的并入会把
    /// 右端时刻换成一个更早的值，序号与时刻当场对不上。
    pub fn update_queued_range(
        &self,
        task_id: &str,
        end_sesno: i32,
        end_sesno_time: Option<String>,
    ) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            if entry.state == TaskState::Queued && end_sesno > entry.end_sesno.unwrap_or(0) {
                entry.end_sesno = Some(end_sesno);
                entry.end_sesno_time = end_sesno_time;
            }
        }
    }

    /// 控制意图提升排队行时同时替换两端；与普通只抬高右端的合并不同，重建可能
    /// 把 `43..=50` 改成 `1..=7`，甚至改成 `0..=0`。
    pub fn replace_queued_range(
        &self,
        task_id: &str,
        start_sesno: i32,
        start_sesno_time: Option<String>,
        end_sesno: i32,
        end_sesno_time: Option<String>,
    ) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            if entry.state == TaskState::Queued {
                entry.start_sesno = Some(start_sesno);
                entry.start_sesno_time = start_sesno_time;
                entry.end_sesno = Some(end_sesno);
                entry.end_sesno_time = end_sesno_time;
            }
        }
    }

    pub fn set_queued_start(
        &self,
        task_id: &str,
        start_sesno: i32,
        start_sesno_time: Option<String>,
    ) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            if entry.state == TaskState::Queued {
                entry.start_sesno = Some(start_sesno);
                entry.start_sesno_time = start_sesno_time;
            }
        }
    }

    /// 冻结点重扫定下真实上界后回写（ADR-011 §5）。
    ///
    /// 与 `update_queued_range` 相反：那个只推高排队行、开跑后一概不动；这个只作用
    /// 于已经开跑的行，且直接赋值不取 max——冻结点看到什么就是什么，界面显示的
    /// 区间必须是真正要应用的那个。
    ///
    /// 时刻同样直接赋值，**包括赋成 `None`**：冻结把序号改了，旧时刻立刻就是错的，
    /// 读不到新时刻时宁可让那一格空着，也不能留一个对不上的时刻在上面。
    pub fn set_frozen_range(&self, task_id: &str, end_sesno: i32, end_sesno_time: Option<String>) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            if entry.state == TaskState::Running {
                entry.end_sesno = Some(end_sesno);
                entry.end_sesno_time = end_sesno_time;
            }
        }
    }

    /// 出队冻结：queued → running，记录开跑时刻。
    pub fn mark_started(&self, task_id: &str) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.state = TaskState::Running;
            entry.started_at = Some(Local::now().to_rfc3339());
        }
    }

    /// 本批次的交付单元总数（阶段二进度分母）。
    pub fn set_unit_totals(&self, task_id: &str, total: u32) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.total_units = Some(total);
            entry.units_done = Some(entry.units_done.unwrap_or(0));
        }
    }

    pub fn bump_units_done(&self, task_id: &str) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.units_done = Some(entry.units_done.unwrap_or(0) + 1);
        }
    }

    pub fn bump_events(&self, task_id: &str) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.events_seen += 1;
        }
    }

    /// 记录数据批次的可观测子阶段。重复报告同一阶段只刷新进展时刻，不重置起点。
    pub fn set_stage(&self, task_id: &str, stage: &str) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            let now = Local::now().to_rfc3339();
            if entry.current_stage.as_deref() != Some(stage) {
                entry.current_stage = Some(stage.to_string());
                entry.stage_started_at = Some(now.clone());
            }
            entry.stage_last_progress_at = Some(now);
        }
    }

    /// 依赖闭包的实质进展；调用一次即代表停滞计时可以重新起算。
    pub fn set_dependency_progress(
        &self,
        task_id: &str,
        stage: &str,
        dbnum: Option<u32>,
        path: Option<String>,
        total: u64,
        parsed: u64,
        missing: u64,
        stall_secs: i64,
    ) {
        self.set_stage(task_id, stage);
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            let now = Local::now();
            entry.dependency_dbnum = dbnum;
            entry.dependency_path = path;
            entry.dependency_refnos_total = Some(total);
            entry.dependency_refnos_parsed = Some(parsed);
            entry.dependency_refnos_missing = Some(missing);
            entry.stage_last_progress_at = Some(now.to_rfc3339());
            entry.stall_deadline = Some((now + chrono::Duration::seconds(stall_secs)).to_rfc3339());
            entry.events_seen = entry.events_seen.saturating_add(1);
        }
    }

    /// 更新当前依赖定位但不把它算作实质进展；用于进入一个可能卡住的文件/块之前。
    pub fn set_dependency_location(
        &self,
        task_id: &str,
        stage: &str,
        dbnum: Option<u32>,
        path: Option<String>,
        total: u64,
    ) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            if entry.current_stage.as_deref() != Some(stage) {
                entry.current_stage = Some(stage.to_string());
                entry.stage_started_at = Some(Local::now().to_rfc3339());
            }
            entry.dependency_dbnum = dbnum;
            entry.dependency_path = path;
            entry.dependency_refnos_total = Some(total);
        }
    }

    /// `/health` 的单一活动依赖快照；同一时刻 CATA 全局锁只允许一个解析者。
    pub fn active_dependency_snapshot(&self) -> Option<TaskEntry> {
        self.entries()
            .values()
            .find(|entry| {
                entry.state == TaskState::Running
                    && entry
                        .current_stage
                        .as_deref()
                        .is_some_and(|stage| stage.starts_with("dependency_"))
            })
            .cloned()
    }

    /// 覆盖 kind 专属详情。房间轮收尾时必须用收敛后的计数覆盖建行时那份
    /// （`{panels, elements, dead_letters}`）：`finish` 从不动 `detail`，而收敛到 0
    /// 的下一空闲轮不再建新行——不覆盖的话，客户端泳道读到的 live 永远停在本轮
    /// 开跑前的数字，30 分钟后被判成「饥饿」刷红且永不自愈（2026-07-30 审计 B2）。
    pub fn set_detail(&self, task_id: &str, detail: serde_json::Value) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.detail = Some(detail);
        }
    }

    pub fn finish(&self, task_id: &str, state: TaskState, result: serde_json::Value) {
        let mut inner = self.entries();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.state = state;
            entry.finished_at = Some(Local::now().to_rfc3339());
            entry.stall_deadline = None;
            entry.result = Some(result);
        }
    }

    pub fn get(&self, task_id: &str) -> Option<TaskEntry> {
        self.entries().get(task_id).cloned()
    }

    /// 按创建时间倒序（最近优先）过滤列出。
    pub fn list(&self, state: Option<&str>, kind: Option<&str>, limit: usize) -> Vec<TaskEntry> {
        let inner = self.entries();
        inner
            .values()
            .rev()
            .filter(|t| state.map_or(true, |s| t.state.as_str() == s))
            .filter(|t| kind.map_or(true, |k| t.kind == k))
            .take(limit)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal(registry: &TaskRegistry, task_id: &str, dbnum: u32, created_at: &str) {
        registry.insert_entry(TaskEntry {
            task_id: task_id.to_string(),
            kind: TASK_KIND_DATA_BATCH,
            state: TaskState::Succeeded,
            project: "P".into(),
            created_at: created_at.to_string(),
            started_at: None,
            finished_at: Some(created_at.to_string()),
            dbnum: Some(dbnum),
            db_type: Some("DESI".into()),
            start_sesno: Some(1),
            end_sesno: Some(2),
            start_sesno_time: None,
            end_sesno_time: None,
            units_done: None,
            total_units: None,
            events_seen: 0,
            current_stage: None,
            stage_started_at: None,
            stage_last_progress_at: None,
            dependency_dbnum: None,
            dependency_path: None,
            dependency_refnos_total: None,
            dependency_refnos_parsed: None,
            dependency_refnos_missing: None,
            stall_deadline: None,
            detail: None,
            result: None,
        });
    }

    /// 只关心区间的用例走这个；关心时刻的用例直接调 `insert_queued_batch`。
    fn queue_row(registry: &TaskRegistry, task_id: &str, dbnum: u32, start: i32, end: i32) {
        registry.insert_queued_batch(task_id, "P", dbnum, "DESI", start, None, end, None);
    }

    fn fill_to_capacity_with_queued(registry: &TaskRegistry, count: usize) {
        for i in 0..count {
            queue_row(registry, &format!("q-{i}"), 90_000 + i as u32, 1, 2);
        }
    }

    #[test]
    fn dependency_progress_is_visible_and_terminal_state_clears_the_deadline() {
        let registry = TaskRegistry::default();
        queue_row(&registry, "dep", 8000, 34, 232);
        registry.mark_started("dep");
        registry.set_dependency_progress(
            "dep",
            "dependency_closure",
            Some(7355),
            Some("C:/fixture/ams7355".into()),
            20,
            12,
            1,
            300,
        );
        let active = registry
            .active_dependency_snapshot()
            .expect("active dependency");
        assert_eq!(active.current_stage.as_deref(), Some("dependency_closure"));
        assert_eq!(active.dependency_refnos_parsed, Some(12));
        assert!(active.stall_deadline.is_some());

        registry.finish(
            "dep",
            TaskState::Failed,
            serde_json::json!({"error":"stall"}),
        );
        assert!(registry.active_dependency_snapshot().is_none());
        assert!(registry.get("dep").unwrap().stall_deadline.is_none());
    }

    /// 终态必须留着 `current_stage`：那是 `/tasks` 上唯一说得出「死在哪一步」的
    /// 字段，面板照着它画。`stall_deadline` 相反——它是活着的任务才有的死线，留到
    /// 终态就变成一个永远过期的时刻。两者同在 `finish` 里，很容易被一起清掉。
    ///
    /// 收集口有十几个各自具名的硬失败出口，回执那句原话分不出它发生在收集还是
    /// 写回；2026-08-27 的 SYST 8191 现场就缺这一格。
    #[test]
    fn a_failed_task_keeps_the_stage_it_died_at() {
        let registry = TaskRegistry::default();
        queue_row(&registry, "batch", 8191, 36, 37);
        registry.mark_started("batch");
        registry.set_stage("batch", "data_parse");
        registry.set_stage("batch", "collect_window");
        registry.finish(
            "batch",
            TaskState::Failed,
            serde_json::json!({"batch":{"message":"读取增量数据失败: 终稿合成: …"}}),
        );

        let entry = registry.get("batch").expect("failed batch stays listed");
        assert_eq!(
            entry.current_stage.as_deref(),
            Some("collect_window"),
            "终态丢了阶段，面板就只剩「失败了」三个字"
        );
        assert!(
            entry.stage_started_at.is_some(),
            "阶段起点要一起留下：它决定这一步走了多久"
        );
    }

    #[test]
    fn queued_and_running_rows_survive_eviction() {
        let registry = TaskRegistry::default();
        fill_to_capacity_with_queued(&registry, MAX_TASKS);
        registry.mark_started("q-0");

        // 满容之后再插入：没有终态可剔，queued/running 一条都不能丢。
        queue_row(&registry, "overflow", 1, 1, 2);
        assert!(registry.get("q-0").is_some(), "running 行被剔除");
        assert!(registry.get("q-1").is_some(), "queued 行被剔除");
        assert!(registry.get("overflow").is_some());
    }

    #[test]
    fn each_dbnum_keeps_its_latest_terminal_entry() {
        let registry = TaskRegistry::default();
        // 同一个 dbnum 两条终态 + 其它 dbnum 各一条，垫到满容。
        terminal(&registry, "old-7997", 7997, "2026-07-27T10:00:00+08:00");
        terminal(&registry, "new-7997", 7997, "2026-07-27T11:00:00+08:00");
        for i in 0..(MAX_TASKS - 2) {
            terminal(
                &registry,
                &format!("t-{i}"),
                10_000 + i as u32,
                "2026-07-27T12:00:00+08:00",
            );
        }

        queue_row(&registry, "trigger", 7997, 3, 4);
        assert!(
            registry.get("old-7997").is_none(),
            "同 dbnum 的旧终态应最先让位"
        );
        assert!(
            registry.get("new-7997").is_some(),
            "每个 dbnum 保留最近一条终态"
        );
    }

    #[test]
    fn overflow_evicts_the_oldest_terminal_when_every_dbnum_is_unique() {
        let registry = TaskRegistry::default();
        terminal(&registry, "oldest", 1, "2026-07-27T09:00:00+08:00");
        for i in 0..(MAX_TASKS - 1) {
            terminal(
                &registry,
                &format!("t-{i}"),
                100 + i as u32,
                "2026-07-27T10:00:00+08:00",
            );
        }
        queue_row(&registry, "trigger", 7997, 1, 2);
        assert!(registry.get("oldest").is_none(), "全局最老的终态先走");
        assert!(registry.get("trigger").is_some());
    }

    #[test]
    fn merge_only_raises_the_queued_end_sesno() {
        let registry = TaskRegistry::default();
        queue_row(&registry, "row", 7997, 1024, 1034);
        registry.update_queued_range("row", 1041, None);
        assert_eq!(registry.get("row").unwrap().end_sesno, Some(1041));
        registry.update_queued_range("row", 1030, None);
        assert_eq!(
            registry.get("row").unwrap().end_sesno,
            Some(1041),
            "并入会话只推高不降低"
        );

        registry.mark_started("row");
        registry.update_queued_range("row", 2000, None);
        assert_eq!(
            registry.get("row").unwrap().end_sesno,
            Some(1041),
            "冻结之后区间不再变"
        );
    }

    /// 保存窗口那一列显示的是时刻，所以时刻必须与右端序号同生共死：并入推高了就
    /// 跟着换，没推高就一个字都不许动。只推序号不刷时刻的话，窗口会停在入队观察到
    /// 的那一刻，并入得越多界面上越骗人；反过来在没推高时也跟着换，就会出现
    /// 「序号是新的、时刻是旧的」这种自相矛盾的一行。
    #[test]
    fn the_window_time_moves_with_the_end_sesno_and_only_with_it() {
        let registry = TaskRegistry::default();
        registry.insert_queued_batch(
            "row",
            "P",
            7997,
            "DESI",
            1024,
            Some("2026-08-01T09:12:00+08:00".into()),
            1034,
            Some("2026-08-07T14:33:00+08:00".into()),
        );

        registry.update_queued_range("row", 1041, Some("2026-08-07T15:42:00+08:00".into()));
        let entry = registry.get("row").unwrap();
        assert_eq!(entry.end_sesno, Some(1041));
        assert_eq!(
            entry.end_sesno_time.as_deref(),
            Some("2026-08-07T15:42:00+08:00"),
            "并入推高了右端，时刻必须跟着走"
        );

        // 一次没推高的并入：序号不动，时刻也不许被换成更早的那个。
        registry.update_queued_range("row", 1030, Some("2026-08-02T08:00:00+08:00".into()));
        let entry = registry.get("row").unwrap();
        assert_eq!(entry.end_sesno, Some(1041));
        assert_eq!(
            entry.end_sesno_time.as_deref(),
            Some("2026-08-07T15:42:00+08:00"),
            "右端没抬高，时刻不能退回去"
        );
        assert_eq!(
            entry.start_sesno_time.as_deref(),
            Some("2026-08-01T09:12:00+08:00"),
            "左端与并入无关，全程不动"
        );

        // 冻结点直接赋值：读不到新时刻时宁可空着，也不能留一个对不上的旧时刻。
        registry.mark_started("row");
        registry.set_frozen_range("row", 1038, None);
        let entry = registry.get("row").unwrap();
        assert_eq!(entry.end_sesno, Some(1038));
        assert!(
            entry.end_sesno_time.is_none(),
            "冻结改了序号，旧时刻立刻就是错的，必须清掉"
        );
    }

    #[test]
    fn task_ids_minted_within_one_second_are_all_distinct() {
        // `init_watcher` 的重扫在一个紧循环里逐个 dbnum 建行，整批落在同一秒。
        // 放宽 `manual_db_nums` 后那一批是 287 条（rollout 六之二实测），这里取
        // 300 覆盖它。旧的 u16 随机后缀在这个量级下约 47% 会撞。
        let ids: std::collections::HashSet<String> =
            (0..300).map(|_| TaskRegistry::new_task_id("db")).collect();
        assert_eq!(ids.len(), 300, "同一秒内生成的 task_id 必须互不相同");
    }

    #[test]
    fn task_ids_keep_the_kind_prefix_and_sort_by_mint_order() {
        let first = TaskRegistry::new_task_id("db");
        let second = TaskRegistry::new_task_id("db");
        assert!(first.starts_with("db-"), "客户端按前缀区分 kind：{first}");
        assert!(
            first < second,
            "序号单调，字典序即入队序：{first} 应排在 {second} 之前"
        );
    }

    /// 房间轮收尾用收敛后的计数覆盖 detail（2026-07-30 审计 B2）。
    ///
    /// `finish` 从不动 `detail`，而收敛到 0 的下一空闲轮不建新行——没有 `set_detail`
    /// 的话，客户端泳道读到的 live 永远停在本轮开跑前的数字，收敛得越干净，
    /// 「N 块面板待重算」的误报挂得越久，30 分钟后还会被判成「饥饿」。
    #[test]
    fn a_room_round_detail_can_be_overwritten_after_convergence() {
        let registry = TaskRegistry::default();
        registry.insert_running_room_round(
            "room-1",
            "P",
            5,
            serde_json::json!({ "panels": 3, "elements": 2, "dead_letters": 1 }),
        );
        registry.set_detail(
            "room-1",
            serde_json::json!({ "panels": 0, "elements": 0, "dead_letters": 1 }),
        );
        registry.finish(
            "room-1",
            TaskState::Succeeded,
            serde_json::json!({ "done": 5, "total": 5 }),
        );

        let entry = registry.get("room-1").unwrap();
        let detail = entry.detail.expect("detail 必须保留分项计数");
        assert_eq!(detail["panels"], 0, "收敛后的面板数必须归零");
        assert_eq!(detail["elements"], 0);
        assert_eq!(detail["dead_letters"], 1, "死信数是唯一的暴露出口，不能丢");
    }

    #[test]
    fn started_at_is_set_on_freeze_not_on_enqueue() {
        let registry = TaskRegistry::default();
        queue_row(&registry, "row", 7997, 1, 2);
        assert!(registry.get("row").unwrap().started_at.is_none());
        registry.mark_started("row");
        let entry = registry.get("row").unwrap();
        assert_eq!(entry.state, TaskState::Running);
        assert!(
            entry.started_at.is_some(),
            "「已排」与「已用」是两个起点，开跑时刻不能缺"
        );
    }

    #[test]
    fn yielded_model_drain_is_terminal_and_keeps_its_root_identity() {
        let registry = TaskRegistry::default();
        registry.insert_running_model_drain(
            "model-1",
            "P",
            1,
            serde_json::json!({
                "epoch_id": 7,
                "roots": [{
                    "dbnum": 8000,
                    "source_end_sesno": 224,
                    "target_refno": "1/2",
                    "action": "regen_root",
                    "revision": 3
                }]
            }),
        );
        registry.finish(
            "model-1",
            TaskState::Yielded,
            serde_json::json!({ "completed": 0, "unstarted": 1 }),
        );

        let entry = registry
            .get("model-1")
            .expect("model task must remain visible");
        assert_eq!(entry.kind, TASK_KIND_MODEL_DRAIN);
        assert_eq!(entry.state.as_str(), "yielded");
        assert!(entry.state.is_terminal());
        assert_eq!(entry.detail.unwrap()["roots"][0]["target_refno"], "1/2");
    }
}
