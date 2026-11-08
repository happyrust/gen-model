use crate::fast_model::utils::save_transforms_to_surreal;
use aios_core::{RefU64, RefnoEnum, SUL_DB, gen_bytes_hash, query_neareast_along_axis};
use bevy_transform::components::Transform;
use glam::Vec3;
use parry3d::bounding_volume::Aabb;
use parry3d::bounding_volume::BoundingVolume;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 全量重算设备派生量：`cal_equi` 的 world transform + `nearest_relate` 最近楼板。
///
/// 先清软删设备的派生行再重算——软删的 EQUI 断开了 `pe_owner`，枚举（按
/// `pe.deleted != true`）自然跳过它们，但**不会**顺手删掉它们留下的行。只靠枚举
/// 跳过的话，软删那一刻的 `cal_equi` 行与 `nearest_relate` 边会永久留在库里。
pub async fn update_cal_equip() -> anyhow::Result<()> {
    clear_soft_deleted_equip_rows().await?;
    update_cal_equip_wtrans().await?;
    cal_equip_nearest_floor().await?;
    Ok(())
}

/// [`update_cal_equip_for_roots`] 的结果，给调用方记日志用。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EquipRefreshOutcome {
    /// 根下重写了 `cal_equi` 的设备台数。
    pub refreshed: usize,
    /// 因楼板变化被牵连、只重算楼板边的设备台数（`cal_equi` 不动：设备本身没变）。
    pub relinked: usize,
    /// 重算后挂着 `nearest_relate` 的设备台数（含牵连的；树已就绪却打不中楼板的，旧边删掉、不计）。
    pub floors_linked: usize,
    /// 重算了楼板边的阀门个数（含因楼板变化被牵连的）。
    pub valves_refreshed: usize,
    /// 重算后挂着 `nearest_relate` 的阀门个数。
    pub valves_linked: usize,
    /// 因为所挂楼板在这批根里（或已软删）而被拉进来重算的设备 + 阀门数。
    pub via_floors: usize,
    /// 不挂这批楼板、但落在楼板新位置正上方，由空间反查拉进来的设备 + 阀门数。
    pub via_floor_space: usize,
    /// 重算失败的设备 + 阀门数（已逐个 `warn!`，旧值原样保留）。
    pub failed: usize,
    /// 空间树没就绪：`nearest_relate` 一条没动（阀门与楼板牵连也一并跳过）。
    pub floors_skipped: bool,
    /// 到期没来得及算的设备 + 阀门，加上柱子空间反查没跑到的楼板块数；旧值原样保留。
    pub deferred: usize,
    /// 整库口径：楼板柱子空间反查没做（见 [`EQUIP_DERIVED_LIBRARY_SCALE_ROOTS`]）。
    pub floor_space_skipped: bool,
}

/// 任务内一轮设备派生量重算的期限（GitHub #88）。派生量是可重建的缓存：到点就停，没算到的设备 /
/// 阀门 / 楼板旧值保留、记进 [`EquipRefreshOutcome::deferred`]，由手动 [`update_cal_equip`] 或下一个碰到
/// 它们的任务回补。不设期限时，一次整库 ModelOnly（7997，6772 根）在这一段跑了 20 分钟以上，把水位和后面
/// 排队的任务一起挂住。
pub(crate) const EQUIP_DERIVED_BUDGET: Duration = Duration::from_secs(5 * 60);

/// 根数到这个量就按整库口径算：不做楼板柱子空间反查（真库 p90 一块楼板 12～25 秒，整库几百块就是小时级）。
/// 整库口径下本库的设备 / 阀门都在根下，逐台重算本来就会重连楼板边；漏掉的只是**别的库**里站在本库楼板
/// 上的设备，留给它们自己的任务或手动重算。
pub(crate) const EQUIP_DERIVED_LIBRARY_SCALE_ROOTS: usize = 2_000;

/// 一条展开 / 反查 SQL 点名的 key 上限：整库 ModelOnly 的前沿是全库节点，拼成一条就是 MB 级语句。
const NODE_QUERY_CHUNK: usize = 1_000;

/// [`update_cal_equip_for_roots_with`] 的口径。缺省（[`update_cal_equip_for_roots`]）不设期限、逐根展开。
#[derive(Debug, Default, Clone, Copy)]
pub struct EquipRefreshScope {
    /// 到点即停；`None` 不设期限。
    pub deadline: Option<Instant>,
    /// 整库口径（见 [`EQUIP_DERIVED_LIBRARY_SCALE_ROOTS`]）。
    pub library_scale: bool,
}

/// `deadline` 已过。期限只在两个目标之间检查：已开始的那一台照常算完，不在写库中途撂下。
pub(crate) fn budget_exhausted(deadline: Option<Instant>, now: Instant) -> bool {
    deadline.is_some_and(|deadline| now >= deadline)
}

/// 整个任务都在重算这个库（整库 ModelOnly）或根数到了门槛，就按整库口径算。
pub(crate) fn library_scale_refresh(whole_library: bool, roots: usize) -> bool {
    whole_library || roots >= EQUIP_DERIVED_LIBRARY_SCALE_ROOTS
}

/// 任务内一轮派生量重算的回执（`/health.equip_derived.last`）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct EquipDerivedRun {
    pub dbnum: u32,
    pub task_id: String,
    pub roots: usize,
    pub library_scale: bool,
    pub elapsed_ms: u64,
    pub budget_secs: u64,
    pub deferred: usize,
    pub failed: usize,
    pub error: Option<String>,
    pub finished_at: String,
}

static EQUIP_DERIVED_LAST: Mutex<Option<EquipDerivedRun>> = Mutex::new(None);
static EQUIP_DERIVED_DEFERRED_TOTAL: AtomicU64 = AtomicU64::new(0);

/// 记下一轮任务内重算（进程内，不落库）。
pub fn note_equip_derived_run(run: EquipDerivedRun) {
    EQUIP_DERIVED_DEFERRED_TOTAL.fetch_add(run.deferred as u64, Ordering::Relaxed);
    *EQUIP_DERIVED_LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(run);
}

/// `/health.equip_derived`：`null` = 本进程还没在任务里重算过；`deferred_total` 非零 = 有设备 / 阀门的
/// 派生量因为到期留着旧值，要靠手动 `update_cal_equip` 或后续任务回补。
pub fn equip_derived_snapshot() -> serde_json::Value {
    let last = EQUIP_DERIVED_LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match last {
        None => serde_json::Value::Null,
        Some(last) => serde_json::json!({
            "last": last,
            "deferred_total": EQUIP_DERIVED_DEFERRED_TOTAL.load(Ordering::Relaxed),
        }),
    }
}

/// 清掉软删设备的两类派生行（`cal_equi` 与 `nearest_relate`）。
///
/// 软删的 EQUI 会断开 `pe_owner`（实测父边为空），从父根往下找不到它们，所以派生行
/// 只能靠这条显式清理销掉。两张表各千行量级，实测各扫一遍 40～60ms。
///
/// 全量与增量**都**要调：只在一侧清，另一侧跑完就会把软删设备的派生行重新留在库里。
/// 真库 2026-11-02 实测——软删 `pe:24381_101725` 后跑全量 `update_cal_equip`，
/// `cal_equi` 行与 `nearest_relate` 边都原样留着（`{cal: true, edges: 1}`），
/// 只有增量路径会清。
pub(crate) async fn clear_soft_deleted_equip_rows() -> anyhow::Result<()> {
    SUL_DB
        .query(
            "DELETE nearest_relate WHERE in.deleted = true; \
             DELETE cal_equi WHERE type::thing('pe', record::id(id)).deleted = true;",
        )
        .await?
        .check()?;
    Ok(())
}

/// 根往下找设备的最大层数。真库 EQUI 最深在 SITE → ZONE → EQUI 这一级，再深是零件；
/// 上限防异常数据把展开拖成全树扫。
const EQUIP_EXPAND_DEPTH: usize = 4;

/// 增量版：只重算**指定生成根**影响到的设备派生量。
///
/// ## 为什么需要它
///
/// `update_cal_equip` 是全量版，只挂在启动序列上（`lib.rs` 的 `cal-equip` 段），而且
/// `nearest_relate` 那一半只给**还没有边**的设备补。增量更新路径此前完全不碰这两张表——
/// 模型改了以后 `cal_equi` 的 world transform 和 `nearest_relate` 的最近楼板都还是旧的。
///
/// ## 根 → 设备
///
/// 设备子件的生成根归到所在的 EQUI（`generation_root.rs` U-E2：EQUI 自成交付单元），所以根
/// 要么就是 EQUI，要么是 ZONE / SITE 这类容器。从根沿 `<-pe_owner`（`in` = 子、`out` = 父）
/// 逐层往下，`noun = 'EQUI'` 的收下且不再往里走（它的子件是零件），其余未删节点作下一层。
///
/// ## 写法（真库 python 探针逐条验过，SurrealDB 2.1.4）
///
/// - 一层一条查询：`SELECT id, noun, deleted FROM array::flatten((SELECT VALUE <-pe_owner.in FROM [..]))`。
/// - `cal_equi` 用 `UPSERT`：`CREATE` 撞上已有 id 报 `Database record already exists`、旧值原样
///   留着，而 `query()` 不检查语句级错误——不 `.check()` 就是静默不更新。
/// - 旧边用图写法 `DELETE pe:x->nearest_relate` 删，和新边放在同一个事务里换。
///
/// ## 阀门
///
/// 根下的 VALV（PIPE → BRAN → VALV）与设备同一趟收集，只重算楼板边：射线起点、方向与启动全量
/// [`aios_core::spatial::pipe::cal_valve_nearest_floor`] 同口径（阀门原点向 -Z）。
///
/// ## 楼板变化
///
/// 根下的 FLOOR 自己不算，受它影响的设备 / 阀门分两路拉进来，**只重算楼板边**（设备本身没动，
/// `cal_equi` 不写）：
///
/// - **现有边反查**：此刻挂在这些楼板上的（楼板离开了原位置、改厚了，旧距离甚至旧归属都可能不再
///   成立）。软删的楼板从根下找不到（`pe_owner` 已断），同一条查询顺带把挂在软删楼板上的也拉进来。
///   一条走 `out` 的查询，真库 2 块楼板 20 个牵连约 150ms。
/// - **空间反查**：楼板**新位置**正上方的（原来挂在下面别的楼板上、或原来没有边的）。xy 取楼板在
///   树上的盒、z 从楼板底面到无穷高，取与这根柱子相交的树条目；VALV 直接收，其余按
///   `owner` / `owner.owner` 归到 EQUI（每批 2000 个）。树上只有子件盒，所以合并盒一角伸到楼板上方、
///   但没有一个子件落进柱子的设备会漏掉。代价随柱子里的条目数走：真库 p90 楼板约 200 个牵连、
///   12～25 秒，最大的一块约 1,550 个、1.5～3 分钟。
///
/// 两路都读的是楼板的**新**几何：本函数在 `await_units_durable` 之后跑，那时
/// `converge_tree_after_persist` 已经把树换成新盒、清掉了这些 refno 的网格缓存。
///
/// ## 增量覆盖面
///
/// | 变化 | 怎么接住 |
/// |---|---|
/// | 设备 / 子件变 | 生成根即 EQUI，整台重算 |
/// | 阀门变 | PIPE / BRAN 根向下展开到 VALV，重连 |
/// | 楼板离开原位 | 现有边反查 |
/// | 楼板到了新位 | 空间反查 |
/// | 楼板软删 | 现有边反查（挂在软删楼板上的） |
/// | 楼板几何清空、pe 还在 | 树条目已摘，现有边反查 |
/// | 非 DESI / 初始化编排任务 | 不等刷盘就不算（库里的模型行未必是这一版） |
///
/// ## 空间树没就绪时不碰楼板边
///
/// 射线查询在树空 / 未装载时一律返回 `None`，与「脚下真的没有楼板」分不开。这时删旧边就是把
/// 正确数据抹掉（2026-11-01 实测：没装树跑了一遍，4 台设备的边被删光）。所以树不是
/// `Ready` / `ReadyEmpty` 时只重写 `cal_equi`，`nearest_relate` 原样保留并在结果里标出。
///
/// ## 已删设备
///
/// 软删的 EQUI 会断开 `pe_owner`（实测父边为空），从父根往下找不到它们；所以每次顺手把
/// `deleted = true` 的设备的两类派生行清掉（见 [`clear_soft_deleted_equip_rows`]）。
///
/// ## 期限与整库口径（GitHub #88）
///
/// 任务里调的是 [`update_cal_equip_for_roots_with`]：先算根下的设备、再算根下的阀门、再算现有边牵连的，
/// 最贵的楼板柱子空间反查放最后，期限一到剩下的记进 `deferred`、旧值保留。整库口径不做柱子空间反查。
pub async fn update_cal_equip_for_roots(
    roots: &[RefnoEnum],
) -> anyhow::Result<EquipRefreshOutcome> {
    update_cal_equip_for_roots_with(roots, EquipRefreshScope::default(), |_, _| {}).await
}

