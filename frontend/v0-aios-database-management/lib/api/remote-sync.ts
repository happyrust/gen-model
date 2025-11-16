// 异地协同运维平台 API 客户端

import type {
  Environment,
  Site,
  SyncTask,
  SyncLog,
  Metrics,
  SyncConfig,
  MetadataResponse,
  FlowStatistics,
  DailyStatistics,
  ApiResponse,
  LogQueryParams,
  StatsQueryParams,
} from '@/types/remote-sync'

const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL || 'http://localhost:8080'

/**
 * API 错误类
 */
export class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
    public data?: any
  ) {
    super(message)
    this.name = 'ApiError'
  }
}

/**
 * 网络错误类
 */
export class NetworkError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'NetworkError'
  }
}

/**
 * 通用 API 调用函数
 */
async function apiCall<T>(
  url: string,
  options?: RequestInit
): Promise<T> {
  try {
    const response = await fetch(`${API_BASE}${url}`, {
      ...options,
      headers: {
        'Content-Type': 'application/json',
        ...options?.headers,
      },
    })
    
    if (!response.ok) {
      const error = await response.json().catch(() => ({}))
      throw new ApiError(
        error.message || `API 调用失败: ${response.statusText}`,
        response.status,
        error
      )
    }
    
    return await response.json()
  } catch (error) {
    if (error instanceof ApiError) {
      throw error
    }
    if (error instanceof TypeError) {
      throw new NetworkError('网络连接失败，请检查网络设置')
    }
    throw error
  }
}

// ==================== 环境管理 API ====================

/**
 * 获取环境列表
 */
export async function listEnvironments(): Promise<Environment[]> {
  const response = await apiCall<ApiResponse<Environment>>('/api/remote-sync/envs')
  return response.items || []
}

/**
 * 获取单个环境
 */
export async function getEnvironment(envId: string): Promise<Environment> {
  const response = await apiCall<ApiResponse<Environment>>(`/api/remote-sync/envs/${envId}`)
  return response.item!
}

/**
 * 创建环境
 */
export async function createEnvironment(data: Partial<Environment>): Promise<string> {
  const response = await apiCall<ApiResponse<{ id: string }>>('/api/remote-sync/envs', {
    method: 'POST',
    body: JSON.stringify(data),
  })
  return response.item!.id
}

/**
 * 更新环境
 */
export async function updateEnvironment(envId: string, data: Partial<Environment>): Promise<void> {
  await apiCall(`/api/remote-sync/envs/${envId}`, {
    method: 'PUT',
    body: JSON.stringify(data),
  })
}

/**
 * 删除环境
 */
export async function deleteEnvironment(envId: string): Promise<void> {
  await apiCall(`/api/remote-sync/envs/${envId}`, {
    method: 'DELETE',
  })
}

/**
 * 激活环境
 */
export async function activateEnvironment(envId: string): Promise<void> {
  await apiCall(`/api/remote-sync/envs/${envId}/activate`, {
    method: 'POST',
  })
}

/**
 * 应用环境配置
 */
export async function applyEnvironmentConfig(envId: string): Promise<void> {
  await apiCall(`/api/remote-sync/envs/${envId}/apply`, {
    method: 'POST',
  })
}

// ==================== 站点管理 API ====================

/**
 * 获取环境下的站点列表
 */
export async function listSites(envId: string): Promise<Site[]> {
  const response = await apiCall<ApiResponse<Site>>(`/api/remote-sync/envs/${envId}/sites`)
  return response.items || []
}

/**
 * 获取单个站点
 */
export async function getSite(siteId: string): Promise<Site> {
  const response = await apiCall<ApiResponse<Site>>(`/api/remote-sync/sites/${siteId}`)
  return response.item!
}

/**
 * 创建站点
 */
export async function createSite(envId: string, data: Partial<Site>): Promise<string> {
  const response = await apiCall<ApiResponse<{ id: string }>>(`/api/remote-sync/envs/${envId}/sites`, {
    method: 'POST',
    body: JSON.stringify(data),
  })
  return response.item!.id
}

/**
 * 更新站点
 */
export async function updateSite(siteId: string, data: Partial<Site>): Promise<void> {
  await apiCall(`/api/remote-sync/sites/${siteId}`, {
    method: 'PUT',
    body: JSON.stringify(data),
  })
}

/**
 * 删除站点
 */
export async function deleteSite(siteId: string): Promise<void> {
  await apiCall(`/api/remote-sync/sites/${siteId}`, {
    method: 'DELETE',
  })
}

// ==================== 同步控制 API ====================

/**
 * 启动同步服务
 */
export async function startSync(envId: string): Promise<void> {
  await apiCall('/api/sync/start', {
    method: 'POST',
    body: JSON.stringify({ env_id: envId }),
  })
}

/**
 * 停止同步服务
 */
export async function stopSync(): Promise<void> {
  await apiCall('/api/sync/stop', {
    method: 'POST',
  })
}

/**
 * 重启同步服务
 */
