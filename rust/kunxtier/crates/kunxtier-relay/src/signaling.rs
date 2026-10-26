//! Description: 信令 WS: 节点凭证挑战应答、在线注册表、节点表分发与打洞信令转发。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 挂载于管理 API 同一 TLS 服务。协议形态 (2026-10-21 裁决): 服务端发 32 字节
//! 随机 nonce, 客户端应答 sha256(nonce || sha256(凭证)), 凭证明文与哈希均不传输。
//! 心跳 10s / 空闲超时 30s, 参数经 AuthOk 下发, 客户端不硬编码 (FR-15)。

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use tokio::sync::mpsc;

use kunxtier_core::protocol::{PathPolicy, SignalClientMsg, SignalPeer, SignalServerMsg};
use kunxtier_core::token;

use crate::server::AppState;
use crate::Result;

/// 心跳间隔与空闲超时 (AuthOk 下发; 2026-10-21 裁决: 10s / 30s)。
pub const HEARTBEAT_INTERVAL_SECS: u64 = 10;
pub const IDLE_TIMEOUT_SECS: u64 = 30;
/// 未认证连接的完成时限, 超时即关: 未认证并发驻留由此约束 (NFR-9)。
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// 每连接出站队列深度: 满即视为慢消费者, 投递失败按不在线处理。
const OUTBOUND_CAPACITY: usize = 64;
/// 候选地址上报上限 (条数 / 单条字节数), 防内存滥用。
const MAX_CANDIDATES: usize = 8;
const MAX_CANDIDATE_LEN: usize = 64;
/// 单条消息大小上限: 信令帧语义上只有小 JSON (默认 64 MiB 远超所需),
/// 收紧以防已认证端点用超大帧造成内存与解析压力。
const MAX_MESSAGE_BYTES: usize = 256 * 1024;

// ------------------------------ 在线注册表 --------------------------------

/// 在线节点键: (网络名, 节点名); 网络名全局唯一, 可作隔离域键 (FR-11)。
type NodeKey = (String, String);

struct Slot {
    tx: mpsc::Sender<SignalServerMsg>,
    /// 代际号: 同名新连接顶替旧连接时递增, 旧会话的收尾只摘除自己这一代。
    gen: u64,
    candidates: Vec<String>,
}

/// 在线节点注册表: 纯内存, 随进程生命周期 (核心重启后节点自动重连, FR-7)。
pub struct OnlineRegistry {
    inner: Mutex<HashMap<NodeKey, Slot>>,
    next_gen: AtomicU64,
}

impl OnlineRegistry {
    pub fn new() -> Self {
        OnlineRegistry {
            inner: Mutex::new(HashMap::new()),
            next_gen: AtomicU64::new(1),
        }
    }

