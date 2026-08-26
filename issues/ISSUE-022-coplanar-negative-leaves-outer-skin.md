# Issue #022: 负体出口面与目标外表面共面时，布尔留下一层外皮

## 📋 Issue 信息

- **Issue ID**: #022
- **标题**: NXTR 穿透洞的侧壁与过梁都切出来了，正对着的那张外表面没被挖开
- **类型**: Bug 🐛
- **优先级**: High 🟠
- **状态**: Fixed ✅
- **创建日期**: 2026-08-25
- **解决日期**: 2026-08-25
- **相关模块**: `fast_model/manifold_bool.rs`（`apply_insts_boolean_manifold`）、负体几何生成

## 🔍 问题描述

`rvm_baseline::mesh_compare::mesh_wall_live::mesh_gwall_extra_against_cwall_union` 长期红在
`pe:17496_105828`（GWALL，`1RS-WF03-W-C-RR001`）：gen→E3D GWALL union **p95 = 753.9mm、
max = 1296.9mm**，门槛 12mm。2026-08-25 定位完成——不是摆位、不是尺寸、不是弦高，
是**一张本该被挖掉的外表面还在**。

### 复现步骤

1. 起 `db_options/DbOption-rvm-rebuild`（8009 = `.surreal/ams-rvm-rebuild-20260824`），
   按 `docs/2026-08-12_live-test-ledger.md` 2026-08-25 那条的前置把 1112 生成出来。
2. `cargo test --lib --no-default-features --features ws,gen_model,manifold,project_hd,rvm_verify
   mesh_gwall_extra -- --ignored --nocapture`

### 预期行为

E3D 侧 GWALL 18 在 x∈[−1300, 1300]、z ≤ 2160 的外表面（y ≈ −16651）**没有三角形**——
那是一个 2600 × 2180 的穿透洞。

### 实际行为

gen 侧同一块区域**满是三角形**。逐三角形对照（世界系，三角形心）：

| | y ≈ −16651 的外表面三角形（|x| < 1400） |
|---|---|
| E3D GWALL 18 | 4 个，全在洞顶之上或洞外：`x=−433 z=2420`、`x=433 z=2290`、`x=1380 z=2301`、`x=1386 z=1147` |
| gen 105828 | 10 个，`z` 从 −20 一路铺到 2290，`x` 覆盖 −482…1377 |

采样点距离随之：最差点 `[−3, −16651, 96] d=1296.9`、`[3, −16651, 524] d=1296.8`——
1296.9 ≈ 洞半宽 1300（洞内一点到最近洞口边缘的距离），p95 = 753.9 ≈ 墙厚 748
（外皮上一点到墙另一侧的距离）。

## 🔬 问题分析

### 根本原因

**洞切了，只是出口那张面没开。** 证据是 gen 网格里洞的其它面**都在**：

- 两侧门垛：`[−1300, −17018.07, 1433]` / `[−1300, −16834.74, 707]` 与 x=+1300 的一对，
  跨越整个墙厚，只可能来自减去负体；
- 洞顶（过梁底面）：`[−433, −17018.07, 2160]`、`[385, −16834.74, 2160]`、`[1252, −16834.74, 2160]`，
  z 恰是负体顶面 2160。

E3D 侧同样有这几张面（`[−1300, −17018.04, 1433]`、`[−433, −17018.04, 2160]` …），
两边逐面对得上。**唯一的差别就是出口那张外表面。**

负体 `pe:17496_105841`（NXTR，`HEIG = 750`）的世界 AABB 是
`[−1300.0011, −17401.4, −20] .. [1299.9989, −16651.4, 2160]`，而 gen 墙体外表面所在平面是
**y = −16651.40 / −16651.39**（f32 在 1.6e4 量级的 ulp 约 0.001mm，两者已经分不开）。
也就是说**负体的出口面与墙的外表面共面**，减完之后那张面原样留下——
就是 ADR-044 那句「共面留一层壁」，只不过这次留下的是一层零厚度外皮。

