"use client"

import { createContext, useContext, useState, useCallback, type ReactNode } from 'react'
import type { Alert } from '@/types/remote-sync'

interface AlertContextValue {
  alerts: Alert[]
  addAlert: (alert: Omit<Alert, 'id' | 'timestamp' | 'acknowledged'>) => void
  acknowledgeAlert: (id: string) => void
  removeAlert: (id: string) => void
  clearAlerts: () => void
  unacknowledgedCount: number
}

const AlertContext = createContext<AlertContextValue | undefined>(undefined)

interface AlertProviderProps {
  children: ReactNode
}

/**
 * 告警状态 Provider
 * 
 * 管理全局告警状态
 */
export function AlertProvider({ children }: AlertProviderProps) {
  const [alerts, setAlerts] = useState<Alert[]>([])

  const addAlert = useCallback((alert: Omit<Alert, 'id' | 'timestamp' | 'acknowledged'>) => {
    const newAlert: Alert = {
      ...alert,
      id: `alert-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
      timestamp: new Date().toISOString(),
      acknowledged: false,
    }
    setAlerts((prev) => [newAlert, ...prev])
  }, [])

  const acknowledgeAlert = useCallback((id: string) => {
    setAlerts((prev) =>
      prev.map((alert) =>
        alert.id === id ? { ...alert, acknowledged: true } : alert
      )
    )
  }, [])

  const removeAlert = useCallback((id: string) => {
    setAlerts((prev) => prev.filter((alert) => alert.id !== id))
  }, [])

  const clearAlerts = useCallback(() => {
    setAlerts([])
  }, [])

  const unacknowledgedCount = alerts.filter((alert) => !alert.acknowledged).length

  const value: AlertContextValue = {
    alerts,
    addAlert,
    acknowledgeAlert,
    removeAlert,
    clearAlerts,
    unacknowledgedCount,
  }

  return <AlertContext.Provider value={value}>{children}</AlertContext.Provider>
}

/**
 * 使用告警 Context 的 Hook
 */
export function useAlerts() {
  const context = useContext(AlertContext)
  if (context === undefined) {
    throw new Error('useAlerts must be used within an AlertProvider')
  }
  return context
}
