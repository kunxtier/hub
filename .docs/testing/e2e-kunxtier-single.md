# kunxtier 单机验证指导

## 定位

单台开发机 (macOS / Linux) 完成的功能链路验证: relay 初始化与管理链路、daemon 特权冒烟、注册闭环、控制面负向场景、控制台双入口、单机双 daemon 组网与恢复类场景。零外部环境依赖, 是多机 (`e2e-kunxtier-multi.md`) 与公网 (`e2e-kunxtier-public.md`) 验证的前置。

覆盖 `e2e-kunxtier-network.md` 场景矩阵的以下行 (其余行归属多机 / 公网文档):

| 矩阵场景 | 本文档章节 |
| ---- | ---- |
| 核心中继初始化 | S1 |
| 令牌注册设备 / 同名注册被拒 / 令牌撤销 / 恶意指纹 | S5, S6 |
| 节点移除后旧凭证失效 / 禁用用户 | S6 |
| 令牌暴力限速 | S6 |
| daemon 未运行提示 | S5 |
| 控制台 (核心托管 / console 子命令 / 会话过期 / admin API 鉴权边界) | S7 |
| 网络组管理 / 网络组隔离 | S3, S8 |
| 节点下线 / 核心中继重启 / 备份与恢复 | S8 |
| 本地网段冲突 | S9 (可选) |
| daemon 服务化 (不重启机器部分) | S10 (可选) |

边界: 单机 relay 仅用于链路验证, relay 的平台级验收 (systemd 托管 / 重启自愈) 见公网文档; 单机直连路径不含真实 NAT 打洞 (见公网文档 E1 / E2)。

结论记录: 场景矩阵行记入 FEAT-kunxtier-core-runtime T10 推进记录; FEAT-kunxtier-manual-verification T03 (前半) / T04 记入该 issue。发现的缺陷开独立 bug issue, 不在验证文档堆叠。

## 环境准备

| 项目 | 值 |
| ---- | --- |
| 机器 | 单台开发机 (macOS 或 Linux) |
| 构建 | `cd rust/kunxtier && cargo make build` (一键: 编译前先构建前端并嵌入), 二进制 `target/debug/kunxtier` (下文以 `kunxtier` 指代) |
| 工作目录 | 独立目录 (如 `~/kunxtier-verify`), 严禁在仓库内落 state |
| 端口 | relay 127.0.0.1:9443 (高位端口避免 443 特权); console 本地入口缺省 127.0.0.1:7600 |
| 特权 | daemon / up / install 需要 sudo (TUN: macOS utun 需 root, Linux 需 CAP_NET_ADMIN) |
| 凭证 | 指纹与 admin 凭证来自 `relay --init` 输出; admin 凭证亦落 `state/admin-credential` |

约定记号: `<fp>` 为核心证书指纹 (init 输出, 或由 `kunxtier-state/cert.pem` 重算), 一次性令牌 / relay 令牌按签发输出记录; admin 凭证经 `KUNXTIER_ADMIN` 环境变量注入, 取值 `kunxtier-state/admin-credential`。

## S1 relay 初始化与启动

```sh
kunxtier relay --init --listen 127.0.0.1:9443 --data-dir ./kunxtier-state
```

- 预期: 生成 state 目录, 自签证书与 admin 凭证明文仅展示一次; 默认网络组已创建; 私钥与 SQLite 权限 0600
- 记录 `<fp>` 与 `$ADMIN`
- 启动: `kunxtier relay --listen 127.0.0.1:9443 --data-dir ./kunxtier-state` 前台常驻 (验证期间保持运行)

## S2 管理链路走查 (manual-verification T04 主体)

```sh
export KUNXTIER_ADMIN=$(cat ./kunxtier-state/admin-credential)   # 错过 init 输出时从 state 目录取
kunxtier admin --server https://127.0.0.1:9443 --fingerprint <fp> network list          # 默认网络
kunxtier admin --server https://127.0.0.1:9443 --fingerprint <fp> network create net2 --cidr 100.67.0.0/16
kunxtier admin --server https://127.0.0.1:9443 --fingerprint <fp> user create --network default --name u1
kunxtier admin --server https://127.0.0.1:9443 --fingerprint <fp> token issue --network default                 # 一次性节点令牌
kunxtier admin --server https://127.0.0.1:9443 --fingerprint <fp> token issue --network default --type relay    # 卫星令牌 (留多机文档用)
kunxtier admin --server https://127.0.0.1:9443 --fingerprint <fp> node list
```

- 预期: 各命令 JSON 输出一致自洽; 令牌明文仅展示一次; net2 创建后无需重启核心即可被后续命令引用
- 未设 `KUNXTIER_ADMIN` 时执行 admin 应得到明确使用错误, 而非晦涩 io 错误

## S3 daemon TUN 特权冒烟 (manual-verification T03 前半)

```sh
cd rust/kunxtier && sudo cargo test -p kunxtier-node -- --ignored
```

- 预期: `tun` 模块真实 TUN 设备用例通过 (macOS utun / Linux TUN 创建、读写、销毁); `service` 模块 install 路径用例通过
- 记录用例清单与结果; 失败即中止后续节点侧验证并开 bug issue

## S4 daemon 启动

```sh
sudo kunxtier daemon --state-dir ./kunxtier-node-state
```

- 预期: 前台启动, 日志装配正常; 不带 sudo 运行应得到 EACCES 映射的提权指引 (sudo / --state-dir / install), 而非裸 io 错误

## S5 注册闭环 (up / status / down / 重连)

另开终端:

