# XKT 格式生成器

基于 [xeokit-convert](https://xeokit.github.io/xeokit-convert/docs/) 规范实现的 XKT 格式生成器，用于将 PDMS 数据转换为 xeokit 可视化格式。

## 功能特性

- ✅ **完整的 XKT 数据模型**：支持几何体、材质、网格、实体的完整层次结构
- ✅ **多种几何体类型**：立方体、球体、圆柱体等基础几何体
- ✅ **智能颜色方案**：根据 PDMS 类型自动分配颜色
- ✅ **材质系统**：支持金属、塑料、木材等多种材质类型
- ✅ **文件压缩**：支持 gzip 压缩以减小文件大小
- ✅ **完整测试覆盖**：包含单元测试和集成测试
- ✅ **丰富示例**：提供桌子、管道系统、工厂布局等示例

## 快速开始

### 1. 运行演示程序

```bash
# 运行所有测试和示例
cargo run --bin xkt_demo -- --mode all

# 仅运行测试
cargo run --bin xkt_demo -- --mode test

# 仅运行示例
cargo run --bin xkt_demo -- --mode examples
```

### 2. 基本使用示例

```rust
use aios_database::xkt_generator::*;
use glam::Vec3;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建 XKT 文件
    let mut xkt_file = XKTFile::new();
    xkt_file.model.metadata.title = "我的模型".to_string();
    
    // 创建颜色方案
    let color_scheme = ColorScheme::new();
    
    // 创建几何体
    let box_geometry = XKTGeometry::create_box("box_geo".to_string(), 2.0, 1.0, 1.0);
    xkt_file.model.create_geometry(box_geometry)?;
    
    // 创建材质
    let pipe_color = color_scheme.get_color_for_type("PIPE");
    let material = XKTMaterial::create_color_material(
        "pipe_material".to_string(),
        "管道材质".to_string(),
        pipe_color,
    );
    xkt_file.model.create_material(material)?;
    
    // 创建网格
    let mut mesh = XKTMesh::new("pipe_mesh".to_string(), "box_geo".to_string());
    mesh.set_material("pipe_material".to_string());
    mesh.set_position(Vec3::new(0.0, 0.0, 0.0));
    xkt_file.model.create_mesh(mesh)?;
    
    // 创建实体
    let mut entity = XKTEntity::new("pipe_001".to_string(), "管道-001".to_string(), "PIPE".to_string());
    entity.add_mesh("pipe_mesh".to_string());
    entity.set_property("diameter".to_string(), "100".to_string());
    xkt_file.model.create_entity(entity)?;
    
    // 完成模型构建
    xkt_file.model.finalize().await?;
    
    // 保存文件
    xkt_file.save_to_file("my_model.xkt", true).await?;
    
    Ok(())
}
```

## 核心组件

### 1. XKTModel - 模型容器
- 管理几何体、材质、网格、实体的集合
- 提供统计信息和边界框计算
- 支持模型验证和完整性检查

### 2. XKTGeometry - 几何体
- 支持三角形、线条、点等几何类型
- 内置立方体、球体、圆柱体生成器
- 自动计算边界框和法向量

### 3. XKTMaterial - 材质
- 支持漫反射、镜面反射、金属度等属性
- 预定义金属、塑料、木材等材质类型
- 支持纹理映射

### 4. XKTMesh - 网格
- 连接几何体和材质
- 支持位置、旋转、缩放变换
- 可设置颜色覆盖和透明度

### 5. XKTEntity - 实体
- 表示逻辑对象，可包含多个网格
- 支持层次结构（父子关系）
- 可附加自定义属性

### 6. ColorScheme - 颜色方案
- 根据 PDMS 类型自动分配颜色
- 支持管道、阀门、设备等预定义类型
- 可生成基于哈希的唯一颜色

## PDMS 类型颜色映射

| 类型 | 颜色系 | 示例组件 |
|------|--------|----------|
| PIPE | 蓝色系 | 管道、弯头、三通、异径管 |
| VALVE | 红色系 | 闸阀、球阀、止回阀 |
| EQUIPMENT | 绿色系 | 容器、罐体、泵 |
| STRUCTURE | 橙色系 | 梁、柱、板 |
| INSTRUMENT | 黄色系 | 仪表、变送器 |
| ELECTRICAL | 紫色系 | 电缆、导管 |
| HVAC | 青色系 | 风管、阻尼器 |

## 文件格式

生成的 XKT 文件包含：

1. **文件头**：魔数、版本、时间戳
2. **模型数据**：JSON 格式的模型信息
3. **压缩选项**：可选的 gzip 压缩

```
XKT File Structure:
┌─────────────────┐
│ Header (24 bytes) │
├─────────────────┤
│ Compression Flag │
├─────────────────┤
│ Data Size Info  │
├─────────────────┤
│ Model Data      │
│ (JSON/Binary)   │
└─────────────────┘
```

## 测试覆盖

### 单元测试
- ✅ 几何体创建和验证
- ✅ 颜色方案匹配
- ✅ 材质属性设置
- ✅ 文件头验证

### 集成测试
- ✅ 基本 XKT 文件生成
- ✅ 复杂场景构建
- ✅ 压缩功能验证
- ✅ 模型完整性检查

### 示例程序
- ✅ 简单桌子模型
- ✅ 管道系统布局
- ✅ 工厂设备布局

## 性能优化

1. **几何体复用**：相同几何体可被多个网格引用
2. **材质共享**：相同材质可被多个网格使用
3. **文件压缩**：支持 gzip 压缩减小文件大小
4. **二进制格式**：可选的二进制几何数据格式

## 与 xeokit 集成

生成的 XKT 文件可直接在 xeokit 查看器中加载：

```javascript
import {Viewer, XKTLoaderPlugin} from "@xeokit/xeokit-sdk";

const viewer = new Viewer({
    canvasId: "myCanvas"
});

const xktLoader = new XKTLoaderPlugin(viewer);

const model = xktLoader.load({
    id: "myModel",
    src: "./my_model.xkt"
});

model.on("loaded", () => {
    viewer.cameraFlight.flyTo(model);
});
```

## 扩展开发

### 添加新的几何体类型

```rust
impl XKTGeometry {
    pub fn create_torus(id: String, major_radius: f32, minor_radius: f32, segments: u32) -> Self {
        // 实现圆环几何体生成
        // ...
    }
}
```

### 自定义材质类型

```rust
let custom_material = XKTMaterial::new("custom_mat".to_string(), "自定义材质".to_string());
custom_material.set_metallic(0.8);
custom_material.set_roughness(0.2);
```

### 扩展颜色方案

```rust
let mut color_scheme = ColorScheme::new();
color_scheme.add_type_color("CUSTOM_TYPE".to_string(), Vec3::new(1.0, 0.5, 0.0));
```

## 依赖项

- `glam`: 3D 数学库
- `serde`: 序列化支持
- `flate2`: 压缩功能
- `byteorder`: 二进制数据处理
- `uuid`: 唯一标识符生成
- `chrono`: 时间处理

## 许可证

本项目遵循与主项目相同的许可证。

## 贡献

欢迎提交 Issue 和 Pull Request 来改进 XKT 生成器功能。

## 参考资料

- [xeokit-convert 文档](https://xeokit.github.io/xeokit-convert/docs/)
- [xeokit SDK](https://xeokit.github.io/xeokit-sdk/)
- [XKT 格式规范](https://github.com/xeokit/xeokit-convert) 