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
//!   `DB_Element::dbno` 同式），再核它在 MDB 成员清单里且是 DESI / ISOD，最后只在**那一个库**
//!   点查一次索引确认元素存在。不再逐个打开全部 DESI 试查，也没有按 word0 建的归属缓存——
//!   同一个库每个 extent 一个 word0，那张表是为一个不存在的歧义建的（切片计划
//!   `2026-09-12-cross-db-attribute-refno-routing-dev-plan.md` §0.2 / W4）。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;
use dashmap::DashMap;
use e3d_io::ReadOnlyEngine;
use e3d_io::refno::RefNo;
use once_cell::sync::Lazy;

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
/// （多半是换过 MDB 之后留下的历史 `gen_root` 行），`Err` 是后者。
///
/// 启动复核（09-04 审核 #7）要把这两栏分开陈述——把「不在 MDB」记进 `unreadable`，
/// `gen_root` 里只要留着一行历史库，`/health.startup.reconcile.result` 就一辈子出不来 `ok`。
pub fn source_file_lookup(dbnum: u32) -> anyhow::Result<Option<(String, PathBuf)>> {
    let (pins, _) = current_mdb_sources()?;
    Ok(pins
        .into_iter()
        .find(|pin| pin.dbnum == dbnum as i32)
        .map(|pin| (pin.db_type, pin.file)))
}

/// 当前 MDB 声明的 DESI 库号（升序、去重）。
///
/// 启动复核「哪些库还没有凭证记录」那一栏的**唯一**来源（09-04 审核 #6）：模型面只认 MDB
/// 成员（本模块头），`DbnumState::list_registered()` 是数据面的登记表——它里面有 SYST /
/// DICT / CATA，也有换过 MDB 之后留下的历史行，拿它当分母会把没有模型面的库报成「无凭证」。
pub fn mdb_design_dbnums() -> anyhow::Result<Vec<u32>> {
    let (pins, _) = current_mdb_sources()?;
    let mut dbnums: Vec<u32> = pins
        .into_iter()
        .filter(|pin| pin.db_type.eq_ignore_ascii_case("DESI"))
        .filter_map(|pin| u32::try_from(pin.dbnum).ok())
        .collect();
    dbnums.sort_unstable();
    dbnums.dedup();
    Ok(dbnums)
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

/// 生成根所在的 DESI / ISOD 库。
///
/// 三步，每一步失败都说清是哪一步：
/// 1. `RefNo::dbno()` 位解码——编号内生，不查表、不开库；
/// 2. 解出的 dbnum 必须是当前 MDB 声明的 DESI / ISOD 成员——范围封闭，不在就是「不在这个 MDB」；
/// 3. 只在**那一个库**的最新会话索引里点查一次——区分「编号合法但元素不存在」（删了、
///    或 refno 写错）与「库不在」。这一步开的是目标库那一个文件，不是全部 DESI。
pub fn dbnum_of_root(root: RefNo) -> anyhow::Result<u32> {
    let dbnum = refno_dbnum_of(root)
        .with_context(|| format!("生成根 {root} 不是合法 RefNo（word1 为负或解出 dbnum 0）"))?;
    let (pins, _) = current_mdb_sources()?;
    let pin = pins
        .iter()
        .find(|pin| pin.dbnum == dbnum as i32)
        .with_context(|| {
            format!("生成根 {root} 解出 dbnum {dbnum}，它不在当前 MDB 的成员清单里")
        })?;
    anyhow::ensure!(
        pin.db_type.eq_ignore_ascii_case("DESI") || pin.db_type.eq_ignore_ascii_case("ISOD"),
        "生成根 {root} 解出 dbnum {dbnum}，但那是 {} 库，不是 DESI / ISOD",
        pin.db_type
    );
    let mut engine = ReadOnlyEngine::open(&pin.file).with_context(|| {
        format!(
            "e3d-io 打开 {} {} ({})",
            pin.db_type,
            dbnum,
            pin.file.display()
        )
    })?;
    let found = engine
        .find_element(root)
        .with_context(|| format!("在 {} {} 里点查 {root}", pin.db_type, dbnum))?;
    anyhow::ensure!(
        found.is_some(),
        "生成根 {root} 解出 dbnum {dbnum}，但 {} 最新会话 {} 的索引里没有它",
        pin.file.display(),
        engine.session().sesno
    );
    Ok(dbnum)
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

    /// 非法 refno 在位解码那一步就停，报错说的是「不是合法 RefNo」——连 MDB 成员清单都不必读
    /// （本测试进程没有 DbOption，读了就会是另一种错）。
    #[test]
    fn an_invalid_root_is_rejected_before_the_mdb_is_consulted() {
        for root in [
            RefNo::new(0, 0),
            RefNo::new(24_384, 0x8000_0000),
            RefNo::new(3 << 13, 5),
        ] {
            let error = dbnum_of_root(root).unwrap_err().to_string();
            assert!(error.contains("不是合法 RefNo"), "{root}: {error}");
        }
    }

    /// 切片 §5.2 源码护栏：定库只靠位解码，不遍历 MDB 的全部 DESI，也没有按 word0 建的缓存。
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
            .split_once("pub fn dbnum_of_root(")
            .expect("dbnum_of_root exists")
            .1
            .split_once("\n}\n")
            .expect("function end")
            .0;
        assert!(
            body.contains("refno_dbnum_of(root)"),
            "定库必须走位解码 helper"
        );
        assert!(!body.contains("for pin in"), "不得遍历 MDB 成员逐库试查");
        assert_eq!(
            body.matches("ReadOnlyEngine::open(").count(),
            1,
            "只开解出的那一个库"
        );
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
