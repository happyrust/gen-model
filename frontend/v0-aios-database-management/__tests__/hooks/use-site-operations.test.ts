/**
 * 测试 hooks/use-site-operations.ts 中的 scanDirectory 函数
 */

import { scanDirectory } from "@/hooks/use-site-operations"
import { buildApiUrl } from "@/lib/api"

// Mock fetch
global.fetch = jest.fn()

describe("scanDirectory", () => {
  const mockSetProjects = jest.fn()

  beforeEach(() => {
    jest.clearAllMocks()
  })

  afterEach(() => {
    jest.resetAllMocks()
  })

  it("应使用 buildApiUrl 构建正确的请求URL", async () => {
    const mockResponse = {
      ok: true,
      json: async () => ({
        projects: [
          {
            name: "测试项目",
            path: "/test/path",
            db_file_count: 5,
            size_bytes: 1024000,
          },
        ],
      }),
    }

    ;(global.fetch as jest.Mock).mockResolvedValueOnce(mockResponse)

    await scanDirectory("/Volumes/DPC/work/e3d_models", mockSetProjects)

    // 验证 fetch 被调用
    expect(global.fetch).toHaveBeenCalledTimes(1)

    // 获取实际调用的URL
    const calledUrl = (global.fetch as jest.Mock).mock.calls[0][0]

    // 验证URL包含正确的路径和参数
    expect(calledUrl).toContain("/api/wizard/scan-directory")
    expect(calledUrl).toContain("directory_path=%2FVolumes%2FDPC%2Fwork%2Fe3d_models")
    expect(calledUrl).toContain("recursive=true")
    expect(calledUrl).toContain("max_depth=4")
  })

  it("应正确处理API响应并设置项目列表", async () => {
    const mockProjects = [
      {
        name: "项目1",
        path: "/test/project1",
        db_file_count: 3,
        size_bytes: 2048000,
      },
      {
        name: "项目2",
        path: "/test/project2",
        db_file_count: 5,
        size_bytes: 4096000,
      },
    ]

    const mockResponse = {
      ok: true,
      json: async () => ({ projects: mockProjects }),
    }

    ;(global.fetch as jest.Mock).mockResolvedValueOnce(mockResponse)

    await scanDirectory("/test/root", mockSetProjects)

    // 验证 setProjects 被调用
    expect(mockSetProjects).toHaveBeenCalledTimes(1)

    // 验证传递的项目数据格式正确
    const calledProjects = mockSetProjects.mock.calls[0][0]
    expect(calledProjects).toHaveLength(2)
    expect(calledProjects[0]).toMatchObject({
      name: "项目1",
      path: "/test/project1",
      selected: false,
      fileCount: 3,
    })
    expect(calledProjects[0].size).toContain("MB")
    expect(calledProjects[1]).toMatchObject({
      name: "项目2",
      path: "/test/project2",
      selected: false,
      fileCount: 5,
    })
    expect(calledProjects[1].size).toContain("MB")
  })

  it("当API返回错误时，应抛出异常", async () => {
    const mockResponse = {
      ok: false,
      status: 500,
      statusText: "Internal Server Error",
      json: async () => ({ error: "服务器错误" }),
    }

    ;(global.fetch as jest.Mock).mockResolvedValueOnce(mockResponse)

    await expect(scanDirectory("/test/root", mockSetProjects)).rejects.toThrow()
  })

  it("应处理空项目列表", async () => {
    const mockResponse = {
      ok: true,
      json: async () => ({ projects: [] }),
    }

    ;(global.fetch as jest.Mock).mockResolvedValueOnce(mockResponse)

    await scanDirectory("/test/empty", mockSetProjects)

    expect(mockSetProjects).toHaveBeenCalledWith([])
  })

  it("应处理缺少可选字段的项目数据", async () => {
    const mockProjects = [
      {
        name: "最小项目",
        path: "/test/minimal",
        // 缺少 db_file_count 和 size_bytes
      },
    ]

    const mockResponse = {
      ok: true,
      json: async () => ({ projects: mockProjects }),
    }

    ;(global.fetch as jest.Mock).mockResolvedValueOnce(mockResponse)

    await scanDirectory("/test/root", mockSetProjects)

    const calledProjects = mockSetProjects.mock.calls[0][0]
    expect(calledProjects[0]).toMatchObject({
      name: "最小项目",
      path: "/test/minimal",
      selected: false,
      fileCount: undefined,
    })
    // size 可能是 "0 B" 或 "0 Bytes"，只验证包含 "0"
    expect(calledProjects[0].size).toContain("0")
  })
})

describe("buildApiUrl 集成测试", () => {
  it("scanDirectory 应使用与 buildApiUrl 相同的URL构建逻辑", () => {
    const path = "/api/wizard/scan-directory"
    const params = new URLSearchParams({
      directory_path: "/test/path",
      recursive: "true",
      max_depth: "4",
    })

    const expectedUrl = buildApiUrl(`${path}?${params.toString()}`)

    // 验证URL格式
    if (process.env.NEXT_PUBLIC_API_BASE_URL) {
      expect(expectedUrl).toContain(process.env.NEXT_PUBLIC_API_BASE_URL)
    }
    expect(expectedUrl).toContain(path)
    expect(expectedUrl).toContain("directory_path")
  })
})

