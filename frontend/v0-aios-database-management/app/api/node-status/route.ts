import { NextResponse } from "next/server"

const API_BASE_URL = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "")

export const revalidate = 0

export async function GET() {
  if (!API_BASE_URL) {
    return NextResponse.json(
      {
        status: "error",
        message: "后端节点状态服务未配置",
      },
      { status: 503 },
    )
  }

  const requestUrl = `${API_BASE_URL}/api/node-status`

  try {
    const upstreamResponse = await fetch(requestUrl, {
      headers: {
        Accept: "application/json",
      },
      cache: "no-store",
    })

    if (!upstreamResponse.ok) {
      const errorText = await upstreamResponse.text()
      return NextResponse.json(
        {
          status: "error",
          message: errorText || `节点状态查询失败 (HTTP ${upstreamResponse.status})`,
        },
        { status: upstreamResponse.status },
      )
    }

    const data = await upstreamResponse.json()
    return NextResponse.json(data, { status: 200 })
  } catch (error) {
    const message = error instanceof Error ? error.message : "获取节点状态发生未知错误"
    return NextResponse.json(
      {
        status: "error",
        message,
      },
      { status: 502 },
    )
  }
}
