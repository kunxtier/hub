//! Description: 分层配置: 持久配置 ENV > CONFIG > DEFAULT, 一次性动作参数不进分层 (FR-7)。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 敏感凭证 (admin 凭证 / relay 令牌) 只走 env, 不进配置文件;
//! 证书指纹与核心地址非机密 (客户端侧公开固定), 2026-11-06 起允许进 relay.toml。

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

/// 卫星模式判定: relay 令牌非空即卫星, 缺失即核心 (2026-11-06 决策, 无独立开关)。
/// 调用方不做冲突校验: KUNXTIER_CORE / KUNXTIER_FINGERPRINT 与 up / admin /
/// console 子命令共用, 属环境固有变量, 核心模式静默忽略; 卫星模式同理忽略
/// data-dir 等核心专属键 (各模式只读各自字段)。
pub fn satellite_mode(env: impl Fn(&str) -> Option<String>) -> bool {
    matches!(env(ENV_RELAY_TOKEN), Some(v) if !v.is_empty())
}

/// relay 配置文件固定路径: 机器级自动发现, 无 CLI 指定入口 (2026-11-06 决策);
/// 与节点全局 config.toml 同目录但各管一个角色, 不合并。
pub fn relay_config_path() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/etc/kunxtier/relay.toml")
    }
    #[cfg(not(unix))]
    {
        PathBuf::from("./kunxtier-relay.toml")
    }
}

/// 文件层 (relay.toml, 全非机密; relay 令牌仍只走 env)。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct FileConfig {
    pub listen: Option<String>,
    pub data_dir: Option<String>,
    /// 对外公布的本机地址 (中继列表内置条目用); 卫星模式即注册声明地址。
    pub advertise: Option<String>,
    /// 卫星模式: 核心中继拨号地址; 核心模式不读取。
    pub core: Option<String>,
    /// 卫星模式: 核心证书指纹; 核心模式不读取。
    pub fingerprint: Option<String>,
    /// 公共 STUN 服务器列表 (host:port), 整组覆盖缺省。
    pub stun_servers: Option<Vec<String>>,
    /// 日志级别 (error/warn/info/debug/trace); 进程级关注点, 不进 RelayConfig 运行时类型。
    pub log_level: Option<String>,
}

