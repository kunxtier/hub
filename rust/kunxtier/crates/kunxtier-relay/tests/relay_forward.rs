//! Description: 转发双模式集成测试: 核心内置转发、卫星注册与列表分发、断连摘除、卫星声明式转发。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! 核心转发面复用信令挑战应答 (IP 由库存决定); 卫星转发面走声明式 bind (零知识)。
//! 卫星以真实进程形态起停 (start / kill), 覆盖 e2e 矩阵的卫星注册与下线摘除场景。

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use kunxtier_core::config::{RelayConfig, SatelliteConfig};
use kunxtier_core::protocol::{decode_forward_frame, encode_forward_frame, SignalRelay};
use kunxtier_relay::client_tls::{pinning_agent, PinVerifier};
use kunxtier_relay::satellite;
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

async fn start_core(dir: &Path) -> Ctx {
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
        SignalRelay {
            addr: addr.to_string(),
            kind: "core".into(),
            fingerprint: out.fingerprint.clone(),
        },
    ));
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cfg.cert_path(), cfg.key_path())
        .await
        .unwrap();
    let app = build_router(state).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move {
        let _ = axum_server::from_tcp_rustls(listener, tls).serve(app).await;
    });
    Ctx {
        addr,
        base: format!("https://{addr}"),
        admin: out.admin_credential,
        agent: pinning_agent(&out.fingerprint),
        fp: out.fingerprint,
    }
}

fn wg_key_b64() -> String {
    STANDARD.encode([3u8; 32])
}

/// 注册节点并返回 (凭证明文, 分配到的 IP)。
fn register_node(ctx: &Ctx, name: &str) -> (String, String) {
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
            "wg_public_key": wg_key_b64()
        }),
    );
    assert_eq!(s, 200, "节点注册必须成功");
    (
        reg["node_credential"].as_str().unwrap().to_string(),
        reg["ip"].as_str().unwrap().to_string(),
    )
}

/// 签发 relay 令牌, 返回明文。
fn issue_relay_token(ctx: &Ctx) -> String {
    let (_, tok) = post(
        &ctx.agent,
        &format!("{}/api/admin/tokens", ctx.base),
        Some(&format!("Bearer {}", ctx.admin)),
        json!({"network": "default", "kind": "relay", "ttl_secs": 600}),
    );
    tok["token"].as_str().unwrap().to_string()
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

/// TLS (指纹固定) + WS 拨号到指定路径。
async fn ws_dial(addr: SocketAddr, fingerprint: &str, path: &str) -> Result<WsClient, String> {
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
    let (ws, _resp) = tokio_tungstenite::client_async(&format!("wss://localhost{path}"), tls)
        .await
        .map_err(|e| format!("ws: {e}"))?;
    Ok(ws)
}

async fn send_json(ws: &mut WsClient, v: Value) {
    ws.send(WsMessage::Text(v.to_string().into()))
        .await
        .unwrap();
}

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

async fn next_msg(ws: &mut WsClient) -> Value {
    tokio::time::timeout(Duration::from_secs(5), recv_json(ws))
        .await
        .expect("等待消息超时")
        .expect("连接已关闭")
}

/// 核心转发面准入: 信令挑战应答, 返回 Bound 消息。
async fn forward_auth_core(
    addr: SocketAddr,
    fp: &str,
    name: &str,
    credential: &str,
) -> (WsClient, Value) {
    let mut ws = ws_dial(addr, fp, "/ws/relay").await.unwrap();
    send_json(
        &mut ws,
        json!({"type": "hello", "network": "default", "name": name}),
    )
    .await;
    let challenge = next_msg(&mut ws).await;
    assert_eq!(challenge["type"], "challenge");
    let nonce = STANDARD
        .decode(challenge["nonce"].as_str().unwrap())
        .unwrap();
    let digest = STANDARD.encode(kunxtier_core::token::challenge_answer(&nonce, credential));
    send_json(&mut ws, json!({"type": "auth", "digest": digest})).await;
    let bound = next_msg(&mut ws).await;
    assert_eq!(bound["type"], "bound", "转发准入必须通过: {bound}");
    (ws, bound)
}

/// 卫星转发面准入: 声明式 bind, 返回 Bound 消息。
async fn forward_bind_claim(addr: SocketAddr, fp: &str, ip: &str) -> (WsClient, Value) {
    let mut ws = ws_dial(addr, fp, "/ws/relay").await.unwrap();
    send_json(&mut ws, json!({"type": "bind", "ip": ip})).await;
    let bound = next_msg(&mut ws).await;
    assert_eq!(bound["type"], "bound", "声明式准入必须通过: {bound}");
    (ws, bound)
}

/// 取下一帧二进制转发帧并解码。
async fn next_frame(ws: &mut WsClient) -> (Ipv4Addr, Vec<u8>) {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("等待转发帧超时")
            .expect("连接已关闭");
        match msg {
            Ok(WsMessage::Ping(p)) => {
                ws.send(WsMessage::Pong(p)).await.unwrap();
            }
            Ok(WsMessage::Binary(b)) => {
                let (dst, payload) = decode_forward_frame(&b).expect("服务端帧必须可解码");
                return (dst, payload.to_vec());
            }
            Ok(_) => continue,
            Err(_) => panic!("转发通道错误"),
        }
    }
}

