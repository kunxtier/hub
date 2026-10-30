# 未注册路径的 POST 请求返回 405 而非控制台 fallback 判定

## 背景

单机验证走查用户启用操作时发现: POST 打到未注册的 admin API 路径 (如旧版 relay 收到新前端的 /users/enable) 返回 405 Method Not Allowed, 而非预期 fallback 判定 (API 前缀 404 JSON / 页面路径回落入口页)。

## 根因

`build_router` 的 `fallback_service(get(serve_console))` 以仅收 GET 的 MethodRouter 兜底: 未匹配路径的 POST 在 MethodRouter 层被拒 (405), 根本进不了 serve_console 的路径判定, 与其注释意图不符。

## 修复

fallback 改为 `any(serve_console)`: 任意方法的未匹配路径都进入 handler 判定 — /api 与 /ws 前缀保持 404 JSON, 其余页面路径回落入口页。已注册路由不受影响。

## 验证

- relay 40 测试通过
- 行为核对: POST 未注册 /api 路径现返回 404 JSON (语义: 路径不存在), 而非 405 (方法不存在)

## 状态

已修复; 发现于 2026-10-31 单机验证 (配合新旧二进制混跑场景暴露)。
