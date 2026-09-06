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

- 14–26 mm 短管（`24383_100028`）PML 是否出尺寸：`getcuttubi` 有没有最短阈值。
- 隐式管身两端「E3D 不画」的槽位 PML 怎么处理（`24383_76191` Head 侧 638.9 mm 偏 40° 的那段）。
- 六条金样的切段数与拐点对照 `isobran.getisolines`——这一遍需要 E3D 里开同一批 BRAN。
