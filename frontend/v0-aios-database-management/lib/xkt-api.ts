import { getPublicApiBaseUrl, getPublicXktApiBaseUrl } from "@/lib/env"

const HTTP_PREFIX = /^https?:\/\//i

export function buildXktApiUrl(path: string): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`
  const base = getXktApiBaseUrlInternal()
  if (!base) {
    const fallback = getPublicApiBaseUrl()
    if (!fallback) return normalizedPath
    return `${fallback}${normalizedPath}`
  }
  return `${base}${normalizedPath}`
}

export function resolveXktResourceUrl(url: string | undefined | null): string {
  if (!url) return ""
  if (HTTP_PREFIX.test(url)) {
    return url
  }
  return buildXktApiUrl(url)
}

function getXktApiBaseUrlInternal(): string {
  const explicit = getPublicXktApiBaseUrl()
  if (explicit) return explicit
  return ""
}
