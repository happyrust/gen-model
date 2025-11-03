/**
 * 测试 lib/api.ts 中的 buildApiUrl 函数
 */

import { buildApiUrl } from "@/lib/api"

describe("buildApiUrl", () => {
  it("当 NEXT_PUBLIC_API_BASE_URL 未设置时，应返回原始路径", () => {
    // 在测试环境中，NEXT_PUBLIC_API_BASE_URL 默认未设置
    const result = buildApiUrl("/api/tasks")
    expect(result).toBe("/api/tasks")
  })

  it("应处理带查询参数的路径", () => {
    const result = buildApiUrl("/api/wizard/scan-directory?directory_path=/test&recursive=true")
    // 验证路径包含正确的部分
    expect(result).toContain("/api/wizard/scan-directory")
    expect(result).toContain("directory_path=/test")
    expect(result).toContain("recursive=true")
  })

  it("当路径不以斜杠开头时，应抛出错误", () => {
    expect(() => buildApiUrl("api/tasks")).toThrow("API 路径必须以 / 开头")
  })

  it("应处理复杂的API路径", () => {
    const result = buildApiUrl("/api/tasks/123/logs?level=Error&limit=100")
    expect(result).toContain("/api/tasks/123/logs")
    expect(result).toContain("level=Error")
    expect(result).toContain("limit=100")
  })

  it("应正确构建扫描目录的URL", () => {
    const params = new URLSearchParams({
      directory_path: "/Volumes/DPC/work/e3d_models",
      recursive: "true",
      max_depth: "4",
    })
    const result = buildApiUrl(`/api/wizard/scan-directory?${params.toString()}`)

    expect(result).toContain("/api/wizard/scan-directory")
    expect(result).toContain("directory_path")
    expect(result).toContain("recursive=true")
    expect(result).toContain("max_depth=4")
  })
})

