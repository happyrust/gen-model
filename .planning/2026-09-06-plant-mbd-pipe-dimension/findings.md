# findings — 2026-09-06-plant-mbd-pipe-dimension

事实与裁决的落点；`task_plan.md` 只留状态与勾选。

## Phase 0（2026-09-06）

- **rs-mbd 纯模块零改动迁入即绿**：`contract / data_source / split_isolines / direction / connectivity / layout`
  + 13 份 fixtures，glam 0.30 → 0.29 无 API 差异，53 用例通过（`old/vendor/plant-mbd` `8055f47`）。
- **f64 化不改任何既有期望值**（`d5f09ec`）：toy fixtures、field `24383_99996` 的尺寸文本、前端 fixture
  逐字节对齐全部保持。新增 `tiny_bend_clears_the_direction_gate_in_f64`：0.02° 缓弯（apos≠lpos）在 f32 下
  被 `dot == 1.0` 吞成共线；f64 下过 `DIR_CHANGE_EPS_DEG = 0.01°` 闸门，② 主切 + ③ 强制切（朝外口径下
  `angle(adir, memldir) = 180° − 0.02° > 100°`）切成三袋、弯头自成单元素袋——与 `toy-bridge-and-stick`
  的 12° 弯 ②+③ 叠加同理，是 PML 语义而非 bug。
- **前端 fixture 与求解器的唯一差异是 `meta.notes` 的 `allarr=[…]` 调试 dump**（rs-mbd 2026-08-03 Phase 0
  移除）。图元逐字节相等。已写进 `contract_guard.rs`。
- clippy 4 条继承告警（3 collapsible-if、1 too_many_arguments in `bridge_close_open`）未动。

## Phase 1（2026-09-06）

### 数据源口径

- **端口方向朝外**：e3d-model `route::slot_alignment` 的 `ArriveCheck::Member` 文档写明「到达端的方向按 P 点
  约定朝外（逆着行进），拿 `−to.dir` 与槽位向量比」——与 plant-mbd `BranchMember.arrive_axis` 的口径同一，
  适配层直接透传 `RoutePoint.dir`，不取反。
- **成员 k 的两端**：`walk_route` 给 `tubes.len() == members.len() + 1`，成员 k 的到达端是 `tubes[k].to`
  （`RouteEnd::MemberArrive`）、离开端是 `tubes[k+1].from`（`RouteEnd::MemberLeave`）。
- **穿过件**（目录 `GTYP≠ANCI` 的 ATTA、WELD）在 `Route::non_route_members`，不在 `members`；适配层按
  `container.members()` 文件原序把两者混排回去，`order` = 原序下标，穿过件只给 `pos + noun`。
- **Head**：`container_head` 会把缺 HDIR 补成 `+Z`，适配层改成分开读 `HPOS` / `HDIR`，缺了走兜底
  （首个带到达方向成员的 apos，方向取其 adir 反向）并记 `adapter.head_attrs_missing`。
- **Tail**：`BranchQueryResult` 没有 tail 字段，`branch_to_split_members` 用最后一个成员的 lpos 合成 Tail
  （rs-mbd 原样）。PML `getbranmems` 的 Tail 是否取 `tpos of bran`（L1493-1494 只见 hpos/hdir）——
  **留 Phase 3 金样时对照 PML 裁决**；有尾管（最后成员 lpos ≠ TPOS）时两者不同。
- **aod**：目录 `PARA[2]` 外径的读法 `tube_outer_diameter_from_stub` 在 e3d-model 里是 `pub(crate)`，
  适配层暂用 `tube_outer_diameter(None, None, arrive.bore)`（到达侧通径回落）。Phase 4 偏移量真正用到 aod
  时再在 e3d-model 开一个 pub 口，不为 Phase 1 改 vendor 仓。

### 环境

- **主工作树编不过，不是本计划的改动**：`cargo check --lib` 报 `src/data_interface/direct_tree.rs:249`
  重复定义 `dbnum_of`（有人正把 `Result<i32>` 版换成 `skeleton` 的 `Option<u32>` 版）与
  `src/web_service/handlers.rs:1014` `json!(dbnum)` 对 `anyhow::Error` 不可序列化——同一处 WIP 的两半。
  `src/mbd/` 自身零错误。
- **HEAD 也编不过**：`src/data_interface/model_update_plan.rs:921` 对 `resolve_unit_rollup` 传 4 参、
  `.xgeometry_gated` 字段不存在——与 vendor/e3d-model 当前工作树不配（gen-model 的 289 处未提交改动
  在追最新 vendor）。
- **共用 target 目录 `D:\Rust\target` 的 `aios-database.exe` 被 PID 60444（09-06 02:05 起的服务进程）占着**，
  集成测试会连带构建 bin，`cargo test --test …` 因此失败。
- 处置：开一次性 worktree `D:\work\plant-code\old\gen-model-mbd-wt`（HEAD + 镜像主树 `src/` + 本计划文件），
  **只在 worktree 里**删掉旧 `dbnum_of` 三行让它编过，`CARGO_TARGET_DIR=D:\Rust\target-mbd-wt` 全量重编后跑
  `tests/mbd_branch_query.rs`。主树一行别人的代码都没动。worktree 用完删（`git worktree remove`）。

### 验收记录（2026-09-06，`tests/mbd_branch_query.rs` 3/3 绿，真库 ams7999 + 目录扫描）

- **成员构成**：BRAN `24383/99996` 直读 31 个成员 = ATTA 14 + BEND 4 + ELBO 3 + FTUB 10，文件原序连续编号
  （rs-mbd 8-20 勘察写的「5 BEND + 3 ELBO + 9 FTUB + 13 ATTA」加起来是 30，数错了一处；31 与其正文一致）。
- **适配层 issue = 0**：全部 17 个站点成员两端 P 点都解出来了，**包括 ELBO `24383_100013`**——Surreal 路径上它
  「catalogue ptset 缺 1/2 号端口」要靠 `connectivity::infer_missing_fitting_axes` 补轴，直读 `CataloguePoints`
  走 `SPRE → SCOM.PTRE → PTAX` 直接给出 60° 转角。所以 `layout_pipe` 的 issues 从 fixture 路径的 3 条
  （2 条补轴留痕 + 1 条偏轴）降到 **1 条**（只剩偏轴）。
- **朝外不变量**：相邻站点成员 `next.arrive · prev.leave` 全部 < −0.9999，唯一违例 `100018 → 100019`
  （dot = −0.087，85° 拐角）——正是 rs-mbd findings 里回源库查实的源模型断点。口径钉死。
