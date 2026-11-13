//! L2 回放（`docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md` §3.1 / §5.2）：
//! 把录好的会话链按 SAVEWORK 逐步「就地」喂给真监听器。
//!
//! dabacon 库文件 append-only + 改写文件头（外加对前一会话页的几个字的就地改写，见 `format` 模块注）：
//! 会话 N+1 的文件 = 会话 N 的文件 + 追加页 + 新头 + 前一会话页的几个字节。
//! `replay_init` 从终稿切出会话 S（再打上 S 的字节补丁 = 真机上 S 当时的字节）写成工作文件；`replay_step` 把工作文件
//! **在原地**推到会话 T：打开同一个文件写入 faithful(T) 的字节再 `set_len`，不删不换——`StableFileId`（卷号 + 文件索引）不变，
//! ADR-089 的文件身份锚才不会把每一步都判成「换了文件」（计划 D3）。
//!
//! 两种半截（`Half`）给「保存中途被监听器看见」的用例（INC-H01）一个确定性触发：
//! - `Pages`：写 T 的页（含前一会话页的改写）、头还是 S 的（E3D 先写页后改头的中间态；这一半已经让 S 的 anchor 变了）；
//! - `Header`：只改头、页还没到（头指向文件尾之外）。
//!
//! 半截之后的补齐步（同一个 T 再来一次，或补另一半）都认：页已到位就只改头，头已改就只补页。
//!
//! 回退（`rollback = true`）把工作文件截回更早的会话，给 INC-B06 / H04；不带这个标志往回走是错。
//!
//! 只读终稿、只写工作文件；不碰任何库、不起任何进程。

use anyhow::{Context, bail, ensure};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::archive_util::extract_single_declared_file;
use crate::format::{BytePatch, SessionFixtureManifest, apply_patches, trim_cut_to};
use crate::session_cut::{PAGE_SIZE, SessionChain, session_chain};

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

/// 就地覆写已有文件时对硬链接的态度。
///
/// 影子工程根里凡是没列进 `-Replayed` 的库都是到真工程原件的硬链接（`scripts/replay/New-ShadowProjectRoot.ps1`）；
/// 就地覆写一个有多个链接的文件，就是把真工程的原件改掉。缺省拒绝；明确知道自己要写穿才 `Allow`（`--allow-hardlink`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardLinks {
    Refuse,
    Allow,
}

impl HardLinks {
    pub fn from_flag(allow: bool) -> Self {
        if allow { Self::Allow } else { Self::Refuse }
    }
}

/// 台账里一个有真快照的 sesno：真文件的长度 + 字节补丁。
#[derive(Debug, Clone)]
pub struct Faithful {
    pub bytes: u64,
    pub patches: Vec<BytePatch>,
}

/// 终稿 + 按 sesno 的忠实快照台账（`--source` 没有）。
#[derive(Debug, Clone)]
pub struct Final {
    pub bytes: Vec<u8>,
    pub snapshots: BTreeMap<u32, Faithful>,
}

impl Final {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            snapshots: BTreeMap::new(),
        }
    }

    fn chain(&self) -> anyhow::Result<SessionChain> {
        session_chain(&self.bytes)
    }

    /// 忠实快照的长度：有真快照就是它的，没有就是 cut 的。
    fn faithful_len(&self, chain: &SessionChain, sesno: u32) -> Option<usize> {
        let cut = chain.cuts.get(&sesno)?;
        Some(match self.snapshots.get(&sesno) {
            Some(snapshot) => snapshot.bytes as usize,
            None => (cut.latest_page as usize + 1) * PAGE_SIZE,
        })
    }

    /// 真机上会话 `sesno` 刚落完时的文件：cut(sesno)（头指针改回）→ 截到真长度 → 打字节补丁。回（字节，补丁字节数）。
    fn faithful(&self, chain: &SessionChain, sesno: u32) -> anyhow::Result<(Vec<u8>, usize)> {
        let cut = chain.cut_for(sesno)?;
        let end = (cut.latest_page as usize + 1) * PAGE_SIZE;
        ensure!(
            end <= self.bytes.len(),
            "cut(sesno={sesno}) 的截断点 {end} 超出终稿 {}",
            self.bytes.len()
        );
        let mut bytes = self.bytes[..end].to_vec();
        patch_header_pointer(&mut bytes, cut.session_page);
        let patched = match self.snapshots.get(&sesno) {
            Some(snapshot) => {
                let trimmed = trim_cut_to(&bytes, snapshot.bytes as usize, PAGE_SIZE)
                    .with_context(|| format!("sesno={sesno} 台账长度对齐"))?
                    .to_vec();
                bytes = trimmed;
                apply_patches(&mut bytes, &snapshot.patches, PAGE_SIZE)
                    .with_context(|| format!("sesno={sesno} 打字节补丁"))?
            }
            None => 0,
        };
        Ok((bytes, patched))
    }
}

