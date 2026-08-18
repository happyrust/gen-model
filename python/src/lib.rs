//! aios-database 的 Python 调试绑定（模块名 `aios_db`）。
//!
//! 设计与决策见 `docs/plans/2026-08-11-python-binding-api-plan.md`。M1 范围：
//!
//! - `aios_db.connect(config=None)` —— 连接层（连 `SUL_DB`，**不拿**单实例锁）
//! - `aios_db.db.query(sql, binds=None)` —— SurrealQL 直通
//! - `aios_db.parse.header / is_db_file / sessions / collect_changes` —— 纯文件解析
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
/// 会触碰配置的函数（`connect`、`parse.collect_changes` 等）之前。解析层看似
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
    use std::io::Read;
    let mut file =
        std::fs::File::open(path).with_context(|| format!("打开 {} 失败", path.display()))?;
    let mut head = [0u8; 60];
    file.read_exact(&mut head)
        .with_context(|| format!("读取 {} 前 60 字节文件头失败", path.display()))?;
    let basic = parse_pdms_db::parse::parse_file_basic_info(&head);

    let mut io = pdms_io::io::PdmsIO::new("", path.to_path_buf(), true);
    io.open()
        .map_err(|error| anyhow::anyhow!("打开 PDMS IO 失败: {error}"))?;
    let info = io
        .get_page_basic_info()
        .map_err(|error| anyhow::anyhow!("读取页级基础信息失败: {error}"))?;
    Ok(serde_json::json!({
        "db_type": basic.db_type,
        "dbnum": basic.db_no,
        "latest_sesno": info.latest_ses_data.sesno,
        "latest_ses_time": info.latest_ses_data.get_dt().to_rfc3339(),
        "latest_ses_pageno": info.latest_ses_pageno,
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
    let mut io = pdms_io::io::PdmsIO::new("", path.to_path_buf(), true);
    io.open()
        .map_err(|error| anyhow::anyhow!("打开 PDMS IO 失败: {error}"))?;
    let mut sessions: Vec<_> = io.ses_data_map.values().collect();
    sessions.sort_by_key(|session| session.sesno);
    let rows = sessions
        .into_iter()
        .map(|session| {
            serde_json::json!({
                "sesno": session.sesno,
                "pgno": session.pgno,
                "end_pgno": session.end_pgno,
                "index_root_pageno": session.index_root_pageno,
                "claim_pageno": session.claim_pageno,
                "computer_name": session.get_computer_name(),
                "comments": session.get_comments_name(),
                "date": session.get_dt().to_rfc3339(),
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::Value::Array(rows))
}

/// 从库文件直读单个元素的属性 dump（不进库、不需要连接）。
///
/// `sesno` 缺省读最新版本；给定时读「该会话或之前」可见的最后版本（做历史对比）。
/// 返回的 `found_sesno` 是命中的版本真正所在的会话号。原始解析、不处理 UDA。
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
    let mut io = pdms_io::io::PdmsIO::new("", path.to_path_buf(), true);
    io.open()
        .map_err(|error| anyhow::anyhow!("打开 PDMS IO 失败: {error}"))?;
    let (found_sesno, offset) = io.search_latest_refno(parsed, sesno).ok_or_else(|| {
        anyhow::anyhow!(
            "库文件里找不到元素 {refno}{}",
            sesno
                .map(|sesno| format!("（会话 <= {sesno}）"))
                .unwrap_or_default()
        )
    })?;
    let data = io
        .parse_raw_element(offset)
        .map_err(|error| anyhow::anyhow!("解析元素 {refno} 失败: {error}"))?;
    Ok(convert::ele_data_to_json(&data, found_sesno))
}

/// 解析 attlib 字典（Attribute Data File），返回全部 noun 的能力矩阵。
///
/// 与 E3D `core.dll` 的 `ATTOPE`/`ATGTIX`/`ATNLOG` 读取链同源（`parse_pdms_db::dict`），
/// 含 base_type 继承与默认表兜底——替代手写复刻的 `gm_noun_caps_probe.py`。
#[pyfunction]
fn noun_dict(py: Python<'_>, attlib_path: PathBuf) -> PyResult<Py<PyAny>> {
    let value = py
        .detach(|| {
            let dict = parse_pdms_db::dict::AttrDataFile::open(&attlib_path)?;
            anyhow::Ok(serde_json::json!({
                "noun_count": dict.noun_count(),
                "field_count": dict.field_count(),
                "nouns": dict.all_noun_capabilities(),
            }))
        })
        .map_err(anyhow_to_py)?;
    Ok(pythonize(py, &value)?.unbind())
}

/// **LEGACY** 逐会话回放收集（ADR-031）：按窗口内每个会话认领本会话新写记录再
/// 做属性 diff。纯函数、不写库、不动水位。
///
/// 生产预览 / 执行走的是 [`net_window`]（`IncrementPipeline::collect_window`），
/// **不是**本入口。这里只给跨结构对拍和「哪个会话动的」取证用。
///
/// 返回 `{sesno: [op, ...]}`；`detail=False` 时属性只给名字列表，`detail=True`
/// 给完整旧值/新值。
#[pyfunction]
#[pyo3(signature = (path, start, end, detail=false))]
fn collect_changes(
    py: Python<'_>,
    path: PathBuf,
    start: i32,
    end: i32,
    detail: bool,
) -> PyResult<Py<PyAny>> {
    let value = py
        .detach(|| {
            let window =
                aios_database::data_interface::increment_pipeline::IncrementPipeline::collect_changes(
                    &path,
                    start..=end,
                )?;
            anyhow::Ok(convert::window_to_json(&window, detail))
        })
        .map_err(anyhow_to_py)?;
    Ok(pythonize(py, &value)?.unbind())
}

/// 会话索引差分：给定 sesno 窗口，**只靠文件本身**判定窗口内的净增删改——
/// 不查任何数据库、不逐会话解析记录，复杂度与窗口内会话数解耦。
///
/// 与 `parse.collect_changes`（逐会话回放）互为对拍：回放给出每会话操作明细，
/// 差分给出净三态（窗口内加了又删不出现，删了又建判 modified）。实现见
/// `data_interface::session_index_diff`（存在性口径与生产 B+ 树点查逐字对齐）。
///
/// 返回 `{requested_start, requested_end, base_sesno, target_sesno,
/// added/deleted/modified: [{refno, record_pgno, record_offset,
/// last_touch_sesno, noun}], counts, stats}`；`with_noun=True` 时按记录位置
/// 解析记录头补类型名（Deleted 解析的是旧记录，每 refno 一次，显式付费）。
#[pyfunction]
#[pyo3(signature = (path, start, end, with_noun=false))]
fn net_changes(
    py: Python<'_>,
    path: PathBuf,
    start: i32,
    end: i32,
    with_noun: bool,
) -> PyResult<Py<PyAny>> {
    let value = py
        .detach(|| {
            let mut io = pdms_io::io::PdmsIO::new("", path.clone(), true);
            io.open()
                .map_err(|error| anyhow::anyhow!("打开 PDMS IO 失败: {error}"))?;
            let set = aios_database::data_interface::session_index_diff::collect_net_changes(
                &mut io,
                start..=end,
                with_noun,
            )?;
            anyhow::Ok(set.to_json())
        })
        .map_err(anyhow_to_py)?;
    Ok(pythonize(py, &value)?.unbind())
}

/// **正式口径**（ADR-031）：解析器语义净窗口。会话索引差分先圈出两端记录位置，
/// 再在文件内解析 base / 终稿并做一次属性 diff。与增量管线 / 手动预览共用同一
/// 实现（`IncrementPipeline::collect_window` → `collect_net_window`）。
///
/// 与 [`net_changes`] 的索引触达三态不同，本入口会过滤“换页但内容相同”的原样
/// 重写。E3D 自己推进的显式元数据（例如 BRAN.CACHID）仍会如实返回。
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
            let mut io = pdms_io::io::PdmsIO::new("", path.clone(), true);
            io.open()
                .map_err(|error| anyhow::anyhow!("打开 PDMS IO 失败: {error}"))?;
            let outcome = aios_database::data_interface::net_window::collect_net_window(
                &mut io,
                start..=end,
            )?;
            let mut counts = serde_json::Map::from_iter([
                ("added".into(), serde_json::json!(0usize)),
                ("deleted".into(), serde_json::json!(0usize)),
                ("modified".into(), serde_json::json!(0usize)),
            ]);
            for operation in outcome.window.values().flatten() {
                let key = match &operation.detail {
                    pdms_io::io::EleOperationDetail::Add(_) => "added",
                    pdms_io::io::EleOperationDetail::Deleted => "deleted",
                    pdms_io::io::EleOperationDetail::Modified(_) => "modified",
                    pdms_io::io::EleOperationDetail::None => continue,
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
    parse.add_function(wrap_pyfunction!(collect_changes, &parse)?)?;
    parse.add_function(wrap_pyfunction!(net_changes, &parse)?)?;
    parse.add_function(wrap_pyfunction!(net_window, &parse)?)?;
    parse.add_function(wrap_pyfunction!(element, &parse)?)?;
    parse.add_function(wrap_pyfunction!(noun_dict, &parse)?)?;
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
