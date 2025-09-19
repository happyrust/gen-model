use aios_core::pdms_types::RefU64;
use anyhow::Result;
use nalgebra::{Point3, Vector3};
use petgraph::algo::{connected_components, dijkstra};
use petgraph::graph::{NodeIndex, UnGraph};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::grpc_service::sctn_contact_detector::CableTraySection;

/// 桥架路径分析器
pub struct SctnPathAnalyzer {
    connection_tolerance: f32,
}

impl SctnPathAnalyzer {
    pub fn new(tolerance: f32) -> Self {
        Self {
            connection_tolerance: tolerance,
        }
    }

    /// 构建桥架网络图
    pub fn build_tray_network(&self, sections: &[CableTraySection]) -> TrayNetwork {
        let mut graph = UnGraph::new_undirected();
        let mut node_map = HashMap::new();

        // 添加节点
        for section in sections {
            let node = graph.add_node(section.refno);
            node_map.insert(section.refno, node);
        }

        // 添加边（连接关系）
        for i in 0..sections.len() {
            for j in i + 1..sections.len() {
                if self.are_connected(&sections[i], &sections[j]) {
                    let node_i = node_map[&sections[i].refno];
                    let node_j = node_map[&sections[j].refno];

                    let distance = self.calculate_distance(&sections[i], &sections[j]);
                    graph.add_edge(node_i, node_j, distance);
                }
            }
        }

        TrayNetwork {
            graph,
            node_map,
            sections: sections.to_vec(),
        }
    }

    /// 判断两个桥架段是否连接
    fn are_connected(&self, sctn1: &CableTraySection, sctn2: &CableTraySection) -> bool {
        // 检查包围盒是否接近
        let dist = self.bbox_distance(&sctn1.bbox, &sctn2.bbox);
        if dist > self.connection_tolerance {
            return false;
        }

        // 检查方向是否合理
        let angle = sctn1.direction.angle(&sctn2.direction);

        // 允许直连、90度转角或T型连接
        angle < std::f32::consts::PI * 0.95
    }

    /// 计算包围盒之间的距离
    fn bbox_distance(
        &self,
        bbox1: &parry3d::bounding_volume::Aabb,
        bbox2: &parry3d::bounding_volume::Aabb,
    ) -> f32 {
        let center1 = bbox1.center();
        let center2 = bbox2.center();

        // 计算最近点距离
        let x_gap = (bbox1.mins.x - bbox2.maxs.x)
            .max(0.0)
            .max(bbox2.mins.x - bbox1.maxs.x);
        let y_gap = (bbox1.mins.y - bbox2.maxs.y)
            .max(0.0)
            .max(bbox2.mins.y - bbox1.maxs.y);
        let z_gap = (bbox1.mins.z - bbox2.maxs.z)
            .max(0.0)
            .max(bbox2.mins.z - bbox1.maxs.z);

        (x_gap * x_gap + y_gap * y_gap + z_gap * z_gap).sqrt()
    }

    /// 计算两个桥架段之间的距离
    fn calculate_distance(&self, sctn1: &CableTraySection, sctn2: &CableTraySection) -> f32 {
        (sctn1.bbox.center() - sctn2.bbox.center()).norm()
    }

    /// 查找最短路径
    pub fn find_shortest_path(
        &self,
        network: &TrayNetwork,
        from: RefU64,
        to: RefU64,
    ) -> Option<TrayPath> {
        let start_node = network.node_map.get(&from)?;
        let end_node = network.node_map.get(&to)?;

        // 使用Dijkstra算法
        let result = dijkstra(&network.graph, *start_node, Some(*end_node), |e| {
            *e.weight()
        });

        // 回溯路径
        if let Some(&distance) = result.get(end_node) {
            let path = self.reconstruct_path(network, *start_node, *end_node, &result);

            Some(TrayPath {
                sections: path,
                total_length: distance,
                from,
                to,
            })
        } else {
            None
        }
    }

    /// 重建路径
    fn reconstruct_path(
        &self,
        network: &TrayNetwork,
        start: NodeIndex,
        end: NodeIndex,
        distances: &HashMap<NodeIndex, f32>,
    ) -> Vec<RefU64> {
        let mut path = Vec::new();
        let mut current = end;

        path.push(network.graph[current]);

        while current != start {
            // 找到前驱节点
            for neighbor in network.graph.neighbors(current) {
                if let Some(&neighbor_dist) = distances.get(&neighbor) {
                    if let Some(edge) = network.graph.find_edge(neighbor, current) {
                        let edge_weight = network.graph[edge];
                        if (neighbor_dist + edge_weight - distances[&current]).abs() < 0.001 {
                            current = neighbor;
                            path.push(network.graph[current]);
                            break;
                        }
                    }
                }
            }
        }

        path.reverse();
        path
    }

