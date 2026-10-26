//! Description: 节点 daemon: 会话生命周期管理 (注册 / 连接 / 断开 / 重连) 与本地 IPC 服务端。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 定位: up / down / status 的常驻服务端。有 state 即自动恢复上线
//! (FR-9 服务重启恢复的前提); 运行时意外退出按指数退避重连 (起步值
//! 可配、30s 封顶, 存活满 60s 后复位), 网段冲突与致命退出不重连。
//! IP 层来源可插拔: 生产为真实 TUN 设备, 测试注入 gotatun 通道 (免特权)。
//! 令牌与节点凭证明文只在内存与 IPC 帧内流动, 不入日志 (NFR-4)。

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gotatun::tun::{IpRecv, IpSend};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot, watch};

use crate::error::{Error, Result};
use crate::events::{EventKind, EventRing};
use crate::identity::WgIdentity;
use crate::ipc::{self, IpcPeer, IpcRequest, IpcResponse, IpcStatus};
use crate::netconf;
use crate::register;
use crate::runtime::{self, NodeHandle, NodeOptions, PathTimings};
use crate::state::{self, NodeState};
use crate::tun::{TunAdapter, TunConfig};

/// 直连 persistent keepalive 秒数 (FR-15 生产值)。
const KEEPALIVE_SECS: u16 = 25;
/// 重连退避上限。
const RETRY_MAX: Duration = Duration::from_secs(30);
/// 会话存活达到该时长即视为健康, 退出后重置退避。
const HEALTHY_UPTIME: Duration = Duration::from_secs(60);
/// 单 IPC 连接的请求处理时限 (up 含注册与拨号)。
const IPC_DEADLINE: Duration = Duration::from_secs(30);
/// down 后等待运行时退出的时限。
const STOP_DEADLINE: Duration = Duration::from_secs(10);
/// verbose 事件的序列化预算: 事件环加 IpcStatus 头部须落在 IPC 单行帧 64KB 内。
const VERBOSE_EVENTS_BUDGET: usize = 56 * 1024;

/// 会话 IP 层来源。
pub trait TunFactory: Send + Sync {
    type Tx: IpSend + Send + 'static;
    type Rx: IpRecv + Send + 'static;
    /// 需与会话同生命周期的附带句柄 (通道实现持有 UDP 半边防提前关闭;
    /// 真实设备为 ())。
    type Keep: Send + 'static;

    /// 以本节点虚拟 IP 打开设备 (地址在创建时落定)。
    fn open(
        &self,
        vip: Ipv4Addr,
    ) -> impl std::future::Future<Output = Result<(Self::Tx, Self::Rx, Self::Keep)>> + Send;
}

/// 真实 TUN 装配 (需要操作系统特权: macOS root / Linux CAP_NET_ADMIN)。
pub struct TunAdapterFactory;

impl TunFactory for TunAdapterFactory {
    type Tx = TunAdapter;
    type Rx = TunAdapter;
    type Keep = ();

    async fn open(&self, vip: Ipv4Addr) -> Result<(Self::Tx, Self::Rx, Self::Keep)> {
        let adapter = TunAdapter::open(&TunConfig::new(vip))?;
        Ok((adapter.clone(), adapter, ()))
    }
}

/// daemon 装配参数。
pub struct DaemonOptions {
    pub state_dir: PathBuf,
    /// WG 固定监听端口; 缺省在首次注册时自动选空闲端口并随 state 持久化。
    pub listen_port: Option<u16>,
    /// 重连退避起步值 (指数增长至 RETRY_MAX); 测试以小值驱动。
    pub retry_base: Duration,
}

/// 会话侧上报事件 (监督任务 → daemon 主循环)。
enum SessionEvent {
    /// 运行时退出 (原因 / 存活时长 / 致命原因)。
    Exited {
        reason: String,
        uptime: Duration,
        fatal: Option<String>,
    },
    /// 退避到点, 重试连接。
    Retry,
}

/// 运行中的网络会话。
struct Session<P: TunFactory> {
    handle: NodeHandle,
    /// IP 层附带句柄, 随会话结束释放。
    _keep: Option<P::Keep>,
}

/// daemon 状态主体: 会话持有与生命周期裁决。
struct Daemon<P: TunFactory> {
    options: DaemonOptions,
    provider: P,
    /// 已注册的节点 state (离线时也保留, 供 status 展示归属与再连接)。
    node: Option<NodeState>,
    session: Option<Session<P>>,
    offline_reason: Option<String>,
    fatal: Option<String>,
    backoff: Duration,
    /// 关键事件留痕环 (跨会话复用, status --verbose 数据源)。
    events: Arc<EventRing>,
}

