//! 中继管理器模块
//! 管理跨节点数据中继和负载均衡

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock, Mutex};
use tokio::time::interval;
use tracing::{info, warn, error};
use uuid::Uuid;

use crate::common::error::CasterError;
use crate::config::NodeConfig;
use crate::connection::{RelayConnection, RelayConfig, ConnectionManager};
use crate::storage::etcd::EtcdClient;
use crate::storage::rqlite::RqliteClient;

/// 中继节点信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayNode {
    /// 节点ID
    pub node_id: String,
    /// 节点IP地址
    pub node_ip: String,
    /// 节点端口
    pub port: u16,
    /// 负载状态 (0-100)
    pub load: u8,
    /// 是否在线
    pub online: bool,
    /// 最后更新时间
    pub last_updated: u64,
}

/// 中继管理器
#[derive(Debug)]
pub struct RelayManager {
    /// 配置
    config: NodeConfig,
    /// 节点列表
    nodes: Arc<RwLock<HashMap<String, RelayNode>>>,
    /// 活跃的中继连接
    relay_connections: Arc<RwLock<HashMap<String, Arc<RelayConnection>>>>,
    /// 连接管理器
    conn_manager: Arc<ConnectionManager>,
    /// Etcd客户端
    etcd_client: Arc<EtcdClient>,
    /// Rqlite客户端
    rqlite_client: Arc<RqliteClient>,
    /// 控制通道
    control_tx: Mutex<Option<mpsc::Sender<RelayControlCommand>>>,
    /// 运行状态
    running: Arc<Mutex<bool>>,
}

/// 中继控制命令
#[derive(Debug)]
enum RelayControlCommand {
    /// 添加中继节点
    AddNode(RelayNode),
    /// 移除中继节点
    RemoveNode(String),
    /// 更新节点负载
    UpdateNodeLoad(String, u8),
    /// 停止所有中继
    StopAll,
}

impl RelayManager {
    /// 创建新的中继管理器
    pub async fn new(
        config: NodeConfig,
        conn_manager: Arc<ConnectionManager>,
        etcd_client: Arc<EtcdClient>,
        rqlite_client: Arc<RqliteClient>
    ) -> Result<Self, CasterError> {
        let mut manager = Self {
            config: config.clone(),
            nodes: Arc::new(RwLock::new(HashMap::new())),
            relay_connections: Arc::new(RwLock::new(HashMap::new())),
            conn_manager,
            etcd_client: etcd_client.clone(),
            rqlite_client,
            control_tx: Mutex::new(None),
            running: Arc::new(Mutex::new(true)),
        };

        // 从Etcd加载现有节点
        manager.load_nodes_from_etcd().await?;

        // 设置控制通道
        let (tx, rx) = mpsc::channel(100);
        *manager.control_tx.lock().await = Some(tx);

        // 启动管理器任务
        manager.start_background_tasks(rx).await;

        Ok(manager)
    }

    /// 从Etcd加载节点
    async fn load_nodes_from_etcd(&mut self) -> Result<(), CasterError> {
        let nodes = self.etcd_client.list_relay_nodes().await?;
        let mut node_map = HashMap::new();

        for node in nodes {
            node_map.insert(node.node_id.clone(), node);
        }

        *self.nodes.write().await = node_map;
        info!("Loaded {} relay nodes from etcd", self.nodes.read().await.len());
        Ok(())
    }

