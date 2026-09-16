# BRAN 中心线邻近与最近净距落到 gen-model-v1 后端——开发计划（纠偏版）

> 来源：用户 2026-09-16 对 fable-5-1-48 的纠正——「前端是 `../plant3d-web`，我的后端是 `D:\work\plant-code\old\gen-model-model-cache`」。
> 此前三段会话（fable-5-1-36 → 45 → 48）把后端改动全部落在了 **`d:\work\plant-code\plant-model-gen`（legacy，`/api/sqlite-spatial/*`，0.3.34）**，
> 而本仓（`codex/model-projection-cache@da461b71f`，0.1.27）没有 `src/web_server`、没有 SQLite 空间索引、没有 `tubi_relate`，
> 空间查询走进程内 `GLOBAL_AABB_TREE`（`/api/v1/spatial/*`）。本计划重新对准本仓。
> 状态：**已落地（P1 / P2 / P3 含 §3.3 ③ ④，2026-09-16）**。契约见 `docs/specs/web-service-api.md` §4.13.1（`source_mode=bran_centerline`）/ §4.13.2
> （`/nearest-clearance`）/ §4.13.3（`/centerline`，④ 管-管间距纠偏用），变更记 `changelog.md` 2026-09-16，词条见 `CONTEXT.md`「空间邻近查询」。§4 的待拍板项按实现口径定下：
> D2 保留前端骨架并把 `branCenterline` 翻 true、D3 `include_self` 缺省 **false**、D5 `useSpatialCompute` 内按 kind 分流、D7 与 legacy 同名
> `/api/v1/spatial/nearest-clearance`、D8 按 P1 → P2 → P3；**仍开着**：D1（legacy 三轮改动留 / 回退）、D4（`surface` 缺省——当前 `false`）、
> D6（`column` 含 GENSEC——当前含）。真机 e2e（§5 那两条 GET）未跑。
> 代码事实原对到本仓工作树 2026-09-16 10:xx（`spatial_query.rs` 1681 行、`mbd/branch_query.rs`、`web_service/handlers.rs:1359-1399`），落地后以源码为准。

## 0. 一句话

**把「指定 BRAN → 沿真实中心线找周边构件 / 找最近墙柱并给出可画尺寸的两个端点」做成 `/api/v1/spatial/*` 下两条只读 GET，
中心线与成员来自本仓已有的 `mbd::branch_query`（MBD 管道尺寸用的同一份成员序 + 隐式管身），候选仍只来自 `GLOBAL_AABB_TREE`；
前端 `gen-model-v1` 数据源接上这两条，`legacy` 那一路今后不再动。**

## 1. 走错的那三段：什么可以留、什么要收回（D1 / D2）

| 仓 | 已改 | 状态 | 建议 |
| --- | --- | --- | --- |
| `plant-model-gen`（legacy 后端，非 git） | `nearest-clearance` A+B（成员排除 + `group_by=noun`，13 测）；`<-pe_owner` 取成员（修 `pe.children` 恒空）；`/query mode=bran_centerline` 自身排除 + `warnings`；测试并行串行锁；共 40 测全绿，`web_server.exe` 已重编 | 对 legacy 后端是正确、完整的改动，只是**不是你在用的后端** | **D1 待定**：留着（legacy 站点还能用）或整体回退。本计划之后不再碰它 |
| `plant3d-web`（前端，未提交，12 文件） | ① `genModelSpatialApi.ts`：`BranNearestClearanceRequest/Response` 加 `group_by / exclude_nouns / target_nouns / include_self / noun_counts / excluded_self_members`；`SpatialNearbyParams.source_mode='bran_centerline'` 在 **legacy** 下改打 `/query`；② 端口 `SpatialSourceCapabilities.branCenterline`（legacy true / v1 false）；③ `useSpatialQuery` refno 两档共用路径、中心线不扫本地、剔源自身；④ 抽屉「沿 BRAN 中心线」一档（按能力显隐） | ②③④ 是**与后端无关的前端骨架**，v1 接上后只需把能力翻成 true；① 的类型形状若 v1 沿用同一契约（§3.2）也直接复用；只有 ①「legacy 改打 `/query`」那一小段是 legacy 专属 | **D2 待定**：保留骨架，v1 落地后把 `GEN_MODEL_V1_SPATIAL_CAPABILITIES.branCenterline` 翻 true；legacy 专属分支留着无害（按源分流） |

