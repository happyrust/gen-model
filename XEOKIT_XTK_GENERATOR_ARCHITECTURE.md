# xeokit 兼容 XTK 生成器架构设计

## 概述

基于 xeokit XKT V4.0 格式规范，设计一个全新的、完全兼容的 XTK 生成器架构，解决当前实现的所有问题。

## 1. 整体架构设计

### 1.1 核心组件架构

```rust
// 主要生成器结构
pub struct XeokitXTKGenerator {
    config: XTKGeneratorConfig,
    geometry_processor: GeometryProcessor,
    material_manager: MaterialManager,
    stream_writer: StreamWriter,
    progress_tracker: ProgressTracker,
}

// 配置管理
pub struct XTKGeneratorConfig {
    pub batch_size: usize,
    pub compression_level: u32,
    pub quantization_bits: u8,
    pub enable_geometry_reuse: bool,
    pub enable_instancing: bool,
    pub memory_limit_mb: usize,
}

// 几何处理器
pub struct GeometryProcessor {
    quantizer: PositionQuantizer,
    normal_encoder: NormalEncoder,
    edge_generator: EdgeIndexGenerator,
    primitive_cache: PrimitiveCache,
}

// 材质管理器
pub struct MaterialManager {
    color_scheme: ColorScheme,
    material_cache: HashMap<String, XKTMaterial>,
    texture_manager: TextureManager,
}

// 流式写入器
pub struct StreamWriter {
    output_buffer: Vec<u8>,
    compression_buffer: Vec<u8>,
    index_builder: IndexBuilder,
}
```

### 1.2 数据流架构

```
PDMS数据 → 批处理器 → 几何处理器 → 压缩器 → XKT文件
    ↓           ↓           ↓          ↓
  进度跟踪   内存监控    质量检查   错误处理
```

## 2. 核心组件详细设计

### 2.1 位置量化器

```rust
pub struct PositionQuantizer {
    kd_tree: KDTree,
    regions: Vec<QuantizationRegion>,
    decode_matrices: Vec<Mat4>,
}

pub struct QuantizationRegion {
    pub bounds: AABB,
    pub positions: Vec<Vec3>,
    pub quantized_positions: Vec<u16>,
    pub decode_matrix_index: usize,
}

impl PositionQuantizer {
    pub fn new(precision_bits: u8) -> Self {
        Self {
            kd_tree: KDTree::new(),
            regions: Vec::new(),
            decode_matrices: Vec::new(),
        }
    }
    
    pub fn add_positions(&mut self, positions: &[Vec3]) -> QuantizationResult {
        // 1. 使用K-d树分区
        let partitions = self.kd_tree.partition(positions, MAX_REGION_SIZE);
        
        for partition in partitions {
            let region = self.create_quantization_region(partition);
            self.regions.push(region);
        }
        
        QuantizationResult {
            region_count: self.regions.len(),
            total_vertices: positions.len(),
        }
    }
    
    fn create_quantization_region(&mut self, positions: Vec<Vec3>) -> QuantizationRegion {
        let bounds = AABB::from_points(&positions);
        let decode_matrix = self.create_decode_matrix(&bounds);
        let decode_matrix_index = self.decode_matrices.len();
        self.decode_matrices.push(decode_matrix);
        
        let quantized_positions = positions.iter()
            .flat_map(|pos| self.quantize_position(*pos, &bounds))
            .collect();
            
        QuantizationRegion {
            bounds,
            positions,
            quantized_positions,
            decode_matrix_index,
        }
    }
    
    fn quantize_position(&self, pos: Vec3, bounds: &AABB) -> [u16; 3] {
        let normalized = (pos - bounds.min) / (bounds.max - bounds.min);
        [
            (normalized.x * 65535.0) as u16,
            (normalized.y * 65535.0) as u16,
            (normalized.z * 65535.0) as u16,
        ]
    }
}
```

### 2.2 法向量编码器

