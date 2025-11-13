use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde_json::json;
use std::time::SystemTime;

use crate::web_server::{
    AppState,
    models::{
        DbStatusInfo, DbStatusQuery, FileVersionInfo, IncrementalUpdateRequest, MeshStatus,
        ModelStatus, ParseStatus, UpdateType,
    },
};

// 引入真实实现作为委托
use crate::fast_model::session::{PdmsTimeExtractor, SESSION_STORE};
use crate::web_server::handlers as real_handlers;
use aios_core::SUL_DB;
use aios_core::get_db_option;

pub async fn get_db_status_list(
    State(_state): State<AppState>,
    Query(params): Query<DbStatusQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 从 SurrealDB 查询真实的 db 信息
    let sql = "SELECT dbnum, file_name, sesno, project FROM dbnum_info_table ORDER BY dbnum";

    let mut db_statuses: Vec<DbStatusInfo> = Vec::new();

    match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let rows: Vec<serde_json::Value> = response.take(0).unwrap_or_default();

            for row in rows {
                if let Some(db_status) = convert_row_to_status(row).await {
                    // 过滤条件
                    let mut include = true;
                    if let Some(ref project) = params.project {
                        if db_status.project != *project {
                            include = false;
                        }
                    }
                    if let Some(ref db_type) = params.db_type {
                        if !db_type.is_empty() && db_status.db_type != *db_type {
                            include = false;
                        }
                    }
                    if let Some(true) = params.needs_update_only {
                        if !db_status.needs_update {
                            include = false;
                        }
                    }
                    if include {
                        db_statuses.push(db_status);
                    }
                }
            }
        }
        Err(_) => {
            // 查询失败返回空数据
        }
    }

    let total = db_statuses.len();
    // 分页
    if let Some(limit) = params.limit {
        let offset = params.offset.unwrap_or(0);
        db_statuses = db_statuses.into_iter().skip(offset).take(limit).collect();
    }

    Ok(Json(json!({
        "status": "success",
        "data": db_statuses,
        "total": total,
        "page": params.offset.unwrap_or(0) / params.limit.unwrap_or(10),
        "page_size": params.limit.unwrap_or(10)
    })))
}

