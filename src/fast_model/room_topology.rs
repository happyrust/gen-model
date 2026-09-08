//! 房间 → 面板拓扑的**文件侧**来源（ADR-010 房间面在 ADR-056 N7 / ADR-053 direct 读模式下的替身）。
//!
//! `room_model::load_room_panel_groups` 从 SurrealDB 的 noun 表（`FRMW` / `SBFR`）+ `pe_owner`
//! 图读「哪些是房间、房间下有哪些面板」，于是 direct 读模式（生产现状：不跑数据解析）与从未
//! 解析过的库上是 **0 间房**——`build_room_relations` 打印「0 间房 / 0 块面板」、盖章成功，
//! 房间子系统整体静默不工作。本模块把同一份口径搬到 e3d-io `DbSet` 上：
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

use aios_core::{RefnoEnum, SUL_DB};
use anyhow::Context;
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;

use crate::data_interface::generation_root::{
    SubtreeElement, refno_from_e3d, subtree_element_from_set,
};
use crate::fast_model::room_model::{RoomPanelMap, room_panel_map_from_groups};

const PUBLISHED_TOPOLOGY_TABLE: &str = "room_published_topology";

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

/// 清单身份：`room_published_topology.manifest_key`。常驻房间模型（`room_resident`）的
/// 摄入侧以它为换代键，所以两边必须是同一个函数算出来的字符串。
pub(crate) fn published_topology_manifest_key(
    inputs: &[crate::data_interface::side_effect_pending::PublishedFileInput],
    hierarchy: RoomHierarchy,
) -> String {
    use sha2::{Digest, Sha256};

    let mut ordered = inputs.to_vec();
    ordered.sort_by_key(|input| input.dbnum);
    let mut digest = Sha256::new();
    digest.update(match hierarchy {
        RoomHierarchy::Hd => b"hd".as_slice(),
        RoomHierarchy::Hh => b"hh".as_slice(),
    });
    for input in ordered {
        digest.update(input.dbnum.to_le_bytes());
        digest.update((input.db_type.len() as u64).to_le_bytes());
        digest.update(input.db_type.as_bytes());
        digest.update(input.end_sesno.to_le_bytes());
        digest.update((input.commit_token.len() as u64).to_le_bytes());
        digest.update(input.commit_token.as_bytes());
    }
    hex::encode(digest.finalize())
}

/// Build the complete topology candidate for one composite file manifest and
/// return replay-safe statements which publish none of it yet. The current
/// `published_file_input` rows choose the visible manifest key.
pub(crate) async fn render_stage_published_room_topology(
    inputs: &[crate::data_interface::side_effect_pending::PublishedFileInput],
) -> anyhow::Result<String> {
    let started = std::time::Instant::now();
    let hierarchy = RoomHierarchy::configured()
        .ok_or_else(|| anyhow::anyhow!("房间子系统需要 project_hd 或 project_hh 特性"))?;
    let manifest_key = published_topology_manifest_key(inputs, hierarchy);
    let mut sources = Vec::new();
    for input in inputs
        .iter()
        .filter(|input| input.db_type.eq_ignore_ascii_case("DESI"))
    {
        let source = super::room_topology_source::load_on(&SUL_DB, &input.commit_token).await?;
        anyhow::ensure!(
            source.dbnum == input.dbnum && source.source_sesno as i32 == input.end_sesno,
            "published topology input row/token mismatch for dbnum {}",
            input.dbnum
        );
        sources.push(source);
    }
    let groups = tokio::task::spawn_blocking(move || {
        super::room_topology_source::compose(&sources, hierarchy)
    })
    .await
    .context("compose published room topology")??;
    let mut statements = vec![format!(
        "DELETE {PUBLISHED_TOPOLOGY_TABLE} WHERE manifest_key = '{manifest_key}';"
    )];
    for (dbnum, groups) in groups {
        let groups = serde_json::to_string(&groups)?;
        statements.push(format!(
            "UPSERT type::thing('{PUBLISHED_TOPOLOGY_TABLE}', '{manifest_key}_{dbnum}') \
             SET manifest_key = '{manifest_key}', dbnum = {dbnum}, groups = {groups}, updated_at = time::now();"
        ));
    }
    println!(
        "[room-topology] composed sources={} elapsed_ms={}",
        inputs.len(),
        started.elapsed().as_millis()
    );
    Ok(statements.join(" "))
}

