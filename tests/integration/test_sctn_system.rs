use std::sync::Arc;
use std::time::Instant;
use anyhow::Result;
use nalgebra::{Point3, Vector3};
use parry3d::bounding_volume::Aabb;
use aios_core::pdms_types::RefU64;

#[cfg(feature = "grpc")]
use aios_database::grpc_service::{
    sctn_contact_detector::{
        SctnContactDetector, BatchSctnDetector, CableTraySection,
        ContactType, SupportType,
    },
    sctn_geometry_extractor::{
        SctnGeometryExtractor, SctnGeometryAnalyzer,
    },
    sctn_raycast_detector::{
        SctnRaycastDetector, AdvancedRaycastAnalyzer,
        SupportCandidate,
    },
    sctn_path_analyzer::{
        SctnPathAnalyzer, PathOptimizer,
    },
    sctn_collision_optimizer::{
        SctnCollisionOptimizer, AdvancedCollisionAnalyzer,
    },
    sctn_visualizer::SctnVisualizer,
};

/// 集成测试套件
#[cfg(feature = "grpc")]
mod integration_tests {
    use super::*;

    /// 测试完整的SCTN处理流程
    #[tokio::test]
    async fn test_complete_sctn_workflow() -> Result<()> {
        println!("=== SCTN完整工作流测试 ===\n");
        
        // 1. 创建测试数据
        let sections = create_test_sections();
        println!("创建了 {} 个测试SCTN", sections.len());
        
        // 2. 接触检测
        let detector = SctnContactDetector::new(0.01)?;
        let mut total_contacts = 0;
        
        for section in &sections {
            let contacts = detector.detect_sctn_contacts(
                section,
                &["PIPE".to_string(), "EQUI".to_string()],
                true,
            ).await?;
            total_contacts += contacts.len();
        }
        
        println!("检测到 {} 个接触关系", total_contacts);
        
        // 3. 支撑检测
        let raycast_detector = SctnRaycastDetector::new(5.0)?;
        let support_candidates = create_support_candidates();
        
        for section in &sections {
            let supports = raycast_detector.detect_supports_by_raycast(
                section,
                &support_candidates,
            ).await?;
            
            if !supports.is_empty() {
                println!("SCTN {} 有 {} 个支撑点", section.refno.0, supports.len());
            }
        }
        
        // 4. 路径分析
        let path_analyzer = SctnPathAnalyzer::new(0.1);
        let network = path_analyzer.build_tray_network(&sections);
        let connectivity = path_analyzer.analyze_connectivity(&network);
        
        println!("\n路径分析结果:");
        println!("- 连通分量数: {}", connectivity.num_components);
        println!("- 最大分量大小: {}", connectivity.largest_component.len());
        println!("- 孤立节点数: {}", connectivity.isolated_sections.len());
        
        // 5. 碰撞优化
        let mut collision_optimizer = SctnCollisionOptimizer::new(0.01);
        collision_optimizer.build_bvh(sections.clone());
        let collisions = collision_optimizer.batch_collision_detection();
        
        println!("\n碰撞检测结果:");
        println!("- 检测到 {} 个碰撞对", collisions.len());
        
        if !collisions.is_empty() {
            let resolutions = collision_optimizer.auto_resolve_collisions(10);
            println!("- 自动解决了 {} 个碰撞", resolutions.len());
        }
        
        Ok(())
    }

    /// 测试性能和扩展性
    #[tokio::test]
    async fn test_performance_scalability() -> Result<()> {
        println!("=== 性能和扩展性测试 ===\n");
        
        let test_sizes = vec![10, 50, 100, 500];
        
        for size in test_sizes {
            let sections = create_large_test_set(size);
            
            // 测试接触检测性能
            let detector = BatchSctnDetector::new(0.01)?;
            let start = Instant::now();
            
            let results = detector.detect_batch(
                sections.clone(),
                &[],
            ).await?;
            
            let elapsed = start.elapsed();
            let total_contacts: usize = results.iter().map(|(_, c)| c.len()).sum();
            
            println!("规模 {}: 处理时间 {:.2}s, 检测到 {} 个接触",
                size, elapsed.as_secs_f32(), total_contacts);
            
            // 测试路径分析性能
            let path_analyzer = SctnPathAnalyzer::new(0.1);
            let start = Instant::now();
            
            let network = path_analyzer.build_tray_network(&sections);
            let elapsed = start.elapsed();
            
            println!("  网络构建: {:.3}s, 节点数: {}, 边数: {}",
                elapsed.as_secs_f32(),
                network.graph.node_count(),
                network.graph.edge_count());
            
            // 测试碰撞优化性能
            let mut optimizer = SctnCollisionOptimizer::new(0.01);
            let start = Instant::now();
            
            optimizer.build_bvh(sections);
            let collisions = optimizer.batch_collision_detection();
            let elapsed = start.elapsed();
            
            println!("  碰撞检测: {:.3}s, 碰撞数: {}",
                elapsed.as_secs_f32(), collisions.len());
        }
        
        Ok(())
    }

