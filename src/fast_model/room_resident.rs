//! 常驻房间模型（Resident Room Model）——房间→面板拓扑与在册面板几何的进程内缓存。
//!
//! 2026-09-07 grill 四轮定下（`docs/plans/2026-09-07-resident-room-model-plan.md` §8，共识 d-3）：
//!
//! 1. **一份真值，两份视图。** 它是库里已发布物（摄入：`room_published_topology` +
//!    `room_published_inst` 当前清单）或实时行（读透）的**缓存**，不是第二份真值：换代只以
//!    清单 / 会话摘要为键整份替换（[`ResidentRooms::install`]），不逐条修补。`latest` 与
//!    `published` 两份并存，**回答归属的入口只许经 [`ResidentRooms::for_membership`]**——
//!    摄入形态永远拿不到 `latest`。拿错了不报错，只是房间号静默算错，所以这一条是源码钉。
//! 2. **判不了就不写。** 合成拓扑是跨文件的（`room_topology_source::compose` 一张 lookup），
//!    任一文件读不到 / 清单分片不齐 → 整份 `Err`（[`ResidentBuildError`]），不产半份模型。
//! 3. **触发源只有一个。** 结构触发是账本（`room_structural_trigger`）；这里的 [`TopologyDiff`]
//!    只在启动对比与护栏两处出手，**不产** `RoomRecalcPanel`。
//!
//! P1（本文件）只有本体：合成、摘要、diff、网格缓存、两代存放，一行生产路径未接。接线
//! （`drain_rooms_selected` 进串行锁后按清单键懒换代、三条归属入口改读这里）在 P2，
//! 开关 `room_resident_model` 默认关。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{GeomInstQuery, RefnoEnum};
use anyhow::Context;
use glam::Mat4;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::shape::{TriMesh, TriMeshFlags};
use sha2::{Digest, Sha256};

use crate::data_interface::side_effect_pending::PublishedFileInput;
use crate::fast_model::room_model::{RoomPanelMap, RoomPanels, room_panel_map_from_groups};
use crate::fast_model::room_predicate::aabb_is_usable;
use crate::fast_model::room_topology::{RoomGroup, RoomHierarchy, room_panel_groups};
use crate::options::DataFace;

/// 一份模型跟着哪一路形态走。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ResidentForm {
    /// 摄入：按发布清单冻结（ADR-062），键 = `room_published_topology.manifest_key`。
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

/// 三角网缓存键：`.mesh` 按内容寻址不可变，键里再带世界矩阵的位模式，命中即可复用、
/// 永不陈旧（工程默认①）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
}

fn build_tri_mesh(mesh_dir: &Path, geo_hash: &str, matrix: Mat4) -> Result<TriMesh, String> {
    let file_path = mesh_dir.join(format!("{geo_hash}.mesh"));
    let mesh = PlantMesh::des_mesh_file(&file_path)
        .map_err(|error| format!("{}: {error}", file_path.display()))?;
    mesh.get_tri_mesh_with_flag(
        matrix,
        TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
    )
    .ok_or_else(|| format!("{}: 三角网转换失败", file_path.display()))
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
    /// 未过关键字的原始分组，与 `room_published_topology.groups` 同形；摘要从它算。
    pub groups: Vec<RoomGroup>,
    /// 过关键字 + 命名校验的在册视图；`all_panels` 是成员候选的排除集。
    pub map: RoomPanelMap,
    /// 在册且几何可用的面板。
    pub panels: BTreeMap<RefnoEnum, ResidentPanel>,
    /// 在册却没拿到几何或包围盒不可用的面板（与 `PanelIndex::missing_panels` 同口径、同序）。
    pub missing_panels: Vec<RefnoEnum>,
    pub digest: String,
    /// 读透：各 DESI 的冻结令牌（按 dbnum 序），启动参照按它们读分片；摄入为空（参照就是
    /// `room_published_topology`）。
    pub source_tokens: Vec<String>,
    pub built_at: std::time::Instant,
    meshes: Arc<MeshCache>,
}

