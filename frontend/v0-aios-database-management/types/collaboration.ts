export type CollaborationGroupType = "ConfigSharing" | "DataSync" | "TaskCoordination" | "Hybrid"

export type SyncMode = "OneWay" | "TwoWay" | "Manual"

export type ConflictResolution = "PrimaryWins" | "LatestWins" | "Manual"

export type CollaborationGroupStatus = "Active" | "Syncing" | "Paused" | "Error" | "Inactive" | "Pending"

export type ConnectionStatus = "Connected" | "Disconnected" | "Connecting" | "Failed" | "Online" | "Offline"

export type SyncType = "Config" | "FullData" | "IncrementalData"

export type SyncStatus = "InProgress" | "Success" | "Failed" | "PartialSuccess"

export interface SyncStrategy {
  mode: SyncMode
  interval_seconds: number
  auto_sync: boolean
  conflict_resolution: ConflictResolution
}

export interface CollaborationGroup {
  id: string
  name: string
  description?: string
  group_type: CollaborationGroupType
  site_ids: string[]
  primary_site_id?: string
  shared_config?: any
  sync_strategy: SyncStrategy
  status: CollaborationGroupStatus
  creator: string
  created_at: string
  updated_at: string
  location?: string  // 添加 location 字段
  tags?: Record<string, any>
}

export interface RemoteSite {
  id: string
  name: string
  api_url?: string
  auth_token?: string
  last_connected?: string
  connection_status?: ConnectionStatus
  latency_ms?: number
  location?: string
  ip_address?: string
  status?: ConnectionStatus
  last_sync?: string
  data_version?: string
  is_local?: boolean
}

export interface SyncRecord {
  id: string
  group_id: string
  source_site_id: string
  target_site_id: string
  sync_type: SyncType
  status: SyncStatus
  started_at: string
  completed_at?: string
  error_message?: string
  data_size?: number
}

export interface RemoteSyncLogEntry {
  id: string
  task_id?: string
  env_id?: string
  source_env?: string
  target_site?: string
  site_id?: string
  direction?: string
  file_path?: string
  file_size?: number
  record_count?: number
  status: string
  error_message?: string
  notes?: string
  started_at?: string
  completed_at?: string
  created_at: string
  updated_at: string
}

export interface RemoteSyncLogResponse {
  items: RemoteSyncLogEntry[]
  total: number
  limit: number
  offset: number
}

export interface RemoteSyncLogQuery {
  envId?: string
  targetSite?: string
  status?: string
  direction?: string
  limit?: number
  offset?: number
}

export interface RemoteSyncDailyStat {
  day: string
  total: number
  completed: number
  failed: number
  record_count: number
  total_bytes: number
}

export interface RemoteSyncDailyStatsResponse {
  items: RemoteSyncDailyStat[]
}

export interface RemoteSyncFlowStat {
  env_id: string
  target_site: string
  direction: string
  total: number
  completed: number
  failed: number
  record_count: number
  total_bytes: number
}

export interface RemoteSyncFlowStatsResponse {
  items: RemoteSyncFlowStat[]
}

export interface SiteMetadataEntry {
  file_name: string
  file_path: string
  file_size: number
  file_hash?: string | null
  record_count?: number | null
  direction?: string | null
  source_env?: string | null
  download_url?: string | null
  relative_path?: string | null
  updated_at: string
}

export interface SiteMetadataFile {
  env_id?: string | null
  env_name?: string | null
  site_id?: string | null
  site_name?: string | null
  site_http_host?: string | null
  generated_at: string
  entries: SiteMetadataEntry[]
}

export interface SiteMetadataResponse {
  status: string
  source: string
  fetched_at: string
  entry_count: number
  cache_path?: string | null
  http_base?: string | null
  local_base?: string | null
  warnings?: string[]
  env?: {
    id?: string
    name?: string | null
    file_host?: string | null
  }
  site?: {
    id?: string
    name?: string
    host?: string | null
  }
  metadata: SiteMetadataFile
}

export interface CreateCollaborationGroupPayload {
  name: string
  description?: string
  group_type: CollaborationGroupType
  site_ids: string[]
  primary_site_id?: string
  sync_strategy: SyncStrategy
  creator: string
  tags?: Record<string, any>
}

export interface UpdateCollaborationGroupPayload {
  name?: string
  description?: string
  site_ids?: string[]
  primary_site_id?: string
  sync_strategy?: SyncStrategy
  status?: CollaborationGroupStatus
  tags?: Record<string, any>
  shared_config?: any
}

export interface CreateRemoteSitePayload {
  name: string
  api_url: string
  auth_token?: string
  metadata?: Record<string, any>
}

export interface SyncOptions {
  force?: boolean
  dry_run?: boolean
}
