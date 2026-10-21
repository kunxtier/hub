//! Description: relay --init: 状态目录初始化 (自签证书 / admin 凭证 / 默认网络)。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

use kunxtier_core::config::RelayConfig;

use crate::cert;
use crate::store::Store;
use crate::{Error, Result};

/// `relay --init` 的产出; 明文凭证只在这里展示一次。
pub struct InitOutput {
    pub admin_credential: String,
    pub fingerprint: String,
    pub data_dir: String,
}

/// 初始化状态目录; 已初始化 (admin 哈希已存在) 则拒绝重复执行。
///
/// 步骤顺序有意为之: 先判定再写证书 (拒绝重复 init 时不能覆盖既有证书,
/// 指纹已被客户端固定), db 侧收尾为单事务 — 中途任一步失败后重跑 init
/// 会覆盖残留证书并自动补齐落库, 无需手工清理状态目录。
pub fn init(cfg: &RelayConfig) -> Result<InitOutput> {
    // 显式选定 ring provider: 依赖图中可能同时存在多个 provider feature,
    // 尽早安装, 避免后续 TLS 构造点 (服务端 / 客户端) panic。
    let _ = rustls::crypto::ring::default_provider().install_default();
    std::fs::create_dir_all(&cfg.data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::metadata(&cfg.data_dir)?.permissions();
        let mut perms = perms.clone();
        perms.set_mode(0o700);
        std::fs::set_permissions(&cfg.data_dir, perms)?;
    }
    let store = Store::open(&cfg.db_path())?;
    if store.has_admin()? {
        return Err(Error::Usage(format!(
            "状态目录已初始化 ({}), 拒绝重复 init",
            cfg.data_dir.display()
        )));
    }

    let ck = cert::generate_self_signed().map_err(|e| Error::Cert(e.to_string()))?;
    std::fs::write(cfg.cert_path(), &ck.cert_pem)?;
    std::fs::write(cfg.key_path(), &ck.key_pem)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(cfg.key_path(), std::fs::Permissions::from_mode(0o600))?;
        std::fs::set_permissions(cfg.cert_path(), std::fs::Permissions::from_mode(0o644))?;
    }

    let base = kunxtier_core::address::nth_network_base(0)
        .map_err(|e| Error::Usage(e.to_string()))?;
    let admin = store.bootstrap_default(base)?;

    tracing::info!(data_dir = %cfg.data_dir.display(), "核心中继初始化完成");
    Ok(InitOutput {
        data_dir: cfg.data_dir.display().to_string(),
        admin_credential: admin,
        fingerprint: ck.fingerprint,
    })
}
