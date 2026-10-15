//! 项目定位器（ADR-078 D2）：项目码 → 库目录，只在人指定的搜索根里按 `<code>000` 找。
//!
//! core.dll 找一个库文件靠的是声明自带的项目码：`DB_DB::makeDirectoryName`（`0x590b4a0`）拼
//! `/%<PROJ>000%`，而 `%ACP000%` 由 E3D 的 evars 脚本设成某个目录——E3D 的「已知项目全集」就是
//! 那些环境变量（`DLL_PDMSProject::BuildProjectsList`，`0x5840b80`），没有第二张表。我们不解析那些
//! 脚本、不读环境变量（d-682 第 3 条）：人把装着项目的目录（本地或 UNC）填成搜索根，这里只在
//! 根里按 `<code>000` 找。三种摆法照 `set_project_evars.bat` 自己的文档：
//!
//! ```text
//! root\<code>000                      // NOMAIN：库目录直接摆在根下
//! root\<Project>\<code>000            // 标准：AvevaCatalogue\acp000
//! root\<Project>\<path2>\<code>000    // 多一层：E3D3.1-slyk-project\YCYK-E3D\acw000
//! ```
//!
//! 规则（d-682 第 2 条）：
//!
//! - 目录名不区分大小写（`acp000` / `ZDJ000` / `SCB000` 随手写），`<code>000` 的判据沿用
//!   [`ends_with_db_suffix`]（`000` 前一位不是数字，`ams1000` 不算）。
//! - 一个根里同码两处 = 冲突，这个根对该码**不给答案**，继续看下一个根。
//! - 跨根同码：**先到的根赢**，其余进 `shadowed`，横幅 / health 里看得见——本地副本压过服务器副本
//!   这种取舍由根的顺序表达。
//! - 根不可达（UNC 掉线、没权限、路径写错）：每个根在独立线程里扫、带超时；到点放弃，只让它
//!   自己缺席，其余根照常。对着掉线的 SMB `read_dir` 可能挂 20–40 s，所以**不能**在调用方线程里裸调。
//! - 三处摆法都没有 → [`LocateFailure::NotFound`]，按码点名，不猜。
//! - 只看目录名、不开任何库文件——文件自证仍归路由目录（lazy-startup 不变量 3）。
//!
//! 深度 ≤ 3 的扫描在大共享盘上可能慢：一个项目目录里一旦见到 `<code>000`，它其余的子目录
//! （`amsdflts` / `amsmac` / `amspic` ……）就不再往下看——第三层只为「多一层」摆法存在，
//! 那种摆法下中间那层本身没有库目录。`back` / `backup` 目录跳过。

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::project_paths::{ends_with_db_suffix, path_identity};

/// 单个搜索根的扫描超时。掉线 SMB 上 `read_dir` 实测能挂 20–40 s，15 s 是「别让启动等它」
/// 与「慢一点的共享盘还来得及答」之间拍的一个数；到点按不可达处理，重挂轮下一次会再试。
pub const DEFAULT_ROOT_SCAN_TIMEOUT: Duration = Duration::from_secs(15);

/// 扫描时跳过的目录名（不区分大小写）：备份目录里常有整份项目的旧副本，认成项目只会制造同码冲突。
const SKIPPED_DIR_NAMES: [&str; 2] = ["back", "backup"];

/// 库目录名的后缀。
const DB_DIR_SUFFIX: &str = "000";

/// 一个搜索根里扫到的一个库目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbDirHit {
    /// 小写项目码（`acp`）。
    pub code: String,
    /// 装着这个库目录的项目目录；NOMAIN 摆法下就是根本身。
    pub project_dir: PathBuf,
    /// `<code>000` 目录本身。
    pub db_dir: PathBuf,
}

/// 一个搜索根的扫描结论——无论成败都要报出来（共享盘哲学：掉线的根只让它自己缺席，但要说）。
#[derive(Debug, Clone)]
pub struct RootScan {
    pub root: PathBuf,
    /// 目录读得动。`false` 时 `problem` 有值、`hits` 为空。
    pub reachable: bool,
    pub problem: Option<String>,
    pub hits: Vec<DbDirHit>,
    pub elapsed: Duration,
}

impl RootScan {
    /// 这个根里见到的项目码（小写、去重、排序）。
    pub fn codes_seen(&self) -> Vec<String> {
        let mut codes: Vec<String> = self.hits.iter().map(|hit| hit.code.clone()).collect();
        codes.sort();
        codes.dedup();
        codes
    }

