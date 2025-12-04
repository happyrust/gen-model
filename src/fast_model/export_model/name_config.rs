//! 名称配置模块
//!
//! 从 Excel 文件读取名称映射配置，用于导出时将三维模型节点名称转换为 PID 对象名称。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use calamine::{DataType, Reader, Xlsx, open_workbook};

/// 名称映射配置
///
/// 存储从 Excel 读取的 "三维模型节点" -> "PID对象" 映射关系
#[derive(Debug, Clone, Default)]
pub struct NameConfig {
    /// 名称映射表：三维模型节点 -> PID对象
    name_map: HashMap<String, String>,
}

impl NameConfig {
    /// 从 Excel 文件加载名称配置
    ///
    /// # 参数
    /// - `path`: Excel 文件路径
    ///
    /// # Excel 格式要求
    /// - D 列（索引 3）: PID对象
    /// - I 列（索引 8）: 三维模型节点
    /// - 第一行为表头，从第二行开始读取数据
    pub fn load_from_excel<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        println!("📖 加载名称配置文件: {}", path.display());

        let mut workbook: Xlsx<_> = open_workbook(path)
            .with_context(|| format!("无法打开 Excel 文件: {}", path.display()))?;

        let sheet_name = workbook
            .sheet_names()
            .first()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Excel 文件没有工作表"))?;

        let range = workbook
            .worksheet_range(&sheet_name)
            .with_context(|| format!("无法读取工作表: {}", sheet_name))?;

        let mut name_map = HashMap::new();
        let mut row_count = 0;
        let mut valid_count = 0;

        // 跳过第一行（表头），从第二行开始
        for row in range.rows().skip(1) {
            row_count += 1;

            // D 列（索引 3）: PID对象
            let pid_object: String = row
                .get(3)
                .and_then(|cell| cell.as_string())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            // I 列（索引 8）: 三维模型节点
            // 去掉开头的斜线，保持与 sanitize_node_name 一致的处理
            let model_node: String = row
                .get(8)
                .and_then(|cell| cell.as_string())
                .map(|s| s.trim().trim_start_matches('/').to_string())
                .unwrap_or_default();

            // 只有当两列都有值时才添加映射
            if !model_node.is_empty() && !pid_object.is_empty() {
                name_map.insert(model_node, pid_object);
                valid_count += 1;
            }
        }

        println!(
            "   ✅ 读取 {} 行数据，有效映射 {} 条",
            row_count, valid_count
        );

        // 调试：打印前 5 条映射
        println!("   📋 前 5 条映射（调试）:");
        for (i, (model, pid)) in name_map.iter().take(5).enumerate() {
            println!("      {}: {:?} -> {:?}", i + 1, model, pid);
        }
        
        // 调试：打印包含"石楼"或"BRAN"或"SITE"的映射
        println!("   📋 包含'石楼/BRAN/SITE/PIPE'的映射:");
        for (model, pid) in name_map.iter()
            .filter(|(k, _)| k.contains("石楼") || k.contains("BRAN") || k.contains("SITE") || k.contains("PIPE"))
            .take(10) 
        {
            println!("      {:?} -> {:?}", model, pid);
        }

        Ok(Self { name_map })
    }

    /// 转换名称
    ///
    /// 如果映射表中存在对应的 PID 对象名称，则返回转换后的名称；
    /// 否则返回原始名称。
    pub fn convert_name(&self, model_name: &str) -> String {
        self.name_map
            .get(model_name)
            .cloned()
            .unwrap_or_else(|| model_name.to_string())
    }

    /// 检查是否有映射
    pub fn has_mapping(&self, model_name: &str) -> bool {
        self.name_map.contains_key(model_name)
    }

    /// 获取映射数量
    pub fn len(&self) -> usize {
        self.name_map.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.name_map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_name() {
        let mut config = NameConfig::default();
        config
            .name_map
            .insert("MODEL-001".to_string(), "PID-001".to_string());

        assert_eq!(config.convert_name("MODEL-001"), "PID-001");
        assert_eq!(config.convert_name("UNKNOWN"), "UNKNOWN");
    }
}
