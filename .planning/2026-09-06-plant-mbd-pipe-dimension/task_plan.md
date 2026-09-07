# Plannator 开发计划：plant-mbd —— 管道尺寸标注新库（PML MBD → Rust 求解器 → gen-model API → plant3d-web）

> 计划 ID：`2026-09-06-plant-mbd-pipe-dimension`
> 创建：2026-09-06
> 状态：**in-progress**（plannotator 门禁 2026-09-06 approved，见 `gate_result.json`；Phase 0/1 complete，Phase 2 端点与 HTTP 门已过、浏览器目测待用户起服务，Phase 3 金样六条已固化，Phase 4 线性尺寸已按 PML isoDim 模型重做并重录金样（2026-09-07），Phase 5 ① ATTA 子尺寸已落地（2026-09-07），PML 图纸人工对照待做；事实与裁决见 `findings.md`）
> 需求（用户 2026-09-06）：新建库 `plant-mbd`，实现 `old/vendor/MBD` 里的 MBD 尺寸标注能力；**第一目标：管道尺寸标注**；经本仓（gen-model）API 提供 JSON，由 `old/plant3d-web` 加载显示。
> 权威：PML 事实源 `old/vendor/MBD/markpipe/**`、`old/vendor/MBD/object/mbd/**`；前端契约 `old/plant3d-web/src/dimension/adapters/mbdV2Contract.ts`（plant3d-web ADR 0041/0043/0046）；先行 Rust 实现 `D:\work\plant-code\rs-mbd`（`contract.rs` / `split_isolines.rs` / `direction.rs` / `layout.rs` + fixtures，HEAD `b6b1027`）；本仓 ADR-053（direct 读）、ADR-054（默认时点 = 文件最新）、ADR-057（e3d-model 纯模型面，gen-model 持状态与副作用）、ADR-059（池级元素缓存）。
> 范围：`old/vendor/plant-mbd`（新仓）+ `old/gen-model`（数据适配层 + HTTP 端点）+ `old/plant3d-web`（首发零代码，只用 URL 参数；契约扩展期才改映射）。

## 0. 现状分析（2026-09-06 已核实）

### 0.1 `old/vendor/MBD` 是什么

AVEVA E3D/PDMS 的 PML 宏包（`.pmlobj/.pmlfrm/.pmlfnc`，`mbd.uic` 工具条入口），按专业分 `markpipe/`（管道）、`markinstpipe/`（仪表管）、`mark*supp/`（支架）、`form/markEqui|Civil|ArchForm`（设备/土建/建筑）。**管道链路**（`开发文档/管道标注绘制流程.md`）：

```
markpipeform.pmlfrm  →  object isobran(bran)              # 一条 BRAN 一个 isobran
  .getbranmems()   L236    coll all bran mem（TUBI/ELBO/BEND/ATTA/WELD…，首尾插 Head/Tail）
  .getisolines()   L1470   按弯头把成员切成若干 isoline（直段）
  .draw()          L2999   每个 isoline：选尺寸方向(isoGetDimDir/isoGetHandleDimDir/isoFindIsoUsedDir)
                           → lindim（线性尺寸）/ mlabel（标签）/ aidline|aidtext|aidpoint|aidarc|aidcircle
                           → isoSlope（坡度）/ isombdangle（角度）/ isoWeldText（焊缝）/ isoTag（位号）
  .getJsonData()           材料/焊缝/切管/属性 表格 JSON（TPlant 交付物，不是图元）
```

| PML 对象（大小） | 职责 | 新库落点 |
|---|---|---|
| `markpipe/object/isobran.pmlobj`（193 KB） | 编排：成员采集、切段、绘制、材料/焊缝/切管统计、JSON 导出 | 切段 → `split`；编排 → `layout_branch`；统计/导出 → **后置**（Phase 6） |
| `markpipe/object/isoline.pmlobj`（157 KB） | 直段布局：dimdir / usedDirs（`getIsoUsedDirs` L439）/ chardirs、Polar 邻段、坡度与斜管分解 | `direction` + `layout` |
| `markpipe/object/isoDim.pmlobj`（48 KB）、`object/mbd/lindim.pmlobj` | 尺寸点 → 尺寸线 / 延长线 / 箭头 / 文字（`lindim.pmlobj:312-320` 数值格式） | `MbdPrimitive::LinearDim` 显式几何 |
| `markpipe/function/isoGetDimDir|isoGetBestDir|isoGetHandleDimDir|isoFindIsoUsedDir.pmlfnc` | 尺寸方向选择系统 | `direction.rs`（rs-mbd 已移植成纯函数，**未接线**） |
| `object/mbd/mlabel|mbdaidline|aidtext|aidpoint|aidarc|aidcircle.pmlobj` | 输出图元 | V2 契约 `label / leader_line / aid_line / aid_text / aid_point / aid_arc / aid_circle` |
| `markpipe/object/isoSlope|isombdangle|isoWeldText|isoTag|isoadjustweld.pmlobj` | 几何伴生符号与非尺寸图元 | `slope_mark / angle_dim / weld_mark / label`（Phase 5 分期） |
| `object/atta/pipeatta.pmlobj`、`object/polarsystem/*` | ATTA 几何、Polar 占位 | Phase 5 |
| `function/export/*`、`totplant/*` | RVM/ATT/JSON 落盘与 TPlant 上传 | **不迁**（前端直接消费 JSON） |

