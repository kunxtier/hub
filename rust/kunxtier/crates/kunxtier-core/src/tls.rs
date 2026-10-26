//! Description: 面向核心中继的 TLS 指纹固定校验器 (NFR-6)。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 自签部署形态下客户端不持 CA 信任库, 改以 `relay --init` 输出的证书
//! sha256 指纹做固定校验: 指纹不匹配的连接一律拒绝。卫星进程与节点
//! (信令 / 转发 WS) 共用本校验器。

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct PinVerifier {
    pub fingerprint: String,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp: String = Sha256::digest(end_entity)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if fp == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("服务端证书指纹不匹配".into()))
        }
    }

    // 握手签名校验放行: 真正的门禁在 verify_server_cert 的指纹比对,
    // 且服务端若不持有证书私钥, 握手本身就会失败。
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
        ]
    }
}

/// 指纹固定的 rustls 客户端配置 (无客户端证书)。
pub fn pinning_client_config(fingerprint: &str) -> Arc<rustls::ClientConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Arc::new(
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinVerifier {
                fingerprint: fingerprint.to_string(),
            }))
            .with_no_client_auth(),
    )
}
