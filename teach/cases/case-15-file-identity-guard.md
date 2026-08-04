# 案例 15 · 自动 watcher 的文件身份守卫：重复 dbnum / 回退 / 路径迁移

<sub>族 D 水位与重放 · Medium · 已修 · 证据层 B（12 个纯函数单测）+ C（真实双文件目录扫描）</sub>

## 一句话

文件异常检测只接在手动路径上，自动 watcher 只会做「文件最新会话 ≤ 水位就跳过」——
同一个 dbnum 放两份文件、或者换成更旧的备份，它照单全收。

## 现象

`check_file_against_state` / `FileAnomaly`（`Rollback` / `Duplicate` / `Missing` / `PathMigrated`）
与 `record_scan` 只在 `manual_update.rs` 里被调用。自动路径的 `SesnoRangeResolver` **从不 record_scan**，
于是：

- `dbnum_watermark` 的文件身份字段（`file_name` / `file_path` / `file_size` / `file_modified_at`）常年为空；
- 同一个 dbnum 出现两份文件时**没有守卫**，两份都会被处理；
- 换成更旧会话的文件时**没有告警**，静默按旧数据走。

## 证据

- 缺陷登记：[`../../docs/specs/incr-gen-fixes/spec.md`](../../docs/specs/incr-gen-fixes/spec.md) **F6（Medium）**。
- 约束来自 [`ADR-001`](../../docs/adr/ADR-001-dbnum-update-state.md)：文件观察字段必须通过**独立写入**更新，
  该写入失败**不能修改或间接影响** `applied_sesno`。这是「扫描」与「应用」两条语义必须分开的根据。
- round2 审核顺带查出两处次要问题（Low），见下。

## 修法

F6 的四条需求（[`tasks.md`](../../docs/specs/incr-gen-fixes/tasks.md) T601–T605）：

1. 自动 watcher 扫描每个候选文件时 MUST `record_scan` 更新文件观察字段——**不触碰 `applied_sesno`**；
2. 自动路径 MUST 复用 `check_file_against_state`：
   `Rollback` / `Duplicate` → 阻断该 dbnum 并告警（不推 / 不回退水位）；
   `PathMigrated`（同项目同类型、水位不回退）→ 自动更新路径；
3. 异常 MUST **只隔离所属 dbnum**，不阻断其它正常批次（与手动路径同口径）；
4. `async_watch` 与 `init_watcher` 两条路径同款接入。

实现要点在 [`../../src/data_interface/increment_manager.rs`](../../src/data_interface/increment_manager.rs)
新增的 `scan_and_check_file` 助手（`:468` 起的 doc 注释写明了「只写观察字段、绝不推进 applied_sesno」）。

2026-07-26 还补了一个跨事件缺口：async watcher 原本只看**当前事件批次**，
现在每次处理前**重扫全部非递归监控文件**并阻断跨事件重复 dbnum。

## 验证

- 判定口径本身有 **12 个纯函数单测**覆盖。
- 实库 `live_record_scan_never_moves_the_applied_watermark`：建立水位 50 后，
  先扫到「路径已变 + 更新会话 60」再扫到「更旧会话 10」，断言两次都只更新身份 / 观察字段、
  `applied_sesno` **恒为 50**，并对拍纯判定给出 `Rollback`。
- `live_watch_directory_blocks_duplicate_dbnum_files`：从真实 E3D 文件复制两个相同 60-byte 头到
  临时监控目录，确认目录扫描**阻断该 dbnum**。

## 顺带记录的三条（Low，部分未修）

**B3 · `record_scan` 早于重复判定**。`scan_and_check_file` 无条件写观察字段，而重复判定在其后。
所以两个重复文件都会写库，后扫到的那个把 `file_path` 覆盖成自己——尽管这个 dbnum 随后就被阻断了。
由于 `init_watcher` 按**文件大小降序**遍历，最终留在库里的身份取决于文件大小，且每轮可能翻转。
`applied_sesno` 不受影响（ADR-001 不变量由 `record_scan` 保证，已被 T605 覆盖），
所以只是状态记录被污染。同一位置还有个更轻的：`#[cfg(feature = "mqtt")]` 的
`SyncPublisher::ensure_archive` 对第一个文件已经执行，为一个随后被阻断的 dbnum 留下无用存档。

**B4 · init 递归扫描，watch 兜底只查一层**。`init_watcher` 用不限深度的 `WalkDir`，
而 `async_watch` 的跨事件重复兜底用 `max_depth(1)`。好在监控注册本身就是 `RecursiveMode::NonRecursive`，
事件只会来自目录直属文件，所以**不构成漏判**；真正的后果是两条路径的候选集合不一致——
子目录里的库文件启动时会被处理，之后却永远收不到变更事件。建议二选一：init 也限深，
或在文档里写明「只有直属文件参与增量」。

**T903 · 容量 1 的事件通道**（已评估，维持现状）。`async_watch` 用容量 1 的 mpsc 接 `PollWatcher`，
消费侧单循环串行处理。**不丢事件**有三层机制，任何一层单独成立都够：

1. 通道满时是**阻塞不是丢弃**（`Sender::send` 等待）；
2. `PollWatcher` 是**快照差分不是事件流**——回调被阻塞期间轮询停摆，恢复后仍与停摆前那份快照比对，
   期间所有改动**合并成一次事件**上报；
3. 真正的工作量由**水位**而非事件决定——事件只是「去看一眼」的触发器，增量窗口由
   `SesnoRangeResolver` 按 `applied_sesno` 重新算。

所以串行处理的后果是**延迟**而非丢失：有效轮询间隔从 30 s 退化为「30 s + 上一轮处理耗时」。

## 规律

**同一套安全判定必须挂在所有入口上，而不只是「人会看着的那个入口」。** 手动路径有人盯着，
出了问题当场就发现；自动路径无人值守，恰恰更需要守卫。缺陷登记里那句「自动 / 手动在文件身份与
异常处理上语义一致」，本身就是一条应该被测试钉住的不变量。

**「观察」与「应用」必须是两条独立的写。** 把 `file_latest_sesno` 和 `applied_sesno` 混在一次写里，
就等于让「我看到了一个新文件」自动变成「我已经处理完它了」。ADR-001 把这条拆开，
换来的是「预览能同时展示文件最新与已经应用」，以及回退 / 重复 / 迁移可以被统一检测。

## 关联

- [`spec.md F6`](../../docs/specs/incr-gen-fixes/spec.md) · [`ADR-001`](../../docs/adr/ADR-001-dbnum-update-state.md)
- [`../../docs/2026-07-26_increment-update-chain-audit-round2.md`](../../docs/2026-07-26_increment-update-chain-audit-round2.md) B3 / B4
- [`../../docs/2026-07-26_p3-t903-t904-assessment.md`](../../docs/2026-07-26_p3-t903-t904-assessment.md) T903
- 案例 [11 水位三段式](case-11-watermark-three-phase.md)
