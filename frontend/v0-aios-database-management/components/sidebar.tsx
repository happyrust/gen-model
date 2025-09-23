"use client"

import Link from "next/link"
import { usePathname } from "next/navigation"
import {
  Activity,
  Box,
  Database,
  Hammer,
  Settings,
  Zap,
} from "lucide-react"
import { cn } from "@/lib/utils"

const navItems = [
  { label: "首页", href: "/", icon: Activity },
  { label: "部署站点", href: "/sites", icon: Database },
  { label: "生成模型", href: "/model", icon: Hammer },
  { label: "解析向导", href: "/wizard", icon: Zap },
  { label: "配置管理", href: "/config", icon: Settings },
]

export function Sidebar() {
  const pathname = usePathname()

  return (
    <aside className="hidden h-screen w-60 shrink-0 flex-col border-r border-border bg-sidebar text-sidebar-foreground md:flex">
      <div className="flex items-center gap-3 px-5 py-6">
        <Box className="h-7 w-7 text-sidebar-primary" />
        <div>
          <p className="text-base font-semibold">AIOS 管理</p>
          <p className="text-xs text-sidebar-foreground/60">模型与任务中心</p>
        </div>
      </div>
      <nav className="flex-1 space-y-1 px-3">
        {navItems.map((item) => {
          const active = pathname === item.href
          return (
            <Link
              key={item.href}
              href={item.href}
              className={cn(
                "flex items-center gap-2 rounded-lg px-3 py-2 text-sm transition-colors",
                active
                  ? "bg-sidebar-accent text-sidebar-accent-foreground"
                  : "text-sidebar-foreground/70 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
              )}
            >
              <item.icon className="h-4 w-4" />
              <span>{item.label}</span>
            </Link>
          )
        })}
      </nav>
      <div className="px-4 pb-6 pt-2 text-xs text-sidebar-foreground/50">
        © {new Date().getFullYear()} AIOS
      </div>
    </aside>
  )
}