    /// unwrap 理由: 锁中毒即持锁线程在临界区内 panic, 注册表状态已不可信,
    /// 快速失败优于以不可信状态继续分发。
    fn lock(&self) -> MutexGuard<'_, HashMap<NodeKey, Slot>> {
        self.inner.lock().unwrap()
    }

    /// 登记并顶替同名旧连接: 旧连接收到 Closed 后自行退出, 其收尾不再生效。
    pub fn register(&self, key: NodeKey, tx: mpsc::Sender<SignalServerMsg>) -> u64 {
        let gen = self.next_gen.fetch_add(1, Ordering::Relaxed);
        let mut map = self.lock();
        if let Some(old) = map.insert(
            key,
            Slot {
                tx,
                gen,
                candidates: Vec::new(),
            },
        ) {
            let _ = old.tx.try_send(SignalServerMsg::Closed {
                reason: "同名新连接已接入".into(),
            });
        }
        gen
    }

    /// 摘除本会话代际: 已被替换 / 已被踢时返回 false (收尾不广播下线)。
    pub fn unregister(&self, key: &NodeKey, gen: u64) -> bool {
        let mut map = self.lock();
        match map.get(key) {
            Some(slot) if slot.gen == gen => {
                map.remove(key);
                true
            }
            _ => false,
        }
    }

    /// 更新候选地址 (仅当前代际生效)。
    pub fn set_candidates(&self, key: &NodeKey, gen: u64, candidates: Vec<String>) {
        let mut map = self.lock();
        if let Some(slot) = map.get_mut(key) {
            if slot.gen == gen {
                slot.candidates = candidates;
            }
        }
    }

    /// 管理侧摘除 (节点移除 / 属主禁用): 通知断开并广播下线; 未在线则无动作。
    pub fn kick(&self, network: &str, name: &str) {
        let removed = {
            let mut map = self.lock();
            map.remove(&(network.to_string(), name.to_string()))
        };
        if let Some(slot) = removed {
            let _ = slot.tx.try_send(SignalServerMsg::Closed {
                reason: "凭证已被吊销".into(),
            });
            self.broadcast(
                network,
                SignalServerMsg::PeerOffline {
                    name: name.to_string(),
                },
                Some(name),
            );
        }
    }

    /// 定向投递 (打洞信令): 不在线或队列满 (慢消费者) 返回 false。
    pub fn send_to(&self, network: &str, name: &str, msg: SignalServerMsg) -> bool {
        let map = self.lock();
        match map.get(&(network.to_string(), name.to_string())) {
            Some(slot) => slot.tx.try_send(msg).is_ok(),
            None => false,
        }
    }

    /// 同网广播 (可选排除自身; 上下线事件用, FR-5)。
    pub fn broadcast(&self, network: &str, msg: SignalServerMsg, except: Option<&str>) {
        let map = self.lock();
        for (key, slot) in map.iter() {
            if key.0 == network && Some(key.1.as_str()) != except {
                let _ = slot.tx.try_send(msg.clone());
            }
        }
    }

    /// 同网在线节点的候选地址快照 (节点表推送合并用)。
    pub fn online_candidates(&self, network: &str) -> HashMap<String, Vec<String>> {
        self.lock()
            .iter()
            .filter(|(k, _)| k.0 == network)
            .map(|(k, s)| (k.1.clone(), s.candidates.clone()))
            .collect()
    }

    /// 各网络在线节点名集合 (admin 节点列表合并在线态用)。
    pub fn online_by_network(&self) -> HashMap<String, HashSet<String>> {
        let mut out: HashMap<String, HashSet<String>> = HashMap::new();
        for (k, _) in self.lock().iter() {
            out.entry(k.0.clone()).or_default().insert(k.1.clone());
        }
        out
    }

    /// 广播到全部网络 (中继列表等全网级事件: 卫星不感知网络, 列表全网同构)。
    pub fn broadcast_all(&self, msg: SignalServerMsg) {
        for (_, slot) in self.lock().iter() {
            let _ = slot.tx.try_send(msg.clone());
        }
    }
}

impl Default for OnlineRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------ 会话状态机 --------------------------------

/// WS 建连入口: 按源限速后再升级 (NFR-9: 未认证建连有频率上限)。
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    if !state.ws_limiter.check(addr.ip()) {
        return (StatusCode::TOO_MANY_REQUESTS, "信令建连过于频繁, 稍后再试").into_response();
    }
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| session(socket, state, addr))
}

/// 认证结果: 会话身份; 凭证相关数据在认证完成后即不可达。
pub(crate) struct Authenticated {
    pub(crate) network: String,
    pub(crate) name: String,
    pub(crate) ip: String,
}

