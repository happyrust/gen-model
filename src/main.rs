#![feature(let_chains)]
#![feature(duration_constructors)]
// 暂时屏蔽warnings
#![allow(warnings)]
#![recursion_limit = "256"]

#[macro_use]
extern crate clap;
#[macro_use]
extern crate nom;

extern crate strum;

use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
use std::fs;
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use aios_core::SUL_DB;
use aios_core::material::save_all_material_data;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::ssc_setting::{
    set_pbs_fixed_node, set_pbs_node, set_pbs_room_major_node, set_pbs_room_node,
    set_pdms_major_code,
};
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use aios_core::{build_cate_relate, get_db_option};
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::fast_model::cal_model::{update_cal_bran_component, update_cal_equip};
#[cfg(feature = "gen_model")]
use aios_database::fast_model::gen_all_geos_data;
use aios_database::fast_model::room_model::build_room_relations;
use aios_database::fast_model::{
    EXIST_MESH_GEO_HASHES, gen_inst_meshes, process_meshes_update_db_deep,
};
use aios_database::team_data::sync_team_data;
use aios_database::versioned_db::database::*;
use aios_database::{run_app, run_cli};
use bevy_reflect::List;
use chrono::{Datelike, Local, Timelike};
use futures::StreamExt;
use itertools::Itertools;
use log::{LevelFilter, error};
use simplelog::*;
use surrealdb::opt::auth::Root;

/// 增量批次把 `execute_frozen_batch_body` 连同两层 `task_local` scope 一起内联
/// await（`batch_worker.rs` 的 `window.scope(...)`）。debug 版不做 async 状态机
/// 布局优化，这条复合 future 加上各层 poll 帧超过 std 默认的 2MB 线程栈，表现为
/// `thread 'tokio-rt-worker' has overflowed its stack`——服务能起、/health 全绿，
/// 直到真有增量要应用才当场死。release 压得下，所以 `#[tokio::main]` 一直没出事。
///
/// 取值对齐 `fast_model/mesh_generate.rs` 给 `gensec-occ-regression` 线程的 64MB：
/// 栈是保留地址空间、按页提交，多留不花实际内存。
const RUNTIME_STACK_SIZE: usize = 64 * 1024 * 1024;

/// 增量服务的命令行面。
///
/// **无子命令 = 起服务**。仓内所有脚本、`l3_suite` 夹具与部署包都是裸调这个
/// 二进制的，这条默认行为是硬约束，不是便利。
#[derive(clap::Parser)]
#[command(
    name = "aios-database",
    version,
    about = "AVEVA E3D 增量解析与几何生成服务"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// 起增量服务（不带任何子命令时的默认行为）。
    Serve(ServeArgs),
    /// 取本机服务的增量链路追踪。纯客户端，走 HTTP，不拿实例锁。
    ///
    /// 不拿锁是关键：`run_app` 一上来就 `acquire_process_instance_lock`，若这个
    /// 子命令也走那条路，服务跑着时它根本执行不了——而那正是唯一想用它的时候。
    Trace(TraceArgs),
}

#[derive(clap::Args, Default)]
struct ServeArgs {
    /// **调试用**：把本轮数据批次的检查圈到这些 dbnum，并全程追踪它们。
    ///
    /// 逗号分隔（`7998,8000`）。SYS meta 不受限——MDB 的成员名单存在那些库里。
    /// 这不是运行策略，所以它只能从命令行来，进不了配置文件。开启后 `/health`、
    /// 每一份 preview / execute 回执都会明说本进程是跛的。
    #[arg(long, value_name = "N[,N...]")]
    debug_dbnum: Option<String>,

    /// **调试用**：把增量摄入的数据批次圈到这些 dbnum，不带追踪。
    ///
    /// 逗号分隔（`7998,8000`）。SYS meta 不受限——MDB 的成员名单存在那些库里。
    /// 压过 `DbOption.toml` 的 `watch_dbnums`；与 `--debug-dbnum` 各管各的，两个
    /// 都给就两道门都要过。开启后启动横幅、`/health`、每一份 preview / execute
    /// 回执都会明说本进程的监听范围被收窄了。
    #[arg(long, value_name = "N[,N...]")]
    watch_dbnum: Option<String>,
}

