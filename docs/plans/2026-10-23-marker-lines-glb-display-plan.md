# 标记线进界面：节点三段线与 37 组只有线的记录按「标记线」单独存、GLB 按线导、前端画线（不进体积 / 碰撞 / 房间 / 空间树）

日期：2026-10-23；状态：**草稿，待审**（口径已定：按 A 记账；开工时机见 D9——等 mesh-purge（`handlers.rs`）与 `e3d_mesh_store` / `mesh_glb` 在途改动合入 `refactor` 后）；起草：Cursor 代理（swe2-bridge 会话）；批准位：用户（Plannotator）。

> 口径来源：2026-10-23 工作线汇报「gen-model 界面里要不要显示节点线」给出 A / B / C 三选，用户回「按推荐继续」——取 **A**：
> gen-model 加一类「标记线」，存每只节点的三段线，GLB 里按线导出，前端画线；与 E3D 一致，不进体积、碰撞、房间统计。
> **B（三根细实体杆）否决**：会进体积 / 碰撞 / 房间 / 空间树 / 净距，落库后换成 A 还要清旧网格。**过渡期按 C**（只保留 RVM 里的线）。
> E3D 另有 37 组只有线的记录（轴网 / 电缆 / 控制节点 21 组、无规格 SCTN 中心线 6 根、踢脚板基准线 10 组）与节点线**同一条通道一起做**。
>
> 目标：把 A 落成可执行的步骤、验收标准与决策点。审核基线：`refactor@13ffb716d` + 工作区未提交改动（mesh-purge；`e3d_mesh_store` / `mesh_glb` 在途）。
> 本计划**只写文档**，不碰源文件；请在 Plannotator 里对 §3 批注，再整体批准或驳回。批准后按 §5 开工；§6 的门由你跑。

## 1. 出发点

现码事实（行号按 2026-10-23 工作区）：

| # | 事实 | 出处 |
|---|---|---|
| F1 | GLB 只会写**一个** `MODE_TRIANGLES`(4) primitive（positions / normals / indices）；空网格在写字节之前就被拒 | `src/web_service/mesh_glb.rs:23, 31`；测试 `degenerate_meshes_are_rejected_before_any_bytes_are_written` |
| F2 | 网格文件是内容寻址的 `PlantMesh`：摘要只覆盖 vertices / normals / indices 三样，键要过 `is_valid_mesh_key`，校验 / 修复 / 并发发布都按三角网格写；没有「线段」这一类 | `src/fast_model/e3d_mesh_store.rs:73-114, 222`；测试 `baked_id_covers_vertices_normals_and_indices_exactly` |
| F3 | 空间树候选只来自 `converge_tree_after_persist` 进过树的盒；房间归属读投影记录现算；`nearby/branches` 对 `mesh_id` 为空 / 负实体 / `tube.invalid` 的记录跳过 | spec §4.13「覆盖面」「房间归属」；`src/fast_model/spatial_branches.rs:93-99` |
| F4 | RVM 对拍里「成员的图元全是 `Line`」记 `exempt_centreline_only`、按两边一致处理、不算缺件；导入侧认 `GeometryKind::Line` | `src/rvm_baseline/compare.rs:436-441`、`src/rvm_baseline/import.rs:935` |
| F5 | 烘焙要求每件都有实体，线只进 RVM 导出（工作线汇报）；没有模型的根会进水位欠账与 ISSUE-029 连败判死 | 判点出处**待取证**（W0-1）；`changelog.md` 2026-10-22 ISSUE-029 条 |
| F6 | mesh-purge 在途：`inst_geo` 行 + `.mesh` 随范围删除、单位几何硬排除、`ClearGeometry::{Keep, Purge}`、「收键 → 删行 → 删几何」 | `docs/plans/2026-10-22-mesh-purge-shared-geometry-fix-plan.md`、`src/data_interface/mesh_purge.rs` |
| F7 | 清单：节点每只 3 段线；另有 37 组只有线的记录——`/GRID-STABILIZER`、`/CABLE00x`、`/RN001`、`/ControlNode1` 共 21 组，无规格 SCTN 中心线 6 根，踢脚板基准线 10 组 | 工作线汇报 2026-10-23；ISSUE_13 草稿 / 汇总贴 03 |
| F8 | `spatial_branches.rs` 模块文档把「隐式管身不进空间树」的出处写成 ADR-072，而 ADR-072 讲的是发布阶段退役与写入标记；该口径实际在 spec §4.13（计划 2026-09-13 §5-5） | `src/fast_model/spatial_branches.rs:1-2`、`docs/adr/ADR-072-…md:1`（顺手校正，归 W6） |

