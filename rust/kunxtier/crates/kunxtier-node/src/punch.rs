//! Description: UDP 打洞与直连保活: 探测帧编解码 (PING/PONG) 与单 peer 路径状态机。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 探测帧与 WG / STUN 共用同一 UDP 端口 (NAT 映射一致); 帧本身不承载认证——
//! 收帧方按节点名匹配后把来源地址回填为对端端点, 真实身份由随后的 WG
//! noise 握手裁决 (伪造探测最多造成一次握手失败)。帧分两类:
//! - Ping: 主动探测 (打洞突发 / Direct 态周期保活观测);
//! - Pong: 回执, 回显触发帧的令牌; Pong 不再触发回执, 故无回声环,
//!   Direct 态节点可安全发起周期 Ping (FR-6 失活检测的观测来源)。
//!
//! 并发打洞 = 对全部候选地址周期性突发 Ping; 对受限锥形 NAT, 双侧同时发
//! 探测即可互开映射。状态机推进由 runtime 事件循环驱动, 本模块只做纯状态与编解码。
//!
//! T08 语义: Punching 携带 fallback (既有中继路径), 重试直连期间流量不断;
//! Direct 携带活性观测 (last_alive / next_ping); Relayed 携带重试节拍。

use std::net::SocketAddr;
use std::time::Instant;

/// 探测帧魔数: 与 WG (首字节 1..=4 保留位为零) / STUN (0x01 01 + cookie) 无交集。
pub const PROBE_MAGIC: [u8; 4] = *b"KXT1";
const TOKEN_LEN: usize = 16;
/// 帧布局: [4B 魔数][16B 会话令牌][1B 帧类][1B 名长][名字]。
const HEADER_LEN: usize = 4 + TOKEN_LEN + 1 + 1;
/// 帧类编码值。
const KIND_PING: u8 = 1;
const KIND_PONG: u8 = 2;

pub fn is_probe(b: &[u8]) -> bool {
    b.len() >= HEADER_LEN && b[..4] == PROBE_MAGIC
}

/// 探测帧类: Ping 主动探测, Pong 回执 (回显令牌, 不再触发应答)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    Ping,
    Pong,
}

/// 会话令牌: 区分不同打洞轮次, 过滤偶发回声。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PunchToken(pub [u8; TOKEN_LEN]);

impl PunchToken {
    pub fn random() -> Self {
        use rand::RngCore;
        let mut token = [0u8; TOKEN_LEN];
        rand::rng().fill_bytes(&mut token);
        PunchToken(token)
    }
}

pub fn encode_probe(kind: ProbeKind, name: &str, token: &PunchToken) -> Vec<u8> {
    let mut frame = Vec::with_capacity(HEADER_LEN + name.len());
    frame.extend_from_slice(&PROBE_MAGIC);
    frame.extend_from_slice(&token.0);
    frame.push(match kind {
        ProbeKind::Ping => KIND_PING,
        ProbeKind::Pong => KIND_PONG,
    });
    frame.push(name.len() as u8);
    frame.extend_from_slice(name.as_bytes());
    frame
}

/// 解析探测帧; 名字按节点名约束 (1..=32 ASCII) 过滤垃圾帧。
pub fn decode_probe(b: &[u8]) -> Option<(ProbeKind, String, PunchToken)> {
    if !is_probe(b) {
        return None;
    }
    let kind = match b[4 + TOKEN_LEN] {
        KIND_PING => ProbeKind::Ping,
        KIND_PONG => ProbeKind::Pong,
        _ => return None,
    };
    let name_len = b[HEADER_LEN - 1] as usize;
    let name_bytes = b.get(HEADER_LEN..HEADER_LEN + name_len)?;
    if name_len == 0 || name_len > 32 {
        return None;
    }
    let name = std::str::from_utf8(name_bytes).ok()?;
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let mut token = [0u8; TOKEN_LEN];
    token.copy_from_slice(&b[4..4 + TOKEN_LEN]);
    Some((kind, name.to_string(), PunchToken(token)))
}

