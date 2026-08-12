//! 执行层（`aios_db.incr` / `aios_db.model` / `aios_db.room` / `aios_db.spatial`）：
//! mutating 管线（部分只读观察函数如 `resolve_window` / `queue_status` /
//! `room.code` / `spatial.status` 放宽为连接层可用）。
//!
//! 三层守护的最后一层：这些函数在 `full_init` 之前一律报错。`full_init` 拿的是
//! 与 `run_app`/`run_cli` 同一把项目单实例锁——服务在跑时 `full_init` 直接失败，
//! 这不是缺陷而是防线（两个进程并发驱动同一批 staging 窗口 / 队列 / pending
//! 表会互踩）。初始化序列严格对齐 `run_cli` 前置段，不自创第二套。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use aios_core::RefnoEnum;
use anyhow::Context;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use serde_json::json;

use crate::{CONNECTED, anyhow_to_py, ensure_connected, pythonized, runtime};

static FULL_INIT: AtomicBool = AtomicBool::new(false);

fn ensure_full() -> PyResult<()> {
    if FULL_INIT.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(PyRuntimeError::new_err(
            "执行层未初始化：先停掉 gen-model 服务，再调用 aios_db.full_init(config, cwd=仓库根)。\
             （mutating 管线与在跑服务并发会互踩暂存窗口/队列/pending 表，单实例锁就是防这个的）",
        ))
    }
}

fn parse_refno(refno: &str) -> RefnoEnum {
    RefnoEnum::from(refno.trim())
}

fn parse_refnos(refnos: Vec<String>) -> Vec<RefnoEnum> {
    refnos.iter().map(|refno| parse_refno(refno)).collect()
}

async fn db_option() -> anyhow::Result<aios_core::options::DbOption> {
    Ok(crate::db_api::manager().await?.db_option.clone())
}

/// 问一个本地端口的 `/api/v1/health`，返回整份 health JSON；不是活的 gen-model
/// 服务就返回 None（连不上、超时、不是 JSON、没有 project 字段，一律不算数）。
///
/// 刻意用裸 TCP 而不是引第三方 HTTP 客户端：绑定 crate 的依赖与主 crate 逐字
/// 对齐才能共享 `[patch]` 与 Cargo.lock，为一次探测加一棵 reqwest 依赖树不值。
/// `Connection: close` 让服务端答完即关，`read_to_end` 自然收尾。
fn health_snapshot(port: u16, timeout: std::time::Duration) -> Option<serde_json::Value> {
    use std::io::{Read, Write};

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = std::net::TcpStream::connect_timeout(&addr, timeout).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    stream
        .write_all(
            b"GET /api/v1/health HTTP/1.1\r\n\
              Host: 127.0.0.1\r\n\
              Accept: application/json\r\n\
              Connection: close\r\n\r\n",
        )
        .ok()?;
    let mut raw = Vec::new();
    // 上限兜住「端口上蹲着个会一直吐字节的东西」这种情况。
    stream.take(256 * 1024).read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let body = text.split("\r\n\r\n").nth(1)?;
    let value: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    // 合法 health 的最低门槛；端口上蹲着别的 HTTP 程序时在这里出局。
    value.get("project")?.as_str()?;
    Some(value)
}

/// SurrealDB 端点归一：小写 + `localhost` → `127.0.0.1`。
///
/// 只做这一步：探测双方都在本机，配置里写 localhost 还是回环地址纯属习惯差异；
/// 「一边写 LAN IP 一边写 localhost 指同一台库」这种形态不展开（要可靠区分得
/// 枚举本机全部地址，收益配不上），漏判的兜底仍是单实例锁与人。
fn normalize_endpoint(raw: &str) -> String {
    let raw = raw.trim().to_ascii_lowercase();
    match raw.split_once(':') {
        Some((host, port)) => {
            let host = match host {
                "localhost" | "" => "127.0.0.1",
                other => other,
            };
            format!("{host}:{port}")
        }
        None => raw,
    }
}

/// 判一份 health 是否与本配置构成互踩，是则给出说得出口的理由。
///
/// 判据从宽到严三层：`project` 不同 → 无关；服务端报了 `sul_db.endpoint`
/// （2026-08-12 起）→ 库端点不同或 namespace 不同都放行（同名工程各写各的库，
/// 不构成互踩）；老版本服务端不报端点 → 分不清就按最坏情况拦（保守），误伤面
/// 即「同名工程的隔离沙箱」，调用方用 `force=True` 放行。
fn conflict_reason(
    our_project: &str,
    our_namespace: &str,
    our_endpoint: &str,
    health: &serde_json::Value,
) -> Option<String> {
    let project = health.get("project")?.as_str()?;
    if project != our_project {
        return None;
    }
    let Some(endpoint) = health
        .pointer("/sul_db/endpoint")
        .and_then(|value| value.as_str())
    else {
        return Some(format!(
            "工程 {project}；服务端未报 SurrealDB 端点（0.1.18 及更早），按最坏情况判"
        ));
    };
    if normalize_endpoint(endpoint) != our_endpoint {
        return None;
    }
    if let Some(namespace) = health.get("namespace") {
        // identity.namespace 历史上有过数字与字符串两种序列化形态，都按字符串比。
        let namespace = namespace
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| namespace.to_string());
        if namespace != our_namespace {
            return None;
        }
    }
    Some(format!("工程 {project} 且同库 {endpoint}"))
}

/// 找出真会与本进程互踩的活服务，返回 `(端口, 理由)`。
///
/// 存在的理由：单实例锁按「项目根」隔离，而两个部署包（各自的仓库/发布目录）
/// 各持各的锁，却可以写同一个 SurrealDB、同一个工程——锁根本挡不住这种互踩。
/// 实测踩过：`test-worklspace` 的部署包在 9099，本仓库在 8022，两把锁互不相干。
///
/// 端口被别的程序占用不算冲突——误伤比漏报更烦人。
fn conflicting_services(db_option: &aios_core::options::DbOption) -> Vec<(u16, String)> {
    let mut ports: Vec<u16> = Vec::new();
    // 本配置声明的端口优先，再加两个常用部署口。`http_api_addr` 挂在扩展配置上
    // （异地部署那几个键与 DbOption 分开），走 get_db_option_ext 取。
    if let Some(port) = aios_database::get_db_option_ext()
        .http_api_addr
        .as_deref()
        .and_then(|addr| addr.rsplit(':').next())
        .and_then(|port| port.parse::<u16>().ok())
    {
        ports.push(port);
    }
    for port in [8022u16, 9099] {
        if !ports.contains(&port) {
            ports.push(port);
        }
    }
    let our_endpoint = normalize_endpoint(&format!("{}:{}", db_option.v_ip, db_option.v_port));
    let timeout = std::time::Duration::from_millis(400);
    ports
        .into_iter()
        .filter_map(|port| {
            let health = health_snapshot(port, timeout)?;
            let reason = conflict_reason(
                &db_option.project_name,
                &db_option.surreal_ns,
                &our_endpoint,
                &health,
            )?;
            Some((port, reason))
        })
        .collect()
}

