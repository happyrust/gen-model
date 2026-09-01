//! `e3d-model` 生产持久化使用的稳定身份和内容寻址 mesh 文件存储。

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use aios_core::shape::pdms_shape::PlantMesh;
use anyhow::{Context, bail};
use e3d_io::refno::RefNo;
use e3d_model::elmodl::GeometryId;
use e3d_model::primitive_instance::PrimitiveMeshKey;
use sha2::{Digest, Sha256};

const BAKED_MESH_DOMAIN: &[u8] = b"e3d-model/baked-mesh/v2\0";
const TESSELLATION_POLICY_VERSION: &[u8] = b"e3d-model-tessellation/v1\0";
static TEMP_MESH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshWrite {
    Written,
    Reused,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct E3dPersistReport {
    pub generation_report: serde_json::Value,
    pub upserted: usize,
    pub removed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub shared_instances: usize,
    pub baked_instances: usize,
    pub unique_meshes: usize,
    pub mesh_reused: usize,
    pub mesh_written: usize,
    #[serde(skip)]
    pub(crate) mesh_ids: std::collections::BTreeSet<String>,
}

pub fn baked_mesh_id(mesh: &PlantMesh) -> String {
    let mut hash = Sha256::new();
    hash.update(BAKED_MESH_DOMAIN);
    hash.update(TESSELLATION_POLICY_VERSION);
    update_mesh_digest(&mut hash, mesh);
    format!("e3d_baked_v2_{}", hex::encode(hash.finalize()))
}

pub fn canonical_mesh_id(key: PrimitiveMeshKey, mesh: &PlantMesh) -> String {
    let mut hash = Sha256::new();
    hash.update(b"e3d-model/primitive-inst-geo/v1\0");
    hash.update(serde_json::to_vec(&key).expect("PrimitiveMeshKey JSON"));
    update_mesh_digest(&mut hash, mesh);
    let bytes: [u8; 8] = hash.finalize()[..8].try_into().expect("sha256 prefix");
    u64::from_be_bytes(bytes).max(1).to_string()
}

pub fn geometry_record_id(geometry_id: &GeometryId, source_refno: RefNo) -> String {
    match geometry_id {
        GeometryId::Element { .. } => format!("{}_{}", source_refno.word0, source_refno.word1),
        GeometryId::ImpliedTube { .. } => format!(
            "derived_{}",
            hex::encode(Sha256::digest(
                serde_json::to_vec(geometry_id).expect("GeometryId JSON")
            ))
        ),
    }
}

pub fn ensure_mesh_file(path: &Path, mesh: &PlantMesh) -> anyhow::Result<MeshWrite> {
    if path.exists() {
        verify_mesh_file(path, mesh)?;
        return Ok(MeshWrite::Reused);
    }

    let parent = path.parent().context("mesh 路径没有父目录")?;
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("mesh"),
        std::process::id(),
        TEMP_MESH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    mesh.ser_to_file(&temp)?;
    verify_mesh_file(&temp, mesh)?;
    // `rename` overwrites an existing destination on Windows, so it is not a
    // create-if-absent primitive there. A same-directory hard-link publishes
    // the fully closed inode atomically without replacing a concurrent winner;
    // removing the private temp name leaves exactly the requested final file.
    match std::fs::hard_link(&temp, path) {
        Ok(()) => {
            std::fs::remove_file(&temp)?;
            Ok(MeshWrite::Written)
        }
        Err(error) if path.exists() => {
            let _ = std::fs::remove_file(&temp);
            verify_mesh_file(path, mesh)
                .with_context(|| format!("{} 并发写入胜者校验失败: {error}", path.display()))?;
            Ok(MeshWrite::Reused)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error).with_context(|| format!("发布 {}", path.display()))
        }
    }
}

fn update_mesh_digest(hash: &mut Sha256, mesh: &PlantMesh) {
    hash.update((mesh.vertices.len() as u64).to_le_bytes());
    for vertex in &mesh.vertices {
        for value in vertex.to_array() {
            hash.update(value.to_le_bytes());
        }
    }
    hash.update((mesh.normals.len() as u64).to_le_bytes());
    for normal in &mesh.normals {
        for value in normal.to_array() {
            hash.update(value.to_le_bytes());
        }
    }
    hash.update((mesh.indices.len() as u64).to_le_bytes());
    for index in &mesh.indices {
        hash.update(index.to_le_bytes());
    }
}

