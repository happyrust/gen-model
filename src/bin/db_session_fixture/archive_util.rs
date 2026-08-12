//! zip 打包 / SHA256 / 受控解压（只认清单里声明过的条目）。

use anyhow::{Context, ensure};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::format::validate_relative_path;

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

/// 从 zip 里解出唯一声明的条目并校验大小与 SHA256。
///
/// v1 格式的 zip 只含最终文件一个条目；多余、缺失、路径穿越都拒绝。
pub fn extract_single_declared_file(
    archive_path: &Path,
    entry_name: &str,
    expected_bytes: u64,
    expected_sha256: &str,
    output: &Path,
) -> anyhow::Result<()> {
    validate_relative_path(entry_name)?;
    let file =
        File::open(archive_path).with_context(|| format!("open {}", archive_path.display()))?;
    let mut archive = ZipArchive::new(file)?;
    ensure!(
        archive.len() == 1,
        "zip 应只含最终文件一个条目，实际 {} 个",
        archive.len()
    );
    let mut entry = archive.by_index(0)?;
    let name = entry.name().replace('\\', "/");
    validate_relative_path(&name)?;
    ensure!(!entry.is_dir(), "zip 条目不应是目录：{name}");
    ensure!(
        name == entry_name,
        "zip 条目 {name} 与清单声明 {entry_name} 不一致"
    );
    ensure!(
        entry.size() == expected_bytes,
        "zip 条目 {name} 大小 {} 与清单 {expected_bytes} 不一致",
        entry.size()
    );

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut target = File::create(output)?;
    std::io::copy(&mut entry, &mut target)?;
    target.sync_all()?;
    let actual = sha256_file(output)?;
    ensure!(
        actual == expected_sha256,
        "zip 条目 {name} 解出后 SHA256 不匹配：{actual} != {expected_sha256}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_single_entry_archive() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("payload.bin");
        fs::write(&source, b"session fixture payload").unwrap();
        let sha = sha256_file(&source).unwrap();

        let zip_path = dir.path().join("bundle.zip");
        write_archive(&zip_path, &[("final/payload.bin", source.as_path())]).unwrap();

        let out = dir.path().join("extracted.bin");
        extract_single_declared_file(&zip_path, "final/payload.bin", 23, &sha, &out).unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"session fixture payload");

        // 声明名不符、大小不符、哈希不符都必须拒绝。
        assert!(
            extract_single_declared_file(&zip_path, "final/other.bin", 23, &sha, &out).is_err()
        );
        assert!(
            extract_single_declared_file(&zip_path, "final/payload.bin", 24, &sha, &out).is_err()
        );
        assert!(
            extract_single_declared_file(&zip_path, "final/payload.bin", 23, "00", &out).is_err()
        );
    }
}
