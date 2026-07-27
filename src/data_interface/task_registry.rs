//! 进程级任务注册表：队列与任务的 UI 视图（ADR-011 §3 / §11）。
//!
//! 从 `web_service::tasks` 搬到 feature 无关层：`web_service` 整个在
//! `http_api` 门后，而合流后的队列消费者（单 worker）不分编译形态都要写
//! 任务状态——队列真身只能有一份，不能随 feature 分叉（rollout 第八节第 4 条）。
//!
//! durable 语义仍由 `applied_sesno` 水位与 `model_update_pending` 表承担；
//! 本表仅内存、重启即清空，重启后由 `init_watcher` 重扫水位把队列重建出来
//! （ADR-011 §4——界面必须说得出「这是重建的队列」）。

use std::sync::{Mutex, OnceLock};

use chrono::Local;
use indexmap::IndexMap;
use serde::Serialize;

/// 一个数据批次（dbnum × 会话区间）的任务行。
pub const TASK_KIND_DATA_BATCH: &str = "data_batch";
/// 一轮房间归属收敛（ADR-011 §10：与数据批次同构的一种 kind）。
pub const TASK_KIND_ROOM_RECALC: &str = "room_recalc";

/// 分层保留的兜底上限（ADR-011 §11 + rollout 第八节第 8 条）：
/// 首轮放宽 `manual_db_nums` 后 287 条排队 + 287 条终态就要 ≥574，
/// 200 差了一个量级；1000 = 574 打底 + 全局最近终态的余量。
const MAX_TASKS: usize = 1000;

/// 任务状态机：`queued -> running -> succeeded | partial | failed`。
///
/// `queued` 随 ADR-011 §3 引入——数据批次在队列里排队时就要有一行可看。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    Succeeded,
    Partial,
    Failed,
}

impl TaskState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }

    /// 终态才可被容量剔除；queued / running 永不剔除（ADR-011 §11）。
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Partial | Self::Failed)
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
    /// 阶段二进度：本批次已生成的交付单元数（口径按数据批次，ADR-0007 迁移）。
    /// 房间轮任务复用同一对字段记 done/total。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units_done: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_units: Option<u32>,
    /// 该任务累计广播过的进度事件数（重连后前端用于对齐，见 spec §5.4）。
    pub events_seen: u64,
    /// 终态结果 JSON；queued / running 时为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
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

impl TaskRegistry {
    /// 进程级单例：worker（feature 无关）与 web_service（`http_api` 门内）
    /// 共用同一份，队列真身不随编译形态分叉。
    pub fn global() -> &'static TaskRegistry {
        REGISTRY.get_or_init(TaskRegistry::default)
    }

    pub fn new_task_id(prefix: &str) -> String {
        format!(
            "{}-{}-{:04x}",
            prefix,
            Local::now().format("%Y%m%d-%H%M%S"),
            rand::random::<u16>()
        )
    }

    /// 新排一条数据批次（state = queued）。返回该行 task_id。
    pub fn insert_queued_batch(
        &self,
        task_id: &str,
        project: &str,
        dbnum: u32,
        db_type: &str,
        start_sesno: i32,
        end_sesno: i32,
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
            units_done: None,
            total_units: None,
            events_seen: 0,
            result: None,
        });
    }

    /// 新排一条房间收敛轮（房间轮不排队，创建即 running；ADR-011 §10）。
    pub fn insert_running_room_round(&self, task_id: &str, project: &str, total: u32) {
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
            units_done: Some(0),
            total_units: Some(total),
            events_seen: 0,
            result: None,
        });
    }

    fn insert_entry(&self, entry: TaskEntry) {
        let mut inner = self.inner.lock().unwrap();
        if inner.len() >= MAX_TASKS {
            Self::evict_one(&mut inner);
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
    pub fn update_queued_range(&self, task_id: &str, end_sesno: i32) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.get_mut(task_id) {
            if entry.state == TaskState::Queued {
                entry.end_sesno = Some(entry.end_sesno.unwrap_or(0).max(end_sesno));
            }
        }
    }

    /// 出队冻结：queued → running，记录开跑时刻。
    pub fn mark_started(&self, task_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.state = TaskState::Running;
            entry.started_at = Some(Local::now().to_rfc3339());
        }
    }

    /// 本批次的交付单元总数（阶段二进度分母）。
    pub fn set_unit_totals(&self, task_id: &str, total: u32) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.total_units = Some(total);
            entry.units_done = Some(entry.units_done.unwrap_or(0));
        }
    }

    pub fn bump_units_done(&self, task_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.units_done = Some(entry.units_done.unwrap_or(0) + 1);
        }
    }

    pub fn bump_events(&self, task_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.events_seen += 1;
        }
    }

    pub fn finish(&self, task_id: &str, state: TaskState, result: serde_json::Value) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.state = state;
            entry.finished_at = Some(Local::now().to_rfc3339());
            entry.result = Some(result);
        }
    }

    pub fn get(&self, task_id: &str) -> Option<TaskEntry> {
        self.inner.lock().unwrap().get(task_id).cloned()
    }

    /// 按创建时间倒序（最近优先）过滤列出。
    pub fn list(&self, state: Option<&str>, kind: Option<&str>, limit: usize) -> Vec<TaskEntry> {
        let inner = self.inner.lock().unwrap();
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
            units_done: None,
            total_units: None,
            events_seen: 0,
            result: None,
        });
    }

    fn fill_to_capacity_with_queued(registry: &TaskRegistry, count: usize) {
        for i in 0..count {
            registry.insert_queued_batch(&format!("q-{i}"), "P", 90_000 + i as u32, "DESI", 1, 2);
        }
    }

    #[test]
    fn queued_and_running_rows_survive_eviction() {
        let registry = TaskRegistry::default();
        fill_to_capacity_with_queued(&registry, MAX_TASKS);
        registry.mark_started("q-0");

        // 满容之后再插入：没有终态可剔，queued/running 一条都不能丢。
        registry.insert_queued_batch("overflow", "P", 1, "DESI", 1, 2);
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

        registry.insert_queued_batch("trigger", "P", 7997, "DESI", 3, 4);
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
        registry.insert_queued_batch("trigger", "P", 7997, "DESI", 1, 2);
        assert!(registry.get("oldest").is_none(), "全局最老的终态先走");
        assert!(registry.get("trigger").is_some());
    }

    #[test]
    fn merge_only_raises_the_queued_end_sesno() {
        let registry = TaskRegistry::default();
        registry.insert_queued_batch("row", "P", 7997, "DESI", 1024, 1034);
        registry.update_queued_range("row", 1041);
        assert_eq!(registry.get("row").unwrap().end_sesno, Some(1041));
        registry.update_queued_range("row", 1030);
        assert_eq!(
            registry.get("row").unwrap().end_sesno,
            Some(1041),
            "并入会话只推高不降低"
        );

        registry.mark_started("row");
        registry.update_queued_range("row", 2000);
        assert_eq!(
            registry.get("row").unwrap().end_sesno,
            Some(1041),
            "冻结之后区间不再变"
        );
    }

    #[test]
    fn started_at_is_set_on_freeze_not_on_enqueue() {
        let registry = TaskRegistry::default();
        registry.insert_queued_batch("row", "P", 7997, "DESI", 1, 2);
        assert!(registry.get("row").unwrap().started_at.is_none());
        registry.mark_started("row");
        let entry = registry.get("row").unwrap();
        assert_eq!(entry.state, TaskState::Running);
        assert!(
            entry.started_at.is_some(),
            "「已排」与「已用」是两个起点，开跑时刻不能缺"
        );
    }
}
