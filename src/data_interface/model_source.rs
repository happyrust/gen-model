//! 生成源版本（ADR-054）：一个库在当前投影下按**哪一版数据**生成模型。
//!
//! 时点只有两种来源——调用方显式指定（历史投影的 `SessionSelector::Sesno / At`、增量窗口的
//! 右端），或**文件此刻自报的最新会话**。`dbnum_watermark.applied_sesno` 是摄入水位（ADR-001），
//! 不是第三种时点来源；`dbnum_watermark.file_path` 也不是库文件的权威——MDB 成员才是。
//! 否则从没解析过的项目看得见树、点不出模型，而文件比库新的那段时间模型停在旧数据上。
//!
//! 三件事各有一处权威，别处不得再有第二份：
//!
//! * **库文件在哪**——[`source_file_of`]，读 MDB 成员（`current_mdb_sources`）。
//! * **「最新」怎么解**——[`latest_source_version`]，复用历史投影那把尺子
//!   `historical_model::resolve_session(path, Latest)`；按文件身份（长度 + 修改时刻）缓存，
//!   文件没动就不重开。E3D 的保存是追加写，长度一变就重解。
//! * **生成根归哪个库**——[`dbnum_of_root`]，从 refno 自身**位解码**（`RefNo::dbno()`，与 core.dll
//!   `DB_Element::dbno` 同式），再核它在 MDB 成员清单里且是 DESI / ISOD（[`is_element_db`]），
//!   最后只在**那一个库**做一次 `element_exists` 索引点查确认元素存在。不再逐个打开全部 DESI
//!   试查，也没有按 word0 建的归属缓存——同一个库每个 extent 一个 word0，那张表是为一个不存在的
//!   歧义建的（切片计划 `2026-09-12-cross-db-attribute-refno-routing-dev-plan.md` §0.2 / §3.4 / W4）。
//!   失败按步骤分开说（[`RootDbError`]），调用方要分辨就 `downcast_ref`。

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;
use dashmap::DashMap;
use e3d_io::ReadOnlyEngine;
use e3d_io::refno::RefNo;
use once_cell::sync::Lazy;

use crate::data_interface::direct_store::DbPin;
use crate::data_interface::refno_route::refno_dbnum_of;
use crate::fast_model::e3d_model_service::current_mdb_sources;
use crate::fast_model::historical_model::{SessionSelector, resolve_session};

/// 一个库在当前投影下的生成源版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceVersion {
    pub dbnum: u32,
    pub db_type: String,
    pub file: PathBuf,
    pub sesno: u32,
    /// 该会话在 E3D 里的写入时刻（RFC3339）。读不到就是 `None`——**不许拿挂钟顶替**
    /// （plant-ui ADR-0019）；凭证比较只比会话号，时刻只是给人看的。
    pub session_time: Option<String>,
}

