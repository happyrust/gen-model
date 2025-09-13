/// 使用真实数据的SCTN接触检测测试
use std::str::FromStr;
use std::sync::Arc;
use anyhow::Result;
use aios_core::pdms_types::RefU64;

use crate::grpc_service::sctn_contact_detector::{
    SctnContactDetector, BatchSctnDetector, ContactType,
};
use crate::grpc_service::sctn_geometry_extractor::SctnGeometryExtractor;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::data_interface::interface::PdmsDataInterface;

/// 获取测试用的数据库管理器
async fn get_test_db_manager() -> Arc<AiosDBManager> {
    // 从配置文件读取数据库连接信息
    let config = crate::config::DbConfig::from_file("DbOption.toml")
        .expect("Failed to load database config");
    
    let manager = AiosDBManager::new(config)
        .await
        .expect("Failed to create database manager");
    
    Arc::new(manager)
}

/// 测试使用真实数据的SCTN几何提取
#[tokio::test]
async fn test_real_sctn_geometry_extraction() -> Result<()> {
    let db_manager = get_test_db_manager().await;
    let extractor = SctnGeometryExtractor::new(db_manager.clone());
    
    // 使用真实的SCTN参考号
    let sctn_refno = RefU64::from_str("24383/95023")?;
    
    // 提取真实的SCTN几何信息
    match extractor.extract_sctn_geometry(sctn_refno).await {
        Ok(sctn) => {
            println!("成功提取SCTN几何信息:");
            println!("  RefNo: {}", sctn.refno.0);
            println!("  宽度: {:.3}m", sctn.width);
            println!("  高度: {:.3}m", sctn.height);
            println!("  深度: {:.3}m", sctn.depth);
            println!("  包围盒: min({:.2}, {:.2}, {:.2}) max({:.2}, {:.2}, {:.2})",
                sctn.bbox.mins.x, sctn.bbox.mins.y, sctn.bbox.mins.z,
                sctn.bbox.maxs.x, sctn.bbox.maxs.y, sctn.bbox.maxs.z);
            println!("  中心线点数: {}", sctn.centerline.len());
            println!("  支撑点数: {}", sctn.support_points.len());
            
            assert!(sctn.width > 0.0, "宽度应该大于0");
            assert!(sctn.height > 0.0, "高度应该大于0");
            assert!(sctn.depth > 0.0, "深度应该大于0");
        }
        Err(e) => {
            eprintln!("提取SCTN几何信息失败: {}", e);
            return Err(e);
        }
    }
    
    Ok(())
}

/// 测试使用真实数据的SCTN接触检测
#[tokio::test]
async fn test_real_sctn_contact_detection() -> Result<()> {
    let db_manager = get_test_db_manager().await;
    let extractor = SctnGeometryExtractor::new(db_manager.clone());
    
    // 创建带数据库管理器的检测器
    let detector = SctnContactDetector::with_db_manager(0.01, db_manager.clone())?;
    
    // 使用真实的SCTN参考号
    let sctn_refno = RefU64::from_str("24383/95023")?;
    
    // 提取SCTN几何信息
    let sctn = extractor.extract_sctn_geometry(sctn_refno).await?;
    
    println!("开始检测SCTN {} 的接触关系...", sctn_refno.0);
    
    // 执行接触检测，查找与管道(PIPE)和设备(EQUI)的接触
    let contacts = detector.detect_sctn_contacts(
        &sctn,
        &["PIPE".to_string(), "EQUI".to_string(), "STRU".to_string()],
        true,  // 包含接近关系
    ).await?;
    
    println!("检测到 {} 个接触/接近关系:", contacts.len());
    
    for (refno, contact) in &contacts {
        println!("\n接触对象: {}", refno.0);
        println!("  接触类型: {:?}", contact.contact_type);
        println!("  距离: {:.3}m", contact.distance);
        
        match contact.contact_type {
            ContactType::Surface => {
                println!("  接触面积: {:.4}m²", contact.contact_area);
            }
            ContactType::Penetration => {
                println!("  穿透深度: {:.3}m", contact.penetration_depth);
            }
            ContactType::Proximity => {
                println!("  接近距离: {:.3}m", contact.distance);
            }
            _ => {}
        }
        
        if !contact.contact_points.is_empty() {
            println!("  接触点数: {}", contact.contact_points.len());
            for (i, point) in contact.contact_points.iter().enumerate() {
                println!("    点{}: ({:.2}, {:.2}, {:.2})", 
                    i + 1, point.x, point.y, point.z);
            }
        }
    }
    
    Ok(())
}

