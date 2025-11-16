# Implementation Plan

## 1. 基础架构搭建

- [ ] 1.1 创建前端路由结构
  - 在 `app/remote-sync/` 下创建所有页面路由
  - 配置 Next.js App Router 和布局组件
  - 添加导航菜单项到侧边栏
  - _Requirements: 1.1, 10.1_

- [ ] 1.2 设置后端 SSE 事件流
  - 实现 `sse_handlers.rs` 模块
  - 创建事件广播通道 (tokio::sync::broadcast)
  - 实现 `/api/sync/events` SSE 端点
  - 集成到 SyncControlCenter 的事件发送
  - _Requirements: 2.1, 2.2_

- [ ] 1.3 创建前端数据模型和类型定义
  - 定义 TypeScript 接口 (Environment, Site, SyncTask, SyncLog, Metrics, Alert)
  - 创建 API 客户端基础函数
  - 实现 SSE 连接 Hook (useSSE)
  - _Requirements: 所有需求_

- [ ] 1.4 配置前端状态管理
  - 安装和配置 React Query
  - 创建全局状态 Context (如告警状态)
  - 实现数据缓存策略
  - _Requirements: 所有需求_

## 2. 部署向导功能

- [ ] 2.1 实现部署向导主组件
  - 创建 `DeployWizard` 组件和步骤状态管理
  - 实现步骤导航逻辑 (上一步/下一步)
  - 添加进度指示器
  - _Requirements: 1.1, 1.2_

- [ ] 2.2 实现基本信息输入步骤
  - 创建 `StepBasicInfo` 组件
  - 实现表单字段 (环境名称、MQTT 配置、文件服务器、地区)
  - 添加实时表单验证
  - _Requirements: 1.2_

- [ ] 2.3 实现站点配置步骤
  - 创建 `StepSiteConfig` 组件
  - 实现站点列表管理 (添加/编辑/删除)
  - 支持批量导入站点信息
  - _Requirements: 1.3_

- [ ] 2.4 实现连接测试步骤
  - 创建 `StepConnectionTest` 组件
  - 实现 MQTT 连接测试 API 调用
  - 实现 HTTP 可达性测试
  - 显示测试结果和延迟信息
  - _Requirements: 1.4_

- [ ] 2.5 实现激活确认步骤
  - 创建 `StepActivation` 组件
  - 显示配置预览
  - 调用激活 API 并显示进度
  - 处理激活成功/失败状态
  - _Requirements: 1.5_

- [ ] 2.6 创建部署向导页面
  - 创建 `app/remote-sync/deploy/page.tsx`
  - 集成 DeployWizard 组件
  - 实现完成后跳转到监控页面
  - _Requirements: 1.1, 1.5_

## 3. 监控仪表板功能

- [ ] 3.1 实现环境状态卡片组件
  - 创建 `EnvironmentCard` 组件
  - 显示环境基本信息和运行状态
  - 显示 MQTT 连接状态指示器
  - 显示站点数量和队列大小
  - _Requirements: 2.3, 2.4_

- [ ] 3.2 实现任务列表组件
  - 创建 `TaskList` 组件
  - 实现虚拟滚动优化 (react-virtual)
  - 显示任务状态和进度条
  - 支持任务筛选和排序
  - _Requirements: 2.2_

- [ ] 3.3 实现性能指标面板组件
  - 创建 `MetricsPanel` 组件
  - 显示实时性能指标 (同步速率、队列长度、活跃任务)
  - 使用迷你图表展示趋势 (Recharts)
  - _Requirements: 2.4, 4.1_

- [ ] 3.4 实现告警横幅组件
  - 创建 `AlertBanner` 组件
  - 显示告警列表和严重程度
  - 支持告警确认和跳转
  - _Requirements: 2.5, 8.1, 8.2, 8.3, 8.4_

- [ ] 3.5 实现 SSE 实时更新逻辑
  - 在监控页面建立 SSE 连接
  - 处理各类同步事件 (Started/Progress/Completed/Failed)
  - 更新组件状态和界面显示
  - 实现断线重连机制
  - _Requirements: 2.1_

- [ ] 3.6 创建监控仪表板页面
  - 创建 `app/remote-sync/monitor/page.tsx`
  - 集成所有监控组件
  - 实现环境筛选功能
  - 添加刷新按钮
  - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5_

