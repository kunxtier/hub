# hub 仓库结构约定

## 背景与目标

hub 是按语言组织的 monorepo, 承载多个独立项目, 项目成熟后整体拆分到独立仓库。当前主力语言为 Rust, Web 前端随 kunxtier 控制台落地。结构约定的核心目标: 项目之间零构建耦合, 任意项目目录可整体迁出而不需要回改仓级配置。

## 方案概览

```text
hub/
├── .docs/                      # 全仓文档, 多项目共用, slug 与分组区分归属
│   ├── requirements/           # feat- / nfr- / rule-
│   ├── architectures/          # arch- / impl- / infra-
│   ├── testing/                # func- / e2e- / perf- / reg-
│   └── issues/                 # feature / bug / optimization / refactor
├── rust/                       # Rust 项目根
│   └── kunxtier/               # 自包含 cargo workspace (独立 Cargo.lock)
├── web/                        # 前端项目根
│   └── kunxtier-console/       # kunxtier Web 控制台前端
├── .claude/                    # 工作流规则与 skills
└── CLAUDE.md                   # 首个代码项目落地时创建
```

## 设计原则与约束

- 按语言分顶层目录, 语言内按项目分目录
- 一个项目可横跨多个语言目录 (如 kunxtier = rust/kunxtier + web/kunxtier-console), 项目名保持可对应; 拆仓时同项目的各语言目录一起迁出
- 每个项目自包含: 独立 workspace / lockfile / 测试 / 构建入口, 不共享根级构建配置
- 项目内跨语言构建链 (如前端产物嵌入 Rust 二进制) 属项目自身事务, 允许存在, 但不得反向影响其他项目
- 项目间原则上不共享代码; 确需共享时通过发布依赖, 不建仓内跨项目 path 依赖, 避免拆仓时产生隐性耦合
- 文档归属: `.docs` 全仓共用, slug 携带项目名 (如 `feat-kunxtier-*`), 各目录 `overview.md` 分组索引标注归属
- 拆仓动作 = 同项目全部语言目录迁出 + `.docs` 内对应前缀组迁出, 不修改其他项目

## 取舍与风险

| 主题 | 结论 | 影响 |
| ---- | ---- | ---- |
| 单 workspace vs 项目级 workspace | 项目级 workspace | 牺牲仓级统一依赖版本管理, 换取零成本拆仓 |
| 文档集中 vs 项目自带 | 集中 `.docs` | 全局可检索; 拆仓时文档需同步迁出 (接受) |
| 项目横跨语言目录 vs 单一项目目录 | 按语言分目录 | 同项目文件物理分离, 靠命名对应; 换取各语言工具链的目录约定不被打破 |

## 当前实际结构 (2026-10-27, ik-advance T12)

```text
hub/
├── .docs/                        # 规划文档全套已落地
├── rust/kunxtier/                # 已落地
│   ├── Cargo.toml                # workspace 根 + 统一依赖版本
│   ├── Makefile.toml             # cargo-make 任务集 (构建 / 质量 / 测试 / 运行 / 发布)
│   └── crates/
│       ├── kunxtier-core/        # 协议 (信令 / 转发帧 / 卫星注册) / 凭证令牌 / 地址 / 分层配置 (含 SatelliteConfig, stun_servers, log_level)
│       │                         #   + tls (指纹固定 PinVerifier 与 ureq pinning_agent, 卫星 / 节点 / admin CLI 共用)
│       ├── kunxtier-relay/       # 核心中继: store (schema v2 含 nodes.policy) + server + init + cert + ratelimit
│       │                         #   + signaling (信令 WS, 候选广播 / 策略推送 / 路径上报聚合) + forward (转发 WS 双准入)
│       │                         #   + satellite (注册表 + 卫星进程) + session (admin 会话) + console (include_dir 文件表静态托管与本地反代)
│       │                         #   + assets/console/ (前端构建产物嵌入; 未构建时为随 git 的占位页)
│       ├── kunxtier-cli/         # 二进制 kunxtier: relay (核心 / --satellite 卫星) / admin 子命令 (含 node policy)
│       │                         #   + 节点子命令 daemon / up / down / status --verbose (IPC 客户端) + console (本地控制台)
│       │                         #   + logging (日志级别与输出目标装配: flag / RUST_LOG / 文件轮转)
│       └── kunxtier-node/        # 节点数据面: engine + tun + identity + netconf (网段冲突检测)
│                                 #   + transport (复合传输: UDP 分流 + 中继伪端点) + stun + punch (PING/PONG 探测与路径状态机)
│                                 #   + relay_conn (转发 WS 客户端) + signal (信令客户端) + runtime (事件循环装配, PathTimings 节奏)
│                                 #   + state (node.json 版本化持久化) + register (一次性令牌注册客户端)
│                                 #   + ipc (UDS + JSON 行协议) + daemon (会话生命周期与 IPC 服务端)
│                                 #   + events (关键事件留痕环, status --verbose 数据源)
├── CLAUDE.md                     # 已创建
└── web/kunxtier-console/         # 控制台前端 (pnpm + Vite + React + TS + Tailwind v4 + shadcn 风格基件)
                                  #   src: api (client / types / hooks) + state (AppContext) + components (布局 / 切换器 / ui 基件)
                                  #   + pages (Login / Overview / Nodes / Tokens / Networks / Relays) + lib (cn / summary)
                                  #   产物经 cargo make console-build 输出 relay assets/console/ (vite outDir 直出)
```

