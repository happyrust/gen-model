//! Always-on coarse-grained progress journal, independent of the optional logger.
use std::{io::Write, sync::{Mutex, atomic::{AtomicU64, Ordering}}, time::Instant};
static LOCK: Mutex<()> = Mutex::new(());
static NEXT: AtomicU64 = AtomicU64::new(1);
const LIMIT: u64 = 10 * 1024 * 1024;

fn console_label(stage: &str) -> &str {
    match stage {
        "increment.watch" => "增量监听",
        "native.runtime" => "C++运行库检查",
        "native.runtime.failed" => "C++运行库不兼容",
        "increment.poll" => "增量检测",
        "increment.poll.failed" => "增量检测失败",
        "increment.file.failed" => "增量文件读取失败",
        "increment.file.recovered" => "增量文件读取恢复",
        "increment.detected" => "发现库变化",
        "increment.delta.begin" => "开始解析增删改",
        "increment.delta.item" => "元素增删改",
        "increment.delta.done" => "增删改解析完成",
        "increment.delta.failed" => "增删改解析失败",
        "increment.delta.unavailable" => "增删改明细不可用",
        "model.publish.begin" => "模型发布开始",
        "model.publish.done" => "模型发布完成",
        "increment.invalidate.begin" => "刷新缓存与句柄",
        "increment.presence.begin" => "检查已有模型根",
        "increment.presence.done" => "模型根检查完成",
        "increment.delete.begin" => "删除失效模型",
        "increment.delete.done" => "模型删除完成",
        "increment.delete.failed" => "模型删除失败",
        "increment.generate.begin" => "开始重新生成模型",
        "http.model.ensure" => "网页模型请求",
        "model.worker.begin" => "模型后台任务开始",
        "model.roots.begin" => "解析生成根",
        "model.source.begin" => "准备模型数据源",
        "model.geometry.begin" => "几何生成开始",
        "model.geometry.done" => "几何生成完成",
        "finished" => "处理结束",
        "interrupted" => "处理未完成",
        _ => stage,
    }
}

fn append(path: &std::path::Path, line: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    if std::fs::metadata(path).is_ok_and(|m| m.len() >= LIMIT) {
        let previous = path.with_extension("jsonl.1");
        if previous.exists() { std::fs::remove_file(&previous)?; }
        std::fs::rename(path, previous)?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line)?;
    file.write_all(b"\n")?;
    file.flush()
}

pub fn event(id: u64, stage: &str, detail: &str) {
    let row = serde_json::json!({"time":chrono::Local::now().to_rfc3339(),
        "pid":std::process::id(), "build_id":crate::build_info::BUILD_ID,
        "operation":id, "stage":stage, "detail":detail});
    let line = row.to_string();
    // A closed/redirection pipe must not make the model service panic.
    let _ = writeln!(std::io::stdout().lock(), "[{}] [{}] op={} {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f%:z"), console_label(stage), id,
        detail.replace('\n', " ").replace('\r', " "));
    let _lock = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Err(error) = append(std::path::Path::new("logs/process-events.jsonl"), line.as_bytes()) {
        eprintln!("[process] journal write failed: {error}");
    }
}

pub struct Operation { id: u64, started: Instant, finished: bool }
impl Operation {
    pub fn id(&self) -> u64 { self.id }
    pub fn begin(stage: &str, detail: impl AsRef<str>) -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        event(id, stage, detail.as_ref());
        Self { id, started: Instant::now(), finished: false }
    }
    pub fn stage(&self, stage: &str, detail: impl AsRef<str>) {
        event(self.id, stage, &format!("elapsed_ms={} {}", self.started.elapsed().as_millis(), detail.as_ref()));
    }
    pub fn finish(&mut self, outcome: impl AsRef<str>) {
        self.stage("finished", outcome);
        self.finished = true;
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        if !self.finished { self.stage("interrupted", "scope exited before completion (error, cancellation or unwind)"); }
    }
}

pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Avoid reacquiring a journal lock if the panic came from diagnostics itself.
        let line = serde_json::json!({"time":chrono::Local::now().to_rfc3339(),
            "pid":std::process::id(), "build_id":crate::build_info::BUILD_ID,
            "stage":"panic", "message":info.to_string(),
            "backtrace":std::backtrace::Backtrace::force_capture().to_string()}).to_string();
        eprintln!("[process] {line}");
        if let Ok(_lock) = LOCK.try_lock() {
            let _ = append(std::path::Path::new("logs/process-events.jsonl"), line.as_bytes());
        }
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_appends_and_rotates_without_a_logger() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/events.jsonl");
        append(&path, br#"{"stage":"begin"}"#).unwrap();
        append(&path, br#"{"stage":"failed"}"#).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        for line in text.lines() { serde_json::from_str::<serde_json::Value>(line).unwrap(); }
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(LIMIT).unwrap();
        append(&path, br#"{"stage":"next"}"#).unwrap();
        assert!(path.with_extension("jsonl.1").exists());
        assert_eq!(std::fs::read_to_string(path).unwrap().lines().count(), 1);
    }
    #[test]
    fn incremental_console_stages_are_readable() {
        assert_eq!(console_label("increment.poll"), "增量检测");
        assert_eq!(console_label("increment.detected"), "发现库变化");
        assert_eq!(console_label("increment.poll.failed"), "增量检测失败");
        assert_eq!(console_label("custom"), "custom");
    }
}
