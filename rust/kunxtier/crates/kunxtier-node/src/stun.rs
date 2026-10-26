//! Description: STUN 最小客户端 (RFC 5389 子集): Binding Request / XOR-MAPPED-ADDRESS。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 只做打洞所需的 "本机地址映射探测": 无属性 Binding 请求 + 响应解析,
//! 不引入完整 STUN 栈。请求与响应走 WG 同一 UDP socket (transport 分流),
//! 保证探测到的映射端口与数据面一致。服务器列表来自核心配置, 经信令下发。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use crate::transport::StunEvent;

/// RFC 5389 魔数 cookie。
pub const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];
const HEADER_LEN: usize = 20;
const BINDING_REQUEST: [u8; 2] = [0x00, 0x01];
const BINDING_SUCCESS: [u8; 2] = [0x01, 0x01];
/// 单轮探测总时限: 各服务器并发请求, 1s 时补发一轮, 2s 截止。
const PROBE_DEADLINE: Duration = Duration::from_secs(2);
const RETRANSMIT_AT: Duration = Duration::from_secs(1);

/// STUN 响应判定 (transport 分流共用): Binding 成功类型 + 魔数双重确认,
/// 与 WG 帧 (首字节 1..=4 且后三字节保留为零) 无交集。
pub fn is_stun_response(b: &[u8]) -> bool {
    b.len() >= HEADER_LEN && b[..2] == BINDING_SUCCESS && b[4..8] == MAGIC_COOKIE
}

/// 构造无属性 Binding Request: 头 20 字节, 事务 ID 随机。
pub fn binding_request(txid: &[u8; 12]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(HEADER_LEN);
    msg.extend_from_slice(&BINDING_REQUEST);
    msg.extend_from_slice(&0u16.to_be_bytes()); // 无属性, 长度为零
    msg.extend_from_slice(&MAGIC_COOKIE);
    msg.extend_from_slice(txid);
    msg
}

/// 解析 Binding 成功响应: 优先 XOR-MAPPED-ADDRESS, 兜底 MAPPED-ADDRESS。
/// 事务 ID 不匹配 (串扰的他人响应) 返回 None。
pub fn parse_mapped_address(resp: &[u8], txid: &[u8; 12]) -> Option<SocketAddr> {
    if !is_stun_response(resp) || resp[8..20] != *txid {
        return None;
    }
    let body_len = u16::from_be_bytes([resp[2], resp[3]]) as usize;
    let body = resp.get(HEADER_LEN..HEADER_LEN + body_len)?;
    let mut mapped = None;
    let mut offset = 0;
    while offset + 4 <= body.len() {
        let attr_type = u16::from_be_bytes([body[offset], body[offset + 1]]);
        let attr_len = u16::from_be_bytes([body[offset + 2], body[offset + 3]]) as usize;
        let value = body.get(offset + 4..offset + 4 + attr_len)?;
        match attr_type {
            0x0020 => {
                mapped = parse_address(value, true);
                break; // XOR 形态优先, 命中即止
            }
            0x0001 if mapped.is_none() => mapped = parse_address(value, false),
            _ => {}
        }
        // 属性按 4 字节对齐填充
        offset += 4 + attr_len.div_ceil(4) * 4;
    }
    mapped
}

/// 解析地址属性值: [族 1B][端口 2B][IPv4 4B] (尾部对齐补位容忍);
/// xor 为真时按 cookie 异或还原。
fn parse_address(value: &[u8], xor: bool) -> Option<SocketAddr> {
    if value.len() < 7 || value[0] != 0x01 {
        return None;
    }
    let port = u16::from_be_bytes([value[1], value[2]]);
    let addr = u32::from_be_bytes([value[3], value[4], value[5], value[6]]);
    let (port, addr) = if xor {
        (
            port ^ u16::from_be_bytes([MAGIC_COOKIE[0], MAGIC_COOKIE[1]]),
            addr ^ u32::from_be_bytes(MAGIC_COOKIE),
        )
    } else {
        (port, addr)
    };
    Some(SocketAddr::from((std::net::Ipv4Addr::from(addr), port)))
}

/// 并发探测全部服务器, 返回去重后的映射地址集合。
/// host:port 形态逐个解析 (域名失败即跳过该台), 结果缺失不阻断节点运行。
pub async fn probe_mapped_addresses(
    servers: &[String],
    udp: Arc<UdpSocket>,
    events: &mut mpsc::Receiver<StunEvent>,
) -> Vec<SocketAddr> {
    let targets = resolve_targets(servers).await;
    if targets.is_empty() {
        return Vec::new();
    }
    // 每台一个事务: 同源响应按事务 ID 归位
    let txids: Vec<[u8; 12]> = (0..targets.len())
        .map(|_| {
            let mut txid = [0u8; 12];
            getrandom_fill(&mut txid);
            txid
        })
        .collect();
    let send_all = |targets: &[SocketAddr], txids: &[[u8; 12]]| {
        for (addr, txid) in targets.iter().zip(txids) {
            let _ = udp.try_send_to(&binding_request(txid), *addr);
        }
    };
    send_all(&targets, &txids);

    // 两阶段: 1s 前收集, 到点整组补发一次, 2s 截止
    let start = tokio::time::Instant::now();
    let mut mapped = Vec::new();
    for phase in 0..2 {
        let phase_deadline = if phase == 0 {
            start + RETRANSMIT_AT
        } else {
            start + PROBE_DEADLINE
        };
        if collect_until(&targets, &txids, events, phase_deadline, &mut mapped).await {
            return mapped;
        }
        if phase == 0 {
            send_all(&targets, &txids);
        }
    }
    mapped
}