- **弯头转角**：BEND 100005 90° / BEND 100007 8° / ELBO 100013 60° / ELBO 100015 60° / BEND 100017 10° /
  ELBO 100020 90° / BEND 100022 90°（朝外口径下 `180° − angle(adir, ldir)`）。
- **交叉核对（Surreal 路径 vs 直读）**：fixture 里 24 个成员逐个在直读结果里找到；kind 全部一致；
  **位置最差 0.0009 mm、方向最差 0.00000°**；Head 的 HPOS/HDIR 也对上。两条数据路径给的是同一份几何。
- **端到端**：`layout_pipe` → 12 直段，尺寸文本 2500 / 1000 / 3500(chain) / 500 / 1600 / 500 / 350 / 850 / 400 / 300
  ——与 plant-mbd 在 fixture 上的结果**逐条相同**；`meta.source_to_design` 到位、`layout_mode = linear_mvp`。
- 结论：**§3 第 2 条完成判据达成**；Phase 2 可以直接挂端点。

## Phase 2（2026-09-06）

### 实现

- `src/web_service/mbd.rs`：`mbd_v2_pipe(State, Path<String>)` → `parse_refno` → 进程内 `OnceCell<Arc<E3dModelService>>`
  （`from_current()` 只建一次，失败下次重试）→ `build_set(dbnum, None)`（目标库文件最新会话，ADR-054）→
  `spawn_blocking(layout_pipe)`。主体 `respond(set, refno, input_refno)` 与 `AppState` 无关，测试直接喂库。
- 状态映射（D6）：`Ok` → 200（零尺寸也 200，抑制原因在 issues）；`NotFound` → 404 `not_found`；
  `NotRouteContainer` / `Solver` → 422 `precondition`；`RootError` → 422 `precondition` + `detail.issues`
  （同一信封，`detail` 不再是 null，前端能说清「为什么整条没出图」）；`Db` → 500 `internal`；refno 写法不对 → 400。
- 路由挂在 `serve()` 的 Router 上（`/api/v1/ws` 之后），路径 `/api/mbd/v2/pipe/{refno}` 是前端 `mbdV2Api.ts`
  写死的契约，不在 `/api/v1` 下。

### 验收（真库，worktree + `D:\Rust\target-mbd-wt`）

- `tests/mbd_http_endpoint.rs`：起真 axum 服务在回环随机端口，reqwest 打——
  `24383_99996` → **200**，`version=v2`、`branch_refno=24383_99996`、`source_mm` + 16 元素矩阵、10 条 `linear_dim`、
  `layout_mode=linear_mvp`；`24383%2F99996`（斜杠写法）→ 200 且 `input_refno` 原样回填 `24383/99996`；
  `24383_999999999` → **404** `not_found`；BRAN 的属主 PIPE → **422** `precondition`；`not-a-refno` → **400** `bad_request`。
  等价于计划 §5 的三条 `curl`。
- **前端契约真身校验**：把 200 的 wire 体落盘，用 `node --experimental-strip-types` 直接跑 plant3d-web 的
  `src/dimension/adapters/mbdV2Contract.ts::parseMbdV2PipeData`（fail-closed 门）→ **ACCEPTED**：
  10 primitives（全 `linear_dim`）、1 issue、0 diagnostics。这就是浏览器里 `useMbdExternalSync` 会走的那道门。

### 浏览器联调——需要用户在自己的终端做（规则：不起长驻进程）

主工作树暂因他人 WIP 编不过，且现网 `aios-database.exe`（PID 60444）是 02:05 的旧构建、占着 8022 与实例锁。
可用 worktree 构建替换：

```powershell
# 1) 用 worktree 出一个带端点的 debug 二进制（deps 已全量编好，只编本仓）
cd D:\work\plant-code\old\gen-model-mbd-wt
$env:CARGO_TARGET_DIR = "D:\Rust\target-mbd-wt"; cargo build
# 2) 停掉旧服务（PID 60444），在 gen-model 目录用同一份 DbOption.toml 起新构建
cd D:\work\plant-code\old\gen-model
D:\Rust\target-mbd-wt\debug\aios-database.exe          # http_api_addr = 0.0.0.0:8022
# 3) 三条 curl
curl    http://127.0.0.1:8022/api/mbd/v2/pipe/24383_99996
curl -i http://127.0.0.1:8022/api/mbd/v2/pipe/24383_999999999   # 404
curl -i http://127.0.0.1:8022/api/mbd/v2/pipe/not-a-refno       # 400
# 4) 前端
cd D:\work\plant-code\old\plant3d-web; npm run dev
# http://127.0.0.1:3101/?output_project=AvevaMarineSample&show_dbnum=7999&show_refno=24383_99996&mbd_refno=24383_99996&data_source=backend&backendPort=3100&mbdBackendPort=8022
```

预期：尺寸随相机跟随、尺寸面板 `layout_mode=linear_mvp` 触发「本分支标注不完整」警示条、1 条 Suppress issue
（`24383_100019` 偏轴）。

## Phase 3（2026-09-06）

### 裁决 1：隐式管身是宿主的事，按 E3D 槽位规则合成

- **事实**：E3D 库里两个管件之间的直管不是成员（只有 FTUB 这种定长管是），`coll all bran mem` 收不到；rs-mbd
  时期 Surreal 把管跨物化成了 `PIPE` 成员，所以线性 MVP 只给 `MemberKind::Tubi` 出尺寸也够用。直读 `.dat` 后，
  13 条候选 BRAN 里 5 条（`24381_145018`、`24381_177565`、`24383_74307`、`24383_76191`、`24384_26229`）**一条尺寸
  都没有**——它们没有 FTUB，直管全是隐式的。而 PML 的直管长度（`getcuttubi`）与线性尺寸量的正是构件之间这段管。
- **先试了求解器侧按空隙补管**（`branch_to_split_members` 里相邻站空隙 ≥ 1 mm 就合成）——`verify_mapping` 立刻在
  `toy-bridge-and-stick` 上翻红：fixture 刻意留的 EB→T3 9.95 mm 空隙长出一根管。求解器不该猜数据里没有的成员。
