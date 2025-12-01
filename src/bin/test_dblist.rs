//! dblist 解析和模型生成测试工具
//!
//! 使用新的基于 NamedAttrMap 的解析器

#![cfg_attr(
    not(feature = "test"),
    allow(dead_code, unused_imports, unused_variables)
)]

#[cfg(feature = "test")]
mod impls {
    use anyhow::Result;
    use clap::{Arg, Command};
    use std::str::FromStr;

    // 使用 aios-core 中的新解析器
    use aios_core::RefnoEnum;
    use aios_core::dblist_parser::DblistParser;
    use aios_core::test::test_surreal::test_helpers::init_sul_db_with_memory;
    use aios_core::{SUL_DB, SurrealQueryExt};
    use serde_json::json;

    /// 打印元素树结构
    fn print_elements_tree(elements: &[aios_core::dblist_parser::PdmsElement], depth: usize) {
        let indent = "  ".repeat(depth);

        for element in elements {
            println!(
                "{}📦 {} ({}) - {} 个属性",
                indent,
                element.get_noun(),
                element.get_refno_string(),
                element.attributes.map.len()
            );

            // 打印属性（只显示前5个）
            for (i, (name, value)) in element.attributes.map.iter().enumerate() {
                if i >= 5 {
                    println!(
                        "{}  ... 还有 {} 个属性",
                        indent,
                        element.attributes.map.len() - 5
                    );
                    break;
                }
                println!("{}  🔧 {}: {:?}", indent, name, value);
            }

            // 递归打印子元素
            if !element.children.is_empty() {
                print_elements_tree(&element.children, depth + 1);
            }
        }
    }

    /// 递归保存所有元素到数据库
    async fn save_all_elements_recursive(
        elements: &[aios_core::dblist_parser::PdmsElement],
    ) -> Result<i32> {
        save_elements_recursive_helper(elements).await
    }

    /// 辅助函数处理递归保存
    async fn save_elements_recursive_helper(
        elements: &[aios_core::dblist_parser::PdmsElement],
    ) -> Result<i32> {
        let mut total_count = 0;

        for element in elements {
            // 将 PdmsElement 转换为数据库格式
            let db_data = convert_element_to_db_format(element);

            // 打印转换后的数据结构
            println!(
                "\n📋 保存元素: {} ({})",
                element.get_noun(),
                element.get_refno_string()
            );
            println!("  属性数量: {}", element.attributes.map.len());

            // 插入数据库
            let sql = "CREATE pe SET data = $data;";
            match SUL_DB.query(sql).bind(("data", db_data)).await {
                Ok(_) => {
                    total_count += 1;
                    println!("  ✅ 保存成功 (总计: {})", total_count);
                }
                Err(e) => {
                    println!("  ❌ 保存失败: {}", e);
                    return Err(e.into());
                }
            }

            // 递归保存子元素
            if !element.children.is_empty() {
                let child_count =
                    Box::pin(save_elements_recursive_helper(&element.children)).await?;
                total_count += child_count;
            }
        }

        Ok(total_count)
    }

    /// 验证加载的数据
    async fn verify_loaded_data() -> Result<()> {
        // 查询总数 - 使用聚合查询
        let total_count: Option<i64> = SUL_DB
            .query_take(
                "SELECT VALUE count() FROM ONLY (SELECT count() FROM pe GROUP ALL)",
                0,
            )
            .await?;
        println!("📊 数据库验证:");
        println!("  总记录数: {}", total_count.unwrap_or(0));

        println!("  ✅ 数据验证完成");

        Ok(())
    }

    /// 简单的模型生成测试
    async fn test_model_generation() -> Result<()> {
        println!("🏗️  开始生成模型...");

        // 简化模型生成测试，只模拟处理过程
        println!("🔄 模拟处理模型结点...");

        // 模拟一些处理时间
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        println!("✅ 模型生成完成（模拟）");
        Ok(())
    }

