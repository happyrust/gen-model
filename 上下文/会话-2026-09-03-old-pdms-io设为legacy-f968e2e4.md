# 会话 f968e2e4 · 把 old-pdms-io 设为 legacy（默认不启用、不编译）

> 用户需求（2026-09-03 16:17）：「项目路径设为 ../gen-model。我想将 old-pdms-io 设为 legacy，默认不启用，不编译。
> 数据解析都是使用 e3d-io，模型生成使用 e3d-model。」
>
> 工作区 `D:\work\plant-code\old\plant-11`，目标仓 `D:\work\plant-code\old\gen-model`（分支 `e3d-direct`，HEAD `9dee46df`，
> 工作树另有大量未提交改动；同一主树上今天还有 T401 影子收集器 / model_type_scope 两个会话在改）。
> 本会话只做小步精确替换，不整文件重写，不提交。本文实时更新。

## 1. 现状核对（只读，16:20–16:40）

- `Cargo.toml:159` `pdms_io` 是 git+rev **非 optional** 依赖；`legacy_pdms_io = []` 是空门（`src/` 零消费者）；
  `debug_parse` / `legacy_session_replay` 两个 feature 直接转发 `pdms_io/*`。
- `rg pdms_io src/` = 31 个文件、约 70 处（与 `docs/plans/2026-09-03-old-pdms-io-retirement-inventory.md` §2 的 A–H 八组一致）。
- 数据面类型 `EleOperationData / EleOperationDetail / ModifiedElement` 定义在 old-pdms-io `io.rs`，底层是
  `parse_pdms_db::parse::EleData`（`WholeAttMap` = attmap + explicit_attmap + uda_atts）——**parse_pdms_db 留下**（清单阶段 8，本会话不动）。
- `to_surql / to_modify_surql / to_patch_json / diff_ele_data / classify_children_delta / changed_qualifiers` 等渲染与差分函数
  也住在 old-pdms-io `io.rs`（约 500 行纯函数），gen-model 的 `render_persist_statements` 直接调它们。
- `shadow_collector.rs`（T401）只做对拍记账，**不产出** `EleOperationData`；e3d 侧产物是 `IndexDiff` 候选 + `diff_element` L2。
- `direct_attmap::to_named_attmap(ElementExtraction) -> DirectAttrs{map, outside_schema, unset, shape_conflicts, …}` 已就位。
- 快照身份门 `DabaconSnapshot::open_verified*` / `SnapshotToken`：`direct_store.rs` 有 `FileIdentity` 替身但没接线。
- `PdmsWatcher`（152 行）：唯一实现在 old-pdms-io；e3d-io 无 watcher。用户已拍板「在 e3d-io 侧新写一个」。
- `sync::clone / sync::compress`：文件分发功能，与解析无关，无替身。

## 2. 方案（本会话执行）

目标判据（清单 §0 三条）：① `pdms_io` optional、只由 `legacy_pdms_io` 打开；② 默认 features 依赖图无 `pdms_io`；③ 源码护栏。

1. **本地类型模块** `data_interface/ele_ops.rs`：把 `EleOperationData` 家族与渲染/差分函数从 old-pdms-io 原样搬进 gen-model
   （保持 SurrealQL 字节级一致），全部消费者改 `use crate::data_interface::ele_ops::*`。
2. **e3d 收集器** `data_interface/e3d_collector.rs`：`IndexDiff(base@S, target@T)` + 两端 `DbElement` → `to_named_attmap` → `EleData`
   → `diff_ele_data` 合成 `Add / Deleted / Modified`，输出与旧收集器同形状的 `CollectedWindow`；`shape_conflicts` 非空阻断窗口；
   `TypeChanged` 渲染成 `Deleted(old noun ATT 行)` + `Add`。
3. **身份门**：`SnapshotToken` → 本地 `FrozenSource`（path + FileIdentity(len/mtime/file_id) + dbnum/db_type/target_sesno），
   `open_verified*` → 复核函数。
4. **文件头**：`DbPageBasicInfo` / `PdmsIO::new(...).get_page_basic_info()` → e3d-io `meta` / `session`（本地 `DbFileHeader`）。
5. **watcher**：e3d-io 新增 `watch` feature（`FileObservation`，notify + walkdir），gen-model 侧 `DbFileWatcher` 负责读头缓存。
6. **clone / compress**：搬进 gen-model `data_interface/file_sync/`（依赖 dpcsync/reqwest 已在图里）或先门后。
7. **门**：探针 / 夹具 / 测试挪到 `#[cfg(feature = "legacy_pdms_io")]`、bin 加 `required-features`；`pdms_io` 改 optional；加源码护栏测试。

