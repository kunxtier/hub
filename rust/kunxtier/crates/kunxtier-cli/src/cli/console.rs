//! Description: console 子命令: 本地托管控制台页面并反代核心 admin API。
//!
//! Author: qinzhenya
//! Created: 2026-10-26

use clap::Args;

use kunxtier_relay::console::{run_local, ConsoleOptions};

use super::error::Fail;

/// 缺省本地监听。
const DEFAULT_LISTEN: &str = "127.0.0.1:7600";

#[derive(Args)]
pub struct ConsoleArgs {
    /// 核心中继地址 host:port
    #[arg(long, env = "KUNXTIER_CORE")]
    pub relay: String,
    /// 核心证书指纹 hex (relay --init 输出)
    #[arg(long, env = "KUNXTIER_FINGERPRINT")]
    pub fingerprint: String,
    /// 本地监听地址 (缺省 127.0.0.1:7600)
    #[arg(long)]
    pub listen: Option<String>,
}

impl ConsoleArgs {
    pub fn execute(self) -> Result<(), Fail> {
        let listen = self
            .listen
            .unwrap_or_else(|| DEFAULT_LISTEN.into())
            .parse()
            .map_err(|e| Fail::usage(format!("listen 不是合法地址: {e}")))?;
        let options = ConsoleOptions {
            core: self
                .relay
                .trim_start_matches("https://")
                .trim_end_matches('/')
                .to_string(),
            fingerprint: self.fingerprint,
            listen,
        };
        // expect 理由: 仅在系统线程资源耗尽时失败, 属环境级故障, 无恢复价值
        let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
        rt.block_on(run_local(options))
            .map_err(|e| Fail::runtime(format!("本地控制台退出: {e}")))
    }
}
