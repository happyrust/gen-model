//! 机器级写者锁（`docs/plans/2026-09-10-watermark-mirror-and-async-data-window-plan.md` §2「写者约束」）。
//!
//! 运行目录锁（lib.rs `ProcessInstanceLock`）挡的是「一个运行目录一个进程」；两个运行目录各自配着
//! 同一台 SurrealDB 的同一个 namespace / database 时它拦不住——那正是两个写者互踩水位镜像、互相
//! 覆盖 CAS 的形状。这把锁按 **端点 + namespace + database** 落在机器级目录下：同机第二个写者拿
//! 不到就得停止写入（服务、初始化工具、维护与清理入口共用同一把，由各入口在**任何库写之前**拿）。
//! 跨机器不在本轮支持范围：外部维护写入前必须先停服务（计划 §2）。
//!
//! 锁的持有 = 一个打开着的文件句柄：Windows 用 deny-share 打开（`share_mode(0)`），Unix 用 `flock`
//! （`File::try_lock`）；两者都随进程退出（含 kill）由内核收回，不留需要人清的陈锁——与 lib.rs 的
//! 单实例锁同一形状（ISSUE-023 的教训：「锁文件存不存在」那种形状会留陈锁、也曾被写成 `Ok(())`）。
//! 锁文件里写着持有者（项目 / 运行目录 / pid / 时刻），拿不到时报出锁路径与身份三元组。
//!
//! `data_face_signature` 继续做数据面兼容校验，不再作为唯一写者证明（计划 §2）。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use aios_core::options::DbOption;
use sha2::{Digest, Sha256};

use crate::options::StoreMode;

/// 覆盖锁目录的环境变量（测试与并排部署用）。缺省见 [`default_lock_dir`]。
pub const LOCK_DIR_ENV: &str = "AIOS_WRITER_LOCK_DIR";

/// 要写的是哪台库的哪个 namespace / database。三元组任一不同就是不同的库，互不相干。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WriterIdentity {
    /// `host:port`。`spawned-*` 形态下也是本进程拉起的那台绑的地址——两个运行目录都想在同一个
    /// 端口上拉库，同样只能有一个写者。
    pub endpoint: String,
    pub namespace: String,
    pub database: String,
}

impl WriterIdentity {
    /// 从运行配置解出身份。`embedded-mem` 的持久层在进程内、没有第二个写者可言，回 None
    /// （与 lib.rs `instance_lock_skip_reason` 同一条例外）。
    pub fn from_db_option(option: &DbOption, store_mode: StoreMode) -> Option<Self> {
        if matches!(store_mode, StoreMode::EmbeddedMem) {
            return None;
        }
        Some(Self {
            endpoint: format!("{}:{}", option.v_ip.trim(), option.v_port),
            namespace: option.surreal_ns.clone(),
            database: option.project_name.clone(),
        })
    }

    /// 锁文件名：可读前缀 + 三元组的 SHA-256 前 16 字节。前缀只为了人在目录里认得出，
    /// 身份靠摘要——namespace / database 里可能有文件名不许出现的字符。
    pub fn lock_file_name(&self) -> String {
        let mut digest = Sha256::new();
        for part in [&self.endpoint, &self.namespace, &self.database] {
            digest.update((part.len() as u64).to_le_bytes());
            digest.update(part.as_bytes());
        }
        let hash = hex::encode(&digest.finalize()[..16]);
        format!("{}-{hash}.writer.lock", sanitize(&self.database))
    }
}

fn sanitize(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    if cleaned.is_empty() {
        "db".to_owned()
    } else {
        cleaned
    }
}

/// 持有中的写者锁。**句柄活多久锁就活多久**：放在进程级 `OnceLock` / 长寿命结构里，别让它在
/// 函数末尾被 drop——drop 即放锁。
pub struct DatabaseWriterLock {
    _file: File,
    path: PathBuf,
    identity: WriterIdentity,
}

