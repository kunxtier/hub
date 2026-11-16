# 配置参考

kunxtier 的配置来自三层, 优先级从高到低:

```text
环境变量 (ENV) > 配置文件 (TOML) > 内置默认值
```

- **空值穿透**: 环境变量设为空串视同未设置,
  继续向下取值
- **无 `--config` 入口**: 配置文件位置固定,
  不提供命令行指定路径
- **敏感值只走 env**: admin 凭证与 relay 令牌
  永不写入配置文件, 见文末边界说明

## 配置文件位置

| 文件 | 角色 | 路径 (macOS / Linux) |
|---|---|---|
| relay.toml | 中继 (核心与卫星) | `/etc/kunxtier/relay.toml` |
| config.toml | 节点 (daemon 等) | `/etc/kunxtier/config.toml` |

两个文件同目录但各管一个角色, 不合并。
文件缺失不是错误, 全部按内置默认值运行。

生成默认模板:

```sh
kunxtier config init              # 两域都生成 (缺哪个生成哪个)
kunxtier config init --relay      # 只生成 relay.toml
kunxtier config init --node       # 只生成 config.toml
```

已存在不改写; 机器级路径通常需要 `sudo`。

## 查看生效配置

```sh
kunxtier config
```

输出三段: 配置文件位置与存在性、
relay 域与节点域的合并终值、敏感环境变量存在性。
终值与实际运行装配同源, 所见即所跑。

## 环境变量

| 变量 | 作用域 | 缺省 | 说明 |
|---|---|---|---|
| `KUNXTIER_LISTEN` | relay | `0.0.0.0:443` | 中继监听地址 |
| `KUNXTIER_DATA_DIR` | relay 核心 | `./kunxtier-state` | 核心 state 目录 |
| `KUNXTIER_ADVERTISE` | relay | 无 | 对外公布地址 |
| `KUNXTIER_STUN` | relay | 内置公共列表 | STUN 列表, 逗号分隔整组覆盖 |
| `KUNXTIER_RELAY_TOKEN` | relay | 无 | 卫星令牌, 非空即卫星模式 |
| `KUNXTIER_CORE` | up / console / 卫星 | 无 | 核心地址 host:port |
| `KUNXTIER_FINGERPRINT` | up / admin / console | 无 | 核心证书 SHA256 指纹 |
| `KUNXTIER_ADMIN` | admin | 无 | admin 凭证, 仅 env |
| `KUNXTIER_TOKEN` | up | 无 | 一次性节点注册令牌 |
| `KUNXTIER_STATE_DIR` | 节点 | `/var/lib/kunxtier` | 节点 state 目录 |
| `KUNXTIER_LOG_FORMAT` | relay / daemon | `text` | 日志格式 text / json |
| `RUST_LOG` | relay / daemon | `info` | 日志级别透传 |

`KUNXTIER_FINGERPRINT` 同样作用于卫星中继。
注意: `admin --server` 只接受命令行参数, 无环境变量。

`RUST_LOG` 支持模块过滤, 如 `kunxtier_node=debug`。

## relay.toml 字段

| 字段 | 类型 | 说明 |
|---|---|---|
| `listen` | string | 监听地址, 同 `KUNXTIER_LISTEN` |
| `data_dir` | string | state 目录, 同 `KUNXTIER_DATA_DIR` |
| `advertise` | string | 对外公布地址, 同 `KUNXTIER_ADVERTISE` |
| `core` | string | 卫星专用: 核心拨号地址 |
| `fingerprint` | string | 卫星专用: 核心证书指纹 |
| `stun_servers` | [string] | STUN 列表, 整组覆盖默认 |
| `log_level` | string | error / warn / info / debug / trace |

默认模板 (listen 与 data_dir 显式落盘, 其余注释示例):

```toml
listen = "0.0.0.0:443"
data_dir = "./kunxtier-state"
# advertise = "1.2.3.4:443"
# core = "core.example.com:443"
# fingerprint = "<核心证书指纹 hex>"
# stun_servers = ["stun.l.google.com:19302"]
# log_level = "info"
```

## config.toml 字段

| 字段 | 类型 | 缺省 | 说明 |
|---|---|---|---|
| `state_dir` | string | `/var/lib/kunxtier` | 节点 state 目录 |
| `log_dir` | string | `/var/log/kunxtier` | 节点日志目录 |
| `log_level` | string | 未设置 (info) | daemon 日志级别 |

state 目录承载节点身份与 IPC socket。
`state_dir` 可被 `KUNXTIER_STATE_DIR` 覆盖;
`log_dir` 与 `log_level` 只认配置文件。

## 日志级别链

长驻进程 (relay / daemon) 的日志级别
按以下顺序取第一个非空值:

```text
--log-level flag > 配置文件 log_level > RUST_LOG > info
```

`--log-file <path>` 启用后 stderr 与文件双写,
单文件 10 MiB 轮转, 保留一份 `.old`。

## 敏感值边界

- **admin 凭证** (`kxt_admin_` 前缀):
  只走 `KUNXTIER_ADMIN`;
  首启同时落盘 state 目录 `admin-credential` (0600)
- **relay 令牌** (`kxt_relay_` 前缀):
  只走 `KUNXTIER_RELAY_TOKEN`,
  不接受配置文件来源
- **节点令牌** (`kxt_node_` 前缀): 一次性使用,
  `up` 时经 flag 或 `KUNXTIER_TOKEN` 传入
- `kunxtier config` 对敏感值只输出存在性, 不回显内容
