//! NTRIP 协议解析模块
//! 实现NTRIP协议v1/v2版本的请求解析与响应构建

use std::str::FromStr;
use std::collections::HashMap;
use bytes::{Bytes, BytesMut};
use super::error::{CasterError, ParseError};
use std::fmt;

/// NTRIP协议版本
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NtripProtocol {
    /// 版本1
    V1,
    /// 版本2
    V2,
}

impl fmt::Display for NtripProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NtripProtocol::V1 => write!(f, "NTRIP/1.0"),
            NtripProtocol::V2 => write!(f, "NTRIP/2.0"),
        }
    }
}

/// NTRIP响应状态码
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseStatus {
    /// 成功
    Ok,
    /// 未授权
    Unauthorized,
    /// 挂载点未找到
    MountPointNotFound,
    /// 方法不允许
    MethodNotAllowed,
    /// 内部服务器错误
    InternalServerError,
    /// 服务不可用
    ServiceUnavailable,
}

impl ResponseStatus {
    /// 获取状态码
    pub fn code(&self) -> u16 {
        match self {
            ResponseStatus::Ok => 200,
            ResponseStatus::Unauthorized => 401,
            ResponseStatus::MountPointNotFound => 404,
            ResponseStatus::MethodNotAllowed => 405,
            ResponseStatus::InternalServerError => 500,
            ResponseStatus::ServiceUnavailable => 503,
        }
    }

    /// 获取状态描述
    pub fn description(&self) -> &str {
        match self {
            ResponseStatus::Ok => "OK",
            ResponseStatus::Unauthorized => "Unauthorized",
            ResponseStatus::MountPointNotFound => "Mount Point Not Found",
            ResponseStatus::MethodNotAllowed => "Method Not Allowed",
            ResponseStatus::InternalServerError => "Internal Server Error",
            ResponseStatus::ServiceUnavailable => "Service Unavailable",
        }
    }
}

/// NTRIP请求类型
#[derive(Debug, Clone)]
pub enum NtripRequest {
    /// 上传请求
    Upload {
        /// 挂载点名称
        mount_name: String,
        /// 协议版本
        protocol: NtripProtocol,
        /// 用户名
        username: Option<String>,
        /// 密码
        password: String,
    },
    /// 下载请求
    Download {
        /// 挂载点名称
        mount_name: String,
        /// 用户名
        username: Option<String>,
        /// 密码
        password: Option<String>,
    },
    /// 挂载点列表请求
    StrList,
    /// 版本请求
    Version,
    /// 未知请求
    Unknown(String),
}

/// NTRIP请求解析器
pub struct NtripParser;

impl NtripParser {
    /// 解析NTRIP请求
    pub fn parse_request(data: &[u8]) -> Result<NtripRequest, ParseError> {
        let data_str = String::from_utf8_lossy(data);
        let mut lines = data_str.lines();

        // 解析请求行
        let request_line = lines.next()
            .ok_or_else(|| ParseError::NtripProtocolError("空请求".to_string()))?;

        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(ParseError::NtripProtocolError(format!("无效请求行: {}", request_line)));
        }

