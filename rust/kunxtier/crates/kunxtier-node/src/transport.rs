//! Description: 数据面复合传输: 直连 UDP 分流 (WG / STUN / 打洞探测) + 中继 WS 伪端点。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! gotatun 引擎视角只有一个 "UDP 端点世界": 本模块把两条物理路径折叠进去——
//! - 直连: 真实 UDP socket; 入向按前缀分流, WG 帧交引擎, STUN 响应与打洞
//!   探测帧走旁路事件通道 (与 WG 同端口, NAT 映射一致是打洞前提);
//! - 中继: 每个中继连接分配 240/4 段伪端点 (240.{中继号}.{vip 后两段}:{vip 第二段}),
//!   上行按伪端点解码回 (中继号, 目的 vip) 封转发帧, 下行帧的源 vip 经伪端点
//!   呈现给引擎——端点漂移语义天然承担直连 / 中继切换 (FR-6 的基础)。
//!
//! 事件通道与 socket 由 PathHub 持有: 引擎重绑定只换 socket, 消费端不漂移。

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use gotatun::packet::{Packet, PacketBufPool};
use gotatun::udp::{UdpRecv, UdpSend, UdpTransportFactory, UdpTransportFactoryParams};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use crate::punch;
use crate::stun;

/// 旁路事件通道容量: 探测 / STUN / 首见直连来源均为低频小帧, 溢出即丢弃。
const EVENT_CAPACITY: usize = 512;
/// 中继入向帧通道容量: 满即按 UDP 语义丢弃。
const INBOUND_CAPACITY: usize = 256;
/// 首见直连来源上报的去重上限 (节点规模量级, 超出后停止上报新来源)。
const ACTIVITY_SOURCES_CAP: usize = 256;

/// 打洞探测帧事件: 旁路自直连 socket 入向。
#[derive(Debug)]
pub struct ProbeEvent {
    pub from: SocketAddr,
    pub frame: Vec<u8>,
}

/// STUN 响应事件: 旁路自直连 socket 入向。
#[derive(Debug)]
pub struct StunEvent {
    pub from: SocketAddr,
    pub bytes: Vec<u8>,
}

/// 中继入向帧: (中继号, 源虚拟 IP, WG 密文)。
#[derive(Debug)]
pub struct RelayInbound {
    pub relay_id: u8,
    pub src_vip: Ipv4Addr,
    pub payload: Vec<u8>,
}

/// PathHub 的消费端事件流 (构造时一次性交出, 进程生命周期内由运行时持有)。
pub struct PathStreams {
    pub probe: mpsc::Receiver<ProbeEvent>,
    pub stun: mpsc::Receiver<StunEvent>,
    /// 首见直连 WG 来源 (打洞成功判据; 端点漂移来源同样会上报)。
    pub activity: mpsc::Receiver<SocketAddr>,
}

/// 两条物理路径的共享枢纽: socket / 中继连接表 / 本节点身份 / 事件出口。
pub struct PathHub {
    udp: Mutex<Option<Arc<UdpSocket>>>,
    /// 中继号 → 帧发送端 (RelayConn 会话持有接收端)。
    relays: Mutex<HashMap<u8, mpsc::Sender<Vec<u8>>>>,
    own_vip: Mutex<Option<Ipv4Addr>>,
    probe_tx: mpsc::Sender<ProbeEvent>,
    stun_tx: mpsc::Sender<StunEvent>,
    activity_tx: mpsc::Sender<SocketAddr>,
    /// 中继入向通道: 接收端由引擎的 PathRecv 持有 (重绑定时整组换新)。
    inbound: Mutex<(
        mpsc::Sender<RelayInbound>,
        Option<mpsc::Receiver<RelayInbound>>,
    )>,
}

impl PathHub {
    pub fn new() -> (Arc<Self>, PathStreams) {
        let (probe_tx, probe) = mpsc::channel(EVENT_CAPACITY);
        let (stun_tx, stun) = mpsc::channel(EVENT_CAPACITY);
        let (activity_tx, activity) = mpsc::channel(EVENT_CAPACITY);
        let (inbound_tx, inbound_rx) = mpsc::channel(INBOUND_CAPACITY);
        (
            Arc::new(PathHub {
                udp: Mutex::new(None),
                relays: Mutex::new(HashMap::new()),
                own_vip: Mutex::new(None),
                probe_tx,
                stun_tx,
                activity_tx,
                inbound: Mutex::new((inbound_tx, Some(inbound_rx))),
            }),
            PathStreams {
                probe,
                stun,
                activity,
            },
        )
    }

