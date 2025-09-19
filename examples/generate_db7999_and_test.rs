use anyhow::Result;
use std::path::PathBuf;
/// 生成DB 7999模型数据并测试SCTN空间接触检测
///
/// 步骤：
/// 1. 生成DB 7999的模型数据
/// 2. 构建空间索引
/// 3. 查询SCTN 24383/86525
/// 4. 执行接触检测
///
/// 运行: cargo run --example generate_db7999_and_test --features grpc,sqlite-index
use std::sync::Arc;

#[cfg(all(feature = "grpc", feature = "sqlite-index"))]
async fn main() -> Result<()> {
    use aios_core::pdms_types::RefU64;
    use aios_database::data_interface::tidb_manager::AiosDBManager;
    use aios_database::fast_model::gen_model::GenModel;
    use aios_database::grpc_service::spatial_index_builder::{
        SpatialIndexBuilder, SpatialIndexConfig, SpatialIndexPersistence,
    };
    use aios_database::spatial_index::SqliteSpatialIndex;
    use std::str::FromStr;

    // 初始化日志
    env_logger::init();

    println!("========================================");
    println!("DB 7999 模型生成与空间索引测试");
    println!("========================================\n");

    // 步骤1: 连接数据库并生成模型
    println!("步骤1: 连接数据库并生成DB 7999的模型数据...");

    // 创建数据库管理器
    let db_config = load_db_config()?;
    let db_manager = Arc::new(AiosDBManager::new(&db_config).await?);

    // 生成DB 7999的模型
    let db_nums = vec![7999];
    println!("正在生成DB {} 的模型数据...", db_nums[0]);

    let gen_model = GenModel::new(db_manager.clone());
    let model_result = gen_model.generate_models(&db_nums).await?;

    println!("✓ 模型生成完成:");
    println!("  - 生成的元素数: {}", model_result.total_elements);
    println!("  - SCTN数量: {}", model_result.sctn_count);
    println!("  - 支架数量: {}", model_result.support_count);
    println!("  - 管道数量: {}", model_result.pipe_count);

    // 步骤2: 构建空间索引
    println!("\n步骤2: 构建空间索引...");

    let config = SpatialIndexConfig {
        bbox_tolerance: 0.001,
        batch_size: 10000,
        include_negative_entities: false,
        filter_types: vec![], // 包含所有类型
        min_bbox_size: 0.0001,
    };

    let builder = SpatialIndexBuilder::new(db_manager.clone()).with_config(config);

    let (rtree, statistics) = builder.build_from_database(&db_nums).await?;

    println!("✓ 空间索引构建完成:");
    println!("  - 索引元素数: {}", statistics.indexed_elements);
    println!("  - 跳过元素数: {}", statistics.skipped_elements);
    println!("  - 构建时间: {}ms", statistics.build_time_ms);
    println!("  - 内存占用: {:.2}MB", statistics.memory_estimate_mb);

    // 保存索引到文件
    let index_file = PathBuf::from("db7999_spatial_index.bin");
    SpatialIndexPersistence::save_index(&rtree, &statistics, &index_file)?;
    println!("✓ 索引已保存到: {:?}", index_file);

    // 步骤3: 初始化SQLite空间索引
    println!("\n步骤3: 初始化SQLite R-Tree索引...");

    let sqlite_index = SqliteSpatialIndex::new("db7999_spatial.sqlite")?;
    sqlite_index.clear()?;

    // 将数据插入SQLite索引
    let mut insert_batch = Vec::new();
    for element in rtree.iter() {
        insert_batch.push((
            element.refno,
            element.bbox.clone(),
            Some(element.element_type.clone()),
        ));

        if insert_batch.len() >= 1000 {
            sqlite_index.insert_many(insert_batch.drain(..))?;
        }
    }

    if !insert_batch.is_empty() {
        sqlite_index.insert_many(insert_batch)?;
    }

    let stats = sqlite_index.get_stats()?;
    println!("✓ SQLite索引创建完成: {} 个元素", stats.total_elements);

    // 步骤4: 查询SCTN 24383/86525
    println!("\n步骤4: 查询SCTN 24383/86525...");

    let target_refno = RefU64::from_str("24383/86525").unwrap();

    // 从SQLite索引获取包围盒
    if let Some(bbox) = sqlite_index.get_aabb(target_refno)? {
        println!("✓ 找到目标SCTN {}", target_refno.0);
        println!(
            "  包围盒: ({:.2}, {:.2}, {:.2}) - ({:.2}, {:.2}, {:.2})",
            bbox.mins.x, bbox.mins.y, bbox.mins.z, bbox.maxs.x, bbox.maxs.y, bbox.maxs.z
        );

        // 扩展包围盒查询周围构件
        use nalgebra::Vector3;
        use parry3d::bounding_volume::Aabb;

        let query_bbox = Aabb::new(
            bbox.mins - Vector3::new(1.0, 1.0, 1.0),
            bbox.maxs + Vector3::new(1.0, 1.0, 1.0),
        );

        let nearby = sqlite_index.query_intersect(&query_bbox)?;
        println!("  周围1米内的构件数: {}", nearby.len());

        // 步骤5: 执行接触检测
        println!("\n步骤5: 执行接触检测...");

        use aios_database::grpc_service::sctn_contact_detector::{
            CableTraySection, SctnContactDetector,
        };
        use aios_database::grpc_service::spatial_query_service::SpatialElement;

        // 创建SCTN数据
        let sctn = CableTraySection {
            refno: target_refno,
            bbox: bbox.clone(),
            centerline: vec![bbox.center()],
            width: 0.6,  // 假设600mm宽
            height: 0.3, // 假设300mm高
            depth: (bbox.maxs.x - bbox.mins.x).max(1.0),
            direction: Vector3::new(1.0, 0.0, 0.0),
            support_points: vec![],
            section_type: "SCTN".to_string(),
        };

        let detector = SctnContactDetector::new(0.1)?; // 100mm容差

        // 获取附近构件的详细信息
        let mut candidates = Vec::new();
        for refno in nearby.iter().take(20) {
            // 只检查前20个
            if *refno == target_refno {
                continue;
            }

            if let Some(candidate_bbox) = sqlite_index.get_aabb(*refno)? {
                candidates.push(SpatialElement {
                    refno: *refno,
                    bbox: candidate_bbox,
                    element_type: "UNKNOWN".to_string(),
                    element_name: format!("Element_{}", refno.0),
                    last_updated: std::time::SystemTime::now(),
                });
            }
        }

        println!("检测 {} 个候选构件的接触关系...", candidates.len());

        let mut contact_count = 0;
        let mut proximity_count = 0;

        for candidate in &candidates {
            if let Some(contact) = detector.check_detailed_contact(&sctn, candidate, true)? {
                use aios_database::grpc_service::sctn_contact_detector::ContactType;

                match contact.contact_type {
                    ContactType::Surface | ContactType::Edge | ContactType::Point => {
                        contact_count += 1;
                        println!(
                            "  ⚠️ 接触: {} 距离 {:.3}m",
                            candidate.refno.0, contact.distance
                        );
                    }
                    ContactType::Penetration => {
                        contact_count += 1;
                        println!(
                            "  ❌ 穿透: {} 深度 {:.3}m",
                            candidate.refno.0, contact.penetration_depth
                        );
                    }
                    ContactType::Proximity => {
                        proximity_count += 1;
                    }
                    _ => {}
                }
            }
        }

        println!("\n检测结果汇总:");
        println!("  - 接触/碰撞: {} 个", contact_count);
        println!("  - 接近关系: {} 个", proximity_count);
    } else {
        println!("⚠️ 未找到SCTN {}", target_refno.0);
        println!("可能该SCTN不在DB 7999中，尝试列出一些可用的SCTN...");

        // 列出一些SCTN供参考
        for element in rtree.iter().take(10) {
            if element.element_type == "SCTN" {
                println!("  可用SCTN: {}", element.refno.0);
            }
        }
    }

    println!("\n========================================");
    println!("测试完成！");
    println!("========================================");

    Ok(())
}

