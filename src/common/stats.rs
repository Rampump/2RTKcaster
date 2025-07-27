//! 统计信息模块
//! 跟踪系统性能和连接指标

use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};
use std::time::Duration;
use tokio::time::interval;
use tracing::info;

/// 连接统计
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionStats {
    /// 总连接数
    pub total_connections: u32,
    /// 当前连接数
    pub current_connections: u32,
    /// 上传连接数
    pub upload_connections: u32,
    /// 下载连接数
    pub download_connections: u32,
    /// 中继连接数
    pub relay_connections: u32,
    /// 连接错误数
    pub connection_errors: u32,
    /// 最大并发连接数
    pub max_concurrent_connections: u32,
}

/// 数据统计
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DataStats {
    /// 总接收字节数
    pub total_rx_bytes: u64,
    /// 总发送字节数
    pub total_tx_bytes: u64,
    /// 当前接收速率 (字节/秒)
    pub current_rx_rate: u64,
    /// 当前发送速率 (字节/秒)
    pub current_tx_rate: u64,
    /// 最大接收速率
    pub max_rx_rate: u64,
    /// 最大发送速率
    pub max_tx_rate: u64,
}

/// 系统统计
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemStats {
    /// CPU 使用率 (%)
    pub cpu_usage: f32,
    /// 内存使用率 (%)
    pub memory_usage: f32,
    /// 正常运行时间 (秒)
    pub uptime_seconds: u64,
    /// 最后更新时间
    pub last_updated: DateTime<Utc>,
}

/// 挂载点统计
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MountPointStats {
    /// 挂载点名称
    pub mount_point: String,
    /// 当前连接数
    pub connections: u32,
    /// 总连接数
    pub total_connections: u32,
    /// 数据速率 (字节/秒)
    pub data_rate: u64,
    /// 可用性 (%)
    pub availability: f32,
}

/// 全局统计管理器
#[derive(Debug, Clone)]
pub struct StatsManager {
    /// 连接统计
    connection_stats: Arc<RwLock<ConnectionStats>>,
    /// 数据统计
    data_stats: Arc<RwLock<DataStats>>,
    /// 系统统计
    system_stats: Arc<RwLock<SystemStats>>,
    /// 挂载点统计
    mount_stats: Arc<RwLock<HashMap<String, MountPointStats>>>,
    /// 启动时间
    start_time: DateTime<Utc>,
    /// 运行状态
    running: Arc<RwLock<bool>>,
}

impl StatsManager {
    /// 创建新的统计管理器
    pub fn new() -> Self {
        let start_time = Utc::now();
        let mut system_stats = SystemStats::default();
        system_stats.last_updated = start_time;
        system_stats.uptime_seconds = 0;

        Self {
            connection_stats: Arc::new(RwLock::new(ConnectionStats::default())),
            data_stats: Arc::new(RwLock::new(DataStats::default())),
            system_stats: Arc::new(RwLock::new(system_stats)),
            mount_stats: Arc::new(RwLock::new(HashMap::new())),
            start_time,
            running: Arc::new(RwLock::new(true)),
        }
    }

