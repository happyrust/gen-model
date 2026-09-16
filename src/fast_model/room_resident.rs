//! 常驻房间模型（Resident Room Model）——房间→面板拓扑与在册面板几何的进程内缓存。
//!
//! 2026-09-07 grill 四轮定下（`docs/plans/2026-09-07-resident-room-model-plan.md` §8，共识 d-3）：
//!
//! 1. **一份真值，两份视图。** 它是冻结 E3D 文件（拓扑）与库里那一份几何（`inst_relate`
//!    实时行，d-21 / ADR-072 D4——发布视图表随 publish 阶段退役）的**缓存**，不是第二份真值：
//!    换代只以会话摘要为键整份替换（[`ResidentRooms::install`]），不逐条修补。`latest` 与
//!    `published` 两份并存，**回答归属的入口只许经 [`ResidentRooms::for_membership`]**——
//!    摄入形态永远拿不到 `latest`。拿错了不报错，只是房间号静默算错，所以这一条是源码钉。
//! 2. **判不了就不写。** 合成拓扑是跨文件的（`room_topology_source::compose` 一张 lookup），
//!    任一文件读不到 / 分片不齐 → 整份 `Err`（[`ResidentBuildError`]），不产半份模型。
//! 3. **触发源只有一个。** 结构触发是账本（`room_structural_trigger`）；这里的 [`TopologyDiff`]
//!    只在启动对比与护栏两处出手，**不产** `RoomRecalcPanel`。
//! 4. **房间是参照系，不是增量的一部分**（第 5 轮 Q17–Q20）。[`initialize_at_startup`] 在启动
//!    最前段建模，范围是 MDB **全部** DESI（`design_sources()`），不看 `watch_dbnums`、不看
//!    上弦位——本文件与 `room_topology` / `room_topology_source` 一律**不得引用 `watch_scope`**
//!    （源码钉 `room_files_never_consult_the_watch_scope`）；哪些清单行归文件轮次管由调用方
//!    作为参数传进来，初始化只替**不归任何轮次管**的文件前进清单行。
//!
//! P1（本文件）只有本体：合成、摘要、diff、网格缓存、两代存放，一行生产路径未接。接线
//! （`drain_rooms_selected` 进串行锁后按清单键懒换代、三条归属入口改读这里）在 P2，
//! 开关 `room_resident_model` 默认开，仍可显式关闭以走兼容读取路径。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::time::Instant;

use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{GeomInstQuery, RefnoEnum};
use anyhow::Context;
use glam::Mat4;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::shape::{TriMesh, TriMeshFlags};
use sha2::{Digest, Sha256};

use crate::fast_model::room_model::{RoomPanelMap, RoomPanels, room_panel_map_from_groups};
use crate::fast_model::room_predicate::aabb_is_usable;
use crate::fast_model::room_topology::{RoomGroup, RoomHierarchy, room_panel_groups};
use crate::options::DataFace;

/// 一份模型跟着哪一路形态走。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ResidentForm {
    /// 摄入：键 = 各 DESI `(dbnum, sesno, token)` 的规范摘要（与读透同源；发布清单键随
    /// ADR-062 一起作废）。名字保留是为了 `/health` 与回执的 `form` 字面不变。
    Published,
    /// 读透：当前文件会话，键 = 各 DESI `(dbnum, sesno, token)` 的规范摘要
    /// （与 `room_topology::load_room_groups_from_files` 的 composite 同一个字符串）。
    Latest,
}

impl ResidentForm {
    pub(crate) fn for_face(face: DataFace) -> Self {
        if face.is_read_through() {
            Self::Latest
        } else {
            Self::Published
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::Latest => "latest",
        }
    }
}

/// 一份模型的身份：形态 + 那一路的键。键相同 = 同一组文件版本 = 同一份模型。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ResidentManifest {
    pub form: ResidentForm,
    pub key: String,
}

/// 在册面板的一条几何实例，世界矩阵已经乘好（`world_trans * transform`）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResidentInst {
    pub geo_hash: String,
    pub matrix: Mat4,
}

/// 一块在册面板：拓扑归属 + 几何。同一 refno 的多条 `inst_relate` 行并成一块
/// （包围盒取并、实例取并），与 `PanelIndex` 一行一条的存法不同，但候选相交与网格集合一致。
#[derive(Debug, Clone)]
pub(crate) struct ResidentPanel {
    pub panel: RefnoEnum,
    pub room: RefnoEnum,
    pub room_num: String,
    pub world_aabb: Aabb,
    pub insts: Vec<ResidentInst>,
    pub has_neg: bool,
}

impl ResidentPanel {
    /// Preserve the preloaded world-space instances when adapting an older
    /// calculation entry; no model row or mesh needs to be fetched again.
    pub(crate) fn room_inst(&self) -> GeomInstQuery {
        use bevy_transform::components::Transform;
        GeomInstQuery {
            refno: self.panel, old_refno: None, owner: self.room,
            world_aabb: self.world_aabb, world_trans: Transform::IDENTITY,
            insts: self.insts.iter().map(|inst| aios_core::ModelHashInst {
                geo_hash: inst.geo_hash.clone(), transform: Transform::from_matrix(inst.matrix), ..Default::default()
            }).collect(),
            has_neg: self.has_neg, generic: "PANE".into(), pts: None, date: None,
        }
    }
}

/// 三角网缓存键：`.mesh` 按内容寻址不可变，键里再带世界矩阵的位模式，命中即可复用、
/// 永不陈旧（工程默认①）。`Ord` 给换代回收的活键集（`BTreeSet`）用。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct MeshKey {
    geo_hash: String,
    matrix_bits: [u32; 16],
}

impl MeshKey {
    fn new(geo_hash: &str, matrix: Mat4) -> Self {
        Self {
            geo_hash: geo_hash.to_string(),
            matrix_bits: matrix.to_cols_array().map(f32::to_bits),
        }
    }
}

/// 跨代共享的世界坐标三角网。模型换代只换拓扑与实例表，网格照旧命中。
#[derive(Default)]
pub(crate) struct MeshCache {
    inner: RwLock<HashMap<MeshKey, Arc<TriMesh>>>,
}

impl MeshCache {
    pub(crate) fn len(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn get_or_build(
        &self,
        key: MeshKey,
        build: impl FnOnce() -> Result<TriMesh, String>,
    ) -> Result<Arc<TriMesh>, String> {
        if let Some(hit) = self
            .inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return Ok(hit.clone());
        }
        let built = Arc::new(build()?);
        let mut guard = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        Ok(guard.entry(key).or_insert(built).clone())
    }

    /// [`Self::get_or_build`] 的闸内版本（S3.2）：命中只碰一次读锁；未命中把 `.mesh`
    /// 读取与三角网构建放进几何 CPU 闸（[`run_gated_cpu`]）——预热的 fan-out 与生成
    /// 路径共用同一份额度，闸就是唯一的并发与内存限流阀门。
    ///
    /// [`run_gated_cpu`]: crate::fast_model::concurrency::run_gated_cpu
    async fn get_or_build_gated(
        &self,
        key: MeshKey,
        mesh_dir: &Path,
        geo_hash: String,
        matrix: Mat4,
    ) -> Result<Arc<TriMesh>, String> {
        if let Some(hit) = self
            .inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return Ok(hit.clone());
        }
        let mesh_dir = mesh_dir.to_path_buf();
        let built = crate::fast_model::concurrency::run_gated_cpu(move || {
            build_tri_mesh(&mesh_dir, &geo_hash, matrix).map_err(anyhow::Error::msg)
        })
        .await
        .map_err(|error| format!("{error:#}"))?;
        let built = Arc::new(built);
        let mut guard = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        Ok(guard.entry(key).or_insert(built).clone())
    }

    fn retain(&self, live: &BTreeSet<MeshKey>) {
        self.inner
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|key, _| live.contains(key));
    }
}

pub(crate) fn build_tri_mesh(
    mesh_dir: &Path,
    geo_hash: &str,
    matrix: Mat4,
) -> Result<TriMesh, String> {
    let file_path = mesh_dir.join(format!("{geo_hash}.mesh"));
    let mesh = PlantMesh::des_mesh_file(&file_path)
        .map_err(|error| format!("{}: {error}", file_path.display()))?;
    mesh.get_tri_mesh_with_flag(
        matrix,
        TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
    )
    .ok_or_else(|| format!("{}: 三角网转换失败", file_path.display()))
}

/// 一张世界坐标三角网占的字节（顶点 + 索引），`mesh_bytes` 指标的口径。
fn tri_mesh_bytes(mesh: &TriMesh) -> usize {
    mesh.vertices().len() * 3 * std::mem::size_of::<f32>()
        + mesh.indices().len() * std::mem::size_of::<[u32; 3]>()
}

/// 拓扑摘要：对房间与面板的顺序不敏感（两条来源的遍历序可能不同），对房间名、房间集合、
/// 每间的面板集合敏感。**只看拓扑**，面板几何的变化不在这里——那一路走发布事务的
/// `RoomRecalcPanel`。
pub(crate) fn topology_digest(groups: &[RoomGroup]) -> String {
    let mut ordered: Vec<(RefnoEnum, &str, Vec<RefnoEnum>)> = groups
        .iter()
        .map(|group| {
            let mut panels = group.panels.clone();
            panels.sort();
            panels.dedup();
            (group.room, group.name.as_str(), panels)
        })
        .collect();
    ordered.sort();
    let mut digest = Sha256::new();
    digest.update(b"resident-room-topology-v1");
    for (room, name, panels) in ordered {
        digest.update(room.to_string().as_bytes());
        digest.update([0u8]);
        digest.update(name.as_bytes());
        digest.update([0u8]);
        digest.update((panels.len() as u64).to_le_bytes());
        for panel in panels {
            digest.update(panel.to_string().as_bytes());
            digest.update([0u8]);
        }
    }
    hex::encode(digest.finalize())
}

/// 几何摘要：有几何的在册面板各自的实例（`geo_hash` + 世界矩阵位模式），按面板序。
/// 它进 [`ResidentRoomModel::input_version`]——面板从有几何那一侧被搬走（`demote_panel`）后要重算，
/// 否则计算器自报的版本对不上它实际拿到的候选集。
fn geometry_digest_of(panels: &BTreeMap<RefnoEnum, ResidentPanel>) -> String {
    let mut geometry = Sha256::new();
    geometry.update(b"resident-room-geometry-v1");
    for (panel, value) in panels {
        geometry.update(panel.to_string().as_bytes());
        for inst in &value.insts {
            geometry.update(inst.geo_hash.as_bytes());
            for bits in inst.matrix.to_cols_array().map(f32::to_bits) {
                geometry.update(bits.to_le_bytes());
            }
        }
    }
    hex::encode(geometry.finalize())
}

/// 关键字过滤 + 命名校验后的在册视图——与 `room_model::load_room_panel_map` 的产物同形。
pub(crate) fn registered_map(
    groups: &[RoomGroup],
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
) -> RoomPanelMap {
    room_panel_map_from_groups(room_panel_groups(groups, room_key_words), match_room_fn)
}

/// 在册面板（去重、按 refno 排序）——几何要查的就是这一批。
pub(crate) fn registered_panels(map: &RoomPanelMap) -> Vec<RefnoEnum> {
    map.rooms
        .iter()
        .flat_map(|room| room.panels.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// 一份合成好的模型。字段是只读快照，换代整份替换。
pub(crate) struct ResidentRoomModel {
    pub manifest: ResidentManifest,
    /// 未过关键字的原始分组（`room_topology::RoomGroup`）；摘要从它算。
    pub groups: Vec<RoomGroup>,
    /// 过关键字 + 命名校验的在册视图；`all_panels` 是成员候选的排除集。
    pub map: RoomPanelMap,
    /// 在册且几何可用的面板。
    pub panels: BTreeMap<RefnoEnum, ResidentPanel>,
    /// 在册却没有可用几何的面板（与 `PanelIndex::missing_panels` 同口径、同序）：从未生成、
    /// 启动预生成失败、世界包围盒不可用、`.mesh` 读不出——四种来源同一处置（ADR-073 D9）：
    /// 不进索引、逐根写边时作为保护集让开、在这里留痕，**不阻断任何根**。
    pub missing_panels: Vec<RefnoEnum>,
    /// `missing_panels` 里**有几何行、却有一行世界包围盒不可用**的那一部分。只是指标上的细分
    /// （`/health` 与横幅点名「几何不可信」而不是「还没生成」）；对归属计算与其余缺几何面板
    /// 一样是保护集。「有行却不可用挡参照」那一档随参照机制作废（ADR-073 2026-09-16 D9）。
    pub unusable_panels: BTreeSet<RefnoEnum>,
    /// MDB 声明了、搜索根里却没有文件的 DESI `(dbnum, 名字)`（ADR-073 D8）：模型按有的文件建，
    /// 缺失库里的房间与面板不在册。
    pub missing_dbnums: Vec<(u32, String)>,
    /// 这份模型带着的缺口，一条一句（缺库 / 不可用几何 / 预生成失败 / 网格预热失败）。非空 =
    /// 生命周期 `Degraded { problems }`（D10 仍放行计算器）；`/health.features.room.problems` 原样出。
    pub problems: Vec<String>,
    pub digest: String,
    pub geometry_digest: String,
    /// 这份拓扑依据的每个文件版本（按 dbnum 序）：两形态都是各 DESI 的冻结令牌。启动参照
    /// 按它们从 `room_topology_source` 的磁盘分片合成「旧」视图（共识 d-42：两形态同一条路）。
    pub source_tokens: Vec<String>,
    /// MDB DESI source set frozen for this generation, sorted by dbnum.
    pub expected_dbnums: Vec<u32>,
    /// 房间 → 声明它的那个**文件** dbnum。只有 `/health` 与回执的「无几何按库」用它
    /// （见 [`ResidentStats::missing_panels_by_dbnum`]）；纯测搭的模型可以是空的，那时退回
    /// 面板 refno 的头一段。跨文件的面板（`compose` 支持、真库未见）算在它房间那个文件名下。
    pub room_files: BTreeMap<RefnoEnum, u32>,
    pub built_at: std::time::Instant,
    meshes: Arc<MeshCache>,
    mesh_warm: MeshWarmStats,
}

impl fmt::Debug for ResidentRoomModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResidentRoomModel")
            .field("manifest", &self.manifest)
            .field("rooms", &self.map.rooms.len())
            .field("registered_panels", &registered_panels(&self.map).len())
            .field("panels_with_geometry", &self.panels.len())
            .field("missing_panels", &self.missing_panels.len())
            .field("unusable_panels", &self.unusable_panels.len())
            .field("missing_dbnums", &self.missing_dbnums.len())
            .field("problems", &self.problems.len())
            .field("digest", &self.digest)
            .finish()
    }
}

/// `/health` 与回执那一格。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ResidentStats {
    pub form: &'static str,
    pub key: String,
    pub digest: String,
    pub rooms: usize,
    pub registered_panels: usize,
    pub panels_with_geometry: usize,
    pub missing_panels: usize,
    /// 无几何的在册面板按**文件** dbnum 分组（Q20 (a)）：`/health` 拿它点名「来自未监听的
    /// 库」——哪些库未监听由读 `/health` 的那一侧对 `watch_scope` 说，这里只给数。
    ///
    /// 键是文件库号（7997），不是面板 refno 的头一段（24381）：`watch_dbnums`、发布清单、
    /// 水位说的都是文件库号，照 refno 报既写错库号，又会把被监听库的面板诬成「未监听」。
    pub missing_panels_by_dbnum: BTreeMap<u32, usize>,
    /// `missing_panels` 里有几何行却不可用的那一部分（指标细分，不再挡任何东西，ADR-073 D9）。
    pub unusable_panels: usize,
    /// MDB 声明了却没有文件的 DESI（ADR-073 D8），按 dbnum 序。
    pub missing_dbnums: Vec<u32>,
    /// 这份模型带着的缺口条数（[`ResidentRoomModel::problems`]）；非空 = `Degraded`。
    pub problems: usize,
    pub meshes_cached: usize,
    pub mesh_keys_expected: usize,
    pub mesh_keys_warmed: usize,
    pub mesh_warm_failed: usize,
    pub mesh_bytes: usize,
    pub source_sessions: BTreeMap<u32, u32>,
    pub expected_dbnums: Vec<u32>,
    pub geometry_digest: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) struct MeshWarmStats {
    pub expected: usize,
    pub warmed: usize,
    pub failed: usize,
    pub bytes: usize,
}

impl ResidentRoomModel {
    pub(crate) fn input_version(&self) -> String {
        // Registration keywords and room-code parsing can change without an E3D
        // save. Hash the actual calculation map as well as raw source topology.
        let mut rooms: Vec<_> = self
            .map
            .rooms
            .iter()
            .map(|room| {
                let mut panels = room.panels.clone();
                panels.sort();
                (room.room.to_string(), room.room_num.clone(), panels)
            })
            .collect();
        rooms.sort();
        let mut excluded: Vec<_> = self.map.all_panels.iter().collect();
        excluded.sort();
        let mut digest = Sha256::new();
        digest.update(b"room-memory-input-v1");
        digest.update(self.digest.as_bytes());
        digest.update(self.geometry_digest.as_bytes());
        digest.update(format!("{rooms:?}:{excluded:?}").as_bytes());
        hex::encode(digest.finalize())
    }