### 0.2 已有的 Rust 先行实现：`D:\work\plant-code\rs-mbd`

绿场 workspace（`crates/rs-mbd` + `crates/rs-mbd-cli`），最后提交 `b6b1027`。已落地：冻结的 V2 契约 `contract.rs`（与前端 1:1）；`split_isolines.rs`（对齐 `isobran.getisolines` L1470-1794 的弯头主路径，p1 fixtures 13 案例）；`direction.rs`（`get_dim_dir / get_handle_dim_dir / get_best_dir` 纯函数 + 测试，**未接入 layout**）；`connectivity.rs`（缺端口管件按端口连通性补轴）；`layout.rs`（线性 MVP：member / chain / small_dim / suppress，方向仍是朴素叉乘 `deterministic_normal`）；`docs/field-dictionary.md`（`BranchMember` 字段 ↔ PML 行号）；11 份现场 fixture `fixtures/field-*.json`（含 `24383_99996`：31 成员 = 5 BEND + 3 ELBO + 9 FTUB + 13 ATTA）。

**它卡在哪**（rs-mbd `.planning/2026-08-03-mbd-dimension-review-fixes/task_plan.md`）：数据源与宿主——依赖 `aios_core`（rs-core，从 SurrealDB `fetch_branch_members` 取数），端到端被 SurrealDB 2.1.4 vs 3.1 fork 的版本错配挡住；Phase 2 金样、Phase 3 方向系统接线、Phase 5 扩面全部 pending。

**可复用**：纯模块不引 `aios_core`（`rg aios_core crates/rs-mbd/src` 仅命中 `core_adapter.rs` 与两处文档注释）——`contract.rs / data_source.rs / split_isolines.rs / direction.rs / connectivity.rs / layout.rs` 与全部 fixtures 可整体搬入。**不可复用**：`core_adapter.rs`（Surreal 段视图 / 成员视图适配）、`plant-web-server` 宿主。

### 0.3 前端 `old/plant3d-web` 已就绪（首发零代码）

- `src/api/mbdV2Api.ts::fetchMbdV2PipeData(refno)` → `GET {mbdBase}/api/mbd/v2/pipe/{refno}`；`mbdBase` 由 URL `?mbdBackend=<url>` 或 `?mbdBackendPort=<port>`（同 hostname）或 `VITE_MBD_API_BASE_URL` 决定，**与主后端（3100）解耦**。
- `src/composables/useMbdExternalSync.ts`：URL 带 `mbd_refno=<refno>` 走实时通道；`parseMbdV2PipeData` 严格校验，**任一图元不可映射即整包拒绝**（ADR 0046）；`angle_dim / aid_arc / aid_circle` 在冻结契约里无几何，出现即拒（`CONTRACT_INCOMPLETE_KINDS`）。
- `meta.layout_mode` 显示在尺寸面板，`linear_mvp` 触发「本分支标注不完整」警示条；`meta.geometry_space="source_mm"` 时必须给 `source_to_design`（16 元素**列主序**，`fixtures/mbd-v2/source-to-design-translation.json` 钉死）。

### 0.4 本仓 gen-model 能给什么

- **HTTP 面**：`src/web_service/mod.rs::serve` 的 axum `Router`（`/api/v1/*`、`/api/v1/ws`），`AppState`、`ApiError{not_found / from_domain}`；监听 `DbOption.toml` `http_api_addr = "0.0.0.0:8022"`，`http_api_cors = ["*"]`；feature `http_api`（默认开）。
- **数据面**：直读 E3D `.dat`（e3d-io `DbSet`）；`E3dModelService::build_set(dbnum, sesno)`（`src/fast_model/e3d_model_service.rs:1343`）按水位钉库；`parse_refno`（L1978）同时接受 `24384/2944` 与 `24384_2944`。当前项目 `AvevaMarineSample` / MDB `ALL`。
- **模型面 = PML `getbranmems` 的天然等价物**：`vendor/e3d-model/src/route.rs`
  - `walk_route(container) -> Route { members（站点成员·文件原序）, non_route_members（穿过件：目录 GTYP≠ANCI 的 ATTA 等）, tubes: Vec<TubeSlot{from: RouteEnd, to: RouteEnd}> }`，`tubes.len() == members.len() + 1`；
  - `resolve_end(RouteEnd::MemberArrive|MemberLeave{member, ppoint}, &CataloguePoints) -> RoutePoint{pos, dir, bore}`：走 `SPRE → SPCO.CATR → SCOM.PTRE → PTAX/PTCA` 解 P 点、`PDIS/PBOR` 表达式求值、世界系 mm——即 PML `apos/adir/lpos/ldir of $!mem wrt /*`；
  - `container_head/tail` = `HPOS/HDIR/HBOR`、`TPOS/TDIR/TBOR`；`tube_outer_diameter(catalogue_od, insulation, arrive_bore)` = `CGETOD`（aod）；`transform::world_matrix(el)` 平移列 = `pos of $!mem`；
  - 这正是 rs-mbd 8 月被卡的「成员结点视图」`{noun, adir, ldir, apos, lpos, pos}`——在本仓不需要 SurrealDB，从库文件直接算出。

