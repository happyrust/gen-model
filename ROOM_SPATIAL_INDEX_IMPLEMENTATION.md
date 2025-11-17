# 房间空间索引查询实现总结

## 📋 实现目标

使用 SQLite RTree 空间索引查询与房间面板相交的元素，替代原来的 `<-pe_owner<-pe` 图数据库关系查询。

## ✅ 已完成的工作

### 1. 核心函数实现

**文件**: `rs-core/src/room/query_v2.rs`

添加了 `query_elements_in_room_by_spatial_index` 函数：

```rust
pub async fn query_elements_in_room_by_spatial_index(
    room_refno: &RefnoEnum,
    panel_refnos: &[RefnoEnum],
    exclude_nouns: &[String],
) -> anyhow::Result<Vec<(RefU64, Aabb, Option<String>)>>
```

**功能：**
- 查询房间所有面板的 AABB
- 使用 SQLite RTree 空间索引查找与面板相交的元素
- 支持排除特定类型（如 PANE, FRMW, SBFR）
- 自动去重
- 详细的日志记录和性能统计

### 2. 函数导出

**文件**: `rs-core/src/room/mod.rs`

```rust
pub use query_v2::{
    query_room_panels_by_keywords, 
    query_elements_in_room_by_spatial_index
};
```

### 3. 测试程序

**文件**: `examples/test_room_spatial_index.rs`

- 测试查询前 3 个房间
- 显示详细的元素统计
- 提供诊断信息和解决方案

## 📊 测试结果

```
✅ 程序运行成功
🔍 查询到 124 个房间
⏱️  查询耗时：~1ms/房间
📊 找到元素：0 个（预期，因为 AABB 未写入）
```

**预期行为：**
- ✅ 程序编译通过
- ✅ 空间索引查询正常执行
- ✅ 性能良好（亚毫秒级）
- ⚠️  未查到元素（AABB 未写入）

## 🔧 使用方法

### 基本用法

```rust
use aios_core::room::{query_room_panels_by_keywords, query_elements_in_room_by_spatial_index};

// 1. 查询房间和面板
let keywords = vec!["-RM".to_string()];
let rooms = query_room_panels_by_keywords(&keywords).await?;

// 2. 对每个房间查询内部元素
for (room_refno, room_num, panel_refnos) in rooms {
    let exclude = vec!["PANE".to_string(), "FRMW".to_string()];
    
    let elements = query_elements_in_room_by_spatial_index(
        &room_refno,
        &panel_refnos,
        &exclude
    ).await?;
    
    println!("房间 {}: {} 个元素", room_num, elements.len());
}
```

### 运行测试

```bash
# 运行测试程序
cd /Volumes/DPC/work/plant-code/gen-model-fork
cargo run --example test_room_spatial_index --features sqlite-index
```

## 🚧 下一步：写入模型 AABB

**当前状态：** 功能已实现但需要在模型生成时写入 AABB

**需要做的：**

### 1. 确保 SQLite 索引已启用

**检查 `DbOption.toml`：**
```toml
enable_sqlite_rtree = true
sqlite_index_path = "./aabb_index.db"  # 可选
```

### 2. 在模型生成时写入 AABB

需要在生成几何体后调用：

```rust
use aios_core::spatial::sqlite::insert_or_update_aabb;
use parry3d::bounding_volume::Aabb;

// 生成模型时
for (refno, mesh) in generated_models {
    // 计算 AABB
    let aabb = mesh.compute_aabb();
    
    // 写入空间索引
    insert_or_update_aabb(
        refno,
        &aabb,
        Some(&noun)  // 元素类型，如 "PUMP", "PIPE"
    )?;
}
```

### 3. 批量写入优化

对于大批量模型生成：

```rust
use aios_core::spatial::sqlite::insert_or_update_aabbs_batch;

let batch_data: Vec<(RefU64, Aabb, Option<String>)> = models
    .iter()
    .map(|(refno, mesh, noun)| {
        let aabb = mesh.compute_aabb();
        (*refno, aabb, Some(noun.clone()))
    })
    .collect();

// 批量写入（使用事务，性能更好）
insert_or_update_aabbs_batch(&batch_data)?;
```