## 演化决策记录

- 2026-10-20 (init): kunxtier-node crate 未建——数据面阶段再落地, 不预建空壳; web/ 同理。已交付切片: 控制面登记闭环 (init / HTTPS 管理 API / 节点注册 / 限速 / 备份)。
- 已知妥协: store 用 std::sync::Mutex 包同步 SQLite, handler 内短临界区; 信令阶段的并发压力实际显现时再评估异步化。
- 已知妥协: 限速器过期表项不主动清理, 源数量自然有界 (单机个人规模)。
- 2026-10-21 (T04 信令 WS 后): forward / satellite 以独立模块落在 relay crate 内 (协议与注册表在 core / relay 的既有分层内), 未达拆分阈值。
- 2026-10-21 (T05 转发双模式): 转发准入有意不对称——核心复用信令挑战应答 (IP 由库存权威决定, 不可声明), 卫星声明式 bind (零知识, NFR-5); 声明劫持面由 WG 端到端加密兜底。
- 2026-10-21 (T05): 卫星无状态目录——证书每次启动临时生成, 指纹随注册上报并经中继列表分发; 节点按条目指纹固定校验, 免去卫星证书分发工序。
- 2026-10-21 (T05): 中继列表顺序为协议语义——卫星在前 (按地址稳定序), 核心内置恒在末位兜底; 内置条目 addr 为空串时表示 "节点已连接的核心自身" (通配监听且未配 advertise 时出现)。
- 2026-10-21 (T05): 卫星进程拆 start (返回实际监听地址与句柄) / run (CLI 前台) 两级, 供集成测试以真实进程形态起停。
- 2026-10-21 (T06 选型锁定): WG 用户态引擎用 gotatun 0.7.1 (Mullvad 维护的 boringtun fork), 以 `=0.7.1` 精确锁定——0.7.2 起 MSRV 1.95 超出当前工具链 (rustc 1.90), 工具链升级后再评估抬升; feature 取 ring + device (default-features 关闭排除 aws-lc-rs, 与 rustls 选型同源)。退路 boringtun 0.7.1 (Cloudflare) 保持在位未启用。
- 2026-10-21 (T06): TUN 用 tun-rs 2.8 (async); 不启用 gotatun 的 tun feature (其绑定的是另一 tun crate), 由 kunxtier-node 自实现 gotatun IpSend/IpRecv 适配, 设备创建 / 地址 / MTU 归 kunxtier 自持 (T08 路由与冲突检测直接可用)。
- 2026-10-21 (T06): gotatun 0.7.1 运行时加 peer 不触发连接建立 (reconfigure 只认密钥 / 端口变更 / 清空), 引擎装配时不设端口, 首次 sync_peers 经 set_listen_port 0→P 变更落地 UDP 绑定; listen_port 拒绝 0 (固定端口是打洞与候选上报前提)。
- 2026-10-21 (T06): TUN MTU 先取保守值 1280 (DEFAULT_MTU), FR-16 的最坏路径精推与大包验证随 T08 落地。
- 2026-10-21 (T06 测试形态): 引擎级 e2e 免特权——TUN 侧用 gotatun 通道 (UDP 语义), UDP 侧为真实 127.0.0.1 socket, 双引擎真实 noise 握手与双向互通; 真实 TUN 设备冒烟为 tun 模块 #[ignore] 用例 (需 root, 手动执行)。
- 2026-10-26 (T07 帧格式修订): 转发帧由 [4B 目的][密文] 扩为 [4B 目的][4B 源][密文]——下行帧若无源字段, 接收方无法构造中继伪端点 (引擎按包源漂移端点); 中继仍只按首部目的路由, 零知识不变, T05 存量测试随修订。
- 2026-10-26 (T07): 中继路径以 240/4 段伪端点折叠进引擎的 UDP 语义——伪地址 240.{中继号}.{vip 后两段}:{vip 第二段} (vip 约束在 100.64/10, 第二段 64..127 恒落端口区间); 直连与中继切换由 gotatun 端点漂移天然承担, 免独立转发状态机。
- 2026-10-26 (T07): STUN 自实现最小子集 (Binding 无属性请求 + XOR-MAPPED-ADDRESS 解析), 不引入 STUN 库; 服务器列表为 relay 配置 (env 整组覆盖 / 文件 / 公共缺省), 经信令 State 下发。
- 2026-10-26 (T07): 打洞成功判据 = 收到对端探测帧 (观测源即双向通路证明: 对端 NAT 映射对本地址可用); WG 握手是按需的, 首个业务包才触发, 不作为直连判定必要条件, WG 流量事件退化为端点漂移监测。探测回执条件 "打洞中或新令牌", 防回声环且支撑策略恢复后的重打洞。
- 2026-10-26 (T07): PinVerifier 自 relay::client_tls 移驻 kunxtier-core::tls——节点信令 / 转发客户端与卫星客户端共用指纹固定校验, relay 侧仅留 ureq agent 装配。
- 2026-10-26 (T07 边界前移): 复合传输自 T08 提前至 T07 交付 (超时回落与强制中继的验收都要求流量真正走中继); T08 保留切换恢复精化 (周期重试直连)、CGNAT 冲突检测、保活与 MTU 精推。
- 2026-10-26 (T08 探测帧升级): 探测帧由单类扩展为 PING/PONG (帧加 1B 帧类)——原 "打洞中或新令牌才回执" 的防回声条件使 Direct 态无法安全发起周期保活探测 (会形成回执风暴); PONG 回显令牌且不再触发应答, 消除回声环, 失活检测 (FR-6) 才有观测来源。
- 2026-10-26 (T08 重试保活语义): Punching 携带 fallback 既有端点 (回落后的重打洞、周期重试均不中断在用中继流量); 路径节奏 (ping 15s / dead_after 60s / retry 30s / punch_window 5s) 收敛为 PathTimings 进 NodeOptions, 生产默认与 e2e 小值同构驱动。
- 2026-10-26 (T08 MTU 裁决): DEFAULT_MTU 取 1440——两路径约束不对称, 直连 (WG over UDP) 有硬上限 (1500-60=1440), 中继 (WG over TLS over TCP) 叠加头但 TCP 流式分段吸收无内层硬上限, 故取直连上限; e2e "ping -s 1400" (IP 1428) 是下限约束, 1280 过保守、1380 不满足。
- 2026-10-26 (T08 冲突检测边界): netconf 只比对接口地址 (含掩码网络, 双向区间相交); 系统路由表全量解析跨平台脆弱、误拒比漏报代价高, 列为 e2e 反馈项。枚举失败按无冲突放行 (防断网保护不阻断组网)。
- 2026-10-26 (T08 实施期修复): 其一, ForceRelay 下对端探测帧会把引擎端点拉回真实地址破坏 FR-10——现完全忽略探测 (不学端点不应答); 其二, 中继通道原仅 5s 对齐节拍建连 (首个节拍早于 State 到达时列表为空), 现列表首达即建, 失活判定与短打洞窗口不再落空。
- 2026-10-26 (T08 依赖): 新增 if-addrs 0.15 (节点接口枚举, 三平台 getifaddrs / GetAdaptersAddresses 封装, 供 netconf)。
- 2026-10-26 (T09 结构迁移): pinning_agent (ureq 指纹 agent) 自 relay::client_tls 移驻 kunxtier-core::tls——节点注册客户端需同源装配, 而 node 不得反向依赖 relay (依赖方向 cli → relay / node → core); relay 的 ureq 随之降为 dev-dep。
- 2026-10-26 (T09 IPC 裁决): 本地 IPC = state 目录内 UDS + JSON 行, 单连接单请求, 目录 0700 即信任边界 (本机当前用户); 残留 socket 以探活区分活 daemon 与死文件。Windows named pipe 归 T13, 不预写无法编译验证的 cfg(windows) 代码, 非 unix 平台 bind/connect 返回 Unsupported 占位。
- 2026-10-26 (T09 up 语义裁决): daemon 常驻, up 经 IPC 由 daemon 完成注册 (WG 身份不离开 daemon 进程); 令牌为空 = 以既有 state 直接重连 (排障恢复入口); 同名重注册 (FR-17) 沿用身份与端口换发凭证, name / core / 指纹与 state 不一致即拒绝; daemon 启动有 state 即自动上线 (FR-9 前提), 意外退出指数退避重连 (起步可配 30s 封顶, 存活满 60s 复位), 网段冲突等致命态封停。
- 2026-10-26 (T09 state 形态): node.json (0600) 集中承载全部节点敏感量 (凭证 / WG 私钥 / vip / core / 指纹 / 监听端口), 原子写 + 版本字段拒读高版本; WG 监听端口缺省自动选闲并持久化 (打洞候选稳定性)。节点配置文件本切片不引入 (state 已承载 core/指纹), T14 服务化按需再评估。
- 2026-10-26 (T09 运行时接口): runtime 增 done 退出通知 (NodeHandle::done watch), daemon 会话监督以此为观测源; IP 层以 TunFactory 特质注入 (生产 TunAdapter / 测试 gotatun 通道), 通道实现的 UDP 半边由 Keep 句柄随会话持有防提前关闭。
- 2026-10-26 (T09 实施期修复): serve 退出必须连带拆除 IPC accept 任务与 socket 文件——残留 listener 既挡新 daemon 的探活判定, 又吞掉 IPC 应答 (连接被接受但无主应答)。
- 2026-10-26 (T09 测试形态): axum-server Handle 管不到已升级的 WS 连接 (升级后流归 axum 内部任务持有), 核心崩溃模拟以独立子 runtime + shutdown_background 丢弃任务组实现; 核心原端口复活复用既有证书与库 (init 拒绝重复初始化)。
- 2026-10-26 (T09 依赖): tokio 补 signal feature (daemon ctrl-c); workspace 增 kunxtier-node path 依赖 (cli 装配节点子命令)。
- 已知妥协 (T09): status --verbose 排障视图归 T15; Windows IPC 为 Unsupported 占位归 T13。
- 2026-10-26 (T15 日志装配): log_level 只进 FileConfig 文件层 (进程级关注点), 不进 RelayConfig 运行时类型; 级别优先级 CLI flag > 配置文件 > RUST_LOG > 默认 info, RUST_LOG 透传 EnvFilter 模块过滤语义; daemon 默认 stderr, --log-file 启用后双写 + 10MB 单份 .old 自实现轮转 (不引入轮转库); subscriber 初始化自 main 头部移至解析后装配位, relay 配置文件预读 (log_spec) 与正式加载各一次, 双读无害。
- 2026-10-26 (T15 事件留痕): EventRing 容量 256, 类型更名 EventRecord (规划名 NodeEvent 与 signal::NodeEvent 撞名); daemon 持环跨会话复用, 事件点与既有 tracing 埋点同位双写 (日志字段结构不变); IPC 增 StatusVerbose, 回传走 snapshot_bounded (56KB 预算) 防 64KB 单行帧越限; 对端失败原因与中继选择过程从环推导, 不另存状态源。已知妥协: 环进程内易失, 重启清零, 历史回看依赖日志文件。
- 2026-10-26 (T15 relay 侧): 注册被拒 warn 留痕补 FR-4 缺口; NFR-4 脱敏全量审计通过 (无令牌 / 凭证 / 私钥入日志)。
- 2026-10-26 (T11 资产裁决): 控制台静态资产随消费方走 (relay crate 内 assets/console/), include_str 两跳; 放弃越出 workspace 五级相对路径直取 web/ dist——跨语言交接显式发生在构建链 (T12 的 cargo-make 前端任务将产物输出至 assets/console/), 不发生在源码路径。嵌入是编译期行为, 部署形态单二进制自包含, 与 web/ 的相对位置无运行时关系。
- 2026-10-26 (T11 会话): 内存会话表 (kxt_ses_ 前缀, TTL 12h, 惰性过期, 多会话并存, 登出幂等), 重启即失效; require_admin 双轨 (会话令牌优先 / admin 凭证兼容), 登录失败沿用 admin 限速。
- 2026-10-26 (T11 路径上报): SignalClientMsg::PathReport 携带逐对端路径矩阵, relayed 条目以隧道实际拨号地址标识中继 (非中继号, 免列表序漂移误归因); 节点侧转换事件置脏经 probe 节拍冲刷 + 60s 兜底, try_send 满即丢弃待下轮补报; 核心聚合每节点每中继只计一次, 内置条目计数取转发注册表实际连接数 (不经上报)。Hello 增 version——经信令上报而非注册落库, 免 schema 迁移且节点升级重连后实时刷新 (与规划的字面偏差, 已裁决)。
- 2026-10-26 (T11 console 子命令): 本地明文 HTTP 托管嵌入资源 + admin API 反代 (ureq 指纹固定, spawn_blocking), 浏览器仅连本地, 无 CORS 面; 仅反代 REST, 信令 / 转发 WS 不经本地。ureq 自 relay dev-dep 迁回正式依赖 (本地反代直接使用其类型)。
- 已知妥协 (T11): server.rs handler 集中约 530 行, 拆分信号未达阈值; 旧核心遇 PathReport 回消息格式非法、客户端忽略, 版本混跑降级可接受。
- 2026-10-27 (T12 前端栈): React 19 + TypeScript + Vite + Tailwind CSS v4 + shadcn 风格基件 (Radix select / Slot + cva, 基件随仓库手写维护), oxfmt / oxlint 管格式与静态检查, pnpm 管依赖; 数据层 TanStack Query (3s 轮询, 页签隐藏自动暂停), 路由 react-router; 会话令牌与网络组选择纯内存态 (FR-7), 401 统一事件跳登录。
- 2026-10-27 (T12 嵌入机制): console 由单文件 include_str 升级为 include_dir 文件表 (路径命中 + 扩展名 MIME + 缓存策略: assets/ 前缀哈希产物 immutable 1y, 入口页 no-cache), 前端分包与命名策略不再反向约束 Rust 侧。两个构建链事实: cargo 不跟踪 include_dir 读入的文件, 资产变更不触发重编译——console-build 任务末尾 touch console.rs 强制重嵌入; vite outDir 相对前端项目根解析, 取 ../../rust/... 直出 assets/console (emptyOutDir)。
- 2026-10-27 (T12 任务归属): 开发热更命令归宿仓库根 Makefile (make dev / dev-relay / dev-console, 用户裁决, 聚合 rust cargo-make 与 web pnpm); rust/kunxtier/Makefile.toml 只保留构建链任务 (console-build / console-check), 不进缺省流程, Rust 侧开发不依赖 Node 工具链。占位 index.html 随 git 保留, 产物文件 gitignore, fresh clone 无 Node 仍可 cargo build 出占位页。
- 已知妥协 (T12): admin API 无用户列表端点, 令牌页以文本输入属主 + 就地创建/禁用用户承载 (用户量个人规模, 列表端点后续有需要再补); TypeScript 已随本机升级 7.x (baseUrl 移除), tsconfig 按相对 paths 适配; 浏览器真实交互验证待 e2e (T10) 与人工走查, 本切片验证到 HTTP 行为层。
- 2026-10-27 (首启自动初始化): relay 运行模式启动前置 ensure_initialized——数据目录未初始化就地进行 init, 免记忆前置命令 (用户反馈 "别人用工具时不会执行 --init"); admin 凭证落 state 目录 admin-credential (0600), 与 TLS 私钥同信任域, 服务化场景输出被 journal 吞掉后可找回; `relay --init` 保留为显式入口, backup 不自动初始化。
- 2026-10-27 (占位页防误提交修正): 原设计 "git 跟踪 assets/console/index.html + console-build 原地覆盖" 使每次构建后出现已修改的被跟踪文件, 一把 git add 即误提交构建产物 (fresh clone 白屏, 预检两次捕获同一问题)——占位页改为 assets/console/placeholder.html (git 跟踪, vite emptyOutDir 关闭不触碰), 目录产物整体 gitignore 无 negation, 陈旧哈希清理移入 console-build 脚本; console.rs 入口回落链 index.html -> placeholder.html, 单测对两种目录形态均成立。
- 流程教训 (T04 带病提交): 提交前必须跑全量 cargo make, 只跑单模块测试曾放过两个缺陷测试 (本次闸门捕获修复)。
- 环境备注: 2026-10-21 已修正本机 cargo 全局配置 (`CARGO_HOME=/Volumes/Base/LangEnv/rust/.cargo/config.toml`), rsproxy 源由停服的 USTC git 索引改为 `sparse+https://rsproxy.cn/index/`; 缓存按源主机名隔离, 新源首次拉取已重建完成。