### 0.5 结论：分层

```
old/vendor/MBD (PML 事实源)                  old/plant3d-web (呈现，已就绪)
        │ 语义对齐                                   ▲ GET /api/mbd/v2/pipe/{refno} → MbdV2PipeData JSON
        ▼                                            │
old/vendor/plant-mbd ── 纯求解器 ──►  old/gen-model  src/mbd/ (e3d-model Route → BranchQueryResult)
  contract / split / direction /                    src/web_service (路由 + 错误语义)
  layout / cli (fixture → json, golden diff)        E3dModelService::build_set → DbSet → DbElement
```

## 1. 目标 / 非目标

**目标（本计划）**：一条真实 BRAN 的 refno 进 `GET /api/mbd/v2/pipe/{refno}`，出符合冻结 V2 契约的线性尺寸（逐直段、含弯头切段）JSON；`old/plant3d-web` 以 `?mbd_refno=<refno>&mbdBackendPort=8022` 打开即显示；CLI 金样回归护航；随后按 PML 顺序补方向系统与几何伴生符号。

**非目标**：支架 / 设备 / 土建 / 仪表管专用逻辑；材料 / 焊缝 / 切管**表格** JSON（`isobran.getJsonData` 系）；RVM/ATT 生成与 TPlant 上传；前端渲染改动（契约扩展期的映射除外）；用户可编辑尺寸（外部尺寸只读，plant3d-web ADR 0028）。

## 2. 架构决策（待门禁拍板；每条附替代方案）

| # | 决策 | 理由 | 被否决的替代 |
|---|---|---|---|
| D1 | **新仓** `D:\work\plant-code\old\vendor\plant-mbd`（独立 git），workspace：`crates/plant-mbd`（纯求解器 lib）+ `crates/plant-mbd-cli`（fixture → JSON / golden diff）；gen-model 以 path 依赖 `../vendor/plant-mbd/crates/plant-mbd`，与 `e3d-model` / `e3d-io` 同款（`Cargo.toml` L167-173 已有先例与说明） | 与既有 vendor 布局一致；CLI 离线回归不需要库文件也不需要服务 | 做成 gen-model 子模块（无法离线回归、无法被别的宿主复用）；git+rev 依赖（两仓并排演进，无可钉上游） |
| D2 | **契约 1:1 复用冻结的 V2 形状**（`MbdV2PipeData` / `MbdPrimitive` 的 kind 与字段名不变）；`meta.geometry_space="source_mm"`，`source_to_design` = 列主序 `diag(0.001)`；`layout_mode` 如实标 `linear_mvp` 直到方向系统接线 | 前端零改动；契约漂移守卫（`mbdV2Contract.test.ts`）继续有效 | 自定义新契约（前端重写适配层，丢掉 ADR 0043/0046 已验证的守卫） |
| D3 | **以复制 rs-mbd 纯模块起步**（`contract.rs` `data_source.rs` `split_isolines.rs` `direction.rs` `connectivity.rs` `layout.rs` + `fixtures/p1-*.json p2-*.json field-*.json`），文件头注明来源提交 `b6b1027`；**不** fork 整仓、不引 `aios_core`、不搬 `core_adapter.rs` | 继承已对齐 PML 的切段语义与 30+ 案例 fixtures；rs-mbd 停摆的原因（数据源 / 宿主）本仓恰好没有 | 从 PML 从零重写（多花 3–5 周，重走一遍 7–8 月已裁决的口径：adir 朝外、near-180 校正、直通件偏轴守卫、缺端口补轴） |
| D4 | **数据适配层放 gen-model**（`src/mbd/branch_query.rs`）：e3d-model `Route` → `plant_mbd::BranchQueryResult`；plant-mbd **不依赖** e3d-io / e3d-model | 求解器保持纯（rs-mbd ADR-0004），CLI / 单测不开库；与 ADR-057「模型面纯、状态与副作用在 gen-model」同一口径 | plant-mbd 直接依赖 e3d-model（耦合库格式，离线回归失效） |
| D5 | 求解器内部几何用 **f64**（与 e3d-model `DVec3` 同口径），契约边界保持 `[f32;3]`；`glam = "0.29"`（与 gen-model / e3d-model 同版；rs-mbd 用 0.30 需降） | rs-mbd findings：f32 下能进方向变更闸门的最小偏角 ≈0.03°，`DIR_CHANGE_EPS_DEG = 0.01°` 形同虚设 | 沿用 f32（把已知精度缺陷带进新库） |
| D6 | 端点 `GET /api/mbd/v2/pipe/{refno}` 挂**现有** Router（同端口 8022）：refno 不存在 → 404 `not_found`；非路由容器（`derives_implied_tubes` 为假）→ 422 `precondition`；可解但零尺寸 → **200 + issues** | 前端对非 200 只显示状态码；200 + issues 能把抑制原因送进尺寸面板（rs-mbd CONTEXT「抑制」四级定位） | rs-mbd 宿主的「< 1 条 linear_dim 即 422」 |
| D7 | 前端首发零代码：`?…&show_dbnum=<dbnum>&show_refno=<refno>&mbd_refno=<refno>&mbdBackendPort=8022`；契约扩展（角度 / 弧几何、文字朝向）**两侧同版**发布——先契约 + 前端映射，后求解器产出 | ADR 0046 fail-closed：后端先出新 kind 会让整页无标注 | 后端先行、前端追赶 |