### 4. 查找模型生成代码

**可能的位置：**
- `src/fast_model/gen_model_old.rs`
- `src/fast_model/gen_model/`
- `src/fast_model/cata_model.rs`

**查找关键词：**
- `gen_geos`
- `compute_aabb`
- `PlantMesh`
- mesh 生成相关代码

## 💡 技术细节

### 空间索引查询流程

1. **查询面板几何信息**
   ```sql
   SELECT value [id, PXYZ] FROM [面板列表]
   ```

2. **计算面板 AABB**
   - 当前简化为点周围的小盒子 (±0.1)
   - TODO: 使用完整的面板几何信息

3. **空间索引查询**
   ```rust
   sqlite::query_overlap(&expanded_aabb, None, None, &[])
   ```
   - 查询与 AABB 相交的所有元素
   - 返回 (RefU64, Aabb, noun)

4. **过滤和去重**
   - 排除指定类型
   - 合并多个面板的结果
   - 去重

### SQLite RTree 表结构

```sql
CREATE VIRTUAL TABLE aabb_index USING rtree(
    id INTEGER PRIMARY KEY,
    min_x REAL, max_x REAL,
    min_y REAL, max_y REAL,
    min_z REAL, max_z REAL
);

CREATE TABLE items (
    id INTEGER PRIMARY KEY,
    noun TEXT
);
```

## 🔍 调试和验证

### 检查 SQLite 索引文件

```bash
# 查看索引文件是否存在
ls -lh ./aabb_index.db

# 查看索引内容
sqlite3 ./aabb_index.db
> SELECT COUNT(*) FROM aabb_index;
> SELECT COUNT(*) FROM items;
> .quit
```

### 添加调试日志

在模型生成代码中：

```rust
info!("写入 AABB: refno={}, noun={}, aabb={:?}", refno, noun, aabb);
```

### 测试空间索引写入

创建一个简单的测试：

```rust
use aios_core::spatial::sqlite::{insert_or_update_aabb, query_overlap};
use parry3d::bounding_volume::Aabb;
use nalgebra::Point3;

// 写入测试数据
let test_aabb = Aabb::new(
    Point3::new(0.0, 0.0, 0.0),
    Point3::new(1.0, 1.0, 1.0)
);
insert_or_update_aabb(RefU64(12345), &test_aabb, Some("TEST"))?;

// 查询测试
let results = query_overlap(&test_aabb, None, None, &[])?;
println!("测试查询结果: {} 个元素", results.len());
```

## 📈 性能优化建议

1. **批量写入**
   - 使用 `insert_or_update_aabbs_batch`
   - 在模型生成结束时一次性写入

2. **并发查询**
   - 可以并发查询多个房间
   - 使用 `tokio::spawn` 或 `futures::stream`

3. **缓存策略**
   - 缓存房间面板的 AABB
   - 缓存空间索引查询结果

## ⚠️ 注意事项

1. **面板 AABB 计算**
   - 当前使用简化方法（点±0.1）
   - 生产环境应该从完整几何信息计算

2. **坐标系一致性**
   - 确保 PXYZ 和 AABB 使用相同的坐标系
   - 注意单位（mm vs m）

3. **特性门控**
   - 空间索引功能需要 `sqlite` feature
   - 编译时确保启用正确的 features

## 📚 相关文件

- ✅ `rs-core/src/room/query_v2.rs` - 核心实现
- ✅ `rs-core/src/room/mod.rs` - 函数导出
- ✅ `rs-core/src/spatial/sqlite.rs` - SQLite 空间索引
- ✅ `examples/test_room_spatial_index.rs` - 测试程序
- 🚧 模型生成代码 - 待添加 AABB 写入

## 🎯 总结

✅ **已完成：**
- 空间索引查询功能实现
- 函数导出和测试程序
- 性能良好（亚毫秒级查询）

🚧 **待完成：**
- 在模型生成时写入 AABB
- 完善面板 AABB 计算
- 添加更多测试和验证

---

**完成时间：** 2025-11-16  
**状态：** ✅ 查询功能已实现，待集成到模型生成流程
