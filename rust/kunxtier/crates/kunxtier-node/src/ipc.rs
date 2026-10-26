//! Description: 本地 IPC: daemon 服务端与 up/down/status 客户端的协议与传输。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 传输为 state 目录内的 Unix domain socket (Windows named pipe 随
//! Windows 平台切片落地, 接口形态不变); 目录 0700 即信任边界, 限定
//! 本机当前用户。协议为单连接单请求: 一行 JSON 进、一行 JSON 出。
//! 令牌 / 凭证只在帧内流动, 不入日志。

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// IPC socket 文件名 (位于 state 目录内)。
pub const IPC_SOCKET: &str = "daemon.sock";
/// 单行帧上限: status 快照远小于此, 超限视为异常客户端直接断开。
const MAX_LINE: usize = 64 * 1024;

pub fn socket_path(state_dir: &Path) -> PathBuf {
    state_dir.join(IPC_SOCKET)
}

// ------------------------------ 协议 ------------------------------

/// 客户端 → daemon。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum IpcRequest {
    /// 首次注册并连接; 令牌一次性。已有 state 时 core / fingerprint
    /// 可省略 (取自 state), 提供且不一致则报错。
    Up {
        token: String,
        name: String,
        core: Option<String>,
        fingerprint: Option<String>,
    },
    /// 断开网络会话 (daemon 留守, state 保留)。
    Down,
    /// 状态快照。
    Status,
}

/// daemon → 客户端。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// up 成功时的归属信息。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// status 成功时的快照。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<IpcStatus>,
}

impl IpcResponse {
    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            ip: None,
            network: None,
            status: None,
        }
    }

    pub fn up(ip: String, network: String) -> Self {
        Self {
            ok: true,
            error: None,
            ip: Some(ip),
            network: Some(network),
            status: None,
        }
    }

    pub fn ack() -> Self {
        Self {
            ok: true,
            error: None,
            ip: None,
            network: None,
            status: None,
        }
    }

    pub fn status(status: IpcStatus) -> Self {
        Self {
            ok: true,
            error: None,
            ip: None,
            network: None,
            status: Some(status),
        }
    }
}

/// status 快照 (自 daemon 视角聚合, 下线时对端表为空)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcStatus {
    pub online: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fatal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own_ip: Option<String>,
    /// auto / force_relay (策略未下达前为 None)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    pub peers: Vec<IpcPeer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcPeer {
    pub name: String,
    pub ip: String,
    /// idle / punching / direct / relayed。
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

// ------------------------------ 编解码 ------------------------------

/// 读一行 (含换行符剥离); 超上限或对流断开返回 None。
pub async fn read_frame<S>(stream: &mut S) -> io::Result<Option<String>>
where
    S: AsyncRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::with_capacity(512);
    let mut chunk = [0u8; 4096];
    loop {
        let n = match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if let Some(pos) = chunk[..n].iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&chunk[..pos]);
            if buf.len() > MAX_LINE {
                return Ok(None);
            }
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_LINE {
            return Ok(None);
        }
    }
    if buf.is_empty() {
        Ok(None)
    } else {
        Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
    }
}

/// 写一行 (补换行并冲刷)。
pub async fn write_frame<S, T: serde::Serialize>(stream: &mut S, value: &T) -> io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_vec(value).map_err(|e| io::Error::other(e.to_string()))?;
    line.push(b'\n');
    stream.write_all(&line).await?;
    stream.flush().await
}

// ------------------------------ 传输 ------------------------------

/// 绑定 IPC 服务端; 残留 socket (上次进程异常退出) 自动清理,
/// 活着的 daemon 则拒绝重复启动。
pub async fn bind(path: &Path) -> io::Result<tokio::net::UnixListener> {
    #[cfg(unix)]
    {
        if path.exists() {
            // 探活: 能连上说明 daemon 在运行; 连不上是残留文件
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(_) => {
                    return Err(io::Error::other(format!(
                        "daemon 已在运行 ({} 存在活连接)",
                        path.display()
                    )))
                }
                Err(_) => std::fs::remove_file(path)?,
            }
        }
        tokio::net::UnixListener::bind(path)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "本地 IPC 当前仅支持 Unix domain socket, Windows named pipe 随平台切片落地",
        ))
    }
}