/// 一步回放的回执（JSON 到 stdout，harness 按机器可读口径解析）。
#[derive(Debug, serde::Serialize)]
pub struct ReplayReport {
    pub work: PathBuf,
    /// 工作文件这一步之前的头指针会话（init 没有；头先改的中间态读不出来也没有）。
    pub from_sesno: Option<u32>,
    /// 要推到的会话。
    pub to_sesno: u32,
    /// 写完之后头指针读回的会话（`Half::Pages` 下仍是 `from_sesno`；`Half::Header` 下读不出来，回 0）。
    pub header_sesno_after: u32,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub half: Option<&'static str>,
    pub rollback: bool,
    /// 这一步的目标快照上打了多少字节的补丁（与终稿现切不同的字节数）。
    pub patched_bytes: usize,
}

impl ReplayReport {
    pub fn print(&self) {
        println!(
            "{}",
            serde_json::to_string(self).expect("replay report serialises")
        );
    }
}

/// 终稿：`--fixture <目录>` 走 manifest + zip（大小 / SHA256 都核，连台账里的字节补丁一起），`--source <文件>` 直接读（没有补丁）。
pub fn load_final(fixture: Option<&Path>, source: Option<&Path>) -> anyhow::Result<Final> {
    match (fixture, source) {
        (Some(_), Some(_)) => bail!("--fixture 与 --source 只能给一个"),
        (None, None) => bail!("--fixture 或 --source 至少要给一个"),
        (None, Some(path)) => {
            Ok(Final::new(fs::read(path).with_context(|| {
                format!("read source {}", path.display())
            })?))
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
            let bytes = fs::read(&final_path).context("read extracted final file")?;
            let snapshots = manifest
                .session_snapshots
                .into_iter()
                .map(|spec| {
                    (
                        spec.sesno,
                        Faithful {
                            bytes: spec.bytes,
                            patches: spec.patches,
                        },
                    )
                })
                .collect();
            Ok(Final { bytes, snapshots })
        }
    }
}

/// 从终稿切出会话 `to`（+ 补丁）写成工作文件（已存在则就地覆写——init 就是重置，文件身份不变；
/// 已存在的文件若还有别的硬链接，按 `hard_links` 拒绝或放行）。
pub fn replay_init(
    final_: &Final,
    to: u32,
    out: &Path,
    hard_links: HardLinks,
) -> anyhow::Result<ReplayReport> {
    let chain = final_.chain()?;
    let (snapshot, patched_bytes) = final_.faithful(&chain, to)?;
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let bytes_before = fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if out.exists() {
        overwrite_in_place(out, &snapshot, hard_links)?;
    } else {
        fs::write(out, &snapshot).with_context(|| format!("write {}", out.display()))?;
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
        patched_bytes,
    })
}

