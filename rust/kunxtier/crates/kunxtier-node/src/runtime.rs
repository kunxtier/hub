//! Description: 节点运行时: 引擎 / 信令 / STUN / 打洞 / 中继通道的装配与事件循环。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 职责分层: 本模块只做事件收集与状态推进 (State / Event / Action), 协议细节
//! 分属 transport / stun / punch / relay_conn / signal / netconf 各模块。
//! 路径策略两档 (FR-10): Auto = 候选到达即并发打洞, 失败回落中继并周期重试;
//! ForceRelay = 跳过打洞直接伪端点。直连与中继的切换以引擎端点漂移为底,
//! T08 补齐活性观测 (Direct 周期 Ping / 失活判定)、周期重试直连与
//! 重试期间的 fallback 保活 (Punching 携带既有路径, 流量不断)。

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gotatun::tun::{IpRecv, IpSend};
use gotatun::x25519::PublicKey;
use tokio::sync::{mpsc, watch};

use kunxtier_core::protocol::{PathPolicy, PeerPathReport, SignalPeer, SignalRelay};
use kunxtier_core::time::elapsed_ms;

use crate::engine::{build_engine_split, EngineOptions, PeerSpec, WgEngine};
use crate::error::{Error, Result};
use crate::events::{EventKind, EventRecord, EventRing};
use crate::identity::WgIdentity;
use crate::netconf;
use crate::punch::{decode_probe, encode_probe, ProbeKind, PunchState, PunchToken};
use crate::relay_conn::{RelayIdentity, RelayTunnel};
use crate::signal::{NodeEvent, SignalClient, SignalCmd, SignalOptions};
use crate::stun;
use crate::transport::{PathHub, PathStreams, PathUdpFactory, StunEvent};

/// 打洞探测突发间隔。
const PROBE_INTERVAL: Duration = Duration::from_millis(250);
/// 打洞期的 keepalive 收紧值: 端点学到后驱动 WG 握手尽快落地,
/// 使 "观测到 WG 流量" 的直连确认落在打洞窗口内 (常态值见 NodeOptions)。
const PUNCH_KEEPALIVE_SECS: u16 = 1;
/// 中继通道对齐周期 (建连 / 重连 / 列表变化收敛)。
const RELAY_ENSURE_INTERVAL: Duration = Duration::from_secs(5);
/// 中继号上限 (240 伪端点编码一个字节)。
const MAX_RELAYS: usize = 254;
/// 中继建连时限: 拨号发生在主事件循环内, 必须有界,
/// 黑洞地址 (不回 RST) 才不会拖垮打洞节拍与信令事件。
const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// 路径上报周期兜底: 转换事件即时上报之外的全量校准节拍。
const PATH_REPORT_INTERVAL: Duration = Duration::from_secs(60);

/// 路径管理节奏 (T08)。生产默认值面向个人规模; 集成测试以小值驱动状态机。
#[derive(Debug, Clone)]
pub struct PathTimings {
    /// Direct 态保活 Ping 周期: 应用层活性观测, 是失活判定的证据来源。
    pub ping: Duration,
    /// 直连失活阈值: 超过该时长无对端探测 / WG 流量证据即判死, 回落中继。
    pub dead_after: Duration,
    /// Relayed 态直连重试间隔 (自动策略)。
    pub retry: Duration,
    /// 打洞窗口: 到期未通即回落 (FR-3)。
    pub punch_window: Duration,
}

impl Default for PathTimings {
    fn default() -> Self {
        PathTimings {
            ping: Duration::from_secs(15),
            dead_after: Duration::from_secs(60),
            retry: Duration::from_secs(30),
            punch_window: Duration::from_secs(5),
        }
    }
}

/// 运行时装配参数。
pub struct NodeOptions {
    /// 核心地址 host:port。
    pub core: String,
    /// 核心证书指纹 (hex)。
    pub fingerprint: String,
    pub network: String,
    pub name: String,
    /// 节点凭证明文 (信令与核心转发准入共用)。
    pub credential: String,
    pub identity: WgIdentity,
    /// WG UDP 固定监听端口 (打洞与候选上报前提)。
    pub listen_port: u16,
    /// 直连 persistent keepalive 秒数 (FR-15, 25s 量级)。
    pub keepalive_secs: u16,
    /// 路径管理节奏 (保活 / 失活 / 重试 / 打洞窗口)。
    pub timings: PathTimings,
    /// 关键事件留痕环 (status --verbose 数据源); 由 daemon 创建并跨会话复用。
    pub events: Arc<EventRing>,
}

/// 单 peer 路径状态快照 (status 输出)。
#[derive(Debug, Clone)]
pub struct PeerStatus {
    pub name: String,
    pub ip: Ipv4Addr,
    /// idle / punching / direct / relayed。
    pub path: &'static str,
    pub endpoint: Option<SocketAddr>,
}

/// 节点状态快照 (status / 测试观测用)。
#[derive(Debug, Clone, Default)]
pub struct NodeStatus {
    pub own_ip: Option<Ipv4Addr>,
    pub policy: Option<PathPolicy>,
    pub peers: Vec<PeerStatus>,
    /// 致命退出原因 (网段冲突拒绝启动等; 运行时已退出)。
    pub fatal: Option<String>,
}

