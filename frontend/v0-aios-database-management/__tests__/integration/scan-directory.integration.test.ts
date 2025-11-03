/**
 * 集成测试：验证扫描目录功能
 * 
 * 这个测试会实际调用后端API来验证扫描功能是否正常工作
 */

import { scanDirectory } from "@/hooks/use-site-operations"
import { buildApiUrl } from "@/lib/api"

describe("扫描目录集成测试", () => {
  // 跳过集成测试（除非明确启用）
  const runIntegrationTests = process.env.RUN_INTEGRATION_TESTS === "true"
  const testCondition = runIntegrationTests ? it : it.skip

  testCondition("应能够扫描真实目录 /Volumes/DPC/work/e3d_models", async () => {
    const mockSetProjects = jest.fn()
    const testDirectory = "/Volumes/DPC/work/e3d_models"

    try {
      await scanDirectory(testDirectory, mockSetProjects)

      // 验证 setProjects 被调用
      expect(mockSetProjects).toHaveBeenCalled()

      // 获取扫描到的项目
      const projects = mockSetProjects.mock.calls[0][0]

      // 验证返回的是数组
      expect(Array.isArray(projects)).toBe(true)

      // 如果有项目，验证项目结构
      if (projects.length > 0) {
        const firstProject = projects[0]
        expect(firstProject).toHaveProperty("name")
        expect(firstProject).toHaveProperty("path")
        expect(firstProject).toHaveProperty("selected")
        expect(firstProject).toHaveProperty("fileCount")
        expect(firstProject).toHaveProperty("size")

        console.log(`✅ 成功扫描到 ${projects.length} 个项目`)
        console.log("第一个项目:", firstProject)
      } else {
        console.log("⚠️ 目录中没有找到项目")
      }
    } catch (error) {
      console.error("❌ 扫描目录失败:", error)
      throw error
    }
  }, 30000) // 30秒超时

  testCondition("应正确构建扫描目录的API URL", () => {
    const params = new URLSearchParams({
      directory_path: "/Volumes/DPC/work/e3d_models",
      recursive: "true",
      max_depth: "4",
    })

    const url = buildApiUrl(`/api/wizard/scan-directory?${params.toString()}`)

    console.log("构建的API URL:", url)

    // 验证URL包含必要的部分
    expect(url).toContain("/api/wizard/scan-directory")
    expect(url).toContain("directory_path")
    expect(url).toContain("recursive=true")
    expect(url).toContain("max_depth=4")
  })

  testCondition("应能处理不存在的目录", async () => {
    const mockSetProjects = jest.fn()
    const nonExistentDirectory = "/path/that/does/not/exist"

    await expect(
      scanDirectory(nonExistentDirectory, mockSetProjects)
    ).rejects.toThrow()
  }, 10000)
})

describe("URL构建验证", () => {
  it("buildApiUrl 应正确处理扫描目录的URL", () => {
    const testCases = [
      {
        path: "/Volumes/DPC/work/e3d_models",
        expected: {
          contains: ["/api/wizard/scan-directory", "directory_path", "recursive=true"],
        },
      },
      {
        path: "/test/path with spaces",
        expected: {
          contains: ["/api/wizard/scan-directory", "directory_path"],
        },
      },
      {
        path: "/中文路径/测试",
        expected: {
          contains: ["/api/wizard/scan-directory", "directory_path"],
        },
      },
    ]

    testCases.forEach(({ path, expected }) => {
      const params = new URLSearchParams({
        directory_path: path,
        recursive: "true",
        max_depth: "4",
      })

      const url = buildApiUrl(`/api/wizard/scan-directory?${params.toString()}`)

      expected.contains.forEach((substring) => {
        expect(url).toContain(substring)
      })
    })
  })
})

