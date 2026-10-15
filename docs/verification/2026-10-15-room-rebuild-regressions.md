# 房间重建：修复前的隔离回归测试

日期：2026-10-15。基线 HEAD：`700e22c01`，叠加用户原有未提交修改。

## 范围与安全边界

仅新增测试及不改变行为的测试入口提取，不修复业务逻辑。

- `handlers.rs`：原清边 SQL 提取到 `clear_room_rebuild_edges_on(db)`，生产仍传 `SUL_DB`。
- `room_resident.rs`：原准入判断提取到 `try_begin_rebuild_in(lifecycle, reset)`，生产仍使用原全局槽与回调。
- 测试不调用 HTTP 重建端点、不初始化 `SUL_DB`、不加载项目配置、不读取 E3D 源模型，也不修改进程级镜像。
- 唯一数据库写入对象是测试新建的 `mem://` 嵌入式连接；无 TCP 监听、无外部数据库。

## 运行

```text
cargo test --offline --lib room_rebuild_regression_ -- --nocapture --test-threads=1
```

## 实测结果：编译成功，三条预期断言失败

退出码 101，`0 passed; 3 failed; 0 ignored`。这些是未修复缺陷的红测试，不是编译失败；未使用 `should_panic` 或 `ignore` 隐藏失败。普通完整测试套件也会包含它们，在对应修复前保持红色。

1. `room_rebuild_regression_same_generation_failure_keeps_alignment_owed`
   - 全新内存库里建立两类关系边及代号 g1 的水位。
   - 控制断言确认清边前有边且已对齐。
   - 执行生产清边 helper；控制断言确认两张关系表确实已空。
   - 在清边后注入重算失败边界：不发布替代边、不写成功章，源输入仍是 g1。
   - 正确行为应继续欠重算，实测 `alignment=Some("g1")`，电平型判据仍认为已对齐，故断言失败。
   - 这是清边/恢复凭证的边界行为测试，不是完整 HTTP 或后台任务端到端测试。
2. `room_rebuild_regression_second_request_rejected_during_edge_recompute`
   - 对局部生命周期槽运行真实生产准入 helper。
   - 控制断言确认 Loading 期间第二次请求被拒。
   - 按当前后台流程，把生命周期切为 Ready、阶段设为 edge-recompute。
   - 实测第二次请求获准，故断言失败。
   - 此测试验证状态准入漏洞，不是双 HTTP 请求/数据库写入交错的并发压力测试。
3. `room_rebuild_regression_success_stamp_has_mirror_invalidation_contract`
   - 对当前生产水位写入函数的源代码做限定范围检查。
   - 直接 UPDATE 之后缺少 DbnumState 镜像失效调用，故断言失败。
   - 这是源码契约守卫，不是缓存行为复现；未来若把写入封装进统一入口，需要同时更新此守卫。

## 测试文件

- `src/web_service/room_rebuild_regression_tests.rs`
- `src/fast_model/room_rebuild_regression_tests.rs`

首次运行时测试计数夹具的返回形状与 SDK 不一致，已改为显式 `count` 对象解码并重跑；最终三条均在上述目标断言失败，而非夹具初始化/反序列化失败。

## 后续修复（2026-10-15）

- 第 1 条（#52）：已按方案 A 修复。`clear_room_rebuild_edges_on` 在同一笔里把整表水位置为 `model_room_generation = NONE, model_room_stray_generations = []`；`run_rooms_rebuild` 在对 `SUL_DB` 清边成功后调用 `DbnumState::invalidate_mirror()`。
  - 单测：`cargo test --offline --lib room_rebuild_regression_same_generation_failure_keeps_alignment_owed -- --nocapture --test-threads=1`，结果为 1 passed。
  - 三条回归一起跑：`1 passed; 2 failed`，仍为红的只剩第 2 和第 3 条。这两条未在本次修改范围内。
  - `handlers.rs` 在编辑器的诊断为 errors 0、warnings 0。
  - 本次代码修改尚未提交，仍在当前工作区内。
- 第 2 条（#53）：已按「进度完成计数」方案修复。`try_begin_rebuild_in` 改为同时接收 lifecycle 与 progress 两个槽：生命周期为 `Loading`，或 progress 处于 `edge-recompute` 且 `dbnums_done < dbnums_total` 时拒绝，且拒绝时不重置进度、不翻生命周期；逐库循环完成后门自动放开。测试夹具由本地 `RefCell<String>` 改为本地 `ResidentProgress`，并补了「循环结束后重新放行」的断言。
  - 单测：`cargo test --offline --lib room_rebuild_regression_second_request_rejected_during_edge_recompute -- --nocapture --test-threads=1`，结果为 1 passed。
  - 三条回归一起跑：`2 passed; 1 failed`，仍为红的只剩第 3 条。
  - `room_resident.rs` 在编辑器的诊断为 errors 0、warnings 0。
  - 本次代码修改尚未提交，仍在当前工作区内。
- 第 3 条：已按最小方案修复（本次未单独建 GitHub Issue，经当面确认方案后直接修复）。`recompute_dbnum_room_edges_now` 的盖章由 `if let Err` 改为 `match`：`SUL_DB` 的 UPDATE 经 `.check()` 确认成功后，在 Ok 分支调用 `DbnumState::invalidate_mirrored(dbnum)`；失败分支保留原日志且不作废镜像。未连带调用 `note_room_generation_stamped`，理由是它只影响 ACK 逐出候选、后果保守且无测试覆盖，见当面方案记录。
  - 需要注意的是，第 3 条测试仍是源码契约守卫：它只能确认失效调用出现在 UPDATE 之后，无法自动证明调用位于成功分支；本次实现确实位于 Ok 分支，但这一点依赖代码评审，而非该测试本身。
  - 单测：`cargo test --offline --lib room_rebuild_regression_success_stamp_has_mirror_invalidation_contract -- --nocapture --test-threads=1`，结果为 1 passed。
  - 三条回归一起跑：`3 passed; 0 failed`。至此，本文件中的三条回归已全部完成修复。
  - `handlers.rs` 在编辑器的诊断为 errors 0、warnings 0。
  - 本次代码修改尚未提交，仍在当前工作区内。
