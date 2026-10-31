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
use std::path::Path;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{OriginalUri, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{any, get};
use axum::Router;
use include_dir::{include_dir, Dir};

use kunxtier_core::protocol::ApiErrorBody;

use crate::Error;

/// 控制台构建产物 (cargo make build 时由 vite 清空并输出, outDir 清理归 vite)。
/// 资产随消费方走: 构建链将 web/kunxtier-console 的产物输出覆盖至 assets/console/,
/// 跨语言交接发生在构建链而非源码路径; 以文件表嵌入, 前端分包与命名策略不反向约束本侧。
static CONSOLE_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/console");

/// fresh clone (未构建前端) 的兜底占位页; 随二进制内嵌, 不依赖 assets 目录内容,
/// vite 清空 outDir 不影响。
const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>kunxtier 控制台</title>
<style>
  body { font-family: system-ui, -apple-system, sans-serif; background: #f5f6f8; color: #1f2328; display: flex; min-height: 100vh; align-items: center; justify-content: center; margin: 0; }
  main { text-align: center; }
  h1 { font-size: 20px; font-weight: 600; margin-bottom: 8px; }
  p { color: #57606a; font-size: 14px; margin: 4px 0; }
</style>
</head>
<body>
<main>
<h1>kunxtier 控制台</h1>
<p>前端构建产物未嵌入。</p>
<p>在 rust/kunxtier 执行 cargo make build 后重启进程即可。</p>
</main>
</body>
</html>
"#;

/// 按扩展名定 Content-Type; 未识别类型按 octet-stream 兜底。
fn mime_of(rel: &str) -> &'static str {
    match rel.rsplit('.').next().unwrap_or_default() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "json" | "map" => "application/json; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// 缓存策略: 构建器哈希产物集中在 assets/ 且内容寻址, 可长缓存;
/// 其余 (入口页与占位资源) 无 hash, 以 no-cache 随二进制版本刷新。
fn cache_of(rel: &str) -> &'static str {
    if rel.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

/// 控制台路径解析结果 (纯函数, 供单测直接驱动)。
/// rel 与 contents 均为静态嵌入数据的借用, 不产生分配。
enum ConsoleRoute {
    Asset {
        rel: &'static Path,
        contents: &'static [u8],
    },
    NotFound,
}

/// 解析控制台路径: 先按路径取嵌入文件; 无扩展名的未命中视为页面路径,
/// 回落入口页交给前端路由; 带扩展名的未命中资源明确 404;
/// 管理 API / WS 前缀不回落, 保持 404 JSON。
fn resolve(path: &str) -> ConsoleRoute {
    if path.starts_with("/api") || path.starts_with("/ws") {
        return ConsoleRoute::NotFound;
    }
    let rel = path.trim_start_matches('/');
    if !rel.is_empty() {
        if let Some(file) = CONSOLE_DIR.get_file(rel) {
            return ConsoleRoute::Asset {
                rel: file.path(),
                contents: file.contents(),
            };
        }
    }
    let last = rel.rsplit('/').next().unwrap_or_default();
    if last.contains('.') {
        return ConsoleRoute::NotFound;
    }
    // 页面路径回落入口页; 未构建状态 (fresh clone) 回落内嵌占位页
    match CONSOLE_DIR.get_file("index.html") {
        Some(file) => ConsoleRoute::Asset {
            rel: file.path(),
            contents: file.contents(),
        },
        None => ConsoleRoute::Asset {
            rel: Path::new("placeholder.html"),
            contents: PLACEHOLDER_HTML.as_bytes(),
        },
    }
}

fn asset_response(rel: &Path, contents: &'static [u8]) -> Response {
    // 嵌入产物路径均产自构建链, 非 UTF-8 时按无扩展名兜底即可
    let rel = rel.to_str().unwrap_or_default();
    (
        StatusCode::OK,
        [(CONTENT_TYPE, mime_of(rel)), (CACHE_CONTROL, cache_of(rel))],
        Bytes::from_static(contents),
    )
        .into_response()
}

fn not_found(path: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiErrorBody {
            code: "not_found".into(),
            message: format!("路径不存在: {path}"),
            details: Vec::new(),
            request_id: String::new(),
        }),
    )
        .into_response()
}

/// 控制台静态托管入口 (核心 fallback 与本地 console 子命令共用)。
pub async fn serve_console(uri: Uri) -> Response {
    match resolve(uri.path()) {
        ConsoleRoute::Asset { rel, contents } => asset_response(rel, contents),
        ConsoleRoute::NotFound => not_found(uri.path()),
    }
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
        // 与核心同源的请求上下文 (request_id / 访问日志), 本地控制台不留观测盲区
        .layer(axum::middleware::from_fn(crate::server::request_context))
        .fallback_service(get(serve_console))
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
        // 传输层失败只留日志: 错误串含内部实现细节, 不进响应体
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "反代核心不可达");
            proxy_error(
                StatusCode::BAD_GATEWAY,
                "core_unreachable",
                "核心中继不可达",
            )
        }
        Err(e) => {
            tracing::error!(error = %e, "反代任务异常");
            proxy_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "内部错误")
        }
    }
}

/// 反代错误出口: 与核心 REST 面同错误模型 (不含内部实现细节)。
fn proxy_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(ApiErrorBody {
            code: code.into(),
            message: message.into(),
            details: Vec::new(),
            request_id: String::new(),
        }),
    )
        .into_response()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rel_of(path: &str) -> Option<&'static str> {
        match resolve(path) {
            ConsoleRoute::Asset { rel, .. } => rel.to_str(),
            ConsoleRoute::NotFound => None,
        }
    }

    #[test]
    fn mime_按扩展名判定() {
        assert_eq!(mime_of("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            mime_of("assets/app-B1b2.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(mime_of("assets/app.css"), "text/css; charset=utf-8");
        assert_eq!(mime_of("assets/logo.svg"), "image/svg+xml");
        assert_eq!(mime_of("assets/app.wasm"), "application/wasm");
        assert_eq!(mime_of("assets/data.bin"), "application/octet-stream");
    }

    #[test]
    fn 缓存策略_哈希产物长缓存_入口页不缓存() {
        assert_eq!(
            cache_of("assets/app-B1b2.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_of("index.html"), "no-cache");
    }

    #[test]
    fn 解析_页面路径回落入口页() {
        // 入口页随构建状态取 index.html (已构建) 或内嵌占位页 (fresh clone),
        // 断言对两种目录形态都成立, 不依赖本机是否跑过前端构建
        for path in ["/", "/nodes", "/nodes/a/b"] {
            let rel = rel_of(path).expect("页面路径应有入口页回落");
            assert!(
                rel == "index.html" || rel == "placeholder.html",
                "意外入口: {rel}"
            );
        }
    }

    #[test]
    fn 解析_api与ws前缀及缺失资源不回落() {
        assert_eq!(rel_of("/api/v1/admin/nodes"), None);
        assert_eq!(rel_of("/ws/signal"), None);
        assert_eq!(rel_of("/assets/app.missing.js"), None);
        assert_eq!(rel_of("/favicon.ico"), None);
    }
}
