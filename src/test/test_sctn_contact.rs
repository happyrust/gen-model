use aios_core::pdms_types::RefU64;
use anyhow::Result;
use nalgebra::{Point3, Vector3};
use parry3d::bounding_volume::Aabb;
use std::str::FromStr;

use crate::grpc_service::sctn_contact_detector::{
    BatchSctnDetector, CableTraySection, ContactType, SctnContactDetector, SupportType,
};

/// 测试基本的SCTN接触检测
#[tokio::test]
async fn test_basic_sctn_contact_detection() -> Result<()> {
    // 创建测试用的SCTN
    let sctn = CableTraySection {
        refno: RefU64::from_str("24383/95023").unwrap(),
        bbox: Aabb::new(Point3::new(0.0, 1.0, 0.0), Point3::new(3.0, 1.3, 0.3)),
        centerline: vec![Point3::new(0.0, 1.15, 0.15), Point3::new(3.0, 1.15, 0.15)],
        width: 0.3,
        height: 0.3,
        depth: 3.0,
        direction: Vector3::new(1.0, 0.0, 0.0),
        support_points: vec![],
        section_type: "SCTN".to_string(),
    };

    // 创建检测器
    let detector = SctnContactDetector::new(0.01)?;

    // 执行接触检测
    let contacts = detector
        .detect_sctn_contacts(&sctn, &["PIPE".to_string(), "EQUI".to_string()], true)
        .await?;

    println!("检测到 {} 个接触", contacts.len());
    for (refno, contact) in &contacts {
        println!(
            "接触对象: {}, 类型: {:?}, 距离: {:.3}m, 穿透深度: {:.3}m",
            refno.0, contact.contact_type, contact.distance, contact.penetration_depth
        );
    }

    Ok(())
}

/// 测试桥架与支架的支撑关系检测
#[tokio::test]
async fn test_tray_support_detection() -> Result<()> {
    let sctn = CableTraySection {
        refno: RefU64::from_str("24383/95024").unwrap(),
        bbox: Aabb::new(Point3::new(0.0, 2.0, 0.0), Point3::new(3.0, 2.1, 0.3)),
        centerline: vec![Point3::new(0.0, 2.05, 0.15), Point3::new(3.0, 2.05, 0.15)],
        width: 0.3,
        height: 0.1,
        depth: 3.0,
        direction: Vector3::new(1.0, 0.0, 0.0),
        support_points: vec![
            Point3::new(0.5, 2.0, 0.15),
            Point3::new(1.5, 2.0, 0.15),
            Point3::new(2.5, 2.0, 0.15),
        ],
        section_type: "SCTN".to_string(),
    };

    let detector = SctnContactDetector::new(0.001)?;

    // 检测支撑关系
    let supports = detector.detect_support_relationships(&sctn, 5.0).await?;

    println!("检测到 {} 个支撑点", supports.len());
    for support in &supports {
        println!(
            "支撑构件: {}, 类型: {:?}, 接触点: ({:.2}, {:.2}, {:.2}), 荷载分布: {:.2}",
            support.support.0,
            support.support_type,
            support.contact_point.x,
            support.contact_point.y,
            support.contact_point.z,
            support.load_distribution
        );
    }

    Ok(())
}

/// 测试批量SCTN接触检测
#[tokio::test]
async fn test_batch_sctn_detection() -> Result<()> {
    // 创建多个测试SCTN
    let sections = vec![
        CableTraySection {
            refno: RefU64::from_str("24383/95025").unwrap(),
            bbox: Aabb::new(Point3::new(0.0, 3.0, 0.0), Point3::new(3.0, 3.1, 0.3)),
            centerline: vec![],
            width: 0.3,
            height: 0.1,
            depth: 3.0,
            direction: Vector3::new(1.0, 0.0, 0.0),
            support_points: vec![],
            section_type: "SCTN".to_string(),
        },
        CableTraySection {
            refno: RefU64::from_str("24383/95026").unwrap(),
            bbox: Aabb::new(Point3::new(3.0, 3.0, 0.0), Point3::new(6.0, 3.1, 0.3)),
            centerline: vec![],
            width: 0.3,
            height: 0.1,
            depth: 3.0,
            direction: Vector3::new(1.0, 0.0, 0.0),
            support_points: vec![],
            section_type: "SCTN".to_string(),
        },
        CableTraySection {
            refno: RefU64::from_str("24383/95027").unwrap(),
            bbox: Aabb::new(Point3::new(6.0, 3.0, 0.0), Point3::new(6.3, 6.0, 0.3)),
            centerline: vec![],
            width: 0.3,
            height: 0.3,
            depth: 3.0,
            direction: Vector3::new(0.0, 1.0, 0.0),
            support_points: vec![],
            section_type: "SCTN".to_string(),
        },
    ];

    let batch_detector = BatchSctnDetector::new(0.01)?;

    // 批量检测
    let results = batch_detector
        .detect_batch(sections.clone(), &["PIPE".to_string(), "STRU".to_string()])
        .await?;

    println!("批量检测结果:");
    for (refno, contacts) in &results {
        println!("SCTN {}: 检测到 {} 个接触", refno.0, contacts.len());
    }

    // 检测桥架间的连接关系
    let connections = batch_detector.detect_tray_connections(&sections).await?;

    println!("\n桥架连接关系:");
    for conn in &connections {
        println!(
            "SCTN {} <-> SCTN {}: {:?} 连接于 ({:.2}, {:.2}, {:.2})",
            conn.section1.0,
            conn.section2.0,
            conn.connection_type,
            conn.connection_point.x,
            conn.connection_point.y,
            conn.connection_point.z
        );
    }

    Ok(())
}

