//! 库文件头与会话页的读法——e3d-io 版（替代 old-pdms-io 的 `DbPageBasicInfo` / `PdmsIO`）。
//!
//! 出处：`docs/plans/2026-09-03-old-pdms-io-retirement-inventory.md` §E（T414）。
//!
//! 数据面对文件头只问四件事：这是哪个库（`dbnum` / 库类型）、最新会话号、那次保存的时刻、
//! 文件多大。old-pdms-io 用 `PdmsIO::new(..).get_page_basic_info()` 一次给全；这里用 e3d-io
//! 的 `DbDescriptor`（第 0 页）+ 最新 `SessionPage`（链尾，一页）给同样的四件事，不再拉
//! 整条会话链、也不再解析任何元素。
//!
//! 会话时刻：e3d-io 把 `+0x34..+0x48` 五个字原样交出（它不肯替编码背书，见
//! `e3d_io::session::SessionPage::stamp_words` 的注释）；old-pdms-io 的 `SessionPageData::get_dt`
//! 多年来一直按「年 / 月 / (日×24+时) / (分×60+秒)」解它，`dbnum_watermark.end_sesno_time`
//! 与批次回执里的时间都是这么来的。[`SessionStamp::local_time`] 沿用这条解法，保证换底座后
//! 写出去的时间逐字相同；解不出来（字段全零 / 越界）给 `None`，不 panic。

use std::path::Path;

use chrono::{DateTime, Local, TimeZone, Utc};
use e3d_io::session::{DbView, SessionPage, SessionSelector};
use e3d_io::{DbDescriptor, ReadOnlyEngine};
use serde::{Deserialize, Serialize};

/// 一页会话页里数据面关心的部分。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStamp {
    pub sesno: u32,
    /// 会话页所在页号。
    pub pgno: u32,
    /// 上一会话页的页号；链头（第一个会话）是 `None`。
    pub previous_pgno: Option<u32>,
    /// 该会话写的最后一页。
    pub end_pgno: u32,
    /// 该会话时刻的索引根页。
    pub index_root_pgno: u32,
    /// `+0x34..+0x48` 五个字，原样。
    pub stamp_words: [u32; 5],
    pub machine: String,
    pub comment: String,
}

impl SessionStamp {
    pub fn from_page(page: &SessionPage) -> Self {
        Self {
            sesno: page.sesno,
            pgno: page.page.page,
            previous_pgno: page.previous.map(|previous| previous.page),
            end_pgno: page.end.page,
            index_root_pgno: page.index_root.page,
            stamp_words: page.stamp_words,
            // e3d-io 把这一格改叫 `user`（core.dll 的会话属性 USER，按 +0x78 的长度字读）；
            // 本仓对外的 `SessionStamp.machine` 键名是既有 JSON 契约，不跟着改。
            machine: page.user.clone(),
            comment: page.comment.clone(),
        }
    }

    /// 与 old-pdms-io `SessionPageData::get_dt` 同一条解法：字 0 = 年，字 1 = 月，
    /// 字 2 = 日×24 + 时，字 3 = 分×60 + 秒；按本机时区解释后换成 UTC。
    pub fn local_time(&self) -> Option<DateTime<Utc>> {
        let [year, month, hours, seconds, _] = self.stamp_words;
        let days = hours / 24;
        let hours = hours % 24;
        let minutes = seconds / 60;
        let seconds = seconds % 60;
        Local
            .with_ymd_and_hms(
                i32::try_from(year).ok()?,
                month,
                days,
                hours,
                minutes,
                seconds,
            )
            .single()
            .map(Into::into)
    }

    /// RFC 3339 文本；解不出时刻给 `None`。
    pub fn rfc3339(&self) -> Option<String> {
        self.local_time().map(|time| time.to_rfc3339())
    }
}

/// 一个库文件的头信息：`DbPageBasicInfo` 的 e3d-io 替身。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DbFileHeader {
    /// 库号（第 0 页 `+0x08`）。
    pub dbnum: i32,
    /// 库类型名（`DESI` / `CATA` / …，第 0 页 `+0x20` 反哈希）；反不出来时是原始哈希的十进制文本。
    pub db_type: String,
    /// 最新会话页号（第 0 页 `+0x28`）。
    pub latest_ses_pgno: u32,
    /// 最新会话。
    pub latest_session: SessionStamp,
    /// 打开时的文件长度，增量用来判断文件有没有长。
    pub file_size: u64,
}

