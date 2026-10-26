//! Description: kunxtier-node: 节点数据面 (TUN + WG 引擎 + 信令 + 打洞 + 路径策略)。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 当前切片 (T07): 复合传输 (直连 UDP 分流 + 中继伪端点)、STUN 探测、
//! 并发打洞与超时回落、节点级强制中继、信令客户端与节点运行时。
//! daemon 服务化 / 本地 IPC / 周期性直连重试与保活精化随 T08-T09 接入。

pub mod engine;
pub mod error;
pub mod identity;
pub mod punch;
pub mod relay_conn;
pub mod runtime;
pub mod signal;
pub mod stun;
pub mod transport;
pub mod tun;

pub use engine::{build_engine, build_engine_split, EngineOptions, PeerSpec, WgEngine};
pub use error::{Error, Result};
pub use identity::WgIdentity;
pub use runtime::{start, NodeHandle, NodeOptions, NodeStatus, PeerStatus};
pub use tun::{TunAdapter, TunConfig, DEFAULT_MTU};
