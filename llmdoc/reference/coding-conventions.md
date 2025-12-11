# 编码约定

## Rust 代码规范

### 命名约定
- **模块名**：snake_case (`fast_model`, `data_interface`)
- **类型名**：PascalCase (`PlantMesh`, `DbOption`)
- **函数名**：snake_case (`gen_inst_meshes`, `process_meshes_update_db`)
- **常量**：SCREAMING_SNAKE_CASE (`EXIST_MESH_GEO_HASHES`)

### 异步编程
- 使用 `tokio` 运行时
- 异步函数使用 `async fn` 和 `.await`
- 并发集合使用 `DashMap` / `DashSet`

### 错误处理
```rust
// 使用 anyhow::Result 作为返回类型
pub async fn gen_model() -> anyhow::Result<()> {
    // 使用 ? 传播错误
    let data = load_data().await?;
    Ok(())
}
```

### 日志与调试
```rust
// 调试宏
debug_model!("message");
smart_debug_model!("conditional message");
smart_debug_error!("error message");

// 启用调试：设置 debug_expr 特性或环境变量
```

## 模块组织

### 文件结构
```
src/
├── lib.rs          # 库入口，公开 API
├── main.rs         # 二进制入口
├── fast_model/     # 模型生成核心
│   ├── mod.rs      # 模块导出
│   ├── gen_model/  # 生成逻辑子模块
│   └── export_model/ # 导出功能
└── data_interface/ # 数据接口层
```

### Feature Flags
```toml
[features]
default = ["ws", "gen_model", "manifold", "sqlite-index", "surreal-save"]
gen_model = ["aios_core/gen_model"]
manifold = ["aios_core/manifold"]
sqlite-index = ["dep:rusqlite", "aios_core/sqlite"]
```

## 数据类型

### 核心类型
- `RefU64` - PDMS 引用号
- `AttrMap` - 属性映射
- `PlantMesh` - 网格数据
- `Aabb` - 轴对齐包围盒

### 集合类型
```rust
use dashmap::DashMap;
use dashmap::DashSet;

// 并发安全的哈希映射
let cache: DashMap<RefU64, PlantMesh> = DashMap::new();
```

## 注释规范
- 公开 API 使用 `///` 文档注释
- 复杂逻辑添加行内注释说明
- 中文注释允许用于业务逻辑说明
