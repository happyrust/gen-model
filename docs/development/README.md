# 开发文档

## 概述

本目录包含 gen-model 项目的开发文档，详细描述了系统的架构设计、工作流程和实现细节。

## 文档目录

### 核心功能文档

1. **[任务创建工作流程](./task-creation-workflow.md)**
   - 完整的任务创建流程
   - API 接口说明
   - 数据结构定义
   - 错误处理机制

2. **[任务创建时序图](./task-creation-sequence.md)**
   - 详细的交互时序
   - 状态转换流程
   - 并发控制机制
   - 错误处理流程图

3. **[异步同步控制系统](./sync-control-system.md)**
   - 同步控制中心架构
   - MQTT 集成
   - 实时监控功能
   - 任务队列管理

### 架构设计文档

4. **[系统架构概述](./architecture-overview.md)** *(待创建)*
   - 整体架构图
   - 模块划分
   - 数据流向
   - 技术栈说明

5. **[数据库设计](./database-design.md)** *(待创建)*
   - 表结构设计
   - 索引策略
   - 数据关系
   - 迁移脚本

### API 文档

6. **[REST API 参考](./api-reference.md)** *(待创建)*
   - 完整的 API 列表
   - 请求/响应格式
   - 认证机制
   - 错误码说明

7. **[WebSocket/SSE 接口](./realtime-api.md)** *(待创建)*
   - 实时通信协议
   - 事件类型
   - 订阅机制
   - 心跳保持

### 部署与运维

8. **[部署指南](./deployment-guide.md)** *(待创建)*
   - 环境要求
   - 安装步骤
   - 配置说明
   - 性能调优

9. **[监控与告警](./monitoring.md)** *(待创建)*
   - 监控指标
   - 日志管理
   - 告警配置
   - 故障排查

### 开发指南

10. **[开发环境搭建](./development-setup.md)** *(待创建)*
    - 工具链安装
    - 依赖配置
    - IDE 设置
    - 调试技巧

11. **[代码规范](./coding-standards.md)** *(待创建)*
    - 命名规范
    - 代码格式
    - 注释要求
    - Git 工作流

12. **[测试指南](./testing-guide.md)** *(待创建)*
    - 单元测试
    - 集成测试
    - 性能测试
    - 测试覆盖率

## 快速导航

### 常见任务

- **创建新任务**：查看 [任务创建工作流程](./task-creation-workflow.md#2-任务创建流程)
- **查询任务状态**：参考 [API 接口](./task-creation-workflow.md#5-api-接口)
- **处理错误**：查看 [错误处理](./task-creation-workflow.md#6-错误处理)
- **配置数据库**：参考 [数据库配置](./task-creation-workflow.md#4-数据库配置)

### 核心概念

- **TaskInfo**: 任务信息对象，包含任务的所有元数据
- **TaskManager**: 任务管理器，负责任务的调度和执行
- **DatabaseConfig**: 数据库配置，定义数据处理参数
- **WizardConfig**: 向导配置，包含批量任务的设置

### 系统组件

```
gen-model/
├── src/
│   ├── web_server/           # Web UI 模块
│   │   ├── handlers.rs   # HTTP 处理器
│   │   ├── models.rs     # 数据模型
│   │   ├── wizard_*.rs   # 向导相关
│   │   └── sync_*.rs     # 同步控制
│   ├── fast_model/        # 核心建模引擎
│   └── data_interface/    # 数据接口层
├── docs/
│   └── development/       # 开发文档
├── rumqttd-server/        # MQTT 服务器
└── deployment_sites.sqlite  # 任务数据库
```

## 更新日志

| 日期 | 版本 | 更新内容 |
|-----|------|---------|
| 2024-01-19 | 1.0.0 | 初始版本，添加任务创建文档 |
| 2024-01-19 | 1.0.1 | 添加时序图文档 |

## 贡献指南

欢迎贡献文档！请遵循以下规范：

1. 使用 Markdown 格式
2. 包含目录结构
3. 提供代码示例
4. 添加图表说明（推荐使用 Mermaid）
5. 保持版本更新

## 联系方式

- 项目负责人：[负责人姓名]
- 技术支持：[support@example.com]
- 问题反馈：[GitHub Issues]

---

*最后更新：2024-01-19*