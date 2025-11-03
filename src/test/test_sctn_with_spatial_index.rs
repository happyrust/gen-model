use aios_core::pdms_types::RefU64;
use anyhow::Result;
use nalgebra::{Point3, Vector3};
use parry3d::bounding_volume::Aabb;
use std::path::PathBuf;
/// SCTN桥架空间索引集成测试
///
/// 测试场景：
/// 1. 初始化DB 7999的空间数据
/// 2. 插入SCTN桥架数据到SQLite空间索引
/// 3. 查找24383/86525这个SCTN对应的桥架
/// 4. 执行接触检测
use std::str::FromStr;

#[cfg(feature = "sqlite-index")]
use crate::spatial_index::{SharedSpatialIndex, SqliteSpatialIndex};

use crate::grpc_service::sctn_contact_detector::{
    BatchSctnDetector, CableTraySection, ContactType, SctnContactDetector, SupportType,
};

use crate::grpc_service::spatial_query_service::SpatialElement;

/// 测试数据结构 - 模拟DB 7999的桥架数据
struct TrayTestData {
    refno: RefU64,
    bbox: Aabb,
    element_type: String,
    element_name: String,
}

/// 初始化DB 7999的测试数据
fn init_db_7999_test_data() -> Vec<TrayTestData> {
    vec![
        // 主桥架段 - SCTN
        TrayTestData {
            refno: RefU64::from_str("24383/86525").unwrap(), // 目标SCTN
            bbox: Aabb::new(Point3::new(100.0, 5.0, 20.0), Point3::new(110.0, 5.3, 20.6)),
            element_type: "SCTN".to_string(),
            element_name: "Cable_Tray_Section_001".to_string(),
        },
        // 相邻桥架段1
        TrayTestData {
            refno: RefU64::from_str("24383/86526").unwrap(),
            bbox: Aabb::new(Point3::new(109.9, 5.0, 20.0), Point3::new(119.9, 5.3, 20.6)),
            element_type: "SCTN".to_string(),
            element_name: "Cable_Tray_Section_002".to_string(),
        },
        // 相邻桥架段2
        TrayTestData {
            refno: RefU64::from_str("24383/86527").unwrap(),
            bbox: Aabb::new(Point3::new(90.1, 5.0, 20.0), Point3::new(100.1, 5.3, 20.6)),
            element_type: "SCTN".to_string(),
            element_name: "Cable_Tray_Section_003".to_string(),
        },
        // 垂直转弯桥架
        TrayTestData {
            refno: RefU64::from_str("24383/86528").unwrap(),
            bbox: Aabb::new(Point3::new(119.8, 5.0, 20.0), Point3::new(120.4, 8.0, 20.6)),
            element_type: "SCTN".to_string(),
            element_name: "Cable_Tray_Vertical_001".to_string(),
        },
        // 支架1
        TrayTestData {
            refno: RefU64::from_str("24383/90001").unwrap(),
            bbox: Aabb::new(Point3::new(102.0, 0.0, 20.2), Point3::new(102.4, 5.0, 20.4)),
            element_type: "SUPPO".to_string(),
            element_name: "Support_001".to_string(),
        },
        // 支架2
        TrayTestData {
            refno: RefU64::from_str("24383/90002").unwrap(),
            bbox: Aabb::new(Point3::new(107.0, 0.0, 20.2), Point3::new(107.4, 5.0, 20.4)),
            element_type: "SUPPO".to_string(),
            element_name: "Support_002".to_string(),
        },
        // 支架3
        TrayTestData {
            refno: RefU64::from_str("24383/90003").unwrap(),
            bbox: Aabb::new(Point3::new(115.0, 0.0, 20.2), Point3::new(115.4, 5.0, 20.4)),
            element_type: "SUPPO".to_string(),
            element_name: "Support_003".to_string(),
        },
        // 管道（可能与桥架碰撞）
        TrayTestData {
            refno: RefU64::from_str("24383/50001").unwrap(),
            bbox: Aabb::new(Point3::new(105.0, 5.2, 19.5), Point3::new(105.3, 5.5, 21.5)),
            element_type: "PIPE".to_string(),
            element_name: "Pipe_001".to_string(),
        },
        // 设备（在桥架附近）
        TrayTestData {
            refno: RefU64::from_str("24383/60001").unwrap(),
            bbox: Aabb::new(Point3::new(108.0, 4.0, 19.0), Point3::new(112.0, 6.0, 22.0)),
            element_type: "EQUI".to_string(),
            element_name: "Equipment_001".to_string(),
        },
        // 结构梁（桥架可能附着）
        TrayTestData {
            refno: RefU64::from_str("24383/70001").unwrap(),
            bbox: Aabb::new(Point3::new(95.0, 8.0, 19.0), Point3::new(125.0, 8.5, 22.0)),
            element_type: "STRU".to_string(),
            element_name: "Beam_001".to_string(),
        },
    ]
}

