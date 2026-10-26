//! Description: kunxtier-node: 节点数据面 (TUN + WG 引擎 + 信令 + 打洞 + 路径管理 + daemon)。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 当前切片 (T09): 复合传输、STUN 探测、并发打洞与超时回落、节点级强制中继、
//! 直连保活观测与失活回落、本地网段 (CGNAT) 冲突检测、MTU 按最坏路径推导;
//! daemon 会话生命周期 (一次性令牌注册 / state 持久化 / 自动恢复 / 退避重连)
//! 与本地 IPC 服务端; Windows named pipe IPC 与服务化随 T13/T14 落地。
//! T15 增补: 关键事件留痕环 (events) 与 status --verbose 排障视图。

pub mod daemon;
pub mod engine;
pub mod error;
pub mod events;
pub mod identity;
pub mod ipc;
pub mod netconf;
pub mod punch;
pub mod register;
pub mod relay_conn;
pub mod runtime;
pub mod signal;
pub mod state;
pub mod stun;
pub mod transport;
pub mod tun;

pub use daemon::{serve, DaemonOptions, TunAdapterFactory, TunFactory};
pub use engine::{build_engine, build_engine_split, EngineOptions, PeerSpec, WgEngine};
pub use error::{Error, Result};
pub use events::{EventKind, EventRecord, EventRing};
pub use identity::WgIdentity;
pub use ipc::{socket_path, IpcPeer, IpcRequest, IpcResponse, IpcStatus};
pub use netconf::{detect_conflicts, IfaceAddr, SubnetConflict};
pub use runtime::{start, NodeHandle, NodeOptions, NodeStatus, PathTimings, PeerStatus};
pub use state::{NodeState, STATE_VERSION};
pub use tun::{TunAdapter, TunConfig, DEFAULT_MTU};