#[cfg(not(all(feature = "grpc", feature = "sqlite-index")))]
fn main() {
    println!("此示例需要启用 grpc 和 sqlite-index 特性");
    println!("请使用: cargo run --example generate_db7999_and_test --features grpc,sqlite-index");
}

/// 加载数据库配置
fn load_db_config() -> Result<DbConfig> {
    use config::Config;
    use std::path::Path;

    // 尝试从DbOption.toml加载配置
    let config_file = if Path::new("DbOption.toml").exists() {
        "DbOption"
    } else if Path::new("DbOption-zsy.toml").exists() {
        "DbOption-zsy"
    } else {
        return Err(anyhow::anyhow!("未找到数据库配置文件"));
    };

    let settings = Config::builder()
        .add_source(config::File::with_name(config_file))
        .build()?;

    Ok(DbConfig {
        host: settings
            .get_string("database.host")
            .unwrap_or_else(|_| "localhost".to_string()),
        port: settings.get_int("database.port").unwrap_or(3306) as u16,
        user: settings
            .get_string("database.user")
            .unwrap_or_else(|_| "root".to_string()),
        password: settings.get_string("database.password").unwrap_or_default(),
        database: settings
            .get_string("database.name")
            .unwrap_or_else(|_| "aios".to_string()),
    })
}

#[derive(Debug)]
struct DbConfig {
    host: String,
    port: u16,
    user: String,
    password: String,
    database: String,
}

/// 模型生成结果
struct ModelGenerationResult {
    total_elements: usize,
    sctn_count: usize,
    support_count: usize,
    pipe_count: usize,
}
