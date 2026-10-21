//! Description: admin user 子命令: 网络内用户的创建与禁用。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

use clap::Subcommand;
use serde_json::json;

use super::remote::{print_result, AdminRemote};
use crate::cli::error::Fail;

#[derive(Subcommand)]
pub enum UserAction {
    /// 在网络内创建用户
    Create {
        #[arg(long)]
        network: String,
        #[arg(long)]
        name: String,
    },
    /// 禁用用户 (其名下令牌全部失效)
    Disable {
        #[arg(long)]
        network: String,
        #[arg(long)]
        name: String,
    },
}

impl UserAction {
    pub fn execute(self, remote: &AdminRemote) -> Result<(), Fail> {
        match self {
            UserAction::Create { network, name } => {
                let v = remote.post("/users", &json!({"network": network, "name": name}))?;
                print_result(&v);
            }
            UserAction::Disable { network, name } => {
                let v =
                    remote.post("/users/disable", &json!({"network": network, "name": name}))?;
                print_result(&v);
            }
        }
        Ok(())
    }
}
