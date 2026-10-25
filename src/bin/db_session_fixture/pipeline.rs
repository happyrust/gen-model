//! 夹具管线主体：`pack`（录制产物 → 夹具目录）与 `verify_fixture`（离线复验）。
//!
//! 放在模块里而不是 bin 根，是为了让集成测试能直接调用同一份实现——阶段二是
//! 一次性 E3D 录制，pack 出问题就要再占一个生产空窗重录，这条路径必须在录制
//! 之前先有覆盖（`tests/db_session_fixture_selfcheck.rs`）。
//!
//! 同级模块经 `crate::` 引用：bin 根与测试根都按同名声明这三个模块，同一份
//! 代码在两个 crate 里都能编。

use aios_database::data_interface::db_file_header::snapshot_presence;
use anyhow::{Context, ensure};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::archive_util::{extract_single_declared_file, sha256_file, write_archive};
use crate::format::{
    ArchiveSpec, COMPRESSION, FORMAT, FinalSnapshotSpec, MAX_ARCHIVE_BYTES, Recording,
    SessionFixtureManifest, SessionSnapshotSpec, SnapshotProbe, apply_patches, diff_patches,
    plan_cases, trim_cut_to, validate_relative_path,
};
use crate::session_cut::{PAGE_SIZE, session_chain, write_snapshot};

/// 一个 DB 文件的会话链概览（录制脚本按它取 baseline 与逐步核对水位推进）。
#[derive(Debug, serde::Serialize)]
pub struct ChainReport {
    pub latest_sesno: u32,
    pub sesnos: Vec<u32>,
}

/// 只读探查会话链。录制期间每执行一个宏就调一次，用「sesno 恰好 +1」把
/// 「每阶段恰好一个 SAVEWORK」的宏纪律当场验掉——事后再发现错位就得重录。
pub fn inspect(source: &Path) -> anyhow::Result<ChainReport> {
    let bytes = fs::read(source).with_context(|| format!("read source {}", source.display()))?;
    let chain = session_chain(&bytes)?;
    Ok(ChainReport {
        latest_sesno: chain.latest_sesno,
        sesnos: chain.cuts.keys().copied().collect(),
    })
}

