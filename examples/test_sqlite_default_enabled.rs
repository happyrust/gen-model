/// 验证 SQLite 空间索引默认启用功能
///
/// 运行方式：
/// cargo run --example test_sqlite_default_enabled --features sqlite-index
use aios_database::spatial_index::SqliteSpatialIndex;

fn main() {
    println!("🔍 检查 SQLite 空间索引默认启用状态...\n");

    // 检查是否启用
    let is_enabled = SqliteSpatialIndex::is_enabled();

    if is_enabled {
        println!("✅ SQLite 空间索引已启用（默认启用）");
    } else {
        println!("❌ SQLite 空间索引未启用");
        println!("   提示：如果 feature 'sqlite-index' 未启用，请使用 --features sqlite-index");
        return;
    }

    // 尝试创建空间索引
    println!("\n📦 测试创建空间索引...");
    match SqliteSpatialIndex::with_default_path() {
        Ok(index) => {
            println!("✅ 空间索引创建成功");

            // 获取统计信息
            match index.get_stats() {
                Ok(stats) => {
                    println!("\n📊 索引统计信息:");
                    println!("   - 索引类型: {}", stats.index_type);
                    println!("   - 元素总数: {}", stats.total_elements);
                }
                Err(e) => {
                    println!("⚠️ 获取统计信息失败: {}", e);
                }
            }
        }
        Err(e) => {
            println!("❌ 创建空间索引失败: {}", e);
            return;
        }
    }

    println!("\n✅ 验证完成！SQLite 空间索引默认启用功能正常工作。");
}