impl DatabaseWriterLock {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn identity(&self) -> &WriterIdentity {
        &self.identity
    }
}

impl std::fmt::Debug for DatabaseWriterLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseWriterLock")
            .field("path", &self.path)
            .field("identity", &self.identity)
            .finish()
    }
}

/// 拿不到锁时的分型：调用方一律停止写入，但报错要分得出「有人持着」与「锁目录不可用」。
#[derive(Debug)]
pub enum WriterLockError {
    /// 同机已有别的写者持着这把锁（多半是另一个运行目录的服务或维护工具）。
    HeldByOther {
        endpoint: String,
        namespace: String,
        database: String,
        path: PathBuf,
        source: std::io::Error,
    },
    /// 锁目录建不出、写不进：环境问题，同样不许写库（不能因为拿不到锁就当没有锁）。
    DirUnavailable {
        dir: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for WriterLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HeldByOther {
                endpoint,
                namespace,
                database,
                path,
                source,
            } => write!(
                f,
                "数据库 {endpoint} {namespace}/{database} 已有别的写者（锁 {}）：{source}。同一台库同时只许一个写者——停掉那个进程再来，或确认两个运行目录本不该指向同一个库",
                path.display()
            ),
            Self::DirUnavailable { dir, source } => write!(
                f,
                "写者锁目录 {} 不可用：{source}（可用 {LOCK_DIR_ENV} 指到别处）",
                dir.display()
            ),
        }
    }
}

impl std::error::Error for WriterLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::HeldByOther { source, .. } | Self::DirUnavailable { source, .. } => Some(source),
        }
    }
}

/// 给这台库拿机器级写者锁；`holder` 是写进锁文件的持有者描述（项目 / 运行目录 / pid 由这里补）。
///
/// 目录取 [`LOCK_DIR_ENV`]，没设走 [`default_lock_dir`]。
pub fn acquire(
    identity: WriterIdentity,
    holder: &str,
) -> Result<DatabaseWriterLock, WriterLockError> {
    acquire_in(&lock_dir(), identity, holder)
}

/// 便捷入口：按运行配置与存储形态拿锁；`embedded-mem` 回 `Ok(None)`（不需要锁）。
pub fn acquire_for(
    option: &DbOption,
    store_mode: StoreMode,
    holder: &str,
) -> Result<Option<DatabaseWriterLock>, WriterLockError> {
    WriterIdentity::from_db_option(option, store_mode)
        .map(|identity| acquire(identity, holder))
        .transpose()
}

/// 在指定目录下拿锁（测试与 [`acquire`] 共用）。
pub fn acquire_in(
    dir: &Path,
    identity: WriterIdentity,
    holder: &str,
) -> Result<DatabaseWriterLock, WriterLockError> {
    fs::create_dir_all(dir).map_err(|source| WriterLockError::DirUnavailable {
        dir: dir.to_path_buf(),
        source,
    })?;
    let path = dir.join(identity.lock_file_name());
    let mut file = open_exclusive(&path).map_err(|source| {
        if is_held_by_other(&source) {
            WriterLockError::HeldByOther {
                endpoint: identity.endpoint.clone(),
                namespace: identity.namespace.clone(),
                database: identity.database.clone(),
                path: path.clone(),
                source,
            }
        } else {
            // 目录只读、路径太长……环境问题，不是有人持着。
            WriterLockError::DirUnavailable {
                dir: dir.to_path_buf(),
                source,
            }
        }
    })?;
    // 持有者写进文件供人排查；写不进不算拿锁失败（锁已经在手上了），只记日志。
    let owner = format!(
        "endpoint={}\nnamespace={}\ndatabase={}\nholder={holder}\nrun_dir={}\npid={}\nstarted_at={}\n",
        identity.endpoint,
        identity.namespace,
        identity.database,
        std::env::current_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|_| "?".to_owned()),
        std::process::id(),
        chrono::Local::now().to_rfc3339(),
    );
    if let Err(error) = file
        .set_len(0)
        .and_then(|()| file.write_all(owner.as_bytes()))
    {
        log::warn!(
            "写者锁 {} 已拿到，但写持有者信息失败：{error}",
            path.display()
        );
    }
    Ok(DatabaseWriterLock {
        _file: file,
        path,
        identity,
    })
}