- **落点**：gen-model 适配层 `push_implied_tube`，按 E3D 画隐式管身的同一套门（e3d-model `walk_route` 槽位：
  上一站离开点 → 本站到达点，含 Head→首站、末站→Tail；长度 ≥ `ZERO_LENGTH_TOL_MM` 1 mm；两端共轴
  `slot_alignment`，尾管缺 TDIR 只判离开侧）合成 `TUBI` 成员（refno `A~B`，`leave_axis` 顺流、`arrive_axis` 朝外、
  `pos` 中点、aod 回落到达侧通径），插在前一站之后。E3D 不画的槽位（反向 / 偏轴）不合成，≥ 10 mm 的记
  `adapter.implied_tube_not_drawn`（< 10 mm 的装配余量 / P 点错位是噪声，不出声）。
- **效果**：13 条全部出尺寸；`24383_99996` 的 10 条尺寸文本一个没变（它的隙全被共轴门拒掉：Head 80 mm 90° 偏、
  `100018→100019` 50 mm 85°、尾管 80 mm 反向）；`24384_26229` 的 TUBE 2 = **312.11 mm** 与 e3d-model `route.rs`
  引用的 E3D 证据（312.1 mm 穿过三只 ATTA 到 VALV）一致——隐式管身的口径就是 E3D 的口径。

### 裁决 2：Tail 取 `tpos of bran`

PML `getisolines` L1499-1505 对 Tail 只收袋、`tpos` 那行注释掉了（切段不读）；但 L1812 / L1901 / L4255 凡取
Tail 位置一律 `tpos of $!this.branname wrt/*`。`BranchQueryResult` 新增 `tail_pos / tail_dir`，
`branch_to_split_members` 的 Tail 优先 tpos、缺则退回最后成员 lpos；宿主从 `TPOS/TDIR` 直读。有尾管时链尺寸终点
因此落在 TPOS。plant-mbd `ec7e880`。

### 裁决 3：ATTA 的目录链走不到 SCOM 时出声

`24384_2944`（`/C-GR-1RX-3020B`，route.rs 证据里「六根 TUBE 全以 ANCI 为端点」）在本次 DbSet（ams000 目录扫描）
里 14 个 ATTA 全被判成穿过件——MDS 梯架支撑的目录库不在扫描范围（加扫 `AvevaCatalogue` 目录也没解出），
`walk_route` 的 `PassThroughUnresolved` 静默降级。适配层新增 `adapter.atta_catalogue_unresolved`（带计数），
`24383_99996` 的 14 个 ATTA 也是这一档（它们本就是穿过件，结果不变）。生产路径经 `E3dModelService` 的 MDB 定位器
开对目录库后再收 `24384_2944 / 3277` 为金样。

### 金样批次（plant-mbd `ec7e880`，`fixtures/golden/`，`cargo test` 逐字段重放）

| 金样 | 库 / sesno | 覆盖 | 直段 / 尺寸 |
|---|---|---|---|
| `24383_99996` | ams7999 / 191 | 混排；源模型断点 → `linear.member_off_axis` | 12 / 10 |
| `24383_76191` | ams7999 / 191 | 17 ELBO、零 FTUB，14 根隐式 TUBI | 21 / 14 |
| `24383_91813` | ams7999 / 191 | 5 共线 FTUB，单直段，chain 13500 | 1 / 6 |
| `24383_100028` | ams7999 / 191 | BEND 8 + ELBO 4，含 14–26 mm 短管走 small_dim | 23 / 23 |
| `24381_145018` | ams7997 / 114 | rs-mbd 时期的「空壳分支」，直读 28 成员 | 11 / 12 |
| `24384_26229` | ams8000 / 323 | VALV/WELD/ATTA，TUBE 2 = 312.11 mm 对上 E3D 证据 | 6 / 5 |

来源 fixture 由 gen-model `examples/mbd_branch_dump.rs` 直读导出（`fixtures/field/*.json`，带 `$schema_comment` 与原始
`branch_query`）。候选 `24381_177565`（BEND 12 + COUP 4 + TEE + VALV，17 尺寸含 19.68 / 29.88 / 30 三条小尺寸）、
`24383_74307`、`24383_100161`、`24383_75574`、`24383_98772` 都能出图，暂不入批次。

### 发现 4（进 Phase 4 前必须拍板）：rs-mbd 线性 MVP 的**尺寸点模型**不是 PML 的

读 PML 非 Polar 路径（`isoline.getIsoDims` L388 → `isoDim(branname, mems, pipedir…)` L129 →
`addmem` L464-728 → `lindim(points…)`），主尺寸的口径是：

- **每条直段一条 running dimension**（`lindim` 多点：`points[1..n]`，`do !x from 1 to size-1` 逐段出值，
  `sepSmallDim` 处理小段），不是「每根管一条 + 一条总长」。
- **尺寸点 = 构件原点投影到直段轴上**（`pos of $!mem wrt/*`，L533），Head→`hpos`、Tail→`tpos`；
  `exceptTypes = ATTA TUBI GASK`（L133）**不进点**；WELD 只在不贴着相邻构件端口时进点（L199-221）；
  PCOM / INST / 名字 `*DI` 有专用取点（L477-535）；点去重 `possunique`；`considerTee`。
  所以一段 ELBO–管–VALV–管–ELBO 的 PML 尺寸是 **[弯头拐点→阀中心, 阀中心→弯头拐点]**，
  rs-mbd MVP 出的是 **[管 1 长, 管 2 长, 总长]**——数值不同、点不同。
- **行位**（`isoDim.drawDim` L1298-1316）：沿 dimdir 取最远点，偏 `od/2 + od/2 + 1.2·cheight·(dimtimes−1)`；
  主尺寸 `dimtimes = 2`（有 ATTA 子尺寸 `twoormoredims` 时）否则 1；ATTA 子尺寸（`getAttaDims`）行 1。
  rs-mbd 的 `MEMBER_OFFSET 500 / STACK_STEP 800`（每条成员尺寸再叠一行）是自造的。
- **方向**（`isoDim.CalculateDimChardirs` L1131-1170）：`dir` = 直段中点 → BRAN 包围盒中心（`volume $bran`）；
  E/N/W/S 按与 `dir` 夹角排序，`gooddimdirs = [最近两个的反向, U]`（从内往外看），`chardirs = [U, 最近, 次近]`；
  再经 `isoori(pipedir, gooddimdirs, chardirs, usedDirs, 60)` → `isoGetHandleDimDir`（已迁为 `direction.rs`）。
  `usedDirs` 来自 `getIsoUsedDirs`（L439-528）：有「手柄」的构件（P3 不在 P1–P2 轴线上，L452-473）
  贡献 P3 方向（TEE/OLET 按 P1 与 pipedir 是否垂直取 P3 或 P1+P2），占位角 60。
