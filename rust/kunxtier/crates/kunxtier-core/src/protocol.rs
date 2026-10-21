//! Description: 控制面协议消息 (当前切片: 登记 / 注册 / 管理 / 信令)。
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
    /// 是否在线 (信令通道当前状态, 由服务端注册表合并, 不落库)。
    pub online: bool,
}

#[derive(Debug, Serialize)]
pub struct ApiErrorBody {
    pub error: String,
    pub message: String,
}

// ---- 信令 WS ----

/// 路径策略: 全局自动 + 节点级强制中继 (feat-kunxtier-network FR-10)。
/// 当前实现仅 Auto; 控制台配置与信令下发策略随 T11 接入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathPolicy {
    Auto,
    ForceRelay,
}

/// 信令节点表条目: 推送给在线节点的同网成员, 含对端 WireGuard 公钥。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalPeer {
    pub name: String,
    pub ip: String,
    pub wg_public_key: String,
    pub online: bool,
    /// 最近一次上报的候选地址 (host:port), 仅在线节点有意义。
    pub candidates: Vec<String>,
}

/// 中继转发端点 (T04 仅占位, 卫星注册与列表分发随 T05 接入)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalRelay {
    pub addr: String,
    /// "core" (核心内置转发) | "satellite"。
    pub kind: String,
}

/// 客户端 → 服务端信令消息: JSON 文本帧, type 标签分派。
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalClientMsg {
    /// 认证第一步: 声明节点身份。
    Hello { network: String, name: String },
    /// 挑战应答: base64(std) 的 sha256(nonce || sha256(节点凭证))。
    Auth { digest: String },
    /// 应用层心跳。
    Ping,
    /// 上报本机候选地址。
    Announce { candidates: Vec<String> },
    /// 打洞信令: 定向转发给同网在线节点, payload 对服务端不透明。
    Signal {
        to: String,
        payload: serde_json::Value,
    },
}

/// 服务端 → 客户端信令消息。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalServerMsg {
    /// 挑战: 32 字节随机 nonce 的 base64(std)。
    Challenge {
        nonce: String,
    },
    /// 认证通过: 本节点 IP 与心跳参数 (客户端不硬编码, FR-15)。
    AuthOk {
        ip: String,
        heartbeat_interval_secs: u64,
        idle_timeout_secs: u64,
    },
    AuthErr {
        message: String,
    },
    /// 全量节点表 + 中继列表 + 本节点策略: 认证通过后推送。
    State {
        nodes: Vec<SignalPeer>,
        relays: Vec<SignalRelay>,
        policy: PathPolicy,
    },
    /// 同网成员上线 (FR-5)。
    PeerOnline {
        peer: SignalPeer,
    },
    /// 同网成员下线 (FR-5)。
    PeerOffline {
        name: String,
    },
    Pong,
    /// 转发自 from 的打洞信令。
    Signal {
        from: String,
        payload: serde_json::Value,
    },
    /// 打洞信令投递失败 (目标不在线等)。
    SignalErr {
        to: String,
        message: String,
    },
    /// 协议错误提示 (连接不断开)。
    Error {
        message: String,
    },
    /// 服务端裁决断开: 被同名新连接替换 / 凭证吊销。
    Closed {
        reason: String,
    },
}
