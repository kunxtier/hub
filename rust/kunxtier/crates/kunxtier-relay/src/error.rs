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
    /// 目标资源不存在: HTTP 404 (admin 面; 节点注册等防枚举面由 handler 压平)。
    #[error("{0}")]
    NotFound(String),
    /// 状态冲突 (重名 / 已存在 / 状态不允许): HTTP 409。
    #[error("{0}")]
    Conflict(String),
    /// 令牌生命周期拒绝 (无效 / 类型不符 / 已用 / 已撤销 / 已过期):
    /// 日志按此留痕区分失败类别, 响应面统一压平防枚举 (NFR-4), 不直接映射 HTTP。
    #[error("{0}")]
    TokenRejected(String),
}

pub type Result<T> = std::result::Result<T, Error>;