/// 完整初始化：拿单实例锁 + `run_cli` 前置段（schema/函数自检、增量状态表、
/// 索引回填、空间树加载），**不**启动 watcher / 批次 worker / Web 服务。
///
/// `config` / `cwd` 语义同 [`crate::connect`]；`cwd` 通常必须是 gen-model 仓库根
/// （`resource/surreal/` 按 CWD 找）。幂等：重复调用是空操作。
///
/// 拿锁之后还会探一次「有没有别的活服务在伺候同一个工程」（见
/// [`conflicting_services`]），发现就拒绝；`force=True` 显式跳过这道探测。
#[pyfunction]
#[pyo3(signature = (config=None, cwd=None, force=false))]
pub fn full_init(
    py: Python<'_>,
    config: Option<String>,
    cwd: Option<PathBuf>,
    force: bool,
) -> PyResult<()> {
    if FULL_INIT.load(Ordering::SeqCst) {
        return Ok(());
    }
    if let Some(cwd) = cwd {
        std::env::set_current_dir(&cwd).map_err(|error| {
            PyRuntimeError::new_err(format!("切换工作目录到 {} 失败: {error}", cwd.display()))
        })?;
    }
    if let Some(config) = config {
        unsafe { std::env::set_var("DB_OPTION_FILE", &config) };
    }
    py.detach(|| {
        runtime().block_on(async {
            // 1. 单实例锁：必须先于一切连接与状态加载（与 run_cli 同序）。
            let db_option = aios_core::get_db_option().clone();
            aios_database::acquire_process_instance_lock(&db_option)
                .context("获取项目单实例锁失败（服务是不是还在跑？）")?;

            // 1b. 锁挡不住跨部署互踩（锁按项目根隔离，两个部署包各持各的锁却写
            //     同一个工程），所以再探一次同工程活服务。放在锁之后：锁能挡的
            //     场景不必付探测的几百毫秒。锁是 OnceLock 且同工程幂等，这里报错
            //     退出不影响修好后在同进程里重试。
            if !force {
                let conflicts = conflicting_services(&db_option);
                if !conflicts.is_empty() {
                    let who = conflicts
                        .iter()
                        .map(|(port, reason)| format!("127.0.0.1:{port}（{reason}）"))
                        .collect::<Vec<_>>()
                        .join("、");
                    anyhow::bail!(
                        "检测到还有服务在伺候同一个工程：{who}。执行层与它并发会互踩\
                         暂存窗口/队列/pending 表——先停掉那个服务，或确认无害后用 \
                         aios_db.full_init(..., force=True) 跳过本检查"
                    );
                }
            }

            // 2. 连接 + define_common_functions + 编译期内置函数快照（含 D11 的
            //    hd/hh 矫正；与 connect / run_cli 同款）。幂等；connect 层可能已做过。
            if !CONNECTED.load(Ordering::SeqCst) {
                aios_core::init_surreal().await?;
                aios_database::data_interface::embedded_surql::define_embedded_functions()
                    .await?;
                CONNECTED.store(true, Ordering::SeqCst);
            }

            // 3. 收口事务依赖的自定义函数自检 + 增量状态表兼容检查 + dbnum 事件。
            //    与 run_cli 同款：自检失败中止初始化（函数缺失时跑 mutating 必炸）。
            aios_database::data_interface::increment_pipeline::selfcheck_surreal_functions()
                .await
                .context("SurrealDB 自定义函数自检失败")?;
            let migrated = aios_database::data_interface::dbnum_state::DbnumState::
                ensure_increment_state_storage()
            .await?;
            println!("增量状态表检查完成（兼容检查 {migrated} 个旧 DBNUM 水位）");
            aios_database::versioned_db::database::define_dbnum_event()
                .await
                .context("重新定义 update_dbnum_event 失败")?;

            // 4. inst_relate 索引 + 自愈回填/清扫（run_cli 同款：失败不阻断，出声）。
            if let Err(error) =
                aios_database::fast_model::pdms_inst::init_inst_relate_indices().await
            {
                eprintln!("初始化 inst_relate 索引失败: {error}");
            }
            if let Err(error) =
                aios_database::fast_model::pdms_inst::backfill_inst_relate_anc().await
            {
                eprintln!("inst_relate anc/dbnum 回填失败: {error}");
            }
            if let Err(error) = aios_database::fast_model::pdms_inst::sweep_inst_relate_flat().await
            {
                eprintln!("inst_relate 平表副本清扫失败: {error}");
            }

            // 5. 空间树（可重建的派生数据；加载失败以空树继续，与 run_cli 一致）。
            //    降级两态由后台复检收敛（runtime 全局常驻，随进程退出）。
            if let Err(error) =
                aios_database::fast_model::aabb_tree::load_project_tree_verified().await
            {
                eprintln!("空间树加载失败（{error:#}），以空树继续");
            }
            aios_database::fast_model::spatial_state::spawn_spatial_revalidator();

            // 6. manager（监控目录解析；后续 enqueue / drain / 生成都用它）。
            crate::db_api::manager().await?;
            anyhow::Ok(())
        })
    })
    .map_err(anyhow_to_py)?;
    FULL_INIT.store(true, Ordering::SeqCst);
    Ok(())
}

// ── incr ────────────────────────────────────────────────────────────────────

