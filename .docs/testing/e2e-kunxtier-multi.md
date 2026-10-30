# kunxtier 多机验证指导

## 定位

局域网 (或本机 + 虚拟机) 内两台及以上真实机器完成的组网验证: 双节点互通与路径策略、受限网络回落中继、保活与 MTU、跨用户与网络隔离、卫星中继协议生命周期、服务化重启自愈。在单机验证 (`e2e-kunxtier-single.md`) 全部通过后执行。

覆盖 `e2e-kunxtier-network.md` 场景矩阵的以下行:

| 矩阵场景 | 本文档章节 |
| ---- | ---- |
| 双节点直连 (同网段, 非真实 NAT) | M2 |
| 打洞失败回落中继 (客户端 C 受限) | M3 |
| 强制中继策略 | M4 |
| 空闲保活 / MTU 大包 | M5 |
| 跨用户互通 / 属主标注与过滤 | M6 |
| 网络组隔离 (真实双机) | M6 |
| 节点下线 / 核心中继重启 | M2 |
| 卫星中继注册 / 卫星下线摘除 | M7 |
| daemon 服务化 (重启机器自愈) | M8 |
| Windows 节点互通 (可选, 局域网) | M9 |

边界: 同网段双机的直连不经公网 NAT, 不构成真实打洞验证 (E1 / E2 归公网文档); relay 平台级托管 (systemd) 归公网文档, 本文档 relay 可跑在任一 Linux 机器或虚拟机。

结论记录: 矩阵行记入 FEAT-kunxtier-core-runtime T10 推进记录; FEAT-kunxtier-manual-verification T03 (后半) 记入该 issue。缺陷开独立 bug issue。

## 环境准备

| 项目 | 值 |
| ---- | --- |
| 核心中继 R | Linux 实机或虚拟机, `kunxtier relay --listen 0.0.0.0:9443 --data-dir ./kunxtier-state`, 机内地址如 192.168.1.10:9443 |
| 客户端 A | macOS (本机), daemon + CLI |
| 客户端 B | Linux (实机或虚拟机), daemon + CLI |
| 客户端 C (可选) | Linux, iptables / nftables 限制仅允许与 R 通信 |
| 卫星 (可选) | 独立机器一个 `relay --satellite`; 无独立机器时用 R 同机第二进程 (仅验证协议, 不验证隔离) |
| 产物 | 各平台按仓库构建流程产出对应二进制; A / B / C 均需 sudo |

各客户端首次注册统一形态:

```sh
sudo kunxtier up --token <tok> --name <a|b|c> --core 192.168.1.10:9443 --fingerprint <fp> --state-dir ./kunxtier-node-state
```

## M1 前置链路确认

- R 侧按单机文档 S1-S2 完成 init 与管理链路; A / B 各自 `up` 成功, `status` 互见
- 预期: A `status` 对端表含 b, B 对端表含 a

## M2 双节点互通与状态一致性

1. A: `ping <B 虚拟 IP>` 持续 60 秒, 期间反复 `sudo kunxtier status --state-dir ...`
2. B: `kunxtier down` 后观察 A

预期:

- 互通稳定; 两端 status 显示路径一致 (同网段预期 direct)
- B 下线后 A 的节点表秒级移除 B, ping 失败; B `up` (免令牌) 恢复

## M3 打洞失败回落中继 (关键用例)

1. C 侧防火墙仅放行与 R 的通信:

```sh
# iptables 示例 (按实际网卡名调整)
sudo iptables -P INPUT DROP
sudo iptables -A INPUT -s 192.168.1.10 -j ACCEPT
sudo iptables -A OUTPUT -d 192.168.1.10 -j ACCEPT
sudo iptables -P OUTPUT DROP
```

2. A 与 C 各自 `up`, A `ping <C 虚拟 IP>` 持续 60 秒, 观察 `status` 与控制台总览

预期:

- 打洞超时窗口内完成向中继回落, ping 恢复连通
- status 与控制台中 C 的路径均显示 relay
- 全程无人工干预, 无 daemon 崩溃或重启
- 解除 C 防火墙限制后 (可选), 路径恢复 direct

## M4 强制中继策略生效与恢复 (关键用例)

1. A 与 B 直连, 业务流量正常
2. 管理侧: `kunxtier admin --server https://<R>:9443 --fingerprint <fp> node policy --network default --name a force_relay` (或控制台开关)
3. A 持续 ping B, 观察 status; 关闭策略 (auto) 后继续观察

预期:

- 生效后 A 流量全部经中继, 路径显示 relay, 秒级切换, 业务不中断
- 关闭后 A 恢复直连 (重新打洞), 路径回到 direct

## M5 链路维持

| 场景 | 操作 | 预期 |
| ---- | ---- | ---- |
| 空闲保活 | A、B 直连后保持 30 分钟无流量再 ping | 首包即通, 路径仍 direct, 无重新打洞痕迹 |
| MTU 大包 | A、B 分别在 direct 与 force_relay 两种路径下 `ping -s 1400 <对端>` 并发起大文件 TCP (如 scp / curl) | 两种路径均无异常丢包或挂死 |

## M6 多用户与网络隔离

1. 跨用户互通: `admin user create --network default --name u2`, A 属 u1 (令牌 `--user u1` 签发), C 属 u2, A ping C
2. 属主标注: 控制台按属主过滤节点列表
3. 网络组隔离: B 以 net2 令牌注册 (新 state-dir), 默认网令牌尝试注册 net2; A ping net2 内节点

预期:

- A ping C 互通 (全互通模型); 属主过滤结果正确, 属主仅为展示标注, 无属主登录入口
- 令牌跨网注册被拒; 跨网 ping 不通; 控制台内两网数据互不可见

## M7 卫星中继协议生命周期

```sh
# 卫星 (独立机器或 R 同机第二进程, 后者仅验证协议)
KUNXTIER_RELAY_TOKEN=<relay-token> kunxtier relay --satellite --core 192.168.1.10:9443 \
  --fingerprint <fp> --advertise <卫星对外地址>:<端口> --listen 0.0.0.0:<端口>
```

1. 卫星注册后, 观察 A 的 `status --verbose` (中继列表分发)
2. 有节点经卫星回落时 (可将 A 置 force_relay), 杀掉卫星进程

预期:

- 注册后中继列表自动更新并分发, 节点无需重启即可使用卫星
- 卫星下线后列表及时摘除, 回落流量切换到下一中继或核心内置转发, 业务不长期中断

## M8 daemon 服务化重启自愈

1. B: `sudo kunxtier install` 后确认服务形态在线
2. 重启 B 机器, 不登录不人工干预

预期: daemon 自动恢复上线 (A 的节点表与控制台可见), 虚拟 IP 不变; 之后 `sudo kunxtier uninstall` 清理 (state 与全局配置保留)

## M9 Windows 节点互通 (可选)

- 前置: 局域网 Windows 机器, wintun 产物 (T13 构建产物)
- 操作: D `up` 后与 A、B 互 ping
- 预期: 互通成功, 路径与 status 显示一致
- 注: wintun 接入验证与三平台构建产物验收以公网文档 (T13) 为准, 此处为提前冒烟

## 通过标准

- M1-M8 全部通过 (M9 可选; 卫星无独立机器时 M7 需标注 "仅同机协议验证" 并在公网阶段补独立机器用例)
- 矩阵行结论记入 issue 推进记录; 缺陷均有 bug issue 且修复关闭
