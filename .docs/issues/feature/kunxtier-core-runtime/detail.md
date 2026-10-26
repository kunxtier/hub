# kunxtier 首阶段核心运行时

## 背景

kunxtier (轻量 P2P 虚拟组网, ZeroTier / EasyTier 平替) 规划已完成: 需求见 `.docs/requirements/feat-kunxtier-*` 与 `nfr-kunxtier-security`, 架构见 `.docs/architectures/arch-kunxtier-overview.md`。本 issue 承载 MVP 实施: 跑通 Linux / Windows / macOS + Web 控制台 + 卫星中继的核心闭环。

## 目标

- 单二进制 `kunxtier` 覆盖 relay 核心模式 (管理 + 信令 + 内置转发 + 控制台托管) / relay 卫星模式 (纯转发) / console (无状态控制台) / daemon / 管理 CLI
- relay + daemon 即闭环: 不开控制台、不部署卫星, 网络也完整可用
- 节点注册、密钥分发、直连打洞、中继回落与策略、卫星扩展全链路自动完成
- macOS、Linux、Windows 三平台提供可运行发布产物
- 通过 `e2e-kunxtier-network.md` 场景矩阵验收

## 范围

- `rust/kunxtier/` cargo workspace: cli / relay / node / core 四 crate, 依赖方向 cli → relay / node → core
- 核心中继 (relay 核心模式): 多网络组管理 (REST + 信令 WS, 网络组间完全隔离) + 内置密文转发 + 控制台托管 + SQLite 持久化
- 卫星中继 (relay 卫星模式): 纯转发, 以 relay 令牌向核心注册, 无管理数据无存储
- 节点 daemon: TUN + WireGuard 用户态引擎 (gotatun / boringtun + tun-rs, 实施期锁定选型并验证) + 打洞 + 路径策略; 保活 (keepalive + WS 心跳)、MTU 按中继最坏路径推导、本地网段 (CGNAT) 冲突检测
- 节点身份双凭证: WG 私钥 (数据面, 不离机) + 节点凭证 (注册时签发仅存哈希, 信令挑战应答); 公开端点限速与失败退避
- daemon 服务化 (install / uninstall) 与三平台特权; 分级结构化日志与 status --verbose 排障视图
- Web 控制台: `web/kunxtier-console/` 前端, 构建产物嵌入二进制; 核心托管与 `kunxtier console --relay` 双入口; 网络切换 / 总览 / 节点 / 令牌 / 策略 / 中继
- 路径策略两档: 全局自动 (直连优先回落中继) + 节点级强制中继
- CLI 子命令: relay / console / daemon / up / down / status / admin, 本地 IPC
- 打洞走公共 STUN (核心可配置列表) + 信令协调
- 配置分层 CLI > ENV > CONFIG > DEFAULT; 私钥 / 令牌 / admin 凭证 / relay 令牌只走 env / 独立文件 / state 目录

## 非目标

- 移动端 (iOS / Android); 扫码加入仅预留令牌编码约定
- 网内名称解析 (DNS / hosts 注入), 名字查询由 status 与控制台承载
- 属主自助管理 (用户凭证体系)
- 子网路由 / 出口网关
- L2 以太网仿真
- ACL 与用户间隔离、多租户、SSO
- 一个节点同时加入多个网络 (多 membership)
- 多核心中继组网与高可用 (EasyTier 式状态同步)

## 方案讨论

- 核心抽象: relay 即中枢——核心中继是唯一管理权威 (SQLite), relay + daemon 即闭环; 控制台是无状态可插拔 UI, 卫星中继是零知识纯转发扩展
- 网络组: network 是核心中继内的一等隔离域, 每网独立用户 / 令牌 / 节点 / IP 段 / 策略, 网间不互通不可见; init 建默认网络, 单网用户无感知
- 单权威约束: 每张网络恰好归属一个核心中继, 卫星永不持有管理数据且不感知网络, 多核心为后续演进
- 数据面: 复用 WireGuard 协议, 全平台统一用户态引擎, 不依赖系统 WireGuard; 中继只见密文, 端到端加密不受部署形态影响
- 路径策略: 两档 (全局自动 + 节点级强制中继), 控制台配置, 信令下发
- 互通模型: MVP 全互通; 属主为标注字段 (无用户凭证, admin 代管, 属主自助为后续演进)
- 用户体系: 令牌登记制, 无注册登录与审批流 (令牌即授权)
- 证书: 默认自签 (CLI / daemon 通道叠加指纹固定), 可选配置域名走 ACME
- 详细取舍见 arch-kunxtier-overview "取舍与风险"