/// 起一个真实卫星进程 (令牌 / 指纹 / 端口由用例给定), 返回句柄。
async fn start_satellite(ctx: &Ctx, token: String) -> satellite::Satellite {
    let port = {
        // 预留端口作 advertise, 随后立即由卫星监听复用 (回环单进程, 冲突窗口可忽略)
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let cfg = SatelliteConfig {
        core: ctx.addr.to_string(),
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        advertise: format!("127.0.0.1:{port}").parse().unwrap(),
        fingerprint: ctx.fp.clone(),
        token,
    };
    satellite::start(&cfg).await.expect("卫星启动必须成功")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn core_builtin_forward_between_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = start_core(dir.path()).await;
    let (cred_a, ip_a) = register_node(&ctx, "na");
    let (cred_b, ip_b) = register_node(&ctx, "nb");

    // 准入: Bound 携带库存权威 IP 与心跳参数 (声明不可信, 由 store 决定)
    let (mut wa, bound_a) = forward_auth_core(ctx.addr, &ctx.fp, "na", &cred_a).await;
    assert_eq!(bound_a["ip"], ip_a);
    assert_eq!(bound_a["heartbeat_interval_secs"], 10);
    assert_eq!(bound_a["idle_timeout_secs"], 30);
    let (mut wb, bound_b) = forward_auth_core(ctx.addr, &ctx.fp, "nb", &cred_b).await;
    assert_eq!(bound_b["ip"], ip_b);

    // A → B: 帧原样投递, 目的字段即接收方地址
    let dst_b: Ipv4Addr = ip_b.parse().unwrap();
    wa.send(WsMessage::Binary(
        encode_forward_frame(dst_b, &[0xaa, 0xbb, 0xcc]).into(),
    ))
    .await
    .unwrap();
    let (dst, payload) = next_frame(&mut wb).await;
    assert_eq!(dst, dst_b);
    assert_eq!(payload, vec![0xaa, 0xbb, 0xcc]);

    // B → A 反向
    let dst_a: Ipv4Addr = ip_a.parse().unwrap();
    wb.send(WsMessage::Binary(encode_forward_frame(dst_a, &[1]).into()))
        .await
        .unwrap();
    let (dst, payload) = next_frame(&mut wa).await;
    assert_eq!(dst, dst_a);
    assert_eq!(payload, vec![1]);

    // 未知目的: 静默丢弃, 连接保持可用 (UDP 语义)
    let ghost: Ipv4Addr = "100.64.0.99".parse().unwrap();
    wa.send(WsMessage::Binary(encode_forward_frame(ghost, &[9]).into()))
        .await
        .unwrap();
    send_json(&mut wa, json!({"type": "ping"})).await;
    let pong = next_msg(&mut wa).await;
    assert_eq!(pong["type"], "pong");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn satellite_register_list_and_forward() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = start_core(dir.path()).await;
    let (cred_a, ip_a) = register_node(&ctx, "na");
    let (_cred_b, ip_b) = register_node(&ctx, "nb");

    // 节点先上线: 初始列表仅内置条目
    let (mut sig, _) = {
        let mut ws = ws_dial(ctx.addr, &ctx.fp, "/ws/signal").await.unwrap();
        send_json(
            &mut ws,
            json!({"type": "hello", "network": "default", "name": "na"}),
        )
        .await;
        let challenge = next_msg(&mut ws).await;
        let nonce = STANDARD
            .decode(challenge["nonce"].as_str().unwrap())
            .unwrap();
        let digest = STANDARD.encode(kunxtier_core::token::challenge_answer(&nonce, &cred_a));
        send_json(&mut ws, json!({"type": "auth", "digest": digest})).await;
        let ok = next_msg(&mut ws).await;
        assert_eq!(ok["type"], "auth_ok");
        let state = next_msg(&mut ws).await;
        let relays = state["relays"].as_array().unwrap();
        assert_eq!(relays.len(), 1);
        assert_eq!(relays[0]["kind"], "core");
        (ws, ok)
    };

    // 卫星注册: 在线节点秒级收到列表全量更新 (e2e: 无需重启任何一方)
    let token = issue_relay_token(&ctx);
    let sat = start_satellite(&ctx, token).await;
    let relays = {
        let updated = next_msg(&mut sig).await;
        assert_eq!(
            updated["type"], "relays",
            "卫星注册必须触发列表分发: {updated}"
        );
        updated["relays"].as_array().unwrap().clone()
    };
    assert_eq!(relays.len(), 2);
    assert_eq!(relays[0]["kind"], "satellite");
    assert_eq!(relays[0]["addr"], format!("127.0.0.1:{}", sat.addr.port()));
    let sat_fp = relays[0]["fingerprint"].as_str().unwrap().to_string();
    assert_eq!(sat_fp.len(), 64);
    // 内置转发恒在末位兜底
    assert_eq!(relays[1]["kind"], "core");
    assert_eq!(relays[1]["addr"], ctx.addr.to_string());

    // 节点按列表拨号卫星 (指纹取自条目), 声明式 bind + 密文帧互通
    let (mut sa, bound_a) = forward_bind_claim(sat.addr, &sat_fp, &ip_a).await;
    assert_eq!(bound_a["ip"], ip_a);
    let (mut sb, bound_b) = forward_bind_claim(sat.addr, &sat_fp, &ip_b).await;
    assert_eq!(bound_b["ip"], ip_b);
    let dst_b: Ipv4Addr = ip_b.parse().unwrap();
    sa.send(WsMessage::Binary(
        encode_forward_frame(dst_b, &[7, 7, 7]).into(),
    ))
    .await
    .unwrap();
    let (dst, payload) = next_frame(&mut sb).await;
    assert_eq!(dst, dst_b);
    assert_eq!(payload, vec![7, 7, 7]);

    // 同 IP 新连接顶替: 旧连接收到 closed
    let (_sa2, _) = forward_bind_claim(sat.addr, &sat_fp, &ip_a).await;
    let closed = next_msg(&mut sa).await;
    assert_eq!(closed["type"], "closed");

    // 卫星死亡: 列表及时摘除 (FR-18), 内置条目回落兜底
    sat.kill();
    let updated = next_msg(&mut sig).await;
    assert_eq!(updated["type"], "relays");
    let relays = updated["relays"].as_array().unwrap();
    assert_eq!(relays.len(), 1);
    assert_eq!(relays[0]["kind"], "core");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn satellite_invalid_token_not_registered() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = start_core(dir.path()).await;
    let (cred_a, _ip_a) = register_node(&ctx, "na");

    // 节点上线, 初始列表仅内置条目
    let mut sig = {
        let mut ws = ws_dial(ctx.addr, &ctx.fp, "/ws/signal").await.unwrap();
        send_json(
            &mut ws,
            json!({"type": "hello", "network": "default", "name": "na"}),
        )
        .await;
        let challenge = next_msg(&mut ws).await;
        let nonce = STANDARD
            .decode(challenge["nonce"].as_str().unwrap())
            .unwrap();
        let digest = STANDARD.encode(kunxtier_core::token::challenge_answer(&nonce, &cred_a));
        send_json(&mut ws, json!({"type": "auth", "digest": digest})).await;
        let ok = next_msg(&mut ws).await;
        assert_eq!(ok["type"], "auth_ok");
        let _ = next_msg(&mut ws).await; // state
        ws
    };

    // 节点令牌冒充 relay 令牌: 注册被拒, 列表不更新 (卫星侧重试不成功)
    let node_token = {
        let auth = format!("Bearer {}", ctx.admin);
        let (_, tok) = post(
            &ctx.agent,
            &format!("{}/api/admin/tokens", ctx.base),
            Some(&auth),
            json!({"network": "default", "user": "alice"}),
        );
        tok["token"].as_str().unwrap().to_string()
    };
    let sat = start_satellite(&ctx, node_token).await;
    let none = tokio::time::timeout(Duration::from_millis(1500), recv_json(&mut sig)).await;
    assert!(
        none.is_err() || none.unwrap().is_none(),
        "非法令牌的卫星不得进入列表"
    );
    sat.kill();
}
