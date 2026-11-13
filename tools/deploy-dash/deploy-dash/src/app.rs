//! 根视图：组装各区块、收事件、发命令、开抽屉和对话框。各区块的渲染在 `ui/` 下的文件里。
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

use gpui_kit::component::button::{ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::SelectState;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{
    IndexPath, Placement, Theme, ThemeMode, TitleBar, WindowExt as _, h_flex, resizable_panel,
    v_flex, v_resizable,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::data::{Command, Fault, Fixture, StackDriver};
use crate::events::{LogLevel, LogLine, NoticeKind, UiEvent};
use crate::model::*;
use crate::state::{DashState, Fix, LOG_VIEW_CAP, LogFilter, LogStore, RunningJob};
use crate::ui::dialogs::{DialogKind, DialogView};
use crate::ui::sheets::{SheetKind, SheetView};
use crate::ui::tokens::Tokens;

use std::collections::HashMap;
use std::time::SystemTime;

use crate::client_store::{self, ClientStore, PanelSettings, ServerEntry, ServerList, StoredToken};
use crate::data::remote::{self, AgentClient, AgentClientOptions, AgentHandle, ConnState, Focus, Token};
use crate::fleet_state::{self, BannerKind, Failure, Field, FleetState, FormMode, LOCAL_NAME, Probe, Reported, ServerId, TestState};
use crate::ui::fleet::{self, FleetAction, OnFleet, ServerForm};

pub const WINDOW_TITLE: &str = "AIOS 部署面板（GPUI 原型）";
pub const WINDOW_TITLE_REAL: &str = "AIOS 部署面板（真实服务）";
pub const PANEL_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct AppOptions {
    pub work_dir: PathBuf,
    pub log_dir: PathBuf,
    pub shots_dir: PathBuf,
    pub selftest: bool,
    pub start: Fixture,
    /// 演示用：`--scenario incident` 时数据层起来后马上注入的故障。
    pub initial_fault: Option<Fault>,
    pub dark: bool,
    /// 真实服务（stack）；自检时走真实模式的那套脚本。
    pub real: bool,
    pub title: &'static str,
}

/// 首次配置表单（DASHBOARD-DESIGN 9.2）。输入框状态建在面板里，抽屉每次重建时还在。
pub struct ConfigForm {
    pub root: Entity<InputState>,
    pub name: Entity<InputState>,
    pub dir: Entity<InputState>,
    pub code: Entity<InputState>,
    pub mdb: Entity<InputState>,
    pub version: Entity<SelectState<Vec<SharedString>>>,
}

/// 没选中的那几台：各自的数据层、面板状态、日志。选中的那台的在 `DashApp.driver` / `st` / `logs`，切换时对调，
/// 所以 `ui\` 下所有读 `self.st`、`self.driver` 的地方都不用改。
pub(crate) struct ServerSlot {
    pub(crate) driver: Box<dyn StackDriver>,
    pub(crate) st: DashState,
    pub(crate) logs: LogStore,
}

/// 服务器抽屉里表单那一半的状态（行来自 `FleetState::drawer_rows`）。
pub(crate) struct DrawerState {
    pub(crate) mode: FormMode,
    pub(crate) errors: Vec<(Field, String)>,
    pub(crate) test: TestState,
    pub(crate) test_endpoint: String,
}

impl DrawerState {
    pub(crate) fn closed() -> Self {
        Self { mode: FormMode::Closed, errors: Vec::new(), test: TestState::Idle, test_endpoint: String::new() }
    }
}

/// 远程时配置抽屉「探测」的结果（R10）：问的是服务器上的路径，不是这台电脑的。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RootProbe {
    Idle,
    Running,
    /// （通没通，给人看的一句）
    Done(bool, String),
}

pub struct DashApp {
    pub(crate) opts: AppOptions,
    pub(crate) driver: Box<dyn StackDriver>,
    pub(crate) st: DashState,
    pub(crate) logs: LogStore,
    pub(crate) log_view: Rc<RefCell<Vec<LogLine>>>,
    pub(crate) log_state: Entity<MessageScrollerState>,
    pub(crate) log_filter: LogFilter,
    pub(crate) log_paused: bool,
    pub(crate) log_pending: u64,
    pub(crate) search: Entity<InputState>,
    pub(crate) form: ConfigForm,
    /// 0 = 填工程；1 = 检查结果
    pub(crate) config_step: usize,
    pub(crate) form_errors: Vec<(&'static str, String)>,
    pub(crate) dbnums: Vec<DbnumRow>,
    pub(crate) picked: BTreeSet<u32>,
    /// 控制条上的「生成模型」勾选：缺省不勾；勾上才打开选库的抽屉。
    pub(crate) gen_checked: bool,
    /// 正在生成的库（最近一次「生成」点名的）。
    pub(crate) generating: Vec<u32>,
    pub(crate) preview: Option<Vec<(u32, u32)>>,
    pub(crate) sheet: Option<SheetKind>,
    pub(crate) dark: bool,
    pub(crate) dropped: Arc<AtomicU64>,
    pub(crate) report: Vec<String>,
    pub(crate) mem_mb: f64,
    pub(crate) upper_h: f32,
    // ── 多服务器（R13）──
    pub(crate) fleet: FleetState,
    /// `driver` / `st` / `logs` 现在属于哪一台。
    pub(crate) current: ServerId,
    pub(crate) parked: HashMap<ServerId, ServerSlot>,
    /// servers.toml 里的那几台（本机那一项不在里面）。令牌在 `StoredToken` 里，只在建 `AgentClient` 时取出来。
    pub(crate) entries: HashMap<ServerId, ServerEntry>,
    /// 经代理的那几台（含本机经代理时）的只读句柄：连接状态、下次重试、摘要、焦点、探测。
    pub(crate) handles: HashMap<ServerId, AgentHandle>,
    /// 后台那几台上次收的摘要的 `seq`（变了才算新数据）。
    pub(crate) summary_seq: HashMap<ServerId, u64>,
    pub(crate) store: Option<ClientStore>,
    pub(crate) panel: PanelSettings,
    /// 读 servers.toml / panel.toml 时的提示（坏行、解不开的令牌），抽屉里列出来。
    pub(crate) store_notes: Vec<String>,
    pub(crate) server_form: ServerForm,
    pub(crate) drawer: DrawerState,
    pub(crate) list_focus: FocusHandle,
    pub(crate) root_probe: RootProbe,
    /// 自检时弹过的提示标题（核对「开面板不弹已连上」用；平时不记）。
    pub(crate) notice_titles: Vec<String>,
    _subs: Vec<Subscription>,
}

impl DashApp {
    /// `local_handle`：本机那一台经代理（真实模式）时，它的 `AgentClient` 的句柄；直接管 stack、模拟数据时是 None。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        opts: AppOptions,
        mut driver: Box<dyn StackDriver>,
        local_handle: Option<AgentHandle>,
        ui_rx: async_channel::Receiver<UiEvent>,
        ui_tx: async_channel::Sender<UiEvent>,
        dropped: Arc<AtomicU64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        window.set_window_title(opts.title);
        let input = |window: &mut Window, cx: &mut Context<Self>, ph: &str| {
            let ph = ph.to_string();
            cx.new(|cx| InputState::new(window, cx).placeholder(ph))
        };
        let form = ConfigForm {
            root: input(window, cx, "例如 D:\\AVEVA\\Projects\\E3D3.1"),
            name: input(window, cx, "工程根目录下那个项目文件夹的名字"),
            dir: input(window, cx, "和项目名不一样时才填"),
            code: input(window, cx, "整数，例如 1516"),
            mdb: input(window, cx, "ALL"),
            version: cx.new(|cx| SelectState::new(vec![SharedString::from("e3d31"), SharedString::from("e3d210")], Some(IndexPath::new(0)), window, cx)),
        };
        cx.bind_keys(fleet::key_bindings());
        // 自检用自己的目录，不碰 %LOCALAPPDATA%\AiosDeployDash
        let store = if opts.selftest { Some(ClientStore::at(opts.work_dir.join("client-store"))) } else { ClientStore::user_default() };
        let panel = store.as_ref().map(|s| s.load_panel()).unwrap_or_default();
        let loaded = store.as_ref().map(|s| s.load_servers()).unwrap_or_default();
        let mut store_notes = panel.notes.clone();
        store_notes.extend(loaded.notes.iter().cloned());
        let server_form = ServerForm::new(window, cx);
        let search = input(window, cx, "搜索日志");
        let log_state = cx.new(|cx| MessageScrollerState::new(0, cx));
        let mut subs = Vec::new();
        subs.push(cx.observe(&search, |this, s, cx| {
            let q = s.read(cx).value().to_lowercase();
            if q != this.log_filter.search {
                this.log_filter.search = q;
                this.rebuild_log_view(cx);
            }
        }));
        subs.push(cx.on_app_quit(|app, _cx| {
            app.driver.shutdown(true);
            for slot in app.parked.values_mut() {
                slot.driver.shutdown(true);
            }
            async {}
        }));

        // 事件泵：后台 → 通道 → 每 16 ms 左右批量交给界面（deploy-ui 已验证的写法）
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(first) = ui_rx.recv().await {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let mut batch = vec![first];
                while batch.len() < 20_000 {
                    match ui_rx.try_recv() {
                        Ok(ev) => batch.push(ev),
                        Err(_) => break,
                    }
                }
                if this.update_in(cx, |app, window, cx| app.on_events(batch, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // 每秒走一下：「更新于 N 秒前」、计时器、面板自己的内存
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let ok = this.update(cx, |app, cx| {
                    app.mem_mb = crate::winsys::process_memory_mb().0;
                    app.mirror_fleet();
                    cx.notify();
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();

        let _ = ui_tx;
        let now = Instant::now();
        let mut fleet = FleetState::default();
        let mut parked: HashMap<ServerId, ServerSlot> = HashMap::new();
        let mut entries: HashMap<ServerId, ServerEntry> = HashMap::new();
        let mut handles: HashMap<ServerId, AgentHandle> = HashMap::new();
        // main.rs 交来的那一台：真实模式是本机，否则是模拟数据。远程客户端（exe 旁没有 stack.ps1）列表里已有服务器时，
        // 不再列一台「模拟数据」充数；两样都没有才是今天的模拟面板。
        if opts.real || opts.selftest || loaded.value.servers.is_empty() {
            let id = fleet.add(if opts.real { LOCAL_NAME } else { "模拟数据" }, "127.0.0.1", true, now);
            let (tx, rx) = async_channel::bounded(200_000);
            driver.start(tx);
            if let Some(f) = opts.initial_fault {
                driver.send(Command::InjectFault(f));
            }
            pump_server(id, rx, window, cx);
            parked.insert(id, ServerSlot { driver, st: DashState::default(), logs: LogStore::default() });
            if let Some(h) = local_handle {
                handles.insert(id, h);
            }
        } else {
            driver.shutdown(true);
        }
        for entry in loaded.value.servers {
            let id = fleet.add(&entry.name, &entry.endpoint(), false, now);
            let (slot, handle) = connect_remote(id, &entry, Focus::Background, window, cx);
            parked.insert(id, slot);
            handles.insert(id, handle);
            entries.insert(id, entry);
        }
        let first = fleet.select_initial(panel.value.selected.as_deref()).map(|sw| sw.to).expect("至少有一台");
        let slot = parked.remove(&first).expect("每台都有 slot");
        for (id, h) in &handles {
            h.set_focus(if *id == first { Focus::Selected } else { Focus::Background });
        }
        let dark = opts.dark;
        if dark {
            Theme::change(ThemeMode::Dark, Some(window), cx);
        }
        let app = Self {
            opts,
            driver: slot.driver,
            st: slot.st,
            logs: slot.logs,
            log_view: Rc::new(RefCell::new(Vec::new())),
            log_state,
            log_filter: LogFilter::default(),
            log_paused: false,
            log_pending: 0,
            search,
            form,
            config_step: 0,
            form_errors: Vec::new(),
            dbnums: Vec::new(),
            picked: BTreeSet::new(),
            gen_checked: false,
            generating: Vec::new(),
            preview: None,
            sheet: None,
            dark,
            dropped,
            report: Vec::new(),
            mem_mb: 0.0,
            upper_h: 0.0,
            fleet,
            current: first,
            parked,
            entries,
            handles,
            summary_seq: HashMap::new(),
            store,
            panel: panel.value,
            store_notes,
            server_form,
            drawer: DrawerState::closed(),
            list_focus: cx.focus_handle(),
            root_probe: RootProbe::Idle,
            notice_titles: Vec::new(),
            _subs: subs,
        };
        tracing::info!(target: "app", "面板已打开：数据来源 {}，工作目录 {}", app.driver.source_label(), app.opts.work_dir.display());
        if app.opts.selftest {
            let real = app.opts.real;
            cx.spawn_in(window, async move |this, cx| {
                let result = if real { crate::selftest::run_real(this.clone(), cx).await } else { crate::selftest::run(this.clone(), cx).await };
                if let Err(e) = result {
                    tracing::error!(target: "app", "自检中断：{e:#}");
                    let _ = this.update(cx, |app, _| app.write_report(&format!("自检中断：{e:#}")));
                }
                let _ = cx.update(|_, cx| cx.quit());
            })
            .detach();
        }
        app
    }

    pub(crate) fn cmd(&mut self, c: Command) {
        self.driver.send(c);
    }

    /// 自检时不挂悬停提示和悬停高亮：桌面上的鼠标停在窗口哪儿，截图里就会多出一个提示框。
    pub(crate) fn hover_ui(&self) -> bool {
        !self.opts.selftest
    }

    // ───────────────────────── 事件 ─────────────────────────

    fn on_server_events(&mut self, id: ServerId, batch: Vec<UiEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let fresh = batch.iter().any(|e| matches!(e, UiEvent::Snapshot(_)));
        let failed = batch.iter().rev().find_map(|e| match e {
            UiEvent::SnapshotFailed { reason } => Some(reason.clone()),
            _ => None,
        });
        if id == self.current {
            let batch = if self.has_handle(id) { batch.into_iter().filter(|e| !is_connection_notice(e)).collect() } else { batch };
            self.on_events(batch, window, cx);
            self.align_job_starts(now, SystemTime::now());
        } else if let Some(slot) = self.parked.get_mut(&id) {
            feed_background(slot, batch, now);
        }
        if fresh {
            let st = if id == self.current { Some(&self.st) } else { self.parked.get(&id).map(|s| &s.st) };
            if let Some(summary) = st.and_then(|s| s.fleet_summary()) {
                self.fleet.on_summary(id, summary, now, SystemTime::now(), None);
            }
        }
        // 有句柄的（经代理）连接状态照 AgentClient 的判定（mirror_fleet）；没有句柄的（本机直连 stack、模拟数据）照失败事件数
        if !self.has_handle(id)
            && let Some(reason) = failed
        {
            self.fleet.on_failure(id, Failure::Unreachable(reason), now);
        }
        cx.notify();
    }

    pub(crate) fn has_handle(&self, id: ServerId) -> bool {
        self.handles.contains_key(&id)
    }

    /// 每秒一次：连接状态照各台 `AgentClient` 的判定（它自己数失败、按 `fleet_state::backoff` 退避）；
    /// 后台那几台的摘要从句柄里读（它们只拉 `summary=1`，不推快照事件）。
    pub(crate) fn mirror_fleet(&mut self) {
        let now = Instant::now();
        let wall = SystemTime::now();
        for (id, h) in &self.handles {
            if *id != self.current
                && let Some(s) = h.summary()
                && self.summary_seq.get(id) != Some(&s.seq)
            {
                self.summary_seq.insert(*id, s.seq);
                let rtt = h.agent().map(|a| a.rtt_ms.min(u32::MAX as u64) as u32);
                self.fleet.on_summary(*id, summary_from_proto(&s), now, wall, rtt);
            }
            self.fleet.on_reported(*id, reported_of(h.state()), h.retry_in(), now, wall);
        }
        self.align_job_starts(now, wall);
    }

    /// 经代理的那几台：正在跑的任务的「用时」从代理任务表里的开始时刻算（再开面板时 `JobStarted` 是面板这次才看到它）。
    fn align_job_starts(&mut self, now: Instant, wall: SystemTime) {
        for (id, h) in &self.handles {
            let st = if *id == self.current { &mut self.st } else if let Some(slot) = self.parked.get_mut(id) { &mut slot.st } else { continue };
            if st.job.is_none() {
                continue;
            }
            if let Some(job) = h.summary().and_then(|s| s.job)
                && let Some(started) = job.started_at.as_deref().and_then(crate::state::parse_iso)
            {
                st.align_job_start(job.id, started, now, wall);
            }
        }
    }

    fn on_events(&mut self, batch: Vec<UiEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut new_lines = Vec::new();
        for ev in batch {
            match ev {
                UiEvent::Snapshot(s) => self.st.on_snapshot(*s, now),
                UiEvent::SnapshotFailed { reason } => {
                    self.st.snapshot_failures += 1;
                    self.st.last_failure = Some(reason);
                }
                UiEvent::Metrics(m) => self.st.on_metrics(*m),
                UiEvent::BackendJobs(j) => self.st.on_backend_jobs(*j),
                UiEvent::HistorySeed(seed) => self.st.on_seed(&seed, now),
                UiEvent::JobStarted { id, kind } => {
                    self.st.job = Some(RunningJob { id, kind, started: now, step_started: [None; 4], step_done: [None; 4], restart_stopped: false });
                    if kind == JobKind::Doctor {
                        self.st.doctor_running = true;
                    }
                }
                UiEvent::JobFinished { id, kind, exit_code, message } => {
                    if self.st.job.as_ref().is_some_and(|j| j.id == id) {
                        self.st.job = None;
                    }
                    if kind == JobKind::Init {
                        self.generating.clear();
                        self.picked.clear();
                    }
                    self.on_job_finished(kind, exit_code, &message, window, cx);
                }
                UiEvent::LogLines(lines) => new_lines.extend(lines),
                UiEvent::Notice { kind, title, message } => {
                    if self.opts.selftest {
                        self.notice_titles.push(title.clone());
                    }
                    let note = match kind {
                        NoticeKind::Info => Notification::info(message),
                        NoticeKind::Success => Notification::success(message),
                        NoticeKind::Warning => Notification::warning(message),
                        NoticeKind::Error => Notification::error(message),
                    };
                    window.push_notification(note.title(title), cx);
                }
                UiEvent::Doctor(report) => {
                    self.st.doctor_running = false;
                    self.form_errors = report
                        .problems
                        .iter()
                        .filter_map(|p| field_key(&p.field).map(|k| (k, format!("{}：{}", p.what, p.next))))
                        .collect();
                    if self.sheet == Some(SheetKind::Config) {
                        self.config_step = if self.form_errors.is_empty() { 1 } else { 0 };
                    }
                    self.st.doctor = Some(report);
                }
                UiEvent::DownBusy { reasons } => self.confirm_force_stop(reasons, window, cx),
                UiEvent::Dbnums(rows) => self.dbnums = rows,
                UiEvent::UpdatePreview(rows) => self.preview = Some(rows),
            }
        }
        if !new_lines.is_empty() {
            self.append_logs(new_lines, cx);
        }
        cx.notify();
    }

    fn on_job_finished(&mut self, kind: JobKind, code: i32, message: &str, window: &mut Window, cx: &mut Context<Self>) {
        match (kind, code) {
            (JobKind::Configure, 0) => {
                self.st.doctor = None;
                self.cmd(Command::Doctor);
            }
            (JobKind::Doctor, _) | (JobKind::Init, 0) | (JobKind::Update, 0) => {}
            (_, 6) => {}
            (k, 0) if k.is_lifecycle() => {
                window.push_notification(Notification::success(message.to_string()).title(k.label()), cx);
            }
            (k, c) => {
                let msg = if message.is_empty() { format!("退出码 {c}") } else { format!("{message}（退出码 {c}）") };
                window.push_notification(Notification::error(msg).title(format!("{}没完成", k.label())), cx);
            }
        }
    }

    // ───────────────────────── 日志 ─────────────────────────

    fn append_logs(&mut self, lines: Vec<LogLine>, cx: &mut Context<Self>) {
        let old_len = self.log_view.borrow().len();
        let mut added = 0usize;
        {
            let mut view = self.log_view.borrow_mut();
            for line in lines {
                if self.log_filter.matches(&line) {
                    if self.log_paused {
                        self.log_pending += 1;
                    } else {
                        view.push(line.clone());
                        added += 1;
                    }
                }
                self.logs.push(line);
            }
        }
        if added == 0 {
            return;
        }
        let (trim, new_len) = {
            let mut v = self.log_view.borrow_mut();
            let trim = v.len().saturating_sub(LOG_VIEW_CAP);
            v.drain(..trim);
            (trim, v.len())
        };
        self.log_state.update(cx, |s, cx| {
            if trim > old_len {
                s.reset(new_len, cx);
            } else {
                if trim > 0 {
                    s.splice(0..trim, 0, cx);
                }
                s.append(added, cx);
            }
        });
    }

    pub(crate) fn rebuild_log_view(&mut self, cx: &mut Context<Self>) {
        let lines = self.logs.collect(&self.log_filter, LOG_VIEW_CAP);
        let n = lines.len();
        *self.log_view.borrow_mut() = lines;
        self.log_pending = 0;
        self.log_state.update(cx, |s, cx| {
            s.reset(n, cx);
            s.scroll_to_end(cx);
        });
        cx.notify();
    }

    pub(crate) fn set_log_source(&mut self, source: usize, cx: &mut Context<Self>) {
        self.log_filter.source = source;
        self.st.selected = if (1..=4).contains(&source) { Some(Svc::ALL[source - 1]) } else { None };
        self.rebuild_log_view(cx);
    }

    pub(crate) fn set_log_level(&mut self, level: Option<LogLevel>, cx: &mut Context<Self>) {
        self.log_filter.min_level = level;
        self.rebuild_log_view(cx);
    }

    pub(crate) fn set_log_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.log_paused = paused;
        if !paused {
            self.rebuild_log_view(cx);
        }
        cx.notify();
    }

    /// 单击服务行：选中它并把日志筛到它；再点一次取消。
    pub(crate) fn toggle_select(&mut self, svc: Svc, cx: &mut Context<Self>) {
        if self.st.selected == Some(svc) {
            self.set_log_source(0, cx);
        } else {
            self.set_log_source(1 + svc.index(), cx);
        }
    }

    pub(crate) fn view_logs(&mut self, svc: Svc, cx: &mut Context<Self>) {
        self.log_filter.min_level = None;
        self.set_log_source(1 + svc.index(), cx);
    }

    // ───────────────────────── 操作 ─────────────────────────

    pub(crate) fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        Theme::change(if self.dark { ThemeMode::Dark } else { ThemeMode::Light }, Some(window), cx);
        cx.notify();
    }

    pub(crate) fn start_all(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.cmd(Command::Up);
        cx.notify();
    }

    pub(crate) fn confirm_stop_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("停止全部服务？")
                .description("按 前端 → rs-server → 后端 → 数据库 的顺序停。正在用网页的人会断开。")
                .show_cancel(true)
                .ok_text("全部停止")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("取消")
                .on_ok(move |_, _, cx| {
                    let _ = this.update(cx, |app, _| app.cmd(Command::Down { force: false }));
                    true
                })
        });
    }

    fn confirm_force_stop(&mut self, reasons: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let body = format!(
            "正在：{}。手上的活会停在半截，下次启动重做；库版本记录还没写时停下，rs-server 可能一直起不来。",
            reasons.join("；")
        );
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("后端正在忙，强行停止？")
                .description(body.clone())
                .show_cancel(true)
                .ok_text("强行停止")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("取消")
                .on_ok(move |_, _, cx| {
                    let _ = this.update(cx, |app, _| app.cmd(Command::Down { force: true }));
                    true
                })
        });
    }

    /// 重启：进程已经不在（或异常）时直接起；正常运行的要先确认。
    pub(crate) fn restart(&mut self, svc: Svc, window: &mut Window, cx: &mut Context<Self>) {
        let running_ok = self.st.snapshot.as_ref().and_then(|s| s.svc(svc)).is_some_and(|s| s.state == SvcState::Ok && s.pid != 0);
        if !running_ok {
            self.cmd(Command::RestartService(svc));
            cx.notify();
            return;
        }
        let this = cx.entity().downgrade();
        let body = match svc {
            Svc::Backend => "正在进行的初始化或增量会中断；rs-server 和前端会短暂断开。",
            Svc::Database => "后端、rs-server 会短暂连不上数据库。",
            _ => "网页会短暂打不开。",
        };
        let title = format!("重启{}？", svc.title());
        let ok = format!("重启{}", svc.title());
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(title.clone())
                .description(body)
                .show_cancel(true)
                .ok_text(ok.clone())
                .cancel_text("取消")
                .on_ok(move |_, _, cx| {
                    let _ = this.update(cx, |app, _| app.cmd(Command::RestartService(svc)));
                    true
                })
        });
    }

    pub(crate) fn confirm_stop(&mut self, svc: Svc, window: &mut Window, cx: &mut Context<Self>) {
        let running: Vec<Svc> = svc
            .dependents()
            .iter()
            .copied()
            .filter(|d| self.st.snapshot.as_ref().and_then(|s| s.svc(*d)).is_some_and(|x| x.pid != 0))
            .collect();
        let this = cx.entity().downgrade();
        let (body, ok) = if running.is_empty() {
            (
                match svc {
                    Svc::Frontend => "网页打不开，直到再次启动。".to_string(),
                    Svc::RsServer => "元数据、提资、校审这些接口不可用，直到再次启动。".to_string(),
                    _ => "依赖它的功能会不可用，直到再次启动。".to_string(),
                },
                format!("停止{}", svc.title()),
            )
        } else {
            (
                format!("{}依赖它，会一起停。", running.iter().map(|s| s.title()).collect::<Vec<_>>().join("、")),
                format!("停止 {} 个服务", running.len() + 1),
            )
        };
        let title = format!("停止{}？", svc.title());
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(title.clone())
                .description(body.clone())
                .show_cancel(true)
                .ok_text(ok.clone())
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("取消")
                .on_ok(move |_, _, cx| {
                    let _ = this.update(cx, |app, _| app.cmd(Command::StopService { svc, force: false }));
                    true
                })
        });
    }

    pub(crate) fn run_doctor(&mut self, cx: &mut Context<Self>) {
        self.st.doctor = None;
        self.st.doctor_running = true;
        self.cmd(Command::Doctor);
        cx.notify();
    }

    pub(crate) fn fix(&mut self, fix: Fix, window: &mut Window, cx: &mut Context<Self>) {
        match fix {
            Fix::Restart(svc) => self.restart(svc, window, cx),
            Fix::ViewSvc(svc) => self.open_service_sheet(svc, window, cx),
            Fix::ViewJobs => self.open_jobs_sheet(window, cx),
            Fix::ViewDoctor => {
                self.config_step = 1;
                self.open_config_sheet(window, cx);
            }
            Fix::OpenLogsDir => self.open_folder(format!("{}\\logs", self.root_dir()), window, cx),
        }
    }

    pub(crate) fn root_dir(&self) -> String {
        self.st.snapshot.as_ref().map(|s| s.root.clone()).unwrap_or_default()
    }

    /// 模拟数据下运行目录不存在，不开资源管理器，只告诉人会打开哪里。
    pub(crate) fn open_folder(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if std::path::Path::new(&path).is_dir() {
            let _ = std::process::Command::new("explorer.exe").arg(&path).spawn();
        } else {
            window.push_notification(Notification::info(format!("会打开 {path}（模拟数据，这个目录不存在）")).title("打开文件夹"), cx);
        }
    }

    pub(crate) fn inject(&mut self, fault: Fault, cx: &mut Context<Self>) {
        self.cmd(Command::InjectFault(fault));
        cx.notify();
    }

    /// 增量同步开关：开 = 后端队列恢复出队；关 = 暂停（状态以后端的 queue_paused 为准，下一份快照就看得到）。
    pub(crate) fn set_increments(&mut self, on: bool, cx: &mut Context<Self>) {
        self.cmd(Command::SetIncrements(on));
        cx.notify();
    }

    // ───────────────────────── 多服务器 ─────────────────────────

    /// 换到另一台：选中那台的 driver / st / logs 和后台那台对调；焦点跟着走（选中的拉全量和日志，别的只拉摘要）。
    /// 跟着服务器走的抽屉（配置、服务详情、后端任务、生成模型）收起；服务器抽屉留着。
    pub(crate) fn switch_to(&mut self, to: ServerId, window: &mut Window, cx: &mut Context<Self>) {
        if to == self.current {
            return;
        }
        let Some(slot) = self.parked.remove(&to) else { return };
        let old = ServerSlot {
            driver: std::mem::replace(&mut self.driver, slot.driver),
            st: std::mem::replace(&mut self.st, slot.st),
            logs: std::mem::replace(&mut self.logs, slot.logs),
        };
        let from = std::mem::replace(&mut self.current, to);
        self.parked.insert(from, old);
        if let Some(h) = self.handles.get(&from) {
            h.set_focus(Focus::Background);
        }
        if let Some(h) = self.handles.get(&to) {
            h.set_focus(Focus::Selected);
        }
        self.root_probe = RootProbe::Idle;
        if matches!(self.sheet, Some(SheetKind::Config | SheetKind::Service(_) | SheetKind::Jobs | SheetKind::Generate)) {
            self.close_sheet(window, cx);
        }
        self.dbnums.clear();
        self.picked.clear();
        self.gen_checked = false;
        self.generating.clear();
        self.preview = None;
        self.form_errors.clear();
        self.config_step = 0;
        self.rebuild_log_view(cx);
        self.panel.selected = self.fleet.get(to).map(|s| s.name.clone());
        self.save_panel();
        cx.notify();
    }

    pub(crate) fn select_server(&mut self, id: ServerId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sw) = self.fleet.select(id) {
            self.switch_to(sw.to, window, cx);
        }
    }

    fn select_nth_server(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sw) = self.fleet.select_nth(n) {
            self.switch_to(sw.to, window, cx);
        }
    }

    fn step_server(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sw) = self.fleet.select_step(delta) {
            self.switch_to(sw.to, window, cx);
        }
    }

    fn save_panel(&mut self) {
        if let Some(store) = &self.store
            && let Err(e) = store.save_panel(&self.panel)
        {
            tracing::warn!(target: "app", "面板设置没存上：{e}");
        }
    }

    fn save_servers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let list = ServerList { servers: self.fleet.servers().iter().filter_map(|s| self.entries.get(&s.id).cloned()).collect() };
        if let Err(msg) = store.save_servers(&list) {
            window.push_notification(Notification::error(msg).title("服务器列表没存上"), cx);
        }
    }

    /// 列表、汇总条、横幅、抽屉上所有点击都到这里。
    pub(crate) fn fleet_action(&mut self, action: FleetAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            FleetAction::Select(id) => self.select_server(id, window, cx),
            FleetAction::ToggleList => {
                let width = window.viewport_size().width.as_f32();
                let mode = fleet_state::list_mode(width, self.fleet.len(), self.panel.list_collapsed);
                if let Some(collapsed) = fleet_state::toggle_list(mode) {
                    self.panel.list_collapsed = Some(collapsed);
                    self.save_panel();
                }
            }
            FleetAction::OpenServers => {
                if self.sheet != Some(SheetKind::Servers) {
                    self.open_sheet(SheetKind::Servers, window, cx);
                }
            }
            FleetAction::Add => {
                self.drawer = DrawerState { mode: FormMode::Adding, ..DrawerState::closed() };
                self.server_form.load(None, window, cx);
                self.fleet_action(FleetAction::OpenServers, window, cx);
            }
            FleetAction::Edit(id) => {
                let Some(e) = self.entries.get(&id) else { return };
                self.drawer = DrawerState { mode: FormMode::Editing { id, has_token: e.token.is_saved() }, ..DrawerState::closed() };
                self.server_form.load(Some((&e.name, &e.address, e.port, &e.note, e.token.is_saved())), window, cx);
                self.fleet_action(FleetAction::OpenServers, window, cx);
            }
            FleetAction::Delete(id) => self.delete_server(id, window, cx),
            FleetAction::Move(id, delta) => {
                if self.fleet.move_by(id, delta) {
                    self.save_servers(window, cx);
                }
            }
            FleetAction::Test => self.test_server(window, cx),
            FleetAction::Save => self.save_server(window, cx),
            FleetAction::CancelEdit => {
                self.drawer = DrawerState::closed();
                self.server_form.clear_token(window, cx);
            }
            FleetAction::Retry(id) => {
                self.fleet.retry(id, Instant::now());
                if let Some(h) = self.handles.get(&id) {
                    h.retry_now();
                }
                if id == self.current {
                    self.cmd(Command::RefreshNow);
                }
            }
            FleetAction::Relaunch(id) => self.relaunch_local(id, window, cx),
        }
        cx.notify();
    }

    fn delete_server(&mut self, id: ServerId, window: &mut Window, cx: &mut Context<Self>) {
        let switch = self.fleet.remove(id);
        self.entries.remove(&id);
        self.summary_seq.remove(&id);
        self.handles.remove(&id);
        if let Some(sw) = switch {
            // 删的是选中的那台：先换到邻居，它自己的 slot 就停到 parked 里了
            self.switch_to(sw.to, window, cx);
        }
        if let Some(mut slot) = self.parked.remove(&id) {
            slot.driver.shutdown(true);
        }
        if matches!(self.drawer.mode, FormMode::Editing { id: editing, .. } if editing == id) {
            self.drawer = DrawerState::closed();
            self.server_form.clear_token(window, cx);
        }
        self.save_servers(window, cx);
    }

    /// 「保存」：先在界面上检查，过了才写 servers.toml（令牌 DPAPI 加密）。地址或令牌变了就换一个新的 AgentClient。
    fn save_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.server_form.read(cx);
        let editing = match self.drawer.mode {
            FormMode::Editing { id, has_token } => Some((id, has_token)),
            _ => None,
        };
        let others = self.fleet.other_names(editing.map(|e| e.0));
        let form = match fleet_state::validate_form(&input, &others, editing.is_some_and(|e| e.1)) {
            Ok(form) => form,
            Err(errors) => {
                self.drawer.errors = errors;
                return;
            }
        };
        let now = Instant::now();
        let endpoint = client_store::endpoint(&form.address, form.port);
        let token_changed = form.token.is_some();
        let id = match editing {
            None => {
                let id = self.fleet.add(&form.name, &endpoint, false, now);
                let entry = ServerEntry {
                    name: form.name,
                    address: form.address,
                    port: form.port,
                    token: form.token.map(StoredToken::Present).unwrap_or_default(),
                    note: form.note,
                    order: 0,
                    // 新加的服务器先不带升级密钥、不自动升；在服务器抽屉里填
                    upgrade_key: StoredToken::None,
                    auto_upgrade: false,
                };
                self.entries.insert(id, entry);
                id
            }
            Some((id, _)) => {
                let Some(e) = self.entries.get_mut(&id) else { return };
                e.name = form.name;
                e.address = form.address;
                e.port = form.port;
                e.note = form.note;
                if let Some(t) = form.token {
                    e.token = StoredToken::Present(t);
                }
                let name = e.name.clone();
                let moved = self.fleet.update(id, &name, &endpoint, now);
                if !moved && !token_changed {
                    self.save_servers(window, cx);
                    self.drawer = DrawerState::closed();
                    self.server_form.clear_token(window, cx);
                    return;
                }
                self.fleet.reset_conn(id, now);
                id
            }
        };
        self.reconnect(id, window, cx);
        self.save_servers(window, cx);
        self.drawer = DrawerState::closed();
        self.server_form.clear_token(window, cx);
    }

    /// 给这台换一个新的数据层（新加的、改了地址或令牌的）。选中的那台就地换掉，状态和日志清空。
    fn reconnect(&mut self, id: ServerId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(&id).cloned() else { return };
        let focus = if id == self.current { Focus::Selected } else { Focus::Background };
        let (slot, handle) = connect_remote(id, &entry, focus, window, cx);
        self.handles.insert(id, handle);
        self.summary_seq.remove(&id);
        if id == self.current {
            let mut old = std::mem::replace(&mut self.driver, slot.driver);
            old.shutdown(true);
            self.st = slot.st;
            self.logs = slot.logs;
            self.rebuild_log_view(cx);
        } else if let Some(mut old) = self.parked.insert(id, slot) {
            old.driver.shutdown(true);
        }
    }

    /// 「测试连接」：不建 AgentClient，问一次 `/v1/ping` 和 `/v1/info`（后台线程），结果分三种失败说清楚。
    fn test_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.server_form.read(cx);
        let (address, port_in_address) = match fleet_state::parse_address(&input.address) {
            Ok(found) => found,
            Err(e) => {
                self.drawer.errors = vec![(Field::Address, e)];
                return;
            }
        };
        let port = input.port.trim().parse::<u16>().ok().filter(|p| *p > 0).or(port_in_address).unwrap_or(client_store::DEFAULT_PORT);
        let typed = input.token.trim();
        let saved = match self.drawer.mode {
            FormMode::Editing { id, .. } => self.entries.get(&id).and_then(|e| e.token.secret()).map(|s| s.expose().to_string()),
            _ => None,
        };
        let token = Token::new(if typed.is_empty() { saved.unwrap_or_default() } else { typed.to_string() });
        self.drawer.errors.clear();
        self.drawer.test = TestState::Running;
        self.drawer.test_endpoint = client_store::endpoint(&address, port);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { remote::test_connection(&address, port, &token) }).await;
            let probe = match result {
                Ok(p) => Probe::Ok {
                    agent_version: p.info.agent_version,
                    stack_name: p.info.name,
                    stack_version: p.info.stack_version,
                    rtt_ms: p.rtt_ms.min(u32::MAX as u64) as u32,
                },
                Err(ConnState::Unauthorized) => Probe::WrongToken,
                Err(ConnState::Incompatible { agent_version, schema }) => Probe::OldAgent { agent_version, schema },
                Err(ConnState::Unreachable { reason }) => Probe::WrongAddress(bare_reason(reason)),
                Err(other) => Probe::WrongAddress(format!("{other:?}")),
            };
            let _ = this.update(cx, |app, cx| {
                app.drawer.test = TestState::Done(probe);
                cx.notify();
            });
        })
        .detach();
    }

    /// 本机那一项「重新拉起代理」：后台线程跑 `ensure_local_agent`（最多等 10 秒），好了叫醒数据层马上重连。
    fn relaunch_local(&mut self, id: ServerId, window: &mut Window, cx: &mut Context<Self>) {
        let root = std::path::PathBuf::from(self.root_dir());
        if root.as_os_str().is_empty() {
            window.push_notification(Notification::warning("还不知道本机的运行目录（从没连上过代理）：关掉面板再开一次。".to_string()), cx);
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { remote::ensure_local_agent(&root) }).await;
            let _ = this.update_in(cx, |app, window, cx| {
                match result {
                    Ok(_) => app.fleet_action(FleetAction::Retry(id), window, cx),
                    Err(e) => window.push_notification(Notification::error(e).title("没拉起本机的代理"), cx),
                }
            });
        })
        .detach();
    }

    /// 自检「本机经代理」：把选中的本机那一台的数据层换成连 `127.0.0.1:<port>` 的 `AgentClient`（和真实模式同一条路，只是代理跑的是 mock）。
    pub(crate) fn attach_local_agent(&mut self, port: u16, token: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut opts = AgentClientOptions::new(LOCAL_NAME, "127.0.0.1", port, Token::new(token));
        opts.backoff = fleet_state::backoff;
        let client = AgentClient::new(opts);
        let handle = client.handle();
        let mut driver: Box<dyn StackDriver> = Box::new(client);
        let (tx, rx) = async_channel::bounded(200_000);
        driver.start(tx);
        pump_server(self.current, rx, window, cx);
        std::mem::replace(&mut self.driver, driver).shutdown(true);
        self.handles.insert(self.current, handle);
        self.fleet.reset_conn(self.current, Instant::now());
        self.st = DashState::default();
        self.logs = LogStore::default();
        self.rebuild_log_view(cx);
        cx.notify();
    }

    /// 选中那台连不上时的横幅。「重新拉起代理」只给经代理的本机那一台（直接管 stack 时没有代理可拉）。
    /// 本机的代理没了，自动重试等不回它（没人会再拉起来），横幅直说，让人点「重新拉起代理」；倒计时照旧是真的。
    pub(crate) fn connection_banner(&self, now: Instant, data_clock: Option<&str>) -> Option<fleet_state::Banner> {
        let mut b = self.fleet.banner(now, data_clock)?;
        b.relaunch = b.relaunch && self.has_handle(self.current);
        if b.relaunch && matches!(b.kind, BannerKind::Unreachable | BannerKind::NeverConnected) {
            let reason = match self.fleet.get(self.current).and_then(|s| s.conn.last_failure.as_ref()) {
                Some(Failure::Unreachable(r)) if !r.trim().is_empty() => r.trim().trim_end_matches('。').to_string(),
                _ => "没有回应".to_string(),
            };
            b.text = match (b.kind, data_clock) {
                (BannerKind::Unreachable, Some(t)) => format!("本机代理不在了（{reason}）：点「重新拉起代理」。下面是 {t} 的数据。"),
                (BannerKind::Unreachable, None) => format!("本机代理不在了（{reason}）：点「重新拉起代理」。"),
                _ => format!("本机代理没起来（{reason}）：点「重新拉起代理」。"),
            };
        }
        Some(b)
    }

    /// 选中的这台是不是在别的电脑上（经代理、又不是本机那一项）：配置抽屉的「选择…」要换成「探测」。
    pub(crate) fn current_is_remote(&self) -> bool {
        self.has_handle(self.current) && self.fleet.get(self.current).is_some_and(|s| !s.local)
    }

    /// 远程时配置抽屉的「探测」（R10）：问代理服务器上这个路径在不在、是不是目录、读不读得了。
    /// `probe_path` 是阻塞的（最多十几秒），放后台任务里调；结果写在输入框下面。
    pub(crate) fn probe_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.form_value(&self.form.root, cx);
        if path.is_empty() {
            self.root_probe = RootProbe::Done(false, "先填服务器上的工程根目录，再探测".into());
            cx.notify();
            return;
        }
        let Some(handle) = self.handles.get(&self.current).cloned() else { return };
        let server = self.current;
        self.root_probe = RootProbe::Running;
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { handle.probe_path(&path) }).await;
            let outcome = match result {
                Ok(p) if p.exists && p.is_dir && p.readable => (true, "服务器上有这个文件夹，读得了".to_string()),
                Ok(p) if p.exists && p.is_dir => (false, "服务器上有这个文件夹，但代理读不了（权限？）".to_string()),
                Ok(p) if p.exists => (false, "服务器上有这个路径，但它不是文件夹".to_string()),
                Ok(_) => (false, "服务器上没有这个路径".to_string()),
                Err(e) => (false, e),
            };
            let _ = this.update(cx, |app, cx| {
                if app.current == server {
                    app.root_probe = RootProbe::Done(outcome.0, outcome.1);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// 给 `ui\fleet.rs` 的组件的回调：点击都转到 `fleet_action`。
    pub(crate) fn fleet_handler(&self, cx: &mut Context<Self>) -> OnFleet {
        let weak = cx.entity().downgrade();
        Rc::new(move |action, window, cx| {
            let _ = weak.update(cx, |app, cx| app.fleet_action(action, window, cx));
        })
    }

    /// 「生成模型」勾选：勾上打开选库的抽屉并读库列表；去掉就关抽屉、清掉已勾的库。
    pub(crate) fn toggle_generate(&mut self, checked: bool, window: &mut Window, cx: &mut Context<Self>) {
        if checked {
            self.gen_checked = true;
            self.cmd(Command::ListDbnums);
            if self.sheet != Some(SheetKind::Generate) {
                self.open_sheet(SheetKind::Generate, window, cx);
            }
        } else {
            self.gen_checked = false;
            if self.st.job.as_ref().is_none_or(|j| j.kind != JobKind::Init) {
                self.picked.clear();
            }
            if self.sheet == Some(SheetKind::Generate) {
                self.close_sheet(window, cx);
            }
        }
        cx.notify();
    }

    /// 只给勾选的库生成。没勾「生成模型」或一个库都没选就什么都不做。
    pub(crate) fn start_generate(&mut self, cx: &mut Context<Self>) {
        if !self.gen_checked || self.picked.is_empty() {
            return;
        }
        let dbs: Vec<u32> = self.picked.iter().copied().collect();
        self.generating = dbs.clone();
        self.cmd(Command::Init(dbs));
        cx.notify();
    }

    // ───────────────────────── 抽屉和对话框 ─────────────────────────

    pub(crate) fn open_sheet(&mut self, kind: SheetKind, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = Some(kind);
        let app = cx.entity();
        let view = cx.new(|cx| SheetView::new(&app, kind, cx));
        let (title, size) = match kind {
            SheetKind::Config => ("配置工程".to_string(), px(520.)),
            SheetKind::Service(s) => (s.title().to_string(), px(560.)),
            SheetKind::Jobs => ("后端任务".to_string(), px(560.)),
            SheetKind::Generate => ("生成模型".to_string(), px(660.)),
            SheetKind::Servers => ("服务器".to_string(), px(520.)),
        };
        let weak = cx.entity().downgrade();
        window.open_sheet_at(Placement::Right, cx, move |sheet, _, _| {
            let weak = weak.clone();
            sheet
                .title(title.clone())
                .size(size)
                .resizable(false)
                .on_close(move |_, window, cx| {
                    let _ = weak.update(cx, |app, cx| {
                        // 换抽屉时旧抽屉的回调可能晚到：只收拾自己那一个
                        if app.sheet != Some(kind) {
                            return;
                        }
                        if kind == SheetKind::Generate {
                            app.gen_checked = false;
                            if app.st.job.as_ref().is_none_or(|j| j.kind != JobKind::Init) {
                                app.picked.clear();
                            }
                        }
                        if kind == SheetKind::Servers {
                            app.drawer = DrawerState::closed();
                            app.server_form.clear_token(window, cx);
                        }
                        app.sheet = None;
                        cx.notify();
                    });
                })
                .child(view.clone())
        });
        cx.notify();
    }

    pub(crate) fn open_config_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.root_probe = RootProbe::Idle;
        if let Some(p) = self.st.snapshot.as_ref().and_then(|s| s.project.clone()) {
            let pairs = [(&self.form.root, p.e3d_root), (&self.form.name, p.name), (&self.form.dir, p.dir), (&self.form.code, p.code), (&self.form.mdb, p.mdb)];
            for (state, value) in pairs {
                state.update(cx, |s, cx| s.set_value(value, window, cx));
            }
        }
        self.open_sheet(SheetKind::Config, window, cx);
    }

    pub(crate) fn open_service_sheet(&mut self, svc: Svc, window: &mut Window, cx: &mut Context<Self>) {
        self.open_sheet(SheetKind::Service(svc), window, cx);
    }

    pub(crate) fn open_jobs_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_sheet(SheetKind::Jobs, window, cx);
    }

    pub(crate) fn close_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet == Some(SheetKind::Generate) {
            self.gen_checked = false;
        }
        window.close_sheet(cx);
        self.sheet = None;
        cx.notify();
    }

    pub(crate) fn form_value(&self, which: &Entity<InputState>, cx: &App) -> String {
        which.read(cx).value().trim().to_string()
    }

    /// 「保存并检查」：先在界面上查必填和格式，过了才发 configure，结果回来再自动 doctor。
    pub(crate) fn save_and_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = ProjectForm {
            e3d_root: self.form_value(&self.form.root, cx),
            name: self.form_value(&self.form.name, cx),
            dir: self.form_value(&self.form.dir, cx),
            code: self.form_value(&self.form.code, cx),
            e3d_version: self.form.version.read(cx).selected_value().map(|v| v.to_string()).unwrap_or_else(|| "e3d31".into()),
            mdb: self.form_value(&self.form.mdb, cx),
        };
        let mut errors = Vec::new();
        if form.e3d_root.is_empty() {
            errors.push(("root", "必填：E3D 工程根目录".to_string()));
        }
        if form.name.is_empty() {
            errors.push(("name", "必填：项目名".to_string()));
        }
        if ["/", "\\", ":", ".."].iter().any(|bad| form.dir.contains(bad)) {
            errors.push(("dir", "只能写一段文件夹名，不能带 / \\ : ..".to_string()));
        }
        if form.code.parse::<u32>().is_err() {
            errors.push(("code", "项目编码要是整数".to_string()));
        }
        self.form_errors = errors;
        if self.form_errors.is_empty() {
            self.config_step = 1;
            self.st.doctor = None;
            self.st.doctor_running = true;
            self.cmd(Command::Configure(form));
        }
        let _ = window;
        cx.notify();
    }

    pub(crate) fn start_from_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_sheet(window, cx);
        self.cmd(Command::Up);
    }

    pub(crate) fn pick_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("选择 E3D 工程根目录".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                this.update_in(cx, |app, window, cx| {
                    let root = app.form.root.clone();
                    root.update(cx, |s, cx| s.set_value(p.display().to_string(), window, cx));
                })?;
            }
            anyhow::Ok(())
        })
        .detach();
    }

    pub(crate) fn open_db_dialog(&mut self, kind: DialogKind, window: &mut Window, cx: &mut Context<Self>) {
        self.picked.clear();
        self.preview = None;
        self.cmd(Command::ListDbnums);
        let app = cx.entity();
        let view = cx.new(|cx| DialogView::new(&app, kind, cx));
        let title = match kind {
            DialogKind::Init => "初始化库",
            DialogKind::Update => "增量更新",
        };
        window.open_dialog(cx, move |dialog, _, _| dialog.title(title).w(px(520.)).child(view.clone()));
    }

    pub(crate) fn write_report(&mut self, tail: &str) {
        if !tail.is_empty() {
            self.report.push(tail.to_string());
        }
        let mut text = String::from("# deploy-dash 自检记录\n\n");
        for line in &self.report {
            text += "- ";
            text += line;
            text += "\n";
        }
        let _ = std::fs::write(self.opts.work_dir.join("selftest-report.md"), text);
    }

    pub(crate) fn note(&mut self, line: impl Into<String>) {
        let line = line.into();
        tracing::info!(target: "selftest", "{line}");
        self.report.push(line);
    }
}

