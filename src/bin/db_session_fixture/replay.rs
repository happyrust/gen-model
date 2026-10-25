//! L2 回放（`docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md` §3.1 / §5.2）：
//! 把录好的会话链按 SAVEWORK 逐步「就地」喂给真监听器。
//!
//! dabacon 库文件 append-only + 改写文件头：会话 N+1 的文件 = 会话 N 的文件 + 追加页 + 新头。
//! `replay_init` 从终稿切出会话 S 写成工作文件；`replay_step` 把工作文件**在原地**推到会话 T：
//! 打开同一个文件写入 cut(T) 的字节再 `set_len`，不删不换——`StableFileId`（卷号 + 文件索引）不变，
//! ADR-089 的文件身份锚才不会把每一步都判成「换了文件」（计划 D3）。
//!
//! 两种半截（`Half`）给「保存中途被监听器看见」的用例（INC-H01）一个确定性触发：
//! - `Pages`：只追加 T 的页、头还是 S 的（E3D 先写页后改头的中间态）；
//! - `Header`：只改头、页还没到（头指向文件尾之外）。
//!
//! 回退（`rollback = true`）把工作文件截回更早的会话，给 INC-B06 / H04；不带这个标志往回走是错。
//!
//! 只读终稿、只写工作文件；不碰任何库、不起任何进程。

use anyhow::{Context, bail, ensure};
use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::archive_util::extract_single_declared_file;
use crate::format::SessionFixtureManifest;
use crate::session_cut::{PAGE_SIZE, session_chain, write_snapshot};

/// 半截：只写页，或只改头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    Pages,
    Header,
}

impl Half {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "pages" => Ok(Self::Pages),
            "header" => Ok(Self::Header),
            other => bail!("--half 只认 pages / header，给的是 {other:?}"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Pages => "pages",
            Self::Header => "header",
        }
    }
}

/// 一步回放的回执（JSON 到 stdout，harness 按机器可读口径解析）。
#[derive(Debug, serde::Serialize)]
pub struct ReplayReport {
    pub work: PathBuf,
    /// 工作文件这一步之前的头指针会话（init 没有）。
    pub from_sesno: Option<u32>,
    /// 要推到的会话。
    pub to_sesno: u32,
    /// 写完之后头指针读回的会话（`Half::Pages` 下仍是 `from_sesno`）。
    pub header_sesno_after: u32,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub half: Option<&'static str>,
    pub rollback: bool,
}

impl ReplayReport {
    pub fn print(&self) {
        println!(
            "{}",
            serde_json::to_string(self).expect("replay report serialises")
        );
    }
}

/// 终稿字节：`--fixture <目录>` 走 manifest + zip（大小 / SHA256 都核），`--source <文件>` 直接读。
pub fn load_final(fixture: Option<&Path>, source: Option<&Path>) -> anyhow::Result<Vec<u8>> {
    match (fixture, source) {
        (Some(_), Some(_)) => bail!("--fixture 与 --source 只能给一个"),
        (None, None) => bail!("--fixture 或 --source 至少要给一个"),
        (None, Some(path)) => {
            fs::read(path).with_context(|| format!("read source {}", path.display()))
        }
        (Some(root), None) => {
            let manifest_path = root.join("manifest.json");
            let manifest: SessionFixtureManifest = serde_json::from_slice(
                &fs::read(&manifest_path)
                    .with_context(|| format!("read {}", manifest_path.display()))?,
            )
            .with_context(|| format!("parse {}", manifest_path.display()))?;
            let temp = tempfile::Builder::new()
                .prefix("aios-session-replay-")
                .tempdir()?;
            let final_path = temp.path().join("final_db");
            extract_single_declared_file(
                &root.join(&manifest.archive.path),
                &manifest.final_snapshot.path,
                manifest.final_snapshot.bytes,
                &manifest.final_snapshot.sha256,
                &final_path,
            )?;
            fs::read(&final_path).context("read extracted final file")
        }
    }
}

/// 从终稿切出会话 `to` 写成工作文件（已存在则覆盖——init 就是重置）。
pub fn replay_init(final_bytes: &[u8], to: u32, out: &Path) -> anyhow::Result<ReplayReport> {
    let chain = session_chain(final_bytes)?;
    let cut = chain.cut_for(to)?;
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    // 第一次落地走 write_snapshot（新建文件）；已存在的工作文件也就地覆写，保持文件身份。
    let bytes_before = fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if out.exists() {
        let end = (cut.latest_page as usize + 1) * PAGE_SIZE;
        let mut snapshot = final_bytes[..end].to_vec();
        patch_header_pointer(&mut snapshot, cut.session_page);
        overwrite_in_place(out, &snapshot)?;
    } else {
        write_snapshot(final_bytes, cut, out)?;
    }
    let after = fs::read(out)?;
    let header_sesno_after = session_chain(&after)?.latest_sesno;
    ensure!(
        header_sesno_after == to,
        "init 后头指针 sesno={header_sesno_after}，应为 {to}"
    );
    Ok(ReplayReport {
        work: out.to_path_buf(),
        from_sesno: None,
        to_sesno: to,
        header_sesno_after,
        bytes_before,
        bytes_after: after.len() as u64,
        half: None,
        rollback: false,
    })
}

