//! Description: 自签证书与指纹: 客户端通道指纹固定的服务端侧 (NFR-6)。
//!
//! Author: qinzhenya
//! Created: 2026-10-20

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::{Error, Result};

pub struct SelfSignedCert {
    pub cert_pem: String,
    pub key_pem: String,
    pub fingerprint: String,
}

pub fn generate_self_signed() -> std::result::Result<SelfSignedCert, rcgen::Error> {
    let ck = rcgen::generate_simple_self_signed(vec!["kunxtier-relay".into()])?;
    Ok(SelfSignedCert {
        fingerprint: fingerprint(ck.cert.der()),
        cert_pem: ck.cert.pem(),
        key_pem: ck.key_pair.serialize_pem(),
    })
}

/// sha256(DER) 的小写 hex, 作为分发与固定的指纹。
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 从 PEM 文本提取首块证书 DER (仅取自身指纹用, 不做完整 PEM 解析)。
pub fn der_from_pem(pem: &str) -> Result<Vec<u8>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let start = pem
        .find(BEGIN)
        .ok_or_else(|| Error::Cert("证书 PEM 缺少 BEGIN 标记".into()))?
        + BEGIN.len();
    let end = pem
        .find(END)
        .ok_or_else(|| Error::Cert("证书 PEM 缺少 END 标记".into()))?;
    let body: String = pem[start..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    STANDARD
        .decode(body)
        .map_err(|e| Error::Cert(format!("证书 PEM base64 解码失败: {e}")))
}