## 3. 完成判据

- [ ] `old/vendor/plant-mbd`：`cargo test --workspace` 绿；`cargo run -p plant-mbd-cli -- layout --fixture fixtures/p2-linear-dimensions.json --case linear-elbow-two-isolines --data-only` 输出 2 段 `linear_dim`、0 issue；输出经 `old/plant3d-web` 的 `parseMbdV2PipeData` 校验 `ok:true`。
- [ ] gen-model：`src/mbd/branch_query.rs` 对选定真实 BRAN 产出成员序列（弯头 `arrive_axis ≠ leave_axis`、ATTA 穿过件留在序列里、Head/Tail 取 HPOS/HDIR、TPOS/TDIR），有测试钉住 **adir/ldir 朝外**口径（`next.arrive_dir == -prev.leave_dir`）。
- [ ] `curl http://127.0.0.1:8022/api/mbd/v2/pipe/<refno>` → 200，`primitives` 含 ≥ 2 条逐段 `linear_dim`，`meta.notes` 含 isoline 计数；404 / 422 语义按 D6。
- [ ] `old/plant3d-web` 以 D7 URL 打开，尺寸随相机跟随真实锚点，尺寸面板显示 `layout_mode` 与 issues；`npx vitest run src/dimension src/api/mbdV2Api.test.ts src/composables/useMbdExternalSync.test.ts` 保持全绿（不改前端代码）。
- [ ] 3–5 条真实 BRAN 求解结果固化为 `plant-mbd/fixtures/golden/*.json`，一键重放 diff 为零；每条附 PML 逐行对照结论（切段数、尺寸点、数值）。
- [x] Phase 4 后：`layout.rs` 无 `deterministic_normal`，偏移由 `od` / `cheight_mm` 派生，`layout_mode` 升级为 `isodim_main`（前端警示条消失）。
- [ ] 本仓 `docs/adr/` 新增一条 ADR（MBD 求解在 vendor 纯库、数据适配与 HTTP 在 gen-model），`CONTEXT.md` 增补「管道尺寸标注 / 直段 / 抑制 / 成员结点视图」词条（自 rs-mbd `CONTEXT.md` 迁入并按本仓口径改写）。

## 4. 分阶段

### Phase 0 — 建仓与契约迁入（≈1.5 人日）

状态：**complete**（2026-09-06；`old/vendor/plant-mbd` 提交 `8055f47`（f32 迁入）+ `d5f09ec`（f64 化））

- [x] `git init old/vendor/plant-mbd`；workspace `Cargo.toml`（edition 2024、`glam 0.29`、serde / serde_json / thiserror / clap）；`crates/plant-mbd/src/{lib,contract,data_source,members,split_isolines,direction,connectivity,layout}.rs` 自 rs-mbd `b6b1027` 复制并去 `aios_core` 痕迹，文件头写来源（`core_adapter.rs` 的纯函数部分落为 `members.rs`，两条 Surreal 适配函数不迁）；`crates/plant-mbd-cli`（`layout --fixture --case [--pretty] [--data-only] [--verbose]`、`golden record / diff`——`record` 默认拒绝覆盖）。
- [x] 先在 f32 下原样跑通迁入（`8055f47`，53 用例迁入即绿），**单独一笔提交**做 f64 化（`d5f09ec`，D5）：`split_isolines / direction / layout` 内部 `Vec3 → DVec3`；契约边界经 `wire()` 转 `[f32;3]`；入口 `BranchMember` 坐标改 `[f64;3]`；补 `tiny_bend_clears_the_direction_gate_in_f64`（0.02° 缓弯：f32 吞成共线，f64 按 PML ②+③ 切三袋）。**既有期望值一个都没改**（toy / field 尺寸文本 / 前端 fixture 逐字节对齐全部保持），不需要按容差重录。
- [x] fixtures 迁入：`p1-split-isolines.json`、`p2-linear-dimensions.json`、`field-*.json`（11 份）；`docs/field-dictionary.md`、`CONTEXT.md`（增「成员结点视图」「端口方向朝外」）、ADR 0001–0005、新 `docs/MIGRATION.md`；「rs-core / Surreal / plant-web-server」改为「gen-model / e3d-model」。
- [x] 契约守卫 `crates/plant-mbd/tests/contract_guard.rs`（前端 fixture 副本在 `fixtures/contract/`）：fixture → `MbdV2PipeData` → JSON 逐字节回环；11 个 kind 字面与 `MbdPrimitiveKind` 一致；求解器对 `linear-small-dimension` 的图元与前端 fixture 逐字节相等（唯一差异 `meta.notes` 的 `allarr` 调试 dump，rs-mbd 2026-08-03 已移除，测试里写明）。
- 验收（已过）：`cargo test --workspace` 54 通过；CLI 三条路径 + `--verbose` + `golden record / diff`（拒绝覆盖路径）实测正常；`verify_mapping` 6/6；`cargo clippy` 仅 4 条 rs-mbd 继承的风格告警（3 个 collapsible-if、1 个 too_many_arguments），未动。

