//! 页面上的一次 RVM 对拍：本程序生成 RVM（[`e3d_model::rvm_export::export_rvm`]）、E3D
//! `des.exe -tty` 导出、逐件比对（[`e3d_model::rvm_compare::compare_rvm_files`]）。只有 E3D 导出
//! 是子进程；生成与比对在本进程里跑，panic 由 [`catch_unwind`] 收成失败的一步，超时靠生成钩子
//! 协作叫停。内存爆掉没有隔离，会直接打到后端进程。判定只看比对 JSON。
//!
//! 密码只活在 [`RunSpec`] 里，拼进 `L3_E3D_LOGIN` 时由 [`crate::e3d_query::E3dDriver`]
//! 负责。任务详情、报告和落盘日志都先过 [`redact`]。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use aios_core::RefU64;
use e3d_model::rvm_export::{RvmExportOptions, export_rvm};
use e3d_model::{GenerationHooks, GenerationStopped, Stop};

use crate::data_interface::direct_store::DirectSchema;
use crate::data_interface::direct_tree::DirectTreeService;
use crate::data_interface::task_registry::{TaskRegistry, TaskState};
use crate::e3d_query::{
    E3dDriver, E3dDriverSpec, E3dProcessEvidence, running_e3d_sessions, validate_refno,
};

mod config;
#[path = "macro.rs"]
mod export_macro;
mod report;

pub use config::{ProjectDefaults, ResolvedTools, project_defaults, resolve_tools};
pub use report::{
    RvmCheckReport, RvmCounts, RvmMismatch, RvmTimingsMs, RvmVerdict, failed_report, judge_compare,
    judge_report, verdict_message,
};

const STAGE_RESOLVE: &str = "resolve";
const STAGE_OURS: &str = "generate_ours";
const STAGE_E3D: &str = "e3d_export";
const STAGE_COMPARE: &str = "compare";

static PIPELINE: OnceLock<Mutex<()>> = OnceLock::new();

fn pipeline_lock() -> std::sync::MutexGuard<'static, ()> {
    PIPELINE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 页面提交的连接参数。空字符串表示「用服务端默认」。密码不实现 Debug。
pub struct RunInput {
    pub install_dir: String,
    pub projects_dir: String,
    pub project_name: String,
    pub e3d_project: String,
    pub username: String,
    pub password: String,
    pub e3d_mdb: String,
    pub refno: String,
}

