# kunxtier 运行时机器布局 (配置 / 存储 / 日志)

## 背景与目标

kunxtier 在一台机器上以四种进程角色运行 (relay 核心模式 / relay 卫星模式 / daemon / console), 外加 admin 管理命令。各角色的配置来源、落盘位置、权限与生命周期此前散落在 arch-kunxtier-overview 的服务化 / 可观测性小节与各 issue 推进记录中, 缺一份面向部署与排障的单一参考。

本文回答: 配置有哪些、从哪几层合并而来; 机器上什么位置有什么文件、权限如何、谁生成、卸载后留什么; 日志写到哪里、怎么轮转、排障视图从哪来。Windows 路径为占位, 随平台切片 (T13) 定稿。

## 方案概览

机器产物按角色划分, 三条不变式贯穿全部设计:

1. 敏感量边界: admin 凭证 / relay 令牌 / 节点一次性令牌 / WG 私钥 / 节点凭证明文, 只存在于 env、CLI 参数或 0600 文件, 不进任何 toml、不进日志、不进服务单元
2. 单一事实源: 节点目录定位只由全局配置承载, daemon / up / down / status / install / uninstall 六命令统一读取; 服务单元不复制 state_dir
3. 无状态角色不落盘: 卫星零存储 (TLS 证书每次启动内存生成), console 零落盘

### 节点 (daemon) 机器布局

macOS / Linux 同构 (unix daemon 惯例; 2026-10-27 布局修订, 原 macOS /Library/Application Support 与配置同置布局废弃, 见 OPT-kunxtier-node-unix-layout):

```text
/etc/kunxtier/config.toml                  # 0644  机器级全局配置 (install 或 daemon 首启生成; 全非敏感)
/var/lib/kunxtier/                         # 目录 0755
├── node.json                              # 0600  注册身份: 节点凭证 + WG 私钥 (不离机)
└── daemon.sock                            # 0666  IPC socket (daemon 运行期存在, 停即清理)
/var/log/kunxtier/daemon.log[.old]         # 服务化日志 (install 固化 --log-file)
服务定义 (install 生成, 0644): macOS /Library/LaunchDaemons/com.kunxtier.daemon.plist
                               Linux /etc/systemd/system/kunxtier.service
```

卸载 (uninstall) 只删服务定义, state / 全局配置 / 日志保留; 彻底移除节点身份 = 删 state 目录。Windows 占位: config `./kunxtier-config.toml`, state `./kunxtier-node-state`, 日志 `./kunxtier-logs`。

### relay 核心模式

```text
<data_dir>/            # 默认 ./kunxtier-state (CWD 相对); 目录 0700
├── state.db           # SQLite 管理数据 (schema 版本见 "存储设计")
├── key.pem            # 0600  自签 TLS 私钥
├── cert.pem           # 0644  自签证书
└── admin-credential   # 0600  admin 凭证 (首启 stderr 展示一次, 之后在此找回)
```

relay.toml 经 `--config` 显式指定 (无隐式默认路径, 位置不限); 日志文件经 `--log-file` 显式指定。首次运行未初始化时自动就地进行 init (生成上述全部文件), `relay --init` 保留为显式预初始化入口。

### relay 卫星模式 / console

卫星无状态目录、无管理数据, 注册凭 KUNXTIER_RELAY_TOKEN (仅 env)。console 子命令本地起 HTTP 进程托管嵌入资源并反代 admin API, 无任何落盘。

## 配置设计

统一分层 CLI > ENV > CONFIG > DEFAULT; 空 env 视同未设置; flag 与 CONFIG 层非法值启动即报错; CONFIG 文件缺失按全默认处理, 不视为错误。

### relay.toml (--config 显式指定, 全非敏感)

| 字段 | 类型 | 默认 | 语义 |
| ---- | ---- | ---- | ---- |
| listen | SocketAddr | 0.0.0.0:443 | 管理 / 信令 / 转发 / 控制台共用监听 |
| data_dir | 路径 | ./kunxtier-state | 状态目录 (核心模式) |
| advertise | SocketAddr | 无 | 对外公布地址 (中继列表内置条目; 缺省取具体监听地址) |
| stun_servers | [host:port] | 公共 STUN 三台 | 整组覆盖缺省列表, 经信令下发节点 |
| log_level | error..trace | 无 (即 info) | 进程级关注点, 不进运行时配置类型 |

