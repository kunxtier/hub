//! Description: daemon 服务化: 平台服务定义生成与 install / uninstall 计划执行 (FR-9)。
//!
//! Author: qinzhenya
//! Created: 2026-10-27
//!
//! 计划与执行分离: install_plan / uninstall_plan 产出纯数据步骤序列
//! (建目录 / 写文件 / 删文件 / 执行命令), execute 落盘并调起系统服务
//! 管理器; 命令经 Runner 注入, 单测断言调用序列而不真调 launchctl /
//! systemctl。服务定义只固化二进制路径与 --log-file (install 时按全局
//! 配置解析), state_dir 运行时由 daemon 读全局配置, 保持单一事实源。

use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;

use crate::error::{Error, Result};

/// 服务装配输入: daemon 二进制绝对路径与日志文件路径。
#[derive(Debug, Clone)]
pub struct ServiceSpec {
    pub bin: PathBuf,
    pub log_file: PathBuf,
}

/// 计划步骤; 顺序执行, 任一失败即中止。
#[derive(Debug, Clone)]
pub enum Step {
    /// 建目录 (0755, 递归)。
    Mkdir(PathBuf),
    /// 写文件 (mode 为 unix 权限位)。
    Write(FileWrite),
    /// 删文件 (缺失不视为错误)。
    Remove(PathBuf),
    /// 执行命令; tolerant 为真时非零退出不视为失败 (幂等重装的清理步)。
    Run(CommandSpec),
}

#[derive(Debug, Clone)]
pub struct FileWrite {
    pub path: PathBuf,
    pub content: String,
    pub mode: u32,
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub tolerant: bool,
}

/// 平台服务操作计划; definition 为服务定义文件路径 (提示输出用)。
#[derive(Debug, Clone)]
pub struct ServicePlan {
    pub definition: PathBuf,
    pub steps: Vec<Step>,
}

/// 命令执行器抽象: 生产走系统进程, 测试注入断言调用序列。
pub trait Runner {
    fn run(&self, program: &str, args: &[String]) -> io::Result<Output>;
}

/// 系统进程执行器。
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[String]) -> io::Result<Output> {
        std::process::Command::new(program).args(args).output()
    }
}

/// 注册系统服务并启动 (重装即覆盖定义并重启, 幂等)。
pub fn install_plan(spec: &ServiceSpec) -> Result<ServicePlan> {
    #[cfg(target_os = "macos")]
    {
        Ok(macos::install(spec))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(linux::install(spec))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = spec;
        Err(Error::Config(
            "当前平台暂不支持服务注册 (Windows 随平台切片落地)".into(),
        ))
    }
}

/// 停服并移除服务注册 (state 与全局配置不在清理范围)。
pub fn uninstall_plan() -> Result<ServicePlan> {
    #[cfg(target_os = "macos")]
    {
        Ok(macos::uninstall())
    }
    #[cfg(target_os = "linux")]
    {
        Ok(linux::uninstall())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(Error::Config(
            "当前平台暂不支持服务注销 (Windows 随平台切片落地)".into(),
        ))
    }
}

/// 顺序执行计划步骤。
pub fn execute(steps: &[Step]) -> Result<()> {
    execute_with(steps, &SystemRunner)
}

pub fn execute_with(steps: &[Step], runner: &dyn Runner) -> Result<()> {
    for step in steps {
        match step {
            Step::Mkdir(dir) => mkdir(dir).map_err(|e| map_io(e, "创建目录"))?,
            Step::Write(file) => write_file(file).map_err(|e| map_io(e, "写入文件"))?,
            Step::Remove(path) => match std::fs::remove_file(path) {
                Ok(()) => {}
                // 缺失即已清理, 幂等语义
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(map_io(e, "删除文件")),
            },
            Step::Run(cmd) => run_command(cmd, runner)?,
        }
    }
    Ok(())
}

