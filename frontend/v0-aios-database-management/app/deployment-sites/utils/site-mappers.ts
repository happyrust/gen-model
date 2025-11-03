/**
 * 站点数据映射工具函数
 *
 * 负责将 API 响应数据转换为前端使用的 Site 类型
 */

import type { Site } from "@/components/deployment-sites/site-card"

/**
 * 映射站点状态
 */
export function mapStatus(value: unknown): Site["status"] {
  if (typeof value === "string") {
    const normalized = value.toLowerCase()
    if (normalized === "stopped") return "stopped"
    if (normalized === "paused") return "paused"
    if (normalized === "running") return "running"
    if (normalized === "deploying") return "deploying"
    if (normalized === "configuring") return "configuring"
    if (normalized === "failed") return "failed"
  }
  return "configuring"
}

/**
 * 映射环境类型
 */
export function mapEnvironment(value: unknown): Site["environment"] {
  if (typeof value === "string") {
    const normalized = value.toLowerCase()
    if (["dev", "test", "staging", "prod"].includes(normalized)) {
      return normalized as Site["environment"]
    }
  }
  return "dev"
}

/**
 * 映射日期字段
 * 支持 ISO 字符串、Unix 时间戳（秒/毫秒）
 */
export function mapDate(value: unknown): string {
  if (!value) return new Date().toISOString()
  if (typeof value === "string") return value
  if (typeof value === "number") {
    // 判断是毫秒还是秒级时间戳
    if (value > 1_000_000_000_000) {
      return new Date(value).toISOString()
    }
    return new Date(value * 1000).toISOString()
  }
  return new Date().toISOString()
}

/**
 * 生成站点 ID
 * 优先使用 API 返回的 ID，否则生成随机 ID
 */
function generateSiteId(id: unknown): string {
  if (typeof id === "string" && id.length > 0) {
    return id
  }

  // 使用 crypto.randomUUID (现代浏览器)
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID()
  }

  // 回退方案：基于时间戳的 ID
  return `site-${Date.now()}-${Math.random().toString(36).substring(2, 9)}`
}

/**
 * 映射单个站点数据
 * 将 API 返回的原始对象转换为 Site 类型
 */
export function mapSite(item: Record<string, unknown>): Site {
  return {
    id: generateSiteId(item.id),
    name: (item.name as string) || "未命名站点",
    status: mapStatus(item.status),
    environment: mapEnvironment(item.env),
    owner: (item.owner as string) || undefined,
    createdAt: mapDate(item.created_at),
    updatedAt: mapDate(item.updated_at),
    url: typeof item.url === "string" ? item.url : undefined,
    description: typeof item.description === "string" ? item.description : undefined,
  }
}

/**
 * 批量映射站点数据
 */
export function mapSites(items: Array<Record<string, unknown>>): Site[] {
  return items.map(mapSite)
}
