//! `GET /api/mbd/v2/pipe/{refno}` 的 HTTP 层门（计划 `2026-09-06-plant-mbd-pipe-dimension` Phase 2，D6）。
//!
//! 起一个真 axum 服务在回环随机端口上，用 reqwest 打三条路径——等价于计划 §5 的三条 `curl`：
//! 200（真实 BRAN，V2 契约体）、404（refno 不存在）、422（不是派生隐式管身的路由容器）；再加 400（refno 写法不对）。
//! 路由主体走 `web_service::mbd::respond`，只把「从 MDB 视图开 DbSet」那一步换成目录扫描出来的库——
//! `AppState` 那一坨（SurrealDB 管理器、任务注册表…）与本端点无关，不为一个 GET 起整套服务。
//!
//! 样本库与模板目录不随仓库走，缺哪个都整体跳过（与 `mbd_branch_query.rs` 同款）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aios_database::data_interface::direct_store::TEMPLATE_DIR_ENV;
use aios_database::fast_model::e3d_model_service::parse_refno;
use aios_database::web_service::mbd::respond;
use axum::Router;
use axum::extract::Path as AxumPath;
use axum::response::IntoResponse;
use axum::routing::get;
use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
use e3d_model::db_discovery::DirectoryDbResolver;

const AMS000: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000";
const AMS7999: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001";
const TEMPLATE_DIR_DEFAULT: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all";

fn fixtures() -> Option<(PathBuf, PathBuf)> {
    let db = PathBuf::from(AMS7999);
    let templates = std::env::var_os(TEMPLATE_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(TEMPLATE_DIR_DEFAULT));
    if !db.is_file() || !Path::new(AMS000).is_dir() || !templates.join("attlib.dat").is_file() {
        eprintln!("SKIP: fixtures not present ({db:?}, {templates:?})");
        return None;
    }
    Some((db, templates))
}

fn open_set(db: &Path, templates: &Path) -> Arc<DbSet> {
    let resolver =
        DirectoryDbResolver::scan(&[PathBuf::from(AMS000)], templates).expect("scan ams000");
    let set =
        DbSet::with_attlib_file_and_resolver(templates.join("attlib.dat"), Box::new(resolver))
            .expect("DbSet opens")
            .with_element_cache();
    set.add_db(DbFilePin {
        file: db.to_path_buf(),
        template: template_file_for(templates, "DESI").expect("DESI template"),
        db_type: Some("DESI".to_string()),
        sesno: None,
    })
    .expect("pin ams7999");
    Arc::new(set)
}

/// 与生产路由同一条主体，只换开库那一步。
async fn serve(set: Arc<DbSet>) -> String {
    let app = Router::new().route(
        "/api/mbd/v2/pipe/{refno}",
        get(move |AxumPath(refno): AxumPath<String>| {
            let set = set.clone();
            async move {
                match parse_refno(&refno) {
                    Ok(parsed) => respond(set, parsed, refno).await.into_response(),
                    Err(error) => aios_database::web_service::ApiError::bad_request(format!(
                        "refno `{refno}` 无法解析：{error:#}"
                    ))
                    .into_response(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_curl_paths_plus_bad_request() {
    let Some((db, templates)) = fixtures() else {
        return;
    };
    let set = open_set(&db, &templates);
    let base = serve(set.clone()).await;
    let client = reqwest::Client::new();

    // 200：真实 BRAN → V2 契约体。
    let ok = client
        .get(format!("{base}/api/mbd/v2/pipe/24383_99996"))
        .send()
        .await
        .expect("GET 200");
    assert_eq!(ok.status(), 200);
    let body: serde_json::Value = ok.json().await.expect("json body");
    assert_eq!(body["version"], "v2");
    assert_eq!(body["branch_refno"], "24383_99996");
    assert_eq!(body["input_refno"], "24383_99996");
    assert_eq!(body["meta"]["geometry_space"], "source_mm");
    assert_eq!(
        body["meta"]["source_to_design"].as_array().map(Vec::len),
        Some(16)
    );
    let linear = body["primitives"]
        .as_array()
        .expect("primitives")
        .iter()
        .filter(|primitive| primitive["kind"] == "linear_dim")
        .count();
    assert!(
        linear >= 2,
        "expected several linear_dim primitives, got {linear}"
    );
    assert!(body["issues"].is_array());
    // 前端契约的严格校验在 plant-mbd `contract_guard` 里钉着；这里只确认 wire 形状到位。
    // 设了 `MBD_DUMP_DIR` 就把 wire 体原样落盘，给前端 `parseMbdV2PipeData` 做一次真身校验。
    if let Some(dir) = std::env::var_os("MBD_DUMP_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("dump dir");
        std::fs::write(
            dir.join("24383_99996.json"),
            serde_json::to_string_pretty(&body).expect("pretty"),
        )
        .expect("write dump");
    }
    let parsed: plant_mbd::MbdV2PipeData =
        serde_json::from_value(body).expect("body is MbdV2PipeData");
    assert_eq!(parsed.meta.layout_mode.as_deref(), Some("linear_mvp"));

    // 同一 refno 用斜杠写法（URL 编码）也要通，且 input_refno 原样回填。
    let slash = client
        .get(format!("{base}/api/mbd/v2/pipe/24383%2F99996"))
        .send()
        .await
        .expect("GET slash");
    assert_eq!(slash.status(), 200);
    let slash: serde_json::Value = slash.json().await.expect("json");
    assert_eq!(slash["input_refno"], "24383/99996");
    assert_eq!(slash["branch_refno"], "24383_99996");

    // 404：所钉会话里没有这个元素。
    let missing = client
        .get(format!("{base}/api/mbd/v2/pipe/24383_999999999"))
        .send()
        .await
        .expect("GET 404");
    assert_eq!(missing.status(), 404);
    let missing: serde_json::Value = missing.json().await.expect("json");
    assert_eq!(missing["code"], "not_found");

    // 422：BRAN 的属主 PIPE 存在，但不是派生隐式管身的路由容器。
    let bran = set.element(e3d_io::refno::RefNo::new(24383, 99996));
    let pipe = bran.owner().expect("owner").expect("BRAN has an owner");
    let pipe_refno = format!("{}_{}", pipe.refno().word0, pipe.refno().word1 as i32);
    let container = client
        .get(format!("{base}/api/mbd/v2/pipe/{pipe_refno}"))
        .send()
        .await
        .expect("GET 422");
    assert_eq!(
        container.status(),
        422,
        "owner {pipe_refno} must be rejected as a non-route container"
    );
    let container: serde_json::Value = container.json().await.expect("json");
    assert_eq!(container["code"], "precondition");

    // 400：refno 写法不对。
    let garbage = client
        .get(format!("{base}/api/mbd/v2/pipe/not-a-refno"))
        .send()
        .await
        .expect("GET 400");
    assert_eq!(garbage.status(), 400);
    let garbage: serde_json::Value = garbage.json().await.expect("json");
    assert_eq!(garbage["code"], "bad_request");
}
