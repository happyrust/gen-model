/// 房间集成测试
///
/// 完整流程：查询房间信息 -> 模型生成 -> 房间计算
/// 使用真实数据库连接和配置

#[cfg(test)]
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
mod tests {
    use crate::options::get_db_option_ext;
    use aios_core::room::query_room_panels_by_keywords;
    use aios_core::{RefnoEnum, SUL_DB, get_db_option, init_surreal};
    use anyhow::{Context, Result};
    use std::time::Instant;

    /// 完整的房间测试流程
    ///
    /// 测试步骤：
    /// 1. 初始化数据库连接
    /// 2. 查询房间信息
    /// 3. 触发模型生成
    /// 4. 执行房间计算
    /// 5. 验证结果
    #[tokio::test]
    #[ignore = "需要真实数据库连接，手动运行"]
    async fn test_room_integration_complete() -> Result<()> {
        println!("\n🏗️  房间集成测试开始");
        println!("{}", "=".repeat(80));

        let overall_start = Instant::now();

        // ===== 步骤 1: 初始化数据库连接 =====
        println!("\n📡 步骤 1: 初始化数据库连接");
        println!("{}", "-".repeat(80));

        init_surreal().await.context("初始化 SurrealDB 失败")?;

        let db_option = get_db_option();
        println!("✅ 数据库连接成功");
        println!("   项目名称: {}", db_option.project_name);
        println!("   项目代码: {}", db_option.project_code);
        println!("   Mesh 路径: {}", db_option.get_meshes_path().display());

        // ===== 步骤 2: 查询房间信息 =====
        println!("\n🔍 步骤 2: 查询房间信息");
        println!("{}", "-".repeat(80));

        let room_keywords = db_option.get_room_key_word();
        println!("🏷️  房间关键词: {:?}", room_keywords);

        let query_start = Instant::now();
        let room_panel_map = query_room_panels_by_keywords(&room_keywords)
            .await
            .context("查询房间信息失败")?;
        let query_duration = query_start.elapsed();

        println!("✅ 房间查询完成");
        println!("   查询耗时: {:?}", query_duration);
        println!("   房间数量: {}", room_panel_map.len());

        // 收集统计信息
        let mut total_panels = 0;
        let mut max_panels_per_room = 0;
        let mut min_panels_per_room = usize::MAX;

        for (room_refno, room_num, panel_refnos) in &room_panel_map {
            total_panels += panel_refnos.len();
            max_panels_per_room = max_panels_per_room.max(panel_refnos.len());
            min_panels_per_room = min_panels_per_room.min(panel_refnos.len());

            println!(
                "   📍 房间 {} (refno={}): {} 个面板",
                room_num,
                room_refno,
                panel_refnos.len()
            );
        }

        if room_panel_map.is_empty() {
            println!("⚠️  警告: 未找到任何房间，测试结束");
            return Ok(());
        }

        println!("\n   总面板数: {}", total_panels);
        println!(
            "   平均每房间面板数: {:.2}",
            total_panels as f64 / room_panel_map.len() as f64
        );
        println!("   最多面板的房间: {} 个面板", max_panels_per_room);
        println!("   最少面板的房间: {} 个面板", min_panels_per_room);

        // 收集所有需要生成模型的 refnos
        let mut all_panel_refnos = Vec::new();
        for (_, _, panel_refnos) in &room_panel_map {
            all_panel_refnos.extend(panel_refnos.clone());
        }

        println!("\n   待生成模型的元素数: {}", all_panel_refnos.len());

        // ===== 步骤 3: 触发模型生成 =====
        println!("\n⚙️  步骤 3: 触发模型生成");
        println!("{}", "-".repeat(80));

        // 创建模型生成配置
        let mut gen_db_option = get_db_option_ext();
        gen_db_option.gen_model = true;
        gen_db_option.gen_mesh = true;
        gen_db_option.apply_boolean_operation = true;

        println!("🔧 模型生成配置:");
        println!("   gen_model: {}", gen_db_option.gen_model);
        println!("   gen_mesh: {}", gen_db_option.gen_mesh);
        println!(
            "   apply_boolean_operation: {:?}",
            gen_db_option.apply_boolean_operation
        );
        println!("   mesh_tol_ratio: {:?}", gen_db_option.mesh_tol_ratio);

        let gen_start = Instant::now();

        use crate::fast_model::gen_model::gen_all_geos_data;

        match gen_all_geos_data(all_panel_refnos.clone(), &gen_db_option, None, None).await {
            Ok(_) => {
                let gen_duration = gen_start.elapsed();
                println!("✅ 模型生成完成");
                println!("   生成耗时: {:?}", gen_duration);
                println!("   处理元素数: {}", all_panel_refnos.len());
                println!(
                    "   平均每元素耗时: {:.2}ms",
                    gen_duration.as_millis() as f64 / all_panel_refnos.len() as f64
                );
            }
            Err(e) => {
                println!("❌ 模型生成失败: {}", e);
                return Err(e.into());
            }
        }

        // ===== 步骤 4: 执行房间计算 =====
        println!("\n🏠 步骤 4: 执行房间计算");
        println!("{}", "-".repeat(80));

        let room_start = Instant::now();

        use crate::fast_model::room_model::build_room_relations;

        match build_room_relations(&db_option).await {
            Ok(stats) => {
                let room_duration = room_start.elapsed();
                println!("✅ 房间计算完成");
                println!("   计算耗时: {:?}", room_duration);
                println!("   处理房间数: {}", stats.total_rooms);
                println!("   处理面板数: {}", stats.total_panels);
                println!("   处理构件数: {}", stats.total_components);
                println!(
                    "   平均每房间构件数: {:.2}",
                    stats.total_components as f64 / stats.total_rooms as f64
                );
                println!("   内存使用: {:.2} MB", stats.memory_usage_mb);
            }
            Err(e) => {
                println!("❌ 房间计算失败: {}", e);
                return Err(e.into());
            }
        }

        // ===== 步骤 5: 验证结果 =====
        println!("\n✅ 步骤 5: 验证结果");
        println!("{}", "-".repeat(80));

        // 查询房间关系数量
        let verify_sql = "SELECT VALUE count() FROM room_relate GROUP ALL LIMIT 1";
        let mut response = SUL_DB.query(verify_sql).await?;
        let relation_count: Option<i64> = response.take(0).ok().flatten();

        println!("📊 数据库验证:");
        println!("   room_relate 关系数: {:?}", relation_count);

        // 总结
        let total_duration = overall_start.elapsed();
        println!("\n{}", "=".repeat(80));
        println!("🎉 测试完成");
        println!("   总耗时: {:?}", total_duration);
        println!(
            "   查询耗时: {:?} ({:.1}%)",
            query_duration,
            query_duration.as_secs_f64() / total_duration.as_secs_f64() * 100.0
        );
        println!("{}", "=".repeat(80));

        Ok(())
    }

