//! Description: 节点一次性令牌注册客户端 (核心 /api/v1/node/register)。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 同步 ureq 调用 (调用方置于 spawn_blocking); 令牌与节点凭证明文
//! 只在参数与返回值中流动, 不入日志 (NFR-4)。指纹固定的 TLS 客户端
//! 装配复用 kunxtier-core::tls::pinning_agent。

use serde::Deserialize;

use crate::error::{Error, Result};

/// 注册成功结果。
#[derive(Debug, Clone)]
pub struct RegisterOutcome {
    pub network: String,
    /// 分配的本节点虚拟 IP。
    pub ip: String,
    /// 节点凭证明文, 仅此一次返回。
    pub credential: String,
}

#[derive(Deserialize)]
struct RegisterReply {
    network: String,
    ip: String,
    node_credential: String,
}

/// 以一次性令牌向核心注册; wg_public_key_base64 为本节点 WG 公钥编码。
/// 重复消费 / 令牌失效 / 名称冲突均以服务端 message 报错。
pub fn register(
    core: &str,
    fingerprint: &str,
    token: &str,
    name: &str,
    wg_public_key_base64: &str,
) -> Result<RegisterOutcome> {
    let agent = kunxtier_core::tls::pinning_agent(fingerprint);
    let host = core.trim_start_matches("https://").trim_end_matches('/');
    let url = format!("https://{host}/api/v1/node/register");
    let resp = match agent.post(&url).send_json(serde_json::json!({
        "token": token,
        "name": name,
        "wg_public_key": wg_public_key_base64,
    })) {
        Ok(resp) => resp,
        // 非 2xx 在 ureq 2 以 Err(Status) 形态返回, 服务端 message 必须带出
        Err(ureq::Error::Status(status, resp)) => {
            let message = resp
                .into_json::<serde_json::Value>()
                .ok()
                .and_then(|v| v["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| "未知错误".into());
            return Err(Error::Transport(format!(
                "注册被拒 (HTTP {status}): {message}"
            )));
        }
        Err(e) => return Err(Error::Transport(format!("注册请求失败: {e}"))),
    };
    let reply: RegisterReply = resp
        .into_json()
        .map_err(|e| Error::Transport(format!("注册响应解析失败: {e}")))?;
    Ok(RegisterOutcome {
        network: reply.network,
        ip: reply.ip,
        credential: reply.node_credential,
    })
}
