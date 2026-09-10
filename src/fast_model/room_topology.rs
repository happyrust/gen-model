//! 房间 → 面板拓扑的**文件侧**来源（ADR-010 房间面在 ADR-056 N7 / ADR-053 direct 读模式下的替身）。
//!
//! legacy 的 `room_model::load_room_panel_groups`（2026-09-08 随 S4 退役）从 SurrealDB 的 noun 表
//! （`FRMW` / `SBFR`）+ `pe_owner` 图读「哪些是房间、房间下有哪些面板」，于是不跑数据解析的部署
//! 与从未解析过的库上是 **0 间房**——`build_room_relations` 打印「0 间房 / 0 块面板」、盖章成功，
//! 房间子系统整体静默不工作。本模块把同一份口径搬到 e3d-io `DbSet` 上，如今是**唯一**来源：
//!
//! - **hd**（`project_hd`）：房间 = `FRMW`，面板 = 它的子与孙里 noun 为 `PANE` 的元素
//!   （legacy SQL 是 `array::flatten([id<-pe_owner<-pe, id<-pe_owner<-pe<-pe_owner<-pe])[?noun='PANE']`
//!   ——孙经**任何**中间 noun，不只 CWALL / CFLOOR；这里逐字照搬）。
//! - **hh**（`project_hh`）：房间 = `SBFR`，面板 = 直接子元素里的 `PANE`。
//!
//! 遍历与 [`enumerate_generation_roots_in_subtree`](crate::data_interface::generation_root)
//! 同一形状：只认 `SubtreeElement{noun, name, members}` 三格，每个元素恰一次 lookup，成员表
//! 重复与成环只算一次，读不到就整体报错。房间关键字过滤与房间号（NAME 按 `-` 切的最后一段）
//! 留在 [`room_panel_groups`]，与 SQL 侧 `'{kw}' in NAME` / `array::last(string::split(NAME,'-'))`
//! 同义；**空关键字不匹配任何房间**（触发侧 `collect_room_structural_triggers` 同一口径；
//! SQL 侧 `'' in NAME` 会匹配一切，那是个从没人依赖过的边角）。
//!
//! 一个库一个会话的原始分组（未过关键字）按 `(dbnum, sesno, 层级)` 缓存：会话内容不可变，
//! 而房间映射「按轮复用」的调用方（drain、`execute_item`、启动全量）今天各自都要重载一遍。
//! 缓存只留两代（[`GROUP_CACHE_GENERATIONS`]，常驻房间模型 P2-b）。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use aios_core::RefnoEnum;
use anyhow::Context;
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;

use crate::data_interface::generation_root::{
    SubtreeElement, refno_from_e3d, subtree_element_from_set,
};
use crate::fast_model::room_model::{RoomPanelMap, room_panel_map_from_groups};

/// 房间层级口径按项目在编译期选定（与 `room_model::configured_match_room_fn` 同一对开关）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RoomHierarchy {
    /// `FRMW` → 子 + 孙 `PANE`。
    Hd,
    /// `SBFR` → 直接子 `PANE`。
    Hh,
}

impl RoomHierarchy {
    /// 当前构建的层级口径；两个 project 特性都没开就是 `None`——房间子系统的入口本来就
    /// 响亮拒绝，这里不替它兜。
    pub(crate) fn configured() -> Option<Self> {
        #[cfg(feature = "project_hd")]
        return Some(Self::Hd);
        #[cfg(all(feature = "project_hh", not(feature = "project_hd")))]
        return Some(Self::Hh);
        #[cfg(not(any(feature = "project_hd", feature = "project_hh")))]
        None
    }

    fn room_noun(self) -> &'static str {
        match self {
            Self::Hd => "FRMW",
            Self::Hh => "SBFR",
        }
    }

    /// 面板离房间节点最多几层。
    fn panel_depth(self) -> usize {
        match self {
            Self::Hd => 2,
            Self::Hh => 1,
        }
    }
}

/// 一个房间节点及其名下面板——**未**过关键字与命名校验的原始形态。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RoomGroup {
    pub room: RefnoEnum,
    /// 文件里存的 NAME 原文（可能带前导 `/`）。
    pub name: String,
    /// 按层级口径收到的 PANE，存储成员序前序，去重。
    pub panels: Vec<RefnoEnum>,
}

fn normalized_noun(noun: &str) -> String {
    noun.trim().to_ascii_uppercase()
}