/// 解析服务器列表为拨号地址 (域名失败即跳过该台)。
async fn resolve_targets(servers: &[String]) -> Vec<SocketAddr> {
    let mut targets = Vec::with_capacity(servers.len());
    for server in servers {
        match tokio::net::lookup_host(server).await {
            Ok(mut addrs) => {
                if let Some(addr) = addrs.next() {
                    targets.push(addr);
                }
            }
            Err(e) => tracing::debug!(server = %server, error = %e, "STUN 服务器解析失败"),
        }
    }
    targets
}

/// 收集到截止时刻; 返回 true 表示已收满 (4 个映射或事件流关闭)。
async fn collect_until(
    targets: &[SocketAddr],
    txids: &[[u8; 12]],
    events: &mut mpsc::Receiver<StunEvent>,
    deadline: tokio::time::Instant,
    mapped: &mut Vec<SocketAddr>,
) -> bool {
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Err(_) => return false,
            Ok(None) => return true,
            Ok(Some(event)) => {
                let Some(idx) = targets
                    .iter()
                    .zip(txids)
                    .position(|(t, txid)| *t == event.from && event.bytes[8..20] == txid[..])
                else {
                    continue;
                };
                if let Some(addr) = parse_mapped_address(&event.bytes, &txids[idx]) {
                    if !mapped.contains(&addr) {
                        tracing::debug!(server = %targets[idx], mapped = %addr, "STUN 映射探测成功");
                        mapped.push(addr);
                        if mapped.len() >= 4 {
                            return true;
                        }
                    }
                }
            }
        }
    }
}

/// 事务 ID 随机填充 (rand::fill 语义随版本漂移, 收口一处)。
fn getrandom_fill(buf: &mut [u8]) {
    use rand::RngCore;
    rand::rng().fill_bytes(buf);
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn success_response(txid: &[u8; 12], xor_addr: SocketAddr) -> Vec<u8> {
        // XOR-MAPPED-ADDRESS 属性: 值 = [族][端口^cookie 高位][地址^cookie], 4 字节对齐补一位
        let ip = match xor_addr.ip() {
            std::net::IpAddr::V4(v4) => v4,
            _ => panic!("测试仅用 v4"),
        };
        let o = ip.octets();
        let port = xor_addr.port() ^ u16::from_be_bytes([MAGIC_COOKIE[0], MAGIC_COOKIE[1]]);
        let mut attr = vec![0x00, 0x20, 0x00, 0x08, 0x01];
        attr.extend_from_slice(&port.to_be_bytes());
        for i in 0..4 {
            attr.push(o[i] ^ MAGIC_COOKIE[i]);
        }
        attr.push(0); // 对齐补位

        let mut msg = Vec::with_capacity(HEADER_LEN + attr.len());
        msg.extend_from_slice(&BINDING_SUCCESS);
        msg.extend_from_slice(&(attr.len() as u16).to_be_bytes());
        msg.extend_from_slice(&MAGIC_COOKIE);
        msg.extend_from_slice(txid);
        msg.extend_from_slice(&attr);
        msg
    }

    #[test]
    fn request_header_shape() {
        let txid = [7u8; 12];
        let msg = binding_request(&txid);
        assert_eq!(msg.len(), HEADER_LEN);
        assert_eq!(&msg[..2], &BINDING_REQUEST);
        assert_eq!(msg[2..4], [0, 0]);
        assert_eq!(msg[4..8], MAGIC_COOKIE);
        assert_eq!(msg[8..20], txid);
    }

    #[test]
    fn parse_xor_mapped_address() {
        let txid = [9u8; 12];
        let expect: SocketAddr = "203.0.113.7:51820".parse().unwrap();
        let resp = success_response(&txid, expect);
        assert!(is_stun_response(&resp));
        assert_eq!(parse_mapped_address(&resp, &txid), Some(expect));
        // 事务 ID 不匹配: 串扰响应不采信
        assert_eq!(parse_mapped_address(&resp, &[0u8; 12]), None);
    }

    #[test]
    fn plain_mapped_address_fallback() {
        let txid = [3u8; 12];
        let ip = std::net::Ipv4Addr::new(198, 51, 100, 9);
        // 值 = [族][端口 0x0050][地址], 长度 8 含对齐补位
        let mut attr = vec![0x00, 0x01, 0x00, 0x08, 0x01, 0x00, 0x50];
        attr.extend_from_slice(&ip.octets());
        attr.push(0);
        let mut msg = Vec::with_capacity(HEADER_LEN + attr.len());
        msg.extend_from_slice(&BINDING_SUCCESS);
        msg.extend_from_slice(&(attr.len() as u16).to_be_bytes());
        msg.extend_from_slice(&MAGIC_COOKIE);
        msg.extend_from_slice(&txid);
        msg.extend_from_slice(&attr);
        assert_eq!(
            parse_mapped_address(&msg, &txid),
            Some("198.51.100.9:80".parse().unwrap())
        );
    }
}
