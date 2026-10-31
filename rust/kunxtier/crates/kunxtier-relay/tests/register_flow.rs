//! Description: 登记闭环集成测试: init → 起 HTTPS 服务 → admin 操作 → 节点注册 → 守恒校验。
//!
//! Author: qinzhenya
//! Created: 2026-10-20

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};

use kunxtier_core::config::RelayConfig;
use kunxtier_core::protocol::SignalRelay;
use kunxtier_core::tls::pinning_agent;
use kunxtier_relay::server::{build_router, AppState};
use kunxtier_relay::store::Store;

async fn start_relay(dir: &Path) -> (SocketAddr, String, String) {
    let cfg = RelayConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.to_path_buf(),
        advertise: None,
        stun_servers: Vec::new(),
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
        Vec::new(),
    ));
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cfg.cert_path(), cfg.key_path())
        .await
        .unwrap();
    let app = build_router(state).into_make_service_with_connect_info::<SocketAddr>();
    // 多线程 runtime: 测试主线程做阻塞 HTTP 调用, 服务端任务由 worker 驱动
    tokio::spawn(async move {
        let _ = axum_server::from_tcp_rustls(listener, tls).serve(app).await;
    });
    (addr, out.admin_credential, out.fingerprint)
}

fn wg_key_b64() -> String {
    STANDARD.encode([3u8; 32])
}

/// GET 并返回 (状态码, 响应体); 非传输错误不视为失败。
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registration_loop_and_guards() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, admin, fp) = start_relay(dir.path()).await;
    let base = format!("https://{addr}");
    let agent = pinning_agent(&fp);
    let auth = format!("Bearer {admin}");

    // 建用户与一次性令牌
    let (s, _) = post(
        &agent,
        &format!("{base}/api/v1/admin/users"),
        Some(&auth),
        json!({"network": "default", "name": "alice"}),
    );
    assert_eq!(s, 200);

    let (s, tok) = post(
        &agent,
        &format!("{base}/api/v1/admin/tokens"),
        Some(&auth),
        json!({"network": "default", "user": "alice"}),
    );
    assert_eq!(s, 200);
    let token = tok["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("kxt_node_"));

    // 节点注册: 拿到网络 / IP / 节点凭证
    let (s, reg) = post(
        &agent,
        &format!("{base}/api/v1/node/register"),
        None,
        json!({"token": token, "name": "laptop", "wg_public_key": wg_key_b64()}),
    );
    assert_eq!(s, 200);
    assert_eq!(reg["network"], "default");
    assert_eq!(reg["ip"], "100.64.0.1");
    assert!(reg["node_credential"]
        .as_str()
        .unwrap()
        .starts_with("kxt_nid_"));

    // 一次性令牌: 复用被拒
    let (s, _) = post(
        &agent,
        &format!("{base}/api/v1/node/register"),
        None,
        json!({"token": token, "name": "laptop2", "wg_public_key": wg_key_b64()}),
    );
    assert_eq!(s, 400);

    // 同名注册被拒 (FR-17)
    let (_, tok2) = post(
        &agent,
        &format!("{base}/api/v1/admin/tokens"),
        Some(&auth),
        json!({"network": "default", "user": "alice"}),
    );
    let (s, _) = post(
        &agent,
        &format!("{base}/api/v1/node/register"),
        None,
        json!({
            "token": tok2["token"].as_str().unwrap(),
            "name": "laptop",
            "wg_public_key": wg_key_b64()
        }),
    );
    assert_eq!(s, 400);

    // admin 列表可见节点与属主标注
    let (s, nodes) = get(&agent, &format!("{base}/api/v1/admin/nodes"), Some(&auth));
    assert_eq!(s, 200);
    assert_eq!(nodes["items"][0]["name"], "laptop");
    assert_eq!(nodes["items"][0]["owner"], "alice");

    // 禁用用户后其名下令牌签发被拒 (FR-1)
    let (s, _) = post(
        &agent,
        &format!("{base}/api/v1/admin/users/disable"),
        Some(&auth),
        json!({"network": "default", "name": "alice"}),
    );
    assert_eq!(s, 200);
    let (s, _) = post(
        &agent,
        &format!("{base}/api/v1/admin/tokens"),
        Some(&auth),
        json!({"network": "default", "user": "alice"}),
    );
    // 用户禁用属状态冲突: 409 (原 400 语义错位, 契约整改后纠正)
    assert_eq!(s, 409);

    // 移除节点后列表为空
    let (s, _) = post(
        &agent,
        &format!("{base}/api/v1/admin/nodes/remove"),
        Some(&auth),
        json!({"network": "default", "name": "laptop"}),
    );
    assert_eq!(s, 200);
    let (s, nodes) = get(&agent, &format!("{base}/api/v1/admin/nodes"), Some(&auth));
    assert_eq!(s, 200);
    assert_eq!(nodes["items"].as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fingerprint_mismatch_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _admin, _fp) = start_relay(dir.path()).await;
    let bad = pinning_agent(&"0".repeat(64));
    let err = bad.get(&format!("https://{addr}/healthz")).call();
    assert!(err.is_err(), "指纹不匹配的连接必须被拒绝");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_auth_and_register_rate_limit() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, admin, fp) = start_relay(dir.path()).await;
    let base = format!("https://{addr}");
    let agent = pinning_agent(&fp);

    // 无凭证 → 401
    let (s, _) = get(&agent, &format!("{base}/api/v1/admin/networks"), None);
    assert_eq!(s, 401);

    // 错误凭证 → 401
    let (s, _) = get(
        &agent,
        &format!("{base}/api/v1/admin/networks"),
        Some("Bearer kxt_admin_wrong"),
    );
    assert_eq!(s, 401);

    // 正确凭证可见默认网络
    let auth = format!("Bearer {admin}");
    let (s, nets) = get(
        &agent,
        &format!("{base}/api/v1/admin/networks"),
        Some(&auth),
    );
    assert_eq!(s, 200);
    assert_eq!(nets["items"][0]["cidr"], "100.64.0.0/16");

    // 注册端点限速: 连续垃圾令牌最终 429 (10 次 / 分钟 / 源)
    let mut hit_429 = false;
    for i in 0..12 {
        let (s, _) = post(
            &agent,
            &format!("{base}/api/v1/node/register"),
            None,
            json!({"token": "kxt_node_garbage", "name": format!("x{i}"), "wg_public_key": wg_key_b64()}),
        );
        if s == 429 {
            hit_429 = true;
            break;
        }
    }
    assert!(hit_429, "超过阈值后必须触发限速");
}
