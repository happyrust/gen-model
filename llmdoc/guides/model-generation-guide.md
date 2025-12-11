# 模型生成使用指南

## 快速开始

### 1. 配置 DbOption.toml
```toml
# 基本配置
project_name = "my_project"
gen_model = true
gen_mesh = true

# 网格路径
meshes_path = "assets/meshes"

# 精度设置
[mesh_precision]
curve_segments = 32
min_segment_angle = 5.0
```

### 2. 运行模型生成
```bash
# 默认模式
cargo run --release

# Full Noun 模式（全量生成）
FULL_NOUN_MODE=true cargo run --release

# 指定数据库编号
cargo run --release -- --dbnos 1112,1113
```

## 生成模式

### Full Noun 模式
适用于首次完整生成或全量重建。

**启用方式**:
```bash
FULL_NOUN_MODE=true cargo run
```

**特点**:
- 按 Noun 类型分类处理
- 自动验证 SJUS 映射
- 并发批量处理

### 增量模式
适用于数据更新后的增量生成。

**启用方式**:
```toml
# DbOption.toml
incr_sync = true
```

### 指定 Refno 模式
适用于调试或部分重建。

```rust
use aios_database::fast_model::gen_geos_data;

// 指定 refno 列表生成
gen_geos_data(refnos, &db_option_ext).await?;
```

## API 使用

### 主入口函数
```rust
use aios_database::fast_model::gen_all_geos_data;

// 全量生成
gen_all_geos_data(vec![], &db_option_ext, None, None).await?;

// 指定 refno 生成
let refnos = vec![RefnoEnum::from(12345u64)];
gen_all_geos_data(refnos, &db_option_ext, Some(100), None).await?;

// 带进度回调
gen_all_geos_data(
    vec![],
    &db_option_ext,
    None,
    Some(Box::new(|progress| println!("Progress: {:.1}%", progress * 100.0))),
).await?;
```

### 网格生成
```rust
use aios_database::fast_model::gen_inst_meshes;

gen_inst_meshes(
    &refnos,
    false,  // replace_exist
    "assets/meshes".into(),
    Arc::new(precision),
).await?;
```

### 布尔运算
```rust
use aios_database::fast_model::booleans_meshes_in_db;

// 执行布尔运算
booleans_meshes_in_db(Some(Arc::new(db_option))).await?;
```

## 调试技巧

### 启用调试输出
```bash
# 启用模型调试
DEBUG_MODEL=true cargo run

# 仅输出错误
DEBUG_MODEL_ERRORS_ONLY=true cargo run

# E3D 调试模式
E3D_DEBUG=true cargo run
```

### 调试宏使用
```rust
use aios_database::fast_model::{debug_model, smart_debug_model};

debug_model!("Processing refno: {}", refno);
smart_debug_model!("Conditional debug: {}", info);
smart_debug_error!("Error occurred: {}", error);
```

### 单个 Refno 调试
```bash
cargo run --example debug_refno_21491_19209
```

## 常见问题

### Q: 网格生成失败
**A**: 检查几何参数是否完整：
```rust
// 查看错误记录
let errors = REFNO_ERROR_STORE.get_errors();
```

### Q: 布尔运算结果异常
**A**: 启用 debug_obj_export 特性导出中间结果：
```bash
cargo run --features debug_obj_export
```

### Q: 内存占用过高
**A**: 减小批处理大小：
```rust
gen_all_geos_data(refnos, &db_option_ext, Some(50), None).await?;
```

## 性能调优

### 批处理大小
```toml
# DbOption.toml
batch_size = 100  # 默认 100
```

### 并发度
```rust
let config = FullNounConfig {
    concurrency: Concurrency::new(num_cpus::get()),
    ..Default::default()
};
```

### 跳过已存在网格
```toml
# DbOption.toml
replace_mesh = false  # 不替换已存在的网格
```
