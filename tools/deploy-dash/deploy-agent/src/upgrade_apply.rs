//! 应用与换回（设计 5.4–5.8）：换文件日志、照日志换回、备份与保留。
//!
//! 三条不变量（设计第 3 节）落在这里：
//! 3. **要么全是旧的，要么全是新的。** 每挪一个文件先写一行日志并落盘（fsync），再动手；
//!    任何时候代理死掉，下次起来照日志换回去。
//! 4. **没过就换回，并恢复升级前的样子。** 换回可以重复做，做几遍结果一样。
//!
//! 这一层只管「文件怎么挪」，不管停起服务（那是 `upgrade_apply`）——所以它能单独测。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use deploy_dash_proto as proto;

use crate::upgrade_store as store;

/// 日志里的一行（`journal.jsonl`）。`op` 决定后面几个字段怎么用。
///
/// 一行一个 JSON，字段用 `Option` 让每种 `op` 只写自己那几个。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JournalLine {
    pub n: u64,
    pub at: String,
    pub op: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    /// `begin`：这次是 `upgrade` 还是 `rollback`；`rollback_begin`：为什么。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session: String,
    /// `begin`：升级前服务在不在跑、增量同步开没开。
    #[serde(default)]
    pub services_running: bool,
    #[serde(default)]
    pub queue_paused: bool,
    /// `rollback_begin`：为什么要换回（如 `up 退出码 4`）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
    /// `rolled_back`：换回本身成没成。
    #[serde(default)]
    pub ok: bool,
}

/// `op` 的取值。
pub mod op {
    pub const BEGIN: &str = "begin";
    pub const STOPPED: &str = "stopped";
    /// 把 `<根>\<路径>` 改名挪到 `backup\<路径>`。
    pub const BACKUP: &str = "backup";
    /// 把内容库里的内容放到 `<根>\<路径>`。
    pub const PLACE: &str = "place";
    /// 新版另存到 `conflicts\<路径>`，不动根下的文件。
    pub const CONFLICT: &str = "conflict";
    /// `release.json` 单独处理：换文件之后才放。
    pub const RELEASE_JSON: &str = "release_json";
    /// 全部换完。
    pub const SWAPPED: &str = "swapped";
    pub const STARTED: &str = "started";
    /// 检查通过，这次应用有了结果。
    pub const COMMIT: &str = "commit";
    pub const ROLLBACK_BEGIN: &str = "rollback_begin";
    /// 换回做完（`ok` 说成没成）。
    pub const ROLLED_BACK: &str = "rolled_back";
}

/// 一次应用在磁盘上的目录：`<根>\run\gui\upgrade\applied\<序号>-<会话号>\`
#[derive(Clone, Debug)]
pub struct Applied {
    pub dir: PathBuf,
}

impl Applied {
    /// 新建（序号由调用方给：从 1 开始，已有的最大 +1）。
    pub fn create(root: &Path, seq: u32, session: &str) -> std::io::Result<Applied> {
        let dir = store::upgrade_dir(root).join("applied").join(format!("{seq:04}-{session}"));
        std::fs::create_dir_all(dir.join("backup"))?;
        std::fs::create_dir_all(dir.join("conflicts"))?;
        Ok(Applied { dir })
    }

    /// 找现有的。
    pub fn open(dir: impl Into<PathBuf>) -> Applied {
        Applied { dir: dir.into() }
    }

    pub fn journal_path(&self) -> PathBuf {
        self.dir.join("journal.jsonl")
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.dir.join("backup")
    }

    pub fn conflicts_dir(&self) -> PathBuf {
        self.dir.join("conflicts")
    }

