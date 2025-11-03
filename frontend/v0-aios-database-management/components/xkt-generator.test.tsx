process.env.NEXT_PUBLIC_XKT_API_BASE_URL = 'http://localhost:8080'

/**
 * XKT生成器组件单元测试
 * 使用Jest和React Testing Library测试前端组件功能
 */

import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { XKTGenerator } from './xkt-generator'
import { toast } from 'sonner'
import { buildXktApiUrl } from '@/lib/xkt-api'

// Mock fetch API
global.fetch = jest.fn()

// Mock sonner toast
jest.mock('sonner', () => ({
  toast: {
    success: jest.fn(),
    error: jest.fn(),
    info: jest.fn()
  }
}))

describe('XKTGenerator Component', () => {
  beforeEach(() => {
    jest.clearAllMocks()
  })

  afterEach(() => {
    jest.restoreAllMocks()
  })

  describe('渲染测试', () => {
    test('应该正确渲染所有输入元素', () => {
      render(<XKTGenerator />)

      // 检查标题
      expect(screen.getByText('XKT 模型生成器')).toBeInTheDocument()
      expect(screen.getByText('生成用于3D查看器的XKT格式文件')).toBeInTheDocument()

      // 检查输入字段
      expect(screen.getByLabelText(/数据库号/)).toBeInTheDocument()
      expect(screen.getByLabelText(/参考号/)).toBeInTheDocument()
      expect(screen.getByLabelText(/压缩文件/)).toBeInTheDocument()

      // 检查按钮
      expect(screen.getByRole('button', { name: /生成XKT文件/ })).toBeInTheDocument()

      // 检查标签页
      expect(screen.getByRole('tab', { name: '生成XKT' })).toBeInTheDocument()
      expect(screen.getByRole('tab', { name: '历史记录' })).toBeInTheDocument()
    })

    test('默认值应该正确设置', () => {
      render(<XKTGenerator />)

      const dbnoInput = screen.getByLabelText(/数据库号/) as HTMLInputElement
      const refnoInput = screen.getByLabelText(/参考号/) as HTMLInputElement
      const compressSwitch = screen.getByRole('switch') as HTMLInputElement

      expect(dbnoInput.value).toBe('1112')
      expect(refnoInput.value).toBe('')
      expect(compressSwitch).toBeChecked()
    })
  })

  describe('输入验证测试', () => {
    test('没有数据库号时不应该允许生成', async () => {
      render(<XKTGenerator />)

      const dbnoInput = screen.getByLabelText(/数据库号/)
      const generateButton = screen.getByRole('button', { name: /生成XKT文件/ })

      // 清空数据库号
      await userEvent.clear(dbnoInput)

      // 点击生成按钮
      await userEvent.click(generateButton)

      // 应该显示错误提示
      expect(toast.error).toHaveBeenCalledWith('请输入数据库号')
      expect(fetch).not.toHaveBeenCalled()
    })

    test('应该接受有效的数据库号', async () => {
      render(<XKTGenerator />)

      const dbnoInput = screen.getByLabelText(/数据库号/)

      await userEvent.clear(dbnoInput)
      await userEvent.type(dbnoInput, '2000')

      expect(dbnoInput).toHaveValue(2000)
    })

    test('应该接受有效的参考号格式', async () => {
      render(<XKTGenerator />)

      const refnoInput = screen.getByLabelText(/参考号/)

      await userEvent.type(refnoInput, '17496/266203')

      expect(refnoInput).toHaveValue('17496/266203')
    })
  })

  describe('API调用测试', () => {
    test('成功生成XKT文件', async () => {
      const mockResponse = {
        success: true,
        filename: 'db1112_compressed_refno_17496_266203.xkt',
        url: '/api/xkt/download/db1112_compressed_refno_17496_266203.xkt',
        dbno: 1112,
        refno: '17496/266203'
      }

      ;(fetch as jest.Mock).mockResolvedValueOnce({
        ok: true,
        json: async () => mockResponse
      }).mockResolvedValueOnce({
        ok: true,
        blob: async () => new Blob(['test data'], { type: 'application/octet-stream' })
      })

      render(<XKTGenerator />)

      const generateButton = screen.getByRole('button', { name: /生成XKT文件/ })

      await userEvent.click(generateButton)

      await waitFor(() => {
        expect(fetch).toHaveBeenCalledWith(
          buildXktApiUrl('/api/xkt/generate'),
          expect.objectContaining({
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              dbno: 1112,
              refno: undefined,
              compress: true
            })
          })
        )
      })

      // 检查成功提示
      expect(toast.success).toHaveBeenCalledWith(
        `XKT文件生成成功: ${mockResponse.filename}`
      )
    })

    test('处理API错误', async () => {
      ;(fetch as jest.Mock).mockRejectedValueOnce(new Error('Network error'))

      render(<XKTGenerator />)

      const generateButton = screen.getByRole('button', { name: /生成XKT文件/ })

      await userEvent.click(generateButton)

      await waitFor(() => {
        expect(toast.error).toHaveBeenCalledWith('Network error')
      })
    })

    test('处理服务器错误响应', async () => {
      ;(fetch as jest.Mock).mockResolvedValueOnce({
        ok: false,
        text: async () => 'Internal Server Error'
      })

      render(<XKTGenerator />)

      const generateButton = screen.getByRole('button', { name: /生成XKT文件/ })

      await userEvent.click(generateButton)

      await waitFor(() => {
        expect(toast.error).toHaveBeenCalledWith('Internal Server Error')
      })
    })
  })

  describe('历史记录功能测试', () => {
    test('成功生成后应该添加到历史记录', async () => {
      const mockResponse = {
        success: true,
        filename: 'test.xkt',
        url: '/api/xkt/download/test.xkt',
        dbno: 1112
      }

      ;(fetch as jest.Mock).mockResolvedValueOnce({
        ok: true,
        json: async () => mockResponse
      }).mockResolvedValueOnce({
        ok: true,
        blob: async () => new Blob(['test'], { type: 'application/octet-stream' })
      })

      render(<XKTGenerator />)

      // 生成文件
      const generateButton = screen.getByRole('button', { name: /生成XKT文件/ })
      await userEvent.click(generateButton)

      // 切换到历史记录标签
      const historyTab = screen.getByRole('tab', { name: '历史记录' })
      await userEvent.click(historyTab)

      await waitFor(() => {
        expect(screen.getByText('test.xkt')).toBeInTheDocument()
        expect(screen.getByText('DB: 1112')).toBeInTheDocument()
      })
    })

    test('空历史记录应该显示提示', () => {
      render(<XKTGenerator />)

      const historyTab = screen.getByRole('tab', { name: '历史记录' })
      fireEvent.click(historyTab)

      expect(screen.getByText('暂无生成记录')).toBeInTheDocument()
    })
  })

  describe('下载功能测试', () => {
    test('点击下载按钮应该创建下载链接', async () => {
      const mockResponse = {
        success: true,
        filename: 'test.xkt',
        url: '/api/xkt/download/test.xkt',
        dbno: 1112
      }

      ;(fetch as jest.Mock).mockResolvedValueOnce({
        ok: true,
        json: async () => mockResponse
      }).mockResolvedValueOnce({
        ok: true,
        blob: async () => new Blob(['test'], { type: 'application/octet-stream' })
      })

      // Mock createElement and appendChild
      const createElementSpy = jest.spyOn(document, 'createElement')
      const appendChildSpy = jest.spyOn(document.body, 'appendChild')
      const removeChildSpy = jest.spyOn(document.body, 'removeChild')

      render(<XKTGenerator />)

      // 生成文件
      await userEvent.click(screen.getByRole('button', { name: /生成XKT文件/ }))

      // 切换到历史记录
      await userEvent.click(screen.getByRole('tab', { name: '历史记录' }))

      // 等待历史记录更新
      await waitFor(() => {
        const downloadButtons = screen.getAllByRole('button')
        const downloadButton = downloadButtons.find(btn =>
          btn.querySelector('svg')?.getAttribute('class')?.includes('h-4 w-4')
        )
        expect(downloadButton).toBeInTheDocument()
      })

      // 点击下载按钮
      const downloadButtons = screen.getAllByRole('button')
      const downloadButton = downloadButtons.find(btn =>
        btn.querySelector('svg')?.getAttribute('class')?.includes('h-4 w-4')
      )

      if (downloadButton) {
        fireEvent.click(downloadButton)

        // 验证创建了下载链接
        expect(createElementSpy).toHaveBeenCalledWith('a')
        expect(appendChildSpy).toHaveBeenCalled()
        expect(removeChildSpy).toHaveBeenCalled()
        expect(toast.success).toHaveBeenCalledWith('开始下载: test.xkt')
      }
    })
  })

  describe('UI交互测试', () => {
    test('生成时应该显示加载状态', async () => {
      // 模拟长时间请求
      ;(fetch as jest.Mock).mockImplementation(() =>
        new Promise(resolve => setTimeout(() => resolve({
          ok: true,
          json: async () => ({ success: true, filename: 'test.xkt' })
        }), 100))
      )

      render(<XKTGenerator />)

      const generateButton = screen.getByRole('button', { name: /生成XKT文件/ })

      // 点击生成
      fireEvent.click(generateButton)

      // 应该显示加载状态
      expect(screen.getByText(/正在生成/)).toBeInTheDocument()

      // 按钮应该被禁用
      expect(generateButton).toBeDisabled()
    })

    test('压缩开关应该正常工作', async () => {
      render(<XKTGenerator />)

      const compressSwitch = screen.getByRole('switch')

      // 默认应该是开启的
      expect(compressSwitch).toBeChecked()

      // 点击关闭
      await userEvent.click(compressSwitch)
      expect(compressSwitch).not.toBeChecked()

      // 再次点击开启
      await userEvent.click(compressSwitch)
      expect(compressSwitch).toBeChecked()
    })

    test('标签页切换应该正常工作', async () => {
      render(<XKTGenerator />)

      const generateTab = screen.getByRole('tab', { name: '生成XKT' })
      const historyTab = screen.getByRole('tab', { name: '历史记录' })

      // 默认应该显示生成页面
      expect(screen.getByLabelText(/数据库号/)).toBeInTheDocument()

      // 切换到历史记录
      await userEvent.click(historyTab)
      expect(screen.getByText('暂无生成记录')).toBeInTheDocument()

      // 切换回生成页面
      await userEvent.click(generateTab)
      expect(screen.getByLabelText(/数据库号/)).toBeInTheDocument()
    })
  })
})

// 导出测试配置
export const testConfig = {
  validDbno: 1112,
  validRefno: '17496/266203',
  apiUrl: buildXktApiUrl('/api/xkt/generate')
}
