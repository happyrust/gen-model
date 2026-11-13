//! 升级的落盘与判定（设计 5.1 路径把关、5.2 逐个文件怎么判、4.4 会话、5.8 布局）。
//!
//! 这一层只管「算什么」，不管 HTTP：`server.rs` 把请求解析好交进来。
//!
//! 落盘布局（`<根>\run\gui\upgrade\`）：
//!
//! ```text
//! blobs\<sha256>                     收齐并核对过的内容（跨会话共用）
//! blobs\<sha256>.part                没收完的
//! sessions\<会话号>\manifest.json    收下的清单原文
//! sessions\<会话号>\state.json       会话状态
//! history.jsonl                      每次升级、换回一行（U2 写）
//! inventory-cache.json               路径、大小、改动时间 → sha256
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use deploy_dash_proto as proto;

/// 一个内容（按 sha256 去重）在磁盘上的状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlobState {
    /// 收齐并核对过的字节数（= 清单里的 bytes 时算齐）。
    pub have: u64,
    pub complete: bool,
}

/// 清单里的一条路径过不过关（设计 5.1）。
///
/// 不合格时返回「第几条 + 为什么」，调用方整份拒绝。
pub fn check_rel_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("路径是空的".into());
    }
    if path.len() > 240 {
        return Err(format!("整条超过 240 个字符（{} 个）", path.len()));
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err("是绝对路径".into());
    }
    if path.contains('\\') {
        return Err("用的是反斜杠：清单里的路径一律用 / 分隔".into());
    }
    if path.contains(':') {
        return Err("带冒号（盘符或数据流）".into());
    }
    for seg in path.split('/') {
        if seg.is_empty() {
            return Err("有一段是空的（连续的 /）".into());
        }
        if seg == "." || seg == ".." {
            return Err("有 . 或 .. 这一段".into());
        }
        if seg.ends_with(' ') || seg.ends_with('.') {
            return Err("有一段以空格或点结尾".into());
        }
        if seg.chars().any(|c| c.is_control()) {
            return Err("有控制字符".into());
        }
        if seg.chars().any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*')) {
            return Err("有 < > \" | ? * 之一".into());
        }
        if is_device_name(seg) {
            return Err(format!("「{seg}」是设备名"));
        }
    }
    let lower = path.to_ascii_lowercase();
    if lower == "stack.toml" {
        return Err("stack.toml 是用户自己的配置，升级不碰".into());
    }
    if lower == "release.json" {
        return Err("release.json 单独处理（换文件之后才放），不进 files".into());
    }
    for dir in ["run/", "logs/", "data/", "work/", "tmp/"] {
        if lower.starts_with(dir) {
            return Err(format!("在 {dir} 下面：这些是运行目录，升级不碰"));
        }
    }
    Ok(())
}

/// 去掉扩展名后是不是 Windows 设备名（不分大小写）。
fn is_device_name(seg: &str) -> bool {
    let stem = seg.split('.').next().unwrap_or(seg).trim_end_matches(' ').to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit() && stem.as_bytes()[3] != b'0')
}

/// 整份清单的路径把关。返回第一处不合格（设计 5.1：整份拒绝）。
pub fn check_manifest_paths(files: &[proto::UpgradeFile]) -> Result<(), String> {
    let mut seen: HashMap<String, &str> = HashMap::new();
    for f in files {
        check_rel_path(&f.path).map_err(|e| format!("{}：{e}", f.path))?;
        let lower = f.path.to_ascii_lowercase();
        if let Some(prev) = seen.insert(lower, &f.path) {
            return Err(format!("{} 和 {prev} 不分大小写时相同", f.path));
        }
        if f.sha256.len() != 64 || !f.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
            return Err(format!("{}：sha256 要是 64 位小写十六进制", f.path));
        }
    }
    Ok(())
}

/// 判定结果（设计 5.2 那张表）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    pub add: Vec<String>,
    pub replace: Vec<String>,
    pub delete: Vec<String>,
    /// 本机改过、包也没改它：留着不动。
    pub kept_modified: Vec<String>,
    /// 本机改过、包也改了它：留着本机的，新版另存到 `conflicts\<路径>`。
    pub conflicts: Vec<String>,
    /// 盘上没有（已装清单里有，但文件不在了）。
    pub missing_on_disk: Vec<String>,
}

impl Decision {
    pub fn changed(&self) -> usize {
        self.add.len() + self.replace.len() + self.delete.len()
    }