/// 把工作文件就地推到会话 `to`。
///
/// 前进步要求工作文件是 cut(to) 的前缀（除文件头页外逐字节相同）——不是就说明工作文件不是从这份终稿来的，
/// 或被别人改过，拒绝继续。往回走要显式 `rollback`。
pub fn replay_step(
    final_bytes: &[u8],
    work: &Path,
    to: u32,
    half: Option<Half>,
    rollback: bool,
) -> anyhow::Result<ReplayReport> {
    let chain = session_chain(final_bytes)?;
    let cut = chain.cut_for(to)?;
    let current = fs::read(work).with_context(|| format!("read work file {}", work.display()))?;
    let current_chain = session_chain(&current)?;
    let from = current_chain.latest_sesno;
    ensure!(from != to, "工作文件已经在 sesno={to}");
    if to < from {
        ensure!(
            rollback,
            "工作文件在 sesno={from}，要推到更早的 {to}：那是回退，要显式 --rollback"
        );
        ensure!(half.is_none(), "回退不支持半截（没有对应的真实形状）");
    } else {
        ensure!(!rollback, "--rollback 只用于往回走；{from} → {to} 是前进");
    }
    let end = (cut.latest_page as usize + 1) * PAGE_SIZE;
    ensure!(
        end <= final_bytes.len(),
        "cut(to={to}) 的截断点 {end} 超出终稿 {}",
        final_bytes.len()
    );
    let mut target = final_bytes[..end].to_vec();
    patch_header_pointer(&mut target, cut.session_page);

    if !rollback {
        // append-only 不变量：工作文件的页（头页以外）必须是目标的前缀。
        ensure!(
            current.len() <= target.len(),
            "工作文件 {} 字节比 cut(to={to}) {} 字节还长，不是前进",
            current.len(),
            target.len()
        );
        ensure!(
            current.len() >= PAGE_SIZE && current[PAGE_SIZE..] == target[PAGE_SIZE..current.len()],
            "工作文件的页不是 cut(to={to}) 的前缀：它不是从这份终稿来的，或被改过"
        );
    }

    let bytes_before = current.len() as u64;
    match half {
        None => overwrite_in_place(work, &target)?,
        Some(Half::Pages) => {
            // 页到位，头还是旧的。
            let mut staged = target.clone();
            staged[..PAGE_SIZE].copy_from_slice(&current[..PAGE_SIZE]);
            overwrite_in_place(work, &staged)?;
        }
        Some(Half::Header) => {
            // 头先改，页还没到：只写第一页，长度不动。
            let mut staged = current.clone();
            staged[..PAGE_SIZE].copy_from_slice(&target[..PAGE_SIZE]);
            overwrite_in_place(work, &staged)?;
        }
    }
    let after = fs::read(work)?;
    let header_sesno_after = header_sesno(&after);
    Ok(ReplayReport {
        work: work.to_path_buf(),
        from_sesno: Some(from),
        to_sesno: to,
        header_sesno_after,
        bytes_before,
        bytes_after: after.len() as u64,
        half: half.map(Half::label),
        rollback,
    })
}

fn patch_header_pointer(bytes: &mut [u8], session_page: u32) {
    use crate::session_cut::HEADER_SESSION_PAGE_OFFSET;
    bytes[HEADER_SESSION_PAGE_OFFSET..HEADER_SESSION_PAGE_OFFSET + 4]
        .copy_from_slice(&session_page.to_be_bytes());
}

/// 头指针指向的会话号；头指向文件尾之外（`Half::Header`）时读不到，回 0。
fn header_sesno(bytes: &[u8]) -> u32 {
    session_chain(bytes).map(|c| c.latest_sesno).unwrap_or(0)
}

