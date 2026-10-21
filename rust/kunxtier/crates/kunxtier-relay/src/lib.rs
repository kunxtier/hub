//! Description: kunxtier-relay: 核心中继。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 当前切片: 初始化 (自签证书 / admin 凭证 / 默认网络)、管理 API、
//! 节点登记 (一次性令牌消费 / IP 分配 / 节点凭证签发)、信令 WS
//! (节点凭证挑战应答 / 在线状态 / 节点表分发 / 打洞信令转发)、
//! 转发双模式 (核心内置 /ws/relay + 卫星进程与注册分发)、限速与备份。
//! 控制台托管在后续阶段接入。

pub mod cert;
pub mod client_tls;
pub mod error;
pub mod forward;
pub mod init;
pub mod ratelimit;
pub mod satellite;
pub mod server;
pub mod signaling;
pub mod store;

pub use error::{Error, Result};
pub use init::{init, InitOutput};
