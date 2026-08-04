use aios_core::pdms_types::RefnoEnum;
use aios_core::{SUL_DB, get_db_option};
use std::time::Instant;

/// 测试TUBI的inst_relate记录是否正确保存
///
/// 这个测试程序用于验证BRAN的TUBI对应的aabb和world_trans是否正确保存到inst_relate表中
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🔍 开始测试TUBI的inst_relate记录保存情况...");

    let start_time = Instant::now();

    // 初始化数据库连接
    let db_option = get_db_option();
    println!("📊 数据库配置: {:?}", db_option.project_name);

    // 测试查询BRAN下的TUBI inst_relate记录
    let test_cases = vec![
        // 可以根据实际情况添加测试用的BRAN参考号
        "24383_66456", // 示例BRAN参考号
    ];

    for test_case in test_cases {
        println!("\n🧪 测试BRAN: {}", test_case);

        // 查询该BRAN下的所有TUBI inst_relate记录
        let sql = format!(
            r#"
            SELECT 
                in as refno,
                in.noun as noun,
                world_trans.d as world_trans,
                aabb.d as aabb,
                out.id as inst_info_id
            FROM inst_relate 
            WHERE in IN (
                SELECT value in 
                FROM pe_owner 
                WHERE out = pe:{}
                AND in.noun = 'TUBI'
            )
            AND world_trans.d != none
            AND aabb.d != none
            "#,
            test_case
        );

        println!("🔍 执行查询SQL...");
        let mut response = SUL_DB.query(&sql).await?;

        #[derive(Debug, serde::Deserialize)]
        struct TubiInstRelateResult {
            refno: RefnoEnum,
            noun: String,
            world_trans: Option<serde_json::Value>,
            aabb: Option<serde_json::Value>,
            inst_info_id: String,
        }

        match response.take::<Vec<TubiInstRelateResult>>(0) {
            Ok(results) => {
                if results.is_empty() {
                    println!("⚠️  未找到BRAN {} 下的TUBI inst_relate记录", test_case);
                } else {
                    println!("✅ 找到 {} 个TUBI inst_relate记录:", results.len());

                    for (i, result) in results.iter().enumerate() {
                        println!("  📋 TUBI #{}: {}", i + 1, result.refno);
                        println!("    - 类型: {}", result.noun);
                        println!(
                            "    - world_trans: {}",
                            if result.world_trans.is_some() {
                                "✅ 已保存"
                            } else {
                                "❌ 缺失"
                            }
                        );
                        println!(
                            "    - aabb: {}",
                            if result.aabb.is_some() {
                                "✅ 已保存"
                            } else {
                                "❌ 缺失"
                            }
                        );
                        println!("    - inst_info_id: {}", result.inst_info_id);

                        // 详细检查数据内容
                        if let Some(world_trans) = &result.world_trans {
                            println!(
                                "    - world_trans详情: {}",
                                serde_json::to_string_pretty(world_trans)?
                            );
                        }

                        if let Some(aabb) = &result.aabb {
                            println!("    - aabb详情: {}", serde_json::to_string_pretty(aabb)?);
                        }
                        println!();
                    }
                }
            }
            Err(e) => {
                println!("❌ 查询失败: {}", e);
            }
        }

        // 额外检查：查询该BRAN下所有TUBI的基本信息
        let basic_sql = format!(
            r#"
            SELECT 
                in as refno,
                in.noun as noun,
                COUNT() as inst_relate_count
            FROM inst_relate 
            WHERE in IN (
                SELECT value in 
                FROM pe_owner 
                WHERE out = pe:{}
                AND in.noun = 'TUBI'
            )
            GROUP BY in
            "#,
            test_case
        );

        println!("🔍 检查TUBI基本信息...");
        let mut basic_response = SUL_DB.query(&basic_sql).await?;

        #[derive(Debug, serde::Deserialize)]
        struct TubiBasicInfo {
            refno: RefnoEnum,
            noun: String,
            inst_relate_count: i64,
        }

        match basic_response.take::<Vec<TubiBasicInfo>>(0) {
            Ok(basic_results) => {
                if basic_results.is_empty() {
                    println!("⚠️  BRAN {} 下没有TUBI元素或没有inst_relate记录", test_case);
                } else {
                    println!("📊 BRAN {} 下的TUBI统计:", test_case);
                    for basic in basic_results {
                        println!(
                            "  - {}: {} 个inst_relate记录",
                            basic.refno, basic.inst_relate_count
                        );
                    }
                }
            }
            Err(e) => {
                println!("❌ 基本信息查询失败: {}", e);
            }
        }
    }

    let elapsed = start_time.elapsed();
    println!("\n⏱️  测试完成，总耗时: {}ms", elapsed.as_millis());

    println!("\n📋 测试总结:");
    println!("1. 如果看到 '✅ 已保存' 说明aabb和world_trans字段已正确保存");
    println!("2. 如果看到 '❌ 缺失' 说明字段未保存，需要重新生成模型");
    println!("3. 如果没有找到记录，可能需要先运行模型生成");

    Ok(())
}
