//! Description: WireGuard 引擎装配与运行时 peer 管理 (基于 gotatun Device)。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 引擎持有节点 WG 私钥与 UDP 监听, 对外暴露三个驱动面:
//! - sync_peers: 信令节点表驱动的全量对齐 (T04 State 推送的消费端)
//! - set_endpoint: 打洞结果回填 (T07 并发打洞的落点)
//! - listen_port: 候选地址上报 (T07 STUN 探测的端口来源)
//! 传输层保持可插拔: 生产路径为真实 UDP + TUN 适配; 中继路径传输随 T08 以
//! 自定义 UdpTransportFactory 接入, 引擎层不感知路径形态。

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use gotatun::device::{Device, DeviceBuilder, DeviceTransports, Peer as WgPeer};
use gotatun::tun::{IpRecv, IpSend};
use gotatun::udp::UdpTransportFactory;
use gotatun::x25519::{PublicKey, StaticSecret};
use ipnetwork::IpNetwork;

use crate::error::{Error, Result};

/// 引擎装配参数。listen_port 为 UDP 固定监听端口 (必须非 0):
/// 0.7.1 语义下端口固定才能支撑打洞与候选地址上报; 绑定延迟到首次
/// sync_peers (经 set_listen_port 0→P 变更触发连接建立)。
#[derive(Clone)]
pub struct EngineOptions {
    pub private_key: StaticSecret,
    pub listen_port: u16,
}

/// 信令节点表条目到引擎 peer 的映射单位。
#[derive(Debug, Clone)]
pub struct PeerSpec {
    pub public_key: PublicKey,
    /// 对端虚拟 IP; 引擎内落地为 /32 allowed ip。
    pub virtual_ip: Ipv4Addr,
    /// 已知对端地址 (打洞结果或信令候选); None 表示尚未知, 等待入向握手或打洞回填。
    pub endpoint: Option<SocketAddr>,
    /// 直连 persistent keepalive 秒数; FR-15 默认 25s 量级, 由装配层决定。
    pub keepalive_secs: Option<u16>,
}

impl PeerSpec {
    fn allowed_ip(&self) -> Result<IpNetwork> {
        host_network(self.virtual_ip)
    }

    fn to_wg_peer(&self) -> Result<WgPeer> {
        let mut peer = WgPeer::new(self.public_key);
        peer.allowed_ips = vec![self.allowed_ip()?];
        peer.endpoint = self.endpoint;
        peer.keepalive = self.keepalive_secs;
        Ok(peer)
    }
}

/// 节点虚拟 IP 的 /32 主机路由。
fn host_network(ip: Ipv4Addr) -> Result<IpNetwork> {
    IpNetwork::new(IpAddr::V4(ip), 32)
        .map_err(|e| Error::Config(format!("虚拟 IP 无法构成 /32 路由: {e}")))
}

/// 运行中的 WireGuard 引擎句柄。泛型参数为传输层组合 (UDP 工厂 + IP 收发)。
pub struct WgEngine<T: DeviceTransports> {
    device: Device<T>,
    /// 约定的固定监听端口; 首次节点表对齐时落地绑定。
    listen_port: u16,
}

impl<T: DeviceTransports> WgEngine<T> {
    /// 约定的 UDP 监听端口 (构造时给定值, 非运行时探测值)。
    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    /// 节点表全量对齐: 摘除已离表 peer, 对齐表内 peer 的 allowed ip 与 keepalive。
    /// spec 未声明 endpoint 时不覆盖已学到的端点 (打洞结果与入向握手学习保持)。
    /// 首次对齐时落地 UDP 监听 (gotatun 0.7.1 运行时加 peer 不触发连接建立,
    /// 借道 set_listen_port 的端口变更语义完成)。
    pub async fn sync_peers(&self, peers: &[PeerSpec]) -> Result<()> {
        let port = self.listen_port;
        let keep: HashSet<&PublicKey> = peers.iter().map(|p| &p.public_key).collect();
        let applied: Result<()> = self
            .device
            .write(async |w| {
                w.set_listen_port(port);
                for stat in w.peers().await {
                    let key = &stat.peer.public_key;
                    if !keep.contains(key) {
                        w.remove_peer(key).await;
                    }
                }
                for spec in peers {
                    let network = spec.allowed_ip()?;
                    let updated = w
                        .modify_peer(&spec.public_key, |p| {
                            p.clear_allowed_ips();
                            p.add_allowed_ip(network);
                            p.set_keepalive(spec.keepalive_secs);
                            if spec.endpoint.is_some() {
                                p.set_endpoint(spec.endpoint);
                            }
                        })
                        .await;
                    if !updated {
                        w.add_peer(spec.to_wg_peer()?);
                    }
                }
                Ok(())
            })
            .await?;
        applied
    }