/// 每台服务器一个事件泵：事件带上是哪一台，再交给面板（写法同原来的泵：每 16 ms 左右一批）。
fn pump_server(id: ServerId, rx: async_channel::Receiver<UiEvent>, window: &mut Window, cx: &mut Context<DashApp>) {
    cx.spawn_in(window, async move |this, cx| {
        while let Ok(first) = rx.recv().await {
            cx.background_executor().timer(Duration::from_millis(16)).await;
            let mut batch = vec![first];
            while batch.len() < 20_000 {
                match rx.try_recv() {
                    Ok(ev) => batch.push(ev),
                    Err(_) => break,
                }
            }
            if this.update_in(cx, |app, window, cx| app.on_server_events(id, batch, window, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
}

/// 后台那几台的事件：喂它自己的 DashState（列表和汇总要用）；对话框、通知都是选中那台的事。
/// 日志照收进它自己的 LogStore：后台不拉日志，但刚切走时还在路上的那一批（例如第一次选中补的 800 行）
/// 丢了就补不回来了（代理那边的游标已经往前走了）。
fn feed_background(slot: &mut ServerSlot, batch: Vec<UiEvent>, now: Instant) {
    let st = &mut slot.st;
    for ev in batch {
        match ev {
            UiEvent::LogLines(lines) => lines.into_iter().for_each(|l| slot.logs.push(l)),
            UiEvent::Snapshot(s) => st.on_snapshot(*s, now),
            UiEvent::SnapshotFailed { reason } => {
                st.snapshot_failures += 1;
                st.last_failure = Some(reason);
            }
            UiEvent::Metrics(m) => st.on_metrics(*m),
            UiEvent::BackendJobs(j) => st.on_backend_jobs(*j),
            UiEvent::HistorySeed(seed) => st.on_seed(&seed, now),
            UiEvent::JobStarted { id, kind } => {
                st.job = Some(RunningJob { id, kind, started: now, step_started: [None; 4], step_done: [None; 4], restart_stopped: false });
            }
            UiEvent::JobFinished { id, .. } => {
                if st.job.as_ref().is_some_and(|j| j.id == id) {
                    st.job = None;
                }
            }
            _ => {}
        }
    }
}

fn field_key(field: &str) -> Option<&'static str> {
    match field {
        "project.e3d_root" => Some("root"),
        "project.name" => Some("name"),
        "project.dir" => Some("dir"),
        "project.code" => Some("code"),
        "project.mdb" => Some("mdb"),
        _ => None,
    }
}

impl Render for DashApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::new(cx);
        let now = Instant::now();
        let width = window.viewport_size().width.as_f32();
        let layout = fleet_state::layout(width, self.fleet.len(), self.panel.list_collapsed);
        let (narrow, wide) = (layout.tier.narrow(), layout.tier.wide());
        let on = self.fleet_handler(cx);
        let hover = self.hover_ui();
        let upper = crate::ui::kpi::TILE_H + 12.0 + crate::ui::services::TABLE_H;
        if self.upper_h == 0.0 {
            self.upper_h = upper;
        }
        // 经代理（或多台）时，选中那台连不上用 fleet 的横幅（点名哪台、什么原因、重试 / 编辑服务器… / 重新拉起代理）；
        // 只有一台本机直连 stack 时照旧是「拿不到状态」横幅
        let data_clock = self.st.snapshot.as_ref().map(|s| crate::ui::format::clock(s.generated_at));
        let fleet_banner = self
            .connection_banner(now, data_clock.as_deref())
            .filter(|_| self.fleet.len() > 1 || self.has_handle(self.current))
            .map(|b| fleet::unreachable_banner(&t, self.current, &b, &on));
        let banner = fleet_banner.or_else(|| self.render_error_banner(&t, now, cx));
        let dim = self.st.stale(now) || self.st.snapshot_failures >= 2 || self.fleet.selected_down(now);
        let skeleton = self.fleet.needs_skeleton() && self.st.snapshot.is_none();
        let upper_view = v_flex()
            .id("upper")
            .size_full()
            .overflow_y_scroll()
            .gap_3()
            .when(dim, |this| this.opacity(fleet::DIM_OPACITY))
            .when(skeleton, |this| this.child(fleet::first_connect_skeleton(&t)))
            .when(!skeleton, |this| {
                this.child(self.render_kpis(&t, now, wide, cx)).child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_start()
                        .child(div().flex_1().min_w_0().child(self.render_services(&t, now, narrow, window, cx)))
                        .when(wide, |this| this.child(div().w(px(layout.main_w * 0.30)).child(self.render_jobs_panel(&t, now, cx)))),
                )
            });
        let logs = self.render_logs(&t, narrow, cx);
        let main = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_header(&t, now, narrow, window, cx))
            .child(self.render_controls(&t, cx))
            .when_some(banner, |this, b| this.child(b))
            .child(
                div().flex_1().min_h_0().px_4().pt_3().pb_3().child(
                    v_resizable("dash-split")
                        .child(resizable_panel().size(px(upper + 12.0)).size_range(px(120.)..px(upper + 12.0)).fixed().child(div().size_full().pb_3().child(upper_view)))
                        .child(resizable_panel().child(logs)),
                ),
            )
            .child(self.render_status_bar(&t, cx));
        v_flex()
            .size_full()
            .bg(t.page)
            .text_color(t.ink)
            .on_action(cx.listener(|this, _: &fleet::ToggleServerList, window, cx| this.fleet_action(FleetAction::ToggleList, window, cx)))
            .on_action(cx.listener(|this, _: &fleet::PrevServer, window, cx| this.step_server(-1, window, cx)))
            .on_action(cx.listener(|this, _: &fleet::NextServer, window, cx| this.step_server(1, window, cx)))
            .on_action(cx.listener(|this, _: &fleet::Server1, window, cx| this.select_nth_server(1, window, cx)))
            .on_action(cx.listener(|this, _: &fleet::Server2, window, cx| this.select_nth_server(2, window, cx)))
            .on_action(cx.listener(|this, _: &fleet::Server3, window, cx| this.select_nth_server(3, window, cx)))
            .on_action(cx.listener(|this, _: &fleet::Server4, window, cx| this.select_nth_server(4, window, cx)))
            .on_action(cx.listener(|this, _: &fleet::Server5, window, cx| this.select_nth_server(5, window, cx)))
            .on_action(cx.listener(|this, _: &fleet::Server6, window, cx| this.select_nth_server(6, window, cx)))
            .on_action(cx.listener(|this, _: &fleet::Server7, window, cx| this.select_nth_server(7, window, cx)))
            .on_action(cx.listener(|this, _: &fleet::Server8, window, cx| this.select_nth_server(8, window, cx)))
            .on_action(cx.listener(|this, _: &fleet::Server9, window, cx| this.select_nth_server(9, window, cx)))
            .child(self.render_title_bar(&t, cx))
            .when(layout.summary_bar, |this| this.child(fleet::summary_bar(&t, &self.fleet.bar(now), hover, &on)))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(fleet::server_list(&t, &self.fleet.rows(now), layout.list, hover, Some(&self.list_focus), &on))
                    .child(main),
            )
    }
}

