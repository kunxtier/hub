//! Description: HTTPS 服务: 管理区 (admin 鉴权) + 节点注册区 (一次性令牌 + 限速) + 信令 WS + 转发 WS + 卫星注册。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! TLS 由自签证书承担, 客户端以指纹固定校验 (见 kunxtier-core::tls)。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{header, header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::Deserialize;

use kunxtier_core::address;
use kunxtier_core::config::RelayConfig;
use kunxtier_core::protocol::{
    ApiErrorBody, ApiErrorDetail, CreateNetworkRequest, CreateUserRequest, IssueTokenRequest,
    IssueTokenResponse, LoginRequest, LoginResponse, NetworkInfo, NodeInfo, Page, PeerPathReport,
    RegisterRequest, RegisterResponse, RelayInfo, RemoveNodeRequest, RenameUserRequest,
    SetNodePolicyRequest, SignalRelay, SignalServerMsg, UserInfo, PAGE_SIZE_DEFAULT, PAGE_SIZE_MAX,
};
use kunxtier_core::time::elapsed_ms;

use crate::forward::ForwardRegistry;
use crate::ratelimit::RateLimiter;
use crate::satellite::SatelliteRegistry;
use crate::session::{SessionTable, SESSION_TTL};
use crate::signaling::{self, OnlineRegistry};
use crate::store::{valid_name, Store};
use crate::{cert, Error};

/// token 有效期契约边界 (秒): 60s 起, 30d 封顶; 越界在入口拒绝,
/// 保证 now() + ttl 不可能进入溢出路径。
const TTL_MIN_SECS: u64 = 60;
const TTL_MAX_SECS: u64 = 30 * 86400;

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

/// 统一错误出口: (状态码, 机器码, 人话, 字段明细, Retry-After)。
/// request_id 由 request_context 中间件注入, handler 不感知。
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    details: Vec<ApiErrorDetail>,
    retry_after: Option<u64>,
}

impl ApiError {
    fn bad_request(message: String) -> Self {
        ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message,
            details: Vec::new(),
            retry_after: None,
        }
    }

    /// 400 + 字段级明细 (边界校验: 分页参数 / ttl / 过滤白名单)。
    fn invalid_field(field: &str, reason: String) -> Self {
        ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: format!("请求参数越界: {field}"),
            details: vec![ApiErrorDetail {
                field: field.to_string(),
                reason,
            }],
            retry_after: None,
        }
    }

    fn unauthorized() -> Self {
        ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "admin 凭证缺失或不正确".into(),
            details: Vec::new(),
            retry_after: None,
        }
    }

    /// 429 附 Retry-After (固定窗口剩余秒数, 由限速器计算)。
    fn too_many(retry_after: u64) -> Self {
        ApiError {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
            message: "尝试过于频繁, 稍后再试".into(),
            details: Vec::new(),
            retry_after: Some(retry_after),
        }
    }

    fn not_found(message: String) -> Self {
        ApiError {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message,
            details: Vec::new(),
            retry_after: None,
        }
    }

    fn conflict(message: String) -> Self {
        ApiError {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message,
            details: Vec::new(),
            retry_after: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorBody {
            code: self.code.to_string(),
            message: self.message,
            details: self.details,
            request_id: String::new(),
        };
        let mut resp = (self.status, Json(body)).into_response();
        if let Some(secs) = self.retry_after {
            if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
        }
        resp
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        match e {
            Error::Usage(m) => ApiError::bad_request(m),
            Error::NotFound(m) => ApiError::not_found(m),
            Error::Conflict(m) => ApiError::conflict(m),
            other => {
                tracing::error!(error = %other, "内部错误");
                ApiError {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    code: "internal",
                    message: "内部错误".into(),
                    details: Vec::new(),
                    retry_after: None,
                }
            }
        }
    }
}

/// 节点注册公开面防枚举压平: 令牌生命周期错误消息也统一 (否则
/// "已用 / 已撤销 / 已过期 / 不存在" 每态一答即令牌状态预言机, NFR-4);
/// 状态码与消息同时压平, 失败类别只留日志。
fn register_error(e: Error) -> ApiError {
    match e {
        Error::Usage(m) | Error::NotFound(m) | Error::Conflict(m) => ApiError::bad_request(m),
        Error::TokenRejected(_) => ApiError::bad_request("令牌无效或不可用".into()),
        other => ApiError::from(other),
    }
}

/// handler 返回类型: 统一走 ApiError 响应出口。
type ApiResult<T> = std::result::Result<T, ApiError>;

/// 列表公共查询参数: 过滤白名单 + 分页 (page 从 1 起, size 有契约上限)。
#[derive(Debug, Deserialize)]
struct ListQuery {
    network: Option<String>,
    /// 属主过滤 (FR-3): 取值须为合法用户名 (白名单语义, 防任意串探测)。
    owner: Option<String>,
    page: Option<u32>,
    size: Option<u32>,
}

