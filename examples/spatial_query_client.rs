use std::time::Duration;
use tonic::transport::Channel;
use tonic::Request;

// 引入生成的 protobuf 代码
pub mod spatial_query {
    tonic::include_proto!("spatial_query");
}

use spatial_query::{
    spatial_query_service_client::SpatialQueryServiceClient,
    *,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 空间查询服务客户端测试");
    println!("=" .repeat(50));

    // 连接到服务器
    let channel = Channel::from_static("http://127.0.0.1:9090")
        .connect_timeout(Duration::from_secs(5))
        .connect()
        .await?;
    
    let mut client = SpatialQueryServiceClient::new(channel);
    println!("✅ 已连接到服务器 127.0.0.1:9090");

    // 测试1: 获取索引统计信息
    println!("\n📊 测试1: 获取空间索引统计信息");
    test_index_stats(&mut client).await?;

    // 测试2: 单个构件相交查询
    println!("\n🔍 测试2: 查询与构件1001相交的其他构件");
    test_single_query(&mut client, 1001).await?;

    // 测试3: 查询与构件1004相交的构件（大包围盒）
    println!("\n🔍 测试3: 查询与构件1004相交的构件");
    test_single_query(&mut client, 1004).await?;

    // 测试4: 使用类型过滤查询
    println!("\n🔍 测试4: 只查询PIPE类型的构件");
    test_filtered_query(&mut client, 1001, vec!["PIPE".to_string()]).await?;

    // 测试5: 自定义包围盒查询
    println!("\n🔍 测试5: 使用自定义包围盒查询");
    test_custom_bbox_query(&mut client).await?;

    // 测试6: 批量查询
    println!("\n🔍 测试6: 批量查询多个构件");
    test_batch_query(&mut client).await?;

    // 测试7: 重建索引
    println!("\n🔄 测试7: 重建空间索引");
    test_rebuild_index(&mut client).await?;

    // 测试8: 错误处理 - 查询不存在的构件
    println!("\n❌ 测试8: 查询不存在的构件");
    test_error_handling(&mut client, 9999).await?;

    println!("\n✅ 所有测试完成!");
    Ok(())
}

/// 测试获取索引统计信息
async fn test_index_stats(client: &mut SpatialQueryServiceClient<Channel>) -> Result<(), Box<dyn std::error::Error>> {
    let request = Request::new(IndexStatsRequest {});
    let response = client.get_index_stats(request).await?;
    let stats = response.into_inner();

    println!("   总元素数量: {}", stats.total_elements);
    println!("   已索引元素: {}", stats.indexed_elements);
    println!("   最后重建时间: {}", stats.last_rebuild_time);
    println!("   索引内存占用: {:.2} MB", stats.index_memory_mb);
    
    println!("   各类型统计:");
    for type_stat in stats.type_stats {
        println!("     - {}: {} 个", type_stat.element_type, type_stat.count);
    }

    Ok(())
}

/// 测试单个构件查询
async fn test_single_query(client: &mut SpatialQueryServiceClient<Channel>, refno: u64) -> Result<(), Box<dyn std::error::Error>> {
    let request = Request::new(SpatialQueryRequest {
        refno,
        custom_bbox: None,
        element_types: vec![], // 不过滤类型
        include_self: false,   // 不包含自身
        tolerance: 0.001,
        max_results: 100,
    });

    let response = client.query_intersecting_elements(request).await?;
    let result = response.into_inner();

    if result.success {
        println!("   查询成功! 找到 {} 个相交构件", result.total_count);
        println!("   查询耗时: {} ms", result.query_time_ms);
        
        if !result.elements.is_empty() {
            println!("   相交构件详情:");
            for element in result.elements {
                println!("     - 参考号: {}, 类型: {}, 名称: {}", 
                    element.refno, element.element_type, element.element_name);
                println!("       相交体积: {:.4}, 距离中心: {:.4}", 
                    element.intersection_volume, element.distance_to_center);
                
                if let Some(bbox) = element.bbox {
                    if let (Some(min), Some(max)) = (bbox.min, bbox.max) {
                        println!("       包围盒: ({:.2}, {:.2}, {:.2}) -> ({:.2}, {:.2}, {:.2})",
                            min.x, min.y, min.z, max.x, max.y, max.z);
                    }
                }
            }
        } else {
            println!("   未找到相交构件");
        }
    } else {
        println!("   查询失败: {}", result.error_message);
    }

    Ok(())
}

