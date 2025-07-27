//! 存储缓存模块
//! 实现内存缓存与数据库同步机制

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{RwLock, Mutex, mpsc};
use tokio::time::interval;
use tracing::{info, warn, debug};

use crate::common::error::CasterError;
use crate::storage::rqlite::{RqliteClient, MountInfo, UserInfo};
use crate::storage::etcd::EtcdClient;

/// 缓存项结构体
#[derive(Debug, Clone)]
struct CacheItem<T> {
    /// 数据值
    value: T,
    /// 过期时间
    expires_at: Option<Instant>,
    /// 是否被修改
    modified: bool,
    /// 版本号
    version: u64,
}

/// 挂载点缓存
type MountCache = HashMap<String, CacheItem<MountInfo>>;

/// 用户缓存
type UserCache = HashMap<String, CacheItem<UserInfo>>;

/// 缓存配置
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// 默认过期时间(秒)
    pub default_ttl: u64,
    /// 同步间隔(秒)
    pub sync_interval: u64,
    /// 最大缓存大小
    pub max_size: usize,
    /// 是否启用写回
    pub write_back: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            default_ttl: 300,
            sync_interval: 60,
            max_size: 10000,
            write_back: true,
        }
    }
}

/// 存储缓存管理器
#[derive(Debug)]
pub struct StorageCache {
    /// 挂载点缓存
    mount_cache: Arc<RwLock<MountCache>>,
    /// 用户缓存
    user_cache: Arc<RwLock<UserCache>>,
    /// 缓存配置
    config: CacheConfig,
    /// Rqlite客户端
    rqlite_client: Arc<RqliteClient>,
    /// Etcd客户端
    etcd_client: Arc<EtcdClient>,
    /// 同步任务句柄
    sync_task: Option<tokio::task::JoinHandle<()>>,
    /// 停止标志
    stop_flag: Arc<Mutex<bool>>,
    /// 版本计数器
    version_counter: Arc<Mutex<u64>>,
    /// 修改队列
    modify_queue: Arc<Mutex<mpsc::Sender<(String, String)>>>,
}

impl StorageCache {
    /// 创建新的存储缓存管理器
    pub async fn new(
        rqlite_client: Arc<RqliteClient>,
        etcd_client: Arc<EtcdClient>,
        config: CacheConfig
    ) -> Result<Self, CasterError> {
        // 创建修改队列
        let (tx, rx) = mpsc::channel(100);

        let cache = Self {
            mount_cache: Arc::new(RwLock::new(HashMap::new())),
            user_cache: Arc::new(RwLock::new(HashMap::new())),
            config,
            rqlite_client: rqlite_client.clone(),
            etcd_client,
            sync_task: None,
            stop_flag: Arc::new(Mutex::new(false)),
            version_counter: Arc::new(Mutex::new(1)),
            modify_queue: Arc::new(Mutex::new(tx)),
        };

        // 预加载缓存
        cache.preload_cache().await?;

        // 启动同步任务
        cache.start_sync_tasks(rqlite_client, rx).await;

        Ok(cache)
    }

    /// 预加载缓存
    async fn preload_cache(&self) -> Result<(), CasterError> {
        info!("Preloading cache from database");

        // 加载挂载点
        let mounts = self.rqlite_client.list_mounts().await?;
        let mut mount_cache = self.mount_cache.write().await;
        for mount in mounts {
            let ttl = if mount.expires_at > 0 {
                Some(Instant::now() + Duration::from_secs(mount.expires_at as u64))
            } else {
                None
            };

            mount_cache.insert(mount.name.clone(), CacheItem {
                value: mount,
                expires_at: ttl,
                modified: false,
                version: self.next_version().await,
            });
        }

        info!("Preloaded {} mount points into cache", mount_cache.len());
        drop(mount_cache);

        // 加载用户
        let users = self.rqlite_client.list_users().await?;
        let mut user_cache = self.user_cache.write().await;
        for user in users {
            user_cache.insert(user.username.clone(), CacheItem {
                value: user,
                expires_at: Some(Instant::now() + Duration::from_secs(self.config.default_ttl)),
                modified: false,
                version: self.next_version().await,
            });
        }

        info!("Preloaded {} users into cache", user_cache.len());
        Ok(())
    }

