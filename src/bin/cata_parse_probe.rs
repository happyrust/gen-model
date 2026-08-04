//! 按需解析 CATA · 解析原语探针：对单个 db 文件验证 by-refno 部分解析 + 出向引用抽取。
//!
//! **不依赖 SurrealDB**，直接读文件，用于验证移植的解析闭包地基是否在真实数据上工作。
//!
//! 用法：
//! ```text
//! cargo run --bin cata_parse_probe -- --file "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001"
//! ```

use aios_core::RefU64;
use clap::Parser;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "cata_parse_probe")]
#[command(about = "对单个 db 文件验证 by-refno 部分解析 + 出向引用抽取（不依赖 SurrealDB）")]
struct Args {
    /// db 文件路径。
    #[arg(short, long)]
    file: String,
    /// 工程名（parse 语义需要）。
    #[arg(short, long, default_value = "AvevaMarineSample")]
    project: String,
    /// 采样解析多少个 refno。
    #[arg(short, long, default_value_t = 5)]
    sample: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let path = PathBuf::from(&args.file);
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    println!("== 文件 == {}", path.display());

    // 1. 头信息（60 字节）：db_no / db_type。
    let info = parse_pdms_db::parse::parse_db_basic_info(path.clone());
    println!(
        "== db 头 == db_no={} db_type={:?} ses_pgno={}",
        info.db_no, info.db_type, info.ses_pgno
    );

    // 2. 全量索引（index-only，不解析属性）：refno_table_map / children_map。
    let db_basic =
        parse_pdms_db::parse::parse_file_db_basic_data(&path, &file_name, &args.project)?;
    let all_refnos: Vec<RefU64> = db_basic.refno_table_map.iter().map(|e| *e.key()).collect();
    println!(
        "== 索引 == refno_table_map={} children_map={} bytes={}",
        all_refnos.len(),
        db_basic.children_map.len(),
        db_basic.bytes.len()
    );

    // 3. ref0 集（定位器 ref0→dbnum 的来源）。
    let mut ref0s: HashSet<u32> = HashSet::new();
    for r in &all_refnos {
        ref0s.insert(r.get_0());
    }
    let ref0_sample: Vec<u32> = ref0s.iter().copied().take(8).collect();
    println!(
        "== ref0 集 == distinct={} sample={:?}",
        ref0s.len(),
        ref0_sample
    );

    // 4. by-refno 部分解析采样 + 出向引用抽取。
    let sample: Vec<RefU64> = all_refnos.iter().copied().take(args.sample).collect();
    let parsed =
        aios_database::data_interface::cata_closure::parse_db_refnos(&args.project, &path, &sample)
            .await?;
    println!("== 部分解析 == 请求={} 成功={}", sample.len(), parsed.len());
    for refno in &sample {
        if let Some(ele) = parsed.get(refno) {
            println!(
                "  refno={:?} noun={} outbound={} children={}",
                refno,
                ele.noun_name,
                ele.outbound.len(),
                ele.children.len()
            );
        }
    }
    Ok(())
}