## 风险与边界

- gotatun (boringtun 维护 fork) 可用性未最终核实; 退路是 vendored boringtun 或 fork 自维护, 不改变模块边界
- Windows 真机验证依赖测试环境, 排期时需提前准备
- 对称 NAT 等拓扑打洞必然失败, 依赖中继兜底, 属设计内行为
- 用户态引擎吞吐上限低于内核态, 个人场景 (20+ 节点) 可接受
- e2e 验证依赖核心中继机器、卫星实例与多平台客户端环境, 排期时需提前准备
- 前端构建链嵌入 Rust 二进制为新增工序, 首次搭建成本计入 T11 / T12
- WS 中继的 TCP-in-TCP 拥塞与封装开销: MTU 必须按最坏路径推导, e2e 大包用例 (ping -s 1400 / 大文件 TCP) 为必过关卡
- 运营商 CGNAT 与 100.64.0.0/10 冲突 (国内高发): 冲突检测与诊断不做好, 会出现 "装完即断网" 且难排查的事故
- "中继" 同时承担管理权威与转发双语义, 文档与命名必须始终区分核心 / 卫星模式, 防止实现期概念混淆

## 验收口径

- `e2e-kunxtier-network.md` 场景矩阵全量通过
- `nfr-kunxtier-security.md` 验收标准逐条核对
- macOS / Linux / Windows 发布产物由统一构建入口产出并实际运行验证

## 推进记录

