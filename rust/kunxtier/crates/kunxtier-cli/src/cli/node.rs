//! Description: 节点子命令: daemon 前台运行与 up / down / status 本地控制 (FR-1 / FR-2)。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! daemon 是常驻服务端 (持有 TUN 与运行时会话); up / down / status 是
//! 经本地 IPC 的薄客户端, daemon 未运行时给出明确提示与启动指引。
//! state 目录承载节点身份与 IPC socket, 定位走机器级全局配置
//! (CLI > ENV > CONFIG > 平台默认, 见 feat-kunxtier-cli "全局配置与目录布局")。

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;

use kunxtier_core::config::{node_config_path, NodeGlobalConfig};
use kunxtier_node::daemon::{serve, DaemonOptions, TunAdapterFactory};
use kunxtier_node::events::EventRecord;
use kunxtier_node::ipc::{self, IpcRequest, IpcResponse, IpcStatus};

use super::error::Fail;

/// daemon 重连退避起步值 (指数增长至 30s)。
const RETRY_BASE: Duration = Duration::from_secs(1);

/// 解析节点全局配置; 文件缺失走平台默认, 解析失败即时报错。
fn node_global(cli_state_dir: Option<PathBuf>) -> Result<NodeGlobalConfig, Fail> {
    NodeGlobalConfig::load(cli_state_dir, |k| std::env::var(k).ok()).map_err(|e| {
        Fail::usage(format!(
            "全局配置读取失败 ({}): {e}",
            node_config_path().display()
        ))
    })
}

/// 前台运行节点守护进程 (通常由终端或 install 的服务拉起)。
#[derive(Args)]
pub struct DaemonArgs {
    /// 节点 state 目录 (缺省读全局配置, 再缺省取平台默认目录)
    #[arg(long, env = "KUNXTIER_STATE_DIR")]
    pub state_dir: Option<PathBuf>,
    /// WG 固定监听端口 (缺省首次注册时自动选择并随 state 持久化)
    #[arg(long)]
    pub listen_port: Option<u16>,
    /// 日志级别 error/warn/info/debug/trace (覆盖 RUST_LOG 与全局配置)
    #[arg(long)]
    pub log_level: Option<String>,
    /// 日志文件路径 (启用后 stderr / 文件双写, 按大小轮转保留一份 .old)
    #[arg(long)]
    pub log_file: Option<PathBuf>,
}

/// 首次注册并连接; 令牌一次性, 需 daemon 在运行 (FR-2)。
#[derive(Args)]
pub struct UpArgs {
    /// 一次性注册令牌 (已注册节点可省略, 以上次身份直接重连)
    #[arg(long, env = "KUNXTIER_TOKEN")]
    pub token: Option<String>,
    /// 节点名 (网络内唯一)
    #[arg(long)]
    pub name: String,
    /// 核心中继地址 host:port (首次注册必填; 已注册时须与 state 一致)
    #[arg(long, env = "KUNXTIER_CORE")]
    pub core: Option<String>,
    /// 核心证书指纹 hex (首次注册必填)
    #[arg(long, env = "KUNXTIER_FINGERPRINT")]
    pub fingerprint: Option<String>,
    /// 节点 state 目录 (需与 daemon 一致; 缺省读全局配置)
    #[arg(long, env = "KUNXTIER_STATE_DIR")]
    pub state_dir: Option<PathBuf>,
}

/// 断开网络会话 (daemon 留守, 注册身份保留)。
#[derive(Args)]
pub struct DownArgs {
    #[arg(long, env = "KUNXTIER_STATE_DIR")]
    pub state_dir: Option<PathBuf>,
}

/// 本机与对端状态 (路径 direct / relay)。
#[derive(Args)]
pub struct StatusArgs {
    #[arg(long, env = "KUNXTIER_STATE_DIR")]
    pub state_dir: Option<PathBuf>,
    /// 排障视图: 附带最近关键事件 (打洞 / 路径 / 信令 / 注册)
    #[arg(long)]
    pub verbose: bool,
}

impl DaemonArgs {
    pub fn execute(self) -> Result<(), Fail> {
        let global = node_global(self.state_dir.clone())?;
        let options = DaemonOptions {
            state_dir: global.state_dir,
            listen_port: self.listen_port,
            retry_base: RETRY_BASE,
        };
        // expect 理由: 仅在系统线程资源耗尽时失败, 属环境级故障, 无恢复价值
        let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
        rt.block_on(async move {
            let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
            // ctrl-c (前台交互) 与 SIGTERM (launchd / systemd 停服) 同送停机
            kunxtier_node::daemon::spawn_shutdown_signals(shutdown_tx).await;
            serve(TunAdapterFactory, options, shutdown_rx).await
        })
        .map_err(|e| Fail::runtime(format!("daemon 退出: {e}")))
    }
}

