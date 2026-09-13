//! Startup snapshot of project-wide DAB session tags.
//!
//! The full inventory is file I/O over every retained session, so `/health`
//! never performs it inline. Startup computes it once on a blocking worker,
//! logs the aggregate and publishes a bounded process-local summary. This is
//! diagnostic evidence only: missing/old tags neither block startup nor select
//! an E3D SchemaPack.

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const ISSUE_LIMIT: usize = 20;

struct Snapshot {
    sampled_at: Option<Instant>,
    value: Value,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            sampled_at: None,
            value: json!({
                "status": "pending",
                "scope": "startup",
                "refreshing": false,
                "sample_age_ms": null,
                "release_selection": "diagnostic_only",
            }),
        }
    }
}

static SNAPSHOT: LazyLock<Mutex<Snapshot>> = LazyLock::new(|| Mutex::new(Snapshot::default()));

/// Start the one-shot inventory after the health server has been spawned.
///
/// The scan stays off the async runtime and the caller does not await it:
/// `/health` reports `pending` while it runs, so a slow project share cannot
/// delay the rest of service startup.
pub fn start_at_startup(project_root: PathBuf) {
    {
        let mut snapshot = SNAPSHOT.lock().unwrap_or_else(PoisonError::into_inner);
        snapshot.sampled_at = None;
        snapshot.value = json!({
            "status": "pending",
            "scope": "startup",
            "refreshing": true,
            "project_root": project_root.display().to_string(),
            "sample_age_ms": null,
            "release_selection": "diagnostic_only",
        });
    }
    tokio::spawn(refresh(project_root));
}

async fn refresh(project_root: PathBuf) {
    let started = Instant::now();
    let task_root = project_root.clone();
    let result = tokio::task::spawn_blocking(move || e3d_io::project_dabacon_tags(task_root)).await;

    let value = match result {
        Ok(Ok(report)) => {
            let elapsed = started.elapsed();
            let message = format!(
                "[init] 项目 DAB tag 聚合: root={} discovered={} readable={} issues={} \
                 latest_tags={:?} historical_session_tags={:?} \
                 latest_untagged={} historical_untagged_databases={} \
                 historical_untagged_sessions={} elapsed_ms={}\
                 （仅诊断，不用于选版本）",
                report.root.display(),
                report.discovered_databases,
                report.databases.len(),
                report.issues.len(),
                report.latest_tag_database_counts,
                report.historical_tag_session_counts,
                report.latest_untagged_databases,
                report.historical_untagged_databases,
                report.historical_untagged_sessions,
                elapsed.as_millis(),
            );
            if report.issues.is_empty() {
                log::info!("{message}");
                println!("{message}");
            } else {
                log::warn!("{message}");
                eprintln!("{message}");
            }
            summary_value(&report, elapsed)
        }
        Ok(Err(error)) => {
            let message = format!(
                "[init] 项目 DAB tag 聚合失败（仅诊断，不阻塞）: root={}；原因: {error}",
                project_root.display()
            );
            log::warn!("{message}");
            eprintln!("{message}");
            unavailable_value(&project_root, &error.to_string(), started.elapsed())
        }
        Err(error) => {
            let message = format!(
                "[init] 项目 DAB tag 聚合任务失败（仅诊断，不阻塞）: root={}；原因: {error}",
                project_root.display()
            );
            log::warn!("{message}");
            eprintln!("{message}");
            unavailable_value(&project_root, &error.to_string(), started.elapsed())
        }
    };

    let mut snapshot = SNAPSHOT.lock().unwrap_or_else(PoisonError::into_inner);
    snapshot.sampled_at = Some(Instant::now());
    snapshot.value = value;
}