/// 对单个库文件执行一个增量窗口（默认窗口 = 水位+1 ..= 文件最新会话）。
///
/// 走与服务完全相同的 `IncrementPipeline::apply`（暂存窗口 → 校验 → 提交 →
/// 水位推进 → durable 模型工作登记）；不含模型生成本身（那是 `incr.drain_data`
/// 或 `model.*` 的事）。
#[pyfunction]
#[pyo3(signature = (path, start=None, end=None))]
pub fn apply_file(
    py: Python<'_>,
    path: PathBuf,
    start: Option<i32>,
    end: Option<i32>,
) -> PyResult<Py<PyAny>> {
    ensure_full()?;
    let value = py
        .detach(|| {
            runtime().block_on(async {
                use std::io::Read;
                let mut file = std::fs::File::open(&path)
                    .with_context(|| format!("打开 {} 失败", path.display()))?;
                let mut head = [0u8; 60];
                file.read_exact(&mut head).context("读取文件头失败")?;
                let basic = parse_pdms_db::parse::parse_file_basic_info(&head);

                let mut io = pdms_io::io::PdmsIO::new("", path.clone(), true);
                io.open()
                    .map_err(|error| anyhow::anyhow!("打开 PDMS IO 失败: {error}"))?;
                let info = io
                    .get_page_basic_info()
                    .map_err(|error| anyhow::anyhow!("读取页级基础信息失败: {error}"))?;
                let latest = info.latest_ses_data.sesno;

                let watermark =
                    aios_database::data_interface::sesno_range::SesnoRangeResolver::query_watermark(
                        basic.db_no,
                    )
                    .await? as i32;
                let start = start.unwrap_or(watermark + 1);
                let end = end.unwrap_or(latest);
                if start > end {
                    return anyhow::Ok(json!({
                        "up_to_date": true,
                        "dbnum": basic.db_no,
                        "applied_sesno": watermark,
                        "file_latest_sesno": latest,
                    }));
                }

                let mut ranges = indexmap::IndexMap::new();
                ranges.insert(path.clone(), (info, start..=end, basic.db_type.clone()));
                let result =
                    aios_database::data_interface::increment_pipeline::IncrementPipeline::new()
                        .apply(ranges)
                        .await;

                let successes = result
                    .successes
                    .iter()
                    .map(|success| {
                        json!({
                            "dbnum": success.dbnum,
                            "db_type": success.db_type,
                            "path": success.path.display().to_string(),
                            "start_sesno": success.start_sesno,
                            "end_sesno": success.end_sesno,
                            "changed_refnos": success.changed_refnos.len(),
                        })
                    })
                    .collect::<Vec<_>>();
                let errors = result
                    .errors
                    .iter()
                    .map(|error| {
                        json!({
                            "path": error.path.display().to_string(),
                            "error": error.error.to_string(),
                        })
                    })
                    .collect::<Vec<_>>();
                anyhow::Ok(json!({
                    "up_to_date": false,
                    "window": [start, end],
                    "successes": successes,
                    "errors": errors,
                }))
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 扫描 + 入队 + **当场消费到队列为空**（等价一次手动更新执行）。
///
/// 返回 `{ receipt: 入队回执, drained: 消费的批次数 }`。队列处于持久化暂停时
/// 会先报错（用 `incr.queue_resume()` 解除）。
#[pyfunction]
#[pyo3(signature = (project=None, mdb=None, dbnums=None))]
pub fn execute_manual(
    py: Python<'_>,
    project: Option<String>,
    mdb: Option<String>,
    dbnums: Option<Vec<u32>>,
) -> PyResult<Py<PyAny>> {
    ensure_full()?;
    let value = py
        .detach(|| {
            runtime().block_on(async {
                let mgr = crate::db_api::manager().await?;
                let scheduler =
                    aios_database::data_interface::batch_scheduler::BatchScheduler::global();
                if scheduler.is_paused() {
                    anyhow::bail!(
                        "队列处于持久化暂停状态，先调用 aios_db.incr.queue_resume() 解除"
                    );
                }
                let project = project.unwrap_or_else(|| mgr.db_option.project_name.clone());
                let receipt = mgr
                    .enqueue_manual_update(&project, mdb.as_deref(), dbnums.as_deref())
                    .await;
                let drained =
                    aios_database::data_interface::batch_worker::drain_queue_until_empty(mgr).await;
                anyhow::Ok(json!({
                    "receipt": serde_json::to_value(&receipt)?,
                    "drained": drained,
                }))
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 消化 durable pending 的前两个数据阶段（非 regen → regen），不含房间。
#[pyfunction]
pub fn drain_data(py: Python<'_>) -> PyResult<usize> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            let mgr = crate::db_api::manager().await?;
            aios_database::data_interface::model_update_pending::drain_data_phases(mgr).await
        })
    })
    .map_err(anyhow_to_py)
}

/// 只读预览下一增量窗口（不执行、不动水位；连接层即可用）。
///
/// 返回 `{dbnum, db_type, window: [start, end], cold_start, db_latest_sesno,
/// file_latest_sesno}`；已到最新时返回 `{up_to_date: true, ...}`。与 watcher /
/// 手动更新同一套窗口决策（`SesnoRangeResolver::resolve`）。
#[pyfunction]
#[pyo3(signature = (path, skip_cata=false))]
pub fn resolve_window(py: Python<'_>, path: PathBuf, skip_cata: bool) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let value = py
        .detach(|| {
            runtime().block_on(async {
                use std::io::Read;
                let mut file = std::fs::File::open(&path)
                    .with_context(|| format!("打开 {} 失败", path.display()))?;
                let mut head = [0u8; 60];
                file.read_exact(&mut head).context("读取文件头失败")?;
                let basic = parse_pdms_db::parse::parse_file_basic_info(&head);

                let mut io = pdms_io::io::PdmsIO::new("", path.clone(), true);
                io.open()
                    .map_err(|error| anyhow::anyhow!("打开 PDMS IO 失败: {error}"))?;
                let latest = io
                    .get_page_basic_info()
                    .map_err(|error| anyhow::anyhow!("读取页级基础信息失败: {error}"))?
                    .latest_ses_data
                    .sesno;

                let mgr = crate::db_api::manager().await?;
                let project = mgr.db_option.project_name.clone();
                let plan = aios_database::data_interface::sesno_range::SesnoRangeResolver::new()
                    .resolve(
                        &path,
                        &project,
                        basic.db_no,
                        latest,
                        skip_cata,
                        &basic.db_type,
                    )
                    .await?;
                anyhow::Ok(match plan {
                    Some(plan) => json!({
                        "up_to_date": false,
                        "dbnum": basic.db_no,
                        "db_type": plan.db_type,
                        "window": [*plan.range.start(), *plan.range.end()],
                        "cold_start": plan.cold_start,
                        "db_latest_sesno": plan.db_latest_sesno,
                        "file_latest_sesno": plan.file_latest_sesno,
                    }),
                    None => json!({
                        "up_to_date": true,
                        "dbnum": basic.db_no,
                        "db_type": basic.db_type,
                        "file_latest_sesno": latest,
                    }),
                })
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 消化 SystDerived / RefRevMaintain 两类提交后副作用（**不含**空间收敛——那
/// 是 `spatial.reconcile()` 的事），返回本轮完成的作业数。
///
/// 零售组合（`apply_file` → `drain_data` → `room.drain`）不会像批次闭环那样
/// 自动收尾副作用，脚本收工前应依次调本函数与 `spatial.reconcile()`。
#[pyfunction]
pub fn drain_side_effects(py: Python<'_>) -> PyResult<usize> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            let mgr = crate::db_api::manager().await?;
            aios_database::data_interface::side_effect_pending::SideEffectCompensator::drain(mgr)
                .await
        })
    })
    .map_err(anyhow_to_py)
}

/// 队列状态快照（连接层只读）：先从库同步持久化暂停位，再报进程内调度器状态。
///
/// 返回 `{paused, rows: [{task_id, dbnum, db_type, state, start_sesno,
/// end_sesno}]}`。注意 `rows` 是**本进程**调度器的队列（服务进程的队列要走
/// `aios_client.queue()` 问在跑服务）。
#[pyfunction]
pub fn queue_status(py: Python<'_>) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let value = py
        .detach(|| {
            runtime().block_on(async {
                let scheduler =
                    aios_database::data_interface::batch_scheduler::BatchScheduler::global();
                scheduler.restore_persisted_pause().await?;
                anyhow::Ok(json!({
                    "paused": scheduler.is_paused(),
                    "rows": serde_json::to_value(scheduler.snapshot())?,
                }))
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 解除队列的持久化暂停（等价 `POST /queue/resume`：持久化标志 + 内存标志一起清）。
#[pyfunction]
pub fn queue_resume(py: Python<'_>) -> PyResult<bool> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            aios_database::data_interface::batch_scheduler::BatchScheduler::global()
                .set_paused_persistent(false)
                .await
        })
    })
    .map_err(anyhow_to_py)?;
    Ok(false)
}

/// 暂停队列出队（等价 `POST /queue/pause`；只挡出队，正在跑的批次跑完为止）。
#[pyfunction]
pub fn queue_pause(py: Python<'_>) -> PyResult<bool> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            aios_database::data_interface::batch_scheduler::BatchScheduler::global()
                .set_paused_persistent(true)
                .await
        })
    })
    .map_err(anyhow_to_py)?;
    Ok(true)
}

// ── model ───────────────────────────────────────────────────────────────────

/// 按需生成单个构件的模型（与 `POST /model/ensure` 同源；幂等，`force` 才重生成）。
#[pyfunction]
#[pyo3(signature = (refno, force=false))]
pub fn ensure(py: Python<'_>, refno: String, force: bool) -> PyResult<Py<PyAny>> {
    ensure_full()?;
    let result = py
        .detach(|| {
            runtime().block_on(async {
                let mgr = crate::db_api::manager().await?;
                mgr.ensure_model_generated(parse_refno(&refno), force).await
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &result)
}

/// 对指定 refno 集重建深层网格数据（`process_meshes_update_db_deep`）。
#[pyfunction]
#[pyo3(name = "gen")]
pub fn gen_models(py: Python<'_>, refnos: Vec<String>) -> PyResult<()> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            let db_option = db_option().await?;
            aios_database::fast_model::occ_generate::process_meshes_update_db_deep(
                &db_option,
                &parse_refnos(refnos),
            )
            .await
        })
    })
    .map_err(anyhow_to_py)
}

/// 整库模型生成（`process_meshes_by_dbnos`）。
#[pyfunction]
pub fn gen_dbnum(py: Python<'_>, dbnum: u32) -> PyResult<()> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            let db_option = db_option().await?;
            aios_database::fast_model::gen_model::process_meshes_by_dbnos(&[dbnum], &db_option)
                .await
        })
    })
    .map_err(anyhow_to_py)
}