/// 测试带类型过滤的查询
async fn test_filtered_query(client: &mut SpatialQueryServiceClient<Channel>, refno: u64, types: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let request = Request::new(SpatialQueryRequest {
        refno,
        custom_bbox: None,
        element_types: types.clone(),
        include_self: false,
        tolerance: 0.001,
        max_results: 100,
    });

    let response = client.query_intersecting_elements(request).await?;
    let result = response.into_inner();

    if result.success {
        println!("   过滤类型 {:?}, 找到 {} 个构件", types, result.total_count);
        for element in result.elements {
            println!("     - {}: {} ({})", element.refno, element.element_name, element.element_type);
        }
    } else {
        println!("   查询失败: {}", result.error_message);
    }

    Ok(())
}

/// 测试自定义包围盒查询
async fn test_custom_bbox_query(client: &mut SpatialQueryServiceClient<Channel>) -> Result<(), Box<dyn std::error::Error>> {
    let custom_bbox = BoundingBox {
        min: Some(Point3D { x: 0.0, y: 0.0, z: 0.0 }),
        max: Some(Point3D { x: 2.0, y: 2.0, z: 2.0 }),
    };

    let request = Request::new(SpatialQueryRequest {
        refno: 1001, // 这里refno会被忽略，因为提供了自定义包围盒
        custom_bbox: Some(custom_bbox),
        element_types: vec![],
        include_self: true, // 包含自身
        tolerance: 0.1,     // 较大容差
        max_results: 100,
    });

    let response = client.query_intersecting_elements(request).await?;
    let result = response.into_inner();

    if result.success {
        println!("   自定义包围盒查询，找到 {} 个构件", result.total_count);
        println!("   查询区域: (0,0,0) -> (2,2,2), 容差: 0.1");
        for element in result.elements {
            println!("     - {}: {} ({})", element.refno, element.element_name, element.element_type);
        }
    } else {
        println!("   查询失败: {}", result.error_message);
    }

    Ok(())
}

/// 测试批量查询
async fn test_batch_query(client: &mut SpatialQueryServiceClient<Channel>) -> Result<(), Box<dyn std::error::Error>> {
    let requests = vec![
        SpatialQueryRequest {
            refno: 1001,
            custom_bbox: None,
            element_types: vec![],
            include_self: false,
            tolerance: 0.001,
            max_results: 10,
        },
        SpatialQueryRequest {
            refno: 1002,
            custom_bbox: None,
            element_types: vec!["PIPE".to_string()],
            include_self: false,
            tolerance: 0.001,
            max_results: 10,
        },
        SpatialQueryRequest {
            refno: 1003,
            custom_bbox: None,
            element_types: vec![],
            include_self: false,
            tolerance: 0.5, // 更大的容差
            max_results: 10,
        },
    ];

    let batch_request = Request::new(BatchSpatialQueryRequest {
        requests,
        parallel_execution: true,
    });

    let response = client.batch_query_intersecting(batch_request).await?;
    let result = response.into_inner();

    println!("   批量查询完成:");
    println!("   总耗时: {} ms", result.total_time_ms);
    println!("   成功: {} 个, 失败: {} 个", result.successful_queries, result.failed_queries);

    for (i, response) in result.responses.iter().enumerate() {
        println!("   查询 {}: {} 个结果, 耗时: {} ms", 
            i + 1, response.total_count, response.query_time_ms);
    }

    Ok(())
}

/// 测试重建索引
async fn test_rebuild_index(client: &mut SpatialQueryServiceClient<Channel>) -> Result<(), Box<dyn std::error::Error>> {
    let request = Request::new(RebuildIndexRequest {
        force_rebuild: true,
        element_types: vec![], // 重建所有类型
    });

    let response = client.rebuild_spatial_index(request).await?;
    let result = response.into_inner();

    if result.success {
        println!("   索引重建成功!");
        println!("   索引元素数量: {}", result.indexed_elements);
        println!("   重建耗时: {} ms", result.rebuild_time_ms);
        println!("   消息: {}", result.message);
    } else {
        println!("   索引重建失败: {}", result.message);
    }

    Ok(())
}

/// 测试错误处理
async fn test_error_handling(client: &mut SpatialQueryServiceClient<Channel>, refno: u64) -> Result<(), Box<dyn std::error::Error>> {
    let request = Request::new(SpatialQueryRequest {
        refno,
        custom_bbox: None,
        element_types: vec![],
        include_self: false,
        tolerance: 0.001,
        max_results: 100,
    });

    let response = client.query_intersecting_elements(request).await?;
    let result = response.into_inner();

    if result.success {
        println!("   意外成功: 找到 {} 个构件", result.total_count);
    } else {
        println!("   预期的错误: {}", result.error_message);
        println!("   查询耗时: {} ms", result.query_time_ms);
    }

    Ok(())
}