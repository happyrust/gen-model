'use client';

import React from 'react';
import { Handle, Position } from 'reactflow';

interface SpaceNodeProps {
  data: {
    label: string;
    refno: number;
    noun: string;
    type: string;
    childrenCount: number;
    onExpand: (refno: number) => void;
  };
}

export default function SpaceNode({ data }: SpaceNodeProps) {
  return (
    <div className="px-4 py-3 bg-blue-100 border-2 border-blue-400 rounded-lg shadow-lg hover:shadow-xl transition-shadow cursor-pointer">
      <Handle type="target" position={Position.Top} />
      
      <div className="flex items-center gap-2 mb-2">
        <span className="text-2xl">🏢</span>
        <div className="flex-1 min-w-0">
          <p className="font-bold text-sm text-gray-900 truncate">
            {data.label}
          </p>
          <p className="text-xs text-gray-600">
            {data.noun}
          </p>
        </div>
      </div>

      <div className="text-xs text-gray-700 mb-2">
        <p>ID: {data.refno}</p>
        <p>类型: {data.type}</p>
      </div>

      {data.childrenCount > 0 && (
        <button
          onClick={() => data.onExpand(data.refno)}
          className="w-full px-2 py-1 bg-blue-500 text-white text-xs rounded hover:bg-blue-600 transition-colors"
        >
          展开 ({data.childrenCount})
        </button>
      )}

      <Handle type="source" position={Position.Bottom} />
    </div>
  );
}

