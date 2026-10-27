//! Description: HTTPS 服务: 管理区 (admin 鉴权) + 节点注册区 (一次性令牌 + 限速) + 信令 WS + 转发 WS + 卫星注册。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! TLS 由自签证书承担, 客户端以指纹固定校验 (见 kunxtier-core::tls)。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use kunxtier_core::address;
use kunxtier_core::config::RelayConfig;
use kunxtier_core::protocol::{
    ApiErrorBody, CreateNetworkRequest, CreateUserRequest, IssueTokenRequest, IssueTokenResponse,
    LoginRequest, LoginResponse, NetworkInfo, NodeInfo, PeerPathReport, RegisterRequest,
    RegisterResponse, RelayInfo, RemoveNodeRequest, SetNodePolicyRequest, SignalRelay,
    SignalServerMsg,
};

use crate::forward::ForwardRegistry;
use crate::ratelimit::RateLimiter;
use crate::satellite::SatelliteRegistry;
use crate::session::{SessionTable, SESSION_TTL};
use crate::signaling::{self, OnlineRegistry};
use crate::store::Store;
use crate::{cert, Error};

pub struct AppState {
    store: Mutex<Store>,
    /// 注册尝试限速 (成功也计数): 10 次 / 分钟 / 源。
    register_limiter: RateLimiter,
    /// admin 认证失败计数: 20 次 / 分钟 / 源。
    admin_limiter: RateLimiter,
    /// 控制台 admin 会话 (登录签发, 内存态)。
    pub(crate) sessions: SessionTable,
    /// 信令 / 转发 / 卫星 WS 建连限速 (成功也计数): 20 次 / 分钟 / 源 (NFR-9)。
    pub(crate) ws_limiter: RateLimiter,
    /// 在线节点注册表 (信令会话持有, admin 列表合并在线态)。
    pub(crate) online: OnlineRegistry,
    /// 内置转发注册表 (/ws/relay)。
    pub(crate) forward: ForwardRegistry,
    /// 在线卫星注册表 (/ws/satellite)。
    pub(crate) satellites: SatelliteRegistry,
    /// 内置转发条目 (中继列表末位兜底, run 时按配置与证书装配)。
    pub(crate) builtin: SignalRelay,
    /// 核心配置的公共 STUN 列表 (host:port), 经信令 State 下发 (FR-2)。
    pub(crate) stuns: Vec<String>,
}

impl AppState {
    pub fn new(store: Store, builtin: SignalRelay, stuns: Vec<String>) -> Self {
        AppState {
            store: Mutex::new(store),
            register_limiter: RateLimiter::new(10, Duration::from_secs(60)),
            admin_limiter: RateLimiter::new(20, Duration::from_secs(60)),
            sessions: SessionTable::new(SESSION_TTL),
            ws_limiter: RateLimiter::new(20, Duration::from_secs(60)),
            online: OnlineRegistry::new(),
            forward: ForwardRegistry::new(),
            satellites: SatelliteRegistry::new(),
            builtin,
            stuns,
        }
    }

    /// 取 store 锁。
    ///
    /// unwrap 理由: 锁中毒意味着持锁线程在临界区内 panic, 连接状态已不可信,
    /// 快速失败优于带病继续服务。
    pub(crate) fn lock_store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap()
    }

    /// 当前中继列表: 卫星按地址序在前, 内置转发恒在末位兜底 (FR-3)。
    pub(crate) fn relay_list(&self) -> Vec<SignalRelay> {
        let mut relays = self.satellites.list();
        relays.push(self.builtin.clone());
        relays
    }

    /// 中继列表变化全网广播 (卫星注册 / 摘除 / 踢线)。
    pub(crate) fn broadcast_relays(&self) {
        let msg = SignalServerMsg::Relays {
            relays: self.relay_list(),
        };
        self.online.broadcast_all(msg);
    }
}

