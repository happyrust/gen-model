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

pub mod baseline;
pub mod batch;
mod config;
pub mod element;
#[path = "macro.rs"]
mod export_macro;
pub mod report;
pub mod store;

pub use baseline::{BaselineProvenance, BaselineVersionCheck, BaselineVersionMismatch};
pub use config::{
    ProjectDefaults, ProjectDirMismatch, ResolvedTools, project_defaults, resolve_tools,
};
pub use report::{
    Judgement, RvmCheckReport, RvmCounts, RvmMismatch, RvmTimingsMs, RvmVerdict, failed_report,
    judge_compare, judge_report, judge_report_full, verdict_message,
};

// 一次比对停在哪一步。库里 `rvm_check_run.stage` 只有这四个取值，页面单根、批量命令
// （[`batch::status::stage`]）与失败归类（[`store::classify_failure`]）共用这一份，前端的阶段标签
// 认的也是这四个字面量。「这一步有没有跑完」不靠另起阶段名来说：批量看终态（`compare-failed` 一类），
// 页面看三色。
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

#[derive(Clone)]
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
    /// 显式给的项目父目录与配置的 `project_path` 不是同一个目录时的那一笔（GitHub #145）；一致或没给为 `None`。
    /// [`prepare`] 里算好，批量与页面各自拿它告警、留痕。
    pub project_dir_mismatch: Option<ProjectDirMismatch>,
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

