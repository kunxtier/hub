//! Description: 节点运行时端到端验证: STUN 探测、打洞直连、强制中继、超时回落与路径切换恢复。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 核心中继以进程内真实 TLS 服务形态起停; STUN 用本地伪服务器 (回
//! XOR-MAPPED-ADDRESS), 走节点真实 stun 代码路径; IP 层用 gotatun 通道免特权。
//! 覆盖 FR-2 (候选交换 + 打洞直连)、FR-3 (超时回落且中继路径互通)、FR-10
//! (强制中继 / 恢复自动)、FR-4 (中继只见密文帧) 与 FR-6 (直连失活回落中继 /
//! 周期重试切回直连, 重试期间流量不断)。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gotatun::packet::{Packet, PacketBufPool};
use gotatun::tun::channel::new_udp_tun_channel;
use gotatun::tun::MtuWatcher;
use gotatun::udp::{UdpRecv, UdpSend, UdpTransportFactory, UdpTransportFactoryParams};
use serde_json::{json, Value};
use tokio::net::UdpSocket;

use kunxtier_core::config::RelayConfig;
use kunxtier_core::protocol::SignalRelay;
use kunxtier_node::identity::WgIdentity;
use kunxtier_node::runtime::{start, NodeHandle, NodeOptions, NodeStatus, PathTimings};
use kunxtier_node::stun::MAGIC_COOKIE;
use kunxtier_node::DEFAULT_MTU;

struct Ctx {
    addr: SocketAddr,
    base: String,
    admin: String,
    fp: String,
    agent: ureq::Agent,
}

/// 起核心中继 (进程内 TLS 服务), STUN 列表指向指定伪服务器。
async fn start_core_with_stun(dir: &Path, stun: SocketAddr) -> Ctx {
    let cfg = RelayConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.to_path_buf(),
        advertise: None,
        stun_servers: Vec::new(),
    };
    let out = kunxtier_relay::init(&cfg).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(kunxtier_relay::server::AppState::new(
        kunxtier_relay::store::Store::open(&cfg.db_path()).unwrap(),
        SignalRelay {
            addr: addr.to_string(),
            kind: "core".into(),
            fingerprint: out.fingerprint.clone(),
        },
        vec![stun.to_string()],
    ));
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cfg.cert_path(), cfg.key_path())
        .await
        .unwrap();
    let app = kunxtier_relay::server::build_router(state)
        .into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move {
        let _ = axum_server::from_tcp_rustls(listener, tls).serve(app).await;
    });
    Ctx {
        addr,
        base: format!("https://{addr}"),
        admin: out.admin_credential,
        agent: kunxtier_core::tls::pinning_agent(&out.fingerprint),
        fp: out.fingerprint,
    }
}

/// 伪 STUN 服务器: honest 为真时回观测源地址, 否则回黑洞地址 (打洞必然失败)。
async fn spawn_fake_stun(honest: bool) -> SocketAddr {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let addr = sock.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 1500];
        loop {
            let Ok((n, src)) = sock.recv_from(&mut buf).await else {
                return;
            };
            if n < 20 || buf[..2] != [0x00, 0x01] {
                continue;
            }
            // 黑洞地址: TEST-NET-1 不可路由, 探测永远无响应
            let mapped: SocketAddr = if honest {
                src
            } else {
                "192.0.2.1:9999".parse().unwrap()
            };
            let IpAddr::V4(ip) = mapped.ip() else {
                continue;
            };
            let mut resp = Vec::with_capacity(32);
            resp.extend_from_slice(&[0x01, 0x01, 0x00, 0x0c]); // Binding 成功 + 12 字节属性
            resp.extend_from_slice(&MAGIC_COOKIE);
            resp.extend_from_slice(&buf[8..20]); // 事务 ID 原样回带
                                                 // XOR-MAPPED-ADDRESS: [族][端口^cookie][地址^cookie] + 对齐补位
            resp.extend_from_slice(&[0x00, 0x20, 0x00, 0x08, 0x01]);
            let port = mapped.port() ^ u16::from_be_bytes([MAGIC_COOKIE[0], MAGIC_COOKIE[1]]);
            resp.extend_from_slice(&port.to_be_bytes());
            let o = ip.octets();
            for i in 0..4 {
                resp.push(o[i] ^ MAGIC_COOKIE[i]);
            }
            resp.push(0);
            let _ = sock.send_to(&resp, src).await;
        }
    });
    addr
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

