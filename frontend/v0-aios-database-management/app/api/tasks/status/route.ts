import { NextResponse } from "next/server"

const API_BASE_URL = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "")

export const revalidate = 0

export async function GET() {
  if (!API_BASE_URL) {
    return NextResponse.json(
      {
        tasks: [],
        systemMetrics: {
          cpu: 0,
          memory: 0,
          disk: 0,
          network: 0,
          uptime: 0,
          services: []
        },
        timestamp: new Date().toISOString(),
      },
      { status: 200 },
    )
  }

  const requestUrl = `${API_BASE_URL}/api/tasks/status`

  try {
    const upstreamResponse = await fetch(requestUrl, {
      headers: {
        Accept: "application/json",
      },
      cache: "no-store",
    })

    if (!upstreamResponse.ok) {
      // 返回默认数据而不是错误
      return NextResponse.json(
        {
          tasks: [],
          systemMetrics: {
            cpu: 0,
            memory: 0,
            disk: 0,
            network: 0,
            uptime: 0,
            services: []
          },
          timestamp: new Date().toISOString(),
        },
        { status: 200 },
      )
    }

    const data = await upstreamResponse.json()
    return NextResponse.json(data, { status: 200 })
  } catch (error) {
    // 返回默认数据
    return NextResponse.json(
      {
        tasks: [],
        systemMetrics: {
          cpu: 0,
          memory: 0,
          disk: 0,
          network: 0,
          uptime: 0,
          services: []
        },
        timestamp: new Date().toISOString(),
      },
      { status: 200 },
    )
  }
}

