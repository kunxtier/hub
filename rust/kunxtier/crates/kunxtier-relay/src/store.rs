//! Description: SQLite 存储: schema 版本化前向迁移; 所有业务流转经此处方法, 不向调用方暴露连接。
//!
//! Author: qinzhenya
//! Created: 2026-10-20
//!
//! 凭证与令牌只存 sha256 哈希 (base64 形态), 明文不落库 (NFR-2 / NFR-8)。

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};

use kunxtier_core::address;
use kunxtier_core::protocol::{NetworkInfo, NodeInfo, Page, PathPolicy, SignalPeer, UserInfo};
use kunxtier_core::token::{self, Kind};

use crate::{Error, Result};

const SCHEMA_VERSION: i64 = 2;

pub struct Store {
    conn: Connection,
}

/// 信令认证查询结果 (auth_lookup): 挑战应答校验所需的最小字段。
pub struct NodeAuth {
    /// base64(std) 的 sha256(节点凭证), 与库中存储形态一致。
    pub credential_hash_b64: String,
    pub ip: String,
}

fn now() -> i64 {
    // unwrap 理由: 仅系统时钟早于 1970 时失败, 属不可达环境故障
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// 名字约束 (网络 / 用户 / 节点共用): [a-zA-Z0-9_-]{1,32}。
/// server 侧列表过滤白名单复用同一取值约束。
pub(crate) fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 分页 offset 换算: page 从 1 起, 饱和运算不外溢。
fn offset_of(page: u32, size: u32) -> u64 {
    (page as u64).saturating_sub(1).saturating_mul(size as u64)
}

fn hash_b64(plain: &str) -> String {
    STANDARD.encode(token::hash(plain))
}

/// 节点表行映射 (list_nodes 共用)。
fn node_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<NodeInfo> {
    let ip_u32: u32 = r.get(1)?;
    let policy: String = r.get(5)?;
    Ok(NodeInfo {
        network: r.get(4)?,
        name: r.get(0)?,
        ip: address::ipv4_from_u32(ip_u32).to_string(),
        owner: r.get(2)?,
        created_at: r.get(3)?,
        // 在线态由服务端注册表合并, 存储层一律填 false
        online: false,
        policy: policy.parse().unwrap_or(PathPolicy::Auto),
        // 候选地址与版本同样由信令注册表合并, 存储层填空
        candidates: Vec::new(),
        version: None,
        paths: Vec::new(),
    })
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        // 运行 / 备份不得隐式建库: 目录缺失时 SQLite 只报难以定位的裸错误,
        // init 是唯一创建入口 (建目录在先), 此处前置拦截给出指引
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() && !dir.exists() {
                return Err(Error::Usage(format!(
                    "数据目录不存在: {}; 请先执行 kunxtier relay --init 初始化",
                    dir.display()
                )));
            }
        }
        let conn = Connection::open(path)?;
        // 单连接被信令/转发/HTTP/CLI 多处共享, 写锁等待必须有上界 (5s);
        // WAL 让读不再阻塞写, -wal/-shm 文件随连接存在, 最后一个连接干净关闭时自动合并回主库
        conn.busy_timeout(Duration::from_millis(5_000))?;
        // WAL 失败不阻断启动 (个别文件系统不支持), busy_timeout 仍兜底写锁上界;
        // 但降级必须留痕可辨, 否则部署差异无从发现
        let mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            tracing::warn!(journal_mode = %mode, "WAL 未生效, 读写并发退回阻塞模式");
        }
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
            // v2: 节点级路径策略 (FR-10)。新建库由下方 ALTER 统一补列,
            // 与 v1 存量库走同一路径, 避免两份建表语句漂移。
            let has_policy: bool = self
                .conn
                .prepare("PRAGMA table_info(nodes)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<String>, _>>()?
                .iter()
                .any(|c| c == "policy");
            if !has_policy {
                self.conn.execute(
                    "ALTER TABLE nodes ADD COLUMN policy TEXT NOT NULL DEFAULT 'auto'",
                    [],
                )?;
            }
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
            .query_row("SELECT value FROM meta WHERE key = 'admin_hash'", [], |r| {
                r.get::<_, String>(0)
            })
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
        self.conn
            .execute(
                "INSERT INTO networks(name, base, created_at) VALUES(?1, ?2, ?3)",
                params![name, base, now()],
            )
            .map_err(|e| match &e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Error::Conflict(format!("网络组已存在: {name}"))
                }
                _ => Error::Store(e),
            })?;
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

    /// 网络组分页列表 (page 从 1 起); total 精确计数, 小表成本可忽略。
    pub fn list_networks(&self, page: u32, size: u32) -> Result<Page<NetworkInfo>> {
        let total: u64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM networks", [], |r| r.get(0))?;
        let mut stmt = self.conn.prepare(
            "SELECT name, base, created_at FROM networks ORDER BY id LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt
            .query_map(params![size, offset_of(page, size)], |r| {
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
        Ok(Page {
            items: rows,
            page,
            size,
            total,
        })
    }

    fn network_id(&self, name: &str) -> Result<(i64, u32)> {
        self.conn
            .query_row(
                "SELECT id, base FROM networks WHERE name = ?1",
                params![name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("网络不存在: {name}")))
    }

    // ---- 用户 ----

    pub fn create_user(&self, network: &str, name: &str) -> Result<i64> {
        if !valid_name(name) {
            return Err(Error::Usage("用户名仅允许 1-32 位字母数字与 - _".into()));
        }
        let (nid, _) = self.network_id(network)?;
        self.conn
            .execute(
                "INSERT INTO users(network_id, name, created_at) VALUES(?1, ?2, ?3)",
                params![nid, name, now()],
            )
            .map_err(|e| match &e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Error::Conflict(format!("用户已存在: 网络组 {network} 内 {name}"))
                }
                _ => Error::Store(e),
            })?;
        tracing::info!(network, user = name, "用户创建");
        Ok(self.conn.last_insert_rowid())
    }

    /// 用户 (属主) 分页列表; network 为 None 时跨网络组全量。
    pub fn list_users(
        &self,
        network: Option<&str>,
        page: u32,
        size: u32,
    ) -> Result<Page<UserInfo>> {
        let total: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM users u JOIN networks nw ON nw.id = u.network_id
             WHERE (?1 IS NULL OR nw.name = ?1)",
            params![network],
            |r| r.get(0),
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT nw.name, u.name, u.disabled, u.created_at
             FROM users u JOIN networks nw ON nw.id = u.network_id
             WHERE (?1 IS NULL OR nw.name = ?1)
             ORDER BY u.network_id, u.id LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt
            .query_map(params![network, size, offset_of(page, size)], |r| {
                Ok(UserInfo {
                    network: r.get(0)?,
                    name: r.get(1)?,
                    disabled: r.get::<_, i64>(2)? != 0,
                    created_at: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Page {
            items: rows,
            page,
            size,
            total,
        })
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
            return Err(Error::NotFound(format!("用户不存在或已禁用: {name}")));
        }
        tracing::info!(network, user = name, "用户禁用");
        Ok(())
    }

    /// 启用用户 (禁用的逆操作): 名下令牌恢复可用, 节点重连即恢复。
    pub fn enable_user(&self, network: &str, name: &str) -> Result<()> {
        let (nid, _) = self.network_id(network)?;
        let n = self.conn.execute(
            "UPDATE users SET disabled = 0
             WHERE network_id = ?1 AND name = ?2 AND disabled = 1",
            params![nid, name],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("用户不存在或未禁用: {name}")));
        }
        tracing::info!(network, user = name, "用户启用");
        Ok(())
    }

    /// 改名: 名下节点与令牌按 user_id 关联, 属主标注随之传播 (无需迁移)。
    pub fn rename_user(&self, network: &str, name: &str, new_name: &str) -> Result<()> {
        if !valid_name(new_name) {
            return Err(Error::Usage("用户名仅允许 1-32 位字母数字与 - _".into()));
        }
        let (nid, _) = self.network_id(network)?;
        let n = self
            .conn
            .execute(
                "UPDATE users SET name = ?3 WHERE network_id = ?1 AND name = ?2",
                params![nid, name, new_name],
            )
            .map_err(|e| match &e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Error::Conflict(format!("用户已存在: 网络组 {network} 内 {new_name}"))
                }
                _ => Error::Store(e),
            })?;
        if n == 0 {
            return Err(Error::NotFound(format!("用户不存在: {name}")));
        }
        tracing::info!(network, from = name, to = new_name, "用户改名");
        Ok(())
    }

    /// 删除用户: 名下存在未移除节点时拒绝 (先移除节点); 历史令牌随用户清理,
    /// 已移除节点的属主引用置空 (标注退为 "-")。三步写包事务, 原子生效。
    pub fn delete_user(&self, network: &str, name: &str) -> Result<()> {
        let (nid, _) = self.network_id(network)?;
        let uid = match self.user_row(nid, name)? {
            Some((id, _)) => id,
            None => return Err(Error::NotFound(format!("用户不存在: {name}"))),
        };
        let active: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE user_id = ?1 AND removed_at IS NULL",
            params![uid],
            |r| r.get(0),
        )?;
        if active > 0 {
            return Err(Error::Conflict(format!(
                "用户 {name} 名下仍有 {active} 个未移除节点, 先移除节点再删除"
            )));
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM tokens WHERE user_id = ?1", params![uid])?;
        tx.execute(
            "UPDATE nodes SET user_id = NULL WHERE user_id = ?1",
            params![uid],
        )?;
        tx.execute("DELETE FROM users WHERE id = ?1", params![uid])?;
        tx.commit()?;
        tracing::info!(network, user = name, "用户删除");
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
                Some((_, true)) => return Err(Error::Conflict(format!("用户已禁用: {u}"))),
                None => return Err(Error::NotFound(format!("用户不存在: {u}"))),
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

    /// 校验卫星注册令牌: kind 必须为 relay 且未撤销未过期; 可复用 (FR-9), 不标记已用。
    /// 通过则返回令牌 id (核心注册表按令牌踢线定位用)。
    pub fn verify_relay_token(&self, plain: &str) -> Result<Option<i64>> {
        if token::kind_of(plain) != Some(Kind::RelayToken) || !token::looks_valid(plain) {
            return Ok(None);
        }
        let row = self
            .conn
            .query_row(
                "SELECT id, expires_at, revoked_at FROM tokens
                 WHERE token_hash = ?1 AND kind = 'relay'",
                params![hash_b64(plain)],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(match row {
            Some((id, expires, revoked)) if revoked.is_none() && expires > now() => Some(id),
            _ => None,
        })
    }

    /// 撤销令牌, 返回其 kind (调用方按 kind 决定联动动作, 如踢卫星线)。
    pub fn revoke_token(&self, id: i64) -> Result<String> {
        let kind: Option<String> = self
            .conn
            .query_row("SELECT kind FROM tokens WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .optional()?;
        let n = self.conn.execute(
            "UPDATE tokens SET revoked_at = ?2
             WHERE id = ?1 AND revoked_at IS NULL AND used_at IS NULL",
            params![id, now()],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("令牌不存在或已不可撤销: {id}")));
        }
        tracing::info!(token_id = id, "令牌撤销");
        Ok(kind.unwrap_or_default())
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
            tok.ok_or_else(|| Error::TokenRejected("令牌不存在或已失效".into()))?;
        if kind != "node" {
            return Err(Error::TokenRejected(
                "令牌类型不匹配, 期望节点注册令牌".into(),
            ));
        }
        if used.is_some() {
            return Err(Error::TokenRejected("令牌已被使用".into()));
        }
        if revoked.is_some() {
            return Err(Error::TokenRejected("令牌已撤销".into()));
        }
        if expires <= now() {
            return Err(Error::TokenRejected("令牌已过期".into()));
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
                Some(0) => {}
                _ => return Err(Error::Conflict("所属用户已禁用或不存在".into())),
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
                Error::Conflict(format!("节点名已存在: {name}"))
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
            return Err(Error::NotFound(format!("节点不存在: {name}")));
        }
        tracing::info!(network, node = name, "节点移除 (凭证随之失效)");
        Ok(())
    }

    /// 节点分页列表; network / owner 为 None 时不过滤 (owner 过滤下推 SQL,
    /// 保证分页语义: 先过滤后截页, 不在内存二次筛)。
    pub fn list_nodes(
        &self,
        network: Option<&str>,
        owner: Option<&str>,
        page: u32,
        size: u32,
    ) -> Result<Page<NodeInfo>> {
        const COND: &str = "n.removed_at IS NULL
             AND (?1 IS NULL OR nw.name = ?1)
             AND (?2 IS NULL OR u.name = ?2)";
        const BODY: &str = "FROM nodes n
             JOIN networks nw ON nw.id = n.network_id
             LEFT JOIN users u ON u.id = n.user_id
             WHERE";
        let total: u64 = self.conn.query_row(
            &format!("SELECT COUNT(*) {BODY} {COND}"),
            params![network, owner],
            |r| r.get(0),
        )?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT n.name, n.ip, u.name, n.created_at, nw.name, n.policy
             {BODY} {COND}
             ORDER BY n.id LIMIT ?3 OFFSET ?4"
        ))?;
        let rows = stmt
            .query_map(
                params![network, owner, size, offset_of(page, size)],
                node_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Page {
            items: rows,
            page,
            size,
            total,
        })
    }

    // ---- 信令认证与节点表 ----

    /// 信令挑战应答查询: 网络 + 节点名 → 凭证哈希与 IP。
    /// 不存在 / 已移除 / 属主已禁用统一返回 None, 不区分原因以免存在性探测。
    pub fn auth_lookup(&self, network: &str, name: &str) -> Result<Option<NodeAuth>> {
        Ok(self
            .conn
            .query_row(
                "SELECT n.credential_hash, n.ip
                 FROM nodes n
                 JOIN networks nw ON nw.id = n.network_id
                 LEFT JOIN users u ON u.id = n.user_id
                 WHERE nw.name = ?1 AND n.name = ?2 AND n.removed_at IS NULL
                   AND (n.user_id IS NULL OR u.disabled = 0)",
                params![network, name],
                |r| {
                    Ok(NodeAuth {
                        credential_hash_b64: r.get(0)?,
                        ip: address::ipv4_from_u32(r.get::<_, u32>(1)?).to_string(),
                    })
                },
            )
            .optional()?)
    }

    /// 节点级路径策略查询 (FR-10): 信令 State 初始化推送用。
    pub fn node_policy(&self, network: &str, name: &str) -> Result<Option<PathPolicy>> {
        Ok(self
            .conn
            .query_row(
                "SELECT n.policy
                 FROM nodes n
                 JOIN networks nw ON nw.id = n.network_id
                 WHERE nw.name = ?1 AND n.name = ?2 AND n.removed_at IS NULL",
                params![network, name],
                |r| {
                    let s: String = r.get(0)?;
                    Ok(s.parse().unwrap_or(PathPolicy::Auto))
                },
            )
            .optional()?)
    }

    /// 设置节点级路径策略; 目标不存在返回 false (由调用方决定报错口径)。
    pub fn set_node_policy(&self, network: &str, name: &str, policy: PathPolicy) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE nodes SET policy = ?3
             WHERE name = ?2 AND removed_at IS NULL
               AND network_id = (SELECT id FROM networks WHERE name = ?1)",
            params![network, name, policy.as_str()],
        )?;
        if n > 0 {
            tracing::info!(network, node = name, %policy, "节点路径策略变更");
        }
        Ok(n > 0)
    }

    /// 信令节点表: 同网全部活跃节点; 在线态与候选地址由调用方从注册表合并。
    pub fn signal_network_nodes(&self, network: &str) -> Result<Vec<SignalPeer>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.name, n.ip, n.wg_public_key
             FROM nodes n
             JOIN networks nw ON nw.id = n.network_id
             WHERE nw.name = ?1 AND n.removed_at IS NULL
             ORDER BY n.id",
        )?;
        let rows = stmt
            .query_map(params![network], |r| {
                Ok(SignalPeer {
                    name: r.get(0)?,
                    ip: address::ipv4_from_u32(r.get::<_, u32>(1)?).to_string(),
                    wg_public_key: r.get(2)?,
                    online: false,
                    candidates: Vec::new(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 属主名下的活跃节点名 (禁用用户时逐个踢下线用)。
    pub fn node_names_of_user(&self, network: &str, user: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.name
             FROM nodes n
             JOIN networks nw ON nw.id = n.network_id
             JOIN users u ON u.id = n.user_id
             WHERE nw.name = ?1 AND u.name = ?2 AND n.removed_at IS NULL
             ORDER BY n.id",
        )?;
        let rows = stmt
            .query_map(params![network, user], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
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
    use kunxtier_core::protocol::PAGE_SIZE_MAX;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("state.db")).unwrap();
        (dir, s)
    }

    #[test]
    fn 目录缺失时给出初始化指引而非裸错误() {
        let dir = tempfile::tempdir().unwrap();
        let err = match Store::open(&dir.path().join("missing/state.db")) {
            Err(e) => e,
            Ok(_) => panic!("目录缺失时应报错"),
        };
        let msg = err.to_string();
        assert!(msg.contains("数据目录不存在"), "实际: {msg}");
        assert!(msg.contains("relay --init"), "实际: {msg}");
    }

    #[test]
    fn 连接启用wal与有界写锁等待() {
        let (_dir, s) = store();
        let mode: String = s
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal", "journal_mode 应为 wal");
        let timeout: i64 = s
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 5000, "busy_timeout 应为 5000ms");
    }

    #[test]
    fn 重名创建映射为业务错误而非内部错误() {
        // 唯一约束冲突出口为 Conflict (409 业务提示), 不落 500 内部错误
        let (_d, s) = store();
        default_network(&s);
        s.create_user("default", "alice").unwrap();

        let err = s.create_user("default", "alice").unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "实际: {err:?}");
        assert!(err.to_string().contains("用户已存在"), "实际: {err}");

        let err = s
            .create_network("default", address::nth_network_base(0).unwrap())
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "实际: {err:?}");
        assert!(err.to_string().contains("网络组已存在"), "实际: {err}");
    }

    fn default_network(s: &Store) {
        s.create_network("default", address::nth_network_base(0).unwrap())
            .unwrap();
    }

    fn wg_key() -> String {
        STANDARD.encode([7u8; 32])
    }

    #[test]
    fn 用户列表_跨网络与禁用状态() {
        let (_d, s) = store();
        default_network(&s);
        s.create_network("net2", address::nth_network_base(1).unwrap())
            .unwrap();
        s.create_user("default", "alice").unwrap();
        s.create_user("net2", "bob").unwrap();
        s.disable_user("net2", "bob").unwrap();

        let all = s.list_users(None, 1, PAGE_SIZE_MAX).unwrap().items;
        assert_eq!(all.len(), 2);
        let bob = all.iter().find(|u| u.name == "bob").unwrap();
        assert_eq!(bob.network, "net2");
        assert!(bob.disabled);

        let only = s
            .list_users(Some("default"), 1, PAGE_SIZE_MAX)
            .unwrap()
            .items;
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].name, "alice");
        assert!(!only[0].disabled);
    }

    #[test]
    fn 用户操作_启用改名删除() {
        let (_d, s) = store();
        default_network(&s);
        s.create_user("default", "alice").unwrap();

        // 禁用 / 启用往返
        s.disable_user("default", "alice").unwrap();
        s.enable_user("default", "alice").unwrap();
        assert!(
            !s.list_users(Some("default"), 1, PAGE_SIZE_MAX)
                .unwrap()
                .items[0]
                .disabled
        );

        // 改名: 冲突拒绝, 成功后节点属主标注随之传播
        s.create_user("default", "bob").unwrap();
        let err = s.rename_user("default", "bob", "alice").unwrap_err();
        assert!(err.to_string().contains("用户已存在"), "实际: {err}");
        let (_id, t) = s
            .issue_token("default", Some("alice"), "node", 3600)
            .unwrap();
        s.consume_node_token(&t, "laptop", &wg_key()).unwrap();
        s.rename_user("default", "alice", "alice2").unwrap();
        let nodes = s
            .list_nodes(Some("default"), None, 1, PAGE_SIZE_MAX)
            .unwrap()
            .items;
        assert_eq!(nodes[0].owner.as_deref(), Some("alice2"));

        // 名下有未移除节点时拒绝删除; 移除节点后可删, 用户从列表消失
        let err = s.delete_user("default", "alice2").unwrap_err();
        assert!(err.to_string().contains("未移除节点"), "实际: {err}");
        s.remove_node("default", "laptop").unwrap();
        s.delete_user("default", "alice2").unwrap();
        let users = s
            .list_users(Some("default"), 1, PAGE_SIZE_MAX)
            .unwrap()
            .items;
        assert!(users.iter().all(|u| u.name != "alice2"));
    }

    #[test]
    fn register_flow_token_single_use() {
        let (_d, s) = store();
        default_network(&s);
        s.create_user("default", "alice").unwrap();
        let (_id, token) = s
            .issue_token("default", Some("alice"), "node", 3600)
            .unwrap();
        let (network, ip, cred) = s.consume_node_token(&token, "laptop", &wg_key()).unwrap();
        assert_eq!(network, "default");
        assert_eq!(ip, "100.64.0.1");
        assert!(cred.starts_with("kxt_nid_"));
        // 一次性: 二次消费被拒
        assert!(s.consume_node_token(&token, "laptop2", &wg_key()).is_err());
    }

    #[test]
    fn node_policy_roundtrip_and_unknown_node() {
        let (_d, s) = store();
        default_network(&s);
        s.create_user("default", "alice").unwrap();
        let (_id, t) = s
            .issue_token("default", Some("alice"), "node", 3600)
            .unwrap();
        s.consume_node_token(&t, "laptop", &wg_key()).unwrap();

        // 缺省 auto; 设置 / 读取 / 再切回
        assert_eq!(
            s.node_policy("default", "laptop").unwrap(),
            Some(PathPolicy::Auto)
        );
        assert!(s
            .set_node_policy("default", "laptop", PathPolicy::ForceRelay)
            .unwrap());
        assert_eq!(
            s.node_policy("default", "laptop").unwrap(),
            Some(PathPolicy::ForceRelay)
        );
        assert_eq!(
            s.list_nodes(Some("default"), None, 1, PAGE_SIZE_MAX)
                .unwrap()
                .items[0]
                .policy,
            PathPolicy::ForceRelay
        );
        assert!(s
            .set_node_policy("default", "laptop", PathPolicy::Auto)
            .unwrap());
        assert_eq!(
            s.node_policy("default", "laptop").unwrap(),
            Some(PathPolicy::Auto)
        );

        // 不存在的节点: 拒绝设置, 查询为空
        assert!(!s
            .set_node_policy("default", "ghost", PathPolicy::ForceRelay)
            .unwrap());
        assert_eq!(s.node_policy("default", "ghost").unwrap(), None);
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
    fn relay_token_verify_reusable_and_rejects() {
        let (_d, s) = store();
        default_network(&s);
        let (id, rt) = s.issue_token("default", None, "relay", 60).unwrap();
        // 可复用: 多次校验均通过且返回同一 id
        assert_eq!(s.verify_relay_token(&rt).unwrap(), Some(id));
        assert_eq!(s.verify_relay_token(&rt).unwrap(), Some(id));
        // node 令牌与非法串不能通过卫星校验
        s.create_user("default", "alice").unwrap();
        let (_, nt) = s.issue_token("default", Some("alice"), "node", 60).unwrap();
        assert_eq!(s.verify_relay_token(&nt).unwrap(), None);
        assert_eq!(s.verify_relay_token("kxt_relay_short").unwrap(), None);
        // 撤销后立即失效
        assert_eq!(s.revoke_token(id).unwrap(), "relay");
        assert_eq!(s.verify_relay_token(&rt).unwrap(), None);
    }

    #[test]
    fn relay_token_expired_is_rejected() {
        let (_d, s) = store();
        default_network(&s);
        let (_, rt) = s.issue_token("default", None, "relay", 0).unwrap();
        assert_eq!(s.verify_relay_token(&rt).unwrap(), None);
    }

    #[test]
    fn remove_node_and_name_reuse() {
        let (_d, s) = store();
        default_network(&s);
        s.create_user("default", "a").unwrap();
        let (_, t) = s.issue_token("default", Some("a"), "node", 3600).unwrap();
        s.consume_node_token(&t, "laptop", &wg_key()).unwrap();
        s.remove_node("default", "laptop").unwrap();
        assert!(s
            .list_nodes(Some("default"), None, 1, PAGE_SIZE_MAX)
            .unwrap()
            .items
            .is_empty());
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
        assert_eq!(
            restored
                .list_networks(1, PAGE_SIZE_MAX)
                .unwrap()
                .items
                .len(),
            1
        );
        assert_eq!(
            restored
                .list_nodes(None, None, 1, PAGE_SIZE_MAX)
                .unwrap()
                .items
                .len(),
            1,
        );
    }

    #[test]
    fn auth_lookup_rejects_removed_and_disabled() {
        let (_d, s) = store();
        default_network(&s);
        s.create_user("default", "a").unwrap();
        let (_, t) = s.issue_token("default", Some("a"), "node", 3600).unwrap();
        let (_, _, _) = s.consume_node_token(&t, "n1", &wg_key()).unwrap();
        // 活跃节点可查
        let auth = s.auth_lookup("default", "n1").unwrap().unwrap();
        assert!(auth.credential_hash_b64.len() >= 44);
        assert_eq!(auth.ip, "100.64.0.1");
        // 未知网络 / 未知节点 → None
        assert!(s.auth_lookup("nope", "n1").unwrap().is_none());
        assert!(s.auth_lookup("default", "nope").unwrap().is_none());
        // 属主禁用 → None (存量凭证随信令实时吊销)
        s.disable_user("default", "a").unwrap();
        assert!(s.auth_lookup("default", "n1").unwrap().is_none());
        // 节点移除 → None
        s.create_user("default", "c").unwrap();
        let (_, t2) = s.issue_token("default", Some("c"), "node", 3600).unwrap();
        s.consume_node_token(&t2, "n2", &wg_key()).unwrap();
        s.remove_node("default", "n2").unwrap();
        assert!(s.auth_lookup("default", "n2").unwrap().is_none());
    }

    #[test]
    fn signal_nodes_table_scopes_to_network() {
        let (_d, s) = store();
        default_network(&s);
        s.create_network("net2", address::nth_network_base(1).unwrap())
            .unwrap();
        s.create_user("default", "a").unwrap();
        s.create_user("net2", "b").unwrap();
        let (_, t1) = s.issue_token("default", Some("a"), "node", 60).unwrap();
        let (_, t2) = s.issue_token("net2", Some("b"), "node", 60).unwrap();
        s.consume_node_token(&t1, "n1", &wg_key()).unwrap();
        s.consume_node_token(&t2, "n2", &wg_key()).unwrap();
        let peers = s.signal_network_nodes("default").unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].name, "n1");
        assert_eq!(peers[0].wg_public_key, wg_key());
        assert!(!peers[0].online);
        // 移除后不再出现在节点表
        s.remove_node("default", "n1").unwrap();
        assert!(s.signal_network_nodes("default").unwrap().is_empty());
    }

    #[test]
    fn node_names_of_user_filters_owner() {
        let (_d, s) = store();
        default_network(&s);
        s.create_user("default", "a").unwrap();
        s.create_user("default", "b").unwrap();
        let (_, t1) = s.issue_token("default", Some("a"), "node", 60).unwrap();
        let (_, t1b) = s.issue_token("default", Some("a"), "node", 60).unwrap();
        let (_, t2) = s.issue_token("default", Some("b"), "node", 60).unwrap();
        s.consume_node_token(&t1, "n1", &wg_key()).unwrap();
        s.consume_node_token(&t1b, "n2", &wg_key()).unwrap();
        s.consume_node_token(&t2, "other", &wg_key()).unwrap();
        assert_eq!(
            s.node_names_of_user("default", "a").unwrap(),
            vec!["n1", "n2"]
        );
        assert!(s.node_names_of_user("default", "ghost").unwrap().is_empty());
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
        assert_eq!(v, SCHEMA_VERSION.to_string());
    }
}