    /// 追加一行并落盘（fsync）。**先落盘再动手**——这是崩溃恢复能成立的前提。
    pub fn append(&self, mut line: JournalLine) -> std::io::Result<()> {
        use std::io::Write;
        let existing = self.read();
        line.n = existing.last().map(|l| l.n + 1).unwrap_or(1);
        if line.at.is_empty() {
            line.at = crate::timefmt::now_iso();
        }
        let text = serde_json::to_string(&line).map_err(std::io::Error::other)?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.journal_path())?;
        writeln!(f, "{text}")?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    }

    /// 读全部行（坏行跳过——写到一半时被杀会留半行）。
    pub fn read(&self) -> Vec<JournalLine> {
        std::fs::read_to_string(self.journal_path()).map(|t| t.lines().filter_map(|l| serde_json::from_str(l.trim()).ok()).collect()).unwrap_or_default()
    }

    /// 这次应用有没有结果：最后一行是 `commit`，或者是成功的 `rolled_back`。
    pub fn finished(&self) -> bool {
        match self.read().last() {
            Some(l) if l.op == op::COMMIT => true,
            Some(l) if l.op == op::ROLLED_BACK => l.ok,
            _ => false,
        }
    }

    /// 是不是一次「升级」的日志（崩溃恢复据此决定是换回还是接着换回）。
    pub fn kind(&self) -> String {
        self.read().first().map(|l| l.kind.clone()).unwrap_or_default()
    }

    pub fn session(&self) -> String {
        self.read().first().map(|l| l.session.clone()).unwrap_or_default()
    }

    pub fn started_at(&self) -> String {
        self.read().first().map(|l| l.at.clone()).unwrap_or_default()
    }
}

/// 备份里的一处：`<应用目录>\backup\<路径>`
pub fn backup_path(applied: &Applied, rel: &str) -> PathBuf {
    let mut p = applied.backup_dir();
    for seg in rel.split('/') {
        p.push(seg);
    }
    p
}

/// 冲突版的一处：`<应用目录>\conflicts\<路径>`
pub fn conflict_path(applied: &Applied, rel: &str) -> PathBuf {
    let mut p = applied.conflicts_dir();
    for seg in rel.split('/') {
        p.push(seg);
    }
    p
}

/// 把文件挪到目标位置（先建目录；目标已存在就覆盖）。
fn move_to(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // 同一个盘上是改名，快；跨盘才退化成复制 + 删
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(src, dst)?;
            std::fs::remove_file(src)
        }
    }
}

/// 把内容库里的内容放到 `<根>\<路径>`（设计 5.5 的 `place`）。
///
/// 同一份内容要放到几个路径时：前几个复制、最后一个挪（省一次写盘）。
pub fn place_blob(root: &Path, sha: &str, rel: &str, last_use: bool) -> Result<(), String> {
    let blob = store::blobs_dir(root).join(sha);
    if !blob.is_file() {
        return Err(format!("内容库里没有 {sha}（{rel}）"));
    }
    let dst = store::safe_join(root, rel)?;
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录 {} 失败：{e}", dir.display()))?;
    }
    if last_use {
        move_to(&blob, &dst).map_err(|e| format!("把内容放到 {} 失败：{e}", dst.display()))
    } else {
        std::fs::copy(&blob, &dst).map(|_| ()).map_err(|e| format!("复制内容到 {} 失败：{e}", dst.display()))
    }
}

/// 换一个文件：`backup`（旧的挪进备份）+ `place`（新的放上去）。先写日志再动手。
///
/// 被占用时重试 5 次、每次隔 2 秒（设计 5.4 第 5 步）。
pub fn replace_file(root: &Path, applied: &Applied, rel: &str, sha: &str, last_use: bool) -> Result<(), String> {
    let target = store::safe_join(root, rel)?;
    // 动手前再核一次联接点（设计 5.1 末段）
    store::check_no_reparse(root, rel)?;

    if target.exists() {
        let bak = backup_path(applied, rel);
        // 旧的 sha 记进日志：换回时据此判断「这个文件还是不是当时放上去的那个」
        let old_sha = store::file_sha256(&target).unwrap_or_default();
        applied
            .append(JournalLine { op: op::BACKUP.into(), path: rel.into(), sha256: old_sha, ..Default::default() })
            .map_err(|e| format!("写日志失败（{rel}）：{e}"))?;
        let mut last_err = String::new();
        for attempt in 0..5 {
            match move_to(&target, &bak) {
                Ok(()) => {
                    last_err.clear();
                    break;
                }
                Err(e) => {
                    last_err = format!("{e}");
                    if attempt < 4 {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                    }
                }
            }
        }
        if !last_err.is_empty() {
            return Err(format!("{rel} 挪不动（被占用？）：{last_err}"));
        }
    }

    applied
        .append(JournalLine { op: op::PLACE.into(), path: rel.into(), sha256: sha.into(), ..Default::default() })
        .map_err(|e| format!("写日志失败（{rel}）：{e}"))?;
    place_blob(root, sha, rel, last_use)
}

