# 增量阶段控制实施计划

## Constitution Check

- 水位是承诺：模型关闭时复用 ADR-017 延后模型阶段，数据与水位原子提交，模型计划 durable 留存。
- 单一消费路径：只在共享 `drain_queue_until_empty` 和共享 worker 阶段增加门，不复制 manual/watcher 路径。
- 静默失效：health 与首次日志都公开最终生效值；关闭阶段不丢队列行。
- 可验证：新增纯函数和源码门控回归测试，使用既有 nightly 质量门。

## 实施步骤

1. 在 `src/options.rs` 增加数据/模型字段、环境变量覆盖及统一阶段快照。
2. 在 `src/data_interface/batch_worker.rs` 给数据领取、模型批内执行、模型空闲消费和房间收口接入阶段门。
3. 在 `src/web_service/handlers.rs` 暴露三个阶段状态并补回归测试。
4. 更新 `CONTEXT.md`、根配置、pytest 配置与 `changelog.md`。
5. 运行格式化、定向单测、`cargo check`、release 构建，部署到测试工作区，以仅数据配置启动并检查日志/health。
