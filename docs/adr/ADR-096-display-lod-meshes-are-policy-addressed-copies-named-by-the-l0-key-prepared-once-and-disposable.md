# ADR-096：显示用粗网格是借 L0 键命名、按策略版本寻址、准备一次、随时可丢的副本

状态：**已接受**（2026-11-06，`feat/mesh-lod1`；取代该分支首个提交 `b0950c906` 里「按元素真实半径重算标准图元、无标记、无回填」的形态）。
日期：2026-11-06
关联：ADR-087 D3（内容寻址网格的校验与原子重写）、ADR-068 D7（网格目录的唯一解析口）、ADR-079（持久模型事实只有水位；粗网格不是模型事实）、plant3d-web `docs/plans/2026-11-06-dtx-multidraw-pass-split-and-pick-plan.md`（前端按屏幕投影直径选档、`?lod=1` 补拉、404 负缓存）。

## 背景

- plant3d-web 的 DTX 层对屏幕投影直径小于阈值的构件改画粗网格，键是 L0 的内容键加 `?lod=1`，没有就 404 负缓存。后端此前（`b0950c906`）在 `AIOS_MESH_LOD1=1` 时把每根在弦高容差下限 4 mm 下再求一遍，写 `meshes/lod1/{L0 mesh_id}.mesh`。
- 审核（2026-11-06）发现三件事：
  1. **标准图元的粗网格内容不确定。** L0 的标准圆柱键只含段数（`CylinderV3 { segments }`），同一段数下所有半径共用一只单位圆柱、一个 `mesh_id`；粗网格却按每个元素的真实半径在 4 mm 下重算段数。0.5 mm 与 4 mm 两套分带边界不对齐，r = 400–5000 mm 的 41 个 L0 键里 14 个映射到两种粗段数（如 n0=68 → {24, 28}），`ensure_mesh_file` 的逐位校验失配就走 `repair_mesh_file` 来回重写，账面把 `Repaired` 记成 `written`，看不出来。
  2. **每根都重求。** 盘上已有粗网格也不看，求过了不值得也不记；稳态下每次重生成都多付一次整根求值，而且 L0 缓存命中的根与开关之前生成好的库一个粗网格都拿不到，没有回填路。
  3. **寻址口径不自洽。** 文件名借 L0 的内容键，却给了 `immutable` 一年缓存头；策略（容差 / 比例 / 门槛）一变，客户端一年内看不到。

## 决定

**D1　粗网格是副本，不是模型事实。** 它不进任何身份键（`baked_mesh_id` / `canonical_mesh_id`）、不进投影记录、不进房间判定、不进 RVM 对拍、不进回执与水位；失败只记日志与报表（`E3dPersistReport.lod1_*`），不影响 L0 发布与任何成败判定。整个目录删掉只是前端回到画原网格。

**D2　文件名借 L0 的键，目录名带策略版本。** 路径 `meshes_dir()/lod1-v{N}/{L0 mesh_id}.mesh`（`fast_model::mesh_lod1::LOD1_DIR`，`LOD1_POLICY_VERSION = 1`）。容差下限（4 mm）、保留比例（≤ 0.7）、门槛（L0 ≥ 256 三角形）任一变动就升版本，新目录从空开始；旧目录不自动删，启动时 `warn` 点名（`stale_lod1_dirs`）。内容不是键的摘要，所以这里的文件**不过** `verify_content_addressed_mesh`；`?lod=1` 的响应**不给 `immutable`**——`Cache-Control: public, max-age=604800`、`ETag: "{键}-lod1v{N}"`。

**D3　标准图元的粗网格是 L0 键的纯函数，启动时一次写全。** `lod1_segments_for(n0)`：取按 0.5 mm 口径严格落在 `n0` 分带内部的代表半径（使 `2·acos(1 − 0.5/r) = 360/(n0 − 2)`），按 4 mm 走同一条 `circle_segments_uncapped`（45° 封顶、补到 4 的倍数、封顶 `MAX_SEGMENTS`），再过比例门；`BoxV1` 不出粗网格。会出粗网格的 L0 键是 68、72、…、1000 共 234 个（`canonical_lod1_source_segments`），`prepare_canonical_lod1` 在服务启动（开着开关时，`spawn_blocking`，不阻塞启动）与每次回填开始时一次写全，幂等。单位圆柱按与 e3d-model `build_canonical` 同一只 `Manifold::cylinder(1, 1, 1, n, false)` 建、**不进**它的进程级缓存（234 只常驻要几十 MB），单测钉住两边 `canonical_mesh_id` 逐位相同。生成期遇到标准图元只查进程内裁决表（按（目录，键）记），不再按真实半径重算。

**D4　目录件 / CSG 件先按盘分拣，只对真正缺的键重求。** `plan_lod1`：L0 ≥ 256 三角形的非标准图元按 `mesh_id` 去重后看盘——有 `.mesh` 计 `reused`，有 `.none` 标记计 `known_skipped`，都没有进 `pending`。`pending` 为空**不做**粗精度求值。非空才 `with_min_chord_tol(4.0)` 把整根再求一遍，`write_lod1` 按 `geometry_id` 匹配：省够的落 `.mesh`，省不够（或 L0 本来就小、粗网格空）的写零字节 `lod1-v{N}/{键}.none`（HTTP 只拼 `.mesh`，看不见它）；粗快照里找不到、或粗容差下变成了标准图元（单位网格与 L0 的实尺寸网格不同系，写出来就是错的）的计 `unmatched`、不留标记、下次再试。

