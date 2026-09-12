//! `POST /api/v1/element/ptset` —— 构件的目录 P 点集（E3D 的 `PTSET`），给 plant3d-web 的
//! 测量捕捉（P-Point 吸附 / 点→线垂距的轴线）用。
//!
//! 之前浏览器端的 P-Point 只有两条来源：legacy 模型包里的 `ptsets.parquet`，或旧后端
//! `:3100` 的 `/api/pdms/ptset/{refno}`（SurrealDB `inst_relate.out.ptset`）。gen-model-v1
//! 的读透形态两样都没有——投影里只存网格与世界矩阵，`ptset` 从未落库。这条端点不走库：
//! 与 `/api/v1/element/attributes` 同一口径直读 dabacon 文件，用 e3d-model 的目录链
//! `SPRE → SCOM → PTRE → PTSE`（[`e3d_model::catalogue_point_set`]）把整个点集解出来，
//! 数字与隐式管身两端所用的 P 点是同一份求值环境算出来的。
//!
//! 响应形状（mm，局部系 + 列主序世界矩阵，前端 `ptsetTransform.ts` 原样吃）：
//!
//! ```json
//! {
//!   "source": "e3d-model",
//!   "refno": "24381/145031", "dbnum": 7997, "noun": "ELBO", "name": "/…",
//!   "catalogue": { "component": "24380/1234", "point_set": "24380/1235" },
//!   "world_transform": [16 个数], "unit": "mm",
//!   "points": [{ "number": 1, "noun": "PTAX", "pt": [x,y,z], "dir": [x,y,z], "bore": 100.0 }],
//!   "unresolved": [{ "number": 3, "reason": "P3（PTCA）方向解不出" }],
//!   "members": [ …同形，每个直属成员一条… ]   // 仅 include_members=true
//! }
//! ```
//!
//! 状态语义：
//! - `200`：构件存在。没有 `SPRE` 链 / 目录件没有 `PTRE` 时 `catalogue` 为 `null`、`points` 为空
//!   ——那是「这个构件没有 P 点」（E3D 也不画），不是错。
//! - `400 bad_request`：refno 写法不对（接受 `a/b`、`a_b`、带 `=` 前缀）。
//! - `404 not_found`：refno 在所钉会话里不存在。
//! - `500 internal`：读库出错。
//!
//! 数据时点与 `/api/mbd/v2/pipe` 相同：目标库按 ADR-054 取**文件最新会话**（每次请求新开
//! `DbSet`），`E3dModelService` 进程内只建一次。

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use e3d_io::db_element::{DbElement, DbElementError, DbSet};
use e3d_io::refno::RefNo;
use serde::{Deserialize, Serialize};

use super::handlers::{ProjectReq, resolve_identity};
use super::{ApiError, AppState};
use crate::fast_model::e3d_model_service::parse_refno;
use crate::mbd::wire_refno;

#[derive(Debug, Deserialize)]
pub struct ElementPtsetReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub refno: String,
    /// 一并解直属成员的 P 点集。BRAN / EQUI 这类容器自身没有 `SPRE`，测量悬停在容器上时
    /// 要显示的是成员的 P 点（E3D 元素级捕捉的等价口径），前端用这一发代替逐成员请求。
    #[serde(default)]
    pub include_members: bool,
}

