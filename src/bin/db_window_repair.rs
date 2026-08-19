use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "服务停止期间纠正已提交的净窗口（不改变水位）")]
struct Args {
    #[arg(long)]
    dbnum: u32,
    #[arg(long)]
    from: i32,
    #[arg(long)]
    to: i32,
    #[arg(long)]
    expect_watermark: i32,
    #[arg(long)]
    file: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    anyhow::ensure!(
        args.file.is_file(),
        "数据库文件不存在: {}",
        args.file.display()
    );

    let option = aios_core::get_db_option().clone();
    aios_database::acquire_process_instance_lock(&option)
        .context("维护纠正要求 aios-database 服务已停止")?;
    aios_core::init_surreal().await?;

    println!(
        "[repair] 开始 dbnum={} 会话区间={}..={} expect-watermark={} file={}",
        args.dbnum,
        args.from,
        args.to,
        args.expect_watermark,
        args.file.display()
    );
    let report = aios_database::data_interface::window_repair::repair_committed_window(
        args.dbnum,
        args.from,
        args.to,
        args.expect_watermark,
        &args.file,
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!(
        "[repair] 完成 dbnum={} sesno {}..={} 新增={} 修改={} 删除={} 成员补删={} \
         不可达={} watermark {}->{} staging_windows={}",
        report.dbnum,
        report.from_sesno,
        report.to_sesno,
        report.added,
        report.modified,
        report.deleted,
        report.membership_deleted,
        report.unreachable_rows,
        report.watermark_before,
        report.watermark_after,
        report.staging_windows,
    );
    Ok(())
}