卫星模式复用 listen / advertise 两字段。

### 节点全局配置 (机器固定路径, install 或 daemon 首启生成默认模板)

| 字段 | 类型 | 默认 | 语义 |
| ---- | ---- | ---- | ---- |
| state_dir | 路径 | 平台默认 state 目录 | 六命令统一读取 |
| log_dir | 路径 | 平台日志目录 | install 据此固化 --log-file; CONFIG > DEFAULT, 无 CLI / env 覆盖 |
| log_level | error..trace | 无 (即 info) | 日志级别链的 CONFIG 层 |

生成时机: daemon 以平台默认目录形态 (无 --state-dir / KUNXTIER_STATE_DIR 覆盖) 启动且文件缺失时, 就地落默认模板 (0644); 显式覆盖 state 目录属本地开发, 不触碰机器级路径。非 root 启动在机器级目录遇权限不足时, 映射 sudo 前台运行 / `--state-dir` / `sudo kunxtier install` 三条出路指引, 不透出裸 io 错误。

留存机制 (程序对全局配置的完整生命周期语义):

- 只生成不修改: 配置是用户资产, 程序仅在文件缺失时生成, 已存在 (含空文件, 空即全默认) 一律不动, 不存在程序侧更新 / 删除路径
- 异常即拒绝: 已存在但非法 (解析失败) / 不可读 / 目标被目录占用, 启动即报错, 不静默重建绕过用户配置
- 原子落盘: 写入走同目录临时文件 + fsync + rename (core::atomic_file, 与 node.json、服务定义写入同源), 中断只可能残留 `.tmp`, 不产生半截文件
- 恢复: 用户删除配置后下次启动按缺失重新生成默认模板; `.tmp` 残留由下次写入前清理, 不参与读取

### 环境变量清单

| 键 | 用途 | 适用角色 | 敏感 |
| -- | -- | -- | -- |
| KUNXTIER_LISTEN | 监听地址 | relay 两模式 | 否 |
| KUNXTIER_DATA_DIR | 状态目录 | relay 核心 | 否 |
| KUNXTIER_ADVERTISE | 公布地址 | relay 两模式 | 否 |
| KUNXTIER_STUN | STUN 列表 (逗号分隔, 整组覆盖) | relay 核心 | 否 |
| KUNXTIER_CORE | 核心中继地址 | 卫星 / up | 否 |
| KUNXTIER_RELAY_TOKEN | relay 注册令牌 | 卫星 | 是 (仅 env, 不接受 CLI / 文件) |
| KUNXTIER_FINGERPRINT | 核心证书指纹 hex | 卫星 / up / admin / console | 否 |
| KUNXTIER_STATE_DIR | 节点 state 目录 | 节点六命令 | 否 |
| KUNXTIER_TOKEN | 一次性注册令牌 | up | 是 |
| KUNXTIER_ADMIN | admin 凭证 | admin CLI | 是 |

### 日志级别链

flag > CONFIG 层 > RUST_LOG > 默认 info, 两角色相同, 仅 CONFIG 层来源不同 (relay 取 relay.toml, daemon 取全局配置)。flag 与 CONFIG 是显式配置, 非法值即报错; RUST_LOG 透传 EnvFilter 指令语义 (支持 `kunxtier_node=debug` 模块过滤)。daemon 的 CONFIG 层在日志装配期预读容错, execute 内正式读取时报错。

## 存储设计

### relay 核心: SQLite 单库

- 全部管理数据 (网络组 / 用户 / 令牌 / 节点 / IP 分配 / 策略) 在 `<data_dir>/state.db` 单库内, 不引入外部数据库
- schema 版本记于 `PRAGMA user_version` (当前 2): 启动时低于程序版本自动前向迁移, 高于程序版本拒绝启动 (防降级写坏)
- 备份: `relay backup <out>` 以 `VACUUM INTO` 导出一致性快照; 恢复 = 快照换库后重启
- TLS 私钥 / 证书 / admin 凭证独立于库外成文件, 库与凭证同处 0700 目录

### 节点: node.json 单文件

