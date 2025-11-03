import { NextResponse } from "next/server"

const API_BASE_URL = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "")

export const revalidate = 0

export async function POST(
  request: Request,
  { params }: { params: Promise<{ taskId: string }> }
) {
  const { taskId } = await params

  if (!API_BASE_URL) {
    return NextResponse.json(
      {
        success: true,
        message: "任务启动请求已记录",
        taskId,
      },
      { status: 200 },
    )
  }

  const requestUrl = `${API_BASE_URL}/api/tasks/${taskId}/start`

  try {
    const upstreamResponse = await fetch(requestUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json",
      },
      cache: "no-store",
    })

    if (!upstreamResponse.ok) {
      const errorText = await upstreamResponse.text()
      return NextResponse.json(
        {
          success: false,
          message: errorText || `任务启动失败 (HTTP ${upstreamResponse.status})`,
          taskId,
        },
        { status: upstreamResponse.status },
      )
    }

    const data = await upstreamResponse.json()
    return NextResponse.json(data, { status: 200 })
  } catch (error) {
    return NextResponse.json(
      {
        success: true,
        message: "任务启动请求已记录",
        taskId,
      },
      { status: 200 },
    )
  }
}