/// [`update_cal_equip_for_roots`] 带期限、口径与进度回调的版本。每算完一个设备 / 阀门调一次
/// `on_progress(已算, 已知总数)`；牵连进来的目标会把总数往上加。
pub async fn update_cal_equip_for_roots_with(
    roots: &[RefnoEnum],
    scope: EquipRefreshScope,
    mut on_progress: impl FnMut(usize, usize) + Send,
) -> anyhow::Result<EquipRefreshOutcome> {
    let mut outcome = EquipRefreshOutcome::default();
    if roots.is_empty() {
        return Ok(outcome);
    }

    clear_soft_deleted_equip_rows().await?;

    let mut scan = collect_live_targets(roots).await?;
    let link_floors = crate::fast_model::spatial_state::current_state().is_ready();
    outcome.floors_skipped =
        !link_floors && !(scan.live.is_empty() && scan.valves.is_empty() && scan.floors.is_empty());
    println!(
        "[设备派生量] 开始 根={} 设备={} 阀门={} 楼板={} 口径={}{}",
        roots.len(),
        scan.live.len(),
        scan.valves.len(),
        scan.floors.len(),
        if scope.library_scale {
            "整库"
        } else {
            "逐根"
        },
        if link_floors {
            ""
        } else {
            "（空间树未就绪，只写 cal_equi）"
        }
    );
    let mut progress = Progress {
        done: 0,
        total: scan.live.len() + if link_floors { scan.valves.len() } else { 0 },
    };
    on_progress(0, progress.total);

    let live = std::mem::take(&mut scan.live);
    for (index, equip) in live.iter().enumerate() {
        if budget_exhausted(scope.deadline, Instant::now()) {
            outcome.deferred += live.len() - index;
            break;
        }
        match refresh_one_equip(*equip, link_floors).await {
            Ok(linked) => {
                outcome.refreshed += 1;
                outcome.floors_linked += usize::from(linked);
            }
            Err(error) => {
                tracing::warn!(equip = ?equip, error = %error, "设备派生量重算失败，旧值保留");
                outcome.failed += 1;
            }
        }
        progress.done += 1;
        on_progress(progress.done, progress.total);
    }
    if !link_floors {
        return Ok(outcome);
    }
    drain_floor_targets(
        &mut scan,
        scope,
        &mut outcome,
        &mut progress,
        &mut on_progress,
    )
    .await;

    // 牵连名单拿不全只记一笔失败，不拖住其余的重算。
    if budget_exhausted(scope.deadline, Instant::now()) {
        outcome.deferred += scan.floors.len();
        return Ok(outcome);
    }
    match floor_dependents(&scan.floors).await {
        Ok(nodes) => {
            outcome.via_floors = scan.absorb_floor_dependents(nodes);
            progress.total += outcome.via_floors;
        }
        Err(error) => {
            tracing::warn!(error = %error, "楼板现有边反查失败，挂在这些楼板上的设备 / 阀门本轮不重算");
            outcome.failed += 1;
        }
    }
    drain_floor_targets(
        &mut scan,
        scope,
        &mut outcome,
        &mut progress,
        &mut on_progress,
    )
    .await;

    if scope.library_scale {
        outcome.floor_space_skipped = !scan.floors.is_empty();
        return Ok(outcome);
    }
    let floors = std::mem::take(&mut scan.floors);
    for (index, floor) in floors.iter().enumerate() {
        if budget_exhausted(scope.deadline, Instant::now()) {
            outcome.deferred += floors.len() - index;
            break;
        }
        let entries = floor_column_entries(std::slice::from_ref(floor)).await;
        let complete = match column_dependents(&entries, scope.deadline).await {
            Ok((nodes, complete)) => {
                let added = scan.absorb_floor_dependents(nodes);
                outcome.via_floor_space += added;
                progress.total += added;
                complete
            }
            Err(error) => {
                tracing::warn!(floor = ?floor, error = %error, "楼板空间反查失败，楼板新位置上方的设备 / 阀门本轮不重算");
                outcome.failed += 1;
                true
            }
        };
        drain_floor_targets(
            &mut scan,
            scope,
            &mut outcome,
            &mut progress,
            &mut on_progress,
        )
        .await;
        if !complete {
            outcome.deferred += floors.len() - index;
            break;
        }
    }
    Ok(outcome)
}

/// 已算 / 已知总数，喂给 [`update_cal_equip_for_roots_with`] 的进度回调。
struct Progress {
    done: usize,
    total: usize,
}

/// 算掉 `scan` 里排着的牵连设备（只重连楼板边）与阀门；期限一到，剩下的记进 `deferred`、留在原地不算。
async fn drain_floor_targets(
    scan: &mut EquipScan,
    scope: EquipRefreshScope,
    outcome: &mut EquipRefreshOutcome,
    progress: &mut Progress,
    on_progress: &mut (impl FnMut(usize, usize) + Send),
) {
    let equips = std::mem::take(&mut scan.floor_equips);
    for (index, equip) in equips.iter().enumerate() {
        if budget_exhausted(scope.deadline, Instant::now()) {
            outcome.deferred += equips.len() - index;
            break;
        }
        match relink_floor(*equip).await {
            Ok(linked) => {
                outcome.relinked += 1;
                outcome.floors_linked += usize::from(linked);
            }
            Err(error) => {
                tracing::warn!(equip = ?equip, error = %error, "设备楼板边重算失败，旧边保留");
                outcome.failed += 1;
            }
        }
        progress.done += 1;
        on_progress(progress.done, progress.total);
    }
    let valves = std::mem::take(&mut scan.valves);
    for (index, valve) in valves.iter().enumerate() {
        if budget_exhausted(scope.deadline, Instant::now()) {
            outcome.deferred += valves.len() - index;
            break;
        }
        match refresh_one_valve(*valve).await {
            Ok(linked) => {
                outcome.valves_refreshed += 1;
                outcome.valves_linked += usize::from(linked);
            }
            Err(error) => {
                tracing::warn!(valve = ?valve, error = %error, "阀门楼板边重算失败，旧边保留");
                outcome.failed += 1;
            }
        }
        progress.done += 1;
        on_progress(progress.done, progress.total);
    }
}

/// 挂在这些楼板上、或挂在已软删楼板上的 `nearest_relate` 起点（设备 / 阀门）。楼板按
/// [`NODE_QUERY_CHUNK`] 分条点名，软删那一句只问一次。
async fn floor_dependents(floors: &[RefnoEnum]) -> anyhow::Result<Vec<Node>> {
    let mut nodes = fetch_nodes(soft_deleted_floor_dependents_sql().to_owned()).await?;
    for chunk in floors.chunks(NODE_QUERY_CHUNK) {
        nodes.extend(fetch_nodes(floor_dependents_sql(chunk)).await?);
    }
    Ok(nodes)
}

fn soft_deleted_floor_dependents_sql() -> &'static str {
    "SELECT in AS id, in.noun AS noun, in.deleted AS deleted FROM nearest_relate \
     WHERE type::thing('pe', record::id(out)).deleted = true;"
}

