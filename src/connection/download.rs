//! 下载端（用户）连接处理逻辑
//! 实现高效的数据接收与转发

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::io::{AsyncWrite, AsyncWriteExt, AsyncRead, AsyncReadExt};
use bytes::{Bytes, BytesMut};
use tokio::sync::{Mutex, broadcast};
use super::buffer::SharedBuffer;
use super::relay::RelayConnection;
use crate::storage::etcd;
use std::time::{Instant, Duration};

/// 下载连接配置
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// 发送缓冲区大小
    pub send_buffer_size: usize,
    /// 最大空闲时间（秒）
    pub max_idle_time: u64,
    /// 批量发送阈值
    pub batch_threshold: usize,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            send_buffer_size: 8192,
            max_idle_time: 300,
            batch_threshold: 4096,
        }
    }
}

/// 下载连接状态
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadStatus {
    /// 正在连接
    Connecting,
    /// 已验证并活跃
    Active,
    /// 正在关闭
    Closing,
    /// 已关闭
    Closed,
}

/// 下载连接结构体，处理用户数据接收
pub struct DownloadConnection {
    /// 底层 TCP 流
    stream: TcpStream,
    /// 客户端地址
    peer_addr: SocketAddr,
    /// 用户名
    username: Option<String>,
    /// 挂载点名称
    mount_name: String,
    /// 连接 ID
    conn_id: String,
    /// 状态
    status: Arc<Mutex<DownloadStatus>>,
    /// 配置
    config: DownloadConfig,
    /// 批量发送缓冲区
    batch_buffer: Arc<Mutex<BytesMut>>,
    /// 最后活动时间
    last_active: Arc<Mutex<Instant>>,
    /// etcd 客户端
    etcd_client: etcd::EtcdClient,
    /// 数据接收任务句柄
    receiver_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl DownloadConnection {
    /// 创建新的下载连接
    pub async fn new(
        stream: TcpStream,
        peer_addr: SocketAddr,
        username: Option<String>,
        mount_name: String,
        conn_id: String,
        etcd_client: etcd::EtcdClient,
        config: DownloadConfig,
    ) -> Self {
        let conn = Self {
            stream,
            peer_addr,
            username: username.clone(),
            mount_name: mount_name.clone(),
            conn_id: conn_id.clone(),
            status: Arc::new(Mutex::new(DownloadStatus::Connecting)),
            config,
            batch_buffer: Arc::new(Mutex::new(BytesMut::with_capacity(config.send_buffer_size))),
            last_active: Arc::new(Mutex::new(Instant::now())),
            etcd_client: etcd_client.clone(),
            receiver_handle: Arc::new(Mutex::new(None)),
        };

        // 记录连接信息到 etcd
        let user = username.as_deref().unwrap_or("anonymous");
        let _ = etcd_client.add_user_connection(
            user,
            &etcd::UserConnectionInfo {
                mount_name: mount_name.clone(),
                node_id: crate::NODE_ID.clone(),
                start_time: chrono::Utc::now().to_rfc3339(),
                client_ip: peer_addr.ip().to_string(),
            }
        ).await;

        conn
    }

    /// 开始接收并处理数据
    pub async fn start(self: Arc<Self>, shared_buffer: SharedBuffer) -> Result<(), std::io::Error> {
        // 订阅共享缓冲区
        let (mut rx, last_data) = shared_buffer.subscribe();
        
        // 发送最后一条数据给新连接
        if !last_data.is_empty() {
            self.send_data(&last_data).await?;
        }
        
        // 更新连接状态
        *self.status.lock().await = DownloadStatus::Active;
        
        // 启动空闲检查
        self.spawn_idle_checker();
        
        // 启动数据接收循环
        let self_clone = Arc::clone(&self);
        let handle = tokio::spawn(async move {
            loop {
                // 检查连接状态
                let status = *self_clone.status.lock().await;
                if status != DownloadStatus::Active {
                    break;
                }
                
                // 等待新数据
                match rx.recv().await {
                    Ok(data) => {
                        // 发送数据到客户端
                        if let Err(e) = self_clone.send_data(&data).await {
                            tracing::warn!("Failed to send data to {}: {}", self_clone.peer_addr, e);
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // 数据源已关闭
                        tracing::info!("Data source closed for {}", self_clone.mount_name);
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Lagged behind, missed {} messages", n);
                        // 可以选择重新同步
                    }
                }
            }
            
            // 清理连接
            let _ = self_clone.cleanup().await;
        });
        
        // 保存任务句柄以便后续取消
        *self.receiver_handle.lock().await = Some(handle);
        
        Ok(())
    }

    /// 从其他节点接收数据（用于跨节点转发）
    pub async fn start_relay(self: Arc<Self>, relay_conn: RelayConnection) -> Result<(), std::io::Error> {
        let self_clone = Arc::clone(&self);
        let handle = tokio::spawn(async move {
            let mut relay_stream = relay_conn.into_stream();
            let mut buffer = BytesMut::with_capacity(self_clone.config.send_buffer_size);
            
            loop {
                // 检查连接状态
                let status = *self_clone.status.lock().await;
                if status != DownloadStatus::Active {
                    break;
                }
                
                // 从relay连接读取数据
                match relay_stream.read_buf(&mut buffer).await {
                    Ok(0) => {
                        // 连接关闭
                        tracing::info!("Relay connection closed for {}", self_clone.mount_name);
                        break;
                    }
                    Ok(n) => {
                        // 发送数据到客户端
                        let data = buffer.split_to(n);
                        if let Err(e) = self_clone.send_data(&data).await {
                            tracing::warn!("Failed to send relay data: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Relay read error: {}", e);
                        break;
                    }
                }
            }
            
            // 清理连接
            let _ = self_clone.cleanup().await;
        });
        
        // 保存任务句柄以便后续取消
        *self.receiver_handle.lock().await = Some(handle);
        
        Ok(())
    }

    /// 发送数据到客户端，支持批量发送
    pub async fn send_data(&self, data: &[u8]) -> Result<(), std::io::Error> {
        let mut batch_buffer = self.batch_buffer.lock().await;
        
        // 如果数据加上现有缓冲区数据超过阈值，则立即发送
        if batch_buffer.len() + data.len() >= self.config.batch_threshold {
            // 先发送现有缓冲区数据
            if !batch_buffer.is_empty() {
                self.stream.write_all(&batch_buffer).await?;
                batch_buffer.clear();
            }
            
            // 直接发送当前数据
            self.stream.write_all(data).await?;
        } else {
            // 否则添加到批量缓冲区
            batch_buffer.extend_from_slice(data);
            
            // 如果缓冲区达到阈值，发送
            if batch_buffer.len() >= self.config.batch_threshold {
                self.stream.write_all(&batch_buffer).await?;
                batch_buffer.clear();
            }
        }
        
        // 更新最后活动时间
        *self.last_active.lock().await = Instant::now();
        
        Ok(())
    }

    /// 强制刷新批量缓冲区
    pub async fn flush(&self) -> Result<(), std::io::Error> {
        let mut batch_buffer = self.batch_buffer.lock().await;
        
        if !batch_buffer.is_empty() {
            self.stream.write_all(&batch_buffer).await?;
            batch_buffer.clear();
        }
        
        self.stream.flush().await
    }

    /// 启动空闲检查器
    fn spawn_idle_checker(self: &Arc<Self>) {
        let self_clone = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                // 每30秒检查一次
                tokio::time::sleep(Duration::from_secs(30)).await;
                
                let status = *self_clone.status.lock().await;
                if status != DownloadStatus::Active {
                    break;
                }
                
                let last_active = *self_clone.last_active.lock().await;
                if last_active.elapsed() >= Duration::from_secs(self_clone.config.max_idle_time) {
                    tracing::info!("Download connection from {} is idle, closing", self_clone.peer_addr);
                    *self_clone.status.lock().await = DownloadStatus::Closing;
                    
                    // 刷新缓冲区并关闭
                    let _ = self_clone.flush().await;
                    break;
                }
            }
        });
    }

    /// 清理连接资源
    pub async fn cleanup(&self) -> Result<(), std::io::Error> {
        // 更新状态
        *self.status.lock().await = DownloadStatus::Closed;
        
        // 刷新剩余数据
        let _ = self.flush().await;
        
        // 取消接收任务
        if let Some(handle) = self.receiver_handle.lock().await.take() {
            handle.abort();
        }
        
        // 从etcd中移除连接信息
        if let Some(username) = &self.username {
            let _ = self.etcd_client.remove_user_connection(username, &self.conn_id).await;
        }
        
        tracing::info!("Download connection from {} cleaned up", self.peer_addr);
        Ok(())
    }

    /// 获取挂载点名称
    pub fn mount_name(&self) -> &str {
        &self.mount_name
    }

    /// 获取连接ID
    pub fn conn_id(&self) -> &str {
        &self.conn_id
    }

    /// 获取连接状态
    pub async fn status(&self) -> DownloadStatus {
        *self.status.lock().await
    }
}

// 实现Drop trait确保资源正确释放
impl Drop for DownloadConnection {
    fn drop(&mut self) {
        // 在Drop中异步清理可能有问题，所以这里只是记录日志
        tracing::debug!("Download connection {} dropped", self.conn_id);
    }
}
