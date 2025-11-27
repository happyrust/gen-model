//! dblist 数据库加载器
//! 
//! 将解析的 PDMS 元素数据加载到 SurrealDB 内存数据库中

use anyhow::Result;
use aios_core::{SUL_DB, SurrealQueryExt, RefnoEnum};
use serde_json::json;
use std::collections::HashMap;
use std::str::FromStr;

use crate::dblist_parser::parser::PdmsElement;

/// dblist 数据库加载器
pub struct DblistLoader {
    elements: Vec<PdmsElement>,
}

impl DblistLoader {
    pub fn new(elements: Vec<PdmsElement>) -> Self {
        Self { elements }
    }

    /// 将所有元素加载到内存数据库
    pub async fn load_to_memory_db(&self) -> Result<()> {
        // 确保使用内存数据库
        self.init_memory_db().await?;
        
        // 清理现有数据
        self.cleanup_existing_data().await?;
        
        // 创建表结构
        self.create_tables().await?;
        
        // 加载所有元素
        for element in &self.elements {
            self.load_element(element).await?;
            self.load_children(element).await?;
        }
        
        println!("✅ 成功加载 {} 个元素到内存数据库", self.elements.len());
        Ok(())
    }

    /// 初始化内存数据库
    async fn init_memory_db(&self) -> Result<()> {
        // 使用 rs-core 的内存数据库初始化
        #[cfg(feature = "test")]
        {
            aios_core::test_surreal_adapter::init_sul_db_with_memory().await?;
            println!("🧠 初始化内存数据库完成");
        }
        #[cfg(not(feature = "test"))]
        {
            println!("⚠️  内存数据库初始化需要启用 test feature");
        }
        Ok(())
    }

    /// 清理现有数据
    async fn cleanup_existing_data(&self) -> Result<()> {
        let sql = "DELETE FROM pe;";
        SUL_DB.query(sql).await?;
        println!("🧹 清理现有数据完成");
        Ok(())
    }

    /// 创建必要的表结构
    async fn create_tables(&self) -> Result<()> {
        // PE 表是主要的元素表
        let sql = r#"
            -- 确保表存在（SurrealDB 会自动创建）
            -- 定义一些必要的字段类型
        "#;
        SUL_DB.query(sql).await?;
        Ok(())
    }

    /// 加载单个元素到数据库
    async fn load_element(&self, element: &PdmsElement) -> Result<()> {
        let element_id = element.get_id();
        let noun = element.element_type.to_noun();
        
        // 构建属性数据
        let mut attributes = json!({
            "id": element_id.clone(),
            "noun": noun,
            "name": element.attributes.get("DESC").unwrap_or(&String::new()).clone(),
            "dbno": element.refno.0,
            "elno": element.refno.1,
        });

        // 添加所有属性
        for (key, value) in &element.attributes {
            // 跳过一些特殊处理的属性
            if key != "DESC" {
                attributes[key] = json!(value);
            }
        }

        // 添加位置信息
        if let Some(ref pos) = element.position {
            attributes["position"] = json!(pos);
        }

        // 插入到数据库
        let sql = format!(
            r#"
            INSERT INTO pe {{
                id: {},
                data: {}
            }};
            "#,
            element_id, attributes
        );

        SUL_DB.query(&sql).await?;
        
        println!("📦 加载元素: {} ({})", noun, element_id);
        Ok(())
    }

    /// 递归加载子元素
    async fn load_children(&self, parent: &PdmsElement) -> Result<()> {
        for child in &parent.children {
            self.load_element(child).await?;
            Box::pin(self.load_children(child)).await?;
        }
        Ok(())
    }

    /// 获取所有加载的 RefnoEnum
    pub async fn get_all_refnos(&self) -> Result<Vec<RefnoEnum>> {
        let sql = "SELECT VALUE id FROM pe;";
        let records: Vec<aios_core::RecordId> = SUL_DB.query_take(sql, 0).await?;
        
        let mut refnos = Vec::new();
        for record in records {
            // 使用调试格式，然后去掉 "RecordId " 前缀，只保留 key 部分
            let debug_str = format!("{:?}", record);
            let refno_str = debug_str.strip_prefix("RecordId ").unwrap_or(&debug_str);
            
            // 从 "pe:\"17496_1\"" 中提取 "17496_1"
            if let Some(start) = refno_str.find('"') {
                if let Some(end) = refno_str.rfind('"') {
                    let key_part = &refno_str[start+1..end];
                    if let Ok(refno_enum) = RefnoEnum::from_str(key_part) {
                        refnos.push(refno_enum);
                    }
                }
            }
        }
        
        Ok(refnos)
    }

    /// 按类型获取 RefnoEnum
    pub async fn get_refnos_by_noun(&self, noun: &str) -> Result<Vec<RefnoEnum>> {
        let sql = format!("SELECT VALUE id FROM pe WHERE noun = '{}';", noun);
        let records: Vec<aios_core::RecordId> = SUL_DB.query_take(&sql, 0).await?;
        
        let mut refnos = Vec::new();
        for record in records {
            // 使用调试格式，然后去掉 "RecordId " 前缀，只保留 key 部分
            let debug_str = format!("{:?}", record);
            let refno_str = debug_str.strip_prefix("RecordId ").unwrap_or(&debug_str);
            
            // 从 "pe:\"17496_1\"" 中提取 "17496_1"
            if let Some(start) = refno_str.find('"') {
                if let Some(end) = refno_str.rfind('"') {
                    let key_part = &refno_str[start+1..end];
                    if let Ok(refno_enum) = RefnoEnum::from_str(key_part) {
                        refnos.push(refno_enum);
                    }
                }
            }
        }
        
        Ok(refnos)
    }

    /// 打印加载统计信息
    pub fn print_statistics(&self) {
        let mut type_counts: HashMap<String, usize> = HashMap::new();
        
        // 递归统计所有元素
        fn count_elements(element: &crate::dblist_parser::parser::PdmsElement, counts: &mut HashMap<String, usize>) {
            let type_name = element.element_type.to_noun().to_string();
            *counts.entry(type_name).or_insert(0) += 1;
            
            for child in &element.children {
                count_elements(child, counts);
            }
        }
        
        for element in &self.elements {
            count_elements(element, &mut type_counts);
        }
        
        println!("\n📊 加载统计信息:");
        let total_elements: usize = type_counts.values().sum();
        for (type_name, count) in type_counts {
            println!("  {}: {} 个", type_name, count);
        }
        println!("  总计: {} 个元素\n", total_elements);
    }
}
