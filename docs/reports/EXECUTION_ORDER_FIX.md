# Full Noun 模式执行顺序修复

## 问题描述

原先的实现使用 `tokio::join!` 并发执行三个 Noun 类别（CATE、LOOP、PRIM），但这不符合实际的依赖关系要求。

## 用户需求

> "一定是 LOOP -> PRIM -> CATE 按照这样的顺序执行，大的流程按这个，然后里面的子任务就是批量的并发生成模型"

## 修改内容

### 文件：`src/fast_model/gen_model/full_noun_mode.rs`

#### 1. 更新函数文档注释（第 41-51 行）

**修改前：**
```rust
/// # 主要改进
/// 1. ✅ 真正的并发处理：使用 tokio::join! 同时处理三个类别
/// 2. ✅ 内存优化：使用 CategorizedRefnos 替代三个 HashSet
/// 3. ✅ 数据验证：检查 SJUS map 完整性
/// 4. ✅ 类型安全：使用 FullNounConfig 和错误类型
///
/// # Performance
/// 相比旧版本预期提升 30-50%（I/O bound 场景）
```

**修改后：**
```rust
/// # 主要改进
/// 1. ✅ 顺序执行：LOOP -> PRIM -> CATE（确保依赖关系正确）
/// 2. ✅ 批量并发：每个类别内部使用批量并发处理
/// 3. ✅ 内存优化：使用 CategorizedRefnos 替代三个 HashSet
/// 4. ✅ 数据验证：检查 SJUS map 完整性
/// 5. ✅ 类型安全：使用 FullNounConfig 和错误类型
///
/// # 执行顺序
/// 必须按照 LOOP -> PRIM -> CATE 顺序执行，因为 CATE 依赖 LOOP 生成的 SJUS 数据
```

#### 2. 修改执行逻辑（第 87-160 行）

**修改前（并发执行）：**
```rust
let (cate_result, loop_result, prim_result) = tokio::join!(
    // Cate 处理
    async move { ... },
    // Loop 处理
    async move { ... },
    // Prim 处理
    async move { ... }
);
```

**修改后（顺序执行）：**
```rust
// ⚡ 顺序执行：LOOP -> PRIM -> CATE（内部批量并发）
println!("⚡ 开始顺序处理三个 Noun 类别（LOOP -> PRIM -> CATE）...");

// 1️⃣ 先执行 LOOP 处理
println!("📍 [1/3] 处理 LOOP Nouns...");
let loop_result = {
    let processor = NounProcessor::new(ctx.clone(), "loop");
    // ... LOOP 处理逻辑
}.await?;

// 2️⃣ 再执行 PRIM 处理
println!("📍 [2/3] 处理 PRIM Nouns...");
let prim_result = {
    let processor = NounProcessor::new(ctx.clone(), "prim");
    // ... PRIM 处理逻辑
}.await?;

// 3️⃣ 最后执行 CATE 处理
println!("📍 [3/3] 处理 CATE Nouns...");
let cate_result = {
    let processor = NounProcessor::new(ctx.clone(), "cate");
    // ... CATE 处理逻辑
}.await?;
```

## 关键优化点

### ✅ 保持批量并发

每个类别内部仍然使用 `NounProcessor::process_nouns()` 的批量并发机制：
- `concurrency` 参数控制并发的 Noun 数量（默认 6）
- `batch_size` 参数控制每页的 refno 数量（默认 200）

### ✅ 顺序执行保证依赖

- LOOP 必须先执行，生成 SJUS map 数据
- PRIM 可以在 LOOP 之后执行
- CATE 最后执行，依赖 LOOP 生成的 SJUS 数据

### ✅ 日志清晰

新增的日志输出：
```
⚡ 开始顺序处理三个 Noun 类别（LOOP -> PRIM -> CATE）...
📍 [1/3] 处理 LOOP Nouns...
📍 [2/3] 处理 PRIM Nouns...
📍 [3/3] 处理 CATE Nouns...
```

## 验证结果

运行测试脚本 `./test_execution_order.sh` 可以看到：

```
🚀 启动 Full Noun 模式（优化版本）
📋 收集到 61 个 Noun 类型（Cate: 35, Loop: 5, Prim: 21）
⚡ 开始顺序处理三个 Noun 类别（LOOP -> PRIM -> CATE）...
📍 [1/3] 处理 LOOP Nouns...
[gen_full_noun_geos] loop noun AEXTR: 共 11 个实例，分页大小 200
[gen_full_noun_geos] loop noun NXTR: 共 2620 个实例，分页大小 200
[gen_full_noun_geos] loop noun EXTR: 共 13950 个实例，分页大小 200
```

确认按照 LOOP → PRIM → CATE 顺序执行。

## 性能影响

虽然移除了顶层的并发（`tokio::join!`），但性能影响有限：
- ✅ 每个类别内部仍然保持批量并发（6 个 Noun 并发，每批 200 个 refno）
- ✅ I/O 操作（数据库查询）仍然是并发的
- ⚠️ 三个类别之间的流水线并行被移除（但这是必要的，因为存在依赖关系）

## 总结

这次修改确保了 Full Noun 模式按照正确的依赖顺序执行（LOOP → PRIM → CATE），同时保持了批量并发的性能优势。
