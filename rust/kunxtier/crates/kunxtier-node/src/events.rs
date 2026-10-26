//! Description: 节点关键事件留痕: 有界环形缓冲, status --verbose 排障视图的数据源 (NFR FR-3)。
//!
//! Author: qinzhenya
//! Created: 2026-10-26
//!
//! 只记四类 (打洞 / 路径切换 / 信令 / 注册), 事件产生点与 tracing 埋点同位双写;
//! 对端最近失败原因与中继选择过程从环内事件推导, 不另存第二份状态源。
//! 规划名 NodeEvent 与 signal::NodeEvent 撞名, 实施期更名 EventRecord。

use std::collections::VecDeque;
use std::fmt;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 环容量: 覆盖个人规模排障回看窗口, 内存有界。
const CAPACITY: usize = 256;

/// 事件类别: 日志与排障视图的稳定检索键 (NFR FR-2)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// 打洞尝试与结果 (含失败原因)。
    Punch,
    /// 路径切换 (direct 与 relay 互切 / 策略切换 / 中继选择)。
    Path,
    /// 信令断连与重连、对端上下线。
    Signal,
    /// 注册与凭证校验。
    Register,
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            EventKind::Punch => "punch",
            EventKind::Path => "path",
            EventKind::Signal => "signal",
            EventKind::Register => "register",
        };
        f.write_str(s)
    }
}

/// 单条事件; ts 为 unix 秒, detail 为人读描述 (含端点等关键量)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub ts: u64,
    pub kind: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    pub detail: String,
}

/// 有界事件环: 满即淘汰最旧; snapshot 按时间序 (旧 → 新) 返回。
#[derive(Debug, Default)]
pub struct EventRing {
    events: Mutex<VecDeque<EventRecord>>,
}

impl EventRing {
    pub fn push(&self, kind: EventKind, peer: Option<&str>, detail: impl Into<String>) {
        let event = EventRecord {
            ts: now_unix(),
            kind,
            peer: peer.map(str::to_string),
            detail: detail.into(),
        };
        // expect 理由: 锁中毒仅发生在持有线程 panic 后, 快速失败优于静默丢事件
        let mut events = self.events.lock().expect("事件环锁中毒");
        if events.len() >= CAPACITY {
            events.pop_front();
        }
        events.push_back(event);
    }

    pub fn snapshot(&self) -> Vec<EventRecord> {
        self.events
            .lock()
            .expect("事件环锁中毒")
            .iter()
            .cloned()
            .collect()
    }

    /// 长度感知快照: 自最新向旧收取, 序列化超预算即止 (保 IPC 单行帧上限),
    /// 至少保留最新一条。
    pub fn snapshot_bounded(&self, max_bytes: usize) -> Vec<EventRecord> {
        let events = self.events.lock().expect("事件环锁中毒");
        let mut picked: Vec<EventRecord> = Vec::new();
        let mut size = 2; // 数组方括号
        for event in events.iter().rev() {
            // 序列化失败 (字段类型固定, 不应发生) 按超预算处理: 宁可少给不可断连
            let need = serde_json::to_vec(event)
                .map(|v| v.len() + 1)
                .unwrap_or(usize::MAX);
            if size + need > max_bytes && !picked.is_empty() {
                break;
            }
            size = size.saturating_add(need);
            picked.push(event.clone());
        }
        picked.reverse();
        picked
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_latest_capacity_events_in_order() {
        let ring = EventRing::default();
        for i in 0..(CAPACITY + 50) {
            ring.push(EventKind::Punch, Some("peer"), format!("e{i}"));
        }
        let snap = ring.snapshot();
        assert_eq!(snap.len(), CAPACITY);
        assert_eq!(snap.first().unwrap().detail, "e50");
        assert_eq!(snap.last().unwrap().detail, format!("e{}", CAPACITY + 49));
        assert!(snap.windows(2).all(|w| w[0].ts <= w[1].ts));
    }

    #[test]
    fn bounded_snapshot_stays_within_budget() {
        let ring = EventRing::default();
        for i in 0..64 {
            ring.push(EventKind::Path, Some(&format!("peer-{i}")), "x".repeat(200));
        }
        let snap = ring.snapshot_bounded(4 * 1024);
        let size = serde_json::to_vec(&snap).unwrap().len();
        assert!(size <= 4 * 1024, "截断后应落在预算内: {size}");
        assert!(
            !snap.is_empty() && snap.len() < 64,
            "超预算应截断: {}",
            snap.len()
        );
        // 预算内保最新段 (截头不截尾)
        assert_eq!(snap.last().unwrap().detail.len(), 200);
        assert_eq!(snap.last().unwrap().peer.as_deref(), Some("peer-63"));
    }

    #[test]
    fn record_serializes_kind_and_optional_peer() {
        let record = EventRecord {
            ts: 1,
            kind: EventKind::Path,
            peer: Some("a".into()),
            detail: "直连失活, 回落中继".into(),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"kind\":\"path\""));
        let back: EventRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);

        let no_peer = EventRecord {
            peer: None,
            ..record
        };
        assert!(!serde_json::to_string(&no_peer).unwrap().contains("peer"));
    }
}