## 2. 本仓现状盘点（能直接复用的积木）

| 积木 | 位置 | 与本需求的关系 |
| --- | --- | --- |
| 空间查询流水线：解中心 → 读锁内取候选 → 锁外过滤 / 排序 / 分页 / 补属性 | `src/fast_model/spatial_query.rs`（`resolve_center` L417、`collect_candidates` L493、`filter_candidates` L621、`assemble_nearby` L816、`nearby` L915） | 新增「中心线走廊」只是多一种 `Target` / 候选取法，过滤排序分页原样复用 |
| refno 模式的自身剔除 = **目标 + 子树**（`Center.subtree`：投影里祖先链含目标且有 mesh 的记录） | `resolve_center` L429-462、`filter_candidates` L631-637 | BRAN 的隐含管子（source_refno=BRAN）与成员构件都在子树里——**A「排除 BRAN 全部成员」在 v1 refno 模式下已经成立**，无需 `pe_owner` |
| 真实中心线：成员序 + 到达 / 离开点（世界 mm）+ 隐式 TUBI 段（`A~B`）+ 外径 `outside_diameter` | `src/mbd/branch_query.rs`（`branch_query(set, refno) -> plant_mbd::BranchQueryResult`，L120；`BranchMember{start,end,pos,noun,order,outside_diameter}`） | 走廊几何与「扣管外径」的数据源；已被 `/api/mbd/v2/pipe/{refno}` 使用（`web_service/mbd.rs:44-80`，`build_set(dbno)` + `spawn_blocking`） |
| 树条目 `RStarBoundingBox{aabb, refno, noun}`；`AccelerationTree::locate_intersecting_bounds / locate_within_distance / entries_of` | rs-core `accel_tree/acceleration_tree.rs:23` | 候选按 noun 分桶、按盒算最近点都够用 |
| 负实体清单 `TOTAL_NEG_NOUN_NAMES`、`is_negative_noun` | `spatial_query.rs` L595-610 | `group_by=noun` 不给目标时默认剔负几何 |
| 只读路由守卫：`spatial_routes_are_read_only_get_endpoints`（`web_service/mod.rs:982`）、树写点白名单测试 | `web_service/mod.rs`、`aabb_tree.rs` | 新路由必须是 GET、不写树 |
| handler 骨架：`resolve_identity` → `SpatialQueryParams::parse` → `state.direct_tree.get()` → `spatial_query::nearby` | `web_service/handlers.rs:1359-1390` | 两条新路由照抄 |
| 前端 v1 适配器 | `plant3d-web/src/model-source/genModelV1/spatialSource.ts`（`nearby / nearbyRefnos` 参数映射 L60-80）、`src/api/genModelV1Api.ts:1042-1067` | 接 `source_mode`；新增 `genModelV1SpatialNearestClearance` |
| 前端消费方 | `useSpatialCompute.branNearestClearance`（`normalizeBranNearestGroups` 已能吃数组形态分桶）、`branExternalDimensions.ts` → `dimensionSystem.replaceExternalSource('bran-clearance')`、`useDtxTools.measure_pipe_to_structure`、抽屉 | 契约对齐后零改动或只改取数函数 |

## 3. 方案（分三段落地，每段可独立验收）

### 3.1 P1 · `/api/v1/spatial/nearby` + `/nearby/refnos` 加 `source_mode=bran_centerline`

