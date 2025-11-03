import { Label } from "@/components/ui/label"
import { Button } from "@/components/ui/button"
import { ChevronRight, ChevronDown } from "lucide-react"
import type { CreateSiteFormData } from "@/hooks/use-create-site-form"
import { BasicDatabaseFields, DatabaseConnectionFields, AdvancedConfigFields } from "./database-fields"

interface Step3Props {
  formData: CreateSiteFormData
  showAdvanced: boolean
  onConfigChange: (field: string, value: any) => void
  onToggleAdvanced: () => void
}

export function Step3DatabaseConfig({
  formData,
  showAdvanced,
  onConfigChange,
  onToggleAdvanced,
}: Step3Props) {
  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <Label>数据库配置</Label>
        <Button
          variant="outline"
          size="sm"
          onClick={onToggleAdvanced}
        >
          {showAdvanced ? <ChevronDown className="h-4 w-4 mr-1" /> : <ChevronRight className="h-4 w-4 mr-1" />}
          {showAdvanced ? "隐藏" : "显示"}高级选项
        </Button>
      </div>

      <div className="space-y-3">
        <BasicDatabaseFields formData={formData} onConfigChange={onConfigChange} />
        <DatabaseConnectionFields formData={formData} onConfigChange={onConfigChange} />
        {showAdvanced && <AdvancedConfigFields formData={formData} onConfigChange={onConfigChange} />}
      </div>
    </div>
  )
}