//! 把一条（或几条）真实 BRAN 的成员结点视图从 E3D `.dat` 直读导出成 plant-mbd fixture
//! （计划 `2026-09-06-plant-mbd-pipe-dimension` Phase 3：金样的数据来源）。
//!
//! 求解器不开库（plant-mbd ADR-0004），所以现场 fixture 只能从宿主这边出。产物是 plant-mbd-cli
//! 能直接吃的形状 `{ "$schema_comment", "cases": [{ id, title, members: [SplitMember…] }] }`，
//! 另带一份原始 `branch_query`（含适配层 issues）方便交叉核对。M2a / M2b 的成员字段（目录原语、`vol`、
//! `ipara`、连接类型、导航…）随 `members` 透传；BRAN 级的 `dmf_walls` 有墙时进 case（CLI 认这个键），
//! `flow_direction` / `heat_tracing` 只在 `branch_query` 里（切段成员 case 装不下）。每条 BRAN 另打一行字段覆盖。
//! 之后：
//!
//! ```powershell
//! cargo run --example mbd_branch_dump -- --db D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001 `
//!     --scan-dir D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000 --scan-dir D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000 `
//!     --scan-dir D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000 --refno 24383/99996 --out-dir vendor\shared\plant-mbd\fixtures\field
//! cd vendor\shared\plant-mbd; cargo run -p plant-mbd-cli -- golden record --fixture fixtures/field/24383_99996.json --case 24383_99996 --note "…"
//! ```
//!
//! `--scan-dir` **按给出的顺序**链式解析（与 `mbd_object_dump` 同款）：每个目录单独扫、第一个答得出的为准——dbnum 7000
//! 在 ams000 与 acp000 各有一份，合进一次扫描会被判成「分不出正身」整组丢掉，MDS（acp7320）与 ZDJ 的支架 ATTA 目录也就
//! 跟着读不到。扫到的 DICT 库的 UDA 定义一并挂上（`--no-uda` 关掉）。模板目录默认取 `AIOS_E3D_TEMPLATE_DIR`，再退到
//! `E:\reverse\e3d\shadow_e3d31_aps_all`（与 direct 系测试同款）。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use aios_database::data_interface::direct_store::TEMPLATE_DIR_ENV;
use aios_database::fast_model::e3d_model_service::parse_refno;
use aios_database::mbd::host::ChainedDbResolver;
use aios_database::mbd::{branch_query, wire_refno};
use clap::Parser;
use e3d_io::db_element::{DbFilePin, DbFileResolver, DbSet, template_file_for};
use e3d_io::uda_catalog::UdaCatalog;
use e3d_model::db_discovery::DirectoryDbResolver;
use plant_mbd::branch_to_split_members;

const TEMPLATE_DIR_DEFAULT: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all";

#[derive(Parser, Debug)]
#[command(
    name = "mbd_branch_dump",
    about = "E3D .dat → plant-mbd field fixture（成员结点视图）"
)]
struct Args {
    /// DESI 主库文件（如 ams7999_0001）
    #[arg(long)]
    db: PathBuf,
    /// 目录库所在目录（可重复，按给出的顺序链式解析，第一个答得出的为准），惰性解析 SPRE/CATR 指向的目录库
    #[arg(long = "scan-dir")]
    scan_dirs: Vec<PathBuf>,
    /// 不挂 DICT 库的 UDA 定义
    #[arg(long = "no-uda")]
    no_uda: bool,
    /// 模板目录（attlib.dat / desvir.dat …）
    #[arg(long = "template-dir")]
    template_dir: Option<PathBuf>,
    /// 钉住的会话号；缺省取文件最新会话
    #[arg(long)]
    sesno: Option<u32>,
    /// BRAN refno（`24383/99996` 或 `24383_99996`），可重复
    #[arg(long, required = true)]
    refno: Vec<String>,
    /// 输出目录；每条 BRAN 一个 `<w0_w1>.json`
    #[arg(long = "out-dir")]
    out_dir: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let templates = args
        .template_dir
        .clone()
        .or_else(|| std::env::var_os(TEMPLATE_DIR_ENV).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(TEMPLATE_DIR_DEFAULT));
    anyhow::ensure!(
        templates.join("attlib.dat").is_file(),
        "模板目录 {} 里没有 attlib.dat（--template-dir 或 {TEMPLATE_DIR_ENV}）",
        templates.display()
    );

