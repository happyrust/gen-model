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
    /// 盘上已有同名文件，但读不出或与这份网格不符，已原子重写（ADR-087 D3）。
    Repaired,
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
    /// 盘上内容与身份不符、本次原子重写了的 mesh 数；每一个都另有一行 warn 点名。
    pub mesh_repaired: usize,
    #[serde(skip)]
    pub(crate) mesh_ids: std::collections::BTreeSet<String>,
    /// 本次生成读的是哪一版设计文件（一次调用一个 dbnum、一个会话）。读透写行时钉的
    /// 就是它——写透与生成必须看同一版文件。
    pub source_sesno: Option<u32>,
    /// 这一跑被两道门（共轴 / 通径）或缺件判**不画**的隐式管身，点名一行；没有就 `None`。
    ///
    /// 它不是失败：E3D 自己在同一份数据上也不画（2026-09-18 原生 RVM 对拍）。但少一根管
    /// 在 `failed=0 failures=[]` 的账面上完全看不出来，下游只看得见模型少了一节——
    /// 这一行就是给日志与任务回执用的线索，来源是 e3d-model 的
    /// `ImpliedTubes::not_drawn_line()`。
    pub tubes_not_drawn: Option<String>,
    /// 失败根的稳定身份与原因。`failed` 保留历史的元素/根计数，这一格让上层只扣真正失败的根，
    /// 不把同批所有兄弟一起判死。
    #[serde(skip)]
    pub failed_roots: Vec<(RefNo, String)>,
    /// **部分发布**的根：有元素没算出来，但这个根此前什么都没发布过，所以能画的那些照发。
    /// 它不是失败——模型行已经落库、回执已经盖上；`failed` 里那几笔仍按元素数计。上层靠它
    /// 把「这一批的失败已经落到具体的根上了」与「失败没有归属、只能整批判死」区分开。
    #[serde(skip)]
    pub partial_roots: Vec<(RefNo, String)>,
    /// 每个**新生成**（非缓存命中）的根：e3d-model DFS 访问到的子树元素名单
    /// （`GenerationOutcome::visited`）。生成服务只报名单、不写库（ADR-057）；把它们写透进
    /// `pe` / `ATT_*` 是编排层 `model_refresh::generate_roots_report` 在发布模型行之前做的事。
    #[serde(skip)]
    pub visited_by_root: std::collections::HashMap<RefNo, Vec<RefNo>>,
}

/// 网格键（`geo_hash`）的合法字符集：内容寻址键只由数字、字母与下划线组成，长度有上限。
///
/// 这道门同时挡目录穿越——键要用来拼 `.mesh` 路径，只放行这一个字符集，`..`、`/`、`\`
/// 连门都进不了。写（[`ensure_mesh_file`] 的调用方算出来的键）、读
/// （`/api/v1/meshes/{geo_hash}`，那里叫 `is_valid_geo_hash`）、删
/// （清理模型的「一并删网格文件」）三条路共用这一份，不许各写一份。
pub fn is_valid_mesh_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
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

/// 一件几何在库里的记录 id：真实元素用它自己的 refno，运行时派生的隐式管身没有 refno，
/// 用身份的 SHA-256。
///
/// 两种形状天生分得开——refno 是 `<十进制>_<十进制>`，摘要是 64 位纯十六进制，不含下划线，
/// 所以摘要不需要再戴前缀来自证身份。（2026-09-04 之前戴的是 `derived_`。它当年的理由是
/// 「按根删除时普通行能靠 refno 猜到 id，派生行猜不到」，而删除路径早就改走 `geometry_id`
/// 清单、更新一律先删后插，猜 id 这件事本身没有了。）
pub fn geometry_record_id(geometry_id: &GeometryId, source_refno: RefNo) -> String {
    match geometry_id {
        GeometryId::Element { .. } => format!("{}_{}", source_refno.word0, source_refno.word1),
        GeometryId::ImpliedTube { .. } => hex::encode(Sha256::digest(
            serde_json::to_vec(geometry_id).expect("GeometryId JSON"),
        )),
    }
}

