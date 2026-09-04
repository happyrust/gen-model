//! aios-database 的 Python 调试绑定（模块名 `aios_db`）。
//!
//! 设计与决策见 `docs/plans/2026-08-11-python-binding-api-plan.md`。M1 范围：
//!
//! - `aios_db.connect(config=None)` —— 连接层（连 `SUL_DB`，**不拿**单实例锁）
//! - `aios_db.db.query(sql, binds=None)` —— SurrealQL 直通
//! - `aios_db.parse.header / is_db_file / sessions / net_window / element` —— 纯文件解析（e3d-io 底座）
//!
//! 异步桥：进程级 tokio 多线程 Runtime + `block_on`，等待期间释放 GIL。
//! 数据形态：serde → dict（pythonize），refno 输出 `a_b`（同 `pe:` record id）。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pythonize::{depythonize, pythonize};

mod convert;
mod db_api;
mod exec_api;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
pub(crate) static CONNECTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("创建 tokio runtime 失败")
    })
}

pub(crate) fn anyhow_to_py(error: anyhow::Error) -> PyErr {
    PyRuntimeError::new_err(format!("{error:#}"))
}

pub(crate) fn ensure_connected() -> PyResult<()> {
    if CONNECTED.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(PyRuntimeError::new_err(
            "尚未连接数据库：先调用 aios_db.connect(config)（config 缺省为当前目录的 DbOption.toml）",
        ))
    }
}

/// serde 值 → Python 对象（统一出口）。
pub(crate) fn pythonized<T: serde::Serialize>(py: Python<'_>, value: &T) -> PyResult<Py<PyAny>> {
    Ok(pythonize(py, value)?.unbind())
}

/// 指定 DbOption 配置文件路径（等价设置环境变量 `DB_OPTION_FILE`，可省略 `.toml` 后缀）。
///
/// 配置是进程级 OnceCell，第一次被读到之后不可更换，所以本调用必须发生在任何
/// 会触碰配置的函数（`connect`、`parse.net_window` 等）之前。解析层看似
/// 纯文件操作，但深处也读全局 DbOption（debug 选项等），配置不可达时会直接 panic。
#[pyfunction]
fn set_config(path: String) -> PyResult<()> {
    unsafe { std::env::set_var("DB_OPTION_FILE", &path) };
    Ok(())
}

/// 连接层初始化：加载 DbOption 配置并连接进程全局 `SUL_DB`（ws），**不拿**单实例锁。
///
/// `config` 同 [`set_config`]；缺省沿用环境变量 `DB_OPTION_FILE`，再缺省为当前
/// 目录的 `DbOption`。`cwd` 指定后先切换进程工作目录——`init_surreal` 内部的
/// `define_common_functions` 按 CWD 相对路径读 `resource/surreal/`，通常应传
/// gen-model 仓库根目录。重复 `connect` 是幂等空操作。
#[pyfunction]
#[pyo3(signature = (config=None, cwd=None))]
fn connect(py: Python<'_>, config: Option<String>, cwd: Option<PathBuf>) -> PyResult<()> {
    if CONNECTED.load(Ordering::SeqCst) {
        return Ok(());
    }
    if let Some(cwd) = cwd {
        std::env::set_current_dir(&cwd).map_err(|error| {
            PyRuntimeError::new_err(format!("切换工作目录到 {} 失败: {error}", cwd.display()))
        })?;
    }
    if let Some(config) = config {
        // aios_core::init_surreal / get_db_option 都按 DB_OPTION_FILE 找配置文件，
        // 必须在第一次读配置之前写入。
        unsafe { std::env::set_var("DB_OPTION_FILE", &config) };
    }
    py.detach(|| {
        runtime().block_on(async {
            aios_core::init_surreal().await?;
            // 与 run_cli 同款（D11/ADR-010）：磁盘脚本之后再灌一遍编译期内置
            // 函数快照——目录序里 hh 排在 hd 之后，不矫正的话连接层的
            // fn::room_code 停在 hh 语义，与服务行为漂移。
            aios_database::data_interface::embedded_surql::define_embedded_functions().await
        })
    })
    .map_err(anyhow_to_py)?;
    CONNECTED.store(true, Ordering::SeqCst);
    Ok(())
}

