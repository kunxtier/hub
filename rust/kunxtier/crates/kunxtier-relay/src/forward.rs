//! Description: 转发 WS (/ws/relay): WireGuard 密文帧按目的虚拟 IP 路由 (FR-4)。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 双模式共用同一注册表与会话循环, 差异只在准入:
//! - 核心内置: 复用信令的节点凭证挑战应答, IP 由库存权威决定, 不可声明;
//! - 卫星: 声明式 bind (零知识, NFR-5)。声明的劫持面由端到端 WG 加密兜底:
//!   被错投的帧不可解密, 注入的帧被接收方以 WG 校验失败丢弃。
//!
//! 帧 = [4 字节大端目的 IPv4][不透明密文], 未知目的静默丢弃 (UDP 语义)。

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use tokio::sync::mpsc;

use kunxtier_core::protocol::{decode_forward_frame, ForwardClientMsg, ForwardServerMsg};

use crate::ratelimit::RateLimiter;
use crate::server::AppState;
use crate::signaling::{self, HEARTBEAT_INTERVAL_SECS, IDLE_TIMEOUT_SECS};

/// 未准入连接的完成时限 (与信令一致)。
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// 单条转发帧上限: TUN 内包最大 64 KiB 量级 + WG 头部冗余。
const MAX_FORWARD_BYTES: usize = 64 * 1024 + 64;
/// 每连接出站队列深度: 满即视为慢消费者, 投递按未知目的处理 (丢弃)。
const OUTBOUND_CAPACITY: usize = 64;

// ------------------------------ 转发注册表 --------------------------------

/// 下行消息: 数据帧或服务端裁决断开。
#[derive(Debug)]
pub(crate) enum Downstream {
    Data(Vec<u8>),
    Closed(String),
}

struct Slot {
    tx: mpsc::Sender<Downstream>,
    /// 代际号: 同 IP 新连接顶替旧连接时递增, 旧会话收尾只摘除自己这一代。
    gen: u64,
}

/// 在线转发端点注册表: 虚拟 IP → 连接; 纯内存, 随进程生命周期。
pub struct ForwardRegistry {
    inner: Mutex<Vec<(Ipv4Addr, Slot)>>,
    next_gen: AtomicU64,
}

impl ForwardRegistry {
    pub fn new() -> Self {
        ForwardRegistry {
            inner: Mutex::new(Vec::new()),
            next_gen: AtomicU64::new(1),
        }
    }

    /// unwrap 理由: 锁中毒即注册表状态不可信, 快速失败优于带病转发。
    fn lock(&self) -> MutexGuard<'_, Vec<(Ipv4Addr, Slot)>> {
        self.inner.lock().unwrap()
    }

    /// 登记并顶替同 IP 旧连接: 旧连接收到 Closed 后自行退出。
    pub(crate) fn bind(&self, ip: Ipv4Addr, tx: mpsc::Sender<Downstream>) -> u64 {
        let gen = self.next_gen.fetch_add(1, Ordering::Relaxed);
        let mut map = self.lock();
        if let Some(pos) = map.iter().position(|(k, _)| *k == ip) {
            let old = map.remove(pos);
            let _ = old
                .1
                .tx
                .try_send(Downstream::Closed("同 IP 新连接已接入".into()));
        }
        map.push((ip, Slot { tx, gen }));
        gen
    }

    /// 摘除本会话代际: 已被替换时返回 false。
    pub(crate) fn unbind(&self, ip: &Ipv4Addr, gen: u64) -> bool {
        let mut map = self.lock();
        match map.iter().position(|(k, s)| *k == *ip && s.gen == gen) {
            Some(pos) => {
                map.remove(pos);
                true
            }
            None => false,
        }
    }

    /// 投递一帧: 目的未登记或慢消费返回 false (调用方按 UDP 语义丢弃)。
    pub(crate) fn route(&self, dst: &Ipv4Addr, frame: Vec<u8>) -> bool {
        let map = self.lock();
        match map.iter().find(|(k, _)| k == dst) {
            Some((_, slot)) => slot.tx.try_send(Downstream::Data(frame)).is_ok(),
            None => false,
        }
    }
}