/// 刷新指定 refno 集的 `inst_relate` aabb，返回真发生变化的元素列表
/// `[{refno, noun}, ...]`——形态与 `room.enqueue` 的入参一致，noun 决定房间
/// 分支（PANE → 整间，其它 → 元素）。
///
/// `durable=True` 走定向增量入口（`update_inst_relate_aabbs_by_refnos_incremental`，
/// 生产上 TransformOnly / 定向 regen 用的就是它）：直写时把 AABB 指针、
/// `room_recalc` 任务与 spatial epoch 放进同一个事务。注意其中 room 任务的发布
/// 还受 `room_incremental` 开关（配置键或 AIOS_ROOM_INCREMENTAL）门控，指针与
/// epoch 不受。默认 False 走普通刷新——包围盒确有变化时同样带 epoch bump，
/// 但不发布房间任务（要排队用 `room.enqueue`）。
#[pyfunction]
#[pyo3(signature = (refnos, replace=false, durable=false))]
pub fn update_aabbs(
    py: Python<'_>,
    refnos: Vec<String>,
    replace: bool,
    durable: bool,
) -> PyResult<Py<PyAny>> {
    ensure_full()?;
    let value = py
        .detach(|| {
            runtime().block_on(async {
                let refnos = parse_refnos(refnos);
                let changes = if durable {
                    aios_database::fast_model::occ_generate::
                        update_inst_relate_aabbs_by_refnos_incremental(&refnos, replace)
                    .await?
                } else {
                    aios_database::fast_model::occ_generate::update_inst_relate_aabbs_by_refnos(
                        &refnos, replace,
                    )
                    .await?
                };
                anyhow::Ok(serde_json::Value::Array(
                    changes
                        .iter()
                        .map(|change| {
                            json!({
                                "refno": change.refno.to_string(),
                                "noun": change.noun,
                            })
                        })
                        .collect(),
                ))
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 删除元素（含其 pe 子树）的全部模型数据：级联删 `inst_relate` / `inst_info` /
/// 几何边，清房间归属两个方向的边，并把包围盒从空间树上摘掉
/// （`delete_inst_relate_subtree`，与 DeleteCleanup 补偿任务同一入口、幂等）。
/// 直写时房间边删除与 spatial epoch bump 同事务提交。
#[pyfunction]
#[pyo3(signature = (refnos, chunk_size=100))]
pub fn delete_subtree(py: Python<'_>, refnos: Vec<String>, chunk_size: usize) -> PyResult<()> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            aios_database::data_interface::helper::delete_inst_relate_subtree(
                &parse_refnos(refnos),
                chunk_size,
            )
            .await
        })
    })
    .map_err(anyhow_to_py)
}