/// 遍历 `roots` 之下的全部元素，收出每个房间节点及其面板。纯函数：只靠 `lookup`。
pub(crate) fn collect_room_groups(
    roots: &[RefnoEnum],
    hierarchy: RoomHierarchy,
    mut lookup: impl FnMut(RefnoEnum) -> anyhow::Result<SubtreeElement>,
) -> anyhow::Result<Vec<RoomGroup>> {
    // 先把整棵树读进内存索引（每个元素恰一次 lookup），再从索引里派生分组——面板要读
    // 房间节点的子与孙，DFS 途中直接再 lookup 会让同一元素被读两遍。
    let mut index: HashMap<RefnoEnum, SubtreeElement> = HashMap::new();
    let mut order: Vec<RefnoEnum> = Vec::new();
    let mut stack: Vec<RefnoEnum> = roots.iter().rev().copied().collect();
    while let Some(current) = stack.pop() {
        if index.contains_key(&current) {
            continue;
        }
        let element = lookup(current)?;
        stack.extend(element.members.iter().rev().copied());
        order.push(current);
        index.insert(current, element);
    }

    let room_noun = hierarchy.room_noun();
    let mut groups = Vec::new();
    for refno in order {
        let element = &index[&refno];
        if normalized_noun(&element.noun) != room_noun {
            continue;
        }
        let mut panels = Vec::new();
        let mut seen = HashSet::new();
        let mut frontier: Vec<RefnoEnum> = element.members.clone();
        for _ in 0..hierarchy.panel_depth() {
            let mut next = Vec::new();
            for member in frontier {
                let Some(child) = index.get(&member) else {
                    // 成员表指向本次遍历没读到的元素（跨库引用）：不算面板，也不往下走。
                    continue;
                };
                if normalized_noun(&child.noun) == "PANE" && seen.insert(member) {
                    panels.push(member);
                }
                next.extend(child.members.iter().copied());
            }
            frontier = next;
        }
        groups.push(RoomGroup {
            room: refno,
            name: element.name.clone(),
            panels,
        });
    }
    Ok(groups)
}

/// 关键字过滤 + 房间号：与 SQL 侧 `'{kw}' in NAME` 与 `array::last(string::split(NAME, '-'))`
/// 同义，输出正好是 [`room_panel_map_from_groups`] 吃的三元组。空关键字不匹配任何房间。
pub(crate) fn room_panel_groups(
    groups: &[RoomGroup],
    room_key_words: &[String],
) -> Vec<(RefnoEnum, String, Vec<RefnoEnum>)> {
    groups
        .iter()
        .filter(|group| {
            room_key_words
                .iter()
                .any(|keyword| !keyword.is_empty() && group.name.contains(keyword.as_str()))
        })
        .map(|group| {
            let room_num = group
                .name
                .rsplit('-')
                .next()
                .unwrap_or_default()
                .to_string();
            (group.room, room_num, group.panels.clone())
        })
        .collect()
}

/// e3d-io 适配：在 `set` 钉住的会话上，从若干顶层元素（通常是 `scan_index(...).roots`）
/// 收出全部房间分组。一行 SurrealDB 都不读。
pub(crate) fn room_groups_from_set(
    set: &Arc<DbSet>,
    roots: &[RefNo],
    hierarchy: RoomHierarchy,
) -> anyhow::Result<Vec<RoomGroup>> {
    let roots: Vec<RefnoEnum> = roots.iter().copied().map(refno_from_e3d).collect();
    collect_room_groups(&roots, hierarchy, |refno| {
        subtree_element_from_set(set, refno)
    })
}

/// 文件侧分组缓存**只留两代**（常驻房间模型 P2-b，与 `ResidentRooms` 同一口径）。
///
/// 「一代」= 一个 `(composite, 层级)`：composite 是这一组文件版本的规范摘要
/// （[`latest_frozen_sources`]），任一 DESI 的会话号变了就是新一代；一代里每个文件各一条
/// （键 `(dbnum, sesno)`，条上带本文件冻结令牌）。此前这张表**从不淘汰**——ams7997 一个
/// 文件的分组是 1 077 组 / 3 937 面板，读透侧每 SAVEWORK 一次就多攒一代，进程活得越久
/// 留下的死代越多，而除了当前那一代与用来做 diff 的上一代，谁也不会再看它们一眼。
///
/// 跨代命中即提升（Q16，ADR-067 实施约束 3，T4.0）：composite 换代后没变的文件
/// （同 dbnum / sesno / 令牌）从旧代直接命中并提进新代，见 [`cached_groups`]。
const GROUP_CACHE_GENERATIONS: usize = 2;

/// 一代里的一条：本文件的分组，连同它的冻结令牌——跨代提升靠令牌认「同一份文件」，
/// `(dbnum, sesno)` 同号换文件时令牌不同，旧产物提不进新代（与磁盘工件键 W2 同判）。
struct CachedFileGroups {
    token: String,
    groups: Arc<Vec<RoomGroup>>,
}

/// 一代：同一组文件版本下，各文件各自的分组。
struct GroupGeneration {
    ident: (String, RoomHierarchy),
    files: HashMap<(u32, u32), CachedFileGroups>,
}

type GroupCache = Mutex<VecDeque<GroupGeneration>>;

fn group_cache() -> &'static GroupCache {
    static CACHE: OnceLock<GroupCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// 找或建 `ident` 那一代，把这条并进去；新建代时排到队尾，超出上限的最老一代出局。
