"use client"

import { useCallback, useMemo } from "react"
import ReactFlow, { Background, Controls, Edge, Node } from "reactflow"
import "reactflow/dist/style.css"
import type { CollaborationGroup } from "@/types/collaboration"

interface GroupGraphProps {
  group: CollaborationGroup
  sites: Array<{
    id: string
    name: string
    isPrimary: boolean
    status?: string
  }>
  onPrimaryNodeClick?: () => void
}

const PRIMARY_NODE_POSITION = { x: 0, y: 0 }
const RADIUS = 200

export function GroupGraph({ group, sites, onPrimaryNodeClick }: GroupGraphProps) {
  const primarySite = sites.find((site) => site.isPrimary)
  const clientSites = sites.filter((site) => !site.isPrimary)

  const nodes = useMemo<Node[]>(() => {
    const primaryNode: Node | null = primarySite
      ? {
          id: primarySite.id,
          data: {
            label: `${primarySite.name}（主）`,
            status: primarySite.status,
          },
          position: PRIMARY_NODE_POSITION,
          className:
            "rounded-lg border-2 border-primary bg-primary/10 text-primary-foreground px-4 py-2 shadow-md font-medium cursor-pointer",
        }
      : null

    const clientNodes: Node[] = clientSites.map((site, index) => {
      const angle = (index / Math.max(clientSites.length, 1)) * Math.PI * 2
      const x = PRIMARY_NODE_POSITION.x + Math.cos(angle) * RADIUS
      const y = PRIMARY_NODE_POSITION.y + Math.sin(angle) * RADIUS

      return {
        id: site.id,
        data: {
          label: site.name,
          status: site.status,
        },
        position: { x, y },
        className:
          "rounded-lg border border-border bg-card px-3 py-2 shadow-sm text-sm text-card-foreground",
      }
    })

    return [primaryNode, ...clientNodes].filter(Boolean) as Node[]
  }, [primarySite, clientSites])

  const edges = useMemo<Edge[]>(() => {
    if (!primarySite) return []
    return clientSites.map((site) => ({
      id: `${primarySite.id}-${site.id}`,
      source: primarySite.id,
      target: site.id,
      animated: true,
      label: "MQTT",
      style: { strokeWidth: 2 },
      labelStyle: {
        fontSize: 12,
        fontWeight: 500,
        fill: "var(--muted-foreground)",
      },
    }))
  }, [primarySite, clientSites])

  const handleNodeClick = useCallback(
    (_: unknown, node: Node) => {
      if (primarySite && node.id === primarySite.id && onPrimaryNodeClick) {
        onPrimaryNodeClick()
      }
    },
    [primarySite, onPrimaryNodeClick],
  )

  if (!primarySite) {
    return (
      <div className="flex h-64 flex-col items-center justify-center rounded-lg border border-dashed border-border text-sm text-muted-foreground">
        <p>请先在“管理站点”中设置主站点后再查看拓扑图。</p>
      </div>
    )
  }

  return (
    <div className="h-80 w-full rounded-lg border border-border">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        fitView
        panOnDrag
        zoomOnScroll
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable={false}
        onNodeClick={handleNodeClick}
      >
        <Background gap={24} size={1} color="rgba(148, 163, 184, 0.3)" />
      </ReactFlow>
    </div>
  )
}
