const HTTP_PREFIX = /^https?:\/\//i

function normalizeBaseUrl(value: string | undefined | null): string {
  if (!value) return ""
  return value.replace(/\/+$/, "")
}

export function getXktApiBaseUrl(): string {
  const explicitBase = normalizeBaseUrl(process.env.NEXT_PUBLIC_XKT_API_BASE_URL)
  if (explicitBase) return explicitBase
  return normalizeBaseUrl(process.env.NEXT_PUBLIC_API_BASE_URL)
}

export function buildXktApiUrl(path: string): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`
  const base = getXktApiBaseUrl()
  if (!base) return normalizedPath
  return `${base}${normalizedPath}`
}

export function resolveXktResourceUrl(url: string | undefined | null): string {
  if (!url) return ""
  if (HTTP_PREFIX.test(url)) {
    return url
  }
  return buildXktApiUrl(url)
}
