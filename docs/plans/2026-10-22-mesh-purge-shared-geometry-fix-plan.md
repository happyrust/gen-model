# 清理模型连几何一起清：`inst_geo` 行与 `.mesh` 文件随范围删除、靠重生成补回（单位几何除外）

日期：2026-10-22；状态：**已批准，已执行待验**（用户「开始执行修复」，无批注，D1–D6 全按推荐；M1–M4 已落地，§6 的门待你跑，见 §9）；起草：Cursor 代理（审核线）；批准位：用户。

> 第 1 稿的「删行后对键集反查、还被别处引用的不删」已按用户 2026-10-22 口径撤回。现行口径（用户原话）：
> 「将删除的方法私有化，放到重新生成模型的过程里。然后配合删除 meshes 里的 mesh 文件，因为最后都是要重新生成的，所以不用担心重用 mesh 的缺失问题」；
> 「不是整个库删除，而是我们在删除时，会把对应的 inst_geo 的信息也清理，对应的 mesh 文件也会删除」；`purge_meshes` 一格保留、实现换成内部那一份。
>
> 目标：把这个口径落成可执行的步骤、验收标准与决策点。审核基线：`refactor@635bf7a90` + 工作区未提交改动（`mesh_purge` 模块、两条 DELETE 的 `purge_meshes`、`/dbnums` 引导形状）；对口 spec `docs/specs/web-service-api.md` §4.5 / §4.5.1 / §4.5.1D / §4.7。
> 请在 Plannotator 里对 §3 批注，再整体批准或驳回。批准后按 §5 开工；§6 的门由你跑。

## 1. 出发点

现码事实（行号按 2026-10-22 工作区）：

| # | 事实 | 出处 |
|---|---|---|
| F1 | 每一条隐含直管段的 `tubi_relate.out` 都是常量 `inst_geo:⟨2⟩`（`TUBI_GEO_HASH`），方管 `⟨1⟩`（`BOXI_GEO_HASH`）；全项目所有 TUBI 共用这一行、这一份 `.mesh`；`gy_common.surql` 按 `inst_geo:⟨2⟩` 汇总管长 | `src/fast_model/cata_model.rs:1460-1464`、`src/fast_model/pdms_inst.rs:928-935` |
| F2 | 现有收集口会收到 `"1"` / `"2"`，`is_valid_mesh_key("2")` 放行，`remove_mesh_files_in` 会删 `<meshes_dir>/2.mesh` | `src/data_interface/mesh_purge.rs:172-188, 201-217` |
| F3 | 重生成不会写回 `1/2/3.mesh`：`ensure_mesh_file` 只对 `baked_mesh_id` 的键调用；`fill_basic_shapes` 只按常量 id 写回 `inst_geo:⟨2⟩` **行**，不写文件 | `src/fast_model/e3d_model_service.rs:3699, 3705`、`pdms_inst.rs:942-945` |
| F4 | `mesh_glb` 对缺文件回 404 `not_found`，是「客户端可负缓存」的档 | `src/web_service/handlers.rs:1429-1456` |
| F5 | 现行清理不删 `inst_geo`（d-8：清理只针对产物行）；`helper.rs` 有守卫 `cascade_delete_never_reclaims_shared_content_addressed_geometry` 钉 `render_cascade_delete` 不动 `inst_geo` | `src/data_interface/helper.rs:107-114, 571-590` |
| F6 | 重生成写行走 `render_inst_geo_upsert`（upsert），写文件走 `ensure_mesh_file`：行与文件被删后，这片范围重生成后**还在用**的键都会回来 | `pdms_inst.rs`、`e3d_model_service.rs:3699` |
| F7 | 两条 DELETE 的收集 / 删文件现在写在 handler 里，`clear_model_scope` / `wipe_dbnum_model_face` 自己不知道这件事；`force=true` 的先删后生走同一个 `clear_model_scope`，不删文件 | `handlers.rs:2652-2702, 3870-3920` |
| F8 | 引导支排在读透分支之前，读透下不带身份的 `/dbnums` 每 1–5 秒读一次 `SUL_DB`，读不到就推 warning | `handlers.rs:3401-3457` |
| F9 | 日期写成 2026-10-23（模块文档、两处字段注释、spec 三处）；`changelog.md` 无条目 | `mesh_purge.rs:1`、`handlers.rs:2589, 3780`、spec `:428, 548, 665` |

