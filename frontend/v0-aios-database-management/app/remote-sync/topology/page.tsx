'use client'

import { useState, useCallback, useEffect } from 'react'
import ReactFlow, {
  Node,
  Edge,
  Controls,
  Background,
  useNodesState,
  useEdgesState,
  addEdge,
  Connection,
  MarkerType,
  Panel,
} from 'reactflow'
import 'reactflow/dist/style.css'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { useToast } from '@/hooks/use-toast'
import { 
  Save, 
  Download, 
  Upload, 
  Plus, 
  Trash2, 
  Layout,
  Server,
  MapPin,
} from 'lucide-react'

// 环境节点组件
function EnvironmentNode({ data }: { data: any }) {
  return (
    <Card className="p-4 min-w-[200px] border-2 border-blue-500 bg-blue-50">
      <div className="flex items-center gap-2 mb-2">
        <Server className="w-5 h-5 text-blue-600" />
        <div className="font-semibold text-blue-900">{data.label}</div>
      </div>
      <div className="text-xs text-gray-600 space-y-1">
        <div>MQTT: {data.mqtt_host || 'N/A'}</div>
        <div>文件服务器: {data.file_server_host || 'N/A'}</div>
        <div>数据库: {data.location_dbs || 'N/A'}</div>
      </div>
    </Card>
  )
}

// 站点节点组件
function SiteNode({ data }: { data: any }) {
  return (
    <Card className="p-3 min-w-[160px] border-2 border-green-500 bg-green-50">
      <div className="flex items-center gap-2 mb-2">
        <MapPin className="w-4 h-4 text-green-600" />
        <div className="font-semibold text-green-900">{data.label}</div>
      </div>
      <div className="text-xs text-gray-600 space-y-1">
        <div>位置: {data.location || 'N/A'}</div>
        <div>数据库: {data.dbnums || 'N/A'}</div>
      </div>
    </Card>
  )
}

const nodeTypes = {
  environment: EnvironmentNode,
  site: SiteNode,
}

