# PDMS 增量更新 API 使用指南

## 快速开始

### 基本使用

```rust
use crate::data_interface::tidb_manager::AiosDBManager;

// 创建数据库管理器实例
let db_manager = AiosDBManager::new();

// 初始化文件监控器
db_manager.init_watcher().await?;

// 开始异步监控
db_manager.async_watch().await?;
```

## 核心 API

### 1. 增量更新执行

#### `execute_incr_update`

执行增量更新的核心方法。

```rust
pub async fn execute_incr_update(
    &self,
    increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, RangeInclusive<i32>)>,
) -> anyhow::Result<bool>
```

**参数说明:**
- `increment_ranges_map`: 增量更新范围映射
  - Key: 数据库文件路径
  - Value: (数据库页面基本信息, 会话号范围)

**使用示例:**
```rust
let mut increment_ranges_map = IndexMap::new();
increment_ranges_map.insert(
    PathBuf::from("/path/to/database.db"),
    (basic_info, 100..=200)
);

let result = db_manager.execute_incr_update(increment_ranges_map).await?;
```

### 2. 文件监控初始化

#### `init_watcher`

初始化文件监控器，扫描现有文件并检查增量更新需求。

```rust
pub async fn init_watcher(&self) -> anyhow::Result<()>
```

**功能:**
- 扫描监控目录中的所有数据库文件
- 比较文件会话号与数据库记录
- 自动执行需要的增量更新
- 生成MQTT同步所需的压缩包

**使用示例:**
```rust
// 系统启动时调用
db_manager.init_watcher().await?;
```

### 3. 异步文件监控

#### `async_watch`

启动实时文件监控，自动处理文件变化。

```rust
pub async fn async_watch(&self) -> notify::Result<()>
```

**功能:**
- 实时监控文件系统变化
- 自动触发增量更新
- 处理MQTT同步消息
- 更新本地缓存

**使用示例:**
```rust
// 在后台任务中运行
tokio::spawn(async move {
    db_manager.async_watch().await.unwrap();
});
```

### 4. 会话号查询

#### `query_latest_sesno_by_dbnum`

根据数据库编号查询最新会话号。

```rust
async fn query_latest_sesno_by_dbnum(dbnum: u32) -> anyhow::Result<u32>
```

**使用示例:**
```rust
let latest_sesno = AiosDBManager::query_latest_sesno_by_dbnum(1001).await?;
println!("数据库1001的最新会话号: {}", latest_sesno);
```

## 配置选项

### 数据库配置

```rust
// 获取数据库配置
let db_option = get_db_option();

// 手动指定的数据库编号（用于调试）
let manual_dbnums = db_option.manual_db_nums.clone().unwrap_or_default();

// 需要排除的数据库编号
let exclude_dbnums = db_option.exclude_db_nums.clone().unwrap_or_default();

// 项目名称
let project_name = db_option.project_name.clone();
```

### 监控目录配置

```rust
// 监控目录列表
let watch_dirs = &db_manager.watcher.watch_dirs;

// 添加监控目录
db_manager.watcher.watch_dirs.push(PathBuf::from("/new/watch/dir"));
```

### MQTT同步配置

```rust
// 地区数据库配置
let location_dbs = db_option.location_dbs;

// 当前地区标识
let location = db_option.location;
```

## 错误处理

### 常见错误类型

```rust
use anyhow::Result;

match db_manager.execute_incr_update(params).await {
    Ok(true) => println!("增量更新成功"),
    Ok(false) => println!("没有需要更新的内容"),
    Err(e) => {
        eprintln!("增量更新失败: {}", e);
        // 错误处理逻辑
    }
}
```

### 错误恢复

```rust
// 重试机制示例
let mut retry_count = 0;
const MAX_RETRIES: usize = 3;

while retry_count < MAX_RETRIES {
    match db_manager.execute_incr_update(params.clone()).await {
        Ok(_) => break,
        Err(e) => {
            retry_count += 1;
            eprintln!("重试 {}/{}: {}", retry_count, MAX_RETRIES, e);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}
```

## 性能优化建议

### 1. 批处理大小调整

```rust
// 调整JSON分块大小
const JSON_CHUNK_COUNT: usize = 200; // 默认值，可根据内存情况调整
```

### 2. 监控目录优化

```rust
// 避免监控包含大量文件的目录
// 使用具体的数据库目录而非根目录
let watch_dirs = vec![
    PathBuf::from("/specific/database/dir"),
    // 避免: PathBuf::from("/")
];
```

### 3. 数据库连接优化

```rust
// 使用连接池
// 设置合适的超时时间
// 定期清理连接
```

## 调试和监控

### 日志配置

```rust
// 启用详细日志
env_logger::init();

// 或使用tracing
use tracing::{info, warn, error};

info!("开始增量更新");
warn!("检测到潜在问题");
error!("增量更新失败: {}", error);
```

### 性能监控

```rust
use std::time::Instant;

let start = Instant::now();
db_manager.execute_incr_update(params).await?;
let duration = start.elapsed();
println!("增量更新耗时: {:?}", duration);
```

### 内存监控

```rust
// 监控内存使用
use sysinfo::{System, SystemExt};

let mut system = System::new_all();
system.refresh_all();
println!("内存使用: {} MB", system.used_memory() / 1024 / 1024);
```

## 最佳实践

### 1. 系统启动流程

```rust
async fn start_increment_system() -> Result<()> {
    let db_manager = AiosDBManager::new();
    
    // 1. 初始化监控器
    db_manager.init_watcher().await?;
    
    // 2. 启动异步监控
    let watch_handle = tokio::spawn(async move {
        db_manager.async_watch().await
    });
    
    // 3. 等待监控任务
    watch_handle.await??;
    
    Ok(())
}
```

### 2. 错误处理策略

```rust
// 使用Result类型进行错误传播
// 在关键点添加错误日志
// 实现优雅的降级处理
```

### 3. 资源管理

```rust
// 及时释放大对象
drop(large_data);

// 使用Arc<Mutex<>>进行线程安全的共享
use std::sync::{Arc, Mutex};
let shared_data = Arc::new(Mutex::new(data));
```

## 故障排除

### 常见问题

1. **文件监控失效**
   ```rust
   // 检查文件权限
   // 验证监控目录配置
   // 确认文件系统支持
   ```

2. **数据库连接超时**
   ```rust
   // 增加连接超时时间
   // 检查网络连接
   // 验证数据库服务状态
   ```

3. **内存溢出**
   ```rust
   // 减小批处理大小
   // 增加内存释放点
   // 优化数据结构
   ```

### 调试工具

```rust
// 启用调试模式
#[cfg(debug_assertions)]
{
    dbg!(&increment_info);
    println!("调试信息: {:?}", debug_data);
}

// 性能分析
#[cfg(feature = "profiling")]
{
    // 性能分析代码
}
```

## 扩展开发

### 自定义监控器

```rust
impl CustomWatcher {
    pub fn new() -> Self {
        // 自定义监控器实现
    }
    
    pub async fn custom_watch(&self) -> Result<()> {
        // 自定义监控逻辑
    }
}
```

### 插件系统

```rust
trait IncrementPlugin {
    async fn before_update(&self, data: &IncrementInfo) -> Result<()>;
    async fn after_update(&self, data: &IncrementInfo) -> Result<()>;
}
```

这个API使用指南提供了PDMS增量更新系统的完整使用方法，包括基本用法、配置选项、错误处理和最佳实践。开发者可以根据这个指南快速上手并有效使用增量更新功能。
