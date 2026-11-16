//! Description: config 子命令: 配置文件位置感知、生效配置打印与默认模板生成 (FR-12)。
//!
//! Author: qinzhenya
//! Created: 2026-11-09
//!
//! 终值与实际运行装配同源 (复用 resolve 与同一 env 读取), 保证所见即所跑;
//! 敏感字段 (relay 令牌 / admin 凭证) 只输出存在性, 不回显内容。
//! 裸命令只读不落盘; 显式 init 才生成缺失的默认模板 (巡检命令不携带 /etc 副作用写)。

use std::path::Path;

use clap::{Args, Subcommand};

use kunxtier_core::config::{
    node_config_path, relay_config_path, satellite_mode, FileConfig, NodeFileConfig,
    NodeGlobalConfig, RelayConfig, SatelliteConfig, ENV_RELAY_TOKEN,
};
use kunxtier_core::error::Error as CoreError;

use super::error::Fail;

/// admin 凭证 env 键 (与 admin 子命令字面一致, core 未单列常量)。
const ENV_ADMIN: &str = "KUNXTIER_ADMIN";

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    action: Option<ConfigAction>,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// 生成缺失的默认配置文件 (已存在不改写; 机器级路径通常需 sudo)
    Init {
        /// 只生成 relay 域 (relay.toml)
        #[arg(long)]
        relay: bool,
        /// 只生成节点域 (config.toml)
        #[arg(long)]
        node: bool,
    },
}

impl ConfigArgs {
    pub fn execute(self) -> Result<(), Fail> {
        match self.action {
            Some(ConfigAction::Init { relay, node }) => init_files(relay, node),
            None => inspect(),
        }
    }
}

/// 只读打印生效配置。
fn inspect() -> Result<(), Fail> {
    // 装配失败口径与 relay / daemon 运行一致: 非法配置在此即报错
    let relay_file =
        FileConfig::discover().map_err(|e| Fail::usage(format!("relay 配置加载失败: {e}")))?;
    let node = NodeGlobalConfig::load(|k| std::env::var(k).ok())
        .map_err(|e| Fail::usage(format!("全局配置读取失败: {e}")))?;
    let text = render(&|k| std::env::var(k).ok(), relay_file, &node)?;
    print!("{text}");
    Ok(())
}

/// init 标志归一: 都不选 = 两域都生成。
fn init_targets(relay: bool, node: bool) -> (bool, bool) {
    match (relay, node) {
        (false, false) => (true, true),
        flags => flags,
    }
}

/// 生成各域缺失的默认模板并打印结果。
fn init_files(relay: bool, node: bool) -> Result<(), Fail> {
    let (do_relay, do_node) = init_targets(relay, node);
    if do_relay {
        init_domain(&relay_config_path(), FileConfig::ensure_written)?;
    }
    if do_node {
        init_domain(&node_config_path(), NodeFileConfig::ensure_written)?;
    }
    Ok(())
}

/// 单域落盘: 权限不足映射提权指引, 其余错误带路径上下文。
fn init_domain(
    path: &Path,
    ensure: fn(&Path) -> kunxtier_core::error::Result<bool>,
) -> Result<(), Fail> {
    match ensure(path) {
        Ok(true) => {
            println!("已生成默认模板: {}", path.display());
            Ok(())
        }
        Ok(false) => {
            println!("已存在, 不改动: {}", path.display());
            Ok(())
        }
        Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(Fail::usage(format!(
                "权限不足: 写入 {} 需要相应权限 (机器级路径通常以 sudo 执行)",
                path.display()
            )))
        }
        Err(e) => Err(Fail::usage(format!(
            "配置生成失败 ({}): {e}",
            path.display()
        ))),
    }
}

