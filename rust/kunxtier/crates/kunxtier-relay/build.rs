//! Description: 构建脚本: 保证控制台嵌入目录存在 (fresh clone 后由本脚本建空目录,
//! include_dir 对空目录生成空文件表, 运行时回落代码内嵌占位页; 前端产物经
//! cargo make build 由 vite 输出并全量清理该目录, git 不跟踪其内容)。
//!
//! Author: qinzhenya
//! Created: 2026-10-30

use std::fs;
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/console");
    // 幂等: 目录已存在 (产物已构建) 时无操作
    if let Err(e) = fs::create_dir_all(&dir) {
        panic!("创建控制台嵌入目录失败 {}: {e}", dir.display());
    }
}