/// 单 peer 的路径状态; endpoint 语义: Direct 为真实地址, Relayed 为中继伪端点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PunchState {
    /// 未知路径: 等待候选 / 在线事件。
    Idle,
    /// 打洞进行中: 截止时间 + 本轮会话令牌 + 既有路径 (回落 / 重试期间保持,
    /// 打洞尝试不中断在用流量; None 表示尚无任何路径)。
    /// started 为窗口起点, 供结果日志计 duration_ms。
    Punching {
        deadline: Instant,
        started: Instant,
        token: PunchToken,
        fallback: Option<SocketAddr>,
    },
    /// 直连已建立; last_alive 为最近一次探测 / WG 流量证据时刻,
    /// next_ping 为下一次保活 Ping 时刻 (失活检测的观测来源, FR-6 / FR-15)。
    Direct {
        endpoint: SocketAddr,
        last_alive: Instant,
        next_ping: Instant,
    },
    /// 回落 / 策略强制走中继; next_retry 为下一次直连重试时刻 (自动策略)。
    Relayed {
        endpoint: SocketAddr,
        next_retry: Instant,
    },
}

impl PunchState {
    /// 引擎应使用的对端端点: 打洞重试期间回落到既有路径, 流量不断。
    pub fn endpoint(&self) -> Option<SocketAddr> {
        match self {
            PunchState::Idle => None,
            PunchState::Punching { fallback, .. } => *fallback,
            PunchState::Direct { endpoint, .. } | PunchState::Relayed { endpoint, .. } => {
                Some(*endpoint)
            }
        }
    }

    pub fn is_punching(&self) -> bool {
        matches!(self, PunchState::Punching { .. })
    }

    pub fn is_direct(&self) -> bool {
        matches!(self, PunchState::Direct { .. })
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_roundtrip() {
        let token = PunchToken([7u8; TOKEN_LEN]);
        for kind in [ProbeKind::Ping, ProbeKind::Pong] {
            let frame = encode_probe(kind, "laptop-1", &token);
            assert!(is_probe(&frame));
            let (back_kind, name, back) = decode_probe(&frame).expect("合法帧必须可解析");
            assert_eq!(back_kind, kind);
            assert_eq!(name, "laptop-1");
            assert_eq!(back, token);
        }
    }

    #[test]
    fn probe_rejects_garbage() {
        // 长度不足
        assert!(!is_probe(b"KXT"));
        assert!(decode_probe(b"KXT1").is_none());
        // 名字非法 (空 / 超长 / 非法字符)
        let token = PunchToken([0u8; TOKEN_LEN]);
        assert!(decode_probe(&encode_probe(ProbeKind::Ping, "", &token)).is_none());
        let long = "a".repeat(33);
        assert!(decode_probe(&encode_probe(ProbeKind::Ping, &long, &token)).is_none());
        assert!(decode_probe(&encode_probe(ProbeKind::Ping, "bad name", &token)).is_none());
        // 未知帧类
        let mut bad_kind = encode_probe(ProbeKind::Ping, "ok", &token);
        bad_kind[4 + TOKEN_LEN] = 9;
        assert!(decode_probe(&bad_kind).is_none());
        // WG 帧不会误判为探测
        let mut wg = vec![0u8; 48];
        wg[0] = 0x04;
        assert!(!is_probe(&wg));
    }

    #[test]
    fn state_endpoint_semantics() {
        let addr: SocketAddr = "1.2.3.4:51820".parse().unwrap();
        let now = Instant::now();
        assert_eq!(PunchState::Idle.endpoint(), None);
        assert_eq!(
            PunchState::Punching {
                deadline: now,
                started: now,
                token: PunchToken::random(),
                fallback: None,
            }
            .endpoint(),
            None
        );
        // 重试直连期间保持既有中继路径 (T08: 打洞不中断在用流量)
        assert_eq!(
            PunchState::Punching {
                deadline: now,
                started: now,
                token: PunchToken::random(),
                fallback: Some(addr),
            }
            .endpoint(),
            Some(addr)
        );
        assert_eq!(
            PunchState::Direct {
                endpoint: addr,
                last_alive: now,
                next_ping: now,
            }
            .endpoint(),
            Some(addr)
        );
        assert_eq!(
            PunchState::Relayed {
                endpoint: addr,
                next_retry: now,
            }
            .endpoint(),
            Some(addr)
        );
    }
}
