//! RTCM 数据解析模块
//! 解析RTK数据流中的关键信息（格式、频率、坐标等）

use std::fmt;
use bytes::Bytes;
use super::error::ParseError;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::Mutex;

/// RTCM 解析结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RtcmInfo {
    /// 数据格式（如 RTCM3.3）
    pub format: String,
    /// 数据频率（Hz）
    pub freq: f64,
    /// 坐标（经度，纬度，海拔）
    pub coords: Option<(f64, f64, f64)>,
    /// 最后更新时间
    pub last_updated: String,
}

/// RTCM 解析器
#[derive(Debug, Clone)]
pub struct RtcmParser {
    /// 状态跟踪
    state: Arc<Mutex<ParserState>>,
}

/// 解析器状态
#[derive(Debug, Clone, Default)]
struct ParserState {
    /// 格式版本
    format: Option<String>,
    /// 消息计数器
    msg_count: u32,
    /// 开始时间
    start_time: Option<u64>,
    /// 最后坐标
    last_coords: Option<(f64, f64, f64)>,
    /// 消息类型计数
    msg_type_counts: std::collections::HashMap<u16, u32>,
}

impl RtcmParser {
    /// 创建新的 RTCM 解析器
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ParserState::default())),
        }
    }

    /// 解析 RTCM 数据
    pub fn parse(&self, data: &[u8]) -> Result<RtcmInfo, ParseError> {
        let mut state = self.state.try_lock()
            .map_err(|_| ParseError::RtcmDataError("解析器忙".to_string()))?;

        // 处理新数据
        self.process_data(data, &mut state)?;

        // 计算频率
        let freq = self.calculate_frequency(&state)?;

        // 创建结果
        let info = RtcmInfo {
            format: state.format.clone().unwrap_or_else(|| "unknown".to_string()),
            freq,
            coords: state.last_coords.clone(),
            last_updated: Utc::now().to_rfc3339(),
        };

        Ok(info)
    }

    /// 处理 RTCM 数据
    fn process_data(&self, data: &[u8], state: &mut ParserState) -> Result<(), ParseError> {
        let mut bytes = data.iter().peekable();
        state.msg_count += 1;

        // 如果是第一次处理数据，记录开始时间
        if state.start_time.is_none() {
            state.start_time = Some(Utc::now().timestamp_millis() as u64);
        }

        // 解析 RTCM 消息头
        while bytes.peek().is_some() {
            // 寻找 RTCM 消息头 (0xD3)
            match bytes.position(|&b| b == 0xD3) {
                Some(pos) => {
                    // 跳过前面的非消息数据
                    for _ in 0..pos {
                        bytes.next();
                    }
                },
                None => break,
            }

            // 检查是否有足够的字节解析头
            if bytes.len() < 3 {
                break;
            }

            // 解析消息头
            let _preamble = bytes.next().unwrap(); // 0xD3
            let length = ((bytes.next().unwrap() as u16) << 4) | ((bytes.next().unwrap() as u16) & 0x0F);

            // 检查是否有足够的字节解析消息
            if bytes.len() < length as usize + 3 { // +3 是 CRC
                break;
            }

            // 解析消息类型
            let msg_type_bytes: [u8; 2] = [*bytes.next().unwrap(), *bytes.next().unwrap()];
            let msg_type = ((msg_type_bytes[0] as u16) << 4) | ((msg_type_bytes[1] as u16) >> 4);

            // 更新消息类型计数
            *state.msg_type_counts.entry(msg_type).or_insert(0) += 1;

            // 根据消息类型解析特定内容
            match msg_type {
                1005 | 1006 | 1007 | 1008 => {
                    // 基准站坐标消息
                    self.parse_base_station_coords(&mut bytes, state)?;
                    state.format = Some("RTCM3.3".to_string());
                },
                1074..=1084 | 1087..=1092 | 1127 => {
                    // GPS/GLONASS/北斗观测值消息
                    state.format = Some("RTCM3.3".to_string());
                },
                4072..=4073 => {
                    // MSM7 消息
                    state.format = Some("RTCM3.3".to_string());
                },
                _ => {
                    // 未知消息类型，跳过
                }
            }

            // 跳过剩余的消息字节和CRC
            for _ in 0..(length as usize - 2 + 3) { // -2 是因为已经读取了2字节消息类型
                bytes.next();
            }
        }

        Ok(())
    }

    /// 解析基准站坐标消息
    fn parse_base_station_coords(&self, bytes: &mut std::iter::Peekable<std::slice::Iter<u8>>, state: &mut ParserState) -> Result<(), ParseError> {
        // 解析基准站坐标 (简化版)
        // 实际解析需要根据RTCM规范处理32位整数和缩放因子
        if bytes.len() < 12 { // 至少需要12字节来解析坐标
            return Ok(());
        }

        // 这里只是模拟解析，实际实现需要根据RTCM规范
        let _station_id = ((bytes.next().unwrap() as u16) << 4) | ((bytes.next().unwrap() as u16) >> 4);
        let _gps_epoch = bytes.next().unwrap();
        let _sv_count = bytes.next().unwrap();

        // 模拟坐标解析
        let lat_bytes: [u8; 4] = [*bytes.next().unwrap(), *bytes.next().unwrap(), *bytes.next().unwrap(), *bytes.next().unwrap()];
        let lon_bytes: [u8; 4] = [*bytes.next().unwrap(), *bytes.next().unwrap(), *bytes.next().unwrap(), *bytes.next().unwrap()];
        let height_bytes: [u8; 4] = [*bytes.next().unwrap(), *bytes.next().unwrap(), *bytes.next().unwrap(), *bytes.next().unwrap()];

        // 转换为经纬度（实际需要根据RTCM规范的缩放因子处理）
        let lat = i32::from_be_bytes(lat_bytes) as f64 / 1e7;
        let lon = i32::from_be_bytes(lon_bytes) as f64 / 1e7;
        let height = i32::from_be_bytes(height_bytes) as f64 / 1e2;

        state.last_coords = Some((lat, lon, height));
        Ok(())
    }

    /// 计算数据频率
    fn calculate_frequency(&self, state: &ParserState) -> Result<f64, ParseError> {
        let start_time = state.start_time
            .ok_or_else(|| ParseError::RtcmDataError("无开始时间".to_string()))?;

        let current_time = Utc::now().timestamp_millis() as u64;
        let elapsed = (current_time - start_time) as f64 / 1000.0;

        if elapsed < 1.0 { // 至少需要1秒才能计算频率
            Ok(0.0)
        } else {
            Ok(state.msg_count as f64 / elapsed)
        }
    }
}

/// RTCM 格式
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtcmFormat {
    /// RTCM 2.x
    V2,
    /// RTCM 3.x
    V3,
    /// 未知格式
    Unknown,
}

impl fmt::Display for RtcmFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RtcmFormat::V2 => write!(f, "RTCM2.x"),
            RtcmFormat::V3 => write!(f, "RTCM3.x"),
            RtcmFormat::Unknown => write!(f, "Unknown"),
        }
    }
}

// 序列化支持
use serde::{Serialize, Deserialize};