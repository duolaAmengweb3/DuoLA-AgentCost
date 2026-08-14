use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use crate::transform::Receipt;

fn ensure_column(conn: &Connection, table: &str, column: &str, alter_sql: &str) -> Result<()> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    let exists = columns
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == column);
    if !exists {
        conn.execute(alter_sql, [])?;
    }
    Ok(())
}

#[derive(Clone)]
pub struct Ledger {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub requests: i64,
    pub successful_requests: i64,
    pub failed_requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub measured_input_tokens: i64,
    pub cached_input_tokens: i64,
    pub measured_requests: i64,
    pub estimated_input_tokens: i64,
    pub saved_input_tokens: i64,
    pub transformed_bytes: i64,
    pub original_input_tokens: i64,
    pub sent_input_tokens: i64,
    pub transformed_requests: i64,
    pub pass_through_requests: i64,
    pub blocked_requests: i64,
    pub total_cost: f64,
    pub applied_rules: i64,
    pub cache_hit_requests: i64,
    pub cache_saved_input_tokens: i64,
    pub task_count: i64,
    pub pass_through_bytes: i64,
    pub applied_requests: i64,
    pub no_gain_requests: i64,
    pub bypassed_requests: i64,
    pub semantic_guard_fallbacks: i64,
    pub provider_error_requests: i64,
}

