use std::net::SocketAddr;
use tonic::transport::Server;

// 简化的 protobuf 定义
pub mod spatial_query {
    tonic::include_proto!("spatial_query");
}

use spatial_query::{
    spatial_query_service_server::{SpatialQueryService, SpatialQueryServiceServer},
    *,
};

/// 简化的空间查询服务实现
#[derive(Debug, Default)]
pub struct SimpleSpatialQueryService {}

#[tonic::async_trait]
impl SpatialQueryService for SimpleSpatialQueryService {
    async fn query_intersecting_elements(
        &self,
        request: tonic::Request<SpatialQueryRequest>,
    ) -> Result<tonic::Response<SpatialQueryResponse>, tonic::Status> {
        let req = request.into_inner();

        println!("🔍 收到空间查询请求: refno={}", req.refno);

        // 返回模拟数据
        let response = SpatialQueryResponse {
            success: true,
            elements: vec![IntersectingElement {
                refno: 1002,
                element_type: "PIPE".to_string(),
                element_name: "测试管道".to_string(),
                intersection_volume: 0.5,
                distance_to_center: 1.0,
                bbox: Some(BoundingBox {
                    min: Some(Point3d {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    }),
                    max: Some(Point3d {
                        x: 1.0,
                        y: 1.0,
                        z: 1.0,
                    }),
                }),
            }],
            query_time_ms: 10,
            total_elements_checked: 100,
            error_message: String::new(),
        };

        Ok(tonic::Response::new(response))
    }

    async fn get_index_stats(
        &self,
        _request: tonic::Request<IndexStatsRequest>,
    ) -> Result<tonic::Response<IndexStatsResponse>, tonic::Status> {
        let response = IndexStatsResponse {
            total_elements: 1000,
            indexed_elements: 1000,
            last_rebuild_time: "2024-08-29 16:00:00".to_string(),
            index_memory_mb: 10.5,
        };

        Ok(tonic::Response::new(response))
    }

    async fn batch_query_intersecting_elements(
        &self,
        request: tonic::Request<BatchSpatialQueryRequest>,
    ) -> Result<tonic::Response<BatchSpatialQueryResponse>, tonic::Status> {
        let req = request.into_inner();

        let mut results = Vec::new();
        for single_request in req.requests {
            let single_response = self
                .query_intersecting_elements(tonic::Request::new(single_request))
                .await?;

            results.push(single_response.into_inner());
        }

        let response = BatchSpatialQueryResponse { results };

        Ok(tonic::Response::new(response))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    env_logger::init();

    println!("🌟 启动简化的空间查询服务器...");

    let addr: SocketAddr = "127.0.0.1:9090".parse()?;
    let service = SimpleSpatialQueryService::default();

    println!("🚀 gRPC 服务器启动在: {}", addr);
    println!("📡 gRPC 端点: http://{}", addr);
    println!("🔍 提供的服务:");
    println!("   - query_intersecting_elements");
    println!("   - get_index_stats");
    println!("   - batch_query_intersecting_elements");
    println!("");
    println!("✅ 服务器已准备就绪，等待客户端连接...");

    Server::builder()
        .add_service(SpatialQueryServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
