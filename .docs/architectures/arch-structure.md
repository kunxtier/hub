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
