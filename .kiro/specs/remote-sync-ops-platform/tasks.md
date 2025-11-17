# Implementation Plan

## 完整实现：远程同步运维平台

- [x] 1. 实现完整的远程同步运维平台（包含所有功能模块）
  - **CBA 文件分发服务**：在 `src/web_server/mod.rs` 添加 `/assets/archives` 路由（ServeDir）、验证目录可访问、在元数据生成中构建完整 download_url、测试 HTTP 下载功能
  - **可视化拓扑配置 - 后端**：创建 `topology_handlers.rs`、实现 TopologyData 结构体和验证逻辑、实现拓扑 CRUD API（GET/POST/PUT/DELETE `/api/remote-sync/topology`）、在 mod.rs 注册路由
  - **可视化拓扑配置 - 前端**：创建 `app/remote-sync/topology/page.tsx`、实现 TopologyCanvas 组件（React Flow）、实现环境节点和站点节点组件、实现同步连线组件、实现拖拽/缩放/平移、实现层次布局算法、实现节点配置面板（侧边栏）、实现工具栏（添加节点/连线/布局/导入导出）、实现保存/加载/删除功能
  - **流向可视化增强**：实现后端流向统计 API（`/api/remote-sync/stats/flows`）、创建 FlowVisualization 组件（React Flow）、实现力导向图布局、实现节点和流向连线渲染、实现悬停显示详情、实现点击高亮、实现时间范围筛选、实现异常流向标识、更新 `app/remote-sync/flow/page.tsx`
  - **日志查询功能**：创建 LogFilters 筛选组件、创建 LogTable 虚拟滚动表格（@tanstack/react-virtual）、创建 LogDetail 抽屉组件、实现日志导出（CSV/JSON，限制 10000 条）、实现错误关键词高亮、实现错误码解释、更新 `app/remote-sync/logs/page.tsx`、确保 2 秒内返回结果
  - **性能监控功能**：实现后端性能指标 API（`/api/sync/metrics` 和 `/api/sync/metrics/history`）、创建 MetricCard 指标卡片、创建 TrendChart 趋势图（Recharts）、创建 StatisticsPanel 统计面板（P50/P95/P99）、实现阈值警告显示、实现性能报告导出（PDF/CSV）、更新 `app/remote-sync/metrics/page.tsx`
  - **站点元数据浏览**：创建 MetadataInfo 组件、创建 FileEntryList 组件、实现文件下载功能（通过 download_url，显示进度条）、实现刷新元数据功能（refresh=true）、实现错误处理和重试选项、更新 `app/remote-sync/[envId]/sites/[siteId]/page.tsx`
  - **运维工具功能**：创建 OpsToolbar 组件、实现启动/停止/暂停/恢复/清空队列按钮（带确认对话框）、创建 AddTaskDialog 手动添加任务对话框、实现批量操作功能、集成到监控/日志/性能等页面
  - **告警和通知功能**：实现后端告警检测逻辑（失败率/MQTT 状态/队列积压）、实现告警事件广播（SSE）、创建 AlertNotification 组件、实现告警历史记录、实现告警规则配置、实现通知渠道（界面/邮件/Webhook）、实现点击告警跳转、创建 `app/remote-sync/alerts/page.tsx`
  - **配置管理功能**：实现后端配置管理 API（`/api/sync/config` GET/PUT）、创建 ConfigForm 组件、实现实时参数验证、实现配置保存和重置功能、实现配置历史记录、更新 `app/remote-sync/config/page.tsx`
  - **多环境管理功能**：创建 EnvironmentList 组件、实现环境切换功能、创建 EnvironmentCompare 配置比较组件、实现环境复制功能、实现环境删除功能（级联删除）、更新 `app/remote-sync/page.tsx`
  - **测试**（可选）：编写前端单元测试（DeployWizard/TopologyCanvas/LogFilters/MetricsPanel）、编写前端集成测试（部署/拓扑/日志流程）、编写后端单元测试（拓扑验证/告警检测/配置验证）、编写后端集成测试（拓扑/告警/配置 API）、编写 E2E 测试（完整流程）
  - **性能优化**（可选）：实现前端代码分割、配置 React Query 缓存策略、实现虚拟滚动、优化数据库查询（索引/连接池）、实现批量操作、实现异步任务队列
  - **文档**（可选）：编写用户文档（部署指南/使用手册/拓扑配置指南/故障排查）、编写开发文档（架构/API/组件/拓扑数据模型）、配置 CI/CD、准备生产部署配置（Docker/systemd/监控/备份）、确保 assets/archives 目录映射
  - _Requirements: 1.1-12.5, 所有需求_
