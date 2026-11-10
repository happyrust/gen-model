//! `?page=rvm-check` 的五个接口。比对本身在 [`crate::rvm_check`]，这里只做校验、
//! 登记和取回。密码停在当次请求里，不进任务详情。

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::data_interface::task_registry::TaskRegistry;
use crate::rvm_check::{
    self, RunInput, artifact_path, failed_report, prepare_preflight, prepare_run, project_defaults,
    resolve_tools, rvm_check_busy, task_state_for,
};
use crate::web_service::handlers::{ProjectReq, resolve_identity};
use crate::web_service::{ApiError, AppState};

#[derive(Debug, Deserialize)]
pub struct PreflightReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    #[serde(default)]
    pub install_dir: String,
    #[serde(default)]
    pub projects_dir: String,
    #[serde(default)]
    pub project_name: String,
    #[serde(default)]
    pub e3d_project: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub e3d_mdb: String,
    #[serde(default)]
    pub try_login: bool,
}

#[derive(Debug, Deserialize)]
pub struct RunReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    #[serde(default)]
    pub install_dir: String,
    #[serde(default)]
    pub projects_dir: String,
    #[serde(default)]
    pub project_name: String,
    #[serde(default)]
    pub e3d_project: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub e3d_mdb: String,
    pub refno: String,
}

pub async fn defaults() -> Json<Value> {
    let project = project_defaults();
    let tools = resolve_tools();
    Json(json!({
        "projects_dir": project.projects_dir.display().to_string(),
        "project_name": project.project_name,
        "project_dir": project.project_dir.display().to_string(),
        "e3d_project": project.e3d_project,
        "e3d_projects": project.e3d_projects,
        "mdb": project.mdb,
        "numeric_project_code": project.numeric_project_code,
        // 生成与比对已在进程内，这两个工具总是在；字段留着给还在读它的页面。
        "gen_ams_exe": "(进程内)",
        "gen_ams_found": true,
        "rvm_compare_exe": "(进程内)",
        "rvm_compare_found": true,
        "e3d_launcher": tools.launcher.display().to_string(),
        "launcher_found": tools.launcher_found,
        "work_dir": tools.work_dir.display().to_string(),
        "alive_timeout_secs": tools.alive_timeout.as_secs(),
        "timeout_secs": tools.timeout.as_secs(),
    }))
}

pub async fn preflight(
    State(state): State<AppState>,
    Json(req): Json<PreflightReq>,
) -> Result<Json<Value>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    if req.try_login && req.password.is_empty() {
        return Err(ApiError::bad_request("试登录需要密码"));
    }
    let spec = prepare_preflight(&RunInput {
        install_dir: req.install_dir,
        projects_dir: req.projects_dir,
        project_name: req.project_name,
        e3d_project: req.e3d_project,
        username: req.username,
        password: req.password,
        e3d_mdb: req.e3d_mdb,
        refno: String::new(),
    })
    .map_err(ApiError::bad_request)?;
    let try_login = req.try_login;
    let report = tokio::task::spawn_blocking(move || crate::rvm_check::preflight(&spec, try_login))
        .await
        .map_err(|error| ApiError::from_domain(anyhow::anyhow!("预检任务异常结束：{error}")))?;
    serde_json::to_value(report)
        .map(Json)
        .map_err(|error| ApiError::from_domain(error.into()))
}

