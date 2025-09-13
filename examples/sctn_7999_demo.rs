/// SCTN 7999 空间接触检测演示
/// 
/// 演示DB 7999中SCTN 24383/86525的空间查询和接触检测
/// 
/// 运行: cargo run --example sctn_7999_demo

use nalgebra::{Point3, Vector3, Isometry3};
use parry3d::bounding_volume::Aabb;
use parry3d::query::contact;
use parry3d::shape::Cuboid;

/// 模拟的SCTN数据
#[derive(Debug, Clone)]
struct SctnData {
    id: String,
    name: String,
    bbox: Aabb,
    width: f32,
    height: f32,
    length: f32,
    element_type: String,
}

/// 创建DB 7999的测试场景
fn create_db_7999_scene() -> Vec<SctnData> {
    vec![
        // 目标SCTN - 24383/86525
        SctnData {
            id: "24383/86525".to_string(),
            name: "主桥架段_001".to_string(),
            bbox: Aabb::new(
                Point3::new(100.0, 5.0, 20.0),
                Point3::new(110.0, 5.3, 20.6),
            ),
            width: 0.6,
            height: 0.3,
            length: 10.0,
            element_type: "SCTN".to_string(),
        },
        // 相邻桥架段
        SctnData {
            id: "24383/86526".to_string(),
            name: "相邻桥架段_002".to_string(),
            bbox: Aabb::new(
                Point3::new(109.9, 5.0, 20.0),
                Point3::new(119.9, 5.3, 20.6),
            ),
            width: 0.6,
            height: 0.3,
            length: 10.0,
            element_type: "SCTN".to_string(),
        },
        // 前段桥架
        SctnData {
            id: "24383/86527".to_string(),
            name: "前段桥架_003".to_string(),
            bbox: Aabb::new(
                Point3::new(90.1, 5.0, 20.0),
                Point3::new(100.1, 5.3, 20.6),
            ),
            width: 0.6,
            height: 0.3,
            length: 10.0,
            element_type: "SCTN".to_string(),
        },
        // 垂直转弯段
        SctnData {
            id: "24383/86528".to_string(),
            name: "垂直转弯段".to_string(),
            bbox: Aabb::new(
                Point3::new(119.8, 5.0, 20.0),
                Point3::new(120.4, 8.0, 20.6),
            ),
            width: 0.6,
            height: 3.0,
            length: 0.6,
            element_type: "SCTN".to_string(),
        },
        // 支架1
        SctnData {
            id: "24383/90001".to_string(),
            name: "支架_001".to_string(),
            bbox: Aabb::new(
                Point3::new(102.0, 0.0, 20.2),
                Point3::new(102.4, 5.0, 20.4),
            ),
            width: 0.4,
            height: 5.0,
            length: 0.2,
            element_type: "SUPPO".to_string(),
        },
        // 支架2
        SctnData {
            id: "24383/90002".to_string(),
            name: "支架_002".to_string(),
            bbox: Aabb::new(
                Point3::new(107.0, 0.0, 20.2),
                Point3::new(107.4, 5.0, 20.4),
            ),
            width: 0.4,
            height: 5.0,
            length: 0.2,
            element_type: "SUPPO".to_string(),
        },
        // 穿越管道
        SctnData {
            id: "24383/50001".to_string(),
            name: "穿越管道_DN200".to_string(),
            bbox: Aabb::new(
                Point3::new(105.0, 5.2, 19.5),
                Point3::new(105.3, 5.5, 21.5),
            ),
            width: 0.3,
            height: 0.3,
            length: 2.0,
            element_type: "PIPE".to_string(),
        },
        // 附近设备
        SctnData {
            id: "24383/60001".to_string(),
            name: "配电柜_001".to_string(),
            bbox: Aabb::new(
                Point3::new(108.0, 4.0, 19.0),
                Point3::new(112.0, 6.0, 22.0),
            ),
            width: 4.0,
            height: 2.0,
            length: 3.0,
            element_type: "EQUI".to_string(),
        },
    ]
}

