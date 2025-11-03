import React from 'react';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import '@testing-library/jest-dom';
import SpatialVisualization from '@/components/spatial-query/SpatialVisualization';
import AdvancedSpatialVisualization from '@/components/spatial-query/AdvancedSpatialVisualization';

// Mock数据
const mockRootNode = {
  refno: 1,
  name: 'Test Space',
  noun: 'FRMW',
  node_type: 'SPACE',
  children_count: 2,
};

const mockChildren = [
  {
    refno: 2,
    name: 'Room 1',
    noun: 'PANE',
    node_type: 'ROOM',
    children_count: 3,
  },
  {
    refno: 3,
    name: 'Room 2',
    noun: 'PANE',
    node_type: 'ROOM',
    children_count: 2,
  },
];

describe('SpatialVisualization Component', () => {
  it('renders root node correctly', () => {
    render(
      <SpatialVisualization
        rootNode={mockRootNode}
        children={mockChildren}
      />
    );

    expect(screen.getByText('Test Space')).toBeInTheDocument();
    expect(screen.getByText('FRMW')).toBeInTheDocument();
  });

  it('renders children nodes', () => {
    render(
      <SpatialVisualization
        rootNode={mockRootNode}
        children={mockChildren}
      />
    );

    expect(screen.getByText('Room 1')).toBeInTheDocument();
    expect(screen.getByText('Room 2')).toBeInTheDocument();
  });

  it('displays children count', () => {
    render(
      <SpatialVisualization
        rootNode={mockRootNode}
        children={mockChildren}
      />
    );

    // 查找包含"3"和"2"的元素(子节点数)
    const countElements = screen.getAllByText(/^[0-9]$/);
    expect(countElements.length).toBeGreaterThan(0);
  });

  it('handles empty children gracefully', () => {
    render(
      <SpatialVisualization
        rootNode={mockRootNode}
        children={[]}
      />
    );

    expect(screen.getByText('Test Space')).toBeInTheDocument();
  });

  it('handles missing root node', () => {
    render(
      <SpatialVisualization
        rootNode={undefined}
        children={mockChildren}
      />
    );

    // 应该显示加载中或空状态
    expect(screen.getByText(/加载中|暂无数据/i)).toBeInTheDocument();
  });
});

describe('AdvancedSpatialVisualization Component', () => {
  it('renders search input', () => {
    render(
      <AdvancedSpatialVisualization
        rootNode={mockRootNode}
        children={mockChildren}
      />
    );

    const searchInput = screen.getByPlaceholderText(/搜索/i);
    expect(searchInput).toBeInTheDocument();
  });

  it('filters nodes by search term', async () => {
    render(
      <AdvancedSpatialVisualization
        rootNode={mockRootNode}
        children={mockChildren}
      />
    );

    const searchInput = screen.getByPlaceholderText(/搜索/i) as HTMLInputElement;
    fireEvent.change(searchInput, { target: { value: 'Room 1' } });

    await waitFor(() => {
      expect(screen.getByText('Room 1')).toBeInTheDocument();
    });
  });

  it('displays filter buttons', () => {
    render(
      <AdvancedSpatialVisualization
        rootNode={mockRootNode}
        children={mockChildren}
      />
    );

    expect(screen.getByText(/全部/i)).toBeInTheDocument();
    expect(screen.getByText(/空间/i)).toBeInTheDocument();
    expect(screen.getByText(/房间/i)).toBeInTheDocument();
    expect(screen.getByText(/构件/i)).toBeInTheDocument();
  });

  it('displays statistics', () => {
    render(
      <AdvancedSpatialVisualization
        rootNode={mockRootNode}
        children={mockChildren}
      />
    );

    // 应该显示统计信息
    expect(screen.getByText(/总计/i)).toBeInTheDocument();
  });

  it('filters by node type', async () => {
    render(
      <AdvancedSpatialVisualization
        rootNode={mockRootNode}
        children={mockChildren}
      />
    );

    const roomButton = screen.getByText(/房间/i);
    fireEvent.click(roomButton);

    await waitFor(() => {
      expect(screen.getByText('Room 1')).toBeInTheDocument();
      expect(screen.getByText('Room 2')).toBeInTheDocument();
    });
  });
});

describe('Node Type Detection', () => {
  it('correctly identifies SPACE nodes', () => {
    const spaceNode = {
      refno: 1,
      name: 'Space',
      noun: 'FRMW',
      node_type: 'SPACE',
      children_count: 0,
    };

    render(
      <SpatialVisualization
        rootNode={spaceNode}
        children={[]}
      />
    );

    expect(screen.getByText('FRMW')).toBeInTheDocument();
  });

  it('correctly identifies ROOM nodes', () => {
    const roomNode = {
      refno: 1,
      name: 'Room',
      noun: 'PANE',
      node_type: 'ROOM',
      children_count: 0,
    };

    render(
      <SpatialVisualization
        rootNode={roomNode}
        children={[]}
      />
    );

    expect(screen.getByText('PANE')).toBeInTheDocument();
  });

  it('correctly identifies COMPONENT nodes', () => {
    const componentNode = {
      refno: 1,
      name: 'Pipe',
      noun: 'PIPE',
      node_type: 'COMPONENT',
      children_count: 0,
    };

    render(
      <SpatialVisualization
        rootNode={componentNode}
        children={[]}
      />
    );

    expect(screen.getByText('PIPE')).toBeInTheDocument();
  });
});

