import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Progress } from "@/components/ui/progress"
import { Database, Activity, Settings, Zap, Clock, CheckCircle, AlertCircle, TrendingUp } from "lucide-react"
import { cn } from "@/lib/utils"

const features = [
  {
    id: "database",
    title: "数据库生成",
    description: "生成和管理数据库编号7999的数据，支持多种数据库类型和自动化配置",
    icon: Database,
    color: "primary",
    stats: {
      total: 7999,
      completed: 7856,
      progress: 98.2,
    },
    status: "active",
    lastRun: "2 分钟前",
    features: ["自动化数据生成", "多数据库支持", "实时监控", "批量处理"],
  },
  {
    id: "spatial",
    title: "空间树生成",
    description: "构建和优化空间关系数据结构，提供高效的空间查询和分析能力",
    icon: Activity,
    color: "success",
    stats: {
      total: 2456,
      completed: 2456,
      progress: 100,
    },
    status: "completed",
    lastRun: "1 小时前",
    features: ["空间索引优化", "关系数据结构", "高效查询", "可视化分析"],
  },
  {
    id: "config",
    title: "配置管理",
    description: "管理系统配置参数和设置，确保系统运行的稳定性和安全性",
    icon: Settings,
    color: "info",
    stats: {
      total: 156,
      completed: 142,
      progress: 91.0,
    },
    status: "warning",
    lastRun: "30 分钟前",
    features: ["参数配置", "安全设置", "性能优化", "备份恢复"],
  },
]

const colorStyles: Record<
  (typeof features)[number]["color"],
  { iconBg: string; iconText: string; actionButton: string }
> = {
  primary: {
    iconBg: "bg-primary/10",
    iconText: "text-primary",
    actionButton: "bg-primary text-primary-foreground hover:bg-primary/90",
  },
  success: {
    iconBg: "bg-emerald-100",
    iconText: "text-success",
    actionButton: "bg-success text-success-foreground hover:bg-success/90",
  },
  info: {
    iconBg: "bg-blue-100",
    iconText: "text-info",
    actionButton: "bg-info text-info-foreground hover:bg-info/90",
  },
}

function getStatusIcon(status: string) {
  switch (status) {
    case "active":
      return <Zap className="h-4 w-4" />
    case "completed":
      return <CheckCircle className="h-4 w-4" />
    case "warning":
      return <AlertCircle className="h-4 w-4" />
    default:
      return <Clock className="h-4 w-4" />
  }
}

function getStatusColor(status: string) {
  switch (status) {
    case "active":
      return "bg-primary text-primary-foreground"
    case "completed":
      return "bg-success text-success-foreground"
    case "warning":
      return "bg-warning text-warning-foreground"
    default:
      return "bg-muted text-muted-foreground"
  }
}

function getStatusText(status: string) {
  switch (status) {
    case "active":
      return "运行中"
    case "completed":
      return "已完成"
    case "warning":
      return "需要关注"
    default:
      return "待处理"
  }
}

export function FeatureCards() {
  return (
    <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
      {features.map((feature) => {
        const IconComponent = feature.icon
        const colorClass = colorStyles[feature.color] ?? colorStyles.primary
        return (
          <Card
            key={feature.id}
            className="bg-card border-border hover:border-primary/30 transition-all duration-300 group"
          >
            <CardHeader className="pb-4">
              <div className="flex items-center justify-between mb-4">
                <div className={cn("w-12 h-12 rounded-xl flex items-center justify-center", colorClass.iconBg)}>
                  <IconComponent className={cn("h-6 w-6", colorClass.iconText)} />
                </div>
                <Badge className={getStatusColor(feature.status)}>
                  {getStatusIcon(feature.status)}
                  <span className="ml-1">{getStatusText(feature.status)}</span>
                </Badge>
              </div>
              <CardTitle className="text-xl text-card-foreground group-hover:text-primary transition-colors">
                {feature.title}
              </CardTitle>
              <CardDescription className="text-muted-foreground leading-relaxed">{feature.description}</CardDescription>
            </CardHeader>

            <CardContent className="space-y-6">
              {/* Progress Section */}
              <div className="space-y-3">
                <div className="flex items-center justify-between text-sm">
                  <span className="text-muted-foreground">处理进度</span>
                  <span className="font-medium text-foreground">{feature.stats.progress}%</span>
                </div>
                <Progress value={feature.stats.progress} className="h-2" />
                <div className="flex items-center justify-between text-xs text-muted-foreground">
                  <span>{feature.stats.completed.toLocaleString()} 已完成</span>
                  <span>{feature.stats.total.toLocaleString()} 总计</span>
                </div>
              </div>

              {/* Features List */}
              <div className="space-y-2">
                <h4 className="text-sm font-medium text-foreground">核心功能</h4>
                <div className="grid grid-cols-2 gap-2">
                  {feature.features.map((feat, index) => (
                    <div key={index} className="flex items-center gap-2 text-xs text-muted-foreground">
                      <div className="w-1.5 h-1.5 bg-primary rounded-full"></div>
                      {feat}
                    </div>
                  ))}
                </div>
              </div>

              {/* Last Run Info */}
              <div className="flex items-center gap-2 text-xs text-muted-foreground border-t border-border pt-4">
                <Clock className="h-3 w-3" />
                <span>最后运行: {feature.lastRun}</span>
              </div>

              {/* Action Buttons */}
              <div className="flex gap-2 pt-2">
                <Button size="sm" className={cn("flex-1", colorClass.actionButton)}>
                  {feature.status === "active" ? "查看详情" : "立即执行"}
                </Button>
                <Button size="sm" variant="outline" className="px-3 bg-transparent">
                  <TrendingUp className="h-4 w-4" />
                </Button>
              </div>
            </CardContent>
          </Card>
        )
      })}
    </div>
  )
}