/// 运行 daemon: 绑定 IPC → 自动恢复 → 事件循环, 直到 shutdown 置位。
pub async fn serve<P>(
    provider: P,
    options: DaemonOptions,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()>
where
    P: TunFactory,
{
    state::ensure_state_dir(&options.state_dir)?;
    let ipc_path = ipc::socket_path(&options.state_dir);
    let listener = ipc::bind(&ipc_path).await?;
    tracing::info!(dir = %options.state_dir.display(), "daemon IPC 就绪");

    let (op_tx, op_rx) = mpsc::channel::<(IpcRequest, oneshot::Sender<IpcResponse>)>(16);
    let (ev_tx, ev_rx) = mpsc::channel::<SessionEvent>(16);
    // 接入循环随 serve 生死: daemon 退出即停收并拆除 socket 文件,
    // 否则残留 listener 会吞掉后续 daemon 的 IPC 探活与请求
    let accept_task = tokio::spawn(accept_loop(listener, op_tx));

    let mut daemon = Daemon {
        node: NodeState::load(&options.state_dir)?,
        provider,
        backoff: options.retry_base,
        options,
        session: None,
        offline_reason: None,
        fatal: None,
        events: Arc::new(EventRing::default()),
    };

    // FR-9: 有 state 即自动恢复上线 (核心暂不可达时经退避重连兜底)
    if daemon.node.is_some() {
        if let Err(e) = daemon.connect(&ev_tx).await {
            tracing::warn!(error = %e, "自动恢复上线失败, 进入退避重连");
            daemon.events.push(
                EventKind::Signal,
                daemon.node.as_ref().map(|n| n.name.as_str()),
                format!("自动恢复上线失败: {e}"),
            );
            daemon.schedule_retry(ev_tx.clone());
        }
    }

    daemon.run(op_rx, ev_rx, &ev_tx, &mut shutdown).await;

    // 收尾: 断开在途会话, 拆除 IPC 接入与 socket 文件
    if let Some(session) = daemon.session.take() {
        session.handle.stop();
        await_exit(&session.handle).await;
    }
    accept_task.abort();
    let _ = tokio::fs::remove_file(ipc_path).await;
    Ok(())
}

/// IPC 接入循环: 一连接一请求, 解码后投递 daemon 主循环并回写应答。
async fn accept_loop(
    listener: UnixListener,
    ops: mpsc::Sender<(IpcRequest, oneshot::Sender<IpcResponse>)>,
) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let ops = ops.clone();
        tokio::spawn(async move {
            let serve = async {
                let mut stream = stream;
                let Some(line) = ipc::read_frame(&mut stream).await? else {
                    return Ok(());
                };
                let req: IpcRequest = match serde_json::from_str(&line) {
                    Ok(req) => req,
                    Err(e) => {
                        let resp = IpcResponse::err(format!("请求无法解析: {e}"));
                        let _ = ipc::write_frame(&mut stream, &resp).await;
                        return Ok(());
                    }
                };
                let (tx, rx) = oneshot::channel();
                if ops.send((req, tx)).await.is_err() {
                    return Ok(());
                }
                let Ok(resp) = rx.await else {
                    return Ok(());
                };
                ipc::write_frame(&mut stream, &resp).await
            };
            // 有界处理: daemon 侧卡死不拖垮连接资源
            if tokio::time::timeout(IPC_DEADLINE, serve).await.is_err() {
                tracing::warn!("IPC 请求处理超时, 断开该连接");
            }
        });
    }
}

