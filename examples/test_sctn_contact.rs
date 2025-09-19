use nalgebra::Isometry3;
/// 独立的SCTN接触检测测试示例
///
/// 运行方式: cargo run --example test_sctn_contact
use nalgebra::{Point3, Vector3};
use parry3d::bounding_volume::Aabb;
use parry3d::query::{Contact, contact};
use parry3d::shape::Cuboid;

#[derive(Debug, Clone)]
pub struct SimpleSCTN {
    pub id: u64,
    pub bbox: Aabb,
    pub width: f32,
    pub height: f32,
    pub depth: f32,
}

#[derive(Debug, Clone)]
pub enum ContactType {
    None,
    Surface,
    Edge,
    Point,
    Penetration,
    Proximity,
}

/// 简单的接触检测函数
fn detect_contact(sctn1: &SimpleSCTN, sctn2: &SimpleSCTN, tolerance: f32) -> (ContactType, f32) {
    // 创建立方体形状
    let cuboid1 = Cuboid::new(Vector3::new(
        sctn1.width / 2.0,
        sctn1.height / 2.0,
        sctn1.depth / 2.0,
    ));

    let cuboid2 = Cuboid::new(Vector3::new(
        sctn2.width / 2.0,
        sctn2.height / 2.0,
        sctn2.depth / 2.0,
    ));

    // 计算位置
    let pos1 = Isometry3::translation(
        sctn1.bbox.center().x,
        sctn1.bbox.center().y,
        sctn1.bbox.center().z,
    );

    let pos2 = Isometry3::translation(
        sctn2.bbox.center().x,
        sctn2.bbox.center().y,
        sctn2.bbox.center().z,
    );

    // 检测接触
    let contact_result = contact(&pos1, &cuboid1, &pos2, &cuboid2, tolerance);

    match contact_result {
        Ok(Some(c)) => {
            let contact_type = if c.dist < -tolerance {
                ContactType::Penetration
            } else if c.dist.abs() < 0.001 {
                ContactType::Surface
            } else if c.dist < tolerance {
                ContactType::Proximity
            } else {
                ContactType::None
            };
            (contact_type, c.dist.abs())
        }
        _ => {
            let distance = (sctn1.bbox.center() - sctn2.bbox.center()).norm();
            (ContactType::None, distance)
        }
    }
}

/// 检测桥架支撑关系
fn detect_support(tray: &SimpleSCTN, support: &SimpleSCTN) -> bool {
    // 检查支撑是否在桥架下方
    let tray_bottom = tray.bbox.mins.y;
    let support_top = support.bbox.maxs.y;

    // 垂直距离检查
    if (support_top - tray_bottom).abs() > 0.1 {
        return false;
    }

    // 水平重叠检查
    let x_overlap =
        tray.bbox.maxs.x > support.bbox.mins.x && tray.bbox.mins.x < support.bbox.maxs.x;
    let z_overlap =
        tray.bbox.maxs.z > support.bbox.mins.z && tray.bbox.mins.z < support.bbox.maxs.z;

    x_overlap && z_overlap
}

