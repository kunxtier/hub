# kunxtier 总体架构

## 背景与目标

kunxtier 是自研轻量 P2P 虚拟组网工具, 定位 ZeroTier / EasyTier 平替: 一条命令起中继, 一条命令加入网络 (移动端时代再扩展扫码)。核心中继 (`kunxtier relay`) 是网络中枢与唯一权威: 一个核心中继承载一张或多张互相隔离的网络组, 每张网络有独立的成员、地址段与策略; 核心中继同时提供信令与内置密文转发。卫星中继是可插拔的纯转发扩展 (不感知网络); 控制台是无状态 UI 接入层, 随处可起。relay + daemon 即闭环, 网络组、控制台与卫星都只是增强。数据面走 WireGuard 协议, P2P 直连优先、自动或按策略走中继。Rust 侧能力聚合进单一二进制 `kunxtier`, 前端源码在 `web/kunxtier-console/`。

## 方案概览

```text
            核心中继  kunxtier relay
            (承载多张隔离网络组; 部署位置不限, 节点可达即可)
┌─────────────────────────────────────────────┐
│ 管理: 多网络组×(用户/令牌/节点/IP/策略) SQLite │
│ 信令: REST + WS                             │
│ 转发: TLS + WS 密文帧 (内置)                 │
│ 控制台: 嵌入静态资源托管                      │
└──────┬──────────────────────────┬───────────┘
       │ 注册 / 中继列表分发       │ TLS (自签+指纹 / ACME)
┌──────┴──────────┐      ┌────────┴─────────────────────┐
│ 卫星中继 (可选)   │      │ 节点 daemon (macOS/Linux/Win) │
│ relay            │      │ 相互 WireGuard UDP 直连       │
│  --satellite     │      │ (默认自动策略)                │
│ 纯转发/零知识/    │      └──────────────────────────────┘
│ 无管理权/无存储   │
└──────┬──────────┘
       └─ 中继路径: WG 密文帧经卫星或核心内置转发 → 节点
          (自动回落与强制策略共用此路径)

控制台 (可选, 无状态): kunxtier console --relay <addr>
能力全部来自核心中继 admin API; 核心中继亦直接托管同一套页面
```

控制面与数据面分离: 管理与信令只在注册、状态变化、打洞协调与策略下发时参与; 已建立的 WireGuard 会话不依赖核心中继存活。

## 模块职责

| 模块 (crate / 目录) | 职责 | 不负责 |
| ---- | ---- | ------ |
| kunxtier-cli | 单一入口二进制: 子命令解析, 装配 relay 核心模式 / relay 卫星模式 / daemon / console 四种进程角色, 本地 IPC 客户端 | 业务逻辑 |
| kunxtier-relay | 网络核心: 多网络组管理 (每网独立的用户/令牌/节点/密钥/IP 段/策略) + 信令 (REST + WS) + 密文转发 + admin API + 控制台托管 + SQLite; 卫星模式仅装配转发与注册部分 | 数据面加解密、节点侧 TUN |
| kunxtier-node | 节点 daemon: TUN 设备、WireGuard 用户态引擎、打洞、路径策略执行、本地 IPC 服务 | 用户与令牌管理 |
| kunxtier-core | 协议消息定义、密钥与证书工具、地址分配规则、路径策略类型、共享配置与错误类型 | 任何一侧的私有业务状态 |
| web/kunxtier-console | 控制台前端单页应用源码, 构建产物嵌入二进制 (relay 托管与 console 子命令共用) | 后端逻辑 |

## 关键结构

```text
rust/kunxtier/
├── crates/
│   ├── kunxtier-cli/        # 入口: 子命令解析 → 装配 relay / console / node
│   ├── kunxtier-relay/      # 核心: 管理 + 信令 + 转发 + 控制台托管 + store; 卫星模式子集
│   ├── kunxtier-node/       # daemon: tun-rs + WG 用户态引擎 + 打洞 + IPC
│   └── kunxtier-core/       # 协议 / 密钥 / 地址 / 策略 / 配置类型
├── Cargo.toml               # workspace 根, 独立 Cargo.lock
└── README.md

web/kunxtier-console/        # 前端源码, 构建产物交付嵌入
```

