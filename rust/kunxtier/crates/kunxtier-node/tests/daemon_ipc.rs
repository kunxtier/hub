//! Description: daemon 会话生命周期与本地 IPC 端到端验证 (T09)。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 核心中继以进程内真实 TLS 服务形态起停 (可整体停机并在原端口重启,
//! 驱动 daemon 会话监督); IP 层用 gotatun 通道免特权注入。覆盖 FR-1
//! (daemon / up / down / status 命令面)、FR-2 (本地 IPC)、FR-9 前提
//! (daemon 重启凭 state 自动恢复上线)、FR-17 (同名重注册换发凭证)
//! 与意外退出后的退避重连; T14 增补启动期 state 故障的致命封停。

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gotatun::tun::channel::{new_udp_tun_channel, TunChannelRx, TunChannelTx};
use gotatun::tun::MtuWatcher;
use gotatun::udp::{UdpTransportFactory, UdpTransportFactoryParams};
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use kunxtier_core::config::RelayConfig;
use kunxtier_core::protocol::SignalRelay;
use kunxtier_node::daemon::{serve, DaemonOptions, TunFactory};
use kunxtier_node::events::EventRecord;
use kunxtier_node::ipc::{self, IpcRequest, IpcResponse, IpcStatus};
use kunxtier_node::state::{NodeState, STATE_FILE};
use kunxtier_node::{EventKind, Result, DEFAULT_MTU};

/// 核心中继实例: 服务跑在独立子 runtime 上——升级后的 WS 连接不归
/// axum-server Handle 管, 整体失联只能以 runtime 任务组丢弃模拟 (崩溃)。
struct Core {
    addr: SocketAddr,
    base: String,
    admin: String,
    agent: ureq::Agent,
    fingerprint: String,
    rt: Option<tokio::runtime::Runtime>,
}

impl Core {
    /// 崩溃式失联: 丢弃任务组, 在途连接随 fd 关闭而断。
    fn kill(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.kill();
    }
}

/// 起核心中继 (进程内 TLS 服务, 无 STUN); port 为 Some 时在指定端口
/// 复活 (bind 竞态重试), 供核心重启场景。
async fn start_core(dir: &Path, port: Option<u16>) -> Core {
    let cfg = RelayConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.to_path_buf(),
        advertise: None,
        stun_servers: Vec::new(),
    };
    // 首启走 init; 原端口复活 (重启场景) 沿用既有证书与库, 指纹从证书回读
    let (admin, fingerprint) = if port.is_some() {
        let pem = std::fs::read_to_string(cfg.cert_path()).unwrap();
        let der = kunxtier_relay::cert::der_from_pem(&pem).unwrap();
        (String::new(), kunxtier_relay::cert::fingerprint(&der))
    } else {
        let out = kunxtier_relay::init(&cfg).unwrap();
        (out.admin_credential, out.fingerprint)
    };
    let listener = match port {
        None => std::net::TcpListener::bind("127.0.0.1:0").unwrap(),
        Some(port) => {
            // 旧实例的 socket 关闭与端口释放存在窗口, 重试到绑上为止
            loop {
                match std::net::TcpListener::bind(("127.0.0.1", port)) {
                    Ok(l) => break l,
                    Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            }
        }
    };
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(kunxtier_relay::server::AppState::new(
        kunxtier_relay::store::Store::open(&cfg.db_path()).unwrap(),
        SignalRelay {
            addr: addr.to_string(),
            kind: "core".into(),
            fingerprint: fingerprint.clone(),
        },
        Vec::new(),
    ));
    // 核心子 runtime: tls 构造经 oneshot 取回 (跨 runtime 不能 block_on)
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (tls_tx, tls_rx) = tokio::sync::oneshot::channel();
    let (cert, key) = (cfg.cert_path(), cfg.key_path());
    rt.spawn(async move {
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
            .await
            .unwrap();
        let _ = tls_tx.send(tls);
    });
    let tls = tls_rx.await.expect("tls 配置构造失败");
    let app = kunxtier_relay::server::build_router(state)
        .into_make_service_with_connect_info::<SocketAddr>();
    rt.spawn(async move {
        let _ = axum_server::from_tcp_rustls(listener, tls).serve(app).await;
    });
    Core {
        addr,
        base: format!("https://{addr}"),
        admin,
        agent: kunxtier_core::tls::pinning_agent(&fingerprint),
        fingerprint,
        rt: Some(rt),
    }
}