pub async fn start_run(
    State(state): State<AppState>,
    Json(req): Json<RunReq>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    resolve_identity(&state, &req.identity)?;
    let spec = prepare_run(&RunInput {
        install_dir: req.install_dir.clone(),
        projects_dir: req.projects_dir,
        project_name: req.project_name,
        e3d_project: req.e3d_project.clone(),
        username: req.username.clone(),
        password: req.password,
        e3d_mdb: req.e3d_mdb.clone(),
        refno: req.refno.clone(),
    })
    .map_err(ApiError::bad_request)?;
    if rvm_check_busy(state.tasks) {
        return Err(ApiError::conflict("已有一次 RVM 比对在进行"));
    }
    let task_id = TaskRegistry::new_task_id("rvm-check");
    state.tasks.insert_running_rvm_check(
        &task_id,
        &state.identity.project,
        json!({
            "refno": req.refno,
            "e3d_project": spec.e3d_project,
            "e3d_mdb": spec.e3d_mdb,
            "install_dir": spec.install_dir.display().to_string(),
            "username": spec.username,
        }),
    );
    let tasks = state.tasks;
    let tree_provider = state.direct_tree.clone();
    let project = state.identity.project.clone();
    let task_for_run = task_id.clone();
    let started_at = chrono::Local::now().to_rfc3339();
    tokio::spawn(async move {
        let tree = match tree_provider.get().await {
            Ok(tree) => tree,
            Err(error) => {
                let message = format!("模型树还没准备好：{error:#}");
                finish_failed(tasks, &task_for_run, &spec.refno, message.clone());
                store_failed_run(&task_for_run, &project, &spec, message, started_at).await;
                return;
            }
        };
        let task_inside = task_for_run.clone();
        let spec_inside = spec.clone();
        let joined = tokio::task::spawn_blocking(move || {
            crate::rvm_check::execute(tree.as_ref(), tasks, &task_inside, &spec_inside)
        })
        .await;
        match joined {
            Ok(outcome) => {
                let state = task_state_for(&outcome.report);
                let value = serde_json::to_value(&outcome.report)
                    .unwrap_or_else(|_| json!({"verdict": "fail", "message": "报告无法序列化"}));
                tasks.finish(&task_for_run, state, value);
                let facts = outcome.run_facts(&task_for_run, &project, &spec, Some(started_at));
                store_run(facts).await;
            }
            Err(error) => {
                let message = format!("比对任务异常结束：{error}");
                finish_failed(tasks, &task_for_run, &spec.refno, message.clone());
                store_failed_run(&task_for_run, &project, &spec, message, started_at).await;
            }
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))))
}

/// 落库。失败只记日志：任务结果已经在注册表与产物目录里，库里少一行不改变页面的结论。
async fn store_run(facts: crate::rvm_check::store::RunFacts) {
    let run_id = facts.run_id.clone();
    match crate::rvm_check::store::persist_facts(&facts).await {
        Ok(stored) => log::info!(
            "[rvm-check.store] {run_id} 已入库：verdict={} 差异 {} 行（含记账共 {} 行），构件 {} 行（清掉陈旧 {} 行{}）",
            facts.verdict,
            stored.mismatch_total,
            stored.rows,
            stored.elements,
            stored.stale_elements,
            if stored.elements_superseded {
                "；库里已有更晚的一次核对，构件行没有动"
            } else {
                ""
            }
        ),
        Err(error) => log::warn!("[rvm-check.store] {run_id} 没有入库：{error:#}"),
    }
}

/// 还没进 [`crate::rvm_check::execute`] 就失败的那两条路（模型树没好、任务线程异常）也要留一行。
async fn store_failed_run(
    task_id: &str,
    project: &str,
    spec: &crate::rvm_check::RunSpec,
    message: String,
    started_at: String,
) {
    let report = failed_report("resolve", &spec.refno, message, Default::default());
    let outcome =
        crate::rvm_check::ExecuteOutcome::failed(report, spec.tools.work_dir.join(task_id));
    store_run(outcome.run_facts(task_id, project, spec, Some(started_at))).await;
}

/// 四个查询接口共用：应答体由 [`crate::rvm_check::store::answer_on`] 定，命令行 `rvm-check-query`
/// 给的是同一份形状。
async fn answer(query: crate::rvm_check::store::Query) -> Result<Json<Value>, ApiError> {
    crate::rvm_check::store::answer(&query)
        .await
        .map(Json)
        .map_err(ApiError::from_domain)
}

/// `GET /api/v1/rvm-check/history?refno=&root_refno=&unit=&dbnum=&verdict=&status=&source=&batch_id=&before=&limit=`：
/// 库里的历次比对（一个根 / 单元一行），最新在前。`root_refno` 给上级 SITE / APPLDW，
/// 取它自己那一行加它下面的全部单元。
pub async fn history(
    State(state): State<AppState>,
    axum::extract::Query(filter): axum::extract::Query<crate::rvm_check::store::RunFilter>,
) -> Result<Json<Value>, ApiError> {
    let _ = &state;
    answer(crate::rvm_check::store::Query::Runs(filter)).await
}

/// `GET /api/v1/rvm-check/rollup?batch_id=&dbnum=&root_refno=&…`（条件同 `history`）：
/// 批量结果按上级根汇总，每批每个根一行：单元数、各终态个数、计数合计、没过的单元。
pub async fn rollup(
    State(state): State<AppState>,
    axum::extract::Query(filter): axum::extract::Query<crate::rvm_check::store::RunFilter>,
) -> Result<Json<Value>, ApiError> {
    let _ = &state;
    answer(crate::rvm_check::store::Query::Rollup(filter)).await
}

