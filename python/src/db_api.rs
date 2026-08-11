//! 连接层（`aios_db.db`）：只读观察面，直查 SurrealDB 或调用现成只读函数。
//!
//! 刻意**不**走 `query_service::QueryService` 的 `e3d.element.*` 工具——那条路
//! 背后是 E3D TTY 驱动（拉起真实 E3D 进程跑 PML），是另一套环境依赖；这里的
//! 元素查询全部直查库内 `pe` / `inst_relate`。
//!
//! `dbnum_statuses` / `preview_manual_update` 构造进程级 `AiosDBManager`（目录
//! 扫描 + 解析簿记，与服务端预览端点同源），会写观察性簿记字段，但不碰模型 /
//! 增量数据。

use std::sync::Arc;

use pyo3::prelude::*;
use serde_json::json;
use tokio::sync::OnceCell;

use aios_core::SUL_DB;
use aios_database::data_interface::tidb_manager::AiosDBManager;

use crate::{anyhow_to_py, ensure_connected, pythonized, runtime};

static MANAGER: OnceCell<Arc<AiosDBManager>> = OnceCell::const_new();

/// 进程级唯一的 manager（首次调用构造：监控目录解析 + 头扫描，之后复用）。
pub(crate) async fn manager() -> anyhow::Result<&'static Arc<AiosDBManager>> {
    MANAGER
        .get_or_try_init(|| async { Ok(Arc::new(AiosDBManager::init_form_config().await?)) })
        .await
}

/// refno 输入宽容两种形态：`a/b`（web 形态）与 `a_b`（record id 形态）。
pub(crate) fn normalize_refno(refno: &str) -> String {
    refno.trim().trim_start_matches('=').replace('/', "_")
}

