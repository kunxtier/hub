//! Description: kunxtier 单一入口: 解析、装配与退出收口。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 当前切片: relay (init / run / backup / 卫星模式) 与 admin 管理;
//! daemon / up / down / status 在数据面阶段接入。

mod cli;

use clap::Parser;

use cli::Cli;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    // 退出收口: 子命令只返回 Fail, 退出码与错误输出在此统一产生
    if let Err(e) = cli.command.execute() {
        eprintln!("{e}");
        std::process::exit(e.code);
    }
}
