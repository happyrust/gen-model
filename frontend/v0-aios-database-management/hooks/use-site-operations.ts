import { createDeploymentSite, type CreateDeploymentSitePayload, buildApiUrl } from "@/lib/api"
import type { Site } from "@/components/deployment-sites/site-card"
import type { CreateSiteFormData, ProjectInfo } from "./use-create-site-form"
import { formatBytes, normalizeDate, normalizeStatus, extractErrorMessage } from "@/components/deployment-sites/site-form-utils"

export async function scanDirectory(
  rootDirectory: string,
  setProjects: (projects: ProjectInfo[]) => void
): Promise<void> {
  const params = new URLSearchParams({
    directory_path: rootDirectory,
    recursive: "true",
    max_depth: "4",
  })

  const requestUrl = buildApiUrl(`/api/wizard/scan-directory?${params.toString()}`)

  const response = await fetch(requestUrl)

  if (!response.ok) {
    const message = await extractErrorMessage(response)
    throw new Error(message || "扫描目录失败")
  }

  const data = await response.json()

  if (data.projects && Array.isArray(data.projects)) {
    const mappedProjects = data.projects.map((p: any) => ({
      name: p.name || "未知项目",
      path: p.path || "",
      selected: false,
      fileCount: p.db_file_count,
      size: formatBytes(p.size_bytes || 0),
    }))
    setProjects(mappedProjects)
  }
}

export async function submitCreateSite(formData: CreateSiteFormData): Promise<Site> {
  const payload: CreateDeploymentSitePayload = {
    name: formData.name,
    description: formData.description || undefined,
    root_directory: formData.rootDirectory || null,
    selected_projects: formData.selectedProjects,
    config: {
      ...formData.config,
      name: `${formData.name} 配置`,
      project_name: formData.name,
      project_path: formData.rootDirectory,
    },
    env: formData.environment,
    owner: formData.owner || null,
    tags: Object.keys(formData.tags).length > 0 ? formData.tags : null,
    notes: formData.notes || undefined,
  }

  const response = await createDeploymentSite(payload)
  const item = response.item ?? {}

  return {
    id: typeof item.id === "string" ? item.id : `site-${Date.now()}`,
    name: (item.name as string) || formData.name,
    status: normalizeStatus(item.status),
    environment: formData.environment as Site["environment"],
    owner: formData.owner || undefined,
    createdAt: normalizeDate(item.created_at),
    updatedAt: normalizeDate(item.updated_at),
    url: typeof item.url === "string" ? item.url : undefined,
    description: formData.description || undefined,
  }
}