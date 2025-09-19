use once_cell::sync::Lazy;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::data_interface::tidb_manager::AiosDBManager;

pub struct RuntimeState {
    pub env_id: String,
    pub mgr: Arc<AiosDBManager>,
    pub watcher_handle: Option<tokio::task::JoinHandle<()>>,
    pub mqtt_handle: Option<tokio::task::JoinHandle<()>>,
}

pub static REMOTE_RUNTIME: Lazy<RwLock<Option<RuntimeState>>> = Lazy::new(|| RwLock::new(None));

/// 停止当前运行态（如存在）
pub async fn stop_runtime() {
    let mut guard = REMOTE_RUNTIME.write().await;
    if let Some(state) = guard.as_mut() {
        if let Some(h) = state.watcher_handle.take() {
            h.abort();
        }
        if let Some(h) = state.mqtt_handle.take() {
            h.abort();
        }
    }
    *guard = None;
}

/// 使用当前 DbOption 配置启动 watcher + mqtt
pub async fn start_runtime(env_id: String) -> anyhow::Result<()> {
    let (init_ms, max_ms) = query_backoff_ms(&env_id).unwrap_or((1000, 30_000));
    let mgr = Arc::new(AiosDBManager::init_form_config().await?);
    // 初始化 watcher（避免 async_watch 前缺少初始扫描）
    mgr.init_watcher().await.ok();

    // 启动文件监听
    let mgr_clone = mgr.clone();
    let watcher_handle = tokio::spawn(async move {
        // 忽略错误并常驻循环
        let _ = mgr_clone.async_watch().await;
    });

    // 启动 MQTT 订阅
    let watcher_arc = mgr.watcher.clone();
    let mqtt_handle = tokio::spawn(async move {
        // 忽略错误并常驻循环
        AiosDBManager::poll_sync_e3d_mqtt_events_with_backoff(watcher_arc, init_ms, max_ms).await;
    });

    let mut guard = REMOTE_RUNTIME.write().await;
    *guard = Some(RuntimeState {
        env_id,
        mgr,
        watcher_handle: Some(watcher_handle),
        mqtt_handle: Some(mqtt_handle),
    });
    Ok(())
}

fn query_backoff_ms(env_id: &str) -> Option<(u64, u64)> {
    // 复用 handlers 中的配置文件约定
    use config as cfg;
    let db_path = if std::path::Path::new("DbOption.toml").exists() {
        cfg::Config::builder()
            .add_source(cfg::File::with_name("DbOption"))
            .build()
            .ok()
            .and_then(|b| b.get_string("deployment_sites_sqlite_path").ok())
            .unwrap_or_else(|| "deployment_sites.sqlite".to_string())
    } else {
        "deployment_sites.sqlite".to_string()
    };
    let conn = rusqlite::Connection::open(&db_path).ok()?;
    let mut stmt = conn
        .prepare("SELECT reconnect_initial_ms, reconnect_max_ms FROM remote_sync_envs WHERE id = ?1 LIMIT 1")
        .ok()?;
    let mut rows = stmt.query(rusqlite::params![env_id]).ok()?;
    let row = rows.next().ok()??;
    let init: Option<i64> = row.get(0).ok().flatten();
    let max: Option<i64> = row.get(1).ok().flatten();
    Some((init.unwrap_or(1000) as u64, max.unwrap_or(30_000) as u64))
}
