//! Description: relay --init: 状态目录初始化 (自签证书 / admin 凭证 / 默认网络)。
//!
//! Author: qinzhenya
//! Created: 2026-10-21

use std::fs::OpenOptions;
use std::path::Path;

use kunxtier_core::atomic_file::write_atomic;
use kunxtier_core::config::RelayConfig;

use crate::cert;
use crate::store::Store;
use crate::{Error, Result};

/// `relay --init` 的产出; 明文凭证只在这里展示一次。
pub struct InitOutput {
    pub admin_credential: String,
    pub fingerprint: String,
    pub data_dir: String,
}

/// admin 凭证落盘文件名 (state 目录内, 0600)。
/// 服务化 / 输出被 journal 吞掉时可找回; state 目录本身即信任边界 (TLS 私钥同域)。
pub const ADMIN_CREDENTIAL_FILE: &str = "admin-credential";

/// init 进程间互斥锁文件名 (state 目录内, 常驻不删除)。
/// 互斥状态在内核 flock 而非文件存在性: 持有进程退出即自动释放, 无残留锁问题。
const INIT_LOCK_FILE: &str = "init.lock";

/// init 互斥锁句柄: 持有打开的锁文件, Drop (关闭 fd) 即释放 flock。
struct InitLock {
    /// 仅承载所有权; 下划线前缀表明不读取, 释放靠 File 的 Drop。
    _file: std::fs::File,
}

/// 锁内初始化判定结果: 已初始化 (幂等无副作用) 或本次完成初始化。
enum Initialized {
    /// admin 哈希已存在; 两个入口对此的解释不同 (显式 init 报错, 运行前置视为通过)。
    Already,
    /// 本次执行完成初始化, 携带首启产出。
    Fresh(InitOutput),
}

/// 加锁执行初始化: 完成度以锁内 has_admin 判定, 不以 db 文件存在为准 —
/// db 创建远早于 admin 落库, 上次 init 中断留下的半初始化目录 (db 在而无 admin)
/// 会被文件存在性误判为已初始化, 导致 relay 无 admin 永久起服务。
///
/// 进程间互斥: init.lock 上的 flock (LOCK_EX|LOCK_NB), 互斥范围覆盖
/// "has_admin 判定到证书/私钥/凭证/落库" 整段 (先于 Store::open, db 文件
/// 创建也在互斥内), 保证并发 init 下文件组 (cert/key/admin) 恒出自同一
/// 生成批次。锁文件常驻不删除 — 持有状态在内核, 进程被 kill 即自动释放,
/// 不依赖 "写 pid / 删文件释放" 类用户态自愈 (该类方案存在 steal 竞态与
/// pid 复用误判, 评审裁决弃用)。
///
/// 步骤顺序有意为之: 先判定再写证书 (已初始化时不能覆盖既有证书,
/// 指纹已被客户端固定), db 侧收尾为单事务 — 中途任一步失败后重跑 init
/// 会覆盖残留证书并自动补齐落库, 无需手工清理状态目录。
fn init_under_lock(cfg: &RelayConfig) -> Result<Initialized> {
    // 显式选定 ring provider: 依赖图中可能同时存在多个 provider feature,
    // 尽早安装, 避免后续 TLS 构造点 (服务端 / 客户端) panic。
    let _ = rustls::crypto::ring::default_provider().install_default();
    std::fs::create_dir_all(&cfg.data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::metadata(&cfg.data_dir)?.permissions();
        let mut perms = perms.clone();
        perms.set_mode(0o700);
        std::fs::set_permissions(&cfg.data_dir, perms)?;
    }
    let _init_lock = acquire_init_lock(&cfg.data_dir)?;
    let store = Store::open(&cfg.db_path())?;
    if store.has_admin()? {
        return Ok(Initialized::Already);
    }

    let ck = cert::generate_self_signed().map_err(|e| Error::Cert(e.to_string()))?;
    // 私钥/证书原子写 + mode 随创建一步到位, 杜绝 "先写后改权限" 的宽权限窗口与半截文件
    write_atomic(&cfg.cert_path(), ck.cert_pem.as_bytes(), 0o644)?;
    write_atomic(&cfg.key_path(), ck.key_pem.as_bytes(), 0o600)?;

    let base =
        kunxtier_core::address::nth_network_base(0).map_err(|e| Error::Usage(e.to_string()))?;
    let admin = store.bootstrap_default(base)?;

    // 凭证同时写入 state 目录 (0600): 启动输出被 journal 吞掉或错过后可找回
    let cred_path = cfg.data_dir.join(ADMIN_CREDENTIAL_FILE);
    write_atomic(&cred_path, format!("{admin}\n").as_bytes(), 0o600)?;

    tracing::info!(data_dir = %cfg.data_dir.display(), "核心中继初始化完成");
    Ok(Initialized::Fresh(InitOutput {
        data_dir: cfg.data_dir.display().to_string(),
        admin_credential: admin,
        fingerprint: ck.fingerprint,
    }))
}

