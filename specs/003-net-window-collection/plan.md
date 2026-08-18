# Implementation Plan: 增量窗口净收集

**Spec**: `specs/003-net-window-collection/spec.md`　**ADR**: ADR-009 + ADR-022 + [ADR-031](../../docs/adr/ADR-031-single-net-window-collection-caliber.md)　**Status**: P5 Completed（2026-08-19）

## Constitution Check

- **静默失效**：不可读子页、层级不下降与终稿不可解析按生产点查可达性跳过并
  分别计数；索引根页不可读与 last-touch 缺失整窗失败；基版本解析失败保守按新增
  并警告，重复/越界残留按已验证点查口径计数。净口径首条警告自报口径与全部容忍
  计数。无 `_ =>` 放行。
- **单一权威**：收集只有 `IncrementPipeline::collect_window` 一个入口
  （预览/执行/崩溃恢复/worker 尾段，源码实测 5 个调用点）；ADR-031 之后它**没有
  口径分支**。逐会话实体回放由默认关闭的 `legacy_session_replay` feature 隔离，
  正常生产依赖图不编译回放 API；无 feature 的 compile-fail 与生产 check 是可达性门。
  `diff_ele_data` 已提取到 vendor 并由净窗口/回放共用（T14）；primaryList 只认
  core.dll 同一字段读取链冻结的快照，未知项单独列账且保守为真（T27）。
- **水位纪律**：不动（窗口起点仍由水位给出；本特性纯收集侧）。
- **回执完整性**：`CollectedWindow` 把操作流与实际会话页清单一起冻结；空保存、
  自抵消、稀疏会话不再因没有 op key 从回执消失，首条 warning 自报口径与容忍计数。
- **纯文件判定**：`net_window` / `session_index_diff` 双模块零 `SUL_DB` 源码断言。
- **注释不变量有测试**：见 `specs/003-net-window-collection/tasks.md` 各项挂的测试名。

## 阶段划分

1. **P0 工具层**（✅ 2026-08-13）：`session_index_diff` 双根差分 +
   `aios_db.parse.net_changes` + `python/testbed/net_changes_probe.py`；点查仲裁三条口径规则。
2. **P0 引擎接线**（✅ 2026-08-13 晚）：`net_window` 合成器 + `collect_window`
   派发 + 灰度开关 + 性质 i + live 负载对拍。默认 off。
3. **P1 灰度证据收口**（历史阶段，已由 ADR-031 收束）：T20 合成器纯单测 ✅、
   T11b 存量库删除等价 ✅、T12 会话页清单 ✅、T19 qualifier 恢复对拍 ✅（非阻断）、
   T18a release 方向性单点 ✅（n=1 非门）；T13 Added 夹具仍 BLOCKED（仓内无
   Added>0 且 raw 两集相等的真实窗口，须受控 E3D `scratch-create` 录制）。
4. **P2 翻默认值**（**已取消**）：该阶段的产物是「默认值从回放翻到净窗口」，
   被 P3 的一次性单路径切换取代。机制层已由 live IDA 闭合（双根差分 / 删除即集差
   非墓碑 / flag 不进变更检测链路（链路外语义未闭合）/ 哨兵，见
   `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`）。
5. **P3 单路径切换**（[ADR-031](../../docs/adr/ADR-031-single-net-window-collection-caliber.md)，
   2026-08-18）：`collect_window` 去口径分支；`net_window_collection` /
   `AIOS_NET_WINDOW` / `CollectionMode` 退役（残留配置键出显式告警）；回放降级为
   legacy 诊断入口；当时的 body-scoped 字符串护栏已由 P5 编译边界取代；T17 CANCELLED
   （无开关即无口径可冻）；T13 / T18 按 ADR-031「门的重定级」处置。
6. **P4 判据层收口**（2026-08-18）：T14 将逐字段元素 diff 收敛到 vendor 单一实现；
   T27 从 live E3D 3.1 直接调用 core.dll 字段读取链冻结 primaryList 快照，已解析
   false 真正关闭成员事件，unknown 保守为真。两项均不改变净窗口三态与公开 DTO。
7. **P5 跨仓编译隔离**（2026-08-19）：`old-pdms-io` 与主仓以默认关闭的
   `legacy_session_replay` feature 隔离全部逐会话实体回放入口；Python、探针和 oracle
   显式启用。生产构建以类型缺席取代源码字符串禁调。`dpcsync` 检入 prost 生成物并
   删除 dpcsync 的构建脚本与 prost-build 构建依赖，跨仓构建不再要求宿主安装 protoc。

8. **P6 收集器下沉**（2026-08-19）：`session_index_diff` 与 `net_window` 整体迁入
   pdms-io，与它们替代的 legacy 逐会话回放同层；`walk_tree` 与
   `btree_search_optimized_recursive` 的路由复刻不再跨 crate 边界。上层只留批次口径
   （`collect_window`）与三条需要回放参照臂的 live 对拍。纯平移，行为零改动，验收面
   是性质 h/i。

## 关键文件

- **pdms-io** `src/session_index_diff.rs`：双根差分（`NetEntry::base_loc` 为合成器供两端位置）
- **pdms-io** `src/net_window.rs`：净操作合成 + `diff_ele_data`
- [src/data_interface/increment_pipeline.rs](../../src/data_interface/increment_pipeline.rs)：`collect_window` 唯一收集入口；`collect_changes` legacy 诊断入口；三条跨结构 live 对拍
- [src/options.rs](../../src/options.rs)：`net_window_collection` 退役探测（残留键告警）
- [src/data_interface/model_impact.rs](../../src/data_interface/model_impact.rs)：primaryList 快照门控（ADR-009）
- [tests/fixtures/core-primary-list-e3d31.json](../../tests/fixtures/core-primary-list-e3d31.json)：core.dll 字段快照与 unknown 清单
- [tests/db8000_session_pairs.rs](../../tests/db8000_session_pairs.rs)：性质 h/i（切换后唯一的跨结构交叉验证，直接调收集器、不经 `collect_window`）
- 证据：`docs/evidence/2026-08-13-session-index-diff-net-changes.md`、`docs/evidence/2026-08-18-single-caliber-net-window.md`
