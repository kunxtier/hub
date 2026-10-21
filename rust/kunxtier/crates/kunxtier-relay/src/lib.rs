//! Description: kunxtier-relay: 核心中继。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 当前切片: 初始化 (自签证书 / admin 凭证 / 默认网络)、管理 API、
//! 节点登记 (一次性令牌消费 / IP 分配 / 节点凭证签发)、限速与备份。
//! 信令 WS / 内置转发 / 控制台托管在后续阶段接入。

pub mod cert;
pub mod client_tls;
pub mod error;
pub mod init;
pub mod ratelimit;
pub mod server;
pub mod store;

pub use error::{Error, Result};
pub use init::{init, InitOutput};
