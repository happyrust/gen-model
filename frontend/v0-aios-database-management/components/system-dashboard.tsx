"use client"

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Progress } from "@/components/ui/progress"
import { Badge } from "@/components/ui/badge"
import {
  Activity,
  Cpu,
  HardDrive,
  MemoryStick,
  Network,
  Clock,
  CheckCircle,
  AlertTriangle,
  TrendingUp,
  TrendingDown,
} from "lucide-react"

const systemMetrics = [
  {
    title: "CPU 使用率",
    value: 23,
    unit: "%",
    icon: Cpu,
    status: "normal",
    trend: "up",
    change: "+2.3%",
  },
  {
    title: "内存使用",
    value: 67,
    unit: "%",
    icon: MemoryStick,
    status: "warning",
    trend: "up",
    change: "+5.1%",
  },
  {
    title: "磁盘空间",
    value: 45,
    unit: "%",
    icon: HardDrive,
    status: "normal",
    trend: "down",
    change: "-1.2%",
  },
  {
    title: "网络流量",
    value: 89,
    unit: "MB/s",
    icon: Network,
    status: "high",
    trend: "up",
    change: "+12.5%",
  },
]

const serviceStatus = [
  {
    name: "数据库服务",
    status: "running",
    uptime: "99.9%",
    lastCheck: "1 分钟前",
  },
  {
    name: "API 网关",
    status: "running",
    uptime: "99.8%",
    lastCheck: "1 分钟前",
  },
  {
    name: "缓存服务",
    status: "running",
    uptime: "100%",
    lastCheck: "30 秒前",
  },
  {
    name: "消息队列",
    status: "warning",
    uptime: "98.5%",
    lastCheck: "2 分钟前",
  },
  {
    name: "文件存储",
    status: "running",
    uptime: "99.7%",
    lastCheck: "1 分钟前",
  },
  {
    name: "监控服务",
    status: "running",
    uptime: "99.9%",
    lastCheck: "30 秒前",
  },
]

const recentActivities = [
  {
    type: "success",
    message: "数据库生成任务完成",
    time: "2 分钟前",
    details: "处理了 1,234 条记录",
  },
  {
    type: "info",
    message: "系统配置更新",
    time: "15 分钟前",
    details: "更新了缓存配置参数",
  },
  {
    type: "warning",
    message: "内存使用率较高",
    time: "30 分钟前",
    details: "当前使用率 67%，建议关注",
  },
  {
    type: "success",
    message: "空间树生成完成",
    time: "1 小时前",
    details: "成功构建 2,456 个节点",
  },
]

function getStatusColor(status: string) {
  switch (status) {
    case "normal":
      return "text-success"
    case "warning":
      return "text-warning"
    case "high":
      return "text-destructive"
    default:
      return "text-muted-foreground"
  }
}

function getServiceStatusBadge(status: string) {
  switch (status) {
    case "running":
      return <Badge className="bg-success text-success-foreground">运行中</Badge>
    case "warning":
      return <Badge className="bg-warning text-warning-foreground">警告</Badge>
    case "error":
      return <Badge className="bg-destructive text-destructive-foreground">错误</Badge>
    default:
      return <Badge className="bg-muted text-muted-foreground">未知</Badge>
  }
}

function getActivityIcon(type: string) {
  switch (type) {
    case "success":
      return <CheckCircle className="h-4 w-4 text-success" />
    case "warning":
      return <AlertTriangle className="h-4 w-4 text-warning" />
    case "info":
      return <Activity className="h-4 w-4 text-info" />
    default:
      return <Clock className="h-4 w-4 text-muted-foreground" />
  }
}

export function SystemDashboard() {
  return (
    <div className="space-y-6">
      {/* System Metrics */}
      <div>
        <h3 className="text-lg font-semibold text-foreground mb-4">系统监控</h3>
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
          {systemMetrics.map((metric, index) => {
            const IconComponent = metric.icon
            return (
              <Card key={index} className="bg-card border-border">
                <CardContent className="p-4">
                  <div className="flex items-center justify-between mb-3">
                    <IconComponent className={`h-5 w-5 ${getStatusColor(metric.status)}`} />
                    <div className="flex items-center gap-1 text-xs">
                      {metric.trend === "up" ? (
                        <TrendingUp className="h-3 w-3 text-success" />
                      ) : (
                        <TrendingDown className="h-3 w-3 text-destructive" />
                      )}
                      <span className={metric.trend === "up" ? "text-success" : "text-destructive"}>
                        {metric.change}
                      </span>
                    </div>
                  </div>
                  <div className="space-y-2">
                    <div className="flex items-baseline gap-1">
                      <span className="text-2xl font-bold text-foreground">{metric.value}</span>
                      <span className="text-sm text-muted-foreground">{metric.unit}</span>
                    </div>
                    <p className="text-sm text-muted-foreground">{metric.title}</p>
                    <Progress value={metric.value} className="h-1.5" />
                  </div>
                </CardContent>
              </Card>
            )
          })}
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Service Status */}
        <Card className="bg-card border-border">
          <CardHeader>
            <CardTitle className="text-lg text-card-foreground">服务状态</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            {serviceStatus.map((service, index) => (
              <div key={index} className="flex items-center justify-between p-3 bg-secondary/50 rounded-lg">
                <div className="flex items-center gap-3">
                  <div className="flex items-center gap-2">
                    <div
                      className={`w-2 h-2 rounded-full ${
                        service.status === "running"
                          ? "bg-success"
                          : service.status === "warning"
                            ? "bg-warning"
                            : "bg-destructive"
                      }`}
                    ></div>
                    <span className="font-medium text-secondary-foreground">{service.name}</span>
                  </div>
                </div>
                <div className="flex items-center gap-3">
                  <div className="text-right">
                    <p className="text-sm font-medium text-secondary-foreground">{service.uptime}</p>
                    <p className="text-xs text-muted-foreground">{service.lastCheck}</p>
                  </div>
                  {getServiceStatusBadge(service.status)}
                </div>
              </div>
            ))}
          </CardContent>
        </Card>

        {/* Recent Activities */}
        <Card className="bg-card border-border">
          <CardHeader>
            <CardTitle className="text-lg text-card-foreground">最近活动</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            {recentActivities.map((activity, index) => (
              <div key={index} className="flex gap-3 p-3 bg-secondary/50 rounded-lg">
                <div className="flex-shrink-0 mt-0.5">{getActivityIcon(activity.type)}</div>
                <div className="flex-1 min-w-0">
                  <p className="font-medium text-secondary-foreground">{activity.message}</p>
                  <p className="text-sm text-muted-foreground">{activity.details}</p>
                  <p className="text-xs text-muted-foreground mt-1">{activity.time}</p>
                </div>
              </div>
            ))}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
