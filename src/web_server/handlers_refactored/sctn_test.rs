// SCTN 测试模块
//
// 负责处理 SCTN（空间接触测试）相关的 HTTP 请求和后台任务执行

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, Json},
};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[cfg(feature = "sqlite-index")]
use crate::fast_model::spatial_index::SqliteSpatialIndex;
#[cfg(feature = "sqlite-index")]
use crate::fast_model::spatial_index::types::{Aabb, Vector3};

use crate::web_server::AppState;

// ================= 全局状态 =================

/// SCTN 测试结果全局存储
static SCTN_TEST_RESULTS: Lazy<DashMap<String, serde_json::Value>> = Lazy::new(DashMap::new);

// ================= 数据结构定义 =================

/// SCTN 测试请求
#[derive(Debug, Deserialize)]
pub struct SctnTestRequest {
    pub target_refno: String,
    #[serde(default)]
    pub radius: Option<f32>,
    #[serde(default)]
    pub tolerance: Option<f32>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// SCTN 测试快照
#[derive(Debug, Serialize)]
struct SctnTestSnapshot {
    target_refno: String,
    target_bbox: Option<serde_json::Value>,
    neighbors: usize,
    contacts: usize,
    proximities: usize,
    supports: usize,
    sample_supports: Vec<serde_json::Value>,
}

// ================= API 处理器 =================

/// SCTN 测试页面
pub async fn sctn_test_page() -> Html<String> {
    let html = r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8" />
  <title>SCTN 测试流程</title>
  <style>
    body { font-family: -apple-system, BlinkMacSystemFont, Segoe UI, Helvetica, Arial, sans-serif; margin: 20px; }
    label { display:block; margin-top:10px; }
    input { padding:6px 8px; margin-top:4px; }
    button { margin-top: 14px; padding: 8px 14px; background:#2563eb; color:#fff; border:none; border-radius:4px; cursor:pointer; }
    pre { background:#0b1021; color:#d1e7ff; padding:10px; border-radius:8px; overflow:auto; }
    .row { display:flex; gap:20px; align-items:flex-start; }
    .card { border:1px solid #e5e7eb; border-radius:8px; padding:16px; }
  </style>
  <script>
    let currentTaskId = null; let timer = null;
    async function runTest(){
      const payload = {
        target_refno: document.getElementById('refno').value.trim(),
        radius: parseFloat(document.getElementById('radius').value||'2.0'),
        tolerance: parseFloat(document.getElementById('tol').value||'0.10'),
        limit: parseInt(document.getElementById('limit').value||'200')
      };
      const r = await fetch('/api/sctn-test/run', { method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify(payload)});
      const js = await r.json();
      if (js.status !== 'success') { alert(js.message||'启动失败'); return; }
      currentTaskId = js.task_id; document.getElementById('taskid').innerText = currentTaskId;
      if (timer) clearInterval(timer);
      timer = setInterval(refresh, 1500);
      await refresh();
    }
    async function refresh(){
      if (!currentTaskId) return;
      const r1 = await fetch('/api/tasks/'+currentTaskId);
      const task = r1.ok ? await r1.json() : null;
      document.getElementById('task').textContent = JSON.stringify(task, null, 2);
      const r2 = await fetch('/api/sctn-test/result/'+currentTaskId);
      const res = r2.ok ? await r2.json() : null;
      document.getElementById('result').textContent = JSON.stringify(res, null, 2);
    }
  </script>
</head>
<body>
  <h2>SCTN 测试流程（后台任务 + 进度）</h2>
  <div class="card" style="max-width:780px;">
    <label>目标SCTN RefNo<br/><input id="refno" value="24383/86525" style="width:340px"/></label>
    <div class="row">
      <label>半径(m)<br/><input id="radius" value="2.0" style="width:120px"/></label>
      <label>容差(m)<br/><input id="tol" value="0.10" style="width:120px"/></label>
      <label>上限<br/><input id="limit" value="200" style="width:120px"/></label>
    </div>
    <button onclick="runTest()">启动测试</button>
    <div style="margin-top:8px">任务ID: <span id="taskid"></span></div>
    <div class="row" style="margin-top:14px">
      <div style="flex:1">
        <h4>任务进度</h4>
        <pre id="task"></pre>
      </div>
      <div style="flex:1">
        <h4>阶段结果</h4>
        <pre id="result"></pre>
      </div>
    </div>
  </div>
</body>
</html>
"#;
    Html(html.to_string())
}

/// 启动后台测试任务
pub async fn api_sctn_test_run(
    State(state): State<AppState>,
    Json(req): Json<SctnTestRequest>,
) -> Json<serde_json::Value> {
    // 创建任务
    let task_name = format!("SCTN测试: {}", &req.target_refno);
    let mut cfg = crate::web_server::models::DatabaseConfig::default();
    cfg.manual_db_nums = vec![];
    let mut tm = state.task_manager.lock().await;
    let task = crate::web_server::models::TaskInfo::new(
        task_name,
        crate::web_server::models::TaskType::Custom("SctnTest".into()),
        cfg,
    );
    let task_id = task.id.clone();
    tm.active_tasks.insert(task_id.clone(), task.clone());
    drop(tm);

    // 启动执行
    tokio::spawn(run_sctn_test_pipeline(state.clone(), task_id.clone(), req));
    Json(json!({"status":"success","task_id": task_id}))
}

/// 获取当前阶段结果
pub async fn api_sctn_test_result(Path(id): Path<String>) -> Json<serde_json::Value> {
    if let Some(v) = SCTN_TEST_RESULTS.get(&id) {
        return Json(v.clone());
    }
    Json(json!({"status":"pending","message":"尚无结果或任务不存在"}))
}

// ================= 内部辅助函数 =================

/// 执行 SCTN 测试流程（后台任务）
async fn run_sctn_test_pipeline(state: AppState, task_id: String, req: SctnTestRequest) {
    // 工具函数：更新任务进度
    let update = |msg: &str, step: u32, total: u32, pct: f32| {
        let st = state.clone();
        let id = task_id.clone();
        let m = msg.to_string();
        tokio::spawn(async move {
            let mut tm = st.task_manager.lock().await;
            if let Some(task) = tm.active_tasks.get_mut(&id) {
                if task.status != crate::web_server::models::TaskStatus::Cancelled {
                    task.update_progress(m, step, total, pct);
                }
            }
        });
    };

    // 仅使用 SQLite 索引，分 4 步：读取目标 -> 邻域检索 -> 接触检测 -> 支撑检测
    let total = 4u32;
    let mut step = 0u32;

    // 初始化快照
    let mut snap = SctnTestSnapshot {
        target_refno: req.target_refno.clone(),
        target_bbox: None,
        neighbors: 0,
        contacts: 0,
        proximities: 0,
        supports: 0,
        sample_supports: vec![],
    };

    // Step1: 读取目标
    step += 1;
    update(
        "读取目标AABB",
        step,
        total,
        100.0 * step as f32 / total as f32,
    );
    #[cfg(feature = "sqlite-index")]
    let index = match SqliteSpatialIndex::with_default_path() {
        Ok(v) => v,
        Err(e) => {
            finish_fail(state, task_id, format!("打开索引失败: {}", e)).await;
            return;
        }
    };
    #[cfg(not(feature = "sqlite-index"))]
    {
        finish_fail(state, task_id, "未启用sqlite-index".into()).await;
        return;
    }

    #[cfg(feature = "sqlite-index")]
    {
        let refno = match aios_core::pdms_types::RefU64::from_str(&req.target_refno) {
            Ok(v) => v,
            Err(_) => {
                finish_fail(state, task_id, "无效RefNo格式".into()).await;
                return;
            }
        };
        let tb = match index.get_aabb(refno) {
            Ok(Some(b)) => b,
            Ok(None) => {
                finish_fail(state, task_id, "索引中未找到目标SCTN".into()).await;
                return;
            }
            Err(e) => {
                finish_fail(state, task_id, format!("查询目标失败: {}", e)).await;
                return;
            }
        };
        snap.target_bbox = Some(
            json!({"mins":[tb.mins.x,tb.mins.y,tb.mins.z], "maxs":[tb.maxs.x,tb.maxs.y,tb.maxs.z]}),
        );
        SCTN_TEST_RESULTS.insert(
            task_id.clone(),
            json!({"status":"running","snapshot": snap}),
        );

        // Step2: 邻域检索
        step += 1;
        update("邻域检索", step, total, 100.0 * step as f32 / total as f32);
        let radius = req.radius.unwrap_or(2.0);
        let query = Aabb::new(
            tb.mins - Vector3::new(radius, radius, radius),
            tb.maxs + Vector3::new(radius, radius, radius),
        );
        let mut neigh = match index.query_intersect(&query) {
            Ok(v) => v,
            Err(e) => {
                finish_fail(state, task_id, format!("邻域查询失败: {}", e)).await;
                return;
            }
        };
        neigh.retain(|r| *r != refno);
        if let Some(lm) = req.limit {
            if neigh.len() > lm {
                neigh.truncate(lm);
            }
        }
        snap.neighbors = neigh.len();
        SCTN_TEST_RESULTS.insert(
            task_id.clone(),
            json!({"status":"running","snapshot": snap}),
        );

        // 读取 items 中 noun
        let mut noun_map = std::collections::HashMap::<u64, String>::new();
        if !neigh.is_empty() {
            if let Ok(conn) = rusqlite::Connection::open(SqliteSpatialIndex::default_path()) {
                let ids = neigh
                    .iter()
                    .map(|r| (r.0 as i64).to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!("SELECT id, noun FROM items WHERE id IN ({})", ids);
                if let Ok(mut stmt) = conn.prepare(&sql) {
                    if let Ok(rows) = stmt.query_map([], |row| {
                        let id: i64 = row.get(0)?;
                        let noun: String = row.get(1)?;
                        Ok((id as u64, noun))
                    }) {
                        for r in rows {
                            if let Ok((id, n)) = r {
                                noun_map.insert(id, n);
                            }
                        }
                    }
                }
            }
        }

        // Step3: 接触检测（Cuboid逼近）
        step += 1;
        update("接触检测", step, total, 100.0 * step as f32 / total as f32);
        let tol = req.tolerance.unwrap_or(0.10);
        use nalgebra::Isometry3;
        use parry3d::query::contact;
        use parry3d::shape::Cuboid;
        let ext_t = (tb.maxs - tb.mins) * 0.5;
        let c_t = tb.center();
        let shape_t = Cuboid::new(Vector3::new(
            ext_t.x.max(1e-6),
            ext_t.y.max(1e-6),
            ext_t.z.max(1e-6),
        ));
        let iso_t = Isometry3::translation(c_t.x, c_t.y, c_t.z);
        let mut contacts = 0usize;
        let mut proximities = 0usize;
        for r in &neigh {
            if let Ok(Some(b)) = index.get_aabb(*r) {
                let ext = (b.maxs - b.mins) * 0.5;
                let c = b.center();
                let shape = Cuboid::new(Vector3::new(
                    ext.x.max(1e-6),
                    ext.y.max(1e-6),
                    ext.z.max(1e-6),
                ));
                let iso = Isometry3::translation(c.x, c.y, c.z);
                if let Ok(Some(ct)) = contact(&iso_t, &shape_t, &iso, &shape, tol) {
                    if ct.dist < -tol || ct.dist.abs() < 1e-3 {
                        contacts += 1;
                    } else if ct.dist < tol {
                        proximities += 1;
                    }
                }
            }
        }
        snap.contacts = contacts;
        snap.proximities = proximities;
        SCTN_TEST_RESULTS.insert(
            task_id.clone(),
            json!({"status":"running","snapshot": snap}),
        );

        // Step4: 支撑检测（顶面对齐 + 水平重叠）
        step += 1;
        update("支撑检测", step, total, 100.0 * step as f32 / total as f32);
        let mut supports = Vec::<serde_json::Value>::new();
        for r in neigh {
            if let Ok(Some(b)) = index.get_aabb(r) {
                let vg = (tb.mins.y - b.maxs.y).abs();
                let xo = tb.maxs.x > b.mins.x && tb.mins.x < b.maxs.x;
                let zo = tb.maxs.z > b.mins.z && tb.mins.z < b.maxs.z;
                if vg <= tol && xo && zo {
                    let cc = b.center();
                    supports.push(json!({"refno": r.0, "noun": noun_map.get(&r.0).cloned().unwrap_or_default(), "cx":cc.x, "cy":cc.y, "cz":cc.z, "max_y": b.maxs.y}));
                }
            }
        }
        snap.supports = supports.len();
        snap.sample_supports = supports.iter().take(10).cloned().collect();
        SCTN_TEST_RESULTS.insert(
            task_id.clone(),
            json!({"status":"completed","snapshot": snap, "supports": supports}),
        );

        // 完成任务
        let mut tm = state.task_manager.lock().await;
        if let Some(task) = tm.active_tasks.get_mut(&task_id) {
            task.status = crate::web_server::models::TaskStatus::Completed;
            task.progress.percentage = 100.0;
            task.progress.current_step = "完成".into();
            task.completed_at = Some(std::time::SystemTime::now());
        }
    }
}

/// 标记任务失败
async fn finish_fail(state: AppState, task_id: String, msg: String) {
    SCTN_TEST_RESULTS.insert(task_id.clone(), json!({"status":"failed","message": msg}));
    let mut tm = state.task_manager.lock().await;
    if let Some(task) = tm.active_tasks.get_mut(&task_id) {
        task.status = crate::web_server::models::TaskStatus::Failed;
        task.error = Some(msg);
        task.completed_at = Some(std::time::SystemTime::now());
    }
}
