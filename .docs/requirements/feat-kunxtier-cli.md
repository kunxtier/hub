# kunxtier CLI 与运维形态

## 背景

kunxtier 的全部角色聚合为单一二进制 `kunxtier`, 通过子命令区分: relay (核心中继, 默认; `--satellite` 为纯转发卫星)、console (无状态控制台)、daemon (节点守护进程) 与管理 CLI。交叉编译分发, 目标是零依赖部署: 任何角色都不依赖系统安装的组件 (包括系统 WireGuard 工具链)。relay + daemon 即闭环, 其余子命令是体验增强。

## 目标

- 单二进制覆盖全部角色: 核心中继 (含控制台托管)、卫星中继、无状态控制台、节点守护进程、管理 CLI
- 常用操作 (上线 / 下线 / 查看状态) 一条命令完成; daemon 可注册为系统服务开机自启
- 管理操作 (网络 / 用户 / 令牌 / 节点 / 策略) CLI 与控制台双入口
- 核心中继与卫星均可在裸机器上以 systemd / launchd 直接拉起; 部署位置不限, 硬要求只有目标端可达
- 管理数据与节点 state 具备版本迁移与备份能力, 升级不丢数据

## 用户流程 / 页面结构

### 首次部署核心中继

1. 拷贝二进制到目标机器 (通常是节点都可达的一台公网机器, 但这不是架构约束)
2. `kunxtier relay` 启动: 数据目录未初始化时自动完成初始化 (自签证书 / admin 凭证 / 默认网络组), 凭证同时写入 state 目录 `admin-credential` (0600); 也可先显式执行 `kunxtier relay --init`
3. 由 systemd (Linux) / launchd (macOS) 托管; 浏览器打开即得控制台

### 扩展卫星中继 (可选)

1. 拷贝二进制到中继机器
2. admin 在控制台或 CLI 签发 relay 注册令牌 (可复用)
3. `kunxtier relay --satellite --core <核心地址> --token <relay-token>` 启动, 注册进核心的中继列表

### 使用控制台 (可选)

1. 浏览器直接打开核心中继托管的页面, 或
2. 在任意机器 (如本机) `kunxtier console --relay <核心地址>` 起本地控制台, 能力与直接打开完全一致

### 节点日常使用

1. `kunxtier up --token <token> --name laptop` 首次注册并连接
2. `kunxtier install` 注册系统服务与开机自启 (按平台需要提权); 首次执行生成机器级全局配置 (见 "全局配置与目录布局"); `kunxtier uninstall` 反向清理 (state 与全局配置保留)
3. `kunxtier status` 查看本机与对端状态; `kunxtier status --verbose` 输出最近路径事件与打洞失败原因
4. `kunxtier down` 断开

### 管理员运维

1. `kunxtier admin network / user / token / node / policy / relay <subcommand>` 完成网络、用户、令牌、节点、策略与中继管理
2. `kunxtier admin node list` 查看全网节点概览; `kunxtier admin relay status` 查看卫星中继注册状态与负载

### 数据运维

1. `kunxtier relay backup` 导出一致性备份 (SQLite 快照)
2. 升级二进制后重启, SQLite schema 自动前向迁移, 节点 state 按版本字段兼容读取

### 命令总览

```sh
# ── 核心中继 (一个网络一个, 部署位置不限, 443/tcp) ─────────────
kunxtier relay                                   # 启动核心中继: 管理 + 信令 + 内置转发 + 控制台托管; 首启自动初始化 (凭证写入 state/admin-credential)
kunxtier relay --init                            # (可选) 显式预初始化: 状态目录/自签证书/admin 凭证/默认网络组
kunxtier relay --config /etc/kunxtier/relay.toml # 指定配置文件 (监听地址/数据目录等长期配置)
kunxtier relay backup                            # 导出一致性备份 (SQLite 快照, 用于迁移与恢复)

# ── 卫星中继 (可选, 可多个, 纯转发) ───────────────────────────
kunxtier relay --satellite --core wss://core.example.com --token <relay-token>

# ── 控制台 (可选, 无状态, 随处可起) ───────────────────────────
kunxtier console --relay wss://core.example.com  # 本机起控制台, 与直开核心页面行为等价

# ── 节点 ──────────────────────────────────────────────────────
kunxtier up --token <token> --name laptop        # 首次注册并连接 (令牌一次性, 绑定网络)
kunxtier status                                  # 本机与对端状态 (路径 direct / relay)
kunxtier status --verbose                        # 排障视图: 最近路径事件与打洞失败原因
kunxtier down                                    # 断开
kunxtier daemon                                  # 前台运行守护进程 (通常由 install 的服务拉起; 本地开发需显式 --state-dir)
kunxtier install                                 # 注册系统服务 + 开机自启 (首次生成全局配置; 需 root)
kunxtier uninstall                               # 停服并移除服务注册 (state 与全局配置保留)

# ── 管理 (admin 凭证走 env 或独立文件) ────────────────────────
kunxtier admin network create [--cidr 100.66.0.0/16]  # 建网络组 (地址段默认按创建顺序递增)
kunxtier admin network list                           # 网络组列表
kunxtier admin user create --network <net>            # 在网络内创建用户 (属主标注)
kunxtier admin user disable --user <name>             # 禁用用户 (令牌失效 + 设备凭证吊销)
kunxtier admin token issue --user <name>              # 签发一次性注册令牌 (绑定该用户的网络)
kunxtier admin token issue --type relay               # 签发可复用的卫星注册令牌
kunxtier admin token revoke --token <id>              # 撤销令牌
kunxtier admin node list [--network <net>]            # 节点概览 (属主 / 在线 / 路径)
kunxtier admin node remove --node <name>              # 移除节点 (吊销凭证, 广播下线)
kunxtier admin policy set --node <name> --path forced-relay  # 强制中继
kunxtier admin policy set --node <name> --path auto          # 恢复自动策略
kunxtier admin relay status                           # 卫星中继注册状态与负载
```