- **cheight**：表单默认 `cheightbili 0.3`（字高 = 0.3 × od）、`autocheight true`、`sepSmallDim true`、
  `firstDimTimes 0.5 / secondDimTimes 1.7`（Polar 路径用）。

**含义**：Phase 3 金样固化的是 rs-mbd MVP 的口径（tube lengths + chain），PML 图纸对照一定对不上。
ADR-0002「语义精确」要的是尺寸点与数值一致——**Phase 4 应改成「线性尺寸对齐 PML isoDim 模型」**，
方向系统与 cheight 行位是它的一部分，而不是先在错的尺寸点上接方向。宿主侧要补三样数据：
BRAN 包围盒中心（`volume_center`）、每个成员的 P1/P2/P3（位置 + 方向，`CataloguePoints::leave(member, Some(n))`
就能解）、od 已有。求解器侧：新建 `isodim.rs`（取点 / 投影 / 去重 / WELD·PCOM·INST 规则）替掉
`layout.rs` 的 member/chain 装配，`direction.rs` 接线，行位按 od + 1.2·cheight·(dimtimes−1)。
金样重录（基线变更显式审阅）。是否按此改，等用户拍板。

### 待人工对照 PML 图纸（ADR-0005 的那一遍审阅还没做）

- ~~14–26 mm 短管（`24383_100028`）PML 是否出尺寸~~：Phase 4 读 `isoDim` 后已知——它们是弯头间的短管，
  PML 不量管长，量拐点到拐点，短管长度本来就不是尺寸点（见 Phase 4）。
- 隐式管身两端「E3D 不画」的槽位 PML 怎么处理（`24383_76191` Head 侧 638.9 mm 偏 40° 的那段）：Phase 4 已知
  ——`isoDim.addmem` 的偏轴检查把远端弯头跳掉、打「请检查该元件在 Bran 中的顺序或坐标」，那一袋无尺寸。
- 六条金样的切段数与拐点对照 `isobran.getisolines`——这一遍需要 E3D 里开同一批 BRAN。

## Phase 4（2026-09-07）：线性尺寸对齐 PML `isoDim` 模型（用户拍板 A）

plant-mbd `287b1f0`（ADR-0006），e3d-model `3c5ee02`，gen-model 本提交。

### 落地口径（对照 PML 行号，全部写在 plant-mbd `isodim.rs` 模块文档）

- **尺寸点**（`isoDim` 构造 L164-240、`addmem` L464-681 / L728-942、`addpos` L944-968、`considerTee` L258-302、
  `possunique` L431-458）：Head→`hpos`、Tail→`tpos`、构件→`pos of $!mem`（原点）投影到直段轴；
  `exceptTypes = ATTA TUBI GASK` 不进点；`FLAN / FBLI / REDU` 两端口进点；贴着相邻构件进出口 1 mm 内的 WELD
  不进点、管中间的进；首末 TEE 不走 P3 时补另一直通口；0.01 mm 去重，lindim 再 1 mm 去重。
- **行位**（`isoDim.drawDim` L1299-1311 = `isoline.drawdim` L1869 的 radius）：`od + 1.2·cheight·(dimtimes−1)`，
  `dimtimes` 暂 1（PML 有 ATTA 子尺寸时主尺寸走第 2 行，`drawOneObjectNew` L4179-4186；Phase 5 接）。
  `drawDim` 的 `goodpos` 循环里 `mindis` 从不更新、落下的是最后一个点的投影——照抄。
- **字高**（`isoline.draw` L1689-1691）：`int(od × 0.3 × (1 − 0.5·(od−50)/(|od−50|+100)))`，不是简单的 0.3·od
  （Phase 3 findings 里那句「cheight = 0.3 × od」不准）；再按 `changeCheightAuto`（isoline 级 nint、lindim 级不取整，
  文字总宽 ≤ 70% 尺寸长，最低缩到 50%）。`meta.cheight_mm` 取各直段最大。
- **小段**（`lindim.sepSmallDim` L282-446，表单默认 `sepSmallDim true` / `autocheight true`）：文字（`mbdtextlen`
  字宽表：数字 ≈ 0.576、`.` 0.2969）装不下时先缩字（`getgoodcheight` 0.8、最低 50%），缩了还装不下就往外叠一行
  `1.2·cheight·level`；这类段 `sub_kind = "small"`（前端箭头外置）。契约没有逐图元字高，缩字只体现在位置上。
- **方向**：`CalculateDimChardirs`（L1131-1170，从直段中点看 `volume $!bran` 中心，E/N/W/S 稳定升序，
  `gooddimdirs = [最近两个的反向, U]`）→ `getIsoUsedDirs`（L439-528：P3 不在 P1 轴线上且 P2 与 P1 重合或在 P1
  轴线上 = 手柄，TEE/OLET 按 P1 是否平行管轴取 P3 或 P1+P2，占位角 60）+ `addUsedDirToNext`（isobran L3288-3336：
  上一段 `dimdir.bisect(−pipedir)`、占位角 90、落在本段起点 ±od/2；尺寸方向与本段管轴几乎平行时不加）
  → `getusedDirs` 区间过滤（`drawOneObject` L4247-4256：主尺寸范围两端各收 `cutlen = min(od/2+1, 长度/10)`，
  所以邻段占位只在长度 < 5·od+10 的短直段上起作用）→ `isoori.getori` L74 `isoGetHandleDimDir(pipedir, used, 60,
  dimdirs)`（`direction.rs` 已有）。`deterministic_normal` 删除。
- **PML 自己的偏轴检查照搬**（`addmem` L635-677）：水平直段（`flowDirtoUangle eq 90`）上构件原点与两端口都偏离
  轴线超过 `od·2/3` → 跳过、`isodim.member_off_axis`（PML `$p 请检查该元件在 Bran 中的顺序或坐标`）。
- **本库保留的口径**（PML 没有）：跨过偏轴直管（> 10° 主切角）的那一段抑制、`linear.member_off_axis`——
  竖直直段上 PML 不做偏轴检查、会无声画出压短的假数（ADR-0002）。
- **未迁**：PCOM / INST / 按名字（`*DI` / `*FL` / `*JH` / `RJ`）的专用取点；ATTA 子尺寸；Polar 路径。
  注意 `isoline` 构造 L134 写死 `usepolarsystem = true`——PML **生产路径是 Polar**（`drawOneObjectNew` →
  `isoline.drawdim` → `polarsystem.getBestPosAndOri`），行位公式与非 Polar 相同，选向不同。本期按用户拍板走
  `CalculateDimChardirs` + `isoGetHandleDimDir`（非 Polar 路径）；Polar 的 `getBestPosAndOri` 留 Phase 5 ⑤。

