let hasWarnedForApiBase = false
let hasWarnedForXktBase = false
let collabWsEnabled: boolean | null = null

function normalizeBaseUrl(value: string | undefined | null): string {
  if (!value) return ""
  return value.replace(/\/+$/, "")
}

export function getPublicApiBaseUrl(): string {
  const raw = normalizeBaseUrl(process.env.NEXT_PUBLIC_API_BASE_URL)
  if (!raw && !hasWarnedForApiBase && typeof window !== "undefined") {
    console.warn("[config] NEXT_PUBLIC_API_BASE_URL 未配置，协同相关请求将回退到同源路径。")
    hasWarnedForApiBase = true
  }
  return raw
}

export function getPublicXktApiBaseUrl(): string {
  const raw = normalizeBaseUrl(process.env.NEXT_PUBLIC_XKT_API_BASE_URL)
  if (!raw && !hasWarnedForXktBase && typeof window !== "undefined") {
    console.warn("[config] NEXT_PUBLIC_XKT_API_BASE_URL 未配置，已回退到 NEXT_PUBLIC_API_BASE_URL。")
    hasWarnedForXktBase = true
  }
  return raw
}

export function isCollaborationWsEnabled(): boolean {
  if (collabWsEnabled !== null) return collabWsEnabled
  const value = (process.env.NEXT_PUBLIC_COLLAB_WS_ENABLED ?? "").toLowerCase().trim()
  collabWsEnabled = value === "1" || value === "true" || value === "yes" || value === "on"
  return collabWsEnabled
}
