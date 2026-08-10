# Issue #20：db8000 可移植模型增量回归与 CI 门禁

## 目标

复用 Issue #19 已入库的 dbnum 8000、sesno `24 → 25 → 26` ZIP，建立一组在本地与
GitHub Actions 都可直接执行的模型增量案例。首批案例不依赖 AVEVA、SurrealDB、Git
LFS 或外部解压程序。

## Oracle 评审结论

Oracle 会话 `db8000-model-increment-suite` 建议首批只覆盖现有 ZIP 能证明的行为：

1. ZIP 安全与三个快照会话号；
2. `collect_changes(25..=26)` 的四条原始操作；
3. 最终文件中的 sesno 25 历史与 sesno 25 点时快照一致；
4. 整窗采集与两个单会话切片的并集一致；
5. 净变化收敛为 BOX Deleted、EQUI Deleted、ZONE Modified；
6. 删除模型计划只保留父 EQUI 的一次递归清理。

测试经过生产入口 `IncrementPipeline::collect_changes` 与 `merge_net_changes`；对比 JSON
只作为人工证据，不作为测试输入。

## 入口

本地：

```powershell
cargo test --locked --test db8000_two_delete_fixture `
  --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture

cargo test --locked --lib `
  child_delete_then_parent_delete_across_sessions_schedules_only_the_parent `
  --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
```

CI：`.github/workflows/windows-tests.yml`，在 pull request、`main` push 和手工触发时运行。

## 后续需要新快照的案例

- 纯 `POS`/`ORI` 变化 → `Transform`；
- BRAN/HANG 派生几何变化 → `RegenRoot`；
- 反向引用或目录变化 → `CascadeExpand`；
- PANE 移动或 ROOM 改名 → `RoomRecalcPanel/Element`。

这些场景应分别建立新的会话快照，不能从当前删除夹具推断预期结果。