### 宿主补的数据（`src/mbd/branch_query.rs`）

- `ppoints` P1/P2/P3：`CataloguePoints::leave(&member, Some(n))` 逐号解（站点与穿过件都解——MDS 支撑 ATTA 的
  P3 就是支架方向，PML 也把它算手柄）；解不出不填。真库 `24383/99996` 全部 17 个站点成员 P1/P2 都解出。
- `arrive_ppoint / leave_ppoint`：`RouteEnd::MemberArrive/Leave{ppoint}`。
- **真外径**：e3d-model 新开 `route::departure_tube_outer_diameter(&RouteEnd)`（`CGETOD` A 段：HSTU/HSRO、LSTU/LSRO
  → SCOM `PARA[2]`；纯转发 `pub(crate) tube_outer_diameter_from_stub`，只加不改，e3d-model `3c5ee02` 只带这一个 hunk）。
  取不到按 PML `isoline.getod` 回落 `min(bore×1.1, bore+10)`。`24383/99996` 的 FTUB 外径 25（DN20），Phase 1
  时用通径回落给的是 20。
- `volume_center` 不在宿主算（要建整条几何），求解器取成员坐标包围盒中心近似。

### 金样重录（plant-mbd `fixtures/golden/README.md` 逐条）

| 金样 | MVP → isoDim | 要点 |
|---|---|---|
| `24383_91813` | 6 → **1**（13500） | 第一条对照：5 根共线 FTUB 不进点，Head→Tail 一条 ✓ |
| `24383_99996` | 10 → 6 | 值含弯头切线（847.76 = 350 切线 + 500 FTUB − 1.4 间隙）；Head 低 80 mm → BEND `100005` 被 PML 偏轴检查跳过（第一袋无尺寸）；`100020` 同理（断点袋无尺寸）——`87.16` 假数不再需要本库守卫拦 |
| `24383_76191` | 14 → 17 | 拐点到拐点；ELBO `76192` 因 Head 偏 638.9 mm / 40° 跳过 |
| `24383_100028` | 23 → 12 | 14–26 mm 短管都在弯头切线里，不是点；BEND `100031` 因 80 mm Head 偏移跳过 |
| `24381_145018` | 12 → 12 | OLET / VALV 原点是点；VALV P3 手轮 60° 占位 |
| `24384_26229` | 5 → 6 | 管中间 WELD `26236` 进点；312.11 mm 管长不再单独成尺寸 |

AvevaMarineSample 里 Head 比首根管低 80 mm 的支管不止一条（`99996` / `100028`）——E3D 不画那段头管，PML 也不给
第一袋尺寸；这是数据的事，不是求解器的事。ams7997 / ams8000 的库文件今天已长出新会话（155 / 379），导出时
`--sesno 114 / 323` 钉回原谱系。

### 验证

- plant-mbd `cargo test --workspace` **71 通过**（66 lib + 4 contract_guard + 1 golden）；`verify_mapping` 6/6；
  `golden diff` 6/6；clippy 新代码零告警（4 条 rs-mbd 继承告警未动）。
- gen-model worktree：`mbd_branch_query` **4/4**（新增 `real_branch_members_carry_ppoints_and_catalogue_od`：站点
  成员 P1/P2 与到达 / 离开口编号齐、外径来自目录；`layout_pipe` 6 条尺寸、`layout_mode = isodim_main`、
  `cheight_mm = 8`、无 `isodim.outside_diameter_unknown`）、`mbd_http_endpoint` 1/1。
- 三条金样 wire 体经 plant3d-web `parseMbdV2PipeData`：**ACCEPTED**（`sub_kind main` / `layout_mode isodim_main`
  / `cheight_mm` 都是自由字段，0 diagnostics）。前端 `PARTIAL_LAYOUT_MODES` 只含 `linear_mvp`，警示条不再触发。
- **未验证**：PML 图纸人工对照仍待 E3D 开同一批 BRAN（ADR-0005）；浏览器目测仍待用户起服务（Phase 2）。

## Phase 5 ①（2026-09-07）：ATTA 子尺寸（`getAttaDims`），主尺寸按 `twoormoredims` 升第 2 行

plant-mbd `2d7ca54`。数据来源不变：全部来自 e3d-io / e3d-model 直读（不查数据库）。

### 口径（`isoline.getAttaDims` L1054-1222 → `isodim.rs::collect_atta_dims`）

- 逐个 ATTA（袋序 = 文件序）：`attadis` = 原点沿管轴到直段起点的有向距离。
  1. 落在某条已有 attaDim 首末点之间 → 加进它；在某条之后 → 记最近的那条（`goodindex`）。
  2. 否则找主尺寸里第一个比它远的点：排在第一个主点之前 → PML 报「请检查 ATTA 位置 / 顺序」、放弃整条直段
     （Polar 路径 `getIsoDims` L422-426 先 `getAttaDims` 再 append 主尺寸，所以主尺寸也没了）；离上一主点比离最近
     attaDim 末点近 → 从那个主点新开一条（`isoDim.copy(index−1, atta)`，点 = [主点, ATTA]）。
  3. 都不是：比最近 attaDim 末点更近的是主尺寸最后一点 → ATTA 加进**主尺寸**；否则加进最近的 attaDim。
- ATTA 进点走 `isoDim.addmem(mem)` 普通分支（原点）。L637-643：原点偏轴超 `od·2/3` 且 `apos of $!mem` 解不出
  （目录链走不到 P 点）→ `return true`——**静默不进点**；有 apos 的水平直段上两端口都偏 → 打「请检查」跳过
  （`isodim.member_off_axis`）。站点 ATTA 用真端口，穿过件用 P1 / P2 当 apos / lpos。
- `twoormoredims` = attaDims 非空（L1213，**不看它有没有两个点**）→ 主尺寸 `dimtimes = 2`（`drawOneObjectNew`
  L4179-4186）；子尺寸 `drawSub(this.dimdir, …, 1)`：第 1 行、与主尺寸同一个 dimdir（`isoori(gooddimdirs,
  goodchardirs)` + 给定 dimdir，L1283-1287 不再选向）；至少两点且首末 > 1 mm 才画（L1216-1220）。