fn floor_dependents_sql(floors: &[RefnoEnum]) -> String {
    let floors = floors
        .iter()
        .map(|floor| floor.to_table_key("FLOOR"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "SELECT in AS id, in.noun AS noun, in.deleted AS deleted FROM nearest_relate \
         WHERE out IN [{floors}];"
    )
}

/// 楼板正上方那根柱子：xy 取楼板盒，z 从楼板底面到无穷高。楼板不在树上（盒无效）时为 `None`。
fn floor_column(floor: &Aabb) -> Option<Aabb> {
    let valid = floor.mins.x <= floor.maxs.x
        && floor.mins.y <= floor.maxs.y
        && floor.mins.z <= floor.maxs.z;
    valid.then(|| {
        Aabb::new(
            parry3d::math::Point::new(floor.mins.x, floor.mins.y, floor.mins.z),
            parry3d::math::Point::new(floor.maxs.x, floor.maxs.y, f32::MAX),
        )
    })
}

/// 这些楼板新位置正上方的树条目（楼板自身除外），按 refno 去重。读锁只在这里持有，不跨 await。
async fn floor_column_entries(floors: &[RefnoEnum]) -> Vec<(RefnoEnum, String)> {
    let tree = aios_core::room::room::GLOBAL_AABB_TREE.read().await;
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for floor in floors {
        let mut merged = Aabb::new_invalid();
        for entry in tree.entries_of(floor.refno()).iter() {
            merged.merge(&entry.aabb);
        }
        let Some(column) = floor_column(&merged) else {
            continue;
        };
        for entry in tree.locate_intersecting_bounds(&column) {
            if entry.noun == "FLOOR" || !seen.insert(entry.refno) {
                continue;
            }
            entries.push((RefnoEnum::from(entry.refno), entry.noun.clone()));
        }
    }
    entries
}

/// 归属查询一批的条目数：最大的楼板柱子里有约 11 万条树条目，一条 SQL 点名 2000 个约 30KB。
const COLUMN_LOOKUP_BATCH: usize = 2000;

#[derive(Debug, serde::Deserialize)]
struct ColumnRow {
    id: surrealdb::sql::Thing,
    noun: Option<String>,
    deleted: Option<bool>,
    o1: Option<surrealdb::sql::Thing>,
    n1: Option<String>,
    d1: Option<bool>,
    o2: Option<surrealdb::sql::Thing>,
    n2: Option<String>,
    d2: Option<bool>,
}

type Node = (RefnoEnum, Option<String>, bool);

/// 柱子里一个条目的自身、父、祖父（`None` = 没有这一级）。
struct ColumnLink {
    node: Node,
    parent: Option<Node>,
    grand: Option<Node>,
}

/// 条目归到哪个设备 / 阀门：VALV 是它自己；其余看父、祖父里第一个 EQUI（设备子件最深两层，
/// SUBE 下的零件是祖父那一级）。都不是就与设备 / 阀门无关。
fn floor_dependent_of(link: ColumnLink) -> Option<Node> {
    let is = |node: &Node, noun: &str| node.1.as_deref() == Some(noun);
    if is(&link.node, "VALV") || is(&link.node, "EQUI") {
        return Some(link.node);
    }
    [link.parent, link.grand]
        .into_iter()
        .flatten()
        .find(|node| is(node, "EQUI"))
}

/// 柱子里的条目 → 设备 / 阀门节点（去重交给 [`EquipScan::absorb_floor_dependents`]）。大楼板的柱子有
/// 十几万条目、要查几十批，期限在批与批之间检查；第二格为 `false` = 到期没查完，已查到的照样返回。
async fn column_dependents(
    entries: &[(RefnoEnum, String)],
    deadline: Option<Instant>,
) -> anyhow::Result<(Vec<Node>, bool)> {
    let to_node =
        |id: Option<surrealdb::sql::Thing>, noun: Option<String>, deleted: Option<bool>| {
            let refno = crate::data_interface::helper::pe_thing_to_refno(id?).ok()?;
            Some((refno, noun, deleted.unwrap_or(false)))
        };
    let mut nodes = Vec::new();
    for batch in entries.chunks(COLUMN_LOOKUP_BATCH) {
        if budget_exhausted(deadline, Instant::now()) {
            return Ok((nodes, false));
        }
        let keys = batch
            .iter()
            .map(|(refno, _)| refno.to_pe_key())
            .collect::<Vec<_>>()
            .join(",");
        let mut response = SUL_DB
            .query(format!(
                "SELECT id, noun, deleted, owner AS o1, owner.noun AS n1, owner.deleted AS d1, \
                 owner.owner AS o2, owner.owner.noun AS n2, owner.owner.deleted AS d2 FROM [{keys}];"
            ))
            .await?
            .check()?;
        let rows: Vec<ColumnRow> = response.take(0)?;
        nodes.extend(rows.into_iter().filter_map(|row| {
            floor_dependent_of(ColumnLink {
                node: to_node(Some(row.id), row.noun, row.deleted)?,
                parent: to_node(row.o1, row.n1, row.d1),
                grand: to_node(row.o2, row.n2, row.d2),
            })
        }));
    }
    Ok((nodes, true))
}

#[derive(Debug, serde::Deserialize)]
struct PeNode {
    id: surrealdb::sql::Thing,
    noun: Option<String>,
    deleted: Option<bool>,
}

/// 「根 → 设备 / 阀门 / 楼板」逐层分拣的状态，抽成纯逻辑好离线钉住。
#[derive(Default)]
struct EquipScan {
    live: Vec<RefnoEnum>,
    /// 因楼板变化牵连进来的设备：只重连楼板边，不写 `cal_equi`。
    floor_equips: Vec<RefnoEnum>,
    valves: Vec<RefnoEnum>,
    floors: Vec<RefnoEnum>,
    // 根不能预先放进 `seen`：先放的话「根本身就是 EQUI」再 insert 返回 false，
    // 它自己就收不进 `live`（2026-11-01 live 测试实测：以设备为根展开出 0 台）。
    seen: std::collections::HashSet<RefnoEnum>,
}

impl EquipScan {
    /// 分拣一层节点，返回下一层要展开的前沿。
    fn absorb(&mut self, nodes: Vec<(RefnoEnum, Option<String>, bool)>) -> Vec<RefnoEnum> {
        let mut frontier = Vec::new();
        for (refno, noun, deleted) in nodes {
            if deleted || !self.seen.insert(refno) {
                continue;
            }
            // 三者都不再往下展开：设备的子件是零件，阀门是叶子，楼板下是 PLOO / 开孔。
            match noun.as_deref() {
                Some("EQUI") => self.live.push(refno),
                Some("VALV") => self.valves.push(refno),
                Some("FLOOR") => self.floors.push(refno),
                _ => frontier.push(refno),
            }
        }
        frontier
    }

    /// 收下楼板牵连进来的设备 / 阀门（软删的、已经收过的跳过），返回新收的个数。
    /// 根下已收的设备留在 `live`（整台重算本来就会重连楼板边）。
    fn absorb_floor_dependents(&mut self, nodes: Vec<Node>) -> usize {
        let mut added = 0;
        for (refno, noun, deleted) in nodes {
            let is_equip = match noun.as_deref() {
                Some("EQUI") => true,
                Some("VALV") => false,
                _ => continue,
            };
            if deleted || !self.seen.insert(refno) {
                continue;
            }
            if is_equip {
                self.floor_equips.push(refno);
            } else {
                self.valves.push(refno);
            }
            added += 1;
        }
        added
    }
}

/// 设备子件 aabb 的原始数组：一层子件 + 两层孙件，可能夹 `NONE`。
///
/// 孙件必须写成 `<-pe_owner<-pe<-pe_owner`：`<-pe_owner` 连写两次时第二跳从**边记录**出发，
/// 恒为空（真库 2026-11-01：没挂楼板边的 489 台设备里 477 台的盒全在孙件那一层，旧写法一个都取不到）。
fn child_aabbs_raw_expr(e: &str) -> String {
    format!(
        "array::flatten([\
         array::map((SELECT VALUE in FROM {e}<-pe_owner), |$c| type::thing('inst_relate', record::id($c)).aabb.d), \
         array::map((SELECT VALUE in FROM {e}<-pe_owner<-pe<-pe_owner), |$c| type::thing('inst_relate', record::id($c)).aabb.d)\
         ])"
    )
}

/// [`child_aabbs_raw_expr`] 去掉 `NONE`：`parry3d::Aabb` 的反序列化遇到一个 `NONE` 整个数组就失败。
fn child_aabbs_expr(e: &str) -> String {
    format!(
        "array::filter({}, |$v| $v != NONE)",
        child_aabbs_raw_expr(e)
    )
}

fn pe_keys(refnos: &[RefnoEnum]) -> String {
    refnos
        .iter()
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>()
        .join(",")
}

async fn fetch_nodes(sql: String) -> anyhow::Result<Vec<(RefnoEnum, Option<String>, bool)>> {
    let mut response = SUL_DB.query(sql).await?.check()?;
    let rows: Vec<PeNode> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let refno = crate::data_interface::helper::pe_thing_to_refno(row.id).ok()?;
            Some((refno, row.noun, row.deleted.unwrap_or(false)))
        })
        .collect())
}

async fn collect_live_targets(roots: &[RefnoEnum]) -> anyhow::Result<EquipScan> {
    let mut scan = EquipScan::default();
    let mut nodes = fetch_nodes_chunked(roots, |keys| {
        format!("SELECT id, noun, deleted FROM [{keys}];")
    })
    .await?;
    for depth in 0..=EQUIP_EXPAND_DEPTH {
        let frontier = scan.absorb(std::mem::take(&mut nodes));
        if frontier.is_empty() || depth == EQUIP_EXPAND_DEPTH {
            break;
        }
        nodes = fetch_nodes_chunked(&frontier, |keys| {
            format!(
                "SELECT id, noun, deleted FROM array::flatten((SELECT VALUE <-pe_owner.in FROM [{keys}]));"
            )
        })
        .await?;
    }
    Ok(scan)
}

/// 每 [`NODE_QUERY_CHUNK`] 个 refno 一条 SQL（`sql` 拿到的是逗号拼好的 pe key），结果按原顺序接起来。
async fn fetch_nodes_chunked(
    refnos: &[RefnoEnum],
    sql: impl Fn(&str) -> String,
) -> anyhow::Result<Vec<Node>> {
    let mut nodes = Vec::new();
    for chunk in refnos.chunks(NODE_QUERY_CHUNK) {
        nodes.extend(fetch_nodes(sql(&pe_keys(chunk))).await?);
    }
    Ok(nodes)
}

/// 重算单台设备：`cal_equi` 照写；`link_floors` 时再换 `nearest_relate`。
/// 返回重算后这台设备是否挂着楼板边。
async fn refresh_one_equip(equip: RefnoEnum, link_floors: bool) -> anyhow::Result<bool> {
    rewrite_world_trans(equip).await?;
    if !link_floors {
        return Ok(false);
    }
    relink_floor(equip).await
}

async fn rewrite_world_trans(equip: RefnoEnum) -> anyhow::Result<()> {
    let world_trans = aios_core::get_world_transform(equip)
        .await?
        .unwrap_or_default();
    let transform_hash = gen_bytes_hash::<_, 64>(&world_trans);
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    transform_map.insert(transform_hash, serde_json::to_string(&world_trans)?);
    save_transforms_to_surreal(&transform_map).await?;
    SUL_DB
        .query(format!(
            "UPSERT {} SET world_trans = trans:⟨{transform_hash}⟩;",
            equip.to_table_key("cal_equi")
        ))
        .await?
        .check()?;
    Ok(())
}

/// 记一次「设备 / 阀门挂不上楼板」：射线打不中不是错误，但必须能查出来为什么。
///
/// 三处调用点以前都是静默放弃（`tracing::debug!` 在生产没有日志后端，一个字都落不到
/// 文件里），排查时只能靠外部脚本反查。记进 `geom_error` 那本账后，
/// `SELECT * FROM geom_error WHERE kind = 'equip_floor_miss'` 就是完整清单。
///
/// 成功挂边时由 [`clear_floor_miss`] 销账——只记不清的话，`occurrences` 只增不减，
/// 一台设备修好之后那行永远留着，清单会从「现在挂不上」退化成「历史上曾经挂不上」。
async fn note_floor_miss(target: &str, noun: &str, reason: &str) {
    println!("楼板未挂: {target} ({noun}) {reason}");
    crate::data_interface::geom_error::note_equip_floor_miss(target, noun, reason).await;
}

/// 这一轮重新挂上了楼板边：销掉 [`note_floor_miss`] 记下的那行。
///
/// 销账失败只 warn：边已经写成功了，账本晚一轮清干净不影响数据正确性。
async fn clear_floor_miss(target: &str) {
    if let Err(error) = crate::data_interface::geom_error::clear_equip_floor_miss(target).await {
        log::warn!("[geom_error] 楼板未挂销账失败 target={target}: {error}");
    }
}

/// 换一台设备的楼板边（调用方保证树已就绪）。返回重算后是否挂着楼板边。
async fn relink_floor(equip: RefnoEnum) -> anyhow::Result<bool> {
    let e = equip.to_pe_key();
    let mut response = SUL_DB
        .query(format!(
            "SELECT VALUE {} FROM ONLY {e};",
            child_aabbs_expr(&e)
        ))
        .await?
        .check()?;
    // 读不出来是数据异常，不是「脚下没有楼板」：上抛，旧边原样留着。
    let aabbs: Vec<Aabb> = response.take(0)?;
    let nearest = nearest_floor_below(&aabbs).await;

    let sql = match nearest {
        Some((floor, dist)) => {
            clear_floor_miss(&e).await;
            format!(
                "BEGIN TRANSACTION; DELETE {e}->nearest_relate; \
                 RELATE {e}->nearest_relate->{} SET dist = {dist}; COMMIT TRANSACTION;",
                floor.to_table_key("FLOOR")
            )
        }
        // 树已就绪仍打不中：脚下确实没有楼板（或设备已无几何），旧边已过期。
        None => {
            // 「没有子件几何」与「有几何但射线打不中」是两回事，分开记，排查不用猜。
            let reason = if aabbs.is_empty() {
                "no_child_geometry"
            } else {
                "no_floor_below"
            };
            note_floor_miss(&e, "EQUI", reason).await;
            format!("DELETE {e}->nearest_relate;")
        }
    };
    SUL_DB.query(sql).await?.check()?;
    Ok(nearest.is_some())
}

/// 重算单个阀门的楼板边（调用方保证树已就绪）。返回重算后是否挂着楼板边。
///
/// 射线口径与启动全量 [`aios_core::spatial::pipe::cal_valve_nearest_floor`] 同源：
/// 都走 `valve_nearest_floor`（阀体底面 4 角 + 中心）。以前这里用
/// `query_neareast_along_axis`（阀门原点单点），全量用原点、增量也用原点，但全量改成
/// 底面多点后两边就不一致了——同一台阀门会随触发方式算出不同的 `dist`。
async fn refresh_one_valve(valve: RefnoEnum) -> anyhow::Result<bool> {
    let nearest = aios_core::spatial::pipe::valve_nearest_floor(valve).await?;
    let v = valve.to_pe_key();
    let sql = match nearest {
        Some((floor, dist)) => {
            clear_floor_miss(&v).await;
            format!(
                "BEGIN TRANSACTION; DELETE {v}->nearest_relate; \
                 RELATE {v}->nearest_relate->{} SET dist = {dist}; COMMIT TRANSACTION;",
                floor.to_table_key("FLOOR")
            )
        }
        None => {
            note_floor_miss(&v, "VALV", "no_floor_below").await;
            format!("DELETE {v}->nearest_relate;")
        }
    };
    SUL_DB.query(sql).await?.check()?;
    Ok(nearest.is_some())
}