impl DbFileHeader {
    pub fn latest_sesno(&self) -> u32 {
        self.latest_session.sesno
    }

    /// 读一个库文件的头：第 0 页描述符 + 链尾会话页 + 文件长度。
    pub fn read(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let engine = ReadOnlyEngine::open(path)
            .map_err(|error| anyhow::anyhow!("打开库文件 {} 失败: {error}", path.display()))?;
        let file_size = std::fs::metadata(path)
            .map_err(|error| anyhow::anyhow!("读取 {} 元数据失败: {error}", path.display()))?
            .len();
        Ok(Self::from_parts(
            engine.descriptor(),
            engine.session(),
            file_size,
        ))
    }

    pub fn from_parts(descriptor: &DbDescriptor, latest: &SessionPage, file_size: u64) -> Self {
        Self {
            dbnum: descriptor.db_mark as i32,
            db_type: db_type_name(descriptor),
            latest_ses_pgno: descriptor.latest_ses_pgno,
            latest_session: SessionStamp::from_page(latest),
            file_size,
        }
    }
}

/// 库类型名：反哈希不出来时交原始哈希的十进制文本（事实，不编名字）。
pub fn db_type_name(descriptor: &DbDescriptor) -> String {
    descriptor
        .db_type_name()
        .map(|name| name.to_ascii_uppercase())
        .unwrap_or_else(|| descriptor.db_type_hash.to_string())
}

/// 头 60 字节能回答的三件事：库类型、最新会话页号、库号。
///
/// `old-parse-pdms-db::parse::parse_file_basic_info` 的替身（parse_pdms_db 退役清单 §B2）。
/// 字段名与旧结构逐字相同，调用方只换 `use`。三个字都在第 0 页描述符上：`db_no` = `+0x08`、
/// `db_type` = `+0x20` 反哈希、`ses_pgno` = `+0x28`——正是 e3d-io `DbDescriptor` 已经解出来
/// 的三个字，这里不再自己数偏移。
///
/// 与旧读法的一处差别：旧版对任何 60 字节都给一个结构（不是库文件时 `db_type` 为空串），
/// 这里交给 `DbDescriptor::parse` 裁决——第 0 页类型字不是 0 / 页大小非法的文件直接
/// `None`。三条调用路径（自动重扫、手动候选扫描、CATA 候选枚举）本来就把空 `db_type`
/// 当「跳过」，这里只是把跳过提前到读头那一步。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DbBasicInfo {
    /// 库类型名（`DESI` / `CATA` / …）；`+0x20` 反哈希不出来时为空串（与旧读法相同）。
    pub db_type: String,
    /// 最新会话页号（`+0x28`）。
    pub ses_pgno: u32,
    /// 库号（`+0x08`）。
    pub db_no: u32,
}

/// 解 [`DbBasicInfo`] 需要的文件头长度：`DbDescriptor` 到 `primary_extent_ref` 为止恰好 60 字节，
/// 与旧读法 `parse_file_basic_info` 的 60 字节缓冲相同。
pub const BASIC_INFO_HEADER_BYTES: usize = 60;

impl DbBasicInfo {
    pub fn from_descriptor(descriptor: &DbDescriptor) -> Self {
        Self {
            db_type: descriptor.db_type_name().unwrap_or_default(),
            ses_pgno: descriptor.latest_ses_pgno,
            db_no: descriptor.db_mark,
        }
    }

    /// 从文件开头的字节解（至少 [`BASIC_INFO_HEADER_BYTES`] 个）；不是描述符页给 `None`。
    pub fn from_header_bytes(header: &[u8]) -> Option<Self> {
        DbDescriptor::parse(header)
            .ok()
            .map(|descriptor| Self::from_descriptor(&descriptor))
    }

