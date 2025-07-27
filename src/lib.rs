//! NTRIP Caster 核心库
//! 高性能RTK数据转发服务，支持分布式部署与多节点协作

#![feature(async_closure)]
#![feature(trait_alias)]
#![allow(clippy::module_inception)]

// 公共依赖
pub use bytes::{Bytes, BytesMut};
pub use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
pub use std::sync::Arc;
use std::net::SocketAddr;

// 导出核心类型
pub mod ntrip_caster {
    // 配置模块
    pub mod config {
        include!("config.rs");
    }

    // 服务器核心
    pub mod server {
        include!("server.rs");
    }

    // 存储层
    pub mod storage {
        pub mod etcd {
            include!("storage/etcd.rs");
        }
        pub mod rqlite {
            include!("storage/rqlite.rs");
        }
    }

    // 连接管理
    pub mod connection {
        include!("connection/mod.rs");
    }

    // 公共组件
    pub mod common {
        pub mod ntrip {
            include!("common/ntrip.rs");
        }
        pub mod rtcm {
            include!("common/rtcm.rs");
        }
        pub mod error {
            include!("common/error.rs");
        }
    }

    // 跨节点中继
    pub mod relay {
        include!("relay.rs");
    }
}

// 全局常量
pub const NODE_ID: &str = env!("NODE_ID");
pub const NODE_IP: &str = env!("NODE_IP");

// 初始化函数
pub async fn init() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    Ok(())
}

// 启动服务器
pub async fn start_server(config_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config = ntrip_caster::config::load(config_path)?;
    let mut server = ntrip_caster::server::NtripServer::new(config).await?;
    server.run().await
}