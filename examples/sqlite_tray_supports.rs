/// 仅基于SQLite空间索引，检测目标SCTN的支撑（无需GRPC/数据库）
///
/// 原理：
/// - 从SQLite R-Tree获取目标AABB
/// - 邻域检索候选AABB
/// - 依据几何关系判断“支撑”：
///   1) 候选的顶部与桥架底部在容差内对齐（|support.maxY - tray.minY| <= tol）
///   2) 在X与Z轴存在水平投影重叠
/// - 若items表记录了noun（类型），会同时输出，用于人工校验SUPPO等
///
/// 运行:
///  cargo run --example sqlite_tray_supports --features sqlite-index -- \
///    --target 24383/86525 --radius 2.0 --tol 0.10 --limit 200 --index aabb_cache.sqlite --export
use anyhow::{Result, anyhow};
use clap::{Arg, Command};
use std::path::PathBuf;

use aios_core::pdms_types::RefU64;
use aios_database::spatial_index::SqliteSpatialIndex;
use nalgebra::{Point3, Vector3};
use parry3d::bounding_volume::Aabb;

fn parse_refno(s: &str) -> Result<RefU64> {
    use std::str::FromStr;
    RefU64::from_str(s).map_err(|_| anyhow!("无效RefNo: {}", s))
}

fn expand_aabb(aabb: &Aabb, r: f32) -> Aabb {
    let mins = aabb.mins - Vector3::new(r, r, r);
    let maxs = aabb.maxs + Vector3::new(r, r, r);
    Aabb::new(mins, maxs)
}

fn detect_support(tray: &Aabb, support: &Aabb, tol: f32) -> bool {
    let vertical_gap = (tray.mins.y - support.maxs.y).abs();
    if vertical_gap > tol {
        return false;
    }
    let x_overlap = tray.maxs.x > support.mins.x && tray.mins.x < support.maxs.x;
    let z_overlap = tray.maxs.z > support.mins.z && tray.mins.z < support.maxs.z;
    x_overlap && z_overlap
}

