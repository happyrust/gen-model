use anyhow::Result;
use nalgebra::Point3;
use parry3d::bounding_volume::Aabb;
use rstar::{AABB, RTree, RTreeObject};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::grpc_service::spatial_query_service::SpatialElement;
use aios_core::pdms_types::{PdmsGenericType, RefU64, RefnoEnum};

/// 空间索引构建器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialIndexConfig {
    /// 包围盒扩展容差
    pub bbox_tolerance: f32,
    /// 最大批量大小
    pub batch_size: usize,
    /// 是否包含负实体
    pub include_negative_entities: bool,
    /// 过滤的构件类型（空表示包含所有）
    pub filter_types: Vec<String>,
    /// 最小包围盒尺寸（避免过小构件影响性能）
    pub min_bbox_size: f32,
}

impl Default for SpatialIndexConfig {
    fn default() -> Self {
        Self {
            bbox_tolerance: 0.001,
            batch_size: 10000,
            include_negative_entities: false,
            filter_types: vec![],
            min_bbox_size: 0.0001,
        }
    }
}

/// 空间索引数据统计
#[derive(Debug, Clone)]
pub struct IndexStatistics {
    pub total_elements: usize,
    pub indexed_elements: usize,
    pub skipped_elements: usize,
    pub build_time_ms: u128,
    pub memory_estimate_mb: f64,
    pub type_distribution: HashMap<String, usize>,
}

/// 空间索引构建器
pub struct SpatialIndexBuilder {
    db_manager: Arc<AiosDBManager>,
    config: SpatialIndexConfig,
}

impl SpatialIndexBuilder {
    pub fn new(db_manager: Arc<AiosDBManager>) -> Self {
        Self {
            db_manager,
            config: SpatialIndexConfig::default(),
        }
    }

    pub fn with_config(mut self, config: SpatialIndexConfig) -> Self {
        self.config = config;
        self
    }

    /// 从数据库构建空间索引
    pub async fn build_from_database(
        &self,
        db_nos: &[i32],
    ) -> Result<(RTree<SpatialElement>, IndexStatistics)> {
        let start_time = SystemTime::now();
        let mut elements = Vec::new();
        let mut statistics = IndexStatistics {
            total_elements: 0,
            indexed_elements: 0,
            skipped_elements: 0,
            build_time_ms: 0,
            memory_estimate_mb: 0.0,
            type_distribution: HashMap::new(),
        };

        log::info!("开始从数据库构建空间索引，DB编号: {:?}", db_nos);

        // 获取目标参考号
        let root_refnos = self.db_manager.get_gen_model_root_refnos(db_nos).await?;
        log::info!("找到 {} 个根节点", root_refnos.len());

        // 批量处理构件
        for chunk in root_refnos.chunks(self.config.batch_size) {
            let batch_elements = self.build_elements_batch(chunk).await?;

            for element in batch_elements {
                statistics.total_elements += 1;

                // 应用过滤条件
                if self.should_include_element(&element) {
                    *statistics
                        .type_distribution
                        .entry(element.element_type.clone())
                        .or_insert(0) += 1;
                    elements.push(element);
                    statistics.indexed_elements += 1;
                } else {
                    statistics.skipped_elements += 1;
                }
            }

            log::info!(
                "已处理 {}/{} 个构件",
                statistics.total_elements,
                root_refnos.len()
            );
        }

        // 构建R-star树
        log::info!("构建R-star树索引，元素数量: {}", elements.len());
        let rtree = RTree::bulk_load(elements);

        statistics.build_time_ms = start_time.elapsed().unwrap().as_millis();
        statistics.memory_estimate_mb = self.estimate_memory_usage(&rtree);

        log::info!("空间索引构建完成: {:?}", statistics);
        Ok((rtree, statistics))
    }

    /// 从指定参考号列表构建索引
    pub async fn build_from_refnos(
        &self,
        refnos: &[RefU64],
    ) -> Result<(RTree<SpatialElement>, IndexStatistics)> {
        let start_time = SystemTime::now();
        let mut elements = Vec::new();
        let mut statistics = IndexStatistics {
            total_elements: refnos.len(),
            indexed_elements: 0,
            skipped_elements: 0,
            build_time_ms: 0,
            memory_estimate_mb: 0.0,
            type_distribution: HashMap::new(),
        };

        log::info!("从指定参考号构建空间索引，数量: {}", refnos.len());

        // 批量处理
        for chunk in refnos.chunks(self.config.batch_size) {
            let batch_elements = self.build_elements_batch(chunk).await?;

            for element in batch_elements {
                if self.should_include_element(&element) {
                    *statistics
                        .type_distribution
                        .entry(element.element_type.clone())
                        .or_insert(0) += 1;
                    elements.push(element);
                    statistics.indexed_elements += 1;
                } else {
                    statistics.skipped_elements += 1;
                }
            }
        }

        let rtree = RTree::bulk_load(elements);
        statistics.build_time_ms = start_time.elapsed().unwrap().as_millis();
        statistics.memory_estimate_mb = self.estimate_memory_usage(&rtree);

        Ok((rtree, statistics))
    }