/// 运行时句柄: 状态观测与停机。Clone 后各持有方共享同一运行时。
#[derive(Clone)]
pub struct NodeHandle {
    status: Arc<Mutex<NodeStatus>>,
    events: Arc<EventRing>,
    cancel: watch::Sender<bool>,
    done: watch::Sender<Option<String>>,
}

impl NodeHandle {
    pub fn status(&self) -> NodeStatus {
        self.status.lock().expect("NodeStatus 锁中毒").clone()
    }

    /// 关键事件快照 (旧 → 新), status --verbose 数据源。
    pub fn events(&self) -> Vec<EventRecord> {
        self.events.snapshot()
    }

    /// 停机: 事件循环退出, 引擎与中继会话收尾。
    pub fn stop(&self) {
        let _ = self.cancel.send(true);
    }

    /// 退出通知: 运行时结束时置为退出原因 (daemon 会话监督的观测源)。
    pub fn done(&self) -> watch::Receiver<Option<String>> {
        self.done.subscribe()
    }
}

/// 对端表条目。
struct PeerEntry {
    public_key: PublicKey,
    vip: Ipv4Addr,
    online: bool,
    candidates: Vec<SocketAddr>,
    state: PunchState,
}

/// 单个路径节拍收集的动作 (扫描产出, 执行侧消费)。
#[derive(Default)]
struct TickActions {
    /// 打洞突发: (候选集, 节点名, 窗口令牌)。
    bursts: Vec<(Vec<SocketAddr>, String, PunchToken)>,
    /// Direct 保活 Ping: (端点, 节点名)。
    pings: Vec<(SocketAddr, String)>,
    /// 打洞窗口到期: (节点名, fallback 端点, 窗口起点)。
    expired: Vec<(String, Option<SocketAddr>, Instant)>,
    /// 直连失活: 节点名。
    dead: Vec<String>,
    /// 周期重试直连: (节点名, fallback 端点)。
    retries: Vec<(String, Option<SocketAddr>)>,
}

/// 启动节点运行时: 建引擎 + 连信令 + 进事件循环 (返回句柄, 循环在后台任务)。
pub async fn start<Tx, Rx>(options: NodeOptions, ip_tx: Tx, ip_rx: Rx) -> Result<NodeHandle>
where
    Tx: IpSend + Send + 'static,
    Rx: IpRecv + Send + 'static,
{
    let (
        hub,
        PathStreams {
            probe,
            stun,
            activity,
        },
    ) = PathHub::new();
    let factory = PathUdpFactory::new(hub.clone());
    let engine = build_engine_split(
        EngineOptions {
            private_key: options.identity.secret().clone(),
            listen_port: options.listen_port,
        },
        factory,
        ip_tx,
        ip_rx,
    )
    .await?;

    let signal_options = SignalOptions {
        core: options.core.clone(),
        fingerprint: options.fingerprint.clone(),
        network: options.network.clone(),
        name: options.name.clone(),
        credential: options.credential.clone(),
    };
    let (client, _signal_task) = SignalClient::connect(signal_options)
        .await
        .map_err(|e| Error::Transport(format!("信令通道建立失败: {e}")))?;
    options.events.push(
        EventKind::Signal,
        Some(&options.name),
        format!("信令通道建立 (核心 {})", options.core),
    );

    let status = Arc::new(Mutex::new(NodeStatus::default()));
    let events = options.events.clone();
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (done_tx, _done_rx) = watch::channel(None);
    let handle = NodeHandle {
        status: status.clone(),
        events: events.clone(),
        cancel: cancel_tx,
        done: done_tx.clone(),
    };
    let runtime = Runtime {
        options,
        hub,
        engine,
        client,
        peers: HashMap::new(),
        reverse: HashMap::new(),
        relays: Vec::new(),
        tunnels: HashMap::new(),
        status,
        events,
        paths_dirty: false,
        done: done_tx,
    };
    tokio::spawn(runtime.run(probe, stun, activity, cancel_rx));
    Ok(handle)
}

/// 事件循环持有的全部状态。
struct Runtime<Tx, Rx>
where
    Tx: IpSend,
    Rx: IpRecv,
{
    options: NodeOptions,
    hub: Arc<PathHub>,
    engine: WgEngine<(PathUdpFactory, Tx, Rx)>,
    client: SignalClient,
    /// 节点名 → 对端条目。
    peers: HashMap<String, PeerEntry>,
    /// 已回填端点 → 节点名 (WG 流量归因)。
    reverse: HashMap<SocketAddr, String>,
    /// 当前中继列表 (服务端权威)。
    relays: Vec<SignalRelay>,
    tunnels: HashMap<u8, RelayTunnel>,
    status: Arc<Mutex<NodeStatus>>,
    /// 关键事件留痕环 (与 handle 共享同一实例)。
    events: Arc<EventRing>,
    /// 路径自上次上报后已有变化 (转换事件置位, 节拍冲刷)。
    paths_dirty: bool,
    /// 退出原因通知 (handle.done 的发送端)。
    done: watch::Sender<Option<String>>,
}

