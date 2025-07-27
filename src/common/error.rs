//! 错误处理模块
//! 定义统一错误类型与转换机制

use std::fmt;
use std::io::{self, Error as IoError};
use std::error::Error;
use thiserror::Error;
use serde_json::Error as SerdeError;
use etcd_client::Error as EtcdClientError;
use bcrypt::BcryptError;

/// 全局错误类型
#[derive(Debug, Error)]
pub enum CasterError {
    /// 配置错误
    #[error("配置错误: {0}")]
    Config(#[from] ConfigError),
    
    /// 服务器错误
    #[error("服务器错误: {0}")]
    Server(#[from] ServerError),
    
    /// 存储错误
    #[error("存储错误: {0}")]
    Storage(#[from] StorageError),
    
    /// 上传错误
    #[error("上传错误: {0}")]
    Upload(#[from] UploadError),
    
    /// 下载错误
    #[error("下载错误: {0}")]
    Download(#[from] DownloadError),
    
    /// 协议解析错误
    #[error("协议解析错误: {0}")]
    Parse(#[from] ParseError),
    
    /// IO错误
    #[error("IO错误: {0}")]
    Io(#[from] IoError),
    
    /// 序列化/反序列化错误
    #[error("序列化错误: {0}")]
    Serialization(#[from] SerdeError),
}

/// 配置错误
#[derive(Debug, Error)]
pub enum ConfigError {
    /// 文件未找到
    #[error("配置文件未找到: {0}")]
    FileNotFound(String),
    
    /// 解析错误
    #[error("配置解析错误: {0}")]
    ParseError(String),
    
    /// 缺少必填字段
    #[error("缺少必填配置字段: {0}")]
    MissingField(String),
    
    /// 无效值
    #[error("配置值无效: {0}")]
    InvalidValue(String),
}

/// 服务器错误
#[derive(Debug, Error)]
pub enum ServerError {
    /// 绑定端口失败
    #[error("绑定端口失败: {0}")]
    BindError(String),
    
    /// 启动失败
    #[error("服务器启动失败: {0}")]
    StartError(String),
    
    /// 关闭失败
    #[error("服务器关闭失败: {0}")]
    ShutdownError(String),
    
    /// 内部错误
    #[error("服务器内部错误: {0}")]
    InternalError(String),
}

/// 存储错误
#[derive(Debug, Error)]
pub enum StorageError {
    /// 连接错误
    #[error("存储连接错误: {0}")]
    ConnectionError(String),
    
    /// 查询错误
    #[error("存储查询错误: {0}")]
    QueryError(String),
    
    /// Etcd错误
    #[error("Etcd错误: {0}")]
    EtcdError(String),
    
    /// Rqlite错误
    #[error("Rqlite错误: {0}")]
    RqliteError(String),
    
    /// 序列化错误
    #[error("数据序列化错误: {0}")]
    SerializationError(#[from] SerdeError),
    
    /// 挂载点未找到
    #[error("挂载点未找到: {0}")]
    MountNotFound(String),
    
    /// 用户未找到
    #[error("用户未找到: {0}")]
    UserNotFound(String),
    
    /// 密码验证错误
    #[error("密码验证错误: {0}")]
    PasswordVerifyError(#[from] BcryptError),
    
    /// 无效的键格式
    #[error("无效的存储键格式: {0}")]
    InvalidKeyFormat(String),
}

/// 上传错误
#[derive(Debug, Error)]
pub enum UploadError {
    /// 认证失败
    #[error("上传认证失败: {0}")]
    AuthenticationFailed(String),
    
    /// 挂载点已存在
    #[error("挂载点已存在: {0}")]
    MountAlreadyExists,
    
    /// 挂载点已在其他节点运行
    #[error("挂载点已在其他节点运行")]
    MountAlreadyRunning,
    
    /// 数据读取错误
    #[error("上传数据读取错误: {0}")]
    ReadError(String),
    
    /// 数据解析错误
    #[error("RTCM数据解析错误: {0}")]
    DataParseError(String),
    
    /// 连接超时
    #[error("上传连接超时")]
    Timeout,
}

/// 下载错误
#[derive(Debug, Error)]
pub enum DownloadError {
    /// 认证失败
    #[error("下载认证失败: {0}")]
    AuthenticationFailed(String),
    
    /// 连接数限制
    #[error("用户连接数已达上限")]
    ConnectionLimitExceeded,
    
    /// 挂载点未找到
    #[error("挂载点未找到: {0}")]
    MountNotFound(String),
    
    /// 用户未找到
    #[error("用户未找到: {0}")]
    UserNotFound(String),
    
    /// 数据库错误
    #[error("数据库操作错误: {0}")]
    DatabaseError(String),
    
    /// Etcd错误
    #[error("Etcd操作错误: {0}")]
    EtcdError(String),
    
    /// 中继错误
    #[error("中继连接错误: {0}")]
    RelayError(String),
    
    /// 连接关闭
    #[error("下载连接已关闭")]
    ConnectionClosed,
}

/// 解析错误
#[derive(Debug, Error)]
pub enum ParseError {
    /// NTRIP协议解析错误
    #[error("NTRIP协议解析错误: {0}")]
    NtripProtocolError(String),
    
    /// RTCM数据解析错误
    #[error("RTCM数据解析错误: {0}")]
    RtcmDataError(String),
    
    /// HTTP头解析错误
    #[error("HTTP头解析错误: {0}")]
    HttpHeaderError(String),
    
    /// URL解析错误
    #[error("URL解析错误: {0}")]
    UrlParseError(String),
    
    /// 格式错误
    #[error("数据格式错误: {0}")]
    FormatError(String),
}

/// 简化错误类型定义
pub type Result<T> = std::result::Result<T, CasterError>;

// 实现Display for所有错误类型（由thiserror宏自动生成）
// 实现Error for所有错误类型（由thiserror宏自动生成）