## 3. 进度

- 16:40 核对完成，开始改码。
- 16:41–16:47（会话 f968e2e4，随后断线）已落地的新文件（均未提交）：
  - `src/data_interface/ele_ops.rs`：old-pdms-io `io.rs` 第 28–1399 行原样搬入（`EleOperationData` 家族 + 渲染/差分纯函数 + 测试）。
  - `src/data_interface/db_file_header.rs`：`DbFileHeader` / `SessionStamp` / `latest_sesno` / `session_at` / `session_chain` / `session_sesnos_in_range`（e3d-io `ReadOnlyEngine` + `DbView`）。
  - `src/data_interface/frozen_source.rs`：`FrozenSource`（`capture` / `capture_at` / `reopen_verified` / `reopen_verified_at` / `verify_path_identity` / `session_chain` / `open_engine_at_target`），Windows 用 `volume_serial_number + file_index`（需 nightly `windows_by_handle`）。
  - `src/data_interface/e3d_collector.rs`：`collect_net_window(frozen, range, db_type) -> NetWindowOutcome{frozen, window, warnings, unchanged_rewrites, unparseable_finals, ignored_finals, retired_type_rows, stats}`、`contains_refnos_at`、`resolve_window_anchors`、`PageSessions`。
  - `src/data_interface/db_file_registry.rs`：`DbFileRegistry`（`PdmsWatcher` 的状态壳：`watch_dirs` / `headers` / `file_name_full_path_map` / `get_dbno` / `save`）。
    **对 §1「watcher 在 e3d-io 侧新写一个」的事实修正**：`rg PdmsWatcher src/` 核对后，`init_local_watcher` / `async_watcher` / `scan_db_headers` 在本仓一处都没被调用，真正盯目录的是 `increment_manager::async_watch` 自己的 `notify::PollWatcher`。所以 e3d-io 侧无事可写，也没有事件序列可对拍。
  - `src/data_interface/file_sync/{mod,utils,clone,compress}.rs`：old-pdms-io `sync/clone.rs` / `sync/compress.rs` 原样搬入 + `human_size`。
  - `Cargo.toml`：`pdms_io` 改 `optional = true`；`legacy_pdms_io = ["dep:pdms_io"]`；`debug_parse` / `legacy_session_replay` 叠在其上；新增 `dpcsync` / `blake2` / `reqwest` / `url` / `futures-util`。
  - `src/data_interface/mod.rs` 挂上 5 个新模块；`direct_attmap.rs` 暴露 `natural_value`。
  - `increment_pipeline.rs` 改了一半：头部 import、`CollectedWindow{frozen, retired_type_rows}`、`net_caliber_warning` 吃 `e3d_collector::NetWindowOutcome`、`verify_frozen_source` 换 `FrozenSource`；**`reconcile_plan_final_presence` 体内还引用不存在的 `snapshot`，`collect_window_for_candidate` / `apply*` / `apply_one` 仍是 pdms_io 形状**。