pub async fn get_db_status_detail(
    State(_state): State<AppState>,
    Path(dbnum): Path<u32>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let sql = format!(
        "SELECT dbnum, file_name, sesno, project FROM dbnum_info_table WHERE dbnum = {} LIMIT 1",
        dbnum
    );

    match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let rows: Vec<serde_json::Value> = response.take(0).unwrap_or_default();
            if let Some(row) = rows.into_iter().next() {
                if let Some(info) = convert_row_to_status(row).await {
                    let change_log = vec![json!({
                        "version": info.sesno,
                        "date": "",
                        "changes": "",
                        "records_changed": info.count
                    })];

                    let related_files = vec![json!({
                        "file_type": "Source",
                        "file_path": info.file_version.as_ref().map(|f| f.file_path.clone()).unwrap_or_default(),
                        "size": info.file_version.as_ref().map(|f| f.file_size).unwrap_or(0),
                        "modified": "",
                        "exists": info.file_version.as_ref().map(|f| f.exists).unwrap_or(false)
                    })];

                    return Ok(Json(json!({
                        "status": "success",
                        "data": {
                            "basic_info": info,
                            "change_log": change_log,
                            "related_files": related_files
                        }
                    })));
                }
            }
            Err(StatusCode::NOT_FOUND)
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn execute_incremental_update(
    State(state): State<AppState>,
    Json(request): Json<IncrementalUpdateRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 委托给真实实现：创建并启动任务
    real_handlers::execute_incremental_update(State(state), Json(request)).await
}

pub async fn check_file_versions(
    State(state): State<AppState>,
    Query(params): Query<DbStatusQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 委托到真实实现，查询 SurrealDB 的 dbnum_info_table
    real_handlers::check_file_versions(Query(params), State(state)).await
}

// 设置/取消自动更新选项
#[derive(serde::Deserialize)]
pub struct AutoUpdateRequest {
    pub auto_update: bool,
}

pub async fn set_auto_update(
    Path(dbnum): Path<u32>,
    Json(req): Json<AutoUpdateRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if std::env::var("WEBUI_SUR_WRITE")
        .map(|v| v == "0")
        .unwrap_or(false)
    {
        return Ok(Json(
            json!({"status":"skipped","message":"SurrealDB write disabled by env WEBUI_SUR_WRITE=0"}),
        ));
    }
    let sql = format!(
        "UPDATE dbnum_info_table SET auto_update = {} WHERE dbnum = {}",
        if req.auto_update { "true" } else { "false" },
        dbnum
    );
    match SUL_DB.query(sql).await {
        Ok(_) => Ok(Json(
            json!({"status":"success","dbnum":dbnum,"auto_update":req.auto_update}),
        )),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(serde::Deserialize)]
pub struct AutoUpdateTypeRequest {
    pub auto_update_type: String,
}

pub async fn set_auto_update_type(
    Path(dbnum): Path<u32>,
    Json(req): Json<AutoUpdateTypeRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if std::env::var("WEBUI_SUR_WRITE")
        .map(|v| v == "0")
        .unwrap_or(false)
    {
        return Ok(Json(
            json!({"status":"skipped","message":"SurrealDB write disabled by env WEBUI_SUR_WRITE=0"}),
        ));
    }
    let t = req.auto_update_type;
    if !matches!(t.as_str(), "ParseOnly" | "ParseAndModel" | "Full") {
        return Ok(Json(
            json!({"status":"error","message":"invalid auto_update_type"}),
        ));
    }
    let sql = format!(
        "UPDATE dbnum_info_table SET auto_update_type = '{}' WHERE dbnum = {}",
        t, dbnum
    );
    match SUL_DB.query(sql).await {
        Ok(_) => Ok(Json(
            json!({"status":"success","dbnum":dbnum,"auto_update_type":t}),
        )),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

// === 内部辅助函数（与 handlers.rs 的实现保持一致的最小版本） ===

async fn convert_row_to_status(row: serde_json::Value) -> Option<DbStatusInfo> {
    let dbnum = row["dbnum"].as_u64()? as u32;
    let file_name = row["file_name"].as_str().unwrap_or("").to_string();
    let project = row["project"].as_str().unwrap_or("").to_string();
    let sesno = row["sesno"].as_u64().unwrap_or(0) as u32;

    // 查询解析计数（以 pe 表为准）
    let count = query_count_pe(dbnum).await.unwrap_or(0);
    let parse_status = if count > 0 {
        ParseStatus::Parsed
    } else {
        ParseStatus::NotParsed
    };

    let model_status = check_model_status(dbnum).await;
    let mesh_status = match model_status {
        ModelStatus::Generated => MeshStatus::Generated,
        _ => MeshStatus::NotGenerated,
    };

    // 基于本地 redb 与当前文件 sesno 判断
    let cached_sesno = SESSION_STORE.get_max_sesno_for_dbnum(dbnum).unwrap_or(0);
    let latest_file_sesno = get_latest_sesno_from_file(&project, dbnum).unwrap_or(sesno);
    let needs_update = cached_sesno < latest_file_sesno;

    let file_version = get_file_version_info(&file_name, &project).await;

    // 可选字段（保存在 dbnum_info_table 记录上）
    let auto_update = row["auto_update"].as_bool().unwrap_or(false);
    let updating = row["updating"].as_bool().unwrap_or(false);
    let last_update_at = None; // 目前从文件读取，服务端可以直接序列化 SystemTime
    let last_update_result = row["last_update_result"].as_str().map(|s| s.to_string());

    Some(DbStatusInfo {
        dbnum,
        file_name,
        db_type: String::new(),
        project,
        count: count as u64,
        sesno,
        max_ref1: 0,
        updated_at: SystemTime::now(),
        parse_status,
        model_status,
        mesh_status,
        file_version,
        needs_update,
        cached_sesno: Some(cached_sesno),
        latest_file_sesno: Some(latest_file_sesno),
        auto_update_type: row["auto_update_type"].as_str().map(|s| s.to_string()),
        auto_update,
        updating,
        last_update_at,
        last_update_result,
    })
}

async fn query_count_pe(dbnum: u32) -> Option<u64> {
    let sql = format!("SELECT count() FROM pe WHERE dbnum = {}", dbnum);
    if let Ok(mut response) = SUL_DB.query(sql).await {
        let counts: Vec<u64> = response.take(0).ok()?;
        return counts.into_iter().next();
    }
    None
}

async fn check_model_status(dbnum: u32) -> ModelStatus {
    let sql = format!("SELECT count() FROM inst_geo WHERE dbnum = {}", dbnum);
    match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let counts: Vec<u64> = response.take(0).unwrap_or_default();
            if counts.first().copied().unwrap_or(0) > 0 {
                ModelStatus::Generated
            } else {
                ModelStatus::NotGenerated
            }
        }
        Err(_) => ModelStatus::NotGenerated,
    }
}

async fn get_file_version_info(file_name: &str, _project: &str) -> Option<FileVersionInfo> {
    if file_name.is_empty() {
        return None;
    }
    let file_path = format!("/data/{}", file_name);
    if let Ok(metadata) = std::fs::metadata(&file_path) {
        Some(FileVersionInfo {
            file_path: file_path.clone(),
            file_version: 0,
            file_size: metadata.len(),
            file_modified: metadata.modified().unwrap_or(SystemTime::now()),
            exists: true,
        })
    } else {
        Some(FileVersionInfo {
            file_path,
            file_version: 0,
            file_size: 0,
            file_modified: SystemTime::now(),
            exists: false,
        })
    }
}

fn get_latest_sesno_from_file(project: &str, dbnum: u32) -> Option<u32> {
    use pdms_io::io::PdmsIO;
    use std::path::Path;

    // 获取项目路径
    let db_option = aios_core::get_db_option();
    let project_path = db_option.get_project_path(project)?;

    // 检查路径是否存在
    if !Path::new(&project_path).exists() {
        eprintln!("项目路径不存在: {}", project_path.display());
        return None;
    }

    // 创建 PdmsIO 实例 - PdmsIO::new 需要三个参数
    let mut pdms_io = PdmsIO::new(project.to_string(), project_path, true);

    // 获取最新 sesno (注意: 这个方法可能不是针对特定 dbnum 的)
    // TODO: 需要确认是否有获取特定 dbnum sesno 的方法
    match pdms_io.get_latest_sesno() {
        Ok(sesno) => Some(sesno),
        Err(e) => {
            eprintln!("获取 sesno 失败 (dbnum: {}): {}", dbnum, e);
            None
        }
    }
}

// ==== 本地扫描与同步接口 ====

#[derive(serde::Serialize)]
pub struct LocalScanItem {
    pub dbnum: u32,
    pub project: String,
    pub surreal_sesno: u32,
    pub file_sesno: u32,
    pub cached_sesno: u32,
    pub exists_in_surreal: bool,
    pub needs_update: bool,
}

#[derive(serde::Serialize)]
pub struct LocalScanResult {
    pub items: Vec<LocalScanItem>,
    pub total: usize,
    pub needs_update_count: usize,
}

// 扫描本地 E3D/PDMS 文件，比较 SurrealDB 与本地/缓存 sesno
pub async fn scan_local_files() -> Result<Json<serde_json::Value>, StatusCode> {
    // 查询 SurrealDB 中的 db 列表
    let sql = "SELECT dbnum, project, sesno FROM dbnum_info_table ORDER BY dbnum";
    let (rows,): (Vec<serde_json::Value>,) = match SUL_DB.query(sql).await {
        Ok(mut resp) => (resp.take(0).unwrap_or_default(),),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut items: Vec<LocalScanItem> = Vec::new();
    for row in rows.into_iter() {
        let dbnum = row["dbnum"].as_u64().unwrap_or(0) as u32;
        let project = row["project"].as_str().unwrap_or("").to_string();
        let surreal_sesno = row["sesno"].as_u64().unwrap_or(0) as u32;
        let cached_sesno = SESSION_STORE.get_max_sesno_for_dbnum(dbnum).unwrap_or(0);
        let file_sesno = get_latest_sesno_from_file(&project, dbnum).unwrap_or(0);
        let needs_update = cached_sesno < file_sesno;

        items.push(LocalScanItem {
            dbnum,
            project,
            surreal_sesno,
            file_sesno,
            cached_sesno,
            exists_in_surreal: true,
            needs_update,
        });
    }

    let total = items.len();
    let needs_update_count = items.iter().filter(|x| x.needs_update).count();

    Ok(Json(json!({
        "status": "success",
        "data": LocalScanResult { items, total, needs_update_count }
    })))
}

#[derive(serde::Deserialize)]
pub struct SyncFileMetadataRequest {
    pub dbnums: Option<Vec<u32>>,
}

// 将本地扫描到的 file_sesno 写入 SurrealDB（持久化文件侧版本），便于后续对比
pub async fn sync_file_metadata(
    Json(req): Json<SyncFileMetadataRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 查询列表
    let sql = "SELECT dbnum, project FROM dbnum_info_table ORDER BY dbnum";
    let (rows,): (Vec<serde_json::Value>,) = match SUL_DB.query(sql).await {
        Ok(mut resp) => (resp.take(0).unwrap_or_default(),),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut updates = String::new();
    for row in rows.into_iter() {
        let dbnum = row["dbnum"].as_u64().unwrap_or(0) as u32;
        if let Some(ref only) = req.dbnums {
            if !only.contains(&dbnum) {
                continue;
            }
        }
        let project = row["project"].as_str().unwrap_or("");
        if let Some(file_sesno) = get_latest_sesno_from_file(project, dbnum) {
            updates.push_str(&format!(
                "UPDATE dbnum_info_table SET file_sesno = {} WHERE dbnum = {};",
                file_sesno, dbnum
            ));
        }
    }

    if !updates.is_empty() {
        let _ = SUL_DB.query(updates).await;
    }

    Ok(Json(json!({"status":"success"})))
}

// 扫描并写入到本地 redb（SESSION_STORE），作为本地缓存基线
pub async fn rescan_and_cache(
    Json(req): Json<SyncFileMetadataRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let sql = "SELECT dbnum, project FROM dbnum_info_table ORDER BY dbnum";
    let (rows,): (Vec<serde_json::Value>,) = match SUL_DB.query(sql).await {
        Ok(mut resp) => (resp.take(0).unwrap_or_default(),),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut updated = 0usize;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    for row in rows.into_iter() {
        let dbnum = row["dbnum"].as_u64().unwrap_or(0) as u32;
        if let Some(ref only) = req.dbnums {
            if !only.contains(&dbnum) {
                continue;
            }
        }
        let project = row["project"].as_str().unwrap_or("");
        if let Some(file_sesno) = get_latest_sesno_from_file(project, dbnum) {
            if SESSION_STORE
                .put_sesno_time_mapping(dbnum, file_sesno, now_secs)
                .is_ok()
            {
                updated += 1;
            }
        }
    }

    Ok(Json(json!({"status":"success", "updated": updated})))
}
