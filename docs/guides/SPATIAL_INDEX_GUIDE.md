# 空间索引预构建指南

## 概述

为了提升空间查询服务的启动速度和查询性能，你可以预先构建空间树数据结构。本指南将详细说明如何构建、管理和使用预构建的空间索引。

## 快速开始

### 1. 自动构建（推荐）

```bash
# 给脚本添加执行权限
chmod +x build_spatial_index.sh

# 使用默认配置构建索引
./build_spatial_index.sh

# 使用自定义配置
./build_spatial_index.sh -d 1,2,3 -o my_spatial_index.bin -v -s
```

### 2. 手动构建

```bash
# 编译索引构建工具
cargo build --example spatial_index_builder --features grpc

# 从数据库构建索引
cargo run --example spatial_index_builder --features grpc -- build \
  --db-nos 1,2,3 \
  --output spatial_index.bin \
  --batch-size 10000 \
  --tolerance 0.001
```

## 构建选项详解

### 基础参数

| 参数 | 说明 | 默认值 | 示例 |
|------|------|--------|------|
| `--db-nos` | 数据库编号列表 | `1,2,3` | `--db-nos 1,5,10` |
| `--output` | 输出文件路径 | `./spatial_index.bin` | `--output /data/index.bin` |
| `--batch-size` | 批量处理大小 | `10000` | `--batch-size 5000` |
| `--tolerance` | 包围盒容差 | `0.001` | `--tolerance 0.01` |

### 过滤参数

| 参数 | 说明 | 默认值 | 示例 |
|------|------|--------|------|
| `--filter-types` | 只包含指定类型 | 空(全部) | `--filter-types PIPE,EQUI,STRU` |
| `--min-bbox-size` | 最小包围盒尺寸 | `0.0001` | `--min-bbox-size 0.01` |

### 构建示例

```bash
# 1. 只构建管道和设备索引
./build_spatial_index.sh -d 1,2 -f PIPE,EQUI -o pipe_equi_index.bin

# 2. 大批量数据，使用小批次处理
./build_spatial_index.sh -d 1,2,3,4,5 -b 5000 -o large_index.bin

# 3. 高精度索引（小容差）
./build_spatial_index.sh -d 1 -t 0.0001 -o precision_index.bin

# 4. 构建后验证和显示统计
./build_spatial_index.sh -d 1,2,3 -v -s -o verified_index.bin
```

## 使用预构建索引

### 启动服务器

```bash
# 方式1: 使用预构建索引文件启动服务器
cargo run --example spatial_query_server --features grpc -- --index-file spatial_index.bin

# 方式2: 在代码中使用预构建索引
let service = SpatialQueryServiceImpl::from_index_file("spatial_index.bin").await?;
```

### 性能对比

| 启动方式 | 索引构建时间 | 内存使用 | 启动时间 |
|----------|-------------|----------|----------|
| 动态构建 | 每次启动都构建 | 峰值较高 | 较慢 |
| 预构建索引 | 离线构建一次 | 稳定 | 极快 |

## 索引管理

### 验证索引文件

```bash
# 检查索引文件是否有效
cargo run --example spatial_index_builder --features grpc -- validate --file spatial_index.bin

# 自动脚本验证
./build_spatial_index.sh -v
```

### 查看索引统计

```bash
# 显示详细统计信息
cargo run --example spatial_index_builder --features grpc -- stats --file spatial_index.bin

# 脚本方式
./build_spatial_index.sh -s
```

统计信息包括：
- 总构件数量和已索引数量
- 构建耗时和内存估算
- 各类型构件分布
- 包围盒体积统计

### 合并索引文件

```bash
# 合并多个索引文件
cargo run --example spatial_index_builder --features grpc -- merge \
  --inputs index1.bin,index2.bin,index3.bin \
  --output merged_index.bin
```

## 最佳实践

### 1. 索引更新策略

```bash
# 定期重建索引（建议每日）
#!/bin/bash
# daily_index_rebuild.sh
DATE=$(date +%Y%m%d)
BACKUP_FILE="spatial_index_backup_${DATE}.bin"

# 备份当前索引
cp spatial_index.bin "$BACKUP_FILE"

# 重建索引
./build_spatial_index.sh -d 1,2,3 -o spatial_index.bin -v

# 重启服务（如果需要）
# systemctl restart spatial-query-service
```

### 2. 多环境配置