    pub fn nothing_to_do(&self) -> bool {
        self.changed() == 0
    }
}

/// 照设计 5.2 逐个文件判。
///
/// - `installed`：已装清单（`<根>\release.json` 的 files）。
/// - `new`：来源包的清单。
/// - `disk`：已装清单里每个路径在磁盘上的实际 sha256；文件不在就不放这一项（或放空串）。
/// - `overwrite`：点名要覆盖的「本机改过」的路径。
pub fn decide(
    installed: &[proto::UpgradeFile],
    new: &[proto::UpgradeFile],
    disk: &HashMap<String, Option<String>>,
    overwrite: &[String],
) -> Decision {
    let mut d = Decision::default();
    let old: BTreeMap<&str, &str> = installed.iter().map(|f| (f.path.as_str(), f.sha256.as_str())).collect();
    let fresh: BTreeMap<&str, &str> = new.iter().map(|f| (f.path.as_str(), f.sha256.as_str())).collect();
    let overwrite: std::collections::HashSet<&str> = overwrite.iter().map(|s| s.as_str()).collect();

    for f in new {
        let disk_hash = disk.get(&f.path).cloned().flatten();
        match old.get(f.path.as_str()) {
            // 已装清单里没有它：盘上也没有就是 add；盘上有同内容算相同，不同内容算冲突
            None => match disk_hash {
                None => d.add.push(f.path.clone()),
                Some(h) if h == f.sha256 => {}
                Some(_) => {
                    if overwrite.contains(f.path.as_str()) {
                        d.replace.push(f.path.clone());
                    } else {
                        d.conflicts.push(f.path.clone());
                    }
                }
            },
            Some(old_hash) => match disk_hash {
                None => {
                    // 已装清单里有、盘上没了：当作「本机删了」，放回去
                    d.missing_on_disk.push(f.path.clone());
                    d.add.push(f.path.clone());
                }
                Some(h) if h == f.sha256 => {} // 盘 = 新
                Some(h) if h == *old_hash => {
                    if f.sha256 == *old_hash {
                        // 新旧一样、盘也是它：不动
                    } else {
                        d.replace.push(f.path.clone())
                    }
                }
                Some(_) => {
                    // 盘 ≠ 旧：本机改过
                    if f.sha256 == *old_hash {
                        d.kept_modified.push(f.path.clone());
                    } else if overwrite.contains(f.path.as_str()) {
                        d.replace.push(f.path.clone());
                    } else {
                        d.conflicts.push(f.path.clone());
                    }
                }
            },
        }
    }

    for f in installed {
        if fresh.contains_key(f.path.as_str()) {
            continue;
        }
        // 只在旧清单里：盘 = 旧才删；本机改过就留着
        match disk.get(&f.path).cloned().flatten() {
            Some(h) if h == f.sha256 => d.delete.push(f.path.clone()),
            Some(_) => d.kept_modified.push(f.path.clone()),
            None => {}
        }
    }

    d.add.sort();
    d.replace.sort();
    d.delete.sort();
    d.kept_modified.sort();
    d.conflicts.sort();
    d.missing_on_disk.sort();
    d
}

/// 要不要停服务（设计 5.3）：要换的路径全在 `web/`、`www/` 下面，或者是根下的 `.md` / `.txt`，就不停。
pub fn needs_stop(d: &Decision) -> bool {
    d.add.iter().chain(d.replace.iter()).chain(d.delete.iter()).any(|p| {
        let lower = p.to_ascii_lowercase();
        if lower.starts_with("web/") || lower.starts_with("www/") {
            return false;
        }
        if !lower.contains('/') && (lower.ends_with(".md") || lower.ends_with(".txt")) {
            return false;
        }
        true
    })
}

/// 相对路径能不能安全地拼到根下面（动手前再查一次，设计 5.1 末段：每一级目录不能是联接点）。
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, String> {
    check_rel_path(rel)?;
    let mut out = root.to_path_buf();
    for seg in rel.split('/') {
        out.push(seg);
    }
    // 再核一遍：拼出来的路径必须真的在根下面（防 Windows 的路径归一化意外）
    let norm_root = normalize(root);
    let norm_out = normalize(&out);
    if !norm_out.starts_with(&norm_root) {
        return Err(format!("{rel}：拼出来不在根下面"));
    }
    Ok(out)
}