fn main() {
    println!("=== SCTN 空间接触检测示例 ===\n");

    // 创建测试场景：3个桥架截面
    let sctn1 = SimpleSCTN {
        id: 1001,
        bbox: Aabb::new(Point3::new(0.0, 2.0, 0.0), Point3::new(3.0, 2.1, 0.3)),
        width: 0.3,
        height: 0.1,
        depth: 3.0,
    };

    let sctn2 = SimpleSCTN {
        id: 1002,
        bbox: Aabb::new(Point3::new(2.9, 2.0, 0.0), Point3::new(5.9, 2.1, 0.3)),
        width: 0.3,
        height: 0.1,
        depth: 3.0,
    };

    let sctn3 = SimpleSCTN {
        id: 1003,
        bbox: Aabb::new(Point3::new(5.9, 2.0, 0.0), Point3::new(6.2, 5.0, 0.3)),
        width: 0.3,
        height: 0.3,
        depth: 3.0,
    };

    // 创建支撑
    let support1 = SimpleSCTN {
        id: 2001,
        bbox: Aabb::new(Point3::new(1.4, 0.0, 0.1), Point3::new(1.6, 2.0, 0.2)),
        width: 0.2,
        height: 2.0,
        depth: 0.1,
    };

    let support2 = SimpleSCTN {
        id: 2002,
        bbox: Aabb::new(Point3::new(4.4, 0.0, 0.1), Point3::new(4.6, 2.0, 0.2)),
        width: 0.2,
        height: 2.0,
        depth: 0.1,
    };

    // 测试接触检测
    println!("1. 桥架间接触检测:");
    println!("-------------------");

    let (contact_type_1_2, dist_1_2) = detect_contact(&sctn1, &sctn2, 0.1);
    println!(
        "SCTN {} <-> SCTN {}: {:?}, 距离: {:.3}m",
        sctn1.id, sctn2.id, contact_type_1_2, dist_1_2
    );

    let (contact_type_2_3, dist_2_3) = detect_contact(&sctn2, &sctn3, 0.1);
    println!(
        "SCTN {} <-> SCTN {}: {:?}, 距离: {:.3}m",
        sctn2.id, sctn3.id, contact_type_2_3, dist_2_3
    );

    let (contact_type_1_3, dist_1_3) = detect_contact(&sctn1, &sctn3, 0.1);
    println!(
        "SCTN {} <-> SCTN {}: {:?}, 距离: {:.3}m",
        sctn1.id, sctn3.id, contact_type_1_3, dist_1_3
    );

    // 测试支撑检测
    println!("\n2. 桥架支撑检测:");
    println!("----------------");

    if detect_support(&sctn1, &support1) {
        println!("✓ SCTN {} 由支架 {} 支撑", sctn1.id, support1.id);
    } else {
        println!("✗ SCTN {} 与支架 {} 无支撑关系", sctn1.id, support1.id);
    }

    if detect_support(&sctn2, &support1) {
        println!("✓ SCTN {} 由支架 {} 支撑", sctn2.id, support1.id);
    } else {
        println!("✗ SCTN {} 与支架 {} 无支撑关系", sctn2.id, support1.id);
    }

    if detect_support(&sctn2, &support2) {
        println!("✓ SCTN {} 由支架 {} 支撑", sctn2.id, support2.id);
    } else {
        println!("✗ SCTN {} 与支架 {} 无支撑关系", sctn2.id, support2.id);
    }

    // 测试不同容差值的影响
    println!("\n3. 容差影响测试:");
    println!("----------------");

    let tolerances = vec![0.001, 0.01, 0.05, 0.1, 0.2];
    for tolerance in tolerances {
        let (ct, _) = detect_contact(&sctn1, &sctn2, tolerance);
        println!("容差 {:.3}m: SCTN 1001-1002 接触类型: {:?}", tolerance, ct);
    }

    // 批量检测示例
    println!("\n4. 批量接触检测统计:");
    println!("--------------------");

    let all_sctns = vec![sctn1.clone(), sctn2.clone(), sctn3.clone()];
    let mut contact_count = 0;
    let mut proximity_count = 0;

    for i in 0..all_sctns.len() {
        for j in i + 1..all_sctns.len() {
            let (ct, _) = detect_contact(&all_sctns[i], &all_sctns[j], 0.1);
            match ct {
                ContactType::Surface | ContactType::Edge | ContactType::Point => contact_count += 1,
                ContactType::Proximity => proximity_count += 1,
                _ => {}
            }
        }
    }

    println!("总SCTN数: {}", all_sctns.len());
    println!("接触对数: {}", contact_count);
    println!("接近对数: {}", proximity_count);

    // 性能测试
    println!("\n5. 性能测试:");
    println!("-----------");

    use std::time::Instant;

    let start = Instant::now();
    let iterations = 10000;

    for _ in 0..iterations {
        detect_contact(&sctn1, &sctn2, 0.01);
    }

    let elapsed = start.elapsed();
    println!(
        "执行 {} 次接触检测耗时: {:.2}ms",
        iterations,
        elapsed.as_secs_f32() * 1000.0
    );
    println!(
        "平均每次: {:.3}μs",
        elapsed.as_micros() as f32 / iterations as f32
    );

    println!("\n=== 测试完成 ===");
}
