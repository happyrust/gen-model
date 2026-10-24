//! 终态任务的落盘账本（每日 JSONL）+ 进程内归档缓存：`GET /api/v1/tasks` 的「历史」那一半。
//!
//! 任务注册表（[`super::task_registry`]）是进程内的 UI 视图：`MAX_TASKS` 封顶、重启即空。增量记录页
//! （plant-ui「增量记录」页签）要回答的却是「这几天每一轮增量改了什么」：服务一重启账本就空，在
//! 用的人眼里就是历史被吃掉了（`docs/plans/2026-10-24-increment-records-server-gaps-plan.md` W5）。
//!
//! 所以每条任务收口时（[`super::task_registry::TaskRegistry::finish`]）按注册表行的**同一形状**往
//! `logs/task-history-YYYY-MM-DD.jsonl` 追加一行，形制与 [`super::batch_failure_log`] 一致（同一个目录、
//! 每日切分、一行一条 JSON）。读侧不每次翻文件：首次访问时把最近几天预热进一个有界的进程内归档
//! （按 `created_at` 倒序），以后收口一条推一条；`/tasks` 列表 = 注册表里的（含未收口的）⊕ 归档里
//! 注册表已经没有的，按 `created_at` 倒序、`?before=` 往回翻。
//!
//! 不记 `model_drain`：模型消费页一轮几十条，它们不是「一轮增量」的账，只会把文件撑大。
//! 不进库：这是 UI 视图不是水位，丢了只是看不到，不影响数据正确性。

use std::collections::{HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Local};
use serde_json::Value;

use super::task_registry::{TASK_KIND_MODEL_DRAIN, TaskEntry};

/// 落盘目录。与失败记录 / 停滞看门狗同一个，相对进程工作目录。
const DIRECTORY: &str = "logs";
const FILE_PREFIX: &str = "task-history-";
const FILE_SUFFIX: &str = ".jsonl";

/// 进程内归档上限（条）。一条收口行带全部告警约 5 KB，五千条约 25 MB；再早的要看就翻文件。
const MAX_CACHED: usize = 5000;

/// 预热时最多翻几天的文件。
const MAX_FILES_SCANNED: usize = 30;

/// 进程内归档：按 `created_at` 倒序（最新在前）。
pub struct TaskHistory {
    directory: PathBuf,
    cache: Mutex<VecDeque<Value>>,
}

static HISTORY: OnceLock<TaskHistory> = OnceLock::new();