/// EACCES 统一映射为提权提示 (FR-9: 特权要求在安装提示明确), 其余保留原语境。
fn map_io(e: io::Error, ctx: &str) -> Error {
    if e.kind() == io::ErrorKind::PermissionDenied {
        Error::Config(format!(
            "{ctx}权限不足: install / uninstall 需要 root, 请使用 sudo 执行"
        ))
    } else {
        Error::Config(format!("{ctx}失败: {e}"))
    }
}

fn mkdir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(dir)
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

fn write_file(file: &FileWrite) -> io::Result<()> {
    if let Some(parent) = file.path.parent() {
        if !parent.as_os_str().is_empty() {
            mkdir(parent)?;
        }
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(file.mode)
            .open(&file.path)?;
        f.write_all(file.content.as_bytes())
    }
    #[cfg(not(unix))]
    std::fs::write(&file.path, &file.content)
}

fn run_command(cmd: &CommandSpec, runner: &dyn Runner) -> Result<()> {
    let out = runner
        .run(&cmd.program, &cmd.args)
        .map_err(|e| map_io(e, "执行命令"))?;
    if !out.status.success() && !cmd.tolerant {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Error::Config(format!(
            "命令执行失败: {} {} ({}): {}",
            cmd.program,
            cmd.args.join(" "),
            out.status,
            stderr.trim()
        )));
    }
    Ok(())
}

