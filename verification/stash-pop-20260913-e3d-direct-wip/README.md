# e3d-direct 主工作树在途改动（stash@{0}）合并 gen-model-refactor 后回灌的裁决 · 2026-09-14 00:30

**给谁看**：这批在途改动（dbslot / 惰性路由那条线，`verification/20260912-current-change-review.md` 是它自己的进度记录）的作者。
它们在 2026-09-13 22:55 被 stash（fable-5-1-49，为了把 `gen-model-refactor` 并进 `e3d-direct`），
合并（`793176510`）与 AMS 复验（`1fcb32934`）之后由 fable-5-1-58 回灌到主工作树。**回灌不是无损的**——
这份文件说清楚哪些留下了、哪些丢了、为什么，以及丢掉的部分去哪里找。

## 原件在哪（一个字节都没丢）

| ref | 内容 |
|---|---|
| `wip/mainwt-stash-20260913`（`0cca6418a`） | stash 原样：35 个修改 / 9 个删除，第三个父提交里是 26 个未跟踪文件 |
| `wip/mainwt-stash-20260913-lf`（`4da56554e`） | 同一份内容，行尾归一到仓库约定（`.rs` 为 LF），未跟踪文件直接进树 |
| `stash@{0}` | 已按用户确认 drop（2026-09-14 00:29） |

基线是 stash 当时的 `e3d-direct` HEAD `cf168862a`（`docs(release): 记录 0.1.22 本地发布`）。

## 为什么不是一句 `git stash pop`

1. **行尾噪声**：stash 里 10 个 `.rs`（`increment_manager.rs` / `lib.rs` / `handlers.rs` / `web_service/mod.rs` /
   `read_through.rs` / `db_file_header.rs` / `element_attributes.rs` / `e3d_mesh_store.rs` / `direct_tree.rs` /
   `data_interface/mod.rs`）被整文件 LF→CRLF 了；原始 diff 17608+/18515−，去掉行尾后真实改动是
   **38 文件 1444+/2351−**。直接 pop 会在这 10 个文件上撞出成百条冲突块。另有 6 个文件
   （`Cargo.toml` / `DbOption.toml` / `changelog.md` / `docs/release-notes/0.1.22.md` /
   `scripts/templates/DbOption.factory.toml` / `verification/e3d-version-20260911/database-field-redesign.md`）
   只有空白改动，丢弃。
2. **两套分叉的实现**：归一后对 HEAD 做 `merge-tree` 干跑仍有 **21 个文件冲突**（66 个冲突块）。
   WIP 这边是 `DirectStoreRegistry` + 自己那版 `MdbRouteDirectory` + `DirectStoreError::NoFileForDbnum` +
   `model_source::dbnum_from_word0` + `model_generation_flight/version`；refactor 并进来的 HEAD 这边是
   `MdbReadProvider`/`MdbReadStore` + `refno_route` 位解码 + `InvalidRefNo`/`DbNotInMdb`/`DbNumberMismatch` +
   `generation_flight::Flights`。两边奔的是同一批目标（启动不预热骨架、按 RefNo 定库、惰性路由），
   HEAD 那一版已经合并、编过、AMS 真库复验过。WIP 新增的代码行只有 3–15% 能在 HEAD 里找到——
   不是同一份工作的先后快照，是分叉。

## 裁决原则

**HEAD（已合并的主线）为准；WIP 里不依赖它自己那套类型、能独立成立的改动叠上去；分叉的那半不硬合。**
每个冲突块都看过，没有一块是「同一实现的小差异」，全部取 HEAD。

## 保留（8 文件，259+/5−，现在是主工作树里的未暂存改动）