fn post(agent: &ureq::Agent, url: &str, auth: Option<&str>, body: Value) -> (u16, Value) {
    let mut req = agent.post(url);
    if let Some(a) = auth {
        req = req.set("Authorization", a);
    }
    match req.send_json(body) {
        Ok(r) => {
            let s = r.status();
            (s, r.into_json().unwrap_or(Value::Null))
        }
        Err(ureq::Error::Status(s, r)) => (s, r.into_json().unwrap_or(Value::Null)),
        Err(e) => panic!("传输错误: {e}"),
    }
}

/// 签发一枚节点一次性注册令牌 (经真实 admin API, 复用默认网络与用户)。
fn issue_node_token(core: &Core) -> String {
    let auth = format!("Bearer {}", core.admin);
    let _ = post(
        &core.agent,
        &format!("{}/api/admin/users", core.base),
        Some(&auth),
        json!({"network": "default", "name": "alice"}),
    );
    let (_, token) = post(
        &core.agent,
        &format!("{}/api/admin/tokens", core.base),
        Some(&auth),
        json!({"network": "default", "user": "alice"}),
    );
    token["token"].as_str().expect("令牌签发失败").to_string()
}

fn remove_node(core: &Core, name: &str) {
    let auth = format!("Bearer {}", core.admin);
    let (s, v) = post(
        &core.agent,
        &format!("{}/api/admin/nodes/remove", core.base),
        Some(&auth),
        json!({"network": "default", "name": name}),
    );
    assert_eq!(s, 200, "节点移除必须成功: {v}");
}

/// gotatun 通道 IP 层 (免特权), daemon 的可注入 TunFactory。
/// 通道的 UDP 半边绑定后由 Keep 持有, 防止提前 drop 关闭通道对。
struct ChannelTunFactory;

/// 通道 UDP 半边句柄 (随会话生命周期)。
struct ChannelKeep {
    _inject: ChanV4Send,
    _observe: ChanV4Recv,
}

type ChanV4Send = <gotatun::udp::channel::UdpChannelFactory as UdpTransportFactory>::SendV4;
type ChanV4Recv = <gotatun::udp::channel::UdpChannelFactory as UdpTransportFactory>::RecvV4;

/// 通道工厂 bind 参数 (通道实现不实际监听, 仅取端口语义)。
fn bind_params() -> UdpTransportFactoryParams {
    #[cfg(not(target_os = "linux"))]
    return UdpTransportFactoryParams {
        addr_v4: Ipv4Addr::LOCALHOST,
        addr_v6: Ipv6Addr::UNSPECIFIED,
        port: 0,
    };
    #[cfg(target_os = "linux")]
    return UdpTransportFactoryParams {
        addr_v4: Ipv4Addr::LOCALHOST,
        addr_v6: Ipv6Addr::UNSPECIFIED,
        port: 0,
        fwmark: None,
    };
}

impl TunFactory for ChannelTunFactory {
    type Tx = TunChannelTx;
    type Rx = TunChannelRx;
    type Keep = ChannelKeep;

    async fn open(&self, vip: Ipv4Addr) -> Result<(Self::Tx, Self::Rx, Self::Keep)> {
        let (tx, rx, mut factory) =
            new_udp_tun_channel(64, vip, Ipv6Addr::UNSPECIFIED, MtuWatcher::new(DEFAULT_MTU));
        let ((inject, observe), _) = factory.bind(&bind_params()).await.unwrap();
        Ok((
            tx,
            rx,
            ChannelKeep {
                _inject: inject,
                _observe: observe,
            },
        ))
    }
}

