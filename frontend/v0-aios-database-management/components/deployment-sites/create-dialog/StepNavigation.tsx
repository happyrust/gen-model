/**
 * 创建站点对话框 - 步骤导航组件
 */

import { Button } from "@/components/ui/button"
import { Loader2 } from "lucide-react"

interface StepNavigationProps {
  currentStep: number
  totalSteps: number
  isSubmitting: boolean
  isLoading?: boolean
  onPrevious: () => void
  onNext: () => void
  onSubmit: () => void
}

export function StepNavigation({
  currentStep,
  totalSteps,
  isSubmitting,
  isLoading = false,
  onPrevious,
  onNext,
  onSubmit,
}: StepNavigationProps) {
  const isFirstStep = currentStep === 1
  const isLastStep = currentStep === totalSteps

  return (
    <div className="flex items-center justify-between space-x-2">
      <Button
        type="button"
        variant="outline"
        onClick={onPrevious}
        disabled={isFirstStep || isSubmitting || isLoading}
      >
        上一步
      </Button>

      <div className="flex items-center space-x-2">
        {Array.from({ length: totalSteps }, (_, i) => i + 1).map((stepNum) => (
          <div
            key={stepNum}
            className={`h-2 w-2 rounded-full transition-colors ${
              stepNum === currentStep
                ? "bg-primary"
                : stepNum < currentStep
                  ? "bg-primary/50"
                  : "bg-muted"
            }`}
          />
        ))}
      </div>

      {isLastStep ? (
        <Button type="button" onClick={onSubmit} disabled={isSubmitting || isLoading}>
          {isSubmitting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
          {isSubmitting ? "创建中..." : "创建站点"}
        </Button>
      ) : (
        <Button type="button" onClick={onNext} disabled={isSubmitting || isLoading}>
          下一步
        </Button>
      )}
    </div>
  )
}
