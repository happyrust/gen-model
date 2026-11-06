//! 把被选中的元素按 MBD 统一端点（`GET /api/mbd/v2/object/{refno}`，plant3d-web#103 M7）的口径判型、取归属对象，
//! 从 E3D `.dat` 直读导出成 plant-mbd 对象 fixture。产物是 plant-mbd-cli 能直接吃的对象 case
//! （`crates/plant-mbd-cli/src/object_case.rs`、`fixtures/object/stub-cases.json`）：
//!
//! ```json
//! { "$schema_comment": "…",
//!   "cases": [{ "id": "24383_69027", "title": "…", "annotation_type": "em4_supp",
//!               "subject_refno": "24383_69033", "subject_noun": "STRU",
//!               "support": { "subject_refno": "24383_69033", "subject_noun": "STRU", "name": "/1WCC-R412.006" } }] }
//! ```
//!
//! 视图键随类型：`branch`（管道，`branch_query` 的 `BranchQueryResult` 全量，含 M2a / M2b 的成员字段与 BRAN 级
//! `flow_direction` / `heat_tracing` / `dmf_walls`）/ `support` / `civil` / `equipment`。
//! 设备是 `equipment_view` 的全量视图（原点 / 朝向 / 包围盒 / ELCONN / 管口 P1 / 找墙候选，判型说明在 `issues` 最前）；
//! 找墙候选只在 `--db` 钉进来的设计库里找（服务端是 MDB 的全部 DESI 库），设备旁的墙在别的库时把那座库也给上
//! （AMS：设备在 7999 / 8000，墙在 7997）。支架 / 土建现在只有归属（refno / noun / NAME），各 lane 的读取器落地后补字段。
//! 每个 `--refno` 一个 `<w0_w1>.json`（按选中的 refno 命名）。
//!
//! ```powershell
//! cargo run --example mbd_object_dump -- --db D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001 `
//!     --db D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7997_0001 `
//!     --scan-dir D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000 --scan-dir D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000 `
//!     --scan-dir D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000 --refno 24383/69027 --refno 24383/76416 --out-dir target\mbd-object
//! cd ..\vendor\plant-mbd-all; cargo run -p plant-mbd-cli -- layout --fixture <out-dir>\24383_69027.json --case 24383_69027
//! ```
//!
//! 与 `mbd_branch_dump` 的不同：`--db` 可重复（选中的元素可以分在几座设计库里）；`--scan-dir` **按给出的顺序**链式
//! 解析，每个目录单独扫、第一个答得出的为准——dbnum 7000 在 ams000 与 acp000 各有一份，合在一次扫描里会被判成
//! 「分不出正身」整组丢掉。扫到的 DICT 库的 UDA 定义一并挂上（`--no-uda` 关掉）。判型规则取当前目录
//! `DbOption.toml` 的 `[mbd_type_rules]`，没写就是缺省表。模板目录默认取 `AIOS_E3D_TEMPLATE_DIR`，再退到
//! `E:\reverse\e3d\shadow_e3d31_aps_all`。

use std::path::PathBuf;
use std::sync::Arc;

use aios_database::data_interface::direct_store::TEMPLATE_DIR_ENV;
use aios_database::fast_model::e3d_model_service::parse_refno;
use aios_database::mbd::host::ChainedDbResolver;
use aios_database::mbd::object::{MbdTypeRules, SubjectKind, stub_source};
use aios_database::mbd::{branch_query, classify_with, equipment_view, wire_refno};
use clap::Parser;
use e3d_io::db_element::{DbFilePin, DbFileResolver, DbSet, template_file_for};
use e3d_io::uda_catalog::UdaCatalog;
use e3d_model::db_discovery::DirectoryDbResolver;

const TEMPLATE_DIR_DEFAULT: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all";

