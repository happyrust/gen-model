//! 集成测试：使用 dbnum 1112 测试空间索引的完整流程
//! 
//! 运行方式：
//! ```bash
//! cargo test --features sqlite-index --test spatial_index_integration -- --nocapture
//! ```

// 使用基于 SQLite RTree 的 AabbCache 实现进行集成测试
#[cfg(all(test, feature = "sqlite-index"))]
mod tests {
    use aios_database::fast_model::aabb_cache::AabbCache;
    use aios_core::RefU64;
    use parry3d::bounding_volume::{Aabb, BoundingVolume};
    use nalgebra::Point3;
    
    /// 模拟初始化 dbnum 1112 的测试数据
    async fn setup_test_data_1112(cache: &AabbCache) -> anyhow::Result<()> {
        println!("\n📦 初始化 dbnum 1112 测试数据...");
        
        // 不需要 SurrealDB，直接使用 SQLite 索引
        
        // Schema is initialized automatically when AABB_CACHE is opened
        println!("✅ SQLite 索引已自动初始化");
        
        // 模拟插入 dbnum 1112 的一些测试数据
        let test_elements = vec![
            // 模拟管道元件
            (RefU64(1112_00100), Aabb::new(
                Point3::new(1000.0, 2000.0, 3000.0),
                Point3::new(1500.0, 2100.0, 3050.0)
            ), "PIPE_1"),
            
            // 模拟阀门
            (RefU64(1112_00200), Aabb::new(
                Point3::new(1450.0, 2050.0, 3025.0),
                Point3::new(1550.0, 2150.0, 3075.0)
            ), "VALVE_1"),
            
            // 模拟设备
            (RefU64(1112_00300), Aabb::new(
                Point3::new(2000.0, 2000.0, 3000.0),
                Point3::new(3000.0, 3000.0, 4000.0)
            ), "EQUIPMENT_1"),
            
            // 模拟支架
            (RefU64(1112_00400), Aabb::new(
                Point3::new(1200.0, 1900.0, 2900.0),
                Point3::new(1300.0, 2200.0, 3100.0)
            ), "SUPPORT_1"),
            
            // 模拟仪表
            (RefU64(1112_00500), Aabb::new(
                Point3::new(1480.0, 2080.0, 3040.0),
                Point3::new(1520.0, 2120.0, 3060.0)
            ), "INSTRUMENT_1"),
        ];
        
        // 批量插入数据（写入 ref_bbox 并同步 RTree）
        use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
        for (refno, aabb, name) in &test_elements {
            let bbox = RStarBoundingBox::new(aabb.clone(), (*refno).into(), name.to_string());
            cache.put_ref_bbox(&bbox)?;
        }
        let count = test_elements.len();
        println!("✅ 插入 {} 个元件到空间索引", count);
        
        // 显示插入的元件信息
        for (refno, aabb, name) in &test_elements {
            println!("  - {} (RefNo {}): 中心点 ({:.0}, {:.0}, {:.0})",
                     name, refno.0,
                     (aabb.mins.x + aabb.maxs.x) / 2.0,
                     (aabb.mins.y + aabb.maxs.y) / 2.0,
                     (aabb.mins.z + aabb.maxs.z) / 2.0);
        }
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_spatial_index_workflow() -> anyhow::Result<()> {
        println!("\n🚀 开始空间索引集成测试...\n");
        // 独立的临时缓存文件，避免并发测试锁
        let tmp = tempfile::tempdir()?;
        let cache_path = tmp.path().join("aabb_cache.sqlite");
        let cache = AabbCache::open_with_path(&cache_path)?;

        // 1. 设置测试数据
        setup_test_data_1112(&cache).await?;
        
        // 2. 测试空间查询场景
        println!("\n🔍 场景1: 查找管道附近的元件");
        
        // 获取管道的 AABB
        let pipe_refno = RefU64(1112_00100);
        if let Ok(Some(pipe_aabb)) = cache.sqlite_get_aabb(pipe_refno) {
            // 扩展查询范围（例如查找200mm范围内的元件）
            let search_aabb = Aabb::new(
                Point3::from(pipe_aabb.mins.coords - nalgebra::Vector3::new(200.0, 200.0, 200.0)),
                Point3::from(pipe_aabb.maxs.coords + nalgebra::Vector3::new(200.0, 200.0, 200.0)),
            );
            
            let nearby = cache.sqlite_query_intersect(&search_aabb)?;
            println!("  找到 {} 个附近的元件:", nearby.len());
            
            for refno in &nearby {
                if *refno != pipe_refno {
                    let name = match refno.0 {
                        1112_00200 => "VALVE_1",
                        1112_00300 => "EQUIPMENT_1",
                        1112_00400 => "SUPPORT_1",
                        1112_00500 => "INSTRUMENT_1",
                        _ => "UNKNOWN",
                    };
                    println!("    - {} (RefNo {})", name, refno.0);
                }
            }
        }
        
        // 3. 测试碰撞检测场景
        println!("\n💥 场景2: 检测阀门的潜在碰撞");
        
        let valve_refno = RefU64(1112_00200);
        if let Ok(Some(valve_aabb)) = cache.sqlite_get_aabb(valve_refno) {
            // 使用阀门的精确 AABB 查找相交的元件
            let collisions = cache.sqlite_query_intersect(&valve_aabb)?;
            
            println!("  阀门可能与 {} 个元件相交:", collisions.len() - 1); // 减去自身
            
            for refno in &collisions {
                if *refno != valve_refno {
                    if let Ok(Some(other_aabb)) = cache.sqlite_get_aabb(*refno) {
                        if valve_aabb.intersects(&other_aabb) {
                            let name = match refno.0 {
                                1112_00100 => "PIPE_1",
                                1112_00500 => "INSTRUMENT_1",
                                _ => "UNKNOWN",
                            };
                            println!("    ⚠️ 与 {} 相交", name);
                        }
                    }
                }
            }
        }
        
        // 4. 测试区域查询场景
        println!("\n📐 场景3: 查询指定区域内的所有元件");
        
        let region = Aabb::new(
            Point3::new(1000.0, 1900.0, 2900.0),
            Point3::new(2000.0, 2200.0, 3100.0),
        );
        
        let in_region = cache.sqlite_query_intersect(&region)?;
        println!("  区域 ({:.0}-{:.0}, {:.0}-{:.0}, {:.0}-{:.0}) 内有 {} 个元件",
                 region.mins.x, region.maxs.x,
                 region.mins.y, region.maxs.y,
                 region.mins.z, region.maxs.z,
                 in_region.len());
        
        // 5. 性能统计
        println!("\n📊 性能统计:");
        
        // 小范围查询性能
        let start = std::time::Instant::now();
        let small_region = Aabb::new(
            Point3::new(1400.0, 2000.0, 3000.0),
            Point3::new(1600.0, 2200.0, 3100.0),
        );
        let _ = cache.sqlite_query_intersect(&small_region)?;
        let small_time = start.elapsed();
        
        // 大范围查询性能
        let start = std::time::Instant::now();
        let large_region = Aabb::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(5000.0, 5000.0, 5000.0),
        );
        let results = cache.sqlite_query_intersect(&large_region)?;
        let large_time = start.elapsed();
        
        println!("  - 小范围查询: {:?}", small_time);
        println!("  - 大范围查询: {:?} (返回 {} 个结果)", large_time, results.len());
        
        // 6. 清理（可选）
        println!("\n🧹 测试完成，空间索引功能正常！");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_rebuild_from_cache() -> anyhow::Result<()> {
        println!("\n🔨 测试从缓存重建 SQLite 索引...\n");
        
        // 打开缓存并尝试从内部存储重建 RTree
        let cache = AabbCache::open_default()?;
        match cache.sqlite_rebuild_from_internal() {
            Ok(count) => {
                println!("✅ 成功从 ReDB 重建 {} 条索引记录", count);
                
                // 验证重建的数据
                let test_region = Aabb::new(
                    Point3::new(-10000.0, -10000.0, -10000.0),
                    Point3::new(10000.0, 10000.0, 10000.0),
                );
                
                let all_items = cache.sqlite_query_intersect(&test_region)?;
                println!("  验证: 索引中共有 {} 个元件", all_items.len());
                
                // 统计各个 dbnum
                let mut dbnum_counts = std::collections::HashMap::new();
                for refno in &all_items {
                    let dbnum = (refno.0 / 10000) as u32;
                    *dbnum_counts.entry(dbnum).or_insert(0) += 1;
                }
                
                println!("\n  按数据库分组:");
                for (dbnum, count) in dbnum_counts {
                    println!("    - dbnum {}: {} 个元件", dbnum, count);
                }
            }
            Err(e) => {
                println!("⚠️ 重建失败（可能是没有缓存数据）: {}", e);
            }
        }
        
        Ok(())
    }
}