    /// 测试边界情况和错误处理
    #[tokio::test]
    async fn test_edge_cases() -> Result<()> {
        println!("=== 边界情况测试 ===\n");
        
        // 空数据集
        let empty: Vec<CableTraySection> = vec![];
        let detector = BatchSctnDetector::new(0.01)?;
        let results = detector.detect_batch(empty.clone(), &[]).await?;
        assert_eq!(results.len(), 0);
        println!("✓ 空数据集处理正常");
        
        // 单个SCTN
        let single = vec![create_test_sections()[0].clone()];
        let results = detector.detect_batch(single, &[]).await?;
        assert_eq!(results.len(), 1);
        println!("✓ 单个SCTN处理正常");
        
        // 完全重叠的SCTN
        let overlapping = vec![
            create_sctn_at(RefU64(1001), 0.0, 0.0, 0.0),
            create_sctn_at(RefU64(1002), 0.0, 0.0, 0.0),
        ];
        let results = detector.detect_batch(overlapping, &[]).await?;
        let contacts = &results[0].1;
        assert!(!contacts.is_empty());
        assert!(matches!(contacts[0].1.contact_type, ContactType::Penetration));
        println!("✓ 完全重叠检测正常");
        
        // 极大距离的SCTN
        let far_apart = vec![
            create_sctn_at(RefU64(2001), 0.0, 0.0, 0.0),
            create_sctn_at(RefU64(2002), 1000.0, 1000.0, 1000.0),
        ];
        let results = detector.detect_batch(far_apart, &[]).await?;
        let contacts = &results[0].1;
        assert!(contacts.is_empty());
        println!("✓ 远距离SCTN处理正常");
        
        Ok(())
    }

    /// 测试可视化输出
    #[tokio::test]
    async fn test_visualization_output() -> Result<()> {
        println!("=== 可视化输出测试 ===\n");
        
        let sections = create_test_sections();
        let detector = SctnContactDetector::new(0.01)?;
        
        // 收集接触信息
        let mut all_contacts = Vec::new();
        for section in &sections {
            let contacts = detector.detect_sctn_contacts(
                section,
                &[],
                true,
            ).await?;
            all_contacts.extend(contacts);
        }
        
        // 创建可视化器
        let visualizer = SctnVisualizer::new("test_output");
        
        // 导出各种格式
        visualizer.export_to_obj(&sections, "sctn_model.obj")?;
        println!("✓ 导出OBJ模型: test_output/sctn_model.obj");
        
        visualizer.export_to_html(
            &sections,
            &all_contacts,
            &[],
            "sctn_visualization.html"
        )?;
        println!("✓ 导出HTML可视化: test_output/sctn_visualization.html");
        
        visualizer.export_to_csv(
            &sections,
            &all_contacts,
            "sctn_data.csv"
        )?;
        println!("✓ 导出CSV数据: test_output/sctn_data.csv");
        
        Ok(())
    }

