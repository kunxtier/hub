//! Description: kunxtier 单一入口: 解析、装配与退出收口。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 当前切片: relay (init / run / backup / 卫星模式)、admin 管理、
//! 节点侧 daemon / up / down / status (含 --verbose 排障视图)、
//! 本地控制台 console 与服务化 install / uninstall (T14, 节点全局配置);
//! relay / daemon 支持 --log-level 与 --log-file (stderr / 文件双写)。

mod cli;
mod logging;

use clap::Parser;

use cli::Cli;

fn main() {
    let cli = Cli::parse();
    // 日志装配先于执行: relay 的配置文件层 log_level 由 log_spec 预读
    // (execute 内还会正式加载一次, TOML 体量小, 双读无害)
    let (spec, config_level) = cli.command.log_spec();
    if let Err(e) = logging::init(&spec, config_level.as_deref()) {
        eprintln!("{e}");
        std::process::exit(e.code);
    }
    // 退出收口: 子命令只返回 Fail, 退出码与错误输出在此统一产生
    if let Err(e) = cli.command.execute() {
        eprintln!("{e}");
        std::process::exit(e.code);
    }
}
