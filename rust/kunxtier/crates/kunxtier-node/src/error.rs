//! Description: kunxtier-node 错误类型。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

use gotatun::device::Error as EngineError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("WG 引擎错误: {0}")]
    Engine(#[from] EngineError),

    #[error("传输错误: {0}")]
    Transport(String),

    #[error("密钥错误: {0}")]
    Key(String),

    #[error("参数错误: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, Error>;
