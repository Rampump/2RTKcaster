//! 高效缓冲区管理与批量发送机制
//! 基于 bytes crate 和批处理优化

use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use bytes::{Bytes, BytesMut, BufMut};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use std::collections::VecDeque;
use std::time::{Instant, Duration};

/// 共享缓冲区，用于在上传端和下载端之间传递数据
#[derive(Clone, Debug)]
pub struct SharedBuffer {
    /// 广播通道，用于通知新数据可用
    tx: broadcast::Sender<Bytes>,
    /// 最后一次数据的缓存，用于新连接快速获取最新数据
    last_data: Arc<Mutex<Bytes>>,
    /// 挂载点名称
    mount_name: String,
}

impl SharedBuffer {
    /// 创建新的共享缓冲区
    pub fn new(mount_name: String) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            tx,
            last_data: Arc::new(Mutex::new(Bytes::new())),
            mount_name,
        }
    }

    /// 发送数据到缓冲区
    pub async fn send(&self, data: &[u8]) -> Result<(), broadcast::error::SendError<Bytes>> {
        let bytes = Bytes::copy_from_slice(data);
        
        // 更新最后数据缓存
        *self.last_data.lock().await = bytes.clone();
        
        // 广播数据
        self.tx.send(bytes)
    }

    /// 订阅缓冲区数据
    pub fn subscribe(&self) -> (broadcast::Receiver<Bytes>, Bytes) {
        let mut rx = self.tx.subscribe();
        let last_data = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { self.last_data.lock().await.clone() });
        
        (rx, last_data)
    }

    /// 获取挂载点名称
    pub fn mount_name(&self) -> &str {
        &self.mount_name
    }
}

/// 批量发送器，用于高效地批量发送数据到多个连接
pub struct BatchSender {
    /// 待发送的数据队列
    queue: Mutex<VecDeque<Bytes>>,
    /// 批量发送任务是否在运行
    running: Mutex<bool>,
    /// 最大批量大小
    max_batch_size: usize,
    /// 最大等待时间（毫秒）
    max_wait_time: u64,
}

impl BatchSender {
    /// 创建新的批量发送器
    pub fn new(max_batch_size: usize, max_wait_time: u64) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            running: Mutex::new(false),
            max_batch_size,
            max_wait_time,
        }
    }

    /// 添加数据到发送队列
    pub async fn queue_data(&self, data: Bytes) {
        let mut queue = self.queue.lock().await;
        queue.push_back(data);
        
        // 如果发送任务未运行，则启动它
        let mut running = self.running.lock().await;
        if !*running {
            *running = true;
            let self_clone = self.clone();
            tokio::spawn(async move {
                self_clone.process_queue().await;
                *self_clone.running.lock().await = false;
            });
        }
    }

    /// 处理发送队列，批量发送数据
    async fn process_queue(&self) {
        let start_time = Instant::now();
        let mut batch = Vec::with_capacity(self.max_batch_size);
        
        // 收集批量数据
        loop {
            let mut queue = self.queue.lock().await;
            
            // 检查是否达到批量大小或超时
            if batch.len() >= self.max_batch_size || 
               start_time.elapsed() >= Duration::from_millis(self.max_wait_time) {
                break;
            }
            
            // 从队列中获取数据
            if let Some(data) = queue.pop_front() {
                batch.push(data);
            } else {
                // 队列为空，退出循环
                break;
            }
        }
        
        // 如果有数据要发送，则合并并发送
        if !batch.is_empty() {
            self.send_batch(batch).await;
        }
    }

    /// 发送批量数据（具体实现由子类提供）
    async fn send_batch(&self, _batch: Vec<Bytes>) {
        // 由具体实现重写
    }
}

// 实现 Clone 以允许在任务间传递
impl Clone for BatchSender {
    fn clone(&self) -> Self {
        Self {
            queue: self.queue.clone(),
            running: self.running.clone(),
            max_batch_size: self.max_batch_size,
            max_wait_time: self.max_wait_time,
        }
    }
}

/// 针对多个下载连接的批量发送器
pub struct DownloadBatchSender {
    inner: BatchSender,
    connections: Arc<Mutex<Vec<download::DownloadConnection>>>,
}

impl DownloadBatchSender {
    /// 创建新的下载批量发送器
    pub fn new(connections: Arc<Mutex<Vec<download::DownloadConnection>>>, 
               max_batch_size: usize, 
               max_wait_time: u64) -> Self {
        Self {
            inner: BatchSender::new(max_batch_size, max_wait_time),
            connections,
        }
    }
    
    /// 添加数据到发送队列
    pub async fn queue_data(&self, data: Bytes) {
        self.inner.queue_data(data).await;
    }
    
    /// 实际发送批量数据到所有连接
    async fn send_to_connections(&self, data: &[u8]) {
        let mut connections = self.connections.lock().await;
        let mut active_connections = Vec::with_capacity(connections.len());
        
        // 向每个连接发送数据，同时过滤掉已关闭的连接
        for conn in connections.drain(..) {
            if let Err(e) = conn.send_data(data).await {
                tracing::warn!("Failed to send data to connection: {}", e);
                // 连接已关闭，不添加回活跃连接列表
            } else {
                active_connections.push(conn);
            }
        }
        
        // 更新活跃连接列表
        *connections = active_connections;
    }
}

impl BatchSender for DownloadBatchSender {
    async fn send_batch(&self, batch: Vec<Bytes>) {
        // 合并批量数据
        let mut combined = BytesMut::new();
        for data in batch {
            combined.put_slice(&data);
        }
        
        // 发送合并后的数据
        self.send_to_connections(&combined).await;
    }
}