#[tokio::main]
async fn main() -> Result<()> {
    let m = Command::new("SQLite Tray Supports")
        .version("0.1")
        .about("基于SQLite R-Tree，检测SCTN的支撑（几何法）")
        .arg(
            Arg::new("target")
                .long("target")
                .required(true)
                .help("目标SCTN，例如 24383/86525"),
        )
        .arg(
            Arg::new("radius")
                .long("radius")
                .default_value("2.0")
                .help("邻域半径(米)"),
        )
        .arg(
            Arg::new("tol")
                .long("tol")
                .default_value("0.10")
                .help("垂直对齐容差(米)"),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .default_value("200")
                .help("最多检查邻居数量"),
        )
        .arg(
            Arg::new("index")
                .long("index")
                .required(false)
                .help("索引路径，默认 aabb_cache.sqlite"),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .action(clap::ArgAction::SetTrue)
                .help("以 JSON 输出结果（便于脚本处理）"),
        )
        .arg(
            Arg::new("filter-noun")
                .long("filter-noun")
                .required(false)
                .help("仅保留 items.noun 含此关键字的支撑（大小写不敏感）"),
        )
        .arg(
            Arg::new("export")
                .long("export")
                .action(clap::ArgAction::SetTrue)
                .help("导出到 test_output/sqlite_tray_supports.html/obj"),
        )
        .get_matches();

    let target = parse_refno(m.get_one::<String>("target").unwrap())?;
    let radius: f32 = m
        .get_one::<String>("radius")
        .unwrap()
        .parse()
        .unwrap_or(2.0);
    let tol: f32 = m.get_one::<String>("tol").unwrap().parse().unwrap_or(0.10);
    let limit: usize = m.get_one::<String>("limit").unwrap().parse().unwrap_or(200);

    // 若未启用 Sqlite RTree 或未在配置中打开，给出友好提示
    if !SqliteSpatialIndex::is_enabled() {
        eprintln!(
            "未启用 sqlite-index 或配置未打开(enable_sqlite_rtree=false)。\n请在 Cargo features 启用 `sqlite-index`，并在 DbOption.toml 设置 enable_sqlite_rtree=true。"
        );
        return Ok(());
    }

    let index_path = m
        .get_one::<String>("index")
        .map(PathBuf::from)
        .unwrap_or_else(|| SqliteSpatialIndex::default_path());
    let index = SqliteSpatialIndex::new(&index_path)?;

    let tb = index
        .get_aabb(target)?
        .ok_or_else(|| anyhow!("索引缺少目标SCTN: {}", target.0))?;
    println!(
        "目标SCTN {} BBox: ({:.3},{:.3},{:.3})-({:.3},{:.3},{:.3})",
        target.0, tb.mins.x, tb.mins.y, tb.mins.z, tb.maxs.x, tb.maxs.y, tb.maxs.z
    );

    let query = expand_aabb(&tb, radius);
    let mut neigh = index.query_intersect(&query)?;
    neigh.retain(|r| *r != target);
    if neigh.len() > limit {
        neigh.truncate(limit);
    }

    // 尝试读取items表以获得noun（类型）
    let mut noun_map = std::collections::HashMap::new();
    #[cfg(feature = "sqlite-index")]
    {
        use rusqlite::Connection;
        if let Ok(conn) = Connection::open(&index_path) {
            let query = "SELECT id, noun FROM items WHERE id IN (".to_string()
                + &neigh
                    .iter()
                    .map(|r| (r.0 as i64).to_string())
                    .collect::<Vec<_>>()
                    .join(",")
                + ")";
            let mut stmt = conn
                .prepare(&query)
                .unwrap_or_else(|_| conn.prepare("SELECT id, noun FROM items").unwrap());
            let rows = stmt
                .query_map([], |row| {
                    let id: i64 = row.get(0)?;
                    let noun: String = row.get(1)?;
                    Ok((id as u64, noun))
                })
                .unwrap();
            for r in rows {
                if let Ok((id, noun)) = r {
                    noun_map.insert(id, noun);
                }
            }
        }
    }

    // 筛选支撑
    let mut supports: Vec<(RefU64, Aabb, String)> = Vec::new();
    for r in neigh {
        if let Some(b) = index.get_aabb(r)? {
            if detect_support(&tb, &b, tol) {
                let noun = noun_map.get(&r.0).cloned().unwrap_or_else(|| "".into());
                supports.push((r, b, noun));
            }
        }
    }

    // 可选：按 noun 过滤
    if let Some(pat) = m.get_one::<String>("filter-noun") {
        let key = pat.to_lowercase();
        supports.retain(|(_, _, n)| n.to_lowercase().contains(&key));
    }

    // 输出：文本或 JSON
    if m.get_flag("json") {
        let target_bbox = serde_json::json!({
            "mins": [tb.mins.x, tb.mins.y, tb.mins.z],
            "maxs": [tb.maxs.x, tb.maxs.y, tb.maxs.z]
        });
        let items: Vec<_> = supports
            .iter()
            .map(|(id, bb, noun)| {
                let c = bb.center();
                serde_json::json!({
                    "refno": id.0,
                    "noun": noun,
                    "center": [c.x, c.y, c.z],
                    "max_y": bb.maxs.y,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "target": target.0,
                "target_bbox": target_bbox,
                "count": items.len(),
                "supports": items
            })
            .to_string()
        );
    } else {
        println!("\n检测到支撑: {} 个", supports.len());
        for (id, bb, noun) in &supports {
            println!(
                "  ✓ {} [{}] at ({:.3},{:.3},{:.3})",
                id.0,
                noun,
                bb.center().x,
                bb.center().y,
                bb.center().z
            );
        }
    }

    if m.get_flag("export") {
        // 仅输出目标和支撑点标记
        #[cfg(feature = "grpc")]
        use aios_database::grpc_service::sctn_contact_detector::{
            CableTraySection, ContactResult, ContactType,
        };
        #[cfg(feature = "grpc")]
        use aios_database::grpc_service::sctn_visualizer::SctnVisualizer;
        std::fs::create_dir_all("test_output").ok();
        #[cfg(feature = "grpc")]
        {
            let vis = SctnVisualizer::new("test_output");
            let ext = tb.maxs - tb.mins;
            let sctn = CableTraySection {
                refno: target,
                bbox: tb.clone(),
                centerline: vec![tb.center()],
                width: (ext.x.min(ext.z)).max(0.05),
                height: ext.y.max(0.05),
                depth: ext.x.max(ext.y).max(ext.z),
                direction: Vector3::x(),
                support_points: vec![],
                section_type: "SCTN".into(),
            };
            let mut contacts = Vec::new();
            for (id, bb, _) in &supports {
                contacts.push((
                    *id,
                    ContactResult {
                        contact_type: ContactType::Point,
                        contact_points: vec![Point3::new(bb.center().x, bb.maxs.y, bb.center().z)],
                        contact_normal: Vector3::y(),
                        penetration_depth: 0.0,
                        contact_area: 0.0,
                        distance: 0.0,
                    },
                ));
            }
            vis.export_to_obj(&[sctn.clone()], "sqlite_tray_supports.obj")?;
            vis.export_to_html(&[sctn], &contacts, &[], "sqlite_tray_supports.html")?;
            println!(
                "\n已导出: test_output/sqlite_tray_supports.obj 与 test_output/sqlite_tray_supports.html"
            );
        }
    }

    Ok(())
}
