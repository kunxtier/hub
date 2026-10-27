//! Description: 原子文件写入: 同目录临时文件 + fsync + rename, 供机器级落盘共用 (配置 / 服务定义 / 节点身份)。
//!
//! Author: qinzhenya
//! Created: 2026-10-27
//!
//! 写入中途失败 (崩溃 / 磁盘满) 只可能残留 .tmp, 目标文件保持旧内容完好;
//! 半截文件对 "非法内容阻断下次启动" 类场景是启动级故障, 必须杜绝。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 原子写入: 父目录按需创建, 内容写同目录 `<name>.tmp` 并 sync 后 rename 覆盖目标。
/// unix 下新文件以 mode 创建; 写入失败清理临时文件, 目标文件不受影响。
pub fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path(path);
    // 残留 tmp 先移除: OpenOptions 的 mode 只在新建时生效
    let _ = fs::remove_file(&tmp);
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options.open(&tmp)?;
    if let Err(e) = file.write_all(contents).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, path)
}

/// 临时文件路径: 与目标同目录, 保证 rename 原子性 (跨目录 rename 不保证)。
fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_nested_content_with_mode_and_no_tmp_leftover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        write_atomic(&path, b"state_dir = \"/x\"\n", 0o644).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"state_dir = \"/x\"\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644);
        }
        assert!(!tmp_path(&path).exists(), "成功后不应残留 .tmp");
    }

    #[test]
    fn overwrite_replaces_existing_content_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, "old").unwrap();
        write_atomic(&path, b"new", 0o644).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn stale_tmp_is_replaced_not_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(tmp_path(&path), "stale").unwrap();
        write_atomic(&path, b"fresh", 0o600).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "残留 tmp 不得让 mode 失效");
        }
    }
}
