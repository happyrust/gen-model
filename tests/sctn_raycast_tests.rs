#![cfg(feature = "grpc")]
use aios_database::grpc_service::sctn_contact_detector::{SctnContactDetector, CableTraySection};
use aios_database::spatial_index::SqliteSpatialIndex;
use aios_core::RefU64;
use anyhow::Result;
use nalgebra::{Point3, Vector3};
use parry3d::bounding_volume::Aabb;

#[cfg(feature = "sqlite-index")]
#[test]
fn test_raycast_support_detection() -> Result<()> {
    // 构建临时索引并插入一个桥架与一个支架
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("aabb_cache.sqlite");
    let index = SqliteSpatialIndex::new(path)?;

    // 桥架（SCTN）AABB：位于 y=5 附近
    let tray_ref = RefU64(100);
    let tray_bbox = Aabb::new(Point3::new(0.0, 5.0, 0.0), Point3::new(2.0, 5.3, 1.0));
    index.insert_aabb(tray_ref, &tray_bbox, Some("SCTN"))?;

    // 支架（SUPPO）：在桥架下方 y<5
    let supp_ref = RefU64(200);
    let supp_bbox = Aabb::new(Point3::new(0.8, 0.0, 0.2), Point3::new(1.2, 4.9, 0.8));
    index.insert_aabb(supp_ref, &supp_bbox, Some("SUPPO"))?;

    // 构建检测器（注入索引）
    let detector = SctnContactDetector::with_index(0.01, index)?;

    // 目标桥架
    let sctn = CableTraySection {
        refno: tray_ref,
        bbox: tray_bbox.clone(),
        centerline: vec![],
        width: 0.6,
        height: 0.3,
        depth: 2.0,
        direction: Vector3::x(),
        support_points: vec![Point3::new(1.0, tray_bbox.mins.y, 0.5)],
        section_type: "SCTN".into(),
    };

    // 触发支撑检测（最大距离 10m）
    let rt = tokio::runtime::Runtime::new().unwrap();
    let relations = rt.block_on(detector.detect_support_relationships(&sctn, 10.0))?;

    assert!(!relations.is_empty(), "应检测到至少一个支撑");
    assert_eq!(relations[0].support.0, supp_ref.0);
    Ok(())
}
