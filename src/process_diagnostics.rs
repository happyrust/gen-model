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
// Windows reports every abort as 0xC0000409 (`__fastfail`), so the exit code says no more than "aborted". The
// fault offset says which kind of abort and nothing else: within one build every allocation failure dies at the
// same instruction whatever was being allocated, an explicit `process::abort()` at another one, and the C
// runtime's `abort()` (native C/C++ code, or a Rust panic raised while another one is unwinding) inside
// ucrtbase.dll. None of them points at the code that was running; the test
// `each_way_of_aborting_looks_the_same_from_outside` below shows all four, and
// `a_stack_overflow_does_not_look_like_an_abort` the death that does have an exit code of its own (0xC00000FD).
//
// The hooks below run on the failing thread just before the process dies and leave one line naming the thread, the
// generation root it was working on, the last journal stage and the process memory. They do not allocate: the
// allocation-failure hook runs with the allocator already refusing. An explicit `process::abort()` reaches no hook
// and leaves no line.
//
// The line also carries how much commit the whole machine had left. Every 0xC0000409 of a release build that could
// be traced back (2026-10-16 / 10-19, see docs/analysis/2026-11-12-issue78-crash-root-cause.md) was an allocation of
// a megabyte or two failing while Windows itself was reporting the machine out of virtual memory, with other
// programs holding most of it. `commit_mb` against `sys_commit_free_mb` tells that case (the process is a
// bystander) from a process that ate the memory itself.

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
impl NoteLine {
    /// Close the line. `write_str` keeps this one byte free, so an overlong note still ends where it should
    /// instead of running into the next one.
    fn end(&mut self) {
        if self.len < self.buf.len() {
            self.buf[self.len] = b'\n';
            self.len += 1;
        }
    }

    fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}
impl std::fmt::Write for NoteLine {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        // What does not fit is dropped; the cut falls on a character boundary.
        let mut take = text.len().min(self.buf.len().saturating_sub(self.len + 1));
        while !text.is_char_boundary(take) {
            take -= 1;
        }
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

/// `(commit limit, commit still available, physical memory still available)` of the whole machine, in bytes;
/// zeros where the platform query is not wired. One call into a struct on the stack, so it is safe with the
/// allocator refusing.
fn system_memory() -> (u64, u64, u64) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
            return (
                status.ullTotalPageFile,
                status.ullAvailPageFile,
                status.ullAvailPhys,
            );
        }
    }
    (0, 0, 0)
}

/// One crash note into a caller-owned buffer (the stack-overflow path has no room for a second copy).
fn render_crash_note(line: &mut NoteLine, kind: &str, extra: std::fmt::Arguments<'_>) {
    use std::fmt::Write as _;
    let (word0, word1) = CURRENT_ROOT.try_with(|root| root.get()).unwrap_or((0, 0));
    let (working_set, commit) = process_memory();
    let (sys_commit_limit, sys_commit_free, sys_phys_free) = system_memory();
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let _ = write!(
        line,
        "[crash] unix_ms={unix_ms} kind={kind} pid={} build={} thread={} root={word0}/{word1} working_set_mb={} commit_mb={} sys_commit_free_mb={} sys_commit_limit_mb={} sys_phys_free_mb={} ",
        std::process::id(),
        crate::build_info::BUILD_ID,
        std::thread::current().name().unwrap_or("?"),
        working_set >> 20,
        commit >> 20,
        sys_commit_free >> 20,
        sys_commit_limit >> 20,
        sys_phys_free >> 20
    );
    if let Ok(last) = LAST_STAGE.try_lock() {
        let _ = write!(line, "last_stage=\"{}\" ", utf8_prefix(&last.0[..last.1]));
    }
    let _ = line.write_fmt(extra);
    line.end();
}

