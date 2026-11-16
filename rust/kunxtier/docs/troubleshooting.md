# 故障排查与安全须知

## 诊断入口

排查节点问题的三个入口, 按顺序使用:

- **`kunxtier status`**: 在线状态 / 本机 IP / 路径策略 / 对端
- **`kunxtier status --verbose`**: 追加最近关键事件
  (打洞 / 路径切换 / 信令 / 注册, 按时间旧到新)
- **日志**: 见下表

| 来源 | 位置 |
|---|---|
| relay | stderr, 或 `--log-file` 指定文件 |
| daemon (install 托管) | `log_dir/daemon.log`, 缺省 `/var/log/kunxtier` |
| daemon (前台运行) | stderr, 或 `--log-file` 指定文件 |
| 配置终值 | `kunxtier config` (脱敏) |

调高日志级别临时排障:

```sh
kunxtier relay --log-level debug
kunxtier daemon --log-level debug
```

## 常见问题

**首启凭证输出没看到或已丢失**
admin 凭证同时落盘在核心 state 目录的
`admin-credential` 文件 (0600), 直接读取即可。
服务化 (无终端) 运行时本就不回显明文, 以文件为准。

**up 报令牌无效**
节点令牌一次性, 可能已被使用或已过期撤销。
在核心侧重新 `token issue` 后重试。
已注册过的节点执行 up 可不带令牌直接重连。

**连接报指纹不匹配**
客户端固定的指纹与核心当前证书不一致,
常见于核心 state 目录被重建 (证书随之重新生成)。
取核心重建后的首启输出中的新指纹,
更新客户端 `--fingerprint` 或 `KUNXTIER_FINGERPRINT`。

**对端路径一直显示 relay**
打洞未成功, 可能处于对称 NAT 或受限网络,
回落中继属预期行为。
需要强制验证中继链路时可用
`admin node policy ... force_relay`。

**relay 启动报端口权限不足**
缺省监听 443 需要 root。
本地或无 443 场景用 `KUNXTIER_LISTEN` 指定高位端口,
客户端连接地址带端口即可。

**up / status 提示 daemon 未运行**
按提示先启动 daemon:
本地开发 `sudo kunxtier daemon`,
生产环境 `sudo kunxtier install` 注册系统服务。

**up / status 报 IPC 无权限**
state 目录权限与当前用户不匹配。
确认 `KUNXTIER_STATE_DIR` 与 daemon 启动时一致,
或以相应用户执行。

**install 失败**
两类常见原因: 未以 root 执行;
Windows 平台 (服务注册未实现, 直接报错)。

## 退出码

| 码 | 含义 | 排查方向 |
|---|---|---|
| 0 | 成功 | - |
| 1 | 运行失败 | 看错误信息与日志, 多为网络 / 服务端问题 |
| 2 | 用法或配置错误 | 检查参数与配置 (先 `kunxtier config`) |

## 安全须知

- **state 目录即信任边界**:
  核心 `kunxtier-state` 含 TLS 私钥与 SQLite,
  节点 state 目录含节点凭证与 WG 私钥。
  两者均严禁提交版本库或外传
- **凭证只走 env**: admin 凭证与 relay 令牌
  永不写入配置文件与日志
- **指纹固定防中间人**:
  所有客户端连接核心必须带证书指纹, 不要跳过
- **令牌即权限**: 节点令牌一次性但仍应保密分发;
  relay 令牌可复用, 泄露后立即 `token revoke`
- **relay 转发有速率限制**: 异常流量会被限速防护拦截
