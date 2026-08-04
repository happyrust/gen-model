# 结构专业 GENSEC 按需生成与增量路由测试报告

日期：2026-07-25

## 结论

- 当前样本库中 398 个 GENSEC 均来自 dbnum 8000，dbnum 7997 没有 GENSEC，故本轮使用真实 8000 数据，未伪造 7997 通过结果。
- GENSEC 的生成根正确归一到 `SUPPO`；修改 GENSEC 或其 `SPINE` 子构件时，不会把 GENSEC/FTUB 错当成最小交付单元。
- 修复前，真实圆角 SPRO 在 OCC 网格化阶段报错 `encountered a face with no triangulation`。
- 修复后，BEAM、BOX 两种 GTYP 以及 3 种不同 SPRE 的真实 GENSEC 均已产生 `inst_relate`、`inst_info`、有限 AABB 和可用 mesh。
- 首次遇到未解析 SPRE 时，开启默认 CATA 闭包可以自动解析依赖；关闭闭包时失败属于预期的“目录尚未加载”，不是伪通过。

## 真实样本

| GTYP | GENSEC | 名称 | SPRE | 生成根 | 结果 |
|---|---|---|---|---|---|
| BEAM | `24384/25743` | `/6KA02-MSUP-E0090-V1` | `21438/3316` | `SUPPO 24384/25725` | 修复前不可三角化；修复后通过 |
| BOX | `24384/25923` | `/6KA02-MSUP-E0020-V6` | `21438/3247` | `SUPPO 24384/25872` | 首次自动解析 236 个 CATA 依赖后通过 |
| BEAM | `24384/25888` | `/6KA02-MSUP-E0020-V1` | `21438/3276` | `SUPPO 24384/25872` | 通过 |
| BEAM | `24384/29771` | `/6KA02-MSUP-E0035/STRU-BAR-1` | `21438/3277` | `SUPPO 24384/29765` | 首次自动解析 233 个 CATA 依赖后通过 |

库内分布：

- `GTYP=BEAM`：390
- `GTYP=BOX`：8
- SPRE：`21438/3316`、`21438/3315`、`21438/3277`、`21438/3276`、`21438/3247`

## 根因与修复

### 1. FRADIUS 轮廓不可三角化

真实 `24384/25743` 使用 20 点 SPRO，包含 16 个非零 FRADIUS。新接入的
`rust-ploop-processor` 处理后又在 `convert_vertices_to_polyline` 中把 FRADIUS
转换为 bulge，OCC 可以创建 shape，但其中存在无法三角化的 face。

修复：OCC wire 恢复使用项目已有的 `gen_polyline_original` 圆角算法。真实参数回归
测试由红转绿。

### 2. 直线 SPINE 端面方向误判

GENSEC 直线 SPINE 的外法向是起点 `-Z`、终点 `+Z`。旧逻辑把起点 `-Z`
当作斜切端面，导致不必要的端面变换和 mesh hash 分支。

修复：

- 起点以 `-Z` 为标准法向；
- 非斜切恒截面直线使用 OCC 原生 extrusion，不再用两个近似重合截面 loft；
- 仅对真正的斜切端面应用端面矩阵。

### 3. `SweepSolid::default()` 无限递归

原实现通过 `..Default::default()` 构造自身，会造成栈溢出。已改为逐字段显式默认值。

### 4. 隐含结构子元素与生成根

- 保留 SPINE/POINSP/CURVE、PAVE/VERT 缺失公共 `pe` 行时的 noun 表回退。
- GENSEC、SPINE、FRMW 均沿 owner 链归一到最近的 `SUPPO`。
- `GTYP`、`SPRE`、`BANG`、`DRNS`、`DRNE`、`FRAD` 归类为几何重生成；
  `POS` 为仅变换更新；`NAME` 为模型树元数据更新，不重建 mesh。

## 数据库证据

最终 3 个代表样本均存在实例关系和 AABB：

```text
inst_relate:24384_25743 -> inst_info:24384_25743_21
aabb:12178125128990152755

inst_relate:24384_25923 -> inst_info:24384_25923_21
aabb:10630235562449433495

inst_relate:24384_29771 -> inst_info:24384_29771_21
aabb:7976020965884347107
```

首个修复样本生成日志：

```text
实际需要更新模型结点数量: 21
GLOBAL_AABB_TREE: 7
生成完所有模型时间: 8873ms
```

BOX 首次自动解析日志：

```text
[cata_closure] 按需预加载完成: parsed=236 missing=0
实际需要更新模型结点数量: 23
GLOBAL_AABB_TREE: 12
```

## 自动化测试

```powershell
cargo test gensec_straight_spro_can_be_triangulated --lib
cargo test structural_ --lib

set AIOS_CATA_CLOSURE_MODE=on
set AIOS_ON_DEMAND_TEST_REFNO=24384/25923
cargo test live_generates_a_missing_model --lib -- --ignored --nocapture
```

结果：

- GENSEC 真实 SPRO/OCC 回归：1 passed
- 结构生成根、FLOOR 极端圆角、增量属性路由：3 passed
- 真实 Surreal 按需生成：BEAM/BOX/SPRE 变体通过

## UI 验收限制

本轮已完成服务端、Surreal 实例/AABB/mesh 和增量路由验证。当前桌面捕获仍受
Computer Use monitor capture interop 错误阻塞，因此没有伪造或声称已取得
rs-plant-3d 三维前后截图。捕获恢复后，应在 E3D 对同一 GENSEC 分别修改：

- `POS`：验证仅实例变换和三维位置变化；
- `SPRE` 或 SPRO `FRAD`：验证旧 mesh 被替换、AABB/三维外形变化；
- `NAME`：验证模型树名称变化而 mesh hash 不变；
- 新增/删除同一 SUPPO 下的 GENSEC：验证模型树和三维实例同步增删。