/// 初始化SQLite空间索引并插入数据
#[cfg(feature = "sqlite-index")]
async fn init_spatial_index(test_data: &[TrayTestData]) -> Result<SqliteSpatialIndex> {
    println!("初始化SQLite空间索引...");

    // 创建或打开空间索引
    let index_path = PathBuf::from("test_7999_spatial.sqlite");
    let index = SqliteSpatialIndex::new(&index_path)?;

    // 清空旧数据
    index.clear()?;
    println!("已清空旧索引数据");

    // 批量插入测试数据
    let insert_data: Vec<(RefU64, Aabb, Option<String>)> = test_data
        .iter()
        .map(|td| (td.refno, td.bbox.clone(), Some(td.element_type.clone())))
        .collect();

    let inserted_count = index.insert_many(insert_data)?;
    println!("已插入 {} 条空间数据到索引", inserted_count);

    // 验证插入
    let stats = index.get_stats()?;
    println!("索引统计: {:?}", stats);

    Ok(index)
}

/// 测试SCTN 24383/86525的空间查询
#[cfg(feature = "sqlite-index")]
async fn test_sctn_spatial_query(index: &SqliteSpatialIndex) -> Result<()> {
    println!("\n=== 测试SCTN 24383/86525的空间查询 ===");

    let target_refno = RefU64::from_str("24383/86525").unwrap();

    // 获取目标SCTN的包围盒
    let target_bbox = index
        .get_aabb(target_refno)?
        .ok_or_else(|| anyhow::anyhow!("未找到SCTN {}", target_refno.0))?;

    println!("目标SCTN {} 包围盒:", target_refno.0);
    println!(
        "  最小点: ({:.1}, {:.1}, {:.1})",
        target_bbox.mins.x, target_bbox.mins.y, target_bbox.mins.z
    );
    println!(
        "  最大点: ({:.1}, {:.1}, {:.1})",
        target_bbox.maxs.x, target_bbox.maxs.y, target_bbox.maxs.z
    );

    // 扩展包围盒进行查询（容差1米）
    let tolerance = 1.0;
    let query_bbox = Aabb::new(
        target_bbox.mins - Vector3::new(tolerance, tolerance, tolerance),
        target_bbox.maxs + Vector3::new(tolerance, tolerance, tolerance),
    );

    // 查询相交的构件
    let intersecting = index.query_intersect(&query_bbox)?;

    println!("\n查询结果（容差 {}m）:", tolerance);
    println!("找到 {} 个相交/邻近的构件:", intersecting.len());

    for refno in &intersecting {
        if *refno == target_refno {
            continue; // 跳过自身
        }

        if let Some(bbox) = index.get_aabb(*refno)? {
            let distance = (bbox.center() - target_bbox.center()).norm();
            println!("  RefNo {}: 距离 {:.2}m", refno.0, distance);
        }
    }

    Ok(())
}

/// 测试SCTN接触检测
async fn test_sctn_contact_detection(test_data: &[TrayTestData]) -> Result<()> {
    println!("\n=== 测试SCTN接触检测 ===");

    let target_refno = RefU64::from_str("24383/86525").unwrap();

    // 找到目标SCTN数据
    let target_data = test_data
        .iter()
        .find(|td| td.refno == target_refno)
        .ok_or_else(|| anyhow::anyhow!("未找到目标SCTN数据"))?;

    // 创建CableTraySection
    let target_sctn = CableTraySection {
        refno: target_data.refno,
        bbox: target_data.bbox.clone(),
        centerline: vec![
            Point3::new(100.0, 5.15, 20.3),
            Point3::new(110.0, 5.15, 20.3),
        ],
        width: 0.6,                             // 600mm宽
        height: 0.3,                            // 300mm高
        depth: 10.0,                            // 10m长
        direction: Vector3::new(1.0, 0.0, 0.0), // X方向
        support_points: vec![Point3::new(102.2, 5.0, 20.3), Point3::new(107.2, 5.0, 20.3)],
        section_type: "SCTN".to_string(),
    };

    // 创建检测器
    let detector = SctnContactDetector::new(0.1)?; // 100mm容差

    // 模拟其他构件作为SpatialElement
    let mut candidates: Vec<SpatialElement> = test_data
        .iter()
        .filter(|td| td.refno != target_refno)
        .map(|td| SpatialElement {
            refno: td.refno,
            bbox: td.bbox.clone(),
            element_type: td.element_type.clone(),
            element_name: td.element_name.clone(),
            last_updated: std::time::SystemTime::now(),
        })
        .collect();

    println!(
        "目标SCTN: {} ({})",
        target_refno.0, target_data.element_name
    );
    println!("候选构件数量: {}", candidates.len());

    // 执行接触检测
    println!("\n接触检测结果:");
    println!("{}", "-".repeat(60));

    for candidate in &candidates {
        if let Some(contact) = detector.check_detailed_contact(
            &target_sctn,
            candidate,
            true, // 包含接近关系
        )? {
            let contact_type_str = match contact.contact_type {
                ContactType::Surface => "表面接触",
                ContactType::Edge => "边缘接触",
                ContactType::Point => "点接触",
                ContactType::Penetration => "穿透",
                ContactType::Proximity => "接近",
                ContactType::None => "无接触",
            };

            println!(
                "{} {} [{}]:",
                candidate.element_type, candidate.refno.0, candidate.element_name
            );
            println!("  接触类型: {}", contact_type_str);
            println!("  距离: {:.3}m", contact.distance);

            if contact.penetration_depth > 0.0 {
                println!("  穿透深度: {:.3}m", contact.penetration_depth);
            }

            if contact.contact_area > 0.0 {
                println!("  接触面积: {:.4}m²", contact.contact_area);
            }

            if !contact.contact_points.is_empty() {
                println!("  接触点数: {}", contact.contact_points.len());
            }
        }
    }

    Ok(())
}