    pub async fn main_impl() -> Result<()> {
        let matches = Command::new("test_dblist")
            .version("1.0")
            .about("dblist 解析和模型生成测试工具")
            .arg(
                Arg::new("file")
                    .help("要解析的 dblist 文件路径")
                    .required(true)
                    .index(1),
            )
            .arg(
                Arg::new("generate")
                    .help("解析完成后执行模型生成")
                    .long("generate")
                    .short('g')
                    .action(clap::ArgAction::SetTrue),
            )
            .get_matches();

        let file_path = matches.get_one::<String>("file").unwrap();
        let should_generate = matches.get_flag("generate");

        println!("🚀 开始解析 dblist 文件: {}", file_path);

        // 使用新的解析器
        let mut parser = DblistParser::new();
        let elements = parser.parse_file(file_path)?;

        println!("📚 解析完成，共找到 {} 个元素", elements.len());

        // 初始化内存数据库
        println!("🧠 初始化内存数据库");
        init_sul_db_with_memory().await?;

        // 清理现有数据
        println!("🧹 清理现有数据");
        SUL_DB.query("DELETE FROM pe;").await?;

        // 加载解析的元素到数据库
        println!("📦 开始加载数据到数据库");
        let mut loaded_count = 0;

        // 详细打印解析结果
        println!("\n🔍 解析结果详细分析:");
        print_elements_tree(&elements, 0);

        // 递归保存所有元素
        let save_result = save_all_elements_recursive(&elements).await;
        loaded_count = save_result?;

        println!("✅ 成功加载 {} 个元素到内存数据库", loaded_count);

        // 验证加载的数据
        verify_loaded_data().await?;

        // 如果指定了生成选项，执行模型生成
        if should_generate {
            test_model_generation().await?;
        }

        println!("🎉 dblist 解析测试完成！");
        Ok(())
    }

    /// 将 PdmsElement 转换为数据库格式
    fn convert_element_to_db_format(
        element: &aios_core::dblist_parser::PdmsElement,
    ) -> serde_json::Value {
        let mut data = json!({
            "refno": element.get_refno_string(),
            "noun": element.get_noun(),
            "attributes": serde_json::Map::new(),
        });

        // 转换 NamedAttrMap 为 JSON
        if let Some(attributes_obj) = data.as_object_mut() {
            let mut attrs_map = serde_json::Map::new();

            for (name, value) in &element.attributes.map {
                attrs_map.insert(name.clone(), convert_named_attr_value_to_json(value));
            }

            attributes_obj.insert(
                "attributes".to_string(),
                serde_json::Value::Object(attrs_map),
            );
        }

        data
    }

    /// 将 NamedAttrValue 转换为 JSON
    fn convert_named_attr_value_to_json(value: &aios_core::NamedAttrValue) -> serde_json::Value {
        use aios_core::NamedAttrValue;

        match value {
            NamedAttrValue::StringType(s) => serde_json::Value::String(s.clone()),
            NamedAttrValue::IntegerType(i) => {
                serde_json::Value::Number(serde_json::Number::from(*i))
            }
            NamedAttrValue::F32Type(f) => serde_json::Value::Number(
                serde_json::Number::from_f64(*f as f64)
                    .unwrap_or_else(|| serde_json::Number::from(0)),
            ),
            NamedAttrValue::BoolType(b) => serde_json::Value::Bool(*b),
            NamedAttrValue::ElementType(s) => serde_json::Value::String(s.clone()),
            NamedAttrValue::WordType(s) => serde_json::Value::String(s.clone()),
            NamedAttrValue::RefU64Type(r) => serde_json::Value::String(format!("{}", r.0)),
            NamedAttrValue::Vec3Type(v) => serde_json::Value::Array(vec![
                serde_json::Value::Number(
                    serde_json::Number::from_f64(v.x as f64)
                        .unwrap_or_else(|| serde_json::Number::from(0)),
                ),
                serde_json::Value::Number(
                    serde_json::Number::from_f64(v.y as f64)
                        .unwrap_or_else(|| serde_json::Number::from(0)),
                ),
                serde_json::Value::Number(
                    serde_json::Number::from_f64(v.z as f64)
                        .unwrap_or_else(|| serde_json::Number::from(0)),
                ),
            ]),
            NamedAttrValue::IntArrayType(arr) => serde_json::Value::Array(
                arr.iter()
                    .map(|&i| serde_json::Value::Number(serde_json::Number::from(i)))
                    .collect(),
            ),
            NamedAttrValue::StringArrayType(arr) => serde_json::Value::Array(
                arr.iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
            NamedAttrValue::BoolArrayType(arr) => {
                serde_json::Value::Array(arr.iter().map(|&b| serde_json::Value::Bool(b)).collect())
            }
            _ => serde_json::Value::String("unknown".to_string()),
        }
    }
}

#[cfg(feature = "test")]
#[tokio::main]
async fn main() -> impls::Result<()> {
    impls::main_impl().await
}

#[cfg(not(feature = "test"))]
fn main() {
    eprintln!("test_dblist 需要启用 `test` feature，已跳过编译。");
}