/// 解析分页边界 (四列表公共): page 从 1 起, size 有契约上限,
/// 越界即 400 + details, 不静默钳制。
fn page_of(q: &ListQuery) -> ApiResult<(u32, u32)> {
    let page = q.page.unwrap_or(1);
    let size = q.size.unwrap_or(PAGE_SIZE_DEFAULT);
    if page == 0 {
        return Err(ApiError::invalid_field("page", "page 从 1 起".into()));
    }
    if size == 0 || size > PAGE_SIZE_MAX {
        return Err(ApiError::invalid_field(
            "size",
            format!("size 须在 1..={PAGE_SIZE_MAX}"),
        ));
    }
    Ok((page, size))
}

/// 过滤参数白名单校验: 只在端点实际支持该过滤时调用,
/// 不给 "校验了却静默忽略" 留伪契约。
fn valid_filter(field: &str, value: Option<&str>) -> ApiResult<()> {
    if let Some(name) = value {
        if !valid_name(name) {
            return Err(ApiError::invalid_field(
                field,
                "仅允许 1-32 位字母数字与 - _".into(),
            ));
        }
    }
    Ok(())
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/v1/node/register", post(register))
        .route(
            "/api/v1/admin/networks",
            get(list_networks).post(create_network),
        )
        .route("/api/v1/admin/users", get(list_users).post(create_user))
        .route("/api/v1/admin/users/disable", post(disable_user))
        .route("/api/v1/admin/users/enable", post(enable_user))
        .route("/api/v1/admin/users/rename", post(rename_user))
        .route("/api/v1/admin/users/delete", post(delete_user))
        .route("/api/v1/admin/tokens", post(issue_token))
        .route("/api/v1/admin/tokens/{id}/revoke", post(revoke_token))
        .route("/api/v1/admin/login", post(login))
        .route("/api/v1/admin/logout", post(logout))
        .route("/api/v1/admin/relays", get(list_relays))
        .route("/api/v1/admin/nodes", get(list_nodes))
        .route("/api/v1/admin/nodes/remove", post(remove_node))
        .route("/api/v1/admin/nodes/policy", post(set_node_policy))
        // WS 不挂版本前缀: 协议消息自带演进语义 (只增字段), 无 URL 资源版本需求
        .route("/ws/signal", get(crate::signaling::ws_handler))
        .route("/ws/relay", get(crate::forward::core_ws_handler))
        .route("/ws/satellite", get(crate::satellite::ws_handler))
        // 控制台托管: 嵌入资源按路径命中, 页面路径回落入口页;
        // /api 与 /ws 未匹配保持 404 JSON (any: 任意方法都进 handler 判定,
        // 若用 get 则 POST 类未匹配路径在 MethodRouter 层被 405 拦截)
        .fallback_service(any(crate::console::serve_console))
        .layer(middleware::from_fn(request_context))
        .with_state(state)
}

/// 请求上下文中间件: request_id 生成 (响应头 + 错误体注入) 与访问日志
/// (outcome + duration); healthz 探活降为 debug 不刷 info。
pub(crate) async fn request_context(req: Request, next: Next) -> Response {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let started = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let request_id = format!("req-{ts:x}-{}", SEQ.fetch_add(1, Ordering::Relaxed));
    let mut resp = next.run(req).await;
    let status = resp.status();
    // 错误响应 (JSON) 注入 request_id: 覆盖 handler 错误与 axum 内建 404/405;
    // 只改写 body, 原样保留头部 (Retry-After 等), 缓冲上界 64KiB 防异常大响应
    if status.is_client_error() || status.is_server_error() {
        let is_json = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("application/json"));
        if is_json {
            let (mut parts, body) = resp.into_parts();
            // 重写后必须同步 content-length: 下游代理与 hyper 严格校验
            // "声明长度与实际载荷一致", 沿用旧头会造成链路断言失败
            let buffered = axum::body::to_bytes(body, 64 * 1024).await.ok();
            let new_body = buffered.map(|bytes| {
                let mut value = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
                match value.as_mut() {
                    Some(v) if v.is_object() => {
                        v["request_id"] = serde_json::json!(request_id);
                        serde_json::to_vec(v).unwrap_or_else(|_| bytes.to_vec())
                    }
                    _ => bytes.to_vec(),
                }
            });
            let body = match new_body {
                Some(b) => {
                    parts
                        .headers
                        .insert(header::CONTENT_LENGTH, HeaderValue::from(b.len()));
                    Body::from(b)
                }
                // 超过缓冲上界的错误体属异常形态: 退化为空体, 状态与其余头部保持
                None => {
                    parts
                        .headers
                        .insert(header::CONTENT_LENGTH, HeaderValue::from(0));
                    Body::empty()
                }
            };
            resp = Response::from_parts(parts, body);
        }
    }
    if let Ok(v) = HeaderValue::from_str(&request_id) {
        resp.headers_mut().insert("x-request-id", v);
    }
    // 探活降为 debug, 业务请求 info 留痕 (method/path/status/duration/outcome)
    if path == "/healthz" {
        tracing::debug!(
            %request_id,
            method = %method,
            path = %path,
            status = status.as_u16(),
            duration_ms = elapsed_ms(started),
            "http 请求完成"
        );
    } else {
        tracing::info!(
            %request_id,
            method = %method,
            path = %path,
            status = status.as_u16(),
            duration_ms = elapsed_ms(started),
            "http 请求完成"
        );
    }
    resp
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
        return Err(ApiError::too_many(state.admin_limiter.retry_after()));
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
            return Err(ApiError::too_many(state.admin_limiter.retry_after()));
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
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<RelayInfo>>> {
    require_admin(&state, addr.ip(), &headers)?;
    let (page, size) = page_of(&q)?;
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
    // 注册表内存态条目 (卫星数 + 内置), 按契约分页截取
    let total = relays.len() as u64;
    let items = relays
        .into_iter()
        .skip(((page as u64 - 1) * size as u64) as usize)
        .take(size as usize)
        .collect();
    Ok(Json(Page {
        items,
        page,
        size,
        total,
    }))
}