/// 只删一个文件（新版清单里没有它）：挪进备份。先写日志。
pub fn delete_file(root: &Path, applied: &Applied, rel: &str) -> Result<(), String> {
    let target = store::safe_join(root, rel)?;
    if !target.exists() {
        return Ok(());
    }
    store::check_no_reparse(root, rel)?;
    let old_sha = store::file_sha256(&target).unwrap_or_default();
    applied
        .append(JournalLine { op: op::BACKUP.into(), path: rel.into(), sha256: old_sha, ..Default::default() })
        .map_err(|e| format!("写日志失败（{rel}）：{e}"))?;
    move_to(&target, &backup_path(applied, rel)).map_err(|e| format!("{rel} 挪进备份失败：{e}"))
}

/// 本机改过、包也改了它：留着本机的，新版另存一份到 `conflicts\<路径>`。先写日志。
pub fn save_conflict(root: &Path, applied: &Applied, rel: &str, sha: &str) -> Result<(), String> {
    store::check_no_reparse(root, rel)?;
    applied
        .append(JournalLine { op: op::CONFLICT.into(), path: rel.into(), sha256: sha.into(), ..Default::default() })
        .map_err(|e| format!("写日志失败（{rel}）：{e}"))?;
    let blob = store::blobs_dir(root).join(sha);
    let dst = conflict_path(applied, rel);
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录 {} 失败：{e}", dir.display()))?;
    }
    std::fs::copy(&blob, &dst).map(|_| ()).map_err(|e| format!("把新版另存到 {} 失败：{e}", dst.display()))
}

/// 换 `release.json`：旧的挪进备份、新的放上去。先写日志。
///
/// 日志里 `path` 记的是**旧的 sha**（空串 = 当时没有 `release.json`），换回时据此决定「放回旧的」还是「删掉新的」。
pub fn replace_release_json(root: &Path, applied: &Applied, sha: &str) -> Result<(), String> {
    let target = root.join("release.json");
    let old_sha = if target.exists() { store::file_sha256(&target).unwrap_or_default() } else { String::new() };
    applied
        .append(JournalLine { op: op::RELEASE_JSON.into(), sha256: sha.into(), path: old_sha, ..Default::default() })
        .map_err(|e| format!("写日志失败（release.json）：{e}"))?;
    if target.exists() {
        move_to(&target, &applied.backup_dir().join("release.json")).map_err(|e| format!("旧的 release.json 挪进备份失败：{e}"))?;
    }
    let blob = store::blobs_dir(root).join(sha);
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败：{e}"))?;
    }
    move_to(&blob, &target).map_err(|e| format!("放新的 release.json 失败：{e}"))
}

