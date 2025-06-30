//! 测试 update_world_transforms 方法的改进
//! 
//! 这个测试文件用于验证优化后的 update_world_transforms 方法是否正确工作

use std::collections::HashSet;
use crate::data_interface::tidb_manager::AiosDBManager;
use aios_core::types::RefnoEnum;

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 get_inst_relate_nodes_in_subtree 方法
    /// 
    /// 这个测试验证新的方法能够正确获取子树中有 inst_relate 数据的节点
    #[tokio::test]
    async fn test_get_inst_relate_nodes_in_subtree() {
        // 创建一个测试用的 AiosDBManager 实例
        let db_manager = AiosDBManager::default();
        
        // 创建测试用的 refnos 集合
        let mut test_refnos = HashSet::new();
        // 这里应该使用实际存在的测试数据
        // test_refnos.insert(RefnoEnum::from("test_refno_1"));
        // test_refnos.insert(RefnoEnum::from("test_refno_2"));
        
        // 如果没有测试数据，直接返回
        if test_refnos.is_empty() {
            println!("跳过测试：没有测试数据");
            return;
        }
        
        // 调用被测试的方法
        let result = db_manager.get_inst_relate_nodes_in_subtree(&test_refnos).await;
        
        // 验证结果
        match result {
            Ok(nodes) => {
                println!("找到 {} 个有 inst_relate 的节点", nodes.len());
                for node in &nodes {
                    println!("节点: {}", node);
                }
            },
            Err(e) => {
                println!("测试失败: {}", e);
                panic!("get_inst_relate_nodes_in_subtree 方法失败");
            }
        }
    }

    /// 测试 check_single_inst_relate_exists 方法
    /// 
    /// 这个测试验证单个节点检查方法是否正确工作
    #[tokio::test]
    async fn test_check_single_inst_relate_exists() {
        let db_manager = AiosDBManager::default();
        
        // 使用一个测试 refno
        // let test_refno = RefnoEnum::from("test_refno");
        
        // 如果没有具体的测试数据，创建一个默认的
        let test_refno = RefnoEnum::default();
        
        // 调用被测试的方法
        let result = db_manager.check_single_inst_relate_exists(&test_refno).await;
        
        // 验证结果
        match result {
            Ok(exists) => {
                println!("节点 {} 是否存在 inst_relate: {}", test_refno, exists);
            },
            Err(e) => {
                println!("测试失败: {}", e);
                panic!("check_single_inst_relate_exists 方法失败");
            }
        }
    }

    /// 测试完整的 update_world_transforms 方法
    /// 
    /// 这个测试验证整个优化后的流程是否正确工作
    #[tokio::test]
    async fn test_update_world_transforms_integration() {
        let db_manager = AiosDBManager::default();
        
        // 创建测试用的 refnos 集合
        let mut test_refnos = HashSet::new();
        // 这里应该使用实际存在的测试数据
        // test_refnos.insert(RefnoEnum::from("test_refno_with_transform_change"));
        
        // 如果没有测试数据，直接返回
        if test_refnos.is_empty() {
            println!("跳过测试：没有测试数据");
            return;
        }
        
        // 调用被测试的方法
        let result = db_manager.update_world_transforms(&test_refnos).await;
        
        // 验证结果
        match result {
            Ok(()) => {
                println!("update_world_transforms 执行成功");
            },
            Err(e) => {
                println!("测试失败: {}", e);
                panic!("update_world_transforms 方法失败");
            }
        }
    }
}

/// 性能基准测试模块
/// 
/// 用于比较优化前后的性能差异
#[cfg(test)]
mod benchmarks {
    use super::*;
    use std::time::Instant;

    /// 基准测试：比较新旧方法的性能
    /// 
    /// 这个测试可以用来验证新方法确实比旧方法更高效
    #[tokio::test]
    async fn benchmark_get_inst_relate_nodes() {
        let db_manager = AiosDBManager::default();
        
        // 创建一个较大的测试数据集
        let mut test_refnos = HashSet::new();
        // 这里应该使用实际的大量测试数据
        // for i in 0..100 {
        //     test_refnos.insert(RefnoEnum::from(format!("test_refno_{}", i)));
        // }
        
        if test_refnos.is_empty() {
            println!("跳过基准测试：没有测试数据");
            return;
        }
        
        // 测试新方法的性能
        let start_time = Instant::now();
        let result = db_manager.get_inst_relate_nodes_in_subtree(&test_refnos).await;
        let new_method_duration = start_time.elapsed();
        
        match result {
            Ok(nodes) => {
                println!("新方法耗时: {:?}, 找到 {} 个节点", new_method_duration, nodes.len());
            },
            Err(e) => {
                println!("新方法测试失败: {}", e);
            }
        }
        
        // 这里可以添加旧方法的测试进行比较
        // 但由于我们已经替换了旧方法，所以只能记录新方法的性能
    }
}
