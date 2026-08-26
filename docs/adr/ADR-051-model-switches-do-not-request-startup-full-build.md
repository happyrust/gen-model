# ADR-051：模型开关不代表启动整库全量生成

- 状态：Accepted
- 日期：2026-08-25
- 关联：ADR-001（水位是提交承诺）、ADR-011（唯一批次队列）、ADR-025（严格初始化阶段）、ADR-050（进程级模型工作单）

## 背景

服务过去把 `DbOption::is_gen_mesh_or_model()` 直接赋给 `full_model_requested`。只要
`gen_model` 或 `gen_mesh` 为真，启动重扫完成后就绕过增量裁决，直接调用
`gen_all_geos_data`。即使文件最新会话号与 `applied_sesno` 完全一致，也会对
`manual_db_nums` 中的整个数据库执行 `SaveMode::FullBuild`。

这混淆了两个概念：模型/网格阶段是否启用，以及这次启动是否明确要求整库重建。

## 决策

1. `gen_model` / `gen_mesh` 是增量提交链的能力开关，不是整库重建命令。
2. 服务启动的唯一工作裁决是 watcher 重扫：对每个范围内文件执行身份检查，再比较
   `file_latest_sesno` 与 `applied_sesno`，只把首次导入、回退重建或真实增量放入唯一队列。
3. `run_cli` 不得调用 `gen_all_geos_data`、`begin_full_model`，也不得由
   `is_gen_mesh_or_model()` 配置启动全量模型屏障。
4. 数据阶段就绪后直接打开模型门，消费本次增量批次产生的精确模型工作；没有变化时
   模型阶段空转收口。
5. 显式整库生成能力继续由探针、Python/API 或其他直接调用 `gen_all_geos_data` 的入口提供，
   与服务重启解耦。
6. `watch_dbnums` 继续约束 watcher 比对范围与本进程模型工作单。

## 结果

- `gen_model=true`、`gen_mesh=true` 的服务重启会先比较会话水位；无变化时不生成模型。
- 首次导入和文件回退仍按既有 ADR-021 进入重建批次，不被误判为“无全量入口”。
- 启动日志明确区分“能力开启”和“本次实际存在模型工作”。
