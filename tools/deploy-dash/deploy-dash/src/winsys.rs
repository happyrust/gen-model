//! Windows 小工具：父控制台、单实例、Job Object、监听端口表、进程内存、窗口截图、
//! 目录对话框自动化（自检用）、OEM 代码页解码、本地时间。
//! 从 deploy-ui 沿用；面板加了：截图按 DWM 可见边框裁剪、本地时区偏移。
#![allow(dead_code)]
use std::collections::HashSet;
use std::ffi::c_void;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, HWND,
    INVALID_HANDLE_VALUE, LPARAM, RECT, SYSTEMTIME,
};
use windows_sys::Win32::Globalization::{GetOEMCP, MultiByteToWideChar};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HBITMAP, HDC, ReleaseDC, SRCCOPY,
    SelectObject,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_CHAR, FILE_TYPE_UNKNOWN, GetFileType,
    OPEN_EXISTING,
};
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    SetStdHandle,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, GetCurrentProcessId};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetDlgCtrlID, GetForegroundWindow,
    GetParent, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IDCANCEL, IDOK, IsIconic,
    IsWindow, IsWindowVisible, PW_RENDERFULLCONTENT, PostMessageW, SW_RESTORE, SW_SHOWNOACTIVATE,
    SendMessageW, SetForegroundWindow, ShowWindow, WM_COMMAND, WM_SETTEXT,
};
use windows_sys::core::BOOL;

pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 本地时区相对 UTC 的秒数（GetLocalTime 和 GetSystemTime 的差）。
pub fn local_utc_offset_secs() -> i64 {
    let mut l: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut u: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe {
        GetLocalTime(&mut l);
        GetSystemTime(&mut u);
    }
    let secs = |s: &SYSTEMTIME| {
        days_from_civil(s.wYear as i64, s.wMonth as i64, s.wDay as i64) * 86_400 + s.wHour as i64 * 3600 + s.wMinute as i64 * 60 + s.wSecond as i64
    };
    let diff = secs(&l) - secs(&u);
    // 两次调用之间可能跨秒，取最近的一刻钟
    ((diff as f64 / 900.0).round() as i64) * 900
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmGetWindowAttribute(hwnd: HWND, attr: u32, pv: *mut c_void, cb: u32) -> i32;
}

const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;

/// 窗口看得见的边框（GetWindowRect 含系统的不可见缩放边）。
fn visible_frame(hwnd: HWND) -> Option<RECT> {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    let hr = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut c_void, size_of::<RECT>() as u32) };
    (hr == 0 && r.right > r.left && r.bottom > r.top).then_some(r)
}

pub fn local_time_hms() -> String {
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    )
}

/// `windows_subsystem = "windows"` 的程序没有自己的控制台。标准输出已被重定向（管道、文件）时
/// 原样使用；否则挂到父进程（cmd / PowerShell）的控制台上，命令行模式就能看到输出。
pub fn attach_parent_console() -> &'static str {
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        let kind = if out.is_null() || out == INVALID_HANDLE_VALUE {
            FILE_TYPE_UNKNOWN
        } else {
            GetFileType(out)
        };
        if kind != FILE_TYPE_UNKNOWN && kind != FILE_TYPE_CHAR {
            return "沿用已重定向的标准输出";
        }
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return "没有父控制台（双击启动），只写日志文件";
        }
        if kind == FILE_TYPE_CHAR {
            return "已挂到父控制台（沿用继承的控制台句柄）";
        }
        let name = to_wide("CONOUT$");
        let con = CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        );
        if con == INVALID_HANDLE_VALUE {
            return "挂上了父控制台但打不开 CONOUT$";
        }
        SetStdHandle(STD_OUTPUT_HANDLE, con);
        SetStdHandle(STD_ERROR_HANDLE, con);
        "已挂到父控制台（重新打开 CONOUT$）"
    }
}

/// 命名互斥量做单实例。拿不到（已有实例）返回 None。
pub struct InstanceGuard(HANDLE);
unsafe impl Send for InstanceGuard {}
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub fn acquire_single_instance(name: &str) -> anyhow::Result<Option<InstanceGuard>> {
    let wide = to_wide(name);
    unsafe {
        let h = CreateMutexW(null(), 0, wide.as_ptr());
        if h.is_null() {
            bail!("CreateMutexW 失败：{}", GetLastError());
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            return Ok(None);
        }
        Ok(Some(InstanceGuard(h)))
    }
}