export default function TopologyPage() {
  const [nodes, setNodes, onNodesChange] = useNodesState([])
  const [edges, setEdges, onEdgesChange] = useEdgesState([])
  const [selectedNode, setSelectedNode] = useState<Node | null>(null)
  const [isLoading, setIsLoading] = useState(false)
  const { toast } = useToast()

  // 加载拓扑配置
  const loadTopology = useCallback(async () => {
    setIsLoading(true)
    try {
      const response = await fetch('/api/remote-sync/topology')
      if (!response.ok) throw new Error('加载拓扑配置失败')
      
      const result = await response.json()
      if (result.status === 'success' && result.data) {
        const { environments, sites, connections } = result.data
        
        // 转换环境为节点
        const envNodes: Node[] = environments.map((env: any, index: number) => ({
          id: env.id,
          type: 'environment',
          position: { x: 100, y: index * 200 + 50 },
          data: {
            label: env.name,
            mqtt_host: env.mqtt_host,
            file_server_host: env.file_server_host,
            location_dbs: env.location_dbs,
            ...env,
          },
        }))

        // 转换站点为节点
        const siteNodes: Node[] = sites.map((site: any, index: number) => ({
          id: site.id,
          type: 'site',
          position: { x: 500, y: index * 150 + 50 },
          data: {
            label: site.name,
            location: site.location,
            dbnums: site.dbnums,
            env_id: site.env_id,
            ...site,
          },
        }))

        // 转换连接为边
        const topologyEdges: Edge[] = connections.map((conn: any) => ({
          id: `${conn.env_id}-${conn.site_id}`,
          source: conn.env_id,
          target: conn.site_id,
          type: 'smoothstep',
          animated: true,
          markerEnd: {
            type: MarkerType.ArrowClosed,
          },
        }))

        setNodes([...envNodes, ...siteNodes])
        setEdges(topologyEdges)
        
        toast({
          title: '加载成功',
          description: `已加载 ${environments.length} 个环境和 ${sites.length} 个站点`,
        })
      }
    } catch (error) {
      toast({
        title: '加载失败',
        description: error instanceof Error ? error.message : '未知错误',
        variant: 'destructive',
      })
    } finally {
      setIsLoading(false)
    }
  }, [setNodes, setEdges, toast])

  // 保存拓扑配置
  const saveTopology = useCallback(async () => {
    setIsLoading(true)
    try {
      // 分离环境和站点节点
      const environments = nodes
        .filter(node => node.type === 'environment')
        .map(node => ({
          id: node.data.id || node.id,
          name: node.data.label,
          mqtt_host: node.data.mqtt_host,
          mqtt_port: node.data.mqtt_port,
          file_server_host: node.data.file_server_host,
          location: node.data.location,
          location_dbs: node.data.location_dbs,
          reconnect_initial_ms: node.data.reconnect_initial_ms,
          reconnect_max_ms: node.data.reconnect_max_ms,
          created_at: node.data.created_at || new Date().toISOString(),
          updated_at: new Date().toISOString(),
        }))

      const sites = nodes
        .filter(node => node.type === 'site')
        .map(node => ({
          id: node.data.id || node.id,
          env_id: node.data.env_id,
          name: node.data.label,
          location: node.data.location,
          http_host: node.data.http_host,
          dbnums: node.data.dbnums,
          notes: node.data.notes,
          created_at: node.data.created_at || new Date().toISOString(),
          updated_at: new Date().toISOString(),
        }))

      const connections = edges.map(edge => ({
        env_id: edge.source,
        site_id: edge.target,
      }))

      const response = await fetch('/api/remote-sync/topology', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ environments, sites, connections }),
      })

      if (!response.ok) {
        const error = await response.json()
        throw new Error(error.error || '保存失败')
      }

      toast({
        title: '保存成功',
        description: '拓扑配置已保存',
      })
    } catch (error) {
      toast({
        title: '保存失败',
        description: error instanceof Error ? error.message : '未知错误',
        variant: 'destructive',
      })
    } finally {
      setIsLoading(false)
    }
  }, [nodes, edges, toast])

  // 自动布局
  const autoLayout = useCallback(() => {
    const envNodes = nodes.filter(n => n.type === 'environment')
    const siteNodes = nodes.filter(n => n.type === 'site')

    const layoutNodes = [
      ...envNodes.map((node, index) => ({
        ...node,
        position: { x: 100, y: index * 200 + 50 },
      })),
      ...siteNodes.map((node, index) => ({
        ...node,
        position: { x: 500, y: index * 150 + 50 },
      })),
    ]

    setNodes(layoutNodes)
    toast({
      title: '布局完成',
      description: '节点已自动排列',
    })
  }, [nodes, setNodes, toast])

  // 添加环境节点
  const addEnvironment = useCallback(() => {
    const newNode: Node = {
      id: `env-${Date.now()}`,
      type: 'environment',
      position: { x: 100, y: nodes.length * 100 },
      data: {
        label: '新环境',
        mqtt_host: '',
        file_server_host: '',
        location_dbs: '',
      },
    }
    setNodes((nds) => [...nds, newNode])
  }, [nodes.length, setNodes])

  // 添加站点节点
  const addSite = useCallback(() => {
    const newNode: Node = {
      id: `site-${Date.now()}`,
      type: 'site',
      position: { x: 500, y: nodes.length * 100 },
      data: {
        label: '新站点',
        location: '',
        dbnums: '',
        env_id: '',
      },
    }
    setNodes((nds) => [...nds, newNode])
  }, [nodes.length, setNodes])

  // 删除选中节点
  const deleteSelected = useCallback(() => {
    if (selectedNode) {
      setNodes((nds) => nds.filter((n) => n.id !== selectedNode.id))
      setEdges((eds) => eds.filter((e) => e.source !== selectedNode.id && e.target !== selectedNode.id))
      setSelectedNode(null)
      toast({
        title: '删除成功',
        description: '节点已删除',
      })
    }
  }, [selectedNode, setNodes, setEdges, toast])

  // 连接节点
  const onConnect = useCallback(
    (params: Connection) => {
      // 只允许从环境连接到站点
      const sourceNode = nodes.find(n => n.id === params.source)
      const targetNode = nodes.find(n => n.id === params.target)
      
      if (sourceNode?.type === 'environment' && targetNode?.type === 'site') {
        setEdges((eds) => addEdge({
          ...params,
          type: 'smoothstep',
          animated: true,
          markerEnd: { type: MarkerType.ArrowClosed },
        }, eds))
        
        // 更新站点的 env_id
        setNodes((nds) => nds.map(node => 
          node.id === params.target 
            ? { ...node, data: { ...node.data, env_id: params.source } }
            : node
        ))
      } else {
        toast({
          title: '连接失败',
          description: '只能从环境节点连接到站点节点',
          variant: 'destructive',
        })
      }
    },
    [nodes, setEdges, setNodes, toast]
  )

  // 节点点击事件
  const onNodeClick = useCallback((_: any, node: Node) => {
    setSelectedNode(node)
  }, [])

  // 初始加载
  useEffect(() => {
    loadTopology()
  }, [loadTopology])

  return (
    <div className="h-screen flex flex-col">
      {/* 工具栏 */}
      <div className="border-b bg-white p-4 flex items-center justify-between">
        <div className="flex items-center gap-2">
          <h1 className="text-2xl font-bold">拓扑配置</h1>
          <span className="text-sm text-gray-500">
            {nodes.length} 个节点, {edges.length} 个连接
          </span>
        </div>
        
        <div className="flex items-center gap-2">
          <Button onClick={addEnvironment} variant="outline" size="sm">
            <Server className="w-4 h-4 mr-2" />
            添加环境
          </Button>
          <Button onClick={addSite} variant="outline" size="sm">
            <MapPin className="w-4 h-4 mr-2" />
            添加站点
          </Button>
          <Button onClick={autoLayout} variant="outline" size="sm">
            <Layout className="w-4 h-4 mr-2" />
            自动布局
          </Button>
          <Button onClick={deleteSelected} variant="outline" size="sm" disabled={!selectedNode}>
            <Trash2 className="w-4 h-4 mr-2" />
            删除
          </Button>
          <Button onClick={loadTopology} variant="outline" size="sm" disabled={isLoading}>
            <Upload className="w-4 h-4 mr-2" />
            加载
          </Button>
          <Button onClick={saveTopology} size="sm" disabled={isLoading}>
            <Save className="w-4 h-4 mr-2" />
            保存
          </Button>
        </div>
      </div>

      {/* React Flow 画布 */}
      <div className="flex-1">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          onNodeClick={onNodeClick}
          nodeTypes={nodeTypes}
          fitView
        >
          <Background />
          <Controls />
          <Panel position="top-left" className="bg-white p-4 rounded-lg shadow-lg">
            <div className="text-sm space-y-2">
              <div className="font-semibold">操作说明：</div>
              <div>• 拖动节点调整位置</div>
              <div>• 从环境连接到站点</div>
              <div>• 点击节点查看详情</div>
              <div>• 使用鼠标滚轮缩放</div>
            </div>
          </Panel>
        </ReactFlow>
      </div>

      {/* 节点详情面板 */}
      {selectedNode && (
        <div className="absolute right-4 top-20 w-80 bg-white rounded-lg shadow-xl p-4 border">
          <h3 className="font-semibold mb-4">节点详情</h3>
          <div className="space-y-2 text-sm">
            <div>
              <span className="font-medium">类型：</span>
              {selectedNode.type === 'environment' ? '环境' : '站点'}
            </div>
            <div>
              <span className="font-medium">名称：</span>
              {selectedNode.data.label}
            </div>
            {selectedNode.type === 'environment' && (
              <>
                <div>
                  <span className="font-medium">MQTT：</span>
                  {selectedNode.data.mqtt_host || 'N/A'}
                </div>
                <div>
                  <span className="font-medium">文件服务器：</span>
                  {selectedNode.data.file_server_host || 'N/A'}
                </div>
              </>
            )}
            {selectedNode.type === 'site' && (
              <>
                <div>
                  <span className="font-medium">位置：</span>
                  {selectedNode.data.location || 'N/A'}
                </div>
                <div>
                  <span className="font-medium">数据库：</span>
                  {selectedNode.data.dbnums || 'N/A'}
                </div>
              </>
            )}
          </div>
          <Button 
            onClick={() => setSelectedNode(null)} 
            variant="outline" 
            size="sm" 
            className="mt-4 w-full"
          >
            关闭
          </Button>
        </div>
      )}
    </div>
  )
}
