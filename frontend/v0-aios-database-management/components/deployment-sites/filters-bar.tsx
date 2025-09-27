"use client"

import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Search } from "lucide-react"

interface FiltersBarProps {
  search: string
  status: string
  environment: string
  owner: string
  sort: string
  pageSize: number
  total: number
  viewMode: "grid" | "list"
  onSearchChange?: (search: string) => void
  onStatusChange?: (status: string) => void
  onEnvironmentChange?: (environment: string) => void
  onOwnerChange?: (owner: string) => void
  onSortChange?: (sort: string) => void
  onPageSizeChange?: (pageSize: string) => void
  onViewModeChange?: (mode: "grid" | "list") => void
}

export function FiltersBar({
  search,
  status,
  environment,
  owner,
  sort,
  pageSize,
  total,
  viewMode,
  onSearchChange,
  onStatusChange,
  onEnvironmentChange,
  onOwnerChange,
  onSortChange,
  onPageSizeChange,
  onViewModeChange,
}: FiltersBarProps) {
  const handleSearchChange = (value: string) => {
    onSearchChange?.(value)
  }

  const handleOwnerChange = (value: string) => {
    onOwnerChange?.(value)
  }

  const handleStatusChange = (value: string) => {
    onStatusChange?.(value === "all-status" ? "" : value)
  }

  const handleEnvironmentChange = (value: string) => {
    onEnvironmentChange?.(value === "all-env" ? "" : value)
  }

  const handleSortChange = (value: string) => {
    onSortChange?.(value)
  }

  const handlePageSizeChange = (value: string) => {
    onPageSizeChange?.(value)
  }

  const handleViewModeChange = (mode: "grid" | "list") => {
    onViewModeChange?.(mode)
  }

  return (
    <div className="space-y-4">
      {/* Main Filters */}
      <div className="flex items-center justify-between">
        <div className="flex items-center space-x-4">
          <div className="relative">
            <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              placeholder="搜索名称/描述/负责人"
              className="pl-10 w-80"
              value={search}
              onChange={(e) => handleSearchChange(e.target.value)}
            />
          </div>

          <Select value={status || "all-status"} onValueChange={handleStatusChange}>
            <SelectTrigger className="w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all-status">全部状态</SelectItem>
              <SelectItem value="running">运行中</SelectItem>
              <SelectItem value="deploying">部署中</SelectItem>
              <SelectItem value="configuring">配置中</SelectItem>
              <SelectItem value="failed">失败</SelectItem>
              <SelectItem value="paused">已暂停</SelectItem>
              <SelectItem value="stopped">已停止</SelectItem>
            </SelectContent>
          </Select>

          <Select value={environment || "all-env"} onValueChange={handleEnvironmentChange}>
            <SelectTrigger className="w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all-env">全部环境</SelectItem>
              <SelectItem value="dev">开发环境</SelectItem>
              <SelectItem value="test">测试环境</SelectItem>
              <SelectItem value="staging">预发布</SelectItem>
              <SelectItem value="prod">生产环境</SelectItem>
            </SelectContent>
          </Select>

          <Input
            placeholder="负责人"
            className="w-24"
            value={owner}
            onChange={(e) => handleOwnerChange(e.target.value)}
          />
        </div>
      </div>

      {/* Sort and Pagination Options */}
      <div className="flex items-center justify-between">
        <div className="flex items-center space-x-4">
          <span className="text-sm text-muted-foreground">排序</span>
          <Select value={sort} onValueChange={handleSortChange}>
            <SelectTrigger className="w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="updated_at:desc">最近更新</SelectItem>
              <SelectItem value="name:asc">名称 (A→Z)</SelectItem>
              <SelectItem value="name:desc">名称 (Z→A)</SelectItem>
              <SelectItem value="created_at:asc">创建时间 (旧→新)</SelectItem>
              <SelectItem value="created_at:desc">创建时间 (新→旧)</SelectItem>
            </SelectContent>
          </Select>

          <span className="text-sm text-muted-foreground">每页</span>
          <Select value={String(pageSize)} onValueChange={handlePageSizeChange}>
            <SelectTrigger className="w-16">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="12">12</SelectItem>
              <SelectItem value="24">24</SelectItem>
              <SelectItem value="48">48</SelectItem>
              <SelectItem value="96">96</SelectItem>
            </SelectContent>
          </Select>
        </div>

        <div className="flex items-center gap-2">
          <div className="text-sm text-muted-foreground">共 {total} 个站点</div>
          <div className="inline-flex rounded overflow-hidden border border-border">
            <button
              type="button"
              className={`px-3 py-1 text-sm ${viewMode === "grid" ? "bg-muted" : "bg-background"}`}
              onClick={() => handleViewModeChange("grid")}
            >
              网格
            </button>
            <button
              type="button"
              className={`px-3 py-1 text-sm ${viewMode === "list" ? "bg-muted" : "bg-background"}`}
              onClick={() => handleViewModeChange("list")}
            >
              列表
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
