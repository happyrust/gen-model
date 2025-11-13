"use client"

import { useMemo } from "react"
import ReactFlow, { Background, Controls, Edge, Node } from "reactflow"
import "reactflow/dist/style.css"

import type { RemoteSyncFlowStat, RemoteSite } from "@/types/collaboration"

interface SyncFlowGraphProps {
  groupName: string
  flows: RemoteSyncFlowStat[]
  sites: RemoteSite[]
}

const SOURCE_NODE_ID = "remote-env"
const SOURCE_NODE_POSITION = { x: 0, y: 0 }
const GRAPH_RADIUS = 240

function formatBytes(value: number | undefined) {
  if (!value || value <= 0) return "0 B"
  const units = ["B", "KB", "MB", "GB", "TB"]
  const exponent = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1)
  const scaled = value / Math.pow(1024, exponent)
  return `${scaled.toFixed(scaled >= 10 ? 0 : 1)} ${units[exponent]}`
}

function getSiteLabel(flow: RemoteSyncFlowStat, sites: RemoteSite[]) {
  const match =
    sites.find((site) => site.id === flow.target_site) ??
    sites.find((site) => site.name === flow.target_site)

  return match?.name ?? flow.target_site ?? "未知站点"
}

export function SyncFlowGraph({ groupName, flows, sites }: SyncFlowGraphProps) {
  const { nodes, edges } = useMemo(() => {
    if (!flows.length) {
      return { nodes: [], edges: [] }
    }

    const maxBytes = Math.max(...flows.map((flow) => flow.total_bytes ?? 0), 1)

    const targetNodes: Node[] = []
    const targetEdges: Edge[] = []

    flows.forEach((flow, index) => {
      const nodeId = `${flow.target_site || "unknown"}-${flow.direction || "direction"}-${index}`
      const angle = (index / flows.length) * Math.PI * 2
      const x = SOURCE_NODE_POSITION.x + Math.cos(angle) * GRAPH_RADIUS
      const y = SOURCE_NODE_POSITION.y + Math.sin(angle) * GRAPH_RADIUS

      targetNodes.push({
        id: nodeId,
        data: {
          label: `${getSiteLabel(flow, sites)}\n${flow.direction || "单向"}`,
        },
        position: { x, y },
        className:
          "rounded-lg border border-border bg-card text-card-foreground px-3 py-2 text-xs text-center whitespace-pre-line shadow-sm",
      })

      const successRate = flow.total > 0 ? flow.completed / flow.total : 0
      const strokeWidth = 2 + Math.max(0, Math.min(6, ((flow.total_bytes ?? 0) / maxBytes) * 6))
      const edgeColor = successRate >= 0.9 ? "#22c55e" : successRate >= 0.6 ? "#f59e0b" : "#ef4444"

      targetEdges.push({
        id: `edge-${nodeId}`,
        source: SOURCE_NODE_ID,
        target: nodeId,
        animated: true,
        label: `${formatBytes(flow.total_bytes)} · 成功 ${flow.completed}/${flow.total}`,
        labelStyle: {
          fontSize: 11,
          fill: "var(--muted-foreground)",
          fontWeight: 500,
        },
        style: {
          strokeWidth,
          stroke: edgeColor,
        },
      })
    })

    const envNode: Node = {
      id: SOURCE_NODE_ID,
      data: { label: `${groupName}\n环境` },
      position: SOURCE_NODE_POSITION,
      className:
        "rounded-xl border-2 border-primary bg-primary/10 text-primary-foreground px-4 py-3 text-sm font-semibold whitespace-pre-line shadow-md",
    }

    return {
      nodes: [envNode, ...targetNodes],
      edges: targetEdges,
    }
  }, [flows, groupName, sites])

  if (!flows.length) {
    return (
      <div className="flex h-80 flex-col items-center justify-center rounded-lg border border-dashed border-border text-sm text-muted-foreground">
        <p>暂无同步流向数据</p>
      </div>
    )
  }

  return (
    <div className="h-96 w-full rounded-lg border border-border">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        fitView
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable={false}
        zoomOnScroll
        panOnDrag
      >
        <Background gap={28} size={1} color="rgba(148, 163, 184, 0.25)" />
      </ReactFlow>
    </div>
  )
}
