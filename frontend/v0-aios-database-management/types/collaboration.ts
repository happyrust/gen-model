export type CollaborationGroupType = "ConfigSharing" | "DataSync" | "TaskCoordination" | "Hybrid"

export type SyncMode = "OneWay" | "TwoWay" | "Manual"

export type ConflictResolution = "PrimaryWins" | "LatestWins" | "Manual"

export type CollaborationGroupStatus = "Active" | "Syncing" | "Paused" | "Error"

export type ConnectionStatus = "Connected" | "Disconnected" | "Connecting" | "Failed"

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
  tags?: Record<string, any>
}

export interface RemoteSite {
  id: string
  name: string
  api_url: string
  auth_token?: string
  last_connected?: string
  connection_status: ConnectionStatus
  latency_ms?: number
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
}

export interface CreateRemoteSitePayload {
  name: string
  api_url: string
  auth_token?: string
}

export interface SyncOptions {
  force?: boolean
  dry_run?: boolean
}