```bash
# 开发环境 - 快速构建
./build_spatial_index.sh -d 1 -b 1000 -o dev_index.bin

# 测试环境 - 部分数据
./build_spatial_index.sh -d 1,2 -b 5000 -o test_index.bin

# 生产环境 - 完整数据
./build_spatial_index.sh -d 1,2,3,4,5 -b 10000 -o prod_index.bin -v -s
```

### 3. 存储建议

- **本地存储**: 适合单机部署
  ```bash
  --output /var/lib/spatial-query/spatial_index.bin
  ```

- **共享存储**: 适合集群部署
  ```bash
  --output /shared/spatial-query/spatial_index.bin
  ```

- **版本管理**: 建议保留多个版本
  ```bash
  /data/spatial-indexes/
  ├── current -> spatial_index_20241201.bin
  ├── spatial_index_20241201.bin
  ├── spatial_index_20241130.bin
  └── spatial_index_20241129.bin
  ```

## 监控和维护

### 1. 索引健康检查

```bash
#!/bin/bash
# health_check.sh

INDEX_FILE="/var/lib/spatial-query/spatial_index.bin"

if [ ! -f "$INDEX_FILE" ]; then
    echo "ERROR: Index file not found"
    exit 1
fi

# 验证索引完整性
if cargo run --example spatial_index_builder --features grpc -- validate --file "$INDEX_FILE" > /dev/null 2>&1; then
    echo "OK: Index file is valid"
    exit 0
else
    echo "ERROR: Index file is corrupted"
    exit 1
fi
```

### 2. 性能监控

```bash
#!/bin/bash
# performance_monitor.sh

INDEX_FILE="spatial_index.bin"
LOG_FILE="index_performance.log"

echo "=== $(date) ===" >> "$LOG_FILE"

# 文件大小
echo "File size: $(ls -lh $INDEX_FILE | awk '{print $5}')" >> "$LOG_FILE"

# 加载时间测试
START_TIME=$(date +%s.%N)
cargo run --example spatial_index_builder --features grpc -- validate --file "$INDEX_FILE" > /dev/null 2>&1
END_TIME=$(date +%s.%N)
LOAD_TIME=$(echo "$END_TIME - $START_TIME" | bc)
echo "Load time: ${LOAD_TIME}s" >> "$LOG_FILE"

# 统计信息
cargo run --example spatial_index_builder --features grpc -- stats --file "$INDEX_FILE" >> "$LOG_FILE" 2>&1
```

## 故障排除

### 常见问题

1. **构建失败: "数据库连接错误"**
   ```bash
   # 检查数据库配置
   cat DbOption.toml
   
   # 验证数据库连接
   cargo test --test database_connection
   ```

2. **内存不足**
   ```bash
   # 减小批量大小
   ./build_spatial_index.sh -b 1000
   
   # 按类型分批构建后合并
   ./build_spatial_index.sh -f PIPE -o pipe_index.bin
   ./build_spatial_index.sh -f EQUI -o equi_index.bin
   cargo run --example spatial_index_builder --features grpc -- merge \
     --inputs pipe_index.bin,equi_index.bin --output combined_index.bin
   ```

3. **索引文件损坏**
   ```bash
   # 重新构建
   ./build_spatial_index.sh -c -v
   
   # 从备份恢复
   cp spatial_index_backup.bin spatial_index.bin
   ```

### 调试模式

```bash
# 启用详细日志
RUST_LOG=debug ./build_spatial_index.sh -d 1 -o debug_index.bin

# 构建小规模测试索引
./build_spatial_index.sh -d 1 -b 100 -o test_index.bin -v -s
```

## API 集成

### Rust 代码示例

```rust
use aios_database::grpc_service::spatial_query_service::SpatialQueryServiceImpl;
use aios_database::grpc_service::spatial_index_builder::{
    SpatialIndexBuilder, SpatialIndexConfig, SpatialIndexPersistence
};

// 1. 使用预构建索引
let service = SpatialQueryServiceImpl::from_index_file("spatial_index.bin").await?;

// 2. 动态构建索引
let db_manager = Arc::new(AiosDBManager::init_form_config().await?);
let config = SpatialIndexConfig {
    batch_size: 5000,
    filter_types: vec!["PIPE".to_string(), "EQUI".to_string()],
    ..Default::default()
};

let builder = SpatialIndexBuilder::new(db_manager).with_config(config);
let (rtree, statistics) = builder.build_from_database(&[1, 2, 3]).await?;

// 3. 保存索引
SpatialIndexPersistence::save_index(&rtree, &statistics, "my_index.bin")?;
```

通过预构建空间索引，你可以显著提升空间查询服务的性能，特别是在处理大规模数据时。建议根据实际需求选择合适的构建策略和参数配置。