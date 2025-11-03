# 任务创建功能设计文档

## 概述

本文档描述了在 `frontend/v0-aios-database-management` 中实现的创建解析任务和模型生成任务的功能设计。

## 功能特性

### 1. 任务类型支持
- **数据解析任务** (`DataParsingWizard`) - 解析PDMS数据库文件，提取几何和属性信息
- **模型生成任务** (`ModelGeneration`) - 基于解析数据生成3D模型和网格文件
- **空间树生成任务** (`SpatialTreeGeneration`) - 构建空间索引树，优化查询性能
- **全量同步任务** (`FullSync`) - 完整同步所有数据到目标数据库
- **增量同步任务** (`IncrementalSync`) - 仅同步变更的数据到目标数据库

### 2. 操作流程

#### 步骤1: 基础信息配置
- 任务名称（必填，支持重复性验证）
- 任务类型选择（单选）
- 优先级设置（低/普通/高/紧急）
- 任务描述（可选）

#### 步骤2: 选择部署站点
- 从可用站点列表中选择
- 显示站点状态和环境信息
- 支持站点详情查看

#### 步骤3: 任务参数配置
根据任务类型显示不同的参数配置：

**数据解析任务参数：**
- 解析模式：全部解析 / 指定数据库编号 / 指定参考号
- 数据库编号（当选择指定模式时）
- 参考号（当选择指定模式时）

**模型生成任务参数：**
- 生成选项：3D模型 / 网格 / 空间树 / 布尔运算
- 网格容差比例
- 最大并发数
- 并行处理开关

**通用参数：**
- 最大并发数
- 并行处理开关

#### 步骤4: 预览和确认
- 任务配置预览
- 资源需求预估
- 注意事项和警告
- 最终确认创建

### 3. UI组件设计

#### 主要组件
- `TaskCreationWizard` - 任务创建向导主组件
- `TaskParametersForm` - 任务参数配置组件
- `TaskPreview` - 任务预览组件
- `TaskCreationPage` - 任务创建页面

#### 页面路由
- `/task-creation` - 任务创建页面
- `/task-monitor` - 任务监控页面

#### 导航集成
在侧边栏的"任务管理"菜单下添加：
- 创建任务
- 任务监控
- 批量任务
- 定时任务

### 4. API接口设计

#### 前端API (`lib/api/task-creation.ts`)
- `fetchDeploymentSites()` - 获取部署站点列表
- `fetchTaskTemplates()` - 获取任务模板
- `createTask()` - 创建任务
- `validateTaskName()` - 验证任务名称
- `previewTaskConfig()` - 预览任务配置

#### 后端API (`src/web_ui/task_creation_handlers.rs`)
- `POST /api/tasks` - 创建任务
- `GET /api/deployment-sites` - 获取部署站点
- `GET /api/task-templates` - 获取任务模板
- `GET /api/tasks/validate-name` - 验证任务名称
- `POST /api/tasks/preview` - 预览任务配置

### 5. 数据流设计

#### 前端数据流
1. 用户选择任务类型 → 加载对应模板
2. 用户配置参数 → 实时验证和预览
3. 用户确认创建 → 提交到后端API
4. 创建成功 → 跳转到任务监控页面

#### 后端数据流
1. 接收创建请求 → 验证参数
2. 创建TaskRequest → 提交到TaskManager
3. 保存到数据库 → 返回创建结果
4. 任务进入队列 → 开始执行

### 6. 状态管理

#### 使用自定义Hook (`hooks/use-task-creation.ts`)
- 站点列表管理
- 任务模板管理
- 表单状态管理
- API调用封装
- 错误处理

#### 表单状态 (`TaskCreationFormData`)
```typescript
interface TaskCreationFormData {
  taskName: string
  taskType: TaskType
  siteId: string
  priority: TaskPriority
  description: string
  parameters: TaskParameters
}
```

### 7. 错误处理

#### 前端错误处理
- 表单验证错误
- API调用错误
- 网络连接错误
- 用户友好的错误提示

#### 后端错误处理
- 参数验证错误
- 任务创建失败
- 数据库操作错误
- 详细的错误信息返回

### 8. 用户体验优化

#### 交互优化
- 步骤式向导流程
- 实时表单验证
- 参数配置预览
- 进度指示器
- 成功/失败反馈

#### 视觉设计
- 现代化的UI组件
- 清晰的信息层次
- 直观的图标使用
- 响应式布局设计

### 9. 扩展性设计

#### 任务类型扩展
- 支持添加新的任务类型
- 模板化配置系统
- 参数验证规则可配置

#### 功能扩展
- 批量任务创建
- 任务模板管理
- 定时任务调度
- 任务依赖关系

## 使用指南

### 1. 访问任务创建页面
- 点击侧边栏"任务管理" → "创建任务"
- 或直接访问 `/task-creation`

### 2. 创建任务步骤
1. 选择任务类型模板
2. 填写基础信息
3. 选择部署站点
4. 配置任务参数
5. 预览并确认创建

### 3. 监控任务状态
- 创建成功后自动跳转到任务监控页面
- 实时查看任务进度和状态
- 支持任务操作（启动/停止/暂停）

## 技术实现

### 前端技术栈
- Next.js 14
- React 18
- TypeScript
- Tailwind CSS
- Radix UI组件库
- React Hook Form

### 后端技术栈
- Rust
- Axum Web框架
- Serde序列化
- SQLite数据库
- gRPC服务

### 开发工具
- ESLint代码检查
- Prettier代码格式化
- TypeScript类型检查
- Rust Clippy检查

## 部署说明

### 前端部署
```bash
cd frontend/v0-aios-database-management
npm install
npm run build
npm start
```

### 后端部署
```bash
cargo build --release
./target/release/gen-model
```

## 注意事项

1. **任务名称唯一性**：系统会验证任务名称的唯一性，避免重复
2. **资源预估**：创建任务前会预估资源需求，确保系统有足够资源
3. **并发控制**：支持设置最大并发数，避免系统过载
4. **错误恢复**：任务失败时支持重新启动和错误诊断
5. **数据持久化**：任务信息会保存到数据库，支持系统重启后恢复

## 未来规划

1. **批量操作**：支持批量创建和管理任务
2. **模板系统**：完善任务模板管理和自定义
3. **调度功能**：支持定时任务和任务依赖
4. **监控增强**：更详细的性能监控和告警
5. **用户权限**：基于角色的访问控制







