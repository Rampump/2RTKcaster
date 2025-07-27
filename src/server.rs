//! NTRIP Caster 服务器模块
//! 实现核心服务启动与连接管理

use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use super::config::NodeConfig;
use super::storage::{etcd::EtcdClient, rqlite::RqliteClient};
use super::connection::{ConnectionManager, UploadConfig, DownloadConfig, RelayConfig};
use super::common::error::{CasterError, ServerError};
use super::relay::RelayManager;
use tracing::{info, warn, error};
use std::net::SocketAddr;

/// NTRIP服务器
pub struct NtripServer {
    config: NodeConfig,
    etcd_client: EtcdClient,
    rqlite_client: RqliteClient,
    connection_manager: Arc<ConnectionManager>,
    relay_manager: Arc<RelayManager>,
    shutdown: bool,
}

impl NtripServer {
    /// 创建新服务器实例
    pub async fn new(config: NodeConfig) -> Result<Self, CasterError> {
        // 初始化存储客户端
        let etcd_client = EtcdClient::new(
            &config.storage.etcd.endpoints,
            &config.storage.etcd.prefix
        ).await?;

        let rqlite_client = RqliteClient::new(
    &config.storage.rqlite.addr,
    config.storage.rqlite.username.clone(),
    config.storage.rqlite.password.clone()
);

// 初始化存储缓存
let cache_config = CacheConfig {
    default_ttl: config.cache.default_ttl,
    sync_interval: config.cache.sync_interval,
    max_size: config.cache.max_size,
    write_back: config.cache.write_back,
};
let storage_cache = Arc::new(StorageCache::new(
    Arc::new(rqlite_client.clone()),
    Arc::new(etcd_client.clone()),
    cache_config
).await?);

        // 初始化连接管理器
        let connection_manager = Arc::new(ConnectionManager::new(
            etcd_client.clone(),
            rqlite_client.clone(),
            config.node_id.clone(),
            Some(UploadConfig {
                read_buffer_size: config.connection.upload.read_buffer_size,
                max_idle_time: config.connection.upload.max_idle_time,
                enable_rtcm_parsing: config.connection.upload.enable_rtcm_parsing,
            }),
            Some(DownloadConfig {
                send_buffer_size: config.connection.download.send_buffer_size,
                max_idle_time: config.connection.download.max_idle_time,
                batch_threshold: config.connection.download.batch_threshold,
            }),
            Some(RelayConfig {
                connect_timeout: config.connection.relay.connect_timeout,
                read_buffer_size: config.connection.relay.read_buffer_size,
                heartbeat_interval: config.connection.relay.heartbeat_interval,
                max_retries: config.connection.relay.max_retries,
            })
        ));

        // 初始化中继管理器
        let relay_manager = Arc::new(RelayManager::new(
            config.node_id.clone(),
            etcd_client.clone()
        ));

        Ok(Self {
            config,
            etcd_client,
            rqlite_client,
            connection_manager,
            relay_manager,
            shutdown: false,
        })
    }