    /// 引擎绑定 / 重绑定时回填真实 socket。
    pub(crate) fn set_udp(&self, sock: Arc<UdpSocket>) {
        *self.udp.lock().expect("PathHub.udp 锁中毒") = Some(sock);
    }

    /// 直连 socket 出口 (STUN 请求与打洞探测共用 WG 端口)。
    pub fn udp(&self) -> Option<Arc<UdpSocket>> {
        self.udp.lock().expect("PathHub.udp 锁中毒").clone()
    }

    pub fn set_own_vip(&self, vip: Ipv4Addr) {
        *self.own_vip.lock().expect("PathHub.own_vip 锁中毒") = Some(vip);
    }

    /// 本节点虚拟 IP (转发帧的源字段)。
    pub fn own_vip(&self) -> Option<Ipv4Addr> {
        *self.own_vip.lock().expect("PathHub.own_vip 锁中毒")
    }

    /// 登记中继连接 (中继号由运行时按列表位置分配)。
    pub fn register_relay(&self, id: u8, tx: mpsc::Sender<Vec<u8>>) {
        self.relays
            .lock()
            .expect("PathHub.relays 锁中毒")
            .insert(id, tx);
    }

    pub fn unregister_relay(&self, id: u8) {
        self.relays
            .lock()
            .expect("PathHub.relays 锁中毒")
            .remove(&id);
    }

    /// 中继号是否在线 (回落选择用)。
    pub fn relay_online(&self, id: u8) -> bool {
        self.relays
            .lock()
            .expect("PathHub.relays 锁中毒")
            .contains_key(&id)
    }

    /// 在线中继号快照 (地址序)。
    pub fn online_relay_ids(&self) -> Vec<u8> {
        let mut ids: Vec<u8> = self
            .relays
            .lock()
            .expect("PathHub.relays 锁中毒")
            .keys()
            .copied()
            .collect();
        ids.sort_unstable();
        ids
    }

    /// 经中继发一帧; 未登记 / 慢消费按 UDP 语义静默丢弃。
    pub fn send_relay_frame(&self, id: u8, dst: Ipv4Addr, payload: &[u8]) {
        let Some(own) = self.own_vip() else {
            return;
        };
        let frame = kunxtier_core::protocol::encode_forward_frame(dst, own, payload);
        let mut relays = self.relays.lock().expect("PathHub.relays 锁中毒");
        if let Some(tx) = relays.get_mut(&id) {
            let _ = tx.try_send(frame);
        }
    }

    /// 中继入向帧回灌 (RelayConn 会话调用)。
    pub(crate) fn relay_inbound(&self, relay_id: u8, src_vip: Ipv4Addr, payload: &[u8]) {
        let tx = self
            .inbound
            .lock()
            .expect("PathHub.inbound 锁中毒")
            .0
            .clone();
        let _ = tx.try_send(RelayInbound {
            relay_id,
            src_vip,
            payload: payload.to_vec(),
        });
    }

    /// 引擎绑定取走入向接收端; 重复绑定 (端口变更) 时整组换新通道。
    pub(crate) fn take_inbound_rx(&self) -> mpsc::Receiver<RelayInbound> {
        let mut slot = self.inbound.lock().expect("PathHub.inbound 锁中毒");
        match slot.1.take() {
            Some(rx) => rx,
            None => {
                let (tx, rx) = mpsc::channel(INBOUND_CAPACITY);
                slot.0 = tx;
                rx
            }
        }
    }

    fn emit_probe(&self, from: SocketAddr, frame: &[u8]) {
        let _ = self.probe_tx.try_send(ProbeEvent {
            from,
            frame: frame.to_vec(),
        });
    }

    fn emit_stun(&self, from: SocketAddr, bytes: &[u8]) {
        let _ = self.stun_tx.try_send(StunEvent {
            from,
            bytes: bytes.to_vec(),
        });
    }

    fn emit_activity(&self, from: SocketAddr) {
        let _ = self.activity_tx.try_send(from);
    }

