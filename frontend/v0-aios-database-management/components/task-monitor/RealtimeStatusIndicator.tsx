"use client"

import { Wifi, WifiOff, RefreshCw } from "lucide-react"

interface RealtimeStatusIndicatorProps {
  isConnected: boolean
  lastUpdate?: string
  onReconnect?: () => void
}

export function RealtimeStatusIndicator({ isConnected, lastUpdate, onReconnect }: RealtimeStatusIndicatorProps) {
  return (
    <div className="flex items-center gap-3 text-sm">
      <div className="flex items-center gap-1">
        {isConnected ? (
          <>
            <span className="w-2 h-2 rounded-full bg-green-500 inline-block" />
            <Wifi className="h-4 w-4 text-green-600" />
            <span className="text-green-700">实时连接正常</span>
          </>
        ) : (
          <>
            <span className="w-2 h-2 rounded-full bg-red-500 inline-block" />
            <WifiOff className="h-4 w-4 text-red-600" />
            <span className="text-red-700">连接断开</span>
          </>
        )}
      </div>
      {lastUpdate && (
        <span className="text-muted-foreground">上次更新: {lastUpdate}</span>
      )}
      <button
        type="button"
        className="inline-flex items-center gap-1 text-muted-foreground hover:text-foreground"
        onClick={onReconnect}
      >
        <RefreshCw className="h-4 w-4" /> 重试
      </button>
    </div>
  )
}


