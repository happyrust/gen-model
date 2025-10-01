# XTK 数据库生成器

本模块提供了从 PDMS 数据库生成 XTK 格式三维模型文件的功能。XTK 是一种高效的三维模型格式，适用于 Web 端的三维可视化应用。

## 功能特性

### 🚀 核心功能
- **数据库直接导出**: 直接从 PDMS 数据库读取几何数据并转换为 XTK 格式
- **批量处理**: 支持按数据库号或参考号列表批量导出
- **几何体转换**: 自动将 PDMS 几何参数转换为标准几何体（立方体、圆柱体、球体等）
- **材质映射**: 根据 PDMS 元素类型自动分配颜色和材质
- **压缩支持**: 可选的 gzip 压缩以减小文件大小
- **进度监控**: 实时显示处理进度和统计信息

### 📊 支持的几何类型
- **PrimBox**: 立方体/长方体
- **PrimCylinder**: 圆柱体
- **PrimSCylinder**: 特殊圆柱体
- **PrimSphere**: 球体
- **PrimPyramid**: 金字塔（近似为立方体）
- **占位符**: 对于无几何数据的元素创建半透明占位符

### 🎨 颜色方案
根据 PDMS 元素类型自动分配颜色：
- **PIPE**: 蓝色系（管道、弯头、三通等）
- **VALVE**: 红色系（各种阀门）
- **EQUIPMENT**: 绿色系（设备、容器、泵等）
- **STRUCTURE**: 橙色系（梁、柱、板等）
- **INSTRUMENT**: 黄色系（仪表、变送器等）
- **ELECTRICAL**: 紫色系（电缆、导管等）

## 使用方法

### 1. 基本用法

```rust
use crate::fast_model::gen_model::{generate_xtk_from_database, generate_xtk_by_dbno};
use aios_core::options::DbOption;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_option = DbOption::default();
    
    // 方法1: 根据参考号列表生成
    let refnos = vec![
        "12345/67890".into(),
        "23456/78901".into(),
        "34567/89012".into(),
    ];
    
    generate_xtk_from_database(
        refnos,
        "output/my_model.xkt",
        true, // 启用压缩
        &db_option,
    ).await?;
    
    // 方法2: 根据数据库号生成整个数据库
    generate_xtk_by_dbno(
        1, // 数据库号
        "output/database_1.xkt",
        true, // 启用压缩
        &db_option,
    ).await?;
    
    Ok(())
}
```

### 2. 批量导出多个数据库

```rust
use crate::fast_model::gen_model::generate_xtk_by_dbno;
use aios_core::options::DbOption;

async fn batch_export() -> anyhow::Result<()> {
    let db_option = DbOption::default();
    let database_numbers = vec![1, 2, 3, 4, 5];
    
    std::fs::create_dir_all("output/batch_export")?;
    
    for dbno in database_numbers {
        let output_path = format!("output/batch_export/database_{}.xkt", dbno);
        
        println!("正在处理数据库号: {}", dbno);
        match generate_xtk_by_dbno(dbno, &output_path, true, &db_option).await {
            Ok(_) => println!("✅ 数据库 {} 导出成功", dbno),
            Err(e) => eprintln!("❌ 数据库 {} 导出失败: {}", dbno, e),
        }
    }
    
    Ok(())
}
```

### 3. 过滤特定类型的元素

```rust
use crate::fast_model::gen_model::generate_xtk_from_database;
use aios_core::{query_type_refnos_by_dbnum, options::DbOption};

async fn export_piping_system() -> anyhow::Result<()> {
    let db_option = DbOption::default();
    
    // 只导出管道相关的元素
    let pipe_types = ["PIPE", "ELBO", "TEE", "REDU"];
    let pipe_refnos = query_type_refnos_by_dbnum(&pipe_types, 1, None, false).await?;
    
    generate_xtk_from_database(
        pipe_refnos,
        "output/piping_system.xkt",
        true,
        &db_option,
    ).await?;
    
    Ok(())
}
```

## API 参考

### 主要函数

#### `generate_xtk_from_database`
从指定的参考号列表生成 XTK 文件。

**参数:**
- `refnos: Vec<RefnoEnum>` - 要导出的参考号列表
- `output_path: &str` - 输出文件路径
- `compress: bool` - 是否启用压缩
- `db_option: &DbOption` - 数据库选项配置

**返回值:**
- `anyhow::Result<()>` - 成功返回 `Ok(())`，失败返回错误信息

#### `generate_xtk_by_dbno`
根据数据库号导出整个数据库的 XTK 文件。

**参数:**
- `dbno: u32` - 数据库号
- `output_path: &str` - 输出文件路径
- `compress: bool` - 是否启用压缩
- `db_option: &DbOption` - 数据库选项配置

**返回值:**
- `anyhow::Result<()>` - 成功返回 `Ok(())`，失败返回错误信息