    /// 批量构建空间元素
    async fn build_elements_batch(&self, refnos: &[RefU64]) -> Result<Vec<SpatialElement>> {
        let mut elements = Vec::new();

        // 这里需要从实际数据源获取构件信息
        // 目前先创建模拟数据，实际应用中需要替换为真实的数据查询
        for &refno in refnos {
            if let Some(element) = self.create_spatial_element(refno).await? {
                elements.push(element);
            }
        }

        Ok(elements)
    }

    /// 创建单个空间元素（需要根据实际数据源实现）
    async fn create_spatial_element(&self, refno: RefU64) -> Result<Option<SpatialElement>> {
        // TODO: 实现真实的数据查询逻辑
        // 这里需要从数据库查询构件的包围盒、类型等信息

        // 模拟数据创建过程
        let element_type = self.get_element_type(refno).await?;
        let bbox = self.get_element_bbox(refno).await?;
        let element_name = format!("Element_{}", refno.0);

        if let (Some(element_type), Some(bbox)) = (element_type, bbox) {
            Ok(Some(SpatialElement {
                refno,
                bbox,
                element_type,
                element_name,
                last_updated: SystemTime::now(),
            }))
        } else {
            Ok(None)
        }
    }

    /// 获取构件类型（需要实现）
    async fn get_element_type(&self, refno: RefU64) -> Result<Option<String>> {
        // TODO: 从数据库查询构件类型
        // 暂时返回模拟数据
        match refno.0 % 4 {
            0 => Ok(Some("PIPE".to_string())),
            1 => Ok(Some("EQUI".to_string())),
            2 => Ok(Some("STRU".to_string())),
            _ => Ok(Some("ROOM".to_string())),
        }
    }

    /// 获取构件包围盒（需要实现）
    async fn get_element_bbox(&self, refno: RefU64) -> Result<Option<Aabb>> {
        // TODO: 从数据库查询构件包围盒
        // 暂时返回模拟数据
        let base = (refno.0 as f32) * 0.1;
        Ok(Some(Aabb::new(
            Point3::new(base, base, base),
            Point3::new(base + 1.0, base + 1.0, base + 1.0),
        )))
    }

    /// 判断是否包含该元素
    fn should_include_element(&self, element: &SpatialElement) -> bool {
        // 类型过滤
        if !self.config.filter_types.is_empty()
            && !self.config.filter_types.contains(&element.element_type)
        {
            return false;
        }

        // 包围盒尺寸过滤
        let size = element.bbox.maxs - element.bbox.mins;
        if size.x < self.config.min_bbox_size
            || size.y < self.config.min_bbox_size
            || size.z < self.config.min_bbox_size
        {
            return false;
        }

        true
    }

    /// 估算内存使用量
    fn estimate_memory_usage(&self, rtree: &RTree<SpatialElement>) -> f64 {
        // 粗略估算每个元素占用的内存
        let element_size = std::mem::size_of::<SpatialElement>();
        let tree_overhead = rtree.size() * 64; // 树结构开销估算
        let total_bytes = rtree.size() * element_size + tree_overhead;
        total_bytes as f64 / (1024.0 * 1024.0) // 转换为MB
    }
}

/// 空间索引持久化管理器
pub struct SpatialIndexPersistence;

impl SpatialIndexPersistence {
    /// 保存索引到文件
    pub fn save_index(
        rtree: &RTree<SpatialElement>,
        statistics: &IndexStatistics,
        file_path: &Path,
    ) -> Result<()> {
        use std::fs::File;
        use std::io::BufWriter;

        let file = File::create(file_path)?;
        let writer = BufWriter::new(file);

        // 使用bincode进行序列化
        let data = IndexPersistenceData {
            elements: rtree.iter().cloned().collect(),
            statistics: statistics.clone(),
            version: 1,
            created_at: SystemTime::now(),
        };

        bincode::serialize_into(writer, &data)?;
        log::info!("空间索引已保存到文件: {:?}", file_path);
        Ok(())
    }

