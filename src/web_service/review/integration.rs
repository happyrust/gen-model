//! 校审域 · 模型侧集成（`POST /api/review/aux-data`、`GET /api/review/collision-data`）。
//!
//! 旧端 `src/web_api/review_integration.rs`（377 行）的对外形状**逐字保留**：
//! - 请求 / 响应结构体与 serde 重命名（`ObjectOneLoc` / `ObjectTowLoc` / `ObjectTow` 的拼写沿用旧端，
//!   前端 `reviewApi.ts::CollisionItem` 就是按这个拼写读的，计划 N4：不「顺手改好」）；
//! - `aux-data` 的 `UCode` / `UKey` 头校验：与环境变量 `AUX_DATA_UCODE`（缺省 `ZY`）/ `AUX_DATA_UKEY`
//!   （缺省空串）比对，缺头或不匹配一律 401 空体；
//! - 两端点成功一律 HTTP 200：`aux-data` 回 `{code:200,message:"ok",page,page_size,total,data:{collision,
//!   quality,otverification,rules}}`，`collision-data` 回 `{success:true,data,total}`；`page` / `page_size`
//!   原样回显请求值。
//!
//! 与旧端的**唯一差异**（计划 Q4 拍板，验收 A10）：碰撞数据从哪来。
//! 旧端从 `aios_core::project_primary_db()` 直读 `collision_events` 表，查不到或查询失败时**静默回落到
//! `generate_mock_collisions` 造两条假碰撞**——那正是宪法 III 定义的静默失效。gen-model 的模型数据面是
//! e3d-direct / RocksDB（`QueryService` / direct store），既没有 `collision_events` 同形表，也没有任何写入
//! 碰撞事件的生产者（旧端 `collision_api.rs` 的 `POST /api/collision/detect` 自己就是一句
//! `Collision detection feature is not available` 的空壳，所以旧端线上这两条接口实际交付的一直是 mock）。
//! 因此本文件：
//! - 用 [`CollisionSource`] 把「碰撞数据源」收成一个接缝；今天唯一的形态是 [`CollisionSource::Unavailable`]，
//!   原因是常量 [`COLLISION_SOURCE_UNAVAILABLE_REASON`]；
//! - 没有数据源时返回**显式空集 + `reason` 字段**：两端点各多出一个 `reason: Option<String>`，
//!   有数据源时不序列化，键集与旧端逐字相同；
//! - **不造假的数据面、不返回 mock**。`collision_events` 的接入记在 `docs/adr/ADR-072-review-data-persistence.md` §未决「碰撞数据源待接」，
//!   接的时候只换 [`collision_source`] 的实现（把 `AppState.queries` 穿进来），两条 handler 的形状不动。
//!
//! 文件归 T4（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）；规范 `docs/specs/review-api.md` §9。

use axum::{
    Json, Router,
    extract::Query,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use log::{info, warn};
use serde::{Deserialize, Serialize};

// ============================================================================
// Routes
// ============================================================================

/// 模型侧集成路由（旧端 `create_review_integration_routes`）。
///
/// 不压 `review_auth_middleware`：旧端这两条就注册在 JWT 中间件之外——`aux-data` 自带 `UCode` / `UKey`
/// 头校验（面向 PMS / 模型中心的 S2S 口令），`collision-data` 匿名可读；移植不改鉴权面。
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/review/aux-data", post(get_aux_data))
        .route("/api/review/collision-data", get(get_collision_data))
}

// ============================================================================
// 契约类型（与旧端 review_integration.rs 逐字一致；`reason` 是唯一新增）
// ============================================================================

/// `POST /api/review/aux-data` 请求体（前端 `reviewApi.ts::AuxDataRequest`）。
///
/// `page` / `page_size` 按旧端收 `i32`（前端类型虽写 `number | string`，但旧端收到字符串就是 422，
/// 契约以现状为准）。
#[derive(Debug, Deserialize)]
pub struct AuxDataRequest {
    pub project_id: String,
    pub model_refnos: Vec<String>,
    pub major: String,
    pub requester_id: String,
    pub page: i32,
    pub page_size: i32,
    pub form_id: String,
    pub new_search: Option<bool>,
}

