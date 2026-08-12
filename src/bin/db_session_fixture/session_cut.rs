//! 从 append-only 的 PDMS DB 文件里按 sesno 切出历史快照。
//!
//! 机制：文件头偏移 40 处存最新 session page 号；每个 session page 记录
//! `previous`（上一个 session page）、`sesno`、`latest_page`（该会话结束时文件的
//! 最后一页）。沿链回溯即可枚举全部会话；把文件截断到 `latest_page + 1` 页并把
//! 头指针改回该会话的 session page，就得到该会话时刻的完整文件。
//!
//! 由 Issue #19 专用实现（`src/bin/db8000_two_delete_fixture.rs`）泛化而来；
//! 那份保持冻结作为回归，本模块是后续夹具的通用入口
//! （docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md 阶段一）。

use anyhow::{Context, ensure};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

pub const PAGE_SIZE: usize = 0x800;
pub const HEADER_SESSION_PAGE_OFFSET: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionCut {
    pub session_page: u32,
    pub latest_page: u32,
}

/// 一个 DB 文件里能回溯到的全部会话。
#[derive(Debug, Clone)]
pub struct SessionChain {
    /// 头指针指向的（最新）会话号。
    pub latest_sesno: u32,
    pub cuts: BTreeMap<u32, SessionCut>,
}

impl SessionChain {
    pub fn cut_for(&self, sesno: u32) -> anyhow::Result<SessionCut> {
        self.cuts
            .get(&sesno)
            .copied()
            .with_context(|| format!("会话链里没有 sesno={sesno}"))
    }

    pub fn contains(&self, sesno: u32) -> bool {
        self.cuts.contains_key(&sesno)
    }
}

fn be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("four bytes"))
}

/// 沿头指针回溯 session page 链，枚举全部会话。
pub fn session_chain(bytes: &[u8]) -> anyhow::Result<SessionChain> {
    ensure!(
        bytes.len() >= PAGE_SIZE,
        "PDMS 文件不足一页：{}",
        bytes.len()
    );
    ensure!(
        bytes.len().is_multiple_of(PAGE_SIZE),
        "PDMS 文件大小未按页对齐：{}",
        bytes.len()
    );
    let mut page = be_u32(&bytes[HEADER_SESSION_PAGE_OFFSET..HEADER_SESSION_PAGE_OFFSET + 4]);
    let mut cuts = BTreeMap::new();
    let mut latest_sesno = None;
    let mut seen = HashSet::new();
    while page != 0 && page != u32::MAX && seen.insert(page) {
        let start = page as usize * PAGE_SIZE;
        ensure!(
            start + PAGE_SIZE <= bytes.len(),
            "session page {page} 超出文件范围"
        );
        let data = &bytes[start..start + PAGE_SIZE];
        let previous = be_u32(&data[4..8]);
        let sesno = be_u32(&data[12..16]);
        let cut = SessionCut {
            session_page: page,
            latest_page: be_u32(&data[20..24]),
        };
        ensure!(
            cuts.insert(sesno, cut).is_none(),
            "会话链里出现重复 sesno={sesno}（page {page}）"
        );
        if latest_sesno.is_none() {
            latest_sesno = Some(sesno);
        }
        page = previous;
    }
    let latest_sesno = latest_sesno.context("头指针没有指向任何 session page")?;
    Ok(SessionChain { latest_sesno, cuts })
}

