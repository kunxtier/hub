# 控制台前端规范合规整改 (react/typescript/tailwindcss/basic-ui)

## 背景

2026-10-31 以 `.claude/rules/lang/{react,typescript,tailwindcss}.md` 与 `.claude/skills/basic-ui` 为规范对照 `web/kunxtier-console` 全量 22 个 ts/tsx 审计, 13 条违规 (高 3 / 中 4 / 低 6)。高危项已抽查核实代码证据。

合规面: 文件头与 JSDoc 全覆盖; useEffect 依赖与清理; 派生值现算; 列表 key 稳定; cn() 分组+条件置后; 语义 token + dark 适配; 边框/字重分层结构风格; 登录登出错误处理。

## 目标

- cn 工具切到 `cn` 包单一实现, 移除 clsx/tailwind-merge
- 查询三态齐全: loading / empty / error 一个不缺
- 类型收紧: 复用 api/types.ts 投影, 封闭词汇用字面量联合

## 范围

### 高

| 条目 | 位置 | 问题 |
| --- | --- | --- |
| 自封装 cn 且三依赖并存 | `src/lib/utils.ts:8-13`, `package.json:19-24` | `twMerge(clsx(inputs))` 自封装, clsx/tailwind-merge/cn 三依赖并存, `cn` 包引入未用; 规则要求 re-export `cn` 并移除另两个 |
| 查询缺 error 态 | `Overview.tsx:44-49`, `Nodes.tsx:65-68`, `Relays.tsx:26-29`, `Tokens.tsx:229-232`, `NetworkSwitcher.tsx:37` | 只判 loading/empty, 失败被空态掩盖, 违反交互边界 |
| Networks 无 empty 无 error | `Networks.tsx:72-110` | 空数据与失败都渲染成只有表头的空表格 |

### 中

| 条目 | 位置 | 问题 |
| --- | --- | --- |
| 匿名 Context.Provider | `src/state/AppContext.tsx:57` | JSX 内裸写 `<AppContext.Provider>`, 未具名包裹也未赋 displayName |
| Tokens.tsx 职责混杂 | `src/pages/Tokens.tsx:42-378` | 378 行: 页面编排 + 3 表单 + 7 mutation + 用户表格全混一个组件 |
| 重复发明类型 | `Nodes.tsx:175`, `Overview.tsx:101` | 手写内联近似结构, 未复用 `NodeInfo`/`PeerPathReport` |
| 封闭词汇用宽类型 | `src/api/types.ts:23`, `src/lib/summary.ts:11` | `path: string` 与 `Record<string, string>`, 应为字面量联合 `PathStatus` |

### 低

| 条目 | 位置 | 问题 |
| --- | --- | --- |
| 一文件多导出组件 | `src/components/ui/select.tsx:15-20` | Select 与 SelectValue 同文件导出 (shadcn 生成物, 可豁免或注释说明) |
| policy 参数宽类型 | `Nodes.tsx:152` | `policy: string`, 应为 `PathPolicy` |
| 错误上下文被压扁 | `src/api/client.ts:68-70` | 非 ApiRequestError 一律 "操作失败, 请重试", 无日志留痕 |
| 失控剪贴板 Promise | `Tokens.tsx:171` | `void navigator.clipboard.writeText`, 失败静默但 UI 已置"已复制" |
| utils 弱语义命名 | `src/lib/utils.ts` | 改 re-export cn 包后可连同消除 |
| bg-muted 边界用法 | `Tokens.tsx:178` | 令牌明文块用背景色, 属强调语义的边界情况, 可保留但记录 |

## 非目标

- 不重做页面信息架构, 只补态与收紧类型
- shadcn 生成物 (select.tsx) 不为合规而改, 注明豁免即可
- 不在本 issue 做 API 分页适配 (响应形态变更由 REF-kunxtier-admin-api-contract 驱动, 届时协同)

## 方案讨论

- error 态呈现统一做一个小组件 (ErrorHint), 五处复用, 避免 copy-paste
- Tokens.tsx 拆分按表单域: UserManagement / TokenIssue / TokenList 三块
- PathStatus 字面量联合需与后端 protocol.rs 词汇对齐 (idle/punching/direct/relayed), 属契约投影

## 风险与边界

- 移除 clsx/tailwind-merge 前需 grep 确认无其他直接 import
- cn 包 re-export 后 components.json 别名指向不变, shadcn 生态约定保持

## 验收口径

- package.json 无 clsx/tailwind-merge; utils.ts 为 re-export 或按 cn 包迁移指南落地
- 五个查询页 error 态可见且文案可区分; Networks 空态与失败态可区分
- AppContext Provider 具名; Tokens.tsx 单文件不超约 150 行
- NodeInfo/PathPolicy/PathStatus 复用与收紧后 `grep "path: string"` 在 types.ts 无命中