`src/fast_model/manifold_bool.rs` 里没有任何沿挤出轴给负体加余量的处理
（`rg 'NEG_EXTEND|neg_extend|外扩'` 全仓无命中）：负体按原尺寸喂进布尔，
出口面正好落在目标表面上时结果就取决于布尔库怎么裁决共面，而不是取决于设计意图。

### 影响范围

- 直接后果：这堵墙的洞在下游看不见（可视化、碰撞、房间归属都会当它是实心）。
- 同族风险：任何「负体长度正好等于被穿透件厚度」的穿透洞都在这条线上。本轮
  1112 + 8000 里 `neg_relate` 共 **480 行**，还没逐行筛过有多少是共面出口。
- 反例（说明不是全局失效）：同一堵墙的 `pe:17496_105847`（`HEIG = 200`，
  只挖内侧凹槽，出口面在墙体内部、不与任何表面共面）切得干净；
  `pe:17496_105880`（p95 = 9.4）与 `pe:17496_116569`（p95 = 147.4）都在各自门内。

### 相关代码

- `src/fast_model/manifold_bool.rs`：`apply_insts_boolean_manifold` / `_single`
- `src/rvm_baseline/mesh_compare.rs`：`mesh_gwall_extra_against_cwall_union`（门）、
  `gen_world_mesh`（读侧口径）

## 🛠️ 解决方案

### libgm 的口径（2026-08-25 IDA 已查，见 `docs/evidence/2026-08-25-ida-libgm-coincidence-tolerances.md`）

ε 不用拍脑袋，E3D 自己写着：Core3D 建体前（`0x104da260`，MTR 标签
`adp_geometry/adp_gm_mk_body`；另一处 `0x108e6a80` 同值）连着调四次——

```c
gm_SetResolutionTolerance(0.051);           // → GM_User::restol_
gm_SetDefaultNormalisationTolerance(0.051); // → GM_User::normtol_
gm_SetDefaultTangentTolerance(0.0087266);   // 0.5°
gm_SetDefaultFacetTolerance(0.5);           // → GM_User::arctol_，本仓 FACET_TOL_MM 就是它
```

**0.051mm 是 libgm 的「多近算同一个」**，而 0.5 那个我们已经从同一处抄过来了。

更关键的是 E3D 的负体**不是三维实体 CSG**：`CSG_PrimitiveUtilities::addStandAloneNegative`
建 `gm_CreateCombination(3)`（3 = 相减），`GM_AggregateCombination::calcFacets` 把
`restol_` 一路传给 `GM_CompFacets::aggregateWith`，真正相减发生在
`GM_Facets::obscureFaces`（libgm `0x10068710`）——逐对超级面在**面内做二维多边形相减**
（`D2_PolySet::booleanPrecise(..., 3)`），切分线的 side 判定与事后
`D2_PolySet::normalise` 都吃 `restol`。近共面被 0.051mm 塞成真共面，然后减掉。

对照我们这条路：`plant_mesh_to_manifold` 焊顶点用的是 `to_bits()` **逐位相等**，
整条链上没有任何位置容差。所以结论不是「libgm 有个共面规则我们没抄」，而是
**「libgm 有 0.051mm 的重合容差，我们一个都没有」**。

### 候选方向

1. **减之前沿负体自身的挤出轴两端各外扩 0.051mm**（只在布尔入口做，不写回 `inst_geo`，
   `geo_hash` 不动）。最小改动，理由硬：外扩量正好等于 libgm 的分辨率，在它的世界里
   这段距离不存在，跨不过任何 libgm 分得清的界限。局限：只覆盖有明确挤出轴的负体。
2. **给 manifold 设同一个容差**（若 `manifold-csg` 暴露 tolerance/epsilon），让整条布尔
   在 0.051mm 口径下工作。最忠实，但同时影响简化与合并，影响面大得多。
3. **入口焊接从逐位改成 0.051mm 容差焊**。顺带解决共面，但同样是全局口径变更，
   且可能把 0.05mm 的真实薄壁焊没。

### 已实施（2026-08-25）