    /// 中继伪端点: 240.{中继号}.{vip 第三四段}:{vip 第二段}。
    /// 240/4 为保留段不会与真实地址混淆; vip 约束在 100.64.0.0/10,
    /// 第二段 64..=127 可安全落在端口范围内。
    pub fn pseudo_endpoint(relay_id: u8, vip: Ipv4Addr) -> SocketAddr {
        let o = vip.octets();
        SocketAddr::from((Ipv4Addr::new(240, relay_id, o[2], o[3]), o[1] as u16))
    }

    /// 伪端点还原 (中继号, 对端虚拟 IP); 非伪端点返回 None。
    pub fn split_pseudo(addr: SocketAddr) -> Option<(u8, Ipv4Addr)> {
        let ip = match addr.ip() {
            std::net::IpAddr::V4(v4) => v4,
            std::net::IpAddr::V6(_) => return None,
        };
        let o = ip.octets();
        if o[0] != 240 {
            return None;
        }
        // 端口还原 vip 第二段; 前两段由地址段约定补齐
        Some((o[1], Ipv4Addr::new(100, addr.port() as u8, o[2], o[3])))
    }
}

/// 复合传输工厂: 直连 socket + 中继伪端点路由。
pub struct PathUdpFactory {
    hub: Arc<PathHub>,
}

impl PathUdpFactory {
    pub fn new(hub: Arc<PathHub>) -> Self {
        PathUdpFactory { hub }
    }
}

impl UdpTransportFactory for PathUdpFactory {
    type SendV4 = PathSend;
    type SendV6 = V6Send;
    type RecvV4 = PathRecv;
    type RecvV6 = V6Recv;

    async fn bind(
        &mut self,
        params: &UdpTransportFactoryParams,
    ) -> io::Result<((Self::SendV4, Self::RecvV4), (Self::SendV6, Self::RecvV6))> {
        let v4 = Arc::new(UdpSocket::bind(SocketAddr::from((params.addr_v4, params.port))).await?);
        self.hub.set_udp(v4.clone());
        // v6 仅为满足引擎双栈装配: 绑定失败时以空实现兜底, 不阻断 v4 数据面
        let v6 = match UdpSocket::bind(SocketAddr::from((params.addr_v6, params.port))).await {
            Ok(s) => Some(Arc::new(s)),
            Err(_) => None,
        };
        let send = PathSend {
            hub: self.hub.clone(),
            udp: v4.clone(),
        };
        let recv = PathRecv {
            hub: self.hub.clone(),
            udp: v4,
            inbound: self.hub.take_inbound_rx(),
            seen: HashSet::new(),
        };
        let v6_send = V6Send { sock: v6.clone() };
        let v6_recv = V6Recv { sock: v6 };
        Ok(((send, recv), (v6_send, v6_recv)))
    }
}

/// 直连出向: 伪端点走中继, 其余走真实 UDP。
#[derive(Clone)]
pub struct PathSend {
    hub: Arc<PathHub>,
    udp: Arc<UdpSocket>,
}

impl UdpSend for PathSend {
    type SendManyBuf = ();

    async fn send_to(&self, packet: Packet, target: SocketAddr) -> io::Result<()> {
        if let Some((relay_id, dst_vip)) = PathHub::split_pseudo(target) {
            self.hub.send_relay_frame(relay_id, dst_vip, &packet);
            Ok(())
        } else {
            self.udp.send_to(&packet, target).await.map(|_| ())
        }
    }

    fn local_addr(&self) -> io::Result<Option<SocketAddr>> {
        self.udp.local_addr().map(Some)
    }
}

/// 直连入向: 分类分流 + 中继入向合流。
pub struct PathRecv {
    hub: Arc<PathHub>,
    udp: Arc<UdpSocket>,
    inbound: mpsc::Receiver<RelayInbound>,
    /// 已上报过的直连 WG 来源 (去重; 超上限后停止上报新来源)。
    seen: HashSet<SocketAddr>,
}

impl UdpRecv for PathRecv {
    type RecvManyBuf = ();

