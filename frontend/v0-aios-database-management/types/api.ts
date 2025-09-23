export interface ApiErrorPayload {
  success?: boolean
  message?: string
  error?: string
}

export type TaskPriority = "Low" | "Normal" | "High" | "Urgent"

export type TaskType =
  | "DataGeneration"
  | "SpatialTreeGeneration"
  | "FullGeneration"
  | "MeshGeneration"
  | "ParsePdmsData"
  | "GenerateGeometry"
  | "BuildSpatialIndex"
  | "BatchDatabaseProcess"
  | "BatchGeometryGeneration"
  | "DataExport"
  | "DataImport"
  | "DataParsingWizard"

export interface DatabaseConfig {
  name: string
  manual_db_nums: number[]
  project_name: string
  project_path: string
  project_code: number
  mdb_name: string
  module: string
  db_type: string
  surreal_ns: number
  db_ip: string
  db_port: string
  db_user: string
  db_password: string
  gen_model: boolean
  gen_mesh: boolean
  gen_spatial_tree: boolean
  apply_boolean_operation: boolean
  mesh_tol_ratio: number
  room_keyword: string
  target_sesno?: number | null
  [key: string]: unknown
}

export interface DeploymentSite {
  id: string
  name?: string
  status?: string
  env?: string
  owner?: string
  description?: string
  url?: string
  health_url?: string
  project_code?: number | null
  created_at?: string
  updated_at?: string
  last_health_check?: string
  config?: DatabaseConfig
}

export interface DeploymentSiteDetail extends DeploymentSite {
  history?: Array<Record<string, unknown>>
}

export interface DeploymentSitesResponse {
  items: DeploymentSite[]
  total: number
  page: number
  per_page: number
  pages: number
}

export interface DeploymentSiteFilters {
  q?: string
  status?: string
  env?: string
  owner?: string
  sort?: string
  page?: number
  per_page?: number
}

export interface DeploymentSiteDetailResponse {
  status?: string
  data?: DeploymentSiteDetail
  error?: string
}

export interface DeploymentSiteImportRequest {
  path?: string
  name?: string
  description?: string
  env?: string
  owner?: string
  notes?: string
  health_url?: string
}

export interface DeploymentSiteImportResponse {
  status?: string
  message?: string
  error?: string
  item?: DeploymentSite
}

export interface DeploymentSiteTaskRequest {
  site_id: string
  task_type: TaskType
  task_name?: string
  priority?: TaskPriority
}

export interface DeploymentSiteTaskResponse {
  status?: string
  task_id?: string
  message?: string
  error?: string
}

export interface DeploymentSiteHealthResponse {
  status?: string
  healthy?: boolean
  item?: DeploymentSite
  error?: string
}

export interface DeploymentSiteExportResponse {
  status?: string
  name?: string
  config?: DatabaseConfig
  error?: string
}

export interface DatabaseInfo {
  db_num: number
  name: string
  record_count?: number
  last_updated?: string
  available?: boolean
  [key: string]: unknown
}

export interface ModelGeneratePayload {
  dbnum?: number
  refno?: string
}

export interface ModelGenerateResponse {
  status?: string
  message?: string
  filename?: string
  download_url?: string
  error?: string
}

export interface WizardTemplatesResponse {
  templates: Record<string, DatabaseConfig>
}

export interface WizardTaskPayload {
  task_name: string
  wizard_config: {
    base_config: DatabaseConfig
    selected_projects: string[]
    root_directory: string
    parallel_processing: boolean
    max_concurrent?: number | null
    continue_on_failure: boolean
    output_directory?: string | null
  }
  priority?: TaskPriority
  task_mode?: string
}

export interface WizardTaskResponse {
  status?: string
  task_id?: string
  message?: string
  error?: string
}