/// 每个子件盒底面的四个角 + 底面中心，按 0.1mm 去重。环形 / 管状布置的设备合并盒四角常常
/// 落在空处，只能逐子件打（真库 24381_57646：合并盒四角全空，逐子件 236 个点全部命中）。
///
/// 底面中心是 2026-11-08 补的：四角只覆盖轮廓，设备中部正下方有更高的局部基础 / 凸台时，
/// 四角全部落到远处楼板、中心那个点却离凸台最近，只打四角会把距离算大。
fn bottom_points(aabbs: &[Aabb]) -> Vec<Vec3> {
    let mut seen = std::collections::HashSet::new();
    let mut points = Vec::new();
    let mut push = |pt: Vec3, points: &mut Vec<Vec3>| {
        let key = (
            (pt.x * 10.0).round() as i64,
            (pt.y * 10.0).round() as i64,
            (pt.z * 10.0).round() as i64,
        );
        if seen.insert(key) {
            points.push(pt);
        }
    };
    for aabb in aabbs {
        for corner in &aabb.vertices()[..4] {
            push(Vec3::from(*corner), &mut points);
        }
        let center = aabb.center();
        push(Vec3::new(center.x, center.y, aabb.mins.z), &mut points);
    }
    points
}

/// 逐子件那一级最多提交多少个探测点。
///
/// 合并盒四角已经覆盖了轮廓，逐子件这一级只为抓「设备中部正下方更高的局部基础 / 凸台」
/// 和「穿孔立管把合并盒拉低导致的跳层」。这两类都只会出现在**最低的**若干子件下方，
/// 所以按 z 升序取前 N 个点就够；不设上限时一台 200+ 子件的设备要打 1000+ 条射线。
const BOTTOM_POINT_LIMIT: usize = 64;

/// 逐子件底角 + 底面中心向 -Z 打射线，取最近的那块楼板。
///
/// 两级：
///
/// 1. **合并盒四角**（4 个点）。绝大多数设备这一级就是最终答案。
/// 2. **逐子件底角 + 底面中心**，按 z 升序取前 [`BOTTOM_POINT_LIMIT`] 个，并把第一级
///    的结果当作初始上界传进批量查询（只有严格更近的才回报）。
///
/// 第二级不能省，也不能在第一级命中后直接返回：
///
/// - **跳层**：合并盒是外接矩形，穿孔立管会把 `merged.mins.z` 拉到本层楼板以下，四角
///   直接打到下一层。
/// - **局部凸台漏检**：设备中部正下方有更高的局部基础时，四角全落在远处楼板上，只有
///   中部那些点看得见凸台。
///
/// 以前是「四角命中即短路」，真库 2026-11-08 因此在这两类设备上连错楼板 / 连出偏大的
/// 距离。现在两级都跑，但第二级带上界剪枝 + 点数上限，全量重算耗时与短路版本同级。
async fn nearest_floor_below(aabbs: &[Aabb]) -> Option<(RefnoEnum, f32)> {
    if aabbs.is_empty() {
        return None;
    }
    let mut merged = Aabb::new_invalid();
    for aabb in aabbs {
        merged.merge(aabb);
    }
    let merged_corners = merged.vertices()[..4]
        .iter()
        .map(|corner| Vec3::from(*corner))
        .collect::<Vec<_>>();
    let coarse = nearest_floor_among(&merged_corners, None).await;
    // 贴面（0 距离）已经是最优，逐子件不可能更近。
    if let Some((_, dist)) = coarse {
        if dist <= f32::EPSILON {
            return coarse;
        }
    }
    let mut detailed = bottom_points(aabbs);
    // 按 z 升序：更低的点更可能更近，先打它们。
    detailed.sort_by(|a, b| a.z.total_cmp(&b.z));
    detailed.truncate(BOTTOM_POINT_LIMIT);
    let fine = nearest_floor_among(&detailed, coarse).await;
    match (coarse, fine) {
        (Some(a), Some(b)) => Some(if better(b, a) { b } else { a }),
        (Some(a), None) => Some(a),
        (None, fine) => fine,
    }
}

/// `candidate` 是否优于 `current`：更近，或同距但 refno 更小（保证同一台设备重跑结果稳定）。
fn better(candidate: (RefnoEnum, f32), current: (RefnoEnum, f32)) -> bool {
    candidate.1 < current.1 || (candidate.1 == current.1 && candidate.0 < current.0)
}

/// 这一批点上向 -Z 打射线，返回最近的一块楼板（0 距离直接返回）。
///
/// 一次交给 `AccelerationTree::query_nearest_below_bounded`：它只做一次 R-tree 包络
/// 粗筛，再逐点精筛。以前这里是逐点调 `query_neareast_by_pos_dir`，每个点各读一次
/// `GLOBAL_AABB_TREE`、各扫一次 R-tree——设备底面动辄几十到两百多个点时，粗筛成了主要开销。
///
/// `bound` 是调用方已有的最优解：传进去后批量查询只用它做剪枝与「是否改进」的判据，
/// 返回 `Some` 即代表这一点严格更近。
async fn nearest_floor_among(
    points: &[Vec3],
    bound: Option<(RefnoEnum, f32)>,
) -> Option<(RefnoEnum, f32)> {
    if points.is_empty() {
        return None;
    }
    let filter = HashSet::from(["FLOOR".to_string()]);
    let hits = {
        let tree = aios_core::room::room::GLOBAL_AABB_TREE.read().await;
        tree.query_nearest_below_bounded(points, &filter, bound)
            .await
            .ok()?
    };
    let mut best: Option<(RefnoEnum, f32)> = None;
    for hit in hits.into_iter().flatten() {
        if hit.1 <= f32::EPSILON {
            return Some(hit);
        }
        if best.is_none_or(|current| better(hit, current)) {
            best = Some(hit);
        }
    }
    best
}

/// 全量路径的设备枚举口径：与增量路径同源（`pe` + 未软删）。
///
/// 这里曾经写成 `select value record::id(id) from EQUI where
/// type::thing("cal_equi", record::id(id)) != none`。两处都有问题：
///
/// 1. **`type::thing(...) != none` 是恒真谓词，什么都没过滤。** `type::thing` 只
///    按表名 + id 构造一个 `Thing`（记录指针），不查这条记录是否存在；真库实测
///    `type::thing('cal_equi', '999999_999999') != none` 返回 `true`。所以这行读起来
///    像「只处理已经有 `cal_equi` 行的设备」，实际等价于裸的 `FROM EQUI`——判断
///    存在性要用 `record::exists()`。这段误导性的谓词留着只会让人以为这里有过滤。
/// 2. **它按 `EQUI` 类型表枚举，且没有 `deleted` 过滤。** `EQUI` 类型表根本没有
///    `deleted` 字段（真库 1165 行全为 NONE），软删状态只记在 `pe` 上；而
///    `read_through` 形态的软删只写 `UPDATE pe:{id} SET deleted = true`、不删类型行
///    （`src/data_interface/read_through.rs:228`），于是软删设备会被当成活设备重写
///    `world_trans`，把增量路径刚清掉的派生行又建回来。
///
/// 改成 `pe` 口径后两条一起修掉：`pe` 是权威来源，`deleted` 字段在那里，软删判断
/// 与增量路径的 `EquipScan::absorb` 同一条规则。
///
/// 真库实测（2026-11-02）：`pe` 里 `noun = 'EQUI' AND deleted != true` 恰好 1165 台，
/// 与旧的 `FROM EQUI` 口径条数相同——当前数据下两者等价（软删的 8 台类型行已清），
/// 差异在语义正确性与将来分叉时的行为。
async fn enumerate_live_equips() -> anyhow::Result<Vec<RefnoEnum>> {
    let mut response = SUL_DB
        .query("SELECT VALUE record::id(id) FROM pe WHERE noun = 'EQUI' AND deleted != true;")
        .await?
        .check()?;
    Ok(response.take(0)?)
}

/// 全量补边入口只枚举**还没有楼板边**的未删设备。
///
/// 原因：SurrealDB Rust SDK 的 `response.take::<Vec<T>>(0)` 遇到非数组值（比如
/// `(SELECT VALUE <arr> FROM pe:x WHERE array::len(->nearest_relate) = 0)[0]`
/// 在已有边时因无匹配行而返回 `CoreValue::None`）会先把 `None` 包成
/// `vec![CoreValue::None]` 再反序列化 `T`（`crates/sdk/src/api/opt/query.rs:366`）。
/// 于是热库重跑 `cal_equip_nearest_floor` 时，已挂边的 1147 台设备不仅空跑 1147 次
/// 串行 SQL，还会逐台报 `expected an object-like struct named Aabb, found None`，
/// 打出 1147 条假 `warn!("读取设备子件 aabb 失败，跳过")`。
///
/// 把 `array::len(->nearest_relate) = 0` 提到外层枚举一次滤掉后，内层直接用
/// `SELECT VALUE <expr> FROM ONLY {e};`（与 [`relink_floor`] 一致），热库重跑只需
/// 算那 18 台真正未挂边的设备，不再产生任何假告警。
fn unlinked_live_equips_sql() -> &'static str {
    "SELECT VALUE record::id(id) FROM pe WHERE noun = 'EQUI' AND deleted != true \
     AND array::len(->nearest_relate) = 0;"
}

async fn enumerate_unlinked_live_equips() -> anyhow::Result<Vec<RefnoEnum>> {
    let mut response = SUL_DB.query(unlinked_live_equips_sql()).await?.check()?;
    Ok(response.take(0)?)
}

/// 把已经挂上楼板边的未删设备从 `geom_error` 账本里销掉。
///
/// 走 [`clear_equip_floor_miss`]（内部靠 `KNOWN` 播种过滤），只有账本里确实有行的目标
/// 才会真正发 `DELETE`，正常库上是内存 O(1) 空操作。
async fn clear_linked_equip_floor_misses() {
    let targets = match SUL_DB
        .query(
            "SELECT VALUE <string>id FROM pe WHERE noun = 'EQUI' AND deleted != true \
             AND array::len(->nearest_relate) > 0;",
        )
        .await
        .and_then(|response| response.check())
        .and_then(|mut response| response.take::<Vec<String>>(0))
    {
        Ok(v) => v,
        Err(error) => {
            log::warn!("[geom_error] 反查已挂边设备失败: {error}");
            return;
        }
    };
    for target in targets {
        clear_floor_miss(&target).await;
    }
}

//将equip的 world transform 这些放到cal_equip 这张表里，存储一些需要计算的缓存数据
//这样可以减少查询次数，提高性能
//这个表的数据是在equip的数据变更时，由equip的数据变更触发的
pub async fn update_cal_equip_wtrans() -> anyhow::Result<()> {
    let equips = enumerate_live_equips().await?;
    if equips.is_empty() {
        return Ok(());
    }
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    transform_map.insert(0, serde_json::to_string(&Transform::IDENTITY).unwrap());
    let mut sql = String::new();
    for refno in equips {
        let world_trans = aios_core::get_world_transform(refno)
            .await?
            .unwrap_or_default();
        let transform_hash = gen_bytes_hash::<_, 64>(&world_trans);
        if !transform_map.contains_key(&transform_hash) {
            transform_map.insert(transform_hash, serde_json::to_string(&world_trans).unwrap());
        }
        // `UPSERT` 不是 `CREATE`：`CREATE` 撞上已有 id 报错、旧值原样留着，第二次启动起
        // world transform 就再也不更新了（真库实测 SurrealDB 2.1.4）。
        sql.push_str(&format!(
            "UPSERT cal_equi:{refno} SET world_trans=trans:⟨{}⟩;",
            transform_hash
        ));
    }
    save_transforms_to_surreal(&transform_map).await?;
    // 2026-11-01 修：这里原来是 `.unwrap()`。1165 条 `create` 挤在一个请求里，
    // 一旦超时或服务端拒绝就 panic；而这个函数现在挂在启动序列上
    // （`lib.rs` 的 `cal-equip` 段），panic 会直接带走整个进程——
    // 派生的缓存数据没写成功不该是启动杀手。
    SUL_DB
        .query(sql)
        .await
        .and_then(|response| response.check())
        .map_err(|error| anyhow::anyhow!("写入 cal_equi 的 world_trans 失败: {error}"))?;
    Ok(())
}

