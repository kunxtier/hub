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
pub const ENV_ADVERTISE: &str = "KUNXTIER_ADVERTISE";
pub const ENV_STUN: &str = "KUNXTIER_STUN";
pub const ENV_CORE: &str = "KUNXTIER_CORE";
pub const ENV_RELAY_TOKEN: &str = "KUNXTIER_RELAY_TOKEN";
pub const ENV_FINGERPRINT: &str = "KUNXTIER_FINGERPRINT";

/// 缺省公共 STUN 列表 (host:port): 打洞探测的候选地址来源, 经信令 State 下发给节点。
pub const DEFAULT_STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

/// 文件层 (relay.toml)。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct FileConfig {
    pub listen: Option<String>,
    pub data_dir: Option<String>,
    /// 对外公布的本机地址 (中继列表内置条目用); 卫星模式即注册声明地址。
    pub advertise: Option<String>,
    /// 公共 STUN 服务器列表 (host:port), 整组覆盖缺省。
    pub stun_servers: Option<Vec<String>>,
    /// 日志级别 (error/warn/info/debug/trace); 进程级关注点, 不进 RelayConfig 运行时类型。
    pub log_level: Option<String>,
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
    pub advertise: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelayConfig {
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    /// 对节点公布的本机地址; 缺省在监听地址具体 (非 0.0.0.0) 时取监听地址。
    pub advertise: Option<SocketAddr>,
    /// 公共 STUN 服务器列表 (host:port), 经信令下发给节点做候选地址探测。
    pub stun_servers: Vec<String>,
}

impl RelayConfig {
    /// 按优先级合并: CLI > ENV > CONFIG > DEFAULT。
    pub fn resolve(
        cli: CliOverrides,
        env: impl Fn(&str) -> Option<String>,
        file: FileConfig,
    ) -> Result<Self> {
        let pick = |cli_v: Option<String>,
                    env_key: &str,
                    file_v: Option<String>,
                    default: &str|
         -> String {
            cli_v
                .or_else(|| env(env_key).filter(|s| !s.is_empty()))
                .or(file_v)
                .unwrap_or_else(|| default.to_string())
        };
        let listen: SocketAddr = pick(cli.listen, ENV_LISTEN, file.listen, DEFAULT_LISTEN)
            .parse()
            .map_err(|_| Error::Config("listen 不是合法的 SocketAddr".into()))?;
        let advertise = match pick(cli.advertise, ENV_ADVERTISE, file.advertise, "") {
            s if s.is_empty() => None,
            s => Some(
                s.parse()
                    .map_err(|_| Error::Config("advertise 不是合法的 SocketAddr".into()))?,
            ),
        };
        let stun_servers = match env(ENV_STUN).filter(|s| !s.is_empty()) {
            // env 为整组覆盖 (逗号分隔); 条目格式留待节点侧解析时校验
            Some(s) => s
                .split(',')
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .map(str::to_string)
                .collect(),
            None => file
                .stun_servers
                .unwrap_or_else(|| DEFAULT_STUN_SERVERS.iter().map(|s| s.to_string()).collect()),
        };
        Ok(RelayConfig {
            listen,
            advertise,
            stun_servers,
            data_dir: PathBuf::from(pick(
                cli.data_dir,
                ENV_DATA_DIR,
                file.data_dir,
                DEFAULT_DATA_DIR,
            )),
        })
    }

