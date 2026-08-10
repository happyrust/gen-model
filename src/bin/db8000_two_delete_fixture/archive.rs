use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const ARCHIVE_NAME: &str = "db8000-sesno24-26.zip";
pub const MAX_ARCHIVE_BYTES: u64 = 6 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureManifest {
    pub format: String,
    pub issue: IssueSpec,
    pub dbnum: u32,
    pub window: WindowSpec,
    pub refs: RefSpec,
    pub archive: ArchiveSpec,
    pub snapshots: Vec<SnapshotSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueSpec {
    pub id: u32,
    pub title: String,
    pub slug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSpec {
    pub baseline_sesno: u32,
    pub start_sesno: u32,
    pub end_sesno: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefSpec {
    pub zone: String,
    pub parent_equi: String,
    pub child: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveSpec {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub compression: String,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSpec {
    pub role: String,
    pub sesno: u32,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub elements: Vec<ElementState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementState {
    pub refno: String,
    pub noun: String,
    pub present: bool,
}

pub struct ExtractedFixture {
    _temp: TempDir,
    pub root: PathBuf,
    pub manifest: FixtureManifest,
}

impl ExtractedFixture {
    pub fn path_for_role(&self, role: &str) -> anyhow::Result<PathBuf> {
        let snapshot = self
            .manifest
            .snapshots
            .iter()
            .find(|snapshot| snapshot.role == role)
            .with_context(|| format!("manifest is missing snapshot role {role}"))?;
        Ok(self.root.join(&snapshot.path))
    }
}

pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn write_archive(path: &Path, entries: &[(&str, &Path)]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9));
    let mut buffer = [0u8; 128 * 1024];

    for (archive_path, source_path) in entries {
        validate_relative_path(archive_path)?;
        writer.start_file(*archive_path, options)?;
        let mut source =
            File::open(source_path).with_context(|| format!("open {}", source_path.display()))?;
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            writer.write_all(&buffer[..read])?;
        }
    }
    writer.finish()?.sync_all()?;
    Ok(())
}

pub fn verify_and_extract(fixture_root: &Path) -> anyhow::Result<ExtractedFixture> {
    let manifest_path = fixture_root.join("manifest.json");
    let manifest: FixtureManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .with_context(|| format!("read manifest {}", manifest_path.display()))?,
    )?;
    ensure!(manifest.format == "aios-issue-fixture-v1");
    ensure!(manifest.issue.id == 19);
    ensure!(manifest.dbnum == 8000);
    ensure!(manifest.archive.max_bytes == MAX_ARCHIVE_BYTES);
    validate_relative_path(&manifest.archive.path)?;

    let archive_path = fixture_root.join(&manifest.archive.path);
    let metadata = fs::metadata(&archive_path)
        .with_context(|| format!("stat archive {}", archive_path.display()))?;
    ensure!(metadata.len() == manifest.archive.bytes);
    ensure!(metadata.len() < manifest.archive.max_bytes);
    ensure!(sha256_file(&archive_path)? == manifest.archive.sha256);

    let expected: HashMap<&str, &SnapshotSpec> = manifest
        .snapshots
        .iter()
        .map(|snapshot| (snapshot.path.as_str(), snapshot))
        .collect();
    ensure!(expected.len() == manifest.snapshots.len());
    ensure!(expected.len() == 3);
    for path in expected.keys() {
        validate_relative_path(path)?;
    }

    let temp = tempfile::Builder::new()
        .prefix("aios-issue019-")
        .tempdir()?;
    let root = temp.path().to_path_buf();
    let file = File::open(&archive_path)?;
    let mut archive = ZipArchive::new(file)?;
    ensure!(archive.len() == expected.len());
    let mut seen = BTreeSet::new();

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().replace('\\', "/");
        validate_relative_path(&name)?;
        ensure!(!entry.is_dir(), "unexpected directory entry {name}");
        let spec = expected
            .get(name.as_str())
            .with_context(|| format!("undeclared archive entry {name}"))?;
        ensure!(seen.insert(name.clone()), "duplicate archive entry {name}");
        ensure!(entry.size() == spec.bytes, "size mismatch for {name}");

        let output = root.join(&name);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut target = File::create(&output)?;
        std::io::copy(&mut entry, &mut target)?;
        target.sync_all()?;
        ensure!(
            sha256_file(&output)? == spec.sha256,
            "hash mismatch for {name}"
        );
    }

    ensure!(seen.len() == expected.len());
    Ok(ExtractedFixture {
        _temp: temp,
        root,
        manifest,
    })
}

fn validate_relative_path(path: &str) -> anyhow::Result<()> {
    ensure!(!path.is_empty(), "empty archive path");
    let path = Path::new(path);
    ensure!(
        !path.is_absolute(),
        "absolute archive path: {}",
        path.display()
    );
    for component in path.components() {
        ensure!(
            matches!(component, Component::Normal(_)),
            "unsafe archive path: {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_relative_path;

    #[test]
    fn archive_paths_must_stay_relative_and_normal() {
        assert!(validate_relative_path("sesno-024-baseline/ams8000_0001").is_ok());
        for unsafe_path in ["", "../escape", "safe/../../escape", "/absolute"] {
            assert!(
                validate_relative_path(unsafe_path).is_err(),
                "accepted unsafe path {unsafe_path}"
            );
        }
        #[cfg(windows)]
        assert!(validate_relative_path(r"C:\escape").is_err());
    }
}