/// 一次 SQL 里点名多少台设备去取子件 aabb。
///
/// 全量补边以前是逐台一次 SQL（热库 18 台尚可，冷库 1165 台就是 1165 次串行 RTT）。
/// 一批 200 台时语句约 60KB，实测 8009 单请求 200～400ms，比逐台快一个数量级。
const CHILD_AABB_BATCH: usize = 200;

/// 批量读一批设备的子件 aabb（一层子件 + 两层孙件，滤掉 `NONE`）。
///
/// 返回 `(设备 → 子件盒列表)`。**只把取到结果（含空数组）的设备放进返回值**：
/// 查询里没出现的设备说明它在库里没有 `pe` 行，调用方据此区分「读不到」与「没几何」。
///
/// 逐台查询的版本在 `cal_equip_nearest_floor` 里被换成这个，原因见 [`CHILD_AABB_BATCH`]。
async fn child_aabbs_batch(
    equips: &[RefnoEnum],
) -> anyhow::Result<HashMap<RefnoEnum, Vec<Aabb>>> {
    #[derive(serde::Deserialize)]
    struct Row {
        id: RefnoEnum,
        aabbs: Vec<Aabb>,
    }
    let mut result = HashMap::new();
    for chunk in equips.chunks(CHILD_AABB_BATCH) {
        let keys = pe_keys(chunk);
        let mut response = SUL_DB
            .query(format!(
                "SELECT id, {} AS aabbs FROM [{keys}];",
                child_aabbs_expr("$this.id")
            ))
            .await?
            .check()?;
        let rows: Vec<Row> = response.take(0)?;
        for row in rows {
            result.insert(row.id, row.aabbs);
        }
    }
    Ok(result)
}

/// 一批「设备 → 楼板 + 距离」，按 [`CHILD_AABB_BATCH`] 规模分块写成一个事务。
///
/// 以前全量补边把所有 `relate` 拼成一条语句发一次（冷库 1165 条约 200KB），单请求
/// 体积过大时服务端会拒绝整条、一条边都写不进去。分块后单请求可控，块内用
/// `BEGIN/COMMIT` 保证「要么这批都写进去、要么都不写」，重跑幂等。
///
/// 每块先 `DELETE` 这些设备的旧边再 `RELATE`，与 [`relink_floor`] 同语义：一台设备
/// 只允许有一条最近楼板边。
const EDGE_WRITE_BATCH: usize = 200;

async fn write_floor_edges(edges: &[(RefnoEnum, RefnoEnum, f32)]) -> anyhow::Result<()> {
    for chunk in edges.chunks(EDGE_WRITE_BATCH) {
        let mut sql = String::from("BEGIN TRANSACTION;");
        for (equip, floor, dist) in chunk {
            let e = equip.to_pe_key();
            sql.push_str(&format!(
                "DELETE {e}->nearest_relate; RELATE {e}->nearest_relate->{} SET dist = {dist};",
                floor.to_table_key("FLOOR")
            ));
        }
        sql.push_str("COMMIT TRANSACTION;");
        SUL_DB
            .query(sql)
            .await
            .and_then(|response| response.check())
            .map_err(|error| anyhow::anyhow!("写入 nearest_relate 失败: {error}"))?;
    }
    Ok(())
}

//取得设备下的所有aabb，然后取下面的点到楼板的最近距离
pub async fn cal_equip_nearest_floor() -> anyhow::Result<()> {
    clear_linked_equip_floor_misses().await;
    let equips = enumerate_unlinked_live_equips().await?;
    if equips.is_empty() {
        return Ok(());
    }

    // 子件 aabb 按批读：逐台一次 SQL 时冷库 1165 台就是 1165 次串行 RTT。
    //
    // 读法（`child_aabbs_expr`）的由来见该函数：`->inst_relate` 是错写法（`inst_relate`
    // 是普通表不是从 `pe` 出去的边），孙件必须写成 `<-pe_owner<-pe<-pe_owner`（连写两次
    // `<-pe_owner` 时第二跳从边记录出发恒为空），最外层必须 `array::filter(..., |$v| $v != NONE)`
    // （`parry3d::Aabb` 的反序列化遇到一个 `NONE` 整个数组就失败）。
    let aabbs_by_equip = child_aabbs_batch(&equips).await?;

    let mut edges: Vec<(RefnoEnum, RefnoEnum, f32)> = Vec::new();
    for equip in equips {
        let e = equip.to_pe_key();
        let Some(aabbs) = aabbs_by_equip.get(&equip) else {
            // 库里没有这台 `pe` 行（刚被删 / 权限问题）。以前这里会静默跳过。
            tracing::warn!(equip = ?equip, "读取设备子件 aabb 失败，跳过");
            continue;
        };
        if aabbs.is_empty() {
            tracing::debug!(equip = ?equip, "设备无子件 aabb，跳过");
            note_floor_miss(&e, "EQUI", "no_child_geometry").await;
            continue;
        }
        // 逐子件底角打射线，取最近的一块楼板（合并盒四角见 `nearest_floor_below` 的说明）。
        match nearest_floor_below(aabbs).await {
            Some((nearest, dist)) => {
                clear_floor_miss(&e).await;
                edges.push((equip, nearest, dist));
            }
            None => note_floor_miss(&e, "EQUI", "no_floor_below").await,
        }
    }
    write_floor_edges(&edges).await?;
    Ok(())
}

#[cfg(test)]
mod nearest_floor_live_tests {
    //! `cal_equip_nearest_floor` 的两条 live 钉，都需要真库 8009。
    //!
    //! 对应 2026-11-01 修掉的两个缺陷：
    //!
    //! 1. **射线候选集**（vendor `accel_tree`）：`should_unpack_leaf` 曾只保留
    //!    AABB 最近的那一个候选。AABB 的 `time_of_impact` 是真实三角网格 toi
    //!    的**下界**，所以「AABB 最近」经常是网格在该 (x,y) 镂空的构件（楼板
    //!    有孔洞、开口、异形轮廓）。只留它，精筛失败就返回 `None`，真实命中的
    //!    候选全被丢掉。
    //!
    //! 2. **子件缺 aabb 整台跳过**（本模块）：`parry3d::Aabb` 的 `Deserialize`
    //!    是严格对象结构体，数组里混一个 `NONE` 就让整个 `Vec<Aabb>` 失败，
    //!    原来的 `let Ok(...) else { continue }` 把整台设备静默吞掉。
    //!
    //! 运行（**必须逐条单跑**：`SUL_DB` 是进程级全局连接，同一进程里第二次
    //! `attach_store_connected` 会让后一条测试的射线查询全部返回 None）：
    //!
    //! ```text
    //! cargo test --lib child_aabb_survives_none_entries -- --ignored --test-threads=1
    //! cargo test --lib coverage_does_not_regress          -- --ignored --test-threads=1
    //! ```

    use super::*;
    // 走 `attach_store_connected`（lib.rs 里点名的旁路入口）而不是
    // `test::test_query::init_test_surreal`——后者所在模块已被注释掉，
    // 且它的连接建在当前 runtime 上，进程跑完就没了。
    use crate::attach_store_connected;

