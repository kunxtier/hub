//! Description: 引擎级端到端验证: 双引擎经真实 UDP 环回完成 WireGuard 握手与双向互通。
//!
//! Author: qinzhenya
//! Created: 2026-10-21
//!
//! TUN 侧用 gotatun 通道实现 (免特权, 仅承载 UDP 语义载荷), UDP 侧为真实
//! 127.0.0.1 socket: noise 握手、加解密、端点学习、节点表对齐全走生产代码路径。
//! 真实 TUN 设备的特权冒烟见 tun 模块 #[ignore] 用例。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use gotatun::packet::{Packet, PacketBufPool};
use gotatun::tun::channel::new_udp_tun_channel;
use gotatun::tun::MtuWatcher;
use gotatun::udp::socket::UdpSocketFactory;
use gotatun::udp::{UdpRecv, UdpSend, UdpTransportFactory, UdpTransportFactoryParams};
use kunxtier_node::engine::{build_engine_split, EngineOptions, PeerSpec};
use kunxtier_node::identity::WgIdentity;
use kunxtier_node::DEFAULT_MTU;

/// 探测一个空闲 UDP 端口 (探测与绑定之间存在竞态, 环回单测可接受)。
fn free_udp_port() -> u16 {
    // expect 理由: 环回 socket 绑定失败属环境级故障, 无恢复价值
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").expect("环回 UDP 绑定失败");
    sock.local_addr().expect("读取端口失败").port()
}

/// 通道工厂 bind 参数 (通道实现不实际监听, 仅取端口语义)。
fn bind_params() -> UdpTransportFactoryParams {
    #[cfg(not(target_os = "linux"))]
    return UdpTransportFactoryParams {
        addr_v4: Ipv4Addr::LOCALHOST,
        addr_v6: Ipv6Addr::UNSPECIFIED,
        port: 0,
    };
    #[cfg(target_os = "linux")]
    return UdpTransportFactoryParams {
        addr_v4: Ipv4Addr::LOCALHOST,
        addr_v6: Ipv6Addr::UNSPECIFIED,
        port: 0,
        fwmark: None,
    };
}

fn peer_spec(identity: &WgIdentity, ip: Ipv4Addr, endpoint: Option<SocketAddr>) -> PeerSpec {
    PeerSpec {
        public_key: identity.public_key(),
        virtual_ip: ip,
        endpoint,
        keepalive_secs: Some(25),
    }
}

#[tokio::test]
async fn two_engines_establish_session_over_real_udp() {
    // 诊断通道: RUST_LOG 控制粒度, 缺省静默
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "off".into()),
        )
        .try_init();

    let identity_a = WgIdentity::generate();
    let identity_b = WgIdentity::generate();
    let ip_a: Ipv4Addr = "100.64.0.1".parse().expect("测试地址恒合法");
    let ip_b: Ipv4Addr = "100.64.0.2".parse().expect("测试地址恒合法");
    let port_a = free_udp_port();
    let port_b = free_udp_port();

    let (tun_tx_a, tun_rx_a, mut factory_a) = new_udp_tun_channel(
        64,
        ip_a,
        Ipv6Addr::UNSPECIFIED,
        MtuWatcher::new(DEFAULT_MTU),
    );
    let (tun_tx_b, tun_rx_b, mut factory_b) = new_udp_tun_channel(
        64,
        ip_b,
        Ipv6Addr::UNSPECIFIED,
        MtuWatcher::new(DEFAULT_MTU),
    );

    let engine_a = build_engine_split(
        EngineOptions {
            private_key: identity_a.secret().clone(),
            listen_port: port_a,
        },
        UdpSocketFactory::default(),
        tun_tx_a,
        tun_rx_a,
    )
    .await
    .expect("引擎 A 装配失败");
    let engine_b = build_engine_split(
        EngineOptions {
            private_key: identity_b.secret().clone(),
            listen_port: port_b,
        },
        UdpSocketFactory::default(),
        tun_tx_b,
        tun_rx_b,
    )
    .await
    .expect("引擎 B 装配失败");

    assert_eq!(engine_a.listen_port(), port_a);
    assert_eq!(engine_b.listen_port(), port_b);

    // 通道侧句柄: 注入 (模拟主机发包进 TUN) 与观察 (收取引擎写回 TUN 的明文)
    let ((inject_a, mut observe_a), _) = factory_a
        .bind(&bind_params())
        .await
        .expect("通道 A 绑定失败");
    let ((inject_b, mut observe_b), _) = factory_b
        .bind(&bind_params())
        .await
        .expect("通道 B 绑定失败");

    // A 已知 B 端点 (对齐打洞前); B 端点未知, 由入向握手学习
    engine_a
        .sync_peers(&[peer_spec(
            &identity_b,
            ip_b,
            Some(SocketAddr::from(([127, 0, 0, 1], port_b))),
        )])
        .await
        .expect("A 节点表对齐失败");
    engine_b
        .sync_peers(&[peer_spec(&identity_a, ip_a, None)])
        .await
        .expect("B 节点表对齐失败");

    let mut pool = PacketBufPool::new(16);

    // A -> B: 首包触发按需握手, 会话建立后载荷送达
    let payload_a = b"engine-loopback-a-to-b".to_vec();
    inject_a
        .send_to(
            Packet::copy_from(&payload_a[..]),
            SocketAddr::from((ip_b, 9999)),
        )
        .await
        .expect("A 注入失败");
    let (received, from) =
        tokio::time::timeout(Duration::from_secs(15), observe_b.recv_from(&mut pool))
            .await
            .expect("A->B 超时: 会话未在窗口内建立并送达")
            .expect("B 侧接收失败");
    assert_eq!(&received[..], &payload_a[..], "A->B 载荷不一致");
    assert_eq!(
        from.ip(),
        IpAddr::V4(ip_a),
        "B 侧观察到的源地址应为 A 虚拟 IP"
    );

    // B -> A: B 已通过入向握手学习到 A 端点
    let payload_b = b"engine-loopback-b-to-a".to_vec();
    inject_b
        .send_to(
            Packet::copy_from(&payload_b[..]),
            SocketAddr::from((ip_a, 9998)),
        )
        .await
        .expect("B 注入失败");
    let (received, from) =
        tokio::time::timeout(Duration::from_secs(15), observe_a.recv_from(&mut pool))
            .await
            .expect("B->A 超时: 端点学习或回程路径未生效")
            .expect("A 侧接收失败");
    assert_eq!(&received[..], &payload_b[..], "B->A 载荷不一致");
    assert_eq!(
        from.ip(),
        IpAddr::V4(ip_b),
        "A 侧观察到的源地址应为 B 虚拟 IP"
    );

    // 节点表摘除: 空表对齐后 B 离开, A 不再向其转发
    engine_a.sync_peers(&[]).await.expect("A 摘除节点失败");
    inject_a
        .send_to(
            Packet::copy_from(b"should-be-dropped"[..].as_ref()),
            SocketAddr::from((ip_b, 9997)),
        )
        .await
        .expect("A 注入失败");
    let dropped =
        tokio::time::timeout(Duration::from_secs(2), observe_b.recv_from(&mut pool)).await;
    assert!(dropped.is_err(), "B 已离表, 不应再收到 A 的转发");

    engine_a.stop().await;
    engine_b.stop().await;
}
