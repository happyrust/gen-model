use aios_database::web_ui::{handlers, AppState};
use axum::{routing::{get, post}, Router};
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 最小化 Web UI，仅包含桥架支撑检测与SCTN测试页面/接口
    let state = AppState::new();

    let app = Router::new()
        // 简单首页
        .route("/", get(|| async { axum::response::Html("<h3>Minimal Web UI</h3><ul><li><a href='/tray-supports'>桥架支撑检测</a></li><li><a href='/sctn-test'>SCTN 测试流程</a></li></ul>") }))
        // SQLite 支撑检测
        .route("/tray-supports", get(handlers::tray_supports_page))
        .route("/api/sqlite-tray-supports/detect", post(handlers::api_sqlite_tray_supports_detect))
        // SCTN 测试流程（后台任务 + 进度 + 结果）
        .route("/sctn-test", get(handlers::sctn_test_page))
        .route("/api/sctn-test/run", post(handlers::api_sctn_test_run))
        .route("/api/sctn-test/result/:id", get(handlers::api_sctn_test_result))
        // 任务进度查看（只读）
        .route("/tasks", get(handlers::tasks_page))
        .route("/api/tasks", get(handlers::get_tasks))
        .route("/api/tasks/:id", get(handlers::get_task))
        // 静态资源（可选）
        .nest_service("/static", ServeDir::new("src/web_ui/static"))
        .with_state(state);

    let port: u16 = std::env::var("WEB_UI_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(8000);
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).await?;
    println!("🚀 Minimal Web UI 启动: http://localhost:{}", port);
    axum::serve(listener, app).await?;
    Ok(())
}