两条不随决策点变的事：

1. **标记线不进体积 / 碰撞 / 房间 / 空间树 / 净距**（`/nearby`、`nearby/branches`、`nearest-clearance`、`surface-clearance`、房间归属、体积表都不该看到它）。
2. **只有标记线的件不算「没有模型」**：轴网 / ControlNode 这类件不能再进水位欠账或 ISSUE-029 连败判死。

## 2. 待解决清单与归属

| # | 问题 | 工作包 | 严重度 | 状态 |
|---|---|---|---|---|
| 1 | 没有存「线」的地方（F2） | W1 | 阻塞 | 待做 |
| 2 | 生成侧只把线写进 RVM，投影 / 库里没有（F5） | W2 | 高 | 待做 |
| 3 | GLB 只导三角面（F1） | W3 | 高 | 待做 |
| 4 | 统计 / 树 / 房间 / 净距要显式跳过新类（F3） | W4 | 高 | 待做 |
| 5 | 随范围删除 / 重生成的路（F6） | W5 | 中 | 待做（D7） |
| 6 | 「没有模型」判定、`records` 形状、spec / changelog（F5、F8） | W6 | 中 | 待做（D5） |
| 7 | 前端画线（另仓 plant3d-web） | W7 | 高 | 待取证（W0-3） |
| 8 | RVM 对拍豁免变真对拍（F4） | W8 | 低 | 待做（D6） |

## 3. 决策点（请批注；不批注即按推荐）

| ID | 问题 | 选项 | 推荐 |
|---|---|---|---|
| D1 | 存储形态 | (a) 独立一类：投影记录多一档「标记线」（`mesh_id` 空、`marker_line_id` 非空），库上单独一张内容寻址行，文件 `.lines`（段表：`[start, end]` × N，f32 mm 世界坐标），键与 `.mesh` 分空间；(b) 往 `PlantMesh` 加 `lines` 字段、复用 `.mesh` | **(a)**：F2 的摘要 / 校验 / 修复 / purge 全按三角网格写，(b) 每一条都要改且语义混；独立一类让 F3 的所有统计天然跳过 |
| D2 | 分类 | (a) 每条标记线带 `kind`：`node`（节点三段线）/ `grid`（轴网）/ `centerline`（无规格 SCTN）/ `datum`（踢脚板基准线）/ `route`（电缆 / 控制节点），由生成侧按 E3D 元素类型 / 目录出身打；(b) 不分类 | **(a)**：前端按 kind 开关，默认只开 `grid`；节点三段线多数场景要关 |
| D3 | GLB 出口 | (a) 同一条 `GET /api/v1/meshes/{id}.glb`：键落在标记线空间就回 `mode: LINES`(1) 的 primitive（只有 POSITION，无 normals）；(b) 单独路由 `/api/v1/marker-lines/{id}.glb` | **(a)**：客户端加载管线一条；`mesh_glb` 按类分支，「退化」判据对线改成 ≥ 1 段 |
| D4 | 统计隔离 | (a) 树入口、房间归属、`spatial_query` / `spatial_clearance` / `surface_clearance` / `spatial_branches`、体积表显式跳过 + 源码钉 `marker_lines_never_enter_the_tree_or_statistics`；(b) 靠 `mesh_id` 为空被动跳过 | **(a)**：被动跳过是巧合，哪天有人把记录读全就漏进去 |
| D5 | 「没有模型」判定 | (a) 只有标记线的件视为「有模型」：水位 / ISSUE-029 / `/model/records` 都按有模型记，`records` 条目 `mesh_id` 空、`marker_line_id` 非空；(b) 维持「没有模型」 | **(a)**：否则轴网 / ControlNode 永远欠账、连败判死把它们标「已放弃」 |
| D6 | RVM 对拍 | (a) A 落地后 37 组从 `exempt_centreline_only` 改成真对拍（记录种类 `Line` 条数一致、端点在公差内），豁免只留给生成侧确实不出线的件；(b) 维持豁免 | **(a)**，当验收项 |
| D7 | 删除 / 重生成 | (a) `.lines` 行与文件并进 mesh-purge 的 `purge_geometry`（收键 → 删行 → 删文件），无共享单位键；(b) 另起一套 | **(a)**：同一条纪律、同一份回执（多一格 `marker_line_files_removed`） |
| D8 | 前端第一版 | (a) `LineSegments` 1px、按 kind 开关、不参与拾取 / 选中 / X 光材质；(b) Line2 粗线 + 可拾取 | **(a)**：WebGL 线宽恒 1px，粗线要换 Line2，先把通道打通 |
| D9 | 开工时机 | (a) 等 mesh-purge（`handlers.rs`）与 `e3d_mesh_store` / `mesh_glb` 在途改动合入 `refactor` 后，在干净检出上开；(b) 现在在独立 worktree 开 | **(a)**：三个文件都在别人手里，(b) 必冲突 |