- 参数：`source_mode=bran_centerline`（缺省 = 现状），仅与 `refno` 搭配；`radius` = 走廊外扩距离；`shape` 沿用（sphere = 段到盒最小距离 ≤ r；cube = 只按外扩盒相交）。
- 中心线：handler 侧 `spawn_blocking(branch_query(set, refno))` 取 `members[].{start,end}`（含隐式 TUBI 段）拼成线段表，穿过件（`start==end` 或缺几何）跳过；
  `BranchQueryError::NotFound → 404`，`NotRouteContainer → 422`（「不是 BRAN」），`RootError → 422`，与 `mbd.rs` 同映射。
- 候选：读锁内对每段 `probe = 段盒外扩 r` 做 `locate_intersecting_bounds`，按 refno 去重；距离 = `min_k segment_aabb_distance(seg_k, aabb)`（从 legacy 移植 `segment_aabb_distance` / `segment_aabb_nearest_points` 两个纯函数，带单测）。
- 自身：`Center.subtree`（现有口径）∪ `branch_query` 成员 refno——两边取并，`include_self=false` 时整组剔；`center.source = "bran_centerline"`，响应多 `source: { kind, refno, segment_count, centerline_bbox }`。
- `/nearby/refnos` 同参同实现，只是不分页。
- 前端：`GEN_MODEL_V1_SPATIAL_CAPABILITIES.branCenterline = true`；`genModelV1/spatialSource.ts` 把 `source_mode` 映射进 v1 参数；`genModelV1Api.ts` 的 `SpatialNearbyRequest` 加 `sourceMode`。抽屉那一档随即在 v1 下出现。

### 3.2 P2 · 新增 `GET /api/v1/spatial/nearest-clearance`

- 入参（与 legacy 契约同名，前端 `queryBranCenterlineNearestClearance` 一字不改）：
  `source_refno`（必填）、`source_mode=bran_centerline|aabb`（缺省 bran_centerline）、`target_groups=wall,column`、`target_nouns=…`、
  `group_by=target_groups|noun`（缺省 target_groups）、`exclude_nouns=WELD,ATTA`、`radius`（缺省 5000，上限 `MAX_RADIUS_MM`）、
  `max_per_group`（缺省 1）、`dbnums`、`include_self`（缺省 **false**）、`surface=0|1`（**v1 独有**：扣 `outside_diameter/2`，legacy 没有 OD）、`debug`。
- 预置组沿用 legacy：`wall = WALL/PANE/GWALL/STWALL`，`column = COLU/SCTN/GENSEC`（**D6**：GENSEC 是否算柱）。
- 出参与 legacy `NearestClearanceResponse` 同形：`nearest_by_group[]{group, nouns, candidates[]{refno, noun, dbnum, distance_mm, intersects, aabb, nearest{source_segment_refno, source_segment_order, source_point, target_point, vector}, annotation{start_point, end_point, label_mm}}}`、
  `noun_counts`、`excluded_self_members`、`resolved_filters`、`warnings`、`source{kind, refno, segment_count, centerline_bbox, outside_diameter_mm}`、`distance_method`（`centerline_aabb_clearance_mm` / `centerline_surface_aabb_clearance_mm`）、`debug`。
  这样 `useSpatialCompute` 与 `branExternalDimensions` 直接可用，尺寸系统按 `bran-clearance` external source 画。
- 实现全部放 `spatial_query.rs` 旁的新文件 `src/fast_model/spatial_clearance.rs`（纯函数 + 本地 `AccelerationTree` 夹具单测），handler 只做身份 / 取服务 / 状态码；不写树、不新增表。
- `group_by=noun`：不给任何目标 = 半径内全部类型（负几何默认剔、显式点名放行）；每 NOUN 一桶按 `max_per_group` 截断；`noun_counts` 是截断前计数。

### 3.3 P3 · 前端接线（`plant3d-web`）

