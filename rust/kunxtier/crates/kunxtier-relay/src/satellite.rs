//! Description: 卫星中继: 核心侧注册表与 /ws/satellite, 以及卫星进程装配。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 生命周期 (FR-9 / FR-18): 卫星以可复用 relay 令牌注册并保持长连接,
//! 断连即从列表摘除并全网更新; 核心重启后卫星自动重连。
//! 卫星进程零知识 (NFR-5): 无存储, 每次启动生成临时自签证书,
//! 指纹随注册上报、经中继列表分发给节点固定。

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use kunxtier_core::config::SatelliteConfig;
use kunxtier_core::protocol::{SatelliteClientMsg, SatelliteServerMsg, SignalRelay};

use crate::cert;
use crate::forward::{claim_ws_handler, SatelliteState};
use crate::server::AppState;
use crate::{Error, Result};

/// 未注册连接的完成时限。
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// 注册通道空闲超时: 卫星 10s 心跳, 断连须秒级可见 (FR-18)。
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// 卫星心跳间隔: 与空闲超时配套, 断连须秒级可见 (FR-18)。
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
/// 注册重连退避: 1s 起步, 指数退避, 30s 封顶; 注册成功后复位。
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

// ------------------------------ 核心侧注册表 ------------------------------

/// 下行消息: 注册通道只承载服务端裁决断开 (Pong 在会话内直接应答)。
pub(crate) enum SatDown {
    Closed(String),
}

struct Slot {
    tx: mpsc::Sender<SatDown>,
    /// 代际号: 同地址新注册顶替旧连接时递增。
    gen: u64,
    token_id: i64,
    fingerprint: String,
}

/// 在线卫星注册表: 公布地址 → 连接; 纯内存 (核心重启即清空, 卫星重连重建)。
pub struct SatelliteRegistry {
    inner: Mutex<BTreeMap<SocketAddr, Slot>>,
    next_gen: AtomicU64,
}

impl SatelliteRegistry {
    pub fn new() -> Self {
        SatelliteRegistry {
            inner: Mutex::new(BTreeMap::new()),
            next_gen: AtomicU64::new(1),
        }
    }

    /// unwrap 理由: 锁中毒即注册表状态不可信, 快速失败优于带病分发。
    fn lock(&self) -> MutexGuard<'_, BTreeMap<SocketAddr, Slot>> {
        self.inner.lock().unwrap()
    }

    /// 登记并顶替同地址旧连接; 返回代际号。
    pub(crate) fn register(
        &self,
        advertise: SocketAddr,
        token_id: i64,
        fingerprint: String,
        tx: mpsc::Sender<SatDown>,
    ) -> u64 {
        let gen = self.next_gen.fetch_add(1, Ordering::Relaxed);
        let mut map = self.lock();
        if let Some(old) = map.insert(
            advertise,
            Slot {
                tx,
                gen,
                token_id,
                fingerprint,
            },
        ) {
            let _ = old
                .tx
                .try_send(SatDown::Closed("同地址新注册已接入".into()));
        }
        gen
    }

    /// 摘除本会话代际: 已被替换时返回 false。
    pub fn unregister(&self, advertise: &SocketAddr, gen: u64) -> bool {
        let mut map = self.lock();
        match map.get(advertise) {
            Some(slot) if slot.gen == gen => {
                map.remove(advertise);
                true
            }
            _ => false,
        }
    }

    /// 按令牌踢线 (relay 令牌撤销): 返回被摘除的连接数。
    pub fn kick_by_token(&self, token_id: i64) -> usize {
        let mut map = self.lock();
        let hits: Vec<SocketAddr> = map
            .iter()
            .filter(|(_, s)| s.token_id == token_id)
            .map(|(k, _)| *k)
            .collect();
        for k in &hits {
            if let Some(slot) = map.remove(k) {
                let _ = slot
                    .tx
                    .try_send(SatDown::Closed("relay 令牌已被撤销".into()));
            }
        }
        hits.len()
    }

    /// 卫星条目快照 (BTreeMap 序保证列表稳定)。
    pub fn list(&self) -> Vec<SignalRelay> {
        self.lock()
            .iter()
            .map(|(addr, s)| SignalRelay {
                addr: addr.to_string(),
                kind: "satellite".into(),
                fingerprint: s.fingerprint.clone(),
            })
            .collect()
    }
}

