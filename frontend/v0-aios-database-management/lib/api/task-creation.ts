import { buildApiUrl, handleResponse } from "../api"
import type { 
  TaskCreationRequest, 
  TaskCreationResponse, 
  DeploymentSite, 
  TaskTemplate 
} from "@/types/task-creation"

// 获取可用的部署站点列表
export async function fetchDeploymentSites(): Promise<DeploymentSite[]> {
  const response = await fetch(buildApiUrl('/api/deployment-sites'), {
    method: 'GET',
    headers: {
      'Accept': 'application/json',
    },
  })

  const data = await handleResponse<{ items: any[] }>(response)
  
  // 转换数据格式以匹配我们的接口
  return data.items.map((item: any) => ({
    id: item.id,
    name: item.name,
    status: item.status || 'unknown',
    environment: item.env || 'unknown',
    description: item.description,
    config: item.config
  }))
}

// 获取任务模板
export async function fetchTaskTemplates(): Promise<TaskTemplate[]> {
  const response = await fetch(buildApiUrl('/api/task-templates'), {
    method: 'GET',
    headers: {
      'Accept': 'application/json',
    },
  })

  return handleResponse<TaskTemplate[]>(response)
}

// 创建任务
export async function createTask(request: TaskCreationRequest): Promise<TaskCreationResponse> {
  const response = await fetch(buildApiUrl('/api/task-creation'), {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  })

  return handleResponse<TaskCreationResponse>(response)
}

// 验证任务名称是否可用
export async function validateTaskName(taskName: string): Promise<{ available: boolean; message?: string }> {
  const response = await fetch(buildApiUrl(`/api/task-creation/validate-name?name=${encodeURIComponent(taskName)}`), {
    method: 'GET',
    headers: {
      'Accept': 'application/json',
    },
  })

  return handleResponse<{ available: boolean; message?: string }>(response)
}

// 获取站点详情
export async function fetchSiteDetails(siteId: string): Promise<DeploymentSite> {
  const response = await fetch(buildApiUrl(`/api/deployment-sites/${siteId}`), {
    method: 'GET',
    headers: {
      'Accept': 'application/json',
    },
  })

  return handleResponse<DeploymentSite>(response)
}

// 预览任务配置
export async function previewTaskConfig(request: Partial<TaskCreationRequest>): Promise<{
  estimatedDuration: number
  resourceRequirements: {
    memory: string
    cpu: string
    disk: string
  }
  warnings: string[]
}> {
  const response = await fetch(buildApiUrl('/api/task-creation/preview'), {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  })

  return handleResponse<{
    estimatedDuration: number
    resourceRequirements: {
      memory: string
      cpu: string
      disk: string
    }
    warnings: string[]
  }>(response)
}