/// 初始化状态目录; 已初始化 (admin 哈希已存在) 则拒绝重复执行。
pub fn init(cfg: &RelayConfig) -> Result<InitOutput> {
    match init_under_lock(cfg)? {
        Initialized::Fresh(output) => Ok(output),
        Initialized::Already => Err(Error::Usage(format!(
            "状态目录已初始化 ({}), 拒绝重复 init",
            cfg.data_dir.display()
        ))),
    }
}

/// 运行模式启动前置: 未初始化则就地进行 init, 返回 Some(首启产出);
/// 已初始化返回 None, 不做任何变更。免记忆 `relay --init` 前置命令。
/// 并发首启撞锁时直接报错 (持有方必在执行中, 无残留态): 由服务管理器重启重试,
/// 对方完成后本进程即按已初始化通过。
pub fn ensure_initialized(cfg: &RelayConfig) -> Result<Option<InitOutput>> {
    Ok(match init_under_lock(cfg)? {
        Initialized::Fresh(output) => Some(output),
        Initialized::Already => None,
    })
}

/// 获取 init 互斥锁: 打开 (或复用) 常驻锁文件并独占 flock。
/// 撞锁即失败 — flock 持有者必为存活进程, 无需残留自愈与人工清理指引。
fn acquire_init_lock(data_dir: &Path) -> Result<InitLock> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .open(data_dir.join(INIT_LOCK_FILE))?;
    lock_exclusive(&file)?;
    Ok(InitLock { _file: file })
}

/// 对锁文件独占加锁 (unix): LOCK_EX|LOCK_NB, 持有至 fd 关闭 (进程退出内核回收)。
/// 信号打断 (EINTR) 即重试 — LOCK_NB 本不等待, 重试无饥饿风险。
#[cfg(unix)]
fn lock_exclusive(file: &std::fs::File) -> Result<()> {
    use std::os::fd::AsRawFd;
    loop {
        // SAFETY: flock 对有效 fd 是标准 advisory 锁调用, 无内存安全约束
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            // EWOULDBLOCK 在 Linux/macOS 与 EAGAIN 同值, 单判即覆盖两者
            Some(libc::EWOULDBLOCK) => {
                return Err(Error::Usage(
                    "另一个初始化正在进行 (init.lock 被持有); 其结束后自动释放, 请稍后重试".into(),
                ));
            }
            Some(libc::EINTR) => continue,
            _ => return Err(err.into()),
        }
    }
}