    /// 启动同步任务
    async fn start_sync_tasks(&self, rqlite_client: Arc<RqliteClient>, rx: mpsc::Receiver<(String, String)>) {
        // 定期同步任务
        let config = self.config.clone();
        let mount_cache = self.mount_cache.clone();
        let user_cache = self.user_cache.clone();
        let stop_flag = self.stop_flag.clone();

        let sync_task = tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(config.sync_interval));

            loop {
                interval.tick().await;

                // 检查是否需要停止
                let stop = *stop_flag.lock().await;
                if stop {
                    break;
                }

                // 同步修改的挂载点
                let mut mount_cache = mount_cache.write().await;
                let mut modified_mounts = HashSet::new();

                for (name, item) in mount_cache.iter_mut() {
                    if item.modified {
                        modified_mounts.insert((name.clone(), item.value.clone()));
                        item.modified = false;
                    }
                }

                drop(mount_cache);

                if !modified_mounts.is_empty() {
                    info!("Syncing {} modified mounts to database", modified_mounts.len());
                    for (_, mount) in modified_mounts {
                        if let Err(e) = rqlite_client.update_mount(&mount).await {
                            warn!("Failed to sync mount {}: {}", mount.name, e);
                        }
                    }
                }

                // 同步修改的用户
                let mut user_cache = user_cache.write().await;
                let mut modified_users = HashSet::new();

                for (username, item) in user_cache.iter_mut() {
                    if item.modified {
                        modified_users.insert((username.clone(), item.value.clone()));
                        item.modified = false;
                    }
                }

                drop(user_cache);

                if !modified_users.is_empty() {
                    info!("Syncing {} modified users to database", modified_users.len());
                    for (_, user) in modified_users {
                        if let Err(e) = rqlite_client.update_user(&user).await {
                            warn!("Failed to sync user {}: {}", user.username, e);
                        }
                    }
                }
            }

