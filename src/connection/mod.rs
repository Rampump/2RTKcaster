//! 连接管理与数据转发核心模块
//! 基于 tokio + io-uring + bytes 实现高性能网络处理

pub mod manager;
pub mod upload;
pub mod download;
pub mod relay;
pub mod buffer;
pub mod scheduler;

// 重新导出常用类型
pub use manager::{ConnectionManager, ConnectionStats};
pub use upload::UploadConnection;
pub use download::DownloadConnection;
pub use relay::RelayConnection;
pub use buffer::{SharedBuffer, BatchSender};
pub use scheduler::ConnectionScheduler;
