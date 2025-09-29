pub mod color_scheme;
pub mod examples;
pub mod xkt_entity;
pub mod xkt_geometry;
pub mod xkt_material;
pub mod xkt_model;
pub mod xkt_writer;
pub mod xkt_v10_writer;
pub mod xkt_v11_writer;
pub mod xkt_index;
pub mod xkt_proper_writer;
pub mod xkt_simple_writer;
pub mod xkt_spatial;

#[cfg(test)]
mod test_data_flow;

#[cfg(test)]
pub mod tests;

pub use color_scheme::*;
pub use xkt_entity::*;
pub use xkt_geometry::*;
pub use xkt_index::*;
pub use xkt_material::*;
pub use xkt_model::*;
pub use xkt_proper_writer::*;
pub use xkt_simple_writer::*;
pub use xkt_spatial::*;
pub use xkt_writer::*;
pub use xkt_v10_writer::*;
pub use xkt_v11_writer::*;

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

    /// 保存到文件 (使用正确的 XKT v10 格式)
    pub async fn save_to_file(&self, path: &str, compress: bool) -> Result<()> {
        let writer = XKTProperWriter::new();
        writer.write_to_file(self, path, compress).await
    }

    /// 保存为标准 XKT v10 文件 (严格按照xeokit规范)
    pub async fn save_to_file_v10(&self, path: &str) -> Result<()> {
        let writer = XKTv10Writer::new();
        writer.write_to_file(self, path).await
    }

    /// 保存为旧版 XKT 文件 (使用原格式)
    pub async fn save_to_file_legacy(&self, path: &str, compress: bool) -> Result<()> {
        let writer = XKTWriter::new();
        writer.write_to_file(self, path, compress).await
    }
}
