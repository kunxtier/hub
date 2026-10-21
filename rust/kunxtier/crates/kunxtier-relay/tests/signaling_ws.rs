//! Description: 信令 WS 集成测试: 挑战应答、节点表推送、上下线事件、打洞信令转发、凭证吊销、建连限速。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! WS 客户端以指纹固定 TLS + client_async 模拟 daemon 信令面行为。

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use kunxtier_core::config::RelayConfig;
use kunxtier_core::protocol::SignalRelay;
use kunxtier_relay::client_tls::{pinning_agent, PinVerifier};
use kunxtier_relay::server::{build_router, AppState};
use kunxtier_relay::store::Store;

type WsClient =
    tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>;

struct Ctx {
    addr: SocketAddr,
    base: String,
    admin: String,
    fp: String,
    agent: ureq::Agent,
}

/// 内置转发条目 (与 server::run 的装配口径一致: 具体监听地址 + 自身指纹)。
fn builtin_relay(addr: SocketAddr, fingerprint: &str) -> SignalRelay {
    SignalRelay {
        addr: addr.to_string(),
        kind: "core".into(),
        fingerprint: fingerprint.to_string(),
    }
}

async fn start_relay(dir: &Path) -> Ctx {
    let cfg = RelayConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.to_path_buf(),
        advertise: None,
    };
    let out = kunxtier_relay::init(&cfg).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(AppState::new(
        Store::open(&cfg.db_path()).unwrap(),
        builtin_relay(addr, &out.fingerprint),
    ));
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cfg.cert_path(), cfg.key_path())
        .await
        .unwrap();
    let app = build_router(state).into_make_service_with_connect_info::<SocketAddr>();
    // 服务端任务由 worker 驱动, 测试主线程做阻塞 HTTP / 异步 WS 调用
    tokio::spawn(async move {
        let _ = axum_server::from_tcp_rustls(listener, tls).serve(app).await;
    });
    let agent = pinning_agent(&out.fingerprint);
    Ctx {
        addr,
        base: format!("https://{addr}"),
        admin: out.admin_credential,
        fp: out.fingerprint,
        agent,
    }
}

fn wg_key_b64() -> String {
    STANDARD.encode([3u8; 32])
}

/// POST JSON 并返回 (状态码, 响应体)。
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

fn get(agent: &ureq::Agent, url: &str, auth: Option<&str>) -> (u16, Value) {
    let mut req = agent.get(url);
    if let Some(a) = auth {
        req = req.set("Authorization", a);
    }
    match req.call() {
        Ok(r) => {
            let s = r.status();
            (s, r.into_json().unwrap_or(Value::Null))
        }
        Err(ureq::Error::Status(s, r)) => (s, r.into_json().unwrap_or(Value::Null)),
        Err(e) => panic!("传输错误: {e}"),
    }
}

/// 注册节点并返回节点凭证明文 (用户幂等创建)。
fn register_node(ctx: &Ctx, network: &str, user: &str, name: &str) -> String {
    let auth = format!("Bearer {}", ctx.admin);
    let _ = post(
        &ctx.agent,
        &format!("{}/api/admin/users", ctx.base),
        Some(&auth),
        json!({"network": network, "name": user}),
    );
    let (_, tok) = post(
        &ctx.agent,
        &format!("{}/api/admin/tokens", ctx.base),
        Some(&auth),
        json!({"network": network, "user": user}),
    );
    let (s, reg) = post(
        &ctx.agent,
        &format!("{}/api/node/register", ctx.base),
        None,
        json!({
            "token": tok["token"].as_str().unwrap(),
            "name": name,
            "wg_public_key": wg_key_b64()
        }),
    );
    assert_eq!(s, 200, "节点注册必须成功");
    reg["node_credential"].as_str().unwrap().to_string()
}

fn admin_auth(ctx: &Ctx) -> String {
    format!("Bearer {}", ctx.admin)
}

async fn admin_nodes(ctx: &Ctx) -> Value {
    let (s, nodes) = get(
        &ctx.agent,
        &format!("{}/api/admin/nodes", ctx.base),
        Some(&admin_auth(ctx)),
    );
    assert_eq!(s, 200);
    nodes
}

