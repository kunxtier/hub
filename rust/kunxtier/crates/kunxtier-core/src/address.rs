//! Description: 虚拟地址分配: 每网络一个 /16, 取自 100.64.0.0/10 (CGNAT 段);
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 网络内 /32 顺序递增分配, 不回收复用 (FR-17 保护窗口的最简实现)。

use std::net::Ipv4Addr;

use crate::error::{Error, Result};

/// 管理池: 100.64.0.0/10。
pub const POOL_BASE: u32 = (100 << 24) | (64 << 16);
pub const POOL_PREFIX: u8 = 10;
/// 每网络占用的前缀长度 (FR-1)。
pub const NETWORK_PREFIX: u8 = 16;
/// 每网络可用 /32 上限。
pub const HOSTS_PER_NETWORK: u32 = 1 << 16;

pub fn ipv4_to_u32(ip: Ipv4Addr) -> u32 {
    u32::from(ip)
}

pub fn ipv4_from_u32(v: u32) -> Ipv4Addr {
    Ipv4Addr::from(v)
}

fn ensure_in_pool(base: u32) -> Result<()> {
    let host_bits = 32 - POOL_PREFIX;
    if base & (u32::MAX << host_bits) != POOL_BASE {
        return Err(Error::Address("网段必须落在 100.64.0.0/10 内".into()));
    }
    Ok(())
}

/// 解析 "a.b.c.d/16"; 仅接受 /16 且落在管理池内。
pub fn parse_cidr(s: &str) -> Result<u32> {
    let (base, prefix) = s
        .split_once('/')
        .ok_or_else(|| Error::Address("缺少前缀长度, 期望 a.b.c.d/16".into()))?;
    let base: Ipv4Addr = base
        .parse()
        .map_err(|_| Error::Address(format!("非法地址: {base}")))?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| Error::Address(format!("非法前缀: {prefix}")))?;
    if prefix != NETWORK_PREFIX {
        return Err(Error::Address("MVP 仅支持 /16 网段".into()));
    }
    let b = ipv4_to_u32(base);
    ensure_in_pool(b)?;
    Ok(b)
}

/// 第 n 个 (从 0 计) 可用 /16 的基地址, 供新网络按创建顺序递增取段。
pub fn nth_network_base(n: u32) -> Result<u32> {
    let base = POOL_BASE + n * 0x1_0000;
    ensure_in_pool(base)?;
    Ok(base)
}

/// 网络内第 offset (从 1 计) 个 /32。
pub fn host_at(base: u32, offset: u32) -> Result<Ipv4Addr> {
    if offset == 0 || offset >= HOSTS_PER_NETWORK {
        return Err(Error::Address("网段地址耗尽".into()));
    }
    Ok(ipv4_from_u32(base + offset))
}

/// 判断某地址是否落入指定网络的 /16 (节点侧 CGNAT 冲突检测使用, FR-14)。
pub fn in_network(base: u32, ip: Ipv4Addr) -> bool {
    (ipv4_to_u32(ip) & 0xFFFF_0000) == (base & 0xFFFF_0000)
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cidr_accepts_pool_16() {
        assert_eq!(parse_cidr("100.64.0.0/16").unwrap(), POOL_BASE);
        assert_eq!(
            parse_cidr("100.127.0.0/16").unwrap(),
            (127 << 16) | (100 << 24)
        );
    }

    #[test]
    fn parse_cidr_rejects_out_of_pool_or_wrong_prefix() {
        assert!(parse_cidr("100.128.0.0/16").is_err());
        assert!(parse_cidr("10.0.0.0/16").is_err());
        assert!(parse_cidr("100.64.0.0/24").is_err());
        assert!(parse_cidr("100.64.0.0").is_err());
    }

    #[test]
    fn nth_network_base_bounds() {
        assert_eq!(nth_network_base(0).unwrap(), POOL_BASE);
        assert_eq!(nth_network_base(1).unwrap(), POOL_BASE + 0x1_0000);
        assert!(nth_network_base(64).is_err());
    }

    #[test]
    fn host_at_bounds() {
        let base = POOL_BASE;
        assert_eq!(host_at(base, 1).unwrap().to_string(), "100.64.0.1");
        assert!(host_at(base, 0).is_err());
        assert!(host_at(base, HOSTS_PER_NETWORK).is_err());
    }

    #[test]
    fn in_network_membership() {
        let base = POOL_BASE;
        assert!(in_network(base, "100.64.3.4".parse().unwrap()));
        assert!(!in_network(base, "100.65.0.1".parse().unwrap()));
        assert!(!in_network(base, "192.168.1.1".parse().unwrap()));
    }
}
