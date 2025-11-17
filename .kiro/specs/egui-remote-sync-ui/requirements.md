# Requirements Document

## Introduction

本需求文档定义了基于 egui 0.33 + glow 的异地协同运维原生界面的功能需求。该界面将复刻现有 Next.js Web UI 的核心功能，提供独立的桌面应用体验，支持配置异地部署节点、启动 web-server、一键部署项目站点等核心运维操作。

当前系统已实现：
- 基于 Next.js 的 Web UI（frontend/v0-aios-database-management/components/remote-sync）
- 后端异地同步核心功能（MQTT + 文件监控 + HTTP 上传）
- 完整的 REST API（remote_sync_handlers.rs, topology_handlers.rs）

需要新增的能力：
- 基于 egui 的原生 GUI 界面
- 环境和站点的可视化配置
- 拓扑图编辑器（Canvas 画布）
- 实时状态监控面板
- 一键部署和启动功能

## Glossary

- **egui**: Rust 生态的即时模式 GUI 库，支持跨平台原生渲染
- **glow**: OpenGL 抽象层，egui 的渲染后端之一
- **RemoteSyncEnv（远程同步环境）**: 代表一个地理位置的同步环境，包含 MQTT 和文件服务器配置
- **RemoteSyncSite（远程站点）**: 环境下的具体外部站点，用于数据同步目标
- **TopologyCanvas（拓扑画布）**: 可视化配置环境和站点连接关系的交互式画布
- **WebServer**: 内置的 Axum Web 服务器，提供 REST API 和文件服务
- **DeploymentSite（部署站点）**: 项目部署的目标站点配置

## Requirements

### Requirement 1: 主窗口和导航

**User Story:** 作为运维人员，我希望通过原生桌面应用访问异地协同管理功能，以便获得更好的性能和离线使用能力

#### Acceptance Criteria

1. WHEN 运维人员启动应用，THE EguiRemoteSyncUI SHALL 显示主窗口，包含顶部菜单栏、左侧导航栏和主内容区域
2. WHEN 运维人员点击导航栏项目，THE EguiRemoteSyncUI SHALL 切换到对应的功能页面（环境列表/拓扑配置/监控面板/日志查询）
3. WHEN 应用窗口大小改变，THE EguiRemoteSyncUI SHALL 自动调整布局以适应新的窗口尺寸
4. WHEN 运维人员关闭应用，THE EguiRemoteSyncUI SHALL 保存当前窗口位置和大小到配置文件
5. WHEN 运维人员重新打开应用，THE EguiRemoteSyncUI SHALL 恢复上次的窗口位置、大小和选中的页面

### Requirement 2: 环境配置管理

**User Story:** 作为运维人员，我希望通过图形界面创建和编辑异地协同环境配置，以便快速建立多地区的数据同步网络

#### Acceptance Criteria

1. WHEN 运维人员访问环境列表页面，THE EguiRemoteSyncUI SHALL 显示所有已配置的环境，包含名称、MQTT 地址、文件服务器地址、地区标识和状态指示器
2. WHEN 运维人员点击"添加环境"按钮，THE EguiRemoteSyncUI SHALL 显示环境配置表单，包含名称、MQTT 主机、MQTT 端口、文件服务器地址、地区标识、本地数据库编号列表等字段
3. WHEN 运维人员填写环境配置并点击保存，THE EguiRemoteSyncUI SHALL 验证输入格式（MQTT 端口 1-65535、URL 格式、数据库编号逗号分隔），调用 POST /api/remote-sync/envs API 创建环境
4. WHEN 运维人员选中环境并点击"编辑"按钮，THE EguiRemoteSyncUI SHALL 显示预填充的配置表单，允许修改除 ID 外的所有字段
5. WHEN 运维人员选中环境并点击"删除"按钮，THE EguiRemoteSyncUI SHALL 显示确认对话框，确认后调用 DELETE /api/remote-sync/envs/{id} API 删除环境

### Requirement 3: 站点配置管理

**User Story:** 作为运维人员，我希望为每个环境配置多个远程站点，以便实现一对多的数据分发