async fn register(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<Json<RegisterResponse>> {
    if !state.register_limiter.check(addr.ip()) {
        return Err(ApiError::too_many(state.register_limiter.retry_after()));
    }
    let started = Instant::now();
    let (network, ip, node_credential) =
        match state
            .lock_store()
            .consume_node_token(&req.token, &req.name, &req.wg_public_key)
        {
            Ok(out) => out,
            Err(e) => {
                // FR-4 / NFR-4: 留痕失败类别与来源, 不记录令牌 / 凭证明文;
                // 错误压平 400 防枚举 (不区分令牌状态)
                tracing::warn!(
                    ip = %addr.ip(),
                    node = %req.name,
                    error = %e,
                    duration_ms = elapsed_ms(started),
                    "节点注册被拒"
                );
                return Err(register_error(e));
            }
        };
    // 注意: 不记录任何令牌 / 凭证明文 (NFR-4)
    tracing::info!(
        node = %req.name,
        network = %network,
        ip = %ip,
        duration_ms = elapsed_ms(started),
        "节点注册成功"
    );
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

async fn list_users(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<UserInfo>>> {
    require_admin(&state, addr.ip(), &headers)?;
    let (page, size) = page_of(&q)?;
    valid_filter("network", q.network.as_deref())?;
    Ok(Json(state.lock_store().list_users(
        q.network.as_deref(),
        page,
        size,
    )?))
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

async fn enable_user(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    state.lock_store().enable_user(&req.network, &req.name)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn rename_user(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<RenameUserRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    state
        .lock_store()
        .rename_user(&req.network, &req.name, &req.new_name)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_user(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&state, addr.ip(), &headers)?;
    state.lock_store().delete_user(&req.network, &req.name)?;
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
    // 契约边界: 越界 400 + details, 保证 ttl 不进入 now()+ttl 溢出路径
    if !(TTL_MIN_SECS..=TTL_MAX_SECS).contains(&ttl) {
        return Err(ApiError::invalid_field(
            "ttl_secs",
            format!("ttl_secs 须在 {TTL_MIN_SECS}..={TTL_MAX_SECS} 秒"),
        ));
    }
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
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<NetworkInfo>>> {
    require_admin(&state, addr.ip(), &headers)?;
    let (page, size) = page_of(&q)?;
    Ok(Json(state.lock_store().list_networks(page, size)?))
}

async fn list_nodes(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<NodeInfo>>> {
    require_admin(&state, addr.ip(), &headers)?;
    let (page, size) = page_of(&q)?;
    valid_filter("network", q.network.as_deref())?;
    valid_filter("owner", q.owner.as_deref())?;
    let online = state.online.online_by_network();
    // owner 过滤已下推 SQL (先过滤后截页, 分页语义成立)
    let mut page_out =
        state
            .lock_store()
            .list_nodes(q.network.as_deref(), q.owner.as_deref(), page, size)?;
    // 候选地址、版本与路径快照来自信令注册表 (仅在线节点有意义), 按网络取一次
    let mut cand_cache: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    let mut ver_cache: HashMap<String, HashMap<String, Option<String>>> = HashMap::new();
    let mut path_cache: HashMap<String, HashMap<String, Vec<PeerPathReport>>> = HashMap::new();
    for n in &mut page_out.items {
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
    Ok(Json(page_out))
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
        return Err(ApiError::not_found(format!(
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
