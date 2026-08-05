//! RVM 基准对拍探针。
//!
//! 与本仓其它 `*_probe.rs` 同一路数：主程序没有 CLI 开关，验证走独立 bin + JSON。
//!
//! 用法：
//!   cargo run --features rvm_verify --bin rvm_verify -- import \
//!       --rvm test_data/rvm/C-IY-1R330-B.rvm --dbnum 8000 [--att x.att] [--out y.json]
//!
//! compare 子命令把快照与 SurrealDB 生成结果做三层对拍并输出机器报告。

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use aios_database::rvm_baseline::{
    CompareOptions, ImportOptions, compare, default_report_path, default_snapshot_path,
    import_and_save,
};

#[derive(Parser, Debug)]
#[command(name = "rvm_verify", about = "RVM 基准对拍：导入与比对")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 解析 E3D 导出的 RVM/ATT，产出基准快照 JSON
    Import {
        /// RVM 文件路径
        #[arg(long)]
        rvm: PathBuf,
        /// 该 RVM 对应的设计库编号，如 8000
        #[arg(long)]
        dbnum: u32,
        /// 配套的 ATT 属性文件，可重复
        #[arg(long)]
        att: Vec<PathBuf>,
        /// 快照输出路径，默认与 RVM 同目录同名 .rvm.json
        #[arg(long)]
        out: Option<PathBuf>,
        /// 根元素真实 refno，如 24384/22404。命名元素的 refno 不在 ATT 里，
        /// 给了就直接钉上，省一次站点库反查。
        #[arg(long)]
        root_refno: Option<String>,
        #[arg(long)]
        verbose: bool,
    },
    /// 快照 vs SurrealDB 生成数据的三层对拍
    ///
    /// 当前 world rotation 与 TUBI join 尚未实现，报告会列出并返回失败。
    Compare {
        /// import 产出的快照 JSON
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long, default_value = "ws://127.0.0.1:8009")]
        url: String,
        #[arg(long, default_value = "1516")]
        ns: String,
        #[arg(long, default_value = "AvevaMarineSample")]
        db: String,
        #[arg(long, default_value = "root")]
        user: String,
        #[arg(long, default_value = "root")]
        password: String,
        /// world 平移容差（mm）
        #[arg(long, default_value_t = 1.0)]
        tol_translation_mm: f64,
        /// AABB 各分量容差（mm）
        #[arg(long, default_value_t = 1.0)]
        tol_aabb_mm: f64,
        /// 报告输出路径，默认 output/rvm-verify/<root>-<时间戳>.json
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        verbose: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Import {
            rvm,
            dbnum,
            att,
            out,
            root_refno,
            verbose,
        } => {
            let out_path = out.unwrap_or_else(|| default_snapshot_path(&rvm));
            let options = ImportOptions {
                dbnum,
                rvm_path: rvm,
                att_paths: att,
                out_path: out_path.clone(),
                root_refno,
                verbose,
            };
            let snapshot = import_and_save(&options)?;
            snapshot.print_summary();
            println!("  快照           : {}", out_path.display());
        }
        Command::Compare {
            snapshot,
            url,
            ns,
            db,
            user,
            password,
            tol_translation_mm,
            tol_aabb_mm,
            report,
            verbose,
        } => {
            let report_path = match report {
                Some(path) => path,
                None => {
                    let loaded = aios_database::rvm_baseline::RvmSnapshot::load(&snapshot)?;
                    default_report_path(loaded.meta.root_name.as_deref())
                }
            };
            let options = CompareOptions {
                snapshot_path: snapshot,
                url,
                ns,
                db,
                user,
                password,
                tol_translation_mm,
                tol_aabb_mm,
                report_path,
                verbose,
            };
            let summary = compare::compare(&options).await?;
            // 退出码 0=容差内全过，1=存在差异，供回归脚本直接判定。
            if !summary.passed() {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
