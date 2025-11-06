# CTorus Mesh 生成问题分析报告

## 一、日志文件位置

### 1.1 默认日志配置
- **配置文件**: `DbOption.toml`
- **日志开关**: `enable_log = false` (默认关闭)
- **日志文件位置**: 如果启用，会在项目根目录生成格式为 `{year}-{month}-{day}-{hour}-{minute}-{second}_dblog.txt` 的文件

### 1.2 当前测试日志
- **终端输出**: 命令执行时的输出直接显示在终端
- **编译日志**: `model_gen_debug.log` (包含编译警告)
- **运行时日志**: 测试运行时的详细输出已显示在终端

### 1.3 启用日志的方法
在 `DbOption.toml` 中设置：
```toml
enable_log = true
```

## 二、CTorus 未生成 Mesh 的原因分析

### 2.1 问题定位

从测试输出可以看到：
```
gen mesh param: PrimCTorus(CTorus { rins: 0.8867925, rout: 1.0, angle: 180.0 })
[WARN] [Refno(21491_18957)] CSG mesh generation not supported for type: PrimCTorus
```

### 2.2 代码流程分析

#### 步骤 1: Mesh 生成入口
```rust
// src/fast_model/mesh_generate.rs:489
match generate_csg_mesh(&g.param, &profile.csg_settings, non_scalable_geo) {
    Some(csg_mesh) => {
        // 成功生成 mesh
    }
    None => {
        // CSG 生成失败
        debug_model_warn!(
            "{:?} CSG mesh generation not supported for type: {}",
            failed_refnos,
            geo_type_name
        );
        // 标记 bad，避免后续重复尝试
        update_sql.push_str(&format!("update inst_geo:⟨{}⟩ set bad=true;", mesh_id));
    }
}
```

#### 步骤 2: CSG 生成函数
- **函数位置**: `aios_core::geometry::csg::generate_csg_mesh`
- **返回值**: 
  - `Some(GeneratedMesh)` - 成功生成
  - `None` - 不支持该类型

### 2.3 根本原因

根据 `docs/csg_primitive_migration_plan.md` 文档：

> **3. 实现迭代**
> 1. **圆柱族**（含 LCylinder/SCylinder）：解析生成 + LOD 控制，接入 `gen_inst_meshes`。
> 2. **球体/球冠**：经纬网格生成，支持 Dish 类与 LOD 自适应。
> 3. **Snout/圆台**：处理上下半径、偏移，保留弯管后续迭代。
> **4. 扩展至 Pyramid、Dish、Torus 等体，逐步减少 OCC 依赖。**

**结论**: **CTorus (Circular Torus) 目前尚未实现 CSG mesh 生成功能**，这是计划中的后续扩展项。

### 2.4 当前支持的基本体类型

根据代码和文档，当前已实现 CSG 生成的基本体：
- ✅ **LCylinder** / **SCylinder** (圆柱体)
- ✅ **Box** (长方体)
- ✅ **Dish** (碟形体) - 部分支持
- ❌ **CTorus** (圆环体) - **未实现**
- ❌ **Pyramid** (棱锥体) - 未实现
- ❌ **RTorus** (矩形环面体) - 未实现

### 2.5 影响

1. **几何体已生成**: CTorus 的几何数据（brep shape）已成功生成并保存到数据库
2. **Mesh 未生成**: 由于 CSG 生成不支持，无法生成三角网格文件
3. **标记为 bad**: 数据库中的 `inst_geo` 记录被标记为 `bad=true`，避免重复尝试
4. **可视化影响**: 没有 mesh 文件会导致该几何体无法在可视化中显示

## 三、解决方案

### 方案 1: 等待实现（推荐）
根据项目计划，CTorus 将在后续版本中实现 CSG 生成。

### 方案 2: 使用 OCC Fallback（如果可用）
如果项目启用了 `occ` feature，可以尝试：
1. 检查是否有 OCC fallback 机制
2. 查看是否可以通过配置启用 OCC 生成

### 方案 3: 临时绕过
对于需要立即可视化的场景：
1. 可以手动实现 CTorus 的 CSG 生成
2. 或者使用近似几何体（如多个 Cylinder 组合）替代

## 四、相关文件

- **代码位置**: 
  - `src/fast_model/mesh_generate.rs:489` - Mesh 生成逻辑
  - `aios_core::geometry::csg::generate_csg_mesh` - CSG 生成函数
  
- **文档位置**:
  - `docs/csg_primitive_migration_plan.md` - CSG 迁移计划
  - `开发文档/gen-model生成模型流程分析.md` - 模型生成流程

- **测试日志**:
  - 终端输出（最新测试）
  - `model_gen_debug.log` (编译日志)

## 五、建议

1. **短期**: 记录需要 CTorus 支持的具体用例，以便实现时优先处理
2. **中期**: 按照迁移计划实现 CTorus 的 CSG 生成
3. **长期**: 完成所有基本体的 CSG 支持，减少对 OCC 的依赖










