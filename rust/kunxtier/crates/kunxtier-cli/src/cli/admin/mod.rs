//! Description: admin 子命令: 通过指纹固定的 HTTPS 调用核心中继 admin API。
//!
//! Author: qinzhenya
//! Created: 2026-10-20

mod network;
mod node;
mod remote;
mod token;
mod user;

use clap::{Args, Subcommand};

use network::NetworkAction;
use node::NodeAction;
use token::TokenAction;
use user::UserAction;

use crate::cli::error::Fail;
use remote::AdminRemote;

#[derive(Args)]
pub struct AdminCommand {
    #[arg(long)]
    server: String,
    #[arg(long, env = "KUNXTIER_FINGERPRINT")]
    fingerprint: String,
    #[command(subcommand)]
    action: AdminAction,
}

// 子命令组按领域嵌套 (network create / user disable 等),
// 与 .docs 需求文档 (feat-kunxtier-cli) 约定的 CLI 形态一致
#[derive(Subcommand)]
enum AdminAction {
    /// 网络组管理
    Network {
        #[command(subcommand)]
        action: NetworkAction,
    },
    /// 用户管理
    User {
        #[command(subcommand)]
        action: UserAction,
    },
    /// 令牌管理
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
    /// 节点管理
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },
}

impl AdminCommand {
    pub fn execute(self) -> Result<(), Fail> {
        let credential = std::env::var("KUNXTIER_ADMIN").map_err(|_| {
            Fail::usage("缺少 admin 凭证: 请设置环境变量 KUNXTIER_ADMIN (见 relay --init 输出)")
        })?;
        let remote = AdminRemote::new(&self.server, &self.fingerprint, &credential);
        match self.action {
            AdminAction::Network { action } => action.execute(&remote),
            AdminAction::User { action } => action.execute(&remote),
            AdminAction::Token { action } => action.execute(&remote),
            AdminAction::Node { action } => action.execute(&remote),
        }
    }
}