### Phase 1 — gen-model 数据适配：`Route` → `BranchQueryResult`（≈2 人日）

状态：**complete**（2026-09-06；`findings.md`「Phase 1」有口径与验收记录。主工作树因他人 WIP 编不过、共用 target 的 bin 被占，验证在一次性 worktree `gen-model-mbd-wt` + `D:\Rust\target-mbd-wt` 上跑，主树未动他人一行）

- [x] `Cargo.toml` 加 `plant-mbd = { path = "../vendor/plant-mbd/crates/plant-mbd" }`（放在 e3d-model / e3d-io 那段路径依赖注释旁，同口径说明）；`src/lib.rs` `pub mod mbd;`。
- [x] 新模块 `src/mbd/mod.rs`（`layout_pipe` + `SOURCE_TO_DESIGN_MM_TO_M`）+ `src/mbd/branch_query.rs`：
  - 入口 `branch_query(set: &Arc<DbSet>, refno: RefNo) -> Result<BranchQueryResult, MbdAdapterError>`；
  - `element_exists` 为假 → `NotFound`（→ 404）；`derives_implied_tubes(container)` 为假 → `NotRouteContainer`（→ 422）；
  - `walk_route` 后按 `container.members()` **文件原序**合并 `members` 与 `non_route_members`，`order` = 原序下标；
  - 站点成员 k：`arrive = resolve_end(tubes[k].to)`, `leave = resolve_end(tubes[k+1].from)`（`CataloguePoints`）→ `start / end / arrive_axis / leave_axis`；`pos` = `world_matrix(member)` 平移列；`noun` = `element_type()`；`outside_diameter` = `tube_outer_diameter(...)`；
  - 穿过件（ATTA GTYP≠ANCI、WELD 等）：`pos` + `noun`，无方向，**不丢**（切段成员序不能错位——rs-mbd 2026-08-20 教训）；
  - 端口解不出的成员：留在序列、`reliable=false` 进 `issues`（`category=data`），不整条失败（rs-mbd ADR-0002 局部抑制）；
  - Head/Tail：`container_head / tail` → `head_pos / head_dir`（Tail 同理；契约缺字段则在 `split_members` 合成，沿 rs-mbd `branch_members_to_branch_query`）。
- [x] 口径测试 `tests/mbd_branch_query.rs`（开真库 ams7999 + `DirectoryDbResolver` 扫 ams000 解目录库；样本缺席整体跳过，与 `direct_index_contract.rs` 同款，不用 `#[ignore]`）：BRAN `24383/99996` 31 成员文件原序、17 个站点成员两端单位方向、ATTA 穿过件留在序列、朝外不变量唯一违例是已知断点 `100018→100019`、7 个弯头转角 8°–90°、直管声明方向与 apos→lpos 一致、Head 取真实 HPOS/HDIR。
- [x] 交叉核对：与 plant-mbd `fixtures/field-24383_99996.json`（Surreal 路径）24 个成员逐个比对——kind 全同、**位置最差 0.0009 mm、方向最差 0.00000°**；直读多解出 ELBO `100013` 的两端口（Surreal 路径缺），适配层 issue 为 0。
- [x] 端到端 `layout_pipe`：12 直段、10 条线性尺寸文本与 fixture 路径逐条相同、issues 从 3 降到 1。
- 验收（已过）：`cargo test --test mbd_branch_query -- --nocapture` 3/3 绿；结论已写 `findings.md`。

### Phase 2 — HTTP 端点 + 前端联调（≈1 人日）

状态：**端点与 HTTP 门 complete（2026-09-06），浏览器目测待用户起服务**（命令见 `findings.md`「Phase 2」）

- [x] `src/web_service/mod.rs`：`pub mod mbd;` + `.route("/api/mbd/v2/pipe/{refno}", get(mbd::mbd_v2_pipe))`；新文件 `src/web_service/mbd.rs`（不改 WIP 重灾区 `handlers.rs`）：`parse_refno` → 进程内 `OnceCell<Arc<E3dModelService>>` → `build_set(refno.dbno(), None)` → `spawn_blocking(layout_pipe)` → `Json<MbdV2PipeData>`；错误按 D6 映射（`RootError` 422 且 `detail.issues` 带定位）。
- [x] `LayoutRequest`：`source_to_design = diag(0.001)` 列主序、`cheight_mm` 暂 `None`、`layout_mode = "linear_mvp"`、`input_refno` 原样回填（`24383/99996` 写法也原样）、`branch_refno` 归一化为 `w0_w1`。
- [x] `tests/mbd_http_endpoint.rs`：真 axum 回环服务 + reqwest，200（两种 refno 写法）/ 404 / 422 / 400 全过——等价于 §5 三条 `curl`；wire 体经前端 `parseMbdV2PipeData` **ACCEPTED**（10 linear_dim，0 diagnostics）。
- [ ] 浏览器目测（D7 URL）：需要停掉现网旧构建（PID 60444）、起带端点的新构建——用户终端操作，命令在 `findings.md`；截图存本目录。
- 验收：§3 第 3 条已过；第 4 条待浏览器。

