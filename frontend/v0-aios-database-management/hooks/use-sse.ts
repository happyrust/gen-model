import { useEffect, useRef, useState, useCallback } from 'react'
import type { SyncEvent } from '@/types/remote-sync'

interface UseSSEOptions {
  url: string
  onMessage?: (event: SyncEvent) => void
  onError?: (error: Error) => void
  onOpen?: () => void
  enabled?: boolean
  maxReconnectAttempts?: number
  reconnectDelay?: number
}

interface UseSSEReturn {
  connected: boolean
  error: string | null
  reconnecting: boolean
  reconnectAttempts: number
  close: () => void
  reconnect: () => void
}

/**
 * SSE (Server-Sent Events) 连接 Hook
 * 
 * 提供自动重连、错误处理和状态管理功能
 */
export function useSSE({
  url,
  onMessage,
  onError,
  onOpen,
  enabled = true,
  maxReconnectAttempts = 5,
  reconnectDelay = 1000,
}: UseSSEOptions): UseSSEReturn {
  const [connected, setConnected] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [reconnecting, setReconnecting] = useState(false)
  const [reconnectAttempts, setReconnectAttempts] = useState(0)
  
  const eventSourceRef = useRef<EventSource | null>(null)
  const reconnectTimeoutRef = useRef<NodeJS.Timeout>()
  const shouldReconnectRef = useRef(true)

  const close = useCallback(() => {
    shouldReconnectRef.current = false
    
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current)
      reconnectTimeoutRef.current = undefined
    }
    
    if (eventSourceRef.current) {
      eventSourceRef.current.close()
      eventSourceRef.current = null
    }
    
    setConnected(false)
    setReconnecting(false)
  }, [])

  const connect = useCallback(() => {
    if (!enabled) {
      return
    }

    // 关闭现有连接
    if (eventSourceRef.current) {
      eventSourceRef.current.close()
    }

    try {
      const eventSource = new EventSource(url)
      eventSourceRef.current = eventSource

      eventSource.onopen = () => {
        setConnected(true)
        setError(null)
        setReconnecting(false)
        setReconnectAttempts(0)
        onOpen?.()
      }

      eventSource.onmessage = (event) => {
        try {
          const data = JSON.parse(event.data) as SyncEvent
          onMessage?.(data)
        } catch (err) {
          console.error('Failed to parse SSE message:', err)
        }
      }

      eventSource.onerror = () => {
        setConnected(false)
        eventSource.close()

        if (!shouldReconnectRef.current) {
          return
        }

        // 自动重连逻辑
        if (reconnectAttempts < maxReconnectAttempts) {
          setReconnecting(true)
          
          // 指数退避算法
          const delay = Math.min(
            reconnectDelay * Math.pow(2, reconnectAttempts),
            30000 // 最大 30 秒
          )
          
          reconnectTimeoutRef.current = setTimeout(() => {
            setReconnectAttempts((prev) => prev + 1)
            connect()
          }, delay)
        } else {
          const errorMsg = 'SSE 连接失败，已达到最大重试次数'
          setError(errorMsg)
          setReconnecting(false)
          onError?.(new Error(errorMsg))
        }
      }
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : 'SSE 连接失败'
      setError(errorMsg)
      onError?.(err instanceof Error ? err : new Error(errorMsg))
    }
  }, [url, enabled, onMessage, onError, onOpen, reconnectAttempts, maxReconnectAttempts, reconnectDelay])

  const reconnect = useCallback(() => {
    setReconnectAttempts(0)
    setError(null)
    shouldReconnectRef.current = true
    connect()
  }, [connect])

  useEffect(() => {
    if (enabled) {
      shouldReconnectRef.current = true
      connect()
    }

    return () => {
      close()
    }
  }, [enabled, connect, close])

  return {
    connected,
    error,
    reconnecting,
    reconnectAttempts,
    close,
    reconnect,
  }
}

/**
 * 简化版 SSE Hook，用于快速集成
 */
export function useSimpleSSE(
  url: string,
  onMessage: (event: SyncEvent) => void,
  enabled = true
) {
  return useSSE({
    url,
    onMessage,
    enabled,
  })
}