/// 「被别的句柄占着」长什么样：Windows 的 deny-share 打开撞上已有句柄回 ERROR_SHARING_VIOLATION
/// （32，`ErrorKind::Uncategorized`），范围锁撞上回 ERROR_LOCK_VIOLATION（33）；Unix 的 flock 回
/// `WouldBlock`。其余错误（含 PermissionDenied——那是目录真的不让写）都是环境问题。
fn is_held_by_other(error: &std::io::Error) -> bool {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    error.kind() == std::io::ErrorKind::WouldBlock
        || (cfg!(windows)
            && matches!(
                error.raw_os_error(),
                Some(ERROR_SHARING_VIOLATION) | Some(ERROR_LOCK_VIOLATION)
            ))
}

/// 缺省锁目录：机器级、按用户可写。Windows 走 `%LOCALAPPDATA%\gen-model\writer-locks`，其余走
/// `$XDG_RUNTIME_DIR/gen-model/writer-locks`，都没有就退到系统临时目录下的同名子目录。
///
/// 故意不放运行目录：那正是运行目录锁挡不住的形状。也故意不放项目目录：项目树是甲方的数据。
pub fn default_lock_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("gen-model")
        .join("writer-locks")
}

fn lock_dir() -> PathBuf {
    std::env::var_os(LOCK_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_lock_dir)
}

/// 独占打开：Windows 用 deny-share（别的进程连读都打不开这个文件，句柄随进程退出释放）。
#[cfg(windows)]
fn open_exclusive(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
}

