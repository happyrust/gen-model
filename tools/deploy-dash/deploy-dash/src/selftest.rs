//! `--selftest`：一次开窗，按顺序走完各个状态并截图（DASHBOARD-DESIGN 16.4），对照第 3.4 节的已知答案，
//! 写 `selftest-report.md` 后退出。窗口不抢焦点；截图用 PrintWindow，按 DWM 可见边框裁掉系统的不可见边。
//! 已知答案读的是渲染用的同一份派生状态（GPUI 里没有现成办法读回画面上的文字），这一点在报告里写明。
//!
//! 另两种走法（J3）：`--selftest --selftest-via-agent` 先起一台 mock 代理、把本机那一台换成经代理，再走同一套已知答案；
//! `--selftest --selftest-fleet` 走多服务器：本机（模拟数据）+ 两台 mock 代理 + 一台连不上，三种宽度 × 亮暗截图。
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use gpui_kit::component::{Theme, ThemeMode, WindowExt as _};
use gpui_kit::*;

use crate::app::DashApp;
use crate::data::remote::Focus;
use crate::data::{Command, Fault, Fixture};
use crate::events::LogSource;
use crate::fleet_state::{self, BannerKind, Health, ListMode, Probe, ServerId, TestState, Tier};
use crate::model::{JobKind, Svc, SvcState};
use crate::state::{LogFilter, Overall, RowKind};
use crate::ui::fleet::FleetAction;
use crate::winsys;

async fn sleep(cx: &mut AsyncWindowContext, ms: u64) {
    cx.background_executor().timer(Duration::from_millis(ms)).await;
}

async fn wait_until(this: &WeakEntity<DashApp>, cx: &mut AsyncWindowContext, timeout: Duration, f: impl Fn(&DashApp) -> bool) -> anyhow::Result<bool> {
    let t = Instant::now();
    loop {
        if this.update(cx, |app, _| f(app))? {
            return Ok(true);
        }
        if t.elapsed() > timeout {
            return Ok(false);
        }
        cx.background_executor().timer(Duration::from_millis(100)).await;
    }
}

async fn shot(this: &WeakEntity<DashApp>, cx: &mut AsyncWindowContext, path: PathBuf) -> anyhow::Result<()> {
    if winsys::restore_main_window_no_activate() {
        sleep(cx, 600).await;
    }
    // 通知浮层会盖住右上角的磁贴和按钮：截图前清掉
    this.update_in(cx, |_, window, cx| window.clear_notifications(cx))?;
    sleep(cx, 350).await;
    let p = path.clone();
    let msg = cx
        .background_executor()
        .spawn(async move {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            match winsys::own_main_window() {
                Some(w) => match winsys::capture_window_png(w.hwnd, &p) {
                    Ok((w, h, m)) => format!("截图 {name} {w}x{h}（{m}）"),
                    Err(e) => format!("截图 {name} 失败：{e}"),
                },
                None => format!("截图 {name} 失败：找不到主窗口"),
            }
        })
        .await;
    this.update(cx, |app, _| app.note(msg))?;
    Ok(())
}

fn check(app: &mut DashApp, label: &str, ok: bool, detail: String) {
    app.note(format!("{} {label}：{detail}", if ok { "通过" } else { "不符" }));
}

