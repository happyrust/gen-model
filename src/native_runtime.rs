//! Guard the minimum STL runtime used by the current MSVC 14.44 geometry build.
//! This prevents known old-runtime ABI failures; it is not a geometry validity check.
fn compatible(version: [u16; 4]) -> bool {
    (version[0], version[1]) >= (14, 44)
}

pub fn ensure_geometry_runtime() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        static CHECK: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
        CHECK
            .get_or_init(check_windows)
            .clone()
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

#[cfg(windows)]
fn check_windows() -> Result<(), String> {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, size: u32) -> u32;
    }
    #[link(name = "version")]
    unsafe extern "system" {
        fn GetFileVersionInfoSizeW(path: *const u16, handle: *mut u32) -> u32;
        fn GetFileVersionInfoW(path: *const u16, handle: u32, size: u32, data: *mut c_void) -> i32;
        fn VerQueryValueW(
            data: *const c_void,
            block: *const u16,
            value: *mut *mut c_void,
            len: *mut u32,
        ) -> i32;
    }
    let name: Vec<u16> = "msvcp140.dll\0".encode_utf16().collect();
    // The process loader has already resolved directly linked CRT imports.
    let module = unsafe { GetModuleHandleW(name.as_ptr()) };
    if module.is_null() {
        crate::process_diagnostics::event(
            0,
            "native.runtime",
            "MSVCP140.dll 未加载：跳过动态 STL 版本检查（不主动加载 DLL）",
        );
        return Ok(());
    }
    let mut path = vec![0u16; 32768];
    let n = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) } as usize;
    if n == 0 || n >= path.len() {
        return Err("读取已加载 MSVCP140.dll 路径失败".into());
    }
    path.truncate(n + 1);
    path[n] = 0;
    let display = String::from_utf16_lossy(&path[..n]);
    let mut handle = 0;
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut handle) };
    if size == 0 {
        return Err(format!("读取运行库版本失败: {display}"));
    }
    let mut data = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return Err(format!("读取运行库版本资源失败: {display}"));
    }
    let mut value = std::ptr::null_mut();
    let mut len = 0;
    if unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            [92u16, 0].as_ptr(),
            &mut value,
            &mut len,
        )
    } == 0
        || value.is_null()
        || len < 52
    {
        return Err(format!("运行库固定版本信息缺失: {display}"));
    }
    // VS_FIXEDFILEINFO starts with signature, struct version, file version MS/LS.
    let words = unsafe { std::ptr::read_unaligned(value.cast::<[u32; 13]>()) };
    if words[0] != 0xfeef04bd {
        return Err("运行库版本资源签名无效".into());
    }
    let version = [
        (words[2] >> 16) as u16,
        words[2] as u16,
        (words[3] >> 16) as u16,
        words[3] as u16,
    ];
    let text = format!(
        "{}.{}.{}.{}",
        version[0], version[1], version[2], version[3]
    );
    crate::process_diagnostics::event(
        0,
        "native.runtime",
        &format!("path={display} version={text} minimum=14.44"),
    );
    if !compatible(version) {
        let error = format!(
            "C++ 几何运行库版本过旧：已加载 {display} ({text})，当前几何构建要求 MSVC 14.44 或更新。请部署与操作系统兼容的配套 x64 VC++ 运行库；旧系统需匹配工具链重新构建。已停止本次几何调用以避免旧 STL 访问违例。"
        );
        crate::process_diagnostics::event(0, "native.runtime.failed", &error);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_field_runtime_and_accept_build_runtime() {
        assert!(!compatible([14, 26, 28914, 0]));
        assert!(!compatible([14, 40, 33810, 0]));
        assert!(compatible([14, 44, 35211, 0]));
        assert!(compatible([14, 50, 0, 0]));
    }
    #[test]
    fn inspect_loaded_runtime_on_build_machine() {
        ensure_geometry_runtime().unwrap();
    }
}