    /// 只读文件开头 60 字节（`parse_db_basic_info(path)` 的替身，但不 panic）：
    /// 打不开 / 不足 60 字节 / 不是描述符页都给 `None`。
    pub fn read(path: impl AsRef<Path>) -> Option<Self> {
        use std::io::Read;
        let mut file = std::fs::File::open(path).ok()?;
        let mut header = [0u8; BASIC_INFO_HEADER_BYTES];
        file.read_exact(&mut header).ok()?;
        Self::from_header_bytes(&header)
    }
}

/// 一个快照文件坐在头指针会话上时，哪些 refno 在活索引里。
///
/// `old-parse-pdms-db::paged::PagedDbSession::{open, snapshot().sesno, read_raw_records}` 三连的替身
/// （parse_pdms_db 退役清单 §G）：夹具构建器与集成测试的「验证闸」拿它核对切割快照自称的 sesno
/// 与逐元素的存在性。每个 refno 走一次 `ReadOnlyEngine::find_element`——查活索引并把记录头的
/// refno 核一遍，与旧读法「索引命中 + 读得出记录」同一口径。
#[derive(Debug, Clone, Default)]
pub struct SnapshotPresence {
    /// 文件头指针指向的会话号。
    pub sesno: u32,
    /// `refnos` 里在该会话活索引中命中的那些。
    pub present: std::collections::HashSet<aios_core::RefU64>,
}

impl SnapshotPresence {
    pub fn contains(&self, refno: &aios_core::RefU64) -> bool {
        self.present.contains(refno)
    }
}

pub fn snapshot_presence(
    path: impl AsRef<Path>,
    refnos: &[aios_core::RefU64],
) -> anyhow::Result<SnapshotPresence> {
    let path = path.as_ref();
    let mut engine = ReadOnlyEngine::open(path)
        .map_err(|error| anyhow::anyhow!("打开快照 {} 失败: {error}", path.display()))?;
    let sesno = engine.session().sesno;
    let mut present = std::collections::HashSet::with_capacity(refnos.len());
    for refno in refnos {
        let found = engine
            .find_element(e3d_io::RefNo::new(refno.get_0(), refno.get_1()))
            .map_err(|error| {
                anyhow::anyhow!(
                    "在 {}（sesno={sesno}）查 {refno} 失败: {error}",
                    path.display()
                )
            })?;
        if found.is_some() {
            present.insert(*refno);
        }
    }
    Ok(SnapshotPresence { sesno, present })
}

/// 只读最新会话号（`PdmsIO::get_latest_sesno` 的替身）：第 0 页 + 一页会话页。
pub fn latest_sesno(path: impl AsRef<Path>) -> anyhow::Result<u32> {
    let path = path.as_ref();
    let engine = ReadOnlyEngine::open(path)
        .map_err(|error| anyhow::anyhow!("打开库文件 {} 失败: {error}", path.display()))?;
    Ok(engine.session().sesno)
}

/// 指定会话的会话页（`PdmsIO::get_ses_data(sesno)` 的替身）。会话不在链上是 `Err`。
pub fn session_at(path: impl AsRef<Path>, sesno: u32) -> anyhow::Result<SessionStamp> {
    let path = path.as_ref();
    let view = DbView::open_at(path, SessionSelector::Exact(sesno)).map_err(|error| {
        anyhow::anyhow!("打开库文件 {} 的会话 {sesno} 失败: {error}", path.display())
    })?;
    Ok(SessionStamp::from_page(view.session()))
}

/// 整条会话链，**旧 → 新**（`PdmsIO::sesno_pgno_map` 的替身）。
pub fn session_chain(path: impl AsRef<Path>) -> anyhow::Result<Vec<SessionStamp>> {
    let path = path.as_ref();
    let mut view = DbView::open(path)
        .map_err(|error| anyhow::anyhow!("打开库文件 {} 失败: {error}", path.display()))?;
    let mut chain: Vec<SessionStamp> = view
        .walk_sessions()
        .map_err(|error| anyhow::anyhow!("遍历 {} 的会话链失败: {error}", path.display()))?
        .iter()
        .map(SessionStamp::from_page)
        .collect();
    chain.reverse();
    Ok(chain)
}