impl Default for ForwardRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------ 卫星侧服务状态 ----------------------------

/// 卫星进程的服务状态: 无存储, 仅转发注册表与建连限速 (零知识, NFR-5)。
pub struct SatelliteState {
    pub(crate) forward: ForwardRegistry,
    /// 转发 WS 建连限速 (成功也计数): 与核心一致, 20 次 / 分钟 / 源。
    pub(crate) ws_limiter: RateLimiter,
}

impl SatelliteState {
    pub fn new() -> Self {
        SatelliteState {
            forward: ForwardRegistry::new(),
            ws_limiter: RateLimiter::new(20, Duration::from_secs(60)),
        }
    }
}

impl Default for SatelliteState {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------ 建连入口 ----------------------------------

/// 核心 /ws/relay: 挑战应答准入, IP 取自库存 (声明不可信)。
pub async fn core_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    if !state.ws_limiter.check(addr.ip()) {
        return (StatusCode::TOO_MANY_REQUESTS, "转发建连过于频繁, 稍后再试").into_response();
    }
    ws.max_message_size(MAX_FORWARD_BYTES)
        .on_upgrade(move |socket| core_session(socket, state, addr))
}

/// 卫星 /ws/relay: 声明式 bind 准入 (零知识)。
pub async fn claim_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<SatelliteState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    if !state.ws_limiter.check(addr.ip()) {
        return (StatusCode::TOO_MANY_REQUESTS, "转发建连过于频繁, 稍后再试").into_response();
    }
    ws.max_message_size(MAX_FORWARD_BYTES)
        .on_upgrade(move |socket| claim_session(socket, state, addr))
}

async fn core_session(mut socket: WebSocket, state: Arc<AppState>, peer: SocketAddr) {
    // 认证复用信令面: 失败原因只进日志, 不区分不存在 / 吊销 (防存在性探测)
    let auth = match signaling::authenticate(&mut socket, &state, peer).await {
        Ok(a) => a,
        Err(reason) => {
            tracing::info!(%peer, %reason, "转发准入未完成");
            let _ = socket.close().await;
            return;
        }
    };
    let ip: Ipv4Addr = match auth.ip.parse() {
        Ok(ip) => ip,
        Err(_) => {
            // 库存 IP 由地址分配器产出, 解析失败属数据损坏
            tracing::error!(network = %auth.network, node = %auth.name, ip = %auth.ip, "库存 IP 非法");
            let _ = socket.close().await;
            return;
        }
    };
    run_forward_loop(socket, &state.forward, ip, peer).await;
}

async fn claim_session(mut socket: WebSocket, state: Arc<SatelliteState>, peer: SocketAddr) {
    let deadline = tokio::time::Instant::now() + AUTH_TIMEOUT;
    let Some(ip) = admit_by_claim(&mut socket, deadline, peer).await else {
        return; // 已应答错误并关闭
    };
    tracing::info!(%peer, %ip, "卫星转发端点绑定");
    run_forward_loop(socket, &state.forward, ip, peer).await;
}