/// 把一个元素（含子树）的已生成网格导出为 OBJ 目视检查。
///
/// 顶点/法线用 `world_trans × inst.transform` 变换到世界坐标，每个交付单元
/// 一个 `{refno}.obj`（内部按 geo_hash 分 `o` 组）。**连接层即可用**（读库 +
/// 读 mesh 文件 + 写 `dir`，不碰模型/增量数据）——刻意不设 full_init 门，
/// 服务在跑时也能导出。前提是模型已生成过（`.mesh` 文件在 meshes 目录里）。
#[pyfunction]
#[pyo3(signature = (refno, dir))]
pub fn export_obj(py: Python<'_>, refno: String, dir: PathBuf) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let value = py
        .detach(|| {
            runtime().block_on(async {
                // 交付单元根（EQUI/BRAN…）自身通常没有直接 inst_relate——实例挂在
                // 具体图元上。先按 `db.inst` 同款 anc 谓词把整棵子树的实例 refno
                // 收齐，再喂给生产同源的 query_valid_insts（有效实例口径的唯一权威，
                // 只按 key 直取、不做子树展开）。
                let normalized = crate::db_api::normalize_refno(&refno);
                let refno_u64 = {
                    use std::str::FromStr;
                    aios_core::RefU64::from_str(&normalized)
                        .map(|r| ((r.get_0() as u64) << 32) | r.get_1() as u64)
                        .map_err(|error| {
                            anyhow::anyhow!(
                                "refno {refno} 解析失败（{error}）——期待 =a/b、a/b 或 a_b 形制。\
                                 打包值解析不出来时 anc 谓词永不命中，宁可报错也不静默导出空集"
                            )
                        })?
                };
                // 只走 `anc CONTAINS`（idx_inst_relate_anc 索引查询）。anc 含自身，
                // 元素自己的实例行也被同一谓词圈住——不要再 OR 一个 `in = …` 臂：
                // `in` 上没有索引（preload.rs 的实测账：in 谓词全表扫 1.57s，
                // 图跳/索引 3.1ms），OR 会把整条谓词退化回全表扫。
                let ids = crate::db_api::take_json(
                    "SELECT VALUE record::id(id) FROM inst_relate WHERE anc CONTAINS $refno_u64;"
                        .into(),
                    vec![("refno_u64", json!(refno_u64))],
                )
                .await?;
                let inst_refnos: Vec<RefnoEnum> = ids
                    .as_array()
                    .map(|rows| {
                        rows.iter()
                            .filter_map(|row| row.as_str())
                            .map(parse_refno)
                            .collect()
                    })
                    .unwrap_or_default();
                if inst_refnos.is_empty() {
                    // 空结果分两种，必须说得出是哪种（plant-ui P3 同款纪律：anc 未
                    // 回填响亮失败带自愈指引，不静默降级）。存量行 anc 未回填的库上
                    // 子树收集会漏行；gen-model 启动序列的 backfill_inst_relate_anc
                    // 幂等自愈，只连库不启服务的调试进程才看得到未回填态。探针与
                    // rs-core `inst_relate_anc_ready` 同口径（LIMIT 1，全回填库扫
                    // 不到即通过；只在空结果分支付这一次扫描成本）。
                    let unfilled = crate::db_api::take_json(
                        "SELECT VALUE record::id(id) FROM inst_relate WHERE anc = NONE LIMIT 1;"
                            .into(),
                        vec![],
                    )
                    .await?;
                    if unfilled.as_array().is_some_and(|rows| !rows.is_empty()) {
                        anyhow::bail!(
                            "库里还有 anc 未回填的 inst_relate 行，子树收集不可信——\
                             先启动一次 gen-model（启动回填幂等自愈）再导"
                        );
                    }
                    anyhow::bail!(
                        "元素 {refno} 及其子树没有任何几何实例（模型没生成过？\
                         先 model.ensure 再导）"
                    );
                }
                let insts =
                    aios_database::data_interface::staging::query_valid_insts(&inst_refnos)
                        .await?;
                if insts.is_empty() {
                    anyhow::bail!(
                        "元素 {refno} 有 {} 个实例但全部缺 aabb/world_trans（生成没收口？\
                         先 model.ensure(force=True) 再导）",
                        inst_refnos.len()
                    );
                }
                let mesh_dir = db_option().await?.get_meshes_path();
                std::fs::create_dir_all(&dir)
                    .with_context(|| format!("创建输出目录 {} 失败", dir.display()))?;

                // 整棵子树合成一个 {refno}.obj，内部按「实例_geo_hash」分 `o` 组；
                // 顶点/法线用各实例自己的 world_trans × inst.transform 变换到世界坐标。
                let mut obj = String::new();
                let mut vertex_base = 1usize;
                let mut triangles = 0usize;
                let mut exported = 0usize;
                let mut total_insts = 0usize;
                let mut missing = Vec::new();
                for geom_inst in &insts {
                    for inst in &geom_inst.insts {
                        total_insts += 1;
                        let mesh_path = mesh_dir.join(format!("{}.mesh", inst.geo_hash));
                        let Ok(mesh) =
                            aios_core::shape::pdms_shape::PlantMesh::des_mesh_file(&mesh_path)
                        else {
                            missing.push(inst.geo_hash.clone());
                            continue;
                        };
                        let matrix = (geom_inst.world_trans * inst.transform).compute_matrix();
                        let with_normals = mesh.normals.len() == mesh.vertices.len();
                        obj.push_str(&format!(
                            "o {}_{}\n",
                            geom_inst.refno.to_string().replace('/', "_"),
                            inst.geo_hash
                        ));
                        for vertex in &mesh.vertices {
                            let p = matrix.transform_point3(*vertex);
                            obj.push_str(&format!("v {} {} {}\n", p.x, p.y, p.z));
                        }
                        if with_normals {
                            for normal in &mesh.normals {
                                let d = matrix.transform_vector3(*normal).normalize_or_zero();
                                obj.push_str(&format!("vn {} {} {}\n", d.x, d.y, d.z));
                            }
                        }
                        for tri in mesh.indices.chunks_exact(3) {
                            let (a, b, c) = (
                                tri[0] as usize + vertex_base,
                                tri[1] as usize + vertex_base,
                                tri[2] as usize + vertex_base,
                            );
                            if with_normals {
                                obj.push_str(&format!("f {a}//{a} {b}//{b} {c}//{c}\n"));
                            } else {
                                obj.push_str(&format!("f {a} {b} {c}\n"));
                            }
                            triangles += 1;
                        }
                        vertex_base += mesh.vertices.len();
                        exported += 1;
                    }
                }
                let file = dir.join(format!("{normalized}.obj"));
                std::fs::write(&file, obj)
                    .with_context(|| format!("写 {} 失败", file.display()))?;
                let files = vec![json!({
                    "refno": normalized,
                    "path": file.display().to_string(),
                    "insts": total_insts,
                    "exported_insts": exported,
                    "triangles": triangles,
                })];
                anyhow::Ok(json!({ "files": files, "missing_meshes": missing }))
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

// ── sync ────────────────────────────────────────────────────────────────────

/// 给一个从未解析过的 dbnum 补一次全量基线（首次入库），幂等收口水位与生成工作。
///
/// 与自动 watcher / 手动更新走同一入口（`initialize_project_dbnum_baseline`）：
/// 全量解析 → PE/dbnum_info 一致性校验 → 登记模型生成工作 → 水位推进原子收口。
/// 返回 `{dbnum, planned_roots}`（空库 planned_roots=0）。注意：对已入库的
/// dbnum 调用会重走收口（重登记生成工作、水位落到文件最新会话），谨慎使用。
#[pyfunction]
#[pyo3(signature = (dbnum, project=None))]
pub fn baseline(py: Python<'_>, dbnum: u32, project: Option<String>) -> PyResult<Py<PyAny>> {
    ensure_full()?;
    let value = py
        .detach(|| {
            runtime().block_on(async {
                let mgr = crate::db_api::manager().await?;
                let project = project.unwrap_or_else(|| mgr.db_option.project_name.clone());
                let planned_roots = mgr
                    .initialize_project_dbnum_baseline(&project, dbnum)
                    .await?;
                anyhow::Ok(json!({ "dbnum": dbnum, "planned_roots": planned_roots }))
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

// ── room ────────────────────────────────────────────────────────────────────

/// 房间归属全量重建（`build_room_relations`）。
#[pyfunction]
pub fn build_all(py: Python<'_>) -> PyResult<()> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            let db_option = db_option().await?;
            aios_database::fast_model::room_model::build_room_relations(&db_option).await
        })
    })
    .map_err(anyhow_to_py)
}

/// 消化待重算的房间归属目标（第三阶段），返回 `DrainReport` 的 JSON 形态
/// （requested/loaded/done + 逐条失败 + 失败牵涉的 dbnum）。
#[pyfunction]
pub fn drain(py: Python<'_>) -> PyResult<Py<PyAny>> {
    ensure_full()?;
    let value = py
        .detach(|| {
            runtime().block_on(async {
                let db_option = db_option().await?;
                let report =
                    aios_database::data_interface::model_update_pending::drain_rooms(&db_option)
                        .await?;
                anyhow::Ok(json!({
                    "requested": report.requested,
                    "loaded": report.loaded,
                    "done": report.done,
                    "failures": report.failures,
                    "failed_dbnums": report.failed_dbnums,
                }))
            })
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 把「包围盒确实变了」的元素排进房间重算队列（`enqueue_room_recalc`），
/// 返回入队条数。
///
/// 入参就是 `model.update_aabbs` 的返回形态 `[{refno, noun}, ...]`，按 noun
/// 分流（PANE → `room_recalc_panel` 整间分支，其它 → `room_recalc_element`）。
/// 与 Rust 夹具对拍测试同一触发方式，**不受** `room_incremental` 开关影响；
/// 消费用 `room.drain()`。同 target 只占一行（record id 不带 dbnum，UPSERT 递增
/// revision），重复入队幂等。
#[pyfunction]
pub fn enqueue(py: Python<'_>, changes: Bound<'_, PyAny>) -> PyResult<usize> {
    ensure_full()?;
    #[derive(serde::Deserialize)]
    struct ChangeIn {
        refno: String,
        noun: String,
    }
    let changes: Vec<ChangeIn> = pythonize::depythonize(&changes)?;
    let changes: Vec<aios_database::fast_model::occ_generate::AabbChange> = changes
        .into_iter()
        .map(|change| aios_database::fast_model::occ_generate::AabbChange {
            refno: parse_refno(&change.refno),
            noun: change.noun,
        })
        .collect();
    let count = changes.len();
    py.detach(|| {
        runtime().block_on(
            aios_database::data_interface::model_update_pending::enqueue_room_recalc(&changes),
        )
    })
    .map_err(anyhow_to_py)?;
    Ok(count)
}

/// 元素的房间编码（`fn::room_code` 直通，连接层只读；无归属返回 None）。
#[pyfunction]
pub fn code(py: Python<'_>, refno: String) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let refno = crate::db_api::normalize_refno(&refno);
    let value = py
        .detach(|| {
            runtime().block_on(crate::db_api::take_json(
                "RETURN fn::room_code(type::thing('pe', $refno));".into(),
                vec![("refno", json!(refno))],
            ))
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 元素（BRAN 等）穿过的房间 PANE refno 列表（`fn::get_room_nodes` 直通）。
#[pyfunction]
pub fn nodes(py: Python<'_>, refno: String) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let refno = crate::db_api::normalize_refno(&refno);
    let value = py
        .detach(|| {
            runtime().block_on(crate::db_api::take_json(
                "RETURN fn::get_room_nodes(type::thing('pe', $refno));".into(),
                vec![("refno", json!(refno))],
            ))
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 元素穿过的房间号列表（`fn::get_room_names` 直通）。
#[pyfunction]
pub fn names(py: Python<'_>, refno: String) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let refno = crate::db_api::normalize_refno(&refno);
    let value = py
        .detach(|| {
            runtime().block_on(crate::db_api::take_json(
                "RETURN fn::get_room_names(type::thing('pe', $refno));".into(),
                vec![("refno", json!(refno))],
            ))
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

// ── spatial ─────────────────────────────────────────────────────────────────

/// 空间收敛积压状态（连接层只读）：`{pending, retries, last_error, stalled}`。
#[pyfunction]
pub fn status(py: Python<'_>) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let value = py
        .detach(|| {
            runtime().block_on(
                aios_database::data_interface::side_effect_pending::SideEffectCompensator::
                    spatial_reconcile_status(),
            )
        })
        .map_err(anyhow_to_py)?;
    pythonized(py, &value)
}

/// 空间树状态（连接层只读）：原样透出 /health `spatial_tree` 那份渲染。
///
/// 键面以渲染半边（`aabb_tree::render_spatial_tree_status`）为唯一权威，形状钉
/// 也在那边——这里不复述全集，免得两处各说一套（G-02 契约迁移期间它正从九键
/// 走向十五键，复述必然过期）。稳定核：`entries`（当前内存树条目数）、
/// `file_epoch` / `db_epoch`、`drift`、`startup_verdict`。
///
/// 指纹现读现比（不是启动快照）：`drift=true` 而空间收敛又没有积压
/// （`spatial.status()`），说明树相对库在静默漂移。
#[pyfunction]
pub fn tree_status(py: Python<'_>) -> PyResult<Py<PyAny>> {
    ensure_connected()?;
    let value =
        py.detach(|| runtime().block_on(aios_database::fast_model::aabb_tree::spatial_tree_status()));
    pythonized(py, &value)
}

/// 消化待收敛的空间意图（树刷新/删除 + 文件持久化），返回收敛条数。
///
/// 与 batch worker 出队门同一实现——零售组合（`apply_file` / `drain_data` /
/// `room.drain` / `model.gen*`）收工前必须调，否则空间意图滞留 pending 表、
/// 内存树不落盘（要等下次服务启动重放自愈）。
#[pyfunction]
pub fn reconcile(py: Python<'_>) -> PyResult<usize> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            let mgr = crate::db_api::manager().await?;
            aios_database::data_interface::side_effect_pending::SideEffectCompensator::
                reconcile_spatial_pending(mgr)
            .await
        })
    })
    .map_err(anyhow_to_py)
}

/// 把内存空间树落盘。`force=False` 只在脏时写（返回是否真的写了）；
/// `force=True` 无条件写回并清脏标记。
///
/// `force=True` 只在 Ready/ReadyEmpty 放行（一致性闭环方案 §7）：重放/重建/降级
/// 中的树无条件覆盖快照，会把中间态或不可信内容写过好文件。脏位路径自带发布门，
/// 不额外拦。
#[pyfunction]
#[pyo3(signature = (force=false))]
pub fn persist(py: Python<'_>, force: bool) -> PyResult<bool> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            if force {
                aios_database::fast_model::spatial_state::ensure_spatial_ready()?;
                aios_database::fast_model::aabb_tree::persist_aabb_tree().await?;
                anyhow::Ok(true)
            } else {
                aios_database::fast_model::aabb_tree::persist_aabb_tree_if_dirty().await
            }
        })
    })
    .map_err(anyhow_to_py)
}

/// 从库内包围盒指针全量重建空间树并立即落盘（树损坏/陈旧时的兜底）。
#[pyfunction]
pub fn rebuild(py: Python<'_>) -> PyResult<()> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(aios_database::fast_model::aabb_tree::rebuild_tree_from_pointers())
    })
    .map_err(anyhow_to_py)
}

// ── fixture（测试支撑：合成房间夹具）────────────────────────────────────────
//
// 与 Rust 侧 `room_fixture` live 测试**同一套**夹具：1 间房 `/ZZ-R-K100` +
// 2 块 PANE（A: 0..1000 / B: 900..1900，重叠区 900..1000）+ 5 个盒形构件
// （2 个在 A、2 个在 B、1 个骑在重叠区上），保留 refno 段 4000000001。
// **只对一次性测试库使用**：create 会写 pe / FRMW / inst_* / geo_relate /
// aabb / vec3 多张表并在 mesh 目录落 `zzfx_*.mesh`；drop 按固定 id 清理。
// 配置的 `room_key_word` 需含 "ZZ-R-" 才能让 build_all / drain 只圈住夹具房。

async fn fixture_mesh_dir(explicit: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    match explicit {
        Some(dir) => Ok(dir),
        None => Ok(db_option().await?.get_meshes_path()),
    }
}

/// 建房间夹具（幂等：内部先 drop 再建）。`mesh_dir` 缺省用配置的 meshes 目录。
#[pyfunction]
#[pyo3(name = "create", signature = (mesh_dir=None))]
pub fn fixture_create(py: Python<'_>, mesh_dir: Option<PathBuf>) -> PyResult<()> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            let dir = fixture_mesh_dir(mesh_dir).await?;
            aios_database::fast_model::room_fixture::create_room_fixture(&dir).await
        })
    })
    .map_err(anyhow_to_py)
}

/// 清夹具（库内记录 + `.mesh` 文件），幂等。
#[pyfunction]
#[pyo3(name = "drop", signature = (mesh_dir=None))]
pub fn fixture_drop(py: Python<'_>, mesh_dir: Option<PathBuf>) -> PyResult<()> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            let dir = fixture_mesh_dir(mesh_dir).await?;
            aios_database::fast_model::room_fixture::drop_room_fixture(&dir).await
        })
    })
    .map_err(anyhow_to_py)
}

