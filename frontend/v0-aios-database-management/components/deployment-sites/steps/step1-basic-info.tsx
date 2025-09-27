import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Textarea } from "@/components/ui/textarea"
import { ENVIRONMENT_LABELS } from "../site-config"
import type { CreateSiteFormData } from "@/hooks/use-create-site-form"

interface Step1Props {
  formData: CreateSiteFormData
  onInputChange: (field: string, value: string) => void
}

export function Step1BasicInfo({ formData, onInputChange }: Step1Props) {
  return (
    <div className="space-y-4">
      <div className="space-y-2">
        <Label htmlFor="name">站点名称 *</Label>
        <Input
          id="name"
          placeholder="输入站点名称（如：生产环境-项目A）"
          value={formData.name}
          onChange={(e) => onInputChange("name", e.target.value)}
        />
      </div>

      <div className="space-y-2">
        <Label htmlFor="description">站点描述</Label>
        <Textarea
          id="description"
          placeholder="简要描述站点的用途、包含的项目等信息"
          value={formData.description}
          onChange={(e) => onInputChange("description", e.target.value)}
          rows={3}
        />
      </div>

      <div className="grid grid-cols-2 gap-4">
        <div className="space-y-2">
          <Label htmlFor="environment">部署环境</Label>
          <Select value={formData.environment} onValueChange={(value) => onInputChange("environment", value)}>
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {Object.entries(ENVIRONMENT_LABELS).map(([key, label]) => (
                <SelectItem key={key} value={key}>{label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="space-y-2">
          <Label htmlFor="owner">负责人</Label>
          <Input
            id="owner"
            placeholder="输入负责人姓名"
            value={formData.owner}
            onChange={(e) => onInputChange("owner", e.target.value)}
          />
        </div>
      </div>
    </div>
  )
}