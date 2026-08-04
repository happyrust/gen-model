# 案例 17 · 结构专业三连：FLOOR 隐含子元素、GENSEC 圆角与端面

<sub>族 E 解析与按需 · High · 已修 · 证据层 B（回归单测）+ C（8000 库真实 GENSEC）</sub>

## 一句话

结构专业把三类问题一次性暴露出来：**隐含子元素没有通用 `pe` 行**、**真实圆角轮廓 OCC 三角化失败**、
**直线 SPINE 的端面法向被误判**——三者都表现为「按需生成走完流程但拿不到可用几何」。

## 一 · FLOOR：隐含子元素与缺失 owner

**现象**：FLOOR 的按需生成拿不到轮廓参数，或者 owner 查询直接失败后被上层吞成空集合。

**根因三条**：

1. 部分结构元素的 owner `pe` 行缺失，`[noun, owner.noun]` 反序列化失败 → 被上层吞成空集合；
2. PAVE / VERT 等**隐含子元素只有 noun 表记录，没有通用 `pe` 行**；
3. 样本 PAVE 的 `FRAD = 24450` 与边长相等（极限圆角），需要保证轮廓、OCC 近似和最终 AABB
   **均为有限值**——否则 AABB 会算出 inf / NaN。

**修法**：owner noun 查询用空字符串兜底；PAVE / VERT / POINSP / CURVE 支持**从 noun 表读取隐含元素**；
结构轮廓参数查询支持从 PAVE / VERT 表回退；新增「极限圆角的轮廓 → PlantMesh」回归测试。

**验证**：`fast_model::loop_model::tests::structural_floor_extreme_fillet_remains_finite`（1 passed）。

## 二 · GENSEC：三个独立缺陷

出处 [`../../docs/2026-07-25_test-structure-gensec-on-demand-report.md`](../../docs/2026-07-25_test-structure-gensec-on-demand-report.md)。

**前置事实**：样本库里 398 个 GENSEC **全部来自 dbnum 8000**，7997 里一个都没有。
因此本轮用的是真实 8000 数据，**没有伪造 7997 通过结果**——这条纪律与「`SUPPO` 在 7997 数量为 0，
不能跳过后记为通过」是同一条。

### 2.1 FRADIUS 轮廓不可三角化

真实 `24384/25743` 用的是 20 点 SPRO，含 **16 个非零 FRADIUS**。新接入的 `rust-ploop-processor`
处理后又在 `convert_vertices_to_polyline` 里把 FRADIUS 转成 bulge，OCC 能创建 shape，
但其中存在**无法三角化的 face**，网格化阶段报 `encountered a face with no triangulation`。

**修法**：OCC wire 恢复使用项目已有的 `gen_polyline_original` 圆角算法。真实参数回归测试由红转绿。

### 2.2 直线 SPINE 端面方向误判

GENSEC 直线 SPINE 的外法向是**起点 `-Z`、终点 `+Z`**。旧逻辑把起点的 `-Z` 当作斜切端面，
导致不必要的端面变换和 mesh hash 分支。

**修法**：起点以 `-Z` 为标准法向；非斜切恒截面直线**改用 OCC 原生 extrusion**，
不再用两个近似重合截面做 loft；仅对**真正的**斜切端面应用端面矩阵。

### 2.3 `SweepSolid::default()` 无限递归

原实现通过 `..Default::default()` 构造自身 → 栈溢出。改为逐字段显式默认值。

### 2.4 隐含结构子元素与生成根

- 保留 SPINE / POINSP / CURVE、PAVE / VERT 缺失公共 `pe` 行时的 noun 表回退（与 FLOOR 同一修法）；
- GENSEC、SPINE、FRMW 均沿 owner 链**归一到最近的 `SUPPO`**——修改 GENSEC 或其 SPINE 子构件时，
  不会把 GENSEC / FTUB 错当成最小交付单元；
- 属性路由：`GTYP` `SPRE` `BANG` `DRNS` `DRNE` `FRAD` → 几何重生成；`POS` → 仅变换更新；
  `NAME` → 模型树元数据更新，不重建 mesh。

## 验证（真实样本）

| GTYP | GENSEC | 名称 | SPRE | 生成根 | 结果 |
|---|---|---|---|---|---|
| BEAM | `24384/25743` | `/6KA02-MSUP-E0090-V1` | `21438/3316` | `SUPPO 24384/25725` | 修复前不可三角化；修复后通过 |
| BOX | `24384/25923` | `/6KA02-MSUP-E0020-V6` | `21438/3247` | `SUPPO 24384/25872` | 首次自动解析 236 个 CATA 依赖后通过 |
| BEAM | `24384/25888` | `/6KA02-MSUP-E0020-V1` | `21438/3276` | `SUPPO 24384/25872` | 通过 |
| BEAM | `24384/29771` | `/6KA02-MSUP-E0035/STRU-BAR-1` | `21438/3277` | `SUPPO 24384/29765` | 首次自动解析 233 个 CATA 依赖后通过 |

数据库证据（三个代表样本均有实例关系与有限 AABB）：

```text
inst_relate:24384_25743 -> inst_info:24384_25743_21   aabb:12178125128990152755
inst_relate:24384_25923 -> inst_info:24384_25923_21   aabb:10630235562449433495
inst_relate:24384_29771 -> inst_info:24384_29771_21   aabb:7976020965884347107
```

生成日志：

```text
实际需要更新模型结点数量: 21
GLOBAL_AABB_TREE: 7
生成完所有模型时间: 8873ms

[cata_closure] 按需预加载完成: parsed=236 missing=0      # BOX 首次自动解析
实际需要更新模型结点数量: 23
GLOBAL_AABB_TREE: 12
```

一条重要的口径：**首次遇到未解析 SPRE 时，开启默认 CATA 闭包可以自动解析依赖；
关闭闭包时失败属于预期的「目录尚未加载」，不是伪通过。**

**UI 验收限制**：桌面捕获仍受 monitor capture interop 错误阻塞，因此没有伪造或声称已取得
`rs-plant-3d` 三维前后截图。

## 规律

**「有 noun 记录但没有通用 `pe` 行」是这类数据里的常态，不是异常。** 隐含子元素（PAVE / VERT /
SPINE / POINSP / CURVE）由父元素的几何定义带出来，本来就不是独立元素。任何按 `pe` 表查子元素的代码
都要准备好回退到 noun 表——否则表现是「查询失败被吞成空集合」，比报错更难查。

**真实参数会打败合成测试。** 圆角不可三角化、`FRAD` 恰好等于边长、20 点里 16 个非零 FRADIUS——
这些都不是随手造的测试数据会覆盖到的形态。回归测试必须**用真实样本的参数**建，
而不是用「看起来合理」的参数。

**没有样本就明说没有样本。** GENSEC 只在 8000 库、`SUPPO` 在 7997 数量为 0——遇到这种情况
要么换库测，要么记为「不适用」，**跳过不等于通过**。

## 关联

- [`../../docs/2026-07-25_test-structure-gensec-on-demand-report.md`](../../docs/2026-07-25_test-structure-gensec-on-demand-report.md)
- [`../../docs/2026-07-25_test-structure-floor-wall-incremental-report.md`](../../docs/2026-07-25_test-structure-floor-wall-incremental-report.md)
- 案例 [16 WALL 闭包漏 GMSS](case-16-wall-cata-closure-missed-gmss.md) · [04 生成根归一](case-04-generation-root-must-be-one-rule.md)
