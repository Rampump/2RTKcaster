// 项目根模块
ntrip_caster:: {
    // === 核心配置与启动 ===
    pub mod config {
        // 节点配置（IP、端口、etcd/rqlite 连接信息等）
        pub struct NodeConfig;
        // 加载配置文件（toml/yaml）
        pub fn load() -> Result<NodeConfig, ConfigError>;
    }

    // 系统启动入口（初始化组件、监听端口）
    pub mod server {
        pub struct NtripServer {
            config: config::NodeConfig,
            etcd_client: storage::etcd::EtcdClient,
            rqlite_client: storage::rqlite::RqliteClient,
            // 本地挂载点管理（上传端连接池）
            mount_manager: MountManager,
            // 本地下载端连接池
            client_manager: ClientManager,
        }
        
        impl NtripServer {
            // 初始化服务器
            pub fn new(config: config::NodeConfig) -> Result<Self, ServerError>;
            // 启动 TCP 监听（同时处理上传/下载连接）
            pub async fn run(&mut self) -> Result<(), ServerError>;
        }
    }


    // === 存储层（rqlite + etcd 交互） ===
    pub mod storage {
        // --- rqlite 交互（持久化配置：挂载点、用户信息）---
        pub mod rqlite {
            pub struct RqliteClient {
                // 连接池或客户端句柄
            }
            
            impl RqliteClient {
                // 初始化连接
                pub fn connect(addr: &str) -> Result<Self, RqliteError>;
                
                // 挂载点相关查询
                pub async fn get_mount(&self, mount_name: &str) -> Result<Option<MountInfo>, RqliteError>;
                // 检查挂载点是否存在
                pub async fn mount_exists(&self, mount_name: &str) -> Result<bool, RqliteError>;
                
                // 用户相关查询
                pub async fn get_user(&self, username: &str) -> Result<Option<UserInfo>, RqliteError>;
                // 验证用户密码（哈希比对）
                pub async fn verify_user_password(&self, username: &str, password: &str) -> Result<bool, RqliteError>;
            }
            
            // 挂载点持久化信息（rqlite 存储）
            #[derive(Debug, Serialize, Deserialize)]
            pub struct MountInfo {
                pub name: String,          // 挂载点名称
                pub upload_password: String, // 上传密码（哈希）
                pub owner: String,         // 所属用户
                pub is_public: bool,       // 是否允许匿名下载
                // 其他静态配置...
            }
            
            // 用户信息（rqlite 存储）
            #[derive(Debug, Serialize, Deserialize)]
            pub struct UserInfo {
                pub username: String,
                pub password_hash: String,
                pub max_connections: u32, // 最大连接数限制
                pub allowed_mounts: Vec<String>, // 可访问的挂载点
            }
        }

        // --- etcd 交互（分布式状态：在线挂载点、连接状态）---
        pub mod etcd {
            pub struct EtcdClient {
                client: etcd_client::Client, // 基于官方 etcd 客户端
            }
            
            impl EtcdClient {
                // 初始化连接
                pub async fn connect(endpoints: &[&str]) -> Result<Self, EtcdError>;
                
                // 挂载点在线状态管理
                pub async fn get_mount_node(&self, mount_name: &str) -> Result<Option<MountNodeInfo>, EtcdError>;
                pub async fn set_mount_online(&self, mount_name: &str, node_info: &MountNodeInfo) -> Result<(), EtcdError>;
                pub async fn delete_mount_online(&self, mount_name: &str) -> Result<(), EtcdError>;
                pub async fn list_online_mounts(&self) -> Result<Vec<(String, MountNodeInfo)>, EtcdError>;
                
                // 用户连接状态管理
                pub async fn add_user_connection(&self, user: &str, conn: &UserConnectionInfo) -> Result<(), EtcdError>;
                pub async fn remove_user_connection(&self, user: &str, conn_id: &str) -> Result<(), EtcdError>;
                pub async fn count_user_connections(&self, user: &str) -> Result<u32, EtcdError>;
            }
            
            // etcd 中存储的挂载点在线信息
            #[derive(Debug, Serialize, Deserialize)]
            pub struct MountNodeInfo {
                pub node_id: String,      // 节点标识（如 node-a）
                pub node_ip: String,      // 节点 IP
                pub status: String,       // running / error
                pub start_time: String,   // 启动时间（RFC3339）
                pub format: Option<String>, // RTCM 格式（旁路解析后更新）
                pub freq: Option<f64>,    // 数据频率
                pub coords: Option<(f64, f64, f64)>, // 经纬度海拔
            }
            
            // etcd 中存储的用户连接信息
            #[derive(Debug, Serialize, Deserialize)]
            pub struct UserConnectionInfo {
                pub mount_name: String,   // 订阅的挂载点
                pub node_id: String,      // 处理连接的节点
                pub start_time: String,   // 连接建立时间
                pub client_ip: String,    // 客户端 IP
            }
        }
    }


    // === 上传端（基准站）处理模块 ===
    pub mod upload {
        use super::*;
        
        // 上传连接管理器（单节点内的挂载点连接池）
        pub struct MountManager {
            mounts: HashMap<String, MountConnection>, // 挂载点名称 → 连接实例
            etcd: storage::etcd::EtcdClient,
            rqlite: storage::rqlite::RqliteClient,
            node_id: String, // 当前节点标识
        }
        
        impl MountManager {
            pub fn new(etcd: storage::etcd::EtcdClient, rqlite: storage::rqlite::RqliteClient, node_id: String) -> Self;
            
            // 处理新的上传连接
            pub async fn handle_connection(&mut self, stream: TcpStream) -> Result<(), UploadError>;
            
            // 检查挂载点是否已在本节点运行
            fn is_local_mount(&self, mount_name: &str) -> bool;
            
            // 广播数据到本地下载端 + 其他节点 relay
            fn broadcast_data(&self, mount_name: &str, data: &[u8]);
        }
        
        // 单个挂载点的上传连接实例
        struct MountConnection {
            stream: TcpStream,         // TCP 长连接
            mount_name: String,        // 挂载点名称
            protocol: NtripProtocol,   // 1.0 / 2.0
            parser: Option<RtcmParser>, // 旁路线程解析器
            last_active: Instant,      // 最后活动时间
        }
        
        impl MountConnection {
            // 读取 RTCM 数据
            async fn read_data(&mut self) -> Result<Option<Vec<u8>>, IoError>;
            
            // 启动旁路线程解析数据并更新 etcd
            fn start_parser(&mut self, etcd: &storage::etcd::EtcdClient);
            
            // 关闭连接并清理
            async fn close(&mut self, etcd: &storage::etcd::EtcdClient) -> Result<(), UploadError>;
        }
        
        // NTRIP 协议版本
        #[derive(Debug, Clone, Copy)]
        enum NtripProtocol {
            V1,
            V2,
        }
    }


    // === 下载端（用户）处理模块 ===
    pub mod download {
        use super::*;
        
        // 下载连接管理器（单节点内的用户连接池）
        pub struct ClientManager {
            connections: HashMap<String, ClientConnection>, // 连接 ID → 连接实例
            etcd: storage::etcd::EtcdClient,
            rqlite: storage::rqlite::RqliteClient,
            node_id: String,
        }
        
        impl ClientManager {
            pub fn new(etcd: storage::etcd::EtcdClient, rqlite: storage::rqlite::RqliteClient, node_id: String) -> Self;
            
            // 处理新的下载连接
            pub async fn handle_connection(&mut self, stream: TcpStream) -> Result<(), DownloadError>;
            
            // 为挂载点添加本地订阅者
            fn add_subscriber(&mut self, mount_name: &str, conn: ClientConnection);
            
            // 向本地订阅者推送数据
            fn push_data(&self, mount_name: &str, data: &[u8]);
        }
        
        // 单个用户的下载连接实例
        struct ClientConnection {
            stream: TcpStream,         // TCP 长连接
            user: String,              // 用户名（匿名则为空）
            mount_name: String,        // 订阅的挂载点
            conn_id: String,           // 唯一连接 ID（如 user@ip:port-timestamp）
            relay_stream: Option<TcpStream>, // 若挂载点在其他节点，则为 relay 连接
        }
        
        impl ClientConnection {
            // 写入数据到客户端
            async fn write_data(&mut self, data: &[u8]) -> Result<(), IoError>;
            
            // 建立到其他节点的 relay 连接（若挂载点不在本地）
            async fn connect_relay(&mut self, node_ip: &str) -> Result<(), RelayError>;
            
            // 关闭连接并清理 etcd 状态
            async fn close(&mut self, etcd: &storage::etcd::EtcdClient) -> Result<(), DownloadError>;
        }
    }


    // === 公共组件与工具 ===
    pub mod common {
        // NTRIP 协议解析（请求头、响应头）
        pub mod ntrip {
            pub fn parse_request(stream: &TcpStream) -> Result<NtripRequest, ParseError>;
            pub fn build_response(protocol: NtripProtocol, status: ResponseStatus) -> Vec<u8>;
            
            #[derive(Debug)]
            pub enum NtripRequest {
                Upload {
                    mount_name: String,
                    protocol: NtripProtocol,
                    username: Option<String>,
                    password: String,
                },
                Download {
                    mount_name: String,
                    username: Option<String>,
                    password: Option<String>,
                },
                StrList, // 请求挂载点列表
            }
        }
        
        // RTCM 数据解析工具（旁路分析用）
        pub mod rtcm {
            pub struct RtcmParser;
            
            impl RtcmParser {
                pub fn parse(&self, data: &[u8]) -> Result<RtcmInfo, ParseError>;
            }
            
            pub struct RtcmInfo {
                pub format: String,       // 如 RTCM3.3
                pub freq: f64,            // 数据频率
                pub coords: Option<(f64, f64, f64)>, // 从 GGA 解析的位置
            }
        }
        
        // 错误类型定义（统一错误处理）
        pub mod error {
            #[derive(Debug)]
            pub enum CasterError {
                Config(ConfigError),
                Server(ServerError),
                Storage(StorageError),
                Upload(UploadError),
                Download(DownloadError),
                Parse(ParseError),
                Io(std::io::Error),
            }
            // 实现各种错误的 From 转换
        }
    }


    // === 跨节点数据转发模块 ===
    pub mod relay {
        // 管理与其他节点的 relay 连接
        pub struct RelayManager {
            node_id: String,
            etcd: storage::etcd::EtcdClient,
            peer_connections: HashMap<String, TcpStream>, // 其他节点 ID → 连接
        }
        
        impl RelayManager {
            // 当本地挂载点有数据时，转发到订阅该挂载点的其他节点
            pub async fn forward_to_peers(&mut self, mount_name: &str, data: &[u8]) -> Result<(), RelayError>;
            
            // 发现并连接新节点
            async fn discover_peers(&mut self) -> Result<(), RelayError>;
        }
    }
}