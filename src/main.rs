//! NTRIP Caster 服务器入口
//! 高性能RTK数据转发服务，支持分布式部署

use clap::Parser;
use std::net::SocketAddr;
use tracing::info;
use ntrip_caster::start_server;

/// NTRIP Caster 服务器配置参数
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 配置文件路径
    #[arg(short, long, default_value = "config.toml")]
    config: String,

    /// 绑定地址
    #[arg(short, long, default_value = "0.0.0.0:2101")]
    bind_addr: SocketAddr,

    /// 节点ID
    #[arg(long, env = "NODE_ID")]
    node_id: String,

    /// 节点IP
    #[arg(long, env = "NODE_IP")]
    node_ip: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 配置Tokio运行时，启用io_uring（Linux平台）
    let mut rt_builder = tokio::runtime::Builder::new_multi_thread();
    #[cfg(target_os = "linux")]
    {
        rt_builder.enable_io_uring();
    }
    let rt = rt_builder.build()?;
    rt.block_on(async {
        // 解析命令行参数
        let args = Args::parse();

        // 初始化环境变量
        std::env::set_var("NODE_ID", args.node_id);
        std::env::set_var("NODE_IP", args.node_ip.to_string());

        // 初始化日志和核心组件
        ntrip_caster::init().await?;
        info!("NTRIP Caster starting with config: {}", args.config);

        // 启动服务器
        start_server(&args.config).await?;

        Ok(())
    })
} {
    // 解析命令行参数
    let args = Args::parse();

    // 初始化环境变量
    std::env::set_var("NODE_ID", args.node_id);
    std::env::set_var("NODE_IP", args.node_ip.to_string());

    // 初始化日志和核心组件
    ntrip_caster::init().await?;
    info!("NTRIP Caster starting with config: {}", args.config);

    // 启动服务器
    start_server(&args.config).await?;

    Ok(())
}