/// 测试支撑关系检测
async fn test_support_detection(test_data: &[TrayTestData]) -> Result<()> {
    println!("\n=== 测试支撑关系检测 ===");

    let target_refno = RefU64::from_str("24383/86525").unwrap();
    let target_data = test_data
        .iter()
        .find(|td| td.refno == target_refno)
        .unwrap();

    // 创建目标SCTN
    let target_sctn = CableTraySection {
        refno: target_data.refno,
        bbox: target_data.bbox.clone(),
        centerline: vec![],
        width: 0.6,
        height: 0.3,
        depth: 10.0,
        direction: Vector3::new(1.0, 0.0, 0.0),
        support_points: vec![Point3::new(102.2, 5.0, 20.3), Point3::new(107.2, 5.0, 20.3)],
        section_type: "SCTN".to_string(),
    };

    let detector = SctnContactDetector::new(0.01)?;

    // 检测支撑关系
    let supports = detector
        .detect_support_relationships(&target_sctn, 10.0)
        .await?;

    println!("SCTN {} 的支撑检测结果:", target_refno.0);

    if supports.is_empty() {
        println!("未检测到支撑关系");

        // 手动检查支架位置
        println!("\n手动分析支架位置:");
        for td in test_data {
            if td.element_type == "SUPPO" {
                let vertical_gap = target_data.bbox.mins.y - td.bbox.maxs.y;
                let x_overlap = td.bbox.maxs.x > target_data.bbox.mins.x
                    && td.bbox.mins.x < target_data.bbox.maxs.x;
                let z_overlap = td.bbox.maxs.z > target_data.bbox.mins.z
                    && td.bbox.mins.z < target_data.bbox.maxs.z;

                println!("  {} [{}]:", td.refno.0, td.element_name);
                println!("    垂直间距: {:.2}m", vertical_gap);
                println!("    X轴重叠: {}", if x_overlap { "是" } else { "否" });
                println!("    Z轴重叠: {}", if z_overlap { "是" } else { "否" });

                if vertical_gap.abs() < 0.1 && x_overlap && z_overlap {
                    println!("    -> 可能存在支撑关系");
                }
            }
        }
    } else {
        for support in &supports {
            println!(
                "  支撑 {}: 类型={:?}, 荷载分布={:.2}",
                support.support.0, support.support_type, support.load_distribution
            );
        }
    }

    Ok(())
}

/// 测试桥架连接关系
async fn test_tray_connections(test_data: &[TrayTestData]) -> Result<()> {
    println!("\n=== 测试桥架连接关系 ===");

    // 收集所有SCTN
    let sctns: Vec<CableTraySection> = test_data
        .iter()
        .filter(|td| td.element_type == "SCTN")
        .map(|td| {
            let direction = if td.element_name.contains("Vertical") {
                Vector3::new(0.0, 1.0, 0.0)
            } else {
                Vector3::new(1.0, 0.0, 0.0)
            };

            CableTraySection {
                refno: td.refno,
                bbox: td.bbox.clone(),
                centerline: vec![],
                width: 0.6,
                height: if td.element_name.contains("Vertical") {
                    3.0
                } else {
                    0.3
                },
                depth: td.bbox.maxs.x - td.bbox.mins.x,
                direction,
                support_points: vec![],
                section_type: "SCTN".to_string(),
            }
        })
        .collect();

    let batch_detector = BatchSctnDetector::new(0.2)?; // 200mm容差

    // 检测连接关系
    let connections = batch_detector.detect_tray_connections(&sctns).await?;

    println!("检测到 {} 个桥架连接关系:", connections.len());

    for conn in &connections {
        let sctn1_name = test_data
            .iter()
            .find(|td| td.refno == conn.section1)
            .map(|td| td.element_name.as_str())
            .unwrap_or("Unknown");

        let sctn2_name = test_data
            .iter()
            .find(|td| td.refno == conn.section2)
            .map(|td| td.element_name.as_str())
            .unwrap_or("Unknown");

        println!("  {} <-> {}", sctn1_name, sctn2_name);
        println!("    类型: {:?}", conn.connection_type);
        println!(
            "    连接点: ({:.1}, {:.1}, {:.1})",
            conn.connection_point.x, conn.connection_point.y, conn.connection_point.z
        );
    }

    Ok(())
}