/// 注册节点: 以真实 WG 公钥登记, 返回 (凭证明文, 分配 IP)。
fn register_node(ctx: &Ctx, name: &str, identity: &WgIdentity) -> (String, String) {
    let auth = format!("Bearer {}", ctx.admin);
    let _ = post(
        &ctx.agent,
        &format!("{}/api/admin/users", ctx.base),
        Some(&auth),
        json!({"network": "default", "name": "alice"}),
    );
    let (_, tok) = post(
        &ctx.agent,
        &format!("{}/api/admin/tokens", ctx.base),
        Some(&auth),
        json!({"network": "default", "user": "alice"}),
    );
    let (s, reg) = post(
        &ctx.agent,
        &format!("{}/api/node/register", ctx.base),
        None,
        json!({
            "token": tok["token"].as_str().unwrap(),
            "name": name,
            "wg_public_key": identity.public_key_base64(),
        }),
    );
    assert_eq!(s, 200, "节点注册必须成功");
    (
        reg["node_credential"].as_str().unwrap().to_string(),
        reg["ip"].as_str().unwrap().to_string(),
    )
}

fn set_policy(ctx: &Ctx, name: &str, policy: &str) {
    let auth = format!("Bearer {}", ctx.admin);
    let (s, v) = post(
        &ctx.agent,
        &format!("{}/api/admin/nodes/policy", ctx.base),
        Some(&auth),
        json!({"network": "default", "name": name, "policy": policy}),
    );
    assert_eq!(s, 200, "策略设置必须成功: {v}");
}

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

/// 探测一个空闲 UDP 端口 (探测与绑定之间存在竞态, 环回测试可接受)。
fn free_udp_port() -> u16 {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").expect("环回 UDP 绑定失败");
    sock.local_addr().expect("读取端口失败").port()
}

struct Node {
    handle: NodeHandle,
    /// 注入: 模拟主机发包进 TUN。
    inject: <gotatun::udp::channel::UdpChannelFactory as UdpTransportFactory>::SendV4,
    /// 观察: 收取引擎写回 TUN 的明文。
    observe: <gotatun::udp::channel::UdpChannelFactory as UdpTransportFactory>::RecvV4,
}

/// 以通道 IP 层启动一个节点运行时 (路径节奏可配, 缺省生产值)。
async fn start_node(ctx: &Ctx, name: &str, identity: &WgIdentity, timings: PathTimings) -> Node {
    let (credential, ip) = register_node(ctx, name, identity);
    let (tun_tx, tun_rx, mut factory) = new_udp_tun_channel(
        64,
        ip.parse().unwrap(),
        Ipv6Addr::UNSPECIFIED,
        MtuWatcher::new(DEFAULT_MTU),
    );
    let ((inject, observe), _) = factory.bind(&bind_params()).await.unwrap();
    let handle = start(
        NodeOptions {
            core: ctx.addr.to_string(),
            fingerprint: ctx.fp.clone(),
            network: "default".into(),
            name: name.into(),
            credential,
            identity: identity.clone(),
            listen_port: free_udp_port(),
            keepalive_secs: 25,
            timings,
            events: Arc::default(),
        },
        tun_tx,
        tun_rx,
    )
    .await
    .expect("节点运行时启动失败");
    Node {
        handle,
        inject,
        observe,
    }
}

