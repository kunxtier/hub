# 快速上手

目标: 两台机器组成一个虚拟网, 三步完成。
核心中继一台 (机器 A), 节点至少一台 (机器 B)。
本地没有第二台机器时,
可在同一台机器用两个不同 state 目录跑双 daemon 验证。

## 前置条件

- **系统**: macOS 或 Linux (Windows 未支持)
- **权限**: 节点侧 daemon 需要 root (创建 TUN 网卡)
- **网络**: 节点能访问核心的监听端口 (下例为 9443)
- **二进制**: 已构建 `kunxtier` (见 [README](../README.md))

## 第一步: 启动核心中继 (机器 A)

```sh
KUNXTIER_LISTEN=0.0.0.0:9443 \
KUNXTIER_DATA_DIR=./kunxtier-state kunxtier relay
```

零配置首启自动初始化, 终端会输出并需要记录两项:

- **admin 凭证**: 形如 `kxt_admin_xxx`, 管理操作凭它
- **证书指纹**: 一段 hex, 所有客户端连接核心固定用它

默认创建网络组 `default` (网段 100.64.0.0/16), 直接用它即可。
生产部署见 [relay.md](relay.md)。

## 第二步: 签发节点令牌 (机器 A, 另开终端)

```sh
export KUNXTIER_ADMIN=<admin 凭证>
kunxtier admin --server <核心地址>:9443 --fingerprint <指纹> \
  token issue --network default
```

输出形如 `kxt_node_xxx` 的一次性令牌。
明文仅此一次展示, 立即传给机器 B。

## 第三步: 节点接入 (机器 B)

```sh
sudo KUNXTIER_STATE_DIR=./kunxtier-node-state kunxtier daemon &
KUNXTIER_STATE_DIR=./kunxtier-node-state kunxtier up \
  --token <一次性令牌> --name peer-b \
  --core <核心地址>:9443 --fingerprint <指纹>
```

成功输出: `已上线: 节点 peer-b 网络 default IP 100.64.0.x`。
daemon 需保持运行; up / status / down 免 sudo (走本地 IPC)。

机器 A 自身入网时, 用另一个 state 目录
再跑一组 daemon + up (节点名如 `peer-a`)。

## 验证连通

```sh
KUNXTIER_STATE_DIR=./kunxtier-node-state kunxtier status
```

对端表出现另一节点即组网成功:

```text
对端:
名称      IP          路径     端点
peer-a    100.64.0.1  direct   1.2.3.4:51820
```

路径 `direct` 为直连, `relay` 为中继回落。
再直接 ping 对端虚拟 IP 验证数据面。
路径与打洞行为说明见 [node.md](node.md)。

## 可选: Web 控制台

```sh
kunxtier console --relay <核心地址>:9443 --fingerprint <指纹>
```

浏览器打开 `http://127.0.0.1:7600`, 以 admin 凭证登录,
可视化管理网络组 / 节点 / 令牌。
也可直接浏览器访问核心地址 (核心同样托管该页面)。

## 下一步

- **生产部署中继**: [relay.md](relay.md)
  (卫星扩展 / 备份 / 日志)
- **节点服务化与连接行为**: [node.md](node.md)
  (install 开机自启 / 打洞回落)
- **管理操作全景**: [admin.md](admin.md)
  (网络组 / 令牌 / 节点策略)
- **遇到问题**: [troubleshooting.md](troubleshooting.md)