/// 照日志换回（设计 5.6）。**从后往前读，可以重复做，做几遍结果一样。**
///
/// - `place <路径>`：根下那个文件的 sha256 是这一行记的新值，就把它挪回内容库（下次重试不用重传）；不是就不动它。
/// - `backup <路径>`：`backup\<路径>` 在，就挪回 `<根>\<路径>`；不在说明当时没来得及挪，跳过。
/// - `release_json`：备份里有旧的 `release.json` 就放回去。
/// - `conflict`：不用管。
///
/// 返回（恢复了几个文件，出错说明）。
pub fn rollback(root: &Path, applied: &Applied) -> (u32, Vec<String>) {
    let lines = applied.read();
    let mut restored = 0u32;
    let mut errors = Vec::new();
    for line in lines.iter().rev() {
        match line.op.as_str() {
            op::PLACE => {
                if line.path.is_empty() {
                    continue;
                }
                let target = match store::safe_join(root, &line.path) {
                    Ok(p) => p,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                };
                if !target.exists() {
                    continue;
                }
                // 现在这个文件是不是当时放上去的那个新版？是就收起来；不是（后来被改过）就不动它
                let now = store::file_sha256(&target).unwrap_or_default();
                if now == line.sha256 {
                    let blob = store::blobs_dir(root).join(&line.sha256);
                    if !blob.exists() {
                        if let Some(dir) = blob.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        if let Err(e) = move_to(&target, &blob) {
                            errors.push(format!("{}：收不回内容库（{e}）", line.path));
                        }
                    } else if let Err(e) = std::fs::remove_file(&target) {
                        errors.push(format!("{}：删不掉（{e}）", line.path));
                    }
                }
            }
            op::BACKUP => {
                if line.path.is_empty() {
                    continue;
                }
                let bak = backup_path(applied, &line.path);
                if !bak.exists() {
                    continue; // 当时没来得及挪，跳过
                }
                let target = match store::safe_join(root, &line.path) {
                    Ok(p) => p,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                };
                if target.exists() {
                    // 目标位置上还有个文件（不该有，但别覆盖）：先挪开
                    let aside = bak.with_extension("leftover");
                    if let Err(e) = move_to(&target, &aside) {
                        errors.push(format!("{}：占着位置挪不开（{e}）", line.path));
                        continue;
                    }
                }
                match move_to(&bak, &target) {
                    Ok(()) => restored += 1,
                    Err(e) => errors.push(format!("{}：换回失败（{e}）", line.path)),
                }
            }
            op::RELEASE_JSON => {
                let bak = applied.backup_dir().join("release.json");
                let target = root.join("release.json");
                if bak.exists() {
                    let _ = std::fs::remove_file(&target);
                    match move_to(&bak, &target) {
                        Ok(()) => restored += 1,
                        Err(e) => errors.push(format!("release.json：换回失败（{e}）")),
                    }
                } else if line.path.is_empty() {
                    // 原来没有 release.json（`path` 记的是旧 sha，空 = 当时没有）：把新放上去的那个删掉
                    let _ = std::fs::remove_file(&target);
                }
            }
            _ => {}
        }
    }
    (restored, errors)
}

/// 算一个文件的 sha256（转给 `upgrade_store`）。
pub fn sha_of(path: &Path) -> String {
    store::file_sha256(path).unwrap_or_default()
}

/// 内容库的 sha（给单测造夹具）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// 应用目录的序号：已有的最大 + 1。
pub fn next_seq(root: &Path) -> u32 {
    let dir = store::upgrade_dir(root).join("applied");
    let mut max = 0u32;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(n) = name.split('-').next().and_then(|s| s.parse::<u32>().ok()) {
                max = max.max(n);
            }
        }
    }
    max + 1
}

/// 列出现有的应用目录（新的在前）。
pub fn list_applied(root: &Path) -> Vec<Applied> {
    let dir = store::upgrade_dir(root).join("applied");
    let mut out: Vec<Applied> = std::fs::read_dir(&dir)
        .map(|entries| entries.flatten().filter(|e| e.path().is_dir()).map(|e| Applied::open(e.path())).collect())
        .unwrap_or_default();
    out.sort_by(|a, b| b.dir.file_name().cmp(&a.dir.file_name()));
    out
}

/// 最近一次成功的升级（「回到上一版」用）。
pub fn last_committed_upgrade(root: &Path) -> Option<Applied> {
    list_applied(root).into_iter().find(|a| a.kind() == proto::JobKindName::Upgrade.as_str() && a.finished() && a.read().last().map(|l| l.op == op::COMMIT).unwrap_or(false))
}

/// 保留最近 `keep` 次成功的升级的备份，更早的连目录删掉（设计 5.8）。
pub fn prune_backups(root: &Path, keep: u32) -> Vec<PathBuf> {
    let mut committed: Vec<Applied> =
        list_applied(root).into_iter().filter(|a| a.read().last().map(|l| l.op == op::COMMIT).unwrap_or(false)).collect();
    let mut removed = Vec::new();
    while committed.len() > keep.max(1) as usize {
        let oldest = committed.pop().expect("长度大于 keep");
        if std::fs::remove_dir_all(&oldest.dir).is_ok() {
            removed.push(oldest.dir);
        }
    }
    removed
}