impl fmt::Debug for ResidentRoomModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResidentRoomModel")
            .field("manifest", &self.manifest)
            .field("rooms", &self.map.rooms.len())
            .field("registered_panels", &registered_panels(&self.map).len())
            .field("panels_with_geometry", &self.panels.len())
            .field("missing_panels", &self.missing_panels.len())
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
    pub meshes_cached: usize,
}

impl ResidentRoomModel {
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
        missing_panels.sort_by_key(RefnoEnum::to_string);
        Self {
            manifest,
            groups,
            map,
            panels,
            missing_panels,
            digest,
            source_tokens: Vec::new(),
            built_at: std::time::Instant::now(),
            meshes,
        }
    }

    pub(crate) fn with_source_tokens(mut self, tokens: Vec<String>) -> Self {
        self.source_tokens = tokens;
        self
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

    pub(crate) fn stats(&self) -> ResidentStats {
        ResidentStats {
            form: self.manifest.form.as_str(),
            key: self.manifest.key.clone(),
            digest: self.digest.clone(),
            rooms: self.map.rooms.len(),
            registered_panels: registered_panels(&self.map).len(),
            panels_with_geometry: self.panels.len(),
            missing_panels: self.missing_panels.len(),
            meshes_cached: self.meshes.len(),
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

/// 摄入侧：从库里当前清单选中的合成分组与版本化几何合成。`db` 显式传入，`mem://` 门直接喂。
pub(crate) async fn load_published_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    inputs: &[PublishedFileInput],
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
    meshes: Arc<MeshCache>,
) -> anyhow::Result<ResidentRoomModel> {
    let key = crate::fast_model::room_topology::published_topology_manifest_key(inputs, hierarchy);
    let groups =
        crate::fast_model::room_topology::load_published_topology_groups_on(db, inputs, hierarchy)
            .await
            .map_err(|error| {
                ResidentBuildError::new(
                    ResidentForm::Published,
                    Some(key.clone()),
                    format!("读发布拓扑失败: {error:#}"),
                )
            })?;
    let map = registered_map(&groups, room_key_words, match_room_fn);
    let registered = registered_panels(&map);
    let insts = crate::fast_model::room_publication::query_published_room_insts_on(db, &registered)
        .await
        .map_err(|error| {
            ResidentBuildError::new(
                ResidentForm::Published,
                Some(key.clone()),
                format!(
                    "读 {} 块在册面板的已发布几何失败: {error:#}",
                    registered.len()
                ),
            )
        })?;
    Ok(ResidentRoomModel::assemble(
        ResidentManifest {
            form: ResidentForm::Published,
            key,
        },
        groups,
        map,
        insts,
        meshes,
    ))
}

/// 摄入侧生产入口。`None` = 还没有发布清单（升级基线未建），调用方保留今天的路。
pub(crate) async fn load_published(
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
    meshes: Arc<MeshCache>,
) -> anyhow::Result<Option<ResidentRoomModel>> {
    let inputs =
        crate::data_interface::side_effect_pending::SideEffectCompensator::published_file_inputs()
            .await
            .map_err(|error| {
                ResidentBuildError::new(
                    ResidentForm::Published,
                    None,
                    format!("读发布清单失败: {error:#}"),
                )
            })?;
    if inputs.is_empty() {
        return Ok(None);
    }
    load_published_on(
        &aios_core::SUL_DB,
        &inputs,
        hierarchy,
        room_key_words,
        match_room_fn,
        meshes,
    )
    .await
    .map(Some)
}

/// 读透侧：从当前 MDB 各设计库文件的生成会话读分组（`room_topology` 的文件侧遍历），
/// 几何取实时行。任一文件读不到 → 整份失败。
pub(crate) async fn load_latest(
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
    meshes: Arc<MeshCache>,
) -> anyhow::Result<ResidentRoomModel> {
    let (composite, tokens, groups) =
        crate::fast_model::room_topology::load_room_groups_from_files(hierarchy)
            .await
            .map_err(|error| {
                ResidentBuildError::new(
                    ResidentForm::Latest,
                    None,
                    format!("读文件侧房间分组失败: {error:#}"),
                )
            })?;
    let map = registered_map(&groups, room_key_words, match_room_fn);
    let registered = registered_panels(&map);
    let insts = crate::data_interface::staging::query_valid_insts(&registered)
        .await
        .map_err(|error| {
            ResidentBuildError::new(
                ResidentForm::Latest,
                Some(composite.clone()),
                format!(
                    "读 {} 块在册面板的实时几何失败: {error:#}",
                    registered.len()
                ),
            )
        })?;
    Ok(ResidentRoomModel::assemble(
        ResidentManifest {
            form: ResidentForm::Latest,
            key: composite,
        },
        groups,
        map,
        insts,
        meshes,
    )
    .with_source_tokens(tokens))
}

// ---------------------------------------------------------------------------
// 启动参照（Q9 (a)）与启动对比（Q3 / Q6 / Q7）——P3 里不碰邻线脏文件的那一半。
// 应用侧（按 diff 删 / 重算 / 改列、接进 `lib.rs` 启动段）随 P3 后半截进。
// ---------------------------------------------------------------------------

/// 上次成功结算所依据的那组文件版本与拓扑摘要，落在 `room_build:main.resident_reference`。
///
/// 读透：`tokens` 是各 DESI 的冻结令牌，分片按它们从 `room_topology_source` 读回来合成
/// 「旧」那份；摄入：`tokens` 为空，参照就是 `room_published_topology` 本身（与常驻模型同源，
/// 摘要不等即库被恢复 / 手改）。
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

/// 读透侧：把这份模型依据的每个文件版本冻成 `room_topology_source` 分片（已有同令牌的分片就
/// 复用，`prepare_producer_source` 幂等），再写参照行。**只在一轮成功结算之后调**——参照说的是
/// 「边是按这份拓扑算出来的」，算失败的轮次不许推进它。
pub(crate) async fn persist_latest_reference(
    model: &ResidentRoomModel,
) -> anyhow::Result<ResidentReference> {
    anyhow::ensure!(
        model.manifest.form == ResidentForm::Latest,
        "只有读透侧的模型需要写分片参照；摄入侧的参照就是 room_published_topology"
    );
    for token in &model.source_tokens {
        let source =
            crate::data_interface::frozen_source::FrozenSource::from_commit_token_json(token)?;
        crate::fast_model::room_topology_source::prepare_producer_source(&source)
            .await
            .with_context(|| format!("冻结 dbnum {} 的房间拓扑分片", source.dbnum()))?;
    }
    let reference = ResidentReference::for_model(model);
    write_resident_reference_on(&aios_core::SUL_DB, &reference).await?;
    Ok(reference)
}

/// 启动对比的「旧」那份：按参照里的令牌读分片、合成、过关键字。任一分片缺失 / 校验失败
/// → 整份失败（原则 2），调用方按「参照不可读」处置，不拿半份去 diff。
pub(crate) async fn load_reference_map_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    reference: &ResidentReference,
    hierarchy: RoomHierarchy,
    room_key_words: &[String],
    match_room_fn: impl Fn(&str) -> bool,
) -> anyhow::Result<(Vec<RoomGroup>, RoomPanelMap)> {
    let mut sources = Vec::with_capacity(reference.tokens.len());
    for token in &reference.tokens {
        sources.push(
            crate::fast_model::room_topology_source::load_on(db, token)
                .await
                .context("读启动参照的房间拓扑分片")?,
        );
    }
    let composed = crate::fast_model::room_topology_source::compose(&sources, hierarchy)?;
    let groups: Vec<RoomGroup> = composed
        .into_iter()
        .flat_map(|(_, groups)| groups)
        .collect();
    let map = registered_map(&groups, room_key_words, match_room_fn);
    Ok((groups, map))
}

/// 启动该做什么（Q7 (a)：全量降为显式修复入口，其余走对比）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupRoomPlan {
    /// 没有参照（开关第一次打开 / 库刚建 / 形态换了）：按今天的口径跑一次全量并写参照。
    NoReference,
    /// 参照摘要 == 现状摘要：零写入，只刷参照时间。
    Unchanged,
    /// 参照能读回来且摘要不同：按 diff 删 / 重算 / 改列。
    Apply(TopologyDiff),
    /// 摄入侧参照与常驻模型同源却对不上：库被恢复 / 手改，标 `rebuild_required` 走显式全量。
    IntegrityMismatch { expected: String, actual: String },
    /// 参照在但读不回来（分片缺失 / 校验失败）：不猜，走显式全量。
    ReferenceUnreadable(String),
}

