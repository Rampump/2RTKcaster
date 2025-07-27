//! 连接调度器，实现多核心负载均衡与优化

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use super::{UploadConnection, DownloadConnection, RelayConnection};
use num_cpus;

/// 连接类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionType {
    /// 上传连接
    Upload,
    /// 下载连接
    Download,
    /// 中继连接
    Relay,
}

/// 核心负载统计
#[derive(Debug, Clone, Default)]
struct CoreStats {
    /// 核心 ID
    core_id: usize,
    /// 活跃任务数
    active_tasks: usize,
    /// 总 CPU 时间（毫秒）
    cpu_time: u64,
    /// 各类连接的数量
    connection_counts: HashMap<ConnectionType, usize>,
}

/// 连接任务句柄
#[derive(Debug)]
pub struct ConnectionHandle {
    /// 任务 ID
    task_id: String,
    /// 核心 ID
    core_id: usize,
    /// 任务句柄
    handle: JoinHandle<()>,
}

/// 连接调度器，负责在多个核心之间分配连接任务
pub struct ConnectionScheduler {
    /// 核心统计信息
    core_stats: RwLock<Vec<CoreStats>>,
    /// 连接到核心的映射
    connection_map: RwLock<HashMap<String, usize>>, // connection_id -> core_id
    /// 任务队列
    task_queues: Vec<Mutex<VecDeque<ConnectionTask>>>,
    /// 工作线程
    workers: Vec<JoinHandle<()>>,
    /// 是否运行中
    running: Mutex<bool>,
}

/// 连接任务
enum ConnectionTask {
    /// 上传连接任务
    Upload(Arc<UploadConnection>),
    /// 下载连接任务
    Download(Arc<DownloadConnection>),
    /// 中继连接任务
    Relay(Arc<RelayConnection>),
}

impl ConnectionScheduler {
    /// 创建新的连接调度器
    pub fn new() -> Self {
        let num_cores = num_cpus::get();
        let mut task_queues = Vec::with_capacity(num_cores);
        let mut workers = Vec::with_capacity(num_cores);
        
        // 初始化每个核心的任务队列和工作线程
        for core_id in 0..num_cores {
            task_queues.push(Mutex::new(VecDeque::new()));
            
            let queue = task_queues[core_id].clone();
            let core_stats = (0..num_cores).map(|id| CoreStats {
                core_id: id,
                ..CoreStats::default()
            }).collect();
            
            // 启动工作线程
            let worker = tokio::spawn(async move {
                Self::worker_loop(core_id, queue, core_stats).await;
            });
            
            workers.push(worker);
        }
        
        Self {
            core_stats: RwLock::new((0..num_cores).map(|id| CoreStats {
                core_id: id,
                ..CoreStats::default()
            }).collect()),
            connection_map: RwLock::new(HashMap::new()),
            task_queues,
            workers,
            running: Mutex::new(true),
        }
    }

    /// 工作线程循环，处理分配给核心的任务
    async fn worker_loop(
        core_id: usize,
        queue: Mutex<VecDeque<ConnectionTask>>,
        mut core_stats: Vec<CoreStats>,
    ) {
        while let Some(task) = queue.lock().await.pop_front() {
            match task {
                ConnectionTask::Upload(conn) => {
                    // 处理上传连接
                    if let Err(e) = conn.start().await {
                        tracing::error!("Upload connection task failed: {}", e);
                    }
                    
                    // 更新统计信息
                    let mut stats = &mut core_stats[core_id];
                    stats.active_tasks -= 1;
                    stats.connection_counts.entry(ConnectionType::Upload).and_modify(|v| *v -= 1);
                }
                ConnectionTask::Download(conn) => {
                    // 下载连接已经在start中处理，这里只是监控
                    let conn_id = conn.conn_id().to_string();
                    let status = conn.status().await;
                    
                    while status == super::download::DownloadStatus::Active {
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        let new_status = conn.status().await;
                        if new_status != status {
                            break;
                        }
                    }
                    
                    // 更新统计信息
                    let mut stats = &mut core_stats[core_id];
                    stats.active_tasks -= 1;
                    stats.connection_counts.entry(ConnectionType::Download).and_modify(|v| *v -= 1);
                }
                ConnectionTask::Relay(conn) => {
                    // 中继连接监控
                    let status = conn.status().await;
                    
                    while status == super::relay::RelayStatus::Active {
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        let new_status = conn.status().await;
                        if new_status != status {
                            break;
                        }
                    }
                    
                    // 更新统计信息
                    let mut stats = &mut core_stats[core_id];
                    stats.active_tasks -= 1;
                    stats.connection_counts.entry(ConnectionType::Relay).and_modify(|v| *v -= 1);
                }
            }
        }
    }

    /// 选择最佳核心来运行任务
    async fn select_best_core(&self, conn_type: ConnectionType) -> usize {
        let core_stats = self.core_stats.read().await;
        let num_cores = core_stats.len();
        
        // 简单策略：选择该类型连接数量最少的核心
        let mut best_core = 0;
        let mut min_count = usize::MAX;
        
        for (i, stats) in core_stats.iter().enumerate() {
            let count = stats.connection_counts.get(&conn_type).copied().unwrap_or(0);
            if count < min_count || (count == min_count && stats.active_tasks < core_stats[best_core].active_tasks) {
                min_count = count;
                best_core = i;
            }
        }
        
        best_core
    }

