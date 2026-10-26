//! Description: 信令 WS 客户端: 挑战应答认证、事件流与命令通道。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 认证与服务端 signaling::authenticate 对偶: hello → challenge →
//! sha256(nonce || sha256(凭证)) 应答, 凭证明文与哈希均不出本机。
//! 会话循环只做协议编解码与转发: 服务端消息译成 NodeEvent 交运行时,
//! 运行时经 SignalCmd 下发候选地址与心跳。

use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use kunxtier_core::protocol::{
    PathPolicy, PeerPathReport, SignalClientMsg, SignalPeer, SignalRelay, SignalServerMsg,
};
use kunxtier_core::token;

use crate::error::{Error, Result};

/// 未认证连接的完成时限 (与服务端一致)。
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// 核心拨号时限: 拨号发生在 daemon 主循环内, 必须有界——黑洞地址 (不回
/// RST) 的 OS 级重试可达分钟级, 无界会把 daemon 的 IPC 一并卡死
/// (同 runtime::RELAY_CONNECT_TIMEOUT 的裁决)。
const DIAL_TIMEOUT: Duration = Duration::from_secs(8);
/// 命令通道容量 (心跳 / 候选上报均为低频)。
const CMD_CAPACITY: usize = 32;

/// 连接参数。
#[derive(Debug, Clone)]
pub struct SignalOptions {
    /// 核心地址 host:port。
    pub core: String,
    /// 核心证书 sha256 指纹 (hex)。
    pub fingerprint: String,
    pub network: String,
    pub name: String,
    /// 节点凭证明文 (仅本地使用)。
    pub credential: String,
}

/// 服务端事件 → 运行时的归一化视图。
#[derive(Debug)]
pub enum NodeEvent {
    /// 认证通过: 本节点 IP 与心跳参数。
    Ready {
        ip: String,
        heartbeat_secs: u64,
    },
    State {
        nodes: Vec<SignalPeer>,
        relays: Vec<SignalRelay>,
        policy: PathPolicy,
        stuns: Vec<String>,
    },
    PeerOnline(SignalPeer),
    PeerOffline {
        name: String,
    },
    Relays(Vec<SignalRelay>),
    Policy(PathPolicy),
    /// 服务端裁决断开 / 通道错误 (原因)。
    Closed(String),
}

/// 运行时 → 会话的命令。
#[derive(Debug)]
pub enum SignalCmd {
    /// 上报候选地址 (打洞用)。
    Announce(Vec<String>),
    /// 应用层心跳。
    Ping,
    /// 上报逐对端路径快照 (控制台总览与中继计数数据源)。
    ReportPaths(Vec<PeerPathReport>),
}

/// 运行中的信令会话句柄。
pub struct SignalClient {
    pub events: mpsc::Receiver<NodeEvent>,
    pub cmd: mpsc::Sender<SignalCmd>,
}

type TlsWs = tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>;

impl SignalClient {
    /// 建连并完成认证; 返回 (句柄, 会话任务)。
    /// 任务由调用方 spawn, 断开时向 events 发 Closed 后结束。
    pub async fn connect(options: SignalOptions) -> Result<(Self, tokio::task::JoinHandle<()>)> {
        let (event_tx, events) = mpsc::channel(64);
        let (cmd_tx, cmd_rx) = mpsc::channel(CMD_CAPACITY);
        let ws = dial(&options).await?;
        let task = tokio::spawn(session(ws, options, event_tx, cmd_rx));
        Ok((
            SignalClient {
                events,
                cmd: cmd_tx,
            },
            task,
        ))
    }

    /// 命令尽力下发 (会话已断时静默失败, 由 Closed 事件兜底)。
    pub async fn announce(&self, candidates: Vec<String>) {
        let _ = self.cmd.send(SignalCmd::Announce(candidates)).await;
    }
}

/// 指纹固定 TLS + WS 建连。
async fn dial(options: &SignalOptions) -> Result<TlsWs> {
    let connector = tokio_rustls::TlsConnector::from(kunxtier_core::tls::pinning_client_config(
        &options.fingerprint,
    ));
    let tcp = tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect(&options.core))
        .await
        .map_err(|_| Error::Transport("核心中继 tcp 连接超时".into()))?
        .map_err(|e| Error::Transport(format!("核心中继 tcp 连接失败: {e}")))?;
    tcp.set_nodelay(true).ok();
    let host = options
        .core
        .rsplit_once(':')
        .map_or(options.core.as_str(), |(h, _)| h);
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| Error::Transport(format!("ServerName 非法: {e}")))?;
    let tls = connector
        .connect(name, tcp)
        .await
        .map_err(|e| Error::Transport(format!("核心中继 tls 失败: {e}")))?;
    let (ws, _resp) =
        tokio_tungstenite::client_async(format!("wss://{}/ws/signal", options.core), tls)
            .await
            .map_err(|e| Error::Transport(format!("信令 ws 建连失败: {e}")))?;
    Ok(ws)
}

/// 认证 + 会话循环。
async fn session(
    mut ws: TlsWs,
    options: SignalOptions,
    events: mpsc::Sender<NodeEvent>,
    mut cmd: mpsc::Receiver<SignalCmd>,
) {
    let outcome = run_session(&mut ws, &options, &events, &mut cmd).await;
    let reason = match outcome {
        Ok(reason) => reason,
        Err(e) => e.to_string(),
    };
    let _ = events.send(NodeEvent::Closed(reason.clone())).await;
    let _ = ws.close(None).await;
}

