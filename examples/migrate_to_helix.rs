/// 数据迁移工具：从 SurrealDB 迁移到 HelixDB
///
/// 基于 pe_owner 字段创建图关系
///
/// 运行: cargo run --example migrate_to_helix

use aios_core::pdms_types::RefU64;
use aios_database::data_interface::helix_manager::{HelixConfig, HelixDBManager};
use indicatif::{ProgressBar, ProgressStyle};
use neo4rs::query;
use std::time::Instant;

#[derive(Debug)]
struct ElementData {
    refno: RefU64,
    pe_owner: RefU64,
    type_name: String,
    name: String,
}

/// 步骤 1: 从 SurrealDB 读取数据
async fn load_data_from_surrealdb() -> anyhow::Result<Vec<ElementData>> {
    println!("\n📖 步骤 1: 从 SurrealDB 读取数据");
    println!("{}", "-".repeat(80));

    // TODO: 实际从 SurrealDB 读取
    // 这里使用模拟数据展示流程

    let sample_data = vec![
        ElementData {
            refno: RefU64(1001),
            pe_owner: RefU64(0),
            type_name: "SITE".to_string(),
            name: "Site001".to_string(),
        },
        ElementData {
            refno: RefU64(1002),
            pe_owner: RefU64(1001),
            type_name: "ZONE".to_string(),
            name: "Zone01".to_string(),
        },
        ElementData {
            refno: RefU64(1003),
            pe_owner: RefU64(1001),
            type_name: "ZONE".to_string(),
            name: "Zone02".to_string(),
        },
        ElementData {
            refno: RefU64(1004),
            pe_owner: RefU64(1002),
            type_name: "EQUI".to_string(),
            name: "Equipment01".to_string(),
        },
        ElementData {
            refno: RefU64(1005),
            pe_owner: RefU64(1002),
            type_name: "EQUI".to_string(),
            name: "Equipment02".to_string(),
        },
        ElementData {
            refno: RefU64(1006),
            pe_owner: RefU64(1003),
            type_name: "EQUI".to_string(),
            name: "Equipment03".to_string(),
        },
        ElementData {
            refno: RefU64(1007),
            pe_owner: RefU64(1004),
            type_name: "PIPE".to_string(),
            name: "Pipe001".to_string(),
        },
        ElementData {
            refno: RefU64(1008),
            pe_owner: RefU64(1004),
            type_name: "PIPE".to_string(),
            name: "Pipe002".to_string(),
        },
    ];

    println!("✅ 读取 {} 个元素", sample_data.len());

    // 统计类型分布
    let mut type_counts = std::collections::HashMap::new();
    for element in &sample_data {
        *type_counts.entry(&element.type_name).or_insert(0) += 1;
    }

    println!("\n类型分布:");
    for (type_name, count) in type_counts {
        println!("  {}: {} 个", type_name, count);
    }

    Ok(sample_data)
}

/// 步骤 2: 创建节点
async fn create_nodes(
    helix: &HelixDBManager,
    elements: &[ElementData],
) -> anyhow::Result<()> {
    println!("\n🔨 步骤 2: 创建节点");
    println!("{}", "-".repeat(80));

    let pb = ProgressBar::new(elements.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("█▓▒░"),
    );

    let start = Instant::now();

    for element in elements {
        let q = query(
            "CREATE (e:Element {
                refno: $refno,
                pe_owner: $pe_owner,
                type_name: $type_name,
                name: $name
             })"
        )
        .param("refno", element.refno.0 as i64)
        .param("pe_owner", element.pe_owner.0 as i64)
        .param("type_name", &element.type_name)
        .param("name", &element.name);

        helix.graph.run(q).await?;

        pb.set_message(format!("创建节点 {} ({})", element.refno.0, element.type_name));
        pb.inc(1);
    }

    pb.finish_with_message("完成");

    let elapsed = start.elapsed();
    println!("✅ 创建 {} 个节点，耗时: {:?}", elements.len(), elapsed);

    Ok(())
}

