//! 分组写侧 HTTP 入口（GPWL / GPSET / GPITEM）：`POST /api/v1/group/write` 收一批变更，
//! 登记任务、后台落库，202 回执 `{ "task_id": ... }`，进度与结果走 `/api/v1/tasks/{id}`。
//!
//! 本文件只做三件事：把请求体的字符串 refno 解析成领域类型、按 `mesh_lod1` 的形状登记与
//! 收尾任务、把活儿交给 [`group_write_task::run`](crate::data_interface::group_write_task::run)。
//! 校验口径与写库口径都在 `data_interface::group_write` / `group_exec`，这里不重复。
//!
//! `GroupWriteOp` 本身不带 `Deserialize`（它属于写侧编排的领域类型，编码形状由 HTTP 层定），
//! 所以请求体用镜像 DTO [`GroupWriteOpDto`] 收，再逐条转成领域类型。

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use aios_core::RefU64;
use std::str::FromStr;

use crate::data_interface::group_write::{ContainerRef, GroupWriteOp, MemberPosition};
use crate::data_interface::group_write_task;
use crate::data_interface::task_registry::{TASK_KIND_GROUP_WRITE, TaskRegistry, TaskState};
use crate::web_service::handlers::{ProjectReq, resolve_identity};
use crate::web_service::{ApiError, AppState};

/// 请求体：身份字段平铺，`ops` 是这一批变更。
#[derive(Debug, Deserialize)]
pub struct GroupWriteReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// 一批 `GroupWriteOp`；空表是请求错误（没有可做的事）。
    #[serde(default)]
    pub ops: Vec<GroupWriteOpDto>,
}

/// 一条变更的 HTTP 形状。refno 一律用 pdms 串（`a/b`，如 `24381/100677`）；容器用
/// `existing:a/b`（库里已有的元素）或 `in_batch:N`（本批第 N 条 `create_world` / `create_set`
/// 的产物，下标从 0 起，必须排在前面）。
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GroupWriteOpDto {
    /// `world`（WORL）下建 GPWL。
    CreateWorld { world: String, name: String },
    /// GPWL 下建 GPSET；GPSET 落在它的 GPWL 所在的库。
    CreateSet { gpwl: String, name: String },
    /// 建 GPITEM + 写 GPREF，按 `at` 放进成员表；缺省追加到末尾。
    AddMember {
        gpset: String,
        target: String,
        #[serde(default)]
        at: Option<MemberPositionDto>,
    },
    /// 写 SCOSEL。空数组写的是 len=0 的 hunk，不是删属性。
    SetRules { gpset: String, scosel: Vec<i32> },
}

/// 成员在 GPSET 成员表里的落点；`after` / `before` 的锚点是成员表里的 GPITEM。
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemberPositionDto {
    Last,
    First,
    After { anchor: String },
    Before { anchor: String },
}

impl GroupWriteOpDto {
    /// 转成领域类型；refno / 容器串写错了回 400，并说清该写成什么样。
    fn into_op(self) -> Result<GroupWriteOp, ApiError> {
        Ok(match self {
            Self::CreateWorld { world, name } => GroupWriteOp::CreateWorld {
                world: parse_refno(&world)?,
                name,
            },
            Self::CreateSet { gpwl, name } => GroupWriteOp::CreateSet {
                gpwl: parse_container(&gpwl)?,
                name,
            },
            Self::AddMember { gpset, target, at } => GroupWriteOp::AddMember {
                gpset: parse_container(&gpset)?,
                target: parse_refno(&target)?,
                at: match at {
                    Some(at) => at.into_position()?,
                    None => MemberPosition::Last,
                },
            },
            Self::SetRules { gpset, scosel } => GroupWriteOp::SetRules {
                gpset: parse_container(&gpset)?,
                scosel,
            },
        })
    }
}