- 2026-10-20 (ik-advance --init): 交付控制面登记闭环。rust/kunxtier workspace (core / relay / cli 三 crate) 与根 CLAUDE.md 落地; `relay --init` (自签证书 / admin 凭证 / 默认网络 / key 0600)、`relay` 运行 (HTTPS 管理 API)、admin 全套管理命令 (含 user disable)、节点一次性令牌注册 (IP 分配 + 节点凭证签发)、注册与认证限速、`relay backup`。28 项测试通过 (单元 25 + 集成 3), 二进制冒烟通过。T01-T03 done; 下一步 T04 信令 WS (节点凭证挑战应答)。
- 实现期修复的两个缺陷 (测试守恒捕获): next_ip 漏递增导致 IP 重复分配; 节点同名唯一约束未排除已移除行, 挡住 FR-17 的同名重注册。
- 2026-10-21 (ik-commit): 交付信令 WS 切片。`/ws/signal` 节点凭证挑战应答 (nonce + sha256 摘要, 明文与哈希均不传输, 2026-10-21 协议裁决)、内存在线注册表与上下线秒级广播 (FR-5)、节点表与对端公钥分发、打洞信令同网定向转发、心跳 10s / 空闲 30s (AuthOk 下发)、建连限速与消息大小上限 (NFR-9); admin 移除节点 / 禁用用户即踢线 (存量凭证实时吊销), 节点列表并入在线态。配套仓库根 Makefile 与 git hooks (`make hooks` 装载)。T04 done; 下一步 T05 转发双模式。
- 复盘 (T05 闸门): T04 提交时未跑全量测试, 带病提交两个缺陷测试——node_names_of_user_filters_owner 对一次性令牌消费两次; revocation 用例未按 "禁用即 hello 拒绝" 的统一口径编排。行为实现本身正确, 修测试预期。此后提交前必须全量 cargo make。
- 2026-10-21 (ik-advance): 交付转发双模式切片。核心 `/ws/relay` 转发面: 复用信令挑战应答准入 (IP 由库存权威决定), 二进制帧 `[4B 目的 IPv4][WG 密文]` 按虚拟 IP 路由, 未知目的 UDP 语义静默丢弃 (FR-4); 卫星进程 `relay --satellite`: 声明式 bind 准入 (零知识, NFR-5), 无状态目录, 证书每次启动临时生成; `/ws/satellite` 以可复用 relay 令牌注册 (令牌撤销即按令牌踢线), 中继列表 (卫星按地址序在前 + 核心内置末位兜底) 经信令 State 下发并在注册 / 断连时全网广播 (FR-9 / FR-18), 条目携带各自 TLS 指纹; 卫星注册断线指数退避重连 (1s 起步 30s 封顶)。配置层新增 advertise / SatelliteConfig (relay 令牌只走 env KUNXTIER_RELAY_TOKEN)。测试: 转发注册表 / 卫星列表 / relay 令牌校验 / 帧编解码 / 配置分层单元 + 核心两节点互通、卫星真实进程注册分发-声明转发-同 IP 顶替-死亡摘除、非法令牌不进列表集成; 全量 58 项通过, 二进制卫星模式冒烟通过。T05 done; 下一步 T06 kunxtier-node TUN 与 WG 用户态引擎。
- 2026-10-21 (ik-advance): 交付节点数据面引擎切片。kunxtier-node crate 落地: 选型锁定 gotatun 0.7.1 (ring + device, `=0.7.1` 锁版, 0.7.2+ MSRV 1.95 超工具链) + tun-rs 2.8 (async); WgEngine 装配 (私钥 / 固定监听端口 / 可插拔传输) 与运行时 peer 管理——sync_peers 节点表全量对齐 (endpoint 未声明时保持已学端点)、set_endpoint 打洞回填缝、listen_port 候选上报源; TunAdapter 自实现 gotatun IpSend/IpRecv (设备 / 地址 / MTU 自持, MTU 保守 1280 待 T08 精推); WgIdentity 密钥对 (公钥 base64 与注册协议对齐)。绕开 gotatun 0.7.1 运行时加 peer 不触发连接建立的语义 (首次 sync 经 set_listen_port 落地绑定)。测试: 密钥 / peer 映射单元 + 引擎级 e2e (双引擎真实 noise 握手、UDP 环回双向互通、端点学习、节点表摘除即停转发); 全量 64 项通过。真实 TUN 特权冒烟为 #[ignore] 手动用例。T06 done; 下一步 T07 打洞与路径策略。
- 2026-10-26 (ik-advance): 交付打洞与路径策略切片 (T07)。kunxtier-node 新增 transport / stun / punch / relay_conn / signal / runtime 六模块: 复合传输 (真实 UDP 入向按前缀分流 WG / STUN / 探测, 中继 WS 折叠为 240/4 段伪端点, 引擎端点漂移天然承担路径切换)、STUN 自实现最小客户端 (无属性 Binding + XOR-MAPPED-ADDRESS, 不引入新依赖, 服务器列表经信令 State 下发)、并发打洞 (250ms 突发 / 5s 窗口, 帧携带本节点名, 收到对端探测即判直连——观测源即双向通路证明, WG 流量退化为漂移监测)、超时回落中继与节点级强制中继 (FR-3 / FR-10)、信令客户端 (挑战应答 / 心跳 / 事件流)。relay 侧: store schema v2 (nodes.policy)、admin API 与 CLI `admin node policy`、Policy 定向推送、Announce 候选更新即 PeerOnline 广播 (打洞触发源)。两个实施期裁决: 转发帧扩为 [4B 目的][4B 源][密文] (下行帧需源字段构造回程伪端点, T05 帧格式修订); PinVerifier 移驻 kunxtier-core::tls (卫星 / 节点共用)。测试: 协议帧 / 伪端点编解码 / STUN 编解码 / 探测帧 / 策略存取单元 + 运行时 e2e 两场景 (本地伪 STUN 真实探测: 直连打洞-强制中继-恢复自动-离线清理全链; 说谎 STUN 黑洞候选: 双侧超时回落且中继路径互通); 全量 78 项通过, 二进制 policy 子命令冒烟通过。T07 done; 下一步 T08 路径管理 (直连/中继切换恢复精化、CGNAT 冲突检测、保活与 MTU 精推)。