#### `generate_xtk_by_dbno_refno`
根据数据库号与单个参考号导出对应层级的 XKT 文件。

**参数:**
- `dbno: u32` - 数据库号
- `refno: RefnoEnum` - 目标参考号
- `output_path: &str` - 输出文件路径
- `compress: bool` - 是否启用压缩
- `db_option: &DbOption` - 数据库选项配置

**返回值:**
- `anyhow::Result<()>` - 成功返回 `Ok(())`，失败返回错误信息

### 辅助函数

#### `process_refno_to_xtk`
处理单个参考号并转换为 XTK 格式（内部函数）。

#### `create_geometry_from_geo_param`
从 PDMS 几何参数创建 XTK 几何体（内部函数）。

#### `create_placeholder_entity`
为没有几何数据的元素创建占位符实体（内部函数）。

## 输出格式

生成的 XTK 文件包含以下结构：

```
XTK File Structure:
┌─────────────────┐
│ 文件头          │ <- 魔数、版本、时间戳
├─────────────────┤
│ 模型元数据      │ <- 标题、作者、创建时间等
├─────────────────┤
│ 几何体数据      │ <- 顶点、法向量、索引等
├─────────────────┤
│ 材质数据        │ <- 颜色、透明度、材质属性
├─────────────────┤
│ 网格数据        │ <- 几何体与材质的关联
├─────────────────┤
│ 实体数据        │ <- 逻辑对象、属性、层次结构
└─────────────────┘
```

## 性能优化

### 内存管理
- **分批处理**: 大型数据库按批次处理，避免内存溢出
- **几何体复用**: 相同几何体被多个网格引用，减少内存占用
- **流式处理**: 边读取边转换，降低内存峰值

### 处理速度
- **并发处理**: 利用异步处理提高效率
- **缓存机制**: 重复使用的几何体和材质被缓存
- **增量更新**: 支持增量更新模式

### 文件大小
- **压缩选项**: 可选的 gzip 压缩，通常可减少 60-80% 的文件大小
- **几何体优化**: 自动优化几何体数据结构
- **索引优化**: 使用索引减少重复数据

## 错误处理

### 常见错误及解决方案

1. **数据库连接失败**
   ```
   错误: 无法连接到数据库
   解决: 检查数据库配置和网络连接
   ```

2. **参考号不存在**
   ```
   错误: 处理参考号 12345/67890 时出错
   解决: 验证参考号是否存在于数据库中
   ```

3. **几何数据缺失**
   ```
   警告: 创建几何体失败，使用占位符
   解决: 正常情况，会自动创建半透明占位符
   ```

4. **文件写入失败**
   ```
   错误: 无法写入文件
   解决: 检查输出目录权限和磁盘空间
   ```

## 示例程序

项目包含多个示例程序，位于 `src/xkt_generator/examples.rs`：

- `example_generate_xtk_from_database()` - 基本导出示例
- `example_batch_generate_xtk()` - 批量导出示例
- `example_filtered_xtk_generation()` - 过滤导出示例

运行示例：
```bash
cargo test --release example_generate_xtk_from_database
```

## 测试

运行所有 XTK 相关测试：
```bash
cargo test xtk --release
```

运行特定测试：
```bash
cargo test test_xtk_database_generation --release
cargo test test_xtk_geometry_conversion --release
```

## 配置选项

### DbOption 配置
```rust
let mut db_option = DbOption::default();
db_option.gen_mesh = true;           // 启用网格生成
db_option.gen_model = true;          // 启用模型生成
db_option.debug_root_refnos = None;  // 调试模式参考号
```

### 输出选项
- **压缩**: 建议对大型模型启用压缩
- **路径**: 支持相对路径和绝对路径
- **格式**: 固定为 `.xkt` 扩展名

## 注意事项

1. **数据库权限**: 确保有足够的数据库读取权限
2. **磁盘空间**: 大型数据库可能生成几百MB到几GB的文件
3. **处理时间**: 复杂模型的处理时间可能较长，请耐心等待
4. **内存使用**: 建议在内存充足的环境中运行
5. **版本兼容**: 确保 XTK 查看器支持当前版本的文件格式

## 故障排除

### 性能问题
- 减少批次大小（BATCH_SIZE）
- 启用压缩减少 I/O 开销
- 增加系统内存

### 质量问题
- 检查源数据完整性
- 验证几何参数有效性
- 使用调试模式查看详细信息

### 兼容性问题
- 确认 XTK 查看器版本
- 检查文件格式版本
- 验证压缩设置

## 更新日志

### v1.0.0
- ✅ 基本 XTK 生成功能
- ✅ 数据库直接导出
- ✅ 几何体转换
- ✅ 材质映射
- ✅ 压缩支持
- ✅ 批量处理
- ✅ 错误处理
- ✅ 测试覆盖

## 贡献

欢迎提交 Issue 和 Pull Request 来改进这个模块。

## 许可证

本项目遵循项目根目录的许可证。 