/// 批量对拍（[`batch`]）的连接参数：没有 refno（逐根再填），`need_e3d` 为假时（只生成 /
/// 只比对、子进程）不要求安装目录与账号密码。
pub(crate) fn prepare_connection(input: &RunInput, need_e3d: bool) -> Result<RunSpec, String> {
    if need_e3d {
        let spec = prepare(input, false)?;
        if spec.password.is_empty() {
            return Err("E3D 导出需要密码（--password 或环境变量 RVM_CHECK_PASSWORD）".to_string());
        }
        return Ok(spec);
    }
    let mut local = RunInput {
        install_dir: input.install_dir.clone(),
        projects_dir: input.projects_dir.clone(),
        project_name: input.project_name.clone(),
        e3d_project: input.e3d_project.clone(),
        username: if input.username.trim().is_empty() {
            "SYSTEM".to_string()
        } else {
            input.username.clone()
        },
        password: String::new(),
        e3d_mdb: input.e3d_mdb.clone(),
        refno: String::new(),
    };
    if local.install_dir.trim().is_empty() {
        // 不碰 E3D 的阶段用不到安装目录；填一个绝对路径只为过校验。
        local.install_dir = std::env::temp_dir().display().to_string();
    }
    prepare(&local, false)
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
    // 显式给的项目父目录只换掉预检、E3D 会话、找被引用库这一段；枚举根与模型树照旧按配置找（GitHub #145）。
    // 两边不是同一个目录就记下来——这里不出声：批量的子进程也走这一段，告警由批量入口打一次，页面走报告。
    let project_dir_mismatch =
        config::project_dir_mismatch_of(&input.projects_dir, &config::configured_search_roots());
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
        project_dir_mismatch,
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
    // 页面在预检这一步就看得到（GitHub #145）；不让预检失败——同一份数据有两种合法写法的部署字面上也不一样。
    if let Some(mismatch) = &spec.project_dir_mismatch {
        notes.push(mismatch.warning());
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

/// [`execute`] 的结果：页面报告（差异清单截到页面上限），加上**不截断**的差异清单与构件级结果给入库用
/// （[`store`]）。
pub struct ExecuteOutcome {
    pub report: RvmCheckReport,
    pub mismatches: Vec<RvmMismatch>,
    pub task_dir: PathBuf,
    /// 构件级结果（落 `rvm_check_element`）；比对没出结果时为 `None`。
    pub elements: Option<store::ElementFacts>,
}

impl ExecuteOutcome {
    /// 某一步没跑完：只有一份失败的报告，没有差异清单与构件级结果。
    pub fn failed(report: RvmCheckReport, task_dir: PathBuf) -> Self {
        Self {
            report,
            mismatches: Vec::new(),
            task_dir,
            elements: None,
        }
    }

    /// 入库用的事实。`started_at` 由调用方给（任务登记时刻）。
    pub fn run_facts(
        &self,
        task_id: &str,
        project: &str,
        spec: &RunSpec,
        started_at: Option<String>,
    ) -> store::RunFacts {
        let report = &self.report;
        store::RunFacts {
            run_id: task_id.to_string(),
            source: store::SOURCE_PAGE,
            batch_id: None,
            project: project.to_string(),
            e3d_project: spec.e3d_project.clone(),
            e3d_mdb: spec.e3d_mdb.clone(),
            refno: spec.refno.clone(),
            dbnum: report.dbnum,
            db_type: report.db_type.clone(),
            sesno: report.sesno,
            noun: report.noun.clone(),
            name: report.name.clone(),
            // 页面单根比的是任选的一个节点，没有比对单元与上级根的概念。
            unit: None,
            root_refno: None,
            root_noun: None,
            root_name: None,
            verdict: report.verdict.clone(),
            status: report.verdict.clone(),
            stage: report.stage.clone(),
            message: report.message.clone(),
            counts: report.counts.clone(),
            mismatches: self.mismatches.clone(),
            centreline_only_baseline: report.centreline_only_baseline.clone(),
            e3d_export_by: report.e3d_export_by.clone(),
            seconds: store::StageSeconds {
                resolve: report.timings_ms.resolve as f64 / 1000.0,
                generate: report.timings_ms.generate_ours as f64 / 1000.0,
                e3d_export: report.timings_ms.e3d_export as f64 / 1000.0,
                compare: report.timings_ms.compare as f64 / 1000.0,
            },
            gen_peak_private_mb: None,
            artifact_dir: Some(self.task_dir.clone()),
            files: report.files.clone(),
            started_at,
            finished_at: chrono::Local::now(),
            elements: self.elements.clone(),
            project_dir_mismatch: spec.project_dir_mismatch.clone(),
        }
    }
}

/// 页面上比的那个节点在「上级根 → 核对单元」里落在哪：构件行按它记，与批量命令按 ZONE / 整根写下的行
/// 同一套归属，谁重新核对都清得到对方留下的陈旧行。
///
/// 从 WORLD 下的根（SITE / APPLDW）沿属主链往下，第一个不是纯容器的节点就是单元（ZONE 或兜底单元，
/// 与 [`batch`] 划分单元同一条规则）。节点就是单元或就是根：整个单元核对过；节点在单元之下、或夹在根与
/// 单元之间的容器层上：只核对了一块（`subtree`）。属主链到不了 WORLD 时单元记节点自己（`node`）。
fn page_element_facts(
    tree: &DirectTreeService,
    refno: &str,
    checks: Vec<element::ElementCheck>,
) -> store::ElementFacts {
    let lineage = RefU64::from_str_refno(refno)
        .ok()
        .and_then(|start| tree.ancestors(start).ok())
        .map(|chain| {
            chain
                .into_iter()
                .map(|node| {
                    let refno = node.refno();
                    let noun = tree
                        .noun_of(refno)
                        .unwrap_or_default()
                        .trim()
                        .to_ascii_uppercase();
                    (format!("{}/{}", refno.get_0(), refno.get_1()), noun)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let (unit_refno, unit_kind, root_refno, scoped) = locate_unit(refno, &lineage);
    store::ElementFacts {
        checks,
        unit_refno,
        unit_kind,
        root_refno,
        scope: scoped.then(|| store::ElementScope {
            kind: "subtree".to_string(),
            refno: refno.to_string(),
        }),
    }
}

/// [`page_element_facts`] 的判法。`lineage` 是节点自身 → 属主 → … 的 `(refno, 大写名词)`。
/// 返回 `(单元 refno, 单元种类, 上级根, 是否只核对了单元里的一块)`。
fn locate_unit(refno: &str, lineage: &[(String, String)]) -> (String, String, Option<String>, bool) {
    // 属主链的头是 WORLD：它下面那一个就是上级根。
    let Some(world_at) = lineage.iter().position(|(_, noun)| noun == "WORL") else {
        return (refno.to_string(), "node".to_string(), None, false);
    };
    if world_at == 0 {
        return (refno.to_string(), "node".to_string(), None, false);
    }
    let root = &lineage[world_at - 1].0;
    // 从根往下（下标递减）找第一个不是纯容器的节点。
    let unit = lineage[..world_at]
        .iter()
        .rev()
        .find(|(_, noun)| !batch::UNIT_CONTAINER_NOUNS.contains(&noun.as_str()));
    match unit {
        Some((unit, noun)) => (
            unit.clone(),
            if noun == "ZONE" { "zone" } else { "fallback" }.to_string(),
            Some(root.clone()),
            unit != refno,
        ),
        // 节点自己就在容器层上（根，或根与 ZONE 之间）：单元记根。
        None => (root.clone(), "root".to_string(), Some(root.clone()), root != refno),
    }
}

/// 整段比对。调用方放在 `spawn_blocking` 里。返回的报告已经是终态。
///
/// 项目父目录与配置的 `project_path` 不一致时（GitHub #145），不管停在哪一步，报告的 `warnings` 里都带上那一句：
/// 这种运行比出来的差异可能不是模型造成的，看报告的人要先知道。
pub fn execute(
    tree: &DirectTreeService,
    tasks: &TaskRegistry,
    task_id: &str,
    spec: &RunSpec,
) -> ExecuteOutcome {
    let mut outcome = execute_steps(tree, tasks, task_id, spec);
    if let Some(mismatch) = &spec.project_dir_mismatch {
        outcome.report.warnings.push(mismatch.warning());
    }
    outcome
}

fn execute_steps(
    tree: &DirectTreeService,
    tasks: &TaskRegistry,
    task_id: &str,
    spec: &RunSpec,
) -> ExecuteOutcome {
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
            return ExecuteOutcome::failed(
                failed_report(
                    STAGE_RESOLVE,
                    &spec.refno,
                    redact(&message, &spec.password),
                    timings,
                ),
                task_dir,
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
    if let Err(message) = generate_ours(spec, &resolved, &task_dir, None, 0) {
        report.stage = STAGE_OURS.to_string();
        report.message = redact(&message, &spec.password);
        report.timings_ms.generate_ours = elapsed_ms(ours_started);
        report.files = existing_files(&task_dir);
        return ExecuteOutcome::failed(report, task_dir);
    }
    report.timings_ms.generate_ours = elapsed_ms(ours_started);

    tasks.set_stage(task_id, STAGE_E3D);
    tasks.set_units_done(task_id, 3);
    let export_started = Instant::now();
    match export_e3d(spec, &task_dir) {
        Ok(by) => report.e3d_export_by = Some(by),
        Err(message) => {
            report.stage = STAGE_E3D.to_string();
            report.message = redact(&message, &spec.password);
            report.timings_ms.e3d_export = elapsed_ms(export_started);
            report.files = existing_files(&task_dir);
            return ExecuteOutcome::failed(report, task_dir);
        }
    }
    report.timings_ms.e3d_export = elapsed_ms(export_started);

    tasks.set_stage(task_id, STAGE_COMPARE);
    tasks.set_units_done(task_id, 4);
    let compare_started = Instant::now();
    let mut mismatches_all = Vec::new();
    let mut elements = None;
    match compare_rvms_with_elements(
        &task_dir,
        &e3d_model::rvm_compare::CompareScope::All,
        spec,
        &resolved,
    ) {
        Ok((judgement, checks)) => {
            let (page_mismatches, truncated) = judgement.page_mismatches();
            report.verdict = judgement.verdict.as_str().to_string();
            report.message = verdict_message(judgement.verdict, &judgement.counts);
            report.stage = STAGE_COMPARE.to_string();
            report.counts = judgement.counts;
            report.mismatches = page_mismatches;
            report.mismatches_truncated = truncated;
            report.centreline_only_baseline = judgement.centreline_only_baseline;
            mismatches_all = judgement.mismatches;
            elements = Some(page_element_facts(tree, &spec.refno, checks));
        }
        Err(message) => {
            report.stage = STAGE_COMPARE.to_string();
            report.message = redact(&message, &spec.password);
        }
    }
    report.timings_ms.compare = elapsed_ms(compare_started);
    report.files = existing_files(&task_dir);
    ExecuteOutcome {
        report,
        mismatches: mismatches_all,
        task_dir,
        elements,
    }
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

pub(crate) struct ResolvedNode {
    dbnum: u32,
    db_type: String,
    db_file: PathBuf,
    project_dir: PathBuf,
    sesno: u32,
    noun: String,
    name: String,
    /// 根所在 SITE（根自己，或根的 SITE 祖先）在 MDB 全部 SITE 根里的序号（1 起，
    /// [`DirectTreeService::roots`] 的顺序）。无名 SITE 的 E3D 显示名 `SITE n of WORLD /*` 按整个
    /// MDB 数，单库数不出来。SITE 有名、或属主链上没有 SITE（APPLDW 一类的根）时为 `None`。
    world_ordinal: Option<usize>,
    /// [`Self::world_ordinal`] 数的那个无名 SITE 的 refno（`a/b`）：基准成员的显示名逆解回 refno 时，
    /// `SITE n of WORLD /*` 只认它。
    unnamed_site: Option<String>,
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
    sites
        .iter()
        .position(|site| *site == target)
        .map(|at| at + 1)
}

pub(crate) fn resolve_node(
    tree: &DirectTreeService,
    spec: &RunSpec,
) -> Result<ResolvedNode, String> {
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
    let site = if noun.eq_ignore_ascii_case("SITE") {
        Some(refno)
    } else {
        tree.ancestors(refno)
            .map_err(|error| format!("上溯 {} 的属主链失败：{error:#}", spec.refno))?
            .into_iter()
            .map(|node| node.refno())
            .find(|node| {
                tree.noun_of(*node)
                    .is_ok_and(|noun| noun.eq_ignore_ascii_case("SITE"))
            })
    };
    let world_ordinal = match site {
        Some(site) if tree.name_of(site).is_none() => {
            let sites: Vec<RefU64> = tree
                .roots()
                .map_err(|error| format!("读 MDB 的 SITE 根表失败：{error:#}"))?
                .into_iter()
                .map(|site| site.refno.refno())
                .collect();
            world_ordinal(&sites, site)
        }
        _ => None,
    };
    let unnamed_site = world_ordinal
        .and(site)
        .map(|site| format!("{}/{}", site.get_0(), site.get_1()));
    Ok(ResolvedNode {
        dbnum,
        db_type: metadata.db_type,
        db_file: metadata.source.file,
        project_dir,
        sesno: metadata.source.sesno,
        noun,
        name,
        world_ordinal,
        unnamed_site,
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

/// 生成即比对（[`crate::rvm_check::batch`] 的 `--fused-compare`）：生成流直喂
/// `e3d_model::rvm_compare::StreamingCompare`，`generated.rvm` 不必为对拍而写
/// （`emit_rvm` 为假时整份不落盘）。报告与判定都按 [`compare_rvms`] 同一套落盘。
pub(crate) struct FusedCompare {
    /// E3D 基准 `.rvm`（父进程已放进 `job.dir` 的那一份）。
    pub baseline: PathBuf,
    /// 是否还写 `generated.rvm`。false = 纯流式对拍，产物只有 compare/judgement。
    pub emit_rvm: bool,
    /// 并发核对窗口（字节）：待判成员攒满即并发判一批；0 = vendor 默认 256MB。
    /// 内存/性能旋钮——大窗口并发摊得开、驻留多；小窗口省内存、核对近串行。
    pub window_bytes: usize,
    /// 是否还落 `compare.json` 全量报告（大件上百 MB）。false = 结果只入库
    /// （判定 + 不截断的差异行），盘上只留 judgement.json 做进程间运输。
    pub emit_compare: bool,
}

/// 开库那一套参数（库文件、会话、模板、项目目录、DFS 起点、无名 SITE 的序号）。生成与「基准成员的
/// 显示名逆解回 refno」（[`baseline_member_sources`]）共用一份：两处读的是同一只库的同一个会话。
/// 其余开关（写不写 RVM、流式对拍、缓存上限）由调用方在此之上填。
fn open_options(
    spec: &RunSpec,
    node: &ResolvedNode,
    start_refno: &str,
    out: PathBuf,
) -> Result<RvmExportOptions, String> {
    let template = DirectSchema::template_dir_from_env()
        .map_err(|error| format!("打不开 schema 模板目录：{error:#}"))?;
    let attlib = template.join("attlib.dat");
    if !attlib.is_file() {
        return Err(format!("模板目录里没有 attlib.dat：{}", attlib.display()));
    }
    let root = e3d_model::rvm_export::parse_refno(start_refno)
        .map_err(|error| format!("refno 无法解析：{error:#}"))?;
    let project_dir = session_project_dir(spec, node);
    Ok(RvmExportOptions {
        attlib,
        db_file: node.db_file.clone(),
        template: template.to_path_buf(),
        db_type: node.db_type.clone(),
        sesno: (node.sesno > 0).then_some(node.sesno),
        root: Some(root),
        out,
        project_dirs: vec![project_dir.clone()],
        main_project_dir: Some(project_dir),
        root_world_ordinal: node.world_ordinal,
        ..RvmExportOptions::default()
    })
}

/// 只在基准里有的那几件按显示名找回库里的元素（`e3d_model::rvm_export::resolve_member_names`）。
/// 单开一只只读集合，只有对拍出了基准侧的单侧件才会调到；panic 收成 `Err`。
pub(crate) fn baseline_member_sources(
    spec: &RunSpec,
    node: &ResolvedNode,
    names: &[String],
) -> Result<Vec<Option<e3d_model::rvm_export::MemberSource>>, String> {
    let opts = open_options(spec, node, &spec.refno, PathBuf::new())?;
    let unnamed_site = node
        .unnamed_site
        .as_deref()
        .map(e3d_model::rvm_export::parse_refno)
        .transpose()
        .map_err(|error| format!("无名 SITE 的 refno 无法解析：{error:#}"))?;
    match catch_unwind(AssertUnwindSafe(|| {
        e3d_model::rvm_export::resolve_member_names(&opts, unnamed_site, names)
    })) {
        Ok(Ok(sources)) => Ok(sources),
        Ok(Err(error)) => Err(format!("{error:#}")),
        Err(panic) => Err(format!("按名字解 refno 时 panic：{}", panic_text(&*panic))),
    }
}

/// 把比对报告摊成构件级结果：生成侧按成员出处接 refno，基准侧按显示名解。解名字那一步失败只告警
/// （标准错误），那几条照样留着、`refno` 空着。
fn element_checks_of(
    report: &e3d_model::rvm_compare::CompareReport,
    sources: &[e3d_model::rvm_export::MemberSource],
    spec: &RunSpec,
    node: &ResolvedNode,
) -> Vec<element::ElementCheck> {
    let mut checks = element::element_checks(report, sources);
    if let Err(message) = element::resolve_baseline_side(&mut checks, |names| {
        baseline_member_sources(spec, node, names)
    }) {
        eprintln!("警告：={}：{}", spec.refno, redact(&message, &spec.password));
    }
    checks
}

/// `element_cache_limit`：池级元素缓存上限（条），0 = vendor 默认 200_000——内存/性能旋钮。
pub(crate) fn generate_ours(
    spec: &RunSpec,
    node: &ResolvedNode,
    task_dir: &Path,
    fused: Option<&FusedCompare>,
    element_cache_limit: usize,
) -> Result<(), String> {
    generate_ours_scoped(spec, node, task_dir, fused, element_cache_limit, None)
}

/// 带 `--scope` 的 [`generate_ours`]：`scope` 非空时 DFS 起点换成选中节点，
/// 并把比对范围一起塞进 [`RvmExportOptions::compare_scope`]。
///
/// **产物目录与基准文件仍按根算**：`task_dir` 由调用方给（`batch.rs` 的 `job.dir`），
/// 与这里选的 DFS 起点无关；`RvmExportOptions.out` 是 `task_dir/ours`，也不看 `root`。
/// 也就是说 `root` 只承担「从哪起 DFS」这一个职责，目录命名归 `RootJob`。
pub(crate) fn generate_ours_scoped(
    spec: &RunSpec,
    node: &ResolvedNode,
    task_dir: &Path,
    fused: Option<&FusedCompare>,
    element_cache_limit: usize,
    scope: Option<&batch::ScopeArg>,
) -> Result<(), String> {
    let ours = task_dir.join("ours");
    std::fs::create_dir_all(&ours).map_err(|error| format!("建输出目录失败：{error}"))?;
    // DFS 起点：`--scope` 给了就是选中节点，否则还是根。
    let start_refno = scope.map_or(spec.refno.as_str(), |scope| scope.refno());
    let mut opts = RvmExportOptions {
        rvm: fused.is_none_or(|fused| fused.emit_rvm),
        compare_baseline: fused.map(|fused| fused.baseline.clone()),
        compare_window_bytes: fused.map(|fused| fused.window_bytes).unwrap_or_default(),
        element_cache_limit,
        ..open_options(spec, node, start_refno, ours.clone())?
    };
    if let Some(scope) = scope {
        let (compare_scope, anchor) = scope_compare_scope(&opts, scope)?;
        println!(
            "比对范围 {}：选中 ={}，名字路径 {:?}（noun {} / 属主 {:?} / 序号 {}）",
            match scope {
                batch::ScopeArg::Node(_) => "node",
                batch::ScopeArg::Subtree(_) => "subtree",
            },
            scope.refno(),
            anchor.name_path,
            anchor.noun,
            anchor.owner,
            anchor.ordinal
        );
        opts.compare_scope = compare_scope;
        // 落盘给父进程：比对在父进程里跑，而它手里只有 `DirectTreeService`，
        // 没有 `DbSet` / `UdetCatalog`，算不出 E3D 显示名路径。
        write_scope_file(task_dir, scope, &anchor)?;
    }
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
    if fused.is_none_or(|fused| fused.emit_rvm)
        && !outcome.rvm_path.as_deref().is_some_and(Path::is_file)
    {
        return Err("生成没有写出 generated.rvm".to_string());
    }
    // 流式对拍：判定结果（紧凑 judgement.json）是子进程→父进程的运输件；全量
    // compare.json 只在 emit_compare 开时落盘——结果的权威归宿是库
    // （`rvm_check_run` / `rvm_check_mismatch`），大件 JSON 默认不产。
    if let Some(report) = &outcome.compare {
        if fused.is_some_and(|fused| fused.emit_compare) {
            let compare_out = task_dir.join("compare.json");
            {
                use std::io::Write;
                let file = std::fs::File::create(&compare_out)
                    .map_err(|error| format!("建 compare.json 失败：{error}"))?;
                let mut writer = std::io::BufWriter::new(file);
                serde_json::to_writer_pretty(&mut writer, report)
                    .map_err(|error| format!("比对报告无法序列化：{error}"))?;
                writer
                    .flush()
                    .map_err(|error| format!("写 compare.json 失败：{error}"))?;
            }
        }
        // 构件级结果（每个带几何的构件一条，通过的也有）同样经文件交给父进程：`element-checks.json`。
        // 先写它、再写 judgement.json——父进程拿后者当「比对出了结果」的物证。
        let checks = element_checks_of(report, &outcome.member_sources, spec, node);
        element::write_checks(task_dir, &checks)?;
        let judgement = judge_report_full(report);
        let judgement_out = task_dir.join("judgement.json");
        serde_json::to_writer_pretty(
            std::io::BufWriter::new(
                std::fs::File::create(&judgement_out)
                    .map_err(|error| format!("建 judgement.json 失败：{error}"))?,
            ),
            &judgement,
        )
        .map_err(|error| format!("判定结果无法序列化：{error}"))?;
    } else {
        // 不走生成即比对：比对在生成之后按文件做，那时手里只有 generated.rvm（里面没有 refno）。
        // 把每个成员的出处落在旁边，比对那一步按成员路径接回来。
        element::write_sources(task_dir, &outcome.member_sources)?;
    }
    Ok(())
}

/// `--scope` → vendor 的 [`e3d_model::rvm_compare::CompareScope`]。
///
/// 名字路径由 [`e3d_model::rvm_export::scope_anchor`] 在 vendor 侧算（它握着 `DbSet` 与
/// `UdetCatalog`，能复现 `owner_display_desc` 那套 E3D 显示名）。算出的路径**如实**交给
/// 基准侧定位，不做大小写折叠、不做模糊匹配——对不上就让基准装载报「定位不到」，
/// 由实测暴露口径问题（`owner_display_desc` 上溯无名 SITE 时只数本库成员）。
fn scope_compare_scope(
    opts: &RvmExportOptions,
    scope: &batch::ScopeArg,
) -> Result<
    (
        e3d_model::rvm_compare::CompareScope,
        e3d_model::rvm_export::ScopeAnchor,
    ),
    String,
> {
    let refno = e3d_model::rvm_export::parse_refno(scope.refno())
        .map_err(|error| format!("refno 无法解析：{error:#}"))?;
    let anchor = e3d_model::rvm_export::scope_anchor(opts, refno)
        .map_err(|error| format!("算选中节点的名字路径失败：{error:#}"))?;
    let mut target = e3d_model::rvm_compare::ScopeTarget::path(anchor.name_path.clone());
    // 退化三元组与 `rvm_compare` 建表同口径：无名节点在主定位落空时按
    // (noun, 属主显示名, 序号) 再认一次。
    if let Some(owner) = &anchor.owner {
        target = target.with_fallback(anchor.noun.clone(), owner.clone(), anchor.ordinal);
    }
    let compare_scope = match scope {
        batch::ScopeArg::Node(_) => e3d_model::rvm_compare::CompareScope::Node { target },
        batch::ScopeArg::Subtree(_) => e3d_model::rvm_compare::CompareScope::Subtree { target },
    };
    Ok((compare_scope, anchor))
}

/// 落盘的比对范围：父进程读它重建 [`e3d_model::rvm_compare::CompareScope`]。
///
/// 名字路径只能在生成侧算（要 `DbSet` + `UdetCatalog` 复现 E3D 显示名）。父进程里
/// `--in-process` 与子进程两种模式都能读到这一份，比对口径因此只有一处来源。
const SCOPE_FILE: &str = "scope.json";

fn write_scope_file(
    task_dir: &Path,
    scope: &batch::ScopeArg,
    anchor: &e3d_model::rvm_export::ScopeAnchor,
) -> Result<(), String> {
    let body = serde_json::json!({
        "kind": match scope {
            batch::ScopeArg::Node(_) => "node",
            batch::ScopeArg::Subtree(_) => "subtree",
        },
        "refno": scope.refno(),
        "name_path": anchor.name_path,
        "noun": anchor.noun,
        "owner": anchor.owner,
        "ordinal": anchor.ordinal,
    });
    let path = task_dir.join(SCOPE_FILE);
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&body)
            .map_err(|error| format!("scope.json 无法序列化：{error}"))?,
    )
    .map_err(|error| format!("写 {} 失败：{error}", path.display()))
}

/// 从 `scope.json` 读回比对范围；文件不在时返回 `All`（老产物目录、没给 `--scope`）。
pub(crate) fn read_scope_file(
    task_dir: &Path,
) -> Result<e3d_model::rvm_compare::CompareScope, String> {
    let path = task_dir.join(SCOPE_FILE);
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(e3d_model::rvm_compare::CompareScope::All);
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("解析 {} 失败：{error}", path.display()))?;
    let kind = value
        .get("kind")
        .and_then(|kind| kind.as_str())
        .unwrap_or_default();
    let name_path = value
        .get("name_path")
        .and_then(|path| path.as_str())
        .unwrap_or_default()
        .to_string();
    let mut target = e3d_model::rvm_compare::ScopeTarget::path(name_path);
    if let (Some(noun), Some(owner), Some(ordinal)) = (
        value.get("noun").and_then(|noun| noun.as_str()),
        value.get("owner").and_then(|owner| owner.as_str()),
        value.get("ordinal").and_then(|ordinal| ordinal.as_u64()),
    ) {
        target = target.with_fallback(noun.to_string(), owner.to_string(), ordinal as usize);
    }
    Ok(match kind {
        "node" => e3d_model::rvm_compare::CompareScope::Node { target },
        "subtree" => e3d_model::rvm_compare::CompareScope::Subtree { target },
        _ => e3d_model::rvm_compare::CompareScope::All,
    })
}

pub(crate) fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "（非字符串 panic）".to_string())
}

pub(crate) fn export_e3d(spec: &RunSpec, task_dir: &Path) -> Result<String, String> {
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

pub(crate) fn export_ok(log: &str, rvm: &Path) -> bool {
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

pub(crate) fn compare_rvms(task_dir: &Path) -> Result<Judgement, String> {
    compare_rvms_scoped(task_dir, &e3d_model::rvm_compare::CompareScope::All)
}

/// 带比对范围的 [`compare_rvms`]：`All` 时走 `compare_rvm_files` 同一条老路径。
pub(crate) fn compare_rvms_scoped(
    task_dir: &Path,
    scope: &e3d_model::rvm_compare::CompareScope,
) -> Result<Judgement, String> {
    compare_rvm_report(task_dir, scope).map(|report| judge_report_full(&report))
}

/// [`compare_rvms_scoped`] 加构件级结果：同一份报告既判色、也摊成每个构件一条。生成侧的 refno 从
/// 生成时落下的 `ours/member-sources.json` 按成员路径接；那份文件不在（加它之前生成的产物目录）时
/// 生成侧的构件都记成没解出 refno。
pub(crate) fn compare_rvms_with_elements(
    task_dir: &Path,
    scope: &e3d_model::rvm_compare::CompareScope,
    spec: &RunSpec,
    node: &ResolvedNode,
) -> Result<(Judgement, Vec<element::ElementCheck>), String> {
    let report = compare_rvm_report(task_dir, scope)?;
    let sources = element::read_sources(task_dir).unwrap_or_else(|message| {
        eprintln!("警告：={}：{message}（生成侧的构件记成没解出 refno）", spec.refno);
        Vec::new()
    });
    let checks = element_checks_of(&report, &sources, spec, node);
    Ok((judge_report_full(&report), checks))
}

/// 按文件比对两份 RVM，落 `compare.json`，交回整份报告。
fn compare_rvm_report(
    task_dir: &Path,
    scope: &e3d_model::rvm_compare::CompareScope,
) -> Result<e3d_model::rvm_compare::CompareReport, String> {
    let baseline = task_dir.join("e3d.rvm");
    let generated = task_dir.join("ours").join("generated.rvm");
    let out = task_dir.join("compare.json");
    let report = match catch_unwind(AssertUnwindSafe(|| {
        e3d_model::rvm_compare::compare_rvm_files_scoped(&baseline, &generated, scope)
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
    Ok(report)
}

pub(crate) fn driver_for(spec: &RunSpec) -> E3dDriver {
    driver_with_grace(spec, crate::e3d_query::done_grace_from_env())
}

/// 同 [`driver_for`]，但 `L3-DONE` 之后等 E3D 自己退出的宽限由调用方给：批量导出一库一次会话，
/// 导出文件在 `L3-DONE` 落盘前就已写完，不必每次都等满缺省的 60 秒。
pub(crate) fn driver_with_grace(spec: &RunSpec, done_grace: std::time::Duration) -> E3dDriver {
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
        done_grace,
        timeout: spec.tools.timeout,
    })
}

pub(crate) fn evidence_text(evidence: &E3dProcessEvidence) -> String {
    match &evidence.scenario_log {
        Some(scenario) => format!("{}\n{}", evidence.driver_log, scenario),
        None => evidence.driver_log.clone(),
    }
}

pub(crate) fn append_log(task_dir: &Path, text: &str) {
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

pub(crate) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub fn redact(text: &str, secret: &str) -> String {
    if secret.len() < 2 {
        return text.to_string();
    }
    text.replace(secret, "******")
}

pub(crate) fn tail(text: &str, max: usize) -> String {
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

    /// 批量对拍的连接参数：不碰 E3D 的阶段（子进程生成、只比对）不要安装目录与密码；要导基准
    /// 时密码缺席必须在起任何进程之前就被拦下。
    #[test]
    fn batch_connection_only_demands_credentials_when_e3d_is_needed() {
        let mut local = input();
        local.install_dir = String::new();
        local.password = String::new();
        let spec = prepare_connection(&local, false).unwrap();
        assert_eq!(spec.e3d_project, "AMS");
        assert_eq!(spec.e3d_mdb, "/ALL");
        assert!(spec.refno.is_empty());
        assert!(spec.password.is_empty());

        let mut no_password = input();
        no_password.password = String::new();
        let error = match prepare_connection(&no_password, true) {
            Ok(_) => panic!("没有密码不该通过"),
            Err(error) => error,
        };
        assert!(error.contains("密码"), "{error}");

        let spec = prepare_connection(&input(), true).unwrap();
        assert_eq!(spec.password, "secret-value");
        assert_eq!(spec.install_dir, PathBuf::from(r"E:\e3d\install"));
    }

    /// GitHub #145：显式给的项目父目录与配置的 `project_path` 不是同一个目录，准备好的参数里带着这一笔——批量的
    /// 子进程走的那一支也带着（它不告警，只随记录走），入库的事实里也有；没给、或给的就是配置的那个目录，没有。
    /// 比较本身（同一目录的不同写法、排在后面的搜索根、留痕的形状）钉在 `config` 的单测里；这里钉的是接线。
    #[test]
    fn a_projects_dir_other_than_the_configured_one_rides_the_spec_into_the_stored_run() {
        let roots = config::configured_search_roots();
        let Some(configured) = roots.first() else {
            // 这台机器的配置里一个搜索根都没有：无从比较。
            assert_eq!(
                prepare_preflight(&input()).unwrap().project_dir_mismatch,
                None
            );
            return;
        };
        let mut given = input();
        given.projects_dir = String::new();
        assert_eq!(
            prepare_preflight(&given).unwrap().project_dir_mismatch,
            None,
            "没给：用的就是配置的值"
        );
        given.projects_dir = format!("  {}  ", configured.display());
        assert_eq!(
            prepare_preflight(&given).unwrap().project_dir_mismatch,
            None,
            "给的就是配置的那个目录"
        );

        let other = std::env::temp_dir().join("rvm-check-145-another-projects-dir");
        given.projects_dir = other.display().to_string();
        let spec = prepare_preflight(&given).unwrap();
        let mismatch = spec.project_dir_mismatch.clone().expect("不一致");
        assert_eq!(mismatch.projects_dir, other.display().to_string());
        assert_eq!(
            mismatch.configured_roots,
            roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            prepare_connection(&given, false)
                .unwrap()
                .project_dir_mismatch,
            Some(mismatch.clone())
        );

        let outcome = ExecuteOutcome::failed(
            failed_report(STAGE_RESOLVE, "24381/1", "x", Default::default()),
            other,
        );
        assert_eq!(
            outcome
                .run_facts("rvm-check-t", "P", &spec, None)
                .project_dir_mismatch,
            Some(mismatch)
        );

        // 页面的两处出口：预检的说明、报告的告警（不管停在哪一步）。两处都要起进程 / 开模型树，按源码钉。
        let source = include_str!("mod.rs");
        let preflight_body = source
            .split_once("pub fn preflight(")
            .expect("preflight")
            .1
            .split_once("fn try_login_now(")
            .expect("try_login_now follows")
            .0;
        assert!(
            preflight_body.contains("notes.push(mismatch.warning());"),
            "预检的说明里要有这一句"
        );
        let execute_body = source
            .split_once("pub fn execute(")
            .expect("execute")
            .1
            .split_once("fn execute_steps(")
            .expect("execute_steps follows")
            .0;
        assert!(
            execute_body.contains("execute_steps(tree, tasks, task_id, spec)")
                && execute_body.contains("outcome.report.warnings.push(mismatch.warning());"),
            "报告的告警加在所有出口之外：{execute_body}"
        );
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
            "14195/1",
            "14195/2",
            "15201/535",
            "23705/1",
            "23705/289",
            "23713/2343",
            "23714/1",
            "23717/2",
            "23734/1",
            "23737/1",
            "24381/46775",
            "24381/79977",
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

        let judgement = compare_rvms(&task_dir).unwrap();
        let wrote_json = task_dir.join("compare.json").is_file();
        let _ = std::fs::remove_dir_all(&task_dir);
        assert_eq!(
            judgement.verdict,
            RvmVerdict::Pass,
            "{:?}",
            judgement.mismatches
        );
        assert!(judgement.counts.both > 0);
        assert_eq!(judgement.counts.off, 0);
        assert!(wrote_json);
    }

    /// 红的那条路：两份不相干的 RVM 全是 only_*，入库的差异清单要**全量**（不受页面 400 条上限），
    /// 每条带原因种类；页面那份截断。
    #[test]
    fn the_full_mismatch_list_survives_for_storage_while_the_page_copy_is_capped() {
        let rvm_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/rvm");
        let report = e3d_model::rvm_compare::compare_rvm_files(
            &rvm_dir.join("1RCV0101-B1.rvm"),
            &rvm_dir.join("1RS-WF03-W-C-RR001.rvm"),
        )
        .unwrap();
        let judgement = judge_report_full(&report);
        assert_eq!(judgement.verdict, RvmVerdict::Fail);
        let expected = report.only_baseline.len()
            + report.only_generated.len()
            + report.unkeyable_baseline.len()
            + report.unkeyable_generated.len();
        assert_eq!(judgement.mismatches.len(), expected);
        assert!(
            judgement
                .mismatches
                .iter()
                .all(|row| !row.reason_kind.is_empty() && !row.reason.is_empty()),
            "每条差异都要有原因种类"
        );
        let (page, truncated) = judgement.page_mismatches();
        assert_eq!(truncated, expected > 400);
        assert_eq!(page.len(), expected.min(400));
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

    /// 页面上比的节点归到哪个单元名下：与批量划分单元同一条规则（根往下第一个不是纯容器的节点）。
    /// 节点就是单元或就是根算整个单元核对过；在单元之下、或夹在根与单元之间的容器层上算只核对了一块。
    #[test]
    fn a_page_node_is_filed_under_the_same_unit_the_batch_would_use() {
        let chain = |nodes: &[(&str, &str)]| -> Vec<(String, String)> {
            nodes
                .iter()
                .map(|(refno, noun)| (refno.to_string(), noun.to_string()))
                .collect()
        };
        let site = [
            ("1/40", "ELBO"),
            ("1/30", "BRAN"),
            ("1/20", "PIPE"),
            ("1/10", "ZONE"),
            ("1/2", "SITE"),
            ("1/1", "WORL"),
        ];
        let located = |from: usize| {
            let lineage = chain(&site[from..]);
            locate_unit(&lineage[0].0.clone(), &lineage)
        };
        let unit = |unit: &str, kind: &str, root: &str, scoped: bool| {
            (
                unit.to_string(),
                kind.to_string(),
                Some(root.to_string()),
                scoped,
            )
        };
        assert_eq!(located(0), unit("1/10", "zone", "1/2", true), "ZONE 之下的构件");
        assert_eq!(located(2), unit("1/10", "zone", "1/2", true), "ZONE 之下的 PIPE");
        assert_eq!(located(3), unit("1/10", "zone", "1/2", false), "就是那个 ZONE");
        assert_eq!(located(4), unit("1/2", "root", "1/2", false), "就是那个 SITE");

        // APPLDW 下经 APPLDA、APPDAR 两层才到 ZONE；夹在中间的容器层记在根名下、只算一块。
        let appl = chain(&[
            ("2/5", "ZONE"),
            ("2/4", "APPDAR"),
            ("2/3", "APPLDA"),
            ("2/2", "APPLDW"),
            ("2/1", "WORL"),
        ]);
        assert_eq!(locate_unit("2/5", &appl), unit("2/5", "zone", "2/2", false));
        assert_eq!(
            locate_unit("2/3", &appl[2..]),
            unit("2/2", "root", "2/2", true)
        );
        // 根下不在任何 ZONE 里的子节点是兜底单元。
        let loose = chain(&[("3/9", "BOX"), ("3/3", "STRU"), ("3/2", "SITE"), ("3/1", "WORL")]);
        assert_eq!(locate_unit("3/9", &loose), unit("3/3", "fallback", "3/2", true));
        // 属主链到不了 WORLD：单元记节点自己。
        assert_eq!(
            locate_unit("9/9", &chain(&[("9/9", "EQUI")])),
            ("9/9".to_string(), "node".to_string(), None, false)
        );
        assert_eq!(
            locate_unit("9/9", &[]),
            ("9/9".to_string(), "node".to_string(), None, false)
        );
    }

    #[test]
    fn task_ids_cannot_escape_the_work_directory() {
        assert!(task_id_ok("rvm-check-20261104-120000-000001"));
        assert!(!task_id_ok("../etc"));
        assert!(!task_id_ok(""));
        assert!(artifact_path(Path::new("output"), "rvm-check-1", "../e3d.log").is_none());
    }

    /// `--scope` 的落盘与读回：父进程靠这一份重建比对范围（名字路径只能在生成侧算）。
    /// 没有 `scope.json` 的老产物目录必须落回 `All`，与加这个参数之前同一条路。
    #[test]
    fn scope_json_round_trips_and_falls_back_to_all() {
        let dir = std::env::temp_dir().join(format!("rvm-check-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 没给 --scope：读回 All。
        assert!(read_scope_file(&dir).unwrap().is_all());

        let anchor = e3d_model::rvm_export::ScopeAnchor {
            name_path: "/SITE-1/EQUIPMENT-A".to_string(),
            noun: "EQUI".to_string(),
            owner: Some("SITE /SITE-1".to_string()),
            ordinal: 3,
        };
        write_scope_file(
            &dir,
            &batch::ScopeArg::Subtree("9304/2".to_string()),
            &anchor,
        )
        .unwrap();
        let scope = read_scope_file(&dir).unwrap();
        match scope {
            e3d_model::rvm_compare::CompareScope::Subtree { target } => {
                assert_eq!(target.name_path, "/SITE-1/EQUIPMENT-A");
                let fallback = target.fallback.expect("退化三元组要一起落盘");
                assert_eq!(fallback.noun, "EQUI");
                assert_eq!(fallback.owner, "SITE /SITE-1");
                assert_eq!(fallback.ordinal, 3);
            }
            other => panic!("读回来的档位不对：{other:?}"),
        }

        write_scope_file(&dir, &batch::ScopeArg::Node("9304/2".to_string()), &anchor).unwrap();
        assert!(matches!(
            read_scope_file(&dir).unwrap(),
            e3d_model::rvm_compare::CompareScope::Node { .. }
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
