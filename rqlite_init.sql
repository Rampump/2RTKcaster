-- Rqlite数据库初始化脚本
-- 执行命令: rqlite -f rqlite_init.sql

-- 用户表
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    salt TEXT NOT NULL,
    is_admin BOOLEAN DEFAULT false,
    max_connections INTEGER DEFAULT 10,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- 挂载点表
CREATE TABLE IF NOT EXISTS mounts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    format TEXT NOT NULL,
    baud_rate INTEGER NOT NULL DEFAULT 9600,
    latitude REAL NOT NULL DEFAULT 0,
    longitude REAL NOT NULL DEFAULT 0,
    nmea TEXT,
    country TEXT,
    description TEXT,
    generator TEXT,
    compression TEXT DEFAULT 'none',
    auth_required BOOLEAN DEFAULT true,
    max_clients INTEGER DEFAULT 50,
    expires_at INTEGER DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- 访问日志表
CREATE TABLE IF NOT EXISTS access_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT,
    mount_name TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    connection_type TEXT NOT NULL,
    start_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    end_time TIMESTAMP,
    bytes_in INTEGER DEFAULT 0,
    bytes_out INTEGER DEFAULT 0,
    duration INTEGER
);

-- 节点状态表
CREATE TABLE IF NOT EXISTS node_status (
    node_id TEXT PRIMARY KEY,
    ip_address TEXT NOT NULL,
    port INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'unknown',
    last_heartbeat TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    connections INTEGER DEFAULT 0,
    load FLOAT DEFAULT 0
);

-- 创建索引
CREATE INDEX IF NOT EXISTS idx_mounts_name ON mounts(name);
CREATE INDEX IF NOT EXISTS idx_access_logs_mount ON access_logs(mount_name);
CREATE INDEX IF NOT EXISTS idx_access_logs_user ON access_logs(username);
CREATE INDEX IF NOT EXISTS idx_node_status_id ON node_status(node_id);

-- 插入默认管理员用户 (密码: admin123)
INSERT OR IGNORE INTO users (username, password_hash, salt, is_admin, max_connections)
VALUES (
    'admin',
    '5f4dcc3b5aa765d61d8327deb882cf99',
    'salt123',
    true,
    100
);

-- 插入默认挂载点
INSERT OR IGNORE INTO mounts (name, format, baud_rate, latitude, longitude, description)
VALUES (
    'RTCM3',
    'RTCM3',
    115200,
    39.9042,
    116.4074,
    'Default RTCM3 Mount Point'
);