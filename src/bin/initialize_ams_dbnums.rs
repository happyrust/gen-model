//! Establish per-dbnum design baselines for the project configured in the current working
//! directory's `DbOption.toml` (`project_name`), the way the service's own initialization does:
//! schema first, then the project's SYS metadata (SYST / COMM / MISC / GLB / GLOB → DICT) for
//! every library that has no watermark yet, then the requested design dbnums.
//!
//! 2026-09-07：SYS 元数据不再是一道单独的引导（`bin/sync_sys_only` / `database::bootstrap_sys_metadata`
//! 退役）。它是初始化的一部分——服务启动的首轮重扫把 SYS 库当普通库入队首载、之后 watcher 按会话
//! 增量追；这个工具走同一条 `initialize_dbnum_baseline`，只是不起服务。DESI 的 `ATT_UDA` 要 DICT
//! 先落（`UdaKeyMap`），所以 SYS 这一步排在设计库之前，且不可跳过。
use aios_database::data_interface::tidb_manager::AiosDBManager;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dbnums = std::env::args()
        .skip(1)
        .map(|arg| arg.parse::<u32>())
        .collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(
        !dbnums.is_empty(),
        "usage: initialize_ams_dbnums <dbnum>..."
    );

    aios_core::init_test_surreal().await?;
    // 与 `run_cli` 的 schema 段同一套 DEFINE（函数 + 索引）：空店上没有它们，基线的计数与
    // 统计重算就是全表扫。
    aios_database::data_interface::table_parity::init_schema_on(&aios_core::SUL_DB).await?;
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

    let mut failures = 0usize;
    for dbnum in dbnums {
        match manager
            .initialize_project_dbnum_baseline(&project, dbnum)
            .await
        {
            Ok(count) => {
                println!("BASELINE|{project}|{dbnum}|ok|{count}");
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