#[derive(Parser, Debug)]
#[command(
    name = "mbd_object_dump",
    about = "E3D .dat → plant-mbd object fixture（统一端点的判型 + 归属对象视图）"
)]
struct Args {
    /// DESI 库文件（如 ams7999_0001），可重复
    #[arg(long, required = true)]
    db: Vec<PathBuf>,
    /// 目录库所在目录（可重复，按给出的顺序链式解析，第一个答得出的为准）
    #[arg(long = "scan-dir")]
    scan_dirs: Vec<PathBuf>,
    /// 模板目录（attlib.dat / desvir.dat / dicvir.dat …）
    #[arg(long = "template-dir")]
    template_dir: Option<PathBuf>,
    /// 钉住的会话号（作用于每个 --db）；缺省取文件最新会话
    #[arg(long)]
    sesno: Option<u32>,
    /// 选中的元素 refno（`24383/69027` 或 `24383_69027`），可重复
    #[arg(long, required = true)]
    refno: Vec<String>,
    /// 输出目录；每个 refno 一个 `<w0_w1>.json`
    #[arg(long = "out-dir")]
    out_dir: PathBuf,
    /// 不挂 DICT 库的 UDA 定义
    #[arg(long = "no-uda")]
    no_uda: bool,
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
        eprintln!("{}: {}", dir.display(), resolver.summary());
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
        eprintln!("UDA 定义 {} 条", merged.definitions().len());
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
    let mut pinned = Vec::new();
    for file in &args.db {
        let dbno = set.add_db(DbFilePin {
            file: file.clone(),
            template: template_file_for(&templates, "DESI")?,
            db_type: Some("DESI".to_string()),
            sesno: args.sesno,
        })?;
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let sesno = set.pinned_sesno(dbno);
        pinned.push((dbno, name, sesno));
    }
    let set = Arc::new(set);
    let rules = MbdTypeRules::current();
    std::fs::create_dir_all(&args.out_dir)?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    for raw in &args.refno {
        let refno = parse_refno(raw)?;
        let classified = match classify_with(&set, refno, &rules) {
            Ok(classified) => classified,
            Err(error) => {
                eprintln!("{raw}: {error}");
                continue;
            }
        };
        let id = wire_refno(refno);
        let subject_refno = classified.subject_wire_refno();
        let view = match classified.kind {
            SubjectKind::Pipe => match branch_query(&set, classified.subject_refno) {
                Ok(query) => {
                    let real: Vec<&plant_mbd::BranchMember> = query
                        .members
                        .iter()
                        .filter(|member| !member.refno.contains('~'))
                        .collect();
                    let count = |has: fn(&plant_mbd::BranchMember) -> bool| {
                        real.iter().filter(|member| has(member)).count()
                    };
                    eprintln!(
                        "{raw}: BRAN {subject_refno} E3D members {}, vol {}, catalogue_primitives {}, aconn/lconn {}, \
                         ipara {}, navigation {}, dmf_targets {}, dmf_walls {}, issues {}",
                        real.len(),
                        count(|m| m.vol.is_some()),
                        count(|m| !m.catalogue_primitives.is_empty()),
                        count(|m| m.aconn.is_some() && m.lconn.is_some()),
                        count(|m| !m.ipara.is_empty()),
                        count(|m| m.navigation.is_some()),
                        count(|m| !m.dmf_targets.is_empty()),
                        query.dmf_walls.len(),
                        query.issues.len()
                    );
                    serde_json::to_value(query)?
                }
                Err(error) => {
                    eprintln!("{raw}: BRAN {subject_refno}: {error}");
                    continue;
                }
            },
            SubjectKind::Support => serde_json::to_value(&stub_source(&classified).supports[0])?,
            SubjectKind::Civil => serde_json::to_value(&stub_source(&classified).civils[0])?,
            SubjectKind::Equipment => match equipment_view(&set, &classified) {
                Ok(view) => {
                    let nouns: Vec<&str> = view
                        .civil_candidates
                        .iter()
                        .map(|candidate| candidate.noun.as_str())
                        .collect();
                    eprintln!(
                        "{raw}: EQUI {subject_refno} nozzles {} (P1 {}), elconn {}, bbox {}, civil candidates {nouns:?}, issues {}",
                        view.nozzles.len(),
                        view.nozzles.iter().filter(|n| n.p1_pos.is_some()).count(),
                        view.elconn_pos.is_some(),
                        view.bbox.is_some(),
                        view.issues.len()
                    );
                    serde_json::to_value(view)?
                }
                Err(error) => {
                    eprintln!("{raw}: EQUI {subject_refno}: {error}");
                    continue;
                }
            },
        };
        let (db_name, sesno) = pinned
            .iter()
            .find(|(dbno, _, _)| *dbno == refno.dbno())
            .map(|(_, name, sesno)| {
                (
                    name.clone(),
                    sesno
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "latest".into()),
                )
            })
            .unwrap_or_else(|| ("?".into(), "?".into()));
        let title = format!(
            "{} {} {} (selected {raw}; SITE {} › ZONE {}) from {db_name} sesno {sesno}",
            classified.annotation_type.wire_name(),
            classified.subject_noun,
            classified.subject_name.as_deref().unwrap_or(&subject_refno),
            classified.site_name.as_deref().unwrap_or("-"),
            classified.zone_name.as_deref().unwrap_or("-"),
        );
        let mut case = serde_json::json!({
            "id": id,
            "title": title,
            "annotation_type": classified.annotation_type,
            "subject_refno": subject_refno,
            "subject_noun": classified.subject_noun,
        });
        case[classified.kind.view_key()] = view;
        let file = serde_json::json!({
            "$schema_comment": format!(
                "Generated {today} by gen-model `cargo run --example mbd_object_dump -- --db {db_name} --refno {raw}` \
                 (direct .dat read; annotation type and subject from [mbd_type_rules], dbno {} sesno {sesno}). \
                 equipment views are full (civil candidates searched in the --db design databases only); \
                 support / civil views carry only the subject until their readers land. \
                 Do not hand-edit: re-run instead.",
                refno.dbno()
            ),
            "cases": [case],
        });
        let path = args.out_dir.join(format!("{id}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(&file)? + "\n")?;
        println!("{raw} -> {} ({title})", path.display());
    }
    Ok(())
}
