# dbnum=8000 / sesno=239 staging 写回卡顿修复证据

日期：2026-08-19

## 现场与根因

- 任务：`db-20260819-143110-000005`，保存时间 `2026-08-19T06:30:54Z`，窗口 `239..=239`。
- 依赖闭包已完成：CATA `41/41`，`missing=0`；停点是 `commit`，不是文件监听或依赖解析。
- 暂存窗口包含 167 条 journal、118757 字节 SQL、预计 869 行写入。旧执行器只按
  500 条 journal 分块，故 167 条全部进入一个大事务。
- 卡住时 SurrealDB 持续占用单核、工作集约 5.5 GiB，连接 ping 仍约 4 ms；控制台每十秒
  只打印“提交等待”，没有真实进展，也没有 commit 查询超时。停止 aios-database 后该查询
  随连接取消，SurrealDB CPU 降至约 0.6%。这说明问题是无界大事务，而非 watcher 漏事件。

## 修复

- 普通 journal 同时按 `32` 条、`64 KiB`、预计 `250` 行切块；任何一维触顶即结束当前块。
- 显式事务保持原子，但独占一个块。
- 每个块打印序号、journal 数、字节数、预计行数、事务类型与稳定指纹；只有块成功后才刷新
  `stage_last_progress_at`。
- 每次 SurrealDB commit 查询设置 120 秒连续无返回边界；超时直接使窗口失败，水位不推进，
  后续按同一权威窗口幂等重放。
- 尾事务仍最后提交，因此分块中间写不会提前承诺水位。

## 命令与字面结果

### 单元测试

```text
cargo test --locked --lib data_interface::staging::executor::tests \
  --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1000 filtered out
exit status: 0
```

覆盖多维切块、显式事务隔离、执行顺序、失败门控、重放收敛和资源估算错误上下文。

### 编译与构建

```text
cargo check --locked --bin aios-database --no-default-features \
  --features ws,gen_model,manifold,occ,project_hd,http_api
Finished `dev` profile ...
exit status: 0

cargo build --release --locked --bin aios-database --no-default-features \
  --features ws,gen_model,manifold,occ,project_hd,http_api
Finished `release` profile [optimized] target(s) in 1m 38s
exit status: 0
```

### Live 状态

旧进程最终在被停止前完成了原来的 239 大事务，因此没有人为制造额外 E3D 保存来重复写现场数据；
本次 live 验收验证已提交数据、服务恢复、空 staging、模型树，以及新二进制的启动与回滚。

```text
SELECT dbnum, applied_sesno, file_latest_sesno FROM dbnum_watermark WHERE dbnum=8000;
[{"applied_sesno":239,"dbnum":8000,"file_latest_sesno":239}]

SELECT count() AS staging_windows FROM staging_window GROUP ALL;
[{"staging_windows":0}]

GET /api/v1/health
status=ok initialization.status=model_ready worker_alive=true
staging_windows=[] sul_db.connected=true spatial_tree.state=ready
```

数据库与 Plant UI 均可见复制保存的树：

```text
pe:24384_26205 /Copy-of--SR-CSV-S5001 owner=pe:24384_26199
pe:24384_26206 /Copy-of--SR-CSV-S5001/MAIN owner=pe:24384_26205
pe:24384_26211 /Copy-of--SR-CSV-S5001/SUBS1 owner=pe:24384_26205
直接子元素总计 5 个（FRMW、2 SUBS、2 TEXT）
```

Plant UI 截图：
`D:\work\plant-code\old\plant-ui\artifacts\db8000-ses239-copy-after-writeback-fix.png`。

## 部署与回滚

部署目录：
`D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\staging-writeback-20260819-145510`

```text
original sha256=896C2C1F688F2CB238C0B0EBACE912ED5A27B14AEA085AF373E0BD71CE5E63C6
modified sha256=D63EBECA9B11E3216300D606E73F4C67D4C5007E7CDCB15D538E2A08725D9F79
rollback verified sha256=896C2C1F688F2CB238C0B0EBACE912ED5A27B14AEA085AF373E0BD71CE5E63C6
redeploy verified sha256=D63EBECA9B11E3216300D606E73F4C67D4C5007E7CDCB15D538E2A08725D9F79
```

最终 release 服务运行于 `127.0.0.1:9099`，SurrealDB 保持在 `127.0.0.1:8009`。
最终进程为 `aios-database` PID 30600、`plant-ui-app` PID 19780，二者均响应。

## SigMap 收口

- `sigmap verify-plan specs/014-bounded-staging-writeback/plan.md`：通过。
- `sigmap verify-ai-output docs/evidence/2026-08-19-db8000-staging-writeback-stall-fix.md`：通过。
- `sigmap scaffold "bounded staging writeback"`：仓库未检测到统一文件命名约定，工具按设计拒绝。
- `sigmap review-pr`：对共享工作树中的 388 个既有变更做全局审计并报 88 项；其范围包含
  16 个顶层目录及本任务未触及的 Python、CI、数据库连接字符串等，因此不能作为本补丁的
  局部通过结论。本任务相关执行器已有内联单测 10 条，worker commit 回归 2 条均通过。
