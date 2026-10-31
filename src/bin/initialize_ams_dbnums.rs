//! Establish per-dbnum design baselines for the project configured in the current working
//! directory's `DbOption.toml` (`project_name`), the way the service's own initialization does:
//! schema first, then the project's SYS metadata (SYST / COMM / MISC / GLB / GLOB → DICT) for
//! every library that has no watermark yet, then the requested design dbnums.
//!
//! 2026-09-07：SYS 元数据不再是一道单独的引导（`bin/sync_sys_only` / `database::bootstrap_sys_metadata`
//! 退役）。它是初始化的一部分——服务启动的首轮重扫把 SYS 库当普通库入队首载、之后 watcher 按会话
//! 增量追；这个工具走同一条 `initialize_dbnum_baseline`，只是不起服务。DESI 的 `ATT_UDA` 要 DICT
//! 先落（`UdaKeyMap`），所以 SYS 这一步排在设计库之前，且不可跳过。
//!
//! `--with-model`（ADR-077）：设计库随初始化一起生成整库模型、属性面与模型面双 lane 直写同一台
//! RocksDB、收尾一笔盖两枚水位——与 dbnum 任务那条路同一个编排（`dbnum_initialize::initialize`）。
//! 输出一行 `INIT|…` 带两条 lane 的账（事务数 / 字节 / 冲突重试 / 超时），是 ADR-077 D3 那道门的尺子。
//! 不带这个开关就是老口径：只落属性面，模型留给启动核对。
//!
//! ```text
//! cargo run --release --bin initialize_ams_dbnums -- --with-model 8000
//! ```
use aios_database::data_interface::dbnum_initialize::ModelHalf;
use aios_database::data_interface::tidb_manager::AiosDBManager;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut with_model = false;
    let mut dbnums = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--with-model" => with_model = true,
            other => dbnums.push(other.parse::<u32>()?),
        }
    }
    anyhow::ensure!(
        !dbnums.is_empty(),
        "usage: initialize_ams_dbnums [--with-model] <dbnum>..."
    );

    // 两条 lane 排空时各打一行账（`persist lane drained: …`）；初始化编排的收尾一行走 println。
    let log_config = simplelog::ConfigBuilder::new()
        .add_filter_allow_str("aios_database::data_interface::persist_writer")
        .add_filter_allow_str("aios_database::data_interface::dbnum_initialize")
        .set_time_format_rfc3339()
        .build();
    let _ = simplelog::TermLogger::init(
        simplelog::LevelFilter::Info,
        log_config,
        simplelog::TerminalMode::Stderr,
        simplelog::ColorChoice::Never,
    );

    aios_database::attach_store_connected().await?;
    // 与 `run_cli` 的 schema 段同一套 DEFINE（函数 + 索引）：空店上没有它们，基线的计数与
    // 统计重算就是全表扫。
    aios_database::data_interface::table_parity::init_schema_on(&aios_core::SUL_DB).await?;
    if with_model {
        // 模型面的谓词全靠 `idx_*_anc`；全新库上没人替它建。语句自带 `IF NOT EXISTS`。
        aios_core::SUL_DB
            .query(aios_database::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
            .await?
            .check()?;
    }
    let project = aios_core::get_db_option().project_name.clone();
    let manager = AiosDBManager::init_form_config().await?;

    for sys in manager.initialize_project_sys_metadata(&project).await? {
        match sys.elements {
            Some(elements) => println!(
                "SYSMETA|{project}|{}|{}|ok|{elements}",
                sys.dbnum, sys.db_type
            ),
            None => println!(
                "SYSMETA|{project}|{}|{}|skipped|already_initialized",
                sys.dbnum, sys.db_type
            ),
        }
    }

    let model = if with_model {
        ModelHalf::Generate
    } else {
        ModelHalf::Skip
    };
    let mut failures = 0usize;
    for dbnum in dbnums {
        let started = std::time::Instant::now();
        match manager.initialize_project_dbnum(&project, dbnum, model).await {
            Ok(outcome) if with_model => {
                let generated = outcome
                    .units
                    .iter()
                    .filter(|unit| {
                        unit.status
                            == aios_database::data_interface::manual_update::UnitGenStatus::Generated
                    })
                    .count();
                println!(
                    "INIT|{project}|{dbnum}|ok|elements={}|roots={}|generated={generated}|failed={}|model_sesno_stamped={}|wall_ms={}|attr_tx={}|attr_bytes={}|attr_conflicts={}|attr_timeouts={}|attr_ms={}|model_tx={}|model_bytes={}|model_conflicts={}|model_timeouts={}|model_ms={}",
                    outcome.elements,
                    outcome.units.len(),
                    outcome.units.len() - generated,
                    outcome.model_stamped,
                    started.elapsed().as_millis(),
                    outcome.attribute_writer.transactions,
                    outcome.attribute_writer.bytes,
                    outcome.attribute_writer.conflict_retries,
                    outcome.attribute_writer.timeouts,
                    outcome.attribute_writer.elapsed_ms,
                    outcome.model_writer.transactions,
                    outcome.model_writer.bytes,
                    outcome.model_writer.conflict_retries,
                    outcome.model_writer.timeouts,
                    outcome.model_writer.elapsed_ms,
                );
                for warning in &outcome.warnings {
                    println!("INIT|{project}|{dbnum}|warning|{warning}");
                }
            }
            Ok(outcome) => {
                println!("BASELINE|{project}|{dbnum}|ok|{}", outcome.elements);
            }
            Err(error) => {
                failures += 1;
                println!("BASELINE|{project}|{dbnum}|failed|{error:#}");
            }
        }
    }
    anyhow::ensure!(failures == 0, "{failures} dbnum baseline(s) failed");
    Ok(())
}