/// 测试批量SCTN真实数据处理
#[tokio::test]
async fn test_real_batch_sctn_detection() -> Result<()> {
    let db_manager = get_test_db_manager().await;
    let extractor = SctnGeometryExtractor::new(db_manager.clone());
    
    // 使用真实的BRAN参考号
    let bran_refno = RefU64::from_str("24383/95000")?;
    
    println!("提取桥架分支 {} 下的所有SCTN...", bran_refno.0);
    
    // 提取分支下的所有SCTN
    let sections = extractor.extract_branch_sections(bran_refno).await?;
    
    println!("找到 {} 个SCTN截面", sections.len());
    
    if sections.is_empty() {
        println!("警告: 未找到SCTN，可能需要检查参考号");
        return Ok(());
    }
    
    // 创建批量检测器
    let batch_detector = BatchSctnDetector::new(0.01)?;
    
    // 批量检测
    let results = batch_detector.detect_batch(
        sections.clone(),
        &["PIPE".to_string(), "EQUI".to_string(), "STRU".to_string()],
    ).await?;
    
    println!("\n批量检测结果:");
    for (refno, contacts) in &results {
        println!("SCTN {}: 检测到 {} 个接触", refno.0, contacts.len());
        
        // 统计不同类型的接触
        let mut surface_count = 0;
        let mut proximity_count = 0;
        let mut penetration_count = 0;
        
        for (_, contact) in contacts {
            match contact.contact_type {
                ContactType::Surface => surface_count += 1,
                ContactType::Proximity => proximity_count += 1,
                ContactType::Penetration => penetration_count += 1,
                _ => {}
            }
        }
        
        if surface_count > 0 {
            println!("  - 表面接触: {}", surface_count);
        }
        if proximity_count > 0 {
            println!("  - 接近关系: {}", proximity_count);
        }
        if penetration_count > 0 {
            println!("  - 穿透: {}", penetration_count);
        }
    }
    
    // 检测桥架间的连接关系
    println!("\n检测桥架间连接关系...");
    let connections = batch_detector.detect_tray_connections(&sections).await?;
    
    println!("检测到 {} 个连接关系:", connections.len());
    for conn in &connections {
        println!("  SCTN {} <-> SCTN {}: {:?}",
            conn.section1.0,
            conn.section2.0,
            conn.connection_type
        );
    }
    
    Ok(())
}

/// 测试真实的支撑关系检测
#[tokio::test]
async fn test_real_support_detection() -> Result<()> {
    let db_manager = get_test_db_manager().await;
    let extractor = SctnGeometryExtractor::new(db_manager.clone());
    let detector = SctnContactDetector::with_db_manager(0.001, db_manager.clone())?;
    
    // 使用真实的SCTN参考号
    let sctn_refno = RefU64::from_str("24383/95023")?;
    
    // 提取SCTN几何信息
    let sctn = extractor.extract_sctn_geometry(sctn_refno).await?;
    
    println!("检测SCTN {} 的支撑关系...", sctn_refno.0);
    
    // 检测支撑关系
    let supports = detector.detect_support_relationships(&sctn, 5.0).await?;
    
    println!("检测到 {} 个支撑点:", supports.len());
    
    for support in &supports {
        println!("\n支撑构件: {}", support.support.0);
        println!("  支撑类型: {:?}", support.support_type);
        println!("  接触点: ({:.2}, {:.2}, {:.2})",
            support.contact_point.x,
            support.contact_point.y,
            support.contact_point.z
        );
        println!("  荷载分布系数: {:.2}", support.load_distribution);
    }
    
    // 验证支撑关系的合理性
    for support in &supports {
        // 支撑点应该在桥架下方
        assert!(
            support.contact_point.y <= sctn.bbox.maxs.y,
            "支撑点应该在桥架下方或同高度"
        );
    }
    
    Ok(())
}

/// 测试不同容差对真实数据检测的影响
#[tokio::test]
async fn test_real_data_tolerance_impact() -> Result<()> {
    let db_manager = get_test_db_manager().await;
    let extractor = SctnGeometryExtractor::new(db_manager.clone());
    
    let sctn_refno = RefU64::from_str("24383/95023")?;
    let sctn = extractor.extract_sctn_geometry(sctn_refno).await?;
    
    let tolerances = vec![0.001, 0.01, 0.05, 0.1];
    
    println!("测试不同容差对检测结果的影响:");
    
    for tolerance in tolerances {
        let detector = SctnContactDetector::with_db_manager(tolerance, db_manager.clone())?;
        
        let contacts = detector.detect_sctn_contacts(
            &sctn,
            &[],  // 不限制类型
            true,
        ).await?;
        
        let proximity_count = contacts.iter()
            .filter(|(_, c)| matches!(c.contact_type, ContactType::Proximity))
            .count();
        
        let contact_count = contacts.iter()
            .filter(|(_, c)| matches!(c.contact_type, 
                ContactType::Surface | ContactType::Edge | ContactType::Point))
            .count();
        
        println!("  容差 {:.3}m: 总计 {} 个关系 (接触: {}, 接近: {})",
            tolerance, contacts.len(), contact_count, proximity_count);
    }
    
    Ok(())
}

/// 测试查询特定类型构件的接触
#[tokio::test]
async fn test_real_type_specific_detection() -> Result<()> {
    let db_manager = get_test_db_manager().await;
    let extractor = SctnGeometryExtractor::new(db_manager.clone());
    let detector = SctnContactDetector::with_db_manager(0.01, db_manager.clone())?;
    
    let sctn_refno = RefU64::from_str("24383/95023")?;
    let sctn = extractor.extract_sctn_geometry(sctn_refno).await?;
    
    // 测试不同类型的接触检测
    let test_types = vec![
        ("PIPE", "管道"),
        ("EQUI", "设备"),
        ("STRU", "结构"),
        ("SUPPO", "支架"),
    ];
    
    println!("按构件类型检测接触:");
    
    for (type_code, type_name) in test_types {
        let contacts = detector.detect_sctn_contacts(
            &sctn,
            &[type_code.to_string()],
            false,  // 不包含接近关系
        ).await?;
        
        println!("  {} ({}): {} 个接触", type_name, type_code, contacts.len());
        
        if !contacts.is_empty() {
            // 显示第一个接触的详细信息
            let (refno, contact) = &contacts[0];
            println!("    示例: RefNo {} - {:?}, 距离 {:.3}m",
                refno.0, contact.contact_type, contact.distance);
        }
    }
    
    Ok(())
}