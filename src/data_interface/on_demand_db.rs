//! Unified read substrate for request-scoped PDMS database access.
//!
//! The page-backed path is the production default. `legacy` keeps the previous
//! whole-file index parser available for immediate rollback, while `compare`
//! evaluates both readers and rejects the first normalized difference.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use aios_core::RefU64;
use anyhow::Context;
use parse_pdms_db::paged::{PageReadStats, PagedDbSession};
use parse_pdms_db::parse::{DbIndexData, EleData};

const READ_MODE_ENV: &str = "AIOS_PDMS_ON_DEMAND_READ_MODE";
const INVALID_REF0_SENTINEL: u32 = 0x8000_0001;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadMode {
    Legacy,
    Compare,
    Paged,
}

impl ReadMode {
    fn configured() -> Self {
        let value = std::env::var(READ_MODE_ENV)
            .unwrap_or_else(|_| "paged".to_string())
            .trim()
            .to_ascii_lowercase();
        match Self::parse(&value) {
            Some(mode) => mode,
            None => {
                log::warn!(
                    "[paged_db] invalid_mode env={} value={value:?} fallback=paged",
                    READ_MODE_ENV
                );
                Self::Paged
            }
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "legacy" => Some(Self::Legacy),
            "compare" => Some(Self::Compare),
            "paged" => Some(Self::Paged),
            _ => None,
        }
    }
}

enum SessionSource {
    Legacy(DbIndexData),
    Paged(PagedDbSession),
    Compare {
        legacy: DbIndexData,
        paged: PagedDbSession,
    },
}

pub(crate) struct OnDemandDbSession {
    path: PathBuf,
    dbnum: u32,
    source: SessionSource,
    parsed_records: usize,
}

impl OnDemandDbSession {
    pub(crate) fn open(path: &Path) -> anyhow::Result<Self> {
        let configured = ReadMode::configured();
        let (mode, fallback_reason) = if let Some(extent) = first_extra_extent(path) {
            (
                ReadMode::Legacy,
                Some(format!("multi_extent:{}", extent.display())),
            )
        } else {
            (configured, None)
        };
        if let Some(reason) = fallback_reason {
            log::warn!(
                "[paged_db] route=legacy path={} configured={configured:?} reason={reason}",
                path.display()
            );
        }

        let source = match mode {
            ReadMode::Legacy => SessionSource::Legacy(open_legacy(path)?),
            ReadMode::Paged => SessionSource::Paged(PagedDbSession::open(path)?),
            ReadMode::Compare => SessionSource::Compare {
                legacy: open_legacy(path)?,
                paged: PagedDbSession::open(path)?,
            },
        };
        Ok(Self {
            path: path.to_path_buf(),
            dbnum: read_dbnum(path).unwrap_or_default(),
            source,
            parsed_records: 0,
        })
    }

    pub(crate) fn is_compare(&self) -> bool {
        matches!(self.source, SessionSource::Compare { .. })
    }

    pub(crate) fn legacy_world_refno(&self) -> Option<RefU64> {
        match &self.source {
            SessionSource::Legacy(index) | SessionSource::Compare { legacy: index, .. } => {
                Some(index.world_refno)
            }
            SessionSource::Paged(_) => None,
        }
    }

    pub(crate) async fn parse_element(&mut self, refno: RefU64) -> anyhow::Result<Option<EleData>> {
        let db_info = aios_core::get_default_pdms_db_info();
        let result = match &mut self.source {
            SessionSource::Legacy(index) => parse_legacy_element(index, refno, &db_info).await,
            SessionSource::Paged(paged) => Ok(paged
                .parse_elements_with_info(&[refno], &db_info)
                .await?
                .remove(&refno)),
            SessionSource::Compare { legacy, paged } => {
                let legacy_ele = parse_legacy_element(legacy, refno, &db_info).await?;
                let paged_ele = paged
                    .parse_elements_with_info(&[refno], &db_info)
                    .await?
                    .remove(&refno);
                compare_elements(
                    &self.path,
                    self.dbnum,
                    refno,
                    legacy_ele.as_ref(),
                    paged_ele.as_ref(),
                )?;
                Ok(paged_ele)
            }
        }?;
        self.parsed_records += usize::from(result.is_some());
        Ok(result)
    }
}

impl Drop for OnDemandDbSession {
    fn drop(&mut self) {
        let (snapshot, stats) = match &self.source {
            SessionSource::Legacy(_) => return,
            SessionSource::Paged(paged) | SessionSource::Compare { paged, .. } => {
                (paged.snapshot(), paged.stats())
            }
        };
        log_page_summary(
            &self.path,
            snapshot.sesno,
            snapshot.page_size,
            stats,
            self.parsed_records,
        );
    }
}