依赖方向: cli → relay / node → core; relay 与 node 互不依赖。

## 关键流程

1. 开网: `relay --init` 创建默认网络组, `admin network create` 按需另建; 每网独立成员表与 /16 地址段
2. 注册: `up --token` → 节点生成 WG 密钥对 → REST 注册公钥 → 核心中继校验令牌 (含网络绑定)、记录属主、在该网络段内分配 /32、签发节点凭证 (仅存哈希) → 节点写入 state, 令牌失效
3. 上线: daemon 以节点凭证完成挑战应答, 建立信令 WS → 上报候选地址 (STUN 探测结果与本机地址) → 核心中继推送本网络节点表 (含对端公钥)、中继列表与路径策略
4. 连接: 自动策略下按节点表配置 WG peer → 信令互换候选地址 → 并发 UDP 打洞 → 成功切直连; 超时回落中继 (核心内置或卫星)。强制中继策略下跳过打洞, 直接经中继交换 WG 密文帧
5. 卫星注册: `kunxtier relay --satellite --core <addr>` 以 relay 令牌注册 (卫星不感知网络概念) → 核心更新中继列表并分发给全部网络 → 节点按列表连接卫星
6. 控制台接入: 浏览器直开核心中继托管的页面, 或在任意机器 `kunxtier console --relay <addr>` 起本地控制台; 两者同一套静态资源与 admin API, 行为等价, 均含网络组切换
7. 恢复与维持: 直连与中继链路按保活策略维持 (persistent keepalive 与 WS 心跳); 周期性重试直连; 信令断线自动重连; 核心中继重启不要求节点与卫星重启; 卫星断连即从列表摘除

## 可观测性

可观测性基线 (需求见 nfr-kunxtier-observability): 分级结构化日志 + status 排障视图, 不引入指标与追踪体系。

- 日志基建: tracing 统一埋点, subscriber 只在 kunxtier-cli main 初始化一次 (四种进程角色共用); 级别优先级遵循全局分层 CLI flag > config (relay 配置文件) > ENV (RUST_LOG) > 默认 info。daemon 默认输出 stderr (服务化后由 systemd / launchd journal 接管), `--log-file` 显式启用文件输出, 启用时 stderr / 文件双写, 文件按大小轮转保留一份历史
- 节点事件环: kunxtier-node 内维护有界 NodeEvent 环形缓冲 (时间戳 / 类别 / peer / detail), 只记打洞、路径切换、信令、注册四类关键事件; 事件产生点同源双写 tracing 与环; 对端最近失败原因与中继选择过程从环推导, 不另存第二份状态源; `status --verbose` 经本地 IPC 拉取事件环与当前路径状态
- relay 侧: 只补齐事件日志 (节点注册 / 凭证吊销 / 令牌校验失败 / 卫星注册与摘除 / admin 登录失败), 不建事件环
- 脱敏: 日志字段按 nfr-kunxtier-security NFR-4 审计, 令牌 / 私钥 / 凭证一律脱敏形式

## 设计原则与约束