    async fn recv_from(&mut self, pool: &mut PacketBufPool) -> io::Result<(Packet, SocketAddr)> {
        let mut inbound = std::pin::pin!(self.inbound.recv());
        loop {
            let mut buf = pool.get();
            tokio::select! {
                biased;
                frame = &mut inbound.as_mut() => {
                    // 中继入向: 以伪端点呈现给引擎 (端点漂移即路径切换)
                    let Some(frame) = frame else {
                        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "中继入向通道关闭"));
                    };
                    let body = buf.buf_mut();
                    body.clear();
                    body.extend_from_slice(&frame.payload);
                    return Ok((buf, PathHub::pseudo_endpoint(frame.relay_id, frame.src_vip)));
                }
                received = self.udp.recv_from(&mut buf) => {
                    let (n, src) = received?;
                    buf.truncate(n);
                    if punch::is_probe(&buf) {
                        self.hub.emit_probe(src, &buf);
                        continue;
                    }
                    if stun::is_stun_response(&buf) {
                        self.hub.emit_stun(src, &buf);
                        continue;
                    }
                    // WG 帧: 首见来源上报 (打洞成功 / 端点漂移判据)
                    if self.seen.len() < ACTIVITY_SOURCES_CAP && self.seen.insert(src) {
                        self.hub.emit_activity(src);
                    }
                    return Ok((buf, src));
                }
            }
        }
    }
}

/// v6 出向空实现兜底。
#[derive(Clone)]
pub struct V6Send {
    sock: Option<Arc<UdpSocket>>,
}

impl UdpSend for V6Send {
    type SendManyBuf = ();

    async fn send_to(&self, _packet: Packet, _target: SocketAddr) -> io::Result<()> {
        match &self.sock {
            Some(s) => s.send_to(&_packet, _target).await.map(|_| ()),
            None => Err(io::Error::new(io::ErrorKind::AddrNotAvailable, "v6 未绑定")),
        }
    }

    fn local_addr(&self) -> io::Result<Option<SocketAddr>> {
        match &self.sock {
            Some(s) => s.local_addr().map(Some),
            None => Ok(None),
        }
    }
}

/// v6 入向空实现兜底: 永不完成 (v6 数据面为后续演进)。
pub struct V6Recv {
    sock: Option<Arc<UdpSocket>>,
}

impl UdpRecv for V6Recv {
    type RecvManyBuf = ();

    async fn recv_from(&mut self, pool: &mut PacketBufPool) -> io::Result<(Packet, SocketAddr)> {
        match &self.sock {
            Some(s) => {
                let mut buf = pool.get();
                let (n, src) = s.recv_from(&mut buf).await?;
                buf.truncate(n);
                Ok((buf, src))
            }
            None => std::future::pending().await,
        }
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn vip(a: u8, b: u8, c: u8) -> Ipv4Addr {
        Ipv4Addr::new(100, a, b, c)
    }

    #[test]
    fn pseudo_endpoint_roundtrip() {
        for (relay_id, peer) in [(1u8, vip(64, 0, 1)), (7, vip(100, 200, 3))] {
            let pseudo = PathHub::pseudo_endpoint(relay_id, peer);
            let (id, back) = PathHub::split_pseudo(pseudo).expect("伪端点必须可还原");
            assert_eq!(id, relay_id);
            assert_eq!(back, peer);
            // 伪端点必落在 240/4 保留段
            assert!(pseudo.ip().to_string().starts_with("240."));
        }
    }

    #[test]
    fn split_pseudo_rejects_real_addresses() {
        assert!(PathHub::split_pseudo("1.2.3.4:51820".parse().unwrap()).is_none());
        assert!(PathHub::split_pseudo("100.64.0.1:51820".parse().unwrap()).is_none());
    }

    #[test]
    fn stun_classification_away_from_wg() {
        // STUN Binding 成功响应: 01 01 + cookie
        let mut resp = vec![0u8; 20];
        resp[0] = 0x01;
        resp[1] = 0x01;
        resp[4..8].copy_from_slice(&stun::MAGIC_COOKIE);
        assert!(stun::is_stun_response(&resp));
        // WG initiation: 首字节 01, 后三字节保留零, 无 cookie
        let mut wg = vec![0u8; 32];
        wg[0] = 0x01;
        assert!(!stun::is_stun_response(&wg));
        // WG data: 首字节 04
        let mut data = vec![0u8; 48];
        data[0] = 0x04;
        assert!(!stun::is_stun_response(&data));
    }

    #[test]
    fn probe_frames_route_to_probe_channel() {
        let (hub, mut streams) = PathHub::new();
        hub.emit_probe("1.2.3.4:5".parse().unwrap(), b"KXT1payload");
        let ev = streams.probe.try_recv().expect("探测事件必须可达");
        assert_eq!(ev.from, "1.2.3.4:5".parse().unwrap());
        assert_eq!(ev.frame, b"KXT1payload".to_vec());
    }
}
