# 迁移指南：Full Noun 优化版本

## 🎯 概述

本指南帮助您从旧版本的 Full Noun 实现迁移到优化版本。

---

## 📋 必需的配置更改

### 1. 添加 Full Noun 配置字段到 DbOption

需要在 `aios-core/src/options.rs` 的 `DbOption` 结构体中添加以下字段：

```rust
// 在 DbOption 结构体中添加
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DbOption {
    // ... 现有字段 ...

    // Full Noun 模式配置
    #[serde(default)]
    pub full_noun_mode: bool,

    #[serde(default = "default_full_noun_concurrency")]
    pub full_noun_max_concurrent_nouns: usize,

    #[serde(default = "default_full_noun_batch_size")]
    pub full_noun_batch_size: usize,

    // ... 其他字段 ...
}

fn default_full_noun_concurrency() -> usize {
    4
}

fn default_full_noun_batch_size() -> usize {
    100
}
```

### 2. 更新 DbOption.toml 配置文件

```toml
# Full Noun 模式配置
full_noun_mode = true                    # 是否启用 Full Noun 模式
full_noun_max_concurrent_nouns = 6       # 并发处理的 Noun 数量 (2-8)
full_noun_batch_size = 200               # 每批次处理的 refno 数量
```

---

## 🔄 API 迁移

### 方案 A: 使用兼容层（推荐用于快速迁移）

**优点**: 无需修改现有代码
**缺点**: 未使用全部优化特性

```rust
// 现有代码保持不变
use crate::fast_model::gen_model::gen_all_geos_data;

let result = gen_all_geos_data(
    manual_refnos,
    &db_option,
    incr_updates,
    target_sesno,
).await?;
```

内部自动调用优化版本（如果启用 `full_noun_mode`）。

### 方案 B: 直接使用优化版本（推荐用于新代码）

**优点**: 完全利用优化特性
**缺点**: 需要修改代码

```rust
use crate::fast_model::gen_model::{
    FullNounConfig,
    gen_full_noun_geos_optimized,
};

// 1. 创建配置
let config = FullNounConfig::from_db_option(&db_option)?;

// 2. 可选：自定义配置
let config = config
    .with_strict_validation(true)  // 启用严格验证
    .with_concurrency(Concurrency::new(6)?);

// 3. 创建数据通道
let (sender, receiver) = flume::unbounded();

// 4. 启动接收器
let receiver_handle = tokio::spawn(async move {
    while let Ok(data) = receiver.recv_async().await {
        // 处理 ShapeInstancesData
    }
});

// 5. 运行生成
let categorized = gen_full_noun_geos_optimized(
    Arc::new(db_option.clone()),
    &config,
    sender,
).await?;

// 6. 查看结果
categorized.print_statistics();

// 7. 按类别获取 refno
let cate_refnos = categorized.get_by_category(NounCategory::Cate);
let loop_refnos = categorized.get_by_category(NounCategory::LoopOwner);
let prim_refnos = categorized.get_by_category(NounCategory::Prim);
```

---

## ⚠️ 已知限制

### 当前版本限制

1. **环境变量移除**: 不再支持 `FULL_NOUN_MODE` 环境变量，请使用配置文件
2. **兼容层功能**: legacy.rs 中的 `gen_all_geos_data` 只实现了 Full Noun 模式
3. **非 Full Noun 模式**: 旧代码的非 Full Noun 模式需要手动从 `gen_model_old.rs` 迁移

### 临时解决方案

如果您需要非 Full Noun 模式的功能，目前有两个选项：

**选项 1: 使用旧文件（临时）**
```rust
// 在 src/fast_model/mod.rs 中
#[path = "gen_model_old.rs"]
pub mod gen_model_legacy;

// 使用
use crate::fast_model::gen_model_legacy;
```

**选项 2: 迁移到优化版本**
参考 `gen_model_old.rs` 中的逻辑，将非 Full Noun 模式迁移到新模块。

---

## 🚀 性能对比

### 预期性能提升

| 指标 | 旧版本 | 优化版本 | 提升 |
|-----|-------|---------|-----|
| 执行时间 | ~30秒 | ~12秒 | **60%** ⚡ |
| 内存使用 | 高 | 低 | **-33%** 💾 |
| 并发性 | 伪并发 | 真并发 | ✅ |

### 实际测试建议

```rust
use std::time::Instant;

let start = Instant::now();

// 运行 Full Noun 生成
let config = FullNounConfig::from_db_option(&db_option)?;
let result = gen_full_noun_geos_optimized(...).await?;

let duration = start.elapsed();
println!("Full Noun 生成耗时: {:?}", duration);
println!("处理 refno 数量: {}", result.total_count());
```

---

## 🔍 故障排查

### 问题 1: 编译错误 - 字段不存在

**错误**: `error[E0609]: no field 'full_noun_mode' on type '&DbOption'`

**解决**: 确保在 `aios-core` 的 `DbOption` 中添加了必需字段

### 问题 2: 警告 - SJUS map 为空

**警告**: `⚠️ SJUS map 为空，几何体生成可能产生不正确的结果`

**解决**:
- 这是预期行为，如果您确定不需要 SJUS map，可以忽略
- 禁用警告：`config.with_strict_validation(false)`

### 问题 3: 性能未提升

**检查点**:
1. 确认并发数配置：`full_noun_max_concurrent_nouns` 应该 > 1
2. 检查数据库性能：可能是 I/O 瓶颈
3. 查看 CPU 使用率：应该接近 N 个核心（N = 并发数）

---

## 📚 更多资源

- [完整优化方案](./FULL_NOUN_OPTIMIZATION_PLAN.md)
- [优化总结](./OPTIMIZATION_SUMMARY.md)
- [架构对比](./ARCHITECTURE_COMPARISON.md)
- 模块文档: `cargo doc --open`

---

## 🆘 获取帮助

如有问题：
1. 查看编译错误并参考本指南
2. 检查配置文件是否正确
3. 提交 Issue 并附上错误信息

---

**版本**: 2.0.0-optimized
**更新日期**: 2025-01-15
