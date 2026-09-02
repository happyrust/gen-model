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
//! * **生成根归哪个库**——[`dbnum_of_root`]，在 MDB 的 DESI 文件里按会话索引点查；命中零个
//!   或多个都是错误，不猜（CONTEXT.md「Ref0 库归属」）。命中后按 ref0 缓存：同一 ref0 的
//!   全部元素都在同一个库里。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;
use dashmap::DashMap;
use e3d_io::ReadOnlyEngine;
use e3d_io::refno::RefNo;
use once_cell::sync::Lazy;

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
static DBNUM_BY_REF0: Lazy<DashMap<u32, u32>> = Lazy::new(DashMap::new);

/// 当前 MDB 里 `dbnum` 的库类型与文件。
pub fn source_file_of(dbnum: u32) -> anyhow::Result<(String, PathBuf)> {
    let (pins, _) = current_mdb_sources()?;
    pins.into_iter()
        .find(|pin| pin.dbnum == dbnum as i32)
        .map(|pin| (pin.db_type, pin.file))
        .with_context(|| format!("当前 MDB 没有 dbnum {dbnum} 的库文件"))
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

/// 生成根所在的 DESI 库。
///
/// 不查 `pe`（零解析下没有行），在 MDB 的每个 DESI 文件里按会话索引点查这个元素：
/// 恰好一个库命中才是答案；一个都没有说明它不在当前 MDB 里，多个命中是 Ref0 归属冲突。
pub fn dbnum_of_root(root: RefNo) -> anyhow::Result<u32> {
    if let Some(hit) = DBNUM_BY_REF0.get(&root.word0) {
        return Ok(*hit);
    }
    let (pins, _) = current_mdb_sources()?;
    let mut hits = Vec::new();
    let mut searched = 0usize;
    for pin in pins
        .iter()
        .filter(|pin| pin.db_type.eq_ignore_ascii_case("DESI"))
    {
        searched += 1;
        let mut engine = ReadOnlyEngine::open(&pin.file)
            .with_context(|| format!("e3d-io 打开 DESI {} ({})", pin.dbnum, pin.file.display()))?;
        let found = engine
            .find_element(root)
            .with_context(|| format!("在 DESI {} 里点查 {root}", pin.dbnum))?;
        if found.is_some() {
            hits.push(pin.dbnum as u32);
        }
    }
    match hits.as_slice() {
        [dbnum] => {
            DBNUM_BY_REF0.insert(root.word0, *dbnum);
            Ok(*dbnum)
        }
        [] => anyhow::bail!("生成根 {root} 不在当前 MDB 的任何 DESI 库里（已查 {searched} 个库）"),
        many => anyhow::bail!("生成根 {root} 同时出现在 DESI {many:?} 里：Ref0 归属冲突，不猜"),
    }
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