    let attlib = Arc::new(
        e3d_attlib::AttlibData::parse_file(templates.join("attlib.dat"))
            .map_err(|error| anyhow::anyhow!("parse attlib.dat: {error}"))?,
    );
    let mut resolvers = Vec::with_capacity(args.scan_dirs.len());
    for dir in &args.scan_dirs {
        let resolver = DirectoryDbResolver::scan(std::slice::from_ref(dir), &templates)?;
        for skipped in resolver.skipped() {
            eprintln!("scan 诊断 {}: {}", skipped.file.display(), skipped.reason);
        }
        resolvers.push(resolver);
    }
    let uda = (!args.no_uda && !resolvers.is_empty()).then(|| {
        let mut merged = UdaCatalog::default();
        for resolver in &resolvers {
            let (catalog, unreadable) =
                resolver.uda_catalog(&resolver.dbnos_of_type("DICT"), &templates, &attlib);
            for (dbno, file, error) in unreadable {
                eprintln!("DICT {dbno}（{}）读不开：{error}", file.display());
            }
            merged.merge_in_order(catalog);
        }
        Arc::new(merged)
    });
    let chain = ChainedDbResolver(
        resolvers
            .into_iter()
            .map(|resolver| Box::new(resolver) as Box<dyn DbFileResolver>)
            .collect(),
    );
    let mut set = DbSet::with_resolver(attlib, Box::new(chain)).with_element_cache();
    if let Some(uda) = uda {
        set = set.with_uda_catalog(uda);
    }
    let dbno = set.add_db(DbFilePin {
        file: args.db.clone(),
        template: template_file_for(&templates, "DESI")?,
        db_type: Some("DESI".to_string()),
        sesno: args.sesno,
    })?;
    let sesno = set.pinned_sesno(dbno);
    let set = Arc::new(set);
    std::fs::create_dir_all(&args.out_dir)?;

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let db_name = args
        .db
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    for raw in &args.refno {
        let refno = parse_refno(raw)?;
        let query = match branch_query(&set, refno) {
            Ok(query) => query,
            Err(error) => {
                eprintln!("{raw}: {error}");
                continue;
            }
        };
        let id = wire_refno(refno);
        let mut by_noun: BTreeMap<String, usize> = BTreeMap::new();
        for member in &query.members {
            *by_noun
                .entry(member.noun.clone().unwrap_or_default())
                .or_default() += 1;
        }
        let composition = by_noun
            .iter()
            .map(|(noun, count)| format!("{noun} {count}"))
            .collect::<Vec<_>>()
            .join(", ");
        let members = branch_to_split_members(&query);
        let title = format!(
            "Field BRAN {id} from {db_name} sesno {}: {} members ({composition}), {} adapter issue(s)",
            sesno
                .map(|s| s.to_string())
                .unwrap_or_else(|| "latest".into()),
            query.members.len(),
            query.issues.len()
        );
        let mut case = serde_json::json!({
            "id": id,
            "title": title,
            "members": members,
            // 管道名称标签（plant-mbd ADR-0009）要的 BRAN 级数据，切段成员表里装不下。
            "branch_name": query.name,
            "branch_label_bore": query.label_bore_text,
        });
        if !query.dmf_walls.is_empty() {
            case["dmf_walls"] = serde_json::to_value(&query.dmf_walls)?;
        }
        let coverage = field_coverage(&query);
        let file = serde_json::json!({
            "$schema_comment": format!(
                "Generated {today} by gen-model `cargo run --example mbd_branch_dump -- --db {} --refno {raw}` \
                 (direct .dat read: e3d-model walk_route + CataloguePoints, dbno {dbno} sesno {}). Do not hand-edit: re-run instead.",
                db_name,
                sesno.map(|s| s.to_string()).unwrap_or_else(|| "latest".into())
            ),
            "cases": [case],
            "branch_query": query,
        });
        let path = args.out_dir.join(format!("{id}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(&file)? + "\n")?;
        println!("{raw} -> {} ({})", path.display(), title);
        println!("  M2a/M2b fields: {coverage}");
    }
    Ok(())
}

/// 一行字段覆盖：E3D 成员（不含合成的隐式管身）里各字段有值的个数，BRAN 级字段有没有。
fn field_coverage(query: &plant_mbd::BranchQueryResult) -> String {
    let real: Vec<&plant_mbd::BranchMember> = query
        .members
        .iter()
        .filter(|member| !member.refno.contains('~'))
        .collect();
    let count = |has: fn(&plant_mbd::BranchMember) -> bool| real.iter().filter(|m| has(m)).count();
    let primitives: usize = real.iter().map(|m| m.catalogue_primitives.len()).sum();
    format!(
        "{} E3D members: vol {}, catalogue_primitives {} ({primitives} primitives), para {}, desp {}, \
         spref_name {}, cate_name {}, gmref_name {}, aconn {}, lconn {}, p3_conn {}, ipara {}, navigation {}, \
         ppoint bores {}, P4-P6/P101 {}, dmf_targets {}, ATTA rpro {}, prop_bang {}, atta_adlongth {}; \
         BRAN flow_direction {:?}, heat_tracing {:?}, dmf_walls {}",
        real.len(),
        count(|m| m.vol.is_some()),
        count(|m| !m.catalogue_primitives.is_empty()),
        count(|m| !m.para.is_empty()),
        count(|m| !m.desp.is_empty()),
        count(|m| m.spref_name.is_some()),
        count(|m| m.cate_name.is_some()),
        count(|m| m.gmref_name.is_some()),
        count(|m| m.aconn.is_some()),
        count(|m| m.lconn.is_some()),
        count(|m| m.p3_conn.is_some()),
        count(|m| !m.ipara.is_empty()),
        count(|m| m.navigation.is_some()),
        count(|m| m.ppoints.iter().any(|p| p.bore.is_some())),
        count(|m| m.ppoints.iter().any(|p| matches!(p.number, 4..=6 | 101))),
        count(|m| !m.dmf_targets.is_empty()),
        count(|m| m.rpro_dod.is_some() || m.rpro_dthi.is_some()),
        count(|m| m.prop_bang.is_some()),
        count(|m| m.atta_adlongth.is_some()),
        query.flow_direction,
        query.heat_tracing,
        query.dmf_walls.len(),
    )
}