    /// 内置转发条目的公布地址: advertise 优先, 监听地址具体时取监听地址,
    /// 通配监听且未配置 advertise 时为 None (由节点以自身核心地址代入)。
    pub fn builtin_addr(&self) -> Option<SocketAddr> {
        if self.advertise.is_some() {
            return self.advertise;
        }
        (!self.listen.ip().is_unspecified()).then_some(self.listen)
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

/// 卫星模式 CLI 覆盖层。
#[derive(Debug, Default)]
pub struct SatelliteOverrides {
    pub core: Option<String>,
    pub listen: Option<String>,
    pub advertise: Option<String>,
    pub fingerprint: Option<String>,
}

/// 卫星模式配置: 纯转发, 无状态目录; relay 令牌与指纹只走 env (NFR: 敏感字段不进文件)。
#[derive(Debug, Clone, PartialEq)]
pub struct SatelliteConfig {
    /// 核心中继拨号地址 host:port (不含 scheme)。
    pub core: String,
    pub listen: SocketAddr,
    /// 对节点公布的拨号地址 (注册时自报, 无缺省值)。
    pub advertise: SocketAddr,
    pub fingerprint: String,
    /// env KUNXTIER_RELAY_TOKEN; 不接受 CLI / 文件来源。
    pub token: String,
}

impl SatelliteConfig {
    /// 分层同 RelayConfig; listen / advertise 复用同一组键, advertise 缺省取具体监听地址。
    pub fn resolve(
        cli: SatelliteOverrides,
        env: impl Fn(&str) -> Option<String>,
        file: FileConfig,
    ) -> Result<Self> {
        let pick = |cli_v: Option<String>,
                    env_key: &str,
                    file_v: Option<String>,
                    default: &str|
         -> Option<String> {
            Some(
                cli_v
                    .or_else(|| env(env_key).filter(|s| !s.is_empty()))
                    .or(file_v)
                    .unwrap_or_else(|| default.to_string()),
            )
            .filter(|s| !s.is_empty())
        };
        let core = pick(cli.core, ENV_CORE, None, "")
            .ok_or_else(|| {
                Error::Config("卫星模式必须指定核心地址 (--core 或 KUNXTIER_CORE)".into())
            })?
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string();
        let listen: SocketAddr = pick(cli.listen, ENV_LISTEN, file.listen, DEFAULT_LISTEN)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::Config("listen 不是合法的 SocketAddr".into()))?;
        let advertise = pick(cli.advertise, ENV_ADVERTISE, file.advertise, "")
            .and_then(|s| s.parse().ok())
            .or((!listen.ip().is_unspecified()).then_some(listen))
            .ok_or_else(|| {
                Error::Config("卫星模式必须指定公布地址 (--advertise 或 KUNXTIER_ADVERTISE)".into())
            })?;
        let fingerprint = pick(cli.fingerprint, ENV_FINGERPRINT, None, "").ok_or_else(|| {
            Error::Config(
                "卫星模式必须指定核心证书指纹 (--fingerprint 或 KUNXTIER_FINGERPRINT)".into(),
            )
        })?;
        let token = env(ENV_RELAY_TOKEN)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::Config("卫星模式必须设置 KUNXTIER_RELAY_TOKEN".into()))?;
        Ok(SatelliteConfig {
            core,
            listen,
            advertise,
            fingerprint,
            token,
        })
    }
}

// ------------------------------ 节点全局配置 ------------------------------

/// 节点 state 目录的 env 键 (与 CLI --state-dir 同义)。
pub const ENV_NODE_STATE_DIR: &str = "KUNXTIER_STATE_DIR";

/// 节点机器级全局配置文件路径 (T14 服务化; Windows 路径随平台切片定稿)。
pub fn node_config_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/kunxtier/config.toml")
    }
    // 其余 unix (含 Linux) 走发行版惯例 /etc
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from("/etc/kunxtier/config.toml")
    }
    #[cfg(not(unix))]
    {
        PathBuf::from("./kunxtier-config.toml")
    }
}

/// state 目录平台默认 (macOS 与配置同置, ZeroTier 先例)。
pub fn node_state_dir_default() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/kunxtier")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from("/var/lib/kunxtier")
    }
    #[cfg(not(unix))]
    {
        PathBuf::from("./kunxtier-node-state")
    }
}

/// 日志目录平台默认 (各平台日志根下建应用子目录)。
pub fn node_log_dir_default() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Logs/kunxtier")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from("/var/log/kunxtier")
    }
    #[cfg(not(unix))]
    {
        PathBuf::from("./kunxtier-logs")
    }
}