async fn run_session(
    ws: &mut TlsWs,
    options: &SignalOptions,
    events: &mpsc::Sender<NodeEvent>,
    cmd: &mut mpsc::Receiver<SignalCmd>,
) -> Result<String> {
    authenticate(ws, options).await?;

    let (mut sink, mut stream) = ws.split();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // interval 首个 tick 立即到期, 先消费掉
    heartbeat.tick().await;
    let reason: String = loop {
        tokio::select! {
            c = cmd.recv() => {
                match c {
                    Some(SignalCmd::Announce(candidates)) => {
                        if send_client(&mut sink, &SignalClientMsg::Announce { candidates })
                            .await
                            .is_err()
                        {
                            break "发送失败".into();
                        }
                    }
                    Some(SignalCmd::Ping) => {
                        if send_client(&mut sink, &SignalClientMsg::Ping).await.is_err() {
                            break "发送失败".into();
                        }
                    }
                    Some(SignalCmd::ReportPaths(paths)) => {
                        if send_client(&mut sink, &SignalClientMsg::PathReport { paths })
                            .await
                            .is_err()
                        {
                            break "发送失败".into();
                        }
                    }
                    None => break "命令通道关闭".into(),
                }
            }
            _ = heartbeat.tick() => {
                if send_client(&mut sink, &SignalClientMsg::Ping).await.is_err() {
                    break "发送失败".into();
                }
            }
            msg = stream.next() => {
                match msg {
                    None => break "对端断开".into(),
                    Some(Err(e)) => break format!("接收错误: {e}"),
                    Some(Ok(WsMessage::Text(t))) => {
                        match serde_json::from_str::<SignalServerMsg>(&t) {
                            Ok(server_msg) => {
                                let server_close =
                                    matches!(server_msg, SignalServerMsg::Closed { .. });
                                if let Some(ev) = translate(server_msg) {
                                    if events.send(ev).await.is_err() {
                                        break "事件通道关闭".into();
                                    }
                                }
                                if server_close {
                                    break "服务端断开".into();
                                }
                            }
                            Err(_) => tracing::warn!("忽略无法解析的信令消息"),
                        }
                    }
                    Some(Ok(WsMessage::Ping(p))) => {
                        let _ = sink.send(WsMessage::Pong(p)).await;
                    }
                    Some(Ok(WsMessage::Close(_))) => break "对端关闭".into(),
                    Some(Ok(_)) => {}
                }
            }
        }
    };
    Ok(reason)
}

/// 认证握手: hello → challenge → auth; 服务端消息顺序由协议固定。
async fn authenticate(ws: &mut TlsWs, options: &SignalOptions) -> Result<()> {
    let deadline = tokio::time::Instant::now() + AUTH_TIMEOUT;

    send_client(
        &mut *ws,
        &SignalClientMsg::Hello {
            network: options.network.clone(),
            name: options.name.clone(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        },
    )
    .await?;

    let nonce_b64 = match tokio::time::timeout_at(deadline, ws.next()).await {
        Err(_) => return Err(Error::Transport("认证超时: 未收到挑战".into())),
        Ok(None) => return Err(Error::Transport("认证中断: 连接关闭".into())),
        Ok(Some(Err(e))) => return Err(Error::Transport(format!("认证接收失败: {e}"))),
        Ok(Some(Ok(WsMessage::Text(t)))) => match serde_json::from_str::<SignalServerMsg>(&t) {
            Ok(SignalServerMsg::Challenge { nonce }) => nonce,
            Ok(SignalServerMsg::AuthErr { message }) => {
                return Err(Error::Transport(format!("认证被拒: {message}")));
            }
            other => return Err(Error::Transport(format!("期望挑战, 实际: {other:?}"))),
        },
        Ok(Some(Ok(_))) => return Err(Error::Transport("认证期收到非文本帧".into())),
    };
    let nonce = STANDARD
        .decode(&nonce_b64)
        .map_err(|_| Error::Transport("挑战 nonce 非法".into()))?;
    let digest = STANDARD.encode(token::challenge_answer(&nonce, &options.credential));
    send_client(&mut *ws, &SignalClientMsg::Auth { digest }).await?;
    Ok(())
}

/// 服务端消息 → 运行时事件 (心跳应答等无需上抛的消息返回 None)。
fn translate(msg: SignalServerMsg) -> Option<NodeEvent> {
    match msg {
        SignalServerMsg::AuthOk {
            ip,
            heartbeat_interval_secs,
            ..
        } => Some(NodeEvent::Ready {
            ip,
            heartbeat_secs: heartbeat_interval_secs,
        }),
        SignalServerMsg::State {
            nodes,
            relays,
            policy,
            stuns,
        } => Some(NodeEvent::State {
            nodes,
            relays,
            policy,
            stuns,
        }),
        SignalServerMsg::PeerOnline { peer } => Some(NodeEvent::PeerOnline(peer)),
        SignalServerMsg::PeerOffline { name } => Some(NodeEvent::PeerOffline { name }),
        SignalServerMsg::Relays { relays } => Some(NodeEvent::Relays(relays)),
        SignalServerMsg::Policy { policy } => Some(NodeEvent::Policy(policy)),
        SignalServerMsg::Closed { reason } => Some(NodeEvent::Closed(reason)),
        // 认证期消息已在握手处消费; Pong / Error / SignalErr 不驱动节点状态机
        _ => None,
    }
}

async fn send_client<S>(mut sink: S, msg: &SignalClientMsg) -> Result<()>
where
    S: futures_util::Sink<WsMessage> + Unpin,
    S::Error: std::fmt::Display,
{
    // unwrap 理由: 消息为纯数据类型, serde 序列化不会失败
    let text = serde_json::to_string(msg).unwrap();
    sink.send(WsMessage::Text(text.into()))
        .await
        .map_err(|e| Error::Transport(format!("信令发送失败: {e}")))
}