/// 测试不同接触类型的识别
#[tokio::test]
async fn test_contact_type_recognition() -> Result<()> {
    let detector = SctnContactDetector::new(0.001)?;

    // 测试表面接触
    let sctn_surface = CableTraySection {
        refno: RefU64::from_str("24383/95028").unwrap(),
        bbox: Aabb::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.1, 1.0)),
        centerline: vec![],
        width: 1.0,
        height: 0.1,
        depth: 1.0,
        direction: Vector3::new(1.0, 0.0, 0.0),
        support_points: vec![],
        section_type: "SCTN".to_string(),
    };

    let contacts = detector
        .detect_sctn_contacts(&sctn_surface, &[], true)
        .await?;

    // 验证接触类型
    for (_, contact) in &contacts {
        match contact.contact_type {
            ContactType::Surface => {
                println!("检测到表面接触，接触面积: {:.4} m²", contact.contact_area);
            }
            ContactType::Edge => {
                println!("检测到边缘接触");
            }
            ContactType::Point => {
                println!("检测到点接触");
            }
            ContactType::Penetration => {
                println!("检测到穿透，深度: {:.3} m", contact.penetration_depth);
            }
            ContactType::Proximity => {
                println!("检测到接近关系，距离: {:.3} m", contact.distance);
            }
            ContactType::None => {
                println!("无接触");
            }
        }
    }

    Ok(())
}

/// 测试容差设置对检测结果的影响
#[tokio::test]
async fn test_tolerance_impact() -> Result<()> {
    let sctn = CableTraySection {
        refno: RefU64::from_str("24383/95029").unwrap(),
        bbox: Aabb::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.1, 1.0)),
        centerline: vec![],
        width: 1.0,
        height: 0.1,
        depth: 1.0,
        direction: Vector3::new(1.0, 0.0, 0.0),
        support_points: vec![],
        section_type: "SCTN".to_string(),
    };

    // 测试不同容差值
    let tolerances = vec![0.001, 0.01, 0.1, 1.0];

    for tolerance in tolerances {
        let detector = SctnContactDetector::new(tolerance)?;
        let contacts = detector.detect_sctn_contacts(&sctn, &[], true).await?;

        println!(
            "容差 {:.3}m: 检测到 {} 个接触/接近关系",
            tolerance,
            contacts.len()
        );
    }

    Ok(())
}

/// 测试性能：大批量SCTN检测
#[tokio::test]
#[ignore] // 性能测试，默认跳过
async fn test_performance_large_batch() -> Result<()> {
    use std::time::Instant;

    // 生成大量测试SCTN
    let mut sections = Vec::new();
    for i in 0..100 {
        for j in 0..10 {
            let x = i as f32 * 3.0;
            let y = j as f32 * 0.5;

            sections.push(CableTraySection {
                refno: RefU64(10000 + i * 10 + j),
                bbox: Aabb::new(Point3::new(x, y, 0.0), Point3::new(x + 3.0, y + 0.1, 0.3)),
                centerline: vec![],
                width: 0.3,
                height: 0.1,
                depth: 3.0,
                direction: Vector3::new(1.0, 0.0, 0.0),
                support_points: vec![],
                section_type: "SCTN".to_string(),
            });
        }
    }

    println!("测试 {} 个SCTN的批量检测性能", sections.len());

    let batch_detector = BatchSctnDetector::new(0.01)?;
    let start = Instant::now();

    let results = batch_detector.detect_batch(sections.clone(), &[]).await?;

    let elapsed = start.elapsed();
    let total_contacts: usize = results.iter().map(|(_, c)| c.len()).sum();

    println!(
        "处理时间: {:.2}s, 总接触数: {}, 平均每个SCTN: {:.2}ms",
        elapsed.as_secs_f32(),
        total_contacts,
        elapsed.as_millis() as f32 / sections.len() as f32
    );

    // 测试连接关系检测性能
    let start = Instant::now();
    let connections = batch_detector.detect_tray_connections(&sections).await?;
    let elapsed = start.elapsed();

    println!(
        "连接检测时间: {:.2}s, 检测到 {} 个连接",
        elapsed.as_secs_f32(),
        connections.len()
    );

    Ok(())
}

/// 测试与真实数据库的集成
#[tokio::test]
#[ignore] // 需要真实数据库连接
async fn test_with_real_database() -> Result<()> {
    // 从数据库查询真实的SCTN数据
    let bran_refno = RefU64::from_str("24383/95023").unwrap();

    // TODO: 实现从数据库获取SCTN几何信息的逻辑
    // let sctn_data = mgr.get_sctn_geometry(bran_refno).await?;

    println!("测试与真实数据库的集成（需要实现数据库查询）");
    println!("Branch RefNo: {}", bran_refno.0);

    Ok(())
}