pub struct RunSpec {
    pub install_dir: PathBuf,
    pub projects_dir: PathBuf,
    pub project_name: String,
    pub project_dir: PathBuf,
    pub project_evar: PathBuf,
    pub e3d_project: String,
    pub username: String,
    pub password: String,
    pub e3d_mdb: String,
    pub refno: String,
    pub tools: ResolvedTools,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PreflightCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PreflightReport {
    pub ok: bool,
    pub checks: Vec<PreflightCheck>,
    pub running_des: Vec<String>,
    pub login_ok: Option<bool>,
    pub notes: Vec<String>,
}

pub fn rvm_check_busy(tasks: &TaskRegistry) -> bool {
    use crate::data_interface::task_registry::TASK_KIND_RVM_CHECK;
    !tasks
        .list(Some("running"), Some(TASK_KIND_RVM_CHECK), 1)
        .is_empty()
        || !tasks
            .list(Some("queued"), Some(TASK_KIND_RVM_CHECK), 1)
            .is_empty()
}

pub fn prepare_run(input: &RunInput) -> Result<RunSpec, String> {
    prepare(input, true)
}

pub fn prepare_preflight(input: &RunInput) -> Result<RunSpec, String> {
    prepare(input, false)
}

fn prepare(input: &RunInput, require_refno: bool) -> Result<RunSpec, String> {
    let defaults = project_defaults();
    let projects_dir = filled_path(&input.projects_dir, &defaults.projects_dir, "项目父目录")?;
    let project_name = filled_name(&input.project_name, &defaults.project_name, "项目目录名")?;
    let e3d_project = {
        let raw = input.e3d_project.trim();
        let raw = if raw.is_empty() {
            defaults
                .e3d_project
                .as_deref()
                .ok_or_else(|| "项目目录下没有 <代码>000，请填写 E3D 项目码".to_string())?
        } else {
            raw
        };
        validate_project_code(raw)?
    };
    let e3d_mdb = {
        let raw = input.e3d_mdb.trim();
        validate_mdb(if raw.is_empty() {
            defaults.mdb.as_str()
        } else {
            raw
        })?
    };
    let username = validate_username(input.username.trim())?;
    let password = if input.password.is_empty() && !require_refno {
        String::new()
    } else {
        validate_password(&input.password)?
    };
    let install_dir = absolute_path(input.install_dir.trim(), "E3D 安装目录")?;
    let refno = if require_refno {
        validate_refno(&normalize_refno(input.refno.trim())).map_err(|error| error.to_string())?
    } else {
        String::new()
    };
    let project_dir = projects_dir.join(&project_name);
    Ok(RunSpec {
        install_dir,
        project_evar: project_dir.join(format!("evars{project_name}.bat")),
        project_dir,
        projects_dir,
        project_name,
        e3d_project,
        username,
        password,
        e3d_mdb,
        refno,
        tools: resolve_tools(),
    })
}

/// 只读检查。`try_login` 时多跑一条 `$P` 宏，整段在调用方的 `spawn_blocking` 里。
pub fn preflight(spec: &RunSpec, try_login: bool) -> PreflightReport {
    let mut checks = Vec::new();
    checks.push(file_check("des.exe", &spec.install_dir.join("des.exe")));
    checks.push(file_check("evars.bat", &spec.install_dir.join("evars.bat")));
    checks.push(file_check("启动脚本", &spec.tools.launcher));
    checks.push(dir_check("项目目录", &spec.project_dir));
    checks.push(file_check("项目 evars", &spec.project_evar));
    let code_dir = spec.project_dir.join(format!("{}000", spec.e3d_project));
    checks.push(dir_check("项目库目录", &code_dir));

    let running_des = running_e3d_sessions().unwrap_or_default();
    let mut notes = vec![
        "登录会给打开的库追加一条只含 DBMDAT 的尾会话，增量监听可能因此空转一轮。宏本身不保存。"
            .to_string(),
        "本机若已开着 E3D 图形界面，TTY 会再占一份许可证，图形界面里没保存的修改这里看不到。"
            .to_string(),
        "默认的 e3d31-runtime 安装在 TTY 下会崩溃，安装目录请填实际能跑 des.exe -tty 的那一份。"
            .to_string(),
    ];
    if !running_des.is_empty() {
        notes.push(format!("当前正在运行：{}", running_des.join("，")));
    }

    let mut login_ok = None;
    if try_login {
        if spec.password.is_empty() {
            login_ok = Some(false);
            notes.push("试登录需要密码".to_string());
        } else if pipeline_lock_busy() {
            notes.push("另有一次 RVM 比对正在占用 E3D，试登录先跳过".to_string());
        } else {
            let _guard = pipeline_lock();
            login_ok = Some(try_login_now(spec, &mut notes));
        }
    }

    let ok = checks.iter().all(|check| check.ok) && login_ok != Some(false);
    PreflightReport {
        ok,
        checks,
        running_des,
        login_ok,
        notes,
    }
}

fn try_login_now(spec: &RunSpec, notes: &mut Vec<String>) -> bool {
    let driver = driver_for(spec);
    match driver.run_source_evidence(
        spec.tools.work_dir.as_path(),
        "rvm-check-login",
        export_macro::login_probe_body(),
    ) {
        Ok(evidence) => {
            let log = evidence_text(&evidence);
            let ok = evidence.completed() && export_macro::login_ok(&log);
            notes.push(session_note(&evidence));
            if !ok {
                notes.push(redact(
                    &format!(
                        "试登录没有走完：exit={:?} alive={} done={} timeout={}",
                        evidence.exit_status,
                        evidence.alive_seen,
                        evidence.done_seen,
                        evidence.timed_out
                    ),
                    &spec.password,
                ));
            }
            ok
        }
        Err(error) => {
            notes.push(redact(&format!("试登录失败：{error:#}"), &spec.password));
            false
        }
    }
}

/// 整段比对。调用方放在 `spawn_blocking` 里。返回的报告已经是终态。
pub fn execute(
    tree: &DirectTreeService,
    tasks: &TaskRegistry,
    task_id: &str,
    spec: &RunSpec,
) -> RvmCheckReport {
    let _guard = pipeline_lock();
    let started = Instant::now();
    let mut timings = RvmTimingsMs::default();
    let task_dir = spec.tools.work_dir.join(task_id);

    tasks.set_stage(task_id, STAGE_RESOLVE);
    tasks.set_units_done(task_id, 1);
    let resolved = match resolve_node(tree, spec) {
        Ok(resolved) => resolved,
        Err(message) => {
            timings.resolve = elapsed_ms(started);
            return failed_report(
                STAGE_RESOLVE,
                &spec.refno,
                redact(&message, &spec.password),
                timings,
            );
        }
    };
    timings.resolve = elapsed_ms(started);
    tasks.set_dbnum(task_id, resolved.dbnum, &resolved.db_type);
    tasks.set_detail(
        task_id,
        serde_json::json!({
            "refno": spec.refno,
            "name": resolved.name,
            "noun": resolved.noun,
            "dbnum": resolved.dbnum,
            "db_type": resolved.db_type,
            "sesno": resolved.sesno,
            "db_file": resolved.db_file.display().to_string(),
            "e3d_project": spec.e3d_project,
            "e3d_mdb": spec.e3d_mdb,
            "install_dir": spec.install_dir.display().to_string(),
            "username": spec.username,
        }),
    );

    let mut report = failed_report(STAGE_RESOLVE, &spec.refno, "", timings.clone());
    report.name = Some(resolved.name.clone());
    report.noun = Some(resolved.noun.clone());
    report.dbnum = Some(resolved.dbnum);
    report.db_type = Some(resolved.db_type.clone());
    report.sesno = Some(resolved.sesno);
    report.db_file = Some(resolved.db_file.display().to_string());

    tasks.set_stage(task_id, STAGE_OURS);
    tasks.set_units_done(task_id, 2);
    let ours_started = Instant::now();
    if let Err(message) = generate_ours(spec, &resolved, &task_dir) {
        report.stage = STAGE_OURS.to_string();
        report.message = redact(&message, &spec.password);
        report.timings_ms.generate_ours = elapsed_ms(ours_started);
        report.files = existing_files(&task_dir);
        return report;
    }
    report.timings_ms.generate_ours = elapsed_ms(ours_started);

    tasks.set_stage(task_id, STAGE_E3D);
    tasks.set_units_done(task_id, 3);
    let export_started = Instant::now();
    match export_e3d(spec, &resolved, &task_dir) {
        Ok(by) => report.e3d_export_by = Some(by),
        Err(message) => {
            report.stage = STAGE_E3D.to_string();
            report.message = redact(&message, &spec.password);
            report.timings_ms.e3d_export = elapsed_ms(export_started);
            report.files = existing_files(&task_dir);
            return report;
        }
    }
    report.timings_ms.e3d_export = elapsed_ms(export_started);

    tasks.set_stage(task_id, STAGE_COMPARE);
    tasks.set_units_done(task_id, 4);
    let compare_started = Instant::now();
    match compare_rvms(&task_dir) {
        Ok((verdict, counts, mismatches, centreline, truncated)) => {
            report.verdict = verdict.as_str().to_string();
            report.message = verdict_message(verdict, &counts);
            report.stage = STAGE_COMPARE.to_string();
            report.counts = counts;
            report.mismatches = mismatches;
            report.mismatches_truncated = truncated;
            report.centreline_only_baseline = centreline;
        }
        Err(message) => {
            report.stage = STAGE_COMPARE.to_string();
            report.message = redact(&message, &spec.password);
        }
    }
    report.timings_ms.compare = elapsed_ms(compare_started);
    report.files = existing_files(&task_dir);
    report
}

pub fn task_state_for(report: &RvmCheckReport) -> TaskState {
    match report.verdict.as_str() {
        report::VERDICT_PASS => TaskState::Succeeded,
        report::VERDICT_KIND => TaskState::Partial,
        _ => TaskState::Failed,
    }
}

pub fn task_id_ok(task_id: &str) -> bool {
    let mut chars = task_id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    task_id.len() <= 81
        && task_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

pub fn artifact_path(work_dir: &Path, task_id: &str, name: &str) -> Option<PathBuf> {
    if !task_id_ok(task_id) {
        return None;
    }
    let base = work_dir.join(task_id);
    let path = match name {
        "generated.rvm" => base.join("ours").join("generated.rvm"),
        "e3d.rvm" => base.join("e3d.rvm"),
        "compare.json" => base.join("compare.json"),
        "e3d.log" => base.join("e3d.log"),
        _ => return None,
    };
    path.is_file().then_some(path)
}

struct ResolvedNode {
    dbnum: u32,
    db_type: String,
    db_file: PathBuf,
    project_dir: PathBuf,
    sesno: u32,
    noun: String,
    name: String,
    /// SITE 在 MDB 全部 SITE 根里的序号（1 起，[`DirectTreeService::roots`] 的顺序）。
    /// 无名 SITE 的 E3D 显示名 `SITE n of WORLD /*` 按整个 MDB 数，单库数不出来。
    world_ordinal: Option<usize>,
}

/// 生成时解库号的目录：E3D 会话主项目的 `<代号>000`。
///
/// AMS 会话打开 ACP 的设计库（23705/289 在 acp7321）时，引用到的库号 7000 在 E3D 里是主项目的
/// ams7000，不是 acp000 里同号的 acp7000；拿库文件所在目录去解，目录件就跟 E3D 不是同一份。
/// 外项目库（SCB 一类）再按主项目 `<代号>sys` 的登记补。主项目目录不在就退回库文件所在目录。
fn session_project_dir(spec: &RunSpec, node: &ResolvedNode) -> PathBuf {
    let main = spec
        .project_dir
        .join(format!("{}000", spec.e3d_project.to_ascii_lowercase()));
    if main.is_dir() {
        main
    } else {
        node.project_dir.clone()
    }
}

/// `target` 在 MDB 顺序的 SITE 根表里排第几（1 起）；不在表里（不是 SITE 根）为 `None`。
fn world_ordinal(sites: &[RefU64], target: RefU64) -> Option<usize> {
    sites.iter().position(|site| *site == target).map(|at| at + 1)
}

fn resolve_node(tree: &DirectTreeService, spec: &RunSpec) -> Result<ResolvedNode, String> {
    let refno =
        RefU64::from_str_refno(&spec.refno).map_err(|error| format!("refno 无法解析：{error}"))?;
    let dbnum = tree
        .dbnum_of(refno)
        .ok_or_else(|| format!("节点 {} 不在当前 MDB", spec.refno))?;
    let metadata = tree
        .source_metadata(dbnum)
        .map_err(|error| format!("读库 {dbnum} 的文件头失败：{error:#}"))?;
    let project_dir = metadata
        .source
        .file
        .parent()
        .ok_or_else(|| format!("库文件 {} 没有父目录", metadata.source.file.display()))?
        .to_path_buf();
    let noun = tree.noun_of(refno).unwrap_or_default();
    let name = tree.name_of(refno).unwrap_or_default();
    let world_ordinal = if noun.eq_ignore_ascii_case("SITE") {
        let sites: Vec<RefU64> = tree
            .roots()
            .map_err(|error| format!("读 MDB 的 SITE 根表失败：{error:#}"))?
            .into_iter()
            .map(|site| site.refno.refno())
            .collect();
        world_ordinal(&sites, refno)
    } else {
        None
    };
    Ok(ResolvedNode {
        dbnum,
        db_type: metadata.db_type,
        db_file: metadata.source.file,
        project_dir,
        sesno: metadata.source.sesno,
        noun,
        name,
        world_ordinal,
    })
}

/// 到点叫停生成。生成在 DFS 每弹一个元素、每个根写 RVM 之前问一次，叫停是协作式的：
/// 卡在单个元素里不出来的那种挂起管不到。
struct Deadline(Instant);

impl GenerationHooks for Deadline {
    fn should_stop(&self, _visited: usize) -> Option<Stop> {
        (Instant::now() >= self.0).then_some(Stop::Cancelled)
    }
}

fn generate_ours(spec: &RunSpec, node: &ResolvedNode, task_dir: &Path) -> Result<(), String> {
    let template = DirectSchema::template_dir_from_env()
        .map_err(|error| format!("打不开 schema 模板目录：{error:#}"))?;
    let attlib = template.join("attlib.dat");
    if !attlib.is_file() {
        return Err(format!("模板目录里没有 attlib.dat：{}", attlib.display()));
    }
    let ours = task_dir.join("ours");
    std::fs::create_dir_all(&ours).map_err(|error| format!("建输出目录失败：{error}"))?;
    let root = e3d_model::rvm_export::parse_refno(&spec.refno)
        .map_err(|error| format!("refno 无法解析：{error:#}"))?;
    let project_dir = session_project_dir(spec, node);
    let opts = RvmExportOptions {
        attlib,
        db_file: node.db_file.clone(),
        template: template.to_path_buf(),
        db_type: node.db_type.clone(),
        sesno: (node.sesno > 0).then_some(node.sesno),
        root: Some(root),
        out: ours.clone(),
        project_dirs: vec![project_dir.clone()],
        main_project_dir: Some(project_dir),
        root_world_ordinal: node.world_ordinal,
        rvm: true,
        ..RvmExportOptions::default()
    };
    let deadline = Deadline(Instant::now() + spec.tools.timeout);
    // 库句柄、元素缓存都是 export_rvm 自己的局部量，返回时随之释放；跨 SITE 只留进程级的
    // 基本体网格缓存（按形状键，不按实例）。
    let result = catch_unwind(AssertUnwindSafe(|| export_rvm(&opts, &deadline)));
    let outcome = match result {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => {
            if matches!(
                error.downcast_ref::<GenerationStopped>(),
                Some(GenerationStopped::Hook { .. })
            ) {
                return Err(format!(
                    "生成超时（{} 秒）：{error:#}",
                    spec.tools.timeout.as_secs()
                ));
            }
            return Err(format!("生成失败：{error:#}"));
        }
        Err(panic) => return Err(format!("生成时 panic：{}", panic_text(&*panic))),
    };
    let _ = std::fs::write(ours.join("gen.log"), outcome.log.join("\n"));
    if let Some(detail) = outcome.accounts_error {
        return Err(format!("生成的账没平：{detail}"));
    }
    if !outcome.rvm_path.as_deref().is_some_and(Path::is_file) {
        return Err("生成没有写出 generated.rvm".to_string());
    }
    Ok(())
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "（非字符串 panic）".to_string())
}

fn export_e3d(spec: &RunSpec, node: &ResolvedNode, task_dir: &Path) -> Result<String, String> {
    if !spec.tools.launcher_found {
        return Err(format!(
            "找不到 E3D 启动脚本（试过 {}）",
            spec.tools.launcher.display()
        ));
    }
    std::fs::create_dir_all(task_dir).map_err(|error| format!("建输出目录失败：{error}"))?;
    let rvm = task_dir.join("e3d.rvm");
    let body = export_macro::export_body(&spec.refno, &rvm)
        .map_err(|error| format!("导出宏无法生成：{error:#}"))?;
    let driver = driver_for(spec);
    let first = driver
        .run_source_evidence(task_dir, "rvm-check-export", &body)
        .map_err(|error| format!("E3D 导出没有启动：{error:#}"))?;
    let first_log = evidence_text(&first);
    append_log(task_dir, &redact(&first_log, &spec.password));
    append_log(task_dir, &session_note(&first));
    if export_ok(&first_log, &rvm) {
        return Ok("refno".to_string());
    }
    let name = export_macro::parse_element_name(&first_log)
        .filter(|name| export_macro::safe_element_name(name).is_ok());
    let Some(name) = name else {
        return Err(export_failure(
            "按 refno 导出没有落出文件",
            &first,
            &first_log,
        ));
    };
    let _ = std::fs::remove_file(&rvm);
    let body = export_macro::export_body_by_name(&name, &rvm)
        .map_err(|error| format!("按名称导出的宏无法生成：{error:#}"))?;
    let second = driver
        .run_source_evidence(task_dir, "rvm-check-export-name", &body)
        .map_err(|error| format!("按名称再导出没有启动：{error:#}"))?;
    let second_log = evidence_text(&second);
    append_log(task_dir, &redact(&second_log, &spec.password));
    append_log(task_dir, &session_note(&second));
    if export_ok(&second_log, &rvm) {
        return Ok("name".to_string());
    }
    Err(export_failure(
        &format!("按 refno 和按名称（{name}）导出都没有落出文件"),
        &second,
        &second_log,
    ))
}

fn export_ok(log: &str, rvm: &Path) -> bool {
    export_macro::export_finished(log) && std::fs::metadata(rvm).is_ok_and(|meta| meta.len() > 32)
}

fn export_failure(prefix: &str, evidence: &E3dProcessEvidence, log: &str) -> String {
    format!(
        "{prefix}（exit={:?} alive={} done={} timeout={} forced_after_done={} tail_markers={}）\n{}",
        evidence.exit_status,
        evidence.alive_seen,
        evidence.done_seen,
        evidence.timed_out,
        evidence.forced_after_done,
        evidence.tail_markers,
        tail(log, 500)
    )
}

/// 一次 E3D 会话怎么结束的（GitHub #87）：成功也记，`forced_after_done` 为真 = 宏走完后会话不退、被宽限收掉；
/// `tail_markers` 是包装宏 `QUIT` 之后的收尾走到了第几条（0 = `QUIT` 就退了）。
fn session_note(evidence: &E3dProcessEvidence) -> String {
    format!(
        "[E3D 会话] exit={:?} forced_after_done={} tail_markers={} timeout={}",
        evidence.exit_status, evidence.forced_after_done, evidence.tail_markers, evidence.timed_out
    )
}

fn compare_rvms(
    task_dir: &Path,
) -> Result<(RvmVerdict, RvmCounts, Vec<RvmMismatch>, Vec<String>, bool), String> {
    let baseline = task_dir.join("e3d.rvm");
    let generated = task_dir.join("ours").join("generated.rvm");
    let out = task_dir.join("compare.json");
    let report = match catch_unwind(AssertUnwindSafe(|| {
        e3d_model::rvm_compare::compare_rvm_files(&baseline, &generated)
    })) {
        Ok(Ok(report)) => report,
        Ok(Err(error)) => return Err(format!("比对没有跑完：{error:#}")),
        Err(panic) => return Err(format!("比对时 panic：{}", panic_text(&*panic))),
    };
    // 流式落盘、直接从结构体判色：早先先 `to_value` 再 `to_vec_pretty` 再从 `Value` 判，
    // 80k 件的报告在后端进程里同时活着三份（结构体、Value 树、字节串），近 1 GB。
    {
        use std::io::Write;
        let file = std::fs::File::create(&out)
            .map_err(|error| format!("建 compare.json 失败：{error}"))?;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &report)
            .map_err(|error| format!("比对报告无法序列化：{error}"))?;
        writer
            .flush()
            .map_err(|error| format!("写 compare.json 失败：{error}"))?;
    }
    Ok(judge_report(&report))
}

