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

/// admin 凭证落盘文件名 (state 目录内, 0600)。
/// 服务化 / 输出被 journal 吞掉时可找回; state 目录本身即信任边界 (TLS 私钥同域)。
pub const ADMIN_CREDENTIAL_FILE: &str = "admin-credential";

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

    let base =
        kunxtier_core::address::nth_network_base(0).map_err(|e| Error::Usage(e.to_string()))?;
    let admin = store.bootstrap_default(base)?;

    // 凭证同时写入 state 目录 (0600): 启动输出被 journal 吞掉或错过后可找回
    let cred_path = cfg.data_dir.join(ADMIN_CREDENTIAL_FILE);
    std::fs::write(&cred_path, format!("{admin}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cred_path, std::fs::Permissions::from_mode(0o600))?;
    }

    tracing::info!(data_dir = %cfg.data_dir.display(), "核心中继初始化完成");
    Ok(InitOutput {
        data_dir: cfg.data_dir.display().to_string(),
        admin_credential: admin,
        fingerprint: ck.fingerprint,
    })
}

/// 运行模式启动前置: 未初始化 (db 不存在) 则就地进行 init, 返回 Some(首启产出);
/// 已初始化返回 None, 不做任何变更。免记忆 `relay --init` 前置命令。
pub fn ensure_initialized(cfg: &RelayConfig) -> Result<Option<InitOutput>> {
    if cfg.db_path().exists() {
        return Ok(None);
    }
    init(cfg).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kunxtier_core::config::RelayConfig;

    fn cfg_in(dir: &std::path::Path) -> RelayConfig {
        RelayConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            data_dir: dir.to_path_buf(),
            advertise: None,
            stun_servers: Vec::new(),
        }
    }

    #[test]
    fn 运行模式自动初始化_首启产出与凭证文件() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let first = ensure_initialized(&cfg)
            .unwrap()
            .expect("首启应返回初始化产出");
        assert!(cfg.db_path().exists());
        let cred_path = dir.path().join(ADMIN_CREDENTIAL_FILE);
        let content = std::fs::read_to_string(&cred_path).unwrap();
        assert_eq!(content.trim(), first.admin_credential);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&cred_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // 已初始化后不重复执行
        assert!(ensure_initialized(&cfg).unwrap().is_none());
    }
}