/// SurrealQL 直通查询：按语句返回结果数组（每条语句一个 JSON 值）。
#[pyfunction]
#[pyo3(signature = (sql, binds=None))]
fn query(py: Python<'_>, sql: String, binds: Option<Bound<'_, PyAny>>) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let binds: Option<serde_json::Map<String, serde_json::Value>> = match binds {
        Some(object) => Some(depythonize(&object)?),
        None => None,
    };
    let results = py
        .detach(|| {
            runtime().block_on(async move {
                let mut request = aios_core::SUL_DB.query(sql);
                if let Some(binds) = binds {
                    for (key, value) in binds {
                        request = request.bind((key, value));
                    }
                }
                let mut response = request.await.context("查询发送失败")?;
                let statements = response.num_statements();
                let mut out = Vec::with_capacity(statements);
                for index in 0..statements {
                    match response.take::<surrealdb::Value>(index) {
                        // into_inner → 核心 sql::Value，into_json 给干净 JSON
                        //（Thing 等类型转简单形态，而不是 SDK 包装的 tagged 枚举）。
                        Ok(value) => out.push(value.into_inner().into_json()),
                        Err(error) => out.push(serde_json::json!({
                            "error": error.to_string(),
                        })),
                    }
                }
                anyhow::Ok(serde_json::Value::Array(out))
            })
        })
        .map_err(anyhow_to_py)?;
    Ok(pythonize(py, &results)?.unbind())
}

/// 读库文件头：60 字节快速头（库类型 / dbnum）+ 最新会话页（会话号 / 时刻 / 文件大小）。
#[pyfunction]
fn header(py: Python<'_>, path: PathBuf) -> PyResult<Py<PyAny>> {
    let value = py.detach(|| header_impl(&path)).map_err(anyhow_to_py)?;
    Ok(pythonize(py, &value)?.unbind())
}

fn header_impl(path: &Path) -> anyhow::Result<serde_json::Value> {
    let info = aios_database::data_interface::db_file_header::DbFileHeader::read(path)?;
    Ok(serde_json::json!({
        "db_type": info.db_type,
        "dbnum": info.dbnum,
        "latest_sesno": info.latest_session.sesno,
        "latest_ses_time": info.latest_session.rfc3339(),
        "latest_ses_pageno": info.latest_ses_pgno,
        "file_size": info.file_size,
    }))
}

/// 这个路径是不是候选 AVEVA 库文件（名字形态 + 文件头两道门）。
#[pyfunction]
fn is_db_file(path: PathBuf) -> bool {
    aios_database::data_interface::increment_manager::is_candidate_db_file(&path)
}