    /// 启动服务器
    pub async fn run(&mut self) -> Result<(), CasterError> {
        // 绑定地址
        let listener = TcpListener::bind(self.config.bind_addr)
            .await
            .map_err(|e| ServerError::BindError(format!("无法绑定地址 {}: {}", self.config.bind_addr, e)))?;

        info!("NTRIP Caster 服务器已启动，监听地址: {}", self.config.bind_addr);

        // 初始化统计管理器
        let stats_manager = Arc::new(StatsManager::new());
        stats_manager.start().await;

        // 初始化中继管理器
        let relay_manager = RelayManager::new(
            self.config.clone(),
            self.connection_manager.clone(),
            self.etcd_client.clone(),
            self.rqlite_client.clone()
        ).await?;
        let relay_manager = Arc::new(relay_manager);
        let relay_manager_clone = Arc::clone(&relay_manager);
        tokio::spawn(async move {
    if let Err(e) = relay_manager_clone.start().await {
        error!("中继管理器启动失败: {}", e);
    }
});

// 启动API服务器
let api_bind_addr = config.api.bind_addr.clone();
let api_node_id = config.node.id.clone();
let api_stats = stats_manager.clone();
let api_cache = storage_cache.clone();
let api_server = Arc::new(ntrip_server.clone());

tokio::spawn(async move {
    if let Err(e) = start_api_server(
        api_bind_addr,
        api_stats,
        api_cache,
        api_server,
        api_node_id
    ).await {
        error!("API server exited with error: {}", e);
    }
});

        // 接受连接循环
        while !self.shutdown {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    info!("新连接来自: {}", addr);
                    let connection_manager = Arc::clone(&self.connection_manager);
                    let stats_manager_clone = Arc::clone(&stats_manager);
                    let relay_manager_clone = Arc::clone(&relay_manager);

                    // 处理连接
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(
                            connection_manager,
                            stream,
                            addr,
                            stats_manager_clone,
                            relay_manager_clone
                        ).await {
                            warn!("连接处理错误: {}", e);
                        }
                    });
                }
                Err(e) => {
                    if self.shutdown {
                        break;
                    }
                    error!("接受连接失败: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        }

        Ok(())
    }

    /// 处理新连接
    async fn handle_connection(
        connection_manager: Arc<ConnectionManager>,
        stream: tokio::net::TcpStream,
        addr: SocketAddr,
        stats_manager: Arc<StatsManager>,
        relay_manager: Arc<RelayManager>
    ) -> Result<(), CasterError> {
        // 读取并解析NTRIP请求
        let mut buffer = [0; 1024];
        let n = stream.peek(&mut buffer).await?;
        if n == 0 {
            stats_manager.increment_error().await;
            return Err(ServerError::InternalError("空请求".to_string()).into());
        }

        // 更新统计信息
        stats_manager.add_rx_bytes(n as u64, "unknown").await;
        stats_manager.increment_connection(false, "unknown").await;

        let request = super::common::ntrip::NtripParser::parse_request(&buffer[..n])?;

        // 根据请求类型处理
        match request {
            super::common::ntrip::NtripRequest::Upload { mount_name, protocol, username, password } => {
                // 转换为tokio-uring TcpStream
                let uring_stream = tokio_uring::net::TcpStream::from_std(stream.into_std()?)
                    .map_err(|e| ServerError::InternalError(format!("转换IO流失败: {}", e)))?;

                // 更新统计信息
                stats_manager.decrement_connection(false, "unknown").await;
                stats_manager.increment_connection(true, &mount_name).await;

                // 处理上传连接
                let result = connection_manager.handle_upload_connection(
                    uring_stream,
                    addr,
                    mount_name.clone(),
                    username.as_deref(),
                    &password,
                    match protocol {
                        super::common::ntrip::NtripProtocol::V1 => 1,
                        super::common::ntrip::NtripProtocol::V2 => 2,
                    }
                ).await;

                if result.is_err() {
                    stats_manager.decrement_connection(true, &mount_name).await;
                    stats_manager.increment_error().await;
                }
                result
            }
            super::common::ntrip::NtripRequest::Download { mount_name, username, password } => {
                // 更新统计信息
                stats_manager.decrement_connection(false, "unknown").await;
                stats_manager.increment_connection(false, &mount_name).await;

                // 处理下载连接
                let result = connection_manager.handle_download_connection(
                    stream,
                    addr,
                    username,
                    mount_name.clone()
                ).await;

                if result.is_err() {
                    stats_manager.decrement_connection(false, &mount_name).await;
                    stats_manager.increment_error().await;
                }
                result
            }
            super::common::ntrip::NtripRequest::StrList => {
                // 处理挂载点列表请求
                let mounts = connection_manager.list_mount_points().await?;
                let response = super::common::ntrip::NtripParser::build_str_list(&mounts);
                let mut stream = stream;
                stream.write_all(&response).await?;
                stream.shutdown().await?;

                // 更新统计信息
                stats_manager.add_tx_bytes(response.len() as u64, "strlist").await;
                stats_manager.decrement_connection(false, "unknown").await;
                Ok(())
            }
            _ => {
                // 发送不支持的请求响应
                let response = super::common::ntrip::NtripParser::build_response(
                    super::common::ntrip::NtripProtocol::V1,
                    super::common::ntrip::ResponseStatus::MethodNotAllowed
                );
                let mut stream = stream;
                stream.write_all(&response).await?;
                stream.shutdown().await?;

                // 更新统计信息
                stats_manager.increment_error().await;
                stats_manager.decrement_connection(false, "unknown").await;
                Ok(())
            }
        }
    }

    /// 关闭服务器
    pub async fn shutdown(&mut self) {
        self.shutdown = true;
        info!("NTRIP Caster 服务器正在关闭...");

        // 关闭所有连接
        self.connection_manager.close_all_connections().await;
        info!("所有连接已关闭");
    }
}