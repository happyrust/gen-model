//! 冻结源：一次收集冻住的「哪个文件、哪一代、哪个会话」，持久化前必须还是它。
//!
//! old-pdms-io 的 `snapshot::SnapshotToken` / `DabaconSnapshot::open_verified*` 的 e3d-io 替身
//! （`docs/plans/2026-09-03-old-pdms-io-retirement-inventory.md` §B，T407「要保留的那一半」）。
//!
//! 它挡的是「收集与持久化之间文件被原子替换 / 回拨」：同号会话在另一个文件里是另一件事，
//! 写进去的就是另一个工程的数据。纯数据窗口（零模型候选）也必须验，所以 `apply_one`
//! 无条件调 [`FrozenSource::reopen_verified`]。
//!
//! 身份三件事，与旧 token 一致：
//! 1. **稳定文件身份**（Windows `volume serial + file index` / Unix `dev + inode`）——路径没变、
//!    文件被换掉时它会变；同文件追加时不变。
//! 2. **dbnum / 库类型**（第 0 页）——同一路径、同一 inode 上不会变，变了就是另一个库。
//! 3. **会话与长度只许前进**——回拨到更早的会话或更短的文件是回卷，拒绝。
//!
//! 字段私有：调用方拼不出一个 token，只能从一次真实的打开里拿到。

use std::fs::{File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};

