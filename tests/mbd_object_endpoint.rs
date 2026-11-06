//! `GET /api/mbd/v2/object/{refno}` 的 HTTP 层门（plant3d-web#103 M7）。
//!
//! 与 `mbd_http_endpoint.rs` 同一姿势：真 axum 服务起在回环随机端口，reqwest 打过去；路由主体走
//! `web_service::mbd::respond_object` / `respond`，只把「从 MDB 视图开 DbSet」换成 `common/mbd_db.rs` 的 AMS 优先
//! 链式开库。判型规则取本进程的 `[mbd_type_rules]`（仓根 `DbOption.toml` 没写这一段就是缺省表）。
//!
//! 200：BRAN（与 `/pipe` 同一份图元、多出 `annotation_type` / `subject`）、EQUI / 支架 STRU（含从 REST 配对过去）/
//! STWALL（信封；求解器还是占位的类型带 `layout.not_implemented`）；422 `not_annotatable`（PIPE）；400（refno 写法
//! 不对）；404（不存在）。样本库或模板目录缺席整体跳过。

#[path = "common/mbd_db.rs"]
mod mbd_db;

use std::sync::Arc;

use aios_database::fast_model::e3d_model_service::parse_refno;
use aios_database::web_service::ApiError;
use aios_database::web_service::mbd::{respond, respond_object};
use axum::Router;
use axum::extract::Path as AxumPath;
use axum::response::IntoResponse;
use axum::routing::get;
use e3d_io::db_element::DbSet;
use serde_json::Value;

