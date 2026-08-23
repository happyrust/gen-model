# Implementation Plan：当期项目文件夹扫描范围

## 方案

1. 在 `src/data_interface/project_paths.rs` 建立单一的文件夹名校验与名单成员解析。
2. 让 `resolve_project_root` 只接受 `included_projects` 成员，并固定拼到 `project_path` 下。
3. 让 `plan_watch_dirs` 只遍历 `included_projects`，删除 `project_dirs` 回退。
4. 用纯函数/临时目录回归测试覆盖范围不扩张、空名单、名单外项目和非法文件夹名。
5. 更新 ADR-016 修订注记、配置变更记录与 `changelog.md`。

## Constitution Check

- **I 水位承诺**：不触及水位与事务。
- **II 单一实现**：所有路径继续共用 `resolve_project_root`，符合。
- **III 错误可见**：非法名单条目进入 `WatchDirPlan::problems`，不静默吞掉。
- **IV 队列收口**：不改变队列 action 或 drain。
- **V 标识真值**：不推导或近似任何项目标识。
- **VI 可执行守护**：新增旧写法会失败的回归测试。

## Complexity Tracking

无宪法例外；不引入新配置字段或第二条扫描路径。

## 验证

- 定向运行 `project_paths` 单元测试。
- `cargo fmt --check` 与 `cargo check`。
- `sigmap verify-ai-output`、`sigmap review-pr`。
