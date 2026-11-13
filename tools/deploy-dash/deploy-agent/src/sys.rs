//! 几个 Win32 小调用（手写 extern，不给 windows-sys 加特性：加了会让 tokio 等一串基础包连带重编）。
//! 单实例：按安装根取名的命名互斥量 `Local\AiosDeployDashAgent-<根路径哈希>`（同 `winsys::acquire_single_instance`）。

use std::path::Path;

use sha2::{Digest, Sha256};

/// 互斥量名字：安装根的完整路径（小写、斜杠统一）取 SHA-256 前 16 个十六进制字。
pub fn instance_mutex_name(root: &Path) -> String {
    let full = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let text = full.display().to_string().replace('/', "\\").trim_start_matches("\\\\?\\").to_lowercase();
    let digest = Sha256::digest(text.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("Local\\AiosDeployDashAgent-{hex}")
}

/// 拿着它就是「这一个」；放掉（drop）就让出来。
pub struct InstanceGuard {
    #[cfg(windows)]
    handle: *mut std::ffi::c_void,
}

// SAFETY：句柄只在 drop 时关一次，不跨线程同时使用。
unsafe impl Send for InstanceGuard {}

/// 拿单实例锁。`Ok(None)` = 已经有一个在跑了。
pub fn acquire_single_instance(name: &str) -> std::io::Result<Option<InstanceGuard>> {
    imp::acquire(name)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use super::InstanceGuard;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attrs: *const c_void, initial_owner: i32, name: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    const ERROR_ALREADY_EXISTS: u32 = 183;

    pub fn acquire(name: &str) -> std::io::Result<Option<InstanceGuard>> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY：名字是以 0 结尾的 UTF-16；返回的句柄由 InstanceGuard 负责关。
        unsafe {
            let handle = CreateMutexW(std::ptr::null(), 0, wide.as_ptr());
            let err = GetLastError();
            if handle.is_null() {
                return Err(std::io::Error::from_raw_os_error(err as i32));
            }
            if err == ERROR_ALREADY_EXISTS {
                CloseHandle(handle);
                return Ok(None);
            }
            Ok(Some(InstanceGuard { handle }))
        }
    }

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            // SAFETY：句柄来自 CreateMutexW，只关这一次。
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::InstanceGuard;

    pub fn acquire(_name: &str) -> std::io::Result<Option<InstanceGuard>> {
        Ok(Some(InstanceGuard {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_is_refused_until_first_is_dropped() {
        let name = format!("Local\\AiosDeployDashAgent-test-{}", std::process::id());
        let first = acquire_single_instance(&name).unwrap();
        assert!(first.is_some());
        assert!(acquire_single_instance(&name).unwrap().is_none(), "第二个应当拿不到");
        drop(first);
        assert!(acquire_single_instance(&name).unwrap().is_some(), "第一个放掉之后能拿到");
    }

    #[test]
    fn mutex_name_depends_on_root_only() {
        let a = instance_mutex_name(Path::new("D:\\x\\pkg"));
        let b = instance_mutex_name(Path::new("d:/X/PKG"));
        assert_eq!(a, b);
        assert!(a.starts_with("Local\\AiosDeployDashAgent-"));
        assert_eq!(a.len(), "Local\\AiosDeployDashAgent-".len() + 16);
    }
}