/// 信令会话: 认证 (限时) → 登记 → 初始化推送 → 事件循环 → 收尾。
/// 主循环为事件驱动的最外层封装 (rust.md 架构约束), 退出与清理路径显式收口。
async fn session(mut socket: WebSocket, state: Arc<AppState>, peer: SocketAddr) {
    let auth = match authenticate(&mut socket, &state, peer).await {
        Ok(a) => a,
        Err(reason) => {
            tracing::info!(%peer, %reason, "信令认证未完成");
            let _ = socket.close().await;
            return;
        }
    };
    let key: NodeKey = (auth.network.clone(), auth.name.clone());
    let (tx, mut rx) = mpsc::channel::<SignalServerMsg>(OUTBOUND_CAPACITY);
    let gen = state.online.register(key.clone(), tx);
    // 认证全程无凭证明文经手, 日志仅记身份 (NFR-4)
    tracing::info!(network = %key.0, node = %key.1, %peer, "信令通道建立");

    if let Err(reason) = announce_and_push(&mut socket, &state, &auth).await {
        teardown(&state, &key, gen);
        let _ = socket.close().await;
        tracing::warn!(network = %key.0, node = %key.1, %reason, "信令初始化推送失败");
        return;
    }

    let (mut sender, mut receiver) = socket.split();
    let idle = Duration::from_secs(IDLE_TIMEOUT_SECS);
    let reason: String = loop {
        tokio::select! {
            biased;
            // 注册表侧事件优先 (上下线 / 信令转发 / 服务端断开)
            out = rx.recv() => {
                match out {
                    Some(msg) => {
                        let server_close = matches!(msg, SignalServerMsg::Closed { .. });
                        if send_frame(&mut sender, &msg).await.is_err() {
                            break "发送失败".into();
                        }
                        if server_close {
                            break "服务端断开".into();
                        }
                    }
                    None => break "注册表通道关闭".into(),
                }
            }
            received = tokio::time::timeout(idle, receiver.next()) => {
                match received {
                    Err(_) => break "空闲超时".into(),
                    Ok(None) => break "对端断开".into(),
                    Ok(Some(Err(e))) => break format!("接收错误: {e}"),
                    Ok(Some(Ok(msg))) => {
                        if let Some(stop) =
                            handle_client_msg(&mut sender, &state, &key, gen, msg).await
                        {
                            break stop;
                        }
                    }
                }
            }
        }
    };
    let _ = sender.close().await;
    teardown(&state, &key, gen);
    tracing::info!(network = %key.0, node = %key.1, %reason, "信令通道关闭");
}

/// 会话收尾: 摘除注册表; 摘除成功 (未被替换 / 未被踢) 时向同网广播下线。
fn teardown(state: &AppState, key: &NodeKey, gen: u64) {
    if state.online.unregister(key, gen) {
        state.online.broadcast(
            &key.0,
            SignalServerMsg::PeerOffline {
                name: key.1.clone(),
            },
            Some(&key.1),
        );
    }
}

/// 认证阶段: hello → challenge → auth, 全程限时 AUTH_TIMEOUT。
/// 失败返回原因 (客户端已尽力收到 AuthErr 的, 由调用方关闭连接)。
/// 转发面 (/ws/relay) 复用本流程, 通过后 IP 由库存权威决定。
pub(crate) async fn authenticate(
    socket: &mut WebSocket,
    state: &AppState,
    peer: SocketAddr,
) -> std::result::Result<Authenticated, String> {
    let deadline = tokio::time::Instant::now() + AUTH_TIMEOUT;

    let first = parse_frame(&next_text(socket, deadline).await?)?;
    let SignalClientMsg::Hello { network, name } = first else {
        return Err("首条消息必须是 hello".into());
    };
    let auth = {
        let store = state.lock_store();
        store
            .auth_lookup(&network, &name)
            .map_err(|e| format!("存储查询失败: {e}"))?
    };
    let Some(auth) = auth else {
        // 统一口径: 不区分不存在 / 已移除 / 属主禁用, 避免存在性探测
        try_send(
            socket,
            &SignalServerMsg::AuthErr {
                message: "节点不存在或凭证已吊销".into(),
            },
        )
        .await;
        return Err(format!("hello 拒绝: {network}/{name}"));
    };

    let nonce = token::generate_nonce();
    try_send(
        socket,
        &SignalServerMsg::Challenge {
            nonce: STANDARD.encode(nonce),
        },
    )
    .await;
    let second = parse_frame(&next_text(socket, deadline).await?)?;
    let SignalClientMsg::Auth { digest } = second else {
        return Err("第二条消息必须是 auth".into());
    };

    // 期望值: sha256(nonce || 库存凭证哈希); 库存解不出 32 字节属数据损坏
    let stored_hash = STANDARD
        .decode(&auth.credential_hash_b64)
        .map_err(|_| "库存凭证哈希损坏".to_string())?;
    let expect = STANDARD.encode(token::challenge_digest(&nonce, &stored_hash));
    if !token::ct_eq(expect.as_bytes(), digest.as_bytes()) {
        tracing::warn!(%peer, network = %network, node = %name, "信令挑战应答失败");
        try_send(
            socket,
            &SignalServerMsg::AuthErr {
                message: "凭证应答不匹配".into(),
            },
        )
        .await;
        return Err("挑战应答不匹配".into());
    }
    Ok(Authenticated {
        network,
        name,
        ip: auth.ip,
    })
}

