//! Unified read substrate for request-scoped PDMS database access.
//!
//! Every production request is pinned to one validated page-backed snapshot.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aios_core::RefU64;
use parse_pdms_db::paged::{PageReadStats, PagedDbSession};
use parse_pdms_db::parse::EleData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

fn file_identity(path: &Path) -> anyhow::Result<FileIdentity> {
    let metadata = std::fs::metadata(path)?;
    Ok(FileIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn open_pinned_paged(path: &Path) -> anyhow::Result<PagedDbSession> {
    let before = file_identity(path)?;
    let paged = PagedDbSession::open(path)?;
    let after = file_identity(path)?;
    anyhow::ensure!(
        before == after,
        "paged source changed while opening path={} before={before:?} after={after:?}",
        path.display()
    );
    Ok(paged)
}

pub(crate) struct OnDemandDbSession {
    path: PathBuf,
    paged: PagedDbSession,
    parsed_records: usize,
    parent: Option<Box<OnDemandDbSession>>,
}

impl OnDemandDbSession {
    pub(crate) fn open(path: &Path) -> anyhow::Result<Self> {
        let mut session = Self::open_single(path)?;
        if let Some(parent) = crate::data_interface::extract_family::parent_path_of(path)
            .filter(|parent| parent.is_file() && parent != path)
        {
            session.parent = Some(Box::new(Self::open_single(&parent)?));
        }
        Ok(session)
    }

    fn open_single(path: &Path) -> anyhow::Result<Self> {
        let paged = open_pinned_paged(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            paged,
            parsed_records: 0,
            parent: None,
        })
    }

    pub(crate) async fn parse_element(&mut self, refno: RefU64) -> anyhow::Result<Option<EleData>> {
        if let Some(found) = self.parse_element_here(refno).await? {
            return Ok(Some(found));
        }
        if let Some(parent) = self.parent.as_mut() {
            return parent.parse_element_here(refno).await;
        }
        Ok(None)
    }

    async fn parse_element_here(&mut self, refno: RefU64) -> anyhow::Result<Option<EleData>> {
        let db_info = aios_core::get_default_pdms_db_info();
        let result = self
            .paged
            .parse_elements_with_info(&[refno], &db_info)
            .await?
            .remove(&refno);
        self.parsed_records += usize::from(result.is_some());
        Ok(result)
    }
}

impl Drop for OnDemandDbSession {
    fn drop(&mut self) {
        let snapshot = self.paged.snapshot();
        let stats = self.paged.stats();
        log_page_summary(
            &self.path,
            snapshot.sesno,
            snapshot.page_size_bytes,
            stats,
            self.parsed_records,
        );
    }
}

pub(crate) fn scan_ref0s(path: &Path, _project: &str) -> anyhow::Result<Vec<u32>> {
    let mut values = scan_ref0s_paged(path)?;
    if let Some(parent) = crate::data_interface::extract_family::parent_path_of(path)
        .filter(|parent| parent.is_file() && parent != path)
    {
        values.extend(scan_ref0s_paged(&parent)?);
        values.sort_unstable();
        values.dedup();
    }
    Ok(values)
}

fn scan_ref0s_paged(path: &Path) -> anyhow::Result<Vec<u32>> {
    let mut session = open_pinned_paged(path)?;
    let values = session.scan_ref0s()?;
    let snapshot = session.snapshot();
    log_page_summary(
        path,
        snapshot.sesno,
        snapshot.page_size_bytes,
        session.stats(),
        0,
    );
    Ok(values)
}

fn log_page_summary(
    path: &Path,
    sesno: u32,
    page_size_bytes: usize,
    stats: PageReadStats,
    parsed_records: usize,
) {
    println!(
        "[paged_db] path={} snapshot_sesno={} page_size_bytes={} physical_pages={} bytes_read={} cache_hits={} cache_misses={} prefetched_pages={} index_pages={} record_pages={} parsed_records={}",
        path.display(),
        sesno,
        page_size_bytes,
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
    fn file_identity_detects_length_or_timestamp_change() {
        let now = std::time::SystemTime::now();
        let baseline = FileIdentity {
            len: 2048,
            modified: Some(now),
        };
        assert_eq!(baseline, baseline);
        assert_ne!(
            baseline,
            FileIdentity {
                len: 4096,
                ..baseline
            }
        );
        assert_ne!(
            baseline,
            FileIdentity {
                modified: Some(now + std::time::Duration::from_secs(1)),
                ..baseline
            }
        );
    }

    #[test]
    fn paged_snapshot_verification_is_fail_closed() {
        let source = include_str!("on_demand_db.rs");
        let open = source
            .split_once("fn open_single(path: &Path)")
            .expect("open_single")
            .1
            .split_once("pub(crate) async fn parse_element")
            .expect("open boundary")
            .0;
        assert!(open.contains("open_pinned_paged(path)"));
        assert!(!open.contains("DabaconSnapshot"));
        assert!(!open.contains("verification unavailable"));
        let removed_mode_env = ["AIOS", "PDMS", "ON", "DEMAND", "READ", "MODE"].join("_");
        let removed_route = ["route=", "legacy"].concat();
        let removed_scan = ["scan_ref0s_", "legacy"].concat();
        assert!(!source.contains(&removed_mode_env));
        assert!(!source.contains(&removed_route));
        assert!(!source.contains(&removed_scan));
    }

    #[test]
    #[ignore = "requires the production ACP 7320 fixture"]
    fn production_cata_locator_uses_paged_snapshot_below_io_budget() {
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

    #[test]
    #[ignore = "requires the production ACP 7000 fixture"]
    fn production_acp7000_locator_opens_authoritative_paged_session() {
        let path = Path::new(r"D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7000_0001");
        if !path.exists() {
            return;
        }

        let paged = PagedDbSession::open(path).unwrap();
        assert_eq!(paged.snapshot().page_size_bytes, 2048);
        assert_eq!(paged.snapshot().sesno, 272);

        let ref0s = scan_ref0s(path, "ACP").unwrap();
        assert!(!ref0s.is_empty());
    }
}