/// 把一个夹具几何体搬到新包围盒（`min` / `max` 为世界坐标三元组）。
///
/// 只动几何侧（`aabb:zzfx_*` 记录、`vec3` 顶点、面板还重写 `.mesh`），**不碰**
/// `inst_relate.aabb`——那要靠 `model.update_aabbs` 从 geo 侧重算，走的正是
/// 「包围盒真的变了」的触发源；直接改指针等于绕过被测对象。
#[pyfunction]
#[pyo3(name = "move_body", signature = (seq, min, max, mesh_dir=None))]
pub fn fixture_move_body(
    py: Python<'_>,
    seq: u64,
    min: Vec<f32>,
    max: Vec<f32>,
    mesh_dir: Option<PathBuf>,
) -> PyResult<()> {
    ensure_full()?;
    py.detach(|| {
        runtime().block_on(async {
            anyhow::ensure!(
                min.len() == 3 && max.len() == 3,
                "min/max 需为三元组 [x, y, z]（收到 {} / {} 个分量）",
                min.len(),
                max.len()
            );
            let dir = fixture_mesh_dir(mesh_dir).await?;
            aios_database::fast_model::room_fixture::move_fixture_body(
                &dir,
                seq,
                glam::Vec3::new(min[0], min[1], min[2]),
                glam::Vec3::new(max[0], max[1], max[2]),
            )
            .await
        })
    })
    .map_err(anyhow_to_py)
}

