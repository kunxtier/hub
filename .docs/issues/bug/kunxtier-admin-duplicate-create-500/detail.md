# admin 重复创建用户/网络组返回 500 内部错误

## 背景

单机管理链路走查 (e2e-kunxtier-single S2 / FEAT-kunxtier-manual-verification T04) 中发现: 同网络组内重复创建同名用户, relay 返回 500 "内部错误", 日志落 ERROR `UNIQUE constraint failed: users.network_id, users.name`。重复创建同名网络组同病 (networks.name UNIQUE)。节点注册路径 (consume_node_token) 已有同类映射, 这两处遗漏。

## 影响

- CLI / 控制台用户得到 "内部错误", 无从判断该换名还是该怀疑服务故障; 运维侧日志出现 ERROR 噪音 (实为业务性重复请求)。

## 修复

- store `create_user` / `create_network`: INSERT 的约束冲突按节点注册同款模式映射为 `Error::Usage` (400, 消息含冲突名), 其余存储错误原样透传
- 单测: `重名创建映射为业务错误而非内部错误`

## 验证

- 单测通过 (38 passed)
- 复现命令: 重复执行 `admin user create --network default --name <同名>`, 修复前 500, 修复后 400 与明确提示

## 状态

已修复并随验证流程闭环; 发现于 2026-10-30 单机验证 S2。