/// 一个 P 点，构件**局部系**，mm；`dir` 已归一化。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PtsetPointDto {
    pub number: u32,
    /// `PTAX` / `PTCA` / `PTMI`
    pub noun: String,
    pub pt: [f64; 3],
    pub dir: [f64; 3],
    /// `PBOR`，求不出为 null
    pub bore: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UnresolvedPointDto {
    pub number: u32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CatalogueRefsDto {
    /// 目录件（SCOM / SFIT），`a/b`
    pub component: String,
    /// `PTSE`，目录件没有 `PTRE` 时为 null
    pub point_set: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ElementPtsetDto {
    /// `a/b`
    pub refno: String,
    pub dbnum: u32,
    pub noun: String,
    pub name: Option<String>,
    pub catalogue: Option<CatalogueRefsDto>,
    /// 局部 → 世界，列主序 16 个数（THREE `Matrix4.fromArray` 布局，平移在 12–14），mm。
    pub world_transform: [f64; 16],
    pub unit: &'static str,
    /// `PTSE` 成员原序
    pub points: Vec<PtsetPointDto>,
    pub unresolved: Vec<UnresolvedPointDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ElementPtsetResponse {
    pub source: &'static str,
    #[serde(flatten)]
    pub element: ElementPtsetDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<ElementPtsetDto>>,
}

/// POST /api/v1/element/ptset
pub async fn element_ptset(
    State(state): State<AppState>,
    Json(req): Json<ElementPtsetReq>,
) -> Result<Json<ElementPtsetResponse>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let parsed = parse_refno(&req.refno).map_err(|error| {
        ApiError::bad_request(format!("refno `{}` 无法解析：{error:#}", req.refno))
    })?;
    let service = super::mbd::service().await?;
    let set = service
        .build_set(parsed.dbno(), None)
        .map_err(ApiError::from_domain)?;
    let include_members = req.include_members;
    tokio::task::spawn_blocking(move || respond(&set, parsed, include_members))
        .await
        .map_err(|join| ApiError::from_domain(anyhow::anyhow!("ptset task aborted: {join}")))?
        .map(Json)
}

/// 与 `AppState` / MDB 视图无关的主体：给定一个 `DbSet` 就能答。
pub fn respond(
    set: &Arc<DbSet>,
    refno: RefNo,
    include_members: bool,
) -> Result<ElementPtsetResponse, ApiError> {
    let element = set.element(refno);
    if !element.exists().map_err(db_error)? {
        return Err(ApiError::not_found(format!(
            "refno {} 在所钉会话里不存在",
            wire_refno(refno)
        )));
    }
    let dto = element_dto(&element).map_err(db_error)?;
    let members = if include_members {
        let mut out = Vec::new();
        for member in element.members().map_err(db_error)? {
            out.push(element_dto(&member).map_err(db_error)?);
        }
        Some(out)
    } else {
        None
    };
    Ok(ElementPtsetResponse {
        source: "e3d-model",
        element: dto,
        members,
    })
}

fn element_dto(element: &DbElement) -> Result<ElementPtsetDto, DbElementError> {
    let refno = element.refno();
    let set = e3d_model::catalogue_point_set(element)?;
    let world = e3d_model::transform::world_matrix(element)?;
    Ok(ElementPtsetDto {
        refno: pdms_refno(refno),
        dbnum: refno.dbno(),
        noun: element.element_type()?,
        name: element.stored_name()?,
        catalogue: set.component.map(|component| CatalogueRefsDto {
            component: pdms_refno(component),
            point_set: set.point_set.map(pdms_refno),
        }),
        world_transform: world.to_cols_array(),
        unit: "mm",
        points: set
            .points
            .iter()
            .map(|point| PtsetPointDto {
                number: point.number,
                noun: point.noun.clone(),
                pt: point.pos.to_array(),
                dir: point.dir.to_array(),
                bore: point.bore,
            })
            .collect(),
        unresolved: set
            .unresolved
            .iter()
            .map(|point| UnresolvedPointDto {
                number: point.number,
                reason: point.reason.clone(),
            })
            .collect(),
    })
}

/// `a/b`，与 `tree/*`、`model/records` 的 `generation_root` 同一写法。
fn pdms_refno(refno: RefNo) -> String {
    format!("{}/{}", refno.word0, refno.word1 as i32)
}

fn db_error(error: DbElementError) -> ApiError {
    ApiError::from_domain(error.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空库上的 refno 是 404，不是 500：前端据此负缓存。
    #[test]
    fn a_missing_refno_is_not_found() {
        let set = Arc::new(DbSet::new(Arc::new(e3d_attlib::AttlibData::default())));
        let error = respond(&set, RefNo::new(1, 1), false).unwrap_err();
        assert_eq!(error.code, "not_found", "{}", error.message);
    }

    /// 请求体：`include_members` 缺省为 false，身份三元组可省。
    #[test]
    fn request_defaults() {
        let req: ElementPtsetReq = serde_json::from_str(r#"{"refno":"24381/145031"}"#).unwrap();
        assert_eq!(req.refno, "24381/145031");
        assert!(!req.include_members);
        let req: ElementPtsetReq =
            serde_json::from_str(r#"{"refno":"24381_145031","include_members":true}"#).unwrap();
        assert!(req.include_members);
    }

    /// 响应是「构件字段平铺 + 可选 members」，前端按 `points` / `members[].points` 取。
    #[test]
    fn response_flattens_the_element_and_omits_absent_members() {
        let response = ElementPtsetResponse {
            source: "e3d-model",
            element: ElementPtsetDto {
                refno: "24381/145031".into(),
                dbnum: 7997,
                noun: "ELBO".into(),
                name: None,
                catalogue: None,
                world_transform: glam::DMat4::IDENTITY.to_cols_array(),
                unit: "mm",
                points: vec![PtsetPointDto {
                    number: 1,
                    noun: "PTAX".into(),
                    pt: [0.0, 0.0, 0.0],
                    dir: [0.0, 0.0, 1.0],
                    bore: Some(100.0),
                }],
                unresolved: vec![],
            },
            members: None,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["source"], "e3d-model");
        assert_eq!(json["refno"], "24381/145031");
        assert_eq!(json["points"][0]["number"], 1);
        assert_eq!(json["world_transform"].as_array().unwrap().len(), 16);
        assert!(json.get("members").is_none());
    }
}
