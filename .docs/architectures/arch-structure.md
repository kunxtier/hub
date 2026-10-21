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

## 当前实际结构 (2026-10-20, ik-advance --init)

```text
hub/
├── .docs/                        # 规划文档全套已落地
├── rust/kunxtier/                # 已落地
│   ├── Cargo.toml                # workspace 根 + 统一依赖版本
│   ├── Makefile.toml             # cargo-make 任务集 (构建 / 质量 / 测试 / 运行 / 发布)
│   └── crates/
│       ├── kunxtier-core/        # 协议 / 凭证令牌 / 地址 / 分层配置
│       ├── kunxtier-relay/       # 核心中继: store + server + init + cert + ratelimit + client_tls + error
│       └── kunxtier-cli/         # 二进制 kunxtier: relay / admin 子命令
├── CLAUDE.md                     # 已创建
└── web/                          # 未建 (控制台前端阶段落地)
```

## 演化决策记录

- 2026-10-20 (init): kunxtier-node crate 未建——数据面阶段再落地, 不预建空壳; web/ 同理。已交付切片: 控制面登记闭环 (init / HTTPS 管理 API / 节点注册 / 限速 / 备份)。
- 已知妥协: store 用 std::sync::Mutex 包同步 SQLite, handler 内短临界区; 信令阶段的并发压力实际显现时再评估异步化。
- 已知妥协: 限速器过期表项不主动清理, 源数量自然有界 (单机个人规模)。
- 环境备注: 2026-10-21 已修正本机 cargo 全局配置 (`CARGO_HOME=/Volumes/Base/LangEnv/rust/.cargo/config.toml`), rsproxy 源由停服的 USTC git 索引改为 `sparse+https://rsproxy.cn/index/`; 缓存按源主机名隔离, 新源首次拉取已重建完成。
