//! Etcd 分布式存储交互模块
//! 实现挂载点在线状态管理与节点协同

use std::sync::Arc;
use etcd_client::{Client, Error as EtcdError, GetOptions, PutOptions, DeleteOptions};
use serde::{Serialize, Deserialize};
use chrono::{Utc, DateTime};
use super::super::common::error::{CasterError, StorageError};
use super::super::Bytes;

/// Etcd 客户端包装器
#[derive(Clone)]
pub struct EtcdClient {
    client: Arc<Client>,
    prefix: String,
}

/// 挂载点在线信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MountNodeInfo {
    /// 节点标识
    pub node_id: String,
    /// 节点 IP
    pub node_ip: String,
    /// 状态 (running/error)
    pub status: String,
    /// 启动时间 (RFC3339)
    pub start_time: String,
    /// RTCM 格式
    pub format: Option<String>,
    /// 数据频率
    pub freq: Option<f64>,
    /// 经纬度海拔
    pub coords: Option<(f64, f64, f64)>,
}

/// 用户连接信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserConnectionInfo {
    /// 订阅的挂载点
    pub mount_name: String,
    /// 处理连接的节点
    pub node_id: String,
    /// 连接建立时间
    pub start_time: String,
    /// 客户端 IP
    pub client_ip: String,
}

impl EtcdClient {
    /// 创建新的 Etcd 客户端
    pub async fn new(endpoints: &[&str], prefix: &str) -> Result<Self, CasterError> {
        let client = Client::connect(endpoints, None)
            .await
            .map_err(|e| StorageError::EtcdError(format!("连接失败: {}", e)))?;

        Ok(Self {
            client: Arc::new(client),
            prefix: prefix.to_string(),
        })
    }

    /// 获取挂载点的在线节点信息
    pub async fn get_mount_node(&self, mount_name: &str) -> Result<Option<MountNodeInfo>, CasterError> {
        let key = format!("{}{}/node", self.prefix, mount_name);

        let resp = self.client.get(key, None)
            .await
            .map_err(|e| StorageError::EtcdError(format!("获取失败: {}", e)))?;

        if let Some(kv) = resp.kvs().first() {
            let info: MountNodeInfo = serde_json::from_slice(kv.value())
                .map_err(|e| StorageError::SerializationError(e))?;
            Ok(Some(info))
        } else {
            Ok(None)
        }
    }

    /// 将挂载点标记为在线
    pub async fn set_mount_online(&self, mount_name: &str, info: &MountNodeInfo) -> Result<(), CasterError> {
        let key = format!("{}{}/node", self.prefix, mount_name);
        let value = serde_json::to_vec(info)
            .map_err(|e| StorageError::SerializationError(e))?;

        // 设置键值对，TTL 设为 30 秒，需要定期心跳更新
        self.client.put(key, value, Some(PutOptions::new().with_lease(30)))
            .await
            .map_err(|e| StorageError::EtcdError(format!("设置失败: {}", e)))?;

        Ok(())
    }

    /// 将挂载点标记为离线
    pub async fn delete_mount_online(&self, mount_name: &str) -> Result<(), CasterError> {
        let key = format!("{}{}/node", self.prefix, mount_name);

        self.client.delete(key, None)
            .await
            .map_err(|e| StorageError::EtcdError(format!("删除失败: {}", e)))?;

        Ok(())
    }

    /// 列出所有在线挂载点
    pub async fn list_online_mounts(&self) -> Result<Vec<(String, MountNodeInfo)>, CasterError> {
        let key_prefix = format!("{}*", self.prefix);

        let resp = self.client.get(key_prefix, Some(GetOptions::new().with_prefix()))
            .await
            .map_err(|e| StorageError::EtcdError(format!("列出失败: {}", e)))?;

        let mut mounts = Vec::new();

        for kv in resp.kvs() {
            let key = String::from_utf8_lossy(kv.key());
            // 提取挂载点名称 (假设 key 格式为