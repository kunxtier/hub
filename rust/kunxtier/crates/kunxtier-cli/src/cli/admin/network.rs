//! Description: admin network 子命令: 网络组列表与创建。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

use clap::Subcommand;
use serde_json::{json, Value};

use super::remote::{print_result, AdminRemote};
use crate::cli::error::Fail;

#[derive(Subcommand)]
pub enum NetworkAction {
    /// 网络组列表
    List,
    /// 创建网络组
    Create {
        name: String,
        /// 形如 100.66.0.0/16; 缺省按创建顺序取下一个 /16
        #[arg(long)]
        cidr: Option<String>,
    },
}

impl NetworkAction {
    pub fn execute(self, remote: &AdminRemote) -> Result<(), Fail> {
        match self {
            NetworkAction::List => {
                let v = remote.get("/networks")?;
                print_result(&v);
            }
            NetworkAction::Create { name, cidr } => {
                let mut body = json!({"name": name});
                if let Some(c) = cidr {
                    body["cidr"] = Value::String(c);
                }
                let v = remote.post("/networks", &body)?;
                print_result(&v);
            }
        }
        Ok(())
    }
}