#### Acceptance Criteria

1. WHEN 运维人员在环境详情页面点击"添加站点"按钮，THE EguiRemoteSyncUI SHALL 显示站点配置表单，包含名称、所属环境、地区标识、HTTP 地址、数据库编号列表、备注等字段
2. WHEN 运维人员填写站点配置并点击保存，THE EguiRemoteSyncUI SHALL 验证输入格式（HTTP URL 格式、数据库编号逗号分隔），调用 POST /api/remote-sync/sites API 创建站点
3. WHEN 运维人员选中站点并点击"测试连接"按钮，THE EguiRemoteSyncUI SHALL 调用 POST /api/remote-sync/sites/{id}/test API，显示连接测试结果（成功/失败、延迟、错误信息）
4. WHEN 运维人员选中站点并点击"查看元数据"按钮，THE EguiRemoteSyncUI SHALL 调用 GET /api/remote-sync/sites/{id}/metadata API，在弹窗中显示站点的同步文件列表
5. WHEN 运维人员选中站点并点击"删除"按钮，THE EguiRemoteSyncUI SHALL 显示确认对话框，确认后调用 DELETE /api/remote-sync/sites/{id} API 删除站点

### Requirement 4: 可视化拓扑配置

**User Story:** 作为运维人员，我希望通过可视化的画布配置环境和站点的连接关系，以便直观地设计和管理复杂的同步拓扑结构

#### Acceptance Criteria

1. WHEN 运维人员访问拓扑配置页面，THE EguiRemoteSyncUI SHALL 显示一个可交互的 Canvas 画布，支持鼠标拖拽、滚轮缩放和平移操作
2. WHEN 运维人员点击"添加环境节点"按钮并在画布上点击，THE EguiRemoteSyncUI SHALL 在点击位置创建环境节点（矩形，蓝色边框），显示环境名称和 MQTT 地址
3. WHEN 运维人员点击"添加站点节点"按钮并在画布上点击，THE EguiRemoteSyncUI SHALL 在点击位置创建站点节点（圆形，绿色边框），显示站点名称和 HTTP 地址
4. WHEN 运维人员选中环境节点并拖拽到站点节点，THE EguiRemoteSyncUI SHALL 创建有向连线（箭头从环境指向站点），表示同步关系
5. WHEN 运维人员点击节点，THE EguiRemoteSyncUI SHALL 在右侧面板显示节点详细配置，支持实时编辑属性
6. WHEN 运维人员点击"自动布局"按钮，THE EguiRemoteSyncUI SHALL 应用层次布局算法（环境节点在上层，站点节点在下层），自动调整节点位置
7. WHEN 运维人员点击"保存拓扑"按钮，THE EguiRemoteSyncUI SHALL 验证拓扑有效性（环境必须有 MQTT 配置、站点必须关联环境），调用 POST /api/topology/save API 保存配置
8. WHEN 运维人员点击"导出 JSON"按钮，THE EguiRemoteSyncUI SHALL 将拓扑配置导出为 JSON 文件，包含所有环境、站点和连接关系
9. WHEN 运维人员点击"导入 JSON"按钮并选择文件，THE EguiRemoteSyncUI SHALL 解析 JSON 文件，验证格式后加载拓扑到画布

### Requirement 5: 实时监控面板

**User Story:** 作为运维人员，我希望实时查看同步状态和性能指标，以便及时发现和处理同步异常

#### Acceptance Criteria

1. WHEN 运维人员访问监控面板页面，THE EguiRemoteSyncUI SHALL 显示环境状态卡片，包含环境名称、运行状态（运行中/暂停/停止）、MQTT 连接状态、站点数量、队列大小
2. WHEN 运维人员点击"刷新"按钮，THE EguiRemoteSyncUI SHALL 调用 GET /api/sync/status API 获取最新状态，并在 1 秒内更新界面显示
3. WHEN 同步任务状态发生变化，THE EguiRemoteSyncUI SHALL 通过轮询（每 5 秒）或 WebSocket 连接实时更新任务列表
4. WHEN 运维人员查看任务列表，THE EguiRemoteSyncUI SHALL 显示每个任务的文件名、源环境、目标站点、状态（待处理/运行中/完成/失败）、进度条（0-100%）
5. WHEN 运维人员点击任务行，THE EguiRemoteSyncUI SHALL 在弹窗中显示任务详情，包含文件路径、大小、记录数、开始时间、完成时间、错误信息