/// 第二个实例把已有窗口提到前台（按窗口标题找）。
pub enum Activation {
    NotFound,
    Foreground,
    /// 找到了，但系统前台锁不允许切前台：只有用户刚操作过的进程（比如双击启动的）才有这个权限
    Refused,
}

/// 按 GPUI 窗口类名 + 标题找另一个实例的窗口，还原并切到前台。
pub fn activate_window_by_title(title: &str) -> Activation {
    let me = unsafe { GetCurrentProcessId() };
    let Some(w) = top_windows()
        .into_iter()
        .find(|w| w.pid != me && w.class == "Zed::Window" && w.title == title)
    else {
        return Activation::NotFound;
    };
    unsafe {
        let hwnd = w.hwnd as HWND;
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd) != 0 { Activation::Foreground } else { Activation::Refused }
    }
}

/// 设了 KILL_ON_JOB_CLOSE 的 Job：句柄关掉（包括本进程崩溃、被结束）时，里面的子进程一起被结束。
pub struct Job(HANDLE);
unsafe impl Send for Job {}
unsafe impl Sync for Job {}
impl Job {
    pub fn kill_on_close() -> anyhow::Result<Self> {
        unsafe {
            let h = CreateJobObjectW(null(), null());
            if h.is_null() {
                bail!("CreateJobObjectW 失败：{}", GetLastError());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let e = GetLastError();
                CloseHandle(h);
                bail!("SetInformationJobObject 失败：{e}");
            }
            Ok(Self(h))
        }
    }

    pub fn assign(&self, child: &std::process::Child) -> anyhow::Result<()> {
        use std::os::windows::io::AsRawHandle;
        unsafe {
            if AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) == 0 {
                bail!("AssignProcessToJobObject 失败：{}", GetLastError());
            }
        }
        Ok(())
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// 查 TCP 监听表（IPv4 + IPv6）。判断端口是否被占用要看监听表：别人监听 0.0.0.0:P 时，
/// 再绑 127.0.0.1:P 在 Windows 上照样能成功，拿绑定试探会误判为可用。
pub fn listening_ports() -> HashSet<u16> {
    listening_port_owners().into_keys().collect()
}

/// 监听表：端口 → 监听它的进程号（同一端口有多个监听者时取先查到的那个）。
pub fn listening_port_owners() -> std::collections::HashMap<u16, u32> {
    let mut ports = std::collections::HashMap::new();
    for af in [AF_INET as u32, AF_INET6 as u32] {
        let mut size = 0u32;
        unsafe { GetExtendedTcpTable(null_mut(), &mut size, 0, af, TCP_TABLE_OWNER_PID_LISTENER, 0) };
        if size == 0 {
            continue;
        }
        let mut buf = vec![0u8; size as usize + 4096];
        let mut size = buf.len() as u32;
        let rc = unsafe {
            GetExtendedTcpTable(buf.as_mut_ptr() as *mut c_void, &mut size, 0, af, TCP_TABLE_OWNER_PID_LISTENER, 0)
        };
        if rc != 0 {
            continue;
        }
        let n = u32::from_ne_bytes(buf[0..4].try_into().unwrap()) as usize;
        if af == AF_INET as u32 {
            let base = std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table);
            for i in 0..n {
                let off = base + i * size_of::<MIB_TCPROW_OWNER_PID>();
                let row: MIB_TCPROW_OWNER_PID =
                    unsafe { std::ptr::read_unaligned(buf.as_ptr().add(off) as *const _) };
                ports.entry(u16::from_be(row.dwLocalPort as u16)).or_insert(row.dwOwningPid);
            }
        } else {
            let base = std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table);
            for i in 0..n {
                let off = base + i * size_of::<MIB_TCP6ROW_OWNER_PID>();
                let row: MIB_TCP6ROW_OWNER_PID =
                    unsafe { std::ptr::read_unaligned(buf.as_ptr().add(off) as *const _) };
                ports.entry(u16::from_be(row.dwLocalPort as u16)).or_insert(row.dwOwningPid);
            }
        }
    }
    ports
}