```sh
sudo kunxtier up --token <一次性令牌> --name solo --core 127.0.0.1:9443 --fingerprint <fp> --state-dir ./kunxtier-node-state
sudo kunxtier status --state-dir ./kunxtier-node-state
sudo kunxtier status --verbose --state-dir ./kunxtier-node-state   # 排障视图: 注册 / 信令 / 路径事件留痕
sudo kunxtier down --state-dir ./kunxtier-node-state
sudo kunxtier up --state-dir ./kunxtier-node-state                 # 已注册节点免令牌以上次身份重连
```

- 预期: 注册成功并分配 /32 虚拟 IP; status 显示本机与会话状态; verbose 含关键事件留痕; down 后 daemon 留守; 免令牌 up 以原身份恢复
- daemon 未运行时执行 `kunxtier status` 应提示 daemon 未运行与启动方式, 不报晦涩错误

## S6 负向与治理场景

| 场景 | 操作 | 预期 |
| ---- | ---- | ---- |
| 同令牌二次注册 | S5 令牌成功后再用同令牌 `up` | 被拒, 错误明确 |
| 同名注册被拒 | 另一 state-dir 用 `--name solo` 注册 | 被拒, 提示名字冲突 |
| 令牌撤销 | `admin token revoke <id>` 后 `up` | 被拒, 错误信息明确 |
| 恶意指纹 | `--fingerprint` 改一位 hex 后 `up` | 连接被拒, 提示指纹不匹配 |
| 节点移除后凭证失效 | `admin node remove --network default --name solo` 后观察 daemon | 信令被拒 (凭证已吊销), 网内流量停止 |
| 禁用用户 | `admin user disable --network default --name u1` | 其名下设备凭证吊销、重连被拒, 令牌全部失效 |
| 令牌暴力限速 | 脚本连续提交错误令牌注册 (如 shell 循环 20 次) | 达阈值后被限速 / 退避并返回明确错误; 正常客户端不受影响 |
| admin API 鉴权边界 | 用节点凭证 (state 内) 调 admin API | 被拒, 凭证类型不互通 |

## S7 控制台双入口冒烟

1. 核心托管: 浏览器打开 `https://127.0.0.1:9443` (自签证书需手动接受), 以 `$ADMIN` 登录
2. console 子命令: `kunxtier console --relay 127.0.0.1:9443 --fingerprint <fp>`, 浏览器打开 `http://127.0.0.1:7600`
3. 校验点:
   - 总览状态与 `admin node list` / 节点 `status` 一致
   - 令牌签发与撤销可用
   - 网络切换器出现 default / net2, 切到 net2 可立即签发令牌
   - console 子命令入口与核心托管行为等价, 本地无管理数据落盘 (仅缓存)
   - 会话过期后操作要求重新登录, 无越权残留

## S8 单机双 daemon 组网 (可选进阶)

同机两个 daemon 实例 (各自 state-dir, 各自 utun / TUN 设备), 直连路径为回环量级, 验证控制面与转发面行为, 不代表真实 NAT 打洞:

```sh
sudo kunxtier daemon --state-dir ./node-a &
sudo kunxtier daemon --state-dir ./node-b &
sudo kunxtier up --token <tok-a> --name a --core 127.0.0.1:9443 --fingerprint <fp> --state-dir ./node-a
sudo kunxtier up --token <tok-b> --name b --core 127.0.0.1:9443 --fingerprint <fp> --state-dir ./node-b
sudo ping <b 虚拟 IP>        # 从 a 侧
sudo kunxtier status --state-dir ./node-a   # 对端 b 路径
```

| 场景 | 操作 | 预期 |
| ---- | ---- | ---- |
| 双节点互通 | a ping b | 通; status 路径显示一致 |
| 节点下线秒级移除 | b `down` | a 的节点表秒级移除 b, ping 失败 |
| 网络组隔离 | b 改用 net2 令牌重注册 (node-b 换新 state-dir) | 默认网令牌跨网注册被拒; a ping 不通 b; 控制台两网数据互不可见 |
| 强制中继策略 | `admin node policy --network default --name a force_relay` | a 流量走中继, status 显示 relay; 改回 auto 恢复 |
| 核心中继重启 | 重启 relay 进程 | 双节点自动重连, IP 不变, 互通恢复 |
| 备份与恢复 | `kunxtier relay --data-dir ./kunxtier-state backup ./backup.sqlite` 后以新 data-dir 起第二个核心并用其注册新节点 | 网络组 / 用户 / 节点 / 令牌状态完整还原 |

## S9 本地网段冲突 (可选)

- 前置: 本机路由含与目标网络 /16 重叠的地址 (如手动 `sudo ifconfig alias` 构造)
- 操作: 启动 daemon
- 预期: 拒绝启动, 输出诊断与处理建议
- 构造成本高时标注 "环境不具备, 转多机 / 公网补测"

## S10 daemon 服务化 (可选, 不含重启机器)

```sh
sudo kunxtier install      # 注册系统服务与开机自启, 首次生成全局配置
# 观察服务状态与 daemon 上线
sudo kunxtier uninstall    # 停服并移除注册, state 与全局配置保留
```

- 预期: install 后 daemon 以服务形态拉起, 节点重新上线; uninstall 后服务移除, state 目录与全局配置保留; 重复 install 幂等
- 重启机器自动恢复场景归多机文档 (B 机) 执行

## 通过标准

- S1-S7 全部通过 (S8-S10 可选或在环境就绪时补测)
- 每个矩阵行有明确结论 (通过 / 缺陷指针), 记录到对应 issue 推进记录
- 发现的缺陷均有 bug issue 且修复关闭