impl DashApp {
    fn render_title_bar(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        // 经代理、还没拿到过快照的那台（连不上、正在连）不知道配没配置，不写「未配置」
        let unknown = if self.has_handle(self.current) { "—" } else { "未配置" };
        let name = self.st.snapshot.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| unknown.into());
        let root = self.root_dir();
        TitleBar::new().child(
            h_flex()
                .w_full()
                .pr_2()
                .gap_2()
                .items_center()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("AIOS 部署面板"))
                .when(self.fleet.len() > 1 || self.has_handle(self.current), |this| {
                    let name = self.fleet.get(self.current).map(|s| s.name.clone()).unwrap_or_default();
                    this.child(div().text_sm().font_weight(FontWeight::MEDIUM).child(format!("· {name}")))
                })
                .child(
                    div()
                        .id("stack-name")
                        .text_sm()
                        .text_color(t.muted)
                        .child(format!("· {name}"))
                        .when(!root.is_empty() && self.hover_ui(), |this| {
                            let tip = format!("运行目录 {root}");
                            this.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                        }),
                )
                .child(
                    gpui_kit::component::tag::Tag::secondary()
                        .outline()
                        .child(self.driver.source_label()),
                )
                .child(div().flex_1())
                .child(
                    gpui_kit::component::button::Button::new("theme")
                        .ghost()
                        .icon(if self.dark { gpui_kit::component::IconName::Sun } else { gpui_kit::component::IconName::Moon })
                        .accessibility_label(if self.dark { "切到浅色" } else { "切到深色" })
                        .when(self.hover_ui(), |b| b.tooltip(if self.dark { "切到浅色" } else { "切到深色" }))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_theme(window, cx))),
                ),
        )
        .into_any_element()
    }

    fn render_status_bar(&self, t: &Tokens, _cx: &mut Context<Self>) -> AnyElement {
        let snap = self.st.snapshot.as_ref();
        let agent = self
            .fleet
            .get(self.current)
            .and_then(|s| s.conn.agent_version.clone().map(|v| (v, s.conn.rtt_ms)))
            .map(|(v, rtt)| format!(" · 代理 {v}{}", rtt.map(|ms| format!(" · 往返 {ms} ms")).unwrap_or_default()))
            .unwrap_or_default();
        let left = match snap {
            Some(s) => format!(
                "运行目录 {} · stack {} · 面板 {} · 后端 {} · 起停时每 1 秒、平时每 2 秒刷新{agent}",
                s.root, s.stack_version, PANEL_VERSION, s.backend_exe_note
            ),
            None => format!("面板 {PANEL_VERSION} · 正在读取状态{agent}"),
        };
        let dropped = self.dropped.load(std::sync::atomic::Ordering::Relaxed);
        let right = format!(
            "{} · 面板内存 {:.0} MB · 日志 {} 行{}",
            self.driver.source_label(),
            self.mem_mb,
            crate::ui::format::thousands(self.logs.kept() as u64),
            if dropped > 0 { format!("（通道丢弃 {dropped}）") } else { String::new() }
        );
        StatusBar::new()
            .left(div().text_xs().text_color(t.muted).child(left))
            .right(div().text_xs().text_color(t.muted).child(right))
            .into_any_element()
    }
}

