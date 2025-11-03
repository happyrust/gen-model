import { useState, useCallback } from 'react'
import { 
  fetchDeploymentSites, 
  fetchTaskTemplates, 
  createTask, 
  validateTaskName,
  previewTaskConfig 
} from '@/lib/api/task-creation'
import type { 
  TaskCreationFormData, 
  DeploymentSite, 
  TaskTemplate,
  TaskCreationRequest 
} from '@/types/task-creation'

export function useTaskCreation() {
  const [sites, setSites] = useState<DeploymentSite[]>([])
  const [templates, setTemplates] = useState<TaskTemplate[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // 加载部署站点
  const loadSites = useCallback(async () => {
    try {
      setLoading(true)
      setError(null)
      const sitesData = await fetchDeploymentSites()
      setSites(sitesData)
    } catch (err) {
      setError(err instanceof Error ? err.message : '加载站点失败')
    } finally {
      setLoading(false)
    }
  }, [])

  // 加载任务模板
  const loadTemplates = useCallback(async () => {
    try {
      setLoading(true)
      setError(null)
      const templatesData = await fetchTaskTemplates()
      setTemplates(templatesData)
    } catch (err) {
      setError(err instanceof Error ? err.message : '加载模板失败')
    } finally {
      setLoading(false)
    }
  }, [])

  // 验证任务名称
  const validateName = useCallback(async (taskName: string) => {
    if (!taskName.trim()) {
      return { available: false, message: '任务名称不能为空' }
    }
    
    try {
      return await validateTaskName(taskName)
    } catch (err) {
      return { available: false, message: '验证失败' }
    }
  }, [])

  // 预览任务配置
  const previewConfig = useCallback(async (formData: Partial<TaskCreationFormData>) => {
    try {
      const request: Partial<TaskCreationRequest> = {
        taskName: formData.taskName,
        taskType: formData.taskType,
        siteId: formData.siteId,
        priority: formData.priority,
        description: formData.description,
        parameters: formData.parameters
      }
      return await previewTaskConfig(request)
    } catch (err) {
      throw new Error(err instanceof Error ? err.message : '预览失败')
    }
  }, [])

  // 创建任务
  const submitTask = useCallback(async (formData: TaskCreationFormData) => {
    try {
      setLoading(true)
      setError(null)
      
      const request: TaskCreationRequest = {
        taskName: formData.taskName,
        taskType: formData.taskType,
        siteId: formData.siteId,
        priority: formData.priority,
        description: formData.description,
        parameters: formData.parameters
      }
      
      const result = await createTask(request)
      
      if (!result.success) {
        throw new Error(result.error || '创建任务失败')
      }
      
      return result
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : '创建任务失败'
      setError(errorMessage)
      throw new Error(errorMessage)
    } finally {
      setLoading(false)
    }
  }, [])

  return {
    sites,
    templates,
    loading,
    error,
    loadSites,
    loadTemplates,
    validateName,
    previewConfig,
    submitTask,
    setError
  }
}