fn verify_mesh_file(path: &Path, expected: &PlantMesh) -> anyhow::Result<()> {
    // The legacy rkyv reader panics on some truncated inputs. A damaged cache
    // entry is ordinary validation failure, not a reason to abort the process.
    let actual = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        PlantMesh::des_mesh_file(&path)
    }))
    .map_err(|_| anyhow::anyhow!("{} mesh 文件损坏", path.display()))??;
    if !vec3_buffers_bit_equal(&actual.vertices, &expected.vertices)
        || !vec3_buffers_bit_equal(&actual.normals, &expected.normals)
        || actual.indices != expected.indices
    {
        bail!(
            "{} 内容 ID 对应文件的 vertices/normals/indices 不一致",
            path.display()
        );
    }
    Ok(())
}

fn vec3_buffers_bit_equal(actual: &[glam::Vec3], expected: &[glam::Vec3]) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(left, right)| {
            left.to_array().map(f32::to_bits) == right.to_array().map(f32::to_bits)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_mesh() -> PlantMesh {
        let solid =
            e3d_model::primitive_instance::canonical_primitive_mesh(PrimitiveMeshKey::BoxV1)
                .unwrap();
        crate::fast_model::manifold_csg::manifold_to_plant_mesh(&solid)
    }

    #[test]
    fn baked_id_covers_vertices_normals_and_indices_exactly() {
        let mesh = box_mesh();
        let original = baked_mesh_id(&mesh);

        let mut vertex = mesh.clone();
        vertex.vertices[0].x = f32::from_bits(vertex.vertices[0].x.to_bits() ^ 1);
        assert_ne!(original, baked_mesh_id(&vertex));

        let mut normal = mesh.clone();
        normal.normals[0].x = f32::from_bits(normal.normals[0].x.to_bits() ^ 1);
        assert_ne!(original, baked_mesh_id(&normal));

        let mut index = mesh.clone();
        index.indices.swap(0, 1);
        assert_ne!(original, baked_mesh_id(&index));
    }

    #[test]
    fn concurrent_publish_writes_once_and_every_writer_verifies_the_winner() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = box_mesh();
        let path = dir.path().join(format!("{}.mesh", baked_mesh_id(&mesh)));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let path = path.clone();
                let mesh = mesh.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    ensure_mesh_file(&path, &mesh).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let outcomes = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == MeshWrite::Written)
                .count(),
            1
        );
        assert_eq!(
            PlantMesh::des_mesh_file(&path).unwrap().indices,
            mesh.indices
        );
        assert!(dir.path().read_dir().unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn damaged_existing_content_addressed_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = box_mesh();
        let path = dir.path().join(format!("{}.mesh", baked_mesh_id(&mesh)));
        ensure_mesh_file(&path, &mesh).unwrap();
        std::fs::write(&path, b"damaged").unwrap();
        assert!(ensure_mesh_file(&path, &mesh).is_err());
    }

    #[test]
    fn cache_verification_uses_the_same_float_bit_identity_as_the_hash() {
        let dir = tempfile::tempdir().unwrap();
        let mut expected = box_mesh();
        expected.vertices[0].x = -0.0;
        let mut wrong_zero = expected.clone();
        wrong_zero.vertices[0].x = 0.0;
        let path = dir
            .path()
            .join(format!("{}.mesh", baked_mesh_id(&expected)));
        wrong_zero.ser_to_file(&path).unwrap();
        assert!(ensure_mesh_file(&path, &expected).is_err());

        let mut nan_mesh = box_mesh();
        nan_mesh.normals[0].x = f32::from_bits(0x7fc0_0123);
        let nan_path = dir
            .path()
            .join(format!("{}.mesh", baked_mesh_id(&nan_mesh)));
        assert_eq!(
            ensure_mesh_file(&nan_path, &nan_mesh).unwrap(),
            MeshWrite::Written
        );
        assert_eq!(
            ensure_mesh_file(&nan_path, &nan_mesh).unwrap(),
            MeshWrite::Reused
        );
    }
}