### Phase 3 — 真实金样与 PML 语义裁决（≈2 人日，可与 Phase 4 并行开工）

状态：**金样已固化（2026-09-06，plant-mbd `ec7e880`）；PML 图纸人工对照待做**（三条裁决见 `findings.md`「Phase 3」）

- [x] 从 `AvevaMarineSample` 挑 6 条 BRAN 入批次：`24383_99996`（混排 + 源模型断点）、`24383_76191`（17 ELBO 零 FTUB，纯隐式管身）、`24383_91813`（5 共线 FTUB 单直段）、`24383_100028`（BEND 8 + ELBO 4 + 短管 small_dim）、`24381_145018`（rs-mbd 时期「空壳」，直读 28 成员）、`24384_26229`（TUBE 2 = 312.11 mm 对上 E3D 证据）。`24384_2944 / 3277` 因 MDS 支撑 ATTA 目录链在本次 DbSet 走不到 SCOM 暂不收（`adapter.atta_catalogue_unresolved` 出声）。
- [x] gen-model `examples/mbd_branch_dump.rs`（`--db --scan-dir --refno… --out-dir`）直读导出 `BranchQueryResult` → plant-mbd fixture（`$schema_comment` + `cases` + 原始 `branch_query`）；`plant-mbd-cli golden record` 固化；`tests/golden_regression.rs` 让 `cargo test` 逐字段重放。
- [x] **裁决 1 隐式管身**：直读后 13 条候选里 5 条零尺寸（没有 FTUB）——按 E3D 槽位规则在**宿主侧**合成 `TUBI`（长度 ≥ 1 mm、两端共轴门；不画的 ≥ 10 mm 记 `adapter.implied_tube_not_drawn`）；求解器不按空隙猜（`toy-bridge-and-stick` 的 9.95 mm 空隙翻红教训）。`24383_99996` 十条尺寸不变；`24384_26229` 312.11 mm 与 E3D 一致。
- [x] **裁决 2 Tail**：取 `tpos of bran`（PML L1812/L1901/L4255），`BranchQueryResult.tail_pos/tail_dir`。
- [ ] 每条金样对照 PML 图纸：`isobran.getisolines` 切段数与拐点、`lindim` 尺寸点与数值；14–26 mm 短管与「E3D 不画」槽位的 PML 处理——需要在 E3D 里开同一批 BRAN，人工审阅。
- 验收：`golden diff` 6/6 OK、`cargo test --workspace` 56 通过（已过）；PML 对照结论待写。

### Phase 4 — 线性尺寸对齐 PML `isoDim` 模型（含方向系统与 cheight 行位）（≈4–5 人日）

状态：**complete（2026-09-07，用户拍板 A；plant-mbd `287b1f0` ADR-0006、e3d-model `3c5ee02`、gen-model 本提交；口径与对照见 `findings.md`「Phase 4」）**——读 PML 非 Polar 路径后发现 rs-mbd 线性 MVP 的尺寸点模型不是 PML 的（`findings.md`「发现 4」）：PML 每条直段一条 running dimension，尺寸点是**构件原点投影**（ATTA/TUBI/GASK 不进点），不是「每根管一条 + 总长」。

拍板：**(A)** 按 PML isoDim 重做线性尺寸（2026-09-07 用户选定）；(B) 保留 MVP 尺寸点只接方向——否决。

