//! 跨库 CATA 闭包探针（Phase 2/5 端到端验证，**无需 SurrealDB**）。
//!
//! 目录扫描建 `ref0→dbnum` 定位器 → 对设计根跑 refno 级引用闭包 → 打印 CATA 闭包 manifest。
//!
//! 单根用法：
//! ```text
//! cargo run --bin cata_closure_probe -- --dir "...\AvevaMarineSample" --root 24384_18447
//! ```
//! 整库用法（统计一个 DESI 库引用的全部 CATA 参考号，用其 world 根做全子树闭包）：
//! ```text
//! cargo run --bin cata_closure_probe -- --dir "...\AvevaMarineSample" \
//!   --whole-file "...\AvevaMarineSample\ams000\ams8000_0001"
//! ```

use aios_core::RefU64;
use aios_database::data_interface::cata_closure::{
    CataClosureConfig, CataDbLocator, InMemoryCataLocator, run_cata_closure_pass_for_refnos,
};
use clap::Parser;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Parser, Debug)]
#[command(name = "cata_closure_probe")]
#[command(about = "目录扫描定位器 + 设计根跨库 CATA 闭包（无需 SurrealDB）")]
struct Args {
    /// 工程根目录（含 DESI + CATA 库）。
    #[arg(short, long)]
    dir: String,
    /// 工程名。
    #[arg(short, long, default_value = "AvevaMarineSample")]
    project: String,
    /// 单设计根 refno（与 --whole-file 二选一，如 24384_18447）。
    #[arg(short, long)]
    root: Option<String>,
    /// 整库统计：给一个 DESI 库文件，用其 world 根做全子树闭包，统计它引用的全部 CATA 参考号。
    #[arg(short, long)]
    whole_file: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    println!("== 扫描目录建定位器 == {}", args.dir);
    let locator = InMemoryCataLocator::build_from_dir(&args.project, &PathBuf::from(&args.dir));
    println!(
        "== 定位器 == dbnum={} ref0={}",
        locator.dbnum_count(),
        locator.ref0_count()
    );

    // 确定闭包种子根：--whole-file（取库 world 根）优先，否则 --root。
    let root: RefU64 = if let Some(wf) = &args.whole_file {
        let path = PathBuf::from(wf);
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let db_basic =
            parse_pdms_db::parse::parse_file_db_basic_data(&path, &file_name, &args.project)?;
        println!(
            "== 整库 == {} world_refno={:?} refnos={}",
            wf,
            db_basic.world_refno,
            db_basic.refno_table_map.len()
        );
        db_basic.world_refno
    } else if let Some(r) = &args.root {
        RefU64::from_str(r).map_err(|_| anyhow::anyhow!("非法 root refno: {}", r))?
    } else {
        anyhow::bail!("需要 --root 或 --whole-file 之一");
    };

    println!(
        "== 根 == {:?} dbnum={:?} type={:?}",
        root,
        locator.dbnum_of_ref0(root.get_0()),
        locator
            .dbnum_of_ref0(root.get_0())
            .and_then(|d| locator.db_type_of(d))
    );

    let manifest =
        run_cata_closure_pass_for_refnos(&locator, &[root], CataClosureConfig::precise()).await?;

    let total: usize = manifest.by_dbnum.values().map(|s| s.len()).sum();
    println!(
        "== 闭包 == 库数={} seeds={} visited={} missing={} rounds={}",
        manifest.by_dbnum.len(),
        manifest.seed_count,
        manifest.visited_count,
        manifest.missing,
        manifest.rounds
    );
    println!(
        "== 总 CATA 参考号 == {} （分布在 {} 个 CATA 库）",
        total,
        manifest.by_dbnum.len()
    );
    for sample in &manifest.missing_samples {
        println!("  missing: {sample}");
    }
    for (dbnum, refs) in &manifest.by_dbnum {
        let ty = locator.db_type_of(*dbnum).unwrap_or_default();
        println!("  dbnum={} type={} refnos={}", dbnum, ty, refs.len());
    }
    Ok(())
}
