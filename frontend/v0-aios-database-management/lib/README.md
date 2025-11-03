# API 模块使用说明

## 概述

API 模块已按功能拆分为多个文件，每个文件都严格遵循 200 行代码的架构规范。

## 文件结构

### 1. `api.ts` - 核心部署站点 API
核心的部署站点管理功能。

**导出的接口：**
- `DeploymentSiteConfigPayload`
- `CreateDeploymentSitePayload`
- `DeploymentSiteListParams`
- `CreateDeploymentSiteResponse`
- `DeploymentSiteListResponse`

**导出的函数：**
- `createDeploymentSite(payload)` - 创建部署站点
- `fetchDeploymentSites(params)` - 获取部署站点列表
- `fetchDeploymentSite(siteId)` - 获取单个站点详情
- `patchDeploymentSite(siteId, payload)` - 更新站点信息
- `deleteDeploymentSite(siteId)` - 删除站点

### 2. `database-status.ts` - 数据库状态管理
数据库的启动、停止和状态查询功能。

**导出的接口：**
- `DatabaseStatusResponse`
- `StartDatabasePayload`
- `StopDatabasePayload`

**导出的函数：**
- `fetchDatabaseStatus(ip, port)` - 获取数据库状态
- `startDatabase(payload)` - 启动数据库
- `stopDatabase(ip, port)` - 停止数据库

### 3. `parsing-apis.ts` - 解析任务管理
解析任务的状态查询和列表管理。

**导出的接口：**
- `ParsingTask`
- `ParsingTaskListParams`
- `ParsingTaskListResponse`
- `TaskStatusResponse`

**导出的函数：**
- `fetchParsingTasks(siteId, params?)` - 获取解析任务列表
- `fetchParsingTaskStatus(siteId)` - 获取解析任务状态概览

### 4. `model-generation-apis.ts` - 模型生成任务管理
模型生成任务和任务详情管理。

**导出的接口：**
- `ModelGenerationTask`
- `ModelGenerationTaskListParams`
- `ModelGenerationTaskListResponse`
- `TaskDetail`

**导出的函数：**
- `fetchModelGenerationTasks(siteId, params?)` - 获取模型生成任务列表
- `fetchModelGenerationTaskStatus(siteId)` - 获取模型生成任务状态概览
- `fetchTaskDetail(taskId, taskType)` - 获取任务详情
- `retryTask(taskId, taskType)` - 重新执行任务

## 使用示例

### 在组件中导入和使用

```typescript
// 导入核心 API
import {
  fetchDeploymentSite,
} from "@/lib/api"

// 导入数据库状态 API
import {
  fetchDatabaseStatus,
  startDatabase,
  stopDatabase,
} from "@/lib/database-status"

// 导入解析任务 API
import {
  fetchParsingTasks,
  fetchParsingTaskStatus,
} from "@/lib/parsing-apis"

// 导入模型生成任务 API
import {
  fetchModelGenerationTasks,
  fetchModelGenerationTaskStatus,
  fetchTaskDetail,
  retryTask,
} from "@/lib/model-generation-apis"

// 使用示例
async function loadSiteDetails(siteId: string) {
  try {
    // 获取站点详情
    const siteResponse = await fetchDeploymentSite(siteId)

    // 获取数据库状态
    const dbStatus = await fetchDatabaseStatus(
      siteResponse.item.config.db_ip,
      siteResponse.item.config.db_port
    )

    // 获取解析任务状态
    const parsingStatus = await fetchParsingTaskStatus(siteId)

    // 获取模型生成任务状态
    const modelGenStatus = await fetchModelGenerationTaskStatus(siteId)

    // 获取解析任务列表
    const parsingTasks = await fetchParsingTasks(siteId, {
      page: 1,
      per_page: 10,
      status: "running"
    })

    // 启动数据库
    await startDatabase({
      ip: siteResponse.item.config.db_ip,
      port: parseInt(siteResponse.item.config.db_port),
      user: siteResponse.item.config.db_user,
      password: siteResponse.item.config.db_password,
      dbFile: "surreal.db"
    })

    // 停止数据库
    await stopDatabase(
      siteResponse.item.config.db_ip,
      siteResponse.item.config.db_port
    )

    // 获取任务详情
    const taskDetail = await fetchTaskDetail("task-123", "parsing")

    // 重新执行任务
    const retryResult = await retryTask("task-123", "parsing")
  } catch (error) {
    console.error("操作失败:", error)
  }
}
```

## 架构规范检查

所有 API 文件都严格遵循以下规范：

1. **文件长度**：每个文件不超过 200 行
2. **功能分离**：按功能领域拆分为独立文件
3. **类型安全**：所有接口和函数都使用 TypeScript 严格类型定义
4. **错误处理**：使用统一的 `handleResponse` 函数进行错误处理
5. **响应格式**：保持一致的响应数据结构

## 当前文件统计

- `api.ts`: 164 行
- `database-status.ts`: 64 行
- `parsing-apis.ts`: 83 行
- `model-generation-apis.ts`: 131 行

所有文件都符合 200 行的架构规范要求。