- [x] 宿主补数据：`BranchMember.ppoints`（P1/P2/P3，`CataloguePoints::leave(member, Some(n))`）、`arrive_ppoint / leave_ppoint`、**真外径**（e3d-model 新开 `route::departure_tube_outer_diameter`，取不到按 PML `getod` 回落 `min(bore×1.1, bore+10)`）；`BranchQueryResult.volume_center` 可选（宿主不算，求解器取成员坐标包围盒中心近似）；`mbd_branch_dump` 随之带出，六份 field fixtures 重导（ams7997/ams8000 `--sesno` 钉回 114/323）。
- [x] plant-mbd 新建 `isodim.rs`：按 `isoDim.addmem`（L464-942）取点——Head `hpos` / Tail `tpos` / 构件 `pos` 投影到直段轴；`exceptTypes = ATTA TUBI GASK`；WELD 贴端口不进点（L199-221 / L738-768）；`FLAN/FBLI/REDU` 两端进点；`considerTee`；`possunique` 去重；PML 的偏轴检查（L635-677）→ `isodim.member_off_axis`。每条直段一条 running dimension → N−1 条 `linear_dim`（`sub_kind = "main"`，同一行位）。PCOM / INST / `*DI` 专用取点**未迁**（本批金样没有这类构件，记在 ADR-0006）。
- [x] 行位 `od + 1.2·cheight·(dimtimes−1)`（`drawDim` L1310），`dimtimes` 暂 1（有 ATTA 子尺寸时 PML 走 2，Phase 5 接 `MAIN_DIM_ROW`）；字高按 `isoline.draw` L1689-1691 的真公式 `int(od·0.3·(1−0.5·(od−50)/(|od−50|+100)))` + `changeCheightAuto`，`meta.cheight_mm` 下发（多直段取最大）；小段按 `lindim.sepSmallDim`（缩字 → 往外叠行，`sub_kind = "small"`）。
- [x] 方向：`CalculateDimChardirs`（L1131-1170）→ `good_dim_dirs = [最近两水平反向, U]`；`getIsoUsedDirs`（L439-528）由 P1/P2/P3 判「手柄」→ `DirOccupancy{angle: 60}`（TEE/OLET 按 P1∥pipedir 取 P3 或 P1+P2）+ `addUsedDirToNext` 邻段占位（90°，起点 ±od/2）→ `getusedDirs` 区间过滤（`cutlen`）→ `get_handle_dim_dir(pipe_dir, occupancies, 60, good_dim_dirs)`；`deterministic_normal` 删除（`rg deterministic_normal crates/plant-mbd/src` 无命中 ✓）。
- [x] `layout_mode` 升为 `isodim_main`（`plant_mbd::LAYOUT_MODE_ISODIM_MAIN`），前端 `PARTIAL_LAYOUT_MODES` 只含 `linear_mvp`，警示条自然消失（wire 体经 `parseMbdV2PipeData` ACCEPTED）；ATTA 子尺寸（`getAttaDims`）留 Phase 5。
- [x] 重录六条金样（`plant-mbd/fixtures/golden/README.md`「2026-09-07 基线变更」逐条）；`24383_91813` = Head→Tail 一条 **13500** ✓（第一条对照）。
- [ ] PML 图纸人工对照（ADR-0005）：六条金样的尺寸点与数值 vs E3D 里同一批 BRAN 跑 `markpipe`——需要 E3D。特别看两条：Head 比首根管低 80 mm 的支管 PML 是否真的不给第一袋尺寸；Polar 生产路径（`usepolarsystem = true`）选出的方向与本期非 Polar 选向差多少。
- 验收（已过）：plant-mbd `cargo test --workspace` 71 通过、`golden diff` 6/6、`verify_mapping` 6/6；gen-model worktree `mbd_branch_query` 4/4（含新 `real_branch_members_carry_ppoints_and_catalogue_od`）、`mbd_http_endpoint` 1/1。

### Phase 5 — 几何伴生符号与非尺寸图元（分期立项，每项到时细化）

状态：**in-progress**（① ATTA 子尺寸 complete 2026-09-07 plant-mbd `2d7ca54`；② 坡度 + 斜管分解 complete 2026-09-07 plant-mbd `da4868e`；见 `findings.md`「Phase 5 ①」「Phase 5 ②」）

- [x] **① ATTA 子尺寸**（`isoline.getAttaDims` L1054-1222）：每个 ATTA 归到从上一主尺寸点起步的 attaDim（落在已有 attaDim 内加进它、否则近者归属），子尺寸第 1 行、同 dimdir、`sub_kind = "atta"`；有 attaDim 的直段主尺寸 `dimtimes = 2`（`twoormoredims`）；ATTA 排在首点前放弃直段（`isodim.atta_before_first_point`）；偏轴且 apos 解不出的 ATTA 静默不进点。三条金样再录（`99996` +2、`145018` +6、`26229` 5+1，其第 4 袋折线管路的 4403.29 消失）。按 SPCO 名字的两侧取点未做。
- [x] **② 坡度 + 斜管分解**（2026-09-07，plant-mbd `da4868e`，ADR-0007）：`isoSlope` 三角形 → 斜边 `slope_mark` + 两条 `aid_line`（坡度 [0.001, 0.1]、长度 ≥ 外径的直管，TUBI/FTUB 同角色，坡度按几何算）；`isoDim.dimOneMemSlope` / `dimslope` → 斜管（轴离最近世界轴 > 5.71°）主尺寸首末点间 X/Y/Z 分解腿 `aid_line` + 整数腿长 `aid_text`（`X:/Y:/Z:` 前缀）+ 直角标。五条金样再录。契约零改动。
- [ ] ③ 焊缝 `getwelds` / `isoWeldText` → `weld_mark`
- [ ] ④ 位号 / 材料标签 `getbranmlabel`（L5323）/ `isoTag` → `label` + `leader_line`（先作布局障碍，后输出）
- [ ] ⑤ 角度 `getinstallationangles` + `isombdangle` → **需契约扩展** `angle_dim` / `aid_arc` 几何（PML `aidarc`：pos / ori / radius / stangle / sweepAngle ↔ 前端 `ExplicitArcInput`），两侧同版（D7）
- [ ] ⑥ Polar 生产路径（`usepolarsystem = true`）：`polarsystem.getBestPosAndOri` 选向 + `addtopreornextpolarsystem` 邻段，评估与当前非 Polar 选向的差距再决定是否切换
- [ ] ⑦ 调整焊 `getadjustwelds`；⑧ PCOM / tee-olet 切段（`isoteeline.pmlobj`）

