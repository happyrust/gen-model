#[cfg(all(test, feature = "sqlite-index"))]
mod tests {
    use crate::spatial_index::SqliteSpatialIndex;
    use aios_core::RefU64;
    use nalgebra::{Point3, Vector3};
    use parry3d::bounding_volume::{Aabb, BoundingVolume};

    #[test]
    fn test_sqlite_spatial_basic() {
        println!("\n🧪 测试 SQLite 空间索引基本功能...\n");

        // 1. 检查 SQLite 是否启用
        if !SqliteSpatialIndex::is_enabled() {
            println!("⚠️ SQLite 索引未启用，跳过测试");
            return;
        }

        println!("✅ SQLite 索引已启用");

        // 创建空间索引（使用临时文件，避免并发干扰）
        let tmp = tempfile::tempdir().expect("create temp dir");
        let db_path = tmp.path().join("aabb_cache.sqlite");
        let spatial_index =
            SqliteSpatialIndex::new(&db_path).expect("Failed to create spatial index");
        println!("✅ SQLite 表结构已自动初始化");

        // 2. 创建测试数据
        let test_aabbs = vec![
            (
                RefU64(1112_00001),
                Aabb::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 10.0, 10.0)),
            ),
            (
                RefU64(1112_00002),
                Aabb::new(Point3::new(5.0, 5.0, 5.0), Point3::new(15.0, 15.0, 15.0)),
            ),
            (
                RefU64(1112_00003),
                Aabb::new(Point3::new(20.0, 20.0, 20.0), Point3::new(30.0, 30.0, 30.0)),
            ),
            (
                RefU64(1112_00004),
                Aabb::new(Point3::new(-10.0, -10.0, -10.0), Point3::new(0.0, 0.0, 0.0)),
            ),
            (
                RefU64(1112_00005),
                Aabb::new(
                    Point3::new(100.0, 100.0, 100.0),
                    Point3::new(110.0, 110.0, 110.0),
                ),
            ),
        ];

        println!("📦 准备插入 {} 个测试 AABB", test_aabbs.len());

        // 3. 插入测试数据到 SQLite
        for (refno, aabb) in &test_aabbs {
            if let Err(e) =
                spatial_index.insert_aabb(*refno, aabb, Some(&format!("test_{}", refno.0)))
            {
                panic!("❌ 插入数据失败: {}", e);
            }
        }
        let count = test_aabbs.len();
        println!("✅ 成功插入 {} 条记录", count);

        // 4. 测试点查询
        println!("\n🔍 测试点查询...");
        for (refno, expected_aabb) in &test_aabbs {
            match spatial_index.get_aabb(*refno) {
                Ok(Some(aabb)) => {
                    assert_eq!(
                        aabb.mins, expected_aabb.mins,
                        "RefNo {} mins 不匹配",
                        refno.0
                    );
                    assert_eq!(
                        aabb.maxs, expected_aabb.maxs,
                        "RefNo {} maxs 不匹配",
                        refno.0
                    );
                    println!("  ✅ RefNo {} 查询成功", refno.0);
                }
                Ok(None) => {
                    panic!("❌ RefNo {} 未找到", refno.0);
                }
                Err(e) => {
                    panic!("❌ 查询 RefNo {} 失败: {}", refno.0, e);
                }
            }
        }

        // 5. 测试空间相交查询
        println!("\n📐 测试空间相交查询...");

        // 查询与第一个 AABB 相交的对象
        let query_bbox = Aabb::new(Point3::new(-5.0, -5.0, -5.0), Point3::new(12.0, 12.0, 12.0));

        println!(
            "  查询框: min({:.1}, {:.1}, {:.1}) max({:.1}, {:.1}, {:.1})",
            query_bbox.mins.x,
            query_bbox.mins.y,
            query_bbox.mins.z,
            query_bbox.maxs.x,
            query_bbox.maxs.y,
            query_bbox.maxs.z
        );

        match spatial_index.query_intersect(&query_bbox) {
            Ok(results) => {
                println!("  找到 {} 个相交的对象:", results.len());
                for id in &results {
                    println!("    - RefNo {}", id.0);
                }

                // 验证结果
                assert!(results.contains(&RefU64(1112_00001)), "应该包含 1112_00001");
                assert!(results.contains(&RefU64(1112_00002)), "应该包含 1112_00002");
                assert!(results.contains(&RefU64(1112_00004)), "应该包含 1112_00004");
                assert!(
                    !results.contains(&RefU64(1112_00003)),
                    "不应该包含 1112_00003"
                );
                assert!(
                    !results.contains(&RefU64(1112_00005)),
                    "不应该包含 1112_00005"
                );
            }
            Err(e) => {
                panic!("❌ 空间查询失败: {}", e);
            }
        }

        // 6. 测试大范围查询
        println!("\n🌍 测试大范围查询...");
        let large_bbox = Aabb::new(
            Point3::new(-1000.0, -1000.0, -1000.0),
            Point3::new(1000.0, 1000.0, 1000.0),
        );

        match spatial_index.query_intersect(&large_bbox) {
            Ok(results) => {
                println!("  大范围查询返回 {} 个对象", results.len());
                assert_eq!(results.len(), 5, "大范围查询应该返回所有 5 个对象");
            }
            Err(e) => {
                panic!("❌ 大范围查询失败: {}", e);
            }
        }

        // 7. 测试精确边界查询
        println!("\n🎯 测试精确边界查询...");
        let exact_bbox = Aabb::new(Point3::new(5.0, 5.0, 5.0), Point3::new(15.0, 15.0, 15.0));

        match spatial_index.query_intersect(&exact_bbox) {
            Ok(results) => {
                println!("  精确查询返回 {} 个对象", results.len());
                assert!(
                    results.contains(&RefU64(1112_00001)),
                    "应该包含 1112_00001（相交）"
                );
                assert!(
                    results.contains(&RefU64(1112_00002)),
                    "应该包含 1112_00002（完全匹配）"
                );
            }
            Err(e) => {
                panic!("❌ 精确查询失败: {}", e);
            }
        }

        println!("\n✅ 所有 SQLite 空间索引测试通过！");
    }

    #[test]
    fn test_sqlite_spatial_performance() {
        println!("\n⚡ 测试 SQLite 空间索引性能...\n");

        if !SqliteSpatialIndex::is_enabled() {
            println!("⚠️ SQLite 索引未启用，跳过性能测试");
            return;
        }

        // 创建空间索引（使用临时文件，避免并发干扰）
        let tmp = tempfile::tempdir().expect("create temp dir");
        let db_path = tmp.path().join("aabb_cache.sqlite");
        let spatial_index = match SqliteSpatialIndex::new(&db_path) {
            Ok(idx) => idx,
            Err(e) => panic!("❌ 初始化 SQLite 空间索引失败: {}", e),
        };

        // 1. 批量插入性能测试
        println!("📦 批量插入性能测试...");
        let count = 1000;
        let mut test_data = Vec::new();

        for i in 0..count {
            let x = (i as f32) * 10.0;
            let refno = RefU64(1112_10000 + i as u64);
            let aabb = Aabb::new(Point3::new(x, 0.0, 0.0), Point3::new(x + 5.0, 5.0, 5.0));
            test_data.push((refno, aabb, None));
        }

        let start = std::time::Instant::now();
        let _ = spatial_index.insert_many(test_data.clone().into_iter());
        let insert_time = start.elapsed();

        println!("  - 插入 {} 条记录耗时: {:?}", count, insert_time);
        println!("  - 平均每条: {:?}", insert_time / count as u32);
        println!(
            "  - 插入速度: {:.0} 条/秒",
            count as f64 * 1000.0 / insert_time.as_millis() as f64
        );

        // 2. 查询性能测试
        println!("\n🔍 查询性能测试...");

        // 小范围查询
        let small_query = Aabb::new(
            Point3::new(500.0, -10.0, -10.0),
            Point3::new(600.0, 10.0, 10.0),
        );

        let start = std::time::Instant::now();
        let results = spatial_index.query_intersect(&small_query).unwrap();
        let query_time = start.elapsed();

        println!("  小范围查询:");
        println!("    - 返回 {} 个结果", results.len());
        println!("    - 耗时: {:?}", query_time);

        // 大范围查询
        let large_query = Aabb::new(
            Point3::new(0.0, -100.0, -100.0),
            Point3::new(5000.0, 100.0, 100.0),
        );

        let start = std::time::Instant::now();
        let results = spatial_index.query_intersect(&large_query).unwrap();
        let query_time = start.elapsed();

        println!("  大范围查询:");
        println!("    - 返回 {} 个结果", results.len());
        println!("    - 耗时: {:?}", query_time);
        println!(
            "    - 处理速度: {:.0} 结果/秒",
            results.len() as f64 * 1000.0 / query_time.as_millis() as f64
        );

        // 3. 批量查询性能
        println!("\n⏱️ 批量查询性能测试...");
        let query_count = 100;
        let mut total_time = std::time::Duration::ZERO;
        let mut total_results = 0;

        for i in 0..query_count {
            let x = (i as f32) * 50.0;
            let query = Aabb::new(Point3::new(x, -5.0, -5.0), Point3::new(x + 100.0, 5.0, 5.0));

            let start = std::time::Instant::now();
            let results = spatial_index.query_intersect(&query).unwrap();
            total_time += start.elapsed();
            total_results += results.len();
        }

        println!("  - 执行 {} 次查询", query_count);
        println!("  - 总耗时: {:?}", total_time);
        println!("  - 平均每次: {:?}", total_time / query_count);
        println!(
            "  - 平均返回: {} 个结果",
            total_results / query_count as usize
        );
        println!(
            "  - 查询速度: {:.0} 查询/秒",
            query_count as f64 * 1000.0 / total_time.as_millis() as f64
        );

        println!("\n✅ 性能测试完成！");
    }

    #[test]
    fn test_sqlite_spatial_point_query() {
        println!("\n🎯 测试 SQLite 空间索引点查询功能...\n");

        if !SqliteSpatialIndex::is_enabled() {
            println!("⚠️ SQLite 索引未启用，跳过点查询测试");
            return;
        }

        // 创建空间索引（使用临时文件，避免并发干扰）
        let tmp = tempfile::tempdir().expect("create temp dir");
        let db_path = tmp.path().join("aabb_cache.sqlite");
        let spatial_index = match SqliteSpatialIndex::new(&db_path) {
            Ok(idx) => idx,
            Err(e) => panic!("❌ 初始化 SQLite 空间索引失败: {}", e),
        };

        // 1. 插入测试数据
        println!("📦 插入测试数据...");
        let test_data = vec![
            (
                RefU64(2001),
                Aabb::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 10.0, 10.0)),
                "EQUI",
            ),
            (
                RefU64(2002),
                Aabb::new(Point3::new(5.0, 5.0, 5.0), Point3::new(15.0, 15.0, 15.0)),
                "PIPE",
            ),
            (
                RefU64(2003),
                Aabb::new(Point3::new(20.0, 20.0, 20.0), Point3::new(30.0, 30.0, 30.0)),
                "EQUI",
            ),
            (
                RefU64(2004),
                Aabb::new(Point3::new(-10.0, -10.0, -10.0), Point3::new(0.0, 0.0, 0.0)),
                "PIPE",
            ),
            (
                RefU64(2005),
                Aabb::new(
                    Point3::new(100.0, 100.0, 100.0),
                    Point3::new(110.0, 110.0, 110.0),
                ),
                "EQUI",
            ),
        ];

        for (refno, aabb, noun) in &test_data {
            spatial_index
                .insert_aabb(*refno, aabb, Some(noun))
                .expect("插入失败");
        }
        println!("✅ 成功插入 {} 条记录", test_data.len());

        // 2. 测试点查询 - 查询最近的 k 个点
        println!("\n🔍 测试点查询 (query_nearest_to_point)...");
        use crate::spatial_index::{QueryOptions, SpatialQueryBackend};

        // 测试点：在第一个和第二个 AABB 的交集附近
        let query_point = Point3::new(7.5, 7.5, 7.5);

        println!("  查询点: ({:.1}, {:.1}, {:.1})", query_point.x, query_point.y, query_point.z);

        // 查询最近的 3 个点
        let results = spatial_index
            .query_nearest_to_point(query_point, 3, Some(50.0), &QueryOptions::default())
            .expect("点查询失败");

        println!("  找到 {} 个最近的对象:", results.len());
        for (i, hit) in results.iter().enumerate() {
            println!(
                "    {}. RefNo: {}, 距离: {:.2}",
                i + 1,
                hit.refno.0,
                hit.distance.unwrap_or(0.0)
            );
        }

        // 验证结果
        assert!(!results.is_empty(), "应该至少找到一个结果");
        assert!(results.len() <= 3, "结果数量不应超过 3 个");
        
        // 第一个结果应该是最近的（2001 或 2002）
        let first_refno = results[0].refno;
        assert!(
            first_refno == RefU64(2001) || first_refno == RefU64(2002),
            "最近的应该是 2001 或 2002"
        );

        // 3. 测试球形范围查询
        println!("\n🌐 测试球形范围查询 (query_within_radius)...");
        let center = Point3::new(7.5, 7.5, 7.5);
        let radius = 20.0;

        let radius_results = spatial_index
            .query_within_radius(center, radius, &QueryOptions::default())
            .expect("范围查询失败");

        println!("  中心点: ({:.1}, {:.1}, {:.1}), 半径: {:.1}", center.x, center.y, center.z, radius);
        println!("  找到 {} 个在范围内的对象:", radius_results.len());
        for hit in &radius_results {
            println!(
                "    - RefNo: {}, 距离: {:.2}",
                hit.refno.0,
                hit.distance.unwrap_or(0.0)
            );
        }

        // 验证结果：应该包含 2001, 2002, 2004（2003 和 2005 太远）
        let refnos: Vec<u64> = radius_results.iter().map(|h| h.refno.0).collect();
        assert!(refnos.contains(&2001), "应该包含 2001");
        assert!(refnos.contains(&2002), "应该包含 2002");
        assert!(refnos.contains(&2004), "应该包含 2004");
        assert!(!refnos.contains(&2003), "不应该包含 2003（太远）");
        assert!(!refnos.contains(&2005), "不应该包含 2005（太远）");

        // 4. 测试类型过滤
        println!("\n🏷️ 测试类型过滤...");
        let mut type_opts = QueryOptions::default();
        type_opts.types = vec!["EQUI".to_string()];

        let type_results = spatial_index
            .query_within_radius(center, radius, &type_opts)
            .expect("类型过滤查询失败");

        println!("  过滤类型: EQUI");
        println!("  找到 {} 个 EQUI 类型的对象:", type_results.len());
        for hit in &type_results {
            println!("    - RefNo: {}", hit.refno.0);
        }

        // 验证：只应该包含 EQUI 类型（2001, 2005，但 2005 太远，所以只有 2001）
        assert_eq!(type_results.len(), 1, "应该只有 1 个 EQUI 类型在范围内");
        assert_eq!(type_results[0].refno, RefU64(2001), "应该是 2001");

        println!("\n✅ 所有点查询测试通过！");
    }
}
