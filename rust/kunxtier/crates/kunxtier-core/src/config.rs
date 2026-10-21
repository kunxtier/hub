//! Description: 分层配置: CLI > ENV > CONFIG > DEFAULT (优先级固定, FR-7 / basic-config-system)。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 敏感字段 (admin 凭证 / relay 令牌 / 证书指纹) 只走 env, 不进配置文件。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

pub const DEFAULT_LISTEN: &str = "0.0.0.0:443";
pub const DEFAULT_DATA_DIR: &str = "./kunxtier-state";
pub const ENV_LISTEN: &str = "KUNXTIER_LISTEN";
pub const ENV_DATA_DIR: &str = "KUNXTIER_DATA_DIR";

/// 文件层 (relay.toml)。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct FileConfig {
    pub listen: Option<String>,
    pub data_dir: Option<String>,
}

impl FileConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|e| Error::Config(format!("配置文件解析失败: {e}")))
    }
}

/// CLI 覆盖层。
#[derive(Debug, Default)]
pub struct CliOverrides {
    pub listen: Option<String>,
    pub data_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelayConfig {
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
}

impl RelayConfig {
    /// 按优先级合并: CLI > ENV > CONFIG > DEFAULT。
    pub fn resolve(
        cli: CliOverrides,
        env: impl Fn(&str) -> Option<String>,
        file: FileConfig,
    ) -> Result<Self> {
        let pick = |cli_v: Option<String>, env_key: &str, file_v: Option<String>, default: &str| -> String {
            cli_v
                .or_else(|| env(env_key).filter(|s| !s.is_empty()))
                .or(file_v)
                .unwrap_or_else(|| default.to_string())
        };
        let listen: SocketAddr = pick(cli.listen, ENV_LISTEN, file.listen, DEFAULT_LISTEN)
            .parse()
            .map_err(|_| Error::Config("listen 不是合法的 SocketAddr".into()))?;
        Ok(RelayConfig {
            listen,
            data_dir: PathBuf::from(pick(
                cli.data_dir,
                ENV_DATA_DIR,
                file.data_dir,
                DEFAULT_DATA_DIR,
            )),
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("state.db")
    }

    pub fn cert_path(&self) -> PathBuf {
        self.data_dir.join("cert.pem")
    }

    pub fn key_path(&self) -> PathBuf {
        self.data_dir.join("key.pem")
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_when_all_layers_empty() {
        let cfg = RelayConfig::resolve(CliOverrides::default(), no_env, FileConfig::default()).unwrap();
        assert_eq!(cfg.listen.to_string(), "0.0.0.0:443");
        assert_eq!(cfg.data_dir, PathBuf::from("./kunxtier-state"));
    }

    #[test]
    fn priority_cli_over_env_over_file() {
        let env = |k: &str| {
            match k {
                ENV_LISTEN => Some("1.1.1.1:1111".to_string()),
                ENV_DATA_DIR => Some("./from-env".to_string()),
                _ => None,
            }
        };
        let file = FileConfig {
            listen: Some("2.2.2.2:2222".into()),
            data_dir: Some("./from-file".into()),
        };

        // 三层齐设: CLI 胜出
        let cli = CliOverrides {
            listen: Some("3.3.3.3:3333".into()),
            data_dir: Some("./from-cli".into()),
        };
        let cfg = RelayConfig::resolve(cli, env, file.clone()).unwrap();
        assert_eq!(cfg.listen.to_string(), "3.3.3.3:3333");
        assert_eq!(cfg.data_dir, PathBuf::from("./from-cli"));

        // 无 CLI: ENV 胜过 FILE
        let cfg = RelayConfig::resolve(CliOverrides::default(), env, file.clone()).unwrap();
        assert_eq!(cfg.listen.to_string(), "1.1.1.1:1111");
        assert_eq!(cfg.data_dir, PathBuf::from("./from-env"));

        // 无 CLI / ENV: FILE 胜过 DEFAULT
        let cfg = RelayConfig::resolve(CliOverrides::default(), no_env, file).unwrap();
        assert_eq!(cfg.listen.to_string(), "2.2.2.2:2222");
        assert_eq!(cfg.data_dir, PathBuf::from("./from-file"));
    }

    #[test]
    fn empty_env_value_falls_through_to_file() {
        let env = |k: &str| (k == ENV_LISTEN).then(String::new);
        let file = FileConfig {
            listen: Some("2.2.2.2:2222".into()),
            data_dir: None,
        };
        let cfg = RelayConfig::resolve(CliOverrides::default(), env, file).unwrap();
        assert_eq!(cfg.listen.to_string(), "2.2.2.2:2222");
    }

    #[test]
    fn invalid_listen_is_rejected() {
        let cli = CliOverrides {
            listen: Some("not-a-socket".into()),
            data_dir: None,
        };
        assert!(RelayConfig::resolve(cli, no_env, FileConfig::default()).is_err());
    }
}