/// 检测两个构件之间的接触
fn detect_contact(item1: &SctnData, item2: &SctnData, tolerance: f32) -> (String, f32, bool) {
    // 创建形状
    let shape1 = Cuboid::new(Vector3::new(
        item1.width / 2.0,
        item1.height / 2.0,
        item1.length / 2.0,
    ));
    
    let shape2 = Cuboid::new(Vector3::new(
        item2.width / 2.0,
        item2.height / 2.0,
        item2.length / 2.0,
    ));
    
    // 计算位置
    let pos1 = Isometry3::translation(
        item1.bbox.center().x,
        item1.bbox.center().y,
        item1.bbox.center().z,
    );
    
    let pos2 = Isometry3::translation(
        item2.bbox.center().x,
        item2.bbox.center().y,
        item2.bbox.center().z,
    );
    
    // 检测接触
    let distance = (item1.bbox.center() - item2.bbox.center()).norm();
    
    match contact(&pos1, &shape1, &pos2, &shape2, tolerance) {
        Ok(Some(c)) => {
            if c.dist < -tolerance {
                ("穿透".to_string(), c.dist.abs(), true)
            } else if c.dist.abs() < 0.001 {
                ("表面接触".to_string(), 0.0, true)
            } else if c.dist < tolerance {
                ("接近".to_string(), c.dist, true)
            } else {
                ("无接触".to_string(), distance, false)
            }
        }
        _ => ("无接触".to_string(), distance, false)
    }
}

/// 检测支撑关系
fn detect_support(tray: &SctnData, support: &SctnData) -> bool {
    // 垂直对齐检查
    let vertical_gap = (tray.bbox.mins.y - support.bbox.maxs.y).abs();
    if vertical_gap > 0.1 {
        return false;
    }
    
    // 水平重叠检查
    let x_overlap = tray.bbox.maxs.x > support.bbox.mins.x && 
                   tray.bbox.mins.x < support.bbox.maxs.x;
    let z_overlap = tray.bbox.maxs.z > support.bbox.mins.z && 
                   tray.bbox.mins.z < support.bbox.maxs.z;
    
    x_overlap && z_overlap
}