/// 等待唯一 peer 进入指定路径状态 (打洞 / 回落 / 策略生效均有异步时延)。
async fn wait_path(node: &Node, want: &str, timeout: Duration) -> NodeStatus {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let status = node.handle.status();
        if status.peers.len() == 1 && status.peers[0].path == want {
            return status;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("等待路径状态 {want} 超时, 当前: {status:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 注入一笔载荷并断言对端观察到 (源虚拟 IP 校验)。
/// 等待核心 admin 视图呈现指定节点的对端路径 (T11 路径上报经信令异步到达)。
async fn wait_admin_path(
    ctx: &Ctx,
    node: &str,
    peer: &str,
    want: &str,
    want_relay: Option<&str>,
    timeout: Duration,
) {
    let auth = format!("Bearer {}", ctx.admin);
    let url = format!("{}/api/admin/nodes", ctx.base);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let found = ctx
            .agent
            .get(&url)
            .set("Authorization", &auth)
            .call()
            .ok()
            .and_then(|r| r.into_json::<serde_json::Value>().ok())
            .and_then(|nodes| nodes.as_array().cloned())
            .and_then(|nodes| nodes.into_iter().find(|n| n["name"] == node));
        if let Some(n) = found {
            if let Some(paths) = n["paths"].as_array() {
                if let Some(p) = paths
                    .iter()
                    .find(|p| p["peer"] == peer && p["path"] == want)
                {
                    match want_relay {
                        Some(relay) if p["relay"] != relay => {}
                        _ => return,
                    }
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "等待 admin 路径视图 {node}->{peer}={want} (relay={want_relay:?}) 超时"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn exchange(a: &Node, b: &mut Node, tag: &[u8]) {
    let dst: Ipv4Addr = b.handle.status().own_ip.expect("B 应已就绪");
    a.inject
        .send_to(
            Packet::copy_from(tag),
            SocketAddr::new(IpAddr::V4(dst), 9999),
        )
        .await
        .expect("A 注入失败");
    let mut pool = PacketBufPool::new(16);
    let (received, from) =
        tokio::time::timeout(Duration::from_secs(10), b.observe.recv_from(&mut pool))
            .await
            .expect("载荷未在窗口内送达 (路径未打通)")
            .expect("B 侧接收失败");
    assert_eq!(&received[..], tag, "载荷不一致");
    let src: Ipv4Addr = a.handle.status().own_ip.expect("A 应已就绪");
    assert_eq!(from.ip(), IpAddr::V4(src), "源虚拟 IP 不一致");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn punch_direct_force_relay_and_recover() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "off".into()),
        )
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let stun = spawn_fake_stun(true).await;
    let ctx = start_core_with_stun(dir.path(), stun).await;

    let identity_a = WgIdentity::generate();
    let identity_b = WgIdentity::generate();
    let mut node_a = start_node(&ctx, "na", &identity_a, PathTimings::default()).await;
    let mut node_b = start_node(&ctx, "nb", &identity_b, PathTimings::default()).await;

    // ---- FR-2: STUN 候选交换 + 并发打洞 → 直连 ----
    // 打洞期 keepalive 收紧驱动握手, "观测到 WG 流量" 即确认 direct
    let status = wait_path(&node_a, "direct", Duration::from_secs(15)).await;
    assert_eq!(status.peers[0].name, "nb");
    let endpoint = status.peers[0].endpoint.expect("direct 应有端点");
    assert!(
        endpoint.ip().to_string().starts_with("127.0.0."),
        "直连端点应为对端真实 UDP 地址: {endpoint}"
    );
    wait_path(&node_b, "direct", Duration::from_secs(15)).await;

    // T11: 路径上报到达核心, admin 总览可见直连 (FR-1)
    wait_admin_path(&ctx, "na", "nb", "direct", None, Duration::from_secs(15)).await;
    wait_admin_path(&ctx, "nb", "na", "direct", None, Duration::from_secs(15)).await;

    exchange(&node_a, &mut node_b, b"punch-direct-a-to-b").await;
    exchange(&node_b, &mut node_a, b"punch-direct-b-to-a").await;

    // ---- FR-10: 强制中继 (admin 设置 → Policy 推送 → 伪端点) ----
    set_policy(&ctx, "na", "force_relay");
    let status = wait_path(&node_a, "relayed", Duration::from_secs(10)).await;
    let pseudo = status.peers[0].endpoint.expect("relayed 应有伪端点");
    assert!(
        pseudo.ip().to_string().starts_with("240."),
        "强制中继端点应为 240/4 伪端点: {pseudo}"
    );
    // 中继路径经核心内置转发, 流量仍互通 (FR-4: 帧对中继不透明)
    exchange(&node_a, &mut node_b, b"force-relay-a-to-b").await;

    // ---- FR-10: 恢复自动 → 清端点重打洞 → 回直连 ----
    set_policy(&ctx, "na", "auto");
    wait_path(&node_a, "direct", Duration::from_secs(15)).await;
    exchange(&node_a, &mut node_b, b"recover-direct-a-to-b").await;

    // ---- 对端离线: 端点清理, 路径回 idle ----
    node_b.handle.stop();
    wait_path(&node_a, "idle", Duration::from_secs(10)).await;
    node_a.handle.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unreachable_candidates_fall_back_to_relay() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "off".into()),
        )
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    // 说谎 STUN: 候选指向黑洞, 双侧打洞必然失败 (模拟对称 NAT 等打不通拓扑)
    let stun = spawn_fake_stun(false).await;
    let ctx = start_core_with_stun(dir.path(), stun).await;
    // 收紧节奏驱动重试振荡: 窗口 1.5s, 回落 600ms 后即重试
    let timings = PathTimings {
        punch_window: Duration::from_millis(1500),
        retry: Duration::from_millis(600),
        ..PathTimings::default()
    };

    let identity_a = WgIdentity::generate();
    let identity_b = WgIdentity::generate();
    let mut node_a = start_node(&ctx, "na", &identity_a, timings.clone()).await;
    let mut node_b = start_node(&ctx, "nb", &identity_b, timings).await;

    // ---- FR-3: 打洞窗口到期 → 回落核心内置中继 ----
    let status = wait_path(&node_a, "relayed", Duration::from_secs(20)).await;
    let pseudo = status.peers[0].endpoint.expect("回落应有伪端点");
    assert!(
        pseudo.ip().to_string().starts_with("240."),
        "回落端点应为伪端点: {pseudo}"
    );
    wait_path(&node_b, "relayed", Duration::from_secs(20)).await;

    // T11: 回落路径上报核心, admin 可见中继归属 (内置条目即核心地址)
    let core_addr = ctx.addr.to_string();
    wait_admin_path(
        &ctx,
        "na",
        "nb",
        "relayed",
        Some(&core_addr),
        Duration::from_secs(15),
    )
    .await;
    wait_admin_path(
        &ctx,
        "nb",
        "na",
        "relayed",
        Some(&core_addr),
        Duration::from_secs(15),
    )
    .await;

    // 中继路径互通: 上行帧 [目的][源][密文] 经核心内置转发投递
    exchange(&node_a, &mut node_b, b"fallback-relay-a-to-b").await;
    exchange(&node_b, &mut node_a, b"fallback-relay-b-to-a").await;

    // ---- FR-6: 回落后周期重试直连; 重试打洞期间以中继为 fallback, 流量不断 ----
    wait_path(&node_a, "punching", Duration::from_secs(10)).await;
    exchange(&node_a, &mut node_b, b"retry-punching-a-to-b").await;
    // 重试窗口到期仍未通 → 回到中继, 流量继续
    wait_path(&node_a, "relayed", Duration::from_secs(10)).await;
    exchange(&node_a, &mut node_b, b"retry-expired-a-to-b").await;

    node_a.handle.stop();
    node_b.handle.stop();
}

/// FR-6 全链: 直连静默失活 → 回落中继 (业务不断) → 周期重试 → 切回直连。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_silence_falls_back_and_retry_recovers() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "off".into()),
        )
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let stun = spawn_fake_stun(true).await;
    let ctx = start_core_with_stun(dir.path(), stun).await;
    // 关闭周期保活 Ping (1h), 失活阈值 800ms, 回落停留 5s 后重试——
    // 双侧无探测无业务即直连失活, 模拟路径中断 (FR-6 的 "直连中断")
    let timings = PathTimings {
        ping: Duration::from_secs(3600),
        dead_after: Duration::from_millis(800),
        retry: Duration::from_secs(5),
        ..PathTimings::default()
    };

    let identity_a = WgIdentity::generate();
    let identity_b = WgIdentity::generate();
    let mut node_a = start_node(&ctx, "na", &identity_a, timings.clone()).await;
    let mut node_b = start_node(&ctx, "nb", &identity_b, timings).await;

    // ---- FR-2: 双侧直连 ----
    wait_path(&node_a, "direct", Duration::from_secs(15)).await;
    wait_path(&node_b, "direct", Duration::from_secs(15)).await;
    exchange(&node_a, &mut node_b, b"silence-a-to-b").await;
    exchange(&node_b, &mut node_a, b"silence-b-to-a").await;

    // ---- FR-6: 静默失活 → 回落中继, 端点为伪端点 ----
    let status = wait_path(&node_a, "relayed", Duration::from_secs(10)).await;
    let pseudo = status.peers[0].endpoint.expect("回落应有伪端点");
    assert!(
        pseudo.ip().to_string().starts_with("240."),
        "回落端点应为伪端点: {pseudo}"
    );
    // 回落期间业务走中继不断
    exchange(&node_a, &mut node_b, b"dead-fallback-a-to-b").await;

    // ---- FR-6: 周期重试打洞 → 切回直连 ----
    wait_path(&node_a, "direct", Duration::from_secs(20)).await;
    exchange(&node_a, &mut node_b, b"recovered-a-to-b").await;
    wait_path(&node_b, "direct", Duration::from_secs(20)).await;
    exchange(&node_b, &mut node_a, b"recovered-b-to-a").await;

    node_a.handle.stop();
    node_b.handle.stop();
}