## 4. 流向可视化功能

- [ ] 4.1 实现流向图数据获取
  - 创建流向统计 API 调用函数
  - 实现数据转换逻辑 (API 数据 → 图节点和边)
  - 支持时间范围筛选
  - _Requirements: 3.1, 3.4_

- [ ] 4.2 实现流向图组件
  - 创建 `FlowVisualization` 组件
  - 集成 React Flow 库
  - 实现节点和边的自定义渲染
  - 添加布局算法 (力导向或层次布局)
  - _Requirements: 3.1_

- [ ] 4.3 实现流向图交互功能
  - 实现鼠标悬停显示详情
  - 实现节点点击高亮相关流向
  - 支持拖拽调整布局
  - 支持缩放和平移
  - _Requirements: 3.2, 3.3_

- [ ] 4.4 实现流向图控制面板
  - 添加时间范围选择器
  - 添加环境筛选器
  - 添加布局算法切换
  - 添加导出图片功能
  - _Requirements: 3.4_

- [ ] 4.5 创建流向可视化页面
  - 创建 `app/remote-sync/flow/page.tsx`
  - 集成 FlowVisualization 组件
  - 实现异常流向标识
  - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5_

## 5. 日志查询功能

- [ ] 5.1 实现日志筛选组件
  - 创建 `LogFilters` 组件
  - 实现多维度筛选表单 (环境、站点、状态、方向、时间、关键词)
  - 添加快捷筛选按钮 (最近 1 小时/今天/本周)
  - 实现筛选条件保存和恢复
  - _Requirements: 5.1_

- [ ] 5.2 实现日志表格组件
  - 创建 `LogTable` 组件
  - 实现虚拟滚动优化
  - 显示日志关键信息 (时间、文件、状态、耗时)
  - 支持列排序
  - _Requirements: 5.2_

- [ ] 5.3 实现日志详情组件
  - 创建 `LogDetail` 抽屉组件
  - 显示完整日志信息
  - 高亮错误关键词
  - 显示重试历史
  - 提供错误解决建议
  - _Requirements: 5.3, 5.5_

- [ ] 5.4 实现日志导出功能
  - 创建 `LogExport` 组件
  - 支持导出为 CSV 格式
  - 支持导出为 JSON 格式
  - 限制单次导出不超过 10000 条
  - _Requirements: 5.4_

- [ ] 5.5 创建日志查询页面
  - 创建 `app/remote-sync/logs/page.tsx`
  - 集成日志筛选和表格组件
  - 实现分页功能
  - 添加从告警跳转的自动筛选
  - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5_

## 6. 性能监控功能

- [ ] 6.1 实现实时指标卡片组件
  - 创建 `MetricCard` 组件
  - 显示单个指标的当前值和趋势
  - 实现阈值告警状态显示
  - 添加迷你图表
  - _Requirements: 4.1, 4.3_

- [ ] 6.2 实现历史趋势图表组件
  - 创建 `TrendChart` 组件
  - 使用 Recharts 绘制折线图和面积图
  - 支持多指标叠加显示
  - 实现时间范围缩放
  - _Requirements: 4.2_

- [ ] 6.3 实现性能统计面板组件
  - 创建 `StatisticsPanel` 组件
  - 显示分位数统计 (P50/P95/P99)
  - 显示平均同步时间和成功率
  - 使用柱状图展示成功/失败统计
  - _Requirements: 4.4_

- [ ] 6.4 实现性能报告导出功能
  - 创建导出 PDF 功能 (使用 jsPDF)
  - 创建导出 CSV 功能
  - 包含图表截图和统计数据
  - _Requirements: 4.5_

- [ ] 6.5 创建性能监控页面
  - 创建 `app/remote-sync/metrics/page.tsx`
  - 集成所有性能监控组件
  - 实现环境和时间范围筛选
  - 添加自动刷新功能
  - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5_

## 7. 站点元数据浏览功能

- [ ] 7.1 实现元数据信息面板组件
  - 创建 `MetadataInfo` 组件
  - 显示站点基本信息和元数据生成时间
  - 显示元数据来源 (本地/HTTP/缓存)
  - 添加刷新按钮
  - _Requirements: 6.1, 6.2_