/// `POST /api/review/aux-data` 响应（前端 `reviewApi.ts::AuxDataResponse`）。
#[derive(Debug, Serialize)]
pub struct AuxDataResponse {
    pub code: i32,
    pub message: String,
    pub page: i32,
    pub page_size: i32,
    pub total: i32,
    pub data: AuxDataContent,
    /// 碰撞数据源不可用时的显式说明（A10）；有数据源时不出现，键集与旧端相同。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 四类辅助数据。旧端只实现了 `collision`，其余三类是空占位结构体（序列化为 `[]`），原样保留。
#[derive(Debug, Serialize, Default)]
pub struct AuxDataContent {
    pub collision: Vec<CollisionItem>,
    pub quality: Vec<QualityItem>,
    pub otverification: Vec<VerificationItem>,
    pub rules: Vec<RuleItem>,
}

/// 一条碰撞记录。键名（含 `ObjectTowLoc` / `ObjectTow` 的拼写）是前端写死的契约。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollisionItem {
    #[serde(rename = "ObjectOneLoc")]
    pub object_one_loc: String,
    #[serde(rename = "ObjectOne")]
    pub object_one: String,
    #[serde(rename = "ObjectTowLoc")]
    pub object_two_loc: String,
    #[serde(rename = "ObjectTow")]
    pub object_two: String,
    #[serde(rename = "ErrorMsg")]
    pub error_msg: String,
    #[serde(rename = "ObjectOneMajor")]
    pub object_one_major: String,
    #[serde(rename = "ObjectTwoMajor")]
    pub object_two_major: String,
    #[serde(rename = "CheckUsr")]
    pub check_usr: String,
    #[serde(rename = "CheckDate")]
    pub check_date: String,
    #[serde(rename = "UpUsr")]
    pub up_usr: String,
    #[serde(rename = "UpTime")]
    pub up_time: String,
    #[serde(rename = "ErrorStatus")]
    pub error_status: String,
}

/// 旧端占位（PMS 质量校验数据）；无字段，序列化为 `{}`，列表恒为空。
#[derive(Debug, Clone, Serialize)]
pub struct QualityItem {}

/// 旧端占位（PMS 二三维校验数据）。
#[derive(Debug, Clone, Serialize)]
pub struct VerificationItem {}

/// 旧端占位（PMS 规则校验数据）。
#[derive(Debug, Clone, Serialize)]
pub struct RuleItem {}

/// `GET /api/review/collision-data` 查询串（前端 `reviewApi.ts::CollisionQueryParams`）。
#[derive(Debug, Deserialize)]
pub struct CollisionQueryParams {
    pub project_id: Option<String>,
    pub refno: Option<String>,
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}