    /// E3D output adapter. No persisted instance rows participate in this view.
    pub(crate) fn from_projection(
        manifest: ResidentManifest,
        groups: Vec<RoomGroup>,
        map: RoomPanelMap,
        records: &[crate::fast_model::model_memory_store::ProjectedGeometry],
        meshes: Arc<MeshCache>,
    ) -> Self {
        let rows = records.iter().map(|record| record.room_inst()).collect();
        Self::assemble(manifest, groups, map, rows, meshes)
    }
    fn mesh_keys(&self) -> impl Iterator<Item = MeshKey> + '_ {
        self.panels.values().flat_map(|panel| {
            panel
                .insts
                .iter()
                .map(|inst| MeshKey::new(&inst.geo_hash, inst.matrix))
        })
    }
    /// 纯函数：拓扑分组 + 几何行 → 模型。两路形态共用这一段，差别只在调用方从哪拿输入。
    pub(crate) fn assemble(
        manifest: ResidentManifest,
        groups: Vec<RoomGroup>,
        map: RoomPanelMap,
        insts: Vec<GeomInstQuery>,
        meshes: Arc<MeshCache>,
    ) -> Self {
        let digest = topology_digest(&groups);
        let mut owner_of: BTreeMap<RefnoEnum, (RefnoEnum, &str)> = BTreeMap::new();
        for room in &map.rooms {
            for panel in &room.panels {
                owner_of
                    .entry(*panel)
                    .or_insert((room.room, room.room_num.as_str()));
            }
        }
        let mut panels: BTreeMap<RefnoEnum, ResidentPanel> = BTreeMap::new();
        let mut invalid: BTreeSet<RefnoEnum> = BTreeSet::new();
        for inst in insts {
            let Some((room, room_num)) = owner_of.get(&inst.refno) else {
                continue;
            };
            if !aabb_is_usable(&inst.world_aabb) {
                invalid.insert(inst.refno);
                continue;
            }
            let entry = panels.entry(inst.refno).or_insert_with(|| ResidentPanel {
                panel: inst.refno,
                room: *room,
                room_num: room_num.to_string(),
                world_aabb: inst.world_aabb,
                insts: Vec::new(),
                has_neg: false,
            });
            entry.world_aabb.merge(&inst.world_aabb);
            entry.has_neg |= inst.has_neg;
            for geo in &inst.insts {
                entry.insts.push(ResidentInst {
                    geo_hash: geo.geo_hash.clone(),
                    matrix: (inst.world_trans * geo.transform).compute_matrix(),
                });
            }
        }
        // 一条行都拿不到、或每条行的包围盒都不可用的在册面板：没几何就判不了。
        let mut missing_panels: Vec<RefnoEnum> = owner_of
            .keys()
            .copied()
            .filter(|panel| !panels.contains_key(panel) || invalid.contains(panel))
            .collect();
        for panel in &invalid {
            panels.remove(panel);
        }
        for panel in panels.values_mut() {
            panel.insts.sort_by(|a, b| {
                MeshKey::new(&a.geo_hash, a.matrix).cmp(&MeshKey::new(&b.geo_hash, b.matrix))
            });
            panel.insts.dedup_by(|a, b| {
                MeshKey::new(&a.geo_hash, a.matrix) == MeshKey::new(&b.geo_hash, b.matrix)
            });
        }
        missing_panels.sort_by_key(RefnoEnum::to_string);
        let geometry_digest = geometry_digest_of(&panels);
        let mut model = Self {
            manifest,
            groups,
            map,
            panels,
            missing_panels,
            unusable_panels: invalid,
            missing_dbnums: Vec::new(),
            problems: Vec::new(),
            digest,
            geometry_digest,
            source_tokens: Vec::new(),
            expected_dbnums: Vec::new(),
            room_files: BTreeMap::new(),
            built_at: std::time::Instant::now(),
            meshes,
            mesh_warm: MeshWarmStats::default(),
        };
        // 有几何行却世界包围盒不可用的面板：留痕成一条缺口（ADR-073 D9），不阻断。
        let unusable: Vec<RefnoEnum> = model.unusable_panels.iter().copied().collect();
        for panel in unusable {
            let dbnum = model.file_of_panel(panel);
            model.problems.push(format!(
                "dbnum={dbnum} panel={panel} stage=geometry reason=invalid-geometry"
            ));
        }
        model
    }

    pub(crate) fn with_source_tokens(mut self, tokens: Vec<String>) -> Self {
        self.source_tokens = tokens;
        self
    }

    /// MDB 声明了却没有文件的 DESI（ADR-073 D8）：记下来、每个库一条缺口。
    pub(crate) fn with_missing_dbnums(mut self, mut missing: Vec<(u32, String)>) -> Self {
        missing.sort();
        missing.dedup();
        for (dbnum, name) in &missing {
            self.problems.push(format!(
                "dbnum={dbnum} stage=source reason=missing-desi-file name={name}"
            ));
        }
        self.missing_dbnums = missing;
        self
    }

    /// 建模路上别处发现的缺口（启动预生成一块面板失败等）：原样追加，不去重、不排序——
    /// 调用方按发生顺序给。
    pub(crate) fn with_problems(mut self, problems: impl IntoIterator<Item = String>) -> Self {
        self.problems.extend(problems);
        self
    }

    /// 一块在册面板的几何判不了（`.mesh` 读不出 / 三角化失败）：从有几何那一侧搬到
    /// `missing_panels`——它此后是保护集的一员，不是候选。几何摘要随之重算。
    fn demote_panel(&mut self, panel: RefnoEnum) {
        let removed = self.panels.remove(&panel).is_some();
        if !self.missing_panels.contains(&panel) {
            self.missing_panels.push(panel);
            self.missing_panels.sort_by_key(RefnoEnum::to_string);
        }
        if removed {
            self.geometry_digest = geometry_digest_of(&self.panels);
        }
    }

    fn with_expected_dbnums(mut self, mut dbnums: Vec<u32>) -> Self {
        dbnums.sort_unstable();
        dbnums.dedup();
        self.expected_dbnums = dbnums;
        self
    }

    pub(crate) fn with_room_files(mut self, room_files: BTreeMap<RefnoEnum, u32>) -> Self {
        self.room_files = room_files;
        self
    }

    /// 这块在册面板算在哪个**文件**库号名下：先看它归属的房间来自哪个文件，没有文件归属
    /// （纯测模型）才退回 refno 的头一段。启动对比的回执按它给「无几何」分组
    /// （`room_startup::split_recalc_by_geometry`），与 [`ResidentStats::missing_panels_by_dbnum`] 同口径。
    pub(crate) fn file_of_panel(&self, panel: RefnoEnum) -> u32 {
        self.map
            .rooms
            .iter()
            .find(|room| room.panels.contains(&panel))
            .and_then(|room| self.room_files.get(&room.room).copied())
            .unwrap_or_else(|| panel.refno().get_0())
    }

    pub(crate) fn panel(&self, panel: RefnoEnum) -> Option<&ResidentPanel> {
        self.panels.get(&panel)
    }

    /// 一块面板的世界坐标三角网，经共享缓存。失败逐条留着：调用方要按面板汇总
    /// 「哪几块判不了」，与 `PanelMeshes::build` 同口径。
    pub(crate) fn panel_tri_meshes(
        &self,
        panel: RefnoEnum,
        mesh_dir: &Path,
    ) -> Result<Vec<Arc<TriMesh>>, Vec<String>> {
        let Some(entry) = self.panels.get(&panel) else {
            return Err(vec![format!(
                "面板 {panel} 不在常驻模型里（不在册或没有几何）"
            )]);
        };
        let mut meshes = Vec::with_capacity(entry.insts.len());
        let mut failures = Vec::new();
        for inst in &entry.insts {
            match self
                .meshes
                .get_or_build(MeshKey::new(&inst.geo_hash, inst.matrix), || {
                    build_tri_mesh(mesh_dir, &inst.geo_hash, inst.matrix)
                }) {
                Ok(mesh) => meshes.push(mesh),
                Err(failure) => failures.push(failure),
            }
        }
        if failures.is_empty() {
            Ok(meshes)
        } else {
            Err(failures)
        }
    }

    /// Materialize every unique geometry/placement used by this generation before it
    /// becomes visible, so that the per-root path never reads a `.mesh` it has not seen.
    ///
    /// **面板级失败不阻代**（ADR-073 D9，2026-09-16）：一块面板的 `.mesh` 读不出 / 三角化失败，
    /// 这块面板从有几何那一侧搬进 `missing_panels`（[`Self::demote_panel`]）、记一条缺口——
    /// 它此后是逐根写边的保护集成员，存量边不删不写；同一 `.mesh` 被别的面板共用时它们一并
    /// 搬走。有几何行却世界包围盒不可用的面板（`unusable_panels`）在 `assemble` 就已留痕，
    /// 这里不再把它们当阻断。此前这两类都整份 `Err` → resident `Blocked` → 之后每个任务的
    /// 全部根「room calculator unavailable」判死，与「一处输入缺失」的分量不成比例。
    /// 只剩一种 `Err`：预热 worker 本身崩了（`JoinError`）——那是进程级故障，受影响的面板集
    /// 不可知，按「房间体制没起来」处置。
    ///
    /// 并发走几何闸（S3.2）：[`resident_workers`] 个常驻 worker 从共享游标动态领取，
    /// `.mesh` 读取与 `TriMesh` 构建各在一张 [`run_gated_cpu`] 许可之内——预热与生成
    /// 路径共用同一份额度，`geometry_workers = 1` 即退化为旧实现那条串行预热。
    /// 任务按缓存键排序后派发，失败清单的顺序与重跑一致。
    ///
    /// [`resident_workers`]: crate::fast_model::concurrency::resident_workers
    /// [`run_gated_cpu`]: crate::fast_model::concurrency::run_gated_cpu
    pub(crate) async fn warm_meshes(&mut self, mesh_dir: &Path) -> Result<(), ResidentBuildError> {
        let mut work: HashMap<MeshKey, (RefnoEnum, String, Mat4)> = HashMap::new();
        let mut panels_of_key: HashMap<MeshKey, Vec<RefnoEnum>> = HashMap::new();
        for (panel, entry) in &self.panels {
            for inst in &entry.insts {
                let key = MeshKey::new(&inst.geo_hash, inst.matrix);
                work.entry(key.clone())
                    .or_insert_with(|| (*panel, inst.geo_hash.clone(), inst.matrix));
                panels_of_key.entry(key).or_default().push(*panel);
            }
        }
        self.mesh_warm.expected = work.len();
        self.mesh_warm.warmed = 0;
        self.mesh_warm.failed = 0;
        self.mesh_warm.bytes = 0;

        let mut failures = Vec::<String>::new();

        let mut jobs: Vec<(MeshKey, RefnoEnum, String, Mat4)> = work
            .into_iter()
            .map(|(key, (panel, geo_hash, matrix))| (key, panel, geo_hash, matrix))
            .collect();
        jobs.sort_by(|a, b| a.0.cmp(&b.0));
        let worker_count = crate::fast_model::concurrency::resident_workers(jobs.len());
        let jobs = Arc::new(jobs);
        let cursor = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut workers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let jobs = jobs.clone();
            let cursor = cursor.clone();
            let meshes = self.meshes.clone();
            let mesh_dir = mesh_dir.to_path_buf();
            workers.push(tokio::spawn(async move {
                let mut warmed = 0usize;
                let mut bytes = 0usize;
                let mut failed: Vec<(usize, RefnoEnum, String, String)> = Vec::new();
                loop {
                    let index = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((key, panel, geo_hash, matrix)) = jobs.get(index).cloned() else {
                        break;
                    };
                    match meshes
                        .get_or_build_gated(key, &mesh_dir, geo_hash.clone(), matrix)
                        .await
                    {
                        Ok(mesh) => {
                            warmed += 1;
                            bytes = bytes.saturating_add(tri_mesh_bytes(&mesh));
                        }
                        Err(reason) => failed.push((index, panel, geo_hash, reason)),
                    }
                }
                (warmed, bytes, failed)
            }));
        }
        let mut failed: Vec<(usize, RefnoEnum, String, String)> = Vec::new();
        for worker in workers {
            match worker.await {
                Ok((warmed, bytes, worker_failed)) => {
                    self.mesh_warm.warmed += warmed;
                    self.mesh_warm.bytes = self.mesh_warm.bytes.saturating_add(bytes);
                    failed.extend(worker_failed);
                }
                Err(join_error) => {
                    self.mesh_warm.failed += 1;
                    failures.push(format!(
                        "stage=mesh-warm reason=预热 worker 崩了: {join_error}"
                    ));
                }
            }
        }
        failed.sort_by_key(|(index, ..)| *index);
        self.mesh_warm.failed += failed.len();
        // 面板级失败：搬进保护集、留痕，不阻代（D9）。共用同一份 `.mesh` 的面板一并搬走——
        // 它们的三角网同样取不到。
        let mut demoted = BTreeSet::new();
        for (index, _, geo_hash, reason) in failed {
            let key = &jobs[index].0;
            let mut affected = panels_of_key.remove(key).unwrap_or_default();
            affected.sort_by_key(RefnoEnum::to_string);
            affected.dedup();
            for panel in affected {
                if demoted.insert(panel) {
                    self.problems.push(format!(
                        "dbnum={} panel={panel} stage=mesh-warm geo_hash={geo_hash} reason={reason}",
                        self.file_of_panel(panel)
                    ));
                }
            }
        }
        for panel in demoted {
            self.demote_panel(panel);
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ResidentBuildError {
                form: self.manifest.form,
                key: Some(self.manifest.key.clone()),
                reasons: failures,
            })
        }
    }

    pub(crate) fn stats(&self) -> ResidentStats {
        let source_sessions = self
            .source_tokens
            .iter()
            .filter_map(|token| {
                crate::data_interface::frozen_source::FrozenSource::from_commit_token_json(token)
                    .ok()
                    .map(|source| (source.dbnum() as u32, source.target_sesno()))
            })
            .collect();
        ResidentStats {
            form: self.manifest.form.as_str(),
            key: self.manifest.key.clone(),
            digest: self.digest.clone(),
            rooms: self.map.rooms.len(),
            registered_panels: registered_panels(&self.map).len(),
            panels_with_geometry: self.panels.len(),
            missing_panels: self.missing_panels.len(),
            missing_panels_by_dbnum: self.missing_panels.iter().fold(
                BTreeMap::new(),
                |mut by_dbnum, panel| {
                    *by_dbnum.entry(self.file_of_panel(*panel)).or_insert(0) += 1;
                    by_dbnum
                },
            ),
            unusable_panels: self.unusable_panels.len(),
            missing_dbnums: self
                .missing_dbnums
                .iter()
                .map(|(dbnum, _)| *dbnum)
                .collect(),
            problems: self.problems.len(),
            meshes_cached: self.meshes.len(),
            mesh_keys_expected: self.mesh_warm.expected,
            mesh_keys_warmed: self.mesh_warm.warmed,
            mesh_warm_failed: self.mesh_warm.failed,
            mesh_bytes: self.mesh_warm.bytes,
            source_sessions,
            expected_dbnums: self.expected_dbnums.clone(),
            geometry_digest: self.geometry_digest.clone(),
        }
    }
}

/// 一间房只改了房间号、面板集合没动：`room_relate` / `room_panel_relate` 改列，不重算。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoomRenumber {
    pub room: RefnoEnum,
    pub old_num: String,
    pub new_num: String,
    /// 两边都在这间房名下的面板；只有它们的边需要改列。
    pub panels: Vec<RefnoEnum>,
}

/// 两份在册视图之间的拓扑差（Q6，双向）。只看在册面板——只有它们有边。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TopologyDiff {
    /// 旧 \ 新：面板挪出在册房间、房间改出关键字 / 命名不再合规、房间没了。
    /// 处置 = `recalc_panel_membership_inner` 的「面板已不在册」分支（清它全部出边与
    /// `room_panel_relate`）。必须从**旧**那份枚举——新那份里根本没有它们。
    pub removed_panels: BTreeSet<RefnoEnum>,
    /// 新 \ 旧，以及两边都在但所属房间变了的：整间重算。
    pub recalc_panels: BTreeSet<RefnoEnum>,
    /// 同一间房（refno 相同）房间号变了：交集里的面板改列。
    pub renumbered_rooms: Vec<RoomRenumber>,
}

fn panel_owner_index(map: &RoomPanelMap) -> BTreeMap<RefnoEnum, RefnoEnum> {
    let mut index = BTreeMap::new();
    for room in &map.rooms {
        for panel in &room.panels {
            index.entry(*panel).or_insert(room.room);
        }
    }
    index
}

impl TopologyDiff {
    pub(crate) fn between(old: &RoomPanelMap, new: &RoomPanelMap) -> Self {
        let old_owner = panel_owner_index(old);
        let new_owner = panel_owner_index(new);
        let mut diff = Self::default();
        for panel in old_owner.keys() {
            if !new_owner.contains_key(panel) {
                diff.removed_panels.insert(*panel);
            }
        }
        for (panel, room) in &new_owner {
            match old_owner.get(panel) {
                Some(old_room) if old_room == room => {}
                _ => {
                    diff.recalc_panels.insert(*panel);
                }
            }
        }
        let old_rooms: BTreeMap<RefnoEnum, &RoomPanels> =
            old.rooms.iter().map(|room| (room.room, room)).collect();
        for room in &new.rooms {
            let Some(before) = old_rooms.get(&room.room) else {
                continue;
            };
            if before.room_num == room.room_num {
                continue;
            }
            let common: Vec<RefnoEnum> = room
                .panels
                .iter()
                .copied()
                .filter(|panel| before.panels.contains(panel))
                .filter(|panel| !diff.recalc_panels.contains(panel))
                .collect();
            if !common.is_empty() {
                diff.renumbered_rooms.push(RoomRenumber {
                    room: room.room,
                    old_num: before.room_num.clone(),
                    new_num: room.room_num.clone(),
                    panels: common,
                });
            }
        }
        diff
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.removed_panels.is_empty()
            && self.recalc_panels.is_empty()
            && self.renumbered_rooms.is_empty()
    }

    /// diff 碰到的全部面板——护栏拿它对本窗 / 本轮的面板目标。
    pub(crate) fn touched_panels(&self) -> BTreeSet<RefnoEnum> {
        let mut touched = self.removed_panels.clone();
        touched.extend(self.recalc_panels.iter().copied());
        for room in &self.renumbered_rooms {
            touched.extend(room.panels.iter().copied());
        }
        touched
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "removed={} recalc={} renumbered_rooms={} (panels {})",
            self.removed_panels.len(),
            self.recalc_panels.len(),
            self.renumbered_rooms.len(),
            self.renumbered_rooms
                .iter()
                .map(|room| room.panels.len())
                .sum::<usize>()
        )
    }
}

/// 建不起来：整份失败，不产半份模型（原则 2）。`/health` 与启动日志按它点名。
#[derive(Debug)]
pub(crate) struct ResidentBuildError {
    pub form: ResidentForm,
    pub key: Option<String>,
    pub reasons: Vec<String>,
}

impl ResidentBuildError {
    fn new(form: ResidentForm, key: Option<String>, reason: impl Into<String>) -> Self {
        Self {
            form,
            key,
            reasons: vec![reason.into()],
        }
    }
}

impl fmt::Display for ResidentBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "常驻房间模型（{}{}）建不起来，本轮止步、存量边不动: {}",
            self.form.as_str(),
            self.key
                .as_deref()
                .map(|key| format!(" {key}"))
                .unwrap_or_default(),
            self.reasons.join("; ")
        )
    }
}

impl std::error::Error for ResidentBuildError {}

/// 实时行那一份的身份键：按文件 dbnum 序的 `(dbnum, 分组摘要)`。没有令牌可摘时用它——
/// 只用来判「同一份输入」，不参与任何跨进程比对。
fn live_generation_key(by_file: &[(u32, Vec<RoomGroup>)]) -> String {
    let mut ordered: Vec<&(u32, Vec<RoomGroup>)> = by_file.iter().collect();
    ordered.sort_by_key(|(dbnum, _)| *dbnum);
    let mut digest = Sha256::new();
    digest.update(b"resident-live-generation-v1");
    for (dbnum, groups) in ordered {
        digest.update(dbnum.to_le_bytes());
        digest.update(topology_digest(groups).as_bytes());
    }
    hex::encode(digest.finalize())
}

/// 按文件分好的分组 → 房间 → 文件 dbnum。同一个房间在两个文件里都有根是不合法的
/// （`compose` 的 `duplicate physical RefNo` 已经拦住），这里取先出现的那个。
fn room_files_of(by_file: &[(u32, Vec<RoomGroup>)]) -> BTreeMap<RefnoEnum, u32> {
    let mut room_files = BTreeMap::new();
    for (dbnum, groups) in by_file {
        for group in groups {
            room_files.entry(group.room).or_insert(*dbnum);
        }
    }
    room_files
}

/// 摄入侧生产入口。`None` = 还没有发布清单（升级基线未建），调用方保留今天的路。
pub(crate) async fn load_published(
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
    meshes: Arc<MeshCache>,
) -> anyhow::Result<Option<ResidentRoomModel>> {
    load_file_backed(ResidentForm::Published, hierarchy, room_key_words, match_room_fn, meshes)
        .await.map(Some)
}

/// Current MDB room inputs come exclusively from frozen E3D sources and local
/// generation artifacts. A missing artifact regenerates only that panel root.
pub(crate) async fn load_latest(
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
    meshes: Arc<MeshCache>,
) -> anyhow::Result<ResidentRoomModel> {
    load_file_backed(
        ResidentForm::Latest,
        hierarchy,
        room_key_words,
        match_room_fn,
        meshes,
    )
    .await
}

/// Build the calculation view from frozen MDB files and e3d-model output for
/// either serving face. `form` is only the resident slot selected by callers;
/// topology and panel geometry never come from the database.
async fn load_file_backed(
    form: ResidentForm,
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
    meshes: Arc<MeshCache>,
) -> anyhow::Result<ResidentRoomModel> {
    let crate::fast_model::room_topology::RoomGroupsFromFiles {
        composite,
        tokens,
        by_file,
        missing_design,
    } = crate::fast_model::room_topology::load_room_groups_from_files(hierarchy)
        .await
        .map_err(|error| {
            ResidentBuildError::new(form, None, format!("读文件侧房间分组失败: {error:#}"))
        })?;
    let room_files = room_files_of(&by_file);
    let groups = crate::fast_model::room_topology::flatten_groups_by_file(by_file);
    let map = registered_map(&groups, room_key_words, match_room_fn);
    let preload = preload_panel_projection(&tokens, &map, &room_files).await?;
    let expected_dbnums = tokens
        .iter()
        .filter_map(|token| {
            crate::data_interface::frozen_source::FrozenSource::from_commit_token_json(token)
                .ok()
                .map(|source| source.dbnum() as u32)
        })
        .collect();
    Ok(ResidentRoomModel::from_projection(
        ResidentManifest {
            form,
            key: composite,
        },
        groups,
        map,
        &preload.records,
        meshes,
    )
    .with_source_tokens(tokens)
    .with_expected_dbnums(expected_dbnums)
    .with_room_files(room_files)
    .with_missing_dbnums(missing_design)
    .with_problems(preload.problems))
}

/// [`preload_panel_projection`] 的回执：拿到几何的面板投影行，以及没拿到的那些的缺口记录。
struct PanelPreload {
    records: Vec<crate::fast_model::model_memory_store::ProjectedGeometry>,
    /// 一块面板一条（或一个文件一条，当整个文件的 E3D 服务都起不来时）：
    /// `dbnum=… panel=… stage=panel-preload reason=…`。没拿到几何的面板在 `assemble` 里
    /// 自然落进 `missing_panels`（一条几何行都没有），这里只补原因。
    problems: Vec<String>,
}

