# Python 增量测试接口

这里的 Python 代码只负责编排和断言；PDMS/E3D 解析、增量折叠及数据写入仍由 Rust 实现。
所有模块仅使用 Python 标准库，因此本地和 GitHub Actions 都无需安装额外包。

## 接口

- `GenModelClient`：覆盖健康检查、查询、preview/execute、任务、队列、模型生成、pending unit、DBNUM 状态与清理接口。
- `RustTools`：调用 `incr_fold_probe`、`l3_suite`，也可用上下文管理器启动并自动关闭 `aios-database`。
- `E3dTtyRunner`：复制 E3D macro 到证据目录，移除会关闭 TTY 的 `FINISH/QUIT`，重定向 `ALPHA LOG`，再交给 Rust `l3_suite`。
- `run_db8000_increment.py`：组合 E3D 变更、Rust 折叠、HTTP preview/execute、水位等待与 finally 恢复。

## 本地验证

```powershell
python -m unittest discover -s tests/python -p "test_*.py" -v
```

只验证既有 DB 文件的 Rust 增量折叠与 API preview：

```powershell
python scripts/python/run_db8000_increment.py `
  --bin-dir D:\work\plant-code\old\test-worklspace\bin `
  --db-file D:\PATH\ams8000_0001 `
  --from-sesno 27 --to-sesno 28
```

真实 E3D TTY 案例额外传入 `--macro`、`--restore-macro`、`--project-dir` 和
`--e3d-install`。只有显式传入 `--execute` 才提交正常增量更新；restore macro
始终在 `finally` 中运行，日志与 `summary.json` 写入唯一证据目录。
