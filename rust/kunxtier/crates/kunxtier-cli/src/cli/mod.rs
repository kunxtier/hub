//! Description: CLI 装配层: 顶层命令定义与一层委派。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

pub mod admin;
pub mod error;
mod relay;

use clap::{Parser, Subcommand};

pub use admin::AdminCommand;
use error::Fail;
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
}

impl Command {
    pub fn execute(self) -> Result<(), Fail> {
        match self {
            Command::Relay(cmd) => cmd.execute(),
            Command::Admin(cmd) => cmd.execute(),
        }
    }
}
