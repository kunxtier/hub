# 日常管理 (admin CLI 与 Web 控制台)

管理操作通过指纹固定的 HTTPS 调用核心 admin API, 有两个入口:

- **`kunxtier admin`**: 命令行, 适合脚本与自动化
- **Web 控制台**: 浏览器页面, 适合日常查看与操作

## 认证与连接

每个 admin 子命令需要三要素:

```sh
export KUNXTIER_ADMIN=<admin 凭证>   # 只认环境变量, 无对应 flag
kunxtier admin --server <核心地址> --fingerprint <核心指纹> \
  network list
```

| 要素 | 来源 | 说明 |
|---|---|---|
| 凭证 | `KUNXTIER_ADMIN` | 首启输出或 admin-credential 文件 |
| 核心地址 | `--server` (必填 flag) | host:port, 无环境变量 |
| 指纹 | `--fingerprint` | 也可用 `KUNXTIER_FINGERPRINT` |

指纹即核心自签证书的 SHA256, 用于防中间人。

## 网络组管理

```sh
kunxtier admin ... network list
kunxtier admin ... network create <name> [--cidr 100.66.0.0/16]
```

- **网段池**: 100.64.0.0/10 (CGNAT 段),
  每网络组一个 /16
- **cidr 缺省**: 按创建顺序自动取下一个可用 /16
- **命名规则**: 1-32 位字母数字与 `-` `_`
- 首启自动创建的网络组名为 `default` (100.64.0.0/16)
- 网络组不支持删除

## 用户管理 (过渡期)

> 注意: 用户 (属主) 模型已在移除计划中,
> 届时本节命令与用户界面整体调整。
> 过渡期行为以本文为准,
> 详见 [capabilities.md](capabilities.md)。

```sh
kunxtier admin ... user list [--network <网络>]
kunxtier admin ... user create --network <网络> --name <用户>
kunxtier admin ... user disable --network <网络> --name <用户>
kunxtier admin ... user enable --network <网络> --name <用户>
kunxtier admin ... user rename --network <网络> --name <旧名> <新名>
kunxtier admin ... user remove --network <网络> --name <用户>
```

- **disable**: 名下令牌全部失效
- **remove**: 名下有节点时拒绝
- **rename**: 节点与令牌的属主标注随之传播

用户是网络内令牌与节点的属主标注, 用于批量停用与归属过滤。
最简用法可以完全跳过用户: 直接对 `default` 网络签发令牌。

## 令牌管理

```sh
kunxtier admin ... token issue \
  --network <网络> [--user <用户>] [--type relay] [--ttl <秒>]
kunxtier admin ... token revoke <id>
```

| 令牌类型 | 前缀 | 语义 |
|---|---|---|
| 节点令牌 (缺省) | `kxt_node_` | 一次性, 注册一个节点 |
| relay 令牌 (`--type relay`) | `kxt_relay_` | 可复用, 供卫星注册 |

要点:

- **明文仅展示一次**: 签发输出的令牌明文不再可查,
  丢失只能撤销重发
- **网络绑定**: 令牌只能用于签发时指定的网络组
- **ttl**: 有效期秒数, 允许 60 至 2592000,
  缺省 86400 (1 天)
- 撤销按签发输出中的 token id (`token_id` 字段)

## 节点管理

```sh
kunxtier admin ... node list [--network <网络>]
kunxtier admin ... node remove --network <网络> --name <节点>
kunxtier admin ... node policy --network <网络> --name <节点> \
  <auto|force_relay>
```

- **list**: 含节点属主、在线状态与当前路径策略
- **remove**: 移除后节点凭证失效,
  需重新签发令牌注册
- **policy**: 路径策略;
  `auto` 直连优先回落中继 (默认),
  `force_relay` 强制全程中继

## Web 控制台

两种等价入口, 页面功能相同:

- **核心直接托管**: 浏览器访问核心中继地址即可
  (控制台以 fallback 路由托管)
- **本地 console 命令**: 本地起 HTTP 页面并反代核心 admin API,
  适合核心地址不便直连浏览器的场景

```sh
kunxtier console --relay <核心地址> --fingerprint <指纹> \
  [--listen 127.0.0.1:7600]
```

`--relay` / `--fingerprint` 可由
`KUNXTIER_CORE` / `KUNXTIER_FINGERPRINT` 提供。
页面登录使用 admin 凭证。

控制台页面为编译期嵌入 relay 二进制,
更新前端产物后需重新编译并重启进程生效。