两件不随口径变的事：

1. **单位几何 `1/2/3` 必须硬排除**（行和文件都是）：文件没人写回（F3），行被全库所有管段引用（F1）。
2. **共享键的补回只覆盖这片范围重生成后还在用的**（F6）：数据改了、这片范围不再用的键，若别的库还在用，行与文件都补不回来，要等那个库自己重生成。按用户口径**接受**，记在 §7。

## 2. 待解决清单与归属

| # | 问题 | 工作包 | 严重度 | 状态 |
|---|---|---|---|---|
| 1 | 单位键 `1` / `2` / `3` 会被删（F1–F3） | W1-S1 | 阻塞 | 待做 |
| 2 | 删文件长在 handler 里、不在删除例程里；`inst_geo` 行不跟着清（F5、F7） | W1-S2 / S3 | 高 | 待做 |
| 3 | `force=true` / 整库重建的先删不连几何（F7） | W1-S4 | 中 | 待做（D2） |
| 4 | 子树档缺顺序钉、`dbnum_fast_delete` 吞 `purge_meshes`、同步删盘 | W3 | 低 | 待做 |
| 5 | 读透下引导支白读（F8） | W4 | 低 | 待取证 |
| 6 | 日期、changelog、spec 口径、d-8 注记（F9、F5） | W2 | 低 | 待做 |

## 3. 决策点（请批注；不批注即按推荐）

| ID | 问题 | 选项 | 推荐 |
|---|---|---|---|
| D1 | 连着删哪些几何行 | (a) 只删 `inst_geo:⟨k⟩` 行 + `.mesh` 文件；`aabb` / `trans` / `vec3` 这几张内容寻址小表照旧不动；(b) 连 `aabb:<mesh id>` 共享盒一起删 | **(a)**：那几张表一行几十字节、一直就是「泄漏有界、不回收」的处理；网格才是占盘的那一半。少删一张表少一类悬引用 |
| D2 | `force=true` 范围重生成与整库重建（`/dbnums/{dbnum}/model/rebuild`、启动续跑的留痕）的先删要不要连几何 | (a) 一律连几何清（马上就重生成，正是「放到重生成过程里」）；(b) 只有两条 DELETE 按 `purge_meshes` 清，重生成的先删照旧只删行 | **(a)**：这是用户口径的本意；重生成收尾会把还在用的行与文件写回（F6）。代价见 §7 第 1 条 |
| D3 | Memory 路由（只写内存 / 读透）的库 | (a) 没有权威行可删，只删投影记录指向的 `.mesh`；(b) 文件也不删 | **(a)**：与现在 `include_durable` 的判据一致；投影记录的 `mesh_id` 就是文件名 |
| D4 | 回执 | (a) `mesh_files_removed` / `_absent` / `_failed` 三格照旧，多一格 `inst_geo_removed`，单位键跳过不单列（永远是 0–3，没有信息量）；(b) 只留原三格 | **(a)**：行数和文件数对不上时（文件本来就不在 / 删失败）得看得出来 |
| D5 | 数据面 `DELETE /dbnums/{dbnum}/data` 收到 `purge_meshes=true` | (a) `400 bad_request`；(b) 维持静默忽略 | **(a)**：一行 if，免得调用方以为网格也清了 |
| D6 | 读透下不带身份的 `/dbnums` | (a) 引导支在 `data_face().is_read_through()` 时跳过 `observations_for_all`；(b) 维持现状 | **(a)**，W0 先实测一次：读透下 `dbnum_state` 若在且有意义则改 (b) |

不再当决策点的：反查（撤回）；plant-ui 那一格的默认值（维持「默认勾上」，前端不动）；收不动的 HTTP 码（两条入口都走 `ensure_error` 的分型，spec 照实写）。

## 4. 工作包

### W0 取证（不改代码，结论写进 §9）

