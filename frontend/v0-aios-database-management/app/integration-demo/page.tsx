"use client"

import { useState } from "react"
import { useRouter } from "next/navigation"
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { 
  Zap, 
  Settings, 
  Database, 
  Play, 
  ArrowRight,
  CheckCircle2,
  Clock,
  Package
} from "lucide-react"
import { Sidebar } from "@/components/sidebar"

export default function IntegrationDemoPage() {
  const router = useRouter()
  const [activeTab, setActiveTab] = useState("overview")

  const features = [
    {
      title: "快速向导模式",
      description: "保持原有向导的简洁性，支持快速创建常见任务",
      icon: <Zap className="h-6 w-6" />,
      color: "bg-blue-100 text-blue-800",
      path: "/wizard"
    },
    {
      title: "高级创建模式",
      description: "提供完整的任务创建向导，支持详细参数配置",
      icon: <Settings className="h-6 w-6" />,
      color: "bg-green-100 text-green-800",
      path: "/task-creation"
    },
    {
      title: "任务监控",
      description: "实时监控任务状态，支持任务操作和管理",
      icon: <Database className="h-6 w-6" />,
      color: "bg-purple-100 text-purple-800",
      path: "/task-monitor"
    }
  ]

  const taskTypes = [
    { name: "数据解析任务", description: "解析PDMS数据库文件", icon: <Database className="h-4 w-4" /> },
    { name: "模型生成任务", description: "生成3D模型和网格文件", icon: <Package className="h-4 w-4" /> },
    { name: "空间树生成任务", description: "构建空间索引树", icon: <Settings className="h-4 w-4" /> },
    { name: "全量同步任务", description: "完整同步所有数据", icon: <Clock className="h-4 w-4" /> },
    { name: "增量同步任务", description: "仅同步变更数据", icon: <Clock className="h-4 w-4" /> }
  ]

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      
      <div className="ml-64 p-8">
        <div className="max-w-6xl mx-auto">
          {/* 页面头部 */}
          <div className="mb-8">
            <div className="flex items-center gap-3 mb-4">
              <div className="w-12 h-12 bg-primary/10 rounded-lg flex items-center justify-center">
                <Zap className="h-6 w-6 text-primary" />
              </div>
              <div>
                <h1 className="text-3xl font-bold text-foreground">向导与任务创建集成演示</h1>
                <p className="text-muted-foreground">展示快速向导与高级任务创建功能的完美结合</p>
              </div>
            </div>
          </div>

          <Tabs value={activeTab} onValueChange={setActiveTab} className="space-y-6">
            <TabsList className="grid w-full grid-cols-3">
              <TabsTrigger value="overview">功能概览</TabsTrigger>
              <TabsTrigger value="comparison">功能对比</TabsTrigger>
              <TabsTrigger value="demo">在线演示</TabsTrigger>
            </TabsList>

            <TabsContent value="overview" className="space-y-6">
              <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
                {features.map((feature, index) => (
                  <Card key={index} className="hover:shadow-lg transition-shadow cursor-pointer" onClick={() => router.push(feature.path)}>
                    <CardHeader>
                      <div className="flex items-center gap-3">
                        <div className="w-10 h-10 bg-primary/10 rounded-lg flex items-center justify-center">
                          {feature.icon}
                        </div>
                        <div>
                          <CardTitle className="text-lg">{feature.title}</CardTitle>
                          <CardDescription>{feature.description}</CardDescription>
                        </div>
                      </div>
                    </CardHeader>
                    <CardContent>
                      <Button className="w-full" variant="outline">
                        <ArrowRight className="h-4 w-4 mr-2" />
                        立即体验
                      </Button>
                    </CardContent>
                  </Card>
                ))}
              </div>

              <Card>
                <CardHeader>
                  <CardTitle>集成优势</CardTitle>
                  <CardDescription>快速向导与高级创建功能的完美结合</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                    <div className="space-y-4">
                      <h4 className="font-semibold text-green-600">快速向导模式</h4>
                      <ul className="space-y-2 text-sm text-muted-foreground">
                        <li className="flex items-center gap-2">
                          <CheckCircle2 className="h-4 w-4 text-green-500" />
                          保持原有简洁性
                        </li>
                        <li className="flex items-center gap-2">
                          <CheckCircle2 className="h-4 w-4 text-green-500" />
                          支持批量任务创建
                        </li>
                        <li className="flex items-center gap-2">
                          <CheckCircle2 className="h-4 w-4 text-green-500" />
                          快速配置常见任务
                        </li>
                        <li className="flex items-center gap-2">
                          <CheckCircle2 className="h-4 w-4 text-green-500" />
                          一键跳转监控页面
                        </li>
                      </ul>
                    </div>
                    <div className="space-y-4">
                      <h4 className="font-semibold text-blue-600">高级创建模式</h4>
                      <ul className="space-y-2 text-sm text-muted-foreground">
                        <li className="flex items-center gap-2">
                          <CheckCircle2 className="h-4 w-4 text-blue-500" />
                          4步向导流程
                        </li>
                        <li className="flex items-center gap-2">
                          <CheckCircle2 className="h-4 w-4 text-blue-500" />
                          详细参数配置
                        </li>
                        <li className="flex items-center gap-2">
                          <CheckCircle2 className="h-4 w-4 text-blue-500" />
                          资源需求预估
                        </li>
                        <li className="flex items-center gap-2">
                          <CheckCircle2 className="h-4 w-4 text-blue-500" />
                          任务预览确认
                        </li>
                      </ul>
                    </div>
                  </div>
                </CardContent>
              </Card>
            </TabsContent>

            <TabsContent value="comparison" className="space-y-6">
              <Card>
                <CardHeader>
                  <CardTitle>功能对比表</CardTitle>
                  <CardDescription>快速向导与高级创建模式的详细对比</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="overflow-x-auto">
                    <table className="w-full border-collapse">
                      <thead>
                        <tr className="border-b">
                          <th className="text-left p-3 font-semibold">功能特性</th>
                          <th className="text-center p-3 font-semibold">快速向导</th>
                          <th className="text-center p-3 font-semibold">高级创建</th>
                        </tr>
                      </thead>
                      <tbody>
                        <tr className="border-b">
                          <td className="p-3">多任务创建</td>
                          <td className="text-center p-3">
                            <Badge className="bg-green-100 text-green-800">✅ 支持</Badge>
                          </td>
                          <td className="text-center p-3">
                            <Badge className="bg-gray-100 text-gray-800">❌ 单任务</Badge>
                          </td>
                        </tr>
                        <tr className="border-b">
                          <td className="p-3">参数配置</td>
                          <td className="text-center p-3">
                            <Badge className="bg-blue-100 text-blue-800">基础</Badge>
                          </td>
                          <td className="text-center p-3">
                            <Badge className="bg-green-100 text-green-800">完整</Badge>
                          </td>
                        </tr>
                        <tr className="border-b">
                          <td className="p-3">任务预览</td>
                          <td className="text-center p-3">
                            <Badge className="bg-gray-100 text-gray-800">❌ 无</Badge>
                          </td>
                          <td className="text-center p-3">
                            <Badge className="bg-green-100 text-green-800">✅ 支持</Badge>
                          </td>
                        </tr>
                        <tr className="border-b">
                          <td className="p-3">资源预估</td>
                          <td className="text-center p-3">
                            <Badge className="bg-gray-100 text-gray-800">❌ 无</Badge>
                          </td>
                          <td className="text-center p-3">
                            <Badge className="bg-green-100 text-green-800">✅ 支持</Badge>
                          </td>
                        </tr>
                        <tr className="border-b">
                          <td className="p-3">模板支持</td>
                          <td className="text-center p-3">
                            <Badge className="bg-gray-100 text-gray-800">❌ 无</Badge>
                          </td>
                          <td className="text-center p-3">
                            <Badge className="bg-green-100 text-green-800">✅ 支持</Badge>
                          </td>
                        </tr>
                        <tr className="border-b">
                          <td className="p-3">适用场景</td>
                          <td className="text-center p-3">
                            <Badge className="bg-blue-100 text-blue-800">快速批量</Badge>
                          </td>
                          <td className="text-center p-3">
                            <Badge className="bg-purple-100 text-purple-800">精细配置</Badge>
                          </td>
                        </tr>
                      </tbody>
                    </table>
                  </div>
                </CardContent>
              </Card>
            </TabsContent>

            <TabsContent value="demo" className="space-y-6">
              <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                <Card>
                  <CardHeader>
                    <CardTitle className="flex items-center gap-2">
                      <Zap className="h-5 w-5" />
                      快速向导演示
                    </CardTitle>
                    <CardDescription>体验快速创建多个任务的便捷流程</CardDescription>
                  </CardHeader>
                  <CardContent className="space-y-4">
                    <div className="space-y-2">
                      <h4 className="font-semibold">支持的任务类型：</h4>
                      <div className="space-y-1">
                        {taskTypes.map((type, index) => (
                          <div key={index} className="flex items-center gap-2 text-sm">
                            {type.icon}
                            <span>{type.name}</span>
                          </div>
                        ))}
                      </div>
                    </div>
                    <Button 
                      className="w-full" 
                      onClick={() => router.push('/wizard')}
                    >
                      <Play className="h-4 w-4 mr-2" />
                      开始快速向导
                    </Button>
                  </CardContent>
                </Card>

                <Card>
                  <CardHeader>
                    <CardTitle className="flex items-center gap-2">
                      <Settings className="h-5 w-5" />
                      高级创建演示
                    </CardTitle>
                    <CardDescription>体验完整的任务创建向导流程</CardDescription>
                  </CardHeader>
                  <CardContent className="space-y-4">
                    <div className="space-y-2">
                      <h4 className="font-semibold">向导步骤：</h4>
                      <div className="space-y-1 text-sm">
                        <div className="flex items-center gap-2">
                          <div className="w-6 h-6 bg-primary rounded-full flex items-center justify-center text-primary-foreground text-xs font-bold">1</div>
                          <span>基础信息配置</span>
                        </div>
                        <div className="flex items-center gap-2">
                          <div className="w-6 h-6 bg-primary rounded-full flex items-center justify-center text-primary-foreground text-xs font-bold">2</div>
                          <span>选择部署站点</span>
                        </div>
                        <div className="flex items-center gap-2">
                          <div className="w-6 h-6 bg-primary rounded-full flex items-center justify-center text-primary-foreground text-xs font-bold">3</div>
                          <span>配置任务参数</span>
                        </div>
                        <div className="flex items-center gap-2">
                          <div className="w-6 h-6 bg-primary rounded-full flex items-center justify-center text-primary-foreground text-xs font-bold">4</div>
                          <span>预览和确认</span>
                        </div>
                      </div>
                    </div>
                    <Button 
                      className="w-full" 
                      variant="outline"
                      onClick={() => router.push('/task-creation')}
                    >
                      <ArrowRight className="h-4 w-4 mr-2" />
                      开始高级创建
                    </Button>
                  </CardContent>
                </Card>
              </div>

              <Card>
                <CardHeader>
                  <CardTitle>任务监控</CardTitle>
                  <CardDescription>创建任务后，可以在监控页面查看任务状态和进度</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="flex items-center justify-between">
                    <div>
                      <h4 className="font-semibold mb-2">实时监控功能</h4>
                      <ul className="space-y-1 text-sm text-muted-foreground">
                        <li>• 任务状态实时更新</li>
                        <li>• 进度条和详细信息</li>
                        <li>• 任务操作（启动/停止/暂停）</li>
                        <li>• 错误日志和诊断</li>
                      </ul>
                    </div>
                    <Button onClick={() => router.push('/task-monitor')}>
                      查看任务监控
                    </Button>
                  </div>
                </CardContent>
              </Card>
            </TabsContent>
          </Tabs>
        </div>
      </div>
    </div>
  )
}