/// 逐级检查已存在的目录不是联接点 / 符号链接。
pub fn check_no_reparse(root: &Path, rel: &str) -> Result<(), String> {
    let mut cur = root.to_path_buf();
    for seg in rel.split('/') {
        cur.push(seg);
        match std::fs::symlink_metadata(&cur) {
            Ok(md) => {
                if md.file_type().is_symlink() {
                    return Err(format!("{}：是符号链接 / 联接点，升级不动手", cur.display()));
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
                    if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                        return Err(format!("{}：是联接点，升级不动手", cur.display()));
                    }
                }
            }
            Err(_) => return Ok(()), // 后面还不存在，剩下的不用查
        }
    }
    Ok(())
}

/// 词法归一化（不碰磁盘）：去掉 `.`、处理 `..`。
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 算一个文件的 sha256（小写十六进制）。
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// `<根>\run\gui\upgrade\`
pub fn upgrade_dir(root: &Path) -> PathBuf {
    root.join("run").join("gui").join("upgrade")
}

pub fn blobs_dir(root: &Path) -> PathBuf {
    upgrade_dir(root).join("blobs")
}

pub fn sessions_dir(root: &Path) -> PathBuf {
    upgrade_dir(root).join("sessions")
}

pub fn session_dir(root: &Path, id: &str) -> PathBuf {
    sessions_dir(root).join(id)
}