impl<Tx, Rx> Runtime<Tx, Rx>
where
    Tx: IpSend,
    Rx: IpRecv,
{
    async fn run(
        mut self,
        mut probe_rx: mpsc::Receiver<crate::transport::ProbeEvent>,
        stun_rx: mpsc::Receiver<StunEvent>,
        mut activity_rx: mpsc::Receiver<SocketAddr>,
        mut cancel: watch::Receiver<bool>,
    ) {
        let (relay_down_tx, mut relay_down) = mpsc::channel::<u8>(16);
        let (stun_cmd_tx, stun_cmd_rx) = mpsc::channel::<Vec<String>>(4);
        // STUN 探测独立任务: 持有事件接收端, 结果直接回报候选地址
        tokio::spawn(stun_loop(
            self.hub.clone(),
            stun_rx,
            stun_cmd_rx,
            self.client.cmd.clone(),
            self.events.clone(),
        ));

        let mut probe_tick = tokio::time::interval(PROBE_INTERVAL);
        probe_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut ensure_tick = tokio::time::interval(RELAY_ENSURE_INTERVAL);
        ensure_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut report_tick = tokio::time::interval(PATH_REPORT_INTERVAL);
        report_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let reason: String = loop {
            tokio::select! {
                _ = cancel.changed() => break "本地停机".into(),
                ev = self.client.events.recv() => {
                    match ev {
                        Some(NodeEvent::Closed(reason)) => {
                            self.note(EventKind::Signal, None, format!("信令通道关闭: {reason}"));
                            break format!("信令通道关闭: {reason}");
                        }
                        Some(event) => {
                            if let Some(stop) = self.on_signal_event(event, &stun_cmd_tx, &relay_down_tx).await {
                                break stop;
                            }
                        }
                        None => break "信令会话结束".into(),
                    }
                }
                probe = probe_rx.recv() => {
                    if let Some(probe) = probe {
                        self.on_probe(probe.from, &probe.frame).await;
                    }
                }
                source = activity_rx.recv() => {
                    if let Some(source) = source {
                        self.on_activity(source);
                    }
                }
                down = relay_down.recv() => {
                    if let Some(id) = down {
                        self.on_relay_down(id).await;
                    }
                }
                _ = probe_tick.tick() => self.on_probe_tick().await,
                _ = report_tick.tick() => self.send_path_report(),
                _ = ensure_tick.tick() => {
                    if let Err(e) = self.ensure_relays(&relay_down_tx).await {
                        tracing::warn!(error = %e, "中继通道对齐未完成");
                    }
                }
            }
        };
        // 先广播退出 (daemon 侧 down / 监督流程即可推进), 再做引擎收尾
        let _ = self.done.send(Some(reason.clone()));
        self.shutdown(reason).await;
    }

    // ------------------------------ 信令事件 ------------------------------

    async fn on_signal_event(
        &mut self,
        event: NodeEvent,
        stun_cmd: &mpsc::Sender<Vec<String>>,
        relay_down: &mpsc::Sender<u8>,
    ) -> Option<String> {
        match event {
            NodeEvent::Ready { ip, .. } => {
                if let Ok(vip) = ip.parse::<Ipv4Addr>() {
                    self.hub.set_own_vip(vip);
                    self.status.lock().expect("NodeStatus 锁中毒").own_ip = Some(vip);
                    // FR-14: 本地网段 (CGNAT) 冲突即拒绝运行 (vip 此刻才权威已知)
                    let conflicts = netconf::detect_conflicts(vip);
                    if !conflicts.is_empty() {
                        let report = conflicts
                            .iter()
                            .map(|c| c.describe(vip))
                            .collect::<Vec<_>>()
                            .join("; ");
                        let reason = format!("本地网段与虚拟网段冲突: {report}");
                        tracing::error!(%reason, "拒绝启动 (FR-14)");
                        self.status.lock().expect("NodeStatus 锁中毒").fatal = Some(reason.clone());
                        return Some(reason);
                    }
                }
            }
            NodeEvent::State {
                nodes,
                relays,
                policy,
                stuns,
            } => {
                self.relays = relays;
                // 列表首达即建通道, 不等对齐节拍 (回落 / 失活判定可能早于首个节拍)
                if let Err(e) = self.ensure_relays(relay_down).await {
                    tracing::warn!(error = %e, "中继通道对齐未完成");
                }
                self.apply_policy(policy).await;
                self.merge_nodes(nodes).await;
                if !stuns.is_empty() {
                    let _ = stun_cmd.send(stuns).await;
                }
            }
            NodeEvent::PeerOnline(peer) => {
                self.merge_nodes(vec![peer]).await;
            }
            NodeEvent::PeerOffline { name } => {
                self.note(EventKind::Signal, Some(&name), "对端离线");
                if let Some(entry) = self.peers.get_mut(&name) {
                    entry.online = false;
                    entry.candidates.clear();
                    let public_key = entry.public_key;
                    let _ = self.engine.set_endpoint(&public_key, None).await;
                    entry.state = PunchState::Idle;
                }
                self.sync_peers().await;
            }
            NodeEvent::Relays(relays) => {
                self.relays = relays;
                if let Err(e) = self.ensure_relays(relay_down).await {
                    tracing::warn!(error = %e, "中继通道对齐未完成");
                }
            }
            NodeEvent::Policy(policy) => {
                self.apply_policy(policy).await;
            }
            NodeEvent::Closed(_) => unreachable!("Closed 在事件循环入口已拦截"),
        }
        self.refresh_status();
        None
    }

    /// 节点表合并: 新增 / 更新条目, 在线且自动策略时启动打洞。
    async fn merge_nodes(&mut self, nodes: Vec<SignalPeer>) {
        let names: Vec<String> = nodes.iter().map(|n| n.name.clone()).collect();
        for node in &nodes {
            let Ok(public_key) = WgIdentity::parse_public_key_base64(&node.wg_public_key) else {
                tracing::warn!(node = %node.name, "节点公钥非法, 跳过");
                continue;
            };
            let Ok(vip) = node.ip.parse::<Ipv4Addr>() else {
                tracing::warn!(node = %node.name, ip = %node.ip, "节点 IP 非法, 跳过");
                continue;
            };
            let candidates: Vec<SocketAddr> = node
                .candidates
                .iter()
                .filter_map(|c| c.parse().ok())
                .collect();
            let entry = self.peers.entry(node.name.clone()).or_insert(PeerEntry {
                public_key,
                vip,
                online: false,
                candidates: Vec::new(),
                state: PunchState::Idle,
            });
            entry.public_key = public_key;
            entry.vip = vip;
            let came_online = node.online && !entry.online;
            let got_candidates = !candidates.is_empty() && entry.candidates.is_empty();
            entry.candidates = candidates;
            entry.online = node.online;
            if came_online || got_candidates {
                // 上线或候选到达 (Announce 广播刷新) 即具备打洞条件;
                // 既有路径 (如回落后的重打洞) 作为 fallback 保持流量不断
                let fallback = entry.state.endpoint();
                self.begin_punch(&node.name, fallback);
            }
        }
        self.peers.retain(|name, _| names.contains(name));
        self.sync_peers().await;
    }

    /// 就地开一轮打洞窗口: 策略 Auto、非 punching、非 direct 且有候选时。
    /// fallback 为窗口期内保持的既有端点 (重试直连不中断在用流量)。
    fn begin_punch(&mut self, name: &str, fallback: Option<SocketAddr>) {
        if self.policy() != PathPolicy::Auto {
            return;
        }
        let window = self.options.timings.punch_window;
        if let Some(entry) = self.peers.get_mut(name) {
            if entry.state.is_punching() || entry.state.is_direct() || entry.candidates.is_empty() {
                return;
            }
            entry.state = PunchState::Punching {
                deadline: Instant::now() + window,
                started: Instant::now(),
                token: PunchToken::random(),
                fallback,
            };
        }
    }

    fn policy(&self) -> PathPolicy {
        self.status
            .lock()
            .expect("NodeStatus 锁中毒")
            .policy
            .unwrap_or(PathPolicy::Auto)
    }

    /// 关键事件留痕: 与既有 tracing 埋点同位双写 (日志字段结构不变, 环供排障视图);
    /// 顺带标记路径待上报 (事件点即路径转换点)。
    fn note(&mut self, kind: EventKind, peer: Option<&str>, detail: impl Into<String>) {
        self.events.push(kind, peer, detail);
        self.paths_dirty = true;
    }

    /// 策略切换: 强制中继取消打洞并全量回落; 恢复自动时对在线节点重启打洞。
    async fn apply_policy(&mut self, policy: PathPolicy) {
        {
            let mut status = self.status.lock().expect("NodeStatus 锁中毒");
            if status.policy == Some(policy) {
                return;
            }
            status.policy = Some(policy);
        }
        let names: Vec<String> = self.peers.keys().cloned().collect();
        match policy {
            PathPolicy::ForceRelay => {
                tracing::info!("路径策略切换为强制中继 (FR-10)");
                self.note(EventKind::Path, None, "路径策略切换为强制中继");
                self.enforce_force_relay(&names).await;
            }
            PathPolicy::Auto => {
                tracing::info!("路径策略切换为自动 (直连优先)");
                self.note(EventKind::Path, None, "路径策略切换为自动 (直连优先)");
                for name in names {
                    // 强制中继恢复自动: 以既有中继路径为 fallback 立即重打洞,
                    // 由打洞裁决是否回直连 (FR-10 恢复语义); direct 节点不受影响
                    let fallback = self
                        .peers
                        .get(&name)
                        .and_then(|entry| entry.state.endpoint());
                    self.begin_punch(&name, fallback);
                }
            }
        }
        self.sync_peers().await;
    }

    /// 强制中继执行: 未处于中继态的节点逐个指到在线中继伪端点。
    /// 策略到达与中继通道就绪存在时序竞争, 通道变化后需重施。
    async fn enforce_force_relay(&mut self, names: &[String]) {
        let next_retry = Instant::now() + self.options.timings.retry;
        for name in names {
            let already = self
                .peers
                .get(name)
                .map(|e| matches!(e.state, PunchState::Relayed { .. }))
                .unwrap_or(true);
            if already {
                continue;
            }
            let Some((vip, public_key)) = self
                .peers
                .get(name)
                .map(|entry| (entry.vip, entry.public_key))
            else {
                continue;
            };
            if let Some(pseudo) = self.pick_relay(vip) {
                if let Some(entry) = self.peers.get_mut(name) {
                    entry.state = PunchState::Relayed {
                        endpoint: pseudo,
                        next_retry,
                    };
                }
                let _ = self.engine.set_endpoint(&public_key, Some(pseudo)).await;
            }
        }
        self.refresh_status();
    }

    // ------------------------------ 打洞事件 ------------------------------

    /// 收到探测帧 (Ping 主动探测 / Pong 回执)。
    /// 观测源本身即通路证明 (对端 NAT 映射对本地址可用), 打洞期收到任一类
    /// 即判 direct; Direct 态收到即刷新活性并跟随端点漂移 (FR-6)。
    /// Ping 一律回 Pong (回显令牌; Pong 不再触发应答, 无回声环), 使对端
    /// 打洞 / 重试总能拿到回执; 强制中继策略下完全忽略——不学直连端点也
    /// 不应答, 否则对端探测会把引擎端点拉回真实地址, 破坏 FR-10。
    async fn on_probe(&mut self, from: SocketAddr, frame: &[u8]) {
        let Some((kind, name, token)) = decode_probe(frame) else {
            return;
        };
        if self.policy() == PathPolicy::ForceRelay {
            return;
        }
        let now = Instant::now();
        let next_ping = now + self.options.timings.ping;
        // 单次借用完成判定: Punching 取窗口起点 (稍后转直连),
        // Direct 跟随端点漂移并刷新活性
        let (public_key, punch_started) = match self.peers.get_mut(&name) {
            Some(entry) => {
                let public_key = entry.public_key;
                let punch_started = match &mut entry.state {
                    PunchState::Punching { started, .. } => Some(*started),
                    // 端点漂移 (对端 NAT 重映射): 跟随新端点并刷新活性
                    PunchState::Direct {
                        endpoint,
                        last_alive,
                        ..
                    } => {
                        *endpoint = from;
                        *last_alive = now;
                        None
                    }
                    // 无打洞意图 (Relayed / Idle): 只应答, 不动在用路径,
                    // 直连由本端自己的重试窗口裁决
                    _ => None,
                };
                (public_key, punch_started)
            }
            None => return,
        };
        let learn_endpoint = punch_started.is_some()
            || self
                .peers
                .get(&name)
                .is_some_and(|entry| entry.state.is_direct());
        if let Some(started) = punch_started {
            self.enter_direct(&name, from, started, now, next_ping);
        }
        self.reverse.insert(from, name.clone());
        if learn_endpoint {
            let _ = self.engine.set_endpoint(&public_key, Some(from)).await;
        }
        if kind == ProbeKind::Ping {
            if let Some(udp) = self.hub.udp() {
                let pong = encode_probe(ProbeKind::Pong, &self.options.name, &token);
                let _ = udp.send_to(&pong, from).await;
            }
        }
        self.refresh_status();
    }

    /// 观测到来自某端点的 WG 流量 (首见来源): 打洞成功 / 端点漂移确认。
    /// 后续流量不再上报 (传输层去重), 持续活性由周期 Ping 承担。
    fn on_activity(&mut self, source: SocketAddr) {
        let Some(name) = self.reverse.get(&source).cloned() else {
            return;
        };
        let Some(entry) = self.peers.get_mut(&name) else {
            return;
        };
        let now = Instant::now();
        let next_ping = now + self.options.timings.ping;
        let punch_started = match &mut entry.state {
            PunchState::Punching { started, .. } => Some(*started),
            // 端点漂移 (对端 NAT 重映射): 跟随新端点并刷新活性
            PunchState::Direct {
                endpoint,
                last_alive,
                ..
            } if *endpoint != source => {
                *endpoint = source;
                *last_alive = now;
                None
            }
            _ => None,
        };
        if let Some(started) = punch_started {
            self.enter_direct(&name, source, started, now, next_ping);
        }
        self.refresh_status();
    }

    /// 打洞成功共通收口 (探测应答 / WG 流量两条观测路共用):
    /// 留痕 duration 日志与关键事件后转入 Direct; 日志/事件/状态构造保持单点。
    fn enter_direct(
        &mut self,
        name: &str,
        endpoint: SocketAddr,
        started: Instant,
        now: Instant,
        next_ping: Instant,
    ) {
        tracing::info!(
            peer = %name,
            endpoint = %endpoint,
            duration_ms = elapsed_ms(started),
            "打洞成功, 切换直连"
        );
        if let Some(entry) = self.peers.get_mut(name) {
            entry.state = PunchState::Direct {
                endpoint,
                last_alive: now,
                next_ping,
            };
        }
        self.note(
            EventKind::Punch,
            Some(name),
            format!("打洞成功, 切换直连 ({endpoint})"),
        );
    }

    /// 路径节拍 (PROBE_INTERVAL): 打洞突发 / 窗口到期 / Direct 保活 Ping 与
    /// 失活判定 / Relayed 周期重试 (FR-6: 直连中断切中继, 直连恢复切回)。
    async fn on_probe_tick(&mut self) {
        let actions = self.scan_tick_actions(Instant::now());
        for (candidates, name, token) in actions.bursts {
            self.send_probes(&candidates, &name, &token);
        }
        for (endpoint, name) in actions.pings {
            let token = PunchToken::random();
            self.send_probes(&[endpoint], &name, &token);
        }
        for (name, fallback, started) in actions.expired {
            self.on_punch_expired(name, fallback, started).await;
        }
        for name in actions.dead {
            self.on_direct_dead(name).await;
        }
        for (name, fallback) in actions.retries {
            self.begin_punch(&name, fallback);
        }
        // 转换事件即时上报 (事件点经 note 置脏; 未变化不重发)
        if self.paths_dirty {
            self.paths_dirty = false;
            self.send_path_report();
        }
        self.refresh_status();
    }

    /// 发送逐对端路径快照。
    /// try_send 理由: 命令通道打满时丢弃本帧, 下一转换或周期兜底自然补报。
    fn send_path_report(&self) {
        let _ = self
            .client
            .cmd
            .try_send(SignalCmd::ReportPaths(self.path_report()));
    }

    /// 逐对端路径快照 (idle / punching / direct / relayed, relayed 附中继地址)。
    fn path_report(&self) -> Vec<PeerPathReport> {
        self.peers
            .iter()
            .map(|(name, entry)| {
                let (path, relay) = match &entry.state {
                    PunchState::Idle => ("idle", None),
                    PunchState::Punching { .. } => ("punching", None),
                    PunchState::Direct { .. } => ("direct", None),
                    PunchState::Relayed { endpoint, .. } => (
                        "relayed",
                        PathHub::split_pseudo(*endpoint).and_then(|(id, _)| {
                            self.tunnels.get(&id).map(|t| t.endpoint.addr.clone())
                        }),
                    ),
                };
                PeerPathReport {
                    peer: name.clone(),
                    path: path.into(),
                    relay,
                }
            })
            .collect()
    }

    /// 扫描路径状态, 收集推进到当前时刻应执行的动作 (纯状态推进, 无 IO;
    /// 执行侧含 await 与引擎调用, 分离以保持节拍主体可读)。
    fn scan_tick_actions(&mut self, now: Instant) -> TickActions {
        let timings = self.options.timings.clone();
        let auto = self.policy() == PathPolicy::Auto;
        let mut actions = TickActions::default();
        for (name, entry) in self.peers.iter_mut() {
            match &mut entry.state {
                PunchState::Punching {
                    deadline,
                    started,
                    token,
                    fallback,
                } => {
                    if now >= *deadline {
                        actions.expired.push((name.clone(), *fallback, *started));
                    } else if entry.online && !entry.candidates.is_empty() {
                        actions
                            .bursts
                            .push((entry.candidates.clone(), name.clone(), *token));
                    }
                }
                PunchState::Direct {
                    endpoint,
                    last_alive,
                    next_ping,
                } => {
                    if now >= *next_ping {
                        actions.pings.push((*endpoint, name.clone()));
                        *next_ping = now + timings.ping;
                    }
                    if now.duration_since(*last_alive) >= timings.dead_after {
                        actions.dead.push(name.clone());
                    }
                }
                PunchState::Relayed {
                    endpoint,
                    next_retry,
                } => {
                    if auto && entry.online && !entry.candidates.is_empty() && now >= *next_retry {
                        actions.retries.push((name.clone(), Some(*endpoint)));
                    }
                }
                PunchState::Idle => {}
            }
        }
        actions
    }

    /// 打洞窗口到期: fallback (既有中继路径) 仍在线才直接回退, 流量未中断;
    /// 中继在窗口期死亡 / 失联时重选在线中继, 避免锚定死端点无法自愈;
    /// 无 fallback 时回落中继 (FR-3) 或无中继可用则回 Idle 等待。
    async fn on_punch_expired(
        &mut self,
        name: String,
        fallback: Option<SocketAddr>,
        started: Instant,
    ) {
        let now = Instant::now();
        let duration_ms = elapsed_ms(started);
        let next_retry = now + self.options.timings.retry;
        let fallback = fallback.filter(|pseudo| {
            PathHub::split_pseudo(*pseudo)
                .map(|(id, _)| self.hub.relay_online(id))
                .unwrap_or(false)
        });
        if let Some(pseudo) = fallback {
            tracing::info!(peer = %name, duration_ms, "重试打洞未果, 保持中继 (FR-6)");
            self.note(EventKind::Punch, Some(&name), "重试打洞未果, 保持中继");
            if let Some(entry) = self.peers.get_mut(&name) {
                entry.state = PunchState::Relayed {
                    endpoint: pseudo,
                    next_retry,
                };
            }
            return;
        }
        let Some((vip, public_key)) = self
            .peers
            .get(&name)
            .map(|entry| (entry.vip, entry.public_key))
        else {
            return;
        };
        match self.pick_relay(vip) {
            Some(pseudo) => {
                tracing::info!(peer = %name, relay_endpoint = %pseudo, duration_ms, "打洞超时, 回落中继 (FR-3)");
                self.note(
                    EventKind::Punch,
                    Some(&name),
                    format!("打洞超时, 回落中继 ({pseudo})"),
                );
                if let Some(entry) = self.peers.get_mut(&name) {
                    entry.state = PunchState::Relayed {
                        endpoint: pseudo,
                        next_retry,
                    };
                }
                let _ = self.engine.set_endpoint(&public_key, Some(pseudo)).await;
            }
            None => {
                tracing::warn!(peer = %name, "打洞超时且无可用中继, 保持等待");
                self.note(
                    EventKind::Punch,
                    Some(&name),
                    "打洞超时且无可用中继, 等待候选或中继",
                );
                if let Some(entry) = self.peers.get_mut(&name) {
                    entry.state = PunchState::Idle;
                }
            }
        }
    }

    /// 直连失活 (超过阈值无探测 / WG 流量证据): 回落中继, 到重试节拍再尝试
    /// 切回直连 (FR-6)。失活已静默一个 dead_after 窗口, 立即再打洞的边际收益
    /// 有限, 统一交由 Relayed 的周期重试接管, 也避免对端真正离线时探测风暴。
    async fn on_direct_dead(&mut self, name: String) {
        let Some((vip, public_key)) = self
            .peers
            .get(&name)
            .map(|entry| (entry.vip, entry.public_key))
        else {
            return;
        };
        let next_retry = Instant::now() + self.options.timings.retry;
        match self.pick_relay(vip) {
            Some(pseudo) => {
                tracing::warn!(peer = %name, relay_endpoint = %pseudo, "直连失活, 回落中继 (FR-6)");
                self.note(
                    EventKind::Path,
                    Some(&name),
                    format!("直连失活, 回落中继 ({pseudo})"),
                );
                if let Some(entry) = self.peers.get_mut(&name) {
                    entry.state = PunchState::Relayed {
                        endpoint: pseudo,
                        next_retry,
                    };
                }
                let _ = self.engine.set_endpoint(&public_key, Some(pseudo)).await;
            }
            None => {
                tracing::warn!(peer = %name, "直连失活且无可用中继, 回到等待");
                self.note(
                    EventKind::Path,
                    Some(&name),
                    "直连失活且无可用中继, 回到等待",
                );
                if let Some(entry) = self.peers.get_mut(&name) {
                    entry.state = PunchState::Idle;
                }
            }
        }
    }

    /// 突发探测: 帧携带本节点名 (接收方以此归位发送者), 发往全部目标地址;
    /// 打洞窗口内为候选突发, Direct 态为单点保活 Ping。
    fn send_probes(&self, targets: &[SocketAddr], peer: &str, token: &PunchToken) {
        let Some(udp) = self.hub.udp() else {
            return;
        };
        let frame = encode_probe(ProbeKind::Ping, &self.options.name, token);
        for target in targets {
            let _ = udp.try_send_to(&frame, *target);
        }
        tracing::debug!(peer = %peer, targets = targets.len(), "探测已发出");
    }

    /// 选择回落中继: 列表序首个在线连接 (核心内置恒在末位兜底)。
    fn pick_relay(&self, peer_vip: Ipv4Addr) -> Option<SocketAddr> {
        self.hub
            .online_relay_ids()
            .first()
            .map(|id| PathHub::pseudo_endpoint(*id, peer_vip))
    }

    // ------------------------------ 中继通道 ------------------------------

    /// 列表驱动的中继通道对齐: 新建 / 替换 / 摘除 (FR-18)。
    async fn ensure_relays(&mut self, down: &mpsc::Sender<u8>) -> Result<()> {
        let list_len = self.relays.len().min(MAX_RELAYS);
        if self.relays.len() > MAX_RELAYS {
            tracing::warn!(
                count = self.relays.len(),
                "中继数量超过伪端点编码上限, 忽略多余条目"
            );
        }
        self.prune_stale_tunnels(list_len).await;

        // 新建 / 替换 (克隆列表避免与 tunnel 表的借用交叉)
        let relays = self.relays.clone();
        let mut changed = false;
        for (idx, endpoint) in relays.iter().take(MAX_RELAYS).enumerate() {
            let id = (idx + 1) as u8;
            if self.tunnel_matches(id, endpoint) {
                continue;
            }
            self.replace_tunnel(id, endpoint).await;
            changed |= self.connect_relay(id, endpoint, down).await;
        }
        // 通道可用性变化后重施强制中继 (策略可能先于通道到达)
        if changed && self.policy() == PathPolicy::ForceRelay {
            let names: Vec<String> = self.peers.keys().cloned().collect();
            self.enforce_force_relay(&names).await;
        }
        Ok(())
    }

    /// 摘除超出当前列表的通道。
    async fn prune_stale_tunnels(&mut self, list_len: usize) {
        let stale: Vec<u8> = self
            .tunnels
            .keys()
            .copied()
            .filter(|id| (*id as usize) > list_len)
            .collect();
        for id in stale {
            if let Some(tunnel) = self.tunnels.remove(&id) {
                tunnel.shutdown(&self.hub);
                self.repoint_relayed(id).await;
            }
        }
    }

    /// 同位通道是否已对应该条目 (地址 + 指纹)。
    fn tunnel_matches(&self, id: u8, endpoint: &SignalRelay) -> bool {
        self.tunnels
            .get(&id)
            .map(|existing| {
                existing.endpoint.addr == endpoint.addr
                    && existing.endpoint.fingerprint == endpoint.fingerprint
            })
            .unwrap_or(false)
    }

    /// 废弃同位旧通道 (条目身份变更时)。
    async fn replace_tunnel(&mut self, id: u8, endpoint: &SignalRelay) {
        if let Some(tunnel) = self.tunnels.remove(&id) {
            let old_addr = tunnel.endpoint.addr.clone();
            tunnel.shutdown(&self.hub);
            tracing::info!(id, old = %old_addr, new = %endpoint.addr, "中继条目变更, 重建通道");
            self.note(
                EventKind::Path,
                None,
                format!("中继 {id} 条目变更, 重建通道"),
            );
            self.repoint_relayed(id).await;
        }
    }

    /// 建连并登记; 限时兜底 (拨号在主循环内), 返回通道集合是否变化。
    async fn connect_relay(
        &mut self,
        id: u8,
        endpoint: &SignalRelay,
        down: &mpsc::Sender<u8>,
    ) -> bool {
        let identity = self.relay_identity();
        let connect = RelayTunnel::connect(
            endpoint.clone(),
            id,
            identity,
            self.hub.clone(),
            down.clone(),
        );
        match tokio::time::timeout(RELAY_CONNECT_TIMEOUT, connect).await {
            Ok(Ok(tunnel)) => {
                tracing::info!(id, relay = %endpoint.addr, kind = %endpoint.kind, "中继通道就绪");
                self.note(
                    EventKind::Path,
                    None,
                    format!("中继 {id} 通道就绪 ({})", endpoint.addr),
                );
                self.tunnels.insert(id, tunnel);
                true
            }
            Ok(Err(e)) => {
                tracing::warn!(id, relay = %endpoint.addr, error = %e, "中继通道建立失败, 待下轮重试");
                self.note(
                    EventKind::Path,
                    None,
                    format!("中继 {id} 通道建立失败 ({e})"),
                );
                false
            }
            Err(_) => {
                tracing::warn!(id, relay = %endpoint.addr, "中继通道建立超时, 待下轮重试");
                self.note(EventKind::Path, None, format!("中继 {id} 通道建立超时"));
                false
            }
        }
    }

    /// 转发准入身份 (与信令共用一套凭证)。
    fn relay_identity(&self) -> RelayIdentity {
        RelayIdentity {
            network: self.options.network.clone(),
            name: self.options.name.clone(),
            credential: self.options.credential.clone(),
            own_vip: self
                .status
                .lock()
                .expect("NodeStatus 锁中毒")
                .own_ip
                .map(|ip| ip.to_string())
                .unwrap_or_default(),
            core: self.options.core.clone(),
        }
    }

    /// 某中继号失联: 走该伪端点的节点重指到其他在线中继。
    async fn on_relay_down(&mut self, id: u8) {
        self.repoint_relayed(id).await;
    }

    async fn repoint_relayed(&mut self, id: u8) {
        let names: Vec<String> = self.peers.keys().cloned().collect();
        let next_retry = Instant::now() + self.options.timings.retry;
        for name in names {
            let Some((vip, public_key, relay_id)) = self.peers.get(&name).and_then(|entry| {
                if let PunchState::Relayed { endpoint, .. } = entry.state {
                    PathHub::split_pseudo(endpoint)
                        .map(|(rid, _)| (entry.vip, entry.public_key, rid))
                } else {
                    None
                }
            }) else {
                continue;
            };
            if relay_id != id {
                continue;
            }
            match self.pick_relay(vip) {
                Some(pseudo) => {
                    self.note(
                        EventKind::Path,
                        Some(&name),
                        format!("中继 {id} 失联, 重指中继 ({pseudo})"),
                    );
                    if let Some(entry) = self.peers.get_mut(&name) {
                        entry.state = PunchState::Relayed {
                            endpoint: pseudo,
                            next_retry,
                        };
                    }
                    let _ = self.engine.set_endpoint(&public_key, Some(pseudo)).await;
                }
                None => {
                    if let Some(entry) = self.peers.get_mut(&name) {
                        entry.state = PunchState::Idle;
                    }
                }
            }
        }
        self.refresh_status();
    }

    // ------------------------------ 引擎同步 ------------------------------

    /// 节点表全量对齐: 摘除离表节点, 对齐 allowed ip / keepalive / 已知端点。
    /// 打洞期 keepalive 收紧, 驱动握手尽快确认直连。
    async fn sync_peers(&self) {
        let specs: Vec<PeerSpec> = self
            .peers
            .values()
            .map(|entry| PeerSpec {
                public_key: entry.public_key,
                virtual_ip: entry.vip,
                endpoint: entry.state.endpoint(),
                keepalive_secs: Some(if entry.state.is_punching() {
                    PUNCH_KEEPALIVE_SECS
                } else {
                    self.options.keepalive_secs
                }),
            })
            .collect();
        if let Err(e) = self.engine.sync_peers(&specs).await {
            tracing::error!(error = %e, "节点表对齐失败");
        }
    }

    fn refresh_status(&mut self) {
        let mut peers: Vec<PeerStatus> = self
            .peers
            .iter()
            .map(|(name, entry)| PeerStatus {
                name: name.clone(),
                ip: entry.vip,
                path: match entry.state {
                    PunchState::Idle => "idle",
                    PunchState::Punching { .. } => "punching",
                    PunchState::Direct { .. } => "direct",
                    PunchState::Relayed { .. } => "relayed",
                },
                endpoint: entry.state.endpoint(),
            })
            .collect();
        peers.sort_by(|a, b| a.ip.octets().cmp(&b.ip.octets()));
        let mut status = self.status.lock().expect("NodeStatus 锁中毒");
        status.peers = peers;
    }

    async fn shutdown(self, reason: String) {
        tracing::info!(%reason, "节点运行时退出");
        for (_, tunnel) in self.tunnels {
            tunnel.shutdown(&self.hub);
        }
        self.engine.stop().await;
    }
}

/// STUN 探测任务: 收到服务器列表即探测, 结果作为候选地址上报。
async fn stun_loop(
    hub: Arc<PathHub>,
    mut events: mpsc::Receiver<StunEvent>,
    mut cmd: mpsc::Receiver<Vec<String>>,
    signal: mpsc::Sender<SignalCmd>,
    ring: Arc<EventRing>,
) {
    while let Some(servers) = cmd.recv().await {
        let Some(udp) = hub.udp() else {
            continue;
        };
        let mapped = stun::probe_mapped_addresses(&servers, udp, &mut events).await;
        if mapped.is_empty() {
            tracing::warn!("STUN 探测无结果, 候选地址为空");
            ring.push(EventKind::Punch, None, "STUN 探测无结果, 候选地址为空");
        }
        let candidates: Vec<String> = mapped.iter().map(|a| a.to_string()).collect();
        let _ = signal.send(SignalCmd::Announce(candidates)).await;
    }
}
