//! Description: CLI 装配层: 顶层命令定义与一层委派。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

pub mod admin;
mod console;
pub mod error;
mod node;
mod relay;

use clap::{Parser, Subcommand};

use crate::logging::LogSpec;
pub use admin::AdminCommand;
pub use console::ConsoleArgs;
use error::Fail;
pub use node::{DaemonArgs, DownArgs, StatusArgs, UpArgs};
pub use relay::RelayCommand;

#[derive(Parser)]
#[command(
    name = "kunxtier",
    version,
    about = "轻量 P2P 虚拟组网 (ZeroTier / EasyTier 平替)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// 运行核心中继 (管理 + 登记); --init 初始化状态目录; --satellite 起纯转发卫星
    Relay(RelayCommand),
    /// 管理操作; admin 凭证走 env KUNXTIER_ADMIN
    Admin(AdminCommand),
    /// 前台运行节点守护进程 (up / down / status 的服务端)
    Daemon(DaemonArgs),
    /// 首次注册并连接 (令牌一次性); 需 daemon 在运行
    Up(UpArgs),
    /// 断开网络会话 (daemon 留守, 注册身份保留)
    Down(DownArgs),
    /// 本机与对端状态 (路径 direct / relay)
    Status(StatusArgs),
    /// 本地起 Web 控制台 (托管页面并反代核心 admin API)
    Console(ConsoleArgs),
}

impl Command {
    /// 日志装配输入: 长驻进程 (relay / daemon) 携带 flag 层, relay 另取配置文件层。
    pub fn log_spec(&self) -> (LogSpec, Option<String>) {
        match self {
            Command::Relay(cmd) => cmd.log_spec(),
            Command::Daemon(args) => (
                LogSpec {
                    level: args.log_level.clone(),
                    file: args.log_file.clone(),
                },
                None,
            ),
            _ => (LogSpec::default(), None),
        }
    }

    pub fn execute(self) -> Result<(), Fail> {
        match self {
            Command::Relay(cmd) => cmd.execute(),
            Command::Admin(cmd) => cmd.execute(),
            Command::Daemon(cmd) => cmd.execute(),
            Command::Up(cmd) => cmd.execute(),
            Command::Down(cmd) => cmd.execute(),
            Command::Status(cmd) => cmd.execute(),
            Command::Console(cmd) => cmd.execute(),
        }
    }
}