pub async fn run(this: WeakEntity<DashApp>, cx: &mut AsyncWindowContext) -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--selftest-fleet") {
        return run_fleet(this, cx).await;
    }
    // 本机经代理：代理活到这次自检结束（local 丢掉时结束它、删掉它的根）
    let mut local = if args.iter().any(|a| a == "--selftest-via-agent") { Some(attach_mock_agent(&this, cx).await?) } else { None };
    let shots = this.update(cx, |app, _| app.opts.shots_dir.clone())?;
    std::fs::create_dir_all(&shots).ok();
    let started = Instant::now();
    sleep(cx, 1500).await;

    for (theme, dark) in [("light", false), ("dark", true)] {
        let name = |n: &str| shots.join(format!("dash-{theme}-{n}.png"));
        this.update_in(cx, |app, window, cx| {
            app.dark = dark;
            Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, Some(window), cx);
            app.cmd(Command::Fixture(Fixture::Unconfigured));
            app.set_log_source(0, cx);
            app.config_step = 0;
            for s in [&app.form.root, &app.form.name, &app.form.dir, &app.form.code, &app.form.mdb] {
                s.update(cx, |s, cx| s.set_value("", window, cx));
            }
        })?;
        sleep(cx, 1400).await;

        // 01 未配置
        this.update(cx, |app, _| {
            let o = app.st.overall(Instant::now());
            check(app, "未配置首屏", o == Overall::Unconfigured, format!("整体状态 {o:?}"));
        })?;
        shot(&this, cx, name("01-unconfigured")).await?;

        // 02 配置抽屉（填好六项）
        this.update_in(cx, |app, window, cx| {
            app.open_config_sheet(window, cx);
            let values = [
                (app.form.root.clone(), "D:\\AVEVA\\Projects\\E3D3.1"),
                (app.form.name.clone(), "AvevaMarineSample"),
                (app.form.dir.clone(), ""),
                (app.form.code.clone(), "1516"),
                (app.form.mdb.clone(), "ALL"),
            ];
            for (s, v) in values {
                s.update(cx, |s, cx| s.set_value(v, window, cx));
            }
        })?;
        sleep(cx, 900).await;
        shot(&this, cx, name("02-config-sheet")).await?;

        // 保存并检查 → doctor
        this.update_in(cx, |app, window, cx| app.save_and_check(window, cx))?;
        let doctor = wait_until(&this, cx, Duration::from_secs(8), |a| a.st.doctor.is_some() && !a.st.doctor_running).await?;
        this.update(cx, |app, _| {
            let can = app.st.doctor.as_ref().is_some_and(|d| d.can_start);
            check(app, "保存并检查", doctor && can, format!("检查结果{}，可以启动：{can}", if doctor { "已回来" } else { "超时" }));
        })?;
        sleep(cx, 400).await;

        // 03 启动中
        this.update_in(cx, |app, window, cx| app.start_from_sheet(window, cx))?;
        let mid = wait_until(&this, cx, Duration::from_secs(8), |a| a.st.row_kind(Svc::Backend) == RowKind::Starting).await?;
        sleep(cx, 1200).await;
        this.update(cx, |app, _| {
            let o = app.st.overall(Instant::now());
            check(app, "启动中", mid && o == Overall::Starting, format!("后端在起：{mid}；整体状态 {o:?}"));
        })?;
        shot(&this, cx, name("03-starting")).await?;
        let up = wait_until(&this, cx, Duration::from_secs(20), |a| a.st.overall(Instant::now()) == Overall::AllOk).await?;
        this.update(cx, |app, _| {
            let (ok, all) = app.st.counts();
            check(app, "一键启动按顺序起完", up && ok == all, format!("{ok}/{all} 正常"));
        })?;

        // 04 全部正常（固定场景：稳定运行 2 小时 14 分，带 15 分钟历史）
        this.update(cx, |app, _| app.cmd(Command::Fixture(Fixture::RunningHealthy)))?;
        sleep(cx, 2600).await;
        this.update(cx, |app, _| {
            let now = Instant::now();
            let o = app.st.overall(now);
            let (ok, all) = app.st.counts();
            check(app, "全部正常", o == Overall::AllOk && ok == 4 && all == 4, format!("整体 {o:?}，{ok}/{all}"));
        })?;
        shot(&this, cx, name("04-all-ok")).await?;

        // 05 部分异常：注入「现场故障」，再点 rs-server 那一行（日志筛到它）
        this.update(cx, |app, cx| app.inject(Fault::FieldIncident, cx))?;
        sleep(cx, 1600).await;
        this.update(cx, |app, cx| app.view_logs(Svc::RsServer, cx))?;
        sleep(cx, 1400).await;
        this.update(cx, |app, _| {
            let now = Instant::now();
            let o = app.st.overall(now);
            let (ok, all) = app.st.counts();
            let p = app.st.problems(now);
            let top = p.first().map(|x| x.text.clone()).unwrap_or_default();
            check(app, "部分异常·顶栏", o == Overall::Partial { red: true } && ok == 3 && all == 4 && top == "rs-server 异常 · 进程不在", format!("整体 {o:?}，{ok}/{all}，最要紧的问题「{top}」"));
            let d = app.st.disk_eval(now);
            let free = d.free.unwrap_or(0.0);
            let m = d.minutes_to_red.unwrap_or(0);
            check(app, "部分异常·磁盘", (11.9..=12.05).contains(&free) && (48..=51).contains(&m), format!("可用 {free:.2} GB，约 {m} 分钟到红线"));
            let j = app.st.backend_jobs.as_ref();
            let (r, q, f) = j.map(|j| (j.running(), j.queued(), j.failures_24h.len())).unwrap_or((0, 0, 0));
            check(app, "部分异常·后端任务", (r, q, f) == (1, 2, 2), format!("在跑 {r}、排队 {q}、失败 {f}"));
            let ticks = app.st.rows[Svc::RsServer.index()].buckets(now, 30);
            let red_tail = ticks.iter().rev().take_while(|k| **k == Some(RowKind::Bad)).count();
            check(app, "部分异常·rs-server 刻度", red_tail == 3, format!("最后 {red_tail} 格是异常"));
            let mem_row = app.st.metrics.as_ref().and_then(|m| m.per_svc[Svc::Backend.index()]).map(|p| p.mem_gb);
            let mem_tile = app.st.mem_eval().backend_gb;
            check(app, "部分异常·后端内存两处一致", mem_row == mem_tile && mem_row.is_some(), format!("行里 {mem_row:?}，磁贴 {mem_tile:?}"));
            check(app, "部分异常·日志已筛到 rs-server", app.log_filter.source == 3, format!("来源 {}", app.log_filter.source));
        })?;
        shot(&this, cx, name("05-partial-fault")).await?;

        // 06 服务详情抽屉
        this.update_in(cx, |app, window, cx| app.open_service_sheet(Svc::RsServer, window, cx))?;
        sleep(cx, 1000).await;
        shot(&this, cx, name("06-service-sheet")).await?;
        this.update_in(cx, |app, window, cx| {
            window.close_sheet(cx);
            app.sheet = None;
        })?;
        sleep(cx, 400).await;
    }

    // 1280×800：最小窗口下的部分异常
    this.update_in(cx, |app, window, cx| {
        app.dark = false;
        Theme::change(ThemeMode::Light, Some(window), cx);
        window.resize(size(px(1280.), px(800.)));
        app.cmd(Command::Fixture(Fixture::RunningHealthy));
    })?;
    sleep(cx, 1500).await;
    this.update(cx, |app, cx| {
        app.inject(Fault::FieldIncident, cx);
    })?;
    sleep(cx, 1500).await;
    this.update(cx, |app, cx| app.view_logs(Svc::RsServer, cx))?;
    sleep(cx, 1400).await;
    shot(&this, cx, shots.join("dash-light-1280-partial-fault.png")).await?;

    // 现场注入：从全部正常注入「rs-server 退出」，看面板进入部分异常
    this.update(cx, |app, _| app.cmd(Command::Fixture(Fixture::RunningHealthy)))?;
    sleep(cx, 1500).await;
    this.update(cx, |app, cx| app.inject(Fault::RsServerExit, cx))?;
    let flipped = wait_until(&this, cx, Duration::from_secs(6), |a| a.st.overall(Instant::now()) == Overall::Partial { red: true }).await?;
    let via_agent = local.is_some();
    this.update(cx, |app, _| {
        check(app, "现场注入 rs-server 退出", flipped, "全部正常 → 部分异常".to_string());
        if via_agent {
            let popped = app.notice_titles.iter().filter(|t| t.starts_with("已连上 ")).count();
            check(app, "本机经代理：开面板不弹「已连上」", popped == 0, format!("弹过 {} 条提示，其中「已连上」{popped} 条", app.notice_titles.len()));
        }
    })?;
    if let Some((_, agent)) = local.as_mut() {
        polish_checks(&this, cx, agent, &shots).await?;
    }
    this.update(cx, |app, _| {
        let (p, w) = winsys::process_memory_mb();
        app.note(format!("自检用时 {:.1} 秒；结束时面板私有内存 {p:.1} MB，工作集 {w:.1} MB", started.elapsed().as_secs_f64()));
        app.note("已知答案读的是渲染用的同一份派生状态，不是从画面上读回的文字".to_string());
        app.write_report("");
    })?;
    sleep(cx, 300).await;
    Ok(())
}

// ───────────────────────── 多服务器（J3）─────────────────────────

/// 两台 mock 代理的端口（派工指定，只听 127.0.0.1），和一个没人听的端口（「连不上」那台）。
const PORT_B: u16 = 18601;
const PORT_C: u16 = 18602;
const PORT_NOBODY: u16 = 18609;

/// 自检起的 mock 代理：`deploy-dash.exe --agent --mock`，只听 127.0.0.1。令牌在它自己的根里，只读进内存交给面板；
/// 丢掉时结束进程、连根删掉。面板进程意外退出时由 Job 一起结束。
struct MockAgent {
    child: std::process::Child,
    root: PathBuf,
    port: u16,
    scenario: Option<&'static str>,
}

