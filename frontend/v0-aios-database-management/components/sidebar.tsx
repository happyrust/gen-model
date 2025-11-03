"use client"

import type React from "react"

import { useState } from "react"
import { useRouter, usePathname } from "next/navigation"
import { Database, Server, Settings, Activity, Zap, Shield, BarChart3, ChevronDown, ChevronRight, Network, Package, Eye, TestTube, Plus, Monitor } from "lucide-react"
import { cn } from "@/lib/utils"
import { ThemeToggle } from "@/components/theme-toggle"
import { NodeStatusBadge } from "@/components/node-status-badge"

interface NavItem {
  title: string
  icon: React.ComponentType<{ className?: string }>
  href: string
  isActive?: boolean
  children?: NavItem[]
}

const navigationItems: NavItem[] = [
  {
    title: "首页",
    icon: Activity,
    href: "/",
    isActive: true,
  },
  {
    title: "仪表板",
    icon: BarChart3,
    href: "/dashboard",
  },
  {
    title: "任务管理",
    icon: Server,
    href: "/task-monitor",
    children: [
      { title: "快速向导", icon: Zap, href: "/wizard" },
      { title: "创建任务", icon: Plus, href: "/task-creation" },
      { title: "任务监控", icon: Monitor, href: "/task-monitor" },
      { title: "批量任务", icon: Zap, href: "/tasks/batch" },
      { title: "定时任务", icon: Activity, href: "/tasks/scheduled" },
    ],
  },
  {
    title: "部署站点",
    icon: Database,
    href: "/deployment-sites",
  },
]

const systemItems: NavItem[] = [
  {
    title: "配置管理",
    icon: Settings,
    href: "/config",
  },
  {
    title: "系统状态",
    icon: Activity,
    href: "/status",
  },
  {
    title: "空间查询",
    icon: Shield,
    href: "/spatial",
    children: [
      { title: "可视化查询", icon: Eye, href: "/spatial-visualization" },
    ],
  },
]

const toolItems: NavItem[] = [
  {
    title: "数据库连接",
    icon: Database,
    href: "/database",
  },
  {
    title: "解析向导",
    icon: Zap,
    href: "/wizard",
  },
  {
    title: "XKT 生成器",
    icon: Package,
    href: "/xkt-generator",
  },
  {
    title: "XKT 查看器",
    icon: Eye,
    href: "/xkt-viewer",
  },
  {
    title: "XKT 测试",
    icon: TestTube,
    href: "/xkt-test",
  },
  {
    title: "异地协同",
    icon: Network,
    href: "/collaboration",
  },
  {
    title: "异地环境",
    icon: Server,
    href: "/remote",
  },
]

interface NavItemComponentProps {
  item: NavItem
  level?: number
}

function NavItemComponent({ item, level = 0 }: NavItemComponentProps) {
  const [isExpanded, setIsExpanded] = useState(false)
  const router = useRouter()
  const pathname = usePathname()
  const hasChildren = item.children && item.children.length > 0
  const isCurrentPage = pathname === item.href

  const handleClick = () => {
    if (hasChildren) {
      setIsExpanded(!isExpanded)
    } else {
      router.push(item.href)
    }
  }

  return (
    <div>
      <button
        onClick={handleClick}
        className={cn(
          "w-full flex items-center gap-3 px-3 py-2 rounded-lg text-left transition-colors",
          level > 0 && "ml-4",
          isCurrentPage || item.isActive
            ? "bg-sidebar-accent text-sidebar-accent-foreground"
            : "text-sidebar-foreground/70 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
        )}
      >
        <item.icon className="h-4 w-4 flex-shrink-0" />
        <span className="flex-1">{item.title}</span>
        {hasChildren && (isExpanded ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />)}
      </button>

      {hasChildren && isExpanded && (
        <div className="mt-1 space-y-1">
          {item.children?.map((child, index) => (
            <NavItemComponent key={index} item={child} level={level + 1} />
          ))}
        </div>
      )}
    </div>
  )
}

export function Sidebar() {
  return (
    <div className="fixed left-0 top-0 h-full w-64 bg-sidebar border-r border-sidebar-border">
      <div className="p-6">
        {/* Logo */}
        <div className="flex items-center justify-between mb-8">
          <div className="flex items-center gap-2">
            <div className="w-10 h-10 bg-sidebar-primary rounded-lg flex items-center justify-center">
              <Database className="h-6 w-6 text-sidebar-primary-foreground" />
            </div>
            <div>
              <h1 className="text-xl font-bold text-sidebar-foreground">AIOS</h1>
              <p className="text-sm text-sidebar-foreground/60">数据库管理平台</p>
            </div>
          </div>
          {/* Theme Toggle Button */}
          <ThemeToggle />
        </div>

        {/* Navigation */}
        <nav className="space-y-6">
          {/* Main Navigation */}
          <div>
            <div className="text-xs font-medium text-sidebar-foreground/40 uppercase tracking-wider mb-3">导航</div>
            <div className="space-y-1">
              {navigationItems.map((item, index) => (
                <NavItemComponent key={index} item={item} />
              ))}
            </div>
          </div>

          {/* System Section */}
          <div>
            <div className="text-xs font-medium text-sidebar-foreground/40 uppercase tracking-wider mb-3">系统</div>
            <div className="space-y-1">
              {systemItems.map((item, index) => (
                <NavItemComponent key={index} item={item} />
              ))}
            </div>
          </div>

          {/* Tools Section */}
          <div>
            <div className="text-xs font-medium text-sidebar-foreground/40 uppercase tracking-wider mb-3">工具</div>
            <div className="space-y-1">
              {toolItems.map((item, index) => (
                <NavItemComponent key={index} item={item} />
              ))}
            </div>
          </div>
        </nav>

        {/* Footer */}
        <div className="absolute bottom-6 left-6 right-6">
          <div className="p-3 bg-sidebar-accent rounded-lg space-y-3">
            <div>
              <div className="flex items-center gap-2 mb-2">
                <div className="w-2 h-2 bg-success rounded-full"></div>
                <span className="text-sm font-medium text-sidebar-accent-foreground">系统状态</span>
              </div>
              <p className="text-xs text-sidebar-accent-foreground/70">所有服务正常运行</p>
            </div>
            <div className="border-t border-sidebar-border pt-2">
              <NodeStatusBadge />
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