use e3d_io::ReadOnlyEngine;
use e3d_io::page::PageId;
use e3d_io::session::{DbView, SessionSelector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::db_file_header::{DbFileHeader, SessionStamp, session_chain};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

/// 捕获时文件持续变化的重试次数（与旧 `SNAPSHOT_CAPTURE_ATTEMPTS` 同值）。
const CAPTURE_ATTEMPTS: usize = 4;
/// 用来判定「同一世代」的文件头样本长度（与旧 `SNAPSHOT_HEADER_PREFIX_LEN` 同值）。
const HEADER_PREFIX_LEN: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum StableFileId {
    #[cfg(windows)]
    Windows { volume_serial: u32, file_index: u64 },
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct PageAddressV1 {
    extent: u16,
    page: u32,
}

impl From<PageId> for PageAddressV1 {
    fn from(value: PageId) -> Self {
        Self {
            extent: value.ext,
            page: value.page,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TargetAnchorV1 {
    session_page: PageAddressV1,
    index_root: PageAddressV1,
    session_page_sha256: String,
    index_root_sha256: String,
}

/// Durable representation stored in `IncrementUpdateAttempt.commit_token`.
/// Versioning keeps old rows fail-closed when this contract changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCommitTokenV1 {
    version: u8,
    canonical_path: PathBuf,
    file_id: StableFileId,
    dbnum: i32,
    db_type: String,
    target_sesno: u32,
    latest_sesno: u32,
    opened_len: u64,
    header_prefix_hex: String,
    anchor: TargetAnchorV1,
}

impl StableFileId {
    fn from_metadata(metadata: &Metadata) -> anyhow::Result<Self> {
        #[cfg(windows)]
        {
            let volume_serial = metadata
                .volume_serial_number()
                .ok_or_else(|| anyhow::anyhow!("文件元数据缺少 volume serial"))?;
            let file_index = metadata
                .file_index()
                .ok_or_else(|| anyhow::anyhow!("文件元数据缺少 file index"))?;
            return Ok(Self::Windows {
                volume_serial,
                file_index,
            });
        }
        #[cfg(unix)]
        {
            return Ok(Self::Unix {
                device: metadata.dev(),
                inode: metadata.ino(),
            });
        }
        #[allow(unreachable_code)]
        Err(anyhow::anyhow!("当前平台没有 dabacon 稳定文件身份实现"))
    }
}

/// 一次收集冻住的文件世代与目标会话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenSource {
    path: PathBuf,
    file_id: StableFileId,
    dbnum: i32,
    db_type: String,
    /// 冻住的目标会话（窗口终点）。
    target_sesno: u32,
    /// 打开那一刻文件自报的最新会话。
    latest_sesno: u32,
    latest_ses_pgno: u32,
    opened_len: u64,
    header_prefix: Vec<u8>,
    anchor: TargetAnchorV1,
}

impl FrozenSource {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dbnum(&self) -> i32 {
        self.dbnum
    }

    pub fn db_type(&self) -> &str {
        &self.db_type
    }

    /// 冻住的目标会话。
    pub fn target_sesno(&self) -> u32 {
        self.target_sesno
    }

    /// 打开那一刻文件自报的最新会话（≥ [`Self::target_sesno`]）。
    pub fn latest_sesno(&self) -> u32 {
        self.latest_sesno
    }

    pub fn opened_len(&self) -> u64 {
        self.opened_len
    }

    pub fn latest_ses_pgno(&self) -> u32 {
        self.latest_ses_pgno
    }

    /// 只给单测拼一个**不指向真实文件**的冻结源：身份字段全零，所有复核类方法对它
    /// 一律失败。用来构造 `CollectedWindow` / `NetWindowOutcome` 这类载体做纯函数断言。
    #[cfg(test)]
    pub(crate) fn synthetic(
        path: impl Into<PathBuf>,
        dbnum: i32,
        db_type: &str,
        target_sesno: u32,
    ) -> Self {
        Self {
            path: path.into(),
            #[cfg(windows)]
            file_id: StableFileId::Windows {
                volume_serial: 0,
                file_index: 0,
            },
            #[cfg(unix)]
            file_id: StableFileId::Unix {
                device: 0,
                inode: 0,
            },
            dbnum,
            db_type: db_type.to_string(),
            target_sesno,
            latest_sesno: target_sesno,
            latest_ses_pgno: 0,
            opened_len: 0,
            header_prefix: Vec::new(),
            anchor: TargetAnchorV1 {
                session_page: PageAddressV1 { extent: 0, page: 0 },
                index_root: PageAddressV1 { extent: 0, page: 0 },
                session_page_sha256: String::new(),
                index_root_sha256: String::new(),
            },
        }
    }

    /// 打开文件、冻住当前世代与它自报的最新会话。
    ///
    /// 头 / 长度在捕获前后不一致（有人正在写）就重试，重试耗尽才报错——与旧
    /// `DabaconSnapshot::open` 同一条纪律。
    pub fn capture(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)
            .map_err(|error| anyhow::anyhow!("打开冻结源 {} 失败: {error}", path.display()))?;
        for _attempt in 0..CAPTURE_ATTEMPTS {
            let before = file.metadata()?;
            let file_id = StableFileId::from_metadata(&before)?;
            let header_prefix = match read_prefix(&mut file) {
                Ok(prefix) => prefix,
                Err(error) => {
                    if file.metadata()?.len() != before.len() {
                        continue;
                    }
                    return Err(error.context("读取冻结 dabacon 文件头失败"));
                }
            };
            let header = DbFileHeader::read(&path);
            let after = file.metadata()?;
            let after_prefix = match read_prefix(&mut file) {
                Ok(prefix) => prefix,
                Err(_) if after.len() != before.len() => continue,
                Err(error) => return Err(error.context("复核冻结 dabacon 文件头失败")),
            };
            if before.len() != after.len() || header_prefix != after_prefix {
                continue;
            }
            let header = header?;
            let anchor = target_anchor(&path, header.latest_session.sesno)?;
            anyhow::ensure!(
                StableFileId::from_metadata(&File::open(&path)?.metadata()?)? == file_id,
                "dabacon 冻结源在捕获 target anchor 时被替换: {}",
                path.display()
            );
            return Ok(Self {
                path,
                file_id,
                dbnum: header.dbnum,
                db_type: header.db_type,
                target_sesno: header.latest_session.sesno,
                latest_sesno: header.latest_session.sesno,
                latest_ses_pgno: header.latest_ses_pgno,
                opened_len: before.len(),
                header_prefix,
                anchor,
            });
        }
        anyhow::bail!(
            "dabacon 冻结源捕获期间文件持续变化，重试 {CAPTURE_ATTEMPTS} 次仍未得到稳定 header/length: {}",
            path.display()
        )
    }

    /// 冻住当前世代、但把目标钉在一个不晚于最新的历史会话上（`DabaconSnapshot::open_at`）。
    pub fn capture_at(path: impl AsRef<Path>, target_sesno: u32) -> anyhow::Result<Self> {
        let mut frozen = Self::capture(path)?;
        frozen.freeze_target(target_sesno)?;
        Ok(frozen)
    }

    fn freeze_target(&mut self, target_sesno: u32) -> anyhow::Result<()> {
        anyhow::ensure!(
            target_sesno <= self.latest_sesno,
            "请求冻结会话 {target_sesno} 超过文件权威会话 {}",
            self.latest_sesno
        );
        if target_sesno != self.latest_sesno {
            let chain = session_chain(&self.path)?;
            let page = chain
                .iter()
                .find(|stamp| stamp.sesno == target_sesno)
                .ok_or_else(|| anyhow::anyhow!("dabacon 冻结源缺少目标会话 {target_sesno}"))?;
            self.latest_ses_pgno = page.pgno;
        }
        self.target_sesno = target_sesno;
        self.anchor = target_anchor(&self.path, target_sesno)?;
        Ok(())
    }

    /// Serialize the exact source generation used to prepare a durable attempt.
    pub fn commit_token_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&SourceCommitTokenV1 {
            version: 1,
            canonical_path: std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone()),
            file_id: self.file_id.clone(),
            dbnum: self.dbnum,
            db_type: self.db_type.clone(),
            target_sesno: self.target_sesno,
            latest_sesno: self.latest_sesno,
            opened_len: self.opened_len,
            header_prefix_hex: hex::encode(&self.header_prefix),
            anchor: self.anchor.clone(),
        })?)
    }

    /// Restore a frozen source from a durable attempt. Verification remains a
    /// separate explicit step so callers cannot confuse decoding with trust.
    pub fn from_commit_token_json(json: &str) -> anyhow::Result<Self> {
        let token: SourceCommitTokenV1 = serde_json::from_str(json)
            .map_err(|error| anyhow::anyhow!("解析冻结源 commit token 失败: {error}"))?;
        anyhow::ensure!(
            token.version == 1,
            "不支持冻结源 commit token 版本 {}",
            token.version
        );
        let header_prefix = hex::decode(&token.header_prefix_hex)
            .map_err(|error| anyhow::anyhow!("解析冻结源 header prefix 失败: {error}"))?;
        anyhow::ensure!(
            header_prefix.len() <= HEADER_PREFIX_LEN,
            "冻结源 header prefix 长度 {} 超过上限 {HEADER_PREFIX_LEN}",
            header_prefix.len()
        );
        Ok(Self {
            path: token.canonical_path,
            file_id: token.file_id,
            dbnum: token.dbnum,
            db_type: token.db_type,
            target_sesno: token.target_sesno,
            latest_sesno: token.latest_sesno,
            latest_ses_pgno: token.anchor.session_page.page,
            opened_len: token.opened_len,
            header_prefix,
            anchor: token.anchor,
        })
    }

    pub fn matches_path(&self, path: &Path) -> bool {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        canonical == std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone())
    }

    /// 重开同一路径并证明它还是同一世代（`DabaconSnapshot::open_verified`）。
    ///
    /// 同文件后来追加的会话可以接受；换文件、dbnum / 类型漂移、会话或长度回退一律拒绝。
    /// 返回的 token 保留**原来的**目标、长度与头样本——消费者仍绑定冻结那一代。
    pub fn reopen_verified(&self) -> anyhow::Result<Self> {
        let current = Self::capture(&self.path)?;
        anyhow::ensure!(
            current.file_id == self.file_id,
            "dabacon 路径 {} 的文件身份与冻结 token 不一致",
            self.path.display()
        );
        anyhow::ensure!(
            current.dbnum == self.dbnum && current.db_type == self.db_type,
            "dabacon 文件身份字段漂移：冻结 dbnum/type={}/{}, 当前={}/{}",
            self.dbnum,
            self.db_type,
            current.dbnum,
            current.db_type
        );
        anyhow::ensure!(
            current.latest_sesno >= self.target_sesno,
            "dabacon 会话从冻结值 {} 回退到 {}",
            self.target_sesno,
            current.latest_sesno
        );
        anyhow::ensure!(
            current.opened_len >= self.opened_len,
            "dabacon 文件长度从冻结值 {} 回退到 {}",
            self.opened_len,
            current.opened_len
        );
        if current.latest_sesno != self.target_sesno {
            let chain = session_chain(&self.path)?;
            anyhow::ensure!(
                chain.iter().any(|stamp| stamp.sesno == self.target_sesno),
                "dabacon 当前世代缺少冻结会话 {}",
                self.target_sesno
            );
        }
        let anchor = target_anchor(&self.path, self.target_sesno)?;
        anyhow::ensure!(
            anchor == self.anchor,
            "dabacon 冻结目标会话 {} 的 session/index anchor 已改变",
            self.target_sesno
        );
        Ok(self.clone())
    }

    /// 重开同一世代并把目标收窄到不晚于原冻结点的会话（`DabaconSnapshot::open_verified_at`）。
    pub fn reopen_verified_at(&self, target_sesno: u32) -> anyhow::Result<Self> {
        anyhow::ensure!(
            target_sesno <= self.target_sesno,
            "请求会话 {target_sesno} 超过冻结 token target={}",
            self.target_sesno
        );
        let mut frozen = self.reopen_verified()?;
        frozen.freeze_target(target_sesno)?;
        Ok(frozen)
    }

    /// 路径现在指向的还是当初打开的那个文件（`DabaconSnapshot::verify_path_identity`）。
    /// 同文件增长接受；换成另一个文件拒绝。
    pub fn verify_path_identity(&self) -> anyhow::Result<()> {
        let metadata = File::open(&self.path)?.metadata()?;
        let actual = StableFileId::from_metadata(&metadata)?;
        anyhow::ensure!(
            actual == self.file_id,
            "dabacon 路径 {} 已指向另一文件（冻结身份不匹配）",
            self.path.display()
        );
        Ok(())
    }

    /// 冻结世代的整条会话链（旧 → 新），用来取窗口内的会话清单。
    pub fn session_chain(&self) -> anyhow::Result<Vec<SessionStamp>> {
        self.verify_path_identity()?;
        session_chain(&self.path)
    }

    /// 以冻结目标会话打开只读引擎（索引根按 `target_sesno` 选）。
    pub fn open_engine_at_target(&self) -> anyhow::Result<ReadOnlyEngine> {
        self.verify_path_identity()?;
        ReadOnlyEngine::open_at(&self.path, self.target_sesno).map_err(|error| {
            anyhow::anyhow!(
                "以会话 {} 打开 {} 失败: {error}",
                self.target_sesno,
                self.path.display()
            )
        })
    }
}

