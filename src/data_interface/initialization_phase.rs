//! Strict data-initialization ordering (ADR-025), amended by ADR-058 D7: the
//! data chain (Meta → Design) and the model chain run in parallel.  The model
//! gate opens when the manifest is installed (file identity resolved) instead
//! of when every data phase has settled; only the convergence banner
//! (`mark_model_ready`) and room post-processing still wait for `data_ready`.
//!
//! The queue itself is deliberately non-durable.  This coordinator therefore
//! keeps only reconstructible process state; watermarks and durable model work
//! remain the recovery authorities across restarts.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::Serialize;
use tokio::time::{Duration, sleep};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataPhase {
    Meta,
    Catalogue,
    Design,
}

impl DataPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Meta => "meta",
            Self::Catalogue => "catalogue",
            Self::Design => "design",
        }
    }

    pub fn of_db_type(db_type: &str) -> Self {
        match db_type.trim().to_ascii_uppercase().as_str() {
            "SYST" | "DICT" | "COMM" | "MISC" | "GLB" | "GLOB" => Self::Meta,
            "CATA" => Self::Catalogue,
            _ => Self::Design,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitializationStatus {
    Discovering,
    AwaitingTrigger,
    Running,
    Blocked,
    DataReady,
    ModelRunning,
    ModelReady,
}

impl InitializationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovering => "discovering",
            Self::AwaitingTrigger => "awaiting_trigger",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::DataReady => "data_ready",
            Self::ModelRunning => "model_running",
            Self::ModelReady => "model_ready",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PhaseCounts {
    pub total: usize,
    pub pending: usize,
    pub running: usize,
    pub failed: usize,
    pub blocked: usize,
}

/// 一条阶段阻断，带上「是谁挡的」。
///
/// 过去它是 `(DataPhase, String)`：相知道，dbnum 从来没进去过，对外快照连相也压掉
/// 了。2026-08-27 现场那条 DESI 任务因此只看得见「被 meta 挡着」，看不见挡它的是
/// 8191——meta 库不止一个，人得自己回水位表里挨个找哪个没追平（ISSUE-025 §三）。
///
/// 记 blocker 的那几处本来就握着 dbnum，只是在拼字符串时把它拌进去了。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhaseBlocker {
    pub phase: DataPhase,
    pub dbnum: Option<u32>,
    pub project: Option<String>,
    /// 记下这条阻断的批次任务。扫描期的阻断（目录不可读、清单歧义）没有任务。
    pub task_id: Option<String>,
    /// 去 `/api/v1/batch-failures` 取全文原因的键。只有确实落过一条失败记录才给
    /// ——指向一个不存在的记录比不给更费人时间。
    pub reason_ref: Option<String>,
    pub message: String,
}

impl PhaseBlocker {
    pub fn new(phase: DataPhase, message: impl Into<String>) -> Self {
        Self {
            phase,
            dbnum: None,
            project: None,
            task_id: None,
            reason_ref: None,
            message: message.into(),
        }
    }

    pub fn with_dbnum(mut self, dbnum: u32) -> Self {
        self.dbnum = Some(dbnum);
        self
    }

    pub fn with_project(mut self, project: impl Into<String>) -> Self {
        self.project = Some(project.into());
        self
    }

    pub fn with_task(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    /// 这条阻断的全文原因已经落进 `logs/batch-failures-*.jsonl`，键是同一个 task_id。
    pub fn with_failure_record(mut self, task_id: impl Into<String>) -> Self {
        self.reason_ref = Some(task_id.into());
        self
    }

    /// 兼容旧消费者的那一行字符串。形状不许变：`/health` 的 `blockers` 与
    /// `InitializationNotReady` 的错误串都念它。
    pub fn rendered(&self) -> String {
        format!("{}: {}", self.phase.as_str(), self.message)
    }
}

/// 本次启动核对了多大一片模型覆盖面（ADR-058 S2）。
///
/// 这些数此前只在控制台印一行「模型凭证核对完成：…」，滚过去就没了——而「模型面
/// 为什么还没追平」恰恰要回头看那一行。
///
/// 与 `/dbnums` 上那两列凭证的分工：这里是**启动那一刻的快照**，那边是活值。启动
/// 之后被按需 ensure 过的库会从 `no_credential_dbnums` 里「消失」而这份快照不变，
/// 两者可以合法地不同，界面上分开说。
#[derive(Debug, Clone, Serialize)]
pub struct StartupCoverage {
    /// `watch` = 本进程被监听限定域收窄，只核对了名单内的库；`all` = 全范围。
    pub scope: &'static str,
    /// 真正逐库核对过的库数（有凭证记录、且在本进程范围内）。
    pub in_scope: usize,
    /// `gen_root` 里有凭证记录的库数，不分范围。
    pub candidates: usize,
    /// 启动那一刻登记了、却一条凭证记录都没有的库（ADR-058 D6：这些根等按需
    /// ensure，启动一根不排）。
    ///
    /// `null` = 这次连登记表都没读到，说不出有没有——它与空列表（每个登记库都有
    /// 凭证）是两句话，合成一句就是拿「不知道」冒充绿灯。
    pub no_credential_dbnums: Option<Vec<u32>>,
    pub reconcile: StartupReconcile,
}

/// 启动复核的结果。[`StartupCoverage`] 的一格。
#[derive(Debug, Clone, Serialize)]
pub struct StartupReconcile {
    /// `pending` 还没落地 · `ok` 逐库核完 · `floors_failed` 连凭证下限都没读到
    /// （覆盖状态整体未知）· `unreadable` 有库取不到文件最新会话 · `window_failed`
    /// 有库开了退化窗口却没收口。
    ///
    /// 几种同时成立时取最重的那个（`floors_failed` > `window_failed` >
    /// `unreadable` > `ok`）：这一格是给人决定「要不要动手」的，不是统计。
    pub result: &'static str,
    /// 取不到文件最新会话、按「未确认」跳过的库数。
    pub unreadable: usize,
    /// 开了退化窗口但收口失败的库数。人工出口是
    /// `POST /api/v1/dbnums/{dbnum}/model/rebuild`。
    pub window_failed: usize,
    /// 一级过滤判定落后、需要开窗的库数。
    pub lagging: usize,
    /// 真正开成了退化窗口的库数（`lagging - window_failed`）。
    pub windows_opened: usize,
    /// 目录前移了、而反查**没能精确回答**（2026-09-06 计划 P3 护栏，ADR-058 实施约束 5）：
    /// 某个过期目录库的窗口收不了 / 是导入级窗口 / 变动件数超阈 / 零反向表且变动在目录内部层，
    /// 或整个反查没跑成。为真时那些库的**依赖根全部**按过期重排（反查跑成的情形），或一根
    /// 都没排、只剩按需 `ensure` 的目录指纹失配兜底（反查没跑成）；哪一种看 `catalogue_cascade`。
    ///
    /// 目录前移但反查精确命中、只排了引用它的根——**不算**退化，这一位为假。
    /// 2026-09-04 到 09-06 P2 期间它曾是「目录前移了、启动不为此排任何工作」的点名位。
    pub catalogue_cascade_degraded: bool,
    /// 具体是哪几个 CATA 库前移了（升序）。它说的是「有没有根落在旧目录上」，与
    /// `catalogue_cascade_degraded` 是两件事：前移且精确反查了的库也在这里。
    pub stale_catalogue_dbnums: Vec<u32>,
    /// 记录在案、却在当前 MDB 里读不到最新会话的 CATA 库数。它与「对不上」是两件事：
    /// 这一栏是**不知道**（换过 MDB、文件不在了），不是「已确认过期」。
    pub unreadable_catalogue_dbnums: usize,
    /// `stale_catalogue_dbnums` 里每个库一条：`记在案的最旧会话 → 文件最新` 这一窗变了
    /// 多少目录件、什么 noun、该库有没有反向引用表可供反查（2026-09-06 计划 P2，
    /// P3 反查的输入）。收不了的窗口带 `error`，那一窗是**未知**不是零。
    pub catalogue_windows: Vec<crate::data_interface::cata_cascade_window::CatalogueWindowReport>,
    /// 对上面那些窗口做的反查与排工（2026-09-06 计划 P3）：变动目录件 → 目录侧闭包 →
    /// 设计侧引用者 → 上卷到根 → 落款仍在旧目录上的根排 `RegenRoot`；退化库的依赖根全部重排。
    /// `null` = 没有过期目录库，反查没跑。带 `error` = 跑了没跑成，一根都没排（未知，不是零）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalogue_cascade: Option<crate::data_interface::reverse_reference::CatalogueCascadeReport>,
}

impl Default for StartupReconcile {
    fn default() -> Self {
        Self {
            result: "pending",
            unreadable: 0,
            window_failed: 0,
            lagging: 0,
            windows_opened: 0,
            catalogue_cascade_degraded: false,
            stale_catalogue_dbnums: Vec::new(),
            unreadable_catalogue_dbnums: 0,
            catalogue_windows: Vec::new(),
            catalogue_cascade: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InitializationSnapshot {
    pub epoch_id: u64,
    pub status: &'static str,
    pub current_phase: Option<&'static str>,
    pub data_ready: bool,
    pub model_ready: bool,
    pub model_in_flight: bool,
    /// 模型写入准入门（ADR-058 D7）。`false` = 清单还没装好，或本次要求全量模型前置
    /// ——模型链此刻一根都不许生成。
    ///
    /// 它与 `data_ready` 分家之后，「模型面为什么不动」有了两个完全不同的答案：门
    /// 关着（等清单）与门开着但没活。少了这一位，外面只能拿 `data_ready` 猜，而
    /// ADR-058 D7 之后那个猜法恒错——两条链现在是并行的。
    pub model_phase_open: bool,
    /// 启动模型凭证复核（ADR-058 D4）还没落地：为真时 `model_ready` 不可能置位。
    pub startup_reconcile_pending: bool,
    /// 本次启动的模型覆盖面。`null` = 本进程没跑过启动复核（python 绑定、测试与
    /// 不经 `run_cli` 的入口都没有这一步），不是「覆盖为空」。
    pub startup: Option<StartupCoverage>,
    pub phases: BTreeMap<&'static str, PhaseCounts>,
    /// 渲染好的那份，形状与结构化之前一字不差。
    pub blockers: Vec<String>,
    /// 同一批阻断的结构化形态：相 + dbnum + 项目 + 任务 + 失败记录键。
    pub blocked_by: Vec<PhaseBlocker>,
    pub shadowed: Vec<String>,
}

#[derive(Debug, Clone)]
struct CoordinatorState {
    epoch_id: u64,
    status: InitializationStatus,
    current_phase: Option<DataPhase>,
    data_ready: bool,
    model_ready: bool,
    full_model_required: bool,
    /// 模型门。ADR-058 D7：它只等「文件身份解析完成」（清单装好），**不等**数据相
    /// 清零——`begin_discovery` 关、`install_manifest` 开（`full_model_required` 除外）。
    /// 数据链与模型链从此并行；仍然排在数据之后的只剩两处：`mark_model_ready`
    /// （收敛宣告要 `data_ready`，因为数据批次的尾事务还会往模型积压里添行）与
    /// `postprocess_allowed`（房间吃模型产物）。
    model_phase_open: bool,
    model_in_flight: bool,
    /// 服务启动的模型凭证复核（`reconcile_model_coverage_at_startup`）尚未落地。
    ///
    /// 清单一就绪模型门就自动开，worker 空闲轮随即可能把 `model_ready` 置位——而此刻
    /// 启动复核还在逐库读文件头，它随后入队的启动窗口工作项不会再把 `model_ready`
    /// 打回去。于是「持久模型工作单已收敛」会在积压刚被重新排出来的那一刻打出来。
    /// 这一位让 `model_ready` 在复核落地前不可能为真。默认 false：python 绑定、测试
    /// 与不经 `run_cli` 的入口没有这一步，不该被它拦住。
    startup_reconcile_pending: bool,
    /// 本次启动核对到的覆盖面。进程级事实，**不随 `begin_discovery` 清**：重扫开
    /// 一个新 epoch，但启动那一刻核了多少库不会因此改变。
    startup_coverage: Option<StartupCoverage>,
    phases: BTreeMap<DataPhase, PhaseCounts>,
    blockers: Vec<PhaseBlocker>,
    shadowed: Vec<String>,
    needs_rescan: bool,
}

impl Default for CoordinatorState {
    fn default() -> Self {
        Self {
            epoch_id: 0,
            status: InitializationStatus::Discovering,
            current_phase: None,
            data_ready: false,
            model_ready: false,
            full_model_required: false,
            model_phase_open: true,
            model_in_flight: false,
            startup_reconcile_pending: false,
            startup_coverage: None,
            phases: BTreeMap::new(),
            blockers: Vec::new(),
            shadowed: Vec::new(),
            needs_rescan: false,
        }
    }
}

pub struct InitializationCoordinator {
    state: Mutex<CoordinatorState>,
}

static COORDINATOR: OnceLock<InitializationCoordinator> = OnceLock::new();

impl InitializationCoordinator {
    pub fn global() -> &'static Self {
        COORDINATOR.get_or_init(|| Self {
            state: Mutex::new(CoordinatorState::default()),
        })
    }

    fn state(&self) -> MutexGuard<'_, CoordinatorState> {
        self.state.lock().unwrap_or_else(|poisoned| {
            log::error!("初始化阶段锁曾因 panic 中毒，已恢复继续使用");
            poisoned.into_inner()
        })
    }

    /// Start a complete observation pass.  No model write is admitted until
    /// the resulting manifest is installed and all of its data rows settle.
    pub fn begin_discovery(&self) -> u64 {
        let mut state = self.state();
        state.epoch_id = state.epoch_id.saturating_add(1);
        state.status = InitializationStatus::Discovering;
        state.current_phase = None;
        state.data_ready = false;
        state.model_ready = false;
        state.model_phase_open = false;
        state.phases.clear();
        state.blockers.clear();
        state.shadowed.clear();
        state.needs_rescan = false;
        state.epoch_id
    }

    pub fn install_manifest(
        &self,
        epoch_id: u64,
        phases: impl IntoIterator<Item = DataPhase>,
        held: bool,
        blockers: impl IntoIterator<Item = PhaseBlocker>,
    ) -> bool {
        let mut state = self.state();
        if epoch_id != state.epoch_id {
            return false;
        }
        state.phases.clear();
        for phase in phases {
            let counts = state.phases.entry(phase).or_default();
            counts.total += 1;
            counts.pending += 1;
        }
        state.blockers = blockers.into_iter().collect();
        state.needs_rescan = false;
        let blocked_phases = state
            .blockers
            .iter()
            .map(|blocker| blocker.phase)
            .collect::<Vec<_>>();
        for phase in blocked_phases {
            state.phases.entry(phase).or_default().blocked += 1;
        }
        state.current_phase = state
            .blockers
            .iter()
            .map(|blocker| blocker.phase)
            .chain(state.phases.keys().copied())
            .min();
        state.data_ready = state.current_phase.is_none();
        state.model_ready = false;
        // 清单装好 = 文件身份解析完成，模型面即刻开工（ADR-058 D7）；此前这里还要
        // `data_ready`，那是「生成器要读 pe」时代的前置，ADR-056 N7 之后已不成立。
        state.model_phase_open = !state.full_model_required;
        let current_is_blocked = state.current_phase.is_some_and(|current| {
            state
                .blockers
                .iter()
                .any(|blocker| blocker.phase == current)
        });
        state.status = if current_is_blocked {
            InitializationStatus::Blocked
        } else if state.data_ready {
            InitializationStatus::DataReady
        } else if held {
            InitializationStatus::AwaitingTrigger
        } else {
            InitializationStatus::Running
        };
        true
    }

    pub fn arm(&self) {
        let mut state = self.state();
        if state.status == InitializationStatus::AwaitingTrigger {
            state.status = InitializationStatus::Running;
        }
    }

    pub fn allows(&self, phase: DataPhase, epoch_id: u64) -> bool {
        // Legacy/direct test callers that do not participate in an installed
        // manifest remain executable; production discovery always uses a
        // positive epoch.
        if epoch_id == 0 {
            return true;
        }
        let state = self.state();
        epoch_id == state.epoch_id
            && state.status == InitializationStatus::Running
            && state.current_phase == Some(phase)
            && !state.model_in_flight
            && !state.blockers.iter().any(|blocker| blocker.phase == phase)
    }

    /// Recompute the active barrier from the queue after a row settles.  The
    /// caller supplies only rows from the current manifest.
    pub fn reconcile_pending(
        &self,
        epoch_id: u64,
        pending: impl IntoIterator<Item = (DataPhase, bool)>,
    ) {
        let mut state = self.state();
        if epoch_id != state.epoch_id {
            return;
        }
        for counts in state.phases.values_mut() {
            counts.pending = 0;
            counts.running = 0;
        }
        for (phase, running) in pending {
            let counts = state.phases.entry(phase).or_default();
            if running {
                counts.running += 1;
            } else {
                counts.pending += 1;
            }
        }
        let previous_phase = state.current_phase;
        state.current_phase = state
            .phases
            .iter()
            .find_map(|(phase, counts)| ((counts.pending + counts.running) > 0).then_some(*phase));
        state.current_phase = state
            .current_phase
            .into_iter()
            .chain(state.blockers.iter().map(|blocker| blocker.phase))
            .min();
        if state.current_phase != previous_phase {
            // A barrier transition is not complete merely because the current
            // in-memory rows drained.  The authoritative file snapshot must be
            // rebuilt before a later phase (or models) can run.
            state.status = InitializationStatus::Discovering;
            state.data_ready = false;
            state.needs_rescan = true;
        } else if state.current_phase.is_some_and(|current| {
            state
                .blockers
                .iter()
                .any(|blocker| blocker.phase == current)
        }) {
            state.status = InitializationStatus::Blocked;
            state.data_ready = false;
        }
    }

    pub fn take_rescan_requested(&self) -> bool {
        let mut state = self.state();
        std::mem::take(&mut state.needs_rescan)
    }

    /// 相取自 `blocker` 自己：一条阻断的相与它的肇事者是同一件事的两半，分两个
    /// 参数传就允许它们对不上。
    pub fn mark_failed(&self, epoch_id: u64, blocker: PhaseBlocker) {
        let mut state = self.state();
        if epoch_id != state.epoch_id {
            return;
        }
        let phase = blocker.phase;
        state.status = InitializationStatus::Blocked;
        state.current_phase = Some(phase);
        state.data_ready = false;
        state.model_ready = false;
        state.phases.entry(phase).or_default().failed += 1;
        state.blockers.push(blocker);
    }

    pub fn set_shadowed(&self, epoch_id: u64, shadowed: Vec<String>) {
        let mut state = self.state();
        if epoch_id == state.epoch_id {
            state.shadowed = shadowed;
        }
    }

    pub fn set_phase_totals(&self, epoch_id: u64, phases: impl IntoIterator<Item = DataPhase>) {
        let mut state = self.state();
        if epoch_id != state.epoch_id {
            return;
        }
        let mut totals = BTreeMap::new();
        for phase in phases {
            *totals.entry(phase).or_insert(0usize) += 1;
        }
        for (phase, total) in totals {
            state.phases.entry(phase).or_default().total = total;
        }
    }

    pub fn data_ready(&self) -> bool {
        self.state().data_ready
    }

    /// Configure whether startup must finish a configured full-model pass
    /// before durable/on-demand model generation may begin.  This is process
    /// policy, so it survives manifest replacement within the same startup.
    pub fn configure_model_bootstrap(&self, required: bool) {
        let mut state = self.state();
        state.full_model_required = required;
        // 门的另一半「文件身份已解析」由 `install_manifest` / `mark_data_ready_without_manifest`
        // 负责；这里只按策略关门，或在清单已装好时开门。发现期（含 epoch 0 的初态）
        // 不开：epoch 0 靠 `model_generation_allowed` 的旁路，清单期等 `install_manifest`。
        if required {
            state.model_phase_open = false;
        } else if state.status != InitializationStatus::Discovering {
            state.model_phase_open = true;
        }
        state.model_ready = false;
    }

    /// Full/manual synchronization has already awaited its three global data
    /// phases before the incremental manager is created, so it has no manifest
    /// rows to reconstruct.  Record that fact explicitly instead of relying on
    /// the legacy epoch-zero bypass for model writes.
    pub fn mark_data_ready_without_manifest(&self) {
        let mut state = self.state();
        if state.epoch_id == 0 {
            state.data_ready = true;
            state.status = InitializationStatus::DataReady;
            state.model_phase_open = !state.full_model_required;
        }
    }

    /// Wait for the installed data manifest to settle.  A blocked phase stays
    /// observable through health while the watcher is alive; a later complete
    /// scan installs a new epoch and releases this wait.
    pub async fn wait_for_data_ready(&self) {
        loop {
            if self.data_ready() {
                return;
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    pub async fn wait_for_model_ready(&self) {
        loop {
            if self.state().model_ready {
                return;
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    /// Release durable model work after the configured startup-wide full model
    /// pass.  For deployments without that pass, the manifest opens this gate
    /// automatically as soon as it is installed (ADR-058 D7: file identity
    /// resolved, not data settled), so this explicit call is a no-op there.
    /// It still requires `data_ready`: it also moves `status` to `ModelRunning`,
    /// and `allows()` would then refuse every data phase still pending.
    pub fn open_model_phase(&self) -> bool {
        let mut state = self.state();
        if !state.data_ready || state.model_in_flight {
            return false;
        }
        state.model_phase_open = true;
        state.status = InitializationStatus::ModelRunning;
        true
    }

    pub fn begin_full_model(&self) -> bool {
        let mut state = self.state();
        if !state.data_ready || state.model_in_flight {
            return false;
        }
        state.model_in_flight = true;
        state.status = InitializationStatus::ModelRunning;
        state.model_ready = false;
        true
    }

    pub fn end_full_model(&self) {
        let mut state = self.state();
        state.model_in_flight = false;
        if state.data_ready {
            state.status = InitializationStatus::DataReady;
        }
    }

    /// 模型写入准入。ADR-058 D7：不看 `data_ready`——数据链（Meta → Design 摄入）与
    /// 模型链并行，模型面只等清单装好（`model_phase_open`）。仍然挡它的只有进行中的
    /// 全量模型（`model_in_flight`）与启动策略要求的全量前置（`full_model_required`）。
    pub fn model_generation_allowed(&self) -> bool {
        let state = self.state();
        !state.model_in_flight
            && ((state.epoch_id == 0 && !state.full_model_required) || state.model_phase_open)
    }

    /// Room/spatial post-processing is one barrier later than model writes.
    /// Epoch zero preserves the standalone Python/full-sync compatibility path,
    /// which does not install an incremental manifest.
    pub fn postprocess_allowed(&self) -> bool {
        let state = self.state();
        state.epoch_id == 0 || (state.data_ready && state.model_ready)
    }

    pub fn mark_model_running(&self) -> bool {
        let mut state = self.state();
        if !state.data_ready || !state.model_phase_open {
            return false;
        }
        state.status = InitializationStatus::ModelRunning;
        state.model_ready = false;
        true
    }

    pub fn mark_model_ready(&self) -> bool {
        let mut state = self.state();
        if state.data_ready && state.model_phase_open && !state.startup_reconcile_pending {
            let changed = !state.model_ready;
            state.status = InitializationStatus::ModelReady;
            state.model_ready = true;
            changed
        } else {
            false
        }
    }

    /// 服务启动宣告「模型凭证复核还没跑完」：从此到 [`Self::settle_startup_reconcile`]
    /// 之间 `model_ready` 置不上，worker 空闲轮照常消化积压，只是不能宣告收敛。
    /// 必须在 worker 起来之前调，否则第一圈空闲轮就可能抢先置位。
    pub fn begin_startup_reconcile(&self) {
        let mut state = self.state();
        state.startup_reconcile_pending = true;
        state.model_ready = false;
        // 上一份覆盖面在这一刻作废：复核重跑期间摆着上一轮的数，比什么都不摆更糟。
        state.startup_coverage = None;
    }

    /// 落地本次启动核到的覆盖面。与 [`Self::settle_startup_reconcile`] 分开两步：
    /// 落地覆盖面是「核出了什么」，settle 是「已经排过了」——后者哪条出口都得走到，
    /// 前者只有复核函数自己给得出。
    pub fn record_startup_coverage(&self, coverage: StartupCoverage) {
        self.state().startup_coverage = Some(coverage);
    }

    /// 启动复核落地（成功、取数失败、无凭证记录都算落地——它是「已经排过了」，不是
    /// 「排成功了」）。不调这一下 `model_ready` 永远起不来，所以复核函数的每条出口都
    /// 得走到它。
    pub fn settle_startup_reconcile(&self) {
        let mut state = self.state();
        state.startup_reconcile_pending = false;
    }

    pub fn startup_reconcile_pending(&self) -> bool {
        self.state().startup_reconcile_pending
    }

    pub fn snapshot(&self) -> InitializationSnapshot {
        let state = self.state();
        InitializationSnapshot {
            epoch_id: state.epoch_id,
            status: state.status.as_str(),
            current_phase: state.current_phase.map(DataPhase::as_str),
            data_ready: state.data_ready,
            model_ready: state.model_ready,
            model_in_flight: state.model_in_flight,
            model_phase_open: state.model_phase_open,
            startup_reconcile_pending: state.startup_reconcile_pending,
            startup: state.startup_coverage.clone(),
            phases: state
                .phases
                .iter()
                .map(|(phase, counts)| (phase.as_str(), counts.clone()))
                .collect(),
            blockers: state.blockers.iter().map(PhaseBlocker::rendered).collect(),
            blocked_by: state.blockers.clone(),
            shadowed: state.shadowed.clone(),
        }
    }
}

/// Typed model-write rejection shared by REST, Python and background drains.
#[derive(Debug, Clone)]
pub struct InitializationNotReady {
    pub snapshot: InitializationSnapshot,
}

impl std::fmt::Display for InitializationNotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "initialization_not_ready: status={} epoch={} current_phase={} blockers={}",
            self.snapshot.status,
            self.snapshot.epoch_id,
            self.snapshot.current_phase.unwrap_or("none"),
            self.snapshot.blockers.join(" | ")
        )
    }
}

impl std::error::Error for InitializationNotReady {}

pub fn require_model_generation() -> Result<(), InitializationNotReady> {
    let coordinator = InitializationCoordinator::global();
    if coordinator.model_generation_allowed() {
        Ok(())
    } else {
        Err(InitializationNotReady {
            snapshot: coordinator.snapshot(),
        })
    }
}

pub fn require_postprocess() -> Result<(), InitializationNotReady> {
    let coordinator = InitializationCoordinator::global();
    if coordinator.postprocess_allowed() {
        Ok(())
    } else {
        Err(InitializationNotReady {
            snapshot: coordinator.snapshot(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueCandidate {
    pub project: String,
    pub dbnum: u32,
    pub path: PathBuf,
}

/// [`select_catalogue_candidates`] 的一条阻断。
///
/// 它跨 DICT（meta 相）与 CATA（catalogue 相）复用，所以自己不知道相；相由调用方
/// 在 [`Self::into_phase`] 时补上。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogueBlocker {
    pub dbnum: Option<u32>,
    pub project: Option<String>,
    pub message: String,
}

impl CatalogueBlocker {
    fn new(message: impl Into<String>) -> Self {
        Self {
            dbnum: None,
            project: None,
            message: message.into(),
        }
    }

    fn with_dbnum(mut self, dbnum: u32) -> Self {
        self.dbnum = Some(dbnum);
        self
    }

    fn with_project(mut self, project: impl Into<String>) -> Self {
        self.project = Some(project.into());
        self
    }

    pub fn into_phase(self, phase: DataPhase) -> PhaseBlocker {
        PhaseBlocker {
            phase,
            dbnum: self.dbnum,
            project: self.project,
            task_id: None,
            reason_ref: None,
            message: self.message,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogueSelection {
    pub selected: Vec<CatalogueCandidate>,
    pub shadowed: Vec<CatalogueCandidate>,
    pub blockers: Vec<CatalogueBlocker>,
}

/// Resolve the repository's naked-dbnum limitation before any observation is
/// written.  Same-project duplicates always block; cross-project collisions are
/// decided by project rank.
///
/// Rank is `catalogue_project_priority` first, then every remaining
/// `included_projects` entry in the order it is written.  The explicit list is
/// an override layer, not the whole ranking: leaving a project out of it means
/// "no opinion", not "unrankable".  `versioned_db::database` already builds its
/// full-sync project order exactly this way, and the config changelog has
/// documented this key's default as "same order as `included_projects`" since
/// it was introduced.  While this function alone treated an unlisted project as
/// unrankable, one name missing from the list blocked the whole Catalogue phase
/// — and with it every Design batch behind the barrier.
pub fn select_catalogue_candidates(
    candidates: impl IntoIterator<Item = CatalogueCandidate>,
    included_projects: &[String],
    priority: &[String],
) -> CatalogueSelection {
    let included: HashSet<String> = included_projects
        .iter()
        .map(|project| project.trim().to_ascii_lowercase())
        .collect();
    let mut rank = HashMap::new();
    let mut blockers = Vec::new();
    for (index, project) in priority.iter().enumerate() {
        let key = project.trim().to_ascii_lowercase();
        if key.is_empty() || !included.contains(&key) {
            blockers.push(
                CatalogueBlocker::new(format!("catalogue_project_priority 含未知项目 {project:?}"))
                    .with_project(project.clone()),
            );
            continue;
        }
        if rank.insert(key.clone(), index).is_some() {
            blockers.push(
                CatalogueBlocker::new(format!("catalogue_project_priority 重复项目 {project:?}"))
                    .with_project(project.clone()),
            );
        }
    }
    // Offsets start past the explicit list so a named project always outranks an
    // unnamed one; `or_insert` keeps the first occurrence when the list repeats
    // a name.
    for (offset, project) in included_projects.iter().enumerate() {
        let key = project.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        rank.entry(key).or_insert(priority.len() + offset);
    }

    let mut by_dbnum: BTreeMap<u32, Vec<CatalogueCandidate>> = BTreeMap::new();
    let mut seen_project_dbnum: HashMap<(String, u32), PathBuf> = HashMap::new();
    let mut extract_shadowed = Vec::new();
    let included_candidates: Vec<CatalogueCandidate> = candidates
        .into_iter()
        .filter_map(|candidate| {
            let project_key = candidate.project.trim().to_ascii_lowercase();
            if !included.contains(&project_key) {
                blockers.push(
                    CatalogueBlocker::new(format!(
                        "元件库候选归属项目不在 included_projects: {} dbnum={}",
                        candidate.project, candidate.dbnum
                    ))
                    .with_dbnum(candidate.dbnum)
                    .with_project(candidate.project.clone()),
                );
                return None;
            }
            Some(candidate)
        })
        .collect();
    let collapsed = crate::data_interface::extract_family::collapse_extract_families(
        included_candidates.iter().map(|candidate| {
            (
                candidate.project.clone(),
                candidate.dbnum,
                candidate.path.clone(),
            )
        }),
    );
    for mismatch in &collapsed.mismatches {
        // 两个号本来就对不上，挂哪个都是挑边。挂 header：`collapse_extract_families`
        // 把它记进 `duplicate_keys` 用的就是 header，管线其余部分也按它认库。
        blockers.push(
            CatalogueBlocker::new(format!(
                "CATA/DICT 文件名库号与文件头不一致: {} filename={} header={}",
                mismatch.path.display(),
                mismatch.filename_dbnum,
                mismatch.header_dbnum
            ))
            .with_dbnum(mismatch.header_dbnum),
        );
    }
    let mut by_path: HashMap<PathBuf, CatalogueCandidate> = included_candidates
        .into_iter()
        .map(|candidate| (candidate.path.clone(), candidate))
        .collect();
    for parent in &collapsed.shadowed_parents {
        if let Some(candidate) = by_path.remove(parent) {
            extract_shadowed.push(candidate);
        }
    }
    for alias in &collapsed.shadowed_aliases {
        if let Some(candidate) = by_path.remove(alias) {
            extract_shadowed.push(candidate);
        }
    }
    for key in &collapsed.duplicate_keys {
        let paths: Vec<String> = by_path
            .values()
            .filter(|candidate| {
                candidate.dbnum == key.1 && candidate.project.trim().eq_ignore_ascii_case(&key.0)
            })
            .map(|candidate| candidate.path.display().to_string())
            .collect();
        blockers.push(
            CatalogueBlocker::new(format!(
                "项目 {} 内 CATA/DICT dbnum={} 有多个抽取或副本: {}",
                key.0,
                key.1,
                paths.join(" / ")
            ))
            .with_dbnum(key.1)
            .with_project(key.0.clone()),
        );
    }
    for sel in collapsed.selected {
        if collapsed
            .duplicate_keys
            .contains(&(sel.project.clone(), sel.dbnum))
        {
            continue;
        }
        let Some(candidate) = by_path.remove(&sel.leaf_path) else {
            continue;
        };
        let project_key = candidate.project.trim().to_ascii_lowercase();
        if let Some(previous) =
            seen_project_dbnum.insert((project_key, candidate.dbnum), candidate.path.clone())
        {
            blockers.push(
                CatalogueBlocker::new(format!(
                    "项目 {} 内 CATA/DICT dbnum={} 有多个文件: {} / {}",
                    candidate.project,
                    candidate.dbnum,
                    previous.display(),
                    candidate.path.display()
                ))
                .with_dbnum(candidate.dbnum)
                .with_project(candidate.project.clone()),
            );
        }
        by_dbnum.entry(candidate.dbnum).or_default().push(candidate);
    }

    let mut selected = Vec::new();
    let mut shadowed = Vec::new();
    for (dbnum, mut group) in by_dbnum {
        if group.len() == 1 {
            selected.push(group.pop().expect("one candidate"));
            continue;
        }
        group.sort_by_key(|candidate| {
            rank.get(&candidate.project.trim().to_ascii_lowercase())
                .copied()
                .unwrap_or(usize::MAX)
        });
        let Some(winner_rank) = rank
            .get(&group[0].project.trim().to_ascii_lowercase())
            .copied()
        else {
            blockers.push(
                CatalogueBlocker::new(format!(
                    "跨项目 CATA/DICT dbnum={dbnum} 冲突，但候选归属项目名为空，\
                     排不进 included_projects 顺序"
                ))
                .with_dbnum(dbnum),
            );
            continue;
        };
        let tied = group.iter().filter(|candidate| {
            rank.get(&candidate.project.trim().to_ascii_lowercase())
                .copied()
                == Some(winner_rank)
        });
        if tied.count() != 1 {
            blockers.push(
                CatalogueBlocker::new(format!("跨项目 CATA/DICT dbnum={dbnum} 优先级仍有歧义"))
                    .with_dbnum(dbnum),
            );
            continue;
        }
        selected.push(group.remove(0));
        shadowed.extend(group);
    }
    shadowed.extend(extract_shadowed);
    selected.sort_by_key(|candidate| candidate.dbnum);
    shadowed.sort_by_key(|candidate| candidate.dbnum);
    CatalogueSelection {
        selected,
        shadowed,
        blockers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coordinator() -> InitializationCoordinator {
        InitializationCoordinator {
            state: Mutex::new(CoordinatorState::default()),
        }
    }

    fn candidate(project: &str, dbnum: u32, path: &str) -> CatalogueCandidate {
        CatalogueCandidate {
            project: project.into(),
            dbnum,
            path: path.into(),
        }
    }

    #[test]
    fn phases_are_dependency_ordered() {
        assert!(DataPhase::Meta < DataPhase::Catalogue);
        assert!(DataPhase::Catalogue < DataPhase::Design);
        assert_eq!(DataPhase::of_db_type("DICT"), DataPhase::Meta);
        assert_eq!(DataPhase::of_db_type("COMM"), DataPhase::Meta);
        assert_eq!(DataPhase::of_db_type("MISC"), DataPhase::Meta);
        assert_eq!(DataPhase::of_db_type("CATA"), DataPhase::Catalogue);
        assert_eq!(DataPhase::of_db_type("DESI"), DataPhase::Design);
    }

    #[test]
    fn old_epoch_cannot_satisfy_new_manifest() {
        let coordinator = coordinator();
        let old = coordinator.begin_discovery();
        coordinator.install_manifest(old, [DataPhase::Meta], false, []);
        let new = coordinator.begin_discovery();
        coordinator.install_manifest(new, [DataPhase::Catalogue], false, []);
        coordinator.reconcile_pending(old, []);
        let snapshot = coordinator.snapshot();
        assert_eq!(snapshot.epoch_id, new);
        assert_eq!(snapshot.current_phase, Some("catalogue"));
        assert!(!snapshot.data_ready);
    }

    #[test]
    fn a_blocked_meta_phase_never_admits_design() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(
            epoch,
            [DataPhase::Design],
            false,
            [PhaseBlocker::new(DataPhase::Meta, "DICT unreadable")],
        );
        assert!(!coordinator.allows(DataPhase::Design, epoch));
        assert_eq!(coordinator.snapshot().current_phase, Some("meta"));
    }

    #[test]
    fn a_catalogue_blocker_does_not_prevent_meta_from_settling_first() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(
            epoch,
            [DataPhase::Meta, DataPhase::Catalogue, DataPhase::Design],
            false,
            [PhaseBlocker::new(DataPhase::Catalogue, "duplicate CATA")],
        );
        assert!(coordinator.allows(DataPhase::Meta, epoch));
        assert!(!coordinator.allows(DataPhase::Catalogue, epoch));
        assert!(!coordinator.allows(DataPhase::Design, epoch));
    }

    #[test]
    fn one_real_trigger_arms_the_whole_manifest() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(
            epoch,
            [DataPhase::Meta, DataPhase::Catalogue, DataPhase::Design],
            true,
            [],
        );
        assert!(!coordinator.allows(DataPhase::Meta, epoch));
        coordinator.arm();
        assert!(coordinator.allows(DataPhase::Meta, epoch));
    }

    #[test]
    fn configured_full_model_keeps_all_model_writes_closed_until_completion() {
        let coordinator = coordinator();
        coordinator.configure_model_bootstrap(true);
        coordinator.mark_data_ready_without_manifest();
        assert!(coordinator.data_ready());
        assert!(!coordinator.model_generation_allowed());
        assert!(!coordinator.mark_model_ready());
        assert!(!coordinator.snapshot().model_ready);

        assert!(coordinator.open_model_phase());
        assert!(coordinator.model_generation_allowed());
        assert!(coordinator.mark_model_ready());
        assert!(coordinator.snapshot().model_ready);
    }

    /// ADR-058 D7：模型门跟着清单开，不跟着数据相清零开。清单里还压着 Meta / Design
    /// 行时模型写入已准入；仍等数据的只有收敛宣告（`mark_model_ready`）与房间。
    #[test]
    fn model_gate_opens_with_the_manifest_not_with_data_ready() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        assert!(
            !coordinator.model_generation_allowed(),
            "发现期（身份还没解析完）模型门必须关着"
        );
        coordinator.install_manifest(epoch, [DataPhase::Meta, DataPhase::Design], false, []);
        assert!(!coordinator.data_ready());
        assert!(coordinator.allows(DataPhase::Meta, epoch), "数据链照常放行");
        assert!(
            coordinator.model_generation_allowed(),
            "清单装好即开模型门，不等 Meta → Design 摄完"
        );
        assert!(
            !coordinator.mark_model_ready(),
            "收敛宣告仍要 data_ready：数据尾事务还会往模型积压添行"
        );
        assert!(
            !coordinator.postprocess_allowed(),
            "房间仍排在数据与模型之后"
        );
        assert_eq!(coordinator.snapshot().status, "running");

        // 数据链全部落地后收敛才可宣告；模型门自始至终没有关过。
        let settled = coordinator.begin_discovery();
        coordinator.install_manifest(settled, [], false, []);
        assert!(coordinator.data_ready());
        assert!(coordinator.model_generation_allowed());
        assert!(coordinator.mark_model_ready());
        assert!(coordinator.postprocess_allowed());
    }

    /// 数据相被挡（头部不可读、同号重复）不是模型面的事：模型门保持打开，
    /// 只有收敛被打回。
    #[test]
    fn a_blocked_data_phase_does_not_close_the_model_gate() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(
            epoch,
            [DataPhase::Design],
            false,
            [PhaseBlocker::new(DataPhase::Meta, "DICT unreadable")],
        );
        assert_eq!(coordinator.snapshot().status, "blocked");
        assert!(!coordinator.data_ready());
        assert!(coordinator.model_generation_allowed());

        coordinator.mark_failed(
            epoch,
            PhaseBlocker::new(DataPhase::Design, "DESI header unreadable").with_dbnum(8191),
        );
        assert!(coordinator.model_generation_allowed());
        assert!(!coordinator.snapshot().model_ready);
        assert!(!coordinator.mark_model_ready());
    }

    /// 策略要求的全量前置仍是硬门：清单装好也不开，直到显式 `open_model_phase`。
    #[test]
    fn a_required_full_model_pass_still_closes_the_gate_after_the_manifest() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(epoch, [DataPhase::Meta], false, []);
        assert!(coordinator.model_generation_allowed());
        coordinator.configure_model_bootstrap(true);
        assert!(!coordinator.model_generation_allowed());
        let next = coordinator.begin_discovery();
        coordinator.install_manifest(next, [], false, []);
        assert!(coordinator.data_ready());
        assert!(!coordinator.model_generation_allowed());
        assert!(coordinator.open_model_phase());
        assert!(coordinator.model_generation_allowed());
    }

    #[test]
    fn model_ready_waits_for_the_startup_reconcile_to_settle() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(epoch, [], false, []);
        assert!(coordinator.data_ready());
        assert!(coordinator.model_generation_allowed());

        coordinator.begin_startup_reconcile();
        assert!(coordinator.startup_reconcile_pending());
        assert!(coordinator.model_generation_allowed());
        assert!(!coordinator.mark_model_ready());
        assert!(!coordinator.snapshot().model_ready);
        assert!(coordinator.snapshot().startup_reconcile_pending);

        coordinator.settle_startup_reconcile();
        assert!(!coordinator.startup_reconcile_pending());
        assert!(coordinator.mark_model_ready());
        assert!(coordinator.snapshot().model_ready);
        assert!(!coordinator.snapshot().startup_reconcile_pending);
    }

    #[test]
    fn beginning_the_startup_reconcile_retracts_an_early_model_ready() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(epoch, [], false, []);
        assert!(coordinator.mark_model_ready());

        coordinator.begin_startup_reconcile();
        assert!(!coordinator.snapshot().model_ready);
        assert!(!coordinator.postprocess_allowed());
    }

    #[test]
    fn postprocessing_waits_for_model_settlement() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(epoch, [], false, []);
        assert!(coordinator.data_ready());
        assert!(coordinator.model_generation_allowed());
        assert!(!coordinator.postprocess_allowed());

        assert!(coordinator.mark_model_ready());
        assert!(coordinator.postprocess_allowed());
    }

    #[test]
    fn earlier_data_epoch_waits_for_an_in_flight_full_model_to_settle() {
        let coordinator = coordinator();
        coordinator.mark_data_ready_without_manifest();
        assert!(coordinator.begin_full_model());
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(epoch, [DataPhase::Meta], false, []);
        assert!(!coordinator.allows(DataPhase::Meta, epoch));
        coordinator.end_full_model();
        assert!(coordinator.allows(DataPhase::Meta, epoch));
    }

    #[test]
    fn cross_project_collision_uses_explicit_priority() {
        let result = select_catalogue_candidates(
            [
                candidate("Main", 7000, "main7000"),
                candidate("Catalogue", 7000, "cat7000"),
                candidate("Catalogue", 7001, "cat7001"),
            ],
            &["Main".into(), "Catalogue".into()],
            &["Catalogue".into(), "Main".into()],
        );
        assert!(result.blockers.is_empty(), "{:?}", result.blockers);
        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected[0].project, "Catalogue");
        assert_eq!(result.shadowed, vec![candidate("Main", 7000, "main7000")]);
    }

    /// An unwritten priority list is "no opinion", not "unrankable": the naked
    /// dbnum tie-break has always been "whoever comes first in
    /// `included_projects`", which is what the full-sync path does too.
    /// Blocking instead turns one name missing from the config into a stalled
    /// Catalogue phase, and every Design batch waits behind that barrier.
    ///
    /// Candidates are handed in back-to-front on purpose: the winner must come
    /// from the configured project order, not from iteration order.
    #[test]
    fn cross_project_collision_falls_back_to_included_project_order() {
        let result = select_catalogue_candidates(
            [
                candidate("Catalogue", 7000, "cat7000"),
                candidate("Main", 7000, "main7000"),
            ],
            &["Main".into(), "Catalogue".into()],
            &[],
        );
        assert!(result.blockers.is_empty(), "{:?}", result.blockers);
        assert_eq!(result.selected, vec![candidate("Main", 7000, "main7000")]);
        assert_eq!(
            result.shadowed,
            vec![candidate("Catalogue", 7000, "cat7000")]
        );
    }

    /// The explicit list only reorders the projects it names; the rest keep
    /// their `included_projects` order behind them.  Both halves have to be
    /// live, or a half-written list silently becomes an all-or-nothing switch.
    #[test]
    fn a_partial_priority_list_ranks_the_rest_by_included_project_order() {
        let result = select_catalogue_candidates(
            [
                candidate("Main", 7000, "main7000"),
                candidate("Catalogue", 7000, "cat7000"),
                candidate("ZDJ", 7000, "zdj7000"),
                candidate("Catalogue", 7001, "cat7001"),
                candidate("Main", 7001, "main7001"),
            ],
            &["Main".into(), "Catalogue".into(), "ZDJ".into()],
            &["ZDJ".into()],
        );
        assert!(result.blockers.is_empty(), "{:?}", result.blockers);
        assert_eq!(
            result.selected,
            vec![
                // Named, so it beats Main even though Main leads the included list.
                candidate("ZDJ", 7000, "zdj7000"),
                // Neither is named, so the included order decides.
                candidate("Main", 7001, "main7001"),
            ]
        );
        assert_eq!(result.shadowed.len(), 3, "{:?}", result.shadowed);
    }

    /// A name that is not in `included_projects` still blocks.  It is a typo,
    /// not an omission, and the two want opposite handling — an omission now
    /// falls back to the included order, so the typo has to stay loud.
    #[test]
    fn a_priority_entry_outside_included_projects_still_blocks() {
        let result = select_catalogue_candidates(
            [candidate("Main", 7000, "main7000")],
            &["Main".into()],
            &["Typo".into()],
        );
        assert_eq!(result.blockers.len(), 1, "{:?}", result.blockers);
        assert!(
            result.blockers[0].message.contains("含未知项目"),
            "{:?}",
            result.blockers
        );
        assert_eq!(result.blockers[0].project.as_deref(), Some("Typo"));
    }

    #[test]
    fn same_project_extract_parent_and_leaf_are_not_blockers() {
        let result = select_catalogue_candidates(
            [
                candidate("AMS", 7355, "ams000/ams7355"),
                candidate("AMS", 7355, "ams000/ams7355_0001"),
            ],
            &["AMS".into()],
            &[],
        );
        assert!(result.blockers.is_empty(), "{:?}", result.blockers);
        assert_eq!(
            result.selected,
            vec![candidate("AMS", 7355, "ams000/ams7355_0001")]
        );
        assert_eq!(
            result.shadowed,
            vec![candidate("AMS", 7355, "ams000/ams7355")]
        );
    }

    #[test]
    fn sibling_extracts_still_block_catalogue_selection() {
        let result = select_catalogue_candidates(
            [
                candidate("AMS", 9990, "ams000/ams9990_0001"),
                candidate("AMS", 9990, "ams000/ams9990_0002"),
            ],
            &["AMS".into()],
            &[],
        );
        assert!(result.selected.is_empty());
        assert_eq!(result.blockers.len(), 1);
        // 归属说得出来，被挡住的人才跳得过去（ISSUE-025 §三）。
        assert_eq!(result.blockers[0].dbnum, Some(9990));
        assert_eq!(result.blockers[0].project.as_deref(), Some("AMS"));
    }

    #[test]
    fn exact_filename_dbnum_shadows_same_project_mismatched_alias() {
        let result = select_catalogue_candidates(
            [
                candidate("ZDJ", 7200, "ZDJ000/zdj7200_0001"),
                candidate("ZDJ", 7200, "ZDJ000/zdj7210"),
            ],
            &["ZDJ".into()],
            &["ZDJ".into()],
        );
        assert!(result.blockers.is_empty(), "{:?}", result.blockers);
        assert_eq!(
            result.selected,
            vec![candidate("ZDJ", 7200, "ZDJ000/zdj7200_0001")]
        );
        assert_eq!(
            result.shadowed,
            vec![candidate("ZDJ", 7200, "ZDJ000/zdj7210")]
        );
    }

    /// 8191 现场：meta 相被一个失败批次焊住，DESI 在屏障后面排着。快照要说得出
    /// **是哪个库**挡的、去哪儿取它的全文原因，而不是只丢一句「被 meta 挡着」。
    #[test]
    fn a_failed_batch_names_its_culprit_in_the_snapshot() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(epoch, [DataPhase::Meta, DataPhase::Design], false, []);
        coordinator.mark_failed(
            epoch,
            PhaseBlocker::new(
                DataPhase::Meta,
                "读取增量数据失败: 打开 dabacon 冻结快照失败",
            )
            .with_dbnum(8191)
            .with_project("JEU")
            .with_task("db-20260827-114844-000000")
            .with_failure_record("db-20260827-114844-000000"),
        );

        let snapshot = coordinator.snapshot();
        assert_eq!(snapshot.current_phase, Some("meta"));
        assert!(!coordinator.allows(DataPhase::Design, epoch));

        let blocker = snapshot.blocked_by.first().expect("one blocker");
        assert_eq!(blocker.phase, DataPhase::Meta);
        assert_eq!(blocker.dbnum, Some(8191));
        assert_eq!(blocker.project.as_deref(), Some("JEU"));
        assert_eq!(
            blocker.reason_ref.as_deref(),
            Some("db-20260827-114844-000000")
        );
    }

    /// 渲染串是旧消费者（`/health` 的 `blockers`、`InitializationNotReady` 的错误
    /// 串）唯一读的东西，结构化不许把它的形状改掉。
    #[test]
    fn the_rendered_string_keeps_its_pre_structuring_shape() {
        let coordinator = coordinator();
        let epoch = coordinator.begin_discovery();
        coordinator.install_manifest(
            epoch,
            [DataPhase::Design],
            false,
            [PhaseBlocker::new(DataPhase::Meta, "dbnum=8191 同号多文件").with_dbnum(8191)],
        );
        assert_eq!(
            coordinator.snapshot().blockers,
            vec!["meta: dbnum=8191 同号多文件".to_string()]
        );
    }

    /// 扫描期的阻断没有任务，也就没有落盘的失败记录。`reason_ref` 这时必须为空
    /// ——指向一条不存在的记录，比不给更费人时间。
    #[test]
    fn a_scan_time_blocker_points_at_no_failure_record() {
        let blocker = CatalogueBlocker::new("目录清单不可读").into_phase(DataPhase::Meta);
        assert_eq!(blocker.task_id, None);
        assert_eq!(blocker.reason_ref, None);
    }
}
