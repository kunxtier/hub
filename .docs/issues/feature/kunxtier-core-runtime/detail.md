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
