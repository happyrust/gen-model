/// 通过目标 SCTN 找到“关联的桥架”（相邻/连接/接近）
/// 参考模块：
/// - 接触检测: sctn_contact_detector.rs
/// - 几何提取: sctn_geometry_extractor.rs
/// - 路径分析: sctn_path_analyzer.rs
/// - 可视化:   sctn_visualizer.rs
/// - 空间索引: spatial_index.rs (SQLite R-Tree)
///
/// 特性: 需要启用 sqlite-index；如需精确几何参数和类型过滤，建议同时启用 grpc 并正确配置数据库
/// 运行示例:
///   cargo run --example find_associated_trays --features sqlite-index,grpc -- \
///     --target 24383/86525 --radius 1.0 --limit 50 --tolerance 0.1 --export
use anyhow::{Result, anyhow};
use clap::{Arg, Command};
use std::sync::Arc;

use aios_core::pdms_types::RefU64;
use nalgebra::{Point3, Vector3};
use parry3d::bounding_volume::Aabb;

use aios_database::spatial_index::SqliteSpatialIndex;

#[cfg(feature = "grpc")]
use aios_database::data_interface::tidb_manager::AiosDBManager;
#[cfg(feature = "grpc")]
use aios_database::grpc_service::sctn_geometry_extractor::SctnGeometryExtractor;

use aios_database::grpc_service::sctn_contact_detector::{CableTraySection, SctnContactDetector};
use aios_database::grpc_service::sctn_path_analyzer::SctnPathAnalyzer;
use aios_database::grpc_service::sctn_visualizer::SctnVisualizer;

fn parse_refno(s: &str) -> Result<RefU64> {
    use std::str::FromStr;
    RefU64::from_str(s).map_err(|_| anyhow!("无效的RefNo格式: {}", s))
}

fn expand_aabb(aabb: &Aabb, r: f32) -> Aabb {
    let mins = aabb.mins - Vector3::new(r, r, r);
    let maxs = aabb.maxs + Vector3::new(r, r, r);
    Aabb::new(mins, maxs)
}

