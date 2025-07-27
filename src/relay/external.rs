//! 外部Caster中继模块
//! 管理与外部NTRIP Caster的连接和数据中继

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::fmt;

use bytes::Bytes;
use reqwest::Client as HttpClient;
use tokio::sync::{mpsc, RwLock, Mutex};
use tokio::time::{interval, timeout};
use tracing::{info, warn, error, debug};
use url::Url;

use crate::common::error::CasterError;
use crate::config::ExternalCasterConfig;
use crate::connection::SharedBuffer;
use crate::storage::etcd::EtcdClient;
use crate::common::stats::StatsManager;

/// 外部中继状态
#[derive(Debug, Clone, PartialEq)]
pub enum ExternalRelayStatus {
    /// 已连接
    Connected,
    /// 连接中
    Connecting,
    /// 断开连接
    Disconnected,
    /// 重连中
    Reconnecting,
    /// 认证失败
    AuthFailed,
}

impl fmt::Display for ExternalRelayStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExternalRelayStatus::Connected => write!(f, "Connected"),
            ExternalRelayStatus::Connecting => write!(f, "Connecting"),
            ExternalRelayStatus::Disconnected => write!(f, "Disconnected"),
            ExternalRelayStatus::Reconnecting => write!(f, "Reconnecting"),
            ExternalRelayStatus::AuthFailed => write!(f, "AuthFailed"),
        }
    }
}

/// 外部Caster中继连接
#[derive(Debug)]
pub struct ExternalRelayConnection {
    /// 配置
    config: ExternalCasterConfig,
    /// HTTP客户端
    http_client: HttpClient,
    /// 状态
    status: Arc<RwLock<ExternalRelayStatus>>,
    /// 共享缓冲区映射
    buffers: Arc<RwLock<HashMap<String, Arc<SharedBuffer>>>>,
    /// 连接任务句柄
    task_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// 停止标志
    stop_flag: Arc<Mutex<bool>>,
    /// 最后活动时间
    last_active: Arc<Mutex<Instant>>,
    /// Etcd客户端
    etcd_client: Arc<EtcdClient>,
    /// 统计管理器
    stats_manager: Arc<StatsManager>,
    /// 重连退避时间(秒)
    backoff_seconds: u8,
}

impl ExternalRelayConnection {
    /// 创建新的外部Caster中继连接
    pub fn new(
        config: ExternalCasterConfig,
        etcd_client: Arc<EtcdClient>,
        stats_manager: Arc<StatsManager>
    ) -> Self {
        // 创建HTTP客户端
        let http_client = HttpClient::builder()
            .timeout(Duration::from_secs(config.timeout as u64))
            .user_agent("NTRIP-Caster-Relay/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            http_client,
            status: Arc::new(RwLock::new(ExternalRelayStatus::Disconnected)),
            buffers: Arc::new(RwLock::new(HashMap::new())),
            task_handle: Arc::new(Mutex::new(None)),
            stop_flag: Arc::new(Mutex::new(false)),
            last_active: Arc::new(Mutex::new(Instant::now())),
            etcd_client,
            stats_manager,
            backoff_seconds: 1,
        }
    }

    /// 获取当前状态
    pub async fn status(&self) -> ExternalRelayStatus {
        let status = self.status.read().await;
        status.clone()
    }