    /// 将连接分配到最佳核心
    pub async fn assign_connection(&self, conn_id: String, conn_type: ConnectionType) -> usize {
        let core_id = self.select_best_core(conn_type).await;
        
        // 更新连接映射
        let mut connection_map = self.connection_map.write().await;
        connection_map.insert(conn_id, core_id);
        drop(connection_map);
        
        // 更新核心统计
        let mut core_stats = self.core_stats.write().await;
        core_stats[core_id].active_tasks += 1;
        *core_stats[core_id].connection_counts.entry(conn_type).or_insert(0) += 1;
        drop(core_stats);
        
        core_id
    }

    /// 在最佳核心上启动连接任务
    pub async fn spawn_connection_task(&self, task: ConnectionTask) -> ConnectionHandle {
        // 确定任务类型
        let conn_type = match &task {
            ConnectionTask::Upload(_) => ConnectionType::Upload,
            ConnectionTask::Download(_) => ConnectionType::Download,
            ConnectionTask::Relay(_) => ConnectionType::Relay,
        };
        
        // 获取任务ID
        let task_id = match &task {
            ConnectionTask::Upload(conn) => format!("upload-{}", conn.mount_name()),
            ConnectionTask::Download(conn) => format!("download-{}", conn.conn_id()),
            ConnectionTask::Relay(conn) => format!("relay-{}", conn.mount_name().await),
        };
        
        // 选择最佳核心
        let core_id = self.select_best_core(conn_type).await;
        
        // 将任务添加到核心的任务队列
        let mut queue = self.task_queues[core_id].lock().await;
        queue.push_back(task);
        drop(queue);
        
        // 更新核心统计
        let mut core_stats = self.core_stats.write().await;
        core_stats[core_id].active_tasks += 1;
        *core_stats[core_id].connection_counts.entry(conn_type).or_insert(0) += 1;
        drop(core_stats);
        
        // 创建任务句柄（实际任务由工作线程处理）
        ConnectionHandle {
            task_id,
            core_id,
            handle: tokio::spawn(async {}), // 空任务，实际任务在工作线程中运行
        }
    }

    /// 重新平衡核心负载
    pub async fn rebalance_load(&self) {
        let core_stats = self.core_stats.read().await;
        let num_cores = core_stats.len();
        if num_cores <= 1 {
            return; // 单核心无需平衡
        }
        
        // 计算平均任务数
        let total_tasks: usize = core_stats.iter().map(|s| s.active_tasks).sum();
        let avg_tasks = total_tasks / num_cores;
        let threshold = avg_tasks + (avg_tasks / 4); // 允许25%的偏差
        
        // 找出过载的核心
        let overloaded_cores: Vec<usize> = core_stats.iter()
            .filter(|s| s.active_tasks > threshold)
            .map(|s| s.core_id)
            .collect();
            
        // 找出负载不足的核心
        let underloaded_cores: Vec<usize> = core_stats.iter()
            .filter(|s| s.active_tasks < avg_tasks - (avg_tasks / 4))
            .map(|s| s.core_id)
            .collect();
            
        drop(core_stats);
        
        // 如果没有需要平衡的核心，直接返回
        if overloaded_cores.is_empty() || underloaded_cores.is_empty() {
            return;
        }
        
        tracing::info!("Rebalancing load: overloaded cores {:?}, underloaded cores {:?}",
                      overloaded_cores, underloaded_cores);
        
        // 简单的负载平衡策略：从过载核心迁移任务到负载不足的核心
        let mut connection_map = self.connection_map.write().await;
        let mut core_stats = self.core_stats.write().await;
        
        for &overloaded_core in &overloaded_cores {
            for &underloaded_core in &underloaded_cores {
                // 计算需要迁移的任务数量
                let excess = core_stats[overloaded_core].active_tasks - threshold;
                if excess <= 0 {
                    break;
                }
                
                // 找出可以迁移的任务
                let mut migrated = 0;
                let mut to_migrate = Vec::new();
                
                for (&conn_id, &core_id) in connection_map.iter() {
                    if core_id == overloaded_core {
                        to_migrate.push(conn_id.clone());
                        migrated += 1;
                        if migrated >= excess {
                            break;
                        }
                    }
                }
                
                // 迁移任务
                for conn_id in to_migrate {
                    connection_map.insert(conn_id.clone(), underloaded_core);
                    
                    // 更新统计信息
                    // 这里简化处理，实际实现需要追踪连接类型并调整相应计数
                    core_stats[overloaded_core].active_tasks -= 1;
                    core_stats[underloaded_core].active_tasks += 1;
                }
                
                if migrated >= excess {
                    break;
                }
            }
        }
        
        drop(connection_map);
        drop(core_stats);
        
        tracing::info!("Load rebalancing completed");
    }

    /// 停止调度器
    pub async fn stop(&self) {
        *self.running.lock().await = false;
        
        // 清空任务队列
        for queue in &self.task_queues {
            queue.lock().await.clear();
        }
        
        // 等待工作线程结束
        for worker in &self.workers {
            let _ = worker.await;
        }
    }
}

// 实现Drop以确保资源正确释放
impl Drop for ConnectionScheduler {
    fn drop(&mut self) {
        // 在Drop中不能使用await，所以这里只是记录日志
        tracing::debug!("Connection scheduler dropped");
    }
}
