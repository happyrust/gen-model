/**
 * 站点过滤器状态管理 Hook
 *
 * 管理部署站点列表的所有过滤条件和分页状态
 */

import { useMemo, useState } from "react"

export interface SiteFilters {
  search: string
  status: string
  environment: string
  owner: string
  sort: string
  page: number
  perPage: number
  viewMode: "grid" | "list"
}

export interface SiteFiltersActions {
  setSearch: (value: string) => void
  setStatus: (value: string) => void
  setEnvironment: (value: string) => void
  setOwner: (value: string) => void
  setSort: (value: string) => void
  setPage: (value: number) => void
  setPerPage: (value: number) => void
  setViewMode: (mode: "grid" | "list") => void
  resetPage: () => void
}

/**
 * 使用站点过滤器
 */
export function useSiteFilters() {
  const [search, setSearch] = useState("")
  const [status, setStatus] = useState("")
  const [environment, setEnvironment] = useState("")
  const [owner, setOwner] = useState("")
  const [sort, setSort] = useState("updated_at:desc")
  const [page, setPage] = useState(1)
  const [perPage, setPerPage] = useState(12)
  const [viewMode, setViewMode] = useState<"grid" | "list">("grid")

  const resetPage = () => setPage(1)

  const filters: SiteFilters = useMemo(() => ({
    search,
    status,
    environment,
    owner,
    sort,
    page,
    perPage,
    viewMode,
  }), [search, status, environment, owner, sort, page, perPage, viewMode])

  const actions: SiteFiltersActions = {
    setSearch: (value: string) => {
      setSearch(value)
      resetPage()
    },
    setStatus: (value: string) => {
      setStatus(value)
      resetPage()
    },
    setEnvironment: (value: string) => {
      setEnvironment(value)
      resetPage()
    },
    setOwner: (value: string) => {
      setOwner(value)
      resetPage()
    },
    setSort,
    setPage,
    setPerPage: (value: number) => {
      setPerPage(value)
      resetPage()
    },
    setViewMode,
    resetPage,
  }

  return { filters, actions }
}