impl Default for SatelliteRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------ 核心侧 /ws/satellite ----------------------

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    if !state.ws_limiter.check(addr.ip()) {
        return (StatusCode::TOO_MANY_REQUESTS, "卫星建连过于频繁, 稍后再试").into_response();
    }
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| registration_session(socket, state, addr))
}

/// 卫星注册会话: register → Registered → 心跳长连接; 断开即摘除并全网更新列表。
async fn registration_session(mut socket: WebSocket, state: Arc<AppState>, peer: SocketAddr) {
    let deadline = tokio::time::Instant::now() + AUTH_TIMEOUT;
    let Some((advertise, token_id, fingerprint)) =
        admit_registration(&mut socket, &state, deadline, peer).await
    else {
        return; // 已应答错误并关闭
    };

    let (tx, mut rx) = mpsc::channel::<SatDown>(8);
    let gen = state
        .satellites
        .register(advertise, token_id, fingerprint, tx);
    // 注意: 不记录令牌内容 (NFR-4)
    tracing::info!(%peer, advertise = %advertise, "卫星注册完成");
    // unwrap 理由: 纯数据消息序列化不会失败
    if socket
        .send(Message::Text(
            serde_json::to_string(&SatelliteServerMsg::Registered)
                .unwrap()
                .into(),
        ))
        .await
        .is_err()
    {
        state.satellites.unregister(&advertise, gen);
        let _ = socket.close().await;
        return;
    }
    state.broadcast_relays();

    let (mut sender, mut receiver) = socket.split();
    let reason: String = loop {
        tokio::select! {
            biased;
            out = rx.recv() => {
                match out {
                    Some(SatDown::Closed(reason)) => {
                        // unwrap 理由: 同上
                        let _ = sender
                            .send(Message::Text(
                                serde_json::to_string(&SatelliteServerMsg::Closed {
                                    reason: reason.clone(),
                                })
                                .unwrap()
                                .into(),
                            ))
                            .await;
                        break "服务端断开".into();
                    }
                    None => break "注册表通道关闭".into(),
                }
            }
            received = tokio::time::timeout(IDLE_TIMEOUT, receiver.next()) => {
                match received {
                    Err(_) => break "空闲超时".into(),
                    Ok(None) => break "对端断开".into(),
                    Ok(Some(Err(e))) => break format!("接收错误: {e}"),
                    Ok(Some(Ok(msg))) => match msg {
                        Message::Text(t) => {
                            match serde_json::from_str::<SatelliteClientMsg>(t.as_str()) {
                                Ok(SatelliteClientMsg::Ping) => {
                                    // unwrap 理由: 纯数据消息序列化不会失败
                                    let _ = sender
                                        .send(Message::Text(
                                            serde_json::to_string(&SatelliteServerMsg::Pong)
                                                .unwrap()
                                                .into(),
                                        ))
                                        .await;
                                }
                                _ => {
                                    let _ = sender
                                        .send(Message::Text(
                                            serde_json::to_string(&SatelliteServerMsg::Error {
                                                message: "已注册, 忽略重复 register".into(),
                                            })
                                            .unwrap()
                                            .into(),
                                        ))
                                        .await;
                                }
                            }
                        }
                        Message::Ping(p) => {
                            let _ = sender.send(Message::Pong(p)).await;
                        }
                        Message::Close(_) => break "对端关闭".into(),
                        _ => {}
                    },
                }
            }
        }
    };
    let _ = sender.close().await;
    if state.satellites.unregister(&advertise, gen) {
        state.broadcast_relays();
    }
    tracing::info!(%peer, advertise = %advertise, %reason, "卫星注册通道断开");
}

