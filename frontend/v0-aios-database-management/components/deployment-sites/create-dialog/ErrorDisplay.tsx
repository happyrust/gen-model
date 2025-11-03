/**
 * 创建站点对话框 - 错误显示组件
 */

import { AlertCircle, ChevronDown, ChevronRight } from "lucide-react"
import { Button } from "@/components/ui/button"

interface ErrorDisplayProps {
  error: string | null
  errorDetails: string | null
  showDetails: boolean
  onToggleDetails: () => void
}

export function ErrorDisplay({ error, errorDetails, showDetails, onToggleDetails }: ErrorDisplayProps) {
  if (!error) return null

  return (
    <div className="space-y-2">
      <div className="rounded-md border border-destructive/50 bg-destructive/10 p-3">
        <div className="flex items-start space-x-2">
          <AlertCircle className="h-5 w-5 text-destructive flex-shrink-0 mt-0.5" />
          <div className="flex-1">
            <p className="text-sm font-medium text-destructive">{error}</p>
            {errorDetails && (
              <Button
                type="button"
                variant="link"
                size="sm"
                onClick={onToggleDetails}
                className="h-auto p-0 text-xs text-destructive/80 hover:text-destructive"
              >
                {showDetails ? (
                  <>
                    <ChevronDown className="mr-1 h-3 w-3" />
                    隐藏详情
                  </>
                ) : (
                  <>
                    <ChevronRight className="mr-1 h-3 w-3" />
                    查看详情
                  </>
                )}
              </Button>
            )}
          </div>
        </div>
      </div>

      {showDetails && errorDetails && (
        <div className="rounded-md border border-muted bg-muted/30 p-3">
          <pre className="text-xs text-muted-foreground overflow-auto max-h-40 whitespace-pre-wrap break-words">
            {errorDetails}
          </pre>
        </div>
      )}
    </div>
  )
}
