//! 上传端（基准站）连接处理逻辑
//! 基于 tokio-uring 实现高性能 IO 操作

use std::net::SocketAddr;
use std::sync::Arc;
use tokio_uring::net::TcpStream;
use bytes::{Bytes, BytesMut};
use std::io::{self, Read};
use tokio::sync::Mutex;
use super::buffer::SharedBuffer;
use super::scheduler::ConnectionHandle;
use crate::storage::{etcd, rqlite};
use crate::common::rtcm::RtcmParser;

/// 上传连接配置
#[derive(Debug, Clone)]
pub struct UploadConfig {
    /// 读取缓冲区大小
    pub read_buffer_size: usize,
    /// 最大空闲时间（秒）
    pub max_idle_time: u64,
    /// 是否启用 RTCM 解析
    pub enable_rtcm_parsing: bool,
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            read_buffer_size: 8192,
            max_idle_time: 300,
            enable_rtcm_parsing: true,
        }
    }
}

/// 上传连接状态
#[derive(Debug, Clone, PartialEq)]
pub enum UploadStatus {
    /// 正在连接
    Connecting,
    /// 已验证并活跃
    Active,
    /// 正在关闭
    Closing,
    /// 已关闭
    Closed,
}

/// 上传连接结构体，处理基准站数据接收
pub struct UploadConnection {
    /// 底层 TCP 流（使用 tokio-uring 实现高性能 IO）
    stream: TcpStream,
    /// 客户端地址
    peer_addr: SocketAddr,
    /// 挂载点名称
    mount_name: String,
    /// 共享缓冲区，用于向下载端广播数据
    shared_buffer: SharedBuffer,
    /// 连接状态
    status: Arc<Mutex<UploadStatus>>,
    /// 配置参数
    config: UploadConfig,
    /// RTCM 解析器
    rtcm_parser: Option<RtcmParser>,
    /// etcd 客户端
    etcd_client: etcd::EtcdClient,
    /// 最后活动时间（用于空闲检测）
    last_active: Arc<Mutex<std::time::Instant>>,
}

impl UploadConnection {
    /// 创建新的上传连接
    pub async fn new(
        stream: TcpStream,
        peer_addr: SocketAddr,
        mount_name: String,
        shared_buffer: SharedBuffer,
        etcd_client: etcd::EtcdClient,
        config: UploadConfig,
    ) -> Self {
        let rtcm_parser = if config.enable_rtcm_parsing {
            Some(RtcmParser::new())
        } else {
            None
        };

        Self {
            stream,
            peer_addr,
            mount_name,
            shared_buffer,
            status: Arc::new(Mutex::new(UploadStatus::Connecting)),
            config,
            rtcm_parser,
            etcd_client,
            last_active: Arc::new(Mutex::new(std::time::Instant::now())),
        }
    }

    /// 验证上传连接
    pub async fn authenticate(
        &self,
        username: Option<&str>,
        password: &str,
        rqlite_client: &rqlite::RqliteClient,
        protocol_version: u8,
    ) -> Result<(), AuthenticationError> {
        // 从 rqlite 获取挂载点信息
        let mount_info = rqlite_client
            .get_mount(&self.mount_name)
            .await
            .map_err(|_| AuthenticationError::DatabaseError)?
            .ok_or(AuthenticationError::MountNotFound)?;

        // 根据协议版本验证
        match protocol_version {
            1 => {
                // NTRIP 1.0: 仅验证挂载点密码
                if !rqlite::verify_password(password, &mount_info.upload_password) {
                    return Err(AuthenticationError::InvalidCredentials);
                }
            }
            2 => {
                // NTRIP 2.0: 验证用户名、密码和挂载点权限
                let username = username.ok_or(AuthenticationError::MissingUsername)?;
                let user_info = rqlite_client
                    .get_user(username)
                    .await
                    .map_err(|_| AuthenticationError::DatabaseError)?
                    .ok_or(AuthenticationError::UserNotFound)?;

                if !rqlite::verify_password(password, &user_info.password_hash) {
                    return Err(AuthenticationError::InvalidCredentials);
                }

                if !user_info.allowed_mounts.contains(&self.mount_name) {
                    return Err(AuthenticationError::MountAccessDenied);
                }
            }
            _ => return Err(AuthenticationError::UnsupportedProtocol),
        }

        // 检查挂载点是否已在其他节点运行
        if self.etcd_client
            .get_mount_node(&self.mount_name)
            .await
            .map_err(|_| AuthenticationError::DatabaseError)?
            .is_some() {
            return Err(AuthenticationError::MountAlreadyRunning);
        }

        // 验证成功，更新状态
        *self.status.lock().await = UploadStatus::Active;
        Ok(())
    }