    /// 启动统计收集器
    pub async fn start(&self) {
        let connection_stats = self.connection_stats.clone();
        let data_stats = self.data_stats.clone();
        let system_stats = self.system_stats.clone();
        let mount_stats = self.mount_stats.clone();
        let start_time = self.start_time;
        let running = self.running.clone();

        // 定期更新系统统计
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(5));
            while *running.read().await {
                interval.tick().await;

                // 更新系统统计
                let mut sys_stats = system_stats.write().await;
                sys_stats.uptime_seconds = (Utc::now() - start_time).num_seconds() as u64;
                sys_stats.last_updated = Utc::now();

                // 在实际实现中，这里应该获取真实的CPU和内存使用率
                // 简化实现：使用随机值模拟
                sys_stats.cpu_usage = (rand::random::<f32>() * 30.0) + 5.0; // 5-35%
                sys_stats.memory_usage = (rand::random::<f32>() * 40.0) + 20.0; // 20-60%
                drop(sys_stats);

                // 重置速率统计（将在下次数据传输时更新）
                let mut data = data_stats.write().await;
                data.current_rx_rate = 0;
                data.current_tx_rate = 0;
            }
        });

        info!("Statistics manager started");
    }

    /// 停止统计收集器
    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
        info!("Statistics manager stopped");
    }

    /// 增加接收字节数
    pub async fn add_rx_bytes(&self, bytes: u64, mount_point: &str) {
        let mut data = self.data_stats.write().await;
        data.total_rx_bytes += bytes;
        data.current_rx_rate += bytes;
        drop(data);

        // 更新挂载点统计
        self.update_mount_point_stats(mount_point, bytes, true).await;
    }

    /// 增加发送字节数
    pub async fn add_tx_bytes(&self, bytes: u64, mount_point: &str) {
        let mut data = self.data_stats.write().await;
        data.total_tx_bytes += bytes;
        data.current_tx_rate += bytes;

        // 更新最大速率
        if data.current_tx_rate > data.max_tx_rate {
            data.max_tx_rate = data.current_tx_rate;
        }
        drop(data);

        // 更新挂载点统计
        self.update_mount_point_stats(mount_point, bytes, false).await;
    }

    /// 更新挂载点统计
    async fn update_mount_point_stats(&self, mount_point: &str, bytes: u64, is_rx: bool) {
        let mut mounts = self.mount_stats.write().await;
        let mount_stats = mounts.entry(mount_point.to_string())
            .or_insert_with(|| MountPointStats {
                mount_point: mount_point.to_string(),
                ..MountPointStats::default()
            });

        mount_stats.data_rate += bytes;
        // 在实际实现中，这里应该根据时间计算速率
        // 简化实现：直接累加
    }

    /// 增加连接
    pub async fn increment_connection(&self, is_upload: bool, mount_point: &str) {
        let mut conn_stats = self.connection_stats.write().await;
        conn_stats.total_connections += 1;
        conn_stats.current_connections += 1;

        if conn_stats.current_connections > conn_stats.max_concurrent_connections {
            conn_stats.max_concurrent_connections = conn_stats.current_connections;
        }

        if is_upload {
            conn_stats.upload_connections += 1;
        } else {
            conn_stats.download_connections += 1;
        }
        drop(conn_stats);

        // 更新挂载点统计
        let mut mounts = self.mount_stats.write().await;
        let mount_stats = mounts.entry(mount_point.to_string())
            .or_insert_with(|| MountPointStats {
                mount_point: mount_point.to_string(),
                ..MountPointStats::default()
            });
        mount_stats.connections += 1;
        mount_stats.total_connections += 1;
    }

    /// 减少连接
    pub async fn decrement_connection(&self, is_upload: bool, mount_point: &str) {
        let mut conn_stats = self.connection_stats.write().await;
        if conn_stats.current_connections > 0 {
            conn_stats.current_connections -= 1;
        }

        if is_upload && conn_stats.upload_connections > 0 {
            conn_stats.upload_connections -= 1;
        } else if !is_upload && conn_stats.download_connections > 0 {
            conn_stats.download_connections -= 1;
        }
        drop(conn_stats);

        // 更新挂载点统计
        let mut mounts = self.mount_stats.write().await;
        if let Some(mount_stats) = mounts.get_mut(mount_point) {
            if mount_stats.connections > 0 {
                mount_stats.connections -= 1;
            }
        }
    }

    /// 增加连接错误
    pub async fn increment_error(&self) {
        let mut conn_stats = self.connection_stats.write().await;
        conn_stats.connection_errors += 1;
    }

    /// 获取连接统计
    pub async fn get_connection_stats(&self) -> ConnectionStats {
        let conn_stats = self.connection_stats.read().await;
        conn_stats.clone()
    }

    /// 获取数据统计
    pub async fn get_data_stats(&self) -> DataStats {
        let data_stats = self.data_stats.read().await;
        data_stats.clone()
    }

    /// 获取系统统计
    pub async fn get_system_stats(&self) -> SystemStats {
        let system_stats = self.system_stats.read().await;
        system_stats.clone()
    }

    /// 获取挂载点统计
    pub async fn get_mount_point_stats(&self) -> Vec<MountPointStats> {
        let mounts = self.mount_stats.read().await;
        mounts.values().cloned().collect()
    }

    /// 重置统计
    pub async fn reset_stats(&self) {
        let mut conn_stats = self.connection_stats.write().await;
        *conn_stats = ConnectionStats::default();
        drop(conn_stats);

        let mut data_stats = self.data_stats.write().await;
        *data_stats = DataStats::default();
        drop(data_stats);

        let mut mounts = self.mount_stats.write().await;
        mounts.clear();

        info!("Statistics reset");
    }
}

// 随机数生成用于模拟系统统计
use rand::Rng;