fn driver_for(spec: &RunSpec) -> E3dDriver {
    E3dDriver::from_spec(E3dDriverSpec {
        launcher: spec.tools.launcher.clone(),
        projects_dir: spec.projects_dir.clone(),
        project_evar: spec.project_evar.clone(),
        project: spec.e3d_project.clone(),
        project_name: Some(spec.project_name.clone()),
        login: format!("{}/{}", spec.username, spec.password),
        mdb: spec.e3d_mdb.clone(),
        install_dir: Some(spec.install_dir.clone()),
        alive_timeout: spec.tools.alive_timeout,
        done_grace: crate::e3d_query::done_grace_from_env(),
        timeout: spec.tools.timeout,
    })
}

fn evidence_text(evidence: &E3dProcessEvidence) -> String {
    match &evidence.scenario_log {
        Some(scenario) => format!("{}\n{}", evidence.driver_log, scenario),
        None => evidence.driver_log.clone(),
    }
}

fn append_log(task_dir: &Path, text: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(task_dir.join("e3d.log"))
    {
        let _ = writeln!(file, "{text}");
    }
}

fn existing_files(task_dir: &Path) -> Vec<String> {
    [
        ("generated.rvm", task_dir.join("ours").join("generated.rvm")),
        ("e3d.rvm", task_dir.join("e3d.rvm")),
        ("compare.json", task_dir.join("compare.json")),
        ("e3d.log", task_dir.join("e3d.log")),
    ]
    .into_iter()
    .filter(|(_, path)| path.is_file())
    .map(|(name, _)| name.to_string())
    .collect()
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub fn redact(text: &str, secret: &str) -> String {
    if secret.len() < 2 {
        return text.to_string();
    }
    text.replace(secret, "******")
}

fn tail(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let start = text.chars().count() - max;
    text.chars().skip(start).collect()
}

fn file_check(name: &str, path: &Path) -> PreflightCheck {
    PreflightCheck {
        name: name.to_string(),
        ok: path.is_file(),
        detail: path.display().to_string(),
    }
}

fn dir_check(name: &str, path: &Path) -> PreflightCheck {
    PreflightCheck {
        name: name.to_string(),
        ok: path.is_dir(),
        detail: path.display().to_string(),
    }
}

fn pipeline_lock_busy() -> bool {
    PIPELINE
        .get()
        .is_some_and(|mutex| mutex.try_lock().is_err())
}

/// 树/URL 用 `a_b`，API 用 `a/b`；两种写法都接受，统一成 `a/b` 再交给
/// [`validate_refno`] 校验。已经带 `/` 的原样返回，让下游给出精确的错误。
fn normalize_refno(raw: &str) -> String {
    if raw.contains('/') {
        return raw.to_string();
    }
    match raw.split_once('_') {
        Some((db, index)) if !db.is_empty() && !index.is_empty() => format!("{db}/{index}"),
        _ => raw.to_string(),
    }
}

fn filled_path(given: &str, fallback: &Path, label: &str) -> Result<PathBuf, String> {
    let raw = given.trim();
    if raw.is_empty() {
        absolute_path(&fallback.display().to_string(), label)
    } else {
        absolute_path(raw, label)
    }
}

fn filled_name(given: &str, fallback: &str, label: &str) -> Result<String, String> {
    let raw = given.trim();
    let raw = if raw.is_empty() { fallback.trim() } else { raw };
    if raw.is_empty()
        || raw
            .chars()
            .any(|ch| matches!(ch, '/' | '\\' | '"' | ':' | '*'))
    {
        return Err(format!("{label}不合法"));
    }
    Ok(raw.to_string())
}

fn absolute_path(raw: &str, label: &str) -> Result<PathBuf, String> {
    if raw.is_empty() || raw.chars().any(|ch| matches!(ch, '"' | '\n' | '\r')) {
        return Err(format!("{label}不能为空，也不能含引号"));
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(format!("{label}要填绝对路径"));
    }
    Ok(path)
}

fn validate_username(raw: &str) -> Result<String, String> {
    if (1..=32).contains(&raw.len())
        && raw
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        Ok(raw.to_string())
    } else {
        Err("用户名只允许字母、数字、下划线、减号和点".to_string())
    }
}

