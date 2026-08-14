# ADR-027：E3D 变更宏按文件保存事实分类

状态：Accepted（2026-08-14）

关联：ADR-019（无人值守驱动通道）、ADR-020（预览与执行边界）、ADR-021（水位承诺）、
ADR-025（严格初始化阶段）；`specs/007-plant-ui-e3d-increment-closure/`

## 背景

E3D 可能在 `SAVEWORK` 已推进文件会话后、包装宏写出 DONE 标记前退出。把非零退出或缺少
DONE 一律视为“未执行”并重试，会把同一个增加、删除或修改再次应用。

## 决策

1. 底层驱动返回 ALIVE、DONE、退出状态和日志路径，不凭错误文本决定重试。
2. 变更执行器必须在宏前后读取同一目标 DB 的文件身份与最新会话号。
3. DONE 存在为 `Completed`；DONE 缺失但会话增加为 `SavedButUnconfirmed`；未进入宏且会话
   未变的已知启动故障为 `FailedBeforeSave`；其余为 `Indeterminate`。
4. `SavedButUnconfirmed` 不重放变更，只继续真实状态验证及恢复。只有
   `FailedBeforeSave` 允许一次自动重试。
5. `merged_sesnos` 仍从“预览确认后出现的保存”计算，不以进程退出状态替代文件真值。

## 后果

- 变更宏必须显式声明目标 DB 文件；只读查询继续使用普通驱动接口。
- 会话前后值、标记和退出状态进入测试证据，便于区分启动失败、已保存崩溃和未知状态。
- 不新增持久表，恢复仍依赖文件会话、水位和幂等清理宏。