/// 声明式准入 (单发语义): 首条文本帧必须是合法 bind, 其余一律拒绝并关闭;
/// 返回声明的虚拟 IP。
async fn admit_by_claim(
    socket: &mut WebSocket,
    deadline: tokio::time::Instant,
    peer: SocketAddr,
) -> Option<Ipv4Addr> {
    let frame = match tokio::time::timeout_at(deadline, socket.recv()).await {
        Err(_) => {
            tracing::info!(%peer, "卫星转发准入超时");
            let _ = socket.close().await;
            return None;
        }
        Ok(None | Some(Err(_))) => {
            let _ = socket.close().await;
            return None;
        }
        Ok(Some(Ok(frame))) => frame,
    };
    match frame {
        Message::Text(t) => match serde_json::from_str::<ForwardClientMsg>(t.as_str()) {
            Ok(ForwardClientMsg::Bind { ip }) => match ip.parse::<Ipv4Addr>() {
                Ok(ip) => Some(ip),
                Err(_) => {
                    let _ = try_send_msg(
                        socket,
                        &ForwardServerMsg::Error {
                            message: "ip 不是合法的 IPv4 地址".into(),
                        },
                    )
                    .await;
                    let _ = socket.close().await;
                    None
                }
            },
            _ => {
                let _ = try_send_msg(
                    socket,
                    &ForwardServerMsg::Error {
                        message: "首条消息必须是 bind".into(),
                    },
                )
                .await;
                let _ = socket.close().await;
                None
            }
        },
        Message::Close(_) => {
            let _ = socket.close().await;
            None
        }
        _ => {
            let _ = try_send_msg(
                socket,
                &ForwardServerMsg::Error {
                    message: "准入期仅接受文本帧".into(),
                },
            )
            .await;
            let _ = socket.close().await;
            None
        }
    }
}

// ------------------------------ 会话循环 ----------------------------------

/// 准入后的统一转发循环: 上行按帧头路由, 下行投递注册表消息。
async fn run_forward_loop(
    mut socket: WebSocket,
    forward: &ForwardRegistry,
    ip: Ipv4Addr,
    peer: SocketAddr,
) {
    // 心跳参数随 Bound 下发, 客户端不硬编码 (与信令 AuthOk 同一原则)
    let bound = ForwardServerMsg::Bound {
        ip: ip.to_string(),
        heartbeat_interval_secs: HEARTBEAT_INTERVAL_SECS,
        idle_timeout_secs: IDLE_TIMEOUT_SECS,
    };
    // Bound 属初始化推送: 发送失败即放弃该端点 (与信令 send_strict 同口径)
    if try_send_msg(&mut socket, &bound).await.is_err() {
        let _ = socket.close().await;
        return;
    }

    let (tx, mut rx) = mpsc::channel::<Downstream>(OUTBOUND_CAPACITY);
    let gen = forward.bind(ip, tx);
    let (mut sender, mut receiver) = socket.split();
    let idle = Duration::from_secs(IDLE_TIMEOUT_SECS);
    let reason: String = loop {
        tokio::select! {
            biased;
            out = rx.recv() => {
                match out {
                    Some(Downstream::Data(bytes)) => {
                        if sender.send(Message::Binary(bytes.into())).await.is_err() {
                            break "发送失败".into();
                        }
                    }
                    Some(Downstream::Closed(reason)) => {
                        send_msg_text(&mut sender, &ForwardServerMsg::Closed { reason: reason.clone() }).await;
                        break "服务端断开".into();
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
                            handle_frame(&mut sender, forward, msg).await
                        {
                            break stop;
                        }
                    }
                }
            }
        }
    };
    let _ = sender.close().await;
    forward.unbind(&ip, gen);
    tracing::info!(%peer, %ip, %reason, "转发端点断开");
}