- isoline 级 `changeCheightAuto`（L1822-1834）看主 + 子尺寸全部 ISODIM 对象的 `getgoodcheight` 取最小。
- **两处容差**（PML 是逐位比较，库里的浮点噪声会误报）：「ATTA 在第一主点之前」比较加 0.01 mm（`possunique`
  同量级；`99996` 第 11 袋 BEND `100022` 的 ldir 带 1.96e-8 的 y 分量，乘 500 mm 横向偏移 = −1e-5 → 差点放弃整条）；
  `flowDirtoUangle eq 90` 放 0.01°（`26229` 第 4 袋 89.9998°；1:1000 坡管 0.057° 不受影响）。
- 未做：按 SPCO 名字分派的 `*STE*` / `*TC*` / `*TD*` / `*TE*` / `*TF*` / `*Tuser*` 两侧取点（L896-920，
  `p101dir` / `para[9]` / `desp[3]`）——没有 SPCO 名字，按原点一个点。

### 金样再审（三条变）

| 金样 | 变化 | 备注 |
|---|---|---|
| `24383_99996` | 6 → 6 + 2 | 第 3 袋子尺寸 [拐点 100007, A9, A11] = 96.4 + 1400，主升第 2 行；第 11 袋 A23 / A25 挂在离轴 700 mm 的结构上、目录链走不到 → 静默不进点，但 attaDim 已建 → 主 271.5 在第 2 行、第 1 行空着（PML 怪癖照搬） |
| `24381_145018` | 12 → 12 + 6 | 六个支撑各一条「上一拐点→支撑」子尺寸（480 / 1257.56 / 1035.51 / 907.52 / 1650 / 402），六个袋主升第 2 行 |
| `24384_26229` | 6 → 5 + 1 | 第 2 袋子尺寸 300、主升第 2 行；ATTA `26237` 在阀门上方 529 mm → 跳过留痕。**第 4 袋 4403.29 消失**：袋轴从 ELBO `26239` 朝东北出发，管子沿 +X 到达 BEND `26246`（中间在两个穿过件 ATTA 处折了两次，E3D 也不画跨过它的管身 `implied_tube_not_drawn`）；水平判定放容差后 PML 的偏轴检查在这个 89.9998° 的袋上生效、跳过 BEND → 那个数本来是折线的压缩投影 |

`76191` / `100028` / `91813` 没有 ATTA，不变。

### 验证

- plant-mbd `cargo test --workspace` **75 通过**（70 lib + 4 + 1，新增 4 条 ATTA 用例：接力子尺寸 + 主升第 2 行、
  靠后主点新开一条、无 ATTA 仍第 1 行、ATTA 在首点前放弃直段）；`golden diff` 6/6；clippy 新代码零告警。
- gen-model worktree `mbd_branch_query` 4/4（真库 `99996`：6 main + 2 atta）、`mbd_http_endpoint` 1/1。
- 三条金样 wire 体经 plant3d-web `parseMbdV2PipeData` ACCEPTED（`sub_kind = atta` 自由字段，箭头内置）。

## Phase 5 ②（2026-09-07）：坡度符号（`getSlopes` / `isoSlope`）+ 斜管分解（`isoDim.dimslope`）

plant-mbd `da4868e`（ADR-0007，新模块 `slope.rs`）。契约零改动：全部走冻结契约里已有几何的
`slope_mark` / `aid_line` / `aid_text`。

### 口径

- **坡度符号**（`isoSlope.pmlobj` L14-75，`isobran.getSlopes` L2522-2534 逐 BRAN 成员，`putIntoIsoline` 归段）：
  `type eq 'TUBI'`、`|Gradient| ∈ [minslope 0.001, maxslope 0.1]`（`isobran.pmlobj` L191-192 默认）、`Itlength ≥ aod`
  的直管，在 `Itposition`（管中点）上方画直角三角形：`dir1` = 坡度为正（沿流向上升）时流向反向、否则流向；
  `tempdir = dir1 × U`，`ydir = tempdir × dir1`；`p1 = p0 + dir1·od/2 + ydir·(od/2+20)`，`p2 = p0 − dir1·od/2 + ydir·(od/2+20)`，
  `p3 = p2 + ydir·od/4`；三条线 p3→p1 / p3→p2 / p2→p1，文字 `'slope ' + string(slop×100, 'd1') + '%'` 在 `p4 = p1 + ydir·(od/4+10)`。
  → 斜边 `slope_mark`（前端画 start→end + `end` 处箭头 + 中点文字，箭头正好落在下坡一侧）+ 两条 `aid_line`。
  `Gradient` 属性这边按两端点几何算 `Δz / 水平投影长`；TUBI / FTUB 同一线性角色（Phase 0 口径）。
- **斜管分解**（`isoDim.dimOneMemSlope` L1251-1270：主尺寸首末点；直段轴与 E/W/S/N/U/D 最近者夹角 `tan > maxslope`
  → 5.71° 以上算斜管 → `dimslope` L1342-1506）：|Δz| > 0.5 时高点投到低点高程 → 竖直腿 + 文字 → 递归水平部分：
  两个轴对齐角点 `point1 = (pos2.x, pos1.y)` / `point2 = (pos1.x, pos2.y)`，腿向落在 `dimdirs` 里更多的那个胜出
  （非 Polar 版；Polar 版 `isoline.dimslopedim` L2357-2465 用「离包围盒中心远」）；≥ 1 mm 的腿各一条 `aid_line` +
  整数腿长 `aid_text`（`len.string('d0')`，加 Polar 版的 `X:` / `Y:` / `Z:` 前缀）；角点直角标
  `min(min(腿长)/4, cheight)`（`drawRightAngle`）。pos1 就是角点时退回第二条腿给竖直直角标定向（L1484-1487）。
- 未建：`dimslope` 往 `usedDirs` 加的 `ISODIM-1/2` 占位（L1366 / L1473）——它们落在主尺寸范围两端 `cutlen` 内，
  `getusedDirs` 一定过滤掉，对选向无影响。腿长文字的 Polar 摆位（`getBestPosAndOri`）用「腿中点沿
  `get_dim_dir(腿向, dimdirs)` 外移 0.6·cheight」代替。

### 金样再审（五条变，`91813` 竖管不变）