- [ ] 7.2 实现文件条目列表组件
  - 创建 `FileEntryList` 组件
  - 显示文件列表 (名称、大小、哈希、记录数、方向、更新时间)
  - 支持排序和筛选
  - 添加下载链接
  - _Requirements: 6.3_

- [ ] 7.3 实现文件下载功能
  - 实现通过后端代理下载文件
  - 显示下载进度条
  - 处理下载错误
  - _Requirements: 6.4_

- [ ] 7.4 实现元数据错误处理
  - 显示元数据获取失败的原因
  - 提供重试和使用缓存的选项
  - 显示警告信息
  - _Requirements: 6.5_

- [ ] 7.5 创建站点详情页面
  - 创建 `app/remote-sync/[envId]/sites/[siteId]/page.tsx`
  - 集成元数据浏览组件
  - 实现面包屑导航
  - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5_

## 8. 运维工具功能

- [ ] 8.1 实现运维工具栏组件
  - 创建 `OpsToolbar` 组件
  - 实现操作按钮列表 (启动/停止/暂停/恢复/清空队列)
  - 添加确认对话框
  - 显示操作结果反馈
  - _Requirements: 7.1, 7.2, 7.3, 7.4_

- [ ] 8.2 实现手动添加任务功能
  - 创建 `AddTaskDialog` 组件
  - 实现任务参数表单
  - 验证输入参数
  - 调用添加任务 API
  - _Requirements: 7.5_

- [ ] 8.3 实现批量操作功能
  - 支持批量取消任务
  - 支持批量重试失败任务
  - 显示批量操作进度
  - _Requirements: 7.4_

- [ ] 8.4 集成运维工具到各页面
  - 在监控页面添加运维工具栏
  - 在环境详情页添加运维工具栏
  - 实现操作权限控制
  - _Requirements: 7.1, 7.2, 7.3, 7.4, 7.5_

## 9. 告警和通知功能

- [ ] 9.1 实现告警检测逻辑
  - 在后端实现告警规则检查
  - 检测同步任务失败率
  - 检测 MQTT 连接状态
  - 检测任务队列积压
  - 生成告警事件
  - _Requirements: 8.1, 8.2, 8.3_

- [ ] 9.2 实现告警通知组件
  - 创建 `AlertNotification` 组件
  - 显示告警横幅
  - 支持告警确认
  - 提供快捷操作链接
  - _Requirements: 8.4_

- [ ] 9.3 实现告警历史记录
  - 创建告警历史表
  - 实现告警查询 API
  - 显示告警列表和详情
  - _Requirements: 8.1, 8.2, 8.3_

- [ ] 9.4 实现告警规则配置
  - 创建 `AlertRuleConfig` 组件
  - 支持自定义阈值
  - 支持启用/禁用规则
  - 保存配置到文件
  - _Requirements: 8.5_

- [ ] 9.5 实现告警通知渠道
  - 支持邮件通知
  - 支持 Webhook 通知
  - 配置通知模板
  - _Requirements: 8.5_

## 10. 配置管理功能

- [ ] 10.1 实现配置表单组件
  - 创建 `ConfigForm` 组件
  - 实现配置参数输入字段
  - 添加实时验证
  - 显示参数说明和默认值
  - _Requirements: 9.1, 9.2_

- [ ] 10.2 实现配置保存和重置功能
  - 调用更新配置 API
  - 实现重置为默认值
  - 显示保存成功/失败反馈
  - _Requirements: 9.3, 9.4_

- [ ] 10.3 实现配置历史记录
  - 记录配置变更历史
  - 显示变更时间和操作人
  - 支持配置回滚
  - _Requirements: 9.3_

- [ ] 10.4 创建配置管理页面
  - 创建 `app/remote-sync/config/page.tsx`
  - 集成配置表单组件
  - 实现配置预览
  - _Requirements: 9.1, 9.2, 9.3, 9.4, 9.5_

## 11. 多环境管理功能

- [ ] 11.1 实现环境列表组件
  - 创建 `EnvironmentList` 组件
  - 显示所有环境和状态
  - 标识当前激活环境
  - 支持环境筛选和搜索
  - _Requirements: 10.1_

- [ ] 11.2 实现环境切换功能
  - 实现切换激活环境 API 调用
  - 显示切换进度
  - 处理切换失败情况
  - _Requirements: 10.2_

