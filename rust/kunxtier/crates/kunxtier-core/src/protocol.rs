//! Description: 控制面协议消息 (当前切片: 登记 / 注册 / 管理 / 信令 / 转发)。
//!
//! Author: qinzhenya
//! Created: 2026-10-20

use std::net::Ipv4Addr;

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

/// 节点级路径策略设置 (FR-10): auto 恢复打洞, force_relay 强制中继。
#[derive(Debug, Serialize, Deserialize)]
pub struct SetNodePolicyRequest {
    pub network: String,
    pub name: String,
    pub policy: PathPolicy,
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
    /// 节点级路径策略 (FR-10)。
    pub policy: PathPolicy,
    /// 最近上报的候选地址 (仅在线节点有意义, 由信令注册表合并, 不落库)。
    pub candidates: Vec<String>,
    /// 节点版本 (hello 上报, 仅在线节点有意义)。
    pub version: Option<String>,
    /// 逐对端路径快照 (路径上报合并; 仅在线节点有意义)。
    pub paths: Vec<PeerPathReport>,
}

/// admin 登录 (控制台会话, feat-kunxtier-console FR-6)。
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub credential: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    /// 会话令牌 (Bearer, 内存态, 随 relay 重启失效)。
    pub token: String,
    /// 过期时间 (unix 秒)。
    pub expires_at: i64,
}

/// 中继汇总条目 (FR-5): 在线卫星与核心内置, 每中继连接节点数来自路径上报聚合。
#[derive(Debug, Serialize)]
pub struct RelayInfo {
    pub addr: String,
    /// "core" | "satellite"。
    pub kind: String,
    pub fingerprint: String,
    /// 当前经该中继转发的节点数。
    pub nodes: usize,
}

#[derive(Debug, Serialize)]
pub struct ApiErrorBody {
    pub error: String,
    pub message: String,
}

// ---- 信令 WS ----

/// 路径策略: 全局自动 + 节点级强制中继 (feat-kunxtier-network FR-10)。
/// 存于节点库存, 经信令 State 初始化下发与 Policy 变更推送; 控制台配置入口随 T11 接入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathPolicy {
    Auto,
    ForceRelay,
}

impl PathPolicy {
    /// 存储与协议线格式 (snake_case, 与 serde 序列化一致)。
    pub fn as_str(self) -> &'static str {
        match self {
            PathPolicy::Auto => "auto",
            PathPolicy::ForceRelay => "force_relay",
        }
    }
}

impl std::str::FromStr for PathPolicy {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "auto" => Ok(PathPolicy::Auto),
            "force_relay" => Ok(PathPolicy::ForceRelay),
            other => Err(format!("未知路径策略: {other} (期望 auto / force_relay)")),
        }
    }
}

impl std::fmt::Display for PathPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
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

/// 中继转发端点: 推送给节点的回落目标。
/// 列表顺序即尝试顺序: 卫星在前, 核心内置转发恒在末位兜底 (FR-3)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalRelay {
    /// 拨号地址 host:port; kind 为 "core" 且为空串时表示
    /// "节点已连接的核心自身", 由节点用自身配置的核心地址代入。
    pub addr: String,
    /// "core" (核心内置转发) | "satellite"。
    pub kind: String,
    /// 该端点 TLS 证书指纹 (sha256 hex), 客户端按条目固定校验 (NFR-6)。
    pub fingerprint: String,
}

/// 路径上报条目 (节点 → 核心信令, T11): 对端与当前路径快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerPathReport {
    pub peer: String,
    /// idle / punching / direct / relayed (与 status 词汇一致)。
    pub path: String,
    /// 走中继时上报中继地址 (与中继列表条目 addr 一致); 其余路径为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
}

/// 客户端 → 服务端信令消息: JSON 文本帧, type 标签分派。
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalClientMsg {
    /// 认证第一步: 声明节点身份 (version 为节点二进制版本, 控制台总览展示)。
    Hello {
        network: String,
        name: String,
        #[serde(default)]
        version: Option<String>,
    },
    /// 挑战应答: base64(std) 的 sha256(nonce || sha256(节点凭证))。
    Auth { digest: String },
    /// 应用层心跳。
    Ping,
    /// 上报本机候选地址。
    Announce { candidates: Vec<String> },
    /// 上报逐对端路径快照 (控制台总览与中继计数的数据源)。
    PathReport { paths: Vec<PeerPathReport> },
    /// 打洞信令: 定向转发给同网在线节点, payload 对服务端不透明。
    Signal {
        to: String,
        payload: serde_json::Value,
    },
}

