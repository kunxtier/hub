//! Description: 节点 state 持久化: 注册身份与网络归属的落盘与恢复 (版本化兼容读取)。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! node.json 集中承载节点全部敏感量 (节点凭证 / WG 私钥), 文件 0600,
//! 目录 0755 (T14 服务化: IPC socket 位于目录内且 0666, 敏感由文件级
//! 权限承载), 只存在于本机 state 目录, 不进日志与配置 (NFR-4)。

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::identity::WgIdentity;

/// state 结构版本: 高版本文件拒绝读取 (前向兼容判据, 升级时按版本迁移)。
pub const STATE_VERSION: u32 = 1;
/// state 文件名 (位于 state 目录内)。
pub const STATE_FILE: &str = "node.json";

/// 节点持久化身份: 一次注册, 后续 daemon 启动即凭此自动上线。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeState {
    pub version: u32,
    pub network: String,
    pub name: String,
    /// 本节点虚拟 IP (注册时分配; 权威值以信令 Ready 事件为准)。
    pub vip: Ipv4Addr,
    /// 节点凭证明文 (信令与转发准入共用)。
    pub credential: String,
    /// 核心地址 host:port。
    pub core: String,
    /// 核心证书指纹 (hex)。
    pub fingerprint: String,
    /// WG UDP 固定监听端口 (打洞候选上报前提, 重启保持稳定)。
    pub listen_port: u16,
    /// WG 私钥 (base64; 双凭证之一, 不离机)。
    pub wg_private_key: String,
}

impl NodeState {
    /// 以新生成身份构造未注册 state 的字段骨架 (注册结果随后填入)。
    pub fn from_identity(identity: &WgIdentity) -> Self {
        Self {
            version: STATE_VERSION,
            network: String::new(),
            name: String::new(),
            vip: Ipv4Addr::UNSPECIFIED,
            credential: String::new(),
            core: String::new(),
            fingerprint: String::new(),
            listen_port: 0,
            wg_private_key: BASE64_STANDARD.encode(identity.private_key_bytes()),
        }
    }

    /// 恢复 WG 身份。
    pub fn identity(&self) -> Result<WgIdentity> {
        let bytes: [u8; 32] = BASE64_STANDARD
            .decode(&self.wg_private_key)
            .map_err(|e| Error::Key(format!("state 私钥解码失败: {e}")))?
            .try_into()
            .map_err(|v: Vec<u8>| {
                Error::Key(format!(
                    "state 私钥长度非法: 期望 32 字节, 实际 {}",
                    v.len()
                ))
            })?;
        Ok(WgIdentity::from_private_key_bytes(bytes))
    }

    /// 落盘: 临时文件 + 原子改名, 避免写一半损坏既有身份。
    pub fn save(&self, dir: &Path) -> Result<()> {
        ensure_state_dir(dir)?;
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::Config(format!("state 序列化失败: {e}")))?;
        let tmp = dir.join(format!("{STATE_FILE}.tmp"));
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&json)?;
        file.sync_all()?;
        fs::rename(&tmp, dir.join(STATE_FILE))?;
        Ok(())
    }

    /// 读取; 目录存在但未注册 (文件不存在) 返回 None。
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let path: PathBuf = dir.join(STATE_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let state: NodeState = serde_json::from_str(&text)
            .map_err(|e| Error::Config(format!("state 解析失败: {e}")))?;
        if state.version != STATE_VERSION {
            return Err(Error::Config(format!(
                "state 版本不兼容: 文件 v{}, 当前支持 v{}",
                state.version, STATE_VERSION
            )));
        }
        Ok(Some(state))
    }
}

/// 创建 state 目录 (0755, 已存在时不改权限); 敏感量由 node.json 0600 承载。
pub fn ensure_state_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(dir)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(dir)?;
    Ok(())
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NodeState {
        let mut state = NodeState::from_identity(&WgIdentity::generate());
        state.network = "default".into();
        state.name = "laptop".into();
        state.vip = "100.64.0.5".parse().unwrap();
        state.credential = "cred".into();
        state.core = "core.example.com:443".into();
        state.fingerprint = "ab12".into();
        state.listen_port = 51820;
        state
    }

    #[test]
    fn state_roundtrips_and_identity_restores() {
        let dir = tempfile::tempdir().unwrap();
        let state = sample();
        state.save(dir.path()).unwrap();
        let loaded = NodeState::load(dir.path()).unwrap().expect("应读到 state");
        assert_eq!(loaded.name, state.name);
        assert_eq!(loaded.vip, state.vip);
        assert_eq!(loaded.core, state.core);
        assert_eq!(loaded.listen_port, 51820);
        // 身份可恢复且公钥一致 (重启后节点身份不变的前提)
        assert_eq!(
            loaded.identity().unwrap().public_key_base64(),
            state.identity().unwrap().public_key_base64()
        );
    }

    #[test]
    fn missing_state_file_yields_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(NodeState::load(dir.path()).unwrap().is_none());
    }

    #[test]
    fn future_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = sample();
        state.version = STATE_VERSION + 1;
        state.save(dir.path()).unwrap();
        let err = NodeState::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("版本不兼容"), "实际: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn state_file_and_dir_are_user_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let state = sample();
        state.save(dir.path()).unwrap();
        let file_mode = fs::metadata(dir.path().join(STATE_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "state 文件必须 0600");
        let sub = dir.path().join("nested/dir");
        ensure_state_dir(&sub).unwrap();
        let dir_mode = fs::metadata(&sub).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o755,
            "新建 state 目录必须 0755 (socket 所在, 敏感由文件级承载)"
        );
    }
}
