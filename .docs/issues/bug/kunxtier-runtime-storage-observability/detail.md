# relay 与 node 运行时合规整改 (存储落盘/可观测/热路径)

## 背景

2026-10-31 以 `.claude/skills/basic-config-system`、`basic-observability`、`basic-db-safety`、`basic-sql-runtime`、`rules/lang/rust.md`、`basic-coding-std` 为规范对照 relay/node 代码审计。高危项已逐条抽查核实代码证据。

合规面: 配置分层 (CLI>ENV>CONFIG>DEFAULT 统一 pick)、秘密只走 env、node state 0600 原子写、参数化查询、版本化迁移、事务短小、聚合下推、凭证明文不落库 (sha256+恒时比较)、错误级别纪律、日志无秘密、文件头 100%、CLI 组织、退出收口、无 TODO 残留、常量三分法。

## 目标

- 消除存储层唯一高危: SQLite 连接无 busy_timeout/WAL
- 私钥与凭证落盘改原子写, 消除宽权限窗口
- 关键路径补 duration/outcome, 日志形态显式决策
- 转发热路径去掉逐帧多余拷贝

## 范围

### 高

| 条目 | 位置 | 问题 |
| --- | --- | --- |
| SQLite 无 busy_timeout / WAL | `kunxtier-relay/src/store.rs:86-89` | `Connection::open` 后无任何 PRAGMA; 信令/转发/HTTP/CLI 共用单连接, 写锁等待无上界, 违反 "写锁等待上界" 铁律 |

### 中

| 条目 | 位置 | 问题 |
| --- | --- | --- |
| 私钥/凭证非原子写 + 权限窗口 | `kunxtier-relay/src/init.rs:50-57, 64-70` | `std::fs::write` 后才 `set_permissions(0o600)`: 创建瞬间 umask 默认权限可见; 非原子无 fsync, 崩溃可留半截私钥。core 已有 `write_atomic` (node 侧在用) 未复用 |
| 日志非结构化 | `kunxtier-cli/src/logging.rs:74-77` | tracing fmt 默认人读格式, 无 `.json()` 选项; event 为自由中文句子非稳定键 |
| 关键路径无 duration | `runtime.rs:597,640,789,808`, `forward.rs:199,328`, `signaling.rs:299,346`, `server.rs:305,310` | 打洞/路径切换/信令/转发/注册均有 outcome 无 duration_ms |
| 无指标暴露 | 全库 | 无 metrics 依赖, 打洞成功率/转发字节/注册失败分类无趋势数据 |
| 查询无语句级超时兜底 | `store.rs` | 除 busy_timeout 外无 progress_handler 类兜底 |
| 五处 SELECT 无 LIMIT | `store.rs:263-280, 318-336, 670-696, 756-776, 779-792` | `signal_network_nodes` 在每次信令 State 推送路径整网拉取 (与 API 域分页 issue 关联) |
| 转发逐帧多余拷贝 | `kunxtier-relay/src/forward.rs:339` | `bytes.to_vec()` 对已 owned 的帧做完整 O(帧长) memcpy, 解码只借用, 可直接传 owned |
| 后台任务 expect | `kunxtier-node/src/daemon.rs:730` | spawn 信号监听任务内 `expect("shutdown 通道不应关闭")`, send 端 drop 即 panic (当前为测试代码) |

### 低

| 条目 | 位置 | 问题 |
| --- | --- | --- |
| 无 trace 上下文 | 全库 | HTTP/信令入口无 trace id, 跨进程排障靠时间戳对表 |
| 无 SQL 打点 | `store.rs` | 慢查询/错误分类不可观测 |
| 锁 unwrap 缺理由注释 | `forward.rs:74`, `signaling.rs:82`, `session.rs:38,50,63`, `satellite.rs:75`, `ratelimit.rs:57` | 同类锁 unwrap 在 server.rs/ratelimit.rs 有理由注释, 这几处裸写, 与项目自身惯例不一致 |
| env 散读 | `kunxtier-node/src/service.rs:552` | `KUNXTIER_SMOKE_BIN` 在 service 模块内读取, 未收进配置对象 (smoke 测试钩子) |
| journal 凭证留存提示 | `init.rs:20` 注释, `cli/relay.rs:143-150` | 服务化启动时 admin 凭证 eprintln 可能进 journal; 设计内一次性交付, 需评估服务化场景改文件-only 提示 |
| 心跳间隔内联硬编码 | `satellite.rs:490-492` | `Duration::from_secs(10)` 内联在 select 循环, 同文件其余策略参数已收常量区 |
| 超限多职责文件 | `store.rs` 1167 行, `runtime.rs` 1154 行, `signaling.rs` 816 行, `daemon.rs` 736 行 | 超 300 行拆分信号, store/runtime 承载多职责 |

## 非目标

- 不引入 metrics 栈全家桶 (是否暴露 prometheus 端点属架构决策)
- 不为 trace 引入分布式追踪系统, 最小做请求 id 贯穿 (与 API 域 request_id 协同)
- 不拆超限文件 (记录性, 触碰到再拆)

## 方案讨论

- D1 修复就两行 PRAGMA (`busy_timeout=5000` + `journal_mode=WAL`), 收益最大, 建议第一个做
- init.rs 改 `write_atomic` 直接复用 `kunxtier-core/src/atomic_file.rs`, 与 node 侧对齐
- 日志 JSON 化是形态决策: daemon 场景人读也有价值, 可选 env 开关 (`KUNXTIER_LOG_FORMAT=json`), 走配置分层
- duration 打点先覆盖四条关键路径: 打洞结果 / 路径切换 / 信令建连 / 转发准入

## 风险与边界

- WAL 模式改变 state 目录文件布局 (多 -wal/-shm 文件), 备份逻辑 (`store.rs` backup) 需同步核对
- 错误体/日志字段变更需与 API 域 issue 及前端 issue 协同

## 验收口径

- 连接初始化含 busy_timeout 与 WAL, 备份流程在 WAL 下仍正确
- init 落盘全部走 `write_atomic` 且权限 0600 一步到位, 无先写后改窗口
- 四条关键路径日志含 duration_ms 与 outcome
- forward.rs 转发路径无逐帧 to_vec 拷贝
- 补齐的锁 unwrap 均带理由注释
