//! Description: SQLite 存储: schema 版本化前向迁移; 所有业务流转经此处方法, 不向调用方暴露连接。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 凭证与令牌只存 sha256 哈希 (base64 形态), 明文不落库 (NFR-2 / NFR-8)。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};

use kunxtier_core::address;
use kunxtier_core::protocol::{NetworkInfo, NodeInfo};
use kunxtier_core::token::{self, Kind};

use crate::{Error, Result};

const SCHEMA_VERSION: i64 = 1;

pub struct Store {
    conn: Connection,
}

fn now() -> i64 {
    // unwrap 理由: 仅系统时钟早于 1970 时失败, 属不可达环境故障
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// 名字约束 (网络 / 用户 / 节点共用): [a-zA-Z0-9_-]{1,32}。
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn hash_b64(plain: &str) -> String {
    STANDARD.encode(token::hash(plain))
}

/// 节点表行映射 (list_nodes 共用)。
fn node_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<NodeInfo> {
    let ip_u32: u32 = r.get(1)?;
    Ok(NodeInfo {
        network: r.get(4)?,
        name: r.get(0)?,
        ip: address::ipv4_from_u32(ip_u32).to_string(),
        owner: r.get(2)?,
        created_at: r.get(3)?,
    })
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    /// 只做前向迁移; 未来版本的结构变更按版本号在此追加。
    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta(
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS networks(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                base INTEGER NOT NULL,
                next_ip INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS users(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                network_id INTEGER NOT NULL REFERENCES networks(id),
                name TEXT NOT NULL,
                disabled INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                UNIQUE(network_id, name));
            CREATE TABLE IF NOT EXISTS tokens(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                network_id INTEGER REFERENCES networks(id),
                user_id INTEGER REFERENCES users(id),
                expires_at INTEGER NOT NULL,
                used_at INTEGER,
                revoked_at INTEGER,
                created_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS nodes(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                network_id INTEGER NOT NULL REFERENCES networks(id),
                user_id INTEGER REFERENCES users(id),
                name TEXT NOT NULL,
                wg_public_key TEXT NOT NULL,
                ip INTEGER NOT NULL,
                credential_hash TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                removed_at INTEGER);
            -- 同名约束只作用于活跃行: 移除后的名字可被新节点复用 (FR-17)
            CREATE UNIQUE INDEX IF NOT EXISTS idx_nodes_name_active
                ON nodes(network_id, name) WHERE removed_at IS NULL;",
        )?;
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let cur: i64 = stored.unwrap_or_default().parse().unwrap_or(0);
        if cur > SCHEMA_VERSION {
            return Err(Error::Usage(format!(
                "数据库 schema 版本 {cur} 高于本程序支持的 {SCHEMA_VERSION}, 请先升级程序"
            )));
        }
        if cur < SCHEMA_VERSION {
            // v1: 初始结构已由上方 IF NOT EXISTS 建立, 此处落版本戳
            self.conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('schema_version', ?1)",
                params![SCHEMA_VERSION.to_string()],
            )?;
            tracing::info!(from = cur, to = SCHEMA_VERSION, "schema 迁移完成");
        }
        Ok(())
    }

    // ---- 管理员 ----

    pub fn set_admin_hash(&self, plain: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('admin_hash', ?1)",
            params![hash_b64(plain)],
        )?;
        Ok(())
    }

    pub fn verify_admin(&self, plain: &str) -> bool {
        let stored: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'admin_hash'", [], |r| {
                r.get(0)
            })
            .optional()
            .unwrap_or(None);
        match stored {
            Some(s) => token::ct_eq(s.as_bytes(), hash_b64(plain).as_bytes()),
            None => false,
        }
    }

    /// init 的"已初始化"判定: admin 哈希存在即视为已初始化;
    /// 不以 db 文件存在为准, 中途失败的残留库允许重跑 init 补齐。
    pub fn has_admin(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'admin_hash'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .is_some())
    }

    /// init 专用落库: 默认网络与 admin 哈希单事务提交, 失败整体回滚;
    /// 幂等 (default 已存在则复用), 返回 admin 凭证明文。
    pub fn bootstrap_default(&self, base: u32) -> Result<String> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO networks(name, base, created_at) VALUES('default', ?1, ?2)",
            params![base, now()],
        )?;
        let admin = token::generate(Kind::Admin);
        tx.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('admin_hash', ?1)",
            params![hash_b64(&admin)],
        )?;
        tx.commit()?;
        tracing::info!(network = "default", "默认网络与 admin 凭证落库");
        Ok(admin)
    }

    // ---- 网络组 ----

    pub fn create_network(&self, name: &str, base: u32) -> Result<NetworkInfo> {
        if !valid_name(name) {
            return Err(Error::Usage("网络名仅允许 1-32 位字母数字与 - _".into()));
        }
        let cidr = format!("{}/16", address::ipv4_from_u32(base));
        self.conn.execute(
            "INSERT INTO networks(name, base, created_at) VALUES(?1, ?2, ?3)",
            params![name, base, now()],
        )?;
        tracing::info!(network = name, cidr = %cidr, "网络组创建");
        Ok(NetworkInfo {
            name: name.to_string(),
            cidr,
            created_at: now(),
        })
    }

    pub fn network_count(&self) -> Result<u32> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM networks", [], |r| r.get(0))?;
        Ok(n as u32)
    }

    pub fn list_networks(&self) -> Result<Vec<NetworkInfo>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, base, created_at FROM networks ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| {
                let name: String = r.get(0)?;
                let base: u32 = r.get(1)?;
                let created_at: i64 = r.get(2)?;
                Ok(NetworkInfo {
                    cidr: format!("{}/16", address::ipv4_from_u32(base)),
                    name,
                    created_at,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn network_id(&self, name: &str) -> Result<(i64, u32)> {
        self.conn
            .query_row(
                "SELECT id, base FROM networks WHERE name = ?1",
                params![name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| Error::Usage(format!("网络不存在: {name}")))
    }

    // ---- 用户 ----

    pub fn create_user(&self, network: &str, name: &str) -> Result<i64> {
        if !valid_name(name) {
            return Err(Error::Usage("用户名仅允许 1-32 位字母数字与 - _".into()));
        }
        let (nid, _) = self.network_id(network)?;
        self.conn.execute(
            "INSERT INTO users(network_id, name, created_at) VALUES(?1, ?2, ?3)",
            params![nid, name, now()],
        )?;
        tracing::info!(network, user = name, "用户创建");
        Ok(self.conn.last_insert_rowid())
    }

    /// 禁用用户: 其名下令牌签发被拒 (存量节点凭证的实时吊销随信令阶段接入)。
    pub fn disable_user(&self, network: &str, name: &str) -> Result<()> {
        let (nid, _) = self.network_id(network)?;
        let n = self.conn.execute(
            "UPDATE users SET disabled = 1
             WHERE network_id = ?1 AND name = ?2 AND disabled = 0",
            params![nid, name],
        )?;
        if n == 0 {
            return Err(Error::Usage(format!("用户不存在或已禁用: {name}")));
        }
        tracing::info!(network, user = name, "用户禁用");
        Ok(())
    }

    fn user_row(&self, nid: i64, name: &str) -> Result<Option<(i64, bool)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, disabled FROM users WHERE network_id = ?1 AND name = ?2",
                params![nid, name],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? != 0)),
            )
            .optional()?)
    }

    // ---- 令牌 ----

    /// 签发令牌, kind: "node" (需绑定未禁用用户) | "relay" (不绑用户, 可复用)。
    pub fn issue_token(
        &self,
        network: &str,
        user: Option<&str>,
        kind: &str,
        ttl_secs: u64,
    ) -> Result<(i64, String)> {
        let kind = match kind {
            "relay" => {
                if user.is_some() {
                    return Err(Error::Usage("relay 令牌不绑定用户".into()));
                }
                "relay"
            }
            "node" | "" => "node",
            other => return Err(Error::Usage(format!("未知令牌类型: {other}"))),
        };
        let (nid, _) = self.network_id(network)?;
        let uid = match user {
            Some(u) => match self.user_row(nid, u)? {
                Some((id, false)) => Some(id),
                Some((_, true)) => return Err(Error::Usage(format!("用户已禁用: {u}"))),
                None => return Err(Error::Usage(format!("用户不存在: {u}"))),
            },
            None => {
                if kind == "node" {
                    return Err(Error::Usage("node 令牌必须指定用户".into()));
                }
                None
            }
        };
        let plain = token::generate(if kind == "relay" {
            Kind::RelayToken
        } else {
            Kind::NodeToken
        });
        let token_network = if kind == "relay" { None } else { Some(nid) };
        self.conn.execute(
            "INSERT INTO tokens(kind, token_hash, network_id, user_id, expires_at, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                kind,
                hash_b64(&plain),
                token_network,
                uid,
                now() + ttl_secs as i64,
                now()
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        tracing::info!(token_id = id, kind, network, "令牌签发");
        Ok((id, plain))
    }

    pub fn revoke_token(&self, id: i64) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE tokens SET revoked_at = ?2
             WHERE id = ?1 AND revoked_at IS NULL AND used_at IS NULL",
            params![id, now()],
        )?;
        if n == 0 {
            return Err(Error::Usage(format!("令牌不存在或已不可撤销: {id}")));
        }
        tracing::info!(token_id = id, "令牌撤销");
        Ok(())
    }

    // ---- 节点注册与生命周期 ----

    /// 消费一次性注册令牌: 校验、分配 /32、签发节点凭证, 单事务完成。
    /// 返回 (网络名, IP, 节点凭证明文)。
    pub fn consume_node_token(
        &self,
        plain: &str,
        name: &str,
        wg_public_key_b64: &str,
    ) -> Result<(String, String, String)> {
        if !token::looks_valid(plain) {
            return Err(Error::Usage("令牌格式非法".into()));
        }
        if !valid_name(name) {
            return Err(Error::Usage("节点名仅允许 1-32 位字母数字与 - _".into()));
        }
        let key = STANDARD
            .decode(wg_public_key_b64)
            .map_err(|_| Error::Usage("wg_public_key 不是合法 base64".into()))?;
        if key.len() != 32 {
            return Err(Error::Usage("wg_public_key 必须为 32 字节".into()));
        }

        let tx = self.conn.unchecked_transaction()?;
        let tok = tx
            .query_row(
                "SELECT id, kind, network_id, user_id, expires_at, used_at, revoked_at
                 FROM tokens WHERE token_hash = ?1",
                params![hash_b64(plain)],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, Option<i64>>(6)?,
                    ))
                },
            )
            .optional()?;
        let (tok_id, kind, nid, uid, expires, used, revoked) =
            tok.ok_or_else(|| Error::Usage("令牌不存在或已失效".into()))?;
        if kind != "node" {
            return Err(Error::Usage("令牌类型不匹配, 期望节点注册令牌".into()));
        }
        if used.is_some() {
            return Err(Error::Usage("令牌已被使用".into()));
        }
        if revoked.is_some() {
            return Err(Error::Usage("令牌已撤销".into()));
        }
        if expires <= now() {
            return Err(Error::Usage("令牌已过期".into()));
        }
        if let Some(uid) = uid {
            let disabled: Option<i64> = tx
                .query_row(
                    "SELECT disabled FROM users WHERE id = ?1",
                    params![uid],
                    |r| r.get(0),
                )
                .optional()?;
            match disabled {
                Some(d) if d == 0 => {}
                _ => return Err(Error::Usage("所属用户已禁用或不存在".into())),
            }
        }
        let (base, next_ip): (u32, u32) = tx.query_row(
            "SELECT base, next_ip FROM networks WHERE id = ?1",
            params![nid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let ip = address::host_at(base, next_ip)?;
        tx.execute(
            "UPDATE networks SET next_ip = next_ip + 1 WHERE id = ?1",
            params![nid],
        )?;
        let network_name: String = tx.query_row(
            "SELECT name FROM networks WHERE id = ?1",
            params![nid],
            |r| r.get(0),
        )?;
        let credential = token::generate(Kind::NodeCredential);
        tx.execute(
            "INSERT INTO nodes(network_id, user_id, name, wg_public_key, ip, credential_hash, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                nid,
                uid,
                name,
                wg_public_key_b64,
                u32::from(ip),
                hash_b64(&credential),
                now()
            ],
        )
        .map_err(|e| match &e {
            rusqlite::Error::SqliteFailure(f, _)
                if f.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Error::Usage(format!("节点名已存在: {name}"))
            }
            _ => Error::Store(e),
        })?;
        tx.execute(
            "UPDATE tokens SET used_at = ?2 WHERE id = ?1",
            params![tok_id, now()],
        )?;
        tx.commit()?;
        let ip_s = ip.to_string();
        tracing::info!(node = name, network = %network_name, ip = %ip_s, "节点注册");
        Ok((network_name, ip_s, credential))
    }

    pub fn remove_node(&self, network: &str, name: &str) -> Result<()> {
        let (nid, _) = self.network_id(network)?;
        let n = self.conn.execute(
            "UPDATE nodes SET removed_at = ?3
             WHERE network_id = ?1 AND name = ?2 AND removed_at IS NULL",
            params![nid, name, now()],
        )?;
        if n == 0 {
            return Err(Error::Usage(format!("节点不存在: {name}")));
        }
        tracing::info!(network, node = name, "节点移除 (凭证随之失效)");
        Ok(())
    }

    pub fn list_nodes(&self, network: Option<&str>) -> Result<Vec<NodeInfo>> {
        const SQL_ALL: &str = "SELECT n.name, n.ip, u.name, n.created_at, nw.name
             FROM nodes n
             JOIN networks nw ON nw.id = n.network_id
             LEFT JOIN users u ON u.id = n.user_id
             WHERE n.removed_at IS NULL
             ORDER BY n.id";
        const SQL_BY_NETWORK: &str = "SELECT n.name, n.ip, u.name, n.created_at, nw.name
             FROM nodes n
             JOIN networks nw ON nw.id = n.network_id
             LEFT JOIN users u ON u.id = n.user_id
             WHERE n.removed_at IS NULL AND nw.name = ?1
             ORDER BY n.id";
        let mut stmt = self
            .conn
            .prepare(match network {
                Some(_) => SQL_BY_NETWORK,
                None => SQL_ALL,
            })?;
        let rows = match network {
            Some(n) => stmt
                .query_map([n], node_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            None => stmt
                .query_map([], node_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    // ---- 备份 ----

    pub fn backup_to(&self, out: &Path) -> Result<()> {
        self.conn
            .execute("VACUUM INTO ?1", params![out.to_string_lossy()])?;
        tracing::info!(out = %out.display(), "备份导出");
        Ok(())
    }
}

// ------------------------------ 单元测试 --------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("state.db")).unwrap();
        (dir, s)
    }

    fn default_network(s: &Store) {
        s.create_network("default", address::nth_network_base(0).unwrap())
            .unwrap();
    }

    fn wg_key() -> String {
        STANDARD.encode([7u8; 32])
    }

    #[test]
    fn register_flow_token_single_use() {
        let (_d, s) = store();
        default_network(&s);
        s.create_user("default", "alice").unwrap();
        let (_id, token) = s.issue_token("default", Some("alice"), "node", 3600).unwrap();
        let (network, ip, cred) = s.consume_node_token(&token, "laptop", &wg_key()).unwrap();
        assert_eq!(network, "default");
        assert_eq!(ip, "100.64.0.1");
        assert!(cred.starts_with("kxt_nid_"));
        // 一次性: 二次消费被拒
        assert!(s.consume_node_token(&token, "laptop2", &wg_key()).is_err());
    }

    #[test]
    fn nodes_in_two_networks_are_isolated() {
        let (_d, s) = store();
        default_network(&s);
        s.create_network("net2", address::nth_network_base(1).unwrap())
            .unwrap();
        s.create_user("default", "a").unwrap();
        s.create_user("net2", "b").unwrap();
        s.create_user("net2", "b2").unwrap();
        let (_, t1) = s.issue_token("default", Some("a"), "node", 60).unwrap();
        let (_, t2) = s.issue_token("net2", Some("b"), "node", 60).unwrap();
        let (_, t3) = s.issue_token("net2", Some("b2"), "node", 60).unwrap();
        let (_, ip1, _) = s.consume_node_token(&t1, "n1", &wg_key()).unwrap();
        let (_, ip2, _) = s.consume_node_token(&t2, "n2", &wg_key()).unwrap();
        assert_eq!(ip1, "100.64.0.1");
        assert_eq!(ip2, "100.65.0.1");
        // 同名不跨网冲突
        let (_, ip3, _) = s.consume_node_token(&t3, "n1", &wg_key()).unwrap();
        assert_eq!(ip3, "100.65.0.2");
    }

    #[test]
    fn revoked_and_expired_tokens_rejected() {
        let (_d, s) = store();
        default_network(&s);
        s.create_user("default", "a").unwrap();
        let (id, t) = s.issue_token("default", Some("a"), "node", 3600).unwrap();
        s.revoke_token(id).unwrap();
        assert!(s.consume_node_token(&t, "x", &wg_key()).is_err());
        let (_, t2) = s.issue_token("default", Some("a"), "node", 0).unwrap();
        assert!(s.consume_node_token(&t2, "x", &wg_key()).is_err());
    }

    #[test]
    fn token_kind_rules() {
        let (_d, s) = store();
        default_network(&s);
        // node 令牌必须绑定用户
        assert!(s.issue_token("default", None, "node", 60).is_err());
        // relay 令牌不绑用户, 可复用语义 (多次签发独立)
        let (_, rt) = s.issue_token("default", None, "relay", 60).unwrap();
        assert!(rt.starts_with("kxt_relay_"));
        // relay 令牌不能用于节点注册
        assert!(s.consume_node_token(&rt, "x", &wg_key()).is_err());
    }

    #[test]
    fn remove_node_and_name_reuse() {
        let (_d, s) = store();
        default_network(&s);
        s.create_user("default", "a").unwrap();
        let (_, t) = s.issue_token("default", Some("a"), "node", 3600).unwrap();
        s.consume_node_token(&t, "laptop", &wg_key()).unwrap();
        s.remove_node("default", "laptop").unwrap();
        assert!(s.list_nodes(Some("default")).unwrap().is_empty());
        assert!(s.remove_node("default", "laptop").is_err());
        // 同名可再注册 (新身份), IP 不复用 (FR-17)
        let (_, t2) = s.issue_token("default", Some("a"), "node", 3600).unwrap();
        let (_, ip, _) = s.consume_node_token(&t2, "laptop", &wg_key()).unwrap();
        assert_eq!(ip, "100.64.0.2");
    }

    #[test]
    fn disabled_user_blocks_token_issue() {
        let (_d, s) = store();
        default_network(&s);
        s.create_user("default", "bob").unwrap();
        s.disable_user("default", "bob").unwrap();
        // 已禁用: 签发被拒, 重复禁用报错
        assert!(s.issue_token("default", Some("bob"), "node", 60).is_err());
        assert!(s.disable_user("default", "bob").is_err());
    }

    #[test]
    fn admin_hash_roundtrip() {
        let (_d, s) = store();
        s.set_admin_hash("kxt_admin_secret").unwrap();
        assert!(s.verify_admin("kxt_admin_secret"));
        assert!(!s.verify_admin("kxt_admin_wrong"));
    }

    #[test]
    fn backup_restores_full_state() {
        let (dir, s) = store();
        default_network(&s);
        s.create_user("default", "a").unwrap();
        let (_, t) = s.issue_token("default", Some("a"), "node", 3600).unwrap();
        s.consume_node_token(&t, "n", &wg_key()).unwrap();
        let out = dir.path().join("backup.db");
        s.backup_to(&out).unwrap();
        // 恢复侧通过公开查询接口核对 (只读)
        let restored = Store::open(&out).unwrap();
        assert_eq!(restored.list_networks().unwrap().len(), 1);
        assert_eq!(restored.list_nodes(None).unwrap().len(), 1);
    }

    #[test]
    fn schema_version_is_stamped() {
        let (_d, s) = store();
        let v: String = s
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "1");
    }
}
