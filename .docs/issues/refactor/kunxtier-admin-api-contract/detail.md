# relay 管理 API 契约合规整改 (basic-api-contract)

## 背景

2026-10-31 以 `.claude/skills/basic-api-contract` (SKILL.md + 4 个 references) 为规范, 对照 relay 管理 API 与信令 WS 全量审计, 发现 17 条违规 (高 3 / 中 9 / 低 5)。属 ".claude 规范对照代码" 合规审计的 API 域, 高危项已逐条抽查核实代码证据。

合规面 (无需处理): REST 面统一错误出口与机器可读 code; 名字/CIDR/枚举白名单校验; 信令 WS 有界性 (256KiB 消息上限 / 候选数上限 / 出站队列 64); 新字段一律 `#[serde(default)]` 的只增演进; 存储迁移版本化只增列; 认证防枚举与失败限速; 内部错误不外泄。

## 目标

- 错误模型对齐规范: request_id/details、状态码语义、429 Retry-After、WS 错误 code 化
- 列表接口有界: 分页参数 + 默认值 + 上限, 列表瘦身
- 关键写幂等与边界校验补齐 (ttl_secs 溢出路径是唯一可 panic 的输入)
- 版本策略与契约事实源显式化

## 范围

### 高

| 条目 | 位置 | 问题 |
| --- | --- | --- |
| 错误体无 request_id/details | `kunxtier-relay/src/server.rs:132-140`, `kunxtier-core/src/protocol.rs:144-148` | `ApiErrorBody` 只有 error+message, 无法贯穿日志定位, 字段级校验明细无处安放 |
| 404/409 压成 400 | `server.rs:142-156`, `store.rs:243-247, 307-311, 525-530` | "不存在"与"已存在"全部走 `Error::Usage` -> 400, 状态码语义错位 |
| 列表完全无界 | `server.rs:339-347, 447-499`, `store.rs:266, 320-323, 671-682` | networks/users/nodes/relays 四个列表无分页参数、无 LIMIT、无上限 |

### 中

| 条目 | 位置 | 问题 |
| --- | --- | --- |
| 429 缺 Retry-After | `server.rs:123-129`, `signaling.rs:266`, `forward.rs:153,166`, `satellite.rs:161` | 限流拒绝无退避提示 |
| WS 限流拒绝裸文本 | `signaling.rs:266`, `forward.rs:153,166`, `satellite.rs:161` | 与 REST 面错误形态不一致 |
| WS 协议错误无 code | `protocol.rs:264-266, 293-301` | AuthErr/Error/SignalErr/Closed 全是自由文本, 客户端只能字符串匹配 |
| 列表携带重嵌套 | `protocol.rs:100-117` | NodeInfo 挂 candidates + 最多 256 条 paths, 进无界列表 |
| 无 URL 版本前缀 | `server.rs:170-191` | 无 `/v1`, 破坏性变更无退守位 |
| 契约双份手写投影 | `protocol.rs` 与 `web/kunxtier-console/src/api/types.ts` | 无 schema/CI 校验, 漂移靠人肉 |
| 关键写无幂等键 | `server.rs:413-430` | token 签发重试即重复落库 |
| ttl_secs 无范围钳制 | `server.rs:421`, `store.rs:484` | `now() + ttl_secs as i64`: 大值 debug 构建溢出 panic (持 store 锁), 跨 2^63 值转负成立即过期 |
| 时间格式 unix 秒 | `protocol.rs:86, 96, 106, 130` | 三方一致且注释声明, 属有意契约, 但与规范 RFC3339 不符, 需显式决策 |

### 低

| 条目 | 位置 | 问题 |
| --- | --- | --- |
| 错误体字段名 error 非 code | `protocol.rs:145` | 机器可读但与规范模板命名不一致 |
| console 反代错误裸文本且泄内部实现 | `kunxtier-relay/src/console.rs:806-814` | 非 JSON, 直接吐 ureq 错误串 |
| owner 过滤无白名单语义 | `server.rs:465-467` | 任意字符串精确匹配, 契约未声明 |
| 未知字段策略隐式 | 各请求结构体 | serde 默认忽略, 巧合一致而非显式声明 |
| 校验错误逐条串行报 | `store.rs` | 无 details[] 聚合, 一次只报第一个错 |

## 非目标

- 不重写协议消息结构 (只增字段演进保持)
- 不引入 OpenAPI 全家桶, 契约事实源方案先讨论再动
- unix 秒 vs RFC3339 属契约决策, 若维持 unix 秒则记录豁免理由

## 方案讨论

- 修 V2 需要把 `Error::Usage` 拆出 `NotFound` / `Conflict` 变体, 影响全部 handler 分支, 建议与 V1 (错误体增强) 同批做
- V8 分页方案二选一: offset 分页 (简单, 个人规模够用) 或 cursor (节点列表有活跃变更); 倾向 offset + 上限 200
- `/v1` 前缀越晚引入成本越高, 但会破坏现有 console 前端与 CLI, 需与前端 issue (REF-kunxtier-console-frontend-rules) 协同
- 幂等键: token 签发是本地 CLI/同源调用, 失败面小, 可先只做服务端去重 (同 kind+ttl 短窗口), 不必上 Idempotency-Key 全套

## 风险与边界

- 错误体加字段 / 状态码纠正对 CLI (`kunxtier admin`) 与 console 前端是行为变更, 需同步适配
- 分页改造改变响应形态 (信封 or header), 前端 types.ts 与页面需同步

## 验收口径

- 错误体含 request_id; 不存在/冲突/参数错误分别映射 404/409/400
- 列表接口带分页参数与上限, store 侧查询有 LIMIT
- ttl_secs 越界在边界被拒 (400), 不可能进入 `now() + ttl` 运算
- 429 带 Retry-After; WS 错误消息有稳定 code 字段
- 版本前缀与契约事实源决策落档 (做或不做都写明理由)