1. `inst_geo` 的读方清单：`grep inst_geo src/ resource/`，列出谁会沿 `geo_relate` / `tubi_relate.out` / `booled_id` 读到一行 **已不存在** 的 `inst_geo`（记录读取器、房间 / 净距、`gy_common.surql`、空间树），每一处写明是「返回空」还是「报错」。报错的那几处要在 W1 里改成容错，或列进 §7。
2. `render_inst_geo_upsert` 在生成路径上是否对每一个键都走到（含 `booled_id` 那一份），确认 F6。
3. 单位球的常量名：`aios_core::prim_geo::basic` 里 `BOXI_GEO_HASH` / `TUBI_GEO_HASH` 之外那一枚；没有就在 `mesh_purge` 里点名 `"3"` 并注明出处（`mesh_glb::repository_unit_meshes_convert_end_to_end`）。
4. 读透形态起一次服务，不带身份打 `/dbnums` 三拍，看 `warnings` 是否每拍多一条（定 D6）。

### W1 内部删除例程（`src/data_interface/`）

- **S1 单位键硬排除**（可单独提交，先堵阻塞项）：`mesh_purge.rs` 新增 `shared_unit_keys()`（`BOXI_GEO_HASH` / `TUBI_GEO_HASH` / 单位球）；删行与删文件两步都先把这三枚从键集里去掉，不拼路径、不发 DELETE。单测：键集放 `"1"` / `"2"` / `"3"` + 目录里真放三份文件 + `mem://` 里真建 `inst_geo:⟨1⟩` / `⟨2⟩`，断言文件与行一个都没动。
- **S2 收成内部例程**：`mesh_purge` 模块改 `pub(crate)`，对外只留一个入口 `purge_geometry(keys, PurgeTarget) -> PurgeReport`：按 D1 删 `inst_geo:⟨k⟩`（一句 `DELETE` 带 `$keys`，分批），再 `remove_mesh_files_in`；`PurgeTarget::Durable` 行 + 文件，`PurgeTarget::FilesOnly` 只文件（D3）。收集 SQL（子树 / 整库两句）和投影收集照留，还是「收在删行之前」。`std::fs::remove_file` 套 `spawn_blocking`。
- **S3 接进两条删除路**：
  - `AiosDBManager::clear_model_scope(refno, roots)` 加一个 `ClearGeometry::{Keep, Purge}` 参数：`Purge` 时在现有删行之前收键（投影 + 库上，库上只在 `Database` 路由），删行之后调 `purge_geometry`。Memory 路由在 `scope_memory_only` 里只做 `FilesOnly`。
  - `fast_delete::wipe_dbnum_model_face(dbnum)` 同款：闸门 → 收键 → 按 Ref0 区间删行 → `purge_geometry`。`inst_geo` 不按 Ref0 寻址，所以收键这一步省不掉。
  - `render_cascade_delete` 与 `helper.rs` 的守卫**不动**：级联删本身仍不回收 `inst_geo`，回收是清理 / 重生成上显式的一步；注释补一句指向 `mesh_purge`。
  - handlers：两条 DELETE 把 `query.purge_meshes` 映射成 `ClearGeometry`，**自己不再收键、不再删文件**；回执按 D4。
- **S4 重生成路**（D2 (a)）：`/model/generate` `force=true` 那次 `clear_model_scope` 传 `Purge`；`model_scope_regen` 留痕的启动续跑走同一条 `force` 路，不另加开关。**整库重建不动**（执行时核实）：`model_rebuild::start` 只排一条起点 0 的 `ModelOnly` 任务、逐根覆盖，进程内执行器（`EnsurePurpose::ForcedRebuild`）也只是逐根强制生成，两条都没有「先删」这一步可挂；要让整库重建也清几何得改逐根生成的 delta 路，不在本计划。
- **S5 测试**：
  - `mem://` 真起一台：子树与整库各一条——范围内的 `inst_geo` 行被删、范围外的不动、单位行不动；文件同款。
  - 源码钉：`clear_model_scope` 与 `wipe_dbnum_model_face` 内部顺序「收键 → 删行 → 删几何」；两条 DELETE handler 不再直接调 `mesh_purge::`（只许传 `ClearGeometry`）；`force=true` 路传 `Purge`；现有 `dbnum_model_delete_checks_confirm_then_scope_then_the_gate_then_deletes` 里关于 `mesh_purge::` 的断言改成钉 `ClearGeometry`。
  - `the_purge_is_all_or_nothing_on_the_collection_side` 保留（收不动仍是整趟中止、一行不删）。

