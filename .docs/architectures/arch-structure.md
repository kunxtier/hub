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

## 当前实际结构 (2026-10-26, ik-advance T07)

```text
hub/
├── .docs/                        # 规划文档全套已落地
├── rust/kunxtier/                # 已落地
│   ├── Cargo.toml                # workspace 根 + 统一依赖版本
│   ├── Makefile.toml             # cargo-make 任务集 (构建 / 质量 / 测试 / 运行 / 发布)
│   └── crates/
│       ├── kunxtier-core/        # 协议 (信令 / 转发帧 / 卫星注册) / 凭证令牌 / 地址 / 分层配置 (含 SatelliteConfig, stun_servers)
│       │                         #   + tls (指纹固定 PinVerifier, 卫星与节点客户端共用)
│       ├── kunxtier-relay/       # 核心中继: store (schema v2 含 nodes.policy) + server + init + cert + ratelimit + client_tls (ureq 装配)
│       │                         #   + signaling (信令 WS, 候选广播 / 策略推送) + forward (转发 WS 双准入) + satellite (注册表 + 卫星进程)
│       ├── kunxtier-cli/         # 二进制 kunxtier: relay (核心 / --satellite 卫星) / admin 子命令 (含 node policy)
│       └── kunxtier-node/        # 节点数据面: engine + tun + identity
│                                 #   + transport (复合传输: UDP 分流 + 中继伪端点) + stun + punch (探测与状态机)
│                                 #   + relay_conn (转发 WS 客户端) + signal (信令客户端) + runtime (事件循环装配)
├── CLAUDE.md                     # 已创建
└── web/                          # 未建 (控制台前端阶段落地)
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
- 流程教训 (T04 带病提交): 提交前必须跑全量 cargo make, 只跑单模块测试曾放过两个缺陷测试 (本次闸门捕获修复)。
- 环境备注: 2026-10-21 已修正本机 cargo 全局配置 (`CARGO_HOME=/Volumes/Base/LangEnv/rust/.cargo/config.toml`), rsproxy 源由停服的 USTC git 索引改为 `sparse+https://rsproxy.cn/index/`; 缓存按源主机名隔离, 新源首次拉取已重建完成。