### Requirement 6: 运维操作工具栏

**User Story:** 作为运维人员，我希望通过工具栏快速执行常见的运维操作，以便快速响应系统问题

#### Acceptance Criteria

1. WHEN 运维人员点击"启动同步"按钮，THE EguiRemoteSyncUI SHALL 调用 POST /api/sync/start API，并在 3 秒内显示启动结果（成功/失败、错误信息）
2. WHEN 运维人员点击"停止同步"按钮，THE EguiRemoteSyncUI SHALL 显示确认对话框，确认后调用 POST /api/sync/stop API，等待服务停止完成
3. WHEN 运维人员点击"暂停同步"按钮，THE EguiRemoteSyncUI SHALL 调用 POST /api/sync/pause API，暂停新任务处理但不中断运行中的任务
4. WHEN 运维人员点击"恢复同步"按钮，THE EguiRemoteSyncUI SHALL 调用 POST /api/sync/resume API，恢复任务处理
5. WHEN 运维人员点击"清空队列"按钮，THE EguiRemoteSyncUI SHALL 显示二次确认对话框，确认后调用 POST /api/sync/queue/clear API，清空所有待处理任务

### Requirement 7: 日志查询和过滤

**User Story:** 作为运维人员，我希望查询和过滤同步日志，以便排查历史问题和审计同步操作

#### Acceptance Criteria

1. WHEN 运维人员访问日志查询页面，THE EguiRemoteSyncUI SHALL 显示日志筛选表单，包含环境选择器、站点选择器、状态选择器（全部/待处理/运行中/完成/失败）、时间范围选择器
2. WHEN 运维人员填写筛选条件并点击"查询"按钮，THE EguiRemoteSyncUI SHALL 调用 GET /api/remote-sync/logs API（带查询参数），并在 2 秒内显示查询结果
3. WHEN 查询结果超过 100 条，THE EguiRemoteSyncUI SHALL 显示分页控件，支持上一页/下一页/跳转到指定页
4. WHEN 运维人员点击日志行，THE EguiRemoteSyncUI SHALL 在弹窗中显示完整的日志详情，包含文件路径、大小、记录数、耗时、错误信息、重试历史
5. WHEN 运维人员点击"导出 CSV"按钮，THE EguiRemoteSyncUI SHALL 将当前筛选结果导出为 CSV 文件，保存到用户选择的路径

### Requirement 8: Web Server 启动和管理

**User Story:** 作为运维人员，我希望通过 GUI 界面启动和管理内置的 Web Server，以便提供 REST API 和文件服务

#### Acceptance Criteria

1. WHEN 运维人员访问 Web Server 配置页面，THE EguiRemoteSyncUI SHALL 显示服务器配置表单，包含监听地址（默认 0.0.0.0）、监听端口（默认 3000）、数据库路径、静态文件目录
2. WHEN 运维人员点击"启动服务器"按钮，THE EguiRemoteSyncUI SHALL 在后台线程启动 Axum Web Server，并在 5 秒内显示启动结果（成功/失败、监听地址）
3. WHEN Web Server 启动成功，THE EguiRemoteSyncUI SHALL 在状态栏显示绿色指示器和监听地址（如 http://0.0.0.0:3000）
4. WHEN 运维人员点击"停止服务器"按钮，THE EguiRemoteSyncUI SHALL 优雅关闭 Web Server（等待当前请求完成），并在 10 秒内显示停止结果
5. WHEN Web Server 运行时发生错误，THE EguiRemoteSyncUI SHALL 在界面顶部显示错误横幅，包含错误信息和重启按钮

### Requirement 9: 一键部署项目站点

**User Story:** 作为运维人员，我希望通过一键操作部署项目到远程站点，以便快速完成项目分发