fn validate_password(raw: &str) -> Result<String, String> {
    if (1..=64).contains(&raw.len())
        && raw
            .chars()
            .all(|ch| ch.is_ascii_graphic() && !matches!(ch, '"' | '%' | '/' | ' '))
    {
        Ok(raw.to_string())
    } else {
        Err("密码含有不能写进 E3D 登录串的字符（引号、百分号、斜杠或空白）".to_string())
    }
}

fn validate_project_code(raw: &str) -> Result<String, String> {
    let code = raw.trim().to_ascii_uppercase();
    if (1..=8).contains(&code.len()) && code.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        Ok(code)
    } else {
        Err("E3D 项目码要是 1 到 8 位字母或数字".to_string())
    }
}

fn validate_mdb(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let body = raw.strip_prefix('/').unwrap_or(raw);
    if (1..=32).contains(&body.len())
        && body
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        Ok(format!("/{body}"))
    } else {
        Err("MDB 只允许字母、数字、下划线和减号".to_string())
    }
}

trait RefnoParse {
    fn from_str_refno(text: &str) -> Result<Self, String>
    where
        Self: Sized;
}

impl RefnoParse for RefU64 {
    fn from_str_refno(text: &str) -> Result<Self, String> {
        use std::str::FromStr;
        RefU64::from_str(text).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> RunInput {
        RunInput {
            install_dir: r"E:\e3d\install".to_string(),
            projects_dir: r"D:\AVEVA\Projects\E3D3.1".to_string(),
            project_name: "AvevaMarineSample".to_string(),
            e3d_project: "ams".to_string(),
            username: "SYSTEM".to_string(),
            password: "secret-value".to_string(),
            e3d_mdb: "ALL".to_string(),
            refno: "24381/145018".to_string(),
        }
    }

    #[test]
    fn credentials_are_checked_before_any_process_starts() {
        let spec = prepare_run(&input()).unwrap();
        assert_eq!(spec.e3d_project, "AMS");
        assert_eq!(spec.e3d_mdb, "/ALL");
        assert!(spec.project_evar.ends_with("evarsAvevaMarineSample.bat"));

        let mut bad = input();
        bad.password = "has space".to_string();
        assert!(prepare_run(&bad).is_err());
        bad = input();
        bad.refno = "24381/1;QUIT".to_string();
        assert!(prepare_run(&bad).is_err());
        bad = input();
        bad.e3d_project = "AMS/ALL".to_string();
        assert!(prepare_run(&bad).is_err());
        bad = input();
        bad.install_dir = "relative\\e3d".to_string();
        assert!(prepare_run(&bad).is_err());
    }

    #[test]
    fn refno_accepts_both_a_b_and_a_underscore_b_forms() {
        assert_eq!(normalize_refno("24383/73927"), "24383/73927");
        assert_eq!(normalize_refno("24383_73927"), "24383/73927");

        let mut spec_input = input();
        spec_input.refno = "24383_73927".to_string();
        let spec = prepare_run(&spec_input).unwrap();
        assert_eq!(spec.refno, "24383/73927");

        let mut spec_input = input();
        spec_input.refno = "24383/73927".to_string();
        let spec = prepare_run(&spec_input).unwrap();
        assert_eq!(spec.refno, "24383/73927");
    }

    #[test]
    fn redaction_removes_the_password_and_leaves_short_tokens_alone() {
        assert_eq!(
            redact("login secret-value failed", "secret-value"),
            "login ****** failed"
        );
        assert_eq!(redact("ab", "a"), "ab");
    }

    #[test]
    fn unnamed_site_ordinal_counts_every_world_in_mdb_order() {
        let roots = [
            "14195/1", "14195/2", "15201/535", "23705/1", "23705/289", "23713/2343",
            "23714/1", "23717/2", "23734/1", "23737/1", "24381/46775", "24381/79977",
        ]
        .map(|text| RefU64::from_str_refno(text).unwrap());
        let at = |text: &str| world_ordinal(&roots, RefU64::from_str_refno(text).unwrap());
        assert_eq!(at("14195/2"), Some(2));
        assert_eq!(at("24381/79977"), Some(12));
        assert_eq!(at("24381/1"), None);
    }

    /// 进程内比对：同一份 RVM 两侧各放一份，必须绿，`compare.json` 照写。
    #[test]
    fn an_rvm_compared_with_itself_in_process_passes() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/rvm/1RCV0101-B1.rvm");
        let task_dir =
            std::env::temp_dir().join(format!("rvm-check-self-compare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&task_dir);
        std::fs::create_dir_all(task_dir.join("ours")).unwrap();
        std::fs::copy(&fixture, task_dir.join("e3d.rvm")).unwrap();
        std::fs::copy(&fixture, task_dir.join("ours").join("generated.rvm")).unwrap();

        let (verdict, counts, mismatches, _, _) = compare_rvms(&task_dir).unwrap();
        let wrote_json = task_dir.join("compare.json").is_file();
        let _ = std::fs::remove_dir_all(&task_dir);
        assert_eq!(verdict, RvmVerdict::Pass, "{mismatches:?}");
        assert!(counts.both > 0);
        assert_eq!(counts.off, 0);
        assert!(wrote_json);
    }

    /// 进程内从结构体判色与读落盘 JSON 判色必须一个口径：同一份报告两条路得同一结果，
    /// 绿的（自比对）与红的（两份不相干的 RVM，全是 only_*）都钉住。
    #[test]
    fn judging_the_struct_matches_judging_its_json() {
        let rvm_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/rvm");
        let fixture = rvm_dir.join("1RCV0101-B1.rvm");
        let other = rvm_dir.join("1RS-WF03-W-C-RR001.rvm");
        for (baseline, generated) in [(&fixture, &fixture), (&fixture, &other)] {
            let report = e3d_model::rvm_compare::compare_rvm_files(baseline, generated).unwrap();
            let value = serde_json::to_value(&report).unwrap();
            let from_struct = judge_report(&report);
            let from_json = judge_compare(&value);
            assert_eq!(from_struct.0, from_json.0, "{}", generated.display());
            assert_eq!(from_struct.1, from_json.1);
            assert_eq!(from_struct.2, from_json.2);
            assert_eq!(from_struct.3, from_json.3);
            assert_eq!(from_struct.4, from_json.4);
        }
    }

    #[test]
    fn task_ids_cannot_escape_the_work_directory() {
        assert!(task_id_ok("rvm-check-20261104-120000-000001"));
        assert!(!task_id_ok("../etc"));
        assert!(!task_id_ok(""));
        assert!(artifact_path(Path::new("output"), "rvm-check-1", "../e3d.log").is_none());
    }
}