    /// 设备子件里混着 `NONE` 时，过滤后必须还能反序列化出非空 `Vec<Aabb>`。
    ///
    /// 这条钉的是 SQL 最外层的 `array::filter(..., |$v| $v != NONE)`。去掉它，
    /// `take::<Vec<Aabb>>` 就整体失败——真库实测 1165 台 EQUI 里 525 台因此
    /// 被静默跳过。
    #[tokio::test]
    #[ignore = "live: 需要真库 8009（AvevaMarineSample）；必须单跑，见模块注释"]
    async fn child_aabb_survives_none_entries() -> anyhow::Result<()> {
        attach_store_connected().await?;

        // 找一台「有子件、且子件里确实混了 NONE」的设备。
        let mut r = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM pe WHERE noun = 'EQUI' AND array::len(<-pe_owner) > 0 \
                 AND array::len({}) > array::len({}) LIMIT 1;",
                child_aabbs_raw_expr("$this.id"),
                child_aabbs_expr("$this.id")
            ))
            .await?;
        let ids: Vec<surrealdb::sql::Thing> = r.take(0)?;
        if ids.is_empty() {
            eprintln!("库里没有「子件混 NONE」的设备，跳过");
            return Ok(());
        }
        let key = format!("{}", ids[0]);

        // 不过滤：必然失败。这是本条钉的立论基础——如果哪天不失败了，说明
        // Aabb 的 Deserialize 放宽了，本模块里的 array::filter 就可以撤。
        let raw = format!("RETURN {};", child_aabbs_raw_expr(&key));
        let mut resp = SUL_DB.query(raw).await?;
        assert!(
            resp.take::<Vec<Aabb>>(0).is_err(),
            "含 NONE 的数组本该反序列化失败；若这里通过了，说明 Aabb 的 \
             Deserialize 已放宽，equip_model 里的 array::filter 可以撤"
        );

        // 过滤后：必须成功且非空。
        let filtered = format!("RETURN {};", child_aabbs_expr(&key));
        let mut resp = SUL_DB.query(filtered).await?;
        let aabbs: Vec<Aabb> = resp.take(0)?;
        assert!(
            !aabbs.is_empty(),
            "{key} 过滤掉 NONE 之后仍有子件几何，不该为空"
        );
        eprintln!("{key}: 过滤后 {} 个 aabb", aabbs.len());

        Ok(())
    }

    /// 跑一遍 `cal_equip_nearest_floor`，覆盖率不能退回修前水平。
    ///
    /// 修前基线：1165 台 EQUI 只有 367 台写出 `nearest_relate`（31.5%）。
    /// 两个缺陷都修掉后是 675 台（57.9%）；射线起点后退修掉「底面贴楼板面」后
    /// 676/1173（57.6%，2026-11-01）。
    ///
    /// 之后查明「483 台子件全是 SUBE、没有几何」是**查询写错**而不是数据缺口：孙件那一跳写成
    /// 连写两次 `<-pe_owner`，第二跳从边记录出发恒为空；SUBE 下的零件其实都有 `inst_relate`。
    /// 改成 `<-pe_owner<-pe<-pe_owner` 后（`child_aabbs_raw_expr`），489 台没挂边的设备里 484 台
    /// 取得到盒（合并盒 xy 跨度中位 563mm、p90 3.8m）。修后全量 1147/1165（98.45%，分母已排除 8 台软删 EQUI，2026-11-02）。
    /// 射线本身（网格读取、坐标变换、solid 标志）没有查出问题。
    #[tokio::test]
    #[ignore = "live: 需要真库 8009，会清空并重写 nearest_relate；必须单跑，见模块注释"]
    async fn coverage_does_not_regress() -> anyhow::Result<()> {
        attach_store_connected().await?;

        // 分母口径与生产对齐：生产按 EQUI 类型表 + cal_equi 行枚举（真库 1165 台、无软删），
        // pe 里那 8 台 deleted = true 的 EQUI 生产本来就不碰，不该算进「未覆盖」。
        let mut r = SUL_DB
            .query("RETURN count(SELECT * FROM pe WHERE noun = 'EQUI' AND deleted != true);")
            .await?;
        let total: Option<i64> = r.take(0)?;
        let total = total.unwrap_or(0);
        if total == 0 {
            eprintln!("库里没有 EQUI，跳过");
            return Ok(());
        }

        // 清空 EQUI 的边后重算，避免旧的边让内层 `where array::len(->nearest_relate)=0`
        // 提前跳过。
        //
        // 只清 `in.noun = 'EQUI'`：这条测试只验设备覆盖率，而 `DELETE nearest_relate;`
        // 是全表删，会把阀门那 510 条边一起清掉。真库 2026-11-02 实测——跑一次这条
        // 测试，VALV 边从 510 变 0，而阀门全量只在启动序列里跑，下一次启动前阀门侧
        // 一直是空的。测试不该有这种跨 noun 的副作用。
        SUL_DB
            .query("DELETE nearest_relate WHERE in.noun = 'EQUI';")
            .await?;
        // `cal_equip_nearest_floor` 靠 `GLOBAL_AABB_TREE` 做射线查询，不先加载
        // 树的话 `query_neareast_by_pos_dir` 一律返回 None，覆盖率会是 0。
        crate::fast_model::aabb_tree::load_project_tree_verified().await?;
        cal_equip_nearest_floor().await?;

        let mut r = SUL_DB
            .query(
                "RETURN count(SELECT * FROM pe WHERE noun = 'EQUI' AND deleted != true AND array::len(->nearest_relate) > 0);",
            )
            .await?;
        let covered: Option<i64> = r.take(0)?;
        let covered = covered.unwrap_or(0);
        let pct = covered as f64 * 100.0 / total as f64;
        eprintln!("EQUI 覆盖 {covered}/{total} = {pct:.1}%");

        // 修后 98.45%（分母已排除软删）。门槛设在 96%：孙件路径、射线候选集、NONE 过滤任何一项回退都会掉到 60% 以下。
        assert!(
            pct >= 96.0,
            "覆盖率 {pct:.1}% 低于 96%，孙件路径 / 射线候选集 / NONE 过滤可能回退了（修后 98.45%）"
        );

        Ok(())
    }

    /// 热库重跑只碰「还没有楼板边」的设备，且不产生任何假告警。
    ///
    /// 修前：全量入口枚举全部 1165 台未删设备，再用
    /// `(SELECT VALUE <arr> FROM pe:x WHERE array::len(->nearest_relate) = 0)[0]`
    /// 过滤。已挂边的 1147 台让子查询返回 `CoreValue::None`，
    /// `response.take::<Vec<Aabb>>(0)` 把它包成 `vec![CoreValue::None]` 后反序列化失败
    /// （`crates/sdk/src/api/opt/query.rs:366`），于是热库每跑一次全量就打 1147 条
    /// 假 `warn!("读取设备子件 aabb 失败，跳过")`、空耗 1147 次串行 SQL。
    ///
    /// 修后：枚举阶段就按 `array::len(->nearest_relate) = 0` 过滤，热库只剩真正
    /// 挂不上的那十几台要算。
    ///
    /// ```text
    /// cargo test --lib hot_rerun_only_touches_unlinked_equips -- --ignored --test-threads=1
    /// ```
    #[tokio::test]
    #[ignore = "live: 需要真库 8009；会先跑一遍全量再重跑一次；必须单跑"]
    async fn hot_rerun_only_touches_unlinked_equips() -> anyhow::Result<()> {
        attach_store_connected().await?;
        crate::fast_model::aabb_tree::load_project_tree_verified().await?;

        // 先跑一遍全量，把热库状态建立起来（有边的设备占绝大多数）。
        cal_equip_nearest_floor().await?;

        let mut r = SUL_DB
            .query(
                "RETURN count(SELECT * FROM pe WHERE noun = 'EQUI' AND deleted != true \
                 AND array::len(->nearest_relate) = 0);",
            )
            .await?;
        let unlinked: Option<i64> = r.take(0)?;
        let unlinked = unlinked.unwrap_or(0);
        let mut r = SUL_DB
            .query("RETURN count(SELECT * FROM pe WHERE noun = 'EQUI' AND deleted != true);")
            .await?;
        let total: Option<i64> = r.take(0)?;
        let total = total.unwrap_or(0);
        assert!(total > 0, "库里没有 EQUI");

        // 枚举口径必须与「未挂边」严格一致：这是「热库不再空跑」的根。
        let enumerated = enumerate_unlinked_live_equips().await?;
        assert_eq!(
            enumerated.len() as i64,
            unlinked,
            "枚举出来的必须恰好是没挂边的那些设备"
        );
        assert!(
            (unlinked as f64) < (total as f64) * 0.2,
            "热库里挂不上的应只是少数：{unlinked}/{total}"
        );

        // 热库重跑：应当只算这 {unlinked} 台，耗时与写出的边数都要跟着掉下来。
        let started = std::time::Instant::now();
        cal_equip_nearest_floor().await?;
        let elapsed = started.elapsed();
        eprintln!("热库重跑 {unlinked}/{total} 台未挂边，耗时 {elapsed:?}");

        // 假告警的判据：重跑后挂边数不能因为反序列化失败而掉下来。
        let mut r = SUL_DB
            .query(
                "RETURN count(SELECT * FROM pe WHERE noun = 'EQUI' AND deleted != true \
                 AND array::len(->nearest_relate) > 0);",
            )
            .await?;
        let covered: Option<i64> = r.take(0)?;
        let covered = covered.unwrap_or(0);
        assert_eq!(
            covered,
            total - unlinked,
            "热库重跑不该改变挂边数（{covered} vs {}）",
            total - unlinked
        );
        Ok(())
    }
}

#[cfg(test)]
mod equip_scan_tests {
    //! 「根 → 设备」分拣的离线钉，不需要真库。
    use super::*;

    fn r(n: u32) -> RefnoEnum {
        RefnoEnum::from(format!("24381/{n}").as_str())
    }

    fn node(n: u32, noun: &str, deleted: bool) -> (RefnoEnum, Option<String>, bool) {
        (r(n), Some(noun.to_string()), deleted)
    }

    /// 生成根本身就是设备（EQUI 自成交付单元，这是常态）时要把它自己收下，且不再往下展开。
    #[test]
    fn equip_root_collects_itself() {
        let mut scan = EquipScan::default();
        let frontier = scan.absorb(vec![node(1, "EQUI", false)]);
        assert_eq!(scan.live, vec![r(1)]);
        assert!(frontier.is_empty(), "设备的子件是零件，不再往下展开");
    }

    #[test]
    fn container_root_expands_to_child_equips() {
        let mut scan = EquipScan::default();
        let frontier = scan.absorb(vec![node(1, "ZONE", false)]);
        assert_eq!(frontier, vec![r(1)]);
        assert!(scan.live.is_empty());

        let frontier = scan.absorb(vec![
            node(2, "EQUI", false),
            node(3, "EQUI", false),
            node(4, "STRU", false),
        ]);
        assert_eq!(scan.live, vec![r(2), r(3)]);
        assert_eq!(frontier, vec![r(4)]);
    }

    #[test]
    fn deleted_nodes_are_neither_collected_nor_expanded() {
        let mut scan = EquipScan::default();
        let frontier = scan.absorb(vec![node(1, "EQUI", true), node(2, "ZONE", true)]);
        assert!(scan.live.is_empty());
        assert!(frontier.is_empty());
    }

    /// 全量与增量必须都清软删设备的派生行。
    ///
    /// 这条钉的是「只有一侧清」的回归：软删的 EQUI 断开 `pe_owner`，枚举会跳过它，
    /// 但它留下的 `cal_equi` 行与 `nearest_relate` 边不会自己消失。真库 2026-11-02
    /// 实测，软删 `pe:24381_101725` 后跑全量，`{cal: true, edges: 1}` 原样留着。
    ///
    /// 用源码文本断言而不是行为测试：清理是两条 SQL，离线跑不了；而「两条入口都调了」
    /// 这件事在源码里是唯一可查的证据。
    #[test]
    fn both_paths_clear_soft_deleted_derived_rows() {
        let src = include_str!("equip_model.rs");
        // 两条入口（update_cal_equip 与 update_cal_equip_for_roots）都得调。
        assert!(
            src.matches("clear_soft_deleted_equip_rows().await?;")
                .count()
                >= 2,
            "全量与增量都要调 clear_soft_deleted_equip_rows"
        );
        // 软删清理必须有自己的函数体，不能在两处各写一份 SQL。
        assert!(
            src.contains("async fn clear_soft_deleted_equip_rows()"),
            "软删清理要有独立函数，两条入口共用"
        );
    }