- 字段: version (当前 1) / network / name / vip / credential / core / fingerprint / listen_port / wg_private_key
- 版本不符直接拒读 (防跨版本误用); 写入原子化 (临时文件 + rename)
- 双明文凭证的信任边界: 节点凭证与 WG 私钥明文只在本机 node.json (0600), 不离机; relay 侧只存凭证哈希, 信令走挑战应答, 明文不过网
- 权限模型: state 目录 0755 (socket 需本机用户可达), 敏感收敛到文件级 0600, daemon.sock 显式 0666; 信任域 = 本机已登录用户——up 仍需一次性令牌, 私钥不出 0600 信任域
- 生命周期: 首次 up 生成; down 保留 (断会话不清身份); daemon 启动见 state 即自动上线; daemon.sock 随 daemon 生死, 启动时残留 socket 探活清理

## 日志设计

- 统一 tracing 埋点, subscriber 进程仅装配一次 (kunxtier-cli main, 四角色共用)
- 输出目标: 默认 stderr; `--log-file` 显式启用后 stderr / 文件双写; 服务化由 install 固化 `--log-file <log_dir>/daemon.log`, Linux 下 stderr 旁路进 journald
- 轮转: 单文件 10MB, 轮转为 `<path>.old` 只保留一份; rename 失败退化为丢弃重建, 不中断日志链路
- 节点事件环: 内存环形缓冲容量 256, 只记打洞 / 路径切换 / 信令 / 注册四类, 事件点与 tracing 同位双写; `status --verbose` 经 IPC 拉取; 进程重启即清零, 历史回看依赖日志文件
- relay 侧不建事件环, 五类关键事件 (注册被拒 / 凭证吊销 / 令牌校验失败 / 卫星注册与摘除 / admin 认证失败) 走日志留痕
- 脱敏: 按 NFR-4 审计, 令牌 / 私钥 / 凭证一律不进日志

## 设计原则与约束

- 配置分层顺序全局固定, 敏感字段只出现在 env / CLI / 0600 文件
- 目录与权限统一 unix daemon 惯例 (macOS / Linux 同构 /etc + /var/lib + /var/log; macOS 原生路径已弃用)
- relay 与节点是两套独立配置体系 (relay.toml 显式指定 vs 节点全局配置机器固定), 是否统一机器级布局留独立切片评估
- 服务定义只固化二进制绝对路径与 --log-file, 其余运行时读配置, 改配置无需重生成单元 (改 log_dir 除外, 需重跑 install)

## 取舍与风险

| 主题 | 结论 | 影响 |
| ---- | ---- | ---- |
| relay data_dir 默认 CWD 相对 vs 平台固定 | CWD 相对 (与节点不对齐) | 部署者自选位置更自由; 代价是服务化托管 relay 时须显式传目录, 统一机器级布局留独立切片 |
| macOS 机器布局: /Library/Application Support (ZeroTier 先例) vs unix 统一 /etc + /var | 统一 unix 布局 (2026-10-27 修订) | 终端应用 + 特权服务气质一致, 两平台一套心智; 代价是弃 Finder 可达性与 ZeroTier 先例依据 |
| daemon 首启生成全局配置 vs 仅 install 生成 | 首启生成 (平台默认目录形态) | 免记忆前置命令, 与 relay 首启自动初始化对称; 代价是 daemon 启动路径多一处写盘 (已存在则不动) |
| node.json 单文件 vs 凭证 / 私钥拆分多文件 | 单文件 | 原子写保证整体一致, 版本化简单; 代价是权限策略只能文件级统一 0600 |
| 明文凭证 0600 vs 加密存储 | 明文 + 文件权限 | 不引入口令 / KEX 体系; 代价是拿到文件即拿到身份, 依赖 OS 文件权限与磁盘加密兜底 |
| IPC socket 0666 vs 0660 管理组 | 0666 (本机用户信任域) | 普通用户免提权可用 up / down / status; 代价是本机任意用户可断开会话 |
| 单份 .old vs 多代轮转 | 单份 (最轻手段) | 不引入轮转库; 代价是只有一段历史, 更久回看靠 journald / 自行重定向 |
| 事件环内存 vs 落盘 | 内存 (容量 256) | status --verbose 免解析即取、内存有界; 代价是重启清零 |
| install 显式生成默认配置 vs 全隐式默认 | 显式生成 (0644) | 运维可见可改; 代价是多一个落盘动作与幂等语义要维护 |
| relay 首启自动初始化 vs 强制 --init | 自动 (init 保留为显式入口) | 免记忆前置命令; 代价是误起进程即在 CWD 建目录 (数据目录可预期, 接受) |