/// 处理上行帧; 返回 Some(原因) 表示会话结束。
async fn handle_frame(
    sender: &mut SplitSink<WebSocket, Message>,
    forward: &ForwardRegistry,
    msg: Message,
) -> Option<String> {
    match msg {
        Message::Binary(bytes) => {
            let frame = bytes.to_vec();
            match decode_forward_frame(&frame) {
                // 原帧整体投递 (帧头对上下行同构, 接收方以自身 IP 校验目的);
                // 未知目的 / 慢消费按 UDP 语义静默丢弃: 不回执, 不逐帧记日志
                Some((dst, _)) => {
                    let _ = forward.route(&dst, frame);
                    None
                }
                None => {
                    send_msg_text(
                        sender,
                        &ForwardServerMsg::Error {
                            message: "转发帧缺少合法头或载荷".into(),
                        },
                    )
                    .await;
                    Some("非法转发帧".into())
                }
            }
        }
        Message::Text(t) => {
            match serde_json::from_str::<ForwardClientMsg>(t.as_str()) {
                Ok(ForwardClientMsg::Ping) => {
                    send_msg_text(sender, &ForwardServerMsg::Pong).await;
                }
                Ok(ForwardClientMsg::Bind { .. }) => {
                    send_msg_text(
                        sender,
                        &ForwardServerMsg::Error {
                            message: "已绑定, 忽略重复 bind".into(),
                        },
                    )
                    .await;
                }
                Err(_) => {
                    send_msg_text(
                        sender,
                        &ForwardServerMsg::Error {
                            message: "消息格式非法".into(),
                        },
                    )
                    .await;
                }
            }
            None
        }
        Message::Ping(p) => {
            let _ = sender.send(Message::Pong(p)).await;
            None
        }
        Message::Close(_) => Some("对端关闭".into()),
        // 协议未定义的帧类型忽略
        _ => None,
    }
}

// ------------------------------ 帧收发工具 --------------------------------

/// unwrap 理由: 消息均为纯数据类型, serde 序列化不会失败。
fn to_text(msg: &ForwardServerMsg) -> String {
    serde_json::to_string(msg).unwrap()
}

/// 尽力发送 (准入期): 失败由随后的收包失败兜底。
async fn try_send_msg(
    socket: &mut WebSocket,
    msg: &ForwardServerMsg,
) -> std::result::Result<(), axum::Error> {
    socket.send(Message::Text(to_text(msg).into())).await
}

/// 尽力发送 (会话期, 经出站半流)。
async fn send_msg_text(sender: &mut SplitSink<WebSocket, Message>, msg: &ForwardServerMsg) {
    let _ = sender.send(Message::Text(to_text(msg).into())).await;
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn downstream() -> (mpsc::Sender<Downstream>, mpsc::Receiver<Downstream>) {
        mpsc::channel(8)
    }

    fn got_closed(rx: &mut mpsc::Receiver<Downstream>) -> bool {
        matches!(rx.try_recv(), Ok(Downstream::Closed(_)))
    }

    #[tokio::test]
    async fn bind_replaces_and_guards_unbind() {
        let reg = ForwardRegistry::new();
        let ip: Ipv4Addr = "100.64.0.1".parse().unwrap();
        let (tx1, mut rx1) = downstream();
        let g1 = reg.bind(ip, tx1);
        let (tx2, _rx2) = downstream();
        let g2 = reg.bind(ip, tx2);
        assert_ne!(g1, g2);
        // 旧连接收到 Closed, 旧代际摘除无效, 新代际摘除生效且不可重复
        assert!(got_closed(&mut rx1));
        assert!(!reg.unbind(&ip, g1));
        assert!(reg.unbind(&ip, g2));
        assert!(!reg.unbind(&ip, g2));
    }

    #[tokio::test]
    async fn route_hits_owner_and_misses_unknown() {
        let reg = ForwardRegistry::new();
        let a: Ipv4Addr = "100.64.0.1".parse().unwrap();
        let b: Ipv4Addr = "100.64.0.2".parse().unwrap();
        let (tx, mut rx) = downstream();
        reg.bind(a, tx);
        // 目的命中: 原帧投递 (帧头保留)
        let frame = kunxtier_core::protocol::encode_forward_frame(b, &[9, 9]);
        assert!(reg.route(&a, frame));
        match rx.try_recv() {
            Ok(Downstream::Data(bytes)) => {
                let (dst, payload) = decode_forward_frame(&bytes).unwrap();
                assert_eq!(dst, b);
                assert_eq!(payload, &[9, 9]);
            }
            other => panic!("期望数据帧, 实际: {other:?}"),
        }
        // 未登记目的: 不投递也不报错
        assert!(!reg.route(&b, vec![0; 5]));
    }
}
