import { NextResponse } from "next/server"

const API_BASE_URL = process.env.NEXT_PUBLIC_API_BASE_URL?.replace(/\/$/, "")

export const revalidate = 0

export async function GET(
  request: Request,
  { params }: { params: Promise<{ taskId: string }> }
) {
  const { taskId } = await params

  if (!API_BASE_URL) {
    return NextResponse.json(
      {
        progress: 0,
        status: "unknown",
      },
      { status: 200 },
    )
  }

  const requestUrl = `${API_BASE_URL}/api/tasks/${taskId}/progress`

  try {
    const upstreamResponse = await fetch(requestUrl, {
      headers: {
        Accept: "application/json",
      },
      cache: "no-store",
    })

    if (!upstreamResponse.ok) {
      return NextResponse.json(
        {
          progress: 0,
          status: "unknown",
        },
        { status: 200 },
      )
    }

    const data = await upstreamResponse.json()
    return NextResponse.json(data, { status: 200 })
  } catch (error) {
    return NextResponse.json(
      {
        progress: 0,
        status: "unknown",
      },
      { status: 200 },
    )
  }
}