/// 主测试函数
#[tokio::test]
async fn test_sctn_7999_spatial_integration() -> Result<()> {
    println!("====== SCTN 7999 空间索引集成测试 ======\n");

    // 初始化测试数据
    let test_data = init_db_7999_test_data();
    println!("已创建 {} 条测试数据", test_data.len());

    // SQLite空间索引测试
    #[cfg(feature = "sqlite-index")]
    {
        // 初始化空间索引
        let index = init_spatial_index(&test_data).await?;

        // 测试空间查询
        test_sctn_spatial_query(&index).await?;
    }

    // 测试接触检测
    test_sctn_contact_detection(&test_data).await?;

    // 测试支撑关系
    test_support_detection(&test_data).await?;

    // 测试桥架连接
    test_tray_connections(&test_data).await?;

    println!("\n====== 测试完成 ======");

    Ok(())
}

/// 性能测试：大规模桥架网络
#[tokio::test]
#[ignore]
async fn test_large_scale_tray_network() -> Result<()> {
    use std::time::Instant;

    println!("====== 大规模桥架网络性能测试 ======\n");

    // 生成大规模测试数据
    let mut test_data = Vec::new();

    // 生成100x10的桥架网格
    for i in 0..100 {
        for j in 0..10 {
            let x = i as f32 * 10.0;
            let y = j as f32 * 0.5 + 5.0;

            test_data.push(TrayTestData {
                refno: RefU64(24383_00000 + i * 100 + j),
                bbox: Aabb::new(
                    Point3::new(x, y, 20.0),
                    Point3::new(x + 10.0, y + 0.3, 20.6),
                ),
                element_type: "SCTN".to_string(),
                element_name: format!("Tray_{}_{}", i, j),
            });

            // 每5个桥架添加一个支架
            if i % 5 == 0 {
                test_data.push(TrayTestData {
                    refno: RefU64(24383_90000 + i * 100 + j),
                    bbox: Aabb::new(
                        Point3::new(x + 2.0, 0.0, 20.2),
                        Point3::new(x + 2.4, y, 20.4),
                    ),
                    element_type: "SUPPO".to_string(),
                    element_name: format!("Support_{}_{}", i, j),
                });
            }
        }
    }

    println!("生成测试数据: {} 个构件", test_data.len());

    // 测试SQLite索引性能
    #[cfg(feature = "sqlite-index")]
    {
        let start = Instant::now();
        let index = init_spatial_index(&test_data).await?;
        println!("索引构建时间: {:.2}s", start.elapsed().as_secs_f32());

        // 测试查询性能
        let start = Instant::now();
        let mut total_results = 0;

        for i in (0..100).step_by(10) {
            let refno = RefU64(24383_00000 + i * 100);
            if let Some(bbox) = index.get_aabb(refno)? {
                let query_bbox = Aabb::new(
                    bbox.mins - Vector3::new(5.0, 5.0, 5.0),
                    bbox.maxs + Vector3::new(5.0, 5.0, 5.0),
                );
                let results = index.query_intersect(&query_bbox)?;
                total_results += results.len();
            }
        }

        println!("查询10个区域耗时: {:.2}ms", start.elapsed().as_millis());
        println!("总查询结果: {} 个", total_results);
    }

    // 测试批量接触检测性能
    let sctns: Vec<CableTraySection> = test_data
        .iter()
        .filter(|td| td.element_type == "SCTN")
        .take(100)
        .map(|td| CableTraySection {
            refno: td.refno,
            bbox: td.bbox.clone(),
            centerline: vec![],
            width: 0.6,
            height: 0.3,
            depth: 10.0,
            direction: Vector3::new(1.0, 0.0, 0.0),
            support_points: vec![],
            section_type: "SCTN".to_string(),
        })
        .collect();

    let batch_detector = BatchSctnDetector::new(0.1)?;

    let start = Instant::now();
    let connections = batch_detector.detect_tray_connections(&sctns).await?;
    println!("\n批量检测100个SCTN的连接关系:");
    println!("  耗时: {:.2}s", start.elapsed().as_secs_f32());
    println!("  检测到连接: {} 个", connections.len());

    Ok(())
}
