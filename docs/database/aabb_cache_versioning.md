# AABB 缓存版本化功能

## 概述

AABB 缓存现在支持版本化存储，允许您存储和检索不同 session 的几何边界框数据。这对于历史数据查询、版本比较和时间旅行调试非常有用。

## 新增功能

### 1. 版本化存储

```rust
// 存储指定 session 的 AABB 数据
pub fn put_ref_bbox_versioned(&self, bbox: &RStarBoundingBox, session: u32) -> anyhow::Result<()>
```

### 2. 按 Session 查询

```rust
// 获取指定 session 的 AABB 数据
pub fn get_ref_bbox_at_session(&self, refno: RefU64, session: u32) -> Option<RStarBoundingBox>
```

### 3. 历史记录查询

```rust
// 获取指定 refno 的所有历史版本
pub fn get_ref_bbox_history(&self, refno: RefU64) -> Vec<(u32, RStarBoundingBox)>
```

### 4. 版本清理

```rust
// 清理指定时间之前的旧版本
pub fn cleanup_old_versions(&self, before_timestamp: u64) -> anyhow::Result<usize>
```

## 使用示例

```rust
use aios_database::fast_model::aabb_cache::AabbCache;
use aios_core::types::RefU64;
use parry3d::bounding_volume::Aabb;
use glam::Vec3;

// 打开缓存
let cache = AabbCache::open_with_path("cache.db")?;
let refno = RefU64(123456);

// 创建不同版本的几何数据
let aabb_v1 = Aabb::new(Vec3::ZERO.into(), Vec3::new(10.0, 10.0, 10.0).into());
let aabb_v2 = Aabb::new(Vec3::new(1.0, 1.0, 1.0).into(), Vec3::new(11.0, 11.0, 11.0).into());

let bbox_v1 = RStarBoundingBox::new(aabb_v1, refno.into(), "PIPE".to_string());
let bbox_v2 = RStarBoundingBox::new(aabb_v2, refno.into(), "PIPE".to_string());

// 存储不同版本
cache.put_ref_bbox_versioned(&bbox_v1, 100)?; // session 100
cache.put_ref_bbox_versioned(&bbox_v2, 200)?; // session 200

// 查询特定版本
let bbox_at_100 = cache.get_ref_bbox_at_session(refno, 100).unwrap();
let bbox_at_200 = cache.get_ref_bbox_at_session(refno, 200).unwrap();

// 获取完整历史
let history = cache.get_ref_bbox_history(refno);
println!("Found {} versions", history.len());

for (session, bbox) in history {
    println!("Session {}: {:?}", session, bbox.aabb);
}

// 清理旧版本（保留最近30天的数据）
let thirty_days_ago = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)?
    .as_secs() - (30 * 24 * 60 * 60);
    
let cleaned = cache.cleanup_old_versions(thirty_days_ago)?;
println!("Cleaned {} old versions", cleaned);
```

## 数据结构

版本化数据使用以下结构存储：

```rust
struct VersionedStoredAabb {
    refno_value: u64,      // 参考号
    session: u32,          // Session ID
    mins: [f32; 3],        // 最小边界点
    maxs: [f32; 3],        // 最大边界点
    created_at: u64,       // 创建时间戳
    updated_at: u64,       // 更新时间戳
}
```

## 性能考虑

- 版本化数据存储在单独的表中，不影响现有的非版本化查询性能
- 历史查询按 session 排序返回
- 建议定期清理旧版本以控制数据库大小
- 使用 bincode 序列化确保高效的存储和检索

## 兼容性

- 新功能完全向后兼容
- 现有的非版本化 API 继续正常工作
- 可以同时使用版本化和非版本化存储

## 时间数据功能

### 新增时间数据结构

```rust
// RefnoEnum 时间数据
pub struct RefnoTimeData {
    pub refno_value: u64,
    pub session: u32,
    pub dbnum: u32,
    pub created_at: u64,        // 创建时间戳
    pub updated_at: u64,        // 更新时间戳
    pub sesno_timestamp: u64,   // sesno 对应的实际时间
    pub author: Option<String>, // 创建者
    pub description: Option<String>, // 变更描述
}

// Sesno 时间映射
pub struct SesnoTimeMapping {
    pub dbnum: u32,
    pub sesno: u32,
    pub timestamp: u64,
    pub description: Option<String>,
}
```

### 时间数据 API

```rust
// 存储 sesno 时间映射
pub fn put_sesno_time_mapping(&self, mapping: &SesnoTimeMapping) -> anyhow::Result<()>

// 根据 dbnum 和 sesno 获取时间戳
pub fn get_timestamp_by_sesno(&self, dbnum: u32, sesno: u32) -> Option<u64>

// 存储 refno 时间数据
pub fn put_refno_time_data(&self, time_data: &RefnoTimeData) -> anyhow::Result<()>

// 获取指定 refno 和 session 的时间数据
pub fn get_refno_time_data(&self, refno: RefU64, session: u32) -> Option<RefnoTimeData>

// 获取指定 refno 的所有时间历史记录
pub fn get_refno_time_history(&self, refno: RefU64) -> Vec<RefnoTimeData>

// 按时间范围查询 refno
pub fn query_refnos_by_time_range(&self, start_time: u64, end_time: u64) -> Vec<RefU64>

// 从 PDMS 文件初始化时间数据
pub fn initialize_time_data_from_pdms(&self, db_path: PathBuf, dbnum: u32) -> anyhow::Result<usize>
```

### PDMS 时间数据提取器

```rust
pub struct PdmsTimeExtractor {
    pdms_io: PdmsIO,
    dbnum: u32,
}

impl PdmsTimeExtractor {
    pub fn new(db_path: PathBuf, dbnum: u32) -> anyhow::Result<Self>
    pub fn extract_sesno_time_mappings(&mut self) -> anyhow::Result<Vec<SesnoTimeMapping>>
    pub fn get_latest_sesno(&mut self) -> u32
}
```

## 演示程序

运行演示程序查看完整功能：

```bash
cargo run --example pdms_time_cache_demo
```

演示内容包括：
- 数据库 1112 的最新 refno 时间信息
- 某个 refno 的完整历史记录
- 按时间范围查询 refno

## 测试

运行以下命令测试所有功能：

```bash
# 测试版本化功能
cargo test aabb_cache

# 测试时间数据功能
cargo test test_time

# 运行演示测试
cargo test test_pdms_time_data_demo -- --nocapture
```

所有测试包括：
- 基本版本化存储和检索
- 多版本历史查询
- 版本清理功能
- 时间数据存储和查询
- 时间范围查询
- PDMS 数据集成
- 兼容性测试