- `genModelV1Api.ts` 新增 `genModelV1SpatialNearestClearance(req)`；`useSpatialCompute.ts:521` 的取数按 `getModelSource().kind` 分流（或把 `nearestClearance` 收进 `SpatialSource` 端口 + 能力 `nearestClearance: boolean`，legacy 委托旧函数、v1 打新路由——**D5**）。
- Dock「目标 chips」改成由 `noun_counts` 驱动的动态 facet（fable-5-1-36 的 Pencil 设计 `ui/管道标注/bran-nearest-clearance.pen` 画板 02），每类默认画最近 1 条。
- `useDtxTools.measure_pipe_to_structure` 的 Toast「暂不创建尺寸」改为写入同一份 ClearanceRecord（沿用 2026-09-11 收敛计划 §6 M1）——
  **已落地（2026-09-16 晚，fable-5-1-9）**：`useDtxTools` 多一个 `recordBranClearance` 落点，`ViewerPanel` 接到
  `useSpatialCompute.recordInteractiveBranClearance`——点选管件算出的最近点对按 DTX 全局矩阵的逆换回 E3D 世界 mm，落进 BRAN 净距
  **同一份**结果（同一张表、同一组 facet、同一个 `bran-clearance` external source；目标类型那一桶末尾追加、同一目标就地替换、
  下一次服务端查询连它一起清），来源登记 `sampled-object / approximate-sampled`，表里标「估算最近距离（网格采样）」、尺寸文字带「≈」
  （2026-09-11 收敛计划 D2 / PR0.2）；不给落点时保留旧 Toast。顺手修了 `ViewerPanel.syncBranClearanceDimensions` 的坐标换算：
  候选端点是 E3D 世界 mm，Design Space 是同原点的米，应 `/1000`——此前套的是 `designToWorld` 的逆（scene → design），
  对 mm 端点差 1000 倍还带重定心平移，两个后端的 Dock 结果之前都画不到位（真机未验过所以没暴露）。
- ④ **管-管间距纠偏（用户 2026-09-16 19:06）**：「两个排管的距离测量是指两条 BRAN 的直段之间，如果有平行的部分就可以标注它们的间距」——
  ③ 里顺手把 `measure_pipe_to_pipe` 也接到同一份结果时，用的还是网格采样的最近点对（任意两个构件表面最近处，比如直管到阀体），
  答的不是这个问题。改法：
  - 后端加一条只读 `GET /api/v1/spatial/centerline?refno=` 把 P1 已拼出的走廊线段表原样交出去（每段带成员 noun / `implicit` /
    外径；spec §4.13.3）。**服务端不算平行**：几何只是几十条线段的配对，而点选上下文（点在哪一段、两根管各是谁）和「结果 → 尺寸」
    的整条链路都在前端；服务端只做它独有的事——把 E3D 库的成员序 + 隐式管身拼成真实中心线。
  - 前端 `measure_pipe_to_pipe`：点两根管 → 各自所属 BRAN（DTX 缓存的 `owner_refno` / `owner_noun`）→ 取两条中心线 → 连续共线的段
    合成**直段**（`implicit` 管身一定直；ELBO / BEND 的到达→离开是弦，剔掉）→ 直段两两配**平行对**（夹角 ≤ 0.5°、沿轴投影区间有重叠）→
    每对一条**中心距**尺寸：两端都在轴线上、落在重叠区中点（与 D4 当前口径一致：从中心线起算、不扣外径；表里另给扣两侧外径后的净距
    与重叠长度）。写进「BRAN 中心线最近清距」同一份结果——同一目标 BRAN 可有多条（一对一条），再测同一对整组替换；来源登记
    `parallel-centerline / exact-centerline`，尺寸文字不带「≈」。没有平行直段就只报状态、不写。
  - 旧的网格采样管-管路径删掉（`computeApproxNearestBetweenObjects` 留给「构件最近点」工具）。
  - **真机验出并修掉一处前提错误（2026-09-17）**：点选 → 所属 BRAN 靠 DTX 缓存的 `owner_noun == BRAN`，而真机 `model/records`
    从不发 `generic=BRAN` 的记录——BRAN 的隐式管身 11 条都挂在它自己的 refno 上、`generic=TUBI`，`instanceMapping.buildNounIndex`
    照字面收成「这个 refno 是 TUBI」，整条 BRAN 下所有构件的 `owner_noun` 都成了 `TUBI`，点哪根都答「不属于任何 BRAN」。改成
    `TUBI` 记录只说明「这个 refno 是一条 BRAN」（非 `TUBI` 的 generic 优先），加了真机形状的用例。e2e（:3111 + :8023，`24381_145018 ↔
    24381_144924`）：6 对平行直段、6 条中心距尺寸（1584 / 1920 / 1980 / 2719 / 1920 / 6829 mm）落在轴线之间；反着再点同一对整组替换仍 6 条。

