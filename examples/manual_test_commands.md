# 空间查询服务手动测试指南

## 快速开始

### 1. 自动化测试（推荐）
```bash
# 添加执行权限
chmod +x test_spatial_query.sh

# 运行完整测试
./test_spatial_query.sh
```

### 2. 手动测试步骤

#### 步骤1: 编译项目
```bash
cargo build --release
```

#### 步骤2: 启动服务器
```bash
# 终端1 - 启动服务器
cargo run --bin gen_model -- --spatial-query-server

# 或者直接运行测试服务器
cargo run --bin test_spatial_query
```

#### 步骤3: 运行客户端测试
```bash
# 终端2 - 运行客户端测试
cargo run --example spatial_query_client
```

## 测试用例说明

### 测试数据
服务器包含4个预设测试构件:

| 参考号 | 类型 | 名称 | 包围盒 | 说明 |
|--------|------|------|--------|------|
| 1001 | PIPE | 管道001 | (0,0,0)→(1,1,1) | 基础管道 |
| 1002 | EQUI | 设备001 | (0.5,0.5,0.5)→(1.5,1.5,1.5) | 与1001相交 |
| 1003 | PIPE | 管道002 | (2,2,2)→(3,3,3) | 独立管道 |
| 1004 | STRU | 结构001 | (0.8,0.8,0.8)→(2.2,2.2,2.2) | 大结构，与多个构件相交 |

### 预期测试结果

1. **索引统计**: 4个元素，3种类型(PIPE:2, EQUI:1, STRU:1)
2. **查询1001**: 应找到1002, 1004 (相交构件)
3. **查询1004**: 应找到1001, 1002 (大包围盒与多个相交)
4. **类型过滤**: 只返回指定类型的构件
5. **自定义包围盒**: 查询指定空间区域内的构件
6. **批量查询**: 同时查询多个构件的相交情况

## 使用 grpcurl 测试

如果已安装 grpcurl，可以直接测试API:

```bash
# 1. 获取服务信息
grpcurl -plaintext localhost:9090 describe

# 2. 获取索引统计
grpcurl -plaintext localhost:9090 spatial_query.SpatialQueryService/GetIndexStats

# 3. 单个查询
grpcurl -plaintext -d '{
    "refno": 1001,
    "include_self": false,
    "tolerance": 0.001,
    "max_results": 100
}' localhost:9090 spatial_query.SpatialQueryService/QueryIntersectingElements

# 4. 带类型过滤查询
grpcurl -plaintext -d '{
    "refno": 1001,
    "element_types": ["PIPE", "EQUI"],
    "include_self": false,
    "tolerance": 0.001,
    "max_results": 100
}' localhost:9090 spatial_query.SpatialQueryService/QueryIntersectingElements

# 5. 自定义包围盒查询
grpcurl -plaintext -d '{
    "refno": 1001,
    "custom_bbox": {
        "min": {"x": 0, "y": 0, "z": 0},
        "max": {"x": 2, "y": 2, "z": 2}
    },
    "include_self": true,
    "tolerance": 0.1,
    "max_results": 100
}' localhost:9090 spatial_query.SpatialQueryService/QueryIntersectingElements

# 6. 批量查询
grpcurl -plaintext -d '{
    "requests": [
        {"refno": 1001, "include_self": false, "tolerance": 0.001, "max_results": 10},
        {"refno": 1002, "include_self": false, "tolerance": 0.001, "max_results": 10}
    ],
    "parallel_execution": true
}' localhost:9090 spatial_query.SpatialQueryService/BatchQueryIntersecting

# 7. 重建索引
grpcurl -plaintext -d '{
    "force_rebuild": true
}' localhost:9090 spatial_query.SpatialQueryService/RebuildSpatialIndex
```

## 性能测试

### 简单负载测试
```bash
# 使用 Apache Bench (如果可用)
ab -n 1000 -c 10 -T 'application/grpc' http://127.0.0.1:9090/

# 或使用 wrk (如果可用)
wrk -t2 -c10 -d30s http://127.0.0.1:9090/
```

### 自定义性能测试
客户端程序中包含了基本的性能测量，会显示每个查询的耗时。

## 故障排除

### 常见问题

1. **连接失败**
   - 确保服务器正在运行
   - 检查端口9090是否被占用: `lsof -i :9090`

2. **编译错误**
   - 确保安装了必要的依赖: `cargo check`
   - 检查Rust版本是否兼容

3. **查询无结果**
   - 检查参考号是否存在(1001-1004)
   - 确认空间范围和容差设置

4. **性能问题**
   - 检查测试数据量
   - 调整查询参数(max_results, tolerance)

## 扩展测试

### 添加更多测试数据
修改 `src/grpc_service/spatial_query_service.rs` 中的 `build_initial_index` 函数。

### 集成现有数据库
替换测试数据加载逻辑，连接到实际的PDMS数据库。

### 并发测试
创建多个客户端并发查询，测试服务器的并发处理能力。