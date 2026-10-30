# kunxtier 公网验证指导

## 定位

公网环境完成的最终验收: 真实 NAT 打洞直连、公网核心中继生产部署 (systemd 托管 / 备份跨机恢复 / schema 升级)、卫星中继真机、Windows 平台与三平台发布产物。在单机 (`e2e-kunxtier-single.md`) 与多机 (`e2e-kunxtier-multi.md`) 验证通过后执行, 是 FEAT-kunxtier-core-runtime T10 / T13 与 FEAT-kunxtier-manual-verification T01 / T02 的收官环境。

覆盖 `e2e-kunxtier-network.md` 场景矩阵的以下行:

| 矩阵场景 | 本文档章节 |
| ---- | ---- |
| 双节点直连 (真实 NAT, E1 / E2) | P2 |
| 核心中继初始化 (公网 443) | P1 |
| 核心中继重启 (生产形态) | P3 |
| 备份与恢复 (跨机) / 旧版本升级 | P4 |
| 卫星中继注册 / 卫星下线摘除 (独立机器) | P5 |
| Windows 节点互通 | P6 |
| daemon 服务化 (生产形态) | P3 |

对应 issue 任务:

- FEAT-kunxtier-manual-verification T01: relay 真机 init / run / backup 与 systemd 托管 (P1, P3, P4)
- FEAT-kunxtier-manual-verification T02: 卫星真机注册分发 / 断连摘除 / 指纹校验 (P5)
- FEAT-kunxtier-core-runtime T13: Windows wintun 接入验证与三平台统一构建产物 (P6)
- FEAT-kunxtier-core-runtime T10: 场景矩阵剩余行收口 (全部完成后汇总)

## 环境准备

| 项目 | 值 |
| ---- | --- |
| 核心中继 R | 公网 Linux VPS, 开放 443/tcp (管理 + 信令 + 转发 + 控制台), 建议独立域名 (可选 ACME 时) |
| 卫星中继 S | 独立公网机器一台 (与 R 不同机, 验证隔离与摘除) |
| 打洞组 E1 / E2 | 两台分别位于不同 NAT 后的节点 (不同家庭网络实机, 或两套 network namespace + NAT 规则仿真) |
| 客户端 A | macOS (本机或另一 NAT 后实机) |
| 客户端 D | Windows 实机 (wintun 产物) |
| 产物 | 三平台发布构建产物 (linux x86_64 / aarch64, macOS, windows x86_64); 各节点 sudo / 管理员权限 |
| 安全 | admin 凭证仅走 env 与 state 目录, 不入仓库不入聊天记录; R 的 state 目录 (TLS 私钥 + SQLite) 权限 0600, 不外传 |

公网核心初始化:

```sh
kunxtier relay --init --listen 0.0.0.0:443 --data-dir /var/lib/kunxtier
# 记录指纹与 admin 凭证后启动 (生产以 systemd 托管, 见 P3)
```

## P1 核心中继公网部署 (manual-verification T01)

1. `relay --init` 校验 state 权限 0600 与默认网络组
2. 以 443 启动, 从外部机器确认管理 API / 信令 / 控制台页面可达
3. 校验: `kunxtier admin --server https://<R>:443 ...` 自外网可用; 浏览器打开 `https://<R>` 可登录

预期: 初始化产物完整; 公网 443 全链路可达; 恶意指纹 / 错误凭证仍被拒 (负向复测一次)。

## P2 真实 NAT 打洞直连 (E1 / E2, 关键用例)

1. E1、E2 分别在不同 NAT 后 `up` 上线
2. 任一侧: `sudo kunxtier status --verbose` 观察打洞过程 (STUN 探测 / 候选交换记录)
3. E1 持续 ping E2 虚拟 IP, 并发起 TCP 业务 (scp / curl 大文件)

预期:

- status 路径显示 direct, 延迟为直连量级
- verbose 日志含 STUN 探测与候选交换留痕
- 空闲 30 分钟后业务不回退 (与 M5 呼应)
- 若两端 NAT 均受限导致打洞失败, 自动回落中继且业务可用 (记录实际 NAT 类型与结论)

## P3 生产形态托管与自愈 (manual-verification T01)

1. R 配置 systemd 单元托管 relay (或经 `install` 类机制), 开机自启
2. `sudo systemctl restart <relay>` 模拟核心重启, 观察 E1 / E2 / 卫星
3. 选一台节点 (如 E1) 同样服务化, 重启机器验证自愈

预期:

- relay 重启后节点与卫星在重试窗口内自动重连信令; 既有直连会话在重启期间不中断
- 恢复后节点表、中继列表与虚拟 IP 不变
- 节点机器重启后 daemon 自动恢复上线, 无需人工登录

## P4 备份跨机恢复与 schema 升级 (manual-verification T01)

1. R: `kunxtier relay --data-dir /var/lib/kunxtier backup ./backup.sqlite` (或既有备份机制导出)
2. 将备份恢复到新 VPS (同版本二进制) 并启动, 原节点免令牌重连
3. (具备旧版本产物时) 用旧 schema 数据库起新版二进制

预期:

- 网络组 / 用户 / 节点 / 令牌状态完整还原, 节点重连即恢复, 虚拟 IP 不变
- 旧 schema 自动前向迁移, 数据完整, 全网恢复

## P5 卫星中继真机 (manual-verification T02)

```sh
# S (独立公网机器)
KUNXTIER_RELAY_TOKEN=<relay-token> kunxtier relay --satellite --core <R>:443 \
  --fingerprint <fp> --advertise <S 对外地址>:443
```

1. 指纹校验: 用错误指纹起卫星
2. 注册分发: 正确指纹注册, 观察经中继回落的节点 (可置 force_relay) 是否切到卫星
3. 断连摘除: 杀掉 S 进程, 观察中继列表与回落流量

预期:

- 错误指纹被拒, 提示明确
- 注册后列表分发及时, 节点无需重启即可使用卫星
- 断连后列表及时摘除, 流量切换到核心内置转发, 业务不长期中断

## P6 Windows 平台与三平台产物 (FEAT-kunxtier-core-runtime T13)

1. 构建: 三平台统一构建产物 (含 windows wintun 接入)
2. D (Windows) 以管理员权限 `up` 注册, 与 A / E1 互 ping, 观察 `status`
3. 服务化: D `install` 后重启机器

预期:

- wintun 接入正常, 互通成功, 路径与 status 显示一致
- 重启后 daemon 自愈
- 三平台产物形态一致, 版本与校验和可追溯

## 通过标准

- P1-P5 全部通过; P6 为 T13 收口条件
- E2E 场景矩阵 31 行全部有结论 (通过 / 缺陷指针 / 环境受限说明), 汇总记入 FEAT-kunxtier-core-runtime T10 推进记录
- FEAT-kunxtier-manual-verification 四项任务全部完成并记录结论; 缺陷均有 bug issue 且修复关闭
- 验证完成后释放公网验证资源, 备份产物按需留存 (注意不含私钥外传)
