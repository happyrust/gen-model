import type { Site } from "./site-card"

export function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 Bytes"
  const k = 1024
  const sizes = ["Bytes", "KB", "MB", "GB"]
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return Math.round(bytes / Math.pow(k, i) * 100) / 100 + " " + sizes[i]
}

export function normalizeDate(value: unknown): string {
  if (!value) return new Date().toISOString()
  if (typeof value === "string") return value
  if (typeof value === "number") return new Date(value).toISOString()
  return new Date().toISOString()
}

export function normalizeStatus(value: unknown): Site["status"] {
  if (typeof value === "string") {
    const mapped = value.toLowerCase() as Site["status"]
    if (["running", "deploying", "configuring", "failed", "paused", "stopped"].includes(mapped)) {
      return mapped === "stopped" ? "paused" : mapped
    }
  }
  return "configuring"
}

export async function extractErrorMessage(response: Response): Promise<string | null> {
  try {
    const data = await response.json()
    if (typeof data?.message === "string") {
      return data.message
    }
    if (Array.isArray(data?.errors) && data.errors.length > 0) {
      return String(data.errors[0])
    }
  } catch (_) {
    // ignore
  }
  return response.status === 400 ? "请求参数不正确" : null
}