/// 认证后的初始化推送: AuthOk (心跳参数) → 全量节点表 → 向同网其他节点广播上线。
async fn announce_and_push(
    socket: &mut WebSocket,
    state: &AppState,
    auth: &Authenticated,
) -> std::result::Result<(), String> {
    let ok = SignalServerMsg::AuthOk {
        ip: auth.ip.clone(),
        heartbeat_interval_secs: HEARTBEAT_INTERVAL_SECS,
        idle_timeout_secs: IDLE_TIMEOUT_SECS,
    };
    send_strict(socket, &ok)
        .await
        .map_err(|_| "AuthOk 发送失败".to_string())?;

    let nodes =
        build_state_nodes(state, &auth.network).map_err(|e| format!("节点表查询失败: {e}"))?;
    let self_entry = nodes.iter().find(|p| p.name == auth.name).cloned();
    // 策略以库存为准 (admin 可在节点离线期间变更, FR-10)
    let policy = state
        .lock_store()
        .node_policy(&auth.network, &auth.name)
        .map_err(|e| format!("策略查询失败: {e}"))?
        .unwrap_or(PathPolicy::Auto);
    let state_msg = SignalServerMsg::State {
        nodes,
        relays: state.relay_list(),
        policy,
        stuns: state.stuns.clone(),
    };
    send_strict(socket, &state_msg)
        .await
        .map_err(|_| "节点表发送失败".to_string())?;

    if let Some(peer_entry) = self_entry {
        // 登记先于推送, 自身条目在此刻必为在线态
        state.online.broadcast(
            &auth.network,
            SignalServerMsg::PeerOnline { peer: peer_entry },
            Some(&auth.name),
        );
    }
    Ok(())
}

/// 组装全量节点表: store 的同网节点 + 注册表的在线态与候选地址。
fn build_state_nodes(state: &AppState, network: &str) -> Result<Vec<SignalPeer>> {
    let mut nodes = state.lock_store().signal_network_nodes(network)?;
    let online = state.online.online_candidates(network);
    for n in &mut nodes {
        if let Some(c) = online.get(&n.name) {
            n.online = true;
            n.candidates = c.clone();
        }
    }
    Ok(nodes)
}

/// 策略变更定向推送 (admin 设置时调用); 节点不在线则无动作 (重连后经 State 补齐)。
pub(crate) fn push_policy(state: &AppState, network: &str, name: &str, policy: PathPolicy) {
    state
        .online
        .send_to(network, name, SignalServerMsg::Policy { policy });
}