    /// 从文件加载索引
    pub fn load_index(file_path: &Path) -> Result<(RTree<SpatialElement>, IndexStatistics)> {
        use std::fs::File;
        use std::io::BufReader;

        let file = File::open(file_path)?;
        let reader = BufReader::new(file);

        let data: IndexPersistenceData = bincode::deserialize_from(reader)?;
        let rtree = RTree::bulk_load(data.elements);

        log::info!("空间索引已从文件加载: {:?}", file_path);
        Ok((rtree, data.statistics))
    }

    /// 检查索引文件是否存在且有效
    pub fn is_valid_index_file(file_path: &Path) -> bool {
        if !file_path.exists() {
            return false;
        }

        // 检查文件是否可读取
        match Self::load_index(file_path) {
            Ok(_) => true,
            Err(e) => {
                log::warn!("索引文件无效: {:?}, 错误: {}", file_path, e);
                false
            }
        }
    }
}

/// 持久化数据结构
#[derive(Serialize, Deserialize, Clone)]
struct IndexPersistenceData {
    elements: Vec<SpatialElement>,
    statistics: IndexStatistics,
    version: u32,
    created_at: SystemTime,
}

/// 增量索引更新器
pub struct IncrementalIndexUpdater {
    index: Arc<RwLock<RTree<SpatialElement>>>,
}

impl IncrementalIndexUpdater {
    pub fn new(index: Arc<RwLock<RTree<SpatialElement>>>) -> Self {
        Self { index }
    }

    /// 添加新元素
    pub async fn add_element(&self, element: SpatialElement) -> Result<()> {
        let mut index = self.index.write().await;
        // R-star树不支持直接插入，需要重建
        // 在实际应用中可能需要使用支持增量更新的数据结构
        log::info!("添加新元素到索引: {}", element.refno.0);
        // 这里需要实现增量更新逻辑
        Ok(())
    }

    /// 删除元素
    pub async fn remove_element(&self, refno: RefU64) -> Result<()> {
        let mut index = self.index.write().await;
        log::info!("从索引中删除元素: {}", refno.0);
        // 这里需要实现删除逻辑
        Ok(())
    }

    /// 更新元素
    pub async fn update_element(&self, element: SpatialElement) -> Result<()> {
        // 先删除后添加
        self.remove_element(element.refno).await?;
        self.add_element(element).await?;
        Ok(())
    }
}

// 实现序列化支持
impl Serialize for IndexStatistics {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("IndexStatistics", 6)?;
        state.serialize_field("total_elements", &self.total_elements)?;
        state.serialize_field("indexed_elements", &self.indexed_elements)?;
        state.serialize_field("skipped_elements", &self.skipped_elements)?;
        state.serialize_field("build_time_ms", &self.build_time_ms)?;
        state.serialize_field("memory_estimate_mb", &self.memory_estimate_mb)?;
        state.serialize_field("type_distribution", &self.type_distribution)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for IndexStatistics {
    fn deserialize<D>(deserializer: D) -> Result<IndexStatistics, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, Deserialize, Deserializer, MapAccess, Visitor};
        use std::fmt;

        struct IndexStatisticsVisitor;

        impl<'de> Visitor<'de> for IndexStatisticsVisitor {
            type Value = IndexStatistics;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct IndexStatistics")
            }

            fn visit_map<V>(self, mut map: V) -> Result<IndexStatistics, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut total_elements = None;
                let mut indexed_elements = None;
                let mut skipped_elements = None;
                let mut build_time_ms = None;
                let mut memory_estimate_mb = None;
                let mut type_distribution = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        "total_elements" => total_elements = Some(map.next_value()?),
                        "indexed_elements" => indexed_elements = Some(map.next_value()?),
                        "skipped_elements" => skipped_elements = Some(map.next_value()?),
                        "build_time_ms" => build_time_ms = Some(map.next_value()?),
                        "memory_estimate_mb" => memory_estimate_mb = Some(map.next_value()?),
                        "type_distribution" => type_distribution = Some(map.next_value()?),
                        _ => {
                            let _ = map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }

                Ok(IndexStatistics {
                    total_elements: total_elements.unwrap_or(0),
                    indexed_elements: indexed_elements.unwrap_or(0),
                    skipped_elements: skipped_elements.unwrap_or(0),
                    build_time_ms: build_time_ms.unwrap_or(0),
                    memory_estimate_mb: memory_estimate_mb.unwrap_or(0.0),
                    type_distribution: type_distribution.unwrap_or_default(),
                })
            }
        }

        deserializer.deserialize_struct(
            "IndexStatistics",
            &[
                "total_elements",
                "indexed_elements",
                "skipped_elements",
                "build_time_ms",
                "memory_estimate_mb",
                "type_distribution",
            ],
            IndexStatisticsVisitor,
        )
    }
}