/// 一个进程此刻的采样：活着没有、累计 CPU 时间（100 纳秒）、内存、创建时刻。
#[derive(Clone, Copy, Debug)]
pub struct ProcSample {
    pub alive: bool,
    pub cpu_100ns: u64,
    pub working_set: u64,
    pub private_bytes: u64,
    pub created: Option<std::time::SystemTime>,
}

fn filetime_u64(ft: &windows_sys::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// 按进程号采样（只要 PROCESS_QUERY_LIMITED_INFORMATION）。打不开（不在了、权限更高）回 None。
pub fn proc_sample(pid: u32) -> Option<ProcSample> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    if pid == 0 {
        return None;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut code = 0u32;
        let alive = GetExitCodeProcess(h, &mut code) != 0 && code == 259;
        let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
        let times = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0;
        let mut m: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        m.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let mem = K32GetProcessMemoryInfo(h, &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS, m.cb) != 0;
        CloseHandle(h);
        // FILETIME 是 1601-01-01 起的 100 纳秒数
        let created = times.then(|| {
            let ticks = filetime_u64(&c);
            let unix_100ns = ticks.saturating_sub(116_444_736_000_000_000);
            std::time::UNIX_EPOCH + Duration::from_nanos(unix_100ns.saturating_mul(100))
        });
        Some(ProcSample {
            alive,
            cpu_100ns: if times { filetime_u64(&k) + filetime_u64(&u) } else { 0 },
            working_set: if mem { m.WorkingSetSize as u64 } else { 0 },
            private_bytes: if mem { m.PrivateUsage as u64 } else { 0 },
            created,
        })
    }
}

/// 某个路径所在盘的（可用 GB, 总 GB）。
pub fn disk_free_gb(path: &Path) -> Option<(f64, f64)> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide = to_wide(&path.display().to_string());
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free) };
    (ok != 0).then(|| (avail as f64 / 1073741824.0, total as f64 / 1073741824.0))
}

/// （可提交内存余量 GB, 物理内存总量 GB）。余量和 stack.ps1 用的 FreeVirtualMemory 同一个量。
pub fn memory_status_gb() -> (f64, f64) {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    unsafe {
        let mut s: MEMORYSTATUSEX = std::mem::zeroed();
        s.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut s) == 0 {
            return (0.0, 0.0);
        }
        (s.ullAvailPageFile as f64 / 1073741824.0, s.ullTotalPhys as f64 / 1073741824.0)
    }
}

/// 当前进程的（私有提交 MB, 工作集 MB）。
pub fn process_memory_mb() -> (f64, f64) {
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            c.cb,
        ) == 0
        {
            return (0.0, 0.0);
        }
        (
            c.PrivateUsage as f64 / 1048576.0,
            c.WorkingSetSize as f64 / 1048576.0,
        )
    }
}

#[derive(Clone, Debug)]
pub struct WinInfo {
    pub hwnd: usize,
    pub pid: u32,
    pub class: String,
    pub title: String,
    pub visible: bool,
}

fn win_info(hwnd: HWND) -> WinInfo {
    let mut pid = 0u32;
    let mut class = [0u16; 256];
    let mut title = [0u16; 512];
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
        let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
        let m = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32).max(0) as usize;
        WinInfo {
            hwnd: hwnd as usize,
            pid,
            class: String::from_utf16_lossy(&class[..n]),
            title: String::from_utf16_lossy(&title[..m]),
            visible: IsWindowVisible(hwnd) != 0,
        }
    }
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = unsafe { &mut *(lparam as *mut Vec<WinInfo>) };
    out.push(win_info(hwnd));
    1
}

pub fn top_windows() -> Vec<WinInfo> {
    let mut v: Vec<WinInfo> = Vec::new();
    unsafe { EnumWindows(Some(collect_window), &mut v as *mut _ as LPARAM) };
    v
}

pub fn child_windows(parent: usize) -> Vec<WinInfo> {
    let mut v: Vec<WinInfo> = Vec::new();
    unsafe { EnumChildWindows(parent as HWND, Some(collect_window), &mut v as *mut _ as LPARAM) };
    v
}

const CONSOLE_CLASSES: [&str; 2] = ["ConsoleWindowClass", "CASCADIA_HOSTING_WINDOW_CLASS"];

/// 桌面上当前可见的控制台窗口（检查起子进程时有没有黑窗闪一下）。
pub fn visible_console_windows() -> HashSet<usize> {
    top_windows()
        .into_iter()
        .filter(|w| w.visible && CONSOLE_CLASSES.contains(&w.class.as_str()))
        .map(|w| w.hwnd)
        .collect()
}

