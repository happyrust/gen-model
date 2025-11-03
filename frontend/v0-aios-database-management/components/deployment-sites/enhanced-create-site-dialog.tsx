"use client"

/**
 * 增强版创建站点对话框
 *
 * 三步向导流程：
 * 1. 基础信息 - 站点名称、描述、环境等
 * 2. 选择项目 - 扫描目录并选择项目
 * 3. 数据库配置 - 配置数据库连接和选项
 */

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
import { Plus } from "lucide-react"

import type { Site } from "./site-card"
import { useCreateSiteForm } from "@/hooks/use-create-site-form"
import { scanDirectory, submitCreateSite } from "@/hooks/use-site-operations"
import { Step1BasicInfo } from "./steps/step1-basic-info"
import { Step2SelectProjects } from "./steps/step2-select-projects"
import { Step3DatabaseConfig } from "./steps/step3-database-config"
import { StepNavigation } from "./create-dialog/StepNavigation"
import { ErrorDisplay } from "./create-dialog/ErrorDisplay"

interface EnhancedCreateSiteDialogProps {
  onCreateSite?: (site: Site) => void
  trigger?: ReactNode
}

const TOTAL_STEPS = 3

export function EnhancedCreateSiteDialog({ onCreateSite, trigger }: EnhancedCreateSiteDialogProps) {
  // 对话框状态
  const [open, setOpen] = useState(false)
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [isScanning, setIsScanning] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [errorDetails, setErrorDetails] = useState<string | null>(null)
  const [showErrorDetails, setShowErrorDetails] = useState(false)

  // 表单状态
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

  // 扫描目录
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

  // 表单验证
  const validateStep = (currentStep: number): boolean => {
    if (currentStep === 1 && !formData.name.trim()) {
      setError("请输入站点名称")
      return false
    }
    if (currentStep === 2 && formData.selectedProjects.length === 0) {
      setError("请至少选择一个项目")
      return false
    }
    setError(null)
    return true
  }

  // 步骤导航
  const handleNext = () => {
    if (validateStep(step) && step < TOTAL_STEPS) {
      setStep(step + 1)
    }
  }

  const handlePrevious = () => {
    setError(null)
    if (step > 1) {
      setStep(step - 1)
    }
  }

  // 提交表单
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

      // 提取详细错误信息
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

  // 关闭对话框
  const handleClose = (isOpen: boolean) => {
    if (!isOpen) {
      resetForm()
      setError(null)
      setErrorDetails(null)
      setShowErrorDetails(false)
    }
    setOpen(isOpen)
  }

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogTrigger asChild>
        {trigger || (
          <Button className="gap-1">
            <Plus className="h-4 w-4" />
            创建站点
          </Button>
        )}
      </DialogTrigger>

      <DialogContent className="max-w-4xl max-h-[90vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>创建部署站点 - 步骤 {step}/{TOTAL_STEPS}</DialogTitle>
          <DialogDescription>
            {step === 1 && "填写站点基本信息，包括名称、描述和环境配置"}
            {step === 2 && "扫描并选择要包含的 PDMS 项目"}
            {step === 3 && "配置数据库连接和数据生成选项"}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-6 py-4">
          {/* 错误显示 */}
          <ErrorDisplay
            error={error}
            errorDetails={errorDetails}
            showDetails={showErrorDetails}
            onToggleDetails={() => setShowErrorDetails(!showErrorDetails)}
          />

          {/* 步骤内容 */}
          {step === 1 && (
            <Step1BasicInfo formData={formData} onInputChange={handleInputChange} />
          )}

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

        <DialogFooter>
          <StepNavigation
            currentStep={step}
            totalSteps={TOTAL_STEPS}
            isSubmitting={isSubmitting}
            isLoading={isScanning}
            onPrevious={handlePrevious}
            onNext={handleNext}
            onSubmit={handleSubmit}
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
