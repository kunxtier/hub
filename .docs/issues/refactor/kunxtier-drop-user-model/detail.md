# 移除用户属主模型回归令牌直连接入

## 背景

2026-11-06 产品决策 (单机验证体验复盘): user/属主概念使个人自用场景的接入仪式过重 (先 user create 再 token issue --user), 与 kunxtier "个人与小团队自建组网" 定位不符。决策: 彻底移除用户模型, 令牌是唯一入网凭证, 治理由 token revoke + node remove 承担。

需求已同步: `feat-kunxtier-access.md` 重写为令牌直连模型 (FR-1 直接按网络签发, FR-3 声明无用户实体); `feat-kunxtier-cli.md` 命令面移除 user 子命令组; `overview.md` 索引行更新。

## 目标

- 全链路不存在用户实体: store / 协议 / API / CLI / console 五层同步拆除
- `token issue --network <net>` 直接签发, 回归 e2e 原始直觉用法
- 治理路径收口为 token revoke + node remove

## 范围

| 层 | 位置 | 动作 |
| --- | --- | --- |
| store | `kunxtier-relay/src/store.rs` | 拆 `users` 表与 user_id 关联 (tokens.user_id / nodes 属主列), `issue_token` 去用户绑定校验, `consume_node_token` 去 disabled 检查, 删 `disable_user`/`enable_user`/`user_row`/`list_users`/`node_names_of_user`/`delete_user`; schema_version +1 前向迁移 (DROP users, tokens 去 user_id 列) |
| 协议 | `kunxtier-core/src/protocol.rs` | 删 `UserInfo` 类消息与 `NodeInfo` 的属主字段 (如有), 快照测试同步 |
| server | `kunxtier-relay/src/server.rs` | 删 `/users` 端点组 (create/list/disable/enable/rename/remove), disable 的 kick 联动随端点删除 |
| CLI | `kunxtier-cli/src/cli/admin/user.rs` | 整文件删除, `AdminAction` 摘除 User 分支, token issue 去 `--user` |
| console | `web/kunxtier-console/src` | 删用户管理界面/API 方法/类型投影; `Tokens.tsx` 内用户列表与用户增删改禁表单移除 |
| 测试文档 | `.docs/testing/e2e-kunxtier-single.md` | S2 删 `user create` 行, token issue 去 `--user`; S6 删 "禁用用户" 场景行 |
| 架构文档 | `arch-kunxtier-overview.md` | 属主/用户条目 (注册流程 "记录属主"、API 扩展 "属主过滤") 按目标形态改写; `arch-structure.md` 的 T12 妥协与演进记录属历史, 不改写只追加 |
| 控制台需求 | `.docs/requirements/feat-kunxtier-console.md` | 核对并移除用户管理相关条目 |
| CLAUDE.md | 根 | 命令示例不含 user |

## 非目标

- 不保留任何用户/属主底层字段 "以备将来" (决策为彻底删除, 回归时重新设计)
- 不做令牌形态演进 (如批量签发/有效期界面), 维持现有 token 能力

## 方案讨论

- 迁移策略: schema v3 直接 DROP users 并去列, 不搬迁数据; 属主标注随迁移消失, 与决策一致。现存开发态 state 直接重建亦可接受 (记录在案)
- `NodesQuery` 的 owner 过滤参数随属主字段一并删除 (api-contract issue 中 owner 白名单校验随之失效, 该 issue 范围联动见下)

## 风险与边界

- 联动 `REF-kunxtier-console-frontend-rules`: Tokens.tsx 拆分任务的对象 (用户管理块) 将消失, 实施顺序建议本 issue 先行, 前端 issue 收窄后执行
- 联动 `REF-kunxtier-admin-api-contract` (若已收口): /users 端点删除属契约破坏性变更, 无外部消费者, 同仓锁步即可
- e2e S6 "禁用用户" 是场景矩阵行, 删除后矩阵文档 (`e2e-kunxtier-network.md` 相关行) 需同步核对

## 验收口径

- 全仓 grep 无 user/属主 残留面 (store 函数/端点/CLI/console/协议消息)
- `token issue --network default` 直接成功; e2e S2 按新命令面通过
- schema v3 迁移测试: 旧库 (v2 含 users) 打开自动迁移不 panic, 数据面 (网络/节点/令牌) 完整
- 现有测试全绿, 涉用户用例改写或删除