/// 窗口内实际存在的会话号（含空保存）：`DabaconSnapshot::session_sesnos_in_range` 的替身。
pub fn session_sesnos_in_range(
    chain: &[SessionStamp],
    range: &std::ops::RangeInclusive<i32>,
) -> Vec<u32> {
    chain
        .iter()
        .map(|stamp| stamp.sesno)
        .filter(|sesno| i32::try_from(*sesno).is_ok_and(|sesno| range.contains(&sesno)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与 old-pdms-io `SessionPageData::get_dt` 同解：2026 年 8 月、`hours = 3×24 + 14`、
    /// `seconds = 25×60 + 7` → 2026-08-03 14:25:07（本机时区）。
    #[test]
    fn the_stamp_decodes_like_the_legacy_session_page() {
        let stamp = SessionStamp {
            stamp_words: [2026, 8, 3 * 24 + 14, 25 * 60 + 7, 0],
            ..SessionStamp::default()
        };
        let expected: DateTime<Utc> = Local
            .with_ymd_and_hms(2026, 8, 3, 14, 25, 7)
            .single()
            .unwrap()
            .into();
        assert_eq!(stamp.local_time(), Some(expected));
        assert_eq!(stamp.rfc3339(), Some(expected.to_rfc3339()));
    }

    /// 字段全零（没写过时间的会话页）不许 panic，给 `None`。
    #[test]
    fn an_unstamped_session_has_no_time() {
        let stamp = SessionStamp::default();
        assert_eq!(stamp.local_time(), None);
        assert_eq!(stamp.rfc3339(), None);
    }

    /// 按 `ams8000_0001` 第 0 页的实测布局拼一个 60 字节头：`db_no` / 库类型 / 最新会话页
    /// 三个字要与旧读法 `parse_file_basic_info` 落在同样的偏移上。
    fn synthetic_header(db_type_hash: u32) -> [u8; BASIC_INFO_HEADER_BYTES] {
        let mut header = [0u8; BASIC_INFO_HEADER_BYTES];
        let mut put = |offset: usize, word: u32| {
            header[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
        };
        put(0x00, 0); // 描述符页类型
        put(0x04, 2); // noun
        put(0x08, 8000); // db_num
        put(0x20, db_type_hash);
        put(0x28, 8897); // latest_ses_pgno
        put(0x2C, 1); // ext_no
        put(0x30, 4); // first_ses_pgno
        put(0x34, 512); // page size in words
        put(0x38, 16192); // primary_extent_ref
        header
    }

    #[test]
    fn basic_info_reads_the_same_three_words_as_the_legacy_header_parser() {
        let header = synthetic_header(e3d_attlib::db1_hash("DESI"));
        assert_eq!(
            DbBasicInfo::from_header_bytes(&header),
            Some(DbBasicInfo {
                db_type: "DESI".to_string(),
                ses_pgno: 8897,
                db_no: 8000,
            })
        );
    }

    /// 类型字反哈希不出来时 `db_type` 为空串（旧读法同此），不是把哈希编成名字。
    #[test]
    fn basic_info_leaves_an_undecodable_type_word_empty() {
        let header = synthetic_header(0);
        let info = DbBasicInfo::from_header_bytes(&header).expect("descriptor page");
        assert_eq!(info.db_type, "");
        assert_eq!(info.db_no, 8000);
    }

    /// 第 0 页类型字不是 0 的文件不是 dabacon 库，给 `None` 而不是一个空壳结构。
    #[test]
    fn basic_info_rejects_a_non_descriptor_page() {
        let mut header = synthetic_header(e3d_attlib::db1_hash("DESI"));
        header[0..4].copy_from_slice(&3u32.to_be_bytes());
        assert_eq!(DbBasicInfo::from_header_bytes(&header), None);
        assert_eq!(DbBasicInfo::from_header_bytes(&header[..32]), None);
    }

    #[test]
    fn sesnos_in_range_keep_chain_order_and_drop_outsiders() {
        let chain: Vec<SessionStamp> = [3u32, 4, 5, 7]
            .into_iter()
            .map(|sesno| SessionStamp {
                sesno,
                ..SessionStamp::default()
            })
            .collect();
        assert_eq!(session_sesnos_in_range(&chain, &(4..=7)), vec![4, 5, 7]);
        assert_eq!(session_sesnos_in_range(&chain, &(8..=9)), Vec::<u32>::new());
    }
}
