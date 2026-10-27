//! Description: 服务化子命令: install 注册系统服务与开机自启, uninstall 反向清理 (FR-9)。
//!
//! Author: qinzhenya
//! Created: 2026-10-27
//!
//! install 职责: 生成 / 校验机器级全局配置 → 确保 state / log 目录 → 写
//! 服务定义并注册启动 (平台差异收口在 kunxtier-node::service); uninstall
//! 停服并移除服务定义。两者均保留 state 与全局配置 (身份不丢)。

use clap::Args;

use kunxtier_core::config::{node_config_path, NodeFileConfig, NodeGlobalConfig};
use kunxtier_node::service::{self, FileWrite, ServiceSpec, Step};

use super::error::Fail;

/// 注册系统服务与开机自启 (首次生成全局配置; 需 root)。
#[derive(Args)]
pub struct InstallArgs;

/// 停服并移除系统服务注册 (state 与全局配置保留)。
#[derive(Args)]
pub struct UninstallArgs;

impl InstallArgs {
    pub fn execute(self) -> Result<(), Fail> {
        let bin =
            std::env::current_exe().map_err(|e| Fail::usage(format!("无法定位当前二进制: {e}")))?;
        // 全局配置: 缺失则以默认值显式生成 (可读可改), 存在则只校验不改写;
        // 不接受 env 覆盖——服务运行时读不到安装时的 env, 避免两处真相
        let (file, generated) =
            NodeFileConfig::load_or_default(&node_config_path()).map_err(|e| {
                Fail::usage(format!(
                    "全局配置读取失败 ({}): {e}",
                    node_config_path().display()
                ))
            })?;
        let config = NodeGlobalConfig::resolve(None, |_| None, file);
        let spec = ServiceSpec {
            bin,
            log_file: config.log_dir.join("daemon.log"),
        };

        let mut steps = Vec::new();
        if let Some(content) = generated {
            if let Some(parent) = node_config_path().parent() {
                steps.push(Step::Mkdir(parent.to_path_buf()));
            }
            steps.push(Step::Write(FileWrite {
                path: node_config_path(),
                content,
                mode: 0o644,
            }));
        }
        steps.push(Step::Mkdir(config.log_dir.clone()));
        steps.push(Step::Mkdir(config.state_dir.clone()));
        let plan = service::install_plan(&spec).map_err(fail)?;
        steps.extend(plan.steps);
        service::execute(&steps).map_err(fail)?;

        println!("全局配置: {}", node_config_path().display());
        println!("state 目录: {}", config.state_dir.display());
        println!("日志文件: {}", spec.log_file.display());
        println!("服务定义: {}", plan.definition.display());
        println!(
            "服务已注册并启动; 首次使用执行 kunxtier up --token <令牌> --name <节点名> 完成注册"
        );
        Ok(())
    }
}

impl UninstallArgs {
    pub fn execute(self) -> Result<(), Fail> {
        let plan = service::uninstall_plan().map_err(fail)?;
        service::execute(&plan.steps).map_err(fail)?;
        println!(
            "服务已停止并移除 (定义 {}); state 目录与全局配置保留",
            plan.definition.display()
        );
        Ok(())
    }
}

fn fail(e: kunxtier_node::Error) -> Fail {
    // 服务化失败多为环境与权限问题, 归 usage 由用户处置
    Fail::usage(format!("服务化操作失败: {e}"))
}