/// 步骤 3: 创建关系（基于 pe_owner）
async fn create_relationships(helix: &HelixDBManager) -> anyhow::Result<usize> {
    println!("\n🔗 步骤 3: 创建关系（基于 pe_owner 字段）");
    println!("{}", "-".repeat(80));

    println!("\n💡 Cypher 查询:");
    println!("   MATCH (child:Element)");
    println!("   WHERE child.pe_owner IS NOT NULL AND child.pe_owner <> 0");
    println!("   MATCH (parent:Element {{refno: child.pe_owner}})");
    println!("   CREATE (parent)-[:HAS_CHILD]->(child)");

    let start = Instant::now();

    // 批量创建所有关系
    let q = query(
        "MATCH (child:Element)
         WHERE child.pe_owner IS NOT NULL AND child.pe_owner <> 0
         MATCH (parent:Element {refno: child.pe_owner})
         CREATE (parent)-[:HAS_CHILD]->(child)
         RETURN count(*) as count"
    );

    let mut result = helix.graph.execute(q).await?;
    let count = if let Some(row) = result.next().await? {
        row.get::<i64>("count")? as usize
    } else {
        0
    };

    let elapsed = start.elapsed();
    println!("✅ 创建 {} 条关系，耗时: {:?}", count, elapsed);

    Ok(count)
}

/// 步骤 4: 创建索引
async fn create_indexes(helix: &HelixDBManager) -> anyhow::Result<()> {
    println!("\n📇 步骤 4: 创建索引");
    println!("{}", "-".repeat(80));

    let indexes = vec![
        ("refno", "CREATE INDEX ON :Element(refno)"),
        ("pe_owner", "CREATE INDEX ON :Element(pe_owner)"),
        ("type_name", "CREATE INDEX ON :Element(type_name)"),
    ];

    for (name, cypher) in indexes {
        println!("   创建索引: {}", name);
        helix.graph.run(query(cypher)).await?;
    }

    println!("✅ 索引创建完成");

    Ok(())
}

/// 步骤 5: 验证迁移结果
async fn verify_migration(helix: &HelixDBManager) -> anyhow::Result<()> {
    println!("\n✓ 步骤 5: 验证迁移结果");
    println!("{}", "-".repeat(80));

    // 验证 1: 节点数量
    let q = query("MATCH (n:Element) RETURN count(n) as count");
    let mut result = helix.graph.execute(q).await?;
    let node_count = if let Some(row) = result.next().await? {
        row.get::<i64>("count")?
    } else {
        0
    };
    println!("   节点总数: {}", node_count);

    // 验证 2: 关系数量
    let q = query("MATCH ()-[r:HAS_CHILD]->() RETURN count(r) as count");
    let mut result = helix.graph.execute(q).await?;
    let rel_count = if let Some(row) = result.next().await? {
        row.get::<i64>("count")?
    } else {
        0
    };
    println!("   关系总数: {}", rel_count);

    // 验证 3: 根节点
    let q = query(
        "MATCH (root:Element)
         WHERE NOT ()-[:HAS_CHILD]->(root)
         RETURN root.refno as refno, root.type_name as type_name"
    );
    let mut result = helix.graph.execute(q).await?;
    print!("   根节点: ");
    while let Some(row) = result.next().await? {
        let refno: i64 = row.get("refno")?;
        let type_name: String = row.get("type_name")?;
        print!("{} ({}), ", refno, type_name);
    }
    println!();

    // 验证 4: 叶子节点
    let q = query(
        "MATCH (leaf:Element)
         WHERE NOT (leaf)-[:HAS_CHILD]->()
         RETURN count(leaf) as count"
    );
    let mut result = helix.graph.execute(q).await?;
    let leaf_count = if let Some(row) = result.next().await? {
        row.get::<i64>("count")?
    } else {
        0
    };
    println!("   叶子节点: {} 个", leaf_count);

    // 验证 5: 最大深度
    let q = query(
        "MATCH path = (root)-[:HAS_CHILD*]->(leaf)
         WHERE NOT ()-[:HAS_CHILD]->(root)
           AND NOT (leaf)-[:HAS_CHILD]->()
         RETURN max(length(path)) as max_depth"
    );
    let mut result = helix.graph.execute(q).await?;
    let max_depth = if let Some(row) = result.next().await? {
        row.get::<Option<i64>>("max_depth")?.unwrap_or(0)
    } else {
        0
    };
    println!("   最大深度: {}", max_depth);

    println!("\n✅ 验证通过");

    Ok(())
}

