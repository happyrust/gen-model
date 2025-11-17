# BRAN/HANG Mesh 生成验证指南

本文档说明如何验证 Full Noun 模式下 BRAN/HANG 的 mesh 生成修复是否生效。

## 问题背景

在 Full Noun 模式下，BRAN/HANG 类型的几何体数据正常生成并入库，但 mesh 文件没有生成。

**根因**：批次处理中查询到的 BRAN/HANG 子元素只存储到局部变量 `bran_comp_eles`，没有添加到全局的 `all_bran_children_refnos`，导致后续 mesh 生成阶段无法访问这些子元素。

**修复位置**：`src/fast_model/gen_model_old.rs` 第 1237-1248 行

## 验证流程

### 方法一：自动化测试（推荐）

#### 1. 运行测试脚本

```bash
cd /Volumes/DPC/work/plant-code/gen-model-fork

# 运行完整测试流程（包括编译、测试、验证）
./scripts/test/test_bran_mesh_generation.sh
```

#### 2. 查看测试输出

测试脚本会自动：
- 编译测试用例
- 清理旧的输出目录
- 运行 Full Noun 模式生成
- 统计生成的 mesh 文件
- 验证文件有效性

**预期输出示例**：

```
🚀 测试 BRAN/HANG Full Noun 模式 mesh 生成

📦 步骤 1: 编译测试用例...
✅ 编译完成

🧹 步骤 2: 清理旧的测试输出...
✅ 清理完成

🔨 步骤 3: 运行测试...
[gen_full_noun_geos] 批次 1: 已收集 9 个子元素到全局列表，当前总数: 9
✅ 测试执行成功

📊 统计结果:
   - Mesh 文件总数: 15
   - Mesh 文件列表 (前 10 个):
      * 25688_76336.mesh (234 KB)
      * 25688_76337.mesh (189 KB)
      ...
```

#### 3. 验证 mesh 文件有效性

```bash
# 运行 Python 验证脚本
python3 scripts/test/verify_bran_meshes.py
```

**预期输出示例**：

```
🔍 验证 mesh 文件...
   目录: test_output/full_noun_bran_meshes

📊 找到 15 个 mesh 文件

============================================================
📊 验证摘要
============================================================
总文件数:    15
有效文件:    15
无效文件:    0
总大小:      3,245,678 bytes (3.09 MB)
平均大小:    216,379 bytes (211.31 KB)

✅ 所有 mesh 文件验证通过
```

### 方法二：手动验证

#### 1. 编译项目

```bash
cd /Volumes/DPC/work/plant-code/gen-model-fork
cargo build --release
```

#### 2. 运行测试用例

```bash
cargo run --example test_full_noun_bran_mesh --release
```

#### 3. 检查关键日志

在输出中查找以下关键信息：

**✅ 成功标志**：

```
[gen_full_noun_geos] 批次 1: 已收集 9 个子元素到全局列表，当前总数: 9
[gen_full_noun_geos] 批次 2: 已收集 8 个子元素到全局列表，当前总数: 17
...
   - BRAN/HANG 子元素 refnos: 17
   ✅ BRAN/HANG 子元素收集成功
   
🎨 步骤 4: 生成 mesh 文件...
   - 总 mesh 文件数: 25
   - BRAN/HANG 相关 mesh: 17
   ✅ Mesh 文件生成成功
```

**❌ 失败标志**：

```
   - BRAN/HANG 子元素 refnos: 0
   ⚠️  警告: BRAN/HANG 子元素列表为空！
```

#### 4. 检查文件系统

```bash
# 查看生成的 mesh 文件
ls -lh test_output/full_noun_bran_meshes/

# 统计 mesh 文件数量
find test_output/full_noun_bran_meshes -name "*.mesh" | wc -l

# 查看文件大小分布
find test_output/full_noun_bran_meshes -name "*.mesh" -exec ls -lh {} \; | awk '{print $5}' | sort -h
```

## 验证标准

### ✅ 测试通过条件

1. **子元素收集**：
   - 日志显示 `已收集 N 个子元素到全局列表`
   - `db_refnos.bran_hanger_refnos.len() > 0`

2. **Mesh 文件生成**：
   - `test_output/full_noun_bran_meshes/` 目录存在
   - 目录中包含 `.mesh` 文件
   - mesh 文件数量 >= BRAN/HANG 子元素数量

3. **文件有效性**：
   - 文件大小 > 100 bytes
   - 可以正常读取文件头部

### ❌ 测试失败场景

1. **子元素未收集**：
   - `bran_hanger_refnos` 为空
   - 未看到 `已收集 N 个子元素到全局列表` 日志
   - **原因**：修复代码未正确应用

