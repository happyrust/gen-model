/// 通过SCTN找到对应的支撑（SUPPO等）
///
/// 流程:
/// 1) 用SQLite R-Tree读出目标SCTN的AABB，估算或提取几何
/// 2) 在其邻域范围内检索候选支撑构件
/// 3) 使用 SctnRaycastDetector 基于射线投射判定支撑
/// 4) 输出支撑坐标、跨度分析和悬空段提示，可选导出可视化
///
/// 运行:
///  cargo run --example find_tray_supports --features sqlite-index,grpc -- \
///    --target 24383/86525 --radius 2.0 --limit 200 --maxray 6.0 --export

use anyhow::{anyhow, Result};
use clap::{Arg, Command};
use std::collections::HashMap;
use std::sync::Arc;

use aios_core::pdms_types::RefU64;
use nalgebra::Vector3;
use parry3d::bounding_volume::Aabb;

use aios_database::spatial_index::SqliteSpatialIndex;
use aios_database::grpc_service::sctn_contact_detector::CableTraySection;
use aios_database::grpc_service::sctn_raycast_detector::{
    SctnRaycastDetector, SupportCandidate, AdvancedRaycastAnalyzer
};
use aios_database::grpc_service::sctn_visualizer::SctnVisualizer;

#[cfg(feature = "grpc")]
use aios_database::grpc_service::sctn_geometry_extractor::SctnGeometryExtractor;
#[cfg(feature = "grpc")]
use aios_database::data_interface::tidb_manager::AiosDBManager;

fn parse_refno(s: &str) -> Result<RefU64> { use std::str::FromStr; RefU64::from_str(s).map_err(|_| anyhow!("无效RefNo: {}", s)) }

fn expand_aabb(aabb: &Aabb, r: f32) -> Aabb {
    let mins = aabb.mins - Vector3::new(r, r, r);
    let maxs = aabb.maxs + Vector3::new(r, r, r);
    Aabb::new(mins, maxs)
}

fn reconstruct_sctn_from_aabb(refno: RefU64, bbox: &Aabb) -> CableTraySection {
    let ext = bbox.maxs - bbox.mins;
    let (depth, dir) = if ext.x >= ext.y && ext.x >= ext.z {(ext.x, Vector3::new(1.0,0.0,0.0))}
                       else if ext.z >= ext.x && ext.z >= ext.y {(ext.z, Vector3::new(0.0,0.0,1.0))}
                       else {(ext.y, Vector3::new(0.0,1.0,0.0))};
    let width = (ext.x.min(ext.z)).max(0.05);
    let height = ext.y.max(0.05);
    CableTraySection { refno, bbox: bbox.clone(), centerline: vec![bbox.center()], width, height, depth: depth.max(0.1), direction: dir, support_points: vec![], section_type: "SCTN".into() }
}

