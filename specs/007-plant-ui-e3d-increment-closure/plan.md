# Implementation Plan：Plant UI 与 E3D 增量闭环

## Constitution Check

- 文件会话和水位分别保持各自真值，不用进程退出近似保存结果。
- 初始化门关闭响亮表现为让位，真实错误才进入失败账。
- 持久 pending 每行仍有消费、失败和复活出口，不增加第二工作真值。
- 自动化按 refno 与语义 role 定位，缺少目标时直接失败。

无宪法例外。

## Design

1. 模型 drain 逐根检查阶段门，以 TaskRegistry 公开一次消费尝试。
2. E3D 底层输出进程证据，l3 变更层读取文件会话并裁决是否可重试。
3. Plant UI 增加独立设置路径和树项可访问性身份；刷新等待数据、模型、pending 与视图代次。
4. 后端契约与 UI 编排测试分层，共用设备/管道变更及恢复宏。

## Verification

- Rust 纯单测覆盖让位、revision、E3D 分类、配置隔离和语义定位。
- 两仓格式化、检查和定向测试；gen-model 四个 CI 集成目标。
- SurrealDB 2.x 沙箱运行设备/管道 CRUD、保存后崩溃和积压抢占 live。
