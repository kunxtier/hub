//! Description: admin 子命令: 通过指纹固定的 HTTPS 调用核心中继 admin API。
//!
//! Author: qinzhenya
//! Created: 2026-10-20

use serde_json::{json, Value};

use kunxtier_relay::client_tls::pinning_agent;

use crate::{AdminAction, NetworkAction, NodeAction, TokenAction, UserAction};

pub fn dispatch(server: &str, fingerprint: &str, credential: &str, action: AdminAction) {
    let agent = pinning_agent(fingerprint);
    let auth = format!("Bearer {credential}");
    let base = format!("{}/api/admin", server.trim_end_matches('/'));

    let result: Result<Value, String> = match action {
        AdminAction::Network { action } => match action {
            NetworkAction::List => get_json(&agent, &format!("{base}/networks"), &auth),
            NetworkAction::Create { name, cidr } => {
                let mut body = json!({"name": name});
                if let Some(c) = cidr {
                    body["cidr"] = Value::String(c);
                }
                post_json(&agent, &format!("{base}/networks"), &auth, &body)
            }
        },
        AdminAction::User { action } => match action {
            UserAction::Create { network, name } => post_json(
                &agent,
                &format!("{base}/users"),
                &auth,
                &json!({"network": network, "name": name}),
            ),
            UserAction::Disable { network, name } => post_json(
                &agent,
                &format!("{base}/users/disable"),
                &auth,
                &json!({"network": network, "name": name}),
            ),
        },
        AdminAction::Token { action } => match action {
            TokenAction::Issue {
                network,
                user,
                r#type,
                ttl,
            } => {
                let mut body = json!({"network": network});
                if let Some(u) = user {
                    body["user"] = Value::String(u);
                }
                if let Some(t) = r#type {
                    body["kind"] = Value::String(t);
                }
                if let Some(t) = ttl {
                    body["ttl_secs"] = json!(t);
                }
                post_json(&agent, &format!("{base}/tokens"), &auth, &body)
            }
            TokenAction::Revoke { id } => post_json(
                &agent,
                &format!("{base}/tokens/{id}/revoke"),
                &auth,
                &json!({}),
            ),
        },
        AdminAction::Node { action } => match action {
            NodeAction::List { network } => {
                let url = match &network {
                    Some(n) => format!("{base}/nodes?network={n}"),
                    None => format!("{base}/nodes"),
                };
                get_json(&agent, &url, &auth)
            }
            NodeAction::Remove { network, name } => post_json(
                &agent,
                &format!("{base}/nodes/remove"),
                &auth,
                &json!({"network": network, "name": name}),
            ),
        },
    };

    match result {
        Ok(v) => {
            print_result(&v);
            if v.get("token").is_some() {
                println!("提示: 令牌明文仅此一次展示, 请立即分发给使用者");
            }
        }
        Err(e) => {
            eprintln!("操作失败: {e}");
            std::process::exit(1);
        }
    }
}

fn get_json(agent: &ureq::Agent, url: &str, auth: &str) -> Result<Value, String> {
    let resp = agent
        .get(url)
        .set("Authorization", auth)
        .call()
        .map_err(|e| format!("请求失败: {e}"))?;
    json_or_error(resp)
}

fn post_json(agent: &ureq::Agent, url: &str, auth: &str, body: &Value) -> Result<Value, String> {
    let resp = agent
        .post(url)
        .set("Authorization", auth)
        .send_json(body.clone())
        .map_err(|e| format!("请求失败: {e}"))?;
    json_or_error(resp)
}

fn json_or_error(resp: ureq::Response) -> Result<Value, String> {
    let status = resp.status();
    let body: Value = resp.into_json().unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        let msg = body["message"].as_str().unwrap_or("未知错误").to_string();
        Err(format!("HTTP {status}: {msg}"))
    }
}

fn print_result(v: &Value) {
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