/// 夹具清单：面板/构件的 refno（`a_b` 形态）、`move_body` 用的 seq、房间号。
#[pyfunction]
#[pyo3(name = "refnos")]
pub fn fixture_refnos(py: Python<'_>) -> PyResult<Py<PyAny>> {
    fn seq_of(refno: &str) -> u64 {
        refno
            .rsplit('_')
            .next()
            .and_then(|tail| tail.parse().ok())
            .unwrap_or(0)
    }
    let (pane_a, pane_b) = aios_database::fast_model::room_fixture::panel_refnos();
    let (in_a, in_b, straddler) = aios_database::fast_model::room_fixture::part_refnos();
    let value = json!({
        // 与夹具源常量一致（room_fixture.rs 的 ROOM_NUM）；边表断言用。
        "room_num": "K100",
        "pane_a": pane_a,
        "pane_b": pane_b,
        "in_a": in_a,
        "in_b": in_b,
        "straddler": straddler,
        "seqs": {
            "pane_a": seq_of(&pane_a),
            "pane_b": seq_of(&pane_b),
            "in_a": in_a.iter().map(|r| seq_of(r)).collect::<Vec<_>>(),
            "in_b": in_b.iter().map(|r| seq_of(r)).collect::<Vec<_>>(),
            "straddler": seq_of(&straddler),
        },
    });
    pythonized(py, &value)
}

// ── 注册 ────────────────────────────────────────────────────────────────────