/// 本进程的 GPUI 主窗口（类名 Zed::Window）。
pub fn own_main_window() -> Option<WinInfo> {
    let pid = unsafe { GetCurrentProcessId() };
    top_windows()
        .into_iter()
        .find(|w| w.pid == pid && w.visible && w.class == "Zed::Window")
}

pub fn main_window_minimized() -> bool {
    own_main_window().is_some_and(|w| unsafe { IsIconic(w.hwnd as HWND) } != 0)
}

/// 自检测帧率前用：主窗口被最小化了就不抢焦点地还原。返回是否做了还原。
pub fn restore_main_window_no_activate() -> bool {
    match own_main_window() {
        Some(w) if unsafe { IsIconic(w.hwnd as HWND) } != 0 => {
            unsafe { ShowWindow(w.hwnd as HWND, SW_SHOWNOACTIVATE) };
            true
        }
        _ => false,
    }
}

/// 当前前台窗口的进程号和类名（不取标题，避免把用户别的窗口内容写进日志）。
pub fn foreground_window() -> (u32, String) {
    let h = unsafe { GetForegroundWindow() };
    if h.is_null() {
        return (0, String::new());
    }
    let info = win_info(h);
    (info.pid, info.class)
}

fn window_rect(hwnd: usize) -> RECT {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd as HWND, &mut r) };
    r
}

fn ctrl_chain(hwnd: usize) -> String {
    let mut parts = Vec::new();
    let mut h = hwnd as HWND;
    for _ in 0..4 {
        if h.is_null() {
            break;
        }
        let info = win_info(h);
        parts.push(format!("{}#{:#x}", info.class, unsafe { GetDlgCtrlID(h) }));
        h = unsafe { GetParent(h) };
    }
    parts.join(" ← ")
}

unsafe fn read_bits(dc: HDC, bmp: HBITMAP, w: i32, h: i32) -> Vec<u8> {
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = w;
    info.bmiHeader.biHeight = -h;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    let mut buf = vec![0u8; (w * h * 4) as usize];
    unsafe {
        GetDIBits(dc, bmp, 0, h as u32, buf.as_mut_ptr() as *mut c_void, &mut info, DIB_RGB_COLORS)
    };
    buf
}

/// 窗口截图存 PNG：先 PrintWindow(PW_RENDERFULLCONTENT)（被遮挡也能截），几乎全黑再退回屏幕拷贝。
pub fn capture_window_png(hwnd: usize, path: &Path) -> anyhow::Result<(u32, u32, &'static str)> {
    let hwnd = hwnd as HWND;
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut r) == 0 {
            bail!("GetWindowRect 失败：{}", GetLastError());
        }
        let (w, h) = ((r.right - r.left).max(1), (r.bottom - r.top).max(1));
        let screen = GetDC(null_mut());
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp);
        let ok = PrintWindow(hwnd, mem, PW_RENDERFULLCONTENT);
        SelectObject(mem, old);
        let mut pixels = read_bits(mem, bmp, w, h);
        let mut method = "PrintWindow";
        let dark = pixels.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 <= 12).count();
        if ok == 0 || dark * 100 > (w * h) as usize * 97 {
            let old = SelectObject(mem, bmp);
            BitBlt(mem, 0, 0, w, h, screen, r.left, r.top, SRCCOPY);
            SelectObject(mem, old);
            pixels = read_bits(mem, bmp, w, h);
            method = "屏幕拷贝";
        }
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        // 裁到 DWM 的可见边框：去掉 GetWindowRect 里那圈系统的不可见缩放边
        let (mut out_w, mut out_h) = (w, h);
        if let Some(f) = visible_frame(hwnd) {
            let ox = (f.left - r.left).clamp(0, w - 1);
            let oy = (f.top - r.top).clamp(0, h - 1);
            let cw = (f.right - f.left).min(w - ox);
            let ch = (f.bottom - f.top).min(h - oy);
            if cw > 0 && ch > 0 && (cw, ch) != (w, h) {
                let mut cropped = Vec::with_capacity((cw * ch * 4) as usize);
                for row in oy..oy + ch {
                    let start = ((row * w + ox) * 4) as usize;
                    cropped.extend_from_slice(&pixels[start..start + (cw * 4) as usize]);
                }
                pixels = cropped;
                out_w = cw;
                out_h = ch;
                method = if method == "PrintWindow" { "PrintWindow，按 DWM 可见边框裁剪" } else { "屏幕拷贝，按 DWM 可见边框裁剪" };
            }
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        image::save_buffer(path, &pixels, out_w as u32, out_h as u32, image::ExtendedColorType::Rgba8)
            .with_context(|| format!("保存截图 {}", path.display()))?;
        Ok((out_w as u32, out_h as u32, method))
    }
}

