use aios_core::RefU64;
use aios_database::spatial_index::SqliteSpatialIndex;
use nalgebra::Point3;
use parry3d::bounding_volume::Aabb;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 空间查询功能测试\n");

    // 检查是否启用 SQLite 索引
    if !SqliteSpatialIndex::is_enabled() {
        println!("⚠️  SQLite 索引未启用");
        println!("请在 DbOption.toml 中设置: enable_sqlite_rtree = true");
        return Ok(());
    }

    // 创建空间索引
    let spatial_index = SqliteSpatialIndex::with_default_path()?;
    println!("✅ SQLite 空间索引已初始化");

    // 插入测试数据
    println!("\n📦 插入测试数据...");
    let test_data = vec![
        (
            RefU64(111200001),
            create_aabb(0.0, 0.0, 0.0, 100.0, 100.0, 100.0),
            Some("PIPE_001"),
        ),
        (
            RefU64(111200002),
            create_aabb(50.0, 50.0, 50.0, 150.0, 150.0, 150.0),
            Some("VALVE_001"),
        ),
        (
            RefU64(111200003),
            create_aabb(200.0, 200.0, 200.0, 300.0, 300.0, 300.0),
            Some("PUMP_001"),
        ),
        (
            RefU64(111200004),
            create_aabb(-100.0, -100.0, -100.0, 0.0, 0.0, 0.0),
            Some("TANK_001"),
        ),
        (
            RefU64(111200005),
            create_aabb(1000.0, 1000.0, 1000.0, 1100.0, 1100.0, 1100.0),
            Some("EQUIP_001"),
        ),
    ];

    for (refno, aabb, noun) in &test_data {
        spatial_index.insert_aabb(*refno, aabb, noun.as_deref())?;
        println!("  插入: RefNo {} - {}", refno.0, noun.unwrap_or("Unknown"));
    }

    // 测试1: 边界框查询
    println!("\n📐 测试边界框查询...");
    let query_box = create_aabb(-50.0, -50.0, -50.0, 120.0, 120.0, 120.0);
    println!(
        "  查询范围: ({:.0}, {:.0}, {:.0}) 到 ({:.0}, {:.0}, {:.0})",
        query_box.mins.x,
        query_box.mins.y,
        query_box.mins.z,
        query_box.maxs.x,
        query_box.maxs.y,
        query_box.maxs.z
    );

    let results = spatial_index.query_intersect(&query_box)?;
    println!("  找到 {} 个相交对象:", results.len());
    for refno in &results {
        if let Ok(Some(aabb)) = spatial_index.get_aabb(*refno) {
            println!(
                "    - RefNo {}: ({:.0},{:.0},{:.0}) - ({:.0},{:.0},{:.0})",
                refno.0,
                aabb.mins.x,
                aabb.mins.y,
                aabb.mins.z,
                aabb.maxs.x,
                aabb.maxs.y,
                aabb.maxs.z
            );
        }
    }

    // 测试2: 参考号周围查询
    println!("\n🎯 测试参考号周围查询...");
    let target_refno = RefU64(111200002);
    let distance = 100.0;

    if let Ok(Some(target_aabb)) = spatial_index.get_aabb(target_refno) {
        println!("  目标: RefNo {} (VALVE_001)", target_refno.0);
        println!("  查询距离: {} mm", distance);

        // 扩展边界框
        let expanded_box = Aabb::new(
            Point3::new(
                target_aabb.mins.x - distance,
                target_aabb.mins.y - distance,
                target_aabb.mins.z - distance,
            ),
            Point3::new(
                target_aabb.maxs.x + distance,
                target_aabb.maxs.y + distance,
                target_aabb.maxs.z + distance,
            ),
        );

        let nearby = spatial_index.query_intersect(&expanded_box)?;
        println!("  找到 {} 个附近对象:", nearby.len());
        for refno in &nearby {
            if *refno != target_refno {
                let name = match refno.0 {
                    111200001 => "PIPE_001",
                    111200003 => "PUMP_001",
                    111200004 => "TANK_001",
                    111200005 => "EQUIP_001",
                    _ => "Unknown",
                };
                println!("    - RefNo {}: {}", refno.0, name);
            }
        }
    }

    // 测试3: 大范围查询性能
    println!("\n⚡ 测试大范围查询性能...");
    let large_box = create_aabb(-10000.0, -10000.0, -10000.0, 10000.0, 10000.0, 10000.0);

    let start = std::time::Instant::now();
    let all_results = spatial_index.query_intersect(&large_box)?;
    let elapsed = start.elapsed();

    println!("  查询范围: 20m x 20m x 20m");
    println!("  返回结果: {} 个", all_results.len());
    println!("  查询耗时: {:?}", elapsed);

    // 获取统计信息
    println!("\n📊 索引统计信息:");
    let stats = spatial_index.get_stats()?;
    println!("  索引类型: {}", stats.index_type);
    println!("  总元素数: {}", stats.total_elements);

    println!("\n✅ 所有测试完成!");

    Ok(())
}

fn create_aabb(min_x: f32, min_y: f32, min_z: f32, max_x: f32, max_y: f32, max_z: f32) -> Aabb {
    Aabb::new(
        Point3::new(min_x, min_y, min_z),
        Point3::new(max_x, max_y, max_z),
    )
}
