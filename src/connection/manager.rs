//! 连接管理器，负责全局连接的创建、销毁和监控

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use super::{UploadConnection, DownloadConnection, RelayConnection, SharedBuffer};
use super::upload::UploadConfig;
use super::download::DownloadConfig;
use super::relay::RelayConfig;
use crate::storage::{etcd, rqlite};
use std::net::SocketAddr;
use uuid::Uuid;
use super::scheduler::ConnectionScheduler;

/// 连接统计信息
#[derive(Debug, Clone, Default)]
pub struct ConnectionStats {
    /// 活跃上传连接数
    pub active_uploads: usize,
    /// 活跃下载连接数
    pub active_downloads: usize,
    /// 活跃中继连接数
    pub active_relays: usize,
    /// 总数据上传量（字节）
    pub total_upload_bytes: u64,
    /// 总数据下载量（字节）
    pub total_download_bytes: u64,
}

/// 连接管理器
pub struct ConnectionManager {
    /// 上传连接映射：挂载点名称 -> 上传连接
    upload_connections: RwLock<HashMap<String, Arc<UploadConnection>>>,
    /// 下载连接映射：挂载点名称 -> 下载连接列表
    download_connections: RwLock<HashMap<String, Vec<Arc<DownloadConnection>>>>,
    /// 中继连接映射：挂载点名称 -> 中继连接
    relay_connections: RwLock<HashMap<String, Arc<RelayConnection>>>,
    /// 共享缓冲区映射：挂载点名称 -> 共享缓冲区
    shared_buffers: RwLock<HashMap<String, SharedBuffer>>,
    /// 上传连接配置
    upload_config: UploadConfig,
    /// 下载连接配置
    download_config: DownloadConfig,
    /// 中继连接配置
    relay_config: RelayConfig,
    /// etcd 客户端
    etcd_client: etcd::EtcdClient,
    /// rqlite 客户端
    rqlite_client: rqlite::RqliteClient,
    /// 节点 ID
    node_id: String,
    /// 连接调度器，用于多核心优化
    scheduler: ConnectionScheduler,
    /// 连接统计信息
    stats: RwLock<ConnectionStats>,
}

impl ConnectionManager {
    /// 创建新的连接管理器
    pub fn new(
        etcd_client: etcd::EtcdClient,
        rqlite_client: rqlite::RqliteClient,
        node_id: String,
        upload_config: Option<UploadConfig>,
        download_config: Option<DownloadConfig>,
        relay_config: Option<RelayConfig>,
    ) -> Self {
        Self {
            upload_connections: RwLock::new(HashMap::new()),
            download_connections: RwLock::new(HashMap::new()),
            relay_connections: RwLock::new(HashMap::new()),
            shared_buffers: RwLock::new(HashMap::new()),
            upload_config: upload_config.unwrap_or_default(),
            download_config: download_config.unwrap_or_default(),
            relay_config: relay_config.unwrap_or_default(),
            etcd_client,
            rqlite_client,
            node_id,
            scheduler: ConnectionScheduler::new(),
            stats: RwLock::new(ConnectionStats::default()),
        }
    }

    /// 处理新的上传连接
    pub async fn handle_upload_connection(
        &self,
        stream: tokio_uring::net::TcpStream,
        peer_addr: SocketAddr,
        mount_name: String,
        username: Option<&str>,
        password: &str,
        protocol_version: u8,
    ) -> Result<Arc<UploadConnection>, UploadError> {
        // 检查挂载点是否已存在活跃连接
        let uploads = self.upload_connections.read().await;
        if uploads.contains_key(&mount_name) {
            return Err(UploadError::MountAlreadyExists);
        }
        drop(uploads);

        // 获取或创建共享缓冲区
        let shared_buffer = {
            let mut buffers = self.shared_buffers.write().await;
            buffers.entry(mount_name.clone())
                .or_insert_with(|| SharedBuffer::new(mount_name.clone()))
                .clone()
        };

        // 创建上传连接
        let conn = Arc::new(UploadConnection::new(
            stream,
            peer_addr,
            mount_name.clone(),
            shared_buffer,
            self.etcd_client.clone(),
            self.upload_config.clone(),
        ).await);

        // 验证连接
        conn.authenticate(username, password, &self.rqlite_client, protocol_version)
            .await
            .map_err(UploadError::AuthenticationFailed)?;

        // 将挂载点标记为在线
        self.etcd_client.set_mount_online(
            &mount_name,
            &etcd::MountNodeInfo {
                node_id: self.node_id.clone(),
                node_ip: crate::NODE_IP.clone(),
                status: "running".to_string(),
                start_time: chrono::Utc::now().to_rfc3339(),
                format: None,
                freq: None,
                coords: None,
            }
        ).await.map_err(|_| UploadError::EtcdError)?;

        // 将连接添加到管理器
        let mut uploads = self.upload_connections.write().await;
        uploads.insert(mount_name.clone(), Arc::clone(&conn));
        drop(uploads);

        // 使用调度器在最佳核心上启动连接处理任务
        self.scheduler.spawn_connection_task(
            ConnectionTask::Upload(Arc::clone(&conn))
        ).await;

        // 更新统计信息
        let mut stats = self.stats.write().await;
        stats.active_uploads += 1;
        drop(stats);

        tracing::info!("New upload connection for {} from {}", mount_name, peer_addr);
        Ok(conn)
    }