impl UpArgs {
    pub fn execute(self) -> Result<(), Fail> {
        let req = IpcRequest::Up {
            token: self.token.unwrap_or_default(),
            name: self.name.clone(),
            core: self.core.clone(),
            fingerprint: self.fingerprint.clone(),
        };
        let state_dir = node_global(self.state_dir)?.state_dir;
        let resp = call_daemon(&state_dir, req)?;
        match resp {
            IpcResponse {
                ok: true,
                ip: Some(ip),
                network: Some(network),
                ..
            } => {
                println!("已上线: 节点 {} 网络 {} IP {}", self.name, network, ip);
                Ok(())
            }
            other => Err(Fail::runtime(
                other.error.unwrap_or_else(|| "未知错误".into()),
            )),
        }
    }
}

impl DownArgs {
    pub fn execute(self) -> Result<(), Fail> {
        let state_dir = node_global(self.state_dir)?.state_dir;
        let resp = call_daemon(&state_dir, IpcRequest::Down)?;
        match resp {
            IpcResponse { ok: true, .. } => println!("已断开 (daemon 留守, 身份保留)"),
            other => {
                return Err(Fail::runtime(
                    other.error.unwrap_or_else(|| "未知错误".into()),
                ))
            }
        }
        Ok(())
    }
}

impl StatusArgs {
    pub fn execute(self) -> Result<(), Fail> {
        let req = if self.verbose {
            IpcRequest::StatusVerbose
        } else {
            IpcRequest::Status
        };
        let state_dir = node_global(self.state_dir)?.state_dir;
        let resp = call_daemon(&state_dir, req)?;
        match resp {
            IpcResponse {
                ok: true,
                status: Some(status),
                events,
                ..
            } => {
                print_status(&status, events.as_deref());
                Ok(())
            }
            other => Err(Fail::runtime(
                other.error.unwrap_or_else(|| "未知错误".into()),
            )),
        }
    }
}

/// 单请求往返; 连接类错误转成 daemon 未运行指引 (FR-2), 权限错误单独提示。
fn call_daemon(state_dir: &Path, req: IpcRequest) -> Result<IpcResponse, Fail> {
    let path = ipc::socket_path(state_dir);
    let call = ipc::request(&path, &req);
    // expect 理由: 仅在系统线程资源耗尽时失败, 属环境级故障, 无恢复价值
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    match rt.block_on(call) {
        Ok(resp) => Ok(resp),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Err(Fail::usage(format!(
                "daemon 未运行 ({}): 请先执行 kunxtier daemon (本地开发) 或 sudo kunxtier install 注册系统服务",
                state_dir.display()
            )))
        }
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(Fail::usage(format!(
            "无权限访问 daemon IPC ({}): 请确认 state 目录权限或以相应用户执行",
            state_dir.display()
        ))),
        Err(e) => Err(Fail::runtime(format!("IPC 请求失败: {e}"))),
    }
}

fn print_status(status: &IpcStatus, events: Option<&[EventRecord]>) {
    match (&status.name, &status.network) {
        (Some(name), Some(network)) => {
            let state = if status.online { "在线" } else { "离线" };
            println!("节点: {name} 网络 {network} [{}]", state);
        }
        _ => println!("未注册 (先 kunxtier up --token <令牌> --name <节点名>)"),
    }
    if !status.online {
        if let Some(reason) = &status.offline_reason {
            println!("离线原因: {reason}");
        }
    }
    if let Some(ip) = &status.own_ip {
        println!("本机 IP: {ip}");
    }
    if let Some(policy) = &status.policy {
        println!("路径策略: {policy}");
    }
    if let Some(fatal) = &status.fatal {
        println!("致命问题: {fatal}");
    }
    if !status.peers.is_empty() {
        println!("对端:");
        println!("名称\tIP\t路径\t端点");
        for peer in &status.peers {
            let endpoint = peer.endpoint.as_deref().unwrap_or("-");
            println!("{}\t{}\t{}\t{}", peer.name, peer.ip, peer.path, endpoint);
        }
    }
    if let Some(events) = events.filter(|e| !e.is_empty()) {
        println!("最近事件 (旧 → 新):");
        for ev in events {
            match &ev.peer {
                Some(peer) => println!("[{}] {}\t{}\t{}", ev.ts, ev.kind, peer, ev.detail),
                None => println!("[{}] {}\t-\t{}", ev.ts, ev.kind, ev.detail),
            }
        }
    }
}
