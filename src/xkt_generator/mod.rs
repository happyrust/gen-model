pub mod xkt_model;
pub mod xkt_geometry;
pub mod xkt_material;
pub mod xkt_entity;
pub mod xkt_writer;
pub mod color_scheme;
pub mod examples;

#[cfg(test)]
pub mod tests;

pub use xkt_model::*;
pub use xkt_geometry::*;
pub use xkt_material::*;
pub use xkt_entity::*;
pub use xkt_writer::*;
pub use color_scheme::*;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// XKT 文件版本
pub const XKT_VERSION: u32 = 10;

/// XKT 文件头部信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTHeader {
    pub version: u32,
    pub created_at: String,
    pub created_by: String,
    pub schema_version: String,
}

impl Default for XKTHeader {
    fn default() -> Self {
        Self {
            version: XKT_VERSION,
            created_at: chrono::Utc::now().to_rfc3339(),
            created_by: "aios-database".to_string(),
            schema_version: "1.0.0".to_string(),
        }
    }
}

/// XKT 文件的主要结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTFile {
    pub header: XKTHeader,
    pub model: XKTModel,
}

impl XKTFile {
    pub fn new() -> Self {
        Self {
            header: XKTHeader::default(),
            model: XKTModel::new(),
        }
    }

    /// 序列化为 XKT 格式的字节数组
    pub fn to_bytes(&self, compress: bool) -> Result<Vec<u8>> {
        let writer = XKTWriter::new();
        writer.write_to_bytes(self, compress)
    }

    /// 保存到文件
    pub async fn save_to_file(&self, path: &str, compress: bool) -> Result<()> {
        let writer = XKTWriter::new();
        writer.write_to_file(self, path, compress).await
    }
} 