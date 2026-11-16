//! Description: relay 子命令: 核心中继 run / backup 与卫星模式装配 (零配置类 flag)。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 配置只来自 env 与 /etc/kunxtier/relay.toml (ENV > CONFIG > DEFAULT, 2026-11-06
//! 决策); 卫星模式由 KUNXTIER_RELAY_TOKEN 存在性判定, 无独立开关; 首启自动初始化。

use std::path::PathBuf;

use clap::{Args, Subcommand};

use kunxtier_core::config::{satellite_mode, FileConfig, RelayConfig, SatelliteConfig};

use super::error::Fail;
use crate::logging::LogSpec;

#[derive(Args)]
pub struct RelayCommand {
    /// 日志级别 error/warn/info/debug/trace (覆盖 relay.toml 与 RUST_LOG)
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
    /// 日志装配: flag 层 + relay.toml log_level 层 (固定路径预读)。
    /// 预读失败时置 None, 正式加载的错误仍由 execute 报出。
    pub fn log_spec(&self) -> (LogSpec, Option<String>) {
        let config_level = FileConfig::discover().ok().and_then(|f| f.log_level);
        (
            LogSpec {
                level: self.log_level.clone(),
                file: self.log_file.clone(),
            },
            config_level,
        )
    }

    pub fn execute(self) -> Result<(), Fail> {
        let file =
            FileConfig::discover().map_err(|e| Fail::usage(format!("relay 配置加载失败: {e}")))?;
        // 卫星判定语义 (含冲突容忍理由) 收口在 config::satellite_mode
        if satellite_mode(|k| std::env::var(k).ok()) {
            self.satellite_mode(file)
        } else {
            self.core_mode(file)
        }
    }

    fn satellite_mode(self, file: FileConfig) -> Result<(), Fail> {
        if self.action.is_some() {
            return Err(Fail::usage(
                "已设置 KUNXTIER_RELAY_TOKEN, 当前为卫星模式 (无本地管理数据), 不支持 backup; \
                 核心备份请在未设置该 env 的环境执行",
            ));
        }
        let cfg = SatelliteConfig::resolve(|k| std::env::var(k).ok(), file)
            .map_err(|e| Fail::usage(format!("卫星配置解析失败: {e}")))?;
        // expect 理由: 仅在系统线程资源耗尽时失败, 属环境级故障, 无恢复价值
        let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
        rt.block_on(kunxtier_relay::satellite::run(&cfg))
            .map_err(|e| Fail::runtime(format!("卫星中继退出: {e}")))
    }

    fn core_mode(self, file: FileConfig) -> Result<(), Fail> {
        let cfg = RelayConfig::resolve(|k| std::env::var(k).ok(), file)
            .map_err(|e| Fail::usage(format!("配置解析失败: {e}")))?;

        if let Some(RelayAction::Backup { out }) = self.action {
            kunxtier_relay::store::Store::open(&cfg.db_path())
                .and_then(|s| s.backup_to(&out))
                .map_err(|e| Fail::runtime(format!("备份失败: {e}")))?;
            println!("备份完成: {}", out.display());
            return Ok(());
        }

        // 未初始化的 state 目录就地进行 init: 裸命令首启即完整可用, 无前置命令
        if let Some(out) = kunxtier_relay::ensure_initialized(&cfg)
            .map_err(|e| Fail::runtime(format!("自动初始化失败: {e}")))?
        {
            eprintln!("数据目录未初始化, 已自动完成初始化");
            // 服务化 (无终端) 时凭证只落文件不回显: journal 属持久日志, 明文留存违反安全红线
            use std::io::IsTerminal;
            if std::io::stderr().is_terminal() {
                eprintln!(
                    "  admin 凭证 (已写入 {}/{}):",
                    out.data_dir,
                    kunxtier_relay::ADMIN_CREDENTIAL_FILE
                );
                eprintln!("    {}", out.admin_credential);
            } else {
                eprintln!(
                    "  admin 凭证已写入 {}/{} (服务化日志不回显明文)",
                    out.data_dir,
                    kunxtier_relay::ADMIN_CREDENTIAL_FILE
                );
            }
            eprintln!("  服务端证书指纹 (客户端固定使用):");
            eprintln!("    {}", out.fingerprint);
        }

        // expect 理由: 仅在系统线程资源耗尽时失败, 属环境级故障, 无恢复价值
        let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
        rt.block_on(kunxtier_relay::server::run(&cfg))
            .map_err(|e| Fail::runtime(format!("核心中继退出: {e}")))
    }
}
