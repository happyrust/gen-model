use super::{
    FORMAT_VERSION, FrozenRootEnvelope, PreparedRootPublication, RunManifest, digest, now_ms,
};
use crate::fast_model::room_topology_source::RoomTopologySource;
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

const SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FRAME_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Ready,
    Inflight,
    Committed,
    FailedHold,
    UnknownHold,
    Superseded,
    GenerationHold,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    segment: u64,
    offset: u64,
    bytes: u64,
    state: TaskState,
    created_ms: u64,
    run_id: String,
    error: Option<String>,
    #[serde(default)]
    prepared: Option<Frame>,
    #[serde(default)]
    mesh_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Frame {
    segment: u64,
    offset: u64,
    bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
enum Event {
    RoomSource(RoomTopologySource),
    RoomSourceStored(String),
    Manifest(RunManifest),
    Reserved(String),
    Activate(String),
    Result(FrozenRootEnvelope),
    GenerationFailure {
        envelope: FrozenRootEnvelope,
        error: String,
    },
    RetryGeneration(String),
    Prepared {
        id: String,
        plan: PreparedRootPublication,
    },
    State {
        id: String,
        state: TaskState,
        error: Option<String>,
    },
    Finish(String),
    ConsumerPause(Option<String>),
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    room_sources: BTreeMap<String, Frame>,
    #[serde(default)]
    stored_room_sources: std::collections::BTreeSet<String>,
    entries: BTreeMap<String, Entry>,
    manifests: BTreeMap<String, RunManifest>,
    active: std::collections::BTreeSet<String>,
    #[serde(default)]
    reserved: std::collections::BTreeSet<String>,
    finished: std::collections::BTreeSet<String>,
    live_by_segment: BTreeMap<u64, usize>,
    segment: u64,
    offset: u64,
    #[serde(default)]
    consumer_pause: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct QueueHealth {
    pub generated: usize,
    pub handed_off: usize,
    pub db_committed: usize,
    pub pending_publish: usize,
    pub failed_hold: usize,
    pub unknown_hold: usize,
    pub superseded: usize,
    pub held_jobs: Vec<serde_json::Value>,
    pub held_jobs_truncated: bool,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub log_bytes: u64,
    pub queued_mesh_bytes: u64,
    pub oldest_age_ms: Option<u64>,
    pub capacity_paused: bool,
    pub active_runs: usize,
}

/// 队列盯着的一件 mesh 依赖。入册时记下**内容摘要**而不只是字节数——长度相同的损坏
/// 文件照样能骗过长度比较。`bytes` 只用来记账（磁盘预算）。
#[derive(Debug, Clone)]
struct MeshDependency {
    bytes: u64,
    digest: String,
    /// 还有几条未确认的结果引用着它。归零后这件 mesh 归已发布模型所有，队列不再计账。
    refs: usize,
}

/// One owner holds this journal under the process/queue lock. Frames are
/// length+SHA256+JSON; JSON is deliberate (GeometryId uses tagged serde enums).
/// A checkpoint contains only offsets, never copies of queued geometry.
pub struct Journal {
    root: PathBuf,
    index: Index,
    file: File,
    disk_bytes: u64,
    memory_limit: u64,
    disk_limit: u64,
    mesh_root: Option<PathBuf>,
    mesh_usage: BTreeMap<String, MeshDependency>,
    mesh_bytes: u64,
    memory_bytes: u64,
    cache: BTreeMap<String, (Arc<FrozenRootEnvelope>, u64, (u64, u64))>,
    cache_order: BTreeMap<(u64, u64), String>,
    ready: BTreeMap<String, VecDeque<String>>,
    committed_by_run: BTreeMap<String, usize>,
    io_fault: bool,
    /// 有帧只写进了页缓存、还没 fsync。[`Journal::barrier`] 据此跳过空转的落盘。
    unsynced: bool,
    /// 日志文件的落盘次数。组级屏障（[`Journal::barrier`]）要证明的就是它：一批 N 根
    /// 从 N 次 fsync 降到 1 次。`/health` 的 `consumer_work.journal_file_syncs`。
    file_syncs: u64,
    // Retain the exclusive handle for the complete journal lifetime.
    _lock: File,
}

fn segment_path(root: &Path, id: u64) -> PathBuf {
    root.join(format!("{id:020}.journal"))
}

fn open_segment(root: &Path, id: u64, new: bool) -> anyhow::Result<File> {
    let path = segment_path(root, id);
    let created = new || !path.exists();
    let mut options = OpenOptions::new();
    options.append(true).read(true);
    if new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    let file = options.open(path)?;
    if created {
        file.sync_all()?;
        // POSIX file fsync alone does not persist its new directory entry.
        #[cfg(unix)]
        File::open(root)?.sync_all()?;
    }
    Ok(file)
}

impl Journal {
    pub fn open(root: PathBuf, memory_limit: u64, disk_limit: u64) -> anyhow::Result<Self> {
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()?.join(root)
        };
        #[cfg(unix)]
        let missing_directories = root
            .ancestors()
            .take_while(|path| !path.exists())
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        for directory in missing_directories.iter().rev() {
            File::open(directory)?.sync_all()?;
            if let Some(parent) = directory.parent() {
                File::open(parent)?.sync_all()?;
            }
        }
        let lock_path = root.join("owner.lock");
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let lock = options
            .open(lock_path)
            .context("model publish queue already owned")?;
        #[cfg(not(windows))]
        lock.try_lock()
            .context("model publish queue already owned")?;
        let checkpoint = root.join("checkpoint.json");
        let index = if checkpoint.exists() {
            let bytes = fs::read(&checkpoint)?;
            let (checksum, index): (String, serde_json::Value) = serde_json::from_slice(&bytes)?;
            ensure!(
                digest(&serde_json::to_vec(&index)?) == checksum,
                "queue checkpoint checksum mismatch"
            );
            serde_json::from_value(index)?
        } else {
            Index::default()
        };
        ensure!(
            index.offset == 0 || segment_path(&root, index.segment).exists(),
            "checkpoint journal segment is missing; queue retained"
        );
        let file = open_segment(&root, index.segment, false)?;
        let mut result = Self {
            root,
            index,
            file,
            disk_bytes: 0,
            memory_limit,
            disk_limit,
            mesh_root: None,
            mesh_usage: BTreeMap::new(),
            mesh_bytes: 0,
            memory_bytes: 0,
            cache: BTreeMap::new(),
            cache_order: BTreeMap::new(),
            ready: BTreeMap::new(),
            committed_by_run: BTreeMap::new(),
            io_fault: false,
            unsynced: false,
            file_syncs: 0,
            _lock: lock,
        };
        result.recover()?;
        Ok(result)
    }

    /// Account only queue-owned mesh dependencies. Once every referencing
    /// result is acknowledged, the published model owns the existing mesh file.
    pub fn with_mesh_root(mut self, root: PathBuf) -> anyhow::Result<Self> {
        self.mesh_root = Some(root);
        let live = self
            .index
            .entries
            .iter()
            .filter(|(_, e)| !matches!(e.state, TaskState::Committed | TaskState::Superseded))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in live {
            let payload = self.load(&id)?;
            // 重启这一路要**对着内容 id** 验一遍：进程不在的时候文件可能被改坏，而这时
            // 「当前文件」正是待证的东西，不能拿它自己当基准（旧实现就是这么做的，于是
            // 一次损坏之后永远校验通过）。只走活着的结果，条数被积压量框住。
            let ids = self.admit_mesh_dependencies(&payload, true)?;
            self.index.entries.get_mut(&id).unwrap().mesh_ids = ids.clone();
            self.adjust_mesh_refs(&ids, true);
        }
        Ok(self)
    }

    fn mesh_ids_and_sizes(&mut self, payload: &FrozenRootEnvelope) -> anyhow::Result<Vec<String>> {
        self.admit_mesh_dependencies(payload, false)
    }

    /// 把这条结果引用的 mesh 收进账本；每个 id 只在**第一次见到**时读一遍文件，记下它的
    /// 内容摘要当基准。`verify_content_addressed_mesh` 只在重启重建时开，见
    /// [`Journal::with_mesh_root`]。
    fn admit_mesh_dependencies(
        &mut self,
        payload: &FrozenRootEnvelope,
        verify_content: bool,
    ) -> anyhow::Result<Vec<String>> {
        let Some(root) = &self.mesh_root else {
            return Ok(Vec::new());
        };
        let ids = payload
            .projection
            .upserts
            .iter()
            .map(|g| g.mesh_id.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        for id in &ids {
            ensure!(
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                "invalid frozen mesh identity"
            );
            if !self.mesh_usage.contains_key(id) {
                let path = root.join(format!("{id}.mesh"));
                let meta = fs::metadata(&path)
                    .with_context(|| format!("missing immutable mesh {}", path.display()))?;
                ensure!(
                    meta.is_file() && meta.len() > 0,
                    "invalid immutable mesh {}",
                    path.display()
                );
                if verify_content {
                    crate::fast_model::e3d_mesh_store::verify_content_addressed_mesh(&path, id)?;
                }
                let dependency = MeshDependency {
                    bytes: meta.len(),
                    digest: digest(&fs::read(&path)?),
                    refs: 0,
                };
                self.mesh_usage.insert(id.clone(), dependency);
            }
        }
        Ok(ids)
    }

    fn adjust_mesh_refs(&mut self, ids: &[String], live: bool) {
        for id in ids {
            if let Some(dependency) = self.mesh_usage.get_mut(id) {
                if live {
                    if dependency.refs == 0 {
                        self.mesh_bytes += dependency.bytes;
                    }
                    dependency.refs += 1;
                } else if dependency.refs > 0 {
                    dependency.refs -= 1;
                    if dependency.refs == 0 {
                        self.mesh_bytes -= dependency.bytes;
                    }
                }
            }
        }
    }

    pub fn verify_meshes(&self, payload: &FrozenRootEnvelope) -> anyhow::Result<()> {
        let Some(root) = &self.mesh_root else {
            return Ok(());
        };
        for id in payload
            .projection
            .upserts
            .iter()
            .map(|g| &g.mesh_id)
            .collect::<std::collections::BTreeSet<_>>()
        {
            let expected = self
                .mesh_usage
                .get(id)
                .context("mesh dependency is absent from queue index")?;
            let path = root.join(format!("{id}.mesh"));
            let meta = fs::metadata(&path)
                .with_context(|| format!("missing immutable mesh {}", path.display()))?;
            ensure!(meta.is_file(), "invalid immutable mesh {}", path.display());
            // 比内容摘要，不只比长度：长度相同的损坏文件正是长度比较看不见的那一类。
            // 7997 那轮整个 mesh 语料 190.9 MB / 15,556 件，摘要一遍是零点几秒的量级，
            // 而同一轮 `publish` 是 68.9 s——这道门付得起。
            ensure!(
                digest(&fs::read(&path)?) == expected.digest,
                "immutable mesh changed: {}",
                path.display()
            );
        }
        Ok(())
    }

    fn recover(&mut self) -> anyhow::Result<()> {
        let mut segments = Vec::new();
        for path in fs::read_dir(&self.root)? {
            let path = path?.path();
            if path.extension().and_then(|v| v.to_str()) == Some("journal") {
                let id: u64 = path.file_stem().unwrap().to_str().unwrap().parse()?;
                self.disk_bytes += path.metadata()?.len();
                if id >= self.index.segment {
                    segments.push(id);
                }
            }
        }
        segments.sort_unstable();
        ensure!(
            segments.first() == Some(&self.index.segment),
            "checkpoint journal is missing"
        );
        ensure!(
            segments.windows(2).all(|s| s[1] == s[0] + 1),
            "journal segment gap; queue retained"
        );
        for &id in &segments {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(segment_path(&self.root, id))?;
            let mut offset = if id == self.index.segment {
                self.index.offset
            } else {
                0
            };
            let len = file.metadata()?.len();
            ensure!(offset <= len, "checkpoint extends beyond journal");
            while offset < len {
                file.seek(SeekFrom::Start(offset))?;
                if len - offset < 40 {
                    ensure!(Some(&id) == segments.last(), "truncated non-tail journal");
                    file.set_len(offset)?;
                    file.sync_all()?;
                    self.disk_bytes -= len - offset;
                    break;
                }
                let mut header = [0u8; 40];
                file.read_exact(&mut header)?;
                let n = u64::from_le_bytes(header[..8].try_into().unwrap());
                ensure!(n <= MAX_FRAME_BYTES, "invalid queue frame length");
                if offset + 40 + n > len {
                    ensure!(Some(&id) == segments.last(), "truncated non-tail payload");
                    file.set_len(offset)?;
                    file.sync_all()?;
                    self.disk_bytes -= len - offset;
                    break;
                }
                let mut bytes = vec![0; n as usize];
                file.read_exact(&mut bytes)?;
                ensure!(
                    digest(&bytes) == hex::encode(&header[8..]),
                    "queue checksum mismatch at {id}:{offset}; file retained"
                );
                let event: Event = serde_json::from_slice(&bytes)?;
                self.apply(event, id, offset, n + 40);
                offset += n + 40;
            }
            self.index.segment = id;
            self.index.offset = offset;
        }
        self.file = open_segment(&self.root, self.index.segment, false)?;
        // An interrupted submission is never silently replayed. Runtime checks
        // database ack markers before allowing an explicit replay.
        for entry in self.index.entries.values_mut() {
            if entry.state == TaskState::Inflight {
                entry.state = TaskState::UnknownHold;
            }
        }
        for (id, e) in &self.index.entries {
            if e.state == TaskState::Ready {
                self.ready
                    .entry(e.run_id.clone())
                    .or_default()
                    .push_back(id.clone());
            }
        }
        self.committed_by_run.clear();
        for e in self
            .index
            .entries
            .values()
            .filter(|e| e.state == TaskState::Committed)
        {
            *self.committed_by_run.entry(e.run_id.clone()).or_default() += 1;
        }
        for (id, e) in &self.index.entries {
            if !matches!(e.state, TaskState::Committed | TaskState::Superseded) {
                self.load(id)
                    .with_context(|| format!("recover immutable result {id}"))?;
                self.load_prepared(id)
                    .with_context(|| format!("recover prepared publication {id}"))?;
            }
        }
        for key in self.index.room_sources.keys() {
            if !self.room_source_stored(key) {
                self.load_room_source(key)?;
            }
        }
        Ok(())
    }

    fn apply(&mut self, event: Event, segment: u64, offset: u64, bytes: u64) {
        match event {
            Event::RoomSource(source) => {
                self.index.room_sources.insert(
                    source.key(),
                    Frame {
                        segment,
                        offset,
                        bytes,
                    },
                );
                *self.index.live_by_segment.entry(segment).or_default() += 1;
            }
            Event::RoomSourceStored(key) => {
                if self.index.stored_room_sources.insert(key.clone()) {
                    if let Some(frame) = self.index.room_sources.get(&key) {
                        *self.index.live_by_segment.entry(frame.segment).or_default() -= 1;
                    }
                }
            }
            Event::GenerationFailure { envelope, error } => {
                let id = envelope.job_id.clone();
                self.apply(Event::Result(envelope), segment, offset, bytes);
                if let Some(e) = self.index.entries.get_mut(&id) {
                    e.state = TaskState::GenerationHold;
                    e.error = Some(error);
                }
            }
            Event::RetryGeneration(id) => {
                if let Some(e) = self.index.entries.remove(&id) {
                    *self.index.live_by_segment.entry(e.segment).or_default() -= 1;
                    self.adjust_mesh_refs(&e.mesh_ids, false);
                }
            }
            Event::ConsumerPause(reason) => {
                self.index.consumer_pause = reason;
            }
            Event::Manifest(m) => {
                self.index.manifests.insert(m.run_id.clone(), m);
            }
            Event::Reserved(id) => {
                self.index.reserved.insert(id);
            }
            Event::Activate(id) => {
                self.index.active.insert(id);
            }
            Event::Finish(id) => {
                self.index.finished.insert(id.clone());
                self.index.active.remove(&id);
            }
            Event::Result(e) => {
                if let Some(old) = self.index.entries.remove(&e.job_id) {
                    if !matches!(old.state, TaskState::Committed | TaskState::Superseded) {
                        *self.index.live_by_segment.entry(old.segment).or_default() -= 1;
                        self.adjust_mesh_refs(&old.mesh_ids, false);
                    }
                }
                *self.index.live_by_segment.entry(segment).or_default() += 1;
                let mesh_ids = e
                    .projection
                    .upserts
                    .iter()
                    .map(|g| g.mesh_id.clone())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                self.adjust_mesh_refs(&mesh_ids, true);
                self.index.entries.insert(
                    e.job_id.clone(),
                    Entry {
                        segment,
                        offset,
                        bytes,
                        state: TaskState::Ready,
                        created_ms: e.created_ms,
                        run_id: e.run_id.clone(),
                        error: None,
                        prepared: None,
                        mesh_ids,
                    },
                );
            }
            Event::Prepared { id, .. } => {
                if let Some(e) = self.index.entries.get_mut(&id) {
                    e.prepared = Some(Frame {
                        segment,
                        offset,
                        bytes,
                    });
                    *self.index.live_by_segment.entry(segment).or_default() += 1;
                }
            }
            Event::State { id, state, error } => {
                let mut mesh_change = None;
                if let Some(e) = self.index.entries.get_mut(&id) {
                    if e.state != state {
                        let count = self.committed_by_run.entry(e.run_id.clone()).or_default();
                        if e.state == TaskState::Committed {
                            *count = count.saturating_sub(1);
                        }
                        if state == TaskState::Committed {
                            *count += 1;
                        }
                    }
                    let was_live = !matches!(e.state, TaskState::Committed | TaskState::Superseded);
                    let live = !matches!(state, TaskState::Committed | TaskState::Superseded);
                    if was_live != live {
                        mesh_change = Some((e.mesh_ids.clone(), live));
                        let n = self.index.live_by_segment.entry(e.segment).or_default();
                        if live {
                            *n += 1;
                        } else {
                            *n -= 1;
                        }
                        if let Some(p) = &e.prepared {
                            let n = self.index.live_by_segment.entry(p.segment).or_default();
                            if live {
                                *n += 1;
                            } else {
                                *n -= 1;
                            }
                        }
                    }
                    e.state = state;
                    e.error = error;
                }
                if let Some((ids, live)) = mesh_change {
                    self.adjust_mesh_refs(&ids, live);
                }
            }
        }
    }

    /// 日志文件的唯一落盘入口，顺带计数与清「未落盘」标记。
    fn sync_file(&mut self) -> std::io::Result<()> {
        self.file_syncs += 1;
        self.file.sync_all()?;
        self.unsynced = false;
        Ok(())
    }

    pub fn file_syncs(&self) -> u64 {
        self.file_syncs
    }

    /// 组级持久化屏障：把此前追加的帧一次性落盘。`sync_all` 覆盖当前段里的每一帧，
    /// 所以一组只要一次；跨段时旧段已在 [`Journal::append`] 轮换处落过盘。上次落盘后
    /// 没有新帧就直接返回，别为空转再付一次 fsync。
    ///
    /// 失败按「不确定的写」处理（`io_fault`）：后续追加一律拒绝，等重启恢复。
    pub fn barrier(&mut self) -> anyhow::Result<()> {
        ensure!(
            !self.io_fault,
            "journal has an uncertain write; restart and recover before writing again"
        );
        if !self.unsynced {
            return Ok(());
        }
        self.io_fault = true;
        self.sync_file()?;
        self.io_fault = false;
        Ok(())
    }

    fn append(&mut self, event: Event, sync: bool) -> anyhow::Result<()> {
        ensure!(
            !self.io_fault,
            "journal has an uncertain write; restart and recover before writing again"
        );
        let bytes = serde_json::to_vec(&event)?;
        ensure!(
            bytes.len() as u64 <= MAX_FRAME_BYTES,
            "single queue frame exceeds 512 MiB"
        );
        // Result writes use 90%; the remaining capacity belongs to ACKs/errors.
        let ceiling = if matches!(&event, Event::Result(_)) {
            self.disk_limit / 10 * 9
        } else {
            self.disk_limit
        };
        ensure!(
            self.disk_bytes + self.mesh_bytes + bytes.len() as u64 + 40 <= ceiling,
            "model publish queue capacity exceeded"
        );
        self.io_fault = true;
        if self.index.offset >= SEGMENT_BYTES {
            self.sync_file()?;
            self.checkpoint()?;
            self.index.segment += 1;
            self.index.offset = 0;
            self.file = open_segment(&self.root, self.index.segment, true)?;
        }
        let offset = self.index.offset;
        self.file.write_all(&(bytes.len() as u64).to_le_bytes())?;
        self.file.write_all(&hex::decode(digest(&bytes))?)?;
        self.file.write_all(&bytes)?;
        self.unsynced = true;
        if sync {
            self.sync_file()?;
        }
        self.index.offset += bytes.len() as u64 + 40;
        self.disk_bytes += bytes.len() as u64 + 40;
        self.apply(event, self.index.segment, offset, bytes.len() as u64 + 40);
        self.io_fault = false;
        Ok(())
    }

    pub fn register(&mut self, manifest: RunManifest) -> anyhow::Result<()> {
        ensure!(
            manifest.version == FORMAT_VERSION,
            "unsupported run manifest version"
        );
        ensure!(
            manifest
                .room_source_keys
                .iter()
                .all(|key| self.index.room_sources.contains_key(key)),
            "manifest room topology source is not durable"
        );
        self.append(Event::Manifest(manifest), true)
    }
    pub fn mark_reserved(&mut self, id: &str) -> anyhow::Result<()> {
        ensure!(
            self.index.manifests.contains_key(id),
            "unknown initialization run"
        );
        self.append(Event::Reserved(id.into()), true)
    }
    pub fn reserved(&self, id: &str) -> bool {
        self.index.reserved.contains(id)
    }
    pub fn activate(&mut self, id: &str) -> anyhow::Result<()> {
        ensure!(
            self.reserved(id),
            "initialization reservation is incomplete; manual repair required"
        );
        self.append(Event::Activate(id.into()), true)
    }
    pub fn finish(&mut self, id: &str) -> anyhow::Result<()> {
        self.append(Event::Finish(id.into()), true)?;
        self.checkpoint()?;
        self.reclaim()
    }
    pub fn unfinished_dbnums(&self) -> Vec<u32> {
        self.index
            .manifests
            .values()
            .filter(|m| !self.index.finished.contains(&m.run_id))
            .map(|m| m.dbnum)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn manifests(&self) -> Vec<RunManifest> {
        self.index
            .manifests
            .values()
            .filter(|m| !self.index.finished.contains(&m.run_id))
            .cloned()
            .collect()
    }
    pub fn model_projection_was_empty(&self, run_id: &str) -> Option<bool> {
        self.index
            .manifests
            .get(run_id)
            .map(|manifest| manifest.model_projection_empty_at_reservation)
    }
    pub fn completed_manifests(&self) -> Vec<RunManifest> {
        self.index
            .manifests
            .values()
            .filter(|m| {
                self.index.finished.contains(&m.run_id) && (2..=FORMAT_VERSION).contains(&m.version)
            })
            .cloned()
            .collect()
    }
    pub fn pending_manifests(
        &self,
        started: &std::collections::BTreeSet<String>,
    ) -> Vec<RunManifest> {
        self.index
            .manifests
            .values()
            .filter(|m| {
                !self.index.finished.contains(&m.run_id)
                    && self.index.reserved.contains(&m.run_id)
                    && !started.contains(&m.run_id)
            })
            .cloned()
            .collect()
    }
    /// 已开始生成、尚未收口、且还有根没定案的 run 数——内存侧的「在飞」口径：每个这样的
    /// run 在收口前都握着自己的生成输入（`DbSet` + `SourceIndex`）与在飞几何。
    ///
    /// 整个 run 的根都已停在 hold 上（生成整体失败、或全部 FailedHold / UnknownHold）的
    /// 不算在飞：它在等人工 replay，不该永久占着一个生成槽位把后面的库都堵死。
    pub fn in_flight_runs(&self, started: &std::collections::BTreeSet<String>) -> usize {
        let candidates = started
            .iter()
            .filter(|id| !self.index.finished.contains(*id))
            .filter_map(|id| self.index.manifests.get(id))
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return 0;
        }
        let mut held = BTreeMap::<&str, usize>::new();
        for e in self.index.entries.values() {
            if matches!(
                e.state,
                TaskState::GenerationHold | TaskState::FailedHold | TaskState::UnknownHold
            ) {
                *held.entry(e.run_id.as_str()).or_default() += 1;
            }
        }
        candidates
            .iter()
            .filter(|m| held.get(m.run_id.as_str()).copied().unwrap_or(0) < m.roots.len())
            .count()
    }
    pub fn finish_candidates(&self) -> Vec<RunManifest> {
        self.index
            .manifests
            .values()
            .filter(|m| {
                self.index.active.contains(&m.run_id)
                    && self.committed_by_run.get(&m.run_id).copied().unwrap_or(0) == m.roots.len()
            })
            .cloned()
            .collect()
    }
    pub fn run_for_dbnum(&self, dbnum: u32) -> Option<&RunManifest> {
        self.index
            .manifests
            .values()
            .find(|m| m.dbnum == dbnum && !self.index.finished.contains(&m.run_id))
    }
    pub fn active(&self, id: &str) -> bool {
        self.index.active.contains(id)
    }
    pub fn pause_reason(&self) -> Option<String> {
        self.index.consumer_pause.clone()
    }
    pub fn set_pause(&mut self, reason: Option<String>) -> anyhow::Result<()> {
        self.append(Event::ConsumerPause(reason), true)
    }
    pub fn state(&self, id: &str) -> Option<TaskState> {
        self.index.entries.get(id).map(|e| e.state)
    }
    pub(crate) fn has_room_source(&self, key: &str) -> bool {
        self.index.room_sources.contains_key(key)
    }
    pub(crate) fn room_source_stored(&self, key: &str) -> bool {
        self.index.stored_room_sources.contains(key)
    }
    pub(crate) fn append_room_source(&mut self, source: RoomTopologySource) -> anyhow::Result<()> {
        source.validate()?;
        if self.has_room_source(&source.key()) {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&source)?.len() as u64 + 100;
        ensure!(
            self.disk_bytes + self.mesh_bytes + bytes <= self.disk_limit / 10 * 9,
            "room topology capture exceeds initialization queue capacity"
        );
        self.append(Event::RoomSource(source), true)
    }
    pub(crate) fn load_room_source(&self, key: &str) -> anyhow::Result<RoomTopologySource> {
        ensure!(
            !self.room_source_stored(key),
            "room topology source ownership has transferred to database"
        );
        let frame = self
            .index
            .room_sources
            .get(key)
            .context("missing local room topology source")?;
        let mut file = File::open(segment_path(&self.root, frame.segment))?;
        file.seek(SeekFrom::Start(frame.offset))?;
        let mut header = [0; 40];
        file.read_exact(&mut header)?;
        ensure!(
            u64::from_le_bytes(header[..8].try_into().unwrap()) + 40 == frame.bytes,
            "room topology frame length mismatch"
        );
        let mut bytes = vec![0; (frame.bytes - 40) as usize];
        file.read_exact(&mut bytes)?;
        ensure!(
            digest(&bytes) == hex::encode(&header[8..]),
            "room topology frame checksum mismatch"
        );
        match serde_json::from_slice(&bytes)? {
            Event::RoomSource(source) if source.key() == key => {
                source.validate()?;
                Ok(source)
            }
            _ => anyhow::bail!("invalid room topology frame identity"),
        }
    }
    pub(crate) fn mark_room_source_stored(&mut self, key: &str) -> anyhow::Result<()> {
        ensure!(self.has_room_source(key), "unknown room topology source");
        if !self.room_source_stored(key) {
            self.append(Event::RoomSourceStored(key.into()), true)?;
        }
        Ok(())
    }
    pub fn held(&self) -> Vec<String> {
        self.index
            .entries
            .iter()
            .filter(|(_, e)| matches!(e.state, TaskState::UnknownHold))
            .map(|(id, _)| id.clone())
            .collect()
    }
    pub fn has_capacity(&self) -> bool {
        self.disk_bytes + self.mesh_bytes < self.disk_limit / 10 * 9
    }
    pub fn can_handoff(&mut self, payload: &FrozenRootEnvelope) -> anyhow::Result<bool> {
        self.can_handoff_batch(std::slice::from_ref(payload))
    }
    pub fn can_handoff_batch(&mut self, envelopes: &[FrozenRootEnvelope]) -> anyhow::Result<bool> {
        let mut seen = std::collections::BTreeSet::new();
        let mut meshes = std::collections::BTreeSet::new();
        let mut additional = 0u64;
        for e in envelopes {
            if self
                .index
                .entries
                .get(&e.job_id)
                .is_some_and(|old| old.state != TaskState::GenerationHold)
                || !seen.insert(&e.job_id)
            {
                continue;
            }
            additional += serde_json::to_vec(&Event::Result(e.clone()))?.len() as u64 + 40;
            for id in self.mesh_ids_and_sizes(e)? {
                let dependency = &self.mesh_usage[&id];
                let (bytes, refs) = (dependency.bytes, dependency.refs);
                if refs == 0 && meshes.insert(id) {
                    additional += bytes;
                }
            }
        }
        ensure!(
            additional <= self.disk_limit / 10 * 9,
            "one handoff group and mesh dependencies exceed the entire queue budget"
        );
        Ok(self
            .disk_bytes
            .saturating_add(self.mesh_bytes)
            .saturating_add(additional)
            <= self.disk_limit / 10 * 9)
    }

    /// Called by the single batching actor. No acknowledgment escapes before
    /// sync_all covers every frame in the group.
    pub fn handoff(&mut self, envelopes: Vec<FrozenRootEnvelope>) -> anyhow::Result<()> {
        // Reject a whole group before mutating the append cursor. A capacity
        // failure must not leave half a group visible without acknowledgments.
        ensure!(
            self.can_handoff_batch(&envelopes)?,
            "model publish queue capacity exceeded"
        );
        let mut admitted = Vec::new();
        for e in envelopes {
            if self
                .index
                .entries
                .get(&e.job_id)
                .is_some_and(|old| old.state != TaskState::GenerationHold)
            {
                continue;
            }
            let id = e.job_id.clone();
            let size = serde_json::to_vec(&e)?.len() as u64;
            self.append(Event::Result(e.clone()), false)?;
            admitted.push((id, Arc::new(e), size));
        }
        self.barrier()?;
        for (id, e, size) in admitted {
            self.ready
                .entry(e.run_id.clone())
                .or_default()
                .push_back(id.clone());
            if size <= self.memory_limit {
                while self.memory_bytes + size > self.memory_limit {
                    let Some((_, old)) = self.cache_order.pop_first() else {
                        break;
                    };
                    self.evict_cached(&old);
                }
                self.evict_cached(&id);
                let entry = &self.index.entries[&id];
                let order = (entry.segment, entry.offset);
                self.memory_bytes += size;
                self.cache_order.insert(order, id.clone());
                self.cache.insert(id, (e, size, order));
            }
        }
        Ok(())
    }

    fn evict_cached(&mut self, id: &str) {
        if let Some((_, bytes, order)) = self.cache.remove(id) {
            self.memory_bytes -= bytes;
            self.cache_order.remove(&order);
        }
    }
    #[cfg(test)]
    pub(super) fn cache_entry_counts(&self) -> (usize, usize) {
        (self.cache.len(), self.cache_order.len())
    }

    pub fn load(&self, id: &str) -> anyhow::Result<Arc<FrozenRootEnvelope>> {
        if let Some((e, _, _)) = self.cache.get(id) {
            return Ok(e.clone());
        }
        let e = self
            .index
            .entries
            .get(id)
            .context("unknown model publish job")?;
        let mut file = File::open(segment_path(&self.root, e.segment))?;
        file.seek(SeekFrom::Start(e.offset))?;
        let mut header = [0; 40];
        file.read_exact(&mut header)?;
        ensure!(
            u64::from_le_bytes(header[..8].try_into().unwrap()) + 40 == e.bytes,
            "model publish frame length mismatch"
        );
        let mut bytes = vec![0; (e.bytes - 40) as usize];
        file.read_exact(&mut bytes)?;
        ensure!(
            digest(&bytes) == hex::encode(&header[8..]),
            "model publish payload checksum mismatch"
        );
        match serde_json::from_slice(&bytes)? {
            Event::Result(mut payload)
            | Event::GenerationFailure {
                envelope: mut payload,
                ..
            } => {
                if payload.source_token.is_empty() {
                    payload.source_token = self
                        .index
                        .manifests
                        .get(&payload.run_id)
                        .context("result has no frozen source token or run manifest")?
                        .source_token
                        .clone();
                }
                Ok(Arc::new(payload))
            }
            _ => anyhow::bail!("model publish offset is not a result"),
        }
    }
    pub fn prepare(&mut self, id: &str, plan: PreparedRootPublication) -> anyhow::Result<()> {
        let e = self
            .index
            .entries
            .get(id)
            .context("unknown model publish job")?;
        ensure!(e.prepared.is_none(), "publication plan is already frozen");
        ensure!(
            e.state == TaskState::Ready,
            "prepare requires a ready task before any writes"
        );
        self.append(
            Event::Prepared {
                id: id.into(),
                plan,
            },
            true,
        )
    }
    /// The immutable plan and the intent to write share one durability barrier.
    /// No database mutation is allowed until this method returns successfully.
    /// A recovered plan without an Inflight record is safe to start; a recovered
    /// Inflight record is held for database-ACK reconciliation as usual.
    ///
    /// 单根路径用这一个方法就够；成批发布走
    /// [`Journal::begin_publication_deferred`] + [`Journal::barrier`]，同一个屏障。
    pub fn begin_publication(
        &mut self,
        id: &str,
        plan: Option<PreparedRootPublication>,
    ) -> anyhow::Result<()> {
        self.begin_publication_deferred(id, plan)?;
        self.barrier()
    }

    /// 同 [`Journal::begin_publication`]，但把落盘留给 [`Journal::barrier`]：一批根
    /// 一起发时，逐根调本方法、发出本组第一条 SQL 之前调一次 `barrier`，「每根一次
    /// fsync」就摊成「每批一次」。
    ///
    /// **调用方义务**：拿不到 `barrier` 的 Ok 就一条 SQL 都不许发——Inflight 记录还
    /// 可能只在页缓存里，掉电后恢复不出「已经动过库」的意图，也就走不到 ACK 对账。
    pub fn begin_publication_deferred(
        &mut self,
        id: &str,
        plan: Option<PreparedRootPublication>,
    ) -> anyhow::Result<()> {
        let entry = self
            .index
            .entries
            .get(id)
            .context("unknown model publish job")?;
        ensure!(
            entry.state == TaskState::Ready,
            "publication requires a ready task"
        );
        ensure!(
            entry.prepared.is_some() != plan.is_some(),
            "publication requires exactly one frozen plan"
        );
        if let Some(plan) = plan {
            self.append(
                Event::Prepared {
                    id: id.into(),
                    plan,
                },
                false,
            )?;
        }
        self.append(
            Event::State {
                id: id.into(),
                state: TaskState::Inflight,
                error: None,
            },
            false,
        )
    }
    pub fn generation_failure(
        &mut self,
        envelope: FrozenRootEnvelope,
        error: String,
    ) -> anyhow::Result<()> {
        if self.index.entries.contains_key(&envelope.job_id) {
            return Ok(());
        }
        self.append(Event::GenerationFailure { envelope, error }, true)
    }
    pub fn retry_generation(&mut self, id: &str) -> anyhow::Result<()> {
        ensure!(
            self.state(id) == Some(TaskState::GenerationHold),
            "task is not a generation hold"
        );
        let old_segment = self.index.entries[id].segment;
        self.append(Event::RetryGeneration(id.into()), true)?;
        self.evict_cached(id);
        if self
            .index
            .live_by_segment
            .get(&old_segment)
            .copied()
            .unwrap_or(0)
            == 0
        {
            if old_segment == self.index.segment {
                self.io_fault = true;
                self.sync_file()?;
                self.index.segment += 1;
                self.index.offset = 0;
                self.file = open_segment(&self.root, self.index.segment, true)?;
                self.io_fault = false;
            }
            self.checkpoint()?;
            self.reclaim()?;
        }
        Ok(())
    }
    pub fn load_prepared(&self, id: &str) -> anyhow::Result<Option<PreparedRootPublication>> {
        let e = self
            .index
            .entries
            .get(id)
            .context("unknown model publish job")?;
        let Some(p) = &e.prepared else {
            return Ok(None);
        };
        let mut file = File::open(segment_path(&self.root, p.segment))?;
        file.seek(SeekFrom::Start(p.offset))?;
        let mut header = [0; 40];
        file.read_exact(&mut header)?;
        ensure!(
            u64::from_le_bytes(header[..8].try_into().unwrap()) + 40 == p.bytes,
            "prepared frame length mismatch"
        );
        let mut bytes = vec![0; (p.bytes - 40) as usize];
        file.read_exact(&mut bytes)?;
        ensure!(
            digest(&bytes) == hex::encode(&header[8..]),
            "prepared publication checksum mismatch"
        );
        match serde_json::from_slice(&bytes)? {
            Event::Prepared { id: actual, plan } if actual == id => Ok(Some(plan)),
            _ => anyhow::bail!("invalid prepared publication pointer"),
        }
    }
    pub fn next_ready(&mut self) -> Option<String> {
        for (run, ready) in &mut self.ready {
            if self.index.manifests.contains_key(run) && !self.index.active.contains(run) {
                continue;
            }
            while let Some(id) = ready.pop_front() {
                if self
                    .index
                    .entries
                    .get(&id)
                    .is_some_and(|e| e.state == TaskState::Ready)
                {
                    return Some(id);
                }
            }
        }
        None
    }
    /// Pop at most `limit` ready jobs using the per-run queues. This keeps
    /// dequeue cost proportional to the returned batch instead of scanning the
    /// complete durable backlog.
    pub fn next_ready_batch(&mut self, limit: usize) -> Vec<String> {
        let mut jobs = Vec::with_capacity(limit);
        while jobs.len() < limit {
            let Some(id) = self.next_ready() else {
                break;
            };
            jobs.push(id);
        }
        jobs
    }
    pub fn transition(
        &mut self,
        id: &str,
        state: TaskState,
        error: Option<String>,
    ) -> anyhow::Result<()> {
        self.transition_with_sync(id, state, error, true)
    }
    /// The database final-ACK is the durable authority for a committed root.
    /// Leave this tiny local state frame for the next publication barrier (or
    /// run finish) to fsync. If the process dies first, the already-durable
    /// Inflight frame is recovered as UnknownHold and the database ACK repairs
    /// it without replaying side effects.
    pub fn transition_after_database_commit(&mut self, id: &str) -> anyhow::Result<()> {
        self.transition_with_sync(id, TaskState::Committed, None, false)
    }
    fn transition_with_sync(
        &mut self,
        id: &str,
        state: TaskState,
        error: Option<String>,
        sync: bool,
    ) -> anyhow::Result<()> {
        ensure!(
            self.index.entries.contains_key(id),
            "unknown model publish job {id}"
        );
        self.append(
            Event::State {
                id: id.into(),
                state,
                error,
            },
            sync,
        )?;
        if state == TaskState::Ready {
            self.ready
                .entry(self.index.entries[id].run_id.clone())
                .or_default()
                .push_back(id.into());
        }
        if matches!(state, TaskState::Committed | TaskState::Superseded) {
            self.evict_cached(id);
            // Reclaim sealed segments throughout a run, not only after its end:
            // otherwise a run larger than the disk budget could never finish.
            let segment = self.index.entries[id].segment;
            if segment < self.index.segment && self.index.live_by_segment.get(&segment) == Some(&0)
            {
                self.checkpoint()?;
                self.reclaim()?;
            }
        }
        Ok(())
    }
    pub fn checkpoint(&mut self) -> anyhow::Result<()> {
        self.sync_file()?;
        let value = serde_json::to_value(&self.index)?;
        let checksum = digest(&serde_json::to_vec(&value)?);
        let bytes = serde_json::to_vec(&(checksum, value))?;
        let tmp = self.root.join("checkpoint.tmp");
        let mut f = File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        // Windows rename replaces a file atomically via std's MoveFileExW.
        fs::rename(tmp, self.root.join("checkpoint.json"))?;
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
    fn reclaim(&mut self) -> anyhow::Result<()> {
        let used: std::collections::BTreeSet<u64> = self
            .index
            .live_by_segment
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(id, _)| *id)
            .collect();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.extension().and_then(|v| v.to_str()) != Some("journal") {
                continue;
            }
            let id: u64 = path.file_stem().unwrap().to_str().unwrap().parse()?;
            if id < self.index.segment && !used.contains(&id) {
                let n = path.metadata()?.len();
                fs::remove_file(path)?;
                self.disk_bytes -= n;
            }
        }
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
    pub fn health(&self) -> QueueHealth {
        let mut h = QueueHealth {
            generated: self.index.entries.len(),
            handed_off: self.index.entries.len(),
            memory_bytes: self.memory_bytes,
            disk_bytes: self.disk_bytes + self.mesh_bytes,
            log_bytes: self.disk_bytes,
            queued_mesh_bytes: self.mesh_bytes,
            capacity_paused: !self.has_capacity(),
            active_runs: self.index.active.len(),
            ..QueueHealth::default()
        };
        for (id, e) in &self.index.entries {
            match e.state {
                TaskState::Committed => h.db_committed += 1,
                TaskState::FailedHold => h.failed_hold += 1,
                TaskState::GenerationHold => {
                    h.failed_hold += 1;
                    h.handed_off -= 1;
                }
                TaskState::UnknownHold => h.unknown_hold += 1,
                TaskState::Superseded => h.superseded += 1,
                _ => {}
            }
            if matches!(
                e.state,
                TaskState::FailedHold
                    | TaskState::UnknownHold
                    | TaskState::GenerationHold
                    | TaskState::Superseded
            ) {
                if h.held_jobs.len() < 100 {
                    h.held_jobs.push(serde_json::json!({"job_id":id,"run_id":e.run_id,"state":e.state,"error":e.error}));
                } else {
                    h.held_jobs_truncated = true;
                }
            }
            if !matches!(e.state, TaskState::Committed | TaskState::Superseded) {
                h.pending_publish += 1;
                let age = now_ms().saturating_sub(e.created_ms);
                h.oldest_age_ms = Some(h.oldest_age_ms.unwrap_or(0).max(age));
            }
        }
        h
    }
}