#### Acceptance Criteria

1. WHEN 运维人员访问部署页面，THE EguiRemoteSyncUI SHALL 显示部署配置表单，包含项目路径选择器、目标站点多选框、部署选项（是否备份、是否重启服务）
2. WHEN 运维人员选择项目路径并点击"扫描"按钮，THE EguiRemoteSyncUI SHALL 扫描项目目录，显示将要部署的文件列表和总大小
3. WHEN 运维人员选择目标站点并点击"开始部署"按钮，THE EguiRemoteSyncUI SHALL 为每个站点创建部署任务，调用 POST /api/deploy/start API 开始部署
4. WHEN 部署任务运行时，THE EguiRemoteSyncUI SHALL 显示进度条和实时日志输出，包含当前步骤（压缩/上传/解压/重启）和完成百分比
5. WHEN 所有部署任务完成，THE EguiRemoteSyncUI SHALL 显示部署摘要，包含成功数量、失败数量、总耗时，并提供查看详细日志的链接

### Requirement 10: 配置持久化和加载

**User Story:** 作为运维人员，我希望应用自动保存和加载配置，以便下次启动时恢复上次的工作状态

#### Acceptance Criteria

1. WHEN 运维人员修改环境或站点配置，THE EguiRemoteSyncUI SHALL 自动保存配置到 SQLite 数据库（deployment_sites.sqlite）
2. WHEN 运维人员修改窗口布局（大小、位置、分栏比例），THE EguiRemoteSyncUI SHALL 保存布局配置到本地配置文件（~/.config/egui_remote_sync/layout.json）
3. WHEN 运维人员修改 Web Server 配置，THE EguiRemoteSyncUI SHALL 保存配置到 DbOption.toml 文件
4. WHEN 应用启动时，THE EguiRemoteSyncUI SHALL 从 SQLite 数据库加载环境和站点配置，从本地配置文件加载窗口布局
5. WHEN 配置文件不存在或损坏，THE EguiRemoteSyncUI SHALL 使用默认配置并显示警告提示

### Requirement 11: 主题和样式定制

**User Story:** 作为运维人员，我希望定制界面主题和样式，以便获得更舒适的视觉体验

#### Acceptance Criteria

1. WHEN 运维人员访问设置页面，THE EguiRemoteSyncUI SHALL 显示主题选择器，包含浅色主题、深色主题和自动跟随系统选项
2. WHEN 运维人员切换主题，THE EguiRemoteSyncUI SHALL 立即应用新主题，更新所有界面元素的颜色和样式
3. WHEN 运维人员调整字体大小滑块，THE EguiRemoteSyncUI SHALL 实时更新界面字体大小（范围 10-20px）
4. WHEN 运维人员选择自定义颜色，THE EguiRemoteSyncUI SHALL 允许修改主色调、强调色、背景色、文本色
5. WHEN 运维人员点击"重置为默认"按钮，THE EguiRemoteSyncUI SHALL 恢复所有样式设置为默认值

### Requirement 12: 错误处理和用户反馈

**User Story:** 作为运维人员，我希望在操作失败时获得清晰的错误提示，以便快速定位和解决问题

#### Acceptance Criteria

1. WHEN API 调用失败（网络错误、超时、服务器错误），THE EguiRemoteSyncUI SHALL 在界面顶部显示错误横幅，包含错误类型、错误信息和重试按钮
2. WHEN 表单验证失败，THE EguiRemoteSyncUI SHALL 在对应输入框下方显示红色错误提示文本，并禁用提交按钮
3. WHEN 长时间操作（如部署、大文件上传）进行中，THE EguiRemoteSyncUI SHALL 显示加载指示器和进度条，防止用户重复点击
4. WHEN 操作成功完成，THE EguiRemoteSyncUI SHALL 在界面右下角显示绿色成功提示（Toast），3 秒后自动消失
5. WHEN 发生致命错误（如数据库损坏、配置文件缺失），THE EguiRemoteSyncUI SHALL 显示模态对话框，包含错误详情和建议解决方案，并提供"查看日志"和"重置配置"按钮