**D5　清理随 L0。** `mesh_purge::remove_mesh_files_in` 删 `{键}.mesh` 时一并删 `lod1-v{N}/{键}.mesh` 与 `.none`，只计 `.mesh` 进 `MeshPurgeReport.lod1_removed`，删失败只记日志。粗网格借的是 L0 的名字，L0 没了它就是孤儿。

**D6　回填接口给开关之前生成好的库补课。** `POST /api/v1/mesh-lod1/backfill { dbnums }` → 202 任务（`TaskRegistry` kind `mesh_lod1_backfill`，同时只跑一次 409，开关没开 412），`GET …/{task_id}` 看进度与账。任务体：`prepare_canonical_lod1` → 逐库 `E3dModelService::backfill_lod1`：交付单元根按初始化同一条路从文件枚举（`initialization_roots`），每根在几何闸下 L0 只求值、只算身份（`element_mesh`，与 `bake_element` 同一函数）——**不写 L0、不算房间、不碰库**——L0 文件不在盘上的件视为未发布、跳过，其余交 D4。一根失败只记账；一个库起不来记 `dbnums_failed`。再跑一次全是 `reused` / `known_skipped`。

**D7　账单独记。** 逐根日志 `model root generated: … bake={ms} lod1={ms}`——粗网格的时间不许混进 `bake=`（那一栏是用来定位长尾的）。`BakedSnapshot.lod1: Lod1Bake { written, reused, known_skipped, skipped, unmatched, failed }` 进 `E3dPersistReport.lod1_written / lod1_reused / lod1_skipped（= skipped + known_skipped）/ lod1_unmatched / lod1_failed`。`Repaired`（盘上内容与本次不符而重写）计 `written` 并 `warn` 点名：同一键只该有一种内容，走到这里说明策略变了却没升版本。

## 选项与取舍

- **粗网格自己内容寻址（`lod1/{粗网格摘要}.mesh` + 一张 L0 键 → 粗键的映射）**：前端手里只有 L0 键，映射要么进投影记录（粗网格就成了模型事实，违背 D1），要么多一次往返。借 L0 的名字、用目录名承担版本，前端一个字不改。
- **标准图元按真实半径算、再去重**：分带不对齐这件事是数学上的，去重救不了内容不确定；而 L0 键本就只含段数，粗网格由段数推出才与 L0 的寻址粒度一致。
- **保留 `immutable`，策略变就换 L0 键**：L0 键是 RVM 对拍与投影的身份，不能为显示增强而变。换目录名便宜得多。
- **把回填做成 CLI 子命令**：仓内 CLI 的纪律是纯 HTTP 客户端（`main.rs` 的 `Trace`），服务跑着时直接起子进程会撞实例锁；HTTP 任务与 rvm-check 同形，`curl` 即可。
- **在生成 worker 里把粗网格挪到 `queue.store` 之后或后台线程**：D4 之后稳态下每根只是几次 `exists`，不值得再加一条异步账。

## 实施约束

1. `LOD1_DIR == format!("lod1-v{LOD1_POLICY_VERSION}")`（`the_directory_name_carries_the_policy_version`）；改策略先升版本。
2. 纯函数：`lod1_segments_are_a_pure_function_of_the_l0_key`（234 个键、4 的倍数、单调、都省够 30%）、`the_mapping_ignores_an_ambient_chord_tolerance_floor`（线程局部下限不改映射）、`the_band_radius_lands_back_on_its_own_segment_count`。
3. 名字对得上：`the_uncached_unit_cylinder_has_the_same_content_id_as_the_canonical_one`——e3d-model 改了 `build_canonical` 这条先红。
4. 分拣与写盘：`plan_sorts_sources_by_what_is_already_on_disk`、`write_lod1_keeps_marks_and_reports_unmatched`、`prepare_writes_every_canonical_lod1_once_and_reuses_afterwards`。
5. 读与删：`lod_mesh_dir_maps_lod_to_a_policy_versioned_subdirectory_or_rejects`、`mesh_glb_handler_caches_lod1_for_a_week_and_l0_forever`、`the_purge_takes_the_lod1_copy_and_its_marker_along_with_the_l0_mesh`。
6. 前端门槛常量 `DTX_LOD1_MIN_SOURCE_TRIANGLES` 与 `LOD1_MIN_SOURCE_TRIANGLES` 同值（256）；改一边要改另一边——没有共享契约文件，靠这两处注释互指。
7. 文档不写访问端点、令牌（`AGENTS.md`）。

## 后果

- 开着开关的稳态生成几乎不再为粗网格付求值成本：只有新键第一次出现时多求一遍整根。
- 已有库要先 `POST /api/v1/mesh-lod1/backfill` 一次才有粗网格；之后的生成顺手维持。
- `lod1-v{N}/` 可以整目录删掉再回填，前端只是暂时回到原网格。
- 线程局部的 4 mm 下限只影响原容差低于它的路径（正体曲面原语 0.5 mm）；负体刀（10 mm）、SNOU（5 mm）、截面弧（10 mm）、GENPRI（自带 6 mm 弦高）不变，这些件的粗网格省不够会被比例门拦下、留 `.none`，不会出错，只是拿不到 LOD。
- 旧目录 `meshes/lod1/`（`b0950c906` 的形态，只在联调机上出现过）不再读写，启动时点名，手工删。

## 不做

- 不做多档（`lod=2` 及以上）；前端只有一档粗网格的用法。
- 不按构件尺寸或场景自适应容差；那是另一个策略版本的事。
- 不做跨机器的粗网格分发；它随网格目录走（ADR-068）。
