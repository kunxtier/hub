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
    /// 控制台 admin 会话 (内存态, 不入库)。
    Session,
}

impl Kind {
    pub fn prefix(self) -> &'static str {
        match self {
            Kind::Admin => "kxt_admin_",
            Kind::NodeToken => "kxt_node_",
            Kind::NodeCredential => "kxt_nid_",
            Kind::RelayToken => "kxt_relay_",
            Kind::Session => "kxt_ses_",
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

// ---- 信令挑战应答 (协议形态: nonce + sha256 摘要, 2026-10-21 裁决) ----

/// 服务端挑战 nonce: 32 字节 CSPRNG。
pub fn generate_nonce() -> [u8; 32] {
    let mut b = [0u8; 32];
    rand::rng().fill_bytes(&mut b);
    b
}

/// 摘要 = sha256(nonce || credential_hash)。
/// credential_hash 为 sha256(凭证明文): 客户端本地可算, 服务端为库中唯一存储形态,
/// 双方均不传输明文或哈希本体 (与 HTTP Digest 同构, 见 nfr-kunxtier-security)。
pub fn challenge_digest(nonce: &[u8], credential_hash: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(nonce);
    h.update(credential_hash);
    h.finalize().into()
}

/// 客户端侧封装: 以凭证明文计算挑战应答。
pub fn challenge_answer(nonce: &[u8], credential_plain: &str) -> [u8; 32] {
    challenge_digest(nonce, &hash(credential_plain))
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

    #[test]
    fn challenge_answer_matches_digest_of_hash() {
        let nonce = generate_nonce();
        let credential = generate(Kind::NodeCredential);
        // 客户端应答 = 服务端以库存哈希计算的期望值
        let answer = challenge_answer(&nonce, &credential);
        let expect = challenge_digest(&nonce, &hash(&credential));
        assert!(ct_eq(&answer, &expect));
    }

    #[test]
    fn challenge_digest_varies_with_nonce() {
        let h = hash("kxt_nid_fixed");
        let a = challenge_digest(&generate_nonce(), &h);
        let b = challenge_digest(&generate_nonce(), &h);
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
    }
}