pub fn pack(
    cli_source: Option<&Path>,
    recording_path: &Path,
    out: &Path,
    cli_dbnum: Option<u32>,
    force: bool,
) -> anyhow::Result<VerifyReport> {
    let recording: Recording = serde_json::from_slice(
        &fs::read(recording_path)
            .with_context(|| format!("read recording {}", recording_path.display()))?,
    )
    .with_context(|| format!("parse recording {}", recording_path.display()))?;
    if let Some(dbnum) = cli_dbnum {
        ensure!(
            dbnum == recording.dbnum,
            "--dbnum {dbnum} 与 recording.dbnum {} 不一致",
            recording.dbnum
        );
    }
    let source_path = match (cli_source, recording.source.as_deref()) {
        (Some(path), _) => path.to_path_buf(),
        (None, Some(path)) => PathBuf::from(path),
        (None, None) => anyhow::bail!("--source 或 recording.source 至少要给一个"),
    };
    let plan = plan_cases(recording.baseline_sesno, &recording.cases)?;

    // 录制期拷下的真文件：按各自头里的 sesno 归位，必须落在链的范围里（拷错腿就在这里红）。
    let mut real_snapshots: BTreeMap<u32, PathBuf> = BTreeMap::new();
    for snapshot in &recording.snapshots {
        let path = PathBuf::from(snapshot);
        let bytes = fs::read(&path).with_context(|| format!("read snapshot {}", path.display()))?;
        let sesno = session_chain(&bytes)
            .with_context(|| format!("snapshot {} 不是能读的 DB 文件", path.display()))?
            .latest_sesno;
        ensure!(
            sesno >= recording.baseline_sesno && sesno <= plan.final_sesno,
            "快照 {} 的 sesno={sesno} 不在链的范围 {}..={} 里",
            path.display(),
            recording.baseline_sesno,
            plan.final_sesno
        );
        ensure!(
            real_snapshots.insert(sesno, path.clone()).is_none(),
            "sesno={sesno} 有两份快照（{}）",
            path.display()
        );
    }
    // 台账 = 案例推导的关键 sesno ∪ 录制时拷了真文件的 sesno（回放要逐步走，每一步都要忠实）。
    let ledger: BTreeSet<u32> = plan
        .ledger
        .iter()
        .copied()
        .chain(real_snapshots.keys().copied())
        .collect();

    let source_bytes =
        fs::read(&source_path).with_context(|| format!("read source {}", source_path.display()))?;
    let source_chain = session_chain(&source_bytes)?;
    let file_name = source_path
        .file_name()
        .with_context(|| format!("source 路径没有文件名：{}", source_path.display()))?
        .to_string_lossy()
        .into_owned();
    let final_entry = format!("final/{file_name}");
    validate_relative_path(&final_entry)?;

    // 落位目录的父级先就绪，staging 建在同一卷上（rename 不跨卷）。
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }
    let stage = tempfile::Builder::new()
        .prefix(".session-fixture-stage-")
        .tempdir_in(parent.unwrap_or_else(|| Path::new(".")))?;

    // 最终文件冻结在最后一个案例窗口末：源文件之后的会话（录制后有人继续改库）
    // 一律裁掉，夹具不随源漂移。
    let final_path = stage.path().join("final").join(&file_name);
    write_snapshot(
        &source_bytes,
        source_chain.cut_for(plan.final_sesno)?,
        &final_path,
    )?;
    let final_bytes = fs::read(&final_path)?;
    let final_chain = session_chain(&final_bytes)?;
    ensure!(
        final_chain.latest_sesno == plan.final_sesno,
        "最终快照头指针 sesno={} 应为 {}",
        final_chain.latest_sesno,
        plan.final_sesno
    );
    for &sesno in &plan.ledger {
        ensure!(
            final_chain.contains(sesno),
            "最终快照会话链缺台账要求的 sesno={sesno}"
        );
    }
    for &sesno in real_snapshots.keys() {
        ensure!(
            final_chain.contains(sesno),
            "最终快照会话链缺录制快照的 sesno={sesno}"
        );
    }

    // 台账切割一律从**最终文件**切：verify 与回归将来就是这么切的，
    // 台账散列必须是它们能复现的那一份。有真快照的 sesno 再把「真 − 切」折成补丁打上，
    // 台账记的是打完的那份。逐切过验证闸（sesno + 存在性探针）。
    let mut session_snapshots = Vec::new();
    let mut patched_sesnos = Vec::new();
    for &sesno in &ledger {
        let cut_path = stage.path().join("cuts").join(format!("sesno-{sesno:03}"));
        write_snapshot(&final_bytes, final_chain.cut_for(sesno)?, &cut_path)?;
        let patches = match real_snapshots.get(&sesno) {
            Some(real_path) => {
                let full_cut = fs::read(&cut_path)?;
                let real_bytes = fs::read(real_path)?;
                let mut cut_bytes = trim_cut_to(&full_cut, real_bytes.len(), PAGE_SIZE)
                    .with_context(|| format!("sesno={sesno} 快照 {}", real_path.display()))?
                    .to_vec();
                let patches = diff_patches(&cut_bytes, &real_bytes, PAGE_SIZE)
                    .with_context(|| format!("sesno={sesno} 快照 {}", real_path.display()))?;
                apply_patches(&mut cut_bytes, &patches, PAGE_SIZE)?;
                ensure!(
                    cut_bytes == real_bytes,
                    "sesno={sesno} cut + 补丁没有还原成快照（内部错误）"
                );
                fs::write(&cut_path, &cut_bytes)?;
                if !patches.is_empty() {
                    patched_sesnos.push(sesno);
                }
                patches
            }
            None => Vec::new(),
        };
        let probes = plan.probes.get(&sesno).map_or(&[][..], Vec::as_slice);
        let observed = probe_snapshot(&cut_path, probes)?;
        ensure!(
            observed == sesno,
            "sesno={sesno} 的切割快照打开后 sesno={observed}"
        );
        session_snapshots.push(SessionSnapshotSpec {
            sesno,
            bytes: fs::metadata(&cut_path)?.len(),
            sha256: sha256_file(&cut_path)?,
            patches,
        });
    }
    if !patched_sesnos.is_empty() {
        println!("带字节补丁的台账 sesno（真文件 ≠ 终稿现切）：{patched_sesnos:?}");
    }

    let archive_name = format!(
        "db{}-sesno{}-{}.zip",
        recording.dbnum, recording.baseline_sesno, plan.final_sesno
    );
    let archive_path = stage.path().join(&archive_name);
    write_archive(
        &archive_path,
        &[(final_entry.as_str(), final_path.as_path())],
    )?;
    let archive_bytes = fs::metadata(&archive_path)?.len();
    ensure!(
        archive_bytes <= MAX_ARCHIVE_BYTES,
        "zip {archive_bytes} 字节超出 {MAX_ARCHIVE_BYTES} 预算——按方案 §5 拆批次或评估 LFS"
    );

    let manifest = SessionFixtureManifest {
        format: FORMAT.to_owned(),
        dbnum: recording.dbnum,
        baseline_sesno: recording.baseline_sesno,
        archive: ArchiveSpec {
            path: archive_name.clone(),
            bytes: archive_bytes,
            sha256: sha256_file(&archive_path)?,
            compression: COMPRESSION.to_owned(),
            max_bytes: MAX_ARCHIVE_BYTES,
        },
        final_snapshot: FinalSnapshotSpec {
            path: final_entry,
            sesno: plan.final_sesno,
            bytes: fs::metadata(&final_path)?.len(),
            sha256: sha256_file(&final_path)?,
        },
        session_snapshots,
        cases: recording.cases.clone(),
    };
    fs::write(
        stage.path().join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    let mut sums = format!("{}  {}\n", manifest.archive.sha256, manifest.archive.path);
    sums.push_str(&format!(
        "{}  {}\n",
        manifest.final_snapshot.sha256, manifest.final_snapshot.path
    ));
    for snapshot in &manifest.session_snapshots {
        sums.push_str(&format!(
            "{}  sesno-{:03}（运行时现切{}，不入库）\n",
            snapshot.sha256,
            snapshot.sesno,
            if snapshot.patches.is_empty() {
                ""
            } else {
                " + 字节补丁"
            }
        ));
    }
    fs::write(stage.path().join("SHA256SUMS"), sums)?;
    // 中间产物不入库：历史切割与最终文件明文只活在 staging（zip 已含最终文件）。
    fs::remove_dir_all(stage.path().join("cuts"))?;
    fs::remove_dir_all(stage.path().join("final"))?;

    if out.exists() {
        ensure!(force, "{} 已存在；用 --force 覆盖", out.display());
        let looks_like_fixture = out.join("manifest.json").is_file();
        let is_empty_dir = out.is_dir() && out.read_dir()?.next().is_none();
        ensure!(
            looks_like_fixture || is_empty_dir,
            "--force 只覆盖夹具目录（含 manifest.json）或空目录，拒绝删除 {}",
            out.display()
        );
        fs::remove_dir_all(out)?;
    }
    let staged = stage.keep();
    fs::rename(&staged, out)
        .with_context(|| format!("move staging {} -> {}", staged.display(), out.display()))?;

    // 端到端复验刚写出的目录：pack 的产物必须当场通过 verify 的全部裁决。
    verify_fixture(out)
}