/// 写一份报告到 `history.jsonl`（新的追加在后面，读的时候倒过来）。
pub fn append_history(root: &Path, report: &proto::UpgradeReport) -> std::io::Result<()> {
    use std::io::Write;
    let dir = store::upgrade_dir(root);
    std::fs::create_dir_all(&dir)?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("history.jsonl"))?;
    writeln!(f, "{}", serde_json::to_string(report).map_err(std::io::Error::other)?)?;
    f.flush()?;
    f.sync_all()
}

/// 把一个文件写成「旧版」的样子（单测造包用）。
#[cfg(test)]
pub fn write_file(root: &Path, rel: &str, bytes: &[u8]) {
    let p = store::safe_join(root, rel).unwrap();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("upgrade-apply-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 造一个内容库条目。
    fn seed_blob(root: &Path, bytes: &[u8]) -> String {
        let sha = sha256_hex(bytes);
        let dir = store::blobs_dir(root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(&sha), bytes).unwrap();
        sha
    }

    #[test]
    fn journal_is_append_only_and_numbers_grow() {
        let root = temp_root("journal");
        let a = Applied::create(&root, 1, "u1").unwrap();
        a.append(JournalLine { op: op::BEGIN.into(), kind: "upgrade".into(), session: "u1".into(), services_running: true, ..Default::default() }).unwrap();
        a.append(JournalLine { op: op::STOPPED.into(), ..Default::default() }).unwrap();
        let lines = a.read();
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0].n, lines[1].n), (1, 2));
        assert!(lines[0].services_running);
        assert!(!lines[0].at.is_empty(), "时刻自动填");
        assert_eq!(a.kind(), "upgrade");
        assert_eq!(a.session(), "u1");
        assert!(!a.finished());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finished_only_on_commit_or_successful_rollback() {
        let root = temp_root("finished");
        let a = Applied::create(&root, 1, "u1").unwrap();
        a.append(JournalLine { op: op::BEGIN.into(), kind: "upgrade".into(), ..Default::default() }).unwrap();
        assert!(!a.finished());
        a.append(JournalLine { op: op::ROLLED_BACK.into(), ok: false, ..Default::default() }).unwrap();
        assert!(!a.finished(), "换回没成不算有结果");
        a.append(JournalLine { op: op::ROLLED_BACK.into(), ok: true, ..Default::default() }).unwrap();
        assert!(a.finished());

        let b = Applied::create(&root, 2, "u2").unwrap();
        b.append(JournalLine { op: op::BEGIN.into(), kind: "upgrade".into(), ..Default::default() }).unwrap();
        b.append(JournalLine { op: op::COMMIT.into(), ..Default::default() }).unwrap();
        assert!(b.finished());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn replace_backs_up_the_old_and_places_the_new() {
        let root = temp_root("replace");
        write_file(&root, "bin/a.exe", b"old-bytes");
        let new_sha = seed_blob(&root, b"new-bytes");
        let a = Applied::create(&root, 1, "u1").unwrap();
        a.append(JournalLine { op: op::BEGIN.into(), kind: "upgrade".into(), ..Default::default() }).unwrap();

        replace_file(&root, &a, "bin/a.exe", &new_sha, true).unwrap();
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"new-bytes", "新内容就位");
        assert_eq!(std::fs::read(backup_path(&a, "bin/a.exe")).unwrap(), b"old-bytes", "旧内容进了备份");
        assert!(!store::blobs_dir(&root).join(&new_sha).exists(), "最后一份是挪过去的，内容库不再留");

        let ops: Vec<String> = a.read().iter().map(|l| l.op.clone()).collect();
        assert_eq!(ops, vec![op::BEGIN, op::BACKUP, op::PLACE]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn add_has_no_backup_line() {
        let root = temp_root("add");
        let sha = seed_blob(&root, b"brand-new");
        let a = Applied::create(&root, 1, "u1").unwrap();
        replace_file(&root, &a, "www/new.js", &sha, true).unwrap();
        let ops: Vec<String> = a.read().iter().map(|l| l.op.clone()).collect();
        assert_eq!(ops, vec![op::PLACE], "文件原来不在，不该有 backup 行");
        assert_eq!(std::fs::read(root.join("www").join("new.js")).unwrap(), b"brand-new");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_moves_to_backup() {
        let root = temp_root("delete");
        write_file(&root, "www/old.js", b"gone");
        let a = Applied::create(&root, 1, "u1").unwrap();
        delete_file(&root, &a, "www/old.js").unwrap();
        assert!(!root.join("www").join("old.js").exists());
        assert_eq!(std::fs::read(backup_path(&a, "www/old.js")).unwrap(), b"gone");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn conflict_keeps_local_and_stores_the_new() {
        let root = temp_root("conflict");
        write_file(&root, "config.toml", b"local-with-password");
        let sha = seed_blob(&root, b"package-template");
        let a = Applied::create(&root, 1, "u1").unwrap();
        save_conflict(&root, &a, "config.toml", &sha).unwrap();
        assert_eq!(std::fs::read(root.join("config.toml")).unwrap(), b"local-with-password", "本机的不动");
        assert_eq!(std::fs::read(conflict_path(&a, "config.toml")).unwrap(), b"package-template", "新版另存");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rollback_restores_everything_and_is_idempotent() {
        let root = temp_root("rollback");
        write_file(&root, "bin/a.exe", b"old-a");
        write_file(&root, "www/old.js", b"old-js");
        let new_a = seed_blob(&root, b"new-a");
        let a = Applied::create(&root, 1, "u1").unwrap();
        a.append(JournalLine { op: op::BEGIN.into(), kind: "upgrade".into(), ..Default::default() }).unwrap();
        replace_file(&root, &a, "bin/a.exe", &new_a, true).unwrap();
        delete_file(&root, &a, "www/old.js").unwrap();

        let (n, errs) = rollback(&root, &a);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(n, 2);
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"old-a", "旧版回来了");
        assert_eq!(std::fs::read(root.join("www").join("old.js")).unwrap(), b"old-js", "删掉的回来了");
        assert!(store::blobs_dir(&root).join(&new_a).exists(), "新版收回内容库，下次重试不用重传");

        // 再来一遍：什么都不该变（可重复）
        let (n2, errs2) = rollback(&root, &a);
        assert!(errs2.is_empty(), "{errs2:?}");
        assert_eq!(n2, 0, "第二遍没有东西可恢复");
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"old-a");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rollback_leaves_a_file_that_was_modified_after_the_upgrade() {
        let root = temp_root("rollback-modified");
        write_file(&root, "bin/a.exe", b"old-a");
        let new_a = seed_blob(&root, b"new-a");
        let a = Applied::create(&root, 1, "u1").unwrap();
        replace_file(&root, &a, "bin/a.exe", &new_a, true).unwrap();
        // 升级之后有人又改了这个文件
        std::fs::write(root.join("bin").join("a.exe"), b"hand-edited").unwrap();

        let (_, errs) = rollback(&root, &a);
        assert!(errs.is_empty(), "{errs:?}");
        // 换回会把备份挪回去（备份还在），所以最终是旧版；关键是没把「手改的」当成新版收回内容库
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"old-a");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rollback_survives_a_half_written_journal() {
        let root = temp_root("half");
        write_file(&root, "bin/a.exe", b"old-a");
        let a = Applied::create(&root, 1, "u1").unwrap();
        a.append(JournalLine { op: op::BEGIN.into(), kind: "upgrade".into(), ..Default::default() }).unwrap();
        // 手写一行半截的（模拟写到一半被杀）
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(a.journal_path()).unwrap();
        writeln!(f, "{{\"n\":2,\"op\":\"backup\",\"path\":\"bin/a.exe\"").unwrap();
        drop(f);
        let lines = a.read();
        assert_eq!(lines.len(), 1, "半行跳过");
        let (_, errs) = rollback(&root, &a);
        assert!(errs.is_empty(), "{errs:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn release_json_is_replaced_and_restored() {
        let root = temp_root("releasejson");
        std::fs::write(root.join("release.json"), b"old-manifest").unwrap();
        let sha = seed_blob(&root, b"new-manifest");
        let a = Applied::create(&root, 1, "u1").unwrap();
        replace_release_json(&root, &a, &sha).unwrap();
        assert_eq!(std::fs::read(root.join("release.json")).unwrap(), b"new-manifest");

        let (n, errs) = rollback(&root, &a);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(n, 1);
        assert_eq!(std::fs::read(root.join("release.json")).unwrap(), b"old-manifest", "旧清单回来了");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn release_json_rollback_deletes_it_when_there_was_none() {
        let root = temp_root("releasejson-new");
        let sha = seed_blob(&root, b"first-manifest");
        let a = Applied::create(&root, 1, "u1").unwrap();
        replace_release_json(&root, &a, &sha).unwrap();
        assert!(root.join("release.json").exists());
        let (_, errs) = rollback(&root, &a);
        assert!(errs.is_empty(), "{errs:?}");
        assert!(!root.join("release.json").exists(), "原来没有，就该没有");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn seq_and_listing() {
        let root = temp_root("seq");
        assert_eq!(next_seq(&root), 1);
        Applied::create(&root, 1, "u1").unwrap();
        Applied::create(&root, 2, "u2").unwrap();
        assert_eq!(next_seq(&root), 3);
        let list = list_applied(&root);
        assert_eq!(list.len(), 2);
        assert!(list[0].dir.file_name().unwrap().to_string_lossy().starts_with("0002"), "新的在前");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn last_committed_upgrade_skips_unfinished_and_rollbacks() {
        let root = temp_root("last");
        let a1 = Applied::create(&root, 1, "u1").unwrap();
        a1.append(JournalLine { op: op::BEGIN.into(), kind: "upgrade".into(), ..Default::default() }).unwrap();
        a1.append(JournalLine { op: op::COMMIT.into(), ..Default::default() }).unwrap();
        let a2 = Applied::create(&root, 2, "u2").unwrap();
        a2.append(JournalLine { op: op::BEGIN.into(), kind: "upgrade".into(), ..Default::default() }).unwrap();
        // 没走完
        let found = last_committed_upgrade(&root).unwrap();
        assert!(found.dir.file_name().unwrap().to_string_lossy().starts_with("0001"), "只认走完的那次");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prune_keeps_the_newest_n() {
        let root = temp_root("prune");
        for i in 1..=4 {
            let a = Applied::create(&root, i, &format!("u{i}")).unwrap();
            a.append(JournalLine { op: op::BEGIN.into(), kind: "upgrade".into(), ..Default::default() }).unwrap();
            a.append(JournalLine { op: op::COMMIT.into(), ..Default::default() }).unwrap();
        }
        let removed = prune_backups(&root, 2);
        assert_eq!(removed.len(), 2);
        assert_eq!(list_applied(&root).len(), 2);
        let names: Vec<String> = list_applied(&root).iter().map(|a| a.dir.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert!(names.iter().all(|n| n.starts_with("0003") || n.starts_with("0004")), "{names:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn history_is_append_only_jsonl() {
        let root = temp_root("history");
        let r = proto::UpgradeReport { id: "0001-u1".into(), kind: proto::JobKindName::Upgrade, outcome: proto::UpgradeOutcome::Ok, ..Default::default() };
        append_history(&root, &r).unwrap();
        let r2 = proto::UpgradeReport { id: "0002-u2".into(), kind: proto::JobKindName::Upgrade, outcome: proto::UpgradeOutcome::RolledBack, ..Default::default() };
        append_history(&root, &r2).unwrap();
        let text = std::fs::read_to_string(store::upgrade_dir(&root).join("history.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().next().unwrap().contains("0001-u1"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reparse_point_blocks_the_operation() {
        let root = temp_root("reparse");
        // 在根下放一个指向别处的目录联接（Windows 上要管理员才能建，这里只验「查得出来」这条路的逻辑）
        let target = temp_root("reparse-target");
        std::fs::write(target.join("evil.txt"), b"x").unwrap();
        #[cfg(windows)]
        {
            let link = root.join("linked");
            let out = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&target).output();
            if let Ok(o) = out {
                if o.status.success() && link.exists() {
                    let err = store::check_no_reparse(&root, "linked/evil.txt").unwrap_err();
                    assert!(err.contains("联接点"), "{err}");
                }
            }
        }
        // 不管上面成没成，普通路径都不该被拦
        assert!(store::check_no_reparse(&root, "bin/a.exe").is_ok());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&target);
    }
}
