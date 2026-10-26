//! Description: 节点运行时: 引擎 / 信令 / STUN / 打洞 / 中继通道的装配与事件循环。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 职责分层: 本模块只做事件收集与状态推进 (State / Event / Action), 协议细节
//! 分属 transport / stun / punch / relay_conn / signal 各模块。路径策略两档
//! (FR-10): Auto = 候选到达即并发打洞, 窗口内无 WG 流量则回落中继;
//! ForceRelay = 跳过打洞直接伪端点。直连与中继的切换以引擎端点漂移为底,
//! 周期性重试直连与保活精化随 T08 落地。

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gotatun::tun::{IpRecv, IpSend};
use gotatun::x25519::PublicKey;
use tokio::sync::{mpsc, watch};

use kunxtier_core::protocol::{PathPolicy, SignalPeer, SignalRelay};

use crate::engine::{build_engine_split, EngineOptions, PeerSpec, WgEngine};
use crate::error::{Error, Result};
use crate::identity::WgIdentity;
use crate::punch::{decode_probe, encode_probe, PunchState, PunchToken};
use crate::relay_conn::{RelayIdentity, RelayTunnel};
use crate::signal::{NodeEvent, SignalClient, SignalCmd, SignalOptions};
use crate::stun;
use crate::transport::{PathHub, PathStreams, PathUdpFactory, StunEvent};

/// 打洞探测突发间隔。
const PROBE_INTERVAL: Duration = Duration::from_millis(250);
/// 打洞窗口: 超时回落中继 (FR-3)。
const PUNCH_WINDOW: Duration = Duration::from_secs(5);
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
}

/// 运行时句柄: 状态观测与停机。
pub struct NodeHandle {
    status: Arc<Mutex<NodeStatus>>,
    cancel: watch::Sender<bool>,
}

impl NodeHandle {
    pub fn status(&self) -> NodeStatus {
        self.status.lock().expect("NodeStatus 锁中毒").clone()
    }

    /// 停机: 事件循环退出, 引擎与中继会话收尾。
    pub fn stop(&self) {
        let _ = self.cancel.send(true);
    }
}

/// 对端表条目。
struct PeerEntry {
    public_key: PublicKey,
    vip: Ipv4Addr,
    online: bool,
    candidates: Vec<SocketAddr>,
    state: PunchState,
    /// 最近一次收到的该节点探测令牌 (回执防回声环)。
    last_probe_token: Option<PunchToken>,
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