/// `AgentClient` 借 `Notice` 报的连接状态（见 agent-client 的 `PATCH-events.md`）：第一次连上、连不上、令牌被拒、版本对不上。
/// 选中那台的横幅都说了（带真的倒计时和该点的按钮），再弹一条是重复；「连不上」那条还说「过一会儿自动重试」，
/// 对本机那台不对（代理没了不会自己回来）。只留「又连上」。
fn is_connection_notice(ev: &UiEvent) -> bool {
    match ev {
        UiEvent::Notice { kind: NoticeKind::Info, title, .. } => title.starts_with("已连上 "),
        UiEvent::Notice { kind: NoticeKind::Error, title, .. } => title.starts_with("连不上 ") || title.ends_with(" 拒绝了令牌") || title == "代理版本对不上",
        _ => false,
    }
}

/// `AgentClient` 的原因以「连不上 主机:端口: 」开头；横幅和测试结果自己会说是哪一台，去掉这一截免得说两遍。
fn bare_reason(reason: String) -> String {
    match reason.strip_prefix("连不上 ").and_then(|r| r.split_once(": ")) {
        Some((who, rest)) if !who.contains(' ') && !rest.is_empty() => rest.to_string(),
        _ => reason,
    }
}

fn reported_of(state: ConnState) -> Reported {
    match state {
        ConnState::Connecting => Reported::Connecting,
        ConnState::Connected { agent_version, rtt_ms, .. } => Reported::Connected { agent_version, rtt_ms: rtt_ms.min(u32::MAX as u64) as u32 },
        ConnState::Unreachable { reason } => Reported::Down(Failure::Unreachable(bare_reason(reason))),
        ConnState::Unauthorized => Reported::Down(Failure::Unauthorized),
        ConnState::Incompatible { agent_version, schema } => Reported::Down(Failure::Incompatible { agent_version, schema }),
    }
}