impl FileConfig {
    /// 固定路径发现 (路径经参数注入以便测试): 文件缺失视同空配置 (全默认可用,
    /// 不以模板值模拟 "存在", 各层保持 truthful), 存在但解析失败即报错。
    pub fn discover_at(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| {
                Error::Config(format!("relay 配置解析失败 ({}): {e}", path.display()))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// 机器级固定路径自动发现 (--config 指定入口已随 2026-11-06 决策拆除)。
    pub fn discover() -> Result<Self> {
        Self::discover_at(&relay_config_path())
    }

    /// 读取; 文件缺失时附默认模板内容 (待落盘), 供 config init 显式生成;
    /// 语义与 NodeFileConfig::load_or_default 一致 (只生成不修改)。
    pub fn load_or_default(path: &Path) -> Result<(Self, Option<String>)> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let file = toml::from_str::<Self>(&text).map_err(|e| {
                    Error::Config(format!("relay 配置解析失败 ({}): {e}", path.display()))
                })?;
                Ok((file, None))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let content = Self::render_default();
                // expect 理由: render_default 输出自控常量模板, 解析失败属实现回归, 单测已覆盖
                let file = toml::from_str(&content).expect("默认 relay 配置模板解析失败");
                Ok((file, Some(content)))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// 默认配置模板: 运维关键项显式落盘 (可见可改), 卫星与可选项以注释示例呈现。
    fn render_default() -> String {
        format!(
            "# kunxtier relay 配置 (机器级; relay 令牌等敏感字段只走环境变量)\n\
             # 优先级 ENV > 本文件 > 默认; KUNXTIER_RELAY_TOKEN 非空即卫星模式\n\
             listen = \"{DEFAULT_LISTEN}\"\n\
             data_dir = \"{DEFAULT_DATA_DIR}\"\n\
             # advertise = \"1.2.3.4:443\"            # 对外公布地址 (核心 / 卫星共用)\n\
             # core = \"core.example.com:443\"        # 卫星模式: 核心拨号地址\n\
             # fingerprint = \"<核心证书指纹 hex>\"    # 卫星模式\n\
             # stun_servers = [\"stun.l.google.com:19302\"]\n\
             # log_level = \"info\"\n"
        )
    }

    /// config init 用: 文件缺失时以默认模板原子落盘 (0644, 可读可改), 已存在则
    /// 不动, 返回是否落盘; 与 NodeFileConfig::ensure_written 同语义。
    pub fn ensure_written(path: &Path) -> Result<bool> {
        let Some(content) = Self::load_or_default(path)?.1 else {
            return Ok(false);
        };
        crate::atomic_file::write_atomic(path, content.as_bytes(), 0o644)?;
        Ok(true)
    }
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
    /// 按优先级合并: ENV > CONFIG > DEFAULT (2026-11-06 反转决策, relay 无配置类
    /// flag; 单次临时覆盖用 env, 不回退 CLI)。
    pub fn resolve(env: impl Fn(&str) -> Option<String>, file: FileConfig) -> Result<Self> {
        let pick = |env_key: &str, file_v: Option<String>, default: &str| -> String {
            env(env_key)
                .filter(|s| !s.is_empty())
                .or(file_v)
                .unwrap_or_else(|| default.to_string())
        };
        let listen: SocketAddr = pick(ENV_LISTEN, file.listen, DEFAULT_LISTEN)
            .parse()
            .map_err(|_| Error::Config("listen 不是合法的 SocketAddr".into()))?;
        let advertise = match pick(ENV_ADVERTISE, file.advertise, "") {
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
            data_dir: PathBuf::from(pick(ENV_DATA_DIR, file.data_dir, DEFAULT_DATA_DIR)),
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

/// 卫星模式配置: 纯转发, 无状态目录; relay 令牌只走 env (NFR: 敏感字段不进文件)。
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
    /// 分层同 RelayConfig (ENV > CONFIG > DEFAULT); listen / advertise 复用同一组键,
    /// advertise 缺省取具体监听地址; core / 指纹可来自 env 或 relay.toml, 令牌只认 env。
    pub fn resolve(env: impl Fn(&str) -> Option<String>, file: FileConfig) -> Result<Self> {
        let pick = |env_key: &str, file_v: Option<String>| -> Option<String> {
            env(env_key)
                .filter(|s| !s.is_empty())
                .or(file_v)
                .filter(|s| !s.is_empty())
        };
        let core = pick(ENV_CORE, file.core)
            .ok_or_else(|| {
                Error::Config("卫星模式必须指定核心地址 (KUNXTIER_CORE 或 relay.toml core)".into())
            })?
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string();
        let listen: SocketAddr = pick(ENV_LISTEN, file.listen)
            .unwrap_or_else(|| DEFAULT_LISTEN.to_string())
            .parse()
            .map_err(|_| Error::Config("listen 不是合法的 SocketAddr".into()))?;
        let advertise = pick(ENV_ADVERTISE, file.advertise)
            .and_then(|s| s.parse().ok())
            .or((!listen.ip().is_unspecified()).then_some(listen))
            .ok_or_else(|| {
                Error::Config(
                    "卫星模式必须指定公布地址 (KUNXTIER_ADVERTISE 或 relay.toml advertise)".into(),
                )
            })?;
        let fingerprint = pick(ENV_FINGERPRINT, file.fingerprint).ok_or_else(|| {
            Error::Config(
                "卫星模式必须指定核心证书指纹 (KUNXTIER_FINGERPRINT 或 relay.toml fingerprint)"
                    .into(),
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

/// 节点 state 目录的 env 键 (本地开发覆盖用, 持久值进全局配置文件; 无 CLI flag)。
pub const ENV_NODE_STATE_DIR: &str = "KUNXTIER_STATE_DIR";

/// 节点机器级全局配置文件路径 (unix 各平台统一 /etc, 发行版 daemon 惯例,
/// 2026-10-27 布局修订; Windows 路径随平台切片定稿)。
pub fn node_config_path() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/etc/kunxtier/config.toml")
    }
    #[cfg(not(unix))]
    {
        PathBuf::from("./kunxtier-config.toml")
    }
}

/// state 目录平台默认 (unix 各平台统一 /var/lib; 原 macOS 与配置同置布局已废弃)。
pub fn node_state_dir_default() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/var/lib/kunxtier")
    }
    #[cfg(not(unix))]
    {
        PathBuf::from("./kunxtier-node-state")
    }
}

/// 日志目录平台默认 (unix 各平台统一 /var/log 下应用子目录)。
pub fn node_log_dir_default() -> PathBuf {
    #[cfg(unix)]
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
             # state / 日志目录与日志级别; 优先级 ENV > 本文件 > 默认\n\
             state_dir = \"{}\"\n\
             log_dir = \"{}\"\n\
             # log_level = \"info\"\n",
            node_state_dir_default().display(),
            node_log_dir_default().display(),
        )
    }

    /// daemon 首启生成: 文件缺失时以默认模板原子落盘 (0644, 可读可改), 已存在则
    /// 不动, 返回是否落盘; 与 install 共用 load_or_default, 已存在且非法同样报错。
    pub fn ensure_written(path: &Path) -> Result<bool> {
        let Some(content) = Self::load_or_default(path)?.1 else {
            return Ok(false);
        };
        crate::atomic_file::write_atomic(path, content.as_bytes(), 0o644)?;
        Ok(true)
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
    pub fn load(env: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let path = node_config_path();
        let file = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str::<NodeFileConfig>(&text).map_err(|e| {
                Error::Config(format!("全局配置解析失败 ({}): {e}", path.display()))
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => NodeFileConfig::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self::resolve(env, file))
    }

    /// state_dir 分层 ENV > CONFIG > DEFAULT (2026-11-06 反转; --state-dir flag
    /// 已拆除, KUNXTIER_STATE_DIR 接管); log_dir / log_level 为 CONFIG > DEFAULT。
    pub fn resolve(env: impl Fn(&str) -> Option<String>, file: NodeFileConfig) -> Self {
        let state_dir = env(ENV_NODE_STATE_DIR)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
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
        let cfg = RelayConfig::resolve(no_env, FileConfig::default()).unwrap();
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
        let cfg = RelayConfig::resolve(env, FileConfig::default()).unwrap();
        assert_eq!(cfg.stun_servers, vec!["1.1.1.1:3478", "2.2.2.2:3478"]);

        // 文件层覆盖缺省
        let file = FileConfig {
            stun_servers: Some(vec!["3.3.3.3:3478".into()]),
            ..FileConfig::default()
        };
        let cfg = RelayConfig::resolve(no_env, file).unwrap();
        assert_eq!(cfg.stun_servers, vec!["3.3.3.3:3478"]);
    }

    #[test]
    fn priority_env_over_file_over_default() {
        let env = |k: &str| match k {
            ENV_LISTEN => Some("1.1.1.1:1111".to_string()),
            ENV_DATA_DIR => Some("./from-env".to_string()),
            _ => None,
        };
        let file = FileConfig {
            listen: Some("2.2.2.2:2222".into()),
            data_dir: Some("./from-file".into()),
            ..FileConfig::default()
        };

        // 两层齐设: ENV 胜出
        let cfg = RelayConfig::resolve(env, file.clone()).unwrap();
        assert_eq!(cfg.listen.to_string(), "1.1.1.1:1111");
        assert_eq!(cfg.data_dir, PathBuf::from("./from-env"));

        // 无 ENV: FILE 胜过 DEFAULT
        let cfg = RelayConfig::resolve(no_env, file).unwrap();
        assert_eq!(cfg.listen.to_string(), "2.2.2.2:2222");
        assert_eq!(cfg.data_dir, PathBuf::from("./from-file"));
    }

    #[test]
    fn empty_env_value_falls_through_to_file() {
        let env = |k: &str| (k == ENV_LISTEN).then(String::new);
        let file = FileConfig {
            listen: Some("2.2.2.2:2222".into()),
            ..FileConfig::default()
        };
        let cfg = RelayConfig::resolve(env, file).unwrap();
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
        let env = |k: &str| (k == ENV_LISTEN).then(|| "not-a-socket".to_string());
        assert!(RelayConfig::resolve(env, FileConfig::default()).is_err());
    }

    #[test]
    fn satellite_mode_follows_token_presence_only() {
        // 非空即卫星, 空串视同未设置, 缺失即核心
        assert!(satellite_mode(|_| Some("kxt_relay_x".to_string())));
        assert!(!satellite_mode(|_| Some(String::new())));
        assert!(!satellite_mode(no_env));
    }

    #[test]
    fn relay_ensure_written_generates_missing_and_keeps_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("relay.toml");
        // 缺失: 建父目录并落默认模板, 0644, 可解析回配置层 (卫星字段注释呈现)
        assert!(FileConfig::ensure_written(&path).unwrap());
        let f = FileConfig::discover_at(&path).unwrap();
        assert_eq!(f.listen.as_deref(), Some(DEFAULT_LISTEN));
        assert_eq!(f.data_dir.as_deref(), Some(DEFAULT_DATA_DIR));
        assert_eq!(f.core, None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644, "relay 配置必须 0644");
        }
        // 已存在: 不改写 (自定义内容保留), 返回 false
        std::fs::write(&path, "listen = \"1.2.3.4:9443\"\n").unwrap();
        assert!(!FileConfig::ensure_written(&path).unwrap());
        assert!(std::fs::read_to_string(&path).unwrap().contains("1.2.3.4"));
        // 已存在但非法: 报错, 不静默覆盖
        std::fs::write(&path, "listen = [1]").unwrap();
        assert!(FileConfig::ensure_written(&path).is_err());
    }

    #[test]
    fn discover_at_missing_is_default_invalid_rejected_fields_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.toml");
        // 缺失: 视同空配置, 全默认可用
        let f = FileConfig::discover_at(&path).unwrap();
        assert_eq!(f.listen, None);
        assert_eq!(f.core, None);
        // 存在且合法: 字段读入 (含卫星专属 core / fingerprint)
        std::fs::write(
            &path,
            "listen = \"1.2.3.4:9443\"\ncore = \"5.6.7.8:9443\"\nfingerprint = \"ab\"\n",
        )
        .unwrap();
        let f = FileConfig::discover_at(&path).unwrap();
        assert_eq!(f.listen.as_deref(), Some("1.2.3.4:9443"));
        assert_eq!(f.core.as_deref(), Some("5.6.7.8:9443"));
        assert_eq!(f.fingerprint.as_deref(), Some("ab"));
        // 存在但非法: 报错, 不静默回退默认
        std::fs::write(&path, "listen = [1]").unwrap();
        assert!(FileConfig::discover_at(&path).is_err());
    }

    #[test]
    fn node_config_defaults_follow_platform() {
        let cfg = NodeGlobalConfig::resolve(no_env, NodeFileConfig::default());
        assert_eq!(cfg.state_dir, node_state_dir_default());
        assert_eq!(cfg.log_dir, node_log_dir_default());
        assert_eq!(cfg.log_level, None);
        // 平台默认显式核对 (unix 各平台同构, 2026-10-27 布局修订)
        #[cfg(unix)]
        {
            assert_eq!(cfg.state_dir, PathBuf::from("/var/lib/kunxtier"));
            assert_eq!(cfg.log_dir, PathBuf::from("/var/log/kunxtier"));
        }
    }

    #[test]
    fn node_config_state_dir_priority_env_file_default() {
        let file = NodeFileConfig {
            state_dir: Some("./from-file".into()),
            log_dir: Some("/logs".into()),
            log_level: Some("debug".into()),
        };
        // ENV 胜出
        let env = |k: &str| (k == ENV_NODE_STATE_DIR).then(|| "./from-env".to_string());
        let cfg = NodeGlobalConfig::resolve(env, file.clone());
        assert_eq!(cfg.state_dir, PathBuf::from("./from-env"));
        // 无 ENV: FILE 胜默认; log 层随文件
        let cfg = NodeGlobalConfig::resolve(no_env, file);
        assert_eq!(cfg.state_dir, PathBuf::from("./from-file"));
        assert_eq!(cfg.log_dir, PathBuf::from("/logs"));
        assert_eq!(cfg.log_level.as_deref(), Some("debug"));
        // 空 env 视同未设置
        let empty_env = |k: &str| (k == ENV_NODE_STATE_DIR).then(String::new);
        let cfg = NodeGlobalConfig::resolve(empty_env, NodeFileConfig::default());
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
    fn ensure_written_generates_missing_template_and_keeps_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");

        // 缺失: 建父目录并落默认模板, 0644, 可解析回配置层
        assert!(NodeFileConfig::ensure_written(&path).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: NodeFileConfig = toml::from_str(&text).unwrap();
        assert_eq!(
            parsed.state_dir.as_deref(),
            Some(node_state_dir_default().to_str().unwrap())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644, "全局配置必须 0644");
        }

        // 已存在: 不改写 (自定义内容保留), 返回 false
        std::fs::write(&path, "state_dir = \"/custom\"\n").unwrap();
        assert!(!NodeFileConfig::ensure_written(&path).unwrap());
        assert!(std::fs::read_to_string(&path).unwrap().contains("/custom"));
    }

    #[test]
    fn ensure_written_rejects_invalid_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "state_dir = [1]").unwrap();
        assert!(NodeFileConfig::ensure_written(&path).is_err());
    }

