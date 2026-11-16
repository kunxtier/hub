# kunxtier

轻量 P2P 虚拟组网工具 (ZeroTier / EasyTier 平替)。
单一二进制承担全部角色:
核心中继、卫星中继、节点守护进程、Web 控制台都是其子命令。

- **节点互通**: 自动打洞直连, 失败回落中继;
  WireGuard 用户态加密, 无需内核模块
- **零配置启动**: 核心中继裸命令首启自动初始化,
  产出自签证书、admin 凭证与默认网络
- **信任模型**: 自签证书 + 指纹固定, 不依赖第三方 CA;
  敏感凭证只走环境变量
- **多角色扩展**: 卫星中继纯转发扩展容量,
  Web 控制台与 CLI 双管理入口

使用文档见 [docs/](docs/),
能力边界见 [capabilities](docs/capabilities.md)。

## 快速上手 (3 步)

```sh
# 1. 机器 A: 启动核心中继, 记下输出的 admin 凭证与证书指纹
KUNXTIER_LISTEN=0.0.0.0:9443 KUNXTIER_DATA_DIR=./kunxtier-state \
  kunxtier relay

# 2. 机器 A: 签发一次性节点令牌 (kxt_node_xxx)
export KUNXTIER_ADMIN=<admin 凭证>
kunxtier admin --server <核心地址>:9443 --fingerprint <指纹> \
  token issue --network default

# 3. 机器 B: 启动 daemon 并注册 (up/status/down 免 sudo)
sudo KUNXTIER_STATE_DIR=./kunxtier-node-state kunxtier daemon &
KUNXTIER_STATE_DIR=./kunxtier-node-state kunxtier up \
  --token <令牌> --name peer-b --core <核心地址>:9443 \
  --fingerprint <指纹>
```

完整流程与连通验证见 [quick-start](docs/quick-start.md)。

## 能力概览

| 状态 | 能力 |
|---|---|
| 已验证 | 打洞直连 / 中继回落 / 双模式转发 / 令牌体系 / 管理 API |
| 已验证 | Web 控制台 / CLI 全命令面 / macOS + Linux 服务化 / 备份 |
| 待真机验证 | 真实 NAT 打洞 / 卫星公网部署 / systemd 生产托管 |
| 未实现 | Windows / ACME / IPv6 / 子网路由 / DNS / ACL / 多核心 HA |

进行中: 用户 (属主) 模型移除, `admin user` 子命令将随之调整。
逐项说明见 [docs/capabilities.md](docs/capabilities.md)。

## 文档索引

| 文档 | 内容 |
|---|---|
| [quick-start](docs/quick-start.md) | 两台机器三步组网 |
| [relay](docs/relay.md) | 核心与卫星部署 / 备份 / 日志 |
| [node](docs/node.md) | daemon / up / status / 服务化 |
| [admin](docs/admin.md) | admin CLI 与 Web 控制台 |
| [config](docs/config.md) | 配置优先级 / 环境变量 / 文件字段 |
| [capabilities](docs/capabilities.md) | 能力边界与路线图 |
| [troubleshooting](docs/troubleshooting.md) | 排障 / 退出码 / 安全须知 |

## 开发

```sh
cargo build                 # 编译
cargo test                  # 测试
cargo make                  # 提交前检查 (格式 / clippy / 编译 / 测试)
cargo make build            # 先编译控制台前端 (pnpm) 再编译 crate
cargo make --list-all-steps # 全部任务
```

- 控制台前端源码在仓库 `web/kunxtier-console`,
  产物嵌入 relay 二进制, 更新后需重启进程生效
- 运行态目录 `kunxtier-state/` (TLS 私钥与 SQLite) 与
  `kunxtier-node-state/` (节点凭证与 WG 私钥) 已进 .gitignore,
  严禁提交或外传
- 规划文档 (需求 / 架构 / 测试 / issue) 见仓库 `.docs/`