### 3.4 明确不做

- 不碰 SurrealDB / `pe_owner`（v1 的成员关系用投影子树 + `branch_query`）；不新增 SQLite 索引；不写 `GLOBAL_AABB_TREE`。
- SCTN / GENSEC 按轴向过滤「只要竖直的当柱」——需要成员姿态，另立一条（P4）。
- legacy 后端不再改。

## 4. 待拍板（Plannotator 里逐条批注）

- **D1** `plant-model-gen` 里那三轮改动：留 / 回退？
- **D2** `plant3d-web` 已改 12 文件：保留骨架按 §1 走 / 全部回退重来？
- **D3** 中心线模式 `include_self` 缺省：**false**（净距场景没人要自己）/ 与现有 `/nearby` 一致缺省 true？
- **D4** 净距口径：只做中心线→候选盒（`surface=0`），还是缺省扣外径（`surface=1`）？目标侧仍是 AABB，精算留给前端网格（36 方案级 2）。
- **D5** 前端分流方式：`useSpatialCompute` 内按 kind 分流（改动最小）/ 进 `SpatialSource` 端口（架构一致）。
- **D6** `column` 组是否含 GENSEC；预置组要不要开成服务端配置。
- **D7** 端点命名：`/api/v1/spatial/nearest-clearance`（与 legacy 同名，前端零改）/ `/api/v1/clearance/bran`。
- **D8** 先做 P1 还是 P2：P1 是抽屉那一档（发现阶段），P2 是尺寸标注（标注阶段）；两者共享中心线取法，建议 **P1 → P2 → P3** 顺序，各自单独可验收。

## 5. 验证口径

- 单测：`spatial_query.rs` 夹具树（`4000000011/*` refno 段）加中心线走廊用例（自身剔除、走廊 vs 整盒命中、cube/sphere）；`spatial_clearance.rs` 纯函数用例（段↔盒最近点、分桶截断、`noun_counts`、`exclude_nouns`、`surface` 扣径、负几何默认剔 / 显式放行）；`web_service/mod.rs` 只读路由钉加两条路由。
- `cargo test --lib` 不新增失败；`cargo clippy --lib` 本仓零告警（沿用 ADR-079 口径）。
- 真机：用现有 0.1.27 部署方式起本仓（`aios-database serve`，`http_port` 见 `db_options`），对 AGENTS 测试 BRAN **`24381_145018`**：
  `GET /api/v1/spatial/nearby?refno=24381_145018&source_mode=bran_centerline&radius=1500&include_self=false` 看 `center.source=bran_centerline`、结果里没有它自己的 TUBI/ELBO；
  `GET /api/v1/spatial/nearest-clearance?source_refno=24381_145018&group_by=noun&radius=1500&max_per_group=3&debug=1` 看 `noun_counts`、`excluded_self_members>0`、每桶带 `annotation`。
- 前端：`npm test`、`npm run type-check`（基线外 0 新增）、抽屉切到 gen-model-v1 数据源后「沿 BRAN 中心线」一档可用；尺寸系统能按 `bran-clearance` external source 画出最近墙 / 柱那两条。

## 6. 工作量与顺序（估）

| 段 | 内容 | 规模 |
| --- | --- | --- |
| P1 | `Target::BranCenterline` + 走廊候选 + 成员并集 + 两条路由参数 + 单测 + 前端能力翻 true | 后端 ~300 行 + 测 ~150；前端 ~40 行 |
| P2 | `spatial_clearance.rs` + handler + 路由钉 + 单测 | 后端 ~600 行 + 测 ~300 |
| P3 | v1 API 函数 + 分流 + Dock facet | 前端 ~200 行 |