- [ ] 11.3 实现环境配置比较功能
  - 创建 `ConfigComparison` 组件
  - 并排显示多个环境配置
  - 高亮配置差异
  - _Requirements: 10.3_

- [ ] 11.4 实现环境复制功能
  - 创建 `CopyEnvironmentDialog` 组件
  - 复制环境配置
  - 提示修改必要参数
  - _Requirements: 10.4_

- [ ] 11.5 实现环境删除功能
  - 显示删除确认对话框
  - 显示关联站点数量
  - 级联删除站点和日志
  - _Requirements: 10.5_

- [ ] 11.6 创建环境列表页面
  - 创建 `app/remote-sync/page.tsx`
  - 集成环境列表组件
  - 添加创建环境按钮
  - _Requirements: 10.1, 10.2, 10.3, 10.4, 10.5_

## 12. 后端 API 增强

- [ ] 12.1 实现 SSE 事件流端点
  - 创建 `/api/sync/events` 端点
  - 实现事件广播机制
  - 处理客户端连接和断开
  - _Requirements: 2.1_

- [ ] 12.2 实现流向统计 API
  - 创建 `/api/remote-sync/stats/flow` 端点
  - 查询环境和站点之间的数据流向
  - 计算流量统计 (文件数、大小、速率)
  - _Requirements: 3.1, 3.4_

- [ ] 12.3 实现每日统计 API
  - 创建 `/api/remote-sync/stats/daily` 端点
  - 按天聚合同步统计
  - 支持时间范围查询
  - _Requirements: 4.2_

- [ ] 12.4 实现性能指标 API
  - 创建 `/api/sync/metrics` 端点
  - 返回实时性能指标
  - 返回历史性能数据
  - _Requirements: 4.1, 4.2_

- [ ] 12.5 实现告警查询 API
  - 创建 `/api/alerts` 端点
  - 查询告警历史
  - 支持告警确认
  - _Requirements: 8.1, 8.2, 8.3_

- [ ] 12.6 实现配置管理 API
  - 创建 `/api/sync/config` GET/PUT 端点
  - 验证配置参数
  - 持久化配置
  - _Requirements: 9.1, 9.2, 9.3_

- [ ] 12.7 实现环境切换 API
  - 创建 `/api/remote-sync/envs/switch` 端点
  - 停止当前环境运行时
  - 启动新环境运行时
  - _Requirements: 10.2_

## 13. 测试和优化

- [ ] 13.1 编写前端单元测试
  - 测试核心组件 (DeployWizard, MonitorDashboard, LogQuery)
  - 测试 API 客户端函数
  - 测试自定义 Hooks
  - _Requirements: 所有需求_

- [ ] 13.2 编写前端集成测试
  - 测试部署流程
  - 测试监控实时更新
  - 测试日志查询和筛选
  - _Requirements: 所有需求_

- [ ] 13.3 编写后端单元测试
  - 测试 API 处理器
  - 测试同步控制中心
  - 测试事件广播
  - _Requirements: 所有需求_

- [ ] 13.4 编写后端集成测试
  - 测试完整部署流程
  - 测试同步任务处理
  - 测试 SSE 事件推送
  - _Requirements: 所有需求_

- [ ] 13.5 性能优化
  - 实现前端代码分割
  - 实现数据缓存策略
  - 实现虚拟滚动
  - 优化后端数据库查询
  - 实现连接池
  - _Requirements: 所有需求_

- [ ] 13.6 编写 E2E 测试
  - 测试完整用户流程
  - 测试跨页面交互
  - 测试错误场景
  - _Requirements: 所有需求_

## 14. 文档和部署

- [ ] 14.1 编写用户文档
  - 编写部署指南
  - 编写使用手册
  - 编写故障排查指南
  - _Requirements: 所有需求_

- [ ] 14.2 编写开发文档
  - 编写架构说明
  - 编写 API 文档
  - 编写组件文档
  - _Requirements: 所有需求_

- [ ] 14.3 配置 CI/CD
  - 配置前端构建流程
  - 配置后端编译流程
  - 配置自动化测试
  - 配置部署脚本
  - _Requirements: 所有需求_

- [ ] 14.4 准备生产部署
  - 创建 Docker 镜像
  - 配置 systemd 服务
  - 配置监控和日志
  - 配置备份策略
  - _Requirements: 所有需求_
