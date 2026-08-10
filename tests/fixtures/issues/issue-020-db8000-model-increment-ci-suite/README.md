# Issue #20：db8000 模型增量 CI 案例

本目录记录 Issue #20 的 Oracle 设计评审、验证结果与可逆补丁。实际数据库 ZIP 继续
复用相邻的 `issue-019-cross-session-parent-child-delete`，避免重复存储 4.49 MiB 数据。

权威执行入口：

```powershell
cargo test --locked --test db8000_two_delete_fixture `
  --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
```

GitHub Actions 入口：`.github/workflows/windows-tests.yml`。