/// 同一个文件就地覆写：不删、不换、不走临时文件 + rename。卷号 / 文件索引不变。
fn overwrite_in_place(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("open {} for in-place write", path.display()))?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(bytes)?;
    file.set_len(bytes.len() as u64)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_cut::HEADER_SESSION_PAGE_OFFSET;

    /// page0 头；page1 = sesno 7 的 session page（latest_page=1）；page2 数据；
    /// page3 = sesno 8（previous=1，latest_page=3）；page4 数据；page5 = sesno 9（previous=3，latest_page=5）。
    fn synthetic_three_sessions() -> Vec<u8> {
        let mut bytes = vec![0u8; PAGE_SIZE * 6];
        for (page, byte) in [(2usize, 0xA2u8), (4, 0xA4)] {
            bytes[page * PAGE_SIZE..(page + 1) * PAGE_SIZE].fill(byte);
        }
        let session = |bytes: &mut [u8], page: usize, previous: u32, sesno: u32, latest: u32| {
            let base = page * PAGE_SIZE;
            bytes[base + 4..base + 8].copy_from_slice(&previous.to_be_bytes());
            bytes[base + 12..base + 16].copy_from_slice(&sesno.to_be_bytes());
            bytes[base + 20..base + 24].copy_from_slice(&latest.to_be_bytes());
        };
        session(&mut bytes, 1, 0, 7, 1);
        session(&mut bytes, 3, 1, 8, 3);
        session(&mut bytes, 5, 3, 9, 5);
        bytes[HEADER_SESSION_PAGE_OFFSET..HEADER_SESSION_PAGE_OFFSET + 4]
            .copy_from_slice(&5u32.to_be_bytes());
        bytes
    }

    #[cfg(windows)]
    fn file_identity(path: &Path) -> (u32, u64) {
        use std::os::windows::fs::MetadataExt;
        let metadata = fs::metadata(path).unwrap();
        (
            metadata.volume_serial_number().unwrap(),
            metadata.file_index().unwrap(),
        )
    }

    #[test]
    fn init_then_forward_steps_reproduce_each_cut_without_changing_the_file_identity() {
        let final_bytes = synthetic_three_sessions();
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("ams8000_0001");

        let init = replay_init(&final_bytes, 7, &work).unwrap();
        assert_eq!(init.header_sesno_after, 7);
        assert_eq!(init.bytes_after as usize, PAGE_SIZE * 2);
        #[cfg(windows)]
        let identity = file_identity(&work);

        let step = replay_step(&final_bytes, &work, 8, None, false).unwrap();
        assert_eq!((step.from_sesno, step.to_sesno), (Some(7), 8));
        assert_eq!(step.header_sesno_after, 8);
        assert_eq!(step.bytes_after as usize, PAGE_SIZE * 4);
        let chain = session_chain(&final_bytes).unwrap();
        let expected = dir.path().join("expected-8");
        write_snapshot(&final_bytes, chain.cut_for(8).unwrap(), &expected).unwrap();
        assert_eq!(fs::read(&work).unwrap(), fs::read(&expected).unwrap());

        let step = replay_step(&final_bytes, &work, 9, None, false).unwrap();
        assert_eq!(step.header_sesno_after, 9);
        assert_eq!(fs::read(&work).unwrap(), final_bytes);
        #[cfg(windows)]
        assert_eq!(file_identity(&work), identity, "就地追加不得换文件");
    }

    #[test]
    fn half_steps_leave_the_file_in_the_two_intermediate_shapes_and_complete_afterwards() {
        let final_bytes = synthetic_three_sessions();
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("db");
        replay_init(&final_bytes, 7, &work).unwrap();

        // 页先到：长度是 8 的，头还说 7。
        let pages = replay_step(&final_bytes, &work, 8, Some(Half::Pages), false).unwrap();
        assert_eq!(pages.header_sesno_after, 7);
        assert_eq!(pages.bytes_after as usize, PAGE_SIZE * 4);
        assert_eq!(pages.half, Some("pages"));
        // 补齐：同一个 to 再来一次完整步。
        let done = replay_step(&final_bytes, &work, 8, None, false).unwrap();
        assert_eq!(done.header_sesno_after, 8);

        // 头先改：长度还是 8 的，头指向文件尾之外，链读不出来。
        let header = replay_step(&final_bytes, &work, 9, Some(Half::Header), false).unwrap();
        assert_eq!(header.bytes_after as usize, PAGE_SIZE * 4);
        assert_eq!(header.header_sesno_after, 0);
        assert!(session_chain(&fs::read(&work).unwrap()).is_err());
        // 补齐要把头先读回来：上一步的头已经是 9 的，所以走不了「前缀」检查——用 init 重置到 9 就地写。
        let reset = replay_init(&final_bytes, 9, &work).unwrap();
        assert_eq!(reset.header_sesno_after, 9);
        assert_eq!(fs::read(&work).unwrap(), final_bytes);
    }

    #[test]
    fn backwards_needs_the_rollback_flag_and_foreign_files_are_refused() {
        let final_bytes = synthetic_three_sessions();
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("db");
        replay_init(&final_bytes, 9, &work).unwrap();

        let refused = replay_step(&final_bytes, &work, 7, None, false).unwrap_err();
        assert!(refused.to_string().contains("--rollback"), "{refused}");
        let rolled = replay_step(&final_bytes, &work, 7, None, true).unwrap();
        assert_eq!(rolled.header_sesno_after, 7);
        assert_eq!(rolled.bytes_after as usize, PAGE_SIZE * 2);
        assert!(rolled.rollback);

        // 同一会话再推一次是错；不是这份终稿的工作文件也是错。
        assert!(replay_step(&final_bytes, &work, 7, None, false).is_err());
        let mut foreign = fs::read(&work).unwrap();
        foreign[PAGE_SIZE + 100] ^= 0xFF;
        fs::write(&work, &foreign).unwrap();
        let refused = replay_step(&final_bytes, &work, 8, None, false).unwrap_err();
        assert!(refused.to_string().contains("前缀"), "{refused}");
    }

    #[test]
    fn load_final_takes_exactly_one_source() {
        assert!(load_final(None, None).is_err());
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        fs::write(&a, b"x").unwrap();
        assert!(load_final(Some(dir.path()), Some(&a)).is_err());
        assert_eq!(load_final(None, Some(&a)).unwrap(), b"x");
    }
}
