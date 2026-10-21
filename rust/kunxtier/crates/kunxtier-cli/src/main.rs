//! Description: kunxtier 单一入口: 子命令装配各角色。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 当前切片: relay (init / run / backup) 与 admin 管理;
//! daemon / up / down / status 在数据面阶段接入。

mod admin;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use kunxtier_core::config::{CliOverrides, FileConfig, RelayConfig};

#[derive(Parser)]
#[command(name = "kunxtier", version, about = "轻量 P2P 虚拟组网 (ZeroTier / EasyTier 平替)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 运行核心中继 (管理 + 登记); --init 初始化状态目录
    Relay {
        /// 初始化: 自签证书 / admin 凭证 / 默认网络 (明文凭证仅展示一次)
        #[arg(long)]
        init: bool,
        /// 配置文件路径 (relay.toml)
        #[arg(long)]
        config: Option<PathBuf>,
        /// 监听地址 (覆盖 env / 配置文件)
        #[arg(long)]
        listen: Option<String>,
        /// 状态数据目录 (覆盖 env / 配置文件)
        #[arg(long)]
        data_dir: Option<String>,
        #[command(subcommand)]
        action: Option<RelayAction>,
    },
    /// 管理操作; admin 凭证走 env KUNXTIER_ADMIN
    Admin {
        #[arg(long)]
        server: String,
        #[arg(long, env = "KUNXTIER_FINGERPRINT")]
        fingerprint: String,
        #[command(subcommand)]
        action: AdminAction,
    },
}

#[derive(Subcommand)]
enum RelayAction {
    /// 导出一致性备份 (SQLite 快照)
    Backup { out: PathBuf },
}

#[derive(Subcommand)]
pub enum AdminAction {
    /// 网络组管理
    Network {
        #[command(subcommand)]
        action: NetworkAction,
    },
    /// 用户管理
    User {
        #[command(subcommand)]
        action: UserAction,
    },
    /// 令牌管理
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
    /// 节点管理
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },
}

// 子命令组按领域嵌套 (network create / user disable 等),
// 与 .docs 需求文档 (feat-kunxtier-cli) 约定的 CLI 形态一致

#[derive(Subcommand)]
pub enum NetworkAction {
    /// 网络组列表
    List,
    /// 创建网络组
    Create {
        name: String,
        /// 形如 100.66.0.0/16; 缺省按创建顺序取下一个 /16
        #[arg(long)]
        cidr: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum UserAction {
    /// 在网络内创建用户
    Create {
        #[arg(long)]
        network: String,
        #[arg(long)]
        name: String,
    },
    /// 禁用用户 (其名下令牌全部失效)
    Disable {
        #[arg(long)]
        network: String,
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
pub enum TokenAction {
    /// 签发令牌 (默认一次性节点令牌; --type relay 为可复用卫星令牌)
    Issue {
        #[arg(long)]
        network: String,
        #[arg(long)]
        user: Option<String>,
        #[arg(long = "type")]
        r#type: Option<String>,
        /// 有效期秒数
        #[arg(long)]
        ttl: Option<u64>,
    },
    /// 撤销令牌
    Revoke { id: i64 },
}

#[derive(Subcommand)]
pub enum NodeAction {
    /// 节点概览 (含属主 / 在线状态)
    List {
        #[arg(long)]
        network: Option<String>,
    },
    /// 移除节点 (凭证随之失效)
    Remove {
        #[arg(long)]
        network: String,
        #[arg(long)]
        name: String,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Relay {
            init,
            config,
            listen,
            data_dir,
            action,
        } => run_relay(init, config, listen, data_dir, action),
        Command::Admin {
            server,
            fingerprint,
            action,
        } => {
            let credential = std::env::var("KUNXTIER_ADMIN").unwrap_or_else(|_| {
                eprintln!("缺少 admin 凭证: 请设置环境变量 KUNXTIER_ADMIN (见 relay --init 输出)");
                std::process::exit(2);
            });
            admin::dispatch(&server, &fingerprint, &credential, action);
        }
    }
}

fn run_relay(
    init: bool,
    config: Option<PathBuf>,
    listen: Option<String>,
    data_dir: Option<String>,
    action: Option<RelayAction>,
) {
    let file = match config.as_deref() {
        Some(p) => match FileConfig::load(p) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("配置文件加载失败: {e}");
                std::process::exit(2);
            }
        },
        None => FileConfig::default(),
    };
    let cfg = match RelayConfig::resolve(
        CliOverrides { listen, data_dir },
        |k| std::env::var(k).ok(),
        file,
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("配置解析失败: {e}");
            std::process::exit(2);
        }
    };

    if let Some(RelayAction::Backup { out }) = action {
        match kunxtier_relay::store::Store::open(&cfg.db_path()).and_then(|s| s.backup_to(&out)) {
            Ok(()) => println!("备份完成: {}", out.display()),
            Err(e) => {
                eprintln!("备份失败: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if init {
        match kunxtier_relay::init(&cfg) {
            Ok(out) => {
                println!("初始化完成");
                println!("  状态目录: {}", out.data_dir);
                println!();
                println!("  admin 凭证 (仅此一次展示, 妥善保存):");
                println!("    {}", out.admin_credential);
                println!();
                println!("  服务端证书指纹 (客户端固定使用):");
                println!("    {}", out.fingerprint);
                println!();
                println!("  启动: kunxtier relay");
                println!(
                    "  管理: KUNXTIER_ADMIN=<上面的 admin 凭证> kunxtier admin --server https://<addr> --fingerprint {} network list",
                    out.fingerprint
                );
            }
            Err(e) => {
                eprintln!("初始化失败: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // expect 理由: 仅在系统线程资源耗尽时失败, 属环境级故障, 无恢复价值
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    if let Err(e) = rt.block_on(kunxtier_relay::server::run(&cfg)) {
        eprintln!("核心中继退出: {e}");
        std::process::exit(1);
    }
}
