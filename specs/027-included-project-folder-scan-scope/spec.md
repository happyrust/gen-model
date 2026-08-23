# Feature Specification：当期项目文件夹扫描范围

## 目标

当 `project_path` 指向项目集合根目录时，只扫描 `included_projects` 点名的直接子文件夹。
`included_projects` 中的值就是文件夹名称，不是别名、绝对路径或另一份路径表的索引。

## 功能要求

- **FR-001**：项目根必须解析为 `project_path/<included_projects 文件夹名>`。
- **FR-002**：名单外项目不得得到可扫描的项目根。
- **FR-003**：`project_dirs` 不得扩大、替换或重定向名单确定的扫描范围。
- **FR-004**：空 `included_projects` 必须产生空扫描计划，不得回退到 `project_dirs`。
- **FR-005**：绝对路径、UNC、`.`、`..` 和多段相对路径不是合法文件夹名，必须作为可见的
  项目解析问题报告。
- **FR-006**：自动 watcher、手动扫描、初始化与项目依赖定位必须共用同一项目根解析规则。

## 验收场景

1. `project_path=P`、`included_projects=[A]` 时，只收集 `P/A` 下的库目录；即使
   `project_dirs=[B]` 且 `P/B` 存在，也不扫描 `P/B`。
2. 请求解析不在名单中的 `B` 时返回未解析，不访问 `P/B`。
3. `included_projects=[]`、`project_dirs=[B]` 时扫描计划为空。
4. 名单条目为 `A/B`、`..` 或绝对路径时，扫描计划不包含该路径并给出可见原因。

## 非目标

- 本特性不改变库目录 `*000` 的识别规则。
- 本特性不改变 MDB/dbnum 层面的执行范围判定。