// ------------------------------ 平台实现 ------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use super::{CommandSpec, FileWrite, PathBuf, ServicePlan, ServiceSpec, Step};

    const LABEL: &str = "com.kunxtier.daemon";
    const PLIST: &str = "/Library/LaunchDaemons/com.kunxtier.daemon.plist";

    pub fn install(spec: &ServiceSpec) -> ServicePlan {
        ServicePlan {
            definition: PathBuf::from(PLIST),
            steps: vec![
                Step::Write(FileWrite {
                    path: PathBuf::from(PLIST),
                    content: plist_content(&spec.bin, &spec.log_file),
                    mode: 0o644,
                }),
                // 先摘除旧实例: 重装即以新定义重启 (首装无旧实例, tolerant)
                Step::Run(CommandSpec {
                    program: "launchctl".into(),
                    args: vec!["bootout".into(), "system".into(), PLIST.into()],
                    tolerant: true,
                }),
                Step::Run(CommandSpec {
                    program: "launchctl".into(),
                    args: vec!["bootstrap".into(), "system".into(), PLIST.into()],
                    tolerant: false,
                }),
            ],
        }
    }

    pub fn uninstall() -> ServicePlan {
        ServicePlan {
            definition: PathBuf::from(PLIST),
            steps: vec![
                Step::Run(CommandSpec {
                    program: "launchctl".into(),
                    args: vec!["bootout".into(), "system".into(), PLIST.into()],
                    tolerant: true,
                }),
                Step::Remove(PathBuf::from(PLIST)),
            ],
        }
    }

    /// RunAtLoad + KeepAlive: 开机自启与崩溃自动拉起。daemon 持久故障
    /// (运行期致命态与启动期 state 故障) 留守不退出, 不会引发拉起风暴;
    /// 目录 / socket 级环境失败退出后由 launchd 默认节流重试。
    /// 日志走 --log-file (launchd 不接管 stderr)。
    fn plist_content(bin: &std::path::Path, log_file: &std::path::Path) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>{LABEL}</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             \t\t<string>{bin}</string>\n\
             \t\t<string>daemon</string>\n\
             \t\t<string>--log-file</string>\n\
             \t\t<string>{log}</string>\n\
             \t</array>\n\
             \t<key>RunAtLoad</key>\n\
             \t<true/>\n\
             \t<key>KeepAlive</key>\n\
             \t<true/>\n\
             </dict>\n\
             </plist>\n",
            bin = bin.display(),
            log = log_file.display(),
        )
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{CommandSpec, FileWrite, PathBuf, ServicePlan, ServiceSpec, Step};

    const NAME: &str = "kunxtier";
    const UNIT: &str = "/etc/systemd/system/kunxtier.service";

    pub fn install(spec: &ServiceSpec) -> ServicePlan {
        ServicePlan {
            definition: PathBuf::from(UNIT),
            steps: vec![
                Step::Write(FileWrite {
                    path: PathBuf::from(UNIT),
                    content: unit_content(&spec.bin, &spec.log_file),
                    mode: 0o644,
                }),
                Step::Run(CommandSpec {
                    program: "systemctl".into(),
                    args: vec!["daemon-reload".into()],
                    tolerant: false,
                }),
                Step::Run(CommandSpec {
                    program: "systemctl".into(),
                    args: vec!["enable".into(), "--now".into(), NAME.into()],
                    tolerant: false,
                }),
                // 重装即以新定义重启 (运行中实例的原单元定义不热更新)
                Step::Run(CommandSpec {
                    program: "systemctl".into(),
                    args: vec!["restart".into(), NAME.into()],
                    tolerant: false,
                }),
            ],
        }
    }

    pub fn uninstall() -> ServicePlan {
        ServicePlan {
            definition: PathBuf::from(UNIT),
            steps: vec![
                Step::Run(CommandSpec {
                    program: "systemctl".into(),
                    args: vec!["disable".into(), "--now".into(), NAME.into()],
                    tolerant: true,
                }),
                Step::Remove(PathBuf::from(UNIT)),
                Step::Run(CommandSpec {
                    program: "systemctl".into(),
                    args: vec!["daemon-reload".into()],
                    tolerant: false,
                }),
            ],
        }
    }

    /// network-online 之后启动 (网络未就绪时 daemon 亦有退避重连兜底);
    /// stderr 旁路进 journald, 权威日志走 --log-file。
    fn unit_content(bin: &std::path::Path, log_file: &std::path::Path) -> String {
        format!(
            "[Unit]\n\
             Description=kunxtier node daemon\n\
             Wants=network-online.target\n\
             After=network-online.target\n\
             \n\
             [Service]\n\
             ExecStart=\"{bin}\" daemon --log-file \"{log}\"\n\
             Restart=on-failure\n\
             RestartSec=5\n\
             \n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            bin = bin.display(),
            log = log_file.display(),
        )
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeRunner {
        calls: RefCell<Vec<(String, Vec<String>)>>,
    }

    impl Runner for FakeRunner {
        fn run(&self, program: &str, args: &[String]) -> io::Result<Output> {
            self.calls
                .borrow_mut()
                .push((program.to_string(), args.to_vec()));
            Ok(Output {
                status: std::process::ExitStatus::default(),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }

    #[test]
    fn permission_denied_maps_to_sudo_hint() {
        let e = io::Error::from(io::ErrorKind::PermissionDenied);
        let msg = map_io(e, "写入文件 ").to_string();
        assert!(msg.contains("sudo"), "应包含提权提示: {msg}");
        let other = io::Error::other("boom");
        assert!(!map_io(other, "写入文件 ").to_string().contains("sudo"));
    }

    #[test]
    fn execute_applies_steps_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("svc.conf");
        let runner = FakeRunner {
            calls: RefCell::new(Vec::new()),
        };
        let steps = vec![
            Step::Mkdir(dir.path().join("logs")),
            Step::Write(FileWrite {
                path: file.clone(),
                content: "hello".into(),
                mode: 0o644,
            }),
            Step::Run(CommandSpec {
                program: "echo".into(),
                args: vec!["hi".into()],
                tolerant: false,
            }),
            Step::Remove(file.clone()),
        ];
        execute_with(&steps, &runner).unwrap();
        assert!(dir.path().join("logs").is_dir());
        assert!(!file.exists(), "Remove 应删除文件");
        assert_eq!(runner.calls.borrow().len(), 1);
        // 文件权限按 mode 落盘 (unix)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let f = dir.path().join("perm.conf");
            execute_with(
                &[Step::Write(FileWrite {
                    path: f.clone(),
                    content: "x".into(),
                    mode: 0o600,
                })],
                &runner,
            )
            .unwrap();
            assert_eq!(
                std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn execute_ignores_missing_removal() {
        let runner = FakeRunner {
            calls: RefCell::new(Vec::new()),
        };
        // 缺失文件的 Remove 幂等通过
        execute_with(
            &[Step::Remove(PathBuf::from("/nonexistent/kunxtier-test"))],
            &runner,
        )
        .unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn failing_command_aborts_unless_tolerant() {
        struct FailRunner;
        impl Runner for FailRunner {
            fn run(&self, _program: &str, _args: &[String]) -> io::Result<Output> {
                use std::os::unix::process::ExitStatusExt;
                Ok(Output {
                    status: std::process::ExitStatus::from_raw(1 << 8),
                    stdout: Vec::new(),
                    stderr: b"nope".to_vec(),
                })
            }
        }
        // 非容忍失败: 报错并携带命令与 stderr
        let steps = [Step::Run(CommandSpec {
            program: "bad-cmd".into(),
            args: vec![],
            tolerant: false,
        })];
        let msg = execute_with(&steps, &FailRunner).unwrap_err().to_string();
        assert!(msg.contains("bad-cmd") && msg.contains("nope"), "{msg}");
        // 容忍失败: 通过 (幂等重装的清理步)
        let steps = [Step::Run(CommandSpec {
            program: "bad-cmd".into(),
            args: vec![],
            tolerant: true,
        })];
        execute_with(&steps, &FailRunner).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_install_plan_generates_plist_and_launchctl_sequence() {
        let spec = ServiceSpec {
            bin: "/usr/local/bin/kunxtier".into(),
            log_file: "/Library/Logs/kunxtier/daemon.log".into(),
        };
        let plan = install_plan(&spec).unwrap();
        assert_eq!(
            plan.definition,
            PathBuf::from("/Library/LaunchDaemons/com.kunxtier.daemon.plist")
        );
        assert_eq!(plan.steps.len(), 3);
        match &plan.steps[0] {
            Step::Write(f) => {
                assert_eq!(f.mode, 0o644);
                let c = &f.content;
                assert!(c.contains("com.kunxtier.daemon"));
                assert!(c.contains("/usr/local/bin/kunxtier</string>"));
                assert!(c.contains("--log-file"));
                assert!(c.contains("/Library/Logs/kunxtier/daemon.log"));
                assert!(c.contains("<key>RunAtLoad</key>"));
                assert!(c.contains("<key>KeepAlive</key>"));
            }
            other => panic!("首步应为写 plist, 实际 {other:?}"),
        }
        match (&plan.steps[1], &plan.steps[2]) {
            (Step::Run(bootout), Step::Run(bootstrap)) => {
                assert!(bootout.tolerant, "bootout 需容忍无旧实例");
                assert!(!bootstrap.tolerant);
                assert_eq!(bootstrap.args[0], "bootstrap");
            }
            other => panic!("后两步应为 launchctl 调用, 实际 {other:?}"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uninstall_plan_boots_out_then_removes() {
        let plan = uninstall_plan().unwrap();
        match (&plan.steps[0], &plan.steps[1]) {
            (Step::Run(cmd), Step::Remove(path)) => {
                assert!(cmd.tolerant);
                assert_eq!(cmd.args[0], "bootout");
                assert_eq!(*path, plan.definition);
            }
            other => panic!("应为 bootout + 删除 plist, 实际 {other:?}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_install_and_uninstall_plan_sequences() {
        let spec = ServiceSpec {
            bin: "/usr/local/bin/kunxtier".into(),
            log_file: "/var/log/kunxtier/daemon.log".into(),
        };
        let plan = install_plan(&spec).unwrap();
        assert_eq!(
            plan.definition,
            PathBuf::from("/etc/systemd/system/kunxtier.service")
        );
        match &plan.steps[0] {
            Step::Write(f) => {
                assert!(f.content.contains("ExecStart=\"/usr/local/bin/kunxtier\" daemon --log-file \"/var/log/kunxtier/daemon.log\""));
                assert!(f.content.contains("Restart=on-failure"));
                assert!(f.content.contains("After=network-online.target"));
                assert!(f.content.contains("WantedBy=multi-user.target"));
            }
            other => panic!("首步应为写 unit, 实际 {other:?}"),
        }
        let cmds: Vec<_> = plan.steps[1..]
            .iter()
            .map(|s| match s {
                Step::Run(c) => c.args[0].as_str(),
                other => panic!("期望命令步, 实际 {other:?}"),
            })
            .collect();
        assert_eq!(cmds, vec!["daemon-reload", "enable", "restart"]);

        let plan = uninstall_plan().unwrap();
        assert!(matches!(&plan.steps[0], Step::Run(c) if c.args[0] == "disable" && c.tolerant));
        assert!(matches!(&plan.steps[1], Step::Remove(_)));
        assert!(matches!(&plan.steps[2], Step::Run(c) if c.args[0] == "daemon-reload"));
    }

    /// 特权冒烟 (手动执行):
    /// `KUNXTIER_SMOKE_BIN=<kunxtier 二进制> sudo cargo test -p kunxtier-node service -- --ignored`
    /// 真实 launchd 全链: install 注册启动 → kickstart 重启 (自动恢复) → uninstall 清理。
    /// 会在本机生成全局配置 (uninstall 按设计保留), 重复执行前可手动删除回归首装路径。
    #[test]
    #[ignore]
    #[cfg(target_os = "macos")]
    fn macos_privileged_service_smoke() {
        let bin = std::env::var("KUNXTIER_SMOKE_BIN")
            // expect 理由: 冒烟需真实二进制 (current_exe 是测试进程, 不可注册为服务)
            .expect("需以 KUNXTIER_SMOKE_BIN=<kunxtier 二进制路径> 提供被测二进制");
        let (file, generated) = kunxtier_core::config::NodeFileConfig::load_or_default(
            &kunxtier_core::config::node_config_path(),
        )
        .unwrap();
        let config = kunxtier_core::config::NodeGlobalConfig::resolve(None, |_| None, file);
        let spec = ServiceSpec {
            bin: bin.into(),
            log_file: config.log_dir.join("daemon.log"),
        };

        let mut steps = Vec::new();
        if let Some(content) = generated {
            steps.push(Step::Mkdir(
                kunxtier_core::config::node_config_path()
                    .parent()
                    .unwrap()
                    .to_path_buf(),
            ));
            steps.push(Step::Write(FileWrite {
                path: kunxtier_core::config::node_config_path(),
                content,
                mode: 0o644,
            }));
        }
        steps.push(Step::Mkdir(config.log_dir.clone()));
        steps.push(Step::Mkdir(config.state_dir.clone()));
        steps.extend(install_plan(&spec).unwrap().steps);
        execute(&steps).unwrap();

        // 服务拉起的 daemon 应在 state 目录绑定 IPC socket (无 state 即留守)
        let sock = crate::ipc::socket_path(&config.state_dir);
        wait_for(|| sock.exists(), "daemon 启动后应出现 IPC socket");

        // kickstart -k 重启: 自动恢复 (开机自启同机制)
        let _ = std::process::Command::new("launchctl")
            .args(["kickstart", "-k", "system/com.kunxtier.daemon"])
            .status();
        wait_for(|| sock.exists(), "重启后 IPC socket 应恢复");

        execute(&uninstall_plan().unwrap().steps).unwrap();
        assert!(
            !PathBuf::from("/Library/LaunchDaemons/com.kunxtier.daemon.plist").exists(),
            "uninstall 后服务定义应被移除"
        );

        fn wait_for(cond: impl Fn() -> bool, what: &str) {
            for _ in 0..100 {
                if cond() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            panic!("等待超时: {what}");
        }
    }
}