#[tokio::main]
async fn main() -> Result<()> {
    let m = Command::new("Find Tray Supports")
        .version("0.1")
        .about("通过SCTN查找对应支撑（基于SQLite索引+射线投射）")
        .arg(Arg::new("target").long("target").required(true).help("目标SCTN，如 24383/86525"))
        .arg(Arg::new("radius").long("radius").default_value("2.0").help("邻域半径(米)"))
        .arg(Arg::new("limit").long("limit").default_value("200").help("最多检查邻居数量"))
        .arg(Arg::new("maxray").long("maxray").default_value("6.0").help("射线最大距离(米)"))
        .arg(Arg::new("export").long("export").action(clap::ArgAction::SetTrue).help("导出可视化到 test_output/"))
        .get_matches();

    let target = parse_refno(m.get_one::<String>("target").unwrap())?;
    let radius: f32 = m.get_one::<String>("radius").unwrap().parse().unwrap_or(2.0);
    let limit: usize = m.get_one::<String>("limit").unwrap().parse().unwrap_or(200);
    let max_ray: f32 = m.get_one::<String>("maxray").unwrap().parse().unwrap_or(6.0);
    let do_export = m.get_flag("export");

    let index = SqliteSpatialIndex::with_default_path()?;
    let tb = index.get_aabb(target)?.ok_or_else(|| anyhow!("索引缺少目标SCTN: {}", target.0))?;

    #[cfg(feature = "grpc")]
    let dbm_opt = Some(Arc::new(AiosDBManager::init_form_config().await?));
    #[cfg(not(feature = "grpc"))]
    let dbm_opt: Option<Arc<AiosDBManager>> = None;

    // 目标SCTN几何
    #[cfg(feature = "grpc")]
    let sctn = {
        let ex = SctnGeometryExtractor::new(dbm_opt.clone().unwrap());
        ex.extract_sctn_geometry(target).await.unwrap_or_else(|_| reconstruct_sctn_from_aabb(target, &tb))
    };
    #[cfg(not(feature = "grpc"))]
    let sctn = reconstruct_sctn_from_aabb(target, &tb);

    println!("目标SCTN {}: ({:.3},{:.3},{:.3})-({:.3},{:.3},{:.3})", target.0, tb.mins.x,tb.mins.y,tb.mins.z, tb.maxs.x,tb.maxs.y,tb.maxs.z);

    // 候选支撑（SUPPO等）
    let query = expand_aabb(&tb, radius);
    let mut neigh = index.query_intersect(&query)?;
    neigh.retain(|r| *r != target);
    if neigh.len() > limit { neigh.truncate(limit); }

    let mut candidates: Vec<SupportCandidate> = Vec::new();
    for r in neigh {
        if let Some(b) = index.get_aabb(r)? {
            #[cfg(feature = "grpc")]
            let typ = if let Some(ref dbm) = dbm_opt { dbm.get_type_name(r).await } else { "".into() };
            #[cfg(not(feature = "grpc"))]
            let typ = String::new();

            // 优先仅收集类型为SUPPO的；无DB时无法识别类型，则先纳入，后续由射线几何过滤
            if typ.is_empty() || typ == "SUPPO" || typ == "STRU" || typ == "HANG" {
                candidates.push(SupportCandidate { refno: r, bbox: b, element_type: if typ.is_empty(){"UNKNOWN".into()}else{typ}, attributes: HashMap::new() });
            }
        }
    }

    println!("候选支撑数量: {}", candidates.len());

    // 射线投射检测
    let detector = SctnRaycastDetector::new(max_ray)?;
    let supports = detector.detect_supports_by_raycast(&sctn, &candidates).await?;

    println!("\n检测到支撑数: {}", supports.len());
    for s in &supports {
        println!("  ✓ 支撑 {} 于 ({:.3},{:.3},{:.3})", s.support.0, s.contact_point.x, s.contact_point.y, s.contact_point.z);
    }

    // 跨度分析与悬空段
    let analyzer = AdvancedRaycastAnalyzer::new(max_ray)?;
    let span = analyzer.analyze_support_spans(&supports);
    println!("\n跨度统计: 数量={} 最大={:.3} 最小={:.3} 平均={:.3}", span.num_supports, span.max_span, span.min_span, span.avg_span);

    let unsupported = analyzer.detect_unsupported_segments(&sctn, &supports, 2.0);
    if !unsupported.is_empty() {
        println!("\n悬空段: {} 段", unsupported.len());
        for u in &unsupported {
            println!("  - 长度 {:.3} ({:.3}->{:.3})", u.length, u.start.x, u.end.x);
        }
    }

    if do_export {
        std::fs::create_dir_all("test_output").ok();
        let vis = SctnVisualizer::new("test_output");
        // 仅导出目标SCTN为简化；可扩展导出周边支撑AABB
        use aios_database::grpc_service::sctn_contact_detector::{ContactResult, ContactType};
        let mut fake_contacts = Vec::new();
        for s in &supports {
            fake_contacts.push((s.support, ContactResult{
                contact_type: ContactType::Point,
                contact_points: vec![s.contact_point],
                contact_normal: Vector3::y(),
                penetration_depth: 0.0,
                contact_area: 0.0,
                distance: 0.0,
            }));
        }
        vis.export_to_obj(&[sctn.clone()], "tray_support.obj")?;
        vis.export_to_html(&[sctn.clone()], &fake_contacts, &supports, "tray_support.html")?;
        println!("\n已导出: test_output/tray_support.obj 与 test_output/tray_support.html");
    }

    Ok(())
}