原来**已经有**一步「让」：`manifold_csg.rs` 的 `NEGATIVE_INFLATE = 1e-6`，按负体包围盒
中心**等比**放大。问题在量级和形状：等比量在薄方向上等于没让——这个负体沿墙厚只有
750mm，1e-6 给出 **0.000375mm**，比实测那道缝还小一个量级；而三个轴 2600 × 750 × 2180
差着一个数量级，等比放大要么长轴让太多、要么薄轴让不够。

改成 **逐轴各向外让一个绝对量 `RES_TOL_MM = 0.051`**（新常量，落在
`libgm_discretise.rs` 里 `FACET_TOL_MM` 旁边，注释带 IDA 出处）：

```rust
let axis_scale = |min: f64, max: f64| {
    let extent = max - min;
    if extent > f64::EPSILON { (extent + 2.0 * grow) / extent } else { 1.0 }
};
```

退化到零厚的轴不缩放——那种负体本来就不是合法实体，放大它只会把 NaN 带进布尔。

### 回归钉

`fast_model::manifold_csg::tests::a_negative_stopping_a_hair_short_still_opens_the_exit_face`：
200×100×200 的块，负体从 −y 穿进来、出口停在 y = 49.99（差 **0.01mm**，比 `RES_TOL_MM`
小一个量级）。挖穿了亏格 1、留一层皮亏格 0，所以亏格就是红绿灯。把常量退回旧的等效量
（这个负体上是 0.000055mm）实测即红：`genus=0`、`volume=3360061.89`（比干净穿透多出
约 62mm³，正是那层皮）。

### 现场验证（8009 = `.surreal/ams-rvm-rebuild-20260824`）

删掉 8 堵带负体 GWALL 的 `booled` `.mesh` 让对拍就地重算布尔后：

| | 修前 | 修后 |
|---|---|---|
| `pe:17496_105828` gen→gwall p95 / max | 753.9 / 1296.9（门槛 12，**红**） | **0.1 / 65.2** |
| 105828 三角数 | 188 | 184（那层皮的 4 个三角没了） |
| `pe:17496_105880` p95 | 9.4 | 8.9 |
| `pe:17496_116569` p95 | 147.4 | 137.3 |
| GWALL union both mean / p95 / hausdorff | 10.53 / 8.44 / 1286.31 | **4.75 / 5.33 / 647.09** |

八条 mesh 级对拍 **8 passed / 0 failed**（修前 7/1）。

### 仍未做的（第 2/3 条候选，另立项）

「给 manifold 设同一个容差」与「入口焊接从逐位改成 0.051 容差焊」都还没做，
480 行 `neg_relate` 里共面出口到底占多少也还没量。

### 顺带带出的第二个口径问题（不在本 issue 范围，需单独决策）

`src/fast_model/libgm_discretise.rs` 的 `NORM_TOL = 1e-6` 注释断言「没有人改它」——
成员写入器 `GM_User::normtol(double)` 确实零调用，但 Core3D 走的是自由函数
`gm_SetDefaultNormalisationTolerance`，运行期真值是 **0.051**，差 51000 倍。
`normtol_` 的读者是 `gm_CreateBody` / `gm_CreateNormalisedItem` /
`gm_CreateFacetStructure` / `gm_QueryMass`，还影响回转轮廓的轴心吸附。改它会动到
所有回转体，本次只记录不动代码。

## 🧪 测试验证

### 验证标准

`mesh_gwall_extra_against_cwall_union` 里 `pe:17496_105828` 的 gen→gwall p95 从 753.9
落到门槛 12 以内，且 `mesh_gwall_union_surface_distance` 的 20 堵不出现新的红。

### 需要补的纯函数用例

一条不连库的回归：正方体 + 一个长度**正好等于**正方体厚度、出口面与其一面共面的负体，
布尔后该面必须被挖开（回退到不加余量即红）。现有布尔用例没有共面这一档。

## 📚 相关文档

- 台账：`docs/2026-08-12_live-test-ledger.md`（2026-08-25 八条重新取证那一条）
- 证据：`docs/evidence/2026-08-25-sweep-path-frame-fix.md` §6
- 同族：ADR-044（负体分段身份键，「共面留一层壁」）

## 🏷️ 标签

bug high-priority geometry boolean manifold rvm-baseline

---

**创建日期**: 2026-08-25
