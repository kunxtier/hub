# kunxtier 节点 macOS 机器布局统一 unix 路径与首启配置生成

## 背景

T14 交付后实际走查暴露两处不适 (2026-10-27, 用户反馈):

- 非 root 前台运行 `kunxtier daemon` 在 macOS 必然失败: `/Library/Application Support` 属 root:admin 且组无写权限, `ensure_state_dir` 收到 EACCES 后 daemon 直接退出, 透出的是裸 io 错误 (os error 13), 无从得知该 sudo / `--state-dir` / install; 目录从未创建, 用户只能靠猜
- `/Library/Application Support` 是 GUI 正统应用的位置; kunxtier 是终端应用 + 特权系统服务, 该路径气质不符, 且与 Linux 侧 `/etc` + `/var/lib` + `/var/log` 的 unix daemon 惯例不一致, 两平台两套心智

曾评估 `~/.kunxtier` 用户级布局, 否决: TUN 要求 root, 生产形态是 root 系统服务 (root home 为 /var/root, 用户级目录会分裂服务与 CLI 的身份对齐); 同类产品 (tailscaled /var/lib, WireGuard /etc/wireguard) 均为机器级。

## 规划裁决 (2026-10-27, 用户拍板方案 B)

- macOS 平台路径改为与 Linux 完全同构: 全局配置 `/etc/kunxtier/config.toml` (0644), state `/var/lib/kunxtier`, 日志 `/var/log/kunxtier`; 服务定义位置不变 (LaunchDaemons / systemd)。原 "ZeroTier 先例 (/Library/Application Support)" 依据废弃
- daemon 首启生成全局配置: 以平台默认目录解析生效运行 (无 --state-dir / KUNXTIER_STATE_DIR 覆盖) 且配置文件缺失时, 就地落默认模板 (复用 install 的 load_or_default 逻辑); 显式覆盖 state 目录时不触碰全局配置 (本地开发不应写 /etc)
- daemon 启动 EACCES 统一映射指引: 提示前台运行需 sudo (TUN 特权)、本地开发可 `--state-dir`、生产走 `sudo kunxtier install` 三条出路, 消除裸 io 错误
- 时机零成本: MVP 未发布, 旧路径目录从未成功创建, 无迁移负担

## 范围

- kunxtier-core `config.rs` 三处平台常量 (node_config_path / node_state_dir_default / node_log_dir_default) macOS 分支改为 unix 布局, 单测平台断言同步
- kunxtier-node daemon 启动序列: 目录创建失败 EACCES 映射指引; 默认目录运行时全局配置首启落盘
- install / uninstall 与服务化测试中的路径断言同步; CLAUDE.md 命令示例、arch-kunxtier-overview "服务化与全局配置"、impl-kunxtier-runtime-layout 布局表同步

## 非目标

- Windows 路径 (归 FEAT-kunxtier-core-runtime T13, 届时一并定稿)
- relay 侧布局 (data_dir CWD 相对) 是否并入机器级统一, 仍留独立切片
- `~/.kunxtier` 用户级默认与免特权用户态运行模式

## 验收口径

- macOS 非 root 裸跑 `kunxtier daemon`: 报错含明确指引 (sudo / --state-dir / install), 不再出现裸 os error
- macOS `sudo kunxtier daemon` 首启: `/etc/kunxtier/config.toml` 与 `/var/lib/kunxtier` 就地生成, config.toml 0644, 二次启动不重复生成
- `--state-dir` 显式覆盖时不写任何平台默认路径
- install / 服务定义 / status 指引在新路径下全链可用, 全量测试通过

## 推进记录

- 2026-10-27 (ik-advance): 交付全部四项。T01: core `config.rs` 三处平台常量 macOS 分支并入 unix 统一分支 (/etc/kunxtier + /var/lib/kunxtier + /var/log/kunxtier), 平台断言合并为单一 unix 块。T02: `NodeFileConfig::ensure_written` (缺失落默认模板 0644, 已存在不动, 非法即报错; 复用 install 的 load_or_default), daemon 子命令以平台默认目录形态 (无 CLI/env 覆盖) 启动时接线调用。T03: `daemon_start_fail` 与 `daemon_permission_hint`——daemon 启动路径 (全局配置写入与 serve) 的 PermissionDenied 统一映射 sudo 前台 / --state-dir 本地开发 / sudo install 服务化三条出路。T04: service.rs macOS fixture 路径、CLAUDE.md 节点命令注释、requirements feat-kunxtier-cli 布局表与生成时机口径、arch-kunxtier-overview 服务化小节与取舍表、impl-kunxtier-runtime-layout 机器布局树与配置生成时机同步。实施期两项顺带修正: 其一, 空 KUNXTIER_STATE_DIR 经 clap 折叠为 Some("") 会绕过 "空 env 视同未设置" 语义 (cli 层先于 resolve 生效), node_global 助手内统一归一覆盖全部节点子命令; 其二, kunxtier-core 补 dev-dep tempfile (工作区既有版本, 测试隔离用)。测试: ensure_written 生成/保留/拒非法三用例、daemon 错误映射两用例、平台默认合并断言; 全量 129 项通过 (cargo make 含 fmt/clippy), 非 root 裸跑 daemon 冒烟输出指引文本且 exit 2、status 默认目录解析至 /var/lib/kunxtier。已知边界: sudo 前台首启生成 /etc/kunxtier/config.toml 的真机冒烟留待用户执行 (会话无 sudo); 二进制 install 冒烟同 T14 现状。
- 2026-10-27 (ik-commit 预检后补强, 用户要求完善留存机制): 新增 core::atomic_file::write_atomic (同目录 tmp + fsync + rename, 失败清理 tmp 目标不动, 残留 tmp 先移除保证 mode 生效), 全局配置生成 (ensure_written) / install 服务定义写入 (service write_file) / node.json 保存 (state save, 原内联逻辑收编) 三条写路径归一, 消除非原子写半截文件风险。ensure_written 补异常用例: 已存在不可读即报错、目标是目录即报错、空文件视为已存在不生成不改写; atomic_file 补嵌套创建 / 原子覆盖 / 残留 tmp 顶替三用例。留存语义 (只生成不修改 / 异常即拒绝 / 原子落盘 / 删除后重建) 补入 impl-kunxtier-runtime-layout。真机验证: 用户确认 sudo daemon 首启生成与新路径运行正常。
