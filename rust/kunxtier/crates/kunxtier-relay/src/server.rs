//! Description: HTTPS 服务: 管理区 (admin 鉴权) + 节点注册区 (一次性令牌 + 限速)。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! TLS 由自签证书承担, 客户端以指纹固定校验 (见 client_tls)。

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
    ApiErrorBody, CreateNetworkRequest, CreateUserRequest, IssueTokenRequest,
    IssueTokenResponse, NetworkInfo, NodeInfo, RegisterRequest, RegisterResponse,
    RemoveNodeRequest,
};

use crate::ratelimit::RateLimiter;
use crate::store::Store;
use crate::Error;

pub struct AppState {
    store: Mutex<Store>,
    /// 注册尝试限速 (成功也计数): 10 次 / 分钟 / 源。
    register_limiter: RateLimiter,
    /// admin 认证失败计数: 20 次 / 分钟 / 源。
    admin_limiter: RateLimiter,
}

impl AppState {
    pub fn new(store: Store) -> Self {
        AppState {
            store: Mutex::new(store),
            register_limiter: RateLimiter::new(10, Duration::from_secs(60)),
            admin_limiter: RateLimiter::new(20, Duration::from_secs(60)),
        }
    }

    /// 取 store 锁。
    ///
    /// unwrap 理由: 锁中毒意味着持锁线程在临界区内 panic, 连接状态已不可信,
    /// 快速失败优于带病继续服务。
    fn lock_store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap()
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
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/node/register", post(register))
        .route("/api/admin/networks", get(list_networks).post(create_network))
        .route("/api/admin/users", post(create_user))
        .route("/api/admin/users/disable", post(disable_user))
        .route("/api/admin/tokens", post(issue_token))
        .route("/api/admin/tokens/{id}/revoke", post(revoke_token))
        .route("/api/admin/nodes", get(list_nodes))
        .route("/api/admin/nodes/remove", post(remove_node))
        .with_state(state)
}

fn require_admin(state: &AppState, ip: IpAddr, headers: &HeaderMap) -> ApiResult<()> {
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    let ok = bearer
        .map(|t| state.lock_store().verify_admin(t))
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

async fn register(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<Json<RegisterResponse>> {
    if !state.register_limiter.check(addr.ip()) {
        return Err(ApiError::too_many());
    }
    let (network, ip, node_credential) = state
        .lock_store()
        .consume_node_token(&req.token, &req.name, &req.wg_public_key)?;
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
    let id = state
        .lock_store()
        .create_user(&req.network, &req.name)?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn disable_user(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    state
        .lock_store()
        .disable_user(&req.network, &req.name)?;
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
    let (id, token) = state.lock_store().issue_token(
        &req.network,
        req.user.as_deref(),
        kind,
        ttl,
    )?;
    Ok(Json(IssueTokenResponse { token_id: id, token }))
}

async fn revoke_token(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    state.lock_store().revoke_token(id)?;
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
    Ok(Json(state.lock_store().list_nodes(q.network.as_deref())?))
}

async fn remove_node(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<RemoveNodeRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    state.lock_store().remove_node(&req.network, &req.name)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 启动核心中继 (TLS)。
pub async fn run(cfg: &RelayConfig) -> crate::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let store = Store::open(&cfg.db_path())?;
    let state = Arc::new(AppState::new(store));
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        cfg.cert_path(),
        cfg.key_path(),
    )
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
