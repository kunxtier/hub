//! Description: kunxtier-node: 节点数据面 (TUN + WG 引擎 + 信令 + 打洞 + 路径管理)。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 当前切片 (T08): 复合传输 (直连 UDP 分流 + 中继伪端点)、STUN 探测、
//! 并发打洞与超时回落、节点级强制中继、直连保活观测与失活回落、周期重试直连
//! (重试期间中继流量不断)、本地网段 (CGNAT) 冲突检测、MTU 按最坏路径推导。
//! daemon 服务化 / 本地 IPC 随 T09 接入。

pub mod engine;
pub mod error;
pub mod identity;
pub mod netconf;
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
pub use netconf::{detect_conflicts, IfaceAddr, SubnetConflict};
pub use runtime::{start, NodeHandle, NodeOptions, NodeStatus, PathTimings, PeerStatus};
pub use tun::{TunAdapter, TunConfig, DEFAULT_MTU};
