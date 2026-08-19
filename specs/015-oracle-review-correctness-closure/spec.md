# Spec 015：Oracle 二次审核正确性收口

## 目标

收口 Oracle 会话 `gen-model-increment-review-20260819` 已确认的跨窗口缓存、Ref0 冲突、快照绑定、epoch 激活、提交结果确认、硬分块与几何失败策略缺陷，并保证干净克隆可复现构建。

## 需求

1. 暂存 CATA 缓存只使用显式的源 dbnum 与实际窗口右端；冲突 Ref0 仅在被引用时阻断。
2. 维护纠正的成员判断与删除展开必须复用收集窗口冻结的 `SnapshotToken`。
3. epoch 安装与任务冻结共用激活门，锁序为 activation gate → scheduler queue → coordinator。
4. 暂存尾事务具有稳定提交令牌；超时进入 `commit_reconcile`，重放同一尾事务不得重复推进 revision 或空间 epoch。
5. journal planner 按包装后字节数、预计行数和条目数硬验收，不可拆条目超限时返回确定性错误。
6. 活动暂存窗口的几何失败使用 `Required`；窗口外生成使用 `BestEffortFallback`。
7. 三个依赖仓库使用已发布 Git revision，本仓不提交本地 `[patch]` 或路径依赖。

## 成功标准

- 分窗缓存、Ref0 冲突、快照替换、epoch 交错、尾事务结果未知、硬上限和两类几何策略均有回归测试。
- 水位只在受提交令牌保护的尾事务内推进；结果未确认期间同 dbnum 不产生第二提交。
- `cargo metadata --locked` 与 Release 构建在不依赖兄弟目录时解析到固定 revision。
