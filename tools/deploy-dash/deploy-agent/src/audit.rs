//! 审计（设计第 6 节、R9）：`<根>\logs\gui\agent-audit.jsonl`，一行一个 JSON：
//! `{ "t", "from_ip", "client", "action", "args", "job_id", "confirm", "result" }`。满 10 MB 改名 `.1`（盖掉旧的 `.1`），
//! 照后端 `process-events.jsonl` 的做法。令牌、口令从不进审计。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct AuditEntry {
    pub t: String,
    pub from_ip: String,
    pub client: String,
    pub action: String,
    pub args: serde_json::Value,
    pub job_id: Option<u64>,
    pub confirm: Option<bool>,
    pub result: String,
}

pub struct AuditLog {
    path: PathBuf,
    rotate_bytes: u64,
    lock: Mutex<()>,
}

impl AuditLog {
    pub fn new(path: PathBuf, rotate_bytes: u64) -> Self {
        Self { path, rotate_bytes: rotate_bytes.max(1024), lock: Mutex::new(()) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn rotated_path(&self) -> PathBuf {
        let mut name = self.path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        name.push(".1");
        self.path.with_file_name(name)
    }

    /// 追加一行；写之前文件已经满了就先改名 `.1`。
    pub fn append(&self, entry: &AuditEntry) -> std::io::Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
        line.push('\n');
        write_rotating(&self.path, &line, self.rotate_bytes)
    }
}

/// 代理自己的文本日志（`agent.log`）也用同一套：满了改名 `.1` 再追加。
pub fn append_rotating(path: &Path, line: &str, rotate_bytes: u64) -> std::io::Result<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    write_rotating(path, line, rotate_bytes)
}

fn write_rotating(path: &Path, line: &str, rotate_bytes: u64) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).map(|m| m.len() >= rotate_bytes).unwrap_or(false) {
        let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        name.push(".1");
        std::fs::rename(path, path.with_file_name(name))?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(line.as_bytes())
}

/// `X-Client` 头只用于审计：去掉控制字符，最多 128 个字。
pub fn clean_client(raw: Option<&str>) -> String {
    raw.unwrap_or("").chars().filter(|c| !c.is_control()).take(128).collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(i: usize) -> AuditEntry {
        AuditEntry {
            t: "2026-11-12T20:30:00+08:00".into(),
            from_ip: "127.0.0.1".into(),
            client: "<计算机名>\\<用户名>".into(),
            action: "job.down".into(),
            args: serde_json::json!({ "force": false, "i": i }),
            job_id: Some(i as u64),
            confirm: Some(true),
            result: "accepted".into(),
        }
    }

    #[test]
    fn rotates_when_full_and_keeps_one_old_file() {
        let dir = std::env::temp_dir().join(format!("deploy-agent-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = AuditLog::new(dir.join("agent-audit.jsonl"), 1024);
        for i in 0..40 {
            log.append(&entry(i)).unwrap();
        }
        let cur = std::fs::read_to_string(log.path()).unwrap();
        let old = std::fs::read_to_string(log.rotated_path()).unwrap();
        assert!(std::fs::metadata(log.path()).unwrap().len() < 1024 + 300, "当前文件不会比门槛大一行以上");
        assert!(!old.is_empty());
        // 每行都是完整的 JSON，字段齐
        for line in cur.lines().chain(old.lines()) {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            for k in ["t", "from_ip", "client", "action", "args", "job_id", "confirm", "result"] {
                assert!(v.get(k).is_some(), "缺 {k}：{line}");
            }
        }
        // 最后一条在当前文件里
        assert!(cur.lines().last().unwrap().contains("\"i\":39"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn client_header_is_cleaned() {
        assert_eq!(clean_client(Some("PC\\user\r\nX-Evil: 1")), "PC\\userX-Evil: 1");
        assert_eq!(clean_client(None), "");
        assert_eq!(clean_client(Some(&"a".repeat(500))).len(), 128);
    }
}
