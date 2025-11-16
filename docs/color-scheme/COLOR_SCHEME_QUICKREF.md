# 颜色配置快速参考

## 快速开始

### 1. 在 Rust 代码中使用

```rust
use aios_core::color_scheme::ColorSchemeManager;
use aios_database::fast_model::material_config::MaterialLibrary;

// 方式 1: 直接使用颜色配置管理器
let manager = ColorSchemeManager::load_from_file("ColorSchemes.toml")
    .unwrap_or_default();
let pipe_color = manager.get_color_for_type(PdmsGenericType::PIPE);

// 方式 2: 通过材质库使用(推荐)
let library = MaterialLibrary::load_default()?;
let normalized_color = library.get_normalized_color_for_noun("PIPE");
```

### 2. 在模型导出中自动应用

颜色配置会在模型导出时自动应用,无需额外代码。

## 支持的元件类型速查表

| 类型 | 名称 | 默认颜色 (RGB) | 用途 |
|------|------|----------------|------|
| PIPE | 管道 | 255, 255, 0 (黄) | 管道系统 |
| EQUI | 设备 | 255, 190, 0 (橙) | 设备主体 |
| CE | 设备组件 | 0, 100, 200 (蓝) | 设备零件 |
| STRU | 结构 | 0, 150, 255 (青) | 结构件 |
| WALL | 墙体 | 150, 150, 150 (灰) | 普通墙 |
| GWALL | 玻璃墙 | 173, 216, 230 (淡蓝) | 玻璃幕墙 |
| ROOM | 房间 | 144, 238, 144 (绿) | 房间区域 |
| HANG | 悬挂件 | 255, 126, 0 (橙红) | 管道支架 |
| FLOOR | 地板 | 210, 180, 140 (棕) | 地板 |
| HVAC | 暖通空调 | 175, 238, 238 (青绿) | 空调系统 |

完整列表见 `ColorSchemes.toml`

## 配色方案

### standard_pdms (默认)
标准 PDMS 配色,与原始系统一致

### high_contrast
高对比度配色,适用于视觉辅助

### dark_mode
夜间模式配色,适用于暗色主题

## 常用 API

```rust
// ColorSchemeManager
manager.get_color_for_type(pdms_type)       // 获取颜色 [u8; 4]
manager.set_current_scheme("dark_mode")     // 切换方案
manager.get_available_schemes()             // 列出所有方案

// MaterialLibrary
library.get_color_for_noun("PIPE")                    // [u8; 4]
library.get_normalized_color_for_noun("PIPE")         // [f32; 4]
library.create_color_based_material("PIPE", false)    // JSON Value
library.get_or_create_material_for_noun(...)          // usize (索引)
```

## 配置文件格式

```toml
[schemes.方案名]
name = "显示名称"
description = "描述"

[schemes.方案名.colors]
类型名 = [R, G, B, A]  # 0-255
```

## 测试

```bash
# 运行颜色配置测试
cargo run --example test_color_scheme --features gen_model
```

## 文档

- **使用指南**: [COLOR_SCHEME_USAGE.md](COLOR_SCHEME_USAGE.md)
- **集成报告**: [COLOR_SCHEME_INTEGRATION.md](COLOR_SCHEME_INTEGRATION.md)
- **配置文件**: [ColorSchemes.toml](ColorSchemes.toml)

## 常见问题

**Q: 如何添加新的配色方案?**
A: 在 `ColorSchemes.toml` 中添加新的 `[schemes.xxx]` 部分

**Q: 如何自定义某个类型的颜色?**
A: 修改配置文件中对应类型的颜色值

**Q: 如果配置文件不存在会怎样?**
A: 系统会使用内置的默认配色方案

**Q: 导出的模型颜色不对?**
A: 检查 noun 类型名称是否正确,是否在配置文件中定义

**Q: 如何使用 PBR vs Unlit 材质?**
A: 在导出配置中设置 `use_basic_materials` 参数
