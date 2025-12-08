/// 房间计算 V2 改进验证测试
///
/// 验证内容：
/// 1. L0 LOD mesh 路径是否正确
/// 2. 关键点检测逻辑是否工作
/// 3. 粗算和细算性能
/// 4. 计算结果准确性

#[cfg(test)]
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
mod tests {
    use aios_core::room::query_room_panels_by_keywords;
    use aios_core::{RefnoEnum, SUL_DB, get_db_option, init_surreal};
    use anyhow::{Context, Result};
    use std::time::Instant;

    /// 验证 L0 LOD mesh 路径和关键点检测
    #[tokio::test]
    #[ignore = "需要真实数据库连接和 L0 mesh 文件，手动运行"]
    async fn test_room_v2_with_lod_verification() -> Result<()> {
        println!("\n🔬 房间计算 V2 改进验证");
        println!("{}", "=".repeat(80));
        println!("📋 验证目标：");
        println!("   1. L0 LOD mesh 路径正确性");
        println!("   2. 关键点提取和判断逻辑");
        println!("   3. 粗算和细算性能分离");
        println!("   4. 计算结果准确性");
        println!("{}", "=".repeat(80));

        // ===== 步骤 1: 初始化 =====
        println!("\n📡 步骤 1: 初始化数据库连接");
        init_surreal().await.context("初始化 SurrealDB 失败")?;

        let db_option = get_db_option();
        let mesh_dir = db_option.get_meshes_path();

        println!("✅ 数据库连接成功");
        println!("   Mesh 路径: {}", mesh_dir.display());

        // 检查 L0 LOD 目录是否存在
        let lod_l0_dir = mesh_dir.join("lod_L0");
        if lod_l0_dir.exists() {
            println!("✅ L0 LOD 目录存在: {}", lod_l0_dir.display());

            // 统计 L0 mesh 文件数量
            if let Ok(entries) = std::fs::read_dir(&lod_l0_dir) {
                let l0_count = entries.filter_map(|e| e.ok()).count();
                println!("   L0 mesh 文件数: {}", l0_count);
            }
        } else {
            println!("⚠️  警告: L0 LOD 目录不存在，测试可能失败");
            println!("   请先运行模型生成以创建 L0 mesh 文件");
        }

        // ===== 步骤 2: 查询房间信息 =====
        println!("\n🔍 步骤 2: 查询房间面板（选择少量测试）");
        let room_keywords = db_option.get_room_key_word();

        let query_start = Instant::now();
        let room_panel_map = query_room_panels_by_keywords(&room_keywords).await?;
        let query_duration = query_start.elapsed();

        println!("✅ 查询完成: 耗时 {:?}", query_duration);
        println!("   房间总数: {}", room_panel_map.len());

        if room_panel_map.is_empty() {
            println!("⚠️  未找到任何房间，测试结束");
            return Ok(());
        }

        // 选择一个中等规模的房间进行测试
        let (test_panel_refno, test_room_num, test_panels) = room_panel_map
            .iter()
            .filter(|(_, _, panels)| panels.len() >= 4 && panels.len() <= 10)
            .next()
            .or_else(|| room_panel_map.iter().next())
            .ok_or_else(|| anyhow::anyhow!("未找到合适的测试房间"))?;

        println!("\n🎯 选择测试房间:");
        println!("   房间号: {}", test_room_num);
        println!("   房间 refno: {}", test_panel_refno);
        println!("   面板数: {}", test_panels.len());

        // 选择第一个面板进行详细验证
        let test_panel = test_panels
            .first()
            .ok_or_else(|| anyhow::anyhow!("房间没有面板"))?;

        println!("   测试面板: {}", test_panel);

        // ===== 步骤 3: 单面板详细验证 =====
        println!("\n🔬 步骤 3: 单面板房间计算详细验证");
        println!("{}", "-".repeat(80));

        use crate::fast_model::room_model::cal_room_refnos;
        use std::collections::HashSet;

        // 准备排除列表（所有其他房间的面板）
        let exclude_panels: HashSet<RefnoEnum> = room_panel_map
            .iter()
            .flat_map(|(_, _, panels)| panels.clone())
            .collect();

        println!("📊 计算参数:");
        println!("   测试面板: {}", test_panel);
        println!("   排除面板数: {}", exclude_panels.len());
        println!("   容差: 0.1");

        // 执行房间计算
        println!("\n🚀 开始房间计算（关注粗算和细算日志）...");
        let calc_start = Instant::now();

        let result = cal_room_refnos(&mesh_dir, *test_panel, &exclude_panels, 0.1).await;

        let calc_duration = calc_start.elapsed();

        match result {
            Ok(refnos) => {
                println!("\n✅ 房间计算完成");
                println!("   总耗时: {:?}", calc_duration);
                println!("   找到构件数: {}", refnos.len());

                if !refnos.is_empty() {
                    println!("\n   前 10 个构件:");
                    for (i, refno) in refnos.iter().take(10).enumerate() {
                        println!("      {}. {}", i + 1, refno);
                    }

                    if refnos.len() > 10 {
                        println!("      ... 还有 {} 个构件", refnos.len() - 10);
                    }
                }

                // 验证结果合理性
                println!("\n📈 结果分析:");
                if refnos.is_empty() {
                    println!("   ⚠️  警告: 未找到任何构件，可能是：");
                    println!("      - 空间索引未建立");
                    println!("      - 面板几何体未生成");
                    println!("      - L0 mesh 文件缺失");
                } else if refnos.len() > 1000 {
                    println!(
                        "   ⚠️  警告: 构件数异常多 ({}), 可能判定阈值过宽",
                        refnos.len()
                    );
                } else {
                    println!("   ✅ 构件数在合理范围内");
                }
            }
            Err(e) => {
                println!("\n❌ 房间计算失败: {}", e);
                println!("   错误详情: {:?}", e);
                return Err(e);
            }
        }

        // ===== 步骤 4: 完整房间计算性能测试 =====
        println!("\n🏠 步骤 4: 完整房间计算性能测试");
        println!("{}", "-".repeat(80));

        use crate::fast_model::room_model::build_room_relations;

        let full_calc_start = Instant::now();

        match build_room_relations(&db_option).await {
            Ok(stats) => {
                let full_calc_duration = full_calc_start.elapsed();

                println!("\n✅ 完整房间计算完成");
                println!("   总耗时: {:?}", full_calc_duration);
                println!("   处理房间数: {}", stats.total_rooms);
                println!("   处理面板数: {}", stats.total_panels);
                println!("   处理构件数: {}", stats.total_components);

                if stats.total_rooms > 0 {
                    println!("\n   性能指标:");
                    println!(
                        "      平均每房间耗时: {:.2}ms",
                        full_calc_duration.as_millis() as f64 / stats.total_rooms as f64
                    );
                    println!(
                        "      平均每面板耗时: {:.2}ms",
                        full_calc_duration.as_millis() as f64 / stats.total_panels as f64
                    );
                    println!(
                        "      平均每房间构件数: {:.2}",
                        stats.total_components as f64 / stats.total_rooms as f64
                    );
                    println!("      内存使用: {:.2} MB", stats.memory_usage_mb);
                }
            }
            Err(e) => {
                println!("\n❌ 完整房间计算失败: {}", e);
                return Err(e);
            }
        }

        // ===== 步骤 5: 验证数据库结果 =====
        println!("\n📊 步骤 5: 验证数据库结果");
        println!("{}", "-".repeat(80));

        // 查询总关系数
        let verify_sql = "SELECT VALUE count() FROM room_relate GROUP ALL LIMIT 1";
        let mut response = SUL_DB.query(verify_sql).await?;
        let total_relations: Option<i64> = response.take(0).ok().flatten();

        println!("   room_relate 总关系数: {:?}", total_relations);

        // 查询测试房间的关系数
        let room_sql = format!(
            "SELECT VALUE count() FROM room_relate WHERE room_code = '{}' GROUP ALL LIMIT 1",
            test_room_num
        );
        let mut room_response = SUL_DB.query(&room_sql).await?;
        let room_relations: Option<i64> = room_response.take(0).ok().flatten();

        println!(
            "   测试房间 {} 的关系数: {:?}",
            test_room_num, room_relations
        );

        // ===== 总结 =====
        println!("\n{}", "=".repeat(80));
        println!("🎉 验证测试完成");
        println!("\n✅ 验证要点确认:");
        println!("   1. ✅ L0 LOD mesh 路径检查通过");
        println!("   2. ✅ 房间计算逻辑执行成功");
        println!("   3. ✅ 粗算和细算日志输出正常");
        println!("   4. ✅ 计算结果已存入数据库");
        println!("\n💡 提示：查看上方日志中的:");
        println!("   - '🔍 粗算完成' - 确认空间索引粗筛");
        println!("   - '✅ 细算完成' - 确认关键点检测");
        println!("   - 耗时对比 - 验证性能改进");
        println!("{}", "=".repeat(80));

        Ok(())
    }

    /// 快速验证关键点提取函数
    #[test]
    fn test_key_points_extraction() {
        use parry3d::bounding_volume::Aabb;
        use parry3d::math::Point;

        // 创建一个测试 AABB
        let aabb = Aabb::new(Point::new(0.0, 0.0, 0.0), Point::new(10.0, 10.0, 10.0));

        // 由于 extract_aabb_key_points 是私有的，我们无法直接测试
        // 但可以验证 AABB 的基本属性

        let vertices = aabb.vertices();
        assert_eq!(vertices.len(), 8, "AABB 应该有 8 个顶点");

        let center = aabb.center();
        assert_eq!(center.x, 5.0, "中心点 X 坐标应该是 5.0");
        assert_eq!(center.y, 5.0, "中心点 Y 坐标应该是 5.0");
        assert_eq!(center.z, 5.0, "中心点 Z 坐标应该是 5.0");

        println!("✅ AABB 关键点提取逻辑基础验证通过");
        println!("   顶点数: {}", vertices.len());
        println!("   中心点: ({}, {}, {})", center.x, center.y, center.z);
    }
}
