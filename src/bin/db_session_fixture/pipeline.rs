//! 夹具管线主体：`pack`（录制产物 → 夹具目录）与 `verify_fixture`（离线复验）。
//!
//! 放在模块里而不是 bin 根，是为了让集成测试能直接调用同一份实现——阶段二是
//! 一次性 E3D 录制，pack 出问题就要再占一个生产空窗重录，这条路径必须在录制
//! 之前先有覆盖（`tests/db_session_fixture_selfcheck.rs`）。
//!
//! 同级模块经 `crate::` 引用：bin 根与测试根都按同名声明这三个模块，同一份
//! 代码在两个 crate 里都能编。

use anyhow::{Context, ensure};
use parse_pdms_db::paged::PagedDbSession;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::archive_util::{extract_single_declared_file, sha256_file, write_archive};
use crate::format::{
    ArchiveSpec, COMPRESSION, FORMAT, FinalSnapshotSpec, MAX_ARCHIVE_BYTES, Recording,
    SessionFixtureManifest, SessionSnapshotSpec, SnapshotProbe, plan_cases, validate_relative_path,
};
use crate::session_cut::{session_chain, write_snapshot};

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

    // 台账切割一律从**最终文件**切：verify 与回归将来就是这么切的，
    // 台账散列必须是它们能复现的那一份。逐切过验证闸（sesno + 存在性探针）。
    let mut session_snapshots = Vec::new();
    for &sesno in &plan.ledger {
        let cut_path = stage.path().join("cuts").join(format!("sesno-{sesno:03}"));
        write_snapshot(&final_bytes, final_chain.cut_for(sesno)?, &cut_path)?;
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
        });
    }

    let archive_name = format!(
        "db{}-sesno{}-{}.zip",
        recording.dbnum, recording.baseline_sesno, plan.final_sesno
    );
    let archive_path = stage.path().join(&archive_name);
    write_archive(&archive_path, &[(final_entry.as_str(), final_path.as_path())])?;
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
            "{}  sesno-{:03}（运行时现切，不入库）\n",
            snapshot.sha256, snapshot.sesno
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
    ensure!(
        recorded == plan.ledger,
        "还原台账 sesno 集合 {recorded:?} 与案例推导 {:?} 不一致",
        plan.ledger
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

/// 验证闸（方案 §1 第 4 条）：打开切割快照读回 sesno，并逐条核对存在性探针。
fn probe_snapshot(path: &Path, probes: &[SnapshotProbe]) -> anyhow::Result<u32> {
    let mut db = PagedDbSession::open(path)
        .with_context(|| format!("open cut snapshot {}", path.display()))?;
    let sesno = db.snapshot().sesno;
    if probes.is_empty() {
        return Ok(sesno);
    }
    let refnos: Vec<_> = probes.iter().map(|probe| probe.refno).collect();
    let found = db.read_raw_records(&refnos)?;
    for probe in probes {
        let present = found.contains_key(&probe.refno);
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
