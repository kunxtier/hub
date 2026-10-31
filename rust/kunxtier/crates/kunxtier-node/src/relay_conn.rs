//! Description: 中继转发 WS 客户端: 准入 (核心挑战应答 / 卫星声明) 与帧搬运会话。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 准入与服务端 forward 双模式对偶: 核心内置条目走信令同款 hello/auth
//! (IP 由库存权威决定), 卫星条目走声明式 bind (零知识)。分三层:
//! - RelayTunnel: 运行时句柄, 持上行帧通道;
//! - run: 会话级重连循环, 断开即摘除 hub 并退避重建 (帧通道复用, 断连期
//!   上行帧自然丢弃, UDP 语义); 运行时废弃通道时彻底退出;
//! - conn_loop: 单连接内的三路合流 (上行帧 / 下行帧 / 心跳)。
//!
//! 列表变化 (卫星摘除 / 令牌吊销) 由运行时废弃旧通道并按新列表重建 (FR-18)。

use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use kunxtier_core::protocol::{
    decode_forward_frame, ForwardClientMsg, ForwardServerMsg, SignalClientMsg, SignalRelay,
};
use kunxtier_core::token;

use crate::error::{Error, Result};
use crate::transport::PathHub;

/// 准入完成时限。
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// 上行帧通道容量 (慢消费由引擎侧重发兜底, 不在中继路径堆队列)。
const FRAME_CAPACITY: usize = 64;
/// 重连退避: 1s 起步 30s 封顶, 准入 / 会话成功即复位。
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// 准入所需的节点身份 (与信令共用一套凭证)。
#[derive(Debug, Clone)]
pub struct RelayIdentity {
    pub network: String,
    pub name: String,
    pub credential: String,
    pub own_vip: String,
    /// 核心 dial 地址 host:port: 内置条目 addr 为空串时以此代入。
    pub core: String,
}

type WsStream = tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>;

/// 一条中继连接: 运行时持有的句柄。
/// 上行帧入口由 hub 持有 (register_relay); 废弃通道 = abort 会话 + hub 摘除。
pub struct RelayTunnel {
    /// 列表条目 (身份比对用)。
    pub endpoint: SignalRelay,
    /// hub 侧中继号。
    pub id: u8,
    session: tokio::task::JoinHandle<()>,
}

impl RelayTunnel {
    /// 废弃通道: 终止会话任务并从 hub 摘除 (列表变化驱动, FR-18)。
    pub fn shutdown(self, hub: &PathHub) {
        self.session.abort();
        hub.unregister_relay(self.id);
    }

    /// 发起连接: 准入成功后返回; 会话任务随句柄生命周期运行。
    /// down 向运行时报断线事件 (hub 侧已摘除, 可重路由)。
    pub async fn connect(
        endpoint: SignalRelay,
        id: u8,
        identity: RelayIdentity,
        hub: Arc<PathHub>,
        down: mpsc::Sender<u8>,
    ) -> Result<Self> {
        // 首连失败直接报错 (由运行时的周期 ensure 重试), 不进会话循环
        let (ws, heartbeat_secs) = dial_and_admit(&endpoint, &identity).await?;
        let (frames_tx, frames_rx) = mpsc::channel::<Vec<u8>>(FRAME_CAPACITY);
        hub.register_relay(id, frames_tx.clone());
        let session = tokio::spawn(run(RelaySession {
            endpoint: endpoint.clone(),
            id,
            identity,
            hub,
            frames_tx,
            frames: frames_rx,
            ready: Some((ws, heartbeat_secs)),
            down,
        }));
        Ok(RelayTunnel {
            endpoint,
            id,
            session,
        })
    }
}

/// 会话的全部状态 (避免长参数列表)。
struct RelaySession {
    endpoint: SignalRelay,
    id: u8,
    identity: RelayIdentity,
    hub: Arc<PathHub>,
    frames_tx: mpsc::Sender<Vec<u8>>,
    frames: mpsc::Receiver<Vec<u8>>,
    /// 首连就绪的连接与心跳参数 (重连循环首轮消费)。
    ready: Option<(WsStream, u64)>,
    down: mpsc::Sender<u8>,
}

