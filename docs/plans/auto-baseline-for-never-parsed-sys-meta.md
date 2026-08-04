# 从未解析过的 SYS meta 也自动走全量基线

日期：2026-07-27
状态：**已实施**（手动与自动两条路径都已对齐）
关联：ADR-006（跨块 CURD 解析修复）、ADR-007（SYS 解析不受 included_db_files 限制）

## 现状

「从来没解析过就自动解析」这个检查已经有了一半：

```rust
// src/data_interface/manual_update.rs
fn needs_initial_load(applied_sesno: i32, file_latest_sesno: i32, db_type: &str) -> bool {
    applied_sesno == 0 && file_latest_sesno > 0 && !COLD_START_DB_TYPES.contains(&db_type)
}
```

前两项就是「从未解析过」的判据。命中后预览标 `initialization_required`、执行时走
`initialize_dbnum_baseline` 自动补一次全量。**DESI / CATA 早就是自动的。**

第三项 `!COLD_START_DB_TYPES.contains(&db_type)` 把 `SYST / DICT / GLB / GLOB` 显式排除了。
它们改走 `SesnoRangeResolver::allows_cold_start`——水位缺失时从 0 开始，用普通增量窗口把历史
会话重放一遍。

## 为什么要改

两条路**用的不是同一个解析器**：

| | 解析器 | 是否带 ADR-006 的跨块修复 |
|---|---|---|
| `initialize_dbnum_baseline` → `sync_total_async_threaded` | `parse_pdms_db`（`vendor/aios-parse-pdms`） | **是** |
| cold start 重放 → `PdmsIO::collect_increment_eles` | `pdms_io`（`../pdms-io`） | 从未移植 |

ADR-006 修的是 `vendor/aios-parse-pdms/src/parse.rs` 的 `collect_explict_data`：跨多个记录块
存储的长引用列表（`CURD` / `DBLS`）原先遇不匹配块直接 `break`，后续块被丢弃。而 SYS meta 里
设计 MDB 的 `CURD` 正是这类属性——它决定模型树能不能解析到设计库。

在 `old/pdms-io` 里搜 `collect_explict_data` / `resync` / `MAX_RESYNC` 一个都没有，它是另一套
按会话差分的实现。不能据此断言它解不对 CURD，但可以断言：**那个修复从没走过这条路，而一个
从未解析过的 SYS 库恰恰只会走这条路。**

现网佐证：8009 上 `NAME == "/ALL"` 只有一条合成记录 `MDB:1`，CURD 仅一项、指向单行表
`db_desc`，解出来 `$dbnos = [8000]`；模型树因此只显示 dbnum 8000 下三个夹具 SITE
（`/SITE-PIPING` 等），而真实的 15.7 万个元素、974 个几何实例在 7997 上，到不了树。

## 改动

### 手动路径（`manual_update.rs`）

判据不再按 db_type 分叉，`db_type` 参数整个拆掉——留一个不影响结果的参数在那里，下一个人
迟早要回来查它到底管不管用：

```rust
fn needs_initial_load(applied_sesno: i32, file_latest_sesno: i32) -> bool {
    applied_sesno == 0 && file_latest_sesno > 0
}
```

`COLD_START_DB_TYPES` 在本文件就此成为死引入，一并删除。测试里原有的
`assert!(!needs_initial_load(0, 76, "SYST"))` 断言的正是旧行为，改成不分类型的四条断言，
并补上 `assert!(!needs_initial_load(0, 0))`——**空文件不是「没解析过」**，没有会话可解析，
派一次基线纯属白跑。

### 自动路径（`increment_manager.rs`）

`initialize_dbnum_baseline` 原先吃 `&FileCandidate`（`manual_update.rs` 的私有结构），而
watcher 那侧手里没有这个结构。改成吃四个标量 `(dbnum, file_name, db_type, file_latest_sesno)`
并提到 `pub(crate)`——它本来也只用到这四个字段。

新增 `AiosDBManager::baseline_if_never_parsed`，在 `init_watcher` 与 `async_watch` 两处
**`SesnoRangeResolver` 之前**调用，位置与手动路径里 `needs_initial_load` 的位置对应。
返回 `true` 表示已由基线接管，调用方 `continue` 跳过增量窗口。基线失败也返回 `true`：
水位没推进，下一轮自然重来，但不该退回增量窗口去猜历史——那正是这次要消除的分叉。

## 边界与副作用

1. **自动路径连 DESI/CATA 的行为也变了。** 手动侧只是把 SYS 从排除项里放出来；而自动侧原先在
   水位为 0 时对 DESI/CATA 是**直接跳过、什么都不做**（"unsafe to guess history"）。现在它们
   也会补基线。这是对齐的应有之义——同一个从未解析过的设计库，点预览会补、让 watcher 发现
   却永远不补，本身就说不通——但它确实扩大了自动模式的动作面，值得单独盯一轮。
2. **SYS 基线天然是项目级的。** `baseline_sync_options` 会设 `included_db_files = [file_name]`，
   但按 ADR-007，SYS 解析不受它约束、会遍历项目全部文件再按 db_type 筛。所以传 `SYST` 实际会
   解析该项目下**所有** SYST 文件，不止这一个。一般项目只有一个 `amssys`，影响不大，但这条得
   写进代码注释，免得下一个人以为是单文件操作。
3. **cold start 不会失效**，只是让位。`execute_one_dbnum` 里 `needs_initial_load` 在
   `SesnoRangeResolver` 之前，所以全新的 SYS 库走基线；`allows_cold_start` 之后主要服务
   「水位记录被人删掉、但数据还在」这种场景。
4. **已有水位的库完全不受影响**，`applied_sesno == 0` 挡着。8009 上 8191 是 169/169，改完照旧
   走增量。

## 验收

```sql
-- 改动前后都应为 0 变化：8191 已有水位，不该被判为需初始化
select dbnum, applied_sesno, file_latest_sesno from dbnum_watermark where dbnum = 8191;
```

真要看到新行为，把 `dbnum_watermark:8191` 删掉再点预览：那一行应显示
`initialization_required = true`（而不是像现在这样安静地走增量重放），执行后
MDB 行数与 `/ALL` 的 CURD 长度应按 ADR-007 的记录涨上去（当时验证是 51 行 / CURD 71 项）。

对照 `docs/runbook-sys-reparse-for-model-tree.md` 的三条验收 SQL 一起看。
