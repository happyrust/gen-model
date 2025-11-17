'use client'

import { useState } from 'react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { AlertPanel } from '@/components/remote-sync/alerts/alert-panel'
import { 
  Bell, 
  Settings, 
  Download,
  Search,
} from 'lucide-react'

export default function AlertsPage() {
  const [searchTerm, setSearchTerm] = useState('')
  const [levelFilter, setLevelFilter] = useState<string>('all')
  const [showConfig, setShowConfig] = useState(false)

  return (
    <div className="min-h-screen bg-background p-8">
      <div className="max-w-7xl mx-auto space-y-6">
        {/* Header */}
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-3xl font-bold tracking-tight flex items-center gap-2">
              <Bell className="w-8 h-8" />
              告警中心
            </h1>
            <p className="text-muted-foreground mt-1">
              实时告警通知和历史记录
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Button variant="outline" size="sm">
              <Download className="w-4 h-4 mr-2" />
              导出
            </Button>
            <Button variant="outline" size="sm" onClick={() => setShowConfig(!showConfig)}>
              <Settings className="w-4 h-4 mr-2" />
              配置
            </Button>
          </div>
        </div>

        {/* 告警配置 */}
        {showConfig && (
          <Card>
            <CardHeader>
              <CardTitle>告警规则配置</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="space-y-4">
                <div>
                  <label className="text-sm font-medium mb-2 block">失败率阈值</label>
                  <div className="flex items-center gap-2">
                    <Input type="number" defaultValue="30" className="w-32" />
                    <span className="text-sm text-muted-foreground">%</span>
                  </div>
                  <p className="text-xs text-muted-foreground mt-1">
                    当同步失败率超过此值时触发告警
                  </p>
                </div>

                <div>
                  <label className="text-sm font-medium mb-2 block">队列积压阈值</label>
                  <div className="flex items-center gap-2">
                    <Input type="number" defaultValue="100" className="w-32" />
                    <span className="text-sm text-muted-foreground">个任务</span>
                  </div>
                  <p className="text-xs text-muted-foreground mt-1">
                    当队列长度超过此值时触发告警
                  </p>
                </div>

                <div>
                  <label className="text-sm font-medium mb-2 block">MQTT 重连次数阈值</label>
                  <div className="flex items-center gap-2">
                    <Input type="number" defaultValue="5" className="w-32" />
                    <span className="text-sm text-muted-foreground">次</span>
                  </div>
                  <p className="text-xs text-muted-foreground mt-1">
                    当 MQTT 重连次数超过此值时触发严重告警
                  </p>
                </div>

                <div>
                  <label className="text-sm font-medium mb-2 block">通知渠道</label>
                  <div className="space-y-2">
                    <label className="flex items-center gap-2">
                      <input type="checkbox" defaultChecked />
                      <span className="text-sm">界面通知</span>
                    </label>
                    <label className="flex items-center gap-2">
                      <input type="checkbox" />
                      <span className="text-sm">邮件通知</span>
                    </label>
                    <label className="flex items-center gap-2">
                      <input type="checkbox" />
                      <span className="text-sm">Webhook</span>
                    </label>
                  </div>
                </div>

                <div className="flex justify-end gap-2 pt-4">
                  <Button variant="outline" onClick={() => setShowConfig(false)}>
                    取消
                  </Button>
                  <Button>
                    保存配置
                  </Button>
                </div>
              </div>
            </CardContent>
          </Card>
        )}

        {/* 筛选器 */}
        <Card>
          <CardContent className="pt-6">
            <div className="flex items-center gap-4">
              <div className="flex-1">
                <div className="relative">
                  <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 w-4 h-4 text-gray-400" />
                  <Input
                    placeholder="搜索告警信息..."
                    value={searchTerm}
                    onChange={(e) => setSearchTerm(e.target.value)}
                    className="pl-10"
                  />
                </div>
              </div>
              <Select value={levelFilter} onValueChange={setLevelFilter}>
                <SelectTrigger className="w-[180px]">
                  <SelectValue placeholder="告警级别" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">全部级别</SelectItem>
                  <SelectItem value="critical">严重</SelectItem>
                  <SelectItem value="error">错误</SelectItem>
                  <SelectItem value="warning">警告</SelectItem>
                  <SelectItem value="info">信息</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </CardContent>
        </Card>

        {/* 告警面板 */}
        <AlertPanel maxAlerts={50} showHistory={true} />

        {/* 统计信息 */}
        <div className="grid gap-4 md:grid-cols-4">
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium">严重告警</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold text-red-600">0</div>
              <p className="text-xs text-muted-foreground mt-1">过去 24 小时</p>
            </CardContent>
          </Card>

          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium">错误告警</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold text-red-500">0</div>
              <p className="text-xs text-muted-foreground mt-1">过去 24 小时</p>
            </CardContent>
          </Card>

          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium">警告告警</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold text-yellow-500">0</div>
              <p className="text-xs text-muted-foreground mt-1">过去 24 小时</p>
            </CardContent>
          </Card>

          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium">平均响应时间</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold">N/A</div>
              <p className="text-xs text-muted-foreground mt-1">告警到处理</p>
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  )
}