/// 会话级重连循环。
async fn run(mut s: RelaySession) {
    let mut backoff = BACKOFF_START;
    loop {
        let (stream, heartbeat_secs) = match s.ready.take() {
            Some(r) => r,
            None => match dial_and_admit(&s.endpoint, &s.identity).await {
                Ok(r) => {
                    backoff = BACKOFF_START;
                    r
                }
                Err(e) => {
                    tracing::warn!(
                        relay = %effective_addr(&s.endpoint, &s.identity),
                        error = %e,
                        "中继连接失败, 退避重试"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = backoff.saturating_mul(2).min(BACKOFF_MAX);
                    continue;
                }
            },
        };
        s.hub.register_relay(s.id, s.frames_tx.clone());
        match conn_loop(stream, s.id, &s.hub, &mut s.frames, heartbeat_secs).await {
            // 运行时已废弃通道: 彻底退出
            None => {
                s.hub.unregister_relay(s.id);
                return;
            }
            Some(reason) => {
                s.hub.unregister_relay(s.id);
                let _ = s.down.try_send(s.id);
                tracing::info!(
                    relay = %effective_addr(&s.endpoint, &s.identity),
                    %reason,
                    "中继会话断开, 1s 后重连"
                );
                tokio::time::sleep(BACKOFF_START).await;
            }
        }
    }
}

/// 单连接循环: 返回 Some(原因) 表示连接断开 (可重连), None 表示通道废弃。
async fn conn_loop(
    ws: WsStream,
    id: u8,
    hub: &PathHub,
    frames: &mut mpsc::Receiver<Vec<u8>>,
    heartbeat_secs: u64,
) -> Option<String> {
    let (mut sink, mut stream) = ws.split();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(heartbeat_secs.max(1)));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    loop {
        tokio::select! {
            f = frames.recv() => {
                match f {
                    Some(frame) => {
                        if sink.send(WsMessage::Binary(frame.into())).await.is_err() {
                            return Some("上行发送失败".into());
                        }
                    }
                    None => return None,
                }
            }
            _ = heartbeat.tick() => {
                if send_forward(&mut sink, &ForwardClientMsg::Ping).await.is_err() {
                    return Some("心跳发送失败".into());
                }
            }
            msg = stream.next() => {
                match msg {
                    None => return Some("对端断开".into()),
                    Some(Err(e)) => return Some(format!("接收错误: {e}")),
                    Some(Ok(WsMessage::Binary(b))) => {
                        if let Some((_dst, src, payload)) = decode_forward_frame(&b) {
                            hub.relay_inbound(id, src, payload);
                        }
                    }
                    Some(Ok(WsMessage::Text(t))) => {
                        match serde_json::from_str::<ForwardServerMsg>(&t) {
                            Ok(ForwardServerMsg::Closed { reason, .. }) => {
                                return Some(format!("服务端断开: {reason}"));
                            }
                            Ok(_) => {}
                            Err(_) => {
                                tracing::debug!("忽略无法解析的中继消息");
                            }
                        }
                    }
                    Some(Ok(WsMessage::Ping(p))) => {
                        let _ = sink.send(WsMessage::Pong(p)).await;
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

/// 建连 + 准入; 返回 (就绪的 ws, 心跳间隔秒)。
async fn dial_and_admit(
    endpoint: &SignalRelay,
    identity: &RelayIdentity,
) -> Result<(WsStream, u64)> {
    let addr = effective_addr(endpoint, identity);
    let mut ws = dial(&addr, &endpoint.fingerprint).await?;
    let heartbeat_secs = admit(&mut ws, &endpoint.kind, identity).await?;
    Ok((ws, heartbeat_secs))
}

/// 实际 dial 地址: 内置条目空地址 = 节点已连接的核心自身 (T05 语义)。
fn effective_addr(endpoint: &SignalRelay, identity: &RelayIdentity) -> String {
    if endpoint.addr.is_empty() {
        identity.core.clone()
    } else {
        endpoint.addr.clone()
    }
}

/// 指纹固定 TLS + WS 建连 (/ws/relay)。
async fn dial(dial_addr: &str, fingerprint: &str) -> Result<WsStream> {
    let connector =
        tokio_rustls::TlsConnector::from(kunxtier_core::tls::pinning_client_config(fingerprint));
    let tcp = TcpStream::connect(dial_addr)
        .await
        .map_err(|e| Error::Transport(format!("中继 tcp 连接失败: {e}")))?;
    tcp.set_nodelay(true).ok();
    let host = dial_addr.rsplit_once(':').map_or(dial_addr, |(h, _)| h);
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| Error::Transport(format!("ServerName 非法: {e}")))?;
    let tls = connector
        .connect(name, tcp)
        .await
        .map_err(|e| Error::Transport(format!("中继 tls 失败: {e}")))?;
    let (ws, _resp) = tokio_tungstenite::client_async(format!("wss://{dial_addr}/ws/relay"), tls)
        .await
        .map_err(|e| Error::Transport(format!("中继 ws 建连失败: {e}")))?;
    Ok(ws)
}

/// 准入: 核心条目走信令同款挑战应答, 卫星条目走声明式 bind;
/// 通过后等待 Bound (携带心跳参数), 返回心跳间隔秒。
async fn admit(ws: &mut WsStream, kind: &str, identity: &RelayIdentity) -> Result<u64> {
    let deadline = tokio::time::Instant::now() + AUTH_TIMEOUT;
    match kind {
        "core" => {
            send_signal(
                ws,
                SignalClientMsg::Hello {
                    network: identity.network.clone(),
                    name: identity.name.clone(),
                    version: Some(env!("CARGO_PKG_VERSION").to_string()),
                },
            )
            .await?;
            let nonce_b64 = next_admission_text(ws, deadline).await?;
            let nonce = match serde_json::from_str::<ChallengeOrErr>(&nonce_b64) {
                Ok(ChallengeOrErr::Challenge { nonce }) => nonce,
                Ok(ChallengeOrErr::AuthErr { message }) => {
                    return Err(Error::Transport(format!("中继准入被拒: {message}")));
                }
                Err(_) => return Err(Error::Transport("中继准入: 期望挑战消息".into())),
            };
            let nonce = STANDARD
                .decode(&nonce)
                .map_err(|_| Error::Transport("挑战 nonce 非法".into()))?;
            let digest = STANDARD.encode(token::challenge_answer(&nonce, &identity.credential));
            send_signal(ws, SignalClientMsg::Auth { digest }).await?;
        }
        "satellite" => {
            send_forward_text(
                ws,
                ForwardClientMsg::Bind {
                    ip: identity.own_vip.clone(),
                },
            )
            .await?;
        }
        other => return Err(Error::Config(format!("未知中继条目类型: {other}"))),
    }
    let bound = next_admission_text(ws, deadline).await?;
    match serde_json::from_str::<ForwardServerMsg>(&bound) {
        Ok(ForwardServerMsg::Bound {
            heartbeat_interval_secs,
            ..
        }) => Ok(heartbeat_interval_secs),
        Ok(ForwardServerMsg::Error { message, .. }) => {
            Err(Error::Transport(format!("中继准入失败: {message}")))
        }
        other => Err(Error::Transport(format!("期望 bound, 实际: {other:?}"))),
    }
}

/// 准入期文本帧接收 (服务端关闭 / 超时 / 非文本帧统一为失败原因)。
async fn next_admission_text(ws: &mut WsStream, deadline: tokio::time::Instant) -> Result<String> {
    match tokio::time::timeout_at(deadline, ws.next()).await {
        Err(_) => Err(Error::Transport("中继准入超时".into())),
        Ok(None) => Err(Error::Transport("中继准入被拒绝: 连接关闭".into())),
        Ok(Some(Err(e))) => Err(Error::Transport(format!("中继准入接收失败: {e}"))),
        Ok(Some(Ok(WsMessage::Text(t)))) => Ok(t.as_str().to_string()),
        Ok(Some(Ok(_))) => Err(Error::Transport("中继准入期收到非文本帧".into())),
    }
}

/// 服务端复用信令认证, 准入期只可能是挑战或拒绝。
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ChallengeOrErr {
    Challenge { nonce: String },
    AuthErr { message: String },
}

async fn send_signal(ws: &mut WsStream, msg: SignalClientMsg) -> Result<()> {
    // unwrap 理由: 消息为纯数据类型, serde 序列化不会失败
    let text = serde_json::to_string(&msg).unwrap();
    ws.send(WsMessage::Text(text.into()))
        .await
        .map_err(|e| Error::Transport(format!("中继准入发送失败: {e}")))
}

async fn send_forward_text(ws: &mut WsStream, msg: ForwardClientMsg) -> Result<()> {
    // unwrap 理由: 同 send_signal
    let text = serde_json::to_string(&msg).unwrap();
    ws.send(WsMessage::Text(text.into()))
        .await
        .map_err(|e| Error::Transport(format!("中继准入发送失败: {e}")))
}

async fn send_forward(
    sink: &mut futures_util::stream::SplitSink<WsStream, WsMessage>,
    msg: &ForwardClientMsg,
) -> Result<()> {
    // unwrap 理由: 同 send_signal
    let text = serde_json::to_string(msg).unwrap();
    sink.send(WsMessage::Text(text.into()))
        .await
        .map_err(|e| Error::Transport(format!("中继发送失败: {e}")))
}
