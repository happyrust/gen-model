use crate::grpc_service::spatial_query_service::{
    SpatialQueryServiceImpl, spatial_query::spatial_query_service_server::SpatialQueryServiceServer,
};
use std::net::SocketAddr;
use tonic::transport::Server;

/// 启动空间查询服务器用于测试
pub async fn start_spatial_query_server() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = "127.0.0.1:9090".parse()?;

    println!("🚀 启动空间查询服务器，地址: {}", addr);

    let spatial_service = SpatialQueryServiceImpl::new().await?;

    println!("✅ 空间索引初始化完成");

    Server::builder()
        .add_service(SpatialQueryServiceServer::new(spatial_service))
        .serve(addr)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc_service::spatial_query_service::spatial_query::{
        SpatialQueryRequest, spatial_query_service_client::SpatialQueryServiceClient,
    };
    use tonic::Request;

    #[tokio::test]
    async fn test_spatial_query_service() {
        // 启动服务器（在后台）
        tokio::spawn(async {
            if let Err(e) = start_spatial_query_server().await {
                eprintln!("服务器启动失败: {}", e);
            }
        });

        // 等待服务器启动
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // 连接客户端
        let mut client = SpatialQueryServiceClient::connect("http://127.0.0.1:9090")
            .await
            .expect("连接服务器失败");

        // 测试查询
        let request = Request::new(SpatialQueryRequest {
            refno: 1001,
            custom_bbox: None,
            element_types: vec!["PIPE".to_string(), "EQUI".to_string()],
            include_self: false,
            tolerance: 0.001,
            max_results: 100,
        });

        let response = client.query_intersecting_elements(request).await;

        match response {
            Ok(resp) => {
                let inner = resp.into_inner();
                println!("✅ 查询成功!");
                println!("   找到 {} 个相交构件", inner.total_count);
                println!("   查询耗时: {} ms", inner.query_time_ms);

                for element in inner.elements {
                    println!(
                        "   - 参考号: {}, 类型: {}, 名称: {}",
                        element.refno, element.element_type, element.element_name
                    );
                }
            }
            Err(e) => {
                panic!("查询失败: {}", e);
            }
        }

        // 测试获取统计信息
        let stats_request = Request::new(
            crate::grpc_service::spatial_query_service::spatial_query::IndexStatsRequest {},
        );
        let stats_response = client.get_index_stats(stats_request).await;

        match stats_response {
            Ok(resp) => {
                let stats = resp.into_inner();
                println!("✅ 获取统计信息成功!");
                println!("   总元素: {}", stats.total_elements);
                println!("   已索引: {}", stats.indexed_elements);
                println!("   最后重建: {}", stats.last_rebuild_time);
            }
            Err(e) => {
                println!("⚠️ 获取统计信息失败: {}", e);
            }
        }
    }
}