/// 同项目抽取家族归并（ADR-028）：主库 + 唯一 `_NNNN` → 选叶子、父层进 shadowed；
/// 兄弟抽取 / 文件名与头库号不一致 → duplicate。纯函数，不读文件内容。
#[pyfunction]
fn collapse_extract_files(
    py: Python<'_>,
    entries: Vec<(String, u32, PathBuf)>,
) -> PyResult<Py<PyAny>> {
    let result = aios_database::data_interface::extract_family::collapse_extract_families(entries);
    let mut duplicate_keys: Vec<_> = result
        .duplicate_keys
        .into_iter()
        .map(|(project, dbnum)| serde_json::json!([project, dbnum]))
        .collect();
    duplicate_keys.sort_by(|left, right| left.to_string().cmp(&right.to_string()));
    let value = serde_json::json!({
        "selected": result.selected.iter().map(|family| serde_json::json!({
            "project": family.project,
            "dbnum": family.dbnum,
            "leaf_path": family.leaf_path.to_string_lossy(),
            "parent_path": family.parent_path.as_ref().map(|path| path.to_string_lossy().into_owned()),
        })).collect::<Vec<_>>(),
        "shadowed_parents": result.shadowed_parents.iter().map(|path| path.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "duplicate_keys": duplicate_keys,
        "mismatches": result.mismatches.iter().map(|row| serde_json::json!({
            "path": row.path.to_string_lossy(),
            "filename_dbnum": row.filename_dbnum,
            "header_dbnum": row.header_dbnum,
        })).collect::<Vec<_>>(),
    });
    Ok(pythonize(py, &value)?.unbind())
}

/// 父层索引里有、叶子索引里没有的 refno 个数。基线只在 gap>0 时补缺。
#[pyfunction]
fn parent_gap_refno_count(leaf: PathBuf, parent: PathBuf) -> PyResult<usize> {
    aios_database::data_interface::extract_family::parent_gap_refno_count(&leaf, &parent)
        .map_err(anyhow_to_py)
}

/// 列出库文件的全部会话页（升序）：会话号 / 页号 / 索引根 / 机器名 / 注释 / 时刻。
#[pyfunction]
fn sessions(py: Python<'_>, path: PathBuf) -> PyResult<Py<PyAny>> {
    let value = py.detach(|| sessions_impl(&path)).map_err(anyhow_to_py)?;
    Ok(pythonize(py, &value)?.unbind())
}

fn sessions_impl(path: &Path) -> anyhow::Result<serde_json::Value> {
    let sessions = aios_database::data_interface::db_file_header::session_chain(path)?;
    let rows = sessions
        .into_iter()
        .map(|session| {
            serde_json::json!({
                "sesno": session.sesno,
                "pgno": session.pgno,
                "end_pgno": session.end_pgno,
                "index_root_pageno": session.index_root_pgno,
                "claim_pageno": serde_json::Value::Null,
                "computer_name": session.machine,
                "comments": session.comment,
                "date": session.rfc3339(),
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::Value::Array(rows))
}

/// 从库文件直读单个元素在某个会话时点的持久属性（不进库、不需要连接）。
///
/// 与生产收集器同一条读法（`FrozenSource` 身份门 → e3d-io 钉住会话的 `DbSet` →
/// `to_named_attmap`，`e3d_collector::read_element_at`）。`sesno` 缺省读文件最新会话；
/// 给定时把文件钉在该会话（做历史对比 / TTY 往返前后读数）。元素在该会话索引里不存在
/// 时抛 `RuntimeError`（「最新索引里没有」不等于「从未存在」，读历史版本传 `sesno`）。
///
/// 返回 `{refno, sesno, noun_hash, noun, name, owner, children, attrs, explicit_attrs}`；
/// `attrs` / `explicit_attrs` 是 `NamedAttrMap` 的 serde 形态（值带类型标签）。
/// 这是 old-pdms-io `parse.element`（`PdmsIO::parse_raw_element`）的 e3d-io 替身，2026-09-04 起。
#[pyfunction]
#[pyo3(signature = (path, refno, sesno=None))]
fn element(
    py: Python<'_>,
    path: PathBuf,
    refno: String,
    sesno: Option<u32>,
) -> PyResult<Py<PyAny>> {
    let value = py
        .detach(|| element_impl(&path, &refno, sesno))
        .map_err(anyhow_to_py)?;
    Ok(pythonize(py, &value)?.unbind())
}

fn element_impl(path: &Path, refno: &str, sesno: Option<u32>) -> anyhow::Result<serde_json::Value> {
    use std::str::FromStr;
    let parsed = aios_core::RefU64::from_str(refno.trim())
        .map_err(|_| anyhow::anyhow!("refno 形态不认识: {refno}（要 a_b / a/b / pe:a_b）"))?;
    let frozen = match sesno {
        Some(sesno) => aios_database::data_interface::frozen_source::FrozenSource::capture_at(path, sesno),
        None => aios_database::data_interface::frozen_source::FrozenSource::capture(path),
    }
    .map_err(|error| anyhow::anyhow!("冻结 dabacon 源失败: {error}"))?;
    let data = aios_database::data_interface::e3d_collector::read_element_at(&frozen, parsed)?;
    Ok(convert::ele_data_to_json(&data, frozen.target_sesno()))
}

/// **正式口径**（ADR-031）：解析器语义净窗口。会话索引差分先圈出两端记录位置，
/// 再在文件内解析 base / 终稿并做一次属性 diff。与增量管线 / 手动预览共用同一
/// 实现（`IncrementPipeline::collect_window` → `collect_net_window`）。
///
/// 与只看索引触达三态的读法不同，本入口会过滤“换页但内容相同”的原样重写。
/// E3D 自己推进的显式元数据（例如 BRAN.CACHID）仍会如实返回。
///
/// 这是 `aios_db.parse` 里**唯一**的窗口入口：old-pdms-io 底座的 `collect_changes`（逐会话回放）
/// 与 `net_changes`（老索引差分）2026-09-04 已删。判它对不对的是 E3D TTY 录下的会话
/// （`scripts/e3d/Test-TtyNetWindow.ps1`、`tests/fixtures/issues/issue-019-*`），不是另一套解析器。
///
/// 返回 `{requested_start/end, window: {sesno: [op, ...]}, counts,
/// warnings, unchanged_rewrites, unparseable_finals}`。`detail=True` 时 Modified 携带
/// 完整属性旧值/新值。全程只读 dabacon 文件，不连接 SurrealDB。
#[pyfunction]
#[pyo3(signature = (path, start, end, detail=false))]
fn net_window(
    py: Python<'_>,
    path: PathBuf,
    start: i32,
    end: i32,
    detail: bool,
) -> PyResult<Py<PyAny>> {
    let value = py
        .detach(|| {
            // 冻结点取窗口终点，与生产收集口径一致
            // （`IncrementPipeline::collect_window_for_candidate` 同样 `open_at(target)`）。
            // 冻在「文件当前最新会话」的话，库一长出新会话，历史窗口就再也复算不出来
            // ——净窗口对冻结点有硬校验，事后取证与回归夹具首当其冲。
            let target =
                u32::try_from(end).map_err(|_| anyhow::anyhow!("窗口终点会话号非法: {end}"))?;
            // 与生产同一份收集器（e3d-io 底座）：冻结源钉在窗口终点，再做两端索引差分。
            let frozen = aios_database::data_interface::frozen_source::FrozenSource::capture_at(
                &path, target,
            )
            .map_err(|error| anyhow::anyhow!("冻结 dabacon 源失败: {error}"))?;
            let outcome = aios_database::data_interface::e3d_collector::collect_net_window(
                &frozen,
                start..=end,
                frozen.db_type(),
            )?;
            let mut counts = serde_json::Map::from_iter([
                ("added".into(), serde_json::json!(0usize)),
                ("deleted".into(), serde_json::json!(0usize)),
                ("modified".into(), serde_json::json!(0usize)),
            ]);
            for operation in outcome.window.values().flatten() {
                use aios_database::data_interface::ele_ops::EleOperationDetail;
                let key = match &operation.detail {
                    EleOperationDetail::Add(_) => "added",
                    EleOperationDetail::Deleted => "deleted",
                    EleOperationDetail::Modified(_) => "modified",
                    EleOperationDetail::None => continue,
                };
                let count = counts[key].as_u64().unwrap_or_default() + 1;
                counts.insert(key.into(), serde_json::json!(count));
            }
            anyhow::Ok(serde_json::json!({
                "requested_start": start,
                "requested_end": end,
                "window": convert::window_to_json(&outcome.window, detail),
                "counts": counts,
                "warnings": outcome.warnings,
                "unchanged_rewrites": outcome.unchanged_rewrites,
                "unparseable_finals": outcome.unparseable_finals,
            }))
        })
        .map_err(anyhow_to_py)?;
    Ok(pythonize(py, &value)?.unbind())
}

#[pymodule]
fn _aios_db(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(set_config, m)?)?;
    m.add_function(wrap_pyfunction!(connect, m)?)?;

    let parse = PyModule::new(py, "parse")?;
    parse.add_function(wrap_pyfunction!(header, &parse)?)?;
    parse.add_function(wrap_pyfunction!(is_db_file, &parse)?)?;
    parse.add_function(wrap_pyfunction!(collapse_extract_files, &parse)?)?;
    parse.add_function(wrap_pyfunction!(parent_gap_refno_count, &parse)?)?;
    parse.add_function(wrap_pyfunction!(sessions, &parse)?)?;
    parse.add_function(wrap_pyfunction!(net_window, &parse)?)?;
    parse.add_function(wrap_pyfunction!(element, &parse)?)?;
    // old-pdms-io 底座的绑定（`collect_changes` / `net_changes` / `attmap` / `subtree` /
    // `generate_*` / `noun_dict`）2026-09-04 随 old-pdms-io 一起删了。窗口语义只剩 `net_window`
    // 一个入口（生产收集器 `e3d_collector`）；`element` 换成同一底座的单元素直读；子树 /
    // 合并视图走 web-service 的 direct 读 API。
    m.add_submodule(&parse)?;

    let db = PyModule::new(py, "db")?;
    db.add_function(wrap_pyfunction!(query, &db)?)?;
    db_api::register(py, &db)?;
    m.add_submodule(&db)?;

    exec_api::register(py, m)?;

    // 注册进 sys.modules，让 `from aios_db import parse` / `import aios_db.db` 都可用。
    let modules = py.import("sys")?.getattr("modules")?;
    modules.set_item("aios_db.parse", &parse)?;
    modules.set_item("aios_db.db", &db)?;
    Ok(())
}