/// `GET /api/v1/rvm-check/mismatches?run_id=&refno=&root_refno=&dbnum=&batch_id=&category=&reason_kind=&noun=&severity=&limit=&offset=`：
/// 差异明细（每条带 `category` / `reason_kind` / `reason`）。
pub async fn mismatches(
    State(state): State<AppState>,
    axum::extract::Query(filter): axum::extract::Query<crate::rvm_check::store::MismatchFilter>,
) -> Result<Json<Value>, ApiError> {
    let _ = &state;
    answer(crate::rvm_check::store::Query::Mismatches(filter)).await
}

/// `GET /api/v1/rvm-check/reasons?…`（条件同 `mismatches`）：按原因种类汇总的计数，多的在前。
pub async fn reasons(
    State(state): State<AppState>,
    axum::extract::Query(filter): axum::extract::Query<crate::rvm_check::store::MismatchFilter>,
) -> Result<Json<Value>, ApiError> {
    let _ = &state;
    answer(crate::rvm_check::store::Query::Reasons(filter)).await
}

/// `GET /api/v1/rvm-check/elements?refno=&unit_refno=&root_refno=&dbnum=&batch_id=&run_id=&source=&noun=&grade=&severity=&passed=&kind_match=&derived=&refno_resolved=&limit=&offset=`：
/// 构件级结果（`rvm_check_element`）：一个带几何的构件一行，通过的也有。`refno` 给构件自己的 refno，
/// 取它那一行加记在它名下的派生件（隐式管身）。应答 `{count, more, rows}`，按记录 id 的顺序翻页。
pub async fn elements(
    State(state): State<AppState>,
    axum::extract::Query(filter): axum::extract::Query<crate::rvm_check::store::ElementFilter>,
) -> Result<Json<Value>, ApiError> {
    let _ = &state;
    answer(crate::rvm_check::store::Query::Elements(filter)).await
}

/// `GET /api/v1/rvm-check/element-grades?…`（条件同 `elements`）：构件行按判档计数，按运行与单元分组；
/// 每组的 `counts` 与 `rvm_check_run.counts` 同一个形状，可以直接对账。
pub async fn element_grades(
    State(state): State<AppState>,
    axum::extract::Query(filter): axum::extract::Query<crate::rvm_check::store::ElementFilter>,
) -> Result<Json<Value>, ApiError> {
    let _ = &state;
    answer(crate::rvm_check::store::Query::ElementGrades(filter)).await
}

pub async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let entry = state
        .tasks
        .get(&id)
        .filter(|entry| entry.kind == crate::data_interface::task_registry::TASK_KIND_RVM_CHECK)
        .ok_or_else(|| ApiError::not_found(format!("没有这次 RVM 比对：{id}")))?;
    Ok(Json(json!({
        "task_id": entry.task_id,
        "state": entry.state.as_str(),
        "stage": entry.current_stage,
        "detail": entry.detail,
        "result": entry.result,
        "started_at": entry.started_at,
        "finished_at": entry.finished_at,
        "units_done": entry.units_done,
        "total_units": entry.total_units,
        "dbnum": entry.dbnum,
        "db_type": entry.db_type,
    })))
}

pub async fn get_file(Path((id, name)): Path<(String, String)>) -> Result<Response, ApiError> {
    let tools = resolve_tools();
    let path = artifact_path(&tools.work_dir, &id, &name)
        .ok_or_else(|| ApiError::not_found("产物不存在"))?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| ApiError::from_domain(anyhow::anyhow!("读产物失败：{error}")))?;
    let content_type = match name.as_str() {
        "compare.json" => "application/json",
        "e3d.log" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        )
        .body(Body::from(bytes))
        .map_err(|error| ApiError::from_domain(anyhow::anyhow!("产物响应无法生成：{error}")))
}

fn finish_failed(tasks: &'static TaskRegistry, task_id: &str, refno: &str, message: String) {
    let report = failed_report("resolve", refno, message, Default::default());
    let value = serde_json::to_value(&report).unwrap_or_else(|_| json!({"verdict": "fail"}));
    tasks.finish(task_id, task_state_for(&report), value);
}