#[derive(Debug)]
pub struct VerifyReport {
    pub dbnum: u32,
    pub final_sesno: u32,
    pub ledger_len: usize,
    pub probes_checked: usize,
    pub cases: usize,
}

impl VerifyReport {
    pub fn print(&self, root: &Path) {
        println!(
            "fixture={} dbnum={} final_sesno={} 台账现切对账={} 存在性探针={} 案例={}",
            root.display(),
            self.dbnum,
            self.final_sesno,
            self.ledger_len,
            self.probes_checked,
            self.cases
        );
    }
}

/// 夹具复验主体（pack 收尾与 `verify` 子命令共用）。
///
/// 顺序即证明链：档案完整性 → 最终文件受控解出 → 会话链在场 → 逐台账 sesno
/// **现切** → 大小/SHA256 与台账相等（性质 g「任意历史可精确还原」）→
/// sesno + 存在性验证闸（方案 §1 第 4 条）。
pub fn verify_fixture(root: &Path) -> anyhow::Result<VerifyReport> {
    let manifest_path = root.join("manifest.json");
    let manifest: SessionFixtureManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .with_context(|| format!("read manifest {}", manifest_path.display()))?,
    )
    .with_context(|| format!("parse manifest {}", manifest_path.display()))?;
    ensure!(
        manifest.format == FORMAT,
        "manifest format {:?} 不是 {FORMAT:?}",
        manifest.format
    );
    ensure!(
        manifest.archive.compression == COMPRESSION,
        "压缩口径 {:?} 不是 {COMPRESSION:?}",
        manifest.archive.compression
    );
    validate_relative_path(&manifest.archive.path)?;
    let archive_path = root.join(&manifest.archive.path);
    let archive_bytes = fs::metadata(&archive_path)
        .with_context(|| format!("stat archive {}", archive_path.display()))?
        .len();
    ensure!(
        archive_bytes == manifest.archive.bytes,
        "zip 大小 {archive_bytes} 与台账 {} 不一致",
        manifest.archive.bytes
    );
    ensure!(
        archive_bytes <= manifest.archive.max_bytes,
        "zip 大小 {archive_bytes} 超出台账预算 {}",
        manifest.archive.max_bytes
    );
    let archive_digest = sha256_file(&archive_path)?;
    ensure!(
        archive_digest == manifest.archive.sha256,
        "zip SHA256 {archive_digest} 与台账 {} 不一致",
        manifest.archive.sha256
    );

    // 用 manifest 里的案例重推执行计划：台账 sesno 集合必须与案例推导一致，
    // 手工篡改台账（多切/漏切）在这里就红，不用等到切割阶段。
    let plan = plan_cases(manifest.baseline_sesno, &manifest.cases)?;
    ensure!(
        plan.final_sesno == manifest.final_snapshot.sesno,
        "final sesno 台账 {} 与案例推导 {} 不一致",
        manifest.final_snapshot.sesno,
        plan.final_sesno
    );
    let recorded: BTreeSet<u32> = manifest
        .session_snapshots
        .iter()
        .map(|snapshot| snapshot.sesno)
        .collect();
    ensure!(
        recorded.len() == manifest.session_snapshots.len(),
        "还原台账里有重复 sesno"
    );
    // 案例推导的关键 sesno 必须都在台账里；台账里多出来的只能是录制时拷了真文件的 sesno（带字节补丁，或与终稿现切恰好相同）。
    ensure!(
        plan.ledger.is_subset(&recorded),
        "还原台账 sesno 集合 {recorded:?} 缺案例推导的 {:?}",
        plan.ledger.difference(&recorded).collect::<Vec<_>>()
    );

    let temp = tempfile::Builder::new()
        .prefix("aios-session-fixture-verify-")
        .tempdir()?;
    let final_path = temp.path().join("final_db");
    extract_single_declared_file(
        &archive_path,
        &manifest.final_snapshot.path,
        manifest.final_snapshot.bytes,
        &manifest.final_snapshot.sha256,
        &final_path,
    )?;
    let final_bytes = fs::read(&final_path)?;
    let chain = session_chain(&final_bytes)?;
    ensure!(
        chain.latest_sesno == manifest.final_snapshot.sesno,
        "最终文件头指针 sesno={} 与台账 {} 不一致",
        chain.latest_sesno,
        manifest.final_snapshot.sesno
    );

    let mut probes_checked = 0usize;
    for spec in &manifest.session_snapshots {
        let cut_path = temp.path().join(format!("sesno-{:03}", spec.sesno));
        write_snapshot(&final_bytes, chain.cut_for(spec.sesno)?, &cut_path)?;
        if !spec.patches.is_empty() || fs::metadata(&cut_path)?.len() != spec.bytes {
            let full_cut = fs::read(&cut_path)?;
            let mut bytes = trim_cut_to(&full_cut, spec.bytes as usize, PAGE_SIZE)
                .with_context(|| format!("sesno={} 台账长度对齐", spec.sesno))?
                .to_vec();
            apply_patches(&mut bytes, &spec.patches, PAGE_SIZE)
                .with_context(|| format!("sesno={} 打字节补丁", spec.sesno))?;
            fs::write(&cut_path, &bytes)?;
        }
        let cut_bytes = fs::metadata(&cut_path)?.len();
        ensure!(
            cut_bytes == spec.bytes,
            "sesno={} 现切大小 {cut_bytes} 与台账 {} 不一致",
            spec.sesno,
            spec.bytes
        );
        let digest = sha256_file(&cut_path)?;
        ensure!(
            digest == spec.sha256,
            "sesno={} 现切 SHA256 {digest} 与台账 {} 不一致——历史还原对账失败",
            spec.sesno,
            spec.sha256
        );
        let probes = plan.probes.get(&spec.sesno).map_or(&[][..], Vec::as_slice);
        let observed = probe_snapshot(&cut_path, probes)?;
        ensure!(
            observed == spec.sesno,
            "sesno={} 的现切快照打开后 sesno={observed}",
            spec.sesno
        );
        probes_checked += probes.len();
    }
    // 反空转：plan_cases 保证每案例至少一条探针，全程零探针说明计划被绕过了。
    ensure!(probes_checked > 0, "复验没有执行任何存在性探针");

    Ok(VerifyReport {
        dbnum: manifest.dbnum,
        final_sesno: manifest.final_snapshot.sesno,
        ledger_len: manifest.session_snapshots.len(),
        probes_checked,
        cases: manifest.cases.len(),
    })
}

/// 验证闸（方案 §1 第 4 条）：打开切割快照读回 sesno，并逐条核对存在性探针（e3d-io 活索引）。
fn probe_snapshot(path: &Path, probes: &[SnapshotProbe]) -> anyhow::Result<u32> {
    let refnos: Vec<_> = probes.iter().map(|probe| probe.refno).collect();
    let found = snapshot_presence(path, &refnos)
        .with_context(|| format!("open cut snapshot {}", path.display()))?;
    let sesno = found.sesno;
    for probe in probes {
        let present = found.contains(&probe.refno);
        ensure!(
            present == probe.present,
            "案例 {} 的元素 {}（{}）在 sesno={sesno} 应当{}，实际{}",
            probe.case_id,
            probe.refno_text,
            probe.noun,
            if probe.present { "存在" } else { "不存在" },
            if present { "存在" } else { "不存在" }
        );
    }
    Ok(sesno)
}
