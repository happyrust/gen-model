//! 生产环境唯一的 `e3d-model` 生成与增量入口。
//!
//! 生成出来的几何只落进程内投影存储（[`crate::fast_model::model_memory_store`]）。
//! **本模块不认识数据库**：没有 SurrealDB 句柄，不拼 SQL，不开发布事务。把投影搬进库里
//! 是另一层适配器的事，它从存储里读结构化记录，不必回头再算一遍几何。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{RefU64, RefnoEnum};
use anyhow::{Context, bail};
use e3d_io::db_element::{DbFilePin, DbFileResolver, DbSet, template_file_for};
use e3d_io::engine::{ReadOnlyEngine, ScanTier};
use e3d_io::refno::RefNo;
use e3d_io::session::DbView;
use e3d_model::catalogue::CatalogueMeshCache;
use e3d_model::elmodl::{GeneratedElement, GeometryId};
use e3d_model::increment::{
    IncrementReport, PlanLimits, UpdatePlan, collect_window, execute_plan, increment_update,
    plan_update, plan_update_with,
};
use e3d_model::ledger::{ChangeLedger, Disposition};
use e3d_model::pipeline::{
    GenerationHooks, GenerationStopped, NoHooks, Report, Stop, generate_subtree_with_cache,
    generate_subtree_with_hooks,
};
use e3d_model::primitive_instance::canonical_primitive_mesh;
use e3d_model::transform::dmat4_to_affine4x3;
use glam::DMat4;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::data_interface::cata_closure::{CataDbLocator, InMemoryCataLocator};
use crate::data_interface::direct_store::{DbPin, DirectSchema};
use crate::data_interface::geom_error::GeometryFailurePolicy;
use crate::data_interface::mdb_membership;
use crate::data_interface::model_source;
use crate::data_interface::rigid_motion::{
    RepublishGate, TransformAnchor, bounds_of_points, republish_gate, republish_records,
};
use crate::fast_model::historical_input::HistoricalInputLru;
use crate::fast_model::model_memory_store::{
    CommitScope, ModelMemoryStore, ProjectedGeometry, ProjectedShape, ProjectedTube,
    ProjectionCommit, ProjectionScope, RootReceipt, packed_refno,
};

#[cfg(test)]
#[path = "orientation_increment_regression_tests.rs"]
mod orientation_increment_regression_tests;

#[cfg(test)]
#[path = "orientation_source_diff_fixture.rs"]
mod orientation_source_diff_fixture;

/// 刚体前移执行体（ADR-066 B6）一轮的结果：哪些根走成便宜路（`completed`），哪些是「回执已在 T」
/// 的窗口重放（`replayed`——投影一字未动、按已完成计，见 [`republish_gate`]），哪些回退整根重算
/// （`fallbacks`，带原因）。回退永远正确、只是不省。用 pdms `a/b` 串标识根，方便与 `UnitTask` 对齐。
#[derive(Debug, Default)]
pub struct RigidMotionReport {
    pub completed: Vec<String>,
    pub replayed: Vec<String>,
    pub fallbacks: Vec<(String, String)>,
}

/// 刚体前移执行体一次吃的一根。`source_start_sesno` 是规划器算 `Δ` 的窗口基线 S（随
/// [`TransformUnit`](crate::data_interface::rigid_motion::TransformUnit) 的线格式带过来）：
/// `Δ = W_T·W_S⁻¹` 只对「投影还停在 S」的记录成立，重放闸拿它对回执，对不上就回退 regen。
#[derive(Debug, Clone)]
pub struct RigidMotionUnit {
    pub root_refno: String,
    pub source_start_sesno: u32,
    pub anchors: Vec<TransformAnchor>,
}

/// 单元级发布执行体（ADR-080 D4）一轮的结果，与 [`RigidMotionReport`] 同形：`completed` 走成了局部提交，
/// `replayed` 是「回执已在 T」的窗口重放（投影一字未动、按已完成计），`fallbacks` 回退整根重算（带原因；
/// D8：任何一件不成都整根回退，回退永远正确、只是不省）。`receipt_only` 是 `completed` 的**子集**——
/// 局部清单为空、只把回执推到 T 的那几根（ADR-080 D10：只被 `no_model` 候选碰到的根，一件几何都没重建），
/// 只作观测。
#[derive(Debug, Default)]
pub struct UnitPublishReport {
    pub completed: Vec<String>,
    pub replayed: Vec<String>,
    pub fallbacks: Vec<(String, String)>,
    pub receipt_only: Vec<String>,
}

/// 单元级发布执行体一次吃的一根。`source_start_sesno` 是 `plan_update(S→T)` 的窗口基线 S：局部清单只对
/// 「投影还停在 S」的根成立——回执已在 T 是重放，既不在 S 也不在 T 是半截状态或别的路径动过它，两者
/// 都不许把局部清单叠上去（[`republish_gate`]，ADR-080 D4）。
///
/// `no_model_only`（ADR-080 D10）：规划器判这根这一窗**只被账本去向 `no_model` 的候选碰到**——闭包直接命中、
/// 不是级联（C2 / C8 / P4）扩进来的、S 端已发布且安定、归到它名下的每个候选都是 `no_model`。只有带着它，
/// 生产方切出来的**空**子计划才许当「这根名下没有一件几何要变」发一份空的 `Partial`（只推回执）；不带它的
/// 空子计划照旧回退整根 regen——选根看得比 `plan_update` 远，空清单不是「没变」。
#[derive(Debug, Clone)]
pub struct UnitPublishUnit {
    pub root_refno: String,
    pub source_start_sesno: u32,
    pub no_model_only: bool,
}

/// 单元级发布一根的**局部**产出（ADR-080 D2 的 `Partial`）：只点名这几件，缺席什么也不证明，要删的
/// 必须写进 `removals`。`scope` 与回执由执行体盖——生产方给不出整根的章（D5：存储在写锁里算），
/// 也不该替执行体决定这一笔是不是局部。
#[derive(Debug, Default)]
pub struct UnitDelta {
    pub tube_branch_resets: Vec<RefNo>,
    pub removals: Vec<String>,
    pub upserts: Vec<ProjectedGeometry>,
    /// 本轮重算出来的不可变目标（D5：不沿用旧回执的）。
    pub model_target: Option<ModelTarget>,
    /// 本次触到的每条路由容器（BRAN）在 T 端**应有**的管段数——生产方按 `route_tubes_of` 另记一账，
    /// 不从 `upserts` 里数回来。管段完整性门（D6 / 实施约束 4，[`tube_segment_completeness`]）拿它对
    /// `upserts` 里真带上的段数：`tube_branch_resets` 是整段重写，漏一段就是静默丢管。
    /// 键是 `(word0, word1)`——`RefNo` 不实现 `Ord`，与 `owners` 表同一约定。
    pub tube_segments: BTreeMap<(u32, u32), usize>,
}

impl UnitDelta {
    /// 一件几何都不动：不插、不删、不整段重写。这样的产出只有一个合法来源——生产方对一根**只被
    /// `no_model` 候选碰到**的根切出空子计划（ADR-080 D10）；发出去就是「只推回执」。
    pub fn is_empty(&self) -> bool {
        self.upserts.is_empty() && self.removals.is_empty() && self.tube_branch_resets.is_empty()
    }
}

/// 管段完整性门（ADR-080 D6 / 实施约束 4）：一次 `Partial` 若触到某条 BRAN 的管身，它的管身按**整段**处理——
/// 存储那边 `tube_branch_resets` 一碰就清光那条分支的全部旧段，`upserts` 里没带上的段不会有人替它留着，
/// 而且两边都不报错（`a_partial_commit_that_resets_a_branch_still_clears_every_segment_of_it` 演的就是这个）。
///
/// 所以在产出与提交之间核三件事，任一不成就整根回退 regen（D8），**不发**：
/// 1. `upserts` 里带了 C 的管段，C 就必须在 `tube_branch_resets` 里——不重置，旧段会留在新段旁边成鬼影；
/// 2. 触到的每条 C（重置的 ∪ 带了段的）都要在 `tube_segments` 里有账——没账就没法核；
/// 3. `upserts` 里 C 的段数 == 账上的段数——少了是漏段，多了是账错，都不许发。
///
/// 只数管段数，不核 `route_ordinal` 连不连续：退化槽位本就不出几何（`execute_plan` 的 `kept` 只收真建成的），
/// 序号有洞是正常形状。
pub(crate) fn tube_segment_completeness(delta: &UnitDelta) -> Result<(), String> {
    let key = |refno: RefNo| (refno.word0, refno.word1);
    let mut carried = BTreeMap::<(u32, u32), usize>::new();
    for record in &delta.upserts {
        if let GeometryId::ImpliedTube {
            container_refno, ..
        } = &record.geometry_id
        {
            let container = parse_refno(container_refno).map_err(|error| {
                format!("unparsable tube container {container_refno}: {error:#}")
            })?;
            *carried.entry(key(container)).or_default() += 1;
        }
    }
    let resets = delta
        .tube_branch_resets
        .iter()
        .map(|branch| key(*branch))
        .collect::<BTreeSet<_>>();
    let touched = carried
        .keys()
        .copied()
        .chain(resets.iter().copied())
        .collect::<BTreeSet<_>>();
    for container in touched {
        let name = RefNo::new(container.0, container.1);
        let carried = carried.get(&container).copied().unwrap_or(0);
        if !resets.contains(&container) {
            return Err(format!(
                "BRAN {name}: {carried} tube segment(s) upserted without resetting the branch; \
                 stale segments would survive next to the new ones"
            ));
        }
        let Some(expected) = delta.tube_segments.get(&container).copied() else {
            return Err(format!(
                "BRAN {name}: branch reset with {carried} tube segment(s) but no segment count \
                 declared for it; cannot prove the partial manifest carries every segment"
            ));
        };
        if carried != expected {
            return Err(format!(
                "BRAN {name}: partial manifest carries {carried} tube segment(s), plan expects \
                 {expected}; a reset would silently drop the rest"
            ));
        }
    }
    Ok(())
}

/// 规划期已打开的两端 `DbSet` / `SourceIndex`（Issue #50 / ADR-080 A7）：`UnitDeltaProducer::open`
/// 直接复用，避免同库 S / T 在单库槽位的 `GENERATION_INPUT_CACHE` 里互相逐出并重复扫索引、开 `DbSet`。
/// "这份输入是哪个文件的"不在这里记：分域由 [`WindowPlanKey`] 一处负责，载荷与键在
/// [`window_plan_sets`] 里同时构造，两处不会漂。
#[derive(Clone)]
pub(crate) struct CachedWindowInputs {
    pub(crate) base_set: Arc<DbSet>,
    pub(crate) base_index: Arc<SourceIndex>,
    pub(crate) target_set: Arc<DbSet>,
    pub(crate) target_index: Arc<SourceIndex>,
    /// 打开这两端集合的 service 的依赖会话表（S 端、T 端）。集合懒开目录库时按这张表钉会话、往这张表里记，
    /// `ModelTarget` 也只能从它算——换一张表配，回执的目录依赖就不是几何实际读的那一份。规划器的输入先以
    /// `None` 入缓存，由 [`E3dModelService::bind_window_input_dependencies`] 补上；没补上的，执行体只复用
    /// 计划、不复用集合。
    dependency_sessions: Option<(DependencySessions, DependencySessions)>,
}

impl CachedWindowInputs {
    pub(crate) fn planned(
        base_set: Arc<DbSet>,
        base_index: Arc<SourceIndex>,
        target_set: Arc<DbSet>,
        target_index: Arc<SourceIndex>,
    ) -> Self {
        Self {
            base_set,
            base_index,
            target_set,
            target_index,
            dependency_sessions: None,
        }
    }

    fn from_generation_inputs(base: &GenerationInput, target: &GenerationInput) -> Self {
        Self {
            base_set: base.set.clone(),
            base_index: base.index.clone(),
            target_set: target.set.clone(),
            target_index: target.index.clone(),
            dependency_sessions: Some((
                base.dependency_sessions.clone(),
                target.dependency_sessions.clone(),
            )),
        }
    }

    fn generation_inputs(&self) -> Option<(GenerationInput, GenerationInput)> {
        let (base_sessions, target_sessions) = self.dependency_sessions.as_ref()?;
        Some((
            GenerationInput {
                set: self.base_set.clone(),
                index: self.base_index.clone(),
                dependency_sessions: base_sessions.clone(),
            },
            GenerationInput {
                set: self.target_set.clone(),
                index: self.target_index.clone(),
                dependency_sessions: target_sessions.clone(),
            },
        ))
    }
}

/// 一窗 `plan_update(S→T)` 的四个动作集，从 [`UpdatePlan`] 里拆出来按根切分（ADR-080 D1：单元级发布
/// **直接吃 `plan_update` 的产出**，不引入第二套变更检测）。`UpdatePlan` 会被 `execute_plan` 整个吃掉、
/// 又不 `Clone`，所以一窗算一次放在这里，逐根切出子计划再执行。
///
/// `Clone` 是为 Issue #50 的窗口计划缓存：规划器（`WindowRootSources::collect`）与单元级发布执行体
/// （`UnitDeltaProducer::open`）对同一窗各算一次 `plan_update`，300.5s 重复开销。缓存用 `Arc` 共享，
/// `Clone` 只在缓存未命中时兜底构造用。
#[derive(Clone)]
pub(crate) struct WindowPlanSets {
    /// 这份计划属于哪个库、哪份文件、哪一窗。缓存键由它派生，插入不再另收一个键。
    key: WindowPlanKey,
    pub(crate) regenerate: BTreeSet<(u32, u32)>,
    pub(crate) remove: BTreeSet<(u32, u32)>,
    pub(crate) regenerate_derived: BTreeSet<(u32, u32)>,
    pub(crate) derived_stale: BTreeSet<GeometryId>,
    /// 规划器退化成整库重建的原因（ADR-080 Q4）：非空时四个集合全空，一根都不许走局部——
    /// 空产出不是「没变」。
    pub(crate) full_rebuild: Option<String>,
    /// 计划期就读挂的候选 / 容器数（`unresolved` + `derived_stale_unreadable`）：非零时这一窗的
    /// 分类不可信，一根都不许走局部（D8）。
    pub(crate) unreadable: usize,
    /// 账本上每个候选的去向（`ChangeLedger::dispositions`，`(word0, word1)` 键）。D10 的生产方守卫拿它
    /// 复核「这根名下的候选全是 `no_model`」——规划器的提示与生产方的计划是两次独立算的（实施约束 12），
    /// 空子计划能不能当「没变」发，两边都得点头。
    pub(crate) dispositions: Vec<((u32, u32), Disposition)>,
    /// 规划期已打开的两端 `DbSet` 与 `SourceIndex`（Issue #50 / ADR-080 A7）：`UnitDeltaProducer::open`
    /// 直接复用，避免同库 S / T 在单库槽位的 `GENERATION_INPUT_CACHE` 里互相逐出并重复扫索引、开 `DbSet`。
    pub(crate) inputs: Option<CachedWindowInputs>,
}

/// 窗口计划在缓存里的分域键：**dbnum 不够**。同号跨项目副本（MDB 路由目录允许）可以有各自的
/// 文件、甚至相同的会话号；只按 `(dbnum, S, T)` 会串项目，只靠复核载荷里的文件又会让两个项目
/// 互相把对方的计划顶掉（谁最后规划谁留下）。文件身份是键的一部分，两个项目各留一份。
///
/// 字段私有、只能经 `WindowPlanKey::new` 构造：四个分量就是分域规则本身，不接受调用方自己拼
/// 元组。`WindowPlanSets` 自带这个键，插入时由载荷派生键（`WindowPlanCache::insert`）——
/// 不会出现「键里一个文件、载荷里另一个文件」。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct WindowPlanKey {
    dbnum: u32,
    file: PathBuf,
    base_sesno: u32,
    target_sesno: u32,
}

impl WindowPlanKey {
    pub(crate) fn new(dbnum: u32, file: &Path, base_sesno: u32, target_sesno: u32) -> Self {
        Self {
            dbnum,
            file: file.to_path_buf(),
            base_sesno,
            target_sesno,
        }
    }

    /// 同一个库、同一个文件上的另一窗：淘汰规则按这一条替换旧窗。
    fn same_file_other_window(&self, other: &Self) -> bool {
        self.dbnum == other.dbnum
            && self.file == other.file
            && (self.base_sesno, self.target_sesno) != (other.base_sesno, other.target_sesno)
    }

    fn file(&self) -> &Path {
        &self.file
    }
}

/// Issue #50：窗口计划缓存本体。容量与淘汰规则只在这里写一次：
/// 1. 同库同文件新窗替换该文件的旧窗——旧 `DbSet` / `SourceIndex` 句柄不随新窗继续滞留；
///    **别的项目 / 别的文件不替它背锅**（键里有文件身份）；
/// 2. 总条目超过 `WindowPlanCache::CAPACITY` 时淘汰最久没用的一格（插入与命中都算一次使用）——会话号是
///    各文件自己的计数，跨库比大小挑不出谁旧，按它淘汰会把会话少的库刚规划完的窗先挤掉；
/// 3. 同键重插直接替换。
/// 键由载荷自带的 `WindowPlanSets::key` 派生，插入不接受第二个键；被移出的 `Arc` 返回给调用方，
/// 由它在锁外释放。
struct WindowPlanCache {
    entries: HashMap<WindowPlanKey, CachedWindowPlan>,
    clock: u64,
}

struct CachedWindowPlan {
    sets: Arc<WindowPlanSets>,
    last_used: u64,
}

impl WindowPlanCache {
    /// 长生命周期进程最多留几窗计划：多一窗就多一副 `DbSet` / `SourceIndex` 句柄。
    const CAPACITY: usize = 8;

    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn get(&mut self, key: &WindowPlanKey) -> Option<Arc<WindowPlanSets>> {
        let now = self.tick();
        let cached = self.entries.get_mut(key)?;
        cached.last_used = now;
        Some(cached.sets.clone())
    }

    fn insert(&mut self, sets: Arc<WindowPlanSets>) -> Vec<Arc<WindowPlanSets>> {
        let key = sets.key.clone();
        let mut evicted = self
            .entries
            .keys()
            .filter(|cached| cached.same_file_other_window(&key))
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|stale| self.entries.remove(&stale))
            .map(|stale| stale.sets)
            .collect::<Vec<_>>();
        while !self.entries.contains_key(&key) && self.entries.len() >= Self::CAPACITY {
            let Some(least_recent) = self
                .entries
                .iter()
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(cached, _)| cached.clone())
            else {
                break;
            };
            if let Some(old) = self.entries.remove(&least_recent) {
                evicted.push(old.sets);
            }
        }
        let last_used = self.tick();
        if let Some(old) = self
            .entries
            .insert(key, CachedWindowPlan { sets, last_used })
        {
            evicted.push(old.sets);
        }
        evicted
    }

    /// 给 `key` 那一格的输入补上依赖会话表，只认 `base` / `target` 这两个集合本身：那一格若已被另一次
    /// 规划换成别的集合，就不动它。返回被换下的旧载荷，由调用方在锁外释放。
    fn bind_dependencies(
        &mut self,
        key: &WindowPlanKey,
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
        sessions: &DependencySessions,
    ) -> Option<Arc<WindowPlanSets>> {
        let cached = self.entries.get_mut(key)?;
        let inputs = cached.sets.inputs.as_ref()?;
        if !Arc::ptr_eq(&inputs.base_set, base) || !Arc::ptr_eq(&inputs.target_set, target) {
            return None;
        }
        let mut bound = WindowPlanSets::clone(&cached.sets);
        if let Some(inputs) = bound.inputs.as_mut() {
            inputs.dependency_sessions = Some((sessions.clone(), sessions.clone()));
        }
        Some(std::mem::replace(&mut cached.sets, Arc::new(bound)))
    }
}

/// Issue #50：窗口计划缓存。规划器（`WindowRootSources::collect`）算出的 `plan_update(S→T)`
/// 与两端输入按 `(dbnum, file, base_sesno, target_sesno)` 存一份 `Arc`，单元级发布执行体
/// （`UnitDeltaProducer::open`）同一窗直接拿，省掉重复 `scan_index` + `build_set` +
/// `collect_window` + `plan_update`。
static WINDOW_PLAN_CACHE: OnceLock<std::sync::Mutex<WindowPlanCache>> = OnceLock::new();

fn window_plan_cache() -> &'static std::sync::Mutex<WindowPlanCache> {
    WINDOW_PLAN_CACHE.get_or_init(|| std::sync::Mutex::new(WindowPlanCache::new()))
}

/// 一窗计划折成缓存的载荷：分域键在这里与 `plan_update` 的产出绑定，规划器与执行体兜底路径
/// 都从这里构造，字段口径只写一次。
pub(crate) fn window_plan_sets(
    dbnum: u32,
    file: &Path,
    base_sesno: u32,
    target_sesno: u32,
    plan: &UpdatePlan,
    inputs: Option<CachedWindowInputs>,
) -> Arc<WindowPlanSets> {
    Arc::new(WindowPlanSets {
        key: WindowPlanKey::new(dbnum, file, base_sesno, target_sesno),
        regenerate: plan.regenerate.clone(),
        remove: plan.remove.clone(),
        regenerate_derived: plan.regenerate_derived.clone(),
        derived_stale: plan.derived_stale.clone(),
        full_rebuild: plan.full_rebuild.clone(),
        unreadable: plan.report.unresolved.len() + plan.report.derived_stale_unreadable.len(),
        dispositions: plan.ledger.dispositions().to_vec(),
        inputs,
    })
}

/// 把一份载荷按它自带的键存入缓存；淘汰规则由 `WindowPlanCache` 一处决定。
pub(crate) fn insert_window_plan_sets(sets: Arc<WindowPlanSets>) {
    let evicted = {
        let mut cache = window_plan_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.insert(sets)
    };
    drop(evicted);
}

/// 从窗口计划缓存取一份 `Arc`。命中返回 `Some`，未命中返回 `None`（调用方自行计算并回填）。
pub(crate) fn cached_window_plan_sets(
    dbnum: u32,
    file: &Path,
    base_sesno: u32,
    target_sesno: u32,
) -> Option<Arc<WindowPlanSets>> {
    let key = WindowPlanKey::new(dbnum, file, base_sesno, target_sesno);
    window_plan_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
}

impl E3dModelService {
    /// 把本 service 的依赖会话表绑到缓存里同一窗的两端输入上。规划器用本 service `build_set` 打开
    /// `base` / `target`、经 `WindowRootSources::collect` 入缓存之后调：执行体是另一个 service，命中时
    /// 必须配打开这两个集合的这张表，而不是它自己的。
    pub(crate) fn bind_window_input_dependencies(
        &self,
        dbnum: u32,
        file: &Path,
        base_sesno: u32,
        target_sesno: u32,
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
    ) {
        let key = WindowPlanKey::new(dbnum, file, base_sesno, target_sesno);
        let replaced = window_plan_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .bind_dependencies(&key, base, target, &self.dependency_sessions);
        drop(replaced);
    }
}

impl WindowPlanSets {
    /// 这一窗的计划能不能给局部发布切子计划（ADR-080 D8 ① ②）：退化成整库重建的计划四个集合全空，
    /// 切出来的子计划执行后是**空产出**，把它当「没变」盖一个 T 章就是静默缺件（Q4）；计划期有读挂的
    /// 分类不可信，整窗一根都不许走局部。
    fn admits_partial_publish(&self) -> anyhow::Result<()> {
        if let Some(reason) = &self.full_rebuild {
            bail!(
                "window plan degraded to a full rebuild; a partial manifest would publish an empty \
                 delta as \"unchanged\": {reason}"
            );
        }
        anyhow::ensure!(
            self.unreadable == 0,
            "window plan has {} unreadable candidate(s) / container(s); its classification cannot \
             back a partial manifest",
            self.unreadable
        );
        Ok(())
    }
}

/// D10 的生产方守卫：这根名下（两端属主链任一端经过它，或候选就是它）的账本候选**全部**去向
/// `no_model`，且至少有一个。成立时「子计划为空」才读得成「这一窗 DESI 侧没有一件几何要变」——
/// 去向 `no_model` 的候选是两票元数据门放行的（字典 DCHC=0 ∨ 是戳，∧ 生成器不读），按构造不改几何。
///
/// 两种不成立各是一种「空清单不是没变」：名下一个候选都没有 = 这根不是被账本碰到的（级联 / 恢复 /
/// 目录名单——选根看得比 `plan_update` 远）；名下有 `rolled_up` / `unresolved` 的候选却切出空子计划 =
/// 它折进的单元不在这根之下（嵌套根）或分类不可信，都不许拿空清单盖 T 章。
///
/// **不看候选在根之上的方向**（与刚体路的归因差这一条，故意的）：祖先闭包按构造不因根之上的种子把根
/// 收进来；根之上的改动若真波及根下几何（挪 ZONE / 换类型 / 删除），e3d-model 的级联会把根下的单元放进
/// 动作集，子计划就不空——空子计划这一支根本走不到。反过来把 `WORL` 的 `UPDT` 一类每窗必跳的祖先候选算
/// 进来，这条路在真库上一次都打不着。
pub(crate) fn root_touched_only_by_no_model_candidates(
    dispositions: &[((u32, u32), Disposition)],
    root: RefNo,
    owners_s: &BTreeMap<(u32, u32), RefNo>,
    owners_t: &BTreeMap<(u32, u32), RefNo>,
) -> anyhow::Result<()> {
    let packed_root = packed_refno(root);
    // 链含自身：候选就是根也算「名下」；两端表里都没有的候选链上只有它自己，除了它是根之外归不到谁。
    let under = |unit: (u32, u32), owners: &BTreeMap<(u32, u32), RefNo>| {
        ancestor_chain(RefNo::new(unit.0, unit.1), owners).map(|chain| chain.contains(&packed_root))
    };
    let mut attributed = 0usize;
    for (key, disposition) in dispositions {
        if !(under(*key, owners_t)? || under(*key, owners_s)?) {
            continue;
        }
        attributed += 1;
        anyhow::ensure!(
            *disposition == Disposition::NoModel,
            "candidate {}/{} under root {root} is {disposition:?}, not no_model; an empty partial \
             manifest cannot stand for its change",
            key.0,
            key.1
        );
    }
    anyhow::ensure!(
        attributed > 0,
        "no ledger candidate is attributed to root {root}; it was selected for a reason a partial \
         DESI delta cannot see (cascade / recovery)"
    );
    Ok(())
}

/// 一根子计划里 `execute_plan` 的 `removals` **唯一**许来自的三处（ADR-080 D6 第一条）：
/// `remove` 的单元（窗里删掉的 + 迁出这根的）、`regenerate` 里画不出 / 画失败而被擦掉的单元、
/// 派生容器 base 端的旧管段键（要重算的 + 迁出这根而退休的容器，见 [`RootSubplan`]）。
struct RootPlanSets {
    regenerate: BTreeSet<(u32, u32)>,
    remove: BTreeSet<(u32, u32)>,
    derived_stale: BTreeSet<GeometryId>,
    /// 迁出这根的路由容器：生产方替它在管段账上记 0，管段门按整段清空放行。
    retired_containers: BTreeSet<(u32, u32)>,
}

/// D6 第一条的守卫：产出里每一条 removal 都要能回溯到子计划。回溯不到就是 `execute_plan` 在别处
/// 又推出了一条删除——局部提交下那就是静默丢件，整根回退（D8）。
fn removals_trace_back_to_the_plan(
    sets: &RootPlanSets,
    removals: &[GeometryId],
) -> anyhow::Result<()> {
    for removal in removals {
        let traced = match removal {
            GeometryId::Element { refno } => {
                let unit = parse_refno(refno)?;
                let unit = (unit.word0, unit.word1);
                sets.remove.contains(&unit) || sets.regenerate.contains(&unit)
            }
            GeometryId::ImpliedTube { .. } => sets.derived_stale.contains(removal),
        };
        anyhow::ensure!(
            traced,
            "removal {removal:?} does not trace back to the root's plan (neither UpdatePlan.remove / \
             regenerate nor derived_stale); refusing to publish it as a partial manifest"
        );
    }
    Ok(())
}

/// 不可变目标在**设计窗之外**的那几格有没有变：项目、设计文件身份、目录依赖（含会话）、模板、
/// 生成器指纹、细分档。变了就不是一窗 DESI 差分能覆盖的——core 对目录 / 几何集变化走的是 `REPALL`，
/// 我方对应整根重算；只有 `design.session` / `design.digest` 前移是局部发布该看到的那一格。
fn same_target_outside_the_design_window(published: &ModelTarget, current: &ModelTarget) -> bool {
    published.project == current.project
        && published.design.dbnum == current.design.dbnum
        && published.design.db_type == current.design.db_type
        && same_file_path(&published.design.file, &current.design.file)
        && published.catalogue.len() == current.catalogue.len()
        && published
            .catalogue
            .iter()
            .zip(current.catalogue.iter())
            .all(|(old, new)| same_file_target(old, new))
        && published.template_attlib_digest == current.template_attlib_digest
        && published.generator_fingerprint == current.generator_fingerprint
        && published.tessellation_profile == current.tessellation_profile
}

/// 模型目标里记文件用的路径写法：去掉 Windows `canonicalize()` 给的长路径前缀（`\\?\D:\…` → `D:\…`，
/// `\\?\UNC\服务器\共享` → `\\服务器\共享`）。同一个文件在冻结源服务（`from_frozen_source`，钉的是 canonicalize
/// 过的路径）和普通 MDB 服务（项目清单里的原样路径）里会以两种写法出现，按字串比就「变了」——夹具 8023
/// 2026-10-24 `db-20261024-200916-000003`：前一窗 regen 的回执记 `\\?\D:\…\ams8000_0001`，刚体前移算的目标是
/// `D:\…\ams8000_0001`，「immutable target changed」把刚体路永远拒在外面（d-149 2026-09-21 同症，ADR-082 追记「原因待查」）。
/// 摘要不受影响：身份摘要里没有路径（`frozen_source::model_file_identity`，ADR-089 D2）。
pub(crate) fn plain_file_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text.into_owned()
    }
}

fn same_file_path(a: &str, b: &str) -> bool {
    a == b || plain_file_path(Path::new(a)) == plain_file_path(Path::new(b))
}

/// 目录 / 字典项逐格相等，路径按 [`plain_file_path`] 比。
fn same_file_target(a: &ModelFileTarget, b: &ModelFileTarget) -> bool {
    a.dbnum == b.dbnum
        && a.db_type == b.db_type
        && a.session == b.session
        && a.digest == b.digest
        && same_file_path(&a.file, &b.file)
}

/// 两份不可变目标在设计窗之外差在哪几格，一格一句，给回退理由用。目录项按 dbnum 对齐，说清是会话前移了还是
/// 只有摘要变了——后者在真机上曾是文件身份按 mtime 记、E3D 每开一次库就漂一次（`catalogue_file_identity_digest`）。
/// 回执里只有「changed」一个词时，谁也看不出是目录真换版了还是判据在漂。
fn describe_target_difference(published: &ModelTarget, current: &ModelTarget) -> String {
    fn prefix(digest: &str) -> &str {
        &digest[..digest.len().min(8)]
    }
    let mut parts = Vec::new();
    if published.project != current.project {
        parts.push(format!(
            "project {} -> {}",
            published.project, current.project
        ));
    }
    if published.design.dbnum != current.design.dbnum
        || published.design.db_type != current.design.db_type
        || !same_file_path(&published.design.file, &current.design.file)
    {
        parts.push(format!(
            "design file {} {} {} -> {} {} {}",
            published.design.db_type,
            published.design.dbnum,
            published.design.file,
            current.design.db_type,
            current.design.dbnum,
            current.design.file
        ));
    }
    let catalogue_same = published.catalogue.len() == current.catalogue.len()
        && published
            .catalogue
            .iter()
            .zip(current.catalogue.iter())
            .all(|(old, new)| same_file_target(old, new));
    if !catalogue_same {
        let mut by_dbnum: BTreeMap<u32, (Option<&ModelFileTarget>, Option<&ModelFileTarget>)> =
            BTreeMap::new();
        for file in &published.catalogue {
            by_dbnum.entry(file.dbnum).or_default().0 = Some(file);
        }
        for file in &current.catalogue {
            by_dbnum.entry(file.dbnum).or_default().1 = Some(file);
        }
        for (dbnum, pair) in by_dbnum {
            match pair {
                (Some(old), Some(new)) if same_file_target(old, new) => {}
                (Some(old), Some(new)) => {
                    let mut line = format!(
                        "catalogue {} {dbnum}: session {} -> {}, digest {} -> {}",
                        new.db_type,
                        old.session,
                        new.session,
                        prefix(&old.digest),
                        prefix(&new.digest)
                    );
                    if !same_file_path(&old.file, &new.file) {
                        line.push_str(&format!(", file {} -> {}", old.file, new.file));
                    }
                    parts.push(line);
                }
                (Some(old), None) => {
                    parts.push(format!("catalogue {} {dbnum} dropped", old.db_type));
                }
                (None, Some(new)) => {
                    parts.push(format!("catalogue {} {dbnum} added", new.db_type));
                }
                (None, None) => {}
            }
        }
    }
    if published.template_attlib_digest != current.template_attlib_digest {
        parts.push(format!(
            "template attlib digest {} -> {}",
            prefix(&published.template_attlib_digest),
            prefix(&current.template_attlib_digest)
        ));
    }
    if published.generator_fingerprint != current.generator_fingerprint {
        parts.push("generator fingerprint".to_string());
    }
    if published.tessellation_profile != current.tessellation_profile {
        parts.push(format!(
            "tessellation {} -> {}",
            published.tessellation_profile, current.tessellation_profile
        ));
    }
    if parts.is_empty() {
        "no difference".to_string()
    } else {
        parts.join("; ")
    }
}

/// 刚体前移回执的落款（ADR-066 D4「与重生成同款」）。`current` 是按 pin@T 现算的目标；只有设计窗之外
/// 每一格（项目 / 设计文件身份 / 目录依赖含会话 / 模板 / 生成器指纹 / 细分档）都与旧回执相同、且设计会话
/// 确实落在 T 上，才许拿它当这根的新落款——目录变了核对拍里 `Δ` 前移的记录与 regen@T 就不是同一份几何，
/// core 对目录 / 几何集变化走的也是 `REPALL`。旧回执没带目标就证不了目录没动，一律回退整根重算。
pub(crate) fn rigid_motion_target_at(
    old_receipt: &RootReceipt,
    current: ModelTarget,
    target_sesno: u32,
) -> anyhow::Result<ModelTarget> {
    let Some(published) = old_receipt.model_target.as_ref() else {
        bail!("cached receipt carries no model target; cannot prove the catalogue did not move");
    };
    anyhow::ensure!(
        same_target_outside_the_design_window(published, &current),
        "immutable target changed outside the design window (catalogue / template / generator / \
         tessellation); rigid motion cannot cover it: {}",
        describe_target_difference(published, &current)
    );
    anyhow::ensure!(
        current.design.session == target_sesno,
        "model target pins design session {} but the window targets {target_sesno}",
        current.design.session
    );
    Ok(current)
}

/// 含旋转的刚体前移重算盒（ADR-066 D3）：`bake_element` 存的 `world_bounds` 是世界网格顶点的紧 AABB，
/// 本地盒过矩阵对含旋转的 `Δ` 一律偏松、过不了对拍门，所以读回这条记录自己的网格，顶点乘到新世界系
/// `world` 再取 AABB（[`bounds_of_points`]）。网格到世界的矩阵见 [`rigid_motion_mesh_to_world`]。
/// 读不到网格 / 网格没有顶点都是这根的回退理由（`republish_records` 整根 `Err`，不留半套）。
pub(crate) fn rigid_motion_mesh_bounds(
    mesh_dir: &Path,
    record: &ProjectedGeometry,
    world: DMat4,
) -> anyhow::Result<[[f32; 3]; 2]> {
    let path = mesh_dir.join(format!("{}.mesh", record.mesh_id));
    let mesh = read_mesh_file(&path)
        .with_context(|| format!("record {} rotated: bounds need its mesh", record.record_id))?;
    let mesh_to_world = rigid_motion_mesh_to_world(record, world);
    bounds_of_points(
        mesh.vertices
            .iter()
            .map(|vertex| mesh_to_world.transform_point3(vertex.as_dvec3())),
    )
    .with_context(|| {
        format!(
            "record {} rotated: mesh {} has no vertices",
            record.record_id,
            path.display()
        )
    })
}

/// 记录的网格顶点到世界系，与生成时 [`persisted_world_transform`] / `bake_element` 同口径：烘焙件
/// （`Baked`）的网格已按 `world⁻¹` 本地化，直接乘世界矩阵；标准图元（`Canonical`）先乘 `local_transform`；
/// 隐式管身的 `local_transform` 生成时已折进存下来的世界矩阵（`persisted_world_transform`），不再乘。
pub(crate) fn rigid_motion_mesh_to_world(record: &ProjectedGeometry, world: DMat4) -> DMat4 {
    match (&record.shape, record.tube.is_some()) {
        (
            ProjectedShape::Canonical {
                local_transform, ..
            },
            false,
        ) => world * *local_transform,
        _ => world,
    }
}

/// 单元级发布的**真生产方**（ADR-080 D1 / D5 / D6 / D8）：`plan_update(S→T)` 一窗算一次，逐根切出子计划 →
/// `execute_plan` → 烘焙 → `project_geometry_delta`，管段账与 removals 来源核过之后交给执行体
/// （[`E3dModelService::republish_roots_by_unit_delta`] 的 `produce`）。
///
/// 它替代不了整根重算的几种情形各自 `bail!`，让执行体回退（回退永远正确、只是不省）：
/// 1. 规划器退化成整库重建（`full_rebuild`，Q4）——空产出不是「没变」；
/// 2. 计划期有读挂（`unresolved` / `derived_stale_unreadable`）——分类不可信；
/// 3. 旧回执没带目标、或不可变目标在设计窗之外变了（目录 / 模板 / 生成器 / 细分档）——core 对这类是
///    `REPALL`，一窗 DESI 差分覆盖不到；
/// 4. `execute_plan` 报 `regen_failed` / `derived_failed`（D8）；
/// 5. 产出里有回溯不到子计划的 removal（D6 第一条）。
///
/// 管段账（`UnitDelta::tube_segments`）取自 `IncrementReport::tube_segments`——`execute_plan` 在
/// `route_tubes_of` 返回处逐容器另记的一笔，与 `upserts` 分开；执行体的管段完整性门拿它对真带上的段数。
pub(crate) struct UnitDeltaProducer<'a> {
    service: &'a E3dModelService,
    dbnum: u32,
    base_sesno: u32,
    target_sesno: u32,
    base: GenerationInput,
    target: GenerationInput,
    plan: Arc<WindowPlanSets>,
}

impl<'a> UnitDeltaProducer<'a> {
    /// 打开一窗：两端 `DbSet` / 索引（与整根重算共用 `generation_input` 缓存），`plan_update(S→T)` 算一次。
    /// 计划口径与选根那一步（`WindowRootSources::collect`）**同一份**：元数据门开着就走两票门的
    /// `plan_update_with`，关着就是裸 `plan_update`——两边口径不同，选出来要局部发布的根与切给它的子计划
    /// 就对不上。
    ///
    /// Issue #50：先查窗口计划缓存（`cached_window_plan_sets`），命中则直接拿 `Arc` 共享的
    /// `WindowPlanSets`，省掉重复 `collect_window` + `plan_update` 的 300.5s 开销；未命中才现算
    /// 并回填缓存（`cache_window_plan_sets`）。
    pub(crate) fn open(
        service: &'a E3dModelService,
        dbnum: u32,
        base_sesno: u32,
        target_sesno: u32,
    ) -> anyhow::Result<Self> {
        // Issue #50：先查窗口计划缓存。分域键含文件身份，命中即同一份文件的同一窗；载荷与键在
        // `window_plan_sets` 里同时写入，不会再出现「键是 B、输入是 A」。两端集合只在绑着打开它们的
        // 那张依赖会话表时复用（省掉 `generation_input` 的单库槽位乒乓逐出），否则只复用计划。
        let source_file = service.source_file(dbnum)?.to_path_buf();
        let cached = cached_window_plan_sets(dbnum, &source_file, base_sesno, target_sesno);
        if let Some(plan) = &cached
            && let Some((base, target)) = plan
                .inputs
                .as_ref()
                .and_then(CachedWindowInputs::generation_inputs)
        {
            return Ok(Self {
                service,
                dbnum,
                base_sesno,
                target_sesno,
                base,
                target,
                plan: plan.clone(),
            });
        }
        let base = generation_input(service, dbnum, base_sesno)?;
        let target = generation_input(service, dbnum, target_sesno)?;
        if let Some(plan) = cached {
            return Ok(Self {
                service,
                dbnum,
                base_sesno,
                target_sesno,
                base,
                target,
                plan,
            });
        }
        // 缓存未命中：现算并回填，直接拿返回的 Arc（省一次锁）
        let pin = service.pin(dbnum)?;
        let window = collect_window(&pin.file, base_sesno, target_sesno).with_context(|| {
            format!(
                "collect window {base_sesno}→{target_sesno} of {}",
                pin.file.display()
            )
        })?;
        let plan = if crate::options::model_skip_metadata_only_changes() {
            plan_update_with(
                &base.set,
                &target.set,
                &window,
                PlanLimits::default(),
                crate::data_interface::window_root_plan::change_metadata_tier,
            )
        } else {
            plan_update(&base.set, &target.set, &window)
        };
        let plan = window_plan_sets(
            dbnum,
            &source_file,
            base_sesno,
            target_sesno,
            &plan,
            Some(CachedWindowInputs::from_generation_inputs(&base, &target)),
        );
        insert_window_plan_sets(plan.clone());
        Ok(Self {
            service,
            dbnum,
            base_sesno,
            target_sesno,
            base,
            target,
            plan,
        })
    }

    /// 这一窗的计划规模（调试 / 探针用）：重建单元、删除单元、派生容器。
    pub(crate) fn planned(&self) -> (usize, usize, usize) {
        (
            self.plan.regenerate.len(),
            self.plan.remove.len(),
            self.plan.regenerate_derived.len(),
        )
    }

    /// 一根的局部产出。执行体已过完幂等闸（回执停在 S）才会叫到这里。
    ///
    /// `no_model_only` 是规划器随 unit 带来的提示（[`UnitPublishUnit::no_model_only`]，D10）：为真且这根的
    /// 子计划为空时，再经 [`root_touched_only_by_no_model_candidates`] 对账本复核一遍，成立就回一份**空**
    /// [`UnitDelta`]（只带本轮的 `model_target`）——执行体拿它发一份空的 `Partial`，存储在写锁里把回执推到 T、
    /// 章按原记录重算（逐字不变）。为假的空子计划照旧 `bail!` 回退。
    pub(crate) fn produce(
        &mut self,
        root: RefNo,
        old_receipt: &RootReceipt,
        no_model_only: bool,
    ) -> anyhow::Result<UnitDelta> {
        self.plan.admits_partial_publish()?;
        // Issue #50：按根大小设门——记录数 < 10 的根走单元级发布不划算（切子计划、
        // 校验、提交的固定开销摊不过来），直接回退整根重算。阈值拍脑袋定 10：
        // 小根局部重建本身就快，单元路省不下多少，反而多一道闸。
        // 但 D10 纯回执快道（no_model_only）不切子计划网格只推回执，必须豁免门禁。
        const MIN_RECORDS_FOR_UNIT_PUBLISH: usize = 10;
        if !no_model_only && old_receipt.geometry_count < MIN_RECORDS_FOR_UNIT_PUBLISH {
            bail!(
                "root {root} has {} geometry record(s) (< {MIN_RECORDS_FOR_UNIT_PUBLISH}); \
                 unit-level publish overhead exceeds its benefit, falling back to regen",
                old_receipt.geometry_count
            );
        }
        // 不可变目标先算、先核（便宜）：设计窗之外任何一格变了，都不是局部能覆盖的。
        self.service
            .hydrate_published_dependencies(old_receipt, &self.target.dependency_sessions)?;
        let model_target = self.service.model_target(
            self.dbnum,
            self.target_sesno,
            &self.target.dependency_sessions,
        )?;
        let Some(published_target) = old_receipt.model_target.as_ref() else {
            bail!(
                "cached receipt of root {root} carries no model target; cannot prove a partial \
                 delta covers the whole root"
            );
        };
        anyhow::ensure!(
            same_target_outside_the_design_window(published_target, &model_target),
            "immutable target of root {root} changed outside the design window (catalogue / \
             template / generator / tessellation); a partial DESI delta cannot cover it: {}",
            describe_target_difference(published_target, &model_target)
        );

        let subplan = restrict_plan_to_root(
            &self.plan,
            root,
            self.base.index.owners.as_ref(),
            self.target.index.owners.as_ref(),
        )?;
        // 选根比 `plan_update` 看得更远（结构连接 / 电缆端点 / 设计引用级联、目录级联、带过来的
        // 未完成生成）：那些根这一窗没有一个自己的单元变了，几何却可能变了。子计划为空默认就不是
        // 「没变」，是局部清单覆盖不到的理由——空 delta 盖一个 T 章正是 Q4 那种静默缺件。
        //
        // 唯一的例外（D10）：规划器判这根**只被 `no_model` 候选碰到**（闭包直接命中、非级联、S 端安定、
        // 名下候选全是两票门放行的），且这里对账本复核也成立——那空子计划读作「DESI 侧没有一件几何要变」，
        // 回一份空 delta 只推回执。设计窗之外的目标（目录 / 模板 / 生成器）已在上面核过没变。
        if subplan.is_empty() {
            anyhow::ensure!(
                no_model_only,
                "window plan {}→{} names no unit under root {root}; the root was selected for a reason a \
                 partial DESI delta cannot see (cascade / recovery) and the planner did not mark it \
                 no_model_only; falling back to regen",
                self.base_sesno,
                self.target_sesno
            );
            root_touched_only_by_no_model_candidates(
                &self.plan.dispositions,
                root,
                self.base.index.owners.as_ref(),
                self.target.index.owners.as_ref(),
            )
            .with_context(|| {
                format!(
                    "window plan {}→{} names no unit under root {root} and the no_model_only hint \
                     does not hold against the ledger; falling back to regen",
                    self.base_sesno, self.target_sesno
                )
            })?;
            return Ok(UnitDelta {
                model_target: Some(model_target),
                ..UnitDelta::default()
            });
        }
        let (plan, sets) = subplan.into_update_plan(self.base_sesno, self.target_sesno);
        let outcome = execute_plan(&self.target.set, plan);
        let failed = outcome.report.regen_failed.len() + outcome.report.derived_failed.len();
        anyhow::ensure!(
            failed == 0,
            "execute_plan under root {root}: {failed} unit(s) / container(s) failed ({} regen, {} derived)",
            outcome.report.regen_failed.len(),
            outcome.report.derived_failed.len()
        );
        // 生成途中懒开的目录库这时才进依赖会话表：按生成后的表重算落款再核一次。设计改动在本窗
        // 引进的新目录依赖（换了 SPREF 一类）到这里才看得见，局部清单记不下它，回退整根重算。
        let model_target = self.service.model_target(
            self.dbnum,
            self.target_sesno,
            &self.target.dependency_sessions,
        )?;
        anyhow::ensure!(
            same_target_outside_the_design_window(published_target, &model_target),
            "immutable target of root {root} changed outside the design window during generation \
             (a catalogue dependency first read in this window); a partial DESI delta cannot record it: {}",
            describe_target_difference(published_target, &model_target)
        );
        removals_trace_back_to_the_plan(&sets, &outcome.removals)?;
        let mut tube_segments = outcome
            .report
            .tube_segments
            .iter()
            .map(|(container, count)| {
                let container = parse_refno(container)?;
                Ok(((container.word0, container.word1), *count))
            })
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        // 迁出这根的路由容器：它的新管身归 T 端的新根去建，这根只整段清掉旧的——账上记 0 段，
        // 管段门按「容器退休的合法整段清空」放行。
        for container in &sets.retired_containers {
            tube_segments.entry(*container).or_insert(0);
        }
        // 单元级局部发布（ADR-078）不带房间边：只算了变了的元素，整根的边由 flush 现算（B/Q5 兜底）。
        let baked = bake_elements(outcome.upserts, self.service.mesh_dir.as_path(), None)?;
        let (commit, _) = project_geometry_delta(
            self.dbnum,
            self.target_sesno,
            baked,
            outcome.removals,
            None,
            &self.target.index.owners,
        )?;
        Ok(UnitDelta {
            tube_branch_resets: commit.tube_branch_resets,
            removals: commit.removals,
            upserts: commit.upserts,
            model_target: Some(model_target),
            tube_segments,
        })
    }
}

/// 一根从一窗计划里切出来的子计划（[`restrict_plan_to_root`] 的产物）。
#[derive(Debug, Default, PartialEq, Eq)]
struct RootSubplan {
    /// T 端在这根之下、要重建的单元。
    regenerate: BTreeSet<(u32, u32)>,
    /// 这根的投影里要显式删掉的单元：窗里删掉的（只在 S 端有），**加上迁出这根的**（T 端还在、却已不在
    /// 这根之下）。
    remove: BTreeSet<(u32, u32)>,
    /// T 端在这根之下、要整链重算的路由容器；T 端已删、S 端在这根之下的也在这里（`execute_plan` 记 0 段、
    /// 旧键由差集清）。
    regenerate_derived: BTreeSet<(u32, u32)>,
    /// 上面两组容器在 S 端的旧管段键：`execute_plan` 没被本轮建成的都发 Remove。
    derived_stale: BTreeSet<GeometryId>,
    /// **迁出**这根的路由容器：T 端还在、却已不在这根之下。它留在这根名下的旧管身要整段清掉
    /// （旧键进 `derived_stale` → Remove → 存储对该分支 reset），新管身归它在 T 端的新根去建；
    /// 生产方替它记 0 段的账，管段门按「容器退休的合法整段清空」放行。
    retired_containers: BTreeSet<(u32, u32)>,
}

impl RootSubplan {
    /// 这一窗的计划里没有一件落在这根名下。
    fn is_empty(&self) -> bool {
        self.regenerate.is_empty()
            && self.remove.is_empty()
            && self.regenerate_derived.is_empty()
            && self.derived_stale.is_empty()
            && self.retired_containers.is_empty()
    }

    /// 折成 `execute_plan` 吃的 [`UpdatePlan`]，连同 D6 守卫要对的三处来源（[`RootPlanSets`]）。
    fn into_update_plan(self, base_sesno: u32, target_sesno: u32) -> (UpdatePlan, RootPlanSets) {
        let plan = UpdatePlan {
            regenerate: self.regenerate.clone(),
            remove: self.remove.clone(),
            regenerate_derived: self.regenerate_derived.clone(),
            derived_stale: self.derived_stale.clone(),
            ledger: ChangeLedger::new(),
            full_rebuild: None,
            report: IncrementReport {
                base_sesno,
                target_sesno,
                units_to_regenerate: self.regenerate.len(),
                units_to_remove: self.remove.len(),
                derived_to_regenerate: self.regenerate_derived.len(),
                ..IncrementReport::default()
            },
        };
        (
            plan,
            RootPlanSets {
                regenerate: self.regenerate,
                remove: self.remove,
                derived_stale: self.derived_stale,
                retired_containers: self.retired_containers,
            },
        )
    }
}

/// 从一窗的计划里切出 `root` 名下的子计划。归属按祖先链判：活在 T 端的（`regenerate`、还在的派生容器）
/// 走 T 端属主表，T 端已没有的（`remove`、退休的派生容器）走 S 端属主表；`derived_stale` 跟着它的容器。
///
/// **迁出这根的**（S 端在这根之下、T 端不在了——改挂到别的根，或挂到了没有生成根的地方）局部清单必须
/// 显式点名（D2）：单元进这根的 `remove`，路由容器进 `retired_containers`（旧管段键进 `derived_stale`）。
/// 不点名的话它的记录留在这根名下成鬼影；整根重算时 `whole_root_removals` 拿缺席推出的正是这一条删除，
/// 局部清单没有缺席这回事，只能自己写出来。它在 T 端的新根（若有）以同一个 `record_id` 重新 upsert，
/// 整窗同一批发布里删除先于插入，两边不打架。
///
/// 祖先链读挂（回环）按错误上抛——静默把它从子计划里漏掉就是陈旧几何。
fn restrict_plan_to_root(
    plan: &WindowPlanSets,
    root: RefNo,
    owners_s: &BTreeMap<(u32, u32), RefNo>,
    owners_t: &BTreeMap<(u32, u32), RefNo>,
) -> anyhow::Result<RootSubplan> {
    let packed_root = packed_refno(root);
    let under = |unit: (u32, u32), owners: &BTreeMap<(u32, u32), RefNo>| {
        ancestor_chain(RefNo::new(unit.0, unit.1), owners).map(|chain| chain.contains(&packed_root))
    };
    let mut subplan = RootSubplan::default();
    for unit in &plan.regenerate {
        if under(*unit, owners_t)? {
            subplan.regenerate.insert(*unit);
        } else if under(*unit, owners_s)? {
            subplan.remove.insert(*unit);
        }
    }
    for unit in &plan.remove {
        if under(*unit, owners_s)? {
            subplan.remove.insert(*unit);
        }
    }
    for container in &plan.regenerate_derived {
        if owners_t.contains_key(container) {
            if under(*container, owners_t)? {
                subplan.regenerate_derived.insert(*container);
            } else if under(*container, owners_s)? {
                subplan.retired_containers.insert(*container);
            }
        } else if under(*container, owners_s)? {
            subplan.regenerate_derived.insert(*container);
        }
    }
    for stale in &plan.derived_stale {
        if let GeometryId::ImpliedTube {
            container_refno, ..
        } = stale
        {
            let container = parse_refno(container_refno)?;
            let container = (container.word0, container.word1);
            if subplan.regenerate_derived.contains(&container)
                || subplan.retired_containers.contains(&container)
            {
                subplan.derived_stale.insert(stale.clone());
            }
        }
    }
    Ok(subplan)
}

/// 一个生成根这一轮的投影决策：现在要投的目标是什么，上一轮投成了什么。
///
/// 它取代了旧的根凭证库行。跟过来的是「目标没变就不必重算」那条判据；没跟过来的是
/// revision CAS——那把锁护的是多个进程抢同一个库行，进程内投影没有那个竞争。
#[derive(Debug, Clone)]
struct RootPublication {
    root: RefNo,
    model_target: Option<ModelTarget>,
    published: Option<RootReceipt>,
    projection_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFileTarget {
    pub dbnum: u32,
    pub db_type: String,
    pub file: String,
    pub session: u32,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTarget {
    pub project: String,
    pub design: ModelFileTarget,
    pub catalogue: Vec<ModelFileTarget>,
    pub template_attlib_digest: String,
    pub generator_fingerprint: String,
    pub tessellation_profile: String,
}

impl ModelFileTarget {
    /// 这一项记了会话没有。`session == 0` 是「没记」：字典库（DICT）按约定会话位固定 0、身份只按
    /// 文件记（[`E3dModelService::model_target`]）；目录库没被这根打开过、又没钉住时也落 0。没记的
    /// 会话不能拿去对文件最新会话——永远不等：`/model/ensure` 会把每一根都判成过期、生成完再判一次
    /// 照样过期、抛 409。判新旧只比摘要；库级目录落款（ADR-079）也收不了它。
    pub fn session_tracked(&self) -> bool {
        self.session != 0
    }
}

impl ModelTarget {
    /// 库级目录落款要收的项（ADR-079，`目录 dbnum → 会话`）：只有记了会话的目录项。会话位 0 的项
    /// （字典库）收进去就是 `0 → 文件最新` 这一窗，永远收不了，目录核对每次冷启动都把整库判成前移。
    pub fn catalogue_sessions(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.catalogue
            .iter()
            .filter(|file| file.session_tracked())
            .map(|file| (file.dbnum, file.session))
    }
}
use crate::fast_model::e3d_mesh_store::{
    E3dPersistReport, MeshWrite, baked_mesh_id, canonical_mesh_id, ensure_mesh_file,
    geometry_record_id, read_mesh_file,
};

/// One database's element index at a pinned session: its top-level elements
/// (owner missing, self, or not indexed — normally the WORL) and the owner of
/// every indexed element.  `roots` is also the entry set for the file-side
/// generation-root enumeration (`generation_root::enumerate_generation_roots`).
#[derive(Debug, Clone)]
pub(crate) struct SourceIndex {
    /// 文件自报的库号（描述符 `db_mark`）——扫的是哪个库由文件说了算，不由调用方转述。
    pub(crate) dbnum: u32,
    pub(crate) roots: Vec<RefNo>,
    /// Shared, never mutated after the scan: one in-memory batch holds as many
    /// `GeneratedSnapshot`s as it has roots, and each used to carry its own copy
    /// of this whole-database table.
    pub(crate) owners: Arc<BTreeMap<(u32, u32), RefNo>>,
}

/// Typed failures of [`E3dModelService::generate_snapshot_source`] that a caller
/// must be able to distinguish without parsing message text.  Everything else
/// that path returns is a plain `anyhow` error meaning "geometry could not be
/// generated"; downcast to this type to recognise the source-side cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotSourceError {
    /// The session's element index has no entry for the requested root: at that
    /// session the refno simply did not exist (yet, or any more).
    RootNotInSession { root: RefNo, dbnum: u32, sesno: u32 },
}

impl std::fmt::Display for SnapshotSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootNotInSession { root, dbnum, sesno } => write!(
                f,
                "refno {root} is not in the element index of dbnum {dbnum} at session {sesno}"
            ),
        }
    }
}

impl std::error::Error for SnapshotSourceError {}

/// [`E3dModelService::realtime_compute_source`] 没有产出快照的两种结局。
#[derive(Debug)]
pub enum RealtimeComputeError {
    /// DFS 钩子叫停（取消或超过元素上限）：已访问 `visited` 个元素，部分产出丢弃。
    Stopped { stop: Stop, visited: usize },
    /// 与 [`E3dModelService::generate_snapshot_source`] 同一种失败：根不在会话索引里
    /// （可 downcast 成 [`SnapshotSourceError`]）、读库失败、`Required` 策略下有失败元素。
    Failed(anyhow::Error),
}

impl std::fmt::Display for RealtimeComputeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped { stop, visited } => {
                write!(f, "generation stopped after {visited} element(s): {stop:?}")
            }
            Self::Failed(error) => write!(f, "{error:#}"),
        }
    }
}

impl std::error::Error for RealtimeComputeError {}

impl From<anyhow::Error> for RealtimeComputeError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(error)
    }
}

/// Pure, session-pinned generation result.  It contains no database handle and
/// is safe to route either to the current projection or to the historical
/// in-memory projection.
pub struct GeneratedSnapshot {
    pub elements: Vec<GeneratedElement>,
    pub owners: Arc<BTreeMap<(u32, u32), RefNo>>,
    pub report: Report,
    /// e3d-model DFS 访问到的子树元素（含根、含不出几何的容器）。读透形态下这就是
    /// 「生成一根要顺手写透哪些元素」的名单（`GenerationOutcome::visited`）。
    pub visited: Vec<RefNo>,
    /// 路由成员 / NOZZ 的目录 P 点集，每件一条（`GenerationOutcome::point_sets`）：与几何同一次求值，
    /// 发布时折成 `pe_ptset` 行随根落库（[`crate::fast_model::ptset_rows`]）。
    pub point_sets: Vec<e3d_model::MemberPointSet>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectDbFile {
    db_type: String,
    project: String,
    path: PathBuf,
}

type ProjectDbFiles = Arc<HashMap<(String, u32), ProjectDbFile>>;
type DependencySessions = Arc<std::sync::Mutex<BTreeMap<(u32, u32), u32>>>;

fn project_db_key(project: &str, dbnum: u32) -> (String, u32) {
    (project.trim().to_ascii_lowercase(), dbnum)
}

/// Pick a dependency in the target design database's project when that project
/// has a same-type copy of the selected MDB database. Foreign DESI files carry
/// project-relative catalogue references: ACP 7015, for example, needs ACP
/// CATA 7000 even though `/ALL` also declares AMS CATA 7000.
fn select_dependency_file(
    locator: &InMemoryCataLocator,
    project_files: &ProjectDbFiles,
    preferred_project: Option<&str>,
    dbnum: u32,
) -> Option<ProjectDbFile> {
    let selected_type = locator.db_type_of(dbnum)?;
    if let Some(candidate) = preferred_project
        .and_then(|project| project_files.get(&project_db_key(project, dbnum)))
        .filter(|candidate| candidate.db_type.eq_ignore_ascii_case(&selected_type))
    {
        return Some(candidate.clone());
    }
    let (project, path) = locator.file_of(dbnum)?;
    Some(ProjectDbFile {
        db_type: selected_type,
        project,
        path,
    })
}

/// A source-session-pinned batch of generated roots.  Generation fills this
/// structure without touching the persisted model projection; a later
/// consumer may publish its snapshots in bounded transactions.
pub struct GeneratedMemoryBatch {
    pub(crate) dbnum: u32,
    pub(crate) source_sesno: u32,
    pub(crate) set: Arc<DbSet>,
    pub(crate) index: Arc<SourceIndex>,
    dependency_sessions: DependencySessions,
    pub(crate) snapshots: Vec<(RefNo, BakedSnapshot)>,
    /// Roots whose generation raised a hard error, kept as dead letters instead
    /// of aborting the batch (ADR-041 §6).  The consumer counts them as failed.
    pub(crate) failures: Vec<(RefNo, String)>,
}

/// 一片切片按初始化口径折成的投影（ADR-077 D5）：逐根的 [`ProjectionCommit`]（带回执）与死信。
/// 没有进过 `ModelMemoryStore`，也不会进——它只是渲 SQL 的输入。
pub(crate) struct InitialSliceProjection {
    pub dbnum: u32,
    pub source_sesno: u32,
    /// 成功折出来的根：`(根, 提交, 这一根的账)`。零几何的根也在这里（回执 `geometry_count = 0`）。
    pub commits: Vec<(RefNo, ProjectionCommit, E3dPersistReport)>,
    /// 死信：生产端硬错（整根没算出来）。
    pub failures: Vec<(RefNo, String)>,
    /// 有失败元素、却按冷根口径发布了幸存件的根（GitHub #74，`incomplete_root_policy(false, false)` = `Publish`）：
    /// 它们的提交也在 `commits` 里；这里记的是「哪几件没算出来」那句话，供回执点名。初始化前库里没有旧行、
    /// 也没有回执，丢掉整根只会「保住」一个空快照（与 SLYK `21491/17101` 同一形状），所以不再当死信。
    pub partial: Vec<(RefNo, String)>,
    /// 每个成功根 e3d-model DFS 访问到的子树元素名单（观测用，初始化不写透）。
    pub visited_by_root: HashMap<RefNo, Vec<RefNo>>,
}

/// An initialization producer outcome. Success is a frozen projection, not a
/// database completion and not a mutation of the process Current projection.
pub struct FrozenGeneration {
    pub root: RefNo,
    pub outcome: Result<ProjectionCommit, String>,
}

/// The run's catalogue mesh caches, one per resident worker, checked out for
/// the length of a wave and returned afterwards.  Only the checkout touches the
/// mutex; evaluation never runs inside it (ADR-052 background 2).
type CachePool = Arc<std::sync::Mutex<Vec<CatalogueMeshCache>>>;

fn checkout_cache(pool: &CachePool) -> CatalogueMeshCache {
    pool.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop()
        .unwrap_or_default()
}

fn return_cache(pool: &CachePool, cache: CatalogueMeshCache) {
    pool.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(cache);
}

/// The roots of one production run, and the sliding window that keeps their
/// generated geometry bounded.
///
/// Workers claim by index and write back **into their own slot**; the assembler
/// takes them off the front in source order.  Order is not decoration: it is
/// what makes slice boundaries and the `Required` verdict reproducible, and it
/// is why `geometry_workers = 1` replays the old sequential run exactly.
///
/// The window is what replaces a per-cohort barrier.  Waiting for a whole
/// cohort to finish before starting the next one costs the slowest root of
/// every cohort, and geometry load is long-tailed; here a worker may run up to
/// `window` roots ahead of the last one the assembler took, so only the head of
/// the line can stall anyone.
struct RootQueue {
    roots: Vec<RefNo>,
    /// Next index a worker may claim.
    next: std::sync::atomic::AtomicUsize,
    /// How many roots the assembler has taken off the front.
    committed: std::sync::atomic::AtomicUsize,
    window: usize,
    slots: std::sync::Mutex<Vec<Option<anyhow::Result<BakedSnapshot>>>>,
}

impl RootQueue {
    fn new(roots: Vec<RefNo>, window: usize) -> Self {
        let slots = std::sync::Mutex::new((0..roots.len()).map(|_| None).collect());
        Self {
            roots,
            next: std::sync::atomic::AtomicUsize::new(0),
            committed: std::sync::atomic::AtomicUsize::new(0),
            window,
            slots,
        }
    }

    /// How many roots a worker could still start right now.  The assembler uses
    /// it to decide how many workers to keep resident.
    fn claimable(&self) -> usize {
        let next = self.next.load(std::sync::atomic::Ordering::Acquire);
        let ceiling = self
            .roots
            .len()
            .min(self.committed.load(std::sync::atomic::Ordering::Acquire) + self.window);
        ceiling.saturating_sub(next)
    }

    /// Claim the next root, or `None` when the run is done or the window is
    /// full.  Returning `None` is how a worker hands its permit back rather
    /// than waiting on the assembler: a permit holder that waits is exactly the
    /// capacity inversion ADR-052 opens with.
    fn claim(&self) -> Option<(usize, RefNo)> {
        let mut next = self.next.load(std::sync::atomic::Ordering::Acquire);
        loop {
            let ceiling = self
                .roots
                .len()
                .min(self.committed.load(std::sync::atomic::Ordering::Acquire) + self.window);
            if next >= ceiling {
                return None;
            }
            match self.next.compare_exchange_weak(
                next,
                next + 1,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => return Some((next, self.roots[next])),
                Err(observed) => next = observed,
            }
        }
    }

    fn store(&self, slot: usize, outcome: anyhow::Result<BakedSnapshot>) {
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[slot] = Some(outcome);
    }

    /// Take the next root in source order, if its worker has finished.  Taking
    /// it is also what slides the window forward.
    fn take_ready(&self) -> Option<(RefNo, anyhow::Result<BakedSnapshot>)> {
        let committed = self.committed.load(std::sync::atomic::Ordering::Acquire);
        let outcome = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(committed)?
            .take()?;
        self.committed
            .store(committed + 1, std::sync::atomic::Ordering::Release);
        Some((self.roots[committed], outcome))
    }
}

/// One resident worker: one permit, one catalogue cache, and a loop that keeps
/// claiming until the run is done or the window is full.
///
/// Dropping the returned future does not cancel the closure — a synchronous CPU
/// segment cannot be interrupted.  An abandoned worker stops on its own within
/// at most `window` roots, because nothing is advancing `committed` any more.
fn spawn_generation_worker(
    set: Arc<DbSet>,
    index: Arc<SourceIndex>,
    queue: Arc<RootQueue>,
    caches: CachePool,
    mesh_dir: PathBuf,
    rooms: Option<Arc<crate::fast_model::room_model::MemoryRoomCalculator>>,
) -> impl std::future::Future<Output = anyhow::Result<()>> {
    crate::fast_model::concurrency::run_gated_cpu(move || {
        let mut cache = checkout_cache(&caches);
        let mut lod1_cache =
            crate::fast_model::mesh_lod1::lod1_enabled().then(CatalogueMeshCache::default);
        while let Some((slot, root)) = queue.claim() {
            // Bake here, not on the consumer: triangulation, the SHA-256 and
            // the mesh file write are the rest of this root's CPU cost, and
            // doing them here is what lets the `Manifold` die on this thread
            // instead of riding into the serial publication step.
            // 房间归属也在这里算（B/T2）：网格还在手上，判定点不必等 flush 再从盘上回读。
            let started = std::time::Instant::now();
            let evaluated = generate_snapshot_from_set(&set, &index, root, &mut cache);
            let evaluate_micros = started.elapsed().as_micros();
            let mut outcome =
                evaluated.and_then(|snapshot| bake_snapshot(snapshot, &mesh_dir, rooms.as_deref()));
            let bake_micros = started.elapsed().as_micros().saturating_sub(evaluate_micros);
            // 粗网格（`AIOS_MESH_LOD1`）单独记时：它是 L0 之外的显示增强，不许混进 `bake=` 那一栏——
            // 那一栏是用来定位长尾的。稳态下这里只按盘分拣，几乎不花时间。
            let lod1_started = std::time::Instant::now();
            if let (Some(lod1_cache), Ok(baked)) = (lod1_cache.as_mut(), outcome.as_mut()) {
                baked.lod1 = crate::fast_model::mesh_lod1::bake_lod1_root(
                    &set,
                    root,
                    baked.elements.iter().map(Into::into),
                    &mesh_dir,
                    lod1_cache,
                );
            }
            let lod1_micros = lod1_started.elapsed().as_micros();
            // 一根一行的生成账：整库那一档的墙钟压在哪几根上、是取数+几何还是烘焙，
            // 只有按根记才看得出来——按批记会把长尾摊平（7999 上 25 根里几根就占了 80%）。
            log::info!(
                "model root generated: root={root} slot={slot} elements={} evaluate={:.1}ms bake={:.1}ms lod1={:.1}ms",
                outcome
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.elements.len()),
                evaluate_micros as f64 / 1000.0,
                bake_micros as f64 / 1000.0,
                lod1_micros as f64 / 1000.0,
            );
            queue.store(slot, outcome);
        }
        return_cache(&caches, cache);
        Ok(())
    })
}

/// One slice under construction on the producer side.  It exists so that the
/// element budget is counted where the elements are produced, not recovered by
/// walking a finished batch.
#[derive(Default)]
struct MemorySlice {
    snapshots: Vec<(RefNo, BakedSnapshot)>,
    failures: Vec<(RefNo, String)>,
    elements: usize,
}

impl MemorySlice {
    fn push(&mut self, root: RefNo, snapshot: BakedSnapshot) {
        self.elements += snapshot.elements.len();
        self.snapshots.push((root, snapshot));
    }

    fn is_full(&self, root_budget: usize, element_budget: usize) -> bool {
        self.snapshots.len() >= root_budget || self.elements >= element_budget
    }

    /// Hand the sealed slice over and start an empty one.  A slice carrying
    /// nothing but dead letters still goes: the consumer is where they are
    /// counted.  Returns false once the consumer is gone — the producer stops
    /// quietly then, so that the consumer's error is the one that surfaces.
    async fn deliver(
        &mut self,
        dbnum: u32,
        source_sesno: u32,
        input: &GenerationInput,
        sink: &tokio::sync::mpsc::Sender<GeneratedMemoryBatch>,
    ) -> bool {
        if self.snapshots.is_empty() && self.failures.is_empty() {
            return true;
        }
        self.elements = 0;
        let batch = GeneratedMemoryBatch {
            dbnum,
            source_sesno,
            set: input.set.clone(),
            index: input.index.clone(),
            dependency_sessions: input.dependency_sessions.clone(),
            snapshots: std::mem::take(&mut self.snapshots),
            failures: std::mem::take(&mut self.failures),
        };
        sink.send(batch).await.is_ok()
    }
}

static DB_GENERATION_LOCKS: OnceLock<dashmap::DashMap<u32, Arc<Mutex<()>>>> = OnceLock::new();

#[derive(Clone)]
struct GenerationInput {
    set: Arc<DbSet>,
    index: Arc<SourceIndex>,
    dependency_sessions: DependencySessions,
}

type GenerationInputKey = (u32, u32, String);

/// 生成输入缓存的表（ADR-084）：键 `(dbnum, source_sesno, identity)`，**每库最多一条**（同库换 sesno / 换 identity
/// 就替换——换库 = 换时点，旧条不能再喂），再按**库数**设上限：每条记「最近一次被生成用到」的单调序号，
/// 命中也刷新（保护的正是「最近在生成的库」这份热度；一窗内同库只刷一次，成本可忽略），库数超过上限时
/// 最久没用的库整份出。值是 `Arc` 的句柄：被逐出的那份由还握着它的切片继续用完，这里只是不再喂新调用方。
/// `max_dbnums == 0` = 不限（与 2026-09-22 之前逐位相同）。泛型只为了单测不用开真文件。
struct DbnumLru<V> {
    entries: HashMap<GenerationInputKey, (V, u64)>,
    next_tick: u64,
    evicted_dbnums: u64,
}

impl<V: Clone> DbnumLru<V> {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            next_tick: 0,
            evicted_dbnums: 0,
        }
    }

    fn tick(&mut self) -> u64 {
        self.next_tick += 1;
        self.next_tick
    }

    /// 命中即刷新：这一库刚被生成用到。
    fn get_touch(&mut self, key: &GenerationInputKey) -> Option<V> {
        let tick = self.tick();
        let (value, last_used) = self.entries.get_mut(key)?;
        *last_used = tick;
        Some(value.clone())
    }

    /// 装入：先把同库的旧条替掉（换 sesno / identity），再按库数上限把最久没用的**别的**库整份出。
    /// 返回被替 / 被逐的值，调用方在锁外 drop（`DbSet` 的析构要关文件、放元素缓存，不该在缓存锁里做）。
    fn insert(&mut self, key: GenerationInputKey, value: V, max_dbnums: usize) -> Vec<V> {
        let dbnum = key.0;
        let mut displaced: Vec<V> = Vec::new();
        let same_dbnum: Vec<GenerationInputKey> = self
            .entries
            .keys()
            .filter(|(cached, _, _)| *cached == dbnum)
            .cloned()
            .collect();
        for stale in same_dbnum {
            if let Some((old, _)) = self.entries.remove(&stale) {
                displaced.push(old);
            }
        }
        let tick = self.tick();
        self.entries.insert(key, (value, tick));
        if max_dbnums > 0 {
            while self.dbnum_count() > max_dbnums {
                let Some(oldest) = self
                    .entries
                    .iter()
                    .filter(|((cached, _, _), _)| *cached != dbnum)
                    .min_by_key(|(_, (_, last_used))| *last_used)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                if let Some((old, _)) = self.entries.remove(&oldest) {
                    displaced.push(old);
                }
                self.evicted_dbnums += 1;
            }
        }
        displaced
    }

    /// 整库忘掉（初始化收尾、常驻房间换代），不计入按上限的逐出数。
    fn forget_dbnum(&mut self, dbnum: u32) -> Vec<V> {
        let keys: Vec<GenerationInputKey> = self
            .entries
            .keys()
            .filter(|(cached, _, _)| *cached == dbnum)
            .cloned()
            .collect();
        keys.iter()
            .filter_map(|key| self.entries.remove(key).map(|(value, _)| value))
            .collect()
    }

    fn dbnum_count(&self) -> usize {
        self.entries
            .keys()
            .map(|(dbnum, _, _)| *dbnum)
            .collect::<BTreeSet<_>>()
            .len()
    }

    fn iter(&self) -> impl Iterator<Item = (&GenerationInputKey, &V, u64)> {
        self.entries
            .iter()
            .map(|(key, (value, last_used))| (key, value, *last_used))
    }
}

static GENERATION_INPUT_CACHE: OnceLock<std::sync::Mutex<DbnumLru<GenerationInput>>> =
    OnceLock::new();

fn generation_input_cache() -> &'static std::sync::Mutex<DbnumLru<GenerationInput>> {
    GENERATION_INPUT_CACHE.get_or_init(|| std::sync::Mutex::new(DbnumLru::new()))
}

/// 输入缓存键里的 identity：冻结源摘要，没有时取库文件规范路径。调用方已确认 `dbnum` 有 pin。
fn generation_input_identity(service: &E3dModelService, dbnum: u32) -> String {
    service.generation_identity.clone().unwrap_or_else(|| {
        let pin = service
            .pin(dbnum)
            .expect("generation db pin checked by caller");
        std::fs::canonicalize(&pin.file)
            .unwrap_or_else(|_| pin.file.clone())
            .to_string_lossy()
            .into_owned()
    })
}

/// 现建一份生成输入（扫索引 + 建 `DbSet`）：当前模型缓存与历史缓存共用这一段，两路输入一致。
fn build_generation_input(
    service: &E3dModelService,
    dbnum: u32,
    source_sesno: u32,
) -> anyhow::Result<GenerationInput> {
    let pin = service.pin(dbnum)?;
    let index = Arc::new(scan_index(&pin.file, Some(source_sesno))?);
    let set = service.build_set(dbnum, Some(source_sesno))?;
    Ok(GenerationInput {
        set,
        index,
        dependency_sessions: service.dependency_sessions.clone(),
    })
}

/// 只给历史用的输入缓存（设计 §1.3，[`HistoricalInputLru`]）：与 `GENERATION_INPUT_CACHE` 分开，
/// 来回拖动历史会话不会把当前模型那份输入挤掉。
static HISTORICAL_INPUT_CACHE: OnceLock<HistoricalInputLru<GenerationInput>> = OnceLock::new();

fn historical_input_cache() -> &'static HistoricalInputLru<GenerationInput> {
    HISTORICAL_INPUT_CACHE.get_or_init(HistoricalInputLru::with_configured_capacity)
}

fn historical_input(
    service: &E3dModelService,
    dbnum: u32,
    source_sesno: u32,
) -> anyhow::Result<GenerationInput> {
    service.pin(dbnum)?;
    let key = (
        dbnum,
        source_sesno,
        generation_input_identity(service, dbnum),
    );
    historical_input_cache()
        .get_or_build(key, || build_generation_input(service, dbnum, source_sesno))
}

/// 历史输入缓存此刻的条数与容量（`/health.model_cache.realtime_history.input_cache`）。
pub(crate) fn historical_input_stats() -> crate::fast_model::historical_input::HistoricalInputStats {
    historical_input_cache().stats()
}

fn generation_input(
    service: &E3dModelService,
    dbnum: u32,
    source_sesno: u32,
) -> anyhow::Result<GenerationInput> {
    let cache = generation_input_cache();
    let key = (
        dbnum,
        source_sesno,
        generation_input_identity(service, dbnum),
    );
    if let Some(value) = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_touch(&key)
    {
        return Ok(value);
    }
    let input = build_generation_input(service, dbnum, source_sesno)?;
    // 按库数上限（ADR-084，`model_generation_input_max_dbnums`，0 = 不限）：被替 / 被逐的那几份在锁外 drop。
    let max_dbnums = crate::options::model_generation_input_max_dbnums();
    let (displaced, resident_dbnums) = {
        let mut guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let displaced = guard.insert(key, input.clone(), max_dbnums);
        (displaced, guard.dbnum_count())
    };
    if max_dbnums > 0 && !displaced.is_empty() {
        log::info!(
            "dbnum={dbnum} 生成输入缓存按库数上限收回：放掉 {} 份，现留 {resident_dbnums} 库，上限 {max_dbnums}",
            displaced.len()
        );
    }
    drop(displaced);
    Ok(input)
}

/// 生成输入缓存这一维的现值（ADR-084，`/health.model_cache.element_cache`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GenerationInputCacheLimits {
    /// 生效的库数上限（0 = 不限）。
    pub max_dbnums: usize,
    /// 此刻留着几个库的 `GenerationInput`。
    pub resident_dbnums: usize,
    /// 进程起来以来按库数上限逐出了几份（同库换 sesno 的替换与 `forget_generation_input` 不计）。
    pub evicted_dbnums_total: u64,
}

pub(crate) fn generation_input_cache_limits() -> GenerationInputCacheLimits {
    let max_dbnums = crate::options::model_generation_input_max_dbnums();
    let Some(cache) = GENERATION_INPUT_CACHE.get() else {
        return GenerationInputCacheLimits {
            max_dbnums,
            resident_dbnums: 0,
            evicted_dbnums_total: 0,
        };
    };
    let guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    GenerationInputCacheLimits {
        max_dbnums,
        resident_dbnums: guard.dbnum_count(),
        evicted_dbnums_total: guard.evicted_dbnums,
    }
}

/// 生成输入缓存里一份 `DbSet` 的池级元素缓存此刻的体量（d-149 观测用，
/// `/health.model_cache.element_cache.databases[]`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GenerationInputFootprint {
    pub dbnum: u32,
    pub source_sesno: u32,
    /// 已缓存的解析记录条数——元素数上限数的就是它。
    pub parsed_entries: usize,
    /// 已缓存的描述符抽取条数（解析条数的子集）。
    pub extraction_entries: usize,
    /// 估算字节（`DbSet::element_cache_stats`：`Arc` 块 + 表槽 + 内联结构 + 字串 / 数组载荷）。
    pub approx_bytes: usize,
    /// 这份 `DbSet` 生效的元素数上限（0 = 不限 / 没开缓存）。
    pub max_entries: usize,
    /// 最近一次被生成用到的序号（进程内单调计数，不是时刻；越大越新）——按库数上限逐出时最小的先出（ADR-084）。
    pub last_used_tick: u64,
}

/// 生成输入缓存里每库那份 `DbSet` 的元素缓存体量，按 `dbnum` 升序（d-149 观测用）。
///
/// 查时现算、不缓存、不落库：先在缓存锁里把各份 `Arc<DbSet>` 抄出来，再在锁外逐份走它的缓存
/// （`element_cache_stats`，每份 O(条数)，各库缓存各自的读锁）——`GENERATION_INPUT_CACHE` 那把锁
/// 不被观测拖住。按观测频率调，别放进生成路径。
pub(crate) fn generation_input_cache_footprint() -> Vec<GenerationInputFootprint> {
    let Some(cache) = GENERATION_INPUT_CACHE.get() else {
        return Vec::new();
    };
    let sets: Vec<(u32, u32, Arc<DbSet>, u64)> = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|((dbnum, sesno, _), input, last_used)| (*dbnum, *sesno, input.set.clone(), last_used))
        .collect();
    let mut out: Vec<GenerationInputFootprint> = sets
        .into_iter()
        .map(|(dbnum, source_sesno, set, last_used_tick)| {
            let stats = set.element_cache_stats();
            GenerationInputFootprint {
                dbnum,
                source_sesno,
                parsed_entries: stats.parsed_entries,
                extraction_entries: stats.extraction_entries,
                approx_bytes: stats.approx_bytes,
                max_entries: stats.limit,
                last_used_tick,
            }
        })
        .collect();
    out.sort_by_key(|footprint| (footprint.dbnum, footprint.source_sesno));
    out
}

/// Drop every cached generation input of one database.
///
/// The cache is keyed by `(dbnum, sesno, identity)`; a new entry only replaces
/// the *same* dbnum's older one, and beyond that the only bound is the dbnum
/// count limit (`model_generation_input_max_dbnums`, ADR-084, default unbounded),
/// so an entry can outlive the run that built it for as long as the process
/// lives.  An initialization run keys its entry by the digest of its frozen
/// tokens — a key no later service (`from_current`, `from_frozen_source`) asks
/// for again — which makes the entry dead weight once the run has finished:
/// one parsed `attlib.dat`, one open `DbSet` over every MDB design file, and
/// one whole-database owner table per initialized dbnum, held until exit.
///
/// Callers still holding an `Arc` clone keep their input alive; the next
/// `generation_input` for the dbnum rebuilds from the file.  The removed values
/// are dropped outside the cache lock.  Returns how many entries were removed.
pub(crate) fn forget_generation_input(dbnum: u32) -> usize {
    let Some(cache) = GENERATION_INPUT_CACHE.get() else {
        return 0;
    };
    let removed: Vec<GenerationInput> = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .forget_dbnum(dbnum);
    removed.len()
}

fn db_generation_lock(dbnum: u32) -> Arc<Mutex<()>> {
    DB_GENERATION_LOCKS
        .get_or_init(dashmap::DashMap::new)
        .entry(dbnum)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Serializes the final durable-watermark fence with every current-model
/// generation for this database. The write-behind worker holds it only for the
/// last pending check and the short control-plane transaction.
pub(crate) async fn lock_db_generation(dbnum: u32) -> tokio::sync::OwnedMutexGuard<()> {
    db_generation_lock(dbnum).lock_owned().await
}

/// 拿不到就立刻返回，不等。整库模型清理用它：有生成在飞就 409，一行不删。
pub(crate) fn try_lock_db_generation(
    dbnum: u32,
) -> Result<tokio::sync::OwnedMutexGuard<()>, tokio::sync::TryLockError> {
    db_generation_lock(dbnum).try_lock_owned()
}

/// 把一段同步的 CPU 活从 tokio 工作线程上让出去，**借用照常**（不必 `'static`，这是不走
/// `spawn_blocking` 的原因：单元级发布的生产方借着 `&E3dModelService` 与两端 `DbSet`）。
///
/// 多线程运行时走 `block_in_place`：当前工作线程转成阻塞线程、别的任务照跑；`current_thread`
/// 运行时（`#[tokio::test]`）与运行时之外就地跑——`block_in_place` 在那两处会 panic。
/// 让出的只是线程，调用方手里的锁（`db_generation_lock`）照握着。
fn run_blocking<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(work)
        }
        _ => work(),
    }
}

pub struct E3dModelService {
    schema: DirectSchema,
    pins: Vec<DbPin>,
    locator: InMemoryCataLocator,
    project_files: ProjectDbFiles,
    dependency_sessions: DependencySessions,
    mesh_dir: PathBuf,
    /// Digest of the durable file-generation token for a file update round.
    /// It prevents a replaced file at the same path/dbnum/session from reusing
    /// another generation's cached DbSet and index.
    generation_identity: Option<String>,
    /// Initialization reuses the verified, durable source token, not mutable
    /// file length/mtime that may advance while the pinned session is generated.
    frozen_file_digests: BTreeMap<(u32, u32), String>,
    /// The MDB's Dictionary UDA definitions, so `build_set` can answer project
    /// attributes (`:HXYsize`) by name. `None` = no dictionary was readable;
    /// that is a degraded read, not a reason to refuse to generate geometry.
    uda: Option<Arc<e3d_io::uda_catalog::UdaCatalog>>,
}

#[cfg(test)]
#[path = "initialization_target_tests.rs"]
mod initialization_target_tests;

#[derive(Clone)]
struct E3dDbResolver {
    locator: InMemoryCataLocator,
    project_files: ProjectDbFiles,
    target_dbnum: u32,
    preferred_project: Option<String>,
    template_dir: PathBuf,
    sessions: DependencySessions,
}

impl DbFileResolver for E3dDbResolver {
    fn resolve(&self, dbno: u32) -> Option<DbFilePin> {
        let selected = select_dependency_file(
            &self.locator,
            &self.project_files,
            self.preferred_project.as_deref(),
            dbno,
        )?;
        let template = template_file_for(&self.template_dir, &selected.db_type).ok()?;
        let sesno = {
            let mut sessions = self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let key = (self.target_dbnum, dbno);
            if let Some(&sesno) = sessions.get(&key) {
                sesno
            } else {
                let mut view = DbView::open(&selected.path).ok()?;
                let sesno = view.walk_sessions().ok()?.first()?.sesno;
                sessions.insert(key, sesno);
                sesno
            }
        };
        Some(DbFilePin {
            file: selected.path,
            template,
            db_type: Some(selected.db_type),
            sesno: Some(sesno),
        })
    }
}

impl E3dModelService {
    /// Check the same immutable target and mesh completeness as generation.
    /// Receipt existence alone is not freshness: SAVEWORK, replacement/rollback,
    /// catalogue dependencies and schema changes can all invalidate it.
    pub(crate) fn current_cached_roots(
        &self,
        roots: &[RefNo],
    ) -> anyhow::Result<std::collections::HashSet<RefNo>> {
        let store = ModelMemoryStore::global();
        let mut current = std::collections::HashSet::new();
        for &root in roots {
            let Some(receipt) = store.receipt(ProjectionScope::Current, root) else {
                continue;
            };
            // Keep each root's real dependency set separate; combining unrelated
            // roots would invalidate otherwise-current targets on every request.
            let sessions = Arc::new(std::sync::Mutex::new(BTreeMap::new()));
            self.hydrate_published_dependencies(&receipt, &sessions)?;
            let source_sesno = self.source_session(self.pin(root.dbno())?)?;
            let publication = RootPublication {
                root,
                model_target: Some(self.model_target(root.dbno(), source_sesno, &sessions)?),
                published: Some(receipt),
                projection_complete: false,
            };
            if cached_root_target_matches(&publication, source_sesno)
                && store.root_projection_complete(ProjectionScope::Current, root, &self.mesh_dir)
            {
                current.insert(root);
            }
        }
        Ok(current)
    }

    /// The current projection (ADR-054): source files come from the MDB
    /// declaration, and every pin is left unpinned so that generation resolves
    /// the target database's **latest file session** at the moment it runs
    /// (`source_session`). `dbnum_watermark.applied_sesno` is the ingestion
    /// watermark, not a generation timepoint, and an empty watermark table must
    /// not stop a never-parsed project from generating.
    pub async fn from_current() -> anyhow::Result<Self> {
        let option = aios_core::get_db_option();
        let (pins, locator, project_files) = current_mdb_source_inventory()?;
        Ok(Self {
            uda: current_uda_catalog(),
            schema: DirectSchema::open_from_env()?,
            // CATA identity stays with the project-aware locator (see `build_set`
            // and `model_target`); the pins carry the design-side files only, as
            // the watermark pins did before.
            pins: pins
                .into_iter()
                .filter(|pin| !pin.db_type.eq_ignore_ascii_case("CATA"))
                .collect(),
            locator,
            project_files,
            dependency_sessions: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            mesh_dir: crate::options::meshes_dir_for(option),
            generation_identity: None,
            frozen_file_digests: BTreeMap::new(),
        })
    }

    /// Build the normal MDB-aware service while pinning one DESI input to the
    /// exact file generation and session captured by the data file round.
    pub async fn from_frozen_source(
        source: &crate::data_interface::frozen_source::FrozenSource,
    ) -> anyhow::Result<Self> {
        Self::from_frozen_sources(std::slice::from_ref(source), false).await
    }

    /// Initialization pins the entire resolver inventory before the manifest
    /// becomes active. A dependency can never fall back to a later session.
    pub async fn from_initialization_sources(
        source: &crate::data_interface::frozen_source::FrozenSource,
        dependencies: &[String],
    ) -> anyhow::Result<Self> {
        use crate::data_interface::frozen_source::FrozenSource;
        let mut service = Self::from_frozen_source(source).await?;
        let mut files = HashMap::new();
        let mut tokens = vec![source.commit_token_json()?];
        let target_dbnum = source.dbnum() as u32;
        for token in dependencies.iter().chain(tokens.clone().iter()) {
            let frozen = FrozenSource::from_commit_token_json(token)?.reopen_verified()?;
            // 摘要故意记 sha256(commit_token_json) 而不是现场身份（2026-10-24 起是冻结锚 `model_file_identity`，
            // 此前是 `model_file_identity_digest` 路径 + 长度 + 修改时间 + 会话）：token 里全是冻结那一刻的值，同一份 token 之后重开、文件被 SAVEWORK
            // 追加过也算得出同一个数（`initialization_target_tests::initialization_target_survives_append_…`），
            // `room_resident` 的 room-input-cache 就靠它让没变的文件只付重开校验的钱。它与
            // `room_read_through::target_current` 现场重算的公式对不上没关系：初始化的回执不进投影
            // （ADR-077 D5），`target_current` 只会看到按需 / 批次那条路写的、按现场公式记的回执。
            service.frozen_file_digests.insert(
                (frozen.dbnum() as u32, frozen.target_sesno()),
                hex::encode(Sha256::digest(frozen.commit_token_json()?.as_bytes())),
            );
            let project = service
                .locator
                .file_of(frozen.dbnum() as u32)
                .map(|(project, _)| project)
                .context("frozen dependency is no longer in MDB")?;
            files.insert(
                frozen.dbnum() as u32,
                (
                    frozen.db_type().to_string(),
                    project,
                    frozen.path().to_path_buf(),
                ),
            );
            service
                .dependency_sessions
                .lock()
                .unwrap()
                .insert((target_dbnum, frozen.dbnum() as u32), frozen.target_sesno());
            if let Some(pin) = service.pins.iter_mut().find(|p| p.dbnum == frozen.dbnum()) {
                pin.file = frozen.path().to_path_buf();
                pin.sesno = Some(frozen.target_sesno());
            }
        }
        service.locator = InMemoryCataLocator::from_parts(HashMap::new(), files);
        // The initialization token list is already the exact dependency
        // inventory. Do not replace one of those frozen files with a live
        // same-project alternative.
        service.project_files = Arc::new(HashMap::new());
        tokens.extend_from_slice(dependencies);
        tokens.sort_unstable();
        service.generation_identity =
            Some(hex::encode(Sha256::digest(tokens.join("\n").as_bytes())));
        Ok(service)
    }

    pub fn initialization_target_hash(&self, dbnum: u32, session: u32) -> anyhow::Result<String> {
        let target = self.model_target(dbnum, session, &self.dependency_sessions)?;
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(&Some(
            target,
        ))?)))
    }

    /// Build one service whose selected DESI inputs are all pinned to their
    /// durable file generations. Room topology passes `published_only=true`
    /// so an MDB file without a successful published version cannot leak in at
    /// its live latest session.
    pub(crate) async fn from_frozen_sources(
        sources: &[crate::data_interface::frozen_source::FrozenSource],
        published_only: bool,
    ) -> anyhow::Result<Self> {
        let sources = sources
            .iter()
            .map(|source| source.reopen_verified())
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut service = Self::from_current().await?;
        if published_only {
            let published = sources
                .iter()
                .map(|source| source.dbnum())
                .collect::<std::collections::HashSet<_>>();
            service.pins.retain(|pin| {
                !pin.db_type.eq_ignore_ascii_case("DESI") || published.contains(&pin.dbnum)
            });
        }
        let mut tokens = Vec::with_capacity(sources.len());
        for source in &sources {
            let pin = service
                .pins
                .iter_mut()
                .find(|pin| pin.dbnum == source.dbnum())
                .with_context(|| format!("current MDB has no DESI dbnum {}", source.dbnum()))?;
            anyhow::ensure!(
                pin.db_type.eq_ignore_ascii_case(source.db_type()),
                "frozen source type {} differs from MDB pin {}",
                source.db_type(),
                pin.db_type
            );
            pin.file = source.path().to_path_buf();
            pin.sesno = Some(source.target_sesno());
            tokens.push(source.commit_token_json()?);
        }
        tokens.sort_unstable();
        service.generation_identity =
            Some(hex::encode(Sha256::digest(tokens.join("\n").as_bytes())));
        Ok(service)
    }

    /// The DESI database that owns a batch of generation roots.
    ///
    /// Resolved from the MDB's DESI files by index lookup (ADR-054 constraint 3),
    /// not from `pe`: a never-parsed project has no `pe` rows at all.
    pub async fn dbnum_for_roots(roots: &[String]) -> anyhow::Result<u32> {
        let root = roots.first().context("generation roots are empty")?;
        let refno = parse_refno(root)?;
        tokio::task::spawn_blocking(move || model_source::dbnum_of_root(refno))
            .await
            .map_err(|error| anyhow::anyhow!("resolve root dbnum task failed: {error}"))?
            .with_context(|| format!("root {root} has no source database in the current MDB"))
    }

    pub async fn generate_roots(
        &self,
        dbnum: u32,
        roots: &[String],
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<E3dPersistReport> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        let pin = self.pin(dbnum)?;
        let source_sesno = self.source_session(pin)?;
        ensure_not_older_than_persisted(dbnum, source_sesno).await?;
        let input = generation_input(self, dbnum, source_sesno)?;
        let roots = roots
            .iter()
            .map(|value| parse_refno(value))
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.generate_refs(
            dbnum,
            source_sesno,
            &input.set,
            &input.index,
            &roots,
            failure_policy,
            &input.dependency_sessions,
            None,
        )
        .await
    }

    /// Producer half of the Core.dll-style split: read the source, evaluate the
    /// geometry, and hand the consumer one **bounded** slice at a time.  It
    /// performs no model writes.
    ///
    /// A slice is sealed as soon as it reaches either budget:
    /// `model_regen_execution_group` roots (the publication cohort width, which
    /// is what decided transaction boundaries before) or
    /// `model_memory_slice_elements` generated elements.  Root count alone is
    /// not a memory budget — a BRAN and an EQUI differ by two orders of
    /// magnitude, and what sits in the slice is `GeneratedElement`, the fattest
    /// representation on the chain.  A root is never split across slices: it is
    /// the atomic publication unit.
    ///
    /// Generation runs continuously: `min(quota, claimable)` resident workers
    /// keep claiming roots off a shared queue, and may run up to
    /// `model_regen_execution_group` roots ahead of the last one folded into a
    /// slice.  That run-ahead is what decouples generation from publication —
    /// the workers do not stop at every cohort boundary waiting for the slowest
    /// root in it — and it is also the memory bound: the peak is one window of
    /// generated roots plus the partial slice.  Under parallel workers the
    /// element budget can only seal *publication* slices, not cap the in-flight
    /// set: N workers means N roots being built no matter what the budget says.
    /// Projects whose roots are unusually fat should lower
    /// `model_regen_execution_group`, which narrows the window and the cohort
    /// together.
    ///
    /// The producer takes no `db_generation_lock`: reading the source file and
    /// evaluating geometry touches neither the projection store nor the root
    /// receipts that lock protects.  What each slice carries out of here is the
    /// pinned `source_sesno`; the consumer re-checks it under the lock before it
    /// commits anything.
    pub(crate) async fn produce_slices(
        &self,
        dbnum: u32,
        roots: &[String],
        failure_policy: GeometryFailurePolicy,
        sink: &tokio::sync::mpsc::Sender<GeneratedMemoryBatch>,
        rooms: Option<Arc<crate::fast_model::room_model::MemoryRoomCalculator>>,
    ) -> anyhow::Result<()> {
        let pin = self.pin(dbnum)?;
        let source_sesno = self.source_session(pin)?;
        let input = generation_input(self, dbnum, source_sesno)?;
        let roots = roots
            .iter()
            .map(|value| parse_refno(value))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let root_budget = crate::options::model_regen_execution_group().max(1);
        let element_budget = crate::options::model_memory_slice_elements();
        // A production run is one catalogue reuse window: identical evaluated
        // primitives across different delivery roots must share their local
        // mesh.  One cache per worker rather than one shared behind a mutex —
        // evaluating inside that mutex is the convoy ADR-052 opens with — and
        // the pool outlives the waves so the window is the whole run.
        let caches = CachePool::default();
        let queue = Arc::new(RootQueue::new(roots, root_budget));
        let mut workers = futures::stream::FuturesUnordered::new();
        let mut slice = MemorySlice::default();
        loop {
            // Top the pool back up: a worker that hit the window returned its
            // permit rather than sit on it, so it has to be re-launched once
            // the assembler slid the window forward.
            let wanted = crate::fast_model::concurrency::resident_workers(queue.claimable());
            while workers.len() < wanted {
                workers.push(spawn_generation_worker(
                    input.set.clone(),
                    input.index.clone(),
                    queue.clone(),
                    caches.clone(),
                    self.mesh_dir.clone(),
                    rooms.clone(),
                ));
            }
            let Some(finished) = futures::StreamExt::next(&mut workers).await else {
                break;
            };
            finished?;
            while let Some((root, outcome)) = queue.take_ready() {
                // A root that cannot be read or evaluated is one dead letter,
                // not a reason to drop the whole page: a single permanently
                // broken component would otherwise wedge every batch that
                // contains it.  The verdict stays sequential and in root order,
                // so `geometry_workers = 1` reproduces the old run exactly.
                match outcome {
                    Ok(snapshot) => {
                        if !snapshot.report.failed.is_empty()
                            && matches!(failure_policy, GeometryFailurePolicy::Required)
                        {
                            bail!(
                                "e3d-model root {root} has {} failed element(s)",
                                snapshot.report.failed.len()
                            );
                        }
                        slice.push(root, snapshot);
                    }
                    Err(error) if !matches!(failure_policy, GeometryFailurePolicy::Required) => {
                        log::error!("e3d-model root {root} failed: {error:#}");
                        slice.failures.push((root, format!("{error:#}")));
                    }
                    Err(error) => {
                        return Err(classify_required_root_error(root, error))
                            .with_context(|| format!("generate root {root}"));
                    }
                }
                if slice.is_full(root_budget, element_budget)
                    && !slice.deliver(dbnum, source_sesno, &input, sink).await
                {
                    return Ok(());
                }
            }
        }
        slice.deliver(dbnum, source_sesno, &input, sink).await;
        Ok(())
    }

    /// Consume an in-memory batch without re-reading or re-generating any
    /// root.  Only this phase commits into the projection store.
    pub async fn persist_memory_batch(
        &self,
        mut batch: GeneratedMemoryBatch,
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<E3dPersistReport> {
        let _generation_guard = db_generation_lock(batch.dbnum).lock_owned().await;
        ensure_not_older_than_persisted(batch.dbnum, batch.source_sesno).await?;
        let chunk_size = crate::options::model_regen_execution_group().max(1);
        let snapshots = std::mem::take(&mut batch.snapshots);
        let mut total = E3dPersistReport::default();
        // Producer-side dead letters are failures of this publication too: the
        // roots they name keep their previously published geometry, and the
        // caller must still see a non-zero `failed`.
        for (root, error) in std::mem::take(&mut batch.failures) {
            log::error!("e3d-model root {root} was not generated: {error}");
            total.failed += 1;
            total.failed_roots.push((root, error));
        }
        let mut snapshots = snapshots;
        while !snapshots.is_empty() {
            let take = snapshots.len().min(chunk_size);
            let mut memory_chunk = snapshots.drain(..take).collect::<Vec<_>>();
            let roots = memory_chunk
                .iter()
                .map(|(root, _)| *root)
                .collect::<Vec<_>>();
            let report = self
                .generate_refs(
                    batch.dbnum,
                    batch.source_sesno,
                    &batch.set,
                    &batch.index,
                    &roots,
                    failure_policy,
                    &batch.dependency_sessions,
                    Some(&mut memory_chunk),
                )
                .await?;
            total.merge_counts(report);
        }
        Ok(total)
    }

    /// Generate `roots` and publish them slice by slice, producer and consumer
    /// running concurrently across a **depth-1 bounded** channel (ADR-041 §8).
    ///
    /// The channel does not make the write side faster — publication is
    /// serial by construction — it moves the producer's wait from "the whole
    /// page is published" to "there is room for one more slice", which is the
    /// `(P + C) × N → max(P, C) × N` the ADR costs out.  Its depth is the whole
    /// point: an unbounded channel would not block either, at the price of
    /// unbounded memory, and that is the trade the ADR rejected.  At most three
    /// slices are alive at once — one being published, one buffered, one under
    /// construction — instead of the entire page.
    pub async fn generate_frozen_roots(
        &self,
        dbnum: u32,
        roots: &[String],
        sink: tokio::sync::mpsc::Sender<FrozenGeneration>,
    ) -> anyhow::Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<GeneratedMemoryBatch>(1);
        let produce = async {
            let result = self
                .produce_slices(
                    dbnum,
                    roots,
                    GeometryFailurePolicy::BestEffortFallback,
                    &tx,
                    None,
                )
                .await;
            drop(tx);
            result
        };
        let freeze = async move {
            while let Some(batch) = rx.recv().await {
                for (root, error) in batch.failures {
                    sink.send(FrozenGeneration {
                        root,
                        outcome: Err(error),
                    })
                    .await
                    .map_err(|_| anyhow::anyhow!("initialization journal receiver closed"))?;
                }
                for (root, snapshot) in batch.snapshots {
                    let outcome = (|| -> anyhow::Result<ProjectionCommit> {
                        anyhow::ensure!(
                            snapshot.report.failed.is_empty(),
                            "root {root} has {} failed geometry elements: {}",
                            snapshot.report.failed.len(),
                            summarize_geometry_incidents(&snapshot.report.failed)
                        );
                        let publication = RootPublication {
                            root,
                            model_target: Some(self.model_target(
                                dbnum,
                                batch.source_sesno,
                                &batch.dependency_sessions,
                            )?),
                            published: None,
                            projection_complete: false,
                        };
                        let (projection, _) = project_geometry_delta(
                            dbnum,
                            batch.source_sesno,
                            snapshot.elements,
                            Vec::new(),
                            Some(&publication),
                            &batch.index.owners,
                        )?;
                        // Mesh files are content addressed. Flush dependencies before
                        // the queue may acknowledge this envelope after a crash.
                        for mesh in projection
                            .upserts
                            .iter()
                            .map(|g| g.mesh_id.as_str())
                            .collect::<BTreeSet<_>>()
                        {
                            let path = self.mesh_dir.join(format!("{mesh}.mesh"));
                            std::fs::OpenOptions::new()
                                .read(true)
                                .write(true)
                                .open(&path)
                                .with_context(|| {
                                    format!("missing handed-off mesh {}", path.display())
                                })?
                                .sync_all()?;
                        }
                        Ok(projection)
                    })()
                    .map_err(|e| format!("{e:#}"));
                    sink.send(FrozenGeneration { root, outcome })
                        .await
                        .map_err(|_| anyhow::anyhow!("initialization journal receiver closed"))?;
                }
            }
            Ok::<_, anyhow::Error>(())
        };
        futures::future::try_join(produce, freeze).await?;
        Ok(())
    }

    /// `rooms` 是本任务开工时冻住的那台房间计算器（一任务一台，ADR-073 D1 / D12，B/T1）：给了，每件几何的
    /// 房间归属就在 bake 时算出来随根走（[`BakedElement::room_edges`]，B/T2）；`None` 则房间那一半照旧由
    /// flush 时的 `room_stage` 现算。
    pub async fn generate_and_persist_roots(
        &self,
        dbnum: u32,
        roots: &[String],
        failure_policy: GeometryFailurePolicy,
        rooms: Option<Arc<crate::fast_model::room_model::MemoryRoomCalculator>>,
    ) -> anyhow::Result<E3dPersistReport> {
        crate::native_runtime::ensure_geometry_runtime()?;
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<GeneratedMemoryBatch>(1);
        let produce = async move {
            let outcome = self
                .produce_slices(dbnum, roots, failure_policy, &sender, rooms)
                .await;
            // Closing the last sender is what ends the consumer's loop.
            drop(sender);
            outcome
        };
        let consume = async move {
            let mut total = E3dPersistReport::default();
            while let Some(slice) = receiver.recv().await {
                match self.persist_memory_batch(slice, failure_policy).await {
                    Ok(report) => total.merge_counts(report),
                    Err(error) => {
                        // Stop the producer at its next hand-off rather than
                        // let it keep filling memory for a consumer that is
                        // already done; its error is the one worth reporting.
                        receiver.close();
                        return Err(error);
                    }
                }
            }
            Ok(total)
        };
        let (produced, consumed) = futures::future::join(produce, consume).await;
        let total = consumed?;
        produced?;
        Ok(total)
    }

    /// 初始化直写（ADR-077 D4 / D5）的**投影半边**：把生产端交出的一片切片逐根折成
    /// [`ProjectionCommit`]（带回执、无 removals），**不碰** `ModelMemoryStore`、不登记 write-behind、
    /// 不读任何回执——初始化前库里没有旧行，也没有上一轮可比。渲成 SQL、交模型 lane 是
    /// `model_initial_persist` 与 `dbnum_initialize` 的事。
    ///
    /// 死信口径：生产端硬错 = 该根死信，不拦同片的其他根（`BestEffortFallback`）；`Required` 才整片报错。
    /// 有失败元素的根**不是死信**（GitHub #74，2026-10-27）：初始化是冷根，按 `incomplete_root_policy` 的 `Publish`
    /// 判决发布幸存件、把没算出来的那几件记进 [`InitialSliceProjection::partial`]——与 [`Self::persist_roots_with`]
    /// 对冷根的处置同一口径。
    pub(crate) fn project_slice_for_initialization(
        &self,
        batch: GeneratedMemoryBatch,
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<InitialSliceProjection> {
        let GeneratedMemoryBatch {
            dbnum,
            source_sesno,
            index,
            dependency_sessions,
            snapshots,
            failures,
            ..
        } = batch;
        // 目标里带着每个 CATA 依赖的文件身份；这一片生成完之后算一次、全片共用。
        let model_target = self.model_target(dbnum, source_sesno, &dependency_sessions)?;
        let mut projection = InitialSliceProjection {
            dbnum,
            source_sesno,
            commits: Vec::with_capacity(snapshots.len()),
            failures,
            partial: Vec::new(),
            visited_by_root: HashMap::new(),
        };
        for (root, snapshot) in snapshots {
            let root_failed = snapshot.report.failed.len();
            if root_failed > 0 {
                if matches!(failure_policy, GeometryFailurePolicy::Required) {
                    bail!("e3d-model root {root} has {root_failed} failed element(s)");
                }
                // GitHub #74：初始化是冷根（没有回执、没有旧行），`incomplete_root_policy(false, false)` 的判决是
                // `Publish`——幸存件照发，没算出来的那几件记在 `partial` 里点名。此前这里把整根当死信丢掉，
                // 一只零长 SCTN 名下的 SJOI 就让同框好杆件一并消失（ams7330 四根 → 整库 lagging）。
                let failure = crate::data_interface::mdb_route_directory::explain_unregistered_dbs(
                    &format!(
                        "{root_failed} geometry element(s) failed: {:?}",
                        snapshot.report.failed
                    ),
                );
                log::warn!(
                    "e3d-model root {root} publishing {} element(s) of an incomplete candidate at initialization; {root_failed} did not evaluate: {failure}",
                    snapshot.elements.len()
                );
                projection.partial.push((root, failure));
            }
            let publication = RootPublication {
                root,
                model_target: Some(model_target.clone()),
                published: None,
                projection_complete: false,
            };
            let point_sets = snapshot.point_sets;
            let (mut commit, mut report) = project_geometry_delta(
                dbnum,
                source_sesno,
                snapshot.elements,
                Vec::new(),
                Some(&publication),
                &index.owners,
            )?;
            // 初始切片与整根重生成同一条规矩：P 点行随根落（ptset 分表计划 W2）。
            commit.point_sets = Some(crate::fast_model::ptset_rows::project_point_sets(
                root, point_sets,
            ));
            report.skipped += snapshot.report.skipped.len();
            report.failed += root_failed;
            report.note_lod1(snapshot.lod1);
            report.generation_report = serde_json::to_value(&snapshot.report)?;
            projection.visited_by_root.insert(root, snapshot.visited);
            projection.commits.push((root, commit, report));
        }
        Ok(projection)
    }

    /// 刚体前移执行体（ADR-066 B6）的**存储半边**：把「只动摆放」的根从整根重算改成「投影记录左乘
    /// Δ + 前移回执到 T」，只左乘并提交进内存投影——**生成服务不碰数据库**（登记 / durable flush 由
    /// 调用方在 batch_worker 里做，见 `run_rigid_motion_units`）。**保守优先**：任一前置不满足就把这根
    /// 放进 `fallbacks` 退回整根重算，回退永远正确。盒按 D3：纯平移直接加 `t`（`republish_records` 内部走
    /// `translate_bounds`）；含旋转读回记录自己的网格、顶点乘到新世界系再取紧 AABB
    /// （[`rigid_motion_mesh_bounds`]），网格读不到 / 没有顶点就回退。`completed` 的根已把左乘后的
    /// 记录与前移到 T 的回执提交进投影，等调用方登记后由 `publish_completed_file_round` 统一刷 durable。
    /// 开关 `model_transform_cheap_path` 关着时规划器不产 transform 单元，本函数不会被调到。
    ///
    /// 幂等（S4b 审核 P0）：transform 不像 regen 天然幂等——动记录之前先过 [`republish_gate`]：
    /// 回执已在 T（发布成功但回合没收尾的窗口重放）跳过、按已完成计（`replayed`）；回执不在规划
    /// 基线 S 上（半截状态 / 旧计划 / 别的路径动过它）`Δ` 对不上，回退整根重算。
    ///
    /// 落款按 D4「与重生成同款」：回执的 `model_target` 是按 pin@T 现算的（与 regen / 单元级同一条
    /// `hydrate_published_dependencies → model_target` 路，目录会话沿旧回执补水），且只在设计窗之外
    /// 每一格都没变时才落（[`rigid_motion_target_at`]）——目录 / 模板 / 生成器 / 细分档任一变了，左乘
    /// 一个 `Δ` 覆盖不了，回退整根重算。于是下一窗 `generation_root_cache_current` 看到的就是一根
    /// 在 T 上完成的根，与一次 regen 落地后一模一样。
    pub(crate) async fn republish_roots_by_rigid_motion(
        &self,
        dbnum: u32,
        target_sesno: u32,
        units: &[RigidMotionUnit],
    ) -> anyhow::Result<RigidMotionReport> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        ensure_not_older_than_persisted(dbnum, target_sesno).await?;
        run_blocking(|| {
            self.republish_roots_by_rigid_motion_on(
                ModelMemoryStore::global(),
                dbnum,
                target_sesno,
                units,
                |commits| {
                    super::model_write_behind::publish(commits)?;
                    Ok(())
                },
            )
        })
    }

    /// [`Self::republish_roots_by_rigid_motion`] 的无锁本体：投影与发布口由调用方注入——生产给全局投影 +
    /// write-behind，门 3 探针（`probe_rigid_motion_gate3_for_window`）给本地投影 + 直接提交，两边跑的是
    /// 同一段闸 → 目标核对 → 左乘 → 回执。与单元级的 `republish_roots_by_unit_delta_on` 同形；同步 CPU +
    /// 文件 IO，调用方负责在 [`run_blocking`] 里跑。
    pub(crate) fn republish_roots_by_rigid_motion_on(
        &self,
        store: &ModelMemoryStore,
        dbnum: u32,
        target_sesno: u32,
        units: &[RigidMotionUnit],
        mut publish: impl FnMut(Vec<ProjectionCommit>) -> anyhow::Result<()>,
    ) -> anyhow::Result<RigidMotionReport> {
        let mesh_dir = self.mesh_dir.as_path();
        let mut report = RigidMotionReport::default();
        // 目录会话表整轮共用（与单元级 `produce` 用窗口的 `dependency_sessions` 同形）：
        // `hydrate_published_dependencies` 已有的键不重开文件。
        let dependency_sessions: DependencySessions =
            Arc::new(std::sync::Mutex::new(BTreeMap::new()));
        for unit in units {
            let root_refno = &unit.root_refno;
            let anchors = unit.anchors.as_slice();
            let root = match parse_refno(root_refno) {
                Ok(root) => root,
                Err(error) => {
                    report
                        .fallbacks
                        .push((root_refno.clone(), format!("unparsable root: {error:#}")));
                    continue;
                }
            };
            // 前置：投影完整（记录数与回执相符、mesh 文件都在）且有旧回执可前移。
            if !store.root_projection_complete(ProjectionScope::Current, root, mesh_dir) {
                report
                    .fallbacks
                    .push((root_refno.clone(), "projection incomplete".into()));
                continue;
            }
            let Some(old_receipt) = store.receipt(ProjectionScope::Current, root) else {
                report
                    .fallbacks
                    .push((root_refno.clone(), "no cached receipt".into()));
                continue;
            };
            // 幂等闸（S4b 审核 P0 / mem-42；ADR-089 D7）：只有「投影还停在规划基线 S 或 S 之前」才许左乘。
            match republish_gate(
                old_receipt.source_sesno,
                unit.source_start_sesno,
                target_sesno,
            ) {
                RepublishGate::Proceed => {}
                RepublishGate::SkipAlreadyAtTarget => {
                    report.replayed.push(root_refno.clone());
                    continue;
                }
                RepublishGate::FallbackBaselineMismatch => {
                    report.fallbacks.push((
                        root_refno.clone(),
                        format!(
                            "projection receipt at sesno {} is newer than the baseline {} Δ was \
                             planned against (target {}); falling back to regen",
                            old_receipt.source_sesno, unit.source_start_sesno, target_sesno
                        ),
                    ));
                    continue;
                }
            }
            // 落款先算、先核（便宜，且不动任何记录）：pin@T 的目标与旧回执在设计窗之外逐格相同才往下走。
            let model_target = match self
                .hydrate_published_dependencies(&old_receipt, &dependency_sessions)
                .and_then(|()| self.model_target(dbnum, target_sesno, &dependency_sessions))
                .and_then(|current| rigid_motion_target_at(&old_receipt, current, target_sesno))
            {
                Ok(target) => target,
                Err(error) => {
                    report
                        .fallbacks
                        .push((root_refno.clone(), format!("model target: {error:#}")));
                    continue;
                }
            };
            let records = store.records_of_root(ProjectionScope::Current, root);
            // 左乘 Δ：纯平移直接加 t；含旋转读回记录自己的网格重算盒（D3），读不到就回退。已在 T 的记录
            // （嵌套根共有、同窗别的根落的、窗口重放）按记录的闸放过、不再乘（S4b §9 B8 首跑那条红）。
            let moved = match republish_records(
                records,
                anchors,
                unit.source_start_sesno,
                target_sesno,
                |record, world| rigid_motion_mesh_bounds(mesh_dir, record, world),
            ) {
                Ok(moved) => moved,
                Err(error) => {
                    report
                        .fallbacks
                        .push((root_refno.clone(), format!("{error:#}")));
                    continue;
                }
            };
            let (manifest_digest, geometry_count) = match manifest_hash(&moved) {
                Ok(pair) => pair,
                Err(error) => {
                    report
                        .fallbacks
                        .push((root_refno.clone(), format!("manifest hash: {error:#}")));
                    continue;
                }
            };
            // 回执整根前移到 T，落款是 pin@T 的目标（D4）。生产的发布口只同步
            // 提交内存投影并登记 write-behind；后台 worker 独立完成 durable flush。
            publish(vec![ProjectionCommit {
                upserts: moved,
                receipt: Some((
                    root,
                    RootReceipt {
                        source_sesno: target_sesno,
                        model_target: Some(model_target),
                        manifest_hash: manifest_digest,
                        geometry_count,
                        published_via: "transform".into(),
                        room: None,
                    },
                )),
                ..ProjectionCommit::default()
            }])?;
            report.completed.push(root_refno.clone());
        }
        Ok(report)
    }

    /// 单元级发布执行体（ADR-080 D4 / D5 / D8）：每一根**先过闸，再要局部产出，再以 `Partial` 提交发布**。
    ///
    /// 顺序钉死为 闸 → 产出 → 提交（实施约束 5，与 [`Self::republish_roots_by_rigid_motion`] 同序）。
    /// 局部清单不像 regen 天然幂等——它只对「投影还停在规划基线 S」的根成立：回执已在 T 是窗口重放
    /// （发布成功但回合没收尾就崩），再叠一次就是把 T 的记录当 S 的去删 / 改；回执既不在 S 也不在 T 是
    /// 半截状态或别的路径动过它，`plan_update(S→T)` 算出的清单对不上记录。两者都在**读任何记录、
    /// 求任何几何之前**判掉：前者跳过、按已完成计（`replayed`），后者回退整根 regen（`fallbacks`）。
    ///
    /// `produce` 给出这根的局部产出——接上 `execute_plan` 之前由调用方提供，接上之后就是它；报错即
    /// 回退（D8：任何一件失败 → 整根 regen，记原因，不静默）。回执的 `manifest_hash` / `geometry_count`
    /// 由存储在写锁里盖（D5），这里只填 `source_sesno = T` / `model_target` / `published_via = "unit"`。
    /// 与刚体路同一把 `db_generation_lock`、同一道 `ensure_not_older_than_persisted`（D4：不新增锁）。
    ///
    /// 执行体与 `produce`（`plan_update` / `execute_plan` / 烘焙）都是同步的 CPU 活，在 [`run_blocking`]
    /// 里跑：多线程运行时让出当前 tokio 工作线程，锁照握着（同一把 `db_generation_lock` 下只有这一个
    /// 写者，让出线程不放锁）。生产调用点是 `batch_worker::run_unit_publish_units`（开关
    /// `model_unit_level_publish`，D9，2026-10-14 起默认开；2026-09-16 首版时为关）。
    pub(crate) async fn republish_roots_by_unit_delta(
        &self,
        dbnum: u32,
        target_sesno: u32,
        units: &[UnitPublishUnit],
        produce: impl FnMut(&UnitPublishUnit, RefNo, &RootReceipt) -> anyhow::Result<UnitDelta>,
    ) -> anyhow::Result<UnitPublishReport> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        ensure_not_older_than_persisted(dbnum, target_sesno).await?;
        run_blocking(|| {
            republish_roots_by_unit_delta_on(
                ModelMemoryStore::global(),
                self.mesh_dir.as_path(),
                target_sesno,
                units,
                produce,
                |commits| {
                    super::model_write_behind::publish(commits)?;
                    Ok(())
                },
            )
        })
    }

    /// [`Self::republish_roots_by_unit_delta`] 接上真生产方（[`UnitDeltaProducer`]）：一窗 S→T 的
    /// `plan_update` 算一次，逐根切子计划执行。每个 unit 的 `source_start_sesno` 必须就是这一窗的 S——
    /// 闸对回执用的基线与生产方切计划用的基线要是同一个，否则闸放行的记录与 delta 对的不是同一版。
    ///
    /// 生产方**惰性**打开：第一根过了闸才 `open`（两端 `DbSet` + `plan_update`，同步 CPU 活，随执行体一起
    /// 在 [`run_blocking`] 里跑）——整窗都是重放 / 回退时一份计划都不算；打开失败的原因记一次、之后每根
    /// 原样回退（D8），不反复开。生产调用点是 `batch_worker::run_unit_publish_units`（D9）；真库探针
    /// `probe_unit_publish_one_root_against_regen` 走它的无锁本体。
    pub(crate) async fn republish_roots_by_unit_delta_from_window(
        &self,
        dbnum: u32,
        base_sesno: u32,
        target_sesno: u32,
        units: &[UnitPublishUnit],
    ) -> anyhow::Result<UnitPublishReport> {
        let mut producer: Option<Result<UnitDeltaProducer<'_>, String>> = None;
        self.republish_roots_by_unit_delta(dbnum, target_sesno, units, |unit, root, old| {
            anyhow::ensure!(
                unit.source_start_sesno == base_sesno,
                "root {root} was planned against baseline {} but the window producer is S = {base_sesno}",
                unit.source_start_sesno
            );
            let producer = producer.get_or_insert_with(|| {
                UnitDeltaProducer::open(self, dbnum, base_sesno, target_sesno)
                    .map_err(|error| format!("{error:#}"))
            });
            match producer {
                Ok(producer) => producer.produce(root, old, unit.no_model_only),
                Err(reason) => bail!("window producer {base_sesno}→{target_sesno} failed to open: {reason}"),
            }
        })
        .await
    }

    /// 这个库当前会话下的全部生成根：**每个 ZONE 一根**（文件侧枚举，一行库都不读）。
    ///
    /// 整库重生成用它把根取齐，再交给 [`Self::generate_and_persist_roots`]。根只到 ZONE，
    /// **不上溯 SITE**（2026-09-14 定）：SITE 只拥有 ZONE，几何都在某个 ZONE 之下，上溯只会造出
    /// 一个罩住别的 ZONE 根的 SITE 根——它那一片的整片删会把邻居刚落一半的行抹掉。文件顶层
    /// （WORL）曾是这里的根：7997 上一个 WORL 装着全部 52,373 件，生成并行度为 0、渲成 66 MB
    /// 撞事务硬顶；按 ZONE 取 142 根之后生成快 1.5–1.9×（几何闸利用率 6% → 39%），最大一根只占
    /// 10%，回读逐字相同（`docs/plans/2026-09-14-zone-partitioned-model-persistence-dev-plan.md`）。
    /// 想量文件顶层那一档，用 [`Self::top_level_roots_of`]。
    ///
    /// 库里一个 ZONE 都没有（不是 DESI 库、或空设计库）按错误上抛，不静默生成零根。
    /// 此前并排有一个 `generate_dbnum` 自己取根再直落 [`Self::generate_refs`]，那是逐根串行、
    /// 不过几何并发闸的老形态；生产（`model_refresh`）从来不走它，只有烟测在用。已退役。
    pub async fn generation_roots_of(&self, dbnum: u32) -> anyhow::Result<Vec<String>> {
        let pin = self.pin(dbnum)?;
        let source_sesno = self.source_session(pin)?;
        ensure_not_older_than_persisted(dbnum, source_sesno).await?;
        let roots = self
            .elements_with_noun(dbnum, "ZONE")?
            .into_iter()
            .map(|root| format!("{}/{}", root.word0, root.word1))
            .collect::<Vec<_>>();
        anyhow::ensure!(
            !roots.is_empty(),
            "dbnum {dbnum} 在会话 {source_sesno} 里没有 ZONE 元素，按 ZONE 分组无根可取（不上溯 SITE）"
        );
        Ok(roots)
    }

    /// 给已有库补显示用粗网格（ADR-096，`POST /api/v1/mesh-lod1/backfill`）。
    ///
    /// 交付单元根按初始化同一条路从文件枚举（[`Self::initialization_roots`]）；每根在几何闸下 L0
    /// **只求值、只算身份**——不写 L0、不算房间、不碰库——再按盘分拣，只对既没有粗网格也没有标记的键
    /// 粗精度重求（`mesh_lod1::backfill_lod1_root`）。开着 `AIOS_MESH_LOD1` 的生成会顺手产粗网格，
    /// 这条路是给开关之前就生成好的库补课用的。一根失败只记账、不中断；`progress(done, total)`
    /// 每根回调一次。
    pub async fn backfill_lod1(
        &self,
        dbnum: u32,
        progress: &(dyn Fn(usize, usize) + Sync),
    ) -> anyhow::Result<crate::fast_model::mesh_lod1::Lod1BackfillReport> {
        let pin = self.pin(dbnum)?;
        let source_sesno = self.source_session(pin)?;
        let input = generation_input(self, dbnum, source_sesno)?;
        let roots = self.initialization_roots(dbnum)?;
        let total = roots.len();
        progress(0, total);
        let mut report = crate::fast_model::mesh_lod1::Lod1BackfillReport {
            dbnums: vec![dbnum],
            roots: total,
            ..Default::default()
        };
        // 两份目录网格缓存精度不同，必须分开；随根在几何闸的闭包里进出。
        let mut l0_cache = CatalogueMeshCache::default();
        let mut l1_cache = CatalogueMeshCache::default();
        for (done, generation_root) in roots.iter().enumerate() {
            let root = crate::data_interface::generation_root::refno_to_e3d(generation_root.root);
            let set = Arc::clone(&input.set);
            let mesh_dir = self.mesh_dir.clone();
            let (returned_l0, returned_l1, outcome) =
                crate::fast_model::concurrency::run_gated_cpu(move || {
                    let outcome = crate::fast_model::mesh_lod1::backfill_lod1_root(
                        &set,
                        root,
                        &mesh_dir,
                        &mut l0_cache,
                        &mut l1_cache,
                    );
                    Ok((l0_cache, l1_cache, outcome))
                })
                .await?;
            l0_cache = returned_l0;
            l1_cache = returned_l1;
            match outcome {
                Ok(root_report) => report.absorb_root(root_report),
                Err(error) => {
                    report.roots_failed += 1;
                    log::warn!("[mesh-lod1] backfill dbnum={dbnum} root={root} 失败: {error:#}");
                }
            }
            progress(done + 1, total);
        }
        log::info!(
            "[mesh-lod1] backfill dbnum={dbnum} roots={} roots_failed={} unpublished={} identity_failed={} {}",
            report.roots,
            report.roots_failed,
            report.unpublished,
            report.identity_failed,
            report.bake
        );
        Ok(report)
    }

    /// 初始化（ADR-077 D6）的交付单元根：**从文件枚举**，一行库都不读。
    ///
    /// 与 `model_dbnum_ensure::capture_snapshot` 同一条路（`enumerate_generation_roots` 从文件顶层
    /// 元素往下走、按配置的交付单元类型收根），只是 `DbSet` / 索引复用本服务的生成输入缓存——随后
    /// `produce_slices` 读的就是同一份。此前 `manual_update::load_baseline_nodes` 分页读 `pe` 表来
    /// 枚举，那是初始化里属性面与模型面唯一的串行点。
    pub(crate) fn initialization_roots(
        &self,
        dbnum: u32,
    ) -> anyhow::Result<Vec<crate::data_interface::generation_root::GenerationRoot>> {
        let pin = self.pin(dbnum)?;
        let source_sesno = self.source_session(pin)?;
        let input = generation_input(self, dbnum, source_sesno)?;
        let mut roots = crate::data_interface::generation_root::enumerate_generation_roots(
            &input.set,
            &input.index.roots,
            &crate::data_interface::generation_root::configured_delivery_unit_types(),
        )?;
        // 共享遍历本就唯一；留这道护栏，畸形成员图不至于让任务总数与真正生成的集合对不上。
        let mut seen = std::collections::HashSet::new();
        roots.retain(|root| seen.insert(root.root.to_pdms_str()));
        Ok(roots)
    }

    /// 这个库当前会话的文件顶层元素（owner 缺席 / 指向自身 / 不在索引里——通常就是 WORL，
    /// 外加 MNUM / DBMETA 这类簿记根）。
    ///
    /// 这是 [`Self::generation_roots_of`] 2026-09-14 之前的取根口径，只留给烟测对照
    /// （`e3d_persist_smoke --roots file`）：一个 WORL 一根意味着整库一笔生成、零并行。
    pub async fn top_level_roots_of(&self, dbnum: u32) -> anyhow::Result<Vec<String>> {
        let pin = self.pin(dbnum)?;
        let source_sesno = self.source_session(pin)?;
        ensure_not_older_than_persisted(dbnum, source_sesno).await?;
        let input = generation_input(self, dbnum, source_sesno)?;
        Ok(input
            .index
            .roots
            .iter()
            .map(|root| format!("{}/{}", root.word0, root.word1))
            .collect())
    }

    /// 当前源会话里 noun 为 `noun` 的全部元素，按文件扫描序。
    ///
    /// 只读文件头（`ScanTier::Header`），按 noun hash 比对，不为每个元素反查名字——整库 17 万个
    /// 元素扫一遍只为挑出一百多个 ZONE。[`Self::generation_roots_of`] 与烟测的 `--roots site`
    /// 共用这一段。
    pub fn elements_with_noun(&self, dbnum: u32, noun: &str) -> anyhow::Result<Vec<RefNo>> {
        let pin = self.pin(dbnum)?;
        let source_sesno = self.source_session(pin)?;
        let wanted = e3d_attlib::db1_hash(noun);
        let mut engine = ReadOnlyEngine::open_at(&pin.file, source_sesno)?;
        let mut picked = Vec::new();
        for element in engine.scan_elements(ScanTier::Header)? {
            let element = element?;
            if element.noun_hash == wanted {
                picked.push(element.refno);
            }
        }
        Ok(picked)
    }

    /// 解析侧的层级快照：这个库当前源会话里每个元素的 `(refno, owner, noun)`。
    ///
    /// 只读文件头（`ScanTier::Header`），不生成、不碰库。给 `e3d_persist_smoke --analyze` 做
    /// 「按哪一层分组落库」的数据分析用——投影记录只带祖先 refno 链，不带祖先的 noun，
    /// 要知道链上哪一个是 SITE / ZONE 只能回文件问。
    pub fn hierarchy_of(&self, dbnum: u32) -> anyhow::Result<Vec<(RefNo, RefNo, String)>> {
        let pin = self.pin(dbnum)?;
        let source_sesno = self.source_session(pin)?;
        let mut engine = ReadOnlyEngine::open_at(&pin.file, source_sesno)?;
        let mut elements = Vec::new();
        for element in engine.scan_elements(ScanTier::Header)? {
            let element = element?;
            let noun = e3d_attlib::db1_dehash(element.noun_hash)
                .unwrap_or_else(|| format!("0x{:08X}", element.noun_hash));
            elements.push((element.refno, element.owner, noun));
        }
        Ok(elements)
    }

    /// Generate one root at an exact source session without reading or writing
    /// the persisted model projection.
    ///
    /// A root that the session's element index does not contain fails **before**
    /// geometry evaluation with [`SnapshotSourceError::RootNotInSession`], so a
    /// caller can tell "this refno did not exist at that session" apart from
    /// "its geometry could not be generated" by type instead of by message text.
    pub async fn generate_snapshot_source(
        &self,
        dbnum: u32,
        root: RefNo,
        source_sesno: u32,
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<GeneratedSnapshot> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        let input = generation_input(self, dbnum, source_sesno)?;
        let computed =
            compute_snapshot_source(&input, dbnum, root, source_sesno, failure_policy, &NoHooks);
        match computed {
            Ok(snapshot) => Ok(snapshot),
            Err(RealtimeComputeError::Failed(error)) => Err(error),
            Err(RealtimeComputeError::Stopped { stop, visited }) => {
                unreachable!("空钩子从不叫停，却在访问 {visited} 个元素后收到 {stop:?}")
            }
        }
    }

    /// 实时历史几何的计算源（设计 §1.3）：与 [`Self::generate_snapshot_source`] 同一段求值
    /// （[`compute_snapshot_source`]），区别只有三处：
    ///
    /// - **不拿** `db_generation_lock`：纯读源文件、不写投影，没有要护的发布链；
    /// - 输入取自只给历史用的 [`HistoricalInputLru`]，不挤掉当前模型那份 `generation_input`；
    /// - DFS 带钩子（[`crate::fast_model::cancel::HistoryHooks`]）：取消或超过元素上限时中途返回
    ///   [`RealtimeComputeError::Stopped`]。
    ///
    /// 同步阻塞（冷输入要扫索引、建 `DbSet`，求值要三角化），调用方放在 `spawn_blocking` 里跑。
    pub fn realtime_compute_source(
        &self,
        dbnum: u32,
        root: RefNo,
        source_sesno: u32,
        failure_policy: GeometryFailurePolicy,
        hooks: &dyn GenerationHooks,
    ) -> Result<GeneratedSnapshot, RealtimeComputeError> {
        let input = historical_input(self, dbnum, source_sesno)?;
        compute_snapshot_source(&input, dbnum, root, source_sesno, failure_policy, hooks)
    }

    pub(crate) fn source_file(&self, dbnum: u32) -> anyhow::Result<&Path> {
        self.pins
            .iter()
            .find(|pin| pin.dbnum == dbnum as i32)
            .map(|pin| pin.file.as_path())
            .with_context(|| format!("MDB has no source file for dbnum {dbnum}"))
    }

    pub(crate) fn from_source_files(
        pins: Vec<DbPin>,
        locator: InMemoryCataLocator,
        mesh_dir: PathBuf,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            schema: DirectSchema::open_from_env()?,
            pins,
            locator,
            project_files: Arc::new(HashMap::new()),
            dependency_sessions: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            mesh_dir,
            generation_identity: None,
            frozen_file_digests: BTreeMap::new(),
            // 这条口子是拿显式文件清单建的，身边没有 MDB，也就没有字典库名单。
            uda: None,
        })
    }

    pub async fn apply_window(
        &self,
        dbnum: u32,
        base_sesno: u32,
        target_sesno: u32,
        failure_policy: GeometryFailurePolicy,
    ) -> anyhow::Result<E3dPersistReport> {
        let _generation_guard = db_generation_lock(dbnum).lock_owned().await;
        let pin = self.pin(dbnum)?;
        // The window's right end is an explicit timepoint (ADR-054 Q2): open the
        // file at exactly that session instead of demanding that the current pin
        // happens to equal it. A newer session already published for this
        // database means the window has been superseded, not that it failed.
        if let Some(newer) = persisted_session_newer_than(dbnum, target_sesno).await? {
            log::info!(
                "e3d-model window {base_sesno}..{target_sesno} for dbnum {dbnum} is already covered by published session {newer}; nothing to apply"
            );
            return Ok(E3dPersistReport::default());
        }
        let window = collect_window(&pin.file, base_sesno, target_sesno)?;
        let base = self.build_set(dbnum, Some(base_sesno))?;
        let target = self.build_set(dbnum, Some(target_sesno))?;
        let outcome = increment_update(&base, &target, &window);
        let failed = increment_failure_count(&outcome.report);
        let generation_report = serde_json::to_value(&outcome.report)?;
        if failed > 0 {
            if matches!(failure_policy, GeometryFailurePolicy::Required) {
                bail!("e3d-model incremental generation failed for {failed} element(s)");
            }
            // An incremental delta is only a candidate until every affected
            // element was evaluated. Publishing a partial delta could delete
            // valid geometry from the last complete snapshot.
            return Ok(E3dPersistReport {
                failed,
                generation_report,
                ..E3dPersistReport::default()
            });
        }
        let index = scan_index(&pin.file, Some(target_sesno))?;
        if let Some(newer) = persisted_session_newer_than(dbnum, target_sesno).await? {
            log::info!(
                "e3d-model window {base_sesno}..{target_sesno} for dbnum {dbnum} was superseded by published session {newer} during evaluation; discarding the delta"
            );
            return Ok(E3dPersistReport {
                generation_report,
                ..E3dPersistReport::default()
            });
        }
        let mut report = apply_geometry_delta(
            dbnum,
            target_sesno,
            outcome.upserts,
            outcome.removals,
            &index.owners,
            &self.mesh_dir,
        )
        .await?;
        report.failed += failed;
        report.generation_report = generation_report;
        Ok(report)
    }

    async fn generate_refs(
        &self,
        dbnum: u32,
        source_sesno: u32,
        set: &Arc<DbSet>,
        index: &SourceIndex,
        roots: &[RefNo],
        failure_policy: GeometryFailurePolicy,
        dependency_sessions: &DependencySessions,
        mut memory_snapshots: Option<&mut Vec<(RefNo, BakedSnapshot)>>,
    ) -> anyhow::Result<E3dPersistReport> {
        let store = ModelMemoryStore::global();
        let mut total = E3dPersistReport {
            source_sesno: Some(source_sesno),
            ..E3dPersistReport::default()
        };
        let mut generation = Report::default();
        let mut commits = Vec::new();
        let mut prepared_reports = Vec::new();
        // 上一轮的回执要先读齐、依赖会话要先补齐，本轮的目标才算得准——目标里带着每个
        // CATA 依赖的文件身份，逐根现算会让先算的那几根少看见后面才发现的依赖。
        let published = roots
            .iter()
            .copied()
            .map(|root| store.receipt(ProjectionScope::Current, root))
            .collect::<Vec<_>>();
        let cohort_was_empty = published.iter().all(Option::is_none);
        for receipt in published.iter().flatten() {
            self.hydrate_published_dependencies(receipt, dependency_sessions)?;
        }
        let model_target = self.model_target(dbnum, source_sesno, dependency_sessions)?;
        let publications = roots
            .iter()
            .copied()
            .zip(published)
            .map(|(root, published)| {
                let mut publication = RootPublication {
                    root,
                    model_target: Some(model_target.clone()),
                    published,
                    projection_complete: false,
                };
                publication.projection_complete =
                    cached_root_target_matches(&publication, source_sesno)
                        && store.root_projection_complete(
                            ProjectionScope::Current,
                            root,
                            &self.mesh_dir,
                        );
                publication
            })
            .collect::<Vec<_>>();
        let cached = publications
            .iter()
            .map(|publication| publication.projection_complete)
            .collect::<Vec<_>>();
        let prior = roots
            .iter()
            .copied()
            .zip(cached.iter().copied())
            .map(|(root, cached)| {
                if cached {
                    Vec::new()
                } else {
                    store.geometry_ids_of_root(ProjectionScope::Current, dbnum, root)
                }
            })
            .collect::<Vec<_>>();
        // A production batch is one reuse window: identical evaluated catalogue
        // primitives across different delivery roots must share their local mesh.
        let mut catalogue_mesh_cache = CatalogueMeshCache::default();
        let mut lod1_cache =
            crate::fast_model::mesh_lod1::lod1_enabled().then(CatalogueMeshCache::default);
        for (root_index, &root) in roots.iter().enumerate() {
            let mut publication = publications[root_index].clone();
            if cached[root_index] {
                // 目标与上一轮逐位相同，投影也还完整：只重盖一次章，几何一条都不动。
                commits.push(cached_root_commit(&publication, source_sesno)?);
                prepared_reports.push(E3dPersistReport::default());
                continue;
            }
            let incomplete = incomplete_root_policy(
                publication.published.is_some(),
                !prior[root_index].is_empty(),
            );
            let snapshot = if let Some(snapshots) = memory_snapshots.as_deref_mut() {
                let index = snapshots
                    .iter()
                    .position(|(candidate, _)| *candidate == root)
                    .with_context(|| format!("memory batch missing root {root}"))?;
                snapshots.swap_remove(index).1
            } else {
                match generate_snapshot_from_set(set, index, root, &mut catalogue_mesh_cache)
                    .and_then(|snapshot| bake_snapshot(snapshot, &self.mesh_dir, None))
                {
                    Ok(mut outcome) => {
                        if let Some(lod1_cache) = lod1_cache.as_mut() {
                            outcome.lod1 = crate::fast_model::mesh_lod1::bake_lod1_root(
                                set,
                                root,
                                outcome.elements.iter().map(Into::into),
                                &self.mesh_dir,
                                lod1_cache,
                            );
                        }
                        outcome
                    }
                    Err(error) if !matches!(failure_policy, GeometryFailurePolicy::Required) => {
                        total.failed += 1;
                        total.failed_roots.push((root, format!("{error:#}")));
                        log::error!("e3d-model root {root} failed: {error:#}");
                        continue;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| format!("generate root {root}"));
                    }
                }
            };
            let root_failed = snapshot.report.failed.len();
            if root_failed > 0 && matches!(failure_policy, GeometryFailurePolicy::Required) {
                bail!("e3d-model root {root} has {root_failed} failed element(s)");
            }
            let failure = (root_failed > 0).then(|| {
                eprintln!(
                    "e3d-model root {root} failed geometry details: {:?}",
                    snapshot.report.failed
                );
                // e3d-io 的「db N 没注册进这个 DbSet」补上路由目录知道的原因（ADR-078 D5）：
                // 现场人看到的才是「项目 ACP 的目录没定位到」，不是一个库号。
                crate::data_interface::mdb_route_directory::explain_unregistered_dbs(&format!(
                    "{root_failed} geometry element(s) failed: {:?}",
                    snapshot.report.failed
                ))
            });
            if let Some(failure) = failure {
                if incomplete == IncompleteRoot::Discard {
                    // Best-effort generation may report successful siblings next
                    // to failed elements. Publishing that partial manifest would
                    // delete the missing GeometryIds, so retain the previous
                    // complete root.
                    let root_skipped = snapshot.report.skipped.len();
                    generation.merge(snapshot.report);
                    total.failed += root_failed;
                    total.failed_roots.push((root, failure));
                    total.skipped += root_skipped;
                    log::error!(
                        "e3d-model root {root} produced an incomplete candidate ({root_failed} failure(s)); preserving published snapshot"
                    );
                    continue;
                }
                // Published partial: the root lands with the elements that did
                // evaluate, and says so.  `failed` still counts every element
                // that did not, so `/health` and the ensure receipt keep the
                // omission on the books instead of calling this a clean root.
                log::warn!(
                    "e3d-model root {root} publishing {} element(s) of an incomplete candidate; {root_failed} did not evaluate",
                    snapshot.elements.len()
                );
                total.partial_roots.push((root, failure));
            }
            let root_skipped = snapshot.report.skipped.len();
            // 子树访问名单只报出去，不在这里写库：读透形态下编排层拿它在发布模型行之前
            // 把子树元素写透（`docs/plans/2026-09-05-kv-mem-zero-ingest-read-through-plan.md` §3.2）。
            total.visited_by_root.insert(root, snapshot.visited);
            let removals = whole_root_removals(&prior[root_index], &snapshot.elements)?;
            generation.merge(snapshot.report);
            // Catalogue resolution during generation may have discovered
            // cross-database dependencies that were not in watermark pins.
            publication.model_target =
                Some(self.model_target(dbnum, source_sesno, dependency_sessions)?);
            let carried_room = snapshot.room;
            let point_sets = snapshot.point_sets;
            let (mut commit, mut projected) = project_geometry_delta(
                dbnum,
                source_sesno,
                snapshot.elements,
                removals,
                Some(&publication),
                &index.owners,
            )?;
            // bake 时算好的房间边随这一根的回执走（B/T3）：只有整根重生成这一条路带；清单没变、几何一条不重写的
            // 那种回执也带——边对着同一份构件集，flush 时照样能用。
            if let Some((_, receipt)) = commit.receipt.as_mut() {
                receipt.room = carried_room;
            }
            // 路由成员的 P 点集也随整根重生成走（ptset 分表计划 W2）：清单没变的回执照带——几何一样不等于 P 点行已经在
            // （换 vendor 后第一次重建正是这种），行便宜，写就是了。
            commit.point_sets = Some(crate::fast_model::ptset_rows::project_point_sets(
                root, point_sets,
            ));
            projected.skipped += root_skipped;
            projected.failed += root_failed;
            projected.note_lod1(snapshot.lod1);
            commits.push(commit);
            prepared_reports.push(projected);
        }
        if total.failed == 0 || cohort_was_empty {
            // One generate_roots call is one projection cohort: an OWNER move
            // (old root cleanup + new root takeover) and a PIPE expansion
            // containing several BRANs land in the store together. A cold
            // process store has no old ownership to preserve, so complete
            // siblings may still land when another root cannot be generated.
            super::model_write_behind::publish(commits)?;
            for report in prepared_reports {
                total.merge_counts(report);
            }
        } else {
            // On a warm projection, committing only the roots that happened to
            // generate could delete an element from its old root while its new
            // (failed) root never publishes it. Keep the established cohort
            // atomic and report every held sibling explicitly instead of
            // letting an upper layer mistake an uncommitted candidate for
            // success.
            let failed = total
                .failed_roots
                .iter()
                .map(|(root, _)| (root.word0, root.word1))
                .collect::<BTreeSet<_>>();
            for root in roots
                .iter()
                .copied()
                .filter(|root| !failed.contains(&(root.word0, root.word1)))
            {
                total.failed += 1;
                total.failed_roots.push((
                    root,
                    "projection cohort held because a peer root failed".into(),
                ));
            }
        }
        // 两道门判不画的那几根在这里离开生成层：报告里有点名账，回执与日志靠它才说得出
        // 「少的是哪一根」。渲染在序列化之前，`generation` 之后就被 JSON 吃掉了。
        total.tubes_not_drawn = generation.implied_tubes.not_drawn_line();
        total.generation_report = serde_json::to_value(generation)?;
        Ok(total)
    }

    fn pin(&self, dbnum: u32) -> anyhow::Result<&DbPin> {
        self.pins
            .iter()
            .find(|pin| pin.dbnum == dbnum as i32)
            .with_context(|| format!("current MDB has no source file for dbnum {dbnum}"))
    }

    /// 当前 MDB 里 `dbnum` 那份设计文件的路径与库类型。读透（`data_interface::read_through`）
    /// 要拿它反查 last-touch 会话——与生成读的是同一个文件。
    pub(crate) fn source_pin(&self, dbnum: u32) -> anyhow::Result<(PathBuf, String)> {
        let pin = self.pin(dbnum)?;
        Ok((pin.file.clone(), pin.db_type.clone()))
    }

    /// Every design database of the current MDB as `(dbnum, file, source session)`,
    /// the session being the one generation would read right now (ADR-054 Q1).
    ///
    /// `None` = 未钉住的成员此刻问不出最新会话（文件不在、被占、会话头读挂）。**不让调用方整个失败**：
    /// 唯一的调用方是 P4 设计引用级联（`window_root_plan` 的窗口规划），`None` 交给反查按「文件最新」
    /// 自己去开——开得了，读到的就是生成此刻会读的那个会话；开不了，记进 `unreadable_dbs`，整窗按
    /// 计划 §2 原则 3 退化（已持久化的根全部重算）并在回执点名。2026-09-30 P4 翻默认开（GitHub #32）
    /// 时补的：此前这里一个成员问不出会话，`?` 就让**每一个**设计库的窗口规划一起报错。
    pub(crate) fn design_sources(&self) -> Vec<(u32, PathBuf, Option<u32>)> {
        design_members(&self.pins, |pin| self.source_session(pin))
    }

    /// The session a database is generated from (ADR-054 Q1): an explicit pin
    /// wins; an unpinned database means the file's latest session right now.
    fn source_session(&self, pin: &DbPin) -> anyhow::Result<u32> {
        match pin.sesno {
            Some(sesno) => Ok(sesno),
            None => Ok(model_source::latest_source_version_of_file(
                pin.dbnum as u32,
                &pin.db_type,
                &pin.file,
            )?
            .sesno),
        }
    }

    fn model_target(
        &self,
        dbnum: u32,
        source_sesno: u32,
        dependency_sessions: &DependencySessions,
    ) -> anyhow::Result<ModelTarget> {
        let design_pin = self.pin(dbnum)?;
        let sessions = dependency_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let file_target = |dbnum: u32,
                           db_type: &str,
                           file: &Path,
                           session: u32|
         -> anyhow::Result<ModelFileTarget> {
            Ok(ModelFileTarget {
                dbnum,
                db_type: db_type.to_string(),
                // 记普通写法：冻结源服务的 pin 是 canonicalize 过的 `\\?\` 长路径，普通 MDB 服务的不是，
                // 同一个文件不该在回执里有两种写法（[`plain_file_path`]）。
                file: plain_file_path(file),
                session,
                // 身份是冻结锚（ADR-089 D2）：稳定文件 id + 库号 / 类型 + 会话 + 该会话两页的 sha256。此前记的是
                // 路径 + 长度 + mtime + 会话（`model_file_identity_digest`），而 E3D 每次会话都改写每个打开库的文件头，
                // mtime 一漂、摘要就变：按需显示把每个回执判过期，刚体 / 单元级的目录项核对也跟着回退（GitHub #65）。
                // 字典项会话位 0，取文件最新会话的锚。
                digest: match self.frozen_file_digests.get(&(dbnum, session)) {
                    Some(digest) => digest.clone(),
                    None => crate::data_interface::frozen_source::model_file_identity(
                        file,
                        (session != 0).then_some(session),
                    )?,
                },
            })
        };
        let design = file_target(
            design_pin.dbnum as u32,
            &design_pin.db_type,
            &design_pin.file,
            source_sesno,
        )?;
        let mut catalogue = BTreeMap::new();
        for pin in self
            .pins
            .iter()
            .filter(|pin| pin.db_type.eq_ignore_ascii_case("CATA"))
        {
            let session = sessions
                .get(&(dbnum, pin.dbnum as u32))
                .copied()
                .or(pin.sesno)
                .unwrap_or_default();
            catalogue.insert(
                pin.dbnum as u32,
                file_target(pin.dbnum as u32, &pin.db_type, &pin.file, session)?,
            );
        }
        for (&(target_dbnum, dependency_dbnum), &session) in sessions.iter() {
            if target_dbnum != dbnum {
                continue;
            }
            if catalogue.contains_key(&dependency_dbnum) {
                continue;
            }
            let Some(selected) = self.dependency_file(dbnum, dependency_dbnum) else {
                continue;
            };
            if !selected.db_type.eq_ignore_ascii_case("CATA") {
                continue;
            }
            catalogue.insert(
                dependency_dbnum,
                file_target(dependency_dbnum, &selected.db_type, &selected.path, session)?,
            );
        }
        // 字典库（DICT）也是几何的不可变输入：`build_set` 把 MDB 声明的字典库挂进 `DbSet`
        // （`7d8e88c29`），目录表达式里的 UDA 从此按字典的键求值（e3d-io `9508ac4`，
        // `( :HXYsize[2] )` 一类）——字典换一版，同一段目录程序可能求出别的数。身份按文件
        // （长度 + 修改时间）记，会话位固定 0：字典库由 `add_db` 预开、不经 `E3dDbResolver`
        // 懒开，没有谁往 `dependency_sessions` 里记它，让它跟会话走只会在两次算目标之间漂。
        for pin in self
            .pins
            .iter()
            .filter(|pin| pin.db_type.eq_ignore_ascii_case("DICT"))
        {
            catalogue.insert(
                pin.dbnum as u32,
                file_target(pin.dbnum as u32, &pin.db_type, &pin.file, 0)?,
            );
        }
        let project = self
            .locator
            .file_of(dbnum)
            .map(|(project, _)| project)
            .unwrap_or_default();
        Ok(ModelTarget {
            project,
            design,
            catalogue: catalogue.into_values().collect(),
            template_attlib_digest: model_file_identity_digest(
                &self.schema.template_dir().join("attlib.dat"),
                0,
            )?,
            // GitHub #73：指纹只有 `generator_identity` 那一份——`e3d-model:` 槽是 vendor 的构建身份（版本@提交），
            // 不再是本仓自己的版本号；水位行的库级落款写的也是同一串。
            generator_fingerprint: crate::fast_model::generator_identity::generator_fingerprint()
                .to_string(),
            tessellation_profile: "e3d-default-v1".to_string(),
        })
    }

    fn hydrate_published_dependencies(
        &self,
        receipt: &RootReceipt,
        dependency_sessions: &DependencySessions,
    ) -> anyhow::Result<()> {
        let Some(published) = receipt.model_target.as_ref() else {
            return Ok(());
        };
        let mut sessions = dependency_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for dependency in &published.catalogue {
            // 目标里的字典库（DICT）按文件记身份、会话位固定 0（见 `model_target`），
            // 不进依赖会话表——开它只为读一个不会用到的会话号。
            if !dependency.db_type.eq_ignore_ascii_case("CATA") {
                continue;
            }
            let key = (published.design.dbnum, dependency.dbnum);
            if sessions.contains_key(&key) {
                continue;
            }
            let Some(selected) = self.dependency_file(published.design.dbnum, dependency.dbnum)
            else {
                continue;
            };
            let mut view = DbView::open(&selected.path)?;
            if let Some(session) = view.walk_sessions()?.first() {
                sessions.insert(key, session.sesno);
            }
        }
        Ok(())
    }

    fn dependency_file(&self, target_dbnum: u32, dependency_dbnum: u32) -> Option<ProjectDbFile> {
        let preferred_project = self
            .locator
            .file_of(target_dbnum)
            .map(|(project, _)| project);
        select_dependency_file(
            &self.locator,
            &self.project_files,
            preferred_project.as_deref(),
            dependency_dbnum,
        )
    }

    /// Open the MDB's DESI files as one `DbSet`, pinning `target_dbnum` at
    /// `target_sesno` (`None` = the file's latest session) and every other
    /// database at its current pin.  Catalogue databases resolve lazily through
    /// `E3dDbResolver`.  The increment planner uses it for the `DbSet@S` /
    /// `DbSet@T` pair (`window_root_plan`).
    pub(crate) fn build_set(
        &self,
        target_dbnum: u32,
        target_sesno: Option<u32>,
    ) -> anyhow::Result<Arc<DbSet>> {
        let preferred_project = self
            .locator
            .file_of(target_dbnum)
            .map(|(project, _)| project);
        let set = DbSet::with_attlib_file_and_resolver(
            self.schema.template_dir().join("attlib.dat"),
            Box::new(E3dDbResolver {
                locator: self.locator.clone(),
                project_files: self.project_files.clone(),
                target_dbnum,
                preferred_project,
                template_dir: self.schema.template_dir().to_path_buf(),
                sessions: self.dependency_sessions.clone(),
            }),
        )?;
        // 池级元素缓存（[`crate::options::model_element_cache`]，**默认开**）。
        // `DbSet::element` 每次造新句柄，于是 owner 链上溯与目录链走查会把同一批祖先、
        // 同一批目录件反复查索引反复解析（AMS 7999 实测：15,968 个元素跑出 537 万次点查、
        // 264 s，占生成 CPU 的 85%；开着之后生成 71.0 → 3.84 s）。关掉只是慢回去，
        // 落下去的行逐字不变——它换的是内存（7997 峰值 768 MB → 1.55 GB），所以留了关位。
        // 元素数上限（[`crate::options::model_element_cache_max_entries`]，d-149，
        // 默认 200,000；**写 0 才是不限**）：
        // 满了整张清、下一次读再从文件装——产物仍逐字不变，只是清空之后那一小段重新变冷。
        let set = if crate::options::model_element_cache() {
            set.with_element_cache_limit(crate::options::model_element_cache_max_entries())
        } else {
            set
        };
        // 字典库的 UDA 定义（`:HXYsize` 一类）。系统属性来自 attlib 与 `*vir.dat`，
        // 项目属性不在那儿——名字 → 键那张表在 MDB 声明的 DICT 库里，名单只能由这一层给，
        // `DbSet` 自己不去找（见 `DbSet::with_uda_catalog`）。
        let set = match self.uda.clone() {
            Some(catalog) => set.with_uda_catalog(catalog),
            None => set,
        };
        let set = Arc::new(set);
        for pin in &self.pins {
            // CATA identity belongs to the project-aware locator.  Preloading a
            // watermark path here can shadow the resolver with a same-dbnum file
            // from another project (for example APS7600 vs ZDJ7600).
            if pin.db_type.eq_ignore_ascii_case("CATA") {
                continue;
            }
            let sesno = if pin.dbnum == target_dbnum as i32 {
                target_sesno
            } else {
                pin.sesno
            };
            set.add_db(DbFilePin {
                file: pin.file.clone(),
                template: template_file_for(self.schema.template_dir(), &pin.db_type)?,
                db_type: Some(pin.db_type.clone()),
                sesno,
            })?;
        }
        Ok(set)
    }
}

/// Resolve the configured MDB once into the source pins and dependency locator
/// used by both current and historical generation.  Current projection replaces
/// the returned pins with watermark-pinned DESI pins, while historical generation
/// keeps these paths and selects a session per request.
/// 当前 MDB 声明的字典库合成的一份 UDA 定义表，按 `CURD` 序合并。
///
/// 名单走 [`mdb_membership::MdbMembership::dictionary_paths`]，不扫项目目录：MDB 是跨项目的，
/// `AvevaMarineSample /ALL` 声明的六个字典库有四个在 `AvevaCatalogue` 与 `SCB` 底下，
/// 猜错不报错，而少一个字典库跟「这个 UDA 没值」长得一模一样。
///
/// 读不出来**不拦生成**：几何目前一格都不靠 UDA，为一份读不开的字典库把整轮生成判死
/// 是更坏的交换。但它也不能没声音——问题原样进 `log::warn!`。
fn current_uda_catalog() -> Option<Arc<e3d_io::uda_catalog::UdaCatalog>> {
    let option = aios_core::get_db_option();
    let membership = mdb_membership::get(&option.project_name, &option.mdb_name)
        .map(Ok)
        .unwrap_or_else(|| {
            mdb_membership::resolve(option, &option.project_name, &option.mdb_name).map(Arc::new)
        });
    let catalog = membership.and_then(|membership| {
        let schema = Arc::new(DirectSchema::open_from_env()?);
        crate::data_interface::uda_catalog_provider::UdaCatalogProvider::shared(&membership, schema)
            .get()
    });
    match catalog {
        Ok(catalog) => Some(catalog),
        Err(error) => {
            log::warn!("模型生成拿不到字典库的 UDA 定义，本轮按「没有 UDA」读：{error:#}");
            None
        }
    }
}

fn current_mdb_source_inventory()
-> anyhow::Result<(Vec<DbPin>, InMemoryCataLocator, ProjectDbFiles)> {
    let option = aios_core::get_db_option();
    let project = option.project_name.clone();
    let mdb = option.mdb_name.clone();
    let membership = mdb_membership::get(&project, &mdb)
        .map(Ok)
        .unwrap_or_else(|| {
            mdb_membership::resolve(option, &project, &mdb).map(std::sync::Arc::new)
        })?;
    // 与上面成员清单的 `get()` 同一口径：拿最近一次权威解析（startup / SYST 刷新 /
    // 重挂 / 兜底都会刷新 `latest` 缓存）的监控目录计划。这里不是重做时刻，
    // `plan_watch_dirs` 会把搜索根真扫一遍——探针路径上每任务一次的远程 IO。
    let watch_dirs = crate::data_interface::mdb_closure::latest_for(option).watch_dir_plan();
    crate::data_interface::project_paths::record_watch_dir_owners(&watch_dirs);
    let mut files = HashMap::new();
    let mut project_files = HashMap::new();
    let mut pins = Vec::new();
    for item in membership.databases() {
        let Some(path) = item.path.clone() else {
            continue;
        };
        let Some(db_type) = styp_name(item.styp) else {
            continue;
        };
        let source_project = item.project.clone().unwrap_or_else(|| project.clone());
        project_files
            .entry(project_db_key(&source_project, item.dbnum))
            .or_insert_with(|| ProjectDbFile {
                db_type: db_type.to_string(),
                project: source_project.clone(),
                path: path.clone(),
            });
        for shadowed in &item.shadowed {
            let Ok(header) = crate::data_interface::db_file_header::DbFileHeader::read(shadowed)
            else {
                continue;
            };
            if header.dbnum != item.dbnum as i32 || !header.db_type.eq_ignore_ascii_case(db_type) {
                continue;
            }
            let Some(shadowed_project) =
                crate::data_interface::project_paths::project_of_path(shadowed)
            else {
                continue;
            };
            project_files
                .entry(project_db_key(&shadowed_project, item.dbnum))
                .or_insert_with(|| ProjectDbFile {
                    db_type: header.db_type,
                    project: shadowed_project,
                    path: shadowed.clone(),
                });
        }
        files.insert(
            item.dbnum,
            (db_type.to_string(), source_project, path.clone()),
        );
        pins.push(DbPin {
            dbnum: item.dbnum as i32,
            db_type: db_type.to_string(),
            file: path,
            sesno: None,
        });
    }
    Ok((
        pins,
        InMemoryCataLocator::from_parts(HashMap::new(), files),
        Arc::new(project_files),
    ))
}

pub(crate) fn current_mdb_sources() -> anyhow::Result<(Vec<DbPin>, InMemoryCataLocator)> {
    let (pins, locator, _) = current_mdb_source_inventory()?;
    Ok((pins, locator))
}

/// [`E3dModelService::design_sources`] 的纯核：MDB 的设计成员，各带生成此刻读的会话
/// （`session` 即 `source_session`：钉住的照钉、没钉的问文件最新）；问不出的交 `None`，不中断。
fn design_members(
    pins: &[DbPin],
    mut session: impl FnMut(&DbPin) -> anyhow::Result<u32>,
) -> Vec<(u32, PathBuf, Option<u32>)> {
    pins.iter()
        .filter(|pin| pin.db_type.eq_ignore_ascii_case("DESI"))
        .map(|pin| (pin.dbnum as u32, pin.file.clone(), session(pin).ok()))
        .collect()
}

/// 目录文件的选法（GitHub #47）：生成读哪一份、MDB 声明的是哪一份。与 [`E3dDbResolver`] 同一张 locator、
/// 同一份项目副本表、同一个函数（[`select_dependency_file`]），只是不开库、不读 schema。
///
/// 同一个 CATA 库号在闭包里有几份文件时，设计库所在项目里那份才是它的模型真读的（AMS `/ALL` 实测：ACP 的
/// 设计库读 `acp7000`、AMS 的读 `ams7355`，MDB 声明的却是 `ams7000` / `aps7355`）。目录版本核对拿它比会话、
/// 在它上面收窗、给它盖落款——不拿 MDB 声明那份去比别的文件记下的会话号。
pub(crate) struct CatalogueFileChoice {
    locator: InMemoryCataLocator,
    project_files: ProjectDbFiles,
}

impl CatalogueFileChoice {
    pub(crate) fn current() -> anyhow::Result<Self> {
        let (_, locator, project_files) = current_mdb_source_inventory()?;
        Ok(Self {
            locator,
            project_files,
        })
    }

    #[cfg(test)]
    fn from_parts(locator: InMemoryCataLocator, project_files: ProjectDbFiles) -> Self {
        Self {
            locator,
            project_files,
        }
    }

    /// 设计库 `design_dbnum` 生成时目录库 `catalogue_dbnum` 读的那份文件；`None` = 生成也解不出它
    /// （MDB 没声明——resolver 只认 MDB 声明的库，GitHub #46）。
    pub(crate) fn for_library(&self, design_dbnum: u32, catalogue_dbnum: u32) -> Option<PathBuf> {
        let preferred_project = self
            .locator
            .file_of(design_dbnum)
            .map(|(project, _)| project);
        select_dependency_file(
            &self.locator,
            &self.project_files,
            preferred_project.as_deref(),
            catalogue_dbnum,
        )
        .map(|file| file.path)
    }

    /// MDB 声明的那份。
    pub(crate) fn declared(&self, dbnum: u32) -> Option<PathBuf> {
        self.locator.file_of(dbnum).map(|(_, path)| path)
    }
}

fn styp_name(value: i64) -> Option<&'static str> {
    match value {
        1 => Some("DESI"),
        2 => Some("CATA"),
        4 => Some("PROP"),
        6 => Some("ISOD"),
        7 => Some("PADD"),
        8 => Some("DICT"),
        9 => Some("ENGI"),
        14 => Some("SCHE"),
        _ => None,
    }
}

/// The requested root itself is absent from the generator's pinned source.
/// A missing catalogue/owner dependency is deliberately not this outcome.
#[derive(Debug)]
pub(crate) struct GenerationRootMissing {
    pub root: RefNo,
    pub dbnum: u32,
    pub source_sesno: u32,
    source: anyhow::Error,
}

impl std::fmt::Display for GenerationRootMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "generation root {} is absent from db {} session {}",
            self.root, self.dbnum, self.source_sesno
        )
    }
}
impl std::error::Error for GenerationRootMissing {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub(crate) fn classify_required_root_error(root: RefNo, error: anyhow::Error) -> anyhow::Error {
    let absent = error.chain().find_map(|cause| {
        match cause.downcast_ref::<e3d_io::db_element::DbElementError>() {
            Some(e3d_io::db_element::DbElementError::NotFound { refno, dbno, sesno })
                if *refno == root =>
            {
                Some((*dbno, *sesno))
            }
            _ => None,
        }
    });
    if let Some((dbnum, source_sesno)) = absent {
        return GenerationRootMissing {
            root,
            dbnum,
            source_sesno,
            source: error,
        }
        .into();
    }
    error
}

fn generate_snapshot_from_set(
    set: &Arc<DbSet>,
    index: &SourceIndex,
    root: RefNo,
    cache: &mut CatalogueMeshCache,
) -> anyhow::Result<GeneratedSnapshot> {
    let outcome = generate_subtree_with_cache(set.element(root), cache)
        .with_context(|| format!("generate root {root}"))?;
    Ok(GeneratedSnapshot {
        elements: outcome.elements,
        owners: Arc::clone(&index.owners),
        report: outcome.report,
        visited: outcome.visited,
        point_sets: outcome.point_sets,
    })
}

/// 与 [`generate_snapshot_from_set`] 相同，但 DFS 走 [`generate_subtree_with_hooks`]：钩子叫停时返回
/// [`RealtimeComputeError::Stopped`]；读库失败照旧包成 `generate root …` 上下文。
fn generate_snapshot_from_set_with_hooks(
    set: &Arc<DbSet>,
    index: &SourceIndex,
    root: RefNo,
    cache: &mut CatalogueMeshCache,
    hooks: &dyn GenerationHooks,
) -> Result<GeneratedSnapshot, RealtimeComputeError> {
    let outcome = match generate_subtree_with_hooks(set.element(root), cache, hooks) {
        Ok(outcome) => outcome,
        Err(GenerationStopped::Hook { stop, visited }) => {
            return Err(RealtimeComputeError::Stopped { stop, visited });
        }
        Err(GenerationStopped::Failed(error)) => {
            return Err(anyhow::Error::new(error)
                .context(format!("generate root {root}"))
                .into());
        }
    };
    Ok(GeneratedSnapshot {
        elements: outcome.elements,
        owners: Arc::clone(&index.owners),
        report: outcome.report,
        visited: outcome.visited,
        point_sets: outcome.point_sets,
    })
}

/// 单根快照的求值本体：Legacy 快照源（空钩子、当前模型输入、外面套 `db_generation_lock`）与
/// 实时计算源（历史钩子、历史输入缓存、不拿锁）共用这一段，两路求值代码一致（Requirement 6.15）。
///
/// 根不在那一会话的元素索引里，在求几何**之前**以 [`SnapshotSourceError::RootNotInSession`] 退出。
fn compute_snapshot_source(
    input: &GenerationInput,
    dbnum: u32,
    root: RefNo,
    source_sesno: u32,
    failure_policy: GeometryFailurePolicy,
    hooks: &dyn GenerationHooks,
) -> Result<GeneratedSnapshot, RealtimeComputeError> {
    if !input.index.owners.contains_key(&(root.word0, root.word1)) {
        return Err(anyhow::Error::from(SnapshotSourceError::RootNotInSession {
            root,
            dbnum,
            sesno: source_sesno,
        })
        .into());
    }
    let mut cache = CatalogueMeshCache::default();
    let snapshot = generate_snapshot_from_set_with_hooks(
        &input.set,
        &input.index,
        root,
        &mut cache,
        hooks,
    )?;
    if !snapshot.report.failed.is_empty()
        && matches!(failure_policy, GeometryFailurePolicy::Required)
    {
        return Err(anyhow::anyhow!(
            "e3d-model root {root} has {} failed element(s)",
            snapshot.report.failed.len()
        )
        .into());
    }
    Ok(snapshot)
}

/// One evaluated element, reduced to what publication actually needs.
///
/// Triangulation, the SHA-256 content id and the mesh file write all happen
/// while building this, so the `Manifold` — a world-space f64 CSG solid, the
/// fattest representation on the chain — dies where it was computed instead of
/// riding into the serial publication step.  What is left is a mesh id, two
/// boxes and the element's own metadata.
pub struct BakedElement {
    pub geometry_id: GeometryId,
    pub refno: RefNo,
    pub noun: String,
    pub is_invalid_tubi: bool,
    pub world: DMat4,
    pub primitive_instance: Option<e3d_model::primitive_instance::PrimitiveInstance>,
    /// Content-addressed id; the file is already published under it.
    pub mesh_id: String,
    pub mesh_write: MeshWrite,
    /// 盘上 `{mesh_id}.mesh` 的三角形数；粗精度副本（[`bake_lod1_root`]）拿它判值不值得写。
    pub mesh_triangles: usize,
    pub world_bounds: [[f32; 3]; 2],
    /// Only a canonical primitive persists a local box.
    pub local_bounds: Option<[[f32; 3]; 2]>,
    /// 这件几何对着本任务冻住的候选面板算出的房间归属边（ADR-073 D1，B/T2）：bake 时用手上的网格取判定点，
    /// 与 flush 时从盘上回读算的逐字相同；未与同构件其它实例合并（发布前按根经 `room_model::merge_root_edges`
    /// 收拢）。没给计算器（房间开关关 / 没有任务语境）时为空，房间那一半照旧由 flush 现算。
    pub room_edges: Vec<crate::fast_model::room_model::ElementRoomEdge>,
}

/// A generated root after baking.  It carries no `owners`: publication reads
/// the batch's own source index, and the single-root historical path keeps
/// using [`GeneratedSnapshot`].
pub struct BakedSnapshot {
    pub elements: Vec<BakedElement>,
    pub report: Report,
    /// 随 [`GeneratedSnapshot::visited`] 原样带过来；烤制不改子树。
    pub visited: Vec<RefNo>,
    /// 这一根随回执走的房间归属边（B/T3，`RootReceipt::room`）：bake 时给了计算器才有；发布段把它挂到
    /// `project_geometry_delta` 折出来的回执上。
    pub room: Option<crate::fast_model::model_memory_store::CarriedRoomEdges>,
    /// 随 [`GeneratedSnapshot::point_sets`] 原样带过来；烤制不动 P 点。
    pub point_sets: Vec<e3d_model::MemberPointSet>,
    /// 这一根的显示用粗网格账（`AIOS_MESH_LOD1` 关着时全零）。只进报表，不进身份、投影与回执。
    pub lod1: crate::fast_model::mesh_lod1::Lod1Bake,
}

/// Triangulate, hash and publish one element's mesh.
///
/// Baking here means a root that later fails its `Required` check — or whose
/// publication transaction rolls back — leaves its mesh on disk.  That is the
/// content-addressed contract rather than a new risk: the id *is* the content
/// hash, so an orphan is simply unreferenced and waits for reachability GC.
///
/// `rooms` 给了就顺手算这件几何的房间归属（ADR-073 D1，B/T2）：判定点取自手上这份**就要写盘的网格**
/// （本地 / 标准图元那一份，不是上面为算盒子折出来的 `world_mesh`——那是 manifold 在 f64 里算完再转的，
/// 与盘上回读差几个 ulp）经 `room_model::world_points_of` 变换，与 flush 时从 `.mesh` 回读的那条路是**同一个
/// 函数**，边逐字相同；只有盒跨在面板边上的构件才真取点。算不出来 = 这一根 `Err`（D2「房间失败 = 该根失败」，
/// 文案与 flush 时 `room_stage` 的同形）。
fn bake_element(
    element: GeneratedElement,
    mesh_dir: &Path,
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
) -> anyhow::Result<BakedElement> {
    let world_bounds = {
        let world_mesh = element.parts.as_deref().map_or_else(
            || crate::fast_model::manifold_csg::manifold_to_plant_mesh(&element.solid),
            crate::fast_model::manifold_csg::manifolds_to_plant_mesh,
        );
        let aabb = world_mesh.aabb.context("world PlantMesh missing AABB")?;
        [point3(&aabb.mins), point3(&aabb.maxs)]
    };
    let (mesh, mesh_id) = element_mesh(&element)?;
    // Only a canonical primitive persists a local box.
    let local_bounds = match element.primitive_instance.as_ref() {
        Some(_) => {
            let aabb = mesh.aabb.context("canonical PlantMesh missing AABB")?;
            Some([point3(&aabb.mins), point3(&aabb.maxs)])
        }
        None => None,
    };
    let mesh_write = ensure_mesh_file(&mesh_dir.join(format!("{mesh_id}.mesh")), &mesh)?;
    let room_edges = match rooms {
        Some(rooms) => {
            let input = baked_room_input(
                &element.geometry_id,
                element.primitive_instance.as_ref(),
                element.world,
                world_bounds,
                &mesh_id,
            )?;
            // 算不出 = 这一根 Err（D2）：生成 worker 把它记成这一根的死信、不发布；文案带「room membership」，
            // 与 flush 时 `room_stage` 的同族，账本 / 回执的消费方不用分辨是哪一处算的。
            rooms
                .edges_for_record(&input, || {
                    crate::fast_model::room_model::world_points_of(&mesh, input.mesh_to_world)
                })
                .with_context(|| {
                    format!("room membership of {} could not be computed", input.label)
                })?
        }
        None => Vec::new(),
    };
    Ok(BakedElement {
        geometry_id: element.geometry_id,
        refno: element.refno,
        noun: element.noun,
        is_invalid_tubi: element.is_invalid_tubi,
        world: element.world,
        primitive_instance: element.primitive_instance,
        mesh_triangles: mesh.indices.len() / 3,
        mesh_id,
        mesh_write,
        world_bounds,
        local_bounds,
        room_edges,
    })
}

/// bake 时这件几何在房间判定眼里的形状——与它随后变成的投影记录**字段相同**（`project_geometry_delta`
/// 的那一段）：构件 = `geometry_source_refno`（隐式管身挂容器）、标签 = 投影记录 id、世界变换 =
/// `persisted_world_transform`（隐式管身把标准图元摆放折进去）、「网格 → 世界」= `room_model::projected_mesh_to_world`
/// （标准图元再乘本地摆放；隐式管身**不再乘**——它的本地摆放已经在存下的世界变换里，与库上管身行读回的
/// 单位阵实例变换同义）。两边只许有这一份配方，源码钉 `bake_time_room_input_matches_the_projected_record`。
fn baked_room_input(
    geometry_id: &GeometryId,
    primitive: Option<&e3d_model::primitive_instance::PrimitiveInstance>,
    world: DMat4,
    world_bounds: [[f32; 3]; 2],
    mesh_id: &str,
) -> anyhow::Result<crate::fast_model::room_model::BakedRoomInput> {
    let source_refno = geometry_source_refno(geometry_id)?;
    let world_transform = persisted_world_transform(geometry_id, primitive, world);
    Ok(crate::fast_model::room_model::BakedRoomInput {
        element: crate::data_interface::generation_root::refno_from_e3d(source_refno),
        label: geometry_record_id(geometry_id, source_refno),
        world_bounds,
        mesh_id: mesh_id.to_string(),
        mesh_to_world: crate::fast_model::room_model::projected_mesh_to_world(
            world_transform,
            primitive.map(|instance| instance.local_transform),
            matches!(geometry_id, GeometryId::ImpliedTube { .. }),
        ),
    })
}

/// 失败元素的有界摘要：按 noun 聚合，再带前几条的 refno / noun / detail。
///
/// 整份 `{:?}` 在 7022（CABLETMPL）那种一根 4548 个失败元素的库上会把一行日志和
/// 失败账本条目撑到几百 KB；而只写「N 个失败」又定位不了任何东西——run-ad 的 4,089 个
/// FailedHold 就是这样查不下去的。
pub(crate) fn summarize_geometry_incidents(failed: &[e3d_model::pipeline::Incident]) -> String {
    const SAMPLES: usize = 5;
    const NOUNS: usize = 8;
    const DETAIL_CHARS: usize = 160;
    let mut by_noun = BTreeMap::<&str, usize>::new();
    for incident in failed {
        *by_noun.entry(incident.noun.as_str()).or_default() += 1;
    }
    let mut nouns: Vec<(&str, usize)> = by_noun.into_iter().collect();
    nouns.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let more = nouns.len().saturating_sub(NOUNS);
    let mut nouns = nouns
        .iter()
        .take(NOUNS)
        .map(|(noun, count)| format!("{noun}×{count}"))
        .collect::<Vec<_>>()
        .join(", ");
    if more > 0 {
        nouns.push_str(&format!(", +{more} nouns"));
    }
    let samples = failed
        .iter()
        .take(SAMPLES)
        .map(|incident| {
            let mut detail: String = incident.detail.chars().take(DETAIL_CHARS).collect();
            if incident.detail.chars().count() > DETAIL_CHARS {
                detail.push('…');
            }
            format!("{} {}: {detail}", incident.refno, incident.noun)
        })
        .collect::<Vec<_>>()
        .join(" | ");
    format!(
        "by noun [{nouns}]; first {}: {samples}",
        failed.len().min(SAMPLES)
    )
}

fn bake_elements(
    elements: Vec<GeneratedElement>,
    mesh_dir: &Path,
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
) -> anyhow::Result<Vec<BakedElement>> {
    elements
        .into_iter()
        .map(|element| bake_element(element, mesh_dir, rooms))
        .collect()
}

/// What a root whose elements did not all evaluate is worth publishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncompleteRoot {
    /// Throw the whole candidate away.  Publishing a partial manifest deletes
    /// the GeometryIds it omits, so a root that already published keeps what it
    /// has; the baked survivors become orphans for the reachability GC.
    Discard,
    /// Publish whatever evaluated.  Only for a root with nothing published to
    /// lose: there the discard preserves an empty snapshot, which is how one
    /// dangling catalogue item hides an entire branch.
    Publish,
}

/// A partial manifest deletes the GeometryIds it omits, so publishing one is
/// only safe for a root with nothing to lose: no publication receipt **and** no
/// geometry rows.  Everywhere else the incomplete candidate is dropped and the
/// established snapshot stands.
///
/// The cold case is not a corner: on a read-through store every first display is
/// cold, and dropping the candidate there preserves an empty snapshot — which is
/// how one dangling catalogue item hides a whole branch (SLYK `21491/17101`: one
/// FLAN whose `Spref` points at a catalogue element the project never had, and
/// the other 16 members of the branch never reach the viewport).
fn incomplete_root_policy(published: bool, has_geometry_rows: bool) -> IncompleteRoot {
    if published || has_geometry_rows {
        IncompleteRoot::Discard
    } else {
        IncompleteRoot::Publish
    }
}

/// Bake a generated root for publication.
///
/// An incomplete root is baked like any other: whether its survivors are worth
/// publishing is [`incomplete_root_policy`]'s call, and that needs publication
/// state the producer half of the memory pipeline deliberately cannot see
/// (`memory_generation_reads_only_the_e3d_source_projection`).  Baking here and
/// deciding there costs a discarded candidate its triangulation — its meshes
/// are content-addressed, so a root that published before rewrites nothing —
/// and it is the only order in which both halves can reach the same verdict.
fn bake_snapshot(
    snapshot: GeneratedSnapshot,
    mesh_dir: &Path,
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
) -> anyhow::Result<BakedSnapshot> {
    let elements = bake_elements(snapshot.elements, mesh_dir, rooms)?;
    // 随根走的那一份边（B/T3）：这一根全部几何的构件（有边没边都在册，flush 时与投影记录比对）+ 各元素的边收拢。
    let room = match rooms {
        Some(rooms) => {
            let members = elements
                .iter()
                .map(|element| {
                    geometry_source_refno(&element.geometry_id)
                        .map(crate::data_interface::generation_root::refno_from_e3d)
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Some(crate::fast_model::room_model::carry_root_edges(
                rooms.resident_generation(),
                members,
                elements
                    .iter()
                    .flat_map(|element| element.room_edges.iter().cloned()),
            ))
        }
        None => None,
    };
    Ok(BakedSnapshot {
        elements,
        report: snapshot.report,
        visited: snapshot.visited,
        room,
        point_sets: snapshot.point_sets,
        lod1: Default::default(),
    })
}

impl E3dPersistReport {
    fn merge_counts(&mut self, other: Self) {
        self.upserted += other.upserted;
        self.removed += other.removed;
        self.skipped += other.skipped;
        self.failed += other.failed;
        self.shared_instances += other.shared_instances;
        self.baked_instances += other.baked_instances;
        self.mesh_reused += other.mesh_reused;
        self.mesh_written += other.mesh_written;
        self.mesh_repaired += other.mesh_repaired;
        self.lod1_written += other.lod1_written;
        self.lod1_reused += other.lod1_reused;
        self.lod1_skipped += other.lod1_skipped;
        self.lod1_unmatched += other.lod1_unmatched;
        self.lod1_failed += other.lod1_failed;
        self.mesh_ids.extend(other.mesh_ids);
        self.failed_roots.extend(other.failed_roots);
        self.partial_roots.extend(other.partial_roots);
        self.unique_meshes = self.mesh_ids.len();
        self.source_sesno = self.source_sesno.or(other.source_sesno);
        self.visited_by_root.extend(other.visited_by_root);
        self.tubes_not_drawn = match (self.tubes_not_drawn.take(), other.tubes_not_drawn) {
            (Some(mine), Some(theirs)) => Some(format!("{mine} | {theirs}")),
            (mine, theirs) => mine.or(theirs),
        };
    }
}

pub async fn apply_geometry_delta(
    dbnum: u32,
    source_sesno: u32,
    upserts: Vec<GeneratedElement>,
    removals: Vec<GeometryId>,
    owners: &BTreeMap<(u32, u32), RefNo>,
    mesh_dir: &Path,
) -> anyhow::Result<E3dPersistReport> {
    apply_geometry_delta_on(
        ProjectionScope::Current,
        dbnum,
        source_sesno,
        upserts,
        removals,
        owners,
        mesh_dir,
    )
    .await
}

/// 「上一版有、这一版没有」= 删掉了——**只有当 `generated` 确实是这根的全集时这句才成立**
/// （ADR-080 D2，[`CommitScope::WholeRoot`]）。
///
/// 这是整个发布链路上**唯一**一处拿缺席推删除的地方，所以单独拎出来命名：单元级发布只重建变了
/// 的那几件，同一条推导会把没重建的全部删掉，而且投影与库两边都不会报错——静默丢件。
/// 局部提交要删什么必须显式列出，不得改道走这里。
fn whole_root_removals(
    prior: &[GeometryId],
    generated: &[BakedElement],
) -> anyhow::Result<Vec<GeometryId>> {
    let generated_ids = generated
        .iter()
        .map(|element| Ok(serde_json::to_string(&element.geometry_id)?))
        .collect::<anyhow::Result<BTreeSet<String>>>()?;
    Ok(prior
        .iter()
        .cloned()
        .filter_map(|geometry_id| {
            let key = serde_json::to_string(&geometry_id).ok()?;
            (!generated_ids.contains(&key)).then_some(geometry_id)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

/// [`E3dModelService::republish_roots_by_unit_delta`] 的本体；锁与会话闸留在外面。
///
/// 拆出来是为了让「闸 → 产出 → 提交」这条顺序能在一个本地存储、一个本地发布口上被测试钉住
/// （生产的 `publish` 走 write-behind 的全局控制器）。这里**不读任何记录**：删什么、改什么由
/// `produce` 点名，整根的章由存储在写锁里盖（D5），所以闸后面没有第二个能读到旧状态的窗口。
///
/// **整窗一批发布**（D4：一次 `publish(commits)` 是一个原子边界，可以跨多根）：走成的根先攒着，循环
/// 结束一次发出。一件改挂的单元在旧根的清单里是显式删除、在新根的清单里是同一个 `record_id` 的 upsert
/// ——存储对一批提交先落全部删除再落全部插入、章按整批落完之后的状态盖，两根谁先谁后都不打架；
/// 逐根各发一批的话，旧根的删除排在新根的插入之后就把刚插的那件删掉了。
pub(crate) fn republish_roots_by_unit_delta_on(
    store: &ModelMemoryStore,
    mesh_dir: &Path,
    target_sesno: u32,
    units: &[UnitPublishUnit],
    mut produce: impl FnMut(&UnitPublishUnit, RefNo, &RootReceipt) -> anyhow::Result<UnitDelta>,
    mut publish: impl FnMut(Vec<ProjectionCommit>) -> anyhow::Result<()>,
) -> anyhow::Result<UnitPublishReport> {
    let mut report = UnitPublishReport::default();
    let mut commits = Vec::new();
    let mut landing = Vec::new();
    for unit in units {
        let root_refno = &unit.root_refno;
        let root = match parse_refno(root_refno) {
            Ok(root) => root,
            Err(error) => {
                report
                    .fallbacks
                    .push((root_refno.clone(), format!("unparsable root: {error:#}")));
                continue;
            }
        };
        // 前置：投影完整（记录数与回执相符、mesh 文件都在）且有旧回执可对。残缺的根没有「S 上的
        // 全集」可叠——`IncompleteRoot::Discard` 在 `Partial` 下不成立（D2），由整根回退顶上（D8）。
        if !store.root_projection_complete(ProjectionScope::Current, root, mesh_dir) {
            report
                .fallbacks
                .push((root_refno.clone(), "projection incomplete".into()));
            continue;
        }
        let Some(old_receipt) = store.receipt(ProjectionScope::Current, root) else {
            report
                .fallbacks
                .push((root_refno.clone(), "no cached receipt".into()));
            continue;
        };
        // 幂等闸（D4，实施约束 5；ADR-089 D7）：在要产出、读记录、动几何之前。只有「投影还停在规划基线 S 或 S 之前」才许叠局部清单。
        match republish_gate(
            old_receipt.source_sesno,
            unit.source_start_sesno,
            target_sesno,
        ) {
            RepublishGate::Proceed => {}
            RepublishGate::SkipAlreadyAtTarget => {
                report.replayed.push(root_refno.clone());
                continue;
            }
            RepublishGate::FallbackBaselineMismatch => {
                report.fallbacks.push((
                    root_refno.clone(),
                    format!(
                        "projection receipt at sesno {} is newer than the baseline {} the partial \
                         manifest was planned against (target {}); falling back to regen",
                        old_receipt.source_sesno, unit.source_start_sesno, target_sesno
                    ),
                ));
                continue;
            }
        }
        let delta = match produce(unit, root, &old_receipt) {
            Ok(delta) => delta,
            Err(error) => {
                report
                    .fallbacks
                    .push((root_refno.clone(), format!("{error:#}")));
                continue;
            }
        };
        // 管段完整性门（D6 / 实施约束 4）：产出与提交之间。触到的 BRAN 少带一段就整根回退，不发。
        if let Err(reason) = tube_segment_completeness(&delta) {
            report.fallbacks.push((
                root_refno.clone(),
                format!("tube segment completeness: {reason}; falling back to regen"),
            ));
            continue;
        }
        // D10：空清单（不插、不删、不重写管段）只许来自带 `no_model_only` 提示的根——生产方对那种根切出
        // 空子计划、对账本复核过才回空 delta；别的根回出空 delta 是生产方的 bug，不许拿它盖 T 章。
        let receipt_only = delta.is_empty();
        if receipt_only && !unit.no_model_only {
            report.fallbacks.push((
                root_refno.clone(),
                format!(
                    "partial manifest for root {root} is empty but the root was not planned as \
                     no_model_only; an empty delta is not proof of \"unchanged\" (D10); falling back \
                     to regen"
                ),
            ));
            continue;
        }
        // D2：一根被删空（最后一件也删了）不走 `Partial`，退回根级路径。执行体不读记录，判据只用
        // 回执上的记录数与清单本身：带了 upsert 的清单落完必有记录；一件不带、又删了不少于整根记录数
        // 的 id，或整段清了某条分支的管身（清多少段这里看不见），都可能把这根删空——按可能算，回退。
        // 什么都不删的空清单（D10 只推回执）不在此列：它留下的记录就是原来那些，零记录的根也照旧零记录。
        if partial_delta_may_empty_the_root(&delta, old_receipt.geometry_count) {
            report.fallbacks.push((
                root_refno.clone(),
                format!(
                    "partial manifest carries no upsert and may leave root {root} empty ({} removal(s), \
                     {} branch reset(s) against {} record(s) on the receipt); an emptied root is the \
                     root-level path's to publish (D2); falling back to regen",
                    delta.removals.len(),
                    delta.tube_branch_resets.len(),
                    old_receipt.geometry_count
                ),
            ));
            continue;
        }
        // 局部提交：`scope` 说明缺席不证明任何事（D2）；回执两格是占位，存储在写锁里按 delta 落完之后的
        // 整根重算（D5）；`published_via = "unit"` 只作观测，不进任何判据。攒进本窗这一批，循环结束一次发。
        commits.push(ProjectionCommit {
            tube_branch_resets: delta.tube_branch_resets,
            removals: delta.removals,
            upserts: delta.upserts,
            receipt: Some((
                root,
                RootReceipt {
                    source_sesno: target_sesno,
                    model_target: delta.model_target,
                    manifest_hash: String::new(),
                    geometry_count: 0,
                    published_via: "unit".into(),
                    room: None,
                },
            )),
            scope: CommitScope::Partial,
            // 局部清单不带 P 点：只重建了变了的几件，整根的点集在存储里照旧（`None` = 不动）。
            point_sets: None,
        });
        landing.push((root_refno.clone(), receipt_only));
    }
    if !commits.is_empty() {
        // 一次发布调用 = 一个原子边界（D4）：改挂两端的根一起落地。发布口不收件整轮上抛，与刚体路一致
        // ——没有一根算走成。
        publish(commits)?;
        for (root_refno, receipt_only) in landing {
            if receipt_only {
                report.receipt_only.push(root_refno.clone());
            }
            report.completed.push(root_refno);
        }
    }
    Ok(report)
}

/// ADR-080 D2 第二条的判据：一份 `Partial` 清单**可能**把这根删空吗？执行体不读记录，能用的只有回执上
/// 的记录数（投影完整时就是这根此刻的记录数）与清单本身。
///
/// - 带了 upsert → 落完必有记录，不可能删空；
/// - 一件不带、显式删除的 id 数 ≥ 记录数（且真删了东西）→ 可能删空（多出来的 id 若是无效 id 就删不到，
///   那是保守多退一次）；
/// - 一件不带、还整段清了某条分支的管身 → 一条重置清掉几段这里看不见，按可能删空算；
/// - 什么都不删、什么都不插（D10 的只推回执）→ 记录集合原样不动，**不是**删空——零记录的根照旧零记录，
///   那是它本来的样子，不是这份清单删出来的。
///
/// 只在「可能」这一侧保守：判 `false` 的清单落完记录集合非空或与落之前相同，判 `true` 的回退整根重算，
/// 回退永远正确。
fn partial_delta_may_empty_the_root(delta: &UnitDelta, records_on_receipt: usize) -> bool {
    delta.upserts.is_empty()
        && (!delta.tube_branch_resets.is_empty()
            || (!delta.removals.is_empty() && delta.removals.len() >= records_on_receipt))
}

/// 目标没变、投影也还完整时这一根的全部改动：只重盖一次章。
///
/// 旧版这里要发一条根凭证的 `published_*` 更新，外加一批依赖收据行；现在收据就是回执本身，
/// 依赖清单在 `model_target.catalogue` 里带着。
fn cached_root_commit(
    publication: &RootPublication,
    source_sesno: u32,
) -> anyhow::Result<ProjectionCommit> {
    let published = publication
        .published
        .as_ref()
        .context("cached projection has no receipt")?;
    Ok(ProjectionCommit {
        receipt: Some((
            publication.root,
            RootReceipt {
                source_sesno,
                model_target: publication.model_target.clone(),
                manifest_hash: published.manifest_hash.clone(),
                geometry_count: published.geometry_count,
                published_via: "regen".into(),
                room: None,
            },
        )),
        ..ProjectionCommit::default()
    })
}

/// 这一根能不能走「不必重算」那条路：源会话与不可变目标都必须与上一轮逐位相同。
fn cached_root_target_matches(publication: &RootPublication, source_sesno: u32) -> bool {
    let Some(published) = publication.published.as_ref() else {
        return false;
    };
    if published.source_sesno != source_sesno {
        return false;
    }
    let Some(current) = publication.model_target.as_ref() else {
        return false;
    };
    published.model_target.as_ref() == Some(current)
}

/// 把一次增量投进指定投影域。历史快照与当前投影共用这一条。
pub(crate) async fn apply_geometry_delta_on(
    scope: ProjectionScope<'_>,
    dbnum: u32,
    source_sesno: u32,
    upserts: Vec<GeneratedElement>,
    removals: Vec<GeometryId>,
    owners: &BTreeMap<(u32, u32), RefNo>,
    mesh_dir: &Path,
) -> anyhow::Result<E3dPersistReport> {
    let mut report = match scope {
        ProjectionScope::Current => {
            let (commit, report) = project_geometry_delta(
                dbnum,
                source_sesno,
                bake_elements(upserts, mesh_dir, None)?,
                removals,
                None,
                owners,
            )?;
            super::model_write_behind::publish(vec![commit])?;
            report
        }
        ProjectionScope::Historical(_) => {
            // 历史快照每次整根重生成（调用方先 `drop_snapshot`），没有要删的旧记录。
            anyhow::ensure!(
                removals.is_empty(),
                "a historical snapshot is generated whole and carries no removals"
            );
            let projected =
                project_historical_records(dbnum, source_sesno, upserts, owners, mesh_dir, None)?;
            ModelMemoryStore::global().commit(scope, vec![projected.commit]);
            projected.report
        }
    };
    report.unique_meshes = report.mesh_ids.len();
    Ok(report)
}

/// [`project_historical_records`] 在 bake 两个元素之间看到取消标志已置位。
///
/// 调用方用 `error.downcast_ref::<HistoricalProjectionCancelled>()` 区分「被取消」与「算失败」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HistoricalProjectionCancelled;

impl std::fmt::Display for HistoricalProjectionCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("historical projection cancelled")
    }
}

impl std::error::Error for HistoricalProjectionCancelled {}

/// 一次历史投影的产物：记录在 `commit.upserts`，`report` 是 bake 与投影的计数。
#[derive(Debug, Clone)]
pub(crate) struct HistoricalProjection {
    /// 与 `project_geometry_delta(…, removals = [], publication = None, …)` 产出的提交逐位相同。
    /// Legacy 路整笔提交进 `ProjectionScope::Historical`；实时路只取 `upserts` 序列化。
    pub(crate) commit: ProjectionCommit,
    pub(crate) report: E3dPersistReport,
}

impl HistoricalProjection {
    /// 投影记录，按产出顺序（同一 `record_id` 可能出现多次，存储里后写覆盖先写）。
    pub(crate) fn records(&self) -> &[ProjectedGeometry] {
        &self.commit.upserts
    }

    pub(crate) fn into_records(self) -> Vec<ProjectedGeometry> {
        self.commit.upserts
    }
}

/// 历史快照的「bake + project」：Legacy 快照路（随后 `commit` 进 `ProjectionScope::Historical`）与
/// 实时历史路（直接序列化记录）共用这一个函数，两路的记录出自同一段代码（Requirement 6.15）。
///
/// - bake：逐个元素三角化、算内容寻址 `mesh_id`、把网格写进共享网格目录；历史不算房间归属（`rooms = None`）。
/// - 取消：每 bake 一个元素**之前**查一次 `cancel`，置位即返回 [`HistoricalProjectionCancelled`]；
///   已写下的网格文件是内容寻址的，留着无害。投影段是纯结构化，不再查。
/// - project：`project_geometry_delta(…, removals = [], publication = None, owners)`，不产回执。
pub(crate) fn project_historical_records(
    dbnum: u32,
    source_sesno: u32,
    elements: Vec<GeneratedElement>,
    owners: &BTreeMap<(u32, u32), RefNo>,
    mesh_dir: &Path,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> anyhow::Result<HistoricalProjection> {
    let mut baked = Vec::with_capacity(elements.len());
    for element in elements {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            return Err(HistoricalProjectionCancelled.into());
        }
        baked.push(bake_element(element, mesh_dir, None)?);
    }
    let (commit, report) =
        project_geometry_delta(dbnum, source_sesno, baked, Vec::new(), None, owners)?;
    Ok(HistoricalProjection { commit, report })
}

/// 把一次增量折成投影存储要的一笔改动。
///
/// 这里点名的每个 mesh 都已经在盘上了（见 [`bake_element`]），所以这一步只做结构化与记账，
/// 可以留在串行的消费端而不把几何 CPU 压在它后面。
fn project_geometry_delta(
    dbnum: u32,
    source_sesno: u32,
    upserts: Vec<BakedElement>,
    removals: Vec<GeometryId>,
    publication: Option<&RootPublication>,
    owners: &BTreeMap<(u32, u32), RefNo>,
) -> anyhow::Result<(ProjectionCommit, E3dPersistReport)> {
    let mut report = E3dPersistReport::default();
    let mut commit = ProjectionCommit::default();
    // 一条 BRAN 的隐式管身是整段重写的：碰到的分支先清空旧管身，再插这一轮的。
    commit.tube_branch_resets = removals
        .iter()
        .chain(upserts.iter().map(|element| &element.geometry_id))
        .filter_map(|geometry_id| match geometry_id {
            GeometryId::ImpliedTube {
                container_refno, ..
            } => Some(container_refno.as_str()),
            GeometryId::Element { .. } => None,
        })
        .map(|value| parse_refno(value).map(|refno| (refno.word0, refno.word1)))
        .collect::<anyhow::Result<BTreeSet<_>>>()?
        .into_iter()
        .map(|(word0, word1)| RefNo::new(word0, word1))
        .collect();

    for geometry_id in &removals {
        let source = geometry_source_refno(geometry_id)?;
        commit
            .removals
            .push(geometry_record_id(geometry_id, source));
        report.removed += 1;
    }

    for element in upserts {
        let source_refno = geometry_source_refno(&element.geometry_id)?;
        let record_id = geometry_record_id(&element.geometry_id, source_refno);
        let ancestors = ancestor_chain(source_refno, owners)?;
        // 隐式管身只向 Plant UI 暴露一个变换，所以标准管身要把元素摆放与标准网格的
        // 缩放 / 居中折进同一个存下来的世界变换里。
        let world_transform = persisted_world_transform(
            &element.geometry_id,
            element.primitive_instance.as_ref(),
            element.world,
        );
        note_mesh_write(&mut report, element.mesh_write);
        report.mesh_ids.insert(element.mesh_id.clone());

        let shape = match element.primitive_instance.as_ref() {
            Some(instance) => {
                report.shared_instances += 1;
                ProjectedShape::Canonical {
                    key: instance.key,
                    local_transform: instance.local_transform,
                    local_bounds: element
                        .local_bounds
                        .context("canonical primitive baked without local bounds")?,
                }
            }
            None => {
                report.baked_instances += 1;
                ProjectedShape::Baked
            }
        };
        let tube = match &element.geometry_id {
            GeometryId::ImpliedTube {
                route_ordinal,
                from_refno,
                to_refno,
                ..
            } => Some(ProjectedTube {
                route_ordinal: *route_ordinal,
                from: parse_refno(from_refno)?,
                to: parse_refno(to_refno)?,
                invalid: element.is_invalid_tubi,
            }),
            GeometryId::Element { .. } => None,
        };

        commit.upserts.push(ProjectedGeometry {
            record_id,
            geometry_id: element.geometry_id,
            source_refno,
            dbnum,
            source_sesno,
            noun: element.noun,
            mesh_id: element.mesh_id,
            world_transform,
            world_bounds: element.world_bounds,
            ancestors,
            shape,
            tube,
        });
        report.upserted += 1;
    }

    if let Some(publication) = publication {
        // 这一根「看得见的清单」：目标变了但清单逐位没变时，几何一条都不必重写。
        let (manifest_hash, geometry_count) = manifest_hash(&commit.upserts)?;
        if publication.projection_complete
            && publication
                .published
                .as_ref()
                .is_some_and(|published| published.manifest_hash == manifest_hash)
        {
            // 不可变目标变了，但这一根看得见的清单没变：只留回执，几何一条都不动。
            commit.tube_branch_resets.clear();
            commit.removals.clear();
            commit.upserts.clear();
            report.upserted = 0;
            report.removed = 0;
        }
        commit.receipt = Some((
            publication.root,
            RootReceipt {
                source_sesno,
                model_target: publication.model_target.clone(),
                manifest_hash,
                geometry_count,
                published_via: "regen".into(),
                room: None,
            },
        ));
    }
    report.unique_meshes = report.mesh_ids.len();
    Ok((commit, report))
}

/// 一条投影记录在根回执清单里的那一格：`geometry_key → entry`。
///
/// 这是 `RootReceipt.manifest_hash` 的**配方**：重生成（[`project_geometry_delta`]）与刚体前移
/// （`rigid_motion::republish_records`，ADR-066）都必须从这里取——两条路对同一批记录才会算出
/// 同一个章，对拍门比的正是它。条目只放「看得见」的东西（几何身份、mesh、世界变换、noun、
/// 图元摆放 / 烘焙标记、管段序号与有效位），不放来源会话与祖先链。
pub(crate) fn manifest_entry(record: &ProjectedGeometry) -> anyhow::Result<(String, Value)> {
    let geometry_key = serde_json::to_string(&record.geometry_id)?;
    let mut entry = json!({
        "geometry_id": serde_json::to_value(&record.geometry_id)?,
        "mesh": record.mesh_id,
        "world": transform_value(record.world_transform),
        "noun": record.noun,
        "solid": true,
    });
    match (&record.tube, &record.shape) {
        (Some(tube), _) => {
            entry["route_ordinal"] = json!(tube.route_ordinal);
            entry["invalid"] = json!(tube.invalid);
        }
        (
            None,
            ProjectedShape::Canonical {
                key,
                local_transform,
                ..
            },
        ) => {
            entry["local"] = transform_value(*local_transform);
            entry["primitive_key"] = serde_json::to_value(key)?;
        }
        (None, ProjectedShape::Baked) => entry["booled"] = json!(true),
    }
    Ok((geometry_key, entry))
}

/// 一根记录的清单摘要与条目数。同一 `geometry_id` 只算一条（后者覆盖前者），与它取代的
/// 内联实现逐位相同。
pub(crate) fn manifest_hash(records: &[ProjectedGeometry]) -> anyhow::Result<(String, usize)> {
    manifest_hash_of(records)
}

/// [`manifest_hash`] 的借用形——同一份配方、同一个函数体，只是不要求记录连成一片。
///
/// 单元级发布（ADR-080 D5）的回执是存储在**写锁里**、delta 落进去之后按这根此刻的全部记录
/// 盖的；那时记录散在 `BTreeMap` 里，按 record_id 一条条取出来的是借用，整根克隆一遍再喂
/// `&[ProjectedGeometry]` 就是白扛一次 O(根内记录数) 的复制。三条路（重生成 / 刚体前移 /
/// 单元级）从这里取的章对同一批记录逐字相同，对拍门比的正是它。
pub(crate) fn manifest_hash_of<'a>(
    records: impl IntoIterator<Item = &'a ProjectedGeometry>,
) -> anyhow::Result<(String, usize)> {
    let mut entries = BTreeMap::<String, Value>::new();
    for record in records {
        let (key, entry) = manifest_entry(record)?;
        entries.insert(key, entry);
    }
    let digest = hex::encode(Sha256::digest(serde_json::to_vec(&entries)?));
    Ok((digest, entries.len()))
}

/// 一件几何落盘用的网格与内容键——[`bake_element`] 发布的就是这一份；粗网格回填
/// （`mesh_lod1`）只算身份、不写 L0 时也走这里，两边不许各算各的。
pub(crate) fn element_mesh(element: &GeneratedElement) -> anyhow::Result<(PlantMesh, String)> {
    match element.primitive_instance.as_ref() {
        Some(instance) => {
            let canonical = canonical_primitive_mesh(instance.key)?;
            let mesh = crate::fast_model::manifold_csg::manifold_to_plant_mesh(&canonical);
            let mesh_id = canonical_mesh_id(instance.key, &mesh);
            Ok((mesh, mesh_id))
        }
        None => {
            let mesh = local_generated_mesh(element);
            let mesh_id = baked_mesh_id(&mesh);
            Ok((mesh, mesh_id))
        }
    }
}

pub(crate) fn local_generated_mesh(element: &GeneratedElement) -> PlantMesh {
    let inverse_world = element.world.inverse();
    let mut mesh = element.parts.as_deref().map_or_else(
        || {
            let local = element.solid.transform(&dmat4_to_affine4x3(inverse_world));
            crate::fast_model::manifold_csg::manifold_to_plant_mesh(&local)
        },
        |parts| {
            crate::fast_model::manifold_csg::manifolds_to_transformed_plant_mesh(
                parts,
                inverse_world,
            )
        },
    );
    // Catalogue parts are evaluated in world coordinates and localized here.
    // A pure element translation can leave ~1e-14 cancellation residue where
    // the catalogue-local coordinate is exactly zero. PlantMesh stores f32, so
    // canonicalizing only this numerical noise keeps its content id invariant
    // without changing any CAD-scale feature.
    for vertex in &mut mesh.vertices {
        if vertex.x.abs() < 1e-10 {
            vertex.x = 0.0;
        }
        if vertex.y.abs() < 1e-10 {
            vertex.y = 0.0;
        }
        if vertex.z.abs() < 1e-10 {
            vertex.z = 0.0;
        }
    }
    mesh
}

/// 按路径 + 长度 + mtime + 会话记一个文件的身份。**只给模板** `attlib.dat`（静态文件，换版就是换文件）用；
/// dabacon 库文件的身份改走冻结锚 `frozen_source::model_file_identity`（ADR-089 D2）——E3D 每次会话都改写每个打开
/// 库的文件头，mtime 不是库文件内容的身份。
pub(crate) fn model_file_identity_digest(path: &Path, session: u32) -> anyhow::Result<String> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("read model target identity {}", path.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut digest = Sha256::new();
    digest.update(canonical.to_string_lossy().as_bytes());
    digest.update(metadata.len().to_le_bytes());
    digest.update(modified.to_le_bytes());
    digest.update(session.to_le_bytes());
    Ok(hex::encode(digest.finalize()))
}

fn persisted_world_transform(
    geometry_id: &GeometryId,
    primitive: Option<&e3d_model::primitive_instance::PrimitiveInstance>,
    world: DMat4,
) -> DMat4 {
    if matches!(geometry_id, GeometryId::ImpliedTube { .. }) {
        primitive
            .map(|instance| world * instance.local_transform)
            .unwrap_or(world)
    } else {
        world
    }
}

/// Remove every projected e3d-model geometry owned by one generation root,
/// and retire that root's overlay receipt in the same outcome.
///
/// Derived geometry (implied tubes) lives under the root's ancestor index, so
/// it is dropped with the rest of the root. Shared content-addressed `.mesh`
/// files are not deleted; the durable rows are removed by the caller's
/// tombstone (`render_spooled_scope_deletion`).
///
/// Record removals still go through [`apply_geometry_delta`] so a durable
/// write-behind sees the deletion. That commit does not replace the resident
/// receipt. Leaving the receipt in place makes `/model/records` fail with
/// `memory overlay root … is incomplete: receipt expects N, store has 0`.
/// The second delete then finds no records and would return `deleted` while
/// that receipt stays. Retiring the receipt even when the store is already
/// empty closes that hole.
pub async fn delete_persisted_geometry_root(root: RefNo) -> anyhow::Result<usize> {
    let store = ModelMemoryStore::global();
    let mut by_dbnum = BTreeMap::<u32, Vec<GeometryId>>::new();
    for record in store.records_of_root(ProjectionScope::Current, root) {
        by_dbnum
            .entry(record.dbnum)
            .or_default()
            .push(record.geometry_id);
    }

    let mut removed = 0;
    for (dbnum, geometry_ids) in by_dbnum {
        removed += apply_geometry_delta(
            dbnum,
            0,
            Vec::new(),
            geometry_ids,
            &BTreeMap::new(),
            Path::new("."),
        )
        .await?
        .removed;
    }
    store.retire_deleted_root(ProjectionScope::Current, root);
    Ok(removed)
}

/// Full generation must never regress the projection. With the source pinned to
/// the file's latest session this only trips when the file itself was rolled
/// back or replaced, which is a file anomaly, not a window to skip.
async fn ensure_not_older_than_persisted(dbnum: u32, source_sesno: u32) -> anyhow::Result<()> {
    let projected = ModelMemoryStore::global().published_sesno(ProjectionScope::Current, dbnum);
    anyhow::ensure!(
        session_not_stale(projected, source_sesno),
        "stale e3d-model session {source_sesno} cannot overwrite projected session {}",
        projected.unwrap_or_default()
    );
    Ok(())
}

/// An explicit window target that is older than what is already projected has
/// been superseded (ADR-054 constraint 5): the caller settles it as covered.
async fn persisted_session_newer_than(
    dbnum: u32,
    target_sesno: u32,
) -> anyhow::Result<Option<u32>> {
    Ok(ModelMemoryStore::global()
        .published_sesno(ProjectionScope::Current, dbnum)
        .filter(|projected| *projected > target_sesno))
}

fn session_not_stale(persisted: Option<u32>, source_sesno: u32) -> bool {
    persisted.is_none_or(|sesno| source_sesno >= sesno)
}

fn increment_failure_count(report: &IncrementReport) -> usize {
    report.regen_failed.len()
        + report.derived_failed.len()
        + report.unresolved.len()
        + report.derived_stale_unreadable.len()
}

fn note_mesh_write(report: &mut E3dPersistReport, outcome: MeshWrite) {
    match outcome {
        MeshWrite::Written => report.mesh_written += 1,
        MeshWrite::Reused => report.mesh_reused += 1,
        MeshWrite::Repaired => report.mesh_repaired += 1,
    }
}

pub(crate) fn scan_index(path: &Path, sesno: Option<u32>) -> anyhow::Result<SourceIndex> {
    let mut engine = match sesno {
        Some(value) => ReadOnlyEngine::open_at(path, value),
        None => ReadOnlyEngine::open(path),
    }?;
    let mut indexed = BTreeSet::new();
    let mut owners = BTreeMap::new();
    for element in engine.scan_elements(ScanTier::Header)? {
        let element = element?;
        indexed.insert((element.refno.word0, element.refno.word1));
        owners.insert((element.refno.word0, element.refno.word1), element.owner);
    }
    let roots = owners
        .iter()
        .filter_map(|(&(word0, word1), &owner)| {
            let refno = RefNo::new(word0, word1);
            (owner == refno || !owner.is_valid() || !indexed.contains(&(owner.word0, owner.word1)))
                .then_some(refno)
        })
        .collect();
    Ok(SourceIndex {
        dbnum: engine.descriptor().db_mark,
        roots,
        owners: Arc::new(owners),
    })
}

pub fn parse_refno(value: &str) -> anyhow::Result<RefNo> {
    let normalized = value.trim().trim_start_matches('=').replace('_', "/");
    let (word0, word1) = normalized
        .split_once('/')
        .with_context(|| format!("invalid refno {value}"))?;
    Ok(RefNo::new(word0.parse()?, word1.parse()?))
}

pub(crate) fn geometry_source_refno(id: &GeometryId) -> anyhow::Result<RefNo> {
    match id {
        GeometryId::Element { refno } => parse_refno(refno),
        GeometryId::ImpliedTube {
            container_refno, ..
        } => parse_refno(container_refno),
    }
}

fn ancestor_chain(refno: RefNo, owners: &BTreeMap<(u32, u32), RefNo>) -> anyhow::Result<Vec<i64>> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = refno;
    loop {
        if !seen.insert((current.word0, current.word1)) {
            bail!("OWNER cycle at {current}");
        }
        chain.push((((current.word0 as u64) << 32) | current.word1 as u64) as i64);
        let Some(&owner) = owners.get(&(current.word0, current.word1)) else {
            break;
        };
        if owner == current
            || !owner.is_valid()
            || !owners.contains_key(&(owner.word0, owner.word1))
        {
            break;
        }
        current = owner;
    }
    Ok(chain)
}

/// 一个 f32 进 JSON 数值，**不放大精度**。
///
/// `json!` 走的是 `From<f32> for Value`，而它做的是 `value as f64`：`0.1f32` 于是印成
/// `0.10000000149011612`——20 个字符里 12 个是 f32 根本没有的位数。这些位不是精度，是
/// 二进制展开的尾巴，读侧一位都用不上，却要按字节付钱：落库时间几乎全按 SQL 字节走
/// （2026-09-14 AMS 7997 真写 RocksDB 实测 `commit(ms) ≈ 37.5 + 0.231 × sql_KB`，R²=0.84）。
///
/// 这里先把 f32 印成**它自己的最短往返写法**，再解回 f64。代价是存下来的 f64 不再逐位
/// 等于 `value as f64`，差一个 f32 ULP 以内——几何本来就只有 f32 这一份真值，
/// 拿它跟现算的 f32 做逐位比对的地方不该存在。非有限值照旧落 `null`，与 `json!` 一致。
pub(crate) fn f32_value(value: f32) -> Value {
    json!(value.to_string().parse::<f64>().unwrap_or(value as f64))
}

/// [`f32_value`] 的定长数组版：坐标、包围盒角点、四元数都走它。
pub(crate) fn f32_values<const N: usize>(values: [f32; N]) -> Value {
    Value::Array(values.into_iter().map(f32_value).collect())
}

pub(crate) fn transform_value(matrix: DMat4) -> Value {
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    json!({
        "translation": f32_values([translation.x as f32, translation.y as f32, translation.z as f32]),
        "rotation": f32_values([rotation.x as f32, rotation.y as f32, rotation.z as f32, rotation.w as f32]),
        "scale": f32_values([scale.x as f32, scale.y as f32, scale.z as f32])
    })
}

fn point3(point: &parry3d::math::Point<f32>) -> [f32; 3] {
    [point.x, point.y, point.z]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::model_memory_store::packed_refno;
    use std::collections::HashMap;

    /// ADR-084（内存控制计划 P3）：生成输入缓存按**库数**设上限——每库一条（同库换 sesno 替换、不算逐出），
    /// 超过上限时最久没被生成用到的库整份出，**命中也刷新**；0 = 不限；`forget_dbnum` 不计入逐出数；
    /// 被替 / 被逐的值原样交还给调用方（在锁外 drop）。
    #[test]
    fn the_generation_input_cache_keeps_at_most_n_dbnums_and_evicts_the_least_recently_used() {
        let key = |dbnum: u32, sesno: u32| (dbnum, sesno, format!("id-{dbnum}"));
        let mut cache: DbnumLru<&'static str> = DbnumLru::new();

        // 0 = 不限：三个库都留着。
        assert!(cache.insert(key(7997, 1), "7997@1", 0).is_empty());
        assert!(cache.insert(key(8000, 1), "8000@1", 0).is_empty());
        assert!(cache.insert(key(1112, 1), "1112@1", 0).is_empty());
        assert_eq!(cache.dbnum_count(), 3);
        assert_eq!(cache.evicted_dbnums, 0);

        // 同库换 sesno：替换，不算逐出，旧值交还。
        assert_eq!(cache.insert(key(7997, 2), "7997@2", 0), vec!["7997@1"]);
        assert_eq!(cache.dbnum_count(), 3);
        assert_eq!(cache.evicted_dbnums, 0);
        assert_eq!(cache.get_touch(&key(7997, 1)), None, "旧 sesno 的键已不在");

        // 命中刷新：8000 最旧 → 摸一下 8000，最旧变成 1112。
        assert_eq!(cache.get_touch(&key(8000, 1)), Some("8000@1"));

        // 上限 3、再进第四个库：最久没用的 1112 出，刚进的 7998 一定留。
        assert_eq!(cache.insert(key(7998, 1), "7998@1", 3), vec!["1112@1"]);
        assert_eq!(cache.dbnum_count(), 3);
        assert_eq!(cache.evicted_dbnums, 1);
        assert_eq!(cache.get_touch(&key(1112, 1)), None);

        // 上限收到 1：留刚进的那一份，其余按序出（7997@2 比 8000@1 更旧——8000 被摸过）。
        let displaced = cache.insert(key(5052, 1), "5052@1", 1);
        assert_eq!(displaced.len(), 3, "{displaced:?}");
        assert!(
            displaced.contains(&"7997@2")
                && displaced.contains(&"8000@1")
                && displaced.contains(&"7998@1")
        );
        assert_eq!(cache.dbnum_count(), 1);
        assert_eq!(cache.evicted_dbnums, 4);
        assert_eq!(cache.get_touch(&key(5052, 1)), Some("5052@1"));

        // 序在别的库之间也成立：上限 2，最旧的先出、被摸过的留。
        assert!(cache.insert(key(6000, 1), "6000@1", 2).is_empty());
        assert_eq!(cache.get_touch(&key(5052, 1)), Some("5052@1"));
        assert_eq!(cache.insert(key(6001, 1), "6001@1", 2), vec!["6000@1"]);
        assert_eq!(cache.evicted_dbnums, 5);

        // forget：整库拿走、不计逐出。
        assert_eq!(cache.forget_dbnum(5052), vec!["5052@1"]);
        assert_eq!(cache.forget_dbnum(5052), Vec::<&str>::new());
        assert_eq!(cache.dbnum_count(), 1);
        assert_eq!(cache.evicted_dbnums, 5);
        let ticks: Vec<(u32, u64)> = cache.iter().map(|(key, _, tick)| (key.0, tick)).collect();
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].0, 6001);
    }

    /// f32 印进 SQL 时只许带 f32 有的那几位。`json!` 的 `value as f64` 会把二进制展开的
    /// 尾巴一起印出来，而落库时间几乎全按字节走——这条钉住的是**字节**，不是数值。
    #[test]
    fn a_f32_renders_with_only_the_digits_it_actually_has() {
        assert_eq!(f32_value(0.1).to_string(), "0.1");
        assert_eq!(json!(0.1_f32).to_string(), "0.10000000149011612");
        assert_eq!(f32_value(1.0).to_string(), "1.0");
        assert_eq!(
            f32_values([0.1_f32, -2.5, 0.0]).to_string(),
            "[0.1,-2.5,0.0]"
        );
        // 解回来还是同一个 f32：短的只是写法，不是精度。
        for value in [0.1_f32, -12345.678, 1.0e-7, f32::MAX, f32::MIN_POSITIVE] {
            let rendered = f32_value(value).to_string();
            assert_eq!(
                rendered.parse::<f64>().expect("finite") as f32,
                value,
                "{rendered} 必须往返回同一个 f32"
            );
        }
        // 非有限值与 `json!` 今天一样落 null，不会渲出 `NaN` 这种 SurrealQL 认不得的字面量。
        assert_eq!(f32_value(f32::NAN).to_string(), "null");
        assert_eq!(f32_value(f32::INFINITY).to_string(), "null");
    }

    #[test]
    fn persist_report_merge_keeps_failed_root_identity() {
        let root = RefNo::new(23399, 13);
        let mut total = E3dPersistReport::default();
        total.merge_counts(E3dPersistReport {
            failed: 3,
            failed_roots: vec![(root, "three geometry incidents".into())],
            ..Default::default()
        });
        assert_eq!(total.failed, 3);
        assert_eq!(
            total.failed_roots,
            vec![(root, "three geometry incidents".into())]
        );
    }

    #[test]
    fn only_a_root_with_nothing_to_lose_publishes_a_partial_manifest() {
        // Cold root: the discard would "preserve" an empty snapshot.
        assert_eq!(
            incomplete_root_policy(false, false),
            IncompleteRoot::Publish
        );
        // A receipt, geometry rows, or both — all of them can lose something.
        assert_eq!(incomplete_root_policy(true, false), IncompleteRoot::Discard);
        assert_eq!(incomplete_root_policy(false, true), IncompleteRoot::Discard);
        assert_eq!(incomplete_root_policy(true, true), IncompleteRoot::Discard);
    }

    /// 生产端烘的是「算出来的那些」，丢不丢由消费端定——它看不见发布状态
    /// （`memory_generation_reads_only_the_e3d_source_projection` 钉的正是这条）。
    /// 让生产端自己判一次，按需生成那条路就只拿得到一个空候选：SLYK
    /// `21491/17101` 一个缺目录件把整根 17 个构件全按下去，就是这么来的。
    #[test]
    fn the_producer_bakes_every_survivor_and_leaves_the_verdict_to_the_consumer() {
        let source = service_source();
        let worker = source
            .split_once("fn spawn_generation_worker(")
            .expect("generation worker")
            .1
            .split_once("\n}\n")
            .expect("generation worker end")
            .0;
        assert!(
            !worker.contains("IncompleteRoot"),
            "发布状态在消费端，生产端判不了这根丢不丢"
        );
        assert!(
            production_half(&source).contains("if incomplete == IncompleteRoot::Discard {"),
            "丢弃不完整候选只能发生在拿得到发布回执的那一侧"
        );
    }

    #[tokio::test]
    #[ignore = "read-only AMS catalogue source diagnosis"]
    async fn ams_catalogue_7000_reference_inventory() -> anyhow::Result<()> {
        let service = E3dModelService::from_current().await?;
        let option = aios_core::get_db_option();
        let membership = mdb_membership::resolve(option, &option.project_name, &option.mdb_name)?;
        for database in membership
            .databases()
            .iter()
            .filter(|db| db.dbnum == 7000 || db.dbnum == 7015)
        {
            println!("MDB_DECLARATION {database:?}");
        }
        println!("MDB_PROBLEMS {:?}", membership.problems());
        for pin in current_mdb_sources()?
            .0
            .iter()
            .filter(|pin| pin.dbnum == 7000 || pin.dbnum == 7015)
        {
            println!(
                "MDB_PIN dbnum={} type={} file={}",
                pin.dbnum,
                pin.db_type,
                pin.file.display()
            );
        }
        for file in [
            "D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7000_0001",
            "D:/AVEVA/Projects/E3D3.1/AvevaCatalogue/acp000/acp7000_0001",
        ] {
            let mut view = DbView::open(file)?;
            let session = view
                .walk_sessions()?
                .first()
                .context("catalogue session")?
                .sesno;
            let set = Arc::new(DbSet::with_attlib_file(
                service.schema.template_dir().join("attlib.dat"),
            )?);
            set.add_db(DbFilePin {
                file: file.into(),
                template: template_file_for(service.schema.template_dir(), "CATA")?,
                db_type: Some("CATA".into()),
                sesno: Some(session),
            })?;
            for id in [11893, 19297, 32948, 11899, 19303, 47230] {
                let element = set.element(RefNo::new(15192, id));
                println!(
                    "CATA_REF file={file} session={session} ref=15192/{id} result={:?}",
                    element.element_type()
                );
            }
        }
        let target = service.build_set(7015, None)?;
        for id in [11893, 19297, 32948, 11899, 19303, 47230] {
            anyhow::ensure!(
                target.element(RefNo::new(15192, id)).element_type().is_ok(),
                "ACP DESI 7015 must resolve catalogue ref 15192/{id} through ACP CATA 7000"
            );
        }
        let pin = service.pin(7015)?;
        let source_sesno = service.source_session(pin)?;
        let index = scan_index(&pin.file, Some(source_sesno))?;
        let mut cache = CatalogueMeshCache::default();
        for root in [
            RefNo::new(23399, 13),
            RefNo::new(23399, 22),
            RefNo::new(23399, 24),
            RefNo::new(23399, 33),
        ] {
            let snapshot = generate_snapshot_from_set(&target, &index, root, &mut cache)?;
            anyhow::ensure!(
                snapshot.report.failed.is_empty(),
                "ACP root {root} still has geometry incidents: {:?}",
                snapshot.report.failed
            );
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "read-only AMS initialization root overlap census"]
    async fn ams_initialization_root_overlap_census() -> anyhow::Result<()> {
        let service = E3dModelService::from_current().await?;
        for dbnum in [7022, 7333] {
            let pin = service.pin(dbnum)?;
            let mut view = DbView::open(&pin.file)?;
            let session = view
                .walk_sessions()?
                .first()
                .context("source session")?
                .sesno;
            let index = scan_index(&pin.file, Some(session))?;
            let roots =
                crate::data_interface::window_root_plan::enumerate_target_roots(dbnum, session)
                    .await?;
            let root_set = roots
                .iter()
                .map(|root| parse_refno(&root.root.to_string()))
                .collect::<anyhow::Result<std::collections::HashSet<_>>>()?;
            let mut nouns = BTreeMap::<String, usize>::new();
            let mut nested = 0;
            for root in roots {
                *nouns.entry(root.noun).or_default() += 1;
                let mut current = parse_refno(&root.root.to_string())?;
                let mut seen = std::collections::HashSet::new();
                while seen.insert(current) {
                    let Some(owner) = index.owners.get(&(current.word0, current.word1)) else {
                        break;
                    };
                    if root_set.contains(owner) {
                        nested += 1;
                        break;
                    }
                    current = *owner;
                }
            }
            println!(
                "ROOT_OVERLAP dbnum={dbnum} session={session} roots={} nested={nested} nouns={nouns:?}",
                root_set.len()
            );
            if dbnum == 7022 {
                let set = service.build_set(dbnum, Some(session))?;
                for id in [6, 8, 10, 14] {
                    let element = set.element(RefNo::new(23406, id));
                    println!(
                        "ROOT_INPUT ref=23406/{id} noun={:?} owner={:?} members={:?}",
                        element.element_type(),
                        element.owner()?.map(|e| e.refno()),
                        element.member_refnos()
                    );
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "real AMS 7022 missing-RPATH Core3D behavior"]
    async fn ams_cable_tray_without_rpath_uses_core_origin() -> anyhow::Result<()> {
        let service = E3dModelService::from_current().await?;
        let set = service.build_set(7022, Some(254))?;
        let element = set.element(RefNo::new(23406, 8));
        let placement = e3d_model::cable_tray::placement(&element)?;
        assert_eq!(placement.position, glam::DVec3::ZERO);
        assert_eq!(placement.rotation, glam::DMat3::IDENTITY);
        println!(
            "CORE_MISSING_RPATH ref=23406/8 position={:?} rotation={:?}",
            placement.position, placement.rotation
        );
        let index = scan_index(&service.pin(7022)?.file, Some(254))?;
        let snapshot = generate_snapshot_from_set(
            &set,
            &index,
            RefNo::new(23406, 6),
            &mut CatalogueMeshCache::default(),
        )?;
        anyhow::ensure!(
            snapshot.report.failed.is_empty(),
            "{}",
            summarize_geometry_incidents(&snapshot.report.failed)
        );
        anyhow::ensure!(
            !snapshot.elements.is_empty(),
            "repaired root must produce geometry"
        );
        println!(
            "CORE_MISSING_RPATH_MODEL root=23406/6 elements={} failed={}",
            snapshot.elements.len(),
            snapshot.report.failed.len()
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "read-only AMS unfinished model attribute diagnosis"]
    async fn ams_unfinished_model_attribute_inventory() -> anyhow::Result<()> {
        let service = E3dModelService::from_current().await?;
        for (dbnum, refno, attributes) in [
            (
                7048,
                RefNo::new(23432, 32104),
                vec!["BANG", "POS", "ORI", "TYPE"],
            ),
            (
                7330,
                RefNo::new(23714, 1011),
                vec!["POSS", "POSE", "POS", "ORI"],
            ),
        ] {
            let set = service.build_set(dbnum, None)?;
            let element = set.element(refno);
            for attribute in attributes {
                println!(
                    "MODEL_INPUT dbnum={dbnum} ref={refno} attribute={attribute} value={:?}",
                    element.attribute(attribute)
                );
                if attribute == "BANG" {
                    println!(
                        "MODEL_DESCRIPTOR {:?}",
                        element.extraction()?.find_attribute(attribute)
                    );
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "real AMS finite zero-length section completion"]
    async fn ams_zero_length_section_root_completes() -> anyhow::Result<()> {
        let service = E3dModelService::from_current().await?;
        let pin = service.pin(7330)?;
        let session = service.source_session(pin)?;
        let set = service.build_set(7330, Some(session))?;
        let index = scan_index(&pin.file, Some(session))?;
        let snapshot = generate_snapshot_from_set(
            &set,
            &index,
            RefNo::new(23714, 997),
            &mut CatalogueMeshCache::default(),
        )?;
        anyhow::ensure!(
            snapshot.report.failed.is_empty(),
            "{}",
            summarize_geometry_incidents(&snapshot.report.failed)
        );
        anyhow::ensure!(
            !snapshot.elements.is_empty(),
            "other valid members must survive"
        );
        println!(
            "ZERO_LENGTH_SECTION root=23714/997 elements={} failed={}",
            snapshot.elements.len(),
            snapshot.report.failed.len()
        );
        Ok(())
    }

    /// 字典库挂进 `build_set` 之后：UDA 按名字读得出，而几何**一根管也没多**。
    ///
    /// `:HXYsize` 是 H 族头槽位那条目录表达式 `( :HXYsize[n] )` 要的那一格。挂上字典库
    /// 之前它根本不是个名字（`UnknownAttribute`），挂上之后是。两件事要一起成立：
    /// 读得出，且这 7 个「只差这一格」的头槽位仍然一根管都不长——E3D 在本库自己也不画
    /// （6 条报 `Catalogue error`、`30297` 连报都不报），所以长出来的会是我们凭空造的几何。
    /// 它们不长，是因为风管隐式管身的求值环境**不挂设计元素**（`PREV` 才是那个构件），
    /// UDA 在的是 BRAN 身上——挂字典库并不能把 BRAN 放进那个环境。
    ///
    /// 取证：`_runs/ams-init-1112-7997-20260917-mc/ledger-seg15-frameall.md` §16 / §18。
    #[tokio::test]
    #[ignore = "real AMS 7997 + the project Dictionary databases"]
    async fn ams7997_dictionary_uda_reads_by_name_and_grows_no_duct() -> anyhow::Result<()> {
        use e3d_io::record::descriptor::DescriptorValue;
        use e3d_model::hvac_tube::duct_departure_section;
        use e3d_model::route::RouteEnd;

        let service = E3dModelService::from_current().await?;
        anyhow::ensure!(
            service.uda.is_some(),
            "MDB 声明的字典库一个都没读出来，这条用例证明不了任何事"
        );
        let set = service.build_set(7997, None)?;

        // 读得出：E3D 的 `Q :HXYsize` 在这条支管上答 675 600。
        let branch = set.element(RefNo::new(24381, 59287));
        assert_eq!(
            branch.attribute(":HXYsize")?,
            Some(DescriptorValue::RealArray(vec![675.0, 600.0]))
        );

        // 一根也没多：这 7 个头槽位就是「接上 UDA 就会长出来」的全部。
        for word1 in [30297, 47065, 58284, 59287, 59302, 79980, 91034] {
            let container = set.element(RefNo::new(24381, word1));
            let grown = duct_departure_section(&RouteEnd::ContainerHead(container))?;
            anyhow::ensure!(
                grown.is_none(),
                "24381/{word1} 的头槽位长出了一节 E3D 不画的管身：{grown:?}"
            );
        }

        // 已经在画的那一支照旧：中段走的是构件自己的 `LSTU` 与离开 P 点，不碰 UDA。
        let departure = set.element(RefNo::new(24381, 30293));
        let section = duct_departure_section(&RouteEnd::MemberLeave {
            member: departure,
            ppoint: None,
        })?
        .context("TAPE 24381/30293 的离开侧断面")?;
        assert_eq!(section.source, "/RTUBE2 BOXI /RTUBE2-BOXI");
        match section.section {
            e3d_model::route::TubeSection::Rectangular {
                x_length, z_length, ..
            } => {
                assert_eq!((x_length, z_length), (1460.0, 1520.0));
            }
            other => anyhow::bail!("24381/30293 的中段断面变了：{other:?}"),
        }

        // 管道那一侧也数一遍：ZONE 24381/147826 的 13 条支管，E3D 导出 103 根隐式管身，
        // 引擎在 `e1745dc` 之后逐根对上（§4 / §5）。挂字典库不该动这个数。
        let pin = service.pin(7997)?;
        let session = service.source_session(pin)?;
        let index = scan_index(&pin.file, Some(session))?;
        let snapshot = generate_snapshot_from_set(
            &service.build_set(7997, Some(session))?,
            &index,
            RefNo::new(24381, 147826),
            &mut CatalogueMeshCache::default(),
        )?;
        let tubes = snapshot
            .elements
            .iter()
            .filter(|element| matches!(element.geometry_id, GeometryId::ImpliedTube { .. }))
            .count();
        assert_eq!(tubes, 103, "ZONE 24381/147826 的隐式管身根数变了");
        Ok(())
    }

    /// 头槽位「一根也没多」不只靠求值环境不挂设计元素——**把 BRAN 挂上也长不出来**。
    ///
    /// 目录表达式里的 UDA 按字典的键求值（e3d-io `9508ac4`）：`/HRTUBE1-BOXI.PXLE` 写的键
    /// `0x17EF4B61` 是 `AvevaCatalogue` 字典（acp7006）那条 `HXYsize` 的 `OLDKEY`，而支管的
    /// `675 600` 存在本项目字典（ams5100）那条同名定义的键 `0x2652AB52` 下。六本字典按 CURD
    /// 序合起来，键认成 acp 那条 → 对 BRAN 有效、元素上没值 → unset → 公式未解，印成 E3D
    /// 的写法。这正是 E3D 自己对这些槽位报 `Catalogue error … unable to evaluate … PXLE`
    /// 的落点；同一条支管 `Q :HXYsize` 却答 `675 600`。
    /// GitHub #46 / #47 探针（只读真部署闭包）：同一个 CATA 库号在闭包里有几份文件时，三处各认哪一份——
    /// sweep 观察（监控目录里按文件头认出的 CATA 经 `select_catalogue_candidates` + `catalogue_project_priority`
    /// 选正本，数据面依赖行同一份清单）、目录版本核对（`model_source::source_file_lookup` = MDB 声明那份）、
    /// 生成（`select_dependency_file`：目标 DESI 所在项目里有同类型副本就用它）。逐个 (DESI 项目, CATA) 打印，
    /// 末行 `CATA_SELECTION_SUMMARY` 汇总；另数 sweep 观察得到、生成却解不出的未声明 CATA（#46）。
    #[tokio::test]
    #[ignore = "real MDB closure from DbOption.toml"]
    async fn live_catalogue_file_selection_across_observation_reconcile_and_generation()
    -> anyhow::Result<()> {
        use crate::data_interface::initialization_phase::{
            CatalogueCandidate, select_catalogue_candidates,
        };
        let same_file = |a: &Path, b: &Path| {
            a.to_string_lossy().to_ascii_lowercase() == b.to_string_lossy().to_ascii_lowercase()
        };
        let option = aios_core::get_db_option();
        let (pins, locator, project_files) = current_mdb_source_inventory()?;
        // 设计库按所在项目分组：生成时 `preferred_project` 就是它。
        let mut library_projects: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for pin in pins
            .iter()
            .filter(|pin| crate::data_interface::model_source::is_element_db(&pin.db_type))
        {
            let dbnum = pin.dbnum as u32;
            if let Some((project, _)) = locator.file_of(dbnum) {
                library_projects
                    .entry(project.to_ascii_lowercase())
                    .or_default()
                    .push(dbnum);
            }
        }
        // 观察那一份：与 sweep 同一个选主函数、同一个闭包项目序、同一个优先级配置。
        let plan = crate::data_interface::project_paths::plan_watch_dirs(option);
        let mut candidates = Vec::new();
        for dir in plan.dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for path in entries.flatten().map(|entry| entry.path()) {
                if !path.is_file() {
                    continue;
                }
                let Ok(header) = crate::data_interface::db_file_header::DbFileHeader::read(&path)
                else {
                    continue;
                };
                if !header.db_type.eq_ignore_ascii_case("CATA") {
                    continue;
                }
                let Some(project) = crate::data_interface::project_paths::project_of_path(&path)
                else {
                    continue;
                };
                candidates.push(CatalogueCandidate {
                    project,
                    dbnum: header.dbnum as u32,
                    path,
                });
            }
        }
        let selection = select_catalogue_candidates(
            candidates.clone(),
            &crate::data_interface::project_paths::project_names(option),
            &crate::options::catalogue_project_priority(),
        );
        let observed: BTreeMap<u32, (String, PathBuf)> = selection
            .selected
            .iter()
            .map(|candidate| {
                (
                    candidate.dbnum,
                    (candidate.project.clone(), candidate.path.clone()),
                )
            })
            .collect();
        // 同号多份：生成能看见的副本（MDB 声明 + 遮蔽）与监控目录里的候选，按项目去重。
        let mut copies: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
        for ((project, dbnum), file) in project_files.iter() {
            if file.db_type.eq_ignore_ascii_case("CATA") {
                copies.entry(*dbnum).or_default().insert(project.clone());
            }
        }
        for candidate in &candidates {
            copies
                .entry(candidate.dbnum)
                .or_default()
                .insert(candidate.project.to_ascii_lowercase());
        }
        let mut same_number = 0usize;
        let mut pairs = 0usize;
        let mut against_reconcile = 0usize;
        let mut against_observed = 0usize;
        for (&cata, projects) in copies.iter().filter(|(_, projects)| projects.len() > 1) {
            same_number += 1;
            let reconcile = crate::data_interface::model_source::source_file_lookup(cata)?;
            let observed_file = observed.get(&cata);
            let latest = |path: &Path| {
                crate::data_interface::model_source::latest_source_version_of_file(
                    cata, "CATA", path,
                )
                .map(|version| version.sesno.to_string())
                .unwrap_or_else(|error| format!("? ({error})"))
            };
            println!(
                "CATA {cata}: copies in {projects:?}; observed {:?}; reconcile (MDB pin) {:?}",
                observed_file.map(|(project, path)| format!(
                    "{project} {} @{}",
                    path.display(),
                    latest(path)
                )),
                reconcile
                    .as_ref()
                    .map(|(_, path)| format!("{} @{}", path.display(), latest(path))),
            );
            for ((project, dbnum), file) in project_files.iter() {
                if *dbnum == cata {
                    println!(
                        "  copy {project}: {} @{}",
                        file.path.display(),
                        latest(&file.path)
                    );
                }
            }
            for (project, libraries) in &library_projects {
                pairs += 1;
                let Some(generated) =
                    select_dependency_file(&locator, &project_files, Some(project), cata)
                else {
                    println!(
                        "  DESI project {project} ({} libs): generation resolves nothing",
                        libraries.len()
                    );
                    continue;
                };
                let off_reconcile = reconcile
                    .as_ref()
                    .is_none_or(|(_, path)| !same_file(path, &generated.path));
                let off_observed =
                    observed_file.is_none_or(|(_, path)| !same_file(path, &generated.path));
                against_reconcile += usize::from(off_reconcile);
                against_observed += usize::from(off_observed);
                println!(
                    "  DESI project {project} ({} libs, e.g. {:?}): generation reads {} {}{}{}",
                    libraries.len(),
                    &libraries[..libraries.len().min(3)],
                    generated.project,
                    generated.path.display(),
                    if off_reconcile { " ≠reconcile" } else { "" },
                    if off_observed { " ≠observed" } else { "" },
                );
            }
        }
        // #46：sweep 观察得到、MDB 没声明的 CATA——生成的 resolver 只认 MDB 声明的库，解不出它。
        let undeclared: Vec<String> = observed
            .iter()
            .filter(|(dbnum, _)| locator.db_type_of(**dbnum).is_none())
            .map(|(dbnum, (project, _))| format!("{dbnum}@{project}"))
            .collect();
        println!(
            "undeclared CATA observed by the sweep (generation cannot resolve them): {} {:?}",
            undeclared.len(),
            &undeclared[..undeclared.len().min(40)]
        );
        println!(
            "CATA_SELECTION_SUMMARY: DESI projects {} / CATA with >1 copy {same_number} / (DESI project, CATA) pairs {pairs} / generation != reconcile {against_reconcile} / generation != observed {against_observed} / observed {} / undeclared observed {} / selection blockers {}",
            library_projects.len(),
            observed.len(),
            undeclared.len(),
            selection.blockers.len()
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "real AMS 7997 + the project Dictionary databases"]
    async fn ams7997_head_slot_expression_stays_unset_by_key_even_with_the_bran_attached()
    -> anyhow::Result<()> {
        use e3d_io::db_param_env::DbElementParamEnv;
        use e3d_io::record::catalogue_eval::{AttributeNumber, ExpressionForm};
        use e3d_io::record::descriptor::DescriptorValue;

        let service = E3dModelService::from_current().await?;
        let set = service.build_set(7997, None)?;
        let dictionary = set
            .uda_catalog()
            .context("MDB 声明的字典库一个都没读出来")?;
        assert_eq!(
            dictionary
                .by_key(0x17EF_4B61)
                .map(|definition| definition.refno),
            Some(RefNo::new(15198, 55)),
            "目录写下的键该由 acp7006 的 /UDA/HVAC/HXYsize（OLDKEY）认下"
        );

        let branch = set.element(RefNo::new(24381, 59287));
        assert_eq!(
            branch.uda_by_key(0x2652_AB52)?,
            Some(DescriptorValue::RealArray(vec![675.0, 600.0])),
            "本项目字典那条键下有值"
        );
        assert_eq!(
            branch.uda_by_key(0x17EF_4B61)?,
            None,
            "目录指的那条键下没值"
        );

        let boxi = set
            .find_named(7002, "/HRTUBE1-BOXI")?
            .into_iter()
            .map(|refno| set.element(refno))
            .next()
            .context("/HRTUBE1-BOXI 在 acp7002 里")?;
        let attached = DbElementParamEnv::new(Some(branch), Some(boxi.clone()));
        assert_eq!(
            boxi.get_number("PXLE", &attached)?,
            AttributeNumber::Unresolved {
                form: ExpressionForm::Pml,
                text: Some("( :HXYsize[2 ] )".into()),
            }
        );
        Ok(())
    }

    fn baked_element(refno: RefNo, mesh_id: &str) -> BakedElement {
        BakedElement {
            geometry_id: GeometryId::Element {
                refno: refno.to_string(),
            },
            refno,
            noun: "EQUI".into(),
            is_invalid_tubi: false,
            world: DMat4::IDENTITY,
            primitive_instance: None,
            mesh_id: mesh_id.into(),
            mesh_write: MeshWrite::Reused,
            mesh_triangles: 0,
            world_bounds: [[0.0; 3], [1.0; 3]],
            local_bounds: None,
            room_edges: Vec::new(),
        }
    }

    fn baked_tube(branch: RefNo, route_ordinal: u32, mesh_id: &str) -> BakedElement {
        BakedElement {
            geometry_id: GeometryId::ImpliedTube {
                container_refno: branch.to_string(),
                route_ordinal,
                from_refno: "24383/10".into(),
                to_refno: "24383/20".into(),
            },
            refno: branch,
            noun: "TUBI".into(),
            is_invalid_tubi: false,
            world: DMat4::IDENTITY,
            primitive_instance: None,
            mesh_id: mesh_id.into(),
            mesh_write: MeshWrite::Reused,
            mesh_triangles: 0,
            world_bounds: [[0.0; 3], [1.0; 3]],
            local_bounds: None,
            room_edges: Vec::new(),
        }
    }

    #[test]
    fn an_invalid_implied_tube_survives_projection_for_the_ui() {
        let root = RefNo::new(24383, 100);
        let branch = RefNo::new(24383, 73948);
        let owners = BTreeMap::from([
            ((root.word0, root.word1), root),
            ((branch.word0, branch.word1), root),
        ]);
        let mut tube = baked_tube(branch, 0, "diagnostic_tube");
        tube.is_invalid_tubi = true;

        let (commit, _) = project_geometry_delta(7997, 42, vec![tube], Vec::new(), None, &owners)
            .expect("project invalid tube");

        assert!(commit.upserts[0].tube.as_ref().unwrap().invalid);
    }

    fn historical_fixture_element(refno: RefNo, translation: glam::DVec3) -> GeneratedElement {
        let world = DMat4::from_translation(translation);
        let parts = vec![
            manifold_csg::Manifold::cube(20.0, 20.0, 20.0, true)
                .transform(&dmat4_to_affine4x3(world)),
        ];
        GeneratedElement {
            geometry_id: GeometryId::Element {
                refno: refno.to_string(),
            },
            refno,
            noun: "VALV".into(),
            is_invalid_tubi: false,
            name: None,
            solid: manifold_csg::Manifold::compose(&parts),
            parts: Some(parts),
            part_catalogue_nouns: Vec::new(),
            degenerate_parts: Vec::new(),
            part_record_repeats: Vec::new(),
            world,
            primitive_instance: None,
            analytic_primitive: None,
            negatives_applied: 0,
            own_ngms_cuts: 0,
            negatives_skipped: vec![],
            joint_cuts_explained: vec![],
            notes: vec![],
        }
    }

    fn historical_fixture() -> (Vec<GeneratedElement>, BTreeMap<(u32, u32), RefNo>) {
        let root = RefNo::new(24383, 100);
        let first = RefNo::new(24383, 201);
        let second = RefNo::new(24383, 202);
        let owners = BTreeMap::from([
            ((root.word0, root.word1), root),
            ((first.word0, first.word1), root),
            ((second.word0, second.word1), root),
        ]);
        let elements = vec![
            historical_fixture_element(first, glam::DVec3::new(0.0, 0.0, 0.0)),
            historical_fixture_element(second, glam::DVec3::new(100.0, 0.0, 0.0)),
        ];
        (elements, owners)
    }

    /// 任务 2.3 / R6.15：共用的 bake + project 与它取代的 Legacy 组合
    /// `project_geometry_delta(…, bake_elements(…, None), [], None, …)` 产出逐位相同的记录与计数，
    /// 不置位的取消标志与不给标志结果相同。
    #[test]
    fn project_historical_records_matches_the_legacy_bake_then_project() {
        let (elements, owners) = historical_fixture();
        let legacy_dir = tempfile::tempdir().unwrap();
        let (legacy, legacy_report) = project_geometry_delta(
            24383,
            7,
            bake_elements(elements, legacy_dir.path(), None).unwrap(),
            Vec::new(),
            None,
            &owners,
        )
        .unwrap();

        let shared_dir = tempfile::tempdir().unwrap();
        let shared = project_historical_records(
            24383,
            7,
            historical_fixture().0,
            &owners,
            shared_dir.path(),
            None,
        )
        .unwrap();
        assert_eq!(shared.records(), legacy.upserts.as_slice());
        assert_eq!(shared.commit.tube_branch_resets, legacy.tube_branch_resets);
        assert!(shared.commit.removals.is_empty() && shared.commit.receipt.is_none());
        assert_eq!(shared.report.upserted, legacy_report.upserted);
        assert_eq!(shared.report.mesh_ids, legacy_report.mesh_ids);
        assert_eq!(shared.report.baked_instances, legacy_report.baked_instances);

        let flag = std::sync::atomic::AtomicBool::new(false);
        let with_flag = project_historical_records(
            24383,
            7,
            historical_fixture().0,
            &owners,
            shared_dir.path(),
            Some(&flag),
        )
        .unwrap();
        assert_eq!(with_flag.into_records(), legacy.upserts);
    }

    /// 取消标志在 bake 每个元素之前查：置位后一个网格都不写，错误能按类型认出来。
    #[test]
    fn project_historical_records_stops_before_baking_once_cancelled() {
        let (elements, owners) = historical_fixture();
        let dir = tempfile::tempdir().unwrap();
        let flag = std::sync::atomic::AtomicBool::new(true);

        let error =
            project_historical_records(24383, 7, elements, &owners, dir.path(), Some(&flag))
                .unwrap_err();

        assert!(
            error
                .downcast_ref::<HistoricalProjectionCancelled>()
                .is_some(),
            "{error:#}"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// B/T2：bake 时房间判定看到的这件几何（`baked_room_input`）与它随后变成的投影记录（`project_geometry_delta`）
    /// 在 `RoomInput` 的五格上**逐字相同**——构件（隐式管身挂容器）、标签（投影记录 id）、世界盒、网格 id、
    /// 「网格 → 世界」（与投影记录的 `mesh_to_world()` 同一条公式）。三种形状各一例：烘焙实体、带标准图元的元素、
    /// 带标准图元的隐式管身。而且那条公式对三种形状都是**本地摆放只乘一次**：烘焙实体 = 世界变换；标准图元元素
    /// = 世界 × 本地；标准管身的本地摆放已由 `persisted_world_transform` 折进存下的世界变换，`mesh_to_world()`
    /// 不再乘——此前两边一致地多乘了一次（`world × local × local`），判定点落在 Plant UI 画的网格之外，
    /// 与库上 `tubi_relate` 行回读（实例变换单位阵）算出的边也对不上（2026-09-17 修）。
    #[test]
    fn bake_time_room_input_matches_the_projected_record() {
        use crate::fast_model::room_model::RoomInput;
        let root = RefNo::new(24383, 100);
        let branch = RefNo::new(24383, 73948);
        let equi = RefNo::new(24383, 200);
        let owners = BTreeMap::from([
            ((root.word0, root.word1), root),
            ((branch.word0, branch.word1), root),
            ((equi.word0, equi.word1), root),
        ]);
        let world = DMat4::from_scale_rotation_translation(
            glam::DVec3::new(1.0, 1.0, 1.0),
            glam::DQuat::from_rotation_z(0.7),
            glam::DVec3::new(10.0, 20.0, 30.0),
        );
        let local = DMat4::from_scale(glam::DVec3::new(100.0, 100.0, 3000.0));
        let primitive = e3d_model::primitive_instance::PrimitiveInstance {
            key: e3d_model::primitive_instance::PrimitiveMeshKey::CylinderV3 { segments: 32 },
            local_transform: local,
        };
        let mut baked = baked_element(equi, "baked-mesh");
        baked.world = world;
        baked.world_bounds = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
        let mut canonical = baked_element(equi, "canonical-mesh");
        canonical.world = world;
        canonical.primitive_instance = Some(primitive.clone());
        canonical.local_bounds = Some([[0.0; 3], [1.0; 3]]);
        let mut tube = baked_tube(branch, 3, "tube-mesh");
        tube.world = world;
        tube.primitive_instance = Some(primitive.clone());
        tube.local_bounds = Some([[0.0; 3], [1.0; 3]]);

        for element in [baked, canonical, tube] {
            let shape = format!("{:?}", element.geometry_id);
            let placed_once = if element.primitive_instance.is_some() {
                world * local
            } else {
                world
            };
            let input = baked_room_input(
                &element.geometry_id,
                element.primitive_instance.as_ref(),
                element.world,
                element.world_bounds,
                &element.mesh_id,
            )
            .expect("bake-time room input");
            let (commit, _) =
                project_geometry_delta(24383, 42, vec![element], Vec::new(), None, &owners)
                    .expect("project");
            let record = &commit.upserts[0];
            assert_eq!(input.element(), record.element(), "{shape}");
            assert_eq!(input.label(), record.label(), "{shape}");
            assert_eq!(input.world_bounds(), record.world_bounds());
            assert_eq!(input.mesh_id(), record.mesh_id());
            assert_eq!(
                input.mesh_to_world().to_cols_array(),
                record.mesh_to_world().to_cols_array(),
                "「网格 → 世界」同一条公式: {shape}"
            );
            assert_eq!(
                record.mesh_to_world().to_cols_array(),
                placed_once.to_cols_array(),
                "本地摆放只乘一次（标准管身的已折进存下的世界变换）: {shape}"
            );
        }
    }

    /// B/T2 + T5 端到端（真几何、不连库）：一块 1000³ 的面板，两件 cube——完全在内的与跨在 x=1000 那一面上的；
    /// `bake_element` 带计算器 → 边随 `BakedElement` 出来（跨边那件真走了点检查，用的是手上的网格），与随后
    /// 变成的投影记录经 flush 路（`edges_for_record` 回读盘上 `.mesh`）算出的边**逐字相同**；`bake_snapshot`
    /// 把全部构件与收拢后的边装进 `room`。算不出（面板网格缺）= 这件几何 `Err`、文案带「room membership」
    /// ——生成 worker 据此把整根记死信、不发布（D2 失败前移）。
    #[test]
    fn baking_computes_the_same_room_edges_the_flush_would_and_fails_the_root_when_it_cannot() {
        use crate::fast_model::room_model::{
            RoomInput, RoomPanelMap, RoomPanels, box_mesh_for_test,
        };
        use crate::fast_model::room_resident::{
            MeshCache, ResidentForm, ResidentManifest, ResidentRoomModel,
        };
        let dir = tempfile::tempdir().unwrap();
        let mesh_dir = dir.path().to_path_buf();
        box_mesh_for_test(glam::Vec3::ZERO, glam::Vec3::splat(1000.))
            .ser_to_file(&mesh_dir.join("panel.mesh"))
            .unwrap();
        let panel_refno = RefNo::new(7000, 1);
        let panel: RefnoEnum = crate::data_interface::generation_root::refno_from_e3d(panel_refno);
        let calculator = |panel_mesh: &str| {
            let map = RoomPanelMap {
                rooms: vec![RoomPanels {
                    room: "7000_2".into(),
                    room_num: "R1".into(),
                    panels: vec![panel],
                }],
                all_panels: std::collections::HashSet::from([panel]),
            };
            let panel_record = ProjectedGeometry {
                record_id: "7000_1".into(),
                geometry_id: GeometryId::Element {
                    refno: "7000/1".into(),
                },
                source_refno: panel_refno,
                dbnum: 7000,
                source_sesno: 1,
                noun: "PANE".into(),
                mesh_id: panel_mesh.into(),
                world_transform: DMat4::IDENTITY,
                world_bounds: [[0.; 3], [1000.; 3]],
                ancestors: vec![],
                shape: ProjectedShape::Baked,
                tube: None,
            };
            let model = Arc::new(ResidentRoomModel::from_projection(
                ResidentManifest {
                    form: ResidentForm::Latest,
                    key: format!("fixture-{panel_mesh}"),
                },
                vec![],
                map,
                &[panel_record],
                Arc::<MeshCache>::default(),
            ));
            crate::fast_model::room_model::MemoryRoomCalculator::new(model, mesh_dir.clone())
                .expect("calculator")
        };
        let element_at = |seq: u32, center: glam::DVec3| {
            let world = DMat4::from_translation(center) * DMat4::from_rotation_z(0.31);
            let local = manifold_csg::Manifold::cube(200.0, 200.0, 200.0, true);
            let parts = vec![local.transform(&dmat4_to_affine4x3(world))];
            GeneratedElement {
                geometry_id: GeometryId::Element {
                    refno: format!("8000/{seq}"),
                },
                refno: RefNo::new(8000, seq),
                noun: "EQUI".into(),
                is_invalid_tubi: false,
                name: None,
                solid: manifold_csg::Manifold::compose(&parts),
                parts: Some(parts),
                part_catalogue_nouns: Vec::new(),
                degenerate_parts: Vec::new(),
                part_record_repeats: Vec::new(),
                world,
                primitive_instance: None,
                analytic_primitive: None,
                negatives_applied: 0,
                own_ngms_cuts: 0,
                negatives_skipped: vec![],
                joint_cuts_explained: vec![],
                notes: vec![],
            }
        };
        let rooms = calculator("panel");

        let inside = bake_element(
            element_at(10, glam::DVec3::new(500., 500., 500.)),
            &mesh_dir,
            Some(&rooms),
        )
        .expect("bake inside");
        assert_eq!(inside.room_edges.len(), 1, "完全在内：一条边");
        assert_eq!(inside.room_edges[0].member.inside_count, 8);
        let straddling = bake_element(
            element_at(20, glam::DVec3::new(1000., 500., 500.)),
            &mesh_dir,
            Some(&rooms),
        )
        .expect("bake straddling");
        assert_eq!(straddling.room_edges.len(), 1, "跨在面板边上：点检查判进");
        assert!(straddling.room_edges[0].member.inside_count < 8);
        let outside = bake_element(
            element_at(30, glam::DVec3::new(3000., 500., 500.)),
            &mesh_dir,
            Some(&rooms),
        )
        .expect("bake outside");
        assert!(outside.room_edges.is_empty(), "盒都不交：没有边");

        // flush 路：同一件几何折成投影记录、回读盘上 .mesh 取点 → 边逐字相同。
        let root = RefNo::new(8000, 1);
        let owners = BTreeMap::from([
            ((root.word0, root.word1), root),
            ((8000, 10), root),
            ((8000, 20), root),
            ((8000, 30), root),
        ]);
        for baked in [&inside, &straddling, &outside] {
            let expected = baked.room_edges.clone();
            let clone = BakedElement {
                geometry_id: baked.geometry_id.clone(),
                refno: baked.refno,
                noun: baked.noun.clone(),
                is_invalid_tubi: baked.is_invalid_tubi,
                world: baked.world,
                primitive_instance: baked.primitive_instance.clone(),
                mesh_id: baked.mesh_id.clone(),
                mesh_write: MeshWrite::Reused,
                mesh_triangles: baked.mesh_triangles,
                world_bounds: baked.world_bounds,
                local_bounds: baked.local_bounds,
                room_edges: Vec::new(),
            };
            let (commit, _) =
                project_geometry_delta(8000, 1, vec![clone], Vec::new(), None, &owners)
                    .expect("project");
            let record = &commit.upserts[0];
            let from_flush = rooms
                .calculate_for_root(std::slice::from_ref(record))
                .unwrap();
            assert_eq!(
                crate::fast_model::room_model::merge_root_edges(expected),
                from_flush,
                "bake 时的边 == flush 回读算的边: {}",
                record.label()
            );
        }

        // 随根的那一份：全部构件在册、边收拢。
        let snapshot = bake_snapshot(
            GeneratedSnapshot {
                elements: vec![
                    element_at(10, glam::DVec3::new(500., 500., 500.)),
                    element_at(30, glam::DVec3::new(3000., 500., 500.)),
                ],
                owners: Arc::new(owners.clone()),
                report: Report::default(),
                visited: Vec::new(),
                point_sets: Vec::new(),
            },
            &mesh_dir,
            Some(&rooms),
        )
        .expect("bake snapshot");
        let room = snapshot.room.expect("带计算器就有随根的边");
        assert_eq!(room.generation, rooms.resident_generation());
        assert_eq!(
            room.elements,
            vec!["8000_10".to_string(), "8000_30".to_string()]
        );
        assert_eq!(room.edges.len(), 1);
        assert!(
            bake_snapshot(
                GeneratedSnapshot {
                    elements: vec![element_at(10, glam::DVec3::new(500., 500., 500.))],
                    owners: Arc::new(owners),
                    report: Report::default(),
                    visited: Vec::new(),
                    point_sets: Vec::new(),
                },
                &mesh_dir,
                None,
            )
            .unwrap()
            .room
            .is_none(),
            "没给计算器 → 不带边，flush 现算"
        );

        // 算不出 = Err（D2）：面板网格缺、构件盒又与它相交 → 这件几何 Err，文案带「room membership」。
        let broken = calculator("missing-panel-mesh");
        let error = bake_element(
            element_at(40, glam::DVec3::new(500., 500., 500.)),
            &mesh_dir,
            Some(&broken),
        )
        .err()
        .expect("面板网格缺就算不出");
        let text = format!("{error:#}");
        assert!(
            text.contains("room membership of 8000_40 could not be computed")
                && text.contains("room panel mesh incomplete"),
            "{text}"
        );
    }

    /// 源码钉（B/T5）：bake 的 `Err`（含房间算不出）在 `produce_slices` 的消费侧是这一根的**死信**——进
    /// `slice.failures`、不 `slice.push`、不发布（`BestEffortFallback`）；`Required` 才整批上抛。
    #[test]
    fn a_root_whose_rooms_cannot_be_computed_is_a_dead_letter_and_never_published() {
        let source = include_str!("e3d_model_service.rs")
            .split_once("\n#[cfg(test)]\nmod tests {")
            .or_else(|| {
                include_str!("e3d_model_service.rs").split_once("\r\n#[cfg(test)]\r\nmod tests {")
            })
            .expect("production code precedes the inline test module")
            .0;
        let produce = source
            .split_once("pub(crate) async fn produce_slices(")
            .expect("produce_slices")
            .1;
        let produce = &produce[..produce
            .find("pub async fn persist_memory_batch(")
            .expect("consumer follows")];
        let dead_letter = produce
            .find("Err(error) if !matches!(failure_policy, GeometryFailurePolicy::Required) => {")
            .expect("非 Required 的 Err 臂");
        let arm = &produce[dead_letter..];
        let arm = &arm[..arm.find("Err(error) => {").expect("Required 臂紧随")];
        assert!(
            arm.contains("slice.failures.push((root, format!(\"{error:#}\")));")
                && !arm.contains("slice.push("),
            "bake Err = 这一根的死信，不进切片、不发布: {arm}"
        );
        let worker = source
            .split_once("fn spawn_generation_worker(")
            .expect("worker")
            .1;
        assert!(
            worker.contains("queue.store(slot, outcome);"),
            "worker 把 bake 的成败原样交给消费侧"
        );
    }

    /// 源码钉（B/T3）：随回执走的边只有**整根重生成**这一条路带——`bake_snapshot` 用那台计算器的代号把这一根全部
    /// 构件与收拢后的边装成 `BakedSnapshot::room`，`persist_memory_batch` 把它挂到 `project_geometry_delta` 折出来的
    /// 回执上；刚体前移（`published_via = "transform"`）与单元级局部发布（`"unit"`）的回执 `room: None`——它们没有整根
    /// 的边，flush 时现算（Q5 兜底）。
    #[test]
    fn carried_room_edges_ride_only_on_the_regen_receipt() {
        let source = include_str!("e3d_model_service.rs")
            .split_once("\n#[cfg(test)]\nmod tests {")
            .or_else(|| {
                include_str!("e3d_model_service.rs").split_once("\r\n#[cfg(test)]\r\nmod tests {")
            })
            .expect("production code precedes the inline test module")
            .0;
        fn function_body<'a>(source: &'a str, start: &str) -> &'a str {
            let body = source.split_once(start).expect("函数头").1;
            let end = body
                .find("\n}\r\n")
                .or_else(|| body.find("\n}\n"))
                .expect("函数总有结尾");
            &body[..end]
        }
        let bake = function_body(source, "fn bake_snapshot(");
        assert!(
            bake.contains("carry_root_edges(")
                && bake.contains("rooms.resident_generation(),")
                && bake.contains("geometry_source_refno(&element.geometry_id)"),
            "bake_snapshot 用那台计算器的代号、这一根全部构件装随根的边: {bake}"
        );
        let consumer = source
            .split_once("pub async fn persist_memory_batch(")
            .expect("整根重生成的发布段")
            .1;
        let attach = consumer
            .find("receipt.room = carried_room;")
            .expect("挂到回执上");
        let project = consumer
            .find("project_geometry_delta(")
            .expect("折投影记录");
        assert!(project < attach, "先折回执再挂边");
        assert!(
            consumer[..attach].contains("let carried_room = snapshot.room;"),
            "边从 BakedSnapshot 上取"
        );
        // 刚体前移与单元级发布的回执不带边。
        for (path, marker) in [
            ("刚体前移", "published_via: \"transform\".into(),"),
            ("单元级发布", "published_via: \"unit\".into(),"),
        ] {
            let at = source
                .find(marker)
                .unwrap_or_else(|| panic!("{path} 的回执"));
            let after = &source[at + marker.len()..];
            let next = after.trim_start();
            assert!(
                next.starts_with("room: None,"),
                "{path} 的回执 room 必须是 None（没有整根的边，flush 现算）: {}",
                &after[..after.len().min(80)]
            );
        }
        assert_eq!(
            source.matches("receipt.room = carried_room;").count(),
            1,
            "只有整根重生成这一处挂边"
        );
    }

    /// 源码钉（B/T2）：bake 时的房间判定只走 `edges_for_record` + `world_points_of(&mesh, …)`——取点的是手上
    /// **就要写盘的那份网格**，不是为算盒子折出来的 `world_mesh`（f64 → 与盘上回读差几个 ulp，会让贴面板边
    /// 的点翻面）；输入配方只有 `baked_room_input` 一处，且与 `project_geometry_delta` 折投影记录用的
    /// 同三个函数（`geometry_source_refno` / `geometry_record_id` / `persisted_world_transform`）；
    /// 冻好的计算器沿 `produce_slices → spawn_generation_worker → bake_snapshot → bake_elements → bake_element`
    /// 一路显式传递，工作线程里不许自己冻。
    #[test]
    fn bake_computes_room_edges_from_the_mesh_it_is_about_to_write() {
        // 生产段到内联的 `#[cfg(test)] mod tests {` 为止（1043 行那处 `#[path = …]` 只是外置测试声明，不截）。
        let source = include_str!("e3d_model_service.rs")
            .split_once("\n#[cfg(test)]\nmod tests {")
            .or_else(|| {
                include_str!("e3d_model_service.rs").split_once("\r\n#[cfg(test)]\r\nmod tests {")
            })
            .expect("production code precedes the inline test module")
            .0;
        fn function_body<'a>(source: &'a str, start: &str) -> &'a str {
            let body = source.split_once(start).expect("函数头").1;
            let end = body
                .find("\n}\r\n")
                .or_else(|| body.find("\n}\n"))
                .expect("函数总有结尾");
            &body[..end]
        }
        let bake = function_body(source, "fn bake_element(");
        let edges = bake
            .find(".edges_for_record(&input, || {")
            .expect("逐元素判定走 edges_for_record");
        assert!(
            bake[edges..].contains("world_points_of(&mesh, input.mesh_to_world)"),
            "取点用手上就要写盘的那份 `mesh`: {bake}"
        );
        assert!(
            !bake.contains("world_mesh.vertices") && !bake.contains("world_points_of(&world_mesh"),
            "不许拿 f64 折出来的 world_mesh 取点: {bake}"
        );
        assert!(
            bake.contains("baked_room_input(") && bake.contains("room_edges,"),
            "输入配方只有 baked_room_input 一处、结果挂到 BakedElement.room_edges: {bake}"
        );
        assert!(
            bake.contains("room membership of {} could not be computed"),
            "失败文案与 flush 时 room_stage 同族、带「room membership」（D2）: {bake}"
        );
        let recipe = function_body(source, "fn baked_room_input(");
        let projection = function_body(source, "fn project_geometry_delta(");
        for shared in [
            "geometry_source_refno(",
            "geometry_record_id(",
            "persisted_world_transform(",
        ] {
            assert!(
                recipe.contains(shared) && projection.contains(shared),
                "bake 输入与投影记录共用 {shared}"
            );
        }
        assert!(
            recipe.contains("projected_mesh_to_world(")
                && projection.contains("world_bounds: element.world_bounds,"),
            "「网格 → 世界」经 room_model::projected_mesh_to_world；世界盒同一份"
        );
        for (function, carries) in [
            ("async fn produce_slices(", "rooms.clone(),"),
            (
                "fn spawn_generation_worker(",
                "bake_snapshot(snapshot, &mesh_dir, rooms.as_deref())",
            ),
            (
                "fn bake_snapshot(",
                "bake_elements(snapshot.elements, mesh_dir, rooms)",
            ),
            (
                "fn bake_elements(",
                "bake_element(element, mesh_dir, rooms)",
            ),
        ] {
            let body = function_body(source, function);
            assert!(
                body.contains(carries),
                "{function} 把冻好的计算器往下传: {body}"
            );
            assert!(
                !body.contains("frozen_room_calculator()"),
                "{function} 不许自己冻计算器（一任务一台）"
            );
        }
    }

    /// 本文件的正文，行尾归一。护栏测试按文本形状判定，`core.autocrlf` 决定的检出行尾
    /// 不该让它们时红时绿。
    fn service_source() -> String {
        include_str!("e3d_model_service.rs").replace("\r\n", "\n")
    }

    fn production_half(source: &str) -> &str {
        source.split_once("mod tests {").expect("test module").0
    }

    /// 本模块只生成、只写进程内投影。任何一处库句柄、SQL 片段或事务都是回退：
    /// 落库是另一层适配器的事，从这里长回来的第一行 SQL 就是那条边界破了。
    #[test]
    fn the_generation_service_never_reaches_for_a_database() {
        let source = service_source();
        let production = production_half(&source);
        for forbidden in [
            "SUL_DB",
            "surrealdb",
            "type::thing",
            "UPSERT",
            "INSERT RELATION",
            "BEGIN TRANSACTION",
            "publication_transaction",
            "gen_root",
            "inst_relate",
            "tubi_relate",
        ] {
            assert!(
                !production.contains(forbidden),
                "生成入口不许认识数据库，却出现了 {forbidden}"
            );
        }
    }

    #[test]
    fn baked_persistence_keeps_independent_parts_in_local_coordinates() {
        let world = DMat4::from_translation(glam::DVec3::new(100.0, 200.0, 300.0));
        let local_left = manifold_csg::Manifold::cube(20.0, 20.0, 20.0, true);
        let local_right = manifold_csg::Manifold::cube(20.0, 20.0, 20.0, true).transform(
            &dmat4_to_affine4x3(DMat4::from_translation(glam::DVec3::new(10.0, 0.0, 0.0))),
        );
        let parts = vec![
            local_left.transform(&dmat4_to_affine4x3(world)),
            local_right.transform(&dmat4_to_affine4x3(world)),
        ];
        let element = GeneratedElement {
            geometry_id: GeometryId::Element {
                refno: "1/2".into(),
            },
            refno: RefNo::new(1, 2),
            noun: "HELE".into(),
            is_invalid_tubi: false,
            name: None,
            solid: manifold_csg::Manifold::compose(&parts),
            parts: Some(parts),
            part_catalogue_nouns: Vec::new(),
            degenerate_parts: Vec::new(),
            part_record_repeats: Vec::new(),
            world,
            primitive_instance: None,
            analytic_primitive: None,
            negatives_applied: 0,
            own_ngms_cuts: 0,
            negatives_skipped: vec![],
            joint_cuts_explained: vec![],
            notes: vec![],
        };

        let mesh = local_generated_mesh(&element);

        assert_eq!(
            mesh.indices.len() / 3,
            24,
            "must not boolean-union the parts"
        );
        let bounds = mesh.aabb.expect("local baked mesh must have bounds");
        assert_eq!([bounds.mins.x, bounds.mins.y, bounds.mins.z], [-10.0; 3]);
        assert_eq!(
            [bounds.maxs.x, bounds.maxs.y, bounds.maxs.z],
            [20.0, 10.0, 10.0]
        );
    }

    #[test]
    fn pure_translation_keeps_baked_catalogue_mesh_id() {
        fn element_at(translation: glam::DVec3) -> GeneratedElement {
            let world =
                DMat4::from_translation(translation) * DMat4::from_rotation_z(30_f64.to_radians());
            let local = manifold_csg::Manifold::cube(20.0, 20.0, 20.0, true);
            let parts = vec![local.transform(&dmat4_to_affine4x3(world))];
            GeneratedElement {
                geometry_id: GeometryId::Element {
                    refno: "1/2".into(),
                },
                refno: RefNo::new(1, 2),
                noun: "VALV".into(),
                is_invalid_tubi: false,
                name: None,
                solid: manifold_csg::Manifold::compose(&parts),
                parts: Some(parts),
                part_catalogue_nouns: Vec::new(),
                degenerate_parts: Vec::new(),
                part_record_repeats: Vec::new(),
                world,
                primitive_instance: None,
                analytic_primitive: None,
                negatives_applied: 0,
                own_ngms_cuts: 0,
                negatives_skipped: vec![],
                joint_cuts_explained: vec![],
                notes: vec![],
            }
        }

        let before =
            local_generated_mesh(&element_at(glam::DVec3::new(-2685.11, -8478.79, 334.34)));
        let after = local_generated_mesh(&element_at(glam::DVec3::new(-2685.11, -8478.79, 659.34)));

        assert_eq!(baked_mesh_id(&before), baked_mesh_id(&after));
    }

    #[test]
    fn every_incomplete_increment_bucket_blocks_publication() {
        let incident = e3d_model::pipeline::Incident {
            refno: "1/2".into(),
            noun: "TEST".into(),
            detail: "fixture".into(),
        };
        let mut report = IncrementReport::default();
        report.regen_failed.push(incident.clone());
        report.derived_failed.push(incident.clone());
        report.unresolved.push(incident.clone());
        report.derived_stale_unreadable.push(incident);
        assert_eq!(increment_failure_count(&report), 4);
    }

    /// 一根重投影的完整形状：碰过的分支先清管身，上一轮留下、这一轮没再生成的几何被删，
    /// 新几何带着祖先链进存储，回执盖在根上。
    #[test]
    fn a_reprojected_root_resets_its_branch_tubes_and_drops_stale_geometry() {
        let store = ModelMemoryStore::default();
        let root = RefNo::new(24383, 100);
        let branch = RefNo::new(24383, 73948);
        // 祖先链走到「owner 不在索引里」为止，所以根自己也要在表里（指向自身）。
        let owners = BTreeMap::from([
            ((root.word0, root.word1), root),
            ((branch.word0, branch.word1), root),
            ((24383, 5), root),
            ((24383, 6), root),
        ]);

        let (first, _) = project_geometry_delta(
            7997,
            42,
            vec![
                baked_element(RefNo::new(24383, 5), "mesh_five"),
                baked_element(RefNo::new(24383, 6), "mesh_six"),
                baked_tube(branch, 0, "mesh_tube_a"),
            ],
            Vec::new(),
            None,
            &owners,
        )
        .expect("first projection");
        store.commit(ProjectionScope::Current, vec![first]);
        assert_eq!(
            store
                .geometry_ids_of_root(ProjectionScope::Current, 7997, root)
                .len(),
            3
        );

        // 第二轮：5 还在、6 没了、分支上换了一段新管身。
        let stale = store
            .geometry_ids_of_root(ProjectionScope::Current, 7997, root)
            .into_iter()
            .filter(|geometry_id| {
                matches!(geometry_id, GeometryId::Element { refno } if refno.contains("/6"))
            })
            .collect::<Vec<_>>();
        let (second, report) = project_geometry_delta(
            7997,
            43,
            vec![
                baked_element(RefNo::new(24383, 5), "mesh_five"),
                baked_tube(branch, 1, "mesh_tube_b"),
            ],
            stale,
            None,
            &owners,
        )
        .expect("second projection");
        assert_eq!((report.upserted, report.removed), (2, 1));
        assert_eq!(
            second.tube_branch_resets,
            vec![branch],
            "碰过的分支要整段重置，而不是逐段增删"
        );
        store.commit(ProjectionScope::Current, vec![second]);

        let records = store.records_of_root(ProjectionScope::Current, root);
        let mut meshes = records
            .iter()
            .map(|record| record.mesh_id.as_str())
            .collect::<Vec<_>>();
        meshes.sort_unstable();
        assert_eq!(meshes, vec!["mesh_five", "mesh_tube_b"]);
        assert!(
            records
                .iter()
                .all(|record| record.ancestors.contains(&packed_refno(root))),
            "每条记录都要带着到根的祖先链，按根查才捞得回来"
        );
        assert_eq!(
            store.published_sesno(ProjectionScope::Current, 7997),
            Some(43)
        );
    }

    fn unit_publish_mesh_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "unit-publish-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 一根**完整**的投影：三件（根号 +5 / +6 / +7，几根并存时记录 id 才不撞）、回执停在 `receipt_sesno`、
    /// 章是真算的、mesh 文件都在盘上。
    fn seed_complete_root(
        store: &ModelMemoryStore,
        mesh_dir: &Path,
        root: RefNo,
        receipt_sesno: u32,
    ) -> BTreeMap<(u32, u32), RefNo> {
        let member = |offset: u32| RefNo::new(root.word0, root.word1 + offset);
        let owners = BTreeMap::from([
            ((root.word0, root.word1), root),
            ((member(5).word0, member(5).word1), root),
            ((member(6).word0, member(6).word1), root),
            ((member(7).word0, member(7).word1), root),
        ]);
        let (mut commit, _) = project_geometry_delta(
            7997,
            receipt_sesno,
            vec![
                baked_element(member(5), "mesh_five"),
                baked_element(member(6), "mesh_six"),
                baked_element(member(7), "mesh_seven"),
            ],
            Vec::new(),
            None,
            &owners,
        )
        .expect("seed projection");
        for mesh in ["mesh_five", "mesh_six", "mesh_seven", "mesh_six_rebuilt"] {
            std::fs::write(mesh_dir.join(format!("{mesh}.mesh")), b"fixture").unwrap();
        }
        let (manifest, geometry_count) = manifest_hash(&commit.upserts).unwrap();
        commit.receipt = Some((
            root,
            RootReceipt {
                source_sesno: receipt_sesno,
                model_target: None,
                manifest_hash: manifest,
                geometry_count,
                published_via: "regen".into(),
                room: None,
            },
        ));
        store.commit(ProjectionScope::Current, vec![commit]);
        owners
    }

    /// ADR-080 D4：闸在产出之前。回执已在 T 的根是**重放**——跳过、按已完成计；回执比规划基线 S 新又不在 T 的根
    /// **回退**整根 regen 并说明原因；两种都不叫生产方、不发布、投影一字不动。残缺 / 解析不了的根在闸之前
    /// 就回退；生产方报错的根照 D8 回退并记原因。回执停在 S **之前**的根过闸（ADR-089 D7：未触根的常态，
    /// 登录写的 DBMDAT 会话让每根都这样；2026-10-24 之前它被当成 drifted 挡回 regen）。
    #[test]
    fn the_unit_gate_sorts_replays_and_baseline_mismatches_before_anything_is_produced() {
        let store = ModelMemoryStore::default();
        let mesh_dir = unit_publish_mesh_dir();
        let replayed = RefNo::new(24383, 100); // 回执已在 T = 43
        let drifted = RefNo::new(24383, 200); // 回执在 44：比基线 S = 42 新又不在 T（被别的路径推过了头）
        let incomplete = RefNo::new(24383, 300); // 回执数与记录数对不上
        let failing = RefNo::new(24383, 400); // 停在 S，但生产方报错
        let behind = RefNo::new(24383, 500); // 回执停在 41（S 之前）：未触根，过闸、叫生产方
        seed_complete_root(&store, &mesh_dir, replayed, 43);
        seed_complete_root(&store, &mesh_dir, drifted, 44);
        seed_complete_root(&store, &mesh_dir, incomplete, 42);
        seed_complete_root(&store, &mesh_dir, failing, 42);
        seed_complete_root(&store, &mesh_dir, behind, 41);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                receipt: Some((
                    incomplete,
                    RootReceipt {
                        source_sesno: 42,
                        model_target: None,
                        manifest_hash: "counts-do-not-match".into(),
                        geometry_count: 99,
                        published_via: "regen".into(),
                        room: None,
                    },
                )),
                ..ProjectionCommit::default()
            }],
        );
        let roots = [replayed, drifted, incomplete, failing, behind];
        let before = roots.map(|root| store.root_snapshot(ProjectionScope::Current, root));
        let mut units = roots
            .iter()
            .map(|root| UnitPublishUnit {
                root_refno: root.to_string(),
                source_start_sesno: 42,
                no_model_only: false,
            })
            .collect::<Vec<_>>();
        units.push(UnitPublishUnit {
            root_refno: "not-a-refno".into(),
            source_start_sesno: 42,
            no_model_only: false,
        });

        let mut produced = Vec::new();
        let mut published = 0;
        let report = republish_roots_by_unit_delta_on(
            &store,
            &mesh_dir,
            43,
            &units,
            |_, root, _| {
                produced.push(root);
                anyhow::bail!("execute_plan: regen_failed for {root}")
            },
            |_| {
                published += 1;
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(report.replayed, vec![replayed.to_string()]);
        assert_eq!(report.completed, Vec::<String>::new());
        let reasons = report.fallbacks.into_iter().collect::<BTreeMap<_, _>>();
        let drifted_reason = &reasons[&drifted.to_string()];
        assert!(
            drifted_reason.contains("receipt at sesno 44")
                && drifted_reason.contains("baseline 42")
                && drifted_reason.contains("target 43")
                && drifted_reason.contains("falling back to regen"),
            "{drifted_reason}"
        );
        assert_eq!(reasons[&incomplete.to_string()], "projection incomplete");
        assert!(reasons[&failing.to_string()].contains("regen_failed"));
        assert!(
            reasons[&behind.to_string()].contains("regen_failed"),
            "停在基线之前的根过了闸，轮到生产方说话"
        );
        assert!(reasons["not-a-refno"].contains("unparsable root"));
        assert_eq!(
            produced,
            vec![failing, behind],
            "只有过了闸的根才会叫生产方；重放、比基线新、残缺的一件几何都不求"
        );
        assert_eq!(published, 0, "没有一根走到发布");
        for (root, snapshot) in roots.into_iter().zip(before) {
            assert_eq!(
                store.root_snapshot(ProjectionScope::Current, root),
                snapshot,
                "{root}: 没发布的根投影一字不动"
            );
        }
    }

    /// ADR-080 D4 / D5 / A4：回执停在 S 的根走成局部提交——`scope = Partial`、`published_via = "unit"`、
    /// `source_sesno` 前移到 T、章由存储盖、没点名的那件原地不动（还停在旧会话上，正是 D3 放宽 `<=` 要接住的形状）。
    /// 同一个计划再提一次：回执已在 T，闸判 `replayed`，生产方一次都不被叫到，投影一字不动。
    #[test]
    fn a_unit_root_on_its_baseline_publishes_a_partial_commit_and_replays_idempotently() {
        let store = ModelMemoryStore::default();
        let mesh_dir = unit_publish_mesh_dir();
        let root = RefNo::new(24383, 100);
        let owners = seed_complete_root(&store, &mesh_dir, root, 42);
        let unit = UnitPublishUnit {
            root_refno: root.to_string(),
            source_start_sesno: 42,
            no_model_only: false,
        };

        let stamped = std::cell::RefCell::new(Vec::new());
        let report = republish_roots_by_unit_delta_on(
            &store,
            &mesh_dir,
            43,
            std::slice::from_ref(&unit),
            |_, produced_root, old_receipt| {
                assert_eq!(produced_root, root);
                assert_eq!(old_receipt.source_sesno, 42, "生产方拿到的是 S 上的旧回执");
                // 局部产出：106 重建、107 删掉、105 一个字不提。
                let (delta, _) = project_geometry_delta(
                    7997,
                    43,
                    vec![baked_element(RefNo::new(24383, 106), "mesh_six_rebuilt")],
                    vec![GeometryId::Element {
                        refno: RefNo::new(24383, 107).to_string(),
                    }],
                    None,
                    &owners,
                )?;
                Ok(UnitDelta {
                    tube_branch_resets: delta.tube_branch_resets,
                    removals: delta.removals,
                    upserts: delta.upserts,
                    ..UnitDelta::default()
                })
            },
            |commits| {
                assert_eq!(commits.len(), 1);
                let commit = &commits[0];
                assert_eq!(
                    commit.scope,
                    CommitScope::Partial,
                    "局部提交必须自报 Partial（D2）"
                );
                let (_, placeholder) = commit.receipt.as_ref().expect("局部提交也盖章");
                assert_eq!(placeholder.source_sesno, 43);
                assert_eq!(placeholder.published_via, "unit");
                assert!(
                    placeholder.manifest_hash.is_empty() && placeholder.geometry_count == 0,
                    "执行体不自己算章，两格留给存储在写锁里盖（D5）"
                );
                stamped
                    .borrow_mut()
                    .extend(store.try_commit_current(commits));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(report.completed, vec![root.to_string()]);
        assert!(report.replayed.is_empty() && report.fallbacks.is_empty());

        let records = store.records_of_root(ProjectionScope::Current, root);
        let mut meshes = records
            .iter()
            .map(|record| record.mesh_id.as_str())
            .collect::<Vec<_>>();
        meshes.sort_unstable();
        assert_eq!(
            meshes,
            vec!["mesh_five", "mesh_six_rebuilt"],
            "106 换了、107 没了、105 还在"
        );
        let untouched = records
            .iter()
            .find(|record| record.mesh_id == "mesh_five")
            .unwrap();
        assert_eq!(
            untouched.source_sesno, 42,
            "没点名的那件停在旧会话上——D3 放宽 `<=` 接的就是它"
        );
        let receipt = store
            .receipt(ProjectionScope::Current, root)
            .expect("receipt");
        assert_eq!(
            (receipt.source_sesno, receipt.published_via.as_str()),
            (43, "unit")
        );
        assert_eq!(
            (receipt.manifest_hash.clone(), receipt.geometry_count),
            manifest_hash(&records).unwrap(),
            "章是存储按提交之后的整根盖的"
        );
        assert_eq!(stamped.borrow().as_slice(), &[(root, receipt.clone())]);
        assert!(
            store.root_projection_complete(ProjectionScope::Current, root, &mesh_dir),
            "局部提交之后这根仍是完整的：数对得上、mesh 都在"
        );

        // A4：同一个计划连提第二次。回执已在 T，闸判重放；生产方与发布口一次都不该被叫到。
        let before = store.root_snapshot(ProjectionScope::Current, root);
        let report = republish_roots_by_unit_delta_on(
            &store,
            &mesh_dir,
            43,
            std::slice::from_ref(&unit),
            |_, _, _| panic!("重放不该再要产出"),
            |_| panic!("重放不该再发布"),
        )
        .unwrap();
        assert_eq!(report.replayed, vec![root.to_string()]);
        assert!(report.completed.is_empty() && report.fallbacks.is_empty());
        assert_eq!(
            store.root_snapshot(ProjectionScope::Current, root),
            before,
            "A4：第二次提交投影一字不动"
        );
    }

    /// ADR-080 D10：带 `no_model_only` 提示的根，生产方回一份**空** delta，执行体照样以 `Partial` 发出去
    /// ——一件几何都不动（三条记录原地、`source_sesno` 停在 S），回执推到 T、章按原记录重算逐字不变，
    /// `receipt_only` 点名它；同一份计划再提是重放。不带提示的根回出空 delta 是生产方的 bug：回退、不发。
    #[test]
    fn a_no_model_only_root_publishes_an_empty_partial_that_only_advances_the_receipt() {
        let store = ModelMemoryStore::default();
        let mesh_dir = unit_publish_mesh_dir();
        let root = RefNo::new(24383, 800);
        seed_complete_root(&store, &mesh_dir, root, 42);
        let receipt_at_s = store.receipt(ProjectionScope::Current, root).unwrap();
        let records_at_s = store.records_of_root(ProjectionScope::Current, root);
        assert_eq!(records_at_s.len(), 3);

        // 不带提示：空 delta 不许当「没变」发。
        let unhinted = UnitPublishUnit {
            root_refno: root.to_string(),
            source_start_sesno: 42,
            no_model_only: false,
        };
        let before = store.root_snapshot(ProjectionScope::Current, root);
        let report = republish_roots_by_unit_delta_on(
            &store,
            &mesh_dir,
            43,
            std::slice::from_ref(&unhinted),
            |_, _, _| Ok(UnitDelta::default()),
            |_| panic!("空 delta 没有 no_model_only 提示不该发布"),
        )
        .unwrap();
        assert!(report.completed.is_empty() && report.receipt_only.is_empty());
        assert_eq!(report.fallbacks.len(), 1);
        let reason = &report.fallbacks[0].1;
        assert!(
            reason.contains("not planned as no_model_only")
                && reason.contains("falling back to regen"),
            "{reason}"
        );
        assert_eq!(store.root_snapshot(ProjectionScope::Current, root), before);

        // 带提示：空 Partial 发出去，只推回执。
        let hinted = UnitPublishUnit {
            no_model_only: true,
            ..unhinted.clone()
        };
        let mut published = Vec::new();
        let report = republish_roots_by_unit_delta_on(
            &store,
            &mesh_dir,
            43,
            std::slice::from_ref(&hinted),
            |unit, produced_root, old_receipt| {
                assert!(unit.no_model_only);
                assert_eq!(produced_root, root);
                assert_eq!(old_receipt.source_sesno, 42);
                Ok(UnitDelta::default())
            },
            |commits| {
                assert_eq!(commits.len(), 1);
                let commit = &commits[0];
                assert_eq!(commit.scope, CommitScope::Partial);
                assert!(
                    commit.upserts.is_empty()
                        && commit.removals.is_empty()
                        && commit.tube_branch_resets.is_empty(),
                    "空清单：不插、不删、不重写管段"
                );
                let (_, placeholder) = commit.receipt.as_ref().expect("回执照盖");
                assert_eq!(
                    (placeholder.source_sesno, placeholder.published_via.as_str()),
                    (43, "unit")
                );
                published.extend(store.try_commit_current(commits));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(report.completed, vec![root.to_string()]);
        assert_eq!(
            report.receipt_only,
            vec![root.to_string()],
            "只推回执的根单列出来，供回执 / 面板观测"
        );
        assert!(report.replayed.is_empty() && report.fallbacks.is_empty());

        let records = store.records_of_root(ProjectionScope::Current, root);
        assert_eq!(
            records, records_at_s,
            "一件几何都没动，`source_sesno` 停在 S"
        );
        let receipt = store.receipt(ProjectionScope::Current, root).unwrap();
        assert_eq!(
            (receipt.source_sesno, receipt.published_via.as_str()),
            (43, "unit")
        );
        assert_eq!(
            (receipt.manifest_hash.as_str(), receipt.geometry_count),
            (
                receipt_at_s.manifest_hash.as_str(),
                receipt_at_s.geometry_count
            ),
            "章按原记录重算，与 S 上的逐字相同"
        );
        assert_eq!(published.as_slice(), &[(root, receipt.clone())]);
        assert!(store.root_projection_complete(ProjectionScope::Current, root, &mesh_dir));

        // 再提一次：重放。
        let report = republish_roots_by_unit_delta_on(
            &store,
            &mesh_dir,
            43,
            std::slice::from_ref(&hinted),
            |_, _, _| panic!("重放不该再要产出"),
            |_| panic!("重放不该再发布"),
        )
        .unwrap();
        assert_eq!(report.replayed, vec![root.to_string()]);
    }

    /// ADR-080 D10 的典型形状——**零记录的根**（HANG / SUPPO 那类没有 builder 的根，Q6 语料里 9172 个多算根有
    /// 8486 个是它）：回执 `geometry_count = 0`、投影按定义完整。空 delta 对它**不是**「删空」（D2 那条判据
    /// 只看真删了东西的清单），照样只推回执；显式删除 / 整段重写才按可能删空回退。
    #[test]
    fn an_empty_partial_on_a_zero_record_root_is_not_an_emptied_root() {
        // 判据本身：什么都不删的空清单不算「可能删空」，零记录的根也一样。
        assert!(!partial_delta_may_empty_the_root(&UnitDelta::default(), 0));
        assert!(!partial_delta_may_empty_the_root(&UnitDelta::default(), 3));
        assert!(partial_delta_may_empty_the_root(
            &UnitDelta {
                removals: vec!["x".into()],
                ..UnitDelta::default()
            },
            1
        ));
        assert!(partial_delta_may_empty_the_root(
            &UnitDelta {
                tube_branch_resets: vec![RefNo::new(24383, 1)],
                ..UnitDelta::default()
            },
            0
        ));
        assert!(!partial_delta_may_empty_the_root(
            &UnitDelta {
                removals: vec!["x".into()],
                ..UnitDelta::default()
            },
            2
        ));

        // 执行体上走一遍：零记录的根、回执停在 S。
        let store = ModelMemoryStore::default();
        let mesh_dir = unit_publish_mesh_dir();
        let root = RefNo::new(24383, 900);
        let (empty_hash, zero) = manifest_hash(&[]).unwrap();
        assert_eq!(zero, 0);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                receipt: Some((
                    root,
                    RootReceipt {
                        source_sesno: 42,
                        model_target: None,
                        manifest_hash: empty_hash.clone(),
                        geometry_count: 0,
                        published_via: "regen".into(),
                        room: None,
                    },
                )),
                ..ProjectionCommit::default()
            }],
        );
        assert!(
            store.root_projection_complete(ProjectionScope::Current, root, &mesh_dir),
            "零记录 + 回执数 0 = 完整"
        );
        let unit = UnitPublishUnit {
            root_refno: root.to_string(),
            source_start_sesno: 42,
            no_model_only: true,
        };
        let report = republish_roots_by_unit_delta_on(
            &store,
            &mesh_dir,
            43,
            std::slice::from_ref(&unit),
            |_, _, _| Ok(UnitDelta::default()),
            |commits| {
                store.try_commit_current(commits);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            report.completed,
            vec![root.to_string()],
            "{:?}",
            report.fallbacks
        );
        assert_eq!(report.receipt_only, vec![root.to_string()]);
        let receipt = store.receipt(ProjectionScope::Current, root).unwrap();
        assert_eq!(receipt.source_sesno, 43);
        assert_eq!(
            (receipt.manifest_hash, receipt.geometry_count),
            (empty_hash, 0),
            "零记录的章就是空清单的章"
        );
        let records = store.records_of_root(ProjectionScope::Current, root);
        assert!(records.is_empty(), "零记录的根照旧零记录");
    }

    /// ADR-080 D10 生产方守卫 [`root_touched_only_by_no_model_candidates`]：只看两个方向（候选就是根、
    /// 候选的 T 端 / S 端链经过根），根之上的候选不算；名下候选全是 `no_model` 且至少一个才成立。
    ///
    /// 层级（T 端）：ZONE 1 ─┬─ EQUI 10（根）─── BOX 11 / SUBE 12
    ///                     └─ EQUI 20（根）─── BOX 21
    /// S 端另有 BOX 13 在 EQUI 10 下（T 端删了）。
    #[test]
    fn the_producer_guard_attributes_ledger_candidates_to_the_root_by_owner_chain() {
        let e = |n: u32| RefNo::new(24383, n);
        let key = |n: u32| (24383u32, n);
        let owners_t = BTreeMap::from([
            (key(1), e(1)),
            (key(10), e(1)),
            (key(11), e(10)),
            (key(12), e(10)),
            (key(20), e(1)),
            (key(21), e(20)),
        ]);
        let mut owners_s = owners_t.clone();
        owners_s.insert(key(13), e(10));
        let guard = |dispositions: &[((u32, u32), Disposition)]| {
            root_touched_only_by_no_model_candidates(dispositions, e(10), &owners_s, &owners_t)
        };

        // 名下一个只戳的候选：成立。候选就是根自己：成立。
        guard(&[(key(11), Disposition::NoModel)]).unwrap();
        guard(&[(key(10), Disposition::NoModel)]).unwrap();
        // 别的根名下的候选不算进来；根之上（ZONE 1）的 RolledUp 候选也不算——两个都不拦。
        guard(&[
            (key(11), Disposition::NoModel),
            (key(21), Disposition::RolledUp),
            (key(1), Disposition::RolledUp),
        ])
        .unwrap();
        // 名下有一个 RolledUp / Unresolved：不成立，点名它。
        let err = guard(&[
            (key(11), Disposition::NoModel),
            (key(12), Disposition::RolledUp),
        ])
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("24383/12") && err.contains("RolledUp"),
            "{err}"
        );
        let err = guard(&[(key(12), Disposition::Unresolved)])
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unresolved"), "{err}");
        // T 端删掉的 BOX 13 只在 S 端链上：照样归到这根名下。
        let err = guard(&[
            (key(11), Disposition::NoModel),
            (key(13), Disposition::RolledUp),
        ])
        .unwrap_err()
        .to_string();
        assert!(err.contains("24383/13"), "{err}");
        // 一个都归不到：不成立。
        let err = guard(&[(key(21), Disposition::NoModel)])
            .unwrap_err()
            .to_string();
        assert!(err.contains("no ledger candidate is attributed"), "{err}");
        assert!(guard(&[]).is_err());
    }

    /// ADR-080 D6 / 实施约束 4 的三条判据，直接对门本身：不重置就带段、重置了没账、账与段数不符，
    /// 三种都拦；重置 + 有账 + 段数对上才放行。只数段数，`route_ordinal` 有洞不算错（退化槽位本就不出几何）。
    #[test]
    fn the_tube_segment_gate_rejects_unreset_unaccounted_and_short_branches() {
        let root = RefNo::new(24383, 600);
        let branch = RefNo::new(24383, 621);
        let branch_key = (branch.word0, branch.word1);
        let owners = BTreeMap::from([
            ((root.word0, root.word1), root),
            ((branch.word0, branch.word1), root),
        ]);
        let segments = |ordinals: &[u32]| {
            project_geometry_delta(
                7997,
                43,
                ordinals
                    .iter()
                    .map(|ordinal| baked_tube(branch, *ordinal, &format!("mesh_t{ordinal}")))
                    .collect(),
                Vec::new(),
                None,
                &owners,
            )
            .expect("tube projection")
            .0
            .upserts
        };

        // ① 带了段、没重置：旧段会留在新段旁边。
        let unreset = UnitDelta {
            upserts: segments(&[0, 1]),
            tube_segments: BTreeMap::from([(branch_key, 2)]),
            ..UnitDelta::default()
        };
        let reason = tube_segment_completeness(&unreset).unwrap_err();
        assert!(reason.contains("without resetting the branch"), "{reason}");

        // ② 重置了、没账：核不了。
        let unaccounted = UnitDelta {
            tube_branch_resets: vec![branch],
            upserts: segments(&[0, 1]),
            ..UnitDelta::default()
        };
        let reason = tube_segment_completeness(&unaccounted).unwrap_err();
        assert!(reason.contains("no segment count declared"), "{reason}");

        // ③ 重置了、有账、少一段：正是 D6 要拦的静默丢管。只重置不带段（纯删一段）也是这一档。
        let short = UnitDelta {
            tube_branch_resets: vec![branch],
            upserts: segments(&[0]),
            tube_segments: BTreeMap::from([(branch_key, 2)]),
            ..UnitDelta::default()
        };
        let reason = tube_segment_completeness(&short).unwrap_err();
        assert!(
            reason.contains("carries 1 tube segment(s), plan expects 2"),
            "{reason}"
        );
        let reset_only = UnitDelta {
            tube_branch_resets: vec![branch],
            tube_segments: BTreeMap::from([(branch_key, 2)]),
            ..UnitDelta::default()
        };
        assert!(
            tube_segment_completeness(&reset_only)
                .unwrap_err()
                .contains("carries 0 tube segment(s), plan expects 2")
        );

        // ④ 放行：重置 + 有账 + 段数对上。序号 0 / 2 有洞（1 号槽位退化）不算错。
        let complete = UnitDelta {
            tube_branch_resets: vec![branch],
            upserts: segments(&[0, 2]),
            tube_segments: BTreeMap::from([(branch_key, 2)]),
            ..UnitDelta::default()
        };
        assert_eq!(tube_segment_completeness(&complete), Ok(()));
        // 没触到任何管身的局部提交与门无关。
        assert_eq!(tube_segment_completeness(&UnitDelta::default()), Ok(()));
        // 账上说 0 段（容器在 T 端退休）且只重置不带段：合法的整段清空。
        let retired = UnitDelta {
            tube_branch_resets: vec![branch],
            tube_segments: BTreeMap::from([(branch_key, 0)]),
            ..UnitDelta::default()
        };
        assert_eq!(tube_segment_completeness(&retired), Ok(()));
    }

    /// ADR-080 A3：构造缺一段的 `Partial`，执行体必须在产出与提交之间回退 regen 并报出原因，**不得**发布——
    /// 两段旧管身原地不动。补齐那一段再来，同一条 BRAN 就走成局部提交。
    #[test]
    fn a_partial_delta_missing_one_tube_segment_falls_back_before_publishing() {
        let store = ModelMemoryStore::default();
        let mesh_dir = unit_publish_mesh_dir();
        let root = RefNo::new(24383, 700);
        let branch = RefNo::new(24383, 721);
        let branch_key = (branch.word0, branch.word1);
        let element = RefNo::new(24383, 705);
        let owners = BTreeMap::from([
            ((root.word0, root.word1), root),
            ((branch.word0, branch.word1), root),
            ((element.word0, element.word1), root),
        ]);
        for mesh in ["mesh_e", "mesh_t0", "mesh_t1", "mesh_t0_new", "mesh_t1_new"] {
            std::fs::write(mesh_dir.join(format!("{mesh}.mesh")), b"fixture").unwrap();
        }
        let (mut seed, _) = project_geometry_delta(
            7997,
            42,
            vec![
                baked_element(element, "mesh_e"),
                baked_tube(branch, 0, "mesh_t0"),
                baked_tube(branch, 1, "mesh_t1"),
            ],
            Vec::new(),
            None,
            &owners,
        )
        .expect("seed");
        let (manifest, geometry_count) = manifest_hash(&seed.upserts).unwrap();
        seed.receipt = Some((
            root,
            RootReceipt {
                source_sesno: 42,
                model_target: None,
                manifest_hash: manifest,
                geometry_count,
                published_via: "regen".into(),
                room: None,
            },
        ));
        store.commit(ProjectionScope::Current, vec![seed]);
        let unit = UnitPublishUnit {
            root_refno: root.to_string(),
            source_start_sesno: 42,
            no_model_only: false,
        };
        let rebuilt_segments = |ordinals: &[u32]| -> anyhow::Result<UnitDelta> {
            let (delta, _) = project_geometry_delta(
                7997,
                43,
                ordinals
                    .iter()
                    .map(|ordinal| baked_tube(branch, *ordinal, &format!("mesh_t{ordinal}_new")))
                    .collect(),
                Vec::new(),
                None,
                &owners,
            )?;
            Ok(UnitDelta {
                tube_branch_resets: delta.tube_branch_resets,
                removals: delta.removals,
                upserts: delta.upserts,
                // 生产方另记的账：这条 BRAN 在 T 端有两段。
                tube_segments: BTreeMap::from([(branch_key, 2)]),
                ..UnitDelta::default()
            })
        };

        // 只带了 0 号段：门拦下，回退，不发。
        let before = store.root_snapshot(ProjectionScope::Current, root);
        let mut published = 0;
        let report = republish_roots_by_unit_delta_on(
            &store,
            &mesh_dir,
            43,
            std::slice::from_ref(&unit),
            |_, _, _| rebuilt_segments(&[0]),
            |_| {
                published += 1;
                Ok(())
            },
        )
        .unwrap();
        assert!(report.completed.is_empty() && report.replayed.is_empty());
        assert_eq!(report.fallbacks.len(), 1);
        let (fallen, reason) = &report.fallbacks[0];
        assert_eq!(fallen, &root.to_string());
        assert!(
            reason.contains("tube segment completeness")
                && reason.contains(&format!("BRAN {branch}"))
                && reason.contains("carries 1 tube segment(s), plan expects 2")
                && reason.contains("falling back to regen"),
            "{reason}"
        );
        assert_eq!(published, 0, "缺段的局部清单一个字都不许发");
        assert_eq!(
            store.root_snapshot(ProjectionScope::Current, root),
            before,
            "两段旧管身原地不动，回执还停在 S"
        );

        // 补齐两段：同一条 BRAN 走成局部提交，旧段被整段换掉，元素没人提、原地不动。
        let report = republish_roots_by_unit_delta_on(
            &store,
            &mesh_dir,
            43,
            std::slice::from_ref(&unit),
            |_, _, _| rebuilt_segments(&[0, 1]),
            |commits| {
                store.try_commit_current(commits);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(report.completed, vec![root.to_string()]);
        let mut meshes = store
            .records_of_root(ProjectionScope::Current, root)
            .into_iter()
            .map(|record| record.mesh_id)
            .collect::<Vec<_>>();
        meshes.sort_unstable();
        assert_eq!(meshes, vec!["mesh_e", "mesh_t0_new", "mesh_t1_new"]);
        let receipt = store.receipt(ProjectionScope::Current, root).unwrap();
        assert_eq!((receipt.source_sesno, receipt.geometry_count), (43, 3));
    }

    /// ADR-080 D6 第一条：产出里每一条 removal 都要回溯到子计划（`remove` / 画不出的 `regenerate` /
    /// 派生旧键）。多出来的一条就是 `execute_plan` 在别处推出了删除——局部提交下等于静默丢件。
    #[test]
    fn a_removal_that_does_not_trace_back_to_the_plan_is_refused() {
        let branch = RefNo::new(24383, 21);
        let stale = GeometryId::ImpliedTube {
            container_refno: branch.to_string(),
            route_ordinal: 0,
            from_refno: "24383/10".into(),
            to_refno: "24383/20".into(),
        };
        let sets = RootPlanSets {
            regenerate: BTreeSet::from([(24383, 5)]),
            remove: BTreeSet::from([(24383, 6)]),
            derived_stale: BTreeSet::from([stale.clone()]),
            retired_containers: BTreeSet::new(),
        };
        let element = |n: u32| GeometryId::Element {
            refno: RefNo::new(24383, n).to_string(),
        };
        // 三处合法来源都放行：删除的单元、画不出而被擦掉的重建单元、派生旧键。
        removals_trace_back_to_the_plan(&sets, &[element(6), element(5), stale.clone()]).unwrap();
        removals_trace_back_to_the_plan(&sets, &[]).unwrap();
        // 计划里没提过的单元 / 别的容器的旧键：拒收，原因点名那一条。
        let err = removals_trace_back_to_the_plan(&sets, &[element(6), element(7)]).unwrap_err();
        assert!(err.to_string().contains("24383/7"), "{err:#}");
        let other = GeometryId::ImpliedTube {
            container_refno: RefNo::new(24383, 22).to_string(),
            route_ordinal: 0,
            from_refno: "24383/10".into(),
            to_refno: "24383/20".into(),
        };
        let err = removals_trace_back_to_the_plan(&sets, &[other]).unwrap_err();
        assert!(err.to_string().contains("24383/22"), "{err:#}");
    }

    /// ADR-080 D8 ① ② / Q4：退化成整库重建的窗计划切出来的子计划是空产出，不许当「没变」发；计划期有读挂的
    /// 一窗也不许。两种都在叫 `execute_plan` 之前拦下。
    #[test]
    fn a_degraded_or_unreadable_window_plan_admits_no_partial_publish() {
        let clean = WindowPlanSets {
            key: WindowPlanKey::new(8000, Path::new("ams8000_0001"), 10, 11),
            regenerate: BTreeSet::from([(24383, 5)]),
            remove: BTreeSet::new(),
            regenerate_derived: BTreeSet::new(),
            derived_stale: BTreeSet::new(),
            full_rebuild: None,
            unreadable: 0,
            dispositions: Vec::new(),
            inputs: None,
        };
        clean.admits_partial_publish().unwrap();

        let degraded = WindowPlanSets {
            key: WindowPlanKey::new(8000, Path::new("ams8000_0001"), 10, 11),
            regenerate: BTreeSet::new(),
            remove: BTreeSet::new(),
            regenerate_derived: BTreeSet::new(),
            derived_stale: BTreeSet::new(),
            full_rebuild: Some("candidates 30000 > max_candidates 20000".into()),
            unreadable: 0,
            dispositions: Vec::new(),
            inputs: None,
        };
        let err = degraded.admits_partial_publish().unwrap_err().to_string();
        assert!(
            err.contains("full rebuild") && err.contains("max_candidates"),
            "{err}"
        );

        let unreadable = WindowPlanSets {
            unreadable: 2,
            ..clean
        };
        let err = unreadable.admits_partial_publish().unwrap_err().to_string();
        assert!(err.contains("2 unreadable"), "{err}");
    }

    /// ADR-080 生产方守卫 3：只有 `design.session` / `design.digest` 前移是局部发布该看到的那一格；
    /// 目录会话、模板、生成器指纹、细分档任何一格变了都不是一窗 DESI 差分能覆盖的。
    #[test]
    fn a_target_change_outside_the_design_window_disqualifies_a_partial_delta() {
        let file = |dbnum: u32, db_type: &str, session: u32, digest: &str| ModelFileTarget {
            dbnum,
            db_type: db_type.into(),
            file: format!("{db_type}-{dbnum}"),
            session,
            digest: digest.into(),
        };
        let published = ModelTarget {
            project: "AMS".into(),
            design: file(8000, "DESI", 42, "design@42"),
            catalogue: vec![file(7000, "CATA", 9, "cata@9")],
            template_attlib_digest: "attlib".into(),
            generator_fingerprint: "gen-v1".into(),
            tessellation_profile: "e3d-default-v1".into(),
        };
        let mut advanced = published.clone();
        advanced.design = file(8000, "DESI", 43, "design@43");
        assert!(
            same_target_outside_the_design_window(&published, &advanced),
            "设计文件的会话与摘要前移正是设计窗该带来的变化"
        );
        let mut catalogue_moved = advanced.clone();
        catalogue_moved.catalogue = vec![file(7000, "CATA", 10, "cata@10")];
        assert!(!same_target_outside_the_design_window(
            &published,
            &catalogue_moved
        ));
        // 本窗设计改动引进的新目录依赖（生成途中才懒开）同样不是局部清单记得下的。
        let mut catalogue_added = advanced.clone();
        catalogue_added
            .catalogue
            .push(file(7010, "CATA", 3, "cata7010@3"));
        assert!(!same_target_outside_the_design_window(
            &published,
            &catalogue_added
        ));
        let mut generator_changed = advanced.clone();
        generator_changed.generator_fingerprint = "gen-v2".into();
        assert!(!same_target_outside_the_design_window(
            &published,
            &generator_changed
        ));
        let mut template_changed = advanced.clone();
        template_changed.template_attlib_digest = "attlib-2".into();
        assert!(!same_target_outside_the_design_window(
            &published,
            &template_changed
        ));
        let mut tessellation_changed = advanced;
        tessellation_changed.tessellation_profile = "fine".into();
        assert!(!same_target_outside_the_design_window(
            &published,
            &tessellation_changed
        ));
    }

    /// 同一个文件两种写法不是「变了」：冻结源服务钉的是 `canonicalize` 过的 `\\?\D:\…`，普通 MDB 服务记项目清单里的
    /// `D:\…`——8023 2026-10-24 `db-20261024-200916-000003` 就是这一格把刚体前移拒成 regen（GitHub #64）。摘要、会话、
    /// 库号任一不同仍是变了；回退理由要点名是哪一格。
    #[test]
    fn the_windows_verbatim_prefix_is_not_a_target_difference() {
        let file =
            |dbnum: u32, db_type: &str, path: &str, session: u32, digest: &str| ModelFileTarget {
                dbnum,
                db_type: db_type.into(),
                file: path.into(),
                session,
                digest: digest.into(),
            };
        let published = ModelTarget {
            project: "AMS".into(),
            design: file(
                8000,
                "DESI",
                r"\\?\D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001",
                692,
                "design@692",
            ),
            catalogue: vec![file(
                7000,
                "CATA",
                r"\\?\D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7000_0001",
                9,
                "cata@9",
            )],
            template_attlib_digest: "attlib".into(),
            generator_fingerprint: "gen-v1".into(),
            tessellation_profile: "e3d-default-v1".into(),
        };
        let mut current = published.clone();
        current.design = file(
            8000,
            "DESI",
            r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001",
            694,
            "design@694",
        );
        current.catalogue[0].file =
            r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7000_0001".into();
        assert!(
            same_target_outside_the_design_window(&published, &current),
            "{}",
            describe_target_difference(&published, &current)
        );
        assert_eq!(
            describe_target_difference(&published, &current),
            "no difference"
        );
        let receipt = receipt_at(692, Some(published.clone()));
        rigid_motion_target_at(&receipt, current.clone(), 694)
            .expect("the verbatim prefix alone must not refuse rigid motion");

        // 真变了仍要拦，而且说得出是哪一格。
        let mut catalogue_moved = current.clone();
        catalogue_moved.catalogue[0].session = 10;
        catalogue_moved.catalogue[0].digest = "cata@10".into();
        let error = rigid_motion_target_at(&receipt, catalogue_moved, 694).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("immutable target changed"), "{text}");
        assert!(
            text.contains("catalogue CATA 7000: session 9 -> 10, digest cata@9 -> cata@10"),
            "{text}"
        );
        let mut other_file = current;
        other_file.design.file = r"D:\elsewhere\ams8000_0001".into();
        assert!(
            describe_target_difference(&published, &other_file)
                .starts_with("design file DESI 8000"),
            "{}",
            describe_target_difference(&published, &other_file)
        );
    }

    #[test]
    fn plain_file_path_strips_only_the_verbatim_prefix() {
        assert_eq!(
            plain_file_path(Path::new(r"\\?\D:\proj\ams000\ams8000_0001")),
            r"D:\proj\ams000\ams8000_0001"
        );
        assert_eq!(
            plain_file_path(Path::new(r"\\?\UNC\server\share\ams000\ams8000_0001")),
            r"\\server\share\ams000\ams8000_0001"
        );
        assert_eq!(
            plain_file_path(Path::new(r"D:\proj\ams000\ams8000_0001")),
            r"D:\proj\ams000\ams8000_0001"
        );
        assert_eq!(
            plain_file_path(Path::new("/srv/ams000/ams8000_0001")),
            "/srv/ams000/ams8000_0001"
        );
    }

    /// ADR-080 真库探针（手动）：一窗 S→T 里挑一根 regen 根，S 端整根重算灌进本地投影当基线（回执停在 S、
    /// 目标是 S 的），再走单元级发布（真生产方 [`UnitDeltaProducer`] → 执行体），最后与 T 端整根重算比
    /// **整根 `manifest_hash`**（D9 ①：逐字相同；不等时按 `geometry_id` 逐条报差异）。
    ///
    /// 旋钮：`AIOS_UNIT_PUBLISH_PROBE_DBNUM`（默认 8000）、`AIOS_UNIT_PUBLISH_PROBE_WINDOW=S:T`（默认该库
    /// 最新两个会话）、`AIOS_UNIT_PUBLISH_PROBE_ROOT=a/b`（默认：规划器 regen 名单里第一根 S 端也有的）、
    /// `AIOS_UNIT_PUBLISH_PROBE_FILE=<DESI 文件>`（MDB 没声明的语料库挂成额外的 DESI pin，目录依赖仍经当前 MDB 解；
    /// Q6 那六个库要配 `DB_OPTION_FILE=db_options/DbOption-aps.toml`）。D10 的分桶随 `AIOS_MODEL_UNIT_LEVEL_PUBLISH=1`
    /// **且三条级联同开**（`AIOS_MODEL_STRUCTURAL_CONNECTION_CASCADE=1` / `AIOS_MODEL_CABLE_ENDPOINT_CASCADE=1` /
    /// `AIOS_MODEL_DESIGN_REFERENCE_CASCADE=1`，用户 0917 拍板「级联关着 ⇒ D10 不分桶」）收候选表——缺一个候选表为空、
    /// `no_model_only` 恒假，空子计划照旧回退。
    /// 要配置好的 MDB（`from_current`）与 E3D 模板目录。烘焙写的是真 mesh 目录（内容寻址，与整根重算同一处）。
    #[tokio::test]
    #[ignore = "manual live: needs the configured MDB and the E3D template directory; run with --ignored --nocapture"]
    async fn probe_unit_publish_one_root_against_regen() -> anyhow::Result<()> {
        use std::time::Instant;

        use crate::data_interface::generation_root::{
            enumerate_generation_roots, refno_to_e3d, resolve_delivery_unit_types,
        };
        use crate::data_interface::window_root_plan::{
            PersistedRoot, WindowRootSources, plan_window_roots,
        };
        use e3d_io::engine::ReadOnlyEngine;

        let env_u32 = |name: &str| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
        };
        let dbnum = env_u32("AIOS_UNIT_PUBLISH_PROBE_DBNUM").unwrap_or(8000);
        let mut service = E3dModelService::from_current().await?;
        // `AIOS_UNIT_PUBLISH_PROBE_FILE=<DESI 文件>`：MDB 没声明的语料库（Q6 那六个库都是）挂成一枚
        // 额外的 DESI pin，目录依赖仍经当前 MDB 的定位器解——文件所属项目的 CATA / PROP 得在 MDB 里。
        if let Ok(file) = std::env::var("AIOS_UNIT_PUBLISH_PROBE_FILE") {
            use crate::data_interface::direct_store::DbPin;

            let file = PathBuf::from(file);
            anyhow::ensure!(file.is_file(), "{} is not a file", file.display());
            service.pins.retain(|pin| pin.dbnum != dbnum as i32);
            service.pins.push(DbPin {
                dbnum: dbnum as i32,
                db_type: "DESI".into(),
                file,
                sesno: None,
            });
        }
        let pin = service.pin(dbnum)?;
        let (base_sesno, target_sesno) = match std::env::var("AIOS_UNIT_PUBLISH_PROBE_WINDOW") {
            Ok(window) => {
                let (base, target) = window
                    .split_once(':')
                    .context("AIOS_UNIT_PUBLISH_PROBE_WINDOW=S:T")?;
                (base.trim().parse::<u32>()?, target.trim().parse::<u32>()?)
            }
            Err(_) => {
                // 引擎给的是新→旧序：[0] 最新。
                let sessions: Vec<u32> = ReadOnlyEngine::open(&pin.file)?
                    .sessions()?
                    .iter()
                    .map(|session| session.session_id)
                    .collect();
                anyhow::ensure!(
                    sessions.len() >= 2,
                    "{} has fewer than two sessions",
                    pin.file.display()
                );
                (sessions[1], sessions[0])
            }
        };
        println!(
            "probe dbnum={dbnum} file={} window {base_sesno}→{target_sesno}",
            pin.file.display()
        );

        let base = generation_input(&service, dbnum, base_sesno)?;
        let target = generation_input(&service, dbnum, target_sesno)?;
        let unit_types = resolve_delivery_unit_types(&[]);
        let sources = WindowRootSources::collect(
            &pin.file,
            base_sesno,
            target_sesno,
            &base.set,
            &target.set,
            &unit_types,
            &[],
        )?;
        let roots_s: Vec<PersistedRoot> =
            enumerate_generation_roots(&base.set, &base.index.roots, &unit_types)?
                .into_iter()
                .map(|root| PersistedRoot {
                    refno: root.root,
                    noun: root.noun,
                })
                .collect();
        let impact = sources.impact(&roots_s);
        let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
        println!(
            "planner: regen roots {} degraded {:?}",
            plan.regen.len(),
            plan.degraded
        );
        // D10 分桶：探针没有进程内账本，S 端根全算安定。候选表随 `model_unit_level_publish` 开关收集——
        // 开关关着（没设 `AIOS_MODEL_UNIT_LEVEL_PUBLISH=1`）时表空、一根都判不出来，探针照旧走老路。
        let settled: BTreeSet<_> = roots_s.iter().map(|root| root.refno).collect();
        let no_model_only = sources.no_model_only(&roots_s, &settled, &impact, &plan);
        println!(
            "planner: no_model_only roots {} of regen {} (D10; unit-level switch {}; candidates {})",
            no_model_only.roots.len(),
            plan.regen.len(),
            crate::options::model_unit_level_publish(),
            sources.rigid.candidates.len()
        );
        for root in &no_model_only.roots {
            println!("  no_model_only {}", root.to_pdms_str());
        }
        let root = match std::env::var("AIOS_UNIT_PUBLISH_PROBE_ROOT") {
            Ok(text) => parse_refno(&text)?,
            Err(_) => {
                let at_s = roots_s
                    .iter()
                    .map(|root| {
                        let refno = refno_to_e3d(root.refno);
                        (refno.word0, refno.word1)
                    })
                    .collect::<BTreeSet<_>>();
                plan.regen
                    .iter()
                    .map(|root| refno_to_e3d(root.root))
                    .find(|root| at_s.contains(&(root.word0, root.word1)))
                    .context(
                        "no regen root that also exists at S; pass AIOS_UNIT_PUBLISH_PROBE_ROOT",
                    )?
            }
        };
        let noun = plan
            .regen
            .iter()
            .find(|candidate| refno_to_e3d(candidate.root) == root)
            .map(|candidate| candidate.noun.clone())
            .unwrap_or_default();
        println!("root {root} `{noun}`");

        // 基线：S 端整根重算 → 本地投影，回执停在 S、目标是 S 的（生产方要拿它核设计窗之外没变）。
        let store = ModelMemoryStore::default();
        let mesh_dir = service.mesh_dir.clone();
        let mut cache = CatalogueMeshCache::default();
        let regen_at = |sesno: u32,
                        input: &GenerationInput,
                        cache: &mut CatalogueMeshCache|
         -> anyhow::Result<(ProjectionCommit, usize)> {
            let snapshot = generate_snapshot_from_set(&input.set, &input.index, root, cache)?;
            anyhow::ensure!(
                snapshot.report.failed.is_empty(),
                "regen at {sesno} has {} failed element(s)",
                snapshot.report.failed.len()
            );
            let owners = snapshot.owners.clone();
            let baked = bake_snapshot(snapshot, &mesh_dir, None)?;
            let count = baked.elements.len();
            let (commit, _) =
                project_geometry_delta(dbnum, sesno, baked.elements, Vec::new(), None, &owners)?;
            Ok((commit, count))
        };
        let clock = Instant::now();
        let (mut seed, seed_count) = regen_at(base_sesno, &base, &mut cache)?;
        let regen_s_secs = clock.elapsed().as_secs_f64();
        let (seed_hash, seed_geometry_count) = manifest_hash(&seed.upserts)?;
        seed.receipt = Some((
            root,
            RootReceipt {
                source_sesno: base_sesno,
                model_target: Some(service.model_target(
                    dbnum,
                    base_sesno,
                    &base.dependency_sessions,
                )?),
                manifest_hash: seed_hash,
                geometry_count: seed_geometry_count,
                published_via: "regen".into(),
                room: None,
            },
        ));
        store.commit(ProjectionScope::Current, vec![seed]);
        anyhow::ensure!(
            store.root_projection_complete(ProjectionScope::Current, root, &mesh_dir),
            "seed projection incomplete"
        );
        println!(
            "seed@{base_sesno}: {seed_count} element(s), {seed_geometry_count} manifest entries, {regen_s_secs:.2}s"
        );

        // 单元路：真生产方 + 执行体（无锁本体；探针自己就是唯一写者）。
        let clock = Instant::now();
        let mut producer = UnitDeltaProducer::open(&service, dbnum, base_sesno, target_sesno)?;
        let plan_secs = clock.elapsed().as_secs_f64();
        let (planned_regenerate, planned_remove, planned_derived) = producer.planned();
        println!(
            "window plan: regenerate {planned_regenerate} remove {planned_remove} derived {planned_derived} ({plan_secs:.2}s)"
        );
        let root_enum = crate::data_interface::generation_root::refno_from_e3d(root);
        let root_no_model_only = no_model_only.roots.contains(&root_enum);
        if let Some((_, reason)) = no_model_only
            .rejected
            .iter()
            .find(|(candidate, _)| refno_to_e3d(*candidate) == root)
        {
            println!("root is not no_model_only: {reason}");
        }
        let unit = UnitPublishUnit {
            root_refno: root.to_string(),
            source_start_sesno: base_sesno,
            no_model_only: root_no_model_only,
        };
        let delta_shape = std::cell::RefCell::new((0usize, 0usize, 0usize));
        let clock = Instant::now();
        let report = republish_roots_by_unit_delta_on(
            &store,
            &mesh_dir,
            target_sesno,
            std::slice::from_ref(&unit),
            |unit, root, old_receipt| producer.produce(root, old_receipt, unit.no_model_only),
            |commits| {
                let commit = &commits[0];
                *delta_shape.borrow_mut() = (
                    commit.upserts.len(),
                    commit.removals.len(),
                    commit.tube_branch_resets.len(),
                );
                store.try_commit_current(commits);
                Ok(())
            },
        )?;
        let unit_secs = clock.elapsed().as_secs_f64();
        println!(
            "unit path: completed {:?} (receipt-only {:?}) replayed {:?} fallbacks {:?} ({unit_secs:.2}s); delta upserts / removals / branch resets = {:?}",
            report.completed,
            report.receipt_only,
            report.replayed,
            report.fallbacks,
            delta_shape.borrow()
        );

        // 对拍：T 端整根重算的章。
        let clock = Instant::now();
        let (regen_t, regen_t_count) = regen_at(target_sesno, &target, &mut cache)?;
        let regen_t_secs = clock.elapsed().as_secs_f64();
        let (regen_hash, regen_geometry_count) = manifest_hash(&regen_t.upserts)?;
        println!(
            "regen@{target_sesno}: {regen_t_count} element(s), {regen_geometry_count} manifest entries, {regen_t_secs:.2}s"
        );
        if report.completed.is_empty() {
            println!("root did not go the unit path (replayed / fell back); no parity to check");
            return Ok(());
        }
        let receipt = store
            .receipt(ProjectionScope::Current, root)
            .context("receipt")?;
        let records = store.records_of_root(ProjectionScope::Current, root);
        println!(
            "unit receipt: sesno {} via {} count {} hash {}",
            receipt.source_sesno,
            receipt.published_via,
            receipt.geometry_count,
            receipt.manifest_hash
        );
        println!("regen hash:   {regen_hash}");
        if receipt.manifest_hash != regen_hash {
            let unit_entries = records
                .iter()
                .map(manifest_entry)
                .collect::<anyhow::Result<BTreeMap<String, Value>>>()?;
            let regen_entries = regen_t
                .upserts
                .iter()
                .map(manifest_entry)
                .collect::<anyhow::Result<BTreeMap<String, Value>>>()?;
            for key in unit_entries
                .keys()
                .chain(regen_entries.keys())
                .collect::<BTreeSet<_>>()
            {
                match (unit_entries.get(key), regen_entries.get(key)) {
                    (Some(unit), Some(regen)) if unit == regen => {}
                    (Some(_), Some(_)) => println!("DIFF    {key}"),
                    (Some(_), None) => println!("EXTRA   {key} (unit path has it, regen does not)"),
                    (None, Some(_)) => println!("MISSING {key} (regen has it, unit path does not)"),
                    (None, None) => unreachable!(),
                }
            }
        }
        assert_eq!(
            receipt.geometry_count, regen_geometry_count,
            "geometry_count"
        );
        assert_eq!(
            receipt.manifest_hash, regen_hash,
            "D9 ①：单元路与 regen 路的整根 manifest_hash 必须逐字相同"
        );
        println!(
            "PARITY OK: unit path == regen@{target_sesno} for root {root} `{noun}` ({} records); unit {unit_secs:.2}s (plan {plan_secs:.2}s) vs regen {regen_t_secs:.2}s",
            records.len()
        );
        Ok(())
    }

    /// `x` 这个量级上 f32 相邻两个数的间距（`world_bounds` 存 f32，盒差小于这一格就是舍入、不是几何）。
    fn f32_ulp(x: f32) -> f32 {
        let x = x.abs();
        if !x.is_finite() {
            return f32::INFINITY;
        }
        if x == 0.0 {
            return f32::MIN_POSITIVE;
        }
        f32::from_bits(x.to_bits() + 1) - x
    }

    /// ADR-066 验收 1 的**生产记录半边** = S4b 计划 §5 门 3（手动，真库）：钉一窗 S→T、开着 `model_transform_cheap_path`
    /// 跑生产同款选根 + 刚体分桶（`WindowRootSources::collect` → `impact` → `plan_window_roots` → `rigid_motion`），对每个
    /// 判成 transform 的根：S 端整根重算灌进本地投影当基线（回执停在 S、目标是 S 的），走**真执行体**
    /// （[`E3dModelService::republish_roots_by_rigid_motion_on`]：闸 → pin@T 目标核对 → `republish_records` + 盒闭包 → 回执），
    /// 再与 T 端整根重算的投影记录逐条比。**判死三条**（§5 门 3，09-20 改口）：`record_id` 集合相同；`world_transform`
    /// 逐元素 `|Δ| < 1e-9`；`world_bounds` 逐分量 `|Δ| ≤ max(1e-3 mm, 2·ulp_f32(max(|a|,|b|) + |t_c|))`——1e-3 是绝对底线，
    /// 坐标量级上去了按 f32 ulp 放行（两边各自舍入最多 2 ulp；`t_c` 是该记录命中锚点的平移在这一分量上的值）。`mesh_id`：
    /// Canonical 按 id 判死；Baked **一律按内容判**——id 不等时读两份网格，顶点数同、顶点 / 法线最大偏差 < 1e-3、索引逐位同
    /// 记 `DRIFT`（CAS id 漂、几何同，观察项），否则判死。`manifest_hash` 相等只作观察项。另核 D4 回执：`source_sesno == T`、
    /// `published_via == "transform"`、`model_target` 与 regen@T 会落的那份逐字段相等。另印每根盒差的最大值与该量级下
    /// f32 的 ulp、以及「≥ 1e-3 mm 但被 ulp 臂放过」的分量数——改口前后的差别就在这一格。
    ///
    /// 旋钮：`AIOS_TRANSFORM_WINDOW=S:T`（必填）、`AIOS_RIGID_PROBE_DBNUM`（默认 8000）、`AIOS_RIGID_PROBE_FILE`（MDB 没声明的
    /// 库挂成额外 DESI pin）、`AIOS_RIGID_PROBE_ROOTS=w0/w1,…`（只执行这几根；嵌套根共有记录时用它挑最内层）；
    /// 开关 `AIOS_MODEL_TRANSFORM_CHEAP_PATH=1` 必须开，否则分桶为空。要配置好的 MDB（`from_current`）
    /// 与 E3D 模板目录；烘焙写真 mesh 目录（内容寻址，与整根重算同一处）。
    #[tokio::test]
    #[ignore = "manual live: AIOS_TRANSFORM_WINDOW=S:T on the configured MDB with AIOS_MODEL_TRANSFORM_CHEAP_PATH=1; run with --ignored --nocapture"]
    async fn probe_rigid_motion_gate3_for_window() -> anyhow::Result<()> {
        use std::time::Instant;

        use crate::data_interface::generation_root::{
            enumerate_generation_roots, resolve_delivery_unit_types,
        };
        use crate::data_interface::rigid_motion::pure_translation;
        use crate::data_interface::window_root_plan::{
            PersistedRoot, WindowRootSources, plan_window_roots,
        };

        let env_u32 = |name: &str| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
        };
        let dbnum = env_u32("AIOS_RIGID_PROBE_DBNUM").unwrap_or(8000);
        anyhow::ensure!(
            crate::options::model_transform_cheap_path(),
            "set AIOS_MODEL_TRANSFORM_CHEAP_PATH=1: with the switch off the planner classifies nothing"
        );
        let window = std::env::var("AIOS_TRANSFORM_WINDOW")
            .context("AIOS_TRANSFORM_WINDOW=S:T is required")?;
        let (base_sesno, target_sesno) = {
            let (base, target) = window
                .split_once(':')
                .context("AIOS_TRANSFORM_WINDOW=S:T")?;
            (base.trim().parse::<u32>()?, target.trim().parse::<u32>()?)
        };
        let mut service = E3dModelService::from_current().await?;
        if let Ok(file) = std::env::var("AIOS_RIGID_PROBE_FILE") {
            use crate::data_interface::direct_store::DbPin;

            let file = PathBuf::from(file);
            anyhow::ensure!(file.is_file(), "{} is not a file", file.display());
            service.pins.retain(|pin| pin.dbnum != dbnum as i32);
            service.pins.push(DbPin {
                dbnum: dbnum as i32,
                db_type: "DESI".into(),
                file,
                sesno: None,
            });
        }
        let pin = service.pin(dbnum)?;
        println!(
            "probe dbnum={dbnum} file={} window {base_sesno}→{target_sesno}",
            pin.file.display()
        );

        // 生产同款选根 + 刚体分桶（探针没有进程内账本：S 端根全算安定）。
        let base = generation_input(&service, dbnum, base_sesno)?;
        let target = generation_input(&service, dbnum, target_sesno)?;
        let unit_types = resolve_delivery_unit_types(&[]);
        let sources = WindowRootSources::collect(
            &pin.file,
            base_sesno,
            target_sesno,
            &base.set,
            &target.set,
            &unit_types,
            &[],
        )?;
        let roots_s: Vec<PersistedRoot> =
            enumerate_generation_roots(&base.set, &base.index.roots, &unit_types)?
                .into_iter()
                .map(|root| PersistedRoot {
                    refno: root.root,
                    noun: root.noun,
                })
                .collect();
        let impact = sources.impact(&roots_s);
        let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
        let settled: BTreeSet<_> = roots_s.iter().map(|root| root.refno).collect();
        let rigid = sources.rigid_motion(&roots_s, &settled, &impact);
        println!(
            "planner: regen {} degraded {:?} | transform {} rejected {} | placement candidates {}",
            plan.regen.len(),
            plan.degraded,
            rigid.transform.len(),
            rigid.rejected.len(),
            sources.rigid.candidates.len()
        );
        for (root, reason) in rigid.rejected.iter().take(10) {
            println!("  rejected {} : {reason}", root.to_pdms_str());
        }
        anyhow::ensure!(
            !rigid.transform.is_empty(),
            "no root classified transform in {base_sesno}→{target_sesno}"
        );
        // `AIOS_RIGID_PROBE_ROOTS=w0/w1,w0/w1`：只把这几根交给执行体（嵌套根共有记录时挑最内层那根，
        // 把「两根各乘一次 Δ」与几何 / f32 的题分开量）；不给 = 规划器判成 transform 的全部。
        let only_roots: Option<BTreeSet<String>> =
            std::env::var("AIOS_RIGID_PROBE_ROOTS").ok().map(|value| {
                value
                    .split(',')
                    .map(|root| root.trim().to_string())
                    .filter(|root| !root.is_empty())
                    .collect()
            });
        let units: Vec<RigidMotionUnit> = rigid
            .transform
            .iter()
            .map(|(root, anchors)| RigidMotionUnit {
                root_refno: root.to_pdms_str(),
                source_start_sesno: base_sesno,
                anchors: anchors.clone(),
            })
            .filter(|unit| {
                only_roots
                    .as_ref()
                    .is_none_or(|roots| roots.contains(&unit.root_refno))
            })
            .collect();
        if let Some(roots) = &only_roots {
            println!(
                "  AIOS_RIGID_PROBE_ROOTS={roots:?}: executing {} of {} transform root(s)",
                units.len(),
                rigid.transform.len()
            );
            anyhow::ensure!(
                !units.is_empty(),
                "none of AIOS_RIGID_PROBE_ROOTS was classified transform"
            );
        }
        for unit in &units {
            let shapes = unit
                .anchors
                .iter()
                .map(|anchor| match pure_translation(&anchor.delta) {
                    Some(t) => format!(
                        "{} t=({:.3}, {:.3}, {:.3})",
                        anchor.refno.to_pdms_str(),
                        t.x,
                        t.y,
                        t.z
                    ),
                    None => format!("{} rotation", anchor.refno.to_pdms_str()),
                })
                .collect::<Vec<_>>();
            println!("  transform {} anchors {:?}", unit.root_refno, shapes);
        }

        // 基线：每根 S 端整根重算 → 本地投影，回执停在 S、目标是 S 的。
        let store = ModelMemoryStore::default();
        let mesh_dir = service.mesh_dir.clone();
        let mut cache = CatalogueMeshCache::default();
        // 两端整根重算里画不出的件（生成器本来就画不出的那几件）不拦对拍，只记——与全语料单元级对拍同一口径：
        // 生产路照样按已画出的件发布（`partial_roots`），刚体前移挪的也只是这些记录。两端画不出的不是同一批时，
        // 逐条比对自然对不上（多一条 / 少一条），照样判红。
        let regen_at = |root: RefNo,
                        sesno: u32,
                        input: &GenerationInput,
                        cache: &mut CatalogueMeshCache|
         -> anyhow::Result<(ProjectionCommit, usize)> {
            let snapshot = generate_snapshot_from_set(&input.set, &input.index, root, cache)?;
            let failed = snapshot.report.failed.len();
            if failed > 0 {
                println!(
                    "  regen of {root} at {sesno}: {failed} element(s) failed to evaluate (kept out of both sides)"
                );
            }
            let owners = snapshot.owners.clone();
            let baked = bake_snapshot(snapshot, &mesh_dir, None)?;
            let (commit, _) =
                project_geometry_delta(dbnum, sesno, baked.elements, Vec::new(), None, &owners)?;
            Ok((commit, failed))
        };
        // 两端都先整根重算完再落款：目录会话表是生成时按需填的（`E3dDbResolver` 开到哪个目录库才登记哪个），
        // `model_target` 的 `catalogue` 列就是「此刻表里有什么」——逐根边生成边落款，早生成的根落款里目录少几行，
        // 与 regen@T 的落款比就是假阴性。表填满之后 S / T 两端各算一次，所有根共用。
        let clock = Instant::now();
        let mut seeds = Vec::new();
        let mut regen_t_by_root = BTreeMap::<String, ProjectionCommit>::new();
        let mut failed_elements = (0usize, 0usize);
        for unit in &units {
            let root = parse_refno(&unit.root_refno)?;
            let (seed, seed_failed) = regen_at(root, base_sesno, &base, &mut cache)?;
            let (regen_t, target_failed) = regen_at(root, target_sesno, &target, &mut cache)?;
            failed_elements.0 += seed_failed;
            failed_elements.1 += target_failed;
            seeds.push((root, unit.root_refno.clone(), seed));
            regen_t_by_root.insert(unit.root_refno.clone(), regen_t);
        }
        let target_at_s = service.model_target(dbnum, base_sesno, &base.dependency_sessions)?;
        let target_at_t = service.model_target(dbnum, target_sesno, &target.dependency_sessions)?;
        // 嵌套根（STRU ⊃ FRMW ⊃ SBFR 一类）名下记录互相包含：`records_of_root` 按祖先链取，两根共有的记录
        // 会被两根各处理一遍。先把重叠数出来，对拍失败时好归因。
        let seed_ids: Vec<(String, BTreeSet<String>)> = seeds
            .iter()
            .map(|(_, root_refno, seed)| {
                (
                    root_refno.clone(),
                    seed.upserts
                        .iter()
                        .map(|record| record.record_id.clone())
                        .collect(),
                )
            })
            .collect();
        let mut shared_records = BTreeMap::<String, usize>::new();
        for (i, (root_a, ids_a)) in seed_ids.iter().enumerate() {
            for (root_b, ids_b) in seed_ids.iter().skip(i + 1) {
                let shared = ids_a.intersection(ids_b).count();
                if shared > 0 {
                    println!(
                        "  NESTED  {root_a} ∩ {root_b}: {shared} shared record(s) (both roots will republish them)"
                    );
                    *shared_records.entry(root_a.clone()).or_default() += shared;
                    *shared_records.entry(root_b.clone()).or_default() += shared;
                }
            }
        }
        for (root, _, mut seed) in seeds {
            let (hash, count) = manifest_hash(&seed.upserts)?;
            seed.receipt = Some((
                root,
                RootReceipt {
                    source_sesno: base_sesno,
                    model_target: Some(target_at_s.clone()),
                    manifest_hash: hash,
                    geometry_count: count,
                    published_via: "regen".into(),
                    room: None,
                },
            ));
            store.commit(ProjectionScope::Current, vec![seed]);
            anyhow::ensure!(
                store.root_projection_complete(ProjectionScope::Current, root, &mesh_dir),
                "seed projection of {root} incomplete"
            );
        }
        println!(
            "seed@{base_sesno}: {} root(s) regenerated into the local projection; regen@{target_sesno} reference ready ({:.2}s); nested roots sharing records: {}",
            units.len(),
            clock.elapsed().as_secs_f64(),
            shared_records.len()
        );

        // 真执行体（无锁本体；探针自己是唯一写者），发布口直接提交本地投影。
        let clock = Instant::now();
        let report = service.republish_roots_by_rigid_motion_on(
            &store,
            dbnum,
            target_sesno,
            &units,
            |commits| {
                store.try_commit_current(commits);
                Ok(())
            },
        )?;
        let transform_secs = clock.elapsed().as_secs_f64();
        println!(
            "executor: completed {:?} replayed {:?} fallbacks {:?} ({transform_secs:.2}s)",
            report.completed, report.replayed, report.fallbacks
        );

        // 门 3：与 T 端整根重算的投影记录逐条比。
        let mut failures = Vec::new();
        let mut total_ulp_arm_passes = 0usize;
        let mut total_drift = 0usize;
        let clock = Instant::now();
        for root_refno in &report.completed {
            let root = parse_refno(root_refno)?;
            let unit = units
                .iter()
                .find(|unit| &unit.root_refno == root_refno)
                .context("unit of a completed root")?;
            let pure = unit
                .anchors
                .iter()
                .all(|anchor| pure_translation(&anchor.delta).is_some());
            let moved: BTreeMap<String, ProjectedGeometry> = store
                .records_of_root(ProjectionScope::Current, root)
                .into_iter()
                .map(|record| (record.record_id.clone(), record))
                .collect();
            let regen_t = regen_t_by_root
                .get(root_refno)
                .context("regen@T reference of a completed root")?;
            let regen: BTreeMap<String, ProjectedGeometry> = regen_t
                .upserts
                .iter()
                .cloned()
                .map(|record| (record.record_id.clone(), record))
                .collect();
            let receipt = store
                .receipt(ProjectionScope::Current, root)
                .context("receipt after transform")?;
            let (regen_hash, _) = manifest_hash(&regen_t.upserts)?;
            // D4：transform 落地后的回执要与一次 regen@T 落地后一模一样——凭证在 T、目标是 pin@T 的。
            let receipt_target_equal = receipt.model_target.as_ref() == Some(&target_at_t);
            let receipt_ok = receipt.source_sesno == target_sesno
                && receipt.published_via == "transform"
                && receipt_target_equal;
            println!(
                "    receipt: source_sesno {} published_via {} model_target.design.session {:?} | == regen@{target_sesno} target: {}",
                receipt.source_sesno,
                receipt.published_via,
                receipt
                    .model_target
                    .as_ref()
                    .map(|target| target.design.session),
                if receipt_target_equal { "yes" } else { "NO" }
            );
            if !receipt_target_equal {
                if let Some(stamped) = receipt.model_target.as_ref() {
                    let differing: Vec<&str> = [
                        ("project", stamped.project == target_at_t.project),
                        ("design", stamped.design == target_at_t.design),
                        ("catalogue", stamped.catalogue == target_at_t.catalogue),
                        (
                            "template_attlib_digest",
                            stamped.template_attlib_digest == target_at_t.template_attlib_digest,
                        ),
                        (
                            "generator_fingerprint",
                            stamped.generator_fingerprint == target_at_t.generator_fingerprint,
                        ),
                        (
                            "tessellation_profile",
                            stamped.tessellation_profile == target_at_t.tessellation_profile,
                        ),
                    ]
                    .into_iter()
                    .filter_map(|(name, equal)| (!equal).then_some(name))
                    .collect();
                    println!(
                        "    TARGET  fields differing from regen@{target_sesno}: {differing:?} (catalogue rows: transform {} vs regen {})",
                        stamped.catalogue.len(),
                        target_at_t.catalogue.len()
                    );
                }
            }

            let only_moved: Vec<_> = moved
                .keys()
                .filter(|key| !regen.contains_key(*key))
                .cloned()
                .collect();
            let only_regen: Vec<_> = regen
                .keys()
                .filter(|key| !moved.contains_key(*key))
                .cloned()
                .collect();
            let mut max_matrix = 0f64;
            let mut max_bounds = 0f32;
            let mut max_bounds_ulps = 0f32;
            let mut magnitude = 0f32;
            // §5 门 3 `world_bounds` 判法（09-20 改口）：逐分量 `|Δ| ≤ max(1e-3 mm, 2·ulp_f32(max(|a|,|b|) + |t_c|))`。
            // 1e-3 的绝对底线保留，坐标量级上去了按 ulp 放行：两边各自 f32 舍入——transform 侧纯平移是 f32(f32(盒_S) + t_c)
            // （S 端一次、T 端一次，S 端量级 ≤ |a| + |t_c|），regen 侧 f32(f64 紧盒) 一次，最多 2 ulp（按两端量级较大者）；
            // 含旋转两边都是 f32(f64 紧盒)，`+ |t_c|` 只是多给一点余量。超容差的分量判死；≥ 1e-3 但被 ulp 臂放过的记观察项。
            let mut bounds_over: Vec<(String, &'static str, f32, f32, f32)> = Vec::new();
            let mut ulp_arm_passes = 0usize;
            let mut mesh_mismatch = Vec::new();
            // Baked 网格是「世界求值 → 乘 world⁻¹ 本地化」存 CAS 的：位置一变，本地化后的顶点末位就可能翻，regen@T
            // 落出内容相同、id 不同的新网格——纯平移也漂（09-20 270→271 四条）。§5 门 3（09-20 改口）：Baked **一律按内容判**，
            // 不再分纯平移 / 含旋转：id 不等时读两份网格，顶点数同、顶点 / 法线最大偏差 < 1e-3、索引逐位同 = 同一份几何、
            // 只是 CAS id 漂（`DRIFT` 观察项），否则判死。Canonical 的 `mesh_id` 是标准网格，仍按 id 判死。
            let mut mesh_drift = Vec::new();
            let mut matrix_diffs: Vec<(f64, String)> = Vec::new();
            // 盒差最大的那一格：(记录, 分量名, regen 值, transform 值)——看 1e-3 是撞在哪个量级上。
            let mut worst_bounds: Option<(String, &'static str, f32, f32)> = None;
            let mut worst_ulps: Option<(String, &'static str, f32, f32)> = None;
            const COMPONENTS: [&str; 6] = ["min.x", "min.y", "min.z", "max.x", "max.y", "max.z"];
            for (key, after) in &moved {
                let Some(expected) = regen.get(key) else {
                    continue;
                };
                let matrix = expected
                    .world_transform
                    .to_cols_array()
                    .iter()
                    .zip(after.world_transform.to_cols_array().iter())
                    .map(|(x, y)| (x - y).abs())
                    .fold(0f64, f64::max);
                max_matrix = max_matrix.max(matrix);
                matrix_diffs.push((matrix, key.clone()));
                // 这条记录命中的锚点的平移列——容差里的 `t_c` 按分量取；不在任何锚点之下的记录 t = 0。
                let translation =
                    crate::data_interface::rigid_motion::nearest_anchor(after, &unit.anchors)
                        .map(|anchor| anchor.delta.w_axis.truncate())
                        .unwrap_or(glam::DVec3::ZERO);
                for (component, (x, y)) in expected
                    .world_bounds
                    .iter()
                    .flatten()
                    .zip(after.world_bounds.iter().flatten())
                    .enumerate()
                {
                    let diff = (x - y).abs();
                    let scale = x.abs().max(y.abs());
                    let t_c = translation[component % 3].abs() as f32;
                    let tolerance = 1e-3f32.max(2.0 * f32_ulp(scale + t_c));
                    if diff > max_bounds {
                        worst_bounds = Some((key.clone(), COMPONENTS[component], *x, *y));
                    }
                    let ulps = diff / f32_ulp(scale);
                    if ulps > max_bounds_ulps {
                        worst_ulps = Some((key.clone(), COMPONENTS[component], *x, *y));
                    }
                    max_bounds = max_bounds.max(diff);
                    magnitude = magnitude.max(scale);
                    max_bounds_ulps = max_bounds_ulps.max(ulps);
                    if diff > tolerance {
                        bounds_over.push((key.clone(), COMPONENTS[component], *x, *y, tolerance));
                    } else if diff >= 1e-3 {
                        ulp_arm_passes += 1;
                    }
                }
                let canonical = matches!(expected.shape, ProjectedShape::Canonical { .. });
                if expected.mesh_id != after.mesh_id {
                    // (顶点数, 顶点数', 顶点最大偏差 mm, 法线最大偏差, 索引是否逐位相同)
                    let verdict = if canonical {
                        None
                    } else {
                        let read = |mesh_id: &str| {
                            read_mesh_file(&mesh_dir.join(format!("{mesh_id}.mesh")))
                        };
                        let max_delta = |ours: &[glam::Vec3], theirs: &[glam::Vec3]| {
                            if ours.len() != theirs.len() {
                                return f32::INFINITY;
                            }
                            ours.iter()
                                .zip(theirs.iter())
                                .map(|(a, b)| (*a - *b).abs().max_element())
                                .fold(0f32, f32::max)
                        };
                        match (read(&after.mesh_id), read(&expected.mesh_id)) {
                            (Ok(ours), Ok(theirs)) => Some((
                                ours.vertices.len(),
                                theirs.vertices.len(),
                                max_delta(&ours.vertices, &theirs.vertices),
                                max_delta(&ours.normals, &theirs.normals),
                                ours.indices == theirs.indices,
                            )),
                            _ => None,
                        }
                    };
                    match verdict {
                        Some((n, m, delta, normal_delta, same_indices))
                            if n == m && delta < 1e-3 && normal_delta < 1e-3 && same_indices =>
                        {
                            mesh_drift.push((key.clone(), n, delta, normal_delta));
                        }
                        _ => mesh_mismatch.push((key.clone(), verdict)),
                    }
                }
            }
            let bounds_ok = bounds_over.is_empty();
            total_ulp_arm_passes += ulp_arm_passes;
            total_drift += mesh_drift.len();
            let records_off = matrix_diffs
                .iter()
                .filter(|(diff, _)| *diff >= 1e-9)
                .count();
            println!(
                "root {root_refno} ({} records, {} anchor(s), {}{}): ids +{} −{} | world_transform max |Δ| {max_matrix:.3e} ({records_off} record(s) ≥ 1e-9) | world_bounds max diff {max_bounds:.6} mm = {max_bounds_ulps:.2} ulp (|coord| ≤ {magnitude:.1} mm, ulp there {:.6} mm; over tolerance {}, ≥ 1e-3 passed by ulp arm {ulp_arm_passes}) | mesh_id mismatches {} + drift {} | manifest_hash {}",
                moved.len(),
                unit.anchors.len(),
                if pure { "pure translation" } else { "rotation" },
                match shared_records.get(root_refno) {
                    Some(shared) => format!(", {shared} record(s) shared with nested root(s)"),
                    None => String::new(),
                },
                only_moved.len(),
                only_regen.len(),
                f32_ulp(magnitude),
                bounds_over.len(),
                mesh_mismatch.len(),
                mesh_drift.len(),
                if receipt.manifest_hash == regen_hash {
                    "equal"
                } else {
                    "DIFFERENT (observation only)"
                }
            );
            if max_bounds > 0.0 {
                if let Some((key, component, regen_value, transform_value)) = &worst_bounds {
                    println!(
                        "    BOUNDS  worst abs: {key} {component} regen {regen_value:.6} vs transform {transform_value:.6} (diff {:.6} mm = {:.2} ulp at that magnitude)",
                        (regen_value - transform_value).abs(),
                        (regen_value - transform_value).abs()
                            / f32_ulp(regen_value.abs().max(transform_value.abs()))
                    );
                }
                if let Some((key, component, regen_value, transform_value)) = &worst_ulps {
                    println!(
                        "    BOUNDS  worst ulp: {key} {component} regen {regen_value:.6} vs transform {transform_value:.6} (diff {:.6} mm = {:.2} ulp at that magnitude)",
                        (regen_value - transform_value).abs(),
                        (regen_value - transform_value).abs()
                            / f32_ulp(regen_value.abs().max(transform_value.abs()))
                    );
                }
            }
            for (key, component, regen_value, transform_value, tolerance) in
                bounds_over.iter().take(3)
            {
                println!(
                    "    BOUNDS  OVER: {key} {component} regen {regen_value:.6} vs transform {transform_value:.6} (diff {:.6} mm > tolerance {tolerance:.6} mm = max(1e-3, 2 ulp))",
                    (regen_value - transform_value).abs()
                );
            }
            for key in only_moved.iter().take(5) {
                println!("    EXTRA   {key} (transform has it, regen@T does not)");
            }
            for key in only_regen.iter().take(5) {
                println!("    MISSING {key} (regen@T has it, transform does not)");
            }
            for (key, verdict) in mesh_mismatch.iter().take(5) {
                println!(
                    "    MESH    {key}: transform {} vs regen {} | content: {}",
                    moved[key].mesh_id,
                    regen[key].mesh_id,
                    match verdict {
                        Some((n, m, delta, normal_delta, same_indices)) => format!(
                            "{n} vs {m} vertices, max |Δv| {delta:.6} mm, max |Δn| {normal_delta:.3e}, indices {}",
                            if *same_indices { "same" } else { "DIFFER" }
                        ),
                        None => "unreadable / canonical".to_string(),
                    }
                );
            }
            for (key, n, delta, normal_delta) in mesh_drift.iter().take(3) {
                println!(
                    "    DRIFT   {key}: Baked mesh id differs, content equal ({n} vertices, max |Δv| {delta:.3e} mm, max |Δn| {normal_delta:.3e}, indices same) — transform {} vs regen {}",
                    moved[key].mesh_id, regen[key].mesh_id
                );
            }
            // 矩阵不等的记录：印最差的三条——它在哪条祖先链上、命中了哪个锚点、两边的平移列各是什么。
            matrix_diffs.sort_by(|a, b| b.0.total_cmp(&a.0));
            for (diff, key) in matrix_diffs
                .iter()
                .filter(|(diff, _)| *diff >= 1e-9)
                .take(3)
            {
                let after = &moved[key];
                let expected = &regen[key];
                let chain: Vec<String> = after
                    .ancestors
                    .iter()
                    .map(|packed| {
                        let packed = *packed as u64;
                        format!("{}/{}", packed >> 32, packed as u32)
                    })
                    .collect();
                let anchor =
                    crate::data_interface::rigid_motion::nearest_anchor(after, &unit.anchors)
                        .map(|anchor| anchor.refno.to_pdms_str());
                println!(
                    "    OFF     {key} {} source_sesno {} |Δ| {diff:.3e}: transform t=({:.3}, {:.3}, {:.3}) vs regen t=({:.3}, {:.3}, {:.3}) | nearest anchor {:?} | ancestors {}",
                    after.noun,
                    after.source_sesno,
                    after.world_transform.w_axis.x,
                    after.world_transform.w_axis.y,
                    after.world_transform.w_axis.z,
                    expected.world_transform.w_axis.x,
                    expected.world_transform.w_axis.y,
                    expected.world_transform.w_axis.z,
                    anchor,
                    chain.join(" → ")
                );
            }
            if !only_moved.is_empty() || !only_regen.is_empty() {
                failures.push(format!(
                    "{root_refno}: record_id sets differ (+{} −{})",
                    only_moved.len(),
                    only_regen.len()
                ));
            }
            if max_matrix >= 1e-9 {
                failures.push(format!(
                    "{root_refno}: world_transform max |Δ| {max_matrix:.3e} ≥ 1e-9"
                ));
            }
            if !bounds_ok {
                failures.push(format!(
                    "{root_refno}: {} world_bounds component(s) over max(1e-3 mm, 2 ulp) — max diff {max_bounds:.6} mm = {max_bounds_ulps:.2} ulp at |coord| ≤ {magnitude:.1} mm",
                    bounds_over.len()
                ));
            }
            if !mesh_mismatch.is_empty() {
                failures.push(format!(
                    "{root_refno}: {} mesh_id(s) differ (Canonical id / Baked content)",
                    mesh_mismatch.len()
                ));
            }
            if !receipt_ok {
                failures.push(format!(
                    "{root_refno}: receipt not stamped at T (source_sesno {} published_via {} target == regen@T: {})",
                    receipt.source_sesno,
                    receipt.published_via,
                    receipt.model_target.as_ref() == Some(&target_at_t)
                ));
            }
        }
        println!(
            "parity check {:.2}s; bounds components ≥ 1e-3 mm passed by the ulp arm: {total_ulp_arm_passes}; Baked mesh_id drift with equal content: {total_drift} record(s)",
            clock.elapsed().as_secs_f64()
        );
        anyhow::ensure!(
            !report.completed.is_empty(),
            "no root went the transform path (fallbacks {:?})",
            report.fallbacks
        );
        if failures.is_empty() {
            println!(
                "GATE 3 OK: {} root(s) transform == regen@{target_sesno} (record_id / 1e-9 / bounds ≤ max(1e-3, 2 ulp) / mesh: Canonical id · Baked content / receipt@T){}{}",
                report.completed.len(),
                if total_drift > 0 {
                    format!(
                        " — {total_drift} Baked mesh_id drift(s) counted as observation (content equal)"
                    )
                } else {
                    String::new()
                },
                if failed_elements != (0, 0) {
                    format!(
                        " [failed elements S {} / T {} kept out of both sides]",
                        failed_elements.0, failed_elements.1
                    )
                } else {
                    String::new()
                }
            );
        }
        for failure in &failures {
            println!("GATE 3 FAIL: {failure}");
        }
        assert!(
            failures.is_empty(),
            "gate 3 failed on {} check(s); see GATE 3 FAIL lines",
            failures.len()
        );
        Ok(())
    }

    /// ADR-080 A6 / D9 ②（手动，全语料）：沿每个 DESI 库的会话链逐窗走**生产口径**选根，对 `plan.regen` 里
    /// **S 端也有**的根（跨名单等距抽样）各做一遍 [`probe_unit_publish_one_root_against_regen`] 那三步——
    /// S 端整根重算灌基线 → 单元路（真生产方 + 执行体）→ 与 T 端整根重算比整根 `manifest_hash`——按库汇总进证据档。
    /// **一根 DIFF 即红**（D9 ①）；回退 / 重放 / 生成失败只计数不判红（回退永远正确、只是不省）。
    ///
    /// 目录依赖仍经**当前 MDB**（`DB_OPTION_FILE`）解，所以一次只量一个工程：语料默认就是这个 MDB 声明的
    /// 设计库，别的工程换 `DB_OPTION_FILE` 再跑一遍。一根的整根重算里有件引用了不在这个 MDB 里的库
    /// （「db N 没注册进这个 DbSet」），就记「依赖库不在当前 MDB」、不进对拍——两端缺得一样，diff 为 0 也不是
    /// 证据；pin 不进 / 开不了窗的库记 `skip`，不算红。
    /// 旋钮：`AIOS_POSREF_DIRS`（`;` 分隔，给了就扫目录里全部 DESI；默认只取当前 MDB 声明的设计库）、`AIOS_UNIT_PUBLISH_STEPS_PER_DB`（每库会话链最后 K 步，
    /// 0 = 整条链）、`AIOS_UNIT_PUBLISH_ROOTS_PER_WINDOW`（默认 40，0 = 全量；一窗上千根时跨 regen 名单等距抽）、
    /// `AIOS_UNIT_PUBLISH_BUDGET_SECS`（默认 2400，到了只数不量）、`AIOS_UNIT_PUBLISH_MESH_DIR`（烘焙目录，默认临时目录、
    /// 跑完删；`AIOS_UNIT_PUBLISH_KEEP_MESHES=1` 留着）、`AIOS_UNIT_PUBLISH_REPORT`（默认
    /// `docs/evidence/2026-09-17-unit-publish-parity-corpus.md`，每跑一次追加一节；目录按政策不入库）、
    /// `AIOS_UNIT_PUBLISH_SHARD=k/n`（全语料按**窗**轮流分给 n 个进程并行跑，本进程只量第 k 份；末尾一行
    /// `SUMMARY_JSON` 几份相加就是全语料——ams8000 一条链六百多窗，按库分片会让一个进程吃光预算）。
    /// D10 分桶随 `AIOS_MODEL_UNIT_LEVEL_PUBLISH=1` 且三条级联同开（同单根探针）。
    #[tokio::test]
    #[ignore = "manual live: walks every DESI chain of the corpus; needs DB_OPTION_FILE (MDB) and the E3D template directory"]
    async fn probe_unit_publish_corpus_against_regen() -> anyhow::Result<()> {
        use std::io::Write as _;
        use std::time::Instant;

        use crate::data_interface::direct_store::DbPin;
        use crate::data_interface::generation_root::{
            GenerationRoot, enumerate_generation_roots, refno_from_e3d, refno_to_e3d,
            resolve_delivery_unit_types,
        };
        use crate::data_interface::reverse_reference::sources_in_dir;
        use crate::data_interface::window_root_plan::{
            PersistedRoot, WindowRootSources, plan_window_roots,
        };
        use e3d_io::engine::ReadOnlyEngine;

        let env_usize = |name: &str, default: usize| -> usize {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        };
        let per_db = env_usize("AIOS_UNIT_PUBLISH_STEPS_PER_DB", 0);
        let roots_per_window = env_usize("AIOS_UNIT_PUBLISH_ROOTS_PER_WINDOW", 40);
        let budget_secs = env_usize("AIOS_UNIT_PUBLISH_BUDGET_SECS", 2400) as f64;
        // `AIOS_UNIT_PUBLISH_SHARD=k/n`：全语料按**窗**（不是按库——ams8000 一条链就有六百多窗）轮流分给 n 个进程，
        // 本进程只量第 k 份；别的窗一概不数。几份的 `SUMMARY_JSON` 相加就是全语料。
        let (shard, shards) = std::env::var("AIOS_UNIT_PUBLISH_SHARD")
            .ok()
            .and_then(|value| {
                let (k, n) = value.split_once('/')?;
                Some((
                    k.trim().parse::<usize>().ok()?,
                    n.trim().parse::<usize>().ok()?,
                ))
            })
            .filter(|(k, n)| *n > 0 && k < n)
            .unwrap_or((0, 1));
        let mut window_index = 0usize;
        let keep_meshes = std::env::var("AIOS_UNIT_PUBLISH_KEEP_MESHES").is_ok_and(|v| v == "1");
        let report_path = PathBuf::from(std::env::var("AIOS_UNIT_PUBLISH_REPORT").unwrap_or_else(
            |_| {
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/docs/evidence/2026-09-17-unit-publish-parity-corpus.md"
                )
                .into()
            },
        ));
        let mesh_dir = std::env::var("AIOS_UNIT_PUBLISH_MESH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("unit-publish-parity-meshes"));
        std::fs::create_dir_all(&mesh_dir)?;
        let db_option_file = std::env::var("DB_OPTION_FILE").unwrap_or_else(|_| "(default)".into());
        let switches = format!(
            "unit_level_publish={} structural_connection={} cable_endpoint={} design_reference={} \
             skip_metadata_only={} transform_cheap_path={}",
            crate::options::model_unit_level_publish(),
            crate::options::model_structural_connection_cascade(),
            crate::options::model_cable_endpoint_cascade(),
            crate::options::model_design_reference_cascade(),
            crate::options::model_skip_metadata_only_changes(),
            crate::options::model_transform_cheap_path(),
        );

        let mut service = E3dModelService::from_current().await?;
        service.mesh_dir = mesh_dir.clone();
        let explicit_dirs: Option<Vec<PathBuf>> = std::env::var("AIOS_POSREF_DIRS")
            .ok()
            .map(|value| value.split(';').map(PathBuf::from).collect());
        let unit_types = resolve_delivery_unit_types(&[]);
        let started = Instant::now();
        println!(
            "corpus probe: DB_OPTION_FILE={db_option_file}; {switches}; mesh_dir={}",
            mesh_dir.display()
        );

        /// 一根的结局。
        enum Outcome {
            Parity {
                ok: bool,
                receipt_only: bool,
                records: usize,
                unit_secs: f64,
                regen_t_secs: f64,
                diff: usize,
                extra: usize,
                missing: usize,
                /// 两端整根重算里画不出的件数（S, T）——不拦对拍，只记。
                failed_elements: (usize, usize),
            },
            /// 整根重算里有件引用了不在当前 MDB 里的库：这一轮量不准它，记跳过。
            ForeignDependency(BTreeSet<u32>),
            Fallback(String),
            Replayed,
            SeedFailed(String),
            RegenTargetFailed(String),
        }
        /// 回退原因归桶：同一类只按类计，不按根展开。
        fn fallback_bucket(reason: &str) -> &'static str {
            const BUCKETS: [(&str, &str); 12] = [
                (
                    "overhead exceeds its benefit",
                    "按根大小设门（< 10 条记录回退整根）",
                ),
                ("may leave root", "D2：局部清单会清空整根，归整根路发布"),
                (
                    "no_model_only hint does not hold",
                    "空子计划·no_model_only 复核不成立",
                ),
                (
                    "names no unit under root",
                    "空子计划·级联 / 恢复选进来的根（非 no_model_only）",
                ),
                (
                    "immutable target",
                    "设计窗之外的目标变了（目录 / 模板 / 生成器 / 细分档）",
                ),
                ("carries no model target", "旧回执没带目标"),
                ("execute_plan under root", "execute_plan 报失败"),
                ("tube segment", "管段完整性门"),
                ("does not trace back", "removal 回溯不到子计划"),
                ("full_rebuild", "窗计划退化成整库重建"),
                ("unreadable", "计划期读挂"),
                ("baseline", "回执不在规划基线上"),
            ];
            BUCKETS
                .iter()
                .find(|(needle, _)| reason.contains(needle))
                .map(|(_, bucket)| *bucket)
                .unwrap_or("其它")
        }
        /// 退化窗的原因归桶（`WindowImpact::degraded_reason` 的四类；几类同时成立时取这里排在前面的那一类）。
        fn degraded_bucket(reason: &str) -> &'static str {
            const BUCKETS: [(&str, &str); 4] = [
                ("too many candidates", "比例护栏（净候选 > 30% 索引键）"),
                ("closure incomplete", "闭包读挂"),
                ("unresolved=", "plan_update 分类不了的候选"),
                ("design-reference cascade", "设计引用反查判不了"),
            ];
            BUCKETS
                .iter()
                .find(|(needle, _)| reason.contains(needle))
                .map(|(_, bucket)| *bucket)
                .unwrap_or("其它")
        }
        #[derive(Default)]
        struct FileStats {
            windows: usize,
            windows_skipped: usize,
            windows_degraded: usize,
            regen_roots: usize,
            roots_at_s: usize,
            tried: usize,
            parity_ok: usize,
            receipt_only: usize,
            diff: usize,
            fallback: usize,
            replayed: usize,
            seed_failed: usize,
            regen_failed: usize,
            with_failed_elements: usize,
            foreign_dependency: usize,
            over_budget: usize,
            unit_secs: f64,
            regen_t_secs: f64,
            plan_secs: f64,
        }
        let mut per_file: Vec<(String, FileStats)> = Vec::new();
        let mut skipped_files: Vec<(String, String)> = Vec::new();
        let mut foreign_dbnums: BTreeSet<u32> = BTreeSet::new();
        let mut foreign_examples: Vec<String> = Vec::new();
        let mut diffs: Vec<String> = Vec::new();
        let mut fallback_buckets: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut fallback_examples: BTreeMap<&'static str, String> = BTreeMap::new();
        let mut degraded_buckets: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut degraded_examples: BTreeMap<&'static str, String> = BTreeMap::new();
        let mut failures: Vec<String> = Vec::new();
        let mut budget_exhausted = false;

        // 语料：给了 `AIOS_POSREF_DIRS` 就扫那些目录里的全部 DESI；没给就只取当前 MDB 声明的设计库——
        // 同一个目录里不属于这个 MDB 的库，目录依赖解不全。
        let mut corpus: Vec<Vec<(u32, PathBuf)>> = Vec::new();
        match &explicit_dirs {
            Some(dirs) => {
                for dir in dirs.iter().filter(|dir| dir.is_dir()) {
                    match sources_in_dir(dir, &["DESI"]) {
                        Ok(sources) => corpus.push(
                            sources
                                .into_iter()
                                .map(|source| (source.dbnum, source.file))
                                .collect(),
                        ),
                        Err(error) => skipped_files
                            .push((dir.display().to_string(), format!("scan: {error}"))),
                    }
                }
            }
            None => {
                let mut declared: Vec<(u32, PathBuf)> = service
                    .pins
                    .iter()
                    .filter(|pin| pin.db_type.eq_ignore_ascii_case("DESI"))
                    .filter_map(|pin| {
                        u32::try_from(pin.dbnum)
                            .ok()
                            .map(|dbnum| (dbnum, pin.file.clone()))
                    })
                    .collect();
                declared.sort_by(|a, b| a.1.cmp(&b.1));
                corpus.push(declared);
            }
        }
        let corpus_label = match &explicit_dirs {
            Some(dirs) => dirs
                .iter()
                .map(|dir| dir.display().to_string())
                .collect::<Vec<_>>()
                .join("; "),
            None => format!(
                "当前 MDB 声明的 {} 个设计库",
                corpus.iter().map(Vec::len).sum::<usize>()
            ),
        };

        for sources in corpus {
            for (dbnum, file) in sources {
                let name = file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // 语料库挂成一枚 DESI pin（MDB 没声明的也一样）；目录依赖经当前 MDB 解。
                service.pins.retain(|pin| pin.dbnum != dbnum as i32);
                service.pins.push(DbPin {
                    dbnum: dbnum as i32,
                    db_type: "DESI".into(),
                    file: file.clone(),
                    sesno: None,
                });
                let sessions: Vec<u32> = match ReadOnlyEngine::open(&file) {
                    Ok(mut engine) => match engine.sessions() {
                        Ok(sessions) => sessions.iter().map(|session| session.session_id).collect(),
                        Err(error) => {
                            skipped_files.push((name, format!("sessions: {error}")));
                            continue;
                        }
                    },
                    Err(error) => {
                        skipped_files.push((name, format!("open: {error}")));
                        continue;
                    }
                };
                // 新→旧链取链头 K 对，再倒回旧→新（与 S7 同款）。
                let mut steps: Vec<(u32, u32)> = sessions
                    .windows(2)
                    .take(if per_db == 0 { usize::MAX } else { per_db })
                    .map(|pair| (pair[1], pair[0]))
                    .collect();
                steps.reverse();
                if steps.is_empty() {
                    skipped_files.push((name, "fewer than two sessions".into()));
                    continue;
                }
                let mut stats = FileStats::default();
                for (base_sesno, target_sesno) in steps {
                    let mine = window_index % shards == shard;
                    window_index += 1;
                    if !mine {
                        continue;
                    }
                    let label = format!("{name} {base_sesno}→{target_sesno}");
                    if started.elapsed().as_secs_f64() > budget_secs {
                        budget_exhausted = true;
                        stats.windows_skipped += 1;
                        continue;
                    }
                    let (base_set, target_set) = match (
                        service.build_set(dbnum, Some(base_sesno)),
                        service.build_set(dbnum, Some(target_sesno)),
                    ) {
                        (Ok(base_set), Ok(target_set)) => (base_set, target_set),
                        (Err(error), _) | (_, Err(error)) => {
                            stats.windows_skipped += 1;
                            println!("skip {label}: build_set: {error:#}");
                            continue;
                        }
                    };
                    let sources = match WindowRootSources::collect(
                        &file,
                        base_sesno,
                        target_sesno,
                        &base_set,
                        &target_set,
                        &unit_types,
                        &[],
                    ) {
                        Ok(sources) => sources,
                        Err(error) => {
                            stats.windows_skipped += 1;
                            println!("skip {label}: collect: {error:#}");
                            continue;
                        }
                    };
                    // 与生产规划器同一步：打开两端集合的 service 把依赖会话表绑进缓存，执行体才复用两端输入；
                    // 不绑就只复用计划，`plan_secs` 量到的是生产里不会发生的重开。
                    service.bind_window_input_dependencies(
                        dbnum,
                        &file,
                        base_sesno,
                        target_sesno,
                        &base_set,
                        &target_set,
                    );
                    let roots_s = sources.roots_s.clone();
                    stats.windows += 1;
                    let impact = sources.impact(&roots_s);
                    let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
                    stats.regen_roots += plan.regen.len();
                    if let Some(reason) = &plan.degraded {
                        // 退化窗把每个持久根都判成 regen，单元路对它一根都不许走（Q4）——只数不量；原因按类记账。
                        stats.windows_degraded += 1;
                        let bucket = degraded_bucket(reason);
                        *degraded_buckets.entry(bucket).or_default() += 1;
                        degraded_examples
                            .entry(bucket)
                            .or_insert_with(|| format!("{label}: {reason}"));
                        println!("{label}: degraded: {reason}");
                        continue;
                    }
                    let clock = Instant::now();
                    let mut producer =
                        match UnitDeltaProducer::open(&service, dbnum, base_sesno, target_sesno) {
                            Ok(producer) => producer,
                            Err(error) => {
                                stats.windows_skipped += 1;
                                println!("skip {label}: open: {error:#}");
                                continue;
                            }
                        };
                    stats.plan_secs += clock.elapsed().as_secs_f64();
                    let base = producer.base.clone();
                    let target = producer.target.clone();
                    let settled: BTreeSet<_> = roots_s.iter().map(|root| root.refno).collect();
                    let no_model_only = sources.no_model_only(&roots_s, &settled, &impact, &plan);
                    let at_s: BTreeSet<(u32, u32)> = roots_s
                        .iter()
                        .map(|root| {
                            let refno = refno_to_e3d(root.refno);
                            (refno.word0, refno.word1)
                        })
                        .collect();
                    let candidates: Vec<&GenerationRoot> = plan
                        .regen
                        .iter()
                        .filter(|root| {
                            let refno = refno_to_e3d(root.root);
                            at_s.contains(&(refno.word0, refno.word1))
                        })
                        .collect();
                    stats.roots_at_s += candidates.len();
                    let stride = match roots_per_window {
                        0 => 1,
                        limit => candidates.len().div_ceil(limit.max(1)).max(1),
                    };
                    let mut cache = CatalogueMeshCache::default();
                    for root in candidates.iter().step_by(stride) {
                        if started.elapsed().as_secs_f64() > budget_secs {
                            budget_exhausted = true;
                            stats.over_budget += 1;
                            continue;
                        }
                        let refno = refno_to_e3d(root.root);
                        let root_no_model_only =
                            no_model_only.roots.contains(&refno_from_e3d(refno));
                        stats.tried += 1;

                        // 与单根探针同一套三步；每根一份干净的本地投影。
                        let store = ModelMemoryStore::default();
                        // 整根重算里画不出的件（e3d-model 没有 builder / 读挂）**不拦**：生产的整根重算对这种根也是
                        // 「能画的照发、失败的记账」，两端同一套生成器，失败集相同则章仍该相同；件数记进报告。
                        // 引用了不在当前 MDB 里的库的件例外：那是配置与语料不配套，生产里不会这样缺，整根记跳过。
                        let regen_at = |sesno: u32,
                                        input: &GenerationInput,
                                        cache: &mut CatalogueMeshCache|
                         -> anyhow::Result<(
                            ProjectionCommit,
                            usize,
                            BTreeSet<u32>,
                        )> {
                            let snapshot =
                                generate_snapshot_from_set(&input.set, &input.index, refno, cache)?;
                            let failed = snapshot.report.failed.len();
                            let foreign = snapshot
                                .report
                                .failed
                                .iter()
                                .flat_map(|incident| {
                                    crate::data_interface::mdb_route_directory::unregistered_dbnums(
                                        &incident.detail,
                                    )
                                })
                                .collect::<BTreeSet<_>>();
                            let owners = snapshot.owners.clone();
                            let baked = bake_snapshot(snapshot, &mesh_dir, None)?;
                            let (commit, _) = project_geometry_delta(
                                dbnum,
                                sesno,
                                baked.elements,
                                Vec::new(),
                                None,
                                &owners,
                            )?;
                            Ok((commit, failed, foreign))
                        };
                        let outcome: Outcome = (|| -> anyhow::Result<Outcome> {
                            let (mut seed, seed_failed, seed_foreign) =
                                match regen_at(base_sesno, &base, &mut cache) {
                                    Ok(seed) => seed,
                                    Err(error) => {
                                        return Ok(Outcome::SeedFailed(format!("{error:#}")));
                                    }
                                };
                            if !seed_foreign.is_empty() {
                                return Ok(Outcome::ForeignDependency(seed_foreign));
                            }
                            let (seed_hash, seed_count) = manifest_hash(&seed.upserts)?;
                            seed.receipt = Some((
                                refno,
                                RootReceipt {
                                    source_sesno: base_sesno,
                                    model_target: Some(service.model_target(
                                        dbnum,
                                        base_sesno,
                                        &base.dependency_sessions,
                                    )?),
                                    manifest_hash: seed_hash,
                                    geometry_count: seed_count,
                                    published_via: "regen".into(),
                                    room: None,
                                },
                            ));
                            store.commit(ProjectionScope::Current, vec![seed]);
                            if !store.root_projection_complete(
                                ProjectionScope::Current,
                                refno,
                                &mesh_dir,
                            ) {
                                return Ok(Outcome::SeedFailed(
                                    "seed projection incomplete".into(),
                                ));
                            }
                            let unit = UnitPublishUnit {
                                root_refno: refno.to_string(),
                                source_start_sesno: base_sesno,
                                no_model_only: root_no_model_only,
                            };
                            let clock = Instant::now();
                            let report = republish_roots_by_unit_delta_on(
                                &store,
                                &mesh_dir,
                                target_sesno,
                                std::slice::from_ref(&unit),
                                |unit, root, old_receipt| {
                                    producer.produce(root, old_receipt, unit.no_model_only)
                                },
                                |commits| {
                                    store.try_commit_current(commits);
                                    Ok(())
                                },
                            )?;
                            let unit_secs = clock.elapsed().as_secs_f64();
                            if let Some((_, reason)) = report.fallbacks.first() {
                                return Ok(Outcome::Fallback(reason.clone()));
                            }
                            if !report.replayed.is_empty() {
                                return Ok(Outcome::Replayed);
                            }
                            let receipt_only = !report.receipt_only.is_empty();
                            let clock = Instant::now();
                            let (regen_t, target_failed, target_foreign) =
                                match regen_at(target_sesno, &target, &mut cache) {
                                    Ok(regen_t) => regen_t,
                                    Err(error) => {
                                        return Ok(Outcome::RegenTargetFailed(format!(
                                            "{error:#}"
                                        )));
                                    }
                                };
                            if !target_foreign.is_empty() {
                                return Ok(Outcome::ForeignDependency(target_foreign));
                            }
                            let regen_t_secs = clock.elapsed().as_secs_f64();
                            let (regen_hash, regen_count) = manifest_hash(&regen_t.upserts)?;
                            let receipt = store
                                .receipt(ProjectionScope::Current, refno)
                                .context("unit receipt")?;
                            let records = store.records_of_root(ProjectionScope::Current, refno);
                            let ok = receipt.manifest_hash == regen_hash
                                && receipt.geometry_count == regen_count;
                            let (mut diff, mut extra, mut missing) = (0usize, 0usize, 0usize);
                            if !ok {
                                let unit_entries = records
                                    .iter()
                                    .map(manifest_entry)
                                    .collect::<anyhow::Result<BTreeMap<String, Value>>>(
                                )?;
                                let regen_entries = regen_t
                                    .upserts
                                    .iter()
                                    .map(manifest_entry)
                                    .collect::<anyhow::Result<BTreeMap<String, Value>>>(
                                )?;
                                for key in unit_entries
                                    .keys()
                                    .chain(regen_entries.keys())
                                    .collect::<BTreeSet<_>>()
                                {
                                    match (unit_entries.get(key), regen_entries.get(key)) {
                                        (Some(unit), Some(regen)) if unit == regen => {}
                                        (Some(_), Some(_)) => diff += 1,
                                        (Some(_), None) => extra += 1,
                                        (None, Some(_)) => missing += 1,
                                        (None, None) => unreachable!(),
                                    }
                                }
                            }
                            Ok(Outcome::Parity {
                                ok,
                                receipt_only,
                                records: records.len(),
                                unit_secs,
                                regen_t_secs,
                                diff,
                                extra,
                                missing,
                                failed_elements: (seed_failed, target_failed),
                            })
                        })()
                        .unwrap_or_else(|error| {
                            Outcome::SeedFailed(format!("probe error: {error:#}"))
                        });

                        match outcome {
                            Outcome::Parity {
                                ok,
                                receipt_only,
                                records,
                                unit_secs,
                                regen_t_secs,
                                diff,
                                extra,
                                missing,
                                failed_elements,
                            } => {
                                stats.unit_secs += unit_secs;
                                stats.regen_t_secs += regen_t_secs;
                                if failed_elements != (0, 0) {
                                    stats.with_failed_elements += 1;
                                }
                                let note = format!(
                                    "{}{}",
                                    if receipt_only {
                                        " (receipt-only, D10)"
                                    } else {
                                        ""
                                    },
                                    if failed_elements != (0, 0) {
                                        format!(
                                            " [failed elements S {} / T {}]",
                                            failed_elements.0, failed_elements.1
                                        )
                                    } else {
                                        String::new()
                                    }
                                );
                                if ok {
                                    stats.parity_ok += 1;
                                    if receipt_only {
                                        stats.receipt_only += 1;
                                    }
                                    println!(
                                        "{label}: PARITY OK {refno} `{}` {records} records{note} unit {unit_secs:.2}s regen {regen_t_secs:.2}s",
                                        root.noun
                                    );
                                } else {
                                    stats.diff += 1;
                                    let line = format!(
                                        "{label}: root {refno} `{}` DIFF {diff} EXTRA {extra} MISSING {missing}{note}",
                                        root.noun
                                    );
                                    println!("RED {line}");
                                    diffs.push(line);
                                }
                            }
                            Outcome::ForeignDependency(dbnums) => {
                                stats.foreign_dependency += 1;
                                let named = dbnums
                                    .iter()
                                    .map(u32::to_string)
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                foreign_dbnums.extend(dbnums);
                                if foreign_examples.len() < 40 {
                                    foreign_examples.push(format!(
                                        "{label} {refno} `{}`: db {named}",
                                        root.noun
                                    ));
                                }
                                println!(
                                    "{label}: skip {refno} `{}`: db {named} not in this MDB",
                                    root.noun
                                );
                            }
                            Outcome::Fallback(reason) => {
                                stats.fallback += 1;
                                let bucket = fallback_bucket(&reason);
                                *fallback_buckets.entry(bucket).or_default() += 1;
                                fallback_examples.entry(bucket).or_insert_with(|| {
                                    format!("{label} {refno} `{}`: {reason}", root.noun)
                                });
                                println!("{label}: fallback {refno} `{}`: {bucket}", root.noun);
                            }
                            Outcome::Replayed => {
                                stats.replayed += 1;
                                println!(
                                    "{label}: replayed {refno} (unexpected: seed receipt is at S)"
                                );
                            }
                            Outcome::SeedFailed(error) => {
                                stats.seed_failed += 1;
                                failures.push(format!(
                                    "{label} {refno} `{}` seed@S: {error}",
                                    root.noun
                                ));
                            }
                            Outcome::RegenTargetFailed(error) => {
                                stats.regen_failed += 1;
                                failures.push(format!(
                                    "{label} {refno} `{}` regen@T: {error}",
                                    root.noun
                                ));
                            }
                        }
                    }
                }
                per_file.push((name, stats));
            }
        }

        // 汇总。
        let total = |pick: fn(&FileStats) -> usize| -> usize {
            per_file.iter().map(|(_, stats)| pick(stats)).sum()
        };
        let total_f = |pick: fn(&FileStats) -> f64| -> f64 {
            per_file.iter().map(|(_, stats)| pick(stats)).sum()
        };
        let elapsed = started.elapsed().as_secs_f64();
        let mut report = String::new();
        report.push_str(&format!(
            "\n## 运行 {}：`DB_OPTION_FILE={db_option_file}`{}\n\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            if shards > 1 {
                format!("，分片 {shard}/{shards}（按窗轮流）")
            } else {
                String::new()
            }
        ));
        report.push_str(&format!(
            "- 开关：{switches}\n- 语料：{corpus_label}\n- 抽样：每库最后 {} 步（0 = 整条链），每窗最多 {} 根（跨 regen 名单等距抽，只抽 S 端也有的根）；预算 {budget_secs:.0}s{}；总耗时 {elapsed:.0}s\n\n",
            per_db,
            roots_per_window,
            if budget_exhausted { "（**已用尽**，其后只数不量）" } else { "" },
        ));
        report.push_str("| 合计 | 值 |\n|---|---|\n");
        for (name, value) in [
            (
                "有窗的库",
                per_file.iter().filter(|(_, s)| s.windows > 0).count(),
            ),
            (
                "跳过的库（pin 不进 / 会话不足 / 扫描失败）",
                skipped_files.len(),
            ),
            ("窗（规划成功）", total(|s| s.windows)),
            (
                "窗（跳过：开不了 / 收不了 / 预算）",
                total(|s| s.windows_skipped),
            ),
            ("窗（退化，只数不量）", total(|s| s.windows_degraded)),
            ("regen 根（全量）", total(|s| s.regen_roots)),
            ("其中 S 端也有的根（可走单元路）", total(|s| s.roots_at_s)),
            ("量到的根", total(|s| s.tried)),
            ("**PARITY OK**", total(|s| s.parity_ok)),
            (
                "其中 receipt-only（D10 空 Partial）",
                total(|s| s.receipt_only),
            ),
            ("**DIFF（红）**", total(|s| s.diff)),
            ("回退整根 regen", total(|s| s.fallback)),
            ("重放（不该出现）", total(|s| s.replayed)),
            ("S 端整根生成报错（量不了）", total(|s| s.seed_failed)),
            ("T 端整根生成报错（量不了）", total(|s| s.regen_failed)),
            (
                "对拍过的根里两端带画不出的件的（照常对拍，只记）",
                total(|s| s.with_failed_elements),
            ),
            (
                "依赖库不在当前 MDB（记跳过，不算对拍）",
                total(|s| s.foreign_dependency),
            ),
            ("预算用尽未量", total(|s| s.over_budget)),
        ] {
            report.push_str(&format!("| {name} | {value} |\n"));
        }
        report.push_str(&format!(
            "| 单元路总耗时 / T 端整根重算总耗时（只算走成对拍的根） | {:.1}s / {:.1}s |\n| 开窗（两端 DbSet + plan_update）总耗时 | {:.1}s |\n\n",
            total_f(|s| s.unit_secs),
            total_f(|s| s.regen_t_secs),
            total_f(|s| s.plan_secs),
        ));
        report.push_str("### 逐库\n\n| 库 | 窗 | 退化窗 | regen 根 | S 端也有 | 量到 | OK | receipt-only | DIFF | 回退 | 生成失败 | unit s | regen s |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
        for (name, s) in &per_file {
            // 一根 regen 都没有、也没跳过什么的库不占行（语料里大半是这种）。
            if s.regen_roots == 0 && s.windows_skipped == 0 && s.windows_degraded == 0 {
                continue;
            }
            report.push_str(&format!(
                "| {name} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {:.1} | {:.1} |\n",
                s.windows,
                s.windows_degraded,
                s.regen_roots,
                s.roots_at_s,
                s.tried,
                s.parity_ok,
                s.receipt_only,
                s.diff,
                s.fallback,
                s.seed_failed + s.regen_failed,
                s.unit_secs,
                s.regen_t_secs,
            ));
        }
        report.push_str("\n### 回退原因（按类）\n\n| 类 | 根数 | 一例 |\n|---|---|---|\n");
        for (bucket, count) in &fallback_buckets {
            let example = fallback_examples
                .get(bucket)
                .map(|line| line.replace('|', "\\|"))
                .unwrap_or_default();
            report.push_str(&format!("| {bucket} | {count} | {example} |\n"));
        }
        report.push_str(
            "\n### 退化窗原因（按类，只数不量）\n\n| 类 | 窗数 | 一例 |\n|---|---|---|\n",
        );
        for (bucket, count) in &degraded_buckets {
            let example = degraded_examples
                .get(bucket)
                .map(|line| line.replace('|', "\\|"))
                .unwrap_or_default();
            report.push_str(&format!("| {bucket} | {count} | {example} |\n"));
        }
        if !diffs.is_empty() {
            report.push_str("\n### **红：DIFF**\n\n");
            for line in &diffs {
                report.push_str(&format!("- {line}\n"));
            }
        }
        if !failures.is_empty() {
            report.push_str(&format!(
                "\n### 生成失败（量不了，非红）：{} 根，前 40 条\n\n",
                failures.len()
            ));
            for line in failures.iter().take(40) {
                report.push_str(&format!("- {}\n", line.replace('\n', " ")));
            }
        }
        if !foreign_examples.is_empty() {
            report.push_str(&format!(
                "\n### 依赖库不在当前 MDB（记跳过，不算对拍）：{} 根，库 {}，前 40 条\n\n",
                total(|s| s.foreign_dependency),
                foreign_dbnums
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            for line in &foreign_examples {
                report.push_str(&format!("- {line}\n"));
            }
        }
        if !skipped_files.is_empty() {
            report.push_str(&format!("\n### 跳过的库：{} 个\n\n", skipped_files.len()));
            for (name, reason) in &skipped_files {
                report.push_str(&format!("- {name}: {}\n", reason.replace('\n', " ")));
            }
        }
        if let Some(parent) = report_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let fresh = !report_path.exists();
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&report_path)?;
        if fresh {
            out.write_all(
                "# ADR-080 A6 / D9 ②：单元级发布全语料对拍（`probe_unit_publish_corpus_against_regen`）\n\n\
                 每根：S 端整根重算灌基线 → 单元路（真生产方 + 执行体）→ 与 T 端整根重算比整根 `manifest_hash`。\
                 一根 DIFF 即红；回退 / 重放 / 生成失败只计数。目录依赖经当前 MDB 解，一次一个 `DB_OPTION_FILE`。\n"
                    .as_bytes(),
            )?;
        }
        out.write_all(report.as_bytes())?;
        println!("{report}");
        println!("report appended to {}", report_path.display());
        // 机器可读的一行汇总：几份分片相加就是全语料。
        println!(
            "SUMMARY_JSON {}",
            json!({
                "db_option_file": db_option_file,
                "shard": format!("{shard}/{shards}"),
                "elapsed_secs": elapsed,
                "budget_exhausted": budget_exhausted,
                "dbs_with_windows": per_file.iter().filter(|(_, s)| s.windows > 0).count(),
                "dbs_skipped": skipped_files.len(),
                "windows": total(|s| s.windows),
                "windows_skipped": total(|s| s.windows_skipped),
                "windows_degraded": total(|s| s.windows_degraded),
                "regen_roots": total(|s| s.regen_roots),
                "roots_at_s": total(|s| s.roots_at_s),
                "tried": total(|s| s.tried),
                "parity_ok": total(|s| s.parity_ok),
                "receipt_only": total(|s| s.receipt_only),
                "diff": total(|s| s.diff),
                "fallback": total(|s| s.fallback),
                "replayed": total(|s| s.replayed),
                "seed_failed": total(|s| s.seed_failed),
                "regen_failed": total(|s| s.regen_failed),
                "with_failed_elements": total(|s| s.with_failed_elements),
                "foreign_dependency": total(|s| s.foreign_dependency),
                "foreign_dbnums": foreign_dbnums,
                "over_budget": total(|s| s.over_budget),
                "unit_secs": total_f(|s| s.unit_secs),
                "regen_t_secs": total_f(|s| s.regen_t_secs),
                "plan_secs": total_f(|s| s.plan_secs),
                "fallback_buckets": fallback_buckets,
                "degraded_buckets": degraded_buckets,
            })
        );
        if !keep_meshes && std::env::var("AIOS_UNIT_PUBLISH_MESH_DIR").is_err() {
            let _ = std::fs::remove_dir_all(&mesh_dir);
        }
        assert!(
            total(|s| s.tried) > 0,
            "一根都没量到——DB_OPTION_FILE / AIOS_POSREF_DIRS 不对，或全被跳过"
        );
        assert!(
            diffs.is_empty(),
            "D9 ①：单元路与 regen 路的整根 manifest_hash 必须逐字相同，红了 {} 根：\n{}",
            diffs.len(),
            diffs.join("\n")
        );
        Ok(())
    }

    /// ADR-080 A7 / D9 ③（手动）：单元路一窗的**固定开销**。生产里规划器已在同一窗上跑过 `collect_window` +
    /// `plan_update`，单元路的生产方（`UnitDeltaProducer::open`，每窗第一根单元路的根触发一次）再开一次：两端
    /// `generation_input` + 再算一遍计划。生成输入缓存**每库只留一条**（ADR-084），S 与 T 同库，所以这一开至少要
    /// 重建一端 `DbSet` / 索引——上面那支全语料对拍只比了逐根的单元路 vs 整根路，这一笔没进去。这里对给定的窗各开
    /// 一次、计时，就是单元路比整根路多付的那一笔。
    ///
    /// 窗口清单 `AIOS_A7_WINDOWS_FILE` 一行一个 `文件名 S T`（取自对拍日志里有 PARITY OK 的窗），文件在
    /// `AIOS_POSREF_DIRS` 里找；目录依赖经当前 MDB（`DB_OPTION_FILE`）。每窗一行 `A7_OPEN 文件名 S T 秒`，末尾 `A7_SUMMARY`。
    #[tokio::test]
    #[ignore = "manual live: needs DB_OPTION_FILE (MDB), the corpus directories and AIOS_A7_WINDOWS_FILE"]
    async fn probe_unit_producer_open_cost_per_window() -> anyhow::Result<()> {
        use std::time::Instant;

        use crate::data_interface::direct_store::DbPin;
        use crate::data_interface::generation_root::resolve_delivery_unit_types;
        use crate::data_interface::reverse_reference::sources_in_dir;
        use crate::data_interface::window_root_plan::WindowRootSources;

        let list = std::fs::read_to_string(std::env::var("AIOS_A7_WINDOWS_FILE")?)?;
        let dirs: Vec<PathBuf> = std::env::var("AIOS_POSREF_DIRS")
            .map(|value| value.split(';').map(PathBuf::from).collect())
            .unwrap_or_default();
        let mut files: BTreeMap<String, (u32, PathBuf)> = BTreeMap::new();
        for dir in dirs.iter().filter(|dir| dir.is_dir()) {
            for source in sources_in_dir(dir, &["DESI"])? {
                let name = source
                    .file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                files.insert(name, (source.dbnum, source.file.clone()));
            }
        }
        let mut service = E3dModelService::from_current().await?;
        let unit_types = resolve_delivery_unit_types(&[]);
        let (mut total, mut windows, mut skipped) = (0.0f64, 0usize, 0usize);
        for line in list.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let [name, base, target] = parts.as_slice() else {
                continue;
            };
            let (Ok(base), Ok(target)) = (base.parse::<u32>(), target.parse::<u32>()) else {
                continue;
            };
            let Some((dbnum, file)) = files.get(*name).cloned() else {
                println!("A7_SKIP {line}: not in AIOS_POSREF_DIRS");
                skipped += 1;
                continue;
            };
            service.pins.retain(|pin| pin.dbnum != dbnum as i32);
            service.pins.push(DbPin {
                dbnum: dbnum as i32,
                db_type: "DESI".into(),
                file: file.clone(),
                sesno: None,
            });
            // 与生产一致：规划器先跑完本窗（`build_set` + `WindowRootSources::collect`），再量 `UnitDeltaProducer::open` 额外开销。
            let pre_plan = (|| -> anyhow::Result<()> {
                let base_set = service.build_set(dbnum, Some(base))?;
                let target_set = service.build_set(dbnum, Some(target))?;
                let _ = WindowRootSources::collect(
                    &file,
                    base,
                    target,
                    &base_set,
                    &target_set,
                    &unit_types,
                    &[],
                )?;
                Ok(())
            })();
            if let Err(error) = pre_plan {
                println!("A7_SKIP {line}: {error:#}");
                skipped += 1;
                continue;
            }
            let clock = Instant::now();
            let opened = UnitDeltaProducer::open(&service, dbnum, base, target);
            let secs = clock.elapsed().as_secs_f64();
            match opened {
                Ok(_) => {
                    println!("A7_OPEN {name} {base} {target} {secs:.4}");
                    total += secs;
                    windows += 1;
                }
                Err(error) => {
                    println!("A7_SKIP {line}: {error:#}");
                    skipped += 1;
                }
            }
        }
        println!("A7_SUMMARY windows={windows} skipped={skipped} open_secs={total:.1}");
        assert!(
            windows > 0,
            "一窗都没开成——清单 / AIOS_POSREF_DIRS / DB_OPTION_FILE 不对"
        );
        Ok(())
    }

    /// ADR-080 实施约束 5：单元级执行体里闸在产出之前、产出在提交之前，且执行体自己**不读任何记录、
    /// 不自己算章**——删什么由产出点名，整根的章由存储在写锁里盖（D5）。闸后面要是长出一个
    /// `records_of_root`，就又多了一个能读到旧状态的窗口。闸与刚体路同一道、同一个枚举，不另造判据。
    #[test]
    fn the_unit_publish_gate_precedes_production_and_the_executor_reads_no_records() {
        let source = service_source();
        let body = source
            .split_once("pub(crate) fn republish_roots_by_unit_delta_on(")
            .expect("unit executor")
            .1
            .split_once("\nfn cached_root_commit(")
            .expect("unit executor end")
            .0;
        let complete = body
            .find("store.root_projection_complete(")
            .expect("completeness precondition");
        let gate = body.find("republish_gate(").expect("gate");
        let production = body
            .find("produce(unit, root, &old_receipt)")
            .expect("production");
        let tube_gate = body
            .find("tube_segment_completeness(&delta)")
            .expect("tube segment completeness gate");
        // D10：空清单只许来自带 `no_model_only` 提示的根，判在管段门之后、提交之前。
        let empty_guard = body
            .find("receipt_only && !unit.no_model_only")
            .expect("D10 empty-delta guard");
        // 提交攒成一批、循环结束一次发（D4 的原子边界）。
        let publication = body.find("publish(commits)?").expect("publication");
        assert!(
            complete < gate
                && gate < production
                && production < tube_gate
                && tube_gate < empty_guard
                && empty_guard < publication,
            "顺序必须是 完整性前置 → 闸 → 产出 → 管段完整性门 → 空清单守卫（D10）→ 提交"
        );
        for forbidden in [
            "records_of_root(",
            "geometry_ids_of_root(",
            "root_snapshot(",
            "manifest_hash(",
        ] {
            assert!(
                !body.contains(forbidden),
                "执行体不读记录、不自己算章：{forbidden}"
            );
        }
        assert!(
            body.contains("RepublishGate::SkipAlreadyAtTarget")
                && body.contains("RepublishGate::FallbackBaselineMismatch"),
            "闸的两种否决都要接住，与刚体路同一个枚举"
        );
        assert!(
            body.contains("scope: CommitScope::Partial"),
            "局部提交必须显式声明 Partial（D2）"
        );
        assert!(body.contains("published_via: \"unit\".into()"));
        let service_method = source
            .split_once("pub(crate) async fn republish_roots_by_unit_delta(")
            .expect("service entry")
            .1
            .split_once("\n    }\n")
            .expect("service entry end")
            .0;
        assert!(
            service_method.contains("db_generation_lock(dbnum).lock_owned().await")
                && service_method.contains("ensure_not_older_than_persisted(dbnum, target_sesno)"),
            "与刚体路同一把锁、同一道会话闸（D4：不新增锁）"
        );
    }

    /// 目标变了但这一根看得见的清单逐位没变：只留回执，几何一条都不写。
    #[test]
    fn an_unchanged_manifest_commits_only_the_receipt() {
        let root = RefNo::new(1, 2);
        let empty_manifest = serde_json::to_vec(&BTreeMap::<String, Value>::new()).unwrap();
        let publication = RootPublication {
            root,
            model_target: None,
            published: Some(RootReceipt {
                source_sesno: 42,
                model_target: None,
                manifest_hash: hex::encode(Sha256::digest(empty_manifest)),
                geometry_count: 0,
                published_via: "regen".into(),
                room: None,
            }),
            projection_complete: true,
        };
        let (commit, report) = project_geometry_delta(
            8000,
            42,
            Vec::new(),
            vec![GeometryId::Element {
                refno: "8000/9".into(),
            }],
            Some(&publication),
            &BTreeMap::new(),
        )
        .expect("prepare unchanged root");

        assert_eq!((report.upserted, report.removed), (0, 0));
        assert!(commit.removals.is_empty(), "{commit:?}");
        assert!(commit.upserts.is_empty(), "{commit:?}");
        let (receipt_root, receipt) = commit.receipt.expect("receipt still lands");
        assert_eq!(receipt_root, root);
        assert_eq!(receipt.geometry_count, 0);
    }

    /// 上一轮的投影不完整（比如 mesh 文件被删过）时，同一份清单必须照旧重写一遍。
    #[test]
    fn an_unchanged_manifest_still_repairs_an_incomplete_projection() {
        let root = RefNo::new(24383, 73948);
        let empty_manifest = serde_json::to_vec(&BTreeMap::<String, Value>::new()).unwrap();
        let publication = RootPublication {
            root,
            model_target: None,
            published: Some(RootReceipt {
                source_sesno: 42,
                model_target: None,
                manifest_hash: hex::encode(Sha256::digest(empty_manifest)),
                geometry_count: 0,
                published_via: "regen".into(),
                room: None,
            }),
            projection_complete: false,
        };
        let (commit, report) = project_geometry_delta(
            8000,
            42,
            Vec::new(),
            vec![GeometryId::Element {
                refno: "8000/9".into(),
            }],
            Some(&publication),
            &BTreeMap::new(),
        )
        .expect("prepare repair projection");
        assert_eq!(report.removed, 1);
        assert_eq!(commit.removals, vec!["8000_9".to_string()]);
    }

    /// 不可变目标与源会话都没变，投影也完整：这一根连几何都不必求值，只重盖一次章。
    #[test]
    fn an_identical_immutable_target_reprojects_before_any_geometry_build() {
        let target = ModelTarget {
            project: "fixture".into(),
            design: ModelFileTarget {
                dbnum: 8000,
                db_type: "DESI".into(),
                file: "design.db".into(),
                session: 42,
                digest: "design".into(),
            },
            catalogue: Vec::new(),
            template_attlib_digest: "attlib".into(),
            generator_fingerprint: "generator".into(),
            tessellation_profile: "tessellation".into(),
        };
        let publication = RootPublication {
            root: RefNo::new(1, 2),
            model_target: Some(target.clone()),
            published: Some(RootReceipt {
                source_sesno: 42,
                model_target: Some(target),
                manifest_hash: "manifest".into(),
                geometry_count: 3,
                published_via: "regen".into(),
                room: None,
            }),
            projection_complete: true,
        };
        assert!(cached_root_target_matches(&publication, 42));
        assert!(
            !cached_root_target_matches(&publication, 43),
            "源会话变了就不算命中"
        );

        let commit = cached_root_commit(&publication, 42).expect("cached commit");
        assert!(commit.upserts.is_empty());
        assert!(commit.removals.is_empty());
        assert!(commit.tube_branch_resets.is_empty());
        let (_, receipt) = commit.receipt.expect("receipt");
        assert_eq!(receipt.manifest_hash, "manifest");
        assert_eq!(receipt.geometry_count, 3);
    }

    /// 上一轮没投过、或者投的是另一个目标，都不许走「不必重算」那条路。
    #[test]
    fn a_missing_or_different_target_never_hits_the_cache() {
        let mut publication = RootPublication {
            root: RefNo::new(1, 2),
            model_target: None,
            published: None,
            projection_complete: false,
        };
        assert!(!cached_root_target_matches(&publication, 42));

        publication.published = Some(RootReceipt {
            source_sesno: 42,
            model_target: None,
            manifest_hash: "manifest".into(),
            geometry_count: 0,
            published_via: "regen".into(),
            room: None,
        });
        assert!(
            !cached_root_target_matches(&publication, 42),
            "本轮算不出目标就不能声称命中"
        );
    }

    #[test]
    fn read_through_receipt_is_invalidated_by_each_target_identity_change() {
        let file = ModelFileTarget {
            dbnum: 8000,
            db_type: "DESI".into(),
            file: "design.db".into(),
            session: 459,
            digest: "generation-a".into(),
        };
        let target = ModelTarget {
            project: "fixture".into(),
            design: file.clone(),
            catalogue: vec![ModelFileTarget {
                dbnum: 7000,
                db_type: "CATA".into(),
                ..file
            }],
            template_attlib_digest: "schema-a".into(),
            generator_fingerprint: "generator-a".into(),
            tessellation_profile: "profile-a".into(),
        };
        let mut publication = RootPublication {
            root: RefNo::new(24384, 1),
            model_target: Some(target.clone()),
            published: Some(RootReceipt {
                source_sesno: 459,
                model_target: Some(target.clone()),
                manifest_hash: "manifest".into(),
                geometry_count: 9,
                published_via: "regen".into(),
                room: None,
            }),
            projection_complete: true,
        };
        assert!(cached_root_target_matches(&publication, 459));
        for change in 0..7 {
            let mut changed = target.clone();
            match change {
                0 => changed.design.session = 461,                  // SAVEWORK
                1 => changed.design.session = 457,                  // rollback
                2 => changed.design.digest = "generation-b".into(), // replacement
                3 => changed.catalogue[0].session += 1,
                4 => changed.catalogue[0].file = "other-project.db".into(),
                5 => changed.template_attlib_digest = "schema-b".into(),
                _ => changed.generator_fingerprint = "generator-b".into(),
            }
            publication.model_target = Some(changed);
            assert!(
                !cached_root_target_matches(&publication, 459),
                "change {change}"
            );
        }
    }

    #[test]
    fn model_file_target_digest_pins_file_identity_and_session() {
        let path = std::env::temp_dir().join(format!("e3d-model-target-{}.db", std::process::id()));
        std::fs::write(&path, b"fixture-v1").expect("write fixture");
        let first = model_file_identity_digest(&path, 41).expect("first digest");
        assert_eq!(
            first,
            model_file_identity_digest(&path, 41).expect("repeat digest"),
            "the same immutable target must serialize deterministically"
        );
        assert_ne!(
            first,
            model_file_identity_digest(&path, 42).expect("new session digest"),
            "session is part of target identity"
        );
        std::fs::write(&path, b"fixture-v2-longer").expect("replace fixture");
        assert_ne!(
            first,
            model_file_identity_digest(&path, 41).expect("replacement digest"),
            "same path with replaced content identity is a new target"
        );
        let _ = std::fs::remove_file(path);
    }

    /// 初始化服务记进 `frozen_file_digests` 的是 **token 摘要**，不是现场文件身份：同一份冻结 token
    /// 重开、文件被追加过也得算出同一个数（room-input-cache 的前提，见
    /// `initialization_target_tests::initialization_target_survives_append_and_reopen_but_rejects_replacement`）。
    /// 2026-09-19 差一点被改成 `model_file_identity_digest`——那会让每次 SAVEWORK 都把面板几何缓存打掉。
    #[test]
    fn initialization_keeps_the_token_digest_so_appends_do_not_change_the_target() {
        let body = include_str!("e3d_model_service.rs")
            .split_once("pub async fn from_initialization_sources(")
            .expect("from_initialization_sources exists")
            .1
            .split_once("pub fn initialization_target_hash(")
            .expect("from_initialization_sources end")
            .0;
        assert!(
            body.contains("Sha256::digest(frozen.commit_token_json()"),
            "{body}"
        );
        assert!(
            !body.contains("model_file_identity_digest(frozen.path()"),
            "{body}"
        );
    }

    /// 7bde2aeea 把字典库按会话位 0 塞进 `catalogue` 之后，凡把 `session` 当真会话用的地方
    /// （`target_current` 对文件最新会话、库级目录落款）都得绕开它：0 不是任何一个真会话。
    #[test]
    fn dictionary_entries_carry_no_session_and_stay_out_of_catalogue_stamps() {
        let file = |dbnum: u32, db_type: &str, session: u32| ModelFileTarget {
            dbnum,
            db_type: db_type.into(),
            file: format!("{db_type}-{dbnum}"),
            session,
            digest: format!("{db_type}@{session}"),
        };
        let target = ModelTarget {
            project: "AMS".into(),
            design: file(7997, "DESI", 332),
            catalogue: vec![
                file(5100, "DICT", 0),
                file(7000, "CATA", 11),
                file(7006, "DICT", 0),
                file(7043, "CATA", 0),
            ],
            template_attlib_digest: "attlib".into(),
            generator_fingerprint: "gen".into(),
            tessellation_profile: "e3d-default-v1".into(),
        };
        assert!(target.design.session_tracked());
        assert_eq!(
            target
                .catalogue
                .iter()
                .map(ModelFileTarget::session_tracked)
                .collect::<Vec<_>>(),
            [false, true, false, false],
            "会话位 0 的项——字典库、没钉住也没打开过的目录库——都算没记会话"
        );
        assert_eq!(
            target.catalogue_sessions().collect::<Vec<_>>(),
            [(7000, 11)]
        );
    }

    #[test]
    fn element_and_implied_tubes_have_disjoint_persistence_ids() {
        let source = RefNo::new(17496, 152095);
        let element = GeometryId::Element {
            refno: source.to_string(),
        };
        let tube_a = GeometryId::ImpliedTube {
            container_refno: source.to_string(),
            route_ordinal: 0,
            from_refno: "17496/1".into(),
            to_refno: "17496/2".into(),
        };
        let tube_b = GeometryId::ImpliedTube {
            container_refno: source.to_string(),
            route_ordinal: 1,
            from_refno: "17496/2".into(),
            to_refno: "17496/3".into(),
        };

        let element_id = geometry_record_id(&element, source);
        let tube_a_id = geometry_record_id(&tube_a, source);
        let tube_b_id = geometry_record_id(&tube_b, source);
        assert_eq!(element_id, "17496_152095");
        assert_ne!(element_id, tube_a_id);
        assert_ne!(tube_a_id, tube_b_id);
    }

    #[test]
    fn implied_tube_flattens_canonical_local_transform_into_the_stored_world_transform() {
        let geometry_id = GeometryId::ImpliedTube {
            container_refno: "24383/73948".into(),
            route_ordinal: 0,
            from_refno: "24383/10".into(),
            to_refno: "24383/20".into(),
        };
        let world = DMat4::from_translation(glam::DVec3::new(10.0, 20.0, 30.0));
        let local = DMat4::from_scale(glam::DVec3::new(100.0, 100.0, 3000.0));
        let primitive = e3d_model::primitive_instance::PrimitiveInstance {
            key: e3d_model::primitive_instance::PrimitiveMeshKey::CylinderV3 { segments: 32 },
            local_transform: local,
        };
        assert_eq!(
            persisted_world_transform(&geometry_id, Some(&primitive), world),
            world * local
        );

        let element = GeometryId::Element {
            refno: "24383/73948".into(),
        };
        assert_eq!(
            persisted_world_transform(&element, Some(&primitive), world),
            world,
            "ordinary elements keep the separate local-transform branch"
        );
    }

    fn fixture_target(design_session: u32) -> ModelTarget {
        ModelTarget {
            project: "fixture".into(),
            design: ModelFileTarget {
                dbnum: 8000,
                db_type: "DESI".into(),
                file: "design.db".into(),
                session: design_session,
                digest: format!("design@{design_session}"),
            },
            catalogue: vec![ModelFileTarget {
                dbnum: 7000,
                db_type: "CATA".into(),
                file: "cata.db".into(),
                session: 7,
                digest: "cata@7".into(),
            }],
            template_attlib_digest: "attlib".into(),
            generator_fingerprint: "generator".into(),
            tessellation_profile: "tessellation".into(),
        }
    }

    fn receipt_at(source_sesno: u32, model_target: Option<ModelTarget>) -> RootReceipt {
        RootReceipt {
            source_sesno,
            model_target,
            manifest_hash: "manifest".into(),
            geometry_count: 3,
            published_via: "regen".into(),
            room: None,
        }
    }

    /// ADR-066 D4：刚体前移的回执落款是 pin@T 现算的目标，不是旧回执那份；设计窗之外任何一格变了、
    /// 目标没钉在 T 上、或旧回执根本没带目标，都是这根的回退理由。
    #[test]
    fn the_rigid_motion_receipt_takes_the_target_at_t_only_when_nothing_outside_the_window_moved() {
        let old = receipt_at(41, Some(fixture_target(41)));
        let at_t = rigid_motion_target_at(&old, fixture_target(42), 42)
            .expect("only the design window moved");
        assert_eq!(at_t.design.session, 42);
        assert_eq!(at_t.design.digest, "design@42");
        assert_ne!(
            Some(&at_t),
            old.model_target.as_ref(),
            "the receipt must not reuse the old stamp"
        );

        let mut catalogue_moved = fixture_target(42);
        catalogue_moved.catalogue[0].session = 8;
        let error = rigid_motion_target_at(&old, catalogue_moved, 42).unwrap_err();
        assert!(error.to_string().contains("outside the design window"));

        let mut generator_changed = fixture_target(42);
        generator_changed.generator_fingerprint = "generator-v2".into();
        assert!(rigid_motion_target_at(&old, generator_changed, 42).is_err());

        let error = rigid_motion_target_at(&old, fixture_target(41), 42).unwrap_err();
        assert!(error.to_string().contains("pins design session 41"));

        let error =
            rigid_motion_target_at(&receipt_at(41, None), fixture_target(42), 42).unwrap_err();
        assert!(error.to_string().contains("no model target"));
    }

    /// 盒 [0,1]×[0,2]×[0,3] 的八个角点，写成 `dir/<mesh_id>.mesh`。
    fn box_mesh_file(dir: &Path, mesh_id: &str) {
        let corners = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 2.0, 0.0],
            [1.0, 2.0, 0.0],
            [0.0, 0.0, 3.0],
            [1.0, 0.0, 3.0],
            [0.0, 2.0, 3.0],
            [1.0, 2.0, 3.0],
        ];
        let mesh = PlantMesh {
            indices: vec![0, 1, 2],
            vertices: corners
                .iter()
                .map(|[x, y, z]| glam::Vec3::new(*x, *y, *z))
                .collect(),
            normals: vec![glam::Vec3::Z; corners.len()],
            wire_vertices: Vec::new(),
            aabb: None,
        };
        ensure_mesh_file(&dir.join(format!("{mesh_id}.mesh")), &mesh).expect("fixture mesh");
    }

    fn projected(
        record_id: &str,
        mesh_id: &str,
        shape: ProjectedShape,
        tube: Option<ProjectedTube>,
    ) -> ProjectedGeometry {
        let source = RefNo::new(24384, 11);
        ProjectedGeometry {
            record_id: record_id.into(),
            geometry_id: GeometryId::Element {
                refno: source.to_string(),
            },
            source_refno: source,
            dbnum: 7997,
            source_sesno: 41,
            noun: "BOX".into(),
            mesh_id: mesh_id.into(),
            world_transform: DMat4::IDENTITY,
            world_bounds: [[0.0; 3], [1.0, 2.0, 3.0]],
            ancestors: vec![packed_refno(source)],
            shape,
            tube,
        }
    }

    fn assert_bounds_close(actual: [[f32; 3]; 2], expected: [[f32; 3]; 2]) {
        for (a, e) in actual.iter().flatten().zip(expected.iter().flatten()) {
            assert!((a - e).abs() < 1e-4, "bounds {actual:?} != {expected:?}");
        }
    }

    /// ADR-066 D3：含旋转的 `Δ` 不拿盒过矩阵凑数——读回记录自己的网格、顶点乘到新世界系再取紧 AABB。
    /// 烘焙件直接乘世界矩阵；标准图元先乘 `local_transform`；隐式管身的 `local_transform` 已折进世界矩阵，
    /// 不再乘（与 `persisted_world_transform` 互为镜像）。网格不在是这根的回退理由，不是 panic。
    #[test]
    fn rotating_a_record_rebuilds_its_bounds_from_the_mesh_vertices() {
        let dir = tempfile::tempdir().unwrap();
        box_mesh_file(dir.path(), "fixture_box");
        // 绕 Z 转 90° 再平移 (10, 20, 30)：(x, y, z) → (−y + 10, x + 20, z + 30)。
        let world = DMat4::from_translation(glam::DVec3::new(10.0, 20.0, 30.0))
            * DMat4::from_rotation_z(std::f64::consts::FRAC_PI_2);

        let baked = projected("11", "fixture_box", ProjectedShape::Baked, None);
        assert_bounds_close(
            rigid_motion_mesh_bounds(dir.path(), &baked, world).unwrap(),
            [[8.0, 20.0, 30.0], [10.0, 21.0, 33.0]],
        );

        let scaled = ProjectedShape::Canonical {
            key: e3d_model::primitive_instance::PrimitiveMeshKey::CylinderV3 { segments: 32 },
            local_transform: DMat4::from_scale(glam::DVec3::splat(2.0)),
            local_bounds: [[0.0; 3], [1.0, 2.0, 3.0]],
        };
        let canonical = projected("12", "fixture_box", scaled.clone(), None);
        assert_bounds_close(
            rigid_motion_mesh_bounds(dir.path(), &canonical, world).unwrap(),
            [[6.0, 20.0, 30.0], [10.0, 22.0, 36.0]],
        );

        let tube = projected(
            "13",
            "fixture_box",
            scaled,
            Some(ProjectedTube {
                route_ordinal: 0,
                from: RefNo::new(24384, 1),
                to: RefNo::new(24384, 2),
                invalid: false,
            }),
        );
        assert_bounds_close(
            rigid_motion_mesh_bounds(dir.path(), &tube, world).unwrap(),
            [[8.0, 20.0, 30.0], [10.0, 21.0, 33.0]],
        );

        let missing = projected("14", "fixture_missing", ProjectedShape::Baked, None);
        let error = rigid_motion_mesh_bounds(dir.path(), &missing, world).unwrap_err();
        assert!(
            format!("{error:#}").contains("fixture_missing.mesh"),
            "{error:#}"
        );
    }

    /// 执行体喂给 `republish_records` 的就是这只闭包：旋转的锚点让记录的盒从网格重算、矩阵左乘 `Δ`；
    /// 纯平移的锚点不碰文件（网格不存在也过）。
    #[test]
    fn the_executor_bounds_callback_rebuilds_rotated_records_through_republish_records() {
        use crate::data_interface::generation_root::refno_from_e3d;
        let dir = tempfile::tempdir().unwrap();
        box_mesh_file(dir.path(), "fixture_box");
        let anchor_refno = refno_from_e3d(RefNo::new(24384, 11));
        let rotate = DMat4::from_translation(glam::DVec3::new(10.0, 20.0, 30.0))
            * DMat4::from_rotation_z(std::f64::consts::FRAC_PI_2);

        let moved = republish_records(
            vec![projected("11", "fixture_box", ProjectedShape::Baked, None)],
            &[TransformAnchor {
                refno: anchor_refno,
                delta: rotate,
            }],
            41,
            42,
            |record, world| rigid_motion_mesh_bounds(dir.path(), record, world),
        )
        .expect("rotated record republished");
        assert_eq!(moved[0].world_transform, rotate);
        assert_eq!(moved[0].source_sesno, 42);
        assert_bounds_close(
            moved[0].world_bounds,
            [[8.0, 20.0, 30.0], [10.0, 21.0, 33.0]],
        );

        let shifted = republish_records(
            vec![projected(
                "11",
                "fixture_missing",
                ProjectedShape::Baked,
                None,
            )],
            &[TransformAnchor {
                refno: anchor_refno,
                delta: DMat4::from_translation(glam::DVec3::new(1.0, 2.0, 3.0)),
            }],
            41,
            42,
            |record, world| rigid_motion_mesh_bounds(dir.path(), record, world),
        )
        .expect("a pure translation never opens the mesh");
        assert_eq!(shifted[0].world_bounds, [[1.0, 2.0, 3.0], [2.0, 4.0, 6.0]]);
    }

    /// ADR-066 D3 / D4 两处「待补」已补：执行体对旋转读网格而不是 bail，回执落款是 pin@T 的目标而不是
    /// 旧回执那份，而且目标核对排在读任何记录之前。
    #[test]
    fn the_rigid_motion_executor_rebuilds_rotated_bounds_and_stamps_the_target_at_t() {
        let source = service_source();
        let body = source
            .split_once("pub(crate) async fn republish_roots_by_rigid_motion(")
            .expect("rigid-motion executor")
            .1
            .split_once("pub(crate) async fn republish_roots_by_unit_delta(")
            .expect("rigid-motion executor end")
            .0;
        assert!(
            !body.contains("not yet wired"),
            "rotation must rebuild bounds from the mesh, not bail"
        );
        assert!(body.contains("rigid_motion_mesh_bounds(mesh_dir, record, world)"));
        assert!(body.contains("rigid_motion_target_at(&old_receipt, current, target_sesno)"));
        assert!(
            !body.contains("old_receipt.model_target.clone()"),
            "the transform receipt must carry the target at T"
        );
        let target_check = body.find("rigid_motion_target_at(").expect("target check");
        let records = body.find("store.records_of_root(").expect("record read");
        assert!(
            target_check < records,
            "the immutable-target check precedes touching any record"
        );
        // 按记录的幂等闸要拿到规划基线 S 才判得出「比基线新又不在 T」：执行体必须把
        // `unit.source_start_sesno` 传给 `republish_records`（嵌套根共有记录只乘一次 Δ）。
        let republish = body.find("republish_records(").expect("record republish");
        let republish_call = &body[republish..republish + 400];
        assert!(
            republish_call.contains("unit.source_start_sesno,"),
            "republish_records must receive the planned baseline: {republish_call}"
        );
    }

    #[test]
    fn older_session_cannot_overwrite_equal_or_newer_geometry() {
        assert!(session_not_stale(None, 41));
        assert!(session_not_stale(Some(41), 41));
        assert!(session_not_stale(Some(41), 42));
        assert!(!session_not_stale(Some(42), 41));
    }

    /// ADR-054 constraint 3: a root's database comes from the MDB's DESI files,
    /// never from `pe` (a never-parsed project has no `pe` rows), and the current
    /// projection must not read its timepoint from `dbnum_watermark`.
    #[test]
    fn the_current_projection_reads_neither_pe_nor_the_watermark_table() {
        let source = service_source();
        let source = source.as_str();
        let body = source
            .split_once("pub async fn from_current(")
            .expect("from_current")
            .1
            .split_once("pub async fn generate_roots(")
            .expect("from_current and dbnum_for_roots end before generate_roots")
            .0;
        assert!(body.contains("current_mdb_source_inventory()"));
        assert!(body.contains("model_source::dbnum_of_root("));
        assert!(!body.contains("dbnum_watermark"), "{body}");
        assert!(!body.contains("pins_from_watermark"), "{body}");
    }

    /// 开工探针不许每任务重扫搜索根（ADR-085 D3 追记）：监控目录计划取最近一次权威
    /// 解析的缓存（`latest_for`，startup / SYST 刷新 / 重挂 / 兜底都会刷新它），与紧邻
    /// 的 `get()` 成员清单同一代。`plan_watch_dirs` 是现算口，只属于重做时刻——它出现在
    /// 这里就意味着每个任务开工前都真扫一遍全部搜索根。
    #[test]
    fn the_task_probe_reads_the_cached_watch_dir_plan() {
        let source = service_source();
        let body = source
            .split_once(concat!("fn ", "current_mdb_source_inventory("))
            .expect("current_mdb_source_inventory 还在")
            .1
            .split_once(concat!("pub(crate) fn ", "current_mdb_sources("))
            .expect("current_mdb_source_inventory 在 current_mdb_sources 之前结束")
            .0;
        assert!(body.contains("latest_for(option)"), "{body}");
        assert!(
            !body.contains(concat!("plan_", "watch_dirs(")),
            "探针路径不得现算搜索根（每任务一次的远程 IO）: {body}"
        );
    }

    /// ADR-054 constraint 5: an explicit window target is opened as such; the
    /// current pin is never required to equal it, and a newer projected session
    /// settles the window as covered instead of failing it.
    #[test]
    fn apply_window_opens_the_explicit_target_and_yields_to_newer_publications() {
        let source = service_source();
        let source = source.as_str();
        let body = source
            .split_once("pub async fn apply_window(")
            .expect("apply_window")
            .1
            .split_once("async fn generate_refs(")
            .expect("apply_window ends before generate_refs")
            .0;
        assert!(!body.contains("pin.sesno == Some(target_sesno)"), "{body}");
        assert!(body.contains("persisted_session_newer_than(dbnum, target_sesno)"));
        assert!(body.contains("build_set(dbnum, Some(target_sesno))"));
        assert!(body.contains("scan_index(&pin.file, Some(target_sesno))"));
    }

    fn memory_producer_body(source: &str) -> &str {
        source
            .split_once("async fn produce_slices(")
            .expect("memory producer")
            .1
            .split_once("pub async fn persist_memory_batch(")
            .expect("memory producer end")
            .0
    }

    fn memory_consumer_body(source: &str) -> &str {
        source
            .split_once("pub async fn persist_memory_batch(")
            .expect("memory consumer")
            .1
            .split_once("pub async fn generate_and_persist_roots(")
            .expect("memory consumer end")
            .0
    }

    #[test]
    fn memory_generation_reads_only_the_e3d_source_projection() {
        let source = service_source();
        let source = source.as_str();
        let body = memory_producer_body(source);
        for forbidden in [
            "ModelMemoryStore",
            "ensure_not_older_than_persisted",
            "root_projection_complete",
        ] {
            assert!(
                !body.contains(forbidden),
                "memory producer must get model input only from e3d-io/e3d-model, found {forbidden}"
            );
        }
        assert!(body.contains("spawn_generation_worker("));
    }

    /// 生产端读源文件、算几何，碰不到已投影的那一份，也碰不到根回执——
    /// `db_generation_lock` 护的是投影侧那条单调链，拿在生产端只会把纯读串起来。
    /// 单根硬错是一条死信，不是整批的死因（ADR-041 §6）：必现失败的坏构件不许
    /// 把整页永久卡死。
    #[test]
    fn the_memory_producer_neither_locks_publication_nor_drops_a_page_for_one_root() {
        let source = service_source();
        let source = source.as_str();
        let producer = memory_producer_body(source);
        assert!(
            !producer.contains("db_generation_lock"),
            "the publication lock belongs to the consumer; generation is pure source reading"
        );
        assert!(
            producer.contains("Err(error) if !matches!(failure_policy,")
                && producer.contains("slice.failures.push("),
            "a root that cannot be generated must become a dead letter, not abort the batch"
        );
        let consumer = memory_consumer_body(source);
        for required in [
            "db_generation_lock",
            "ensure_not_older_than_persisted",
            "batch.failures",
        ] {
            assert!(
                consumer.contains(required),
                "the consumer projects under the dbnum lock, re-checks the pinned session and \
                 counts producer dead letters; missing {required}"
            );
        }
    }

    /// ADR-041 §8：生产与消费之间那条 channel 必须**有界**，深度 1。无界 channel
    /// 一样不阻塞，代价是内存无界——那正是 ADR 的否决方案。整页一次性产进内存
    /// （2720 根的 7997 就是 GB 级）与它是同一个形态。
    #[test]
    fn the_generation_pipeline_hands_over_bounded_slices() {
        let source = service_source();
        let source = source.as_str();
        let pipeline = source
            .split_once("pub async fn generate_and_persist_roots(")
            .expect("generation pipeline")
            .1
            .split_once("pub(crate) async fn republish_roots_by_rigid_motion(")
            .expect("generation pipeline end")
            .0;
        assert!(
            pipeline.contains("mpsc::channel::<GeneratedMemoryBatch>(1)"),
            "producer and consumer must meet over a depth-1 bounded channel"
        );
        assert!(
            pipeline.contains("futures::future::join(produce, consume)"),
            "the two halves must overlap; running them in sequence is the (P + C) × N shape"
        );
        let producer = memory_producer_body(source);
        assert!(
            producer.contains("crate::options::model_memory_slice_elements()")
                && producer.contains("crate::options::model_regen_execution_group()"),
            "a slice is sealed on the element budget as well as the cohort width: root count \
             alone is not a memory budget"
        );
        assert!(
            !production_half(source).contains("fn generate_dbnum_in_memory("),
            "producing a whole dbnum into one batch is the unbounded shape ADR-041 §8 rejected"
        );
    }

    /// 整库那一档只剩一条入口：[`E3dModelService::generation_roots_of`] 取根，再交给
    /// `generate_and_persist_roots`。并排那个 `generate_dbnum` 自己取根、逐根串行直落
    /// `generate_refs`，**不过几何并发闸**（闸利用率实测 0.0%）；生产从不走它，只有烟测在用，
    /// 于是烟测量出来的「生成耗时」一直是串行那条路的数。取根这一步必须保持纯枚举——它一旦
    /// 自己生成起来，那条串行路就原样回来了，而且照样没人发现。
    #[test]
    fn a_whole_dbnum_generates_only_through_the_parallel_producer() {
        let source = service_source();
        let source = source.as_str();
        assert!(
            !production_half(source).contains("fn generate_dbnum("),
            "the serial whole-dbnum entry is retired; a whole dbnum enters through \
             generation_roots_of + generate_and_persist_roots"
        );
        let enumerate = source
            .split_once("pub async fn generation_roots_of(")
            .expect("root enumeration")
            .1
            .split_once("\n    }\n")
            .expect("root enumeration end")
            .0;
        assert!(
            !enumerate.contains("generate_refs("),
            "enumerating roots stays a pure file-side read; generating here rebuilds the \
             serial entry that was retired"
        );
    }

    /// ADR-052 第 4 条：常驻 worker 从共享队列动态领取，宽度 `min(额度, 件数)`。
    /// 静态均分正是被取代的形态（几何负载长尾，一块里摆两个重 BRAN 就把其余 worker
    /// 空在那儿）；「每件一个 task 再去挤信号量」同样被禁；每个 worker 一份 catalogue
    /// 缓存，共享一把 Mutex 并在锁内求值就是 ADR-052 开篇那个 convoy。
    #[test]
    fn generation_workers_claim_dynamically_and_never_share_one_catalogue_cache() {
        let source = service_source();
        let source = source.as_str();
        let worker = source
            .split_once("fn spawn_generation_worker(")
            .expect("generation worker")
            .1
            .split_once("\n}\n")
            .expect("generation worker end")
            .0;
        assert!(
            worker.contains("concurrency::run_gated_cpu(")
                && worker.contains("while let Some((slot, root)) = queue.claim()")
                && worker.contains("queue.store(slot, outcome)"),
            "worker 是「一张许可 + 一个领取循环」，写回按领取序号落槽"
        );
        assert!(
            worker.contains("checkout_cache(&caches)") && worker.contains("return_cache(&caches"),
            "每个 worker 一份缓存，从跨波存活的池里借还"
        );
        let producer = memory_producer_body(source);
        assert!(
            producer.contains("concurrency::resident_workers(queue.claimable())"),
            "常驻 worker 数只能是 min(额度, 还能领的件数)"
        );
        for forbidden in ["chunk_size(", "tokio::spawn(", "roots.chunks("] {
            assert!(
                !producer.contains(forbidden),
                "静态均分 / 每件一个 task / 按批设栏都是被取代的形态，找到 {forbidden}"
            );
        }
    }

    /// 窗口是「无栏并行」与「内存有界」之间那根绳子：worker 最多跑在装配端前面
    /// 一个窗口，再多就得先把许可还回去（攥着许可等装配端正是 ADR-052 开篇那个
    /// 容量倒置）。
    #[test]
    fn the_generation_window_bounds_how_far_workers_may_run_ahead() {
        let roots = (0..5).map(|index| RefNo::new(7, index)).collect::<Vec<_>>();
        let queue = RootQueue::new(roots.clone(), 2);

        assert_eq!(queue.claimable(), 2);
        assert_eq!(queue.claim(), Some((0, roots[0])));
        assert_eq!(queue.claim(), Some((1, roots[1])));
        assert_eq!(queue.claim(), None, "窗口满了就不许再领");
        assert_eq!(queue.claimable(), 0);

        // 装配端还没拿到 0 号，窗口就滑不动——先算完的 1 号不能把它顶过去。
        queue.store(1, Ok(snapshot_with_elements(1)));
        assert!(queue.take_ready().is_none(), "队头没好，后面的不许插队");
        assert_eq!(queue.claim(), None);

        queue.store(0, Ok(snapshot_with_elements(1)));
        assert_eq!(queue.take_ready().map(|(root, _)| root), Some(roots[0]));
        assert_eq!(queue.claimable(), 1, "取走一个，窗口向前滑一格");
        assert_eq!(queue.claim(), Some((2, roots[2])));
    }

    /// 装配端只按源顺序取：切片边界与 `Required` 裁决因此与「谁先算完」无关，
    /// `geometry_workers = 1` 才能逐字节复现改前那条串行路径。
    #[test]
    fn finished_roots_are_assembled_in_source_order_not_completion_order() {
        let roots = (0..4).map(|index| RefNo::new(9, index)).collect::<Vec<_>>();
        let queue = RootQueue::new(roots.clone(), 4);
        for _ in 0..4 {
            queue.claim().expect("窗口容得下全部四根");
        }
        assert_eq!(queue.claim(), None, "领完就没有了");

        for slot in [3_usize, 1, 2] {
            queue.store(slot, Ok(snapshot_with_elements(slot)));
        }
        assert!(queue.take_ready().is_none());

        queue.store(0, Ok(snapshot_with_elements(0)));
        let assembled = std::iter::from_fn(|| queue.take_ready())
            .map(|(root, _)| root)
            .collect::<Vec<_>>();
        assert_eq!(assembled, roots, "完成序是 3/1/2/0，交出去的必须是 0/1/2/3");
        assert!(queue.take_ready().is_none());
        assert_eq!(queue.claimable(), 0);
    }

    /// 封片是「够了就交」，不是「攒满整页」：两条预算任一到线即封，只有死信的
    /// 尾片也要交出去（计数在消费端），空片不交。单根不拆——根是投影的原子单位。
    #[test]
    fn a_slice_seals_on_either_budget_and_never_splits_a_root() {
        let mut slice = MemorySlice::default();
        assert!(!slice.is_full(2, 100), "空片不算满");
        slice.failures.push((RefNo::new(1, 1), "boom".into()));
        assert!(
            !slice.is_full(2, 100),
            "死信不占元素预算，也不该单独把片顶满"
        );

        let mut by_roots = MemorySlice::default();
        by_roots.push(RefNo::new(1, 2), snapshot_with_elements(1));
        assert!(!by_roots.is_full(2, 100));
        by_roots.push(RefNo::new(1, 3), snapshot_with_elements(1));
        assert!(by_roots.is_full(2, 100), "到达 cohort 宽度即封片");
        assert_eq!(by_roots.elements, 2);

        let mut by_elements = MemorySlice::default();
        by_elements.push(RefNo::new(1, 4), snapshot_with_elements(150));
        assert!(
            by_elements.is_full(16, 100),
            "一根就撑破预算时立刻封片，而不是等够 16 根"
        );
        assert_eq!(
            by_elements.snapshots.len(),
            1,
            "根是投影的原子单位，超预算也不拆"
        );
    }

    fn snapshot_with_elements(count: usize) -> BakedSnapshot {
        BakedSnapshot {
            elements: (0..count)
                .map(|index| baked_element(RefNo::new(9, index as u32), &format!("mesh_{index}")))
                .collect(),
            report: Report::default(),
            visited: Vec::new(),
            room: None,
            point_sets: Vec::new(),
            lod1: Default::default(),
        }
    }

    /// 一页 100 根就是 100 份同一张 owner 表同时在内存里，而拼记录读的一直是批自己
    /// 带的那份索引。快照只借用扫描期的那一份，不再各自复制。
    #[test]
    fn every_snapshot_in_a_batch_shares_one_owner_index() {
        let source = service_source();
        let source = source.as_str();
        for declaration in [
            "pub(crate) owners: Arc<BTreeMap<(u32, u32), RefNo>>,",
            "pub owners: Arc<BTreeMap<(u32, u32), RefNo>>,",
        ] {
            assert!(
                source.contains(declaration),
                "the owner index is shared, not owned per snapshot; missing {declaration}"
            );
        }
        let body = source
            .split_once("fn generate_snapshot_from_set(")
            .expect("snapshot builder")
            .1
            .split_once("impl E3dPersistReport {")
            .expect("snapshot builder end")
            .0;
        assert!(
            body.contains("Arc::clone(&index.owners)") && !body.contains("index.owners.clone()"),
            "每个快照复制一份整库 owner 表，一页就是百 MB 级的纯重复"
        );
    }

    /// 单根快照路：根不在那一会话的元素索引里，要在求几何**之前**以类型化错误退出。
    /// 判在后面（或只靠消息文本）就是 2026-09-13 live 用例 3 那种「根不存在却报几何失败、
    /// 消息里还没原因」。
    #[test]
    fn snapshot_source_rejects_a_root_missing_from_the_session_index_before_generating() {
        let source = service_source();
        // Legacy 快照源与实时计算源都只经 `compute_snapshot_source` 求值（R6.15），门在那里。
        let entries = source
            .split_once("pub async fn generate_snapshot_source(")
            .expect("snapshot source")
            .1
            .split_once("pub(crate) fn source_file(")
            .expect("snapshot source end")
            .0;
        let (legacy, realtime) = entries
            .split_once("pub fn realtime_compute_source(")
            .expect("realtime compute source");
        for (name, entry) in [("legacy", legacy), ("realtime", realtime)] {
            assert!(
                entry.contains("compute_snapshot_source(&input, dbnum, root, source_sesno"),
                "{name} entry must evaluate through the shared compute_snapshot_source"
            );
            assert!(
                !entry.contains("generate_snapshot_from_set"),
                "{name} entry must not evaluate on its own"
            );
        }
        assert!(
            legacy.contains("db_generation_lock(dbnum).lock_owned().await")
                && !realtime.contains("db_generation_lock"),
            "only the legacy entry takes the generation lock"
        );
        let body = source
            .split_once("fn compute_snapshot_source(")
            .expect("shared snapshot evaluation")
            .1
            .split_once("/// One evaluated element")
            .expect("shared snapshot evaluation end")
            .0;
        let index_gate = body
            .find("input.index.owners.contains_key(&(root.word0, root.word1))")
            .expect("the session index gate");
        let typed_error = body
            .find("SnapshotSourceError::RootNotInSession")
            .expect("the typed not-in-session error");
        let generation = body
            .find("generate_snapshot_from_set_with_hooks(")
            .expect("snapshot generation");
        assert!(
            index_gate < generation && typed_error < generation,
            "the index gate and its typed error must come before geometry evaluation"
        );
    }

    fn generate_refs_body(source: &str) -> &str {
        source
            .split_once("async fn generate_refs(")
            .expect("generate_refs")
            .1
            .split_once("fn pin(&self")
            .expect("generate_refs end")
            .0
    }

    /// 存储状态一次读齐再进根循环。逐根现读会把「一次投影一批根」摊成 N 次往返，
    /// 也会让先算的那几根看不到后面才补上的依赖会话。
    #[test]
    fn projection_reads_the_store_once_before_the_root_loop() {
        let source = service_source();
        let source = source.as_str();
        let body = generate_refs_body(source);
        let (prefetch, root_loop) = body
            .split_once("for (root_index, &root) in roots.iter().enumerate()")
            .expect("root preparation loop");
        for read in [
            "store.receipt(",
            "store.root_projection_complete(",
            "store.geometry_ids_of_root(",
            "hydrate_published_dependencies(",
        ] {
            assert!(prefetch.contains(read), "missing batch prefetch for {read}");
            assert!(
                !root_loop.contains(read),
                "root loop must consume prefetched state instead of calling {read}"
            );
        }
    }

    /// ADR-080 D2：拿「缺席」推删除**只许有一处**，就是 [`whole_root_removals`]。
    ///
    /// 那条推导（`上一版有的 \ 这一版生成的`）只在 `upserts` 是这根全集时成立。单元级发布
    /// 只重建变了的那几件，谁要是顺手在别处又写一遍同样的做差，就会把没重建的全删掉——
    /// 投影与库两边都不报错。所以把它钉成独此一家：`prior` 只许喂给这个函数。
    #[test]
    fn only_whole_root_removals_derives_a_deletion_from_absence() {
        let source = service_source();
        let body = generate_refs_body(&source);
        assert!(
            body.contains("whole_root_removals(&prior[root_index], &snapshot.elements)"),
            "根循环里的删除必须经由 whole_root_removals"
        );
        // 上一版的 id 集合只有两个合法用处：喂给 whole_root_removals 做差，以及
        // 判空（`incomplete_root_policy` 只问「这根以前有没有东西」，不推删除）。多出第三处就要看一眼。
        assert_eq!(
            body.matches("prior[").count(),
            2,
            "上一版的 id 集合多了一个用处——是不是又有人在别处做差？"
        );
        assert!(
            body.contains("!prior[root_index].is_empty()"),
            "另一处应当只是判空"
        );
        // 本条测试自己也写着那个谓词，`include_str!` 会把它一起读进来——只数生产代码那一段。
        let production = production_half(&source);
        let helper = production
            .split_once("fn whole_root_removals(")
            .expect("whole_root_removals")
            .1;
        assert!(
            helper.contains("!generated_ids.contains(&key)"),
            "做差本体应当在 whole_root_removals 里"
        );
        assert_eq!(
            production.matches("!generated_ids.contains").count(),
            1,
            "生产代码里只许有一处「生成物里没有它 ⇒ 删」"
        );
    }

    /// 命中缓存要在求几何之前判掉——判在后面等于白算一遍。
    #[test]
    fn immutable_target_cache_gate_precedes_snapshot_generation() {
        let source = service_source();
        let body = generate_refs_body(&source);
        let cache_gate = body
            .find("store.root_projection_complete(")
            .expect("cache gate");
        let generation = body
            .find("generate_snapshot_from_set(")
            .expect("snapshot generation");
        assert!(
            cache_gate < generation,
            "the immutable-target hit must return before geometry evaluation"
        );
    }

    #[test]
    fn catalogue_resolver_prefers_the_target_design_projects_same_dbnum_file() {
        let global = PathBuf::from(r"D:\selected\AMS\ams7000_0001");
        let contextual = PathBuf::from(r"D:\selected\ACP\acp7000_0001");
        let locator = InMemoryCataLocator::from_parts(
            HashMap::from([(23984, 7600)]),
            HashMap::from([
                (7000, ("CATA".into(), "AMS".into(), global)),
                (
                    7015,
                    (
                        "DESI".into(),
                        "ACP".into(),
                        PathBuf::from(r"D:\selected\ACP\acp7015_0001"),
                    ),
                ),
            ]),
        );
        let resolver = E3dDbResolver {
            locator,
            project_files: Arc::new(HashMap::from([(
                project_db_key("ACP", 7000),
                ProjectDbFile {
                    db_type: "CATA".into(),
                    project: "ACP".into(),
                    path: contextual.clone(),
                },
            )])),
            target_dbnum: 7015,
            preferred_project: Some("ACP".into()),
            template_dir: PathBuf::from(r"D:\templates"),
            sessions: Arc::new(std::sync::Mutex::new(BTreeMap::from([((7015, 7000), 42)]))),
        };
        let pin = resolver.resolve(7000).expect("contextual CATA pin");
        assert_eq!(pin.file, contextual);
        assert_eq!(pin.sesno, Some(42));
        assert_eq!(pin.db_type.as_deref(), Some("CATA"));
        assert!(pin.template.ends_with("catvir.dat"));
    }

    /// GitHub #47：目录版本核对问「这个设计库读哪一份目录文件」，答案必须与 resolver 逐字相同——
    /// 所在项目有同类型副本读副本，没有读 MDB 声明那份；MDB 没声明的库两边都答不出（#46）。
    #[test]
    fn catalogue_file_choice_answers_like_the_resolver_per_design_library() {
        let declared = PathBuf::from(r"D:\selected\AMS\ams7000_0001");
        let copy = PathBuf::from(r"D:\selected\ACP\acp7000_0001");
        let locator = InMemoryCataLocator::from_parts(
            HashMap::new(),
            HashMap::from([
                (7000, ("CATA".into(), "AMS".into(), declared.clone())),
                (
                    7015,
                    (
                        "DESI".into(),
                        "ACP".into(),
                        PathBuf::from(r"D:\selected\ACP\acp7015_0001"),
                    ),
                ),
                (
                    8000,
                    (
                        "DESI".into(),
                        "AMS".into(),
                        PathBuf::from(r"D:\selected\AMS\ams8000_0001"),
                    ),
                ),
            ]),
        );
        let project_files: ProjectDbFiles = Arc::new(HashMap::from([(
            project_db_key("ACP", 7000),
            ProjectDbFile {
                db_type: "CATA".into(),
                project: "ACP".into(),
                path: copy.clone(),
            },
        )]));
        let resolver_file = select_dependency_file(&locator, &project_files, Some("ACP"), 7000)
            .expect("resolver answers")
            .path;
        let choice = CatalogueFileChoice::from_parts(locator, project_files);
        assert_eq!(choice.for_library(7015, 7000), Some(copy.clone()));
        assert_eq!(choice.for_library(7015, 7000), Some(resolver_file));
        assert_eq!(choice.for_library(8000, 7000), Some(declared.clone()));
        assert_eq!(choice.declared(7000), Some(declared));
        assert_eq!(choice.for_library(7015, 7016), None, "MDB 没声明的目录库");
        assert_eq!(choice.declared(7016), None);
    }

    /// GitHub #32：P4 翻默认开之后每一窗规划都要列 MDB 的设计成员。一个成员问不出会话（文件不在 /
    /// 被占）不能让这一窗的规划报错——交 `None`，反查按文件最新自己开；开不了记进 `unreadable_dbs`、
    /// 整窗退化并点名（计划 §2 原则 3），而不是让所有设计库的模型更新一起停。
    #[test]
    fn an_unreadable_design_member_degrades_the_reference_cascade_instead_of_failing_the_window() {
        use crate::data_interface::reverse_reference::{
            DesignReferenceCascade, SourceDb, read_design_referrer_edges,
        };
        let pin = |dbnum: i32, db_type: &str, sesno: Option<u32>| DbPin {
            dbnum,
            db_type: db_type.into(),
            file: PathBuf::from(format!(r"Z:\gm32\{dbnum}_0001")),
            sesno,
        };
        let pins = vec![
            pin(8000, "DESI", None),
            pin(8001, "DESI", Some(7)),
            pin(7351, "CATA", None),
            pin(8002, "DESI", None),
        ];
        let mut asked = Vec::new();
        let members = design_members(&pins, |pin| {
            asked.push(pin.dbnum);
            match (pin.sesno, pin.dbnum) {
                (Some(sesno), _) => Ok(sesno),
                (None, 8000) => Ok(42),
                (None, dbnum) => anyhow::bail!("dbnum {dbnum} 的库文件被占用"),
            }
        });
        assert_eq!(asked, vec![8000, 8001, 8002], "只问设计成员");
        assert_eq!(
            members,
            vec![
                (8000, pins[0].file.clone(), Some(42)),
                (8001, pins[1].file.clone(), Some(7)),
                (8002, pins[3].file.clone(), None),
            ]
        );

        // 交出去的 `None` 走反查自己的开库：开不了 → 记账 → 判不了（不是「没有引用者」）。
        let design_dbs: Vec<SourceDb> = members
            .into_iter()
            .filter(|(dbnum, ..)| *dbnum == 8002)
            .map(|(dbnum, file, sesno)| SourceDb {
                dbnum,
                db_type: "DESI".into(),
                file,
                sesno,
            })
            .collect();
        let edges =
            read_design_referrer_edges(&[e3d_io::RefNo::new(1, 1)], &design_dbs, usize::MAX);
        assert!(
            edges.unreadable_dbs.contains_key(&8002),
            "{:?}",
            edges.unreadable_dbs
        );
        let cascade = DesignReferenceCascade::from_edges(&edges, 8000, |_| Vec::new());
        let reason = cascade.degraded.expect("打不开就是判不了");
        assert!(reason.contains("8002"), "{reason}");
    }

    /// 淘汰规则住在 `WindowPlanCache` 里，单测用本地实例：全局缓存会被并行测试里的真实规划器
    /// （合成夹具 dbnum=8000）同时写入，先清空再断言全局表必然互相踩——红的不是淘汰逻辑。
    #[test]
    fn window_plan_cache_replaces_same_file_window_and_bounds_total_entries() {
        let sets = |dbnum: u32, file: &str, base: u32, target: u32| {
            Arc::new(WindowPlanSets {
                key: WindowPlanKey::new(dbnum, Path::new(file), base, target),
                regenerate: BTreeSet::new(),
                remove: BTreeSet::new(),
                regenerate_derived: BTreeSet::new(),
                derived_stale: BTreeSet::new(),
                full_rebuild: None,
                unreadable: 0,
                dispositions: Vec::new(),
                inputs: None,
            })
        };
        let file_a = "project-a/ams8000_0001";
        let file_b = "project-b/ams8000_0001";
        let key = |file: &str, base: u32, target: u32| {
            WindowPlanKey::new(8000, Path::new(file), base, target)
        };
        let mut cache = WindowPlanCache::new();

        assert!(cache.insert(sets(8000, file_a, 10, 11)).is_empty());
        let replaced = cache.insert(sets(8000, file_a, 11, 12));
        assert_eq!(
            replaced.len(),
            1,
            "同库同文件的新窗必须替换旧窗，且旧载荷交还给调用方在锁外释放"
        );
        assert!(
            cache.get(&key(file_a, 10, 11)).is_none(),
            "旧窗不许留在缓存里，防止旧 DbSet/SourceIndex 句柄滞留"
        );
        assert!(cache.get(&key(file_a, 11, 12)).is_some());

        // 同号跨项目副本的同一窗必须并存：文件身份进键，谁也不能把对方顶掉。
        assert!(cache.insert(sets(8000, file_b, 11, 12)).is_empty());
        assert!(cache.get(&key(file_a, 11, 12)).is_some());
        assert!(cache.get(&key(file_b, 11, 12)).is_some());

        for idx in 0..(WindowPlanCache::CAPACITY + 4) {
            let dbnum = 9000 + idx as u32;
            cache.insert(sets(
                dbnum,
                &format!("project-{dbnum}/ams{dbnum}_0001"),
                idx as u32,
                (idx + 1) as u32,
            ));
        }
        assert!(
            cache.len() <= WindowPlanCache::CAPACITY,
            "窗口计划缓存总条目数必须受 WindowPlanCache::CAPACITY={} 约束，实际 {}",
            WindowPlanCache::CAPACITY,
            cache.len()
        );
        // 同键重插只替换，不占第二个格子，并把旧载荷交还给调用方。
        cache.insert(sets(8100, file_a, 50, 51));
        let len_before = cache.len();
        assert_eq!(
            cache.insert(sets(8100, file_a, 50, 51)).len(),
            1,
            "同键重插要替换旧载荷"
        );
        assert_eq!(cache.len(), len_before);
    }

    /// 容量淘汰按「最久没用」：会话号是各文件自己的计数，一个会话号很小、却刚被执行体命中的窗口
    /// 不许因为号小被别的库的新窗挤掉——那会让执行体落空、把计划重算一遍。
    #[test]
    fn window_plan_cache_evicts_the_least_recently_used_window_not_the_smallest_session() {
        let key = |dbnum: u32, base: u32, target: u32| {
            WindowPlanKey::new(
                dbnum,
                Path::new(&format!("project/ams{dbnum}_0001")),
                base,
                target,
            )
        };
        let sets = |dbnum: u32, base: u32, target: u32| {
            Arc::new(WindowPlanSets {
                key: key(dbnum, base, target),
                regenerate: BTreeSet::new(),
                remove: BTreeSet::new(),
                regenerate_derived: BTreeSet::new(),
                derived_stale: BTreeSet::new(),
                full_rebuild: None,
                unreadable: 0,
                dispositions: Vec::new(),
                inputs: None,
            })
        };
        let mut cache = WindowPlanCache::new();
        assert!(cache.insert(sets(7009, 3, 4)).is_empty());
        for dbnum in 8000..8000 + WindowPlanCache::CAPACITY as u32 - 1 {
            assert!(cache.insert(sets(dbnum, 600, 601)).is_empty());
        }
        assert_eq!(cache.len(), WindowPlanCache::CAPACITY);
        assert!(cache.get(&key(7009, 3, 4)).is_some());

        assert_eq!(cache.insert(sets(9000, 700, 701)).len(), 1);
        assert!(
            cache.get(&key(7009, 3, 4)).is_some(),
            "a just-used window stays even though its session numbers are the smallest"
        );
        assert!(
            cache.get(&key(8000, 600, 601)).is_none(),
            "the least recently used window goes first"
        );
        assert_eq!(cache.len(), WindowPlanCache::CAPACITY);
    }
}
