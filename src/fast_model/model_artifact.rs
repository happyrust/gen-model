//! Source-bound e3d-model output on local disk. No database fallback.
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use e3d_io::refno::RefNo;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::model_memory_store::ProjectionCommit;

#[derive(Serialize, Deserialize)]
struct Artifact {
    version: u32,
    input: String,
    payload_sha256: String,
    payload: String,
}

fn path(directory: &Path, input: &str) -> PathBuf {
    let key = hex::encode(Sha256::digest(format!("model-artifact-v1:{input}")));
    directory.join(format!("{key}.json"))
}

/// A miss/corrupt artifact asks the caller to regenerate this root, never to
/// infer an empty model. Mesh files must be durable before storing a receipt.
pub(crate) fn load(
    directory: &Path,
    meshes: &Path,
    input: &str,
    root: RefNo,
) -> Option<ProjectionCommit> {
    let commit: ProjectionCommit = load_value(directory, &format!("root:{root}:{input}"))?;
    if commit.receipt.as_ref()?.0 != root
        || commit
            .upserts
            .iter()
            .any(|record| !meshes.join(format!("{}.mesh", record.mesh_id)).is_file())
    {
        return None;
    }
    Some(commit)
}

/// Shared checksum envelope for model output and source-derived room topology.
pub(crate) fn load_value<T: serde::de::DeserializeOwned>(
    directory: &Path,
    input: &str,
) -> Option<T> {
    let bytes = std::fs::read(path(directory, input)).ok()?;
    let artifact: Artifact = serde_json::from_slice(&bytes).ok()?;
    if artifact.version != 1
        || artifact.input != input
        || artifact.payload_sha256 != hex::encode(Sha256::digest(artifact.payload.as_bytes()))
    {
        return None;
    }
    serde_json::from_str(&artifact.payload).ok()
}

pub(crate) fn store(
    directory: &Path,
    input: &str,
    root: RefNo,
    commit: &ProjectionCommit,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        commit
            .receipt
            .as_ref()
            .is_some_and(|(actual, _)| *actual == root),
        "model artifact has no matching root receipt"
    );
    store_value(directory, &format!("root:{root}:{input}"), commit)
}

pub(crate) fn store_value<T: Serialize>(
    directory: &Path,
    input: &str,
    value: &T,
) -> anyhow::Result<()> {
    let payload = serde_json::to_string(value)?;
    let artifact = Artifact {
        version: 1,
        input: input.to_owned(),
        payload_sha256: hex::encode(Sha256::digest(payload.as_bytes())),
        payload,
    };
    std::fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut file, &artifact)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path(directory, input))
        .context("publish model artifact")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::model_memory_store::RootReceipt;

    #[test]
    fn empty_root_survives_restart_but_wrong_input_or_corruption_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = RefNo::new(7000, 1);
        let commit = ProjectionCommit {
            receipt: Some((
                root,
                RootReceipt {
                    source_sesno: 12,
                    model_target: None,
                    manifest_hash: "empty".into(),
                    geometry_count: 0,
                    published_via: "e3d-model".into(),
                    room: None,
                },
            )),
            ..Default::default()
        };
        store(
            dir.path(),
            "source-session-dependencies-rules",
            root,
            &commit,
        )
        .unwrap();
        for _ in 0..3 {
            let restored = load(
                dir.path(),
                dir.path(),
                "source-session-dependencies-rules",
                root,
            )
            .unwrap();
            assert!(restored.upserts.is_empty());
            assert_eq!(restored.receipt, commit.receipt);
        }
        assert!(load(dir.path(), dir.path(), "replacement-source", root).is_none());
        let cache_path = path(
            dir.path(),
            &format!("root:{root}:source-session-dependencies-rules"),
        );
        let mut artifact: Artifact =
            serde_json::from_slice(&std::fs::read(&cache_path).unwrap()).unwrap();
        artifact.payload.push(' ');
        std::fs::write(&cache_path, serde_json::to_vec(&artifact).unwrap()).unwrap();
        assert!(
            load(
                dir.path(),
                dir.path(),
                "source-session-dependencies-rules",
                root
            )
            .is_none()
        );
        store(
            dir.path(),
            "source-session-dependencies-rules",
            root,
            &commit,
        )
        .unwrap();
        assert!(
            load(
                dir.path(),
                dir.path(),
                "source-session-dependencies-rules",
                root
            )
            .is_some()
        );
    }
}
