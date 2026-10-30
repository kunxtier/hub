# 修复 .claude rules 与 skills 审计发现的失效引用与残留

## 背景

2026-10-31 对 `.claude/` 全量审计: 9 个 rules (根 5 + lang 4)、6 个 commands (ik-*)、17 个 skills (basic-* 15 + iks-deep-audit) 及 settings.local.json、CLAUDE.md 中指向 `.claude` 的引用。

审计维度: emoji 红线、frontmatter 规范、引用有效性、内容矛盾/重复、语言规范、他仓残留、权限配置一致性。

合规面 (无需处理):

- emoji 零命中 (全量 Unicode 区段扫描, 无存量)
- 全部 SKILL.md 正文与 references 正文均为中文, 符合语言规范
- commands 引用的 skill 全部存在, 无断链
- basic-sql-runtime 与 basic-db-safety 的数据库操作边界划分清晰, 无矛盾

## 目标

- 消除全部失效引用, 使按图索骥 (SKILL.md 索引表 / CLAUDE.md 指引 / settings 放行) 均能命中真实目标
- 清理从其他仓库复制未适配本仓的残留内容
- 收敛重复展开面, 降低双处漂移风险
- 使 settings.local.json 放行与 execution-policy.md 禁止方向一致

## 范围

### 高危: 失效引用 (按指引操作必然 miss)

| 位置 | 问题 | 修复 |
| --- | --- | --- |
| `CLAUDE.md:47` | docsctl 路径写成 `.claude/skills/docs-repo/scripts/docsctl.mjs`, 目录不存在 (实际在 `basic-docs-mgmt/`) | 改正路径 |
| `settings.local.json:4-6` | 3 条 allow 同样指向 `docs-repo` 死路径, 永不命中, 属旧结构残留 | 删除或改指实际脚本 |
| `basic-api-contract/SKILL.md:34` | 索引表拼写 `idempotancy-and-validation.md`, 实际文件为 `idempotency-and-validation.md` | 改正拼写 |
| `basic-coding-std/SKILL.md:93` | 引用不存在的 `rules/frameworks/` 目录 | 删除或改指实际结构 |

### 中危: 配置矛盾与他仓残留

| 位置 | 问题 | 修复 |
| --- | --- | --- |
| `settings.local.json:7,10,11,14` | `cargo make *` / `cargo test *` / `cargo build *` / `python3 *` 宽放行, 与 `execution-policy.md` "构建/测试需明确许可" 方向相反; `python3 *` 属任意代码执行放行 | 收敛为按需精确放行, 与 rules 口径对齐 |
| `rules/lang/rust.md:50-62` | 整节 TUI 规则 (raw mode / alternate screen / redraw / tick) 为他仓残留, 本仓 Rust 项目 (relay/node/cli) 无 TUI 场景; 架构约束段亦偏 Elm/TUI 风味 | 删除 TUI 节, 保留通用条款, 按本仓网络服务场景重写架构约束 |
| `iks-deep-audit/` | 参数集含 `--android` / `--go`, 本仓无此技术栈; `references/android.md` `references/go.md` 为他仓残留; 缺 React 专项 reference; frontmatter 用 `disable-model-invocation` 与 basic-* 的 `user-invocable: false` 风格不一 | 参数集收敛为 `--rust` / `--typescript`, 删除两个残留 reference, 评估补 react.md, 统一 frontmatter 字段 |
| `basic-docs-mgmt/SKILL.md:96-98` | meta.yaml 字段表 `id` 行管道符未转义, 3 列表被撑成 6 列, "必填"列错位丢失 | 按 `:102` 同文件既有的 `\|` 转义风格修复 |
| `basic-docs-mgmt/references/architectures.md:18-65` | 代码围栏开闭错位: 中段内容被渲染为真实标题, "## 编写约束"整节被吞进代码块 | 重排围栏边界 |
| `basic-sql-runtime/references/query-patterns.md:63-70` | "读缓存与读写分离"一节重复展开 basic-cache-runtime 领地 (singleflight/短 TTL/stale-while-revalidate 几乎同构), 双处漂移风险 | 收敛为一句指向 basic-cache-runtime, 仅保留 SQL 侧特有内容 (read-your-writes / 主库路由) |
| `basic-docs-mgmt/references/issues.md` | 孤儿文件: SKILL.md 从未引用, 内容与正文高度重叠 | SKILL.md 挂上引用或删除冗余 |

### 低危: 文字与事实性修正

