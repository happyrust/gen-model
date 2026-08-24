# Tasks 028：硬边法向分组

**Input**：`specs/028-hard-edge-normal-grouping/spec.md`、`plan.md`
**Prerequisites**：ADR-047；证据 `docs/evidence/2026-08-24-ida-gm-collar-ruled-solid.md` §六

## Phase 1 判据与轮廓侧（2026-08-24 全部落地）

- [x] T01（串行）`src/fast_model/libgm_discretise.rs`：相切判据落成纯函数。
  `SMOOTH_TANGENT_TOL = 1e-6`（`D2_Span::leadsSmoothlyTo`，3.1 libgeom `0x10029B50`）、
  `span_first_tangent` / `span_last_tangent`（`0x100296F0` / `0x10029930`）、
  `tangents_are_smooth`、`spans_meet_smoothly`。
  切线三条分支照抄，**包括 `0 < |bulge| < SPAN_EPS` 那条非单位的退化分支**——
  它的实际效果是「极小非零 bulge 必被判成硬边」，归一化会把这个行为改掉。
  本仓 `profile_spans` 先把这种 bulge 收成尖角，所以那条今天不可达，注释里写明。
- [x] T02（依赖 T01）`src/fast_model/sweep_mesh.rs`：`ProfileLoops` 加 `hard`，
  `discretise_loops` 在铺点的同一趟里算硬边。
  三处必须与点同步：去重跳过的点把硬边**并进**留下的那个；掐掉收尾重复点时把它的硬边
  归到首点（`getPolygonForFacet` 闭合处就是末点与首点一起取负）；按绕向翻转时标记
  一起翻。
- [x] T03（依赖 T02）`src/fast_model/sweep_mesh.rs`：`push_quad` → `push_side_grid`。
  拿位置网格 + 硬边出四边形，软点跨面片平均、硬点不平均、扫掠方向永远平均；
  权重取叉积模长（与 `manifold_csg` 的面积加权同口径）。顶点仍逐面片独立，
  拓扑一位不动。`extrude_loops` / `loft_loops` / `revolve_loops` 改收 `&ProfileLoops`。
- [x] T04（依赖 T03）门。`sweep_mesh` 三条新单测，33 全绿：
  - `the_hard_edges_are_the_profile_corners_not_the_arc_interior`（G-1，
    环形扇区恰好四个角，自检弧上点数 > 40）；
  - `a_coarse_arc_is_still_one_smooth_surface`（G-2，整圈 8 段，顶点法向必须等于该顶点
    自己的径向；自检「确实只铺出八边形」）；
  - `a_shallow_corner_is_still_a_hard_edge`（G-3，5.7° 折角仍是硬边；自检「确实浅于
    10°」）。
  G-5 由既有体积 / 包围盒 / `assert_solid_mesh` 那批测试兜住——它们一条没改就全绿，
  说明拓扑确实没动。

## Phase 1 收尾

- [ ] T05 G-4：同一副截面在两个差一个量级的容差下，硬边落在轮廓的同一批角上。
  现有 G-1 只量了一个容差，密度无关这条还没有专门的钉子。
- [x] T06（2026-08-24 落地）弧墙那条路（`manifold_tessellate::tessellate_arc_wall`）
  此前**绕过**本条：它走 `Manifold::extrude`，法向由 `manifold_to_plant_mesh` 的 10° 决定。

  **判读结论：弧墙改走 `sweep_mesh` 的挤出，一般挤出留在 manifold。** 分界不是随手划的：
  `Manifold::extrude` 那一支买的是 `CrossSection` 的 NonZero 填充，它**化解自交轮廓**
  ——两个大倒角在同一条边上撞车时 E3D 照铺（`mthArcFillet` 不裁，见
  `libgm_discretise::profile_spans` 的文档），earcut 不认这种环，`rm12_r972_pane_survives_overlapping_fillets`
  钉的就是这件事。而弧墙的环是**解析出来的**四段（内弧 / 直段 / 外弧 / 直段），
  `thick` 吃穿半径在上游已经硬失败，不可能自交——它不需要那道填充，所以搬过去没有代价。
  一般挤出（PLOO 带倒角）需要，留给 Phase 2 的属性通道。

  落地：`sweep_mesh` 新增 `loops_from_spans`（已解好的 span 环直接进挤出口径的离散），
  `tessellate_arc_wall` 把四段拼成 `ProfileSpan` 后走它 + `extrude_loops`；
  手抄一遍铺点循环是不行的，硬边那一半会漏在门外，所以入口要共用。
  `extrude_flat_polygons` 的文档改写：说明它为什么还留在 manifold 上、代价是什么。
  门：`the_arc_wall_is_smooth_along_the_arcs_and_creased_at_the_four_corners`
  ——半圆环的折痕位置**正好八个**（四个轮廓角 × 两个 z 层），多了说明弧上也在断、
  少了说明角被抹平；自检「相邻侧壁位置 > 40」保证弧真的铺开了。
  既有四条弧墙测试（帕普斯体积 / 三点 / 共线 / 容差）一条没改，`arc_wall` 4 全绿。