/// 处理认证后的客户端消息; 返回 Some(原因) 表示会话结束。
async fn handle_client_msg(
    sender: &mut SplitSink<WebSocket, Message>,
    state: &AppState,
    key: &NodeKey,
    gen: u64,
    msg: Message,
) -> Option<String> {
    let text = match msg {
        Message::Text(t) => t,
        Message::Ping(p) => {
            let _ = sender.send(Message::Pong(p)).await;
            return None;
        }
        Message::Close(_) => return Some("对端关闭".into()),
        // 协议只定义文本帧; Binary / Pong 忽略
        _ => return None,
    };
    let parsed: SignalClientMsg = match serde_json::from_str(text.as_str()) {
        Ok(p) => p,
        Err(_) => {
            send_soft(
                sender,
                &SignalServerMsg::Error {
                    message: "消息格式非法".into(),
                },
            )
            .await;
            return None;
        }
    };
    match parsed {
        SignalClientMsg::Ping => {
            send_soft(sender, &SignalServerMsg::Pong).await;
        }
        SignalClientMsg::Announce { candidates } => {
            if candidates.len() > MAX_CANDIDATES
                || candidates.iter().any(|c| c.len() > MAX_CANDIDATE_LEN)
            {
                send_soft(
                    sender,
                    &SignalServerMsg::Error {
                        message: "候选地址数量或长度超限".into(),
                    },
                )
                .await;
            } else {
                state.online.set_candidates(key, gen, candidates);
                // 候选更新是打洞触发源: 以刷新条目广播同网 (不含自身),
                // 对端据此启动 / 更新并发打洞目标 (FR-2)
                if let Ok(nodes) = build_state_nodes(state, &key.0) {
                    if let Some(entry) = nodes.into_iter().find(|p| p.name == key.1) {
                        state.online.broadcast(
                            &key.0,
                            SignalServerMsg::PeerOnline { peer: entry },
                            Some(&key.1),
                        );
                    }
                }
            }
        }
        SignalClientMsg::Signal { to, payload } => {
            // 同网定向转发; 不在线或慢消费即回执发送方
            let delivered = state.online.send_to(
                &key.0,
                &to,
                SignalServerMsg::Signal {
                    from: key.1.clone(),
                    payload,
                },
            );
            if !delivered {
                send_soft(
                    sender,
                    &SignalServerMsg::SignalErr {
                        to,
                        message: "目标不在线".into(),
                    },
                )
                .await;
            }
        }
        // 已认证后到达的 hello / auth 属协议误用, 提示后忽略
        SignalClientMsg::Hello { .. } | SignalClientMsg::Auth { .. } => {
            send_soft(
                sender,
                &SignalServerMsg::Error {
                    message: "已认证, 忽略重复认证消息".into(),
                },
            )
            .await;
        }
    }
    None
}

// ------------------------------ 帧收发工具 --------------------------------

/// 取下一条文本帧 (应答协议层 Ping; Close / 非文本 / IO 错误 / 超时 → Err 原因)。
async fn next_text(
    socket: &mut WebSocket,
    deadline: tokio::time::Instant,
) -> std::result::Result<String, String> {
    loop {
        let received = tokio::time::timeout_at(deadline, socket.recv())
            .await
            .map_err(|_| "认证超时".to_string())?;
        match received {
            None => return Err("对端提前断开".into()),
            Some(Err(e)) => return Err(format!("socket: {e}")),
            Some(Ok(Message::Text(t))) => return Ok(t.as_str().to_string()),
            Some(Ok(Message::Ping(p))) => {
                socket
                    .send(Message::Pong(p))
                    .await
                    .map_err(|e| format!("socket: {e}"))?;
            }
            Some(Ok(Message::Close(_))) => return Err("对端关闭".into()),
            Some(Ok(_)) => return Err("认证期仅接受文本帧".into()),
        }
    }
}

fn parse_frame(text: &str) -> std::result::Result<SignalClientMsg, String> {
    serde_json::from_str(text).map_err(|_| "消息格式非法".to_string())
}

/// 尽力发送 (认证期): 失败由随后的收包失败兜底。
async fn try_send(socket: &mut WebSocket, msg: &SignalServerMsg) {
    // unwrap 理由: 消息均为纯数据类型, serde 序列化不会失败
    let _ = socket
        .send(Message::Text(serde_json::to_string(msg).unwrap().into()))
        .await;
}

/// 可靠发送 (初始化推送期): 失败向上传播。
async fn send_strict(
    socket: &mut WebSocket,
    msg: &SignalServerMsg,
) -> std::result::Result<(), axum::Error> {
    // unwrap 理由: 同 try_send, 纯数据消息序列化不会失败
    socket
        .send(Message::Text(serde_json::to_string(msg).unwrap().into()))
        .await
}

