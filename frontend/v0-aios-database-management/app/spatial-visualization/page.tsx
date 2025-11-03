'use client';

import React, { useState, useCallback } from 'react';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Card } from '@/components/ui/card';
import SpatialVisualization from '@/components/spatial-query/SpatialVisualization';
import AdvancedSpatialVisualization from '@/components/spatial-query/AdvancedSpatialVisualization';
import ReactFlowVisualization from '@/components/spatial-query/ReactFlowVisualization';
import { buildApiUrl } from '@/lib/api';

interface SpatialNode {
  refno: u64;
  name: string;
  noun: string;
  node_type: string;
  children_count: number;
}

interface QueryResponse {
  success: boolean;
  node?: SpatialNode;
  children: SpatialNode[];
  error_message?: string;
}

export default function SpatialVisualizationPage() {
  const [refno, setRefno] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [queryData, setQueryData] = useState<QueryResponse | null>(null);
  const [visualizationMode, setVisualizationMode] = useState<'tree' | 'advanced' | 'flow'>('advanced');

  const handleQuery = useCallback(async () => {
    if (!refno.trim()) {
      setError('请输入参考号');
      return;
    }

    setLoading(true);
    setError(null);

    try {
      const url = buildApiUrl(`/spatial/query/${refno}`);
      const response = await fetch(url);
      const data: QueryResponse = await response.json();

      if (data.success) {
        setQueryData(data);
      } else {
        setError(data.error_message || '查询失败');
      }
    } catch (err) {
      setError(`网络错误: ${err instanceof Error ? err.message : '未知错误'}`);
    } finally {
      setLoading(false);
    }
  }, [refno]);

  const handleKeyPress = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      handleQuery();
    }
  };

  return (
    <div className="min-h-screen bg-gray-50">
      <div className="max-w-7xl mx-auto px-4 py-8">
        {/* 页面标题 */}
        <div className="mb-8">
          <h1 className="text-3xl font-bold text-gray-900 mb-2">
            空间查询可视化
          </h1>
          <p className="text-gray-600">
            输入参考号查询空间、房间或构件的层级关系
          </p>
        </div>

        {/* 查询面板 */}
        <Card className="mb-8 p-6">
          <div className="flex gap-4">
            <div className="flex-1">
              <label className="block text-sm font-medium text-gray-700 mb-2">
                参考号 (Reference Number)
              </label>
              <Input
                type="text"
                placeholder="例如: 24381"
                value={refno}
                onChange={(e) => setRefno(e.target.value)}
                onKeyPress={handleKeyPress}
                disabled={loading}
                className="w-full"
              />
            </div>
            <div className="flex items-end">
              <Button
                onClick={handleQuery}
                disabled={loading}
                className="w-full"
              >
                {loading ? '查询中...' : '查询'}
              </Button>
            </div>
          </div>

          {/* 错误提示 */}
          {error && (
            <div className="mt-4 p-4 bg-red-50 border border-red-200 rounded-lg">
              <p className="text-red-800 text-sm">{error}</p>
            </div>
          )}

          {/* 节点信息 */}
          {queryData?.node && (
            <div className="mt-6 grid grid-cols-2 md:grid-cols-4 gap-4">
              <div className="bg-blue-50 p-4 rounded-lg">
                <p className="text-xs text-gray-600 mb-1">参考号</p>
                <p className="text-lg font-semibold text-gray-900">
                  {queryData.node.refno}
                </p>
              </div>
              <div className="bg-green-50 p-4 rounded-lg">
                <p className="text-xs text-gray-600 mb-1">名称</p>
                <p className="text-lg font-semibold text-gray-900 truncate">
                  {queryData.node.name}
                </p>
              </div>
              <div className="bg-purple-50 p-4 rounded-lg">
                <p className="text-xs text-gray-600 mb-1">类型</p>
                <p className="text-lg font-semibold text-gray-900">
                  {queryData.node.node_type}
                </p>
              </div>
              <div className="bg-orange-50 p-4 rounded-lg">
                <p className="text-xs text-gray-600 mb-1">子节点数</p>
                <p className="text-lg font-semibold text-gray-900">
                  {queryData.node.children_count}
                </p>
              </div>
            </div>
          )}
        </Card>

        {/* 可视化面板 */}
        {queryData && (
          <Card className="p-6">
            <div className="flex items-center justify-between mb-4">
              <h2 className="text-xl font-bold text-gray-900">
                节点关系图
              </h2>
              <div className="flex gap-2">
                <Button
                  variant={visualizationMode === 'tree' ? 'default' : 'outline'}
                  onClick={() => setVisualizationMode('tree')}
                  className="text-sm"
                >
                  简单树形
                </Button>
                <Button
                  variant={visualizationMode === 'advanced' ? 'default' : 'outline'}
                  onClick={() => setVisualizationMode('advanced')}
                  className="text-sm"
                >
                  高级树形
                </Button>
                <Button
                  variant={visualizationMode === 'flow' ? 'default' : 'outline'}
                  onClick={() => setVisualizationMode('flow')}
                  className="text-sm"
                >
                  流程图视图
                </Button>
              </div>
            </div>
            <div className="bg-gray-100 rounded-lg overflow-hidden" style={{ height: '600px' }}>
              {visualizationMode === 'tree' && (
                <SpatialVisualization
                  rootNode={queryData.node}
                  children={queryData.children}
                />
              )}
              {visualizationMode === 'advanced' && (
                <AdvancedSpatialVisualization
                  rootNode={queryData.node}
                  children={queryData.children}
                />
              )}
              {visualizationMode === 'flow' && (
                <ReactFlowVisualization
                  rootNode={queryData.node}
                  children={queryData.children}
                />
              )}
            </div>
          </Card>
        )}

        {/* 空状态 */}
        {!queryData && !loading && (
          <Card className="p-12 text-center">
            <div className="text-gray-400 mb-4">
              <svg
                className="w-16 h-16 mx-auto"
                fill="none"
                stroke="currentColor"
                viewBox="0 0 24 24"
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  strokeWidth={2}
                  d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"
                />
              </svg>
            </div>
            <p className="text-gray-600">
              输入参考号开始查询
            </p>
          </Card>
        )}
      </div>
    </div>
  );
}