pub fn ensure_mesh_file(path: &Path, mesh: &PlantMesh) -> anyhow::Result<MeshWrite> {
    if path.exists() {
        return match verify_mesh_file(path, mesh) {
            Ok(()) => Ok(MeshWrite::Reused),
            Err(mismatch) => repair_mesh_file(path, mesh, &mismatch),
        };
    }

    let parent = path.parent().context("mesh 路径没有父目录")?;
    std::fs::create_dir_all(parent)?;
    let temp = stage_mesh_file(parent, path, mesh)?;
    // `rename` overwrites an existing destination on Windows, so it is not a
    // create-if-absent primitive there. A same-directory hard-link publishes
    // the fully closed inode atomically without replacing a concurrent winner;
    // removing the private temp name leaves exactly the requested final file.
    match std::fs::hard_link(&temp, path) {
        Ok(()) => {
            std::fs::remove_file(&temp)?;
            // POSIX 下文件 fsync 不带上它的目录项：不刷目录，掉电后可能只剩一个
            // 「有内容但没有名字」的 inode。Windows 没有这一层。
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(MeshWrite::Written)
        }
        Err(error) if path.exists() => {
            let _ = std::fs::remove_file(&temp);
            match verify_mesh_file(path, mesh) {
                Ok(()) => Ok(MeshWrite::Reused),
                Err(mismatch) => repair_mesh_file(
                    path,
                    mesh,
                    &mismatch.context(format!("并发写入胜者校验失败: {error}")),
                ),
            }
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error).with_context(|| format!("发布 {}", path.display()))
        }
    }
}

/// 在目标同目录写一份校验过、刷过盘的临时文件，返回它的路径。
fn stage_mesh_file(parent: &Path, path: &Path, mesh: &PlantMesh) -> anyhow::Result<std::path::PathBuf> {
    let temp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("mesh"),
        std::process::id(),
        TEMP_MESH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let staged = mesh
        .ser_to_file(&temp)
        .and_then(|()| verify_mesh_file(&temp, mesh))
        // 校验过的是页缓存里的字节，不落盘等于没写：发布队列的 Result 帧 fsync 过、引用着
        // 这个 mesh id，掉电后帧还在而文件缺了半截，那条结果就只能作废重生成。所以先把临时
        // 文件本身刷下去，再发布到目标名上。
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&temp)?
                .sync_all()
                .with_context(|| format!("落盘 {}", temp.display()))
        });
    if let Err(error) = staged {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(temp)
}

/// 盘上的同名文件读不出、或与这份网格逐位不符：原子重写（ADR-087 D3）。
///
/// 名字就是内容摘要，这个名字下只可能该是这一份内容，所以覆盖永远安全——别的引用者指着同一个
/// id，拿到的正是它们本该拿到的那份。这里要的正是覆盖，与新建时「不顶掉并发胜者」相反：
/// `rename` 在 Windows 上覆盖已有目标、在 POSIX 上原子替换。
fn repair_mesh_file(
    path: &Path,
    mesh: &PlantMesh,
    mismatch: &anyhow::Error,
) -> anyhow::Result<MeshWrite> {
    // 一份共享网格常被同一批里的许多元素同时烘焙：进程内的重写排成一队，后到的先复核，
    // 前面那个已经修好了就直接复用——多个 rename 抢同一个目标在 Windows 上会互相拒绝。
    static REPAIRS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _serial = REPAIRS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if verify_mesh_file(path, mesh).is_ok() {
        return Ok(MeshWrite::Reused);
    }
    log::warn!("[mesh] {} 与它的身份不符，原子重写: {mismatch:#}", path.display());
    let parent = path.parent().context("mesh 路径没有父目录")?;
    let temp = stage_mesh_file(parent, path, mesh)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        // 另一个进程抢先把它修好了。
        if verify_mesh_file(path, mesh).is_ok() {
            return Ok(MeshWrite::Reused);
        }
        return Err(error).with_context(|| format!("重写 {}（原文件: {mismatch:#}）", path.display()));
    }
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    verify_mesh_file(path, mesh).with_context(|| format!("{} 重写后校验失败", path.display()))?;
    Ok(MeshWrite::Repaired)
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

