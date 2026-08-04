//! 按需解析 CATA · 单根几何冒烟校验（Phase 5）。
//!
//! 对给定设计参考号逐个 `resolve_desi_comp` 算确定性摘要，用于「开/关
//! `AIOS_CATA_CLOSURE_MODE`」跨运行 diff —— 证明「按需解析 == 整库解析」不漏/不改几何。
//!
//! 用法（跑两遍，比对输出的 combined_digest / per_refno）：
//! ```text
//! # 基线：整库 / CATA 已解析（显式关；注意默认已改为 On，必须显式 off 才走整库对照）
//! $env:AIOS_CATA_CLOSURE_MODE = "off"; cargo run --bin cata_smoke -- --refnos 24383_66456,24383_66457
//! # 按需：命中未解析时惰性兜底补齐（默认即开，或显式 on）
//! $env:AIOS_CATA_CLOSURE_MODE = "on"; cargo run --bin cata_smoke -- --refnos 24383_66456,24383_66457
//! ```
//! 两次的 `combined_digest` 应完全一致；不一致即定位到 per_refno 里发散的元件。

use aios_core::options::DbOption;
use aios_core::{RefnoEnum, SUL_DB};
use clap::Parser;
use config::{Config, File};

#[derive(Parser, Debug)]
#[command(name = "cata_smoke")]
#[command(about = "按需解析 CATA 单根几何冒烟：开/关 AIOS_CATA_CLOSURE_MODE 跑两遍比对几何摘要")]
struct Args {
    /// 设计参考号列表，逗号分隔（如 24383_66456,24383_66457）。
    #[arg(short, long)]
    refnos: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 连接 SurrealDB（与 test_query::init_test_surreal 同款，从根目录 DbOption 配置读取）。
    let cfg = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = cfg.try_deserialize()?;
    SUL_DB
        .connect(db_option.get_version_db_conn_str())
        .with_capacity(1000)
        .await?;
    SUL_DB
        .use_ns(&db_option.project_code)
        .use_db(&db_option.project_name)
        .await?;

    let args = Args::parse();
    let refnos: Vec<RefnoEnum> = args
        .refnos
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(RefnoEnum::from)
        .filter(|r| r.is_valid())
        .collect();

    eprintln!(
        "[cata_smoke] mode_on={} refnos={}",
        aios_database::data_interface::cata_closure::cata_closure_enabled(),
        refnos.len()
    );

    let report = aios_database::data_interface::cata_closure::geo_smoke_digest(&refnos).await;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
