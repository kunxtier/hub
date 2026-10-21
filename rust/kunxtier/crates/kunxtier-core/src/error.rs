//! Description: 共享错误类型。
//!
//! Author: qinzhenya
//! Created: 2026-10-20

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("配置错误: {0}")]
    Config(String),
    #[error("地址错误: {0}")]
    Address(String),
    #[error("令牌错误: {0}")]
    Token(String),
}

pub type Result<T> = std::result::Result<T, Error>;