/// 自检用：等本进程弹出的系统目录对话框（类名 #32770），截图后把路径填进「文件夹」框并按确定。
/// 选择对话框本身是系统的 IFileOpenDialog，GPUI 通过 `cx.prompt_for_paths` 调起。
pub fn automate_folder_dialog(target: &str, shot: Option<&Path>, timeout: Duration) -> anyhow::Result<String> {
    let pid = unsafe { GetCurrentProcessId() };
    let deadline = Instant::now() + timeout;
    let dlg = loop {
        if let Some(w) = top_windows()
            .into_iter()
            .find(|w| w.pid == pid && w.visible && w.class == "#32770")
        {
            break w;
        }
        if Instant::now() > deadline {
            bail!("{} 秒内没等到目录对话框", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    std::thread::sleep(Duration::from_millis(800));
    let mut log = format!("对话框标题「{}」", dlg.title);
    if let Some(p) = shot {
        match capture_window_png(dlg.hwnd, p) {
            Ok((w, h, m)) => log += &format!("，截图 {w}x{h}（{m}）"),
            Err(e) => log += &format!("，截图失败：{e}"),
        }
    }
    let children = child_windows(dlg.hwnd);
    // 地址栏里也可能有 Edit；「文件夹:」输入框是对话框底部那个，取位置最靠下的可见 Edit
    let Some(edit) = children
        .iter()
        .filter(|c| c.class == "Edit" && c.visible)
        .max_by_key(|c| window_rect(c.hwnd).top)
    else {
        let tree: Vec<String> = children
            .iter()
            .filter(|c| c.visible)
            .map(|c| format!("{}#{:#x}", c.class, unsafe { GetDlgCtrlID(c.hwnd as HWND) }))
            .collect();
        unsafe { PostMessageW(dlg.hwnd as HWND, WM_COMMAND, IDCANCEL as usize, 0) };
        bail!("{log}；找不到「文件夹」输入框（已按取消），可见子窗口：{}", tree.join(", "));
    };
    log += &format!("，输入框 {}", ctrl_chain(edit.hwnd));
    let wide = to_wide(target);
    unsafe {
        SendMessageW(edit.hwnd as HWND, WM_SETTEXT, 0, wide.as_ptr() as LPARAM);
        PostMessageW(dlg.hwnd as HWND, WM_COMMAND, IDOK as usize, 0);
    }
    for round in 0..3 {
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(150));
            if unsafe { IsWindow(dlg.hwnd as HWND) } == 0 {
                return Ok(log + &format!("，按了 {} 次确定后关闭", round + 1));
            }
        }
        unsafe { PostMessageW(dlg.hwnd as HWND, WM_COMMAND, IDOK as usize, 0) };
    }
    unsafe { PostMessageW(dlg.hwnd as HWND, WM_COMMAND, IDCANCEL as usize, 0) };
    bail!("{log}，按确定后对话框仍未关闭（已按取消）")
}

pub fn oem_code_page() -> u32 {
    unsafe { GetOEMCP() }
}

/// 按系统 OEM 代码页（中文系统默认 936；开了「UTF-8 全球语言支持」则是 65001）解码子进程输出。
pub fn decode_oem(bytes: &[u8]) -> String {
    decode_cp(bytes, unsafe { GetOEMCP() })
}

pub fn decode_cp(bytes: &[u8], cp: u32) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    unsafe {
        let n = MultiByteToWideChar(cp, 0, bytes.as_ptr(), bytes.len() as i32, null_mut(), 0);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut buf = vec![0u16; n as usize];
        MultiByteToWideChar(cp, 0, bytes.as_ptr(), bytes.len() as i32, buf.as_mut_ptr(), n);
        String::from_utf16_lossy(&buf)
    }
}