fn crash_note(kind: &str, extra: std::fmt::Arguments<'_>) {
    let mut line = NoteLine::default();
    render_crash_note(&mut line, kind, extra);
    let _ = std::io::stderr().write_all(line.bytes());
    if let Some(mut file) = CRASH_NOTES.get() {
        let _ = file.write_all(line.bytes());
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
        format_args!(
            "SIGABRT: C runtime abort() (native C/C++ code such as manifold or rocksdb, or a Rust panic raised while another was unwinding)"
        ),
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

    /// GitHub #78：发布包那几次「同偏移连续崩」，事后查实每一次都是整机可提交内存用尽时一两兆的分配失败。
    /// 笔记里要有整机还剩多少，才分得清进程是被牵连的，还是自己把内存吃光的。
    #[test]
    fn a_crash_note_says_how_much_memory_the_machine_had_left() {
        let mut line = NoteLine::default();
        render_crash_note(
            &mut line,
            "alloc_failed",
            format_args!("memory allocation of {} bytes failed", 1_671_856),
        );
        let text = std::str::from_utf8(line.bytes()).unwrap();
        assert!(text.starts_with("[crash] unix_ms="), "{text}");
        assert!(
            text.ends_with("memory allocation of 1671856 bytes failed\n"),
            "{text}"
        );
        assert_eq!(text.matches('\n').count(), 1, "{text}");
        let field = |name: &str| -> u64 {
            text.split_whitespace()
                .find_map(|word| word.strip_prefix(name))
                .and_then(|value| value.parse().ok())
                .unwrap_or_else(|| panic!("{name} missing in {text}"))
        };
        // 进程自己的两个数还在，整机的三个数是新加的。
        field("working_set_mb=");
        field("commit_mb=");
        let free = field("sys_commit_free_mb=");
        let limit = field("sys_commit_limit_mb=");
        let physical = field("sys_phys_free_mb=");
        if cfg!(windows) {
            assert!(limit > 0 && free > 0 && physical > 0, "{text}");
            assert!(free <= limit, "{text}");
        }
    }

    /// 写满了也要收行：否则下一条笔记会接在这一条后面；截断不落在一个汉字中间。
    #[test]
    fn an_overlong_crash_note_still_ends_its_line_on_a_character_boundary() {
        use std::fmt::Write as _;
        let mut line = NoteLine::default();
        let _ = line.write_str("a");
        for _ in 0..400 {
            let _ = line.write_str("根");
        }
        line.end();
        let text = std::str::from_utf8(line.bytes()).expect("the cut falls between characters");
        assert!(text.ends_with("根\n"));
        assert_eq!(text.matches('\n').count(), 1);
        assert!(line.len < line.buf.len());
        // 收过行之后再写、再收，写到头为止，不越界。
        for _ in 0..4 {
            let _ = line.write_str("tail");
            line.end();
        }
        assert_eq!(line.len, line.buf.len());
    }

    /// 起子进程用的环境变量：值是要它怎么死。
    #[cfg(windows)]
    const DEATH_DEMO: &str = "AIOS_DEATH_DEMO";
    /// 两处分配要的大小：远超任何机器的提交上限，系统一口回绝，不真的占内存。
    #[cfg(windows)]
    const DEMO_VEC_BYTES: usize = 1 << 46;
    #[cfg(windows)]
    const DEMO_RAW_BYTES: usize = 3 << 44;

    /// 手动演示（GitHub #78）：进程在 Windows 上自己 abort——分配失败、直接 `abort()`、展开途中再 panic——
    /// 从外面看退出码都是 `0xC0000409`，光凭它分不出是哪一种。分得出的是两样：崩溃笔记（分配失败一条
    /// `alloc_failed`，展开途中再 panic 经 C 运行库的 `abort()` 一条 `abort`，直接 `abort()` 没有），和系统事件
    /// 日志里的出错偏移（两处不同的分配失败同一个偏移，直接 `abort()` 另一个，C 运行库的落在 ucrtbase.dll 里）。
    /// 每种方式真的弄死一个子进程，所以不随整套跑：
    /// `cargo test --lib -- --ignored --exact process_diagnostics::tests::each_way_of_aborting_looks_the_same_from_outside --nocapture`
    /// 在普通控制台里跑，事件日志里各留一条应用崩溃（`scripts/Get-CrashEvidence.ps1 -Days 1 -Process '^aios_database-'`
    /// 看偏移）；从 IDE / 代理的终端里跑什么都不留——那里起的进程继承了「崩了不上报」的错误模式，后端要是
    /// 这么起的，崩了事件日志里同样没有，只有崩溃笔记和 stderr。
    #[cfg(windows)]
    #[test]
    #[ignore = "手动演示：真的弄死四个子进程"]
    fn each_way_of_aborting_looks_the_same_from_outside() {
        for way in ["alloc-vec", "alloc-raw", "abort", "panic-while-unwinding"] {
            let death = die_in_a_child(way);
            assert_eq!(
                death.exit_code,
                Some(0xC000_0409),
                "{way}\n{}",
                death.stderr
            );
            match way {
                "alloc-vec" | "alloc-raw" => {
                    let bytes = if way == "alloc-vec" {
                        DEMO_VEC_BYTES
                    } else {
                        DEMO_RAW_BYTES
                    };
                    death.assert_one_note(way, "alloc_failed");
                    let failed = format!("memory allocation of {bytes} bytes failed\n");
                    assert!(death.notes.ends_with(&failed), "{way}\n{}", death.notes);
                }
                // 展开途中再 panic，由 C++ 运行库 terminate 到 C 运行库的 `abort()`，走 SIGABRT。
                "panic-while-unwinding" => death.assert_one_note(way, "abort"),
                // `process::abort()` 是一条 `__fastfail`，不过任何进程内的处理函数。
                _ => assert!(death.notes.is_empty(), "{way}\n{}", death.notes),
            }
            // 第一次 panic 由 panic 钩子记进了进程日志；别的死法进程日志里什么都没有。
            assert_eq!(
                death.panic_lines > 0,
                way == "panic-while-unwinding",
                "{way}\n{}",
                death.stderr
            );
        }
    }

    /// 对照（GitHub #78 起初怀疑过栈溢出）：栈溢出从外面看不是 abort——退出码是它自己的 `0xC00000FD`，崩溃笔记是
    /// `stack_overflow`。这条笔记是在溢出的那个线程剩下的一点栈上写的，演示也顺带看着它写得出来。跑法同上，换个测试名。
    #[cfg(windows)]
    #[test]
    #[ignore = "手动演示：真的弄死一个子进程"]
    fn a_stack_overflow_does_not_look_like_an_abort() {
        let way = "stack-overflow";
        let death = die_in_a_child(way);
        assert_eq!(death.exit_code, Some(0xC000_00FD), "{}", death.stderr);
        death.assert_one_note(way, "stack_overflow");
        assert!(
            death
                .notes
                .ends_with("first-chance exception code=0xC00000FD\n"),
            "{}",
            death.notes
        );
        assert_eq!(death.panic_lines, 0, "{}", death.stderr);
    }

    /// 一个子进程按指定的方式死掉之后，从外面看得到的东西。
    #[cfg(windows)]
    struct Death {
        exit_code: Option<u32>,
        stderr: String,
        /// `logs/crash-notes.log` 的全文。
        notes: String,
        /// 进程日志里 panic 钩子记下的行数。
        panic_lines: usize,
    }

    #[cfg(windows)]
    impl Death {
        /// 恰好一条笔记：收了行，是 `kind` 这一类，带整机内存余量，stderr 里也有一份。
        fn assert_one_note(&self, way: &str, kind: &str) {
            let notes = &self.notes;
            assert_eq!(notes.lines().count(), 1, "{way}\n{notes}");
            assert!(notes.ends_with('\n'), "{way}\n{notes}");
            assert!(notes.contains(&format!(" kind={kind} ")), "{way}\n{notes}");
            assert!(notes.contains(" sys_commit_free_mb="), "{way}\n{notes}");
            assert!(
                self.stderr.contains(notes.trim_end()),
                "{way}\n{}",
                self.stderr
            );
        }
    }

    /// 把本测试程序再起一份，让它按 `way` 把自己弄死（`die_the_way_the_parent_asks`），收回它留下的东西。
    #[cfg(windows)]
    fn die_in_a_child(way: &str) -> Death {
        let dir = tempfile::tempdir().unwrap();
        let stderr_path = dir.path().join("stderr.txt");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "process_diagnostics::tests::die_the_way_the_parent_asks",
                "--nocapture",
            ])
            .env(DEATH_DEMO, way)
            .current_dir(dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&stderr_path).unwrap())
            .spawn()
            .unwrap();
        // 机器上要是配了崩溃弹框，子进程会停在那儿等人点：两分钟不退就杀掉，不让演示挂住。
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if started.elapsed().as_secs() > 120 {
                let _ = child.kill();
                panic!("{way}: the child did not exit within two minutes");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        let death = Death {
            exit_code: status.code().map(|code| code as u32),
            stderr: String::from_utf8_lossy(&std::fs::read(&stderr_path).unwrap()).into_owned(),
            notes: std::fs::read_to_string(dir.path().join("logs/crash-notes.log"))
                .unwrap_or_default(),
            panic_lines: std::fs::read_to_string(dir.path().join("logs/process-events.jsonl"))
                .unwrap_or_default()
                .lines()
                .filter(|line| line.contains(r#""stage":"panic""#))
                .count(),
        };
        eprintln!(
            "{way}: exit={:X?} seconds={:.1} crash_notes={} panic_lines={}",
            death.exit_code,
            started.elapsed().as_secs_f32(),
            death.notes.lines().count(),
            death.panic_lines
        );
        for line in death.notes.lines() {
            eprintln!("    {line}");
        }
        death
    }

    /// 上面两条演示起的子进程：按环境变量指定的方式把自己弄死；没有那个变量就什么都不做。
    #[cfg(windows)]
    #[test]
    #[ignore = "只由上面两条手动演示当子进程起"]
    fn die_the_way_the_parent_asks() {
        let Ok(way) = std::env::var(DEATH_DEMO) else {
            return;
        };
        install_panic_hook();
        install_crash_handlers();
        match way.as_str() {
            "alloc-vec" => drop(std::hint::black_box(Vec::<u8>::with_capacity(
                DEMO_VEC_BYTES,
            ))),
            "alloc-raw" => unsafe {
                let layout = std::alloc::Layout::from_size_align(DEMO_RAW_BYTES, 8).unwrap();
                let block = std::alloc::alloc(layout);
                if block.is_null() {
                    std::alloc::handle_alloc_error(layout);
                }
                std::alloc::dealloc(block, layout);
            },
            "abort" => std::process::abort(),
            "stack-overflow" => {
                // 每一层都留一块真的用到的局部数组，递归压不平，栈一定用完。
                fn dig(depth: u64) -> u64 {
                    let pad = std::hint::black_box([depth; 64]);
                    if depth == u64::MAX {
                        0
                    } else {
                        dig(depth + 1).wrapping_add(pad[63])
                    }
                }
                std::hint::black_box(dig(0));
            }
            "panic-while-unwinding" => {
                struct PanicsWhenDropped;
                impl Drop for PanicsWhenDropped {
                    fn drop(&mut self) {
                        panic!("second panic, while unwinding from the first");
                    }
                }
                let _guard = PanicsWhenDropped;
                panic!("first panic");
            }
            other => panic!("unknown {DEATH_DEMO}={other}"),
        }
        unreachable!("{way} should have ended the process");
    }
}