/// 服务端 → 客户端信令消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// 全量节点表 + 中继列表 + 本节点策略 + STUN 列表: 认证通过后推送。
    State {
        nodes: Vec<SignalPeer>,
        relays: Vec<SignalRelay>,
        policy: PathPolicy,
        /// 核心配置的公共 STUN 服务器 (host:port), 供节点探测候选地址。
        stuns: Vec<String>,
    },
    /// 本节点路径策略变更 (admin 设置时定向推送, FR-10)。
    Policy {
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
    /// 中继列表全量更新 (卫星注册 / 断连摘除 / 令牌吊销踢线), 广播到全部网络。
    Relays {
        relays: Vec<SignalRelay>,
    },
}

// ---- 转发 WS (/ws/relay) ----

/// 转发面二进制帧: [4 字节大端目的 IPv4][4 字节大端源 IPv4][不透明 WireGuard 密文]。
/// 中继只按首部目的路由; 源字段仅供接收方构造回程端点 (T07), 对中继同样不透明。
/// 上下行同构 (FR-4: 中继只见密文)。
pub const FORWARD_HEADER_BYTES: usize = 8;

pub fn encode_forward_frame(dst: Ipv4Addr, src: Ipv4Addr, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(FORWARD_HEADER_BYTES + payload.len());
    frame.extend_from_slice(&dst.octets());
    frame.extend_from_slice(&src.octets());
    frame.extend_from_slice(payload);
    frame
}

/// 解析转发帧; 长度不足或承载非 IPv4 语义之外的错误均由长度判定兜住。
/// 返回 (目的, 源, 密文)。
pub fn decode_forward_frame(bytes: &[u8]) -> Option<(Ipv4Addr, Ipv4Addr, &[u8])> {
    let (head, payload) = bytes.split_at_checked(FORWARD_HEADER_BYTES)?;
    if payload.is_empty() {
        return None;
    }
    // unwrap 理由: split_at_checked 已保证 8 字节, 再对半切不会越界
    let (dst, src) = head.split_at_checked(4)?;
    let dst: [u8; 4] = dst.try_into().expect("长度已由 split_at_checked 保证");
    let src: [u8; 4] = src.try_into().expect("长度已由 split_at_checked 保证");
    Some((Ipv4Addr::from(dst), Ipv4Addr::from(src), payload))
}

/// 转发面文本控制帧: 准入声明与心跳。
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ForwardClientMsg {
    /// 卫星准入: 声明本机虚拟 IP (零知识模式专用; 核心模式 IP 由库存权威决定)。
    Bind { ip: String },
    /// 应用层心跳 (数据面无常态 JSON 流, 空闲时由客户端驱动保活)。
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ForwardServerMsg {
    /// 绑定成功: 本端 IP 与心跳参数 (客户端不硬编码, 与信令 AuthOk 同一原则)。
    Bound {
        ip: String,
        heartbeat_interval_secs: u64,
        idle_timeout_secs: u64,
    },
    Pong,
    Error {
        message: String,
    },
    Closed {
        reason: String,
    },
}

// ---- 卫星注册 WS (/ws/satellite, 核心侧) ----

/// 卫星 → 核心: 注册声明。relay 令牌可复用 (FR-9), 不因注册而消耗。
/// 双向均有实现 (进程在 relay crate 内), 故两个方向都派生序列化。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SatelliteClientMsg {
    Register {
        token: String,
        /// 对节点公布的拨号地址 host:port, 由卫星自报 (核心不探测)。
        advertise: String,
        /// 卫星 TLS 证书指纹 (sha256 hex), 随列表分发给节点固定。
        fingerprint: String,
    },
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SatelliteServerMsg {
    Registered,
    Error { message: String },
    Pong,
    Closed { reason: String },
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_frame_roundtrip() {
        let frame = encode_forward_frame(
            "100.64.0.7".parse().unwrap(),
            "100.64.0.9".parse().unwrap(),
            &[1, 2, 3],
        );
        let (dst, src, payload) = decode_forward_frame(&frame).unwrap();
        assert_eq!(dst.to_string(), "100.64.0.7");
        assert_eq!(src.to_string(), "100.64.0.9");
        assert_eq!(payload, &[1, 2, 3]);
    }

    #[test]
    fn forward_frame_rejects_short_input() {
        assert!(decode_forward_frame(&[]).is_none());
        assert!(decode_forward_frame(&[100, 64]).is_none());
        // 头部完整但没有载荷: 不构成合法 WG 帧
        assert!(decode_forward_frame(&[100, 64, 0, 7]).is_none());
        assert!(decode_forward_frame(&[100, 64, 0, 7, 100, 64, 0, 9]).is_none());
    }

    #[test]
    fn signal_relay_serializes_fingerprint() {
        let relay = SignalRelay {
            addr: "1.2.3.4:443".into(),
            kind: "satellite".into(),
            fingerprint: "ab".repeat(32),
        };
        let v = serde_json::to_value(&relay).unwrap();
        assert_eq!(v["fingerprint"], "ab".repeat(32));
    }
}
