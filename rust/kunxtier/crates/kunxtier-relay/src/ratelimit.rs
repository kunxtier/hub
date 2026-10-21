//! Description: 极简固定窗口限速 (单机内存实现, NFR-9)。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 保护两类面: 节点注册尝试 (成功也计数) 与 admin 认证失败。
//! 已知妥协: 过期表项不主动清理, 由源数量自然有界 (单机个人规模可接受)。

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;

pub struct RateLimiter {
    max: u32,
    window: Duration,
    inner: Mutex<HashMap<IpAddr, (u64, u32)>>,
}

impl RateLimiter {
    pub fn new(max: u32, window: Duration) -> Self {
        RateLimiter {
            max,
            window,
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, ip: IpAddr) -> bool {
        self.check_at(ip, now_secs())
    }

    /// 以给定时间 (unix 秒) 判定, 供测试注入时钟。
    pub fn check_at(&self, ip: IpAddr, now: u64) -> bool {
        let w = self.window.as_secs().max(1);
        let slot = now / w;
        // unwrap 理由: 锁中毒意味着持锁线程已 panic, 计数表已不可信,
        // 快速失败优于以不可信计数继续判定
        let mut map = self.inner.lock().unwrap();
        let entry = map.entry(ip).or_insert((slot, 0));
        if entry.0 != slot {
            *entry = (slot, 1);
            return true;
        }
        if entry.1 >= self.max {
            return false;
        }
        entry.1 += 1;
        true
    }
}

fn now_secs() -> u64 {
    // unwrap 理由: 仅系统时钟早于 1970 时失败, 属不可达环境故障
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
    }

    #[test]
    fn allows_up_to_max_then_blocks() {
        let l = RateLimiter::new(2, Duration::from_secs(60));
        assert!(l.check_at(ip(), 0));
        assert!(l.check_at(ip(), 0));
        assert!(!l.check_at(ip(), 0));
    }

    #[test]
    fn window_slides() {
        let l = RateLimiter::new(2, Duration::from_secs(60));
        assert!(l.check_at(ip(), 0));
        assert!(l.check_at(ip(), 0));
        assert!(!l.check_at(ip(), 0));
        // 下一窗口重新计数
        assert!(l.check_at(ip(), 60));
    }

    #[test]
    fn sources_are_independent() {
        let l = RateLimiter::new(1, Duration::from_secs(60));
        let a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        assert!(l.check_at(a, 0));
        assert!(!l.check_at(a, 0));
        assert!(l.check_at(b, 0));
    }
}