/// 跑一条只读 SQL，取第 0 条语句的干净 JSON。
pub(crate) async fn take_json(
    sql: String,
    binds: Vec<(&'static str, serde_json::Value)>,
) -> anyhow::Result<serde_json::Value> {
    let mut request = SUL_DB.query(sql);
    for (key, value) in binds {
        request = request.bind((key, value));
    }
    let mut response = request
        .await?
        .check()
        .map_err(|error| anyhow::anyhow!("查询失败: {error}"))?;
    let value: surrealdb::Value = response
        .take(0usize)
        .map_err(|error| anyhow::anyhow!("解码查询结果失败: {error}"))?;
    Ok(value.into_inner().into_json())
}

/// 名字精确匹配的元素 refno 列表（`dbnum` 限定库；名字只保证库内唯一，跨库同名常见）。
#[pyfunction]
#[pyo3(signature = (name, dbnum=None))]
pub fn by_name(py: Python<'_>, name: String, dbnum: Option<u32>) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let value = py
        .detach(|| {
            runtime().block_on(take_json(
                format!(
                    "SELECT VALUE record::id(id) FROM pe WHERE name = $name AND deleted = false{};",
                    dbnum
                        .map(|dbnum| format!(" AND dbnum = {dbnum}"))
                        .unwrap_or_default()
                ),
                vec![("name", json!(name))],
            ))
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 名为 `parent` 的元素下、类型为 `noun` 的子元素 refno 列表。
///
/// 几何图元（CONE / PANE / CAP…）在 PDMS 里没有自己的名字，「有名父 + noun」
/// 是它们最稳定的定位方式（与 tests/common 的先例同款）。`parent` 不唯一时报错。
#[pyfunction]
#[pyo3(signature = (parent, noun, dbnum=None))]
pub fn child_of(
    py: Python<'_>,
    parent: String,
    noun: String,
    dbnum: Option<u32>,
) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let value = py
        .detach(|| {
            runtime().block_on(async {
                let hits = take_json(
                    format!(
                        "SELECT VALUE record::id(id) FROM pe WHERE name = $name AND deleted = false{};",
                        dbnum
                            .map(|dbnum| format!(" AND dbnum = {dbnum}"))
                            .unwrap_or_default()
                    ),
                    vec![("name", json!(parent))],
                )
                .await?;
                let parents = hits.as_array().cloned().unwrap_or_default();
                if parents.len() != 1 {
                    anyhow::bail!("父元素 {parent} 匹配到 {} 个，须唯一", parents.len());
                }
                let owner = parents[0].as_str().unwrap_or_default().to_string();
                take_json(
                    format!(
                        "SELECT VALUE record::id(id) FROM pe \
                         WHERE owner = type::thing('pe', $owner) AND noun = $noun AND deleted = false{};",
                        dbnum
                            .map(|dbnum| format!(" AND dbnum = {dbnum}"))
                            .unwrap_or_default()
                    ),
                    vec![("owner", json!(owner)), ("noun", json!(noun))],
                )
                .await
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 一个元素的 `pe` 行（不存在返回 None）。
#[pyfunction]
pub fn pe(py: Python<'_>, refno: String) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let refno = normalize_refno(&refno);
    let value = py
        .detach(|| {
            runtime().block_on(take_json(
                "SELECT * FROM type::thing('pe', $refno);".into(),
                vec![("refno", json!(refno))],
            ))
        })
        .map_err(anyhow_to_py)?;
    let first = value
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    pythonized(py, &first)
}

/// 一个元素的直接成员（`owner` 反查，未删除的）。
#[pyfunction]
pub fn members(py: Python<'_>, refno: String) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let refno = normalize_refno(&refno);
    let value = py
        .detach(|| {
            runtime().block_on(take_json(
                "SELECT record::id(id) AS refno, noun, name, sesno \
                 FROM pe WHERE owner = type::thing('pe', $refno) AND deleted = false;"
                    .into(),
                vec![("refno", json!(refno))],
            ))
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 从元素沿 `owner` 一路向上到 WORL 的链（含自己；最多 64 跳防环）。
#[pyfunction]
pub fn owner_chain(py: Python<'_>, refno: String) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let refno = normalize_refno(&refno);
    let value = py
        .detach(|| {
            runtime().block_on(async {
                let mut chain = Vec::new();
                let mut current = refno.clone();
                for _ in 0..64 {
                    let rows = take_json(
                        "SELECT record::id(id) AS refno, noun, name, \
                         record::id(owner) AS owner FROM type::thing('pe', $refno);"
                            .into(),
                        vec![("refno", json!(current))],
                    )
                    .await?;
                    let Some(row) = rows.as_array().and_then(|rows| rows.first()).cloned() else {
                        break;
                    };
                    let noun = row["noun"].as_str().unwrap_or_default().to_string();
                    let owner = row["owner"].as_str().map(str::to_string);
                    chain.push(row);
                    if noun.eq_ignore_ascii_case("WORL") || noun.eq_ignore_ascii_case("WORLD") {
                        break;
                    }
                    match owner {
                        Some(owner) if !owner.is_empty() && owner != current => current = owner,
                        _ => break,
                    }
                }
                if chain.is_empty() {
                    anyhow::bail!("元素 {refno} 不存在");
                }
                anyhow::Ok(serde_json::Value::Array(chain))
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 一个元素**及其子树**的几何实例边（`inst_relate`），FETCH 展开 aabb 与 world_trans。
///
/// 实例边挂在具体图元上；交付单元根（EQUI/BRAN…）自身通常没有直接实例，靠
/// `anc`（祖先链的 RefU64 u64 数组）把整棵子树的实例收进来。
#[pyfunction]
pub fn inst(py: Python<'_>, refno: String) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let refno = normalize_refno(&refno);
    let refno_u64 = {
        use std::str::FromStr;
        aios_core::RefU64::from_str(&refno)
            .map(|refno| ((refno.get_0() as u64) << 32) | refno.get_1() as u64)
            .unwrap_or_default()
    };
    let value = py
        .detach(|| {
            runtime().block_on(take_json(
                "SELECT * FROM inst_relate \
                 WHERE in = type::thing('pe', $refno) OR anc CONTAINS $refno_u64 \
                 FETCH aabb, world_trans;"
                    .into(),
                vec![("refno", json!(refno)), ("refno_u64", json!(refno_u64))],
            ))
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 一个库的权威应用水位（未登记为 0）。
#[pyfunction]
pub fn watermark(py: Python<'_>, dbnum: u32) -> PyResult<u32> {
    ensure_connected()?;
    py.detach(|| {
        runtime().block_on(
            aios_database::data_interface::sesno_range::SesnoRangeResolver::query_watermark(dbnum),
        )
    })
    .map_err(anyhow_to_py)
}

/// 水位状态 + 阻断/排除（`GET /dbnums` 同源，`DbnumStatusReport` 原样）。
#[pyfunction]
#[pyo3(signature = (project=None, mdb=None))]
pub fn dbnum_statuses(
    py: Python<'_>,
    project: Option<String>,
    mdb: Option<String>,
) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let report = py
        .detach(|| {
            runtime().block_on(async {
                let mgr = manager().await?;
                let project = project.unwrap_or_else(|| mgr.db_option.project_name.clone());
                mgr.dbnum_statuses(&project, mdb.as_deref()).await
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &report)
}

/// 手动更新只读预览（`POST /update/preview` 同源，`ManualUpdatePreview` 原样）。
///
/// 会刷新扫描观察字段（与服务端预览端点一致），不碰模型/增量数据。
#[pyfunction]
#[pyo3(signature = (project=None, mdb=None))]
pub fn preview_manual_update(
    py: Python<'_>,
    project: Option<String>,
    mdb: Option<String>,
) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let preview = py
        .detach(|| {
            runtime().block_on(async {
                let mgr = manager().await?;
                let project = project.unwrap_or_else(|| mgr.db_option.project_name.clone());
                mgr.preview_manual_update(&project, mdb.as_deref()).await
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &preview)
}

/// 全部模型待重试任务（检查视图，含死信；`GET /update/pending-units` 同源）。
#[pyfunction]
pub fn pending_model_units(py: Python<'_>) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let units = py
        .detach(|| {
            runtime()
                .block_on(aios_database::data_interface::manual_update::load_pending_model_units())
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &units)
}

/// 全部窗口阻断状态（阻断的库为什么不动，唯一出处）。
#[pyfunction]
pub fn window_blocks(py: Python<'_>) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let blocks = py
        .detach(|| {
            runtime()
                .block_on(aios_database::data_interface::staging::attempts::load_window_blocks())
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &blocks)
}

/// 一个库全部生成根的失败记录（root_refno → 记录）。
#[pyfunction]
pub fn root_attempts(py: Python<'_>, dbnum: u32) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let attempts = py
        .detach(|| {
            runtime().block_on(
                aios_database::data_interface::staging::attempts::load_root_attempts(dbnum),
            )
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &attempts)
}

pub fn register(py: Python<'_>, db: &Bound<'_, PyModule>) -> PyResult<()> {
    let _ = py;
    db.add_function(wrap_pyfunction!(by_name, db)?)?;
    db.add_function(wrap_pyfunction!(child_of, db)?)?;
    db.add_function(wrap_pyfunction!(pe, db)?)?;
    db.add_function(wrap_pyfunction!(members, db)?)?;
    db.add_function(wrap_pyfunction!(owner_chain, db)?)?;
    db.add_function(wrap_pyfunction!(inst, db)?)?;
    db.add_function(wrap_pyfunction!(watermark, db)?)?;
    db.add_function(wrap_pyfunction!(dbnum_statuses, db)?)?;
    db.add_function(wrap_pyfunction!(preview_manual_update, db)?)?;
    db.add_function(wrap_pyfunction!(pending_model_units, db)?)?;
    db.add_function(wrap_pyfunction!(window_blocks, db)?)?;
    db.add_function(wrap_pyfunction!(root_attempts, db)?)?;
    Ok(())
}
