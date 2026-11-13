//! Always-on coarse-grained progress journal, independent of the optional logger.
use std::{
    io::Write,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() >= LIMIT) {
        let previous = path.with_extension("jsonl.1");
        if previous.exists() {
            std::fs::remove_file(&previous)?;
        }
        std::fs::rename(path, previous)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line)?;
    file.write_all(b"\n")?;
    file.flush()
}

pub fn event(id: u64, stage: &str, detail: &str) {
    remember_stage(id, stage, detail);
    let row = serde_json::json!({"time":chrono::Local::now().to_rfc3339(),
        "pid":std::process::id(), "build_id":crate::build_info::BUILD_ID,
        "operation":id, "stage":stage, "detail":detail});
    let line = row.to_string();
    // A closed/redirection pipe must not make the model service panic.
    let _ = writeln!(
        std::io::stdout().lock(),
        "[{}] [{}] op={} {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f%:z"),
        console_label(stage),
        id,
        detail.replace('\n', " ").replace('\r', " ")
    );
    let _lock = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Err(error) = append(
        std::path::Path::new("logs/process-events.jsonl"),
        line.as_bytes(),
    ) {
        eprintln!("[process] journal write failed: {error}");
    }
}

pub struct Operation {
    id: u64,
    started: Instant,
    finished: bool,
}
impl Operation {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn begin(stage: &str, detail: impl AsRef<str>) -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        event(id, stage, detail.as_ref());
        Self {
            id,
            started: Instant::now(),
            finished: false,
        }
    }
    pub fn stage(&self, stage: &str, detail: impl AsRef<str>) {
        event(
            self.id,
            stage,
            &format!(
                "elapsed_ms={} {}",
                self.started.elapsed().as_millis(),
                detail.as_ref()
            ),
        );
    }
    pub fn finish(&mut self, outcome: impl AsRef<str>) {
        self.stage("finished", outcome);
        self.finished = true;
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        if !self.finished {
            self.stage(
                "interrupted",
                "scope exited before completion (error, cancellation or unwind)",
            );
        }
    }
}

// ---- Crash forensics (GitHub #78) ----
// Windows reports every Rust abort (allocation failure, panic while aborting, explicit abort) and the CRT's
// `abort()` as 0xC0000409 from one `__fastfail` site, so neither the exit code nor the fault offset tells them
// apart. The hooks below run on the failing thread just before the process dies and leave one line naming the
// thread, the generation root it was working on, the last journal stage and the process memory. They do not
// allocate: the allocation-failure hook runs with the allocator already refusing.

static LAST_STAGE: Mutex<([u8; 160], usize)> = Mutex::new(([0; 160], 0));
static CRASH_NOTES: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();
thread_local! {
    static CURRENT_ROOT: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
}

fn remember_stage(id: u64, stage: &str, detail: &str) {
    use std::fmt::Write as _;
    let Ok(mut last) = LAST_STAGE.try_lock() else {
        return;
    };
    let mut line = NoteLine::default();
    let _ = write!(
        line,
        "op={id} {stage} {}",
        detail.replace(['\n', '\r'], " ")
    );
    let len = line.len.min(last.0.len());
    last.0[..len].copy_from_slice(&line.buf[..len]);
    last.1 = len;
}

/// The generation root this thread is computing until the guard drops; crash notes name it.
pub struct CurrentRoot(());
impl CurrentRoot {
    pub fn enter(word0: u32, word1: u32) -> Self {
        let _ = CURRENT_ROOT.try_with(|root| root.set((word0, word1)));
        Self(())
    }
}
impl Drop for CurrentRoot {
    fn drop(&mut self) {
        let _ = CURRENT_ROOT.try_with(|root| root.set((0, 0)));
    }
}

/// A fixed-size line buffer: formatting into it never touches the allocator.
struct NoteLine {
    buf: [u8; 640],
    len: usize,
}
impl Default for NoteLine {
    fn default() -> Self {
        Self {
            buf: [0; 640],
            len: 0,
        }
    }
}
impl std::fmt::Write for NoteLine {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let take = text.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

fn utf8_prefix(bytes: &[u8]) -> &str {
    match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default(),
    }
}

/// `(working set, private commit)` in bytes; zeros where the platform query is not wired.
fn process_memory() -> (usize, usize) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) } != 0 {
            return (counters.WorkingSetSize, counters.PagefileUsage);
        }
    }
    (0, 0)
}

