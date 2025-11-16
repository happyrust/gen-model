"use client"

import { useState } from "react"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Progress } from "@/components/ui/progress"
import { ChevronLeft, ChevronRight } from "lucide-react"
import { StepBasicInfo } from "./deploy-wizard/step-basic-info"
import { StepSiteConfig } from "./deploy-wizard/step-site-config"
import { StepConnectionTest } from "./deploy-wizard/step-connection-test"
import { StepActivation } from "./deploy-wizard/step-activation"
import type { Environment, Site } from "@/types/remote-sync"

interface DeployWizardProps {
  onComplete: (envId: string) => void
  onCancel: () => void
}

interface WizardData {
  environment: Partial<Environment>
  sites: Partial<Site>[]
  testResults: {
    mqttConnected: boolean
    httpReachable: boolean
    latency: number
  }
}

const STEPS = [
  { id: 1, title: "基本信息", description: "配置环境基本信息" },
  { id: 2, title: "站点配置", description: "添加远程站点" },
  { id: 3, title: "连接测试", description: "测试连接状态" },
  { id: 4, title: "激活确认", description: "确认并激活环境" },
]

export function DeployWizard({ onComplete, onCancel }: DeployWizardProps) {
  const [currentStep, setCurrentStep] = useState(1)
  const [wizardData, setWizardData] = useState<WizardData>({
    environment: {
      mqttPort: 1883,
      reconnectInitialMs: 1000,
      reconnectMaxMs: 30000,
    },
    sites: [],
    testResults: {
      mqttConnected: false,
      httpReachable: false,
      latency: 0,
    },
  })

  const updateEnvironment = (data: Partial<Environment>) => {
    setWizardData((prev) => ({
      ...prev,
      environment: { ...prev.environment, ...data },
    }))
  }

  const updateSites = (sites: Partial<Site>[]) => {
    setWizardData((prev) => ({
      ...prev,
      sites,
    }))
  }

  const updateTestResults = (results: Partial<WizardData["testResults"]>) => {
    setWizardData((prev) => ({
      ...prev,
      testResults: { ...prev.testResults, ...results },
    }))
  }

  const handleNext = () => {
    if (currentStep < STEPS.length) {
      setCurrentStep(currentStep + 1)
    }
  }

  const handlePrevious = () => {
    if (currentStep > 1) {
      setCurrentStep(currentStep - 1)
    }
  }

  const progress = (currentStep / STEPS.length) * 100

  return (
    <div className="max-w-4xl mx-auto space-y-6">
      {/* Progress */}
      <div className="space-y-2">
        <div className="flex items-center justify-between text-sm">
          <span className="font-medium">
            步骤 {currentStep} / {STEPS.length}
          </span>
          <span className="text-muted-foreground">{STEPS[currentStep - 1].title}</span>
        </div>
        <Progress value={progress} className="h-2" />
      </div>

      {/* Steps Indicator */}
      <div className="flex items-center justify-between">
        {STEPS.map((step, index) => (
          <div key={step.id} className="flex items-center">
            <div
              className={`flex items-center justify-center w-10 h-10 rounded-full border-2 transition-colors ${
                currentStep >= step.id
                  ? "border-primary bg-primary text-primary-foreground"
                  : "border-muted bg-background text-muted-foreground"
              }`}
            >
              {step.id}
            </div>
            {index < STEPS.length - 1 && (
              <div
                className={`w-full h-0.5 mx-2 transition-colors ${
                  currentStep > step.id ? "bg-primary" : "bg-muted"
                }`}
                style={{ width: "80px" }}
              />
            )}
          </div>
        ))}
      </div>

      {/* Step Content */}
      <Card>
        <CardHeader>
          <CardTitle>{STEPS[currentStep - 1].title}</CardTitle>
          <CardDescription>{STEPS[currentStep - 1].description}</CardDescription>
        </CardHeader>
        <CardContent>
          {currentStep === 1 && (
            <StepBasicInfo
              data={wizardData.environment}
              onChange={updateEnvironment}
              onNext={handleNext}
            />
          )}
          {currentStep === 2 && (
            <StepSiteConfig
              sites={wizardData.sites}
              onChange={updateSites}
              onNext={handleNext}
              onPrevious={handlePrevious}
            />
          )}
          {currentStep === 3 && (
            <StepConnectionTest
              environment={wizardData.environment}
              sites={wizardData.sites}
              testResults={wizardData.testResults}
              onTestComplete={updateTestResults}
              onNext={handleNext}
              onPrevious={handlePrevious}
            />
          )}
          {currentStep === 4 && (
            <StepActivation
              environment={wizardData.environment}
              sites={wizardData.sites}
              onComplete={onComplete}
              onPrevious={handlePrevious}
              onCancel={onCancel}
            />
          )}
        </CardContent>
      </Card>

      {/* Navigation Buttons (for steps without custom buttons) */}
      {currentStep !== 1 && currentStep !== 4 && (
        <div className="flex items-center justify-between">
          <Button variant="outline" onClick={handlePrevious}>
            <ChevronLeft className="h-4 w-4 mr-2" />
            上一步
          </Button>
          <Button onClick={handleNext}>
            下一步
            <ChevronRight className="h-4 w-4 ml-2" />
          </Button>
        </div>
      )}
    </div>
  )
}