2. **Mesh 文件未生成**：
   - mesh 目录为空或不存在
   - **原因**：
     - `gen_mesh` 配置未启用
     - mesh 生成流程出错
     - 子元素 refnos 未传递给 mesh 生成函数

3. **数据库无数据**：
   - 子元素收集成功但数量为 0
   - **原因**：
     - 数据库中没有 BRAN/HANG 数据
     - `inst_relate` 表中没有 BRAN/HANG 的子元素关系

## 故障排查

### 问题 1: 子元素列表为空

**检查步骤**：

1. 确认数据库中有 BRAN/HANG 数据：
   ```sql
   SELECT count() FROM pe WHERE noun IN ['BRAN', 'HANG'] GROUP ALL;
   ```

2. 确认 inst_relate 表有子元素关系：
   ```sql
   SELECT count() FROM inst_relate WHERE owner_refno IN (
       SELECT refno FROM pe WHERE noun IN ['BRAN', 'HANG']
   ) GROUP ALL;
   ```

3. 检查修复代码是否已应用：
   ```bash
   grep -n "已收集.*个子元素到全局列表" src/fast_model/gen_model_old.rs
   ```
   应该能找到第 1243 行的代码。

### 问题 2: Mesh 文件未生成

**检查步骤**：

1. 确认配置启用了 mesh 生成：
   ```bash
   grep "gen_mesh" DbOption.toml
   ```
   应该显示 `gen_mesh = true`

2. 检查 mesh 目录权限：
   ```bash
   ls -ld test_output/full_noun_bran_meshes
   ```

3. 查看详细错误日志：
   ```bash
   RUST_LOG=debug cargo run --example test_full_noun_bran_mesh --release 2>&1 | tee test.log
   ```

### 问题 3: 文件生成但无效

**检查步骤**：

1. 查看文件内容：
   ```bash
   hexdump -C test_output/full_noun_bran_meshes/lod_0/*.mesh | head -20
   ```

2. 检查文件大小：
   ```bash
   find test_output/full_noun_bran_meshes -name "*.mesh" -size 0
   ```
   不应该有大小为 0 的文件。

## 代码修复说明

### 修复位置

文件：`src/fast_model/gen_model_old.rs`

函数：`gen_full_noun_geos`

位置：第 1237-1248 行

### 修复前代码

```rust
for &refno in chunk {
    match aios_core::collect_children_elements(refno, &[]).await {
        Ok(children) => {
            bran_comp_eles.extend(children.iter().map(|x| x.refno));  // ❌ 只存局部
            branch_refnos_map.insert(refno, children);
        }
        // ...
    }
}

println!(
    "[gen_full_noun_geos] 批次 {}: 查询到 {} 个 BRAN/HANG，{} 个子元素",
    batch_num,
    branch_refnos_map.len(),
    bran_comp_eles.len()
);
// ❌ 未收集到全局，bran_comp_eles 在循环结束后被丢弃
```

### 修复后代码

```rust
for &refno in chunk {
    match aios_core::collect_children_elements(refno, &[]).await {
        Ok(children) => {
            bran_comp_eles.extend(children.iter().map(|x| x.refno));
            branch_refnos_map.insert(refno, children);
        }
        // ...
    }
}

println!(
    "[gen_full_noun_geos] 批次 {}: 查询到 {} 个 BRAN/HANG，{} 个子元素",
    batch_num,
    branch_refnos_map.len(),
    bran_comp_eles.len()
);

// ✅ 关键修复：将批次中的子元素收集到全局 all_bran_children_refnos
if !bran_comp_eles.is_empty() {
    let mut sink = all_bran_children_refnos.write().await;
    sink.extend(bran_comp_eles.iter().copied());
    println!(
        "[gen_full_noun_geos] 批次 {}: 已收集 {} 个子元素到全局列表，当前总数: {}",
        batch_num,
        bran_comp_eles.len(),
        sink.len()
    );
}
```

### 修复原理

1. **全局收集**：将每个批次查询到的子元素添加到 `all_bran_children_refnos`
2. **持久化**：确保所有子元素在批次处理结束后仍然可访问
3. **传递给 mesh 生成**：`DbModelInstRefnos::execute_gen_inst_meshes` 使用 `bran_hanger_refnos` 生成 mesh

## 相关文件

- 测试用例：`examples/test_full_noun_bran_mesh.rs`
- 测试脚本：`scripts/test/test_bran_mesh_generation.sh`
- 验证脚本：`scripts/test/verify_bran_meshes.py`
- 修复代码：`src/fast_model/gen_model_old.rs` (第 1237-1248 行)

## 参考文档

- Full Noun 模式：`docs/architecture/FULL_NOUN_MODE_DESIGN.md`
- BRAN/HANG 实现：系统检索记忆 `b03a1469-01f2-4c6c-b8ae-f187b562e411`