    #[test]
    fn ensure_written_rejects_unreadable_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "state_dir = \"/x\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        assert!(NodeFileConfig::ensure_written(&path).is_err());
    }

    #[test]
    fn ensure_written_rejects_directory_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("as-dir");
        std::fs::create_dir(&path).unwrap();
        assert!(NodeFileConfig::ensure_written(&path).is_err());
    }

    #[test]
    fn ensure_written_treats_empty_existing_file_as_kept() {
        // 空文件是合法 (全默认) 配置, 视为已存在: 不生成不改写
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        assert!(!NodeFileConfig::ensure_written(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
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
    fn satellite_resolves_env_file_and_requires_sensitive_env() {
        // 全 env 形态: advertise 缺省取具体监听地址; core 容忍 https:// 前缀
        let env = |k: &str| match k {
            ENV_RELAY_TOKEN => Some("kxt_relay_secret".into()),
            ENV_CORE => Some("https://5.6.7.8:9443".into()),
            ENV_LISTEN => Some("127.0.0.1:8443".into()),
            ENV_FINGERPRINT => Some("ab".repeat(32)),
            _ => None,
        };
        let cfg = SatelliteConfig::resolve(env, FileConfig::default()).unwrap();
        assert_eq!(cfg.core, "5.6.7.8:9443");
        assert_eq!(cfg.listen.to_string(), "127.0.0.1:8443");
        assert_eq!(cfg.advertise.to_string(), "127.0.0.1:8443");

        // 文件层承载 core / listen / advertise / 指纹; 令牌仍只认 env
        let file = FileConfig {
            core: Some("5.6.7.8:9443".into()),
            listen: Some("0.0.0.0:9443".into()),
            advertise: Some("1.2.3.4:443".into()),
            fingerprint: Some("ab".repeat(32)),
            ..FileConfig::default()
        };
        let token_only = |k: &str| (k == ENV_RELAY_TOKEN).then(|| "kxt_relay_secret".to_string());
        let cfg = SatelliteConfig::resolve(token_only, file).unwrap();
        assert_eq!(cfg.core, "5.6.7.8:9443");
        assert_eq!(cfg.listen.to_string(), "0.0.0.0:9443");
        assert_eq!(cfg.advertise.to_string(), "1.2.3.4:443");

        // 令牌只认 env: 缺失即拒绝, 不回落文件
        let no_token = |_: &str| None::<String>;
        assert!(SatelliteConfig::resolve(
            no_token,
            FileConfig {
                core: Some("5.6.7.8:9443".into()),
                fingerprint: Some("ab".repeat(32)),
                ..FileConfig::default()
            },
        )
        .is_err());

        // core 缺失 (env / 文件均无): 拒绝
        assert!(SatelliteConfig::resolve(token_only, FileConfig::default()).is_err());

        // 通配监听且无 advertise: 拒绝启动
        let wildcard = FileConfig {
            core: Some("5.6.7.8:9443".into()),
            listen: Some("0.0.0.0:9443".into()),
            fingerprint: Some("ab".repeat(32)),
            ..FileConfig::default()
        };
        assert!(SatelliteConfig::resolve(token_only, wildcard).is_err());
    }
}