| 位置 | 问题 |
| --- | --- |
| `basic-docs-mgmt/SKILL.md:3` | description `working with.docs docs` 单词粘连 |
| `commands/ik-doc-migrate.md:2,56` | description `into.docs.backup` 粘连; 示例 `--root.docs` flag 与值粘连, 照抄即错 |
| `rules/lang/tailwindcss.md:14` | `cn` 包定性为 "shadcn-ui 官方维护, 零依赖, API 完全兼容" 事实性存疑, 且 `:93` 以此为据禁止 clsx/tailwind-merge; 需核实来源后修正表述 |
| `settings.local.json:9,13` | 旧仓路径 Read 条目 (`.../kunxtier/crates/...` 拆仓前路径) 与含本地指纹的调试命令残留 |
| 15 个 skill frontmatter | description 全英文而正文规范为中文; 疑似有意为之 (英文利于触发路由), 需确认口径后统一 |

## 非目标

- 不重构 docsctl 脚本本身
- 不调整 basic-* skills 的规则语义 (redis 禁令 / outbox / 超时配置化等已一致的口径不动)
- 不清理 `.docs` 存量文档

## 方案讨论

- 修复均为定点替换与删除, 无方案分叉; 唯一取舍点:
  - rust.md 架构约束段: 整段删除会误伤"副作用与纯状态推进分离"等通用条款, 应逐条甄别而非整段移除
  - skill description 语言口径: 建议维持英文 (触发路由优先), 在规范中显式豁免 frontmatter, 而非全部汉化
  - settings 宽放行收敛后, 日常 cargo build/test 会重新回到逐次询问, 属预期行为

## 风险与边界

- settings.local.json 属个人本地配置, 修正前需本人确认保留哪些日常放行
- 删除 iks-deep-audit 的 android/go reference 前确认本仓确无关联计划 (如后续 Go 卫星端)
- 修改 CLAUDE.md 涉及项目级说明文件, 变更需同步核对仓库根 make hooks 与 Makefile 是否也引用旧路径

## 验收口径

- 全文检索 `.claude/` 与 CLAUDE.md, 不再出现 `docs-repo`、`idempotancy`、`rules/frameworks` 字样
- basic-docs-mgmt SKILL.md 字段表渲染为 3 列; architectures.md 各节标题层级正确
- `iks-deep-audit` 帮助文本与本仓技术栈一致
- settings.local.json 无永不命中的 allow 条目, 无与 execution-policy 矛盾的宽放行

## 修复记录

2026-10-31 六项任务全部完成:

- T01 失效引用: CLAUDE.md docsctl 路径改正为 `basic-docs-mgmt/`; CLAUDE.md issue 指针补哈希后缀 (`FEAT-kunxtier-core-runtime-a580d1`, 审计时新发现, 无哈希时 `docsctl show` 无法解析); settings 三条 `docs-repo` 死条目删除; api-contract 拼写改正 (`idempotency`); coding-std 移除不存在的 `rules/frameworks/`
- T02 settings 收敛: 经确认删除 `python3 *` 与 `node *` 宽放行 (cargo build/test/make 经确认保留为日常高频操作), 同时清理旧仓死路径 Read 条目与含本地指纹的一次性调试命令; execution-policy.md 未动
- T03 rust.md: 删除整节 TUI; 修正 6 处散落 TUI 风味表述 (driver / redraw / terminal / App / UI state / terminal 状态破坏), 通用条款保留并按网络服务场景改写 (传输引擎 / 外部输入解析 / 会话与协议状态分层 / 资源与任务泄漏)
- T04 iks-deep-audit: 参数集收敛为 `--rust` / `--typescript`; 删除 `references/android.md` 与 `references/go.md`; 新增 `references/react.md` (配套 typescript.md 覆盖 `.tsx`, 对齐 core.md 风险模型); `references/rust.md` 同步清理 TUI 检查项并补服务化检查 (后台任务生命周期收口 / 优雅停机); frontmatter 保持 `disable-model-invocation`, 手动触发属有意设计, 不与 basic-* 的 `user-invocable` 统一
- T05 docs-mgmt: meta.yaml 字段表管道符转义修复; architectures.md 围栏重排 (模板围栏 18 行开 52 行闭, 已脚本校验全部闭合); `references/issues.md` 在 SKILL.md 挂引用, 保留原因是含独有搜索友好性细则
- T06 低危: `with.docs` / `into.docs.backup` / `--root.docs` 粘连修正; sql-runtime 读缓存节收敛为指向 basic-cache-runtime, 仅保留 SQL 侧特有内容; tailwindcss `cn` 陈述经 npm registry 核实属实 (维护者 shadcn, 官方描述即 drop-in replacement for clsx + tailwind-merge), 销项不改; skill description 英文口径经确认为显式豁免, 已写入 `rules/safety-and-output.md` 输出约束