/// 注册准入 (单发语义): 首条文本帧必须是合法 register, 令牌 / 地址 / 指纹
/// 逐项校验, 任一失败即拒绝并关闭; 通过则返回 (公布地址, 令牌 id, 指纹)。
async fn admit_registration(
    socket: &mut WebSocket,
    state: &AppState,
    deadline: tokio::time::Instant,
    peer: SocketAddr,
) -> Option<(SocketAddr, i64, String)> {
    let frame = match tokio::time::timeout_at(deadline, socket.recv()).await {
        Err(_) => {
            tracing::info!(%peer, "卫星注册超时");
            let _ = socket.close().await;
            return None;
        }
        Ok(None | Some(Err(_))) => {
            let _ = socket.close().await;
            return None;
        }
        Ok(Some(Ok(frame))) => frame,
    };
    let Message::Text(t) = frame else {
        let _ = socket.close().await;
        return None;
    };
    let Ok(SatelliteClientMsg::Register {
        token,
        advertise,
        fingerprint,
    }) = serde_json::from_str(t.as_str())
    else {
        reject(socket, "首条消息必须是 register").await;
        return None;
    };
    // 令牌校验失败与不存在同口径, 不区分原因 (防枚举, NFR-8);
    // 校验在锁内完成, 锁释放后再进入异步应答路径
    let verified = {
        let store = state.lock_store();
        store.verify_relay_token(&token)
    };
    let token_id = match verified {
        Ok(Some(id)) => id,
        _ => {
            tracing::warn!(%peer, "卫星注册令牌无效");
            reject(socket, "relay 令牌无效或已失效").await;
            return None;
        }
    };
    let Ok(advertise) = advertise.parse::<SocketAddr>() else {
        reject(socket, "advertise 不是合法的 SocketAddr").await;
        return None;
    };
    if !valid_fingerprint(&fingerprint) {
        reject(socket, "fingerprint 必须为 64 位十六进制").await;
        return None;
    }
    Some((advertise, token_id, fingerprint))
}

async fn reject(socket: &mut WebSocket, message: &str) {
    // unwrap 理由: 纯数据消息序列化不会失败
    let _ = socket
        .send(Message::Text(
            serde_json::to_string(&SatelliteServerMsg::Error {
                message: message.into(),
            })
            .unwrap()
            .into(),
        ))
        .await;
    let _ = socket.close().await;
}

/// 指纹格式: sha256 的小写/大写十六进制 64 位 (与 cert::fingerprint 一致)。
fn valid_fingerprint(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

// ------------------------------ 卫星进程 ----------------------------------

/// 已启动的卫星进程句柄: addr 为实际监听地址 (listen 端口为 0 时由内核分配)。
pub struct Satellite {
    pub addr: SocketAddr,
    /// 服务任务 (监听 + 转发)。
    serve: tokio::task::JoinHandle<Result<()>>,
    registration: tokio::task::JoinHandle<()>,
    server: axum_server::Handle,
}

impl Satellite {
    /// 前台运行至服务终止 (CLI 入口)。
    pub async fn run(self) -> Result<()> {
        // unwrap 理由: JoinError 仅在任务 panic / 被终止时出现, 均需按错误显式暴露
        let result = self
            .serve
            .await
            .unwrap_or_else(|e| Err(Error::Serve(format!("卫星服务任务失败: {e}"))));
        self.registration.abort();
        result
    }

    /// 立即终止 (测试收尾 / 异常退出路径)。
    pub fn kill(&self) {
        self.server.graceful_shutdown(None);
        self.registration.abort();
    }
}

/// 装配并启动卫星: 本地 TLS 转发监听 + 面向核心的注册长连接 (断线自动重连)。
/// 无状态目录: 证书每次启动临时生成, 指纹经注册通道公布。
pub async fn start(cfg: &SatelliteConfig) -> Result<Satellite> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let ck = cert::generate_self_signed().map_err(|e| Error::Cert(e.to_string()))?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        ck.cert_pem.clone().into_bytes(),
        ck.key_pem.clone().into_bytes(),
    )
    .await
    .map_err(|e| Error::Cert(format!("卫星证书装配失败: {e}")))?;

    let sat_state = Arc::new(SatelliteState::new());
    let router = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/ws/relay", get(claim_ws_handler))
        .with_state(sat_state)
        .into_make_service_with_connect_info::<SocketAddr>();

    let listener = std::net::TcpListener::bind(cfg.listen)?;
    let addr = listener.local_addr()?;
    let registration = tokio::spawn(registration_loop(cfg.clone(), ck.fingerprint.clone()));
    let server = axum_server::Handle::new();
    let handle = server.clone();
    let serve = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .handle(handle)
            .serve(router)
            .await
            .map_err(|e| Error::Serve(format!("{e}")))
    });
    tracing::info!(
        listen = %addr,
        advertise = %cfg.advertise,
        core = %cfg.core,
        "卫星中继启动"
    );
    Ok(Satellite {
        addr,
        serve,
        registration,
        server,
    })
}

