# ADR-033：增量阶段独立执行控制

- 状态：Accepted
- 日期：2026-08-19
- 关联：ADR-001、ADR-010、ADR-011、ADR-015、ADR-017、ADR-025

## 背景

增量链按「数据增量 → 模型增量 → 房间增量」顺序执行。现场定位数据水位、模型生成或房间归属问题时，需要只运行指定阶段；此前只有房间增量开关，数据批次与模型消费无法分别停住。

## 决策

1. 配置增加 `data_incremental`、`model_incremental`，与既有 `room_incremental` 组成三个独立执行许可，默认均为 `true`。环境变量 `AIOS_DATA_INCREMENTAL`、`AIOS_MODEL_INCREMENTAL`、`AIOS_ROOM_INCREMENTAL` 可覆盖配置。
2. 三个许可控制的是**消费阶段**，不是删除工作：
   - 数据关闭：扫描和入队仍执行，worker 不领取数据批次；
   - 模型关闭：数据批次仍可提交，`applied_sesno` 仍只承诺数据已落库，模型计划保留在 durable pending，worker 不消费模型积压；
   - 房间关闭：不消费房间目标，既有房间增量开关的补偿与重启回补语义保持不变。
3. 顺序依赖由既有队列和初始化门保证。下游开启不允许越过未完成的上游数据批次；关闭模型时也不把房间阶段标记为可执行。
4. `/api/v1/health` 必须同时暴露三个最终生效值。进程启动/首次遇到关闭阶段时必须打印有声提示。
5. 手动触发和 watcher 继续共用 ADR-011 的唯一批次队列及同一组阶段门，不新增消费路径。

## 结果

调试数据水位时可配置 `data_incremental=true`、`model_incremental=false`、`room_incremental=false`。数据成功提交后水位推进，模型与房间副作用不执行；恢复开关并重启后，durable 模型积压可继续消费。
