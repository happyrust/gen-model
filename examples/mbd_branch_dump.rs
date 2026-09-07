//! 把一条（或几条）真实 BRAN 的成员结点视图从 E3D `.dat` 直读导出成 plant-mbd fixture
//! （计划 `2026-09-06-plant-mbd-pipe-dimension` Phase 3：金样的数据来源）。
//!
//! 求解器不开库（plant-mbd ADR-0004），所以现场 fixture 只能从宿主这边出。产物是 plant-mbd-cli
//! 能直接吃的形状 `{ "$schema_comment", "cases": [{ id, title, members: [SplitMember…] }] }`，
//! 另带一份原始 `branch_query`（含适配层 issues）方便交叉核对；之后：
//!
//! ```powershell
//! cargo run --example mbd_branch_dump -- --db D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001 `
//!     --scan-dir D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000 --refno 24383/99996 `
//!     --out-dir ..\vendor\plant-mbd\fixtures\field
//! cd ..\vendor\plant-mbd; cargo run -p plant-mbd-cli -- golden record --fixture fixtures/field/24383_99996.json --case 24383_99996 --note "…"
//! ```
//!
//! 模板目录默认取 `AIOS_E3D_TEMPLATE_DIR`，再退到 `E:\reverse\e3d\shadow_e3d31_aps_all`（与 direct 系测试同款）。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use aios_database::data_interface::direct_store::TEMPLATE_DIR_ENV;
use aios_database::fast_model::e3d_model_service::parse_refno;
use aios_database::mbd::{branch_query, wire_refno};
use clap::Parser;
use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
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
    /// 目录库所在目录（可重复），按页 0 自报库号惰性解析 SPRE/CATR 指向的目录库
    #[arg(long = "scan-dir")]
    scan_dirs: Vec<PathBuf>,
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

    let set = if args.scan_dirs.is_empty() {
        DbSet::with_attlib_file(templates.join("attlib.dat"))?.with_element_cache()
    } else {
        let resolver = DirectoryDbResolver::scan(&args.scan_dirs, &templates)?;
        for skipped in resolver.skipped() {
            eprintln!("scan 诊断 {}: {}", skipped.file.display(), skipped.reason);
        }
        DbSet::with_attlib_file_and_resolver(templates.join("attlib.dat"), Box::new(resolver))?
            .with_element_cache()
    };
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
        let file = serde_json::json!({
            "$schema_comment": format!(
                "Generated {today} by gen-model `cargo run --example mbd_branch_dump -- --db {} --refno {raw}` \
                 (direct .dat read: e3d-model walk_route + CataloguePoints, dbno {dbno} sesno {}). Do not hand-edit: re-run instead.",
                db_name,
                sesno.map(|s| s.to_string()).unwrap_or_else(|| "latest".into())
            ),
            "cases": [{ "id": id, "title": title, "members": members }],
            "branch_query": query,
        });
        let path = args.out_dir.join(format!("{id}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(&file)? + "\n")?;
        println!("{raw} -> {} ({})", path.display(), title);
    }
    Ok(())
}
