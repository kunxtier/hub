//! Description: admin token 子命令: 令牌签发与撤销。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

use clap::Subcommand;
use serde_json::{json, Value};

use super::remote::{print_result, AdminRemote};
use crate::cli::error::Fail;

#[derive(Subcommand)]
pub enum TokenAction {
    /// 签发令牌 (默认一次性节点令牌; --type relay 为可复用卫星令牌)
    Issue {
        #[arg(long)]
        network: String,
        #[arg(long)]
        user: Option<String>,
        #[arg(long = "type")]
        r#type: Option<String>,
        /// 有效期秒数
        #[arg(long)]
        ttl: Option<u64>,
    },
    /// 撤销令牌
    Revoke { id: i64 },
}

impl TokenAction {
    pub fn execute(self, remote: &AdminRemote) -> Result<(), Fail> {
        let v = match self {
            TokenAction::Issue {
                network,
                user,
                r#type,
                ttl,
            } => {
                let mut body = json!({"network": network});
                if let Some(u) = user {
                    body["user"] = Value::String(u);
                }
                if let Some(t) = r#type {
                    body["kind"] = Value::String(t);
                }
                if let Some(t) = ttl {
                    body["ttl_secs"] = json!(t);
                }
                remote.post("/tokens", &body)?
            }
            TokenAction::Revoke { id } => {
                remote.post(&format!("/tokens/{id}/revoke"), &json!({}))?
            }
        };
        print_result(&v);
        if v.get("token").is_some() {
            println!("提示: 令牌明文仅此一次展示, 请立即分发给使用者");
        }
        Ok(())
    }
}
