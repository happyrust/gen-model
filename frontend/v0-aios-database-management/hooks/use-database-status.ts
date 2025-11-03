"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { fetchDatabaseStatus } from "@/lib/database-status"

export type DbSimpleStatus = "unknown" | "starting" | "running" | "stopped"

interface UseDatabaseStatusPollerArgs {
  ip?: string
  port?: string | number
  startImmediately?: boolean
  intervalMs?: number
  timeoutMs?: number
  onStatus?: (status: DbSimpleStatus) => void
}

export function useDatabaseStatusPoller({
  ip,
  port,
  startImmediately = false,
  intervalMs = 1000,
  timeoutMs = 180_000,
  onStatus,
}: UseDatabaseStatusPollerArgs) {
  const [status, setStatus] = useState<DbSimpleStatus>("unknown")
  const [loading, setLoading] = useState<boolean>(false)
  const timerRef = useRef<NodeJS.Timeout | null>(null)
  const startedAtRef = useRef<number | null>(null)
  const latestIp = useRef<string | undefined>(ip)
  const latestPort = useRef<string | number | undefined>(port)

  useEffect(() => {
    latestIp.current = ip
    latestPort.current = port
  }, [ip, port])

  const stop = useCallback(() => {
    if (timerRef.current) {
      clearInterval(timerRef.current)
      timerRef.current = null
    }
    startedAtRef.current = null
    setLoading(false)
  }, [])

  const pollOnce = useCallback(async () => {
    if (!latestIp.current || latestPort.current === undefined) return
    try {
      const res = await fetchDatabaseStatus(latestIp.current, latestPort.current)
      const mapped: DbSimpleStatus =
        res.status === "Running" ? "running" : res.status === "Starting" ? "starting" : res.status === "Stopped" ? "stopped" : "unknown"
      setStatus(mapped)
      onStatus?.(mapped)
      if (mapped === "running" || mapped === "stopped") {
        stop()
      }
    } catch (_) {
      setStatus("stopped")
      onStatus?.("stopped")
      stop()
    }
  }, [onStatus, stop])

  const start = useCallback(() => {
    if (timerRef.current) return
    if (!latestIp.current || latestPort.current === undefined) return
    setLoading(true)
    startedAtRef.current = Date.now()
    // 先立即拉一次
    void pollOnce()
    timerRef.current = setInterval(() => {
      // 超时判定
      if (startedAtRef.current && Date.now() - startedAtRef.current > (timeoutMs || 180_000)) {
        setStatus("stopped")
        onStatus?.("stopped")
        stop()
        return
      }
      void pollOnce()
    }, intervalMs)
  }, [intervalMs, onStatus, pollOnce, stop, timeoutMs])

  useEffect(() => {
    if (startImmediately) {
      start()
    }
    return () => stop()
  }, [startImmediately, start, stop])

  return { status, loading, start, stop, setStatus, pollOnce }
}


