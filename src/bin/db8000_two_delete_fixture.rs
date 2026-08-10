//! Build the portable Issue #19 fixture from a real dbnum=8000 session chain.

#[path = "db8000_two_delete_fixture/archive.rs"]
mod archive;

use aios_core::pdms_types::RefU64;
use anyhow::{Context, ensure};
use archive::{
    ARCHIVE_NAME, ArchiveSpec, ElementState, FixtureManifest, IssueSpec, MAX_ARCHIVE_BYTES,
    RefSpec, SnapshotSpec, WindowSpec, sha256_file, verify_and_extract, write_archive,
};
use clap::Parser;
use parse_pdms_db::paged::PagedDbSession;
use pdms_io::io::PdmsIO;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

const PAGE_SIZE: usize = 0x800;
const HEADER_SESSION_PAGE_OFFSET: usize = 40;
const ISSUE_TITLE: &str = "跨会话父子删除被最终 OWNER 状态误判";
const ISSUE_SLUG: &str = "cross-session-parent-child-delete";
const BASELINE_SESNO: u32 = 24;
const CHILD_DELETE_SESNO: u32 = 25;
const PARENT_DELETE_SESNO: u32 = 26;
const ZONE_REFNO: &str = "24384/24775";
const PARENT_REFNO: &str = "24384/24778";
const CHILD_REFNO: &str = "24384/24779";

#[derive(Parser, Debug)]
#[command(about = "Build the compressed Issue #19 dbnum=8000 regression fixture")]
struct Args {
    #[arg(
        long,
        default_value = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001"
    )]
    source: PathBuf,
    #[arg(
        long,
        default_value = r"tests\fixtures\issues\issue-019-cross-session-parent-child-delete"
    )]
    out: PathBuf,
    #[arg(long, default_value_t = false)]
    force: bool,
}

#[derive(Debug, Clone, Copy)]
struct SessionCut {
    session_page: u32,
    latest_page: u32,
}

fn be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("four bytes"))
}

fn session_chain(bytes: &[u8]) -> anyhow::Result<HashMap<u32, SessionCut>> {
    ensure!(
        bytes.len() >= PAGE_SIZE,
        "PDMS file is smaller than one page"
    );
    ensure!(
        bytes.len().is_multiple_of(PAGE_SIZE),
        "PDMS file size is not page aligned: {}",
        bytes.len()
    );
    let mut page = be_u32(&bytes[HEADER_SESSION_PAGE_OFFSET..HEADER_SESSION_PAGE_OFFSET + 4]);
    let mut cuts = HashMap::new();
    let mut seen = HashSet::new();
    while page != 0 && page != u32::MAX && seen.insert(page) {
        let start = page as usize * PAGE_SIZE;
        ensure!(
            start + PAGE_SIZE <= bytes.len(),
            "session page {page} is outside the file"
        );
        let data = &bytes[start..start + PAGE_SIZE];
        let previous = be_u32(&data[4..8]);
        let sesno = be_u32(&data[12..16]);
        cuts.insert(
            sesno,
            SessionCut {
                session_page: page,
                latest_page: be_u32(&data[20..24]),
            },
        );
        page = previous;
    }
    Ok(cuts)
}