    /// 分析网络连通性
    pub fn analyze_connectivity(&self, network: &TrayNetwork) -> ConnectivityAnalysis {
        let num_components = connected_components(&network.graph);

        // 找出每个连通分量
        let mut components = vec![Vec::new(); num_components];
        for (refno, &node) in &network.node_map {
            let component_id = self.find_component_id(&network.graph, node);
            components[component_id].push(*refno);
        }

        // 找出孤立节点
        let isolated_sections: Vec<RefU64> = components
            .iter()
            .filter(|c| c.len() == 1)
            .flat_map(|c| c.clone())
            .collect();

        // 找出最大连通分量
        let largest_component = components
            .iter()
            .max_by_key(|c| c.len())
            .cloned()
            .unwrap_or_default();

        ConnectivityAnalysis {
            num_components,
            components,
            isolated_sections,
            largest_component,
            is_fully_connected: num_components == 1,
        }
    }

    /// 找出节点所属的连通分量ID
    fn find_component_id(&self, graph: &UnGraph<RefU64, f32>, node: NodeIndex) -> usize {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(node);
        visited.insert(node);

        let mut min_index = node.index();

        while let Some(current) = queue.pop_front() {
            min_index = min_index.min(current.index());

            for neighbor in graph.neighbors(current) {
                if visited.insert(neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }

        min_index
    }

    /// 检测环路
    pub fn detect_loops(&self, network: &TrayNetwork) -> Vec<Loop> {
        let mut loops = Vec::new();
        let mut visited = HashSet::new();

        for node in network.graph.node_indices() {
            if !visited.contains(&node) {
                self.dfs_find_loops(
                    &network.graph,
                    node,
                    node,
                    &mut visited,
                    &mut Vec::new(),
                    &mut loops,
                );
            }
        }

        loops
    }

    /// 深度优先搜索查找环路
    fn dfs_find_loops(
        &self,
        graph: &UnGraph<RefU64, f32>,
        current: NodeIndex,
        start: NodeIndex,
        visited: &mut HashSet<NodeIndex>,
        path: &mut Vec<NodeIndex>,
        loops: &mut Vec<Loop>,
    ) {
        visited.insert(current);
        path.push(current);

        for neighbor in graph.neighbors(current) {
            if neighbor == start && path.len() > 2 {
                // 找到环路
                let loop_sections: Vec<RefU64> = path.iter().map(|&n| graph[n]).collect();

                loops.push(Loop {
                    sections: loop_sections,
                    length: path.len(),
                });
            } else if !visited.contains(&neighbor) {
                self.dfs_find_loops(graph, neighbor, start, visited, path, loops);
            }
        }

        path.pop();
    }

    /// 分析路径复杂度
    pub fn analyze_path_complexity(
        &self,
        path: &TrayPath,
        sections: &[CableTraySection],
    ) -> PathComplexity {
        let mut turns = 0;
        let mut elevations = 0;
        let mut total_angle = 0.0;

        let section_map: HashMap<RefU64, &CableTraySection> =
            sections.iter().map(|s| (s.refno, s)).collect();

        for i in 1..path.sections.len() - 1 {
            if let (Some(prev), Some(curr), Some(next)) = (
                section_map.get(&path.sections[i - 1]),
                section_map.get(&path.sections[i]),
                section_map.get(&path.sections[i + 1]),
            ) {
                // 计算转角
                let angle = prev.direction.angle(&next.direction);
                if angle > 0.1 {
                    turns += 1;
                    total_angle += angle;
                }

                // 计算高程变化
                let height_change = (next.bbox.center().y - prev.bbox.center().y).abs();
                if height_change > 0.1 {
                    elevations += 1;
                }
            }
        }

        let complexity_score =
            turns as f32 + elevations as f32 * 0.5 + total_angle / std::f32::consts::PI;

        PathComplexity {
            num_turns: turns,
            num_elevation_changes: elevations,
            total_angle_degrees: total_angle.to_degrees(),
            complexity_score,
            difficulty: match complexity_score {
                s if s < 2.0 => Difficulty::Simple,
                s if s < 5.0 => Difficulty::Moderate,
                s if s < 10.0 => Difficulty::Complex,
                _ => Difficulty::VeryComplex,
            },
        }
    }
}

/// 桥架网络
pub struct TrayNetwork {
    pub graph: UnGraph<RefU64, f32>,
    pub node_map: HashMap<RefU64, NodeIndex>,
    pub sections: Vec<CableTraySection>,
}

/// 桥架路径
#[derive(Debug, Clone)]
pub struct TrayPath {
    pub sections: Vec<RefU64>,
    pub total_length: f32,
    pub from: RefU64,
    pub to: RefU64,
}

/// 连通性分析结果
#[derive(Debug, Clone)]
pub struct ConnectivityAnalysis {
    pub num_components: usize,
    pub components: Vec<Vec<RefU64>>,
    pub isolated_sections: Vec<RefU64>,
    pub largest_component: Vec<RefU64>,
    pub is_fully_connected: bool,
}

/// 环路
#[derive(Debug, Clone)]
pub struct Loop {
    pub sections: Vec<RefU64>,
    pub length: usize,
}

/// 路径复杂度
#[derive(Debug, Clone)]
pub struct PathComplexity {
    pub num_turns: usize,
    pub num_elevation_changes: usize,
    pub total_angle_degrees: f32,
    pub complexity_score: f32,
    pub difficulty: Difficulty,
}

/// 难度等级
#[derive(Debug, Clone)]
pub enum Difficulty {
    Simple,
    Moderate,
    Complex,
    VeryComplex,
}

/// 路径优化器
pub struct PathOptimizer {
    analyzer: SctnPathAnalyzer,
}

impl PathOptimizer {
    pub fn new(tolerance: f32) -> Self {
        Self {
            analyzer: SctnPathAnalyzer::new(tolerance),
        }
    }

    /// 优化路径以减少转弯
    pub fn optimize_for_fewer_turns(
        &self,
        network: &TrayNetwork,
        path: &TrayPath,
    ) -> Option<TrayPath> {
        // 使用A*算法重新寻路，权重偏向直线
        // TODO: 实现A*算法
        None
    }

    /// 优化路径以减少高程变化
    pub fn optimize_for_level_path(
        &self,
        network: &TrayNetwork,
        path: &TrayPath,
    ) -> Option<TrayPath> {
        // 优先选择同一高程的路径
        // TODO: 实现高程优化
        None
    }

    /// 查找所有可行路径
    pub fn find_all_paths(
        &self,
        network: &TrayNetwork,
        from: RefU64,
        to: RefU64,
        max_paths: usize,
    ) -> Vec<TrayPath> {
        let mut paths = Vec::new();
        let mut visited = HashSet::new();
        let mut current_path = Vec::new();

        self.dfs_all_paths(
            network,
            from,
            to,
            &mut visited,
            &mut current_path,
            &mut paths,
            max_paths,
        );

        // 按长度排序
        paths.sort_by(|a, b| a.total_length.partial_cmp(&b.total_length).unwrap());
        paths.truncate(max_paths);
        paths
    }

    fn dfs_all_paths(
        &self,
        network: &TrayNetwork,
        current: RefU64,
        target: RefU64,
        visited: &mut HashSet<RefU64>,
        path: &mut Vec<RefU64>,
        all_paths: &mut Vec<TrayPath>,
        max_paths: usize,
    ) {
        if all_paths.len() >= max_paths {
            return;
        }

        visited.insert(current);
        path.push(current);

        if current == target {
            // 找到一条路径
            let total_length = self.calculate_path_length(network, path);
            all_paths.push(TrayPath {
                sections: path.clone(),
                total_length,
                from: path[0],
                to: *path.last().unwrap(),
            });
        } else {
            // 继续搜索
            if let Some(&node) = network.node_map.get(&current) {
                for neighbor in network.graph.neighbors(node) {
                    let neighbor_refno = network.graph[neighbor];
                    if !visited.contains(&neighbor_refno) {
                        self.dfs_all_paths(
                            network,
                            neighbor_refno,
                            target,
                            visited,
                            path,
                            all_paths,
                            max_paths,
                        );
                    }
                }
            }
        }

        path.pop();
        visited.remove(&current);
    }

    fn calculate_path_length(&self, network: &TrayNetwork, path: &[RefU64]) -> f32 {
        let mut total = 0.0;

        for i in 1..path.len() {
            if let (Some(&node1), Some(&node2)) = (
                network.node_map.get(&path[i - 1]),
                network.node_map.get(&path[i]),
            ) {
                if let Some(edge) = network.graph.find_edge(node1, node2) {
                    total += network.graph[edge];
                }
            }
        }

        total
    }
}