/// 已装清单（`<根>\release.json` 的 `files`）。读不懂就是 `None`（= 不是发布包装出来的部署）。
pub fn installed_manifest(root: &Path) -> Option<proto::UpgradeManifest> {
    let text = std::fs::read_to_string(root.join("release.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(proto::UpgradeManifest {
        release: Some(proto::UpgradeReleaseId {
            name: value.get("name")?.as_str()?.to_string(),
            version: value.get("version")?.as_str()?.to_string(),
            channel: value.get("channel").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            created_at: value.get("created_at").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        }),
        components: value
            .get("components")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        Some(proto::UpgradeComponent {
                            id: c.get("id")?.as_str()?.to_string(),
                            title: c.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            version: c.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            path: c.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            sha256: c.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            dirty: c.get("dirty").and_then(|v| v.as_bool()).unwrap_or(false),
                            bytes: c.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        files: value
            .get("files")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|f| {
                        Some(proto::UpgradeFile {
                            path: f.get("path")?.as_str()?.to_string(),
                            bytes: f.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                            sha256: f.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        release_json: None,
        agent: None,
    })
}

/// 已装清单里每个路径在磁盘上的实际 sha256（`None` = 文件不在）。
///
/// 按「路径、大小、改动时间」缓存：整套约 600 MB，第一次要十几秒，之后只重算变了的。
pub fn disk_hashes(root: &Path, files: &[proto::UpgradeFile]) -> HashMap<String, Option<String>> {
    let cache_path = upgrade_dir(root).join("inventory-cache.json");
    let mut cache: HashMap<String, CacheEntry> = std::fs::read_to_string(&cache_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut out = HashMap::new();
    let mut dirty = false;
    for f in files {
        let full = match safe_join(root, &f.path) {
            Ok(p) => p,
            Err(_) => {
                out.insert(f.path.clone(), None);
                continue;
            }
        };
        let md = match std::fs::metadata(&full) {
            Ok(m) => m,
            Err(_) => {
                out.insert(f.path.clone(), None);
                if cache.remove(&f.path).is_some() {
                    dirty = true;
                }
                continue;
            }
        };
        let stamp = stamp_of(&md);
        match cache.get(&f.path) {
            Some(e) if e.bytes == md.len() && e.stamp == stamp && !e.sha256.is_empty() => {
                out.insert(f.path.clone(), Some(e.sha256.clone()));
            }
            _ => match file_sha256(&full) {
                Ok(h) => {
                    cache.insert(f.path.clone(), CacheEntry { bytes: md.len(), stamp, sha256: h.clone() });
                    dirty = true;
                    out.insert(f.path.clone(), Some(h));
                }
                Err(_) => {
                    out.insert(f.path.clone(), None);
                }
            },
        }
    }
    if dirty {
        if let Ok(dir) = std::fs::create_dir_all(upgrade_dir(root)) {
            let _ = dir;
        }
        if let Ok(text) = serde_json::to_string(&cache) {
            let _ = std::fs::write(&cache_path, text);
        }
    }
    out
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct CacheEntry {
    bytes: u64,
    stamp: u64,
    sha256: String,
}

fn stamp_of(md: &std::fs::Metadata) -> u64 {
    use std::time::UNIX_EPOCH;
    let m = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    m ^ (md.len().wrapping_mul(0x9e37_79b9_7f4a_7c15))
}

/// `digest`：把「路径 + 磁盘 sha256」排序拼起来再算 sha256（设计 4.2）。面板拿它判断「和上次看到的一样」。
pub fn digest_of(disk: &HashMap<String, Option<String>>) -> String {
    let mut pairs: Vec<(&str, &str)> = disk.iter().map(|(k, v)| (k.as_str(), v.as_deref().unwrap_or(""))).collect();
    pairs.sort();
    let mut hasher = Sha256::new();
    for (path, hash) in pairs {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(hash.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 一个会话在磁盘上的样子（`state.json`）。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SessionState {
    pub id: String,
    pub state: proto::UpgradeSessionState,
    /// 清单原文（`manifest.json` 另存一份，这里留一份方便不读两遍）。
    pub manifest: Option<proto::UpgradeManifest>,
    /// 已收到的内容：sha256 → 字节数。
    pub have: HashMap<String, u64>,
    /// 清单收下时判出来的（预览用）。
    pub plan: Option<proto::UpgradePlan>,
    pub problems: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl SessionState {
    pub fn load(root: &Path, id: &str) -> Option<SessionState> {
        let text = std::fs::read_to_string(session_dir(root, id).join("state.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let dir = session_dir(root, &self.id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("state.json"), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// 清单里所有内容（按 sha256 去重）和字节数。
    pub fn wanted(&self) -> BTreeMap<String, u64> {
        let mut out: BTreeMap<String, u64> = BTreeMap::new();
        if let Some(m) = &self.manifest {
            for f in &m.files {
                out.insert(f.sha256.clone(), f.bytes);
            }
            if let Some(rj) = &m.release_json {
                out.insert(rj.sha256.clone(), rj.bytes);
            }
            if let Some(a) = &m.agent {
                out.insert(a.sha256.clone(), a.bytes);
            }
        }
        out
    }

    /// 还缺哪些（`have` 不够的），以及缺多少字节。
    pub fn missing(&self) -> (Vec<proto::UpgradeMissingBlob>, u64) {
        let mut list = Vec::new();
        let mut bytes = 0u64;
        for (sha, want) in self.wanted() {
            let have = self.have.get(&sha).copied().unwrap_or(0).min(want);
            if have < want {
                list.push(proto::UpgradeMissingBlob { sha256: sha, bytes: want, have });
                bytes += want - have;
            }
        }
        (list, bytes)
    }

    /// 内容齐了就把状态推到 `ready`。
    pub fn refresh_ready(&mut self) {
        if self.state == proto::UpgradeSessionState::Open {
            let (missing, _) = self.missing();
            if missing.is_empty() {
                self.state = proto::UpgradeSessionState::Ready;
            }
        }
    }
}

/// 把一块内容写到 `.part`，返回收到的总字节数。
pub fn append_blob(root: &Path, sha: &str, offset: u64, chunk: &[u8]) -> std::io::Result<u64> {
    use std::io::{Seek, SeekFrom, Write};
    let dir = blobs_dir(root);
    std::fs::create_dir_all(&dir)?;
    let part = dir.join(format!("{sha}.part"));
    let mut f = std::fs::OpenOptions::new().create(true).read(true).write(true).open(&part)?;
    let cur = f.metadata()?.len();
    if offset != cur {
        return Ok(cur); // 调用方据此回 offset_mismatch
    }
    f.seek(SeekFrom::Start(offset))?;
    f.write_all(chunk)?;
    f.flush()?;
    Ok(offset + chunk.len() as u64)
}

/// `.part` 的当前大小。
pub fn blob_part_len(root: &Path, sha: &str) -> u64 {
    std::fs::metadata(blobs_dir(root).join(format!("{sha}.part"))).map(|m| m.len()).unwrap_or(0)
}

/// 收齐后核对 sha256；对了就改名就位，不对就删掉 `.part`。
pub fn finish_blob(root: &Path, sha: &str, expect_bytes: u64) -> Result<bool, String> {
    let dir = blobs_dir(root);
    let part = dir.join(format!("{sha}.part"));
    let done = dir.join(sha);
    if done.exists() {
        return Ok(true);
    }
    let len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if len != expect_bytes {
        return Ok(false);
    }
    match file_sha256(&part) {
        Ok(h) if h == sha => {
            std::fs::rename(&part, &done).map_err(|e| format!("收齐了但改名失败：{e}"))?;
            Ok(true)
        }
        Ok(_) => {
            let _ = std::fs::remove_file(&part);
            Err("hash_mismatch".into())
        }
        Err(e) => Err(format!("算 sha256 失败：{e}")),
    }
}

/// 内容库里的字节数（已收齐的）。
pub fn blob_len(root: &Path, sha: &str) -> Option<u64> {
    std::fs::metadata(blobs_dir(root).join(sha)).ok().map(|m| m.len())
}

/// 磁盘剩余空间（根所在的那个盘）。
pub fn disk_free_bytes(root: &Path) -> u64 {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetDiskFreeSpaceExW(dir: *const u16, free_to_caller: *mut u64, total: *mut u64, total_free: *mut u64) -> i32;
        }
        let mut wide: Vec<u16> = root.as_os_str().encode_wide().collect();
        wide.push(0);
        let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
        if ok != 0 { free } else { 0 }
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, sha: &str) -> proto::UpgradeFile {
        proto::UpgradeFile { path: path.into(), bytes: 1, sha256: sha.into() }
    }

    fn hex(c: char) -> String {
        c.to_string().repeat(64)
    }

    #[test]
    fn good_paths_pass() {
        for p in ["bin/aios-database.exe", "www/index.html", "stack.ps1", "安装和测试说明.md", "schema/e3d31/x.json", "a/b/c/d.txt"] {
            assert!(check_rel_path(p).is_ok(), "{p} 应当通过");
        }
    }

    #[test]
    fn each_bad_path_is_refused_with_a_reason() {
        let cases = [
            ("", "空"),
            ("/abs/x", "绝对"),
            ("\\x", "绝对"),
            ("C:/x", "冒号"),
            ("a\\b", "反斜杠"),
            ("a//b", "空"),
            ("a/./b", ".."),
            ("../x", ".."),
            ("a/ ", "空格"),
            ("a/b.", "点"),
            ("con", "设备名"),
            ("dir/PRN.txt", "设备名"),
            ("com1", "设备名"),
            ("stack.toml", "用户自己的配置"),
            ("release.json", "单独处理"),
            ("run/x", "运行目录"),
            ("LOGS/a.log", "运行目录"),
            ("data/surreal/1", "运行目录"),
            ("work/backend/DbOption.toml", "运行目录"),
            ("tmp/x", "运行目录"),
            ("a<b", "之一"),
            ("a?b", "之一"),
        ];
        for (p, want) in cases {
            let err = check_rel_path(p).unwrap_err();
            assert!(err.contains(want), "{p:?} 的原因里应当有「{want}」，实际是「{err}」");
        }
        let long = format!("{}/x", "a".repeat(240));
        assert!(check_rel_path(&long).unwrap_err().contains("240"));
        assert!(check_rel_path("a/\u{1}b").unwrap_err().contains("控制字符"));
    }

    #[test]
    fn com0_is_not_a_device_name() {
        assert!(check_rel_path("com0").is_ok());
        assert!(check_rel_path("com10").is_ok());
    }

    #[test]
    fn manifest_rejects_duplicates_and_bad_sha() {
        assert!(check_manifest_paths(&[f("a/b", &hex('a'))]).is_ok());
        let dup = check_manifest_paths(&[f("Bin/X", &hex('a')), f("bin/x", &hex('b'))]).unwrap_err();
        assert!(dup.contains("不分大小写"), "{dup}");
        let bad = check_manifest_paths(&[f("a", "ABC")]).unwrap_err();
        assert!(bad.contains("64 位"), "{bad}");
    }

    #[test]
    fn decide_covers_the_design_table() {
        let old = vec![f("same", &hex('1')), f("repl", &hex('2')), f("keepmod", &hex('3')), f("conflict", &hex('4')), f("del", &hex('5')), f("delmod", &hex('6')), f("gone", &hex('7'))];
        let new = vec![
            f("same", &hex('1')),
            f("repl", &hex('a')),
            f("keepmod", &hex('3')),
            f("conflict", &hex('b')),
            f("added", &hex('c')),
            // 新包还带着它，但盘上已经被删了：当作 add，并记进 missing_on_disk
            f("gone", &hex('7')),
        ];
        let mut disk = HashMap::new();
        disk.insert("same".to_string(), Some(hex('1')));
        disk.insert("repl".to_string(), Some(hex('2')));
        disk.insert("keepmod".to_string(), Some(hex('9'))); // 本机改过，包没改
        disk.insert("conflict".to_string(), Some(hex('8'))); // 本机改过，包也改了
        disk.insert("del".to_string(), Some(hex('5')));
        disk.insert("delmod".to_string(), Some(hex('f'))); // 本机改过
        disk.insert("gone".to_string(), None); // 盘上没了

        let d = decide(&old, &new, &disk, &[]);
        assert_eq!(d.replace, vec!["repl"]);
        assert_eq!(d.add, vec!["added", "gone"]);
        assert_eq!(d.delete, vec!["del"]);
        assert_eq!(d.kept_modified, vec!["delmod", "keepmod"]);
        assert_eq!(d.conflicts, vec!["conflict"]);
        assert_eq!(d.missing_on_disk, vec!["gone"]);
        assert_eq!(d.changed(), 4, "repl + added + gone + del");
        assert!(!d.nothing_to_do());

        // 点名覆盖冲突的
        let d2 = decide(&old, &new, &disk, &["conflict".to_string()]);
        assert_eq!(d2.replace, vec!["conflict", "repl"]);
        assert!(d2.conflicts.is_empty());
    }

    #[test]
    fn nothing_to_do_when_identical() {
        let same = vec![f("a", &hex('1'))];
        let mut disk = HashMap::new();
        disk.insert("a".to_string(), Some(hex('1')));
        let d = decide(&same, &same, &disk, &[]);
        assert!(d.nothing_to_do());
        assert_eq!(d.changed(), 0);
    }

    #[test]
    fn stop_rules() {
        let mut d = Decision::default();
        d.replace.push("www/index.html".into());
        d.add.push("README.txt".into());
        assert!(!needs_stop(&d), "只有网页和根下的 md/txt 不用停");

        d.replace.push("bin/aios-database.exe".into());
        assert!(needs_stop(&d), "换了程序就要停");

        let mut only_web = Decision::default();
        only_web.delete.push("web/old.js".into());
        assert!(!needs_stop(&only_web));

        let mut schema = Decision::default();
        schema.add.push("schema/e3d31/x.json".into());
        assert!(needs_stop(&schema), "schema 不在 web/www 下面，要停");
    }

    #[test]
    fn safe_join_stays_under_root() {
        let root = Path::new("C:\\stack");
        assert!(safe_join(root, "bin/x.exe").is_ok());
        assert!(safe_join(root, "../evil").is_err());
        assert!(safe_join(root, "/abs").is_err());
    }

    #[test]
    fn digest_is_order_independent_and_sensitive() {
        let mut a = HashMap::new();
        a.insert("x".to_string(), Some(hex('1')));
        a.insert("y".to_string(), Some(hex('2')));
        let mut b = HashMap::new();
        b.insert("y".to_string(), Some(hex('2')));
        b.insert("x".to_string(), Some(hex('1')));
        assert_eq!(digest_of(&a), digest_of(&b), "排序后拼，和插入顺序无关");

        let mut c = a.clone();
        c.insert("y".to_string(), Some(hex('3')));
        assert_ne!(digest_of(&a), digest_of(&c), "内容变了 digest 就变");

        let mut d = a.clone();
        d.insert("x".to_string(), None);
        assert_ne!(digest_of(&a), digest_of(&d), "文件没了 digest 也变");
    }

    #[test]
    fn missing_counts_only_unfinished_blobs() {
        let mut s = SessionState::default();
        s.manifest = Some(proto::UpgradeManifest {
            files: vec![proto::UpgradeFile { path: "a".into(), bytes: 100, sha256: hex('a') }, proto::UpgradeFile { path: "b".into(), bytes: 50, sha256: hex('b') }],
            ..Default::default()
        });
        s.have.insert(hex('a'), 100);
        let (m, bytes) = s.missing();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].sha256, hex('b'));
        assert_eq!(bytes, 50);
        s.have.insert(hex('b'), 50);
        let (m, bytes) = s.missing();
        assert!(m.is_empty() && bytes == 0);
        s.state = proto::UpgradeSessionState::Open;
        s.refresh_ready();
        assert_eq!(s.state, proto::UpgradeSessionState::Ready);
    }

    #[test]
    fn blob_round_trip_and_hash_check() {
        let dir = std::env::temp_dir().join(format!("upgrade-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let payload = b"hello upgrade";
        let sha = Sha256::digest(payload).iter().map(|b| format!("{b:02x}")).collect::<String>();

        let got = append_blob(&dir, &sha, 0, &payload[..5]).unwrap();
        assert_eq!(got, 5);
        let got = append_blob(&dir, &sha, 5, &payload[5..]).unwrap();
        assert_eq!(got, payload.len() as u64);
        assert!(finish_blob(&dir, &sha, payload.len() as u64).unwrap());
        assert_eq!(blob_len(&dir, &sha), Some(payload.len() as u64));

        // 再传一次同样的内容：已经在库里，直接算完
        assert!(finish_blob(&dir, &sha, payload.len() as u64).unwrap());

        // 内容对不上：.part 被删掉
        let bad = "0".repeat(64);
        append_blob(&dir, &bad, 0, b"wrong").unwrap();
        assert_eq!(finish_blob(&dir, &bad, 5), Err("hash_mismatch".into()));
        assert_eq!(blob_part_len(&dir, &bad), 0, ".part 已经删掉");

        // 偏移不对：回当前大小，不写
        append_blob(&dir, &sha, 0, b"x").unwrap();
        let cur = append_blob(&dir, &sha, 99, b"y").unwrap();
        assert_eq!(cur, 1, "偏移对不上时返回已经收到的字节数（这里刚写进去 1 字节）");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_state_round_trip() {
        let dir = std::env::temp_dir().join(format!("upgrade-session-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = SessionState {
            id: "u20261113-031200-a1b2".into(),
            state: proto::UpgradeSessionState::Open,
            manifest: Some(proto::UpgradeManifest::default()),
            have: [("aa".repeat(32), 7u64)].into_iter().collect(),
            plan: None,
            problems: vec!["提醒一句".into()],
            created_at: "2026-11-13T03:12:00+08:00".into(),
            updated_at: "2026-11-13T03:12:00+08:00".into(),
        };
        s.save(&dir).unwrap();
        let back = SessionState::load(&dir, &s.id).unwrap();
        assert_eq!(back.state, proto::UpgradeSessionState::Open);
        assert_eq!(back.have.values().next(), Some(&7));
        assert_eq!(back.problems, vec!["提醒一句"]);
        assert!(SessionState::load(&dir, "nope").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_manifest_reads_release_json() {
        let dir = std::env::temp_dir().join(format!("upgrade-installed-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(installed_manifest(&dir).is_none(), "没有 release.json = 不是发布包");
        std::fs::write(
            dir.join("release.json"),
            r#"{"name":"pkg","version":"v1","channel":"rc","created_at":"2026-11-12T09:37:05+08:00",
                "components":[{"id":"backend","version":"0.1.44","path":"bin/a.exe","sha256":"aa","dirty":true,"bytes":10}],
                "files":[{"path":"bin/a.exe","bytes":10,"sha256":"aa"}]}"#,
        )
        .unwrap();
        let m = installed_manifest(&dir).unwrap();
        assert_eq!(m.release.as_ref().unwrap().version, "v1");
        assert_eq!(m.files.len(), 1);
        assert_eq!(m.components[0].id, "backend");
        assert!(m.components[0].dirty);
        std::fs::write(dir.join("release.json"), "{ not json").unwrap();
        assert!(installed_manifest(&dir).is_none(), "读不懂就当没有");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_hashes_uses_cache_and_notices_missing() {
        let dir = std::env::temp_dir().join(format!("upgrade-disk-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("a.exe"), b"content").unwrap();
        let sha = file_sha256(&dir.join("bin").join("a.exe")).unwrap();
        let files = vec![
            proto::UpgradeFile { path: "bin/a.exe".into(), bytes: 7, sha256: sha.clone() },
            proto::UpgradeFile { path: "bin/gone.exe".into(), bytes: 7, sha256: "x".repeat(64) },
        ];
        let h = disk_hashes(&dir, &files);
        assert_eq!(h.get("bin/a.exe"), Some(&Some(sha)));
        assert_eq!(h.get("bin/gone.exe"), Some(&None));
        assert!(upgrade_dir(&dir).join("inventory-cache.json").exists(), "算过就落缓存");
        // 第二次走缓存，结果一样
        let h2 = disk_hashes(&dir, &files);
        assert_eq!(h, h2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