fn write_snapshot(source: &[u8], cut: SessionCut, path: &Path) -> anyhow::Result<()> {
    let end = (cut.latest_page as usize + 1) * PAGE_SIZE;
    ensure!(
        end <= source.len(),
        "snapshot end {end} exceeds source size {}",
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

fn require_presence(path: &Path, present: &[RefU64], absent: &[RefU64]) -> anyhow::Result<u32> {
    let mut db = PagedDbSession::open(path)
        .with_context(|| format!("open paged snapshot {}", path.display()))?;
    let sesno = db.snapshot().sesno;
    let mut all = Vec::with_capacity(present.len() + absent.len());
    all.extend_from_slice(present);
    all.extend_from_slice(absent);
    let found = db.read_raw_records(&all)?;
    for refno in present {
        ensure!(
            found.contains_key(refno),
            "{refno} must exist in {}",
            path.display()
        );
    }
    for refno in absent {
        ensure!(
            !found.contains_key(refno),
            "{refno} must be absent in {}",
            path.display()
        );
    }
    Ok(sesno)
}

fn snapshot_spec(
    role: &str,
    sesno: u32,
    archive_path: &str,
    file: &Path,
    elements: Vec<ElementState>,
) -> anyhow::Result<SnapshotSpec> {
    Ok(SnapshotSpec {
        role: role.to_owned(),
        sesno,
        path: archive_path.to_owned(),
        bytes: fs::metadata(file)?.len(),
        sha256: sha256_file(file)?,
        elements,
    })
}

fn element(refno: RefU64, noun: &str, present: bool) -> ElementState {
    ElementState {
        refno: refno.to_string(),
        noun: noun.to_owned(),
        present,
    }
}

fn noun_at(path: &Path, refno: RefU64) -> anyhow::Result<String> {
    let mut io = PdmsIO::new("issue019", path, true);
    io.open()?;
    Ok(io.auto_get_raw_element(refno)?.att_map().get_type())
}

fn write_text_assets(root: &Path, manifest: &FixtureManifest) -> anyhow::Result<()> {
    fs::write(
        root.join("README.md"),
        r#"# Issue #19：跨会话父子删除被最终 OWNER 状态误判

`db8000-sesno24-26.zip` 保存 dbnum 8000 的三个真实 session-chain 快照：

| 快照 | sesno | EQUI `24384/24778` | 子节点 `24384/24779` |
|---|---:|---|---|
| baseline | 24 | 存在 | 存在 |
| child-deleted | 25 | 存在 | 已删除 |
| parent-deleted | 26 | 已删除 | 已删除 |

运行回归：

```powershell
cargo test --test db8000_two_delete_fixture -- --ignored --nocapture
```

测试会验证 ZIP 与三个文件的 SHA256、安全解压，然后只使用 sesno 26 最终文件直接采集 `25..=26`。
"#,
    )?;

    let comparisons = root.join("comparisons");
    fs::create_dir_all(&comparisons)?;
    let session_25 = json!({
        "sesno": 25,
        "expected": [
            {"refno": "24384_24778", "operation": "modified", "noun": "EQUI", "attributes": ["CACHID"], "children": "member_changed"},
            {"refno": "24384_24779", "operation": "deleted"}
        ]
    });
    let session_26 = json!({
        "sesno": 26,
        "expected": [
            {"refno": "24384_24775", "operation": "modified", "noun": "ZONE", "attributes": [], "children": "member_changed"},
            {"refno": "24384_24778", "operation": "deleted"}
        ]
    });
    let before = json!({
        "issue": 19,
        "window": [25, 26],
        "status": "before_fix",
        "operation_count": 3,
        "operations": [
            {"sesno": 25, "refno": "24384_24778", "operation": "deleted", "incorrect": true},
            {"sesno": 26, "refno": "24384_24775", "operation": "modified", "noun": "ZONE"},
            {"sesno": 26, "refno": "24384_24778", "operation": "deleted"}
        ],
        "missing": {"sesno": 25, "refno": "24384_24779", "operation": "deleted"}
    });
    let after = json!({
        "issue": 19,
        "window": [25, 26],
        "status": "expected_after_fix",
        "operation_count": 4,
        "operations": [
            {"sesno": 25, "refno": "24384_24778", "operation": "modified", "noun": "EQUI"},
            {"sesno": 25, "refno": "24384_24779", "operation": "deleted"},
            {"sesno": 26, "refno": "24384_24775", "operation": "modified", "noun": "ZONE"},
            {"sesno": 26, "refno": "24384_24778", "operation": "deleted"}
        ]
    });
    for (name, value) in [
        ("expected-session-25.json", session_25),
        ("expected-session-26.json", session_26),
        ("observed-before-fix-window-25-26.json", before),
        ("expected-after-fix-window-25-26.json", after),
    ] {
        fs::write(comparisons.join(name), serde_json::to_vec_pretty(&value)?)?;
    }

    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(manifest)?,
    )?;
    let mut sums = format!("{}  {}\n", manifest.archive.sha256, manifest.archive.path);
    for snapshot in &manifest.snapshots {
        sums.push_str(&format!("{}  {}\n", snapshot.sha256, snapshot.path));
    }
    fs::write(root.join("SHA256SUMS"), sums)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.out.exists() {
        ensure!(
            args.force,
            "{} already exists; pass --force to replace it",
            args.out.display()
        );
    }

    let zone = RefU64::from_str(ZONE_REFNO).expect("valid Issue #19 ZONE refno");
    let parent = RefU64::from_str(PARENT_REFNO).expect("valid Issue #19 EQUI refno");
    let child = RefU64::from_str(CHILD_REFNO).expect("valid Issue #19 BOX refno");
    let source =
        fs::read(&args.source).with_context(|| format!("read source {}", args.source.display()))?;
    let cuts = session_chain(&source)?;
    let cut = |sesno| {
        cuts.get(&sesno)
            .copied()
            .with_context(|| format!("session {sesno} is missing"))
    };

    let raw = tempfile::Builder::new()
        .prefix("aios-issue019-raw-")
        .tempdir()?;
    let baseline_rel = "sesno-024-baseline/ams8000_0001";
    let child_rel = "sesno-025-child-deleted/ams8000_0001";
    let final_rel = "sesno-026-parent-deleted/ams8000_0001";
    let baseline_file = raw.path().join(baseline_rel);
    let child_file = raw.path().join(child_rel);
    let final_file = raw.path().join(final_rel);
    write_snapshot(&source, cut(BASELINE_SESNO)?, &baseline_file)?;
    write_snapshot(&source, cut(CHILD_DELETE_SESNO)?, &child_file)?;
    write_snapshot(&source, cut(PARENT_DELETE_SESNO)?, &final_file)?;

    ensure!(require_presence(&baseline_file, &[parent, child], &[])? == BASELINE_SESNO);
    ensure!(require_presence(&child_file, &[parent], &[child])? == CHILD_DELETE_SESNO);
    ensure!(require_presence(&final_file, &[], &[parent, child])? == PARENT_DELETE_SESNO);
    let zone_noun = noun_at(&baseline_file, zone)?;
    let parent_noun = noun_at(&baseline_file, parent)?;
    let child_noun = noun_at(&baseline_file, child)?;
    ensure!(zone_noun == "ZONE");
    ensure!(parent_noun == "EQUI");

    let parent_dir = args.out.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent_dir)?;
    let stage = tempfile::Builder::new()
        .prefix(".issue019-stage-")
        .tempdir_in(parent_dir)?;
    let archive_path = stage.path().join(ARCHIVE_NAME);
    write_archive(
        &archive_path,
        &[
            (baseline_rel, baseline_file.as_path()),
            (child_rel, child_file.as_path()),
            (final_rel, final_file.as_path()),
        ],
    )?;
    let archive_bytes = fs::metadata(&archive_path)?.len();
    ensure!(archive_bytes < MAX_ARCHIVE_BYTES);

    let manifest = FixtureManifest {
        format: "aios-issue-fixture-v1".to_owned(),
        issue: IssueSpec {
            id: 19,
            title: ISSUE_TITLE.to_owned(),
            slug: ISSUE_SLUG.to_owned(),
        },
        dbnum: 8000,
        window: WindowSpec {
            baseline_sesno: BASELINE_SESNO,
            start_sesno: CHILD_DELETE_SESNO,
            end_sesno: PARENT_DELETE_SESNO,
        },
        refs: RefSpec {
            zone: zone.to_string(),
            parent_equi: parent.to_string(),
            child: child.to_string(),
        },
        archive: ArchiveSpec {
            path: ARCHIVE_NAME.to_owned(),
            bytes: archive_bytes,
            sha256: sha256_file(&archive_path)?,
            compression: "zip-deflate-level-9".to_owned(),
            max_bytes: MAX_ARCHIVE_BYTES,
        },
        snapshots: vec![
            snapshot_spec(
                "baseline",
                BASELINE_SESNO,
                baseline_rel,
                &baseline_file,
                vec![
                    element(zone, &zone_noun, true),
                    element(parent, &parent_noun, true),
                    element(child, &child_noun, true),
                ],
            )?,
            snapshot_spec(
                "child_deleted",
                CHILD_DELETE_SESNO,
                child_rel,
                &child_file,
                vec![
                    element(zone, &zone_noun, true),
                    element(parent, &parent_noun, true),
                    element(child, &child_noun, false),
                ],
            )?,
            snapshot_spec(
                "parent_deleted",
                PARENT_DELETE_SESNO,
                final_rel,
                &final_file,
                vec![
                    element(zone, &zone_noun, true),
                    element(parent, &parent_noun, false),
                    element(child, &child_noun, false),
                ],
            )?,
        ],
    };
    write_text_assets(stage.path(), &manifest)?;
    let verified = verify_and_extract(stage.path())?;
    ensure!(verified.path_for_role("baseline")?.is_file());
    ensure!(verified.path_for_role("child_deleted")?.is_file());
    ensure!(verified.path_for_role("parent_deleted")?.is_file());

    if args.out.exists() {
        let resolved_parent = fs::canonicalize(parent_dir)?;
        let resolved_output = fs::canonicalize(&args.out)?;
        ensure!(resolved_output.starts_with(&resolved_parent));
        for name in [ARCHIVE_NAME, "README.md", "SHA256SUMS", "manifest.json"] {
            let current = args.out.join(name);
            if current.exists() {
                fs::remove_file(&current)?;
            }
            fs::rename(stage.path().join(name), current)?;
        }
        let comparisons = args.out.join("comparisons");
        if comparisons.exists() {
            fs::remove_dir_all(&comparisons)?;
        }
        fs::rename(stage.path().join("comparisons"), comparisons)?;
    } else {
        let staged_path = stage.keep();
        fs::rename(&staged_path, &args.out)?;
    }

    println!("fixture={}", args.out.display());
    println!(
        "archive={} bytes={archive_bytes}",
        args.out.join(ARCHIVE_NAME).display()
    );
    println!("window={CHILD_DELETE_SESNO}..={PARENT_DELETE_SESNO}");
    println!("operations: {} Deleted -> {} Deleted", child, parent);
    Ok(())
}