/// 文件身份：长度 + 修改时刻。与 `direct_store::FileIdentity` 同一口径，用来判缓存还新不新。
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    fn of(path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

static LATEST_BY_FILE: Lazy<DashMap<PathBuf, (FileStamp, SourceVersion)>> = Lazy::new(DashMap::new);

/// 当前 MDB 里 `dbnum` 的库类型与文件。
pub fn source_file_of(dbnum: u32) -> anyhow::Result<(String, PathBuf)> {
    source_file_lookup(dbnum)?.with_context(|| format!("当前 MDB 没有 dbnum {dbnum} 的库文件"))
}

/// 同上，但把「不在当前 MDB」与「成员名单读不到」分成两个答案：`Ok(None)` 是前者
/// （多半是换过 MDB 之后留下的历史水位行），`Err` 是后者。
///
/// 启动复核（09-04 审核 #7）要把这两栏分开陈述——把「不在 MDB」记进 `unreadable`，
/// 水位表里只要留着一行历史库，`/health.startup.reconcile.result` 就一辈子出不来 `ok`。
pub fn source_file_lookup(dbnum: u32) -> anyhow::Result<Option<(String, PathBuf)>> {
    let (pins, _) = current_mdb_sources()?;
    Ok(pins
        .into_iter()
        .find(|pin| pin.dbnum == dbnum as i32)
        .map(|pin| (pin.db_type, pin.file)))
}

/// 当前 MDB 声明的元素库（DESI / ISOD）库号（升序、去重）。
///
/// 启动复核「哪些库还没有凭证记录」那一栏的**唯一**来源（09-04 审核 #6）：模型面只认 MDB
/// 成员（本模块头），`DbnumState::list_registered()` 是数据面的登记表——它里面有 SYST /
/// DICT / CATA，也有换过 MDB 之后留下的历史行，拿它当分母会把没有模型面的库报成「无凭证」。
///
/// 类型集与 [`dbnum_of_root`] 同一把尺子（[`is_element_db`]，切片 W4 / §9 Q5）：分母与「根归哪个库」
/// 不能各认一套类型，否则 ISOD 的根定得出库、却永远不在「该有凭证」的名单里。
pub fn mdb_design_dbnums() -> anyhow::Result<Vec<u32>> {
    let (pins, _) = current_mdb_sources()?;
    let mut dbnums: Vec<u32> = pins
        .into_iter()
        .filter(|pin| is_element_db(&pin.db_type))
        .filter_map(|pin| u32::try_from(pin.dbnum).ok())
        .collect();
    dbnums.sort_unstable();
    dbnums.dedup();
    Ok(dbnums)
}

/// 生成根只能来自元素库：DESI 与 ISOD——与 baseline 的 417 库口径、`update_scope` 的准入、
/// `MdbRouteDirectory::element_databases()` 同一个集合。
pub fn is_element_db(db_type: &str) -> bool {
    db_type.eq_ignore_ascii_case("DESI") || db_type.eq_ignore_ascii_case("ISOD")
}

/// 未指定时点 → 文件最新（ADR-054 Q1）。
pub fn latest_source_version(dbnum: u32) -> anyhow::Result<SourceVersion> {
    let (db_type, file) = source_file_of(dbnum)?;
    latest_source_version_of_file(dbnum, &db_type, &file)
}

/// 同上，调用方已经知道文件在哪。
pub fn latest_source_version_of_file(
    dbnum: u32,
    db_type: &str,
    file: &Path,
) -> anyhow::Result<SourceVersion> {
    let stamp = FileStamp::of(file)
        .with_context(|| format!("stat dbnum {dbnum} 的库文件 {}", file.display()))?;
    if let Some(hit) = LATEST_BY_FILE.get(file) {
        if hit.0 == stamp {
            return Ok(hit.1.clone());
        }
    }
    let resolved = resolve_session(file, &SessionSelector::Latest)
        .with_context(|| format!("解 dbnum {dbnum} 的最新会话 {}", file.display()))?;
    let version = SourceVersion {
        dbnum,
        db_type: db_type.to_string(),
        file: file.to_path_buf(),
        sesno: resolved.sesno,
        session_time: resolved.session_time,
    };
    LATEST_BY_FILE.insert(file.to_path_buf(), (stamp, version.clone()));
    Ok(version)
}

/// 生成根定库的失败，按发生在哪一步分开说（切片 §3.4 / §9 Q5 的三类
/// `InvalidRefNo` / `DbNotInMdb` / `ElementMissing`，外加开库自身的两种失败与类型不符）。
///
/// [`dbnum_of_root`] 交出去的是 `anyhow::Error`；要按类别分流就 `downcast_ref::<RootDbError>()`，
/// 别匹配文案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootDbError {
    /// `word1` 为负或解出 dbnum 0：这串数字不指向任何库。这一步**不读** MDB 成员清单。
    InvalidRefNo { root: RefNo },
    /// 编号合法，但当前 MDB 的成员清单里没有这个库——范围封闭，不回退扫描。
    DbNotInMdb { root: RefNo, dbnum: u32 },
    /// 库在 MDB 里，却不是 DESI / ISOD：生成根只能来自元素库。
    NotAnElementDb {
        root: RefNo,
        dbnum: u32,
        db_type: String,
    },
    /// 目标库文件打不开，或索引点查时 I/O 出错。
    Open {
        root: RefNo,
        dbnum: u32,
        file: PathBuf,
        detail: String,
    },
    /// 文件自报的库号与要开的不一致（不变量 3「开文件必核头」）：路径上的文件被换成了别的库，
    /// 或成员清单把 dbnum 绑到了错的文件。与 `DirectStore::open_session` / e3d-io `DbSet::db_slot`
    /// 同一条门。
    DbNumberMismatch {
        dbnum: u32,
        actual: u32,
        file: PathBuf,
    },
    /// 库、会话都对，索引里就是没有它：元素删了，或 refno 写错——典型是换过库之后客户端
    /// 还拿着旧的根 refno。`sesno` 是查的那个会话，排障时对表用。
    ElementMissing {
        root: RefNo,
        dbnum: u32,
        file: PathBuf,
        sesno: u32,
    },
}