pub(crate) async fn stage_candidate_room_topology(
    dbnum: u32,
    db_type: &str,
    end_sesno: i32,
    commit_token: &str,
) -> anyhow::Result<()> {
    let mut inputs =
        crate::data_interface::side_effect_pending::SideEffectCompensator::published_file_inputs()
            .await?;
    inputs.retain(|input| input.dbnum != dbnum);
    inputs.push(
        crate::data_interface::side_effect_pending::PublishedFileInput {
            dbnum,
            db_type: db_type.to_string(),
            end_sesno,
            commit_token: commit_token.to_string(),
            published_version: 0,
        },
    );
    ensure_published_room_topology(&inputs).await
}

/// Ensure a complete topology candidate exists for the exact composite. This
/// is called once before promotion and again under the spatial publication
/// lock. The second call closes races with another file publication; after a
/// crash it also reuses the already durable candidate without reopening files.
pub(crate) async fn ensure_published_room_topology(
    inputs: &[crate::data_interface::side_effect_pending::PublishedFileInput],
) -> anyhow::Result<()> {
    let hierarchy = RoomHierarchy::configured()
        .ok_or_else(|| anyhow::anyhow!("房间子系统需要 project_hd 或 project_hh 特性"))?;
    let manifest_key = published_topology_manifest_key(inputs, hierarchy);
    let mut expected_dbnums = inputs
        .iter()
        .filter(|input| input.db_type.eq_ignore_ascii_case("DESI"))
        .map(|input| input.dbnum)
        .collect::<Vec<_>>();
    expected_dbnums.sort_unstable();
    let mut response = SUL_DB
        .query(format!(
            "SELECT VALUE dbnum FROM {PUBLISHED_TOPOLOGY_TABLE} \
             WHERE manifest_key = '{manifest_key}' ORDER BY dbnum ASC;"
        ))
        .await?
        .check()?;
    let actual_dbnums: Vec<u32> = response.take(0)?;
    if actual_dbnums == expected_dbnums {
        return Ok(());
    }
    let statements = render_stage_published_room_topology(inputs).await?;
    SUL_DB
        .query(format!(
            "BEGIN TRANSACTION; {statements} COMMIT TRANSACTION;"
        ))
        .await?
        .check()?;
    Ok(())
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
/// （键 `(dbnum, sesno)`）。此前这张表**从不淘汰**——ams7997 一个文件的分组是 1 077 组 /
/// 3 937 面板，读透侧每 SAVEWORK 一次就多攒一代，进程活得越久留下的死代越多，而除了当前
/// 那一代与用来做 diff 的上一代，谁也不会再看它们一眼。
const GROUP_CACHE_GENERATIONS: usize = 2;

/// 一代：同一组文件版本下，各文件各自的分组。
struct GroupGeneration {
    ident: (String, RoomHierarchy),
    files: HashMap<(u32, u32), Arc<Vec<RoomGroup>>>,
}

type GroupCache = Mutex<VecDeque<GroupGeneration>>;

fn group_cache() -> &'static GroupCache {
    static CACHE: OnceLock<GroupCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn cached_groups(key: (u32, u32, String, RoomHierarchy)) -> Option<Arc<Vec<RoomGroup>>> {
    let (dbnum, sesno, composite, hierarchy) = key;
    group_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|generation| generation.ident == (composite.clone(), hierarchy))
        .and_then(|generation| generation.files.get(&(dbnum, sesno)).cloned())
}

/// 记一条。命中已有的那一代就并进去（不重排——同一代里各文件是一起读出来的，先后无意义）；
/// 是新的一代才排到队尾，队首那一代随之出局。
fn remember_groups(key: (u32, u32, String, RoomHierarchy), groups: Arc<Vec<RoomGroup>>) {
    let (dbnum, sesno, composite, hierarchy) = key;
    let ident = (composite, hierarchy);
    let mut cache = group_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(generation) = cache
        .iter_mut()
        .find(|generation| generation.ident == ident)
    {
        generation.files.insert((dbnum, sesno), groups);
        return;
    }
    cache.push_back(GroupGeneration {
        ident,
        files: HashMap::from([((dbnum, sesno), groups)]),
    });
    while cache.len() > GROUP_CACHE_GENERATIONS {
        cache.pop_front();
    }
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
/// [`load_published_topology_groups_on`]），别的读法照旧只看一整份分组。
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
    let composite =
        crate::data_interface::model_publish_queue::digest(&serde_json::to_vec(&tokens)?);
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
        let (dbnum, file, sesno) = (
            source.dbnum() as u32,
            source.path().to_path_buf(),
            source.target_sesno(),
        );
        let key = (dbnum, sesno, composite.clone(), hierarchy);
        let cached = match cached_groups(key.clone()) {
            Some(cached) => cached,
            None => {
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
                let fresh = Arc::new(fresh);
                remember_groups(key, fresh.clone());
                fresh
            }
        };
        by_file.push((dbnum, cached.as_ref().clone()));
    }
    Ok((composite, tokens, by_file))
}