    /// 回填对端端点 (打洞成功 / 候选切换); None 表示清除端点回到等待入向握手。
    pub async fn set_endpoint(
        &self,
        public_key: &PublicKey,
        endpoint: Option<SocketAddr>,
    ) -> Result<bool> {
        self.device
            .write(async |w| {
                w.modify_peer(public_key, |p| p.set_endpoint(endpoint))
                    .await
            })
            .await
            .map_err(Error::from)
    }

    /// 停止引擎并释放传输资源。
    pub async fn stop(self) {
        self.device.stop().await;
    }
}

/// 生产路径装配: 自定义 UDP 工厂 + 单一 Clone 的 IP 收发 (如 TunAdapter)。
pub async fn build_engine<UF, Ip>(
    options: EngineOptions,
    udp: UF,
    ip: Ip,
) -> Result<WgEngine<(UF, Ip, Ip)>>
where
    UF: UdpTransportFactory,
    Ip: IpSend + IpRecv + Clone,
{
    let device = assemble(options, gotatun::device::build().with_udp(udp).with_ip(ip)).await?;
    Ok(device)
}

/// 分离收发装配: 测试通道与后续复合传输 (直连 + 中继) 使用。
pub async fn build_engine_split<UF, Tx, Rx>(
    options: EngineOptions,
    udp: UF,
    ip_tx: Tx,
    ip_rx: Rx,
) -> Result<WgEngine<(UF, Tx, Rx)>>
where
    UF: UdpTransportFactory,
    Tx: IpSend,
    Rx: IpRecv,
{
    let device = assemble(
        options,
        gotatun::device::build()
            .with_udp(udp)
            .with_ip_pair(ip_tx, ip_rx),
    )
    .await?;
    Ok(device)
}

/// 统一收尾装配: 校验端口约定, 注入私钥; 端口留给首次 sync_peers 落地。
async fn assemble<UF, Tx, Rx>(
    options: EngineOptions,
    builder: DeviceBuilder<UF, Tx, Rx>,
) -> Result<WgEngine<(UF, Tx, Rx)>>
where
    UF: UdpTransportFactory,
    Tx: IpSend,
    Rx: IpRecv,
{
    if options.listen_port == 0 {
        return Err(Error::Config(
            "listen_port 必须非 0: 固定端口是打洞与候选上报的前提".into(),
        ));
    }
    let device = builder
        .with_private_key(options.private_key)
        .build()
        .await?;
    Ok(WgEngine {
        device,
        listen_port: options.listen_port,
    })
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(endpoint: Option<SocketAddr>) -> PeerSpec {
        PeerSpec {
            public_key: WgIdentityForTest::public_key(),
            virtual_ip: "100.64.0.7".parse().unwrap(),
            endpoint,
            keepalive_secs: Some(25),
        }
    }

    /// 测试用身份: 与 identity 模块解耦, 只需要稳定公钥。
    struct WgIdentityForTest;

    impl WgIdentityForTest {
        fn public_key() -> PublicKey {
            crate::identity::WgIdentity::generate().public_key()
        }
    }

    #[test]
    fn peer_spec_maps_to_wg_peer_fields() {
        let endpoint: SocketAddr = "1.2.3.4:51820".parse().unwrap();
        let peer = spec(Some(endpoint)).to_wg_peer().unwrap();
        assert_eq!(peer.endpoint, Some(endpoint));
        assert_eq!(peer.keepalive, Some(25));
        assert_eq!(
            peer.allowed_ips,
            vec![host_network("100.64.0.7".parse().unwrap()).unwrap()]
        );
    }

    #[test]
    fn host_network_is_slash_32() {
        let network = host_network("100.100.7.7".parse().unwrap()).unwrap();
        assert_eq!(network.prefix(), 32);
    }
}
