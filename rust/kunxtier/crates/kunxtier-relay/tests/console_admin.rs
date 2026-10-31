//! Description: 控制台后端切片集成测试: admin 会话与 API 扩展 (login / logout / relays / nodes 过滤)。
//!
//! Author: qinzhenya
//! Created: 2026-10-26

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
    tokio::spawn(async move {
        let _ = axum_server::from_tcp_rustls(listener, tls).serve(app).await;
    });
    (addr, out.admin_credential, out.fingerprint)
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

fn wg_key_b64() -> String {
    STANDARD.encode([7u8; 32])
}

/// 会话主链路: 登录 → 会话调用 admin API → relays 汇总 → 登出后失效 (FR-6 / FR-5)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_session_login_logout_and_relays() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, admin, fp) = start_relay(dir.path()).await;
    let base = format!("https://{addr}");
    let agent = pinning_agent(&fp);

    // 错误凭证 → 401 (且不签发会话)
    let (s, _) = post(
        &agent,
        &format!("{base}/api/v1/admin/login"),
        None,
        json!({"credential": "kxt_admin_wrong"}),
    );
    assert_eq!(s, 401);

    // 正确凭证 → 会话令牌
    let (s, body) = post(
        &agent,
        &format!("{base}/api/v1/admin/login"),
        None,
        json!({"credential": admin}),
    );
    assert_eq!(s, 200, "登录失败: {body}");
    let token = body["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("kxt_ses_"));
    assert!(body["expires_at"].as_i64().is_some());
    let auth = format!("Bearer {token}");

    // 会话可调用 admin API; 会话与凭证双轨 (两者都通)
    let (s, _) = get(
        &agent,
        &format!("{base}/api/v1/admin/networks"),
        Some(&auth),
    );
    assert_eq!(s, 200, "会话令牌应可用");
    let cred_auth = format!("Bearer {admin}");
    let (s, _) = get(
        &agent,
        &format!("{base}/api/v1/admin/networks"),
        Some(&cred_auth),
    );
    assert_eq!(s, 200, "admin 凭证应仍可用");

    // relays 汇总: 无卫星时仅内置条目, 在线端点数 0
    let (s, body) = get(&agent, &format!("{base}/api/v1/admin/relays"), Some(&auth));
    assert_eq!(s, 200);
    let relays = body["items"].as_array().unwrap();
    assert_eq!(relays.len(), 1);
    assert_eq!(relays[0]["kind"], "core");
    assert_eq!(relays[0]["nodes"], 0);

    // 核心托管: 根路径返回嵌入页 (占位页)
    let html = agent
        .get(&format!("{base}/"))
        .call()
        .unwrap()
        .into_string()
        .unwrap();
    assert!(html.contains("kunxtier 控制台"));

    // 登出 → 原会话失效
    let (s, _) = post(
        &agent,
        &format!("{base}/api/v1/admin/logout"),
        Some(&auth),
        json!({}),
    );
    assert_eq!(s, 200);
    let (s, _) = get(
        &agent,
        &format!("{base}/api/v1/admin/networks"),
        Some(&auth),
    );
    assert_eq!(s, 401, "登出后会话应失效");
}

/// nodes 列表扩展: 属主过滤 (FR-3), 离线节点候选与版本为空。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nodes_owner_filter_and_merged_fields() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, admin, fp) = start_relay(dir.path()).await;
    let base = format!("https://{addr}");
    let agent = pinning_agent(&fp);
    let auth = format!("Bearer {admin}");

    // 两属主各注册一个节点
    for owner in ["alice", "bob"] {
        let (s, _) = post(
            &agent,
            &format!("{base}/api/v1/admin/users"),
            Some(&auth),
            json!({"network": "default", "name": owner}),
        );
        assert_eq!(s, 200);
        let (s, body) = post(
            &agent,
            &format!("{base}/api/v1/admin/tokens"),
            Some(&auth),
            json!({"network": "default", "user": owner, "kind": "node"}),
        );
        assert_eq!(s, 200);
        let token = body["token"].as_str().unwrap();
        let (s, _) = post(
            &agent,
            &format!("{base}/api/v1/node/register"),
            None,
            json!({"token": token, "name": format!("node-{owner}"), "wg_public_key": wg_key_b64()}),
        );
        assert_eq!(s, 200, "节点注册失败");
    }

    // 属主过滤: 只见 alice 的节点
    let (s, body) = get(
        &agent,
        &format!("{base}/api/v1/admin/nodes?owner=alice"),
        Some(&auth),
    );
    assert_eq!(s, 200);
    let nodes = body["items"].as_array().unwrap();
    assert_eq!(nodes.len(), 1, "属主过滤应只剩 alice 的节点: {body}");
    assert_eq!(nodes[0]["name"], "node-alice");
    assert_eq!(nodes[0]["owner"], "alice");
    // 离线: 在线态 false, 候选与版本为空 (由信令注册表合并)
    assert_eq!(nodes[0]["online"], false);
    assert_eq!(nodes[0]["candidates"].as_array().unwrap().len(), 0);
    assert!(nodes[0]["version"].is_null());

    // 无过滤: 全量两节点
    let (s, body) = get(&agent, &format!("{base}/api/v1/admin/nodes"), Some(&auth));
    assert_eq!(s, 200);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);

    // 会话过期前 admin 凭证与会话双轨由上一用例覆盖; 未认证拒绝
    let (s, _) = get(&agent, &format!("{base}/api/v1/admin/nodes"), None);
    assert_eq!(s, 401);
}

/// 本地控制台: 静态托管占位页 + admin API 反代 (TLS 指纹固定在反代层)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_console_serves_page_and_proxies_api() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, admin, fp) = start_relay(dir.path()).await;
    let (console_addr, task) =
        kunxtier_relay::console::start_local(kunxtier_relay::console::ConsoleOptions {
            core: addr.to_string(),
            fingerprint: fp,
            listen: "127.0.0.1:0".parse().unwrap(),
        })
        .await
        .unwrap();
    let base = format!("http://{console_addr}");
    let agent = ureq::AgentBuilder::new().build();

    // 页面: 根路径与 SPA 深链均回落嵌入页
    let html = agent
        .get(&format!("{base}/"))
        .call()
        .unwrap()
        .into_string()
        .unwrap();
    assert!(html.contains("kunxtier 控制台"));
    let spa = agent
        .get(&format!("{base}/nodes"))
        .call()
        .unwrap()
        .into_string()
        .unwrap();
    assert!(spa.contains("kunxtier 控制台"), "SPA 深链应回落首页");

    // 反代: 经本地端口登录核心 (指纹校验发生在反代层), 会话可查询 admin API
    let resp = agent
        .post(&format!("{base}/api/v1/admin/login"))
        .send_json(json!({"credential": admin}))
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.into_json().unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("kxt_ses_"));
    let resp = agent
        .get(&format!("{base}/api/v1/admin/networks"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 未匹配的 API 路径经反代回核心, 得 404 JSON 而非页面
    match agent
        .get(&format!("{base}/api/v1/admin/nonexistent"))
        .call()
    {
        Err(ureq::Error::Status(404, _)) => {}
        other => panic!("期望 404, 实际: {other:?}"),
    }

    task.abort();
}
