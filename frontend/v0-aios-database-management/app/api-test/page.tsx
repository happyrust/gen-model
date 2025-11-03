"use client"

import { useState, useEffect } from "react"

export default function ApiTestPage() {
  const [apiBaseUrl, setApiBaseUrl] = useState<string>("")
  const [sites, setSites] = useState<any[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    // 获取环境变量
    setApiBaseUrl(process.env.NEXT_PUBLIC_API_BASE_URL || "未设置")
  }, [])

  const testApi = async () => {
    try {
      setLoading(true)
      setError(null)
      
      const apiUrl = process.env.NEXT_PUBLIC_API_BASE_URL || ""
      const fullUrl = apiUrl ? `${apiUrl}/api/deployment-sites` : "/api/deployment-sites"
      
      console.log("API URL:", fullUrl)
      
      const response = await fetch(fullUrl, {
        method: 'GET',
        headers: {
          'Accept': 'application/json',
        },
      })
      
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}: ${response.statusText}`)
      }
      
      const data = await response.json()
      setSites(data.items || [])
    } catch (err) {
      setError(err instanceof Error ? err.message : 'API调用失败')
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="p-8">
      <h1 className="text-2xl font-bold mb-4">API测试页面</h1>
      
      <div className="mb-4">
        <p><strong>API Base URL:</strong> {apiBaseUrl}</p>
      </div>
      
      <button 
        onClick={testApi}
        disabled={loading}
        className="bg-blue-500 text-white px-4 py-2 rounded disabled:opacity-50"
      >
        {loading ? "测试中..." : "测试API"}
      </button>
      
      {error && (
        <div className="mt-4 p-4 bg-red-100 border border-red-400 text-red-700 rounded">
          <strong>错误:</strong> {error}
        </div>
      )}
      
      {sites.length > 0 && (
        <div className="mt-4">
          <h2 className="text-xl font-semibold mb-2">站点数据:</h2>
          <pre className="bg-gray-100 p-4 rounded overflow-auto">
            {JSON.stringify(sites, null, 2)}
          </pre>
        </div>
      )}
    </div>
  )
}







