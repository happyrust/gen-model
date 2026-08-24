# Plan 028：硬边法向分组

**Spec**：`specs/028-hard-edge-normal-grouping/spec.md`
**ADR**：`docs/adr/ADR-047-hard-edges-come-from-the-profile.md`

## 结构

判据、标记、着色三件事分开落，各自能单独测：

```
libgm_discretise                    sweep_mesh                        manifold_csg
─────────────────                   ──────────                        ────────────
span_first_tangent  ┐                                                 SMOOTH_NORMAL_COS
span_last_tangent   ├─ tangents_are_smooth ─ spans_meet_smoothly       （Phase 3 退役）
                    ┘                              │
                                                   ▼
                                        discretise_loops
                                          → (Vec<Loop2D>, Vec<Vec<bool>>)
                                                   │
                                                   ▼
                                        push_side_grid（按硬边分组平均）
```

- **判据**在 `libgm_discretise`：它已经是「libgm 的离散规则只有一份」的那个模块，
  切线与相切判据属于同一族。纯函数，不碰网格。
- **标记**在 `discretise_loops`：硬边只能在铺点的同一趟里算——`getPolygonForFacet`
  就是这么写的（铺完一段才知道该给哪个点打负号），拆开就要把「哪个点是哪段的末点」
  再推一遍。
- **着色**在 `push_side_grid`：拿位置网格 + 硬边标记出四边形，顶点仍逐面片独立。

## Phase 1（本期）

1. `src/fast_model/libgm_discretise.rs`：`SMOOTH_TANGENT_TOL`、`span_tangent`
   （私有，三条分支照抄）、`span_first_tangent` / `span_last_tangent`、
   `tangents_are_smooth`、`spans_meet_smoothly`。
2. `src/fast_model/sweep_mesh.rs`：`ProfileLoops` 加 `hard`；`discretise_loops` 顺带算
   标记（含闭合处末点与首点一起标、去重与翻转时标记跟着走）；`push_quad` 换成
   `push_side_grid`；`extrude_loops` / `loft_loops` / `revolve_loops` 改收 `&ProfileLoops`。
3. 门 G-1 / G-2 / G-3 / G-5 落成 `sweep_mesh` 单测。

## Phase 2（T07 判读结论，2026-08-24）

### 通道是现成的，而且路子跟 glTF 一样

`manifold-csg` 绑定面上需要的东西全都有（读的是 `../manifold-csg` 的源码，
`API_COVERAGE.md` 逐条对得上）：

| 要的能力 | 绑定里的入口 |
|---|---|
| 带属性建体 | `MeshGL64::new_with_options(...)` → `Manifold::from_meshgl64` |
| 同位置多行顶点缝回一个点 | `MeshGL64::merge_vertices(from, to)`；`MeshGL64::merge()` 按位置自动算 |
| 读回属性 | `Manifold::to_meshgl64()` / `to_mesh_f64()`（`num_prop` + 交错数组） |
| 告诉 manifold 哪一路是法向 | `to_meshgl64_with_normals(idx)` / `to_mesh_f64_with_normals(idx)` |
| 逐顶点改属性 | `Manifold::set_properties(n, |new, pos, old| …)` |

**硬边的表示方式不用发明**：同一个 3D 点上放两行属性顶点（各带各的法向），再用
`merge_from_vert` / `merge_to_vert` 告诉 manifold「这两行其实是同一个点」——这正是
glTF 表达硬边的老办法，也正是我们的生成器已经在做的事（渲染顶点本来就在硬边处
分开了）。今天 `plant_mesh_to_manifold` 按精确坐标**焊成一行**，等于在入口处把这个
分裂扔掉了；Phase 2 是把它留住，而不是新建一套机制。

顺带一条：`Manifold::calculate_normals(idx, min_sharp_angle)` 是 manifold 自带的
**又一个夹角启发式**（上游默认 60°）。别用它——那是把 10° 换成 60°，不是把猜换成规则。

### 一次实测，以及它照出来的一个坑

给 20mm 方块 `set_properties(4, …)`（第 4 路 = 顶点 x），拿一个没加属性的 9mm 方块
走 `subtract_negatives` 切一刀，读回来 **`num_prop = 7`**。

7 = 4 + 3。看着像**两个操作数的通道被拼接**而不是按位对齐。如果属实，Phase 2 有一条
硬约束：**参与同一次布尔的所有操作数必须带同一套通道布局**。这对我们不是小事——
负实体是从磁盘 `.mesh` 读进来的，那条路今天一路属性都没有，不统一就会出现
「正体的法向在第 3–5 路、负体的在第 6–8 路」这种错位。

