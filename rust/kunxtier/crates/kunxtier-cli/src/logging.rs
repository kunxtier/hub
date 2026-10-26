//! Description: 日志装配: 级别分层解析与输出目标 (stderr / 文件轮转双写)。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 级别优先级 CLI flag > relay 配置文件 > RUST_LOG > 默认 info (NFR FR-1);
//! --log-file 启用后 stderr 与文件双写, 文件按大小轮转保留一份 .old。

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

use crate::cli::error::Fail;

pub const DEFAULT_LEVEL: &str = "info";
/// 单文件大小上限, 超限轮转; 只保留一份 .old (最轻手段, 不引入轮转库)。
const ROTATE_BYTES: u64 = 10 * 1024 * 1024;
const LEVELS: &[&str] = &["error", "warn", "info", "debug", "trace"];

/// 长驻进程 (relay / daemon) 的日志装配输入, 来自子命令 flag。
#[derive(Debug, Default)]
pub struct LogSpec {
    pub level: Option<String>,
    pub file: Option<PathBuf>,
}

/// 解析 EnvFilter 指令: flag > config > RUST_LOG > 默认。
/// flag / config 是用户显式配置, 非法值立即报错;
/// RUST_LOG 透传标准指令语义 (允许 kunxtier_node=debug 这类模块过滤)。
fn resolve_directive(
    flag: Option<&str>,
    config: Option<&str>,
    rust_log: Option<&str>,
) -> Result<String, Fail> {
    if let Some(v) = flag.map(str::trim).filter(|s| !s.is_empty()) {
        return level_directive(v);
    }
    if let Some(v) = config.map(str::trim).filter(|s| !s.is_empty()) {
        return level_directive(v);
    }
    if let Some(v) = rust_log.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(v.to_string());
    }
    Ok(DEFAULT_LEVEL.to_string())
}

fn level_directive(v: &str) -> Result<String, Fail> {
    if LEVELS.contains(&v) {
        Ok(v.to_string())
    } else {
        Err(Fail::usage(format!(
            "非法日志级别 {v} (支持 error/warn/info/debug/trace)"
        )))
    }
}

/// 初始化全局 subscriber; 全进程仅调用一次 (main 装配阶段)。
pub fn init(spec: &LogSpec, config_level: Option<&str>) -> Result<(), Fail> {
    let directive = resolve_directive(
        spec.level.as_deref(),
        config_level,
        std::env::var("RUST_LOG").ok().as_deref(),
    )?;
    let filter = tracing_subscriber::EnvFilter::new(directive);
    let file = match &spec.file {
        None => None,
        Some(path) => Some(Arc::new(RotatingFile::new(path, ROTATE_BYTES).map_err(
            |e| Fail::usage(format!("日志文件不可写 {}: {e}", path.display())),
        )?)),
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(TeeWriter { file })
        .init();
    Ok(())
}

/// stderr 与轮转文件双写的 MakeWriter; file 为 None 时仅 stderr。
struct TeeWriter {
    file: Option<Arc<RotatingFile>>,
}

impl<'a> MakeWriter<'a> for TeeWriter {
    type Writer = TeeWrite;

    fn make_writer(&'a self) -> Self::Writer {
        TeeWrite {
            stderr: io::stderr(),
            file: self.file.clone(),
        }
    }
}

struct TeeWrite {
    stderr: io::Stderr,
    file: Option<Arc<RotatingFile>>,
}

impl Write for TeeWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // stderr 尽力而为: 后台形态下可能已关闭, 失败不阻断文件留痕
        let _ = self.stderr.write_all(buf);
        if let Some(f) = &self.file {
            f.append(buf)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = self.stderr.flush();
        if let Some(f) = &self.file {
            f.flush()?;
        }
        Ok(())
    }
}

/// 追加写文件, 超 max_bytes 时轮转为 <path>.old (仅保留一份)。
struct RotatingFile {
    path: PathBuf,
    max_bytes: u64,
    state: Mutex<FileState>,
}

struct FileState {
    file: File,
    written: u64,
}

impl RotatingFile {
    fn new(path: &Path, max_bytes: u64) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            max_bytes,
            state: Mutex::new(FileState { file, written }),
        })
    }

    fn append(&self, buf: &[u8]) -> io::Result<()> {
        // expect 理由: 锁中毒仅发生在持有线程 panic 后, 日志通道已不可信, 快速失败优于静默丢日志
        let mut st = self.state.lock().expect("日志文件锁中毒");
        if st.written > 0 && st.written + buf.len() as u64 >= self.max_bytes {
            rotate(&mut st, &self.path)?;
        }
        st.file.write_all(buf)?;
        st.written += buf.len() as u64;
        Ok(())
    }

    fn flush(&self) -> io::Result<()> {
        self.state.lock().expect("日志文件锁中毒").file.flush()
    }
}

/// 当前文件轮转为 .old: rename 不可用时退化为丢弃重建 (轮转失败不应中断日志链路)。
fn rotate(st: &mut FileState, path: &Path) -> io::Result<()> {
    let old = with_suffix(path, ".old");
    let _ = std::fs::remove_file(&old);
    if std::fs::rename(path, &old).is_err() {
        let _ = std::fs::remove_file(path);
    }
    st.file = OpenOptions::new().create(true).append(true).open(path)?;
    st.written = 0;
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_priority_flag_config_env_default() {
        let mk =
            |f: Option<&str>, c: Option<&str>, e: Option<&str>| resolve_directive(f, c, e).unwrap();
        // 三层齐设: flag 胜出
        assert_eq!(mk(Some("debug"), Some("warn"), Some("error")), "debug");
        // 无 flag: config 胜出
        assert_eq!(mk(None, Some("warn"), Some("error")), "warn");
        // 无 flag / config: RUST_LOG 透传 (允许模块级指令)
        assert_eq!(
            mk(None, None, Some("kunxtier_node=trace")),
            "kunxtier_node=trace"
        );
        // 全空: 默认
        assert_eq!(mk(None, None, None), DEFAULT_LEVEL);
        // 空串与纯空白视同未设置
        assert_eq!(mk(Some("  "), None, Some("")), DEFAULT_LEVEL);
    }

    #[test]
    fn invalid_flag_or_config_level_rejected() {
        assert!(resolve_directive(Some("verbose"), None, None).is_err());
        assert!(resolve_directive(None, Some("DEBUG "), None).is_err());
        // RUST_LOG 层不做级别校验, 由 EnvFilter 语义兜底
        assert!(resolve_directive(None, None, Some("verbose")).is_ok());
    }

    #[test]
    fn file_rotates_at_threshold_keeping_one_old() {
        let dir = std::env::temp_dir().join(format!("kunxtier-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kunxtier.log");
        let old = with_suffix(&path, ".old");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&old);

        let chunk = "x".repeat(60);
        let rf = RotatingFile::new(&path, 100).unwrap();
        rf.append(chunk.as_bytes()).unwrap();
        // 60 + 60 >= 100: 首段轮转入 .old
        rf.append(chunk.as_bytes()).unwrap();
        // 再次越限: .old 只保留最近一段
        rf.append(chunk.as_bytes()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap().len(), 60);
        assert_eq!(std::fs::read(&old).unwrap().len(), 60);

        // 重开沿用既有大小 (written 从 metadata 恢复), 追加语义不破坏
        let rf = RotatingFile::new(&path, 10_000).unwrap();
        rf.append(b"tail").unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().ends_with("tail"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
