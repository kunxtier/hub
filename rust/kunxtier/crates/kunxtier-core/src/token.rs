//! Description: 凭证与令牌: 生成、哈希、前缀校验。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 语义: 明文仅在签发时展示一次, 系统内只流转哈希 (sha256)。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// 凭证种类, 决定前缀; 前缀用于肉眼与程序双重区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Admin,
    NodeToken,
    NodeCredential,
    RelayToken,
}

impl Kind {
    pub fn prefix(self) -> &'static str {
        match self {
            Kind::Admin => "kxt_admin_",
            Kind::NodeToken => "kxt_node_",
            Kind::NodeCredential => "kxt_nid_",
            Kind::RelayToken => "kxt_relay_",
        }
    }
}

/// 生成高熵凭证: 前缀 + 18 字节随机数的 url-safe base64 (144 bit 熵, 满足 NFR-8 的 128 bit 下限)。
pub fn generate(kind: Kind) -> String {
    let mut bytes = [0u8; 18];
    rand::rng().fill_bytes(&mut bytes);
    format!("{}{}", kind.prefix(), URL_SAFE_NO_PAD.encode(bytes))
}

/// 解析凭证种类 (仅校验前缀, 不校验存在性)。
pub fn kind_of(plain: &str) -> Option<Kind> {
    [
        Kind::Admin,
        Kind::NodeToken,
        Kind::NodeCredential,
        Kind::RelayToken,
    ]
    .into_iter()
    .find(|k| plain.starts_with(k.prefix()))
}

/// 明文长度下限校验, 用于拒绝明显不完整的输入。
pub fn looks_valid(plain: &str) -> bool {
    match kind_of(plain) {
        Some(k) => plain.len() >= k.prefix().len() + 22,
        None => false,
    }
}

/// 哈希: 系统内存储与比较的唯一形态。
pub fn hash(plain: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(plain.as_bytes());
    h.finalize().into()
}

/// 常数时间比较, 避免时序侧信道。
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_has_prefix_and_entropy() {
        let t = generate(Kind::NodeToken);
        assert!(t.starts_with("kxt_node_"));
        assert!(t.len() >= "kxt_node_".len() + 22);
        assert_ne!(t, generate(Kind::NodeToken));
    }

    #[test]
    fn kind_of_roundtrip() {
        for k in [
            Kind::Admin,
            Kind::NodeToken,
            Kind::NodeCredential,
            Kind::RelayToken,
        ] {
            assert_eq!(kind_of(&generate(k)), Some(k));
        }
        assert_eq!(kind_of("not_a_token"), None);
    }

    #[test]
    fn hash_is_stable_and_ct_eq_safe() {
        let a = hash("kxt_admin_x");
        let b = hash("kxt_admin_x");
        let c = hash("kxt_admin_y");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(ct_eq(&a, &b));
        assert!(!ct_eq(&a, &c));
        assert!(!ct_eq(&a[1..], &b[..31]));
    }

    #[test]
    fn looks_valid_rejects_truncated() {
        assert!(looks_valid(&generate(Kind::RelayToken)));
        assert!(!looks_valid("kxt_relay_short"));
    }
}
