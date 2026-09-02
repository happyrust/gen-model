# Issue #028: RVM 快照的 `aabb_world_mm` 与同一条记录里的面片顶点对不上，AABB 对拍门整批误判

## 📋 Issue 信息

- **Issue ID**: #028
- **类型**: Bug 🐛（**基准字段坏**——拿它当判据，几何逐位全等的元素被整批判 `off`）
- **优先级**: High 🟠 —— 2026-08-31 凌晨它把 e3d-model 的 20 件 GWALL 对拍全部顶红
  （23mm~5.4m），一整段会话在「轮廓语义差异」的假线索上耗尽后断线；
  凡是吃快照 `aabb_world_mm` 的门（`rvm_compare`、`python/scripts/rvm_aabb_compare.py`
  这一族窄口径 AABB 判据）对**带旋转摆放**的元素都不可信。
- **状态**: Open 📝（e3d-model 侧已绕开；gen-model 快照导入侧未修）
- **创建日期**: 2026-08-31
- **发现于**: e3d-model ams1112 GWALL 对拍（会话 fable-5-7，恢复 73c37445 后三方对账）
- **相关模块**: `src/rvm_baseline/import.rs`（快照 aabb 的来源）、
  rvm-rs `parser/rvm.rs` / `math/bbox.rs`（`geometry.bbox_world` 的算法）

## 🔍 问题描述

`test_data/rvm/1RS-WF03-W-C-RR001.rvm.json`（`rvm_verify import --scope narrow` 产物）
里每个成员的 `aabb_world_mm`，与**同一个 `.rvm` 文件、同一条 PRIM 记录里的
FacetGroup 顶点**实算出的世界 AABB 对不上。

三方对账（20 件 GWALL；`vendor/e3d-model` 的 `examples/rvm_facets_dump.rs` +
`scripts/gwall_aabb_threeway.py` 可复现）：

| 对比 | 最大轴差 |
|---|---|
| 面片顶点实算 vs 快照 `aabb_world_mm` | **23.3mm ~ 5372.8mm，20/20 全超** |
| 面片顶点实算 vs e3d-model 生成侧最终网格 | 17/20 = 0.00mm，其余 0.24 / 6.0 / 63.5mm |

第二行证明面片顶点才是真身（两套独立实现逐位互证）；第一行证明快照字段
与它自己文件里的几何自相矛盾。

样例（GWALL 1，`17496/118174`）：

```
面片顶点实算  [2318.7, 15949.6, 2160.0] ~ [4226.1, 17097.1, 2410.0]
快照字段      [4021.8, 16353.9, 2160.0] ~ [6022.1, 17663.7, 2410.0]   ← X 差 1.8m
```

### 复现步骤

```powershell
cd d:\work\plant-code\old\vendor\e3d-model
cargo run --release --example rvm_facets_dump -- `
    --rvm d:\work\plant-code\old\gen-model\test_data\rvm\1RS-WF03-W-C-RR001.rvm `
    --contains "of CWALL /1RS-WF03-W-C-RR001" --out out/ams1112/rvm-facets-RR001.json
python scripts\gwall_aabb_threeway.py
```

### 预期行为

快照 `aabb_world_mm` = 该成员几何顶点的世界包围盒。

### 实际行为

快照 `aabb_world_mm` 抄的是 rvm-rs `geometry.bbox_world`，而那个值与面片顶点无关。

## 🔬 问题分析

### 根本原因（已核实的两层）

1. **PRIM 记录自带的 24 字节 bbox 本身就不是面片的盒**。实测 GWALL 5
   （简单三角棱柱，无任何负体参与）：

   ```
   面片顶点局部 AABB   X[-199.98, 0]  Y[0, 199.98]      Z[0, 3620]
   PRIM 记录 bbox      X[-199.82, 0]  Y[-201.72, 0]     Z[0, 3620]   ← Y 镜像、略大
   ```

   记录盒是**另一套局部框架**（Y 取反、每边大 ~2mm）——疑似 E3D 的设计范围盒
   而非渲染面片盒。上一会话追的「镜像 + 毫米级外扩」幽灵就是它。

