//! 配置管理模块
//! 加载并解析节点配置，支持TOML/YAML格式

use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use config::{Config, ConfigError, Environment, File};
use chrono::Duration;
use std::net::SocketAddr;

/// 节点配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeConfig {
    /// 服务绑定地址
    pub bind_addr: SocketAddr,
    
    /// 节点ID
    pub node_id: String,
    
    /// 节点IP
    pub node_ip: String,
    
    /// 存储配置
    pub storage: StorageConfig,
    
    /// 连接配置
    pub connection: ConnectionConfig,
    
    /// 日志配置
    pub log: LogConfig,
}

/// 存储配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Etcd配置
    pub etcd: EtcdConfig,
    
    /// Rqlite配置
    pub rqlite: RqliteConfig,
}

/// Etcd配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EtcdConfig {
    /// 端点列表
    pub endpoints: Vec<String>,
    
    /// 连接超时(毫秒)
    pub timeout_ms: u64,
    
    /// 数据前缀
    pub prefix: String,
}

/// Rqlite配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RqliteConfig {
    /// 地址
    pub addr: String,
    
    /// 用户名
    pub username: Option<String>,
    
    /// 密码
    pub password: Option<String>,
    
    /// 连接池大小
    pub pool_size: u32,
}

/// 连接配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionConfig {
    /// 上传连接配置
    pub upload: UploadConfig,
    
    /// 下载连接配置
    pub download: DownloadConfig,
    
    /// 中继连接配置
    pub relay: RelayConfig,
}

/// 上传连接配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UploadConfig {
    /// 读取缓冲区大小
    pub read_buffer_size: usize,
    
    /// 最大空闲时间(秒)
    pub max_idle_time: u64,
    
    /// 是否启用RTCM解析
    pub enable_rtcm_parsing: bool,
}

/// 下载连接配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadConfig {
    /// 发送缓冲区大小
    pub send_buffer_size: usize,
    
    /// 最大空闲时间(秒)
    pub max_idle_time: u64,
    
    /// 批量发送阈值
    pub batch_threshold: usize,
}

/// 中继连接配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RelayConfig {
    /// 连接超时(秒)
    pub connect_timeout: u64,
    
    /// 读取缓冲区大小
    pub read_buffer_size: usize,
    
    /// 心跳间隔(秒)
    pub heartbeat_interval: u64,
    
    /// 最大重试次数
    pub max_retries: usize,
}

/// 日志配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// 日志级别
    pub level: String,
    
    /// 日志文件路径
    pub file_path: Option<PathBuf>,
    
    /// 日志轮转大小(MB)
    pub max_size_mb: u32,
    
    /// 日志保留天数
    pub max_age_days: u32,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:2101".parse().unwrap(),
            node_id: format!("node-{}", uuid::Uuid::new_v4().to_simple()),
            node_ip: "127.0.0.1".to_string(),
            storage: StorageConfig::default(),
            connection: ConnectionConfig::default(),
            log: LogConfig::default(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            etcd: EtcdConfig::default(),
            rqlite: RqliteConfig::default(),
        }
    }
}

impl Default for EtcdConfig {
    fn default() -> Self {
        Self {
            endpoints: vec!["http://127.0.0.1:2379".to_string()],
            timeout_ms: 5000,
            prefix: "/ntrip-caster/".to_string(),
        }
    }
}

impl Default for RqliteConfig {
    fn default() -> Self {
        Self {
            addr: "http://127.0.0.1:4001".to_string(),
            username: None,
            password: None,
            pool_size: 5,
        }
    }
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            upload: UploadConfig::default(),
            download: DownloadConfig::default(),
            relay: RelayConfig::default(),
        }
    }
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

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            send_buffer_size: 8192,
            max_idle_time: 300,
            batch_threshold: 4096,
        }
    }
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

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            file_path: None,
            max_size_mb: 100,
            max_age_days: 7,
        }
    }
}

/// 加载配置文件
pub fn load(config_path: &str) -> Result<NodeConfig, ConfigError> {
    let mut s = Config::new();

    // 从文件加载基础配置
    s.merge(File::with_name(config_path))?;

    // 从环境变量覆盖配置 (NTRIP_前缀)
    s.merge(Environment::with_prefix("NTRIP").separator("__"))?;

    // 解析为NodeConfig
    s.try_into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = NodeConfig::default();
        assert_eq!(config.bind_addr, "0.0.0.0:2101".parse().unwrap());
        assert_eq!(config.storage.etcd.endpoints, vec!["http://127.0.0.1:2379"]);
    }

    #[test]
    fn test_config_loading() {
        // 创建临时配置文件
        let toml_content = r#"
[bind_addr]
addr = "0.0.0.0"
port = 2102

[storage.etcd]
endpoints = ["http://etcd1:2379", "http://etcd2:2379"]

[log]
level = "debug"
"#;

        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(&temp_file, toml_content).unwrap();
        let path = temp_file.path().to_str().unwrap();

        // 加载配置
        let config = load(path).unwrap();
        assert_eq!(config.bind_addr, "0.0.0.0:2102".parse().unwrap());
        assert_eq!(config.storage.etcd.endpoints, vec!["http://etcd1:2379", "http://etcd2:2379"]);
        assert_eq!(config.log.level, "debug");
    }
}