/// `GET /api/review/collision-data` 响应（前端 `reviewApi.ts::CollisionDataResponse`）。
#[derive(Debug, Serialize)]
pub struct CollisionDataResponse {
    pub success: bool,
    pub data: Vec<CollisionItem>,
    pub total: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// 碰撞数据源不可用时的显式说明（A10）；有数据源时不出现，键集与旧端相同。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ============================================================================
// 碰撞数据源接缝
// ============================================================================

/// 碰撞数据源不可用时写进 `reason` 的固定说明。
///
/// 以稳定 token `collision_source_unavailable` 开头，便于脚本判别；后面是给人看的原因与出处。
pub const COLLISION_SOURCE_UNAVAILABLE_REASON: &str = "collision_source_unavailable: gen-model 的模型数据面（e3d-direct/RocksDB）没有 collision_events 同形表，也没有碰撞事件生产者；碰撞数据源待接（docs/plans/2026-09-12-review-backend-port-plan.md Q4；docs/adr/ADR-072-review-data-persistence.md §未决）。本响应是显式空集，不是「查到了零条」。";

/// 「碰撞数据从哪来」的接缝。
///
/// 接入真实数据源时在这里加一个变体（持有 `QueryService` / direct store 句柄或 PMS 代理客户端），
/// 并让 [`collision_source`] 按配置返回它；两条 handler 只通过 [`CollisionSource::query`] 取数，形状不变。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollisionSource {
    /// 没有可用的碰撞数据源。`reason` 原样进响应。
    Unavailable { reason: &'static str },
}

/// 一次碰撞查询的结果：要么有数据源并给出行，要么没有数据源并给出原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollisionQueryOutcome {
    Rows(Vec<CollisionItem>),
    Unavailable { reason: &'static str },
}

impl CollisionSource {
    /// 按 refno 集合查碰撞（`aux-data`：`model_refnos`；`collision-data`：`refno` 单值或不限）。
    ///
    /// `limit` / `offset` 只对真实数据源有意义；不可用形态下原样忽略。
    pub fn query(&self, refnos: &[String], limit: i32, offset: i32) -> CollisionQueryOutcome {
        match self {
            CollisionSource::Unavailable { reason } => {
                info!(
                    "[REVIEW.integration] collision source unavailable; refnos={} limit={} offset={}",
                    refnos.len(),
                    limit,
                    offset
                );
                CollisionQueryOutcome::Unavailable { reason }
            }
        }
    }
}

/// 当前进程的碰撞数据源。
///
/// 今天恒为 [`CollisionSource::Unavailable`]：gen-model 没有 `collision_events` 的来源（见模块文档）。
/// 接入时把 `AppState.queries` 穿进来、按 `DbOptionExt` 配置挑实现，调用方不改。
pub fn collision_source() -> CollisionSource {
    CollisionSource::Unavailable {
        reason: COLLISION_SOURCE_UNAVAILABLE_REASON,
    }
}

// ============================================================================
// 鉴权（旧端 `expected_aux_auth`）
// ============================================================================

/// `aux-data` 的期望口令：环境变量 `AUX_DATA_UCODE`（缺省 `ZY`）/ `AUX_DATA_UKEY`（缺省空串）。
///
/// 前端 `AUX_DATA_DEFAULT_AUTH.uKey` 是一枚非空 token，所以服务端不设 `AUX_DATA_UKEY` 时前端默认口令
/// 会被 401 —— 这是旧端就有的部署口径，移植不改。
fn expected_aux_auth() -> (String, String) {
    let u_code = std::env::var("AUX_DATA_UCODE").unwrap_or_else(|_| "ZY".to_string());
    let u_key = std::env::var("AUX_DATA_UKEY").unwrap_or_default();
    (u_code, u_key)
}

/// 校验 `UCode` / `UKey` 头（HTTP 头名不区分大小写）。缺头与不匹配都是 401，与旧端相同。
fn check_aux_auth(headers: &HeaderMap) -> Result<(), StatusCode> {
    let u_code = headers.get("UCode").and_then(|v| v.to_str().ok());
    let u_key = headers.get("UKey").and_then(|v| v.to_str().ok());
    let (expected_ucode, expected_ukey) = expected_aux_auth();

    if u_code.is_none() || u_key.is_none() {
        warn!("[REVIEW.integration] aux-data request missing UCode/UKey headers");
        return Err(StatusCode::UNAUTHORIZED);
    }
    if u_code != Some(expected_ucode.as_str()) || u_key != Some(expected_ukey.as_str()) {
        warn!("[REVIEW.integration] aux-data request with invalid UCode/UKey headers");
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

// ============================================================================
// Handlers
// ============================================================================

/// `POST /api/review/aux-data` —— 外部模型中心 / PMS 查询辅助校审数据（碰撞 / 质量 / 二三维 / 规则）。
async fn get_aux_data(
    headers: HeaderMap,
    Json(request): Json<AuxDataRequest>,
) -> Result<Json<AuxDataResponse>, StatusCode> {
    check_aux_auth(&headers)?;

    info!(
        "[REVIEW.integration] aux-data project_id={} form_id={} major={} requester_id={} refnos={} page={} page_size={} new_search={:?}",
        request.project_id,
        request.form_id,
        request.major,
        request.requester_id,
        request.model_refnos.len(),
        request.page,
        request.page_size,
        request.new_search
    );

    let (collision, reason) = match collision_source().query(
        &request.model_refnos,
        request.page_size,
        (request.page.max(1) - 1).saturating_mul(request.page_size.max(0)),
    ) {
        CollisionQueryOutcome::Rows(rows) => (rows, None),
        CollisionQueryOutcome::Unavailable { reason } => (Vec::new(), Some(reason.to_string())),
    };
    let total = collision.len() as i32;

    Ok(Json(AuxDataResponse {
        code: 200,
        message: "ok".to_string(),
        page: request.page,
        page_size: request.page_size,
        total,
        data: AuxDataContent {
            collision,
            ..Default::default()
        },
        reason,
    }))
}

/// `GET /api/review/collision-data` —— 按 refno 查碰撞记录（匿名，前端「碰撞数据查询」面板）。
async fn get_collision_data(Query(params): Query<CollisionQueryParams>) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);
    info!(
        "[REVIEW.integration] collision-data project_id={:?} refno={:?} limit={} offset={}",
        params.project_id, params.refno, limit, offset
    );

    let refnos: Vec<String> = params.refno.iter().cloned().collect();
    let (data, reason) = match collision_source().query(&refnos, limit, offset) {
        CollisionQueryOutcome::Rows(rows) => (rows, None),
        CollisionQueryOutcome::Unavailable { reason } => (Vec::new(), Some(reason.to_string())),
    };
    let total = data.len() as i32;

    (
        StatusCode::OK,
        Json(CollisionDataResponse {
            success: true,
            data,
            total,
            error_message: None,
            reason,
        }),
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::Request};
    use tower::ServiceExt;

    fn sorted_keys(value: &serde_json::Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("payload must be a JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    fn app() -> Router<()> {
        routes::<()>()
    }

    /// 旧端 `tests::sample_request_body` 原样。
    fn sample_request_body() -> String {
        serde_json::json!({
            "project_id": "2410",
            "model_refnos": ["24381_1001"],
            "major": "pipe",
            "requester_id": "user-001",
            "page": 1,
            "page_size": 20,
            "form_id": "FORM-123"
        })
        .to_string()
    }

    async fn json_body(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json body")
    }

    /// 旧端 `test_aux_data_rejects_incorrect_auth_headers` 原样。
    #[tokio::test]
    async fn aux_data_rejects_incorrect_auth_headers() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/review/aux-data")
                    .header("content-type", "application/json")
                    .header("UCode", "WRONG")
                    .header("UKey", "WRONG")
                    .body(Body::from(sample_request_body()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// 缺头与错头同为 401（旧端两条 `return Err(UNAUTHORIZED)` 分支的第一条）。
    #[tokio::test]
    async fn aux_data_rejects_missing_auth_headers() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/review/aux-data")
                    .header("content-type", "application/json")
                    .body(Body::from(sample_request_body()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// 旧端 `test_aux_data_accepts_matching_auth_headers` + A10：口令对上是 200；没有数据源时
    /// 是显式空集 + `reason`，不是 mock 的两条假碰撞；`page` / `page_size` 回显；键集比旧端只多 `reason`。
    #[tokio::test]
    async fn aux_data_accepts_matching_auth_headers_and_answers_explicit_empty_set_with_reason() {
        let (u_code, u_key) = expected_aux_auth();
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/review/aux-data")
                    .header("content-type", "application/json")
                    .header("ucode", u_code)
                    .header("ukey", u_key)
                    .body(Body::from(sample_request_body()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let payload = json_body(response).await;
        assert_eq!(
            sorted_keys(&payload),
            ["code", "data", "message", "page", "page_size", "reason", "total"]
        );
        assert_eq!(payload["code"], 200);
        assert_eq!(payload["message"], "ok");
        assert_eq!(payload["page"], 1);
        assert_eq!(payload["page_size"], 20);
        assert_eq!(payload["total"], 0);
        assert_eq!(
            sorted_keys(&payload["data"]),
            ["collision", "otverification", "quality", "rules"]
        );
        for bucket in ["collision", "quality", "otverification", "rules"] {
            assert_eq!(payload["data"][bucket], serde_json::json!([]), "{bucket}");
        }
        let reason = payload["reason"].as_str().expect("reason is a string");
        assert!(
            reason.starts_with("collision_source_unavailable"),
            "reason must start with the stable token: {reason}"
        );
        assert_eq!(reason, COLLISION_SOURCE_UNAVAILABLE_REASON);
    }

    /// A10：`collision-data` 匿名可读，没有数据源时 `success:true` + 空 `data` + `total:0` + `reason`；
    /// 键集比旧端只多 `reason`（`error_message` 为 None 时旧端也不序列化）。
    #[tokio::test]
    async fn collision_data_answers_explicit_empty_set_with_reason() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/review/collision-data?refno=24381_145018&limit=100&offset=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let payload = json_body(response).await;
        assert_eq!(sorted_keys(&payload), ["data", "reason", "success", "total"]);
        assert_eq!(payload["success"], true);
        assert_eq!(payload["data"], serde_json::json!([]));
        assert_eq!(payload["total"], 0);
        assert_eq!(payload["reason"], COLLISION_SOURCE_UNAVAILABLE_REASON);
    }

    /// 查询串全部可选（前端 `reviewGetCollisionData({})` 不带任何参数）；类型不合是 400（axum `Query` 拒绝），
    /// 与旧端一致。
    #[tokio::test]
    async fn collision_data_accepts_empty_query_and_rejects_malformed_numbers() {
        let ok = app()
            .oneshot(
                Request::builder()
                    .uri("/api/review/collision-data")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);

        let bad = app()
            .oneshot(
                Request::builder()
                    .uri("/api/review/collision-data?limit=abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    }

    /// 有数据源时的形状就是旧端的形状：`reason` 不出现，`CollisionItem` 的 12 个键名（含 `ObjectTow*` 拼写）
    /// 逐字对齐 `reviewApi.ts::CollisionItem`。改这里的 rename 先红。
    #[test]
    fn legacy_shapes_are_preserved_when_a_source_exists() {
        let item = CollisionItem {
            object_one_loc: "/PIPE-001".into(),
            object_one: "24381_145018".into(),
            object_two_loc: "/STRU-002".into(),
            object_two: "24381_145020".into(),
            error_msg: "硬碰撞".into(),
            object_one_major: "管道".into(),
            object_two_major: "结构".into(),
            check_usr: "system".into(),
            check_date: "2026-09-13".into(),
            up_usr: String::new(),
            up_time: String::new(),
            error_status: "pending".into(),
        };
        let item_json = serde_json::to_value(&item).expect("serialize");
        assert_eq!(
            sorted_keys(&item_json),
            [
                "CheckDate",
                "CheckUsr",
                "ErrorMsg",
                "ErrorStatus",
                "ObjectOne",
                "ObjectOneLoc",
                "ObjectOneMajor",
                "ObjectTow",
                "ObjectTowLoc",
                "ObjectTwoMajor",
                "UpTime",
                "UpUsr",
            ]
        );

        let aux = serde_json::to_value(AuxDataResponse {
            code: 200,
            message: "ok".into(),
            page: 1,
            page_size: 20,
            total: 1,
            data: AuxDataContent {
                collision: vec![item.clone()],
                ..Default::default()
            },
            reason: None,
        })
        .expect("serialize");
        assert_eq!(
            sorted_keys(&aux),
            ["code", "data", "message", "page", "page_size", "total"]
        );

        let collision = serde_json::to_value(CollisionDataResponse {
            success: true,
            data: vec![item],
            total: 1,
            error_message: None,
            reason: None,
        })
        .expect("serialize");
        assert_eq!(sorted_keys(&collision), ["data", "success", "total"]);
    }

    /// 接缝的当前形态：没有数据源，且 `query` 把同一条原因原样交出来（handler 不另编一套文案）。
    #[test]
    fn collision_source_is_unavailable_today_and_reports_the_fixed_reason() {
        let source = collision_source();
        assert_eq!(
            source,
            CollisionSource::Unavailable {
                reason: COLLISION_SOURCE_UNAVAILABLE_REASON
            }
        );
        assert_eq!(
            source.query(&["24381_1001".to_string()], 100, 0),
            CollisionQueryOutcome::Unavailable {
                reason: COLLISION_SOURCE_UNAVAILABLE_REASON
            }
        );
        assert!(COLLISION_SOURCE_UNAVAILABLE_REASON.starts_with("collision_source_unavailable:"));
    }

    /// 路由清单护栏（宪法 VI · 计划 §6 A10 的静态半边）：`routes()` 里恰好是计划 §1.2「模型侧集成」
    /// 那两条方法·路径，少一条前端静默 404、多一条是契约外扩面。`common.rs` 的总护栏明确把这两条
    /// 留给本模块自己钉。
    #[test]
    fn integration_routes_are_registered_verbatim() {
        let source = include_str!("integration.rs");
        let body = source
            .split_once("pub fn routes<S>() -> Router<S>")
            .expect("routes must exist")
            .1
            .split_once("\n}")
            .expect("routes body must close")
            .0;
        let registrations: Vec<&str> = body
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with(".route("))
            .collect();
        assert_eq!(
            registrations,
            [
                ".route(\"/api/review/aux-data\", post(get_aux_data))",
                ".route(\"/api/review/collision-data\", get(get_collision_data))",
            ],
            "{body}"
        );
    }
}