    fn unreachable(root: PathBuf, problem: String, elapsed: Duration) -> Self {
        Self {
            root,
            reachable: false,
            problem: Some(problem),
            hits: Vec::new(),
            elapsed,
        }
    }
}

/// 扫全部搜索根。每个根一个线程、共用一个截止时刻；超时的根按不可达处理，线程自行收尾。
///
/// 逐根打开始 / 结果行：掉线共享盘上这一段能静默挂满整个超时窗，只有「完成」的话
/// 现场看不出是哪个根在卡（2026-10-15 JEU 现场：控制台外观就是「启动冻结」）。
pub fn scan_roots(roots: &[PathBuf], timeout: Duration) -> Vec<RootScan> {
    let mut receivers = Vec::with_capacity(roots.len());
    for root in roots {
        println!("[root-scan] 搜索根扫描开始: root={}", root.display());
        let (sender, receiver) = mpsc::channel();
        let root = root.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("project-locator {}", root.display()))
            .spawn(move || {
                let started = Instant::now();
                let outcome = scan_root_blocking(&root);
                // 接收方已经放弃（超时）时这里发不出去，忽略即可。
                let _ = sender.send((outcome, started.elapsed()));
            });
        receivers.push(spawned.map(|_| receiver).map_err(|error| error.to_string()));
    }

    let deadline = Instant::now() + timeout;
    roots
        .iter()
        .zip(receivers)
        .map(|(root, receiver)| {
            let receiver = match receiver {
                Ok(receiver) => receiver,
                Err(error) => {
                    println!("[root-scan] 搜索根扫描起不了线程: root={} reason={error}", root.display());
                    return RootScan::unreachable(
                        root.clone(),
                        format!("起不了扫描线程: {error}"),
                        Duration::ZERO,
                    );
                }
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok((Ok(hits), elapsed)) => {
                    println!(
                        "[root-scan] 搜索根扫描完成: root={} hits={} elapsed_ms={}",
                        root.display(),
                        hits.len(),
                        elapsed.as_millis()
                    );
                    RootScan {
                        root: root.clone(),
                        reachable: true,
                        problem: None,
                        hits,
                        elapsed,
                    }
                }
                Ok((Err(error), elapsed)) => {
                    println!(
                        "[root-scan] 搜索根不可达: root={} reason={error} elapsed_ms={}",
                        root.display(),
                        elapsed.as_millis()
                    );
                    RootScan::unreachable(
                        root.clone(),
                        format!("目录不可读（共享盘掉线 / 路径写错 / 无权限）: {error}"),
                        elapsed,
                    )
                }
                Err(_) => {
                    println!(
                        "[root-scan] 搜索根扫描超时: root={} timeout_s={}",
                        root.display(),
                        timeout.as_secs()
                    );
                    RootScan::unreachable(
                        root.clone(),
                        format!(
                            "扫描 {} s 未答，按不可达处理（共享盘掉线时 read_dir 会挂很久）；重挂轮会再试",
                            timeout.as_secs()
                        ),
                        timeout,
                    )
                }
            }
        })
        .collect()
}

/// 阻塞地扫一个根（见模块头的三种摆法与「见到库目录就不再往下」的规则）。
fn scan_root_blocking(root: &Path) -> io::Result<Vec<DbDirHit>> {
    let metadata = std::fs::metadata(root)?;
    if !metadata.is_dir() {
        return Err(io::Error::other("不是目录"));
    }
    let mut hits = Vec::new();
    if ends_with_db_suffix(root) {
        // 共享盘上把库目录本身单独共享出来（`\\host\share\ams000`）是常见做法，
        // 此时根就是库目录、也是「项目目录」。
        if let Some(code) = code_of(root) {
            hits.push(DbDirHit {
                code,
                project_dir: root.to_path_buf(),
                db_dir: root.to_path_buf(),
            });
        }
        return Ok(hits);
    }

    for level1 in subdirs(root)? {
        if ends_with_db_suffix(&level1) {
            push_hit(&mut hits, root, &level1);
            continue;
        }
        if is_skipped(&level1) {
            continue;
        }
        // 单个项目目录读不动只跳过它自己，不牵连整个根。
        let level2_dirs = match subdirs(&level1) {
            Ok(dirs) => dirs,
            Err(error) => {
                log::warn!("跳过读不动的目录（{}）: {error}", level1.display());
                continue;
            }
        };
        let mut found_here = false;
        let mut deeper = Vec::new();
        for level2 in level2_dirs {
            if ends_with_db_suffix(&level2) {
                push_hit(&mut hits, &level1, &level2);
                found_here = true;
            } else if !is_skipped(&level2) {
                deeper.push(level2);
            }
        }
        if found_here {
            // 这是一个项目目录：它其余的子目录是 `amsdflts` / `amsmac` 那些，不是更多项目。
            continue;
        }
        for level2 in deeper {
            let level3_dirs = match subdirs(&level2) {
                Ok(dirs) => dirs,
                Err(error) => {
                    log::warn!("跳过读不动的目录（{}）: {error}", level2.display());
                    continue;
                }
            };
            for level3 in level3_dirs {
                if ends_with_db_suffix(&level3) {
                    push_hit(&mut hits, &level2, &level3);
                }
            }
        }
    }
    // `read_dir` 顺序不是契约；排好序，先到先赢的判定才可复现。
    hits.sort_by(|left, right| left.db_dir.cmp(&right.db_dir));
    Ok(hits)
}