fn crash_note(kind: &str, extra: std::fmt::Arguments<'_>) {
    use std::fmt::Write as _;
    let mut line = NoteLine::default();
    let (word0, word1) = CURRENT_ROOT.try_with(|root| root.get()).unwrap_or((0, 0));
    let (working_set, commit) = process_memory();
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let _ = write!(
        line,
        "[crash] unix_ms={unix_ms} kind={kind} pid={} build={} thread={} root={word0}/{word1} working_set_mb={} commit_mb={} ",
        std::process::id(),
        crate::build_info::BUILD_ID,
        std::thread::current().name().unwrap_or("?"),
        working_set >> 20,
        commit >> 20
    );
    if let Ok(last) = LAST_STAGE.try_lock() {
        let _ = write!(line, "last_stage=\"{}\" ", utf8_prefix(&last.0[..last.1]));
    }
    let _ = line.write_fmt(extra);
    let _ = line.write_str("\n");
    let _ = std::io::stderr().write_all(&line.buf[..line.len]);
    if let Some(mut file) = CRASH_NOTES.get() {
        let _ = file.write_all(&line.buf[..line.len]);
    }
}

fn alloc_failed(layout: std::alloc::Layout) {
    crash_note(
        "alloc_failed",
        format_args!("memory allocation of {} bytes failed", layout.size()),
    );
}

#[cfg(windows)]
static NOTED_EXCEPTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
unsafe extern "system" fn first_chance_note(
    info: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, EXCEPTION_STACK_OVERFLOW};
    const CONTINUE_SEARCH: i32 = 0;
    let code = unsafe {
        info.as_ref()
            .and_then(|info| info.ExceptionRecord.as_ref())
            .map(|record| record.ExceptionCode)
    };
    let kind = match code {
        Some(EXCEPTION_STACK_OVERFLOW) => "stack_overflow",
        Some(EXCEPTION_ACCESS_VIOLATION) => "access_violation",
        _ => return CONTINUE_SEARCH,
    };
    if !NOTED_EXCEPTION.swap(true, Ordering::Relaxed) {
        crash_note(
            kind,
            format_args!(
                "first-chance exception code=0x{:08X}",
                code.unwrap_or_default() as u32
            ),
        );
    }
    CONTINUE_SEARCH
}

#[cfg(windows)]
unsafe extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

#[cfg(windows)]
extern "C" fn native_abort_note(_signum: i32) {
    crash_note(
        "abort",
        format_args!("SIGABRT from native code (C/C++ abort, e.g. manifold or rocksdb)"),
    );
}

/// Install the crash notes above. Call once at process start, after [`install_panic_hook`].
pub fn install_crash_handlers() {
    if let Ok(file) = std::fs::create_dir_all("logs").and_then(|_| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("logs/crash-notes.log")
    }) {
        let _ = CRASH_NOTES.set(file);
    }
    std::alloc::set_alloc_error_hook(alloc_failed);
    #[cfg(windows)]
    unsafe {
        const SIGABRT: i32 = 22;
        windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler(
            1,
            Some(first_chance_note),
        );
        signal(SIGABRT, native_abort_note as usize);
    }
}

pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Avoid reacquiring a journal lock if the panic came from diagnostics itself.
        let (word0, word1) = CURRENT_ROOT.try_with(|root| root.get()).unwrap_or((0, 0));
        let (working_set, commit) = process_memory();
        let last_stage = LAST_STAGE
            .try_lock()
            .map(|last| utf8_prefix(&last.0[..last.1]).to_owned())
            .unwrap_or_default();
        let line = serde_json::json!({"time":chrono::Local::now().to_rfc3339(),
            "pid":std::process::id(), "build_id":crate::build_info::BUILD_ID,
            "stage":"panic", "message":info.to_string(),
            "thread":std::thread::current().name().unwrap_or("?"), "root":format!("{word0}/{word1}"),
            "working_set_bytes":working_set, "commit_bytes":commit, "last_stage":last_stage,
            "backtrace":std::backtrace::Backtrace::force_capture().to_string()}).to_string();
        eprintln!("[process] {line}");
        if let Ok(_lock) = LOCK.try_lock() {
            let _ = append(
                std::path::Path::new("logs/process-events.jsonl"),
                line.as_bytes(),
            );
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
        for line in text.lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(LIMIT)
            .unwrap();
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