/// 节点全局配置文件层 (机器级, 全部非敏感; 敏感字段只走 env)。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct NodeFileConfig {
    pub state_dir: Option<String>,
    pub log_dir: Option<String>,
    /// 日志级别 (error/warn/info/debug/trace); 级别链 CLI flag > RUST_LOG > 本层 > 默认 info。
    pub log_level: Option<String>,
}

impl NodeFileConfig {
    /// install 用: 文件缺失时以平台默认值显式生成 (返回内容待落盘, 可读可改), 存在则原样读取。
    pub fn load_or_default(path: &Path) -> Result<(Self, Option<String>)> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let file = toml::from_str::<Self>(&text)
                    .map_err(|e| Error::Config(format!("配置文件解析失败: {e}")))?;
                Ok((file, None))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let content = Self::render_default();
                // expect 理由: render_default 输出自控常量模板, 解析失败属实现回归, 单测已覆盖
                let file = toml::from_str(&content).expect("默认全局配置模板解析失败");
                Ok((file, Some(content)))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// 默认配置模板: 目录显式落盘 (运维可见可改), log_level 以注释示例呈现。
    fn render_default() -> String {
        format!(
            "# kunxtier 节点全局配置 (机器级; 敏感字段不进本文件, 只走环境变量)\n\
             # state / 日志目录与日志级别; 优先级 CLI > ENV > 本文件 > 默认\n\
             state_dir = \"{}\"\n\
             log_dir = \"{}\"\n\
             # log_level = \"info\"\n",
            node_state_dir_default().display(),
            node_log_dir_default().display(),
        )
    }
}

/// 解析后的节点全局配置: 目录定位与日志级别 (daemon / up / down / status / install 共用)。
#[derive(Debug, Clone, PartialEq)]
pub struct NodeGlobalConfig {
    pub state_dir: PathBuf,
    pub log_dir: PathBuf,
    pub log_level: Option<String>,
}