impl<P: TunFactory> Daemon<P> {
    /// 主循环: IPC 操作 / 会话事件 / 停机信号的串行裁决。
    async fn run(
        &mut self,
        mut ops: mpsc::Receiver<(IpcRequest, oneshot::Sender<IpcResponse>)>,
        mut events: mpsc::Receiver<SessionEvent>,
        ev_tx: &mpsc::Sender<SessionEvent>,
        shutdown: &mut watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                op = ops.recv() => {
                    let Some((req, reply)) = op else { break };
                    let resp = self.handle(req, ev_tx).await;
                    let _ = reply.send(resp);
                }
                ev = events.recv() => {
                    match ev {
                        Some(SessionEvent::Retry) => {
                            if self.session.is_none() && self.node.is_some() && self.fatal.is_none() {
                                if let Err(e) = self.connect(ev_tx).await {
                                    tracing::warn!(error = %e, "重连失败, 继续退避");
                                    self.events.push(
                                        EventKind::Signal,
                                        self.node.as_ref().map(|n| n.name.as_str()),
                                        format!("重连失败: {e}"),
                                    );
                                    self.schedule_retry(ev_tx.clone());
                                }
                            }
                        }
                        Some(SessionEvent::Exited { reason, uptime, fatal }) => {
                            self.on_exited(reason, uptime, fatal, ev_tx);
                        }
                        None => break,
                    }
                }
            }
        }
    }

    // ------------------------------ IPC 操作 ------------------------------

    async fn handle(&mut self, req: IpcRequest, ev_tx: &mpsc::Sender<SessionEvent>) -> IpcResponse {
        match req {
            IpcRequest::Up {
                token,
                name,
                core,
                fingerprint,
            } => self.handle_up(token, name, core, fingerprint, ev_tx).await,
            IpcRequest::Down => self.handle_down().await,
            IpcRequest::Status => IpcResponse::status(self.status_view()),
            IpcRequest::StatusVerbose => IpcResponse::verbose(
                self.status_view(),
                self.events.snapshot_bounded(VERBOSE_EVENTS_BUDGET),
            ),
        }
    }

    /// up: 令牌非空走注册 (首次或同名重注册 FR-17); 令牌为空表示以
    /// 既有 state 直接上线 (已注册节点的重连入口, 如排障后的恢复)。
    async fn handle_up(
        &mut self,
        token: String,
        name: String,
        core: Option<String>,
        fingerprint: Option<String>,
        ev_tx: &mpsc::Sender<SessionEvent>,
    ) -> IpcResponse {
        if self.session.is_some() {
            return IpcResponse::err("节点已在线, 如需重新注册请先 down");
        }
        if self.fatal.is_some() {
            return IpcResponse::err(format!(
                "daemon 处于致命态不重连: {} (处理冲突或删除 state 目录后重启 daemon)",
                self.fatal.clone().unwrap_or_default()
            ));
        }
        let state = match self.node.clone() {
            Some(st) => {
                self.up_existing(st, &token, &name, &core, &fingerprint)
                    .await
            }
            None => self.up_fresh(&token, &name, core, fingerprint).await,
        };
        let state = match state {
            Ok(state) => state,
            Err(message) => return IpcResponse::err(message),
        };

        if let Err(e) = state.save(&self.options.state_dir) {
            return IpcResponse::err(format!("state 落盘失败: {e}"));
        }
        self.node = Some(state);
        let (ip, network) = {
            let st = self.node.as_ref().expect("刚写入 state");
            (st.vip.to_string(), st.network.clone())
        };
        match self.connect(ev_tx).await {
            Ok(()) => IpcResponse::up(ip, network),
            Err(e) => IpcResponse::err(format!(
                "连接失败 (state 已保存, 排障后可 kunxtier up 重连): {e}"
            )),
        }
    }

    /// 已注册节点的 up: 一致性校验后, 令牌为空直接沿用 state, 非空则
    /// 同名重注册 (FR-17: 沿用身份与端口, 换发凭证)。
    async fn up_existing(
        &mut self,
        mut state: NodeState,
        token: &str,
        name: &str,
        core: &Option<String>,
        fingerprint: &Option<String>,
    ) -> std::result::Result<NodeState, String> {
        if state.name != name {
            return Err(format!(
                "state 已绑定节点名 {} (网络 {}); 更换节点名需清空 state 目录 {}",
                state.name,
                state.network,
                self.options.state_dir.display()
            ));
        }
        if let Some(core) = core {
            if normalize_core(core) != state.core {
                return Err(format!(
                    "core 与已注册核心不一致: state 为 {}, 传入 {core}",
                    state.core
                ));
            }
        }
        if let Some(fp) = fingerprint {
            if *fp != state.fingerprint {
                return Err("fingerprint 与已注册核心不一致".into());
            }
        }
        if !token.is_empty() {
            self.register(&mut state, token, name)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(state)
    }

    /// 首次 up: 生成身份、选端口并注册。
    async fn up_fresh(
        &mut self,
        token: &str,
        name: &str,
        core: Option<String>,
        fingerprint: Option<String>,
    ) -> std::result::Result<NodeState, String> {
        if token.is_empty() {
            return Err("未注册: 首次 up 必须提供一次性令牌 --token".into());
        }
        let Some(core) = core else {
            return Err("首次注册需要 --core (或 KUNXTIER_CORE)".into());
        };
        let Some(fingerprint) = fingerprint else {
            return Err("首次注册需要 --fingerprint (或 KUNXTIER_FINGERPRINT)".into());
        };
        let identity = WgIdentity::generate();
        let mut state = NodeState::from_identity(&identity);
        state.name = name.to_string();
        state.core = normalize_core(&core);
        state.fingerprint = fingerprint;
        state.listen_port = self.options.listen_port.unwrap_or_else(free_udp_port);
        self.register(&mut state, token, name)
            .await
            .map_err(|e| e.to_string())?;
        Ok(state)
    }

    /// 调用注册接口并回填 state (网络 / vip / 凭证); 同步 ureq 置于阻塞线程池。
    async fn register(&mut self, state: &mut NodeState, token: &str, name: &str) -> Result<()> {
        let identity = state.identity()?;
        let public_key = identity.public_key_base64();
        let core = state.core.clone();
        let fingerprint = state.fingerprint.clone();
        let token = token.to_string();
        let name = name.to_string();
        // 闭包移走副本, name 留给注册事件留痕
        let reg_name = name.clone();
        let outcome = match tokio::task::spawn_blocking(move || {
            register::register(&core, &fingerprint, &token, &reg_name, &public_key)
        })
        .await
        {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(e)) => {
                self.events
                    .push(EventKind::Register, Some(&name), format!("注册失败: {e}"));
                return Err(e);
            }
            Err(e) => {
                let err = Error::Transport(format!("注册任务异常: {e}"));
                self.events
                    .push(EventKind::Register, Some(&name), format!("注册失败: {err}"));
                return Err(err);
            }
        };
        self.events.push(
            EventKind::Register,
            Some(&name),
            format!("注册成功: 网络 {} IP {}", outcome.network, outcome.ip),
        );
        state.network = outcome.network;
        state.vip = outcome
            .ip
            .parse()
            .map_err(|_| Error::Config(format!("核心分配的 IP 非法: {}", outcome.ip)))?;
        state.credential = outcome.credential;
        Ok(())
    }

    async fn handle_down(&mut self) -> IpcResponse {
        let Some(session) = self.session.take() else {
            return IpcResponse::err("节点未在线");
        };
        session.handle.stop();
        await_exit(&session.handle).await;
        self.offline_reason = Some("本地断开 (down)".into());
        self.events.push(
            EventKind::Signal,
            self.node.as_ref().map(|n| n.name.as_str()),
            "本地断开 (down)",
        );
        tracing::info!(
            node = self.node.as_ref().map(|n| n.name.as_str()).unwrap_or(""),
            "节点会话已断开 (down)"
        );
        IpcResponse::ack()
    }

    // ------------------------------ 会话生命周期 ------------------------------

    /// 以当前 state 建立运行时会话: 网段冲突前置检测 → 打开 IP 层 → 启动运行时。
    async fn connect(&mut self, ev_tx: &mpsc::Sender<SessionEvent>) -> Result<()> {
        let Some(state) = self.node.clone() else {
            return Err(Error::Config("无注册 state, 请先 up".into()));
        };
        // FR-14 前置: 与运行时 Ready 事件的权威复核走同一函数, 冲突即拒绝
        let conflicts = netconf::detect_conflicts(state.vip);
        if !conflicts.is_empty() {
            let report = conflicts
                .iter()
                .map(|c| c.describe(state.vip))
                .collect::<Vec<_>>()
                .join("; ");
            let msg = format!("本地网段与虚拟网段冲突, 拒绝上线: {report}");
            self.fatal = Some(msg.clone());
            return Err(Error::Config(msg));
        }
        let (ip_tx, ip_rx, keep) = self.provider.open(state.vip).await?;
        let handle = runtime::start(
            NodeOptions {
                core: state.core.clone(),
                fingerprint: state.fingerprint.clone(),
                network: state.network.clone(),
                name: state.name.clone(),
                credential: state.credential.clone(),
                identity: state.identity()?,
                listen_port: state.listen_port,
                keepalive_secs: KEEPALIVE_SECS,
                timings: PathTimings::default(),
                events: self.events.clone(),
            },
            ip_tx,
            ip_rx,
        )
        .await?;
        let started = Instant::now();
        self.supervise(handle.clone(), started, ev_tx.clone());
        self.session = Some(Session {
            handle,
            _keep: Some(keep),
        });
        self.offline_reason = None;
        self.fatal = None;
        tracing::info!(
            node = %state.name,
            network = %state.network,
            vip = %state.vip,
            "节点会话建立"
        );
        self.events.push(
            EventKind::Signal,
            Some(&state.name),
            format!("节点会话建立 (核心 {})", state.core),
        );
        Ok(())
    }

    /// 会话监督: 运行时退出即上报 (单发)。
    fn supervise(&self, handle: NodeHandle, started: Instant, events: mpsc::Sender<SessionEvent>) {
        tokio::spawn(async move {
            let mut done = handle.done();
            loop {
                if done.borrow().is_some() {
                    break;
                }
                if done.changed().await.is_err() {
                    break;
                }
            }
            let reason = done
                .borrow()
                .clone()
                .unwrap_or_else(|| "运行时句柄释放".into());
            let fatal = handle.status().fatal;
            let _ = events
                .send(SessionEvent::Exited {
                    reason,
                    uptime: started.elapsed(),
                    fatal,
                })
                .await;
        });
    }

    /// 运行时退出裁决: down 已就地处理; 自然退出时致命态封停、其余安排重连。
    fn on_exited(
        &mut self,
        reason: String,
        uptime: Duration,
        fatal: Option<String>,
        ev_tx: &mpsc::Sender<SessionEvent>,
    ) {
        if self.session.is_none() {
            // down 已接管 (会话已取走), 只留离线原因
            return;
        }
        self.session = None;
        self.offline_reason = Some(reason.clone());
        let node_name = self.node.as_ref().map(|n| n.name.as_str());
        match fatal.as_deref() {
            Some(f) => self
                .events
                .push(EventKind::Signal, node_name, format!("会话致命退出: {f}")),
            None => self
                .events
                .push(EventKind::Signal, node_name, format!("会话退出: {reason}")),
        }
        if let Some(fatal) = fatal {
            tracing::error!(%fatal, "会话致命退出, 停止重连 (down/up 亦拒绝, 处理后重启 daemon)");
            self.fatal = Some(fatal);
            return;
        }
        if uptime >= HEALTHY_UPTIME {
            self.backoff = self.options.retry_base;
        }
        tracing::warn!(%reason, "会话退出, 安排重连");
        self.schedule_retry(ev_tx.clone());
    }

    /// 指数退避 (起步值 → 30s 封顶); 到点投递 Retry。
    fn schedule_retry(&mut self, events: mpsc::Sender<SessionEvent>) {
        let wait = self.backoff;
        self.backoff = std::cmp::min(self.backoff * 2, RETRY_MAX);
        tracing::info!(?wait, "退避后重连");
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let _ = events.send(SessionEvent::Retry).await;
        });
    }

    fn status_view(&self) -> IpcStatus {
        let node = self.node.as_ref();
        match &self.session {
            Some(session) => {
                let st = session.handle.status();
                IpcStatus {
                    online: true,
                    offline_reason: None,
                    fatal: st.fatal.clone(),
                    network: node.map(|n| n.network.clone()),
                    name: node.map(|n| n.name.clone()),
                    own_ip: st
                        .own_ip
                        .map(|ip| ip.to_string())
                        .or_else(|| node.map(|n| n.vip.to_string())),
                    policy: st.policy.map(policy_label),
                    peers: st
                        .peers
                        .iter()
                        .map(|p| IpcPeer {
                            name: p.name.clone(),
                            ip: p.ip.to_string(),
                            path: p.path.to_string(),
                            endpoint: p.endpoint.map(|e| e.to_string()),
                        })
                        .collect(),
                }
            }
            None => IpcStatus {
                online: false,
                offline_reason: self.offline_reason.clone(),
                fatal: self.fatal.clone(),
                network: node.map(|n| n.network.clone()),
                name: node.map(|n| n.name.clone()),
                own_ip: node.map(|n| n.vip.to_string()),
                policy: None,
                peers: Vec::new(),
            },
        }
    }
}

fn policy_label(policy: kunxtier_core::protocol::PathPolicy) -> String {
    match policy {
        kunxtier_core::protocol::PathPolicy::Auto => "auto".into(),
        kunxtier_core::protocol::PathPolicy::ForceRelay => "force_relay".into(),
    }
}

fn normalize_core(core: &str) -> String {
    core.trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string()
}

/// 等待运行时退出 (done 置位或通道关闭), 有界。
async fn await_exit(handle: &NodeHandle) {
    let mut done = handle.done();
    let _ = tokio::time::timeout(STOP_DEADLINE, async {
        loop {
            if done.borrow().is_some() {
                return;
            }
            if done.changed().await.is_err() {
                return;
            }
        }
    })
    .await;
}

/// 探测一个空闲 UDP 端口 (探测与实际绑定之间存在竞态, 单机可接受)。
fn free_udp_port() -> u16 {
    // expect 理由: 环回 UDP 绑定仅在线程资源耗尽时失败, 属环境级故障
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").expect("UDP 端口探测失败");
    sock.local_addr().expect("读取端口失败").port()
}
