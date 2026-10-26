//! Description: admin 会话: 登录签发、校验与登出 (内存表, 重启即失效)。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 控制台登录用 (feat-kunxtier-console FR-6): 会话令牌为 Bearer 形态,
//! 与 admin 凭证双轨并行 (CLI 沿用凭证); 令牌高熵随机 (kxt_ses_ 前缀),
//! 过期惰性清除, 不落任何存储, 也不入日志 (NFR-4)。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kunxtier_core::token::{self, Kind};

/// 会话有效期 (12h, ik-plan T11 裁决)。
pub const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);

pub struct SessionTable {
    ttl: Duration,
    inner: Mutex<HashMap<String, Instant>>,
}

impl SessionTable {
    pub fn new(ttl: Duration) -> Self {
        SessionTable {
            ttl,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// 签发会话: 返回 (令牌, 过期时间 unix 秒)。
    pub fn issue(&self) -> (String, i64) {
        let token = token::generate(Kind::Session);
        // unwrap 理由: 锁中毒即会话表不可信, 快速失败优于带病鉴权
        self.inner
            .lock()
            .unwrap()
            .insert(token.clone(), Instant::now() + self.ttl);
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            + self.ttl.as_secs() as i64;
        (token, expires_at)
    }

    /// 校验会话; 过期惰性清除 (不触发后台清扫)。
    pub fn verify(&self, token: &str) -> bool {
        let mut map = self.inner.lock().unwrap();
        match map.get(token) {
            Some(expires) if *expires > Instant::now() => true,
            Some(_) => {
                map.remove(token);
                false
            }
            None => false,
        }
    }

    /// 登出: 移除会话 (幂等, 返回是否存在)。
    pub fn revoke(&self, token: &str) -> bool {
        self.inner.lock().unwrap().remove(token).is_some()
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_verify_revoke_lifecycle() {
        let table = SessionTable::new(SESSION_TTL);
        let (token, expires_at) = table.issue();
        assert!(token.starts_with("kxt_ses_"));
        // 过期时间在 TTL 窗口内 (允许取时间时的微小漂移)
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!((now + 12 * 3600 - 5..=now + 12 * 3600 + 5).contains(&expires_at));

        assert!(table.verify(&token));
        assert!(table.revoke(&token));
        assert!(!table.verify(&token), "登出后应失效");
        assert!(!table.revoke(&token), "登出幂等");
    }

    #[test]
    fn multiple_sessions_coexist_and_expire_lazily() {
        let table = SessionTable::new(Duration::from_millis(50));
        let (t1, _) = table.issue();
        let (t2, _) = table.issue();
        assert!(table.verify(&t1) && table.verify(&t2), "多会话并存");

        std::thread::sleep(Duration::from_millis(80));
        assert!(!table.verify(&t1), "过期会话应拒绝");
        assert!(!table.verify(&t2));
        assert!(!table.revoke(&t1), "过期即清除, 无残留");
    }
}