/// 统一错误出口: (状态码, 机器码, 人话)。
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn bad_request(message: String) -> Self {
        ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message,
        }
    }

    fn unauthorized() -> Self {
        ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "admin 凭证缺失或不正确".into(),
        }
    }

    fn too_many() -> Self {
        ApiError {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
            message: "尝试过于频繁, 稍后再试".into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorBody {
            error: self.code.to_string(),
            message: self.message,
        };
        (self.status, Json(body)).into_response()
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        match e {
            Error::Usage(m) => ApiError::bad_request(m),
            other => {
                tracing::error!(error = %other, "内部错误");
                ApiError {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    code: "internal",
                    message: "内部错误".into(),
                }
            }
        }
    }
}

/// handler 返回类型: 统一走 ApiError 响应出口。
type ApiResult<T> = std::result::Result<T, ApiError>;

#[derive(Debug, Deserialize)]
struct NodesQuery {
    network: Option<String>,
    /// 属主过滤 (FR-3: 按属主查看)。
    owner: Option<String>,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/node/register", post(register))
        .route(
            "/api/admin/networks",
            get(list_networks).post(create_network),
        )
        .route("/api/admin/users", post(create_user))
        .route("/api/admin/users/disable", post(disable_user))
        .route("/api/admin/tokens", post(issue_token))
        .route("/api/admin/tokens/{id}/revoke", post(revoke_token))
        .route("/api/admin/login", post(login))
        .route("/api/admin/logout", post(logout))
        .route("/api/admin/relays", get(list_relays))
        .route("/api/admin/nodes", get(list_nodes))
        .route("/api/admin/nodes/remove", post(remove_node))
        .route("/api/admin/nodes/policy", post(set_node_policy))
        .route("/ws/signal", get(crate::signaling::ws_handler))
        .route("/ws/relay", get(crate::forward::core_ws_handler))
        .route("/ws/satellite", get(crate::satellite::ws_handler))
        // 控制台托管: 嵌入资源按路径命中, 页面路径回落入口页;
        // /api 与 /ws 未匹配保持 404 JSON
        .fallback_service(get(crate::console::serve_console))
        .with_state(state)
}

fn require_admin(state: &AppState, ip: IpAddr, headers: &HeaderMap) -> ApiResult<()> {
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    // 双轨: 控制台会话令牌 (内存校验优先) 或 admin 凭证 (CLI 沿用)
    let ok = bearer
        .map(|t| state.sessions.verify(t) || state.lock_store().verify_admin(t))
        .unwrap_or(false);
    if ok {
        return Ok(());
    }
    if !state.admin_limiter.check(ip) {
        return Err(ApiError::too_many());
    }
    tracing::warn!(%ip, "admin 认证失败");
    Err(ApiError::unauthorized())
}

/// 登录: admin 凭证换会话令牌 (FR-6); 失败计数沿用 admin 限速。
async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Json<LoginResponse>> {
    if !state.lock_store().verify_admin(&req.credential) {
        if !state.admin_limiter.check(addr.ip()) {
            return Err(ApiError::too_many());
        }
        tracing::warn!(ip = %addr.ip(), "admin 登录失败");
        return Err(ApiError::unauthorized());
    }
    let (token, expires_at) = state.sessions.issue();
    tracing::info!(ip = %addr.ip(), "admin 登录, 会话签发");
    Ok(Json(LoginResponse { token, expires_at }))
}