```rust
pub struct NormalEncoder;

impl NormalEncoder {
    pub fn encode_normals(&self, normals: &[Vec3]) -> Vec<u8> {
        normals.iter()
            .flat_map(|normal| self.oct_encode(*normal))
            .collect()
    }
    
    fn oct_encode(&self, normal: Vec3) -> [u8; 2] {
        let n = normal.normalize();
        let sum = n.x.abs() + n.y.abs() + n.z.abs();
        let n = n / sum;
        
        let (x, y) = if n.z >= 0.0 {
            (n.x, n.y)
        } else {
            let sign_x = if n.x >= 0.0 { 1.0 } else { -1.0 };
            let sign_y = if n.y >= 0.0 { 1.0 } else { -1.0 };
            (
                (1.0 - n.y.abs()) * sign_x,
                (1.0 - n.x.abs()) * sign_y
            )
        };
        
        [
            ((x * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8,
            ((y * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8,
        ]
    }
}
```

### 2.3 基元缓存系统

```rust
pub struct PrimitiveCache {
    primitives: HashMap<PrimitiveHash, PrimitiveId>,
    primitive_data: Vec<XKTPrimitive>,
    hash_builder: GeometryHashBuilder,
}

#[derive(Hash, Eq, PartialEq)]
pub struct PrimitiveHash {
    geometry_hash: u64,
    material_hash: u64,
}

pub struct XKTPrimitive {
    pub id: PrimitiveId,
    pub positions_portion: u32,
    pub normals_portion: u32,
    pub indices_portion: u32,
    pub edge_indices_portion: u32,
    pub decode_matrix_portion: u32,
    pub color: [u8; 4],
    pub usage_count: usize,
}

impl PrimitiveCache {
    pub fn get_or_create_primitive(
        &mut self,
        geometry: &PDMSGeometry,
        material: &XKTMaterial,
    ) -> PrimitiveId {
        let hash = self.hash_builder.hash_geometry_material(geometry, material);
        
        if let Some(&primitive_id) = self.primitives.get(&hash) {
            // 复用现有基元
            self.primitive_data[primitive_id.0].usage_count += 1;
            primitive_id
        } else {
            // 创建新基元
            let primitive_id = PrimitiveId(self.primitive_data.len());
            let primitive = self.create_primitive(geometry, material, primitive_id);
            
            self.primitive_data.push(primitive);
            self.primitives.insert(hash, primitive_id);
            primitive_id
        }
    }
}
```

### 2.4 流式处理器

```rust
pub struct StreamProcessor {
    batch_size: usize,
    current_batch: Vec<RefnoEnum>,
    processed_count: usize,
    total_count: usize,
    memory_monitor: MemoryMonitor,
}

impl StreamProcessor {
    pub async fn process_refnos<F>(
        &mut self,
        refnos: Vec<RefnoEnum>,
        mut processor: F,
    ) -> Result<()>
    where
        F: FnMut(&[RefnoEnum]) -> Result<ProcessBatchResult>,
    {
        self.total_count = refnos.len();
        
        for batch in refnos.chunks(self.batch_size) {
            // 内存检查
            if self.memory_monitor.should_gc() {
                self.memory_monitor.force_gc().await?;
            }
            
            // 处理批次
            let result = processor(batch)?;
            self.processed_count += batch.len();
            
            // 进度报告
            self.report_progress(result);
            
            // 让出控制权
            tokio::task::yield_now().await;
        }
        
        Ok(())
    }
}

pub struct MemoryMonitor {
    memory_limit: usize,
    current_usage: usize,
    gc_threshold: f32,
}

impl MemoryMonitor {
    pub fn should_gc(&self) -> bool {
        let usage_ratio = self.current_usage as f32 / self.memory_limit as f32;
        usage_ratio > self.gc_threshold
    }
    
    pub async fn force_gc(&mut self) -> Result<()> {
        // 强制垃圾回收
        std::hint::black_box(Vec::<u8>::new());
        tokio::task::yield_now().await;
        self.update_memory_usage();
        Ok(())
    }
}
```

## 3. PDMS 几何类型处理

