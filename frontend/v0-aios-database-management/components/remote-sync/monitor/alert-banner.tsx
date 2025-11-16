"use client"

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { AlertCircle, AlertTriangle, Info, X, ExternalLink } from "lucide-react"
import { useAlerts } from "@/contexts/alert-context"
import Link from "next/link"

export function AlertBanner() {
  const { alerts, acknowledgeAlert, removeAlert, unacknowledgedCount } = useAlerts()

  // 只显示未确认的告警
  const unacknowledgedAlerts = alerts.filter((alert) => !alert.acknowledged)

  if (unacknowledgedAlerts.length === 0) {
    return null
  }

  const getIcon = (type: string) => {
    switch (type) {
      case 'error':
        return <AlertCircle className="h-4 w-4" />
      case 'warning':
        return <AlertTriangle className="h-4 w-4" />
      default:
        return <Info className="h-4 w-4" />
    }
  }

  const getVariant = (type: string): "default" | "destructive" => {
    return type === 'error' ? 'destructive' : 'default'
  }

  return (
    <div className="space-y-2">
      {unacknowledgedAlerts.slice(0, 3).map((alert) => (
        <Alert key={alert.id} variant={getVariant(alert.type)}>
          <div className="flex items-start justify-between">
            <div className="flex items-start gap-2 flex-1">
              {getIcon(alert.type)}
              <div className="flex-1">
                <AlertTitle className="flex items-center gap-2">
                  {alert.title}
                  <Badge variant="outline" className="text-xs">
                    {new Date(alert.timestamp).toLocaleTimeString()}
                  </Badge>
                </AlertTitle>
                <AlertDescription className="mt-1">
                  {alert.message}
                </AlertDescription>
                {alert.actionUrl && (
                  <Link href={alert.actionUrl}>
                    <Button variant="link" size="sm" className="mt-2 p-0 h-auto">
                      查看详情
                      <ExternalLink className="h-3 w-3 ml-1" />
                    </Button>
                  </Link>
                )}
              </div>
            </div>
            <div className="flex items-center gap-1 ml-2">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => acknowledgeAlert(alert.id)}
              >
                确认
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => removeAlert(alert.id)}
              >
                <X className="h-4 w-4" />
              </Button>
            </div>
          </div>
        </Alert>
      ))}

      {unacknowledgedAlerts.length > 3 && (
        <div className="text-center">
          <Button variant="outline" size="sm">
            还有 {unacknowledgedAlerts.length - 3} 条告警
          </Button>
        </div>
      )}
    </div>
  )
}