/// flock 为 unix 内核语义; 非 unix 保持先前的无互斥行为并留痕 (relay 验证平台为
/// Linux, 非 unix 本就非支持面), 不把平台边界伪装成调用方错误阻断 init。
#[cfg(not(unix))]
fn lock_exclusive(_file: &std::fs::File) -> Result<()> {
    tracing::warn!("init 进程间互斥 (flock) 仅 unix 支持, 当前平台无互斥");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kunxtier_core::config::RelayConfig;

    fn cfg_in(dir: &std::path::Path) -> RelayConfig {
        RelayConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            data_dir: dir.to_path_buf(),
            advertise: None,
            stun_servers: Vec::new(),
        }
    }

    #[test]
    fn 运行模式自动初始化_首启产出与凭证文件() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let first = ensure_initialized(&cfg)
            .unwrap()
            .expect("首启应返回初始化产出");
        assert!(cfg.db_path().exists());
        let cred_path = dir.path().join(ADMIN_CREDENTIAL_FILE);
        let content = std::fs::read_to_string(&cred_path).unwrap();
        assert_eq!(content.trim(), first.admin_credential);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&cred_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let key_mode = std::fs::metadata(cfg.key_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(key_mode, 0o600, "私钥应以 0600 落盘");
        }
        // 已初始化后不重复执行
        assert!(ensure_initialized(&cfg).unwrap().is_none());
    }

    #[test]
    fn 并发双_init_恰一成功_另一明确报错() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<Result<InitOutput>>();
        for _ in 0..2 {
            let (tx, dir_path) = (tx.clone(), dir.path().to_path_buf());
            std::thread::spawn(move || {
                let _ = tx.send(init(&cfg_in(&dir_path)));
            });
        }
        drop(tx);
        let (mut ok, mut err) = (0, 0);
        for result in rx.iter() {
            match result {
                Ok(_) => ok += 1,
                Err(e) => {
                    err += 1;
                    // 败方要么撞锁 (进行中), 要么晚到 (已初始化), 都应可定位
                    assert!(
                        e.to_string().contains("初始化"),
                        "败方错误应含明确指引: {e}"
                    );
                }
            }
        }
        assert_eq!((ok, err), (1, 1), "并发 init 必须恰好一方成功");
    }

    #[test]
    fn 常驻锁文件不阻碍后续_init() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        // 锁文件常驻: 上次 init 留下的文件 (无人持有 flock) 不阻碍再次拿锁
        std::fs::write(dir.path().join(INIT_LOCK_FILE), b"").unwrap();
        let output = init(&cfg).expect("无人持有的常驻锁文件应可直接加锁");
        let cred = std::fs::read_to_string(dir.path().join(ADMIN_CREDENTIAL_FILE)).unwrap();
        assert_eq!(cred.trim(), output.admin_credential);
        assert!(dir.path().join(INIT_LOCK_FILE).exists(), "锁文件常驻不删除");
    }

    #[test]
    fn 半初始化目录_自动补齐而非误判已初始化() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        // 模拟上次 init 在 Store::open 后中断: db 存在但无 admin (仅建 schema)
        drop(crate::store::Store::open(&cfg.db_path()).unwrap());
        assert!(cfg.db_path().exists());
        let output = ensure_initialized(&cfg)
            .expect("半初始化目录应走补齐路径")
            .expect("has_admin 为假应执行初始化, 而非按 db 存在误判已初始化");
        let cred = std::fs::read_to_string(dir.path().join(ADMIN_CREDENTIAL_FILE)).unwrap();
        assert_eq!(cred.trim(), output.admin_credential);
        // 补齐后再次启动按已初始化通过
        assert!(ensure_initialized(&cfg).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn 锁被持有者在任_报错且不落库() {
        use std::os::fd::AsRawFd;
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        // 以独立 fd 真实持有 flock: 模拟另一 init 正在进行 (flock 按 fd 冲突, 同进程亦然)
        let holder = OpenOptions::new()
            .write(true)
            .create(true)
            .open(dir.path().join(INIT_LOCK_FILE))
            .unwrap();
        assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
        let err = match init(&cfg) {
            Ok(_) => panic!("锁被持有者在任时 init 不应成功"),
            Err(e) => e,
        };
        let message = err.to_string();
        assert!(
            message.contains("正在进行"),
            "应提示 init 进行中: {message}"
        );
        assert!(message.contains(INIT_LOCK_FILE), "应指明锁文件: {message}");
        // 撞锁失败先于 Store::open, 不得留下 db 文件
        assert!(!cfg.db_path().exists());
    }
}
