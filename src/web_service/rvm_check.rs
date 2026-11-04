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
        "gen_ams_exe": tools.gen_ams.display().to_string(),
        "gen_ams_found": tools.gen_ams_found,
        "rvm_compare_exe": tools.rvm_compare.display().to_string(),
        "rvm_compare_found": tools.rvm_compare_found,
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
    let task_for_run = task_id.clone();
    tokio::spawn(async move {
        let tree = match tree_provider.get().await {
            Ok(tree) => tree,
            Err(error) => {
                finish_failed(
                    tasks,
                    &task_for_run,
                    &spec.refno,
                    format!("模型树还没准备好：{error:#}"),
                );
                return;
            }
        };
        let task_inside = task_for_run.clone();
        let refno = spec.refno.clone();
        let joined = tokio::task::spawn_blocking(move || {
            crate::rvm_check::execute(tree.as_ref(), tasks, &task_inside, &spec)
        })
        .await;
        match joined {
            Ok(report) => {
                let state = task_state_for(&report);
                let value = serde_json::to_value(&report)
                    .unwrap_or_else(|_| json!({"verdict": "fail", "message": "报告无法序列化"}));
                tasks.finish(&task_for_run, state, value);
            }
            Err(error) => finish_failed(
                tasks,
                &task_for_run,
                &refno,
                format!("比对任务异常结束：{error}"),
            ),
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))))
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