    /// 处理新的下载连接
    pub async fn handle_download_connection(
        &self,
        stream: tokio::net::TcpStream,
        peer_addr: SocketAddr,
        username: Option<String>,
        mount_name: String,
    ) -> Result<Arc<DownloadConnection>, DownloadError> {
        // 检查用户连接数限制
        if let Some(username) = &username {
            let conn_count = self.etcd_client.count_user_connections(username).await
                .map_err(|_| DownloadError::EtcdError)?;
            
            let user_info = self.rqlite_client.get_user(username).await
                .map_err(|_| DownloadError::DatabaseError)?
                .ok_or(DownloadError::UserNotFound)?;
            
            if conn_count >= user_info.max_connections {
                return Err(Download
                return Err(DownloadError::ConnectionLimitExceeded);
            }
        }

        // 检查挂载点是否存在
        if !self.rqlite_client.mount_exists(&mount_name).await
            .map_err(|_| DownloadError::DatabaseError)? {
            return Err(DownloadError::MountNotFound);
        }

        // 检查挂载点位置
        let mount_node = self.etcd_client.get_mount_node(&mount_name).await
            .map_err(|_| DownloadError::EtcdError)?;

        let conn_id = format!("{}-{}", username.as_deref().unwrap_or("anonymous"), Uuid::new_v4());

        // 创建下载连接
        let conn = Arc::new(DownloadConnection::new(
            stream,
            peer_addr,
            username,
            mount_name.clone(),
            conn_id,
            self.etcd_client.clone(),
            self.download_config.clone(),
        ).await);

        // 确定数据来源（本地或中继）
        match mount_node {
            Some(node_info) if node_info.node_id == self.node_id => {
                // 挂载点在本地，使用共享缓冲区
                let buffers = self.shared_buffers.read().await;
                let shared_buffer = buffers.get(&mount_name)
                    .ok_or(DownloadError::MountNotActive)?
                    .clone();
                drop(buffers);

                // 启动下载连接
                conn.start(shared_buffer).await.map_err(DownloadError::IoError)?;
            }
            Some(node_info) => {
                // 挂载点在其他节点，建立中继连接
                let node_addr = format!("{}:{}", node_info.node_ip, crate::RELAY_PORT)
                    .parse()
                    .map_err(|_| DownloadError::InvalidNodeAddress)?;

                let relay_conn = RelayConnection::new(
                    node_addr,
                    mount_name.clone(),
                    self.node_id.clone(),
                    self.relay_config.clone(),
                ).await.map_err(DownloadError::RelayError)?;

                // 将中继连接添加到管理器
                let mut relays = self.relay_connections.write().await;
                relays.insert(mount_name.clone(), Arc::new(relay_conn.clone()));
                drop(relays);

                // 启动中继数据接收
                conn.start_relay(relay_conn).await.map_err(DownloadError::IoError)?;

                // 更新统计信息
                let mut stats = self.stats.write().await;
                stats.active_relays += 1;
                drop(stats);
            }
            None => {
                // 挂载点不活跃
                return Err(DownloadError::MountNotActive);
            }
        }

        // 将下载连接添加到管理器
        let mut downloads = self.download_connections.write().await;
        downloads.entry(mount_name.clone())
            .or_default()
            .push(Arc::clone(&conn));
        drop(downloads);

        // 使用调度器在最佳核心上运行连接
        self.scheduler.assign_connection(
            conn.conn_id().to_string(),
            ConnectionType::Download
        ).await;

        // 更新统计信息
        let mut stats = self.stats.write().await;
        stats.active_downloads += 1;
        drop(stats);

        tracing::info!("New download connection for {} from {}", mount_name, peer_addr);
        Ok(conn)
    }

    /// 获取挂载点的共享缓冲区
    pub async fn get_shared_buffer(&self, mount_name: &str) -> Option<SharedBuffer> {
        self.shared_buffers.read().await.get(mount_name).cloned()
    }

    /// 关闭指定挂载点的所有连接
    pub async fn close_mount_connections(&self, mount_name: &str) -> Result<(), CloseError> {
        // 关闭上传连接
        let mut uploads = self.upload_connections.write().await;
        if let Some(upload_conn) = uploads.remove(mount_name) {
            // 这里只是标记为关闭，实际关闭由连接自身处理
            tracing::info!("Closing upload connection for {}", mount_name);
        }
        drop(uploads);

        // 关闭下载连接
        let mut downloads = self.download_connections.write().await;
        if let Some(download_conns) = downloads.remove(mount_name) {
            tracing::info!("Closing {} download connections for {}", download_conns.len(), mount_name);
            for conn in download_conns {
                let _ = conn.cleanup().await;
            }
        }
        drop(downloads);

        // 关闭中继连接
        let mut relays = self.relay_connections.write().await;
        if let Some(relay_conn) = relays.remove(mount_name) {
            tracing::info!("Closing relay connection for {}", mount_name);
            let _ = relay_conn.close().await;
        }
        drop(relays);

        // 移除共享缓冲区
        let mut buffers = self.shared_buffers.write().await;
        buffers.remove(mount_name);
        drop(buffers);

        // 更新统计信息
        let mut stats = self.stats.write().await;
        stats.active_uploads = self.upload_connections.read().await.len();
        stats.active_downloads = self.download_connections.read().await
            .values()
            .map(|v| v.len())
            .sum();
        stats.active_relays = self.relay_connections.read().await.len();
        drop(stats);

        Ok(())
    }

    /// 获取当前连接统计信息
    pub async fn get_stats(&self) -> ConnectionStats {
        self.stats.read().await.clone()
    }

    /// 定期清理无效连接
    pub async fn cleanup_inactive_connections(&self) {
        // 清理上传连接
        let mut uploads = self.upload_connections.write().await;
        let mounts_to_remove: Vec<String> = uploads.iter()
            .filter(|(_, conn)| matches!(conn.status().await, upload::UploadStatus::Closed))
            .map(|(name, _)| name.clone())
            .collect();
        
        for mount_name in mounts_to_remove {
            uploads.remove(&mount_name);
            tracing::info!("Cleaned up inactive upload connection for {}", mount_name);
        }
        drop(uploads);

        // 清理下载连接
        let mut downloads = self.download_connections.write().await;
        for (mount_name, conns) in downloads.iter_mut() {
            let active_conns: Vec<Arc<DownloadConnection>> = conns.drain(..)
                .filter(|conn| !matches!(conn.status().await, download::DownloadStatus::Closed))
                .collect();
            
            if active_conns.len() < conns.len() {
                tracing::info!("Cleaned up {} inactive download connections for {}", 
                              conns.len() - active_conns.len(), mount_name);
            }
            
            *conns = active_conns;
        }
        
        // 移除没有连接的挂载点条目
        downloads.retain(|_, conns| !conns.is_empty());
        drop(downloads);

        // 清理中继连接
        let mut relays = self.relay_connections.write().await;
        let mounts_to_remove: Vec<String> = relays.iter()
            .filter(|(_, conn)| matches!(conn.status().await, relay::RelayStatus::Closed))
            .map(|(name, _)| name.clone())
            .collect();
        
        for mount_name in mounts_to_remove {
            relays.remove(&mount_name);
            tracing::info!("Cleaned up inactive relay connection for {}", mount_name);
        }
        drop(relays);

        // 更新统计信息
        let mut stats = self.stats.write().await;
        stats.active_uploads = self.upload_connections.read().await.len();
        stats.active_downloads = self.download_connections.read().await
            .values()
            .map(|v| v.len())
            .sum();
        stats.active_relays = self.relay_connections.read().await.len();
        drop(stats);
    }
}

/// 连接任务类型
enum ConnectionTask {
    Upload(Arc<UploadConnection>),
    Download(Arc<DownloadConnection>),
    Relay(Arc<RelayConnection>),
}

/// 连接类型
enum ConnectionType {
    Upload,
    Download,
    Relay,
}

/// 上传连接错误类型
#[derive(Debug)]
pub enum UploadError {
    /// 挂载点已存在
    MountAlreadyExists,
    /// 认证失败
    AuthenticationFailed(upload::AuthenticationError),
    /// etcd 错误
    EtcdError,
    /// IO 错误
    IoError(std::io::Error),
}

/// 下载连接错误类型
#[derive(Debug)]
pub enum DownloadError {
    /// 挂载点不存在
    MountNotFound,
    /// 挂载点不活跃
    MountNotActive,
    /// 用户不存在
    UserNotFound,
    /// 连接数超过限制
    ConnectionLimitExceeded,
    /// 节点地址无效
    InvalidNodeAddress,
    /// 中继连接错误
    RelayError(std::io::Error),
    /// 数据库错误
    DatabaseError,
    /// etcd 错误
    EtcdError,
    /// IO 错误
    IoError(std::io::Error),
}

/// 关闭连接错误类型
#[derive(Debug)]
pub enum CloseError {
    /// IO 错误
    IoError(std::io::Error),
    /// etcd 错误
    EtcdError,
}