/// 登出: 吊销当前会话 (幂等; 以 admin 凭证调用时无会话可吊, 同样返回 ok)。
async fn logout(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    if let Some(token) = bearer_of(&headers) {
        state.sessions.revoke(token);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn bearer_of(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
}

/// 中继汇总 (FR-5): 在线卫星 + 内置条目, 各带连接节点数。
/// 卫星计数来自节点路径上报聚合 (每节点每中继计一次);
/// 内置条目以转发注册表实际在线端点数计。
async fn list_relays(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<RelayInfo>>> {
    require_admin(&state, addr.ip(), &headers)?;
    let usage = state.online.paths_by_relay();
    let mut relays: Vec<RelayInfo> = state
        .satellites
        .list()
        .into_iter()
        .map(|r| {
            let nodes = usage.get(&r.addr).copied().unwrap_or(0);
            RelayInfo {
                addr: r.addr,
                kind: r.kind,
                fingerprint: r.fingerprint,
                nodes,
            }
        })
        .collect();
    relays.push(RelayInfo {
        addr: state.builtin.addr.clone(),
        kind: "core".into(),
        fingerprint: state.builtin.fingerprint.clone(),
        nodes: state.forward.online_count(),
    });
    Ok(Json(relays))
}

async fn register(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<Json<RegisterResponse>> {
    if !state.register_limiter.check(addr.ip()) {
        return Err(ApiError::too_many());
    }
    let (network, ip, node_credential) =
        match state
            .lock_store()
            .consume_node_token(&req.token, &req.name, &req.wg_public_key)
        {
            Ok(out) => out,
            Err(e) => {
                // FR-4 / NFR-4: 留痕失败类别与来源, 不记录令牌 / 凭证明文
                tracing::warn!(ip = %addr.ip(), node = %req.name, error = %e, "节点注册被拒");
                return Err(e.into());
            }
        };
    // 注意: 不记录任何令牌 / 凭证明文 (NFR-4)
    tracing::info!(node = %req.name, network = %network, ip = %ip, "节点注册成功");
    Ok(Json(RegisterResponse {
        network,
        ip,
        node_credential,
    }))
}

async fn create_network(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<CreateNetworkRequest>,
) -> ApiResult<Json<NetworkInfo>> {
    require_admin(&state, addr.ip(), &headers)?;
    let info = {
        let store = state.lock_store();
        let base = match req.cidr.as_deref() {
            Some(c) => address::parse_cidr(c).map_err(|e| Error::Usage(e.to_string()))?,
            None => {
                let n = store.network_count()?;
                address::nth_network_base(n).map_err(|e| Error::Usage(e.to_string()))?
            }
        };
        store.create_network(&req.name, base)?
    };
    Ok(Json(info))
}

async fn create_user(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    let id = state.lock_store().create_user(&req.network, &req.name)?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn disable_user(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    state.lock_store().disable_user(&req.network, &req.name)?;
    // 属主禁用即吊销其名下存量节点凭证: 在线信令立即断开 (NFR-3 配套)
    let nodes = state
        .lock_store()
        .node_names_of_user(&req.network, &req.name)?;
    for name in nodes {
        state.online.kick(&req.network, &name);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn issue_token(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<IssueTokenRequest>,
) -> ApiResult<Json<IssueTokenResponse>> {
    require_admin(&state, addr.ip(), &headers)?;
    let kind = req.kind.as_deref().unwrap_or("node");
    let ttl = req.ttl_secs.unwrap_or(86400);
    let (id, token) =
        state
            .lock_store()
            .issue_token(&req.network, req.user.as_deref(), kind, ttl)?;
    Ok(Json(IssueTokenResponse {
        token_id: id,
        token,
    }))
}

async fn revoke_token(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    let kind = state.lock_store().revoke_token(id)?;
    // relay 令牌撤销即踢线: 持该令牌的卫星断开, 列表实时收窄 (FR-18 / NFR 吊销即断)
    if kind == "relay" && state.satellites.kick_by_token(id) > 0 {
        state.broadcast_relays();
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn list_networks(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<NetworkInfo>>> {
    require_admin(&state, addr.ip(), &headers)?;
    Ok(Json(state.lock_store().list_networks()?))
}

async fn list_nodes(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<NodesQuery>,
) -> ApiResult<Json<Vec<NodeInfo>>> {
    require_admin(&state, addr.ip(), &headers)?;
    let online = state.online.online_by_network();
    let mut nodes = state.lock_store().list_nodes(q.network.as_deref())?;
    if let Some(owner) = q.owner.as_deref() {
        nodes.retain(|n| n.owner.as_deref() == Some(owner));
    }
    // 候选地址、版本与路径快照来自信令注册表 (仅在线节点有意义), 按网络取一次
    let mut cand_cache: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    let mut ver_cache: HashMap<String, HashMap<String, Option<String>>> = HashMap::new();
    let mut path_cache: HashMap<String, HashMap<String, Vec<PeerPathReport>>> = HashMap::new();
    for n in &mut nodes {
        n.online = online
            .get(&n.network)
            .is_some_and(|set| set.contains(&n.name));
        if !n.online {
            continue;
        }
        let cands = cand_cache
            .entry(n.network.clone())
            .or_insert_with(|| state.online.online_candidates(&n.network));
        let versions = ver_cache
            .entry(n.network.clone())
            .or_insert_with(|| state.online.online_versions(&n.network));
        let slot_paths = path_cache
            .entry(n.network.clone())
            .or_insert_with(|| state.online.paths_of(&n.network));
        if let Some(c) = cands.get(&n.name) {
            n.candidates = c.clone();
        }
        if let Some(v) = versions.get(&n.name) {
            n.version = v.clone();
        }
        if let Some(p) = slot_paths.get(&n.name) {
            n.paths = p.clone();
        }
    }
    Ok(Json(nodes))
}

async fn remove_node(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<RemoveNodeRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    state.lock_store().remove_node(&req.network, &req.name)?;
    // 节点移除即凭证吊销: 在线信令立即断开并广播下线
    state.online.kick(&req.network, &req.name);
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 节点级路径策略设置 (FR-10): 落库后向在线节点定向推送, 离线节点重连时经 State 补齐。
async fn set_node_policy(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<SetNodePolicyRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    let updated = state
        .lock_store()
        .set_node_policy(&req.network, &req.name, req.policy)?;
    if !updated {
        return Err(ApiError::bad_request(format!(
            "节点不存在: {}/{}",
            req.network, req.name
        )));
    }
    signaling::push_policy(&state, &req.network, &req.name, req.policy);
    Ok(Json(
        serde_json::json!({ "ok": true, "policy": req.policy.to_string() }),
    ))
}

/// 启动核心中继 (TLS)。
pub async fn run(cfg: &RelayConfig) -> crate::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let store = Store::open(&cfg.db_path())?;
    // 内置转发条目: 公布地址 + 自身证书指纹 (节点按条目固定校验)
    let cert_pem = std::fs::read_to_string(cfg.cert_path())
        .map_err(|e| Error::Cert(format!("加载证书失败 (请先执行 relay --init): {e}")))?;
    let fingerprint = cert::fingerprint(&cert::der_from_pem(&cert_pem)?);
    let builtin_addr = cfg.builtin_addr();
    if builtin_addr.is_none() {
        tracing::info!("通配监听且未配置 advertise: 内置转发条目地址置空, 节点以自身核心地址代入");
    }
    let state = Arc::new(AppState::new(
        store,
        SignalRelay {
            addr: builtin_addr.map(|a| a.to_string()).unwrap_or_default(),
            kind: "core".into(),
            fingerprint,
        },
        cfg.stun_servers.clone(),
    ));
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cfg.cert_path(), cfg.key_path())
        .await
        .map_err(|e| Error::Cert(format!("加载证书失败 (请先执行 relay --init): {e}")))?;
    let app = build_router(state).into_make_service_with_connect_info::<SocketAddr>();
    tracing::info!(listen = %cfg.listen, "核心中继启动");
    axum_server::bind_rustls(cfg.listen, tls)
        .serve(app)
        .await
        .map_err(|e| Error::Serve(format!("{e}")))?;
    Ok(())
}