## Phase 2 布尔侧（待排期）

- [x] T07（2026-08-24 判读，结论进 plan 的「Phase 2」一节）manifold 的逐顶点属性通道。
  **可行，且不用发明表示法**：同位置放两行属性顶点 + `merge_from_vert` / `merge_to_vert`
  缝回一个点，就是 glTF 表达硬边的老办法；绑定面上
  `MeshGL64::new_with_options` / `merge_vertices` / `merge()` / `from_meshgl64` /
  `to_meshgl64[_with_normals]` / `set_properties` 全都有。
  今天 `plant_mesh_to_manifold` 按精确坐标把渲染顶点**焊成一行**，等于在入口处把硬边
  分裂扔掉了——Phase 2 是留住它，不是新建机制。
  **不要用 `Manifold::calculate_normals(idx, min_sharp_angle)`**：那是 manifold 自带的
  又一个夹角启发式（上游默认 60°），把 10° 换成 60° 不叫把猜换成规则。
  **一次实测照出一个坑**：4 通道的正体 ⊖ 3 通道的负体，读回来 `num_prop = 7`——
  看着像通道**拼接**而非按位对齐。若属实，同一次布尔的所有操作数必须带同一套通道
  布局，而负实体是从磁盘 `.mesh` 读的、今天一路属性都没有。
  这一条只测到一半（工作区被 rev 分裂打断），三条待跑探针写在 plan 里。
- [ ] T07a（从 T07 拆出）把那三条探针一次跑完：通道是拼接还是对齐、插值是不是线性、
  同位置两行属性能不能活过布尔。第三条是 Phase 2 成立与否的那一条。
- [x] T07b（2026-08-24，从 T08 里拆出来先反完）**交线边的定型规则已钉死。**
  证据 `docs/evidence/2026-08-24-ida-edge-types-and-smoothing-groups.md`。
  `GM_Edge::isVisible`（3.1 `0x10004A60`）= `type != 1 && type != 5 && type != 6`，
  `AM_CoEdge::calcStartNormal` 的平滑组走到「可见」边就停——**硬边在 libgm 里是边上的
  一个枚举值，不是几何量**，谁写这个值谁是权威，一共三处（图元建面写死 / 轮廓
  `leadsSmoothlyTo` / 布尔 `normaliseStage2`）。
  `normaliseStage2`（`0x10066E70`）：交线边 type 3 默认判硬，只有一侧有面直接 2；
  两侧都有时 `tangTol ≤ 0`（**开关，不是阈值**）或 `isTangentDiscontinuity` 为真判 2，
  否则留 3、收尾统一降成 1（软）。
  `isTangentDiscontinuity`（`0x10066A40`）：法向弦长 > **0.8182**（夹角 **48.297°**）判硬，
  `< 0.001` 判软，中间带默认硬、除非邻面旁证「曲面还在往下弯」。
  **常量是 cos/sin 22.5°，有效阈值是 48.3°，差一倍**——文档 §6.8.3 的「固定 22.5°」
  说的是生成常量，照着抄会把该软的缝判硬。
- [ ] T08（依赖 T07a + T07b）硬边随属性过布尔，`manifold_to_plant_mesh` 改按属性分组。
  **两步，不是一步**：属性行带过来的硬边只覆盖「交线默认硬」那一半，相切的缝还要按
  `isTangentDiscontinuity` 主动合回软的，否则同轴等径圆柱相接、倒角与母面相切这类
  地方会留亮缝（`d0088e93` 当初治的就是这个）。
- [ ] T09（依赖 T08）门 G-7：过一次布尔的构件，未被切到的曲面法向与不过布尔时一致；
  证据进 `docs/evidence/`，live 台账同步。

## Phase 3 收口（待排期）

- [ ] T10（依赖 T09）删除 `manifold_csg::SMOOTH_NORMAL_COS` 与那段夹角分组。
- [ ] T11（依赖 T10）门 G-6：源码断言——三个相切判据各定义一处，生产半区不得再出现
  「按夹角决定要不要共享法向」的写法。
