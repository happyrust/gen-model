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