    /// 启动上传连接的数据接收循环
    pub async fn start(self: Arc<Self>) -> Result<(), io::Error> {
        let mut buffer = BytesMut::with_capacity(self.config.read_buffer_size);
        let mut parser = self.rtcm_parser.clone();
        
        // 在单独的任务中运行空闲检测
        self.spawn_idle_checker();

        loop {
            // 检查连接状态
            let status = *self.status.lock().await;
            if status != UploadStatus::Active {
                break;
            }

            // 分配缓冲区空间
            buffer.resize(self.config.read_buffer_size, 0);
            
            // 使用 io-uring 进行异步读取（零拷贝优化）
            let (result, n) = self.stream.read_at(buffer.as_mut(), 0).await;
            let n = result.map_err(|e| {
                tracing::error!("Read error: {}", e);
                e
            })?;

            if n == 0 {
                // 连接关闭
                tracing::info!("Upload connection from {} closed", self.peer_addr);
                break;
            }

            // 更新最后活动时间
            *self.last_active.lock().await = std::time::Instant::now();

            // 处理读取到的数据
            let data = Bytes::from(buffer[..n].to_vec());
            
            // 如果启用了解析，解析RTCM数据并更新etcd
            if let Some(parser) = &mut parser {
                if let Ok(rtcm_info) = parser.parse(&data) {
                    let _ = self.etcd_client
                        .update_mount_info(&self.mount_name, &rtcm_info)
                        .await;
                }
            }

            // 发送数据到共享缓冲区
            if let Err(e) = self.shared_buffer.send(&data).await {
                tracing::warn!("Failed to broadcast data: {}", e);
                // 如果没有订阅者，继续运行
                if !e.is_no_receiver() {
                    break;
                }
            }

            // 重置缓冲区
            buffer.clear();
        }

        // 清理连接
        self.cleanup().await;
        Ok(())
    }

    /// 启动空闲连接检查器
    fn spawn_idle_checker(self: &Arc<Self>) {
        let self_clone = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                // 每30秒检查一次
                tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
                
                let status = *self_clone.status.lock().await;
                if status != UploadStatus::Active {
                    break;
                }
                
                let last_active = *self_clone.last_active.lock().await;
                if last_active.elapsed() >= tokio::time::Duration::from_secs(self_clone.config.max_idle_time) {
                    tracing::info!("Upload connection from {} is idle, closing", self_clone.peer_addr);
                    *self_clone.status.lock().await = UploadStatus::Closing;
                    break;
                }
            }
        });
    }

    /// 清理连接资源
    async fn cleanup(&self) {
        *self.status.lock().await = UploadStatus::Closed;
        
        // 从etcd中移除挂载点在线状态
        let _ = self.etcd_client.delete_mount_online(&self.mount_name).await;
        
        tracing::info!("Upload connection for {} cleaned up", self.mount_name);
    }

    /// 获取挂载点名称
    pub fn mount_name(&self) -> &str {
        &self.mount_name
    }

    /// 获取连接状态
    pub async fn status(&self) -> UploadStatus {
        *self.status.lock().await
    }
}

/// 上传连接认证错误类型
#[derive(Debug)]
pub enum AuthenticationError {
    /// 挂载点不存在
    MountNotFound,
    /// 用户不存在
    UserNotFound,
    /// 凭据无效
    InvalidCredentials,
    /// 缺少用户名
    MissingUsername,
    /// 不允许访问挂载点
    MountAccessDenied,
    /// 挂载点已在运行
    MountAlreadyRunning,
    /// 不支持的协议版本
    UnsupportedProtocol,
    /// 数据库错误
    DatabaseError,
}

impl std::fmt::Display for AuthenticationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthenticationError::MountNotFound => write!(f, "Mount point not found"),
            AuthenticationError::UserNotFound => write!(f, "User not found"),
            AuthenticationError::InvalidCredentials => write!(f, "Invalid credentials"),
            AuthenticationError::MissingUsername => write!(f, "Username is required"),
            AuthenticationError::MountAccessDenied => write!(f, "Access to mount point denied"),
            AuthenticationError::MountAlreadyRunning => write!(f, "Mount point is already running"),
            AuthenticationError::UnsupportedProtocol => write!(f, "Unsupported protocol version"),
            AuthenticationError::DatabaseError => write!(f, "Database error"),
        }
    }
}