struct DaemonProc {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// 起 daemon 并等 IPC socket 就绪; 重连退避起步 200ms 驱动监督循环。
async fn start_daemon(dir: &Path) -> DaemonProc {
    let (shutdown, shutdown_rx) = watch::channel(false);
    let options = DaemonOptions {
        state_dir: dir.to_path_buf(),
        listen_port: None,
        retry_base: Duration::from_millis(200),
    };
    let task = tokio::spawn(async move {
        if let Err(e) = serve(ChannelTunFactory, options, shutdown_rx).await {
            eprintln!("daemon 异常退出: {e}");
        }
    });
    let path = ipc::socket_path(dir);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "daemon IPC socket 未就绪"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    DaemonProc { shutdown, task }
}

async fn stop_daemon(proc: DaemonProc) {
    let _ = proc.shutdown.send(true);
    proc.task.await.expect("daemon 任务异常");
}

async fn up(
    dir: &Path,
    token: Option<&str>,
    name: &str,
    core: Option<&str>,
    fingerprint: Option<&str>,
) -> IpcResponse {
    ipc::request(
        &ipc::socket_path(dir),
        &IpcRequest::Up {
            token: token.unwrap_or("").to_string(),
            name: name.to_string(),
            core: core.map(str::to_string),
            fingerprint: fingerprint.map(str::to_string),
        },
    )
    .await
    .expect("IPC 请求失败")
}

async fn down(dir: &Path) -> IpcResponse {
    ipc::request(&ipc::socket_path(dir), &IpcRequest::Down)
        .await
        .expect("IPC 请求失败")
}

async fn status(dir: &Path) -> IpcStatus {
    let resp = ipc::request(&ipc::socket_path(dir), &IpcRequest::Status)
        .await
        .expect("IPC 请求失败");
    resp.status.expect("status 应携带快照")
}

/// status --verbose 的关键事件留痕 (旧 → 新)。
async fn status_verbose(dir: &Path) -> Vec<EventRecord> {
    let resp = ipc::request(&ipc::socket_path(dir), &IpcRequest::StatusVerbose)
        .await
        .expect("IPC 请求失败");
    resp.events.expect("verbose 应携带事件")
}

/// 等待 online 达到期望态。
async fn wait_online(dir: &Path, want: bool, timeout: Duration) -> IpcStatus {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let st = status(dir).await;
        if st.online == want {
            return st;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "等待 online={want} 超时, 当前: {st:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 等 status 对端表出现指定节点。
async fn wait_peer(dir: &Path, peer: &str, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let st = status(dir).await;
        if st.peers.iter().any(|p| p.name == peer) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "等待对端 {peer} 出现超时, 当前: {st:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 主链路: 注册上线 → state 落盘 → down → 无令牌重连 → 同名重注册 →
/// daemon 重启自动恢复 (FR-1 / FR-2 / FR-9 前提 / FR-17)。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn up_down_re_register_and_restart_recover() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "off".into()),
        )
        .try_init();
    let core_dir = tempfile::tempdir().unwrap();
    let core = start_core(core_dir.path(), None).await;
    let state_dir = tempfile::tempdir().unwrap();
    let dir = state_dir.path();
    let daemon = start_daemon(dir).await;

    // 未注册: status 离线且无归属
    let st = status(dir).await;
    assert!(!st.online);
    assert!(st.name.is_none() && st.network.is_none());

    // 首次 up 缺令牌 → 明确报错
    let resp = up(dir, None, "laptop", Some(&core.addr.to_string()), None).await;
    assert!(!resp.ok, "缺令牌的首次 up 必须被拒绝");
    assert!(resp.error.unwrap().contains("令牌"), "应提示令牌缺失");

    // 首次注册并上线
    let resp = up(
        dir,
        Some(&issue_node_token(&core)),
        "laptop",
        Some(&core.addr.to_string()),
        Some(&core.fingerprint),
    )
    .await;
    assert!(resp.ok, "up 失败: {:?}", resp.error);
    let ip = resp.ip.clone().unwrap();
    assert_eq!(resp.network.as_deref(), Some("default"));

    // state 落盘: 身份可恢复、字段完整、权限收紧
    let state = NodeState::load(dir).unwrap().expect("state 应已落盘");
    assert_eq!(state.name, "laptop");
    assert_eq!(state.vip.to_string(), ip);
    assert_eq!(state.core, core.addr.to_string());
    assert!(state.identity().is_ok(), "WG 身份必须可恢复");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join(STATE_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "state 文件必须 0600");
    }

    // 在线态: 归属与 vip 可见
    let st = wait_online(dir, true, Duration::from_secs(10)).await;
    assert_eq!(st.name.as_deref(), Some("laptop"));
    assert_eq!(st.own_ip.as_deref(), Some(ip.as_str()));

    // verbose 排障视图: 注册与信令事件已留痕 (NFR FR-3)
    let events = status_verbose(dir).await;
    assert!(
        events
            .iter()
            .any(|e| e.kind == EventKind::Register && e.detail.contains("注册成功")),
        "verbose 事件应含注册成功: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e.kind == EventKind::Signal && e.detail.contains("信令通道建立")),
        "verbose 事件应含信令建立: {events:?}"
    );

    // 在线重复 up → 拒绝
    let resp = up(dir, Some(&issue_node_token(&core)), "laptop", None, None).await;
    assert!(!resp.ok && resp.error.unwrap().contains("已在线"));

    // down → 离线但归属保留
    assert!(down(dir).await.ok);
    let st = wait_online(dir, false, Duration::from_secs(10)).await;
    assert_eq!(st.name.as_deref(), Some("laptop"));
    assert!(st.offline_reason.is_some());

    // FR-17: 服务端移除节点后同名重注册 (沿用身份, 换发凭证)
    remove_node(&core, "laptop");
    let resp = up(dir, Some(&issue_node_token(&core)), "laptop", None, None).await;
    assert!(resp.ok, "同名重注册失败: {:?}", resp.error);
    wait_online(dir, true, Duration::from_secs(10)).await;

    // daemon 重启 → 凭 state 自动恢复上线 (服务重启恢复的前提)
    assert!(down(dir).await.ok);
    wait_online(dir, false, Duration::from_secs(10)).await;
    stop_daemon(daemon).await;
    let daemon = start_daemon(dir).await;
    wait_online(dir, true, Duration::from_secs(10)).await;
    stop_daemon(daemon).await;
}

/// 双节点对端可见; 核心整体停机后会话退出并在核心复活后退避重连。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peers_visible_and_reconnect_after_core_restart() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "off".into()),
        )
        .try_init();
    let core_dir = tempfile::tempdir().unwrap();
    let mut core = start_core(core_dir.path(), None).await;
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let daemon_a = start_daemon(dir_a.path()).await;
    let daemon_b = start_daemon(dir_b.path()).await;

    let resp = up(
        dir_a.path(),
        Some(&issue_node_token(&core)),
        "na",
        Some(&core.addr.to_string()),
        Some(&core.fingerprint),
    )
    .await;
    assert!(resp.ok, "na 上线失败: {:?}", resp.error);
    let resp = up(
        dir_b.path(),
        Some(&issue_node_token(&core)),
        "nb",
        Some(&core.addr.to_string()),
        Some(&core.fingerprint),
    )
    .await;
    assert!(resp.ok, "nb 上线失败: {:?}", resp.error);

    wait_online(dir_a.path(), true, Duration::from_secs(10)).await;
    wait_online(dir_b.path(), true, Duration::from_secs(10)).await;
    // 对端表经信令 State / PeerOnline 广播到达 (无 STUN, 路径为 idle)
    wait_peer(dir_a.path(), "nb", Duration::from_secs(10)).await;
    wait_peer(dir_b.path(), "na", Duration::from_secs(10)).await;

    // 核心崩溃 (任务组整体丢弃, WS 连接随 fd 关闭而断) → 会话退出转离线
    let port = core.addr.port();
    core.kill();
    let st = wait_online(dir_a.path(), false, Duration::from_secs(20)).await;
    assert!(st.offline_reason.is_some(), "应携带离线原因");

    // 核心原端口复活 → daemon 退避重连自动上线
    let _core2 = start_core(core_dir.path(), Some(port)).await;
    wait_online(dir_a.path(), true, Duration::from_secs(30)).await;
    wait_online(dir_b.path(), true, Duration::from_secs(30)).await;

    stop_daemon(daemon_a).await;
    stop_daemon(daemon_b).await;
}

