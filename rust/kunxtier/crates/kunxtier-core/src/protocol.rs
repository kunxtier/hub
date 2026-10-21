//! Description: 控制面协议消息 (当前切片: 登记 / 注册 / 管理)。
//!
//! Author: qinzhenya
//! Created: 2026-10-20

use serde::{Deserialize, Serialize};

// ---- 节点注册 ----

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub token: String,
    pub name: String,
    /// WireGuard 公钥: base64 (std) 编码的 32 字节。
    pub wg_public_key: String,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub network: String,
    pub ip: String,
    /// 节点凭证明文, 仅此一次返回。
    pub node_credential: String,
}

// ---- 管理操作 ----

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateNetworkRequest {
    pub name: String,
    /// 形如 "100.66.0.0/16"; 缺省按创建顺序取下一个 /16。
    pub cidr: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub network: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IssueTokenRequest {
    pub network: String,
    pub user: Option<String>,
    /// "node" (默认) 或 "relay"。
    #[serde(default)]
    pub kind: Option<String>,
    /// 有效期秒数, 缺省 86400 (一天)。
    #[serde(default)]
    pub ttl_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct IssueTokenResponse {
    pub token_id: i64,
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoveNodeRequest {
    pub network: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct NetworkInfo {
    pub name: String,
    pub cidr: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct NodeInfo {
    pub network: String,
    pub name: String,
    pub ip: String,
    /// 属主标注 (MVP 仅标注, 见 feat-kunxtier-access FR-4)。
    pub owner: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct ApiErrorBody {
    pub error: String,
    pub message: String,
}
