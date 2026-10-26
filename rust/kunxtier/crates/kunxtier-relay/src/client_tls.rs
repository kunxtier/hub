//! Description: 面向核心中继的 TLS 客户端装配 (自签 + 指纹固定, NFR-6)。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 指纹校验器移驻 kunxtier-core::tls (节点信令 / 转发客户端共用);
//! 本模块保留 admin REST 所需的 ureq agent 装配。指纹来自 `relay --init`
//! 输出, 指纹不匹配的连接一律拒绝。

pub use kunxtier_core::tls::PinVerifier;

/// 构建指纹固定的 ureq agent。
pub fn pinning_agent(fingerprint: &str) -> ureq::Agent {
    let tls = kunxtier_core::tls::pinning_client_config(fingerprint);
    ureq::AgentBuilder::new().tls_config(tls).build()
}