#[derive(Debug, Clone, Default)]
pub struct RequestMeta {
    pub session_id: String,
    pub project_id: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub transform_status: String,
    pub transform_rule_count: i64,
    pub original_hash: Option<String>,
    pub sent_hash: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecentRequest {
    pub id: String,
    pub provider: String,
    pub path: String,
    pub status: String,
    pub input_bytes: i64,
    pub sent_bytes: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub measured_input_tokens: Option<i64>,
    pub cached_input_tokens: i64,
    pub usage_estimated: bool,
    pub saved_input_tokens: i64,
    pub cost: f64,
    pub latency_ms: i64,
    pub created_at: i64,
    pub session_id: String,
    pub project_id: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub transform_status: String,
    pub transform_rule_count: i64,
    pub original_hash: Option<String>,
    pub sent_hash: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskSummary {
    pub session_id: String,
    pub project_id: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub requests: i64,
    pub completed_requests: i64,
    pub blocked_requests: i64,
    pub input_tokens: i64,
    pub sent_tokens: i64,
    pub measured_input_tokens: i64,
    pub output_tokens: i64,
    pub saved_input_tokens: i64,
    pub total_cost: f64,
    pub last_seen: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderAttempt {
    pub request_id: String,
    pub attempt: i64,
    pub provider: String,
    pub status: String,
    pub error: Option<String>,
    pub latency_ms: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceiptRecord {
    pub request_id: String,
    pub path: String,
    pub rule_id: String,
    pub original_hash: String,
    pub result_hash: String,
    pub original_bytes: i64,
    pub result_bytes: i64,
    pub status: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ControlEvent {
    pub action: String,
    pub enabled: bool,
    pub reason: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrendPoint {
    pub day: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub sent_tokens: i64,
    pub saved_input_tokens: i64,
    pub measured_input_tokens: i64,
    pub cost: f64,
    pub transformed_requests: i64,
    pub blocked_requests: i64,
}

impl Ledger {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            crate::config::AppConfig::ensure_data_dir(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS requests (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                path TEXT NOT NULL,
                status TEXT NOT NULL,
                input_bytes INTEGER NOT NULL,
                sent_bytes INTEGER NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                measured_input_tokens INTEGER,
                cached_input_tokens INTEGER NOT NULL DEFAULT 0,
                usage_estimated INTEGER NOT NULL DEFAULT 1,
                cost REAL NOT NULL DEFAULT 0,
                latency_ms INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                session_id TEXT NOT NULL DEFAULT 'default',
                project_id TEXT,
                agent TEXT,
                model TEXT,
                transform_status TEXT NOT NULL DEFAULT 'pass_through',
                transform_rule_count INTEGER NOT NULL DEFAULT 0,
                original_hash TEXT,
                sent_hash TEXT,
                reason TEXT
            );
            CREATE TABLE IF NOT EXISTS transformation_receipts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                request_id TEXT NOT NULL,
                block_path TEXT NOT NULL,
                rule_id TEXT NOT NULL,
                original_hash TEXT NOT NULL,
                result_hash TEXT NOT NULL,
                original_bytes INTEGER NOT NULL,
                result_bytes INTEGER NOT NULL,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_requests_created_at ON requests(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_receipts_request_id ON transformation_receipts(request_id);
            CREATE TABLE IF NOT EXISTS provider_attempts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                request_id TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                provider TEXT NOT NULL,
                status TEXT NOT NULL,
                error TEXT,
                latency_ms INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_attempts_request_id ON provider_attempts(request_id, attempt);
            CREATE TABLE IF NOT EXISTS control_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action TEXT NOT NULL,
                enabled INTEGER NOT NULL,
                reason TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_control_events_created_at ON control_events(created_at DESC);
            "#,
        )?;
        ensure_column(
            &conn,
            "requests",
            "measured_input_tokens",
            "ALTER TABLE requests ADD COLUMN measured_input_tokens INTEGER",
        )?;
        ensure_column(
            &conn,
            "requests",
            "cached_input_tokens",
            "ALTER TABLE requests ADD COLUMN cached_input_tokens INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            &conn,
            "requests",
            "usage_estimated",
            "ALTER TABLE requests ADD COLUMN usage_estimated INTEGER NOT NULL DEFAULT 1",
        )?;
        for (column, sql) in [
            (
                "session_id",
                "ALTER TABLE requests ADD COLUMN session_id TEXT NOT NULL DEFAULT 'default'",
            ),
            (
                "project_id",
                "ALTER TABLE requests ADD COLUMN project_id TEXT",
            ),
            ("agent", "ALTER TABLE requests ADD COLUMN agent TEXT"),
            ("model", "ALTER TABLE requests ADD COLUMN model TEXT"),
            (
                "transform_status",
                "ALTER TABLE requests ADD COLUMN transform_status TEXT NOT NULL DEFAULT 'pass_through'",
            ),
            (
                "transform_rule_count",
                "ALTER TABLE requests ADD COLUMN transform_rule_count INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "original_hash",
                "ALTER TABLE requests ADD COLUMN original_hash TEXT",
            ),
            (
                "sent_hash",
                "ALTER TABLE requests ADD COLUMN sent_hash TEXT",
            ),
            ("reason", "ALTER TABLE requests ADD COLUMN reason TEXT"),
        ] {
            ensure_column(&conn, "requests", column, sql)?;
        }
        // A process can be terminated while a request or provider attempt is
        // running. Mark those rows explicitly on the next startup so the
        // dashboard never presents stale work as still active.
        conn.execute(
            "UPDATE requests SET status='interrupted' WHERE status='running'",
            [],
        )?;
        conn.execute(
            "UPDATE provider_attempts SET status='interrupted', error=COALESCE(error, 'gateway restarted') WHERE status='running'",
            [],
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_read_only(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.pragma_update(None, "query_only", true)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an existing ledger for explicit maintenance without converting
    /// in-flight requests to `interrupted`. Used only by user-invoked purge.
    pub fn open_maintenance(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS control_events (id INTEGER PRIMARY KEY AUTOINCREMENT, action TEXT NOT NULL, enabled INTEGER NOT NULL, reason TEXT NOT NULL, created_at INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS idx_control_events_created_at ON control_events(created_at DESC);",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn record_start(
        &self,
        id: &str,
        provider: &str,
        path: &str,
        input_bytes: i64,
        sent_bytes: i64,
        input_tokens: i64,
    ) -> Result<()> {
        self.record_start_with_meta(
            id,
            provider,
            path,
            input_bytes,
            sent_bytes,
            input_tokens,
            &RequestMeta::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_start_with_meta(
        &self,
        id: &str,
        provider: &str,
        path: &str,
        input_bytes: i64,
        sent_bytes: i64,
        input_tokens: i64,
        meta: &RequestMeta,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        conn.execute(
            "INSERT INTO requests (id, provider, path, status, input_bytes, sent_bytes, input_tokens, created_at, session_id, project_id, agent, model, transform_status, transform_rule_count, original_hash, sent_hash, reason) VALUES (?1, ?2, ?3, 'running', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                id,
                provider,
                path,
                input_bytes,
                sent_bytes,
                input_tokens,
                Utc::now().timestamp(),
                if meta.session_id.is_empty() { "default" } else { &meta.session_id },
                meta.project_id,
                meta.agent,
                meta.model,
                if meta.transform_status.is_empty() { "pass_through" } else { &meta.transform_status },
                meta.transform_rule_count,
                meta.original_hash,
                meta.sent_hash,
                meta.reason,
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_blocked(
        &self,
        id: &str,
        provider: &str,
        path: &str,
        input_bytes: i64,
        sent_bytes: i64,
        input_tokens: i64,
        status: &str,
    ) -> Result<()> {
        self.record_blocked_with_meta(
            id,
            provider,
            path,
            input_bytes,
            sent_bytes,
            input_tokens,
            status,
            &RequestMeta {
                reason: Some(status.to_owned()),
                ..RequestMeta::default()
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_blocked_with_meta(
        &self,
        id: &str,
        provider: &str,
        path: &str,
        input_bytes: i64,
        sent_bytes: i64,
        input_tokens: i64,
        status: &str,
        meta: &RequestMeta,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        conn.execute(
            "INSERT OR REPLACE INTO requests (id, provider, path, status, input_bytes, sent_bytes, input_tokens, created_at, session_id, project_id, agent, model, transform_status, transform_rule_count, original_hash, sent_hash, reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                id,
                provider,
                path,
                status,
                input_bytes,
                sent_bytes,
                input_tokens,
                Utc::now().timestamp(),
                if meta.session_id.is_empty() { "default" } else { &meta.session_id },
                meta.project_id,
                meta.agent,
                meta.model,
                if meta.transform_status.is_empty() { "blocked" } else { &meta.transform_status },
                meta.transform_rule_count,
                meta.original_hash,
                meta.sent_hash,
                meta.reason.as_deref().or(Some(status)),
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_cache_hit(
        &self,
        id: &str,
        provider: &str,
        path: &str,
        input_bytes: i64,
        sent_bytes: i64,
        input_tokens: i64,
        output_tokens: i64,
        latency_ms: i64,
    ) -> Result<()> {
        self.record_cache_hit_with_meta(
            id,
            provider,
            path,
            input_bytes,
            sent_bytes,
            input_tokens,
            output_tokens,
            latency_ms,
            &RequestMeta::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_cache_hit_with_meta(
        &self,
        id: &str,
        provider: &str,
        path: &str,
        input_bytes: i64,
        sent_bytes: i64,
        input_tokens: i64,
        output_tokens: i64,
        latency_ms: i64,
        meta: &RequestMeta,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        conn.execute(
            "INSERT INTO requests (id, provider, path, status, input_bytes, sent_bytes, input_tokens, output_tokens, usage_estimated, cost, latency_ms, created_at, session_id, project_id, agent, model, transform_status, transform_rule_count, original_hash, sent_hash, reason) VALUES (?1, ?2, ?3, 'cache_hit', ?4, ?5, ?6, ?7, 1, 0, ?8, ?9, ?10, ?11, ?12, ?13, 'cache_hit', ?14, ?15, ?16, 'exact cache hit')",
            params![
                id,
                provider,
                path,
                input_bytes,
                sent_bytes,
                input_tokens,
                output_tokens,
                latency_ms,
                Utc::now().timestamp(),
                if meta.session_id.is_empty() { "default" } else { &meta.session_id },
                meta.project_id,
                meta.agent,
                meta.model,
                meta.transform_rule_count,
                meta.original_hash,
                meta.sent_hash,
            ],
        )?;
        Ok(())
    }

    pub fn record_attempt_start(
        &self,
        request_id: &str,
        attempt: i64,
        provider: &str,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        conn.execute(
            "INSERT INTO provider_attempts (request_id, attempt, provider, status, created_at) VALUES (?1, ?2, ?3, 'running', ?4)",
            params![request_id, attempt, provider, Utc::now().timestamp()],
        )?;
        Ok(())
    }

    pub fn record_control_event(&self, action: &str, enabled: bool, reason: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        conn.execute(
            "INSERT INTO control_events (action, enabled, reason, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![action, enabled, reason, Utc::now().timestamp()],
        )?;
        Ok(())
    }

    pub fn control_events(&self, limit: i64) -> Result<Vec<ControlEvent>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        let mut statement = conn.prepare(
            "SELECT action, enabled, reason, created_at FROM control_events ORDER BY created_at DESC, id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit], |row| {
            Ok(ControlEvent {
                action: row.get(0)?,
                enabled: row.get::<_, i64>(1)? != 0,
                reason: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn record_attempt_finish(
        &self,
        request_id: &str,
        attempt: i64,
        status: &str,
        error: Option<&str>,
        latency_ms: i64,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        conn.execute(
            "UPDATE provider_attempts SET status=?3, error=?4, latency_ms=?5 WHERE request_id=?1 AND attempt=?2",
            params![request_id, attempt, status, error, latency_ms],
        )?;
        Ok(())
    }

    pub fn record_receipts(&self, request_id: &str, receipts: &[Receipt]) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        let tx = conn.unchecked_transaction()?;
        for receipt in receipts {
            tx.execute(
                "INSERT INTO transformation_receipts (request_id, block_path, rule_id, original_hash, result_hash, original_bytes, result_bytes, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    request_id,
                    receipt.path,
                    receipt.rule_id,
                    receipt.original_hash,
                    receipt.result_hash,
                    receipt.original_bytes as i64,
                    receipt.result_bytes as i64,
                    receipt.status,
                    Utc::now().timestamp()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_finish(
        &self,
        id: &str,
        status: &str,
        measured_input_tokens: Option<i64>,
        output_tokens: i64,
        cached_input_tokens: i64,
        cost: f64,
        latency_ms: i64,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        conn.execute(
            "UPDATE requests SET status=?2, measured_input_tokens=COALESCE(?3, measured_input_tokens), output_tokens=?4, cached_input_tokens=?5, usage_estimated=CASE WHEN ?3 IS NULL THEN usage_estimated ELSE 0 END, cost=?6, latency_ms=?7 WHERE id=?1",
            params![
                id,
                status,
                measured_input_tokens,
                output_tokens,
                cached_input_tokens,
                cost,
                latency_ms
            ],
        )?;
        Ok(())
    }

    pub fn set_provider(&self, id: &str, provider: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        conn.execute(
            "UPDATE requests SET provider=?2 WHERE id=?1",
            params![id, provider],
        )?;
        Ok(())
    }

    pub fn attempts(&self, request_id: &str) -> Result<Vec<ProviderAttempt>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        let mut stmt = conn.prepare(
            "SELECT request_id, attempt, provider, status, error, latency_ms, created_at FROM provider_attempts WHERE request_id=?1 ORDER BY attempt ASC",
        )?;
        let rows = stmt.query_map(params![request_id], |r| {
            Ok(ProviderAttempt {
                request_id: r.get(0)?,
                attempt: r.get(1)?,
                provider: r.get(2)?,
                status: r.get(3)?,
                error: r.get(4)?,
                latency_ms: r.get(5)?,
                created_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn receipts(&self, request_id: &str) -> Result<Vec<ReceiptRecord>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        let mut stmt = conn.prepare(
            "SELECT request_id, block_path, rule_id, original_hash, result_hash, original_bytes, result_bytes, status, created_at FROM transformation_receipts WHERE request_id=?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![request_id], |r| {
            Ok(ReceiptRecord {
                request_id: r.get(0)?,
                path: r.get(1)?,
                rule_id: r.get(2)?,
                original_hash: r.get(3)?,
                result_hash: r.get(4)?,
                original_bytes: r.get(5)?,
                result_bytes: r.get(6)?,
                status: r.get(7)?,
                created_at: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn request_exists(&self, request_id: &str) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM requests WHERE id=?1)",
            params![request_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub fn cost_since(&self, since: i64) -> Result<f64> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        conn.query_row(
            "SELECT COALESCE(SUM(cost), 0) FROM requests WHERE created_at >= ?1",
            params![since],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub fn cost_since_scope(&self, since: i64, scope: Option<&str>) -> Result<f64> {
        let Some((kind, value)) = scope.and_then(|scope| scope.split_once(':')) else {
            return self.cost_since(since);
        };
        let column = match kind {
            "project" => "project_id",
            "agent" => "agent",
            "session" => "session_id",
            "model" => "model",
            _ => return self.cost_since(since),
        };
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        let sql = format!(
            "SELECT COALESCE(SUM(cost), 0) FROM requests WHERE created_at >= ?1 AND {column} = ?2"
        );
        conn.query_row(&sql, params![since, value], |row| row.get(0))
            .map_err(Into::into)
    }

    pub fn input_tokens_since(&self, since: i64) -> Result<i64> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        conn.query_row(
            "SELECT COALESCE(SUM(input_tokens), 0) FROM requests WHERE created_at >= ?1",
            params![since],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub fn input_tokens_since_scope(&self, since: i64, scope: Option<&str>) -> Result<i64> {
        let Some((kind, value)) = scope.and_then(|scope| scope.split_once(':')) else {
            return self.input_tokens_since(since);
        };
        let column = match kind {
            "project" => "project_id",
            "agent" => "agent",
            "session" => "session_id",
            "model" => "model",
            _ => return self.input_tokens_since(since),
        };
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        let sql = format!(
            "SELECT COALESCE(SUM(input_tokens), 0) FROM requests WHERE created_at >= ?1 AND {column} = ?2"
        );
        conn.query_row(&sql, params![since, value], |row| row.get(0))
            .map_err(Into::into)
    }

    pub fn stats(&self) -> Result<Stats> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        let row = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN status IN ('completed','cache_hit') THEN 1 ELSE 0 END),0), COALESCE(SUM(CASE WHEN status NOT IN ('completed','cache_hit','running') THEN 1 ELSE 0 END),0), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(measured_input_tokens),0), COALESCE(SUM(cached_input_tokens),0), COALESCE(SUM(CASE WHEN usage_estimated=0 THEN 1 ELSE 0 END),0), COALESCE(SUM(CASE WHEN input_bytes > sent_bytes THEN (input_bytes - sent_bytes) / 4 ELSE 0 END),0), COALESCE(SUM(CASE WHEN input_bytes > sent_bytes THEN input_bytes - sent_bytes ELSE 0 END),0), COALESCE(SUM(input_bytes) / 4, 0), COALESCE(SUM(sent_bytes) / 4, 0), COALESCE(SUM(CASE WHEN input_bytes > sent_bytes THEN 1 ELSE 0 END),0), COALESCE(SUM(CASE WHEN input_bytes <= sent_bytes THEN 1 ELSE 0 END),0), COALESCE(SUM(CASE WHEN status IN ('budget_blocked','loop_blocked','rate_limited','concurrency_blocked') THEN 1 ELSE 0 END),0), COALESCE(SUM(cost),0) FROM requests",
            [],
            |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, i64>(9)?,
                r.get::<_, i64>(10)?,
                r.get::<_, i64>(11)?,
                r.get::<_, i64>(12)?,
                r.get::<_, i64>(13)?,
                r.get::<_, i64>(14)?,
                r.get::<_, f64>(15)?,
            )),
        )?;
        let applied_rules = conn.query_row(
            "SELECT COUNT(*) FROM transformation_receipts WHERE status='applied'",
            [],
            |r| r.get(0),
        )?;
        let cache_hit_requests = conn.query_row(
            "SELECT COUNT(*) FROM requests WHERE status='cache_hit'",
            [],
            |r| r.get(0),
        )?;
        let cache_saved_input_tokens = conn.query_row(
            "SELECT COALESCE(SUM(input_tokens), 0) FROM requests WHERE status='cache_hit'",
            [],
            |r| r.get(0),
        )?;
        let estimated_input_tokens = conn.query_row(
            "SELECT COALESCE(SUM(CASE WHEN usage_estimated = 1 THEN input_tokens ELSE 0 END), 0) FROM requests",
            [],
            |r| r.get(0),
        )?;
        let task_count =
            conn.query_row("SELECT COUNT(DISTINCT session_id) FROM requests", [], |r| {
                r.get(0)
            })?;
        let pass_through_bytes = conn.query_row(
            "SELECT COALESCE(SUM(CASE WHEN transform_status='pass_through' THEN input_bytes ELSE 0 END), 0) FROM requests",
            [],
            |r| r.get(0),
        )?;
        let applied_requests = conn.query_row(
            "SELECT COUNT(*) FROM requests WHERE transform_status='applied'",
            [],
            |r| r.get(0),
        )?;
        let no_gain_requests = conn.query_row(
            "SELECT COUNT(*) FROM requests WHERE transform_status='pass_through' AND (reason LIKE '%没有可安全%' OR reason LIKE '%没有变短%')",
            [],
            |r| r.get(0),
        )?;
        let bypassed_requests = conn.query_row(
            "SELECT COUNT(*) FROM requests WHERE transform_status='bypassed'",
            [],
            |r| r.get(0),
        )?;
        let semantic_guard_fallbacks = conn.query_row(
            "SELECT COUNT(*) FROM requests WHERE reason LIKE '%语义护栏%'",
            [],
            |r| r.get(0),
        )?;
        let provider_error_requests = conn.query_row(
            "SELECT COUNT(*) FROM requests WHERE status IN ('provider_error','upstream_error','response_error','response_idle_timeout','stream_idle_timeout','response_too_large')",
            [],
            |r| r.get(0),
        )?;
        Ok(Stats {
            requests: row.0,
            successful_requests: row.1,
            failed_requests: row.2,
            input_tokens: row.3,
            output_tokens: row.4,
            measured_input_tokens: row.5,
            cached_input_tokens: row.6,
            measured_requests: row.7,
            estimated_input_tokens,
            saved_input_tokens: row.8,
            transformed_bytes: row.9,
            original_input_tokens: row.10,
            sent_input_tokens: row.11,
            transformed_requests: row.12,
            pass_through_requests: row.13,
            blocked_requests: row.14,
            total_cost: row.15,
            applied_rules,
            cache_hit_requests,
            cache_saved_input_tokens,
            task_count,
            pass_through_bytes,
            applied_requests,
            no_gain_requests,
            bypassed_requests,
            semantic_guard_fallbacks,
            provider_error_requests,
        })
    }

    pub fn recent(&self, limit: i64) -> Result<Vec<RecentRequest>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        let mut stmt = conn.prepare(
            "SELECT id, provider, path, status, input_bytes, sent_bytes, input_tokens, output_tokens, measured_input_tokens, cached_input_tokens, usage_estimated, cost, latency_ms, created_at, session_id, project_id, agent, model, transform_status, transform_rule_count, original_hash, sent_hash, reason FROM requests ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(RecentRequest {
                id: r.get(0)?,
                provider: r.get(1)?,
                path: r.get(2)?,
                status: r.get(3)?,
                input_bytes: r.get(4)?,
                sent_bytes: r.get(5)?,
                input_tokens: r.get(6)?,
                output_tokens: r.get(7)?,
                measured_input_tokens: r.get(8)?,
                cached_input_tokens: r.get(9)?,
                usage_estimated: r.get::<_, i64>(10)? != 0,
                saved_input_tokens: ((r.get::<_, i64>(4)? - r.get::<_, i64>(5)?).max(0)) / 4,
                cost: r.get(11)?,
                latency_ms: r.get(12)?,
                created_at: r.get(13)?,
                session_id: r.get(14)?,
                project_id: r.get(15)?,
                agent: r.get(16)?,
                model: r.get(17)?,
                transform_status: r.get(18)?,
                transform_rule_count: r.get(19)?,
                original_hash: r.get(20)?,
                sent_hash: r.get(21)?,
                reason: r.get(22)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn tasks(&self, limit: i64) -> Result<Vec<TaskSummary>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        let mut stmt = conn.prepare(
            "SELECT session_id, MAX(project_id), MAX(agent), MAX(model), COUNT(*), SUM(CASE WHEN status IN ('completed','cache_hit') THEN 1 ELSE 0 END), SUM(CASE WHEN status IN ('budget_blocked','loop_blocked','rate_limited','concurrency_blocked') THEN 1 ELSE 0 END), COALESCE(SUM(input_tokens),0), COALESCE(SUM(sent_bytes)/4,0), COALESCE(SUM(measured_input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(CASE WHEN input_bytes > sent_bytes THEN (input_bytes-sent_bytes)/4 ELSE 0 END),0), COALESCE(SUM(cost),0), MAX(created_at), CASE WHEN SUM(CASE WHEN status IN ('failed','provider_error','upstream_error','response_error','response_idle_timeout','stream_idle_timeout','response_too_large','interrupted') THEN 1 ELSE 0 END) > 0 THEN 'attention' WHEN SUM(CASE WHEN status='running' THEN 1 ELSE 0 END) > 0 THEN 'running' ELSE 'completed' END FROM requests GROUP BY session_id ORDER BY MAX(created_at) DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(TaskSummary {
                session_id: r.get(0)?,
                project_id: r.get(1)?,
                agent: r.get(2)?,
                model: r.get(3)?,
                requests: r.get(4)?,
                completed_requests: r.get(5)?,
                blocked_requests: r.get(6)?,
                input_tokens: r.get(7)?,
                sent_tokens: r.get(8)?,
                measured_input_tokens: r.get(9)?,
                output_tokens: r.get(10)?,
                saved_input_tokens: r.get(11)?,
                total_cost: r.get(12)?,
                last_seen: r.get(13)?,
                status: r.get(14)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn trends(&self, since: i64) -> Result<Vec<TrendPoint>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        let mut statement = conn.prepare(
            "SELECT strftime('%Y-%m-%d', created_at, 'unixepoch') AS day, COUNT(*), COALESCE(SUM(input_tokens),0), COALESCE(SUM(sent_bytes)/4,0), COALESCE(SUM(CASE WHEN input_bytes > sent_bytes THEN (input_bytes-sent_bytes)/4 ELSE 0 END),0), COALESCE(SUM(measured_input_tokens),0), COALESCE(SUM(cost),0), COALESCE(SUM(CASE WHEN transform_status='applied' THEN 1 ELSE 0 END),0), COALESCE(SUM(CASE WHEN status IN ('budget_blocked','loop_blocked','rate_limited','concurrency_blocked') THEN 1 ELSE 0 END),0) FROM requests WHERE created_at >= ?1 GROUP BY day ORDER BY day ASC",
        )?;
        let rows = statement.query_map(params![since], |row| {
            Ok(TrendPoint {
                day: row.get(0)?,
                requests: row.get(1)?,
                input_tokens: row.get(2)?,
                sent_tokens: row.get(3)?,
                saved_input_tokens: row.get(4)?,
                measured_input_tokens: row.get(5)?,
                cost: row.get(6)?,
                transformed_requests: row.get(7)?,
                blocked_requests: row.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn purge_before(&self, cutoff: i64) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("账本锁已损坏"))?;
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM transformation_receipts WHERE request_id IN (SELECT id FROM requests WHERE created_at < ?1)",
            params![cutoff],
        )?;
        tx.execute(
            "DELETE FROM provider_attempts WHERE request_id IN (SELECT id FROM requests WHERE created_at < ?1)",
            params![cutoff],
        )?;
        tx.execute(
            "DELETE FROM control_events WHERE created_at < ?1",
            params![cutoff],
        )?;
        let requests = tx.execute(
            "DELETE FROM requests WHERE created_at < ?1",
            params![cutoff],
        )?;
        tx.commit()?;
        Ok(requests)
    }
}
