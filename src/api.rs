//! 管理API模块
//! 提供HTTP接口用于系统监控和管理

use std::sync::Arc;

use axum::{
    routing::{get, post}, 
    Router, Server, Json, extract::State,
    http::StatusCode, response::IntoResponse
};
use serde::{Serialize, Deserialize};
use tracing::{info, warn};

use crate::{
    common::error::CasterError,
    stats::StatsManager,
    storage::cache::StorageCache,
    server::NtripServer
};

/// API服务状态
#[derive(Debug, Serialize)]
struct ApiStatus {
    /// 服务状态
    status: &'static str,
    /// 版本信息
    version: &'static str,
    /// 节点ID
    node_id: String,
    /// 启动时间
    uptime: u64,
}

/// 连接统计信息
#[derive(Debug, Serialize)]
struct ConnectionStats {
    /// 总连接数
    total_connections: usize,
    /// 当前连接数
    current_connections: usize,
    /// 上传连接数
    upload_connections: usize,
    /// 下载连接数
    download_connections: usize,
    /// 中继连接数
    relay_connections: usize,
    /// 连接错误数
    connection_errors: usize,
}

/// 挂载点信息
#[derive(Debug, Serialize)]
struct MountPointInfo {
    /// 挂载点名称
    name: String,
    /// 格式
    format: String,
    /// 波特率
    baud_rate: u32,
    /// 坐标
    coordinates: String,
    /// 在线状态
    online: bool,
    /// 客户端数量
    client_count: usize,
    /// 数据速率
    data_rate: f64,
}

/// API共享状态
#[derive(Debug, Clone)]
struct ApiState {
    /// 统计管理器
    stats_manager: Arc<StatsManager>,
    /// 存储缓存
    storage_cache: Arc<StorageCache>,
    /// NTRIP服务器
    ntrip_server: Arc<NtripServer>,
    /// 节点ID
    node_id: String,
    /// 启动时间
    start_time: std::time::Instant,
}

/// 获取服务状态
async fn get_status(State(state): State<ApiState>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs();
    let status = ApiStatus {
        status: "running",
        version: env!("CARGO_PKG_VERSION"),
        node_id: state.node_id.clone(),
        uptime,
    };
    (StatusCode::OK, Json(status))
}

/// 获取连接统计
async fn get_connection_stats(State(state): State<ApiState>) -> impl IntoResponse {
    let stats = state.stats_manager.get_connection_stats().await;
    let connection_stats = ConnectionStats {
        total_connections: stats.total_connections,
        current_connections: stats.current_connections,
        upload_connections: stats.upload_connections,
        download_connections: stats.download_connections,
        relay_connections: stats.relay_connections,
        connection_errors: stats.connection_errors,
    };
    (StatusCode::OK, Json(connection_stats))
}

/// 获取挂载点列表
async fn get_mount_points(State(state): State<ApiState>) -> impl IntoResponse {
    let mounts = state.storage_cache.list_mounts().await.map_err(|e| {
        warn!("Failed to get mount points: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut mount_points = Vec::new();
    for mount in mounts {
        let client_count = state.ntrip_server.get_mount_client_count(&mount.name).await;
        let data_rate = state.stats_manager.get_mount_data_rate(&mount.name).await;
        mount_points.push(MountPointInfo {
            name: mount.name,
            format: mount.format,
            baud_rate: mount.baud_rate,
            coordinates: format!("{:.6},{:.6}", mount.latitude, mount.longitude),
            online: client_count > 0,
            client_count,
            data_rate,
        });
    }

    (StatusCode::OK, Json(mount_points))
}

/// 创建API路由
fn create_router(state: ApiState) -> Router {
    Router::new()
        .route("/status", get(get_status))
        .route("/stats/connections", get(get_connection_stats))
        .route("/mounts", get(get_mount_points))
        .with_state(state)
}

/// 启动API服务器
pub async fn start_api_server(
    bind_addr: String,
    stats_manager: Arc<StatsManager>,
    storage_cache: Arc<StorageCache>,
    ntrip_server: Arc<NtripServer>,
    node_id: String
) -> Result<(), CasterError> {
    let state = ApiState {
        stats_manager,
        storage_cache,
        ntrip_server,
        node_id,
        start_time: std::time::Instant::now(),
    };

    let router = create_router(state);

    info!("Starting API server on {}", bind_addr);

    Server::bind(&bind_addr.parse()?)
        .serve(router.into_make_service())
        .await
        .map_err(|e| CasterError::ServerError(format!("API server failed to start: {}", e)))?;

    Ok(())
}