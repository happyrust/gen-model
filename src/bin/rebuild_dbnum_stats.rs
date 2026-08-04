//! 手动重建 `dbnum_info_table` 统计：连接当前工作目录 `DbOption.toml` 指向的
//! SurrealDB，对点名的 dbnum 从 pe 全量重算 per-ref0 统计。
//!
//! 存在的理由：`update_dbnum_event` 只做增量维护，事件曾被不兼容实现覆盖（或
//! 写入发生在事件缺失的窗口）漏记的 count **不会自愈**；而自动 rebuild 只挂在
//! 基线路径（applied=0）上，已有基线的库没有别的纠偏入口。播种完整性告警
//! （`dbnum_state::seed_integrity_warnings`）报了哪个 dbnum，就拿它来修哪个。
//!
//! 用法（在含 DbOption.toml 的目录下执行）：
//!     rebuild_dbnum_stats <dbnum>...

use aios_core::SUL_DB;
use serde::Deserialize;

#[derive(Deserialize, Default)]
struct StatSnapshot {
    #[serde(default)]
    pe_count: Option<i64>,
    #[serde(default)]
    info_count: Option<i64>,
}

#[derive(Deserialize, Default)]
struct Identity {
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    db_type: Option<String>,
}

async fn snapshot(dbnum: u32) -> anyhow::Result<(i64, i64)> {
    let mut response = SUL_DB
        .query(format!(
            "RETURN {{ pe_count: (SELECT count() AS count FROM pe WHERE dbnum = {dbnum} \
             GROUP ALL)[0].count ?? 0, \
             info_count: (SELECT math::sum(count) AS count FROM dbnum_info_table \
             WHERE dbnum = {dbnum} GROUP ALL)[0].count ?? 0 }};"
        ))
        .await?
        .check()?;
    let row: Option<StatSnapshot> = response.take(0)?;
    let row = row.unwrap_or_default();
    Ok((
        row.pe_count.unwrap_or_default(),
        row.info_count.unwrap_or_default(),
    ))
}

/// 登记身份优先取水位表（权威登记），退而求其次继承现有统计行。
async fn identity(dbnum: u32) -> anyhow::Result<(String, String)> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT file_name, db_type FROM dbnum_watermark:{dbnum};\
             SELECT file_name, db_type FROM dbnum_info_table WHERE dbnum = {dbnum} LIMIT 1;"
        ))
        .await?
        .check()?;
    let watermark: Vec<Identity> = response.take(0)?;
    let info: Vec<Identity> = response.take(1)?;
    let pick = |get: fn(&Identity) -> Option<&String>| {
        watermark
            .first()
            .and_then(get)
            .filter(|s| !s.is_empty())
            .or_else(|| info.first().and_then(get).filter(|s| !s.is_empty()))
            .cloned()
            .unwrap_or_default()
    };
    Ok((
        pick(|i| i.file_name.as_ref()),
        pick(|i| i.db_type.as_ref()),
    ))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dbnums = std::env::args()
        .skip(1)
        .map(|arg| arg.parse::<u32>())
        .collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(!dbnums.is_empty(), "usage: rebuild_dbnum_stats <dbnum>...");

    aios_core::init_test_surreal().await?;

    let mut failures = 0usize;
    for dbnum in dbnums {
        let (pe_before, info_before) = snapshot(dbnum).await?;
        let (file_name, db_type) = identity(dbnum).await?;
        if file_name.is_empty() && db_type.is_empty() {
            println!(
                "REBUILD|{dbnum}|warn|水位表与统计表都没有该库的登记身份，file_name/db_type 将写为空"
            );
        }
        match aios_database::versioned_db::database::rebuild_dbnum_info_from_pe(
            dbnum, &file_name, &db_type,
        )
        .await
        {
            Ok(rows) => {
                let (pe_after, info_after) = snapshot(dbnum).await?;
                println!(
                    "REBUILD|{dbnum}|ok|pe {pe_before}->{pe_after} 条, 统计 {info_before}->{info_after} 条（重算 {rows} 行 PE）{}",
                    if pe_after == info_after {
                        ""
                    } else {
                        "，仍不一致：重建期间可能有并发写入，请复跑"
                    }
                );
            }
            Err(error) => {
                failures += 1;
                println!("REBUILD|{dbnum}|failed|{error:#}");
            }
        }
    }
    anyhow::ensure!(failures == 0, "{failures} 个 dbnum 统计重建失败");
    Ok(())
}
