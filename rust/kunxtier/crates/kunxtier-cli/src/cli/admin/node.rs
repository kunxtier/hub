//! Description: admin node 子命令: 节点概览与移除。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

use clap::Subcommand;
use serde_json::json;

use super::remote::{print_result, AdminRemote};
use crate::cli::error::Fail;

#[derive(Subcommand)]
pub enum NodeAction {
    /// 节点概览 (含属主 / 在线状态)
    List {
        #[arg(long)]
        network: Option<String>,
    },
    /// 移除节点 (凭证随之失效)
    Remove {
        #[arg(long)]
        network: String,
        #[arg(long)]
        name: String,
    },
}

impl NodeAction {
    pub fn execute(self, remote: &AdminRemote) -> Result<(), Fail> {
        match self {
            NodeAction::List { network } => {
                let v = match network {
                    Some(n) => remote.get(&format!("/nodes?network={n}"))?,
                    None => remote.get("/nodes")?,
                };
                print_result(&v);
            }
            NodeAction::Remove { network, name } => {
                let v = remote.post("/nodes/remove", &json!({"network": network, "name": name}))?;
                print_result(&v);
            }
        }
        Ok(())
    }
}