impl fmt::Display for RootDbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRefNo { root } => {
                write!(
                    f,
                    "生成根 {root} 不是合法 RefNo（word1 为负或解出 dbnum 0）"
                )
            }
            Self::DbNotInMdb { root, dbnum } => {
                write!(
                    f,
                    "生成根 {root} 解出 dbnum {dbnum}，它不在当前 MDB 的成员清单里"
                )
            }
            Self::NotAnElementDb {
                root,
                dbnum,
                db_type,
            } => write!(
                f,
                "生成根 {root} 解出 dbnum {dbnum}，但那是 {db_type} 库，不是 DESI / ISOD"
            ),
            Self::Open {
                root,
                dbnum,
                file,
                detail,
            } => write!(
                f,
                "为点查生成根 {root} 打开 dbnum {dbnum} 的库文件 {}: {detail}",
                file.display()
            ),
            Self::DbNumberMismatch {
                dbnum,
                actual,
                file,
            } => write!(
                f,
                "dbnum {dbnum} 的库文件 {} 自报库号 {actual}：文件被换过，或成员清单绑错了文件",
                file.display()
            ),
            Self::ElementMissing {
                root,
                dbnum,
                file,
                sesno,
            } => write!(
                f,
                "生成根 {root} 解出 dbnum {dbnum}，但 {} 最新会话 {sesno} 的索引里没有它",
                file.display()
            ),
        }
    }
}

impl std::error::Error for RootDbError {}

/// 生成根所在的 DESI / ISOD 库。
///
/// 三步，每一步失败都说清是哪一步（[`RootDbError`]）：
/// 1. `RefNo::dbno()` 位解码——编号内生，不查表、不开库；非法引用在这里就停，连成员清单都不读；
/// 2. 解出的 dbnum 必须是当前 MDB 声明的 DESI / ISOD 成员——范围封闭，不在就是「不在这个 MDB」；
/// 3. 只在**那一个库**的最新会话索引里点查一次（[`element_exists`]）——区分「编号合法但元素
///    不存在」（删了、或 refno 写错）与「库不在」。开的是目标库那一个文件，不是全部 DESI；
///    查的是索引 B+tree，不读记录页。
pub fn dbnum_of_root(root: RefNo) -> anyhow::Result<u32> {
    dbnum_of_root_in(root, || Ok(current_mdb_sources()?.0))
}

/// 同 [`dbnum_of_root`]，MDB 成员清单由调用方给，且**惰性**取——非法 refno 不读清单。
///
/// 生产只有 [`dbnum_of_root`] 一个入口。这一层是为了拿合成库文件配 `e3d_io::read_stats`
/// 计数器验「只开目标库一次、零扫描」（`tests/refno_native_guards.rs`）：
/// `current_mdb_sources()` 要 DbOption 与 SYS 文件，单测里给不出。
pub fn dbnum_of_root_in(
    root: RefNo,
    mdb_members: impl FnOnce() -> anyhow::Result<Vec<DbPin>>,
) -> anyhow::Result<u32> {
    let dbnum = refno_dbnum_of(root).ok_or(RootDbError::InvalidRefNo { root })?;
    let members = mdb_members()?;
    let pin = members
        .iter()
        .find(|pin| pin.dbnum == dbnum as i32)
        .ok_or(RootDbError::DbNotInMdb { root, dbnum })?;
    if !is_element_db(&pin.db_type) {
        return Err(RootDbError::NotAnElementDb {
            root,
            dbnum,
            db_type: pin.db_type.clone(),
        }
        .into());
    }
    let probe = element_exists(pin, root)?;
    if !probe.found {
        return Err(RootDbError::ElementMissing {
            root,
            dbnum,
            file: pin.file.clone(),
            sesno: probe.sesno,
        }
        .into());
    }
    Ok(dbnum)
}

/// 目标库那一次点查的账：查的是哪个会话、索引里有没有。
struct IndexProbe {
    sesno: u32,
    found: bool,
}

/// 在 `pin` 指的那一个文件里点查 `root`：开一次库（最新会话）、核一次头、查一次索引 B+tree。
///
/// 与 e3d-io `DbSet::element_exists` 同一口径（`search_index`，命中与否都不读记录页）。这里不经
/// `DbSet` 是因为它要一个 resolver，而 W4 的成员清单仍是 `current_mdb_sources()` 那份 pins
/// （切片 §4：目录派生留到收敛 `current_mdb_source_inventory()` 时一起换）。本模块只有这一处开库。
fn element_exists(pin: &DbPin, root: RefNo) -> Result<IndexProbe, RootDbError> {
    let dbnum = pin.dbnum as u32;
    let io_failed = |detail: String| RootDbError::Open {
        root,
        dbnum,
        file: pin.file.clone(),
        detail,
    };
    let mut engine =
        ReadOnlyEngine::open(&pin.file).map_err(|error| io_failed(error.to_string()))?;
    let actual = engine.descriptor().db_mark;
    if actual != dbnum {
        return Err(RootDbError::DbNumberMismatch {
            dbnum,
            actual,
            file: pin.file.clone(),
        });
    }
    let sesno = engine.session().sesno;
    let found = engine
        .search_index(root)
        .map_err(|error| io_failed(format!("会话 {sesno} 索引点查: {error}")))?
        .is_some();
    Ok(IndexProbe { sesno, found })
}