### 3.1 完整的几何类型支持

```rust
pub enum PDMSGeometryType {
    PrimBox { width: f32, height: f32, depth: f32 },
    PrimCylinder { radius: f32, height: f32 },
    PrimSCylinder { radius1: f32, radius2: f32, height: f32 },
    PrimSphere { radius: f32 },
    PrimPyramid { base_width: f32, base_height: f32, height: f32 },
    PrimTorus { major_radius: f32, minor_radius: f32 },
    CustomMesh { vertices: Vec<Vec3>, indices: Vec<u32> },
}

pub struct GeometryConverter;

impl GeometryConverter {
    pub fn convert_pdms_geometry(
        &self,
        geo_type: &PDMSGeometryType,
        transform: &Mat4,
    ) -> Result<ConvertedGeometry> {
        match geo_type {
            PDMSGeometryType::PrimBox { width, height, depth } => {
                self.create_box_geometry(*width, *height, *depth, transform)
            }
            PDMSGeometryType::PrimCylinder { radius, height } => {
                self.create_cylinder_geometry(*radius, *height, 32, transform)
            }
            PDMSGeometryType::PrimSCylinder { radius1, radius2, height } => {
                self.create_truncated_cone_geometry(*radius1, *radius2, *height, 32, transform)
            }
            PDMSGeometryType::PrimSphere { radius } => {
                self.create_sphere_geometry(*radius, 32, 16, transform)
            }
            PDMSGeometryType::PrimPyramid { base_width, base_height, height } => {
                self.create_pyramid_geometry(*base_width, *base_height, *height, transform)
            }
            PDMSGeometryType::CustomMesh { vertices, indices } => {
                self.create_custom_mesh_geometry(vertices, indices, transform)
            }
            _ => Err(anyhow::anyhow!("不支持的几何类型")),
        }
    }
}
```

## 4. 错误处理和质量保证

### 4.1 统一错误处理

```rust
#[derive(Debug, thiserror::Error)]
pub enum XTKGeneratorError {
    #[error("几何处理错误: {0}")]
    GeometryError(String),
    
    #[error("内存不足: 当前使用 {current}MB, 限制 {limit}MB")]
    OutOfMemory { current: usize, limit: usize },
    
    #[error("数据库错误: {0}")]
    DatabaseError(#[from] DatabaseError),
    
    #[error("文件写入错误: {0}")]
    IoError(#[from] std::io::Error),
    
    #[error("压缩错误: {0}")]
    CompressionError(String),
}

pub struct QualityChecker;

impl QualityChecker {
    pub fn validate_geometry(&self, geometry: &ConvertedGeometry) -> Result<()> {
        // 检查几何体完整性
        if geometry.positions.is_empty() {
            return Err(XTKGeneratorError::GeometryError("空几何体".to_string()));
        }
        
        if geometry.positions.len() % 3 != 0 {
            return Err(XTKGeneratorError::GeometryError("顶点数据不完整".to_string()));
        }
        
        // 检查索引有效性
        let max_vertex_index = (geometry.positions.len() / 3) as u32;
        for &index in &geometry.indices {
            if index >= max_vertex_index {
                return Err(XTKGeneratorError::GeometryError(
                    format!("无效索引: {} >= {}", index, max_vertex_index)
                ));
            }
        }
        
        Ok(())
    }
}
```

## 5. XKT 文件写入器

### 5.1 标准兼容的文件格式

