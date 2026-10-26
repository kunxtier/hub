//! Description: 本地网段冲突检测 (FR-14): 本机接口地址与所属网络 /16 的重叠比对。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 主场景是运营商 CGNAT: 蜂窝 / 部分家宽出口直接把 100.64.0.0/10 段地址配到
//! 本机接口, 与 kunxtier 虚拟网段天然重叠。检测不通过即拒绝启动, 避免
//! "装完即断网" 且难以排查 (判定逻辑纯函数化, 供单测与 T09 daemon 启动前置复用)。
//!
//! 边界: MVP 只比对接口地址 (含掩码网络); 系统路由表全量解析跨平台脆弱,
//! 误拒比漏报代价更高, 列为 e2e 反馈项。

use std::net::Ipv4Addr;

/// 单条接口地址的归一化视图 (纯判定函数的输入)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfaceAddr {
    pub name: String,
    pub ip: Ipv4Addr,
    /// 前缀长度; 掩码无意义 (/0, 部分平台 p2p 接口) 时按 /32 处理。
    pub prefix: u8,
}

/// 一处与所属网络 /16 重叠的本地地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubnetConflict {
    pub iface: String,
    pub ip: Ipv4Addr,
    pub prefix: u8,
}

impl SubnetConflict {
    /// 诊断文本 (FR-14: 冲突时输出诊断信息与处理建议)。
    pub fn describe(&self, vip: Ipv4Addr) -> String {
        let net = network16(vip);
        format!(
            "接口 {} 的地址 {}/{} 与虚拟网段 {}/16 重叠; \
             处理建议: 调整本地网络 (停用或改配该接口), \
             或由 admin 换网段重建网络后重新注册",
            self.iface, self.ip, self.prefix, net
        )
    }
}

/// 本节点虚拟 IP 所在网络的 /16 基址 (FR-1: 每网独立 /16)。
fn network16(vip: Ipv4Addr) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(vip) & 0xffff_0000)
}

/// 前缀 → 掩码。prefix 是 pub 输入字段, 越界值 (大于 32) 按 /32 主机收口,
/// 不做会下溢 panic 的移位。
fn mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else if prefix >= 32 {
        u32::MAX
    } else {
        u32::MAX << (32 - prefix as u32)
    }
}

/// 接口网络与虚拟网段是否重叠 (双向区间相交; 与掩码长度无关的统一判定)。
fn overlaps(vip: Ipv4Addr, iface: &IfaceAddr) -> bool {
    let base = u32::from(network16(vip));
    let iface_net = u32::from(iface.ip) & mask(iface.prefix);
    let iface_end = iface_net | !mask(iface.prefix);
    iface_net <= base + 0xffff && iface_end >= base
}

/// 纯判定: 接口地址集合中与所属网络 /16 重叠的条目。
/// 排除项: 地址恰为本节点虚拟 IP 的条目 (kunxtier 自身 TUN 的 /32)。
pub fn detect_with(vip: Ipv4Addr, ifaces: &[IfaceAddr]) -> Vec<SubnetConflict> {
    ifaces
        .iter()
        .filter(|iface| iface.ip != vip && overlaps(vip, iface))
        .map(|iface| SubnetConflict {
            iface: iface.name.clone(),
            ip: iface.ip,
            prefix: iface.prefix,
        })
        .collect()
}

/// 枚举本机接口并检测 (daemon 启动路径)。
/// 接口枚举失败按无冲突处理: 冲突拒绝启动是防断网保护, 枚举失败不应阻断组网。
pub fn detect_conflicts(vip: Ipv4Addr) -> Vec<SubnetConflict> {
    let Ok(list) = if_addrs::get_if_addrs() else {
        tracing::warn!("本机接口枚举失败, 跳过网段冲突检测");
        return Vec::new();
    };
    let ifaces: Vec<IfaceAddr> = list
        .iter()
        .filter_map(|iface| match &iface.addr {
            if_addrs::IfAddr::V4(v4) => Some(IfaceAddr {
                name: iface.name.clone(),
                ip: v4.ip,
                prefix: host_prefix(v4.prefixlen),
            }),
            if_addrs::IfAddr::V6(_) => None,
        })
        .collect();
    detect_with(vip, &ifaces)
}

/// /0 掩码 (无意义, p2p 接口偶发) 按 /32 单地址处理, 避免 "与任意网段重叠"。
fn host_prefix(prefixlen: u8) -> u8 {
    if prefixlen == 0 {
        32
    } else {
        prefixlen
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, ip: &str, prefix: u8) -> IfaceAddr {
        IfaceAddr {
            name: name.into(),
            ip: ip.parse().unwrap(),
            prefix,
        }
    }

    fn vip(ip: &str) -> Ipv4Addr {
        ip.parse().unwrap()
    }

    #[test]
    fn cgnat_subnet_overlaps_virtual_net() {
        // 运营商 CGNAT: 100.64.0.0/10 整段配在本机, 与任意所属网络 /16 重叠
        let conflicts = detect_with(vip("100.100.5.5"), &[iface("cellular0", "100.64.0.1", 10)]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].iface, "cellular0");
        assert!(conflicts[0]
            .describe(vip("100.100.5.5"))
            .contains("cellular0"));
    }

    #[test]
    fn point_to_point_host_address_in_net_conflicts() {
        // LTE / 运营商 p2p 接口常为 /32 单地址
        let conflicts = detect_with(vip("100.100.5.5"), &[iface("wwan0", "100.100.0.7", 32)]);
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn other_networks_do_not_conflict() {
        let ifaces = vec![
            iface("en0", "192.168.1.10", 24),
            iface("en1", "10.0.0.5", 8),
            // 相邻虚拟网段 (100.101/16 vs 100.100/16) 不重叠
            iface("utun9", "100.101.0.1", 16),
            iface("lo0", "127.0.0.1", 8),
        ];
        assert!(detect_with(vip("100.100.5.5"), &ifaces).is_empty());
    }

    #[test]
    fn wide_local_prefix_covering_virtual_net_conflicts() {
        // 超网掩码 (如 100.0.0.0/8) 同样覆盖虚拟网段
        let conflicts = detect_with(vip("100.100.5.5"), &[iface("en0", "100.1.2.3", 8)]);
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn own_tun_address_is_excluded() {
        // kunxtier 自身 TUN: 地址 == 本节点虚拟 IP, 是 /32 自指不算冲突
        let ifaces = vec![iface("utun8", "100.100.5.5", 32)];
        assert!(detect_with(vip("100.100.5.5"), &ifaces).is_empty());
    }

    #[test]
    fn oversized_prefix_is_clamped_to_host() {
        // prefix 为 pub 字段, 越界值按 /32 单地址处理, 不 panic 不误扩大范围
        let in_net = detect_with(vip("100.100.5.5"), &[iface("x", "100.100.0.9", 40)]);
        assert_eq!(in_net.len(), 1);
        let out_net = detect_with(vip("100.100.5.5"), &[iface("x", "100.101.0.9", 40)]);
        assert!(out_net.is_empty());
    }
}
