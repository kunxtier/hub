//! Description: tun-rs TUN 设备接入: 设备创建与 gotatun IpSend / IpRecv 适配。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 适配方式镜像 gotatun 官方 tun 适配器 (src/tun/tun_async_device.rs), 但设备由
//! tun-rs 创建与持有: 地址 / MTU 在创建时落定, kunxtier 自持设备便于路由与
//! 冲突检测 (netconf 模块) 直接操作。MTU 取按最坏路径推导的静态值 (单一 TUN
//! 同时服务直连与中继两档路径, 无按路径动态调整的空间, 见 DEFAULT_MTU 推导)。

use std::io;
use std::iter;
use std::net::Ipv4Addr;
use std::sync::Arc;

use gotatun::packet::{Ip, Packet, PacketBufPool};
use gotatun::tun::{IpRecv, IpSend, MtuWatcher};

use crate::error::Result;

/// TUN MTU 默认值: 按最坏路径推导 (FR-16)。
/// 两条路径的约束不对称:
/// - 直连 (WG over UDP) 为硬约束: 外层典型 1500 - 60 (外层 IP + UDP + WG 封装)
///   = 1440, 取 1440 保证 WG 外层报文不分片;
/// - 中继 (WG over TLS over TCP) 叠加 TLS 记录头 (5B) + 转发帧头 (8B) +
///   TCP/IP (40B), 但 TCP 是流式传输, 由分段自然吸收, 无内层 MTU 硬上限;
///
/// 故默认取直连上限 1440, 同时覆盖 e2e 必过的 1400 字节级 ICMP 用例
/// (ICMP 载荷 1400 + 28 头 = 1428 ≤ 1440)。超过 MTU 的内层 UDP 由内核
/// 按 IP 分片拆开后进 TUN, 每片独立加密转发 (行为显式定义)。
pub const DEFAULT_MTU: u16 = 1440;

/// TUN 设备创建配置。
#[derive(Debug, Clone)]
pub struct TunConfig {
    /// 设备名; None 由平台自动分配 (macOS utunN / Linux tunN)。
    pub name: Option<String>,
    /// 本节点虚拟 IP (网络内分配的 /32)。
    pub address: Ipv4Addr,
    /// 地址前缀长度; MVP 节点地址恒为 32。
    pub prefix_len: u8,
    pub mtu: u16,
}

impl TunConfig {
    pub fn new(address: Ipv4Addr) -> Self {
        Self {
            name: None,
            address,
            prefix_len: 32,
            mtu: DEFAULT_MTU,
        }
    }
}

/// tun-rs 异步设备的 gotatun 传输适配。Clone 共享同一底层设备。
#[derive(Clone)]
pub struct TunAdapter {
    dev: Arc<tun_rs::AsyncDevice>,
    mtu: u16,
}

impl TunAdapter {
    /// 创建真实 TUN 设备; 需要操作系统特权 (macOS root / Linux CAP_NET_ADMIN)。
    pub fn open(config: &TunConfig) -> Result<Self> {
        let mut builder = tun_rs::DeviceBuilder::new()
            .ipv4(config.address, config.prefix_len, None)
            .mtu(config.mtu);
        if let Some(name) = &config.name {
            builder = builder.name(name.clone());
        }
        let dev = builder.build_async()?;
        Ok(Self {
            dev: Arc::new(dev),
            mtu: config.mtu,
        })
    }

    /// 系统侧实际设备名 (自动分配时用于回读)。
    pub fn tun_name(&self) -> io::Result<String> {
        self.dev.name()
    }
}

impl IpSend for TunAdapter {
    async fn send(&mut self, packet: Packet<Ip>) -> io::Result<()> {
        self.dev.send(&packet.into_bytes()).await?;
        Ok(())
    }
}

impl IpRecv for TunAdapter {
    async fn recv<'a>(
        &'a mut self,
        pool: &mut PacketBufPool,
    ) -> io::Result<impl Iterator<Item = Packet<Ip>> + 'a> {
        let mut packet = pool.get();
        let n = self.dev.recv(&mut packet).await?;
        packet.truncate(n);
        let packet = packet
            .try_into_ip()
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(iter::once(packet))
    }

    fn mtu(&self) -> MtuWatcher {
        MtuWatcher::new(self.mtu)
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "创建真实 TUN 设备需要特权 (macOS root / Linux CAP_NET_ADMIN), 供手动冒烟: sudo cargo test -p kunxtier-node -- --ignored"]
    fn real_tun_device_opens_with_address() {
        let config = TunConfig::new("100.64.0.1".parse().unwrap());
        let adapter = TunAdapter::open(&config).expect("TUN 设备创建失败");
        let name = adapter.tun_name().expect("读取设备名失败");
        assert!(!name.is_empty(), "设备名不应为空");
    }
}
