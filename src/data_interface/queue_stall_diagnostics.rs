//! 数据批次停在出队门之前时的独立看门狗。
//!
//! 它与唯一 worker 分属两个 Tokio 任务：即使 worker 卡在一次数据库 await、空间收敛
//! 或直接退出，这里仍会把队列姿态同时写到 stderr 与每日 JSONL，供异机离线复核。

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Local};
use serde_json::{Value, json};

use crate::data_interface::batch_scheduler::{BatchScheduler, QueueRow};
use crate::data_interface::task_registry::TaskRegistry;

const WATCH_INTERVAL: Duration = Duration::from_secs(30);
const WARN_AFTER_SECS: i64 = 60;
const REPEAT_AFTER_SECS: i64 = 300;
const PREFIX: &str = "AIOS-QUEUE-STALL";

/// 每日文件名前缀。`batch_failure_log` 的统一读取口要按它找文件——两处各写一遍
/// 字面量，改名时只改得动一处，而读那一侧不会报错、只会永远读到空。
pub const FILE_PREFIX: &str = "queue-stalls-";

#[derive(Default)]
struct WatchState {
    first_seen: HashMap<String, i64>,
    last_emitted: HashMap<String, i64>,
}

/// 独立运行的停滞看门狗。由 `ensure_batch_worker` 与 worker 同时启动。
pub async fn run() {
    let mut state = WatchState::default();
    let mut interval = tokio::time::interval(WATCH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    eprintln!(
        "队列停滞看门狗已启动：等待超过 {WARN_AFTER_SECS}s 的未出队任务将写入 {}",
        daily_path(Local::now()).display()
    );
    loop {
        interval.tick().await;
        inspect_and_record(&mut state, Path::new("logs"));
    }
}

fn inspect_and_record(state: &mut WatchState, directory: &Path) {
    let scheduler = BatchScheduler::global();
    let registry = TaskRegistry::global();
    let now = Local::now();
    let now_secs = now.timestamp();
    let paused = scheduler.is_paused();
    let auto_work_armed = scheduler.is_auto_work_armed();
    let data_incremental = crate::options::data_incremental();
    let (worker_alive, worker_idle_secs) = crate::data_interface::batch_worker::worker_liveness();
    let initialization =
        crate::data_interface::initialization_phase::InitializationCoordinator::global().snapshot();
    let mut active = HashSet::new();

    for row in scheduler
        .snapshot()
        .into_iter()
        .filter(|row| row.state != "running")
    {
        let key = row_key(&row);
        active.insert(key.clone());
        let first_seen = *state.first_seen.entry(key.clone()).or_insert(now_secs);
        let task = (!row.task_id.is_empty())
            .then(|| registry.get(&row.task_id))
            .flatten();
        let created_at = task.as_ref().map(|entry| entry.created_at.clone());
        let queued_at = created_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map_or(first_seen, |value| value.timestamp());
        let wait_secs = now_secs.saturating_sub(queued_at);
        let last_emitted = state.last_emitted.get(&key).copied();
        if !should_emit(wait_secs, now_secs, last_emitted) {
            continue;
        }

        let reasons = classify(
            &row,
            paused,
            auto_work_armed,
            data_incremental,
            worker_alive,
        );
        let record = json!({
            "event": "queue_stall",
            "at": now.to_rfc3339(),
            "task_id": row.task_id,
            "dbnum": row.dbnum,
            "db_type": row.db_type,
            "state": row.state,
            "intent": row.intent,
            "phase": row.phase,
            "epoch_id": row.epoch_id,
            "start_sesno": row.start_sesno,
            "end_sesno": row.end_sesno,
            "created_at": created_at,
            "wait_secs": wait_secs,
            "reasons": reasons,
            "queue_paused": paused,
            "auto_work_armed": auto_work_armed,
            "data_incremental": data_incremental,
            "blocked_by_phase": row.blocked_by_phase,
            "worker_alive": worker_alive,
            "worker_idle_secs": worker_idle_secs,
            "initialization": initialization,
        });
        emit(directory, now, &record);
        state.last_emitted.insert(key, now_secs);
    }

    state.first_seen.retain(|key, _| active.contains(key));
    state.last_emitted.retain(|key, _| active.contains(key));
}

fn row_key(row: &QueueRow) -> String {
    if row.task_id.is_empty() {
        format!(
            "dbnum:{}:{}:{}:{}",
            row.dbnum, row.epoch_id, row.start_sesno, row.end_sesno
        )
    } else {
        row.task_id.clone()
    }
}

fn should_emit(wait_secs: i64, now_secs: i64, last_emitted: Option<i64>) -> bool {
    wait_secs >= WARN_AFTER_SECS
        && last_emitted.is_none_or(|last| now_secs.saturating_sub(last) >= REPEAT_AFTER_SECS)
}

fn classify(
    row: &QueueRow,
    paused: bool,
    auto_work_armed: bool,
    data_incremental: bool,
    worker_alive: bool,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if paused {
        reasons.push("queue_paused".to_string());
    }
    if row.state == "held" {
        reasons.push("held_until_real_trigger".to_string());
    }
    if !auto_work_armed {
        reasons.push("auto_work_not_armed".to_string());
    }
    if !data_incremental {
        reasons.push("data_incremental_disabled".to_string());
    }
    if let Some(phase) = row.blocked_by_phase {
        reasons.push(format!("blocked_by_phase:{phase}"));
    }
    if !worker_alive {
        reasons.push("worker_not_alive".to_string());
    }
    if reasons.is_empty() {
        reasons.push("dispatcher_not_claiming_eligible_row".to_string());
    }
    reasons
}

fn daily_path(now: DateTime<Local>) -> PathBuf {
    Path::new("logs").join(format!("{FILE_PREFIX}{}.jsonl", now.format("%Y-%m-%d")))
}

fn emit(directory: &Path, now: DateTime<Local>, record: &Value) {
    eprintln!("{PREFIX} {record}");
    let path = directory.join(format!("{FILE_PREFIX}{}.jsonl", now.format("%Y-%m-%d")));
    if let Err(error) = append_json_line(&path, record) {
        eprintln!("{PREFIX} 写入离线诊断文件 {} 失败: {error}", path.display());
    }
}

fn append_json_line(path: &Path, record: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{record}")?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: &'static str, blocked_by_phase: Option<&'static str>) -> QueueRow {
        QueueRow {
            task_id: "db-1".to_string(),
            dbnum: 7999,
            db_type: "DESI".to_string(),
            phase: "design",
            epoch_id: 7,
            blocked_by_phase,
            intent: "apply_window",
            state,
            start_sesno: 90,
            end_sesno: 92,
        }
    }

    #[test]
    fn stall_reason_explains_every_closed_dispatch_gate() {
        let reasons = classify(&row("held", Some("catalogue")), true, false, false, false);
        assert_eq!(
            reasons,
            [
                "queue_paused",
                "held_until_real_trigger",
                "auto_work_not_armed",
                "data_incremental_disabled",
                "blocked_by_phase:catalogue",
                "worker_not_alive",
            ]
        );
        assert_eq!(
            classify(&row("queued", None), false, true, true, true),
            ["dispatcher_not_claiming_eligible_row"]
        );
    }

    #[test]
    fn stall_warning_starts_after_one_minute_and_repeats_every_five() {
        assert!(!should_emit(59, 1_000, None));
        assert!(should_emit(60, 1_000, None));
        assert!(!should_emit(600, 1_299, Some(1_000)));
        assert!(should_emit(600, 1_300, Some(1_000)));
    }

    #[test]
    fn offline_record_is_one_reopenable_json_line() {
        let dir = std::env::temp_dir().join(format!(
            "aios-queue-stall-test-{}-{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let path = dir.join("queue.jsonl");
        let record = json!({"task_id": "db-1", "reason": "queue_paused"});
        append_json_line(&path, &record).expect("append diagnostic");
        let body = fs::read_to_string(&path).expect("reopen diagnostic");
        let parsed: Value = serde_json::from_str(body.trim()).expect("valid json line");
        assert_eq!(parsed, record);
        fs::remove_dir_all(dir).expect("remove fixture");
    }
}