约束：每项落地前先冻结对应 V2 契约几何；新 kind 上线顺序：契约 + 前端映射同版发布 → 求解器开始产出。

### Phase 6 — 后置（另立计划）

材料 / 焊缝 / 切管 / 属性表格 JSON（`isobran.getJsonData / materialsJson / weldJson / cutpipelengthJson`）；仪表管 `markinstpipe`；支架 / 设备 / 土建；`totplant` 上传。

## 5. 验证矩阵

```powershell
# plant-mbd（离线，不开库）
cd D:\work\plant-code\old\vendor\plant-mbd
cargo test --workspace
cargo run -p plant-mbd-cli -- layout --fixture fixtures/p2-linear-dimensions.json --case linear-elbow-two-isolines --data-only
cargo run -p plant-mbd-cli -- golden diff

# gen-model（适配 + 端点）
cd D:\work\plant-code\old\gen-model
cargo test --test mbd_branch_query -- --ignored
# 服务照常启动（http_api 默认开；DbOption.toml http_api_addr = 0.0.0.0:8022）
curl    http://127.0.0.1:8022/api/mbd/v2/pipe/24383_99996
curl -i http://127.0.0.1:8022/api/mbd/v2/pipe/1_1               # 404
curl -i http://127.0.0.1:8022/api/mbd/v2/pipe/<非 BRAN refno>   # 422

# plant3d-web（不改代码：单测回归 + 浏览器）
cd D:\work\plant-code\old\plant3d-web
npx vitest run src/dimension src/api/mbdV2Api.test.ts src/composables/useMbdExternalSync.test.ts
# 用户自己的终端：npm run dev
# http://127.0.0.1:3101/?output_project=AvevaMarineSample&show_dbnum=<dbnum>&show_refno=<refno>&mbd_refno=<refno>&data_source=backend&backendPort=3100&mbdBackendPort=8022
```

## 6. 风险与依赖

- **R1 P 点解析覆盖**：`CataloguePoints` 对部分目录件解不出（`missing_ppoint`），成员退化为无方向。缓解：留在序列 + `reliable=false` + issue；金样阶段统计比例，必要时对 ELBO / BEND 走 `connectivity::infer_missing_fitting_axes` 补轴（rs-mbd 已验证）。
- **R2 adir/ldir 朝向口径**：PML / E3D 的 P 点方向朝外；e3d-model `slot_alignment` 的 `ArriveCheck` 同口径，但必须用测试钉死（Phase 1）——错一次就是整条 BRAN 切段全错。
- **R3 契约 fail-closed**：任何新 kind / 字段先落前端映射再让求解器产出（D7）；plant-mbd 契约测试直接读前端 fixture 防漂移。
- **R4 f64 化改动面**：rs-mbd 三个模块全用 `glam::Vec3`；改类型会让 fixtures 期望值在 1e-3 mm 量级抖动。缓解：先 f32 原样迁入跑通，f64 化单独一笔提交并按容差重录期望。
- **R5 sesno / 水位**：`build_set(dbnum, None)` 取文件最新会话（ADR-054）；与前端显示的模型版本 `(dbnum, unit_refno, sesno)` 可能不一致。首发接受「最新」，Phase 5 前评估加 `?sesno=` 查询参数。
- **R6 本仓工作区约 289 处未提交改动**：本计划只新增 `src/mbd/`、一条路由、一行依赖与一份 ADR，不碰在途文件；提交时逐 hunk 暂存。
- **R7 与 rs-mbd 分叉**：复制后两库各自演进。约定：plant-mbd 为算法单源；rs-mbd 冻结不再改，其 README 标注指向 plant-mbd。

## 7. 相关文档

- PML：`old/vendor/MBD/开发文档/MBD模块架构与数据接口.md`、`管道标注绘制流程.md`；`markpipe/object/isobran.pmlobj`（`getbranmems` L236、`getisolines` L1470、`draw` L2999、`getbranmlabel` L5323）、`isoline.pmlobj`（`getIsoUsedDirs` L439）、`object/mbd/lindim.pmlobj:312-320`
- rs-mbd：`CONTEXT.md`、`docs/ROADMAP.md`、`docs/field-dictionary.md`、`docs/adr/0001-0005`、`.planning/2026-08-03-mbd-dimension-review-fixes/{task_plan,findings}.md`、`docs/plans/2026-07-next-migration-datasource-first.md`
- plant3d-web：ADR 0028 / 0041 / 0042 / 0043 / 0044 / 0046 / 0048；`docs/plans/2026-08-01-mbd-dimension-next-step-datasource-first-plan.md`、`2026-07-22-rs-mbd-linear-json-web-loading-plan.md`；`src/fixtures/mbd-v2/README.md`
- gen-model：ADR-053 / 054 / 057 / 059；`vendor/e3d-model/src/route.rs`、`catalogue_point.rs`、`transform.rs`；`src/web_service/mod.rs`、`src/fast_model/e3d_model_service.rs`