        match parts[0] {
            "GET" => {
                // 处理GET请求
                if parts[1] == "/" {
                    Ok(NtripRequest::StrList)
                } else if parts[1] == "/VERSION" {
                    Ok(NtripRequest::Version)
                } else {
                    // 下载请求: GET /mountpoint HTTP/1.1
                    let mount_name = parts[1].strip_prefix('/')
                        .ok_or_else(|| ParseError::NtripProtocolError(format!("无效挂载点: {}", parts[1])))?;

                    // 解析头部
                    let mut headers = HashMap::new();
                    for line in lines {
                        if line.is_empty() {
                            break;
                        }
                        if let Some((key, value)) = line.split_once(':') {
                            headers.insert(key.trim().to_lowercase(), value.trim().to_string());
                        }
                    }

                    // 解析认证信息
                    let auth = headers.get("authorization")
                        .and_then(|s| s.strip_prefix("Basic "))
                        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
                        .and_then(|b| String::from_utf8(b).ok());

                    let (username, password) = if let Some(auth_str) = auth {
                        auth_str.split_once(':')
                            .map(|(u, p)| (Some(u.to_string()), Some(p.to_string())))
                            .unwrap_or((None, None))
                    } else {
                        (None, None)
                    };

                    Ok(NtripRequest::Download {
                        mount_name: mount_name.to_string(),
                        username,
                        password,
                    })
                }
            },
            "POST" => {
                // 处理POST请求 (NTRIP 2.0上传)
                let mount_name = parts[1].strip_prefix('/')
                    .ok_or_else(|| ParseError::NtripProtocolError(format!("无效挂载点: {}", parts[1])))?;

                // 解析头部
                let mut headers = HashMap::new();
                let mut protocol = NtripProtocol::V1;

                for line in lines {
                    if line.is_empty() {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        let key = key.trim().to_lowercase();
                        headers.insert(key.clone(), value.trim().to_string());

                        if key == "ntrip-version" {
                            protocol = match value.trim() {
                                "NTRIP/2.0" => NtripProtocol::V2,
                                _ => NtripProtocol::V1,
                            };
                        }
                    }
                }

                // 解析认证信息
                let auth = headers.get("authorization")
                    .and_then(|s| s.strip_prefix("Basic "))
                    .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
                    .and_then(|b| String::from_utf8(b).ok())
                    .ok_or_else(|| ParseError::NtripProtocolError("缺少认证信息".to_string()))?;

                let (username, password) = auth.split_once(':')
                    .map(|(u, p)| (Some(u.to_string()), p.to_string()))
                    .ok_or_else(|| ParseError::NtripProtocolError("无效认证格式".to_string()))?;

                Ok(NtripRequest::Upload {
                    mount_name: mount_name.to_string(),
                    protocol,
                    username,
                    password,
                })
            },
            "SOURCE" => {
                // 处理SOURCE请求 (NTRIP 1.0上传)
                let mount_name = parts[1].to_string();

                // 解析头部
                let mut headers = HashMap::new();
                for line in lines {
                    if line.is_empty() {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        headers.insert(key.trim().to_lowercase(), value.trim().to_string());
                    }
                }

                // 解析认证信息
                let auth = headers.get("authorization")
                    .and_then(|s| s.strip_prefix("Basic "))
                    .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
                    .and_then(|b| String::from_utf8(b).ok())
                    .ok_or_else(|| ParseError::NtripProtocolError("缺少认证信息".to_string()))?;

                let (username, password) = auth.split_once(':')
                    .map(|(u, p)| (Some(u.to_string()), p.to_string()))
                    .ok_or_else(|| ParseError::NtripProtocolError("无效认证格式".to_string()))?;

                Ok(NtripRequest::Upload {
                    mount_name,
                    protocol: NtripProtocol::V1,
                    username,
                    password,
                })
            },
            _ => Ok(NtripRequest::Unknown(request_line.to_string())),
        }
    }

    /// 构建NTRIP响应
    pub fn build_response(protocol: NtripProtocol, status: ResponseStatus) -> Bytes {
        let mut response = BytesMut::new();

        // 响应行
        response.extend_from_slice(format!("{} {}
", status.code(), status.description()).as_bytes());

        // 头部
        response.extend_from_slice(b"Server: NTRIP Caster/1.0
");
        response.extend_from_slice(format!("Ntrip-Version: {}
", protocol).as_bytes());
        response.extend_from_slice(b"Connection: close
");
        response.extend_from_slice(b"
");

        response.freeze()
    }

    /// 构建挂载点列表响应
    pub fn build_str_list(mounts: &[MountPointInfo]) -> Bytes {
        let mut response = BytesMut::new();

        // 响应行
        response.extend_from_slice(b"200 OK
");
        response.extend_from_slice(b"Content-Type: text/plain
");
        response.extend_from_slice(b"Server: NTRIP Caster/1.0
");
        response.extend_from_slice(b"
");

        // 挂载点列表 (STR格式)
        for mount in mounts {
            response.extend_from_slice(format!(
                "{:40} {:6} {:6} {:15} {:30} {:20} {}
",
                mount.name,
                mount.format,
                mount.freq,
                mount.coords.unwrap_or((0.0, 0.0, 0.0)),
                mount.owner,
                mount.ntrip_version,
                mount.network
            ).as_bytes());
        }

        response.freeze()
    }
}

/// 挂载点信息 (用于STR列表)
#[derive(Debug, Clone)]
pub struct MountPointInfo {
    /// 挂载点名称
    pub name: String,
    /// 数据格式
    pub format: String,
    /// 数据频率
    pub freq: f64,
    /// 坐标 (经度, 纬度, 海拔)
    pub coords: Option<(f64, f64, f64)>,
    /// 所有者
    pub owner: String,
    /// NTRIP版本
    pub ntrip_version: String,
    /// 网络名称
    pub network: String,
}

// Base64编码支持
mod base64 {
    pub use base64::engine::general_purpose;
}