| 金样 | 变化 |
|---|---|
| `24381_145018` | +7 `slope_mark`（排水线 0.3%–0.5% 的七根隐式管）+ 14 `aid_line`；第 4 袋平面 45° → X:2002 / Y:1900 / Z:9 |
| `24384_26229` | +1 `slope_mark`（头管 0.3%）；第 2 / 5 袋平面 30° → X:812 / Y:469、X:728 / Y:1388 |
| `24383_99996` | 第 5 袋（离 U 30°）→ Z:458 / X:264 / Y:14；第 10 袋（离 E 12°）→ X:442 / Y:94；没有落在坡度带里的管 |
| `24383_76191` | 第 1 / 3 / 17 / 19 袋平面斜向 → 各两条水平腿 + 直角标 |
| `24383_100028` | 第 5 / 9 / 13 / 20 / 22 袋斜向 → 分解腿 |

### 验证

- plant-mbd `cargo test --workspace` **80 通过**（75 lib + 4 + 1，新增 5 条 slope 用例）；`golden diff` 6/6；clippy 新代码零告警。
- gen-model worktree `mbd_branch_query` 4/4、`mbd_http_endpoint` 1/1（宿主没改，只是求解器多出图元）。
- 三条金样 wire 体经 plant3d-web `parseMbdV2PipeData` ACCEPTED（`145018`：slope_mark 7 / aid_line 21 / linear_dim 18 /
  aid_text 3，0 diagnostics）。前端 `mbdV2ExternalAnnotations.ts` 对 `slope_mark` 画线 + 箭头 + 中点文字、`aid_line` 画
  延长线样式、`aid_text` 画注释（代码已读；`node --experimental-strip-types` 跑不了它——扩展名省略的 bundler 导入）。

## Phase 5 ③（2026-09-07）：焊缝标记（`getwelds` / `isoWeldText`）→ `weld_mark`

plant-mbd `43c9882`（ADR-0008，新模块 `weld.rs`）+ `dc0eec7`；gen-model 本提交（宿主读 `DTXR` / `:ZD_WLDN`）。

### 读 PML 的结论（`isobran.getwelds` L4160-4759、`isoWeldText.pmlobj`、`isoline.drawweld` L3510-3587）

- **普通管道只认显式 WELD 成员**：位置 `pos of $!mem`，`dtxr` 含 `SHOP` 为车间焊、否则现场焊（L4206-4230）。
  按连接类型 BW / SW 在构件端口补焊点（L4401-4483）、Head / Tail 补焊点（L4236-4270）、OLET / TEE 支管焊
  （L4351-4399）、`sortwelds` 后顺序编 `M{x}` / `A{x}`（L4585-4595）——全部包在 `if !this.isinstpipe`
  （「EM4 use weld only」）里，是仪表管路径。
- **编号**：普通管道读 WELD 的项目 UDA `:ZD_WLDN`（L4665），没有就 `'Weld Num Unset'`（L4727）进图。
- **绘制**（`drawweld`）：编号是 `mlabel`，经 Polar 系统摆在管外（半径带 `od/2 + 0.5·cheight` … `+cheight`），
  离焊点超过 `od/2 + 2·cheight` 带引线（L3579-3585）；焊点位置本身 PML 不画符号（TPlant 侧按 JSON 画）。
- ATTA 焊缝（`getattawelds` L4892，要 `pipeatta` 的 PAD / LEG / LUG 几何分类）与 KD 焊缝（`getkdwelds`）未迁。

### 落地

- `weld.rs::weld_primitives`：每个 WELD 一个 `weld_mark {position, weld_type: shop | field}`（前端画圆圈，field 加叉）；
  宿主给了 `weld_number` 才出 `label`（管外 `od/2 + 1.5·cheight`、沿直段尺寸方向，直段没选出方向时朝上）+ 远时
  `leader_line`。「Weld Num Unset」是提醒建模人补 UDA 的，不进图。
- 宿主：WELD 成员 `get_string("DTXR")` / `get_string(":ZD_WLDN")`（`UnknownAttribute` 按没有）→ `BranchMember.dtxr /
  weld_number`。AvevaMarineSample 的 WELD 两者都空。
- 接线在 `layout_isoline` 的每条出口（`finish_with_welds`），不依赖主尺寸是否出得来。

### 金样

只有 `24384_26229` 有 WELD（`26230`，在 Head 上，`DTXR` 空 → field、无编号）→ +1 `weld_mark`。README 顺手更正：
第 2 袋中间的点 `26236` 是 VALV 不是 WELD。

### 验证

- plant-mbd `cargo test --workspace` **84 通过**（79 lib + 4 + 1，新增 4 条 weld 用例）；`golden diff` 6/6。
- gen-model worktree `mbd_branch_query` 4/4、`mbd_http_endpoint` 1/1；六份 field fixtures 重导后与已提交的逐字节相同
  （新字段两者都空、不落键）。
- `26229` wire 体经 plant3d-web `parseMbdV2PipeData` ACCEPTED（slope_mark 1 / aid_line 10 / linear_dim 6 / weld_mark 1 / aid_text 4）。

## Phase 5 ④（2026-09-07）：位号 / 信息标签（`isoTag` / `gettags` / `geteltag` / `getbranmlabel`）→ `label` + `leader_line`

plant-mbd `844ce0e`（ADR-0009，新模块 `tag.rs`，接线 `layout::place_tags`）；gen-model 本提交（宿主给名字 / STEXT / 连接对象）。
数据仍全部 e3d-io / e3d-model 直读，没有数据库查询。

### 读 PML 的结论

- `isoTag.pmlobj` L11-233：Head / Tail 的连接标签 = `接 <名>`（HREF / TREF 连到 NOZZ 用它自己的名、BRAN 用 BRAN 名、
  构件用属主 BRAN 的 `namn`）+ 属主 BRAN 的 `:ZD_NBBM` + `addpos` 的 `X e / Y n / PE u`（`D0` 整数）；连接对象在
  `*ITUB*` / `*INST*` 区域整条不出（Head 命中时 Tail 也不出，`!!mbdBranTail`）。TEE / OLET 的连接标签取支管口
  `p pos`（P1–P3 里不是 arrive / leave 的那个）+ `cref`；**没有 cref 也出坐标**（`addpos` 在 handle 块外）。
  ATTA：SITE 名不含 INST 取 `stext`（`'' / =0/0 / unset / Nulref` 不出），含 INST 走支撑 `:ZD_BZBM`（仪表支架）。
  PCOM 的 `crfa` 连接、`:YKSX` 仪表名（TEE / OLET / VALV）都在，本期不迁。
