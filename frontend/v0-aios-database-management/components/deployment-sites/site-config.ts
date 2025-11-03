import type { DeploymentSiteConfigPayload } from "@/lib/api"

export const DEFAULT_CONFIG: DeploymentSiteConfigPayload = {
  name: "默认配置",
  manual_db_nums: [],
  project_name: "AvevaMarineSample",
  project_path: "/Volumes/DPC/work/e3d_models",
  project_code: 1516,
  mdb_name: "ALL",
  module: "DESI",
  db_type: "surrealdb",
  surreal_ns: 1516,
  db_ip: "localhost",
  db_port: "8009",
  db_user: "root",
  db_password: "root",
  gen_model: true,
  gen_mesh: false,
  gen_spatial_tree: true,
  apply_boolean_operation: true,
  mesh_tol_ratio: 3.0,
  room_keyword: "-RM",
  target_sesno: null,
}

export const DB_TYPES = ["surrealdb", "mysql", "postgresql"] as const
export const MODULES = ["DESI", "PIPE", "EQUI", "STRU"] as const
export const ENVIRONMENTS = ["dev", "test", "staging", "prod"] as const

export const ENVIRONMENT_LABELS: Record<typeof ENVIRONMENTS[number], string> = {
  dev: "开发环境",
  test: "测试环境",
  staging: "预发布环境",
  prod: "生产环境",
}