不当决策点的：B 方案（否决，理由见引言）；过渡期做法（C）；0.1.43 不动。

## 4. 工作包

### W0 取证（不改代码，结论写进 §9）

1. 「每件都有实体」的判点在哪：生成收尾里「没有网格就报错 / 记没有模型」的那几处（`e3d_model_service.rs` / `cata_model.rs` / `model_root_ledger.rs`），逐处写明是「报错」还是「记账」，补进 F5。
2. `PlantMesh`（aios-core）的定义与 `ProjectedShape` 枚举现有档位；`ModelMemoryStore` 记录有没有现成的「非网格」档可套。
3. plant3d-web 加载 / 合批 / 实例化路径对 `mode: LINES` primitive 的处理：是否只认 `Mesh`、遇 `LineSegments` 会不会静默丢；拾取 / X 光 / 仅显示模式对线的材质（plant3d-web 不在本目录，要到那个仓里看）。
4. e3d-model 生成侧线从哪出：`gen_ams` 写 RVM `Line` 的入口、节点三段线和 37 组各自的来源元素 / 目录图元，确认能以「段表 + kind」的形状交给 gen-model。
5. 体积表 / 材料表读的是哪张表（`gy_common.surql` 等），确认新行不会被汇总进去。

### W1 存储（`src/fast_model/e3d_mesh_store.rs` + 库表）

- 新类型 `MarkerLines { kind, segments: Vec<[Vec3; 2]> }`；内容寻址 `marker_line_id`（摘要域单独一个常量，不和 `BAKED_MESH_DOMAIN` 混），文件 `<meshes_dir>/<id>.lines`，写 / 校验 / 修复复用 `ensure_mesh_file` 的 stage → rename → verify 套路（抽公共函数，不复制）。
- 库上一张 `marker_line:⟨id⟩` 内容寻址行 + 投影记录多 `marker_line_id`（`mesh_id` 空）。
- 单测：摘要覆盖 kind + 全部端点；损坏文件原地重写；并发发布只写一次。

### W2 生成侧（`e3d_model_service.rs` / `cata_model.rs` / `pdms_inst.rs`，配合 e3d-model）

- 按 W0-4 的形状把节点三段线与 37 组线从 e3d-model 带出，打 kind，写成标记线记录；这些件的「有模型」判定按 D5。
- 生成收尾里「没有网格」的判点（W0-1）改成「没有网格也没有标记线」。

### W3 GLB（`src/web_service/mesh_glb.rs` + `handlers.rs` 的 `mesh_glb`）

- `marker_lines_to_glb`：一个 `mode: 1` primitive，只有 POSITION（2N 点，不用 indices）；空段表拒。路由按键所在空间分支（D3）；404 仍是可负缓存的档。
- 单测：一段线 → 一个 LINES primitive；JSON 块对齐；kind 落在 `extras`。

### W4 统计隔离

- 树入口（`converge_tree_after_persist`）、房间归属（`MemoryRoomCalculator`）、`spatial_query` / `spatial_clearance` / `surface_clearance` / `spatial_branches` / `spatial_tree_tubes`、体积表显式跳过标记线记录；源码钉 `marker_lines_never_enter_the_tree_or_statistics`（每一处选择器点名）。

### W5 删除 / 重生成（`src/data_interface/mesh_purge.rs`，等 F6 合入）

- 收键 SQL 多收 `marker_line_id`；`purge_geometry` 删 `marker_line:⟨id⟩` 行与 `.lines` 文件；回执多一格；`shared_unit_keys` 不涉及（标记线没有单位键）。

