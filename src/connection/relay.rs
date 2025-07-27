//! 跨节点数据中继模块
//! 实现不同节点间的高效数据转发

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt};
use bytes::{Bytes, BytesMut};
use tokio::sync::Mutex;
use crate::storage::etcd;
use super::buffer::SharedBuffer;

/// 中继连接配置
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// 连接超时时间（秒）
    pub connect_timeout: u64,
    /// 读取缓冲区大小
    pub read_buffer_size: usize,
    /// 心跳间隔（秒）
    pub heartbeat_interval: u64,
    /// 最大重试次数
    pub max_retries: usize,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            connect_timeout: 10,
            read_buffer_size: 8192,
            heartbeat_interval: 30,
            max_retries: 3,
        }
    }
}

/// 中继连接状态
#[derive(Debug, Clone, PartialEq)]
pub enum RelayStatus {
    /// 正在连接
    Connecting,
    /// 已连接并活跃
    Active,
    /// 正在重连
    Reconnecting,
    /// 已关闭
    Closed,
}

/// 中继连接结构体，处理跨节点数据转发
pub struct RelayConnection {
    /// 底层TCP流
    stream: Mutex<TcpStream>,
    /// 目标节点地址
    target_addr: SocketAddr,
    /// 挂载点名称
    mount_name: String,
    /// 连接状态
    status: Arc<Mutex<RelayStatus>>,
    /// 配置参数
    config: RelayConfig,
    /// 重试计数器
    retry_count: Arc<Mutex<usize>>,
    /// 节点ID
    node_id: String,
}

impl RelayConnection {
    /// 创建新的中继连接
    pub async fn new(
        target_addr: SocketAddr,
        mount_name: String,
        node_id: String,
        config: RelayConfig,
    ) -> Result<Self, std::io::Error> {
        // 尝试连接到目标节点
        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(config.connect_timeout),
            TcpStream::connect(target_addr)
        ).await??;
        
        // 发送中继请求
        let request = format!("RELAY {}\r\nNode-ID: {}\r\n\r\n", mount_name, node_id);
        stream.write_all(request.as_bytes()).await?;
        
        // 读取响应
        let mut response = BytesMut::with_capacity(128);
        stream.read_until(b'\n', &mut response).await?;
        
        if !response.starts_with(b"200 OK") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "Relay request rejected"
            ));
        }
        
        Ok(Self {
            stream: Mutex::new(stream),
            target_addr,
            mount_name,
            status: Arc::new(Mutex::new(RelayStatus::Active)),
            config,
            retry_count: Arc::new(Mutex::new(0)),
            node_id,
        })
    }

    /// 启动中继连接，从远程节点接收数据并转发到本地缓冲区
    pub async fn start(self: Arc<Self>, shared_buffer: SharedBuffer) -> Result<(), std::io::Error> {
        // 启动心跳检测
        self.spawn_heartbeat();
        
        let mut buffer = BytesMut::with_capacity(self.config.read_buffer_size);
        
        loop {
            // 检查连接状态
            let status = *self.status.lock().await;
            if status == RelayStatus::Closed {
                break;
            }
            
            // 读取数据
            let stream = self.stream.lock().await;
            let n = match stream.read_buf(&mut buffer).await {
                Ok(0) => {
                    // 连接关闭，尝试重连
                    drop(stream); // 释放锁以便重连
                    if !self.reconnect().await {
                        break;
                    }
                    continue;
                }
                Ok(n) => n,
                Err(e) => {
                    tracing::error!("Relay read error: {}", e);
                    drop(stream); // 释放锁以便重连
                    if !self.reconnect().await {
                        break;
                    }
                    continue;
                }
            };
            
            // 处理数据
            let data = Bytes::from(buffer.split_to(n));
            
            // 发送到本地共享缓冲区
            if let Err(e) = shared_buffer.send(&data).await {
                tracing::warn!("Failed to send relay data to buffer: {}", e);
                if !e.is_no_receiver() {
                    break;
                }
            }
        }
        
        // 标记连接为已关闭
        *self.status.lock().await = RelayStatus::Closed;
        Ok(())
    }

    /// 从重连中恢复
    async fn reconnect(&self) -> bool {
        let mut status = self.status.lock().await;
        let mut retries = self.retry_count.lock().await;
        
        // 检查是否已达到最大重试次数
        if *retries >= self.config.max_retries {
            tracing::error!("Max retries reached for relay to {}", self.target_addr);
            *status = RelayStatus::Closed;
            return false;
        }
        
        // 递增重试计数器
        *retries += 1;
        *status = RelayStatus::Reconnecting;
        drop(status); // 释放状态锁
        
        tracing::info!("Reconnecting to {} (attempt {}/{})", 
                      self.target_addr, *retries, self.config.max_retries);
        
        // 指数退避重试
        let backoff = std::time::Duration::from_secs(2u64.pow(*retries as u32));
        tokio::time::sleep(backoff).await;
        
        // 尝试重新连接
        match TcpStream::connect(self.target_addr).await {
            Ok(new_stream) => {
                // 发送中继请求
                let request = format!("RELAY {}\r\nNode-ID: {}\r\n\r\n", 
                                     self.mount_name, self.node_id);
                if new_stream.write_all(request.as_bytes()).await.is_err() {
                    return false;
                }
                
                // 读取响应
                let mut response = BytesMut::with_capacity(128);
                if new_stream.read_until(b'\n', &mut response).await.is_err() {
                    return false;
                }
                
                if !response.starts_with(b"200 OK") {
                    return false;
                }
                
                // 连接成功，更新流和状态
                *self.stream.lock().await = new_stream;
                *self.retry_count.lock().await = 0;
                
                let mut status = self.status.lock().await;
                *status = RelayStatus::Active;
                tracing::info!("Reconnected to {}", self.target_addr);
                true
            }
            Err(e) => {
                tracing::error!("Reconnection failed: {}", e);
                false
            }
        }
    }

    /// 启动心跳检测任务
    fn spawn_heartbeat(&self) {
        let self_clone = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                // 检查连接状态
                let status = *self_clone.status.lock().await;
                if status != RelayStatus::Active {
                    break;
                }
                
                // 等待心跳间隔
                tokio::time::sleep(std::time::Duration::from_secs(
                    self_clone.config.heartbeat_interval
                )).await;
                
                // 发送心跳包
                let mut stream = self_clone.stream.lock().await;
                if let Err(e) = stream.write_all(b"HEARTBEAT\r\n").await {
                    tracing::warn!("Heartbeat failed: {}", e);
                    drop(stream); // 释放锁以便重连
                    
                    // 触发重连
                    if !self_clone.reconnect().await {
                        break;
                    }
                }
            }
        });
    }

    /// 将中继连接转换为TcpStream
    pub async fn into_stream(self) -> TcpStream {
        self.stream.lock().await.clone()
    }

    /// 关闭中继连接
    pub async fn close(&self) -> Result<(), std::io::Error> {
        *self.status.lock().await = RelayStatus::Closed;
        let mut stream = self.stream.lock().await;
        stream.shutdown().await
    }

    /// 获取当前连接状态
    pub async fn status(&self) -> RelayStatus {
        *self.status.lock().await
    }
}
