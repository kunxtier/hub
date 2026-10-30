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
    /// 用户 (属主) 列表
    List {
        /// 网络组 (缺省跨网络组全量)
        #[arg(long)]
        network: Option<String>,
    },
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
    /// 启用用户 (禁用的逆操作, 名下令牌恢复可用)
    Enable {
        #[arg(long)]
        network: String,
        #[arg(long)]
        name: String,
    },
    /// 改名 (节点与令牌按 user_id 关联, 属主标注随之传播)
    Rename {
        #[arg(long)]
        network: String,
        #[arg(long)]
        name: String,
        new_name: String,
    },
    /// 删除用户 (名下有未移除节点时拒绝; 历史令牌随用户清理)
    Remove {
        #[arg(long)]
        network: String,
        #[arg(long)]
        name: String,
    },
}

impl UserAction {
    pub fn execute(self, remote: &AdminRemote) -> Result<(), Fail> {
        match self {
            UserAction::List { network } => {
                let v = match network {
                    Some(n) => remote.get(&format!("/users?network={n}"))?,
                    None => remote.get("/users")?,
                };
                print_result(&v);
            }
            UserAction::Create { network, name } => {
                let v = remote.post("/users", &json!({"network": network, "name": name}))?;
                print_result(&v);
            }
            UserAction::Disable { network, name } => {
                let v =
                    remote.post("/users/disable", &json!({"network": network, "name": name}))?;
                print_result(&v);
            }
            UserAction::Enable { network, name } => {
                let v = remote.post("/users/enable", &json!({"network": network, "name": name}))?;
                print_result(&v);
            }
            UserAction::Rename {
                network,
                name,
                new_name,
            } => {
                let v = remote.post(
                    "/users/rename",
                    &json!({"network": network, "name": name, "new_name": new_name}),
                )?;
                print_result(&v);
            }
            UserAction::Remove { network, name } => {
                let v = remote.post("/users/delete", &json!({"network": network, "name": name}))?;
                print_result(&v);
            }
        }
        Ok(())
    }
}
