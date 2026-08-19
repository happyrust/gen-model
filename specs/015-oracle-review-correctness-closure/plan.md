# Plan 015：Oracle 二次审核正确性收口

## Constitution Check

- 水位仍是与窗口数据、模型工作和提交回执同事务的承诺。
- watcher 与手动路径继续复用单队列和共享窗口实现。
- 新异常分支全部显式返回错误或进入可观察的 `commit_reconcile`。
- 不增加第二条消费路径或 SurrealDB 表；只扩展既有记录字段。

## 实施

1. 在 CATA 闭包、Dabacon 快照与调度器中建立显式上下文及激活门。
2. 在 staging lifecycle/executor 与恢复记录中加入 commit token 和硬分块校验。
3. 在 manifold 布尔入口按 `GeometryFailurePolicy` 区分窗口内外。
4. 发布依赖仓库，固定 revision，补齐单测、CI 集成测试、Release 与证据。

## Complexity Tracking

提交结果未知仍通过既有恢复记录和同一队列重放，没有引入新协调服务；激活门为进程内短临界区，不覆盖解析或 I/O。