    /// 测试只查询房间信息（不生成模型和计算）
    ///
    /// 用于快速验证房间查询逻辑
    #[tokio::test]
    #[ignore = "需要真实数据库连接，手动运行"]
    async fn test_query_room_info_only() -> Result<()> {
        println!("\n🔍 房间信息查询测试");
        println!("{}", "=".repeat(80));

        // 初始化数据库
        init_surreal().await.context("初始化 SurrealDB 失败")?;
        let db_option = get_db_option();

        // 查询房间
        let room_keywords = db_option.get_room_key_word();
        println!("🏷️  房间关键词: {:?}", room_keywords);

        let room_panel_map = query_room_panels_by_keywords(&room_keywords)
            .await
            .context("查询房间信息失败")?;

        println!("\n✅ 找到 {} 个房间", room_panel_map.len());

        // 详细输出每个房间的信息
        for (i, (room_refno, room_num, panel_refnos)) in room_panel_map.iter().enumerate() {
            println!("\n房间 #{} - {}", i + 1, room_num);
            println!("  Room Refno: {}", room_refno);
            println!("  面板数量: {}", panel_refnos.len());
            println!("  面板列表:");
            for (j, panel) in panel_refnos.iter().enumerate() {
                println!("    [{}] {}", j + 1, panel);
            }
        }

        Ok(())
    }

