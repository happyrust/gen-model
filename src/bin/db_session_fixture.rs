//! 通用会话快照夹具管线（阶段一）：`pack` 把「录制产物 + 源 DB 文件」打成便携夹具，
//! `verify` 对夹具目录做零外部依赖的离线复验。
//! 方案：docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md §1。
//!
//! 夹具只入库**最终文件**（zip 单条目）+ manifest + SHA256SUMS；每个关键 sesno 的
//! 历史快照在 pack 时切一遍、散列进还原台账，verify 与阶段三回归再从最终文件
//! **现切**对账——「任意历史可从最终文件精确还原」是被验证的性质，不是假设。
//!
//! Issue #19 的专用实现（`db8000_two_delete_fixture`）保持冻结作回归；通用模块
//! 的重放自检与 pack 往返覆盖在 `tests/db_session_fixture_selfcheck.rs`。
//!
//! ```text
//! db_session_fixture pack --recording <recording.json> --out <夹具目录> [--source <db文件>]
//! db_session_fixture verify --fixture <夹具目录>
//! ```

#[path = "db_session_fixture/archive_util.rs"]
mod archive_util;
#[path = "db_session_fixture/format.rs"]
mod format;
#[path = "db_session_fixture/pipeline.rs"]
mod pipeline;
#[path = "db_session_fixture/session_cut.rs"]
mod session_cut;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(about = "通用 PDMS 会话快照夹具：打包（pack）与离线复验（verify）")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// recording.json + 源 DB 文件 → 夹具目录（zip 只装最终文件）
    Pack {
        /// 源 DB 文件；缺省用 recording.json 的 source 字段
        #[arg(long)]
        source: Option<PathBuf>,
        #[arg(long)]
        recording: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// 与 recording.dbnum 交叉核对（防拿错文件打进错库的夹具）
        #[arg(long)]
        dbnum: Option<u32>,
        /// 覆盖已存在的夹具目录（仅限带 manifest.json 的目录或空目录）
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// 夹具目录复验：档案对账 → 解出最终文件 → 逐台账 sesno 现切 →
    /// SHA256 对账 + sesno/存在性验证闸（与阶段三回归同一套裁决）
    Verify {
        #[arg(long)]
        fixture: PathBuf,
    },
    /// 只读打印 DB 文件的会话链（录制脚本取 baseline、逐宏核对水位用）
    Inspect {
        #[arg(long)]
        source: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    match Args::parse().command {
        Command::Pack {
            source,
            recording,
            out,
            dbnum,
            force,
        } => {
            let report = pipeline::pack(source.as_deref(), &recording, &out, dbnum, force)?;
            report.print(&out);
        }
        Command::Verify { fixture } => {
            let report = pipeline::verify_fixture(&fixture)?;
            report.print(&fixture);
        }
        // JSON 到 stdout：录制脚本按机器可读口径解析，不靠人眼读日志。
        Command::Inspect { source } => {
            let report = pipeline::inspect(&source)?;
            println!("{}", serde_json::to_string(&report)?);
        }
    }
    Ok(())
}