    /// 测试真实场景模拟
    #[tokio::test]
    async fn test_realistic_scenario() -> Result<()> {
        println!("=== 真实场景模拟测试 ===\n");
        
        // 创建一个真实的桥架布局
        let mut sections = Vec::new();
        
        // 主干线（水平）
        for i in 0..10 {
            sections.push(CableTraySection {
                refno: RefU64(1000 + i),
                bbox: Aabb::new(
                    Point3::new(i as f32 * 3.0, 3.0, 0.0),
                    Point3::new((i + 1) as f32 * 3.0, 3.1, 0.3),
                ),
                centerline: vec![],
                width: 0.3,
                height: 0.1,
                depth: 3.0,
                direction: Vector3::new(1.0, 0.0, 0.0),
                support_points: vec![],
                section_type: "SCTN".to_string(),
            });
        }
        
        // 分支（垂直）
        for i in 0..5 {
            sections.push(CableTraySection {
                refno: RefU64(2000 + i),
                bbox: Aabb::new(
                    Point3::new(15.0, 3.0, i as f32 * 3.0),
                    Point3::new(15.3, 3.1, (i + 1) as f32 * 3.0),
                ),
                centerline: vec![],
                width: 0.3,
                height: 0.1,
                depth: 3.0,
                direction: Vector3::new(0.0, 0.0, 1.0),
                support_points: vec![],
                section_type: "SCTN".to_string(),
            });
        }
        
        // 上升段
        for i in 0..3 {
            sections.push(CableTraySection {
                refno: RefU64(3000 + i),
                bbox: Aabb::new(
                    Point3::new(15.0, 3.0 + i as f32 * 1.0, 15.0),
                    Point3::new(15.3, 3.1 + (i + 1) as f32 * 1.0, 15.3),
                ),
                centerline: vec![],
                width: 0.3,
                height: 0.3,
                depth: 1.0,
                direction: Vector3::new(0.0, 1.0, 0.0),
                support_points: vec![],
                section_type: "SCTN".to_string(),
            });
        }
        
        println!("创建了真实场景: {} 个SCTN", sections.len());
        
        // 分析路径
        let analyzer = SctnPathAnalyzer::new(0.5);
        let network = analyzer.build_tray_network(&sections);
        
        // 查找从起点到终点的路径
        if let Some(path) = analyzer.find_shortest_path(
            &network,
            RefU64(1000),
            RefU64(3002),
        ) {
            println!("找到路径: {} 段, 总长度: {:.2}m", 
                path.sections.len(), path.total_length);
            
            // 分析路径复杂度
            let complexity = analyzer.analyze_path_complexity(&path, &sections);
            println!("路径复杂度分析:");
            println!("  - 转弯数: {}", complexity.num_turns);
            println!("  - 高程变化: {}", complexity.num_elevation_changes);
            println!("  - 总转角: {:.1}°", complexity.total_angle_degrees);
            println!("  - 难度: {:?}", complexity.difficulty);
        }
        
        // 检测环路
        let loops = analyzer.detect_loops(&network);
        println!("检测到 {} 个环路", loops.len());
        
        // 碰撞分析
        let mut collision_analyzer = AdvancedCollisionAnalyzer::new(0.01);
        let hotspots = collision_analyzer.analyze_collision_hotspots(sections.clone());
        
        println!("\n碰撞热点分析:");
        println!("  - 总碰撞数: {}", hotspots.total_collisions);
        for (i, hotspot) in hotspots.hotspots.iter().take(3).enumerate() {
            println!("  - 热点{}: SCTN {} ({} 次碰撞)", 
                i + 1, hotspot.refno.0, hotspot.collision_count);
        }
        
        // 优化布局
        let optimization = collision_analyzer.optimize_layout(sections);
        println!("\n布局优化结果:");
        println!("  - 初始碰撞: {}", optimization.initial_collision_count);
        println!("  - 优化后: {}", optimization.final_collision_count);
        println!("  - 改善率: {:.1}%", optimization.improvement_percentage);
        
        Ok(())
    }

    // 辅助函数

    fn create_test_sections() -> Vec<CableTraySection> {
        vec![
            create_sctn_at(RefU64(1001), 0.0, 2.0, 0.0),
            create_sctn_at(RefU64(1002), 2.9, 2.0, 0.0),
            create_sctn_at(RefU64(1003), 5.8, 2.0, 0.0),
            create_sctn_at(RefU64(1004), 8.7, 2.0, 0.0),
            create_sctn_at(RefU64(1005), 11.6, 2.0, 0.0),
        ]
    }

    fn create_sctn_at(refno: RefU64, x: f32, y: f32, z: f32) -> CableTraySection {
        CableTraySection {
            refno,
            bbox: Aabb::new(
                Point3::new(x, y, z),
                Point3::new(x + 3.0, y + 0.1, z + 0.3),
            ),
            centerline: vec![
                Point3::new(x, y + 0.05, z + 0.15),
                Point3::new(x + 3.0, y + 0.05, z + 0.15),
            ],
            width: 0.3,
            height: 0.1,
            depth: 3.0,
            direction: Vector3::new(1.0, 0.0, 0.0),
            support_points: vec![],
            section_type: "SCTN".to_string(),
        }
    }

    fn create_large_test_set(count: usize) -> Vec<CableTraySection> {
        let mut sections = Vec::new();
        let grid_size = (count as f32).sqrt().ceil() as usize;
        
        for i in 0..count {
            let row = i / grid_size;
            let col = i % grid_size;
            
            sections.push(CableTraySection {
                refno: RefU64(1000 + i as u64),
                bbox: Aabb::new(
                    Point3::new(col as f32 * 3.5, 2.0, row as f32 * 0.5),
                    Point3::new((col + 1) as f32 * 3.5 - 0.5, 2.1, row as f32 * 0.5 + 0.3),
                ),
                centerline: vec![],
                width: 0.3,
                height: 0.1,
                depth: 3.0,
                direction: Vector3::new(1.0, 0.0, 0.0),
                support_points: vec![],
                section_type: "SCTN".to_string(),
            });
        }
        
        sections
    }

    fn create_support_candidates() -> Vec<SupportCandidate> {
        vec![
            SupportCandidate {
                refno: RefU64(5001),
                bbox: Aabb::new(
                    Point3::new(1.4, 0.0, 0.1),
                    Point3::new(1.6, 2.0, 0.2),
                ),
                element_type: "SUPPO".to_string(),
                attributes: std::collections::HashMap::new(),
            },
            SupportCandidate {
                refno: RefU64(5002),
                bbox: Aabb::new(
                    Point3::new(4.4, 0.0, 0.1),
                    Point3::new(4.6, 2.0, 0.2),
                ),
                element_type: "SUPPO".to_string(),
                attributes: std::collections::HashMap::new(),
            },
        ]
    }
}