impl MockAgent {
    fn spawn(job: &winsys::Job, root: PathBuf, port: u16, scenario: Option<&'static str>, ping_schema: Option<u32>) -> anyhow::Result<Self> {
        let gui = root.join("run").join("gui");
        std::fs::create_dir_all(&gui)?;
        if let Some(s) = ping_schema {
            std::fs::write(gui.join("agent.toml"), format!("mock_ping_schema = {s}\n"))?;
        }
        let child = Self::start(&root, port, scenario)?;
        job.assign(&child).ok();
        Ok(Self { child, root, port, scenario })
    }

    fn start(root: &Path, port: u16, scenario: Option<&str>) -> anyhow::Result<std::process::Child> {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new(std::env::current_exe()?);
        cmd.arg("--agent").arg("--mock").arg("--root").arg(root).arg("--listen").arg(format!("127.0.0.1:{port}"));
        if let Some(s) = scenario {
            cmd.arg("--scenario").arg(s);
        }
        Ok(cmd.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?)
    }

    fn token(&self) -> String {
        std::fs::read_to_string(self.root.join("run").join("gui").join("agent.token")).map(|t| t.trim().to_string()).unwrap_or_default()
    }

    fn ready(&self) -> bool {
        winsys::listening_port_owners().get(&self.port) == Some(&self.child.id()) && !self.token().is_empty()
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// 同根、同端口、同令牌再起一次（「停一台再起来」）。
    fn restart(&mut self, job: &winsys::Job) -> anyhow::Result<()> {
        self.kill();
        self.child = Self::start(&self.root, self.port, self.scenario)?;
        job.assign(&self.child).ok();
        Ok(())
    }
}

impl Drop for MockAgent {
    fn drop(&mut self) {
        self.kill();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn wait_agent(cx: &mut AsyncWindowContext, agent: &MockAgent, timeout: Duration) -> bool {
    let t = Instant::now();
    while t.elapsed() < timeout {
        if agent.ready() {
            return true;
        }
        sleep(cx, 100).await;
    }
    false
}

/// 比 `wait_until` 细（每 5 毫秒看一次），回来的是从 `since` 起用了多久：量切换耗时用。
async fn wait_fast(this: &WeakEntity<DashApp>, cx: &mut AsyncWindowContext, since: Instant, timeout: Duration, f: impl Fn(&DashApp) -> bool) -> anyhow::Result<Option<Duration>> {
    loop {
        if this.update(cx, |app, _| f(app))? {
            return Ok(Some(since.elapsed()));
        }
        if since.elapsed() > timeout {
            return Ok(None);
        }
        sleep(cx, 5).await;
    }
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

/// 「本机经代理」：在 18601 起一台 mock 代理（缺省「未配置」），把本机那一台换成连它。
async fn attach_mock_agent(this: &WeakEntity<DashApp>, cx: &mut AsyncWindowContext) -> anyhow::Result<(winsys::Job, MockAgent)> {
    if winsys::listening_ports().contains(&PORT_B) {
        anyhow::bail!("端口 {PORT_B} 有人在听，不起 mock 代理");
    }
    let work = this.update(cx, |app, _| app.opts.work_dir.clone())?;
    let job = winsys::Job::kill_on_close()?;
    let root = work.join("agent-local");
    let _ = std::fs::remove_dir_all(&root);
    let agent = MockAgent::spawn(&job, root, PORT_B, None, None)?;
    if !wait_agent(cx, &agent, Duration::from_secs(15)).await {
        anyhow::bail!("mock 代理 15 秒内没起来");
    }
    let token = agent.token();
    this.update_in(cx, |app, window, cx| {
        app.attach_local_agent(PORT_B, &token, window, cx);
        app.note(format!("本机经代理：127.0.0.1:{PORT_B} 上的 mock 代理（deploy-dash --agent --mock），下面的已知答案和直接模拟时同一套"));
    })?;
    Ok((job, agent))
}

/// J3 之后的打磨（J4 实测 8.4 节）：本机经 mock 代理时，
/// ① 起停任务跑到一半「再开面板」，「用时」从代理任务表里的开始时刻算；
/// ② 结束本机代理：「启动中 N 秒」「用时」定格、「一键启动」不转圈；
/// ③ 横幅直说「本机代理不在了」并给「重新拉起代理」（不点：它会拉起真实模式的代理）；不弹「过一会儿自动重试」那条。
async fn polish_checks(this: &WeakEntity<DashApp>, cx: &mut AsyncWindowContext, agent: &mut MockAgent, shots: &Path) -> anyhow::Result<()> {
    this.update(cx, |app, _| app.cmd(Command::Fixture(Fixture::Stopped)))?;
    wait_until(this, cx, Duration::from_secs(10), |a| a.st.overall(Instant::now()) == Overall::Stopped && a.st.job.is_none()).await?;
    let t_up = Instant::now();
    this.update(cx, |app, _| app.cmd(Command::Up))?;
    wait_until(this, cx, Duration::from_secs(8), |a| a.st.lifecycle_job().is_some()).await?;
    sleep(cx, 2500).await;

    // ① 「再开面板」：换一个新的数据层连同一个代理（状态清空），它第一次看到的任务早就开始了
    let token = agent.token();
    this.update_in(cx, |app, window, cx| app.attach_local_agent(agent.port, &token, window, cx))?;
    let seen = wait_until(this, cx, Duration::from_secs(8), |a| a.st.lifecycle_job().is_some() && a.st.job_progress(Instant::now()).is_some()).await?;
    sleep(cx, 1200).await;
    this.update(cx, |app, _| {
        let since_up = t_up.elapsed();
        let shown = app.st.job_progress(Instant::now()).map(|p| p.elapsed).unwrap_or_default();
        let fine = seen && shown + Duration::from_millis(1500) >= since_up && shown <= since_up + Duration::from_secs(1);
        check(app, "再开面板后「用时」从任务开始算", fine, format!("任务开始 {:.1} 秒了，顶栏「用时」{:.1} 秒", since_up.as_secs_f64(), shown.as_secs_f64()));
    })?;

    // ② 结束本机代理，等判成拿不到状态
    agent.kill();
    let t_kill = Instant::now();
    let lost = wait_until(this, cx, Duration::from_secs(20), |a| a.st.snapshot_failures >= 2 && a.fleet.selected_down(Instant::now())).await?;
    let timers = |app: &DashApp| {
        let now = Instant::now();
        let clock = app.st.clock(now);
        let rows: Vec<Option<Duration>> = Svc::ALL.iter().map(|s| app.st.lifecycle_job().and_then(|j| j.step_started[s.index()]).map(|t0| clock.saturating_duration_since(t0))).collect();
        (rows, app.st.job_progress(now).map(|p| p.elapsed), app.start_spinning(now))
    };
    let first = this.update(cx, |app, _| timers(app))?;
    sleep(cx, 2500).await;
    let second = this.update(cx, |app, _| timers(app))?;
    this.update(cx, |app, _| {
        let starting = first.0.iter().flatten().count();
        let frozen = lost && first.0 == second.0 && first.1 == second.1 && starting > 0;
        check(app, "拿不到状态时「启动中 N 秒」「用时」定格", frozen, format!("结束代理 {:.1} 秒后判拿不到状态；隔 2.5 秒两次读数一样：{}；在起的行 {starting} 个，用时 {:?}", t_kill.elapsed().as_secs_f64(), first.0 == second.0 && first.1 == second.1, first.1.map(|d| format!("{:.1} 秒", d.as_secs_f64()))));
        check(app, "拿不到状态时「一键启动」不转圈", lost && !first.2 && !second.2 && app.st.job.is_some(), "任务还记着，按钮不转".into());
        let ban = app.connection_banner(Instant::now(), None);
        let fine = ban.as_ref().is_some_and(|b| b.relaunch && b.text.starts_with("本机代理不在了") && b.retry_in.is_some());
        check(app, "本机代理不在了：横幅直说并给「重新拉起代理」", fine, ban.map(|b| format!("「{}」，{}", b.text, if b.relaunch { "有「重新拉起代理」" } else { "没有「重新拉起代理」" })).unwrap_or("没有横幅".into()));
        let popped: Vec<&String> = app.notice_titles.iter().filter(|t| t.starts_with("连不上 ")).collect();
        check(app, "不弹「过一会儿自动重试」那条", popped.is_empty(), format!("弹过的提示 {:?}", app.notice_titles));
    })?;
    std::fs::create_dir_all(shots.join("polish")).ok();
    shot(this, cx, shots.join("polish").join("local-agent-gone.png")).await?;
    Ok(())
}

fn fill_server(app: &DashApp, name: &str, address: &str, port: u16, token: &str, window: &mut Window, cx: &mut Context<DashApp>) {
    let f = &app.server_form;
    let port = port.to_string();
    for (input, value) in [(f.name.clone(), name), (f.address.clone(), address), (f.port.clone(), port.as_str()), (f.token.clone(), token)] {
        input.update(cx, |s, cx| s.set_value(value.to_string(), window, cx));
    }
}

/// 点「测试连接」，等结果；回来的是结果和抽屉里显示的那句话。
async fn test_probe(this: &WeakEntity<DashApp>, cx: &mut AsyncWindowContext) -> anyhow::Result<(Option<Probe>, String)> {
    this.update_in(cx, |app, window, cx| app.fleet_action(FleetAction::Test, window, cx))?;
    wait_until(this, cx, Duration::from_secs(20), |a| !matches!(a.drawer.test, TestState::Running)).await?;
    this.update(cx, |app, _| match &app.drawer.test {
        TestState::Done(p) => (Some(p.clone()), fleet_state::probe_text(p, &app.drawer.test_endpoint).1),
        _ => (None, format!("没测成：{:?}", app.drawer.errors.iter().map(|(_, m)| m.as_str()).collect::<Vec<_>>())),
    })
}

/// 选中那台日志里不是面板自己写的行（服务、任务）。
fn agent_lines(app: &DashApp) -> usize {
    app.logs.collect(&LogFilter::default(), usize::MAX).iter().filter(|l| !matches!(l.source, LogSource::Panel)).count()
}

fn id_of(app: &DashApp, name: &str) -> Option<ServerId> {
    app.fleet.servers().iter().find(|s| s.name == name).map(|s| s.id)
}

/// 多服务器：本机（模拟数据）+ 乙、丙两台 mock 代理 + 连不上的丁。J2 的验收逐条在这里核，截图三种宽度 × 亮暗。
pub async fn run_fleet(this: WeakEntity<DashApp>, cx: &mut AsyncWindowContext) -> anyhow::Result<()> {
    let (work, shots, n0) = this.update(cx, |app, _| (app.opts.work_dir.clone(), app.opts.shots_dir.clone(), app.fleet.len()))?;
    std::fs::create_dir_all(&shots).ok();
    let started = Instant::now();
    if n0 != 1 {
        this.update(cx, |app, _| app.write_report("工作目录里已经有服务器列表：多服务器自检要一个新的 --work-dir"))?;
        return Ok(());
    }
    let busy = winsys::listening_ports();
    if let Some(p) = [PORT_B, PORT_C, PORT_NOBODY].into_iter().find(|p| busy.contains(p)) {
        this.update(cx, |app, _| app.write_report(&format!("端口 {p} 有人在听：不起 mock 代理，多服务器自检没跑")))?;
        return Ok(());
    }
    let job = winsys::Job::kill_on_close()?;
    let roots = work.join("agents");
    let _ = std::fs::remove_dir_all(&roots);
    let name = |n: &str| shots.join(format!("fleet-{n}.png"));
    let local = this.update_in(cx, |app, window, cx| {
        app.dark = false;
        Theme::change(ThemeMode::Light, Some(window), cx);
        window.resize(size(px(1440.), px(900.)));
        app.cmd(Command::Fixture(Fixture::RunningHealthy));
        app.current
    })?;
    sleep(cx, 1500).await;

    // 1 「测试连接」认出老代理：18602 上起一台 /v1/ping 报协议 0 的 mock 代理，测完结束、删根
    let old_text = {
        let old = MockAgent::spawn(&job, roots.join("old"), PORT_C, Some("healthy"), Some(0))?;
        let up = wait_agent(cx, &old, Duration::from_secs(15)).await;
        let token = old.token();
        this.update_in(cx, |app, window, cx| {
            app.fleet_action(FleetAction::Add, window, cx);
            fill_server(app, "老代理", "127.0.0.1", PORT_C, &token, window, cx);
        })?;
        let (p, text) = test_probe(&this, cx).await?;
        this.update(cx, |app, _| check(app, "测试连接·老代理", up && matches!(p, Some(Probe::OldAgent { schema: 0, .. })), text.clone()))?;
        text
    };

    // 2 乙 18601、丙 18602：稳定运行
    let mut b = MockAgent::spawn(&job, roots.join("b"), PORT_B, Some("healthy"), None)?;
    let c = MockAgent::spawn(&job, roots.join("c"), PORT_C, Some("healthy"), None)?;
    let up = wait_agent(cx, &b, Duration::from_secs(15)).await && wait_agent(cx, &c, Duration::from_secs(15)).await;
    if !up {
        this.update(cx, |app, _| app.write_report("两台 mock 代理 15 秒内没都起来，多服务器自检没跑完"))?;
        return Ok(());
    }
    let (tb, tc) = (b.token(), c.token());

    // 3 添加乙：「测试连接」对、错令牌、错地址各一句；最后存对的。再加丙、丁（没人听的端口）
    this.update_in(cx, |app, window, cx| fill_server(app, "乙", "127.0.0.1", PORT_B, &tb, window, cx))?;
    let (ok, ok_text) = test_probe(&this, cx).await?;
    shot(&this, cx, name("light-1440-drawer-test-ok")).await?;
    this.update_in(cx, |app, window, cx| fill_server(app, "乙", "127.0.0.1", PORT_B, "selftest-wrong-token", window, cx))?;
    let (wrong_token, wrong_token_text) = test_probe(&this, cx).await?;
    this.update_in(cx, |app, window, cx| fill_server(app, "乙", "127.0.0.1", PORT_NOBODY, &tb, window, cx))?;
    let (wrong_addr, wrong_addr_text) = test_probe(&this, cx).await?;
    shot(&this, cx, name("light-1440-drawer-wrong-address")).await?;
    this.update(cx, |app, _| {
        check(app, "测试连接·对", matches!(ok, Some(Probe::Ok { .. })), ok_text.clone());
        check(app, "测试连接·错令牌", matches!(wrong_token, Some(Probe::WrongToken)), wrong_token_text.clone());
        check(app, "测试连接·错地址", matches!(wrong_addr, Some(Probe::WrongAddress(_))), wrong_addr_text.clone());
        let texts = [&ok_text, &wrong_token_text, &wrong_addr_text, &old_text];
        let distinct = texts.iter().enumerate().all(|(i, a)| texts.iter().skip(i + 1).all(|b| a != b));
        check(app, "测试连接·四种结果的话各不相同", distinct, "对 / 错令牌 / 错地址 / 老代理".into());
    })?;
    this.update_in(cx, |app, window, cx| {
        fill_server(app, "乙", "127.0.0.1", PORT_B, &tb, window, cx);
        app.fleet_action(FleetAction::Save, window, cx);
        app.fleet_action(FleetAction::Add, window, cx);
        fill_server(app, "丙", "127.0.0.1", PORT_C, &tc, window, cx);
        app.fleet_action(FleetAction::Save, window, cx);
        app.fleet_action(FleetAction::Add, window, cx);
        fill_server(app, "丁", "127.0.0.1", PORT_NOBODY, "selftest-unused-token", window, cx);
        app.fleet_action(FleetAction::Save, window, cx);
    })?;
    let ids = this.update(cx, |app, _| (id_of(app, "乙"), id_of(app, "丙"), id_of(app, "丁"), app.drawer.errors.clone()))?;
    let (Some(id_b), Some(id_c), Some(id_d)) = (ids.0, ids.1, ids.2) else {
        this.update(cx, |app, _| app.write_report(&format!("服务器没存上：{:?}", ids.3.iter().map(|(_, m)| m.as_str()).collect::<Vec<_>>())))?;
        return Ok(());
    };

    // 4 乙、丙连上（后台只拉摘要），丁判连不上
    let settled = wait_until(&this, cx, Duration::from_secs(20), move |a| {
        let now = Instant::now();
        a.fleet.health(id_b, now) == Some(Health::Ok) && a.fleet.health(id_c, now) == Some(Health::Ok) && a.fleet.health(id_d, now) == Some(Health::Down)
    })
    .await?;
    sleep(cx, 600).await;
    shot(&this, cx, name("light-1440-drawer-list")).await?;
    this.update_in(cx, |app, window, cx| {
        window.close_sheet(cx);
        app.sheet = None;
        let now = Instant::now();
        let h = |id| app.fleet.health(id, now);
        let detail = format!("本机 {:?}，乙 {:?}，丙 {:?}，丁 {:?}", h(local), h(id_b), h(id_c), h(id_d));
        check(app, "本机 + 两台 mock 代理同在列表、丁连不上", settled && app.fleet.len() == 4, detail);
        let bg = |id: ServerId| {
            app.parked.get(&id).is_some_and(|s| s.st.snapshot.is_none())
                && app.fleet.get(id).is_some_and(|s| s.summary.is_some())
                && app.handles.get(&id).is_some_and(|h| h.focus() == Focus::Background)
        };
        let (bb, bc) = (bg(id_b), bg(id_c));
        check(app, "后台的只拉摘要", bb && bc, format!("乙 {bb}、丙 {bc}（有摘要、没有完整快照、句柄焦点在后台）"));
    })?;

    // 5 丙注入故障：切到丙、注入「rs-server 退出」，再切回本机
    this.update_in(cx, |app, window, cx| app.fleet_action(FleetAction::Select(id_c), window, cx))?;
    wait_until(&this, cx, Duration::from_secs(5), move |a| a.current == id_c && a.st.snapshot.is_some()).await?;
    this.update(cx, |app, cx| app.inject(Fault::RsServerExit, cx))?;
    let c_bad = wait_until(&this, cx, Duration::from_secs(8), move |a| a.fleet.health(id_c, Instant::now()) == Some(Health::Bad)).await?;
    this.update_in(cx, |app, window, cx| app.fleet_action(FleetAction::Select(local), window, cx))?;
    sleep(cx, 1500).await;

    // 6 汇总条点名丙；「切到」换过去（换的只是看哪一份）
    let worst = this.update(cx, |app, _| {
        let now = Instant::now();
        let text = app.fleet.counts(now).text();
        let bar = app.fleet.bar(now);
        check(app, "汇总条·台数", c_bad && text == "2 台正常 · 1 台异常 · 1 台连不上", text);
        let w = bar.worst.clone();
        let named = w.as_ref().is_some_and(|w| w.id == id_c && w.text == "rs-server 异常 · 进程不在" && w.others == 1) && !bar.worst_is_selected;
        check(app, "汇总条·点名最要紧的一台", named, w.as_ref().map(|w| format!("{}：{}（另有 {} 台有问题）", w.name, w.text, w.others)).unwrap_or_default());
        w
    })?;
    shot(&this, cx, name("light-1440-overview")).await?;
    if let Some(w) = worst {
        let t = Instant::now();
        this.update_in(cx, |app, window, cx| app.fleet_action(FleetAction::Select(w.id), window, cx))?;
        let swap = t.elapsed();
        let data = wait_fast(&this, cx, t, Duration::from_secs(3), move |a| a.current == w.id && a.st.snapshot.is_some()).await?;
        this.update(cx, |app, _| {
            let fine = app.current == w.id && data.is_some_and(|d| d < Duration::from_millis(100));
            check(app, "「切到」丙，100 毫秒内换完", fine, format!("换数据源 {} 毫秒，主区域是丙的数据 {} 毫秒", ms(swap), data.map(ms).unwrap_or(u128::MAX)));
        })?;
        sleep(cx, 1000).await;
        shot(&this, cx, name("light-1440-switched-to-bad")).await?;
    }
    // 第一次选中乙：完整快照要现拉（焦点切到选中时代理马上补一份）
    let t = Instant::now();
    this.update_in(cx, |app, window, cx| app.fleet_action(FleetAction::Select(id_b), window, cx))?;
    let swap = t.elapsed();
    let data = wait_fast(&this, cx, t, Duration::from_secs(3), move |a| a.current == id_b && a.st.snapshot.is_some()).await?;
    this.update(cx, |app, _| {
        let fine = data.is_some_and(|d| d < Duration::from_millis(100));
        check(app, "第一次选中乙，100 毫秒内有完整数据", fine, format!("换数据源 {} 毫秒，完整快照到 {} 毫秒", ms(swap), data.map(ms).unwrap_or(u128::MAX)));
    })?;

    // 7 连不上的丁：横幅 + 骨架
    this.update_in(cx, |app, window, cx| app.fleet_action(FleetAction::Select(id_d), window, cx))?;
    sleep(cx, 1200).await;
    this.update(cx, |app, _| {
        let ban = app.fleet.banner(Instant::now(), None);
        let fine = ban.as_ref().is_some_and(|b| b.kind == BannerKind::NeverConnected && b.edit) && app.fleet.needs_skeleton();
        check(app, "连不上的丁：横幅 + 骨架", fine, ban.map(|b| b.text).unwrap_or_default());
    })?;
    shot(&this, cx, name("light-1440-never-connected")).await?;

    // 8 停乙：2 次失败后出横幅、旧数据变暗；再起来自动恢复。先等乙自己的日志接上（经代理拉的服务、任务日志）
    this.update_in(cx, |app, window, cx| app.fleet_action(FleetAction::Select(id_b), window, cx))?;
    wait_until(&this, cx, Duration::from_secs(5), move |a| a.current == id_b && a.st.snapshot.is_some()).await?;
    let logs_in = wait_until(&this, cx, Duration::from_secs(10), |a| agent_lines(a) >= 20).await?;
    this.update(cx, |app, _| check(app, "乙的日志经代理接上了", logs_in, format!("服务和任务日志 {} 行", agent_lines(app))))?;
    sleep(cx, 1000).await;
    b.kill();
    let t = Instant::now();
    let down = wait_fast(&this, cx, t, Duration::from_secs(20), |a| a.fleet.selected_down(Instant::now())).await?;
    sleep(cx, 300).await;
    this.update(cx, |app, _| {
        let ban = app.fleet.banner(Instant::now(), None);
        let fine = down.is_some() && ban.as_ref().is_some_and(|b| b.kind == BannerKind::Unreachable) && app.st.snapshot.is_some();
        let when = down.map(|d| format!("{:.1} 秒后", d.as_secs_f64())).unwrap_or("20 秒内没有".into());
        check(app, "停掉乙：横幅、旧数据变暗", fine, format!("{when}判连不上；横幅「{}」", ban.map(|b| b.text).unwrap_or_default()));
    })?;
    shot(&this, cx, name("light-1440-agent-stopped")).await?;
    b.restart(&job)?;
    let t = Instant::now();
    let back = wait_fast(&this, cx, t, Duration::from_secs(40), move |a| a.fleet.health(id_b, Instant::now()) == Some(Health::Ok) && !a.fleet.selected_down(Instant::now())).await?;
    this.update(cx, |app, _| {
        let when = back.map(|d| format!("再起来 {:.1} 秒后恢复", d.as_secs_f64())).unwrap_or("40 秒内没恢复".into());
        check(app, "乙再起来自动恢复", back.is_some() && app.fleet.banner(Instant::now(), None).is_none(), when);
    })?;
    sleep(cx, 1200).await;
    shot(&this, cx, name("light-1440-recovered")).await?;

    // 9 三种宽度 × 亮暗（选中本机，汇总条点名丙）
    this.update_in(cx, |app, window, cx| app.fleet_action(FleetAction::Select(local), window, cx))?;
    sleep(cx, 800).await;
    for (theme, dark) in [("light", false), ("dark", true)] {
        for (w, h) in [(1280.0_f32, 800.0_f32), (1440.0, 900.0), (1920.0, 1080.0)] {
            let mut size_wh = (w, h);
            let (mut vw, mut vh) = (0.0, 0.0);
            for attempt in 0..2 {
                this.update_in(cx, |app, window, cx| {
                    app.dark = dark;
                    Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, Some(window), cx);
                    window.resize(size(px(size_wh.0), px(size_wh.1)));
                })?;
                sleep(cx, 900).await;
                (vw, vh) = this.update_in(cx, |_, window, _| {
                    let v = window.viewport_size();
                    (v.width.as_f32(), v.height.as_f32())
                })?;
                // 本机 150% 缩放放不下 1920 宽时，退到 1680×1050（计划 J3）
                if attempt == 0 && w >= 1920.0 && vw < 1900.0 {
                    size_wh = (1680.0, 1050.0);
                    continue;
                }
                break;
            }
            let tag = size_wh.0 as u32;
            this.update(cx, |app, _| {
                let l = fleet_state::layout(vw, app.fleet.len(), app.panel.list_collapsed);
                let want_list = if vw >= fleet_state::LIST_EXPAND_FROM { ListMode::Full } else { ListMode::Rail };
                let want_tier = match tag {
                    1280 | 1440 => Tier::W1280,
                    1680 => Tier::W1440,
                    _ => Tier::W1920,
                };
                let fine = l.list == want_list && l.summary_bar && l.tier == want_tier;
                check(app, &format!("{theme} {tag} 宽"), fine, format!("视口 {vw:.0}×{vh:.0}：列表 {:?}，主区域 {:.0}，分档 {:?}", l.list, l.main_w, l.tier));
            })?;
            shot(&this, cx, name(&format!("{theme}-{tag}"))).await?;
        }
    }

    // 10 远程配置的「探测」（R10）：乙切到「未配置」，配置抽屉里问代理服务器上的路径
    this.update_in(cx, |app, window, cx| {
        app.dark = false;
        Theme::change(ThemeMode::Light, Some(window), cx);
        window.resize(size(px(1440.), px(900.)));
        app.fleet_action(FleetAction::Select(id_b), window, cx);
        app.cmd(Command::Fixture(Fixture::Unconfigured));
    })?;
    wait_until(&this, cx, Duration::from_secs(8), |a| a.st.overall(Instant::now()) == Overall::Unconfigured).await?;
    let probe_dirs = [b.root.display().to_string(), b.root.join("没有这个目录").display().to_string()];
    let mut probed = Vec::new();
    for dir in &probe_dirs {
        this.update_in(cx, |app, window, cx| {
            if app.sheet.is_none() {
                app.open_config_sheet(window, cx);
            }
            app.form.root.update(cx, |s, cx| s.set_value(dir.clone(), window, cx));
            app.probe_root(window, cx);
        })?;
        wait_until(&this, cx, Duration::from_secs(20), |a| matches!(a.root_probe, crate::app::RootProbe::Done(..))).await?;
        probed.push(this.update(cx, |app, _| app.root_probe.clone())?);
        if probed.len() == 1 {
            sleep(cx, 500).await;
            shot(&this, cx, name("light-1440-remote-probe")).await?;
        }
    }
    this.update_in(cx, |app, window, cx| {
        let remote = app.current_is_remote();
        let fine = remote
            && matches!(&probed[0], crate::app::RootProbe::Done(true, t) if t == "服务器上有这个文件夹，读得了")
            && matches!(&probed[1], crate::app::RootProbe::Done(false, t) if t == "服务器上没有这个路径");
        check(app, "远程配置「探测」", fine, format!("选中的是远程：{remote}；有的目录 {:?}；没有的 {:?}", probed[0], probed[1]));
        window.close_sheet(cx);
        app.sheet = None;
        app.fleet_action(FleetAction::Select(local), window, cx);
    })?;
    sleep(cx, 900).await;

    // 11 Ctrl+B：收起再展开，记进 panel.toml；删丁：servers.toml 只剩乙、丙，令牌不落明文
    this.update_in(cx, |app, window, cx| app.fleet_action(FleetAction::ToggleList, window, cx))?;
    sleep(cx, 700).await;
    shot(&this, cx, name("light-1440-rail-ctrl-b")).await?;
    let collapsed = this.update(cx, |app, _| app.panel.list_collapsed)?;
    this.update_in(cx, |app, window, cx| {
        app.fleet_action(FleetAction::ToggleList, window, cx);
        app.fleet_action(FleetAction::Delete(id_d), window, cx);
    })?;
    sleep(cx, 500).await;
    this.update(cx, |app, _| {
        let panel_toml = app.store.as_ref().map(|s| std::fs::read_to_string(s.panel_path()).unwrap_or_default()).unwrap_or_default();
        check(app, "Ctrl+B 收起再展开、记进 panel.toml", collapsed == Some(true) && app.panel.list_collapsed == Some(false) && panel_toml.contains("list_collapsed = false"), format!("收起后 {collapsed:?}，展开后 {:?}", app.panel.list_collapsed));
        let saved = app.store.as_ref().map(|s| s.load_servers().value.servers.iter().map(|e| e.name.clone()).collect::<Vec<_>>()).unwrap_or_default();
        let text = app.store.as_ref().map(|s| std::fs::read_to_string(s.servers_path()).unwrap_or_default()).unwrap_or_default();
        let plain = text.contains(&tb) || text.contains(&tc);
        check(app, "删丁、servers.toml 只剩乙丙、令牌不落明文", saved == ["乙", "丙"] && !plain && text.contains("dpapi:"), format!("存着 {saved:?}"));
    })?;

    drop(c);
    drop(b);
    let _ = std::fs::remove_dir_all(&roots);
    this.update(cx, |app, _| {
        let popped = app.notice_titles.iter().filter(|t| t.starts_with("已连上 ") || t.starts_with("连不上 ")).count();
        check(app, "连接状态不弹提示（横幅里说），只弹「又连上」", popped == 0, format!("弹过的提示 {:?}", app.notice_titles));
        let (p, w) = winsys::process_memory_mb();
        app.note(format!("多服务器自检用时 {:.1} 秒；结束时面板私有内存 {p:.1} MB，工作集 {w:.1} MB；两台 mock 代理已结束、根已删", started.elapsed().as_secs_f64()));
        app.note("已知答案读的是渲染用的同一份派生状态，不是从画面上读回的文字".to_string());
        app.write_report("");
    })?;
    sleep(cx, 300).await;
    Ok(())
}

fn incr(app: &DashApp) -> Option<bool> {
    app.st.snapshot.as_ref().and_then(|s| s.increments_on())
}

fn svc_ok(app: &DashApp, svc: Svc) -> bool {
    app.st.snapshot.as_ref().and_then(|s| s.svc(svc)).is_some_and(|x| x.state == SvcState::Ok)
}

/// 真实模式（`--real --selftest`）：照 APS 测试的步骤点一遍，每一步截一张 `aps-*.png`。
/// 动作都是直接调面板的方法（和点按钮走同一条路），不动鼠标、不抢焦点。
pub async fn run_real(this: WeakEntity<DashApp>, cx: &mut AsyncWindowContext) -> anyhow::Result<()> {
    let shots = this.update(cx, |app, _| app.opts.shots_dir.clone())?;
    std::fs::create_dir_all(&shots).ok();
    let name = |n: &str| shots.join(format!("aps-{n}.png"));
    let started = Instant::now();
    wait_until(&this, cx, Duration::from_secs(20), |a| a.st.snapshot.is_some()).await?;
    sleep(cx, 1500).await;
    this.update(cx, |app, _| {
        let o = app.st.overall(Instant::now());
        app.note(format!("打开时整体状态 {o:?}；数据来源 {}", app.driver.source_label()));
    })?;

    // 01 配置：工程是 APS；控制条上「生成模型」缺省不勾
    this.update_in(cx, |app, window, cx| {
        app.config_step = 0;
        app.open_config_sheet(window, cx);
    })?;
    sleep(cx, 1200).await;
    this.update(cx, |app, cx| {
        let project = app.form_value(&app.form.name, cx);
        check(app, "配置：工程是 APS", project == "AvevaPlantSample", format!("项目名 {project}"));
        check(app, "生成模型缺省不勾", !app.gen_checked && app.picked.is_empty(), format!("勾选 {}，已选库 {}", app.gen_checked, app.picked.len()));
    })?;
    shot(&this, cx, name("01-config")).await?;

    // 保存并检查 → stack doctor
    this.update_in(cx, |app, window, cx| app.save_and_check(window, cx))?;
    let doctor = wait_until(&this, cx, Duration::from_secs(90), |a| a.st.doctor.is_some() && !a.st.doctor_running && a.st.job.is_none()).await?;
    this.update(cx, |app, _| {
        let can = app.st.doctor.as_ref().is_some_and(|d| d.can_start);
        check(app, "保存并检查（stack doctor）", doctor && can, format!("检查结果{}，可以启动：{can}", if doctor { "已回来" } else { "超时" }));
    })?;
    sleep(cx, 500).await;

    // 02 启动中：一键启动（stack up -NoInit），数据库起好、后面的还在起时截图
    let t_up = Instant::now();
    this.update_in(cx, |app, window, cx| app.start_from_sheet(window, cx))?;
    let mid = wait_until(&this, cx, Duration::from_secs(60), |a| svc_ok(a, Svc::Database) && a.st.lifecycle_job().is_some()).await?;
    sleep(cx, 1500).await;
    this.update(cx, |app, _| {
        let o = app.st.overall(Instant::now());
        check(app, "启动中", mid && o == Overall::Starting, format!("数据库已起：{mid}；整体 {o:?}"));
    })?;
    shot(&this, cx, name("02-starting")).await?;
    let up = wait_until(&this, cx, Duration::from_secs(240), |a| a.st.job.is_none() && Svc::ALL.iter().all(|s| svc_ok(a, *s))).await?;
    let up_secs = t_up.elapsed().as_secs_f64();
    // 全新部署：库版本记录写上之后面板把队列暂停（增量同步缺省关）
    let paused = wait_until(&this, cx, Duration::from_secs(60), |a| incr(a) == Some(false)).await?;
    this.update(cx, |app, _| {
        let (ok, all) = app.st.counts();
        check(app, "一键启动：四个服务都起好", up, format!("{ok}/{all} 正常，用时 {up_secs:.0} 秒"));
        check(app, "全新部署：增量同步缺省关", paused, format!("queue_paused → 增量同步 {:?}", incr(app)));
    })?;

    // 03 运行中（没有生成）：读一次库列表，确认一个库都没生成
    this.update(cx, |app, _| app.cmd(Command::ListDbnums))?;
    let listed = wait_until(&this, cx, Duration::from_secs(60), |a| !a.dbnums.is_empty()).await?;
    sleep(cx, 1500).await;
    this.update(cx, |app, _| {
        let n = app.dbnums.len();
        let generated = app.dbnums.iter().filter(|r| r.initialized).count();
        let pieces: u64 = app.dbnums.iter().filter_map(|r| r.pieces).sum();
        check(app, "运行中：库列表来自后端、没有生成", listed && n > 0 && generated == 0 && pieces == 0, format!("库 {n} 个，已生成 {generated} 个，模型件数合计 {pieces}"));
    })?;
    shot(&this, cx, name("03-running")).await?;

    // 04 勾上「生成模型」并选一个小库（优先 7352：有 E3D 基准，库文件 144 KB）
    this.update_in(cx, |app, window, cx| app.toggle_generate(true, window, cx))?;
    wait_until(&this, cx, Duration::from_secs(60), |a| !a.dbnums.is_empty()).await?;
    sleep(cx, 1200).await;
    let target = this.update(cx, |app, cx| {
        let pick = app
            .dbnums
            .iter()
            .find(|r| r.dbnum == 7352 && !r.initialized)
            .or_else(|| app.dbnums.iter().filter(|r| !r.initialized && r.db_type == "DESI" && r.file_size > 0).min_by_key(|r| r.file_size))
            .map(|r| r.dbnum);
        if let Some(db) = pick {
            app.picked.insert(db);
        }
        cx.notify();
        pick
    })?;
    let Some(target) = target else {
        this.update(cx, |app, _| app.write_report("没有可生成的库，自检中止"))?;
        return Ok(());
    };
    sleep(cx, 800).await;
    shot(&this, cx, name("04-picked")).await?;

    // 05 生成中
    let t_gen = Instant::now();
    this.update(cx, |app, cx| app.start_generate(cx))?;
    let running = wait_until(&this, cx, Duration::from_secs(30), |a| a.st.job.as_ref().is_some_and(|j| j.kind == JobKind::Init)).await?;
    wait_until(&this, cx, Duration::from_secs(20), |a| a.st.backend_jobs.as_ref().is_some_and(|b| !b.rows.is_empty()) || a.st.job.is_none()).await?;
    sleep(cx, 1000).await;
    this.update(cx, |app, _| check(app, "生成中", running, format!("库 {target}")))?;
    shot(&this, cx, name("05-generating")).await?;

    // 06 生成完：只有选中的库生成了
    let done = wait_until(&this, cx, Duration::from_secs(900), |a| a.st.job.is_none() && a.dbnums.iter().any(|r| r.dbnum == target && r.initialized)).await?;
    let gen_secs = t_gen.elapsed().as_secs_f64();
    sleep(cx, 6000).await;
    this.update(cx, |app, _| {
        let others = app.dbnums.iter().filter(|r| r.initialized && r.dbnum != target).count();
        let pieces = app.dbnums.iter().find(|r| r.dbnum == target).and_then(|r| r.pieces).unwrap_or(0);
        let other_pieces: u64 = app.dbnums.iter().filter(|r| r.dbnum != target).filter_map(|r| r.pieces).sum();
        check(app, "生成完：只生成了选中的库", done && others == 0 && pieces > 0 && other_pieces == 0, format!("库 {target}：{pieces} 件，用时 {gen_secs:.0} 秒；别的库已生成 {others} 个、件数 {other_pieces}"));
        check(app, "生成完：增量同步关回去了", incr(app) == Some(false), format!("增量同步 {:?}", incr(app)));
    })?;
    shot(&this, cx, name("06-generated")).await?;
    this.update_in(cx, |app, window, cx| app.toggle_generate(false, window, cx))?;
    sleep(cx, 800).await;

    // 07 增量同步开 → 08 关
    this.update(cx, |app, cx| app.set_increments(true, cx))?;
    let on = wait_until(&this, cx, Duration::from_secs(30), |a| incr(a) == Some(true)).await?;
    sleep(cx, 1000).await;
    this.update(cx, |app, _| check(app, "增量同步开", on, format!("queue_paused = {}", incr(app).map(|v| !v).map(|v| v.to_string()).unwrap_or_default())))?;
    shot(&this, cx, name("07-incr-on")).await?;
    this.update(cx, |app, cx| app.set_increments(false, cx))?;
    let off = wait_until(&this, cx, Duration::from_secs(30), |a| incr(a) == Some(false)).await?;
    sleep(cx, 1000).await;
    this.update(cx, |app, _| check(app, "增量同步关", off, format!("queue_paused = {}", incr(app).map(|v| !v).map(|v| v.to_string()).unwrap_or_default())))?;
    shot(&this, cx, name("08-incr-off")).await?;

    // 09 全部停止（后端闲着，down 不用 -Force；万一说忙就强停）
    let t_down = Instant::now();
    this.update(cx, |app, _| app.cmd(Command::Down { force: false }))?;
    let mut stopped = wait_until(&this, cx, Duration::from_secs(120), |a| a.st.job.is_none() && a.st.overall(Instant::now()) == Overall::Stopped).await?;
    if !stopped {
        this.update_in(cx, |app, window, cx| {
            window.close_dialog(cx);
            app.note("down 说后端在忙，改用 down -Force");
            app.cmd(Command::Down { force: true });
        })?;
        stopped = wait_until(&this, cx, Duration::from_secs(120), |a| a.st.job.is_none() && a.st.overall(Instant::now()) == Overall::Stopped).await?;
    }
    sleep(cx, 1500).await;
    this.update(cx, |app, _| check(app, "全部停止", stopped, format!("用时 {:.0} 秒", t_down.elapsed().as_secs_f64())))?;
    shot(&this, cx, name("09-stopped")).await?;

    this.update(cx, |app, _| {
        let (p, w) = winsys::process_memory_mb();
        app.note(format!("真实模式自检用时 {:.1} 秒；结束时面板私有内存 {p:.1} MB，工作集 {w:.1} MB", started.elapsed().as_secs_f64()));
        app.write_report("");
    })?;
    sleep(cx, 300).await;
    Ok(())
}