    let status = Arc::new(Mutex::new(NodeStatus::default()));
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let handle = NodeHandle {
        status: status.clone(),
        cancel: cancel_tx,
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
        ));

        let mut probe_tick = tokio::time::interval(PROBE_INTERVAL);
        probe_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut ensure_tick = tokio::time::interval(RELAY_ENSURE_INTERVAL);
        ensure_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let reason: String = loop {
            tokio::select! {
                _ = cancel.changed() => break "本地停机".into(),
                ev = self.client.events.recv() => {
                    match ev {
                        Some(NodeEvent::Closed(reason)) => {
                            break format!("信令通道关闭: {reason}");
                        }
                        Some(event) => {
                            if let Some(stop) = self.on_signal_event(event, &stun_cmd_tx).await {
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
                _ = ensure_tick.tick() => {
                    if let Err(e) = self.ensure_relays(&relay_down_tx).await {
                        tracing::warn!(error = %e, "中继通道对齐未完成");
                    }
                }
            }
        };
        self.shutdown(reason).await;
    }

    // ------------------------------ 信令事件 ------------------------------

    async fn on_signal_event(
        &mut self,
        event: NodeEvent,
        stun_cmd: &mpsc::Sender<Vec<String>>,
    ) -> Option<String> {
        match event {
            NodeEvent::Ready { ip, .. } => {
                if let Ok(vip) = ip.parse::<Ipv4Addr>() {
                    self.hub.set_own_vip(vip);
                    self.status.lock().expect("NodeStatus 锁中毒").own_ip = Some(vip);
                }
            }
            NodeEvent::State {
                nodes,
                relays,
                policy,
                stuns,
            } => {
                self.relays = relays;
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
                last_probe_token: None,
            });
            entry.public_key = public_key;
            entry.vip = vip;
            let came_online = node.online && !entry.online;
            let got_candidates = !candidates.is_empty() && entry.candidates.is_empty();
            entry.candidates = candidates;
            entry.online = node.online;
            if came_online || got_candidates {
                // 上线或候选到达 (Announce 广播刷新) 即具备打洞条件
                self.start_punch(&node.name);
            }
        }
        self.peers.retain(|name, _| names.contains(name));
        self.sync_peers().await;
    }

    /// 就地对单个节点评估打洞启动 (策略 Auto、有候选且当前无工作路径时)。
    fn start_punch(&mut self, name: &str) {
        let policy = self.policy();
        let Some(entry) = self.peers.get_mut(name) else {
            return;
        };
        if policy != PathPolicy::Auto || entry.state.is_punching() || entry.candidates.is_empty() {
            return;
        }
        if matches!(
            entry.state,
            PunchState::Direct { .. } | PunchState::Relayed { .. }
        ) {
            // 已有工作路径: T07 不主动重试直连 (周期重试随 T08)
            return;
        }
        entry.state = PunchState::Punching {
            deadline: std::time::Instant::now() + PUNCH_WINDOW,
            token: PunchToken::random(),
        };
    }

    fn policy(&self) -> PathPolicy {
        self.status
            .lock()
            .expect("NodeStatus 锁中毒")
            .policy
            .unwrap_or(PathPolicy::Auto)
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
                self.enforce_force_relay(&names).await;
            }
            PathPolicy::Auto => {
                tracing::info!("路径策略切换为自动 (直连优先)");
                for name in names {
                    // 强制中继恢复自动: 清端点回 Idle, 由重打洞裁决路径 (FR-10 恢复语义)
                    let public_key = self.peers.get(&name).map(|entry| entry.public_key);
                    if let Some(public_key) = public_key {
                        if let Some(entry) = self.peers.get_mut(&name) {
                            if matches!(entry.state, PunchState::Relayed { .. }) {
                                entry.state = PunchState::Idle;
                                let _ = self.engine.set_endpoint(&public_key, None).await;
                            }
                        }
                    }
                    self.start_punch(&name);
                }
            }
        }
        self.sync_peers().await;
    }

    /// 强制中继执行: 未处于中继态的节点逐个指到在线中继伪端点。
    /// 策略到达与中继通道就绪存在时序竞争, 通道变化后需重施。
    async fn enforce_force_relay(&mut self, names: &[String]) {
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
                    entry.state = PunchState::Relayed { endpoint: pseudo };
                }
                let _ = self.engine.set_endpoint(&public_key, Some(pseudo)).await;
            }
        }
        self.refresh_status();
    }

    // ------------------------------ 打洞事件 ------------------------------

    /// 收到探测帧: 回填来源端点; 回执条件为 "打洞中或新令牌" (防回声环,
    /// 同时保证对端打洞时已直连的本端仍会应答一次, 支撑策略恢复后的重打洞)。
    /// 探测的观测源本身即双向通路证明 (对端 NAT 映射对本地址可用),
    /// 故打洞期收到探测即判 direct; WG 流量事件退化为端点漂移监测——
    /// WG 握手是按需的, 首个业务包才触发, 不作为打洞成功的必要条件。
    async fn on_probe(&mut self, from: SocketAddr, frame: &[u8]) {
        let Some((name, token)) = decode_probe(frame) else {
            return;
        };
        let Some(entry) = self.peers.get_mut(&name) else {
            return;
        };
        let public_key = entry.public_key;
        let is_new_token = entry.last_probe_token != Some(token);
        entry.last_probe_token = Some(token);
        let punching = entry.state.is_punching();
        let reply = (punching || is_new_token).then(|| {
            let token = match &entry.state {
                PunchState::Punching { token, .. } => *token,
                _ => PunchToken::random(),
            };
            encode_probe(&self.options.name, &token)
        });
        self.reverse.insert(from, name.clone());
        let _ = self.engine.set_endpoint(&public_key, Some(from)).await;
        if let Some(entry) = self.peers.get_mut(&name) {
            if punching {
                tracing::info!(peer = %name, endpoint = %from, "打洞成功, 切换直连");
                entry.state = PunchState::Direct { endpoint: from };
            }
        }
        if let Some(frame) = reply {
            if let Some(udp) = self.hub.udp() {
                let _ = udp.send_to(&frame, from).await;
            }
        }
        self.refresh_status();
    }

    /// 观测到来自某端点的 WG 流量: 打洞成功 / 端点漂移确认。
    fn on_activity(&mut self, source: SocketAddr) {
        let Some(name) = self.reverse.get(&source).cloned() else {
            return;
        };
        let Some(entry) = self.peers.get_mut(&name) else {
            return;
        };
        match entry.state {
            PunchState::Punching { .. } => {
                tracing::info!(peer = %name, endpoint = %source, "打洞成功, 切换直连");
                entry.state = PunchState::Direct { endpoint: source };
            }
            PunchState::Direct { endpoint } if endpoint != source => {
                // 端点漂移 (对端 NAT 重映射): 跟随新端点
                entry.state = PunchState::Direct { endpoint: source };
            }
            _ => {}
        }
        self.refresh_status();
    }

    /// 打洞节拍: 突发探测 + 窗口到期回落。
    async fn on_probe_tick(&mut self) {
        let now = std::time::Instant::now();
        let mut bursts: Vec<(Vec<SocketAddr>, String, PunchToken)> = Vec::new();
        let mut expired: Vec<String> = Vec::new();
        for (name, entry) in self.peers.iter_mut() {
            if let PunchState::Punching { deadline, token } = &mut entry.state {
                if now >= *deadline {
                    expired.push(name.clone());
                } else if entry.online && !entry.candidates.is_empty() {
                    bursts.push((entry.candidates.clone(), name.clone(), *token));
                }
            }
        }
        for (candidates, name, token) in bursts {
            self.send_probes(&candidates, &name, &token);
        }
        for name in expired {
            let Some((vip, public_key)) = self
                .peers
                .get(&name)
                .map(|entry| (entry.vip, entry.public_key))
            else {
                continue;
            };
            match self.pick_relay(vip) {
                Some(pseudo) => {
                    tracing::info!(peer = %name, relay_endpoint = %pseudo, "打洞超时, 回落中继 (FR-3)");
                    if let Some(entry) = self.peers.get_mut(&name) {
                        entry.state = PunchState::Relayed { endpoint: pseudo };
                    }
                    let _ = self.engine.set_endpoint(&public_key, Some(pseudo)).await;
                }
                None => {
                    tracing::warn!(peer = %name, "打洞超时且无可用中继, 保持等待");
                    if let Some(entry) = self.peers.get_mut(&name) {
                        entry.state = PunchState::Idle;
                    }
                }
            }
        }
        self.refresh_status();
    }

    /// 突发探测: 帧携带本节点名 (接收方以此归位发送者), 发往对端全部候选。
    fn send_probes(&self, candidates: &[SocketAddr], peer: &str, token: &PunchToken) {
        let Some(udp) = self.hub.udp() else {
            return;
        };
        let frame = encode_probe(&self.options.name, token);
        for candidate in candidates {
            let _ = udp.try_send_to(&frame, *candidate);
        }
        tracing::debug!(peer = %peer, candidates = candidates.len(), "探测突发已发出");
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
                self.tunnels.insert(id, tunnel);
                true
            }
            Ok(Err(e)) => {
                tracing::warn!(id, relay = %endpoint.addr, error = %e, "中继通道建立失败, 待下轮重试");
                false
            }
            Err(_) => {
                tracing::warn!(id, relay = %endpoint.addr, "中继通道建立超时, 待下轮重试");
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
        for name in names {
            let Some((vip, public_key, relay_id)) = self.peers.get(&name).and_then(|entry| {
                if let PunchState::Relayed { endpoint } = entry.state {
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
                    if let Some(entry) = self.peers.get_mut(&name) {
                        entry.state = PunchState::Relayed { endpoint: pseudo };
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
) {
    while let Some(servers) = cmd.recv().await {
        let Some(udp) = hub.udp() else {
            continue;
        };
        let mapped = stun::probe_mapped_addresses(&servers, udp, &mut events).await;
        if mapped.is_empty() {
            tracing::warn!("STUN 探测无结果, 候选地址为空");
        }
        let candidates: Vec<String> = mapped.iter().map(|a| a.to_string()).collect();
        let _ = signal.send(SignalCmd::Announce(candidates)).await;
    }
}