/// 读回一份落盘的 mesh。老的 rkyv 读取器在某些截断输入上会 panic：损坏的缓存文件是普通的
/// 读取失败，不是让进程停下的理由，所以在这里把 panic 接成错误。刚体前移含旋转时重算盒
/// （`e3d_model_service::rigid_motion_mesh_bounds`）走这条路读记录自己的网格。
pub(crate) fn read_mesh_file(path: &Path) -> anyhow::Result<PlantMesh> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        PlantMesh::des_mesh_file(&path)
    }))
    .map_err(|_| anyhow::anyhow!("{} mesh 文件损坏", path.display()))?
    .with_context(|| format!("read mesh {}", path.display()))
}

/// 把落盘的 mesh 拿回它自己的内容 id 上对一遍：`e3d_baked_v2_<sha256>` 的那一档，id
/// 就是内容摘要，重算一遍就知道文件是不是还是当初那份。
///
/// **只有烘焙 mesh 认得出来。** [`canonical_mesh_id`] 那一档是 `PrimitiveMeshKey` 加内容
/// 一起摘要后取前 8 字节，key 不在文件里、也没别处存着，光看文件复原不出来——那些 id
/// 直接放行，队列侧仍靠字节摘要盯着它「入册之后有没有变过」。
///
/// 用在重启后重建 mesh 账本的路上：那时候「当前文件」本身就是待证的东西，不能拿它自己
/// 当基准。生成期的写入路径不需要它——[`ensure_mesh_file`] 写的时候已经对着内存里的
/// mesh 逐字节校验过了。
pub fn verify_content_addressed_mesh(path: &Path, mesh_id: &str) -> anyhow::Result<()> {
    let Some(expected) = mesh_id.strip_prefix("e3d_baked_v2_") else {
        return Ok(());
    };
    let mesh = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        PlantMesh::des_mesh_file(&path)
    }))
    .map_err(|_| anyhow::anyhow!("{} mesh 文件损坏", path.display()))??;
    let actual = baked_mesh_id(&mesh);
    if actual.strip_prefix("e3d_baked_v2_") != Some(expected) {
        bail!(
            "{} 的内容摘要与它的 id 不符（算出来是 {actual}）",
            path.display()
        );
    }
    Ok(())
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

    #[test]
    fn checked_mesh_reader_preserves_legacy_archive_bytes_and_rejects_corruption() {
        #[derive(rkyv::Archive, rkyv::Serialize)]
        struct LegacyMesh {
            indices: Vec<u32>, vertices: Vec<glam::Vec3>, normals: Vec<glam::Vec3>,
            wire_vertices: Vec<Vec<glam::Vec3>>, aabb: Option<parry3d::bounding_volume::Aabb>,
        }
        let mut mesh = box_mesh();
        mesh.wire_vertices = vec![vec![glam::Vec3::new(-0.0, 2.5, -3.0)], vec![]];
        for with_aabb in [false, true] {
            let legacy = LegacyMesh {
                indices: mesh.indices.clone(), vertices: mesh.vertices.clone(), normals: mesh.normals.clone(),
                wire_vertices: mesh.wire_vertices.clone(), aabb: if with_aabb { mesh.aabb.clone() } else { None },
            };
            let mut current = mesh.clone(); current.aabb = legacy.aabb.clone();
            let bytes = rkyv::to_bytes::<_, 1024>(&legacy).unwrap();
            assert_eq!(bytes.as_slice(), current.ser_to_bytes(), "existing .mesh byte format must not change");
            let loaded = PlantMesh::des_from_bytes(&bytes).unwrap();
            assert_eq!(loaded.indices, current.indices);
            assert!(vec3_buffers_bit_equal(&loaded.vertices, &current.vertices));
            assert_eq!(loaded.wire_vertices, current.wire_vertices);
            for end in 0..bytes.len() {
                // A prefix may accidentally form a different valid archive;
                // validation may accept it, but must never read outside input.
                let _ = PlantMesh::des_from_bytes(&bytes[..end]);
            }
            let mut unaligned = vec![0u8]; unaligned.extend_from_slice(&bytes);
            assert_eq!(PlantMesh::des_from_bytes(&unaligned[1..]).unwrap().indices, current.indices);
        }
        for bytes in [b"".as_slice(), b"damaged", &[255u8; 256]] {
            assert!(PlantMesh::des_from_bytes(bytes).is_err());
        }
        let mut invalid_index = mesh.clone();
        invalid_index.indices[0] = u32::MAX;
        assert!(PlantMesh::des_from_bytes(&invalid_index.ser_to_bytes()).is_err());
        let mut invalid_offsets = mesh.ser_to_bytes();
        let root_start = invalid_offsets.len() - std::mem::size_of::<aios_core::shape::pdms_shape::ArchivedPlantMesh>();
        invalid_offsets[root_start..].fill(255);
        assert!(PlantMesh::des_from_bytes(&invalid_offsets).is_err());
    }

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

    /// 重启后重建队列的 mesh 账本走的就是这一条：文件得对着它自己的内容 id 站得住，
    /// 而不是拿「当前文件」当新基准——进程不在的时候文件才是最可能被改坏的。
    #[test]
    fn a_baked_mesh_must_still_hash_to_its_own_id() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = box_mesh();
        let id = baked_mesh_id(&mesh);
        let path = dir.path().join(format!("{id}.mesh"));
        assert_eq!(ensure_mesh_file(&path, &mesh).unwrap(), MeshWrite::Written);
        verify_content_addressed_mesh(&path, &id).unwrap();

        let mut tampered = mesh.clone();
        tampered.vertices[0].x = f32::from_bits(tampered.vertices[0].x.to_bits() ^ 1);
        std::fs::remove_file(&path).unwrap();
        tampered.ser_to_file(&path).unwrap();
        let error = format!(
            "{:#}",
            verify_content_addressed_mesh(&path, &id).unwrap_err()
        );
        assert!(error.contains("内容摘要与它的 id 不符"), "{error}");

        // 图元那一档的短 id 从文件复原不出 PrimitiveMeshKey，只能放行。这是明写的边界，
        // 不是漏网：那些 id 由队列侧的字节摘要盯着「入册之后有没有变过」。
        verify_content_addressed_mesh(&path, &canonical_mesh_id(PrimitiveMeshKey::BoxV1, &mesh))
            .unwrap();
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

    /// 名字就是内容：同名文件读不出、被截断、或换成了另一份合法网格，都原样重写回这一份，
    /// 之后就是普通复用（ADR-087 D3）。此前这三种都让这件几何报错失败、文件一直坏着。
    #[test]
    fn a_damaged_content_addressed_file_is_rewritten_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = box_mesh();
        let path = dir.path().join(format!("{}.mesh", baked_mesh_id(&mesh)));
        assert_eq!(ensure_mesh_file(&path, &mesh).unwrap(), MeshWrite::Written);
        let good = std::fs::read(&path).unwrap();

        let mut other = mesh.clone();
        other.vertices[0].x += 1.0;
        let damages: [(&str, Vec<u8>); 3] = [
            ("garbage", b"damaged".to_vec()),
            ("truncated", good[..good.len() / 2].to_vec()),
            ("another valid mesh", other.ser_to_bytes().to_vec()),
        ];
        for (label, bytes) in damages {
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(
                ensure_mesh_file(&path, &mesh).unwrap(),
                MeshWrite::Repaired,
                "{label}"
            );
            verify_mesh_file(&path, &mesh).unwrap();
            assert_eq!(
                ensure_mesh_file(&path, &mesh).unwrap(),
                MeshWrite::Reused,
                "{label}: 修好之后就是普通复用"
            );
        }
        assert!(dir.path().read_dir().unwrap().all(|entry| {
            !entry.unwrap().file_name().to_string_lossy().ends_with(".tmp")
        }));
    }

    /// 一批里许多元素共用一份坏掉的网格、同时烘焙：恰好一个重写，其余复核后复用，没人失败。
    #[test]
    fn concurrent_repairs_rewrite_once_and_nobody_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = box_mesh();
        let path = dir.path().join(format!("{}.mesh", baked_mesh_id(&mesh)));
        std::fs::write(&path, b"damaged").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let outcomes = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let path = path.clone();
                let mesh = mesh.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    ensure_mesh_file(&path, &mesh).unwrap()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == MeshWrite::Repaired)
                .count(),
            1,
            "{outcomes:?}"
        );
        verify_mesh_file(&path, &mesh).unwrap();
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
        assert_eq!(
            ensure_mesh_file(&path, &expected).unwrap(),
            MeshWrite::Repaired,
            "+0.0 与 -0.0 按位不同，不能当同一份复用"
        );
        assert_eq!(
            PlantMesh::des_mesh_file(&path).unwrap().vertices[0].x.to_bits(),
            (-0.0f32).to_bits()
        );

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
