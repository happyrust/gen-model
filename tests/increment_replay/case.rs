use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::harness::{cases_dir, repo};

#[derive(Debug, Deserialize, Serialize)]
pub struct Case {
    pub name: String,
    #[serde(default)]
    pub registry: Vec<String>,
    pub dbnum: u32,
    /// 相对影子工程根的 DESI 文件路径。
    pub db_file: String,
    /// 终稿来源：zip 里的一个条目（issue 夹具）或 `aios-session-fixture-v1` 目录。
    #[serde(default)]
    pub source_zip: Option<SourceZip>,
    #[serde(default)]
    pub fixture_dir: Option<String>,
    /// 终稿来源的第三种：本机上的一份库文件（相对仓根，或绝对路径），直接当 `--source` 用。
    /// 新录的链先这样跑、验过再决定值不值得打成夹具：库文件只追加，录完之后任何时刻拷下来的那一份
    /// 都是之前每一段窗口的终稿。不带字节补丁，所以「保存中途被看见」那一类（`race`）不要用它。
    #[serde(default)]
    pub source_file: Option<String>,
    /// 被回放的库不在缺省工程（AMS，MDB `/ALL`）里时写这一节：派生配置按它改工程名、工程号、MDB，
    /// 影子工程根也要是以这个工程为主造出来的那一份（`AIOS_REPLAY_PROJECT_ROOT` 指过去）。见 [`ProjectSpec`]。
    #[serde(default)]
    pub project: Option<ProjectSpec>,
    pub start: u32,
    /// 多给服务进程的环境变量（如测试钩 `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY`）。
    #[serde(default)]
    pub service_env: BTreeMap<String, String>,
    /// 初始化到 S 之后先核一遍（`to` 填 S；这是让「有行」那半边的查询先被正面踩过一次）。
    #[serde(default)]
    pub init: Option<Step>,
    pub steps: Vec<Step>,
    /// 断言全过之后，对这条运行目录跑 `scripts/increment_budget.py collect --check`
    /// （INC-I01 / I02 / I04 / I05）。只开在基线覆盖的链上；失败注入的链样本形状不同，不进这道门。
    #[serde(default)]
    pub budget_gate: bool,
    /// `probe_execute` 里那条「`unselected` 必须非空」的断言：只有工程里存在**第二个 DESI** 时才该打开。
    /// 影子工程根目前只有被回放的那一个 DESI，非 DESI 一律不受勾选约束（`subset_selects`），
    /// 所以 db8000-two-delete 上 unselected 合法为空（2026-10-27 实测）。INC-H03 有了两库夹具再开。
    #[serde(default)]
    pub expect_unselected_non_empty: bool,
    /// 链跑完、还没杀进程时打一枪 `POST /update/execute`（只勾选本库）并核对 `/health`
    /// 的监听范围与启动复核字段（INC-J01 / J02）。不在失败 park 的链上打：人工执行会清掉连败账。
    #[serde(default)]
    pub probe_execute: bool,
    /// INC-G01（RI-12）：链跑完之后，这些根名下的 `room_relate` 边集合必须与**同数据的整库全量重建**之后相等。
    /// 做法：先按根把库上的边读成集合，`POST /api/v1/dbnums/{dbnum}/model/rebuild`（ModelOnly、起点 0）等任务落终态，
    /// 再读一遍逐元素对拍。代价是一次整库重排，所以只开在专门的链上，别挂到失败注入的链。
    #[serde(default)]
    pub rebuild_edges_equal: Vec<String>,
    /// 只动方位的便宜路（刚体前移，ADR-066）的对拍：链跑完之后，这些根的 `/model/records` 必须与同数据整库
    /// 全量重建之后逐条相等（数字差不超过 `rebuild_records_tolerance`）——便宜路左乘 Δ 出来的矩阵与盒，要和从
    /// 文件整根重算出来的是同一份。与 `rebuild_edges_equal` 共用同一次 `model/rebuild`。
    #[serde(default)]
    pub rebuild_records_equal: Vec<String>,
    /// `rebuild_records_equal` 的数字容差，口径同冷重启后补常驻重算的那一档（盒是 f32，重算从网格取顶点）。
    #[serde(default = "default_reprime_tolerance")]
    pub rebuild_records_tolerance: f64,
    /// 这条用例的监听限定（`watch_dbnums`）。空 = 只监听 `dbnum`（其余链都是这样）。
    /// INC-E02 / E03 要的正是「域内 / 域外」这一对对照：同一个落后的目录落款，在限定域内
    /// 排一条只盖落款的空任务，在域外只点名、不排工。
    #[serde(default)]
    pub watch_dbnums: Vec<u32>,
    /// 初始化断言过完之后，直接对本次运行的 surreal 发一串 SurrealQL。
    ///
    /// 只给「回放链本身造不出来的前置状态」用：库级目录落款（`dbnum_watermark.model_catalogue_sessions`）
    /// 只有真跑过整库生成才会写，而回放链只动一个 DESI。INC-E02 / E03 靠它把某个库的落款
    /// 钉成旧会话，再去看看核对跑出来说什么。
    #[serde(default)]
    pub seed: Vec<String>,
    /// 规格已钉在 harness、产品侧还没修的用例：默认**跳过**，只有 `AIOS_REPLAY_INCLUDE_KNOWN_RED` 非空
    /// 或 `AIOS_REPLAY_CASE` 点名它时才跑（整档是一个 `#[test]`，一条 panic 会把后面全部用例一起带走）。
    /// 放行判据见 `known_red_allowed`；理由写在用例自己的 `_doc` 里。
    #[serde(default)]
    pub known_red: bool,
    /// 伙伴库（见 [`Companion`]）：自动并进监听名单，init 时各自 replay-init 并等三水位。
    /// 空 = 单库用例（现有八条都是），整条行为与加这一格之前相同。
    #[serde(default)]
    pub companions: Vec<Companion>,
    /// 派生配置里要改的键（值是非负整数）。只放行数据面事务上限那三个：`data_tx_max_bytes`、
    /// `data_tx_max_write_rows`、`data_tx_elements`（`harness::CONFIG_OVERRIDE_KEYS`）。
    /// **带着它过的用例不算在产品缺省配置下过**：用它是为了回答「是不是只卡在这个上限上」，
    /// 用例的 `_doc` 里要写明为什么调、缺省值下是什么表现。
    #[serde(default)]
    pub config_overrides: BTreeMap<String, Value>,
    /// 用例文件里回放程序不认识的键（带路径），[`load_cases`] 填。不认识的键不参与判定——字段名拼错、
    /// 用了还没实现的断言，都会落在这里；跑这条用例时先把它们打出来，`AIOS_REPLAY_STRICT_KEYS=1` 时直接判红。
    #[serde(skip)]
    pub unknown_keys: Vec<String>,
}