impl NodeGlobalConfig {
    /// 读机器级配置文件并解析; 文件缺失按全默认处理 (不视为错误)。
    pub fn load(
        cli_state_dir: Option<PathBuf>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let path = node_config_path();
        let file = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str::<NodeFileConfig>(&text).map_err(|e| {
                Error::Config(format!("全局配置解析失败 ({}): {e}", path.display()))
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => NodeFileConfig::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self::resolve(cli_state_dir, env, file))
    }

    /// state_dir 分层 CLI > ENV > CONFIG > DEFAULT; log_dir / log_level 为 CONFIG > DEFAULT。
    pub fn resolve(
        cli_state_dir: Option<PathBuf>,
        env: impl Fn(&str) -> Option<String>,
        file: NodeFileConfig,
    ) -> Self {
        let state_dir = cli_state_dir
            .or_else(|| {
                env(ENV_NODE_STATE_DIR)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
            })
            .or_else(|| file.state_dir.map(PathBuf::from))
            .unwrap_or_else(node_state_dir_default);
        let log_dir = file
            .log_dir
            .map(PathBuf::from)
            .unwrap_or_else(node_log_dir_default);
        Self {
            state_dir,
            log_dir,
            log_level: file.log_level,
        }
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
        let cfg =
            RelayConfig::resolve(CliOverrides::default(), no_env, FileConfig::default()).unwrap();
        assert_eq!(cfg.listen.to_string(), "0.0.0.0:443");
        assert_eq!(cfg.data_dir, PathBuf::from("./kunxtier-state"));
        assert_eq!(
            cfg.stun_servers,
            DEFAULT_STUN_SERVERS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn stun_list_env_overrides_file_overrides_default() {
        // env 整组覆盖
        let env = |k: &str| (k == ENV_STUN).then(|| "1.1.1.1:3478, 2.2.2.2:3478".to_string());
        let cfg =
            RelayConfig::resolve(CliOverrides::default(), env, FileConfig::default()).unwrap();
        assert_eq!(cfg.stun_servers, vec!["1.1.1.1:3478", "2.2.2.2:3478"]);

        // 文件层覆盖缺省
        let file = FileConfig {
            stun_servers: Some(vec!["3.3.3.3:3478".into()]),
            ..FileConfig::default()
        };
        let cfg = RelayConfig::resolve(CliOverrides::default(), no_env, file).unwrap();
        assert_eq!(cfg.stun_servers, vec!["3.3.3.3:3478"]);
    }

    #[test]
    fn priority_cli_over_env_over_file() {
        let env = |k: &str| match k {
            ENV_LISTEN => Some("1.1.1.1:1111".to_string()),
            ENV_DATA_DIR => Some("./from-env".to_string()),
            _ => None,
        };
        let file = FileConfig {
            listen: Some("2.2.2.2:2222".into()),
            data_dir: Some("./from-file".into()),
            advertise: None,
            stun_servers: None,
            log_level: None,
        };

        // 三层齐设: CLI 胜出
        let cli = CliOverrides {
            listen: Some("3.3.3.3:3333".into()),
            data_dir: Some("./from-cli".into()),
            advertise: None,
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
            advertise: None,
            stun_servers: None,
            log_level: None,
        };
        let cfg = RelayConfig::resolve(CliOverrides::default(), env, file).unwrap();
        assert_eq!(cfg.listen.to_string(), "2.2.2.2:2222");
    }

    #[test]
    fn log_level_parses_from_file_and_defaults_none() {
        let cfg = toml::from_str::<FileConfig>("log_level = \"debug\"").unwrap();
        assert_eq!(cfg.log_level.as_deref(), Some("debug"));
        assert_eq!(FileConfig::default().log_level, None);
    }

    #[test]
    fn invalid_listen_is_rejected() {
        let cli = CliOverrides {
            listen: Some("not-a-socket".into()),
            data_dir: None,
            advertise: None,
        };
        assert!(RelayConfig::resolve(cli, no_env, FileConfig::default()).is_err());
    }

    #[test]
    fn node_config_defaults_follow_platform() {
        let cfg = NodeGlobalConfig::resolve(None, no_env, NodeFileConfig::default());
        assert_eq!(cfg.state_dir, node_state_dir_default());
        assert_eq!(cfg.log_dir, node_log_dir_default());
        assert_eq!(cfg.log_level, None);
        // 平台默认显式核对 (开发机分支实际执行)
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                cfg.state_dir,
                PathBuf::from("/Library/Application Support/kunxtier")
            );
            assert_eq!(cfg.log_dir, PathBuf::from("/Library/Logs/kunxtier"));
        }
        #[cfg(target_os = "linux")]
        {
            assert_eq!(cfg.state_dir, PathBuf::from("/var/lib/kunxtier"));
            assert_eq!(cfg.log_dir, PathBuf::from("/var/log/kunxtier"));
        }
    }

    #[test]
    fn node_config_state_dir_priority_cli_env_file_default() {
        let file = NodeFileConfig {
            state_dir: Some("./from-file".into()),
            log_dir: Some("/logs".into()),
            log_level: Some("debug".into()),
        };
        // CLI 胜出
        let cfg = NodeGlobalConfig::resolve(Some("./from-cli".into()), no_env, file.clone());
        assert_eq!(cfg.state_dir, PathBuf::from("./from-cli"));
        // ENV 胜 FILE
        let env = |k: &str| (k == ENV_NODE_STATE_DIR).then(|| "./from-env".to_string());
        let cfg = NodeGlobalConfig::resolve(None, env, file.clone());
        assert_eq!(cfg.state_dir, PathBuf::from("./from-env"));
        // FILE 胜默认; log 层随文件
        let cfg = NodeGlobalConfig::resolve(None, no_env, file);
        assert_eq!(cfg.state_dir, PathBuf::from("./from-file"));
        assert_eq!(cfg.log_dir, PathBuf::from("/logs"));
        assert_eq!(cfg.log_level.as_deref(), Some("debug"));
        // 空 env 视同未设置
        let empty_env = |k: &str| (k == ENV_NODE_STATE_DIR).then(String::new);
        let cfg = NodeGlobalConfig::resolve(None, empty_env, NodeFileConfig::default());
        assert_eq!(cfg.state_dir, node_state_dir_default());
    }

    #[test]
    fn node_file_config_default_template_roundtrips() {
        let rendered = NodeFileConfig::render_default();
        assert!(rendered.contains("state_dir"));
        assert!(rendered.contains("# log_level"));
        // 模板可被解析回配置层 (install 落盘后即是一份合法配置)
        let parsed: NodeFileConfig = toml::from_str(&rendered).unwrap();
        assert_eq!(
            parsed.state_dir.as_deref(),
            Some(node_state_dir_default().to_str().unwrap())
        );
        assert_eq!(
            parsed.log_dir.as_deref(),
            Some(node_log_dir_default().to_str().unwrap())
        );
        assert_eq!(parsed.log_level, None);
        // 非法值形态被拒绝
        assert!(toml::from_str::<NodeFileConfig>("state_dir = [1]").is_err());
        assert!(toml::from_str::<NodeFileConfig>("state_dir = 1").is_err());
    }

    #[test]
    fn builtin_addr_prefers_advertise_then_concrete_listen() {
        let mk = |listen: &str, advertise: Option<&str>| RelayConfig {
            listen: listen.parse().unwrap(),
            advertise: advertise.map(|a| a.parse().unwrap()),
            data_dir: PathBuf::from("./kunxtier-state"),
            stun_servers: Vec::new(),
        };
        // advertise 显式指定: 直接采用
        assert_eq!(
            mk("0.0.0.0:443", Some("1.2.3.4:9443"))
                .builtin_addr()
                .unwrap(),
            "1.2.3.4:9443".parse().unwrap()
        );
        // 具体监听地址可对外时取监听地址
        assert_eq!(
            mk("127.0.0.1:9443", None).builtin_addr().unwrap(),
            "127.0.0.1:9443".parse().unwrap()
        );
        // 通配监听且未配置: 无公布地址 (节点以自身核心地址代入)
        assert_eq!(mk("0.0.0.0:443", None).builtin_addr(), None);
    }

    #[test]
    fn satellite_resolves_layers_and_requires_sensitive_env() {
        let env = |k: &str| match k {
            ENV_RELAY_TOKEN => Some("kxt_relay_secret".into()),
            _ => None,
        };
        // advertise 缺省取具体监听地址; core 容忍 https:// 前缀
        let cli = SatelliteOverrides {
            core: Some("https://5.6.7.8:9443".into()),
            listen: Some("127.0.0.1:8443".into()),
            advertise: None,
            fingerprint: Some("ab".repeat(32)),
        };
        let cfg = SatelliteConfig::resolve(cli, env, FileConfig::default()).unwrap();
        assert_eq!(cfg.core, "5.6.7.8:9443");
        assert_eq!(cfg.listen.to_string(), "127.0.0.1:8443");
        assert_eq!(cfg.advertise.to_string(), "127.0.0.1:8443");

        // 令牌只认 env: 缺失即拒绝, 不回落其他层
        let no_token = |_: &str| None::<String>;
        assert!(SatelliteConfig::resolve(
            SatelliteOverrides {
                core: Some("5.6.7.8:9443".into()),
                listen: Some("127.0.0.1:8443".into()),
                advertise: Some("1.2.3.4:443".into()),
                fingerprint: Some("ab".repeat(32)),
            },
            no_token,
            FileConfig::default(),
        )
        .is_err());

        // 通配监听且无 advertise: 拒绝启动
        let env2 = |k: &str| (k == ENV_RELAY_TOKEN).then(|| "kxt_relay_secret".to_string());
        assert!(SatelliteConfig::resolve(
            SatelliteOverrides {
                core: Some("5.6.7.8:9443".into()),
                listen: None,
                advertise: None,
                fingerprint: Some("ab".repeat(32)),
            },
            env2,
            FileConfig::default(),
        )
        .is_err());
    }
}