/// 生成报告文本; env / 文件层由调用方注入 (测试可造合成层)。
fn render(
    env: &impl Fn(&str) -> Option<String>,
    file: FileConfig,
    node: &NodeGlobalConfig,
) -> Result<String, Fail> {
    let non_empty = |key: &str| env(key).filter(|v| !v.is_empty()).is_some();
    let mut out = String::new();
    out.push_str("配置文件\n");
    out.push_str(&format!(
        "  relay: {} {}\n",
        relay_config_path().display(),
        state_label(&relay_config_path())
    ));
    out.push_str(&format!(
        "  节点: {} {}\n",
        node_config_path().display(),
        state_label(&node_config_path())
    ));
    // 缺失不阻断读取 (全默认可用), 但给出显式生成入口
    if !relay_config_path().exists() || !node_config_path().exists() {
        out.push_str("\n提示: 缺失的配置文件可用 `kunxtier config init` 生成默认模板\n");
    }

    if satellite_mode(env) {
        // 卫星模式: 与 relay 子命令同一判定与解析路径
        let cfg = SatelliteConfig::resolve(env, file)
            .map_err(|e| Fail::usage(format!("卫星配置解析失败: {e}")))?;
        out.push_str("\nrelay 域 (KUNXTIER_RELAY_TOKEN 已设置, 卫星模式)\n");
        out.push_str(&format!("  core:        {}\n", cfg.core));
        out.push_str(&format!("  listen:      {}\n", cfg.listen));
        out.push_str(&format!("  advertise:   {}\n", cfg.advertise));
        out.push_str(&format!("  fingerprint: {}\n", cfg.fingerprint));
    } else {
        let cfg = RelayConfig::resolve(env, file)
            .map_err(|e| Fail::usage(format!("配置解析失败: {e}")))?;
        out.push_str("\nrelay 域 (核心模式)\n");
        out.push_str(&format!("  listen:    {}\n", cfg.listen));
        out.push_str(&format!("  data-dir:  {}\n", cfg.data_dir.display()));
        match cfg.advertise {
            Some(addr) => out.push_str(&format!("  advertise: {addr}\n")),
            None => out.push_str("  advertise: (未设置)\n"),
        }
        out.push_str(&format!("  stun:      {}\n", cfg.stun_servers.join(", ")));
    }

    out.push_str("\n节点域\n");
    out.push_str(&format!("  state-dir: {}\n", node.state_dir.display()));
    out.push_str(&format!("  log-dir:   {}\n", node.log_dir.display()));
    match &node.log_level {
        Some(level) => out.push_str(&format!("  log-level: {level}\n")),
        None => out.push_str("  log-level: (未设置)\n"),
    }

    out.push_str("\n敏感 env (只显示存在性)\n");
    out.push_str(&format!(
        "  KUNXTIER_RELAY_TOKEN: {}\n",
        presence(non_empty(ENV_RELAY_TOKEN))
    ));
    out.push_str(&format!(
        "  KUNXTIER_ADMIN:       {}\n",
        presence(non_empty(ENV_ADMIN))
    ));
    Ok(out)
}

fn presence(set: bool) -> &'static str {
    if set {
        "已设置"
    } else {
        "未设置"
    }
}

fn state_label(path: &Path) -> &'static str {
    if path.exists() {
        "(存在)"
    } else {
        "(缺失)"
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kunxtier_core::config::{
        NodeFileConfig, ENV_CORE, ENV_FINGERPRINT, ENV_LISTEN, ENV_NODE_STATE_DIR,
    };

    #[test]
    fn render_masks_token_and_shows_merged_values() {
        let secret = "kxt_relay_secret_value";
        let env = |k: &str| match k {
            ENV_RELAY_TOKEN => Some(secret.to_string()),
            ENV_CORE => Some("5.6.7.8:9443".to_string()),
            ENV_FINGERPRINT => Some("ab".repeat(32)),
            ENV_LISTEN => Some("127.0.0.1:8443".to_string()),
            ENV_NODE_STATE_DIR => Some("./dev-state".to_string()),
            _ => None,
        };
        let node = NodeGlobalConfig::resolve(&env, NodeFileConfig::default());
        let text = render(&env, FileConfig::default(), &node).unwrap();
        // 敏感值不回显, 只保留存在性
        assert!(!text.contains(secret), "relay 令牌不得回显: {text}");
        assert!(text.contains("KUNXTIER_RELAY_TOKEN"));
        assert!(text.contains("已设置"));
        // 卫星域与节点域合并终值可见
        assert!(text.contains("5.6.7.8:9443"));
        assert!(text.contains("./dev-state"));
        // admin 凭证未设置
        assert!(text.contains("KUNXTIER_ADMIN"));
        assert!(text.contains("未设置"));
    }

    #[test]
    fn render_core_mode_defaults_when_all_layers_empty() {
        let env = |_: &str| None::<String>;
        let node = NodeGlobalConfig::resolve(&env, NodeFileConfig::default());
        let text = render(&env, FileConfig::default(), &node).unwrap();
        assert!(text.contains("核心模式"));
        assert!(text.contains("0.0.0.0:443"));
        assert!(text.contains("(未设置)"));
    }

    #[test]
    fn init_flags_default_to_both() {
        assert_eq!(init_targets(false, false), (true, true));
        assert_eq!(init_targets(true, false), (true, false));
        assert_eq!(init_targets(false, true), (false, true));
        assert_eq!(init_targets(true, true), (true, true));
    }
}