- 16:47 会话断线，由会话 c930d44b 接手（本文继续实时更新）。
- 16:52 接手核对：`cargo check --lib` 先在 **vendor/e3d-io** 上失败（`session/mod.rs` 新增 `delta` / `diff` 模块、`engine.rs` 半成品）——那是另一会话（`docs/plans/2026-09-03-sesno-range-increment.md`）正在改的现场，文件 mtime 16:50，**不动它**，本会话只改 gen-model，编译验证等它稳定后再跑。
- 16:55–17:40 接线完成（均未提交）：
  - `e3d_collector.rs`：全量扫描分支改走 `DbView` + `IndexCursor`（`ReadOnlyEngine` 没有 `stats()`）。
  - `increment_pipeline.rs`：`collect_window_for_candidate(path, range, Option<&FrozenSource>)`（去掉 `project` 参数）→ `FrozenSource::capture_at` / `reopen_verified_at` → `e3d_collector::collect_net_window(&frozen, range, frozen.db_type())`；`apply*` 签名 `DbPageBasicInfo → DbFileHeader`；`apply_one` 里 `frozen` 复核（`verify_frozen_source` → `reopen_verified`）、`reconcile_plan_final_presence` 走 `e3d_collector::contains_refnos_at`、尾事务前 `frozen.reopen_verified()`、`retired_type_rows` 渲染成 `DELETE {old_noun}:{id};` 并入 `window_statements`（新 `retired_type_row_statements`）；源码钉测试同步改字面量；legacy 回放臂 `collect_changes_from_open_io` 经 `ele_ops::legacy::from_legacy_window` 换类型。
  - `ele_ops.rs` 末尾新增 `#[cfg(feature = "legacy_pdms_io")] pub mod legacy`：`From<pdms_io::io::{ModifiedElement,EleOperationDetail,EleOperationData}>` + `from_legacy_window`。
  - `frozen_source.rs`：新增 `#[cfg(test)] synthetic(...)`（单测拼载体）与 `read_frozen_bytes()`（基线全量读，回填头样本）。
  - `db_file_header.rs`：`SessionStamp` 加 `previous_pgno`；新增 `legacy_session_ranges(chain)`（逐字复刻旧 `ses_range_map`，含链头空区间的怪相）。
  - `sesno_range.rs`：`SesnoUpdatePlan.basic_info: DbFileHeader`；`build_plan(chain, ...)`，`nearest_large_sesno` / `sessions_in_window` 在会话链上算；源码钉改字面量。
  - `increment_manager.rs`：`PdmsWatcher → DbFileRegistry`，`PdmsIO::new(..).get_latest_sesno()` → `db_file_header::latest_sesno`。
  - `db_model.rs` / `tidb_manager.rs`：`Arc<DbFileRegistry>`；clone 走 `file_sync::clone`。`sync_publisher.rs`：compress 走 `file_sync::compress`。
  - `manual_update.rs`：`FileCandidate.frozen: Option<FrozenSource>`；`session_time_rfc3339` → `db_file_header::session_at`；`window_times_rfc3339` / `fill_batch_session_times` → 一次 `session_chain` 查表（新 `session_times_lookup`）；`preview.file_latest_sesno_time = plan.basic_info.latest_session.rfc3339()`。
  - `batch_worker.rs`：`refresh_candidate` 用 `FrozenSource::capture`。
  - `window_repair.rs`：ADR-036 成员扩枝退役，审计只剩「持久层孤儿 ∧ 冻结目标索引里不存在 → Deleted」（`contains_refnos_at`）；`retired_type_rows` 一并删旧 noun 行。
  - `versioned_db/database.rs`：基线全量解析 `FrozenSource::capture` + `read_frozen_bytes` + `parse_pdms_db::parse_db_basic_data`（新 `read_full_basic_data`），`ses_range_map` 用 `legacy_session_ranges`。
  - 类型导入改 `ele_ops`：`model_update_plan` / `model_impact` / `shadow_collector`（头注释补「底座切换后的读法」）/ `staging/{lifecycle,write_context,parity}` / `issue10_direct_add_node` / `direct_window_replay_parity` / `room_fixture` / `room_live_issue7`；`historical_model::session_times` → `session_chain`。
  - bins：`increment_planner_parity`（`--probe` 参照臂进 `legacy_pdms_io` 门）、`manual_scan_probe`、`db8000_two_delete_fixture`（noun 经 `ReadOnlyEngine::find_element` + `db1_dehash`）、`l3_suite/fixture`、`incr_fold_probe` 改端口；`legacy_v2_read_parity` 加 `[[bin]] required-features = ["legacy_pdms_io"]`。
  - tests：`issue7_e2e_increment` 改 `latest_sesno`；`db8000_two_delete_fixture` 改类型；`db8000_session_pairs` 性质 h / i 两条 + 归一化 helper 进 `legacy_pdms_io` 门（性质 i 经 legacy 桥换类型），冻结快照测试改 `FrozenSource`。
  - `python/`：`convert.rs` 改 `ele_ops` 类型；`exec_api.rs` 两处头读改 `DbFileHeader::read` / `latest_sesno`；`lib.rs::net_window` 改 `FrozenSource::capture_at` + `e3d_collector::collect_net_window`（aios-py 自身仍直接依赖 pdms_io，是显式 opt-in 的 legacy 消费者）。
  - `lib.rs`：`#![cfg_attr(windows, feature(windows_by_handle))]`。
  - 护栏：`data_interface/mod.rs` 新增 `legacy_pdms_io_guard` 源码测试（`pdms_io::` 代码行所在文件必须带 legacy 门或在 Cargo.toml 的 legacy `required-features` bin 清单里）；`scripts/Check-LegacyPdmsIo.ps1`（`cargo tree` 两条 + 文件级扫描）。
- 17:40 起 `cargo check --lib` 验证（受 vendor/e3d-io 另一会话改动影响，见下）。