    /// 全量补边入口只能枚举「还没有楼板边」的设备。
    ///
    /// 热库重跑时若枚举全部未删设备、再用
    /// `(SELECT VALUE <arr> FROM pe:x WHERE array::len(->nearest_relate) = 0)[0]`
    /// 过滤，已挂边的设备会让子查询无匹配行、返回 `CoreValue::None`，而
    /// `response.take::<Vec<Aabb>>(0)` 会把 `None` 包成 `vec![CoreValue::None]`
    /// 再反序列化（`crates/sdk/src/api/opt/query.rs:366`），报
    /// `expected an object-like struct named Aabb, found None`。
    ///
    /// 后果是热库每跑一次全量：1147 次无效串行 SQL + 1147 条假
    /// `warn!("读取设备子件 aabb 失败，跳过")`。真库 2026-11-08 实测。
    #[test]
    fn the_full_pass_only_visits_equips_without_a_floor_edge() {
        let src = include_str!("equip_model.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        let body = src
            .split_once("pub async fn cal_equip_nearest_floor()")
            .expect("全量补边入口")
            .1;

        assert!(
            body.contains("enumerate_unlinked_live_equips().await?"),
            "全量补边入口必须先按「还没有边」过滤再枚举"
        );
        assert!(
            !body.contains("WHERE array::len(->nearest_relate) = 0)[0]"),
            "子查询过滤会让已挂边的设备返回 None，进而反序列化失败打出假告警"
        );
        assert!(
            body.contains("child_aabbs_batch(&equips)"),
            "子件 aabb 必须按批读，逐台一次 SQL 在冷库是 1165 次串行 RTT"
        );
        assert!(
            body.contains("write_floor_edges(&edges)"),
            "边必须走分块事务写入，单条 SQL 拼 1165 条 relate 会被服务端整条拒绝"
        );

        let sql = unlinked_live_equips_sql();
        assert!(
            sql.contains("noun = 'EQUI'") && sql.contains("deleted != true"),
            "枚举口径要与增量路径同源（pe + 未软删）: {sql}"
        );
        assert!(
            sql.contains("array::len(->nearest_relate) = 0"),
            "必须只枚举没有楼板边的设备: {sql}"
        );
    }

    /// 覆盖率测试只能清设备自己的边，不能全表删。
    ///
    /// `DELETE nearest_relate;` 会把阀门那 510 条边一起清掉，而阀门全量只在启动序列里
    /// 跑——跑一次这条测试，阀门侧就要等到下次启动才恢复。真库 2026-11-02 实测：
    /// 跑一次 `coverage_does_not_regress`，VALV 边从 510 变 0。
    #[test]
    fn the_coverage_test_does_not_wipe_valve_edges() {
        let src = include_str!("equip_model.rs");
        assert!(
            !src.contains("DELETE nearest_relate;\")"),
            "覆盖率测试不能全表删 nearest_relate，会连阀门边一起清掉"
        );
        assert!(
            src.contains("DELETE nearest_relate WHERE in.noun = 'EQUI';"),
            "覆盖率测试只该清设备自己的边"
        );
    }

    #[test]
    fn repeated_nodes_are_taken_once() {
        let mut scan = EquipScan::default();
        scan.absorb(vec![node(1, "ZONE", false), node(2, "EQUI", false)]);
        let frontier = scan.absorb(vec![node(1, "ZONE", false), node(2, "EQUI", false)]);
        assert_eq!(scan.live, vec![r(2)]);
        assert!(frontier.is_empty(), "已经见过的节点不再进前沿（防环）");
    }

    #[test]
    fn valves_and_floors_are_collected_without_expanding() {
        let mut scan = EquipScan::default();
        let frontier = scan.absorb(vec![
            node(1, "VALV", false),
            node(2, "FLOOR", false),
            node(3, "BRAN", false),
            node(4, "VALV", true),
        ]);
        assert_eq!(scan.valves, vec![r(1)]);
        assert_eq!(scan.floors, vec![r(2)]);
        assert_eq!(frontier, vec![r(3)], "阀门、楼板都不再往下展开");
    }

    /// 楼板牵连进来的设备只进 `floor_equips`（只重连）；与根下已收的去重，软删的、非设备阀门的不收。
    #[test]
    fn floor_dependents_merge_without_duplicates() {
        let mut scan = EquipScan::default();
        scan.absorb(vec![node(1, "EQUI", false), node(2, "FLOOR", false)]);
        let added = scan.absorb_floor_dependents(vec![
            node(1, "EQUI", false),
            node(5, "EQUI", false),
            node(6, "VALV", false),
            node(7, "VALV", true),
            node(8, "BOX", false),
        ]);
        assert_eq!(added, 2);
        assert_eq!(scan.live, vec![r(1)], "根下的设备留在整台重算那一路");
        assert_eq!(scan.floor_equips, vec![r(5)]);
        assert_eq!(scan.valves, vec![r(6)]);

        let again =
            scan.absorb_floor_dependents(vec![node(5, "EQUI", false), node(6, "VALV", false)]);
        assert_eq!(again, 0, "现有边反查与空间反查拉到同一个，只收一次");
    }

    fn link(
        own: (u32, &str),
        parent: Option<(u32, &str, bool)>,
        grand: Option<(u32, &str, bool)>,
    ) -> ColumnLink {
        ColumnLink {
            node: node(own.0, own.1, false),
            parent: parent.map(|(n, noun, d)| node(n, noun, d)),
            grand: grand.map(|(n, noun, d)| node(n, noun, d)),
        }
    }

    #[test]
    fn column_entries_resolve_to_their_equip_or_valve() {
        assert_eq!(
            floor_dependent_of(link((1, "VALV"), Some((2, "BRAN", false)), None)),
            Some(node(1, "VALV", false))
        );
        assert_eq!(
            floor_dependent_of(link((1, "BOX"), Some((2, "EQUI", false)), None)),
            Some(node(2, "EQUI", false))
        );
        assert_eq!(
            floor_dependent_of(link(
                (1, "BOX"),
                Some((2, "SUBE", false)),
                Some((3, "EQUI", false))
            )),
            Some(node(3, "EQUI", false)),
            "SUBE 下的零件归到祖父那台设备"
        );
        assert_eq!(
            floor_dependent_of(link((1, "BOX"), Some((2, "EQUI", true)), None)),
            Some(node(2, "EQUI", true)),
            "软删标记原样带出，由 absorb_floor_dependents 丢掉"
        );
        assert_eq!(
            floor_dependent_of(link(
                (1, "PANE"),
                Some((2, "STRU", false)),
                Some((3, "ZONE", false))
            )),
            None
        );
    }

    #[test]
    fn floor_column_spans_the_floor_footprint_upward() {
        let floor = Aabb::new(
            parry3d::math::Point::new(0.0, 0.0, 100.0),
            parry3d::math::Point::new(10.0, 20.0, 150.0),
        );
        let column = floor_column(&floor).expect("有效楼板盒");
        assert_eq!(
            column.mins, floor.mins,
            "z 从楼板底面起：贴在楼板里的阀门也算"
        );
        assert_eq!((column.maxs.x, column.maxs.y), (10.0, 20.0));
        assert_eq!(column.maxs.z, f32::MAX);
        assert!(
            floor_column(&Aabb::new_invalid()).is_none(),
            "楼板不在树上就没有柱子"
        );
    }

    /// 孙件那一跳必须经过 `pe`：连写两次 `<-pe_owner` 恒为空，曾让 477 台设备取不到盒。
    #[test]
    fn child_aabbs_walk_through_pe_to_reach_grandchildren() {
        let expr = child_aabbs_expr("pe:24381_1");
        assert!(
            expr.contains("pe:24381_1<-pe_owner<-pe<-pe_owner"),
            "{expr}"
        );
        assert!(
            expr.starts_with("array::filter("),
            "最外层要滤掉 NONE: {expr}"
        );

        let source = include_str!("equip_model.rs");
        let doubled = ["<-pe_owner", "<-pe_owner"].concat();
        assert!(
            !source.contains(&doubled),
            "本文件不许再出现两跳之间不经 pe 的写法"
        );
    }

    /// 每个子件底面出 4 角 + 底面中心；重复的不再计。
    ///
    /// 底面中心是 2026-11-08 补的：四角只覆盖轮廓，设备中部正下方更高的局部基础 / 凸台
    /// 只有中心点看得见（真库 24381_46777 这类支腿设备）。
    #[test]
    fn bottom_points_cover_every_child_with_corners_and_center() {
        let far = Aabb::new(
            parry3d::math::Point::new(0.0, 0.0, 0.0),
            parry3d::math::Point::new(10.0, 10.0, 10.0),
        );
        let near = Aabb::new(
            parry3d::math::Point::new(100.0, 100.0, 0.0),
            parry3d::math::Point::new(110.0, 110.0, 10.0),
        );
        let points = bottom_points(&[far, near, far]);
        assert_eq!(
            points.len(),
            10,
            "两个不同子件各 4 角 + 1 中心，重复的不再计: {points:?}"
        );
        assert!(
            points.iter().all(|p| (p.z - 0.0).abs() < 1e-6),
            "只取底面（z 最小）: {points:?}"
        );
        assert!(
            points.iter().any(|p| (p.x - 100.0).abs() < 1e-6)
                && points.iter().any(|p| (p.x - 110.0).abs() < 1e-6),
            "每个子件自己的角都要在，不能用合并盒的四角代替: {points:?}"
        );
        // 中心点：两个子件的底面中心各一个，坐标是各自盒的 xy 中点。
        assert!(
            points
                .iter()
                .any(|p| (p.x - 5.0).abs() < 1e-6 && (p.y - 5.0).abs() < 1e-6),
            "第一个子件的底面中心必须探到: {points:?}"
        );
        assert!(
            points
                .iter()
                .any(|p| (p.x - 105.0).abs() < 1e-6 && (p.y - 105.0).abs() < 1e-6),
            "第二个子件的底面中心必须探到: {points:?}"
        );
        assert!(bottom_points(&[]).is_empty());
    }

    /// 合并盒那一级命中后不能直接短路：穿孔立管会把合并盒拉到本层楼板以下，四角直接跳层；
    /// 而设备中部更高的局部凸台只有逐子件点看得见。
    #[test]
    fn nearest_floor_below_does_not_short_circuit_on_the_merged_box() {
        let src = include_str!("equip_model.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        let body = src
            .split_once("async fn nearest_floor_below(")
            .expect("设备底面探测入口")
            .1
            .split_once("\n/// 这一批点上向 -Z 打射线")
            .expect("入口结尾")
            .0;
        assert!(
            !body.contains("return Some(hit);"),
            "合并盒命中后不能直接返回，否则穿孔立管跳层、局部凸台漏检"
        );
        assert!(
            body.contains("BOTTOM_POINT_LIMIT"),
            "逐子件那一级要有探测点上限，否则一台 200+ 子件的设备要打上千条射线"
        );
        assert!(
            body.contains("nearest_floor_among(&detailed, coarse)"),
            "合并盒的结果必须作为上界传进逐子件那一级，让批量查询直接剪枝"
        );
        assert!(body.contains("better(b, a)"), "两级结果必须比出更近的那个");
    }

    /// 同距时按 refno 定序，保证同一台设备重跑连到同一块楼板。
    #[test]
    fn equal_distances_resolve_to_the_smaller_refno() {
        let a = (RefnoEnum::from("24381/100"), 50.0);
        let b = (RefnoEnum::from("24381/200"), 50.0);
        assert!(better(a, b), "同距选 refno 小的");
        assert!(!better(b, a));
        let closer = (RefnoEnum::from("24381/300"), 10.0);
        assert!(better(closer, a), "更近的赢");
    }

    #[test]
    fn floor_dependents_sql_names_the_floor_table_and_soft_deleted_floors() {
        let sql = floor_dependents_sql(&[r(2), r(3)]);
        assert!(
            sql.contains("out IN [FLOOR:24381_2,FLOOR:24381_3]"),
            "{sql}"
        );
        assert!(
            !sql.contains("deleted = true"),
            "软删楼板那一句单独问一次，不随每批楼板重复: {sql}"
        );
        assert!(
            soft_deleted_floor_dependents_sql().contains("record::id(out)).deleted = true"),
            "挂在软删楼板上的照样拉进来"
        );
    }

    /// #88：期限只在目标之间检查，没设期限就永不到期。
    #[test]
    fn the_budget_only_runs_out_once_its_deadline_has_passed() {
        let now = Instant::now();
        assert!(!budget_exhausted(None, now), "没设期限就不到期");
        assert!(!budget_exhausted(Some(now + Duration::from_secs(1)), now));
        assert!(budget_exhausted(Some(now), now), "到点即到期");
        assert!(budget_exhausted(Some(now), now + Duration::from_millis(1)));
    }

    /// #88：整库 ModelOnly 或根数到门槛走整库口径（不做楼板柱子空间反查）。
    #[test]
    fn whole_library_or_many_roots_refresh_at_library_scale() {
        assert!(library_scale_refresh(true, 1));
        assert!(library_scale_refresh(
            false,
            EQUIP_DERIVED_LIBRARY_SCALE_ROOTS
        ));
        assert!(!library_scale_refresh(
            false,
            EQUIP_DERIVED_LIBRARY_SCALE_ROOTS - 1
        ));
        assert!(!library_scale_refresh(false, 0));
    }

    /// #88：任务里的那一轮必须带期限、口径与进度回调；空间反查排在最后，期限先砍最贵的那段。
    #[test]
    fn the_spatial_floor_pass_runs_last_and_honours_the_deadline() {
        let source = include_str!("equip_model.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        let body = source
            .split_once("pub async fn update_cal_equip_for_roots_with(")
            .expect("带期限的入口")
            .1
            .split_once("\nstruct Progress")
            .expect("入口结尾")
            .0;
        let live = body.find("refresh_one_equip(").expect("根下设备");
        let existing = body
            .find("floor_dependents(&scan.floors)")
            .expect("现有边反查");
        let library = body
            .find("outcome.floor_space_skipped =")
            .expect("整库口径");
        let spatial = body
            .find("column_dependents(&entries, scope.deadline)")
            .expect("空间反查");
        assert!(
            live < existing && existing < library && library < spatial,
            "顺序必须是 根下设备 → 现有边反查 → 整库口径收手 → 柱子空间反查"
        );
        assert!(
            body.matches("budget_exhausted(scope.deadline").count() >= 3,
            "设备循环、现有边反查前、楼板循环都要查期限"
        );
    }
}

#[cfg(test)]
mod incremental_live_tests {
    //! `update_cal_equip_for_roots` 的 live 钉，需要真库 8009。
    //!
    //! **必须逐条单跑**：`SUL_DB` 是进程级全局连接，空间树状态也是进程级的；第二条要求
    //! 进程里**没有**装过空间树。
    //!
    //! ```text
    //! cargo test --lib container_root_refreshes_every_equip_and_relinks_floors -- --ignored --test-threads=1
    //! cargo test --lib equip_root_overwrites_stale_world_trans_and_keeps_edges_without_tree -- --ignored --test-threads=1
    //! cargo test --lib floor_root_relinks_what_hangs_on_its_floors -- --ignored --test-threads=1
    //! cargo test --lib floor_space_finds_equips_that_did_not_link_the_floor -- --ignored --test-threads=1
    //! ```

    use super::*;
    use crate::attach_store_connected;
    use crate::data_interface::helper::pe_thing_to_refno;
    use crate::fast_model::spatial_state;

    /// 一个下挂 >= `min_equips` 台未删 EQUI 的 ZONE，连同这些 EQUI。
    async fn zone_root_with_equips(
        min_equips: usize,
    ) -> anyhow::Result<(RefnoEnum, Vec<RefnoEnum>)> {
        let mut r = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM pe WHERE noun = 'ZONE' \
                 AND array::len(<-pe_owner[WHERE in.noun = 'EQUI']) >= {min_equips} LIMIT 1;"
            ))
            .await?
            .check()?;
        let found: Vec<surrealdb::sql::Thing> = r.take(0)?;
        let root = pe_thing_to_refno(
            found
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("库里没有下挂 >= {min_equips} 台 EQUI 的 ZONE"))?,
        )?;

        let mut r = SUL_DB
            .query(format!(
                "SELECT VALUE in FROM {}<-pe_owner WHERE in.noun = 'EQUI' AND in.deleted != true;",
                root.to_pe_key()
            ))
            .await?
            .check()?;
        let things: Vec<surrealdb::sql::Thing> = r.take(0)?;
        let equips = things
            .into_iter()
            .map(pe_thing_to_refno)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((root, equips))
    }