### W2 文档与口径

- spec §4.5.1 / §4.5.1D：`purge_meshes=true` = 连这片范围引用的 `inst_geo` 行与 `.mesh` 一起删；单位几何永不删；共享的靠重生成补回、这片范围不再用的补不回（写明）；回执按 D4；§4.5 `force` 条目补「先删连几何」（D2）。d-8「清理只针对产物行」那句改成「级联删只针对产物行；几何行与文件由 `purge_meshes` / 重生成显式回收」。
- 日期：`mesh_purge.rs:1`、`handlers.rs:2589, 3780`、spec 三处 2026-10-23 → 2026-10-22。
- `changelog.md`：按仓里格式补一段（为什么 / 改动 / 验证），把 F1–F3 与用户口径写进「为什么」。
- `helper.rs:107-114` 那段注释补一句：显式回收在 `mesh_purge`，单位几何永不回收。

### W3 顺手修（`src/web_service/handlers.rs`）

- D5：`dbnum_fast_delete` 开头对 `purge_meshes` 回 400，源码钉一条。
- `model_delete_subtree`：`route` 为 `None` 时不收键（`is_none_or` → `is_some_and`）。
- 子树档补顺序钉 `model_delete_subtree_passes_purge_to_the_clear_routine`。

### W4 引导形状（D6）

- 按 W0-4：读透下跳过 `observations_for_all`（源码钉 `the_bootstrap_shape_skips_watermarks_under_read_through`），或不动并在 §9 记下原因。

## 5. 执行顺序与里程碑

| 里程碑 | 内容 | 产出 |
|---|---|---|
| M0 | W0 取证 | §9 结论；D6 定稿；`inst_geo` 读方里要改容错的清单 |
| M1 | W1-S1 单位键硬排除 | 提交 1：`mesh_purge.rs` + 单测 |
| M2 | W1-S2 / S3 / S5：内部例程 + 两条删除路 + 钉子 | 提交 2 |
| M3 | W1-S4 重生成路（D2） | 提交 3 |
| M4 | W2 + W3 | 提交 4：spec / changelog / 日期 / 顺手修 |
| M5 | W4 | 提交 5（若 D6 取 (a)） |

每个里程碑一个提交，`git revert` 即回。不进已验证的 0.1.43（`7fdc4c15c`），随下一版上线。

## 6. 回归门（每一步合入前，由你跑）

| ID | 断言 | 命令 |
|---|---|---|
| V1 | `mesh_purge` 单测全过（含单位键、`inst_geo` 行删 / 不删、文件） | `cargo test --lib -- mesh_purge` |
| V2 | 删除例程与 handler 的源码钉全过 | `cargo test --lib -- web_service::handlers`、`cargo test --lib -- on_demand_model`、`cargo test --lib -- fast_delete` |
| V3 | 全量不变差（基线取当前 HEAD 的通过数） | `cargo test --lib` |
| V4 | 服务端二进制按 CI 同款特性集编过 | `cargo check --bin aios-database --no-default-features --features ws,gen_model,manifold,project_hd,http_api` |
| V5 | 手工（测试实例，先备份 `assets/meshes/{1,2,3}.mesh`）：对含管段的根 `DELETE /api/v1/model/subtree?…&purge_meshes=true`，回执里 `inst_geo_removed > 0`；`GET /api/v1/meshes/2.mesh` 仍 200；`SELECT * FROM inst_geo:⟨2⟩` 仍在 | curl + surreal sql |
| V6 | 手工：同一根 `POST /api/v1/model/generate` `force=true`，生成完该根的 `inst_geo` 行与 `.mesh` 都回来，`/model/records` 与 plant-ui 显示完整 | curl + plant-ui |
| V7 | 手工（记录预期行为）：两个库共用一只目录件，整库清其中一库并勾网格 → 另一库那只件 404（预期，§7 第 1 条）；重生成被清的那个库后恢复 | curl |

任一变差就停：保留命令与输出，回到对应工作包重议。

## 7. 风险与不做事项

