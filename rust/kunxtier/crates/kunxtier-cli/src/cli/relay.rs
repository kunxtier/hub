//! Description: relay 子命令: 核心中继 init / run / backup 与卫星模式装配。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

use std::path::PathBuf;

use clap::{Args, Subcommand};

use kunxtier_core::config::{
    CliOverrides, FileConfig, RelayConfig, SatelliteConfig, SatelliteOverrides,
};

use super::error::Fail;
use crate::logging::LogSpec;

#[derive(Args)]
pub struct RelayCommand {
    /// 初始化: 自签证书 / admin 凭证 / 默认网络 (明文凭证仅展示一次)
    #[arg(long)]
    init: bool,
    /// 卫星模式: 纯转发, 以 relay 令牌向核心注册 (与 --init 互斥)
    #[arg(long)]
    satellite: bool,
    /// 卫星模式: 核心中继地址 (host:port 或 https://host:port)
    #[arg(long)]
    core: Option<String>,
    /// 对外公布的本机地址 (中继列表内置条目 / 卫星注册声明用)
    #[arg(long)]
    advertise: Option<String>,
    /// 卫星模式: 核心证书指纹 (relay --init 输出)
    #[arg(long, env = "KUNXTIER_FINGERPRINT")]
    fingerprint: Option<String>,
    /// 配置文件路径 (relay.toml)
    #[arg(long)]
    config: Option<PathBuf>,
    /// 监听地址 (覆盖 env / 配置文件)
    #[arg(long)]
    listen: Option<String>,
    /// 状态数据目录 (覆盖 env / 配置文件)
    #[arg(long)]
    data_dir: Option<String>,
    /// 日志级别 error/warn/info/debug/trace (覆盖配置文件与 RUST_LOG)
    #[arg(long)]
    log_level: Option<String>,
    /// 日志文件路径 (启用后 stderr / 文件双写, 按大小轮转保留一份 .old)
    #[arg(long)]
    log_file: Option<PathBuf>,
    #[command(subcommand)]
    action: Option<RelayAction>,
}

#[derive(Subcommand)]
pub enum RelayAction {
    /// 导出一致性备份 (SQLite 快照)
    Backup { out: PathBuf },
}

impl RelayCommand {
    /// 日志装配: flag 层 + 配置文件 log_level 层。
    /// 文件预读失败时置 None, 正式加载的错误仍由 execute 报出。
    pub fn log_spec(&self) -> (LogSpec, Option<String>) {
        let config_level = self
            .config
            .as_deref()
            .and_then(|p| FileConfig::load(p).ok())
            .and_then(|f| f.log_level);
        (
            LogSpec {
                level: self.log_level.clone(),
                file: self.log_file.clone(),
            },
            config_level,
        )
    }

    pub fn execute(self) -> Result<(), Fail> {
        let file = match self.config.as_deref() {
            Some(p) => {
                FileConfig::load(p).map_err(|e| Fail::usage(format!("配置文件加载失败: {e}")))?
            }
            None => FileConfig::default(),
        };
        if self.satellite {
            self.satellite_mode(file)
        } else {
            self.core_mode(file)
        }
    }

    fn satellite_mode(self, file: FileConfig) -> Result<(), Fail> {
        if self.init {
            return Err(Fail::usage(
                "--satellite 与 --init 互斥: 卫星无状态目录, 无需初始化",
            ));
        }
        if self.action.is_some() {
            return Err(Fail::usage("--satellite 模式不支持 backup 等子动作"));
        }
        let cfg = SatelliteConfig::resolve(
            SatelliteOverrides {
                core: self.core,
                listen: self.listen,
                advertise: self.advertise,
                fingerprint: self.fingerprint,
            },
            |k| std::env::var(k).ok(),
            file,
        )
        .map_err(|e| Fail::usage(format!("卫星配置解析失败: {e}")))?;
        // expect 理由: 仅在系统线程资源耗尽时失败, 属环境级故障, 无恢复价值
        let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
        rt.block_on(kunxtier_relay::satellite::run(&cfg))
            .map_err(|e| Fail::runtime(format!("卫星中继退出: {e}")))
    }

    fn core_mode(self, file: FileConfig) -> Result<(), Fail> {
        let cfg = RelayConfig::resolve(
            CliOverrides {
                listen: self.listen,
                data_dir: self.data_dir,
                advertise: self.advertise,
            },
            |k| std::env::var(k).ok(),
            file,
        )
        .map_err(|e| Fail::usage(format!("配置解析失败: {e}")))?;

        if let Some(RelayAction::Backup { out }) = self.action {
            kunxtier_relay::store::Store::open(&cfg.db_path())
                .and_then(|s| s.backup_to(&out))
                .map_err(|e| Fail::runtime(format!("备份失败: {e}")))?;
            println!("备份完成: {}", out.display());
            return Ok(());
        }

        if self.init {
            let out = kunxtier_relay::init(&cfg)
                .map_err(|e| Fail::runtime(format!("初始化失败: {e}")))?;
            println!("初始化完成");
            println!("  状态目录: {}", out.data_dir);
            println!();
            println!(
                "  admin 凭证 (已写入 {}, 首次登录后建议删除):",
                kunxtier_relay::ADMIN_CREDENTIAL_FILE
            );
            println!("    {}", out.admin_credential);
            println!();
            println!("  服务端证书指纹 (客户端固定使用):");
            println!("    {}", out.fingerprint);
            println!();
            println!("  启动: kunxtier relay");
            println!(
                "  管理: KUNXTIER_ADMIN=<上面的 admin 凭证> kunxtier admin --server https://<addr> --fingerprint {} network list",
                out.fingerprint
            );
            return Ok(());
        }

        // 未初始化的 state 目录就地进行 init: 免记忆前置命令, 首启即完整可用
        if let Some(out) = kunxtier_relay::ensure_initialized(&cfg)
            .map_err(|e| Fail::runtime(format!("自动初始化失败: {e}")))?
        {
            eprintln!("数据目录未初始化, 已自动完成初始化");
            eprintln!(
                "  admin 凭证 (已写入 {}/{}):",
                out.data_dir,
                kunxtier_relay::ADMIN_CREDENTIAL_FILE
            );
            eprintln!("    {}", out.admin_credential);
            eprintln!("  服务端证书指纹 (客户端固定使用):");
            eprintln!("    {}", out.fingerprint);
        }

        // expect 理由: 仅在系统线程资源耗尽时失败, 属环境级故障, 无恢复价值
        let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
        rt.block_on(kunxtier_relay::server::run(&cfg))
            .map_err(|e| Fail::runtime(format!("核心中继退出: {e}")))
    }
}