/// 在册面板的几何：磁盘工件命中就用，没命中的按文件调 e3d-model 现生成。
///
/// **一块面板生成失败不再整份 Err**（ADR-073 D9，2026-09-16）：记一条缺口、这块面板没有几何行
/// → `missing_panels` → 保护集；一个文件的 E3D 服务起不来（缺 CATA 依赖等）也只让这个文件的
/// 全部在册面板缺几何、记一条缺口。此前任一失败都让 `try_join` 整份 Err → resident `Blocked`
/// → 之后每个任务的全部根判死。仍是 Err 的：MDB 来源枚举不出、令牌重开校验失败、生产者本身
/// 崩掉（不是逐根的失败回执而是 `generate_frozen_roots` 自己 Err）——房间体制没起来。
async fn preload_panel_projection(
    tokens: &[String],
    map: &RoomPanelMap,
    room_files: &BTreeMap<RefnoEnum, u32>,
) -> anyhow::Result<PanelPreload> {
    use crate::data_interface::frozen_source::FrozenSource;
    use crate::fast_model::{e3d_model_service::E3dModelService, model_artifact};
    let mesh_dir = aios_core::get_db_option().get_meshes_path();
    let cache_dir = mesh_dir.join("room-input-cache");
    let mut dependencies = tokens.to_vec();
    for pin in crate::fast_model::e3d_model_service::current_mdb_sources()?.0 {
        if !pin.db_type.eq_ignore_ascii_case("DESI") {
            dependencies.push(FrozenSource::capture(pin.file)?.commit_token_json()?);
        }
    }
    dependencies.sort();
    dependencies.dedup();
    let mut by_file = BTreeMap::<u32, BTreeSet<RefnoEnum>>::new();
    for room in &map.rooms {
        let dbnum = room_files
            .get(&room.room)
            .context("room definition has no source file")?;
        by_file
            .entry(*dbnum)
            .or_default()
            .extend(room.panels.iter().copied());
    }
    let mut records = Vec::new();
    let mut problems = Vec::new();
    for token in tokens {
        let source = FrozenSource::from_commit_token_json(token)?.reopen_verified()?;
        let dbnum = source.dbnum() as u32;
        let Some(panels) = by_file.remove(&dbnum) else {
            continue;
        };
        let started = Instant::now();
        let service = match E3dModelService::from_initialization_sources(&source, &dependencies)
            .await
        {
            Ok(service) => service,
            Err(error) => {
                // 这个文件的 E3D 服务起不来：它的在册面板全部缺几何，一条缺口说清原因。
                problems.push(format!(
                    "dbnum={dbnum} panels={} stage=panel-preload reason=e3d-service-unavailable: {error:#}",
                    panels.len()
                ));
                continue;
            }
        };
        let input = service.initialization_target_hash(dbnum, source.target_sesno())?;
        let mut missing = Vec::new();
        let mut cached = 0;
        let before = records.len();
        for panel in &panels {
            let root = crate::fast_model::e3d_model_service::parse_refno(&panel.to_string())?;
            if let Some(commit) = model_artifact::load(&cache_dir, &mesh_dir, &input, root) {
                records.extend(commit.upserts);
                cached += 1;
            } else {
                missing.push(root.to_string());
            }
        }
        let generated = missing.len();
        let mut failed = Vec::<(String, String)>::new();
        if !missing.is_empty() {
            let (tx, mut rx) = tokio::sync::mpsc::channel(1);
            let produce = service.generate_frozen_roots(dbnum, &missing, tx);
            let consume =
                async {
                    let mut received = BTreeSet::new();
                    while let Some(result) = rx.recv().await {
                        anyhow::ensure!(
                            received.insert(result.root.to_string()),
                            "duplicate room panel output"
                        );
                        let commit = match result.outcome {
                            Ok(commit) => commit,
                            Err(reason) => {
                                // 这一块面板算不出几何：留痕、不阻断（D9）。
                                failed.push((result.root.to_string(), reason.to_string()));
                                continue;
                            }
                        };
                        anyhow::ensure!(
                            commit.upserts.iter().all(|row| row.dbnum == dbnum
                                && row.source_sesno == source.target_sesno()),
                            "room panel output version mismatch"
                        );
                        model_artifact::store(&cache_dir, &input, result.root, &commit)?;
                        records.extend(commit.upserts);
                    }
                    // 生产者没交回执的根同样算失败：没有几何行就是缺几何，原因写「没有回执」。
                    for root in &missing {
                        if !received.contains(root) {
                            failed.push((root.clone(), "no generation receipt".to_string()));
                        }
                    }
                    anyhow::Ok(())
                };
            futures::future::try_join(produce, consume).await?;
            // Heavy source indexes belong to the producer; room inputs now own
            // lightweight projection rows and immutable mesh references.
            crate::fast_model::e3d_model_service::forget_generation_input(dbnum);
        }
        source.reopen_verified()?;
        failed.sort();
        for (root, reason) in &failed {
            problems.push(format!(
                "dbnum={dbnum} panel={root} stage=panel-preload reason={reason}"
            ));
        }
        println!(
            "[ams-phase] phase=room_panel_preload dbnum={dbnum} sesno={} panels={} generated={generated} cached={cached} failed={} records={} elapsed_ms={}",
            source.target_sesno(),
            panels.len(),
            failed.len(),
            records.len() - before,
            started.elapsed().as_millis()
        );
    }
    anyhow::ensure!(
        by_file.is_empty(),
        "room definitions reference an unselected source file"
    );
    for token in &dependencies {
        FrozenSource::from_commit_token_json(token)?.reopen_verified()?;
    }
    Ok(PanelPreload { records, problems })
}

// ---------------------------------------------------------------------------
// 启动参照（Q9 (a)）与启动对比（Q3 / Q6 / Q7；臂表按共识 d-42，
// `docs/plans/2026-09-08-room-startup-compare-and-apply-plan.md` §4.1 / §4.3）。
// 应用侧（按 diff 删 / 重算 / 改列、接进 `lib.rs` 启动段）在 `room_startup.rs`（S2 / S3）。
// ---------------------------------------------------------------------------

/// 上次成功结算所依据的那组文件版本与拓扑摘要，落在 `room_build:main.resident_reference`。
///
/// **两形态同一条路**（d-42）：`tokens` 是那一份拓扑依据的每个文件版本——两形态都是各 DESI
/// 的冻结令牌；启动对比按它们从 `room_topology_source` 的磁盘分片合成「旧」那份再 diff。
/// `key` 是形态自己的身份键（两形态都是文件版本 composite），只用来判「同一组文件版本、
/// 拓扑却不同 = 库被恢复 / 手改」；`digest` 是内容摘要，先比它。
/// 三样都不是时间：`settled_at` 只给 `/health` 与日志看，不参与判定。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ResidentReference {
    pub form: String,
    pub key: String,
    pub digest: String,
    #[serde(default)]
    pub tokens: Vec<String>,
    pub rooms: usize,
    pub registered_panels: usize,
    pub settled_at: String,
}

impl ResidentReference {
    pub(crate) fn for_model(model: &ResidentRoomModel) -> Self {
        Self {
            form: model.manifest.form.as_str().to_string(),
            key: model.manifest.key.clone(),
            digest: model.digest.clone(),
            tokens: model.source_tokens.clone(),
            rooms: model.map.rooms.len(),
            registered_panels: registered_panels(&model.map).len(),
            settled_at: chrono::Local::now().to_rfc3339(),
        }
    }

    pub(crate) fn form(&self) -> Option<ResidentForm> {
        match self.form.as_str() {
            "published" => Some(ResidentForm::Published),
            "latest" => Some(ResidentForm::Latest),
            _ => None,
        }
    }
}

/// `UPSERT` 只动 `resident_reference` 一格：`room_build:main` 上的空间凭据
/// （`spatial_epoch` / `tree_entries` / `rebuild_required`）由 `room_model` 管，这里不碰。
pub(crate) fn render_resident_reference_write(reference: &ResidentReference) -> String {
    let json = serde_json::to_string(reference).expect("resident reference serializes");
    format!("UPSERT room_build:main SET resident_reference = {json};")
}

pub(crate) async fn write_resident_reference_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    reference: &ResidentReference,
) -> anyhow::Result<()> {
    db.query(render_resident_reference_write(reference))
        .await?
        .check()?;
    Ok(())
}

pub(crate) async fn read_resident_reference_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
) -> anyhow::Result<Option<ResidentReference>> {
    let mut response = db
        .query("SELECT VALUE resident_reference FROM room_build:main;")
        .await?
        .check()?;
    let rows: Vec<Option<ResidentReference>> = response.take(0)?;
    Ok(rows.into_iter().flatten().next())
}


/// 启动该做什么（Q7 (a)：全量降为显式修复入口，其余走对比；臂表 d-42，两形态同一张）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupRoomPlan {
    /// 没有参照（开关第一次打开 / 库刚建 / 形态换了）：按今天的口径跑一次全量并写参照。
    NoReference,
    /// 参照摘要 == 现状摘要：零写入，参照不动（D2：不刷 `settled_at`，观测走进程内那一格）。
    Unchanged,
    /// 文件版本变了（`key` 不同）、旧视图能从分片合成、摘要不同：按 diff 删 / 重算 / 改列。
    Apply(TopologyDiff),
    /// **同一组文件版本**（`key` 相等）拓扑摘要却不同：分片 / 拓扑行被恢复或手改。不猜，标
    /// `rebuild_required` 走显式全量。两形态同判。
    IntegrityMismatch { expected: String, actual: String },
    /// 参照在但旧视图读不回来（分片缺失 / 校验失败 / `compose` 失败）：不猜，走显式全量。
    ReferenceUnreadable(String),
}

impl StartupRoomPlan {
    pub(crate) fn summary(&self) -> String {
        match self {
            Self::NoReference => "无参照 → 全量一次并写参照".into(),
            Self::Unchanged => "参照摘要 == 现状 → 零写入".into(),
            Self::Apply(diff) => format!("对比 → {}", diff.summary()),
            Self::IntegrityMismatch { expected, actual } => format!(
                "同一组文件版本拓扑摘要却不等（{} ≠ {}）：库被恢复 / 手改 → rebuild_required",
                &expected[..expected.len().min(12)],
                &actual[..actual.len().min(12)]
            ),
            Self::ReferenceUnreadable(reason) => {
                format!("参照不可读（{reason}）→ rebuild_required")
            }
        }
    }
}

/// 纯函数：参照 + 现状 + 参照合成的旧视图 → 计划。三层版本按序比（d-42）：先比内容摘要
/// （`digest`），相等就零写入；不等再看身份键（`key`）——同键 = 同一组文件版本拓扑却不同，
/// 只能是库被动过；键不同才是真的有文件前进，拿 `reference_map`（调用方按参照令牌从分片合成
/// 的旧视图）diff。`reference_map` 只在走到最后一臂时才被看；读不回来传 `Err(原因)`。
pub(crate) fn plan_startup(
    reference: Option<&ResidentReference>,
    current: &ResidentRoomModel,
    reference_map: Option<Result<&RoomPanelMap, String>>,
) -> StartupRoomPlan {
    let Some(reference) = reference else {
        return StartupRoomPlan::NoReference;
    };
    if reference.form() != Some(current.manifest.form) {
        // 换了形态（配置从摄入切到读透或反过来）：参照说的不是这一路，等同于没有。
        return StartupRoomPlan::NoReference;
    }
    if reference.digest == current.digest {
        return StartupRoomPlan::Unchanged;
    }
    if reference.key == current.manifest.key {
        return StartupRoomPlan::IntegrityMismatch {
            expected: reference.digest.clone(),
            actual: current.digest.clone(),
        };
    }
    match reference_map {
        Some(Ok(old)) => StartupRoomPlan::Apply(TopologyDiff::between(old, &current.map)),
        Some(Err(reason)) => StartupRoomPlan::ReferenceUnreadable(reason),
        None => StartupRoomPlan::ReferenceUnreadable("对比需要参照合成的旧视图，调用方没给".into()),
    }
}

/// 改列（Q6 第 3 条）：一间房只改了房间号，交集里每块面板的 `room_relate` 出边与
/// `room_panel_relate` 入边只改 `room_num`，边 id 不动、不重算。走图遍历取边
/// （`{panel}->room_relate` / `{panel}<-room_panel_relate`），与删除侧同一条纪律。
pub(crate) fn render_renumber_statements(renumber: &RoomRenumber) -> Vec<String> {
    let old = crate::data_interface::dbnum_state::escape_surql_str(&renumber.old_num);
    let new = crate::data_interface::dbnum_state::escape_surql_str(&renumber.new_num);
    let mut statements = Vec::with_capacity(renumber.panels.len() * 2);
    for panel in &renumber.panels {
        let key = panel.to_pe_key();
        statements.push(format!(
            "UPDATE (SELECT VALUE id FROM {key}->room_relate WHERE room_num = '{old}') SET room_num = '{new}';"
        ));
        statements.push(format!(
            "UPDATE (SELECT VALUE id FROM {key}<-room_panel_relate WHERE room_num = '{old}') SET room_num = '{new}';"
        ));
    }
    statements
}

#[derive(Default)]
struct Generations {
    current: Option<Arc<ResidentRoomModel>>,
    previous: Option<Arc<ResidentRoomModel>>,
}

/// 一次换代的结果：装进去的那份，以及与上一代的拓扑差（第一代没有上一代 → `None`）。
pub(crate) struct InstallOutcome {
    pub model: Arc<ResidentRoomModel>,
    pub diff: Option<TopologyDiff>,
}

/// 进程内的两路模型，各留当前 + 上一代（diff 只需两份，工程默认②）。
#[derive(Default)]
pub(crate) struct ResidentRooms {
    published: Generations,
    latest: Generations,
    meshes: Arc<MeshCache>,
}

impl ResidentRooms {
    /// 建模型时用的共享网格缓存——不从这里拿、自己 `Default` 一份的模型换代后网格全部重建。
    pub(crate) fn mesh_cache(&self) -> Arc<MeshCache> {
        self.meshes.clone()
    }

    fn slot(&self, form: ResidentForm) -> &Generations {
        match form {
            ResidentForm::Published => &self.published,
            ResidentForm::Latest => &self.latest,
        }
    }

    fn slot_mut(&mut self, form: ResidentForm) -> &mut Generations {
        match form {
            ResidentForm::Published => &mut self.published,
            ResidentForm::Latest => &mut self.latest,
        }
    }

    /// **归属路径唯一允许的取法。** 摄入 → `published`，读透 → `latest`；摄入形态拿不到
    /// `latest`，这是原则 1 的落点。
    pub(crate) fn for_membership(&self, face: DataFace) -> Option<Arc<ResidentRoomModel>> {
        self.slot(ResidentForm::for_face(face)).current.clone()
    }

    /// 这一路的当前模型是不是给定身份的那份——换代点拿它判要不要重建。
    pub(crate) fn is_current(&self, manifest: &ResidentManifest) -> bool {
        self.slot(manifest.form)
            .current
            .as_ref()
            .is_some_and(|model| model.manifest == *manifest)
    }

    /// 整份换代：新的进当前、当前退上一代、再往前的丢掉。返回与上一代的拓扑差。
    pub(crate) fn install(&mut self, model: ResidentRoomModel) -> InstallOutcome {
        let model = Arc::new(model);
        let slot = self.slot_mut(model.manifest.form);
        let diff = slot
            .current
            .as_ref()
            .map(|old| TopologyDiff::between(&old.map, &model.map));
        slot.previous = slot.current.take();
        slot.current = Some(model.clone());
        self.prune_meshes();
        InstallOutcome { model, diff }
    }

    fn prune_meshes(&self) {
        let live = [&self.published, &self.latest]
            .into_iter()
            .flat_map(|generations| [&generations.current, &generations.previous])
            .filter_map(|generation| generation.as_ref())
            .flat_map(|generation| generation.mesh_keys())
            .collect();
        self.meshes.retain(&live);
    }

    /// 只给 `/health`、启动对比与护栏看两代；归属路径不许碰（源码钉
    /// `membership_paths_only_take_the_model_through_for_membership`）。
    pub(crate) fn generations(
        &self,
        form: ResidentForm,
    ) -> (
        Option<Arc<ResidentRoomModel>>,
        Option<Arc<ResidentRoomModel>>,
    ) {
        let slot = self.slot(form);
        (slot.current.clone(), slot.previous.clone())
    }

    pub(crate) fn stats(&self) -> Vec<ResidentStats> {
        [ResidentForm::Published, ResidentForm::Latest]
            .into_iter()
            .filter_map(|form| self.slot(form).current.as_ref().map(|model| model.stats()))
            .collect()
    }
}

/// 进程级单例。锁只在取放 `Arc` 的那一瞬持有，不跨 `await`。
pub(crate) fn resident_rooms() -> &'static RwLock<ResidentRooms> {
    static ROOMS: OnceLock<RwLock<ResidentRooms>> = OnceLock::new();
    ROOMS.get_or_init(|| RwLock::new(ResidentRooms::default()))
}

/// Fixed, already-preloaded room input for a dbnum Task. This accessor performs
/// no file or database I/O; startup (or a prior generation swap) owns rebuilding it.
pub(crate) fn current_for_calculation() -> anyhow::Result<Arc<ResidentRoomModel>> {
    ensure_ready()?;
    resident_rooms()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .for_membership(crate::options::data_face())
        .context("resident room model is not installed")
}

async fn warm_for_install(
    model: &mut ResidentRoomModel,
    mesh_dir: &Path,
) -> Result<(), ResidentBuildError> {
    match model.warm_meshes(mesh_dir).await {
        Ok(()) => Ok(()),
        Err(error) => {
            resident_rooms()
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .prune_meshes();
            Err(error)
        }
    }
}

// ---------------------------------------------------------------------------
// 换代点（P2）：每轮 drain 进串行锁之后按身份键懒判，键变了才整份重建。
// ---------------------------------------------------------------------------

/// [`ensure_current`] 的回执。
pub(crate) struct EnsureOutcome {
    /// 这一轮归属路径要用的那份模型。
    pub model: Arc<ResidentRoomModel>,
    /// 这一次是不是真换了代（`false` = 身份键没变，复用的是上一轮那份）。
    pub rebuilt: bool,
    /// 换了代时与上一代的拓扑差；没换代、或这是第一代，都是 `None`（P4 护栏读它）。
    pub diff: Option<TopologyDiff>,
}

/// 身份探针：只算「此刻这一路是哪一组文件版本」，不读拓扑、不读几何。
///
/// 两形态同一把键：各 DESI 的冻结令牌摘要（`room_topology::latest_frozen_sources`），与
/// [`load_latest`] / [`load_published`] 的 composite 同源（发布清单键随 ADR-062 作废）。
/// 不许在这里另起一套键——键漂了就是每轮都以为文件变了、每轮整份重建。
///
/// 回 `Option` 是历史签名：如今两形态都能算出键，不再有「没有发布清单」这一档。
async fn probe_manifest(
    form: ResidentForm,
    hierarchy: RoomHierarchy,
) -> anyhow::Result<Option<ResidentManifest>> {
    let _ = hierarchy;
    // Both faces use frozen E3D sources, never a database geometry snapshot.
    let key = crate::fast_model::room_topology::latest_frozen_sources()
        .await?
        .composite;
    Ok(Some(ResidentManifest { form, key }))
}

/// 换代点（P2）：把这一路的当前模型对齐到此刻那一组文件版本，交回归属路径要用的那份。
///
/// **调用方必须已持 `spatial_state::lock_spatial_serial()`**（源码钉
/// `the_generation_swap_happens_inside_the_spatial_serial_lock`）。换代、本轮读任务、写归属边
/// 要落在同一段串行区里：算到一半模型被换掉就是拿旧拓扑写新边，而且两边都不报错。
///
/// 身份键没变是常路——除探针那一次查询 / 一遍文件版本，不再有任何 I/O，直接复用当前那份
/// `Arc`。键变了才整份重建（原则 1：不逐条修补），新的进当前、当前退上一代。
///
/// `None` = 摄入侧没有发布清单，**不是失败**：调用方回到今天的读法。建不起来才是 `Err`
/// （[`ResidentBuildError`]），那时按原则 2 止步、不动存量边。
pub(crate) async fn ensure_current(
    face: DataFace,
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
    mesh_dir: &Path,
) -> anyhow::Result<Option<EnsureOutcome>> {
    let result =
        ensure_current_inner(face, hierarchy, room_key_words, match_room_fn, mesh_dir).await;
    if let Err(error) = &result {
        mark_failed(error);
    }
    result
}

async fn ensure_current_inner(
    face: DataFace,
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
    mesh_dir: &Path,
) -> anyhow::Result<Option<EnsureOutcome>> {
    let form = ResidentForm::for_face(face);
    let Some(manifest) = probe_manifest(form, hierarchy).await? else {
        return Err(ResidentBuildError::new(
            form,
            None,
            "发布清单为空，resident generation 不可用",
        )
        .into());
    };
    let (current, meshes) = {
        let rooms = resident_rooms()
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let current = rooms
            .is_current(&manifest)
            .then(|| rooms.for_membership(face))
            .flatten();
        (current, rooms.mesh_cache())
    };
    if let Some(model) = current {
        mark_ready(&model);
        return Ok(Some(EnsureOutcome {
            model,
            rebuilt: false,
            diff: None,
        }));
    }
    let mut model = match form {
        ResidentForm::Published => {
            match load_published(hierarchy, room_key_words, match_room_fn, meshes).await? {
                Some(model) => model,
                // 探针刚看见的清单在这一瞬空了（基线正在重建）：不装零间房的模型（原则 2）。
                None => {
                    return Err(ResidentBuildError::new(
                        form,
                        Some(manifest.key.clone()),
                        "探针之后发布清单消失，拒绝安装或复用旧代",
                    )
                    .into());
                }
            }
        }
        ResidentForm::Latest => {
            load_latest(hierarchy, room_key_words, match_room_fn, meshes).await?
        }
    };
    mark_loading("mesh-warm");
    if let Err(error) = warm_for_install(&mut model, mesh_dir).await {
        let error = anyhow::Error::new(error);
        return Err(error);
    }
    let outcome = resident_rooms()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .install(model);
    mark_ready(&outcome.model);
    Ok(Some(EnsureOutcome {
        model: outcome.model,
        rebuilt: true,
        diff: outcome.diff,
    }))
}

// ---------------------------------------------------------------------------
// 房间初始化（第 5 轮 Q17–Q20）：启动最前段建模，MDB 全部 DESI，不看 watch_dbnums。
// `lib.rs::initialize_resident_rooms_at_startup` 在空间树装载之后、watcher / worker 之前调
// [`initialize_at_startup`]（P3-b）；门只有 `room_resident_model` 开关与 `spatial_compute`。
// ---------------------------------------------------------------------------

/// 一个 MDB 设计库文件在本次初始化里清单行的去向（摄入形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManifestAction {
    /// 归文件轮次管（被监听）：清单行一个字不动，哪怕陈旧——它的几何随轮次一起发布，
    /// 替它前进 token 会把陈旧几何贴上新标签（ADR-062）。
    OwnedByRounds,
    /// 不归轮次管、清单行已在文件最新会话：保留。
    Current,
    /// 不归轮次管、清单里没有这一行：新增到文件最新会话。
    Added,
    /// 不归轮次管、清单行落后于文件最新会话：前进（Q19 (a)）。
    Advanced { from_sesno: i32 },
}