/// 把工作文件就地推到会话 `to`。
///
/// 前进步要求工作文件就是某个会话的 faithful 快照（按长度认出是哪个会话，除文件头页外逐字节相同），
/// 或者是 `to` 的「页已到位、头还没改」中间态——不是就说明工作文件不是从这份终稿来的，或被别人改过，拒绝继续。
/// 往回走要显式 `rollback`。
pub fn replay_step(
    final_: &Final,
    work: &Path,
    to: u32,
    half: Option<Half>,
    rollback: bool,
    hard_links: HardLinks,
) -> anyhow::Result<ReplayReport> {
    let chain = final_.chain()?;
    let (target, patched_bytes) = final_.faithful(&chain, to)?;
    let current = fs::read(work).with_context(|| format!("read work file {}", work.display()))?;
    ensure!(
        current.len() >= PAGE_SIZE && current.len().is_multiple_of(PAGE_SIZE),
        "工作文件 {} 字节，不是整页",
        current.len()
    );
    // 头先改的中间态头指向文件尾之外，链读不出来；那时候页属于哪个会话按长度认。
    let header_from = session_chain(&current).ok().map(|c| c.latest_sesno);
    let pages_from = chain
        .cuts
        .keys()
        .copied()
        .find(|&sesno| final_.faithful_len(&chain, sesno) == Some(current.len()));
    let from = header_from.or(pages_from);

    let pages_match = |expected: &[u8]| {
        current.len() == expected.len() && current[PAGE_SIZE..] == expected[PAGE_SIZE..]
    };
    let pages_already_at_target = pages_match(&target);
    let header_already_at_target = current[..PAGE_SIZE] == target[..PAGE_SIZE];

    if header_from == Some(to) {
        // 头已经是 to 的：只有「页还没到」的补齐才有事可做。
        ensure!(!pages_already_at_target, "工作文件已经在 sesno={to}");
    }
    let forward = match from {
        Some(from) if from > to => {
            ensure!(
                rollback,
                "工作文件在 sesno={from}，要推到更早的 {to}：那是回退，要显式 --rollback"
            );
            ensure!(half.is_none(), "回退不支持半截（没有对应的真实形状）");
            false
        }
        _ => {
            ensure!(!rollback, "--rollback 只用于往回走；{from:?} → {to} 是前进");
            true
        }
    };

    if forward && !pages_already_at_target {
        // append-only 不变量：工作文件的页（头页以外）必须正好是某个更早会话的 faithful 页。
        let Some(pages_sesno) = pages_from else {
            bail!(
                "工作文件 {} 字节不对应终稿里任何会话的长度：它不是从这份终稿来的，或被改过",
                current.len()
            );
        };
        ensure!(
            pages_sesno < to,
            "工作文件的页已经是 sesno={pages_sesno} 的，推到 {to} 不是前进"
        );
        let (expected, _) = final_.faithful(&chain, pages_sesno)?;
        ensure!(
            pages_match(&expected),
            "工作文件的页不是会话 {pages_sesno} 的快照（不是 cut(to={to}) 的前缀）：它不是从这份终稿来的，或被改过"
        );
    }

    let bytes_before = current.len() as u64;
    match half {
        None => overwrite_in_place(work, &target, hard_links)?,
        Some(Half::Pages) => {
            // 页到位，头还是旧的（头已经先改过的话就是补齐，留着那个头）。
            let mut staged = target.clone();
            staged[..PAGE_SIZE].copy_from_slice(&current[..PAGE_SIZE]);
            overwrite_in_place(work, &staged, hard_links)?;
        }
        Some(Half::Header) => {
            // 头先改，页还没到：只写第一页，长度不动（页已到位的话就是补齐，等于完整步）。
            ensure!(
                !header_already_at_target || !pages_already_at_target,
                "工作文件已经在 sesno={to}"
            );
            let mut staged = current.clone();
            staged[..PAGE_SIZE].copy_from_slice(&target[..PAGE_SIZE]);
            overwrite_in_place(work, &staged, hard_links)?;
        }
    }
    let after = fs::read(work)?;
    let header_sesno_after = header_sesno(&after);
    Ok(ReplayReport {
        work: work.to_path_buf(),
        from_sesno: from,
        to_sesno: to,
        header_sesno_after,
        bytes_before,
        bytes_after: after.len() as u64,
        half: half.map(Half::label),
        rollback,
        patched_bytes,
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

/// 文件的硬链接数：Windows 读 `BY_HANDLE_FILE_INFORMATION.nNumberOfLinks`（`windows_by_handle`，与 lib 的 `FrozenSource` 同一个特性门），
/// 其它平台读 `st_nlink`。
fn hard_link_count(path: &Path) -> anyhow::Result<u64> {
    let metadata = fs::metadata(path).with_context(|| format!("metadata {}", path.display()))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(u64::from(metadata.number_of_links().unwrap_or(1)))
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(metadata.nlink())
    }
}

/// 就地覆写前的护栏：文件还有别的硬链接就拒绝（除非 `HardLinks::Allow`）。
/// 影子工程根里漏列进 `-Replayed` 的库正是这种文件，写下去就是改真工程的原件。
fn ensure_overwritable(path: &Path, hard_links: HardLinks) -> anyhow::Result<()> {
    if hard_links == HardLinks::Allow {
        return Ok(());
    }
    let links = hard_link_count(path)?;
    ensure!(
        links <= 1,
        "{} 有 {links} 个硬链接：就地覆写会写穿到其它路径（多半是真工程的原件）。先删掉这一个链接再写；确认就是要写穿才加 --allow-hardlink",
        path.display()
    );
    Ok(())
}

/// 同一个文件就地覆写：不删、不换、不走临时文件 + rename。卷号 / 文件索引不变。
fn overwrite_in_place(path: &Path, bytes: &[u8], hard_links: HardLinks) -> anyhow::Result<()> {
    ensure_overwritable(path, hard_links)?;
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
    use crate::session_cut::{HEADER_SESSION_PAGE_OFFSET, write_snapshot};

    /// page0 头；page1 = sesno 7 的 session page（latest_page=1）；page2 数据；
    /// page3 = sesno 8（previous=1，latest_page=3）；page4 数据；page5 = sesno 9（previous=3，latest_page=6）；page6 数据
    /// （真 dabacon 里 session page 在它的 end 页之前，所以「真快照比 cut 短一页」少的是尾部数据页，不是 session page）。
    fn synthetic_three_sessions() -> Vec<u8> {
        let mut bytes = vec![0u8; PAGE_SIZE * 7];
        for (page, byte) in [(2usize, 0xA2u8), (4, 0xA4), (6, 0xA6)] {
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
        session(&mut bytes, 5, 3, 9, 6);
        bytes[HEADER_SESSION_PAGE_OFFSET..HEADER_SESSION_PAGE_OFFSET + 4]
            .copy_from_slice(&5u32.to_be_bytes());
        bytes
    }

    fn plain(bytes: Vec<u8>) -> Final {
        Final::new(bytes)
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
        let final_ = plain(final_bytes.clone());
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("ams8000_0001");

        let init = replay_init(&final_, 7, &work, HardLinks::Refuse).unwrap();
        assert_eq!(init.header_sesno_after, 7);
        assert_eq!(init.bytes_after as usize, PAGE_SIZE * 2);
        assert_eq!(init.patched_bytes, 0);
        #[cfg(windows)]
        let identity = file_identity(&work);

        let step = replay_step(&final_, &work, 8, None, false, HardLinks::Refuse).unwrap();
        assert_eq!((step.from_sesno, step.to_sesno), (Some(7), 8));
        assert_eq!(step.header_sesno_after, 8);
        assert_eq!(step.bytes_after as usize, PAGE_SIZE * 4);
        let chain = session_chain(&final_bytes).unwrap();
        let expected = dir.path().join("expected-8");
        write_snapshot(&final_bytes, chain.cut_for(8).unwrap(), &expected).unwrap();
        assert_eq!(fs::read(&work).unwrap(), fs::read(&expected).unwrap());

        let step = replay_step(&final_, &work, 9, None, false, HardLinks::Refuse).unwrap();
        assert_eq!(step.header_sesno_after, 9);
        assert_eq!(fs::read(&work).unwrap(), final_bytes);
        #[cfg(windows)]
        assert_eq!(file_identity(&work), identity, "就地追加不得换文件");
    }

    #[test]
    fn half_steps_leave_the_file_in_the_two_intermediate_shapes_and_complete_afterwards() {
        let final_bytes = synthetic_three_sessions();
        let final_ = plain(final_bytes.clone());
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("db");
        replay_init(&final_, 7, &work, HardLinks::Refuse).unwrap();

        // 页先到：长度是 8 的，头还说 7。
        let pages = replay_step(
            &final_,
            &work,
            8,
            Some(Half::Pages),
            false,
            HardLinks::Refuse,
        )
        .unwrap();
        assert_eq!(pages.header_sesno_after, 7);
        assert_eq!(pages.bytes_after as usize, PAGE_SIZE * 4);
        assert_eq!(pages.half, Some("pages"));
        // 补齐：同一个 to 补另一半（只改头）。
        let done = replay_step(
            &final_,
            &work,
            8,
            Some(Half::Header),
            false,
            HardLinks::Refuse,
        )
        .unwrap();
        assert_eq!(done.header_sesno_after, 8);
        let chain = session_chain(&final_bytes).unwrap();
        let expected = dir.path().join("expected-8");
        write_snapshot(&final_bytes, chain.cut_for(8).unwrap(), &expected).unwrap();
        assert_eq!(fs::read(&work).unwrap(), fs::read(&expected).unwrap());
        // 已经在 8 了，再来是错。
        assert!(replay_step(&final_, &work, 8, None, false, HardLinks::Refuse).is_err());

        // 头先改：长度还是 8 的，头指向文件尾之外，链读不出来。
        let header = replay_step(
            &final_,
            &work,
            9,
            Some(Half::Header),
            false,
            HardLinks::Refuse,
        )
        .unwrap();
        assert_eq!(header.bytes_after as usize, PAGE_SIZE * 4);
        assert_eq!(header.header_sesno_after, 0);
        assert!(session_chain(&fs::read(&work).unwrap()).is_err());
        // 补齐：只补页（头已经是 9 的，页按长度认出是 8 的）。
        let done = replay_step(
            &final_,
            &work,
            9,
            Some(Half::Pages),
            false,
            HardLinks::Refuse,
        )
        .unwrap();
        assert_eq!(done.from_sesno, Some(8));
        assert_eq!(done.header_sesno_after, 9);
        assert_eq!(fs::read(&work).unwrap(), final_bytes);
    }

    #[test]
    fn backwards_needs_the_rollback_flag_and_foreign_files_are_refused() {
        let final_ = plain(synthetic_three_sessions());
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("db");
        replay_init(&final_, 9, &work, HardLinks::Refuse).unwrap();

        let refused = replay_step(&final_, &work, 7, None, false, HardLinks::Refuse).unwrap_err();
        assert!(refused.to_string().contains("--rollback"), "{refused}");
        let rolled = replay_step(&final_, &work, 7, None, true, HardLinks::Refuse).unwrap();
        assert_eq!(rolled.header_sesno_after, 7);
        assert_eq!(rolled.bytes_after as usize, PAGE_SIZE * 2);
        assert!(rolled.rollback);

        // 同一会话再推一次是错；不是这份终稿的工作文件也是错。
        assert!(replay_step(&final_, &work, 7, None, false, HardLinks::Refuse).is_err());
        let mut foreign = fs::read(&work).unwrap();
        foreign[PAGE_SIZE + 100] ^= 0xFF;
        fs::write(&work, &foreign).unwrap();
        let refused = replay_step(&final_, &work, 8, None, false, HardLinks::Refuse).unwrap_err();
        assert!(refused.to_string().contains("前缀"), "{refused}");
    }

    /// 字节补丁：会话 8 刚落完时，它自己的 session page 有一个字还没被 9 改过（真机形状：`end` / `claim`）。
    /// faithful(8) 必须是补丁后的样子；推到 9 时那个字回到终稿的值（页半截就已经改了）；从补丁后的 8 前进也认得出来。
    #[test]
    fn byte_patches_make_the_snapshot_faithful_and_the_pages_half_undoes_them() {
        let final_bytes = synthetic_three_sessions();
        let mut final_ = plain(final_bytes.clone());
        // page3 是 sesno 8 的 session page；终稿里 +0x17 = 0x03（latest_page 低字节），假设真机上当时是 0x02。
        final_.snapshots.insert(
            8,
            Faithful {
                bytes: (PAGE_SIZE * 4) as u64,
                patches: vec![BytePatch {
                    page: 3,
                    offset: 0x17,
                    final_hex: "03".into(),
                    real_hex: "02".into(),
                }],
            },
        );
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("db");

        let init = replay_init(&final_, 8, &work, HardLinks::Refuse).unwrap();
        assert_eq!(init.patched_bytes, 1);
        let on_disk = fs::read(&work).unwrap();
        assert_eq!(on_disk[3 * PAGE_SIZE + 0x17], 0x02);
        // 补丁改的是 latest_page，所以头指针还读得出 8。
        assert_eq!(session_chain(&on_disk).unwrap().latest_sesno, 8);

        // 页半截到 9：8 的页回到终稿样子（anchor 已变），头还是 8。
        let pages = replay_step(
            &final_,
            &work,
            9,
            Some(Half::Pages),
            false,
            HardLinks::Refuse,
        )
        .unwrap();
        assert_eq!(pages.from_sesno, Some(8));
        assert_eq!(pages.header_sesno_after, 8);
        let on_disk = fs::read(&work).unwrap();
        assert_eq!(on_disk[3 * PAGE_SIZE + 0x17], 0x03);
        let done = replay_step(
            &final_,
            &work,
            9,
            Some(Half::Header),
            false,
            HardLinks::Refuse,
        )
        .unwrap();
        assert_eq!(done.header_sesno_after, 9);
        assert_eq!(fs::read(&work).unwrap(), final_bytes);

        // 真快照比 cut 短一页（E3D 把前一会话的 end 往后指了一页）：faithful(9) 就是 5 页，前缀按忠实长度认。
        final_.snapshots.insert(
            9,
            Faithful {
                bytes: (PAGE_SIZE * 6) as u64,
                patches: Vec::new(),
            },
        );
        replay_init(&final_, 8, &work, HardLinks::Refuse).unwrap();
        let short = replay_step(&final_, &work, 9, None, false, HardLinks::Refuse).unwrap();
        assert_eq!(short.bytes_after as usize, PAGE_SIZE * 6);
        assert_eq!(short.header_sesno_after, 9);
        assert_eq!(
            &fs::read(&work).unwrap()[PAGE_SIZE..],
            &final_bytes[PAGE_SIZE..PAGE_SIZE * 6]
        );
        assert!(
            replay_step(&final_, &work, 9, None, false, HardLinks::Refuse).is_err(),
            "已经在 9"
        );
        final_.snapshots.remove(&9);

        // 补丁的 final_hex 与终稿不符就拒绝（终稿换了、补丁还是老的）。
        final_.snapshots.get_mut(&8).unwrap().patches[0].final_hex = "ff".into();
        let refused = replay_init(&final_, 8, &work, HardLinks::Refuse).unwrap_err();
        assert!(format!("{refused:#}").contains("final_hex"), "{refused:#}");
    }

    #[test]
    fn load_final_takes_exactly_one_source() {
        assert!(load_final(None, None).is_err());
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        fs::write(&a, b"x").unwrap();
        assert!(load_final(Some(dir.path()), Some(&a)).is_err());
        let loaded = load_final(None, Some(&a)).unwrap();
        assert_eq!(loaded.bytes, b"x");
        assert!(loaded.snapshots.is_empty());
    }

    /// 影子工程根里漏列进 `-Replayed` 的库是到真工程原件的硬链接：init / step 都不得就地覆写它，原件一个字节不动；
    /// 删掉链接后写出的独立文件照常；明确 `Allow` 才写穿。
    #[test]
    fn overwriting_a_hard_linked_file_is_refused_unless_allowed() {
        let final_ = plain(synthetic_three_sessions());
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original");
        fs::write(&original, b"the real project file").unwrap();
        let link = dir.path().join("ams8000_0001");
        fs::hard_link(&original, &link).unwrap();

        let refused = replay_init(&final_, 7, &link, HardLinks::Refuse).unwrap_err();
        assert!(refused.to_string().contains("硬链接"), "{refused}");
        assert_eq!(
            fs::read(&original).unwrap(),
            b"the real project file",
            "原件一个字节都不能动"
        );

        // 链接删掉（原件还在），init 写出独立文件，再 step 照常。
        fs::remove_file(&link).unwrap();
        replay_init(&final_, 7, &link, HardLinks::Refuse).unwrap();
        replay_step(&final_, &link, 8, None, false, HardLinks::Refuse).unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"the real project file");

        // 工作文件被别人挂了第二个链接：step 也拒绝；明确放行才写，且两条路看到同一份字节。
        let second = dir.path().join("second-link");
        fs::hard_link(&link, &second).unwrap();
        let refused = replay_step(&final_, &link, 9, None, false, HardLinks::Refuse).unwrap_err();
        assert!(refused.to_string().contains("硬链接"), "{refused}");
        assert_eq!(
            session_chain(&fs::read(&link).unwrap())
                .unwrap()
                .latest_sesno,
            8
        );
        replay_step(&final_, &link, 9, None, false, HardLinks::Allow).unwrap();
        assert_eq!(fs::read(&second).unwrap(), fs::read(&link).unwrap());
        assert_eq!(
            session_chain(&fs::read(&second).unwrap())
                .unwrap()
                .latest_sesno,
            9
        );
    }
}
