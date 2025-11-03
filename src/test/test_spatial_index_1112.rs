// DEPRECATED: These tests are for the old AABB_CACHE implementation
// TODO: Rewrite tests for the new SqliteSpatialIndex
#[cfg(test)]
#[ignore]
mod tests {
    use aios_core::{
        init_surreal,
        get_db_option,
        options::DbOption,
        RefU64,
    };
    use crate::fast_model::{
        aabb_cache::{AABB_CACHE, AabbCache},
        room_model::build_room_relations,
    };
    use parry3d::bounding_volume::{Aabb, BoundingVolume};
    use nalgebra::{Point3, Vector3};
    
    /// 初始化测试环境，使用 dbnum 1112 的数据
    async fn init_test_env_1112() -> anyhow::Result<DbOption> {
        // 使用默认配置并修改必要字段
        let mut db_option = aios_core::get_db_option();
        
        // 设置项目路径和基本信息
        db_option.project_path = "/Volumes/DPC/work/e3d_models".to_string();
        db_option.project_name = "AvevaMarineSample".to_string();
        db_option.project_code = "1516".to_string();
        db_option.surreal_ns = "1516".to_string();
        db_option.module = "DESI".to_string();
        db_option.mdb_name = "ALL".to_string();
        
        // 指定只处理 dbnum 1112
        db_option.manual_db_nums = Some(vec![1112]);
        db_option.included_db_files = Some(vec!["ams1112_0001".to_string()]);
        
        // 启用空间索引功能
        db_option.gen_spatial_tree = true;
        
        // 启用模型生成
        db_option.gen_model = true;
        db_option.gen_mesh = true;
        db_option.save_db = true;
        
        // 其他配置
        db_option.gen_model_batch_size = Some(16);
        db_option.mesh_tol_ratio = Some(3.0);
        db_option.apply_boolean_operation = Some(true);
        
        // 初始化 SurrealDB
        init_surreal(&db_option).await?;
        
        // 确保 SQLite 索引被启用
        #[cfg(feature = "sqlite-index")]
        {
            let sqlite_path = "test_spatial_index_1112.sqlite";
            // 如果测试数据库已存在，先删除
            if std::path::Path::new(sqlite_path).exists() {
                std::fs::remove_file(sqlite_path)?;
            }
            
            // 初始化 SQLite 索引
            let sqlite_index = crate::sqlite_index::SqliteAabbIndex::open(sqlite_path)?;
            sqlite_index.init_schema()?;
            println!("✅ SQLite 空间索引初始化完成: {}", sqlite_path);
        }
        
        Ok(db_option)
    }
    