impl ManifestAction {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::OwnedByRounds => "归轮次",
            Self::Current => "保留",
            Self::Added => "新增",
            Self::Advanced { .. } => "前进",
        }
    }

    fn moves(&self) -> bool {
        matches!(self, Self::Added | Self::Advanced { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestStep {
    pub dbnum: u32,
    pub path: PathBuf,
    /// 文件此刻的最新会话（`design_sources()` 给的）。
    pub sesno: u32,
    pub action: ManifestAction,
}

/// 一次启动初始化的回执。
#[derive(Debug)]
pub(crate) struct StartupInitOutcome {
    pub form: ResidentForm,
    /// 摄入：每个 MDB DESI 的清单去向；读透：空（读透没有清单）。
    pub steps: Vec<ManifestStep>,
    /// 摄入：这次真的前进 / 新增了的行 `(dbnum, sesno, 冻结耗时 ms)`；读透：每个令牌的冻结耗时。
    pub frozen: Vec<(u32, u32, u128)>,
    /// 摄入：清单前进没按计划走完的话——事务没提交（基线没建成 / 有在途文件轮次 / 冻结失败，
    /// 此时 `frozen` 为空、清单还是旧的）或事务已提交而快照头没持久化（`frozen` 已填、清单是
    /// 新的）。两种都不拦建模。
    pub advance_warning: Option<String>,
    pub model: Arc<ResidentRoomModel>,
    pub elapsed_ms: u128,
}

impl StartupInitOutcome {
    /// 给启动日志的一行：每个文件 `dbnum@sesno 动作`，再是模型统计。
    pub(crate) fn summary(&self) -> String {
        let files = if self.steps.is_empty() {
            self.frozen
                .iter()
                .map(|(dbnum, sesno, ms)| format!("{dbnum}@{sesno} 冻结 {ms}ms"))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            self.steps
                .iter()
                .map(|step| {
                    let ms = self
                        .frozen
                        .iter()
                        .find(|(dbnum, _, _)| *dbnum == step.dbnum)
                        .map(|(_, _, ms)| format!(" {ms}ms"))
                        .unwrap_or_default();
                    match &step.action {
                        ManifestAction::Advanced { from_sesno } => {
                            format!("{}@{}→{} 前进{ms}", step.dbnum, from_sesno, step.sesno)
                        }
                        action => format!("{}@{} {}{ms}", step.dbnum, step.sesno, action.as_str()),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let stats = self.model.stats();
        let missing = if stats.missing_panels_by_dbnum.is_empty() {
            String::new()
        } else {
            format!(
                " 无几何按库 {{{}}}",
                stats
                    .missing_panels_by_dbnum
                    .iter()
                    .map(|(dbnum, count)| format!("{dbnum}: {count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let missing_dbnums = if stats.missing_dbnums.is_empty() {
            String::new()
        } else {
            format!(
                " / 缺文件的 DESI {:?}（不在册，ADR-073 D8）",
                stats.missing_dbnums
            )
        };
        let degraded = if stats.problems == 0 {
            String::new()
        } else {
            format!(
                " → degraded：{} 条缺口（计算器照常冻，缺几何面板的存量边受保护；明细见 /health.room_build.resident.problems）",
                stats.problems
            )
        };
        format!(
            "[房间初始化] {} 文件 [{}]{} → {} 间房 / {} 块在册面板 / {} 块有几何 / {} 块无几何{}{} / 网格 {}/{} 键 {:.1} MB{} 耗时 {}ms{}",
            self.form.as_str(),
            files,
            self.advance_warning
                .as_deref()
                .map(|warning| format!("（{warning}）"))
                .unwrap_or_default(),
            stats.rooms,
            stats.registered_panels,
            stats.panels_with_geometry,
            stats.missing_panels,
            missing,
            missing_dbnums,
            stats.mesh_keys_warmed,
            stats.mesh_keys_expected,
            stats.mesh_bytes as f64 / (1024.0 * 1024.0),
            if stats.mesh_warm_failed > 0 {
                format!("（预热失败 {}）", stats.mesh_warm_failed)
            } else {
                String::new()
            },
            self.elapsed_ms,
            degraded
        )
    }

    /// 这次初始化覆盖到的 MDB 设计库号（`/health.mdb_dbnums_expected` 的来源）：摄入 =
    /// 清单去向表里的每个文件；读透 = 冻结令牌的每个文件。启动只探一次（§2.1），进程内
    /// 之后的换代不再重新枚举 MDB。
    pub(crate) fn expected_dbnums(&self) -> Vec<u32> {
        if self.steps.is_empty() {
            self.frozen.iter().map(|(dbnum, _, _)| *dbnum).collect()
        } else {
            self.steps.iter().map(|step| step.dbnum).collect()
        }
    }
}

/// 启动房间初始化（Q17 (a)）。只做建模与写分片 / 清单，**不写归属边、不写 `resident_reference`**。
///
/// 调用方（`lib.rs::initialize_resident_rooms_at_startup`）只在 `spatial_compute` 开着时调，其余门
/// 一道不看（Q18 (a)）；`ownership` 是它按 `watch_scope` 算好的「哪些库归文件轮次管」——本模块
/// 不认识 `watch_scope`。建不起来整份 `Err`（[`ResidentBuildError`] 可 `downcast_ref`），调用方
/// 记 `resident_blocked`、不动边。
pub(crate) async fn initialize_at_startup(
    face: DataFace,
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
    mesh_dir: &Path,
) -> anyhow::Result<StartupInitOutcome> {
    initialize_file_backed_at_startup(
        ResidentForm::for_face(face),
        hierarchy,
        room_key_words,
        match_room_fn,
        mesh_dir,
    )
    .await
}

/// Both ingest and read-through prebuild the room reference directly from the
/// frozen MDB files. Database state remains an output/credential store only.
async fn initialize_file_backed_at_startup(
    form: ResidentForm,
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
    mesh_dir: &Path,
) -> anyhow::Result<StartupInitOutcome> {
    let started = Instant::now();
    let meshes = resident_rooms()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .mesh_cache();
    let mut model =
        load_file_backed(form, hierarchy, room_key_words, match_room_fn, meshes).await?;
    // 拓扑分片（`room_topology_source`）随房间流程退役（t-678）：没有分片要冻，回执那一格空着。
    let frozen = Vec::new();
    warm_for_install(&mut model, mesh_dir).await?;
    let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
    let model = resident_rooms()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .install(model)
        .model;
    Ok(StartupInitOutcome {
        form,
        steps: Vec::new(),
        frozen,
        advance_warning: None,
        model,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

/// 读透形态：`load_latest`（本就全 MDB）→ 每个令牌冻分片 → 装进单例。读透没有清单，
/// `owned_by_rounds` 用不上。
async fn initialize_latest_at_startup(
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
    mesh_dir: &Path,
) -> anyhow::Result<StartupInitOutcome> {
    let started = Instant::now();
    let meshes = resident_rooms()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .mesh_cache();
    let mut model = load_latest(hierarchy, room_key_words, match_room_fn, meshes).await?;
    // 见 `initialize_file_backed_at_startup`：分片退役，回执那一格空着。
    let frozen = Vec::new();
    warm_for_install(&mut model, mesh_dir).await?;
    let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
    let model = resident_rooms()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .install(model)
        .model;
    Ok(StartupInitOutcome {
        form: ResidentForm::Latest,
        steps: Vec::new(),
        frozen,
        advance_warning: None,
        model,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

// ---------------------------------------------------------------------------
// 启动初始化的结局：进程内一份给 /health，`room_build:main.resident_blocked` 一格给事后查库。
// ---------------------------------------------------------------------------

pub(crate) const RESIDENT_MODEL_UPDATING: &str = "RESIDENT_MODEL_UPDATING";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub(crate) enum ResidentLifecycle {
    Disabled {
        source: String,
    },
    Loading {
        phase: String,
        started_at: String,
    },
    Ready {
        generation: String,
        completed_at: String,
    },
    Degraded {
        generation: String,
        problems: Vec<String>,
    },
    Blocked {
        problems: Vec<String>,
    },
    Unsupported {
        reason: String,
    },
}

fn lifecycle_slot() -> &'static RwLock<ResidentLifecycle> {
    static STATE: OnceLock<RwLock<ResidentLifecycle>> = OnceLock::new();
    STATE.get_or_init(|| {
        RwLock::new(ResidentLifecycle::Disabled {
            source: "startup-not-evaluated".to_string(),
        })
    })
}

pub(crate) fn lifecycle() -> ResidentLifecycle {
    lifecycle_slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn set_lifecycle(state: ResidentLifecycle) {
    *lifecycle_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner) = state;
}

pub(crate) fn mark_disabled(source: impl Into<String>) {
    set_lifecycle(ResidentLifecycle::Disabled {
        source: source.into(),
    });
}

pub(crate) fn mark_unsupported(reason: impl Into<String>) {
    set_lifecycle(ResidentLifecycle::Unsupported {
        reason: reason.into(),
    });
}

pub(crate) fn mark_loading(phase: impl Into<String>) {
    set_lifecycle(ResidentLifecycle::Loading {
        phase: phase.into(),
        started_at: chrono::Local::now().to_rfc3339(),
    });
}

/// 一份装好的模型对应的生命周期（ADR-073 D8–D10）：没有缺口 = `Ready`；带缺口 = `Degraded`
/// ——模型在、计算器照常冻，只是 `problems` 点名的面板 / 库不在承诺内。纯函数，`/health` 与
/// 单测直接喂。`generation` 是**计算输入的摘要**（[`ResidentRoomModel::input_version`]，与
/// `MemoryRoomCalculator::resident_generation` 同一个字串），不是文件版本 composite（D12）。
pub(crate) fn lifecycle_for(model: &ResidentRoomModel) -> ResidentLifecycle {
    if model.problems.is_empty() {
        ResidentLifecycle::Ready {
            generation: model.input_version(),
            completed_at: chrono::Local::now().to_rfc3339(),
        }
    } else {
        ResidentLifecycle::Degraded {
            generation: model.input_version(),
            problems: model.problems.clone(),
        }
    }
}

/// 重建没成、手上还有上一份时的生命周期（ADR-073 D12）：留着旧的继续算，`Degraded` 里多一条
/// `stage=refresh` 的缺口——同一条只留最新一次，不随每个任务开工一次次堆。
pub(crate) fn lifecycle_after_refresh_failure(
    previous: &ResidentRoomModel,
    error: &str,
) -> ResidentLifecycle {
    let mut problems: Vec<String> = previous
        .problems
        .iter()
        .filter(|problem| !problem.starts_with(REFRESH_PROBLEM_PREFIX))
        .cloned()
        .collect();
    problems.push(format!(
        "{REFRESH_PROBLEM_PREFIX} reason=resident refresh failed, calculating with the generation built at startup or last refresh: {error}"
    ));
    ResidentLifecycle::Degraded {
        generation: previous.input_version(),
        problems,
    }
}

const REFRESH_PROBLEM_PREFIX: &str = "stage=refresh";

fn mark_ready(model: &ResidentRoomModel) {
    set_lifecycle(lifecycle_for(model));
}

/// 建模本体失败 = `Blocked`：没有可信的当前代。`Degraded` 从 2026-09-16 起专指「装好了、带缺口」
/// （[`lifecycle_for`]），不再兼任「上一代还在、新代没建成」——那一档只有换代点会产生，而换代点
/// 已无生产调用方；即便有，§4.2 也不许拿旧代为新会话写边，所以这里不看有没有上一代。
fn mark_failed(error: &anyhow::Error) {
    record_failure(format!("{error:#}"));
    set_lifecycle(ResidentLifecycle::Blocked {
        problems: vec![format!("{error:#}")],
    });
}

/// 本进程最近一次 resident 构建 / 换代失败（`/health.room_build.resident.last_failure`）。
/// 只记最近一条：逐条历史在日志里，这一格回答的是「上一次为什么没成」。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ResidentFailure {
    pub reason: String,
    pub at: String,
}

fn last_failure_slot() -> &'static RwLock<Option<ResidentFailure>> {
    static FAILURE: OnceLock<RwLock<Option<ResidentFailure>>> = OnceLock::new();
    FAILURE.get_or_init(|| RwLock::new(None))
}

pub(crate) fn last_failure() -> Option<ResidentFailure> {
    last_failure_slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn record_failure(reason: String) {
    *last_failure_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(ResidentFailure {
        reason,
        at: chrono::Local::now().to_rfc3339(),
    });
}

/// 业务 ready 门（完成定义 #5）：初始化未完成时，会写房间边的入口不许动手——返回
/// 结构化 `UPDATING`，调用方按可重试处置，不拿半份模型（或与初始化交错的实时行）写边。
///
/// 开关判定不在调用方（源码钉 `the_full_rebuild_entry_never_takes_the_resident_model`：
/// `room_model.rs` 不许出现 `room_resident_model()`），而在状态机里：`Disabled` /
/// `Unsupported` 就是「本进程没有 resident 体制」，放行走今天的路；`Loading` / `Blocked`
/// 是「有体制而这一刻没有可信的当前代」，`UPDATING`。**`Degraded` 放行**（ADR-073 D10，
/// 2026-09-16）：它是「装好了、带着已点名的缺口」——缺几何的面板在保护集里、缺文件的库不在册，
/// 计算器照常冻；此前把它也拦住，一处输入缺失就让之后每个任务的全部根判死。
pub(crate) fn ensure_ready() -> anyhow::Result<()> {
    match lifecycle() {
        ResidentLifecycle::Disabled { .. }
        | ResidentLifecycle::Unsupported { .. }
        | ResidentLifecycle::Ready { .. }
        | ResidentLifecycle::Degraded { .. } => Ok(()),
        state @ (ResidentLifecycle::Loading { .. } | ResidentLifecycle::Blocked { .. }) => {
            anyhow::bail!(
                "{RESIDENT_MODEL_UPDATING}: room resident model is not ready ({})",
                match state {
                    ResidentLifecycle::Loading { phase, .. } => format!("loading:{phase}"),
                    _ => "blocked".to_string(),
                }
            )
        }
    }
}

/// 本进程最近一次启动房间初始化的结局（`/health.room_build.resident.startup`）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct StartupInitReport {
    /// 建不起来的原因；`None` = 建起来了，模型已装进单例。
    pub blocked: Option<String>,
    /// 建起来时的那一行回执（[`StartupInitOutcome::summary`]）。
    pub summary: Option<String>,
    /// 这次初始化覆盖到的 MDB 设计库号（[`StartupInitOutcome::expected_dbnums`]）；
    /// 建不起来时是空的——枚举 MDB 本身就可能是失败的那一步。
    pub expected_dbnums: Vec<u32>,
    /// 初始化耗时；建不起来时 `None`。
    pub duration_ms: Option<u128>,
    /// 初始化收尾时刻的进程 RSS（字节）。sysinfo 不给峰值；这一刻紧跟满载的网格预热，
    /// 约等于本阶段峰值。平台探不到时 `None`。
    pub rss_bytes: Option<u64>,
    pub at: String,
}

fn startup_report_slot() -> &'static RwLock<Option<StartupInitReport>> {
    static REPORT: OnceLock<RwLock<Option<StartupInitReport>>> = OnceLock::new();
    REPORT.get_or_init(|| RwLock::new(None))
}

/// 最近一次启动初始化的结局；还没跑过就是 `None`。
pub(crate) fn startup_report() -> Option<StartupInitReport> {
    startup_report_slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// `UPSERT` 只动 `resident_blocked` 一格：`Some(原因)` 落字符串，`None` 清成 `NONE`。空间凭据与
/// `resident_reference` 都不碰（与 [`render_resident_reference_write`] 同一条纪律）。
pub(crate) fn render_resident_blocked_write(reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!(
            "UPSERT room_build:main SET resident_blocked = {};",
            serde_json::to_string(reason).expect("string serializes")
        ),
        None => "UPSERT room_build:main SET resident_blocked = NONE;".to_string(),
    }
}

pub(crate) async fn read_resident_blocked_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
) -> anyhow::Result<Option<String>> {
    let mut response = db
        .query("SELECT VALUE resident_blocked FROM room_build:main;")
        .await?
        .check()?;
    let rows: Vec<Option<String>> = response.take(0)?;
    Ok(rows.into_iter().flatten().next())
}

/// 记下这次启动初始化的结局：进程内一份（`/health` 读）+ `room_build:main.resident_blocked`
/// 一格（建起来了就清成 NONE）。库写失败只打印——结局以进程内那份为准。
pub(crate) async fn record_startup_init(result: Result<&StartupInitOutcome, &anyhow::Error>) {
    let (blocked, summary) = match result {
        Ok(outcome) => (None, Some(outcome.summary())),
        Err(error) => (Some(format!("{error:#}")), None),
    };
    *startup_report_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(StartupInitReport {
        blocked: blocked.clone(),
        summary,
        expected_dbnums: result
            .map(StartupInitOutcome::expected_dbnums)
            .unwrap_or_default(),
        duration_ms: result.ok().map(|outcome| outcome.elapsed_ms),
        rss_bytes: crate::data_interface::model_concurrency::process_rss_bytes(),
        at: chrono::Local::now().to_rfc3339(),
    });
    match result {
        // 模型带缺口（缺库 / 缺几何面板）= `Degraded`，计算器照常冻（ADR-073 D8–D10）；
        // `advance_warning` 那一档（清单前进没走完）随分片退役恒为 `None`，留着只是形状。
        Ok(outcome) => match (lifecycle_for(&outcome.model), &outcome.advance_warning) {
            (ResidentLifecycle::Ready { generation, .. }, Some(warning)) => {
                // `generation` 已是计算输入摘要（`lifecycle_for`），这里只是补上清单前进的告警。
                set_lifecycle(ResidentLifecycle::Degraded {
                    generation,
                    problems: vec![warning.clone()],
                });
            }
            (state, _) => set_lifecycle(state),
        },
        Err(error) => mark_failed(error),
    }
    if let Err(error) = aios_core::SUL_DB
        .query(render_resident_blocked_write(blocked.as_deref()))
        .await
        .and_then(|response| response.check())
    {
        println!(
            "[房间初始化] resident_blocked 没写进 room_build:main（/health 仍按进程内结局报）: {error:#}"
        );
    }
}

// ---------------------------------------------------------------------------
// 任务开工前对齐（ADR-073 D12，2026-09-16）：常驻模型不再是「启动建一次、进程活多久用多久」。
// ---------------------------------------------------------------------------

/// 常驻模型的配置输入：层级口径、房间关键字、房间名校验、网格目录。启动段与任务开工前对齐
/// 共用这一份，免得两处各抄一遍再悄悄分家。`None` = 本进程没有 resident 体制
/// （`spatial_compute` 关着，或 `project_hd` / `project_hh` 特性都不在）。
pub(crate) struct ResidentInputs {
    pub hierarchy: RoomHierarchy,
    pub room_key_words: Vec<String>,
    pub match_room_fn: fn(&str) -> bool,
    pub mesh_dir: PathBuf,
}

impl ResidentInputs {
    pub(crate) fn configured() -> Option<Self> {
        if !crate::options::spatial_compute() {
            return None;
        }
        let hierarchy = RoomHierarchy::configured()?;
        #[cfg(any(feature = "project_hd", feature = "project_hh"))]
        let match_room_fn = crate::fast_model::room_model::configured_match_room_fn();
        #[cfg(not(any(feature = "project_hd", feature = "project_hh")))]
        let match_room_fn: fn(&str) -> bool = |_| false;
        let option = aios_core::get_db_option();
        Some(Self {
            hierarchy,
            room_key_words: option.get_room_key_word(),
            match_room_fn,
            mesh_dir: option.get_meshes_path(),
        })
    }
}

/// [`refresh_before_task`] 的回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RoomReferenceRefresh {
    /// 房间开关关着 / 没有 resident 体制：没什么可对齐。
    NotApplicable,
    /// 此刻的文件版本 composite 与装好的那份相同：复用，零重建。
    Unchanged { key: String },
    /// 文件版本变了（或此前没建成）→ 按文件重建并装上。`generation_changed` = 计算输入的摘要
    /// 变了没有——没变时旧 ACK 照旧有效（一个 DESI 前进却没动房间 / 面板是常态）。
    Rebuilt {
        previous_key: Option<String>,
        key: String,
        generation_changed: bool,
        /// 与上一份的在册视图之差（`TopologyDiff::summary`）；此前没建成时为 `None`。
        diff: Option<String>,
        elapsed_ms: u128,
    },
    /// 重建没成：手上有旧的就留着继续算、`Degraded` 记一条 `stage=refresh` 缺口；没有旧的仍 `Blocked`。
    Failed { error: String, kept_previous: bool },
}

impl RoomReferenceRefresh {
    /// 给任务日志的一行；`Unchanged` / `NotApplicable` 不出声——每个任务都打一行就是噪音。
    pub(crate) fn log_line(&self) -> Option<String> {
        match self {
            Self::NotApplicable | Self::Unchanged { .. } => None,
            Self::Rebuilt {
                previous_key,
                key,
                generation_changed,
                diff,
                elapsed_ms,
            } => Some(format!(
                "[房间参照] 文件版本变了，常驻房间模型按文件重建：{} → {}，计算输入{}，{}耗时 {elapsed_ms}ms",
                previous_key
                    .as_deref()
                    .map(|key| key.chars().take(12).collect::<String>())
                    .unwrap_or_else(|| "（此前没建成）".to_string()),
                key.chars().take(12).collect::<String>(),
                if *generation_changed {
                    "变了（此前的根 ACK 作废，e3d.room.lookup 对它们回 not_computed 直到重生成）"
                } else {
                    "没变（旧 ACK 照旧有效）"
                },
                diff.as_deref()
                    .map(|diff| format!("在册视图之差 {diff}，"))
                    .unwrap_or_default(),
            )),
            Self::Failed {
                error,
                kept_previous,
            } => Some(format!(
                "[房间参照] 常驻房间模型重建失败（{}）: {error}",
                if *kept_previous {
                    "留着上一份继续算，缺口记进 /health.features.room.problems"
                } else {
                    "此前也没建成，仍 blocked：本批根按「房间算不出来」记失败"
                }
            )),
        }
    }
}

fn refresh_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// 任务开工前把常驻房间模型对齐到**此刻的文件版本**（ADR-073 D12）。
///
/// 探针便宜（[`crate::fast_model::room_topology::latest_frozen_sources`]：成员清单 + 每个 DESI
/// 的最新会话 + 令牌，不装 schema、不读拓扑、不读几何），composite 与装好的那份相同就是常路，
/// 零 I/O 之外零成本；变了才整份重建——两层缓存（`room-groups-v2` 磁盘工件 / GroupCache 按文件令牌、
/// `room-input-cache` 面板几何按文件的 `initialization_target_hash`）让没变的文件只付重开校验的钱，
/// 真正重算的只有前进了的那几个文件的拓扑与面板几何。装载在 `lock_spatial_serial` 里整份替换
/// （[`initialize_file_backed_at_startup`]），已开工的任务手里的 `Arc` 不受影响——D1「任务期间不换代」
/// 说的是每个任务自己那份引用。
///
/// 并发的任务开工共用一把 [`refresh_lock`]：第二个到的等第一个重建完，再探一次 composite、多半直接
/// `Unchanged`。重建失败**不**让任务失败：留着旧的那份继续算、生命周期 `Degraded` 记一条
/// `stage=refresh`（[`lifecycle_after_refresh_failure`]）；此前就没建成（启动 `Blocked`）而这次成了，
/// 就是自愈——启动时的暂时性故障不再要重启才能恢复。
///
/// 会话口径：文件**最新**（ADR-054 Q1 未钉住 = 最新），不是本任务的 T——任务排队期间文件又前进了的话，
/// 常驻里的面板比本窗几何新一版；下一窗任务追上来即合一。与 ADR-067 D3 当年接受的口径相同。
pub(crate) async fn refresh_before_task() -> RoomReferenceRefresh {
    // `Loading` = 启动段正在建：那份归启动段装，这里不抢着再建一份（任务本来也进不来——worker
    // 排在它之后；读透侧的即时生成由 `require_room_bootstrap_ready` 挡在门外）。
    if !crate::options::room_membership()
        || matches!(
            lifecycle(),
            ResidentLifecycle::Disabled { .. }
                | ResidentLifecycle::Unsupported { .. }
                | ResidentLifecycle::Loading { .. }
        )
    {
        return RoomReferenceRefresh::NotApplicable;
    }
    let Some(inputs) = ResidentInputs::configured() else {
        return RoomReferenceRefresh::NotApplicable;
    };
    let _serial_refresh = refresh_lock().lock().await;
    let face = crate::options::data_face();
    let installed = resident_rooms()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .for_membership(face);
    let probe = match crate::fast_model::room_topology::latest_frozen_sources().await {
        Ok(frozen) => frozen,
        Err(error) => return refresh_failed(installed.as_deref(), &error),
    };
    if let Some(model) = &installed {
        if model.manifest.key == probe.composite {
            return RoomReferenceRefresh::Unchanged {
                key: probe.composite,
            };
        }
    }
    let started = Instant::now();
    let rebuilt = initialize_file_backed_at_startup(
        ResidentForm::for_face(face),
        inputs.hierarchy,
        &inputs.room_key_words,
        inputs.match_room_fn,
        &inputs.mesh_dir,
    )
    .await;
    match rebuilt {
        Ok(outcome) => {
            let previous_generation = installed.as_ref().map(|model| model.input_version());
            let generation = outcome.model.input_version();
            let diff = installed
                .as_ref()
                .map(|old| TopologyDiff::between(&old.map, &outcome.model.map));
            set_lifecycle(lifecycle_for(&outcome.model));
            record_generation_swap(GenerationSwapReport {
                form: outcome.form.as_str().to_string(),
                key: outcome.model.manifest.key.chars().take(12).collect(),
                diff: diff.as_ref().map(TopologyDiff::summary),
                touched_panels: diff
                    .as_ref()
                    .map(|diff| diff.touched_panels().len())
                    .unwrap_or_default(),
                matched_targets: 0,
                round_targets: 0,
                guard_fired: false,
                note: Some("task-start refresh (ADR-073 D12)".to_string()),
                at: chrono::Local::now().to_rfc3339(),
            });
            RoomReferenceRefresh::Rebuilt {
                previous_key: installed.map(|model| model.manifest.key.clone()),
                key: outcome.model.manifest.key.clone(),
                generation_changed: previous_generation.as_deref() != Some(generation.as_str()),
                diff: diff.map(|diff| diff.summary()),
                elapsed_ms: started.elapsed().as_millis(),
            }
        }
        Err(error) => refresh_failed(installed.as_deref(), &error),
    }
}

fn refresh_failed(
    installed: Option<&ResidentRoomModel>,
    error: &anyhow::Error,
) -> RoomReferenceRefresh {
    record_failure(format!("{error:#}"));
    match installed {
        Some(previous) => {
            set_lifecycle(lifecycle_after_refresh_failure(
                previous,
                &format!("{error:#}"),
            ));
            RoomReferenceRefresh::Failed {
                error: format!("{error:#}"),
                kept_previous: true,
            }
        }
        None => {
            set_lifecycle(ResidentLifecycle::Blocked {
                problems: vec![format!("{error:#}")],
            });
            RoomReferenceRefresh::Failed {
                error: format!("{error:#}"),
                kept_previous: false,
            }
        }
    }
}

/// 本进程最近一次**换代**的观测（`/health.room_build.resident.last_swap`，主计划 §3 P4）。
///
/// 只有真换了代的那一轮才记——身份键没变是常路，一轮一条会把这一格刷成噪音。`guard_fired`
/// 就是 P4 那条护栏：拓扑变了（diff 非空）而这一轮一块面板目标都没落在 diff 上，说明账本
/// 触发（F-A，`room_structural_trigger.rs`）漏了这一形状。护栏只出声、不标脏（§3 P4 落地记）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct GenerationSwapReport {
    pub form: String,
    /// 换到的那份模型的身份键（截断给人看）。
    pub key: String,
    /// [`TopologyDiff::summary`]；第一代没有上一代可比时是 `None`。
    pub diff: Option<String>,
    /// diff 碰到的面板数（[`TopologyDiff::touched_panels`]）。
    pub touched_panels: usize,
    /// 本轮 drain 载到的面板任务里，落在 diff 上的块数。
    pub matched_targets: usize,
    /// 本轮 drain 载到的面板任务总数。
    pub round_targets: usize,
    pub guard_fired: bool,
    /// 护栏那一句；或者判不了时的原因（面板页满了，后面还有页）。
    pub note: Option<String>,
    pub at: String,
}

fn generation_swap_slot() -> &'static RwLock<Option<GenerationSwapReport>> {
    static SWAP: OnceLock<RwLock<Option<GenerationSwapReport>>> = OnceLock::new();
    SWAP.get_or_init(|| RwLock::new(None))
}

/// 本进程最近一次换代的观测；还没换过代就是 `None`。
pub(crate) fn last_generation_swap() -> Option<GenerationSwapReport> {
    generation_swap_slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

pub(crate) fn record_generation_swap(report: GenerationSwapReport) {
    *generation_swap_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(report);
}

/// `/health.room_build.resident`（Q20 (a)）：启动结局 + 每一路当前模型的统计，无几何的在册面板
/// 按库点名，并标出哪些库在监听限定域之外。`is_watched` 由调用方按 `watch_scope` 给——本模块
/// 只给数，不认识限定域。P4 之后多一格 `last_swap`（最近一次换代的 diff 摘要 + 护栏响没响）。
pub(crate) fn resident_health(is_watched: impl Fn(u32) -> bool) -> serde_json::Value {
    let rooms = resident_rooms()
        .read()
        .unwrap_or_else(PoisonError::into_inner);
    let stats = rooms.stats();
    let active = rooms
        .for_membership(crate::options::data_face())
        .map(|model| model.stats());
    drop(rooms);
    let startup = startup_report();
    let mut health = resident_health_from(
        startup.as_ref(),
        last_generation_swap().as_ref(),
        &stats,
        is_watched,
    );
    if let (Some(target), Ok(serde_json::Value::Object(state))) =
        (health.as_object_mut(), serde_json::to_value(lifecycle()))
    {
        target.extend(state);
        target.insert(
            "enabled".to_string(),
            serde_json::json!(!matches!(lifecycle(), ResidentLifecycle::Disabled { .. })),
        );
        // MDB 覆盖判定（完成定义 #1 的观测面）：期望集来自启动那一次枚举（§2.1 只探
        // 一次），已装集来自当前代的来源会话；差集就是「哪个库还没进当前代」。
        let expected = active
            .as_ref()
            .map(|stat| stat.expected_dbnums.clone())
            .or_else(|| {
                startup
                    .as_ref()
                    .map(|report| report.expected_dbnums.clone())
            })
            .unwrap_or_default();
        let loaded: Vec<u32> = active
            .as_ref()
            .map(|stat| stat.source_sessions.keys().copied().collect())
            .unwrap_or_default();
        let missing: Vec<u32> = expected
            .iter()
            .filter(|dbnum| !loaded.contains(dbnum))
            .copied()
            .collect();
        target.insert(
            "mdb_dbnums_expected".to_string(),
            serde_json::json!(expected),
        );
        target.insert("mdb_dbnums_loaded".to_string(), serde_json::json!(loaded));
        target.insert("mdb_dbnums_missing".to_string(), serde_json::json!(missing));
        target.insert(
            "startup_duration_ms".to_string(),
            serde_json::json!(startup.as_ref().and_then(|report| report.duration_ms)),
        );
        target.insert(
            "last_failure".to_string(),
            serde_json::json!(last_failure()),
        );
        if let Some(stat) = active {
            target.insert("rooms".to_string(), serde_json::json!(stat.rooms));
            target.insert(
                "registered_panels".to_string(),
                serde_json::json!(stat.registered_panels),
            );
            target.insert(
                "panels_ready".to_string(),
                serde_json::json!(stat.panels_with_geometry),
            );
            target.insert(
                "missing_panels".to_string(),
                serde_json::json!(stat.missing_panels),
            );
            target.insert(
                "missing_dbnums".to_string(),
                serde_json::json!(stat.missing_dbnums),
            );
            target.insert(
                "mesh_keys_expected".to_string(),
                serde_json::json!(stat.mesh_keys_expected),
            );
            target.insert(
                "mesh_keys_warmed".to_string(),
                serde_json::json!(stat.mesh_keys_warmed),
            );
            target.insert(
                "mesh_warm_failed".to_string(),
                serde_json::json!(stat.mesh_warm_failed),
            );
            target.insert("mesh_bytes".to_string(), serde_json::json!(stat.mesh_bytes));
            target.insert(
                "topology_digest".to_string(),
                serde_json::json!(stat.digest),
            );
            target.insert(
                "geometry_digest".to_string(),
                serde_json::json!(stat.geometry_digest),
            );
            target.insert(
                "source_sessions".to_string(),
                serde_json::json!(stat.source_sessions),
            );
        }
    }
    health
}

/// [`resident_health`] 的纯函数半截，测试直接喂。
pub(crate) fn resident_health_from(
    startup: Option<&StartupInitReport>,
    last_swap: Option<&GenerationSwapReport>,
    stats: &[ResidentStats],
    is_watched: impl Fn(u32) -> bool,
) -> serde_json::Value {
    let mut unwatched_missing: BTreeMap<u32, usize> = BTreeMap::new();
    let models: Vec<serde_json::Value> = stats
        .iter()
        .map(|stat| {
            let missing_geometry: Vec<serde_json::Value> = stat
                .missing_panels_by_dbnum
                .iter()
                .map(|(dbnum, panels)| {
                    let watched = is_watched(*dbnum);
                    if !watched {
                        *unwatched_missing.entry(*dbnum).or_insert(0) += panels;
                    }
                    serde_json::json!({
                        "dbnum": dbnum,
                        "panels": panels,
                        "watched": watched,
                    })
                })
                .collect();
            serde_json::json!({
                "form": stat.form,
                "key": stat.key,
                "digest": stat.digest,
                "rooms": stat.rooms,
                "registered_panels": stat.registered_panels,
                "panels_with_geometry": stat.panels_with_geometry,
                "missing_panels": stat.missing_panels,
                "missing_geometry": missing_geometry,
                "unusable_panels": stat.unusable_panels,
                "missing_dbnums": stat.missing_dbnums,
                "problems": stat.problems,
                "meshes_cached": stat.meshes_cached,
                "mesh_keys_expected": stat.mesh_keys_expected,
                "mesh_keys_warmed": stat.mesh_keys_warmed,
                "mesh_warm_failed": stat.mesh_warm_failed,
                "mesh_bytes": stat.mesh_bytes,
            })
        })
        .collect();
    let notice = (!unwatched_missing.is_empty()).then(|| {
        format!(
            "{} 块在册面板无几何，来自未监听的库 [{}]（watch_dbnums 之外：拓扑在册、不算归属、\
             存量边受保护、面板仍进排除集）",
            unwatched_missing.values().sum::<usize>(),
            unwatched_missing
                .keys()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
    });
    serde_json::json!({
        "startup": startup,
        "last_swap": last_swap,
        "models": models,
        "unwatched_missing_geometry_notice": notice,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::room_model::match_room_name_hd;
    use aios_core::RefU64;
    use bevy_transform::components::Transform;
    use glam::Vec3;
    use parry3d::math::Point;

    /// Real AMS /ALL source read, with no SurrealDB initialization. Keep this
    /// isolated from the regular suite and give it a private meshes directory.
    #[tokio::test]
    #[ignore = "requires AMS source files and an isolated DB_OPTION_FILE"]
    async fn ams_all_room_preload_without_database() -> anyhow::Result<()> {
        let option = aios_core::get_db_option();
        anyhow::ensure!(
            option.project_name == "AvevaMarineSample"
                && option.mdb_name.trim_start_matches('/') == "ALL",
            "this measurement requires AMS /ALL"
        );
        let started = Instant::now();
        let mut model = load_latest(
            RoomHierarchy::configured().context("room hierarchy")?,
            &option.get_room_key_word(),
            crate::fast_model::room_model::configured_match_room_fn(),
            Arc::<MeshCache>::default(),
        )
        .await?;
        let load_ms = started.elapsed().as_millis();
        warm_for_install(&mut model, &option.get_meshes_path()).await?;
        let stats = model.stats();
        let report = serde_json::json!({
            "project": option.project_name, "mdb": option.mdb_name,
            "dbnums": model.expected_dbnums, "rooms": stats.rooms,
            "registered_panels": stats.registered_panels,
            "panels_ready": stats.panels_with_geometry,
            "missing_panels": stats.missing_panels,
            "preload_ms": load_ms, "total_ms": started.elapsed().as_millis(),
            "mesh_bytes": stats.mesh_bytes, "input_version": model.input_version()
        });
        println!("AMS_ROOM_PRELOAD {}", report);
        if let Some(path) = std::env::var_os("AMS_ROOM_TIMING_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&report)?)?;
        }
        anyhow::ensure!(
            stats.rooms > 0 && stats.registered_panels > 0,
            "AMS rooms missing"
        );
        anyhow::ensure!(stats.missing_panels == 0, "AMS room panels incomplete");
        Ok(())
    }

    #[tokio::test]
    #[ignore = "measures all AMS DESI model generation and rooms from real files"]
    async fn ams_all_desi_models_and_rooms_without_database() -> anyhow::Result<()> {
        measure_ams_memory_rooms(None, false).await
    }

    #[tokio::test]
    #[ignore = "real AMS cached model output: vertex optimization edge parity"]
    async fn ams_cached_room_vertex_optimization_parity() -> anyhow::Result<()> {
        measure_ams_memory_rooms(Some(1112), true).await
    }

    #[tokio::test]
    #[ignore = "real AMS 8000 model and room calculation parity; isolated DB_OPTION_FILE required"]
    async fn ams_8000_models_and_rooms_without_database() -> anyhow::Result<()> {
        measure_ams_memory_rooms(Some(8000), true).await
    }

    /// ADR-073 D12 / 源码钉 16 (c) 的真文件版：装一份键对不上此刻文件版本的「陈旧」模型，第一次任务
    /// 开工前对齐必须从真 AMS /ALL 文件整份重建并装上（`Rebuilt`，代号换成真值、生命周期 `Ready`、
    /// `last_swap.note = task-start refresh`）；紧接着第二次对齐 composite 已相同 → `Unchanged`、零重建。
    /// 只读文件、不连库。跑法：`cargo test --lib -- --ignored --exact <本用例> --nocapture`。
    #[tokio::test]
    #[ignore = "real AMS /ALL files: task-start refresh rebuilds the resident from a stale key, then reuses it"]
    async fn ams_task_start_refresh_rebuilds_from_a_stale_key_and_then_reuses_the_generation()
    -> anyhow::Result<()> {
        let option = aios_core::get_db_option();
        anyhow::ensure!(
            option.project_name == "AvevaMarineSample"
                && option.mdb_name.trim_start_matches('/') == "ALL",
            "requires AMS /ALL"
        );
        let _on = crate::options::RoomMembershipOverride::set(true);
        let restore = lifecycle();
        let stale = model_of(groups(), vec![], Arc::default());
        let stale_key = stale.manifest.key.clone();
        {
            let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
            resident_rooms()
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .install(stale);
        }
        set_lifecycle(ResidentLifecycle::Ready {
            generation: "stale".into(),
            completed_at: String::new(),
        });

        let started = Instant::now();
        let first = refresh_before_task().await;
        let first_ms = started.elapsed().as_millis();
        let RoomReferenceRefresh::Rebuilt {
            previous_key,
            key,
            generation_changed,
            ..
        } = &first
        else {
            set_lifecycle(restore);
            anyhow::bail!("键对不上就该重建，却回了 {first:?}");
        };
        anyhow::ensure!(
            previous_key.as_deref() == Some(stale_key.as_str()),
            "{first:?}"
        );
        anyhow::ensure!(key != &stale_key && *generation_changed, "{first:?}");
        let current = current_for_calculation()?;
        let stats = current.stats();
        anyhow::ensure!(
            stats.rooms > 0 && stats.registered_panels > 0 && stats.missing_panels == 0,
            "重建后的模型该是真 AMS：{stats:?}"
        );
        anyhow::ensure!(
            matches!(lifecycle(), ResidentLifecycle::Ready { .. }),
            "{:?}",
            lifecycle()
        );
        let swap = last_generation_swap().context("重建要记一次换代观测")?;
        anyhow::ensure!(
            swap.note.as_deref() == Some("task-start refresh (ADR-073 D12)"),
            "{swap:?}"
        );

        let again = Instant::now();
        let second = refresh_before_task().await;
        let second_ms = again.elapsed().as_millis();
        anyhow::ensure!(
            matches!(&second, RoomReferenceRefresh::Unchanged { key: same } if same == key),
            "composite 相同就该复用，却回了 {second:?}"
        );
        println!(
            "AMS_ROOM_REFRESH {}",
            serde_json::json!({
                "rebuilt_ms": first_ms, "probe_only_ms": second_ms,
                "rooms": stats.rooms, "registered_panels": stats.registered_panels,
                "missing_panels": stats.missing_panels, "generation": current.input_version(),
                "diff": swap.diff,
            })
        );
        set_lifecycle(restore);
        Ok(())
    }

    /// ADR-073 D8 / 源码钉 14 (b) 的真文件版：MDB 声明了、搜索根里却没有文件的那个 DESI（环境变量
    /// `AMS_ROOM_EXPECT_MISSING_DESI`，例如把 SCB 目录从搜索根里拿掉后的 6003）→ 探针把它点进
    /// `missing_design`、冻结源里没有它；常驻模型**照样建起来**（`load_latest` 不再 Err）、缺口进
    /// `problems`、生命周期 `Degraded`、计算器照冻、`ensure_ready` 放行。跑法：`DB_OPTION_FILE` 指向一份
    /// `project_path` 缺那个项目目录的配置，`AMS_ROOM_EXPECT_MISSING_DESI=6003`。
    #[tokio::test]
    #[ignore = "real AMS files with one DESI project removed from the search root; needs DB_OPTION_FILE + AMS_ROOM_EXPECT_MISSING_DESI"]
    async fn ams_missing_desi_degrades_the_room_reference_instead_of_blocking_it()
    -> anyhow::Result<()> {
        use crate::fast_model::room_model::MemoryRoomCalculator;
        let option = aios_core::get_db_option();
        anyhow::ensure!(
            option.project_name == "AvevaMarineSample"
                && option.mdb_name.trim_start_matches('/') == "ALL",
            "requires AMS /ALL"
        );
        let expected: u32 = std::env::var("AMS_ROOM_EXPECT_MISSING_DESI")
            .context("AMS_ROOM_EXPECT_MISSING_DESI=<dbnum>")?
            .trim()
            .parse()?;
        let frozen = crate::fast_model::room_topology::latest_frozen_sources().await?;
        anyhow::ensure!(
            frozen
                .missing_design
                .iter()
                .any(|(dbnum, _)| *dbnum == expected),
            "探针要把缺文件的 DESI 点进 missing_design: {:?}",
            frozen.missing_design
        );
        anyhow::ensure!(
            !frozen
                .sources
                .iter()
                .any(|source| source.dbnum() as u32 == expected),
            "缺文件的 DESI 不在冻结源里"
        );
        let started = Instant::now();
        let mut model = load_latest(
            RoomHierarchy::configured().context("room hierarchy")?,
            &option.get_room_key_word(),
            crate::fast_model::room_model::configured_match_room_fn(),
            Arc::<MeshCache>::default(),
        )
        .await
        .context("缺一个 DESI 文件时常驻模型必须照样建起来（D8）")?;
        warm_for_install(&mut model, &option.get_meshes_path()).await?;
        anyhow::ensure!(
            model
                .missing_dbnums
                .iter()
                .any(|(dbnum, _)| *dbnum == expected),
            "{:?}",
            model.missing_dbnums
        );
        let problems = model.problems.join("\n");
        anyhow::ensure!(
            problems.contains("reason=missing-desi-file")
                && problems.contains(&expected.to_string()),
            "{problems}"
        );
        let stats = model.stats();
        anyhow::ensure!(stats.rooms > 0 && stats.registered_panels > 0, "{stats:?}");
        let state = lifecycle_for(&model);
        anyhow::ensure!(
            matches!(state, ResidentLifecycle::Degraded { .. }),
            "{state:?}"
        );
        let generation = model.input_version();
        let calculator = MemoryRoomCalculator::new(Arc::new(model), option.get_meshes_path())
            .context("Degraded 的模型照样冻得出计算器（D10）")?;
        anyhow::ensure!(calculator.resident_generation() == generation);
        let restore = lifecycle();
        set_lifecycle(state);
        let admitted = ensure_ready();
        set_lifecycle(restore);
        admitted.context("Degraded 放行（D10）")?;
        println!(
            "AMS_ROOM_MISSING_DESI {}",
            serde_json::json!({
                "missing_design": frozen.missing_design, "sources": frozen.sources.len(),
                "rooms": stats.rooms, "registered_panels": stats.registered_panels,
                "missing_panels": stats.missing_panels, "problems": stats.problems,
                "build_ms": started.elapsed().as_millis(),
            })
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "real AMS 7022 model and room initialization after route repair"]
    async fn ams_7022_models_and_rooms_without_database() -> anyhow::Result<()> {
        measure_ams_memory_rooms(Some(7022), false).await
    }

    #[tokio::test]
    #[ignore = "real AMS 7330 zero-length section model and room completion"]
    async fn ams_7330_models_and_rooms_without_database() -> anyhow::Result<()> {
        measure_ams_memory_rooms(Some(7330), false).await
    }

    #[tokio::test]
    #[ignore = "real AMS 7048 explicit-angle model and room completion"]
    async fn ams_7048_models_and_rooms_without_database() -> anyhow::Result<()> {
        measure_ams_memory_rooms(Some(7048), false).await
    }

    async fn measure_ams_memory_rooms(
        only_dbnum: Option<u32>,
        check_parity: bool,
    ) -> anyhow::Result<()> {
        use crate::data_interface::frozen_source::FrozenSource;
        use crate::fast_model::e3d_model_service::{E3dModelService, parse_refno};
        use crate::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope};
        use crate::fast_model::{model_artifact, room_model::MemoryRoomCalculator};
        let option = aios_core::get_db_option();
        anyhow::ensure!(
            option.project_name == "AvevaMarineSample"
                && option.mdb_name.trim_start_matches('/') == "ALL",
            "requires AMS /ALL"
        );
        let output =
            std::env::var_os("AMS_ROOM_TIMING_REPORT").context("timing output path required")?;
        let started = Instant::now();
        let mut model = load_latest(
            RoomHierarchy::configured().context("room hierarchy")?,
            &option.get_room_key_word(),
            crate::fast_model::room_model::configured_match_room_fn(),
            Arc::<MeshCache>::default(),
        )
        .await?;
        warm_for_install(&mut model, &option.get_meshes_path()).await?;
        anyhow::ensure!(
            model.missing_panels.is_empty(),
            "room definitions incomplete"
        );
        let room_preload_ms = started.elapsed().as_millis();
        let room_version = model.input_version();
        let sources = model.source_tokens.clone();
        let expected_files = sources.len();
        let calculator = MemoryRoomCalculator::new(Arc::new(model), option.get_meshes_path())?;
        let memory = ModelMemoryStore::default();
        let cache_dir = option.get_meshes_path().join("model-input-cache");
        let dependencies = crate::fast_model::e3d_model_service::current_mdb_sources()?
            .0
            .into_iter()
            .map(|pin| FrozenSource::capture(pin.file)?.commit_token_json())
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut timings = Vec::new();
        let mut failed_dbnums = Vec::new();
        for token in sources {
            let source = FrozenSource::from_commit_token_json(&token)?.reopen_verified()?;
            let dbnum = source.dbnum() as u32;
            if only_dbnum.is_some_and(|only| only != dbnum) {
                continue;
            }
            let session = source.target_sesno();
            let file_started = Instant::now();
            println!("[ams-phase] phase=dbnum_initialize_started dbnum={dbnum} sesno={session}");
            let roots =
                crate::data_interface::window_root_plan::enumerate_target_roots(dbnum, session)
                    .await?;
            let enumerate_ms = file_started.elapsed().as_millis();
            let service =
                E3dModelService::from_initialization_sources(&source, &dependencies).await?;
            let input = service.initialization_target_hash(dbnum, session)?;
            let generation_started = Instant::now();
            let mut missing = Vec::new();
            let mut cached = 0;
            for root in &roots {
                let root = parse_refno(&root.root.to_string())?;
                if let Some(commit) =
                    model_artifact::load(&cache_dir, &option.get_meshes_path(), &input, root)
                {
                    memory.commit(ProjectionScope::Current, vec![commit]);
                    cached += 1;
                } else {
                    missing.push(root.to_string());
                }
            }
            let generated = missing.len();
            let mut failures = Vec::new();
            if !missing.is_empty() {
                let (tx, mut rx) = tokio::sync::mpsc::channel(1);
                let produce = service.generate_frozen_roots(dbnum, &missing, tx);
                let consume = async {
                    let mut received = BTreeSet::new();
                    while let Some(result) = rx.recv().await {
                        anyhow::ensure!(
                            received.insert(result.root.to_string()),
                            "duplicate model output"
                        );
                        match result.outcome {
                            Ok(commit) => {
                                model_artifact::store(&cache_dir, &input, result.root, &commit)?;
                                memory.commit(ProjectionScope::Current, vec![commit]);
                            }
                            Err(error) => failures.push(format!("{}: {error}", result.root)),
                        }
                    }
                    anyhow::ensure!(
                        received == missing.iter().cloned().collect::<BTreeSet<_>>(),
                        "missing model output"
                    );
                    anyhow::Ok(())
                };
                futures::future::try_join(produce, consume).await?;
            }
            let generation_ms = generation_started.elapsed().as_millis();
            crate::fast_model::e3d_model_service::forget_generation_input(dbnum);
            let (epoch, records) = memory.snapshot_dbnum(dbnum);
            let room_started = Instant::now();
            let mut edge_count = None;
            let mut reference_room_ms = None;
            let mut optimized_room_ms = None;
            if failures.is_empty() {
                // 房间输入版本结构随第三枚水位退了（ADR-073 D2）：算的是哪一版模型、按哪一版
                // 面板定义，直接两格传给计算器。
                match calculator.calculate(dbnum, session as i32, &room_version, &records) {
                    Ok(edges) => {
                        optimized_room_ms = Some(room_started.elapsed().as_millis());
                        edge_count = Some(edges.len());
                        if check_parity {
                            let reference_started = Instant::now();
                            let reference = calculator.calculate_with_component_bvh(
                                dbnum,
                                session as i32,
                                &room_version,
                                &records,
                            )?;
                            reference_room_ms = Some(reference_started.elapsed().as_millis());
                            let key = |edge: &crate::fast_model::room_model::ElementRoomEdge| {
                                (
                                    edge.panel,
                                    edge.room_num.clone(),
                                    edge.member.refno,
                                    edge.member.inside_count,
                                    edge.member.center_dist.to_bits(),
                                )
                            };
                            anyhow::ensure!(
                                edges.iter().map(key).eq(reference.iter().map(key)),
                                "optimized room calculation differs from component-BVH reference"
                            );
                        }
                    }
                    Err(error) => failures.push(format!("room calculation: {error:#}")),
                }
            }
            anyhow::ensure!(
                memory.commit_epoch(dbnum) == epoch,
                "model changed during room calculation"
            );
            source.reopen_verified()?;
            if !failures.is_empty() {
                failed_dbnums.push(dbnum);
            }
            let timing = serde_json::json!({"dbnum": dbnum, "sesno": session,
                "roots": roots.len(), "generated": generated, "cached": cached,
                "records": records.len(), "enumerate_ms": enumerate_ms,
                "model_ms": generation_ms, "room_ms": room_started.elapsed().as_millis(),
                "optimized_room_ms": optimized_room_ms, "reference_room_ms": reference_room_ms,
                "room_edges": edge_count, "total_ms": file_started.elapsed().as_millis(), "failures": failures});
            println!("AMS_DBNUM_TIMING {timing}");
            timings.push(timing);
            std::fs::write(
                &output,
                serde_json::to_vec_pretty(&serde_json::json!({
                    "room_preload_ms": room_preload_ms, "expected_files": expected_files,
                    "completed_files": timings.len(), "dbnums": timings,
                    "failed_dbnums": failed_dbnums, "total_ms": started.elapsed().as_millis(),
                    "scope": "e3d-model generation and memory room calculation; excludes data ingest and publication"
                }))?,
            )?;
        }
        anyhow::ensure!(
            failed_dbnums.is_empty(),
            "unfinished dbnums: {failed_dbnums:?}"
        );
        Ok(())
    }

    fn r(id: u32) -> RefnoEnum {
        RefnoEnum::from(RefU64::from_two_nums(24384, id))
    }

    fn group(room: u32, name: &str, panels: &[u32]) -> RoomGroup {
        RoomGroup {
            room: r(room),
            name: name.into(),
            panels: panels.iter().map(|id| r(*id)).collect(),
        }
    }

    fn keywords() -> Vec<String> {
        vec!["-RM".to_string()]
    }

    fn aabb(min: f32, max: f32) -> Aabb {
        Aabb::new(Point::new(min, min, min), Point::new(max, max, max))
    }

    fn inst(panel: u32, geo_hash: &str, translation: f32, bounds: (f32, f32)) -> GeomInstQuery {
        GeomInstQuery {
            refno: r(panel),
            old_refno: None,
            owner: r(1),
            world_aabb: aabb(bounds.0, bounds.1),
            world_trans: Transform::from_translation(Vec3::splat(translation)),
            insts: vec![aios_core::ModelHashInst {
                geo_hash: geo_hash.into(),
                ..Default::default()
            }],
            has_neg: false,
            generic: "PANE".into(),
            pts: None,
            date: None,
        }
    }

    fn manifest(form: ResidentForm, key: &str) -> ResidentManifest {
        ResidentManifest {
            form,
            key: key.into(),
        }
    }

    /// 两间合规房 R301(7, 8) / R302(9)，一间不合规 LOBBY(22)，一间没关键字 F001(16)。
    fn groups() -> Vec<RoomGroup> {
        vec![
            group(5, "/1RX-RM03-R301", &[7, 8]),
            group(6, "/1RX-RM03-R302", &[9]),
            group(21, "/2RX-RM01-LOBBY", &[22]),
            group(15, "/1RX-STRU-F001", &[16]),
        ]
    }

    fn model_of(
        groups: Vec<RoomGroup>,
        insts: Vec<GeomInstQuery>,
        meshes: Arc<MeshCache>,
    ) -> ResidentRoomModel {
        let map = registered_map(&groups, &keywords(), match_room_name_hd);
        ResidentRoomModel::assemble(
            manifest(ResidentForm::Published, "m1"),
            groups,
            map,
            insts,
            meshes,
        )
    }

    #[test]
    fn assemble_keeps_only_registered_panels_and_lists_the_ones_without_geometry() {
        let model = model_of(
            groups(),
            vec![
                inst(7, "mesh-a", 0.0, (0.0, 1.0)),
                // 同一块面板第二条行：包围盒取并、实例取并。
                inst(7, "mesh-b", 1.0, (0.5, 2.0)),
                // 不合规房间的面板：拿到几何也不进模型，但在排除集里。
                inst(22, "mesh-c", 0.0, (0.0, 1.0)),
                // 包围盒不可用：视同没几何。
                inst(9, "mesh-d", 0.0, (f32::NAN, 1.0)),
            ],
            Arc::default(),
        );
        assert_eq!(model.map.rooms.len(), 2, "LOBBY 不满足命名校验");
        assert_eq!(registered_panels(&model.map), vec![r(7), r(8), r(9)]);
        assert_eq!(
            model.map.all_panels,
            [r(7), r(8), r(9), r(22)]
                .into_iter()
                .collect::<std::collections::HashSet<_>>(),
            "命名不合规房间的面板也进排除集；没命中关键字的不算房间（与 room_topology 同口径）"
        );
        let seven = model.panel(r(7)).expect("7 有几何");
        assert_eq!(seven.room, r(5));
        assert_eq!(seven.room_num, "R301");
        assert_eq!(seven.insts.len(), 2);
        assert_eq!(seven.world_aabb, aabb(0.0, 2.0));
        assert_eq!(
            seven.insts[1].matrix,
            Mat4::from_translation(Vec3::splat(1.0)),
            "世界矩阵 = world_trans * transform 已乘好"
        );
        assert!(model.panel(r(22)).is_none());
        assert_eq!(
            model.missing_panels,
            vec![r(8), r(9)],
            "没行的 8 与包围盒坏的 9"
        );
        // D-A (a)：`missing` 里再分一刀——9 有行却不可用（挡参照、入队），8 从未生成（不挡）。
        assert_eq!(
            model.unusable_panels,
            BTreeSet::from([r(9)]),
            "只有包围盒坏的 9 算不可用；一条行都没有的 8 不算"
        );
        assert_eq!(model.stats().unusable_panels, 1);
        assert_eq!(model.digest, topology_digest(&groups()));
        let stats = model.stats();
        assert_eq!(
            (
                stats.rooms,
                stats.registered_panels,
                stats.panels_with_geometry,
                stats.missing_panels
            ),
            (2, 3, 1, 2)
        );
        assert_eq!(
            stats.missing_panels_by_dbnum,
            BTreeMap::from([(24384, 2)]),
            "无几何按库分组，/health 拿它点名来自未监听的库（Q20 (a)）"
        );
    }

    #[test]
    fn topology_digest_ignores_order_and_tracks_names_and_panel_sets() {
        let base = groups();
        let mut shuffled = base.clone();
        shuffled.reverse();
        shuffled[3].panels.reverse();
        assert_eq!(topology_digest(&base), topology_digest(&shuffled));

        let mut renamed = base.clone();
        renamed[0].name = "/1RX-RM03-R309".into();
        assert_ne!(topology_digest(&base), topology_digest(&renamed));

        let mut moved = base.clone();
        moved[0].panels.pop();
        moved[1].panels.push(r(8));
        assert_ne!(topology_digest(&base), topology_digest(&moved));

        let mut extra = base.clone();
        extra.push(group(99, "/X", &[]));
        assert_ne!(
            topology_digest(&base),
            topology_digest(&extra),
            "没关键字的房也算拓扑"
        );
    }

    fn map_of(groups: &[RoomGroup]) -> RoomPanelMap {
        registered_map(groups, &keywords(), match_room_name_hd)
    }

    #[test]
    fn diff_is_empty_for_identical_registered_views() {
        let old = map_of(&groups());
        let mut reordered = groups();
        reordered.reverse();
        let diff = TopologyDiff::between(&old, &map_of(&reordered));
        assert!(diff.is_empty(), "{diff:?}");
    }

    #[test]
    fn a_panel_moved_between_rooms_is_recalculated_and_nothing_is_removed() {
        let old = map_of(&groups());
        let new = map_of(&[
            group(5, "/1RX-RM03-R301", &[7]),
            group(6, "/1RX-RM03-R302", &[9, 8]),
        ]);
        let diff = TopologyDiff::between(&old, &new);
        assert_eq!(diff.recalc_panels, BTreeSet::from([r(8)]));
        assert!(diff.removed_panels.is_empty());
        assert!(diff.renumbered_rooms.is_empty());
        assert_eq!(diff.touched_panels(), BTreeSet::from([r(8)]));
    }

    #[test]
    fn a_room_leaving_the_keyword_or_naming_rule_removes_every_panel_beneath() {
        let old = map_of(&groups());
        // R301 改出关键字；R302 改成命名不合规。
        let new = map_of(&[
            group(5, "/1RX-STRU-R301", &[7, 8]),
            group(6, "/1RX-RM03-HALL", &[9]),
        ]);
        let diff = TopologyDiff::between(&old, &new);
        assert_eq!(diff.removed_panels, BTreeSet::from([r(7), r(8), r(9)]));
        assert!(diff.recalc_panels.is_empty());
        assert!(diff.renumbered_rooms.is_empty());
    }

    #[test]
    fn a_renumbered_room_with_the_same_panels_only_changes_the_column() {
        let old = map_of(&groups());
        let new = map_of(&[
            group(5, "/1RX-RM03-R309", &[7, 8]),
            group(6, "/1RX-RM03-R302", &[9]),
        ]);
        let diff = TopologyDiff::between(&old, &new);
        assert!(diff.removed_panels.is_empty());
        assert!(diff.recalc_panels.is_empty(), "改房间号不重算：{diff:?}");
        assert_eq!(
            diff.renumbered_rooms,
            vec![RoomRenumber {
                room: r(5),
                old_num: "R301".into(),
                new_num: "R309".into(),
                panels: vec![r(7), r(8)],
            }]
        );
        assert_eq!(diff.touched_panels(), BTreeSet::from([r(7), r(8)]));
        assert_eq!(
            diff.summary(),
            "removed=0 recalc=0 renumbered_rooms=1 (panels 2)"
        );
    }

    #[test]
    fn renumbering_plus_panel_changes_split_into_column_change_and_recalc() {
        let old = map_of(&groups());
        // R301 → R309，同时 8 走了、10 来了：7 改列，10 重算，8 删。
        let new = map_of(&[
            group(5, "/1RX-RM03-R309", &[7, 10]),
            group(6, "/1RX-RM03-R302", &[9]),
        ]);
        let diff = TopologyDiff::between(&old, &new);
        assert_eq!(diff.removed_panels, BTreeSet::from([r(8)]));
        assert_eq!(diff.recalc_panels, BTreeSet::from([r(10)]));
        assert_eq!(diff.renumbered_rooms.len(), 1);
        assert_eq!(diff.renumbered_rooms[0].panels, vec![r(7)]);
    }

    fn box_mesh() -> PlantMesh {
        let v = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let vertices = vec![
            v(0.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(1.0, 1.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(1.0, 0.0, 1.0),
            v(1.0, 1.0, 1.0),
            v(0.0, 1.0, 1.0),
        ];
        let indices = vec![
            0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 1, 2, 6, 1, 6, 5, 2, 3, 7, 2, 7,
            6, 3, 0, 4, 3, 4, 7,
        ];
        let mut mesh = PlantMesh {
            indices,
            vertices,
            normals: vec![],
            wire_vertices: vec![],
            aabb: None,
        };
        mesh.aabb = mesh.cal_aabb();
        mesh
    }

    #[test]
    fn tri_meshes_are_shared_across_generations_by_geo_hash_and_world_matrix() {
        let dir = tempfile::tempdir().unwrap();
        box_mesh()
            .ser_to_file(&dir.path().join("box.mesh"))
            .unwrap();
        let meshes: Arc<MeshCache> = Arc::default();
        let first = model_of(
            groups(),
            vec![inst(7, "box", 0.0, (0.0, 1.0))],
            meshes.clone(),
        );
        let second = model_of(
            groups(),
            vec![inst(7, "box", 0.0, (0.0, 1.0))],
            meshes.clone(),
        );
        let a = first.panel_tri_meshes(r(7), dir.path()).unwrap();
        let b = second.panel_tri_meshes(r(7), dir.path()).unwrap();
        assert_eq!(a.len(), 1);
        assert!(
            Arc::ptr_eq(&a[0], &b[0]),
            "同 geo_hash + 同世界矩阵 = 同一份三角网"
        );
        assert_eq!(meshes.len(), 1);
        assert_eq!(a[0].vertices().len(), 8);

        // 同一份 .mesh、面板挪了位置：矩阵变了就是另一把键，不复用旧位置的网格。
        let moved = model_of(
            groups(),
            vec![inst(7, "box", 5.0, (5.0, 6.0))],
            meshes.clone(),
        );
        let c = moved.panel_tri_meshes(r(7), dir.path()).unwrap();
        assert!(!Arc::ptr_eq(&a[0], &c[0]));
        assert_eq!(meshes.len(), 2);
        assert!(c[0].vertices().iter().all(|p| p.x >= 5.0));

        // 网格文件不在：失败带路径，不静默成空集。
        let broken = model_of(
            groups(),
            vec![inst(7, "missing", 0.0, (0.0, 1.0))],
            meshes.clone(),
        );
        let failures = broken.panel_tri_meshes(r(7), dir.path()).unwrap_err();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("missing.mesh"), "{failures:?}");
        // 不在模型里的面板同样是失败，不是空集。
        assert!(first.panel_tri_meshes(r(8), dir.path()).is_err());
    }

    #[test]
    fn for_membership_never_hands_latest_to_ingest_and_keeps_two_generations() {
        let mut rooms = ResidentRooms::default();
        assert!(rooms.for_membership(DataFace::Ingest).is_none());
        assert!(rooms.for_membership(DataFace::ReadThrough).is_none());

        let published_a = ResidentRoomModel::assemble(
            manifest(ResidentForm::Published, "p-a"),
            groups(),
            map_of(&groups()),
            vec![],
            rooms.mesh_cache(),
        );
        let first = rooms.install(published_a);
        assert!(first.diff.is_none(), "第一代没有上一代");
        assert!(rooms.is_current(&manifest(ResidentForm::Published, "p-a")));
        assert!(
            !rooms.is_current(&manifest(ResidentForm::Latest, "p-a")),
            "键相同形态不同不算同一份"
        );

        let latest = ResidentRoomModel::assemble(
            manifest(ResidentForm::Latest, "l-1"),
            groups(),
            map_of(&groups()),
            vec![],
            rooms.mesh_cache(),
        );
        rooms.install(latest);
        let ingest = rooms
            .for_membership(DataFace::Ingest)
            .expect("摄入拿 published");
        assert_eq!(ingest.manifest.form, ResidentForm::Published);
        let read_through = rooms
            .for_membership(DataFace::ReadThrough)
            .expect("读透拿 latest");
        assert_eq!(read_through.manifest.form, ResidentForm::Latest);

        let renamed = vec![
            group(5, "/1RX-RM03-R309", &[7, 8]),
            group(6, "/1RX-RM03-R302", &[9]),
        ];
        let published_b = ResidentRoomModel::assemble(
            manifest(ResidentForm::Published, "p-b"),
            renamed.clone(),
            map_of(&renamed),
            vec![],
            rooms.mesh_cache(),
        );
        let second = rooms.install(published_b);
        let diff = second.diff.expect("换代给出与上一代的差");
        assert_eq!(diff.renumbered_rooms.len(), 1);
        let (current, previous) = rooms.generations(ResidentForm::Published);
        assert_eq!(current.unwrap().manifest.key, "p-b");
        assert_eq!(previous.unwrap().manifest.key, "p-a");
        assert!(
            rooms
                .for_membership(DataFace::ReadThrough)
                .is_some_and(|m| m.manifest.key == "l-1"),
            "换 published 不动 latest"
        );

        let published_c = ResidentRoomModel::assemble(
            manifest(ResidentForm::Published, "p-c"),
            renamed.clone(),
            map_of(&renamed),
            vec![],
            rooms.mesh_cache(),
        );
        rooms.install(published_c);
        let (current, previous) = rooms.generations(ResidentForm::Published);
        assert_eq!(current.unwrap().manifest.key, "p-c");
        assert_eq!(previous.unwrap().manifest.key, "p-b", "只留两代");
        assert_eq!(rooms.stats().len(), 2);
    }

    /// 预热按缓存键去重；`.mesh` 读不出的面板搬进保护集、不阻代（ADR-073 D9）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn eager_mesh_warm_deduplicates_keys_and_demotes_a_panel_whose_mesh_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        box_mesh()
            .ser_to_file(&dir.path().join("box.mesh"))
            .unwrap();
        let meshes: Arc<MeshCache> = Arc::default();
        let mut model = model_of(
            vec![group(5, "/1RX-RM03-R301", &[7, 8])],
            vec![
                inst(7, "box", 0.0, (0.0, 1.0)),
                inst(8, "box", 0.0, (0.0, 1.0)),
            ],
            meshes.clone(),
        );
        model.warm_meshes(dir.path()).await.expect("warm succeeds");
        let stats = model.stats();
        assert_eq!(
            (
                stats.mesh_keys_expected,
                stats.mesh_keys_warmed,
                stats.mesh_warm_failed
            ),
            (1, 1, 0)
        );
        assert!(stats.mesh_bytes > 0);
        assert_eq!(meshes.len(), 1);

        // `.mesh` 读不出：这块面板搬进保护集、记一条缺口，**不阻代**（ADR-073 D9，源码钉 12）。
        let mut broken = model_of(
            vec![group(5, "/1RX-RM03-R301", &[7, 8])],
            vec![
                inst(7, "missing", 0.0, (0.0, 1.0)),
                inst(8, "box", 0.0, (0.0, 1.0)),
            ],
            meshes,
        );
        let before = broken.geometry_digest.clone();
        broken.warm_meshes(dir.path()).await.expect(
            "a panel whose mesh cannot be read degrades the generation, it does not block it",
        );
        assert_eq!(broken.stats().mesh_warm_failed, 1);
        assert!(
            broken.panel(r(7)).is_none() && broken.panel(r(8)).is_some(),
            "7 搬出有几何那一侧，8 照旧"
        );
        assert_eq!(broken.missing_panels, vec![r(7)], "7 进保护集");
        assert_eq!(broken.problems.len(), 1);
        assert!(
            broken.problems[0].contains("stage=mesh-warm")
                && broken.problems[0].contains(&format!("panel={}", r(7))),
            "{:?}",
            broken.problems
        );
        assert_ne!(
            broken.geometry_digest, before,
            "候选集变了，几何摘要（input_version 的一部分）随之重算"
        );
        assert!(
            matches!(lifecycle_for(&broken), ResidentLifecycle::Degraded { .. }),
            "带缺口的模型 = degraded"
        );
    }

    /// 几何侧的口径（ADR-073 D9，2026-09-16；取代共识 D-A (a) 的「有行却不可用挡参照」）：
    /// **从未生成**与**有几何行却不可用**的在册面板一律只留痕、不阻代——它们不进索引、逐根写边
    /// 时是保护集。此前后者整份 Err → resident `Blocked` → 之后每个任务的全部根判死，
    /// 一块坏面板换整库房间不算，与「一处输入缺失」的分量不成比例。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn eager_mesh_warm_degrades_on_unusable_geometry_and_never_generated_panels_alike() {
        let dir = tempfile::tempdir().unwrap();
        box_mesh()
            .ser_to_file(&dir.path().join("box.mesh"))
            .unwrap();

        // 面板 7 有可用几何；8、9 在册但一条几何行都没有（从未生成）。
        let mut sparse = model_of(
            groups(),
            vec![inst(7, "box", 0.0, (0.0, 1.0))],
            Arc::default(),
        );
        assert!(
            sparse.missing_panels.contains(&r(8)) && sparse.unusable_panels.is_empty(),
            "夹具前提：8/9 是「从未生成」而不是「不可用」: {:?}",
            sparse.missing_panels
        );
        sparse
            .warm_meshes(dir.path())
            .await
            .expect("从未生成的面板只留痕，不阻整代");
        let stats = sparse.stats();
        assert_eq!(
            (
                stats.mesh_keys_expected,
                stats.mesh_keys_warmed,
                stats.mesh_warm_failed
            ),
            (1, 1, 0)
        );
        assert_eq!(stats.missing_panels, 2, "留痕照旧：8、9 在指标里点名");
        assert!(
            sparse.problems.is_empty()
                && matches!(lifecycle_for(&sparse), ResidentLifecycle::Ready { .. }),
            "从未生成不是缺口：没有边可陈旧，几何到来时随它的根算；模型 ready"
        );

        // 面板 9 有几何行、包围盒不可用（NaN）→ 只留痕，理由点名 stage=geometry；不阻代。
        let mut unusable = model_of(
            groups(),
            vec![
                inst(7, "box", 0.0, (0.0, 1.0)),
                inst(9, "box", 0.0, (f32::NAN, 1.0)),
            ],
            Arc::default(),
        );
        assert!(
            unusable.unusable_panels.contains(&r(9)),
            "夹具前提：9 不可用"
        );
        unusable
            .warm_meshes(dir.path())
            .await
            .expect("不可用几何只留痕，不阻代（ADR-073 D9）");
        assert!(
            unusable.panel(r(9)).is_none() && unusable.missing_panels.contains(&r(9)),
            "9 不进有几何那一侧，进保护集"
        );
        let problems = unusable.problems.join("\n");
        assert!(
            problems.contains("stage=geometry")
                && problems.contains("reason=invalid-geometry")
                && problems.contains(&format!("panel={}", r(9))),
            "{problems}"
        );
        match lifecycle_for(&unusable) {
            ResidentLifecycle::Degraded { problems, .. } => assert_eq!(problems.len(), 1),
            other => panic!("带缺口的模型 = degraded，不是 {other:?}"),
        }
    }

    #[tokio::test]
    async fn mesh_pruning_drops_warmed_keys_that_never_reached_install() {
        let dir = tempfile::tempdir().unwrap();
        box_mesh()
            .ser_to_file(&dir.path().join("box.mesh"))
            .unwrap();
        let meshes: Arc<MeshCache> = Arc::default();
        let mut rooms = ResidentRooms {
            meshes: meshes.clone(),
            ..Default::default()
        };
        let mut installed = model_of(
            vec![group(5, "/1RX-RM03-R301", &[7])],
            vec![inst(7, "box", 0.0, (0.0, 1.0))],
            meshes.clone(),
        );
        installed.warm_meshes(dir.path()).await.unwrap();
        rooms.install(installed);

        let mut abandoned = model_of(
            vec![group(6, "/1RX-RM03-R302", &[8])],
            vec![inst(8, "box", 3.0, (0.0, 1.0))],
            meshes.clone(),
        );
        abandoned.warm_meshes(dir.path()).await.unwrap();
        assert_eq!(meshes.len(), 2);

        rooms.prune_meshes();
        assert_eq!(meshes.len(), 1);
    }

    /// 完成定义 #5 的业务 ready 门（S1）：初始化未完成时，会写房间边的入口拿不到放行——
    /// `Loading` / `Blocked` 结构化 `UPDATING`。`Disabled` / `Unsupported`
    /// 是「本进程没有 resident 体制」，放行走今天的路：开关判定收在状态机里，
    /// `room_model.rs` 不判开关（源码钉 `the_full_rebuild_entry_never_takes_the_resident_model`）。
    /// **`Degraded` 放行**（ADR-073 D10，源码钉 13）：装好了、带着已点名的缺口，计算器照常冻。
    ///
    /// 全局 lifecycle 单例，测完复原；本测试二进制里没有别的用例在运行期碰这几个入口。
    #[test]
    fn the_ready_gate_returns_structured_updating_until_the_model_is_ready() {
        let restore = lifecycle();

        mark_loading("mesh-warm");
        let error = ensure_ready().expect_err("loading 必须拦");
        assert!(
            error.to_string().starts_with(RESIDENT_MODEL_UPDATING),
            "门要给结构化 UPDATING，不是随手一句话: {error}"
        );
        assert!(error.to_string().contains("loading:mesh-warm"), "{error}");

        set_lifecycle(ResidentLifecycle::Blocked {
            problems: vec!["建模失败".into()],
        });
        assert!(ensure_ready().is_err(), "blocked 没有可信的当前代");

        set_lifecycle(ResidentLifecycle::Degraded {
            generation: "g1".into(),
            problems: vec!["dbnum=6003 stage=source reason=missing-desi-file name=SCBDESI".into()],
        });
        assert!(
            ensure_ready().is_ok(),
            "degraded = 装好了、带着已点名的缺口：计算器照常冻，缺口在保护集 / 不在册（ADR-073 D10）"
        );

        set_lifecycle(ResidentLifecycle::Ready {
            generation: "g2".into(),
            completed_at: String::new(),
        });
        assert!(ensure_ready().is_ok());

        mark_disabled("configuration");
        assert!(ensure_ready().is_ok(), "关着 = 没有体制，走今天的路，不拦");
        mark_unsupported("project_hd/project_hh feature is absent");
        assert!(ensure_ready().is_ok());

        set_lifecycle(restore);
    }
    /// ADR-073 D12：重建没成时留旧代继续算，`Degraded` 记一条 `stage=refresh` 缺口——同一条只留最新，
    /// 不随每个任务开工一次次堆；代号是计算输入摘要（与 `lifecycle_for` 同一个字串），模型自带的缺口原样在前。
    #[test]
    fn a_failed_refresh_keeps_the_previous_generation_and_exactly_one_refresh_problem() {
        let mut previous = model_of(
            groups(),
            vec![inst(7, "box", 0.0, (0.0, 1.0))],
            Arc::default(),
        );
        previous
            .problems
            .push("dbnum=6003 stage=source reason=missing-desi-file name=SCBDESI".into());
        let ResidentLifecycle::Degraded {
            generation,
            problems,
        } = lifecycle_after_refresh_failure(&previous, "boom 1")
        else {
            panic!("留旧代 = degraded");
        };
        assert_eq!(generation, previous.input_version());
        assert_eq!(problems.len(), 2);
        assert_eq!(
            problems[0],
            "dbnum=6003 stage=source reason=missing-desi-file name=SCBDESI"
        );
        assert!(
            problems[1].starts_with("stage=refresh") && problems[1].contains("boom 1"),
            "{problems:?}"
        );
        // 上一次的 refresh 缺口若混进模型缺口里，再失败一次也只留最新那一条。
        previous.problems = problems;
        let ResidentLifecycle::Degraded { problems, .. } =
            lifecycle_after_refresh_failure(&previous, "boom 2")
        else {
            panic!("留旧代 = degraded");
        };
        assert_eq!(problems.len(), 2, "同一条只留最新一次: {problems:?}");
        assert!(
            problems[1].contains("boom 2") && !problems.iter().any(|p| p.contains("boom 1")),
            "{problems:?}"
        );
        match lifecycle_for(&previous) {
            ResidentLifecycle::Degraded { generation: g, .. } => assert_eq!(g, generation),
            other => panic!("{other:?}"),
        }
    }

    /// 源码钉（ADR-073 D12）：任务开工前对齐接在每一处冻计算器 / 冻代号**之前**——批次执行体的模型半边、
    /// 初始化双 lane 的模型半边、读透侧 ensure 的准入（启动房间屏障那一档显式带代号，不换）；
    /// 探针不得起 `E3dModelService`（那一步整套装 schema，每个任务开工付一次就把任务拖垮）。
    #[test]
    fn the_task_start_refresh_precedes_every_calculator_freeze_and_stays_cheap() {
        let worker = include_str!("../data_interface/batch_worker.rs");
        let body = worker
            .split_once("async fn execute_frozen_batch_body(")
            .expect("批次执行体")
            .1;
        let refresh = body
            .find("refresh_room_reference_before_task(job.dbnum)")
            .expect("批次执行体的模型半边开工前必须对齐常驻房间模型");
        let side_actions = body
            .find("run_model_side_actions(mgr, &side_actions)")
            .expect("位姿 / 删除 / 级联");
        let worklist = body.find("run_unit_worklist(").expect("单元工作单");
        assert!(
            refresh < side_actions && refresh < worklist,
            "对齐必须早于冻计算器的每一处"
        );

        let init = include_str!("../data_interface/dbnum_initialize.rs");
        let half = init
            .split_once("async fn model_half_inner(")
            .expect("初始化模型半边")
            .1;
        assert!(
            half.find("refresh_room_reference_before_task(dbnum)")
                .expect("初始化对齐")
                < half
                    .find("frozen_room_calculator()")
                    .expect("初始化冻计算器"),
            "初始化双 lane 的模型半边先对齐再冻"
        );

        let ensure = include_str!("../data_interface/model_dbnum_ensure.rs");
        let refresh = ensure
            .find("refresh_room_reference_before_task(dbnum)")
            .expect("ensure 准入对齐");
        let generation = ensure
            .find("required_room_generation()?;")
            .expect("ensure 准入冻代号");
        assert!(refresh < generation, "代号在对齐之后冻");
        assert!(
            ensure[..refresh].contains("explicit_room_generation().is_none()"),
            "启动房间屏障那一档不换代"
        );

        let topology = include_str!("room_topology.rs");
        let probe = topology
            .split_once("pub(crate) async fn latest_frozen_sources(")
            .expect("探针")
            .1
            .split_once("\n}\n")
            .expect("函数总有结尾")
            .0;
        assert!(
            !probe.contains("E3dModelService::from_current")
                && !probe.contains("from_frozen_sources(")
                && !probe.contains("open_from_env"),
            "探针不装 schema、不起 E3D 服务: {probe}"
        );
    }

    /// 参照行只动 `resident_reference` 一格：空间凭据原样，`room_model` 那种按字段投影的读法
    /// 不受影响；读回来逐字相等。
    #[tokio::test]
    async fn resident_reference_roundtrips_and_leaves_the_spatial_stamp_alone() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_resident")
            .use_db("reference")
            .await
            .unwrap();
        db.query(
            "UPSERT room_build:main SET spatial_epoch = 355, tree_entries = 105650, \
             rebuild_required = false, built_at = time::now();",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        assert!(read_resident_reference_on(&db).await.unwrap().is_none());

        let model = model_of(groups(), vec![], Arc::default())
            .with_source_tokens(vec!["tok-a".into(), "tok-b".into()]);
        let reference = ResidentReference::for_model(&model);
        assert_eq!(reference.form, "published");
        assert_eq!((reference.rooms, reference.registered_panels), (2, 3));
        write_resident_reference_on(&db, &reference).await.unwrap();
        assert_eq!(
            read_resident_reference_on(&db).await.unwrap(),
            Some(reference.clone())
        );

        #[derive(serde::Deserialize)]
        struct Stamp {
            spatial_epoch: u64,
            tree_entries: u64,
            rebuild_required: Option<bool>,
        }
        let mut response = db
            .query("SELECT spatial_epoch, tree_entries, rebuild_required FROM room_build:main;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let stamps: Vec<Stamp> = response.take(0).unwrap();
        assert_eq!(stamps.len(), 1);
        assert_eq!(
            (
                stamps[0].spatial_epoch,
                stamps[0].tree_entries,
                stamps[0].rebuild_required
            ),
            (355, 105650, Some(false)),
            "空间凭据一格没动"
        );

        // 再写一次是覆盖，不是追加。
        let mut newer = reference.clone();
        newer.digest = "other".into();
        write_resident_reference_on(&db, &newer).await.unwrap();
        assert_eq!(read_resident_reference_on(&db).await.unwrap(), Some(newer));
    }

    #[test]
    fn plan_startup_covers_no_reference_unchanged_apply_mismatch_and_unreadable() {
        let current_published = model_of(groups(), vec![], Arc::default());
        assert_eq!(
            plan_startup(None, &current_published, None),
            StartupRoomPlan::NoReference
        );

        let mut same = ResidentReference::for_model(&current_published);
        assert_eq!(
            plan_startup(Some(&same), &current_published, None),
            StartupRoomPlan::Unchanged
        );

        // 形态换了：参照说的不是这一路。
        same.form = "latest".into();
        assert_eq!(
            plan_startup(Some(&same), &current_published, None),
            StartupRoomPlan::NoReference
        );

        // 同键摘要不等 = 同一组文件版本拓扑却不同 = 库被动过（两形态同判）。
        let mut tampered = ResidentReference::for_model(&current_published);
        tampered.digest = "not-the-same".into();
        assert!(matches!(
            plan_startup(Some(&tampered), &current_published, None),
            StartupRoomPlan::IntegrityMismatch { .. }
        ));

        // 键不同（文件真前进了）：摘要不等 → 拿旧视图 diff；旧视图读不回来 → 不猜。
        let renamed = vec![
            group(5, "/1RX-RM03-R309", &[7, 8]),
            group(6, "/1RX-RM03-R302", &[9]),
        ];
        let current_latest = ResidentRoomModel::assemble(
            manifest(ResidentForm::Latest, "c2"),
            renamed.clone(),
            map_of(&renamed),
            vec![],
            Arc::default(),
        );
        let old_latest = ResidentRoomModel::assemble(
            manifest(ResidentForm::Latest, "c1"),
            groups(),
            map_of(&groups()),
            vec![],
            Arc::default(),
        );
        let reference = ResidentReference::for_model(&old_latest);
        let plan = plan_startup(Some(&reference), &current_latest, Some(Ok(&old_latest.map)));
        let StartupRoomPlan::Apply(diff) = &plan else {
            panic!("{plan:?}");
        };
        assert_eq!(diff.renumbered_rooms.len(), 1);
        assert!(plan.summary().starts_with("对比 → "), "{}", plan.summary());
        assert_eq!(
            plan_startup(
                Some(&reference),
                &current_latest,
                Some(Err("checksum".into()))
            ),
            StartupRoomPlan::ReferenceUnreadable("checksum".into())
        );
        assert!(matches!(
            plan_startup(Some(&reference), &current_latest, None),
            StartupRoomPlan::ReferenceUnreadable(_)
        ));
    }

    /// d-42：摄入侧不再是恒等式——键不同就拿旧视图 diff，与读透同一张臂表；`IntegrityMismatch`
    /// 只留给「同键不等」，两形态同判，旧视图给了也不看。回退到「摄入侧摘要不等即 mismatch」即红。
    #[test]
    fn plan_startup_diffs_by_key_in_both_forms_and_reserves_mismatch_for_the_same_key() {
        let old = model_of(groups(), vec![], Arc::default());
        assert_eq!(old.manifest.form, ResidentForm::Published);
        let renamed = vec![
            group(5, "/1RX-RM03-R309", &[7, 8]),
            group(6, "/1RX-RM03-R302", &[9]),
        ];
        let current = ResidentRoomModel::assemble(
            manifest(ResidentForm::Published, "m2"),
            renamed.clone(),
            map_of(&renamed),
            vec![],
            Arc::default(),
        );
        let reference = ResidentReference::for_model(&old);
        assert_ne!(reference.key, current.manifest.key);

        let plan = plan_startup(Some(&reference), &current, Some(Ok(&old.map)));
        let StartupRoomPlan::Apply(diff) = &plan else {
            panic!("摄入侧键不同必须走 diff，不是 IntegrityMismatch: {plan:?}");
        };
        assert_eq!(diff.renumbered_rooms.len(), 1);
        assert!(diff.removed_panels.is_empty() && diff.recalc_panels.is_empty());
        assert!(matches!(
            plan_startup(Some(&reference), &current, None),
            StartupRoomPlan::ReferenceUnreadable(_)
        ));
        assert_eq!(
            plan_startup(Some(&reference), &current, Some(Err("checksum".into()))),
            StartupRoomPlan::ReferenceUnreadable("checksum".into())
        );

        let mut same_key = ResidentReference::for_model(&current);
        same_key.digest = "tampered".into();
        assert!(matches!(
            plan_startup(Some(&same_key), &current, Some(Ok(&old.map))),
            StartupRoomPlan::IntegrityMismatch { .. }
        ));
        let current_latest = ResidentRoomModel::assemble(
            manifest(ResidentForm::Latest, "c1"),
            renamed.clone(),
            map_of(&renamed),
            vec![],
            Arc::default(),
        );
        let mut latest_same_key = ResidentReference::for_model(&current_latest);
        latest_same_key.digest = "tampered".into();
        assert!(matches!(
            plan_startup(Some(&latest_same_key), &current_latest, Some(Ok(&old.map))),
            StartupRoomPlan::IntegrityMismatch { .. }
        ));
    }
    /// 改列只碰交集里面板的边：`room_num` 变、边 id 不变、别的房间与不在交集的面板原样。
    #[tokio::test]
    async fn renumber_statements_change_only_the_room_num_column_of_the_intersection() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_resident").use_db("renumber").await.unwrap();
        db.query(
            "CREATE pe:24384_1 SET noun = 'FRMW'; CREATE pe:24384_2 SET noun = 'FRMW'; \
             CREATE pe:24384_7 SET noun = 'PANE'; CREATE pe:24384_8 SET noun = 'PANE'; CREATE pe:24384_11 SET noun = 'PANE'; \
             CREATE pe:24384_5 SET noun = 'EQUI'; CREATE pe:24384_6 SET noun = 'BOX'; CREATE pe:24384_9 SET noun = 'EQUI'; CREATE pe:24384_12 SET noun = 'EQUI'; \
             INSERT RELATION INTO room_panel_relate [{ id: room_panel_relate:⟨24384_1_24384_7⟩, in: pe:24384_1, out: pe:24384_7, room_num: 'R301' }]; \
             RELATE pe:24384_1->room_panel_relate->pe:24384_8 SET room_num = 'R301'; \
             RELATE pe:24384_2->room_panel_relate->pe:24384_11 SET room_num = 'R302'; \
             INSERT RELATION INTO room_relate [{ id: room_relate:⟨24384_7_24384_5⟩, in: pe:24384_7, out: pe:24384_5, room_num: 'R301' }]; \
             RELATE pe:24384_7->room_relate->pe:24384_6 SET room_num = 'R301'; \
             RELATE pe:24384_8->room_relate->pe:24384_9 SET room_num = 'R301'; \
             RELATE pe:24384_11->room_relate->pe:24384_12 SET room_num = 'R302';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let statements = render_renumber_statements(&RoomRenumber {
            room: r(1),
            old_num: "R301".into(),
            new_num: "R309".into(),
            panels: vec![r(7)],
        });
        assert_eq!(statements.len(), 2);
        db.query(format!(
            "BEGIN TRANSACTION; {} COMMIT TRANSACTION;",
            statements.join(" ")
        ))
        .await
        .expect("transport")
        .check()
        .expect("statements");

        async fn room_nums(
            db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
            sql: &str,
        ) -> Vec<String> {
            let mut response = db.query(sql).await.unwrap().check().unwrap();
            let mut nums: Vec<String> = response.take(0).unwrap();
            nums.sort();
            nums
        }
        assert_eq!(
            room_nums(&db, "SELECT VALUE room_num FROM pe:24384_7->room_relate;").await,
            vec!["R309", "R309"],
            "7 的两条成员边改列"
        );
        assert_eq!(
            room_nums(
                &db,
                "SELECT VALUE room_num FROM pe:24384_7<-room_panel_relate;"
            )
            .await,
            vec!["R309"],
            "7 的房间边改列"
        );
        assert_eq!(
            room_nums(&db, "SELECT VALUE room_num FROM pe:24384_8->room_relate;").await,
            vec!["R301"],
            "8 不在交集里，原样"
        );
        assert_eq!(
            room_nums(&db, "SELECT VALUE room_num FROM pe:24384_11->room_relate;").await,
            vec!["R302"],
            "别的房间原样"
        );
        let ids = room_nums(&db, "SELECT VALUE <string>id FROM pe:24384_7->room_relate;").await;
        assert!(
            ids.iter().any(|id| id.contains("24384_7_24384_5")),
            "边 id 不动: {ids:?}"
        );
        let edges: Vec<surrealdb::sql::Thing> = db
            .query("SELECT VALUE id FROM room_relate;")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(edges.len(), 4, "改列不增删边");
    }

    /// 回退即红：归属路径只许经 [`ensure_current`]（它内部是唯一调 [`ResidentRooms::for_membership`]
    /// 的地方）取模型，不许自己摸进程单例、两代或点名形态。
    ///
    /// 启动对比（`room_startup.rs`）的换代点走的就是 `ensure_current`；哪一路由
    /// `data_face()` 决定，代码里出现 `ResidentForm::Latest` 就是绕开了这个决定。注释可以提
    /// ——`room_model.rs` 的凭据读法就在注释里点名 `resident_blocked` 为什么会先于全量重建
    /// 把行建出来。
    #[test]
    fn membership_paths_only_take_the_model_through_for_membership() {
        for (name, source) in [("room_model.rs", include_str!("room_model.rs"))] {
            let mentions: Vec<&str> = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .filter(|line| {
                    line.contains("room_resident")
                        || line.contains("ResidentRooms")
                        || line.contains("resident_rooms()")
                })
                .collect();
            for line in &mentions {
                assert!(
                    !line.contains(".generations(")
                        && !line.contains("ResidentForm::Latest")
                        && !line.contains("resident_rooms()"),
                    "{name}: 归属路径不得直接摸进程单例、两代或点名形态: {line}"
                );
            }
            if !mentions.is_empty() {
                assert!(
                    source.contains("for_membership(") || source.contains("ensure_current("),
                    "{name}: 用了常驻模型却没经 for_membership / ensure_current 取"
                );
            }
        }
    }

    /// `resident_blocked` 只动自己那一格：写原因 / 清成 NONE 都不碰 `resident_reference` 与空间凭据；
    /// 原因里的引号与换行照样往返。
    #[tokio::test]
    async fn resident_blocked_roundtrips_and_leaves_the_rest_of_the_record_alone() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_resident").use_db("blocked").await.unwrap();
        db.query("UPSERT room_build:main SET spatial_epoch = 355, tree_entries = 105650;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let reference = ResidentReference::for_model(&model_of(groups(), vec![], Arc::default()));
        write_resident_reference_on(&db, &reference).await.unwrap();
        assert_eq!(read_resident_blocked_on(&db).await.unwrap(), None);

        let reason = "发布清单为空（基线没建成）：'不装' \"零间房\" 的模型\n第二行";
        db.query(render_resident_blocked_write(Some(reason)))
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(
            read_resident_blocked_on(&db).await.unwrap().as_deref(),
            Some(reason)
        );
        assert_eq!(
            read_resident_reference_on(&db).await.unwrap(),
            Some(reference.clone()),
            "参照那一格没动"
        );

        db.query(render_resident_blocked_write(None))
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(read_resident_blocked_on(&db).await.unwrap(), None);
        #[derive(serde::Deserialize)]
        struct Stamp {
            spatial_epoch: u64,
            tree_entries: u64,
        }
        let mut response = db
            .query("SELECT spatial_epoch, tree_entries FROM room_build:main;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let stamps: Vec<Stamp> = response.take(0).unwrap();
        assert_eq!(
            (stamps[0].spatial_epoch, stamps[0].tree_entries),
            (355, 105650),
            "空间凭据没动"
        );
        assert_eq!(
            read_resident_reference_on(&db).await.unwrap(),
            Some(reference)
        );
    }

    /// `/health.room_build.resident`：无几何的在册面板按库点名，限定域之外的库单独成一句；
    /// 全在限定域里就没有那一句；启动结局原样带出。
    ///
    /// 这里的 24384 是**退路**——`model_of` 没给文件归属，只好按面板 refno 的头一段报。
    /// 生产两路都给（[`missing_geometry_is_grouped_by_the_file_dbnum_not_the_refno_dbnum`]）。
    #[test]
    fn resident_health_names_unwatched_libraries_with_missing_geometry() {
        let model = model_of(
            groups(),
            vec![inst(7, "mesh-a", 0.0, (0.0, 1.0))],
            Arc::default(),
        );
        let stats = vec![model.stats()];
        let report = StartupInitReport {
            blocked: None,
            summary: Some("[房间初始化] published …".into()),
            expected_dbnums: vec![8000],
            duration_ms: Some(2950),
            rss_bytes: Some(512 * 1024 * 1024),
            at: "2026-09-07T23:59:00+08:00".into(),
        };

        let health = resident_health_from(Some(&report), None, &stats, |dbnum| dbnum != 24384);
        assert_eq!(health["startup"]["blocked"], serde_json::Value::Null);
        assert_eq!(health["startup"]["summary"], "[房间初始化] published …");
        assert_eq!(
            health["last_swap"],
            serde_json::Value::Null,
            "还没换过代：这一格是 null，不是零计数"
        );
        assert_eq!(health["models"].as_array().unwrap().len(), 1);
        assert_eq!(health["models"][0]["form"], "published");
        assert_eq!(health["models"][0]["missing_panels"], 2);
        assert_eq!(
            health["models"][0]["missing_geometry"],
            serde_json::json!([{ "dbnum": 24384, "panels": 2, "watched": false }])
        );
        let notice = health["unwatched_missing_geometry_notice"]
            .as_str()
            .expect("限定域之外的库要点名");
        assert!(
            notice.starts_with("2 块在册面板无几何，来自未监听的库 [24384]"),
            "{notice}"
        );
        assert!(notice.contains("watch_dbnums"), "{notice}");

        let health = resident_health_from(None, None, &stats, |_| true);
        assert_eq!(health["startup"], serde_json::Value::Null);
        assert_eq!(health["models"][0]["missing_geometry"][0]["watched"], true);
        assert_eq!(
            health["unwatched_missing_geometry_notice"],
            serde_json::Value::Null,
            "库都在限定域里：无几何是模型链还没到，不是限定域圈出来的"
        );

        let blocked = StartupInitReport {
            blocked: Some("枚举 MDB 设计库失败".into()),
            summary: None,
            expected_dbnums: Vec::new(),
            duration_ms: None,
            rss_bytes: None,
            at: String::new(),
        };
        let health = resident_health_from(Some(&blocked), None, &[], |_| true);
        assert_eq!(health["startup"]["blocked"], "枚举 MDB 设计库失败");
        assert_eq!(health["models"], serde_json::json!([]));
    }

    /// `/health.room_build.resident.last_swap`（P4）：最近一次换代的 diff 摘要与护栏结论原样带出，
    /// 且与启动那一格互不覆盖。
    #[test]
    fn resident_health_carries_the_last_generation_swap_and_the_guard_verdict() {
        let swap = GenerationSwapReport {
            form: "published".into(),
            key: "abc123".into(),
            diff: Some("removed=1 recalc=2 renumbered_rooms=0 (panels 0)".into()),
            touched_panels: 3,
            matched_targets: 0,
            round_targets: 4,
            guard_fired: true,
            note: Some("账本触发可能漏了".into()),
            at: "2026-09-08T10:00:00+08:00".into(),
        };
        let health = resident_health_from(None, Some(&swap), &[], |_| true);
        assert_eq!(health["last_swap"]["guard_fired"], true);
        assert_eq!(health["last_swap"]["touched_panels"], 3);
        assert_eq!(health["last_swap"]["matched_targets"], 0);
        assert_eq!(health["last_swap"]["round_targets"], 4);
        assert_eq!(
            health["last_swap"]["diff"],
            "removed=1 recalc=2 renumbered_rooms=0 (panels 0)"
        );
        assert_eq!(health["last_swap"]["note"], "账本触发可能漏了");
        assert_eq!(health["startup"], serde_json::Value::Null);
    }

    /// 「无几何按库」报的是**文件**库号，不是面板 refno 的头一段——两套编号：ams7997 里的
    /// 元素 refno 形如 `24381_*`，而 `watch_dbnums` / 发布清单 / 水位说的都是 7997。
    ///
    /// 照 refno 报有两处错，2026-09-08 的真库门（`watch_dbnums=[8000]`）两处都撞上了：
    /// `/health` 把「7997 的 229 块面板」写成「24381」；而 `is_watched` 收到的是 refno 库号，
    /// 拿去问 `watch_scope::admits` 恒假，**被监听**库的面板也会被诬成「来自未监听的库」。
    #[test]
    fn missing_geometry_is_grouped_by_the_file_dbnum_not_the_refno_dbnum() {
        let by_file = vec![
            (7997, vec![group(5, "/1RX-RM03-R301", &[7, 8])]),
            (8000, vec![group(6, "/1RX-RM03-R302", &[9])]),
        ];
        let room_files = room_files_of(&by_file);
        let flat = crate::fast_model::room_topology::flatten_groups_by_file(by_file);
        let map = registered_map(&flat, &keywords(), match_room_name_hd);
        let model = ResidentRoomModel::assemble(
            manifest(ResidentForm::Published, "m1"),
            flat,
            map,
            vec![inst(7, "mesh-a", 0.0, (0.0, 1.0))],
            Arc::default(),
        )
        .with_room_files(room_files);

        assert_eq!(model.missing_panels.len(), 2, "8 与 9 没有几何行");
        assert_eq!(
            model.stats().missing_panels_by_dbnum,
            BTreeMap::from([(7997, 1), (8000, 1)]),
            "键是文件库号；面板 refno 的头一段 24384 一个都不许出现"
        );

        // 限定域说的也是文件库号：7997 未监听要点名，8000 监听着不许算进那一句。
        let health = resident_health_from(None, None, &[model.stats()], |dbnum| dbnum == 8000);
        let notice = health["unwatched_missing_geometry_notice"]
            .as_str()
            .expect("7997 未监听");
        assert!(
            notice.starts_with("1 块在册面板无几何，来自未监听的库 [7997]"),
            "{notice}"
        );

        // 没有文件归属的模型（纯测搭的）退回 refno 头一段，不至于把面板数报成 0。
        assert_eq!(
            model_of(groups(), vec![], Arc::default())
                .stats()
                .missing_panels_by_dbnum,
            BTreeMap::from([(24384, 3)]),
        );
    }

    /// 回执一行：每个文件 `dbnum@sesno 动作`（前进带 `旧→新`、动过的带冻结耗时）、清单前进的
    /// 告警、模型统计里无几何按库分组（Q20 (a)）；读透没有清单，只报每个令牌的冻结耗时。
    #[test]
    fn startup_init_summary_names_every_file_and_groups_missing_geometry_by_dbnum() {
        let model = Arc::new(model_of(
            groups(),
            vec![inst(7, "mesh-a", 0.0, (0.0, 1.0))],
            Arc::default(),
        ));
        let step = |dbnum: u32, sesno: u32, action: ManifestAction| ManifestStep {
            dbnum,
            path: PathBuf::from(format!("ams{dbnum}")),
            sesno,
            action,
        };
        let outcome = StartupInitOutcome {
            form: ResidentForm::Published,
            steps: vec![
                step(7997, 155, ManifestAction::Added),
                step(8000, 12, ManifestAction::OwnedByRounds),
                step(8001, 7, ManifestAction::Current),
                step(8002, 3, ManifestAction::Advanced { from_sesno: 2 }),
            ],
            frozen: vec![(7997, 155, 7370), (8002, 3, 180)],
            advance_warning: None,
            model: Arc::clone(&model),
            elapsed_ms: 8000,
        };
        let line = outcome.summary();
        assert!(line.starts_with("[房间初始化] published 文件 ["), "{line}");
        assert!(line.contains("7997@155 新增 7370ms"), "{line}");
        assert!(
            line.contains("8000@12 归轮次,"),
            "没动的文件不带耗时: {line}"
        );
        assert!(line.contains("8001@7 保留,"), "{line}");
        assert!(line.contains("8002@2→3 前进 180ms]"), "{line}");
        assert!(
            line.contains(
                "→ 2 间房 / 3 块在册面板 / 1 块有几何 / 2 块无几何 无几何按库 {24384: 2} / 网格 0/0 键 0.0 MB 耗时 8000ms"
            ),
            "{line}"
        );
        assert!(!line.contains('（'), "没告警就没有括号: {line}");

        let warned = StartupInitOutcome {
            form: ResidentForm::Published,
            steps: vec![step(7997, 155, ManifestAction::Added)],
            frozen: Vec::new(),
            advance_warning: Some("清单前进跳过：存在未完成的文件发布".into()),
            model: Arc::clone(&model),
            elapsed_ms: 12,
        };
        let line = warned.summary();
        assert!(
            line.contains("[7997@155 新增]（清单前进跳过：存在未完成的文件发布） → "),
            "{line}"
        );

        let latest = StartupInitOutcome {
            form: ResidentForm::Latest,
            steps: Vec::new(),
            frozen: vec![(8000, 12, 5), (7997, 155, 7370)],
            advance_warning: None,
            model,
            elapsed_ms: 7400,
        };
        let line = latest.summary();
        assert!(
            line.starts_with(
                "[房间初始化] latest 文件 [8000@12 冻结 5ms, 7997@155 冻结 7370ms] → "
            ),
            "{line}"
        );
    }

    /// 源码钉（P2 换代点）：身份探针只许用那一个规范键——两形态都是
    /// `room_topology::latest_frozen_sources` 的 composite（发布清单键随 ADR-062 作废）。它在这里
    /// 自己拼一个摘要，键就与模型自带的 `manifest` 对不上：每轮都判「文件变了」、每轮整份重建，
    /// 而且不报错。
    #[test]
    fn the_manifest_probe_takes_both_keys_from_their_canonical_source() {
        let source = include_str!("room_resident.rs");
        let probe = source
            .split_once("async fn probe_manifest(")
            .expect("probe_manifest 必须存在")
            .1
            .split_once("pub(crate) async fn ensure_current(")
            .expect("probe_manifest 后面就是 ensure_current")
            .0;
        assert!(
            !probe.contains("published_file_inputs") && !probe.contains("manifest_key("),
            "两种形态均使用文件侧版本，不能读旧发布快照: {probe}"
        );
        assert!(
            probe.contains("latest_frozen_sources()"),
            "读透侧的键必须来自 room_topology::latest_frozen_sources: {probe}"
        );
        assert!(
            !probe.contains("Sha256") && !probe.contains("::digest("),
            "探针不得自己拼摘要，两个键各只有一个定义: {probe}"
        );
    }

    /// 源码钉（P2 换代点）：读透侧那个 composite 只许有一个定义。`load_room_groups_from_files`
    /// 自己再算一遍的话，它与探针会在「令牌怎么排序、怎么序列化」上悄悄分家——键一漂，
    /// 每轮都是整份重建（ams7997 一次 6–7 s），却没有任何一处会报错。
    #[test]
    fn the_read_through_change_key_has_exactly_one_definition() {
        let source = include_str!("room_topology.rs");
        assert_eq!(
            source.matches("digest::digest(").count(),
            1,
            "room_topology.rs 里只许 latest_frozen_sources 算这个摘要"
        );
        let loader = source
            .split_once("pub(crate) async fn load_room_groups_from_files(")
            .expect("load_room_groups_from_files 必须存在")
            .1
            .split_once("\n}\n")
            .expect("函数总有结尾")
            .0;
        assert!(
            loader.contains("latest_frozen_sources().await?"),
            "文件侧读法必须与探针共用 latest_frozen_sources: {loader}"
        );
    }

    /// 源码钉（原则 4）：四份房间文件不认识 `watch_scope`——哪些清单行归文件轮次管由调用方传进来，
    /// 房间是参照系，不是增量的一部分。注释里可以提它（说明为什么不引用），代码行里不许出现
    /// 这个模块名（`use` 与 `watch_scope::…` 路径都算）。`room_startup.rs`（启动对比与应用，d-42）
    /// 同受此钉：对比不限初始化前进过的库（D3），它不需要知道限定域。
    #[test]
    fn room_files_never_consult_the_watch_scope() {
        let needle = ["watch_", "scope"].concat();
        let names_the_module = |code: &str| {
            code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .any(|token| token == needle.as_str())
        };
        for (name, source) in [
            ("room_resident.rs", include_str!("room_resident.rs")),
            ("room_topology.rs", include_str!("room_topology.rs")),
        ] {
            for (index, line) in source.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                assert!(
                    !names_the_module(code),
                    "{name}:{}: 房间文件不得引用 {needle}（限定域圈的是数据批次，房间是参照系）: {line}",
                    index + 1
                );
            }
        }
    }
}