/// 启动期 state 持久故障 (版本不兼容): daemon 以致命态封停留守而非退出
/// (T14 服务化: KeepAlive 托管下退出会形成节流拉起循环), IPC 可用、
/// status 可见致命原因、up 被拒绝并指引处理。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupted_state_daemon_stays_fatal_alive() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "off".into()),
        )
        .try_init();
    let state_dir = tempfile::tempdir().unwrap();
    let dir = state_dir.path();
    // 高版本 state: 拒读属持久故障, 修复前 daemon 直接退出进程
    kunxtier_node::state::ensure_state_dir(dir).unwrap();
    let mut st = NodeState::from_identity(&kunxtier_node::WgIdentity::generate());
    st.version = kunxtier_node::STATE_VERSION + 1;
    std::fs::write(dir.join(STATE_FILE), serde_json::to_vec(&st).unwrap()).unwrap();

    // socket 就绪即证明 daemon 未退出 (封停留守)
    let daemon = start_daemon(dir).await;
    let st = status(dir).await;
    assert!(!st.online);
    let fatal = st.fatal.expect("应携带致命问题");
    assert!(fatal.contains("state"), "致命原因应指向 state: {fatal}");

    // up 被致命态拒绝并指引处理
    let resp = up(dir, Some("token"), "laptop", None, None).await;
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("致命"), "应提示致命态");

    stop_daemon(daemon).await;
}
