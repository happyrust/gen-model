'use client';

import React, { useState, useCallback, useMemo } from 'react';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import SpatialVisualization from './SpatialVisualization';

interface SpatialNode {
  refno: number;
  name: string;
  noun: string;
  node_type: string;
  children_count: number;
}

interface AdvancedSpatialVisualizationProps {
  rootNode?: SpatialNode;
  children: SpatialNode[];
}

export default function AdvancedSpatialVisualization({
  rootNode,
  children,
}: AdvancedSpatialVisualizationProps) {
  const [searchTerm, setSearchTerm] = useState('');
  const [filterType, setFilterType] = useState<'all' | 'SPACE' | 'ROOM' | 'COMPONENT'>('all');
  const [expandAll, setExpandAll] = useState(false);

  // 过滤节点
  const filteredChildren = useMemo(() => {
    return children.filter((node) => {
      const matchesSearch =
        node.name.toLowerCase().includes(searchTerm.toLowerCase()) ||
        node.noun.toLowerCase().includes(searchTerm.toLowerCase()) ||
        node.refno.toString().includes(searchTerm);

      const matchesType = filterType === 'all' || node.node_type === filterType;

      return matchesSearch && matchesType;
    });
  }, [children, searchTerm, filterType]);

  // 统计信息
  const stats = useMemo(() => {
    return {
      total: children.length,
      spaces: children.filter((n) => n.node_type === 'SPACE').length,
      rooms: children.filter((n) => n.node_type === 'ROOM').length,
      components: children.filter((n) => n.node_type === 'COMPONENT').length,
      filtered: filteredChildren.length,
    };
  }, [children, filteredChildren]);

  return (
    <div className="flex flex-col h-full bg-white">
      {/* 工具栏 */}
      <div className="border-b p-4 space-y-4">
        {/* 搜索框 */}
        <div>
          <label className="block text-sm font-medium text-gray-700 mb-2">
            搜索
          </label>
          <Input
            type="text"
            placeholder="按名称、类型或ID搜索..."
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
            className="w-full"
          />
        </div>

        {/* 过滤和操作 */}
        <div className="flex gap-2 flex-wrap">
          <div className="flex gap-2">
            <Button
              variant={filterType === 'all' ? 'default' : 'outline'}
              onClick={() => setFilterType('all')}
              size="sm"
            >
              全部
            </Button>
            <Button
              variant={filterType === 'SPACE' ? 'default' : 'outline'}
              onClick={() => setFilterType('SPACE')}
              size="sm"
            >
              🏢 空间
            </Button>
            <Button
              variant={filterType === 'ROOM' ? 'default' : 'outline'}
              onClick={() => setFilterType('ROOM')}
              size="sm"
            >
              🚪 房间
            </Button>
            <Button
              variant={filterType === 'COMPONENT' ? 'default' : 'outline'}
              onClick={() => setFilterType('COMPONENT')}
              size="sm"
            >
              ⚙️ 构件
            </Button>
          </div>
          <Button
            variant="outline"
            onClick={() => setExpandAll(!expandAll)}
            size="sm"
            className="ml-auto"
          >
            {expandAll ? '折叠全部' : '展开全部'}
          </Button>
        </div>

        {/* 统计信息 */}
        <div className="grid grid-cols-5 gap-2 text-xs">
          <div className="bg-blue-50 p-2 rounded">
            <p className="text-gray-600">空间</p>
            <p className="font-bold text-blue-600">{stats.spaces}</p>
          </div>
          <div className="bg-green-50 p-2 rounded">
            <p className="text-gray-600">房间</p>
            <p className="font-bold text-green-600">{stats.rooms}</p>
          </div>
          <div className="bg-purple-50 p-2 rounded">
            <p className="text-gray-600">构件</p>
            <p className="font-bold text-purple-600">{stats.components}</p>
          </div>
          <div className="bg-gray-50 p-2 rounded">
            <p className="text-gray-600">总计</p>
            <p className="font-bold text-gray-600">{stats.total}</p>
          </div>
          <div className="bg-yellow-50 p-2 rounded">
            <p className="text-gray-600">过滤后</p>
            <p className="font-bold text-yellow-600">{stats.filtered}</p>
          </div>
        </div>
      </div>

      {/* 可视化区域 */}
      <div className="flex-1 overflow-auto">
        <SpatialVisualization
          rootNode={rootNode}
          children={filteredChildren}
        />
      </div>
    </div>
  );
}