    /// 启动中继连接
    pub async fn start(&self) -> Result<(), CasterError> {
        let mut status = self.status.write().await;
        if *status != ExternalRelayStatus::Disconnected {
            return Ok(());
        }

        *status = ExternalRelayStatus::Connecting;
        drop(status);

        let mut stop_flag = self.stop_flag.lock().await;
        *stop_flag = false;
        drop(stop_flag);

        let config = self.config.clone();
        let http_client = self.http_client.clone();
        let status = self.status.clone();
        let buffers = self.buffers.clone();
        let stop_flag = self.stop_flag.clone();
        let last_active = self.last_active.clone();
        let etcd_client = self.etcd_client.clone();
        let stats_manager = self.stats_manager.clone();
        let backoff_seconds = Arc::new(Mutex::new(self.backoff_seconds));

        // 创建连接任务
        let handle = tokio::spawn(async move {
            let mut backoff_lock = backoff_seconds.lock().await;
            let mut current_backoff = *backoff_lock;
            drop(backoff_lock);

            loop {
                // 检查是否需要停止
                let stop = *stop_flag.lock().await;
                if stop {
                    break;
                }

                // 连接外部Caster并拉取数据
                match Self::connect_and_stream(
                    &config,
                    &http_client,
                    &buffers,
                    &etcd_client,
                    &stats_manager
                ).await {
                    Ok(_) => {
                        // 连接正常终止，重置退避时间
                        let mut backoff_lock = backoff_seconds.lock().await;
                        *backoff_lock = 1;
                        current_backoff = 1;
                        drop(backoff_lock);

                        // 更新状态
                        let mut status_lock = status.write().await;
                        *status_lock = ExternalRelayStatus::Disconnected;
                        drop(status_lock);

                        // 短暂延迟后重连
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    },
                    Err(e) => {
                        error!("External relay error for {}: {}", config.host, e);

                        // 更新状态
                        let mut status_lock = status.write().await;
                        *status_lock = ExternalRelayStatus::Reconnecting;
                        drop(status_lock);

                        // 更新最后活动时间
                        *last_active.lock().await = Instant::now();

                        // 指数退避重连
                        let sleep_duration = Duration::from_secs(current_backoff as u64);
                        warn!("Reconnecting to {} in {} seconds...", config.host, sleep_duration.as_secs());
                        tokio::time::sleep(sleep_duration).await;

                        // 增加退避时间，最大30秒
                        current_backoff = std::cmp::min(current_backoff * 2, 30);
                        let mut backoff_lock = backoff_seconds.lock().await;
                        *backoff_lock = current_backoff;
                        drop(backoff_lock);
                    }
                }
            }

            // 清理
            let mut status_lock = status.write().await;
            *status_lock = ExternalRelayStatus::Disconnected;
            drop(status_lock);

            info!("External relay to {} stopped", config.host);
        });

        // 保存任务句柄
        let mut task_handle = self.task_handle.lock().await;
        *task_handle = Some(handle);

        Ok(())
    }

    /// 停止中继连接
    pub async fn stop(&self) -> Result<(), CasterError> {
        // 设置停止标志
        let mut stop_flag = self.stop_flag.lock().await;
        *stop_flag = true;
        drop(stop_flag);

        // 更新状态
        let mut status = self.status.write().await;
        *status = ExternalRelayStatus::Disconnected;
        drop(status);

        // 等待任务结束
        let mut task_handle = self.task_handle.lock().await;
        if let Some(handle) = task_handle.take() {
            // 等待任务终止，最多10秒
            match tokio::time::timeout(Duration::from_secs(10), handle).await {
                Ok(_) => info!("External relay to {} stopped successfully", self.config.host),
                Err(_) => warn!("External relay to {} did not stop within timeout period", self.config.host),
            }
        }

        Ok(())
    }