fn push_hit(hits: &mut Vec<DbDirHit>, project_dir: &Path, db_dir: &Path) {
    if let Some(code) = code_of(db_dir) {
        hits.push(DbDirHit {
            code,
            project_dir: project_dir.to_path_buf(),
            db_dir: db_dir.to_path_buf(),
        });
    }
}

/// 列出目录下的子目录。单个条目读不动只跳过并告警；`read_dir` 本身失败才算这个目录失败。
fn subdirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::warn!("跳过读不动的目录项（{}）: {error}", dir.display());
                continue;
            }
        };
        let path = entry.path();
        let is_dir = match entry.file_type() {
            // 目录联接（junction）与目录符号链接在 `file_type()` 里报 symlink、不报 dir，
            // 而现场把项目目录挂成联接指到别的卷是常见摆法；跟到目标再判一次，断链的自然落空。
            Ok(kind) => kind.is_dir() || (kind.is_symlink() && path.is_dir()),
            Err(_) => path.is_dir(),
        };
        if is_dir {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// `acp000` → `acp`。调用前已经过 [`ends_with_db_suffix`]。
fn code_of(db_dir: &Path) -> Option<String> {
    let name = db_dir.file_name()?.to_str()?;
    let code = name.strip_suffix(DB_DIR_SUFFIX)?;
    if code.is_empty() {
        return None;
    }
    Some(code.to_ascii_lowercase())
}

fn is_skipped(dir: &Path) -> bool {
    dir.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            SKIPPED_DIR_NAMES
                .iter()
                .any(|skipped| name.eq_ignore_ascii_case(skipped))
        })
}

/// 定位成功：这个项目码（或项目目录）在哪个根、哪个目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLocation {
    /// 大写项目码；按目录名定位时是该目录下见到的码（多个时以 `+` 连）。
    pub code: String,
    /// 赢的那个根在 `scans` 里的下标。
    pub root_index: usize,
    pub project_dir: PathBuf,
    /// 这个项目目录下的库目录（按目录名定位时含全部码）。
    pub db_dirs: Vec<PathBuf>,
    /// 后面的根里同码 / 同名的副本：只记不选。
    pub shadowed: Vec<PathBuf>,
    /// 前面的根里因为同码两处而放弃的那些，给人看。
    pub problems: Vec<String>,
}

/// 定位失败的每一种原因都说清找过哪里；不存在「找不到就凑一个」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocateFailure {
    /// 可达的根都看过了、都没有。`unreachable` 非空时它可能在那里，但这里不猜。
    NotFound {
        searched: Vec<PathBuf>,
        unreachable: Vec<PathBuf>,
    },
    /// 每一处命中都在冲突里（同一个根下两处），没有一个根给得出答案。
    Conflict {
        conflicts: Vec<(PathBuf, Vec<PathBuf>)>,
    },
}

impl fmt::Display for LocateFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound {
                searched,
                unreachable,
            } => {
                if searched.is_empty() {
                    write!(f, "没有一个搜索根可达")?;
                } else {
                    write!(f, "在 {} 个可达的搜索根里都没有（找过：{}）", searched.len(), join_paths(searched))?;
                }
                if !unreachable.is_empty() {
                    write!(
                        f,
                        "；另有 {} 个根不可达（{}），它可能在那里",
                        unreachable.len(),
                        join_paths(unreachable)
                    )?;
                }
                Ok(())
            }
            Self::Conflict { conflicts } => {
                write!(f, "同一个搜索根里出现两处，无法取舍：")?;
                for (index, (root, dirs)) in conflicts.iter().enumerate() {
                    if index > 0 {
                        write!(f, "；")?;
                    }
                    write!(f, "{} 下 {}", root.display(), join_paths(dirs))?;
                }
                Ok(())
            }
        }
    }
}

