//! 通用会话快照夹具管线（阶段一）：`pack` 把「录制产物 + 源 DB 文件」打成便携夹具，
//! `verify` 对夹具目录做零外部依赖的离线复验。
//! 方案：docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md §1。
//!
//! 夹具只入库**最终文件**（zip 单条目）+ manifest + SHA256SUMS；每个关键 sesno 的
//! 历史快照在 pack 时切一遍、散列进还原台账，verify 与阶段三回归再从最终文件
//! **现切**对账——「任意历史可从最终文件精确还原」是被验证的性质，不是假设。
//! 还原到**字节**要多一样东西：E3D 每写一个新会话会就地改前一会话页的几个字（`end` / `claim`）和头页计数，
//! 所以录制时把每个会话时刻的真文件拷一份（`recording.snapshots`），pack 把「真 − 现切」记成台账里的长度 + 字节补丁；
//! 没有快照的 sesno 还是纯现切（`format` 模块注）。
//!
//! Issue #19 的专用实现（`db8000_two_delete_fixture`）保持冻结作回归；通用模块
//! 的重放自检与 pack 往返覆盖在 `tests/db_session_fixture_selfcheck.rs`。
//!
//! ```text
//! db_session_fixture pack --recording <recording.json> --out <夹具目录> [--source <db文件>]
//! db_session_fixture verify --fixture <夹具目录>
//! db_session_fixture replay-init --fixture <夹具目录> | --source <终稿> --to <sesno> --out <工作文件> [--allow-hardlink]
//! db_session_fixture replay-step --fixture <夹具目录> | --source <终稿> --work <工作文件> --to <sesno> [--half pages|header] [--rollback] [--allow-hardlink]
//! ```
//!
//! `replay-*` 是 L2 回放夹具的文件半边（计划 2026-10-25 §3.1）：把工作文件就地推到下一个会话，文件身份不变。
//! 工作文件若还有别的硬链接（影子工程根里漏列进 `-Replayed` 的库就是到真工程原件的硬链接），就地覆写一律拒绝，
//! 除非 `--allow-hardlink`（计划 2026-11-03 TH-1）。

// 回放要读文件的硬链接数（覆写前的护栏），单测还要核「就地追加不换文件」（卷序列号 + 文件索引）：
// Windows 上都走 `windows_by_handle`，与 lib 的 `FrozenSource` 同一个特性门。
#![cfg_attr(windows, feature(windows_by_handle))]

#[path = "db_session_fixture/archive_util.rs"]
mod archive_util;
#[path = "db_session_fixture/format.rs"]
mod format;
#[path = "db_session_fixture/pipeline.rs"]
mod pipeline;
#[path = "db_session_fixture/replay.rs"]
mod replay;
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
    /// L2 回放：从终稿切出会话 `to` 写成工作文件（已存在则就地覆写 = 重置；有别的硬链接则拒绝）
    ReplayInit {
        /// 夹具目录（manifest.json + zip）
        #[arg(long)]
        fixture: Option<PathBuf>,
        /// 或直接给终稿文件
        #[arg(long)]
        source: Option<PathBuf>,
        #[arg(long)]
        to: u32,
        #[arg(long)]
        out: PathBuf,
        /// 工作文件有多个硬链接时仍就地覆写（缺省拒绝：会写穿到真工程的原件）
        #[arg(long, default_value_t = false)]
        allow_hardlink: bool,
    },
    /// L2 回放：把工作文件就地推到会话 `to`（追加页 + 改头，文件身份不变）
    ReplayStep {
        #[arg(long)]
        fixture: Option<PathBuf>,
        #[arg(long)]
        source: Option<PathBuf>,
        #[arg(long)]
        work: PathBuf,
        #[arg(long)]
        to: u32,
        /// 半截：`pages`（只追加页、头不动）或 `header`（只改头、页不动）
        #[arg(long)]
        half: Option<String>,
        /// 往回走（截回更早的会话）要显式说
        #[arg(long, default_value_t = false)]
        rollback: bool,
        /// 工作文件有多个硬链接时仍就地覆写（缺省拒绝：会写穿到真工程的原件）
        #[arg(long, default_value_t = false)]
        allow_hardlink: bool,
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
        Command::ReplayInit {
            fixture,
            source,
            to,
            out,
            allow_hardlink,
        } => {
            let final_bytes = replay::load_final(fixture.as_deref(), source.as_deref())?;
            let hard_links = replay::HardLinks::from_flag(allow_hardlink);
            replay::replay_init(&final_bytes, to, &out, hard_links)?.print();
        }
        Command::ReplayStep {
            fixture,
            source,
            work,
            to,
            half,
            rollback,
            allow_hardlink,
        } => {
            let final_bytes = replay::load_final(fixture.as_deref(), source.as_deref())?;
            let half = half.as_deref().map(replay::Half::parse).transpose()?;
            let hard_links = replay::HardLinks::from_flag(allow_hardlink);
            replay::replay_step(&final_bytes, &work, to, half, rollback, hard_links)?.print();
        }
    }
    Ok(())
}
