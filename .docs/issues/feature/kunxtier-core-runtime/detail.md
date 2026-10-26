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

## T15 实施拆解 (可观测性基础)

规划裁决 (2026-10-26 ik-plan):

- daemon 默认 stderr, `--log-file` 显式启用文件输出 (stderr / 文件双写), 文件按大小轮转保留一份历史; 不默认写文件, 服务化 (T14) 后由 systemd / launchd journal 接管
- 级别优先级: CLI flag > relay 配置文件字段 > RUST_LOG > 默认 info; daemon / up 走 flag + env, 不引入节点配置文件 (沿 T09 裁决)
- 节点事件环: NodeEvent (时间戳 / 类别 / peer / detail) 有界环形缓冲, 容量 256, 只记打洞 / 路径切换 / 信令 / 注册四类; 事件点同源双写 tracing 与环; 对端最近失败原因与中继选择过程从环推导, 不另存状态源; `status --verbose` 经 IPC 拉取
- relay 侧不建事件环, 按 FR-4 清单补齐日志 (节点注册 / 凭证吊销 / 令牌校验失败 / 卫星注册与摘除 / admin 登录失败)
- 脱敏按 NFR-4 全量审计现有日志点

交付按三个子切片推进 (任务仍归 T15 单条, 不拆 tasks):

1. 日志基建: 级别与输出目标配置面 (CLI flag / relay config 字段 / 文件写入与轮转)
2. 节点事件环与 `status --verbose` (node + ipc + cli)
3. relay 事件补齐与脱敏审计

架构补记见 arch-kunxtier-overview "可观测性"。

## T11 实施拆解 (控制台后端)

规划裁决 (2026-10-26 ik-plan):

- 会话: 内存表 + Bearer 令牌 (TTL 12h, 多会话并存, 登出端点), require_admin 双轨接受 (会话令牌 / admin 凭证), relay 重启即失效; 登录失败沿用 admin 认证限速
- 路径上报: 信令协议扩展——节点上报逐对端路径矩阵 (对端名 → direct / 中继号), 路径切换事件触发 (与 T15 事件留痕埋点同位) + 60s 周期兜底; 核心聚合每节点路径明细与每中继连接节点数
- console 子命令: 本地 HTTP 托管嵌入资源 + admin API 反代 (TLS 指纹固定), 浏览器仅连本地进程, 无 CORS
- 静态嵌入: 构建产物嵌入二进制, T11 落机制与占位资源, T12 填真前端
- ACME: 本切片不做, 配置面预留字段并明确报错, 后续独立切片
- 节点版本: 注册请求携带版本号入库, 总览展示

交付按子切片推进 (任务仍归 T11 单条, 不拆 tasks):

1. 会话与 admin API 扩展 (login / logout / relays 汇总 / nodes 列表扩展)
2. 节点路径上报与核心聚合 (信令协议 + node 上报点 + relay 聚合)
3. 静态嵌入与 console 子命令 (SPA 托管 + 反代骨架)