    /// 这些设备的楼板边，`设备->楼板` 形式、排好序。
    ///
    /// 不能写 `WHERE in IN [..]`：表上有 `(in, out)` 唯一索引，SurrealDB 2.1.4 走索引后一行都不回
    /// （真库实测 1 / 41 个起点都是 0 行），`[..] CONTAINS in` 不走索引、结果正确。
    async fn floor_edges(equips: &[RefnoEnum]) -> anyhow::Result<Vec<String>> {
        let mut r = SUL_DB
            .query(format!(
                "SELECT VALUE record::id(in) + '->' + record::id(out) FROM nearest_relate WHERE [{}] CONTAINS in;",
                pe_keys(equips)
            ))
            .await?
            .check()?;
        let mut edges: Vec<String> = r.take(0)?;
        edges.sort();
        Ok(edges)
    }

    /// 容器根要重算它下面每一台设备；树已就绪时楼板边按「一台一条」换新，重跑不堆边。
    #[tokio::test]
    #[ignore = "live: 需要真库 8009；会装载空间树并重写这些设备的 cal_equi / nearest_relate；必须单跑"]
    async fn container_root_refreshes_every_equip_and_relinks_floors() -> anyhow::Result<()> {
        attach_store_connected().await?;
        crate::fast_model::aabb_tree::load_project_tree_verified().await?;
        assert!(
            spatial_state::current_state().is_ready(),
            "装载后空间树应就绪，实际 {}",
            spatial_state::current_state().as_str()
        );

        let (root, equips) = zone_root_with_equips(2).await?;
        let started = std::time::Instant::now();
        let first = update_cal_equip_for_roots(&[root]).await?;
        eprintln!(
            "根 {}（{} 台 EQUI）→ {first:?}，{}ms",
            root.to_pe_key(),
            equips.len(),
            started.elapsed().as_millis()
        );
        assert_eq!(first.refreshed, equips.len(), "每台 EQUI 子件都要重算");
        assert_eq!(first.failed, 0);
        assert!(!first.floors_skipped, "树已就绪，不该跳过楼板边");
        assert!(first.floors_linked >= 1, "树已就绪，至少一台该打中楼板");
        let edges = floor_edges(&equips).await?;
        assert_eq!(
            edges.len(),
            first.floors_linked,
            "挂楼板的设备恰好一条边: {edges:?}"
        );

        let second = update_cal_equip_for_roots(&[root]).await?;
        assert_eq!(second, first, "同一个根重跑，结果应一致");
        assert_eq!(floor_edges(&equips).await?, edges, "重跑不能堆边或换边");
        Ok(())
    }

    /// 楼板所在的根变了：挂在这些楼板上的设备与阀门全部重算，重跑结果与边都不变。
    #[tokio::test]
    #[ignore = "live: 需要真库 8009；会装载空间树并重写挂在一组楼板上的 nearest_relate；必须单跑"]
    async fn floor_root_relinks_what_hangs_on_its_floors() -> anyhow::Result<()> {
        attach_store_connected().await?;
        crate::fast_model::aabb_tree::load_project_tree_verified().await?;

        // 楼板的生成根是它的 CFLOOR（generation_root.rs），取一个下面有楼板被挂着的。
        let mut r = SUL_DB
            .query(
                "SELECT VALUE type::thing('pe', record::id(out)).owner FROM nearest_relate \
                 WHERE type::thing('pe', record::id(out)).owner.noun = 'CFLOOR' LIMIT 1;",
            )
            .await?
            .check()?;
        let found: Vec<surrealdb::sql::Thing> = r.take(0)?;
        let root = pe_thing_to_refno(
            found
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("库里没有挂着设备 / 阀门的 CFLOOR 楼板"))?,
        )?;
        let mut r = SUL_DB
            .query(format!(
                "SELECT VALUE in FROM nearest_relate WHERE \
                 type::thing('pe', record::id(out)).owner = {} AND in.deleted != true;",
                root.to_pe_key()
            ))
            .await?
            .check()?;
        let dependents = r
            .take::<Vec<surrealdb::sql::Thing>>(0)?
            .into_iter()
            .map(pe_thing_to_refno)
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?
            .into_iter()
            .collect::<Vec<_>>();

        let first = update_cal_equip_for_roots(&[root]).await?;
        eprintln!(
            "根 {}（{} 个挂在其楼板上）→ {first:?}",
            root.to_pe_key(),
            dependents.len()
        );
        assert_eq!(
            first.via_floors,
            dependents.len(),
            "挂在这些楼板上的都要拉进来"
        );
        assert_eq!(
            first.refreshed, 0,
            "根下没有设备，牵连的只重连、不写 cal_equi"
        );
        assert_eq!(first.failed, 0);
        assert!(!first.floors_skipped);
        let edges = floor_edges(&dependents).await?;
        assert!(
            edges.len() >= dependents.len(),
            "现有边牵连的应全部还挂着楼板边（空间反查只会多拉）: {} < {}",
            edges.len(),
            dependents.len()
        );

        let second = update_cal_equip_for_roots(&[root]).await?;
        assert_eq!(floor_edges(&dependents).await?, edges, "重跑不能堆边或换边");
        assert_eq!(second.failed, 0);
        Ok(())
    }

    /// 楼板新位置正上方、原来没挂这块楼板（这里模拟成「没有边」）的设备，要靠空间反查拉进来补边。
    #[tokio::test]
    #[ignore = "live: 需要真库 8009；会装载空间树并重写一组楼板上方的 nearest_relate；必须单跑"]
    async fn floor_space_finds_equips_that_did_not_link_the_floor() -> anyhow::Result<()> {
        attach_store_connected().await?;
        crate::fast_model::aabb_tree::load_project_tree_verified().await?;

        // 挑一台挂在 CFLOOR 楼板上的设备，记下它的根与边，然后删边：此时现有边反查找不到它。
        let mut r = SUL_DB
            .query(
                "SELECT in AS equip, type::thing('pe', record::id(out)).owner AS root FROM nearest_relate \
                 WHERE in.noun = 'EQUI' AND in.deleted != true \
                 AND type::thing('pe', record::id(out)).owner.noun = 'CFLOOR' LIMIT 1;",
            )
            .await?
            .check()?;
        #[derive(serde::Deserialize)]
        struct Pick {
            equip: surrealdb::sql::Thing,
            root: surrealdb::sql::Thing,
        }
        let pick: Option<Pick> = r.take(0)?;
        let pick = pick.ok_or_else(|| anyhow::anyhow!("库里没有挂在 CFLOOR 楼板上的设备"))?;
        let equip = pe_thing_to_refno(pick.equip)?;
        let root = pe_thing_to_refno(pick.root)?;
        let before = floor_edges(&[equip]).await?;
        SUL_DB
            .query(format!("DELETE {}->nearest_relate;", equip.to_pe_key()))
            .await?
            .check()?;

        let started = std::time::Instant::now();
        let outcome = update_cal_equip_for_roots(&[root]).await?;
        eprintln!(
            "根 {} 设备 {} → {outcome:?}，{}ms",
            root.to_pe_key(),
            equip.to_pe_key(),
            started.elapsed().as_millis()
        );
        assert!(
            outcome.via_floor_space >= 1,
            "删了边的设备只能靠空间反查拉进来"
        );
        assert_eq!(outcome.failed, 0);
        assert_eq!(
            floor_edges(&[equip]).await?,
            before,
            "补回的边应与删之前相同"
        );

        update_cal_equip_for_roots(&[root]).await?;
        assert_eq!(floor_edges(&[equip]).await?, before, "重跑不能堆边");
        Ok(())
    }

    /// 根本身是设备时要把它自己算进去；`cal_equi` 已有旧值也必须被覆盖；树没装时旧楼板边原样保留。
    #[tokio::test]
    #[ignore = "live: 需要真库 8009；会改写一台设备的 cal_equi；必须单跑（进程里不能装过空间树）"]
    async fn equip_root_overwrites_stale_world_trans_and_keeps_edges_without_tree()
    -> anyhow::Result<()> {
        attach_store_connected().await?;
        assert!(
            !spatial_state::current_state().is_ready(),
            "本测试要求进程里没装过空间树，实际 {}",
            spatial_state::current_state().as_str()
        );

        let mut r = SUL_DB
            .query(
                "SELECT VALUE in FROM nearest_relate WHERE in.noun = 'EQUI' AND in.deleted != true LIMIT 1;",
            )
            .await?
            .check()?;
        let found: Vec<surrealdb::sql::Thing> = r.take(0)?;
        let equip = pe_thing_to_refno(
            found
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("库里没有挂着楼板边的 EQUI"))?,
        )?;
        let edges_before = floor_edges(&[equip]).await?;

        let cal_key = equip.to_table_key("cal_equi");
        SUL_DB
            .query(format!(
                "UPSERT {cal_key} SET world_trans = trans:⟨stale_sentinel⟩;"
            ))
            .await?
            .check()?;

        let outcome = update_cal_equip_for_roots(&[equip]).await?;
        eprintln!("根 {}（本身是设备）→ {outcome:?}", equip.to_pe_key());
        assert_eq!(outcome.refreshed, 1, "根本身是设备时要把它自己算进去");
        assert!(outcome.floors_skipped, "树没就绪必须跳过楼板边");

        let mut r = SUL_DB
            .query(format!("RETURN record::id({cal_key}.world_trans);"))
            .await?
            .check()?;
        let trans_id: Option<String> = r.take(0)?;
        let expected = gen_bytes_hash::<_, 64>(
            &aios_core::get_world_transform(equip)
                .await?
                .unwrap_or_default(),
        )
        .to_string();
        assert_eq!(
            trans_id.as_deref(),
            Some(expected.as_str()),
            "cal_equi 必须被覆盖成当前 world transform（CREATE 撞已有 id 会静默失败）"
        );
        assert_eq!(
            floor_edges(&[equip]).await?,
            edges_before,
            "树没就绪时旧楼板边必须原样保留"
        );
        Ok(())
    }
}
