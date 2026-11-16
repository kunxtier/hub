# 节点接入

节点侧命令分为两类:

- **daemon**: 常驻守护进程, 持有 TUN 设备与组网会话,
  是 up / down / status 的服务端
- **up / down / status**: 经本地 IPC socket 的薄客户端,
  daemon 未运行时给出启动指引

daemon 需要 root (TUN 特权与机器级目录); 三个薄客户端不需要。
IPC socket 权限 0666, 普通用户即可执行 up / down / status。

## 启动 daemon

本地开发直接前台运行:

```sh
sudo KUNXTIER_STATE_DIR=./kunxtier-node-state kunxtier daemon &
```

- **`--listen-port`**: WireGuard 传输层固定监听端口;
  缺省首次注册时自动选择并随 state 持久化
- **`KUNXTIER_STATE_DIR`**: 节点 state 目录
  (身份凭证 / WG 私钥 / IPC socket),
  缺省 `/var/lib/kunxtier`;
  本地开发指向工作目录可绕开机器级路径
- 首次启动自动生成全局配置 `/etc/kunxtier/config.toml`
  (已存在不改写)
- 权限不足时输出提权指引:
  `sudo kunxtier daemon` 或 `kunxtier install` 注册系统服务

生产环境建议开机自启, 见文末 install 小节。

## 加入网络 (up)

```sh
KUNXTIER_STATE_DIR=./kunxtier-node-state kunxtier up \
  --token <一次性令牌> \
  --name laptop \
  --core <核心地址> \
  --fingerprint <核心指纹>
```

| 参数 | 必填性 | 环境变量 | 说明 |
|---|---|---|---|
| `--name` | 必填 | 无 | 节点名, 网络内唯一 |
| `--token` | 首次必填 | `KUNXTIER_TOKEN` | 一次性注册令牌 |
| `--core` | 首次必填 | `KUNXTIER_CORE` | 核心地址 host:port |
| `--fingerprint` | 首次必填 | `KUNXTIER_FINGERPRINT` | 核心证书指纹 |

要点:

- **令牌决定网络**: 节点加入哪个网络组
  由令牌签发时的 `--network` 决定, up 无网络参数
- **令牌一次性**: 注册成功后节点获得长期凭证
  (存 state 目录), 令牌即失效
- **已注册节点重连**: 令牌 / 核心地址 / 指纹均可省略,
  daemon 以上次身份自动重连
- 虚拟网卡 MTU 1440, 虚拟 IP 由核心从网络组网段内分配

成功输出示例:

```text
已上线: 节点 laptop 网络 default IP 100.64.0.1
```

## 查看状态与断开

```sh
KUNXTIER_STATE_DIR=./kunxtier-node-state kunxtier status
KUNXTIER_STATE_DIR=./kunxtier-node-state kunxtier down
```

`status` 输出 (存在时):

- 节点名 / 网络 / 在线状态 / 离线原因
- 本机虚拟 IP 与路径策略
- 对端表: 名称 / IP / 路径 (direct 或 relay) / 端点地址
- `--verbose` 追加最近关键事件
  (打洞 / 路径切换 / 信令 / 注册, 按时间旧到新)

`down` 断开会话但 daemon 留守, 注册身份保留。
再次 `up` (免令牌) 即可重连。

## 连接行为

两个节点之间的流量路径按以下策略自动协商:

- **默认 (auto)**: STUN 探测公共地址后并发打洞,
  成功即直连 (direct);
  超时未成功回落中继 (relay),
  之后打洞成功会自动切回直连
- **强制中继 (force_relay)**: 跳过打洞全程走中继,
  由管理端按节点设置:

  ```sh
  kunxtier admin ... node policy \
    --network <网络> --name <节点> force_relay
  ```
- **保活**: 对端 ping 间隔 15 秒,
  60 秒无响应判定失联
- **断线重连**: daemon 与核心断连后自动重连,
  退避从 1 秒指数增长至 30 秒

## 开机自启 (install / uninstall)

```sh
sudo kunxtier install
sudo kunxtier uninstall
```

`install` 行为:

- 需 root; 生成或校验全局配置
  `/etc/kunxtier/config.toml` (不接受 env 覆盖)
- 创建 state 与日志目录
- 注册系统服务并立即启动,
  输出各路径与后续 `kunxtier up` 指引
- 服务定义: macOS
  `/Library/LaunchDaemons/com.kunxtier.daemon.plist`,
  Linux `/etc/systemd/system/kunxtier.service`;
  日志 `log_dir/daemon.log`

`uninstall` 停止并移除服务注册,
state 目录与全局配置保留 (节点身份不丢)。
Windows 平台执行 install 直接报错,
见 [capabilities.md](capabilities.md)。

## 节点日志

- 位置缺省 `/var/log/kunxtier`,
  经 config.toml `log_dir` / `log_level` 调整
- daemon 亦支持 `--log-level` / `--log-file` 覆盖,
  级别链见 [config.md](config.md)