/// 用例所在的工程（缺省不写 = 仓根配置里的那一个，AMS）。
///
/// `name` 是工程目录名（`AvevaPlantSample`），`code` 是工程号（APS 是 2501），`mdb` 不带斜杠（`ALL`、`SDNF`）。
/// 后端要打开哪些工程目录由 MDB 的声明派生（ADR-078），这里不用写。
#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectSpec {
    /// 工程目录名，也是派生配置里的 `project_name`（例：`AvevaPlantSample`）。
    pub name: String,
    /// 工程码（`project_code`，库的命名空间也用它；APS 是 2501）。
    pub code: u32,
    /// MDB 名，不带斜杠；不写是 `ALL`。
    #[serde(default = "default_mdb")]
    pub mdb: String,
    /// 元件库的查找次序；不写是「本工程、AvevaCatalogue」。
    #[serde(default)]
    pub catalogue_project_priority: Vec<String>,
}

fn default_mdb() -> String {
    "ALL".to_string()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SourceZip {
    pub path: String,
    pub entry: String,
}

/// 同一个服务里一起监听、各自回放的伙伴库（INC-H03「一库 park 另一库前进」）。
#[derive(Debug, Deserialize, Serialize)]
pub struct Companion {
    pub dbnum: u32,
    /// 相对影子工程根的 DESI 文件路径（同 `Case::db_file`）。影子根要把它列进 `-Replayed`。
    pub db_file: String,
    /// `aios-session-fixture-v1` 目录；伙伴库不接 `source_zip`。与 `source_file` 二选一。
    #[serde(default)]
    pub fixture_dir: Option<String>,
    /// 本机上的一份终稿（相对仓根或绝对路径），同 `Case::source_file`。
    #[serde(default)]
    pub source_file: Option<String>,
    /// 初始化到的会话。
    pub start: u32,
}

/// 「保存中途被看见」（INC-H01）：推到 `to` 后，等监听器把 `to` 的批次跑起来，再把 `next` 按「先页后头」两半写进去；
/// `to` 那一窗必须 failed（消息含 `message_contains`）并被 `to..=next` 的后继窗 supersede，步的其余断言落在后继窗上。
/// 服务要带测试钩 `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY="<to>:<ms>"` 把冻结与复核之间拉开（`service_env`）。
#[derive(Debug, Deserialize, Serialize)]
pub struct Race {
    pub next: u32,
    #[serde(default = "default_anchor_message")]
    pub message_contains: String,
}

fn default_anchor_message() -> String {
    "anchor 已改变".to_string()
}

/// 冷重启对拍（INC-F02）：这一步的断言过了之后，先记下这些根的 `/model/records`（此时还是进程投影在供数），杀服务重拉，
/// 三水位必须还在 `to`（不重新初始化），库上行供出来的 `/model/records` 与重启前逐字段相等（数字容差 `tolerance`）；
/// 再把根叫回投影（首次 `/model/generate` 走「补常驻根」重算），重算后的行与重启前在 `reprime_tolerance` 内相等（ADR-066
/// 「刚体前移 == 重算」那把尺）。
#[derive(Debug, Deserialize, Serialize)]
pub struct Restart {
    pub records_equal: Vec<String>,
    #[serde(default = "default_restart_tolerance")]
    pub tolerance: f64,
    #[serde(default = "default_reprime_tolerance")]
    pub reprime_tolerance: f64,
}

fn default_restart_tolerance() -> f64 {
    1e-3
}

fn default_reprime_tolerance() -> f64 {
    0.5
}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Step {
    pub to: u32,
    /// 见 [`Race`]。
    #[serde(default)]
    pub race: Option<Race>,
    /// 见 [`Restart`]。
    #[serde(default)]
    pub restart: Option<Restart>,
    /// 这一步断言过了之后，把这些根的 `/model/records` 全部行按 `(to, root)` 存下，给后面的 `records_equal_to_step` 对拍。
    /// 写在 `init` 里也行（按用例的 `start` 存）；根可以是伙伴库里的。
    #[serde(default)]
    pub stash_records: Vec<String>,
    /// 这一步的 `/model/records` 要与早先在此 sesno 存下的行相等（数字容差 `records_equal_tolerance`）——回退 → 整库重建后的库
    /// 必须和第一次走到这个会话时一样（INC-B06 / H04，O2）。
    #[serde(default)]
    pub records_equal_to_step: Option<u32>,
    #[serde(default = "default_reprime_tolerance")]
    pub records_equal_tolerance: f64,
    /// 这一步的任务 `intent`（如 `reinitialize`）与 `reinit_trigger`（如 `rollback`）。
    #[serde(default)]
    pub expect_intent: Option<String>,
    #[serde(default)]
    pub expect_reinit_trigger: Option<String>,
    /// 等三水位的上限（秒；缺省 180，整库重建那一步要给多）。
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub half: Option<String>,
    #[serde(default)]
    pub rollback: bool,
    /// 这一步不等水位（半截后的中间态）。
    #[serde(default)]
    pub no_wait: bool,
    /// 与 `Case::known_red` 同一个规矩（`Step` 上也有这一格，是为了在步一级写「这一步属于已知红的规格」时
    /// 不用把整条用例一起挂起）。默认**跳过**——但注意：`Step` 一级的跳过会中断本条用例剩下的断言，
    /// 所以现在只有 `Case` 一级在用。把「规格先写、产品没修」的那几条挂在这里，nightly 照常绿，修好了删标记即可。
    #[serde(default)]
    pub known_red: bool,
    /// INC-E03（GitHub #67）：等 `/health.catalogue_reconcile.behind_out_of_scope_libraries` 出现这些
    /// 库号，并核这一次对账**没有出声**——域外落后库在安静那一拍不打印（附一句的前提是本来就要出声）。
    #[serde(default)]
    pub expect_catalogue_out_of_scope_behind: Vec<u32>,
    /// INC-E02：等 `/health.catalogue_reconcile.catalogue_cascade.stamp_only` 出现这些库号，然后核
    /// 那条「只盖落款」的空任务真的落了终态、落款被盖上，**下一拍照旧安静**（不再点名）。
    #[serde(default)]
    pub expect_catalogue_stamp_only: Vec<u32>,
    /// 失败注入（INC-F05 / H05，计划 §6 D6）：非空 → **先把服务带 `AIOS_TEST_FAIL_ROOTS=<名单>` 冷重启**，再推这一窗；
    /// 名单里的根由 `model_refresh::injected_generation_failures` 记成生成失败、不进生成器，其余根照常生成。
    /// 钩子必须在文件前移之前挂上，否则这一窗早就跑完了。
    #[serde(default)]
    pub fail_roots: Vec<String>,
    /// 清钩子重启（与 `fail_roots` 反向）：去掉 `AIOS_TEST_FAIL_ROOTS` 再冷重启，证明「库健康后下一任务再试」真的会重跑（INC-H05）。
    #[serde(default)]
    pub clear_fail_injection: bool,
    /// 这一窗**预期失败**：设了就**不等 `wait_in_sync`**（三水位本就不该追上来），改为等任务落终态，再判
    /// `model_sesno` 仍停在这个 sesno（生成失败不推模型水位，INC-F05）。`applied_sesno` 与 `file_latest_sesno`
    /// 只打进日志不判——数据半边是成功的，「数据硬失败不推 applied」要另一种注入。
    #[serde(default)]
    pub expect_model_sesno_stay: Option<u32>,
    /// 这一窗预期数据批次失败一次后停自动重跑并 park（INC-H02）。不等三水位。
    /// 服务要带 `AIOS_TEST_FAIL_DATA_ENDS="<dbnum>:<to>"`，并把对账 / 限频 / 轮询间隔调短，
    /// 否则「本轮不自动重跑」要等到下一轮对账。判：`/health` 的 `batch_failures[dbnum]` 已 `parked` 且
    /// `end_sesno == to`、`streak >= 1`；接着日志里出现「本轮不自动重跑」，而 streak 不再涨、
    /// `applied_sesno` 仍停在 `to` 之前。
    #[serde(default)]
    pub expect_batch_parked: bool,
    /// 这一步追平之后，该 dbnum 的连败账必须已经清掉（新会话把右端推过 park 点，INC-H02 的复活半边）。
    #[serde(default)]
    pub expect_batch_failure_cleared: bool,
    /// 设了就在这一步的重启（若有）之后核 INC-F03：`/tasks` 仍能按右端找到这条数据批次，
    /// `?before=` 不再含它且能翻出更早的一页，`/tasks/{id}/changes` 的每条都有 `op` / `refno`，
    /// 并且至少一条的 `root_refno` 是这个根。放在带 `restart` 的那一步上，才是「历史跨重启」。
    #[serde(default)]
    pub history_root: Option<String>,
    #[serde(default)]
    pub records_present: Vec<String>,
    #[serde(default)]
    pub records_absent: Vec<String>,
    /// 根 refno → 期望的 `ModelUnitResult.kind`（regen / transform / unit）。
    #[serde(default)]
    pub unit_kinds: BTreeMap<String, String>,
    /// 先把这些根叫进进程投影：冷初始化 / 重启后投影里没有回执，第一次 `/model/generate` 走「补常驻根」回 `Generated`
    /// （`on_demand_model::regenerate_scope`）；紧接着第二次必须 `AlreadyAvailable`，证明常驻成了。后续窗口的
    /// `generate_already_available` 才有意义（「仍」AlreadyAvailable，INC-D01 / D04），位移走 transform 也要根已在投影（INC-C03）。
    #[serde(default)]
    pub generate_prime: Vec<String>,
    /// 这些根 `/model/generate` 必须 `AlreadyAvailable`。
    #[serde(default)]
    pub generate_already_available: Vec<String>,
    /// 根 refno → 期望的 `geometry_diff.moved`（单元回执，INC-F04）。
    #[serde(default)]
    pub geometry_moved: BTreeMap<String, u64>,
    /// 这些根的 `rooms_before == rooms_after`（房内小位移，INC-G02）。
    #[serde(default)]
    pub rooms_unchanged: Vec<String>,
    /// 根 refno → 期望的房间号集合（只比 `room_num`，不比面板和成员数）。
    /// 跨房位移（INC-G02）：732 从 R224 换成 R124/R144/R413，734 再换回来。
    #[serde(default)]
    pub rooms_changed: BTreeMap<String, RoomChange>,
    /// 根 refno → 库上 `room_relate` 边的期望（INC-G02 的 O2 半边）。边按**边目标**读
    /// （`type::thing('pe', …)<-room_relate`，与 `fn::room_relate_of` 同一写法；#66：按 `out =` 过滤够不着
    /// `unique_room_relate (in, out)` 前缀，整表扫）。跨房位移的「库上旧房边消失」只有这里看得见，
    /// 回执的 `rooms_after` 只是 O3 那一半。
    #[serde(default)]
    pub room_edges: BTreeMap<String, RoomEdgeExpect>,
    /// 根 refno → 期望的整根并集包围盒世界平移 `[dx, dy, dz]`（mm）：`geometry_after.aabb` 的 min / max 都要比
    /// `geometry_before.aabb` 正好差这个向量（容差 [`AABB_TOLERANCE_MM`]），且 `records` 前后相同（刚体位移不增减行，
    /// INC-C03 / INC-F04）。
    #[serde(default)]
    pub aabb_shift: BTreeMap<String, [f32; 3]>,
    /// `generate_already_available` 里每一根 `/model/generate` 往返的上限（毫秒，INC-D01）；缺省只记不判。
    #[serde(default)]
    pub generate_budget_ms: Option<u64>,
    /// 到这一步为止服务 stdout 里不得出现的子串（如 `[模型目标核对]`：未触根的回执被判过期或核对不了，INC-D01）。
    #[serde(default)]
    pub log_must_not_contain: Vec<String>,
    /// 这一步的任务终态（缺省 succeeded）。
    #[serde(default)]
    pub task_state: Option<String>,
    /// 这一窗的交付单元数（`result.units.len()`）：只记账的会话（DBMDAT）必须是 0（INC-A06）。
    #[serde(default)]
    pub unit_count: Option<usize>,
    /// 先把这些伙伴库推到给定会话（dbnum → to），再推主库。
    #[serde(default)]
    pub companion_to: BTreeMap<u32, u32>,
    /// 等这些库三水位都到给定会话（dbnum → sesno；主库或伙伴库），复用 `Service::wait_in_sync`。
    #[serde(default)]
    pub expect_in_sync: BTreeMap<u32, u32>,
    /// 主库这一步不动：不推文件、不等主库水位、不跑主库断言。配 `expect_batch_parked` 时只复核
    /// 主库仍 park，`to` 填主库当前的 park 右端。
    #[serde(default)]
    pub hold_primary: bool,
    /// 推这一步的文件**之前**，先对本次运行的 surreal 发一串 SurrealQL（同 `Case::seed`，只是挂在步上）。
    /// 给「链走到中间才出现的东西」造前置状态用：交付删除记录只对已发布的单元写，而先建后删的那台设备
    /// 是中间某一步才有的，它的 `datacenter_version` 行得在删它的那一步之前补上。
    #[serde(default)]
    pub seed: Vec<String>,
    /// 这些根**不在**这一窗的交付单元里（对照：隔壁的根一点没碰）。`generate_already_available` 只能证
    /// 它的回执没过期，证不了「没被白算一遍」。
    #[serde(default)]
    pub units_absent: Vec<String>,
    /// 任务回执 `result.warnings` 拼起来必须含这些子串。
    #[serde(default)]
    pub warnings_contain: Vec<String>,
    /// 任务回执 `result.warnings` 拼起来不得含这些子串。
    #[serde(default, alias = "warnings_not_contain")]
    pub warnings_absent: Vec<String>,
    /// `/health` 里任意一格的期望（见 [`HealthExpect`]），在这一步的时限内轮询到全部满足为止。
    /// 主库不动的步（`hold_primary`）上也跑：退避、对账这类「不推文件，只等后端自己动」的步靠它判。
    #[serde(default)]
    pub expect_health: Vec<HealthExpect>,
    /// 这一步开始之后的服务日志里必须出现这些子串（在这一步的时限内等）。`hold_primary` 的步上也跑。
    #[serde(default)]
    pub log_must_contain: Vec<String>,
    /// 后端停着的时候推文件：先停服务，推这一步的文件，再用同一套环境变量拉起来，然后照常等水位、跑断言。
    /// 「停机期间有人保存，启动之后自己补」就是这个形状。
    #[serde(default)]
    pub push_while_down: bool,
    /// 一窗做到一半杀掉后端再起：推文件 → 等这一步开始之后的日志里出现这一句 → 杀进程 → 拉起 → 照常等水位、跑断言。
    /// 「一半」的位置用延时钩撑开（`service_env` 里的 `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY` 之类）。
    #[serde(default)]
    pub kill_on_log: Option<String>,
    /// 窗口规划那一行（`window_root_plan::plan_totals_line`，在 `result.warnings` 里）按字段比，只比写了的键：
    /// `candidates` / `added` / `modified` / `deleted` / `rolled_up` / `unchanged` / `no_model` / `metadata_only` /
    /// `unread_attribute` / `cache_stamp` / `unresolved` / `cascades` / `regen` / `remove` / `derived`。
    /// 会话号每次录都不同，这一行按子串比不了，所以拆成数再比。
    #[serde(default)]
    pub plan_totals: BTreeMap<String, u64>,
    /// 根 refno → 单元回执里几何差与行数的期望（只比写了的格）。
    #[serde(default)]
    pub geometry_counts: BTreeMap<String, GeometryCounts>,
    /// 根 refno → 这一窗几何有变化的构件（单元回执的 `geometry_diff.sample`），`a/b` 或 `a_b` 都行。
    /// 样本没被截断时（条数等于动了 + 多了 + 少了）要求**正好**是这几只；截断了只要求列出的都在。
    #[serde(default)]
    pub geometry_sample: BTreeMap<String, Vec<String>>,
    /// 这一步之后**别的库**上因它多出来的任务（跨库交接过去的那一条），或者「不该多出任务」的对照。见 [`OtherDbTaskExpect`]。
    #[serde(default)]
    pub companion_tasks: Vec<OtherDbTaskExpect>,
    /// 交付删除记录（`GET /datacenter/deletes`，本库、`sesno >= start`）的期望，逐条。
    #[serde(default)]
    pub datacenter_deletes: Vec<DeleteExpect>,
    /// 这一步之后，库里的生成结果要对得上 E3D 在这个状态导出的 RVM（见 [`RvmCompare`]）。
    #[serde(default)]
    pub rvm_compare: Option<RvmCompare>,
}

/// `/health` 里一格的期望。`path` 是 JSON 指针（如 `/model_root_backoff/backing_off`）。四种判法写一种：
/// `eq`（值相等；两边都是数时按数比，`1` 与 `1.0` 算相等）、`ge`（是数且不小于）、`absent`（没有这一格，或是 `null`）、
/// `present`（有这一格且不是 `null`）。一种都不写等于 `present`。
#[derive(Debug, Deserialize, Serialize)]
pub struct HealthExpect {
    pub path: String,
    #[serde(default)]
    pub eq: Option<serde_json::Value>,
    #[serde(default)]
    pub ge: Option<f64>,
    #[serde(default)]
    pub absent: bool,
    #[serde(default)]
    pub present: bool,
}

/// 主库这一窗做完之后，另一个库（伙伴库）上应当出现、或不应当出现的任务。
///
/// 用在跨库交接上（gen-model #143：管道库里被支撑的管件换了规格 → 支撑库收到一条只带根名单的任务）。
/// 判法：推文件之前先记下这个库已有的任务号；主库断言过完之后，等一条**新的**任务落终态，再按下面的格子比。
/// 只比写了的格。
#[derive(Debug, Deserialize, Serialize)]
pub struct OtherDbTaskExpect {
    /// 哪个库：要在监听范围里（`companions` 里的库，或写在 `watch_dbnums` 里的）。
    pub dbnum: u32,
    /// `true` = 对照：这一步之后这个库上**不该**多出任务。观察 `timeout_secs` 秒（缺省 15）。
    #[serde(default)]
    pub absent: bool,
    /// 等新任务落终态的上限（秒），缺省 180；`absent` 时是观察多久，缺省 15。
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// 期望的终态，缺省 `succeeded`。
    #[serde(default)]
    pub task_state: Option<String>,
    /// 任务 `detail.intent` 里要含的子串（不分大小写）。写了它，只有含这个子串的新任务才算数。
    #[serde(default)]
    pub intent_contains: Option<String>,
    /// 交付单元的根**正好**是这几个（不多不少）。
    #[serde(default)]
    pub units_exactly: Option<Vec<String>>,
    /// 根 → `kind`（同步上的 `unit_kinds`）。
    #[serde(default)]
    pub unit_kinds: BTreeMap<String, String>,
    /// 根 → 包围盒整体平移 `[dx, dy, dz]`（毫米；min 角、max 角都要平移这么多）。
    #[serde(default)]
    pub aabb_shift: BTreeMap<String, [f32; 3]>,
    /// 根 → 包围盒 min 角 / max 角各自的位移（件自己的尺寸也变了的时候，两个角挪得不一样）。
    #[serde(default)]
    pub aabb_min_shift: BTreeMap<String, [f32; 3]>,
    #[serde(default)]
    pub aabb_max_shift: BTreeMap<String, [f32; 3]>,
    /// 上面三格的容差（毫米），缺省 1.0。
    #[serde(default)]
    pub aabb_tolerance: Option<f32>,
    /// 根 → 几何差与行数（同步上的 `geometry_counts`）。
    #[serde(default)]
    pub geometry_counts: BTreeMap<String, GeometryCounts>,
    /// 回执 `warnings` 里要含 / 不该含的子串。
    #[serde(default)]
    pub warnings_contain: Vec<String>,
    #[serde(default)]
    pub warnings_absent: Vec<String>,
    /// 这条任务落终态之后，早先在这个会话（`init` 是用例的 `start`，步是它的 `to`）用 `stash_records` 存下的根，
    /// 现在的 `/model/records` 要和那时逐条相等——「改了又改回去，那边的件也回到原样」。
    #[serde(default)]
    pub records_equal_to_step: Option<u32>,
    /// 上一格的数字容差，缺省同 `records_equal_tolerance`。
    #[serde(default)]
    pub records_equal_tolerance: Option<f64>,
}

/// 「改后的模型对上 E3D 导出的」：`snapshot` 是 `rvm_verify import` 出的快照 JSON（跑宏入口的导出步会顺手出），
/// `rvm` 是它对应的 RVM 原件（给了才做表面距离那一层）。路径相对仓根或绝对。判定交给 `rvm_verify compare`。
#[derive(Debug, Deserialize, Serialize)]
pub struct RvmCompare {
    pub snapshot: String,
    #[serde(default)]
    pub rvm: Option<String>,
}

/// 单元回执里的几何差与行数：`geometry_diff.{moved,added,removed}`、`geometry_before.records`、`geometry_after.records`。
/// 不写的格不比。成员删后同名重建应是行数不变、`removed 1 added 1`。
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct GeometryCounts {
    #[serde(default)]
    pub moved: Option<u64>,
    #[serde(default)]
    pub added: Option<u64>,
    #[serde(default)]
    pub removed: Option<u64>,
    #[serde(default)]
    pub records_before: Option<u64>,
    #[serde(default)]
    pub records_after: Option<u64>,
}

/// 一条交付删除记录的期望。`present = false`：这个 refno 一条都不该有（成员的增删改名、改名又改回并成一窗）。
/// `present = true`（缺省）：至少有一条对得上写了的那几格——`name` 是要在中台删掉的位号，`reason` 是
/// `deleted` / `renamed`，`sesno` 是变化所在的那个会话（不是窗口右端）。
#[derive(Debug, Deserialize, Serialize)]
pub struct DeleteExpect {
    pub refno: String,
    #[serde(default = "default_true")]
    pub present: bool,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub sesno: Option<u32>,
}

fn default_true() -> bool {
    true
}

/// 一次跨房：开工前、刷盘确认后各有哪些房间号。
#[derive(Debug, Deserialize, Serialize)]
pub struct RoomChange {
    pub before: Vec<String>,
    pub after: Vec<String>,
}

/// 库上 `room_relate` 边的期望（INC-G02 的 O2 半边）。只比房间号：`present` 必须都在、`absent` 必须都不在；
/// 多出来的房间号不管——回执里 R144 / R413 这类同名不同前缀的房间会随轮次漂，钉死会把用例变成噪声。
#[derive(Debug, Deserialize, Serialize)]
pub struct RoomEdgeExpect {
    #[serde(default)]
    pub present: Vec<String>,
    #[serde(default)]
    pub absent: Vec<String>,
}

pub fn companion_of<'a>(case: &'a Case, dbnum: u32) -> &'a Companion {
    case.companions
        .iter()
        .find(|companion| companion.dbnum == dbnum)
        .unwrap_or_else(|| {
            panic!(
                "用例 {} 的步里点了伙伴库 {dbnum}，但 companions 里没有它",
                case.name
            )
        })
}

