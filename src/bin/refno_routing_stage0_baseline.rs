//! AMS 阶段 0 基线：改路由**之前**的旧成本一次量完，写成与 JEU 同 schema 的 JSON。
//!
//! 跨库属性切片 W0-d / 懒加载切片 L0（`docs/plans/2026-09-12-lazy-startup-dev-plan.md` §4）。
//! 量的是四段，各自 `read_stats::reset()` 之后再读一次 `snapshot()`，差值就是那一段的读成本：
//!
//! 1. **成员清单**：`mdb_membership::resolve`（SYS / CURD + 文件头）；
//! 2. **旧骨架**：`mdb_memory_store::ensure`——启动链今天仍在建的那份全 MDB 元素表，
//!    `scanned_elements` 就是它扫过的元素头数，`load_ms` 是它自己记的装载墙钟；
//! 3. **首属性**：冷的 `DirectStore`（定位器 = `MdbRouteDirectory`）读探针 BRAN 的属性；
//! 4. **首根**：`model_source::root_source_version`（位解码定库 + 目标库最新会话）。
//!
//! RSS 在骨架之前、之后与收尾各读一次（sysinfo）。只读文件，不连 Surreal，不写库。
//! 探针元素默认 `24384/23257` = `/C-OR-1R345-C`（`scripts/e3d/db8000_bran_probe.mac` 用的那根 BRAN）。
//!
//! ```text
//! cargo run --release --bin refno_routing_stage0_baseline -- --json docs/baselines/2026-09-12-ams-refno-routing-stage0.json
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use aios_core::RefU64;
use aios_database::data_interface::direct_store::{DirectSchema, DirectStore};
use aios_database::data_interface::mdb_route_directory::MdbRouteDirectory;
use aios_database::data_interface::{mdb_membership, mdb_memory_store, model_source};
use clap::Parser;
use e3d_io::read_stats::{self, ReadStats};
use e3d_io::refno::RefNo;

#[derive(Parser, Debug)]
#[command(name = "refno_routing_stage0_baseline")]
#[command(about = "AMS 阶段 0 基线：旧骨架 / 首属性 / 首根的读成本与 RSS，写成 JSON")]
struct Args {
    /// 项目名；不给用 DbOption 的 project_name。
    #[arg(long)]
    project: Option<String>,
    /// MDB 名；不给用 DbOption 的 mdb_name。
    #[arg(long)]
    mdb: Option<String>,
    /// 探针元素 `word0/word1`；默认 db8000 探针宏那根 BRAN。
    #[arg(long, default_value = "24384/23257")]
    probe_refno: String,
    /// 写出 JSON 的路径；不给只打印。
    #[arg(long)]
    json: Option<PathBuf>,
}

fn rss_bytes(system: &mut sysinfo::System) -> u64 {
    let pid = sysinfo::Pid::from_u32(std::process::id());
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    system
        .process(pid)
        .map(|process| process.memory())
        .unwrap_or(0)
}

fn parse_refno(text: &str) -> anyhow::Result<RefNo> {
    let (word0, word1) = text
        .split_once(['/', '_'])
        .ok_or_else(|| anyhow::anyhow!("探针 refno 要写成 word0/word1：{text}"))?;
    Ok(RefNo::new(word0.trim().parse()?, word1.trim().parse()?))
}

