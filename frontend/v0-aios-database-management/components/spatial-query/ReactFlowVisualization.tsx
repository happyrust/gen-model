'use client';

import React, { useEffect, useState, useCallback } from 'react';
import ReactFlow, {
  Node,
  Edge,
  Controls,
  Background,
  useNodesState,
  useEdgesState,
  MiniMap,
  Panel,
} from 'reactflow';
import 'reactflow/dist/style.css';
import { buildApiUrl } from '@/lib/api';
import SpaceNode from './nodes/SpaceNode';
import RoomNode from './nodes/RoomNode';
import ComponentNode from './nodes/ComponentNode';

interface SpatialNode {
  refno: number;
  name: string;
  noun: string;
  node_type: string;
  children_count: number;
}

interface ReactFlowVisualizationProps {
  rootNode?: SpatialNode;
  children: SpatialNode[];
}

const nodeTypes = {
  space: SpaceNode,
  room: RoomNode,
  component: ComponentNode,
};

export default function ReactFlowVisualization({
  rootNode,
  children,
}: ReactFlowVisualizationProps) {
  const [nodes, setNodes, onNodesChange] = useNodesState([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState([]);
  const [expandedNodes, setExpandedNodes] = useState<Set<number>>(new Set());
  const [loadingNodes, setLoadingNodes] = useState<Set<number>>(new Set());

  // 初始化节点和边
  useEffect(() => {
    if (!rootNode) return;

    const initialNodes: Node[] = [
      {
        id: `node-${rootNode.refno}`,
        data: {
          label: rootNode.name,
          refno: rootNode.refno,
          noun: rootNode.noun,
          type: rootNode.node_type,
          childrenCount: rootNode.children_count,
          onExpand: handleNodeExpand,
        },
        position: { x: 0, y: 0 },
        type: getNodeType(rootNode.node_type),
      },
    ];

    const initialEdges: Edge[] = [];

    // 添加初始子节点
    children.forEach((child, index) => {
      const angle = (index / children.length) * 2 * Math.PI;
      const radius = 200;
      const x = Math.cos(angle) * radius;
      const y = Math.sin(angle) * radius;

      initialNodes.push({
        id: `node-${child.refno}`,
        data: {
          label: child.name,
          refno: child.refno,
          noun: child.noun,
          type: child.node_type,
          childrenCount: child.children_count,
          onExpand: handleNodeExpand,
        },
        position: { x, y },
        type: getNodeType(child.node_type),
      });

      initialEdges.push({
        id: `edge-${rootNode.refno}-${child.refno}`,
        source: `node-${rootNode.refno}`,
        target: `node-${child.refno}`,
        animated: true,
      });
    });

    setNodes(initialNodes);
    setEdges(initialEdges);
    setExpandedNodes(new Set([rootNode.refno]));
  }, [rootNode, children, setNodes, setEdges]);

  // 获取节点类型
  const getNodeType = (nodeType: string): string => {
    switch (nodeType) {
      case 'SPACE':
        return 'space';
      case 'ROOM':
        return 'room';
      case 'COMPONENT':
        return 'component';
      default:
        return 'component';
    }
  };

  // 加载子节点
  const loadChildren = useCallback(
    async (refno: number): Promise<SpatialNode[]> => {
      try {
        const url = buildApiUrl(`/spatial/children/${refno}`);
        const response = await fetch(url);
        const data = await response.json();
        return Array.isArray(data) ? data : [];
      } catch (error) {
        console.error('Failed to load children:', error);
        return [];
      }
    },
    []
  );

  // 处理节点展开
  const handleNodeExpand = useCallback(
    async (refno: number) => {
      const isExpanded = expandedNodes.has(refno);

      if (isExpanded) {
        // 折叠节点
        const newExpanded = new Set(expandedNodes);
        newExpanded.delete(refno);
        setExpandedNodes(newExpanded);

        // 移除子节点和边
        setNodes((prevNodes) =>
          prevNodes.filter((node) => !node.id.startsWith(`child-${refno}-`))
        );
        setEdges((prevEdges) =>
          prevEdges.filter((edge) => edge.source !== `node-${refno}`)
        );
      } else {
        // 展开节点
        setLoadingNodes((prev) => new Set(prev).add(refno));

        const childrenData = await loadChildren(refno);
        const parentNode = nodes.find((n) => n.id === `node-${refno}`);

        if (parentNode && childrenData.length > 0) {
          const newNodes: Node[] = [];
          const newEdges: Edge[] = [];

          childrenData.forEach((child, index) => {
            const angle = (index / childrenData.length) * 2 * Math.PI;
            const radius = 150;
            const x = parentNode.position.x + Math.cos(angle) * radius;
            const y = parentNode.position.y + Math.sin(angle) * radius;

            newNodes.push({
              id: `child-${refno}-${child.refno}`,
              data: {
                label: child.name,
                refno: child.refno,
                noun: child.noun,
                type: child.node_type,
                childrenCount: child.children_count,
                onExpand: handleNodeExpand,
              },
              position: { x, y },
              type: getNodeType(child.node_type),
            });

            newEdges.push({
              id: `edge-${refno}-${child.refno}`,
              source: `node-${refno}`,
              target: `child-${refno}-${child.refno}`,
              animated: true,
            });
          });

          setNodes((prevNodes) => [...prevNodes, ...newNodes]);
          setEdges((prevEdges) => [...prevEdges, ...newEdges]);
          setExpandedNodes((prev) => new Set(prev).add(refno));
        }

        setLoadingNodes((prev) => {
          const newSet = new Set(prev);
          newSet.delete(refno);
          return newSet;
        });
      }
    },
    [nodes, expandedNodes, loadChildren, setNodes, setEdges]
  );

  return (
    <div style={{ width: '100%', height: '100%' }}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        nodeTypes={nodeTypes}
        fitView
      >
        <Background />
        <Controls />
        <MiniMap />
        <Panel position="top-left" className="bg-white p-4 rounded-lg shadow">
          <div className="text-sm text-gray-600">
            <p>节点数: {nodes.length}</p>
            <p>连接数: {edges.length}</p>
          </div>
        </Panel>
      </ReactFlow>
    </div>
  );
}

