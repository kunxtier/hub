//! Description: CLI 统一错误: 携带退出码, 由 main 收口输出。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

use std::fmt;

/// code 语义: 2 = 用法 / 配置错误, 1 = 运行失败
#[derive(Debug)]
pub struct Fail {
    pub code: i32,
    message: String,
}

impl Fail {
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
        }
    }
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