/// 后台那几台的 `summary=1`。`stack.first_run`、`stack.lock_command` 是 agent-server 22:02 只加的字段，老代理不发（读成 false / 空串）。
fn summary_from_proto(s: &deploy_dash_proto::StatusSummary) -> fleet_state::Summary {
    use deploy_dash_proto as proto;
    let mut services = [None; 4];
    for x in &s.services {
        let Some(i) = x.name.index() else { continue };
        let state = match x.state {
            proto::SvcState::Ok => SvcState::Ok,
            proto::SvcState::Stopped => SvcState::Stopped,
            proto::SvcState::Waiting => SvcState::Waiting,
            proto::SvcState::Disabled => SvcState::Disabled,
            proto::SvcState::Bad | proto::SvcState::Unknown => SvcState::Bad,
        };
        let reason = match x.reason_code {
            proto::ReasonCode::NotRunning => ReasonCode::NotRunning,
            proto::ReasonCode::HealthFailed => ReasonCode::HealthFailed,
            proto::ReasonCode::PortTaken => ReasonCode::PortTaken,
            proto::ReasonCode::Unreadable => ReasonCode::Unreadable,
            proto::ReasonCode::WaitingStorageVersion => ReasonCode::WaitingStorageVersion,
            proto::ReasonCode::None | proto::ReasonCode::Unknown => ReasonCode::None,
        };
        services[i] = Some(fleet_state::SvcLite { state, reason, pid: None });
    }
    let disk = |role: &str| s.host.as_ref().and_then(|h| h.disks.iter().find(|d| d.role.split(',').any(|r| r.trim() == role)));
    fleet_state::Summary {
        stack_name: s.stack.name.clone(),
        first_run: s.stack.first_run,
        lock_command: (!s.stack.lock_command.is_empty()).then(|| s.stack.lock_command.clone()),
        job: s.job.as_ref().filter(|j| j.state.is_active()).map(|j| fleet_state::JobLite {
            label: j.label.clone(),
            lifecycle: j.kind.is_lifecycle(),
            stopping: matches!(j.kind, proto::JobKindName::Down | proto::JobKindName::Stop),
        }),
        services,
        commit_free_gb: s.host.as_ref().map(|h| h.commit_free_gb),
        disk_drive: disk("root").map(|d| d.drive.clone()).unwrap_or_default(),
        disk_free_gb: disk("root").map(|d| d.free_gb),
        sys_disk_free_gb: disk("system").map(|d| d.free_gb),
        ..Default::default()
    }
}

/// 建一台远程服务器的数据层：退避用 `fleet_state::backoff`（全项目一份）；令牌只在这里从 `Secret` 交给 `Token`。
/// 令牌解不开或没填也照建（拿空令牌连，代理回 401，列表里显示「令牌不对」，点编辑重新输入）。
fn connect_remote(id: ServerId, entry: &ServerEntry, focus: Focus, window: &mut Window, cx: &mut Context<DashApp>) -> (ServerSlot, AgentHandle) {
    let token = Token::new(entry.token.secret().map(|s| s.expose()).unwrap_or_default());
    let mut opts = AgentClientOptions::new(entry.name.clone(), entry.address.clone(), entry.port, token);
    opts.backoff = fleet_state::backoff;
    let client = AgentClient::new(opts);
    let handle = client.handle();
    handle.set_focus(focus);
    let mut driver: Box<dyn StackDriver> = Box::new(client);
    let (tx, rx) = async_channel::bounded(200_000);
    driver.start(tx);
    pump_server(id, rx, window, cx);
    (ServerSlot { driver, st: DashState::default(), logs: LogStore::default() }, handle)
}
