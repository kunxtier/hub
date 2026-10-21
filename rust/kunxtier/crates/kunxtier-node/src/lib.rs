//! Description: kunxtier-node: 节点数据面 (TUN 设备 + WireGuard 用户态引擎)。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 当前切片 (T06): 引擎装配与运行时 peer 管理、tun-rs 设备适配、节点 WG 身份。
//! 信令客户端与打洞随 T07 接入; 直连 / 中继复合传输与路由策略随 T08 接入。

pub mod engine;
pub mod error;
pub mod identity;
pub mod tun;

pub use engine::{build_engine, build_engine_split, EngineOptions, PeerSpec, WgEngine};
pub use error::{Error, Result};
pub use identity::WgIdentity;
pub use tun::{TunAdapter, TunConfig, DEFAULT_MTU};