#[derive(clap::Args)]
struct TraceArgs {
    /// 只看这一个 dbnum；不给就全都要。
    #[arg(long)]
    dbnum: Option<u32>,
    /// 最多取多少条（取最新的）；0 表示不限。
    #[arg(long, default_value_t = 0)]
    limit: usize,
    /// 服务地址。缺省取配置里的 `server_release_ip`。
    #[arg(long)]
    url: Option<String>,
}

fn main() -> anyhow::Result<()> {
    use clap::Parser;

    let cli = Cli::parse();
    let serve = match cli.command {
        Some(Command::Trace(args)) => return fetch_trace(&args),
        Some(Command::Serve(args)) => args,
        None => ServeArgs::default(),
    };
    if let Some(raw) = serve.debug_dbnum.as_deref() {
        // 拼错的取值直接失败，不回落。悄悄吞成「全范围」看起来像参数没生效，吞成
        // 「空集」看起来像什么都没跑，两种都要人再花一轮才发现是自己手误。
        let dbnums = aios_database::data_interface::debug_scope::parse_dbnums(raw)
            .map_err(|message| anyhow::anyhow!(message))?;
        println!(
            "*** 调试限定模式：数据批次只处理 dbnum {dbnums:?}，SYS meta 不受限。\
             这不是正常运行状态。***"
        );
        aios_database::data_interface::debug_scope::set_dbnums(dbnums);
    }
    if let Some(raw) = serve.watch_dbnum.as_deref() {
        // 同样不回落（理由同上）。横幅不在这里打：配置里写的 `watch_dbnums` 也要
        // 有同一句声明，两个来源共用 `run_cli` 那一处出口才不会漏掉配置那条路。
        let dbnums = aios_database::data_interface::watch_scope::parse_dbnums(raw)
            .map_err(|message| anyhow::anyhow!(message))?;
        aios_database::data_interface::watch_scope::set_cli_dbnums(dbnums);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(RUNTIME_STACK_SIZE)
        .build()?
        .block_on(run_app(None))
}

/// 走 curl 而不是引一个 HTTP 客户端依赖：仓内已有先例
/// （`l3_suite` 的健康等待、`fixture.rs` 的服务探活都用它），为一个诊断子命令
/// 往服务二进制里塞一整套 TLS 栈不划算。
///
/// Windows 上必须点名 `curl.exe`（PowerShell 的 `curl` 是 Invoke-WebRequest
/// 别名，虽然 CreateProcess 不走别名，点名后缀是仓内既有惯例）；部署目标
/// CentOS 7 上没有 `.exe`，写死会让这个子命令在服务器上必挂（2026-08-18 审核 P3）。
#[cfg(windows)]
const CURL: &str = "curl.exe";
#[cfg(not(windows))]
const CURL: &str = "curl";

fn fetch_trace(args: &TraceArgs) -> anyhow::Result<()> {
    let base = args.url.clone().unwrap_or_else(|| {
        let endpoint = get_db_option().server_release_ip.clone();
        if endpoint.starts_with("http") {
            endpoint
        } else {
            format!("http://{endpoint}")
        }
    });
    let mut url = format!(
        "{}/api/v1/trace?limit={}",
        base.trim_end_matches('/'),
        args.limit
    );
    if let Some(dbnum) = args.dbnum {
        url.push_str(&format!("&dbnum={dbnum}"));
    }
    let output = std::process::Command::new(CURL)
        .args(["--silent", "--show-error", "--fail", &url])
        .output()
        .map_err(|e| anyhow::anyhow!("调用 {CURL} 失败: {e}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "取追踪失败（{}）：{}\n服务没起、或它不是带 http_api 的构建时会走到这里。",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    std::io::stdout().write_all(&output.stdout)?;
    println!();
    Ok(())
}

#[test]
fn get_noun_hash() {
    let noun = "DB";
    let hash = db1_hash(noun);
    dbg!(hash);
    let hashes = [0xE567E, 640481, 919399];
    for hash in hashes {
        let str = db1_dehash(hash);
        dbg!(&hash);
        dbg!(str);
    }
}

#[test]
fn test_time() {
    use chrono::prelude::*;
    let local: DateTime<Local> = Local::now();
    println!(
        "year:{} , month: {} , day: {}, week_day:{},hour:{} , min: {} , sec:{}",
        local.year(),
        local.month(),
        local.day(),
        local.weekday(),
        local.hour(),
        local.minute(),
        local.second()
    );
}

/// 将 all_attr_info.bin 文件转成 json
#[test]
fn test_turn_bin_into_json() {
    let mut file = File::open("all_attr_info.bin").unwrap();
    let mut data = vec![];
    file.read_to_end(&mut data).unwrap();
    let map = bincode::deserialize::<PdmsDatabaseInfo>(&data).unwrap();
    let json = serde_json::to_string(&map).unwrap();
    let mut new_file = File::create("all_attr_info_1.json").unwrap();
    new_file.write_all(&json.into_bytes()).unwrap();
}

#[cfg(test)]
mod tests {
    use aios_core::geometry::ShapeInstancesData;
    use aios_core::options::DbOption;
    use aios_core::pdms_types::{CataHashRefnoKV, RefnoEnum};
    use aios_core::pe::SPdmsElement;
    use aios_database::fast_model::cata_model::gen_cata_geos_with_tracing;
    use dashmap::DashMap;
    use flume::unbounded;
    use glam::Vec3;
    use std::sync::Arc;

    #[tokio::test]
    #[cfg(feature = "profile")]
    async fn test_gen_cata_geos_with_tracing() {
        println!("Starting gen_cata_geos tracing test with profile feature enabled");

        // Initialize sample data for testing
        let db_option = Arc::new(DbOption::default());
        let target_cata_map = Arc::new(DashMap::new());

        // Add some sample data to test with
        // This is just an example - you'll need to replace with real data for your test
        target_cata_map.insert(
            "sample_hash".to_string(),
            CataHashRefnoKV {
                cata_hash: "sample_hash".to_string(),
                group_refnos: vec![RefnoEnum::default()],
                exist_inst: false,
                ptset: None,
            },
        );

        let branch_map = Arc::new(DashMap::new());
        let sjus_map_arc = Arc::new(DashMap::new());

        // Create a channel to receive shape instances data
        let (sender, receiver) = unbounded();

        // Run gen_cata_geos with tracing enabled
        let result = gen_cata_geos_with_tracing(
            db_option,
            target_cata_map,
            branch_map,
            sjus_map_arc,
            sender,
        )
        .await;

        println!("gen_cata_geos result: {:?}", result);

        // For testing purposes, drain the receiver
        while let Ok(_) = receiver.try_recv() {}

        println!("Trace file generated at chrome_trace_cata_model.json");
        println!("You can open this file in Chrome at chrome://tracing");
    }

    #[tokio::test]
    #[cfg(not(feature = "profile"))]
    async fn test_gen_cata_geos_with_tracing() {
        println!("Starting gen_cata_geos tracing test without profile feature");
        println!("Note: For full tracing functionality, enable the 'profile' feature");

        // Initialize minimal test data
        let db_option = Arc::new(DbOption::default());
        let target_cata_map = Arc::new(DashMap::new());
        let branch_map = Arc::new(DashMap::new());
        let sjus_map_arc = Arc::new(DashMap::new());
        let (sender, _) = unbounded();

        // Run gen_cata_geos with tracing disabled
        let result = gen_cata_geos_with_tracing(
            db_option,
            target_cata_map,
            branch_map,
            sjus_map_arc,
            sender,
        )
        .await;

        println!("gen_cata_geos result: {:?}", result);
    }
}