/// Bounded, process-local `/health` view of the startup inventory.
pub fn health_snapshot() -> Value {
    let snapshot = SNAPSHOT.lock().unwrap_or_else(PoisonError::into_inner);
    let mut value = snapshot.value.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "sample_age_ms".to_owned(),
            snapshot
                .sampled_at
                .map(|sampled_at| json!(sampled_at.elapsed().as_millis() as u64))
                .unwrap_or(Value::Null),
        );
    }
    value
}

fn summary_value(report: &e3d_io::ProjectDabaconTagReport, elapsed: Duration) -> Value {
    let issues = report
        .issues
        .iter()
        .take(ISSUE_LIMIT)
        .map(|issue| {
            json!({
                "database": issue.database,
                "detail": issue.detail,
            })
        })
        .collect::<Vec<_>>();
    let tagged_historical_sessions: usize = report.historical_tag_session_counts.values().sum();
    let status = if !report.issues.is_empty() {
        "partial"
    } else if report.discovered_databases == 0 {
        "empty"
    } else {
        "ready"
    };

    json!({
        "status": status,
        "scope": "startup",
        "refreshing": false,
        "project_root": report.root.display().to_string(),
        "sample_duration_ms": elapsed.as_millis() as u64,
        "sample_age_ms": 0,
        "discovered_databases": report.discovered_databases,
        "readable_databases": report.databases.len(),
        "issue_count": report.issues.len(),
        "issues": issues,
        "issues_truncated": report.issues.len().saturating_sub(ISSUE_LIMIT),
        "latest": {
            "tag_database_counts": report.latest_tag_database_counts,
            "untagged_databases": report.latest_untagged_databases,
        },
        "history": {
            "tag_session_counts": report.historical_tag_session_counts,
            "tag_database_counts": report.historical_tag_database_counts,
            "tagged_sessions": tagged_historical_sessions,
            "untagged_databases": report.historical_untagged_databases,
            "untagged_sessions": report.historical_untagged_sessions,
        },
        "release_selection": "diagnostic_only",
    })
}

fn unavailable_value(root: &std::path::Path, error: &str, elapsed: Duration) -> Value {
    json!({
        "status": "unavailable",
        "scope": "startup",
        "refreshing": false,
        "project_root": root.display().to_string(),
        "sample_duration_ms": elapsed.as_millis() as u64,
        "sample_age_ms": 0,
        "error": error,
        "release_selection": "diagnostic_only",
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn health_summary_is_bounded_and_never_claims_release_selection() {
        let report = e3d_io::ProjectDabaconTagReport {
            root: PathBuf::from("project"),
            discovered_databases: 27,
            databases: Vec::new(),
            issues: (0..25)
                .map(|index| e3d_io::ProjectDabaconTagIssue {
                    database: PathBuf::from(format!("broken-{index}")),
                    detail: "bad session".to_owned(),
                })
                .collect(),
            latest_tag_database_counts: BTreeMap::from([("2_35_23o".to_owned(), 2)]),
            historical_tag_session_counts: BTreeMap::from([
                ("2_28_19_".to_owned(), 7),
                ("2_35_23o".to_owned(), 5),
            ]),
            historical_tag_database_counts: BTreeMap::from([
                ("2_28_19_".to_owned(), 2),
                ("2_35_23o".to_owned(), 2),
            ]),
            latest_untagged_databases: 1,
            historical_untagged_databases: 1,
            historical_untagged_sessions: 3,
        };

        let value = summary_value(&report, Duration::from_millis(17));
        assert_eq!(value["status"], "partial");
        assert_eq!(value["release_selection"], "diagnostic_only");
        assert_eq!(value["sample_duration_ms"], 17);
        assert_eq!(value["latest"]["tag_database_counts"]["2_35_23o"], 2);
        assert_eq!(value["history"]["tagged_sessions"], 12);
        assert_eq!(value["history"]["untagged_sessions"], 3);
        assert_eq!(value["issue_count"], 25);
        assert_eq!(value["issues"].as_array().unwrap().len(), ISSUE_LIMIT);
        assert_eq!(value["issues_truncated"], 5);
    }
}