- relay 即中枢: 核心中继是唯一管理权威, relay + daemon 即闭环; network 是中继内的一等隔离域, 网络组间不互通不可见; 控制台与卫星中继都是可插拔扩展, 不改变闭环
- 单权威: 每张网络恰好归属一个核心中继, 一个核心中继可承载多张网络组; 卫星永不持有管理数据且不感知网络。多核心 / HA 属后续演进 (EasyTier 式状态同步), MVP 不做
- 卫星零知识: 卫星只转发 WG 密文帧, 无管理权限、无存储; 核心中继持管理面, 但同样不持有 WG 会话密钥, 数据面端到端加密不受部署形态影响
- 部署自由: 角色不绑定部署位置; 核心中继的硬要求只有 "各节点可达", 公网地址是部署层面的常见选择而非架构约束
- 控制台无状态: 能力完全来自核心中继 admin API, 任意 console 实例等价, 无本地存储
- 用户态数据面统一: 全平台内嵌 WireGuard 用户态引擎 (gotatun / boringtun 系) + tun-rs, 不依赖系统 WireGuard, 各平台行为同源
- 传输层机制显式定义: TUN MTU 按最坏路径 (中继上 WG + TLS + TCP 叠加封装) 推导; 直连 keepalive 与 WS 心跳默认开启; 节点启动即检测本地网段与网络 /16 (CGNAT) 冲突
- 节点身份双凭证: WireGuard 私钥 (数据面, 不离机) 与节点凭证 (信令面, 注册时签发、仅存哈希、挑战应答校验) 分离, 吊销即断
- 控制面极简: 单核心中继 + SQLite, 不引入外部数据库与消息队列
- monorepo 自包含: 本项目横跨 rust/kunxtier 与 web/kunxtier-console 两个语言目录, 拆仓时一起迁出 (见 arch-structure.md)
- 配置分层遵循 CLI > ENV > CONFIG > DEFAULT, 敏感字段只出现在 env / 独立文件

## 取舍与风险

| 主题 | 结论 | 影响 |
| ---- | ---- | ---- |
| 管理权威并入 relay vs 独立 controller 角色 | 并入 relay | relay + daemon 即闭环, 角色最少; 控制台退化为无状态 UI; 代价是 "中继" 语义变重, 必须区分核心 / 卫星两种模式 |
| 单网络 vs 多网络组 | 多网络组进 MVP | 全链路多一个网络维度; 换来不同圈子天然隔离、免 ACL 前置, 且零代码期引入成本最低; 节点多 membership 后置 |
| 卫星中继 vs 仅核心内置转发 | 双模式共存 | 多一套卫星注册与列表分发; 换来就近部署与带宽分担的扩展位, 默认部署零额外成本 |
| 控制台独立进程 vs 仅 relay 托管 | 两者共用同一套嵌入资源 | console 子命令多一点宿主代码; 换取 admin API 不对外暴露时也能本地用 UI |
| 用户态 WG 引擎 vs 内核态 | 统一用户态 | 牺牲部分吞吐上限 (个人场景足够), 换取跨平台一致与零系统依赖 |
| gotatun (boringtun 维护 fork) | 首选, 不可用则 vendored boringtun | 上游维护节奏不受本项目控制, 实施期需锁定并验证, 必要时 fork 自维护 |
| WS over TLS vs QUIC 中继 | WS over TLS | 复用 443 与 HTTP 生态, 实现简单; 代价是 TCP-in-TCP 拥塞影响与封装开销, 需按最坏路径推导 TUN MTU 并在 e2e 大包用例验证 |
| SQLite vs Postgres | SQLite (核心中继) | 20+ 节点规模足够; 未来多核心扩展时需迁移 |
| 公共 STUN vs 自建 | 首发公共 STUN, 核心可配置列表 | 减少部署组件; STUN 无鉴权语义, 不构成信任边界问题 |
| 证书: 默认自签 + CLI 指纹固定, 可选 ACME | 自签优先 | 免域名开箱即用; 浏览器侧自签需首次手动信任, 有域名用户可走 ACME |
| 打洞成功率 | 不追求 100%, 策略可强制中继 | 对称 NAT 等场景依赖中继, 属设计内行为; 强制中继覆盖出口不稳定场景 |
| 排障事件: daemon 内存事件环 vs 解析日志文件 | 内存事件环 | status --verbose 免文件解析、内存有界、IPC 直取; 代价是进程重启即清零, 历史回看仍依赖日志文件 |
| daemon 日志默认 stderr vs 默认写文件 | 默认 stderr, `--log-file` 显式启用 | 免自行维护轮转, 服务化后 journal 天然接管; 代价是手动后台运行时需显式开文件或自行重定向 |