    /// 启动后台任务
    async fn start_background_tasks(&self, mut rx: mpsc::Receiver<RelayControlCommand>) {
        let nodes = self.nodes.clone();
        let relay_connections = self.relay_connections.clone();
        let config = self.config.clone();
        let conn_manager = self.conn_manager.clone();
        let etcd_client = self.etcd_client.clone();
        let running = self.running.clone();

        // 控制命令处理任务
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    RelayControlCommand::AddNode(node) => {
                        let mut nodes_map = nodes.write().await;
                        nodes_map.insert(node.node_id.clone(), node.clone());
                        drop(nodes_map);

                        // 创建中继连接
                        Self::create_relay_connection(
                            &node,
                            &config,
                            &conn_manager,
                            &relay_connections,
                            &etcd_client
                        ).await;
                    },
                    RelayControlCommand::RemoveNode(node_id) => {
                        let mut nodes_map = nodes.write().await;
                        nodes_map.remove(&node_id);
                        drop(nodes_map);

                        // 关闭中继连接
                        let mut connections = relay_connections.write().await;
                        if let Some(conn) = connections.remove(&node_id) {
                            let _ = conn.stop().await;
                        }
                    },
                    RelayControlCommand::UpdateNodeLoad(node_id, load) => {
                        let mut nodes_map = nodes.write().await;
                        if let Some(node) = nodes_map.get_mut(&node_id) {
                            node.load = load;
                            node.last_updated = chrono::Utc::now().timestamp_millis() as u64;
                        }
                    },
                    RelayControlCommand::StopAll => {
                        let mut connections = relay_connections.write().await;
                        for (_, conn) in connections.drain() {
                            let _ = conn.stop().await;
                        }
                        break;
                    }
                }
            }
        });

        // 节点健康检查任务
        let nodes_clone = self.nodes.clone();
        let relay_connections_clone = self.relay_connections.clone();
        let config_clone = self.config.clone();
        let conn_manager_clone = self.conn_manager.clone();
        let etcd_client_clone = self.etcd_client.clone();
        let running_clone = self.running.clone();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(30));
            while *running_clone.lock().await {
                interval.tick().await;
                Self::check_node_health(
                    &nodes_clone,
                    &relay_connections_clone,
                    &config_clone,
                    &conn_manager_clone,
                    &etcd_client_clone
                ).await;
            }
        });

        // 负载均衡任务
        let nodes_clone2 = self.nodes.clone();
        let running_clone2 = self.running.clone();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(10));
            while *running_clone2.lock().await {
                interval.tick().await;
                Self::balance_load(&nodes_clone2, &etcd_client_clone).await;
            }
        });
    }

    /// 创建中继连接
    async fn create_relay_connection(
        node: &RelayNode,
        config: &NodeConfig,
        conn_manager: &Arc<ConnectionManager>,
        relay_connections: &Arc<RwLock<HashMap<String, Arc<RelayConnection>>>>,
        etcd_client: &Arc<EtcdClient>
    ) {
        info!("Creating relay connection to node {} at {}:{}", node.node_id, node.node_ip, node.port);

        let relay_config = RelayConfig {
            id: Uuid::new_v4().to_string(),
            remote_node_id: node.node_id.clone(),
            remote_addr: format!("{}:{}", node.node_ip, node.port),
            buffer_size: config.connection.relay_buffer_size,
            reconnect_interval: config.connection.reconnect_interval,
            max_reconnects: config.connection.max_reconnects,
        };

        match RelayConnection::new(relay_config, conn_manager.clone(), etcd_client.clone()).await {
            Ok(conn) => {
                let conn_arc = Arc::new(conn);
                if let Err(e) = conn_arc.start().await {
                    error!("Failed to start relay connection to node {}: {}", node.node_id, e);
                    return;
                }

                let mut connections = relay_connections.write().await;
                connections.insert(node.node_id.clone(), conn_arc);
                info!("Successfully established relay connection to node {}", node.node_id);
            },
            Err(e) => {
                error!("Failed to create relay connection to node {}: {}", node.node_id, e);
            }
        }
    }

    /// 检查节点健康状态
    async fn check_node_health(
        nodes: &Arc<RwLock<HashMap<String, RelayNode>>>,
        relay_connections: &Arc<RwLock<HashMap<String, Arc<RelayConnection>>>>,
        config: &NodeConfig,
        conn_manager: &Arc<ConnectionManager>,
        etcd_client: &Arc<EtcdClient>
    ) {
        let nodes_map = nodes.read().await;
        let current_time = chrono::Utc::now().timestamp_millis() as u64;

        for (node_id, node) in nodes_map.iter() {
            // 检查节点是否超时
            if current_time - node.last_updated > 60_000 { // 60秒超时
                warn!("Relay node {} is offline (last updated: {}s ago)", node_id, (current_time - node.last_updated)/1000);

                // 关闭现有连接
                let mut connections = relay_connections.write().await;
                if let Some(conn) = connections.remove(node_id) {
                    let _ = conn.stop().await;
                }
                drop(connections);

                // 尝试重新连接
                let node_clone = node.clone();
                tokio::spawn(async move {
                    Self::create_relay_connection(
                        &node_clone,
                        config,
                        conn_manager,
                        relay_connections,
                        etcd_client
                    ).await;
                });
            } else if !node.online {
                // 节点标记为离线，但未超时，尝试重新连接
                let node_clone = node.clone();
                tokio::spawn(async move {
                    Self::create_relay_connection(
                        &node_clone,
                        config,
                        conn_manager,
                        relay_connections,
                        etcd_client
                    ).await;
                });
            }
        }
    }

    /// 负载均衡
    async fn balance_load(nodes: &Arc<RwLock<HashMap<String, RelayNode>>>, etcd_client: &Arc<EtcdClient>) {
        let nodes_map = nodes.read().await;
        let mut online_nodes: Vec<&RelayNode> = nodes_map.values()
            .filter(|n| n.online)
            .collect();

        if online_nodes.is_empty() {
            return;
        }

        // 按负载排序
        online_nodes.sort_by_key(|n| n.load);

        // 找出过载和低负载节点
        let high_load_threshold = 70;
        let low_load_threshold = 30;

        let high_load_nodes: Vec<&RelayNode> = online_nodes.iter()
            .filter(|n| n.load >= high_load_threshold)
            .cloned()
            .collect();

        let low_load_nodes: Vec<&RelayNode> = online_nodes.iter()
            .filter(|n| n.load <= low_load_threshold)
            .cloned()
            .collect();

        if !high_load_nodes.is_empty() && !low_load_nodes.is_empty() {
            info!("Load balancing needed: {} high load nodes, {} low load nodes", high_load_nodes.len(), low_load_nodes.len());

            // 这里实现负载均衡逻辑
            // 实际应用中需要根据具体的负载指标和业务规则进行负载迁移
            // 简化实现：仅记录日志
        }
    }

    /// 添加中继节点
    pub async fn add_node(&self, node: RelayNode) -> Result<(), CasterError> {
        let mut control_tx = self.control_tx.lock().await;
        if let Some(tx) = control_tx.as_mut() {
            tx.send(RelayControlCommand::AddNode(node.clone())).await
                .map_err(|e| CasterError::RelayError(format!("Failed to send add node command: {}", e)))?;

            // 保存到Etcd
            self.etcd_client.add_relay_node(&node).await?;
            Ok(())
        } else {
            Err(CasterError::RelayError("Relay manager not initialized".to_string()))
        }
    }

    /// 移除中继节点
    pub async fn remove_node(&self, node_id: &str) -> Result<(), CasterError> {
        let mut control_tx = self.control_tx.lock().await;
        if let Some(tx) = control_tx.as_mut() {
            tx.send(RelayControlCommand::RemoveNode(node_id.to_string())).await
                .map_err(|e| CasterError::RelayError(format!("Failed to send remove node command: {}", e)))?;

            // 从Etcd删除
            self.etcd_client.remove_relay_node(node_id).await?;
            Ok(())
        } else {
            Err(CasterError::RelayError("Relay manager not initialized".to_string()))
        }
    }

    /// 更新节点负载
    pub async fn update_node_load(&self, node_id: &str, load: u8) -> Result<(), CasterError> {
        let mut control_tx = self.control_tx.lock().await;
        if let Some(tx) = control_tx.as_mut() {
            tx.send(RelayControlCommand::UpdateNodeLoad(node_id.to_string(), load)).await
                .map_err(|e| CasterError::RelayError(format!("Failed to send update load command: {}", e)))?;

            // 更新Etcd
            self.etcd_client.update_relay_node_load(node_id, load).await?;
            Ok(())
        } else {
            Err(CasterError::RelayError("Relay manager not initialized".to_string()))
        }
    }

    /// 停止所有中继
    pub async fn stop_all(&self) -> Result<(), CasterError> {
        let mut control_tx = self.control_tx.lock().await;
        if let Some(tx) = control_tx.as_mut() {
            tx.send(RelayControlCommand::StopAll).await
                .map_err(|e| CasterError::RelayError(format!("Failed to send stop all command: {}", e)))?;

            *self.running.lock().await = false;
            Ok(())
        } else {
            Err(CasterError::RelayError("Relay manager not initialized".to_string()))
        }
    }

    /// 获取所有节点
    pub async fn get_all_nodes(&self) -> Vec<RelayNode> {
        let nodes_map = self.nodes.read().await;
        nodes_map.values().cloned().collect()
    }
}

// 序列化支持
use serde::{Serialize, Deserialize};
use chrono;