/// 伙伴库的终稿来源：本机的一份库文件（`--source`），或夹具目录（`--fixture`）。
pub fn companion_args(companion: &Companion) -> Vec<String> {
    if let Some(file) = &companion.source_file {
        return vec![
            "--source".into(),
            repo().join(file).to_string_lossy().into_owned(),
        ];
    }
    let dir = companion.fixture_dir.as_ref().unwrap_or_else(|| {
        panic!(
            "伙伴库 {} 没有终稿来源（fixture_dir / source_file 二选一）",
            companion.dbnum
        )
    });
    vec![
        "--fixture".into(),
        repo().join(dir).to_string_lossy().into_owned(),
    ]
}

/// 监听名单：用例写了就用它，没写就是主库；伙伴库总在里面。
pub fn watch_list(case: &Case) -> Vec<u32> {
    let mut watch = if case.watch_dbnums.is_empty() {
        vec![case.dbnum]
    } else {
        case.watch_dbnums.clone()
    };
    for companion in &case.companions {
        if !watch.contains(&companion.dbnum) {
            watch.push(companion.dbnum);
        }
    }
    watch
}

/// 伙伴库的声明与步上的引用对得上：库号不重复、不等于主库；`companion_to` 只点伙伴库，
/// `expect_in_sync` 只点主库或伙伴库。
pub fn check_companions(case: &Case) {
    let mut declared = BTreeSet::new();
    for companion in &case.companions {
        assert!(
            companion.dbnum != case.dbnum && declared.insert(companion.dbnum),
            "用例 {} 的伙伴库 {} 与主库或别的伙伴库重复",
            case.name,
            companion.dbnum
        );
    }
    for step in case.init.iter().chain(&case.steps) {
        for dbnum in step.companion_to.keys() {
            assert!(
                declared.contains(dbnum),
                "用例 {} 的 companion_to 点了未声明的伙伴库 {dbnum}",
                case.name
            );
        }
        for expect in &step.companion_tasks {
            assert!(
                expect.dbnum != case.dbnum
                    && (declared.contains(&expect.dbnum)
                        || case.watch_dbnums.contains(&expect.dbnum)),
                "用例 {} 的 companion_tasks 点了 {}：它得是主库之外、在监听范围里的库（companions 或 watch_dbnums）",
                case.name,
                expect.dbnum
            );
        }
        for dbnum in step.expect_in_sync.keys() {
            assert!(
                *dbnum == case.dbnum || declared.contains(dbnum),
                "用例 {} 的 expect_in_sync 点了既不是主库也不是伙伴库的 {dbnum}",
                case.name
            );
        }
    }
}