pub(crate) fn scan_ref0s(path: &Path, project: &str) -> anyhow::Result<Vec<u32>> {
    let configured = ReadMode::configured();
    let (mode, fallback_reason) = if let Some(extent) = first_extra_extent(path) {
        (
            ReadMode::Legacy,
            Some(format!("multi_extent:{}", extent.display())),
        )
    } else {
        (configured, None)
    };
    if let Some(reason) = fallback_reason {
        log::warn!(
            "[paged_db] locator_route=legacy path={} configured={configured:?} reason={reason}",
            path.display()
        );
    }

    match mode {
        ReadMode::Legacy => scan_ref0s_legacy(path, project),
        ReadMode::Paged => scan_ref0s_paged(path),
        ReadMode::Compare => {
            let dbnum = read_dbnum(path).unwrap_or_default();
            let mut legacy = scan_ref0s_legacy(path, project)?;
            let mut paged = scan_ref0s_paged(path)?;
            legacy.sort_unstable();
            legacy.dedup();
            paged.sort_unstable();
            paged.dedup();
            anyhow::ensure!(
                legacy == paged,
                "paged compare mismatch path={} dbnum={} field=ref0_set legacy_count={} paged_count={} first_difference={:?}",
                path.display(),
                dbnum,
                legacy.len(),
                paged.len(),
                first_difference(&legacy, &paged)
            );
            Ok(paged)
        }
    }
}

fn scan_ref0s_paged(path: &Path) -> anyhow::Result<Vec<u32>> {
    let mut session = PagedDbSession::open(path)?;
    let values = session.scan_ref0s()?;
    let snapshot = session.snapshot();
    log_page_summary(path, snapshot.sesno, snapshot.page_size, session.stats(), 0);
    Ok(values)
}

fn scan_ref0s_legacy(path: &Path, project: &str) -> anyhow::Result<Vec<u32>> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let data =
        parse_pdms_db::parse::parse_file_db_basic_data(&path.to_path_buf(), file_name, project)?;
    let mut values = HashSet::new();
    for entry in data.refno_table_map.iter() {
        let ref0 = entry.key().get_0();
        if ref0 != 0 && ref0 != INVALID_REF0_SENTINEL {
            values.insert(ref0);
        }
    }
    for (owner_refno, children) in &data.children_map {
        let owner = owner_refno.get_0();
        if owner != 0 && owner != INVALID_REF0_SENTINEL {
            values.insert(owner);
        }
        for child in children {
            let ref0 = child.get_0();
            if ref0 != 0 && ref0 != INVALID_REF0_SENTINEL {
                values.insert(ref0);
            }
        }
    }
    Ok(values.into_iter().collect())
}

fn open_legacy(path: &Path) -> anyhow::Result<DbIndexData> {
    parse_pdms_db::parse::parse_file_db_index_data(&path.to_path_buf())
        .with_context(|| format!("open legacy on-demand index {}", path.display()))
}

async fn parse_legacy_element(
    index: &DbIndexData,
    refno: RefU64,
    db_info: &aios_core::PdmsDatabaseInfo,
) -> anyhow::Result<Option<EleData>> {
    let pos = match index.refno_table_map.get(&refno) {
        Some(entry) => entry.pos,
        None => return Ok(None),
    };
    anyhow::ensure!(
        pos >= 4 && pos <= index.bytes.len(),
        "element {} index position {pos} out of bounds ({} bytes)",
        refno.to_pe_key(),
        index.bytes.len()
    );
    parse_pdms_db::parse::parse_ele_data_with_info(&index.bytes[pos - 4..], db_info)
        .await
        .map(Some)
}

fn compare_elements(
    path: &Path,
    dbnum: u32,
    refno: RefU64,
    legacy: Option<&EleData>,
    paged: Option<&EleData>,
) -> anyhow::Result<()> {
    match (legacy, paged) {
        (None, None) => return Ok(()),
        (Some(_), None) | (None, Some(_)) => anyhow::bail!(
            "paged compare mismatch path={} dbnum={} refno={} field=presence legacy={} paged={}",
            path.display(),
            dbnum,
            refno.to_pe_key(),
            legacy.is_some(),
            paged.is_some()
        ),
        (Some(legacy), Some(paged)) => {
            ensure_field(path, dbnum, refno, "refno", legacy.refno, paged.refno)?;
            ensure_field(path, dbnum, refno, "noun", legacy.noun, paged.noun)?;
            ensure_field(path, dbnum, refno, "owner", legacy.owner, paged.owner)?;
            ensure_field(
                path,
                dbnum,
                refno,
                "children",
                &legacy.children.0,
                &paged.children.0,
            )?;
            let legacy_att = serde_json::to_value(legacy.whole_attmap.merge())?;
            let paged_att = serde_json::to_value(paged.whole_attmap.merge())?;
            ensure_field(
                path,
                dbnum,
                refno,
                "merged_attributes",
                legacy_att,
                paged_att,
            )?;
        }
    }
    Ok(())
}

