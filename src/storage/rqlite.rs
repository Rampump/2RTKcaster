//! Rqlite 关系型数据库交互模块
//! 实现挂载点与用户信息的持久化存储

use std::sync::Arc;
use reqwest::{Client as HttpClient, Error as ReqwestError};
use serde::{Serialize, Deserialize};
use serde_json::Value as JsonValue;
use super::super::common::error::{CasterError, StorageError};
use bcrypt::{hash, verify, DEFAULT_COST};
use chrono::Utc;

/// Rqlite 客户端
#[derive(Clone)]
pub struct RqliteClient {
    client: Arc<HttpClient>,
    addr: String,
    username: Option<String>,
    password: Option<String>,
}

/// 挂载点信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MountInfo {
    /// 挂载点名称
    pub name: String,
    /// 上传密码（哈希）
    pub upload_password: String,
    /// 所属用户
    pub owner: String,
    /// 是否允许匿名下载
    pub is_public: bool,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
}

/// 用户信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserInfo {
    /// 用户名
    pub username: String,
    /// 密码哈希
    pub password_hash: String,
    /// 最大连接数限制
    pub max_connections: u32,
    /// 可访问的挂载点
    pub allowed_mounts: Vec<String>,
    /// 是否为管理员
    pub is_admin: bool,
    /// 创建时间
    pub created_at: String,
}

impl RqliteClient {
    /// 创建新的 Rqlite 客户端
    pub fn new(addr: &str, username: Option<String>, password: Option<String>) -> Self {
        Self {
            client: Arc::new(HttpClient::new()),
            addr: addr.to_string(),
            username,
            password,
        }
    }

    /// 执行 SQL 查询
    async fn query(&self, sql: &str, params: &[&str]) -> Result<Vec<JsonValue>, CasterError> {
        let url = format!("http://{}/db/query", self.addr);
        let mut body = serde_json::json!({"sql": sql, "args": params});

        let request = self.client.post(&url)
            .basic_auth(self.username.clone().unwrap_or_default(), self.password.clone());

        let response = request.json(&body)
            .send()
            .await
            .map_err(|e| StorageError::ConnectionError(format!("HTTP请求失败: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(StorageError::QueryError(format!("SQL执行失败 ({}): {}", status, text)));
        }

        let result: serde_json::Value = response.json().await
            .map_err(|e| StorageError::SerializationError(e))?;

        let mut rows = Vec::new();
        if let Some(results) = result.get("results") {
            for res in results.as_array().unwrap_or(&[]) {
                if let Some(r) = res.get("rows") {
                    rows.extend(r.as_array().unwrap_or(&[]).to_vec());
                }
            }
        }

        Ok(rows)
    }

    /// 执行 SQL 写入操作
    async fn execute(&self, sql: &str, params: &[&str]) -> Result<u64, CasterError> {
        let url = format!("http://{}/db/execute", self.addr);
        let body = serde_json::json!({"sql": sql, "args": params});

        let request = self.client.post(&url)
            .basic_auth(self.username.clone().unwrap_or_default(), self.password.clone());

        let response = request.json(&body)
            .send()
            .await
            .map_err(|e| StorageError::ConnectionError(format!("HTTP请求失败: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(StorageError::QueryError(format!("SQL执行失败 ({}): {}", status, text)));
        }

        let result: serde_json::Value = response.json().await
            .map_err(|e| StorageError::SerializationError(e))?;

        let affected = result.get("results")
            .and_then(|r| r.as_array().first())
            .and_then(|r| r.get("rows_affected"))
            .and_then(|r| r.as_u64())
            .unwrap_or(0);

        Ok(affected)
    }

    /// 获取挂载点信息
    pub async fn get_mount(&self, mount_name: &str) -> Result<Option<MountInfo>, CasterError> {
        let sql = "SELECT name, upload_password, owner, is_public, created_at, updated_at FROM mounts WHERE name = ?";
        let rows = self.query(sql, &[mount_name]).await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let row = &rows[0];
        Ok(Some(MountInfo {
            name: row[0].as_str().unwrap_or_default().to_string(),
            upload_password: row[1].as_str().unwrap_or_default().to_string(),
            owner: row[2].as_str().unwrap_or_default().to_string(),
            is_public: row[3].as_bool().unwrap_or_default(),
            created_at: row[4].as_str().unwrap_or_default().to_string(),
            updated_at: row[5].as_str().unwrap_or_default().to_string(),
        }))
    }

    /// 检查挂载点是否存在
    pub async fn mount_exists(&self, mount_name: &str) -> Result<bool, CasterError> {
        let sql = "SELECT name FROM mounts WHERE name = ? LIMIT 1";
        let rows = self.query(sql, &[mount_name]).await?;
        Ok(!rows.is_empty())
    }

    /// 获取用户信息
    pub async fn get_user(&self, username: &str) -> Result<Option<UserInfo>, CasterError> {
        let sql = "SELECT username, password_hash, max_connections, allowed_mounts, is_admin, created_at FROM users WHERE username = ?";
        let rows = self.query(sql, &[username]).await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let row = &rows[0];
        let allowed_mounts: Vec<String> = serde_json::from_str(row[3].as_str().unwrap_or_default())
            .map_err(|e| StorageError::SerializationError(e))?;

        Ok(Some(UserInfo {
            username: row[0].as_str().unwrap_or_default().to_string(),
            password_hash: row[1].as_str().unwrap_or_default().to_string(),
            max_connections: row[2].as_u64().unwrap_or_default() as u32,
            allowed_mounts,
            is_admin: row[4].as_bool().unwrap_or_default(),
            created_at: row[5].as_str().unwrap_or_default().to_string(),
        }))
    }

    /// 验证用户密码
    pub async fn verify_user_password(&self, username: &str, password: &str) -> Result<bool, CasterError> {
        let user = self.get_user(username).await?
            .ok_or_else(|| StorageError::UserNotFound(username.to_string()))?;

        Ok(verify(password, &user.password_hash).map_err(|e| StorageError::PasswordVerifyError(e))?)
    }

    /// 创建挂载点
    pub async fn create_mount(&self, mount: &MountInfo) -> Result<(), CasterError> {
        let sql = "INSERT INTO mounts (name, upload_password, owner, is_public, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)";
        let now = Utc::now().to_rfc3339();
        let affected = self.execute(
            sql,
            &[&mount.name, &mount.upload_password, &mount.owner, &mount.is_public.to_string(), &now, &now]
        ).await?;

        if affected == 0 {
            return Err(StorageError::QueryError("创建挂载点失败，未影响任何行".to_string()));
        }

        Ok(())
    }

    /// 更新挂载点密码
    pub async fn update_mount_password(&self, mount_name: &str, new_password: &str) -> Result<(), CasterError> {
        let hashed = hash(new_password, DEFAULT_COST)
            .map_err(|e| StorageError::PasswordHashError(e))?;
        let sql = "UPDATE mounts SET upload_password = ?, updated_at = ? WHERE name = ?";
        let now = Utc::now().to_rfc3339();
        let affected = self.execute(sql, &[&hashed, &now, mount_name]).await?;

        if affected == 0 {
            return Err(StorageError::QueryError("更新挂载点密码失败，未找到挂载点".to_string()));
        }

        Ok(())
    }
}