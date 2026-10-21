//! Description: kunxtier-relay 共享错误类型与 Result 别名。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("存储错误: {0}")]
    Store(#[from] rusqlite::Error),
    #[error(transparent)]
    Core(#[from] kunxtier_core::error::Error),
    #[error("证书错误: {0}")]
    Cert(String),
    #[error("服务错误: {0}")]
    Serve(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Usage(String),
}

pub type Result<T> = std::result::Result<T, Error>;