### W6 接口与文档

- `/model/records` 条目多 `marker_line_id` / `marker_kind`（老客户端不读就不受影响）；spec §4.5 / §4.7 / `mesh_glb` 条目同步；`changelog.md` 一段；`spatial_branches.rs:1-2` 的 ADR-072 引用改指 spec §4.13（F8）。

### W7 前端（plant3d-web，另仓）

- 按 D8：加载器认 LINES primitive → `LineSegments`；按 kind 开关（默认只开 grid）；不参与拾取 / 选中 / X 光。

### W8 RVM 对拍（`src/rvm_baseline/compare.rs`）

- 按 D6：生成侧有标记线的成员不再走 `exempt_centreline_only`，改比 `Line` 条数与端点；豁免计数应从 37 组降到 0（除非 W0-4 发现生成侧确实出不了的）。

## 5. 执行顺序与里程碑

| 里程碑 | 内容 | 产出 |
|---|---|---|
| M0 | W0 取证 | §9 结论；F5 判点清单；前端对 LINES 的处理结论；D1–D5 定稿 |
| M1 | W1 存储 + 单测 | 提交 1 |
| M2 | W3 GLB + W4 隔离钉 | 提交 2（此时库里还没有标记线，行为不变） |
| M3 | W2 生成侧（先轴网 `grid`，再节点 `node`，再其余） | 提交 3–5，每类一个 |
| M4 | W5 删除 / 重生成 + W6 文档 | 提交 6 |
| M5 | W8 对拍 | 提交 7 |
| M6 | W7 前端（另仓） | plant3d-web 提交 |

每个里程碑一个提交，`git revert` 即回。不进已验证的 0.1.43（`7fdc4c15c`）。

## 6. 回归门（每一步合入前，由你跑）

| ID | 断言 | 命令 |
|---|---|---|
| V1 | 存储单测全过 | `cargo test --lib -- e3d_mesh_store` |
| V2 | GLB 单测全过，现有三角网格用例不变 | `cargo test --lib -- mesh_glb` |
| V3 | 隔离钉与空间用例全过 | `cargo test --lib -- spatial`、`cargo test --lib -- room_resident`、`cargo test --lib -- mesh_purge` |
| V4 | 全量不变差 | `cargo test --lib` |
| V5 | 服务端二进制按 CI 同款特性集编过 | `cargo check --bin aios-database --no-default-features --features ws,gen_model,manifold,project_hd,http_api` |
| V6 | 手工：含轴网的库生成后 `GET /api/v1/meshes/<marker_line_id>.glb` 的 JSON 块里 primitive `mode == 1`；`/nearby`、`nearby/branches`、`nearest-clearance` 对同一点的结果与改前逐字相同 | curl |
| V7 | 手工：只有标记线的根不进水位欠账、不进 ISSUE-029 连败；`/model/records` 条目带 `marker_line_id` | curl + 日志 |
| V8 | RVM 对拍：37 组记录种类一致，`exempt_centreline_only` 降到 0；其余判档不变 | `rvm_compare` |
| V9 | 前端：轴网线可见、开关生效、拾取不命中线 | 浏览器 |

任一变差就停：保留命令与输出，回到对应工作包重议。

## 7. 风险与不做事项

1. 前端在另一个仓：W7 没落地前，后端已导的线在界面上仍不可见（与 C 等价，不是回退）。
2. 线宽 1px；要粗线另起计划（Line2）。
3. D5 把「有模型」的定义放宽了一档：水位语义从「有网格」变成「有网格或标记线」，要在 ADR-079 补一条修订注记。
4. 已有库的标记线要重生成才有；不做后台 sweep，这是与 mesh-purge 同一条「靠重生成写回」的口径。
5. 不做：B 方案；线的拾取 / 选中；把线计入任何统计；改 0.1.43；本计划内不跑编译 / 测试（§6 由你跑）。

## 8. 批准栏

- [ ] 批准：按 §4–§6 执行（决策点未批注的按推荐项）
- [ ] 驳回 / 改口径：

## 9. 执行记录

- 2026-10-23 · 起草。依据：工作线汇报（A / B / C）+ 用户「按推荐继续」；未开工，按 D9 等在途改动合入。本计划未改任何源文件。