/// CLI 前台入口。
pub async fn run(cfg: &SatelliteConfig) -> Result<()> {
    start(cfg).await?.run().await
}

/// 注册长连接循环: 退出仅在监听器终止时发生; 令牌无效等确定性失败
/// 也保持重试 (核心重启 / 令牌补签后自愈), 退避封顶 30s。
async fn registration_loop(cfg: SatelliteConfig, fingerprint: String) {
    let mut backoff = BACKOFF_START;
    loop {
        match register_once(&cfg, &fingerprint).await {
            Ok(()) => backoff = BACKOFF_START,
            Err(e) => tracing::warn!(error = %e, "卫星注册未建立"),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// 完成一次注册会话; Ok 表示注册成功后的会话期结束 (退避复位),
/// Err 表示未能注册 (拨号 / 令牌 / 握手失败)。
async fn register_once(
    cfg: &SatelliteConfig,
    fingerprint: &str,
) -> std::result::Result<(), String> {
    let tls_cfg = kunxtier_core::tls::pinning_client_config(&cfg.fingerprint);
    let connector = tokio_rustls::TlsConnector::from(tls_cfg);
    let tcp = TcpStream::connect(&cfg.core)
        .await
        .map_err(|e| format!("tcp: {e}"))?;
    // authority 取主机部分作 ServerName (IP 或域名均可)
    let host = cfg
        .core
        .rsplit_once(':')
        .map_or(cfg.core.as_str(), |(h, _)| h);
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| format!("ServerName: {e}"))?;
    let tls = connector
        .connect(name, tcp)
        .await
        .map_err(|e| format!("tls: {e}"))?;
    let (mut ws, _resp) =
        tokio_tungstenite::client_async(format!("wss://{}/ws/satellite", cfg.core), tls)
            .await
            .map_err(|e| format!("ws: {e}"))?;

    let register = SatelliteClientMsg::Register {
        token: cfg.token.clone(),
        advertise: cfg.advertise.to_string(),
        fingerprint: fingerprint.to_string(),
    };
    // unwrap 理由: 纯数据消息序列化不会失败
    ws.send(WsMessage::Text(
        serde_json::to_string(&register).unwrap().into(),
    ))
    .await
    .map_err(|e| format!("发送注册: {e}"))?;

    let accepted = tokio::time::timeout(AUTH_TIMEOUT, ws.next()).await;
    match accepted {
        Err(_) => return Err("等待注册应答超时".into()),
        Ok(None) => return Err("注册通道提前断开".into()),
        Ok(Some(Err(e))) => return Err(format!("注册通道错误: {e}")),
        Ok(Some(Ok(WsMessage::Text(t)))) => {
            let resp: SatelliteServerMsg =
                serde_json::from_str(t.as_str()).map_err(|_| "注册应答格式非法".to_string())?;
            match resp {
                SatelliteServerMsg::Registered => {}
                SatelliteServerMsg::Error { message } => {
                    return Err(format!("核心拒绝注册: {message}"))
                }
                _ => return Err("注册应答非预期".into()),
            }
        }
        Ok(Some(Ok(_))) => return Err("注册应答必须为文本帧".into()),
    }
    tracing::info!(core = %cfg.core, advertise = %cfg.advertise, "卫星注册成功");

    // 注册成功: 心跳维持, 断开 / 超时即返回由外层重连
    let (mut sink, mut stream) = ws.split();
    let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                // unwrap 理由: 纯数据消息序列化不会失败
                let ping = serde_json::to_string(&SatelliteClientMsg::Ping).unwrap();
                if sink.send(WsMessage::Text(ping.into())).await.is_err() {
                    return Ok(());
                }
            }
            received = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => {
                match received {
                    Err(_) => return Ok(()),
                    Ok(None | Some(Err(_))) => return Ok(()),
                    Ok(Some(Ok(WsMessage::Ping(p)))) => {
                        let _ = sink.send(WsMessage::Pong(p)).await;
                    }
                    Ok(Some(Ok(WsMessage::Close(_)))) => return Ok(()),
                    Ok(Some(Ok(_))) => {}
                }
            }
        }
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> (mpsc::Sender<SatDown>, mpsc::Receiver<SatDown>) {
        mpsc::channel(8)
    }

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn register_replaces_and_unregister_guards() {
        let reg = SatelliteRegistry::new();
        let a = addr("1.2.3.4:443");
        let (tx1, mut rx1) = channel();
        let g1 = reg.register(a, 1, "f1".into(), tx1);
        let (tx2, _rx2) = channel();
        let g2 = reg.register(a, 2, "f2".into(), tx2);
        // 旧连接被顶替: 收到 Closed, 旧代际摘除无效
        assert!(matches!(rx1.try_recv(), Ok(SatDown::Closed(_))));
        assert!(!reg.unregister(&a, g1));
        assert!(reg.unregister(&a, g2));
        assert!(reg.list().is_empty());
    }

    #[tokio::test]
    async fn list_orders_by_addr_and_keeps_fingerprint() {
        let reg = SatelliteRegistry::new();
        let (tx, _rx) = channel();
        reg.register(addr("5.6.7.8:443"), 7, "fb".into(), tx);
        let (tx, _rx) = channel();
        reg.register(addr("1.2.3.4:443"), 8, "fa".into(), tx);
        let list = reg.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].addr, "1.2.3.4:443");
        assert_eq!(list[0].kind, "satellite");
        assert_eq!(list[0].fingerprint, "fa");
    }

    #[tokio::test]
    async fn kick_by_token_only_hits_owner() {
        let reg = SatelliteRegistry::new();
        let a = addr("1.2.3.4:443");
        let b = addr("5.6.7.8:443");
        let (tx1, mut rx1) = channel();
        reg.register(a, 11, "f1".into(), tx1);
        let (tx2, _rx2) = channel();
        reg.register(b, 22, "f2".into(), tx2);
        assert_eq!(reg.kick_by_token(11), 1);
        assert!(matches!(rx1.try_recv(), Ok(SatDown::Closed(_))));
        // 未命中的令牌无动作
        assert_eq!(reg.kick_by_token(11), 0);
        assert_eq!(reg.list().len(), 1);
        assert_eq!(reg.list()[0].addr, "5.6.7.8:443");
    }

    #[test]
    fn fingerprint_format_is_validated() {
        assert!(valid_fingerprint(&"ab".repeat(32)));
        assert!(!valid_fingerprint(&"ab".repeat(31)));
        assert!(!valid_fingerprint(&"zz".repeat(32)));
    }
}