以上为规划口径的完整命令面, 与 FR-1 的子命令清单一一对应; 实现期参数命名可微调, 以 `kunxtier --help` 为最终事实。

### 全局配置与目录布局

节点侧配置与存储全局化管理: 机器级全局配置文件承载存储位置与日志级别, daemon / up / down / status / install / uninstall 读取同一份; 敏感字段 (注册令牌 / 指纹 / relay 令牌) 不进配置文件, 仍只走 env。

| 平台 | 全局配置 | state 默认 | 日志默认 |
| ---- | -------- | ---------- | -------- |
| Linux / macOS | /etc/kunxtier/config.toml | /var/lib/kunxtier | /var/log/kunxtier |
| Windows | %ProgramData%\kunxtier\config.toml (随平台切片) | — | — |

- 配置字段: `state_dir` / `log_dir` / `log_level`; 同一字段优先级 CLI flag > 环境变量 > 全局配置 > 平台默认 (FR-7)
- 配置文件缺失时全默认可用; install 或 daemon 首启 (平台默认目录形态, 无 CLI / env 覆盖) 以默认值显式生成 (0644, 可读可改), 已存在则只校验不改写
- daemon 非 root 前台启动在机器级目录遇权限不足时, 明确指引 sudo 前台运行 / `--state-dir` 本地开发 / `sudo kunxtier install` 服务化三条出路
- 服务定义只固化二进制路径与日志文件位置 (平台日志目录下应用子目录), state 目录运行时读全局配置, 保持单一事实源
- IPC socket 位于 state 目录; 服务以 root 运行时目录 0755、身份与私钥文件 0600、socket 0666——本机已登录用户即可执行 up / down / status (up 仍需令牌), 私钥不出信任域
- 本地开发 (免提权) 跑 daemon 时需显式 `--state-dir` 指向工作目录

## 功能要求

| 编号 | 说明 | 验收标准 |
| ---- | ---- | -------- |
| FR-1 | 子命令覆盖 relay / console / daemon / up / down / status / admin / install / uninstall | `kunxtier --help` 可发现全部子命令, 无需额外记忆 |
| FR-2 | up / down / status 与本机 daemon 通过本地 IPC 通信 | daemon 未运行时给出明确提示与启动指引 |
| FR-3 | relay 核心模式单进程承载多网络组管理、信令、内置转发与控制台托管 | 无需拆分部署即可获得完整闭环 (含打洞回落路径); init 自动创建默认网络组 |
| FR-4 | relay 卫星模式为纯转发角色, 以可复用 relay 令牌向核心注册 | 注册后自动进入全网中继列表, 无需改核心配置, 无本地管理数据 |
| FR-5 | console 子命令为无状态控制台宿主 | 在任意机器启动, 连接核心 admin API, 行为与直接访问核心托管页面等价 |
| FR-6 | 首次初始化自动生成密钥材料 | 生成结果落在 state 目录并设置严格文件权限 (0600) |
| FR-7 | 配置遵循 CLI > ENV > CONFIG > DEFAULT 分层 | 同一字段多层出现时按该优先级生效, 行为有单元测试覆盖 |
| FR-8 | macOS / Linux / Windows 三平台发布产物 | 提供可交叉编译的统一构建入口 |
| FR-9 | daemon 服务化: install / uninstall 注册系统服务 (macOS launchd / Linux systemd) 与开机自启 | 系统重启后 daemon 自动恢复并上线; uninstall 停服并清理服务定义, state 与全局配置保留; 三平台特权要求 (Windows 管理员 / macOS root / Linux root) 在文档与安装提示中明确 |
| FR-10 | 数据与状态可迁移可备份: SQLite 带 schema 版本并只做前向迁移; 节点 state 文件带版本字段; `relay backup` 导出一致性备份 | 新版本二进制打开旧库自动迁移不丢数据; 备份文件可在新机恢复出完整网络状态 |
| FR-11 | 节点侧机器级全局配置承载 state_dir / log_dir / log_level (见 "全局配置与目录布局") | 配置缺失全默认可用; install / daemon / up / down / status 读取一致; 优先级与非法值拒绝行为有单元测试覆盖 |

## 边界与约束

- Web 控制台的功能需求由 feat-kunxtier-console 承载, 本文档只约定入口与部署形态
- daemon 服务化只覆盖 daemon 角色; relay / console / 卫星由系统单元 (systemd / launchd) 直接托管
- relay 配置维持独立 relay.toml 分层, 不并入机器级全局配置; 两者统一作为独立演进项评估
- Windows 的服务注册随 Windows 平台支持落地, 现阶段 install 在 Windows 明确报错占位
- relay 的验证环境为 Linux; macOS 声明支持, Windows 上运行 relay 不在 MVP 验证范围
- 核心中继监听地址、数据目录等长期配置走配置文件; TLS 私钥、admin 凭证、relay 令牌走 env 或独立文件

## 非目标

- 包管理器分发渠道 (brew / apt 等), 首发直接分发二进制
- 移动端与图形客户端
- 多核心聚合管理入口
- 自动化备份调度与异地容灾 (backup 命令提供能力, 调度由用户自行安排)