    #[tokio::test]
    async fn test_spatial_tree_generation_1112() -> anyhow::Result<()> {
        println!("\n🚀 开始测试 dbnum 1112 的空间树生成...\n");
        
        // 1. 初始化测试环境
        let db_option = init_test_env_1112().await?;
        println!("✅ 测试环境初始化完成");
        
        // 2. 生成模型数据
        println!("\n📦 开始生成模型数据...");
        let start = std::time::Instant::now();
        
        let result = crate::fast_model::gen_model::gen_model(&db_option).await?;
        assert!(result, "模型生成应该成功");
        
        println!("✅ 模型生成完成，耗时: {:?}", start.elapsed());
        
        // 3. 重建 SQLite 空间索引
        #[cfg(feature = "sqlite-index")]
        {
            println!("\n🔨 重建 SQLite 空间索引...");
            let rebuild_start = std::time::Instant::now();
            
            let row_count = AABB_CACHE.sqlite_rebuild_from_redb()?;
            println!("✅ SQLite 索引重建完成，共 {} 条记录，耗时: {:?}", 
                     row_count, rebuild_start.elapsed());
            
            assert!(row_count > 0, "应该有空间索引数据");
        }
        
        // 4. 生成房间关系（空间树）
        println!("\n🏗️ 构建房间空间关系...");
        let room_start = std::time::Instant::now();
        
        build_room_relations(&db_option).await?;
        
        println!("✅ 房间关系构建完成，耗时: {:?}", room_start.elapsed());
        
        // 5. 验证数据
        println!("\n📊 验证生成的数据...");
        
        // 检查 dbnum 1112 的数据
        let db1112_refnos = AABB_CACHE.get_refnos_by_dbnum(1112)?;
        println!("  - dbnum 1112 包含 {} 个参考号", db1112_refnos.len());
        assert!(!db1112_refnos.is_empty(), "dbnum 1112 应该有数据");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_spatial_query_1112() -> anyhow::Result<()> {
        println!("\n🔍 开始测试 dbnum 1112 的空间查询...\n");
        
        // 1. 初始化测试环境
        let _db_option = init_test_env_1112().await?;
        
        // 2. 确保有测试数据
        #[cfg(feature = "sqlite-index")]
        if !AabbCache::sqlite_is_enabled() {
            println!("⚠️ SQLite 索引未启用，跳过空间查询测试");
            return Ok(());
        }
        
        // 3. 获取一些测试用的参考号
        let test_refnos = AABB_CACHE.get_refnos_by_dbnum(1112)?;
        if test_refnos.is_empty() {
            println!("⚠️ dbnum 1112 没有数据，需要先运行生成测试");
            return Ok(());
        }
        
        println!("📦 使用 {} 个参考号进行测试", test_refnos.len().min(5));
        
        // 4. 测试空间查询
        for (idx, refno) in test_refnos.iter().take(5).enumerate() {
            println!("\n测试 #{}: RefNo {}", idx + 1, refno.0);
            
            // 获取该参考号的 AABB
            #[cfg(feature = "sqlite-index")]
            if let Ok(Some(bbox)) = AABB_CACHE.sqlite_get_aabb(*refno) {
                println!("  - AABB: min({:.2}, {:.2}, {:.2}) max({:.2}, {:.2}, {:.2})",
                         bbox.mins.x, bbox.mins.y, bbox.mins.z,
                         bbox.maxs.x, bbox.maxs.y, bbox.maxs.z);
                
                // 创建一个扩展的查询框
                let query_bbox = Aabb::new(
                    Point3::from(bbox.mins.coords - Vector3::new(100.0, 100.0, 100.0)),
                    Point3::from(bbox.maxs.coords + Vector3::new(100.0, 100.0, 100.0)),
                );
                
                // 执行空间查询
                let intersecting = AABB_CACHE.sqlite_query_intersect(&query_bbox)?;
                println!("  - 查询到 {} 个相交的对象", intersecting.len());
                
                // 验证查询结果包含自身
                assert!(intersecting.contains(refno), 
                        "查询结果应该包含自身 RefNo {}", refno.0);
                
                // 显示前几个相交的对象
                for (i, id) in intersecting.iter().take(3).enumerate() {
                    if id != refno {
                        println!("    {}. RefNo {}", i + 1, id.0);
                    }
                }
            }
        }
        
        // 5. 测试范围查询
        println!("\n📐 测试大范围空间查询...");
        let large_query_bbox = Aabb::new(
            Point3::new(-10000.0, -10000.0, -10000.0),
            Point3::new(10000.0, 10000.0, 10000.0),
        );
        
        #[cfg(feature = "sqlite-index")]
        {
            let all_intersecting = AABB_CACHE.sqlite_query_intersect(&large_query_bbox)?;
            println!("  - 大范围查询返回 {} 个对象", all_intersecting.len());
            
            // 验证返回的对象都属于 dbnum 1112
            let mut count_1112 = 0;
            for id in &all_intersecting {
                let dbnum = (id.0 / 10000) as u32;
                if dbnum == 1112 {
                    count_1112 += 1;
                }
            }
            println!("  - 其中 {} 个属于 dbnum 1112", count_1112);
        }
        
        // 6. 测试精确查询
        println!("\n🎯 测试精确空间查询...");
        if let Some(test_refno) = test_refnos.first() {
            #[cfg(feature = "sqlite-index")]
            if let Ok(Some(bbox)) = AABB_CACHE.sqlite_get_aabb(*test_refno) {
                // 使用完全相同的 AABB 进行查询
                let exact_intersecting = AABB_CACHE.sqlite_query_intersect(&bbox)?;
                println!("  - 精确查询 RefNo {} 返回 {} 个对象", 
                         test_refno.0, exact_intersecting.len());
                
                assert!(exact_intersecting.contains(test_refno),
                        "精确查询应该包含自身");
                
                // 显示所有精确相交的对象
                for id in &exact_intersecting {
                    if id != test_refno {
                        if let Ok(Some(other_bbox)) = AABB_CACHE.sqlite_get_aabb(*id) {
                            // 检查是否真的相交
                            let intersects = bbox.intersects(&other_bbox);
                            println!("    - RefNo {} {}相交", 
                                     id.0, 
                                     if intersects { "确实" } else { "不" });
                        }
                    }
                }
            }
        }
        
        println!("\n✅ 空间查询测试完成！");
        Ok(())
    }
    
    #[tokio::test]
    async fn test_spatial_performance_1112() -> anyhow::Result<()> {
        println!("\n⚡ 开始测试 dbnum 1112 的空间查询性能...\n");
        
        // 1. 初始化测试环境
        let _db_option = init_test_env_1112().await?;
        
        #[cfg(feature = "sqlite-index")]
        if !AabbCache::sqlite_is_enabled() {
            println!("⚠️ SQLite 索引未启用，跳过性能测试");
            return Ok(());
        }
        
        // 2. 获取测试数据
        let test_refnos = AABB_CACHE.get_refnos_by_dbnum(1112)?;
        if test_refnos.len() < 10 {
            println!("⚠️ 测试数据不足，需要至少 10 个参考号");
            return Ok(());
        }
        
        println!("📊 使用 {} 个参考号进行性能测试", test_refnos.len());
        
        // 3. 批量查询性能测试
        println!("\n⏱️ 批量查询性能测试...");
        let mut total_time = std::time::Duration::ZERO;
        let mut total_results = 0;
        let test_count = test_refnos.len().min(100);
        
        #[cfg(feature = "sqlite-index")]
        for refno in test_refnos.iter().take(test_count) {
            if let Ok(Some(bbox)) = AABB_CACHE.sqlite_get_aabb(*refno) {
                // 扩展查询框
                let query_bbox = Aabb::new(
                    Point3::from(bbox.mins.coords - Vector3::new(50.0, 50.0, 50.0)),
                    Point3::from(bbox.maxs.coords + Vector3::new(50.0, 50.0, 50.0)),
                );
                
                let start = std::time::Instant::now();
                let results = AABB_CACHE.sqlite_query_intersect(&query_bbox)?;
                let elapsed = start.elapsed();
                
                total_time += elapsed;
                total_results += results.len();
            }
        }
        
        let avg_time = total_time / test_count as u32;
        let avg_results = total_results / test_count;
        
        println!("  - 执行了 {} 次查询", test_count);
        println!("  - 总耗时: {:?}", total_time);
        println!("  - 平均每次查询: {:?}", avg_time);
        println!("  - 平均每次返回: {} 个结果", avg_results);
        println!("  - 查询速度: {:.0} 查询/秒", 
                 1000.0 / avg_time.as_millis() as f64);
        
        // 4. 大范围查询性能测试
        println!("\n🌍 大范围查询性能测试...");
        let large_bbox = Aabb::new(
            Point3::new(-5000.0, -5000.0, -5000.0),
            Point3::new(5000.0, 5000.0, 5000.0),
        );
        
        #[cfg(feature = "sqlite-index")]
        {
            let start = std::time::Instant::now();
            let results = AABB_CACHE.sqlite_query_intersect(&large_bbox)?;
            let elapsed = start.elapsed();
            
            println!("  - 查询范围: 10000x10000x10000");
            println!("  - 返回结果: {} 个", results.len());
            println!("  - 查询耗时: {:?}", elapsed);
            println!("  - 处理速度: {:.0} 结果/秒", 
                     results.len() as f64 * 1000.0 / elapsed.as_millis() as f64);
        }
        
        println!("\n✅ 性能测试完成！");
        Ok(())
    }
}