/// Load room topology from the durable composite of each file's last
/// successful publication. `None` means the upgrade baseline has not been
/// established yet, so callers may retain their legacy startup behavior.
pub(crate) async fn load_room_panel_map_from_published_files<F>(
    room_key_words: &[String],
    hierarchy: RoomHierarchy,
    match_room_fn: F,
) -> anyhow::Result<Option<RoomPanelMap>>
where
    F: Fn(&str) -> bool,
{
    let rows =
        crate::data_interface::side_effect_pending::SideEffectCompensator::published_file_inputs()
            .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let by_file = load_published_topology_groups_on(&SUL_DB, &rows, hierarchy).await?;
    Ok(Some(room_panel_map_from_groups(
        room_panel_groups(&flatten_groups_by_file(by_file), room_key_words),
        match_room_fn,
    )))
}

/// 发布清单选中的那份拓扑，**按文件 dbnum 分好**（`room_published_topology` 一个 dbnum 一行，
/// 行里的分组就是那个文件的房间树贡献的）。
///
/// 保留行 dbnum 是为了「这块面板来自哪个库」有个能说给人听的答案：面板 refno 的头一段是
/// **refno 空间**的库号（ams7997 里的元素是 24381），与 `watch_dbnums` / 发布清单说的**文件**
/// 库号不是同一套编号。
pub(crate) async fn load_published_topology_groups_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    inputs: &[crate::data_interface::side_effect_pending::PublishedFileInput],
    hierarchy: RoomHierarchy,
) -> anyhow::Result<Vec<(u32, Vec<RoomGroup>)>> {
    #[derive(serde::Deserialize)]
    struct TopologyRow {
        dbnum: u32,
        groups: Vec<RoomGroup>,
    }
    let mut expected_dbnums = inputs
        .iter()
        .filter(|row| row.db_type.eq_ignore_ascii_case("DESI"))
        .map(|row| row.dbnum)
        .collect::<Vec<_>>();
    expected_dbnums.sort_unstable();
    let manifest_key = published_topology_manifest_key(inputs, hierarchy);
    let mut response = db
        .query(format!(
            "SELECT dbnum, groups FROM {PUBLISHED_TOPOLOGY_TABLE} \
             WHERE manifest_key = '{manifest_key}' ORDER BY dbnum ASC;"
        ))
        .await?
        .check()?;
    let topology_rows: Vec<TopologyRow> = response.take(0)?;
    let actual_dbnums = topology_rows
        .iter()
        .map(|row| row.dbnum)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        actual_dbnums == expected_dbnums,
        "published room topology {manifest_key} incomplete: expected dbnums {expected_dbnums:?}, got {actual_dbnums:?}"
    );
    Ok(topology_rows
        .into_iter()
        .map(|row| (row.dbnum, row.groups))
        .collect())
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

    fn published_input(
        dbnum: u32,
        end_sesno: i32,
        token: &str,
    ) -> crate::data_interface::side_effect_pending::PublishedFileInput {
        crate::data_interface::side_effect_pending::PublishedFileInput {
            dbnum,
            db_type: "DESI".into(),
            end_sesno,
            commit_token: token.into(),
            published_version: 99,
        }
    }

    #[tokio::test]
    async fn persisted_topology_is_selected_by_the_complete_file_manifest() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("room_topology")
            .use_db("published_manifest")
            .await
            .unwrap();
        let inputs = vec![
            published_input(8001, 7, "token-b"),
            published_input(8000, 5, "token-a"),
        ];
        let reversed = inputs.iter().cloned().rev().collect::<Vec<_>>();
        let key = published_topology_manifest_key(&inputs, RoomHierarchy::Hd);
        assert_eq!(
            key,
            published_topology_manifest_key(&reversed, RoomHierarchy::Hd),
            "清单身份不得依赖查询返回顺序"
        );
        assert_ne!(
            key,
            published_topology_manifest_key(
                &[published_input(8000, 6, "token-new"), inputs[0].clone()],
                RoomHierarchy::Hd,
            ),
            "任一文件版本变化必须选择另一份拓扑"
        );

        let first = vec![RoomGroup {
            room: r(1),
            name: "/AREA-R001".into(),
            panels: vec![r(2)],
        }];
        db.query(format!(
            "CREATE room_published_topology:a SET manifest_key = '{key}', dbnum = 8000, groups = {};",
            serde_json::to_string(&first).unwrap()
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        let error = load_published_topology_groups_on(&db, &inputs, RoomHierarchy::Hd)
            .await
            .expect_err("缺少一个文件分片必须失败关闭");
        assert!(
            error.to_string().contains("expected dbnums [8000, 8001]"),
            "{error:#}"
        );

        let second = vec![RoomGroup {
            room: r(3),
            name: "/AREA-R002".into(),
            panels: vec![r(4)],
        }];
        db.query(format!(
            "CREATE room_published_topology:b SET manifest_key = '{key}', dbnum = 8001, groups = {};",
            serde_json::to_string(&second).unwrap()
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        let by_file = load_published_topology_groups_on(&db, &inputs, RoomHierarchy::Hd)
            .await
            .unwrap();
        assert_eq!(
            by_file,
            vec![(8000, first.clone()), (8001, second.clone())],
            "行 dbnum 是**文件**库号，要留着——面板 refno 的头一段是 refno 空间的库号，两套编号"
        );
        assert_eq!(flatten_groups_by_file(by_file), [first, second].concat());
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

    /// 回退即红：房间映射的两个来源入口都必须按读模式路由，不得直接读 noun 表。
    #[test]
    fn room_model_routes_both_topology_entries_by_read_mode() {
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
            .split_once("async fn load_room_panel_groups<F>(")
            .expect("by_mode end")
            .0;
        let published = by_mode
            .find("load_room_panel_map_from_published_files(")
            .expect("已发布文件清单必须优先于实时来源");
        let direct = by_mode.find("direct_read_mode()").expect("direct fallback");
        assert!(published < direct, "发布清单必须先于实时来源: {by_mode}");
        assert!(by_mode.contains("direct_read_mode()"), "{by_mode}");
        assert!(
            by_mode.contains("load_room_panel_map_from_files("),
            "{by_mode}"
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

    /// 分组缓存只留两代（常驻房间模型 P2-b）：新一代进来时最老那一代必须出局。
    ///
    /// 用独一无二的 composite 做键，免得和别的测试在这张进程级表上互相踩；断言只看
    /// 「自己这三代里哪一代还在」，不看表长——并行跑的邻居也在往同一张表里写。
    #[test]
    fn the_group_cache_keeps_only_two_generations() {
        let tag = format!("gen-cache-{:?}", std::thread::current().id());
        let key = |era: u32| (77, era, format!("{tag}-{era}"), RoomHierarchy::Hd);
        let groups = |era: u32| {
            Arc::new(vec![RoomGroup {
                room: r(era),
                name: format!("/1RX-RM03-R30{era}"),
                panels: vec![r(era + 100)],
            }])
        };

        for era in 1..=2 {
            remember_groups(key(era), groups(era));
        }
        assert_eq!(cached_groups(key(1)), Some(groups(1)), "两代之内必须还在");
        assert_eq!(cached_groups(key(2)), Some(groups(2)));

        remember_groups(key(3), groups(3));
        assert_eq!(cached_groups(key(1)), None, "第三代进来，第一代必须出局");
        assert_eq!(cached_groups(key(2)), Some(groups(2)));
        assert_eq!(cached_groups(key(3)), Some(groups(3)));

        // 同一代里再来一个文件是并进去，不算新一代——否则一代多文件的项目会自己把自己挤掉。
        let sibling = (78, 3, format!("{tag}-3"), RoomHierarchy::Hd);
        remember_groups(sibling.clone(), groups(4));
        assert_eq!(cached_groups(key(2)), Some(groups(2)), "并入不该顶掉上一代");
        assert_eq!(cached_groups(key(3)), Some(groups(3)));
        assert_eq!(cached_groups(sibling), Some(groups(4)));
    }

    /// 回退即红：淘汰逻辑长回「从不淘汰」就红。
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
        let remember = source
            .split_once("fn remember_groups(")
            .expect("remember_groups 必须存在")
            .1
            .split_once("\n}\n")
            .expect("函数体必须闭合")
            .0;
        assert!(
            remember.contains("while cache.len() > GROUP_CACHE_GENERATIONS")
                && remember.contains("cache.pop_front()"),
            "记一代之后必须淘汰超出上限的老代: {remember}"
        );
    }
}
