"use client"

import { useState, type ReactNode } from "react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog"
import { Plus, AlertCircle, Loader2, ChevronDown, ChevronRight } from "lucide-react"
import type { Site } from "./site-card"
import { useCreateSiteForm } from "@/hooks/use-create-site-form"
import { scanDirectory, submitCreateSite } from "@/hooks/use-site-operations"
import { Step1BasicInfo } from "./steps/step1-basic-info"
import { Step2SelectProjects } from "./steps/step2-select-projects"
import { Step3DatabaseConfig } from "./steps/step3-database-config"

interface EnhancedCreateSiteDialogProps {
  onCreateSite?: (site: Site) => void
  trigger?: ReactNode
}

export function EnhancedCreateSiteDialog({ onCreateSite, trigger }: EnhancedCreateSiteDialogProps) {
  const [open, setOpen] = useState(false)
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [isScanning, setIsScanning] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [errorDetails, setErrorDetails] = useState<string | null>(null)
  const [showErrorDetails, setShowErrorDetails] = useState(false)

  const {
    step,
    setStep,
    formData,
    projects,
    setProjects,
    showAdvanced,
    setShowAdvanced,
    handleInputChange,
    handleConfigChange,
    toggleProjectSelection,
    resetForm,
  } = useCreateSiteForm()

  const handleScanDirectory = async () => {
    if (!formData.rootDirectory) {
      setError("请输入项目根目录路径")
      return
    }

    setIsScanning(true)
    setError(null)

    try {
      await scanDirectory(formData.rootDirectory, setProjects)
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : "扫描目录失败"
      setError(errorMessage)
      setErrorDetails(err instanceof Error ? err.stack || null : null)
      setProjects([])
    } finally {
      setIsScanning(false)
    }
  }

  const handleNext = () => {
    if (step === 1 && !formData.name.trim()) {
      setError("请输入站点名称")
      return
    }
    if (step === 2 && formData.selectedProjects.length === 0) {
      setError("请至少选择一个项目")
      return
    }
    setError(null)
    if (step < 3) setStep(step + 1)
  }

  const handlePrevious = () => {
    setError(null)
    if (step > 1) setStep(step - 1)
  }

  const handleSubmit = async () => {
    if (!formData.name.trim()) {
      setError("站点名称不能为空")
      return
    }

    setIsSubmitting(true)
    setError(null)

    try {
      const newSite = await submitCreateSite(formData)
      onCreateSite?.(newSite)
      setOpen(false)
      resetForm()
      setError(null)
      setErrorDetails(null)
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : "创建站点失败"
      setError(errorMessage)

      // 尝试提取更详细的错误信息
      if (err instanceof Error) {
        setErrorDetails(err.stack || JSON.stringify(err, null, 2))
      } else {
        setErrorDetails(typeof err === "string" ? err : JSON.stringify(err, null, 2))
      }

      console.error("创建站点详细错误:", err)
    } finally {
      setIsSubmitting(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        {trigger ?? (
          <Button size="sm" className="bg-success text-success-foreground hover:bg-success/90">
            <Plus className="h-4 w-4 mr-2" />
            创建站点
          </Button>
        )}
      </DialogTrigger>
      <DialogContent className="sm:max-w-[600px]">
        <DialogHeader>
          <DialogTitle>创建部署站点</DialogTitle>
          <DialogDescription>
            步骤 {step} / 3: {step === 1 ? "基本信息" : step === 2 ? "选择项目" : "数据库配置"}
          </DialogDescription>
        </DialogHeader>

        <div className="py-4">
          {step === 1 && <Step1BasicInfo formData={formData} onInputChange={handleInputChange} />}
          {step === 2 && (
            <Step2SelectProjects
              formData={formData}
              projects={projects}
              isScanning={isScanning}
              onInputChange={handleInputChange}
              onScanDirectory={handleScanDirectory}
              onToggleProjectSelection={toggleProjectSelection}
            />
          )}
          {step === 3 && (
            <Step3DatabaseConfig
              formData={formData}
              showAdvanced={showAdvanced}
              onConfigChange={handleConfigChange}
              onToggleAdvanced={() => setShowAdvanced(!showAdvanced)}
            />
          )}
        </div>

        {error && (
          <div className="space-y-2">
            <div className="flex items-center gap-2 p-3 text-sm text-destructive bg-destructive/10 rounded-md">
              <AlertCircle className="h-4 w-4 flex-shrink-0" />
              <span className="flex-1">{error}</span>
              {errorDetails && (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setShowErrorDetails(!showErrorDetails)}
                  className="h-6 px-2"
                >
                  {showErrorDetails ? (
                    <>
                      <ChevronDown className="h-3 w-3 mr-1" />
                      收起
                    </>
                  ) : (
                    <>
                      <ChevronRight className="h-3 w-3 mr-1" />
                      详情
                    </>
                  )}
                </Button>
              )}
            </div>
            {showErrorDetails && errorDetails && (
              <div className="p-3 text-xs bg-destructive/5 rounded-md border border-destructive/20 max-h-40 overflow-auto">
                <pre className="whitespace-pre-wrap font-mono text-destructive/80">
                  {errorDetails}
                </pre>
              </div>
            )}
          </div>
        )}

        <DialogFooter>
          <div className="flex justify-between w-full">
            <div>
              {step > 1 && (
                <Button variant="outline" onClick={handlePrevious} disabled={isSubmitting}>
                  上一步
                </Button>
              )}
            </div>
            <div className="flex gap-2">
              <Button variant="outline" onClick={() => setOpen(false)} disabled={isSubmitting}>
                取消
              </Button>
              {step < 3 ? (
                <Button onClick={handleNext} disabled={isSubmitting}>
                  下一步
                </Button>
              ) : (
                <Button onClick={handleSubmit} disabled={isSubmitting}>
                  {isSubmitting ? (
                    <>
                      <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                      创建中...
                    </>
                  ) : (
                    "创建站点"
                  )}
                </Button>
              )}
            </div>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}