1. **接受的代价**：这片范围重生成后不再用、别的库还在用的共享键，行与文件都没了，那个库的那些件 404（前端可能负缓存），直到它自己重生成。整库清理 + 勾网格时这个面最大。用户 2026-10-22 拍板接受，不做反查、不做引用计数。
2. 删行到重生成写回之间的窗口：范围重生成是秒级，整库重建是分钟级；窗口内别的库共用的键也 404。
3. `inst_geo` 的读方对缺行的容错（W0-1）：读到悬引用就报错的地方要先改成返回空，否则 1 条的症状从「那些件不显示」变成「整根拉不出来」。
4. 收键与删行之间若有生成收尾写入新键，文件留在盘上——有界泄漏，不加锁。
5. 不做：反查 / 引用计数；全库后台 sweep；改 plant-ui（那一格照旧）；改 0.1.43；本计划内不跑编译 / 测试（§6 由你跑）。

## 8. 批准栏

- [x] 批准：按 §4–§6 执行（决策点未批注的按推荐项）——用户 2026-10-22「开始执行修复」，未走 Plannotator、无批注
- [ ] 驳回 / 改口径：无

## 9. 执行记录

- 2026-10-22 · 第 1 稿（删后反查）送审；用户回「弄得复杂了」，改口径为「删除时连 `inst_geo` 和 `.mesh` 一起清，靠重生成补回」，`purge_meshes` 保留。第 2 稿按此重写。
- 2026-10-22 · 用户「开始执行修复」（无批注，D1–D6 全按推荐）。落地（未编译、未跑测试，§6 由用户跑）：
  - M1 `mesh_purge.rs`：`pub(crate)`；`ClearGeometry::{Keep, Purge}`（`from_flag` / `purges`）；`shared_unit_keys()` = `BOXI_GEO_HASH` / `TUBI_GEO_HASH` / `"3"`（aios-core 没有单位球常量，按 `mesh_glb` 测试点名的 `3.mesh` 硬编），`remove_mesh_files_in` 与 `delete_inst_geo_rows` 都跳过它们（`shared_unit_skipped`）；单测 `the_shared_unit_meshes_are_never_removed_from_disk`、`the_inst_geo_delete_skips_unit_rows_and_leaves_other_rows_alone`（`mem://`）。
  - M2 `purge_geometry(db, keys, durable)`：分批 `DELETE inst_geo:⟨k⟩;`（500 键一句）再 `spawn_blocking` 删文件；`MeshPurgeReport` 加 `inst_geo_removed` / `shared_unit_skipped`。`on_demand_model::clear_model_scope(refno, roots, geometry) -> Option<MeshPurgeReport>`：收键（`durable = !(memory_only || read_through)`）→ 现有删行 → `purge_geometry(&SUL_DB, ..)`；`fast_delete::wipe_dbnum_model_face(dbnum, geometry)`：生成锁内收键 → `wipe_dbnum_model_face_on` → `purge_geometry`，`ModelFaceWipeResult.geometry`（`serde(skip)`）。两条 DELETE handler 只传 `ClearGeometry::from_flag(query.purge_meshes)`，回执由 `annotate_geometry_purge` 写四格。源码钉：`fast_delete::the_model_face_wipe_collects_geometry_before_the_rows_and_purges_after`；`handlers` 两条现有钉改钉 `ClearGeometry` + 不再直调 `mesh_purge::`；`on_demand_model` 的 `forced_regeneration_clears_the_requested_scope_before_generating` 改钉 `ClearGeometry::Purge`；`mesh_purge::the_purge_is_all_or_nothing_on_the_collection_side` 加钉删行先、删文件后。
  - M3 D2：`regenerate_scope` 的 `force` 先删传 `Purge`。整库重建没有先删这一步，不动（W1-S4 已改口）。
  - M4 W2 / W3 / W4：spec §4.5 `force` / §4.5.1 / §4.5.1D（回执四格、数据面 400）/ §4.7（读透不读水位）；日期改 2026-10-22；`changelog.md` 2026-10-22 一段；`helper.rs` 注释补一句；`dbnum_fast_delete` 收到 `purge_meshes` 回 400（钉在整库那条测试里）；`dbnums` 引导支 `is_read_through()` 时跳过 `observations_for_all`（`the_bootstrap_shape_carries_the_gate_material` 加钉）。`model_delete_subtree` 解不出所属库的分支不再单独处理（收键改在 `clear_model_scope` 里，按作用域判）。
  - 补（夹具取证后）：`D:\\work\\plant-code\\old\\test-issue-regen-model` 的 ams.db 里 `tubi_relate.out` 不是 `inst_geo:⟨2⟩`，而是 11 个按段数哈希的单位圆柱键（如 `2542589811961169111` × 3168 行、`2714441752093137411` × 560 行），单位键硬排除护不到它们。按「最稳妙」处理：收集不再看 `tubi_relate.out` 与投影里的管身记录（重生成写回的还是同一个键，删它只会让别的库的管段白白 404）；spec / changelog 同步。要改回「管身也删」是一行过滤的事。
