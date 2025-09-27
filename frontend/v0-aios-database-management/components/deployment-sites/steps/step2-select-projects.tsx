import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Badge } from "@/components/ui/badge"
import { Card, CardContent } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Folder, FolderOpen, Check, Loader2 } from "lucide-react"
import type { CreateSiteFormData, ProjectInfo } from "@/hooks/use-create-site-form"

interface Step2Props {
  formData: CreateSiteFormData
  projects: ProjectInfo[]
  isScanning: boolean
  onInputChange: (field: string, value: string) => void
  onScanDirectory: () => void
  onToggleProjectSelection: (path: string) => void
}

export function Step2SelectProjects({
  formData,
  projects,
  isScanning,
  onInputChange,
  onScanDirectory,
  onToggleProjectSelection,
}: Step2Props) {
  return (
    <div className="space-y-4">
      <div className="space-y-2">
        <Label>项目根目录</Label>
        <div className="flex gap-2">
          <Input
            placeholder="/path/to/e3d/models"
            value={formData.rootDirectory}
            onChange={(e) => onInputChange("rootDirectory", e.target.value)}
          />
          <Button
            onClick={onScanDirectory}
            disabled={isScanning}
            variant="outline"
          >
            {isScanning ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <FolderOpen className="h-4 w-4" />
            )}
            扫描
          </Button>
        </div>
      </div>

      {projects.length > 0 ? (
        <ProjectList projects={projects} onToggle={onToggleProjectSelection} />
      ) : !isScanning ? (
        <EmptyState />
      ) : null}
    </div>
  )
}

function ProjectList({ projects, onToggle }: { projects: ProjectInfo[], onToggle: (path: string) => void }) {
  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <Label>选择 E3D 项目</Label>
        <Badge variant="secondary">
          {projects.filter(p => p.selected).length} / {projects.length} 已选择
        </Badge>
      </div>
      <div className="border rounded-lg max-h-[300px] overflow-y-auto">
        {projects.map((project, index) => (
          <div
            key={project.path}
            className={`flex items-center p-3 hover:bg-accent cursor-pointer ${
              index !== projects.length - 1 ? "border-b" : ""
            }`}
            onClick={() => onToggle(project.path)}
          >
            <div className="flex-1">
              <div className="flex items-center gap-2">
                <Folder className="h-4 w-4 text-muted-foreground" />
                <span className="font-medium">{project.name}</span>
                {project.selected && (
                  <Check className="h-4 w-4 text-success" />
                )}
              </div>
              <p className="text-xs text-muted-foreground mt-1">
                {project.path}
              </p>
              {project.fileCount !== undefined && (
                <p className="text-xs text-muted-foreground">
                  {project.fileCount} 个文件 · {project.size}
                </p>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}

function EmptyState() {
  return (
    <Card>
      <CardContent className="p-6 text-center text-muted-foreground">
        <Folder className="h-12 w-12 mx-auto mb-3" />
        <p>点击"扫描"按钮查找 E3D 项目</p>
      </CardContent>
    </Card>
  )
}