/// 独占打开：Unix 用 `flock(LOCK_EX | LOCK_NB)`（`File::try_lock`），锁挂在打开的文件描述上，
/// 进程一退内核就收。NFS 上 flock 可能只是尽力而为——那是部署指南的问题，这里不许把它变成静默放行。
#[cfg(not(windows))]
fn open_exclusive(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "flock held by another process",
        ),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(endpoint: &str, namespace: &str, database: &str) -> WriterIdentity {
        WriterIdentity {
            endpoint: endpoint.to_owned(),
            namespace: namespace.to_owned(),
            database: database.to_owned(),
        }
    }

    /// 同一身份 → 同一个锁文件；端点 / namespace / database 任一不同 → 不同文件。文件名里
    /// 的可读前缀只是给人看的，身份靠摘要，所以 database 里的怪字符不影响。
    #[test]
    fn the_lock_file_is_keyed_by_endpoint_namespace_and_database() {
        let a = identity("127.0.0.1:8000", "plant", "AvevaMarineSample");
        assert_eq!(a.lock_file_name(), a.clone().lock_file_name());
        assert_ne!(
            a.lock_file_name(),
            identity("127.0.0.1:8001", "plant", "AvevaMarineSample").lock_file_name()
        );
        assert_ne!(
            a.lock_file_name(),
            identity("127.0.0.1:8000", "plant2", "AvevaMarineSample").lock_file_name()
        );
        assert_ne!(
            a.lock_file_name(),
            identity("127.0.0.1:8000", "plant", "Other").lock_file_name()
        );
        // 长度前缀进摘要：("ab","c") 与 ("a","bc") 不许撞。
        assert_ne!(
            identity("x", "ab", "c").lock_file_name(),
            identity("x", "a", "bc").lock_file_name()
        );
        let weird = identity("h:1", "ns", "a b/c:d");
        assert!(weird.lock_file_name().starts_with("a_b_c_d-"));
        assert!(weird.lock_file_name().ends_with(".writer.lock"));
    }

    /// 第二个写者拿同一把锁必须失败并报「有人持着」；放掉（drop）之后立刻能再拿；
    /// 另一台库（不同身份）互不相干。
    #[test]
    fn a_second_writer_of_the_same_database_is_refused_until_the_first_lets_go() {
        let dir = tempfile::tempdir().expect("临时锁目录");
        let db = identity("127.0.0.1:8000", "plant", "AMS");
        let first = acquire_in(dir.path(), db.clone(), "test-first").expect("第一个写者拿到锁");
        assert!(first.path().starts_with(dir.path()));
        assert_eq!(first.identity(), &db);

        let refused = acquire_in(dir.path(), db.clone(), "test-second")
            .err()
            .expect("同一台库的第二个写者必须被拒");
        match &refused {
            WriterLockError::HeldByOther {
                endpoint,
                namespace,
                database,
                path,
                ..
            } => {
                assert_eq!(endpoint, "127.0.0.1:8000");
                assert_eq!(namespace, "plant");
                assert_eq!(database, "AMS");
                assert_eq!(path, first.path());
            }
            other => panic!("该是 HeldByOther：{other:?}"),
        }
        let message = refused.to_string();
        assert!(
            message.contains("plant/AMS") && message.contains("已有别的写者"),
            "{message}"
        );

        // 另一台库不受影响。
        let other = identity("127.0.0.1:8000", "plant", "OTHER");
        let _other_lock = acquire_in(dir.path(), other, "test-other").expect("别的库照常拿到");

        drop(first);
        let again = acquire_in(dir.path(), db, "test-third").expect("放掉之后能再拿");
        drop(again);
    }

    /// 锁目录建不出来是环境问题，报 DirUnavailable——不许把「拿不到锁」当成「不需要锁」。
    #[test]
    fn an_unusable_lock_directory_is_an_error_not_a_pass() {
        let dir = tempfile::tempdir().expect("临时目录");
        // 用一个普通文件占住「目录」的位置，create_dir_all 必然失败。
        let blocker = dir.path().join("not-a-dir");
        fs::write(&blocker, b"x").expect("写占位文件");
        let error = acquire_in(&blocker, identity("h:1", "ns", "db"), "test")
            .err()
            .expect("必须失败");
        assert!(
            matches!(error, WriterLockError::DirUnavailable { .. }),
            "{error:?}"
        );
    }

    /// `embedded-mem` 的库在进程内，没有第二个写者可言，不拿锁；其余形态都拿，端点是
    /// `v_ip:v_port`、database 是 `project_name`（`use_db` 用的就是它）。
    #[test]
    fn only_the_embedded_store_needs_no_writer_lock() {
        let mut option = DbOption::default();
        option.v_ip = " 127.0.0.1 ".to_owned();
        option.v_port = 8000;
        option.surreal_ns = "plant".to_owned();
        option.project_name = "AMS".to_owned();
        assert!(WriterIdentity::from_db_option(&option, StoreMode::EmbeddedMem).is_none());
        for mode in [
            StoreMode::External,
            StoreMode::SpawnedRocks,
            StoreMode::SpawnedMem,
        ] {
            let identity = WriterIdentity::from_db_option(&option, mode).expect("要拿锁");
            assert_eq!(identity.endpoint, "127.0.0.1:8000");
            assert_eq!(identity.namespace, "plant");
            assert_eq!(identity.database, "AMS");
        }
    }

    /// 缺省目录不在运行目录也不在项目目录：那两处正是这把锁要绕开的地方。
    #[test]
    fn the_default_lock_directory_is_machine_level() {
        let dir = default_lock_dir();
        assert!(
            dir.ends_with(Path::new("gen-model").join("writer-locks")),
            "{}",
            dir.display()
        );
        let cwd = std::env::current_dir().expect("cwd");
        assert!(
            !dir.starts_with(&cwd),
            "缺省锁目录不该落在运行目录下：{}",
            dir.display()
        );
    }
}
