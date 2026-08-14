# Implementation Plan: 增量窗口净收集

**Spec**: `spec.md`（同目录）　**ADR**: ADR-022　**Status**: P0 已落地（2026-08-13），灰度证据收口中

## Constitution Check

- **静默失效**：收集降级（基版本解析失败按新增、终稿不可解析跳过）全部计数 +
  警告随回执透出（`NetWindowOutcome::warnings` / `unparseable_finals` /
  `unchanged_rewrites`）；净口径首条警告自报口径。无 `_ =>` 放行。
- **单一权威**：口径开关只在 `IncrementPipeline::collect_window` 一处生效
  （预览/执行/崩溃恢复/worker 尾段，源码实测 5 个调用点，源码断言禁直调回放）；
  `diff_ele_data` 是 vendor 内联 diff 的复刻分支，一致性由性质 i 逐桶对拍钉住，
  vendor 提取纯函数共用是后续合并项（ADR-022 决策 2）。
- **水位纪律**：不动（窗口起点仍由水位给出；本特性纯收集侧）。
- **纯文件判定**：`net_window` / `session_index_diff` 双模块零 `SUL_DB` 源码断言。
- **注释不变量有测试**：见 tasks.md 各项挂的测试名。

## 阶段划分

1. **P0 工具层**（✅ 2026-08-13）：`session_index_diff` 双根差分 +
   `aios_db.parse.net_changes` + `net_changes_probe.py`；点查仲裁三条口径规则。
2. **P0 引擎接线**（✅ 2026-08-13 晚）：`net_window` 合成器 + `collect_window`
   派发 + 灰度开关 + 性质 i + live 负载对拍。默认 off。
3. **P1 灰度证据收口**（进行中，按 M1 / M2 里程碑推进，见
   [开发计划](../../docs/plans/2026-08-13-net-window-default-on-development-plan.md)）：
   **M1 正确性闭环**——T20 合成器纯单测 ✅、T11b 存量库删除等价 ✅、T19 qualifier
   恢复对拍 ✅（非阻断）、T18a release 方向性单点 ✅（n=1 非门）；**唯 T13 Added
   夹具 BLOCKED**（仓内无 Added>0 且 raw 两集相等的真实窗口，须受控 E3D
   `scratch-create` 录制），**M1 Exit gate 因此未通过**。
   **M2 运行闭环**——T17 批次口径冻结 / T12 会话页清单 / T18 正式性能门 / T15 翻默认，
   **Entry gate 是 M1 Exit 全绿，故不得启动**。
4. **P2 翻默认值**（ADR-022 验收 5）：**机制层已由 live IDA 闭合**（双根差分 /
   删除即集差非墓碑 / flag 不进变更检测链路（链路外语义未闭合）/ 哨兵，见
   `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`），翻默认余下
   受**结果层证据门**阻断——**已闭** T11b、T20；**未闭** T13（BLOCKED）+ T17 +
   T12 + T18（完整收集统计 + SYST 现场硬门）。过门 → `net_window_collection`
   默认 on 一个发布周期 → 拆开关与回放收集的执行路径接线（诊断入口保留）。

## 关键文件

- [src/data_interface/session_index_diff.rs](../../src/data_interface/session_index_diff.rs)：双根差分（`NetEntry::base_loc` 为合成器供两端位置）
- [src/data_interface/net_window.rs](../../src/data_interface/net_window.rs)：净操作合成 + `diff_ele_data`
- [src/data_interface/increment_pipeline.rs](../../src/data_interface/increment_pipeline.rs)：`collect_window` 唯一派发点
- [src/options.rs](../../src/options.rs)：`net_window_collection` + `AIOS_NET_WINDOW`
- [tests/db8000_session_pairs.rs](../../tests/db8000_session_pairs.rs)：性质 h/i
- 证据：`docs/evidence/2026-08-13-session-index-diff-net-changes.md`
