# Architectures Overview

## 前缀清单

- arch-: 架构层 (分层、模块边界、依赖方向、总体拆分)
- impl-: 实现细节 (单模块内机制设计)
- infra-: 基础设施 (部署、构建、CI / CD)

## 分组索引

### 仓库级

- arch-structure.md: hub monorepo 结构约定与拆仓原则

### kunxtier (轻量私有虚拟网)

- arch-kunxtier-overview.md: 总体架构、模块边界与关键流程

## 阅读顺序

1. arch-structure.md: 先明确仓库组织方式
2. arch-kunxtier-overview.md: 再看 kunxtier 系统内拆分