/// TLS (指纹固定) + WS 拨号。
async fn ws_dial(addr: SocketAddr, fingerprint: &str) -> Result<WsClient, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls_cfg = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinVerifier {
            fingerprint: fingerprint.to_string(),
        }))
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_cfg));
    let tcp = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|e| format!("tcp: {e}"))?;
    let name =
        rustls::pki_types::ServerName::try_from("localhost".to_string()).expect("合法 ServerName");
    let tls = connector
        .connect(name, tcp)
        .await
        .map_err(|e| format!("tls: {e}"))?;
    let (ws, _resp) = tokio_tungstenite::client_async("wss://localhost/ws/signal", tls)
        .await
        .map_err(|e| format!("ws: {e}"))?;
    Ok(ws)
}

/// 发送 JSON 文本帧。
async fn send_json(ws: &mut WsClient, v: Value) {
    ws.send(WsMessage::Text(v.to_string().into()))
        .await
        .unwrap();
}

/// 收取下一帧 JSON; 跳过协议层 Ping (顺带回 Pong); 通道关闭 / 协议错误返回 None。
async fn recv_json(ws: &mut WsClient) -> Option<Value> {
    loop {
        match ws.next().await? {
            Ok(WsMessage::Text(t)) => return serde_json::from_str(t.as_str()).ok(),
            Ok(WsMessage::Ping(p)) => {
                ws.send(WsMessage::Pong(p)).await.unwrap();
            }
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

/// 带超时收取消息 (服务端事件必须秒级到达, 5s 已是宽容上限)。
async fn next_msg(ws: &mut WsClient) -> Value {
    tokio::time::timeout(Duration::from_secs(5), recv_json(ws))
        .await
        .expect("等待信令消息超时")
        .expect("连接已关闭")
}

/// 完成挑战应答认证, 返回 (ws, AuthOk 消息)。
async fn auth(
    addr: SocketAddr,
    fp: &str,
    network: &str,
    name: &str,
    credential: &str,
) -> (WsClient, Value) {
    let mut ws = ws_dial(addr, fp).await.unwrap();
    send_json(
        &mut ws,
        json!({"type": "hello", "network": network, "name": name}),
    )
    .await;
    let challenge = next_msg(&mut ws).await;
    assert_eq!(challenge["type"], "challenge", "hello 后必须是挑战");
    let nonce = STANDARD
        .decode(challenge["nonce"].as_str().unwrap())
        .unwrap();
    let digest = STANDARD.encode(kunxtier_core::token::challenge_answer(&nonce, credential));
    send_json(&mut ws, json!({"type": "auth", "digest": digest})).await;
    let ok = next_msg(&mut ws).await;
    assert_eq!(ok["type"], "auth_ok", "凭证应答必须通过: {ok}");
    (ws, ok)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn challenge_response_and_state_push() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = start_relay(dir.path()).await;
    let cred = register_node(&ctx, "default", "alice", "laptop");

    // 应答错误: AuthErr 后服务端关闭连接
    let mut bad = ws_dial(ctx.addr, &ctx.fp).await.unwrap();
    send_json(
        &mut bad,
        json!({"type": "hello", "network": "default", "name": "laptop"}),
    )
    .await;
    let _ = next_msg(&mut bad).await; // challenge
    send_json(
        &mut bad,
        json!({"type": "auth", "digest": STANDARD.encode([9u8; 32])}),
    )
    .await;
    let err = next_msg(&mut bad).await;
    assert_eq!(err["type"], "auth_err");
    let closed = tokio::time::timeout(Duration::from_secs(5), recv_json(&mut bad))
        .await
        .unwrap();
    assert!(closed.is_none(), "认证失败后连接必须被关闭");

    // 应答正确: AuthOk 携带 IP 与心跳参数 (裁决值 10s / 30s)
    let (mut ws, ok) = auth(ctx.addr, &ctx.fp, "default", "laptop", &cred).await;
    assert_eq!(ok["ip"], "100.64.0.1");
    assert_eq!(ok["heartbeat_interval_secs"], 10);
    assert_eq!(ok["idle_timeout_secs"], 30);

    // 全量节点表: 自身在线、公钥、策略与中继列表 (无卫星时仅内置条目)
    let state = next_msg(&mut ws).await;
    assert_eq!(state["type"], "state");
    assert_eq!(state["policy"], "auto");
    let relays = state["relays"].as_array().unwrap();
    assert_eq!(relays.len(), 1);
    assert_eq!(relays[0]["kind"], "core");
    assert_eq!(relays[0]["addr"], ctx.addr.to_string());
    assert_eq!(relays[0]["fingerprint"], ctx.fp);
    let nodes = state["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["name"], "laptop");
    assert_eq!(nodes[0]["wg_public_key"], wg_key_b64());
    assert_eq!(nodes[0]["online"], true);

    // admin 节点列表合并在线态
    let nodes = admin_nodes(&ctx).await;
    assert_eq!(nodes[0]["online"], true);

    // 应用层心跳
    send_json(&mut ws, json!({"type": "ping"})).await;
    let pong = next_msg(&mut ws).await;
    assert_eq!(pong["type"], "pong");

    // 断开后在线态回落
    drop(ws);
    let mut offline = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let nodes = admin_nodes(&ctx).await;
        if !nodes[0]["online"].as_bool().unwrap() {
            offline = true;
            break;
        }
    }
    assert!(offline, "断开后 admin 列表必须显示离线");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_events_and_signal_forwarding() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = start_relay(dir.path()).await;
    let cred_a = register_node(&ctx, "default", "alice", "na");
    let cred_b = register_node(&ctx, "default", "alice", "nb");

    // na 先上线并上报候选地址
    let (mut ws_a, _) = auth(ctx.addr, &ctx.fp, "default", "na", &cred_a).await;
    let _ = next_msg(&mut ws_a).await; // na 自身的 state 推送
    send_json(
        &mut ws_a,
        json!({"type": "announce", "candidates": ["203.0.113.5:40000"]}),
    )
    .await;

    // nb 上线: state 中看到 na 在线及其候选地址
    let (mut ws_b, ok_b) = auth(ctx.addr, &ctx.fp, "default", "nb", &cred_b).await;
    assert_eq!(ok_b["ip"], "100.64.0.2");
    let state = next_msg(&mut ws_b).await;
    let peers = state["nodes"].as_array().unwrap();
    let na = peers.iter().find(|p| p["name"] == "na").unwrap();
    assert_eq!(na["online"], true);
    assert_eq!(na["candidates"][0], "203.0.113.5:40000");

    // na 侧秒级感知 nb 上线 (FR-5)
    let online = next_msg(&mut ws_a).await;
    assert_eq!(online["type"], "peer_online");
    assert_eq!(online["peer"]["name"], "nb");

    // 打洞信令双向转发 (payload 不透明)
    send_json(
        &mut ws_a,
        json!({
            "type": "signal", "to": "nb",
            "payload": {"candidates": ["198.51.100.7:51820"]}
        }),
    )
    .await;
    let fwd = next_msg(&mut ws_b).await;
    assert_eq!(fwd["type"], "signal");
    assert_eq!(fwd["from"], "na");
    assert_eq!(fwd["payload"]["candidates"][0], "198.51.100.7:51820");

    // 目标不在线: 回执 signal_err
    send_json(
        &mut ws_b,
        json!({"type": "signal", "to": "ghost", "payload": {}}),
    )
    .await;
    let err = next_msg(&mut ws_b).await;
    assert_eq!(err["type"], "signal_err");
    assert_eq!(err["to"], "ghost");

    // nb 断开: na 秒级感知下线 (FR-5)
    drop(ws_b);
    let offline = next_msg(&mut ws_a).await;
    assert_eq!(offline["type"], "peer_offline");
    assert_eq!(offline["name"], "nb");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replaced_connection_closes_old() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = start_relay(dir.path()).await;
    let cred = register_node(&ctx, "default", "alice", "laptop");

    let (mut ws_old, _) = auth(ctx.addr, &ctx.fp, "default", "laptop", &cred).await;
    let _ = next_msg(&mut ws_old).await; // state 推送

    // 同名新连接顶替: 旧连接收到 Closed
    let (mut ws_new, _) = auth(ctx.addr, &ctx.fp, "default", "laptop", &cred).await;
    let _ = next_msg(&mut ws_new).await; // state 推送
    let closed = next_msg(&mut ws_old).await;
    assert_eq!(closed["type"], "closed");

    // 新连接仍可用, 节点保持在线 (顶替不产生下线事件)
    send_json(&mut ws_new, json!({"type": "ping"})).await;
    let pong = next_msg(&mut ws_new).await;
    assert_eq!(pong["type"], "pong");
    let nodes = admin_nodes(&ctx).await;
    assert_eq!(nodes[0]["online"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revocation_disconnects_and_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = start_relay(dir.path()).await;
    let cred_a = register_node(&ctx, "default", "alice", "laptop");

    // 属主禁用: 在线信令立即断开 (存量凭证实时吊销)
    let (mut ws, _) = auth(ctx.addr, &ctx.fp, "default", "laptop", &cred_a).await;
    let _ = next_msg(&mut ws).await; // state 推送
    let (s, _) = post(
        &ctx.agent,
        &format!("{}/api/admin/users/disable", ctx.base),
        Some(&admin_auth(&ctx)),
        json!({"network": "default", "name": "alice"}),
    );
    assert_eq!(s, 200);
    let closed = next_msg(&mut ws).await;
    assert_eq!(closed["type"], "closed");

    // 重连被拒: 禁用属主下 hello 即拒绝 (统一口径, 不进入挑战阶段)
    let mut retry = ws_dial(ctx.addr, &ctx.fp).await.unwrap();
    send_json(
        &mut retry,
        json!({"type": "hello", "network": "default", "name": "laptop"}),
    )
    .await;
    let err = next_msg(&mut retry).await;
    assert_eq!(err["type"], "auth_err");

    // 节点移除: 在线断开 + 重连被拒
    let cred_b = register_node(&ctx, "default", "bob", "nb");
    let (mut ws_b, _) = auth(ctx.addr, &ctx.fp, "default", "nb", &cred_b).await;
    let _ = next_msg(&mut ws_b).await; // state 推送
    let (s, _) = post(
        &ctx.agent,
        &format!("{}/api/admin/nodes/remove", ctx.base),
        Some(&admin_auth(&ctx)),
        json!({"network": "default", "name": "nb"}),
    );
    assert_eq!(s, 200);
    let closed = next_msg(&mut ws_b).await;
    assert_eq!(closed["type"], "closed");
    let mut retry_b = ws_dial(ctx.addr, &ctx.fp).await.unwrap();
    send_json(
        &mut retry_b,
        json!({"type": "hello", "network": "default", "name": "nb"}),
    )
    .await;
    let err = next_msg(&mut retry_b).await;
    assert_eq!(err["type"], "auth_err");
    // 移除的 nb 不再出现在列表 (laptop 行保留, 属主禁用不移除行)
    let nodes = admin_nodes(&ctx).await;
    let names: Vec<&str> = nodes
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["name"].as_str())
        .collect();
    assert_eq!(names, vec!["laptop"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ws_connect_rate_limit() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = start_relay(dir.path()).await;

    // 20 次 / 分钟 / 源: 前 20 次握手成功 (不做认证随即断开), 第 21 次被拒
    for _ in 0..20 {
        let ws = ws_dial(ctx.addr, &ctx.fp)
            .await
            .expect("限额内建连必须成功");
        drop(ws);
    }
    let err = match ws_dial(ctx.addr, &ctx.fp).await {
        Ok(_) => panic!("超过阈值的建连必须被限速拒绝"),
        Err(e) => e,
    };
    assert!(err.contains("429"), "期望 429 限速, 实际: {err}");
}