/// **全部写入都走这一条**（[`remember_groups`] 与 Q16 提升共用），淘汰上限只在这里守。
fn insert_groups(
    cache: &mut VecDeque<GroupGeneration>,
    ident: (String, RoomHierarchy),
    file_key: (u32, u32),
    token: String,
    groups: Arc<Vec<RoomGroup>>,
) {
    let entry = CachedFileGroups { token, groups };
    if let Some(generation) = cache
        .iter_mut()
        .find(|generation| generation.ident == ident)
    {
        generation.files.insert(file_key, entry);
        return;
    }
    cache.push_back(GroupGeneration {
        ident,
        files: HashMap::from([(file_key, entry)]),
    });
    while cache.len() > GROUP_CACHE_GENERATIONS {
        cache.pop_front();
    }
}

/// 命中顺序：先在 `composite` 自己那一代里找；没有，再从新到旧看别的代里有没有
/// **同一份文件**（dbnum、sesno、冻结令牌逐字相同，且层级同口径）——有就地**提升**进
/// 当前 composite 的那一代再返回（Q16「跨代命中即提升」）。这样任一邻库前进（composite
/// 变、没变文件的令牌不变）时，没变的文件连磁盘工件都不用读；且命中跟着代走，连续
/// 两次换代也不会随最老一代出局。令牌不同（同号换文件）或层级不同，一律不借。
fn cached_groups(
    key: (u32, u32, String, RoomHierarchy),
    token: &str,
) -> Option<Arc<Vec<RoomGroup>>> {
    let (dbnum, sesno, composite, hierarchy) = key;
    let ident = (composite, hierarchy);
    let mut cache = group_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(generation) = cache.iter().find(|generation| generation.ident == ident) {
        if let Some(entry) = generation.files.get(&(dbnum, sesno)) {
            if entry.token == token {
                return Some(entry.groups.clone());
            }
        }
    }
    let promoted = cache
        .iter()
        .rev()
        .filter(|generation| generation.ident != ident && generation.ident.1 == hierarchy)
        .find_map(|generation| {
            generation
                .files
                .get(&(dbnum, sesno))
                .filter(|entry| entry.token == token)
                .map(|entry| entry.groups.clone())
        })?;
    insert_groups(
        &mut cache,
        ident,
        (dbnum, sesno),
        token.to_string(),
        promoted.clone(),
    );
    Some(promoted)
}

/// 记一条。命中已有的那一代就并进去（不重排——同一代里各文件是一起读出来的，先后无意义）；
/// 是新的一代才排到队尾，队首那一代随之出局（见 [`insert_groups`]）。
fn remember_groups(
    key: (u32, u32, String, RoomHierarchy),
    token: String,
    groups: Arc<Vec<RoomGroup>>,
) {
    let (dbnum, sesno, composite, hierarchy) = key;
    let mut cache = group_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    insert_groups(
        &mut cache,
        (composite, hierarchy),
        (dbnum, sesno),
        token,
        groups,
    );
}

/// 从当前 MDB 的每个设计库文件（各自的生成会话：钉住的或文件最新，ADR-054 Q1）读出房间
/// → 面板映射。`room_model::load_room_panel_map` 在 direct 读模式下走这里。
pub(crate) async fn load_room_panel_map_from_files<F>(
    room_key_words: &[String],
    hierarchy: RoomHierarchy,
    match_room_fn: F,
) -> anyhow::Result<RoomPanelMap>
where
    F: Fn(&str) -> bool,
{
    let (_, _, by_file) = load_room_groups_from_files(hierarchy).await?;
    Ok(room_panel_map_from_groups(
        room_panel_groups(&flatten_groups_by_file(by_file), room_key_words),
        match_room_fn,
    ))
}

/// 摊平「按文件分好的分组」。房间从哪个文件来只有 `/health` 的点名要（见
/// `room_resident::load_live_on` 的 `room_files`），别的读法照旧只看一整份分组。
pub(crate) fn flatten_groups_by_file(by_file: Vec<(u32, Vec<RoomGroup>)>) -> Vec<RoomGroup> {
    by_file.into_iter().flat_map(|(_, groups)| groups).collect()
}

