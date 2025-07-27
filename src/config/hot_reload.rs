//! 配置热加载模块
//! 支持TOML配置文件的动态监控与热更新

use std::collections::HashMap;
use std::fs; 
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{RwLock, mpsc};
use tokio::time::sleep;
use tracing::{info, warn, error};

use crate::common::error::CasterError;
use super::NodeConfig;

/// 配置热加载管理器
#[derive(Debug, Clone)]
pub struct ConfigReloader {
    /// 当前配置
    current_config: Arc<RwLock<NodeConfig>>,
    /// 配置文件路径
    config_path: PathBuf,
    /// 监听器
    watcher: Option<RecommendedWatcher>,
    /// 变更通知通道
    event_tx: mpsc::Sender<()>,
    /// 运行状态
    running: Arc<RwLock<bool>>,
}

impl ConfigReloader {
    /// 创建新的配置热加载管理器
    pub fn new(config_path: &str) -> Result<Self, CasterError> {
        let (event_tx, _) = mpsc::channel(10);
        let config = NodeConfig::load(config_path)?;

        Ok(Self {
            current_config: Arc::new(RwLock::new(config)),
            config_path: PathBuf::from(config_path),
            watcher: None,
            event_tx,
            running: Arc::new(RwLock::new(true)),
        })
    }

    /// 获取当前配置
    pub async fn get_config(&self) -> NodeConfig {
        let config = self.current_config.read().await;
        config.clone()
    }

    /// 启动配置监控
    pub async fn start(&mut self) -> Result<(), CasterError> {
        let (tx, mut rx) = mpsc::channel(10);
        self.event_tx = tx;

        // 创建文件监听器
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                if let Ok(event) = res {
                    if let EventKind::Modify(_) = event.kind {
                        // 发送配置变更事件
                        let _ = tx.blocking_send(());
                    }
                }
            },
            notify::Config::default(),
        )?;

        // 监控配置文件
        watcher.watch(
            &self.config_path,
            RecursiveMode::NonRecursive,
        )?;

        self.watcher = Some(watcher);
        info!("Config reloader started, monitoring: {:?}", self.config_path);

        // 启动配置处理任务
        let current_config = self.current_config.clone();
        let config_path = self.config_path.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            while *running.read().await {
                if rx.recv().await.is_some() {
                    // 配置文件变更，延迟加载避免文件写入不完整
                    sleep(Duration::from_millis(500)).await;

                    match Self::reload_config(&config_path).await {
                        Ok(new_config) => {
                            let mut config = current_config.write().await;
                            *config = new_config;
                            info!("Configuration reloaded successfully");
                        },
                        Err(e) => {
                            error!("Failed to reload configuration: {}", e);
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// 重新加载配置文件
    async fn reload_config(config_path: &Path) -> Result<NodeConfig, CasterError> {
        // 读取文件内容
        let content = fs::read_to_string(config_path)
            .map_err(|e| CasterError::ConfigError(format!("Failed to read config file: {}", e)))?;

        // 解析TOML
        let toml_config: toml::Value = toml::from_str(&content)
            .map_err(|e| CasterError::ConfigError(format!("Failed to parse config file: {}", e)))?;

        // 转换为NodeConfig
        NodeConfig::from_toml(&toml_config)
    }

    /// 停止配置监控
    pub async fn stop(&mut self) {
        let mut running = self.running.write().await;
        *running = false;

        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.unwatch(&self.config_path);
        }

        info!("Config reloader stopped");
    }
}

/// 外部Caster配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExternalCasterConfig {
    /// Caster主机地址
    pub host: String,
    /// Caster端口
    pub port: u16,
    /// 挂载点列表
    pub mountpoints: Vec<String>,
    /// 中继用户名
    pub relay_user: String,
    /// 中继密码
    pub relay_password: String,
    /// 中继令牌
    pub relay_token: String,
    /// 连接超时(秒)
    pub timeout: u8,
    /// 重连间隔(秒)
    pub reconnect_interval: u8,
}

// TOML序列化支持
use serde::{Serialize, Deserialize};
use toml;