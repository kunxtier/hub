//! Description: admin node 子命令: 节点概览与移除。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

use clap::{Subcommand, ValueEnum};
use serde_json::json;

use super::remote::{print_result, AdminRemote};
use crate::cli::error::Fail;

/// 节点级路径策略 (FR-10): 与服务端 PathPolicy 的线格式一致。
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum PolicyValue {
    Auto,
    ForceRelay,
}

impl PolicyValue {
    fn as_str(self) -> &'static str {
        match self {
            PolicyValue::Auto => "auto",
            PolicyValue::ForceRelay => "force_relay",
        }
    }
}

#[derive(Subcommand)]
pub enum NodeAction {
    /// 节点概览 (含属主 / 在线状态 / 路径策略)
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
    /// 设置节点级路径策略 (auto 直连优先回落中继 / force_relay 强制中继)
    Policy {
        #[arg(long)]
        network: String,
        #[arg(long)]
        name: String,
        policy: PolicyValue,
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
            NodeAction::Policy {
                network,
                name,
                policy,
            } => {
                let v = remote.post(
                    "/nodes/policy",
                    &json!({"network": network, "name": name, "policy": policy.as_str()}),
                )?;
                print_result(&v);
            }
        }
        Ok(())
    }
}