fn join_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// 按项目码定位：先到的根赢，同根两处冲突不选，其余记影子。
pub fn locate_code(scans: &[RootScan], code: &str) -> Result<ProjectLocation, LocateFailure> {
    let wanted = code.trim().to_ascii_lowercase();
    locate_by(scans, &wanted.to_ascii_uppercase(), |hit| hit.code == wanted)
}

/// 按项目目录名定位（主项目：`project_name` 是文件夹名）：匹配 `project_dir` 的最后一段，
/// 不区分大小写；NOMAIN 摆法下根本身就是项目目录，`project_path` 直接指到项目文件夹也认。
pub fn locate_project_folder(
    scans: &[RootScan],
    folder: &str,
) -> Result<ProjectLocation, LocateFailure> {
    let wanted = folder.trim();
    let mut location = locate_by(scans, "", |hit| {
        hit.project_dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
    })?;
    let mut codes: Vec<String> = location
        .db_dirs
        .iter()
        .filter_map(|dir| code_of(dir))
        .map(|code| code.to_ascii_uppercase())
        .collect();
    codes.sort();
    codes.dedup();
    location.code = codes.join("+");
    Ok(location)
}

fn locate_by(
    scans: &[RootScan],
    code: &str,
    matches: impl Fn(&DbDirHit) -> bool,
) -> Result<ProjectLocation, LocateFailure> {
    let mut winner: Option<ProjectLocation> = None;
    let mut shadowed = Vec::new();
    let mut conflicts: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for (index, scan) in scans.iter().enumerate() {
        let hits: Vec<&DbDirHit> = scan.hits.iter().filter(|hit| matches(hit)).collect();
        if hits.is_empty() {
            continue;
        }
        // 同一个根里落在不同项目目录下 = 冲突（备份副本、拷来拷去的旧项目）。
        let mut by_project_dir: BTreeMap<String, Vec<&DbDirHit>> = BTreeMap::new();
        for hit in &hits {
            by_project_dir
                .entry(path_identity(&hit.project_dir))
                .or_default()
                .push(hit);
        }
        if by_project_dir.len() > 1 {
            conflicts.push((
                scan.root.clone(),
                hits.iter().map(|hit| hit.db_dir.clone()).collect(),
            ));
            continue;
        }
        let db_dirs: Vec<PathBuf> = hits.iter().map(|hit| hit.db_dir.clone()).collect();
        match winner.as_mut() {
            None => {
                winner = Some(ProjectLocation {
                    code: code.to_string(),
                    root_index: index,
                    project_dir: hits[0].project_dir.clone(),
                    db_dirs,
                    shadowed: Vec::new(),
                    problems: Vec::new(),
                });
            }
            Some(_) => shadowed.extend(db_dirs),
        }
    }
    match winner {
        Some(mut location) => {
            location.shadowed = shadowed;
            location.problems = conflicts
                .iter()
                .map(|(root, dirs)| {
                    format!(
                        "搜索根 {} 下同码出现两处（{}），该根对它不给答案，已改用后面的根",
                        root.display(),
                        join_paths(dirs)
                    )
                })
                .collect();
            Ok(location)
        }
        None if !conflicts.is_empty() => Err(LocateFailure::Conflict { conflicts }),
        None => Err(LocateFailure::NotFound {
            searched: scans
                .iter()
                .filter(|scan| scan.reachable)
                .map(|scan| scan.root.clone())
                .collect(),
            unreachable: scans
                .iter()
                .filter(|scan| !scan.reachable)
                .map(|scan| scan.root.clone())
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 每个用例一个独立的临时搜索根，`Drop` 时清掉。
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            static SEQ: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "aios-locator-{name}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("create fixture root");
            Self { root }
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            std::fs::create_dir_all(&path).expect("create fixture dir");
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn scan(roots: &[&Path]) -> Vec<RootScan> {
        scan_roots(
            &roots.iter().map(|root| root.to_path_buf()).collect::<Vec<_>>(),
            DEFAULT_ROOT_SCAN_TIMEOUT,
        )
    }

    fn same_dir(left: &Path, right: &Path) -> bool {
        path_identity(left) == path_identity(right)
    }

    /// `set_project_evars.bat` 文档化的三种摆法都要认，`project_dir` 各自指对。
    #[test]
    fn locator_finds_the_three_documented_layouts() {
        let fixture = Fixture::new("layouts");
        let nomain = fixture.dir("x000");
        let standard = fixture.dir("P/y000");
        let nested = fixture.dir("Q/sub/z000");
        let scans = scan(&[&fixture.root]);
        assert!(scans[0].reachable, "{:?}", scans[0].problem);
        assert_eq!(scans[0].codes_seen(), vec!["x", "y", "z"]);

        let x = locate_code(&scans, "X").expect("x000 直接摆在根下");
        assert!(same_dir(&x.project_dir, &fixture.root));
        assert_eq!(x.db_dirs, vec![nomain]);

        let y = locate_code(&scans, "y").expect("P\\y000");
        assert!(same_dir(&y.project_dir, &fixture.root.join("P")));
        assert_eq!(y.db_dirs, vec![standard]);

        let z = locate_code(&scans, "Z").expect("Q\\sub\\z000");
        assert!(same_dir(&z.project_dir, &fixture.root.join("Q/sub")));
        assert_eq!(z.db_dirs, vec![nested]);
        assert_eq!(z.code, "Z");
    }

    /// 目录名大小写随手写，码也随手写，都对得上；`ams1000` 那种不是库目录。
    #[test]
    fn locator_is_case_insensitive_on_code_dirs() {
        let fixture = Fixture::new("case");
        let zdj = fixture.dir("ZDJ/ZDJ000");
        let scb = fixture.dir("scb/SCB000");
        fixture.dir("Noise/ams1000");
        let scans = scan(&[&fixture.root]);
        assert_eq!(locate_code(&scans, "zdj").unwrap().db_dirs, vec![zdj]);
        assert_eq!(locate_code(&scans, "Scb").unwrap().db_dirs, vec![scb]);
        assert!(matches!(
            locate_code(&scans, "ams1"),
            Err(LocateFailure::NotFound { .. })
        ));
    }

    /// 一个根下同码两处 = 冲突，不选；`problems` 里点名，`code` 无答案。
    #[test]
    fn same_code_twice_in_one_root_is_a_conflict_not_a_pick() {
        let fixture = Fixture::new("conflict");
        fixture.dir("A/cat000");
        fixture.dir("B/cat000");
        let scans = scan(&[&fixture.root]);
        let failure = locate_code(&scans, "cat").expect_err("两处必须是冲突");
        match &failure {
            LocateFailure::Conflict { conflicts } => {
                assert_eq!(conflicts.len(), 1);
                assert_eq!(conflicts[0].1.len(), 2);
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        assert!(failure.to_string().contains("两处"), "{failure}");
    }

    /// 两个根都有 → 根#1 赢，根#2 的进影子；根#1 冲突时改用根#2 并把冲突记进 problems。
    #[test]
    fn earlier_root_wins_and_later_copies_are_shadowed() {
        let first = Fixture::new("root1");
        let second = Fixture::new("root2");
        let local = first.dir("Cat/cat000");
        let remote = second.dir("Cat/cat000");
        let scans = scan(&[&first.root, &second.root]);
        let location = locate_code(&scans, "CAT").unwrap();
        assert_eq!(location.root_index, 0);
        assert_eq!(location.db_dirs, vec![local]);
        assert_eq!(location.shadowed, vec![remote.clone()]);

        // 根#1 里同码两处：放弃它，根#2 给答案，冲突留在 problems。
        first.dir("Copy/cat000");
        let scans = scan(&[&first.root, &second.root]);
        let location = locate_code(&scans, "cat").unwrap();
        assert_eq!(location.root_index, 1);
        assert_eq!(location.db_dirs, vec![remote]);
        assert_eq!(location.problems.len(), 1, "{:?}", location.problems);
    }

    /// 掉线的根只让它自己缺席：其余根照常，依赖它的码以「根不可达」为由落空，且不拖到超时之后。
    #[test]
    fn an_unreachable_root_is_reported_and_does_not_block_the_others() {
        let fixture = Fixture::new("unreachable");
        let good = fixture.dir("Good/good000");
        let missing = fixture.root.join("does-not-exist");
        let started = Instant::now();
        let scans = scan_roots(
            &[missing.clone(), fixture.root.clone()],
            Duration::from_secs(5),
        );
        assert!(started.elapsed() < Duration::from_secs(5), "不该等到超时");
        assert!(!scans[0].reachable);
        assert!(scans[0].problem.as_deref().unwrap_or("").contains("不可读"));
        assert!(scans[1].reachable);
        assert_eq!(locate_code(&scans, "good").unwrap().db_dirs, vec![good]);
        match locate_code(&scans, "gone") {
            Err(LocateFailure::NotFound {
                searched,
                unreachable,
            }) => {
                assert_eq!(searched.len(), 1);
                assert_eq!(unreachable, vec![missing]);
            }
            other => panic!("expected NotFound with an unreachable root, got {other:?}"),
        }
    }

    /// 主项目按文件夹名找：`AvevaMarineSample` 下全部 `*000` 都收，`code` 记的是它们的码。
    #[test]
    fn a_project_folder_is_found_by_name_with_all_its_db_dirs() {
        let fixture = Fixture::new("folder");
        let ams = fixture.dir("AvevaMarineSample/ams000");
        let extra = fixture.dir("AvevaMarineSample/amx000");
        fixture.dir("AvevaMarineSample/amsdflts");
        let scans = scan(&[&fixture.root]);
        let location = locate_project_folder(&scans, "avevamarinesample").unwrap();
        assert!(same_dir(&location.project_dir, &fixture.root.join("AvevaMarineSample")));
        assert_eq!(location.db_dirs, vec![ams, extra]);
        assert_eq!(location.code, "AMS+AMX");
        assert!(matches!(
            locate_project_folder(&scans, "Nope"),
            Err(LocateFailure::NotFound { .. })
        ));
    }

    /// `project_path` 直接指到库目录（`\\host\share\ams000`）或项目文件夹时也认。
    #[test]
    fn a_root_that_is_itself_a_db_dir_or_a_project_folder_is_used_directly() {
        let fixture = Fixture::new("root-shapes");
        let db_dir = fixture.dir("ams000");
        let scans = scan(&[&db_dir]);
        let location = locate_code(&scans, "ams").unwrap();
        assert_eq!(location.db_dirs, vec![db_dir.clone()]);
        assert!(same_dir(&location.project_dir, &db_dir));
        assert!(locate_project_folder(&scans, "ams000").is_ok());

        let project = Fixture::new("root-is-project");
        let inner = project.dir("prj000");
        let scans = scan(&[&project.root]);
        let location = locate_code(&scans, "prj").unwrap();
        assert_eq!(location.db_dirs, vec![inner]);
        assert!(same_dir(&location.project_dir, &project.root));
    }

    /// 搜索根里的项目目录挂成目录联接（junction）指到别的卷时照样扫得到：
    /// `read_dir` 的 `file_type()` 对联接报 symlink 不报 dir，不跟一步整根就是空的。
    #[cfg(windows)]
    #[test]
    fn a_project_folder_behind_a_junction_is_still_scanned() {
        let target = Fixture::new("junction-target");
        let real = target.dir("AvevaCatalogue/acp000");
        let root = Fixture::new("junction-root");
        let link = root.root.join("AvevaCatalogue");
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(target.root.join("AvevaCatalogue"))
            .output();
        // 建不了联接的机器（策略限制）上这条用例没有判据可言，跳过。
        match made {
            Ok(output) if output.status.success() => {}
            _ => return,
        }
        let scans = scan(&[&root.root]);
        assert!(scans[0].reachable, "{:?}", scans[0].problem);
        assert_eq!(scans[0].codes_seen(), vec!["acp"]);
        let location = locate_code(&scans, "ACP").expect("联接后面的 acp000");
        assert_eq!(location.db_dirs, vec![link.join("acp000")]);
        assert!(real.is_dir());
    }

    /// 备份目录跳过；见到库目录的项目目录不再往下翻（`amsdflts` 里不会有项目）。
    #[test]
    fn backup_dirs_are_skipped_and_project_dirs_are_not_descended() {
        let fixture = Fixture::new("skip");
        let real = fixture.dir("Proj/proj000");
        fixture.dir("backup/Proj/proj000");
        fixture.dir("Proj/projdflts/old000");
        let scans = scan(&[&fixture.root]);
        assert_eq!(scans[0].codes_seen(), vec!["proj"]);
        assert_eq!(locate_code(&scans, "proj").unwrap().db_dirs, vec![real]);
    }
}