| 文件 | 改动 | 说明 |
|---|---|---|
| `scripts/New-LocalRelease.ps1` | +12/−3 | `Get-Sha256Compat`（PS 兼容的 SHA-256）+ 3 处调用替换；冲突块取 HEAD 后手工把这三处补回，与 WIP 完全一致 |
| `src/fast_model/e3d_model_service.rs` | +104/−1 | `E3dModelService::current_cached_roots`；`GenerationRootMissing` + `classify_required_root_error`（挂在 Required 策略的失败链上）；单测 `read_through_receipt_is_invalidated_by_each_target_identity_change` |
| `src/fast_model/model_memory_store.rs` | +68 | `ModelMemoryStore::evict_missing_root` + 2 条单测 |
| `src/fast_model/e3d_mesh_store.rs` | +41 | 单测 `checked_mesh_reader_preserves_legacy_archive_bytes_and_rejects_corruption` |
| `src/surreal_retry.rs` | +22 | `closed_response_channel_is_retried_and_recorded` 改为子进程隔离跑 |
| `src/data_interface/model_coverage.rs` | +2/−1 | `startup_coverage_floors_on` 的 SQL 加 `AND (source_end_sesno ?: 0) > 0` |
| `src/data_interface/mdb_memory_store.rs` | +6 | `element_scan_count()` 计数器 |
| `src/data_interface/mdb_membership.rs` | +4 | `#[cfg(test)] MdbMembership::test_with_databases` |

## 丢弃（改动没有进主工作树；要找就去上面两个 wip 分支）

| 文件 | WIP 真实改动 | 为什么丢 |
|---|---|---|
| `src/data_interface/direct_store.rs` | +454/−125 | dbslot 那一整套（`get_slot` / `slot_snapshot` / `route_health` / `replace_route` / `route_pin` / `NoFileForDbnum` …）及其测试；HEAD 是另一套路由实现。末尾的 `stable_identity_regression` 测试断言「同长度同 mtime 不算同一文件」，而 HEAD 的 `FileIdentity` 就是 len+mtime，这条必红，一并丢 |
| `src/data_interface/direct_tree.rs` | +132/−73 | `DirectTreeService` 改走 `DirectStoreRegistry` + WIP 版 `MdbRouteDirectory`；HEAD 走 `MdbReadProvider` + `TreeMetadata` |
| `src/data_interface/element_attributes.rs` | +78/−52 | 同上（`open(project, mdb, registry, uda)`、`membership_fingerprint`、`load_uda_catalog`） |
| `src/data_interface/on_demand_model.rs` | +114/−12 | 单飞改用 `model_generation_flight` / `model_generation_version`，pending 先落再生成；HEAD 是 `generation_flight::Flights` + `generation_source_identity`，且 ADR-072 D5 已去掉 durable pending |
| `src/web_service/handlers.rs` | +187/−55 | `/health` 加 `routing` 段、`/dbnums` 走 `service.route_directory()`、`version` 改 `env!`；HEAD 已有 `build_info::VERSION/BUILD_ID`、`service.metadata()`；内嵌页 `window.location.origin` 计数断言 HEAD 已改成按 URL 断言 |
| `src/web_service/mod.rs` | +36/−24 | `with_registry` 构造、`bind_with_handoff` + 500 ms 维护循环、`ApiError` 只认 `NoSuchElement`；HEAD 三种「不存在」一并 404 |
| `src/lib.rs` | +7/−63 | 注掉启动路由目录横幅与 2 s 轮询（`/* Legacy whole-MDB skeleton refresh removed …`）、`serve_if_configured`；文件尾 700 行 `startup_room_build_gate_tests` HEAD 已删 |
| `src/data_interface/model_source.rs` | +21/−32 | HEAD 整文件重写（ADR-054 `RootDbError` 等） |
| `src/data_interface/model_refresh.rs` | +55/−6 | `generate_roots_report_with_policy(GeometryFailurePolicy)`；HEAD 是 `generate_roots_report_on_source(frozen source)` |
| `src/data_interface/increment_manager.rs` | +17/−9 | 调 `direct_store_registry::observe_directory_change`（WIP 独有） |
| `src/query_service.rs` | +18/−5 | `room()` 错误分类用 `UnresolvableRoot`；HEAD 已按 `room_membership()` 门 + `DirectStoreError` 链分类 |
| `src/data_interface/window_root_plan.rs` | +6/−18 | 新测试 `finalize_attempt_advances_the_planned_credentials_after_the_tail`（用 `increment_tail::finalize_attempt_on` / `credential_advance`，随 ADR-072 在途语义）；两条源码守卫改写 |
| `src/data_interface/pe_batch.rs` | +5/−10 | 源码守卫切片方式改写；HEAD 的更精确 |
| `src/data_interface/on_demand_db.rs` / `src/bin/direct_attmap_probe.rs` / `src/data_interface/db_file_header.rs` / `src/data_interface/read_through.rs` | 各 1–3 行 | 分别是 `NoFileForDbnum` 匹配、`SessionPage.machine` 字段名、`mdb_memory_store` 取 UDA catalog——都是 WIP 那套的接线 |
| `src/data_interface/mod.rs` | +7 | 6 条 `mod` 声明：`uda_catalog_provider` / `room_read_through` / `mdb_route_directory` HEAD 已声明（会重复），`direct_store_registry` / `model_generation_flight` / `model_generation_version` 指向 WIP 独有模块 |
| `src/fast_model/room_resident.rs` | +36 | 新测试与 HEAD 已有的 `ams_8000_models_and_rooms_without_database` 同名，且 `calculator.calculate(dbnum, &version, …)` 三参签名在 HEAD 已退（ADR-073 D2），编不过 |
| `src/data_interface/frozen_source.rs` | +2/−2 | `StableFileId` 放宽可见性，只为 direct_store 那条被丢的 `FileIdentity` 改法服务 |
| `src/data_interface/model_update_pending.rs` | +5/−16 | 文件本身已被 HEAD 删除（`0025edaa4`，ADR-072 D4/D5）；WIP 只改了它的测试 |
| `python/testbed/`（9 个文件） | 全删 | WIP 删整个目录，refactor 刚在 `71962d5cf` 更新过其中 2 个；保留 HEAD 的整个目录（半删的目录比哪一边都差）。要删请在主线上另起一笔 |