/// 此刻 MDB 全部设计库的冻结源（按 dbnum 序）、各自的提交令牌，以及这一组文件版本的规范
/// 摘要（令牌 JSON 序列化再摘要）。
///
/// 摘要只认「有哪些文件、各停在哪个会话」，**不读一个字节的拓扑内容**——常驻房间模型的
/// 读透侧拿它当换代键（`room_resident::ensure_current` 每轮都要算一次）。它与
/// [`load_room_groups_from_files`] 算出来的必须是同一个字符串，所以两边共用本函数：
/// 各写一份的话，键漂一次就是「每轮都以为文件变了、每轮整份重建」，而且不报错。
pub(crate) async fn latest_frozen_sources() -> anyhow::Result<(
    String,
    Vec<String>,
    Vec<crate::data_interface::frozen_source::FrozenSource>,
)> {
    let option = aios_core::get_db_option();
    let membership =
        crate::data_interface::mdb_membership::get(&option.project_name, &option.mdb_name)
            .map(Ok)
            .unwrap_or_else(|| {
                crate::data_interface::mdb_membership::resolve(
                    option,
                    &option.project_name,
                    &option.mdb_name,
                )
                .map(crate::data_interface::mdb_membership::install)
            })?;
    membership.require_design_files()?;
    let current = crate::fast_model::e3d_model_service::E3dModelService::from_current().await?;
    let mut sources = current
        .design_sources()?
        .into_iter()
        .map(|(_, file, sesno)| {
            crate::data_interface::frozen_source::FrozenSource::capture_at(&file, sesno)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    sources.sort_by_key(|source| source.dbnum());
    let tokens = sources
        .iter()
        .map(|source| source.commit_token_json())
        .collect::<anyhow::Result<Vec<_>>>()?;
    let composite = crate::data_interface::digest::digest(&serde_json::to_vec(&tokens)?);
    Ok((composite, tokens, sources))
}

/// [`load_room_panel_map_from_files`] 的下半截：当前 MDB 全部设计库文件的**原始**分组
/// （未过关键字，**按文件 dbnum 分好**）、各 DESI 的冻结令牌（按 dbnum 序），以及这组文件
/// 版本的规范摘要（令牌 JSON 序列化再摘要）。常驻房间模型（`room_resident`）的读透侧以那个
/// 摘要为换代键、以令牌写启动参照、以分组算拓扑摘要，并按文件 dbnum 给「无几何按库」点名。
pub(crate) async fn load_room_groups_from_files(
    hierarchy: RoomHierarchy,
) -> anyhow::Result<(String, Vec<String>, Vec<(u32, Vec<RoomGroup>)>)> {
    let (composite, tokens, sources) = latest_frozen_sources().await?;
    let service =
        crate::fast_model::e3d_model_service::E3dModelService::from_frozen_sources(&sources, true)
            .await?;
    let mut by_file: Vec<(u32, Vec<RoomGroup>)> = Vec::new();
    for source in sources {
        let started = std::time::Instant::now();
        let (dbnum, file, sesno) = (
            source.dbnum() as u32,
            source.path().to_path_buf(),
            source.target_sesno(),
        );
        let key = (dbnum, sesno, composite.clone(), hierarchy);
        let cache_dir = aios_core::get_db_option()
            .get_meshes_path()
            .join("room-input-cache");
        // 磁盘工件按**本文件**的冻结令牌寻址，不含 composite：一个文件的解析产物只由它
        // 自己的内容 + 会话 + 层级口径决定，把整组文件的 composite 编进键里，等于任一
        // 邻库前进就把没变文件的工件全部作废、整份重解析（ams7997 实测 6–7 s / 文件，
        // 审核 2026-09-09 §3）。令牌带文件身份，同号换文件不会误命中旧产物；原文入键即可
        // ——工件信封自己会把键哈希成文件名、装载时逐字节比对（`model_artifact::path` /
        // `load_value`），这里**不另算摘要**（读透变更键只许 `latest_frozen_sources` 一处
        // 定义，钉 `the_read_through_change_key_has_exactly_one_definition`）。进程内
        // GroupCache 两代语义（composite 为代键）不动，跨代按同一份令牌**命中即提升**
        // （Q16，T4.0）：没变的文件先内存跨代命中，其次磁盘工件，最后才重解析。
        let file_token = source.commit_token_json()?;
        let artifact_key = format!("room-groups-v2:{dbnum}:{sesno}:{file_token}:{hierarchy:?}");
        let cached = match cached_groups(key.clone(), &file_token) {
            Some(cached) => cached,
            None => {
                if let Some(groups) = crate::fast_model::model_artifact::load_value::<Vec<RoomGroup>>(
                    &cache_dir,
                    &artifact_key,
                ) {
                    let groups = Arc::new(groups);
                    remember_groups(key, file_token.clone(), groups.clone());
                    println!(
                        "[ams-phase] phase=room_topology dbnum={dbnum} sesno={sesno} cached=true groups={} elapsed_ms={}",
                        groups.len(),
                        started.elapsed().as_millis()
                    );
                    by_file.push((dbnum, groups.as_ref().clone()));
                    continue;
                }
                let set = service.build_set(dbnum, Some(sesno))?;
                let file: PathBuf = file.clone();
                let fresh =
                    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<RoomGroup>> {
                        let index =
                            crate::fast_model::e3d_model_service::scan_index(&file, Some(sesno))
                                .with_context(|| {
                                    format!("scan index of {} at {sesno}", file.display())
                                })?;
                        room_groups_from_set(&set, &index.roots, hierarchy).with_context(|| {
                            format!("collect room groups of {} at {sesno}", file.display())
                        })
                    })
                    .await
                    .map_err(|error| anyhow::anyhow!("room topology task failed: {error}"))??;
                source.reopen_verified()?;
                crate::fast_model::model_artifact::store_value(&cache_dir, &artifact_key, &fresh)?;
                let fresh = Arc::new(fresh);
                remember_groups(key, file_token.clone(), fresh.clone());
                fresh
            }
        };
        println!(
            "[ams-phase] phase=room_topology dbnum={dbnum} sesno={sesno} groups={} elapsed_ms={}",
            cached.len(),
            started.elapsed().as_millis()
        );
        by_file.push((dbnum, cached.as_ref().clone()));
    }
    Ok((composite, tokens, by_file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::room_model::match_room_name_hd;
    use aios_core::RefU64;
    use std::cell::Cell;

    fn r(id: u32) -> RefnoEnum {
        RefnoEnum::from(RefU64::from_two_nums(24384, id))
    }

    /// 发布视图退役（ADR-072 D4）：本文件不再有 manifest 合成、不再物化拓扑到库、不再从库
    /// 里按清单键选拓扑；文件侧分组仍按文件分好（`by_file` 的 dbnum 是**文件**库号）。
    #[test]
    fn room_topology_has_no_published_view_and_never_queries_the_database() {
        let source = include_str!("room_topology.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        for needle in [
            "SUL_DB",
            ".query(",
            "manifest_key",
            "PUBLISHED_TOPOLOGY_TABLE",
            "load_published_topology_groups_on",
            "load_room_panel_map_from_published_files",
        ] {
            assert!(
                !production.contains(needle),
                "room_topology.rs 生产路径不该再出现 `{needle}`"
            );
        }
        let first = vec![RoomGroup {
            room: r(1),
            name: "/AREA-R001".into(),
            panels: vec![r(2)],
        }];
        let second = vec![RoomGroup {
            room: r(3),
            name: "/AREA-R002".into(),
            panels: vec![r(4)],
        }];
        assert_eq!(
            flatten_groups_by_file(vec![(8000, first.clone()), (8001, second.clone())]),
            [first, second].concat()
        );
    }

    struct Tree {
        nodes: HashMap<RefnoEnum, SubtreeElement>,
        lookups: Cell<usize>,
    }

    impl Tree {
        fn new(spec: &[(u32, &str, &str, &[u32])]) -> Self {
            let nodes = spec
                .iter()
                .map(|(id, noun, name, members)| {
                    (
                        r(*id),
                        SubtreeElement {
                            noun: noun.to_string(),
                            name: name.to_string(),
                            members: members.iter().map(|m| r(*m)).collect(),
                        },
                    )
                })
                .collect();
            Self {
                nodes,
                lookups: Cell::new(0),
            }
        }

        fn lookup(&self, refno: RefnoEnum) -> anyhow::Result<SubtreeElement> {
            self.lookups.set(self.lookups.get() + 1);
            self.nodes
                .get(&refno)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no element {refno}"))
        }
    }

    /// hd：FRMW 的直接 PANE 子、任何中间 noun 下的 PANE 孙都算；曾孙不算；没关键字的
    /// FRMW 也先收进分组（过滤在后一步）；分组按前序、面板按成员序。
    #[test]
    fn hd_rooms_collect_child_and_grandchild_panels_through_any_intermediate() {
        let tree = Tree::new(&[
            (1, "WORL", "/*", &[2]),
            (2, "SITE", "/S", &[3, 20]),
            (3, "ZONE", "/Z", &[4]),
            (4, "STRU", "/1RX", &[5, 15]),
            (5, "FRMW", "/1RX-RM03-R301", &[6, 7, 9, 11]),
            (6, "PANE", "/P1", &[]),
            (7, "CWALL", "/W1", &[8]),
            (8, "PANE", "/P2", &[]),
            (9, "SCTN", "/C1", &[10]),
            (10, "PANE", "/P3", &[]),
            (11, "CFLOOR", "/F1", &[12]),
            (12, "GWALL", "/G1", &[13]),
            (13, "PANE", "/P4-too-deep", &[]),
            (15, "FRMW", "/1RX-STRU-F001", &[16]),
            (16, "PANE", "/P5", &[]),
            (20, "ZONE", "/Z2", &[21]),
            (21, "FRMW", "/2RX-RM01-R101", &[22]),
            (22, "PANE", "/P6", &[]),
        ]);
        let groups =
            collect_room_groups(&[r(1)], RoomHierarchy::Hd, |refno| tree.lookup(refno)).unwrap();
        assert_eq!(
            groups,
            vec![
                RoomGroup {
                    room: r(5),
                    name: "/1RX-RM03-R301".into(),
                    panels: vec![r(6), r(8), r(10)],
                },
                RoomGroup {
                    room: r(15),
                    name: "/1RX-STRU-F001".into(),
                    panels: vec![r(16)],
                },
                RoomGroup {
                    room: r(21),
                    name: "/2RX-RM01-R101".into(),
                    panels: vec![r(22)],
                },
            ]
        );
        assert_eq!(
            tree.lookups.get(),
            tree.nodes.len(),
            "每个元素恰一次 lookup"
        );
    }

    /// hh：SBFR 只收直接子 PANE。
    #[test]
    fn hh_rooms_take_only_direct_panels() {
        let tree = Tree::new(&[
            (1, "WORL", "/*", &[2]),
            (2, "SBFR", "/A-RM-R201", &[3, 4]),
            (3, "PANE", "/P1", &[]),
            (4, "CWALL", "/W", &[5]),
            (5, "PANE", "/P2", &[]),
        ]);
        let groups =
            collect_room_groups(&[r(1)], RoomHierarchy::Hh, |refno| tree.lookup(refno)).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].panels, vec![r(3)]);
    }

    /// 成员表重复、成环、指向没读到的元素：每个元素仍恰一次 lookup，面板不重复，不挂死。
    #[test]
    fn cycles_duplicates_and_dangling_members_are_tolerated() {
        let tree = Tree::new(&[
            (1, "WORL", "/*", &[2, 2]),
            (2, "FRMW", "/X-RM-R001", &[3, 3, 1, 99]),
            (3, "PANE", "/P", &[2]),
        ]);
        let groups = collect_room_groups(&[r(1)], RoomHierarchy::Hd, |refno| {
            if refno == r(99) {
                // 99 是跨库引用：lookup 会被调到但这里模拟读不到。
                return Err(anyhow::anyhow!("dangling"));
            }
            tree.lookup(refno)
        });
        // 读不到就整体报错——与生成根枚举同口径，不静默漏。
        assert!(groups.is_err());

        let tree = Tree::new(&[
            (1, "WORL", "/*", &[2, 2]),
            (2, "FRMW", "/X-RM-R001", &[3, 3, 1]),
            (3, "PANE", "/P", &[2]),
        ]);
        let groups =
            collect_room_groups(&[r(1)], RoomHierarchy::Hd, |refno| tree.lookup(refno)).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].panels, vec![r(3)]);
        assert_eq!(tree.lookups.get(), 3);
    }

    /// 关键字过滤与房间号：`-RM` 命中的才算，房间号是 NAME 按 `-` 切的最后一段；空关键字
    /// 不匹配任何房间；命名校验没过的房间仍贡献 `all_panels`（面板不该成为别的房间的成员）。
    #[test]
    fn keyword_filter_room_number_and_naming_validation_follow_the_sql_semantics() {
        let groups = vec![
            RoomGroup {
                room: r(5),
                name: "/1RX-RM03-R301".into(),
                panels: vec![r(6), r(8)],
            },
            RoomGroup {
                room: r(15),
                name: "/1RX-STRU-F001".into(),
                panels: vec![r(16)],
            },
            RoomGroup {
                room: r(21),
                name: "/2RX-RM01-LOBBY".into(),
                panels: vec![r(22)],
            },
        ];
        let filtered = room_panel_groups(&groups, &["-RM".to_string()]);
        assert_eq!(
            filtered,
            vec![
                (r(5), "R301".to_string(), vec![r(6), r(8)]),
                (r(21), "LOBBY".to_string(), vec![r(22)]),
            ]
        );
        assert!(room_panel_groups(&groups, &[String::new()]).is_empty());
        assert!(room_panel_groups(&groups, &[]).is_empty());

        let map = room_panel_map_from_groups(filtered, match_room_name_hd);
        assert_eq!(map.rooms.len(), 1, "LOBBY 不满足 ^[A-Z]\\d{{3}}$");
        assert_eq!(map.rooms[0].room_num, "R301");
        assert_eq!(map.rooms[0].panels, vec![r(6), r(8)]);
        assert_eq!(
            map.all_panels,
            [r(6), r(8), r(22)].into_iter().collect::<HashSet<_>>(),
            "命名不合规房间的面板也要进排除集"
        );
    }

    /// 回退即红：房间映射的两个来源入口都必须走文件侧拓扑（先已发布清单、再实时文件），
    /// 不得直接读 noun 表；legacy 的 `load_room_panel_groups` 与 `direct_read_mode` 那道门
    /// （2026-09-08 随 S4 退役）都不许回来。
    #[test]
    fn room_model_routes_both_topology_entries_to_file_topology() {
        let source = include_str!("room_model.rs");
        let load = source
            .split_once("pub async fn load_room_panel_map(")
            .expect("load_room_panel_map")
            .1
            .split_once("pub async fn load_room_panel_map_from_pe(")
            .expect("load_room_panel_map end")
            .0;
        assert!(load.contains("load_room_panel_groups_by_mode("), "{load}");
        assert!(!load.contains("load_room_panel_groups(&"), "{load}");

        let build = source
            .split_once("async fn build_room_panels_relate_common<F>(")
            .expect("build_room_panels_relate_common")
            .1
            .split_once("async fn load_room_panel_groups_by_mode<F>(")
            .expect("build_room_panels_relate_common end")
            .0;
        assert!(build.contains("load_room_panel_groups_by_mode("), "{build}");
        assert!(
            !build.contains("load_room_panel_groups(room_key_word"),
            "{build}"
        );

        let by_mode = source
            .split_once("async fn load_room_panel_groups_by_mode<F>(")
            .expect("load_room_panel_groups_by_mode")
            .1
            .split_once("pub(crate) fn room_panel_map_from_groups<F>(")
            .expect("by_mode end")
            .0;
        assert!(
            by_mode.contains("load_room_panel_map_from_files("),
            "实时文件来源必须存在: {by_mode}"
        );
        // 「先看已发布文件清单」那一档随 publish 阶段退役（ADR-072 D4）：数据面只剩文件侧一条读路。
        for forbidden in [
            concat!("direct_read", "_mode"),
            concat!("load_room_panel", "_groups("),
            "active_data_db()",
            concat!("load_room_panel_map_from_", "published_files("),
            concat!("published_", "file_inputs"),
        ] {
            assert!(
                !by_mode.contains(forbidden),
                "legacy 读路由不许回来（{forbidden}）: {by_mode}"
            );
        }
        assert!(
            !source.contains(concat!("async fn load_room_panel", "_groups<F>(")),
            "legacy 的 load_room_panel_groups 已随 S4 退役，不许回来"
        );
    }

    /// 真文件门：在 ams8000 上从 WORL 根收房间分组——遍历跑得通、每个分组的面板都在文件里
    /// 且 noun 是 PANE、`-RM` 过滤与命名校验不报错。ams8000 是船体样例，房间数可能为 0，
    /// 所以只钉结构不钉数字，把结果打印出来供人看。
    #[test]
    #[ignore = "manual live: needs the real ams8000 DESI file and the E3D template directory"]
    fn live_ams8000_room_groups_are_structurally_sound() {
        use e3d_io::db_element::{DbFilePin, template_file_for};

        use crate::data_interface::direct_store::DirectSchema;
        use crate::fast_model::e3d_model_service::scan_index;

        let file = PathBuf::from(std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(
            |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into(),
        ));
        let schema = DirectSchema::open_from_env().expect("E3D template directory");
        let set = Arc::new(
            DbSet::with_attlib_file(schema.template_dir().join("attlib.dat")).expect("attlib"),
        );
        set.add_db(DbFilePin {
            file: file.clone(),
            template: template_file_for(schema.template_dir(), "DESI").expect("DESI template"),
            db_type: Some("DESI".into()),
            sesno: None,
        })
        .expect("pin DESI");
        let index = scan_index(&file, None).expect("scan index");
        let started = std::time::Instant::now();
        let groups = room_groups_from_set(&set, &index.roots, RoomHierarchy::Hd).expect("groups");
        println!(
            "ams8000: {} WORL root(s) → {} FRMW group(s) with {} panel(s) in {:?}",
            index.roots.len(),
            groups.len(),
            groups.iter().map(|group| group.panels.len()).sum::<usize>(),
            started.elapsed()
        );
        for group in &groups {
            for panel in &group.panels {
                let element = subtree_element_from_set(&set, *panel).expect("panel readable");
                assert_eq!(
                    element.noun.trim().to_ascii_uppercase(),
                    "PANE",
                    "{group:?}"
                );
            }
        }
        let keyword = std::env::var("AIOS_ROOM_KEYWORD").unwrap_or_else(|_| "-RM".into());
        let map = room_panel_map_from_groups(
            room_panel_groups(&groups, &[keyword.clone()]),
            match_room_name_hd,
        );
        println!(
            "keyword {keyword:?}: {} room(s) pass naming, {} panel(s) excluded in total",
            map.rooms.len(),
            map.all_panels.len()
        );
    }

    /// 在进程级 GroupCache 上操练换代的测试**串行跑**：它们互相推代就会把对方的代挤出局。
    fn group_cache_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 分组缓存只留两代（常驻房间模型 P2-b）：新一代进来时最老那一代必须出局。
    ///
    /// 用独一无二的 composite 做键，免得和别的测试在这张进程级表上互相踩；断言只看
    /// 「自己这三代里哪一代还在」，不看表长。**每代的 sesno 各不相同**，所以 Q16 跨代
    /// 提升在本测试里永远不命中（令牌也不同），出局断言不受它影响。
    #[test]
    fn the_group_cache_keeps_only_two_generations() {
        let _serial = group_cache_test_guard();
        let tag = format!("gen-cache-{:?}", std::thread::current().id());
        let key = |era: u32| (77, era, format!("{tag}-{era}"), RoomHierarchy::Hd);
        let token = |era: u32| format!("{tag}-token-{era}");
        let groups = |era: u32| {
            Arc::new(vec![RoomGroup {
                room: r(era),
                name: format!("/1RX-RM03-R30{era}"),
                panels: vec![r(era + 100)],
            }])
        };

        for era in 1..=2 {
            remember_groups(key(era), token(era), groups(era));
        }
        assert_eq!(
            cached_groups(key(1), &token(1)),
            Some(groups(1)),
            "两代之内必须还在"
        );
        assert_eq!(cached_groups(key(2), &token(2)), Some(groups(2)));

        remember_groups(key(3), token(3), groups(3));
        assert_eq!(
            cached_groups(key(1), &token(1)),
            None,
            "第三代进来，第一代必须出局"
        );
        assert_eq!(cached_groups(key(2), &token(2)), Some(groups(2)));
        assert_eq!(cached_groups(key(3), &token(3)), Some(groups(3)));

        // 同一代里再来一个文件是并进去，不算新一代——否则一代多文件的项目会自己把自己挤掉。
        let sibling = (78, 3, format!("{tag}-3"), RoomHierarchy::Hd);
        remember_groups(sibling.clone(), token(4), groups(4));
        assert_eq!(
            cached_groups(key(2), &token(2)),
            Some(groups(2)),
            "并入不该顶掉上一代"
        );
        assert_eq!(cached_groups(key(3), &token(3)), Some(groups(3)));
        assert_eq!(cached_groups(sibling, &token(4)), Some(groups(4)));
    }

    /// Q16「跨代命中即提升」（ADR-067 实施约束 3，T4.0）：composite 换代后，没变的文件
    /// （同 dbnum / sesno / 令牌）必须直接从旧代命中并被提进新代——连续两次换代命中也
    /// 不丢；同号换文件（令牌不同）与层级不同一律不借。
    #[test]
    fn unchanged_files_are_promoted_across_generations_and_survive_two_swaps() {
        let _serial = group_cache_test_guard();
        let tag = format!("promote-{:?}", std::thread::current().id());
        let composite = |era: u32| format!("{tag}-era-{era}");
        // (dbnum=91, sesno=5) 三代之间都没变，令牌也不变。
        let key = |era: u32| (91, 5, composite(era), RoomHierarchy::Hd);
        let token = format!("{tag}-token");
        let groups = Arc::new(vec![RoomGroup {
            room: r(9),
            name: "/1RX-RM03-R309".into(),
            panels: vec![r(109)],
        }]);

        remember_groups(key(1), token.clone(), groups.clone());
        assert_eq!(
            cached_groups(key(2), &token),
            Some(groups.clone()),
            "第一次换代：代键变了、文件没变，必须跨代命中（而不是重解析）"
        );
        // 邻文件并进第 2 代（提升在上一步已把没变文件带进去了）。
        remember_groups(
            (92, 1, composite(2), RoomHierarchy::Hd),
            format!("{tag}-other"),
            groups.clone(),
        );
        assert_eq!(
            cached_groups(key(3), &token),
            Some(groups.clone()),
            "第二次换代：命中必须跟着代走，不得随最老一代出局"
        );
        assert_eq!(
            cached_groups(key(3), &format!("{tag}-swapped")),
            None,
            "同号换文件（令牌不同）不得跨代借用旧产物"
        );
        assert_eq!(
            cached_groups((91, 5, composite(4), RoomHierarchy::Hh), &token),
            None,
            "层级不同分组口径不同，不得跨代借用"
        );
    }

    /// 回退即红：淘汰逻辑长回「从不淘汰」就红；写入路径长出第二条（绕开淘汰）也红。
    #[test]
    fn the_group_cache_has_an_eviction_bound() {
        let source = include_str!("room_topology.rs")
            .replace("\r\n", "\n")
            .split_once("#[cfg(test)]\nmod tests {")
            .expect("测试跟在生产代码后面")
            .0
            .to_string();
        assert!(
            source.contains("const GROUP_CACHE_GENERATIONS: usize = 2;"),
            "分组缓存的代数上限必须是 2（与 ResidentRooms 同口径）"
        );
        let insert = source
            .split_once("fn insert_groups(")
            .expect("insert_groups 必须存在（remember 与 Q16 提升共用的唯一写入口）")
            .1
            .split_once("\n}\n")
            .expect("函数体必须闭合")
            .0;
        assert!(
            insert.contains("while cache.len() > GROUP_CACHE_GENERATIONS")
                && insert.contains("cache.pop_front()"),
            "写入之后必须淘汰超出上限的老代: {insert}"
        );
        for writer in ["fn remember_groups(", "fn cached_groups("] {
            let body = source
                .split_once(writer)
                .expect("写入方必须存在")
                .1
                .split_once("\n}\n")
                .expect("函数体必须闭合")
                .0;
            assert!(
                body.contains("insert_groups(") && !body.contains("push_back"),
                "{writer} 必须经 insert_groups 写入，不得自带第二条写入路径"
            );
        }
    }

    /// 回退即红（审核 2026-09-09）：磁盘工件键只认**本文件**的身份（dbnum / sesno / 本文件
    /// 令牌 / 层级），不许把整组文件的 composite 编回去——编回去就是任一邻库前进把没变
    /// 文件的工件全部作废、整份重解析（ams7997 实测 6–7 s / 文件）。
    #[test]
    fn the_room_groups_artifact_is_addressed_by_the_file_not_the_composite() {
        let source = include_str!("room_topology.rs")
            .replace("\r\n", "\n")
            .split_once("#[cfg(test)]\nmod tests {")
            .expect("测试跟在生产代码后面")
            .0
            .to_string();
        let needle = concat!("room-groups-", "v2:");
        let key_at = source.find(needle).expect("磁盘工件键必须还在（v2）");
        let key_line = source[key_at..]
            .split_once('\n')
            .map(|(line, _)| line)
            .unwrap_or(&source[key_at..]);
        assert!(
            key_line.contains("{file_token}"),
            "工件键必须按本文件令牌寻址: {key_line}"
        );
        assert!(
            !key_line.contains("{composite}"),
            "工件键不许把 composite 编回去: {key_line}"
        );
    }
}