/// 尽力发送 (认证后, 经出站半流): 失败由随后的收包错误兜底。
async fn send_soft(sender: &mut SplitSink<WebSocket, Message>, msg: &SignalServerMsg) {
    // unwrap 理由: 同 try_send, 纯数据消息序列化不会失败
    let _ = sender
        .send(Message::Text(serde_json::to_string(msg).unwrap().into()))
        .await;
}

/// 发送注册表侧消息 (主循环): 失败表示通道死亡, 由调用方结束会话。
async fn send_frame(
    sender: &mut SplitSink<WebSocket, Message>,
    msg: &SignalServerMsg,
) -> std::result::Result<(), axum::Error> {
    // unwrap 理由: 同 try_send, 纯数据消息序列化不会失败
    sender
        .send(Message::Text(serde_json::to_string(msg).unwrap().into()))
        .await
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> (
        mpsc::Sender<SignalServerMsg>,
        mpsc::Receiver<SignalServerMsg>,
    ) {
        mpsc::channel(8)
    }

    fn got_closed(rx: &mut mpsc::Receiver<SignalServerMsg>) -> bool {
        matches!(rx.try_recv(), Ok(SignalServerMsg::Closed { .. }))
    }

    #[test]
    fn register_replaces_and_guards_unregister() {
        let reg = OnlineRegistry::new();
        let key = ("default".into(), "n1".into());
        let (tx1, mut rx1) = channel();
        let g1 = reg.register(key.clone(), tx1);
        let (tx2, _rx2) = channel();
        let g2 = reg.register(key.clone(), tx2);
        assert_ne!(g1, g2);
        // 旧连接收到 Closed, 旧代际摘除无效, 新代际摘除生效
        assert!(got_closed(&mut rx1));
        assert!(!reg.unregister(&key, g1));
        assert!(reg.unregister(&key, g2));
    }

    #[test]
    fn kick_notifies_and_broadcasts_offline() {
        let reg = OnlineRegistry::new();
        let (tx, mut rx) = channel();
        reg.register(("default".into(), "n1".into()), tx);
        let (otx, mut orx) = channel();
        reg.register(("default".into(), "n2".into()), otx);
        reg.kick("default", "n1");
        assert!(got_closed(&mut rx));
        assert!(matches!(
            orx.try_recv(),
            Ok(SignalServerMsg::PeerOffline { name }) if name == "n1"
        ));
        // 未在线时重复 kick 无动作
        reg.kick("default", "n1");
    }

    #[test]
    fn send_to_and_broadcast_are_scoped_by_network() {
        let reg = OnlineRegistry::new();
        let (tx1, mut rx1) = channel();
        reg.register(("default".into(), "n1".into()), tx1);
        let (tx2, mut rx2) = channel();
        let _ = reg.register(("net2".into(), "n1".into()), tx2);
        // 同网投递成功; 跨网同名 / 不在线目标视为不在线
        assert!(reg.send_to("default", "n1", SignalServerMsg::Pong));
        assert!(!reg.send_to("net2", "ghost", SignalServerMsg::Pong));
        assert!(matches!(rx1.try_recv(), Ok(SignalServerMsg::Pong)));
        // 广播只达同网
        reg.broadcast("default", SignalServerMsg::Pong, None);
        assert!(matches!(rx1.try_recv(), Ok(SignalServerMsg::Pong)));
        assert!(rx2.try_recv().is_err());
    }

    #[test]
    fn candidates_only_apply_to_current_generation() {
        let reg = OnlineRegistry::new();
        let key = ("default".into(), "n1".into());
        let (tx1, _rx1) = channel();
        let g1 = reg.register(key.clone(), tx1);
        let (tx2, _rx2) = channel();
        let g2 = reg.register(key.clone(), tx2);
        reg.set_candidates(&key, g1, vec!["1.2.3.4:5".into()]);
        assert!(reg
            .online_candidates("default")
            .get("n1")
            .unwrap()
            .is_empty());
        reg.set_candidates(&key, g2, vec!["1.2.3.4:5".into()]);
        assert_eq!(
            reg.online_candidates("default")["n1"],
            vec!["1.2.3.4:5".to_string()]
        );
    }
}