    /// 测试针对特定房间号重建关系
    ///
    /// 适用于需要重新计算特定房间的场景
    #[tokio::test]
    #[ignore = "需要真实数据库连接，手动运行"]
    async fn test_rebuild_specific_rooms() -> Result<()> {
        println!("\n🔄 特定房间关系重建测试");
        println!("{}", "=".repeat(80));

        // 初始化数据库
        init_surreal().await.context("初始化 SurrealDB 失败")?;
        let db_option = get_db_option();

        // 查询房间
        let room_keywords = db_option.get_room_key_word();
        let room_panel_map = query_room_panels_by_keywords(&room_keywords)
            .await
            .context("查询房间信息失败")?;

        if room_panel_map.is_empty() {
            println!("⚠️  警告: 未找到任何房间");
            return Ok(());
        }

        // 选择前 3 个房间进行测试（如果有的话）
        let test_room_count = room_panel_map.len().min(3);
        let test_room_numbers: Vec<String> = room_panel_map
            .iter()
            .take(test_room_count)
            .map(|(_, room_num, _)| room_num.clone())
            .collect();

        println!("🎯 测试房间: {:?}", test_room_numbers);

        // 执行重建
        use crate::fast_model::room_model::rebuild_room_relations_for_rooms;

        let rebuild_start = Instant::now();

        match rebuild_room_relations_for_rooms(Some(test_room_numbers), &db_option).await {
            Ok(stats) => {
                let rebuild_duration = rebuild_start.elapsed();
                println!("\n✅ 房间关系重建完成");
                println!("   耗时: {:?}", rebuild_duration);
                println!("   处理房间数: {}", stats.total_rooms);
                println!("   处理面板数: {}", stats.total_panels);
                println!("   处理构件数: {}", stats.total_components);
                println!("   内存使用: {:.2} MB", stats.memory_usage_mb);
            }
            Err(e) => {
                println!("❌ 房间关系重建失败: {}", e);
                return Err(e.into());
            }
        }

        Ok(())
    }

    /// 测试限制房间数量的集成流程
    ///
    /// 用于在大规模数据库中快速验证流程，只处理前 N 个房间
    #[tokio::test]
    #[ignore = "需要真实数据库连接，手动运行"]
    async fn test_limited_room_integration() -> Result<()> {
        const MAX_ROOMS: usize = 5; // 只处理前 5 个房间

        println!("\n🏗️  限制房间数量集成测试 (最多 {} 个房间)", MAX_ROOMS);
        println!("{}", "=".repeat(80));

        // 初始化数据库
        init_surreal().await.context("初始化 SurrealDB 失败")?;
        let db_option = get_db_option();

        // 查询房间
        let room_keywords = db_option.get_room_key_word();
        let mut room_panel_map = query_room_panels_by_keywords(&room_keywords)
            .await
            .context("查询房间信息失败")?;

        // 限制房间数量
        room_panel_map.truncate(MAX_ROOMS);

        println!("✅ 查询到 {} 个房间（限制后）", room_panel_map.len());

        if room_panel_map.is_empty() {
            println!("⚠️  警告: 未找到任何房间");
            return Ok(());
        }

        // 收集面板 refnos
        let all_panel_refnos: Vec<RefnoEnum> = room_panel_map
            .iter()
            .flat_map(|(_, _, panels)| panels.clone())
            .collect();

        println!("📊 待处理元素数: {}", all_panel_refnos.len());

        // 生成模型
        println!("\n⚙️  生成模型...");
        use crate::fast_model::gen_model::gen_all_geos_data;

        let mut gen_db_option = get_db_option_ext();
        gen_db_option.gen_model = true;
        gen_db_option.gen_mesh = true;

        gen_all_geos_data(all_panel_refnos, &gen_db_option, None, None)
            .await
            .context("模型生成失败")?;

        println!("✅ 模型生成完成");

        // 房间计算
        println!("\n🏠 执行房间计算...");
        use crate::fast_model::room_model::build_room_relations;

        let stats = build_room_relations(&db_option)
            .await
            .context("房间计算失败")?;

        println!("✅ 房间计算完成");
        println!("   处理房间数: {}", stats.total_rooms);
        println!("   处理构件数: {}", stats.total_components);

        Ok(())
    }
}