fn main() {
    println!("╔════════════════════════════════════════════════╗");
    println!("║     SCTN 7999 空间接触检测演示系统            ║");
    println!("║     目标: SCTN 24383/86525                    ║");
    println!("╚════════════════════════════════════════════════╝");
    println!();
    
    // 创建场景
    let scene = create_db_7999_scene();
    let target_id = "24383/86525";
    
    // 找到目标SCTN
    let target = scene.iter()
        .find(|item| item.id == target_id)
        .expect("未找到目标SCTN");
    
    println!("【目标SCTN信息】");
    println!("  ID: {}", target.id);
    println!("  名称: {}", target.name);
    println!("  位置: ({:.1}, {:.1}, {:.1}) - ({:.1}, {:.1}, {:.1})",
             target.bbox.mins.x, target.bbox.mins.y, target.bbox.mins.z,
             target.bbox.maxs.x, target.bbox.maxs.y, target.bbox.maxs.z);
    println!("  尺寸: {}mm × {}mm × {}m",
             (target.width * 1000.0) as i32,
             (target.height * 1000.0) as i32,
             target.length);
    println!();
    
    // 空间查询 - 查找附近的构件
    println!("【空间查询结果】");
    println!("查询范围: 目标SCTN周围 1.0m");
    println!("{}", "-".repeat(60));
    
    let query_tolerance = 1.0;
    let mut nearby_items = Vec::new();
    
    for item in &scene {
        if item.id == target_id {
            continue;
        }
        
        let distance = (item.bbox.center() - target.bbox.center()).norm();
        if distance <= 20.0 {  // 20米范围内
            nearby_items.push((item, distance));
        }
    }
    
    // 按距离排序
    nearby_items.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    
    for (item, distance) in &nearby_items {
        println!("{:10} {} [{}] - 距离: {:.2}m",
                 item.element_type,
                 item.id,
                 item.name,
                 distance);
    }
    println!();
    
    // 接触检测
    println!("【接触检测分析】");
    println!("检测容差: 100mm");
    println!("{}", "-".repeat(60));
    
    let contact_tolerance = 0.1; // 100mm
    let mut contacts = Vec::new();
    let mut proximities = Vec::new();
    
    for item in &scene {
        if item.id == target_id {
            continue;
        }
        
        let (contact_type, dist, has_contact) = detect_contact(target, item, contact_tolerance);
        
        if has_contact {
            if contact_type.contains("接近") {
                proximities.push((item, contact_type, dist));
            } else {
                contacts.push((item, contact_type, dist));
            }
        }
    }
    
    // 显示接触
    if !contacts.is_empty() {
        println!("● 接触/碰撞:");
        for (item, contact_type, dist) in &contacts {
            println!("  ⚠️  {} [{}]", item.name, item.id);
            println!("     类型: {}, 距离/深度: {:.3}m", contact_type, dist);
        }
    }
    
    // 显示接近
    if !proximities.is_empty() {
        println!("\n● 接近关系:");
        for (item, _, dist) in &proximities {
            println!("  ℹ️  {} [{}] - 间距: {:.3}m", 
                     item.name, item.id, dist);
        }
    }
    println!();
    
    // 支撑关系检测
    println!("【支撑关系分析】");
    println!("{}", "-".repeat(60));
    
    let mut supports_found = Vec::new();
    
    for item in &scene {
        if item.element_type == "SUPPO" {
            if detect_support(target, item) {
                supports_found.push(item);
            }
        }
    }
    
    if !supports_found.is_empty() {
        println!("检测到 {} 个支撑点:", supports_found.len());
        for support in &supports_found {
            let x_pos = support.bbox.center().x;
            println!("  ✓ {} 在 X={:.1}m 处", support.name, x_pos);
        }
    } else {
        println!("⚠️  未检测到有效支撑");
    }
    println!();
    
    // 桥架连接分析
    println!("【桥架连接拓扑】");
    println!("{}", "-".repeat(60));
    
    let mut connections = Vec::new();
    
    for item in &scene {
        if item.element_type == "SCTN" && item.id != target_id {
            let (contact_type, _, has_contact) = detect_contact(target, item, 0.2);
            if has_contact {
                connections.push((item, contact_type));
            }
        }
    }
    
    if !connections.is_empty() {
        println!("桥架网络连接:");
        for (item, conn_type) in &connections {
            let direction = if item.bbox.center().x < target.bbox.center().x {
                "前段"
            } else if item.bbox.center().x > target.bbox.center().x {
                "后段"
            } else if item.bbox.center().y != target.bbox.center().y {
                "垂直"
            } else {
                "平行"
            };
            
            println!("  {} → {} [{}]", direction, item.name, conn_type);
        }
    }
    println!();
    
    // 统计摘要
    println!("【检测统计摘要】");
    println!("{}", "═".repeat(30));
    println!("  场景构件总数: {}", scene.len());
    println!("  SCTN桥架数: {}", scene.iter().filter(|i| i.element_type == "SCTN").count());
    println!("  支架数量: {}", scene.iter().filter(|i| i.element_type == "SUPPO").count());
    println!("  检测到接触: {}", contacts.len());
    println!("  检测到接近: {}", proximities.len());
    println!("  有效支撑: {}", supports_found.len());
    println!("  桥架连接: {}", connections.len());
    println!();
    
    // 建议
    println!("【系统建议】");
    println!("{}", "-".repeat(60));
    
    if supports_found.len() < 2 {
        println!("⚠️  支撑点不足，建议增加支架");
    }
    
    if !contacts.is_empty() {
        println!("⚠️  存在碰撞风险，请检查布置");
    }
    
    if connections.is_empty() {
        println!("ℹ️  该桥架段似乎是独立的");
    } else {
        println!("✓ 桥架连接正常");
    }
    
    println!("\n演示完成！");
}