/// 基于AABB的兜底重建（无DB时），从AABB估计一个CableTraySection
fn reconstruct_sctn_from_aabb(refno: RefU64, bbox: &Aabb) -> CableTraySection {
    // 以最大轴向作为长度，其他两个作为宽高（简单启发）
    let ext = bbox.maxs - bbox.mins;
    let (depth, dir) = if ext.x >= ext.y && ext.x >= ext.z {
        (ext.x, Vector3::new(1.0, 0.0, 0.0))
    } else if ext.z >= ext.x && ext.z >= ext.y {
        (ext.z, Vector3::new(0.0, 0.0, 1.0))
    } else {
        (ext.y, Vector3::new(0.0, 1.0, 0.0))
    };

    let width = (ext.x.min(ext.z)).max(0.05);
    let height = ext.y.max(0.05);
    CableTraySection {
        refno,
        bbox: bbox.clone(),
        centerline: vec![bbox.center()],
        width,
        height,
        depth: depth.max(0.1),
        direction: dir,
        support_points: vec![],
        section_type: "SCTN".to_string(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Command::new("Find Associated Trays")
        .version("0.1")
        .about("通过目标SCTN查找关联桥架（邻近/连接/接触），并构建拓扑")
        .arg(
            Arg::new("target")
                .long("target")
                .required(true)
                .value_name("REFNO")
                .help("目标SCTN参考号，如 24383/86525"),
        )
        .arg(
            Arg::new("radius")
                .long("radius")
                .default_value("1.0")
                .help("邻域查询半径(米)"),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .default_value("100")
                .help("最多检查邻居数量"),
        )
        .arg(
            Arg::new("tolerance")
                .long("tolerance")
                .default_value("0.1")
                .help("接触/连接判定的容差(米)"),
        )
        .arg(
            Arg::new("export")
                .long("export")
                .action(clap::ArgAction::SetTrue)
                .help("导出HTML/OBJ可视化到 test_output/"),
        )
        .get_matches();

    let target = parse_refno(matches.get_one::<String>("target").unwrap())?;
    let radius: f32 = matches
        .get_one::<String>("radius")
        .unwrap()
        .parse()
        .unwrap_or(1.0);
    let limit: usize = matches
        .get_one::<String>("limit")
        .unwrap()
        .parse()
        .unwrap_or(100);
    let tolerance: f32 = matches
        .get_one::<String>("tolerance")
        .unwrap()
        .parse()
        .unwrap_or(0.1);
    let do_export = matches.get_flag("export");

    if !SqliteSpatialIndex::is_enabled() {
        eprintln!("提示: 未启用 SQLite R-Tree 索引（enable_sqlite_rtree=false?），候选检索会退化");
    }

    // 索引与（可选）DB管理器
    let index = SqliteSpatialIndex::with_default_path()?;

    #[cfg(feature = "grpc")]
    let dbm_opt = Some(Arc::new(AiosDBManager::init_form_config().await?));
    #[cfg(not(feature = "grpc"))]
    let dbm_opt: Option<Arc<AiosDBManager>> = None;

    // 目标SCTN
    let target_bbox = index
        .get_aabb(target)?
        .ok_or_else(|| anyhow!("索引中未找到目标SCTN: {}，请先生成模型并写入索引", target.0))?;

    #[cfg(feature = "grpc")]
    let target_sctn = {
        let extractor = SctnGeometryExtractor::new(dbm_opt.clone().unwrap());
        match extractor.extract_sctn_geometry(target).await {
            Ok(s) => s,
            Err(_) => reconstruct_sctn_from_aabb(target, &target_bbox),
        }
    };
    #[cfg(not(feature = "grpc"))]
    let target_sctn = reconstruct_sctn_from_aabb(target, &target_bbox);

    println!("目标SCTN: {}", target.0);
    println!(
        "  位置: ({:.3},{:.3},{:.3})-({:.3},{:.3},{:.3})",
        target_bbox.mins.x,
        target_bbox.mins.y,
        target_bbox.mins.z,
        target_bbox.maxs.x,
        target_bbox.maxs.y,
        target_bbox.maxs.z
    );

    // 邻域候选
    let query = expand_aabb(&target_bbox, radius);
    let mut neighbors = index.query_intersect(&query)?;
    neighbors.retain(|r| *r != target);
    if neighbors.len() > limit {
        neighbors.truncate(limit);
    }

    // 过滤SCTN + 构建候选几何
    let mut sctn_candidates: Vec<CableTraySection> = Vec::new();
    for r in neighbors {
        if let Some(b) = index.get_aabb(r)? {
            #[cfg(feature = "grpc")]
            let is_sctn = if let Some(ref dbm) = dbm_opt {
                dbm.get_type_name(r).await == "SCTN"
            } else {
                false
            };
            #[cfg(not(feature = "grpc"))]
            let is_sctn = true; // 无法识别类型时保守纳入，交给后续接触判定

            if is_sctn {
                #[cfg(feature = "grpc")]
                {
                    let extractor = SctnGeometryExtractor::new(dbm_opt.clone().unwrap());
                    match extractor.extract_sctn_geometry(r).await {
                        Ok(s) => sctn_candidates.push(s),
                        Err(_) => sctn_candidates.push(reconstruct_sctn_from_aabb(r, &b)),
                    }
                }
                #[cfg(not(feature = "grpc"))]
                sctn_candidates.push(reconstruct_sctn_from_aabb(r, &b));
            }
        }
    }

    // 接触/连接检测
    let detector = if let Some(dbm) = dbm_opt.clone() {
        SctnContactDetector::with_db_manager(tolerance, dbm)?
    } else {
        SctnContactDetector::new(tolerance)?
    };

    let mut associated: Vec<CableTraySection> = Vec::new();
    let mut contacts_all = Vec::new();

    for s in &sctn_candidates {
        let result = detector.check_detailed_contact(
            &target_sctn,
            &aios_database::grpc_service::spatial_query_service::SpatialElement {
                refno: s.refno,
                bbox: s.bbox.clone(),
                element_type: "SCTN".to_string(),
                element_name: format!("SCTN_{}", s.refno.0),
                last_updated: std::time::SystemTime::now(),
            },
            true,
        )?;
        if let Some(contact) = result {
            // 认为接触/接近即为“关联桥架”
            associated.push(s.clone());
            contacts_all.push((s.refno, contact));
        }
    }

    // 构建拓扑并分析
    let mut sections_for_network = associated.clone();
    sections_for_network.insert(0, target_sctn.clone());
    let analyzer = SctnPathAnalyzer::new(tolerance);
    let network = analyzer.build_tray_network(&sections_for_network);
    let connectivity = analyzer.analyze_connectivity(&network);

    println!("\n关联桥架数量: {}", associated.len());
    for s in &associated {
        println!(
            "  - {} 位置中心 ({:.2},{:.2},{:.2})",
            s.refno.0,
            s.bbox.center().x,
            s.bbox.center().y,
            s.bbox.center().z
        );
    }
    println!(
        "\n连通分量: {}，是否完全连通: {}",
        connectivity.num_components, connectivity.is_fully_connected
    );

    if do_export {
        std::fs::create_dir_all("test_output").ok();
        let vis = SctnVisualizer::new("test_output");
        vis.export_to_obj(&sections_for_network, "sctn_associated.obj")?;
        vis.export_to_html(
            &sections_for_network,
            &contacts_all,
            &[],
            "sctn_associated.html",
        )?;
        println!("\n已导出: test_output/sctn_associated.obj 与 test_output/sctn_associated.html");
    }

    Ok(())
}
