//! Description: 时长打点工具: 关键路径 duration_ms 日志字段的统一换算口径。
//!
//! Author: qinzhenya
//! Created: 2026-10-31

use std::time::Instant;

/// 自 `since` 起的耗时毫秒数; 各打点点位统一经此换算, 避免散写 cast 造成单位漂移。
pub fn elapsed_ms(since: Instant) -> u64 {
    since.elapsed().as_millis() as u64
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_ms_is_zero_at_start_and_grows_monotonically() {
        let t = Instant::now();
        assert_eq!(elapsed_ms(t), 0);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(elapsed_ms(t) >= 5);
    }
}