/// 起别名的键：用例里写别名也算认识。
const KEY_ALIASES: &[(&str, &str)] = &[("warnings_not_contain", "warnings_absent")];

/// 用例 JSON（`original`）里有、解出来的结构（`known`，再序列化回 JSON）里没有的键，带路径。
/// `_` 开头的是注释，不算；值是映射的那些格（`unit_kinds`、`plan_totals` 等）键原样往返，不会被误报。
pub fn unknown_keys(original: &Value, known: &Value, path: &str, out: &mut Vec<String>) {
    match (original, known) {
        (Value::Object(have), Value::Object(want)) => {
            for (key, value) in have {
                if key.starts_with('_') {
                    continue;
                }
                let here = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let real = KEY_ALIASES
                    .iter()
                    .find(|(alias, _)| alias == key)
                    .map_or(key.as_str(), |(_, real)| *real);
                match want.get(real) {
                    Some(inner) => unknown_keys(value, inner, &here, out),
                    None => out.push(here),
                }
            }
        }
        (Value::Array(have), Value::Array(want)) => {
            for (index, (item, inner)) in have.iter().zip(want).enumerate() {
                unknown_keys(item, inner, &format!("{path}[{index}]"), out);
            }
        }
        _ => {}
    }
}

pub fn load_cases() -> Vec<Case> {
    let dir = cases_dir();
    let mut cases = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    entries.sort();
    for path in entries {
        let text = fs::read_to_string(&path).expect("read case");
        let mut case: Case = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("用例 {} 解不开：{e}", path.display()));
        check_companions(&case);
        let original: Value = serde_json::from_str(&text).expect("刚解过一遍");
        let known = serde_json::to_value(&case).expect("用例结构能序列化");
        unknown_keys(&original, &known, "", &mut case.unknown_keys);
        case.unknown_keys.sort();
        if case.known_red && !known_red_allowed(&case.name) {
            eprintln!(
                "== 跳过用例 {}：规格已钉、产品未修（`known_red`）。要看真红：set AIOS_REPLAY_INCLUDE_KNOWN_RED=1，\
                 或 set AIOS_REPLAY_CASE={}",
                case.name, case.name
            );
            continue;
        }
        cases.push(case);
    }
    cases
}

/// `known_red` 用例的放行判据：`AIOS_REPLAY_INCLUDE_KNOWN_RED` 非空（`0` / 空串除外），
/// 或者 `AIOS_REPLAY_CASE` 正好点名它。
pub fn known_red_allowed(name: &str) -> bool {
    let include = std::env::var("AIOS_REPLAY_INCLUDE_KNOWN_RED").unwrap_or_default();
    if !matches!(include.trim(), "" | "0" | "false") {
        return true;
    }
    std::env::var("AIOS_REPLAY_CASE")
        .map(|case| case == name)
        .unwrap_or(false)
}