- `isobran.gettags` L1984-2280：ATTA 位号直通且不水平时补标高（SPRE 名含 FLOR 写 `EL +x.xxxm`，否则 `PE ±u`，
  L2026-2047）；BEND `<角度>°`（`D2` 去 `.00` 去尾零）+ `弯曲半径:R`（P1 / P2 各作垂线求圆心，L2093-2116；
  P1 / P2 重合的整条 skip）+ 转向时 `PE ±u`；ELBO 角度 ≠ 90 / 45 才写 + 转向时 PE；TEE 不转向整条不出；
  具名（`/` 开头）非 TUBI / ATTA / WELD / TEE / BEND / ELBO 构件出元件名（首行 `'  '` 给材料序号占位，`*KD-*` 截到 KD，
  `*KD-02` / `*KD-2` 跳过）。**FTUB 不是 TUBI**，PML 会给具名 FTUB 出元件名；本库 FTUB 与 TUBI 同角色、不出。
- `geteltag` L2282-2377：直通（adir / ldir 夹角不在 1°–179°）且不水平（ldir 与 U 夹角不在 87°–93°）的管件原点
  `PE ±u`，去重集合从 hpos / tpos 起步、1 mm 内不重；TEE / OLET 支管口与原点同高不标。
- `getbranmlabel` L5323-5386：`<NAME 第一段（去 /）>-<:ZD_GDWJ>`（`hbore` → AOD 换算表是注释掉的旧路径）；锚点
  `gettubimlabelpoint`：TUBI 的 apos→lpos 线（ATTA 处 `addAttapointintolines` 切开），`findhalfpos` 从中间下标向两侧找
  第一段长度 ≥ `minlen + ⅔·(maxlen − minlen)` 的，取其中点；没有直管时退到管件的 apos→pos / pos→lpos 段。
- `putIntoIsoline` L1144-1298：标签按 `isoline.mems` 归段；直通件与 Head / Tail 归第一条含它的直段，转向件（拐角在相邻
  两袋里都出现）归 `|angle(pipedir, U) − 90|` 最小（最水平）的那条；`name` 未设的对象（管道名称）按 `line.onprojected`
  + 最小距离归段。
- `isoline.drawtag` L3717-4060：字高 `cutpipe` 2 倍、管道名称 3 倍；半径带起点水平直段 `od/2 + 2·cheight`
  （管道名称 6 倍、cutpipe 5 倍），竖直直段单行 `od/2 + 0.5·cheight`（管道名称 2 倍）、多行 `2·cheight`；
  `getbestdir`（L3653-3692）TEE / OLET 沿支管口方向；Polar `getBestPosAndOri` 选位；**离锚点最近的那一行**带
  `dimpos` → `mlabel.addleadline` 引线（`mlabel.pmlobj` L235-269），其它行没有。

### 落地

- `tag.rs`：文字与锚点照搬上表；`tag_distance` 取半径带起点；`member_tags` 一条直段的成员 → `Tag` 列表，
  `branch_label_text` / `branch_label_anchor` 管道名称，`tag_primitives` → `label`（多行 `\n`）+ `leader_line`。
- `layout::place_tags`：全部直段跑完后一遍；拐角件按 `putIntoIsoline` 挑一袋（`member_turns` + 水平度），不重复；
  摆位方向取尺寸方向反侧（避开尺寸行；直段没选出方向朝上），TEE / OLET 沿支管口方向；管道名称归锚点所在直段。
- 宿主 `branch_query`：成员 `stored_name()` → `name`；ATTA（SITE 名不含 INST）`get_string("STEX")` → `stext`；
  TEE / OLET `get_element("CREF")` → `connection`；BRAN `stored_name` / `:ZD_GDWJ` / `HREF` / `TREF` →
  `name` / `label_bore_text` / `head_connection` / `tail_connection`。连接对象按 `isoTag` 解成
  `ConnectionRef { name, note }`（目标库没注册 / 不在会话 / UDA 不在字典按没有，`soft`）。`mbd_branch_dump` 的
  `cases[]` 多写 `branch_name` / `branch_label_bore`，CLI 与金样回归经 `LayoutRequest::with_branch_label` 喂给求解器。

### 与 PML 的差别（ADR-0009 全文）

摆位没有 Polar（方向近似、无文字定向、无每标签字高）；连接对象名一律 `namn`（PML 对 NOZZ 带 `/`）；管道名称无
`:ZD_GDWJ` 时不拼尾随 `-`；ITUB / INST 区域连接对象宿主不填、坐标照出；元件名首行空格不出；FLOR 的 `EL` 写法、
支架材料标签、仪表管分支、`:YKSX`、PCOM `crfa`、`cutpipe` 未迁。

### 金样（六条全变，`fixtures/golden/README.md` 逐条）

- `99996` +17 对：7 个支撑位号（`1RX02-LSUP-E099x`，`/DATUM` 那一半有 STEXT）、BEND `90° / 弯曲半径:350 / PE -2980`
  等、ELBO 60°、管道名称 **`LV`**（`/LV-GR-1RX215-B` 第一段；样本库没有 `:ZD_GDWJ`）。
- `145018` +14 对：Tail `接 Copy-of-RCS0014-1R43013新`（TREF 连到另一条 BRAN）；排水线斜坡让弯头角度 89.7x°，PML 也会写；
  OLET 支管口坐标标签；VALV 元件名。
- `26229` +12 对：支撑位号 `R332.001-BV` 等四条、BEND `100° / 弯曲半径:301.50`、VALV 元件名。
- `76191` +20、`100028` +15、`91813` +2（只有 Head / Tail 坐标：无名 BRAN、FTUB 不出元件名）。
- AvevaMarineSample 的 `/Copy-of-…` 命名让三条金样的管道名称都是 `Copy`——是数据，不是求解器。

### 验证

- plant-mbd `cargo test --workspace` **94 通过**（89 lib + 4 contract_guard + 1 golden，新增 9 条 tag 用例）；
  `golden diff` 6/6；`verify_mapping` 6/6；clippy 新代码零告警（剩 4 条是 `split_isolines.rs` 旧账）。
- gen-model worktree `mbd_branch_query` **5/5**（新增 `real_branch_carries_tag_texts_and_lays_out_labels`：真库 `99996`
  出 `LV` / 支撑位号 / BEND 三行 / Head-Tail 坐标，标签数 = 引线数）、`mbd_http_endpoint` 1/1。
- 四条金样 wire 体经 plant3d-web `parseMbdV2PipeData` ACCEPTED（`label` / `leader_line` 是冻结契约里有几何的 kind；
  多行 `label` 走前端 `multiLineTexts`）。
- 未验证：浏览器目测（需起服务）、PML 图纸人工对照（需 E3D）。