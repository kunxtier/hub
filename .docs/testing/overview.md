# Testing Overview

## 前缀清单

- func-: 功能测试
- e2e-: 端到端测试
- perf-: 性能测试
- reg-: 回归测试

## 分组索引

### kunxtier (轻量私有虚拟网)

- e2e-kunxtier-network.md: 组网端到端场景矩阵 (验收入口, 定义场景与预期)
- e2e-kunxtier-single.md: 单机验证指导 (零外部环境依赖, 功能链路与控制面)
- e2e-kunxtier-multi.md: 多机验证指导 (局域网双机 / 受限网络 / 卫星协议 / 服务化自愈)
- e2e-kunxtier-public.md: 公网验证指导 (真实 NAT 打洞 / relay 生产托管 / 卫星真机 / Windows 与三平台产物)

## 阅读顺序

1. e2e-kunxtier-network.md: 首阶段核心链路验收入口 (矩阵定义 "验什么")
2. e2e-kunxtier-single.md → e2e-kunxtier-multi.md → e2e-kunxtier-public.md: 按环境依赖递进的执行指导 (定义 "怎么验"), 三者依次为前置