架构补记见 arch-kunxtier-overview "控制台后端"。

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
- 2026-10-26 (ik-advance): 交付路径管理切片 (T08)。四项落地: ① FR-6 直连/中继切换恢复精化——探测帧升级为 PING/PONG 两类 (帧加 1B 帧类, PONG 回显令牌且不再触发应答, 消除回声环, Direct 态得以安全发起周期保活 PING: 15s 节奏 / 60s 无探测与 WG 流量证据即判失活, 回落中继并在 30s 重试节拍重打洞); Punching 携带 fallback 既有端点, 重试直连期间中继流量不断; PunchState 四态化 (Punching{deadline,token,fallback} / Direct{endpoint,last_alive,next_ping} / Relayed{endpoint,next_retry})。② FR-14 本地网段冲突检测——netconf 模块 (if-addrs 0.15 枚举接口), 接口网络与所属网络 /16 双向区间相交判定, 纯函数化供单测与 T09 daemon 启动前置复用; Ready 事件 (vip 权威已知) 检测, 冲突即拒绝运行, 诊断含接口名 / 地址 / 处理建议 (NodeStatus.fatal 承载); MVP 只比对接口地址, 路由表全量解析跨平台脆弱列为 e2e 反馈项。③ FR-16 MTU 精推——DEFAULT_MTU 1280→1440: 直连为硬约束 (外层 1500 - 60 WG 封装), 中继 TCP 流式分段吸收 TLS+帧头叠加无硬上限, 取直连上限即同时覆盖 e2e 1400 字节级 ICMP (1428≤1440); 超MTU 内层 UDP 由内核 IP 分片, 行为显式定义。④ FR-15 保活复核——direct WG keepalive 25s / 信令 10s 心跳 / 中继 WS 心跳均已就位, T08 补的是直连活性可观测 (周期 PING)。实施期修复 (T07 遗留缺陷): 强制中继下对端探测帧会把引擎端点拉回真实地址破坏 FR-10——现 ForceRelay 完全忽略探测 (不学端点不应答); 中继通道原先仅 5s 对齐节拍建连, 列表首达 (State/Relays) 即建, 失活判定与短打洞窗口不再落空。新增 PathTimings (ping/dead_after/retry/punch_window) 进 NodeOptions, 生产默认 15s/60s/30s/5s, e2e 以小值驱动状态机。测试: 探测帧 PING/PONG 编解码与四态 endpoint 语义 / netconf 判定 (CGNAT /10、p2p /32、相邻网段、超网 /8、本机 TUN 排除) 单元 + 运行时 e2e 三场景 (原直连-强制中继-恢复场景保持通过; 黑洞回落场景增强重试振荡期 fallback 流量不断; 新增静默失活-回落中继-周期重试-切回直连 FR-6 全链); 全量 84 项通过 (cargo make 含 fmt/clippy)。提交前预检 (ik-commit, mindset/standards/deep-audit) 修复三项: ① on_punch_expired 的 fallback 未校验中继在线性——中继在打洞重试窗口期死亡 / 失联 (如卫星与节点分区但与核心保持注册) 时状态机锚定死伪端点无法自愈, 现回退前经 split_pseudo + relay_online 校验, 失效即走 pick_relay 重选; ② netconf::mask 对 prefix > 32 的越界输入按 /32 主机收口 (pub 字段可构造, 原实现移位下溢 panic); ③ on_probe_tick 超单函数 50 行红线, 拆出无 IO 的 scan_tick_actions (扫描 / 执行分离, 贴合 State/Event/Action 分层)。修复后全量 85 项通过。T08 done; 下一步 T09 CLI 子命令与本地 IPC (daemon/up/down/status 组装 NodeOptions 与 netconf 前置)。
- 2026-10-26 (ik-advance): 交付 CLI 子命令与本地 IPC 切片 (T09)。kunxtier-node 新增四模块: state (node.json 版本化持久化, 目录 0700 文件 0600, 集中承载节点凭证 / WG 私钥 / vip / core / 指纹 / 监听端口, 原子写)、register (一次性令牌注册客户端, ureq 指纹固定经 core::tls, spawn_blocking 承载)、ipc (UDS + JSON 行协议, 单连接单请求, 64KB 帧上限, 残留 socket 探活清理, 未运行形态由 CLI 转 FR-2 启动指引)、daemon (会话生命周期: up 注册/重连、down 有界停机、status 快照; daemon 启动有 state 即自动上线 (FR-9 前提), 意外退出指数退避重连 (起步值可配 30s 封顶, 存活满 60s 复位), 网段冲突 (FR-14 前置, 与运行时 Ready 复核同函数) 与致命态封停不重连; TunFactory 可插拔 IP 层——生产真实 TUN / 测试 gotatun 通道 (Keep 句柄防 UDP 半边提前关闭))。runtime 增 done 退出通知 (NodeHandle::done watch), 会话监督以此观测。CLI 新增 daemon / up / down / status 四顶级子命令 (FR-1), up 语义裁决: 令牌非空走注册 (同名重注册 FR-17 沿用身份换发凭证, name/core/指纹与 state 不一致即拒绝), 令牌为空以既有 state 直接重连 (排障后的恢复入口); WG 监听端口缺省自动选闲并随 state 持久化。结构迁移: pinning_agent (ureq) 自 relay::client_tls 移驻 kunxtier-core::tls (节点注册需同源装配且 node 不得反向依赖 relay; relay 的 ureq 降为 dev-dep)。实施期修复两缺陷 (集成测试捕获): ① serve 退出后 accept_loop 残留持 listener 与 ops 通道——既挡 daemon 重启探活又吞 IPC 应答, 现 accept 任务随 serve 生死 (abort + socket 文件清理); ② 测试注入通道的 UDP 半边被 open 内 drop, gotatun 设备任务即报错, 现随会话以 Keep 句柄持有。测试形态裁决: axum-server Handle 管不到已升级的 WS 连接 (升级后流归内部任务), 核心崩溃模拟改为独立子 runtime + shutdown_background 丢弃任务组。测试: state 往返 / 版本拒读 / 权限收紧、IPC 帧编解码 / 超限拒绝 / 回环、daemon 集成两场景 (up-down-同名重注册-daemon 重启自动恢复全链; 双节点对端可见-核心崩溃离线-原端口复活退避重连); 全量 94 项通过 (cargo make 含 fmt/clippy), 二进制 --help 与 daemon 未运行指引冒烟通过。已知妥协: status --verbose 排障视图归 T15; Windows IPC 以 Unsupported 报错占位归 T13; 节点配置文件本切片不引入 (state 已承载 core/指纹), T14 服务化再评估。T09 done; 下一步 T10 按 e2e-kunxtier-network 场景矩阵端到端验证 (daemon 已可真实运行)。
- 2026-10-26 (ik-advance): 交付可观测性基础切片 (T15)。三段落地: ① 日志基建——cli 新增 logging 装配模块, 级别优先级 CLI flag > relay 配置文件 (FileConfig 新增 log_level, 仅作级别解析的文件层, 不进 RelayConfig 运行时类型) > RUST_LOG > info; flag / config 层非法值即时报错, RUST_LOG 透传 EnvFilter 模块过滤语义; relay 与 daemon 子命令各增 --log-level / --log-file, 文件启用后 stderr / 文件双写, 自实现 10MB 单份 .old 轮转 (rename 失败退化为丢弃重建, 不中断日志链路); subscriber 初始化自 main 头部移至解析后装配位 (relay 配置文件预读一次取 log_level, execute 内正式加载一次, 双读无害); Fail 补 Debug derive。② 节点事件留痕——node 新增 events 模块: EventKind 四类 (punch / path / signal / register)、EventRing 容量 256 满即淘汰最旧; 实施期更名裁决: 规划名 NodeEvent 与 signal::NodeEvent 撞名, 更名 EventRecord。daemon 持环跨会话复用, runtime (打洞成败与回落 / 直连失活 / 策略切换 / 中继通道建立变更失联 / 对端离线 / 信令关闭 / STUN 无结果) 与 daemon (注册成败 / 会话建立 / 退出 / 自动恢复与重连失败) 事件点与既有 tracing 埋点同位双写, 日志字段结构不变; IPC 协议增 StatusVerbose 请求与 events 应答字段, daemon 回传走 snapshot_bounded (56KB 预算自最新向旧收取, 保 64KB 单行帧上限), CLI `status --verbose` 输出最近事件; 对端失败原因与中继选择过程从环推导, 不另存状态源。③ relay 侧 FR-4 补口——注册被拒 warn 留痕 (含来源 ip 与失败类别, 不含令牌明文), 其余四类 (凭证吊销 / 令牌校验失败 / 卫星注册摘除 / admin 认证失败) 盘点确认已有; NFR-4 脱敏全量审计通过 (令牌 / 凭证 / 私钥未入日志, 错误串不含敏感值)。测试: 级别优先级与非法值拒绝 / 轮转边界与重开追加 / 事件环容量序与有界快照 / IPC verbose 往返 / 配置文件 log_level 解析单元 + daemon 集成补 verbose 断言 (注册与信令事件可见); cargo make 全量通过 (fmt / clippy / 测试)。预检闸门捕获一项: verbose 全量回传在长 detail 积压时可越 64KB 帧上限, 以 snapshot_bounded 收口后清零。T15 done; 下一步 T10 e2e 场景矩阵 (环境就绪后可与人工验证 issue 合并执行) 或 T11 控制台后端。
- 2026-10-26 (ik-advance): 交付控制台后端切片 (T11)。三个子切片: ① 会话与 admin API 扩展——relay 新增 session 模块 (内存会话表, kxt_ses_ 前缀 144 bit 熵, TTL 12h, 惰性过期, 多会话并存, 登出幂等); POST /api/admin/login (失败沿用 admin 限速) / logout; require_admin 双轨 (会话令牌内存校验优先, admin 凭证 CLI 沿用)。GET /api/admin/relays (在线卫星 + 内置条目, 卫星计数自路径上报聚合, 内置取转发注册表实际连接数——新增 ForwardRegistry::online_count); nodes 列表补属主过滤 (?owner=) 与候选地址 / 版本 / 路径快照合并 (OnlineRegistry 扩 version / paths 槽位, 代际校验同 candidates)。② 节点路径上报——信令协议扩展 SignalClientMsg::PathReport { paths: Vec<PeerPathReport> } (对端名 / idle-punching-direct-relayed / relayed 附中继地址, 以隧道实际拨号地址上报而非中继号, 免列表序漂移误归因); Hello 增 version (节点二进制版本)——实施期偏差: 版本经信令 hello 上报入库注册表而非注册请求落库, 免 store schema 迁移且节点升级重连后实时刷新; runtime 转换事件置脏 (note() 同位) 经 probe 节拍冲刷 + 60s 周期兜底, try_send 满即丢弃待下轮补报; 核心聚合 paths_by_relay (每节点每中继只计一次)。③ 静态托管与 console 子命令——实施期裁决: 控制台资产随消费方走 (relay crate assets/console/, include_str 两跳), 放弃越出 workspace 五级相对路径直取 web/ dist 的做法, T12 前端构建任务 (cargo-make) 负责将 web/kunxtier-console 产物输出至此, 嵌入为编译期行为、部署单二进制自包含; 核心以 SPA fallback 托管 (/api 与 /ws 未匹配保持 404 JSON); `kunxtier console --relay --fingerprint [--listen]` 起本地明文 HTTP 托管同一份嵌入资源并反代 admin API (ureq 指纹固定 + spawn_blocking, 浏览器仅连本地, 无 CORS 面; 仅 REST, 不反代信令/转发 WS)——ureq 自 relay dev-dep 迁回正式依赖 (本地反代直接使用其类型)。测试: 会话生命周期与惰性过期单元 / 路径聚合与代际校验单元 / 会话登录-登出-限速与 relays-nodes 扩展集成 (console_admin.rs 三用例) / 本地控制台页面-SPA 回落-反代登录-404 透传集成 / path_runtime e2e 补 admin 路径视图断言 (直连与回落场景, 回落断言中继归属核心地址); cargo make 全量通过 (fmt / clippy / 测试)。已知妥协: server.rs handler 集中约 530 行 (拆分信号未达阈值, 随 console 扩张再评估拆 handler 模块); 旧核心遇 PathReport 报消息格式非法、客户端忽略, 版本混跑降级可接受 (同版本部署为常态)。ACME 未实施, 配置面预留随后续切片。T11 done; 下一步 T12 Web 控制台前端 (web/kunxtier-console 源码 + 构建链输出 assets/console)。
