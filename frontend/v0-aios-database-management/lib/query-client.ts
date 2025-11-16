// React Query 配置

import { QueryClient } from '@tanstack/react-query'

/**
 * 创建 Query Client 实例
 */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // 数据保持新鲜的时间（30 秒）
      staleTime: 30000,
      // 缓存时间（5 分钟）
      gcTime: 300000,
      // 失败后重试次数
      retry: 1,
      // 重试延迟
      retryDelay: (attemptIndex) => Math.min(1000 * 2 ** attemptIndex, 30000),
      // 窗口聚焦时重新获取
      refetchOnWindowFocus: false,
      // 网络重连时重新获取
      refetchOnReconnect: true,
    },
    mutations: {
      // 失败后重试次数
      retry: 0,
    },
  },
})