export async function restartSync(): Promise<void> {
  await apiCall('/api/sync/restart', {
    method: 'POST',
  })
}

/**
 * 暂停同步
 */
export async function pauseSync(): Promise<void> {
  await apiCall('/api/sync/pause', {
    method: 'POST',
  })
}

/**
 * 恢复同步
 */
export async function resumeSync(): Promise<void> {
  await apiCall('/api/sync/resume', {
    method: 'POST',
  })
}

/**
 * 获取同步状态
 */
export async function getSyncStatus(): Promise<any> {
  return await apiCall('/api/sync/status')
}

/**
 * 获取性能指标
 */
export async function getMetrics(): Promise<Metrics> {
  const response = await apiCall<ApiResponse<{ metrics: Metrics }>>('/api/sync/metrics')
  return response.item!.metrics
}

/**
 * 获取任务队列
 */
export async function getTaskQueue(): Promise<SyncTask[]> {
  const response = await apiCall<ApiResponse<SyncTask>>('/api/sync/queue')
  return response.items || []
}

/**
 * 清空任务队列
 */
export async function clearQueue(): Promise<void> {
  await apiCall('/api/sync/queue/clear', {
    method: 'POST',
  })
}

/**
 * 添加同步任务
 */
export async function addSyncTask(data: Partial<SyncTask>): Promise<string> {
  const response = await apiCall<ApiResponse<{ task_id: string }>>('/api/sync/task', {
    method: 'POST',
    body: JSON.stringify(data),
  })
  return response.item!.task_id
}

/**
 * 取消同步任务
 */
export async function cancelTask(taskId: string): Promise<void> {
  await apiCall(`/api/sync/task/${taskId}/cancel`, {
    method: 'POST',
  })
}

// ==================== 配置管理 API ====================

/**
 * 获取同步配置
 */
export async function getSyncConfig(): Promise<SyncConfig> {
  const response = await apiCall<ApiResponse<SyncConfig>>('/api/sync/config')
  return response.item!
}

/**
 * 更新同步配置
 */
export async function updateSyncConfig(config: Partial<SyncConfig>): Promise<void> {
  await apiCall('/api/sync/config', {
    method: 'PUT',
    body: JSON.stringify(config),
  })
}

// ==================== 日志和统计 API ====================

/**
 * 查询同步日志
 */
export async function queryLogs(params: LogQueryParams): Promise<{ logs: SyncLog[]; total: number }> {
  const queryString = new URLSearchParams(
    Object.entries(params).filter(([_, v]) => v !== undefined) as [string, string][]
  ).toString()
  
  const response = await apiCall<ApiResponse<SyncLog>>(`/api/remote-sync/logs?${queryString}`)
  return {
    logs: response.items || [],
    total: response.total || 0,
  }
}

/**
 * 获取每日统计
 */
export async function getDailyStats(params: StatsQueryParams): Promise<DailyStatistics[]> {
  const queryString = new URLSearchParams(
    Object.entries(params).filter(([_, v]) => v !== undefined) as [string, string][]
  ).toString()
  
  const response = await apiCall<ApiResponse<DailyStatistics>>(`/api/remote-sync/stats/daily?${queryString}`)
  return response.items || []
}

/**
 * 获取流向统计
 */
export async function getFlowStats(params: StatsQueryParams): Promise<FlowStatistics[]> {
  const queryString = new URLSearchParams(
    Object.entries(params).filter(([_, v]) => v !== undefined) as [string, string][]
  ).toString()
  
  const response = await apiCall<ApiResponse<FlowStatistics>>(`/api/remote-sync/stats/flow?${queryString}`)
  return response.items || []
}

// ==================== 元数据 API ====================

/**
 * 获取站点元数据
 */
export async function getSiteMetadata(
  siteId: string,
  options?: { refresh?: boolean; cacheOnly?: boolean }
): Promise<MetadataResponse> {
  const params: Record<string, string> = {}
  if (options?.refresh !== undefined) {
    params.refresh = String(options.refresh)
  }
  if (options?.cacheOnly !== undefined) {
    params.cache_only = String(options.cacheOnly)
  }
  
  const queryString = new URLSearchParams(params).toString()
  const url = queryString 
    ? `/api/remote-sync/sites/${siteId}/metadata?${queryString}`
    : `/api/remote-sync/sites/${siteId}/metadata`
  
  return await apiCall<MetadataResponse>(url)
}

// ==================== 运行时控制 API ====================

/**
 * 停止运行时
 */
export async function stopRuntime(): Promise<void> {
  await apiCall('/api/remote-sync/runtime/stop', {
    method: 'POST',
  })
}

/**
 * 获取运行时状态
 */
export async function getRuntimeStatus(): Promise<any> {
  return await apiCall('/api/remote-sync/runtime/status')
}

/**
 * 测试连接
 */
export async function testConnection(data: { mqtt_host: string; mqtt_port: number }): Promise<any> {
  return await apiCall('/api/sync/test', {
    method: 'POST',
    body: JSON.stringify(data),
  })
}
