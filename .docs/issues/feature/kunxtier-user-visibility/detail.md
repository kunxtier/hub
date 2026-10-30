# kunxtier 用户 (属主) 可见性

## 背景

单机验证 S2 走查发现: 创建用户后无任何确认与发现途径 — admin API 无 GET /users, CLI 无 user list, 控制台无用户页 (需求页面结构本就未含用户页, 属主为标注概念)。用户名下暂无节点时界面完全无痕; 禁用用户只能凭记忆拼名字, 拼错仅得 "用户不存在"。

## 范围

- T01 (已完成): GET /api/admin/users (query network 可选, 缺省跨网络组) 与 CLI `admin user list [--network]`, 返回 network / name / disabled / created_at; store 单测覆盖跨网络与禁用状态
- T02 (已完成): 控制台侧用户可见性 — 落在既有 "令牌与用户" 页 (不新增页面, 需求页面结构不变): 独立 "禁用用户" 表单替换为用户列表区块 (用户名 / 状态徽标 / 创建时间 / 行内禁用, 3s 轮询), 创建与禁用操作后自动刷新列表

## 验收口径

- T01: `admin user list` 能列出已建用户与禁用状态, 与创建/禁用操作闭环
- T02: 界面上创建用户后可直接确认结果, 不依赖 CLI

## 边界

- 属主登录与自助管理仍为非目标 (feat-kunxtier-console 边界不变)
