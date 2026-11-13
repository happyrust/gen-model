//! `GET /v1/fs/probe`：只探「在不在 / 是不是目录 / 读不读得了」。不列目录、不回文件名和大小。
//! 读不读得了：目录用 `FILE_FLAG_BACKUP_SEMANTICS` 以读权限打开句柄（要的是「列目录」权限，但不真去列），文件就以读打开。
//! 不收 UNC（`\\服务器\共享`）和设备路径（`\\?\`、`\\.\`）：探 UNC 会让代理去连别的机器、还可能把账户凭据交出去。

use std::path::Path;

/// 探的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    pub exists: bool,
    pub is_dir: bool,
    pub readable: bool,
}

/// 路径合不合规矩；不合规矩回原因。
pub fn check_path(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("path 不能是空的".into());
    }
    if path.len() > 1024 {
        return Err("path 太长".into());
    }
    if path.chars().any(|c| c.is_control()) {
        return Err("path 里有控制字符".into());
    }
    if path.starts_with("\\\\") || path.starts_with("//") {
        return Err("不探网络路径（\\\\服务器\\共享）和设备路径".into());
    }
    if !Path::new(path).is_absolute() {
        return Err("要写完整路径，例如 D:\\AVEVA\\Projects".into());
    }
    Ok(())
}

pub fn probe(path: &str) -> Probe {
    let p = Path::new(path);
    match std::fs::metadata(p) {
        Ok(m) => Probe { exists: true, is_dir: m.is_dir(), readable: can_open(p, m.is_dir()) },
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Probe { exists: true, is_dir: false, readable: false },
        Err(_) => Probe { exists: false, is_dir: false, readable: false },
    }
}

#[cfg(windows)]
fn can_open(p: &Path, is_dir: bool) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    if is_dir {
        o.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    o.open(p).is_ok()
}

#[cfg(not(windows))]
fn can_open(p: &Path, is_dir: bool) -> bool {
    if is_dir { std::fs::read_dir(p).is_ok() } else { std::fs::File::open(p).is_ok() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_dir_file_and_missing() {
        let dir = std::env::temp_dir().join(format!("deploy-agent-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        assert_eq!(probe(&dir.display().to_string()), Probe { exists: true, is_dir: true, readable: true });
        assert_eq!(probe(&f.display().to_string()), Probe { exists: true, is_dir: false, readable: true });
        assert_eq!(probe(&dir.join("没有").display().to_string()), Probe { exists: false, is_dir: false, readable: false });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_unc_relative_and_control_chars() {
        assert!(check_path("\\\\server\\share").is_err());
        assert!(check_path("//server/share").is_err());
        assert!(check_path("\\\\?\\C:\\x").is_err());
        assert!(check_path("relative\\dir").is_err());
        assert!(check_path("C:x").is_err());
        assert!(check_path("C:\\a\nb").is_err());
        assert!(check_path("").is_err());
        assert!(check_path("D:\\AVEVA\\Projects").is_ok());
    }
}
