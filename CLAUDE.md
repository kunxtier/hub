# hub monorepo 工作说明

按语言组织的多项目仓库; 项目自包含, 成熟后整体拆仓。结构约定与拆仓规则见 `.docs/architectures/arch-structure.md`。

## 当前项目

- rust/kunxtier: 轻量 P2P 虚拟组网 (ZeroTier / EasyTier 平替)。cargo workspace:
  - kunxtier-core: 协议消息 (信令 / 转发帧 / 卫星注册) / 凭证与令牌 / 地址分配 / 分层配置 / TLS 指纹固定
  - kunxtier-relay: 核心中继 (登记 / 信令 WS / 管理 API / 转发双模式 / 卫星注册分发 / 自签 TLS / 限速 / 备份)
  - kunxtier-node: 节点数据面 (WG 用户态引擎 gotatun + tun-rs TUN 适配 + 复合传输 / STUN / 打洞 / 路径策略 / 信令客户端与运行时; 进度见 issue FEAT-kunxtier-core-runtime)
  - kunxtier-cli: 单一入口二进制 `kunxtier`
  - web/kunxtier-console (控制台前端) 按阶段推进再落地
- 规划文档: `.docs/` (requirements / architectures / testing / issues)

## 常用命令

```sh
cd rust/kunxtier
cargo build
cargo test
# 本地试跑 (避免 443 权限问题, 指定高位端口)
cargo run -p kunxtier-cli --bin kunxtier -- relay --init --listen 127.0.0.1:9443 --data-dir ./kunxtier-state
cargo run -p kunxtier-cli --bin kunxtier -- relay --listen 127.0.0.1:9443 --data-dir ./kunxtier-state
# 卫星中继 (纯转发; relay 令牌经 admin token issue --type relay 签发)
KUNXTIER_RELAY_TOKEN=<relay-token> kunxtier relay --satellite --core <核心地址> --fingerprint <核心指纹> --advertise <对外地址>

# 任务编排 (cargo-make, rust/kunxtier/Makefile.toml, 按领域划分)
cargo make --list-all-steps   # 查看全部任务
cargo make                    # 缺省: 格式校验 + clippy + 测试
```

仓库根 `make hooks` 一次装载 git hooks (pre-commit: 敏感文件与格式检查; commit-msg: conventional 前缀与 emoji 红线), 卸载 `make hooks-uninstall`。

- 运行态目录 `kunxtier-state/` 含 TLS 私钥与 SQLite, 已进 .gitignore, 严禁提交或外传
- admin 凭证只走 env `KUNXTIER_ADMIN`, 证书指纹走 `KUNXTIER_FINGERPRINT` 或 `--fingerprint`

## 流程

- 规划: /ik-plan; 阶段推进: /ik-advance; 阶段内跨会话: /ik-handoff; 提交: /ik-commit
- issue 与文档 CRUD: docsctl (`node .claude/skills/docs-repo/scripts/docsctl.mjs`)
- 全部约束见 .claude/rules (安全红线 / 执行边界 / 输出约束: 禁 emoji, 文档正文中文)
