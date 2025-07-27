# NTRIP Caster

高性能分布式NTRIP协议服务器，支持多节点部署和实时RTK数据转发。

## 目录

- [功能特点](#功能特点)
- [系统要求](#系统要求)
- [安装部署](#安装部署)
  - [Windows环境](#windows环境)
  - [Linux环境](#linux环境)
- [配置说明](#配置说明)
- [运行服务](#运行服务)
- [验证部署](#验证部署)
- [故障排除](#故障排除)
- [常见问题](#常见问题)

## 功能特点

- 完整支持NTRIP 1.0/2.0协议
- 分布式架构，支持多节点部署
- 实时数据中继和负载均衡
- 完善的连接管理和统计监控
- 支持RTCM3.x数据解析和处理
- 基于Etcd的分布式协调和Rqlite的数据持久化
- 高性能IO处理，支持Linux io_uring
- RESTful API管理接口

## 系统要求

- 操作系统: Linux (推荐) 或 Windows
- 内存: 至少2GB RAM
- 处理器: 至少2核CPU
- 网络: 稳定的网络连接
- 数据库: Etcd集群 (>=3.4) 和 Rqlite (>=6.0)

## 安装部署

### Windows环境

#### 1. 安装依赖

##### 安装Rust

1. 访问 [Rust官网](https://www.rust-lang.org/tools/install) 下载并运行rustup-init.exe
2. 按照安装向导操作，选择默认配置即可
3. 安装完成后，打开新的命令提示符，验证安装：
   ```cmd
   rustc --version
   cargo --version
   ```

##### 安装Git

1. 访问 [Git官网](https://git-scm.com/download/win) 下载Git for Windows
2. 运行安装程序，使用默认配置即可
3. 验证安装：
   ```cmd
   git --version
   ```

##### 安装Docker Desktop

1. 访问 [Docker官网](https://www.docker.com/products/docker-desktop) 下载Docker Desktop
2. 安装并启动Docker，等待服务启动完成
3. 验证Docker是否正常运行：
   ```cmd
   docker --version
   docker-compose --version
   ```

#### 2. 获取代码

```cmd
# 克隆代码仓库
git clone https://github.com/your-org/ntripcaster.git
cd ntripcaster
```

#### 3. 启动数据库

```cmd
# 启动Etcd
docker run -d --name etcd -p 2379:2379 -e ETCD_ENABLE_V2=false ^
  quay.io/coreos/etcd:v3.5.0 ^
  /usr/local/bin/etcd --listen-client-urls http://0.0.0.0:2379 ^
  --advertise-client-urls http://0.0.0.0:2379 ^
  --initial-cluster-state new

# 启动Rqlite
docker run -d --name rqlite -p 4001:4001 -p 4002:4002 ^
  rqlite/rqlite:6.10.0 ^
  rqlited -http-addr 0.0.0.0:4001 -raft-addr 0.0.0.0:4002 /data

# 初始化数据库
docker exec -i rqlite rqlite -s http://localhost:4001 < rqlite_init.sql
```

#### 4. 配置文件

```cmd
# 复制配置文件模板
copy config.toml.example config.toml

# 使用记事本编辑配置文件
notepad config.toml
```

#### 5. 构建项目

```cmd
# 构建发布版本
cargo build --release

# 查看生成的可执行文件
dir target\release\ntripcaster.exe
```

#### 6. 运行服务

```cmd
# 设置环境变量
set RUST_LOG=info
set CONFIG_PATH=config.toml

# 运行服务
.target\release\ntripcaster.exe
```

#### 7. 安装为Windows服务（可选）

使用NSSM工具将程序安装为Windows服务：

1. 下载 [NSSM](https://nssm.cc/download)
2. 将nssm.exe放入系统路径或当前目录
3. 安装服务：
   ```cmd
   nssm install Ntripcaster
   ```
4. 在弹出的窗口中设置：
   - Path: 选择target\release\ntripcaster.exe
   - Startup directory: 项目根目录
   - Environment: 添加RUST_LOG=info和CONFIG_PATH=config.toml
5. 启动服务：
   ```cmd
   nssm start Ntripcaster
   ```

### Linux环境

#### 1. 安装依赖

##### Ubuntu/Debian

```bash
# 更新系统
sudo apt update && sudo apt upgrade -y

# 安装依赖
sudo apt install -y build-essential git curl docker.io docker-compose rustc cargo

# 启动Docker服务
sudo systemctl enable docker
sudo systemctl start docker
sudo usermod -aG docker $USER
```

##### CentOS/RHEL

```bash
# 安装依赖
sudo yum install -y gcc git curl docker rust cargo

sudo systemctl enable docker
sudo systemctl start docker
sudo usermod -aG docker $USER
```

> 注意：添加用户到docker组后需要重新登录才能生效

#### 2. 获取代码

```bash
# 克隆代码仓库
git clone https://github.com/your-org/ntripcaster.git
cd ntripcaster
```

#### 3. 启动数据库

```bash
# 启动Etcd
docker run -d --name etcd -p 2379:2379 -e ETCD_ENABLE_V2=false \
  quay.io/coreos/etcd:v3.5.0 \
  /usr/local/bin/etcd --listen-client-urls http://0.0.0.0:2379 \
  --advertise-client-urls http://0.0.0.0:2379 \
  --initial-cluster-state new

# 启动Rqlite
docker run -d --name rqlite -p 4001:4001 -p 4002:4002 \
  rqlite/rqlite:6.10.0 \
  rqlited -http-addr 0.0.0.0:4001 -raft-addr 0.0.0.0:4002 /data

# 初始化数据库
docker exec -i rqlite rqlite -s http://localhost:4001 < rqlite_init.sql
```

#### 4. 配置文件

```bash
# 复制配置文件模板
cp config.toml.example config.toml

# 编辑配置文件
nano config.toml
```

#### 5. 构建项目

```bash
# 构建发布版本
cargo build --release --features linux-optimizations

# 查看生成的可执行文件
ls -l target/release/ntripcaster
```

#### 6. 运行服务

##### 使用系统服务（推荐）

```bash
# 复制服务文件
sudo cp ntripcaster.service /etc/systemd/system/

sudo systemctl daemon-reload
sudo systemctl enable ntripcaster
sudo systemctl start ntripcaster

sudo systemctl status ntripcaster
```

##### 直接运行

```bash
# 设置环境变量
export RUST_LOG=info
export CONFIG_PATH=./config.toml

# 运行服务
./target/release/ntripcaster
```

## 配置说明

配置文件使用TOML格式，主要包含以下部分：

### 核心配置项

```toml
[node]
id = "node-01"          # 节点ID，集群中唯一
bind_addr = "0.0.0.0:2101" # 服务绑定地址

[storage.etcd]
endpoints = ["http://127.0.0.1:2379"] # Etcd集群地址
prefix = "/ntripcaster/v1"          # 键前缀

[storage.rqlite]
addr = "http://127.0.0.1:4001" # Rqlite地址
username = ""                 # 用户名（如有）
password = ""                 # 密码（如有）
```

### 详细配置

完整配置请参考 [config.toml.example](config.toml.example) 文件，其中包含所有可用配置项及说明。

## 运行服务

### 验证部署

部署完成后，可以通过以下方式验证服务是否正常运行：

### 1. 检查服务状态

#### 通过API验证

```bash
# Linux/macOS
curl http://localhost:8080/status

# Windows PowerShell
Invoke-RestMethod http://localhost:8080/status
```

预期输出：
```json
{
  "status": "running",
  "version": "0.1.0",
  "node_id": "node-01",
  "uptime": 120
}
```

#### 检查服务进程

```bash
# Linux
ps aux | grep ntripcaster

sudo systemctl status ntripcaster

# Windows
 tasklist | findstr ntripcaster
```

### 2. 验证数据库连接

```bash
# 检查Etcd连接
docker exec -it etcd etcdctl get --prefix /ntripcaster/v1

# 检查Rqlite数据
curl -X POST http://localhost:4001/db/query -d '{"q": "SELECT * FROM mounts"}'
```

### 3. 测试NTRIP客户端连接

使用NTRIP客户端软件（如RTKLIB的str2str）测试连接：

```bash
str2str -in ntrip://:password@localhost:2101/RTCM3 -out serial://COM1:115200
```

### 4. 查看日志

```bash
# Linux
 tail -f /var/log/ntripcaster/ntripcaster.log

# Windows
Get-Content -Path logs\ntripcaster.log -Tail 100 -Wait
```

## 安全注意事项

- 生产环境必须启用TLS加密
- 敏感配置（如密码）应通过环境变量注入
- 限制API访问权限，仅允许可信IP
- 定期备份Rqlite数据库
- 监控系统资源使用情况

## 常见问题

### 1. 如何扩展服务容量？

NTRIP Caster支持多节点部署以提高容量和可用性：
1. 在新服务器上重复安装部署步骤
2. 确保所有节点使用相同的Etcd集群
3. 在新节点配置中设置不同的`node.id`
4. 启动新节点，系统会自动加入集群并分担负载

### 2. 如何备份数据？

**Rqlite数据库备份**：
```bash
# 创建数据库备份
curl -X POST http://localhost:4001/db/backup -o backup.sql

# 恢复备份
cat backup.sql | curl -X POST http://localhost:4001/db/execute -d @-
```

**配置文件备份**：
```bash
# Linux
cp config.toml config.toml.bak

# Windows
copy config.toml config.toml.bak
```

### 3. 如何更新服务？

```bash
# 1. 获取最新代码
git pull

# 2. 构建新版本
cargo build --release --features linux-optimizations

# 3. 停止当前服务
sudo systemctl stop ntripcaster

# 4. 替换可执行文件
sudo cp target/release/ntripcaster /usr/local/bin/

# 5. 启动服务
sudo systemctl start ntripcaster
```

### 4. 如何启用TLS/SSL加密？

1. 准备SSL证书（自签名或CA颁发）
2. 修改配置文件：
   ```toml
   [tls]
   enable = true
   cert_path = "/path/to/cert.pem"
   key_path = "/path/to/key.pem"
   ```
3. 重启服务

### 5. 如何限制客户端连接数？

在配置文件中设置每个挂载点的最大客户端数：
```toml
[mount_limits]
RTCM3 = 50  # 限制RTCM3挂载点最多50个连接
DEFAULT = 20 # 默认限制
```

## 性能优化

- 在Linux系统上启用io_uring特性
- 根据实际负载调整连接池大小
- 合理设置缓存参数，平衡内存使用和数据库访问
- 使用高性能网络接口和存储
- 生产环境建议使用专用服务器或云服务器（至少2核4GB内存）

## 故障排除

### 常见错误及解决方法

#### 1. 服务无法启动

**症状**：服务启动失败或立即退出

**排查步骤**：
1. 检查配置文件格式是否正确：
   ```bash
   # 使用toml-lint检查配置文件
   cargo install toml-lint
   toml-lint config.toml
   ```
2. 检查数据库连接：
   ```bash
   # 测试Etcd连接
   curl http://localhost:2379/health
   
   # 测试Rqlite连接
   curl http://localhost:4001/health
   ```
3. 查看详细日志获取错误信息

#### 2. 数据库连接失败

**症状**：服务启动时报数据库连接错误

**解决方法**：
- 确认Etcd和Rqlite服务是否正常运行
- 检查配置文件中的数据库地址和端口是否正确
- 验证数据库认证信息（如适用）
- 检查防火墙设置是否允许访问数据库端口

#### 3. 客户端无法连接

**症状**：NTRIP客户端无法连接到服务

**解决方法**：
- 检查服务是否监听正确的地址和端口：
  ```bash
  # Linux
  netstat -tulpn | grep ntripcaster
  
  # Windows
  netstat -ano | findstr :2101
  ```
- 确认挂载点名称是否正确
- 检查认证信息是否正确
- 查看服务日志获取详细错误信息

### 日志文件

日志文件默认位置：
- Linux: `/var/log/ntripcaster/` 或 `./logs/`
- Windows: `./logs/`

关键日志级别：
- `ERROR`: 错误信息
- `WARN`: 警告信息
- `INFO`: 一般信息
- `DEBUG`: 调试信息（仅开发环境）

### 网络排查

```bash
# 检查端口是否开放
# Linux
ss -tuln | grep -E '2101|8080|2379|4001'

# Windows
netstat -ano | findstr /i listening | findstr /i "2101 8080 2379 4001"

# 测试端口连通性
# Linux/macOS
nc -zv localhost 2101

# Windows
Test-NetConnection -ComputerName localhost -Port 2101
```

## 相关文档

- [NTRIP协议规范](https://igs.bkg.bund.de/ntrip/index_de.html)
- [RTCM3消息格式](https://www.rtcm.org/)
- [Etcd文档](https://etcd.io/docs/)
- [Rqlite文档](https://rqlite.io/)

## 许可证

MIT