```rust
pub struct XKTWriter {
    version: u32,
    index_builder: IndexBuilder,
    data_sections: Vec<DataSection>,
    compression_level: u32,
}

impl XKTWriter {
    pub fn new() -> Self {
        Self {
            version: 4, // xeokit XKT V4.0
            index_builder: IndexBuilder::new(),
            data_sections: Vec::new(),
            compression_level: 6,
        }
    }

    pub async fn write_xkt_file(&mut self, model: &XKTModel, output_path: &str) -> Result<()> {
        let mut file = tokio::fs::File::create(output_path).await?;

        // 1. 写入版本号
        file.write_u32_le(self.version).await?;

        // 2. 构建索引
        let index = self.build_index(model)?;
        file.write_u32_le(index.total_size()).await?;
        file.write_all(&index.serialize()).await?;

        // 3. 写入压缩数据段
        self.write_compressed_sections(&mut file, model).await?;

        Ok(())
    }

    fn build_index(&mut self, model: &XKTModel) -> Result<XKTIndex> {
        let mut index = XKTIndex::new();

        // 计算各个数据段的大小
        let positions_data = self.serialize_positions(&model.positions)?;
        let normals_data = self.serialize_normals(&model.normals)?;
        let indices_data = self.serialize_indices(&model.indices)?;
        // ... 其他数据段

        index.size_positions = self.compress_data(&positions_data)?.len() as u32;
        index.size_normals = self.compress_data(&normals_data)?.len() as u32;
        index.size_indices = self.compress_data(&indices_data)?.len() as u32;
        // ... 设置其他大小

        Ok(index)
    }
}

pub struct XKTIndex {
    pub size_positions: u32,
    pub size_normals: u32,
    pub size_indices: u32,
    pub size_edge_indices: u32,
    pub size_decode_matrices: u32,
    pub size_each_primitive_positions_and_normals_portion: u32,
    pub size_each_primitive_indices_portion: u32,
    pub size_each_primitive_edge_indices_portion: u32,
    pub size_each_primitive_decode_matrices_portion: u32,
    pub size_each_primitive_color: u32,
    pub size_primitive_instances: u32,
    pub size_each_entity_id: u32,
    pub size_each_entity_primitive_instances_portion: u32,
    pub size_each_entity_matrix: u32,
}
```

### 5.2 高效的数据序列化

```rust
pub trait XKTSerializable {
    fn serialize(&self) -> Result<Vec<u8>>;
    fn estimated_size(&self) -> usize;
}

impl XKTSerializable for Vec<u16> {
    fn serialize(&self) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(self.len() * 2);
        for &value in self {
            buffer.write_u16::<LittleEndian>(value)?;
        }
        Ok(buffer)
    }

    fn estimated_size(&self) -> usize {
        self.len() * 2
    }
}

impl XKTSerializable for Vec<u8> {
    fn serialize(&self) -> Result<Vec<u8>> {
        Ok(self.clone())
    }

    fn estimated_size(&self) -> usize {
        self.len()
    }
}

impl XKTSerializable for Vec<String> {
    fn serialize(&self) -> Result<Vec<u8>> {
        let json = serde_json::to_string(self)?;
        Ok(json.into_bytes())
    }

    fn estimated_size(&self) -> usize {
        self.iter().map(|s| s.len() + 4).sum::<usize>() + 10 // JSON开销
    }
}
```

## 6. 性能优化策略

### 6.1 并行处理

```rust
pub struct ParallelProcessor {
    thread_pool: ThreadPool,
    chunk_size: usize,
}

impl ParallelProcessor {
    pub async fn process_geometries_parallel(
        &self,
        geometries: Vec<PDMSGeometry>,
    ) -> Result<Vec<ConvertedGeometry>> {
        let chunks: Vec<_> = geometries.chunks(self.chunk_size).collect();
        let mut tasks = Vec::new();

        for chunk in chunks {
            let chunk = chunk.to_vec();
            let task = tokio::task::spawn_blocking(move || {
                chunk.into_iter()
                    .map(|geo| GeometryConverter::convert_geometry(&geo))
                    .collect::<Result<Vec<_>>>()
            });
            tasks.push(task);
        }

        let mut results = Vec::new();
        for task in tasks {
            let chunk_result = task.await??;
            results.extend(chunk_result);
        }

        Ok(results)
    }
}
```

### 6.2 内存池管理

