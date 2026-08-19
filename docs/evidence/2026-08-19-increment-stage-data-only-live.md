# 2026-08-19 增量阶段控制：8000 仅数据模式现场证据

## 环境

- 二进制：`D:\work\plant-code\old\test-worklspace\bin\aios-database.exe`
- 配置：`watch_dbnums=[8000]`、`data_incremental=true`、`model_incremental=false`、`room_incremental=false`
- SurrealDB：`127.0.0.1:8009`，项目 `AvevaMarineSample`
- HTTP：`127.0.0.1:9099`

## 结果

`GET /api/v1/health` 返回：

```json
{"data_incremental":true,"model_incremental":false,"room_incremental":false,"watch_dbnums":[8000],"worker_alive":true}
```

启动日志确认唯一共享 worker 采用 `data=true model=false room=false`。8000 已完成
`34..=232` 的收集与暂存应用，模型路径没有执行；日志准确说明水位只会在写回成功后推进。

现场同时复现既有数据写回停顿：窗口 `staging_8000_1` 保持 `active`，453 个预计写入行，
`staging_commit.last_duration_ms=0`，日志停在“开始写回”。因此当前水位仍为 33 的直接原因
位于数据阶段持久写回事务，已排除模型增量与房间增量造成阻塞。该停顿是后续数据写回诊断对象，
不把它记成阶段控制的成功提交。

## 产物

部署、配置差异、控制台日志、验证记录与回滚脚本：

`D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\increment-stages-20260819-090748`
