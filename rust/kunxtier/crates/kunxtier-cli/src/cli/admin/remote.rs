//! Description: admin API 客户端: 指纹固定 HTTPS 调用与结果输出。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

use serde_json::Value;

use kunxtier_core::protocol::PAGE_SIZE_MAX;
use kunxtier_core::tls::pinning_agent;

use crate::cli::error::Fail;

pub struct AdminRemote {
    agent: ureq::Agent,
    auth: String,
    base: String,
}

impl AdminRemote {
    pub fn new(server: &str, fingerprint: &str, credential: &str) -> Self {
        Self {
            agent: pinning_agent(fingerprint),
            auth: format!("Bearer {credential}"),
            base: format!("{}/api/v1/admin", server.trim_end_matches('/')),
        }
    }

    /// 列表查询: 按契约上限拉取 (个人规模单页足够) 并解包信封 items。
    pub fn get_list(&self, path: &str) -> Result<Value, Fail> {
        let sep = if path.contains('?') { '&' } else { '?' };
        // 与服务端契约上限同源 (protocol.rs 常量), 上限调整时无需同步此处
        let v = self.get(&format!("{path}{sep}size={PAGE_SIZE_MAX}"))?;
        match v.get("items").and_then(|i| i.as_array()) {
            Some(items) => Ok(Value::Array(items.clone())),
            None => Err(Fail::runtime("列表响应缺少 items 字段".to_string())),
        }
    }

    pub fn get(&self, path: &str) -> Result<Value, Fail> {
        let resp = self
            .agent
            .get(&format!("{}{path}", self.base))
            .set("Authorization", &self.auth)
            .call()
            .map_err(|e| Fail::runtime(format!("请求失败: {e}")))?;
        json_or_error(resp)
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value, Fail> {
        let resp = self
            .agent
            .post(&format!("{}{path}", self.base))
            .set("Authorization", &self.auth)
            .send_json(body.clone())
            .map_err(|e| Fail::runtime(format!("请求失败: {e}")))?;
        json_or_error(resp)
    }
}

fn json_or_error(resp: ureq::Response) -> Result<Value, Fail> {
    let status = resp.status();
    let body: Value = resp.into_json().unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        let msg = body["message"].as_str().unwrap_or("未知错误").to_string();
        let code = body["code"].as_str().unwrap_or("unknown");
        Err(Fail::runtime(format!("HTTP {status} [{code}]: {msg}")))
    }
}

pub fn print_result(v: &Value) {
    match v {
        Value::Array(items) => {
            if items.is_empty() {
                println!("(空)");
                return;
            }
            for it in items {
                match (
                    it["name"].as_str(),
                    it["cidr"].as_str(),
                    it["ip"].as_str(),
                    it["owner"].as_str(),
                ) {
                    (Some(n), Some(c), _, _) => println!("{n}\t{c}\tcreated {}", it["created_at"]),
                    (Some(n), _, Some(ip), owner) => {
                        let o = owner.unwrap_or("-");
                        let st = if it["online"].as_bool().unwrap_or(false) {
                            "online"
                        } else {
                            "offline"
                        };
                        println!("{n}\t{ip}\t{st}\towner {o}\tcreated {}", it["created_at"]);
                    }
                    _ => println!("{it}"),
                }
            }
        }
        _ => println!("{v}"),
    }
}