impl StartupRoomPlan {
    pub(crate) fn summary(&self) -> String {
        match self {
            Self::NoReference => "无参照 → 全量一次并写参照".into(),
            Self::Unchanged => "参照摘要 == 现状 → 零写入".into(),
            Self::Apply(diff) => format!("对比 → {}", diff.summary()),
            Self::IntegrityMismatch { expected, actual } => format!(
                "摄入侧参照与常驻模型同源却不等（{} ≠ {}）→ rebuild_required",
                &expected[..expected.len().min(12)],
                &actual[..actual.len().min(12)]
            ),
            Self::ReferenceUnreadable(reason) => {
                format!("参照不可读（{reason}）→ rebuild_required")
            }
        }
    }
}

/// 纯函数：参照 + 现状 + （读透）参照合成的旧视图 → 计划。`reference_map` 只在读透且
/// 摘要不等时才被看；读不回来传 `Err(原因)`。
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
    match current.manifest.form {
        ResidentForm::Published => StartupRoomPlan::IntegrityMismatch {
            expected: reference.digest.clone(),
            actual: current.digest.clone(),
        },
        ResidentForm::Latest => match reference_map {
            Some(Ok(old)) => StartupRoomPlan::Apply(TopologyDiff::between(old, &current.map)),
            Some(Err(reason)) => StartupRoomPlan::ReferenceUnreadable(reason),
            None => StartupRoomPlan::ReferenceUnreadable(
                "读透侧对比需要参照合成的旧视图，调用方没给".into(),
            ),
        },
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
        InstallOutcome { model, diff }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::room_model::match_room_name_hd;
    use aios_core::RefU64;
    use bevy_transform::components::Transform;
    use glam::Vec3;
    use parry3d::math::Point;

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

    fn published_input(dbnum: u32, end_sesno: i32, token: &str) -> PublishedFileInput {
        PublishedFileInput {
            dbnum,
            db_type: "DESI".into(),
            end_sesno,
            commit_token: token.into(),
            published_version: 1,
        }
    }

    /// `mem://` 门：从 `room_published_topology` + `room_published_inst` 合成的模型，拓扑与
    /// 库里那一行逐字相等、几何只取当前令牌、缺分片整份失败。
    #[tokio::test]
    async fn published_model_is_composed_from_the_manifest_tables_and_fails_closed() {
        use crate::fast_model::room_topology::published_topology_manifest_key;
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_resident")
            .use_db("published")
            .await
            .unwrap();
        let inputs = vec![
            published_input(8000, 5, "token-a"),
            published_input(8001, 7, "token-b"),
        ];
        let key = published_topology_manifest_key(&inputs, RoomHierarchy::Hd);
        let shard_a = vec![group(5, "/1RX-RM03-R301", &[7])];
        let shard_b = vec![group(6, "/1RX-RM03-R302", &[9])];
        db.query(format!(
            "CREATE room_published_topology:a SET manifest_key = '{key}', dbnum = 8000, groups = {}; \
             CREATE published_file_input:8000 SET dbnum = 8000, commit_token = 'token-a'; \
             CREATE published_file_input:8001 SET dbnum = 8001, commit_token = 'token-b'; \
             CREATE pe:24384_7 SET noun = 'PANE'; CREATE pe:24384_9 SET noun = 'PANE'; \
             CREATE inst_relate:24384_7 SET dbnum = 8000, in = pe:24384_7, \
               anc = [104728482619399, 104728482619393], generic = 'PANE', \
               aabb_d = {{ mins: [0f, 0f, 0f], maxs: [1f, 1f, 1f] }}, \
               world_trans_d = {{ translation: [0f, 0f, 0f], rotation: [0f, 0f, 0f, 1f], scale: [1f, 1f, 1f] }}, \
               insts_flat = [{{ geo_hash: 'mesh-old' }}], booled_id = NONE; \
             CREATE inst_relate:24384_9 SET dbnum = 8001, in = pe:24384_9, \
               anc = [104732777586696, 104732777586689], generic = 'PANE', \
               aabb_d = {{ mins: [2f, 0f, 0f], maxs: [3f, 1f, 1f] }}, \
               world_trans_d = {{ translation: [0f, 0f, 0f], rotation: [0f, 0f, 0f, 1f], scale: [1f, 1f, 1f] }}, \
               insts_flat = [{{ geo_hash: 'mesh-b' }}], booled_id = NONE;",
            serde_json::to_string(&shard_a).unwrap()
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        // 当前令牌的几何 + 一份还没切进清单的候选（新几何）。
        db.query(format!(
            "BEGIN TRANSACTION; {} {} COMMIT TRANSACTION;",
            crate::fast_model::room_publication::render_stage_published_room_input(8000, "token-a"),
            crate::fast_model::room_publication::render_stage_published_room_input(8001, "token-b"),
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query("UPDATE inst_relate:24384_7 SET insts_flat = [{ geo_hash: 'mesh-new' }];")
            .await
            .unwrap()
            .check()
            .unwrap();
        db.query(format!(
            "BEGIN TRANSACTION; {} COMMIT TRANSACTION;",
            crate::fast_model::room_publication::render_stage_published_room_input(
                8000,
                "token-a-next"
            ),
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        // 缺 8001 的分片：整份失败，错误能认出是「建不起来」。
        let error = load_published_on(
            &db,
            &inputs,
            RoomHierarchy::Hd,
            &keywords(),
            match_room_name_hd,
            Arc::default(),
        )
        .await
        .expect_err("缺一个文件分片必须失败关闭");
        let build_error = error
            .downcast_ref::<ResidentBuildError>()
            .expect("ResidentBuildError");
        assert_eq!(build_error.form, ResidentForm::Published);
        assert_eq!(build_error.key.as_deref(), Some(key.as_str()));
        assert!(
            build_error
                .to_string()
                .contains("expected dbnums [8000, 8001]"),
            "{build_error}"
        );

        db.query(format!(
            "CREATE room_published_topology:b SET manifest_key = '{key}', dbnum = 8001, groups = {};",
            serde_json::to_string(&shard_b).unwrap()
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        let model = load_published_on(
            &db,
            &inputs,
            RoomHierarchy::Hd,
            &keywords(),
            match_room_name_hd,
            Arc::default(),
        )
        .await
        .expect("两片齐了");
        assert_eq!(model.manifest, manifest(ResidentForm::Published, &key));
        assert_eq!(model.groups, [shard_a.clone(), shard_b.clone()].concat());
        assert_eq!(model.digest, topology_digest(&[shard_a, shard_b].concat()));
        assert_eq!(registered_panels(&model.map), vec![r(7), r(9)]);
        assert!(
            model.missing_panels.is_empty(),
            "{:?}",
            model.missing_panels
        );
        let seven = model.panel(r(7)).unwrap();
        assert_eq!(seven.room_num, "R301");
        assert_eq!(
            seven
                .insts
                .iter()
                .map(|inst| inst.geo_hash.as_str())
                .collect::<Vec<_>>(),
            vec!["mesh-old"],
            "候选令牌 token-a-next 的新几何在清单切换前不可见"
        );
        assert_eq!(
            model.panel(r(9)).unwrap().world_aabb,
            Aabb::new(Point::new(2.0, 0.0, 0.0), Point::new(3.0, 1.0, 1.0))
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

    fn shard(
        dbnum: u32,
        room_name: &str,
    ) -> crate::fast_model::room_topology_source::RoomTopologySource {
        use crate::data_interface::generation_root::refno_from_e3d;
        use crate::fast_model::room_topology_source::{TopologyNode, fixture_source};
        use e3d_io::refno::RefNo;

        let node = |id: u32, noun: &str, name: &str, members: Vec<u32>| TopologyNode {
            refno: refno_from_e3d(RefNo::new(dbnum, id)),
            noun: noun.into(),
            name: name.into(),
            members: members
                .into_iter()
                .map(|member| refno_from_e3d(RefNo::new(dbnum, member)))
                .collect(),
        };
        let mut source = fixture_source(dbnum, 1);
        source.roots = vec![refno_from_e3d(RefNo::new(dbnum, 1))];
        source.nodes = vec![
            node(1, "WORL", "", vec![2]),
            node(2, "FRMW", room_name, vec![3]),
            node(3, "PANE", "", vec![]),
        ];
        source
    }

    /// 读透参照的旧视图从持久分片合成；缺一片整份失败，不拿半份去 diff。
    #[tokio::test]
    async fn reference_map_is_composed_from_persisted_shards_and_fails_closed_when_one_is_missing()
    {
        use crate::data_interface::generation_root::refno_from_e3d;
        use crate::fast_model::room_topology_source::persist_on;
        use e3d_io::refno::RefNo;
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_resident")
            .use_db("reference_map")
            .await
            .unwrap();
        let a = shard(1, "/A-RM-R101");
        let b = shard(2, "/B-RM-R201");
        persist_on(&db, &a).await.unwrap();
        let reference = ResidentReference {
            form: "latest".into(),
            key: "composite".into(),
            digest: "d".into(),
            tokens: vec![a.source_token.clone(), b.source_token.clone()],
            rooms: 2,
            registered_panels: 2,
            settled_at: String::new(),
        };
        let error = load_reference_map_on(
            &db,
            &reference,
            RoomHierarchy::Hd,
            &keywords(),
            match_room_name_hd,
        )
        .await
        .expect_err("b 的分片还没落");
        assert!(
            format!("{error:#}").contains("missing exact room topology source"),
            "{error:#}"
        );

        persist_on(&db, &b).await.unwrap();
        let (groups, map) = load_reference_map_on(
            &db,
            &reference,
            RoomHierarchy::Hd,
            &keywords(),
            match_room_name_hd,
        )
        .await
        .unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(map.rooms.len(), 2);
        assert_eq!(
            map.room_num_of(refno_from_e3d(RefNo::new(2, 3))),
            Some("R201")
        );
        assert_eq!(
            topology_digest(&groups),
            topology_digest(&groups.iter().rev().cloned().collect::<Vec<_>>())
        );
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

        // 摄入侧摘要不等 = 库被动过。
        let mut tampered = ResidentReference::for_model(&current_published);
        tampered.digest = "not-the-same".into();
        assert!(matches!(
            plan_startup(Some(&tampered), &current_published, None),
            StartupRoomPlan::IntegrityMismatch { .. }
        ));

        // 读透侧：摘要不等 → 拿旧视图 diff；旧视图读不回来 → 不猜。
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

    /// 回退即红：归属路径只许经 `for_membership` 取模型。P2 接线前这里是空转（两份文件里
    /// 还没有 `room_resident` 的字样），接线后任何一处直接摸 `latest` / `generations` 都红。
    #[test]
    fn membership_paths_only_take_the_model_through_for_membership() {
        for (name, source) in [
            ("room_model.rs", include_str!("room_model.rs")),
            (
                "model_update_pending.rs",
                include_str!("../data_interface/model_update_pending.rs"),
            ),
        ] {
            let mentions: Vec<&str> = source
                .lines()
                .filter(|line| {
                    line.contains("room_resident")
                        || line.contains("ResidentRooms")
                        || line.contains("resident_rooms()")
                })
                .collect();
            for line in &mentions {
                assert!(
                    !line.contains(".generations(") && !line.contains("ResidentForm::Latest"),
                    "{name}: 归属路径不得直接摸两代或点名 latest: {line}"
                );
            }
            if !mentions.is_empty() {
                assert!(
                    source.contains("for_membership("),
                    "{name}: 用了常驻模型却没经 for_membership 取"
                );
            }
        }
    }
}
