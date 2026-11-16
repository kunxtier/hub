# 中继部署 (核心与卫星)

kunxtier 的中继有两种模式, 由同一个 `kunxtier relay` 命令承担:

| 模式 | 职责 | 状态 |
|---|---|---|
| 核心 | 信令、管理 API、内置转发、控制台托管 | 单机已验证 |
| 卫星 | 纯转发, 向核心注册, 无本地管理数据 | 真机部署待验证 |

模式判定只有一条:
`KUNXTIER_RELAY_TOKEN` 非空即卫星模式, 未设置即核心模式。
没有独立的模式开关;
卫星模式下 `data_dir` 等核心专属配置被忽略。

## 部署核心中继

裸命令零配置启动, 首次启动自动完成初始化:

```sh
kunxtier relay
```

缺省监听 `0.0.0.0:443`, state 目录 `./kunxtier-state`。
本地试跑建议指定高位端口, 避免 443 权限问题:

```sh
KUNXTIER_LISTEN=127.0.0.1:9443 \
KUNXTIER_DATA_DIR=./kunxtier-state kunxtier relay
```

### 首次启动产出

首启在 state 目录 (权限 0700) 内生成:

| 文件 | 权限 | 说明 |
|---|---|---|
| `cert.pem` / `key.pem` | 0644 / 0600 | 自签证书与私钥 |
| `state.db` | - | SQLite 数据库 |
| `admin-credential` | 0600 | admin 凭证明文, 找回用 |
| `init.lock` | - | 初始化互斥锁, 常驻 |

同时创建默认网络组 `default`, 网段 100.64.0.0/16。

首启输出 (stderr) 包含两项关键信息:

- **admin 凭证**: 前缀 `kxt_admin_`。
  终端运行时回显一次;
  服务化运行 (无终端) 只落文件不回显
- **证书指纹**: SHA256 hex,
  所有客户端 (up / admin / console / 卫星) 固定使用

凭证丢失后读 state 目录 `admin-credential` 文件找回。
重复启动已初始化的 state 目录不会重复初始化, 也不覆盖证书。

## 部署卫星中继

卫星扩展核心的转发容量, 不承载信令与管理。
前提: 核心已运行, 并在核心侧签发可复用的 relay 令牌:

```sh
export KUNXTIER_ADMIN=<admin 凭证>
kunxtier admin --server <核心地址> --fingerprint <核心指纹> \
  token issue --network default --type relay
```

卫星侧设置四个环境变量启动:

```sh
KUNXTIER_RELAY_TOKEN=<relay 令牌> \
KUNXTIER_CORE=<核心地址> \
KUNXTIER_FINGERPRINT=<核心指纹> \
KUNXTIER_ADVERTISE=<卫星对外地址> \
kunxtier relay
```

- **令牌非空即卫星模式**: 这是唯一的模式开关
- **advertise 必填**: 卫星需要可被节点拨号的对外地址;
  缺省回退具体 (非 0.0.0.0) 的监听地址
- **无本地状态**: 卫星不写 state 目录,
  `relay backup` 在卫星模式下直接报错
- 卫星注册后由核心分发给节点, 断连自动摘除

注意: 卫星部署目前仅通过代码层验证,
公网真机场景尚未验证 (见 [capabilities.md](capabilities.md))。

## 数据与备份

核心的 state 目录承载全部持久状态,
其中 `state.db` 支持一致性快照导出:

```sh
kunxtier relay backup ./backup.db
```

- 导出使用 SQLite `VACUUM INTO`, 不阻塞运行中的服务
- 备份调度未自动化, 定期备份需自行接 cron 或 timer
- schema 版本随版本自动前向迁移, 旧备份可直接启动

## 日志

```sh
kunxtier relay --log-level debug   # 覆盖 relay.toml 与 RUST_LOG
kunxtier relay --log-file /var/log/kunxtier/relay.log
```

- `--log-file` 启用后 stderr 与文件双写,
  单文件 10 MiB 轮转保留一份 `.old`
- `KUNXTIER_LOG_FORMAT=json` 切换结构化输出供采集
- 其余配置项走 env 与 relay.toml, 见 [config.md](config.md)

## 生产化注意

- **443 端口**: 缺省监听 443 需要 root 或能力授权。
  用高位端口可绕开, 客户端连接地址带端口即可
- **托管**: relay 常驻建议交给系统服务管理器。
  `kunxtier install` 注册的是节点 daemon,
  中继的服务化文件需自行编写
- **证书**: 自签 + 指纹固定, 不支持 ACME 自动证书
- **备份**: state 目录含 TLS 私钥与全部管理数据,
  备份文件按敏感文件保管, 严禁外传