            info!("Cache sync task stopped");
        });

        // 修改通知处理任务
        let etcd_client = self.etcd_client.clone();
        let mount_cache = self.mount_cache.clone();
        let user_cache = self.user_cache.clone();
        let stop_flag_clone = self.stop_flag.clone();

        tokio::spawn(async move {
            while let Some((entity_type, name)) = rx.recv().await {
                // 检查是否需要停止
                let stop = *stop_flag_clone.lock().await;
                if stop {
                    break;
                }

                // 通过etcd广播变更
                match entity_type.as_str() {
                    "mount" => {
                        let mount_cache = mount_cache.read().await;
                        if let Some(mount) = mount_cache.get(&name) {
                            let _ = etcd_client.broadcast_mount_change(&mount.value).await;
                        }
                    },
                    "user" => {
                        let user_cache = user_cache.read().await;
                        if let Some(user) = user_cache.get(&name) {
                            let _ = etcd_client.broadcast_user_change(&user.value).await;
                        }
                    },
                    _ => {}
                }
            }
        });

        // 保存任务句柄
        // self.sync_task = Some(sync_task);
    }

    /// 获取下一个版本号
    async fn next_version(&self) -> u64 {
        let mut version = self.version_counter.lock().await;
        *version += 1;
        *version
    }

    /// 获取挂载点
    pub async fn get_mount(&self, name: &str) -> Result<Option<MountInfo>, CasterError> {
        let mount_cache = self.mount_cache.read().await;

        if let Some(item) = mount_cache.get(name) {
            // 检查是否过期
            if let Some(expires_at) = item.expires_at {
                if expires_at < Instant::now() {
                    drop(mount_cache);
                    self.invalidate_mount(name).await;
                    return Ok(None);
                }
            }

            Ok(Some(item.value.clone()))
        } else {
            Ok(None)
        }
    }

    /// 添加或更新挂载点
    pub async fn set_mount(&self, mount: &MountInfo) -> Result<(), CasterError> {
        let ttl = if mount.expires_at > 0 {
            Some(Instant::now() + Duration::from_secs(mount.expires_at as u64))
        } else {
            None
        };

        let mut mount_cache = self.mount_cache.write().await;
        let version = self.next_version().await;

        let modified = if let Some(existing) = mount_cache.get_mut(&mount.name) {
            if existing.value != *mount {
                existing.value = mount.clone();
                existing.expires_at = ttl;
                existing.modified = true;
                existing.version = version;
                true
            } else {
                false
            }
        } else {
            mount_cache.insert(mount.name.clone(), CacheItem {
                value: mount.clone(),
                expires_at: ttl,
                modified: true,
                version,
            });
            true
        };

        // 如果有修改，发送通知
        if modified {
            let mut tx = self.modify_queue.lock().await;
            let _ = tx.send(("mount".to_string(), mount.name.clone())).await;
        }

        Ok(())
    }

    /// 使挂载点缓存失效
    async fn invalidate_mount(&self, name: &str) -> Result<(), CasterError> {
        let mut mount_cache = self.mount_cache.write().await;
        if mount_cache.remove(name).is_some() {
            info!("Mount point {} removed from cache (expired)", name);
        }
        Ok(())
    }

    /// 获取用户
    pub async fn get_user(&self, username: &str) -> Result<Option<UserInfo>, CasterError> {
        let user_cache = self.user_cache.read().await;

        if let Some(item) = user_cache.get(username) {
            // 检查是否过期
            if let Some(expires_at) = item.expires_at {
                if expires_at < Instant::now() {
                    drop(user_cache);
                    self.invalidate_user(username).await;
                    return Ok(None);
                }
            }

            Ok(Some(item.value.clone()))
        } else {
            Ok(None)
        }
    }

    /// 添加或更新用户
    pub async fn set_user(&self, user: &UserInfo) -> Result<(), CasterError> {
        let mut user_cache = self.user_cache.write().await;
        let version = self.next_version().await;

        let modified = if let Some(existing) = user_cache.get_mut(&user.username) {
            if existing.value != *user {
                existing.value = user.clone();
                existing.expires_at = Some(Instant::now() + Duration::from_secs(self.config.default_ttl));
                existing.modified = true;
                existing.version = version;
                true
            } else {
                false
            }
        } else {
            user_cache.insert(user.username.clone(), CacheItem {
                value: user.clone(),
                expires_at: Some(Instant::now() + Duration::from_secs(self.config.default_ttl)),
                modified: true,
                version,
            });
            true
        };

        // 如果有修改，发送通知
        if modified {
            let mut tx = self.modify_queue.lock().await;
            let _ = tx.send(("user".to_string(), user.username.clone())).await;
        }

        Ok(())
    }

    /// 使用户缓存失效
    async fn invalidate_user(&self, username: &str) -> Result<(), CasterError> {
        let mut user_cache = self.user_cache.write().await;
        if user_cache.remove(username).is_some() {
            info!("User {} removed from cache (expired)", username);
        }
        Ok(())
    }

    /// 清理过期缓存项
    pub async fn cleanup_expired(&self) -> Result<(), CasterError> {
        let now = Instant::now();
        let mut mount_count = 0;
        let mut user_count = 0;

        // 清理挂载点
        let mut mount_cache = self.mount_cache.write().await;
        mount_cache.retain(|_, item| {
            let expired = item.expires_at.as_ref().map_or(false, |t| *t < now);
            if expired {
                mount_count += 1;
            }
            !expired
        });

        // 清理用户
        let mut user_cache = self.user_cache.write().await;
        user_cache.retain(|_, item| {
            let expired = item.expires_at.as_ref().map_or(false, |t| *t < now);
            if expired {
                user_count += 1;
            }
            !expired
        });

        if mount_count > 0 || user_count > 0 {
            debug!("Cleaned up {} expired mount points and {} users from cache", mount_count, user_count);
        }

        Ok(())
    }

    /// 停止缓存管理器
    pub async fn stop(&self) -> Result<(), CasterError> {
        info!("Stopping cache manager and syncing changes");

        // 设置停止标志
        let mut stop_flag = self.stop_flag.lock().await;
        *stop_flag = true;
        drop(stop_flag);

        // 等待同步任务完成
        if let Some(task) = self.sync_task.take() {
            if let Err(e) = task.await {
                warn!("Cache sync task panicked: {:?}", e);
            }
        }

        Ok(())
    }
}