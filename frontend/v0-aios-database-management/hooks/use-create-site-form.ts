import { useState } from "react"
import type { DeploymentSiteConfigPayload } from "@/lib/api"
import { DEFAULT_CONFIG } from "@/components/deployment-sites/site-config"

export interface ProjectInfo {
  name: string
  path: string
  selected: boolean
  fileCount?: number
  size?: string
}

export interface CreateSiteFormData {
  name: string
  description: string
  environment: string
  owner: string
  rootDirectory: string
  selectedProjects: string[]
  config: DeploymentSiteConfigPayload
  tags: Record<string, unknown>
  notes: string
}

const INITIAL_FORM_DATA: CreateSiteFormData = {
  name: "",
  description: "",
  environment: "dev",
  owner: "",
  rootDirectory: DEFAULT_CONFIG.project_path,
  selectedProjects: [],
  config: { ...DEFAULT_CONFIG },
  tags: {},
  notes: "",
}

export function useCreateSiteForm() {
  const [step, setStep] = useState(1)
  const [formData, setFormData] = useState<CreateSiteFormData>(INITIAL_FORM_DATA)
  const [projects, setProjects] = useState<ProjectInfo[]>([])
  const [showAdvanced, setShowAdvanced] = useState(false)

  const handleInputChange = (field: string, value: any) => {
    setFormData(prev => ({ ...prev, [field]: value }))
  }

  const handleConfigChange = (field: string, value: any) => {
    setFormData(prev => ({
      ...prev,
      config: {
        ...prev.config,
        [field]: value
      }
    }))
  }

  const toggleProjectSelection = (path: string) => {
    setProjects(prev => {
      const updated = prev.map(p =>
        p.path === path ? { ...p, selected: !p.selected } : p
      )

      setFormData(prevForm => ({
        ...prevForm,
        selectedProjects: updated.filter(p => p.selected).map(p => p.path)
      }))

      return updated
    })
  }

  const resetForm = () => {
    setStep(1)
    setFormData(INITIAL_FORM_DATA)
    setProjects([])
    setShowAdvanced(false)
  }

  return {
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
  }
}