impl MemberPositionDto {
    fn into_position(self) -> Result<MemberPosition, ApiError> {
        Ok(match self {
            Self::Last => MemberPosition::Last,
            Self::First => MemberPosition::First,
            Self::After { anchor } => MemberPosition::After(parse_refno(&anchor)?),
            Self::Before { anchor } => MemberPosition::Before(parse_refno(&anchor)?),
        })
    }
}

/// `a/b`（也接受 `a_b`、`a:b`，与 `RefU64::from_str` 同一口径）。
fn parse_refno(text: &str) -> Result<RefU64, ApiError> {
    RefU64::from_str(text.trim()).map_err(|_| {
        ApiError::bad_request(format!(
            "refno 解析失败：{text:?}；要写成 pdms 形式 `a/b`（如 `24381/100677`）"
        ))
    })
}

/// `existing:a/b` 或 `in_batch:N`。
fn parse_container(text: &str) -> Result<ContainerRef, ApiError> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("existing:") {
        return Ok(ContainerRef::Existing(parse_refno(rest)?));
    }
    if let Some(rest) = text.strip_prefix("in_batch:") {
        return rest.trim().parse::<usize>().map(ContainerRef::InBatch).map_err(|_| {
            ApiError::bad_request(format!(
                "容器下标解析失败：{text:?}；`in_batch:` 后面要写本批第几条 create_world / create_set（从 0 起）"
            ))
        });
    }
    Err(ApiError::bad_request(format!(
        "容器写法不认：{text:?}；要么 `existing:a/b`（库里已有），要么 `in_batch:N`（本批第 N 条建出来的）"
    )))
}

/// 同时只跑一次：一批写会开库事务，两批并行只会互相抢同一批库文件。
pub fn group_write_busy(tasks: &TaskRegistry) -> bool {
    !tasks
        .list(Some("running"), Some(TASK_KIND_GROUP_WRITE), 1)
        .is_empty()
}

pub async fn group_write_start(
    State(state): State<AppState>,
    Json(req): Json<GroupWriteReq>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    resolve_identity(&state, &req.identity)?;
    if req.ops.is_empty() {
        return Err(ApiError::bad_request("ops 不能为空"));
    }
    if group_write_busy(state.tasks) {
        return Err(ApiError::conflict("已有一批分组写在进行"));
    }
    let op_count = req.ops.len();
    let ops = req
        .ops
        .into_iter()
        .map(GroupWriteOpDto::into_op)
        .collect::<Result<Vec<_>, ApiError>>()?;
    let task_id = TaskRegistry::new_task_id("group-write");
    state.tasks.insert_running_group_write(
        &task_id,
        &state.identity.project,
        json!({ "ops": op_count }),
    );
    let tasks = state.tasks;
    let task = task_id.clone();
    let state = state.clone();
    tokio::spawn(async move {
        match run_group_write(&state, &task, ops).await {
            Ok(report) => {
                let state = if report.errors.is_empty() {
                    TaskState::Succeeded
                } else {
                    TaskState::Partial
                };
                let value = serde_json::to_value(&report)
                    .unwrap_or_else(|_| json!({ "message": "分组写报告无法序列化" }));
                tasks.finish(&task, state, value);
            }
            Err(error) => tasks.finish(
                &task,
                TaskState::Failed,
                json!({ "error": format!("{error:#}") }),
            ),
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))))
}

/// 任务体：拿当前冻结会话的树服务与模板目录，交给 [`group_write_task::run`]。
async fn run_group_write(
    state: &AppState,
    task_id: &str,
    ops: Vec<GroupWriteOp>,
) -> anyhow::Result<group_write_task::GroupWriteReport> {
    state.tasks.set_stage(task_id, "validating");
    let tree = state.direct_tree.get().await?;
    let template_dir = group_write_task::template_dir()?;
    let report = group_write_task::run(tree, template_dir, ops).await?;
    state.tasks.set_stage(task_id, "done");
    Ok(report)
}