- 2026-10-23 · 夹具实测（用户授权：停 PID 68992 / 58128，`cargo build --bin aios-database` 50 s 通过，以 `DB_OPTION_FILE=.scratch/fixture-8023/DbOption.toml`（根配置改 v_port=1516 / http 8023 / room_membership=false / enable_log=true）+ `PLANT_ASSET_ROOT=D:\\work\\plant-code\\old\\test-issue-regen-model` 从仓根起 8023，build `0.1.43+g376ce770d311…dirty`，model_ready）：
  - BRAN `/1RCV0196-60.3-VMC-R80-R321` = `24383/74283`（dbnum 7999），`POST /api/v1/model/generate {force:true}`（右键「重新生成模型」调的同一条）：2.3 s，`Generated`，16 件，`published_via=regen`，durable。
  - 先删连几何：7 个 baked `.mesh` 被删后由 `ensure_mesh_file` 写回（mtime 18:31/23:38 → 03:02:19，内容与备份逐字节相同；`ensure_mesh_file` 对已存在且一致的文件不写，所以改写就是删过的证据）；管身单位圆柱 `2714441752093137411.mesh`（560 根管段共用）与其 `inst_geo` 行原样不动（mtime 20:48:27）；`1/2/3.mesh`、`inst_geo:⟨2⟩` 在；`/model/records` 16 条、非管身键集前后一致（7 个）。baked 键本来就没有 `inst_geo` 行，所以这根的 `inst_geo_removed` 是 0。
  - GUI（windows-mcp 经 HTTP 驱动）：plant-ui 能起（`PLANT_UI_SETTINGS_FILE=…\\.scratch\\ui-test\\settings.ron`，指 8023），但 bevy 窗口不在 UIA 树里、`App switch` 找不到句柄，桌面上另有最大化的 Factory 窗口反复盖上来，第一轮右键流程没走到底。
  - 2026-10-23 09:39 GUI 补跑通（后端 PID 48696 / plant-ui PID 13836 重起）：`SetWindowPos` 把 bevy 窗口置顶摆到 (100,80)–(2400,1360)（屏幕 2560×1440 @150%，windows-mcp 坐标是物理像素）；pyautogui 键盘事件（scancode 0）进不了 bevy，改用 `SendKeys` 走剪贴板 `^a ^v` 把 `/1RCV0196-60.3-VMC-R80-R321` 贴进搜索框（逐字 `SendKeys` 会丢字母后面的数字），点结果 → 树展开到 BRAN → 右键「重新生成模型」→ 确认框「重新生成」。服务端 `[http.model.generate] refno=24383/74283 force=true` → 2.67 s `generated … available=true`；plant-ui 日志「生成完成，1 个生成根（重算 1，命中 0）… 查询到 12 个元素，16 个网格实例 … 模型显示完成」，三维视图画出整根支管。盘上：7 个 baked `.mesh` mtime 03:02:19 → 09:39:10.8，与备份逐字节相同（删过再写回）；网格目录 55867 个文件里 09:38 之后只动了这 7 个；`1/2/3.mesh`、`2714441752093137411.mesh` 原样；`inst_geo` 总数 21372 不变，`⟨1⟩⟨2⟩⟨3⟩⟨2714441752093137411⟩` 行都在；`/model/records` 16 条 complete。
  - W0 未做（要跑服务 / 查库）：`inst_geo` 读方对缺行的容错、读透下 `dbnum_state` 在不在。D6 先按推荐 (a) 落了；读方容错按 §7 第 3 条留意，跑 V6 时看 `/model/records` 与 plant-ui 有没有报错。