**这一条只测到一半就断了**（工作区当时被另一会话的 rev 分裂打断，见下）。要补的探针
写在这里，下次一次跑完：

1. 两个操作数都 `set_properties(4, …)`，看 `num_prop` 是 4 还是 8 —— 定「拼接还是对齐」；
2. 逐顶点检查第 4 路是否恒等于 x —— 定「插值是不是线性的」，x 是线性量，是就精确成立；
3. 造一个同位置两行属性的 `MeshGL64` + `merge_vertices`，过一次布尔再读回，看那两行
   还在不在 —— 定「硬边能不能活过布尔」，这是 Phase 2 成立与否的那一条。

### 交线怎么算——已反编译，我原先的猜测只对一半

原先这里写的是「交线两侧来自不同操作数的属性行，硬边**自然落出来**，不需要额外规则」。
**错了一半。** 证据 `docs/evidence/2026-08-24-ida-edge-types-and-smoothing-groups.md`：

`GM_Facets::normaliseStage2`（3.1 libgm `0x10066E70`）对布尔新建的边（type 3）：

```text
只有一侧有面            → type 2（硬）
两侧都有面：
    tangTol ≤ 0        → type 2（硬）        // tangTol 是开关，不是阈值
    isTangentDiscontinuity(a, b) → type 2（硬）
    否则                → 留 3，收尾统一降级成 type 1（软）
```

判据 `isTangentDiscontinuity`（`0x10066A40`）：两个单位法向的**弦长 > 0.8182**
（＝夹角 **48.297°**）就是硬；`< 0.001` 是软；中间带默认硬，除非邻面的法向说明
「曲面还在往下弯」。**常量是 cos/sin 22.5°，但有效阈值是 48.3°**——文档 §6.8.3 记的
「固定 22.5°」是生成常量，照着 22.5° 实现会差一倍。

对 Phase 2 的影响：属性行确实会让交线**天然分开**（硬的那一半自动成立），但
**相切的缝 E3D 会主动合回软的**，只做前一半会在同轴等径圆柱相接、倒角与母面相切
这类地方留亮缝——正是 `d0088e93` 当初要治的毛病。所以 Phase 2 是两步：
属性行带过硬边 + 交线边按 `isTangentDiscontinuity` 复判。

### 还没答的

- 属性行变多会不会拖慢布尔（几何顶点数不变，属性行数变多），要量。
- `simplify` / `refine` 这类操作对属性做什么，没查。
- `isTangentDiscontinuity` 中间带那道「邻面旁证」的局部系构造（`D3_Transform::map`
  的六个实参）与锥角门的精确几何，只读到轮廓层面，没逐位吃透。两个早退分支
  与三个常量是实证。

## Phase 3（待排期）

`SMOOTH_NORMAL_COS` 删除 + 源码断言禁止夹角阈值分组。前置是 Phase 2 全绿。

## Constitution Check

- **响亮失败**：本条不新增判定分支；硬边标记缺失时按「硬」处理——那是最保守的着色
  （逐片平直），抹不掉真实折痕，且它在 Phase 1 里不可达（`discretise_loops` 一定同时
  产出两者）。不设「标记没算出来就当软的」这种静默放行。
- **单一规则**：相切判据只在 `libgm_discretise` 定义一处；三个不同阈值分开命名、
  各带出处，Phase 3 加源码断言钉住。
- **测试钉不变量**：每条门带判别性自检（粗弧那条同时断言「确实只有 8 段」，浅角那条
  同时断言「确实浅于 10°」）——否则测试可能在两种实现下都绿。
- **live 留痕**：Phase 1 不改几何（FR-3），无需 live；Phase 2 改布尔结果，要进
  `docs/evidence/` 与 live 台账。
- **运行环境**：全程禁 `cargo clean`。

## 风险

1. **着色变化肉眼可见。** 弧墙 / 斜切墙从折面变光顺，看的人会以为几何变了。发布说明
   要写清楚「顶点一位没动，变的只有法向」，FR-3 的门就是为了能这么说。
2. **布尔前后不一致的过渡期。** 同一个模型里，过布尔的构件与不过布尔的构件着色口径
   不同。Phase 2 之前这是已知的；写进 ADR-047「后果」，不靠口头约定。
3. **`ProfileLoops` 换了形状。** `loops` 与 `hard` 必须同长同序——去重、掐尾、翻转
   三处都要同步操作，漏一处就是「硬边错位一个点」这种很难看出来的缺陷。已用 G-1
   的「恰好四个角」把位置钉住，而不只是数量。
