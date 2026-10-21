# kunxtier

轻量 P2P 虚拟组网工具 (ZeroTier / EasyTier 平替)。

设计文档见仓库 `.docs/` (需求 / 架构 / 测试 / issue)。

## 构建

```sh
cargo build --release
```

## 快速开始 (当前切片: 控制面登记闭环)

```sh
# 初始化核心中继: 自签证书 / admin 凭证 / 默认网络 (明文凭证仅展示一次)
cargo run -p kunxtier-cli --bin kunxtier -- relay --init

# 启动核心中继 (管理 API + 节点登记; 信令与转发后续阶段接入)
cargo run -p kunxtier-cli --bin kunxtier -- relay

# 管理操作 (admin 凭证走 env, 指纹来自 init 输出)
export KUNXTIER_ADMIN=kxt_admin_xxx
kunxtier admin --server https://127.0.0.1:443 --fingerprint <sha256> user create --network default --name alice
kunxtier admin --server ... --fingerprint ... token issue --network default --user alice
```

节点注册 (`kunxtier up`) 与数据面 (TUN / WireGuard) 在后续阶段交付。

## 测试

```sh
cargo test
```
