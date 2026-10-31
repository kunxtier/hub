# init 并发互斥缺失: 首启竞态可产生证书/私钥错配

## 背景

2026-10-31 BUG-kunxtier-runtime-storage-observability 审查轮发现 (先于本切片已存在): `relay --init` 与运行态自动初始化 (`ensure_initialized`) 均是 "先查 `has_admin` 再写证书/私钥/凭证" 的检查后写入, 无进程间互斥。

竞态场景: 全新 state 目录上, systemd 自启的 relay 与手动 `kunxtier relay --init` 同时进入 — 双方都通过 `has_admin == false` 判定, 各自生成自签证书并先后 rename 落盘。`write_atomic` 只保证单文件原子性, 不保证文件组一致性, 最终可能出现 A 进程的 cert.pem 配 B 进程的 key.pem; 落库侧后到者在 `bootstrap_default` 的 UNIQUE 约束上失败。

影响: 客户端尚未固定指纹的首启窗口内, 错配证书导致 TLS 握手失败; 由于 admin 哈希已落库, 重跑 init 被拒, 需手工清理 state 目录恢复。

## 目标

- init 入口具备进程间互斥: 同一 state 目录同时只有一个 init 执行体

## 范围

| 条目 | 位置 | 问题 |
| --- | --- | --- |
| init 无进程间互斥 | `kunxtier-relay/src/init.rs` (`init` / `ensure_initialized`) | 检查后写入竞态, 证书/私钥文件组可错配 |

## 非目标

- 不处理跨主机并发 (state 目录本就单机信任边界)
- 不改变 init 的步骤顺序与输出契约

## 方案讨论

- 倾向 state 目录内 `O_EXCL` 创建 `init.lock` (拿到即执行, 结束删除; 残留锁以 pid 存活检测兜底), 比 flock 少一层平台差异
- 备选: 把 "检查 + 写证书 + 落库" 收进单个 SQLite 事务并不能覆盖证书文件, 不解决文件组一致性, 排除

## 风险与边界

- 锁文件残留 (进程被 kill) 需可自愈, 否则 init 永久卡死比竞态更糟

## 验收口径

- 并发两个 init 进程: 恰好一个成功, 另一个以明确错误退出 (提示已有 init 进行中或已初始化)
- 落盘文件组 (cert/key/admin 凭证) 始终出自同一生成批次
