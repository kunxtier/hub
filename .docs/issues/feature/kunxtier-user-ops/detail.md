# kunxtier 用户管理完整操作集

## 背景

用户可见性 (FEAT-kunxtier-user-visibility) 交付后, 验证中发现用户操作集不完整: 禁用实现了状态位却无恢复入口 (事实上的单向操作), 亦无改名与删除。原需求 FR-1 仅定义创建/禁用, "用户删除" 被列为低频补充; 验证走查中拍板补齐。

## 范围

- store / admin API / CLI / 控制台四层补齐: enable (禁用逆操作)、rename (属主标注按 user_id 关联随之传播)、delete (受限: 名下有未移除节点时拒绝; 历史令牌随用户清理, 已移除节点属主引用置空)
- 路由沿用既有命名风格: POST /api/admin/users/{enable,rename,delete}
- CLI: `admin user enable / rename <new_name> / remove`
- 控制台 "令牌与用户" 页用户列表操作列: 禁用/启用切换、改名 (prompt)、删除 (确认)
- 需求同步: feat-kunxtier-access FR-1 扩为完整操作集, 非目标区 "用户删除" 条目移除

## 边界

- 删除不级联未移除节点 (拒绝并提示先移除); 历史节点行保留, 属主标注退为 "-"
- 属主登录与自助管理仍为非目标

## 验证

- store 单测: 禁用/启用往返、改名冲突与属主传播、有节点拒绝删除、移除后可删 (40 passed)
- 前端 oxlint + 格式检查通过, cargo make build 重嵌