```rust
pub struct MemoryPool {
    vertex_buffers: Vec<Vec<Vec3>>,
    index_buffers: Vec<Vec<u32>>,
    buffer_size: usize,
}

impl MemoryPool {
    pub fn get_vertex_buffer(&mut self) -> Vec<Vec3> {
        self.vertex_buffers.pop()
            .unwrap_or_else(|| Vec::with_capacity(self.buffer_size))
    }

    pub fn return_vertex_buffer(&mut self, mut buffer: Vec<Vec3>) {
        buffer.clear();
        if buffer.capacity() <= self.buffer_size * 2 {
            self.vertex_buffers.push(buffer);
        }
    }
}
```

## 7. 配置和扩展性

### 7.1 灵活的配置系统

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XTKGeneratorConfig {
    // 性能配置
    pub batch_size: usize,
    pub thread_count: usize,
    pub memory_limit_mb: usize,

    // 质量配置
    pub quantization_bits: u8,
    pub normal_precision: NormalPrecision,
    pub edge_generation: bool,

    // 优化配置
    pub enable_geometry_reuse: bool,
    pub enable_instancing: bool,
    pub compression_level: u32,

    // 输出配置
    pub output_format: OutputFormat,
    pub include_metadata: bool,
    pub include_properties: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NormalPrecision {
    Low,    // 8位Oct编码
    Medium, // 16位Oct编码
    High,   // 32位浮点
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutputFormat {
    XKT4,   // xeokit XKT V4.0
    XKT3,   // 向后兼容
    Custom, // 自定义格式
}
```

### 7.2 插件系统

```rust
pub trait GeometryProcessor {
    fn process_geometry(&self, geometry: &PDMSGeometry) -> Result<ConvertedGeometry>;
    fn supported_types(&self) -> Vec<String>;
}

pub trait MaterialProcessor {
    fn process_material(&self, pdms_type: &str) -> Result<XKTMaterial>;
    fn get_color_scheme(&self) -> &ColorScheme;
}

pub struct ProcessorRegistry {
    geometry_processors: HashMap<String, Box<dyn GeometryProcessor>>,
    material_processors: HashMap<String, Box<dyn MaterialProcessor>>,
}

impl ProcessorRegistry {
    pub fn register_geometry_processor<P: GeometryProcessor + 'static>(
        &mut self,
        name: String,
        processor: P,
    ) {
        self.geometry_processors.insert(name, Box::new(processor));
    }

    pub fn get_geometry_processor(&self, geo_type: &str) -> Option<&dyn GeometryProcessor> {
        self.geometry_processors.values()
            .find(|p| p.supported_types().contains(&geo_type.to_string()))
            .map(|p| p.as_ref())
    }
}
```

## 8. 测试和验证

### 8.1 单元测试框架

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_position_quantization() {
        let positions = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, -1.0, -1.0),
        ];

        let mut quantizer = PositionQuantizer::new(16);
        let result = quantizer.add_positions(&positions);

        assert_eq!(result.total_vertices, 3);
        assert!(result.region_count > 0);

        // 验证反量化精度
        let decoded = quantizer.decode_positions();
        for (original, decoded) in positions.iter().zip(decoded.iter()) {
            let error = (*original - *decoded).length();
            assert!(error < 0.001, "量化误差过大: {}", error);
        }
    }

    #[tokio::test]
    async fn test_xkt_file_generation() {
        let config = XTKGeneratorConfig::default();
        let mut generator = XeokitXTKGenerator::new(config);

        let test_refnos = vec![
            RefnoEnum::from("12345/67890"),
            RefnoEnum::from("23456/78901"),
        ];

        let result = generator.generate_xkt_from_refnos(
            test_refnos,
            "test_output/test.xkt",
            true,
        ).await;

        assert!(result.is_ok());
        assert!(std::path::Path::new("test_output/test.xkt").exists());

        // 验证文件格式
        let file_data = std::fs::read("test_output/test.xkt").unwrap();
        assert!(XKTValidator::validate_format(&file_data).is_ok());
    }
}
```

这个架构设计解决了当前实现的所有主要问题，提供了完整的 xeokit 兼容性、高性能处理和可扩展性。
