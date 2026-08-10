# Issue #19：跨会话父子删除被最终 OWNER 状态误判

`db8000-sesno24-26.zip` 保存 dbnum 8000 的三个真实 session-chain 快照：

| 快照 | sesno | EQUI `24384/24778` | 子节点 `24384/24779` |
|---|---:|---|---|
| baseline | 24 | 存在 | 存在 |
| child-deleted | 25 | 存在 | 已删除 |
| parent-deleted | 26 | 已删除 | 已删除 |

运行回归：

```powershell
cargo test --test db8000_two_delete_fixture -- --nocapture
```

测试会验证 ZIP 与三个文件的 SHA256、安全解压、会话切片一致性，并使用 sesno 26
最终文件直接采集 `25..=26` 后校验原始操作与净模型变化。