/// 一个生成根在当前投影下的生成源版本：所在库 + 该库文件的最新会话。
pub fn root_source_version(root: RefNo) -> anyhow::Result<SourceVersion> {
    latest_source_version(dbnum_of_root(root)?)
}

/// 凭证是否覆盖要求的时点（ADR-054 实施约束 4）。
///
/// 判**单调**不判等值：凭证记的是「模型按哪一版数据生成」，比要求的新就是够新。等值判据
/// 会让 ensure 按最新 N+1 生成之后、增量管线按窗口右端 N 复核时把它判成过期——重排、再撞
/// 「不得回退」守卫，一条正确的新模型被报成批次失败。`0` 是「未认领会话号」（人工强制
/// 重试行），永不算覆盖。
pub fn credential_covers(credential_sesno: Option<i32>, required_sesno: i32) -> bool {
    credential_sesno.is_some_and(|sesno| sesno > 0 && sesno >= required_sesno)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_or_equal_credential_covers_the_required_session() {
        assert!(credential_covers(Some(106), 106));
        assert!(credential_covers(Some(107), 106));
        assert!(!credential_covers(Some(105), 106));
    }

    /// 人工强制重试行写的 `source_end_sesno = 0` 不是「按第 0 版数据生成」，是「没认领」。
    #[test]
    fn an_unclaimed_or_missing_credential_never_covers() {
        assert!(!credential_covers(Some(0), 0));
        assert!(!credential_covers(Some(0), 1));
        assert!(!credential_covers(None, 1));
    }

    /// `RefNo::dbno` 的逆：低 13 位原位，高位段左移 13，中间放 extent。
    fn word0_for(dbno: u32, extent: u32) -> u32 {
        (dbno & 0x1FFF) | ((dbno & 0x3E000) << 13) | (extent << 13)
    }

    fn pin(dbnum: i32, db_type: &str, file: &str) -> DbPin {
        DbPin {
            dbnum,
            db_type: db_type.to_string(),
            file: PathBuf::from(file),
            sesno: None,
        }
    }

    fn root_error(result: anyhow::Result<u32>) -> RootDbError {
        let error = result.expect_err("root resolution must fail");
        error
            .downcast_ref::<RootDbError>()
            .unwrap_or_else(|| panic!("not a RootDbError: {error:#}"))
            .clone()
    }

    /// 非法 refno 在位解码那一步就停，报错说的是「不是合法 RefNo」——连 MDB 成员清单都不读
    /// （本测试进程没有 DbOption，生产入口读了就会是另一种错；`_in` 入口的闭包压根不许被叫）。
    #[test]
    fn an_invalid_root_is_rejected_before_the_mdb_is_consulted() {
        for root in [
            RefNo::new(0, 0),
            RefNo::new(24_384, 0x8000_0000),
            RefNo::new(3 << 13, 5),
        ] {
            let error = dbnum_of_root(root).unwrap_err().to_string();
            assert!(error.contains("不是合法 RefNo"), "{root}: {error}");
            assert_eq!(
                root_error(dbnum_of_root_in(root, || -> anyhow::Result<Vec<DbPin>> {
                    unreachable!("非法 refno 不读 MDB 成员清单")
                })),
                RootDbError::InvalidRefNo { root }
            );
        }
    }

    /// 解出的库不在成员清单里 → `DbNotInMdb`，文案带 dbnum；不回退去开任何文件
    /// （清单里那个文件的路径根本不存在，开了就是 `Open`）。
    #[test]
    fn a_root_outside_the_mdb_is_refused_without_opening_anything() {
        let members = || Ok(vec![pin(1_000, "DESI", "T:/never/opened/tst1000_0001")]);
        let root = RefNo::new(word0_for(2_000, 1), 7);
        let error = dbnum_of_root_in(root, members).unwrap_err();
        assert!(error.to_string().contains("2000"), "{error}");
        assert_eq!(
            root_error(Err(error)),
            RootDbError::DbNotInMdb { root, dbnum: 2_000 }
        );
    }

    /// 库在 MDB 里但不是 DESI / ISOD → `NotAnElementDb`；DESI / ISOD 之外的一律不开。
    #[test]
    fn a_root_in_a_catalogue_database_is_refused() {
        let members = || {
            Ok(vec![
                pin(5_052, "CATA", "T:/never/opened/ams5052_0001"),
                pin(9_100, "DICT", "T:/never/opened/ams9100_0001"),
            ])
        };
        for (dbnum, db_type) in [(5_052, "CATA"), (9_100, "DICT")] {
            let root = RefNo::new(word0_for(dbnum, 1), 7);
            assert_eq!(
                root_error(dbnum_of_root_in(root, members)),
                RootDbError::NotAnElementDb {
                    root,
                    dbnum,
                    db_type: db_type.to_string()
                }
            );
        }
    }

    /// 声明了、类型也对、文件却打不开 → `Open`，把路径说出来；ISOD 与 DESI 同样走到这一步。
    #[test]
    fn a_declared_database_without_a_file_reports_the_open_failure() {
        let missing = std::env::temp_dir().join(format!(
            "model-source-missing-{}-isod1000_0001",
            std::process::id()
        ));
        let members = || Ok(vec![pin(1_000, "ISOD", missing.to_str().unwrap())]);
        let root = RefNo::new(word0_for(1_000, 1), 7);
        match root_error(dbnum_of_root_in(root, members)) {
            RootDbError::Open {
                root: reported,
                dbnum: 1_000,
                file,
                ..
            } => {
                assert_eq!(reported, root);
                assert_eq!(file, missing);
            }
            other => panic!("expected Open, got {other:?}"),
        }
    }

    /// 元素库类型集只有 DESI / ISOD，大小写不敏感；分母 `mdb_design_dbnums` 用的是同一个判据。
    #[test]
    fn element_databases_are_desi_and_isod_only() {
        for db_type in ["DESI", "desi", "ISOD", "isod"] {
            assert!(is_element_db(db_type), "{db_type}");
        }
        for db_type in ["CATA", "DICT", "PROP", "PADD", "SYST", "SCHE", ""] {
            assert!(!is_element_db(db_type), "{db_type}");
        }
        let source = include_str!("model_source.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production part");
        let body = source
            .split_once("pub fn mdb_design_dbnums(")
            .expect("mdb_design_dbnums exists")
            .1
            .split_once("\n}\n")
            .expect("function end")
            .0;
        assert!(
            body.contains("is_element_db(&pin.db_type)"),
            "分母的类型集必须与 dbnum_of_root 共用 is_element_db"
        );
    }

    /// 切片 §5.2 源码护栏：定库只靠位解码，不遍历 MDB 的全部 DESI，也没有按 word0 建的缓存；
    /// 开库与点查只在 `element_exists` 那一处，且恰好一次。
    #[test]
    fn dbnum_of_root_decodes_instead_of_probing_every_desi() {
        // 只看生产代码：测试模块自己就写着那个名字。
        let source = include_str!("model_source.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production part");
        assert!(
            !source.contains("DBNUM_BY_REF0"),
            "按 word0 键的归属缓存必须删干净"
        );
        let body = source
            .split_once("pub fn dbnum_of_root_in(")
            .expect("dbnum_of_root_in exists")
            .1
            .split_once("\n}\n")
            .expect("function end")
            .0;
        assert!(
            body.contains("refno_dbnum_of(root)"),
            "定库必须走位解码 helper"
        );
        assert!(!body.contains("for pin in"), "不得遍历 MDB 成员逐库试查");
        for needle in ["ReadOnlyEngine::open(", "find_element(", "scan_elements("] {
            assert!(
                !body.contains(needle),
                "定库函数体不得含 `{needle}`：开库与点查只在 element_exists 里"
            );
        }
        assert_eq!(
            body.matches("element_exists(").count(),
            1,
            "恰好一次目标库点查（§9 Q5）"
        );
        assert_eq!(
            source.matches("ReadOnlyEngine::open(").count(),
            1,
            "整个模块只在 element_exists 里开库"
        );
        assert!(!source.contains("scan_elements("), "定库不扫元素");
    }

    #[test]
    fn a_file_stamp_changes_when_the_file_grows() {
        let dir = std::env::temp_dir().join(format!(
            "model-source-stamp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("desi");
        std::fs::write(&file, b"session 1").unwrap();
        let before = FileStamp::of(&file).unwrap();
        std::fs::write(&file, b"session 1 + session 2").unwrap();
        let after = FileStamp::of(&file).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_ne!(before, after, "追加保存必须让缓存失效");
    }
}