/// 与生产路由同一条主体，只换开库那一步。
async fn serve(set: Arc<DbSet>) -> String {
    let pipe_set = set.clone();
    let app = Router::new()
        .route(
            "/api/mbd/v2/object/{refno}",
            get(move |AxumPath(refno): AxumPath<String>| {
                let set = set.clone();
                async move {
                    match parse_refno(&refno) {
                        Ok(parsed) => respond_object(set, parsed, refno).await.into_response(),
                        Err(error) => {
                            ApiError::bad_request(format!("refno `{refno}` 无法解析：{error:#}"))
                                .into_response()
                        }
                    }
                }
            }),
        )
        .route(
            "/api/mbd/v2/pipe/{refno}",
            get(move |AxumPath(refno): AxumPath<String>| {
                let set = pipe_set.clone();
                async move {
                    match parse_refno(&refno) {
                        Ok(parsed) => respond(set, parsed, refno).await.into_response(),
                        Err(error) => {
                            ApiError::bad_request(format!("refno `{refno}` 无法解析：{error:#}"))
                                .into_response()
                        }
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    format!("http://{addr}")
}

async fn get_json(client: &reqwest::Client, url: String) -> (u16, Value) {
    let response = client.get(&url).send().await.expect("GET");
    let status = response.status().as_u16();
    let body = response.json().await.expect("json body");
    (status, body)
}

/// 非管道类型的数据包：信封齐全。求解器还是占位（`meta.coverage` 为空）时没有图元、最后一条是
/// `layout.not_implemented`；某个 lane 已落地求解器时（设备 2026-11-07 起），宿主这边仍只给归属视图，求解器按缺数据
/// 出声，这里只核信封。
fn assert_object_envelope(
    body: &Value,
    input: &str,
    annotation_type: &str,
    subject_refno: &str,
    subject_noun: &str,
    dbnum: u32,
) {
    assert_eq!(body["version"], "v2", "{body}");
    assert_eq!(body["input_refno"], input);
    assert_eq!(body["branch_refno"], subject_refno);
    assert_eq!(body["annotation_type"], annotation_type);
    assert_eq!(body["subject"]["refno"], subject_refno);
    assert_eq!(body["subject"]["noun"], subject_noun);
    assert_eq!(body["meta"]["layout_mode"], annotation_type);
    assert_eq!(body["meta"]["geometry_space"], "source_mm");
    assert_eq!(
        body["meta"]["source_to_design"].as_array().map(Vec::len),
        Some(16)
    );
    assert_eq!(body["meta"]["source_version"]["dbnum"], dbnum);
    assert!(body["meta"]["source_version"]["sesno"].as_u64().is_some());
    let issues = body["issues"].as_array().expect("issues");
    let coverage = body["meta"]["coverage"]
        .as_array()
        .expect("coverage declared");
    if coverage.is_empty() {
        assert_eq!(body["primitives"], serde_json::json!([]));
        let last = issues.last().expect("not_implemented issue");
        assert_eq!(last["rule_id"], "layout.not_implemented");
        assert_eq!(last["severity"], "info");
        assert_eq!(last["refno"], subject_refno);
    } else {
        let rules: Vec<&str> = issues
            .iter()
            .filter_map(|issue| issue["rule_id"].as_str())
            .collect();
        eprintln!("{annotation_type} solver landed: coverage {coverage:?}, issues {rules:?}");
        assert!(!rules.contains(&"layout.not_implemented"));
    }
    // wire 体就是冻结契约的形状。
    let parsed: plant_mbd::MbdV2PipeData =
        serde_json::from_value(body.clone()).expect("body is MbdV2PipeData");
    assert_eq!(
        parsed.annotation_type.map(|t| t.wire_name()),
        Some(annotation_type)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn object_endpoint_classifies_and_maps_errors() {
    let Some(set) = mbd_db::open(&["ams7999_0001", "ams7997_0001"]) else {
        return;
    };
    let base = serve(set).await;
    let client = reqwest::Client::new();

    // 200 管道：与 /pipe 同一份图元 / meta / issues，只多 annotation_type 与 subject。
    let (status, object) = get_json(&client, format!("{base}/api/mbd/v2/object/24383_99996")).await;
    assert_eq!(status, 200, "{object}");
    assert_eq!(object["annotation_type"], "em4_pipe");
    assert_eq!(
        object["subject"],
        serde_json::json!({ "refno": "24383_99996", "noun": "BRAN" })
    );
    assert_eq!(object["branch_refno"], "24383_99996");
    assert_eq!(object["input_refno"], "24383_99996");
    let (status, pipe) = get_json(&client, format!("{base}/api/mbd/v2/pipe/24383_99996")).await;
    assert_eq!(status, 200);
    assert!(
        pipe.get("annotation_type").is_none() && pipe.get("subject").is_none(),
        "/pipe 输出不变"
    );
    for key in [
        "primitives",
        "meta",
        "issues",
        "branch_refno",
        "input_refno",
        "version",
    ] {
        assert_eq!(
            object[key], pipe[key],
            "{key} differs between /object and /pipe"
        );
    }
    let linear = object["primitives"]
        .as_array()
        .expect("primitives")
        .iter()
        .filter(|primitive| primitive["kind"] == "linear_dim")
        .count();
    assert!(linear >= 2, "got {linear} linear_dim");

    // 选中管件：归属对象是所属 BRAN，input_refno 原样回填（斜杠写法经 URL 编码）。
    let (status, elbow) =
        get_json(&client, format!("{base}/api/mbd/v2/object/24383%2F100013")).await;
    assert_eq!(status, 200, "{elbow}");
    assert_eq!(elbow["input_refno"], "24383/100013");
    assert_eq!(elbow["branch_refno"], "24383_99996");
    assert_eq!(elbow["subject"]["noun"], "BRAN");
    assert_eq!(elbow["primitives"], object["primitives"]);

    // 200 设备 / 支架 / 建筑墙：信封；求解器还是占位的类型再带 layout.not_implemented。
    let (status, equi) = get_json(&client, format!("{base}/api/mbd/v2/object/24383_76416")).await;
    assert_eq!(status, 200, "{equi}");
    assert_object_envelope(
        &equi,
        "24383_76416",
        "em4_equi",
        "24383_76416",
        "EQUI",
        7999,
    );

    let (status, stru) = get_json(&client, format!("{base}/api/mbd/v2/object/24383_69033")).await;
    assert_eq!(status, 200, "{stru}");
    assert_object_envelope(
        &stru,
        "24383_69033",
        "em4_supp",
        "24383_69033",
        "STRU",
        7999,
    );

    let (status, rest) = get_json(&client, format!("{base}/api/mbd/v2/object/24383_69027")).await;
    assert_eq!(status, 200, "{rest}");
    assert_object_envelope(
        &rest,
        "24383_69027",
        "em4_supp",
        "24383_69033",
        "STRU",
        7999,
    );

    let (status, wall) = get_json(&client, format!("{base}/api/mbd/v2/object/24381_37115")).await;
    assert_eq!(status, 200, "{wall}");
    assert_object_envelope(&wall, "24381_37115", "arch", "24381_37115", "STWALL", 7997);

    // 422 not_annotatable：PIPE 是 BRAN 的容器。
    let (status, pipe_owner) =
        get_json(&client, format!("{base}/api/mbd/v2/object/24383_99742")).await;
    assert_eq!(status, 422, "{pipe_owner}");
    assert_eq!(pipe_owner["code"], "not_annotatable");
    assert_eq!(pipe_owner["detail"]["refno"], "24383_99742");
    assert_eq!(pipe_owner["detail"]["noun"], "PIPE");
    assert!(
        pipe_owner["detail"]["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty())
    );

    // 400：refno 写法不对。
    let (status, garbage) =
        get_json(&client, format!("{base}/api/mbd/v2/object/not-a-refno")).await;
    assert_eq!(status, 400);
    assert_eq!(garbage["code"], "bad_request");

    // 404：所钉会话里没有这个元素。
    let (status, missing) =
        get_json(&client, format!("{base}/api/mbd/v2/object/24383_999999999")).await;
    assert_eq!(status, 404, "{missing}");
    assert_eq!(missing["code"], "not_found");
}