/// 一段读成本的差值（`reset()` 之后的 `snapshot()`）。
fn cost(stats: &ReadStats) -> serde_json::Value {
    serde_json::json!({
        "database_opens": stats.database_opens,
        "scan_calls": stats.scan_calls,
        "scanned_elements": stats.scanned_elements,
        "index_point_lookups": stats.index_point_lookups,
        "attr_lookups": stats.attr_lookups,
        "parse_misses": stats.parse_misses,
        "extract_misses": stats.extract_misses,
    })
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let option = aios_core::get_db_option();
    let project = args
        .project
        .clone()
        .unwrap_or_else(|| option.project_name.clone());
    let mdb = args.mdb.clone().unwrap_or_else(|| option.mdb_name.clone());
    let probe = parse_refno(&args.probe_refno)?;
    let probe_u64 = RefU64::from_two_nums(probe.word0, probe.word1);

    let mut system = sysinfo::System::new();
    let rss_start = rss_bytes(&mut system);

    // 1. 成员清单
    read_stats::reset();
    let started = Instant::now();
    let membership = Arc::new(mdb_membership::resolve(option, &project, &mdb)?);
    let membership_ms = started.elapsed().as_millis();
    let membership_cost = read_stats::snapshot();
    let declared = membership.databases().len();
    let without_file = membership
        .databases()
        .iter()
        .filter(|db| db.path.is_none())
        .count();
    let element_dbs = membership.element_databases().count();
    println!(
        "[baseline] 成员清单 {} / {}：CURD {declared} 条，无文件 {without_file} 条，元素库 {element_dbs} 个，{membership_ms} ms，{}",
        membership.mdb(),
        membership.project(),
        membership_cost.summary()
    );

    // 2. 旧骨架（启动链今天仍在建的那份）
    let rss_before_skeleton = rss_bytes(&mut system);
    read_stats::reset();
    let started = Instant::now();
    let skeleton = mdb_memory_store::ensure(&project, &mdb)?;
    let skeleton_wall_ms = started.elapsed().as_millis();
    let skeleton_cost = read_stats::snapshot();
    let rss_after_skeleton = rss_bytes(&mut system);
    let stats = skeleton.stats();
    println!(
        "[baseline] 旧骨架 {} 库 / {} 元素（读不出记录 {}），自记 load_ms={} 墙钟 {skeleton_wall_ms} ms，RSS {:.1} → {:.1} MiB，{}",
        stats.databases,
        stats.elements,
        stats.unreadable_records,
        stats.load_ms,
        rss_before_skeleton as f64 / 1048576.0,
        rss_after_skeleton as f64 / 1048576.0,
        skeleton_cost.summary()
    );

    // 3. 首属性：冷 DirectStore + 路由目录（与属性服务同一造法，只是不带依赖目录）
    read_stats::reset();
    let started = Instant::now();
    let schema = Arc::new(DirectSchema::open_from_env()?);
    let directory = MdbRouteDirectory::from_membership(&membership, schema.template_dir());
    let store = DirectStore::new(schema, Arc::new(directory));
    let attrs = store.attrs(probe_u64)?;
    let first_attribute_ms = started.elapsed().as_millis();
    let first_attribute_cost = read_stats::snapshot();
    println!(
        "[baseline] 首属性 {probe}：{} 个属性，{first_attribute_ms} ms，{}",
        attrs.map.map.len(),
        first_attribute_cost.summary()
    );

    // 4. 首根：位解码定库 + 目标库最新会话
    read_stats::reset();
    let started = Instant::now();
    let version = model_source::root_source_version(probe)?;
    let first_root_ms = started.elapsed().as_millis();
    let first_root_cost = read_stats::snapshot();
    println!(
        "[baseline] 首根 {probe}：dbnum {} {} sesno {}，{first_root_ms} ms，{}",
        version.dbnum,
        version.db_type,
        version.sesno,
        first_root_cost.summary()
    );

    let rss_end = rss_bytes(&mut system);
    std::hint::black_box(&skeleton);

    let doc = serde_json::json!({
        "schema": "gen-model/refno-native-stage0-baseline/v1",
        "project": membership.project(),
        "mdb": membership.mdb(),
        "captured_from": {
            "kind": "local-probe",
            "date": chrono::Local::now().format("%Y-%m-%d").to_string(),
            "tool": "src/bin/refno_routing_stage0_baseline.rs",
            "probe_refno": format!("{}/{}", probe.word0, probe.word1),
        },
        "runtime": {
            "e3d_version": aios_database::options::get_db_option_ext().e3d_version,
            "project_path": option.project_path,
            "store_mode": aios_database::options::store_mode().as_str(),
            "room_membership": aios_database::options::room_membership(),
            "watch_dbnums": aios_database::options::watch_dbnums(),
        },
        "membership": {
            "raw_curd_rows": declared,
            "decoded_databases": declared,
            "skipped_databases": without_file,
            "design_or_isod_databases": element_dbs,
            "problems": membership.problems(),
            "elapsed_ms": membership_ms,
            "read_cost": cost(&membership_cost),
        },
        "legacy_skeleton": {
            "databases": stats.databases,
            "elements": stats.elements,
            "unreadable_records": stats.unreadable_records,
            "elapsed_ms_approx": stats.load_ms,
            "wall_ms": skeleton_wall_ms,
            "database_opens": skeleton_cost.database_opens,
            "scan_calls": skeleton_cost.scan_calls,
            "scanned_elements": skeleton_cost.scanned_elements,
            "index_point_lookups": skeleton_cost.index_point_lookups,
            "rss_before_bytes": rss_before_skeleton,
            "rss_after_bytes": rss_after_skeleton,
            "rss_delta_bytes": rss_after_skeleton.saturating_sub(rss_before_skeleton),
        },
        "process_rss_bytes": rss_end,
        "process_rss_start_bytes": rss_start,
        "first_attribute_ms": first_attribute_ms,
        "first_attribute": {
            "attributes": attrs.map.map.len(),
            "read_cost": cost(&first_attribute_cost),
        },
        "first_generation_root_ms": first_root_ms,
        "first_generation_root": {
            "dbnum": version.dbnum,
            "db_type": version.db_type,
            "sesno": version.sesno,
            "read_cost": cost(&first_root_cost),
        },
        "limitations": [
            "首属性 / 首根在骨架已建之后量，量的是冷 DirectStore / 冷定库的成本，不含 HTTP 与 provider 构造。",
            "旧骨架的 RSS 差值含分配器尚未归还系统的部分；与 mdb_memory_probe --tier header 的口径相同。",
            "mdb_membership::resolve 不登记进程内注册表，所以旧骨架段（ensure）与首根段（current_mdb_sources）各自又解了一次成员清单：\
             两段的读成本里都多含一次 SYS 库扫描（scan_calls +1、scanned_elements +1207）与它的开库数；服务进程里成员清单只解一次。",
        ],
    });
    if let Some(path) = &args.json {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, format!("{}\n", serde_json::to_string_pretty(&doc)?))?;
        println!("[baseline] 已写出 {}", path.display());
    } else {
        println!("{}", serde_json::to_string_pretty(&doc)?);
    }
    Ok(())
}