/// 把 `source` 截断为 `cut` 对应会话时刻的快照并写盘（头指针一并回写）。
pub fn write_snapshot(source: &[u8], cut: SessionCut, path: &Path) -> anyhow::Result<()> {
    let end = (cut.latest_page as usize + 1) * PAGE_SIZE;
    ensure!(
        end <= source.len(),
        "快照截断点 {end} 超出源文件大小 {}",
        source.len()
    );
    let mut snapshot = source[..end].to_vec();
    snapshot[HEADER_SESSION_PAGE_OFFSET..HEADER_SESSION_PAGE_OFFSET + 4]
        .copy_from_slice(&cut.session_page.to_be_bytes());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, snapshot)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个合成文件：page0 头，page1 = sesno 7 的 session page（latest_page=1），
    /// page2 数据页，page3 = sesno 8 的 session page（previous=1，latest_page=3）。
    fn synthetic_two_sessions() -> Vec<u8> {
        let mut bytes = vec![0u8; PAGE_SIZE * 4];
        bytes[HEADER_SESSION_PAGE_OFFSET..HEADER_SESSION_PAGE_OFFSET + 4]
            .copy_from_slice(&3u32.to_be_bytes());
        let page1 = PAGE_SIZE;
        bytes[page1 + 4..page1 + 8].copy_from_slice(&0u32.to_be_bytes());
        bytes[page1 + 12..page1 + 16].copy_from_slice(&7u32.to_be_bytes());
        bytes[page1 + 20..page1 + 24].copy_from_slice(&1u32.to_be_bytes());
        let page3 = PAGE_SIZE * 3;
        bytes[page3 + 4..page3 + 8].copy_from_slice(&1u32.to_be_bytes());
        bytes[page3 + 12..page3 + 16].copy_from_slice(&8u32.to_be_bytes());
        bytes[page3 + 20..page3 + 24].copy_from_slice(&3u32.to_be_bytes());
        bytes
    }

    #[test]
    fn walks_the_chain_and_reports_latest() {
        let bytes = synthetic_two_sessions();
        let chain = session_chain(&bytes).unwrap();
        assert_eq!(chain.latest_sesno, 8);
        assert_eq!(chain.cuts.len(), 2);
        assert_eq!(
            chain.cut_for(7).unwrap(),
            SessionCut {
                session_page: 1,
                latest_page: 1
            }
        );
        assert_eq!(
            chain.cut_for(8).unwrap(),
            SessionCut {
                session_page: 3,
                latest_page: 3
            }
        );
        assert!(chain.cut_for(9).is_err());
    }

    #[test]
    fn snapshot_truncates_and_rewrites_the_header_pointer() {
        let bytes = synthetic_two_sessions();
        let chain = session_chain(&bytes).unwrap();
        let dir = tempfile::tempdir().unwrap();

        let older = dir.path().join("sesno7");
        write_snapshot(&bytes, chain.cut_for(7).unwrap(), &older).unwrap();
        let older_bytes = fs::read(&older).unwrap();
        assert_eq!(older_bytes.len(), PAGE_SIZE * 2);
        assert_eq!(
            be_u32(&older_bytes[HEADER_SESSION_PAGE_OFFSET..HEADER_SESSION_PAGE_OFFSET + 4]),
            1
        );
        let rechain = session_chain(&older_bytes).unwrap();
        assert_eq!(rechain.latest_sesno, 7);
        assert_eq!(rechain.cuts.len(), 1);

        let newest = dir.path().join("sesno8");
        write_snapshot(&bytes, chain.cut_for(8).unwrap(), &newest).unwrap();
        assert_eq!(fs::read(&newest).unwrap(), bytes);
    }

    #[test]
    fn rejects_unaligned_and_out_of_range_files() {
        assert!(session_chain(&[0u8; 10]).is_err());
        assert!(session_chain(&vec![0u8; PAGE_SIZE + 1]).is_err());

        let mut pointer_out_of_range = vec![0u8; PAGE_SIZE];
        pointer_out_of_range[HEADER_SESSION_PAGE_OFFSET..HEADER_SESSION_PAGE_OFFSET + 4]
            .copy_from_slice(&5u32.to_be_bytes());
        assert!(session_chain(&pointer_out_of_range).is_err());
    }

    #[test]
    fn self_referencing_chain_terminates() {
        let mut bytes = vec![0u8; PAGE_SIZE * 2];
        bytes[HEADER_SESSION_PAGE_OFFSET..HEADER_SESSION_PAGE_OFFSET + 4]
            .copy_from_slice(&1u32.to_be_bytes());
        let page1 = PAGE_SIZE;
        // previous 指向自己：seen 去重保证终止，只收录一次。
        bytes[page1 + 4..page1 + 8].copy_from_slice(&1u32.to_be_bytes());
        bytes[page1 + 12..page1 + 16].copy_from_slice(&3u32.to_be_bytes());
        bytes[page1 + 20..page1 + 24].copy_from_slice(&1u32.to_be_bytes());
        let chain = session_chain(&bytes).unwrap();
        assert_eq!(chain.cuts.len(), 1);
        assert_eq!(chain.latest_sesno, 3);
    }
}
