'use client';

import React, { useEffect, useState, useCallback } from 'react';
import { buildApiUrl } from '@/lib/api';

interface SpatialNode {
  refno: number;
  name: string;
  noun: string;
  node_type: string;
  children_count: number;
}

interface SpatialVisualizationProps {
  rootNode?: SpatialNode;
  children: SpatialNode[];
}

interface TreeNode extends SpatialNode {
  expanded: boolean;
  childrenLoaded: boolean;
  childrenData: TreeNode[];
}

export default function SpatialVisualization({
  rootNode,
  children,
}: SpatialVisualizationProps) {
  const [treeData, setTreeData] = useState<TreeNode | null>(null);
  const [expandedNodes, setExpandedNodes] = useState<Set<number>>(new Set());
  const [loadingNodes, setLoadingNodes] = useState<Set<number>>(new Set());

  // 初始化树数据
  useEffect(() => {
    if (rootNode) {
      const root: TreeNode = {
        ...rootNode,
        expanded: true,
        childrenLoaded: true,
        childrenData: children.map((child) => ({
          ...child,
          expanded: false,
          childrenLoaded: false,
          childrenData: [],
        })),
      };
      setTreeData(root);
      setExpandedNodes(new Set([rootNode.refno]));
    }
  }, [rootNode, children]);

  // 加载子节点
  const loadChildren = useCallback(
    async (node: TreeNode): Promise<TreeNode[]> => {
      try {
        const url = buildApiUrl(`/spatial/children/${node.refno}`);
        const response = await fetch(url);
        const data = await response.json();

        if (Array.isArray(data)) {
          return data.map((child) => ({
            ...child,
            expanded: false,
            childrenLoaded: false,
            childrenData: [],
          }));
        }
      } catch (error) {
        console.error('Failed to load children:', error);
      }
      return [];
    },
    []
  );

  // 切换节点展开/折叠
  const toggleNode = useCallback(
    async (node: TreeNode) => {
      if (!treeData) return;

      const newExpanded = new Set(expandedNodes);
      if (newExpanded.has(node.refno)) {
        newExpanded.delete(node.refno);
        setExpandedNodes(newExpanded);
      } else {
        newExpanded.add(node.refno);
        setExpandedNodes(newExpanded);

        // 如果还没加载过子节点，则加载
        if (!node.childrenLoaded && node.children_count > 0) {
          setLoadingNodes((prev) => new Set(prev).add(node.refno));
          try {
            const childrenData = await loadChildren(node);
            // 更新树数据
            updateNodeChildren(node.refno, childrenData);
          } finally {
            setLoadingNodes((prev) => {
              const newSet = new Set(prev);
              newSet.delete(node.refno);
              return newSet;
            });
          }
        }
      }
    },
    [treeData, expandedNodes, loadChildren]
  );

  // 更新节点的子节点
  const updateNodeChildren = (nodeRefno: number, childrenData: TreeNode[]) => {
    if (!treeData) return;

    const updateRecursive = (node: TreeNode): TreeNode => {
      if (node.refno === nodeRefno) {
        return {
          ...node,
          childrenData,
          childrenLoaded: true,
        };
      }
      return {
        ...node,
        childrenData: node.childrenData.map(updateRecursive),
      };
    };

    setTreeData(updateRecursive(treeData));
  };

  // 获取节点的颜色
  const getNodeColor = (nodeType: string): string => {
    switch (nodeType) {
      case 'SPACE':
        return 'bg-blue-100 border-blue-300';
      case 'ROOM':
        return 'bg-green-100 border-green-300';
      case 'COMPONENT':
        return 'bg-purple-100 border-purple-300';
      default:
        return 'bg-gray-100 border-gray-300';
    }
  };

  // 获取节点的图标
  const getNodeIcon = (nodeType: string): string => {
    switch (nodeType) {
      case 'SPACE':
        return '🏢';
      case 'ROOM':
        return '🚪';
      case 'COMPONENT':
        return '⚙️';
      default:
        return '📦';
    }
  };

  // 递归渲染树节点
  const renderTreeNode = (node: TreeNode, depth: number = 0): React.ReactNode => {
    const isExpanded = expandedNodes.has(node.refno);
    const hasChildren = node.children_count > 0;
    const isLoading = loadingNodes.has(node.refno);

    return (
      <div key={node.refno} style={{ marginLeft: `${depth * 20}px` }}>
        <div
          className={`flex items-center gap-2 p-3 mb-2 rounded border cursor-pointer transition-all ${getNodeColor(
            node.node_type
          )} hover:shadow-md ${isExpanded ? 'ring-2 ring-offset-1' : ''}`}
          onClick={() => hasChildren && toggleNode(node)}
        >
          {hasChildren && (
            <span className="text-lg transition-transform">
              {isExpanded ? '▼' : '▶'}
            </span>
          )}
          {!hasChildren && <span className="text-lg">•</span>}
          <span className="text-xl">{getNodeIcon(node.node_type)}</span>
          <div className="flex-1 min-w-0">
            <p className="font-semibold text-sm text-gray-900 truncate">
              {node.name}
            </p>
            <p className="text-xs text-gray-600">
              {node.noun} (ID: {node.refno})
            </p>
          </div>
          {hasChildren && (
            <div className="flex items-center gap-2">
              {isLoading && (
                <span className="text-xs text-gray-500">加载中...</span>
              )}
              <span className="text-xs bg-white px-2 py-1 rounded text-gray-600">
                {node.children_count}
              </span>
            </div>
          )}
        </div>

        {isExpanded && node.childrenData.length > 0 && (
          <div>
            {node.childrenData.map((child) =>
              renderTreeNode(child, depth + 1)
            )}
          </div>
        )}
      </div>
    );
  };

  if (!treeData) {
    return (
      <div className="flex items-center justify-center h-full text-gray-500">
        <p>加载中...</p>
      </div>
    );
  }

  return (
    <div className="p-4 overflow-auto h-full bg-white">
      <div className="space-y-2">
        {renderTreeNode(treeData)}
      </div>
    </div>
  );
}