fn ensure_field<T: std::fmt::Debug + PartialEq>(
    path: &Path,
    dbnum: u32,
    refno: RefU64,
    field: &str,
    legacy: T,
    paged: T,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        legacy == paged,
        "paged compare mismatch path={} dbnum={} refno={} field={} legacy={legacy:?} paged={paged:?}",
        path.display(),
        dbnum,
        refno.to_pe_key(),
        field
    );
    Ok(())
}

fn first_difference(left: &[u32], right: &[u32]) -> Option<(Option<u32>, Option<u32>)> {
    let len = left.len().max(right.len());
    (0..len)
        .find(|&index| left.get(index) != right.get(index))
        .map(|index| (left.get(index).copied(), right.get(index).copied()))
}

fn first_extra_extent(path: &Path) -> Option<PathBuf> {
    let file_name = path.file_name()?.to_str()?;
    let (prefix, suffix) = file_name.rsplit_once('_')?;
    if suffix.len() != 4 || !suffix.bytes().all(|value| value.is_ascii_digit()) {
        return None;
    }
    let parent = path.parent()?;
    let expected_prefix = format!("{prefix}_");
    std::fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|candidate| {
            let Some(name) = candidate.file_name().and_then(|value| value.to_str()) else {
                return false;
            };
            let Some(candidate_suffix) = name.strip_prefix(&expected_prefix) else {
                return false;
            };
            candidate_suffix.len() == 4
                && candidate_suffix.bytes().all(|value| value.is_ascii_digit())
                && candidate_suffix
                    .parse::<u16>()
                    .is_ok_and(|extent| extent >= 2)
        })
}

fn read_dbnum(path: &Path) -> Option<u32> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 60];
    file.read_exact(&mut header).ok()?;
    let info = parse_pdms_db::parse::parse_file_basic_info(&header);
    (info.db_no != 0).then_some(info.db_no)
}

fn log_page_summary(
    path: &Path,
    sesno: u32,
    page_size: usize,
    stats: PageReadStats,
    parsed_records: usize,
) {
    println!(
        "[paged_db] path={} snapshot_sesno={} page_size={} physical_pages={} bytes_read={} cache_hits={} cache_misses={} prefetched_pages={} index_pages={} record_pages={} parsed_records={}",
        path.display(),
        sesno,
        page_size,
        stats.physical_pages_read,
        stats.bytes_read,
        stats.cache_hits,
        stats.cache_misses,
        stats.prefetched_pages,
        stats.index_pages_read,
        stats.record_pages_read,
        parsed_records
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_later_extent() {
        let root =
            std::env::temp_dir().join(format!("aios-on-demand-extents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("acp7320_0001");
        let second = root.join("acp7320_0002");
        std::fs::write(&first, []).unwrap();
        assert!(first_extra_extent(&first).is_none());
        std::fs::write(&second, []).unwrap();
        assert_eq!(first_extra_extent(&first), Some(second));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_supported_modes_only() {
        assert_eq!(ReadMode::parse("legacy"), Some(ReadMode::Legacy));
        assert_eq!(ReadMode::parse("compare"), Some(ReadMode::Compare));
        assert_eq!(ReadMode::parse("paged"), Some(ReadMode::Paged));
        assert_eq!(ReadMode::parse("unexpected"), None);
    }

    #[test]
    #[ignore = "requires the production ACP 7320 fixture; run with compare mode for parity"]
    fn production_cata_locator_is_identical_and_below_io_budget() {
        let path = Path::new(r"D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7320_0001");
        if !path.exists() {
            return;
        }

        let file_len = std::fs::metadata(path).unwrap().len();
        let mut paged = PagedDbSession::open(path).unwrap();
        let expected = paged.scan_ref0s().unwrap();
        let stats = paged.stats();
        eprintln!(
            "paged locator: ref0s={} bytes_read={} file_len={} physical_pages={} index_pages={} record_pages={}",
            expected.len(),
            stats.bytes_read,
            file_len,
            stats.physical_pages_read,
            stats.index_pages_read,
            stats.record_pages_read
        );
        assert_eq!(stats.record_pages_read, 0);
        assert!(stats.bytes_read <= file_len * 15 / 100);

        let mut routed = scan_ref0s(path, "ACP").unwrap();
        let mut expected = expected;
        routed.sort_unstable();
        expected.sort_unstable();
        assert_eq!(routed, expected);
    }
}