impl TaskHistory {
    /// 进程级单例；第一次拿到时把 `logs/` 里最近几天的行预热进来。
    pub fn global() -> &'static TaskHistory {
        HISTORY.get_or_init(|| TaskHistory::open(Path::new(DIRECTORY)))
    }

    /// 开一份指定目录的归档（单测用临时目录）。目录不存在 / 读不动就是空归档，不拦启动。
    pub fn open(directory: &Path) -> Self {
        let cache = match preload(directory) {
            Ok(cache) => cache,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "[任务历史] 读不动 {}：{error}；归档从空开始，这次进程收口的任务照记",
                        directory.display()
                    );
                }
                VecDeque::new()
            }
        };
        if !cache.is_empty() {
            println!(
                "[任务历史] 从 {} 预热 {} 条已收口任务（/api/v1/tasks 重启后照样翻得到）",
                directory.display(),
                cache.len()
            );
        }
        Self {
            directory: directory.to_path_buf(),
            cache: Mutex::new(cache),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, VecDeque<Value>> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 记一条已收口的任务：追加到当天文件，再进归档。不是终态 / 是 `model_drain` 的不记。
    pub fn record(&self, entry: &TaskEntry) {
        if !entry.state.is_terminal() || entry.kind == TASK_KIND_MODEL_DRAIN {
            return;
        }
        let Ok(value) = serde_json::to_value(entry) else {
            return;
        };
        let path = daily_path(&self.directory, Local::now());
        if let Err(error) = append_json_line(&path, &value) {
            eprintln!(
                "[任务历史] {} 落盘失败 {}: {error}——这条只活在进程内",
                entry.task_id,
                path.display()
            );
        }
        self.push(value);
    }

    fn push(&self, value: Value) {
        let mut cache = self.cache();
        if let Some(id) = value.get("task_id").and_then(Value::as_str) {
            cache.retain(|row| row.get("task_id").and_then(Value::as_str) != Some(id));
        }
        let at = created_at_of(&value).to_owned();
        // 按 created_at 倒序插入：收口顺序≠入队顺序（排久的行晒收口），列表和 `?before=` 都按入队时刻翻。
        let position = cache
            .iter()
            .position(|row| created_at_of(row) < at.as_str())
            .unwrap_or(cache.len());
        cache.insert(position, value);
        cache.truncate(MAX_CACHED);
    }

    /// 翻一页归档：`exclude` 是注册表里还在的 task_id（那份更新，不重复给）；`before` 是入队时刻上界（不含）；
    /// 返回最多 `limit` 条（已按 created_at 倒序）。调用方多要一条就能知道还有没有。
    pub fn page(
        &self,
        exclude: &HashSet<String>,
        state: Option<&str>,
        kind: Option<&str>,
        before: Option<&str>,
        limit: usize,
    ) -> Vec<Value> {
        self.cache()
            .iter()
            .filter(|row| before.is_none_or(|before| created_at_of(row) < before))
            .filter(|row| state.is_none_or(|s| row.get("state").and_then(Value::as_str) == Some(s)))
            .filter(|row| kind.is_none_or(|k| row.get("kind").and_then(Value::as_str) == Some(k)))
            .filter(|row| {
                row.get("task_id")
                    .and_then(Value::as_str)
                    .is_none_or(|id| !exclude.contains(id))
            })
            .take(limit)
            .cloned()
            .collect()
    }

    /// 按 task_id 取一条归档行（`GET /tasks/{id}` 在注册表里找不到时的退路：重启前收口的任务详情也要答得出）。
    pub fn get(&self, task_id: &str) -> Option<Value> {
        self.cache()
            .iter()
            .find(|row| row.get("task_id").and_then(Value::as_str) == Some(task_id))
            .cloned()
    }

    /// 归档里有几条（单测与 `/health` 用）。
    pub fn len(&self) -> usize {
        self.cache().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn created_at_of(row: &Value) -> &str {
    row.get("created_at").and_then(Value::as_str).unwrap_or("")
}

fn daily_path(directory: &Path, now: DateTime<Local>) -> PathBuf {
    directory.join(format!(
        "{FILE_PREFIX}{}{FILE_SUFFIX}",
        now.format("%Y-%m-%d")
    ))
}

fn append_json_line(path: &Path, record: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{record}")?;
    file.flush()
}

/// 预热：最近 [`MAX_FILES_SCANNED`] 天的文件，按文件名（即日期）倒序读，认不出的行跳过而不是整份放弃；
/// 同一 task_id 只留最后一条；最后按 created_at 倒序、截到 [`MAX_CACHED`]。
fn preload(directory: &Path) -> std::io::Result<VecDeque<Value>> {
    let mut files: Vec<PathBuf> = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(FILE_PREFIX) && name.ends_with(FILE_SUFFIX))
        })
        .collect();
    files.sort();
    files.reverse();
    let mut seen: HashSet<String> = HashSet::new();
    let mut rows: Vec<Value> = Vec::new();
    for path in files.iter().take(MAX_FILES_SCANNED) {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("[任务历史] 跳过读不动的 {}: {error}", path.display());
                continue;
            }
        };
        let mut in_file: Vec<Value> = Vec::new();
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            in_file.push(value);
        }
        // 文件内是追加序：后写的在后，同 id 重复时后写的为准。
        for value in in_file.into_iter().rev() {
            let Some(id) = value.get("task_id").and_then(Value::as_str) else {
                continue;
            };
            if seen.insert(id.to_owned()) {
                rows.push(value);
            }
        }
        if rows.len() >= MAX_CACHED {
            break;
        }
    }
    rows.sort_by(|a, b| created_at_of(b).cmp(created_at_of(a)));
    rows.truncate(MAX_CACHED);
    Ok(rows.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::task_registry::{TASK_KIND_DATA_BATCH, TaskState};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "task-history-{name}-{}-{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn entry(task_id: &str, kind: &'static str, state: TaskState, created_at: &str) -> TaskEntry {
        let mut entry = TaskEntry::blank(task_id, kind, "P");
        entry.state = state;
        entry.created_at = created_at.to_owned();
        entry.finished_at = Some(created_at.to_owned());
        entry.dbnum = Some(8000);
        entry
    }

    /// 收口一条落一行；重开目录预热回来的就是那些行，按 created_at 倒序；`?before=` 往回翻不含上界。
    #[test]
    fn finished_tasks_survive_a_reopen_newest_first() {
        let dir = temp_dir("reopen");
        let history = TaskHistory::open(&dir);
        history.record(&entry(
            "db-1",
            TASK_KIND_DATA_BATCH,
            TaskState::Succeeded,
            "2026-10-23T12:12:54+08:00",
        ));
        history.record(&entry(
            "db-3",
            TASK_KIND_DATA_BATCH,
            TaskState::Failed,
            "2026-10-23T13:06:24+08:00",
        ));
        // 排久的行：入队比 db-3 早、收口比它晚——列表要按入队时刻排。
        history.record(&entry(
            "db-2",
            TASK_KIND_DATA_BATCH,
            TaskState::Succeeded,
            "2026-10-23T12:53:29+08:00",
        ));
        assert_eq!(history.len(), 3);

        let reopened = TaskHistory::open(&dir);
        let ids: Vec<String> = reopened
            .page(&HashSet::new(), None, None, None, 10)
            .iter()
            .map(|row| row["task_id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, vec!["db-3", "db-2", "db-1"]);

        let older: Vec<String> = reopened
            .page(
                &HashSet::new(),
                None,
                None,
                Some("2026-10-23T12:53:29+08:00"),
                10,
            )
            .iter()
            .map(|row| row["task_id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(older, vec!["db-1"], "上界不含，只剩更早的");

        let failed: Vec<String> = reopened
            .page(
                &HashSet::new(),
                Some("failed"),
                Some(TASK_KIND_DATA_BATCH),
                None,
                10,
            )
            .iter()
            .map(|row| row["task_id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(failed, vec!["db-3"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 注册表里还在的行不重复给；`model_drain` 与未收口的行不记；残行不连累整份文件。
    #[test]
    fn live_rows_drains_and_torn_lines_stay_out() {
        let dir = temp_dir("filters");
        let history = TaskHistory::open(&dir);
        history.record(&entry(
            "db-1",
            TASK_KIND_DATA_BATCH,
            TaskState::Succeeded,
            "2026-10-23T12:12:54+08:00",
        ));
        history.record(&entry(
            "drain-1",
            TASK_KIND_MODEL_DRAIN,
            TaskState::Succeeded,
            "2026-10-23T12:13:00+08:00",
        ));
        history.record(&entry(
            "db-9",
            TASK_KIND_DATA_BATCH,
            TaskState::Running,
            "2026-10-23T12:14:00+08:00",
        ));
        assert_eq!(history.len(), 1, "只有终态的非 drain 行进账");

        let live: HashSet<String> = ["db-1".to_owned()].into_iter().collect();
        assert!(
            history.page(&live, None, None, None, 10).is_empty(),
            "注册表里还在的不重复给"
        );

        let path = daily_path(&dir, Local::now());
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        write!(
            file,
            "{{\"task_id\":\"db-torn\",\"created_at\":\"2026-10-23T12:15"
        )
        .unwrap();
        drop(file);
        let reopened = TaskHistory::open(&dir);
        assert_eq!(reopened.len(), 1, "残行跳过，完整的那条还在");
        let _ = fs::remove_dir_all(&dir);
    }
}
