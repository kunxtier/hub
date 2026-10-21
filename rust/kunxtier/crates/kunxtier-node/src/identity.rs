//! Description: 节点数据面身份: WireGuard 密钥对。私钥为节点双凭证之一, 不离机、不进配置。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 公钥以 base64 (std) 编码的 32 字节参与注册与信令节点表, 与核心中继
//! RegisterRequest.wg_public_key / SignalPeer.wg_public_key 的编码约定一致。

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use gotatun::x25519::{PublicKey, StaticSecret};
use rand::RngCore;

use crate::error::{Error, Result};

/// 节点 WireGuard 身份: 私钥持有方。
#[derive(Clone)]
pub struct WgIdentity {
    secret: StaticSecret,
}

impl WgIdentity {
    /// 随机生成新身份: 32 字节随机量, 由 x25519 库按协议完成钳位。
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        Self::from_private_key_bytes(bytes)
    }

    /// 从持久化的私钥字节恢复身份 (state 目录加载路径)。
    pub fn from_private_key_bytes(bytes: [u8; 32]) -> Self {
        Self {
            secret: StaticSecret::from(bytes),
        }
    }

    pub fn secret(&self) -> &StaticSecret {
        &self.secret
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey::from(&self.secret)
    }

    /// 注册时上报核心中继的公钥编码。
    pub fn public_key_base64(&self) -> String {
        BASE64_STANDARD.encode(self.public_key().as_bytes())
    }

    /// 解析信令节点表中的对端公钥 (base64 std 的 32 字节)。
    pub fn parse_public_key_base64(text: &str) -> Result<PublicKey> {
        let bytes: [u8; 32] = BASE64_STANDARD
            .decode(text)
            .map_err(|e| Error::Key(format!("公钥 base64 解码失败: {e}")))?
            .try_into()
            .map_err(|v: Vec<u8>| {
                Error::Key(format!("公钥长度非法: 期望 32 字节, 实际 {}", v.len()))
            })?;
        Ok(PublicKey::from(bytes))
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_public_key_roundtrips_through_base64() {
        let identity = WgIdentity::generate();
        let encoded = identity.public_key_base64();
        let parsed = WgIdentity::parse_public_key_base64(&encoded).unwrap();
        assert_eq!(parsed.as_bytes(), identity.public_key().as_bytes());
    }

    #[test]
    fn private_key_restore_is_deterministic() {
        let bytes = [7u8; 32];
        let a = WgIdentity::from_private_key_bytes(bytes);
        let b = WgIdentity::from_private_key_bytes(bytes);
        assert_eq!(a.public_key_base64(), b.public_key_base64());
    }

    #[test]
    fn parse_rejects_malformed_public_key() {
        assert!(WgIdentity::parse_public_key_base64("not-base64!!").is_err());
        // 长度不足 32 字节
        assert!(WgIdentity::parse_public_key_base64(&BASE64_STANDARD.encode([0u8; 16])).is_err());
    }
}
