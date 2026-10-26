//! Description: 控制台静态托管: 嵌入页 SPA 回落与本地反代入口 (T11 占位, T12 换真实构建产物)。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 两种等价入口共用同一份嵌入资源 (feat-kunxtier-console): 核心中继以
//! fallback 托管; console 子命令在本地起明文 HTTP 并反代 admin API,
//! TLS 指纹固定收口在反代层, 浏览器仅连接本地进程, 无 CORS 面。

use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{OriginalUri, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{any, get};
use axum::Router;

use kunxtier_core::protocol::ApiErrorBody;

use crate::Error;

/// 控制台页面 (构建产物嵌入; 当前为占位页)。
/// 资产随消费方走: T12 前端构建任务 (cargo-make) 将 web/kunxtier-console
/// 的构建产物输出覆盖至 assets/console/, 跨语言交接发生在构建链而非源码路径。
pub const INDEX_HTML: &str = include_str!("../assets/console/index.html");

/// SPA fallback: 页面路径回落 index.html; 管理 API / WS 未匹配路径保持 404 JSON。
pub async fn spa_fallback(uri: Uri) -> Response {
    let path = uri.path();
    if path.starts_with("/api") || path.starts_with("/ws") {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody {
                error: "not_found".into(),
                message: format!("路径不存在: {path}"),
            }),
        )
            .into_response();
    }
    Html(INDEX_HTML).into_response()
}

/// 本地控制台装配参数。
pub struct ConsoleOptions {
    /// 核心中继地址 host:port (admin API 反代目标)。
    pub core: String,
    /// 核心证书指纹 hex (反代层 TLS 固定)。
    pub fingerprint: String,
    /// 本地监听地址 (明文 HTTP, 浏览器仅连本地)。
    pub listen: SocketAddr,
}

/// 起本地控制台: 静态托管 + admin API 反代; 返回实际监听地址与服务任务。
pub async fn start_local(
    options: ConsoleOptions,
) -> crate::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let agent = Arc::new(kunxtier_core::tls::pinning_agent(&options.fingerprint));
    let core = options.core;
    let app = Router::new()
        .route("/api/{*path}", any(proxy_api))
        .fallback_service(get(spa_fallback))
        .with_state((agent, core));
    let listener = std::net::TcpListener::bind(options.listen)
        .map_err(|e| Error::Serve(format!("本地控制台监听失败: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| Error::Serve(format!("读取本地监听地址失败: {e}")))?;
    let task = tokio::spawn(async move {
        // 服务异常 (端口被占等) 即退出任务, 由调用方观测
        if let Err(e) = axum_server::from_tcp(listener)
            .serve(app.into_make_service())
            .await
        {
            tracing::error!(error = %e, "本地控制台服务退出");
        }
    });
    Ok((addr, task))
}

/// 前台运行 (CLI 入口): 打印访问地址后驻留。
pub async fn run_local(options: ConsoleOptions) -> crate::Result<()> {
    let core = options.core.clone();
    let (addr, task) = start_local(options).await?;
    println!("控制台已就绪: http://{addr} (核心 {core}, TLS 指纹固定在反代层)");
    let _ = task.await;
    Ok(())
}

/// admin API 反代: 浏览器请求仅达本地明文端口, 核心侧由指纹固定的通道承载。
/// 仅覆盖 REST; 控制台不使用信令 / 转发 WS。
async fn proxy_api(
    State((agent, core)): State<(Arc<ureq::Agent>, String)>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/api/");
    let url = format!("https://{core}{path}");
    let auth = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    // 阻塞 ureq 转到线程池, 不占异步 worker (个人规模流量)
    let outcome = tokio::task::spawn_blocking(move || {
        let mut req = agent.request(method.as_str(), &url);
        if let Some(a) = auth {
            req = req.set("Authorization", &a);
        }
        if let Some(ct) = content_type {
            req = req.set("Content-Type", &ct);
        }
        req.send_bytes(&body)
    })
    .await;
    match outcome {
        Ok(Ok(resp)) | Ok(Err(ureq::Error::Status(_, resp))) => ureq_to_response(resp),
        Ok(Err(e)) => (StatusCode::BAD_GATEWAY, format!("核心中继不可达: {e}")).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("反代任务异常: {e}"),
        )
            .into_response(),
    }
}

/// ureq 应答转 axum 应答 (透传状态码 / Content-Type / 主体)。
fn ureq_to_response(resp: ureq::Response) -> Response {
    let status = StatusCode::from_u16(resp.status()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = resp
        .header("content-type")
        .unwrap_or("application/json")
        .to_string();
    let mut buf = Vec::new();
    let body = resp
        .into_reader()
        .read_to_end(&mut buf)
        .map(|_| buf)
        .unwrap_or_default();
    (status, [(CONTENT_TYPE, content_type)], body).into_response()
}