/// 步骤 6: 测试查询性能
async fn test_query_performance(helix: &HelixDBManager) -> anyhow::Result<()> {
    println!("\n⚡ 步骤 6: 测试查询性能");
    println!("{}", "-".repeat(80));

    // 测试 1: 获取所有子孙节点
    let test_root = RefU64(1001);
    println!("\n测试 1: 获取所有子孙节点 (root: {})", test_root.0);

    let start = Instant::now();
    let descendants = helix.get_descendants(test_root, Some(10)).await?;
    let elapsed = start.elapsed();

    println!("   查询耗时: {:?}", elapsed);
    println!("   子孙节点数: {}", descendants.len());

    // 测试 2: 类型过滤查询
    println!("\n测试 2: 查找所有 PIPE 节点");

    let start = Instant::now();
    let pipes = helix.get_descendants_by_type(test_root, &["PIPE"], Some(10)).await?;
    let elapsed = start.elapsed();

    println!("   查询耗时: {:?}", elapsed);
    println!("   PIPE 节点数: {}", pipes.len());

    // 测试 3: 特定深度查询
    println!("\n测试 3: 查找第 2 层的节点");

    let start = Instant::now();
    let nodes_at_depth_2 = helix.get_nodes_at_depth(test_root, 2, None).await?;
    let elapsed = start.elapsed();

    println!("   查询耗时: {:?}", elapsed);
    println!("   第 2 层节点数: {}", nodes_at_depth_2.len());

    // 测试 4: 统计分析
    println!("\n测试 4: 统计节点类型分布");

    let start = Instant::now();
    let stats = helix.count_descendants_by_type(test_root, Some(10)).await?;
    let elapsed = start.elapsed();

    println!("   查询耗时: {:?}", elapsed);
    println!("   类型分布:");
    for (type_name, count) in stats {
        println!("     {}: {} 个", type_name, count);
    }

    println!("\n✅ 性能测试完成");

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🚀 数据迁移工具：从 SurrealDB 迁移到 HelixDB");
    println!("📋 基于 pe_owner 字段创建图关系");
    println!("{}", "=".repeat(80));

    // 配置
    let config = HelixConfig {
        uri: std::env::var("HELIX_URI")
            .unwrap_or_else(|_| "bolt://localhost:7687".to_string()),
        user: std::env::var("HELIX_USER")
            .unwrap_or_else(|_| "neo4j".to_string()),
        password: std::env::var("HELIX_PASSWORD")
            .unwrap_or_else(|_| "password".to_string()),
        database: None,
    };

    println!("\n📡 连接到 HelixDB: {}", config.uri);

    let helix = HelixDBManager::connect(config).await?;
    helix.test_connection().await?;
    println!("✅ 连接成功");

    // 确认操作
    println!("\n⚠️  警告: 这将清空现有数据并重新导入");
    println!("   按 Enter 继续，Ctrl+C 取消...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    let total_start = Instant::now();

    // 清空现有数据
    println!("\n🗑️  清空现有数据...");
    helix.graph.run(query("MATCH (n) DETACH DELETE n")).await?;
    println!("✅ 清空完成");

    // 执行迁移步骤
    let elements = load_data_from_surrealdb().await?;
    create_nodes(&helix, &elements).await?;
    let rel_count = create_relationships(&helix).await?;
    create_indexes(&helix).await?;
    verify_migration(&helix).await?;
    test_query_performance(&helix).await?;

    let total_elapsed = total_start.elapsed();

    println!("\n{}", "=".repeat(80));
    println!("✅ 迁移完成");
    println!("{}", "=".repeat(80));

    println!("\n📊 迁移统计:");
    println!("   节点数: {}", elements.len());
    println!("   关系数: {}", rel_count);
    println!("   总耗时: {:?}", total_elapsed);

    println!("\n💡 下一步:");
    println!("   1. 使用 helix_pe_owner_demo 测试查询功能");
    println!("   2. 对比 SurrealDB 和 HelixDB 的性能差异");
    println!("   3. 根据实际需求调整索引和查询");

    println!("\n📚 相关文档:");
    println!("   - HELIX_PE_OWNER_IMPLEMENTATION.md");
    println!("   - examples/helix_pe_owner_demo.rs");

    Ok(())
}