    /// 连接到外部Caster并拉取数据流
    async fn connect_and_stream(
        config: &ExternalCasterConfig,
        http_client: &HttpClient,
        buffers: &Arc<RwLock<HashMap<String, Arc<SharedBuffer>>>>,
        etcd_client: &Arc<EtcdClient>,
        stats_manager: &Arc<StatsManager>
    ) -> Result<(), CasterError> {
        // 为每个挂载点创建连接
        for mountpoint in &config.mountpoints {
            let url = format!("http://{}:{}/{} ", config.host, config.port, mountpoint);
            let url = Url::parse(&url)
                .map_err(|e| CasterError::RelayError(format!("Invalid URL for {}: {}", mountpoint, e)))?;

            info!("Connecting to external caster {} for mountpoint {}", config.host, mountpoint);

            // 创建请求
            let mut request_builder = http_client.get(url);

            // 添加认证
            if !config.relay_user.is_empty() && !config.relay_password.is_empty() {
                let auth = base64::encode(format!("{}:{}", config.relay_user, config.relay_password));
                request_builder = request_builder.header("Authorization", format!("Basic {}", auth));
            }

            // 添加自定义中继头
            request_builder = request_builder.header("X-Relay-Token", &config.relay_token);
            request_builder = request_builder.header("X-Node-ID", &config.relay_token);

            // 发送请求并获取响应
            let response = request_builder.send().await
                .map_err(|e| CasterError::RelayError(format!("Connection failed for {}: {}", mountpoint, e)))?;

            // 检查响应状态
            if !response.status().is_success() {
                return Err(CasterError::RelayError(format!(
                    "Failed to connect to {} for {}: HTTP {}",
                    config.host, mountpoint, response.status()
                )));
            }

            // 获取响应体流
            let mut response_body = response.bytes_stream();

            // 获取或创建共享缓冲区
            let buffer = {
                let buffers_map = buffers.read().await;
                if let Some(buf) = buffers_map.get(mountpoint) {
                    buf.clone()
                } else {
                    drop(buffers_map);
                    let mut buffers_map = buffers.write().await;
                    let buf = Arc::new(SharedBuffer::new(1024 * 1024)); // 1MB缓冲区
                    buffers_map.insert(mountpoint.clone(), buf.clone());
                    buf
                }
            };

            // 更新状态
            etcd_client.set_online_mount_point(
                mountpoint,
                &config.host,
                &format!("{}:{}", config.host, config.port),
                0
            ).await?;

            // 启动单独的任务处理此挂载点的数据流
            let mountpoint_clone = mountpoint.clone();
            let config_clone = config.clone();
            let etcd_client_clone = etcd_client.clone();
            let stats_manager_clone = stats_manager.clone();

            tokio::spawn(async move {
                info!("Started streaming for mountpoint {} from {}", mountpoint_clone, config_clone.host);

                // 更新连接状态
                let mut status = ExternalRelayStatus::Connected;
                let mut last_data_time = Instant::now();

                while let Some(result) = response_body.next().await {
                    match result {
                        Ok(bytes) => {
                            // 更新最后数据时间
                            last_data_time = Instant::now();

                            // 记录统计信息
                            stats_manager_clone.add_rx_bytes(bytes.len() as u64, &mountpoint_clone).await;

                            // 将数据写入共享缓冲区
                            buffer.broadcast(Bytes::from(bytes.to_vec()));

                            // 更新状态
                            if status != ExternalRelayStatus::Connected {
                                status = ExternalRelayStatus::Connected;
                                info!("Reconnected to {} for mountpoint {}", config_clone.host, mountpoint_clone);

                                // 更新etcd状态
                                let _ = etcd_client_clone.set_online_mount_point(
                                    &mountpoint_clone,
                                    &config_clone.host,
                                    &format!("{}:{}", config_clone.host, config_clone.port),
                                    0
                                ).await;
                            }
                        },
                        Err(e) => {
                            warn!("Data stream error for {} from {}: {}", mountpoint_clone, config_clone.host, e);
                            break;
                        }
                    }
                }

                info!("Stopped streaming for mountpoint {} from {}", mountpoint_clone, config_clone.host);

                // 更新etcd状态
                let _ = etcd_client_clone.remove_online_mount_point(&mountpoint_clone).await;
            });
        }

        // 更新状态
        Ok(())
    }

    /// 添加共享缓冲区
    pub async fn add_buffer(&self, mountpoint: String, buffer: Arc<SharedBuffer>) {
        let mut buffers_map = self.buffers.write().await;
        buffers_map.insert(mountpoint, buffer);
    }

    /// 检查连接活跃度
    pub async fn is_active(&self) -> bool {
        let last_active = *self.last_active.lock().await;
        last_active.elapsed() < Duration::from_secs(30)
    }
}

// 基础64编码支持
use base64::Engine as _;
static BASE64_ENGINE: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;