2. **rvm-rs 拿这个记录盒再乘一次几何 transform** 得 `bbox_world`
   （`parser/rvm.rs`：`geo.bbox_world = bbox.transform(&transform)`），
   随后 `import.rs::build_geometry` 原样 ×1000 收编进快照。
   垃圾进，垃圾出。

### 影响范围

- **误判方向是「几何对了却报差」**——比漏报安全，但代价是整段排查走错方向：
  2026-08-31 会话 73c37445 在此损失约 40 分钟后断线（配对/摆位/Z 逐项排除，
  最后卡在「轮廓形状本身不同」的错误结论上）。
- 吃快照 `aabb_world_mm` 的所有判据对**带旋转**的元素不可信；平移为主、
  旋转接近单位阵的元素（如多数管件）偏差可能小到看不出来——**更隐蔽**。
- `rvm_baseline/mesh_compare.rs::rvm_world_meshes_by_name` **不受影响**
  （它从面片顶点自己算），mesh 级对拍的既有结论仍然成立。

### 相关代码

| 位置 | 角色 |
|---|---|
| rvm-rs `parser/rvm.rs`（PRIM 分支尾部） | `bbox_world = 记录盒.transform(transform)`，坏值出生点 |
| `gen-model/src/rvm_baseline/import.rs::build_geometry` | 把 `geometry.bbox_world` ×1000 抄进快照 |
| `vendor/e3d-model/src/bin/rvm_compare.rs` | 受害者；已加 `--facets` 绕开（见下） |
| `vendor/e3d-model/examples/rvm_facets_dump.rs` | 取证工具：面片顶点 + 记录盒都倒出来 |

## 🛠️ 解决方案

### 已落地（e3d-model 侧，2026-08-31）

`rvm_compare` 增加 `--facets <rvm_facets_dump 产物>`：基准 AABB 用面片顶点实算值，
快照字段只当兜底并在 `aabb_source` / `aabb_from_snapshot` 里留痕。
效果：20 件 GWALL 从 20/20 off → 18 exact + 1 chord + 1 off（该 off 已归因为
E3D 自家面片里的针状残料，见对拍报告 `out/ams1112/compare-1RS-WF03-W-C-RR001.json`）。

### 待做（gen-model 快照导入侧）

`import.rs` 对带 FacetGroup 的成员改用**顶点实算**世界盒；参数化基本体
（Cylinder/Box/…）用 rvm-rs 的 `Tessellate` 结果算，别抄 `bbox_world`。
改完重导快照，`degenerate_bbox_count` 与逐成员 aabb 全部换血——
吃快照的下游判据要重新跑一轮基线。

### 风险评估

- 重导快照会让历史对拍报告里的 AABB 数字集体变化——那是修对了。
- rvm-rs 是外部 git 依赖，修它的 `bbox_world` 语义动不动上游拍板；
  在 import 侧自算可以不碰 rvm-rs。

## 🧪 测试验证

- `gwall_aabb_threeway.py`：修后「快照字段 vs 面片实算」20/20 应归零（f32 舍入内）。
- 快照导入加一条自检：任何带几何成员，`aabb_world_mm` 必须包含其全部
  tessellate 顶点（含 f32 容差）——本缺陷有这条断言在导入当天就会炸。

## 💡 经验教训

1. **基准也要自证**：拿第三方字段当验收判据前，先拿同一文件里的原始几何
   把字段对一遍。这次的三方对账（面片/快照/生成侧）一步就把方向扳回来了。
2. **「全体整齐地错」指向判据坏，不是几何坏**：20/20 全 off、且摆位/旋转/Z
   逐位全等——几何真坏不会坏得这么整齐。
3. 与 #026/#027 同族：**上游写了一个「看起来像答案」的值，下游不核就收**。

## 🏷️ 标签

`bug` `rvm-baseline` `rvm-rs` `aabb` `acceptance-gate` `false-alarm` `e3d-model`

---

**创建者**: fable-5-7（恢复会话 73c37445 后）
**最后更新**: 2026-08-31