## 未跟踪文件

- 26 个里 **23 个原样还回工作树**（仍未跟踪）：`docs/plans/2026-09-12-review-backend-port-plan.md`、
  `verification/20260912-current-change-review.md`、`verification/dbslot-*-step/`、`verification/e3d-version-20260911/`、
  `verification/mbd-live-20260912/`、`verification/oracle-current-review/`、`verification/slot-refresh-baseline/`、
  `verification/watcher-route-step/`，以及下面三个孤儿。
- **3 个 HEAD 里已有同名文件、没有覆盖**（用的是 HEAD 的）：

  | 文件 | WIP 版 | HEAD 版 |
  |---|---|---|
  | `src/data_interface/mdb_route_directory.rs` | 129 行 | 1469 行 |
  | `src/data_interface/room_read_through.rs` | 94 行 | 116 行 |
  | `src/data_interface/uda_catalog_provider.rs` | 104 行 | 263 行 |

- **3 个孤儿**（还在 `src/data_interface/` 下、未跟踪、`mod.rs` 没声明、不参与编译）：
  `direct_store_registry.rs`（161 行）、`model_generation_flight.rs`（158 行）、`model_generation_version.rs`（119 行）。
  它们建在 WIP 版 `DirectStore` API 上，HEAD 没有那些方法；留在树里只是为了不丢文件，
  **要接着做请从 wip 分支起，不要直接把它们 `mod` 进来**。也可以直接删——两个 wip 分支上都有。

## 验证

- `cargo check --lib --tests`：通过（唯一告警是 vendor 里旧的 `instantiate_cylinder` dead_code）。
- `cargo test --lib -- e3d_model_service historical_model model_memory_store e3d_mesh_store surreal_retry model_coverage`：
  **90 passed / 0 failed / 8 ignored**——含保留下来的 5 条 WIP 单测，以及历史模型那组
  （`classify_required_root_error` 插进 Required 失败链后，`REFNO_NOT_FOUND_AT_SESSION` 的 downcast 不受影响）。
- 主工作树无 merge / squash 残留、无冲突标记；`git status`：8 个未暂存修改 + 上述未跟踪文件。
