# ADR-046：`included_projects` 以项目文件夹名限定扫描范围

- 状态：Accepted
- 日期：2026-08-24
- 修订：ADR-016「监控目录解析与项目数据域」中的 `project_dirs` 路径覆盖约定
- 引用：ADR-011（单队列与共享派发器）、ADR-016（监控目录解析与项目数据域）

## 背景

当期扫描的物理根已经由 `project_path` 给出，`included_projects` 是这个根目录下允许扫描的
项目文件夹名。旧实现却把 `project_dirs` 当成与 `included_projects` 按下标对应的物理位置
覆盖层；名单为空时甚至退回扫描 `project_dirs`。这让不属于当期名单的路径仍可能进入扫描，
也让同一个项目名因另一份配置被重定向到 `project_path` 之外。

## 决策

1. `included_projects` 是当期项目扫描范围的唯一名单，每个值必须是 `project_path` 下的单个
   文件夹名，不接受绝对路径、UNC、`.`、`..` 或多段相对路径。
2. 项目根统一解析为 `project_path/<included_projects 中的文件夹名>`；大小写不敏感地确认
   名单成员，但拼接时使用配置里记录的原始文件夹名。
3. 不在 `included_projects` 的项目不解析目录、不挂 watcher、不进入手动扫描或全量摄入。
4. `included_projects` 为空表示当期没有项目可扫描，不再退回 `project_dirs`。
5. `project_dirs` 不再参与项目扫描范围或项目根解析。字段暂时保留，避免旧配置反序列化失败。
6. 自动 watcher、手动触发、初始化与依赖定位继续共用 `resolve_project_root`，不另造范围判定。

## 结果

- 扫描范围只由 `project_path + included_projects` 决定，配置含义与现场约定一致。
- `project_dirs` 无法扩大或重定向当期扫描范围。
- 旧的跨根/UNC 单项目覆盖方式停止生效；需要扫描共享根时，应把共享根配置为
  `project_path`，并在其下用文件夹名列出 `included_projects`。

## 回滚

回滚本 ADR 时恢复 ADR-016 的按下标 `project_dirs` 覆盖与空名单回退逻辑，并恢复对应测试；
回滚前必须确认不会重新把名单外路径带进当期扫描。
