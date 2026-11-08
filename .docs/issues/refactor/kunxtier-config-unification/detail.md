# relay 零参数与持久配置统一 env 最高及 config 感知

## 背景

2026-11-06 产品决策 (单机验证体验复盘): CLI 样板过重 (`--server`/`--fingerprint`/`--state-dir`/relay 全套 flag 每条命令手抄)。决策三条:

1. relay 子命令零配置类 flag, 配置只来自 env 与配置文件
2. 持久运行配置优先级统一 ENV > CONFIG > DEFAULT (反转现行 CLI > ENV > CONFIG); CLI 仅承载一次性动作参数 (up 的 --token/--name, backup 的路径)
3. 新增 `kunxtier config` 命令: 感知本机配置文件位置 + 打印生效配置

卫星模式由 `KUNXTIER_RELAY_TOKEN` 存在性决定 (卫星流程本就 env-only 依赖该令牌), 不设独立开关; `--init` 移除, 裸命令首启自动初始化 (`ensure_initialized` 已内建)。

需求已同步: `feat-kunxtier-cli.md` FR-7 重写, FR-12 新增, 命令总览/全局配置/边界节更新; `feat-kunxtier-access.md` 卫星流程改为 env 形态。

## 目标

- `kunxtier relay` 裸命令即可起核心; `KUNXTIER_RELAY_TOKEN=... kunxtier relay` 即起卫星
- daemon/up/status/down 无 --state-dir, 统一 `KUNXTIER_STATE_DIR` (env 已实现未接管)
- `kunxtier config` 输出配置文件位置与生效值
- 日常节点命令免 sudo (socket 权限按需求落地)

## 范围

| 项 | 位置 | 动作 |
| --- | --- | --- |
| relay flag 拆除 | `kunxtier-cli/src/cli/relay.rs` | 删 `--listen`/`--data-dir`/`--advertise`/`--core`/`--fingerprint`/`--satellite`/`--init`/`--config` 全部配置类 flag; 保留 `backup <path>` 子命令 |
| relay 配置文件 | `kunxtier-core/src/config.rs` | 新增 relay 配置解析: 固定路径 `/etc/kunxtier/relay.toml` 自动发现, 字段 listen/data-dir/advertise/core/fingerprint/stun; env (`KUNXTIER_LISTEN`/`KUNXTIER_DATA_DIR`/`KUNXTIER_ADVERTISE`/`KUNXTIER_CORE`/`KUNXTIER_FINGERPRINT` 已有) 优先于文件 |
| 卫星模式判定 | `relay.rs` 装配 | `KUNXTIER_RELAY_TOKEN` 存在即卫星, 缺失即核心; 冲突校验 (核心模式出现该 env 应报错还是忽略, 实现期定并落注释) |
| 优先级反转 | `config.rs` `pick`/`resolve` | `cli_v.or(env).or(file)` 改 `env.or(file).or(default)`; `--state-dir` flag 从 daemon/up/down/status/install 摘除, `KUNXTIER_STATE_DIR` 接管; FR-7 相关单测改写 |
| config 子命令 | `kunxtier-cli/src/cli/config.rs` 新建 | 输出: 各域配置文件路径 (存在与否) + 生效配置终值 (env > 文件 > 默认合并结果); 敏感字段 (令牌/凭证/私钥路径内容) 脱敏为存在性提示 |
| IPC 免 sudo | `kunxtier-node/src` state/socket 权限 | 按 feat-kunxtier-cli 承诺落地: 服务形态目录 0755 / 身份文件 0600 / socket 0666, 已登录用户可执行 up/down/status; 现实现 0700 目录偏离该承诺 |
| 文档同步 | CLAUDE.md, e2e S1/S4/S5 | 命令面全部换 env 形态; `make dev` 流程核对 |
| 架构文档 | `impl-kunxtier-runtime-layout.md`, `arch-kunxtier-overview.md`, `arch-structure.md` | impl 的 relay.toml 经 `--config` 指定与 `--log-file`/`--state-dir` 描述、overview 的 "CLI > ENV > CONFIG" 分层与 `--state-dir` 取舍条目、structure 的 `--satellite` 树注释, 均按目标形态改写; 历史演进记录只追加 |

## 非目标

- console 子命令的 `--relay`/`--fingerprint` 暂不动 (临时进程的连接目标参数, 是否 env 化实现期评估, 不在本 issue 强制)
- 不做配置热加载 / SIGHUP reload
- 不引入配置校验框架, 维持现有非法值拒绝行为

## 方案讨论

- relay.toml 与节点全局 config.toml 并存于 /etc/kunxtier/ 下, 两文件各管一个角色, 不合并 (需求边界已固化)
- 优先级反转后, up 的一次性 `--token`/`--name` 是仅存的 CLI 配置面; 若未来需要临时覆盖 listen 等持久项, 用 env, 不回退 flag
- IPC socket 0666 的信任边界: 本机已登录用户可操作本机 daemon (需求原文), 私钥文件 0600 不出信任域; 与现行 0700 目录策略二选一, 按需求口径走 0755+0666

## 风险与边界

- 删 --state-dir 是破坏性变更: e2e/开发流程全部改 env, 迁移窗口内文档与代码必须同批落地
- 优先级反转影响 config.rs 既有测试 (cli 最高用例), 需同步改写而非删除覆盖
- relay.toml 固定路径意味着本地开发无法多实例同机 (除非 env 覆盖 data-dir/listen), 与现状 e2e 单核心一致, 可接受

## 验收口径

- `kunxtier relay` / `KUNXTIER_RELAY_TOKEN=x kunxtier relay` 两形态可用; `kunxtier relay --listen ...` 报错 "不支持的参数"
- `kunxtier config` 输出 relay/node 两域文件位置与合并终值, 令牌类字段只显示 "已设置/未设置"
- daemon 全生命周期命令无 --state-dir; `KUNXTIER_STATE_DIR` 驱动; FR-7 单测按 env > file > default 通过
- 非 root 用户可执行 status/down (socket 0666 生效), 私钥仍 0600