/// 单请求往返: 连接 → 发送 → 读取应答。daemon 未运行时以
/// NotFound / ConnectionRefused 形态报错, 由 CLI 转成启动指引。
pub async fn request(path: &Path, req: &IpcRequest) -> io::Result<IpcResponse> {
    #[cfg(unix)]
    {
        let mut stream = tokio::net::UnixStream::connect(path).await?;
        write_frame(&mut stream, req).await?;
        let line = read_frame(&mut stream)
            .await?
            .ok_or_else(|| io::Error::other("daemon 连接中断: 未收到应答"))?;
        serde_json::from_str(&line).map_err(|e| io::Error::other(format!("应答解析失败: {e}")))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, req);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "本地 IPC 当前仅支持 Unix domain socket",
        ))
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_roundtrips_request_and_response() {
        let req = IpcRequest::Up {
            token: "t".into(),
            name: "laptop".into(),
            core: Some("core:443".into()),
            fingerprint: None,
        };
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, &req).await.unwrap();
        let line = String::from_utf8(buf).unwrap();
        assert!(line.ends_with('\n'));
        let parsed: IpcRequest = serde_json::from_str(line.trim_end()).unwrap();
        match parsed {
            IpcRequest::Up {
                token,
                name,
                core,
                fingerprint,
            } => {
                assert_eq!(token, "t");
                assert_eq!(name, "laptop");
                assert_eq!(core.as_deref(), Some("core:443"));
                assert!(fingerprint.is_none());
            }
            other => panic!("期望 Up, 实际 {other:?}"),
        }

        let resp = IpcResponse::status(IpcStatus {
            online: true,
            offline_reason: None,
            fatal: None,
            network: Some("default".into()),
            name: Some("laptop".into()),
            own_ip: Some("100.64.0.5".into()),
            policy: Some("auto".into()),
            peers: vec![IpcPeer {
                name: "peer".into(),
                ip: "100.64.0.6".into(),
                path: "direct".into(),
                endpoint: Some("1.2.3.4:5".into()),
            }],
        });
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, &resp).await.unwrap();
        let back: IpcResponse =
            serde_json::from_str(String::from_utf8(buf).unwrap().trim_end()).unwrap();
        assert!(back.ok);
        assert_eq!(back.status.unwrap().peers.len(), 1);
    }

    #[tokio::test]
    async fn oversized_line_is_rejected() {
        let payload = format!("{{\"pad\":\"{}\"", "x".repeat(MAX_LINE + 16));
        let mut stream = std::io::Cursor::new(payload.into_bytes());
        let frame = read_frame(&mut stream).await.unwrap();
        assert!(frame.is_none(), "超限帧应返回 None");
    }

    #[tokio::test]
    async fn bind_serves_request_over_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_path(dir.path());
        let listener = bind(&path).await.unwrap();
        let path2 = path.clone();
        tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let line = read_frame(&mut conn).await.unwrap().unwrap();
            let req: IpcRequest = serde_json::from_str(&line).unwrap();
            let resp = match req {
                IpcRequest::Status => IpcResponse::status(IpcStatus {
                    online: false,
                    offline_reason: None,
                    fatal: None,
                    network: None,
                    name: None,
                    own_ip: None,
                    policy: None,
                    peers: Vec::new(),
                }),
                _ => IpcResponse::err("不支持的请求"),
            };
            write_frame(&mut conn, &resp).await.unwrap();
        });
        let resp = request(&path2, &IpcRequest::Status).await.unwrap();
        assert!(resp.ok);
        assert!(!resp.status.unwrap().online);
    }
}
