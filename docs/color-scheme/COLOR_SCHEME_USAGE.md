# 颜色配置功能使用指南

## 概述

本项目已经集成了基于 PDMS 标准的颜色配置系统,可以为导出的 3D 模型应用统一的配色方案。

## 功能特性

1. **标准 PDMS 配色**:基于原始 PDMS 系统的配色规则
2. **多配色方案支持**:可以在配置文件中定义多个配色方案并切换使用
3. **自动颜色映射**:根据元件类型(noun)自动应用对应的颜色
4. **动态材质生成**:如果材质库中没有定义,会根据颜色配置动态创建材质

## 配置文件

颜色配置存储在项目根目录的 `ColorSchemes.toml` 文件中,包含以下配色方案:

- **standard_pdms**: 标准 PDMS 配色(默认)
- **high_contrast**: 高对比度配色(适用于视觉辅助)
- **dark_mode**: 夜间模式配色

### 配色方案格式

```toml
[schemes.standard_pdms]
name = "标准 PDMS 配色"
description = "与原始 PDMS 系统一致的标准配色方案"

[schemes.standard_pdms.colors]
PIPE = [255, 255, 0, 255]      # 管道 - 亮黄色
EQUI = [255, 190, 0, 255]      # 设备 - 橙黄色
CE = [0, 100, 200, 180]        # 设备组件 - 深蓝色,半透明
STRU = [0, 150, 255, 255]      # 结构 - 青蓝色
# ... 更多类型
```

颜色格式为 `[R, G, B, A]`,取值范围 0-255。

## 在代码中使用

### 1. 在 aiOS-core 中使用

```rust
use aios_core::color_scheme::ColorSchemeManager;
use aios_core::pdms_types::PdmsGenericType;

// 加载颜色配置
let mut manager = ColorSchemeManager::load_from_file("ColorSchemes.toml")
    .unwrap_or_else(|_| ColorSchemeManager::default_schemes());

// 获取特定类型的颜色
if let Some(color) = manager.get_color_for_type(PdmsGenericType::PIPE) {
    println!("管道颜色: {:?}", color); // [255, 255, 0, 255]
}

// 切换配色方案
manager.set_current_scheme("high_contrast");
```

### 2. 在模型导出中使用

材质库(`MaterialLibrary`)会自动加载颜色配置:

```rust
use crate::fast_model::material_config::MaterialLibrary;

// 加载材质库(自动加载颜色配置)
let material_library = MaterialLibrary::load_default()?;

// 获取特定类型的颜色(归一化到 0.0-1.0)
if let Some(color) = material_library.get_normalized_color_for_noun("PIPE") {
    println!("管道颜色(归一化): {:?}", color);
}

// 创建基于颜色配置的材质
if let Some(material) = material_library.create_color_based_material("EQUI", false) {
    println!("设备材质: {}", material);
}
```

### 3. 在导出器中自动应用颜色

导出器会自动为每个 noun 类型查找合适的材质:

1. 首先查找材质库中的材质定义
2. 如果没有找到,则使用颜色配置创建动态材质
3. 支持 PBR 和 Unlit 两种材质模式

```rust
// 在导出配置中指定是否使用基础材质
let config = CommonExportConfig {
    use_basic_materials: true,  // 使用 KHR_materials_unlit
    ..Default::default()
};
```

## 支持的元件类型

系统支持以下 PDMS 元件类型的配色:

### 基础几何
- UNKOWN: 未知类型 - 浅灰色

### 设备系统
- CE: 设备组件 - 深蓝色
- EQUI: 设备 - 橙黄色

### 管道系统
- PIPE: 管道 - 亮黄色
- HANG: 悬挂件 - 橙红色

### 结构系统
- STRU: 结构 - 青蓝色
- SCTN: 截面 - 棕色
- GENSEC: 通用截面 - 棕色

### 建筑构件
- WALL: 墙体 - 中灰色
- STWALL: 结构墙 - 中灰色
- CWALL: 混凝土墙 - 深灰色
- GWALL: 玻璃墙 - 浅蓝色(半透明)
- FLOOR: 地板 - 浅棕色
- CFLOOR: 混凝土地板 - 深棕色
- PANE: 面板 - 浅灰色

### 空间和区域
- ROOM: 房间 - 浅绿色(半透明)
- AREADEF: 区域定义 - 浅紫色(半透明)

### 暖通空调
- HVAC: 暖通空调 - 浅蓝绿色

### 其他
- EXTR: 拉伸体 - 紫色
- REVO: 旋转体 - 深紫色
- HANDRA: 扶手 - 金色
- CWBRAN: 电缆桥架分支 - 深橙色
- CTWALL: 幕墙 - 浅蓝色(半透明)
- DEMOPA: 演示面板 - 红色
- INSURQ: 保温要求 - 粉色
- STRLNG: 结构长度 - 青色

## 扩展配色方案

要添加新的配色方案,在 `ColorSchemes.toml` 中添加新的 section:

```toml
[schemes.my_custom]
name = "我的自定义配色"
description = "适用于特定场景的配色方案"

[schemes.my_custom.colors]
PIPE = [100, 200, 255, 255]
EQUI = [255, 100, 100, 255]
# ... 其他类型
```

然后在代码中切换:

```rust
manager.set_current_scheme("my_custom");
```

## 注意事项

1. **颜色格式**: RGBA 值范围为 0-255
2. **透明度**: Alpha 通道控制透明度,255 为完全不透明,0 为完全透明
3. **配置文件位置**: 默认从项目根目录加载 `ColorSchemes.toml`
4. **回退机制**: 如果颜色配置文件不存在,系统会使用内置的默认配色方案
5. **材质优先级**: 材质库中定义的材质优先于颜色配置生成的材质

## 示例

查看以下示例代码了解完整用法:

```rust
use aios_database::fast_model::material_config::MaterialLibrary;

#[tokio::main]
async fn main() -> Result<()> {
    // 加载材质库(自动集成颜色配置)
    let library = MaterialLibrary::load_default()?;

    // 为不同类型创建材质
    let mut dynamic_materials = Vec::new();

    for noun in ["PIPE", "EQUI", "STRU", "WALL"] {
        if let Some(idx) = library.get_or_create_material_for_noun(
            noun,
            true,  // 使用基础材质
            &mut dynamic_materials,
        ) {
            println!("{} 材质索引: {}", noun, idx);
        }
    }

    Ok(())
}
```

## 相关文件

- `rs-core/src/color_scheme/`: 颜色配置核心模块
- `gen-model-fork/src/fast_model/material_config.rs`: 材质库和颜色配置集成
- `ColorSchemes.toml`: 颜色配置文件

## 架构说明

```
aiOS-core (rs-core)
  └─ color_scheme/
      ├─ mod.rs
      └─ color_schemes.rs          # 颜色配置管理器
          ├─ ColorScheme           # 单个配色方案
          ├─ ColorSchemes          # 配色方案集合
          └─ ColorSchemeManager    # 配色管理器

gen-model-fork
  └─ fast_model/
      └─ material_config.rs         # 材质库(集成颜色配置)
          └─ MaterialLibrary
              ├─ get_color_for_noun()
              ├─ create_color_based_material()
              └─ get_or_create_material_for_noun()
```

这样的设计使得:
1. 颜色配置逻辑在 aiOS-core 中独立管理
2. gen-model-fork 通过材质库使用颜色配置
3. 支持灵活的配色方案切换和扩展