pub fn register(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(full_init, m)?)?;

    let incr = PyModule::new(py, "incr")?;
    incr.add_function(wrap_pyfunction!(apply_file, &incr)?)?;
    incr.add_function(wrap_pyfunction!(execute_manual, &incr)?)?;
    incr.add_function(wrap_pyfunction!(drain_data, &incr)?)?;
    incr.add_function(wrap_pyfunction!(resolve_window, &incr)?)?;
    incr.add_function(wrap_pyfunction!(drain_side_effects, &incr)?)?;
    incr.add_function(wrap_pyfunction!(queue_pause, &incr)?)?;
    incr.add_function(wrap_pyfunction!(queue_resume, &incr)?)?;
    incr.add_function(wrap_pyfunction!(queue_status, &incr)?)?;
    m.add_submodule(&incr)?;

    let model = PyModule::new(py, "model")?;
    model.add_function(wrap_pyfunction!(ensure, &model)?)?;
    model.add_function(wrap_pyfunction!(gen_models, &model)?)?;
    model.add_function(wrap_pyfunction!(gen_dbnum, &model)?)?;
    model.add_function(wrap_pyfunction!(update_aabbs, &model)?)?;
    model.add_function(wrap_pyfunction!(delete_subtree, &model)?)?;
    model.add_function(wrap_pyfunction!(export_obj, &model)?)?;
    m.add_submodule(&model)?;

    let room = PyModule::new(py, "room")?;
    room.add_function(wrap_pyfunction!(build_all, &room)?)?;
    room.add_function(wrap_pyfunction!(drain, &room)?)?;
    room.add_function(wrap_pyfunction!(enqueue, &room)?)?;
    room.add_function(wrap_pyfunction!(code, &room)?)?;
    room.add_function(wrap_pyfunction!(nodes, &room)?)?;
    room.add_function(wrap_pyfunction!(names, &room)?)?;
    m.add_submodule(&room)?;

    let spatial = PyModule::new(py, "spatial")?;
    spatial.add_function(wrap_pyfunction!(status, &spatial)?)?;
    spatial.add_function(wrap_pyfunction!(tree_status, &spatial)?)?;
    spatial.add_function(wrap_pyfunction!(reconcile, &spatial)?)?;
    spatial.add_function(wrap_pyfunction!(persist, &spatial)?)?;
    spatial.add_function(wrap_pyfunction!(rebuild, &spatial)?)?;
    m.add_submodule(&spatial)?;

    let fixture = PyModule::new(py, "fixture")?;
    fixture.add_function(wrap_pyfunction!(fixture_create, &fixture)?)?;
    fixture.add_function(wrap_pyfunction!(fixture_drop, &fixture)?)?;
    fixture.add_function(wrap_pyfunction!(fixture_move_body, &fixture)?)?;
    fixture.add_function(wrap_pyfunction!(fixture_refnos, &fixture)?)?;
    m.add_submodule(&fixture)?;

    let sync = PyModule::new(py, "sync")?;
    sync.add_function(wrap_pyfunction!(baseline, &sync)?)?;
    m.add_submodule(&sync)?;

    let modules = py.import("sys")?.getattr("modules")?;
    modules.set_item("aios_db.incr", &incr)?;
    modules.set_item("aios_db.model", &model)?;
    modules.set_item("aios_db.room", &room)?;
    modules.set_item("aios_db.spatial", &spatial)?;
    modules.set_item("aios_db.fixture", &fixture)?;
    modules.set_item("aios_db.sync", &sync)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{conflict_reason, normalize_endpoint};
    use serde_json::json;

    const OURS: (&str, &str, &str) = ("AvevaMarineSample", "1516", "127.0.0.1:8071");

    fn reason(health: serde_json::Value) -> Option<String> {
        conflict_reason(OURS.0, OURS.1, OURS.2, &health)
    }

    #[test]
    fn endpoint_normalization_folds_localhost_and_case() {
        assert_eq!(normalize_endpoint("localhost:8009"), "127.0.0.1:8009");
        assert_eq!(normalize_endpoint("LOCALHOST:8009"), "127.0.0.1:8009");
        assert_eq!(normalize_endpoint(" 127.0.0.1:8009 "), "127.0.0.1:8009");
        // 非回环主机名原样保留（只归一习惯差异，不做地址簿枚举）。
        assert_eq!(normalize_endpoint("192.168.31.58:8009"), "192.168.31.58:8009");
    }

    #[test]
    fn different_project_is_unrelated() {
        assert_eq!(reason(json!({ "project": "ZDJ" })), None);
    }

    #[test]
    fn old_server_without_endpoint_is_conservatively_flagged() {
        // 0.1.18 及更早的 /health 不报 sul_db.endpoint：分不清就按最坏情况拦。
        let flagged = reason(json!({ "project": "AvevaMarineSample", "namespace": "1516" }));
        assert!(flagged.is_some());
        assert!(flagged.unwrap().contains("未报 SurrealDB 端点"));
    }

    #[test]
    fn same_project_on_a_different_database_is_cleared() {
        // 正是房间增量沙箱的形态：工程重名，但库是自己的一次性实例。
        assert_eq!(
            reason(json!({
                "project": "AvevaMarineSample",
                "namespace": "1516",
                "sul_db": { "endpoint": "localhost:8009" },
            })),
            None
        );
    }

    #[test]
    fn same_project_same_database_is_flagged_with_reason() {
        let flagged = reason(json!({
            "project": "AvevaMarineSample",
            "namespace": "1516",
            "sul_db": { "endpoint": "localhost:8071" },
        }));
        assert!(flagged.is_some());
        assert!(flagged.unwrap().contains("同库"));
    }

    #[test]
    fn different_namespace_on_same_database_is_cleared() {
        assert_eq!(
            reason(json!({
                "project": "AvevaMarineSample",
                "namespace": "9999",
                "sul_db": { "endpoint": "127.0.0.1:8071" },
            })),
            None
        );
    }

    #[test]
    fn numeric_namespace_serialization_still_matches() {
        // identity.namespace 历史上有过数字形态；按字符串比不受序列化形态牵连。
        let flagged = reason(json!({
            "project": "AvevaMarineSample",
            "namespace": 1516,
            "sul_db": { "endpoint": "127.0.0.1:8071" },
        }));
        assert!(flagged.is_some());
    }

    #[test]
    fn missing_namespace_is_conservatively_flagged() {
        // 端点已对上、namespace 读不到：按相同处理（保守），不放行。
        let flagged = reason(json!({
            "project": "AvevaMarineSample",
            "sul_db": { "endpoint": "localhost:8071" },
        }));
        assert!(flagged.is_some());
    }
}
