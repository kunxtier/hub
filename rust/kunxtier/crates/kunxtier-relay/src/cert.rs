//! Description: 自签证书与指纹: 客户端通道指纹固定的服务端侧 (NFR-6)。
//!
//! Author: qinzhenya
//! Created: 2026-10-20

use sha2::{Digest, Sha256};

pub struct SelfSignedCert {
    pub cert_pem: String,
    pub key_pem: String,
    pub fingerprint: String,
}

pub fn generate_self_signed() -> Result<SelfSignedCert, rcgen::Error> {
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