fn target_anchor(path: &Path, target_sesno: u32) -> anyhow::Result<TargetAnchorV1> {
    let mut view =
        DbView::open_at(path, SessionSelector::Exact(target_sesno)).map_err(|error| {
            anyhow::anyhow!(
                "打开 {} 的冻结目标会话 {target_sesno} 失败: {error}",
                path.display()
            )
        })?;
    let session_page = view.session().page;
    let index_root = view.session().index_root;
    let session_bytes = view.pages_mut().get(session_page)?.to_vec();
    let index_bytes = view.pages_mut().get(index_root)?.to_vec();
    Ok(TargetAnchorV1 {
        session_page: session_page.into(),
        index_root: index_root.into(),
        session_page_sha256: hex::encode(Sha256::digest(&session_bytes)),
        index_root_sha256: hex::encode(Sha256::digest(&index_bytes)),
    })
}

fn read_prefix(file: &mut File) -> anyhow::Result<Vec<u8>> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    let mut buffer = vec![0u8; HEADER_PREFIX_LEN];
    file.read_exact(&mut buffer)?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::{FrozenSource, StableFileId};
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::path::PathBuf;

    fn temp_dir(case: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "gen-model-frozen-source-{case}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 同文件追加：身份不变——这是「Save Work 之后文件长了」要被接受的那一半。
    #[test]
    fn stable_identity_accepts_append_to_the_same_file() {
        let dir = temp_dir("append");
        let path = dir.join("append.db");
        File::create(&path).unwrap().write_all(b"before").unwrap();
        let before = StableFileId::from_metadata(&fs::metadata(&path).unwrap()).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"after")
            .unwrap();
        let after = StableFileId::from_metadata(&fs::metadata(&path).unwrap()).unwrap();
        assert_eq!(before, after);
        fs::remove_dir_all(dir).unwrap();
    }

    /// 原子替换（写新文件再 rename 盖过去）：身份必须变——这是要被拒绝的那一半。
    #[test]
    fn stable_identity_rejects_atomic_path_replacement() {
        let dir = temp_dir("replace");
        let path = dir.join("current.db");
        let replacement = dir.join("replacement.db");
        File::create(&path).unwrap().write_all(b"first").unwrap();
        File::create(&replacement)
            .unwrap()
            .write_all(b"second")
            .unwrap();
        let before = StableFileId::from_metadata(&fs::metadata(&path).unwrap()).unwrap();
        fs::remove_file(&path).unwrap();
        fs::rename(&replacement, &path).unwrap();
        let after = StableFileId::from_metadata(&fs::metadata(&path).unwrap()).unwrap();
        assert_ne!(before, after);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_commit_token_v1_round_trips_all_frozen_fields() {
        let frozen = FrozenSource::synthetic("fixture.db", 8000, "DESI", 42);
        let json = frozen.commit_token_json().expect("serialize token");
        let restored = FrozenSource::from_commit_token_json(&json).expect("restore token");

        assert_eq!(restored, frozen);
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"target_sesno\":42"));
    }

    #[test]
    fn source_commit_token_rejects_unknown_versions() {
        let frozen = FrozenSource::synthetic("fixture.db", 8000, "DESI", 42);
        let json = frozen
            .commit_token_json()
            .expect("serialize token")
            .replacen("\"version\":1", "\"version\":2", 1);
        let error = FrozenSource::from_commit_token_json(&json)
            .expect_err("unknown token versions must fail closed");

        assert!(error.to_string().contains("版本 2"));
    }
}
